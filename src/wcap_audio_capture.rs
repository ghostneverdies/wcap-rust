use crate::wcap::*;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use windows::{
    core::*,
    Wdk::System::SystemServices::RtlGetVersion,
    Win32::{
        Foundation::*,
        Media::{Audio::*, KernelStreaming::*, Multimedia::*},
        System::{
            Com::{StructuredStorage::*, *},
            Memory::*,
            Performance::*,
            SystemInformation::OSVERSIONINFOW,
            Threading::*,
            Variant::VT_BLOB,
        },
        UI::WindowsAndMessaging::GetWindowThreadProcessId,
    },
};
const HEADER_SIZE: usize = 20;

impl Drop for AudioCapture {
    fn drop(&mut self) {
        unsafe {
            audio_capture_stop(self);
        }
    }
}

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct ActivationCompleted {
    ready: Arc<AtomicBool>,
}
impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationCompleted_Impl {
    fn ActivateCompleted(&self, _: Ref<'_, IActivateAudioInterfaceAsyncOperation>) -> Result<()> {
        self.ready.store(true, Ordering::Release);
        Ok(())
    }
}

pub unsafe fn audio_capture_can_capture_application_local() -> bool {
    let mut v = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    RtlGetVersion(&mut v).is_ok()
        && (v.dwMajorVersion > 10 || (v.dwMajorVersion == 10 && v.dwBuildNumber >= 20348))
}

unsafe fn process_client(pid: u32) -> Result<IAudioClient> {
    let activation = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };
    let blob =
        CoTaskMemAlloc(std::mem::size_of_val(&activation)) as *mut AUDIOCLIENT_ACTIVATION_PARAMS;
    if blob.is_null() {
        return Err(Error::from_hresult(E_OUTOFMEMORY));
    }
    blob.write(activation);
    let mut params = PROPVARIANT::default();
    params.Anonymous.Anonymous = std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
        vt: VT_BLOB,
        Anonymous: PROPVARIANT_0_0_0 {
            blob: BLOB {
                cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                pBlobData: blob as _,
            },
        },
        ..Default::default()
    });
    let ready = Arc::new(AtomicBool::new(false));
    let handler: IActivateAudioInterfaceCompletionHandler = ActivationCompleted {
        ready: ready.clone(),
    }
    .into();
    let op = ActivateAudioInterfaceAsync(
        VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
        &IAudioClient::IID,
        Some(&params),
        &handler,
    )?;
    while !ready.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut status = S_OK;
    let mut object = None;
    op.GetActivateResult(&mut status, &mut object)?;
    status.ok()?;
    object.ok_or_else(|| Error::from_hresult(E_FAIL))?.cast()
}

unsafe fn allocate_ring(size: u32) -> Result<*mut u8> {
    let first = VirtualAlloc2(
        None,
        None,
        2 * size as usize,
        MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
        PAGE_NOACCESS.0,
        None,
    ) as *mut u8;
    if first.is_null() {
        return Err(Error::from_win32());
    }
    let second = first.add(size as usize);
    if let Err(e) = VirtualFree(
        first as _,
        size as usize,
        VIRTUAL_FREE_TYPE(MEM_RELEASE.0 | MEM_PRESERVE_PLACEHOLDER.0),
    ) {
        let _ = VirtualFree(first as _, 0, MEM_RELEASE);
        return Err(e);
    }
    let section =
        match CreateFileMappingW(INVALID_HANDLE_VALUE, None, PAGE_READWRITE, 0, size, None) {
            Ok(s) => s,
            Err(e) => {
                let _ = VirtualFree(first as _, 0, MEM_RELEASE);
                let _ = VirtualFree(second as _, 0, MEM_RELEASE);
                return Err(e);
            }
        };
    let v1 = MapViewOfFile3(
        section,
        None,
        Some(first as _),
        0,
        size as usize,
        MEM_REPLACE_PLACEHOLDER,
        PAGE_READWRITE.0,
        None,
    );
    let v2 = if v1.Value.is_null() {
        MEMORY_MAPPED_VIEW_ADDRESS::default()
    } else {
        MapViewOfFile3(
            section,
            None,
            Some(second as _),
            0,
            size as usize,
            MEM_REPLACE_PLACEHOLDER,
            PAGE_READWRITE.0,
            None,
        )
    };
    let error = Error::from_win32();
    let _ = CloseHandle(section);
    if v1.Value.is_null() || v2.Value.is_null() {
        if !v1.Value.is_null() {
            let _ = UnmapViewOfFileEx(v1, UNMAP_VIEW_OF_FILE_FLAGS(0));
        } else {
            let _ = VirtualFree(first as _, 0, MEM_RELEASE);
        }
        let _ = VirtualFree(second as _, 0, MEM_RELEASE);
        return Err(error);
    }
    Ok(first)
}

