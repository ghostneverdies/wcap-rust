use crate::{
    wcap::*,
    wcap_engine::{self, Engine, EngineEvent, Stats},
    wcap_tray,
};
use std::sync::{Arc, Mutex, MutexGuard};
use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::{Dwm::*, Gdi::*},
        UI::{HiDpi::*, WindowsAndMessaging::*},
    },
};

const ACCENT: egui::Color32 = egui::Color32::from_rgb(75, 123, 237);
#[derive(Clone)]
struct MonitorEntry {
    name: String,
    handle: HMONITOR,
    rect: RECT,
    scale: f32,
}
#[derive(Clone)]
struct WindowEntry {
    handle: HWND,
    title: String,
}
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Capture,
    Settings,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
enum SettingsPage {
    #[default]
    Capture,
    Output,
    Video,
    Audio,
    Shortcuts,
}
#[derive(Default)]
struct RegionState {
    active: bool,
    input: Option<crate::region_input::InputShield>,
    background: Option<egui::ColorImage>,
    texture: Option<egui::TextureHandle>,
    monitor_index: usize,
    start: Option<egui::Pos2>,
    cur: egui::Pos2,
}
enum RegionOutcome {
    None,
    Commit(HMONITOR, RECT),
    Cancel,
}

pub struct WcapApp {
    engine: Arc<Mutex<Engine>>,
    monitors: Vec<MonitorEntry>,
    windows: Vec<WindowEntry>,
    monitor_index: usize,
    window_index: usize,
    tab: Tab,
    settings_page: SettingsPage,
    stats: Stats,
    config: Config,
    saved_config: Config,
    output_dir: String,
    saved: bool,
    region: RegionState,
    selected_region: Option<(HMONITOR, RECT)>,
}

pub fn configure_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    let mut style = (*ctx.global_style()).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size.y = 26.0;
    style.spacing.combo_width = 200.0;
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Heading, egui::FontId::proportional(22.0));
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = egui::Color32::from_rgb(17, 21, 30);
    visuals.window_fill = egui::Color32::from_rgb(25, 31, 43);
    visuals.extreme_bg_color = egui::Color32::from_rgb(18, 23, 33);
    visuals.override_text_color = Some(egui::Color32::from_rgb(231, 236, 245));
    visuals.weak_text_color = Some(egui::Color32::from_rgb(153, 166, 188));
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(37, 46, 63);
    visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(37, 46, 63);
    visuals.widgets.inactive.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(57, 68, 89));
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(48, 64, 91);
    visuals.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(48, 64, 91);
    visuals.widgets.active.bg_fill = ACCENT;
    visuals.widgets.active.weak_bg_fill = ACCENT;
    style.visuals = visuals;
    ctx.set_global_style(style);
}
fn card(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    let width = ui.available_width();
    egui::Frame::NONE
        .fill(egui::Color32::from_rgb(25, 31, 43))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(43, 53, 72)))
        .corner_radius(12)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width((width - 26.0).max(0.0));
            content(ui);
        });
}
fn action_button(ui: &mut egui::Ui, label: &str, primary: bool) -> egui::Response {
    let mut button = egui::Button::new(label);
    if primary {
        button = button.fill(ACCENT);
    }
    ui.add_sized([ui.available_width(), 32.0], button)
}
fn set_bool(ui: &mut egui::Ui, label: &str, value: &mut BOOL) {
    let mut checked = value.as_bool();
    if ui.checkbox(&mut checked, label).changed() {
        *value = checked.into();
    }
}
fn wide_to_string(wide: &[u16]) -> String {
    String::from_utf16_lossy(&wide[..wide.iter().position(|&c| c == 0).unwrap_or(wide.len())])
}
fn hotkey_to_string(packed: u32) -> String {
    if packed == 0 {
        return "Not assigned".into();
    }
    let key = packed & 0xffffff;
    let modifiers = packed >> 24;
    let mut parts = Vec::new();
    for (flag, label) in [(1, "Alt"), (2, "Ctrl"), (4, "Shift"), (8, "Win")] {
        if modifiers & flag != 0 {
            parts.push(label.into());
        }
    }
    parts.push(match key {
        0x2c => "PrtScn".into(),
        0x0d => "Enter".into(),
        0x20 => "Space".into(),
        0x09 => "Tab".into(),
        0x1b => "Esc".into(),
        0x70..=0x87 => format!("F{}", key - 0x70 + 1),
        0x30..=0x39 | 0x41..=0x5a => char::from_u32(key).unwrap().to_string(),
        _ => format!("0x{key:x}"),
    });
    parts.join(" + ")
}

