//! TUI for Aranet sensors.
//!
//! This crate provides a standalone binary wrapper around aranet-cli's TUI functionality.
//! The actual TUI implementation lives in `aranet-cli` with the `tui` feature enabled.
//!
//! For the TUI implementation, see [`aranet_cli::tui`].

pub use aranet_cli::tui;
