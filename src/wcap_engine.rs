use std::sync::{Arc, Mutex, OnceLock};
use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::Direct3D::*,
        Graphics::Direct3D11::*,
        Graphics::Dwm::*,
        Graphics::Dxgi::*,
        Graphics::Gdi::*,
        System::{Com::*, Performance::*, Power::*, SystemInformation::*},
        UI::Shell::*,
    },
};

use crate::wcap::*;

static ENGINE: OnceLock<Arc<Mutex<Engine>>> = OnceLock::new();

pub fn engine() -> Option<Arc<Mutex<Engine>>> {
    ENGINE.get().cloned()
}

pub fn install(engine: Engine) {
    let _ = ENGINE.set(Arc::new(Mutex::new(engine)));
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaptureTarget {
    #[default]
    None,
    Monitor(HMONITOR),
    Window(HWND, bool),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub recording: bool,
    pub started: bool,
    pub duration_msec: u32,
    pub bitrate: u32,
    pub file_size: u64,
    pub dropped_frames: u32,
}

use ::wcap::pacing::{trim_audio_prefix, FramePacer};

pub enum EngineEvent {
    RecordingStateChanged,
    ShowNotification { title: Vec<u16>, body: Vec<u16> },
}

pub struct Engine {
    pub tick_freq: u64,

    pub device: Option<ID3D11Device>,
    device_preference: Option<(bool, bool)>,
    capture: ScreenCapture,
    encoder: Encoder,
    audio: AudioCapture,

    pub config: Config,
    pub config_path: [u16; 260],

    pub target: CaptureTarget,
    pub selection_rect: Option<RECT>,

    recording: bool,
    started: bool,
    recording_limit_framerate: u32,
    recording_dropped_frames: u32,
    rec_audio_pending: bool,
    stop_pending: bool,

    pacer: FramePacer,
    next_tooltip: u64,
    audio_tick_last: u64,
    execution_state: EXECUTION_STATE,
    com_initialized: bool,
    recording_path: Vec<u16>,

    notify_tx: std::sync::mpsc::Sender<EngineEvent>,
    notify_rx: std::sync::mpsc::Receiver<EngineEvent>,
}

unsafe impl Send for Engine {}

impl Engine {
    pub fn new(config: Config, config_path: &[u16]) -> Self {
        let mut cfg_path = [0u16; 260];
        let len = config_path.len().min(cfg_path.len() - 1);
        cfg_path[..len].copy_from_slice(&config_path[..len]);

        let tick_freq = unsafe {
            let mut f = 0i64;
            if QueryPerformanceFrequency(&mut f).is_ok() && f > 0 {
                f as u64
            } else {
                10_000_000
            }
        };

        let (notify_tx, notify_rx) = std::sync::mpsc::channel();
        Self {
            tick_freq,
            device: None,
            device_preference: None,
            capture: ScreenCapture::default(),
            encoder: Encoder::default(),
            audio: AudioCapture::default(),
            config,
            config_path: cfg_path,
            target: CaptureTarget::None,
            selection_rect: None,
            recording: false,
            started: false,
            recording_limit_framerate: 0,
            recording_dropped_frames: 0,
            rec_audio_pending: false,
            stop_pending: false,
            pacer: FramePacer::default(),
            next_tooltip: 0,
            audio_tick_last: 0,
            execution_state: EXECUTION_STATE(0),
            com_initialized: false,
            recording_path: Vec::new(),
            notify_tx,
            notify_rx,
        }
    }

    pub fn is_recording(&self) -> bool {
        self.recording || self.started
    }

    pub fn notify(&self, event: EngineEvent) {
        let _ = self.notify_tx.send(event);
    }

    pub fn take_events(&mut self) -> Vec<EngineEvent> {
        self.notify_rx.try_iter().collect()
    }

    pub unsafe fn init(&mut self) -> bool {
        self.com_initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();

        let Some(device) = create_d3d11_device(&self.config) else {
            self.notify(EngineEvent::ShowNotification {
                title: w2wide("Cannot create graphics device!"),
                body: w2wide("Cannot Start Recording"),
            });
            return false;
        };

        screen_capture_create(&mut self.capture, false);
        if self.capture.item_interop.is_none() || self.capture.controller.is_none() {
            self.notify(EngineEvent::ShowNotification {
                title: w2wide("Cannot initialize screen capture!"),
                body: w2wide("Cannot Start Recording"),
            });
            screen_capture_release(&mut self.capture);
            return false;
        }
        self.capture.on_frame_handler = Some(windows::Foundation::TypedEventHandler::<
            windows::Graphics::Capture::Direct3D11CaptureFramePool,
            IInspectable,
        >::new(|sender, _| {
            if let Some(pool) = sender.as_ref() {
                dispatch_capture_frame(pool);
            }
            Ok(())
        }));
        self.capture.on_close_handler = Some(windows::Foundation::TypedEventHandler::<
            windows::Graphics::Capture::GraphicsCaptureItem,
            IInspectable,
        >::new(|sender, _| {
            if let Some(item) = sender.as_ref() {
                dispatch_capture_closed(item);
            }
            Ok(())
        }));
        encoder_init(&mut self.encoder);
        if self.encoder.last_error.is_some() {
            screen_capture_release(&mut self.capture);
            return false;
        }
        self.device = Some(device);
        self.device_preference = Some((
            self.config.hardware_encoder.as_bool(),
            self.config.hardware_prefer_integrated.as_bool(),
        ));
        true
    }

    pub unsafe fn shutdown(&mut self) {
        self.stop_recording();
        crate::wcap_encoder::encoder_release(&mut self.encoder);
        screen_capture_release(&mut self.capture);
        capture_events()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.device = None;
        if self.com_initialized {
            CoUninitialize();
            self.com_initialized = false;
        }
    }

    unsafe fn ensure_device(&mut self) -> bool {
        let preference = (
            self.config.hardware_encoder.as_bool(),
            self.config.hardware_prefer_integrated.as_bool(),
        );
        if self.device_preference != Some(preference) || self.device.is_none() {
            self.device = create_d3d11_device(&self.config);
            self.device_preference = self.device.as_ref().map(|_| preference);
        }
        self.device.is_some()
    }

    pub unsafe fn capture_monitor(&mut self, monitor: HMONITOR, rect: Option<&RECT>) -> bool {
        if self.is_recording() {
            return false;
        }
        if !self.ensure_device() {
            return false;
        }
        let Some(device) = self.device.as_ref() else {
            return false;
        };
        if !screen_capture_create_for_monitor(&mut self.capture, device, monitor, rect) {
            return false;
        }
        self.target = CaptureTarget::Monitor(monitor);
        self.selection_rect = rect.copied();
        true
    }

    pub unsafe fn capture_window(&mut self, window: HWND, only_client_area: bool) -> bool {
        if self.is_recording() {
            return false;
        }
        if !self.ensure_device() {
            return false;
        }
        let Some(device) = self.device.as_ref() else {
            return false;
        };
        if !screen_capture_create_for_window(
            &mut self.capture,
            device,
            window,
            only_client_area,
            !self.config.keep_rounded_window_corners.as_bool(),
        ) {
            return false;
        }
        self.target = CaptureTarget::Window(window, only_client_area);
        true
    }

    pub unsafe fn start_recording(&mut self) -> bool {
        if self.is_recording() {
            return false;
        }
        let Some(device) = self.device.clone() else {
            return false;
        };

        let mut folder = self.config.output_folder.clone();
        let folder_len = folder.iter().position(|&c| c == 0).unwrap_or(folder.len());
        if folder_len == 0 {
            self.notify(EngineEvent::ShowNotification {
                title: w2wide("Empty output folder!"),
                body: w2wide("Cannot Start Recording"),
            });
            return false;
        }
        if folder_len < folder.len() {
            folder[folder_len] = 0;
        }
        let folder_pcwstr = PCWSTR::from_raw(folder.as_ptr());

        let err = SHCreateDirectoryExW(None, folder_pcwstr, None);
        if err != ERROR_SUCCESS.0 as i32
            && err != ERROR_FILE_EXISTS.0 as i32
            && err != ERROR_ALREADY_EXISTS.0 as i32
        {
            self.notify(EngineEvent::ShowNotification {
                title: w2wide("Cannot create output folder!"),
                body: w2wide("Cannot Start Recording"),
            });
            self.stop_capture_only();
            return false;
        }

        let time = GetLocalTime();
        let stem = format!(
            "{:04}{:02}{:02}_{:02}{:02}{:02}",
            time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute, time.wSecond
        );
        let mut suffix = 0u32;
        let recording_path = loop {
            let name = if suffix == 0 {
                format!("{stem}.mp4")
            } else {
                format!("{stem}_{suffix}.mp4")
            };
            let mut path = folder[..folder_len].to_vec();
            path.push('\\' as u16);
            path.extend(name.encode_utf16());
            path.push(0);
            if windows::Win32::Storage::FileSystem::GetFileAttributesW(PCWSTR(path.as_ptr()))
                == windows::Win32::Storage::FileSystem::INVALID_FILE_ATTRIBUTES
            {
                break path;
            }
            suffix += 1;
        };

        let (mut fr_num, mut fr_den) = compositor_rate();
        self.recording_limit_framerate = 0;
        let max_framerate = self.config.video_max_framerate as u32;
        if max_framerate > 0 && (max_framerate as u64 * fr_den as u64) < fr_num as u64 {
            self.recording_limit_framerate = max_framerate;
            fr_num = max_framerate;
            fr_den = 1;
        }

        let rect = self.capture.rect;
        let mut enc_config = EncoderConfig::default();
        enc_config.width = (rect.right - rect.left).max(2) as u32;
        enc_config.height = (rect.bottom - rect.top).max(2) as u32;
        enc_config.framerate_num = fr_num;
        enc_config.framerate_den = fr_den;
        enc_config.config = &mut self.config;

        let mut application_window = HWND::default();
        self.rec_audio_pending = self.config.capture_audio.as_bool();
        if self.rec_audio_pending {
            if self.config.application_local_audio.as_bool()
                && audio_capture_can_capture_application_local()
            {
                if let CaptureTarget::Window(hwnd, _) = self.target {
                    application_window = hwnd;
                }
            }
            if !audio_capture_start(&mut self.audio, application_window) {
                self.notify(EngineEvent::ShowNotification {
                    title: w2wide("Cannot capture audio!"),
                    body: w2wide("Cannot Start Recording"),
                });
                self.stop_capture_only();
                return false;
            }
            enc_config.audio_format = self.audio.format;
        }

        if !encoder_start(&mut self.encoder, &device, &recording_path, &enc_config).as_bool() {
            if self.rec_audio_pending {
                audio_capture_stop(&mut self.audio);
            }
            self.rec_audio_pending = false;
            self.notify(EngineEvent::ShowNotification {
                title: w2wide("Cannot initialize the recording encoder!"),
                body: w2wide(
                    &self
                        .encoder
                        .last_error
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                ),
            });
            self.stop_capture_only();
            return false;
        }

        self.next_tooltip = 0;
        self.pacer = FramePacer::default();
        self.recording_dropped_frames = 0;
        self.stop_pending = false;
        self.audio_tick_last = qpc();
        self.recording_path = recording_path;

        if !screen_capture_start(
            &mut self.capture,
            self.config.mouse_cursor.as_bool(),
            self.config.show_recording_border.as_bool(),
            self.config.include_secondary_windows.as_bool(),
        ) {
            if self.rec_audio_pending {
                audio_capture_stop(&mut self.audio);
            }
            self.rec_audio_pending = false;
            encoder_stop(&mut self.encoder);
            self.notify(EngineEvent::ShowNotification {
                title: w2wide("Cannot start screen capture!"),
                body: w2wide("Cannot Start Recording"),
            });
            return false;
        }

        self.recording = true;
        self.started = false;
        self.execution_state =
            SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED);
        crate::wcap_tray::set_recording_timer(true);
        self.notify(EngineEvent::RecordingStateChanged);
        true
    }

    pub unsafe fn stop_recording(&mut self) {
        if !self.is_recording() {
            return;
        }
        self.recording = false;
        crate::wcap_tray::set_recording_timer(false);

        if self.rec_audio_pending {
            audio_capture_flush(&mut self.audio);
            self.encode_captured_audio();
            audio_capture_stop(&mut self.audio);
        }
        self.rec_audio_pending = false;

        screen_capture_stop(&mut self.capture);
        encoder_stop(&mut self.encoder);
        if let Some(error) = self.encoder.last_error.take() {
            self.notify(EngineEvent::ShowNotification {
                title: w2wide("Recording could not finish cleanly"),
                body: w2wide(&error.to_string()),
            });
        }
        if self.execution_state.0 != 0 {
            let _ = SetThreadExecutionState(self.execution_state);
            self.execution_state = EXECUTION_STATE(0);
        }
        if self.config.open_folder.as_bool() && !self.recording_path.is_empty() {
            let mut item = std::ptr::null_mut();
            if SHParseDisplayName(
                PCWSTR(self.recording_path.as_ptr()),
                None,
                &mut item,
                0,
                None,
            )
            .is_ok()
            {
                let _ = SHOpenFolderAndSelectItems(item, None, 0);
                CoTaskMemFree(Some(item as _));
            }
        }

        self.started = false;
        self.notify(EngineEvent::RecordingStateChanged);
    }

    fn stop_capture_only(&mut self) {
        unsafe {
            screen_capture_stop(&mut self.capture);
        }
    }

    pub fn stats(&mut self) -> Stats {
        let mut bitrate = 0u32;
        let mut length_msec = 0u32;
        let mut file_size = 0u64;
        if self.started {
            unsafe {
                encoder_get_stats(
                    &mut self.encoder,
                    &mut bitrate,
                    &mut length_msec,
                    &mut file_size,
                );
            }
        }
        Stats {
            recording: self.recording,
            started: self.started,
            duration_msec: length_msec,
            bitrate,
            file_size,
            dropped_frames: self.recording_dropped_frames,
        }
    }

    pub unsafe fn tick(&mut self) {
        if self.is_recording() {
            if let Some(error) = self.encoder.last_error.take() {
                self.notify(EngineEvent::ShowNotification {
                    title: w2wide("Recording error"),
                    body: w2wide(&error.to_string()),
                });
                self.request_stop();
            }
            if self.rec_audio_pending && self.audio.stop.load(std::sync::atomic::Ordering::Acquire)
            {
                self.notify(EngineEvent::ShowNotification {
                    title: w2wide("Audio capture stopped"),
                    body: w2wide("The audio worker stopped unexpectedly."),
                });
                self.request_stop();
            }
        }
        if self.is_recording() {
            let now = qpc();
            if self.started {
                encoder_update(&mut self.encoder, now, self.tick_freq);
            }
            if self.rec_audio_pending
                && now.saturating_sub(self.audio_tick_last) >= self.tick_freq / 50
            {
                self.audio_tick_last = now;
                self.encode_captured_audio();
            }
            self.check_limits(now);
            if self.recording {
                if self.next_tooltip == 0 {
                    self.next_tooltip = now + self.tick_freq;
                } else if now >= self.next_tooltip {
                    self.next_tooltip = now.saturating_add(self.tick_freq);
                    let elapsed = ((now.saturating_sub(self.encoder.start_time) as u128 * 1000)
                        / self.tick_freq.max(1) as u128) as u64;
                    let title = format!("wcap - {}", duration_string(elapsed));
                    crate::wcap_tray::set_title(&title);
                }
            }
        }
        if self.stop_pending {
            self.stop_pending = false;
            self.stop_recording();
        }
    }

    pub fn request_stop(&mut self) {
        if self.is_recording() {
            self.stop_pending = true;
        }
    }

    unsafe fn encode_captured_audio(&mut self) {
        let start = self.encoder.start_time;
        if !self.started || start == 0 || self.audio.format.is_null() {
            return;
        }
        let sample_rate = (*self.audio.format).nSamplesPerSec;
        let frame_bytes = (*self.audio.format).nBlockAlign as usize;
        let mut data = AudioCaptureData::default();
        while audio_capture_get_data(&mut self.audio, &mut data, start) {
            let (skip, time) =
                trim_audio_prefix(data.time, start, data.count, sample_rate, self.tick_freq);
            let count = data.count - skip;
            if !data.samples.is_null() && count > 0 {
                encoder_new_samples(
                    &mut self.encoder,
                    (data.samples as *const u8).add(skip * frame_bytes) as _,
                    count as u32,
                    time,
                    self.tick_freq,
                );
            }
            audio_capture_release_data(&mut self.audio, &mut data);
        }
    }

    pub unsafe fn on_frame(&mut self, frame: *mut ScreenCaptureFrame) {
        let frame = match unsafe { frame.as_mut() } {
            Some(f) => f,
            None => {
                self.request_stop();
                return;
            }
        };

        let time = frame.time;
        let do_encode = self
            .pacer
            .accept(time, self.recording_limit_framerate, self.tick_freq);

        if !self.is_recording() {
            return;
        }

        if do_encode {
            let texture = match frame.texture.as_ref() {
                Some(t) => t,
                None => return,
            };
            let rect = frame.rect;
            let submitted =
                encoder_new_frame(&mut self.encoder, texture, rect, time, self.tick_freq).as_bool();
            if !submitted {
                self.recording_dropped_frames = self.recording_dropped_frames.saturating_add(1);
            }
            if submitted && !self.started {
                self.started = true;
                self.notify(EngineEvent::RecordingStateChanged);
            }
        }

        if !self.recording {
            return;
        }

        self.check_limits(time);
    }

    unsafe fn check_limits(&mut self, time: u64) {
        if !self.started || !self.recording {
            return;
        }
        let mut stop = false;
        if self.config.enable_limit_length.as_bool() {
            let limit = self.config.limit_length as u64 * self.tick_freq;
            if time.saturating_sub(self.encoder.start_time) >= limit {
                stop = true;
            }
        }
        if self.config.enable_limit_size.as_bool() && !stop {
            let mut file_size: u64 = 0;
            let mut bitrate: u32 = 0;
            let mut length_msec: u32 = 0;
            encoder_get_stats(
                &mut self.encoder,
                &mut bitrate,
                &mut length_msec,
                &mut file_size,
            );
            if 1000u128 * file_size as u128 >= 995u128 * ((self.config.limit_size as u128) << 20) {
                stop = true;
            }
        }
        if stop && !self.stop_pending {
            self.stop_pending = true;
        }
    }
}