impl WcapApp {
    pub fn new(engine: Arc<Mutex<Engine>>) -> Self {
        let config = engine.lock().unwrap_or_else(|p| p.into_inner()).config;
        let mut app = Self {
            engine,
            monitors: Vec::new(),
            windows: Vec::new(),
            monitor_index: 0,
            window_index: 0,
            tab: Tab::Capture,
            settings_page: SettingsPage::Capture,
            stats: Stats::default(),
            config,
            saved_config: config,
            output_dir: wide_to_string(&config.output_folder),
            saved: false,
            region: RegionState::default(),
            selected_region: None,
        };
        unsafe {
            app.refresh_monitors();
            app.refresh_windows();
        }
        app
    }
    fn lock(&self) -> MutexGuard<'_, Engine> {
        self.engine.lock().unwrap_or_else(|p| p.into_inner())
    }
    unsafe fn refresh_monitors(&mut self) {
        let mut monitors = Vec::new();
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(monitor_enum_proc),
            LPARAM(&mut monitors as *mut Vec<MonitorEntry> as isize),
        );
        self.monitors = monitors;
        self.monitor_index = self
            .monitor_index
            .min(self.monitors.len().saturating_sub(1));
    }
    unsafe fn refresh_windows(&mut self) {
        let mut windows = Vec::new();
        let _ = EnumWindows(
            Some(window_enum_proc),
            LPARAM(&mut windows as *mut Vec<WindowEntry> as isize),
        );
        self.windows = windows;
        self.window_index = self.window_index.min(self.windows.len().saturating_sub(1));
    }
    fn pump(&mut self, ctx: &egui::Context) {
        wcap_engine::pump_capture_events();
        if wcap_tray::take_quit_request() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if wcap_tray::take_settings_request() {
            self.tab = Tab::Settings;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if wcap_tray::take_region_request() && !self.region.active {
            self.open_region_picker();
        }
        let events = {
            let mut engine = self.lock();
            unsafe {
                engine.tick();
            }
            engine.take_events()
        };
        for event in events {
            if !matches!(event, EngineEvent::RecordingStateChanged) {
                unsafe {
                    wcap_tray::handle_event(event);
                }
            }
        }
        self.stats = {
            let mut engine = self.lock();
            engine.stats()
        };
        if self.stats.recording {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
    fn ui_main(&mut self, ui: &mut egui::Ui) {
        egui::Frame::NONE
            .fill(egui::Color32::from_rgb(17, 21, 30))
            .inner_margin(14)
            .show(ui, |ui| {
                ui.set_min_height(ui.available_height());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("wcap").size(24.0).strong());
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (label, color) = if self.stats.recording {
                            ("Recording", egui::Color32::from_rgb(255, 122, 134))
                        } else {
                            ("Ready", egui::Color32::from_rgb(102, 214, 171))
                        };
                        ui.label(egui::RichText::new(label).color(color));
                    });
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, Tab::Capture, "  Capture  ");
                    ui.selectable_value(&mut self.tab, Tab::Settings, "  Settings  ");
                });
                ui.add_space(8.0);
                match self.tab {
                    Tab::Capture => self.ui_capture(ui),
                    Tab::Settings => self.ui_settings(ui),
                }
            });
    }
    fn ui_capture(&mut self, ui: &mut egui::Ui) {
        ui.weak("Choose a display, an application, or a selected area.");
        ui.add_space(8.0);
        ui.add_enabled_ui(!self.stats.recording, |ui| {
            ui.columns(2, |columns| {
                self.monitor_card(&mut columns[0]);
                self.window_card(&mut columns[1]);
            });
            ui.add_space(8.0);
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.strong("Selected area");
                        ui.weak(
                            self.selected_region
                                .map(|(_, r)| {
                                    format!("{} x {} pixels", r.right - r.left, r.bottom - r.top)
                                })
                                .unwrap_or_else(|| "Select an area, then press Record.".into()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                self.selected_region.is_some(),
                                egui::Button::new("Record area").fill(ACCENT),
                            )
                            .clicked()
                        {
                            if let Some((monitor, rect)) = self.selected_region {
                                let mut engine = self.lock();
                                unsafe {
                                    if engine.capture_monitor(monitor, Some(&rect)) {
                                        engine.start_recording();
                                    }
                                }
                            }
                        }
                        if ui
                            .button(if self.selected_region.is_some() {
                                "Change area"
                            } else {
                                "Select area"
                            })
                            .clicked()
                        {
                            self.open_region_picker();
                        }
                    });
                });
            });
        });
        ui.add_space(8.0);
        card(ui, |ui| {
            ui.label(egui::RichText::new("Recording status").size(18.0).strong());
            ui.add_space(8.0);
            let seconds = self.stats.duration_msec / 1000;
            let values = [
                (
                    "Duration",
                    format!(
                        "{}:{:02}:{:02}",
                        seconds / 3600,
                        seconds / 60 % 60,
                        seconds % 60
                    ),
                ),
                (
                    "File size",
                    format!("{:.1} MB", self.stats.file_size as f64 / 1_048_576.0),
                ),
                ("Bitrate", format!("{} kb/s", self.stats.bitrate)),
                ("Dropped", self.stats.dropped_frames.to_string()),
            ];
            ui.columns(4, |columns| {
                for (column, (label, value)) in columns.iter_mut().zip(values) {
                    column.weak(label);
                    column.label(egui::RichText::new(value).size(18.0).strong());
                }
            });
            ui.add_space(8.0);
            if self.stats.recording {
                ui.weak(if self.stats.started {
                    "Use any recording shortcut to stop."
                } else {
                    "Starting recording..."
                });
                ui.add_space(10.0);
                if ui
                    .add_sized(
                        [ui.available_width(), 32.0],
                        egui::Button::new("Stop recording")
                            .fill(egui::Color32::from_rgb(149, 51, 66)),
                    )
                    .clicked()
                {
                    self.lock().request_stop();
                }
            } else {
                ui.weak(format!(
                    "Quick start: {}",
                    hotkey_to_string(self.config.shortcut_monitor)
                ));
            }
        });
        ui.add_space(8.0);
        ui.weak("Saved to");
        ui.add(egui::Label::new(&self.output_dir).truncate())
            .on_hover_text(&self.output_dir);
    }
    fn monitor_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.label(egui::RichText::new("Display").size(18.0).strong());
            ui.weak("Record an entire monitor.");
            ui.add_space(8.0);
            let selected = self
                .monitors
                .get(self.monitor_index)
                .map(|m| {
                    format!(
                        "{}  /  {} x {}",
                        m.name,
                        m.rect.right - m.rect.left,
                        m.rect.bottom - m.rect.top
                    )
                })
                .unwrap_or_else(|| "No displays found".into());
            egui::ComboBox::from_id_salt("monitor")
                .width(ui.available_width())
                .truncate()
                .selected_text(&selected)
                .show_ui(ui, |ui| {
                    for (i, monitor) in self.monitors.iter().enumerate() {
                        ui.selectable_value(&mut self.monitor_index, i, &monitor.name);
                    }
                })
                .response
                .on_hover_text(selected);
            ui.horizontal(|ui| {
                ui.weak(hotkey_to_string(self.config.shortcut_monitor));
                if ui.small_button("Refresh").clicked() {
                    unsafe {
                        self.refresh_monitors();
                    }
                }
            });
            ui.add_enabled_ui(!self.monitors.is_empty(), |ui| {
                if action_button(ui, "Record display", true).clicked() {
                    self.start_selected_target();
                }
            });
        });
    }
    fn window_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.label(egui::RichText::new("Window").size(18.0).strong());
            ui.weak("Keep the focus on one application.");
            ui.add_space(8.0);
            let selected = self
                .windows
                .get(self.window_index)
                .map(|w| w.title.clone())
                .unwrap_or_else(|| "No windows found".into());
            egui::ComboBox::from_id_salt("window")
                .width(ui.available_width())
                .truncate()
                .selected_text(&selected)
                .show_ui(ui, |ui| {
                    ui.set_max_width(440.0);
                    for (i, window) in self.windows.iter().enumerate() {
                        ui.selectable_value(&mut self.window_index, i, &window.title);
                    }
                })
                .response
                .on_hover_text(selected);
            ui.horizontal(|ui| {
                ui.weak(hotkey_to_string(self.config.shortcut_window));
                if ui.small_button("Refresh").clicked() {
                    unsafe {
                        self.refresh_windows();
                    }
                }
            });
            ui.add_enabled_ui(!self.windows.is_empty(), |ui| {
                if action_button(ui, "Record window", false).clicked() {
                    self.start_selected_window();
                }
            });
        });
    }
    fn start_selected_target(&mut self) {
        let Some(monitor) = self.monitors.get(self.monitor_index) else {
            return;
        };
        let mut engine = self.lock();
        unsafe {
            if engine.capture_monitor(monitor.handle, None) {
                engine.start_recording();
            }
        }
    }
    fn start_selected_window(&mut self) {
        let Some(window) = self.windows.get(self.window_index) else {
            return;
        };
        let mut engine = self.lock();
        unsafe {
            if engine.capture_window(window.handle, self.config.only_client_area.as_bool()) {
                engine.start_recording();
            }
        }
    }
    fn open_region_picker(&mut self) {
        if self.lock().is_recording() {
            return;
        }
        let mut cursor = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut cursor);
        }
        let monitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) };
        let monitor_index = self
            .monitors
            .iter()
            .position(|m| m.handle == monitor)
            .unwrap_or(0);
        let Some(entry) = self.monitors.get(monitor_index) else {
            return;
        };
        let Some(background) = (unsafe { desktop_snapshot(entry.rect) }) else {
            unsafe {
                wcap_tray::notify(
                    "Cannot select an area",
                    "Could not capture the desktop preview. Please try again.",
                    windows::Win32::UI::Shell::NIIF_WARNING,
                );
            }
            return;
        };
        self.region = RegionState {
            active: true,
            monitor_index,
            background: Some(background),
            ..Default::default()
        };
    }
    fn ui_settings(&mut self, ui: &mut egui::Ui) {
        ui.weak("Preferences for your next recording.");
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            for (page, title) in [
                (SettingsPage::Capture, "Capture"),
                (SettingsPage::Output, "Output"),
                (SettingsPage::Video, "Video"),
                (SettingsPage::Audio, "Audio"),
                (SettingsPage::Shortcuts, "Shortcuts"),
            ] {
                ui.selectable_value(&mut self.settings_page, page, title);
            }
        });
        ui.add_space(8.0);
        let before = self.config;
        let folder_before = self.output_dir.clone();
        ui.scope(|ui| {
            ui.add_enabled_ui(!self.stats.recording, |ui| {
                card(ui, |ui| {
                    let (title, subtitle) = match self.settings_page {
                        SettingsPage::Capture => ("Capture behavior", "Control what appears in your recording."),
                        SettingsPage::Output => ("Files & limits", "Choose where recordings go and when they stop."),
                        SettingsPage::Video => ("Video quality", "Balance clarity, file size, and performance."),
                        SettingsPage::Audio => ("Sound", "Choose the audio you want to include."),
                        SettingsPage::Shortcuts => ("Keyboard shortcuts", "These shortcuts work anywhere. Edit the INI beside wcap.exe to change them."),
                    };
                    ui.label(egui::RichText::new(title).size(20.0).strong()); ui.weak(subtitle); ui.add_space(8.0);
                    match self.settings_page {
                        SettingsPage::Capture => self.settings_capture(ui), SettingsPage::Output => self.settings_output(ui),
                        SettingsPage::Video => self.settings_video(ui), SettingsPage::Audio => self.settings_audio(ui),
                        SettingsPage::Shortcuts => {
                            egui::Grid::new("shortcut_grid").spacing([24.0, 18.0]).show(ui, |ui| {
                                for (label, key) in [("Record display", self.config.shortcut_monitor), ("Record window", self.config.shortcut_window), ("Select region", self.config.shortcut_region)] {
                                    ui.label(label); ui.label(egui::RichText::new(hotkey_to_string(key)).monospace()); ui.end_row();
                                }
                            });
                            ui.add_space(16.0); ui.weak("Press any recording shortcut again to stop.");
                        }
                    }
                });
            });
        });
        if self.config != before || self.output_dir != folder_before {
            self.saved = false;
        }
        let dirty = self.config != self.saved_config
            || self.output_dir != wide_to_string(&self.saved_config.output_folder);
        if dirty {
            ui.add_space(6.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.weak(if self.stats.recording {
                    "Stop recording to save changes."
                } else {
                    "Unsaved changes"
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            !self.stats.recording,
                            egui::Button::new("Save settings")
                                .min_size(egui::vec2(120.0, 30.0))
                                .fill(ACCENT),
                        )
                        .clicked()
                    {
                        self.apply_config();
                        self.saved = true;
                    }
                });
            });
        } else if self.saved {
            ui.weak("Settings saved");
        }
    }
    fn settings_capture(&mut self, ui: &mut egui::Ui) {
        set_bool(ui, "Show mouse cursor", &mut self.config.mouse_cursor);
        set_bool(
            ui,
            "Only capture the window's client area",
            &mut self.config.only_client_area,
        );
        set_bool(
            ui,
            "Show recording border",
            &mut self.config.show_recording_border,
        );
        set_bool(
            ui,
            "Keep rounded window corners",
            &mut self.config.keep_rounded_window_corners,
        );
        set_bool(
            ui,
            "Include secondary windows",
            &mut self.config.include_secondary_windows,
        );
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(8.0);
        set_bool(
            ui,
            "Use hardware encoder",
            &mut self.config.hardware_encoder,
        );
        ui.add_enabled_ui(self.config.hardware_encoder.as_bool(), |ui| {
            set_bool(
                ui,
                "Prefer integrated graphics",
                &mut self.config.hardware_prefer_integrated,
            );
        });
    }
    fn settings_output(&mut self, ui: &mut egui::Ui) {
        ui.strong("Recording folder");
        ui.horizontal(|ui| {
            let field_width =
                (ui.available_width() - 38.0 - ui.spacing().item_spacing.x).max(100.0);
            ui.add_sized(
                [field_width, 36.0],
                egui::TextEdit::singleline(&mut self.output_dir),
            );
            let browse = ui
                .add_sized([38.0, 36.0], egui::Button::new(""))
                .on_hover_text("Browse for a recording folder");
            browse.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    ui.is_enabled(),
                    "Browse for a recording folder",
                )
            });
            let icon = egui::Rect::from_center_size(browse.rect.center(), egui::vec2(18.0, 14.0));
            let stroke = ui.style().interact(&browse).fg_stroke;
            ui.painter().add(egui::Shape::closed_line(
                vec![
                    icon.left_bottom(),
                    icon.left_top(),
                    icon.left_top() + egui::vec2(6.0, 0.0),
                    icon.left_top() + egui::vec2(9.0, 3.0),
                    icon.right_top() + egui::vec2(0.0, 3.0),
                    icon.right_bottom(),
                ],
                egui::Stroke::new(1.5, stroke.color),
            ));
            ui.painter().line_segment(
                [
                    icon.left_top() + egui::vec2(0.0, 5.0),
                    icon.right_top() + egui::vec2(0.0, 5.0),
                ],
                egui::Stroke::new(1.5, stroke.color),
            );
            if browse.clicked() {
                match unsafe { browse_output_folder(&self.output_dir) } {
                    Ok(Some(folder)) => self.output_dir = folder,
                    Ok(None) => {}
                    Err(error) => unsafe {
                        wcap_tray::notify(
                            "Cannot choose a recording folder",
                            &error.to_string(),
                            windows::Win32::UI::Shell::NIIF_WARNING,
                        );
                    },
                }
            }
        });
        ui.add_space(8.0);
        set_bool(
            ui,
            "Open folder after recording",
            &mut self.config.open_folder,
        );
        set_bool(
            ui,
            "Fragmented MP4 output",
            &mut self.config.fragmented_output,
        );
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            set_bool(
                ui,
                "Limit recording length",
                &mut self.config.enable_limit_length,
            );
            ui.add_enabled(
                self.config.enable_limit_length.as_bool(),
                egui::DragValue::new(&mut self.config.limit_length)
                    .range(1..=86400)
                    .suffix(" s"),
            );
        });
        ui.horizontal_wrapped(|ui| {
            set_bool(
                ui,
                "Limit recording size",
                &mut self.config.enable_limit_size,
            );
            ui.add_enabled(
                self.config.enable_limit_size.as_bool(),
                egui::DragValue::new(&mut self.config.limit_size)
                    .range(1..=1_048_576)
                    .suffix(" MB"),
            );
        });
    }
    fn settings_video(&mut self, ui: &mut egui::Ui) {
        let old_codec = self.config.video_codec;
        egui::Grid::new("video_grid")
            .min_col_width(130.0)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label("Codec");
                egui::ComboBox::from_id_salt("video_codec")
                    .selected_text(match self.config.video_codec {
                        CONFIG_VIDEO_H265 => "HEVC / H.265",
                        CONFIG_VIDEO_AV1 => "AV1",
                        _ => "H.264",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.config.video_codec,
                            CONFIG_VIDEO_H264,
                            "H.264",
                        );
                        ui.selectable_value(
                            &mut self.config.video_codec,
                            CONFIG_VIDEO_H265,
                            "HEVC / H.265",
                        );
                        ui.selectable_value(&mut self.config.video_codec, CONFIG_VIDEO_AV1, "AV1");
                    });
                ui.end_row();
                if old_codec != self.config.video_codec {
                    self.config.video_profile = if self.config.video_codec == CONFIG_VIDEO_H264 {
                        CONFIG_VIDEO_HIGH
                    } else {
                        CONFIG_VIDEO_MAIN
                    };
                }
                ui.label("Profile");
                egui::ComboBox::from_id_salt("video_profile")
                    .selected_text(match self.config.video_profile {
                        CONFIG_VIDEO_BASE => "Baseline",
                        CONFIG_VIDEO_HIGH => "High",
                        CONFIG_VIDEO_MAIN_10 => "Main 10-bit",
                        _ => "Main",
                    })
                    .show_ui(ui, |ui| {
                        if self.config.video_codec == CONFIG_VIDEO_H264 {
                            ui.selectable_value(
                                &mut self.config.video_profile,
                                CONFIG_VIDEO_BASE,
                                "Baseline",
                            );
                            ui.selectable_value(
                                &mut self.config.video_profile,
                                CONFIG_VIDEO_HIGH,
                                "High",
                            );
                        } else {
                            ui.selectable_value(
                                &mut self.config.video_profile,
                                CONFIG_VIDEO_MAIN_10,
                                "Main 10-bit",
                            );
                        }
                        ui.selectable_value(
                            &mut self.config.video_profile,
                            CONFIG_VIDEO_MAIN,
                            "Main",
                        );
                    });
                ui.end_row();
                ui.label("Maximum width");
                ui.add(
                    egui::DragValue::new(&mut self.config.video_max_width)
                        .range(0..=7680)
                        .suffix(" px"),
                );
                ui.end_row();
                ui.label("Maximum height");
                ui.add(
                    egui::DragValue::new(&mut self.config.video_max_height)
                        .range(0..=4320)
                        .suffix(" px"),
                );
                ui.end_row();
                ui.label("Frame rate limit");
                ui.add(
                    egui::DragValue::new(&mut self.config.video_max_framerate)
                        .range(0..=240)
                        .suffix(" fps"),
                );
                ui.end_row();
                ui.label("Bitrate");
                ui.add(
                    egui::DragValue::new(&mut self.config.video_bitrate)
                        .range(100..=500_000)
                        .suffix(" kb/s"),
                );
                ui.end_row();
            });
        ui.add_space(6.0);
        ui.weak("Set a size or frame rate to 0 to keep the source value.");
        ui.add_space(8.0);
        set_bool(
            ui,
            "Gamma correct resize",
            &mut self.config.gamma_correct_resize,
        );
        set_bool(
            ui,
            "Improved color conversion",
            &mut self.config.improved_color_conversion,
        );
    }
    fn settings_audio(&mut self, ui: &mut egui::Ui) {
        set_bool(ui, "Capture audio", &mut self.config.capture_audio);
        ui.add_enabled_ui(self.config.capture_audio.as_bool(), |ui| {
            set_bool(
                ui,
                "Application audio only (window capture)",
                &mut self.config.application_local_audio,
            );
            ui.add_space(6.0);
            egui::Grid::new("audio_grid")
                .min_col_width(130.0)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Codec");
                    egui::ComboBox::from_id_salt("audio_codec")
                        .selected_text(if self.config.audio_codec == CONFIG_AUDIO_FLAC {
                            "FLAC (lossless)"
                        } else {
                            "AAC"
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.config.audio_codec,
                                CONFIG_AUDIO_AAC,
                                "AAC",
                            );
                            ui.selectable_value(
                                &mut self.config.audio_codec,
                                CONFIG_AUDIO_FLAC,
                                "FLAC (lossless)",
                            );
                        });
                    ui.end_row();
                    ui.label("Channels");
                    egui::ComboBox::from_id_salt("audio_channels")
                        .selected_text(if self.config.audio_channels == 1 {
                            "Mono"
                        } else {
                            "Stereo"
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.config.audio_channels, 1, "Mono");
                            ui.selectable_value(&mut self.config.audio_channels, 2, "Stereo");
                        });
                    ui.end_row();
                    ui.label("Sample rate");
                    egui::ComboBox::from_id_salt("audio_rate")
                        .selected_text(format!("{} Hz", self.config.audio_samplerate))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.config.audio_samplerate,
                                44100,
                                "44,100 Hz",
                            );
                            ui.selectable_value(
                                &mut self.config.audio_samplerate,
                                48000,
                                "48,000 Hz",
                            );
                        });
                    ui.end_row();
                    ui.label("Bitrate");
                    ui.add_enabled(
                        self.config.audio_codec == CONFIG_AUDIO_AAC,
                        egui::DragValue::new(&mut self.config.audio_bitrate)
                            .range(8..=1024)
                            .suffix(" kb/s"),
                    );
                    ui.end_row();
                });
        });
    }
    fn apply_config(&mut self) {
        let mut config = self.config;
        config.output_folder.fill(0);
        for (i, c) in self.output_dir.encode_utf16().take(259).enumerate() {
            config.output_folder[i] = c;
        }
        {
            let mut engine = self.lock();
            engine.config = config;
            unsafe {
                config_save(&engine.config, &engine.config_path);
            }
        }
        self.config = config;
        self.saved_config = config;
        self.output_dir = wide_to_string(&config.output_folder);
    }
    fn ui_region_overlay(&mut self, ctx: &egui::Context) {
        let Some(monitor) = self.monitors.get(self.region.monitor_index).cloned() else {
            self.region.active = false;
            return;
        };
        let scale = monitor.scale.max(1.0);
        let position = egui::pos2(
            monitor.rect.left as f32 / scale,
            monitor.rect.top as f32 / scale,
        );
        let size = egui::vec2(
            (monitor.rect.right - monitor.rect.left) as f32 / scale,
            (monitor.rect.bottom - monitor.rect.top) as f32 / scale,
        );
        if let Some(background) = self.region.background.take() {
            self.region.texture =
                Some(ctx.load_texture("region_desktop", background, egui::TextureOptions::NEAREST));
        }
        let id = egui::ViewportId::from_hash_of("wcap_region_overlay");
        let builder = egui::ViewportBuilder::default()
            .with_title("wcap region")
            .with_active(true)
            .with_decorations(false)
            .with_transparent(false)
            .with_mouse_passthrough(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_resizable(false)
            .with_position(position)
            .with_inner_size(size);
        let mut outcome = RegionOutcome::None;
        ctx.show_viewport_immediate(id, builder, |ui, _| {
            if self.region.input.is_none() {
                let installed = unsafe {
                    FindWindowW(None, w!("wcap region")).and_then(|window| {
                        let mut process = 0;
                        GetWindowThreadProcessId(window, Some(&mut process));
                        if process != std::process::id() {
                            return Err(Error::from_win32());
                        }
                        crate::region_input::InputShield::install(window)
                    })
                };
                match installed {
                    Ok(input) => self.region.input = Some(input),
                    Err(_) => {
                        outcome = RegionOutcome::Cancel;
                        unsafe {
                            wcap_tray::notify(
                                "Cannot select an area",
                                "Could not isolate mouse input. Please try again.",
                                windows::Win32::UI::Shell::NIIF_WARNING,
                            );
                        }
                        return;
                    }
                }
            } else if self
                .region
                .input
                .as_ref()
                .is_some_and(|input| unsafe { input.lost_focus() })
            {
                outcome = RegionOutcome::Cancel;
                return;
            }
            ui.set_min_size(size);
            let bounds = ui.max_rect();
            let response = ui.interact(bounds, egui::Id::new("region_drag"), egui::Sense::drag());
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            let painter = ui.painter();
            if let Some(texture) = &self.region.texture {
                painter.image(
                    texture.id(),
                    bounds,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }

            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                outcome = RegionOutcome::Cancel;
            }
            if response.drag_started() {
                self.region.start = response.interact_pointer_pos();
            }
            if let Some(pointer) = response.interact_pointer_pos() {
                self.region.cur = pointer;
            }
            if let Some(start) = self.region.start {
                let selection = egui::Rect::from_two_pos(start, self.region.cur).intersect(bounds);
                for outside in [
                    egui::Rect::from_min_max(
                        bounds.min,
                        egui::pos2(bounds.right(), selection.top()),
                    ),
                    egui::Rect::from_min_max(
                        egui::pos2(bounds.left(), selection.bottom()),
                        bounds.max,
                    ),
                    egui::Rect::from_min_max(
                        egui::pos2(bounds.left(), selection.top()),
                        selection.left_bottom(),
                    ),
                    egui::Rect::from_min_max(
                        selection.right_top(),
                        egui::pos2(bounds.right(), selection.bottom()),
                    ),
                ] {
                    painter.rect_filled(outside, 0, egui::Color32::from_black_alpha(45));
                }
                painter.rect_stroke(
                    selection,
                    0,
                    egui::Stroke::new(2.0, ACCENT),
                    egui::StrokeKind::Inside,
                );
                painter.text(
                    selection.left_bottom() + egui::vec2(6.0, -6.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!(
                        "{} x {}",
                        (selection.width() * scale) as i32,
                        (selection.height() * scale) as i32
                    ),
                    egui::FontId::monospace(14.0),
                    egui::Color32::WHITE,
                );
                if response.drag_stopped() {
                    outcome =
                        if selection.width() * scale >= 4.0 && selection.height() * scale >= 4.0 {
                            RegionOutcome::Commit(
                                monitor.handle,
                                RECT {
                                    left: (selection.left() * scale).round() as i32,
                                    top: (selection.top() * scale).round() as i32,
                                    right: (selection.right() * scale).round() as i32,
                                    bottom: (selection.bottom() * scale).round() as i32,
                                },
                            )
                        } else {
                            RegionOutcome::Cancel
                        };
                }
            } else {
                painter.rect_filled(bounds, 0, egui::Color32::from_black_alpha(45));
            }
            painter.text(
                bounds.center_top() + egui::vec2(0.0, 32.0),
                egui::Align2::CENTER_TOP,
                "Drag to select an area  /  Esc to cancel",
                egui::FontId::proportional(20.0),
                egui::Color32::WHITE,
            );
        });
        ctx.request_repaint();
        match outcome {
            RegionOutcome::None => {}
            RegionOutcome::Cancel => {
                self.region.active = false;
                self.region.input = None;
                self.region.start = None;
                self.region.texture = None;
                ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Close);
            }
            RegionOutcome::Commit(monitor, rect) => {
                self.region.active = false;
                self.region.input = None;
                self.region.start = None;
                self.region.texture = None;
                ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Close);
                self.selected_region = Some((monitor, rect));
                self.tab = Tab::Capture;
            }
        }
    }
}
unsafe fn browse_output_folder(current: &str) -> Result<Option<String>> {
    use windows::Win32::{
        System::Com::*,
        UI::{Input::KeyboardAndMouse::GetActiveWindow, Shell::*},
    };
    let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
    dialog.SetOptions(
        dialog.GetOptions()?
            | FOS_PICKFOLDERS
            | FOS_FORCEFILESYSTEM
            | FOS_PATHMUSTEXIST
            | FOS_NOCHANGEDIR
            | FOS_DONTADDTORECENT,
    )?;
    dialog.SetTitle(w!("Choose recording folder"))?;
    if let Ok(folder) =
        SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(current), None)
    {
        let _ = dialog.SetFolder(&folder);
    }
    let owner = GetActiveWindow();
    if let Err(error) = dialog.Show(if owner.0.is_null() { None } else { Some(owner) }) {
        return if error.code() == HRESULT::from_win32(ERROR_CANCELLED.0) {
            Ok(None)
        } else {
            Err(error)
        };
    }
    let path = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
    let selected = path.to_string();
    CoTaskMemFree(Some(path.0 as *const _));
    let selected = selected?;
    if selected.encode_utf16().count() >= 260 {
        return Err(Error::new(
            E_INVALIDARG,
            "The recording folder path must be shorter than 260 characters.",
        ));
    }
    Ok(Some(selected))
}

