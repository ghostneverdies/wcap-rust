use crate::wcap::*;
use windows::{
    core::*,
    Foundation::TimeSpan,
    Graphics::{Capture::*, DirectX::DirectXPixelFormat, SizeInt32},
    Wdk::System::SystemServices::RtlGetVersion,
    Win32::{
        Foundation::*,
        Graphics::{Direct3D11::*, Dwm::*, Dxgi::*, Gdi::*},
        System::{
            SystemInformation::OSVERSIONINFOW,
            WinRT::{Direct3D11::*, Graphics::Capture::*, *},
        },
        UI::WindowsAndMessaging::*,
    },
};

unsafe fn supports_build(build: u32) -> bool {
    let mut version = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    RtlGetVersion(&mut version).is_ok()
        && (version.dwMajorVersion > 10
            || (version.dwMajorVersion == 10 && version.dwBuildNumber >= build))
}
pub unsafe fn screen_capture_is_supported() -> bool {
    supports_build(18362)
}
pub unsafe fn screen_capture_can_hide_mouse_cursor() -> bool {
    supports_build(19041)
}
pub unsafe fn screen_capture_can_hide_recording_border() -> bool {
    supports_build(22000)
}
pub unsafe fn screen_capture_can_disable_rounded_corners() -> bool {
    supports_build(22000)
}
pub unsafe fn screen_capture_can_include_secondary_windows() -> bool {
    supports_build(26100)
}

pub unsafe fn screen_capture_create(capture: &mut ScreenCapture, callback_on_thread: bool) {
    capture.callback_on_thread = callback_on_thread;
    if !capture.runtime_initialized {
        if RoInitialize(RO_INIT_SINGLETHREADED).is_err() {
            return;
        }
        capture.runtime_initialized = true;
    }
    capture.item_interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().ok();
    if !callback_on_thread {
        capture.controller = CreateDispatcherQueueController(DispatcherQueueOptions {
            dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
            threadType: DQTYPE_THREAD_CURRENT,
            apartmentType: DQTAT_COM_NONE,
        })
        .ok();
    }
}

pub unsafe fn screen_capture_release(capture: &mut ScreenCapture) {
    screen_capture_stop(capture);
    capture.item_interop = None;
    capture.on_frame_handler = None;
    capture.on_close_handler = None;
    if let Some(controller) = capture.controller.take() {
        let _ = controller.ShutdownQueueAsync();
    }
    if capture.runtime_initialized {
        RoUninitialize();
        capture.runtime_initialized = false;
    }
}

unsafe fn create_pool(
    capture: &mut ScreenCapture,
    device: &ID3D11Device,
    item: GraphicsCaptureItem,
) -> Result<()> {
    let size = item.Size()?;
    if size.Width <= 0 || size.Height <= 0 {
        return Err(Error::from_hresult(E_INVALIDARG));
    }
    let dxgi: IDXGIDevice = device.cast()?;
    let winrt_device = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;
    let pool = if capture.callback_on_thread {
        Direct3D11CaptureFramePool::CreateFreeThreaded(
            &winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            SCREEN_CAPTURE_BUFFER_COUNT as i32,
            size,
        )?
    } else {
        Direct3D11CaptureFramePool::Create(
            &winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            SCREEN_CAPTURE_BUFFER_COUNT as i32,
            size,
        )?
    };
    capture.device = Some(winrt_device);
    capture.item = Some(item);
    capture.frame_pool = Some(pool);
    capture.current_size = size;
    Ok(())
}

pub unsafe fn screen_capture_create_for_window(
    capture: &mut ScreenCapture,
    device: &ID3D11Device,
    window: HWND,
    only_client_area: bool,
    disable_rounded_corners: bool,
) -> bool {
    screen_capture_stop(capture);
    let result = (|| -> Result<()> {
        let interop = capture
            .item_interop
            .as_ref()
            .ok_or_else(|| Error::from_hresult(E_FAIL))?;
        let item: GraphicsCaptureItem = interop.CreateForWindow(window)?;
        create_pool(capture, device, item)?;
        capture.window = window;
        capture.only_client_area = only_client_area;
        capture.rect = capture_rect(
            capture,
            capture.current_size.Width as u32,
            capture.current_size.Height as u32,
        );
        if capture.rect.right <= capture.rect.left || capture.rect.bottom <= capture.rect.top {
            return Err(Error::from_hresult(E_INVALIDARG));
        }
        if disable_rounded_corners {
            let mut preference = DWM_WINDOW_CORNER_PREFERENCE::default();
            if DwmGetWindowAttribute(
                window,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &mut preference as *mut _ as _,
                std::mem::size_of_val(&preference) as u32,
            )
            .is_ok()
            {
                let value = DWMWCP_DONOTROUND;
                if DwmSetWindowAttribute(
                    window,
                    DWMWA_WINDOW_CORNER_PREFERENCE,
                    &value as *const _ as _,
                    std::mem::size_of_val(&value) as u32,
                )
                .is_ok()
                {
                    capture.window_corner_preference = preference;
                    capture.restore_window_corner_preference = true;
                }
            }
        }
        Ok(())
    })();
    if result.is_err() {
        screen_capture_stop(capture);
    }
    result.is_ok()
}

