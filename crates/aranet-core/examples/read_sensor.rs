//! Example: Reading Current Sensor Values
//!
//! This example demonstrates how to connect to an Aranet device
//! and read the current sensor values including CO2, temperature,
//! humidity, pressure, and battery level.
//!
//! Run with: `cargo run --example read_sensor -- <DEVICE_ADDRESS>`

use std::env;

use aranet_core::Device;
use aranet_types::DeviceType;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Get device identifier from command line
    let args: Vec<String> = env::args().collect();
    let identifier = if args.len() > 1 {
        &args[1]
    } else {
        eprintln!("Usage: {} <DEVICE_ADDRESS_OR_NAME>", args[0]);
        eprintln!();
        eprintln!("Example:");
        eprintln!("  {} AA:BB:CC:DD:EE:FF", args[0]);
        eprintln!("  {} \"Aranet4 12345\"", args[0]);
        std::process::exit(1);
    };

    println!("Connecting to {}...", identifier);

    // Connect to the device
    let device = Device::connect(identifier).await?;
    println!("Connected!");
    println!();

    // Read current sensor values
    println!("Reading sensor values...");
    let reading = device.read_current().await?;
    let device_type = device.device_type();

    println!();
    println!("Current Readings:");

    // Display CO2 or Radon depending on device type
    match device_type {
        Some(DeviceType::AranetRadon) => {
            println!("  Radon:       {} Bq/m³", reading.co2);
        }
        Some(DeviceType::AranetRadiation) => {
            println!("  Radiation:   {} (raw)", reading.co2);
        }
        _ => {
            println!("  CO2:         {} ppm", reading.co2);
        }
    }

    println!("  Temperature: {:.1} °C", reading.temperature);
    println!("  Pressure:    {:.2} hPa", reading.pressure);
    println!("  Humidity:    {}%", reading.humidity);
    println!("  Battery:     {}%", reading.battery);
    println!("  Status:      {:?}", reading.status);
    if reading.interval > 0 {
        println!("  Interval:    {}s", reading.interval);
    }
    if reading.age > 0 {
        println!("  Age:         {}s since last measurement", reading.age);
    }

    // Read device info
    println!();
    println!("Device Information:");
    let info = device.read_device_info().await?;
    println!("  Name:         {}", info.name);
    println!("  Model:        {}", info.model);
    println!("  Serial:       {}", info.serial);
    println!("  Firmware:     {}", info.firmware);
    println!("  Hardware:     {}", info.hardware);
    println!("  Manufacturer: {}", info.manufacturer);

    // Disconnect
    device.disconnect().await?;
    println!();
    println!("Disconnected.");

    Ok(())
}