#[derive(Clone)]
enum PendingCapture {
    Frame(windows::Graphics::Capture::Direct3D11CaptureFramePool),
    Closed(windows::Graphics::Capture::GraphicsCaptureItem),
}
static CAPTURE_EVENTS: OnceLock<Mutex<std::collections::VecDeque<PendingCapture>>> =
    OnceLock::new();
fn capture_events() -> &'static Mutex<std::collections::VecDeque<PendingCapture>> {
    CAPTURE_EVENTS.get_or_init(|| Mutex::new(std::collections::VecDeque::new()))
}
fn queue_capture(event: PendingCapture) {
    let mut queue = capture_events().lock().unwrap_or_else(|p| p.into_inner());
    let already_queued = queue.iter().any(|pending| match (pending, &event) {
        (PendingCapture::Frame(a), PendingCapture::Frame(b)) => a == b,
        (PendingCapture::Closed(a), PendingCapture::Closed(b)) => a == b,
        _ => false,
    });
    if !already_queued {
        queue.push_back(event);
    }
    drop(queue);
    unsafe {
        crate::wcap_tray::wake_capture_events();
    }
}
fn process_capture(engine: &mut Engine, event: PendingCapture) {
    match event {
        PendingCapture::Frame(pool) => {
            if engine.capture.frame_pool.as_ref() != Some(&pool) || !engine.recording {
                return;
            }
            for _ in 0..SCREEN_CAPTURE_BUFFER_COUNT {
                unsafe {
                    let mut frame = ScreenCaptureFrame::default();
                    if !screen_capture_get_frame(&mut engine.capture, &mut frame) {
                        break;
                    }
                    engine.on_frame(&mut frame);
                    screen_capture_release_frame(&mut engine.capture, &mut frame);
                }
            }
        }
        PendingCapture::Closed(item) => {
            if engine.capture.item.as_ref() == Some(&item) {
                engine.request_stop();
            }
        }
    }
}
fn dispatch_capture(event: PendingCapture) {
    let Some(handle) = engine() else {
        return;
    };
    match handle.try_lock() {
        Ok(mut engine) => process_capture(&mut engine, event),
        Err(std::sync::TryLockError::Poisoned(p)) => process_capture(&mut p.into_inner(), event),
        Err(std::sync::TryLockError::WouldBlock) => queue_capture(event),
    };
}
pub fn dispatch_capture_frame(pool: &windows::Graphics::Capture::Direct3D11CaptureFramePool) {
    dispatch_capture(PendingCapture::Frame(pool.clone()));
}
pub fn dispatch_capture_closed(item: &windows::Graphics::Capture::GraphicsCaptureItem) {
    dispatch_capture(PendingCapture::Closed(item.clone()));
}
pub fn pump_capture_events() {
    let Some(handle) = engine() else {
        return;
    };
    let Ok(mut engine) = handle.try_lock() else {
        return;
    };
    for _ in 0..SCREEN_CAPTURE_BUFFER_COUNT {
        let event = capture_events()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop_front();
        let Some(event) = event else {
            break;
        };
        process_capture(&mut engine, event);
    }
}

