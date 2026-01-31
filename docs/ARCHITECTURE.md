# Aranet Architecture

Technical architecture and reference documentation for the Aranet Rust implementation.
This project provides feature parity with [Aranet4-Python](https://github.com/Anrijs/Aranet4-Python) and beyond.

> **Dependency Policy**: Always use the **latest stable versions** of all libraries, frameworks, and tools.
> Pin to major versions only (e.g., `btleplug = "0.11"` not `"0.11.4"`). Run `cargo update` regularly.
> Check [crates.io](https://crates.io) and [lib.rs](https://lib.rs) for current versions before adding dependencies.

---

## Current Progress (Updated Jan 30, 2026)

| Phase | Component | Status | Progress |
|-------|-----------|--------|----------|
| 0 | Foundation | Done | README, LICENSE, CI, CHANGELOG, aranet-types |
| 1 | Core Library | Done | Full BLE: scan, connect, read, history, settings - tested with real hardware |
| 2 | CLI Tool | Done | All core commands: scan, read, status, info, history, set, watch, config |
| 3 | TUI Dashboard | Done | Full dashboard with tabs, sparklines, help overlay, multi-device support |
| 4 | GUI Application | Done | Full MVP: device scan, connect, real-time readings with color coding |
| 5 | Data Persistence & API | Done | aranet-store complete; aranet-service complete |
| 6 | Unified Data Architecture | Done | Shared database across all tools; auto-connect; auto-sync |

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

- **GUI enhancements**: Auto-connect/sync preferences UI, data export settings, multiple metrics overlay chart, comparison view
- **GUI polish**: Escape key closes dialogs, notification sound toggle, Do Not Disturb mode, launch minimized, compact mode
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

1. ~~Add sensor data display to TUI shell~~ - Complete (v0.3.0)
2. ~~**TUI Polish**: Auto-refresh, trend indicators, scrollable history, settings editing~~ - Complete (v0.1.8)
3. ~~Add sensor data display to GUI shell~~ - Complete (v0.4.0)
4. ~~**Unified Data Architecture**~~ - All tools share same database, auto-connect, auto-sync - Complete
5. ~~**Data persistence layer (aranet-store)**~~ - Complete (v0.1.7)
6. ~~**Background service (aranet-service)**~~ - Complete (v0.1.8)

## Vision

Build the definitive Rust ecosystem for Aranet devices:

- **aranet-types** - Platform-agnostic data types (shared by all crates)
- **aranet-core** - Native BLE client via btleplug
- **aranet-store** - Local data persistence with SQLite
- **aranet-service** - Background data collector and HTTP REST API
- **aranet-cli** - Feature-complete command-line interface
- **aranet-tui** - Real-time terminal UI for monitoring
- **aranet-gui** - Native desktop app (egui-based)

---

## Unified Data Architecture

All tools (CLI, TUI, GUI, Server) **MUST** share the same database and data source (`aranet-store`).
This ensures a consistent experience across all interfaces with no data silos.

### Core Principles

| Principle | Description |
|-----------|-------------|
| **Shared Database** | All tools read/write to the same SQLite database at platform-specific location |
| **Device Memory** | All previously connected devices are remembered and loaded on startup |
| **Auto-Connect** | Connections to known devices are automatic by default (configurable) |
| **Auto-Sync History** | History is downloaded automatically on first connection and synced on subsequent loads |
| **Feature Parity** | All tool versions (CLI, TUI, GUI, Server) should implement equivalent functionality |

### Automatic Behavior (Defaults)

| Behavior | On Startup | On Connection | Configurable |
|----------|------------|---------------|--------------|
| Load known devices from database | Yes | N/A | Yes |
| Auto-connect to previously paired devices | Yes | N/A | Yes |
| Download full history on first connection | N/A | Yes | Yes |
| Incremental sync on reconnection | N/A | Yes | Yes |
| Cache current readings to database | N/A | Yes | Yes |

### Implementation Status

| Tool | Uses aranet-store | Loads Devices | Auto-Connect | Auto-Sync | Feature Parity |
|------|-------------------|---------------|--------------|-----------|----------------|
| CLI | [x] All commands save to store | [x] Fallback to store | N/A (interactive) | [x] Via sync command | Reference |
| TUI | [x] | [x] LoadCachedData | [x] | [x] | [x] |
| GUI | [x] | [x] CachedDataLoaded | [x] | [x] | [x] |
| Server | [x] | [x] | [x] | [x] | [x] |

### Required Changes for Full Parity

#### CLI (aranet-cli)

- [x] Load known devices from database on startup (fallback when no device specified)
- [x] Auto-connect to default/saved device if no `--device` specified
- [x] Save device to database after successful connection
- [x] Save readings to database after read/status/watch commands
- [x] Save history to database after history command
- [x] Full sync via `aranet sync` command with incremental support

#### TUI (aranet-tui)

- [x] Load cached devices on startup (via `LoadCachedData` command)
- [x] Auto-connect to previously connected devices on startup
- [x] Auto-sync history on successful connection
- [x] Store readings to database

#### GUI (aranet-gui)

- [x] Initialize aranet-store on startup
- [x] Load known devices from database on startup
- [x] Auto-connect to previously connected devices on startup
- [x] Auto-sync history on successful connection
- [x] Store readings to database
- [x] Show last-known readings even when device is offline

### Database Location

All tools use the same database file determined by `aranet_store::default_db_path()`:

| Platform | Path |
|----------|------|
| Linux | `~/.local/share/aranet/data.db` |
| macOS | `~/Library/Application Support/aranet/data.db` |
| Windows | `C:\Users\<user>\AppData\Local\aranet\data.db` |

### Configuration (`~/.config/aranet/config.toml`)

```toml
[behavior]
auto_connect = true       # Auto-connect to known devices on startup
auto_sync = true          # Auto-sync history on connection
remember_devices = true   # Save devices to database after connection

[database]
path = ""                 # Empty = use default platform path
```

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
| `--count` limit records | P1 | [x] Implemented |

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
| Basic TUI framework | P0 | [x] App shell, event loop, terminal setup |
| Live sensor readings display | P0 | [x] Dashboard panel with color-coded readings |
| Multi-device dashboard | P0 | [x] Device list sidebar with connection status |
| Historical chart (sparklines) | P1 | [x] Sparklines in dashboard and history tab |
| CO2 status color coding (green/amber/red) | P1 | [x] Full color coding for CO2, radon, battery |
| Keyboard navigation | P1 | [x] Full keybindings: q/s/r/c/d/y/↑↓/Tab/? |
| Tab system (Dashboard/History/Settings) | P1 | [x] Three tabs with dedicated content |
| Help overlay | P1 | [x] Press ? for keyboard shortcuts |
| Background worker | P0 | [x] Async BLE operations with channels |
| RefreshAll command | P1 | [x] Refresh readings from all devices |
| Disconnect command | P1 | [x] Disconnect from device |
| Alert thresholds | P2 | [x] CO2 threshold alerts with visual banner |
| Data logging to file | P2 | [x] CSV logging with L key toggle, REC indicator |

### TUI Enhancement Roadmap

The current TUI is functional but basic. Below are planned enhancements organized by priority:

#### Visual & Layout Improvements

| Feature | Priority | Status | Description |
|---------|----------|--------|-------------|
| Header bar with app title | P2 | [x] | Show "Aranet Monitor v0.3.0" with connected device count |
| Wider device list option | P2 | [x] | Toggle width with ']' key (28/40 chars) |
| Trend indicators | P1 | [x] | Show ↑↓→ arrows next to readings based on recent history |
| Reading cards with borders | P2 | [x] | Grid of bordered cards for CO2/Temp/Humidity/etc |
| Better sparkline labels | P2 | [x] | Min/max on Y-axis, timestamps on X-axis |
| Responsive layout | P2 | [x] | Auto-hide sidebar on narrow (<80 cols), toggle with '[' |
| Theme support | P3 | [x] | Light/dark theme with 't' key (☀/☽ indicator) |

#### Data Display Enhancements

| Feature | Priority | Status | Description |
|---------|----------|--------|-------------|
| Min/Max/Avg stats | P1 | [x] | Show session statistics (min, max, avg CO2) in readings panel |
| Radon 1-day/7-day averages | P1 | [x] | Display 24h and 7d radon averages for radon devices |
| RSSI signal strength | P2 | [x] | Show BLE signal strength bars for connected devices |
| Device uptime | P3 | [x] | Show uptime in device list and settings |
| Last sync timestamp | P2 | [x] | Show when history was last synced in History tab |
| Reading age warning | P1 | [x] | Highlight if reading is stale (> 2x interval) |

#### History Tab Improvements

| Feature | Priority | Status | Description |
|---------|----------|--------|-------------|
| Scrollable history list | P1 | [x] | Scroll through all history records with PgUp/PgDn |
| Time range filter | P1 | [x] | Filter by 0=all, 1=today, 2=24h, 3=7d, 4=30d |
| Export from TUI | P2 | [x] | Export visible history to CSV with 'e' key |
| Larger chart view | P2 | [x] | Full-screen sparkline with 'g' key |
| Multiple metrics chart | P2 | [x] | Stacked temp/humidity with T/H keys |

#### Settings Tab Improvements

| Feature | Priority | Status | Description |
|---------|----------|--------|-------------|
| Edit measurement interval | P1 | [x] | Change device interval with Enter key (1, 2, 5, 10 min) |
| Edit Bluetooth range | P2 | [x] | Toggle standard/extended with 'B' key |
| Device alias/rename | P2 | [x] | Set friendly name with 'n' key |
| Alert threshold config | P1 | [x] | Customize CO2/radon alert thresholds with +/- keys |
| Toggle Smart Home mode | P3 | [x] | Toggle with 'I' key, 🏠 indicator in header |

#### UX Improvements

| Feature | Priority | Status | Description |
|---------|----------|--------|-------------|
| Auto-refresh readings | P0 | [x] | Automatically poll connected devices on interval |
| Loading spinners | P1 | [x] | Show spinners during connect/sync operations |
| Status message queue | P2 | [x] | Queue multiple messages, auto-dismiss after 5s timeout |
| Confirmation dialogs | P2 | [x] | Confirm before disconnect with Y/N prompt |
| Mouse support | P2 | [x] | Click to select device, tabs, buttons |
| Shift+Tab for prev tab | P1 | [x] | Backward tab navigation (currently only forward) |
| Battery low warning | P1 | [x] | Alert when battery drops below 20% |
| Error details popup | P2 | [x] | Full error with 'E' key, ❌ indicator in status bar |

#### Multi-Device Features

| Feature | Priority | Status | Description |
|---------|----------|--------|-------------|
| Device filter by type | P2 | [x] | Filter by Aranet4/Radon/Radiation/Connected with 'f' key |
| Device filter by status | P2 | [x] | Included in device filter (Connected filter option) |
| Comparison view | P2 | [x] | Side-by-side readings with 'v' key, '</>' to cycle |
| Connect all | P2 | [x] | Connect to all known devices with 'C' (Shift+c) |
| Broadcast refresh | P1 | [x] | Refresh all connected devices (implemented) |

#### Notifications & Alerts

| Feature | Priority | Status | Description |
|---------|----------|--------|-------------|
| Terminal bell on alert | P2 | [x] | Beep when CO2/radon exceeds threshold ('b' to toggle) |
| Alert history log | P2 | [x] | View past alerts with timestamps using 'a' key |
| Sticky alerts | P2 | [x] | Toggle with 'A' key, shows [STICKY] in header |
| Alert severity levels | P2 | [x] | Info (blue ℹ), Warning (yellow ⚠), Critical (red 🚨) |

### TUI Dependencies (use latest versions)

| Dependency | Purpose | Check Latest |
|------------|---------|--------------|
| `ratatui` | TUI framework (successor to tui-rs) | [crates.io/crates/ratatui](https://crates.io/crates/ratatui) |
| `crossterm` | Terminal backend | [crates.io/crates/crossterm](https://crates.io/crates/crossterm) |

> **Note**: Use `ratatui` NOT `tui` - it's the actively maintained fork.

---

## Phase 4: GUI Application (v0.4.0)

### Milestone: Native Desktop App

**MVP Scope**: Device scan + current readings display + history charts + settings view.

| Feature | Priority | Status | Notes |
|---------|----------|--------|-------|
| Basic GUI framework | P0 | [x] | egui shell complete |
| Device discovery UI | P0 | [x] | Device list sidebar + Scan button |
| Real-time readings dashboard | P0 | [x] | CO2/temp/humidity/pressure with color coding |
| Cross-platform (macOS, Windows, Linux) | P0 | [~] | egui handles; needs Windows/Linux testing |
| Multi-device management | P1 | [x] | Device selector sidebar with status |
| Historical data charts | P1 | [x] | egui_plot charts for CO2, radon, temp, humidity |
| Settings configuration | P1 | [x] | Full read/write: interval, Smart Home, Bluetooth Range |
| Auto-refresh | P1 | [x] | Automatic polling at device interval |
| System tray / menubar icon | P2 | [x] | tray-icon with dynamic CO2 colors, close-to-tray |
| Notifications for thresholds | P2 | [x] | notify-rust for CO2 threshold alerts |

### Architecture Notes

The GUI must use the same worker/channel pattern as the TUI to keep BLE operations off the UI thread.
egui's immediate-mode rendering requires non-blocking operations.

```
+------------------+     Command      +------------------+
|    egui UI       | --------------> |  SensorWorker    |
|  (main thread)   |                 |  (tokio runtime) |
|                  | <-------------- |                  |
+------------------+   SensorEvent   +------------------+
                                              |
                                              v
                                     +------------------+
                                     |   aranet-core    |
                                     |   (BLE ops)      |
                                     +------------------+
```

**Shared Message Types**: The `Command` and `SensorEvent` enums are defined in `aranet-core::messages`
and re-exported by both TUI (`aranet-cli::tui::messages`) and GUI applications. This ensures
consistent message definitions for `Scan`, `Connect`, `Disconnect`, `RefreshReading`, `SyncHistory`, etc.

### Implementation Plan

| Week | Focus | Deliverables | Status |
|------|-------|--------------|--------|
| 1 | Worker architecture | Tokio runtime integration, channel setup, scan command | [x] Complete |
| 2 | Readings dashboard | Device list, current readings display, CO2 color coding | [x] Complete |
| 3 | Polish + testing | Multi-device, loading states, error handling, tests | [x] Complete |
| 4 | History + Settings | Tab system, egui_plot charts, settings display | [x] Complete |

**P1 Complete**: The GUI supports device scanning, connection, real-time readings with CO2 color coding,
historical data charts (CO2, radon, radiation, temperature, humidity), and full settings configuration.
Features include Dashboard/History/Settings tabs, time filtering (All/24h/7d/30d), multi-device support,
measurement interval editing, Smart Home and Bluetooth Range toggles, and auto-refresh.
Run with `cargo run --package aranet-gui`.

### GUI Enhancement Roadmap

The current GUI is functional but has room for polish. Below are planned enhancements organized by priority:

#### Visual & Layout Improvements

| Feature | Priority | Status | Description |
|---------|----------|--------|-------------|
| Responsive window sizing | P2 | [x] | Remember window size/position between sessions |
| Collapsible sidebar | P2 | [x] | Toggle device list visibility for more chart space (`[` key) |
| Compact mode | P3 | [x] | Dense layout option for smaller screens |
| Custom color themes | P3 | [ ] | User-defined color schemes beyond light/dark |
| Reading trend indicators | P2 | [x] | Show ↑↓→ arrows next to readings based on recent history |
| Device status badges | P2 | [x] | Visual badges for battery low, stale readings |

#### Settings Tab Improvements

| Feature | Priority | Status | Description |
|---------|----------|--------|-------------|
| Edit measurement interval | P1 | [x] | Change device interval (1, 2, 5, 10 min) |
| Edit Bluetooth range | P1 | [x] | Toggle standard/extended range |
| Toggle Smart Home mode | P1 | [x] | Enable/disable Smart Home integrations |
| Device alias/rename | P2 | [x] | Set friendly name for devices |
| Alert threshold config | P1 | [x] | Customize CO2/radon alert thresholds with sliders |
| Auto-connect preferences | P2 | [x] | Configure auto-connect, auto-sync, remember devices, load cache |
| Notification preferences | P2 | [x] | Notifications enabled toggle, sound toggle, Do Not Disturb |
| Data export settings | P2 | [x] | Default export format (CSV/JSON) and location |
| Temperature unit toggle | P2 | [x] | Switch between Celsius and Fahrenheit |
| Pressure unit toggle | P2 | [x] | Switch between hPa and inHg |

#### History Tab Improvements

| Feature | Priority | Status | Description |
|---------|----------|--------|-------------|
| Time range filter | P1 | [x] | Filter by All/24h/7d/30d |
| Export to CSV/JSON | P2 | [x] | Export visible history with button click |
| Multiple metrics overlay | P2 | [x] | Toggle to show temp/humidity on same chart |
| Date range picker | P2 | [x] | Custom date range selection |
| Sync status indicator | P2 | [x] | Show last sync timestamp and progress |

#### Multi-Device Features

| Feature | Priority | Status | Description |
|---------|----------|--------|-------------|
| Device filter by type | P2 | [x] | Filter sidebar by Aranet4/Radon/Radiation |
| Device filter by status | P2 | [x] | Show only connected or disconnected devices |
| Comparison view | P2 | [x] | Side-by-side readings from multiple devices with Compare button |
| Bulk actions | P2 | [x] | Connect/disconnect/refresh all devices at once |
| Device grouping | P3 | [ ] | Organize devices into custom groups (e.g., "Office", "Home") |

#### Notifications & Alerts

| Feature | Priority | Status | Description |
|---------|----------|--------|-------------|
| Threshold notifications | P2 | [x] | System notifications when CO2/radon exceeds limit |
| Alert history log | P2 | [x] | View past alerts with timestamps in app |
| Alert severity levels | P2 | [x] | Info/Warning/Critical with different notification styles |
| Notification sound toggle | P2 | [x] | Enable/disable alert sounds |
| Do Not Disturb mode | P3 | [x] | Temporarily suppress all notifications |

#### System Tray Enhancements

| Feature | Priority | Status | Description |
|---------|----------|--------|-------------|
| Dynamic CO2 color icon | P2 | [x] | Tray icon color reflects CO2 status |
| Close to tray | P2 | [x] | Window closes to tray instead of quitting |
| Tray context menu | P2 | [x] | Quick actions: Scan, Refresh All, Open Settings |
| Current reading tooltip | P2 | [x] | Hover tray icon to see current CO2/temp |
| Launch minimized option | P2 | [x] | Start app minimized to tray on login |
| Quick device switcher | P3 | [ ] | Select device to display in tray from menu |

#### Keyboard Shortcuts

| Shortcut | Action | Status |
|----------|--------|--------|
| `Cmd+R` | Refresh all connected devices | [x] |
| `Cmd+S` | Sync history for selected device | [x] |
| `Cmd+,` | Open settings tab | [x] |
| `1/2/3/4` | Switch to Dashboard/History/Settings/Service tab | [x] |
| `Cmd+↑/↓` | Navigate device list | [x] |
| `[` | Toggle sidebar collapse | [x] |
| `T` | Toggle dark/light theme | [x] |
| `A` | Toggle auto-refresh | [x] |
| `F5` | Scan for devices | [x] |
| `Ctrl+E` / `Cmd+E` | Export data | [x] |
| `Escape` | Close dialogs/popups | [x] |

#### Configuration Integration

The GUI reads and writes settings from the unified config file at `~/.config/aranet/config.toml`:

```toml
[behavior]
auto_connect = true       # Auto-connect to known devices on startup
auto_sync = true          # Auto-sync history on connection
remember_devices = true   # Save devices to database after connection

[gui]
theme = "system"          # "light", "dark", or "system"
start_minimized = false   # Launch minimized to system tray
show_tray_icon = true     # Show system tray icon
temperature_unit = "C"    # "C" or "F"
pressure_unit = "hPa"     # "hPa" or "inHg"

[alerts]
co2_warning = 1000        # CO2 warning threshold (ppm)
co2_critical = 1400       # CO2 critical threshold (ppm)
radon_warning = 100       # Radon warning threshold (Bq/m³)
notifications = true      # Enable system notifications
sound = true              # Play sound on alerts
```

### AppState Design

```rust
pub struct AppState {
    // Device management
    pub devices: Vec<DeviceState>,
    pub selected_device: Option<usize>,
    pub scanning: bool,

    // Worker communication (initialized at startup)
    pub command_tx: mpsc::Sender<Command>,

    // UI state
    pub active_view: View, // Dashboard | History | Settings
    pub error_message: Option<String>,
    pub status_message: String,
}
```

**Key considerations**:

- `command_tx` is stored in AppState; events are polled each frame via `try_recv()`
- Tokio runtime runs in a separate thread, spawned at app startup
- Use `Arc<Mutex<>>` sparingly; prefer message passing

### GUI Dependencies

| Dependency | Purpose | Check Latest |
|------------|---------|--------------|
| `egui` | Immediate-mode GUI | [crates.io/crates/egui](https://crates.io/crates/egui) |
| `eframe` | Native window wrapper | [crates.io/crates/eframe](https://crates.io/crates/eframe) |
| `egui_plot` | Charts (for v0.4.1) | [crates.io/crates/egui_plot](https://crates.io/crates/egui_plot) |
| `tokio` | Async runtime | [crates.io/crates/tokio](https://crates.io/crates/tokio) |

### GUI Framework Options (for reference)

| Option | Pros | Cons | Check Latest |
|--------|------|------|--------------|
| `egui` + `eframe` | Easy, immediate mode, fast iteration | Less native look | [crates.io/crates/egui](https://crates.io/crates/egui) |
| `iced` | Elm-inspired, polished, reactive | Steeper learning curve | [crates.io/crates/iced](https://crates.io/crates/iced) |
| `tauri` | Web tech UI + Rust backend | Larger binary, WebView dependency | [tauri.app](https://tauri.app) |
| `dioxus` | React-like, multi-platform | Newer, less mature | [crates.io/crates/dioxus](https://crates.io/crates/dioxus) |

> **Decision**: Using `egui` + `eframe` for v0.4.0. Fast iteration, good cross-platform support.

---

## Phase 5: Data Persistence & API (v0.5.0)

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
| Aggregate queries (min, max, avg) | P1 | [x] |
| Export to CSV/JSON | P1 | [x] |
| Import from CSV/JSON backup | P2 | [x] |

### aranet-service (Background Collector + HTTP API)

| Feature | Priority | Status |
|---------|----------|--------|
| REST API endpoints | P0 | [x] |
| Background device polling | P0 | [x] |
| Configurable poll intervals per device | P0 | [x] |
| WebSocket real-time updates | P1 | [x] |
| Health check endpoint | P1 | [x] |
| Foreground server mode | P0 | [x] |
| Daemon mode (background service) | P1 | [x] |
| systemd/launchd service files | P2 | [x] |

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
| `aranet server` - Start HTTP server | P0 | [x] |
| `aranet server --daemon` - Background mode | P1 | [x] |
| `aranet sync` - One-shot sync to database | P0 | [x] |
| `aranet sync --all` - Sync all configured devices | P0 | [x] |
| `aranet cache` - Query cached data | P0 | [x] |
| `history --cache` - Read from local cache first | P1 | [x] |

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

### Phase 5 Dependencies (use latest versions)

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
├── docs/ARCHITECTURE.md
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
│   └── aranet-gui/         # GUI application
│       ├── src/main.rs
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
| 3 | TUI Dashboard | Done | Phase 1 + ratatui, crossterm |
| 4 | GUI App | Done | Phase 1 + egui/iced |
| 5 | Data Persistence & API | Done | Phase 1 + rusqlite, axum |
| 6 | Unified Data Architecture | Done | Shared database, auto-connect, auto-sync |

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

---

## Success Metrics

- [x] All Python library features working in Rust
- [x] Single binary CLI with no runtime dependencies
- [x] TUI that can monitor multiple devices simultaneously
- [x] GUI app installable on macOS, Windows, Linux
- [x] Published to crates.io as `aranet`
- [x] All dependencies on latest stable versions
- [x] Zero `cargo audit` vulnerabilities (10 allowed warnings for unmaintained GTK3 transitive deps)

---

## License

MIT (matching Aranet4-Python)

---

Made with ❤️ by [Cameron Rye](https://rye.dev/)
