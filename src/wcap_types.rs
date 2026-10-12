use windows::{
    core::*,
    Foundation::*,
    Graphics::{Capture::*, DirectX::Direct3D11::*, SizeInt32},
    Win32::{
        Foundation::*,
        Graphics::{Direct3D11::*, Dwm::*},
        Media::{Audio::*, MediaFoundation::*},
        System::WinRT::Graphics::Capture::*,
        UI::WindowsAndMessaging::*,
    },
};
pub const WCAP_TITLE: &str = "wcap";
pub const WCAP_URL: &str = "https://github.com/mmozeiko/wcap";

pub const WM_WCAP_ALREADY_RUNNING: u32 = WM_USER + 1;
pub const WM_WCAP_STOP_CAPTURE: u32 = WM_USER + 2;
pub const WM_WCAP_TRAY_TITLE: u32 = WM_USER + 3;
pub const WM_WCAP_COMMAND: u32 = WM_USER + 4;

pub const WCAP_AUDIO_CAPTURE_TIMER: usize = 1;
pub const WCAP_AUDIO_CAPTURE_INTERVAL: u32 = 100;

pub const WCAP_VIDEO_UPDATE_TIMER: usize = 2;
pub const WCAP_VIDEO_UPDATE_INTERVAL: u32 = 100;

pub const CMD_WCAP: usize = 1;
pub const CMD_QUIT: usize = 2;
pub const CMD_SETTINGS: usize = 3;

pub const HOT_RECORD_WINDOW: i32 = 1;
pub const HOT_RECORD_MONITOR: i32 = 2;
pub const HOT_RECORD_REGION: i32 = 3;

pub const WCAP_RESIZE_NONE: i32 = 0;
pub const WCAP_RESIZE_TL: i32 = 1;
pub const WCAP_RESIZE_T: i32 = 2;
pub const WCAP_RESIZE_TR: i32 = 3;
pub const WCAP_RESIZE_L: i32 = 4;
pub const WCAP_RESIZE_M: i32 = 5;
pub const WCAP_RESIZE_R: i32 = 6;
pub const WCAP_RESIZE_BL: i32 = 7;
pub const WCAP_RESIZE_B: i32 = 8;
pub const WCAP_RESIZE_BR: i32 = 9;

pub const WCAP_UI_FONT: &str = "Segoe UI";
pub const WCAP_UI_FONT_SIZE: i32 = 16;

pub const WCAP_RECT_BORDER: i32 = 2;

pub const MF_UNITS_PER_SECOND: u64 = 10000000;

pub const CONFIG_VIDEO_H264: u32 = 0;
pub const CONFIG_VIDEO_H265: u32 = 1;
pub const CONFIG_VIDEO_AV1: u32 = 2;

pub const CONFIG_VIDEO_BASE: u32 = 0;
pub const CONFIG_VIDEO_MAIN: u32 = 1;
pub const CONFIG_VIDEO_HIGH: u32 = 2;
pub const CONFIG_VIDEO_MAIN_10: u32 = 3;

pub const CONFIG_AUDIO_AAC: u32 = 0;
pub const CONFIG_AUDIO_FLAC: u32 = 1;

pub const ENCODER_VIDEO_BUFFER_COUNT: usize = 8;
pub const ENCODER_AUDIO_BUFFER_COUNT: usize = 16;

pub const SCREEN_CAPTURE_BUFFER_COUNT: u32 = 2;
pub const SCREEN_CAPTURE_BUFFER_FORMAT: i32 = 87;

pub const INI_SECTION: &str = "wcap";

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    pub mouse_cursor: BOOL,
    pub only_client_area: BOOL,
    pub show_recording_border: BOOL,
    pub keep_rounded_window_corners: BOOL,
    pub include_secondary_windows: BOOL,
    pub hardware_encoder: BOOL,
    pub hardware_prefer_integrated: BOOL,
    pub output_folder: [u16; 260],
    pub open_folder: BOOL,
    pub fragmented_output: BOOL,
    pub enable_limit_length: BOOL,
    pub enable_limit_size: BOOL,
    pub limit_length: u32,
    pub limit_size: u32,
    pub gamma_correct_resize: BOOL,
    pub improved_color_conversion: BOOL,
    pub video_codec: u32,
    pub video_profile: u32,
    pub video_max_width: u32,
    pub video_max_height: u32,
    pub video_max_framerate: u32,
    pub video_bitrate: u32,
    pub capture_audio: BOOL,
    pub application_local_audio: BOOL,
    pub audio_codec: u32,
    pub audio_channels: u32,
    pub audio_samplerate: u32,
    pub audio_bitrate: u32,
    pub shortcut_monitor: u32,
    pub shortcut_window: u32,
    pub shortcut_region: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            mouse_cursor: true.into(),
            only_client_area: true.into(),
            show_recording_border: true.into(),
            keep_rounded_window_corners: true.into(),
            include_secondary_windows: false.into(),
            hardware_encoder: true.into(),
            hardware_prefer_integrated: false.into(),
            output_folder: [0; 260],
            open_folder: true.into(),
            fragmented_output: false.into(),
            enable_limit_length: false.into(),
            enable_limit_size: false.into(),
            limit_length: 60,
            limit_size: 25,
            gamma_correct_resize: false.into(),
            improved_color_conversion: false.into(),
            video_codec: CONFIG_VIDEO_H264,
            video_profile: CONFIG_VIDEO_HIGH,
            video_max_width: 1920,
            video_max_height: 1080,
            video_max_framerate: 60,
            video_bitrate: 8000,
            capture_audio: true.into(),
            application_local_audio: true.into(),
            audio_codec: CONFIG_AUDIO_AAC,
            audio_channels: 2,
            audio_samplerate: 48000,
            audio_bitrate: 160,
            shortcut_monitor: 0,
            shortcut_window: 0,
            shortcut_region: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Debug, Default)]
