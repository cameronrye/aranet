# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.11] - 2026-01-31

### Added

- **MQTT Integration** - Publish sensor readings to MQTT brokers for IoT integration
  - Real-time publishing of all sensor readings to configurable topics
  - Topic structure: `{prefix}/{device}/co2`, `/temperature`, `/humidity`, `/pressure`, `/battery`, `/status`, `/json`
  - Support for radon (`/radon`) and radiation (`/radiation_rate`, `/radiation_total`) when available
  - TLS support via `mqtts://` scheme
  - Configurable QoS levels (0, 1, 2), retain flag, and authentication
  - Automatic reconnection with 5-second retry intervals
  - Feature flag: `mqtt` (included in `full` feature)

- **Prometheus Metrics Endpoint** - Export sensor data for monitoring and alerting
  - `/metrics` endpoint with Prometheus text format (0.0.4)
  - Sensor metrics: `aranet_co2_ppm`, `aranet_temperature_celsius`, `aranet_humidity_percent`, `aranet_pressure_hpa`, `aranet_battery_percent`, `aranet_reading_age_seconds`
  - Radon/radiation metrics when available: `aranet_radon_bqm3`, `aranet_radiation_rate_usvh`, `aranet_radiation_total_msv`
  - Collector statistics: `aranet_collector_running`, `aranet_collector_uptime_seconds`, `aranet_device_poll_success_total`, `aranet_device_poll_failure_total`
  - Optional push gateway support with configurable interval
  - Feature flag: `prometheus` (included in `full` feature)

- **Service Management Commands** - Control aranet-service from CLI
  - `aranet-service service install [--user]` - Install as system/user service
  - `aranet-service service uninstall [--user]` - Uninstall service
  - `aranet-service service start/stop/status [--user]` - Control service

- **REST API Enhancements**
  - `POST /api/collector/start` and `POST /api/collector/stop` - Control background collector
  - `GET /api/status` - Full service status with collector state and device statistics
  - `GET/PUT /api/config` - Runtime configuration management
  - `POST/PUT/DELETE /api/config/devices` - Dynamic device management

- **GUI Improvements**
  - Multi-panel architecture: Device List, Device Detail, History, Comparison, Alerts, Service, Settings
  - Export functionality (CSV/JSON) for historical data
  - Comparison view for side-by-side multi-device analysis
  - Alert system with threshold management and alert history
  - Service panel for background collector control
  - macOS native menu bar integration
  - System tray with quick access to readings
  - Light/dark theme support

- **TUI Service Tab** - Monitor and control aranet-service from terminal dashboard
  - Service status display with collector state and uptime
  - Per-device polling statistics
  - Start/stop collector controls

- **Service Client Library** - `service-client` feature in aranet-core
  - Type-safe HTTP client for aranet-service API
  - Service status, collector control, and configuration management

- Downloads page on website with direct download links for all platforms
- New icon assets with multiple resolutions (16px to 1024px)
- `docs/ARCHITECTURE.md` with comprehensive technical documentation
- Troubleshooting guide on website
- Entitlements for Bluetooth access on macOS GUI app

### Changed

- Updated screenshots in README and website
- Improved CI workflow for screenshots and site deployment
- Synchronized all crate versions to 0.1.11

### Removed

- `aranet-wasm` crate removed from workspace (was not published)
- `ROADMAP.md` removed in favor of website roadmap page

### Fixed

- Icon asset included in aranet-cli for crates.io publishing
- Screenshots and deploy-site workflow failures
- Removed references to non-existent CLI flags from troubleshooting documentation

## [0.1.9] - 2026-01-22

### Changed

- Replaced unmaintained `atty` crate with `std::io::IsTerminal` (resolves RUSTSEC-2024-0375)
- Updated `rusqlite` from 0.33 to 0.35
- Updated multiple dependencies to latest versions
- Marked `aranet-gui` and `aranet-wasm` as `publish = false` (not ready for release)

### Added

- CONTRIBUTING.md with contribution guidelines
- SECURITY.md with vulnerability reporting process
- GitHub issue templates for bug reports and feature requests
- Pull request template

## [0.1.8] - 2026-01-19

### Added

