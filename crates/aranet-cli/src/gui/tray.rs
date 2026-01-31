//! System tray integration for the Aranet GUI.
//!
//! This module provides system tray functionality including:
//! - Tray icon with current CO2 status color
//! - Context menu for quick actions
//! - Desktop notifications for threshold alerts
//! - Background monitoring support

use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

use super::types::Co2Level;

/// Global egui context for waking up the event loop from tray events.
/// This is needed because tray icon events may fire when the window is hidden
/// and the event loop is not actively polling.
static EGUI_CTX: Mutex<Option<egui::Context>> = Mutex::new(None);

/// Global queue for tray icon events.
/// When we use set_event_handler, events no longer go to the receiver,
/// so we must queue them ourselves.
static TRAY_EVENTS: Mutex<Vec<TrayIconEvent>> = Mutex::new(Vec::new());

/// Global queue for menu events.
static MENU_EVENTS: Mutex<Vec<MenuEvent>> = Mutex::new(Vec::new());

/// Set the global egui context for tray event handling.
/// This should be called once the egui context is available.
pub fn set_egui_context(ctx: egui::Context) {
    if let Ok(mut guard) = EGUI_CTX.lock() {
        *guard = Some(ctx);
    }
}

/// Error type for tray operations.
#[derive(Debug)]
pub enum TrayError {
    IconLoad(String),
    TrayIcon(tray_icon::Error),
    Menu(tray_icon::menu::Error),
}

impl std::fmt::Display for TrayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrayError::IconLoad(s) => write!(f, "Failed to load icon: {}", s),
            TrayError::TrayIcon(e) => write!(f, "Tray icon error: {}", e),
            TrayError::Menu(e) => write!(f, "Menu error: {}", e),
        }
    }
}

impl std::error::Error for TrayError {}

impl From<tray_icon::Error> for TrayError {
    fn from(e: tray_icon::Error) -> Self {
        TrayError::TrayIcon(e)
    }
}

impl From<tray_icon::menu::Error> for TrayError {
    fn from(e: tray_icon::menu::Error) -> Self {
        TrayError::Menu(e)
    }
}

/// Embedded icon PNG data (same as main app icon)
const ICON_PNG: &[u8] = include_bytes!("../../assets/aranet-icon.png");

/// Commands that can be sent from the tray to the main app.
#[derive(Debug, Clone)]
pub enum TrayCommand {
    /// Show the main window
    ShowWindow,
    /// Hide the main window (minimize to tray)
    HideWindow,
    /// Toggle window visibility
    ToggleWindow,
    /// Scan for devices
    Scan,
    /// Refresh all connected devices
    RefreshAll,
    /// Open settings view
    OpenSettings,
    /// Quit the application
    Quit,
}

/// Shared state between the tray and the main app.
#[derive(Debug, Default)]
pub struct TrayState {
    /// Current CO2 level for icon color
    pub co2_level: Option<Co2Level>,
    /// Current CO2 reading in ppm
    pub co2_ppm: Option<u16>,
    /// Whether the main window is visible
    pub window_visible: bool,
    /// Device name for tooltip
    pub device_name: Option<String>,
    /// Last alert CO2 level (to avoid duplicate notifications)
    pub last_alert_level: Option<Co2Level>,
    /// Whether to show colored tray icon for elevated CO2 (from settings).
    /// When false, always uses native template icon.
    pub colored_tray_icon: bool,
    /// Whether notifications are enabled (from settings).
    pub notifications_enabled: bool,
    /// Whether to play sound with notifications (from settings).
    pub notification_sound: bool,
    /// Do Not Disturb mode - temporarily suppresses all notifications.
    /// This is per-session and not persisted to config.
    pub do_not_disturb: bool,
}

