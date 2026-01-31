<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg">
    <img alt="Aranet" src="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg" height="60">
  </picture>
</p>

# aranet-gui

[![crates.io](https://img.shields.io/crates/v/aranet-gui.svg)](https://crates.io/crates/aranet-gui)
[![docs.rs](https://docs.rs/aranet-gui/badge.svg)](https://docs.rs/aranet-gui)

Native desktop GUI for Aranet environmental sensors built with [egui](https://www.egui.rs/).

**[Full Documentation](https://cameronrye.github.io/aranet/)**

## Features

- **Device Discovery** - Scan for nearby Aranet devices via Bluetooth LE
- **Real-time Monitoring** - Live sensor readings with CO2 color coding (green/yellow/orange/red)
- **Multi-device Support** - Connect to and monitor multiple devices simultaneously
- **Historical Charts** - Visualize CO2, radon, radiation, temperature, and humidity trends
- **Time Filtering** - Filter history by All/24h/7d/30d
- **Device Settings** - Configure measurement interval, Bluetooth range, and Smart Home mode
- **System Tray** - Minimize to system tray with status indicator
- **Cross-platform** - Works on macOS, Windows, and Linux

## Supported Devices

- Aranet4 (CO2, temperature, humidity, pressure)
- Aranet2 (temperature, humidity)
- AranetRn+ (radon, temperature, pressure, humidity)
- Aranet Radiation (radiation rate, total dose)

## Usage

```bash
# Run the GUI application
cargo run -p aranet-gui
```

## Screenshot

![Aranet GUI Dashboard](https://raw.githubusercontent.com/cameronrye/aranet/main/assets/screenshots/gui-main.png)

The application features:
- **Header** - Tabs (Dashboard/History/Settings) and Scan button
- **Sidebar** - Device list with connection status
- **Dashboard** - Current readings with color-coded values
- **History** - Interactive charts for all sensor metrics
- **Settings** - Device configuration and info display

## Related Crates

This crate is part of the [aranet](https://github.com/cameronrye/aranet) workspace:

| Crate | crates.io | Description |
|-------|-----------|-------------|
| [aranet-core](../aranet-core/) | [![crates.io](https://img.shields.io/crates/v/aranet-core.svg)](https://crates.io/crates/aranet-core) | Core BLE library for device communication |
| [aranet-types](../aranet-types/) | [![crates.io](https://img.shields.io/crates/v/aranet-types.svg)](https://crates.io/crates/aranet-types) | Shared types for sensor data |
| [aranet-store](../aranet-store/) | [![crates.io](https://img.shields.io/crates/v/aranet-store.svg)](https://crates.io/crates/aranet-store) | Local data persistence |
| [aranet-cli](../aranet-cli/) | [![crates.io](https://img.shields.io/crates/v/aranet-cli.svg)](https://crates.io/crates/aranet-cli) | Command-line interface |
| [aranet-tui](../aranet-tui/) | [![crates.io](https://img.shields.io/crates/v/aranet-tui.svg)](https://crates.io/crates/aranet-tui) | Terminal UI dashboard |
| [aranet-service](../aranet-service/) | [![crates.io](https://img.shields.io/crates/v/aranet-service.svg)](https://crates.io/crates/aranet-service) | Background collector and REST API |

## License

MIT

---

Made with ❤️ by [Cameron Rye](https://rye.dev/)

