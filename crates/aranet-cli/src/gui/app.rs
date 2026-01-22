//! Main application state and UI rendering for the Aranet GUI.
//!
//! This module contains the [`AranetApp`] struct which implements the egui application,
//! handling user input, rendering, and coordinating with the background BLE worker.

use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aranet_core::messages::{Command, SensorEvent};
use aranet_core::settings::{DeviceSettings, RadonUnit, TemperatureUnit};
use aranet_core::{BluetoothRange, DeviceType};
use eframe::egui::{self, Color32, RichText, UserData, ViewportCommand};
use egui_plot::{HLine, Line, Plot, PlotPoints};
use tokio::sync::mpsc;
use tracing::{debug, info};

use super::components;
use super::theme::{Theme, ThemeMode};
use super::tray::{
    check_co2_threshold, hide_dock_icon, show_dock_icon, TrayCommand, TrayManager, TrayState,
};
use super::types::{
    Co2Level, ConnectionState, DeviceState, HistoryFilter, RadiationLevel, RadonLevel, Tab, Trend,
};

/// Default scan duration.
const SCAN_DURATION: Duration = Duration::from_secs(5);

/// How long toast notifications are displayed.
const TOAST_DURATION: Duration = Duration::from_secs(4);

/// Toast notification type.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Info may be used later
enum ToastType {
    Success,
    Error,
    Info,
}

/// A toast notification.
#[derive(Debug, Clone)]
struct Toast {
    message: String,
    toast_type: ToastType,
    created_at: Instant,
}

/// Available measurement intervals in seconds.
const INTERVAL_OPTIONS: &[(u16, &str)] = &[
    (60, "1 min"),
    (120, "2 min"),
    (300, "5 min"),
    (600, "10 min"),
];

/// Convert Celsius to Fahrenheit.
#[inline]
fn celsius_to_fahrenheit(celsius: f32) -> f32 {
    celsius * 9.0 / 5.0 + 32.0
}

/// Convert Bq/m³ to pCi/L (1 Bq/m³ = 0.027 pCi/L).
#[inline]
fn bq_to_pci(bq: u32) -> f32 {
    bq as f32 * 0.027
}

/// Format temperature value and unit based on device settings.
///
/// Returns (value_string, unit_string) tuple.
fn format_temperature(celsius: f32, settings: Option<&DeviceSettings>) -> (String, &'static str) {
    let use_fahrenheit = settings
        .map(|s| s.temperature_unit == TemperatureUnit::Fahrenheit)
        .unwrap_or(false);

    if use_fahrenheit {
        (format!("{:.1}", celsius_to_fahrenheit(celsius)), "F")
    } else {
        (format!("{:.1}", celsius), "C")
    }
}

/// Format radon value and unit based on device settings.
///
/// Returns (value_string, unit_string) tuple.
fn format_radon(bq: u32, settings: Option<&DeviceSettings>) -> (String, &'static str) {
    let use_pci = settings
        .map(|s| s.radon_unit == RadonUnit::PciL)
        .unwrap_or(false);

    if use_pci {
        (format!("{:.2}", bq_to_pci(bq)), "pCi/L")
    } else {
        (format!("{}", bq), "Bq/m3")
    }
}

/// Main application state.
pub struct AranetApp {
    /// Channel to send commands to the worker.
    command_tx: mpsc::Sender<Command>,
    /// Channel to receive events from the worker (via std mpsc for non-async).
    event_rx: std_mpsc::Receiver<SensorEvent>,
    /// List of discovered/connected devices.
    devices: Vec<DeviceState>,
    /// Currently selected device index.
    selected_device: Option<usize>,
    /// Whether a scan is in progress.
    scanning: bool,
    /// Status message.
    status: String,
    /// Active tab/view.
    active_tab: Tab,
    /// History time filter.
    history_filter: HistoryFilter,
    /// Whether a settings update is in progress.
    updating_settings: bool,
    /// When the last auto-refresh was triggered.
    last_auto_refresh: Option<Instant>,
    /// Whether auto-refresh is enabled.
    auto_refresh_enabled: bool,
    /// Current theme mode (dark/light).
    theme_mode: ThemeMode,
    /// Current theme colors.
    theme: Theme,
    /// Active toast notifications.
    toasts: Vec<Toast>,
    /// Shared tray state for system tray integration.
    tray_state: Arc<Mutex<TrayState>>,
    /// System tray manager (if tray is available).
    tray_manager: Option<TrayManager>,
    /// Whether the main window is visible (for close-to-tray behavior).
    window_visible: bool,
    /// Whether to minimize to tray instead of quitting when closing window.
    close_to_tray: bool,
    /// Whether running in demo mode with mock data.
    demo_mode: bool,
    /// Path to save screenshot (if taking screenshot).
    screenshot_path: Option<std::path::PathBuf>,
    /// Frame counter for screenshot delay.
    frame_count: u32,
    /// Number of frames to wait before taking screenshot.
    screenshot_delay_frames: u32,
}