impl TrayState {
    /// Format tooltip text based on current state.
    pub fn tooltip(&self) -> String {
        let mut parts = vec!["Aranet".to_string()];

        if let Some(name) = &self.device_name {
            parts.push(format!("Device: {}", name));
        }

        if let Some(co2) = self.co2_ppm {
            let level_text = match Co2Level::from_ppm(co2) {
                Co2Level::Good => "Good",
                Co2Level::Moderate => "Moderate",
                Co2Level::Poor => "Poor",
                Co2Level::Bad => "Bad",
            };
            parts.push(format!("CO2: {} ppm ({})", co2, level_text));
        }

        parts.join("\n")
    }
}

/// Manager for the system tray icon and menu.
pub struct TrayManager {
    tray_icon: TrayIcon,
    status_item: MenuItem,
    scan_item: MenuItem,
    refresh_item: MenuItem,
    settings_item: MenuItem,
    show_item: MenuItem,
    hide_item: MenuItem,
    quit_item: MenuItem,
    state: Arc<Mutex<TrayState>>,
}

impl TrayManager {
    /// Create a new tray manager with the given state.
    pub fn new(state: Arc<Mutex<TrayState>>) -> Result<Self, TrayError> {
        // Get initial state including colored icon preference
        let (window_visible, colored_tray_icon) = state
            .lock()
            .map(|s| (s.window_visible, s.colored_tray_icon))
            .unwrap_or((true, true));

        // Load initial icon (always template at startup since no CO2 reading yet)
        let (icon, is_template) = load_tray_icon_for_level(None, colored_tray_icon)?;

        // Create menu items - status item is disabled (display only)
        // Show/Hide items are enabled based on current window visibility
        let status_item = MenuItem::new("Aranet - No reading", false, None);
        let scan_item = MenuItem::new("Scan for Devices", true, None);
        let refresh_item = MenuItem::new("Refresh All", true, None);
        let settings_item = MenuItem::new("Settings...", true, None);
        let show_item = MenuItem::new("Show Aranet", !window_visible, None);
        let hide_item = MenuItem::new("Hide to Tray", window_visible, None);
        let quit_item = MenuItem::new("Quit", true, None);

        // Build the menu with status at top, then quick actions, then window controls
        let menu = Menu::new();
        menu.append_items(&[
            &status_item,
            &PredefinedMenuItem::separator(),
            &scan_item,
            &refresh_item,
            &settings_item,
            &PredefinedMenuItem::separator(),
            &show_item,
            &hide_item,
            &PredefinedMenuItem::separator(),
            &quit_item,
        ])?;

        // Build the tray icon with template support for native macOS appearance
        let tooltip = state.lock().map(|s| s.tooltip()).unwrap_or_default();
        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(&tooltip)
            .with_icon(icon)
            .with_icon_as_template(is_template)
            .with_menu_on_left_click(false)
            .build()?;

        // Set up event handlers that:
        // 1. Queue events (since set_event_handler bypasses the default receiver)
        // 2. Wake up the event loop (critical for macOS when window is hidden)
        TrayIconEvent::set_event_handler(Some(move |event| {
            debug!("TrayIconEvent received: {:?}", event);
            // Queue the event for processing
            if let Ok(mut guard) = TRAY_EVENTS.lock() {
                guard.push(event);
            }
            // Wake up the egui event loop so it can process the tray event
            if let Ok(guard) = EGUI_CTX.lock()
                && let Some(ctx) = guard.as_ref()
            {
                ctx.request_repaint();
            }
        }));

        MenuEvent::set_event_handler(Some(move |event| {
            debug!("MenuEvent received: {:?}", event);
            // Queue the event for processing
            if let Ok(mut guard) = MENU_EVENTS.lock() {
                guard.push(event);
            }
            // Wake up the egui event loop so it can process the menu event
            if let Ok(guard) = EGUI_CTX.lock()
                && let Some(ctx) = guard.as_ref()
            {
                ctx.request_repaint();
            }
        }));

        info!("System tray icon created");

        Ok(Self {
            tray_icon,
            status_item,
            scan_item,
            refresh_item,
            settings_item,
            show_item,
            hide_item,
            quit_item,
            state,
        })
    }

