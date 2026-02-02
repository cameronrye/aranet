//! Main store implementation.
//!
//! # SQLite Concurrency Model
//!
//! This store uses SQLite with WAL (Write-Ahead Logging) mode enabled for improved
//! concurrent read performance. Key concurrency characteristics:
//!
//! - **Multiple readers**: WAL mode allows multiple simultaneous read transactions
//! - **Single writer**: Only one write transaction can be active at a time
//! - **Non-blocking reads**: Read operations don't block write operations and vice versa
//!
//! ## Thread Safety
//!
//! The `Store` struct is **not thread-safe** by itself. When using `Store` in a
//! multi-threaded context (e.g., `aranet-service`), wrap it in a `Mutex` or similar:
//!
//! ```ignore
//! use tokio::sync::Mutex;
//! let store = Mutex::new(Store::open_default()?);
//!
//! // Access the store
//! let guard = store.lock().await;
//! let devices = guard.list_devices()?;
//! ```
//!
//! ## Performance Considerations
//!
//! - For high-concurrency scenarios, consider keeping lock hold times short
//! - Batch operations (like `insert_history`) are more efficient than individual inserts
//! - Query operations with indexes (`device_id`, `timestamp`) are optimized
//!
//! ## Database Location
//!
//! The default database path is platform-specific:
//! - **Linux**: `~/.local/share/aranet/data.db`
//! - **macOS**: `~/Library/Application Support/aranet/data.db`
//! - **Windows**: `C:\Users\<user>\AppData\Local\aranet\data.db`

use std::path::Path;

use rusqlite::{Connection, OptionalExtension};
use time::OffsetDateTime;
use tracing::{debug, info, warn};

use aranet_types::{CurrentReading, DeviceInfo, DeviceType, HistoryRecord, Status};

/// Safely convert a Unix timestamp to OffsetDateTime.
/// Returns UNIX_EPOCH if the timestamp is invalid (corrupted database data).
fn timestamp_from_unix(ts: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(ts).unwrap_or_else(|_| {
        warn!("Invalid timestamp {} in database, using UNIX_EPOCH", ts);
        OffsetDateTime::UNIX_EPOCH
    })
}

use crate::error::{Error, Result};
use crate::models::{StoredDevice, StoredHistoryRecord, StoredReading, SyncState};
use crate::queries::{HistoryQuery, ReadingQuery};
use crate::schema;

