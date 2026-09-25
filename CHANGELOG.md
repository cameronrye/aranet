# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Linux ARM64 builds** — prebuilt CLI and GUI for 64-bit ARM Linux (`aarch64-unknown-linux-gnu`, glibc 2.35 or newer, such as Raspberry Pi OS Bookworm), offered by the install scripts
- **Release install test** — every release is installed with its own install scripts on each platform before it is published, and the documented one-liners are checked again once it is live; each macOS DMG now has a `.sha256`

### Changed

- **`aranet sync` with no device and no default device configured** now falls back to the last-used device, then (when `behavior.load_cache` is on, the default) the most recently seen device in the local database, as `read` and `status` do. It used to scan and prompt for a device, or fail with "No device specified" when not run interactively
- **Where `aranet sync` stores history** — 0.2.1 stores history under the device address. History that 0.2.0 synced under any other identifier (a device name or part of one, a MAC address typed in lowercase or without colons, or part of a macOS device UUID) stays under that ID, and `report`, `history` and `cache` queries for the old ID stop receiving new records. The first sync after upgrading downloads the device's whole history buffer again under the address, and `aranet sync --all` syncs the sensor once for each ID it is stored under. The `device` field in single-device `aranet sync --format json` output is now also this address rather than the identifier given on the command line; `aranet sync --all` output is unchanged
- **History deduplication window** — records within 30 s of a record already stored for the same device are skipped on every history write, including `aranet cache import` (so an import can now report skipped duplicates that are not exact timestamp matches). Aranet devices measure at most once a minute, so real records are never this close. Duplicate rows already in the database are not removed. Rows that 0.2.0 stored were shifted by the length of their download, which can exceed 30 s, so the first download after upgrading that overlaps them (such as `aranet history`, which saves the whole buffer) may still add a copy
- **`aranet set smart-home` without a value** is now an error. 0.2.0 release builds read it as `false`, a request to turn Smart Home off
- **Service WebSocket** — when the API key is sent as `?token=`, a request with more than one `token` parameter is now rejected. 0.2.0 used the first one
- **The prebuilt `aranet` CLI no longer includes `aranet gui`** — install `aranet-gui` or the macOS app for the desktop GUI (`cargo install aranet-cli --features gui` still builds it in)
- **Leaner `aranet-cli` builds** — `aranet-service` is compiled only with the `cli` feature (it runs `aranet server`), so `aranet-tui`, `aranet-gui` and TUI-only builds no longer compile the HTTP server; the unused `axum` and `tower-http` dependencies were removed
- **HTTP clients** — outgoing requests (webhooks, InfluxDB, Prometheus push gateway, service client) use HTTP/1.1, and `aranet server` no longer accepts cleartext HTTP/2 (the standalone `aranet-service` never did). The CLI, TUI and GUI no longer read macOS/Windows system proxy settings; `HTTP_PROXY`, `HTTPS_PROXY` and `NO_PROXY` still apply
- **Releases** — all seven crates take their version from the workspace manifest and are released with cargo-release: one `chore: release vX.Y.Z` commit, one `vX.Y.Z` tag on exactly that commit, and crates published to crates.io from that tag after the release binaries are built (CONTRIBUTING.md, "Releasing")
- **GitHub Release titles and notes** now come from this changelog
- **Homebrew** — one workflow (`.github/workflows/homebrew.yml`) updates the `cameronrye/aranet` tap after each release, and refuses to publish if a release asset is missing or doesn't match its published `.sha256`; pre-releases no longer touch the tap
- **Screenshots** — the screenshot workflow pushes its updates to a `screenshots/<tag>` branch for review instead of committing to `main`

### Removed

- Unused Homebrew formula sources (`distribution/homebrew/`) and the `aranet-cli.rb` / `aranet-gui.rb` release assets that no tap installed

### Fixed

