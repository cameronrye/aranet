//! Main store implementation.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension};
use time::OffsetDateTime;
use tracing::{debug, info};

use aranet_types::{CurrentReading, DeviceInfo, DeviceType, HistoryRecord, Status};

use crate::error::{Error, Result};
use crate::models::{StoredDevice, StoredHistoryRecord, StoredReading, SyncState};
use crate::queries::{HistoryQuery, ReadingQuery};
use crate::schema;

/// SQLite-based store for Aranet sensor data.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open or create a database at the given path.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent).map_err(|e| Error::CreateDirectory {
                    path: parent.to_path_buf(),
                    source: e,
                })?;
            }
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

    /// Open the default database location.
    pub fn open_default() -> Result<Self> {
        Self::open(crate::default_db_path())
    }

    /// Open an in-memory database (for testing).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        schema::initialize(&conn)?;
        Ok(Self { conn })
    }

    // === Device operations ===

    /// Get or create a device entry.
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

    /// Update device info from DeviceInfo.
    pub fn update_device_info(&self, device_id: &str, info: &DeviceInfo) -> Result<()> {
        // Infer device type from model name if possible
        let device_type = if info.model.contains("Aranet4") {
            Some("Aranet4")
        } else if info.model.contains("Aranet2") {
            Some("Aranet2")
        } else if info.model.contains("Radon") || info.model.contains("Rn") {
            Some("AranetRadon")
        } else if info.model.contains("Radiation") {
            Some("AranetRadiation")
        } else {
            None
        };

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

    /// Get a device by ID.
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
                    first_seen: OffsetDateTime::from_unix_timestamp(row.get(6)?).unwrap(),
                    last_seen: OffsetDateTime::from_unix_timestamp(row.get(7)?).unwrap(),
                })
            })
            .optional()?;

        Ok(device)
    }

    /// List all devices.
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
                    first_seen: OffsetDateTime::from_unix_timestamp(row.get(6)?).unwrap(),
                    last_seen: OffsetDateTime::from_unix_timestamp(row.get(7)?).unwrap(),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(devices)
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
    /// Insert a current reading.
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

    /// Query readings with filters.
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
                    captured_at: OffsetDateTime::from_unix_timestamp(row.get(2)?).unwrap(),
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

    /// Get the latest reading for a device.
    pub fn get_latest_reading(&self, device_id: &str) -> Result<Option<StoredReading>> {
        let query = ReadingQuery::new().device(device_id).limit(1);
        let mut readings = self.query_readings(&query)?;
        Ok(readings.pop())
    }

    /// Count readings for a device.
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
    /// Insert history records (with deduplication).
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

        info!("Inserted {} new history records for {}", inserted, device_id);
        Ok(inserted)
    }

    /// Query history records with filters.
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
                    timestamp: OffsetDateTime::from_unix_timestamp(row.get(2)?).unwrap(),
                    synced_at: OffsetDateTime::from_unix_timestamp(row.get(3)?).unwrap(),
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

    /// Count history records for a device.
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
    /// Get sync state for a device.
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
                    last_sync_at: row
                        .get::<_, Option<i64>>(3)?
                        .map(|ts| OffsetDateTime::from_unix_timestamp(ts).unwrap()),
                })
            })
            .optional()?;

        Ok(state)
    }

    /// Update sync state after a successful sync.
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
    pub fn calculate_sync_start(&self, device_id: &str, current_total: u16) -> Result<u16> {
        let state = self.get_sync_state(device_id)?;

        match state {
            Some(s) if s.total_readings == Some(current_total) => {
                // No new readings since last sync
                debug!("No new readings for {}", device_id);
                Ok(current_total + 1) // Return beyond range to indicate no sync needed
            }
            Some(s) if s.last_history_index.is_some() => {
                // We have previous state, calculate new records
                let last_index = s.last_history_index.unwrap();
                let prev_total = s.total_readings.unwrap_or(0);
                let new_count = current_total.saturating_sub(prev_total);

                if new_count > 0 {
                    // Start from where we left off
                    let start = last_index.saturating_add(1);
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
                debug!("First sync for {}: downloading all {} readings", device_id, current_total);
                Ok(1)
            }
        }
    }
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
        let device = store.upsert_device("test-device", Some("New Name")).unwrap();
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

        // After syncing all 100, update state
        store.update_sync_state("test-device", 100, 100).unwrap();

        // No new readings - should return beyond range
        let start = store.calculate_sync_start("test-device", 100).unwrap();
        assert_eq!(start, 101);

        // New readings added - should start from 101
        let start = store.calculate_sync_start("test-device", 110).unwrap();
        assert_eq!(start, 101);
    }
}

