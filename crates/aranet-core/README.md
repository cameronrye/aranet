# aranet-core

Core BLE library for Aranet environmental sensors.

This crate provides low-level Bluetooth Low Energy (BLE) communication with Aranet sensors including the Aranet4, Aranet2, AranetRn+ (Radon), and Aranet Radiation devices.

## Features

- **Device discovery** — Scan for nearby Aranet devices via BLE
- **Current readings** — CO₂, temperature, pressure, humidity, radon, radiation
- **Historical data** — Download measurement history with timestamps
- **Device settings** — Read/write measurement interval, Bluetooth range
- **Auto-reconnection** — Configurable backoff and retry logic
- **Real-time streaming** — Subscribe to sensor value changes
- **Multi-device support** — Manage multiple sensors simultaneously

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
    let history = device.read_history().await?;
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

## License

MIT

---

Made with ❤️ by [Cameron Rye](https://rye.dev/)
