# Aranet Roadmap

A complete Rust implementation for Aranet environmental sensors, designed for feature parity with [Aranet4-Python](https://github.com/Anrijs/Aranet4-Python) and beyond.

> **Dependency Policy**: Always use the **latest stable versions** of all libraries, frameworks, and tools. Pin to major versions only (e.g., `btleplug = "0.11"` not `"0.11.4"`). Run `cargo update` regularly. Check [crates.io](https://crates.io) and [lib.rs](https://lib.rs) for current versions before adding dependencies.

---

## Current Progress (Updated Jan 16, 2026)

| Phase | Component | Status | Progress |
|-------|-----------|--------|----------|
| 0 | Foundation | ✅ DONE | README, LICENSE, CI, CHANGELOG, aranet-types |
| 1 | Core Library | ✅ DONE | Full BLE: scan, connect, read, history, settings - tested with real hardware |
| 2 | CLI Tool | 🔶 WIP | Commands defined + shell completions; core integration pending |
| 3 | TUI Dashboard | 🔶 WIP | App shell + quit key; sensor integration pending |
| 4 | GUI Application | 🔶 WIP | egui shell works; sensor integration pending |
| 5 | WASM Module | 🔶 WIP | Basic init/log; Web Bluetooth pending |

**Legend**: [ ] Not started - [~] In progress/partial - [x] Complete

### What's Working Now
- **aranet-core**: Complete BLE stack with btleplug 0.11 - scan, connect, device info, current readings, history download (V1+V2), settings read/write, auto-reconnection, streaming, notifications, RSSI, multi-device manager, event system, validation, thresholds, metrics, mock device
- **aranet-types**: Shared types for CurrentReading, DeviceInfo, HistoryRecord, Status, DeviceType, all UUIDs
- **Multi-device support**: Aranet4, Aranet2, Aranet Radon, Aranet Radiation - all parsing implemented and tested
- **AranetRn+ (Radon)**: Full support including current readings (radon, temp, pressure, humidity) and complete history download with 4-byte radon values
- **CLI scaffolding**: All commands defined with clap v4, shell completions working
- **Hardware tested**: Aranet4 17C3C (FW v1.4.19), AranetRn+ 306B8 (FW v1.12.0)

### Recent Improvements (Jan 2026)
- Fixed UUID mappings for history characteristics
- Fixed V2 history parsing (10-byte header format)
- Added full AranetRn+ sensor support with radon history download
- `HistoryRecord` now includes optional `radon: Option<u32>` field
- All 179 workspace tests passing (168 run, 15 ignored - require BLE hardware)

### Next Priority
1. Wire CLI commands to aranet-core (scan, read, history, info work end-to-end)
2. Add sensor data display to TUI/GUI shells
3. Implement Web Bluetooth in WASM module

## Vision

Build the definitive Rust ecosystem for Aranet devices:
- **aranet-types** - Platform-agnostic data types (shared by all crates)
- **aranet-core** - Native BLE client via btleplug
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

| Feature | Priority | Status |
|---------|----------|--------|
| `scan` - Discover devices | P0 | [~] Command defined |
| `read` - Current measurements | P0 | [~] Command defined |
| `history` - Download historical data | P0 | [~] Command defined |
| `info` - Device information | P0 | [~] Command defined |
| `export` - CSV/JSON export | P0 | [ ] |
| `set` - Modify device settings | P1 | [~] Command defined |
| `completions` - Shell completions | P1 | [x] Implemented |
| `watch` - Continuous monitoring | P1 | [ ] |
| `--quiet` flag | P1 | [x] Implemented |
| `--output` flag (file output) | P1 | [~] Defined |
| Multi-device support | P1 | [ ] |
| Colored output with status indicators | P2 | [ ] |

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
│                   Browser (Chrome)                   │
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
| 1 | Core Library | 2-3 weeks | btleplug, tokio, thiserror |
| 2 | CLI Tool | 1 week | Phase 1 + clap, serde |
| 3 | TUI Dashboard | 1-2 weeks | Phase 1 + ratatui, crossterm |
| 4 | GUI App | 2-3 weeks | Phase 1 + egui/iced |
| 5 | WASM Web | 2-3 weeks | aranet-types + wasm-bindgen |

---

## Testing Strategy

### Unit Tests
- **aranet-types**: Test data parsing, serialization, type conversions
- **aranet-core**: Test with mock BLE adapter where possible
- Run with: `cargo test --workspace`

### Integration Tests
- Located in `crates/aranet-core/tests/`
- Require actual BLE hardware (marked with `#[ignore]`)
- Run with: `cargo test -- --ignored` (when hardware available)

### CI Testing
- GitHub Actions runs on every PR
- Tests on: Ubuntu, macOS, Windows
- Linting: `cargo clippy`, `cargo fmt --check`
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
