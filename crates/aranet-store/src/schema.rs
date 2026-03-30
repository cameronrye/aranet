//! Database schema and migrations.

use rusqlite::Connection;

use crate::error::Result;

/// Current schema version.
pub const SCHEMA_VERSION: i32 = 3;

/// Initialize the database schema.
pub fn initialize(conn: &Connection) -> Result<()> {
    let version = get_schema_version(conn)?;

    if version == 0 {
        // Fresh database - create all tables in a single transaction
        let tx = conn.unchecked_transaction()?;
        create_schema_v1(&tx)?;
        set_schema_version(&tx, SCHEMA_VERSION)?;
        tx.commit()?;
    } else if version < SCHEMA_VERSION {
        // Run migrations atomically: if a migration or version update fails,
        // the entire transaction is rolled back so we don't end up in a
        // half-migrated state.
        let tx = conn.unchecked_transaction()?;
        migrate(&tx, version)?;
        tx.commit()?;
    }

    Ok(())
}

/// Get the current schema version.
fn get_schema_version(conn: &Connection) -> Result<i32> {
    // Check if the schema_version table exists
    let exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='schema_version'",
        [],
        |row| row.get(0),
    )?;

    if !exists {
        return Ok(0);
    }

    let version: i32 =
        conn.query_row("SELECT version FROM schema_version", [], |row| row.get(0))?;

    Ok(version)
}

/// Set the schema version.
fn set_schema_version(conn: &Connection, version: i32) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO schema_version (id, version) VALUES (1, ?)",
        [version],
    )?;
    Ok(())
}

/// Create the initial schema (version 1).
fn create_schema_v1(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        -- Schema version tracking
        CREATE TABLE IF NOT EXISTS schema_version (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            version INTEGER NOT NULL
        );

        -- Devices table
        CREATE TABLE IF NOT EXISTS devices (
            id TEXT PRIMARY KEY,
            name TEXT,
            device_type TEXT,
            serial TEXT,
            firmware TEXT,
            hardware TEXT,
            first_seen INTEGER NOT NULL,
            last_seen INTEGER NOT NULL
        );

        -- Current readings (polled values)
        CREATE TABLE IF NOT EXISTS readings (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
            captured_at INTEGER NOT NULL,
            co2 INTEGER NOT NULL DEFAULT 0,
            temperature REAL NOT NULL DEFAULT 0.0,
            pressure REAL NOT NULL DEFAULT 0.0,
            humidity INTEGER NOT NULL DEFAULT 0,
            battery INTEGER NOT NULL DEFAULT 0,
            status TEXT,
            radon INTEGER,
            radiation_rate REAL,
            radiation_total REAL,
            radon_avg_24h INTEGER,
            radon_avg_7d INTEGER,
            radon_avg_30d INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_readings_device_time
            ON readings(device_id, captured_at);
        CREATE INDEX IF NOT EXISTS idx_readings_captured_at
            ON readings(captured_at);

        -- History records (downloaded from device memory)
        CREATE TABLE IF NOT EXISTS history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
            timestamp INTEGER NOT NULL,
            synced_at INTEGER NOT NULL,
            co2 INTEGER NOT NULL DEFAULT 0,
            temperature REAL NOT NULL DEFAULT 0.0,
            pressure REAL NOT NULL DEFAULT 0.0,
            humidity INTEGER NOT NULL DEFAULT 0,
            radon INTEGER,
            radiation_rate REAL,
            radiation_total REAL,
            UNIQUE(device_id, timestamp)
        );
        CREATE INDEX IF NOT EXISTS idx_history_device_time
            ON history(device_id, timestamp);
        CREATE INDEX IF NOT EXISTS idx_history_timestamp
            ON history(timestamp);

        -- Sync state tracking (for incremental sync)
        CREATE TABLE IF NOT EXISTS sync_state (
            device_id TEXT PRIMARY KEY REFERENCES devices(id) ON DELETE CASCADE,
            last_history_index INTEGER,
            total_readings INTEGER,
            last_sync_at INTEGER
        );
        "#,
    )?;

    Ok(())
}

/// Run migrations from old_version to current.
///
/// Note: This should be called within a transaction by the caller.
/// The caller is responsible for setting the schema version after commit.
fn migrate(conn: &Connection, old_version: i32) -> Result<()> {
    if old_version < 2 {
        migrate_to_v2(conn)?;
    }

    if old_version < 3 {
        migrate_to_v3(conn)?;
    }

    if old_version > SCHEMA_VERSION {
        tracing::warn!(
            "Database schema version {} is newer than supported version {}. \
             This may cause compatibility issues.",
            old_version,
            SCHEMA_VERSION
        );
    }

    set_schema_version(conn, SCHEMA_VERSION)?;
    Ok(())
}

/// Migration to schema version 2: add radon average columns to readings table.
fn migrate_to_v2(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        ALTER TABLE readings ADD COLUMN radon_avg_24h INTEGER;
        ALTER TABLE readings ADD COLUMN radon_avg_7d INTEGER;
        ALTER TABLE readings ADD COLUMN radon_avg_30d INTEGER;
        "#,
    )?;
    Ok(())
}

/// Migration to schema version 3: add performance index for cross-device
/// time-range queries and backfill zero values for NULL sensor columns.
fn migrate_to_v3(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        -- Add standalone timestamp index for cross-device range queries
        CREATE INDEX IF NOT EXISTS idx_readings_captured_at
            ON readings(captured_at);
        CREATE INDEX IF NOT EXISTS idx_history_timestamp
            ON history(timestamp);

        -- Backfill NULL sensor columns with 0 defaults so future NOT NULL
        -- constraints (on fresh databases) stay consistent with existing data.
        UPDATE readings SET co2 = 0 WHERE co2 IS NULL;
        UPDATE readings SET humidity = 0 WHERE humidity IS NULL;
        UPDATE readings SET battery = 0 WHERE battery IS NULL;
        UPDATE readings SET temperature = 0.0 WHERE temperature IS NULL;
        UPDATE readings SET pressure = 0.0 WHERE pressure IS NULL;
        UPDATE history SET co2 = 0 WHERE co2 IS NULL;
        UPDATE history SET humidity = 0 WHERE humidity IS NULL;
        UPDATE history SET temperature = 0.0 WHERE temperature IS NULL;
        UPDATE history SET pressure = 0.0 WHERE pressure IS NULL;
        "#,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initialize_fresh_database() {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();

        // Verify tables exist
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();

        assert!(tables.contains(&"devices".to_string()));
        assert!(tables.contains(&"readings".to_string()));
        assert!(tables.contains(&"history".to_string()));
        assert!(tables.contains(&"sync_state".to_string()));
        assert!(tables.contains(&"schema_version".to_string()));
    }

    #[test]
    fn test_schema_version_tracking() {
        let conn = Connection::open_in_memory().unwrap();

        // Fresh database should have version 0
        assert_eq!(get_schema_version(&conn).unwrap(), 0);

        // After initialization, should have current version
        initialize(&conn).unwrap();
        assert_eq!(get_schema_version(&conn).unwrap(), SCHEMA_VERSION);
    }
}
