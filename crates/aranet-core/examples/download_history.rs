//! Example: Downloading Historical Data
//!
//! This example demonstrates how to download historical sensor
//! readings from an Aranet device. The device stores measurements
//! at regular intervals which can be retrieved for analysis.
//!
//! Run with: `cargo run --example download_history -- <DEVICE_ADDRESS>`

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

    let is_radon = matches!(device.device_type(), Some(DeviceType::AranetRadon));

    // Get history info
    println!("Reading history information...");
    let info = device.get_history_info().await?;
    println!("  Total readings: {}", info.total_readings);
    println!("  Interval: {} seconds", info.interval_seconds);
    println!("  Last update: {} seconds ago", info.seconds_since_update);
    println!();

    // Download history
    println!("Downloading history (this may take a moment)...");
    let records = device.download_history().await?;

    println!();
    println!("Downloaded {} records:", records.len());
    println!();

    // Print header based on device type
    if is_radon {
        println!(
            "{:<25} {:>10} {:>10} {:>10} {:>8}",
            "Timestamp", "Radon", "Temp", "Pressure", "Humidity"
        );
        println!("{}", "-".repeat(68));
    } else {
        println!(
            "{:<25} {:>8} {:>10} {:>10} {:>8}",
            "Timestamp", "CO2", "Temp", "Pressure", "Humidity"
        );
        println!("{}", "-".repeat(65));
    }

    // Show last 10 records (or all if fewer)
    let start = if records.len() > 10 {
        records.len() - 10
    } else {
        0
    };

    for record in &records[start..] {
        let timestamp = record
            .timestamp
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();

        if is_radon {
            println!(
                "{:<25} {:>6} Bq/m³ {:>7.1} °C {:>8.2} hPa {:>6}%",
                timestamp,
                record.radon.unwrap_or(0),
                record.temperature,
                record.pressure,
                record.humidity
            );
        } else {
            println!(
                "{:<25} {:>6} ppm {:>8.1} °C {:>8.2} hPa {:>6}%",
                timestamp, record.co2, record.temperature, record.pressure, record.humidity
            );
        }
    }

    if records.len() > 10 {
        println!();
        println!("(Showing last 10 of {} records)", records.len());
    }

    // Disconnect
    device.disconnect().await?;
    println!();
    println!("Disconnected.");

    Ok(())
}