pub unsafe fn qpc() -> u64 {
    let mut t = 0i64;
    let _ = QueryPerformanceCounter(&mut t);
    t as u64
}

pub unsafe fn create_d3d11_device(config: &Config) -> Option<ID3D11Device> {
    let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    let adapter: Option<IDXGIAdapter> = if config.hardware_encoder.as_bool() {
        CreateDXGIFactory1::<IDXGIFactory6>()
            .ok()
            .and_then(|factory| {
                factory
                    .EnumAdapterByGpuPreference(
                        0,
                        if config.hardware_prefer_integrated.as_bool() {
                            DXGI_GPU_PREFERENCE_MINIMUM_POWER
                        } else {
                            DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE
                        },
                    )
                    .ok()
            })
    } else {
        None
    };
    let hr = D3D11CreateDevice(
        adapter.as_ref(),
        if adapter.is_some() {
            D3D_DRIVER_TYPE_UNKNOWN
        } else {
            D3D_DRIVER_TYPE_HARDWARE
        },
        HMODULE::default(),
        flags,
        Some(&[D3D_FEATURE_LEVEL_11_0]),
        D3D11_SDK_VERSION,
        Some(&mut device),
        None,
        Some(&mut context),
    );
    if hr.is_err() {
        let hr2 = D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_WARP,
            HMODULE::default(),
            flags,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        );
        if hr2.is_err() {
            return None;
        }
    }
    drop(context);
    device
}

fn compositor_rate() -> (u32, u32) {
    let mut info = DWM_TIMING_INFO::default();
    info.cbSize = std::mem::size_of::<DWM_TIMING_INFO>() as u32;
    let ok = unsafe { DwmGetCompositionTimingInfo(HWND::default(), &mut info) }.is_ok();
    if ok && info.rateCompose.uiNumerator != 0 && info.rateCompose.uiDenominator != 0 {
        (info.rateCompose.uiNumerator, info.rateCompose.uiDenominator)
    } else {
        (60, 1)
    }
}

fn duration_string(msec: u64) -> String {
    let msec = msec as u64;
    let s = msec / 1000;
    let h = s / 3600;
    let m = s % 3600 / 60;
    let sec = s % 60;
    format!("{}:{:02}:{:02}", h, m, sec)
}

fn w2wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
