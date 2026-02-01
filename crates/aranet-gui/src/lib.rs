//! # Aranet GUI
//!
//! A native desktop GUI application for monitoring Aranet environmental sensors.
//!
//! ## Installation
//!
//! This crate provides a standalone binary. Install it with:
//!
//! ```sh
//! cargo install aranet-gui
//! ```
//!
//! Or via Homebrew on macOS:
//!
//! ```sh
//! brew install cameronrye/aranet/aranet
//! ```
//!
//! ## Usage
//!
//! Simply run the `aranet-gui` binary:
//!
//! ```sh
//! aranet-gui
//! ```
//!
//! ### Options
//!
//! - `--demo` - Run in demo mode with mock sensor data
//! - `--screenshot <PATH>` - Take a screenshot and save to the specified path
//! - `--screenshot-delay <N>` - Number of frames to wait before taking screenshot (default: 10)
//!
//! ## Features
//!
//! - Real-time sensor readings with color-coded CO2 levels
//! - Support for Aranet4, Aranet2, AranetRn+, and Aranet Radiation sensors
//! - Historical data visualization
//! - Device settings configuration
//! - Bluetooth Low Energy connectivity
//!
//! ## Library Usage
//!
//! This crate is primarily a binary application. For programmatic access to Aranet
//! sensors, see the [`aranet-core`](https://docs.rs/aranet-core) crate.

// This crate is a binary application; the library target exists for documentation purposes.
