use crate::{
    pacing::{idle_tick_due, multiply_divide_round_up},
    tracked_samples::SamplePool,
    wcap::*,
    wcap_tex_resize::*,
    wcap_yuv_convert::*,
};
use std::mem::ManuallyDrop;
use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::{
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
        Media::{Audio::WAVEFORMATEX, MediaFoundation::*},
        System::{Com::*, Variant::VARIANT},
    },
};

fn packed(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}
impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            encoder_release(self);
        }
    }
}
fn media_time(time: u64, start: u64, frequency: u64) -> i64 {
    multiply_divide_round_up(time.saturating_sub(start), MF_UNITS_PER_SECOND, frequency)
        .min(i64::MAX as u64) as i64
}
pub fn output_dimensions(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    let mut w = u64::from(width.max(2));
    let mut h = u64::from(height.max(2));
    if max_width > 0 && w > max_width as u64 {
        h = (h * max_width as u64 / w).max(2);
        w = max_width as u64;
    }
    if max_height > 0 && h > max_height as u64 {
        w = (w * max_height as u64 / h).max(2);
        h = max_height as u64;
    }
    (((w.max(2) + 1) & !1) as u32, ((h.max(2) + 1) & !1) as u32)
}

pub unsafe fn encoder_init(encoder: &mut Encoder) {
    if !encoder.mf_initialized {
        match MFStartup(MF_VERSION, MFSTARTUP_LITE) {
            Ok(()) => encoder.mf_initialized = true,
            Err(e) => encoder.last_error = Some(e),
        }
    }
}
pub unsafe fn encoder_release(encoder: &mut Encoder) {
    encoder_stop(encoder);
    if encoder.mf_initialized {
        let _ = MFShutdown();
        encoder.mf_initialized = false;
    }
}

unsafe fn attributes(count: u32) -> Result<IMFAttributes> {
    let mut a = None;
    MFCreateAttributes(&mut a, count)?;
    a.ok_or_else(|| Error::from_hresult(E_FAIL))
}

unsafe fn ensure_video_encoder(
    device: &ID3D11Device,
    cfg: &Config,
    input: GUID,
    output: GUID,
) -> Result<()> {
    let attrs = attributes(1)?;
    let mut flags = MFT_ENUM_FLAG_SORTANDFILTER;
    if cfg.hardware_encoder.as_bool() {
        let dxgi: IDXGIDevice = device.cast()?;
        let desc = dxgi.GetAdapter()?.GetDesc()?;
        let luid = std::slice::from_raw_parts(
            &desc.AdapterLuid as *const _ as *const u8,
            std::mem::size_of::<LUID>(),
        );
        attrs.SetBlob(&MFT_ENUM_ADAPTER_LUID, luid)?;
        flags |= MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_HARDWARE;
    } else {
        flags |= MFT_ENUM_FLAG_SYNCMFT;
    }
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: input,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: output,
    };
    let mut list = std::ptr::null_mut();
    let mut count = 0;
    let result = MFTEnum2(
        MFT_CATEGORY_VIDEO_ENCODER,
        flags,
        Some(&input),
        Some(&output),
        &attrs,
        &mut list,
        &mut count,
    );
    if !list.is_null() {
        for i in 0..count as usize {
            drop(list.add(i).read());
        }
        CoTaskMemFree(Some(list as _));
    }
    result?;
    if count == 0 {
        return Err(Error::from_hresult(MF_E_TOPO_CODEC_NOT_FOUND));
    }
    Ok(())
}