unsafe fn desktop_snapshot(rect: RECT) -> Option<egui::ColorImage> {
    let width = rect.right.checked_sub(rect.left)?;
    let height = rect.bottom.checked_sub(rect.top)?;
    if width <= 0 || height <= 0 {
        return None;
    }
    let length = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    let screen = GetDC(None);
    if screen.0.is_null() {
        return None;
    }
    let memory = CreateCompatibleDC(Some(screen));
    if memory.0.is_null() {
        ReleaseDC(None, screen);
        return None;
    }
    let mut info = BITMAPINFO::default();
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = width;
    info.bmiHeader.biHeight = -height;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB.0;
    let mut pixels = std::ptr::null_mut();
    let result = (|| {
        let bitmap =
            CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut pixels, None, 0).ok()?;
        let previous = SelectObject(memory, HGDIOBJ(bitmap.0));
        let copied = BitBlt(
            memory,
            0,
            0,
            width,
            height,
            Some(screen),
            rect.left,
            rect.top,
            SRCCOPY | CAPTUREBLT,
        )
        .is_ok();
        let _ = GdiFlush();
        let image = if copied && !pixels.is_null() {
            let mut rgba = std::slice::from_raw_parts(pixels as *const u8, length).to_vec();
            for pixel in rgba.chunks_exact_mut(4) {
                pixel.swap(0, 2);
                pixel[3] = 255;
            }
            Some(egui::ColorImage::from_rgba_unmultiplied(
                [width as usize, height as usize],
                &rgba,
            ))
        } else {
            None
        };
        SelectObject(memory, previous);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        image
    })();
    let _ = DeleteDC(memory);
    ReleaseDC(None, screen);
    result
}