impl AranetApp {
    /// Create a new AranetApp instance.
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        command_tx: mpsc::Sender<Command>,
        event_rx: std_mpsc::Receiver<SensorEvent>,
        tray_state: Arc<Mutex<TrayState>>,
        tray_manager: Option<TrayManager>,
    ) -> Self {
        Self::new_with_options(cc, command_tx, event_rx, tray_state, tray_manager, false, None, 3)
    }

    /// Create a new AranetApp instance with demo/screenshot options.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_options(
        cc: &eframe::CreationContext<'_>,
        command_tx: mpsc::Sender<Command>,
        event_rx: std_mpsc::Receiver<SensorEvent>,
        tray_state: Arc<Mutex<TrayState>>,
        tray_manager: Option<TrayManager>,
        demo_mode: bool,
        screenshot_path: Option<std::path::PathBuf>,
        screenshot_delay_frames: u32,
    ) -> Self {
        // Initialize theme and apply it
        let theme_mode = ThemeMode::default();
        let theme = Theme::for_mode(theme_mode);
        cc.egui_ctx.set_style(theme.to_style());

        // Close-to-tray is enabled only when tray is available
        let close_to_tray = tray_manager.is_some();

        // Load demo devices if in demo mode
        let devices = if demo_mode {
            super::demo::create_demo_devices()
        } else {
            Vec::new()
        };

        // Select first device in demo mode
        let selected_device = if demo_mode && !devices.is_empty() {
            Some(0)
        } else {
            None
        };

        let status = if demo_mode {
            "Demo Mode - 3 devices loaded".to_string()
        } else {
            "Ready - Click 'Scan' to discover devices".to_string()
        };

        Self {
            command_tx,
            event_rx,
            devices,
            selected_device,
            scanning: false,
            status,
            active_tab: Tab::Dashboard,
            history_filter: HistoryFilter::All,
            updating_settings: false,
            last_auto_refresh: None,
            auto_refresh_enabled: !demo_mode, // Disable auto-refresh in demo mode
            theme_mode,
            theme,
            toasts: Vec::new(),
            tray_state,
            tray_manager,
            window_visible: true,
            close_to_tray,
            demo_mode,
            screenshot_path,
            frame_count: 0,
            screenshot_delay_frames,
        }
    }

    /// Add a toast notification.
    fn add_toast(&mut self, message: impl Into<String>, toast_type: ToastType) {
        self.toasts.push(Toast {
            message: message.into(),
            toast_type,
            created_at: Instant::now(),
        });
    }

    /// Remove expired toasts.
    fn cleanup_toasts(&mut self) {
        self.toasts.retain(|t| t.created_at.elapsed() < TOAST_DURATION);
    }

    /// Process system tray events and handle commands.
    fn process_tray_events(&mut self, ctx: &egui::Context) {
        let Some(ref tray_manager) = self.tray_manager else {
            return;
        };

        // Process tray events
        for command in tray_manager.process_events() {
            match command {
                TrayCommand::ShowWindow => {
                    debug!("Tray command: ShowWindow");
                    self.window_visible = true;
                    show_dock_icon();
                    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }
                TrayCommand::HideWindow => {
                    debug!("Tray command: HideWindow");
                    self.window_visible = false;
                    ctx.send_viewport_cmd(ViewportCommand::Visible(false));
                    hide_dock_icon();
                }
                TrayCommand::ToggleWindow => {
                    debug!("Tray command: ToggleWindow, visible={}", self.window_visible);
                    self.window_visible = !self.window_visible;
                    ctx.send_viewport_cmd(ViewportCommand::Visible(self.window_visible));
                    if self.window_visible {
                        show_dock_icon();
                        ctx.send_viewport_cmd(ViewportCommand::Focus);
                    } else {
                        hide_dock_icon();
                    }
                }
                TrayCommand::Quit => {
                    debug!("Tray command: Quit");
                    show_dock_icon(); // Restore dock icon before quitting
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }

        // Sync window visibility to tray state
        if let Ok(mut state) = self.tray_state.lock() {
            state.window_visible = self.window_visible;
        }
    }

    /// Update tray state with current sensor readings.
    fn update_tray_state(&self, device_name: &str, co2_ppm: Option<u16>) {
        if let Ok(mut state) = self.tray_state.lock() {
            state.device_name = Some(device_name.to_string());
            if let Some(co2) = co2_ppm {
                check_co2_threshold(&mut state, co2, device_name);
            }
        }

        // Update tray tooltip
        if let Some(ref tray_manager) = self.tray_manager {
            tray_manager.update_tooltip();
        }
    }

    /// Process all pending events from the worker.
    fn process_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            self.handle_event(event);
        }
    }

    /// Send a command to the worker.
    fn send_command(&self, cmd: Command) {
        let _ = self.command_tx.try_send(cmd);
    }

    /// Check if auto-refresh is due and refresh connected devices.
    fn check_auto_refresh(&mut self) {
        if !self.auto_refresh_enabled {
            return;
        }

        // Get the shortest interval from connected devices (or default 60s)
        let interval_secs = self
            .devices
            .iter()
            .filter(|d| matches!(d.connection, ConnectionState::Connected))
            .filter_map(|d| d.reading.as_ref())
            .map(|r| r.interval)
            .filter(|&i| i > 0)
            .min()
            .unwrap_or(60) as u64;

        let interval = Duration::from_secs(interval_secs);
        let now = Instant::now();

        let should_refresh = match self.last_auto_refresh {
            Some(last) => now.duration_since(last) >= interval,
            None => {
                self.last_auto_refresh = Some(now);
                false
            }
        };

        if should_refresh {
            self.last_auto_refresh = Some(now);
            let connected_ids: Vec<_> = self
                .devices
                .iter()
                .filter(|d| matches!(d.connection, ConnectionState::Connected))
                .map(|d| d.id.clone())
                .collect();

            for device_id in connected_ids {
                self.send_command(Command::RefreshReading { device_id });
            }
        }
    }

    /// Handle a single event from the worker.
    fn handle_event(&mut self, event: SensorEvent) {
        match event {
            SensorEvent::ScanStarted => {
                self.scanning = true;
                self.status = "Scanning for devices...".to_string();
            }
            SensorEvent::ScanComplete { devices } => {
                self.scanning = false;
                self.status = format!("Found {} device(s)", devices.len());
                for discovered in devices {
                    if !self.devices.iter().any(|d| d.id == discovered.identifier) {
                        self.devices.push(DeviceState::from_discovered(&discovered));
                    }
                }
            }
            SensorEvent::ScanError { error } => {
                self.scanning = false;
                self.add_toast(format!("Scan failed: {}", error), ToastType::Error);
            }
            SensorEvent::DeviceConnecting { device_id } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.connection = ConnectionState::Connecting;
                }
                self.status = "Connecting...".to_string();
            }
            SensorEvent::DeviceConnected {
                device_id,
                name,
                device_type,
                rssi,
            } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.connection = ConnectionState::Connected;
                    if name.is_some() {
                        device.name = name;
                    }
                    if device_type.is_some() {
                        device.device_type = device_type;
                    }
                    if rssi.is_some() {
                        device.rssi = rssi;
                    }
                }
                self.status = "Connected".to_string();
            }
            SensorEvent::DeviceDisconnected { device_id } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.connection = ConnectionState::Disconnected;
                }
            }
            SensorEvent::ConnectionError { device_id, error } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.connection = ConnectionState::Error(error.clone());
                }
                self.add_toast(format!("Connection failed: {}", error), ToastType::Error);
            }
            SensorEvent::ReadingUpdated { device_id, reading } => {
                // Extract CO2 for tray notification before consuming reading
                let co2_ppm = if reading.co2 > 0 { Some(reading.co2) } else { None };
                let device_name = self
                    .devices
                    .iter()
                    .find(|d| d.id == device_id)
                    .map(|d| d.display_name().to_string())
                    .unwrap_or_else(|| device_id.clone());

                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.update_reading(reading);
                }

                // Update tray state with new reading
                self.update_tray_state(&device_name, co2_ppm);
                self.status = "Reading updated".to_string();
            }
            SensorEvent::ReadingError { device_id, error } => {
                self.add_toast(
                    format!("Reading error for {}: {}", device_id, error),
                    ToastType::Error,
                );
            }
            SensorEvent::HistorySyncStarted { device_id } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.syncing_history = true;
                }
                self.status = "Syncing history...".to_string();
            }
            SensorEvent::HistoryLoaded { device_id, records } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.history = records;
                    device.syncing_history = false;
                }
            }
            SensorEvent::HistorySynced { device_id, count } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.syncing_history = false;
                }
                self.status = format!("Synced {} history records", count);
            }
            SensorEvent::HistorySyncError { device_id, error } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.syncing_history = false;
                }
                self.add_toast(format!("History sync failed: {}", error), ToastType::Error);
            }
            SensorEvent::SettingsLoaded { device_id, settings } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.settings = Some(settings);
                }
            }
            SensorEvent::IntervalChanged {
                device_id,
                interval_secs,
            } => {
                self.updating_settings = false;
                self.status = format!("Interval set to {} min", interval_secs / 60);
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id)
                    && let Some(reading) = &mut device.reading
                {
                    reading.interval = interval_secs;
                }
            }
            SensorEvent::IntervalError { device_id: _, error } => {
                self.updating_settings = false;
                self.add_toast(format!("Failed to set interval: {}", error), ToastType::Error);
            }
            SensorEvent::BluetoothRangeChanged {
                device_id: _,
                extended,
            } => {
                self.updating_settings = false;
                let range = if extended { "Extended" } else { "Standard" };
                self.status = format!("Bluetooth range set to {}", range);
            }
            SensorEvent::BluetoothRangeError { device_id: _, error } => {
                self.updating_settings = false;
                self.add_toast(format!("Failed to set BT range: {}", error), ToastType::Error);
            }
            SensorEvent::SmartHomeChanged {
                device_id: _,
                enabled,
            } => {
                self.updating_settings = false;
                let mode = if enabled { "enabled" } else { "disabled" };
                self.add_toast(format!("Smart Home {}", mode), ToastType::Success);
            }
            SensorEvent::SmartHomeError { device_id: _, error } => {
                self.updating_settings = false;
                self.add_toast(
                    format!("Failed to set Smart Home: {}", error),
                    ToastType::Error,
                );
            }
            SensorEvent::CachedDataLoaded { devices } => {
                for cached in devices {
                    if !self.devices.iter().any(|d| d.id == cached.id) {
                        self.devices.push(DeviceState::from_cached(&cached));
                    }
                }
                if !self.devices.is_empty() {
                    self.status = format!("Loaded {} cached device(s)", self.devices.len());
                }
            }
        }
    }
}

