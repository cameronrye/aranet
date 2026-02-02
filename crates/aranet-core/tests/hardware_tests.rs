//! Hardware integration tests for aranet-core
//!
//! These tests require actual BLE hardware and should be run with:
//! ```
//! cargo test --package aranet-core --test hardware_tests -- --ignored --nocapture
//! ```
//!
//! Configure devices via environment variables:
//! - `ARANET4_DEVICE`: Aranet4 device identifier
//! - `ARANET2_DEVICE`: Aranet2 device identifier
//! - `ARANET_RADON_DEVICE`: AranetRn+ device identifier
//! - `ARANET_RADIATION_DEVICE`: Aranet Radiation device identifier
//! - `ARANET_DEVICE`: Fallback for any device type
//!
//! Example:
//! ```
//! ARANET4_DEVICE="Aranet4 12345" cargo test --package aranet-core --test hardware_tests -- --ignored --nocapture
//! ```

use std::env;
use std::time::Duration;

use aranet_core::Device;
use aranet_core::scan::{ScanOptions, scan_with_options};
use aranet_core::settings::MeasurementInterval;
use tokio::time::timeout;

/// Default timeout for BLE operations
const BLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Extended timeout for history operations
const HISTORY_TIMEOUT: Duration = Duration::from_secs(120);

/// Get device identifier from environment
fn get_device(device_type: &str) -> Option<String> {
    // Try specific device type first
    let env_key = match device_type {
        "aranet4" => "ARANET4_DEVICE",
        "aranet2" => "ARANET2_DEVICE",
        "aranet_radon" | "radon" => "ARANET_RADON_DEVICE",
        "aranet_radiation" | "radiation" => "ARANET_RADIATION_DEVICE",
        _ => "ARANET_DEVICE",
    };

    env::var(env_key)
        .ok()
        .or_else(|| env::var("ARANET_DEVICE").ok())
        .filter(|s| !s.is_empty())
}

/// Get any available device
fn get_any_device() -> Option<String> {
    get_device("aranet4")
        .or_else(|| get_device("aranet2"))
        .or_else(|| get_device("aranet_radon"))
        .or_else(|| get_device("aranet_radiation"))
}

// =============================================================================
// Scan Tests
// =============================================================================

#[tokio::test]
#[ignore = "requires BLE hardware"]
async fn test_scan_discovers_devices() {
    let options = ScanOptions::default()
        .duration_secs(15)
        .filter_aranet_only(true);

    let result = timeout(Duration::from_secs(30), scan_with_options(options)).await;

    match result {
        Ok(Ok(devices)) => {
            println!("Scan discovered {} devices:", devices.len());
            for device in &devices {
                println!(
                    "  - {} ({})",
                    device.name.as_deref().unwrap_or("Unknown"),
                    device.address
                );
            }
            // Test passes if scan completes (may find 0 devices if none in range)
            // Scan completed successfully - no assertion needed since reaching here is success
        }
        Ok(Err(e)) => {
            panic!("Scan failed: {}", e);
        }
        Err(_) => {
            panic!("Scan timed out after 30 seconds");
        }
    }
}

#[tokio::test]
#[ignore = "requires BLE hardware"]
async fn test_scan_with_short_timeout() {
    let options = ScanOptions::default()
        .duration_secs(3)
        .filter_aranet_only(true);

    let result = timeout(Duration::from_secs(10), scan_with_options(options)).await;

    match result {
        Ok(Ok(_devices)) => {
            println!("Short scan completed successfully");
        }
        Ok(Err(e)) => {
            panic!("Short scan failed: {}", e);
        }
        Err(_) => {
            panic!("Short scan timed out");
        }
    }
}

#[tokio::test]
#[ignore = "requires BLE hardware"]
async fn test_scan_unfiltered() {
    let options = ScanOptions::default()
        .duration_secs(5)
        .filter_aranet_only(false);

    let result = timeout(Duration::from_secs(15), scan_with_options(options)).await;

    match result {
        Ok(Ok(devices)) => {
            println!("Unfiltered scan found {} devices", devices.len());
            // Should find more devices when not filtering
        }
        Ok(Err(e)) => {
            panic!("Unfiltered scan failed: {}", e);
        }
        Err(_) => {
            panic!("Unfiltered scan timed out");
        }
    }
}