pub unsafe fn encoder_start(
    encoder: &mut Encoder,
    device: &ID3D11Device,
    filename: &[u16],
    config: &EncoderConfig,
) -> BOOL {
    encoder_stop(encoder);
    encoder.last_error = None;
    let result = (|| -> Result<()> {
        let cfg = config
            .config
            .as_ref()
            .ok_or_else(|| Error::from_hresult(E_POINTER))?;
        if config.width == 0
            || config.height == 0
            || config.framerate_num == 0
            || config.framerate_den == 0
            || filename.last() != Some(&0)
        {
            return Err(Error::from_hresult(E_INVALIDARG));
        }
        let context = device.GetImmediateContext()?;
        let multithread: ID3D11Multithread = context.cast()?;
        let _ = multithread.SetMultithreadProtected(true);
        let (width, height) = output_dimensions(
            config.width,
            config.height,
            cfg.video_max_width,
            cfg.video_max_height,
        );
        let (input_format, codec, profile) = match (cfg.video_codec, cfg.video_profile) {
            (CONFIG_VIDEO_H264, CONFIG_VIDEO_BASE) => (
                MFVideoFormat_NV12,
                MFVideoFormat_H264,
                eAVEncH264VProfile_Base.0,
            ),
            (CONFIG_VIDEO_H264, CONFIG_VIDEO_MAIN) => (
                MFVideoFormat_NV12,
                MFVideoFormat_H264,
                eAVEncH264VProfile_Main.0,
            ),
            (CONFIG_VIDEO_H264, CONFIG_VIDEO_HIGH) => (
                MFVideoFormat_NV12,
                MFVideoFormat_H264,
                eAVEncH264VProfile_High.0,
            ),
            (CONFIG_VIDEO_H265, CONFIG_VIDEO_MAIN) => (
                MFVideoFormat_NV12,
                MFVideoFormat_HEVC,
                eAVEncH265VProfile_Main_420_8.0,
            ),
            (CONFIG_VIDEO_H265, CONFIG_VIDEO_MAIN_10) => (
                MFVideoFormat_P010,
                MFVideoFormat_HEVC,
                eAVEncH265VProfile_Main_420_10.0,
            ),
            (CONFIG_VIDEO_AV1, CONFIG_VIDEO_MAIN) => (
                MFVideoFormat_NV12,
                MFVideoFormat_AV1,
                eAVEncAV1VProfile_Main_420_8.0,
            ),
            (CONFIG_VIDEO_AV1, CONFIG_VIDEO_MAIN_10) => (
                MFVideoFormat_P010,
                MFVideoFormat_AV1,
                eAVEncAV1VProfile_Main_420_10.0,
            ),
            _ => return Err(Error::from_hresult(E_INVALIDARG)),
        };
        ensure_video_encoder(device, cfg, input_format, codec)?;
        let attrs = attributes(4)?;
        if cfg.hardware_encoder.as_bool() {
            attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
            let mut token = 0;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.unwrap();
            manager.ResetDevice(device, token)?;
            attrs.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, &manager)?;
        }
        attrs.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;
        let container = if cfg.video_codec == CONFIG_VIDEO_H264 && cfg.fragmented_output.as_bool() {
            MFTranscodeContainerType_FMPEG4
        } else {
            MFTranscodeContainerType_MPEG4
        };
        attrs.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &container)?;
        let file = MFCreateFile(
            MF_ACCESSMODE_WRITE,
            MF_OPENMODE_FAIL_IF_EXIST,
            MF_FILEFLAGS_NONE,
            PCWSTR(filename.as_ptr()),
        )?;
        let writer = MFCreateSinkWriterFromURL(PCWSTR::null(), &file, &attrs)?;
        encoder.writer = Some(writer.clone());
        encoder.video_stream_index = -1;
        encoder.audio_stream_index = -1;
        let hd = width >= 1280 || height > 576 || cfg.video_codec != CONFIG_VIDEO_H264;
        let output = MFCreateMediaType()?;
        output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        output.SetGUID(&MF_MT_SUBTYPE, &codec)?;
        output.SetUINT32(&MF_MT_MPEG2_PROFILE, profile as u32)?;
        output.SetUINT32(
            &MF_MT_VIDEO_CHROMA_SITING,
            MFVideoChromaSubsampling_MPEG2.0 as u32,
        )?;
        output.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_Wide.0 as u32)?;
        output.SetUINT32(
            &MF_MT_VIDEO_PRIMARIES,
            if hd {
                MFVideoPrimaries_BT709.0
            } else {
                MFVideoPrimaries_SMPTE170M.0
            } as u32,
        )?;
        output.SetUINT32(
            &MF_MT_YUV_MATRIX,
            if hd {
                MFVideoTransferMatrix_BT709.0
            } else {
                MFVideoTransferMatrix_BT601.0
            } as u32,
        )?;
        output.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)?;
        output.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        output.SetUINT64(
            &MF_MT_FRAME_RATE,
            packed(config.framerate_num, config.framerate_den),
        )?;
        output.SetUINT64(&MF_MT_FRAME_SIZE, packed(width, height))?;
        output.SetUINT32(&MF_MT_AVG_BITRATE, cfg.video_bitrate.saturating_mul(1000))?;
        encoder.video_stream_index = writer.AddStream(&output)? as i32;
        let input = MFCreateMediaType()?;
        input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        input.SetGUID(&MF_MT_SUBTYPE, &input_format)?;
        input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        input.SetUINT64(
            &MF_MT_FRAME_RATE,
            packed(config.framerate_num, config.framerate_den),
        )?;
        input.SetUINT64(&MF_MT_FRAME_SIZE, packed(width, height))?;
        writer.SetInputMediaType(encoder.video_stream_index as u32, &input, None)?;
        let mut api = std::ptr::null_mut();
        if writer
            .GetServiceForStream(
                encoder.video_stream_index as u32,
                &GUID::zeroed(),
                &ICodecAPI::IID,
                &mut api,
            )
            .is_ok()
            && !api.is_null()
        {
            let api = ICodecAPI::from_raw(api);
            let _ = api.SetValue(
                &CODECAPI_AVEncCommonRateControlMode,
                &VARIANT::from(eAVEncCommonRateControlMode_UnconstrainedVBR.0 as u32),
            );
            let _ = api.SetValue(
                &CODECAPI_AVEncCommonMeanBitRate,
                &VARIANT::from(cfg.video_bitrate.saturating_mul(1000)),
            );
            let _ = api.SetValue(
                &CODECAPI_AVEncMPVGOPSize,
                &VARIANT::from(multiply_divide_round_up(
                    4,
                    config.framerate_num as u64,
                    config.framerate_den as u64,
                ) as u32),
            );
            let _ = api.SetValue(&CODECAPI_AVLowLatencyMode, &VARIANT::from(false));
            let _ = api.SetValue(&CODECAPI_AVEncMPVDefaultBPictureCount, &VARIANT::from(2u32));
        }
        if let Some(format) = config.audio_format.as_ref() {
            if cfg.audio_channels == 0 || cfg.audio_samplerate == 0 {
                return Err(Error::from_hresult(E_INVALIDARG));
            }
            let resampler: IMFTransform =
                CoCreateInstance(&CResamplerMediaObject, None, CLSCTX_INPROC_SERVER)?;
            let input = MFCreateMediaType()?;
            MFInitMediaTypeFromWaveFormatEx(
                &input,
                format,
                std::mem::size_of::<WAVEFORMATEX>() as u32 + format.cbSize as u32,
            )?;
            resampler.SetInputType(0, &input, 0)?;
            let pcm = WAVEFORMATEX {
                wFormatTag: 1,
                nChannels: cfg.audio_channels as u16,
                nSamplesPerSec: cfg.audio_samplerate,
                nAvgBytesPerSec: cfg.audio_samplerate * cfg.audio_channels * 2,
                nBlockAlign: (cfg.audio_channels * 2) as u16,
                wBitsPerSample: 16,
                cbSize: 0,
            };
            let pcm_type = MFCreateMediaType()?;
            MFInitMediaTypeFromWaveFormatEx(
                &pcm_type,
                &pcm,
                std::mem::size_of::<WAVEFORMATEX>() as u32,
            )?;
            resampler.SetOutputType(0, &pcm_type, 0)?;
            resampler.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            let output = MFCreateMediaType()?;
            output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            output.SetGUID(
                &MF_MT_SUBTYPE,
                if cfg.audio_codec == CONFIG_AUDIO_FLAC {
                    &MFAudioFormat_FLAC
                } else {
                    &MFAudioFormat_AAC
                },
            )?;
            output.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            output.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, cfg.audio_samplerate)?;
            output.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, cfg.audio_channels)?;
            if cfg.audio_codec == CONFIG_AUDIO_AAC {
                output.SetUINT32(
                    &MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
                    cfg.audio_bitrate * 1000 / 8,
                )?;
            }
            encoder.audio_stream_index = writer.AddStream(&output)? as i32;
            writer.SetInputMediaType(encoder.audio_stream_index as u32, &pcm_type, None)?;
            let mut samples = Vec::new();
            for _ in 0..ENCODER_AUDIO_BUFFER_COUNT {
                let tracked = MFCreateTrackedSample()?;
                let sample: IMFSample = tracked.cast()?;
                let buffer = MFCreateMemoryBuffer(cfg.audio_samplerate * cfg.audio_channels * 2)?;
                sample.AddBuffer(&buffer)?;
                samples.push(sample);
            }
            encoder.audio_pool = SamplePool::new(samples);
            encoder.audio_frame_size = format.nBlockAlign as u32;
            encoder.audio_sample_rate = format.nSamplesPerSec;
            encoder.resampler = Some(resampler);
        }
        tex_resize_create(
            &mut encoder.resize,
            device,
            (config.width + 1) & !1,
            (config.height + 1) & !1,
            width,
            height,
            cfg.gamma_correct_resize.as_bool(),
            D3D11_BIND_RENDER_TARGET,
        );
        let view_desc = D3D11_RENDER_TARGET_VIEW_DESC {
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
            ..Default::default()
        };
        device.CreateRenderTargetView(
            encoder.resize.input_texture.as_ref().unwrap(),
            Some(&view_desc),
            Some(&mut encoder.input_view),
        )?;
        context.ClearRenderTargetView(encoder.input_view.as_ref().unwrap(), &[0.0; 4]);
        yuv_convert_create(
            &mut encoder.convert,
            device,
            encoder.resize.output_texture.as_ref().unwrap(),
            width,
            height,
            if hd {
                YUV_COLOR_SPACE_BT709
            } else {
                YUV_COLOR_SPACE_BT601
            },
            cfg.improved_color_conversion.as_bool(),
        );
        let mut samples = Vec::new();
        for output in &mut encoder.convert_output {
            yuv_convert_output_create(
                output,
                device,
                width,
                height,
                if input_format == MFVideoFormat_NV12 {
                    DXGI_FORMAT_NV12
                } else {
                    DXGI_FORMAT_P010
                },
            );
            let sample = MFCreateVideoSampleFromSurface(None)?;
            let buffer = MFCreateDXGISurfaceBuffer(
                &ID3D11Texture2D::IID,
                output.texture.as_ref().unwrap(),
                0,
                false,
            )?;
            buffer.SetCurrentLength(buffer.GetMaxLength()?)?;
            sample.AddBuffer(&buffer)?;
            samples.push(sample);
        }
        encoder.video_pool = SamplePool::new(samples);
        encoder.context = Some(context);
        encoder.multithread = Some(multithread);
        encoder.input_width = (config.width + 1) & !1;
        encoder.input_height = (config.height + 1) & !1;
        encoder.output_width = width;
        encoder.output_height = height;
        encoder.framerate_num = config.framerate_num;
        encoder.framerate_den = config.framerate_den;
        encoder.start_time = 0;
        encoder.video_last_time = 0;
        encoder.last_video_timestamp = None;
        encoder.video_discontinuity = false.into();
        writer.BeginWriting()?;
        Ok(())
    })();
    if let Err(e) = result {
        encoder_stop(encoder);
        encoder.last_error = Some(e);
        return false.into();
    }
    true.into()
}

