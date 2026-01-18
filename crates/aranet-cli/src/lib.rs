//! Command-line interface for Aranet environmental sensors.
//!
//! This crate provides a comprehensive CLI for interacting with Aranet devices
//! including the Aranet4, Aranet2, AranetRn+ (Radon), and Aranet Radiation sensors.
//!
//! # Features
//!
//! - **Device scanning**: Discover nearby Aranet devices via BLE
//! - **Current readings**: Display real-time sensor values with color-coded status
//! - **Historical data**: Download and export measurement history
//! - **Device configuration**: Adjust measurement interval, Bluetooth range, and Smart Home mode
//! - **Continuous monitoring**: Watch mode for ongoing data collection
//! - **Multiple output formats**: Text, JSON, and CSV output support
//! - **Configuration file**: Persistent settings for default device and preferences
//! - **Shell completions**: Generate completions for bash, zsh, fish, and PowerShell
//!
//! # Commands
//!
//! | Command | Description |
//! |---------|-------------|
//! | `scan` | Scan for nearby Aranet devices |
//! | `read` | Read current sensor values |
//! | `status` | Quick one-line status display |
//! | `history` | Download historical data |
//! | `info` | Display device information |
//! | `set` | Configure device settings |
//! | `watch` | Continuously monitor a device |
//! | `config` | Manage CLI configuration |
//! | `completions` | Generate shell completions |
//!
//! # Output Formats
//!
//! The CLI supports three output formats:
//!
//! - **Text** (default): Human-readable colored output
//! - **JSON**: Machine-readable JSON format
//! - **CSV**: Comma-separated values for spreadsheets and data analysis
//!
//! # Configuration
//!
//! The CLI stores configuration in `~/.config/aranet/config.toml` (or platform equivalent).
//! Configuration options include:
//!
//! - `device`: Default device address
//! - `format`: Default output format
//! - `no_color`: Disable colored output
//! - `fahrenheit`: Use Fahrenheit for temperature display
//!
//! # Environment Variables
//!
//! - `ARANET_DEVICE`: Default device address (overridden by `--device` flag)
//! - `NO_COLOR`: Disable colored output when set
//!
//! # Examples
//!
//! Scan for devices:
//! ```bash
//! aranet scan
//! ```
//!
//! Read current values from a specific device:
//! ```bash
//! aranet read --device AA:BB:CC:DD:EE:FF
//! ```
//!
//! Download history as CSV:
//! ```bash
//! aranet history --device AA:BB:CC:DD:EE:FF --format csv --output data.csv
//! ```
//!
//! Watch a device continuously:
//! ```bash
//! aranet watch --device AA:BB:CC:DD:EE:FF --interval 60
//! ```
//!
//! Set measurement interval:
//! ```bash
//! aranet set --device AA:BB:CC:DD:EE:FF interval 5
//! ```

// This crate is primarily a binary CLI application.
// The main entry point and command implementations are in main.rs.
// This lib.rs serves as documentation and could be extended to expose
// public APIs for programmatic use if needed in the future.

// Re-export core dependencies for convenience
pub use aranet_core;
pub use aranet_types;

// TUI module - publicly exposed for aranet-tui crate to use
#[cfg(feature = "tui")]
pub mod tui;

