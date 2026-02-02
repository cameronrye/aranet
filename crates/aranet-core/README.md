<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg">
    <img alt="Aranet" src="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg" height="60">
  </picture>
</p>

# aranet-core

[![crates.io](https://img.shields.io/crates/v/aranet-core.svg)](https://crates.io/crates/aranet-core)
[![docs.rs](https://docs.rs/aranet-core/badge.svg)](https://docs.rs/aranet-core)

Core BLE library for Aranet environmental sensors.

**[Full Documentation](https://cameronrye.github.io/aranet/)**

This crate provides low-level Bluetooth Low Energy (BLE) communication with Aranet sensors including the Aranet4, Aranet2, AranetRn+ (Radon), and Aranet Radiation devices.

## Features

- **Device discovery** — Scan for nearby Aranet devices via BLE
- **Current readings** — CO₂, temperature, pressure, humidity, radon, radiation
- **Historical data** — Download measurement history with timestamps and resumable checkpoints
- **Device settings** — Read/write measurement interval, Bluetooth range
- **Auto-reconnection** — Configurable backoff and retry logic with exponential delays
- **Real-time streaming** — Subscribe to sensor value changes
- **Multi-device support** — Manage multiple sensors simultaneously with adaptive polling
- **Passive monitoring** — Monitor devices via BLE advertisements without connecting
- **Platform support** — Platform-specific configuration for macOS, Linux, and Windows
- **Diagnostics** — Bluetooth adapter diagnostics, connection stats, and error tracking
- **Cross-platform aliases** — Device aliasing system for consistent identification

## Supported Devices

| Device | Sensors |
|--------|---------|
| Aranet4 | CO₂, Temperature, Pressure, Humidity |
| Aranet2 | Temperature, Humidity |
| AranetRn+ | Radon (Bq/m³), Temperature, Pressure, Humidity |
| Aranet Radiation | Dose Rate (µSv/h), Total Dose (mSv) |

## Installation

```toml
[dependencies]
aranet-core = "0.1"
```

## Usage

```rust
use aranet_core::{Device, scan};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Scan for devices
    let devices = scan::scan_for_devices().await?;
    println!("Found {} devices", devices.len());

    // Connect to a device
    let device = Device::connect("Aranet4 12345").await?;

    // Read current values
    let reading = device.read_current().await?;
    println!("CO₂: {} ppm", reading.co2);
    println!("Temperature: {:.1}°C", reading.temperature);

    // Read device info
    let info = device.read_device_info().await?;
    println!("Serial: {}", info.serial);

    // Download history
    let history = device.download_history().await?;
    println!("Downloaded {} records", history.len());

    Ok(())
}
```

## Platform Notes

Device identification varies by platform:

- **macOS**: Devices are identified by a UUID assigned by CoreBluetooth (stable per Mac, but differs between Macs)
- **Linux/Windows**: Devices are identified by their Bluetooth MAC address (e.g., `AA:BB:CC:DD:EE:FF`)

## Examples

Run the examples with:

```bash
# Scan for nearby devices
cargo run --example scan_devices

# Read current sensor values
cargo run --example read_sensor

# Download measurement history
cargo run --example download_history
```

## Related Crates

This crate is part of the [aranet](https://github.com/cameronrye/aranet) workspace:

| Crate | crates.io | Description |
|-------|-----------|-------------|
| [aranet-types](../aranet-types/) | [![crates.io](https://img.shields.io/crates/v/aranet-types.svg)](https://crates.io/crates/aranet-types) | Shared types for sensor data |
| [aranet-store](../aranet-store/) | [![crates.io](https://img.shields.io/crates/v/aranet-store.svg)](https://crates.io/crates/aranet-store) | Local data persistence |
| [aranet-cli](../aranet-cli/) | [![crates.io](https://img.shields.io/crates/v/aranet-cli.svg)](https://crates.io/crates/aranet-cli) | Command-line interface |
| [aranet-tui](../aranet-tui/) | [![crates.io](https://img.shields.io/crates/v/aranet-tui.svg)](https://crates.io/crates/aranet-tui) | Terminal UI dashboard |
| [aranet-service](../aranet-service/) | [![crates.io](https://img.shields.io/crates/v/aranet-service.svg)](https://crates.io/crates/aranet-service) | Background collector and REST API |
| [aranet-gui](../aranet-gui/) | [![crates.io](https://img.shields.io/crates/v/aranet-gui.svg)](https://crates.io/crates/aranet-gui) | Desktop GUI application |

## License

MIT

---

Made with ❤️ by [Cameron Rye](https://rye.dev/)