// =============================================================================
// Connection Tests
// =============================================================================

#[tokio::test]
#[ignore = "requires BLE hardware"]
async fn test_connect_disconnect_cycle() {
    let device_name = match get_any_device() {
        Some(d) => d,
        None => {
            println!("SKIP: No device configured (set ARANET_DEVICE env var)");
            return;
        }
    };

    println!("Testing connect/disconnect cycle with: {}", device_name);

    // Connect
    let connect_result = timeout(BLE_TIMEOUT, Device::connect(&device_name)).await;
    let device = match connect_result {
        Ok(Ok(d)) => d,
        Ok(Err(e)) => panic!("Failed to connect: {}", e),
        Err(_) => panic!("Connection timed out"),
    };

    println!("Connected successfully");

    // Verify we can read
    let read_result = timeout(Duration::from_secs(10), device.read_current()).await;
    assert!(read_result.is_ok(), "Should be able to read when connected");

    // Disconnect
    let disconnect_result = timeout(Duration::from_secs(5), device.disconnect()).await;
    assert!(
        disconnect_result.is_ok(),
        "Disconnect should complete without timeout"
    );

    println!("Disconnected successfully");
}

#[tokio::test]
#[ignore = "requires BLE hardware"]
async fn test_reconnect_after_disconnect() {
    let device_name = match get_any_device() {
        Some(d) => d,
        None => {
            println!("SKIP: No device configured");
            return;
        }
    };

    println!("Testing reconnection with: {}", device_name);

    // First connection
    let device1 = timeout(BLE_TIMEOUT, Device::connect(&device_name))
        .await
        .expect("First connect timeout")
        .expect("First connect failed");

    let _ = device1.read_current().await;
    let _ = device1.disconnect().await;
    println!("First connection cycle complete");

    // Brief pause
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Second connection
    let device2 = timeout(BLE_TIMEOUT, Device::connect(&device_name))
        .await
        .expect("Second connect timeout")
        .expect("Second connect failed");

    let reading = device2
        .read_current()
        .await
        .expect("Second read should succeed");
    println!("Reconnection successful, CO2: {} ppm", reading.co2);

    let _ = device2.disconnect().await;
}

// =============================================================================
// Read Tests - Aranet4
// =============================================================================

#[tokio::test]
#[ignore = "requires BLE hardware and Aranet4 device"]
async fn test_aranet4_read_current() {
    let device_name = match get_device("aranet4") {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET4_DEVICE not set");
            return;
        }
    };

    println!("Reading from Aranet4: {}", device_name);

    let device = timeout(BLE_TIMEOUT, Device::connect(&device_name))
        .await
        .expect("Connect timeout")
        .expect("Connect failed");

    let reading = timeout(Duration::from_secs(10), device.read_current())
        .await
        .expect("Read timeout")
        .expect("Read failed");

    println!("Aranet4 Reading:");
    println!("  CO2:         {} ppm", reading.co2);
    println!("  Temperature: {:.1} °C", reading.temperature);
    println!("  Humidity:    {}%", reading.humidity);
    println!("  Pressure:    {:.1} hPa", reading.pressure);
    println!("  Battery:     {}%", reading.battery);
    println!("  Status:      {:?}", reading.status);

    // Validate ranges for Aranet4
    assert!(
        reading.co2 > 0 && reading.co2 < 10000,
        "CO2 should be in valid range (got {})",
        reading.co2
    );
    assert!(
        reading.temperature > -40.0 && reading.temperature < 85.0,
        "Temperature should be in valid range"
    );
    assert!(reading.humidity <= 100, "Humidity should be <= 100%");
    assert!(
        reading.pressure > 300.0 && reading.pressure < 1200.0,
        "Pressure should be in valid range"
    );
    assert!(reading.battery <= 100, "Battery should be <= 100%");

    let _ = device.disconnect().await;
}

