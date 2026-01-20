<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg">
    <img alt="Aranet" src="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg" height="60">
  </picture>
</p>

# aranet-wasm

WebAssembly module for Aranet sensors via Web Bluetooth.

> **Note:** This crate is not yet implemented. It serves as a placeholder for a future WebAssembly module that will enable browser-based interaction with Aranet devices.

## Planned Features

- **Web Bluetooth integration** - Connect to Aranet devices from the browser
- **Shared types** - Use aranet-types for consistent data structures
- **JavaScript bindings** - Easy-to-use API for web applications
- **Real-time readings** - Stream sensor data to web dashboards
- **Cross-browser support** - Chrome, Edge, and other Web Bluetooth-enabled browsers

## Status

This crate is currently a placeholder. The implementation is planned for a future release.

To build for WebAssembly:

```bash
cargo build -p aranet-wasm --target wasm32-unknown-unknown
```

Or with wasm-pack:

```bash
cd crates/aranet-wasm
wasm-pack build --target web
```

## Planned Usage

```javascript
import init, { AranetDevice } from 'aranet-wasm';

await init();

// Request device via Web Bluetooth
const device = await AranetDevice.request();

// Read current values
const reading = await device.readCurrent();
console.log(`CO2: ${reading.co2} ppm`);
console.log(`Temperature: ${reading.temperature}C`);
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
| [aranet-gui](../aranet-gui/) | - | Desktop application (planned) |
| [aranet-service](../aranet-service/) | - | Background collector and REST API |

## License

MIT

---

Made with ❤️ by [Cameron Rye](https://rye.dev/)