    /// Process pending tray events and return any commands.
    pub fn process_events(&self) -> Vec<TrayCommand> {
        let mut commands = Vec::new();

        // Drain all pending menu events from our custom queue
        let menu_events: Vec<MenuEvent> = if let Ok(mut guard) = MENU_EVENTS.lock() {
            std::mem::take(&mut *guard)
        } else {
            Vec::new()
        };

        for event in menu_events {
            if event.id == self.scan_item.id() {
                debug!("Tray: Scan clicked");
                commands.push(TrayCommand::ShowWindow); // Show window first
                commands.push(TrayCommand::Scan);
            } else if event.id == self.refresh_item.id() {
                debug!("Tray: Refresh All clicked");
                commands.push(TrayCommand::RefreshAll);
            } else if event.id == self.settings_item.id() {
                debug!("Tray: Settings clicked");
                commands.push(TrayCommand::ShowWindow); // Show window first
                commands.push(TrayCommand::OpenSettings);
            } else if event.id == self.show_item.id() {
                debug!("Tray: Show window clicked");
                commands.push(TrayCommand::ShowWindow);
            } else if event.id == self.hide_item.id() {
                debug!("Tray: Hide window clicked");
                commands.push(TrayCommand::HideWindow);
            } else if event.id == self.quit_item.id() {
                debug!("Tray: Quit clicked");
                commands.push(TrayCommand::Quit);
            }
        }

        // Drain all pending tray icon click events from our custom queue
        let tray_events: Vec<TrayIconEvent> = if let Ok(mut guard) = TRAY_EVENTS.lock() {
            std::mem::take(&mut *guard)
        } else {
            Vec::new()
        };

        for event in tray_events {
            match event {
                TrayIconEvent::Click {
                    button,
                    button_state,
                    ..
                } => {
                    // Only respond to button Up (click completed), not Down
                    // Otherwise we get two toggles per click
                    if button == tray_icon::MouseButton::Left
                        && button_state == tray_icon::MouseButtonState::Up
                    {
                        debug!("Tray: Left click - toggle window");
                        commands.push(TrayCommand::ToggleWindow);
                    }
                }
                TrayIconEvent::DoubleClick { button, .. } => {
                    if button == tray_icon::MouseButton::Left {
                        debug!("Tray: Double click - show window");
                        commands.push(TrayCommand::ShowWindow);
                    }
                }
                _ => {}
            }
        }

        commands
    }

    /// Update the tray tooltip, status menu item, and icon based on current state.
    pub fn update_tooltip(&self) {
        if let Ok(state) = self.state.lock() {
            // Update tooltip
            let tooltip = state.tooltip();
            if let Err(e) = self.tray_icon.set_tooltip(Some(&tooltip)) {
                warn!("Failed to update tray tooltip: {}", e);
            }

            // Update status menu item and icon color
            let (status_text, level) = if let Some(co2) = state.co2_ppm {
                let level = Co2Level::from_ppm(co2);
                let level_text = match level {
                    Co2Level::Good => "Good",
                    Co2Level::Moderate => "Moderate",
                    Co2Level::Poor => "Poor",
                    Co2Level::Bad => "Bad",
                };
                (format!("CO2: {} ppm ({})", co2, level_text), Some(level))
            } else {
                ("Aranet - No reading".to_string(), None)
            };
            self.status_item.set_text(&status_text);

            // Update icon based on CO2 level and user preference
            self.update_icon_color(level.as_ref(), state.colored_tray_icon);

            // Update menu item enabled states based on window visibility
            // When window is visible: "Show Aranet" should be disabled, "Hide to Tray" enabled
            // When window is hidden: "Show Aranet" should be enabled, "Hide to Tray" disabled
            self.show_item.set_enabled(!state.window_visible);
            self.hide_item.set_enabled(state.window_visible);
        }
    }