- **Aranet2 and Aranet Radiation advertisements** are decoded at the offsets real devices use; Radiation dose rate is no longer reported 10× too high
- **History download** no longer drops the newest record, and no longer spins forever if a device repeats a packet
- **Duplicate history rows** — re-syncing a device no longer inserts copies of records already stored. History timestamps are anchored when the device is queried, and a record within 30 s of one already stored for the same device is skipped as a duplicate (see Changed)
- **`aranet sync`** resolves aliases like `read` and `status` do, and stores history under the connected device's address like other commands (see Changed)
- **`aranet status`** shows radon in pCi/L correctly (previously printed the Bq/m³ value with a pCi/L label); **`aranet report`** labels the radon threshold it actually counts, and its radon "time above" percentage now counts only records that have a radon value
- **`aranet set smart-home true|false`** is accepted (see Changed)
- **Service API** — `offset` without `limit` no longer returns 500; a rejected device update no longer corrupts the running configuration
- **Service WebSocket** — API keys containing `+`, `/` or `=` now work from the dashboard, which URL-encodes the `?token=` value. Clients that send the key unencoded keep working (see Changed)
- **macOS** — no longer leaks an OS thread on every Bluetooth connection. The shared Bluetooth adapter runs on a background thread of its own, so it keeps working for programs that use more than one tokio runtime, and it is replaced if its CoreBluetooth thread stops
- **Install scripts** — `aranet-cli-installer.sh`/`.ps1` and `aranet-gui-installer.sh`/`.ps1` offer every published platform again, and the macOS installs no longer fail with a checksum mismatch
- **Checksums** — the `.sha256` files and `sha256.sum` match the signed macOS archives, and `sha256.sum` lists every archive
- **Linux CLI** no longer needs GTK or libxdo installed
- **Windows GUI** no longer opens a console window; as a result, `aranet-gui --help` and `--version` print nothing in a Windows terminal (use `aranet` there)
- **macOS app** has its icon, minimum macOS version (11.0) and Bluetooth usage descriptions; the `aranet-gui` binary in the macOS tarballs is Developer ID signed and notarized like the CLI
- **`aranet-cli` with only the `tui` feature** (`cargo install aranet-cli --no-default-features --features tui`) compiles; the `aranet` binary now uses the library's TUI instead of compiling a second copy of it

### Security

- **Linux BlueZ agent** — aranet was already BlueZ's default agent while running, and 0.2.0 approved every pairing and authorization request from any device. It now approves pairing, confirmation and authorization only for devices aranet has connected to (or tried to) since it started, and always rejects `AuthorizeService`, which lets a remote device use the host's own Bluetooth profiles (such as HID input). As before, these requests don't reach the desktop's agent while aranet runs; pair other devices from the desktop's Bluetooth settings, or stop aranet first
- **Service logs** no longer record request query strings, which could include the API key
- **Dependencies updated** to clear the 11 RustSec advisories that affect 0.2.0's lockfile: `h2` (RUSTSEC-2026-0258), `quick-xml` (RUSTSEC-2026-0194, RUSTSEC-2026-0195), `quinn-proto` (RUSTSEC-2026-0185), `rustls` (RUSTSEC-2026-0285), `rustls-webpki` (RUSTSEC-2026-0098, RUSTSEC-2026-0099, RUSTSEC-2026-0104) and `webbrowser` (RUSTSEC-2026-0257). This also replaces the unsound `anyhow`, `event-listener`, `lru`, `memmap2` and `rand` releases and the yanked `spin` 0.9.8
- **One TLS stack for HTTP**: webhooks, InfluxDB export, the Prometheus push gateway and the TUI/GUI service client now use rustls on every platform, trusting the OS certificate store plus the bundled Mozilla roots. The Linux `aranet` binary no longer links OpenSSL. MQTT over TLS (`mqtts://`, the `mqtt` feature of `aranet-service`) still uses the platform TLS library until `rumqttc` ships a patched `rustls-webpki`
- **GitHub Actions** — every workflow gives its `GITHUB_TOKEN` only the permissions each job needs, pins every action to a full commit SHA, and no longer leaves the token in the checkout's git config
- **Homebrew tap token** — no longer handed to a third-party action

