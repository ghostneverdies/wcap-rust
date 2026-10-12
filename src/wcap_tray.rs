use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        UI::{Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
    },
};

use crate::wcap::*;
use crate::wcap_engine::EngineEvent;

pub const CMD_WCAP: usize = 1;
pub const CMD_QUIT: usize = 2;
pub const CMD_SETTINGS: usize = 3;

const TRAY_ICON_ID: u32 = 1;
const RECORDING_TIMER_ID: usize = 3;
const WM_CAPTURE_EVENTS: u32 = WM_USER + 5;

static WM_TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static SHOW_SETTINGS: AtomicBool = AtomicBool::new(false);
static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);
static RECORD_REGION: AtomicBool = AtomicBool::new(false);
static TRAY_RECORDING: AtomicBool = AtomicBool::new(false);
static TRAY_TITLE: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
static mut IDLE_ICON: HICON = HICON(core::ptr::null_mut());
static mut RECORD_ICON: HICON = HICON(core::ptr::null_mut());
static mut TRAY_WINDOW: HWND = HWND(core::ptr::null_mut());

pub fn take_settings_request() -> bool {
    SHOW_SETTINGS.swap(false, Ordering::Relaxed)
}

pub fn take_quit_request() -> bool {
    QUIT_REQUESTED.swap(false, Ordering::Relaxed)
}

pub fn take_region_request() -> bool {
    RECORD_REGION.swap(false, Ordering::Relaxed)
}

pub unsafe fn init(instance: HINSTANCE, config: &Config) -> bool {
    let window_class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(tray_window_proc),
        hInstance: instance,
        lpszClassName: w!("wcap_tray_window_class"),
        ..Default::default()
    };
    if RegisterClassExW(&window_class) == 0 {
        return false;
    }

    let wm_taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));
    WM_TASKBAR_CREATED.store(wm_taskbar_created, Ordering::Relaxed);

    let window = match CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        w!("wcap_tray_window_class"),
        w!("wcap"),
        WS_POPUP,
        0,
        0,
        0,
        0,
        None,
        None,
        Some(instance),
        None,
    ) {
        Ok(w) => w,
        Err(_) => return false,
    };
    TRAY_WINDOW = window;

    IDLE_ICON = embedded_icon(include_bytes!("../assets/wcap.ico"));
    RECORD_ICON = embedded_icon(include_bytes!("../assets/wcap-recording.ico"));
    add_tray_icon(window);
    register_hotkeys(window, config);
    true
}

pub unsafe fn dispose() {
    let window = TRAY_WINDOW;
    if !window.0.is_null() {
        let _ = KillTimer(Some(window), RECORDING_TIMER_ID);
        remove_tray_icon(window);
        let _ = DestroyWindow(window);
        TRAY_WINDOW = HWND::default();
        for icon in [IDLE_ICON, RECORD_ICON] {
            if !icon.0.is_null() {
                let _ = DestroyIcon(icon);
            }
        }
        IDLE_ICON = HICON::default();
        RECORD_ICON = HICON::default();
    }
}

pub unsafe fn set_recording_timer(recording: bool) {
    TRAY_RECORDING.store(recording, Ordering::Relaxed);
    set_title(if recording {
        "wcap - 0:00:00"
    } else {
        "wcap - Ready"
    });
    let data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: TRAY_WINDOW,
        uID: TRAY_ICON_ID,
        uFlags: NIF_ICON,
        hIcon: if recording { RECORD_ICON } else { IDLE_ICON },
        ..Default::default()
    };
    if !TRAY_WINDOW.0.is_null() {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
    let window = TRAY_WINDOW;
    if window.0.is_null() {
        return;
    }
    if recording {
        SetTimer(Some(window), RECORDING_TIMER_ID, 20, None);
    } else {
        let _ = KillTimer(Some(window), RECORDING_TIMER_ID);
    }
}

pub unsafe fn wake_capture_events() {
    let window = TRAY_WINDOW;
    if !window.0.is_null() {
        let _ = PostMessageW(Some(window), WM_CAPTURE_EVENTS, WPARAM(0), LPARAM(0));
    }
}