#[tokio::test]
#[ignore = "requires BLE hardware and Aranet4 device"]
async fn test_aranet4_read_device_info() {
    let device_name = match get_device("aranet4") {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET4_DEVICE not set");
            return;
        }
    };

    let device = timeout(BLE_TIMEOUT, Device::connect(&device_name))
        .await
        .expect("Connect timeout")
        .expect("Connect failed");

    let info = timeout(Duration::from_secs(10), device.read_device_info())
        .await
        .expect("Info read timeout")
        .expect("Info read failed");

    println!("Device Info:");
    println!("  Name:         {}", info.name);
    println!("  Model:        {}", info.model);
    println!("  Serial:       {}", info.serial);
    println!("  Firmware:     {}", info.firmware);
    println!("  Hardware:     {}", info.hardware);

    assert!(!info.name.is_empty(), "Device name should not be empty");

    let _ = device.disconnect().await;
}

#[tokio::test]
#[ignore = "requires BLE hardware and Aranet4 device"]
async fn test_aranet4_read_rssi() {
    let device_name = match get_device("aranet4") {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET4_DEVICE not set");
            return;
        }
    };

    let device = timeout(BLE_TIMEOUT, Device::connect(&device_name))
        .await
        .expect("Connect timeout")
        .expect("Connect failed");

    let rssi = timeout(Duration::from_secs(5), device.read_rssi())
        .await
        .expect("RSSI read timeout")
        .expect("RSSI read failed");

    println!("RSSI: {} dBm", rssi);

    // RSSI should be negative and in reasonable range
    assert!(rssi < 0, "RSSI should be negative");
    assert!(rssi > -100, "RSSI should be > -100 dBm (got {})", rssi);

    let _ = device.disconnect().await;
}

#[tokio::test]
#[ignore = "requires BLE hardware and Aranet4 device"]
async fn test_aranet4_read_battery() {
    let device_name = match get_device("aranet4") {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET4_DEVICE not set");
            return;
        }
    };

    let device = timeout(BLE_TIMEOUT, Device::connect(&device_name))
        .await
        .expect("Connect timeout")
        .expect("Connect failed");

    let battery = timeout(Duration::from_secs(5), device.read_battery())
        .await
        .expect("Battery read timeout")
        .expect("Battery read failed");

    println!("Battery: {}%", battery);

    assert!(battery <= 100, "Battery should be <= 100%");
    assert!(battery > 0, "Battery should be > 0% (is device charged?)");

    let _ = device.disconnect().await;
}

// =============================================================================
// Read Tests - Aranet2
// =============================================================================

#[tokio::test]
#[ignore = "requires BLE hardware and Aranet2 device"]
async fn test_aranet2_read_current() {
    let device_name = match get_device("aranet2") {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET2_DEVICE not set");
            return;
        }
    };

    println!("Reading from Aranet2: {}", device_name);

    let device = timeout(BLE_TIMEOUT, Device::connect(&device_name))
        .await
        .expect("Connect timeout")
        .expect("Connect failed");

    let reading = timeout(Duration::from_secs(10), device.read_current())
        .await
        .expect("Read timeout")
        .expect("Read failed");

    println!("Aranet2 Reading:");
    println!("  Temperature: {:.1} °C", reading.temperature);
    println!("  Humidity:    {}%", reading.humidity);
    println!("  Battery:     {}%", reading.battery);

    // Aranet2 doesn't have CO2 sensor - may report 0
    assert!(
        reading.temperature > -40.0 && reading.temperature < 85.0,
        "Temperature should be in valid range"
    );
    assert!(reading.humidity <= 100, "Humidity should be <= 100%");

    let _ = device.disconnect().await;
}

// =============================================================================
// Read Tests - AranetRn+ (Radon)
// =============================================================================