- **TUI Dashboard Enhancements** - Complete overhaul with 44 new features
  - **Navigation**: Tab/Shift+Tab for tabs, j/k/arrows for devices, vim-style keybindings
  - **Auto-refresh**: Readings update automatically based on device interval
  - **Trend indicators**: Up/down/stable arrows next to readings
  - **Statistics**: Min/Max/Avg CO2 stats, radon 1-day/7-day averages
  - **Alerts**: CO2/radon threshold alerts with Info/Warning/Critical severity levels
  - **Alert history**: View past alerts with 'a' key, sticky alerts with 'A'
  - **Terminal bell**: Audio alert on threshold breach (toggle with 'b')
  - **Sparkline charts**: CO2/radon history with min/max labels and time axis
  - **Full-screen chart**: Press 'g' for expanded chart view
  - **Multiple metrics**: Stack temperature/humidity on chart with T/H keys
  - **Time range filter**: 0=all, 1=today, 2=24h, 3=7d, 4=30d
  - **Scrollable history**: PgUp/PgDn to scroll through records
  - **Export history**: Press 'e' to export visible history to CSV
  - **Device filter**: Cycle filter with 'f' (All/Aranet4/Radon/Radiation/Connected)
  - **Comparison view**: Side-by-side device readings with 'v', cycle with '<'/'>'
  - **Connect all**: Connect to all known devices with 'C'
  - **Device alias**: Set friendly names with 'n' key
  - **Settings editing**: Change interval with Enter, BLE range with 'B', Smart Home with 'I'
  - **Threshold config**: Adjust CO2/radon thresholds with +/- keys
  - **Theme support**: Toggle light/dark theme with 't' key
  - **Responsive layout**: Auto-hide sidebar on narrow terminals, toggle with '['
  - **Wider sidebar**: Toggle sidebar width with ']' key
  - **Mouse support**: Click to select devices, tabs, and buttons
  - **RSSI signal strength**: Visual signal bars for connected devices
  - **Device uptime**: Shows how long device has been connected
  - **Battery warning**: Alert when battery drops below 20%
  - **Reading age warning**: Highlight stale readings (> 2x interval)
  - **Loading spinners**: Visual feedback during connect/sync operations
  - **Status messages**: Queue of messages with auto-dismiss timeout
  - **Confirmation dialogs**: Y/N prompts before destructive actions
  - **Error details**: View full error with 'E' key
  - **Help overlay**: Press '?' for organized keyboard shortcuts cheatsheet
  - **Header bar**: Shows connected count, avg CO2, alert count, indicators
  - **ASCII-only output**: All indicators use pure ASCII characters for compatibility

## [0.1.7] - 2026-01-18

### Added

- **aranet-store crate** - New SQLite-based local data persistence layer
  - Store current readings and device metadata
  - Cache history records from devices for offline access
  - Incremental sync support (only download new records)
  - Query by device, time range, with pagination
  - Automatic deduplication of history records
  - Platform-specific database locations:
    - Linux: `~/.local/share/aranet/data.db`
    - macOS: `~/Library/Application Support/aranet/data.db`
    - Windows: `C:\Users\<user>\AppData\Local\aranet\data.db`

- **CLI sync command** - Download device history to local database
  - `aranet sync --device <ADDRESS>` for incremental sync
  - `aranet sync --device <ADDRESS> --full` for complete re-download
  - Progress bar during history download

- **CLI cache command** - Query cached data without device connection
  - `aranet cache devices` - List all cached devices
  - `aranet cache stats` - Show cache statistics (readings, history counts)
  - `aranet cache history` - Query cached history with filters
  - `aranet cache info` - Show database path and size

### Changed

- All crate versions bumped to 0.1.7

## [0.1.6] - 2026-01-18

### Added

- **Code coverage with cargo-llvm-cov** - CI now reports test coverage via Codecov
- **Property-based testing with proptest** - Fuzz testing for all byte parsers
  - aranet-types: CurrentReading parser fuzzing
  - aranet-core: All device parsers (Aranet4, Aranet2, Radon, Radiation)
  - aranet-core: Advertisement parsing fuzzing
- **GUI tests** - Component tests for AppState (6 new tests)
- **TUI tests** - Component tests for App key handling (6 new tests)
- **Expanded MockDevice tests** - Comprehensive coverage for history, settings, calibration

### Fixed

- **Aranet Radiation advertisement parser panic** - Fixed crash on malformed data (found by proptest)
  - Corrected minimum byte length check from 19 to 21 bytes

### Changed

- Test count increased from 268 to 310+ tests
- All test modules now have comprehensive inline documentation

## [0.1.5] - 2026-01-18

### Added

- **Multi-device passive watch mode**
  - `aranet watch --passive` now monitors ALL devices broadcasting advertisements
  - Each reading clearly shows device name: `[AranetRn+ 306B8]`
  - No longer defaults to last connected device when watching all devices
  - Supports CSV, JSON, and text output formats with device identification

### Changed

- Improved watch output formatting with clearer device identification
- Passive mode header now shows "Watching: all devices (passive)"
- Consistent separator line formatting across watch modes

## [0.1.4] - 2026-01-17

### Fixed

- Fix clippy warnings for Rust 2024 edition compliance
- Resolve collapsible if/else-if blocks in CLI styling code
- Remove unnecessary `.clone()` on Copy types in tests
- Fix manual range contains and clamp patterns

## [0.1.3] - 2026-01-17

### Added

