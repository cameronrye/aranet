//! Standalone GUI binary for Aranet sensors.
//!
//! This is a thin wrapper around aranet-cli's GUI functionality,
//! providing a separate binary for users who only want the desktop app.

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

/// Aranet GUI - Desktop application for Aranet environmental sensors
#[derive(Parser, Debug)]
#[command(name = "aranet-gui", version, about)]
struct Args {
    /// Run in demo mode with mock data (for screenshots and testing)
    #[arg(long)]
    demo: bool,

    /// Take a screenshot and save to this path, then exit
    #[arg(long, value_name = "PATH")]
    screenshot: Option<PathBuf>,

    /// Number of frames to wait before taking screenshot (default: 10)
    #[arg(long, default_value = "10")]
    screenshot_delay: u32,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.demo || args.screenshot.is_some() {
        let mut options = aranet_cli::gui::GuiOptions {
            demo: args.demo,
            screenshot: args.screenshot,
            screenshot_delay_frames: args.screenshot_delay,
        };
        // If taking a screenshot without explicit demo flag, enable demo mode
        if options.screenshot.is_some() && !options.demo {
            options.demo = true;
        }
        aranet_cli::gui::run_with_options(options)
    } else {
        aranet_cli::gui::run()
    }
}