unsafe fn embedded_icon(bytes: &[u8]) -> HICON {
    let length = u32::from_le_bytes(bytes[14..18].try_into().unwrap()) as usize;
    let offset = u32::from_le_bytes(bytes[18..22].try_into().unwrap()) as usize;
    CreateIconFromResourceEx(
        &bytes[offset..offset + length],
        true,
        0x00030000,
        GetSystemMetrics(SM_CXSMICON),
        GetSystemMetrics(SM_CYSMICON),
        LR_DEFAULTCOLOR,
    )
    .expect("embedded tray icon")
}
unsafe fn add_tray_icon(window: HWND) {
    let icon = if TRAY_RECORDING.load(Ordering::Relaxed) {
        RECORD_ICON
    } else {
        IDLE_ICON
    };
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: TRAY_ICON_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_WCAP_COMMAND,
        hIcon: icon,
        ..Default::default()
    };
    let title = TRAY_TITLE.lock().unwrap_or_else(|p| p.into_inner());
    str_copy_w(
        &mut data.szTip,
        if title.is_empty() {
            "wcap - Ready"
        } else {
            &title
        },
    );
    let _ = Shell_NotifyIconW(NIM_ADD, &data);
}

unsafe fn remove_tray_icon(window: HWND) {
    let data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: TRAY_ICON_ID,
        ..Default::default()
    };
    let _ = Shell_NotifyIconW(NIM_DELETE, &data);
}

pub unsafe fn notify(title: &str, body: &str, flags: NOTIFY_ICON_INFOTIP_FLAGS) {
    let window = TRAY_WINDOW;
    if window.0.is_null() {
        return;
    }
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: TRAY_ICON_ID,
        uFlags: NIF_INFO,
        dwInfoFlags: flags,
        ..Default::default()
    };
    str_copy_w(&mut data.szInfo, body);
    str_copy_w(&mut data.szInfoTitle, title);
    let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
}

pub unsafe fn set_title(title: &str) {
    *TRAY_TITLE.lock().unwrap_or_else(|p| p.into_inner()) = title.to_owned();
    let window = TRAY_WINDOW;
    if window.0.is_null() {
        return;
    }
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: TRAY_ICON_ID,
        uFlags: NIF_TIP,
        ..Default::default()
    };
    str_copy_w(&mut data.szTip, title);
    let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
}

fn str_copy_w(dst: &mut [u16], src: &str) {
    let wide: Vec<u16> = src.encode_utf16().collect();
    let len = wide.len().min(dst.len() - 1);
    dst[..len].copy_from_slice(&wide[..len]);
    dst[len] = 0;
}

fn loword(lparam: LPARAM) -> u32 {
    (lparam.0 as u32) & 0xffff
}