struct AudioSetup {
    format: usize,
    ring: usize,
    size: u32,
    start: u64,
    frequency: u64,
}
#[derive(Default)]
struct WorkerAudio {
    play: Option<IAudioClient>,
    record: Option<IAudioClient>,
    capture: Option<IAudioCaptureClient>,
    format: *mut WAVEFORMATEX,
    ring: *mut u8,
    size: u32,
    published: bool,
}
impl Drop for WorkerAudio {
    fn drop(&mut self) {
        unsafe {
            if let Some(play) = &self.play {
                let _ = play.Stop();
            }
            if let Some(record) = &self.record {
                let _ = record.Stop();
            }
            self.capture = None;
            self.record = None;
            self.play = None;
            if !self.published {
                free_ring(self.ring, self.size);
                if !self.format.is_null() {
                    CoTaskMemFree(Some(self.format as _));
                }
            }
        }
    }
}
unsafe fn free_ring(ring: *mut u8, size: u32) {
    if !ring.is_null() {
        let _ = UnmapViewOfFileEx(
            MEMORY_MAPPED_VIEW_ADDRESS { Value: ring as _ },
            UNMAP_VIEW_OF_FILE_FLAGS(0),
        );
        let _ = UnmapViewOfFileEx(
            MEMORY_MAPPED_VIEW_ADDRESS {
                Value: ring.add(size as usize) as _,
            },
            UNMAP_VIEW_OF_FILE_FLAGS(0),
        );
    }
}
unsafe fn initialize_worker(pid: Option<u32>, event: HANDLE) -> Result<WorkerAudio> {
    let mut state = WorkerAudio::default();
    let client: IAudioClient;
    if let Some(pid) = pid {
        client = process_client(pid)?;
        let ptr = CoTaskMemAlloc(std::mem::size_of::<WAVEFORMATEXTENSIBLE>())
            as *mut WAVEFORMATEXTENSIBLE;
        if ptr.is_null() {
            return Err(Error::from_hresult(E_OUTOFMEMORY));
        }
        state.format = ptr as _;
        let mut format = WAVEFORMATEXTENSIBLE::default();
        format.Format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
            nChannels: 2,
            nSamplesPerSec: 48_000,
            nAvgBytesPerSec: 384_000,
            nBlockAlign: 8,
            wBitsPerSample: 32,
            cbSize: (std::mem::size_of::<WAVEFORMATEXTENSIBLE>()
                - std::mem::size_of::<WAVEFORMATEX>()) as u16,
        };
        format.Samples.wValidBitsPerSample = 32;
        format.dwChannelMask = SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT;
        format.SubFormat = KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
        ptr.write(format);
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK
                | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY
                | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            MF_UNITS_PER_SECOND as i64,
            0,
            state.format,
            None,
        )?;
    } else {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let play: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let format = play.GetMixFormat()?;
        let init = play.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            0,
            MF_UNITS_PER_SECOND as i64,
            0,
            format,
            None,
        );
        CoTaskMemFree(Some(format as _));
        init?;
        let render: IAudioRenderClient = play.GetService()?;
        let frames = play.GetBufferSize()?;
        render.GetBuffer(frames)?;
        render.ReleaseBuffer(frames, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)?;
        play.Start()?;
        state.play = Some(play);
        client = device.Activate(CLSCTX_ALL, None)?;
        state.format = client.GetMixFormat()?;
        let mut period = 0;
        client.GetDevicePeriod(Some(&mut period), None)?;
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            period,
            0,
            state.format,
            None,
        )?;
    }
    state.capture = Some(client.GetService()?);
    client.SetEventHandle(event)?;
    state.record = Some(client);
    state.size = (*state.format)
        .nAvgBytesPerSec
        .max(65_536)
        .checked_next_power_of_two()
        .ok_or_else(|| Error::from_hresult(E_INVALIDARG))?;
    state.ring = allocate_ring(state.size)?;
    Ok(state)
}

