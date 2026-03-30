#![deny(unsafe_code)]

//! Local data persistence for Aranet sensor readings.
//!
//! This crate provides SQLite-based storage for Aranet sensor data,
//! enabling offline access, history caching, and efficient queries.
//!
//! # Features
//!
//! - Store current readings with timestamps
//! - Cache history records (avoid re-downloading from device)
//! - Incremental sync tracking per device
//! - Query by device, time range, with pagination
//! - Export/import support
//!
//! # Example
//!
//! ```no_run
//! use aranet_store::{Store, ReadingQuery};
//!
//! let store = Store::open_default()?;
//!
//! // Query recent readings
//! let query = ReadingQuery::new()
//!     .device("Aranet4 17C3C")
//!     .limit(10);
//! let readings = store.query_readings(&query)?;
//! # Ok::<(), aranet_store::Error>(())
//! ```

mod error;
mod models;
mod queries;
mod schema;
mod store;

pub use error::{Error, Result};
pub use models::{StoredDevice, StoredHistoryRecord, StoredReading, SyncState};
pub use queries::{HistoryQuery, ReadingQuery};
pub use store::{HistoryAggregates, HistoryStats, ImportResult, Store};

/// Default database path following platform conventions.
///
/// Checks `ARANET_DATA_DIR` first, then falls back to the platform data directory:
/// - Linux: `~/.local/share/aranet/data.db`
/// - macOS: `~/Library/Application Support/aranet/data.db`
/// - Windows: `C:\Users\<user>\AppData\Local\aranet\data.db`
pub fn default_db_path() -> std::path::PathBuf {
    std::env::var_os("ARANET_DATA_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::data_local_dir().map(|d| d.join("aranet")))
        .unwrap_or_else(|| {
            tracing::warn!(
                "Could not determine platform data directory; \
                 falling back to current directory for database"
            );
            std::path::PathBuf::from(".")
        })
        .join("data.db")
}