    /// Update the tray icon based on CO2 level.
    ///
    /// If `use_colored` is true, shows colored icons for elevated CO2 levels.
    /// If false, always uses native template icon (auto dark/light).
    fn update_icon_color(&self, level: Option<&Co2Level>, use_colored: bool) {
        match load_tray_icon_for_level(level, use_colored) {
            Ok((icon, is_template)) => {
                // Use set_icon_with_as_template to atomically set both the icon
                // and template flag, avoiding race conditions on macOS
                if let Err(e) = self
                    .tray_icon
                    .set_icon_with_as_template(Some(icon), is_template)
                {
                    warn!("Failed to update tray icon: {}", e);
                }
            }
            Err(e) => {
                warn!("Failed to generate icon: {}", e);
            }
        }
    }
}

/// Load the tray icon for a given CO2 level.
///
/// Returns the icon and whether it should be treated as a template image.
///
/// If `use_colored` is true:
/// - Good/None levels: native template icon (auto dark/light)
/// - Elevated levels: colored icon (yellow/orange/red) as visual alert
///
/// If `use_colored` is false:
/// - Always returns native template icon regardless of CO2 level
fn load_tray_icon_for_level(
    level: Option<&Co2Level>,
    use_colored: bool,
) -> Result<(Icon, bool), TrayError> {
    let mut img = image::load_from_memory(ICON_PNG)
        .map_err(|e| TrayError::IconLoad(e.to_string()))?
        .into_rgba8();

    // Determine if we should use a template (native dark/light) or colored icon
    let use_template = if use_colored {
        // When colored icons are enabled, use template only for good/none levels
        match level {
            None | Some(Co2Level::Good) => true,
            Some(Co2Level::Moderate | Co2Level::Poor | Co2Level::Bad) => false,
        }
    } else {
        // When colored icons are disabled, always use template
        true
    };

    if use_template {
        // Convert to template icon: white pixels with preserved alpha
        // macOS will use only the alpha channel and render in appropriate color
        for pixel in img.pixels_mut() {
            if pixel[3] > 0 {
                // Set RGB to white, preserve alpha
                pixel[0] = 255;
                pixel[1] = 255;
                pixel[2] = 255;
            }
        }
    } else {
        // Apply color tint for elevated CO2 levels
        let (r, g, b) = match level {
            Some(Co2Level::Moderate) => (255, 193, 7), // Yellow/Amber
            Some(Co2Level::Poor) => (255, 152, 0),     // Orange
            Some(Co2Level::Bad) => (244, 67, 54),      // Red
            _ => unreachable!(),
        };

        // Apply the status color to opaque pixels
        for pixel in img.pixels_mut() {
            if pixel[3] > 128 {
                pixel[0] = r;
                pixel[1] = g;
                pixel[2] = b;
            }
        }
    }

    let (width, height) = img.dimensions();
    let icon = Icon::from_rgba(img.into_raw(), width, height)
        .map_err(|e| TrayError::IconLoad(e.to_string()))?;

    Ok((icon, use_template))
}

/// Send a desktop notification for a threshold alert.
///
/// Parameters:
/// - `title`: Notification title
/// - `body`: Notification body text
/// - `is_critical`: Whether this is a critical alert (affects urgency on Linux)
/// - `play_sound`: Whether to play a sound with the notification
#[allow(unused_variables)]
pub fn send_notification(title: &str, body: &str, is_critical: bool, play_sound: bool) {
    use notify_rust::Notification;

    let mut notification = Notification::new();
    notification.summary(title).body(body).appname("Aranet");

    // Add sound on macOS if enabled
    #[cfg(target_os = "macos")]
    {
        if play_sound {
            notification.sound_name("default");
        }
    }

    // On Linux, we can set urgency (not available on macOS)
    #[cfg(target_os = "linux")]
    {
        if is_critical {
            notification.urgency(notify_rust::Urgency::Critical);
        }
    }

    match notification.show() {
        Ok(_) => debug!("Notification sent: {} - {}", title, body),
        Err(e) => warn!("Failed to send notification: {}", e),
    }
}