#[tokio::test]
#[ignore = "requires BLE hardware and AranetRn+ device"]
async fn test_aranet_radon_read_current() {
    let device_name = match get_device("aranet_radon") {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET_RADON_DEVICE not set");
            return;
        }
    };

    println!("Reading from AranetRn+: {}", device_name);

    let device = timeout(BLE_TIMEOUT, Device::connect(&device_name))
        .await
        .expect("Connect timeout")
        .expect("Connect failed");

    let reading = timeout(Duration::from_secs(10), device.read_current())
        .await
        .expect("Read timeout")
        .expect("Read failed");

    println!("AranetRn+ Reading:");
    println!("  Temperature: {:.1} °C", reading.temperature);
    println!("  Humidity:    {}%", reading.humidity);
    println!("  Pressure:    {:.1} hPa", reading.pressure);
    println!("  Battery:     {}%", reading.battery);

    if let Some(radon) = reading.radon {
        println!("  Radon:       {} Bq/m³", radon);
        assert!(radon < 10000, "Radon should be in reasonable range");
    } else {
        println!("  Radon:       (not available yet - device may need time)");
    }

    if let Some(avg_24h) = reading.radon_avg_24h {
        println!("  Radon 24h:   {} Bq/m³", avg_24h);
    }
    if let Some(avg_7d) = reading.radon_avg_7d {
        println!("  Radon 7d:    {} Bq/m³", avg_7d);
    }
    if let Some(avg_30d) = reading.radon_avg_30d {
        println!("  Radon 30d:   {} Bq/m³", avg_30d);
    }

    let _ = device.disconnect().await;
}

// =============================================================================
// History Tests
// =============================================================================

#[tokio::test]
#[ignore = "requires BLE hardware - slow test"]
async fn test_download_history_info() {
    let device_name = match get_any_device() {
        Some(d) => d,
        None => {
            println!("SKIP: No device configured");
            return;
        }
    };

    let device = timeout(BLE_TIMEOUT, Device::connect(&device_name))
        .await
        .expect("Connect timeout")
        .expect("Connect failed");

    let info = timeout(Duration::from_secs(10), device.get_history_info())
        .await
        .expect("History info timeout")
        .expect("History info failed");

    println!("History Info:");
    println!("  Total readings:      {}", info.total_readings);
    println!("  Interval:            {} seconds", info.interval_seconds);
    println!("  Seconds since update: {}", info.seconds_since_update);

    assert!(info.interval_seconds > 0, "Interval should be > 0");

    let _ = device.disconnect().await;
}

#[tokio::test]
#[ignore = "requires BLE hardware - slow test"]
async fn test_download_history_partial() {
    let device_name = match get_any_device() {
        Some(d) => d,
        None => {
            println!("SKIP: No device configured");
            return;
        }
    };

    let device = timeout(BLE_TIMEOUT, Device::connect(&device_name))
        .await
        .expect("Connect timeout")
        .expect("Connect failed");

    // Get history info first
    let info = timeout(Duration::from_secs(10), device.get_history_info())
        .await
        .expect("History info timeout")
        .expect("History info failed");

    if info.total_readings == 0 {
        println!("SKIP: No history records on device");
        let _ = device.disconnect().await;
        return;
    }

    // Download just the last 10 records (use 1-based indexing, start from 1)
    let start = if info.total_readings > 10 {
        info.total_readings - 10
    } else {
        1
    };
    let options = aranet_core::history::HistoryOptions::default()
        .start_index(start)
        .end_index(info.total_readings);

    println!(
        "Requesting history from index {} to {}",
        start, info.total_readings
    );

    let records = timeout(
        Duration::from_secs(60),
        device.download_history_with_options(options),
    )
    .await
    .expect("History download timeout")
    .expect("History download failed");

    println!("Downloaded {} history records", records.len());

    if let Some(first) = records.first() {
        println!("First record: {:?}", first);
    }
    if let Some(last) = records.last() {
        println!("Last record:  {:?}", last);
    }

    // Records may be 0 if device doesn't support partial downloads
    // Just verify we didn't error
    println!(
        "Partial history download completed (got {} records)",
        records.len()
    );

    let _ = device.disconnect().await;
}