unsafe fn output_audio(encoder: &mut Encoder) -> Result<()> {
    let Some(resampler) = encoder.resampler.as_ref() else {
        return Ok(());
    };
    loop {
        let lease = encoder.audio_pool.wait_take()?;
        let buffer = lease.sample().GetBufferByIndex(0)?;
        buffer.SetCurrentLength(0)?;
        let mut output = MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: 0,
            pSample: ManuallyDrop::new(Some(lease.sample().clone())),
            ..Default::default()
        };
        let mut status = 0;
        let result = resampler.ProcessOutput(0, std::slice::from_mut(&mut output), &mut status);
        drop(ManuallyDrop::take(&mut output.pSample));
        drop(ManuallyDrop::take(&mut output.pEvents));
        match result {
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
            Err(e) => return Err(e),
            Ok(()) => lease.submit(
                encoder
                    .writer
                    .as_ref()
                    .ok_or_else(|| Error::from_hresult(E_FAIL))?,
                encoder.audio_stream_index as u32,
            )?,
        }
    }
}

pub unsafe fn encoder_stop(encoder: &mut Encoder) {
    if let Some(resampler) = encoder.resampler.as_ref() {
        let result = resampler
            .ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)
            .and_then(|_| output_audio(encoder));
        if let Err(e) = result {
            encoder.last_error = Some(e);
        }
    }
    if let Some(writer) = encoder.writer.take() {
        if let Err(e) = writer.Finalize() {
            encoder.last_error = Some(e);
        }
    }
    encoder.resampler = None;
    encoder.video_pool = SamplePool::default();
    encoder.audio_pool = SamplePool::default();
    for output in &mut encoder.convert_output {
        yuv_convert_output_release(output);
    }
    yuv_convert_release(&mut encoder.convert);
    tex_resize_release(&mut encoder.resize);
    if let Some(context) = encoder.context.take() {
        context.ClearState();
    }
    encoder.input_view = None;
    encoder.multithread = None;
    encoder.start_time = 0;
    encoder.last_video_timestamp = None;
}