pub unsafe fn screen_capture_create_for_monitor(
    capture: &mut ScreenCapture,
    device: &ID3D11Device,
    monitor: HMONITOR,
    rect: Option<&RECT>,
) -> bool {
    screen_capture_stop(capture);
    let result = (|| -> Result<()> {
        let interop = capture
            .item_interop
            .as_ref()
            .ok_or_else(|| Error::from_hresult(E_FAIL))?;
        let item: GraphicsCaptureItem = interop.CreateForMonitor(monitor)?;
        create_pool(capture, device, item)?;
        capture.window = HWND::default();
        let size = capture.current_size;
        let full = RECT {
            left: 0,
            top: 0,
            right: size.Width,
            bottom: size.Height,
        };
        capture.rect = clip_rect(rect.unwrap_or(&full), size.Width, size.Height);
        if capture.rect.right <= capture.rect.left || capture.rect.bottom <= capture.rect.top {
            return Err(Error::from_hresult(E_INVALIDARG));
        }
        Ok(())
    })();
    if result.is_err() {
        screen_capture_stop(capture);
    }
    result.is_ok()
}

pub unsafe fn screen_capture_start(
    capture: &mut ScreenCapture,
    with_mouse_cursor: bool,
    with_recording_border: bool,
    include_secondary_windows: bool,
) -> bool {
    let result = (|| -> Result<()> {
        let pool = capture
            .frame_pool
            .as_ref()
            .ok_or_else(|| Error::from_hresult(E_FAIL))?;
        let item = capture
            .item
            .as_ref()
            .ok_or_else(|| Error::from_hresult(E_FAIL))?;
        let session = pool.CreateCaptureSession(item)?;
        let _ = session.SetIsCursorCaptureEnabled(with_mouse_cursor);
        let _ = session.SetIsBorderRequired(with_recording_border);
        let _ = session.SetMinUpdateInterval(TimeSpan { Duration: 10_000 });
        let _ = session.SetIncludeSecondaryWindows(include_secondary_windows);
        capture.session = Some(session.clone());
        {
            if let Some(handler) = capture.on_frame_handler.as_ref() {
                capture.on_frame_token = Some(pool.FrameArrived(handler)?);
            }
            if let Some(handler) = capture.on_close_handler.as_ref() {
                capture.on_close_token = Some(item.Closed(handler)?);
            }
        }
        session.StartCapture()
    })();
    if result.is_err() {
        screen_capture_stop(capture);
    }
    result.is_ok()
}

pub unsafe fn screen_capture_stop(capture: &mut ScreenCapture) {
    if let (Some(pool), Some(token)) = (capture.frame_pool.as_ref(), capture.on_frame_token.take())
    {
        let _ = pool.RemoveFrameArrived(token);
    }
    if let (Some(item), Some(token)) = (capture.item.as_ref(), capture.on_close_token.take()) {
        let _ = item.RemoveClosed(token);
    }
    if let Some(session) = capture.session.take() {
        let _ = session.Close();
    }
    if let Some(pool) = capture.frame_pool.take() {
        let _ = pool.Close();
    }
    capture.item = None;
    capture.device = None;
    if capture.restore_window_corner_preference && !capture.window.is_invalid() {
        let value = capture.window_corner_preference;
        let _ = DwmSetWindowAttribute(
            capture.window,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &value as *const _ as _,
            std::mem::size_of_val(&value) as u32,
        );
    }
    capture.restore_window_corner_preference = false;
    capture.window = HWND::default();
    capture.current_size = SizeInt32::default();
    capture.rect = RECT::default();
}

pub unsafe fn screen_capture_get_frame(
    capture: &mut ScreenCapture,
    frame: &mut ScreenCaptureFrame,
) -> bool {
    let result = (|| -> Result<()> {
        let pool = capture
            .frame_pool
            .as_ref()
            .ok_or_else(|| Error::from_hresult(E_FAIL))?;
        let next = pool.TryGetNextFrame()?;
        let result = (|| -> Result<()> {
            let time = next.SystemRelativeTime()?;
            let size = next.ContentSize()?;
            let access: IDirect3DDxgiInterfaceAccess = next.Surface()?.cast()?;
            let texture: ID3D11Texture2D = access.GetInterface()?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            let rect = capture_rect(
                capture,
                (size.Width.max(0) as u32).min(desc.Width),
                (size.Height.max(0) as u32).min(desc.Height),
            );
            if rect.right <= rect.left || rect.bottom <= rect.top {
                return Err(Error::from_hresult(E_FAIL));
            }
            *frame = ScreenCaptureFrame {
                texture: Some(texture),
                time: wgc_timestamp_to_qpc(time.Duration, qpc_frequency()),
                rect,
                width: size.Width.max(0) as u32,
                height: size.Height.max(0) as u32,
                next_frame: Some(next.cast()?),
            };
            Ok(())
        })();
        if result.is_err() {
            let _ = next.Close();
        }
        result
    })();
    result.is_ok()
}