/// SQLite-based store for Aranet sensor data.
///
/// `Store` provides persistent storage for sensor readings, history records,
/// and device metadata using SQLite. It supports:
///
/// - **Device management**: Track multiple Aranet devices with metadata
/// - **Current readings**: Store real-time sensor data with timestamps
/// - **History records**: Cache device history to avoid re-downloading
/// - **Incremental sync**: Track sync state for efficient history updates
/// - **Export/Import**: CSV and JSON formats for data portability
///
/// # Thread Safety
///
/// `Store` is **not thread-safe**. For concurrent access (e.g., in `aranet-service`),
/// wrap it in a `Mutex`:
///
/// ```ignore
/// use std::sync::Arc;
/// use tokio::sync::Mutex;
/// use aranet_store::Store;
///
/// let store = Arc::new(Mutex::new(Store::open_default()?));
///
/// // In async context:
/// let guard = store.lock().await;
/// let devices = guard.list_devices()?;
/// ```
///
/// # Example
///
/// ```no_run
/// use aranet_store::{Store, ReadingQuery, HistoryQuery};
/// use aranet_types::CurrentReading;
///
/// // Open the default database
/// let store = Store::open_default()?;
///
/// // Store a reading
/// let reading = CurrentReading::default();
/// store.insert_reading("Aranet4 17C3C", &reading)?;
///
/// // Query readings
/// let query = ReadingQuery::new().device("Aranet4 17C3C").limit(10);
/// let readings = store.query_readings(&query)?;
///
/// // Export history to CSV
/// let csv = store.export_history_csv(&HistoryQuery::new())?;
/// # Ok::<(), aranet_store::Error>(())
/// ```
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open or create a database at the given path.
    ///
    /// Creates parent directories if they don't exist. The database is
    /// initialized with WAL mode for better concurrent read performance.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the SQLite database file
    ///
    /// # Example
    ///
    /// ```no_run
    /// use aranet_store::Store;
    ///
    /// let store = Store::open("/path/to/my/aranet.db")?;
    /// # Ok::<(), aranet_store::Error>(())
    /// ```
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        // Create parent directories if needed
        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent).map_err(|e| Error::CreateDirectory {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }

        info!("Opening database at {}", path.display());
        let conn = Connection::open(path)?;

        // Enable foreign keys and WAL mode for better performance
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;",
        )?;

        // Initialize schema
        schema::initialize(&conn)?;

        Ok(Self { conn })
    }

    /// Open the database at the platform-specific default location.
    ///
    /// Default paths by platform:
    /// - **Linux**: `~/.local/share/aranet/data.db`
    /// - **macOS**: `~/Library/Application Support/aranet/data.db`
    /// - **Windows**: `C:\Users\<user>\AppData\Local\aranet\data.db`
    ///
    /// # Example
    ///
    /// ```no_run
    /// use aranet_store::Store;
    ///
    /// let store = Store::open_default()?;
    /// # Ok::<(), aranet_store::Error>(())
    /// ```
    pub fn open_default() -> Result<Self> {
        Self::open(crate::default_db_path())
    }

    /// Open an in-memory database.
    ///
    /// Useful for testing or temporary storage. Data is lost when the
    /// `Store` is dropped.
    ///
    /// # Example
    ///
    /// ```
    /// use aranet_store::Store;
    ///
    /// let store = Store::open_in_memory()?;
    /// // Use for testing...
    /// # Ok::<(), aranet_store::Error>(())
    /// ```
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        schema::initialize(&conn)?;
        Ok(Self { conn })
    }

    // === Device operations ===

    /// Get or create a device entry, updating timestamps.
    ///
    /// If the device exists, updates its `last_seen` timestamp and optionally
    /// the name. If it doesn't exist, creates a new entry with the current time
    /// as both `first_seen` and `last_seen`.
    ///
    /// # Arguments
    ///
    /// * `device_id` - Unique identifier for the device (typically BLE address)
    /// * `name` - Optional human-readable name for the device
    ///
    /// # Returns
    ///
    /// The device record after insert/update.
    ///
    /// # Example
    ///
    /// ```
    /// use aranet_store::Store;
    ///
    /// let store = Store::open_in_memory()?;
    /// let device = store.upsert_device("Aranet4 17C3C", Some("Kitchen"))?;
    /// assert_eq!(device.name, Some("Kitchen".to_string()));
    /// # Ok::<(), aranet_store::Error>(())
    /// ```
    pub fn upsert_device(&self, device_id: &str, name: Option<&str>) -> Result<StoredDevice> {
        let now = OffsetDateTime::now_utc().unix_timestamp();

        self.conn.execute(
            "INSERT INTO devices (id, name, first_seen, last_seen) VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(id) DO UPDATE SET 
                name = COALESCE(?2, name),
                last_seen = ?3",
            rusqlite::params![device_id, name, now],
        )?;

        self.get_device(device_id)?
            .ok_or_else(|| Error::DeviceNotFound(device_id.to_string()))
    }

    /// Update device metadata (name and type).
    ///
    /// This is a simpler version of `update_device_info` for when you only have
    /// basic device information (e.g., from BLE advertisement or connection).
    pub fn update_device_metadata(
        &self,
        device_id: &str,
        name: Option<&str>,
        device_type: Option<DeviceType>,
    ) -> Result<()> {
        let device_type_str = device_type.map(|dt| format!("{:?}", dt));
        let now = OffsetDateTime::now_utc().unix_timestamp();

        self.conn.execute(
            "UPDATE devices SET
                name = COALESCE(?2, name),
                device_type = COALESCE(?3, device_type),
                last_seen = ?4
             WHERE id = ?1",
            rusqlite::params![device_id, name, device_type_str, now],
        )?;

        Ok(())
    }

    /// Update device info from DeviceInfo.
    ///
    /// Device type is automatically inferred from the model name using
    /// `DeviceType::from_name()`, which handles all known Aranet device naming patterns.
    pub fn update_device_info(&self, device_id: &str, info: &DeviceInfo) -> Result<()> {
        // Use the shared DeviceType::from_name() for consistent device type detection
        let device_type = DeviceType::from_name(&info.model).map(|dt| format!("{:?}", dt));

        let name = if info.name.is_empty() {
            None
        } else {
            Some(&info.name)
        };

        self.conn.execute(
            "UPDATE devices SET
                name = COALESCE(?2, name),
                device_type = COALESCE(?3, device_type),
                serial = COALESCE(?4, serial),
                firmware = COALESCE(?5, firmware),
                hardware = COALESCE(?6, hardware),
                last_seen = ?7
             WHERE id = ?1",
            rusqlite::params![
                device_id,
                name,
                device_type,
                &info.serial,
                &info.firmware,
                &info.hardware,
                OffsetDateTime::now_utc().unix_timestamp()
            ],
        )?;

        Ok(())
    }

    /// Get a device by its unique identifier.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The device identifier to look up
    ///
    /// # Returns
    ///
    /// `Some(StoredDevice)` if found, `None` if the device doesn't exist.
    ///
    /// # Example
    ///
    /// ```
    /// use aranet_store::Store;
    ///
    /// let store = Store::open_in_memory()?;
    /// store.upsert_device("Aranet4 17C3C", Some("Kitchen"))?;
    ///
    /// if let Some(device) = store.get_device("Aranet4 17C3C")? {
    ///     println!("Found device: {:?}", device.name);
    /// }
    /// # Ok::<(), aranet_store::Error>(())
    /// ```
    pub fn get_device(&self, device_id: &str) -> Result<Option<StoredDevice>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, device_type, serial, firmware, hardware, first_seen, last_seen 
             FROM devices WHERE id = ?",
        )?;

        let device = stmt
            .query_row([device_id], |row| {
                Ok(StoredDevice {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    device_type: row
                        .get::<_, Option<String>>(2)?
                        .and_then(|s| parse_device_type(&s)),
                    serial: row.get(3)?,
                    firmware: row.get(4)?,
                    hardware: row.get(5)?,
                    first_seen: timestamp_from_unix(row.get(6)?),
                    last_seen: timestamp_from_unix(row.get(7)?),
                })
            })
            .optional()?;

        Ok(device)
    }

    /// List all known devices, ordered by most recently seen first.
    ///
    /// # Returns
    ///
    /// A vector of all stored devices, sorted by `last_seen` descending.
    ///
    /// # Example
    ///
    /// ```
    /// use aranet_store::Store;
    ///
    /// let store = Store::open_in_memory()?;
    /// store.upsert_device("device-1", Some("Kitchen"))?;
    /// store.upsert_device("device-2", Some("Bedroom"))?;
    ///
    /// let devices = store.list_devices()?;
    /// for device in devices {
    ///     println!("{}: {:?}", device.id, device.name);
    /// }
    /// # Ok::<(), aranet_store::Error>(())
    /// ```
    pub fn list_devices(&self) -> Result<Vec<StoredDevice>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, device_type, serial, firmware, hardware, first_seen, last_seen 
             FROM devices ORDER BY last_seen DESC",
        )?;

        let devices = stmt
            .query_map([], |row| {
                Ok(StoredDevice {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    device_type: row
                        .get::<_, Option<String>>(2)?
                        .and_then(|s| parse_device_type(&s)),
                    serial: row.get(3)?,
                    firmware: row.get(4)?,
                    hardware: row.get(5)?,
                    first_seen: timestamp_from_unix(row.get(6)?),
                    last_seen: timestamp_from_unix(row.get(7)?),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(devices)
    }

    /// Delete a device and all associated data (readings, history, sync state).
    ///
    /// Returns true if the device was deleted, false if it didn't exist.
    pub fn delete_device(&self, device_id: &str) -> Result<bool> {
        // Delete in order: history, readings, sync_state, device
        // Foreign keys would handle this, but explicit is clearer
        self.conn.execute(
            "DELETE FROM history WHERE device_id = ?1",
            rusqlite::params![device_id],
        )?;

        self.conn.execute(
            "DELETE FROM readings WHERE device_id = ?1",
            rusqlite::params![device_id],
        )?;

        self.conn.execute(
            "DELETE FROM sync_state WHERE device_id = ?1",
            rusqlite::params![device_id],
        )?;

        let rows_deleted = self.conn.execute(
            "DELETE FROM devices WHERE id = ?1",
            rusqlite::params![device_id],
        )?;

        Ok(rows_deleted > 0)
    }
}

fn parse_device_type(s: &str) -> Option<DeviceType> {
    match s {
        "Aranet4" => Some(DeviceType::Aranet4),
        "Aranet2" => Some(DeviceType::Aranet2),
        "AranetRadon" => Some(DeviceType::AranetRadon),
        "AranetRadiation" => Some(DeviceType::AranetRadiation),
        _ => None,
    }
}

fn parse_status(s: &str) -> Status {
    match s {
        "Green" => Status::Green,
        "Yellow" => Status::Yellow,
        "Red" => Status::Red,
        "Error" => Status::Error,
        _ => Status::Green,
    }
}

// Reading operations
impl Store {
    /// Insert a current reading from a device.
    ///
    /// Automatically creates the device entry if it doesn't exist. The reading
    /// is stored with its `captured_at` timestamp, or the current time if not set.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The device that produced this reading
    /// * `reading` - The sensor reading to store
    ///
    /// # Returns
    ///
    /// The database row ID of the inserted reading.
    ///
    /// # Example
    ///
    /// ```
    /// use aranet_store::Store;
    /// use aranet_types::{CurrentReading, Status};
    ///
    /// let store = Store::open_in_memory()?;
    /// let reading = CurrentReading {
    ///     co2: 800,
    ///     temperature: 22.5,
    ///     pressure: 1013.0,
    ///     humidity: 45,
    ///     battery: 85,
    ///     status: Status::Green,
    ///     ..Default::default()
    /// };
    ///
    /// let row_id = store.insert_reading("Aranet4 17C3C", &reading)?;
    /// # Ok::<(), aranet_store::Error>(())
    /// ```
    pub fn insert_reading(&self, device_id: &str, reading: &CurrentReading) -> Result<i64> {
        // Ensure device exists
        self.upsert_device(device_id, None)?;

        let captured_at = reading
            .captured_at
            .unwrap_or_else(OffsetDateTime::now_utc)
            .unix_timestamp();

        self.conn.execute(
            "INSERT INTO readings (device_id, captured_at, co2, temperature, pressure,
             humidity, battery, status, radon, radiation_rate, radiation_total)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                device_id,
                captured_at,
                reading.co2,
                reading.temperature,
                reading.pressure,
                reading.humidity,
                reading.battery,
                format!("{:?}", reading.status),
                reading.radon,
                reading.radiation_rate,
                reading.radiation_total,
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Query readings with optional filters.
    ///
    /// Use [`ReadingQuery`] to build queries with device, time range,
    /// pagination, and ordering filters.
    ///
    /// # Arguments
    ///
    /// * `query` - Query parameters built using [`ReadingQuery`]
    ///
    /// # Example
    ///
    /// ```
    /// use aranet_store::{Store, ReadingQuery};
    /// use time::{OffsetDateTime, Duration};
    ///
    /// let store = Store::open_in_memory()?;
    ///
    /// // Query last 24 hours for a specific device
    /// let yesterday = OffsetDateTime::now_utc() - Duration::hours(24);
    /// let query = ReadingQuery::new()
    ///     .device("Aranet4 17C3C")
    ///     .since(yesterday)
    ///     .limit(100);
    ///
    /// let readings = store.query_readings(&query)?;
    /// for reading in readings {
    ///     println!("CO2: {} ppm at {}", reading.co2, reading.captured_at);
    /// }
    /// # Ok::<(), aranet_store::Error>(())
    /// ```
    pub fn query_readings(&self, query: &ReadingQuery) -> Result<Vec<StoredReading>> {
        let sql = query.build_sql();
        let (_, params) = query.build_where();

        debug!("Executing query: {}", sql);

        let params_ref: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let mut stmt = self.conn.prepare(&sql)?;
        let readings = stmt
            .query_map(params_ref.as_slice(), |row| {
                Ok(StoredReading {
                    id: row.get(0)?,
                    device_id: row.get(1)?,
                    captured_at: timestamp_from_unix(row.get(2)?),
                    co2: row.get::<_, i64>(3)? as u16,
                    temperature: row.get(4)?,
                    pressure: row.get(5)?,
                    humidity: row.get::<_, i64>(6)? as u8,
                    battery: row.get::<_, i64>(7)? as u8,
                    status: parse_status(&row.get::<_, String>(8)?),
                    radon: row.get::<_, Option<i64>>(9)?.map(|v| v as u32),
                    radiation_rate: row.get(10)?,
                    radiation_total: row.get(11)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(readings)
    }

    /// Get the most recent reading for a device.
    ///
    /// Convenience method equivalent to `query_readings` with `limit(1)`.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The device to get the latest reading for
    ///
    /// # Returns
    ///
    /// The most recent reading, or `None` if no readings exist for this device.
    ///
    /// # Example
    ///
    /// ```
    /// use aranet_store::Store;
    ///
    /// let store = Store::open_in_memory()?;
    ///
    /// if let Some(reading) = store.get_latest_reading("Aranet4 17C3C")? {
    ///     println!("Latest CO2: {} ppm", reading.co2);
    /// }
    /// # Ok::<(), aranet_store::Error>(())
    /// ```
    pub fn get_latest_reading(&self, device_id: &str) -> Result<Option<StoredReading>> {
        let query = ReadingQuery::new().device(device_id).limit(1);
        let mut readings = self.query_readings(&query)?;
        Ok(readings.pop())
    }

    /// Count total readings, optionally filtered by device.
    ///
    /// # Arguments
    ///
    /// * `device_id` - If `Some`, count only readings for this device.
    ///   If `None`, count all readings across all devices.
    ///
    /// # Example
    ///
    /// ```
    /// use aranet_store::Store;
    ///
    /// let store = Store::open_in_memory()?;
    ///
    /// // Count all readings
    /// let total = store.count_readings(None)?;
    ///
    /// // Count for specific device
    /// let device_count = store.count_readings(Some("Aranet4 17C3C"))?;
    /// # Ok::<(), aranet_store::Error>(())
    /// ```
    pub fn count_readings(&self, device_id: Option<&str>) -> Result<u64> {
        let count: i64 = match device_id {
            Some(id) => self.conn.query_row(
                "SELECT COUNT(*) FROM readings WHERE device_id = ?",
                [id],
                |row| row.get(0),
            )?,
            None => self
                .conn
                .query_row("SELECT COUNT(*) FROM readings", [], |row| row.get(0))?,
        };

        Ok(count as u64)
    }
}

// History operations
impl Store {
    /// Insert history records with automatic deduplication.
    ///
    /// Records are deduplicated by `(device_id, timestamp)` - if a record with
    /// the same timestamp already exists for this device, it is skipped.
    /// This allows safe re-syncing without creating duplicates.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The device these history records belong to
    /// * `records` - Slice of history records to insert
    ///
    /// # Returns
    ///
    /// The number of records actually inserted (excluding duplicates).
    ///
    /// # Example
    ///
    /// ```
    /// use aranet_store::Store;
    /// use aranet_types::HistoryRecord;
    /// use time::OffsetDateTime;
    ///
    /// let store = Store::open_in_memory()?;
    ///
    /// let records = vec![
    ///     HistoryRecord {
    ///         timestamp: OffsetDateTime::now_utc(),
    ///         co2: 800,
    ///         temperature: 22.5,
    ///         pressure: 1013.0,
    ///         humidity: 45,
    ///         radon: None,
    ///         radiation_rate: None,
    ///         radiation_total: None,
    ///     },
    /// ];
    ///
    /// let inserted = store.insert_history("Aranet4 17C3C", &records)?;
    /// println!("Inserted {} new records", inserted);
    /// # Ok::<(), aranet_store::Error>(())
    /// ```
    pub fn insert_history(&self, device_id: &str, records: &[HistoryRecord]) -> Result<usize> {
        // Ensure device exists
        self.upsert_device(device_id, None)?;

        let synced_at = OffsetDateTime::now_utc().unix_timestamp();
        let mut inserted = 0;

        for record in records {
            let result = self.conn.execute(
                "INSERT OR IGNORE INTO history (device_id, timestamp, synced_at, co2,
                 temperature, pressure, humidity, radon, radiation_rate, radiation_total)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    device_id,
                    record.timestamp.unix_timestamp(),
                    synced_at,
                    record.co2,
                    record.temperature,
                    record.pressure,
                    record.humidity,
                    record.radon,
                    record.radiation_rate,
                    record.radiation_total,
                ],
            )?;
            inserted += result;
        }

        info!(
            "Inserted {} new history records for {}",
            inserted, device_id
        );
        Ok(inserted)
    }

    /// Query history records with optional filters.
    ///
    /// Use [`HistoryQuery`] to build queries with device, time range,
    /// pagination, and ordering filters.
    ///
    /// # Arguments
    ///
    /// * `query` - Query parameters built using [`HistoryQuery`]
    ///
    /// # Example
    ///
    /// ```
    /// use aranet_store::{Store, HistoryQuery};
    /// use time::{OffsetDateTime, Duration};
    ///
    /// let store = Store::open_in_memory()?;
    ///
    /// // Query last week's history for a device
    /// let week_ago = OffsetDateTime::now_utc() - Duration::days(7);
    /// let query = HistoryQuery::new()
    ///     .device("Aranet4 17C3C")
    ///     .since(week_ago)
    ///     .oldest_first();
    ///
    /// let records = store.query_history(&query)?;
    /// # Ok::<(), aranet_store::Error>(())
    /// ```
    pub fn query_history(&self, query: &HistoryQuery) -> Result<Vec<StoredHistoryRecord>> {
        let mut conditions = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(ref device_id) = query.device_id {
            conditions.push("device_id = ?");
            params.push(Box::new(device_id.clone()));
        }

        if let Some(since) = query.since {
            conditions.push("timestamp >= ?");
            params.push(Box::new(since.unix_timestamp()));
        }

        if let Some(until) = query.until {
            conditions.push("timestamp <= ?");
            params.push(Box::new(until.unix_timestamp()));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let order = if query.newest_first { "DESC" } else { "ASC" };

        let mut sql = format!(
            "SELECT id, device_id, timestamp, synced_at, co2, temperature, pressure,
             humidity, radon, radiation_rate, radiation_total
             FROM history {} ORDER BY timestamp {}",
            where_clause, order
        );

        if let Some(limit) = query.limit {
            sql.push_str(&format!(" LIMIT {}", limit));
        }

        if let Some(offset) = query.offset {
            sql.push_str(&format!(" OFFSET {}", offset));
        }

        let params_ref: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let mut stmt = self.conn.prepare(&sql)?;
        let records = stmt
            .query_map(params_ref.as_slice(), |row| {
                Ok(StoredHistoryRecord {
                    id: row.get(0)?,
                    device_id: row.get(1)?,
                    timestamp: timestamp_from_unix(row.get(2)?),
                    synced_at: timestamp_from_unix(row.get(3)?),
                    co2: row.get::<_, i64>(4)? as u16,
                    temperature: row.get(5)?,
                    pressure: row.get(6)?,
                    humidity: row.get::<_, i64>(7)? as u8,
                    radon: row.get::<_, Option<i64>>(8)?.map(|v| v as u32),
                    radiation_rate: row.get(9)?,
                    radiation_total: row.get(10)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Count total history records, optionally filtered by device.
    ///
    /// # Arguments
    ///
    /// * `device_id` - If `Some`, count only records for this device.
    ///   If `None`, count all records across all devices.
    ///
    /// # Example
    ///
    /// ```
    /// use aranet_store::Store;
    ///
    /// let store = Store::open_in_memory()?;
    ///
    /// // Count all history records
    /// let total = store.count_history(None)?;
    ///
    /// // Count for specific device
    /// let device_count = store.count_history(Some("Aranet4 17C3C"))?;
    /// # Ok::<(), aranet_store::Error>(())
    /// ```
    pub fn count_history(&self, device_id: Option<&str>) -> Result<u64> {
        let count: i64 = match device_id {
            Some(id) => self.conn.query_row(
                "SELECT COUNT(*) FROM history WHERE device_id = ?",
                [id],
                |row| row.get(0),
            )?,
            None => self
                .conn
                .query_row("SELECT COUNT(*) FROM history", [], |row| row.get(0))?,
        };

        Ok(count as u64)
    }
}

// Sync state operations
impl Store {
    /// Get the sync state for a device.
    ///
    /// Sync state tracks the last downloaded history index and total readings,
    /// enabling incremental history downloads instead of re-downloading everything.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The device to get sync state for
    ///
    /// # Returns
    ///
    /// The sync state if any history has been synced, `None` for new devices.
    ///
    /// # Example
    ///
    /// ```
    /// use aranet_store::Store;
    ///
    /// let store = Store::open_in_memory()?;
    /// store.upsert_device("Aranet4 17C3C", None)?;
    ///
    /// // Initially no sync state
    /// let state = store.get_sync_state("Aranet4 17C3C")?;
    /// assert!(state.is_none());
    /// # Ok::<(), aranet_store::Error>(())
    /// ```
    pub fn get_sync_state(&self, device_id: &str) -> Result<Option<SyncState>> {
        let mut stmt = self.conn.prepare(
            "SELECT device_id, last_history_index, total_readings, last_sync_at
             FROM sync_state WHERE device_id = ?",
        )?;

        let state = stmt
            .query_row([device_id], |row| {
                Ok(SyncState {
                    device_id: row.get(0)?,
                    last_history_index: row.get::<_, Option<i64>>(1)?.map(|v| v as u16),
                    total_readings: row.get::<_, Option<i64>>(2)?.map(|v| v as u16),
                    last_sync_at: row.get::<_, Option<i64>>(3)?.map(timestamp_from_unix),
                })
            })
            .optional()?;

        Ok(state)
    }

    /// Update sync state after a successful history download.
    ///
    /// Call this after downloading history records to track progress. The next
    /// sync can then use [`calculate_sync_start`](Self::calculate_sync_start) to
    /// determine which records to download.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The device that was synced
    /// * `last_index` - The highest history index that was downloaded (1-based)
    /// * `total_readings` - Total readings on the device at sync time
    ///
    /// # Example
    ///
    /// ```
    /// use aranet_store::Store;
    ///
    /// let store = Store::open_in_memory()?;
    /// store.upsert_device("Aranet4 17C3C", None)?;
    ///
    /// // After downloading all 500 history records
    /// store.update_sync_state("Aranet4 17C3C", 500, 500)?;
    ///
    /// // Verify sync state was saved
    /// let state = store.get_sync_state("Aranet4 17C3C")?.unwrap();
    /// assert_eq!(state.last_history_index, Some(500));
    /// # Ok::<(), aranet_store::Error>(())
    /// ```
    pub fn update_sync_state(
        &self,
        device_id: &str,
        last_index: u16,
        total_readings: u16,
    ) -> Result<()> {
        let now = OffsetDateTime::now_utc().unix_timestamp();

        self.conn.execute(
            "INSERT INTO sync_state (device_id, last_history_index, total_readings, last_sync_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(device_id) DO UPDATE SET
                last_history_index = ?2,
                total_readings = ?3,
                last_sync_at = ?4",
            rusqlite::params![device_id, last_index, total_readings, now],
        )?;

        debug!(
            "Updated sync state for {}: index={}, total={}",
            device_id, last_index, total_readings
        );

        Ok(())
    }

    /// Calculate the start index for incremental sync.
    ///
    /// Returns the index to start downloading from (1-based).
    /// If the device has new readings since last sync, returns the next index.
    /// If this is the first sync, returns 1 to download all.
    ///
    /// # Buffer Wrap-Around Detection
    ///
    /// Aranet devices have a circular buffer (e.g., ~2016 readings for Aranet4 at 10-min
    /// intervals). When the buffer fills up, new readings replace the oldest ones, but
    /// `total_readings` stays constant. This function detects this wrap-around case by
    /// comparing the latest stored timestamp with the expected time since last sync.
    pub fn calculate_sync_start(&self, device_id: &str, current_total: u16) -> Result<u16> {
        let state = self.get_sync_state(device_id)?;

        match state {
            Some(s) if s.total_readings == Some(current_total) => {
                // Same total readings as last sync - could mean:
                // 1. No new readings (buffer not full, recent sync)
                // 2. Buffer wrapped (old readings replaced with new)
                // 3. History cache was cleared but sync state exists

                // Check if buffer has likely wrapped by comparing timestamps
                if s.last_sync_at.is_some() {
                    let latest_stored = self.get_latest_history_timestamp(device_id)?;

                    match latest_stored {
                        Some(latest_ts) => {
                            let now = OffsetDateTime::now_utc();
                            let time_since_latest = now - latest_ts;

                            // If more than 10 minutes since latest record, new data likely exists
                            // (10 min is the longest standard Aranet4 interval)
                            if time_since_latest > time::Duration::minutes(10) {
                                debug!(
                                    "Buffer may have wrapped for {} (latest record is {} min old), doing full sync",
                                    device_id,
                                    time_since_latest.whole_minutes()
                                );
                                return Ok(1);
                            }

                            // Recent sync and no indication of wrap-around
                            debug!("No new readings for {}", device_id);
                            Ok(current_total + 1)
                        }
                        None => {
                            // Sync state exists but no history records - cache was likely cleared
                            // Do a full sync to repopulate
                            debug!(
                                "Sync state exists but no history for {}, doing full sync",
                                device_id
                            );
                            Ok(1)
                        }
                    }
                } else {
                    // No last_sync_at - shouldn't happen but do full sync to be safe
                    debug!("No sync timestamp for {}, doing full sync", device_id);
                    Ok(1)
                }
            }
            Some(s) if s.last_history_index.is_some() => {
                // We have previous state, calculate new records
                let last_index = s.last_history_index.unwrap();
                let prev_total = s.total_readings.unwrap_or(0);

                // Check if device was reset (current_total < prev_total)
                if current_total < prev_total {
                    debug!(
                        "Device total decreased ({} -> {}) for {}, device was reset - doing full sync",
                        prev_total, current_total, device_id
                    );
                    return Ok(1);
                }

                let new_count = current_total.saturating_sub(prev_total);

                if new_count > 0 {
                    // Start from where we left off
                    let start = last_index.saturating_add(1);

                    // Validate start index doesn't exceed current total
                    // This can happen if device buffer wrapped or was reset
                    if start > current_total {
                        debug!(
                            "Start index {} exceeds device total {} for {}, doing full sync",
                            start, current_total, device_id
                        );
                        return Ok(1);
                    }

                    debug!(
                        "Incremental sync for {}: {} new readings, starting at {}",
                        device_id, new_count, start
                    );
                    Ok(start)
                } else {
                    Ok(current_total + 1)
                }
            }
            _ => {
                // First sync - download all
                debug!(
                    "First sync for {}: downloading all {} readings",
                    device_id, current_total
                );
                Ok(1)
            }
        }
    }

    /// Get the timestamp of the most recent history record for a device.
    ///
    /// Returns `None` if no history exists for the device.
    fn get_latest_history_timestamp(&self, device_id: &str) -> Result<Option<OffsetDateTime>> {
        let ts: Option<i64> = self
            .conn
            .query_row(
                "SELECT MAX(timestamp) FROM history WHERE device_id = ?",
                [device_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();

        Ok(ts.map(timestamp_from_unix))
    }
}

/// Aggregate statistics for history data.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HistoryStats {
    /// Number of records.
    pub count: u64,
    /// Minimum values.
    pub min: HistoryAggregates,
    /// Maximum values.
    pub max: HistoryAggregates,
    /// Average values.
    pub avg: HistoryAggregates,
    /// Time range of records.
    pub time_range: Option<(OffsetDateTime, OffsetDateTime)>,
}

/// Aggregate values for a single metric set.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HistoryAggregates {
    /// CO2 in ppm.
    pub co2: Option<f64>,
    /// Temperature in Celsius.
    pub temperature: Option<f64>,
    /// Pressure in hPa.
    pub pressure: Option<f64>,
    /// Humidity percentage.
    pub humidity: Option<f64>,
    /// Radon in Bq/m3 (for radon devices).
    pub radon: Option<f64>,
}

// Aggregate and export operations
impl Store {
    /// Calculate aggregate statistics for history records.
    ///
    /// Computes min, max, and average values for all sensor metrics across
    /// the records matching the query. Useful for dashboards and reports.
    ///
    /// # Arguments
    ///
    /// * `query` - Filter which records to include in the statistics
    ///
    /// # Example
    ///
    /// ```
    /// use aranet_store::{Store, HistoryQuery};
    /// use time::{OffsetDateTime, Duration};
    ///
    /// let store = Store::open_in_memory()?;
    ///
    /// // Get stats for last 24 hours
    /// let yesterday = OffsetDateTime::now_utc() - Duration::hours(24);
    /// let query = HistoryQuery::new()
    ///     .device("Aranet4 17C3C")
    ///     .since(yesterday);
    ///
    /// let stats = store.history_stats(&query)?;
    /// if let Some(avg_co2) = stats.avg.co2 {
    ///     println!("Average CO2: {:.0} ppm", avg_co2);
    /// }
    /// if let Some((start, end)) = stats.time_range {
    ///     println!("Time range: {} to {}", start, end);
    /// }
    /// # Ok::<(), aranet_store::Error>(())
    /// ```
    pub fn history_stats(&self, query: &HistoryQuery) -> Result<HistoryStats> {
        let mut conditions = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(ref device_id) = query.device_id {
            conditions.push("device_id = ?");
            params.push(Box::new(device_id.clone()));
        }

        if let Some(since) = query.since {
            conditions.push("timestamp >= ?");
            params.push(Box::new(since.unix_timestamp()));
        }

        if let Some(until) = query.until {
            conditions.push("timestamp <= ?");
            params.push(Box::new(until.unix_timestamp()));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let sql = format!(
            "SELECT
                COUNT(*) as count,
                MIN(co2) as min_co2, MAX(co2) as max_co2, AVG(co2) as avg_co2,
                MIN(temperature) as min_temp, MAX(temperature) as max_temp, AVG(temperature) as avg_temp,
                MIN(pressure) as min_press, MAX(pressure) as max_press, AVG(pressure) as avg_press,
                MIN(humidity) as min_hum, MAX(humidity) as max_hum, AVG(humidity) as avg_hum,
                MIN(radon) as min_radon, MAX(radon) as max_radon, AVG(radon) as avg_radon,
                MIN(timestamp) as min_ts, MAX(timestamp) as max_ts
             FROM history {}",
            where_clause
        );

        let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let stats = self.conn.query_row(&sql, params_refs.as_slice(), |row| {
            let count: i64 = row.get(0)?;
            let min_ts: Option<i64> = row.get(16)?;
            let max_ts: Option<i64> = row.get(17)?;

            let time_range = match (min_ts, max_ts) {
                (Some(min), Some(max)) => {
                    Some((timestamp_from_unix(min), timestamp_from_unix(max)))
                }
                _ => None,
            };

            Ok(HistoryStats {
                count: count as u64,
                min: HistoryAggregates {
                    co2: row.get::<_, Option<i64>>(1)?.map(|v| v as f64),
                    temperature: row.get(4)?,
                    pressure: row.get(7)?,
                    humidity: row.get::<_, Option<i64>>(10)?.map(|v| v as f64),
                    radon: row.get::<_, Option<i64>>(13)?.map(|v| v as f64),
                },
                max: HistoryAggregates {
                    co2: row.get::<_, Option<i64>>(2)?.map(|v| v as f64),
                    temperature: row.get(5)?,
                    pressure: row.get(8)?,
                    humidity: row.get::<_, Option<i64>>(11)?.map(|v| v as f64),
                    radon: row.get::<_, Option<i64>>(14)?.map(|v| v as f64),
                },
                avg: HistoryAggregates {
                    co2: row.get(3)?,
                    temperature: row.get(6)?,
                    pressure: row.get(9)?,
                    humidity: row.get(12)?,
                    radon: row.get(15)?,
                },
                time_range,
            })
        })?;

        Ok(stats)
    }

    /// Export history records to CSV format.
    ///
    /// Exports records matching the query to a CSV string with the following columns:
    /// `timestamp`, `device_id`, `co2`, `temperature`, `pressure`, `humidity`, `radon`.
    ///
    /// Timestamps are formatted as RFC 3339 (e.g., `2024-01-15T10:30:00Z`).
    ///
    /// # Arguments
    ///
    /// * `query` - Filter which records to export
    ///
    /// # Example
    ///
    /// ```
    /// use aranet_store::{Store, HistoryQuery};
    /// use std::fs;
    ///
    /// let store = Store::open_in_memory()?;
    ///
    /// let query = HistoryQuery::new().device("Aranet4 17C3C").oldest_first();
    /// let csv = store.export_history_csv(&query)?;
    ///
    /// // Write to file
    /// // fs::write("history.csv", &csv)?;
    /// # Ok::<(), aranet_store::Error>(())
    /// ```
    pub fn export_history_csv(&self, query: &HistoryQuery) -> Result<String> {
        let records = self.query_history(query)?;
        let mut output = String::new();

        // Header
        output.push_str("timestamp,device_id,co2,temperature,pressure,humidity,radon\n");

        // Data rows
        for record in records {
            let timestamp = record
                .timestamp
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default();
            let radon = record.radon.map(|r| r.to_string()).unwrap_or_default();

            output.push_str(&format!(
                "{},{},{},{:.1},{:.2},{},{}\n",
                timestamp,
                record.device_id,
                record.co2,
                record.temperature,
                record.pressure,
                record.humidity,
                radon
            ));
        }

        Ok(output)
    }

    /// Export history records to JSON format.
    ///
    /// Exports records matching the query as a pretty-printed JSON array of
    /// [`StoredHistoryRecord`] objects.
    ///
    /// # Arguments
    ///
    /// * `query` - Filter which records to export
    ///
    /// # Example
    ///
    /// ```
    /// use aranet_store::{Store, HistoryQuery};
    ///
    /// let store = Store::open_in_memory()?;
    ///
    /// let query = HistoryQuery::new().device("Aranet4 17C3C");
    /// let json = store.export_history_json(&query)?;
    /// println!("{}", json);
    /// # Ok::<(), aranet_store::Error>(())
    /// ```
    pub fn export_history_json(&self, query: &HistoryQuery) -> Result<String> {
        let records = self.query_history(query)?;
        let json = serde_json::to_string_pretty(&records)
            .map_err(|e| Error::Database(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))?;
        Ok(json)
    }

    /// Import history records from CSV format.
    ///
    /// Expected CSV format:
    /// ```csv
    /// timestamp,device_id,co2,temperature,pressure,humidity,radon
    /// 2024-01-15T10:30:00Z,Aranet4 17C3C,800,22.5,1013.25,45,
    /// ```
    ///
    /// Returns the number of records imported (deduplicated by device_id + timestamp).
    pub fn import_history_csv(&self, csv_data: &str) -> Result<ImportResult> {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(true)
            .flexible(true)
            .trim(csv::Trim::All)
            .from_reader(csv_data.as_bytes());

        let mut total = 0;
        let mut imported = 0;
        let mut skipped = 0;
        let mut errors = Vec::new();

        for (line_num, result) in reader.records().enumerate() {
            total += 1;
            let line = line_num + 2; // Account for header and 0-indexing

            let record = match result {
                Ok(r) => r,
                Err(e) => {
                    errors.push(format!("Line {}: parse error - {}", line, e));
                    skipped += 1;
                    continue;
                }
            };

            // Parse fields
            let timestamp_str = record.get(0).unwrap_or("").trim();
            let device_id = record.get(1).unwrap_or("").trim();
            let co2_str = record.get(2).unwrap_or("").trim();
            let temp_str = record.get(3).unwrap_or("").trim();
            let pressure_str = record.get(4).unwrap_or("").trim();
            let humidity_str = record.get(5).unwrap_or("").trim();
            let radon_str = record.get(6).unwrap_or("").trim();

            // Validate required fields
            if device_id.is_empty() {
                errors.push(format!("Line {}: missing device_id", line));
                skipped += 1;
                continue;
            }

            // Parse timestamp
            let timestamp = match OffsetDateTime::parse(
                timestamp_str,
                &time::format_description::well_known::Rfc3339,
            ) {
                Ok(ts) => ts,
                Err(_) => {
                    errors.push(format!(
                        "Line {}: invalid timestamp '{}'",
                        line, timestamp_str
                    ));
                    skipped += 1;
                    continue;
                }
            };

            // Parse numeric fields with defaults and validation
            let co2: u16 = match co2_str.parse::<u16>() {
                Ok(v) if v <= 10000 => v, // CO2 sensor max is typically 10000 ppm
                Ok(v) => {
                    errors.push(format!(
                        "Line {}: CO2 value {} exceeds maximum of 10000 ppm",
                        line, v
                    ));
                    skipped += 1;
                    continue;
                }
                Err(_) if co2_str.is_empty() => 0,
                Err(_) => {
                    errors.push(format!("Line {}: invalid CO2 value '{}'", line, co2_str));
                    skipped += 1;
                    continue;
                }
            };

            let temperature: f32 = match temp_str.parse::<f32>() {
                Ok(v) if (-40.0..=100.0).contains(&v) => v,
                Ok(v) => {
                    errors.push(format!(
                        "Line {}: temperature {} is outside valid range (-40 to 100°C)",
                        line, v
                    ));
                    skipped += 1;
                    continue;
                }
                Err(_) if temp_str.is_empty() => 0.0,
                Err(_) => {
                    errors.push(format!(
                        "Line {}: invalid temperature value '{}'",
                        line, temp_str
                    ));
                    skipped += 1;
                    continue;
                }
            };

            let pressure: f32 = match pressure_str.parse::<f32>() {
                Ok(v) if v == 0.0 || (800.0..=1200.0).contains(&v) => v,
                Ok(v) => {
                    errors.push(format!(
                        "Line {}: pressure {} is outside valid range (800-1200 hPa)",
                        line, v
                    ));
                    skipped += 1;
                    continue;
                }
                Err(_) if pressure_str.is_empty() => 0.0,
                Err(_) => {
                    errors.push(format!(
                        "Line {}: invalid pressure value '{}'",
                        line, pressure_str
                    ));
                    skipped += 1;
                    continue;
                }
            };

            let humidity: u8 = match humidity_str.parse::<u8>() {
                Ok(v) if v <= 100 => v,
                Ok(v) => {
                    errors.push(format!(
                        "Line {}: humidity {} exceeds maximum of 100%",
                        line, v
                    ));
                    skipped += 1;
                    continue;
                }
                Err(_) if humidity_str.is_empty() => 0,
                Err(_) => {
                    errors.push(format!(
                        "Line {}: invalid humidity value '{}'",
                        line, humidity_str
                    ));
                    skipped += 1;
                    continue;
                }
            };

            let radon: Option<u32> = if radon_str.is_empty() {
                None
            } else {
                match radon_str.parse::<u32>() {
                    Ok(v) if v <= 100000 => Some(v), // Radon max ~100000 Bq/m³
                    Ok(v) => {
                        errors.push(format!(
                            "Line {}: radon value {} exceeds maximum of 100000 Bq/m³",
                            line, v
                        ));
                        skipped += 1;
                        continue;
                    }
                    Err(_) => {
                        errors.push(format!(
                            "Line {}: invalid radon value '{}'",
                            line, radon_str
                        ));
                        skipped += 1;
                        continue;
                    }
                }
            };

            // Create history record
            let history_record = HistoryRecord {
                timestamp,
                co2,
                temperature,
                pressure,
                humidity,
                radon,
                radiation_rate: None,
                radiation_total: None,
            };

            // Ensure device exists and insert record
            self.upsert_device(device_id, None)?;
            let count = self.insert_history(device_id, &[history_record])?;
            imported += count;
            if count == 0 {
                skipped += 1; // Duplicate record
            }
        }

        Ok(ImportResult {
            total,
            imported,
            skipped,
            errors,
        })
    }

    /// Import history records from JSON format.
    ///
    /// Expected JSON format: an array of StoredHistoryRecord objects.
    ///
    /// Returns the number of records imported (deduplicated by device_id + timestamp).
    pub fn import_history_json(&self, json_data: &str) -> Result<ImportResult> {
        let records: Vec<StoredHistoryRecord> = serde_json::from_str(json_data)
            .map_err(|e| Error::Database(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))?;

        let total = records.len();
        let mut imported = 0;
        let mut skipped = 0;

        for record in records {
            // Convert to HistoryRecord
            let history_record = record.to_history();

            // Ensure device exists and insert record
            self.upsert_device(&record.device_id, None)?;
            let count = self.insert_history(&record.device_id, &[history_record])?;
            imported += count;
            if count == 0 {
                skipped += 1; // Duplicate record
            }
        }

        Ok(ImportResult {
            total,
            imported,
            skipped,
            errors: Vec::new(),
        })
    }
}

/// Result of an import operation.
#[derive(Debug, Clone)]
pub struct ImportResult {
    /// Total records processed.
    pub total: usize,
    /// Records successfully imported.
    pub imported: usize,
    /// Records skipped (duplicates or errors).
    pub skipped: usize,
    /// Error messages for failed records.
    pub errors: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use aranet_types::Status;

    fn create_test_reading() -> CurrentReading {
        CurrentReading {
            co2: 800,
            temperature: 22.5,
            pressure: 1013.0,
            humidity: 45,
            battery: 85,
            status: Status::Green,
            interval: 60,
            age: 30,
            captured_at: Some(OffsetDateTime::now_utc()),
            radon: None,
            radiation_rate: None,
            radiation_total: None,
            radon_avg_24h: None,
            radon_avg_7d: None,
            radon_avg_30d: None,
        }
    }

    #[test]
    fn test_open_in_memory() {
        let store = Store::open_in_memory().unwrap();
        let devices = store.list_devices().unwrap();
        assert!(devices.is_empty());
    }

    #[test]
    fn test_upsert_device() {
        let store = Store::open_in_memory().unwrap();

        let device = store.upsert_device("test-device", Some("Test")).unwrap();
        assert_eq!(device.id, "test-device");
        assert_eq!(device.name, Some("Test".to_string()));

        // Update name
        let device = store
            .upsert_device("test-device", Some("New Name"))
            .unwrap();
        assert_eq!(device.name, Some("New Name".to_string()));
    }

    #[test]
    fn test_insert_and_query_reading() {
        let store = Store::open_in_memory().unwrap();
        let reading = create_test_reading();

        store.insert_reading("test-device", &reading).unwrap();

        let query = ReadingQuery::new().device("test-device");
        let readings = store.query_readings(&query).unwrap();

        assert_eq!(readings.len(), 1);
        assert_eq!(readings[0].co2, 800);
        assert_eq!(readings[0].temperature, 22.5);
    }

    #[test]
    fn test_get_latest_reading() {
        let store = Store::open_in_memory().unwrap();

        let mut reading1 = create_test_reading();
        reading1.co2 = 700;
        store.insert_reading("test-device", &reading1).unwrap();

        let mut reading2 = create_test_reading();
        reading2.co2 = 900;
        store.insert_reading("test-device", &reading2).unwrap();

        let latest = store.get_latest_reading("test-device").unwrap().unwrap();
        assert_eq!(latest.co2, 900);
    }

    #[test]
    fn test_insert_history_deduplication() {
        let store = Store::open_in_memory().unwrap();

        let now = OffsetDateTime::now_utc();
        let records = vec![
            HistoryRecord {
                timestamp: now,
                co2: 800,
                temperature: 22.0,
                pressure: 1013.0,
                humidity: 45,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            },
            HistoryRecord {
                timestamp: now, // Same timestamp - should be deduplicated
                co2: 850,
                temperature: 23.0,
                pressure: 1014.0,
                humidity: 46,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            },
        ];

        let inserted = store.insert_history("test-device", &records).unwrap();
        assert_eq!(inserted, 1); // Only one inserted due to dedup

        let count = store.count_history(Some("test-device")).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_sync_state() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_device("test-device", None).unwrap();

        // Initially no sync state
        let state = store.get_sync_state("test-device").unwrap();
        assert!(state.is_none());

        // Update sync state
        store.update_sync_state("test-device", 100, 100).unwrap();

        let state = store.get_sync_state("test-device").unwrap().unwrap();
        assert_eq!(state.last_history_index, Some(100));
        assert_eq!(state.total_readings, Some(100));
        assert!(state.last_sync_at.is_some());
    }

    #[test]
    fn test_calculate_sync_start() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_device("test-device", None).unwrap();

        // First sync - should start from 1
        let start = store.calculate_sync_start("test-device", 100).unwrap();
        assert_eq!(start, 1);

        // Simulate syncing: insert history records and update state
        let now = OffsetDateTime::now_utc();
        let records = vec![HistoryRecord {
            timestamp: now,
            co2: 800,
            temperature: 22.0,
            pressure: 1013.0,
            humidity: 45,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
        }];
        store.insert_history("test-device", &records).unwrap();
        store.update_sync_state("test-device", 100, 100).unwrap();

        // No new readings and recent history exists - should return beyond range
        let start = store.calculate_sync_start("test-device", 100).unwrap();
        assert_eq!(start, 101);

        // New readings added - should start from 101
        let start = store.calculate_sync_start("test-device", 110).unwrap();
        assert_eq!(start, 101);
    }

    #[test]
    fn test_calculate_sync_start_cache_cleared() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_device("test-device", None).unwrap();

        // Simulate previous sync
        store.update_sync_state("test-device", 100, 100).unwrap();

        // No history records exist (cache was cleared) - should do full sync
        let start = store.calculate_sync_start("test-device", 100).unwrap();
        assert_eq!(start, 1);
    }

    #[test]
    fn test_calculate_sync_start_buffer_wrapped() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_device("test-device", None).unwrap();

        // Insert an old history record (more than 10 min ago)
        let old_time = OffsetDateTime::now_utc() - time::Duration::minutes(30);
        let records = vec![HistoryRecord {
            timestamp: old_time,
            co2: 800,
            temperature: 22.0,
            pressure: 1013.0,
            humidity: 45,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
        }];
        store.insert_history("test-device", &records).unwrap();
        store.update_sync_state("test-device", 100, 100).unwrap();

        // Device still shows 100 readings but latest record is old
        // This indicates buffer may have wrapped - should do full sync
        let start = store.calculate_sync_start("test-device", 100).unwrap();
        assert_eq!(start, 1);
    }

    #[test]
    fn test_calculate_sync_start_index_overflow() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_device("test-device", None).unwrap();

        // Simulate state where last_index exceeds current_total (buffer reset)
        store.update_sync_state("test-device", 500, 500).unwrap();

        // Device was reset and now has fewer readings
        // start would be 501 which exceeds 200, should do full sync
        let start = store.calculate_sync_start("test-device", 200).unwrap();
        assert_eq!(start, 1);
    }

    #[test]
    fn test_import_history_csv() {
        let store = Store::open_in_memory().unwrap();

        let csv_data = r#"timestamp,device_id,co2,temperature,pressure,humidity,radon
2024-01-15T10:30:00Z,Aranet4 17C3C,800,22.5,1013.25,45,
2024-01-15T11:30:00Z,Aranet4 17C3C,850,23.0,1014.00,48,
2024-01-15T12:30:00Z,AranetRn+ 306B8,0,21.0,1012.00,50,150
"#;

        let result = store.import_history_csv(csv_data).unwrap();

        assert_eq!(result.total, 3);
        assert_eq!(result.imported, 3);
        assert_eq!(result.skipped, 0);
        assert!(result.errors.is_empty());

        // Verify data was imported
        let devices = store.list_devices().unwrap();
        assert_eq!(devices.len(), 2);

        // Query defaults to newest_first=true (DESC order)
        let query = HistoryQuery::new().device("Aranet4 17C3C");
        let records = store.query_history(&query).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].co2, 850); // 11:30 - newest first
        assert_eq!(records[1].co2, 800); // 10:30 - oldest

        // Verify radon device
        let query = HistoryQuery::new().device("AranetRn+ 306B8");
        let records = store.query_history(&query).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].radon, Some(150));
    }

    #[test]
    fn test_import_history_csv_deduplication() {
        let store = Store::open_in_memory().unwrap();

        let csv_data = r#"timestamp,device_id,co2,temperature,pressure,humidity,radon
2024-01-15T10:30:00Z,test-device,800,22.5,1013.25,45,
"#;

        // Import once
        let result = store.import_history_csv(csv_data).unwrap();
        assert_eq!(result.imported, 1);

        // Import again - should skip duplicate
        let result = store.import_history_csv(csv_data).unwrap();
        assert_eq!(result.imported, 0);
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn test_import_history_csv_with_errors() {
        let store = Store::open_in_memory().unwrap();

        let csv_data = r#"timestamp,device_id,co2,temperature,pressure,humidity,radon
invalid-timestamp,test-device,800,22.5,1013.25,45,
2024-01-15T10:30:00Z,,800,22.5,1013.25,45,
2024-01-15T11:30:00Z,valid-device,900,23.0,1014.00,50,
"#;

        let result = store.import_history_csv(csv_data).unwrap();

        assert_eq!(result.total, 3);
        assert_eq!(result.imported, 1);
        assert_eq!(result.skipped, 2);
        assert_eq!(result.errors.len(), 2);
    }

    #[test]
    fn test_import_history_json() {
        let store = Store::open_in_memory().unwrap();

        let json_data = r#"[
            {
                "id": 0,
                "device_id": "Aranet4 17C3C",
                "timestamp": "2024-01-15T10:30:00Z",
                "synced_at": "2024-01-15T12:00:00Z",
                "co2": 800,
                "temperature": 22.5,
                "pressure": 1013.25,
                "humidity": 45,
                "radon": null,
                "radiation_rate": null,
                "radiation_total": null
            },
            {
                "id": 0,
                "device_id": "Aranet4 17C3C",
                "timestamp": "2024-01-15T11:30:00Z",
                "synced_at": "2024-01-15T12:00:00Z",
                "co2": 850,
                "temperature": 23.0,
                "pressure": 1014.0,
                "humidity": 48,
                "radon": null,
                "radiation_rate": null,
                "radiation_total": null
            }
        ]"#;

        let result = store.import_history_json(json_data).unwrap();

        assert_eq!(result.total, 2);
        assert_eq!(result.imported, 2);
        assert_eq!(result.skipped, 0);

        // Verify data was imported
        let query = HistoryQuery::new().device("Aranet4 17C3C");
        let records = store.query_history(&query).unwrap();
        assert_eq!(records.len(), 2);
    }

    // ==================== History Stats Tests ====================

    #[test]
    fn test_history_stats_empty() {
        let store = Store::open_in_memory().unwrap();

        let query = HistoryQuery::new();
        let stats = store.history_stats(&query).unwrap();

        assert_eq!(stats.count, 0);
        assert!(stats.min.co2.is_none());
        assert!(stats.max.co2.is_none());
        assert!(stats.avg.co2.is_none());
        assert!(stats.time_range.is_none());
    }

    #[test]
    fn test_history_stats_single_record() {
        let store = Store::open_in_memory().unwrap();

        let now = OffsetDateTime::now_utc();
        let records = vec![HistoryRecord {
            timestamp: now,
            co2: 800,
            temperature: 22.5,
            pressure: 1013.0,
            humidity: 45,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
        }];

        store.insert_history("test-device", &records).unwrap();

        let query = HistoryQuery::new();
        let stats = store.history_stats(&query).unwrap();

        assert_eq!(stats.count, 1);
        assert_eq!(stats.min.co2, Some(800.0));
        assert_eq!(stats.max.co2, Some(800.0));
        assert_eq!(stats.avg.co2, Some(800.0));
        assert_eq!(stats.min.temperature, Some(22.5));
        assert_eq!(stats.max.temperature, Some(22.5));
    }

    #[test]
    fn test_history_stats_multiple_records() {
        let store = Store::open_in_memory().unwrap();

        let base_time = OffsetDateTime::now_utc();
        let records = vec![
            HistoryRecord {
                timestamp: base_time,
                co2: 600,
                temperature: 20.0,
                pressure: 1010.0,
                humidity: 40,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            },
            HistoryRecord {
                timestamp: base_time + time::Duration::hours(1),
                co2: 800,
                temperature: 22.0,
                pressure: 1012.0,
                humidity: 50,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            },
            HistoryRecord {
                timestamp: base_time + time::Duration::hours(2),
                co2: 1000,
                temperature: 24.0,
                pressure: 1014.0,
                humidity: 60,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            },
        ];

        store.insert_history("test-device", &records).unwrap();

        let query = HistoryQuery::new();
        let stats = store.history_stats(&query).unwrap();

        assert_eq!(stats.count, 3);
        assert_eq!(stats.min.co2, Some(600.0));
        assert_eq!(stats.max.co2, Some(1000.0));
        assert_eq!(stats.avg.co2, Some(800.0));
        assert_eq!(stats.min.temperature, Some(20.0));
        assert_eq!(stats.max.temperature, Some(24.0));
        assert_eq!(stats.avg.humidity, Some(50.0));
    }

    #[test]
    fn test_history_stats_with_device_filter() {
        let store = Store::open_in_memory().unwrap();

        let now = OffsetDateTime::now_utc();

        // Device 1 - high CO2
        store
            .insert_history(
                "device-1",
                &[HistoryRecord {
                    timestamp: now,
                    co2: 1200,
                    temperature: 25.0,
                    pressure: 1015.0,
                    humidity: 55,
                    radon: None,
                    radiation_rate: None,
                    radiation_total: None,
                }],
            )
            .unwrap();

        // Device 2 - low CO2
        store
            .insert_history(
                "device-2",
                &[HistoryRecord {
                    timestamp: now,
                    co2: 400,
                    temperature: 18.0,
                    pressure: 1010.0,
                    humidity: 35,
                    radon: None,
                    radiation_rate: None,
                    radiation_total: None,
                }],
            )
            .unwrap();

        // Stats for device 1 only
        let query = HistoryQuery::new().device("device-1");
        let stats = store.history_stats(&query).unwrap();

        assert_eq!(stats.count, 1);
        assert_eq!(stats.avg.co2, Some(1200.0));
    }

    #[test]
    fn test_history_stats_with_time_range() {
        let store = Store::open_in_memory().unwrap();

        let base_time = OffsetDateTime::now_utc();
        let records = vec![
            HistoryRecord {
                timestamp: base_time - time::Duration::days(2),
                co2: 500,
                temperature: 19.0,
                pressure: 1008.0,
                humidity: 40,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            },
            HistoryRecord {
                timestamp: base_time,
                co2: 800,
                temperature: 22.0,
                pressure: 1012.0,
                humidity: 50,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            },
        ];

        store.insert_history("test-device", &records).unwrap();

        // Query only recent records
        let query = HistoryQuery::new().since(base_time - time::Duration::hours(1));
        let stats = store.history_stats(&query).unwrap();

        assert_eq!(stats.count, 1);
        assert_eq!(stats.avg.co2, Some(800.0));
    }

    #[test]
    fn test_history_stats_with_radon() {
        let store = Store::open_in_memory().unwrap();

        let now = OffsetDateTime::now_utc();
        let records = vec![
            HistoryRecord {
                timestamp: now,
                co2: 0,
                temperature: 20.0,
                pressure: 1010.0,
                humidity: 50,
                radon: Some(100),
                radiation_rate: None,
                radiation_total: None,
            },
            HistoryRecord {
                timestamp: now + time::Duration::hours(1),
                co2: 0,
                temperature: 20.0,
                pressure: 1010.0,
                humidity: 50,
                radon: Some(200),
                radiation_rate: None,
                radiation_total: None,
            },
        ];

        store.insert_history("radon-device", &records).unwrap();

        let query = HistoryQuery::new();
        let stats = store.history_stats(&query).unwrap();

        assert_eq!(stats.count, 2);
        assert_eq!(stats.min.radon, Some(100.0));
        assert_eq!(stats.max.radon, Some(200.0));
        assert_eq!(stats.avg.radon, Some(150.0));
    }

    #[test]
    fn test_history_stats_time_range_values() {
        let store = Store::open_in_memory().unwrap();

        // Use fixed timestamps to avoid precision issues with unix timestamp conversion
        use time::macros::datetime;
        let start = datetime!(2024-01-01 00:00:00 UTC);
        let end = datetime!(2024-01-08 00:00:00 UTC);

        let records = vec![
            HistoryRecord {
                timestamp: start,
                co2: 700,
                temperature: 21.0,
                pressure: 1011.0,
                humidity: 45,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            },
            HistoryRecord {
                timestamp: end,
                co2: 900,
                temperature: 23.0,
                pressure: 1013.0,
                humidity: 55,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            },
        ];

        store.insert_history("test-device", &records).unwrap();

        let query = HistoryQuery::new();
        let stats = store.history_stats(&query).unwrap();

        let (min_ts, max_ts) = stats.time_range.unwrap();
        assert_eq!(min_ts, start);
        assert_eq!(max_ts, end);
    }

    // ==================== Export Tests ====================

    #[test]
    fn test_export_history_csv_empty() {
        let store = Store::open_in_memory().unwrap();

        let query = HistoryQuery::new();
        let csv = store.export_history_csv(&query).unwrap();

        assert!(csv.starts_with("timestamp,device_id,co2,temperature,pressure,humidity,radon\n"));
        // Only header, no data
        assert_eq!(csv.lines().count(), 1);
    }

    #[test]
    fn test_export_history_csv_with_data() {
        let store = Store::open_in_memory().unwrap();

        let csv_data = r#"timestamp,device_id,co2,temperature,pressure,humidity,radon
2024-01-15T10:30:00Z,test-device,800,22.5,1013.25,45,
"#;
        store.import_history_csv(csv_data).unwrap();

        let query = HistoryQuery::new();
        let csv = store.export_history_csv(&query).unwrap();

        assert!(csv.contains("test-device"));
        assert!(csv.contains("800"));
        assert!(csv.contains("22.5"));
        assert!(csv.contains("1013.25"));
        assert!(csv.contains("45"));
    }

    #[test]
    fn test_export_history_csv_with_radon() {
        let store = Store::open_in_memory().unwrap();

        let now = OffsetDateTime::now_utc();
        let records = vec![HistoryRecord {
            timestamp: now,
            co2: 0,
            temperature: 20.0,
            pressure: 1010.0,
            humidity: 50,
            radon: Some(150),
            radiation_rate: None,
            radiation_total: None,
        }];

        store.insert_history("radon-device", &records).unwrap();

        let query = HistoryQuery::new();
        let csv = store.export_history_csv(&query).unwrap();

        assert!(csv.contains("150"));
    }

    #[test]
    fn test_export_history_csv_format() {
        let store = Store::open_in_memory().unwrap();

        let csv_data = r#"timestamp,device_id,co2,temperature,pressure,humidity,radon
2024-01-15T10:30:00Z,device-1,800,22.5,1013.25,45,
2024-01-15T11:30:00Z,device-1,850,23.0,1014.00,48,
"#;
        store.import_history_csv(csv_data).unwrap();

        let query = HistoryQuery::new().oldest_first();
        let csv = store.export_history_csv(&query).unwrap();

        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 3); // header + 2 records

        // Check header
        assert!(lines[0].contains("timestamp"));
        assert!(lines[0].contains("device_id"));
        assert!(lines[0].contains("co2"));

        // Check data ordering (oldest first)
        assert!(lines[1].contains("800"));
        assert!(lines[2].contains("850"));
    }

    #[test]
    fn test_export_history_json_empty() {
        let store = Store::open_in_memory().unwrap();

        let query = HistoryQuery::new();
        let json = store.export_history_json(&query).unwrap();

        assert_eq!(json.trim(), "[]");
    }

    #[test]
    fn test_export_history_json_with_data() {
        let store = Store::open_in_memory().unwrap();

        let now = OffsetDateTime::now_utc();
        let records = vec![HistoryRecord {
            timestamp: now,
            co2: 800,
            temperature: 22.5,
            pressure: 1013.0,
            humidity: 45,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
        }];

        store.insert_history("test-device", &records).unwrap();

        let query = HistoryQuery::new();
        let json = store.export_history_json(&query).unwrap();

        // Parse and verify
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0]["device_id"], "test-device");
        assert_eq!(parsed[0]["co2"], 800);
    }

    #[test]
    fn test_export_import_json_roundtrip() {
        let store = Store::open_in_memory().unwrap();

        let now = OffsetDateTime::now_utc();
        let original_records = vec![
            HistoryRecord {
                timestamp: now,
                co2: 750,
                temperature: 21.5,
                pressure: 1012.0,
                humidity: 48,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            },
            HistoryRecord {
                timestamp: now + time::Duration::hours(1),
                co2: 850,
                temperature: 22.5,
                pressure: 1013.0,
                humidity: 52,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            },
        ];

        store
            .insert_history("roundtrip-device", &original_records)
            .unwrap();

        // Export
        let query = HistoryQuery::new()
            .device("roundtrip-device")
            .oldest_first();
        let json = store.export_history_json(&query).unwrap();

        // Create new store and import
        let store2 = Store::open_in_memory().unwrap();
        let result = store2.import_history_json(&json).unwrap();

        assert_eq!(result.imported, 2);

        // Verify data matches
        let records = store2.query_history(&query).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].co2, 750);
        assert_eq!(records[1].co2, 850);
    }

    // ==================== Query Tests ====================

    #[test]
    fn test_query_readings_with_pagination() {
        let store = Store::open_in_memory().unwrap();

        // Insert 10 readings
        for i in 0..10 {
            let mut reading = create_test_reading();
            reading.co2 = 700 + i * 10;
            store.insert_reading("paginated-device", &reading).unwrap();
        }

        // Query with limit and offset
        let query = ReadingQuery::new()
            .device("paginated-device")
            .oldest_first()
            .limit(3)
            .offset(2);

        let readings = store.query_readings(&query).unwrap();
        assert_eq!(readings.len(), 3);
        assert_eq!(readings[0].co2, 720); // 3rd reading (offset 2)
        assert_eq!(readings[2].co2, 740); // 5th reading
    }

    #[test]
    fn test_query_readings_time_range() {
        let store = Store::open_in_memory().unwrap();

        let base_time = OffsetDateTime::now_utc();

        // Insert readings at different times
        let mut reading1 = create_test_reading();
        reading1.captured_at = Some(base_time - time::Duration::days(2));
        reading1.co2 = 600;
        store.insert_reading("time-device", &reading1).unwrap();

        let mut reading2 = create_test_reading();
        reading2.captured_at = Some(base_time - time::Duration::hours(1));
        reading2.co2 = 800;
        store.insert_reading("time-device", &reading2).unwrap();

        let mut reading3 = create_test_reading();
        reading3.captured_at = Some(base_time);
        reading3.co2 = 900;
        store.insert_reading("time-device", &reading3).unwrap();

        // Query last day only
        let query = ReadingQuery::new()
            .device("time-device")
            .since(base_time - time::Duration::days(1));

        let readings = store.query_readings(&query).unwrap();
        assert_eq!(readings.len(), 2);
    }

    #[test]
    fn test_query_history_with_pagination() {
        let store = Store::open_in_memory().unwrap();

        let base_time = OffsetDateTime::now_utc();
        let records: Vec<_> = (0..10)
            .map(|i| HistoryRecord {
                timestamp: base_time + time::Duration::hours(i),
                co2: 700 + (i as u16) * 10,
                temperature: 22.0,
                pressure: 1013.0,
                humidity: 50,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            })
            .collect();

        store.insert_history("paginated-device", &records).unwrap();

        // Query with limit and offset
        let query = HistoryQuery::new()
            .device("paginated-device")
            .oldest_first()
            .limit(3)
            .offset(2);

        let results = store.query_history(&query).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].co2, 720);
        assert_eq!(results[2].co2, 740);
    }

    // ==================== Device Tests ====================

    #[test]
    fn test_update_device_info() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_device("info-device", None).unwrap();

        let info = aranet_types::DeviceInfo {
            name: "My Aranet4".to_string(),
            model: "Aranet4".to_string(),
            serial: "ABC123".to_string(),
            firmware: "v1.2.0".to_string(),
            hardware: "1.0".to_string(),
            ..Default::default()
        };

        store.update_device_info("info-device", &info).unwrap();

        let device = store.get_device("info-device").unwrap().unwrap();
        assert_eq!(device.name, Some("My Aranet4".to_string()));
        assert_eq!(device.serial, Some("ABC123".to_string()));
        assert_eq!(device.firmware, Some("v1.2.0".to_string()));
        assert_eq!(device.device_type, Some(aranet_types::DeviceType::Aranet4));
    }

    #[test]
    fn test_update_device_info_aranet2() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_device("aranet2-device", None).unwrap();

        let info = aranet_types::DeviceInfo {
            name: "My Aranet2".to_string(),
            model: "Aranet2".to_string(),
            serial: "XYZ789".to_string(),
            firmware: "v2.0.0".to_string(),
            hardware: "2.0".to_string(),
            ..Default::default()
        };

        store.update_device_info("aranet2-device", &info).unwrap();

        let device = store.get_device("aranet2-device").unwrap().unwrap();
        assert_eq!(device.device_type, Some(aranet_types::DeviceType::Aranet2));
    }

    #[test]
    fn test_update_device_info_radon() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_device("radon-device", None).unwrap();

        let info = aranet_types::DeviceInfo {
            name: "My AranetRn+".to_string(),
            model: "AranetRn+ Radon".to_string(),
            serial: "RAD001".to_string(),
            firmware: "v1.0.0".to_string(),
            hardware: "1.0".to_string(),
            ..Default::default()
        };

        store.update_device_info("radon-device", &info).unwrap();

        let device = store.get_device("radon-device").unwrap().unwrap();
        assert_eq!(
            device.device_type,
            Some(aranet_types::DeviceType::AranetRadon)
        );
    }

    #[test]
    fn test_update_device_metadata() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_device("meta-device", None).unwrap();

        store
            .update_device_metadata(
                "meta-device",
                Some("Kitchen Sensor"),
                Some(aranet_types::DeviceType::Aranet4),
            )
            .unwrap();

        let device = store.get_device("meta-device").unwrap().unwrap();
        assert_eq!(device.name, Some("Kitchen Sensor".to_string()));
        assert_eq!(device.device_type, Some(aranet_types::DeviceType::Aranet4));
    }

    #[test]
    fn test_list_devices_ordered_by_last_seen() {
        let store = Store::open_in_memory().unwrap();

        // Insert devices and verify ordering
        // We'll use a longer sleep to ensure timestamp differences
        store.upsert_device("device-a", Some("First")).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        store.upsert_device("device-b", Some("Second")).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        store.upsert_device("device-c", Some("Third")).unwrap();

        let devices = store.list_devices().unwrap();
        assert_eq!(devices.len(), 3);

        // Verify devices are ordered by last_seen DESC (most recent first)
        // Since timestamps are stored as unix timestamps (seconds),
        // we need 1+ second sleep between inserts
        assert!(devices[0].last_seen >= devices[1].last_seen);
        assert!(devices[1].last_seen >= devices[2].last_seen);
    }

    #[test]
    fn test_count_readings() {
        let store = Store::open_in_memory().unwrap();

        // Insert readings for multiple devices
        for _ in 0..5 {
            store
                .insert_reading("device-1", &create_test_reading())
                .unwrap();
        }
        for _ in 0..3 {
            store
                .insert_reading("device-2", &create_test_reading())
                .unwrap();
        }

        // Count for specific device
        assert_eq!(store.count_readings(Some("device-1")).unwrap(), 5);
        assert_eq!(store.count_readings(Some("device-2")).unwrap(), 3);
        assert_eq!(store.count_readings(Some("nonexistent")).unwrap(), 0);

        // Count all
        assert_eq!(store.count_readings(None).unwrap(), 8);
    }

    #[test]
    fn test_count_history() {
        let store = Store::open_in_memory().unwrap();

        let now = OffsetDateTime::now_utc();

        // Insert history for multiple devices
        let records: Vec<_> = (0..5)
            .map(|i| HistoryRecord {
                timestamp: now + time::Duration::hours(i),
                co2: 800,
                temperature: 22.0,
                pressure: 1013.0,
                humidity: 50,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            })
            .collect();

        store.insert_history("device-1", &records).unwrap();
        store.insert_history("device-2", &records[..3]).unwrap();

        assert_eq!(store.count_history(Some("device-1")).unwrap(), 5);
        assert_eq!(store.count_history(Some("device-2")).unwrap(), 3);
        assert_eq!(store.count_history(None).unwrap(), 8);
    }

    // ==================== Edge Cases ====================

    #[test]
    fn test_reading_with_all_sensor_types() {
        let store = Store::open_in_memory().unwrap();

        // Aranet4 reading
        let reading = create_test_reading();
        store.insert_reading("aranet4", &reading).unwrap();

        // Radon reading
        let mut radon_reading = create_test_reading();
        radon_reading.co2 = 0;
        radon_reading.radon = Some(150);
        store.insert_reading("aranet-rn", &radon_reading).unwrap();

        // Radiation reading
        let mut rad_reading = create_test_reading();
        rad_reading.co2 = 0;
        rad_reading.radiation_rate = Some(0.12);
        rad_reading.radiation_total = Some(0.003);
        store.insert_reading("aranet-rad", &rad_reading).unwrap();

        // Query each device
        let aranet4_readings = store
            .query_readings(&ReadingQuery::new().device("aranet4"))
            .unwrap();
        assert_eq!(aranet4_readings.len(), 1);
        assert_eq!(aranet4_readings[0].co2, 800);

        let radon_readings = store
            .query_readings(&ReadingQuery::new().device("aranet-rn"))
            .unwrap();
        assert_eq!(radon_readings.len(), 1);
        assert_eq!(radon_readings[0].radon, Some(150));

        let rad_readings = store
            .query_readings(&ReadingQuery::new().device("aranet-rad"))
            .unwrap();
        assert_eq!(rad_readings.len(), 1);
        assert_eq!(rad_readings[0].radiation_rate, Some(0.12));
    }

    #[test]
    fn test_device_not_found_error() {
        let store = Store::open_in_memory().unwrap();

        // This should fail because the device doesn't exist
        // and we're not using upsert
        let result = store.get_device("nonexistent");
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_empty_device_name() {
        let store = Store::open_in_memory().unwrap();

        // Empty name should be treated as None
        let info = aranet_types::DeviceInfo {
            name: "".to_string(),
            model: "Aranet4".to_string(),
            ..Default::default()
        };

        store.upsert_device("empty-name-device", None).unwrap();
        store
            .update_device_info("empty-name-device", &info)
            .unwrap();

        let device = store.get_device("empty-name-device").unwrap().unwrap();
        // Name should remain None since we passed empty string
        assert!(device.name.is_none());
    }

    #[test]
    fn test_import_csv_invalid_json() {
        let store = Store::open_in_memory().unwrap();

        let result = store.import_history_json("not valid json");
        assert!(result.is_err());
    }

    #[test]
    fn test_reading_with_all_status_types() {
        let store = Store::open_in_memory().unwrap();

        for status in [Status::Green, Status::Yellow, Status::Red, Status::Error] {
            let mut reading = create_test_reading();
            reading.status = status;
            let device_id = format!("status-{:?}", status);
            store.insert_reading(&device_id, &reading).unwrap();

            let stored = store.get_latest_reading(&device_id).unwrap().unwrap();
            assert_eq!(stored.status, status);
        }
    }

    // ==================== Concurrent Access Tests ====================
    //
    // These tests verify the store behaves correctly when accessed concurrently
    // through a Mutex, simulating the real-world usage in aranet-service.

    #[tokio::test]
    async fn test_concurrent_reading_inserts() {
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));

        // Spawn 10 concurrent tasks, each inserting 10 readings
        let mut handles = Vec::new();
        for task_id in 0..10 {
            let store = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                for i in 0..10 {
                    let reading = CurrentReading {
                        co2: 400 + (task_id * 100) + i,
                        temperature: 20.0 + (task_id as f32),
                        pressure: 1013.0,
                        humidity: 50,
                        battery: 85,
                        status: Status::Green,
                        interval: 60,
                        age: 0,
                        captured_at: Some(OffsetDateTime::now_utc()),
                        radon: None,
                        radiation_rate: None,
                        radiation_total: None,
                        radon_avg_24h: None,
                        radon_avg_7d: None,
                        radon_avg_30d: None,
                    };
                    let device_id = format!("concurrent-device-{}", task_id);
                    let guard = store.lock().await;
                    guard.insert_reading(&device_id, &reading).unwrap();
                }
            }));
        }

        // Wait for all tasks to complete
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify all readings were inserted
        let guard = store.lock().await;
        let total = guard.count_readings(None).unwrap();
        assert_eq!(total, 100); // 10 tasks * 10 readings each
    }

    #[tokio::test]
    async fn test_concurrent_reads_and_writes() {
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));

        // Pre-populate with some data
        {
            let guard = store.lock().await;
            for i in 0..10 {
                let reading = CurrentReading {
                    co2: 500 + i * 50,
                    temperature: 22.0,
                    pressure: 1013.0,
                    humidity: 50,
                    battery: 85,
                    status: Status::Green,
                    interval: 60,
                    age: 0,
                    captured_at: Some(OffsetDateTime::now_utc()),
                    radon: None,
                    radiation_rate: None,
                    radiation_total: None,
                    radon_avg_24h: None,
                    radon_avg_7d: None,
                    radon_avg_30d: None,
                };
                guard.insert_reading("shared-device", &reading).unwrap();
            }
        }

        // Spawn concurrent readers and writers
        let mut handles = Vec::new();

        // 5 reader tasks
        for _ in 0..5 {
            let store = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                for _ in 0..10 {
                    let guard = store.lock().await;
                    let readings = guard
                        .query_readings(&ReadingQuery::new().device("shared-device"))
                        .unwrap();
                    assert!(!readings.is_empty());
                    drop(guard);
                    tokio::task::yield_now().await;
                }
            }));
        }

        // 3 writer tasks
        for task_id in 0..3 {
            let store = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                for i in 0..5 {
                    let reading = CurrentReading {
                        co2: 1000 + (task_id * 100) + i,
                        temperature: 25.0,
                        pressure: 1015.0,
                        humidity: 55,
                        battery: 80,
                        status: Status::Yellow,
                        interval: 60,
                        age: 0,
                        captured_at: Some(OffsetDateTime::now_utc()),
                        radon: None,
                        radiation_rate: None,
                        radiation_total: None,
                        radon_avg_24h: None,
                        radon_avg_7d: None,
                        radon_avg_30d: None,
                    };
                    let guard = store.lock().await;
                    guard.insert_reading("shared-device", &reading).unwrap();
                    drop(guard);
                    tokio::task::yield_now().await;
                }
            }));
        }

        // Wait for all tasks
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify final state
        let guard = store.lock().await;
        let total = guard.count_readings(Some("shared-device")).unwrap();
        assert_eq!(total, 10 + (3 * 5)); // Initial 10 + 3 writers * 5 each = 25
    }

    #[tokio::test]
    async fn test_concurrent_device_upserts() {
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));

        // Spawn tasks that upsert the same device concurrently
        let mut handles = Vec::new();
        for i in 0..20 {
            let store = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                let guard = store.lock().await;
                guard
                    .upsert_device("contested-device", Some(&format!("Name-{}", i)))
                    .unwrap();
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        // Device should exist with one of the names
        let guard = store.lock().await;
        let device = guard.get_device("contested-device").unwrap().unwrap();
        assert!(device.name.unwrap().starts_with("Name-"));
    }
}
