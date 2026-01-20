<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg">
    <img alt="Aranet" src="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg" height="60">
  </picture>
</p>

# aranet-types

Platform-agnostic types for Aranet environmental sensors.

This crate provides shared types that can be used by both native and WebAssembly implementations for interacting with Aranet devices (Aranet4, Aranet2, Aranet Radon, Aranet Radiation).

## Features

- **Core data types** for sensor readings (CO₂, temperature, humidity, pressure, radon, radiation)
- **Device information structures** for device metadata
- **UUID constants** for BLE characteristics
- **Error types** for data parsing
- **Serde support** (enabled by default) for serialization/deserialization

## Supported Devices

| Device | Measurements |
|--------|-------------|
| Aranet4 | CO₂, temperature, humidity, pressure |
| Aranet2 | Temperature, humidity |
| Aranet Radon | Temperature, humidity, pressure, radon, 24h/7d/30d averages |
| Aranet Radiation | Radiation rate/total |

## Installation

```toml
[dependencies]
aranet-types = "0.1"
```

To disable serde support:

```toml
[dependencies]
aranet-types = { version = "0.1", default-features = false }
```

## Usage

```rust
use aranet_types::{CurrentReading, Status, DeviceType};

// Parse raw BLE data
let bytes: [u8; 13] = [/* ... */];
let reading = CurrentReading::from_bytes(&bytes)?;

println!("CO₂: {} ppm", reading.co2);
println!("Temperature: {:.1}°C", reading.temperature);
println!("Status: {}", reading.status);

// Use the builder pattern
let reading = CurrentReading::builder()
    .co2(800)
    .temperature(22.5)
    .humidity(45)
    .build();

// Device-specific parsing
let reading = CurrentReading::from_bytes_for_device(&data, DeviceType::Aranet2)?;
```

## Feature Flags

| Feature | Default | Description |
|---------|---------|-------------|
| `serde` | Yes | Enables serialization/deserialization support |

## Related Crates

This crate is part of the [aranet](https://github.com/cameronrye/aranet) workspace:

| Crate | crates.io | Description |
|-------|-----------|-------------|
| [aranet-core](../aranet-core/) | [![crates.io](https://img.shields.io/crates/v/aranet-core.svg)](https://crates.io/crates/aranet-core) | Core BLE library for device communication |
| [aranet-store](../aranet-store/) | [![crates.io](https://img.shields.io/crates/v/aranet-store.svg)](https://crates.io/crates/aranet-store) | Local data persistence |
| [aranet-cli](../aranet-cli/) | [![crates.io](https://img.shields.io/crates/v/aranet-cli.svg)](https://crates.io/crates/aranet-cli) | Command-line interface |
| [aranet-tui](../aranet-tui/) | [![crates.io](https://img.shields.io/crates/v/aranet-tui.svg)](https://crates.io/crates/aranet-tui) | Terminal UI dashboard |
| [aranet-service](../aranet-service/) | - | Background collector and REST API |
| [aranet-gui](../aranet-gui/) | - | Desktop application (planned) |
| [aranet-wasm](../aranet-wasm/) | - | WebAssembly module (planned) |

## License

MIT

---

Made with ❤️ by [Cameron Rye](https://rye.dev/)