- **Rich CLI styling** (now the default)
  - Spinners for long-running operations (scan, connect, history download)
  - Color-coded sensor values based on thresholds (CO2, radon, battery, humidity, temperature)
  - Table formatting with `tabled` for history, info, alias, and scan output
  - Trend indicators in watch mode (up/down/stable arrows)
  - `--style` flag: `rich` (default), `minimal`, or `plain` for scripting
  - `--brief` flag for status command (compact one-line output)
  - Device name headers in read output
  - Air quality summary labels (Excellent, Good, Fair, Poor)
- **Pressure unit conversion** (`--inhg` / `--hpa` flags)
  - Display pressure in inches of mercury (inHg) with `--inhg`
  - Explicitly request hPa with `--hpa` (default)
  - Configurable via config file (`inhg = true`)
- **`doctor` command** for BLE diagnostics
  - Checks Bluetooth adapter availability and permissions
  - Scans for devices to verify BLE functionality
  - Platform-specific troubleshooting tips (macOS, Linux, Windows)
  - Numbered progress steps with colored status indicators
- **`alias` command** for device management
  - `alias list` - Show all saved device aliases (now with table formatting)
  - `alias set <name> <address>` - Create a friendly name for a device
  - `alias remove <name>` - Delete an alias
  - Use aliases anywhere a device address is expected
- **Passive read mode** (`--passive` flag on `read`)
  - Read sensor data from BLE advertisements without connecting
  - Requires Smart Home integration enabled on the device
  - Faster readings when device data is advertised
- **Multi-device read support**
  - Specify multiple devices: `aranet read -d device1 -d device2`
  - Comma-separated: `aranet read -d device1,device2`
  - Parallel reading from all devices
  - Combined output in text, JSON, and CSV formats
- **Interactive device picker**
  - When no device is specified, scan and present a selection menu
  - Works for `read`, `history`, `info`, `status`, `watch` commands
- **History date filters** (`--since` / `--until`)
  - Filter history by date range
  - Supports RFC3339 format (e.g., `2026-01-15T10:30:00Z`)
  - Supports date-only format (e.g., `2026-01-15`)
- **Progress bars for history download**
  - Visual progress indicator with percentage and current parameter
  - Shows download progress across all history parameters

### Changed

- Default style mode changed from `minimal` to `rich`
- Replaced unmaintained `atty` crate with `std::io::IsTerminal`
- Refactored `cmd_history` to use `HistoryArgs` struct (clippy compliance)

### Fixed

- macOS device identifier now uses CoreBluetooth UUID instead of placeholder address

## [0.1.2] - 2026-01-16

### Added

- **Full AranetRn+ (Radon) sensor support**
  - Current readings: radon (Bq/m³), temperature, pressure, humidity, battery, interval, age
  - History download: 4-byte radon values (param 10), humidity in tenths (param 5)
  - `HistoryRecord.radon` field for radon history data
  - `HistoryParam::Radon` and `HistoryParam::Humidity2` enum variants
- Complete BLE communication stack with btleplug 0.11
  - Device scanning and discovery
  - Connection management with auto-reconnection
  - Current readings for all device types
  - History download (V1 notification-based, V2 read-based)
  - Device settings read/write (interval, Bluetooth range, Smart Home)
- Multi-device support (Aranet4, Aranet2, AranetRn+, Aranet Radiation)
- Core types (CurrentReading, DeviceInfo, HistoryRecord, Status, DeviceType)
- BLE UUIDs for Aranet devices (both old and new firmware)
- **CLI fully implemented** with all core commands:
  - `scan` — Discover nearby Aranet devices
  - `read` — Read current sensor measurements
  - `status` — Quick one-line reading with colored CO₂ status
  - `info` — Display device information
  - `history` — Download historical data (text, JSON, CSV)
  - `set` — Modify device settings (interval, range, smart_home)
  - `watch` — Continuous monitoring with auto-reconnect
  - `config` — Manage configuration file (`~/.config/aranet/config.toml`)
  - `completions` — Generate shell completions (bash, zsh, fish, PowerShell)
- TUI app shell with ratatui
- GUI app shell with egui/eframe
- WASM module scaffolding
- Error types with thiserror
- Real-time streaming with `ReadingStream`
- Event system with `EventDispatcher`
- Connection metrics and operation tracking
- Mock device for testing
- Data validation and CO2 threshold helpers
- JSON and CSV output formats for all CLI commands
- Colored CO₂ status indicators (green/amber/red)
- Config file support with device, format, no_color, fahrenheit options
- `ARANET_DEVICE` environment variable support
- `--no-color` flag and `NO_COLOR` env var support

### Fixed

- Corrected UUID mappings for history characteristics (V1 → 2003, V2 → 2005)
- Fixed V2 history response parsing (10-byte header format)
- Resolved async deadlock in device connection
- Increased connection timeout for reliable BLE connections
- Corrected AranetRn+ GATT data format parsing (device_type, interval, age fields)

### Changed

- Updated to Rust 1.90 minimum (edition 2024)
- History download now uses correct parameter values for each sensor type
