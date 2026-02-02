//! Example: Scanning for Aranet Devices
//!
//! This example demonstrates how to scan for Aranet devices
//! using Bluetooth Low Energy. It will discover any Aranet4,
//! Aranet2, Aranet Radon, or Aranet Radiation devices in range.
//!
//! Run with: `cargo run --example scan_devices`

use aranet_core::scan::{self, ScanOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    println!("Scanning for Aranet devices...");
    println!();

    // Scan with custom options
    let options = ScanOptions::default()
        .duration_secs(10)
        .filter_aranet_only(true);

    let devices = scan::scan_with_options(options).await?;

    if devices.is_empty() {
        println!("No Aranet devices found.");
        println!();
        println!("Make sure:");
        println!("  - Your Aranet device is powered on");
        println!("  - Bluetooth is enabled on this computer");
        println!("  - The device is within range");
    } else {
        println!("Found {} device(s):", devices.len());
        println!();

        for device in &devices {
            let name = device.name.as_deref().unwrap_or("Unknown");
            let device_type = device
                .device_type
                .map(|t| format!("{:?}", t))
                .unwrap_or_else(|| "Unknown".to_string());
            let rssi = device
                .rssi
                .map(|r| format!("{} dBm", r))
                .unwrap_or_else(|| "N/A".to_string());

            println!("  {} [{}]", name, device_type);
            println!("    Identifier: {}", device.identifier);
            println!("    RSSI: {}", rssi);
            if let Some(ref mfg_data) = device.manufacturer_data {
                println!("    Mfg Data: {:02X?}", mfg_data);
            }
            println!();
        }
    }

    Ok(())
}
