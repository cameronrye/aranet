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
const ICON_PNG: &[u8] = include_bytes!("../../../../assets/aranet-icon.png");

/// Commands that can be sent from the tray to the main app.
#[derive(Debug, Clone)]
pub enum TrayCommand {
    /// Show the main window
    ShowWindow,
    /// Hide the main window (minimize to tray)
    HideWindow,
    /// Toggle window visibility
    ToggleWindow,
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
    show_item: MenuItem,
    hide_item: MenuItem,
    quit_item: MenuItem,
    state: Arc<Mutex<TrayState>>,
}

impl TrayManager {
    /// Create a new tray manager with the given state.
    pub fn new(state: Arc<Mutex<TrayState>>) -> Result<Self, TrayError> {
        let icon = load_tray_icon()?;

        // Create menu items - status item is disabled (display only)
        let status_item = MenuItem::new("Aranet - No reading", false, None);
        let show_item = MenuItem::new("Show Aranet", true, None);
        let hide_item = MenuItem::new("Hide to Tray", true, None);
        let quit_item = MenuItem::new("Quit", true, None);

        // Build the menu with status at top
        let menu = Menu::new();
        menu.append_items(&[
            &status_item,
            &PredefinedMenuItem::separator(),
            &show_item,
            &hide_item,
            &PredefinedMenuItem::separator(),
            &quit_item,
        ])?;

        // Build the tray icon
        let tooltip = state.lock().map(|s| s.tooltip()).unwrap_or_default();
        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(&tooltip)
            .with_icon(icon)
            .with_menu_on_left_click(false)
            .build()?;

        info!("System tray icon created");

        Ok(Self {
            tray_icon,
            status_item,
            show_item,
            hide_item,
            quit_item,
            state,
        })
    }

    /// Process pending tray events and return any commands.
    pub fn process_events(&self) -> Vec<TrayCommand> {
        let mut commands = Vec::new();

        // Drain all pending menu events
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.show_item.id() {
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

        // Drain all pending tray icon click events
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            match event {
                TrayIconEvent::Click { button, .. } => {
                    if button == tray_icon::MouseButton::Left {
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

            // Update icon color based on CO2 level
            self.update_icon_color(level.as_ref());
        }
    }

    /// Update the tray icon color based on CO2 level.
    fn update_icon_color(&self, level: Option<&Co2Level>) {
        match load_tray_icon_with_color(level) {
            Ok(icon) => {
                if let Err(e) = self.tray_icon.set_icon(Some(icon)) {
                    warn!("Failed to update tray icon: {}", e);
                }
            }
            Err(e) => {
                warn!("Failed to generate colored icon: {}", e);
            }
        }
    }
}

/// Load the tray icon from embedded PNG data.
fn load_tray_icon() -> Result<Icon, TrayError> {
    load_tray_icon_with_color(None)
}

/// Load the tray icon with an optional color overlay based on CO2 level.
fn load_tray_icon_with_color(level: Option<&Co2Level>) -> Result<Icon, TrayError> {
    let mut img = image::load_from_memory(ICON_PNG)
        .map_err(|e| TrayError::IconLoad(e.to_string()))?
        .into_rgba8();

    // Apply a color tint based on CO2 level
    if let Some(level) = level {
        let (r, g, b) = match level {
            Co2Level::Good => (76, 175, 80),     // Green
            Co2Level::Moderate => (255, 193, 7), // Yellow/Amber
            Co2Level::Poor => (255, 152, 0),     // Orange
            Co2Level::Bad => (244, 67, 54),      // Red
        };

        // Apply a simple color overlay to opaque pixels
        for pixel in img.pixels_mut() {
            if pixel[3] > 128 {
                // Blend with the status color (50% blend)
                pixel[0] = ((pixel[0] as u16 + r as u16) / 2) as u8;
                pixel[1] = ((pixel[1] as u16 + g as u16) / 2) as u8;
                pixel[2] = ((pixel[2] as u16 + b as u16) / 2) as u8;
            }
        }
    }

    let (width, height) = img.dimensions();
    Icon::from_rgba(img.into_raw(), width, height).map_err(|e| TrayError::IconLoad(e.to_string()))
}

/// Send a desktop notification for a threshold alert.
#[allow(unused_variables)]
pub fn send_notification(title: &str, body: &str, is_critical: bool) {
    use notify_rust::Notification;

    let mut notification = Notification::new();
    notification.summary(title).body(body).appname("Aranet");

    // Add sound on macOS
    #[cfg(target_os = "macos")]
    {
        notification.sound_name("default");
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
pub fn check_co2_threshold(state: &mut TrayState, co2_ppm: u16, device_name: &str) {
    let level = Co2Level::from_ppm(co2_ppm);
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
        send_notification(title, &body, is_critical);
        state.last_alert_level = Some(level);
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

/// Show the application's dock icon (macOS only).
///
/// Sets the activation policy to "Regular" which shows the app in the Dock.
/// Also sets the application icon from the embedded PNG.
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
