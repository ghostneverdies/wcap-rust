pub mod pacing;
pub mod shaders;
pub mod tracked_samples;
#[path = "wcap_types.rs"]
pub mod wcap;
pub mod wcap_audio_capture;
pub mod wcap_config;
pub mod wcap_encoder;
pub mod wcap_screen_capture;
pub mod wcap_tex_resize;
pub mod wcap_yuv_convert;

#[cfg(test)]
mod gpu_tests {
    use super::*;
    use windows::Win32::{
        Foundation::HMODULE,
        Graphics::{Direct3D::*, Direct3D11::*, Dxgi::Common::*},
    };

    #[test]
    fn resize_and_yuv_shaders_create_dispatch_and_release() {
        unsafe {
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
            for gamma in [false, true] {
                let mut resize = wcap::TexResize::default();
                wcap_tex_resize::tex_resize_create(
                    &mut resize,
                    &device,
                    64,
                    48,
                    32,
                    24,
                    gamma,
                    D3D11_BIND_RENDER_TARGET,
                );
                assert!(resize.pass_h.is_some() && resize.pass_v.is_some());
                let white = vec![255u8; 64 * 48 * 4];
                context.UpdateSubresource(
                    resize.input_texture.as_ref().unwrap(),
                    0,
                    None,
                    white.as_ptr() as _,
                    64 * 4,
                    0,
                );
                wcap_tex_resize::tex_resize_dispatch(&mut resize, &context);
                for improved in [false, true] {
                    let mut convert = wcap::YuvConvert::default();
                    wcap_yuv_convert::yuv_convert_create(
                        &mut convert,
                        &device,
                        resize.output_texture.as_ref().unwrap(),
                        32,
                        24,
                        wcap::YUV_COLOR_SPACE_BT709,
                        improved,
                    );
                    for format in [DXGI_FORMAT_NV12, DXGI_FORMAT_P010] {
                        let mut output = wcap::YuvConvertOutput::default();
                        wcap_yuv_convert::yuv_convert_output_create(
                            &mut output,
                            &device,
                            32,
                            24,
                            format,
                        );
                        assert!(output.view_out_y.is_some() && output.view_out_uv.is_some());
                        wcap_yuv_convert::yuv_convert_dispatch(&mut convert, &context, &mut output);
                        let texture = output.texture.as_ref().unwrap();
                        let mut desc = D3D11_TEXTURE2D_DESC::default();
                        texture.GetDesc(&mut desc);
                        desc.Usage = D3D11_USAGE_STAGING;
                        desc.BindFlags = 0;
                        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                        let mut staging = None;
                        device
                            .CreateTexture2D(&desc, None, Some(&mut staging))
                            .unwrap();
                        let staging = staging.unwrap();
                        context.ClearState();
                        context.CopyResource(&staging, texture);
                        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                        context
                            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                            .unwrap();
                        let data = mapped.pData as *const u8;
                        let pitch = mapped.RowPitch as usize;
                        let (y, u, v, scale) = if format == DXGI_FORMAT_NV12 {
                            (
                                *data.add(12 * pitch + 16) as i32,
                                *data.add(30 * pitch + 16) as i32,
                                *data.add(30 * pitch + 17) as i32,
                                1,
                            )
                        } else {
                            let read = |offset| {
                                std::ptr::read_unaligned(data.add(offset) as *const u16) as i32
                            };
                            (
                                read(12 * pitch + 32),
                                read(30 * pitch + 32),
                                read(30 * pitch + 34),
                                257,
                            )
                        };
                        context.Unmap(&staging, 0);
                        assert!(
                            (y - 235 * scale).abs() <= 2 * scale,
                            "white luma: {y}, gamma={gamma}, improved={improved}"
                        );
                        assert!((u - 128 * scale).abs() <= scale, "neutral U: {u}");
                        assert!((v - 128 * scale).abs() <= scale, "neutral V: {v}");
                        wcap_yuv_convert::yuv_convert_output_release(&mut output);
                        assert!(output.texture.is_none());
                    }
                    wcap_yuv_convert::yuv_convert_release(&mut convert);
                    assert!(convert.input_view.is_none());
                }
                wcap_tex_resize::tex_resize_release(&mut resize);
                assert!(resize.input_texture.is_none() && resize.output_texture.is_none());
            }
            let mut resize = wcap::TexResize::default();
            wcap_tex_resize::tex_resize_create(
                &mut resize,
                &device,
                32,
                24,
                32,
                24,
                false,
                D3D11_BIND_RENDER_TARGET,
            );
            assert_eq!(resize.input_texture, resize.output_texture);
            assert!(resize.input_view_in.is_none());
            wcap_tex_resize::tex_resize_release(&mut resize);
            context.ClearState();
            context.Flush();
            device.GetDeviceRemovedReason().unwrap();
        }
    }
}