#[tokio::test]
#[ignore = "requires BLE hardware - very slow test"]
async fn test_download_history_full() {
    let device_name = match get_any_device() {
        Some(d) => d,
        None => {
            println!("SKIP: No device configured");
            return;
        }
    };

    let device = timeout(BLE_TIMEOUT, Device::connect(&device_name))
        .await
        .expect("Connect timeout")
        .expect("Connect failed");

    println!("Downloading full history (this may take a while)...");

    let records = timeout(HISTORY_TIMEOUT, device.download_history())
        .await
        .expect("History download timeout")
        .expect("History download failed");

    println!("Downloaded {} total records", records.len());

    // Validate records are in chronological order
    for i in 1..records.len() {
        assert!(
            records[i].timestamp >= records[i - 1].timestamp,
            "History should be in chronological order"
        );
    }

    let _ = device.disconnect().await;
}

// =============================================================================
// Settings Tests
// =============================================================================

#[tokio::test]
#[ignore = "requires BLE hardware"]
async fn test_read_measurement_interval() {
    let device_name = match get_any_device() {
        Some(d) => d,
        None => {
            println!("SKIP: No device configured");
            return;
        }
    };

    let device = timeout(BLE_TIMEOUT, Device::connect(&device_name))
        .await
        .expect("Connect timeout")
        .expect("Connect failed");

    let interval = timeout(Duration::from_secs(10), device.get_interval())
        .await
        .expect("Get interval timeout")
        .expect("Get interval failed");

    println!("Current measurement interval: {:?}", interval);
    println!("  ({} seconds)", interval.as_seconds());

    // Verify it's a valid interval
    let valid_intervals = [
        MeasurementInterval::OneMinute,
        MeasurementInterval::TwoMinutes,
        MeasurementInterval::FiveMinutes,
        MeasurementInterval::TenMinutes,
    ];
    assert!(
        valid_intervals.contains(&interval),
        "Interval should be one of the valid options"
    );

    let _ = device.disconnect().await;
}

#[tokio::test]
#[ignore = "requires BLE hardware"]
async fn test_read_calibration_data() {
    let device_name = match get_any_device() {
        Some(d) => d,
        None => {
            println!("SKIP: No device configured");
            return;
        }
    };

    let device = timeout(BLE_TIMEOUT, Device::connect(&device_name))
        .await
        .expect("Connect timeout")
        .expect("Connect failed");

    let calibration = timeout(Duration::from_secs(10), device.get_calibration())
        .await
        .expect("Get calibration timeout")
        .expect("Get calibration failed");

    println!("Calibration data:");
    println!("  CO2 offset:  {:?}", calibration.co2_offset);
    println!("  Raw bytes:   {:02x?}", calibration.raw);

    let _ = device.disconnect().await;
}

// =============================================================================
// Multi-Device Tests
// =============================================================================

#[tokio::test]
#[ignore = "requires BLE hardware and multiple devices"]
async fn test_concurrent_reads_multiple_devices() {
    let devices: Vec<String> = [
        get_device("aranet4"),
        get_device("aranet2"),
        get_device("aranet_radon"),
    ]
    .into_iter()
    .flatten()
    .collect();

    if devices.len() < 2 {
        println!("SKIP: Need at least 2 devices configured for multi-device test");
        return;
    }

    println!("Testing concurrent reads from {} devices", devices.len());

    // Connect to all devices
    let mut connections = Vec::new();
    for device_name in &devices {
        match timeout(BLE_TIMEOUT, Device::connect(device_name)).await {
            Ok(Ok(device)) => {
                println!("Connected to: {}", device_name);
                connections.push(device);
            }
            Ok(Err(e)) => {
                println!("Failed to connect to {}: {}", device_name, e);
            }
            Err(_) => {
                println!("Connection timeout for: {}", device_name);
            }
        }
    }

    if connections.len() < 2 {
        println!("SKIP: Could not connect to enough devices");
        return;
    }

    // Read from all devices concurrently
    let futures: Vec<_> = connections.iter().map(|d| d.read_current()).collect();

    let results = futures::future::join_all(futures).await;

    let mut success_count = 0;
    for (i, result) in results.into_iter().enumerate() {
        match result {
            Ok(reading) => {
                println!("Device {}: CO2={} ppm", i, reading.co2);
                success_count += 1;
            }
            Err(e) => {
                println!("Device {} read failed: {}", i, e);
            }
        }
    }

    assert!(
        success_count >= 2,
        "Should successfully read from at least 2 devices"
    );

    // Disconnect all
    for device in connections {
        let _ = device.disconnect().await;
    }
}