## [0.2.0] - 2026-03-28

### Added

- **Docker support** - Multi-stage `Dockerfile` and `docker-compose.yml` at the repo root for containerized deployment with BLE passthrough, health checks, and persistent storage
- **Homebrew formula automation** - GitHub Actions workflow (`.github/workflows/homebrew.yml`) automatically publishes updated Homebrew formulae to the `cameronrye/homebrew-aranet` tap on each release
- **CLI cached reports** - `aranet report` summarizes cached daily, weekly, or monthly history in text or JSON without reconnecting to devices
- **Webhook notifications** - HTTP POST alerts when CO2, radon, or battery thresholds are exceeded, with configurable endpoints, per-device cooldowns, and custom headers for Slack/Discord/PagerDuty integration
- **InfluxDB export** - Real-time sensor data export to InfluxDB v2 using line protocol, with configurable organization, bucket, and measurement names
- **mDNS service discovery** - Automatic network advertisement via `_aranet._tcp.local.` and `_http._tcp.local.` so clients can discover the service without manual IP configuration
- **Grafana dashboard template** - Pre-built dashboard (`grafana/aranet-dashboard.json`) with panels for CO2 gauges, environment time series, radon/radiation, battery levels, and collector statistics
- **Improved web dashboard** - Tab-based UI with Overview (live sparklines), History (interactive charts with statistics), and Service Status (collector health and per-device poll stats)
- **Home Assistant MQTT auto-discovery** - `mqtt.homeassistant = true` publishes discovery payloads while `mqtt.ha_discovery_prefix` controls the HA discovery namespace
- **E2E API tests** - 27 integration tests covering health, devices, readings, configuration, authentication, Prometheus metrics, and broadcast channel integration
- **BLE adapter serialization** - Semaphore-based locking ensures only one device uses the Bluetooth adapter at a time, preventing connection contention
- **Staggered device startup** - Devices start polling with a 5-second stagger to avoid adapter contention on boot
- **Cached BLE manager** - Shared `Manager` instance with automatic reset on D-Bus connection failure
- **Poll duration metric** - New `aranet_device_poll_duration_ms` Prometheus gauge tracks how long each device poll takes
- **Staleness detection** - `/api/devices/:id/current` now returns `age_seconds` and `stale` fields
- **`DeviceType` capability methods** - `has_co2()`, `has_temperature()`, `has_humidity()`, `has_pressure()` for querying sensor capabilities
- **Aranet Radiation ☢ symbol detection** - `DeviceType::from_name` now recognizes the Unicode ☢ (U+2622) in device names

### Changed