impl eframe::App for AranetApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_events();
        self.check_auto_refresh();
        self.cleanup_toasts();
        self.process_tray_events(ctx);

        // Handle screenshot capture in demo mode
        if self.screenshot_path.is_some() {
            self.frame_count += 1;

            // Check if we received a screenshot event
            let screenshot_image = ctx.input(|i| {
                for event in &i.events {
                    if let egui::Event::Screenshot { image, .. } = event {
                        return Some(image.clone());
                    }
                }
                None
            });

            if let Some(image) = screenshot_image {
                if let Some(ref path) = self.screenshot_path {
                    // Save the screenshot
                    let pixels = image.as_raw();
                    let size = image.size;
                    if let Err(e) = image::save_buffer(
                        path,
                        pixels,
                        size[0] as u32,
                        size[1] as u32,
                        image::ColorType::Rgba8,
                    ) {
                        tracing::error!("Failed to save screenshot: {}", e);
                    } else {
                        info!("Screenshot saved to {:?}", path);
                    }
                    // Exit after saving screenshot
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                    return;
                }
            }

            // Request screenshot after delay frames
            if self.frame_count == self.screenshot_delay_frames {
                ctx.send_viewport_cmd(ViewportCommand::Screenshot(UserData::default()));
            }
        }

        // Handle close-to-tray behavior
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if close_requested && self.close_to_tray {
            // Cancel the close and hide to tray instead
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.window_visible = false;
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
            hide_dock_icon();
            debug!("Window close intercepted - minimizing to tray");
        }

        // Handle keyboard shortcuts
        let mut toggle_theme = false;
        ctx.input(|i| {
            if i.key_pressed(egui::Key::F5) && !self.scanning {
                self.send_command(Command::Scan {
                    duration: SCAN_DURATION,
                });
            }
            if i.modifiers.command && i.key_pressed(egui::Key::R) {
                for device in &self.devices {
                    if matches!(device.connection, ConnectionState::Connected) {
                        self.send_command(Command::RefreshReading {
                            device_id: device.id.clone(),
                        });
                    }
                }
            }
            if i.key_pressed(egui::Key::Num1) {
                self.active_tab = Tab::Dashboard;
            }
            if i.key_pressed(egui::Key::Num2) {
                self.active_tab = Tab::History;
            }
            if i.key_pressed(egui::Key::Num3) {
                self.active_tab = Tab::Settings;
            }
            if i.key_pressed(egui::Key::T) && !i.modifiers.command && !i.modifiers.ctrl {
                toggle_theme = true;
            }
            if i.key_pressed(egui::Key::A) && !i.modifiers.command && !i.modifiers.ctrl {
                self.auto_refresh_enabled = !self.auto_refresh_enabled;
            }
        });
        if toggle_theme {
            self.theme_mode.toggle();
            self.theme = Theme::for_mode(self.theme_mode);
            ctx.set_style(self.theme.to_style());
        }

        ctx.request_repaint_after(Duration::from_millis(100));

        // Render toast notifications
        if !self.toasts.is_empty() {
            egui::Area::new(egui::Id::new("toasts"))
                .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -40.0))
                .show(ctx, |ui| {
                    ui.with_layout(egui::Layout::bottom_up(egui::Align::RIGHT), |ui| {
                        for toast in &self.toasts {
                            let (bg_color, icon) = match toast.toast_type {
                                ToastType::Success => (self.theme.success, "[OK]"),
                                ToastType::Error => (self.theme.danger, "[!]"),
                                ToastType::Info => (self.theme.info, "[i]"),
                            };
                            let elapsed = toast.created_at.elapsed().as_secs_f32();
                            let fade_start = TOAST_DURATION.as_secs_f32() - 0.5;
                            let alpha = if elapsed > fade_start {
                                1.0 - (elapsed - fade_start) / 0.5
                            } else {
                                1.0
                            };

                            let toast_text_color =
                                self.theme.text_on_accent.gamma_multiply(alpha);
                            egui::Frame::new()
                                .fill(bg_color.gamma_multiply(0.95 * alpha))
                                .inner_margin(egui::Margin::symmetric(12, 8))
                                .corner_radius(egui::CornerRadius::same(6))
                                .shadow(egui::Shadow {
                                    offset: [0, 2],
                                    blur: 8,
                                    spread: 0,
                                    color: Color32::from_black_alpha((40.0 * alpha) as u8),
                                })
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(icon)
                                                .color(toast_text_color)
                                                .strong(),
                                        );
                                        ui.label(
                                            RichText::new(&toast.message).color(toast_text_color),
                                        );
                                    });
                                });
                            ui.add_space(4.0);
                        }
                    });
                });
        }

        // Top panel with title, tabs, and scan button
        egui::TopBottomPanel::top("header")
            .frame(
                egui::Frame::new()
                    .fill(self.theme.bg_secondary)
                    .inner_margin(egui::Margin::symmetric(
                        self.theme.spacing.lg as i8,
                        self.theme.spacing.md as i8,
                    ))
                    .stroke(egui::Stroke::new(1.0, self.theme.border_subtle)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // App title
                    ui.label(
                        RichText::new("Aranet")
                            .size(self.theme.typography.heading)
                            .strong()
                            .color(self.theme.text_primary),
                    );

                    ui.add_space(self.theme.spacing.lg);
                    ui.separator();
                    ui.add_space(self.theme.spacing.sm);

                    // Tab navigation
                    for (tab, label, shortcut) in [
                        (Tab::Dashboard, "Dashboard", "1"),
                        (Tab::History, "History", "2"),
                        (Tab::Settings, "Settings", "3"),
                    ] {
                        let is_selected = self.active_tab == tab;
                        let text_color = if is_selected {
                            self.theme.accent
                        } else {
                            self.theme.text_secondary
                        };

                        let response = ui.add(
                            egui::Label::new(
                                RichText::new(label)
                                    .size(self.theme.typography.body)
                                    .color(text_color),
                            )
                            .selectable(false)
                            .sense(egui::Sense::click()),
                        );

                        if response.clicked() {
                            self.active_tab = tab;
                        }
                        let rect = response.rect;
                        response.on_hover_text(format!("Press {}", shortcut));

                        // Underline for selected tab
                        if is_selected {
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(
                                    egui::pos2(rect.min.x, rect.max.y + 2.0),
                                    egui::vec2(rect.width(), 2.0),
                                ),
                                egui::CornerRadius::same(1),
                                self.theme.accent,
                            );
                        }

                        ui.add_space(self.theme.spacing.md);
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Theme toggle
                        if ui
                            .add(egui::Button::new(
                                RichText::new(self.theme_mode.icon())
                                    .size(self.theme.typography.caption),
                            ))
                            .on_hover_text("Press T to toggle theme")
                            .clicked()
                        {
                            self.theme_mode.toggle();
                            self.theme = Theme::for_mode(self.theme_mode);
                            ctx.set_style(self.theme.to_style());
                        }

                        ui.add_space(self.theme.spacing.sm);

                        // Auto-refresh toggle
                        let auto_color = if self.auto_refresh_enabled {
                            self.theme.success
                        } else {
                            self.theme.text_muted
                        };
                        if ui
                            .add(egui::Button::new(
                                RichText::new(if self.auto_refresh_enabled {
                                    "Auto: On"
                                } else {
                                    "Auto: Off"
                                })
                                .size(self.theme.typography.caption)
                                .color(auto_color),
                            ))
                            .on_hover_text("Press A to toggle auto-refresh")
                            .clicked()
                        {
                            self.auto_refresh_enabled = !self.auto_refresh_enabled;
                        }

                        ui.add_space(self.theme.spacing.sm);

                        // Scan button
                        ui.add_enabled_ui(!self.scanning, |ui| {
                            let scan_btn = egui::Button::new(
                                RichText::new("Scan")
                                    .size(self.theme.typography.body)
                                    .color(self.theme.text_on_accent),
                            )
                            .fill(self.theme.accent);
                            if ui.add(scan_btn).on_hover_text("F5").clicked() {
                                self.send_command(Command::Scan {
                                    duration: SCAN_DURATION,
                                });
                            }
                        });
                        if self.scanning {
                            ui.spinner();
                        }
                    });
                });
            });

        // Bottom panel with status
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(self.theme.bg_secondary)
                    .inner_margin(egui::Margin::symmetric(
                        self.theme.spacing.lg as i8,
                        self.theme.spacing.sm as i8,
                    ))
                    .stroke(egui::Stroke::new(1.0, self.theme.border_subtle)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // Status indicator dot
                    let status_color = if self.scanning {
                        self.theme.warning
                    } else if self.devices.iter().any(|d| {
                        matches!(d.connection, ConnectionState::Connected)
                    }) {
                        self.theme.success
                    } else {
                        self.theme.text_muted
                    };
                    components::status_dot(ui, status_color, "Connection status");
                    ui.add_space(self.theme.spacing.sm);

                    ui.label(
                        RichText::new(&self.status)
                            .color(self.theme.text_muted)
                            .size(self.theme.typography.caption),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new("F5: Scan | Cmd+R: Refresh | T: Theme | A: Auto")
                                .color(self.theme.text_muted)
                                .size(self.theme.typography.caption),
                        );
                    });
                });
            });

        // Left panel with device list
        self.render_device_list(ctx);

        // Central panel
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(self.theme.bg_primary)
                    .inner_margin(egui::Margin::same(self.theme.spacing.lg as i8)),
            )
            .show(ctx, |ui| {
                if let Some(idx) = self.selected_device {
                    let device = self.devices.get(idx).cloned();
                    if let Some(device) = device {
                        match self.active_tab {
                            Tab::Dashboard => self.render_device_panel(ui, &device, idx),
                            Tab::History => self.render_history_panel(ui, &device),
                            Tab::Settings => self.render_settings_panel(ui, &device),
                        }
                    }
                } else {
                    components::empty_state(
                        ui,
                        &self.theme,
                        "Select a Device",
                        "Choose a device from the sidebar to view readings",
                    );
                }
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        info!("Sending shutdown command");
        let _ = self.command_tx.try_send(Command::Shutdown);
    }
}

