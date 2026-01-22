//! Message types for TUI communication between UI and worker threads.
//!
//! This module re-exports the shared message types from `aranet-core::messages`.
//! These types are used for bidirectional communication in both TUI and GUI applications:
//!
//! - [`Command`]: Messages sent from the UI thread to the background worker
//! - [`SensorEvent`]: Events sent from the worker back to the UI thread

pub use aranet_core::messages::{CachedDevice, Command, SensorEvent};
