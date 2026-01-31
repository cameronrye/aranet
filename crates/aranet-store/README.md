<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg">
    <img alt="Aranet" src="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg" height="60">
  </picture>
</p>

# aranet-store

[![crates.io](https://img.shields.io/crates/v/aranet-store.svg)](https://crates.io/crates/aranet-store)
[![docs.rs](https://docs.rs/aranet-store/badge.svg)](https://docs.rs/aranet-store)

Local data persistence for Aranet sensor readings using SQLite.

**[Full Documentation](https://cameronrye.github.io/aranet/)**

This crate provides SQLite-based storage for Aranet sensor data, enabling offline access, history caching, and efficient queries without requiring a device connection.

## Features

- **SQLite-based storage** — Single-file database, no server needed
- **Incremental history sync** — Only download new records from device
- **Query by device, time range** — With pagination support
- **Sync state tracking** — Per-device progress for efficient updates
- **Deduplication** — Automatic deduplication of history records

## Installation

```toml
[dependencies]
aranet-store = "0.1"
```

## Usage

```rust
use aranet_store::{Store, ReadingQuery, HistoryQuery};

// Open or create database at default location
let store = Store::open_default()?;

// Register a device
store.upsert_device("AA:BB:CC:DD:EE:FF", Some("Living Room"))?;

// Store a reading
store.insert_reading("AA:BB:CC:DD:EE:FF", &reading)?;

// Query readings with filters
let query = ReadingQuery::new()
    .device("AA:BB:CC:DD:EE:FF")
    .limit(100);
let readings = store.query_readings(&query)?;

// Query cached history
let query = HistoryQuery::new()
    .device("AA:BB:CC:DD:EE:FF")
    .since(one_hour_ago);
let history = store.query_history(&query)?;

// Get sync state for incremental updates
let sync_state = store.get_sync_state("AA:BB:CC:DD:EE:FF")?;
```

## Database Location

By default, the database is stored at platform-specific locations:

| Platform | Path |
|----------|------|
| Linux | `~/.local/share/aranet/data.db` |
| macOS | `~/Library/Application Support/aranet/data.db` |
| Windows | `C:\Users\<user>\AppData\Local\aranet\data.db` |

## Schema

The database contains four tables:

| Table | Description |
|-------|-------------|
| `devices` | Known devices and their metadata (name, firmware, model) |
| `readings` | Current readings captured over time |
| `history` | Historical records downloaded from device memory |
| `sync_state` | Tracks incremental sync progress per device |

## CLI Integration

The `aranet-cli` tool provides commands for interacting with the store:

```bash
# Sync device history to local database
aranet sync --device <ADDRESS>

# Query cached data
aranet cache devices   # List cached devices
aranet cache stats     # Show cache statistics
aranet cache history   # Query cached history
aranet cache info      # Show database info
```

## Related Crates

This crate is part of the [aranet](https://github.com/cameronrye/aranet) workspace:

| Crate | crates.io | Description |
|-------|-----------|-------------|
| [aranet-core](../aranet-core/) | [![crates.io](https://img.shields.io/crates/v/aranet-core.svg)](https://crates.io/crates/aranet-core) | Core BLE library for device communication |
| [aranet-types](../aranet-types/) | [![crates.io](https://img.shields.io/crates/v/aranet-types.svg)](https://crates.io/crates/aranet-types) | Shared types for sensor data |
| [aranet-cli](../aranet-cli/) | [![crates.io](https://img.shields.io/crates/v/aranet-cli.svg)](https://crates.io/crates/aranet-cli) | Command-line interface |
| [aranet-tui](../aranet-tui/) | [![crates.io](https://img.shields.io/crates/v/aranet-tui.svg)](https://crates.io/crates/aranet-tui) | Terminal UI dashboard |
| [aranet-service](../aranet-service/) | [![crates.io](https://img.shields.io/crates/v/aranet-service.svg)](https://crates.io/crates/aranet-service) | Background collector and REST API |
| [aranet-gui](../aranet-gui/) | [![crates.io](https://img.shields.io/crates/v/aranet-gui.svg)](https://crates.io/crates/aranet-gui) | Desktop GUI application |

## License

MIT

---

Made with ❤️ by [Cameron Rye](https://rye.dev/)