pub unsafe fn audio_capture_start(capture: &mut AudioCapture, window: HWND) -> bool {
    audio_capture_stop(capture);
    let pid = if window.is_invalid() {
        None
    } else {
        let mut pid = 0;
        if GetWindowThreadProcessId(window, Some(&mut pid)) == 0 {
            return false;
        }
        Some(pid)
    };
    let event = match CreateEventW(None, false, false, None) {
        Ok(event) => event,
        Err(_) => return false,
    };
    capture.event = event;
    capture.stop.store(false, Ordering::Release);
    capture.buffer_read.store(0, Ordering::Relaxed);
    capture.buffer_write.store(0, Ordering::Relaxed);
    let stop = capture.stop.clone();
    let read = capture.buffer_read.clone();
    let write = capture.buffer_write.clone();
    let event = event.0 as usize;
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name("wcap-audio".into())
        .spawn(move || {
            let com = CoInitializeEx(None, COINIT_MULTITHREADED);
            if com.is_err() {
                let _ = ready_tx.send(Err(Error::from_hresult(com)));
                stop.store(true, Ordering::Release);
                return;
            }
            let result = (|| -> Result<()> {
                let mut state = initialize_worker(pid, HANDLE(event as _))?;
                let mut qpc = 0;
                let mut frequency = 0;
                QueryPerformanceCounter(&mut qpc)?;
                QueryPerformanceFrequency(&mut frequency)?;
                state.record.as_ref().unwrap().Start()?;
                let setup = AudioSetup {
                    format: state.format as usize,
                    ring: state.ring as usize,
                    size: state.size,
                    start: qpc as u64,
                    frequency: frequency as u64,
                };
                ready_tx
                    .send(Ok(setup))
                    .map_err(|_| Error::from_hresult(E_FAIL))?;
                state.published = true;
                let record = state.capture.as_ref().unwrap();
                let frame_bytes = (*state.format).nBlockAlign as usize;
                let mut task = 0;
                let priority = AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task).ok();
                let mut cursor = 0u32;
                while WaitForSingleObject(HANDLE(event as _), INFINITE) == WAIT_OBJECT_0 {
                    let stopping = stop.load(Ordering::Acquire);
                    if stopping {
                        if let Some(play) = &state.play {
                            let _ = play.Stop();
                        }
                        if let Some(client) = &state.record {
                            let _ = client.Stop();
                        }
                    }
                    loop {
                        let mut data = std::ptr::null_mut();
                        let mut frames = 0;
                        let mut flags = 0;
                        let mut position = 0;
                        let mut timestamp = 0;
                        if record
                            .GetBuffer(
                                &mut data,
                                &mut frames,
                                &mut flags,
                                Some(&mut position),
                                Some(&mut timestamp),
                            )
                            .is_err()
                            || frames == 0
                        {
                            break;
                        }
                        let bytes = frames as usize * frame_bytes;
                        let packet = HEADER_SIZE + bytes;
                        let available = state
                            .size
                            .wrapping_sub(cursor.wrapping_sub(read.load(Ordering::Acquire)))
                            as usize;
                        if packet <= available {
                            let dst = state.ring.add((cursor & (state.size - 1)) as usize);
                            (dst as *mut u32).write_unaligned(frames);
                            (dst.add(4) as *mut u64).write_unaligned(position);
                            (dst.add(12) as *mut u64).write_unaligned(timestamp);
                            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                                std::ptr::write_bytes(dst.add(HEADER_SIZE), 0, bytes);
                            } else {
                                std::ptr::copy_nonoverlapping(data, dst.add(HEADER_SIZE), bytes);
                            }
                            cursor = cursor.wrapping_add(packet as u32);
                            write.store(cursor, Ordering::Release);
                        }
                        record.ReleaseBuffer(frames)?;
                    }
                    if stopping {
                        break;
                    }
                }
                if let Some(priority) = priority {
                    let _ = AvRevertMmThreadCharacteristics(priority);
                }
                Ok(())
            })();
            if let Err(e) = result {
                let _ = ready_tx.send(Err(e));
            }
            stop.store(true, Ordering::Release);
            CoUninitialize();
        });
    capture.thread = match thread {
        Ok(thread) => Some(thread),
        Err(_) => {
            audio_capture_stop(capture);
            return false;
        }
    };
    match ready_rx.recv() {
        Ok(Ok(setup)) => {
            capture.format = setup.format as _;
            capture.buffer = setup.ring as _;
            capture.buffer_size = setup.size;
            capture.start_qpc = setup.start;
            capture.freq = setup.frequency;
            capture.start_pos = 0;
            capture.use_device_timestamp = true;
            capture.check_device_timestamp = pid.is_none();
            true
        }
        _ => {
            audio_capture_stop(capture);
            false
        }
    }
}