impl AranetApp {
    /// Render the device list side panel.
    fn render_device_list(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("devices")
            .min_width(240.0)
            .max_width(280.0)
            .frame(
                egui::Frame::new()
                    .fill(self.theme.bg_secondary)
                    .inner_margin(egui::Margin::same(self.theme.spacing.md as i8))
                    .stroke(egui::Stroke::new(1.0, self.theme.border_subtle)),
            )
            .show(ctx, |ui| {
                // Header
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Devices")
                            .size(self.theme.typography.subheading)
                            .strong()
                            .color(self.theme.text_primary),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{}", self.devices.len()))
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_muted),
                        );
                    });
                });
                ui.add_space(self.theme.spacing.sm);
                ui.separator();
                ui.add_space(self.theme.spacing.sm);

                if self.devices.is_empty() {
                    components::empty_state(
                        ui,
                        &self.theme,
                        "No Devices",
                        "Click 'Scan' to discover nearby devices",
                    );
                } else {
                    let mut device_indices: Vec<usize> = (0..self.devices.len()).collect();
                    device_indices.sort_by(|&a, &b| {
                        let dev_a = &self.devices[a];
                        let dev_b = &self.devices[b];
                        let conn_a = matches!(dev_a.connection, ConnectionState::Connected);
                        let conn_b = matches!(dev_b.connection, ConnectionState::Connected);
                        conn_b
                            .cmp(&conn_a)
                            .then_with(|| dev_a.display_name().cmp(dev_b.display_name()))
                    });

                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let mut new_selection = self.selected_device;
                        for i in device_indices {
                            let device = &self.devices[i];
                            let selected = self.selected_device == Some(i);
                            let (frame_fill, border_color) = if selected {
                                (
                                    self.theme.tint_bg(self.theme.accent, 20),
                                    self.theme.accent,
                                )
                            } else {
                                (Color32::TRANSPARENT, self.theme.border_subtle)
                            };

                            let response = egui::Frame::new()
                                .fill(frame_fill)
                                .inner_margin(egui::Margin::same(self.theme.spacing.md as i8))
                                .corner_radius(egui::CornerRadius::same(
                                    self.theme.rounding.md as u8,
                                ))
                                .stroke(egui::Stroke::new(1.0, border_color))
                                .show(ui, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    ui.vertical(|ui| {
                                        // Device name row
                                        ui.horizontal(|ui| {
                                            let (dot_color, status_tip) = match &device.connection {
                                                ConnectionState::Disconnected => {
                                                    (self.theme.text_muted, "Disconnected")
                                                }
                                                ConnectionState::Connecting => {
                                                    (self.theme.warning, "Connecting...")
                                                }
                                                ConnectionState::Connected => {
                                                    (self.theme.success, "Connected")
                                                }
                                                ConnectionState::Error(_) => {
                                                    (self.theme.danger, "Connection error")
                                                }
                                            };
                                            components::status_dot(ui, dot_color, status_tip);
                                            ui.add_space(self.theme.spacing.sm);

                                            let name_color = if selected {
                                                self.theme.accent
                                            } else {
                                                self.theme.text_primary
                                            };
                                            ui.label(
                                                RichText::new(device.display_name())
                                                    .color(name_color)
                                                    .size(self.theme.typography.body)
                                                    .strong(),
                                            );
                                        });

                                        // Device info row
                                        ui.add_space(self.theme.spacing.xs);
                                        ui.horizontal(|ui| {
                                            if let Some(device_type) = device.device_type {
                                                let type_label = match device_type {
                                                    DeviceType::Aranet4 => "CO2",
                                                    DeviceType::Aranet2 => "T/H",
                                                    DeviceType::AranetRadon => "Rn",
                                                    DeviceType::AranetRadiation => "Rad",
                                                    _ => "?",
                                                };
                                                components::status_badge(
                                                    ui,
                                                    &self.theme,
                                                    type_label,
                                                    self.theme.info,
                                                );
                                                ui.add_space(self.theme.spacing.xs);
                                            }

                                            // Show primary sensor reading based on device type
                                            if let Some(ref reading) = device.reading {
                                                if reading.co2 > 0 {
                                                    // Aranet4: Show CO2
                                                    let color = self.theme.co2_color(reading.co2);
                                                    ui.label(
                                                        RichText::new(format!("{} ppm", reading.co2))
                                                            .size(self.theme.typography.caption)
                                                            .color(color),
                                                    )
                                                    .on_hover_text("CO2 level");
                                                } else if let Some(radon) = reading.radon {
                                                    // AranetRadon: Show radon
                                                    let (value, unit) =
                                                        format_radon(radon, device.settings.as_ref());
                                                    let color = self.theme.radon_color(radon);
                                                    ui.label(
                                                        RichText::new(format!("{} {}", value, unit))
                                                            .size(self.theme.typography.caption)
                                                            .color(color),
                                                    )
                                                    .on_hover_text("Radon level");
                                                } else if let Some(rate) = reading.radiation_rate {
                                                    // AranetRadiation: Show radiation rate
                                                    let color = self.theme.radiation_color(rate);
                                                    ui.label(
                                                        RichText::new(format!("{:.2} uSv/h", rate))
                                                            .size(self.theme.typography.caption)
                                                            .color(color),
                                                    )
                                                    .on_hover_text("Radiation rate");
                                                } else {
                                                    // Aranet2 or unknown: Show temperature
                                                    let (temp_val, temp_unit) = format_temperature(
                                                        reading.temperature,
                                                        device.settings.as_ref(),
                                                    );
                                                    ui.label(
                                                        RichText::new(format!(
                                                            "{:.1}{}",
                                                            temp_val, temp_unit
                                                        ))
                                                        .size(self.theme.typography.caption)
                                                        .color(self.theme.text_secondary),
                                                    )
                                                    .on_hover_text("Temperature");
                                                }
                                            }

                                            if let Some(rssi) = device.rssi {
                                                let signal_color = if rssi > -60 {
                                                    self.theme.success
                                                } else if rssi > -75 {
                                                    self.theme.warning
                                                } else {
                                                    self.theme.danger
                                                };
                                                ui.with_layout(
                                                    egui::Layout::right_to_left(egui::Align::Center),
                                                    |ui| {
                                                        ui.label(
                                                            RichText::new(format!("{}dB", rssi))
                                                                .size(self.theme.typography.caption)
                                                                .color(signal_color),
                                                        )
                                                        .on_hover_text("Signal strength");
                                                    },
                                                );
                                            }
                                        });
                                    });
                                })
                                .response;

                            if response.interact(egui::Sense::click()).clicked() {
                                new_selection = Some(i);
                            }

                            ui.add_space(self.theme.spacing.xs);
                        }
                        self.selected_device = new_selection;
                    });
                }
            });
    }

    /// Render the device detail panel.
    fn render_device_panel(&self, ui: &mut egui::Ui, device: &DeviceState, idx: usize) {
        // Device header
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(device.display_name())
                        .size(self.theme.typography.heading)
                        .strong()
                        .color(self.theme.text_primary),
                );
                ui.horizontal(|ui| {
                    if let Some(device_type) = device.device_type {
                        components::status_badge(
                            ui,
                            &self.theme,
                            &format!("{:?}", device_type),
                            self.theme.info,
                        );
                    }
                    if let Some(rssi) = device.rssi {
                        let signal_color = if rssi > -60 {
                            self.theme.success
                        } else if rssi > -75 {
                            self.theme.warning
                        } else {
                            self.theme.danger
                        };
                        ui.add_space(self.theme.spacing.sm);
                        ui.label(
                            RichText::new(format!("{} dBm", rssi))
                                .size(self.theme.typography.caption)
                                .color(signal_color),
                        );
                    }
                });
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                match &device.connection {
                    ConnectionState::Disconnected | ConnectionState::Error(_) => {
                        let btn = egui::Button::new(
                            RichText::new("Connect")
                                .size(self.theme.typography.body)
                                .color(self.theme.text_on_accent),
                        )
                        .fill(self.theme.accent);
                        if ui.add(btn).clicked() {
                            self.send_command(Command::Connect {
                                device_id: device.id.clone(),
                            });
                        }
                    }
                    ConnectionState::Connecting => {
                        components::loading_indicator(ui, &self.theme, Some("Connecting..."));
                    }
                    ConnectionState::Connected => {
                        if ui
                            .add(egui::Button::new(
                                RichText::new("Refresh").size(self.theme.typography.body),
                            ))
                            .on_hover_text("Cmd+R")
                            .clicked()
                        {
                            self.send_command(Command::RefreshReading {
                                device_id: device.id.clone(),
                            });
                        }
                        ui.add_space(self.theme.spacing.sm);
                        if ui
                            .add(egui::Button::new(
                                RichText::new("Disconnect")
                                    .size(self.theme.typography.body)
                                    .color(self.theme.danger),
                            ))
                            .clicked()
                        {
                            self.send_command(Command::Disconnect {
                                device_id: device.id.clone(),
                            });
                        }
                    }
                }
            });
        });

        ui.add_space(self.theme.spacing.lg);
        ui.separator();
        ui.add_space(self.theme.spacing.lg);

        // Readings content
        if device.reading.is_some() {
            self.render_readings(ui, device);
        } else if device.connection == ConnectionState::Connected {
            components::loading_indicator(ui, &self.theme, Some("Waiting for readings..."));
        } else {
            components::empty_state(
                ui,
                &self.theme,
                "No Readings",
                "Connect to the device to view sensor readings",
            );
        }

        let _ = idx;
    }

    /// Render the history panel with charts.
    fn render_history_panel(&mut self, ui: &mut egui::Ui, device: &DeviceState) {
        // Header with title and sync button
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} - History", device.display_name()))
                    .size(self.theme.typography.heading)
                    .strong()
                    .color(self.theme.text_primary),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if device.syncing_history {
                    components::loading_indicator(ui, &self.theme, Some("Syncing..."));
                } else {
                    let btn = egui::Button::new(
                        RichText::new("Sync History")
                            .size(self.theme.typography.body)
                            .color(self.theme.text_on_accent),
                    )
                    .fill(self.theme.accent);
                    if ui.add(btn).on_hover_text("Download history from device").clicked() {
                        self.send_command(Command::SyncHistory {
                            device_id: device.id.clone(),
                        });
                    }
                }
            });
        });

        ui.add_space(self.theme.spacing.md);

        // Filter segmented control
        let filter_options = [
            (HistoryFilter::All, HistoryFilter::All.label()),
            (HistoryFilter::Last24Hours, HistoryFilter::Last24Hours.label()),
            (HistoryFilter::Last7Days, HistoryFilter::Last7Days.label()),
            (HistoryFilter::Last30Days, HistoryFilter::Last30Days.label()),
        ];

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Time Range:")
                    .size(self.theme.typography.body)
                    .color(self.theme.text_secondary),
            );
            ui.add_space(self.theme.spacing.sm);

            for (filter, label) in filter_options {
                let is_selected = self.history_filter == filter;
                let (bg, text_color) = if is_selected {
                    (self.theme.accent, self.theme.text_on_accent)
                } else {
                    (self.theme.bg_card, self.theme.text_secondary)
                };

                let btn = egui::Button::new(
                    RichText::new(label)
                        .size(self.theme.typography.caption)
                        .color(text_color),
                )
                .fill(bg)
                .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                if ui.add(btn).clicked() {
                    self.history_filter = filter;
                }
            }
        });

        ui.add_space(self.theme.spacing.lg);
        ui.separator();
        ui.add_space(self.theme.spacing.md);

        if device.history.is_empty() {
            components::empty_state(
                ui,
                &self.theme,
                "No History Data",
                "Click 'Sync History' to download data from your device",
            );
            return;
        }

        let now = time::OffsetDateTime::now_utc();
        let filtered: Vec<_> = device
            .history
            .iter()
            .filter(|r| match self.history_filter {
                HistoryFilter::All => true,
                HistoryFilter::Last24Hours => (now - r.timestamp) < time::Duration::hours(24),
                HistoryFilter::Last7Days => (now - r.timestamp) < time::Duration::days(7),
                HistoryFilter::Last30Days => (now - r.timestamp) < time::Duration::days(30),
            })
            .collect();

        // Record count badge
        ui.horizontal(|ui| {
            components::status_badge(
                ui,
                &self.theme,
                &format!("{} records", filtered.len()),
                self.theme.info,
            );
            if filtered.len() != device.history.len() {
                ui.add_space(self.theme.spacing.sm);
                ui.label(
                    RichText::new(format!("of {} total", device.history.len()))
                        .size(self.theme.typography.caption)
                        .color(self.theme.text_muted),
                );
            }
        });
        ui.add_space(self.theme.spacing.md);

        let has_co2 = filtered.iter().any(|r| r.co2 > 0);
        let has_radon = filtered.iter().any(|r| r.radon.is_some());
        let has_radiation = filtered.iter().any(|r| r.radiation_rate.is_some());

        let now_secs = time::OffsetDateTime::now_utc().unix_timestamp() as f64;
        let to_hours_ago = |ts: time::OffsetDateTime| -> f64 {
            let secs = ts.unix_timestamp() as f64;
            (now_secs - secs) / 3600.0
        };

        // Plot styling constants
        let plot_height = 160.0;

        egui::ScrollArea::vertical().show(ui, |ui| {
            if has_co2 {
                self.render_chart_section(ui, "CO2", "ppm", || {
                    let co2_points: PlotPoints = filtered
                        .iter()
                        .map(|r| [-to_hours_ago(r.timestamp), r.co2 as f64])
                        .collect();
                    (co2_points, self.theme.info)
                }, plot_height, Some(vec![
                    (800.0, "Good", self.theme.success),
                    (1000.0, "Moderate", self.theme.warning),
                    (1500.0, "Poor", self.theme.danger),
                ]));
            }

            if has_radon {
                // Use device settings for radon unit
                let use_pci = device
                    .settings
                    .as_ref()
                    .map(|s| s.radon_unit == RadonUnit::PciL)
                    .unwrap_or(false);
                let radon_unit_label = if use_pci { "pCi/L" } else { "Bq/m3" };
                // Threshold lines (convert if using pCi/L)
                let thresholds = if use_pci {
                    vec![
                        (100.0 * 0.027, "Action", self.theme.warning),
                        (300.0 * 0.027, "High", self.theme.danger),
                    ]
                } else {
                    vec![
                        (100.0, "Action", self.theme.warning),
                        (300.0, "High", self.theme.danger),
                    ]
                };
                self.render_chart_section(ui, "Radon", radon_unit_label, || {
                    let radon_points: PlotPoints = filtered
                        .iter()
                        .filter_map(|r| {
                            r.radon.map(|v| {
                                let value = if use_pci { bq_to_pci(v) as f64 } else { v as f64 };
                                [-to_hours_ago(r.timestamp), value]
                            })
                        })
                        .collect();
                    (radon_points, self.theme.warning)
                }, plot_height, Some(thresholds));
            }

            if has_radiation {
                self.render_chart_section(ui, "Radiation Rate", "uSv/h", || {
                    let radiation_points: PlotPoints = filtered
                        .iter()
                        .filter_map(|r| r.radiation_rate.map(|v| [-to_hours_ago(r.timestamp), v as f64]))
                        .collect();
                    (radiation_points, self.theme.danger)
                }, plot_height, None);
            }

            // Use device settings for temperature unit
            let use_fahrenheit = device
                .settings
                .as_ref()
                .map(|s| s.temperature_unit == TemperatureUnit::Fahrenheit)
                .unwrap_or(false);
            let temp_unit_label = if use_fahrenheit { "F" } else { "C" };
            self.render_chart_section(ui, "Temperature", temp_unit_label, || {
                let temp_points: PlotPoints = filtered
                    .iter()
                    .map(|r| {
                        let value = if use_fahrenheit {
                            celsius_to_fahrenheit(r.temperature) as f64
                        } else {
                            r.temperature as f64
                        };
                        [-to_hours_ago(r.timestamp), value]
                    })
                    .collect();
                (temp_points, self.theme.chart_temperature)
            }, plot_height, None);

            self.render_chart_section(ui, "Humidity", "%", || {
                let humidity_points: PlotPoints = filtered
                    .iter()
                    .map(|r| [-to_hours_ago(r.timestamp), r.humidity as f64])
                    .collect();
                (humidity_points, self.theme.chart_humidity)
            }, plot_height, None);
        });
    }

    /// Render a chart section with consistent styling.
    fn render_chart_section<F>(
        &self,
        ui: &mut egui::Ui,
        title: &str,
        unit: &str,
        data_fn: F,
        height: f32,
        thresholds: Option<Vec<(f64, &str, Color32)>>,
    ) where
        F: FnOnce() -> (PlotPoints<'static>, Color32),
    {
        egui::Frame::new()
            .fill(self.theme.bg_card)
            .inner_margin(egui::Margin::same(self.theme.spacing.md as i8))
            .corner_radius(egui::CornerRadius::same(self.theme.rounding.md as u8))
            .stroke(egui::Stroke::new(1.0, self.theme.border_subtle))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(title)
                            .size(self.theme.typography.subheading)
                            .strong()
                            .color(self.theme.text_primary),
                    );
                    ui.label(
                        RichText::new(format!("({})", unit))
                            .size(self.theme.typography.caption)
                            .color(self.theme.text_muted),
                    );
                });
                ui.add_space(self.theme.spacing.sm);

                let (points, line_color) = data_fn();

                Plot::new(format!("{}_plot", title.to_lowercase().replace(' ', "_")))
                    .height(height)
                    .show_axes(true)
                    .show_grid(true)
                    .allow_drag(true)
                    .allow_zoom(true)
                    .allow_boxed_zoom(true)
                    .allow_scroll(true)
                    .x_axis_label("Hours ago")
                    .show(ui, |plot_ui| {
                        if let Some(ref thresh) = thresholds {
                            for (value, label, color) in thresh {
                                plot_ui.hline(
                                    HLine::new(*label, *value)
                                        .color(*color)
                                        .style(egui_plot::LineStyle::dashed_dense()),
                                );
                            }
                        }
                        plot_ui.line(Line::new(title, points).color(line_color).width(2.0));
                    });
            });
        ui.add_space(self.theme.spacing.md);
    }

    /// Render the settings panel with editable controls.
    fn render_settings_panel(&mut self, ui: &mut egui::Ui, device: &DeviceState) {
        // Header
        ui.label(
            RichText::new(format!("{} - Settings", device.display_name()))
                .size(self.theme.typography.heading)
                .strong()
                .color(self.theme.text_primary),
        );
        ui.add_space(self.theme.spacing.lg);

        // Collect commands to send after UI rendering
        let mut commands_to_send: Vec<Command> = Vec::new();

        egui::ScrollArea::vertical().show(ui, |ui| {
            // Measurement Interval Section
            if device.reading.is_some() {
                components::section_header(ui, &self.theme, "Measurement Interval");

                egui::Frame::new()
                    .fill(self.theme.bg_card)
                    .inner_margin(egui::Margin::same(self.theme.spacing.lg as i8))
                    .corner_radius(egui::CornerRadius::same(self.theme.rounding.md as u8))
                    .stroke(egui::Stroke::new(1.0, self.theme.border_subtle))
                    .show(ui, |ui| {
                        let current_interval =
                            device.reading.as_ref().map(|r| r.interval).unwrap_or(0);

                        ui.horizontal(|ui| {
                            for &(secs, label) in INTERVAL_OPTIONS {
                                let is_selected = current_interval == secs;
                                let (bg, text_color) = if is_selected {
                                    (self.theme.accent, self.theme.text_on_accent)
                                } else {
                                    (self.theme.bg_secondary, self.theme.text_secondary)
                                };

                                ui.add_enabled_ui(!self.updating_settings, |ui| {
                                    let btn = egui::Button::new(
                                        RichText::new(label)
                                            .size(self.theme.typography.caption)
                                            .color(text_color),
                                    )
                                    .fill(bg)
                                    .corner_radius(egui::CornerRadius::same(
                                        self.theme.rounding.sm as u8,
                                    ));

                                    if ui.add(btn).clicked() && !is_selected {
                                        self.updating_settings = true;
                                        self.status = format!("Setting interval to {}...", label);
                                        commands_to_send.push(Command::SetInterval {
                                            device_id: device.id.clone(),
                                            interval_secs: secs,
                                        });
                                    }
                                });
                            }
                            if self.updating_settings {
                                ui.add_space(self.theme.spacing.sm);
                                components::loading_indicator(ui, &self.theme, None);
                            }
                        });

                        ui.add_space(self.theme.spacing.sm);
                        ui.label(
                            RichText::new("How often the sensor takes measurements")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_muted),
                        );
                    });

                ui.add_space(self.theme.spacing.lg);
            }

            // Device Configuration Section
            if let Some(settings) = &device.settings {
                components::section_header(ui, &self.theme, "Device Configuration");

                egui::Frame::new()
                    .fill(self.theme.bg_card)
                    .inner_margin(egui::Margin::same(self.theme.spacing.lg as i8))
                    .corner_radius(egui::CornerRadius::same(self.theme.rounding.md as u8))
                    .stroke(egui::Stroke::new(1.0, self.theme.border_subtle))
                    .show(ui, |ui| {
                        // Smart Home toggle
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new("Smart Home Integration")
                                        .size(self.theme.typography.body)
                                        .color(self.theme.text_primary),
                                );
                                ui.label(
                                    RichText::new("Enable broadcasting to smart home systems")
                                        .size(self.theme.typography.caption)
                                        .color(self.theme.text_muted),
                                );
                            });

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.add_enabled_ui(!self.updating_settings, |ui| {
                                        let current = settings.smart_home_enabled;
                                        for (val, text) in [(true, "On"), (false, "Off")] {
                                            let is_selected = current == val;
                                            let (bg, text_color) = if is_selected {
                                                (self.theme.accent, self.theme.text_on_accent)
                                            } else {
                                                (self.theme.bg_secondary, self.theme.text_secondary)
                                            };

                                            let btn = egui::Button::new(
                                                RichText::new(text)
                                                    .size(self.theme.typography.caption)
                                                    .color(text_color),
                                            )
                                            .fill(bg)
                                            .corner_radius(egui::CornerRadius::same(
                                                self.theme.rounding.sm as u8,
                                            ));

                                            if ui.add(btn).clicked() && !is_selected {
                                                self.updating_settings = true;
                                                self.status = if val {
                                                    "Enabling Smart Home...".to_string()
                                                } else {
                                                    "Disabling Smart Home...".to_string()
                                                };
                                                commands_to_send.push(Command::SetSmartHome {
                                                    device_id: device.id.clone(),
                                                    enabled: val,
                                                });
                                            }
                                        }
                                    });
                                },
                            );
                        });

                        ui.add_space(self.theme.spacing.md);

                        // Bluetooth Range toggle
                        let is_extended =
                            matches!(settings.bluetooth_range, BluetoothRange::Extended);
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new("Bluetooth Range")
                                        .size(self.theme.typography.body)
                                        .color(self.theme.text_primary),
                                );
                                ui.label(
                                    RichText::new("Extended range uses more battery")
                                        .size(self.theme.typography.caption)
                                        .color(self.theme.text_muted),
                                );
                            });

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.add_enabled_ui(!self.updating_settings, |ui| {
                                        for (is_ext, label) in
                                            [(false, "Standard"), (true, "Extended")]
                                        {
                                            let is_selected = is_extended == is_ext;
                                            let (bg, text_color) = if is_selected {
                                                (self.theme.accent, self.theme.text_on_accent)
                                            } else {
                                                (self.theme.bg_secondary, self.theme.text_secondary)
                                            };

                                            let btn = egui::Button::new(
                                                RichText::new(label)
                                                    .size(self.theme.typography.caption)
                                                    .color(text_color),
                                            )
                                            .fill(bg)
                                            .corner_radius(egui::CornerRadius::same(
                                                self.theme.rounding.sm as u8,
                                            ));

                                            if ui.add(btn).clicked() && !is_selected {
                                                self.updating_settings = true;
                                                self.status =
                                                    format!("Setting range to {}...", label);
                                                commands_to_send.push(Command::SetBluetoothRange {
                                                    device_id: device.id.clone(),
                                                    extended: is_ext,
                                                });
                                            }
                                        }
                                    });
                                },
                            );
                        });

                        ui.add_space(self.theme.spacing.lg);
                        ui.separator();
                        ui.add_space(self.theme.spacing.md);

                        // Read-only settings grid
                        egui::Grid::new("settings_grid")
                            .num_columns(2)
                            .spacing([self.theme.spacing.xl, self.theme.spacing.sm])
                            .show(ui, |ui| {
                                Self::render_settings_row_static(
                                    ui,
                                    &self.theme,
                                    "Temperature Unit",
                                    &format!("{:?}", settings.temperature_unit),
                                );
                                Self::render_settings_row_static(
                                    ui,
                                    &self.theme,
                                    "Radon Unit",
                                    &format!("{:?}", settings.radon_unit),
                                );
                                Self::render_settings_row_static(
                                    ui,
                                    &self.theme,
                                    "Buzzer",
                                    if settings.buzzer_enabled {
                                        "Enabled"
                                    } else {
                                        "Disabled"
                                    },
                                );
                                Self::render_settings_row_static(
                                    ui,
                                    &self.theme,
                                    "Auto Calibration",
                                    if settings.auto_calibration_enabled {
                                        "Enabled"
                                    } else {
                                        "Disabled"
                                    },
                                );
                            });
                    });

                ui.add_space(self.theme.spacing.lg);
            } else if device.reading.is_none() {
                components::empty_state(
                    ui,
                    &self.theme,
                    "No Settings Available",
                    "Connect to the device to load settings",
                );
            }

            // Device Info Section
            components::section_header(ui, &self.theme, "Device Information");

            egui::Frame::new()
                .fill(self.theme.bg_card)
                .inner_margin(egui::Margin::same(self.theme.spacing.lg as i8))
                .corner_radius(egui::CornerRadius::same(self.theme.rounding.md as u8))
                .stroke(egui::Stroke::new(1.0, self.theme.border_subtle))
                .show(ui, |ui| {
                    egui::Grid::new("device_info_grid")
                        .num_columns(2)
                        .spacing([self.theme.spacing.xl, self.theme.spacing.sm])
                        .show(ui, |ui| {
                            Self::render_settings_row_static(ui, &self.theme, "Device ID", &device.id);

                            if let Some(name) = &device.name {
                                Self::render_settings_row_static(ui, &self.theme, "Name", name);
                            }

                            if let Some(device_type) = device.device_type {
                                Self::render_settings_row_static(
                                    ui,
                                    &self.theme,
                                    "Type",
                                    &format!("{:?}", device_type),
                                );
                            }

                            if let Some(rssi) = device.rssi {
                                Self::render_settings_row_static(
                                    ui,
                                    &self.theme,
                                    "Signal Strength",
                                    &format!("{} dBm", rssi),
                                );
                            }

                            Self::render_settings_row_static(
                                ui,
                                &self.theme,
                                "History Records",
                                &format!("{}", device.history.len()),
                            );
                        });
                });
        });

        // Send any queued commands
        for cmd in commands_to_send {
            self.send_command(cmd);
        }
    }

    /// Helper to render a settings grid row (static version to avoid borrow issues).
    fn render_settings_row_static(ui: &mut egui::Ui, theme: &Theme, label: &str, value: &str) {
        ui.label(
            RichText::new(label)
                .size(theme.typography.body)
                .color(theme.text_secondary),
        );
        ui.label(
            RichText::new(value)
                .size(theme.typography.body)
                .color(theme.text_primary),
        );
        ui.end_row();
    }

    /// Render sensor readings with styled cards.
    fn render_readings(&self, ui: &mut egui::Ui, device: &DeviceState) {
        let reading = device.reading.as_ref().unwrap();

        components::section_header(ui, &self.theme, "Current Readings");

        // Show cached data banner if device is offline but we have cached readings
        if device.is_showing_cached_data() {
            let is_stale = components::is_reading_stale(reading.captured_at, reading.interval);
            components::cached_data_banner(ui, &self.theme, reading.captured_at, is_stale);
            ui.add_space(self.theme.spacing.md);
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            // CO2 with color-coded card (only for Aranet4)
            if reading.co2 > 0 {
                let level = Co2Level::from_ppm(reading.co2);
                let (status_text, color) = match level {
                    Co2Level::Good => ("Good", self.theme.success),
                    Co2Level::Moderate => ("Moderate", self.theme.warning),
                    Co2Level::Poor => ("Poor", self.theme.caution),
                    Co2Level::Bad => ("Bad", self.theme.danger),
                };
                let bg_color = self.theme.co2_bg_color(reading.co2);

                egui::Frame::new()
                    .fill(bg_color)
                    .inner_margin(egui::Margin::same(self.theme.spacing.lg as i8))
                    .corner_radius(egui::CornerRadius::same(self.theme.rounding.lg as u8))
                    .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.4)))
                    .shadow(self.theme.subtle_shadow())
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width().min(320.0));
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(
                                        RichText::new("CO2")
                                            .color(self.theme.text_muted)
                                            .size(self.theme.typography.caption),
                                    );
                                    ui.add_space(self.theme.spacing.xs);
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(format!("{}", reading.co2))
                                                .color(color)
                                                .size(self.theme.typography.metric)
                                                .strong(),
                                        );
                                        ui.add_space(self.theme.spacing.xs);
                                        ui.label(
                                            RichText::new("ppm")
                                                .color(self.theme.text_muted)
                                                .size(self.theme.typography.body),
                                        );
                                        if let Some(trend) = device.co2_trend() {
                                            let trend_color = match trend {
                                                Trend::Rising => self.theme.danger,
                                                Trend::Falling => self.theme.success,
                                                Trend::Stable => self.theme.text_muted,
                                            };
                                            ui.add_space(self.theme.spacing.sm);
                                            ui.label(
                                                RichText::new(trend.indicator())
                                                    .color(trend_color)
                                                    .size(self.theme.typography.heading),
                                            );
                                        }
                                    });
                                    ui.add_space(self.theme.spacing.xs);
                                    components::status_badge(ui, &self.theme, status_text, color);
                                });
                            });
                            ui.add_space(self.theme.spacing.md);
                            components::co2_gauge(ui, &self.theme, reading.co2);
                        });
                    });
                ui.add_space(self.theme.spacing.lg);
            }

            // Radon with color-coded card (only for AranetRadon)
            if let Some(radon) = reading.radon {
                let level = RadonLevel::from_bq(radon);
                let color = self.theme.radon_color(radon);
                let bg_color = self.theme.radon_bg_color(radon);
                let (radon_value, radon_unit) = format_radon(radon, device.settings.as_ref());

                egui::Frame::new()
                    .fill(bg_color)
                    .inner_margin(egui::Margin::same(self.theme.spacing.lg as i8))
                    .corner_radius(egui::CornerRadius::same(self.theme.rounding.lg as u8))
                    .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.4)))
                    .shadow(self.theme.subtle_shadow())
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width().min(320.0));
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(
                                        RichText::new("Radon")
                                            .color(self.theme.text_muted)
                                            .size(self.theme.typography.caption),
                                    );
                                    ui.add_space(self.theme.spacing.xs);
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(&radon_value)
                                                .color(color)
                                                .size(self.theme.typography.metric)
                                                .strong(),
                                        );
                                        ui.add_space(self.theme.spacing.xs);
                                        ui.label(
                                            RichText::new(radon_unit)
                                                .color(self.theme.text_muted)
                                                .size(self.theme.typography.body),
                                        );
                                    });
                                    ui.add_space(self.theme.spacing.xs);
                                    components::status_badge(
                                        ui,
                                        &self.theme,
                                        level.status_text(),
                                        color,
                                    );
                                });
                            });
                        });
                    });
                ui.add_space(self.theme.spacing.lg);
            }

            // Radiation with color-coded card (only for AranetRadiation)
            if let Some(rate) = reading.radiation_rate {
                let level = RadiationLevel::from_usv(rate);
                let color = self.theme.radiation_color(rate);
                let bg_color = self.theme.radiation_bg_color(rate);

                egui::Frame::new()
                    .fill(bg_color)
                    .inner_margin(egui::Margin::same(self.theme.spacing.lg as i8))
                    .corner_radius(egui::CornerRadius::same(self.theme.rounding.lg as u8))
                    .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.4)))
                    .shadow(self.theme.subtle_shadow())
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width().min(320.0));
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(
                                        RichText::new("Radiation")
                                            .color(self.theme.text_muted)
                                            .size(self.theme.typography.caption),
                                    );
                                    ui.add_space(self.theme.spacing.xs);
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(format!("{:.3}", rate))
                                                .color(color)
                                                .size(self.theme.typography.metric)
                                                .strong(),
                                        );
                                        ui.add_space(self.theme.spacing.xs);
                                        ui.label(
                                            RichText::new("uSv/h")
                                                .color(self.theme.text_muted)
                                                .size(self.theme.typography.body),
                                        );
                                    });
                                    ui.add_space(self.theme.spacing.xs);
                                    components::status_badge(
                                        ui,
                                        &self.theme,
                                        level.status_text(),
                                        color,
                                    );
                                });
                            });
                            // Show total dose if available
                            if let Some(total) = reading.radiation_total {
                                ui.add_space(self.theme.spacing.sm);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("Total Dose:")
                                            .color(self.theme.text_muted)
                                            .size(self.theme.typography.caption),
                                    );
                                    ui.label(
                                        RichText::new(format!("{:.2} uSv", total))
                                            .color(self.theme.text_secondary)
                                            .size(self.theme.typography.caption),
                                    );
                                });
                            }
                        });
                    });
                ui.add_space(self.theme.spacing.lg);
            }

            // Metrics grid
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing =
                    egui::vec2(self.theme.spacing.md, self.theme.spacing.md);

                // Temperature (use device settings for unit)
                let (temp_value, temp_unit) =
                    format_temperature(reading.temperature, device.settings.as_ref());
                components::metric_card(
                    ui,
                    &self.theme,
                    "Temperature",
                    &temp_value,
                    temp_unit,
                    device.temperature_trend(),
                    self.theme.info,
                );

                // Humidity
                components::metric_card(
                    ui,
                    &self.theme,
                    "Humidity",
                    &format!("{}", reading.humidity),
                    "%",
                    device.humidity_trend(),
                    self.theme.info,
                );

                // Pressure (if available)
                if reading.pressure > 0.0 {
                    components::metric_card(
                        ui,
                        &self.theme,
                        "Pressure",
                        &format!("{:.1}", reading.pressure),
                        "hPa",
                        None,
                        self.theme.text_secondary,
                    );
                }

                // Battery
                let battery_color = self.theme.battery_color(reading.battery);
                components::metric_card(
                    ui,
                    &self.theme,
                    "Battery",
                    &format!("{}", reading.battery),
                    "%",
                    None,
                    battery_color,
                );
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // celsius_to_fahrenheit tests
    // ========================================================================

    #[test]
    fn test_celsius_to_fahrenheit_freezing() {
        let result = celsius_to_fahrenheit(0.0);
        assert!((result - 32.0).abs() < 0.01);
    }

    #[test]
    fn test_celsius_to_fahrenheit_boiling() {
        let result = celsius_to_fahrenheit(100.0);
        assert!((result - 212.0).abs() < 0.01);
    }

    #[test]
    fn test_celsius_to_fahrenheit_room_temp() {
        // 20°C = 68°F
        let result = celsius_to_fahrenheit(20.0);
        assert!((result - 68.0).abs() < 0.01);
    }

    #[test]
    fn test_celsius_to_fahrenheit_negative() {
        // -40°C = -40°F (the point where scales meet)
        let result = celsius_to_fahrenheit(-40.0);
        assert!((result - (-40.0)).abs() < 0.01);
    }

    // ========================================================================
    // bq_to_pci tests
    // ========================================================================

    #[test]
    fn test_bq_to_pci_zero() {
        let result = bq_to_pci(0);
        assert!((result - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_bq_to_pci_100() {
        // 100 Bq/m³ = 2.7 pCi/L
        let result = bq_to_pci(100);
        assert!((result - 2.7).abs() < 0.01);
    }

    #[test]
    fn test_bq_to_pci_who_action_level() {
        // WHO action level is ~100-300 Bq/m³
        // 300 Bq/m³ = 8.1 pCi/L
        let result = bq_to_pci(300);
        assert!((result - 8.1).abs() < 0.01);
    }

    // ========================================================================
    // format_temperature tests
    // ========================================================================

    #[test]
    fn test_format_temperature_no_settings_defaults_celsius() {
        let (value, unit) = format_temperature(20.5, None);
        assert_eq!(value, "20.5");
        assert_eq!(unit, "C");
    }

    #[test]
    fn test_format_temperature_celsius_setting() {
        let settings = DeviceSettings {
            temperature_unit: TemperatureUnit::Celsius,
            ..Default::default()
        };
        let (value, unit) = format_temperature(20.5, Some(&settings));
        assert_eq!(value, "20.5");
        assert_eq!(unit, "C");
    }

    #[test]
    fn test_format_temperature_fahrenheit_setting() {
        let settings = DeviceSettings {
            temperature_unit: TemperatureUnit::Fahrenheit,
            ..Default::default()
        };
        let (value, unit) = format_temperature(20.0, Some(&settings));
        // 20°C = 68°F
        assert_eq!(value, "68.0");
        assert_eq!(unit, "F");
    }

    #[test]
    fn test_format_temperature_fahrenheit_decimal() {
        let settings = DeviceSettings {
            temperature_unit: TemperatureUnit::Fahrenheit,
            ..Default::default()
        };
        let (value, unit) = format_temperature(21.5, Some(&settings));
        // 21.5°C = 70.7°F
        assert_eq!(value, "70.7");
        assert_eq!(unit, "F");
    }

    // ========================================================================
    // format_radon tests
    // ========================================================================

    #[test]
    fn test_format_radon_no_settings_defaults_bq() {
        let (value, unit) = format_radon(150, None);
        assert_eq!(value, "150");
        assert_eq!(unit, "Bq/m3");
    }

    #[test]
    fn test_format_radon_bq_setting() {
        let settings = DeviceSettings {
            radon_unit: RadonUnit::BqM3,
            ..Default::default()
        };
        let (value, unit) = format_radon(150, Some(&settings));
        assert_eq!(value, "150");
        assert_eq!(unit, "Bq/m3");
    }

    #[test]
    fn test_format_radon_pci_setting() {
        let settings = DeviceSettings {
            radon_unit: RadonUnit::PciL,
            ..Default::default()
        };
        let (value, unit) = format_radon(100, Some(&settings));
        // 100 Bq/m³ = 2.70 pCi/L
        assert_eq!(value, "2.70");
        assert_eq!(unit, "pCi/L");
    }

    #[test]
    fn test_format_radon_pci_zero() {
        let settings = DeviceSettings {
            radon_unit: RadonUnit::PciL,
            ..Default::default()
        };
        let (value, unit) = format_radon(0, Some(&settings));
        assert_eq!(value, "0.00");
        assert_eq!(unit, "pCi/L");
    }

    #[test]
    fn test_format_radon_pci_high_value() {
        let settings = DeviceSettings {
            radon_unit: RadonUnit::PciL,
            ..Default::default()
        };
        let (value, unit) = format_radon(300, Some(&settings));
        // 300 Bq/m³ = 8.10 pCi/L
        assert_eq!(value, "8.10");
        assert_eq!(unit, "pCi/L");
    }
}
