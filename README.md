<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/aranet-logo-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="assets/aranet-logo-light.svg">
    <img alt="Aranet" src="assets/aranet-logo-light.svg" height="60">
  </picture>
</p>

<p align="center">
  Rust implementation for Aranet environmental sensors.
</p>
Connect to your Aranet devices via Bluetooth LE to read measurements, download history, and monitor air quality.

[![CI](https://github.com/cameronrye/aranet/workflows/CI/badge.svg)](https://github.com/cameronrye/aranet/actions)
[![codecov](https://codecov.io/gh/cameronrye/aranet/graph/badge.svg)](https://codecov.io/gh/cameronrye/aranet)
[![crates.io](https://img.shields.io/crates/v/aranet-cli.svg)](https://crates.io/crates/aranet-cli)
[![docs.rs](https://docs.rs/aranet-core/badge.svg)](https://docs.rs/aranet-core)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.90%2B-orange.svg)](https://www.rust-lang.org)

## Features

- **aranet-core** — Core BLE library supporting Aranet4, Aranet2, AranetRn+ (Radon), and Aranet Radiation sensors
  - Current readings (CO₂, temperature, pressure, humidity, radon, radiation)
  - Historical data download with timestamps
  - Device settings (measurement interval, Bluetooth range)
  - Auto-reconnection with configurable backoff
  - Real-time streaming and event system
- **aranet-cli** — Command-line interface for quick readings and data export
  - Multi-device reading with parallel connections
  - Interactive device picker, device aliases
  - Passive reading from BLE advertisements
  - Progress bars for history download
  - `--since`/`--until` date filters, `--inhg` pressure unit
  - Local data caching with `sync` and `cache` commands
- **aranet-store** — Local SQLite-based data persistence
  - Incremental history sync (download only new records)
  - Query cached data without device connection
  - Automatic deduplication of history records
- **aranet-tui** — Terminal UI dashboard for real-time monitoring
  - Multi-device monitoring with auto-refresh
  - Sparkline charts with min/max labels
  - CO2/radon threshold alerts with audio bell
  - Light/dark theme, mouse support, vim keybindings
  - Export history to CSV, comparison view
  - Device filter, alias management, settings editing
- **aranet-gui** — Desktop application built with egui
- **aranet-wasm** — WebAssembly module for browser integration *(planned)*

## Installation

Install the CLI from [crates.io](https://crates.io/crates/aranet-cli):

```bash
cargo install aranet-cli
```

Or build from source:

```bash
git clone https://github.com/cameronrye/aranet.git
cd aranet
cargo build --release
```

### Using as a Library

Add `aranet-core` to your `Cargo.toml`:

```toml
[dependencies]
aranet-core = "0.1"
```

## Quick Start

### Scan for devices

```bash
aranet scan
```

### Read current measurements

```bash
aranet read <DEVICE_ADDRESS>
```

### Download measurement history

```bash
aranet history <DEVICE_ADDRESS> --output history.csv
```

### View device information

```bash
aranet info <DEVICE_ADDRESS>
```

### Read from multiple devices

```bash
aranet read -d device1 -d device2
aranet read -d living-room,bedroom  # using aliases
```

### Manage device aliases

```bash
aranet alias set living-room AA:BB:CC:DD:EE:FF
aranet alias list
aranet read -d living-room
```

### Diagnose BLE issues

```bash
aranet doctor
```

## Project Structure

```
aranet/
├── crates/
│   ├── aranet-types/    # Platform-agnostic types (shared)
│   ├── aranet-core/     # Core BLE library
│   ├── aranet-store/    # Local SQLite data persistence
│   ├── aranet-cli/      # CLI tool
│   ├── aranet-tui/      # Terminal dashboard
│   ├── aranet-gui/      # Desktop GUI (egui)
│   └── aranet-wasm/     # WebAssembly module
└── docs/                # Protocol documentation
```

## Supported Devices

| Device | Sensors | Current | History | Status |
|--------|---------|---------|---------|--------|
| Aranet4 | CO₂, Temperature, Pressure, Humidity | Yes | Yes | Fully tested |
| Aranet2 | Temperature, Humidity | Yes | Yes | Supported |
| AranetRn+ (Radon) | Radon, Temperature, Pressure, Humidity | Yes | Yes | Fully tested |
| Aranet Radiation | Dose Rate, Total Dose | Yes | Partial | Supported (history not yet implemented) |

## Requirements

- **Rust 1.90+**
- **Bluetooth adapter** with BLE support
- **Platform support:**
  - macOS
  - Linux (with BlueZ)
  - Windows

## Contributing

Contributions are welcome! Please check the [open issues](https://github.com/cameronrye/aranet/issues) for areas where you can help.

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Acknowledgments

- [Aranet4-Python](https://github.com/Anrijs/Aranet4-Python) - Python implementation that inspired this project
- [btleplug](https://github.com/deviceplug/btleplug) - Cross-platform Bluetooth LE library for Rust

---

Made with ❤️ by [Cameron Rye](https://rye.dev/)
