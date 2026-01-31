//! Native desktop GUI for Aranet environmental sensors.
//!
//! This module provides a cross-platform GUI application built with [egui](https://www.egui.rs/).
//!
//! # Usage
//!
//! Run directly:
//! ```bash
//! aranet gui
//! ```
//!
//! Or via the standalone binary:
//! ```bash
//! aranet-gui
//! ```

mod app;
mod components;
pub mod demo;
mod export;
mod helpers;
mod menu;
mod panels;
mod readings;
mod theme;
mod tray;
mod types;
mod worker;

use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use aranet_store::default_db_path;
use eframe::egui::{self, IconData};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use aranet_core::messages::{Command, SensorEvent};

use crate::config::Config;

/// Embedded icon PNG data (64x64 RGBA)
const ICON_PNG: &[u8] = include_bytes!("../../assets/aranet-icon.png");

/// Load the application icon from embedded PNG data.
fn load_icon() -> Option<Arc<IconData>> {
    let img = image::load_from_memory(ICON_PNG).ok()?.into_rgba8();
    let (width, height) = img.dimensions();
    Some(Arc::new(IconData {
        rgba: img.into_raw(),
        width,
        height,
    }))
}

pub use app::AranetApp;
pub use menu::{MenuCommand, MenuManager};
pub use theme::{Theme, ThemeMode};
pub use tray::{
    TrayCommand, TrayError, TrayManager, TrayState, check_co2_threshold, hide_dock_icon,
    set_egui_context, show_dock_icon,
};
pub use types::{
    AlertEntry, AlertSeverity, AlertType, Co2Level, ConnectionFilter, ConnectionState, DeviceState,
    DeviceTypeFilter, HistoryFilter, RadonLevel, Tab, Trend,
};
pub use worker::SensorWorker;

/// Options for running the GUI application.
#[derive(Debug, Default, Clone)]
pub struct GuiOptions {
    /// Run in demo mode with mock data (for screenshots).
    pub demo: bool,
    /// Take a screenshot and save to this path, then exit.
    pub screenshot: Option<PathBuf>,
    /// Number of frames to wait before taking screenshot (default: 3).
    pub screenshot_delay_frames: u32,
}

impl GuiOptions {
    /// Create new options with demo mode enabled.
    pub fn demo() -> Self {
        Self {
            demo: true,
            ..Default::default()
        }
    }

    /// Set screenshot output path.
    pub fn with_screenshot(mut self, path: impl Into<PathBuf>) -> Self {
        self.screenshot = Some(path.into());
        self
    }
}

