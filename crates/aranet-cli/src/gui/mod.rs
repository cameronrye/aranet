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
use tracing::{info, warn};

use aranet_core::messages::{Command, SensorEvent};

/// Embedded icon PNG data (64x64 RGBA)
const ICON_PNG: &[u8] = include_bytes!("../../../../assets/aranet-icon.png");

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
pub use theme::{Theme, ThemeMode};
pub use tray::{
    check_co2_threshold, hide_dock_icon, show_dock_icon, TrayCommand, TrayError, TrayManager,
    TrayState,
};
pub use types::{Co2Level, ConnectionState, DeviceState, HistoryFilter, Tab, Trend};
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
            let worker = SensorWorker::new(command_rx, event_tx, store_path);

            // Send LoadCachedData command to load devices from store on startup
            let _ = startup_command_tx.send(Command::LoadCachedData).await;

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

    // Create shared tray state
    let tray_state = Arc::new(Mutex::new(TrayState {
        window_visible: true,
        ..Default::default()
    }));

    // Create system tray icon (must be on main thread before event loop)
    let tray_manager = match TrayManager::new(tray_state.clone()) {
        Ok(manager) => Some(manager),
        Err(e) => {
            warn!("Failed to create system tray: {}. Continuing without tray.", e);
            None
        }
    };

    // Build viewport with icon and close-to-tray behavior
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([800.0, 600.0])
        .with_min_inner_size([600.0, 400.0])
        .with_close_button(true);

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
            Ok(Box::new(AranetApp::new(
                cc,
                command_tx,
                std_rx,
                tray_state,
                tray_manager,
            )))
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
            let worker = SensorWorker::new(command_rx, event_tx, store_path);

            // Send LoadCachedData command to load devices from store on startup
            // (In demo mode, the app will ignore this and use mock data)
            if !is_demo {
                let _ = startup_command_tx.send(Command::LoadCachedData).await;
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

    // Create shared tray state
    let tray_state = Arc::new(Mutex::new(TrayState {
        window_visible: true,
        ..Default::default()
    }));

    // Create system tray icon (must be on main thread before event loop)
    // Skip tray in demo mode for cleaner screenshots
    let tray_manager = if options.demo {
        None
    } else {
        match TrayManager::new(tray_state.clone()) {
            Ok(manager) => Some(manager),
            Err(e) => {
                warn!("Failed to create system tray: {}. Continuing without tray.", e);
                None
            }
        }
    };

    // Build viewport with icon and close-to-tray behavior
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([800.0, 600.0])
        .with_min_inner_size([600.0, 400.0])
        .with_close_button(true);

    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(icon);
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let screenshot_path = options.screenshot.clone();
    let screenshot_delay = options.screenshot_delay_frames;

    eframe::run_native(
        "Aranet",
        native_options,
        Box::new(move |cc| {
            Ok(Box::new(AranetApp::new_with_options(
                cc,
                command_tx,
                std_rx,
                tray_state,
                tray_manager,
                options.demo,
                screenshot_path,
                screenshot_delay,
            )))
        }),
    )
    .map_err(|e| anyhow::anyhow!("Failed to run eframe: {}", e))?;

    Ok(())
}
