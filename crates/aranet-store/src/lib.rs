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
/// - Linux: `~/.local/share/aranet/data.db`
/// - macOS: `~/Library/Application Support/aranet/data.db`
/// - Windows: `C:\Users\<user>\AppData\Local\aranet\data.db`
pub fn default_db_path() -> std::path::PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("aranet")
        .join("data.db")
}
