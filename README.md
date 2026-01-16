# ARANET

Rust implementation for Aranet environmental sensors. Connect to your Aranet devices via Bluetooth LE to read measurements, download history, and monitor air quality.

[![CI](https://github.com/cameronrye/aranet/workflows/CI/badge.svg)](https://github.com/cameronrye/aranet/actions)
[![crates.io](https://img.shields.io/crates/v/aranet-core.svg)](https://crates.io/crates/aranet-core)
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
- **aranet-tui** — Terminal UI dashboard for real-time monitoring
- **aranet-gui** — Desktop application built with egui
- **aranet-wasm** — WebAssembly module for browser integration *(planned)*

## Installation

> **Coming Soon** - Once published to crates.io:

```bash
cargo install aranet-cli
```

For now, build from source:

```bash
git clone https://github.com/cameronrye/aranet.git
cd aranet
cargo build --release
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

## Project Structure

```
aranet/
├── crates/
│   ├── aranet-core/     # Core BLE library
│   ├── aranet-cli/      # CLI tool
│   ├── aranet-tui/      # Terminal dashboard
│   ├── aranet-gui/      # Desktop GUI (egui)
│   └── aranet-wasm/     # WebAssembly module
└── docs/                # Protocol documentation
```

## Supported Devices

| Device | Sensors | Current | History | Status |
|--------|---------|---------|---------|--------|
| Aranet4 | CO₂, Temperature, Pressure, Humidity | ✅ | ✅ | Fully tested |
| Aranet2 | Temperature, Humidity | ✅ | ✅ | Supported |
| AranetRn+ (Radon) | Radon, Temperature, Pressure, Humidity | ✅ | ✅ | Fully tested |
| Aranet Radiation | Dose Rate, Total Dose | ✅ | ⚠️ | Supported (history not yet implemented) |

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