pub struct ScreenCaptureFrame {
    pub texture: Option<ID3D11Texture2D>,
    pub time: u64,
    pub rect: RECT,
    pub width: u32,
    pub height: u32,
    pub next_frame: Option<IUnknown>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AudioCaptureData {
    pub samples: *mut std::ffi::c_void,
    pub count: usize,
    pub time: u64,
}

pub const YUV_COLOR_SPACE_BT601: u32 = 0;
pub const YUV_COLOR_SPACE_BT709: u32 = 1;
pub const YUV_COLOR_SPACE_BT2020: u32 = 2;

#[repr(C)]
#[derive(Clone, Debug, Default)]
pub struct TexResize {
    pub input_texture: Option<ID3D11Texture2D>,
    pub output_texture: Option<ID3D11Texture2D>,
    pub input_view_in: Option<ID3D11ShaderResourceView>,
    pub output_view_out: Option<ID3D11UnorderedAccessView>,
    pub middle_view_in: Option<ID3D11ShaderResourceView>,
    pub middle_view_out: Option<ID3D11UnorderedAccessView>,
    pub pass_h: Option<ID3D11ComputeShader>,
    pub pass_v: Option<ID3D11ComputeShader>,
    pub input_width: u32,
    pub input_height: u32,
    pub output_width: u32,
    pub output_height: u32,
}

#[repr(C)]
#[derive(Clone, Debug, Default)]
pub struct YuvConvertOutput {
    pub texture: Option<ID3D11Texture2D>,
    pub view_in_uv: Option<ID3D11ShaderResourceView>,
    pub view_out_uv: Option<ID3D11UnorderedAccessView>,
    pub view_out_y: Option<ID3D11UnorderedAccessView>,
}

#[repr(C)]
#[derive(Clone, Debug, Default)]
pub struct YuvConvert {
    pub input_view: Option<ID3D11ShaderResourceView>,
    pub single_pass: Option<ID3D11ComputeShader>,
    pub pass1: Option<ID3D11ComputeShader>,
    pub pass2: Option<ID3D11ComputeShader>,
    pub constant_buffer: Option<ID3D11Buffer>,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Debug, Default)]
pub struct ScreenCapture {
    pub item_interop: Option<IGraphicsCaptureItemInterop>,
    pub controller: Option<windows::System::DispatcherQueueController>,
    pub device: Option<IDirect3DDevice>,
    pub item: Option<GraphicsCaptureItem>,
    pub frame_pool: Option<Direct3D11CaptureFramePool>,
    pub session: Option<GraphicsCaptureSession>,
    pub on_frame_handler: Option<TypedEventHandler<Direct3D11CaptureFramePool, IInspectable>>,
    pub on_close_handler: Option<TypedEventHandler<GraphicsCaptureItem, IInspectable>>,
    pub on_frame_token: Option<i64>,
    pub on_close_token: Option<i64>,
    pub callback_on_thread: bool,
    pub runtime_initialized: bool,
    pub current_size: SizeInt32,
    pub rect: RECT,
    pub window: HWND,
    pub only_client_area: bool,
    pub restore_window_corner_preference: bool,
    pub window_corner_preference: DWM_WINDOW_CORNER_PREFERENCE,
}

#[repr(C)]
pub struct AudioCapture {
    pub format: *mut WAVEFORMATEX,
    pub start_qpc: u64,
    pub start_pos: u64,
    pub freq: u64,
    pub use_device_timestamp: bool,
    pub check_device_timestamp: bool,
    pub stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub event: HANDLE,
    pub thread: Option<std::thread::JoinHandle<()>>,
    pub buffer: *mut u8,
    pub buffer_size: u32,
    pub buffer_read: std::sync::Arc<std::sync::atomic::AtomicU32>,
    pub buffer_write: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

impl Default for AudioCapture {
    fn default() -> Self {
        Self {
            format: std::ptr::null_mut(),
            start_qpc: 0,
            start_pos: 0,
            freq: 0,
            use_device_timestamp: false,
            check_device_timestamp: false,
            stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            event: HANDLE::default(),
            thread: None,
            buffer: std::ptr::null_mut(),
            buffer_size: 0,
            buffer_read: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
            buffer_write: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        }
    }
}

#[derive(Default)]
pub struct Encoder {
    pub input_width: u32,
    pub input_height: u32,
    pub output_width: u32,
    pub output_height: u32,
    pub framerate_num: u32,
    pub framerate_den: u32,
    pub start_time: u64,
    pub context: Option<ID3D11DeviceContext>,
    pub multithread: Option<ID3D11Multithread>,
    pub writer: Option<IMFSinkWriter>,
    pub video_stream_index: i32,
    pub audio_stream_index: i32,
    pub input_view: Option<ID3D11RenderTargetView>,
    pub resize: TexResize,
    pub convert: YuvConvert,
    pub convert_output: [YuvConvertOutput; ENCODER_VIDEO_BUFFER_COUNT],
    pub video_pool: crate::tracked_samples::SamplePool,
    pub audio_pool: crate::tracked_samples::SamplePool,
    pub video_discontinuity: BOOL,
    pub video_last_time: u64,
    pub last_video_timestamp: Option<i64>,
    pub resampler: Option<IMFTransform>,
    pub audio_frame_size: u32,
    pub audio_sample_rate: u32,
    pub mf_initialized: bool,
    pub last_error: Option<Error>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub framerate_num: u32,
    pub framerate_den: u32,
    pub audio_format: *mut WAVEFORMATEX,
    pub config: *mut Config,
}
