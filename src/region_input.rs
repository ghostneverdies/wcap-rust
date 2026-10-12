use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::Gdi::ScreenToClient,
        System::LibraryLoader::GetModuleHandleW,
        UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    },
};

static TARGET: AtomicIsize = AtomicIsize::new(0);
static LEFT_DOWN: AtomicBool = AtomicBool::new(false);

pub struct InputShield {
    hook: HHOOK,
    window: HWND,
}
impl InputShield {
    pub unsafe fn install(window: HWND) -> Result<Self> {
        let module = GetModuleHandleW(None)?;
        LEFT_DOWN.store(false, Ordering::Relaxed);
        TARGET.store(window.0 as isize, Ordering::Release);
        let hook =
            match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(HINSTANCE(module.0)), 0) {
                Ok(hook) => hook,
                Err(error) => {
                    TARGET.store(0, Ordering::Release);
                    return Err(error);
                }
            };
        let shield = Self { hook, window };
        let _ = SetForegroundWindow(window);
        let _ = SetFocus(Some(window));
        Ok(shield)
    }
    pub unsafe fn lost_focus(&self) -> bool {
        GetForegroundWindow() != self.window
    }
}
impl Drop for InputShield {
    fn drop(&mut self) {
        TARGET.store(0, Ordering::Release);
        LEFT_DOWN.store(false, Ordering::Relaxed);
        unsafe {
            let _ = ShowWindow(self.window, SW_HIDE);
            let _ = UnhookWindowsHookEx(self.hook);
            if GetCapture() == self.window {
                let _ = ReleaseCapture();
            }
        }
    }
}
unsafe extern "system" fn mouse_hook(code: i32, message: WPARAM, packet: LPARAM) -> LRESULT {
    let target = TARGET.load(Ordering::Acquire);
    if code < 0 || target == 0 {
        return CallNextHookEx(None, code, message, packet);
    }
    let window = HWND(target as *mut _);
    let mouse = &*(packet.0 as *const MSLLHOOKSTRUCT);
    let message = message.0 as u32;
    match message {
        WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP => {
            let mut point = mouse.pt;
            let _ = ScreenToClient(window, &mut point);
            let position =
                LPARAM((((point.y as u32 & 0xffff) << 16) | (point.x as u32 & 0xffff)) as isize);
            if message == WM_LBUTTONDOWN {
                LEFT_DOWN.store(true, Ordering::Relaxed);
            }
            if message == WM_LBUTTONUP {
                LEFT_DOWN.store(false, Ordering::Relaxed);
            }
            let buttons = WPARAM(if LEFT_DOWN.load(Ordering::Relaxed) {
                1
            } else {
                0
            });
            let _ = PostMessageW(Some(window), WM_MOUSEMOVE, buttons, position);
            if message != WM_MOUSEMOVE {
                let _ = PostMessageW(Some(window), message, buttons, position);
                return LRESULT(1);
            }
        }
        WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN | WM_MBUTTONUP | WM_XBUTTONDOWN
        | WM_XBUTTONUP | WM_MOUSEWHEEL | WM_MOUSEHWHEEL => return LRESULT(1),
        _ => {}
    }
    CallNextHookEx(None, code, WPARAM(message as usize), packet)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::AtomicUsize,
        time::{Duration, Instant},
    };
    static UNDERLYING: AtomicUsize = AtomicUsize::new(0);
    static PICKER: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "system" fn probe(window: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        if matches!(
            msg,
            WM_LBUTTONDOWN
                | WM_LBUTTONUP
                | WM_RBUTTONDOWN
                | WM_RBUTTONUP
                | WM_MBUTTONDOWN
                | WM_MBUTTONUP
                | WM_XBUTTONDOWN
                | WM_XBUTTONUP
                | WM_MOUSEWHEEL
        ) {
            let counter = if GetWindowLongPtrW(window, GWLP_USERDATA) == 1 {
                &PICKER
            } else {
                &UNDERLYING
            };
            counter.fetch_add(1, Ordering::Relaxed);
            return LRESULT(0);
        }
        DefWindowProcW(window, msg, w, l)
    }
    unsafe fn pump() {
        let until = Instant::now() + Duration::from_millis(100);
        while Instant::now() < until {
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    unsafe fn inject(flags: &[MOUSE_EVENT_FLAGS]) {
        let inputs: Vec<INPUT> = flags
            .iter()
            .map(|&flags| INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dwFlags: flags,
                        mouseData: if flags == MOUSEEVENTF_WHEEL {
                            120
                        } else if flags == MOUSEEVENTF_XDOWN || flags == MOUSEEVENTF_XUP {
                            1
                        } else {
                            0
                        },
                        ..Default::default()
                    },
                },
            })
            .collect();
        let count = inputs.len() as u32;
        let sender =
            std::thread::spawn(move || SendInput(&inputs, std::mem::size_of::<INPUT>() as i32));
        pump();
        assert_eq!(sender.join().unwrap(), count);
        pump();
    }
    struct ProbeWindows {
        beneath: HWND,
        picker: HWND,
        cursor: POINT,
        foreground: HWND,
    }
    impl Drop for ProbeWindows {
        fn drop(&mut self) {
            unsafe {
                let _ = ReleaseCapture();
                let _ = DestroyWindow(self.picker);
                let _ = DestroyWindow(self.beneath);
                let _ = SetCursorPos(self.cursor.x, self.cursor.y);
                let _ = SetForegroundWindow(self.foreground);
            }
        }
    }
    #[test]
    fn clicks_are_blocked_even_when_another_window_owns_capture_and_restored_on_drop() {
        unsafe {
            let module = HINSTANCE(GetModuleHandleW(None).unwrap().0);
            let class = WNDCLASSW {
                lpfnWndProc: Some(probe),
                hInstance: module,
                lpszClassName: w!("wcap_input_probe"),
                ..Default::default()
            };
            assert_ne!(RegisterClassW(&class), 0);
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            let foreground = GetForegroundWindow();
            let beneath = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                w!("wcap_input_probe"),
                w!("Input probe"),
                WS_POPUP | WS_VISIBLE,
                40,
                40,
                160,
                100,
                None,
                None,
                Some(module),
                None,
            )
            .unwrap();
            let picker = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                w!("wcap_input_probe"),
                w!("Picker probe"),
                WS_POPUP | WS_VISIBLE,
                40,
                40,
                160,
                100,
                None,
                None,
                Some(module),
                None,
            )
            .unwrap();
            let windows = ProbeWindows {
                beneath,
                picker,
                cursor,
                foreground,
            };
            SetWindowLongPtrW(picker, GWLP_USERDATA, 1);
            let shield = InputShield::install(picker).unwrap();
            SetCapture(beneath);
            assert_eq!(GetCapture(), beneath);
            let _ = SetCursorPos(80, 80);
            UNDERLYING.store(0, Ordering::Relaxed);
            PICKER.store(0, Ordering::Relaxed);
            inject(&[
                MOUSEEVENTF_RIGHTDOWN,
                MOUSEEVENTF_RIGHTUP,
                MOUSEEVENTF_MIDDLEDOWN,
                MOUSEEVENTF_MIDDLEUP,
                MOUSEEVENTF_XDOWN,
                MOUSEEVENTF_XUP,
                MOUSEEVENTF_WHEEL,
                MOUSEEVENTF_LEFTDOWN,
                MOUSEEVENTF_LEFTUP,
            ]);
            assert_eq!(
                UNDERLYING.load(Ordering::Relaxed),
                0,
                "underlying window must receive no clicks or wheel events"
            );
            assert_eq!(
                PICKER.load(Ordering::Relaxed),
                2,
                "left selection input must reach the picker"
            );
            drop(shield);
            inject(&[MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP]);
            assert_eq!(
                UNDERLYING.load(Ordering::Relaxed),
                2,
                "normal input must resume after closing the picker"
            );
            drop(windows);
        }
    }
}