- **Aranet2 GATT protocol** - Fixed parsing to use correct 12-byte format with proper field ordering (header, interval, age, battery, temp, humidity, status flags)
- **Aranet2 status bit extraction** - Status is now read from bits[2:3] (temperature status) instead of bits[0:1]
- **Aranet2 humidity parsing** - Humidity is now parsed as u16 LE divided by 10 (was u8 raw)
- **BlueZ agent** - Now registers as default agent with retry cap (max 3 attempts) and permanent failure state
- **Prometheus metrics** - Metrics are now filtered by device capabilities (e.g., Aranet2 won't emit `aranet_co2_ppm`)
- **Prometheus device labels** - Device labels now use configured aliases when available
- **MQTT publishing** - Published topics now use configured aliases when available and include radon averages when present
- **Passive monitor** - Retries adapter acquisition on startup instead of exiting immediately; logs all consecutive errors
- **Service discovery retry** - Automatically disconnects and retries when BlueZ returns 0 services (stale state)
- **Connection timeouts** - `challenging_environment()` preset now uses 90s connection / 30s discovery timeouts

### Breaking

- **`GET /api/devices/:id/current`** now returns `CurrentReadingResponse` with `age_seconds` and `stale` fields alongside the reading (previously returned raw `StoredReading`)
- **WebSocket query-token auth** - the `token` query parameter is now accepted only on `/api/ws`; REST and metrics requests must send `X-API-Key`

### Internal

- Bumped Rust edition to 2024 with minimum supported Rust version 1.90
- Removed legacy advertisement parsers (v1 format) — only v2 parsers are used
- Consolidated GATT reading parsers in `aranet-core` to delegate to `aranet-types::CurrentReading`
- Extracted `spawn_staggered_device_tasks()` helper to deduplicate collector startup logic
- Synchronized all crate versions to 0.2.0

## [0.1.13] - 2026-03-25

### Added

- **BlueZ Agent** - Register a BlueZ agent on Linux to prevent BLE hangs during service discovery

### Fixed

- Fix BLE connection hang on Linux caused by unhandled BlueZ pairing/authorization requests
- Update vulnerable dependencies: `bytes` 1.11.1, `quinn-proto` 0.11.14, `rustls-webpki` 0.103.10, `time` 0.3.47
- Ignore unpatchable `rustls-webpki` 0.102.x advisory (pinned by `rumqttc`)

## [0.1.12] - 2026-02-01

### Added

- **Diagnostics Module** - Comprehensive Bluetooth diagnostics and troubleshooting
  - `DiagnosticsCollector` for gathering adapter info, connection stats, and recent errors
  - `AdapterInfo` with state detection (Available, PoweredOff, NotFound)
  - `ConnectionStats` tracking success rates, connection times, and disconnection reasons
  - `ErrorCategory` classification with recorded error history
  - Global diagnostics collector accessible via `global_diagnostics()`

- **Passive Monitoring** - Monitor devices via BLE advertisements without connecting
  - `PassiveMonitor` for low-power, connectionless monitoring
  - Support for more devices than the BLE connection limit
  - Configurable deduplication and device filtering
  - `PassiveReading` with device ID, name, RSSI, and parsed advertisement data

- **Platform Module** - Platform-specific BLE configuration and tuning
  - `Platform` detection (macOS, Linux, Windows) with appropriate defaults
  - `PlatformConfig` with recommended timeouts and scan durations
  - Cross-platform device aliasing with `DeviceAlias` and `AliasStore`
  - Documentation of macOS UUID behavior and cross-platform considerations

- **Retry Logic** - Configurable retry policies for BLE operations
  - `RetryPolicy` with immediate, fixed, and exponential backoff strategies
  - Configurable max attempts, delays, and jitter
  - Integration with connection and read operations

- **History Improvements**
  - `HistoryCheckpoint` for resumable history downloads
  - `PartialHistoryData` for incremental sync with checkpoint tracking
  - History sync progress events with record counts

- **Device Connection Improvements**
  - `ConnectionConfig` for configurable connection settings
  - `SignalQuality` enum (Excellent, Good, Fair, Poor, Unknown) for RSSI classification
  - Improved connection state tracking

- **Device Manager Improvements**
  - `AdaptiveInterval` for smart polling that adjusts based on device activity
  - `DevicePriority` for prioritizing certain devices during polling

- **Error Context** - User-friendly error messages with actionable suggestions
  - `ErrorContext` with message, retryable flag, and suggestion
  - Automatic classification of errors from `aranet_core::Error`
  - Suggestions for common issues (out of range, Bluetooth errors, etc.)

- **New Commands**
  - `CancelOperation` - Cancel long-running scans, syncs, and other operations
  - `StartBackgroundPolling` / `StopBackgroundPolling` - Automatic device polling
  - System service management from GUI/TUI

- **GUI Service Panel** - Manage aranet-service from the desktop application
  - View service status (installed, running)
  - Install/uninstall system service (user-level, no admin required)
  - Start/stop service controls
  - Device collection statistics

- **API Improvements**
  - `/api/health/detailed` endpoint with database, collector, and platform diagnostics
  - Improved Prometheus metrics with better lock management
  - Enhanced error responses with context

### Changed

- Improved Prometheus metrics collection performance (shorter lock duration)
- Enhanced history sync events with progress tracking
- Better error propagation with context information
- Synchronized all crate versions to 0.1.12

### Fixed

- Lock contention in Prometheus metrics endpoint

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
