<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg">
    <img alt="Aranet" src="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg" height="60">
  </picture>
</p>

# aranet-gui

Native desktop GUI for Aranet environmental sensors.

> **Note:** This crate is not yet implemented. It serves as a placeholder for a future desktop application built with [egui](https://www.egui.rs/).

## Planned Features

- **Native desktop application** - Cross-platform GUI using egui/eframe
- **Real-time monitoring** - Live sensor readings with visual indicators
- **Device management** - Scan, connect, and configure Aranet devices
- **Historical data** - View and export measurement history
- **Threshold alerts** - Visual notifications for CO2/radon levels
- **System tray integration** - Background monitoring with notifications

## Status

This crate is currently a placeholder. The implementation is planned for a future release.

To build:

```bash
cargo build -p aranet-gui
```

## Related Crates

This crate is part of the [aranet](https://github.com/cameronrye/aranet) workspace:

| Crate | crates.io | Description |
|-------|-----------|-------------|
| [aranet-core](../aranet-core/) | [![crates.io](https://img.shields.io/crates/v/aranet-core.svg)](https://crates.io/crates/aranet-core) | Core BLE library for device communication |
| [aranet-types](../aranet-types/) | [![crates.io](https://img.shields.io/crates/v/aranet-types.svg)](https://crates.io/crates/aranet-types) | Shared types for sensor data |
| [aranet-store](../aranet-store/) | [![crates.io](https://img.shields.io/crates/v/aranet-store.svg)](https://crates.io/crates/aranet-store) | Local data persistence |
| [aranet-cli](../aranet-cli/) | [![crates.io](https://img.shields.io/crates/v/aranet-cli.svg)](https://crates.io/crates/aranet-cli) | Command-line interface |
| [aranet-tui](../aranet-tui/) | [![crates.io](https://img.shields.io/crates/v/aranet-tui.svg)](https://crates.io/crates/aranet-tui) | Terminal UI dashboard |
| [aranet-service](../aranet-service/) | - | Background collector and REST API |
| [aranet-wasm](../aranet-wasm/) | - | WebAssembly module (planned) |

## License

MIT

---

Made with ❤️ by [Cameron Rye](https://rye.dev/)

