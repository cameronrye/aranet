//! Native desktop GUI for Aranet environmental sensors

use anyhow::Result;
use eframe::egui;

/// Main application state
struct AranetApp {
    // Placeholder for future state
}

impl AranetApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {}
    }
}

impl eframe::App for AranetApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(100.0);
                ui.heading("Aranet GUI");
                ui.add_space(20.0);
                ui.label("Coming Soon");
                ui.add_space(40.0);
                ui.label("Native desktop interface for Aranet environmental sensors");
            });
        });
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let native_options = eframe::NativeOptions::default();

    eframe::run_native(
        "Aranet GUI",
        native_options,
        Box::new(|cc| Ok(Box::new(AranetApp::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("Failed to run eframe: {}", e))?;

    Ok(())
}
