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
mod toml_file;

pub use error::{Error, Result};
pub use models::{StoredDevice, StoredHistoryRecord, StoredReading, SyncState};
pub use queries::{HistoryQuery, MAX_QUERY_LIMIT, ReadingQuery};
pub use store::{HistoryAggregates, HistoryStats, ImportResult, PRUNE_BATCH_ROWS, Store};
pub use toml_file::{TomlFile, TomlFileError, write_private};

/// Default directory for aranet's data files: the database and the CLI state file.
///
/// Checks `ARANET_DATA_DIR` first, then falls back to the platform data directory:
/// - Linux: `~/.local/share/aranet`
/// - macOS: `~/Library/Application Support/aranet`
/// - Windows: `C:\Users\<user>\AppData\Local\aranet`
///
/// An `ARANET_DATA_DIR` that is set to an empty string counts as unset: as a path it would
/// be the current directory.
///
/// Without a platform data directory, it logs a warning and uses the current directory.
pub fn default_data_dir() -> std::path::PathBuf {
    std::env::var_os("ARANET_DATA_DIR")
        .filter(|dir| !dir.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::data_local_dir().map(|d| d.join("aranet")))
        .unwrap_or_else(|| {
            tracing::warn!(
                "Could not determine platform data directory; \
                 falling back to current directory for data files"
            );
            std::path::PathBuf::from(".")
        })
}

/// Default database path: `data.db` in [`default_data_dir`].
///
/// - Linux: `~/.local/share/aranet/data.db`
/// - macOS: `~/Library/Application Support/aranet/data.db`
/// - Windows: `C:\Users\<user>\AppData\Local\aranet\data.db`
pub fn default_db_path() -> std::path::PathBuf {
    default_data_dir().join("data.db")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_db_path_is_in_default_data_dir() {
        assert_eq!(default_db_path(), default_data_dir().join("data.db"));
    }
}
