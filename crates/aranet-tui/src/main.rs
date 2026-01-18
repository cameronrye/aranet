//! Standalone TUI binary for Aranet sensors.
//!
//! This is a thin wrapper around aranet-cli's TUI functionality,
//! providing a separate binary for users who only want the dashboard.

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    aranet_cli::tui::run().await
}