pub unsafe fn screen_capture_release_frame(
    capture: &mut ScreenCapture,
    frame: &mut ScreenCaptureFrame,
) {
    frame.texture = None;
    if let Some(next) = frame.next_frame.take() {
        if let Ok(closable) = next.cast::<windows::Foundation::IClosable>() {
            let _ = closable.Close();
        }
    }
    let size = SizeInt32 {
        Width: frame.width as i32,
        Height: frame.height as i32,
    };
    if size.Width > 0 && size.Height > 0 && size != capture.current_size {
        if let (Some(pool), Some(device)) = (&capture.frame_pool, &capture.device) {
            if pool
                .Recreate(
                    device,
                    DirectXPixelFormat::B8G8R8A8UIntNormalized,
                    SCREEN_CAPTURE_BUFFER_COUNT as i32,
                    size,
                )
                .is_ok()
            {
                capture.current_size = size;
            }
        }
    }
}

fn clip_rect(rect: &RECT, width: i32, height: i32) -> RECT {
    RECT {
        left: rect.left.clamp(0, width.max(0)),
        top: rect.top.clamp(0, height.max(0)),
        right: rect.right.clamp(0, width.max(0)),
        bottom: rect.bottom.clamp(0, height.max(0)),
    }
}

unsafe fn capture_rect(capture: &ScreenCapture, width: u32, height: u32) -> RECT {
    if capture.window.is_invalid() {
        return clip_rect(&capture.rect, width as i32, height as i32);
    }
    let mut bounds = RECT::default();
    if DwmGetWindowAttribute(
        capture.window,
        DWMWA_EXTENDED_FRAME_BOUNDS,
        &mut bounds as *mut _ as _,
        std::mem::size_of::<RECT>() as u32,
    )
    .is_err()
    {
        return RECT::default();
    }
    let rect = if capture.only_client_area {
        let mut client = RECT::default();
        let mut origin = POINT::default();
        if GetClientRect(capture.window, &mut client).is_err()
            || !ClientToScreen(capture.window, &mut origin).as_bool()
        {
            return RECT::default();
        }
        let left = (origin.x - bounds.left).max(0);
        let top = (origin.y - bounds.top).max(0);
        RECT {
            left,
            top,
            right: left.saturating_add(client.right),
            bottom: top.saturating_add(client.bottom),
        }
    } else {
        RECT {
            left: 0,
            top: 0,
            right: bounds.right - bounds.left,
            bottom: bounds.bottom - bounds.top,
        }
    };
    clip_rect(&rect, width as i32, height as i32)
}

fn wgc_timestamp_to_qpc(time_100ns: i64, qpc_frequency: u64) -> u64 {
    if time_100ns <= 0 || qpc_frequency == 0 {
        return 0;
    }
    let ticks = (time_100ns as u128 * qpc_frequency as u128 + 5_000_000) / 10_000_000;
    ticks.min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamp_conversion() {
        assert_eq!(wgc_timestamp_to_qpc(10_000_000, 3_579_545), 3_579_545);
        assert_eq!(wgc_timestamp_to_qpc(-1, 10_000_000), 0);
        assert_eq!(wgc_timestamp_to_qpc(10, 0), 0);
        assert_eq!(wgc_timestamp_to_qpc(i64::MAX, u64::MAX), u64::MAX);
    }
    #[test]
    fn crops_stay_inside_texture_after_resize() {
        let rect = clip_rect(
            &RECT {
                left: -10,
                top: 20,
                right: 800,
                bottom: 600,
            },
            640,
            480,
        );
        assert_eq!(
            (rect.left, rect.top, rect.right, rect.bottom),
            (0, 20, 640, 480)
        );
        let rect = clip_rect(
            &RECT {
                left: 700,
                top: 500,
                right: 800,
                bottom: 600,
            },
            640,
            480,
        );
        assert_eq!(
            (rect.left, rect.top, rect.right, rect.bottom),
            (640, 480, 640, 480)
        );
    }
}

fn qpc_frequency() -> u64 {
    static FREQUENCY: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *FREQUENCY.get_or_init(|| {
        let mut frequency = 0i64;
        unsafe {
            let _ = windows::Win32::System::Performance::QueryPerformanceFrequency(&mut frequency);
        }
        if frequency > 0 {
            frequency as u64
        } else {
            10_000_000
        }
    })
}