unsafe extern "system" fn tray_window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let wm_taskbar_created = WM_TASKBAR_CREATED.load(Ordering::Relaxed);
    if wm_taskbar_created != 0 && message == wm_taskbar_created {
        unsafe {
            add_tray_icon(window);
        }
        return LRESULT(0);
    }

    match message {
        WM_CAPTURE_EVENTS => {
            crate::wcap_engine::pump_capture_events();
            LRESULT(0)
        }
        WM_WCAP_ALREADY_RUNNING => {
            SHOW_SETTINGS.store(true, Ordering::Relaxed);
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == RECORDING_TIMER_ID => {
            crate::wcap_engine::pump_capture_events();
            if let Some(engine) = crate::wcap_engine::engine() {
                if let Ok(mut engine) = engine.try_lock() {
                    unsafe {
                        engine.tick();
                    }
                }
            }
            LRESULT(0)
        }
        WM_WCAP_COMMAND => {
            let msg = loword(lparam);
            if msg == WM_RBUTTONUP {
                unsafe {
                    show_tray_menu(window);
                }
            } else if msg == WM_LBUTTONDBLCLK {
                SHOW_SETTINGS.store(true, Ordering::Relaxed);
            }
            LRESULT(0)
        }
        WM_HOTKEY => {
            if let Some(engine) = crate::wcap_engine::engine() {
                let mut engine = engine.lock().unwrap_or_else(|p| p.into_inner());
                if engine.is_recording() {
                    unsafe {
                        engine.stop_recording();
                    }
                    return LRESULT(0);
                }
            }
            let id = wparam.0 as i32;
            match id {
                HOT_RECORD_WINDOW => unsafe {
                    record_focused_window();
                },
                HOT_RECORD_MONITOR => unsafe {
                    record_current_monitor();
                },
                HOT_RECORD_REGION => {
                    RECORD_REGION.store(true, Ordering::Relaxed);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

unsafe fn show_tray_menu(window: HWND) {
    let Some(engine) = crate::wcap_engine::engine() else {
        return;
    };
    let recording = engine
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .is_recording();
    drop(engine);

    let menu = CreatePopupMenu().unwrap_or_default();
    if menu.0.is_null() {
        return;
    }
    let title_w: Vec<u16> = WCAP_TITLE
        .encode_utf16()
        .chain(core::iter::once(0))
        .collect();
    let _ = AppendMenuW(menu, MF_STRING, CMD_WCAP, PCWSTR(title_w.as_ptr()));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

    let settings_flags = if recording {
        MF_DISABLED | MF_GRAYED
    } else {
        MF_STRING
    };
    let _ = AppendMenuW(menu, settings_flags, CMD_SETTINGS, w!("Settings"));
    let _ = AppendMenuW(menu, MF_STRING, CMD_QUIT, w!("Exit"));

    let mut pos = POINT::default();
    let _ = GetCursorPos(&mut pos);
    let _ = SetForegroundWindow(window);

    let command = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
        pos.x,
        pos.y,
        Some(0),
        window,
        None,
    );
    let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
    let _ = DestroyMenu(menu);

    match command.0 as usize {
        CMD_WCAP | CMD_SETTINGS => {
            SHOW_SETTINGS.store(true, Ordering::Relaxed);
        }
        CMD_QUIT => {
            QUIT_REQUESTED.store(true, Ordering::Relaxed);
        }
        _ => {}
    }
}

unsafe fn register_hotkeys(window: HWND, config: &Config) {
    let _ = register_hotkey(window, HOT_RECORD_WINDOW, config.shortcut_window);
    let _ = register_hotkey(window, HOT_RECORD_MONITOR, config.shortcut_monitor);
    let _ = register_hotkey(window, HOT_RECORD_REGION, config.shortcut_region);
}

unsafe fn register_hotkey(window: HWND, id: i32, key_mod: u32) -> bool {
    let key = key_mod & 0xffffff;
    let modifiers = (key_mod >> 24) & 0xff;
    if key == 0 {
        return true;
    }
    let flags = HOT_KEY_MODIFIERS(modifiers as u32);
    RegisterHotKey(Some(window), id, flags, key).is_ok()
}

unsafe fn record_focused_window() {
    let focus = GetForegroundWindow();
    if focus.0.is_null() {
        return;
    }
    let Some(engine) = crate::wcap_engine::engine() else {
        return;
    };
    let mut engine = engine.lock().unwrap_or_else(|p| p.into_inner());
    let only_client_area = engine.config.only_client_area.as_bool();
    if engine.capture_window(focus, only_client_area) {
        let _ = engine.start_recording();
    }
}

unsafe fn record_current_monitor() {
    let mut cursor = POINT::default();
    let _ = GetCursorPos(&mut cursor);
    let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
    if monitor.0.is_null() {
        return;
    }
    let Some(engine) = crate::wcap_engine::engine() else {
        return;
    };
    let mut engine = engine.lock().unwrap_or_else(|p| p.into_inner());
    if engine.capture_monitor(monitor, None) {
        let _ = engine.start_recording();
    }
}

pub unsafe fn handle_event(event: EngineEvent) {
    match event {
        EngineEvent::ShowNotification { title, body } => {
            let title = String::from_utf16_lossy(&title);
            let body = String::from_utf16_lossy(&body);
            notify(&title, &body, NIIF_WARNING);
        }
        _ => {}
    }
}