/// Run the GUI application.
///
/// This is the main entry point for the GUI. It:
/// 1. Sets up the tokio runtime in a background thread
/// 2. Creates communication channels between UI and worker
/// 3. Spawns the background sensor worker
/// 4. Runs the egui/eframe main loop
pub fn run() -> Result<()> {
    tracing_subscriber::fmt::init();

    // Load config to get service URL
    let config = Config::load();
    let service_url = config.gui.service_url.clone();

    // Get store path (shared database location)
    let store_path = default_db_path();
    info!("Using database at: {:?}", store_path);

    // Create tokio runtime in a separate thread
    let (command_tx, command_rx) = mpsc::channel::<Command>(32);
    let (event_tx, event_rx_tokio) = mpsc::channel::<SensorEvent>(32);

    // Bridge from tokio mpsc to std mpsc for sync access in egui
    let (std_tx, std_rx) = std_mpsc::channel::<SensorEvent>();

    // Clone command_tx for sending initial load command
    let startup_command_tx = command_tx.clone();

    // Spawn tokio runtime thread
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        rt.block_on(async {
            let worker =
                SensorWorker::with_service_url(command_rx, event_tx, store_path, &service_url);

            // Send startup commands: load cached data and fetch service status
            let _ = startup_command_tx.send(Command::LoadCachedData).await;
            let _ = startup_command_tx.send(Command::RefreshServiceStatus).await;

            // Forward events from worker to std channel
            let mut event_rx = event_rx_tokio;
            let forward_handle = tokio::spawn(async move {
                while let Some(event) = event_rx.recv().await {
                    if std_tx.send(event).is_err() {
                        break; // GUI closed
                    }
                }
            });

            // Run the worker
            worker.run().await;
            forward_handle.abort();
        });
    });

    // Reuse config loaded earlier for GUI settings
    let gui_config = &config.gui;
    let default_width = 800.0;
    let default_height = 600.0;
    let window_width = gui_config.window_width.unwrap_or(default_width);
    let window_height = gui_config.window_height.unwrap_or(default_height);

    // Create system tray icon (must be on main thread before event loop)
    // We need to create a temporary tray state first
    let tray_state_temp = Arc::new(Mutex::new(TrayState {
        window_visible: true, // Will be updated below
        ..Default::default()
    }));

    let tray_manager = match TrayManager::new(tray_state_temp.clone()) {
        Ok(manager) => Some(manager),
        Err(e) => {
            warn!(
                "Failed to create system tray: {}. Continuing without tray.",
                e
            );
            None
        }
    };

    // Check if we should start minimized (requires tray to be available)
    let start_minimized = gui_config.start_minimized && tray_manager.is_some();
    if start_minimized {
        info!("Starting minimized to system tray");
        // Update tray state to reflect hidden window
        if let Ok(mut state) = tray_state_temp.lock() {
            state.window_visible = false;
        }
        // Hide the dock icon on macOS
        hide_dock_icon();
    }

    // Use the properly initialized tray state
    let tray_state = tray_state_temp;

    // Build viewport with icon, saved size, and close-to-tray behavior
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([window_width, window_height])
        .with_min_inner_size([600.0, 400.0])
        .with_close_button(true)
        .with_visible(!start_minimized); // Start hidden if start_minimized is enabled

    // Restore window position if saved
    if let (Some(x), Some(y)) = (gui_config.window_x, gui_config.window_y) {
        // Validate the position is reasonable
        if x >= -500.0 && y >= -500.0 && x < 5000.0 && y < 5000.0 {
            debug!("Restoring window position: ({}, {})", x, y);
            viewport = viewport.with_position([x, y]);
        }
    }

    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(icon);
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Aranet",
        native_options,
        Box::new(move |cc| {
            // Set the egui context for tray event handling.
            // This allows tray events to wake up the event loop when the window is hidden.
            set_egui_context(cc.egui_ctx.clone());

            // Create app first without menu
            let mut app = AranetApp::new(cc, command_tx, std_rx, tray_state, tray_manager, None);

            // Create native menu bar AFTER eframe has initialized NSApp (required for macOS)
            let menu_manager = match MenuManager::new() {
                Ok(manager) => {
                    // Initialize for macOS - now safe because NSApp is ready
                    manager.init_for_macos();
                    Some(manager)
                }
                Err(e) => {
                    warn!(
                        "Failed to create native menu: {}. Continuing without menu.",
                        e
                    );
                    None
                }
            };

            // Set the menu manager on the app
            app.set_menu_manager(menu_manager);

            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("Failed to run eframe: {}", e))?;

    Ok(())
}