pub unsafe fn audio_capture_flush(capture: &mut AudioCapture) {
    capture.stop.store(true, Ordering::Release);
    if !capture.event.is_invalid() {
        let _ = SetEvent(capture.event);
    }
    if let Some(thread) = capture.thread.take() {
        let _ = thread.join();
    }
}
pub unsafe fn audio_capture_stop(capture: &mut AudioCapture) {
    audio_capture_flush(capture);
    if !capture.event.is_invalid() {
        let _ = CloseHandle(capture.event);
        capture.event = HANDLE::default();
    }
    free_ring(capture.buffer, capture.buffer_size);
    capture.buffer = std::ptr::null_mut();
    capture.buffer_size = 0;
    if !capture.format.is_null() {
        CoTaskMemFree(Some(capture.format as _));
        capture.format = std::ptr::null_mut();
    }
}

pub unsafe fn audio_capture_get_data(
    capture: &mut AudioCapture,
    data: &mut AudioCaptureData,
    expected: u64,
) -> bool {
    if capture.buffer.is_null() || capture.format.is_null() {
        return false;
    }
    let read = capture.buffer_read.load(Ordering::Relaxed);
    let available = capture
        .buffer_write
        .load(Ordering::Acquire)
        .wrapping_sub(read) as usize;
    if available < HEADER_SIZE {
        return false;
    }
    let ptr = capture
        .buffer
        .add((read & (capture.buffer_size - 1)) as usize);
    let count = (ptr as *const u32).read_unaligned() as usize;
    let position = (ptr.add(4) as *const u64).read_unaligned();
    let timestamp = (ptr.add(12) as *const u64).read_unaligned();
    let size = HEADER_SIZE + count * (*capture.format).nBlockAlign as usize;
    if available < size || size > capture.buffer_size as usize {
        return false;
    }
    let converted = ((timestamp as u128 * capture.freq as u128) / MF_UNITS_PER_SECOND as u128)
        .min(u64::MAX as u128) as u64;
    if capture.check_device_timestamp && expected != 0 {
        if converted.abs_diff(expected) > capture.freq / 2 {
            capture.use_device_timestamp = false;
        }
        capture.start_pos = position;
        capture.check_device_timestamp = false;
    }
    data.time = if capture.use_device_timestamp {
        converted
    } else {
        capture.start_qpc.saturating_add(
            ((position.saturating_sub(capture.start_pos) as u128 * capture.freq as u128)
                / (*capture.format).nSamplesPerSec as u128)
                .min(u64::MAX as u128) as u64,
        )
    };
    data.count = count;
    data.samples = ptr.add(HEADER_SIZE) as _;
    true
}