unsafe fn send_gap(encoder: &mut Encoder, time: u64, frequency: u64) -> Result<()> {
    if encoder.start_time == 0 {
        return Ok(());
    }
    let timestamp = media_time(time, encoder.start_time, frequency);
    if encoder
        .last_video_timestamp
        .is_some_and(|last| timestamp <= last)
    {
        return Ok(());
    }
    if let Some(writer) = &encoder.writer {
        writer.SendStreamTick(encoder.video_stream_index as u32, timestamp)?;
    }
    encoder.last_video_timestamp = Some(timestamp);
    encoder.video_discontinuity = true.into();
    Ok(())
}

pub unsafe fn encoder_new_frame(
    encoder: &mut Encoder,
    texture: &ID3D11Texture2D,
    rect: RECT,
    time: u64,
    frequency: u64,
) -> BOOL {
    if frequency == 0
        || time <= encoder.video_last_time
        || rect.left < 0
        || rect.top < 0
        || rect.right <= rect.left
        || rect.bottom <= rect.top
    {
        return false.into();
    }
    let timestamp = if encoder.start_time == 0 {
        0
    } else {
        media_time(time, encoder.start_time, frequency)
    };
    if encoder
        .last_video_timestamp
        .is_some_and(|last| timestamp <= last)
    {
        return false.into();
    }
    encoder.video_last_time = time;
    let Some(lease) = encoder.video_pool.try_take() else {
        if let Err(e) = send_gap(encoder, time, frequency) {
            encoder.last_error = Some(e);
        }
        return false.into();
    };
    let result = (|| -> Result<()> {
        let context = encoder
            .context
            .as_ref()
            .ok_or_else(|| Error::from_hresult(E_FAIL))?;
        let multithread = encoder
            .multithread
            .as_ref()
            .ok_or_else(|| Error::from_hresult(E_FAIL))?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        let width = ((rect.right - rect.left) as u32)
            .min(encoder.input_width)
            .min(desc.Width.saturating_sub(rect.left as u32));
        let height = ((rect.bottom - rect.top) as u32)
            .min(encoder.input_height)
            .min(desc.Height.saturating_sub(rect.top as u32));
        if width == 0 || height == 0 {
            return Err(Error::from_hresult(E_INVALIDARG));
        }
        multithread.Enter();
        struct Guard<'a>(&'a ID3D11Multithread);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                unsafe {
                    self.0.Leave();
                }
            }
        }
        let guard = Guard(multithread);
        if width < encoder.input_width || height < encoder.input_height {
            context.ClearRenderTargetView(encoder.input_view.as_ref().unwrap(), &[0.0; 4]);
        }
        let area = D3D11_BOX {
            left: rect.left as u32,
            top: rect.top as u32,
            front: 0,
            right: rect.left as u32 + width,
            bottom: rect.top as u32 + height,
            back: 1,
        };
        context.CopySubresourceRegion(
            encoder.resize.input_texture.as_ref().unwrap(),
            0,
            0,
            0,
            0,
            texture,
            0,
            Some(&area),
        );
        tex_resize_dispatch(&mut encoder.resize, context);
        yuv_convert_dispatch(
            &mut encoder.convert,
            context,
            &mut encoder.convert_output[lease.index],
        );
        context.Flush();
        drop(guard);
        let sample = lease.sample();
        sample.SetSampleDuration(multiply_divide_round_up(
            encoder.framerate_den as u64,
            MF_UNITS_PER_SECOND,
            encoder.framerate_num as u64,
        ) as i64)?;
        sample.SetSampleTime(timestamp)?;
        if encoder.video_discontinuity.as_bool() {
            sample.SetUINT32(&MFSampleExtension_Discontinuity, 1)?;
        } else {
            let _ = sample.DeleteItem(&MFSampleExtension_Discontinuity);
        }
        lease.submit(
            encoder
                .writer
                .as_ref()
                .ok_or_else(|| Error::from_hresult(E_FAIL))?,
            encoder.video_stream_index as u32,
        )?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            if encoder.start_time == 0 {
                encoder.start_time = time;
            }
            encoder.last_video_timestamp = Some(timestamp);
            encoder.video_discontinuity = false.into();
            true.into()
        }
        Err(e) => {
            encoder.last_error = Some(e);
            false.into()
        }
    }
}