unsafe extern "system" fn monitor_enum_proc(
    monitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    param: LPARAM,
) -> BOOL {
    let output = &mut *(param.0 as *mut Vec<MonitorEntry>);
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(monitor, &mut info.monitorInfo).as_bool() {
        let mut x = 0;
        let mut y = 0;
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y);
        output.push(MonitorEntry {
            name: wide_to_string(&info.szDevice).replace(r"\\.\DISPLAY", "Display "),
            handle: monitor,
            rect: info.monitorInfo.rcMonitor,
            scale: if x > 0 { x as f32 / 96.0 } else { 1.0 },
        });
    }
    TRUE
}
unsafe extern "system" fn window_enum_proc(window: HWND, param: LPARAM) -> BOOL {
    let mut class = [0u16; 256];
    GetClassNameW(window, &mut class);
    let class = wide_to_string(&class);
    let mut cloaked = 0u32;
    let _ = DwmGetWindowAttribute(window, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut _, 4);
    let mut process = 0;
    GetWindowThreadProcessId(window, Some(&mut process));
    if cloaked != 0
        || process == std::process::id()
        || [
            "Progman",
            "WorkerW",
            "Shell_TrayWnd",
            "Shell_SecondaryTrayWnd",
            "Windows.UI.Core.CoreWindow",
            "XamlExplorerHostIslandWindow",
            "ApplicationManager_DesktopShellWindow",
        ]
        .contains(&class.as_str())
        || GetWindowLongPtrW(window, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0
    {
        return TRUE;
    }
    if GetAncestor(window, GA_ROOTOWNER) == window
        && GetAncestor(window, GA_ROOT) == window
        && IsWindowVisible(window).as_bool()
    {
        let mut title = [0u16; 256];
        let len = GetWindowTextW(window, &mut title);
        if len > 0
            && ![
                "Program Manager",
                "Windows Input Experience",
                "Microsoft Text Input Application",
            ]
            .contains(&wide_to_string(&title).as_str())
        {
            (&mut *(param.0 as *mut Vec<WindowEntry>)).push(WindowEntry {
                handle: window,
                title: String::from_utf16_lossy(&title[..len as usize]),
            });
        }
    }
    TRUE
}
impl eframe::App for WcapApp {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pump(&ctx);
        self.ui_main(ui);
        if self.region.active && !self.stats.recording {
            self.ui_region_overlay(&ctx);
        }
    }
}