/// Check CO2 level and send notification if threshold exceeded.
///
/// Notifications are only sent if `state.notifications_enabled` is true
/// and `state.do_not_disturb` is false.
pub fn check_co2_threshold(state: &mut TrayState, co2_ppm: u16, device_name: &str) {
    let level = Co2Level::from_ppm(co2_ppm);

    // Only check for notifications if enabled in settings and DND is off
    if state.notifications_enabled && !state.do_not_disturb {
        let should_notify = match (&state.last_alert_level, &level) {
            // Notify when transitioning to a worse level
            (None, Co2Level::Poor | Co2Level::Bad) => true,
            (Some(Co2Level::Good), Co2Level::Poor | Co2Level::Bad) => true,
            (Some(Co2Level::Moderate), Co2Level::Poor | Co2Level::Bad) => true,
            (Some(Co2Level::Poor), Co2Level::Bad) => true,
            // Also notify when recovering to good
            (Some(Co2Level::Bad | Co2Level::Poor), Co2Level::Good) => true,
            _ => false,
        };

        if should_notify {
            let (title, body, is_critical) = match level {
                Co2Level::Good => (
                    "CO2 Level Normal",
                    format!("{}: {} ppm - Air quality is good", device_name, co2_ppm),
                    false,
                ),
                Co2Level::Moderate => (
                    "CO2 Level Moderate",
                    format!("{}: {} ppm - Consider ventilating", device_name, co2_ppm),
                    false,
                ),
                Co2Level::Poor => (
                    "CO2 Level Poor",
                    format!("{}: {} ppm - Ventilation recommended", device_name, co2_ppm),
                    true,
                ),
                Co2Level::Bad => (
                    "CO2 Level Critical",
                    format!("{}: {} ppm - Ventilate immediately!", device_name, co2_ppm),
                    true,
                ),
            };
            send_notification(title, &body, is_critical, state.notification_sound);
            state.last_alert_level = Some(level);
        }
    }

    state.co2_level = Some(level);
    state.co2_ppm = Some(co2_ppm);
}

/// Hide the application's dock icon (macOS only).
///
/// Sets the activation policy to "Accessory" which hides the app from the Dock.
/// The app will still appear in the menu bar via the system tray icon.
#[cfg(target_os = "macos")]
pub fn hide_dock_icon() {
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::MainThreadMarker;

    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        debug!("Dock icon hidden");
    } else {
        warn!("Cannot hide dock icon: not on main thread");
    }
}

/// Show the application's dock icon and activate the app (macOS only).
///
/// Sets the activation policy to "Regular" which shows the app in the Dock.
/// Also sets the application icon from the embedded PNG and activates the app
/// to bring it to the foreground.
#[cfg(target_os = "macos")]
pub fn show_dock_icon() {
    use objc2::ClassType;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSImage};
    use objc2_foundation::{MainThreadMarker, NSData};

    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

        // Set the application icon from embedded PNG data
        let icon_data = NSData::with_bytes(ICON_PNG);
        if let Some(icon) = NSImage::initWithData(NSImage::alloc(), &icon_data) {
            // SAFETY: We have a MainThreadMarker proving we're on the main thread,
            // and the icon is a valid NSImage created from our embedded PNG data.
            unsafe {
                app.setApplicationIconImage(Some(&icon));
            }
            debug!("Dock icon shown with custom icon");
        } else {
            debug!("Dock icon shown (failed to load custom icon)");
        }

        // Activate the app to bring it to the foreground
        // This is necessary when showing from menu bar/tray on macOS
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        debug!("App activated");
    } else {
        warn!("Cannot show dock icon: not on main thread");
    }
}

/// Hide the dock icon (no-op on non-macOS platforms).
#[cfg(not(target_os = "macos"))]
pub fn hide_dock_icon() {
    // No-op on other platforms
}

/// Show the dock icon (no-op on non-macOS platforms).
#[cfg(not(target_os = "macos"))]
pub fn show_dock_icon() {
    // No-op on other platforms
}