pub unsafe fn encoder_new_samples(
    encoder: &mut Encoder,
    samples: *const std::ffi::c_void,
    count: u32,
    time: u64,
    frequency: u64,
) {
    if count == 0
        || samples.is_null()
        || encoder.start_time == 0
        || time < encoder.start_time
        || frequency == 0
    {
        return;
    }
    let result = (|| -> Result<()> {
        let bytes = count
            .checked_mul(encoder.audio_frame_size)
            .ok_or_else(|| Error::from_hresult(E_INVALIDARG))?;
        let buffer = MFCreateMemoryBuffer(bytes)?;
        let mut ptr = std::ptr::null_mut();
        buffer.Lock(&mut ptr, None, None)?;
        std::ptr::copy_nonoverlapping(samples as *const u8, ptr, bytes as usize);
        buffer.Unlock()?;
        buffer.SetCurrentLength(bytes)?;
        let sample = MFCreateSample()?;
        sample.AddBuffer(&buffer)?;
        sample.SetSampleTime(media_time(time, encoder.start_time, frequency))?;
        sample.SetSampleDuration(multiply_divide_round_up(
            count as u64,
            MF_UNITS_PER_SECOND,
            encoder.audio_sample_rate as u64,
        ) as i64)?;
        output_audio(encoder)?;
        encoder
            .resampler
            .as_ref()
            .ok_or_else(|| Error::from_hresult(E_FAIL))?
            .ProcessInput(0, &sample, 0)?;
        output_audio(encoder)
    })();
    if let Err(e) = result {
        encoder.last_error = Some(e);
    }
}

