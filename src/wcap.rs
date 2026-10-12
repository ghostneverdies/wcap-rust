use windows::{
    core::*,
    Win32::{
        Foundation::*,
        System::{LibraryLoader::*, Threading::*},
        UI::WindowsAndMessaging::*,
    },
};

use crate::{wcap_engine, wcap_tray, wcap_ui};

pub use ::wcap::wcap::*;

pub use ::wcap::wcap_audio_capture::*;
pub use ::wcap::wcap_config::*;
pub use ::wcap::wcap_encoder::*;
pub use ::wcap::wcap_screen_capture::*;

pub unsafe fn run() {
    let instance = GetModuleHandleW(None).unwrap();

    if let Ok(existing) = FindWindowW(w!("wcap_tray_window_class"), None) {
        let _ = PostMessageW(
            Some(existing),
            WM_WCAP_ALREADY_RUNNING,
            WPARAM(0),
            LPARAM(0),
        );
        ExitProcess(0);
    }

    if !screen_capture_is_supported() {
        MessageBoxW(
            None,
            w!("Windows 10 Version 1903, May 2019 Update (19H1) or newer is required!"),
            w!("wcap"),
            MB_ICONEXCLAMATION,
        );
        ExitProcess(0);
    }

    let mut config_path = [0u16; 260];
    let config_len = GetModuleFileNameW(None, &mut config_path) as usize;
    if let Some(pos) = config_path[..config_len]
        .iter()
        .rposition(|&c| c == b'.' as u16)
    {
        let ext: Vec<u16> = ".ini".encode_utf16().collect();
        for (i, u) in ext.iter().enumerate() {
            if pos + i < config_path.len() - 1 {
                config_path[pos + i] = *u;
            }
        }
    }

    let mut config = Config::default();
    config_defaults(&mut config);
    config_load(&mut config, &config_path);

    if !wcap_tray::init(HINSTANCE(instance.0), &config) {
        MessageBoxW(
            None,
            w!("Cannot create wcap window!"),
            w!("wcap"),
            MB_ICONEXCLAMATION,
        );
        ExitProcess(0);
    }

    let engine = wcap_engine::Engine::new(config, &config_path);
    wcap_engine::install(engine);

    let engine_ref = wcap_engine::engine().unwrap();
    let initialized = {
        let mut guard = engine_ref.lock().unwrap_or_else(|p| p.into_inner());
        guard.init()
    };
    if !initialized {
        MessageBoxW(
            None,
            w!("Cannot initialize capture and encoding!"),
            w!("wcap"),
            MB_ICONEXCLAMATION,
        );
        wcap_tray::dispose();
        ExitProcess(0);
    }

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(WCAP_TITLE)
            .with_icon({
                let icon = image::load_from_memory_with_format(
                    include_bytes!("../assets/wcap.ico"),
                    image::ImageFormat::Ico,
                )
                .expect("embedded wcap icon")
                .into_rgba8();
                egui::IconData {
                    width: icon.width(),
                    height: icon.height(),
                    rgba: icon.into_raw(),
                }
            })
            .with_inner_size([680.0, 620.0])
            .with_min_inner_size([680.0, 620.0])
            .with_resizable(false)
            .with_maximize_button(false),
        ..Default::default()
    };

    let _ = eframe::run_native(
        WCAP_TITLE,
        options,
        Box::new(move |cc| {
            wcap_ui::configure_style(&cc.egui_ctx);
            Ok(Box::new(wcap_ui::WcapApp::new(engine_ref)))
        }),
    );

    if let Some(engine) = wcap_engine::engine() {
        engine.lock().unwrap_or_else(|p| p.into_inner()).shutdown();
    }
    wcap_tray::dispose();
}
