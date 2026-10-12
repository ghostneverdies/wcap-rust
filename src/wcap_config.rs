use windows::{
    core::*, Win32::Foundation::*, Win32::System::Com::*, Win32::System::WindowsProgramming::*,
    Win32::UI::Input::KeyboardAndMouse::*, Win32::UI::Shell::*,
};

use crate::wcap::*;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

fn wide_to_string(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

fn b(v: bool) -> BOOL {
    if v {
        TRUE
    } else {
        FALSE
    }
}

unsafe fn profile_int(section: &str, key: &str, default: i32, filename: &[u16]) -> i32 {
    let section_w = wide(section);
    let key_w = wide(key);
    GetPrivateProfileIntW(
        PCWSTR::from_raw(section_w.as_ptr()),
        PCWSTR::from_raw(key_w.as_ptr()),
        default,
        PCWSTR::from_raw(filename.as_ptr()),
    )
}

unsafe fn profile_str(
    section: &str,
    key: &str,
    default: &str,
    buffer: &mut [u16],
    filename: &[u16],
) {
    let section_w = wide(section);
    let key_w = wide(key);
    let default_w = wide(default);
    GetPrivateProfileStringW(
        PCWSTR::from_raw(section_w.as_ptr()),
        PCWSTR::from_raw(key_w.as_ptr()),
        PCWSTR::from_raw(default_w.as_ptr()),
        Some(buffer),
        PCWSTR::from_raw(filename.as_ptr()),
    );
}

unsafe fn profile_write(section: &str, key: &str, value: &str, filename: &[u16]) {
    let section_w = wide(section);
    let key_w = wide(key);
    let value_w = wide(value);
    let _ = WritePrivateProfileStringW(
        PCWSTR::from_raw(section_w.as_ptr()),
        PCWSTR::from_raw(key_w.as_ptr()),
        PCWSTR::from_raw(value_w.as_ptr()),
        PCWSTR::from_raw(filename.as_ptr()),
    );
}

fn profile_read_string(section: &str, key: &str, filename: &[u16]) -> String {
    let mut buffer = [0u16; 260];
    unsafe { profile_str(section, key, "", &mut buffer, filename) };
    wide_to_string(&buffer)
}

pub unsafe fn config_defaults(config: &mut Config) {
    *config = Config {
        mouse_cursor: TRUE,
        only_client_area: TRUE,
        show_recording_border: TRUE,
        keep_rounded_window_corners: TRUE,
        include_secondary_windows: FALSE,
        hardware_encoder: TRUE,
        hardware_prefer_integrated: FALSE,
        open_folder: TRUE,
        fragmented_output: FALSE,
        enable_limit_length: FALSE,
        enable_limit_size: FALSE,
        limit_length: 60,
        limit_size: 25,
        gamma_correct_resize: FALSE,
        improved_color_conversion: FALSE,
        video_codec: CONFIG_VIDEO_H264,
        video_profile: CONFIG_VIDEO_HIGH,
        video_max_width: 1920,
        video_max_height: 1080,
        video_max_framerate: 60,
        video_bitrate: 8000,
        capture_audio: TRUE,
        application_local_audio: TRUE,
        audio_codec: CONFIG_AUDIO_AAC,
        audio_channels: 2,
        audio_samplerate: 48000,
        audio_bitrate: 160,
        shortcut_monitor: (VK_SNAPSHOT.0 as u32) | ((MOD_CONTROL.0 & 0xff) << 24),
        shortcut_window: (VK_SNAPSHOT.0 as u32) | (((MOD_CONTROL | MOD_WIN).0 & 0xff) << 24),
        shortcut_region: (VK_SNAPSHOT.0 as u32) | (((MOD_CONTROL | MOD_SHIFT).0 & 0xff) << 24),
        ..Default::default()
    };

    if let Ok(path) = SHGetKnownFolderPath(&FOLDERID_Videos, KF_FLAG_DEFAULT, None) {
        if let Ok(s) = path.to_string() {
            let w = wide(&s);
            let len = w.len().min(config.output_folder.len() - 1);
            config.output_folder.fill(0);
            config.output_folder[..len].copy_from_slice(&w[..len]);
        }
        CoTaskMemFree(Some(path.as_ptr() as *const core::ffi::c_void));
    }
}

pub unsafe fn config_load(config: &mut Config, filename: &[u16]) {
    config.mouse_cursor =
        b(profile_int(INI_SECTION, "MouseCursor", config.mouse_cursor.0, filename) != 0);
    config.only_client_area = b(profile_int(
        INI_SECTION,
        "OnlyClientArea",
        config.only_client_area.0,
        filename,
    ) != 0);
    config.show_recording_border = b(profile_int(
        INI_SECTION,
        "ShowRecordingBorder",
        config.show_recording_border.0,
        filename,
    ) != 0);
    config.keep_rounded_window_corners = b(profile_int(
        INI_SECTION,
        "KeepRoundedWindowCorners",
        config.keep_rounded_window_corners.0,
        filename,
    ) != 0);
    config.include_secondary_windows = b(profile_int(
        INI_SECTION,
        "IncludeSecondaryWindows",
        config.include_secondary_windows.0,
        filename,
    ) != 0);
    config.hardware_encoder = b(profile_int(
        INI_SECTION,
        "HardwareEncoder",
        config.hardware_encoder.0,
        filename,
    ) != 0);
    config.hardware_prefer_integrated = b(profile_int(
        INI_SECTION,
        "HardwarePreferIntegrated",
        config.hardware_prefer_integrated.0,
        filename,
    ) != 0);

    let output_folder = profile_read_string(INI_SECTION, "OutputFolder", filename);
    if !output_folder.is_empty() {
        let w = wide(&output_folder);
        let len = w.len().min(config.output_folder.len() - 1);
        config.output_folder.fill(0);
        config.output_folder[..len].copy_from_slice(&w[..len]);
    }
    config.open_folder =
        b(profile_int(INI_SECTION, "OpenFolder", config.open_folder.0, filename) != 0);
    config.fragmented_output = b(profile_int(
        INI_SECTION,
        "FragmentedOutput",
        config.fragmented_output.0,
        filename,
    ) != 0);
    config.enable_limit_length = b(profile_int(
        INI_SECTION,
        "EnableLimitLength",
        config.enable_limit_length.0,
        filename,
    ) != 0);
    config.enable_limit_size = b(profile_int(
        INI_SECTION,
        "EnableLimitSize",
        config.enable_limit_size.0,
        filename,
    ) != 0);
    config.limit_length = profile_int(
        INI_SECTION,
        "LimitLength",
        config.limit_length as i32,
        filename,
    )
    .max(0) as u32;
    config.limit_size =
        profile_int(INI_SECTION, "LimitSize", config.limit_size as i32, filename).max(0) as u32;

    config.gamma_correct_resize = b(profile_int(
        INI_SECTION,
        "GammaCorrectResize",
        config.gamma_correct_resize.0,
        filename,
    ) != 0);
    config.improved_color_conversion = b(profile_int(
        INI_SECTION,
        "ImprovedColorConversion",
        config.improved_color_conversion.0,
        filename,
    ) != 0);
    config.video_codec = match profile_read_string(INI_SECTION, "VideoCodec", filename).as_str() {
        "H264" => CONFIG_VIDEO_H264,
        "H265" => CONFIG_VIDEO_H265,
        "AV1" => CONFIG_VIDEO_AV1,
        _ => CONFIG_VIDEO_H264,
    };
    config.video_profile = match profile_read_string(INI_SECTION, "VideoProfile", filename).as_str()
    {
        "Base" => CONFIG_VIDEO_BASE,
        "Main" => CONFIG_VIDEO_MAIN,
        "High" => CONFIG_VIDEO_HIGH,
        "Main10" => CONFIG_VIDEO_MAIN_10,
        _ => CONFIG_VIDEO_HIGH,
    };
    config.video_max_width = profile_int(
        INI_SECTION,
        "VideoMaxWidth",
        config.video_max_width as i32,
        filename,
    )
    .max(0) as u32;
    config.video_max_height = profile_int(
        INI_SECTION,
        "VideoMaxHeight",
        config.video_max_height as i32,
        filename,
    )
    .max(0) as u32;
    config.video_max_framerate = profile_int(
        INI_SECTION,
        "VideoMaxFramerate",
        config.video_max_framerate as i32,
        filename,
    )
    .max(0) as u32;
    config.video_bitrate = profile_int(
        INI_SECTION,
        "VideoBitrate",
        config.video_bitrate as i32,
        filename,
    )
    .max(0) as u32;

    config.capture_audio = b(profile_int(
        INI_SECTION,
        "CaptureAudio",
        config.capture_audio.0,
        filename,
    ) != 0);
    config.application_local_audio = b(profile_int(
        INI_SECTION,
        "ApplicationLocalAudio",
        config.application_local_audio.0,
        filename,
    ) != 0);
    config.audio_codec = match profile_read_string(INI_SECTION, "AudioCodec", filename).as_str() {
        "AAC" => CONFIG_AUDIO_AAC,
        "FLAC" => CONFIG_AUDIO_FLAC,
        _ => CONFIG_AUDIO_AAC,
    };
    config.audio_channels = profile_int(
        INI_SECTION,
        "AudioChannels",
        config.audio_channels as i32,
        filename,
    )
    .max(0) as u32;
    config.audio_samplerate =
        match profile_read_string(INI_SECTION, "AudioSamplerate", filename).as_str() {
            "44100" => 44100,
            "48000" => 48000,
            _ => 48000,
        };
    config.audio_bitrate = match profile_read_string(INI_SECTION, "AudioBitrate", filename).as_str()
    {
        "96" => 96,
        "128" => 128,
        "160" => 160,
        "192" => 192,
        _ => 160,
    };

    config.shortcut_monitor = profile_int(
        INI_SECTION,
        "ShortcutMonitor",
        config.shortcut_monitor as i32,
        filename,
    )
    .max(0) as u32;
    config.shortcut_window = profile_int(
        INI_SECTION,
        "ShortcutWindow",
        config.shortcut_window as i32,
        filename,
    )
    .max(0) as u32;
    config.shortcut_region = profile_int(
        INI_SECTION,
        "ShortcutRegion",
        config.shortcut_region as i32,
        filename,
    )
    .max(0) as u32;

    config_validate_video_profile(config);
}

pub unsafe fn config_save(config: &Config, filename: &[u16]) {
    profile_write(
        INI_SECTION,
        "MouseCursor",
        if config.mouse_cursor.as_bool() {
            "1"
        } else {
            "0"
        },
        filename,
    );
    profile_write(
        INI_SECTION,
        "OnlyClientArea",
        if config.only_client_area.as_bool() {
            "1"
        } else {
            "0"
        },
        filename,
    );
    profile_write(
        INI_SECTION,
        "ShowRecordingBorder",
        if config.show_recording_border.as_bool() {
            "1"
        } else {
            "0"
        },
        filename,
    );
    profile_write(
        INI_SECTION,
        "KeepRoundedWindowCorners",
        if config.keep_rounded_window_corners.as_bool() {
            "1"
        } else {
            "0"
        },
        filename,
    );
    profile_write(
        INI_SECTION,
        "IncludeSecondaryWindows",
        if config.include_secondary_windows.as_bool() {
            "1"
        } else {
            "0"
        },
        filename,
    );
    profile_write(
        INI_SECTION,
        "HardwareEncoder",
        if config.hardware_encoder.as_bool() {
            "1"
        } else {
            "0"
        },
        filename,
    );
    profile_write(
        INI_SECTION,
        "HardwarePreferIntegrated",
        if config.hardware_prefer_integrated.as_bool() {
            "1"
        } else {
            "0"
        },
        filename,
    );

    profile_write(
        INI_SECTION,
        "OutputFolder",
        &wide_to_string(&config.output_folder),
        filename,
    );
    profile_write(
        INI_SECTION,
        "OpenFolder",
        if config.open_folder.as_bool() {
            "1"
        } else {
            "0"
        },
        filename,
    );
    profile_write(
        INI_SECTION,
        "FragmentedOutput",
        if config.fragmented_output.as_bool() {
            "1"
        } else {
            "0"
        },
        filename,
    );
    profile_write(
        INI_SECTION,
        "EnableLimitLength",
        if config.enable_limit_length.as_bool() {
            "1"
        } else {
            "0"
        },
        filename,
    );
    profile_write(
        INI_SECTION,
        "EnableLimitSize",
        if config.enable_limit_size.as_bool() {
            "1"
        } else {
            "0"
        },
        filename,
    );
    profile_write(
        INI_SECTION,
        "LimitLength",
        &config.limit_length.to_string(),
        filename,
    );
    profile_write(
        INI_SECTION,
        "LimitSize",
        &config.limit_size.to_string(),
        filename,
    );

    profile_write(
        INI_SECTION,
        "GammaCorrectResize",
        if config.gamma_correct_resize.as_bool() {
            "1"
        } else {
            "0"
        },
        filename,
    );
    profile_write(
        INI_SECTION,
        "ImprovedColorConversion",
        if config.improved_color_conversion.as_bool() {
            "1"
        } else {
            "0"
        },
        filename,
    );
    profile_write(
        INI_SECTION,
        "VideoCodec",
        match config.video_codec {
            CONFIG_VIDEO_H264 => "H264",
            CONFIG_VIDEO_H265 => "H265",
            CONFIG_VIDEO_AV1 => "AV1",
            _ => "H264",
        },
        filename,
    );
    profile_write(
        INI_SECTION,
        "VideoProfile",
        match config.video_profile {
            CONFIG_VIDEO_BASE => "Base",
            CONFIG_VIDEO_MAIN => "Main",
            CONFIG_VIDEO_HIGH => "High",
            CONFIG_VIDEO_MAIN_10 => "Main10",
            _ => "High",
        },
        filename,
    );
    profile_write(
        INI_SECTION,
        "VideoMaxWidth",
        &config.video_max_width.to_string(),
        filename,
    );
    profile_write(
        INI_SECTION,
        "VideoMaxHeight",
        &config.video_max_height.to_string(),
        filename,
    );
    profile_write(
        INI_SECTION,
        "VideoMaxFramerate",
        &config.video_max_framerate.to_string(),
        filename,
    );
    profile_write(
        INI_SECTION,
        "VideoBitrate",
        &config.video_bitrate.to_string(),
        filename,
    );

    profile_write(
        INI_SECTION,
        "CaptureAudio",
        if config.capture_audio.as_bool() {
            "1"
        } else {
            "0"
        },
        filename,
    );
    profile_write(
        INI_SECTION,
        "ApplicationLocalAudio",
        if config.application_local_audio.as_bool() {
            "1"
        } else {
            "0"
        },
        filename,
    );
    profile_write(
        INI_SECTION,
        "AudioCodec",
        match config.audio_codec {
            CONFIG_AUDIO_AAC => "AAC",
            CONFIG_AUDIO_FLAC => "FLAC",
            _ => "AAC",
        },
        filename,
    );
    profile_write(
        INI_SECTION,
        "AudioChannels",
        &config.audio_channels.to_string(),
        filename,
    );
    profile_write(
        INI_SECTION,
        "AudioSamplerate",
        &config.audio_samplerate.to_string(),
        filename,
    );
    profile_write(
        INI_SECTION,
        "AudioBitrate",
        &config.audio_bitrate.to_string(),
        filename,
    );

    profile_write(
        INI_SECTION,
        "ShortcutMonitor",
        &config.shortcut_monitor.to_string(),
        filename,
    );
    profile_write(
        INI_SECTION,
        "ShortcutWindow",
        &config.shortcut_window.to_string(),
        filename,
    );
    profile_write(
        INI_SECTION,
        "ShortcutRegion",
        &config.shortcut_region.to_string(),
        filename,
    );
}

fn config_validate_video_profile(config: &mut Config) {
    let profiles: &[u32] = match config.video_codec {
        CONFIG_VIDEO_H264 => &[CONFIG_VIDEO_BASE, CONFIG_VIDEO_MAIN, CONFIG_VIDEO_HIGH],
        CONFIG_VIDEO_H265 => &[CONFIG_VIDEO_MAIN, CONFIG_VIDEO_MAIN_10],
        CONFIG_VIDEO_AV1 => &[CONFIG_VIDEO_MAIN, CONFIG_VIDEO_MAIN_10],
        _ => &[CONFIG_VIDEO_HIGH],
    };

    if !profiles.contains(&config.video_profile) {
        config.video_profile = profiles[0];
    }
}

pub const HOT_KEY: fn(u32, u32) -> u32 = |key, mod_| key | (mod_ << 24);
pub const HOT_GET_KEY: fn(u32) -> u32 = |key_mod| key_mod & 0xffffff;
pub const HOT_GET_MOD: fn(u32) -> u32 = |key_mod| (key_mod >> 24) & 0xff;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_ini_keeps_defaults_instead_of_enabling_zero_limits() {
        let filename =
            std::env::temp_dir().join(format!("wcap-missing-{}.ini", std::process::id()));
        assert!(!filename.exists());
        let filename = wide(&filename.to_string_lossy());
        let mut config = Config::default();
        unsafe {
            config_defaults(&mut config);
        }
        let expected = config;
        unsafe {
            config_load(&mut config, &filename);
        }
        assert_eq!(config, expected);
    }
    #[test]
    fn saved_settings_round_trip() {
        let path = std::env::temp_dir().join(format!("wcap-settings-{}.ini", std::process::id()));
        let filename = wide(&path.to_string_lossy());
        let mut config = Config::default();
        unsafe {
            config_defaults(&mut config);
        }
        config.video_codec = CONFIG_VIDEO_H265;
        config.video_profile = CONFIG_VIDEO_MAIN_10;
        config.audio_codec = CONFIG_AUDIO_FLAC;
        config.audio_channels = 1;
        config.video_bitrate = 12345;
        config.enable_limit_length = true.into();
        config.limit_length = 120;
        unsafe {
            config_save(&config, &filename);
        }
        let mut loaded = Config::default();
        unsafe {
            config_defaults(&mut loaded);
            config_load(&mut loaded, &filename);
        }
        assert_eq!(loaded, config);
        std::fs::remove_file(path).unwrap();
    }
}
