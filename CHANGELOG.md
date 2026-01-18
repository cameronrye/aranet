# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