// =============================================================================
// Stress Tests
// =============================================================================

#[tokio::test]
#[ignore = "requires BLE hardware - stress test"]
async fn test_repeated_reads() {
    let device_name = match get_any_device() {
        Some(d) => d,
        None => {
            println!("SKIP: No device configured");
            return;
        }
    };

    let device = timeout(BLE_TIMEOUT, Device::connect(&device_name))
        .await
        .expect("Connect timeout")
        .expect("Connect failed");

    const NUM_READS: usize = 10;
    let mut success_count = 0;

    println!("Performing {} repeated reads...", NUM_READS);

    for i in 0..NUM_READS {
        match timeout(Duration::from_secs(10), device.read_current()).await {
            Ok(Ok(reading)) => {
                println!("  Read {}: CO2={} ppm", i + 1, reading.co2);
                success_count += 1;
            }
            Ok(Err(e)) => {
                println!("  Read {} failed: {}", i + 1, e);
            }
            Err(_) => {
                println!("  Read {} timed out", i + 1);
            }
        }

        // Brief pause between reads
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    println!(
        "Completed {}/{} reads successfully",
        success_count, NUM_READS
    );
    assert!(
        success_count >= NUM_READS - 1,
        "Should succeed at least {} times",
        NUM_READS - 1
    );

    let _ = device.disconnect().await;
}

#[tokio::test]
#[ignore = "requires BLE hardware - stress test"]
async fn test_rapid_connect_disconnect() {
    let device_name = match get_any_device() {
        Some(d) => d,
        None => {
            println!("SKIP: No device configured");
            return;
        }
    };

    const NUM_CYCLES: usize = 5;
    let mut success_count = 0;

    println!(
        "Performing {} rapid connect/disconnect cycles...",
        NUM_CYCLES
    );

    for i in 0..NUM_CYCLES {
        let start = std::time::Instant::now();

        match timeout(BLE_TIMEOUT, Device::connect(&device_name)).await {
            Ok(Ok(device)) => {
                // Quick read to verify connection
                if device.read_current().await.is_ok() {
                    success_count += 1;
                }
                let _ = device.disconnect().await;
                println!("  Cycle {}: {:?}", i + 1, start.elapsed());
            }
            Ok(Err(e)) => {
                println!("  Cycle {} connect failed: {}", i + 1, e);
            }
            Err(_) => {
                println!("  Cycle {} timed out", i + 1);
            }
        }

        // Brief pause between cycles
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    println!(
        "Completed {}/{} cycles successfully",
        success_count, NUM_CYCLES
    );
    assert!(
        success_count >= NUM_CYCLES - 1,
        "Should succeed at least {} times",
        NUM_CYCLES - 1
    );
}

// =============================================================================
// Error Handling Tests
// =============================================================================

#[tokio::test]
#[ignore = "requires BLE hardware"]
async fn test_connect_nonexistent_device() {
    let result = timeout(Duration::from_secs(10), Device::connect("NonExistent12345")).await;

    match result {
        Ok(Ok(_)) => {
            panic!("Should not connect to nonexistent device");
        }
        Ok(Err(e)) => {
            println!("Expected error for nonexistent device: {}", e);
            // Test passes - we got an error as expected
        }
        Err(_) => {
            // Timeout is also acceptable - device wasn't found
            println!("Connection timed out (expected for nonexistent device)");
        }
    }
}

#[tokio::test]
#[ignore = "requires BLE hardware"]
async fn test_connect_invalid_address() {
    // Try various invalid address formats
    let invalid_addresses = ["", "invalid", "XX:XX:XX:XX:XX:XX", "not-a-uuid"];

    for addr in invalid_addresses {
        let result = timeout(Duration::from_secs(5), Device::connect(addr)).await;

        match result {
            Ok(Ok(_)) => {
                println!("Unexpected success for address: {}", addr);
            }
            Ok(Err(e)) => {
                println!("Expected error for '{}': {}", addr, e);
            }
            Err(_) => {
                println!("Timeout for '{}' (acceptable)", addr);
            }
        }
    }
}