pub unsafe fn audio_capture_release_data(capture: &mut AudioCapture, data: &mut AudioCaptureData) {
    if capture.format.is_null() {
        return;
    }
    let size = HEADER_SIZE + data.count * (*capture.format).nBlockAlign as usize;
    capture
        .buffer_read
        .fetch_add(size as u32, Ordering::Release);
    *data = AudioCaptureData::default();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ring_packets_cross_mapping_and_counter_boundaries() {
        unsafe {
            let mut capture = AudioCapture::default();
            capture.format = CoTaskMemAlloc(std::mem::size_of::<WAVEFORMATEX>()) as _;
            capture.format.write(WAVEFORMATEX {
                nChannels: 2,
                nSamplesPerSec: 48_000,
                nBlockAlign: 8,
                ..Default::default()
            });
            capture.buffer_size = 65_536;
            capture.buffer = allocate_ring(capture.buffer_size).unwrap();
            capture.freq = 3_579_545;
            for read in [65_520u32, u32::MAX - 15] {
                let ptr = capture
                    .buffer
                    .add((read & (capture.buffer_size - 1)) as usize);
                (ptr as *mut u32).write_unaligned(4);
                (ptr.add(4) as *mut u64).write_unaligned(123);
                (ptr.add(12) as *mut u64).write_unaligned(20_000_000);
                std::ptr::write_bytes(ptr.add(HEADER_SIZE), 0x5a, 32);
                capture.buffer_read.store(read, Ordering::Relaxed);
                capture
                    .buffer_write
                    .store(read.wrapping_add(52), Ordering::Release);
                capture.use_device_timestamp = true;
                capture.check_device_timestamp = true;
                let mut data = AudioCaptureData::default();
                assert!(audio_capture_get_data(&mut capture, &mut data, 7_159_090));
                assert_eq!(data.time, 7_159_090);
                assert_eq!(data.count, 4);
                assert_eq!(
                    std::slice::from_raw_parts(data.samples as *const u8, 32),
                    &[0x5a; 32]
                );
                assert!(capture.use_device_timestamp);
                audio_capture_release_data(&mut capture, &mut data);
                assert_eq!(
                    capture.buffer_read.load(Ordering::Acquire),
                    read.wrapping_add(52)
                );
                assert!(!audio_capture_get_data(&mut capture, &mut data, 7_159_090));
            }
            audio_capture_stop(&mut capture);
            audio_capture_stop(&mut capture);
        }
    }

    #[test]
    fn process_audio_worker_starts_and_stops_without_borrowing_stack_state() {
        unsafe {
            if !audio_capture_can_capture_application_local() {
                return;
            }
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
            let window = windows::Win32::UI::WindowsAndMessaging::CreateWindowExW(
                Default::default(),
                w!("STATIC"),
                w!("wcap audio test"),
                Default::default(),
                0,
                0,
                0,
                0,
                Some(windows::Win32::UI::WindowsAndMessaging::HWND_MESSAGE),
                None,
                None,
                None,
            )
            .unwrap();
            let mut capture = AudioCapture::default();
            assert!(audio_capture_start(&mut capture, window));
            std::thread::sleep(Duration::from_millis(100));
            assert!(!capture.stop.load(Ordering::Acquire));
            audio_capture_flush(&mut capture);
            assert!(capture.thread.is_none());
            audio_capture_stop(&mut capture);
            assert!(capture.buffer.is_null() && capture.format.is_null());
            windows::Win32::UI::WindowsAndMessaging::DestroyWindow(window).unwrap();
            CoUninitialize();
        }
    }
}