pub unsafe fn encoder_update(encoder: &mut Encoder, time: u64, frequency: u64) {
    if encoder.start_time != 0 && idle_tick_due(encoder.video_last_time, time, frequency) {
        if let Err(e) = send_gap(encoder, time, frequency) {
            encoder.last_error = Some(e);
        } else {
            encoder.video_last_time = time;
        }
    }
}

pub unsafe fn encoder_get_stats(
    encoder: &mut Encoder,
    bitrate: &mut u32,
    length: &mut u32,
    size: &mut u64,
) {
    *bitrate = 0;
    *length = 0;
    *size = 0;
    let Some(writer) = &encoder.writer else {
        return;
    };
    for index in [encoder.video_stream_index, encoder.audio_stream_index] {
        if index < 0 {
            continue;
        }
        let mut stats = MF_SINK_WRITER_STATISTICS {
            cb: std::mem::size_of::<MF_SINK_WRITER_STATISTICS>() as u32,
            ..Default::default()
        };
        if writer.GetStatistics(index as u32, &mut stats).is_err() {
            continue;
        }
        *size = size.saturating_add(stats.qwByteCountProcessed);
        if stats.llLastTimestampProcessed > 0 {
            let bits = (stats.qwByteCountProcessed as u128 * 8 * MF_UNITS_PER_SECOND as u128
                / (1000 * stats.llLastTimestampProcessed as u128))
                .min(u32::MAX as u128) as u32;
            *bitrate = bitrate.saturating_add(bits);
            if index == encoder.video_stream_index {
                *length = (stats.llLastTimestampProcessed / 10_000).min(u32::MAX as i64) as u32;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use windows::Win32::Graphics::Direct3D::*;
    use windows::Win32::System::Threading::GetCurrentThreadId;

    #[test]
    fn dimensions_preserve_aspect_and_never_upscale() {
        assert_eq!(output_dimensions(1920, 1080, 1280, 720), (1280, 720));
        assert_eq!(output_dimensions(640, 480, 1920, 1080), (640, 480));
        assert_eq!(output_dimensions(1920, 1080, 0, 480), (854, 480));
        assert_eq!(output_dimensions(641, 481, 0, 0), (642, 482));
    }

    #[test]
    fn software_h264_reuses_all_slots_and_finalizes_monotonic_mp4() {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_WARP,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .unwrap();
            let device = device.unwrap();
            let context = context.unwrap();
            let mut texture = None;
            let desc = D3D11_TEXTURE2D_DESC {
                Width: 64,
                Height: 48,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                ..Default::default()
            };
            device
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .unwrap();
            let texture = texture.unwrap();
            let path = std::env::temp_dir().join(format!(
                "wcap-port-{}-{}.mp4",
                std::process::id(),
                GetCurrentThreadId()
            ));
            let filename: Vec<u16> = path
                .as_os_str()
                .to_string_lossy()
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let mut cfg = Config::default();
            cfg.hardware_encoder = false.into();
            cfg.capture_audio = true.into();
            cfg.video_max_width = 0;
            cfg.video_max_height = 0;
            cfg.video_bitrate = 1000;
            let format = WAVEFORMATEX {
                wFormatTag: 1,
                nChannels: 2,
                nSamplesPerSec: 48_000,
                nAvgBytesPerSec: 192_000,
                nBlockAlign: 4,
                wBitsPerSample: 16,
                cbSize: 0,
            };
            let config = EncoderConfig {
                width: 64,
                height: 48,
                framerate_num: 30,
                framerate_den: 1,
                config: &mut cfg,
                audio_format: &format as *const _ as *mut _,
                ..Default::default()
            };
            let mut encoder = Encoder::default();
            encoder_init(&mut encoder);
            assert!(encoder.last_error.is_none());
            std::fs::write(&path, b"existing recording").unwrap();
            assert!(!encoder_start(&mut encoder, &device, &filename, &config).as_bool());
            assert_eq!(std::fs::read(&path).unwrap(), b"existing recording");
            std::fs::remove_file(&path).unwrap();
            assert!(
                encoder_start(&mut encoder, &device, &filename, &config).as_bool(),
                "{:?}",
                encoder.last_error
            );
            let rect = RECT {
                left: 0,
                top: 0,
                right: 64,
                bottom: 48,
            };
            let freq = 10_000_000;
            let mut time = freq;
            for i in 0..40 {
                let deadline = Instant::now() + Duration::from_secs(5);
                while encoder.video_pool.try_take().is_none() {
                    assert!(
                        Instant::now() < deadline,
                        "sample ring never returned a slot after frame {i}"
                    );
                    std::thread::sleep(Duration::from_millis(2));
                }
                let pixels = vec![i as u8 * 5; 64 * 48 * 4];
                context.UpdateSubresource(&texture, 0, None, pixels.as_ptr() as _, 64 * 4, 0);
                assert!(
                    encoder_new_frame(&mut encoder, &texture, rect, time, freq).as_bool(),
                    "{:?}",
                    encoder.last_error
                );
                if i == 0 {
                    assert_eq!(encoder.last_video_timestamp, Some(0));
                }
                let silence = vec![0i16; 1600 * 2];
                encoder_new_samples(&mut encoder, silence.as_ptr() as _, 1600, time, freq);
                assert!(encoder.last_error.is_none(), "{:?}", encoder.last_error);
                assert!(!encoder_new_frame(&mut encoder, &texture, rect, time, freq).as_bool());
                assert!(!encoder_new_frame(&mut encoder, &texture, rect, time - 1, freq).as_bool());
                if i == 20 {
                    encoder_update(&mut encoder, time + freq, freq);
                    assert!(
                        !encoder_new_frame(&mut encoder, &texture, rect, time + 1000, freq)
                            .as_bool()
                    );
                    time += freq;
                }
                time += 333_334;
            }
            encoder_stop(&mut encoder);
            assert!(encoder.last_error.is_none(), "{:?}", encoder.last_error);
            assert!(std::fs::metadata(&path).unwrap().len() > 1000);
            let reader = MFCreateSourceReaderFromURL(PCWSTR(filename.as_ptr()), None).unwrap();
            let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
            let input = reader.GetNativeMediaType(stream, 0).unwrap();
            assert_eq!(input.GetGUID(&MF_MT_SUBTYPE).unwrap(), MFVideoFormat_H264);
            assert_eq!(input.GetUINT64(&MF_MT_FRAME_SIZE).unwrap(), packed(64, 48));
            let decoded = MFCreateMediaType().unwrap();
            decoded
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .unwrap();
            decoded
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
                .unwrap();
            reader.SetCurrentMediaType(stream, None, &decoded).unwrap();
            let mut timestamps = Vec::new();
            let mut luma = Vec::new();
            loop {
                let mut sample = None;
                let mut flags = 0;
                let mut timestamp = 0;
                reader
                    .ReadSample(
                        stream,
                        0,
                        None,
                        Some(&mut flags),
                        Some(&mut timestamp),
                        Some(&mut sample),
                    )
                    .unwrap();
                if let Some(sample) = sample {
                    timestamps.push(timestamp);
                    let buffer = sample.ConvertToContiguousBuffer().unwrap();
                    let mut bytes = std::ptr::null_mut();
                    buffer.Lock(&mut bytes, None, None).unwrap();
                    luma.push(*bytes.add(24 * 64 + 32));
                    buffer.Unlock().unwrap();
                }
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
                assert!(timestamps.len() <= 40);
            }
            assert_eq!(timestamps.len(), 40);
            assert!((0..=666_668).contains(&timestamps[0]));
            assert!(timestamps.windows(2).all(|w| w[0] < w[1]));
            assert!(
                luma.windows(2).all(|w| w[0] < w[1]),
                "GPU slots were reused before encoding completed: {luma:?}"
            );
            assert!(timestamps.windows(2).any(|w| w[1] - w[0] >= 10_000_000));
            let audio = reader
                .GetNativeMediaType(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32, 0)
                .unwrap();
            assert_eq!(audio.GetGUID(&MF_MT_SUBTYPE).unwrap(), MFAudioFormat_AAC);
            assert_eq!(
                audio.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap(),
                48_000
            );
            assert_eq!(audio.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap(), 2);
            drop(reader);
            let reader = MFCreateSourceReaderFromURL(PCWSTR(filename.as_ptr()), None).unwrap();
            let audio_stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
            let pcm = MFCreateMediaType().unwrap();
            pcm.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).unwrap();
            pcm.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).unwrap();
            reader
                .SetCurrentMediaType(audio_stream, None, &pcm)
                .unwrap();
            let mut audio_frames = 0u64;
            let mut packets = 0;
            loop {
                let mut sample = None;
                let mut flags = 0;
                reader
                    .ReadSample(
                        audio_stream,
                        0,
                        None,
                        Some(&mut flags),
                        None,
                        Some(&mut sample),
                    )
                    .unwrap();
                if let Some(sample) = sample {
                    audio_frames += sample
                        .ConvertToContiguousBuffer()
                        .unwrap()
                        .GetCurrentLength()
                        .unwrap() as u64
                        / 4;
                    packets += 1;
                }
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
                assert!(packets < 128);
            }
            assert!(
                audio_frames.abs_diff(64_000) <= 2048,
                "lost audio samples: {audio_frames}"
            );
            drop(reader);
            encoder_release(&mut encoder);
            std::fs::remove_file(path).unwrap();
            CoUninitialize();
        }
    }
}