/// Run the GUI application with custom options.
///
/// This allows running in demo mode with mock data for screenshots.
pub fn run_with_options(options: GuiOptions) -> Result<()> {
    tracing_subscriber::fmt::init();

    if options.demo {
        info!("Running in demo mode with mock data");
    }

    // Load config to get service URL and GUI settings
    let config = Config::load();
    let service_url = config.gui.service_url.clone();

    // Get store path (shared database location) - not used in demo mode
    let store_path = default_db_path();
    if !options.demo {
        info!("Using database at: {:?}", store_path);
    }

    // Create tokio runtime in a separate thread
    let (command_tx, command_rx) = mpsc::channel::<Command>(32);
    let (event_tx, event_rx_tokio) = mpsc::channel::<SensorEvent>(32);

    // Bridge from tokio mpsc to std mpsc for sync access in egui
    let (std_tx, std_rx) = std_mpsc::channel::<SensorEvent>();

    // Clone command_tx for sending initial load command
    let startup_command_tx = command_tx.clone();
    let is_demo = options.demo;

    // Spawn tokio runtime thread
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        rt.block_on(async {
            let worker =
                SensorWorker::with_service_url(command_rx, event_tx, store_path, &service_url);

            // Send startup commands (skip in demo mode)
            if !is_demo {
                let _ = startup_command_tx.send(Command::LoadCachedData).await;
                let _ = startup_command_tx.send(Command::RefreshServiceStatus).await;
            }

            // Forward events from worker to std channel
            let mut event_rx = event_rx_tokio;
            let forward_handle = tokio::spawn(async move {
                while let Some(event) = event_rx.recv().await {
                    if std_tx.send(event).is_err() {
                        break; // GUI closed
                    }
                }
            });

            // Run the worker
            worker.run().await;
            forward_handle.abort();
        });
    });
    let gui_config = &config.gui;
    let default_width = 800.0;
    let default_height = 600.0;
    // Use 900x600 for screenshots to match VHS tape dimensions
    let screenshot_width = 900.0;
    let screenshot_height = 600.0;
    let (window_width, window_height) = if options.demo {
        (screenshot_width, screenshot_height)
    } else {
        (
            gui_config.window_width.unwrap_or(default_width),
            gui_config.window_height.unwrap_or(default_height),
        )
    };

    // Create shared tray state
    let tray_state_temp = Arc::new(Mutex::new(TrayState {
        window_visible: true, // Will be updated below if start_minimized
        ..Default::default()
    }));

    // Create system tray icon (must be on main thread before event loop)
    // Skip tray in demo mode for cleaner screenshots
    let tray_manager = if options.demo {
        None
    } else {
        match TrayManager::new(tray_state_temp.clone()) {
            Ok(manager) => Some(manager),
            Err(e) => {
                warn!(
                    "Failed to create system tray: {}. Continuing without tray.",
                    e
                );
                None
            }
        }
    };

    // Check if we should start minimized (requires tray and not in demo mode)
    let start_minimized = !options.demo && gui_config.start_minimized && tray_manager.is_some();
    if start_minimized {
        info!("Starting minimized to system tray");
        // Update tray state to reflect hidden window
        if let Ok(mut state) = tray_state_temp.lock() {
            state.window_visible = false;
        }
        // Hide the dock icon on macOS
        hide_dock_icon();
    }

    // Use the properly initialized tray state
    let tray_state = tray_state_temp;

    // Build viewport with icon, saved size, and close-to-tray behavior
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([window_width, window_height])
        .with_min_inner_size([600.0, 400.0])
        .with_close_button(true)
        .with_visible(!start_minimized); // Start hidden if start_minimized is enabled

    // Restore window position if saved (skip in demo mode)
    if !options.demo
        && let (Some(x), Some(y)) = (gui_config.window_x, gui_config.window_y)
        && x >= -500.0
        && y >= -500.0
        && x < 5000.0
        && y < 5000.0
    {
        debug!("Restoring window position: ({}, {})", x, y);
        viewport = viewport.with_position([x, y]);
    }

    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(icon);
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let screenshot_path = options.screenshot.clone();
    let screenshot_delay = options.screenshot_delay_frames;
    let demo_mode = options.demo;

    eframe::run_native(
        "Aranet",
        native_options,
        Box::new(move |cc| {
            // Set the egui context for tray event handling.
            // This allows tray events to wake up the event loop when the window is hidden.
            set_egui_context(cc.egui_ctx.clone());

            // Create app first without menu
            let mut app = AranetApp::new_with_options(
                cc,
                command_tx,
                std_rx,
                tray_state,
                tray_manager,
                None,
                demo_mode,
                screenshot_path,
                screenshot_delay,
            );

            // Create native menu bar AFTER eframe has initialized NSApp (required for macOS)
            // Skip menu in demo mode for cleaner screenshots
            let menu_manager = if demo_mode {
                None
            } else {
                match MenuManager::new() {
                    Ok(manager) => {
                        // Initialize for macOS - now safe because NSApp is ready
                        manager.init_for_macos();
                        Some(manager)
                    }
                    Err(e) => {
                        warn!(
                            "Failed to create native menu: {}. Continuing without menu.",
                            e
                        );
                        None
                    }
                }
            };

            // Set the menu manager on the app
            app.set_menu_manager(menu_manager);

            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("Failed to run eframe: {}", e))?;

    Ok(())
}
