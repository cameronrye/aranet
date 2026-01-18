# Aranet Roadmap

A complete Rust implementation for Aranet environmental sensors,
designed for feature parity with [Aranet4-Python](https://github.com/Anrijs/Aranet4-Python) and beyond.

> **Dependency Policy**: Always use the **latest stable versions** of all libraries, frameworks, and tools.
> Pin to major versions only (e.g., `btleplug = "0.11"` not `"0.11.4"`). Run `cargo update` regularly.
> Check [crates.io](https://crates.io) and [lib.rs](https://lib.rs) for current versions before adding dependencies.

---

## Current Progress (Updated Jan 18, 2026)

| Phase | Component | Status | Progress |
|-------|-----------|--------|----------|
| 0 | Foundation | Done | README, LICENSE, CI, CHANGELOG, aranet-types |
| 1 | Core Library | Done | Full BLE: scan, connect, read, history, settings - tested with real hardware |
| 2 | CLI Tool | Done | All core commands: scan, read, status, info, history, set, watch, config |
| 3 | TUI Dashboard | WIP | App shell + quit key; sensor integration pending |
| 4 | GUI Application | WIP | egui shell works; sensor integration pending |
| 5 | WASM Module | WIP | Basic init/log; Web Bluetooth pending |
| 6 | Data Persistence & API | WIP | aranet-store complete; aranet-service pending |

**Legend**: [ ] Not started - [~] In progress/partial - [x] Complete

### What's Working Now

- **aranet-core**: Complete BLE stack with btleplug 0.11 - scan, connect, device info, current readings,
  history download (V1+V2), settings read/write, auto-reconnection, streaming, notifications, RSSI,
  multi-device manager, event system, validation, thresholds, metrics, mock device
- **aranet-types**: Shared types for CurrentReading, DeviceInfo, HistoryRecord, Status, DeviceType, all UUIDs
- **Multi-device support**: Aranet4, Aranet2, Aranet Radon, Aranet Radiation - all parsing implemented and tested
- **AranetRn+ (Radon)**: Full support including current readings (radon, temp, pressure, humidity) and complete history download with 4-byte radon values
- **CLI complete**: All commands working (scan, read, status, info, history, set, watch, config)
- **Hardware tested**: Aranet4 17C3C (FW v1.4.19), AranetRn+ 306B8 (FW v1.12.0)

### Recent Improvements (Jan 2026)

- **Code coverage with cargo-llvm-cov**: CI now reports test coverage via Codecov
- **Property-based testing with proptest**: Fuzz testing for all byte parsers to catch edge cases
- **310+ tests**: Expanded from 268 to 310+ tests across the workspace
- **Bug fix**: Fixed Aranet Radiation advertisement parser panic on malformed data (found by proptest)
- **GUI/TUI tests**: Added component tests for aranet-gui and aranet-tui (previously 0 tests each)
- **Enhanced MockDevice tests**: Comprehensive coverage for history, settings, calibration operations
- **CLI Phase 2 complete**: All commands fully implemented
- `set` command for device settings (interval, range, smart_home)
- `watch` command for continuous monitoring with auto-reconnect
- `config` command for managing `~/.config/aranet/config.toml`
- Config file support with device, format, no_color, fahrenheit, inhg options
- Added `--json` global flag, `ARANET_DEVICE` env var, `--no-color` flag
- Added `status` command for quick one-line output
- Colored CO2 status indicators (green/amber/red)
- JSON and CSV output for all commands
- `doctor` command for BLE diagnostics and troubleshooting
- `alias` command for friendly device names
- `--passive` read mode from BLE advertisements
- Multi-device read with parallel connections
- Interactive device picker when no device specified
- History `--since`/`--until` date filters
- Progress bars for history download
- `--inhg`/`--hpa` pressure unit options
- All workspace tests passing (100% pass rate)

### Next Priority

1. Add sensor data display to TUI shell
2. Add sensor data display to GUI shell
3. Implement Web Bluetooth in WASM module
4. ~~**Data persistence layer (aranet-store)**~~ - Complete (v0.1.7)
5. **Background service (aranet-service)** - Data collector + REST API

## Vision

Build the definitive Rust ecosystem for Aranet devices:

- **aranet-types** - Platform-agnostic data types (shared by all crates)
- **aranet-core** - Native BLE client via btleplug
- **aranet-store** - Local data persistence with SQLite
- **aranet-service** - Background data collector and HTTP REST API
- **aranet-cli** - Feature-complete command-line interface
- **aranet-tui** - Real-time terminal UI for monitoring
- **aranet-gui** - Native desktop app (egui-based)
- **aranet-wasm** - Web Bluetooth integration for browsers

---

## Phase 0: Foundation [x]

### Milestone: Project Infrastructure

| Feature | Priority | Status |
|---------|----------|--------|
| README.md with documentation | P0 | [x] |
| LICENSE file (MIT) | P0 | [x] |
| CHANGELOG.md | P0 | [x] |
| GitHub Actions CI (build, test, lint) | P0 | [x] |
| Example programs | P0 | [x] |
| aranet-types crate (platform-agnostic) | P0 | [x] |

---

## Phase 1: Core Library (v0.1.0) [x]

### Milestone: Feature Parity with Python

| Feature | Priority | Status |
|---------|----------|--------|
| Connect to Aranet4 | P0 | [x] |
| Read current measurements (CO2, temp, humidity, pressure) | P0 | [x] |
| Read historical records | P0 | [x] |
| Device info (name, serial, firmware, hardware) | P0 | [x] |
| Battery level | P0 | [x] |
| Scan for devices | P0 | [x] |
| Connect by MAC address | P0 | [x] |
| Support BOTH service UUIDs (old + new firmware) | P0 | [x] |
| Read from BLE advertisements (passive scan) | P1 | [x] Full parsing implemented |
| Aranet2 support | P1 | [x] Parsing implemented |
| Aranet Radiation support | P1 | [x] Full GATT parsing implemented |
| Aranet Radon Plus support | P1 | [x] Parsing implemented |

### Settings (Read/Write)

| Feature | Priority | Status |
|---------|----------|--------|
| Get/Set measurement interval (1, 2, 5, 10 min) | P1 | [x] |
| Get/Set Bluetooth range (standard/extended) | P1 | [x] |
| Toggle Smart Home integrations | P1 | [x] |
| Read calibration data | P1 | [x] |

### Advanced Features

| Feature | Priority | Status |
|---------|----------|--------|
| Real-time reading streams | P1 | [x] `ReadingStream` with polling |
| Subscribe to BLE notifications | P1 | [x] |
| Auto-reconnection with backoff | P1 | [x] `ReconnectingDevice` |
| Multi-device manager | P1 | [x] `DeviceManager` |
| Event system (connect/disconnect/reading) | P1 | [x] `EventDispatcher` |
| Data validation & bounds checking | P2 | [x] `ReadingValidator` |
| CO2 threshold helpers | P2 | [x] `Thresholds`, `Co2Level` |
| Connection/operation metrics | P2 | [x] `ConnectionMetrics` |
| Mock device for testing | P2 | [x] `MockDevice` |
| History V1 protocol (notification-based) | P2 | [x] For older devices |
| RSSI signal strength reading | P2 | [x] |

### Technical Requirements

> **Always check for latest versions before starting development.**

| Dependency | Purpose | Min Version | Check Latest |
|------------|---------|-------------|--------------|
| `btleplug` | Cross-platform BLE | 0.11+ | [crates.io/crates/btleplug](https://crates.io/crates/btleplug) |
| `tokio` | Async runtime | 1.0+ | [crates.io/crates/tokio](https://crates.io/crates/tokio) |
| `thiserror` | Error handling | 2.0+ | [crates.io/crates/thiserror](https://crates.io/crates/thiserror) |
| `bytes` | Binary parsing | 1.0+ | [crates.io/crates/bytes](https://crates.io/crates/bytes) |
| `uuid` | UUID handling | 1.0+ | [crates.io/crates/uuid](https://crates.io/crates/uuid) |
| `time` | Date/time | 0.3+ | [crates.io/crates/time](https://crates.io/crates/time) |
| `tracing` | Logging/diagnostics | 0.1+ | [crates.io/crates/tracing](https://crates.io/crates/tracing) |

### Key UUIDs (from Aranet4-Python docs)

```
Service UUIDs:
- NEW (v1.2.0+): 0000fce0-0000-1000-8000-00805f9b34fb
- OLD (pre-1.2.0): f0cd1400-95da-4f4b-9ac8-aa55d312af0c

Characteristic UUIDs:
- Current Readings:  f0cd3001-95da-4f4b-9ac8-aa55d312af0c
- History Records:   f0cd2001-95da-4f4b-9ac8-aa55d312af0c  
- History Command:   f0cd2002-95da-4f4b-9ac8-aa55d312af0c
- Interval:          f0cd1401-95da-4f4b-9ac8-aa55d312af0c
- Seconds Since:     f0cd1402-95da-4f4b-9ac8-aa55d312af0c
- Total Readings:    f0cd1403-95da-4f4b-9ac8-aa55d312af0c
- BT Range:          f0cd1406-95da-4f4b-9ac8-aa55d312af0c

Manufacturer ID: 0x0702 (SAF Tehnika)
```

---

## Phase 2: CLI Tool (v0.2.0)

### Milestone: Replace `aranetctl`

#### Core Commands

| Feature | Priority | Status |
|---------|----------|--------|
| `scan` - Discover devices | P0 | [x] Implemented |
| `read` - Current measurements | P0 | [x] Implemented |
| `history` - Download historical data | P0 | [x] Implemented |
| `info` - Device information | P0 | [x] Implemented |
| `status` - Quick one-line reading | P0 | [x] Implemented |
| `set` - Modify device settings | P1 | [x] Implemented (interval, range, smart_home) |
| `watch` - Continuous monitoring | P1 | [x] Implemented |
| `config` - Manage configuration | P1 | [x] Implemented |
| `completions` - Shell completions | P1 | [x] Implemented |
| `doctor` - Diagnose BLE/permission issues | P2 | [x] Implemented |
| `alias` - Save friendly device names | P2 | [x] Implemented |

#### Global Flags & Configuration

| Feature | Priority | Status |
|---------|----------|--------|
| `--quiet` flag | P1 | [x] Implemented |
| `--output` flag (file output) | P1 | [x] Implemented |
| `--json` global flag | P0 | [x] Implemented |
| `--no-color` flag (+ `NO_COLOR` env) | P1 | [x] Implemented |
| `--fahrenheit` / `--celsius` units | P1 | [x] Implemented |
| `--inhg` pressure unit (inches Hg) | P2 | [x] Implemented |
| `ARANET_DEVICE` env var | P0 | [x] Implemented |
| Config file (`~/.config/aranet/config.toml`) | P1 | [x] Implemented |
| Interactive device picker (when no device specified) | P1 | [x] Implemented |

#### Read Command Options

| Feature | Priority | Status |
|---------|----------|--------|
| `--passive` (read from advertisements only) | P1 | [x] Implemented |
| `--format` (text, json, csv) | P1 | [x] Implemented |

#### History Command Options

| Feature | Priority | Status |
|---------|----------|--------|
| `--since` / `--until` date filters | P1 | [x] Implemented |
| `--format` (text, json, csv) | P1 | [x] Implemented |
| `--count` limit records | P1 | [~] Defined, not wired |

#### Output & Export

| Feature | Priority | Status |
|---------|----------|--------|
| JSON output | P0 | [x] Implemented |
| CSV export | P0 | [x] Implemented |
| Colored output with CO₂ status indicators | P2 | [x] Implemented |
| Progress bars for history download | P2 | [x] Implemented |

#### Multi-Device Support

| Feature | Priority | Status |
|---------|----------|--------|
| Read from multiple devices | P1 | [x] Implemented |
| Device aliases (friendly names) | P2 | [x] Implemented |

### Future CLI Enhancements (Post-v0.2.0)

#### Integrations

| Feature | Priority | Notes |
|---------|----------|-------|
| `--mqtt` / MQTT broker publishing | P2 | Home Assistant, etc. |
| `--influx` InfluxDB line protocol | P2 | Time-series export |
| `--prometheus` metrics endpoint | P2 | Metrics scraping |
| `--webhook URL` POST readings | P2 | Custom integrations |

#### Alerts & Notifications

| Feature | Priority | Notes |
|---------|----------|-------|
| `alert` subcommand | P2 | Trigger on threshold breach |
| `--threshold` on watch (exit code) | P2 | For cron/scripts |
| System notifications (macOS/Linux) | P2 | Desktop alerts |

### CLI Dependencies (use latest versions)

| Dependency | Purpose | Check Latest |
|------------|---------|--------------|
| `clap` | Argument parsing (use v4+) | [crates.io/crates/clap](https://crates.io/crates/clap) |
| `clap_complete` | Shell completions | [crates.io/crates/clap_complete](https://crates.io/crates/clap_complete) |
| `serde` | Serialization | [crates.io/crates/serde](https://crates.io/crates/serde) |
| `serde_json` | JSON output | [crates.io/crates/serde_json](https://crates.io/crates/serde_json) |
| `csv` | CSV export | [crates.io/crates/csv](https://crates.io/crates/csv) |
| `owo-colors` | Terminal colors | [crates.io/crates/owo-colors](https://crates.io/crates/owo-colors) |
| `indicatif` | Progress bars | [crates.io/crates/indicatif](https://crates.io/crates/indicatif) |
| `dialoguer` | Interactive prompts | [crates.io/crates/dialoguer](https://crates.io/crates/dialoguer) |
| `directories` | Config file paths | [crates.io/crates/directories](https://crates.io/crates/directories) |
| `toml` | Config file parsing | [crates.io/crates/toml](https://crates.io/crates/toml) |

---

## Phase 3: TUI Dashboard (v0.3.0)

### Milestone: Real-time Terminal Monitoring

| Feature | Priority | Status |
|---------|----------|--------|
| Basic TUI framework | P0 | [x] App shell complete |
| Live sensor readings display | P0 | [ ] |
| Multi-device dashboard | P0 | [ ] |
| Historical chart (sparklines) | P1 | [ ] |
| CO2 status color coding (green/amber/red) | P1 | [ ] |
| Keyboard navigation | P1 | [~] Quit key works |
| Alert thresholds | P2 | [ ] |
| Data logging to file | P2 | [ ] |

### TUI Dependencies (use latest versions)

| Dependency | Purpose | Check Latest |
|------------|---------|--------------|
| `ratatui` | TUI framework (successor to tui-rs) | [crates.io/crates/ratatui](https://crates.io/crates/ratatui) |
| `crossterm` | Terminal backend | [crates.io/crates/crossterm](https://crates.io/crates/crossterm) |

> **Note**: Use `ratatui` NOT `tui` - it's the actively maintained fork.

---

## Phase 4: GUI Application (v0.4.0)

### Milestone: Native Desktop App

| Feature | Priority | Status |
|---------|----------|--------|
| Basic GUI framework | P0 | [x] App shell complete |
| Device discovery UI | P0 | [ ] |
| Real-time readings dashboard | P0 | [ ] |
| Historical data charts | P0 | [ ] |
| Multi-device management | P1 | [ ] |
| Settings configuration | P1 | [ ] |
| System tray / menubar icon | P2 | [ ] |
| Notifications for thresholds | P2 | [ ] |
| Cross-platform (macOS, Windows, Linux) | P0 | [ ] |

### GUI Framework Options (evaluate latest versions)

| Option | Pros | Cons | Check Latest |
|--------|------|------|--------------|
| `egui` + `eframe` | Easy, immediate mode, fast iteration | Less native look | [crates.io/crates/egui](https://crates.io/crates/egui) |
| `iced` | Elm-inspired, polished, reactive | Steeper learning curve | [crates.io/crates/iced](https://crates.io/crates/iced) |
| `tauri` | Web tech UI + Rust backend | Larger binary, WebView dependency | [tauri.app](https://tauri.app) |
| `dioxus` | React-like, multi-platform | Newer, less mature | [crates.io/crates/dioxus](https://crates.io/crates/dioxus) |

> **Recommendation**: Start with `egui` for rapid prototyping, consider `iced` or `tauri` for production polish.

---

## Phase 5: WASM Web App (v0.5.0)

### Milestone: Browser-Based Aranet Monitor

| Feature | Priority | Status |
|---------|----------|--------|
| Basic WASM module | P0 | [x] Init, greet, log work |
| Web Bluetooth device discovery | P0 | [ ] |
| Connect and read current values | P0 | [ ] |
| Real-time dashboard UI | P0 | [ ] |
| Historical data download | P1 | [ ] |
| PWA (installable, offline) | P1 | [ ] |
| Export to CSV | P2 | [ ] |
| Chart visualizations | P2 | [ ] |

### WASM Dependencies (use latest versions)

| Dependency | Purpose | Check Latest |
|------------|---------|--------------|
| `wasm-bindgen` | Rust/JS interop | [crates.io/crates/wasm-bindgen](https://crates.io/crates/wasm-bindgen) |
| `web-sys` | Web API bindings | [crates.io/crates/web-sys](https://crates.io/crates/web-sys) |
| `js-sys` | JS type bindings | [crates.io/crates/js-sys](https://crates.io/crates/js-sys) |
| `wasm-bindgen-futures` | Async/Promise interop | [crates.io/crates/wasm-bindgen-futures](https://crates.io/crates/wasm-bindgen-futures) |

### Frontend Framework Options (use latest versions)

| Option | Style | Check Latest |
|--------|-------|--------------|
| `yew` | React-like, mature | [crates.io/crates/yew](https://crates.io/crates/yew) |
| `leptos` | Signals-based, fast, SSR | [crates.io/crates/leptos](https://crates.io/crates/leptos) |
| `dioxus` | React-like, multi-platform | [crates.io/crates/dioxus](https://crates.io/crates/dioxus) |
| `sycamore` | Reactive, fine-grained | [crates.io/crates/sycamore](https://crates.io/crates/sycamore) |

> **Recommendation**: `leptos` is currently the most modern and performant choice for new WASM projects.

### Technical Approach

```
┌─────────────────────────────────────────────────────┐
│                   Browser (Chrome)                  │
├─────────────────────────────────────────────────────┤
│  ┌─────────────┐    ┌─────────────────────────────┐ │
│  │  Leptos/Yew │◄───│  aranet (WASM compiled)     │ │
│  │   Frontend  │    └──────────────┬──────────────┘ │
│  └─────────────┘                   │                │
│                                    ▼                │
│                    ┌───────────────────────────────┐│
│                    │   Web Bluetooth API (JS glue) ││
│                    └───────────────────────────────┘│
└─────────────────────────────────────────────────────┘
```

**Key Considerations:**

- Web Bluetooth only works in Chrome/Edge (~50% browser support)
- iOS Safari does NOT support Web Bluetooth (no workaround)
- Need `wasm-bindgen` for JS interop
- Consider `web-sys` for Web Bluetooth bindings

**Existing Reference**: [Sensor Pilot](https://github.com/kasparsd/sensor-pilot) - vanilla JS implementation (uses OLD UUID only, needs update)

---

## Phase 6: Data Persistence & API (v0.6.0)

### Milestone: Local Storage + HTTP API for Integrations

This phase adds two new crates for data persistence, caching, and external integrations.

### Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Database location | `~/.local/share/aranet/data.db` | XDG Base Directory spec compliance |
| History sync strategy | Incremental | Only download new records since last sync |
| API authentication | None | Local-first design; bind to localhost by default |
| Data retention | No pruning | Users manage their own data; no automatic deletion |

### aranet-store (Data Persistence)

| Feature | Priority | Status |
|---------|----------|--------|
| SQLite database with schema migrations | P0 | [x] |
| Store current readings with timestamps | P0 | [x] |
| Cache history records (avoid re-downloading) | P0 | [x] |
| Track sync state per device | P0 | [x] |
| Query by device, time range | P0 | [x] |
| Aggregate queries (min, max, avg) | P1 | [ ] |
| Export to CSV/JSON | P1 | [ ] |
| Import from CSV/JSON backup | P2 | [ ] |

### aranet-service (Background Collector + HTTP API)

| Feature | Priority | Status |
|---------|----------|--------|
| REST API endpoints | P0 | [ ] |
| Background device polling | P0 | [ ] |
| Configurable poll intervals per device | P0 | [ ] |
| WebSocket real-time updates | P1 | [ ] |
| Health check endpoint | P1 | [ ] |
| Foreground server mode | P0 | [ ] |
| Daemon mode (background service) | P1 | [ ] |
| systemd/launchd service files | P2 | [ ] |

### REST API Endpoints

```
GET  /api/health                     # Service health check
GET  /api/devices                    # List all known devices
GET  /api/devices/:id                # Get device info
GET  /api/devices/:id/current        # Latest reading for device
GET  /api/devices/:id/readings       # Query readings (?since, ?until, ?limit)
GET  /api/devices/:id/history        # Query cached history
POST /api/devices/:id/sync           # Trigger manual history sync
GET  /api/readings                   # All readings across devices (paginated)
WS   /api/ws                         # Real-time readings stream (WebSocket)
```

### CLI Integration

| Feature | Priority | Status |
|---------|----------|--------|
| `aranet server` - Start HTTP server | P0 | [ ] |
| `aranet server --daemon` - Background mode | P1 | [ ] |
| `aranet sync` - One-shot sync to database | P0 | [x] |
| `aranet sync --all` - Sync all configured devices | P0 | [ ] |
| `aranet cache` - Query cached data | P0 | [x] |
| `history --cache` - Read from local cache first | P1 | [ ] |

### Database Schema

```sql
-- Devices table
CREATE TABLE devices (
    id TEXT PRIMARY KEY,           -- device address/UUID
    name TEXT,
    device_type TEXT,              -- Aranet4, Aranet2, etc.
    serial TEXT,
    firmware TEXT,
    hardware TEXT,
    first_seen INTEGER NOT NULL,   -- Unix timestamp
    last_seen INTEGER NOT NULL
);

-- Current readings (polled values)
CREATE TABLE readings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    device_id TEXT NOT NULL REFERENCES devices(id),
    captured_at INTEGER NOT NULL,  -- Unix timestamp
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
CREATE INDEX idx_readings_device_time ON readings(device_id, captured_at);

-- History records (downloaded from device memory)
CREATE TABLE history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    device_id TEXT NOT NULL REFERENCES devices(id),
    timestamp INTEGER NOT NULL,    -- Reading timestamp from device
    synced_at INTEGER NOT NULL,    -- When we downloaded it
    co2 INTEGER,
    temperature REAL,
    pressure REAL,
    humidity INTEGER,
    radon INTEGER,
    radiation_rate REAL,
    radiation_total REAL,
    UNIQUE(device_id, timestamp)   -- Deduplicate by device + time
);
CREATE INDEX idx_history_device_time ON history(device_id, timestamp);

-- Sync state tracking (for incremental sync)
CREATE TABLE sync_state (
    device_id TEXT PRIMARY KEY REFERENCES devices(id),
    last_history_index INTEGER,    -- Last downloaded history index
    total_readings INTEGER,        -- Total readings on device at last sync
    last_sync_at INTEGER           -- When last synced
);
```

### Configuration (`~/.config/aranet/server.toml`)

```toml
# Server settings
[server]
bind = "127.0.0.1:8080"           # Listen address (localhost only by default)

# Database location
[storage]
path = "~/.local/share/aranet/data.db"

# Devices to monitor (background collector)
[[devices]]
address = "Aranet4 17C3C"
alias = "office"
poll_interval = 60                 # seconds between readings

[[devices]]
address = "AranetRn+ 306B8"
alias = "basement"
poll_interval = 300                # radon changes slowly
```

### Architecture

```
                      External Clients
        (Web apps, scripts, Home Assistant, Grafana)
                            |
                            | HTTP/WebSocket
                            v
+---------------------------------------------------------------+
|                      aranet-service                            |
|  +----------------+  +---------------+  +-------------------+  |
|  |   REST API     |  |   WebSocket   |  | Background        |  |
|  |   (axum)       |  |   (real-time) |  | Collector         |  |
|  +-------+--------+  +-------+-------+  +--------+----------+  |
+-----------|--------------------|-----------------|-------------+
            |                    |                 |
            v                    v                 v
+---------------------------------------------------------------+
|                       aranet-store                             |
|  +----------------------------------------------------------+  |
|  |  SQLite: ~/.local/share/aranet/data.db                   |  |
|  |  - devices, readings, history, sync_state                |  |
|  +----------------------------------------------------------+  |
+---------------------------------------------------------------+
                            ^
                            | (read/write)
+---------------------------------------------------------------+
|                       aranet-core                              |
|  +----------------+  +---------------+  +-------------------+  |
|  |   Device       |  |   History     |  |   DeviceManager   |  |
|  |   (BLE conn)   |  |   (download)  |  |   (multi-device)  |  |
|  +----------------+  +---------------+  +-------------------+  |
+---------------------------------------------------------------+
                            ^
                            | Bluetooth LE
                            v
+---------------------------------------------------------------+
|          Aranet4 / Aranet2 / AranetRn+ / Radiation            |
+---------------------------------------------------------------+
```

### Phase 6 Dependencies (use latest versions)

| Dependency | Purpose | Crate | Check Latest |
|------------|---------|-------|--------------|
| `rusqlite` | SQLite bindings | aranet-store | [crates.io/crates/rusqlite](https://crates.io/crates/rusqlite) |
| `refinery` | Schema migrations | aranet-store | [crates.io/crates/refinery](https://crates.io/crates/refinery) |
| `axum` | HTTP framework | aranet-service | [crates.io/crates/axum](https://crates.io/crates/axum) |
| `tower-http` | HTTP middleware | aranet-service | [crates.io/crates/tower-http](https://crates.io/crates/tower-http) |
| `tokio-tungstenite` | WebSocket | aranet-service | [crates.io/crates/tokio-tungstenite](https://crates.io/crates/tokio-tungstenite) |

---

## Existing Rust Crates Analysis

Before building, learn from existing implementations:

| Crate | Strengths | Weaknesses | btleplug Version |
|-------|-----------|------------|------------------|
| `aranet` (m1guelpf) | Clean API, modern deps, device info | No history, no scanning | 0.11.4 (current) |
| `aranet-btle` (DDRBoxman) | BLE advertisement scanning | No device info, no history | 0.11 (current) |
| `aranet4` (lpraneis) | Historical data support | Uses OLD UUID only! | 0.9.1 (outdated) |
| `aranet4-cli` (quentinms) | Multi-device, JSON output | CLI only, not a library | 0.9 (outdated) |

**Strategy**: Combine the best of all crates with latest dependencies:

- Clean API from `aranet`
- Advertisement scanning from `aranet-btle`
- History support from `aranet4`
- Multi-device from `aranet4-cli`

---

## Data Structures

### Current Reading (from Python reference)

```rust
pub struct CurrentReading {
    pub co2: u16,           // ppm
    pub temperature: f32,   // °C (raw / 20.0)
    pub pressure: u16,      // hPa (raw / 10)
    pub humidity: u8,       // %
    pub battery: u8,        // %
    pub status: Status,     // GREEN/AMBER/RED
    pub interval: Duration, // measurement interval
    pub age: Duration,      // time since last reading
}

pub enum Status {
    Green = 1,  // CO₂ < 1000 ppm
    Amber = 2,  // 1000-1400 ppm
    Red = 3,    // > 1400 ppm
}
```

### History Record

```rust
pub struct HistoryRecord {
    pub timestamp: DateTime<Utc>,
    pub co2: u16,
    pub temperature: f32,
    pub pressure: u16,
    pub humidity: u8,
}
```

### Device Info

```rust
pub struct DeviceInfo {
    pub name: String,
    pub model: String,
    pub serial: String,
    pub firmware: String,
    pub hardware: String,
    pub software: String,
    pub manufacturer: String,
}
```

---

## Project Structure

```
aranet/
├── Cargo.toml              # Workspace manifest
├── README.md
├── LICENSE
├── CHANGELOG.md
├── ROADMAP.md
├── .github/
│   └── workflows/
│       └── ci.yml          # GitHub Actions CI
├── crates/
│   ├── aranet-types/       # Platform-agnostic types (shared)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── types.rs    # CurrentReading, DeviceInfo, etc.
│   │   │   ├── uuid.rs     # GATT UUIDs
│   │   │   └── error.rs    # Parse errors
│   │   └── Cargo.toml
│   ├── aranet-core/        # Native BLE library (btleplug)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── device.rs   # Device connection
│   │   │   ├── readings.rs # Current readings
│   │   │   ├── history.rs  # Historical data
│   │   │   ├── settings.rs # Device configuration
│   │   │   ├── scan.rs     # Device discovery
│   │   │   └── error.rs    # BLE error types
│   │   ├── examples/
│   │   │   ├── read_sensor.rs
│   │   │   ├── scan_devices.rs
│   │   │   └── download_history.rs
│   │   └── Cargo.toml
│   ├── aranet-store/       # Data persistence layer (SQLite)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── store.rs    # Database operations
│   │   │   ├── models.rs   # Stored data types
│   │   │   ├── queries.rs  # Query builders
│   │   │   └── migrations/ # Schema migrations
│   │   └── Cargo.toml
│   ├── aranet-service/     # Background collector + HTTP API
│   │   ├── src/
│   │   │   ├── main.rs
│   │   │   ├── api.rs      # REST endpoints
│   │   │   ├── ws.rs       # WebSocket handler
│   │   │   ├── collector.rs # Background polling
│   │   │   └── config.rs   # Server configuration
│   │   └── Cargo.toml
│   ├── aranet-cli/         # CLI application
│   │   ├── src/main.rs
│   │   └── Cargo.toml
│   ├── aranet-tui/         # TUI dashboard
│   │   ├── src/main.rs
│   │   └── Cargo.toml
│   ├── aranet-gui/         # GUI application
│   │   ├── src/main.rs
│   │   └── Cargo.toml
│   └── aranet-wasm/        # WASM web app (Web Bluetooth)
│       ├── src/lib.rs
│       └── Cargo.toml
└── docs/
    ├── PROTOCOL.md
    └── UUIDs.md
```

---

## Development Phases Timeline

| Phase | Scope | Est. Time | Key Dependencies (latest) |
|-------|-------|-----------|---------------------------|
| 0 | Foundation | Done | README, LICENSE, CI, examples |
| 1 | Core Library | Done | btleplug, tokio, thiserror |
| 2 | CLI Tool | Done | Phase 1 + clap, serde |
| 3 | TUI Dashboard | 1-2 weeks | Phase 1 + ratatui, crossterm |
| 4 | GUI App | 2-3 weeks | Phase 1 + egui/iced |
| 5 | WASM Web | 2-3 weeks | aranet-types + wasm-bindgen |
| 6 | Data Persistence & API | 1-2 weeks | Phase 1 + rusqlite, axum |

---

## Testing Strategy

### Unit Tests

- **aranet-types**: Test data parsing, serialization, type conversions, proptest fuzz testing
- **aranet-core**: Test with mock BLE adapter, proptest for parsers
- **aranet-store**: Test database operations with in-memory SQLite
- **aranet-service**: Test API endpoints with mock store
- **aranet-gui**: Component tests for AppState
- **aranet-tui**: Component tests for App key handling
- Run with: `cargo test --workspace`

### Property-Based Testing (Proptest)

- Fuzz testing for all byte parsers to catch edge cases
- Tests that parsing never panics on arbitrary input
- Located in `proptests` modules within each crate
- Run with: `cargo test --workspace` (included in normal test run)

### Integration Tests

- Located in `crates/aranet-core/tests/`
- Require actual BLE hardware (marked with `#[ignore]`)
- Run with: `cargo test -- --ignored` (when hardware available)

### Code Coverage

- **cargo-llvm-cov** integrated into CI workflow
- Coverage reports uploaded to Codecov automatically
- Run locally with: `cargo llvm-cov --workspace`

### CI Testing

- GitHub Actions runs on every PR
- Tests on: Ubuntu, macOS, Windows
- Linting: `cargo clippy`, `cargo fmt --check`
- Coverage: cargo-llvm-cov with Codecov upload
- No hardware tests in CI (no BLE adapter)

### Hardware Testing Checklist

When testing with real Aranet devices:

- [ ] Aranet4 (new firmware v1.2.0+)
- [ ] Aranet4 (old firmware pre-1.2.0)
- [ ] Aranet2 (if available)
- [ ] Multiple devices simultaneously
- [ ] Connection stability over time
- [ ] Reconnection after disconnect

---

## Tooling Requirements

> **Always use latest stable versions of all tools.**

| Tool | Purpose | Install/Check |
|------|---------|---------------|
| Rust | Compiler | `rustup update stable` |
| cargo | Package manager | Comes with Rust |
| wasm-pack | WASM builds | `cargo install wasm-pack` |
| trunk | WASM dev server | `cargo install trunk` |
| cargo-watch | Auto-rebuild | `cargo install cargo-watch` |
| cargo-audit | Security audit | `cargo install cargo-audit` |
| cargo-outdated | Dep version check | `cargo install cargo-outdated` |

### Keeping Dependencies Updated

```bash
# Check for outdated dependencies
cargo outdated

# Update all dependencies to latest compatible versions
cargo update

# Audit for security vulnerabilities
cargo audit
```

---

## Rust Edition & MSRV

- **Rust Edition**: 2024 (or latest stable)
- **MSRV**: Latest stable (don't artificially constrain)

```toml
# Cargo.toml
[package]
edition = "2024"
rust-version = "1.90"  # Updated Jan 2026
```

---

## Resources

### Protocol Documentation

- [Aranet4-Python UUIDs](https://github.com/Anrijs/Aranet4-Python/blob/master/docs/UUIDs.md)
- [Aranet4-Python client.py](https://github.com/Anrijs/Aranet4-Python/blob/master/aranet4/client.py)

### Rust BLE

- [btleplug docs](https://docs.rs/btleplug)
- [btleplug examples](https://github.com/deviceplug/btleplug/tree/master/examples)

### Existing Implementations

- [aranet](https://github.com/m1guelpf/aranet) - cleanest API
- [aranet-btle](https://github.com/DDRBoxman/aranet-btle) - advertisement scanning
- [aranet4-rs](https://github.com/lpraneis/aranet4-rs) - history support

### Web Bluetooth

- [Sensor Pilot](https://github.com/kasparsd/sensor-pilot) - JS reference
- [Web Bluetooth API](https://developer.mozilla.org/en-US/docs/Web/API/Web_Bluetooth_API)

---

## Success Metrics

- [ ] All Python library features working in Rust
- [ ] Single binary CLI with no runtime dependencies
- [ ] TUI that can monitor multiple devices simultaneously
- [ ] GUI app installable on macOS, Windows, Linux
- [ ] WASM app deployed to GitHub Pages
- [ ] Published to crates.io as `aranet`
- [ ] All dependencies on latest stable versions
- [ ] Zero `cargo audit` warnings

---

## License

MIT (matching Aranet4-Python)
