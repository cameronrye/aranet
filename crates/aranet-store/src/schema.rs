//! Database schema and migrations.

use rusqlite::Connection;

use crate::error::Result;

/// Current schema version.
pub const SCHEMA_VERSION: i32 = 1;

/// Initialize the database schema.
pub fn initialize(conn: &Connection) -> Result<()> {
    let version = get_schema_version(conn)?;

    if version == 0 {
        // Fresh database - create all tables
        create_schema_v1(conn)?;
        set_schema_version(conn, SCHEMA_VERSION)?;
    } else if version < SCHEMA_VERSION {
        // Run migrations
        migrate(conn, version)?;
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
            device_id TEXT NOT NULL REFERENCES devices(id),
            captured_at INTEGER NOT NULL,
            co2 INTEGER,
            temperature REAL,
            pressure REAL,
            humidity INTEGER,
            battery INTEGER,
            status TEXT,
            radon INTEGER,
            radiation_rate REAL,
            radiation_total REAL
        );
        CREATE INDEX IF NOT EXISTS idx_readings_device_time 
            ON readings(device_id, captured_at);

        -- History records (downloaded from device memory)
        CREATE TABLE IF NOT EXISTS history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            device_id TEXT NOT NULL REFERENCES devices(id),
            timestamp INTEGER NOT NULL,
            synced_at INTEGER NOT NULL,
            co2 INTEGER,
            temperature REAL,
            pressure REAL,
            humidity INTEGER,
            radon INTEGER,
            radiation_rate REAL,
            radiation_total REAL,
            UNIQUE(device_id, timestamp)
        );
        CREATE INDEX IF NOT EXISTS idx_history_device_time 
            ON history(device_id, timestamp);

        -- Sync state tracking (for incremental sync)
        CREATE TABLE IF NOT EXISTS sync_state (
            device_id TEXT PRIMARY KEY REFERENCES devices(id),
            last_history_index INTEGER,
            total_readings INTEGER,
            last_sync_at INTEGER
        );
        "#,
    )?;

    Ok(())
}

/// Run migrations from old_version to current.
fn migrate(conn: &Connection, old_version: i32) -> Result<()> {
    // Add future migrations here
    // if old_version < 2 { migrate_to_v2(conn)?; }

    let _ = old_version; // Suppress unused warning
    set_schema_version(conn, SCHEMA_VERSION)?;
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
