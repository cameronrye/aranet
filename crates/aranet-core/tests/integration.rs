//! Integration tests for aranet-core
//!
//! These tests require actual BLE hardware and should be run with:
//! `cargo test --package aranet-core -- --ignored --nocapture`
//!
//! Set the ARANET_DEVICE environment variable to specify which device to test:
//! `ARANET_DEVICE="Aranet4 12345" cargo test --package aranet-core -- --ignored`
//!
//! If not set, tests will use "Aranet4" as the default device name.

use std::env;
use std::time::Duration;

use aranet_core::Device;
use aranet_core::scan::{ScanOptions, scan_with_options};
use aranet_core::settings::MeasurementInterval;
use aranet_core::types::{CurrentReading, Status};
use tokio::time::timeout;

// Suppress unused warnings for test-only items
#[allow(dead_code)]
/// Default timeout for BLE operations.
const BLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Get the device name from environment or use default.
fn get_device_name() -> String {
    env::var("ARANET_DEVICE").unwrap_or_else(|_| "Aranet4".to_string())
}

#[tokio::test]
#[ignore = "requires BLE hardware"]
async fn test_scan_for_devices() {
    // Use 15-second scan to catch multiple devices with different advertisement intervals
    let options = ScanOptions {
        duration: Duration::from_secs(15),
        filter_aranet_only: true,
    };

    let result = timeout(Duration::from_secs(30), scan_with_options(options)).await;

    match result {
        Ok(Ok(devices)) => {
            println!("Found {} devices", devices.len());
            for device in devices {
                println!(
                    "  {} ({})",
                    device.name.as_deref().unwrap_or("Unknown"),
                    device.address
                );
            }
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
async fn test_connect_and_read() {
    let device_name = get_device_name();
    println!("Connecting to device: {}", device_name);

    // Connect with timeout
    let connect_result = timeout(BLE_TIMEOUT, Device::connect(&device_name)).await;

    let device = match connect_result {
        Ok(Ok(d)) => d,
        Ok(Err(e)) => panic!("Failed to connect to {}: {}", device_name, e),
        Err(_) => panic!("Connection timed out after {:?}", BLE_TIMEOUT),
    };

    println!("Connected!");

    // Read with timeout
    let read_result = timeout(Duration::from_secs(10), device.read_current()).await;

    match read_result {
        Ok(Ok(reading)) => {
            println!("CO2: {} ppm", reading.co2);
            println!("Temperature: {:.1} °C", reading.temperature);
            println!("Humidity: {}%", reading.humidity);
            println!("Battery: {}%", reading.battery);
            println!("Status: {:?}", reading.status);
        }
        Ok(Err(e)) => {
            eprintln!("Failed to read: {}", e);
        }
        Err(_) => {
            eprintln!("Read timed out after 10 seconds");
        }
    }

    // Disconnect with timeout
    let _ = timeout(Duration::from_secs(5), device.disconnect()).await;
    println!("Disconnected.");
}

#[tokio::test]
#[ignore = "requires BLE hardware"]
async fn test_download_history() {
    let device_name = get_device_name();
    println!("Connecting to device: {}", device_name);

    // Connect with timeout
    let connect_result = timeout(BLE_TIMEOUT, Device::connect(&device_name)).await;

    let device = match connect_result {
        Ok(Ok(d)) => d,
        Ok(Err(e)) => panic!("Failed to connect to {}: {}", device_name, e),
        Err(_) => panic!("Connection timed out after {:?}", BLE_TIMEOUT),
    };

    println!("Connected!");

    // Get history info with timeout
    let info_result = timeout(Duration::from_secs(10), device.get_history_info()).await;

    match info_result {
        Ok(Ok(info)) => {
            println!("Total readings: {}", info.total_readings);
            println!("Interval: {} seconds", info.interval_seconds);
            println!("Last update: {} seconds ago", info.seconds_since_update);

            // Only download if there are readings
            if info.total_readings > 0 {
                // Download with longer timeout (can take a while)
                let download_result =
                    timeout(Duration::from_secs(120), device.download_history()).await;

                match download_result {
                    Ok(Ok(records)) => {
                        println!("Downloaded {} records", records.len());
                        if let Some(first) = records.first() {
                            println!("First: {:?}", first);
                        }
                        if let Some(last) = records.last() {
                            println!("Last: {:?}", last);
                        }
                    }
                    Ok(Err(e)) => {
                        eprintln!("Failed to download history: {}", e);
                    }
                    Err(_) => {
                        eprintln!("History download timed out after 120 seconds");
                    }
                }
            } else {
                println!("No readings to download");
            }
        }
        Ok(Err(e)) => {
            // This is expected on some devices that don't support history
            eprintln!("Failed to get history info: {}", e);
            eprintln!("This device may not support history download or may have older firmware.");
        }
        Err(_) => {
            eprintln!("Get history info timed out after 10 seconds");
        }
    }

    // Disconnect with timeout
    let _ = timeout(Duration::from_secs(5), device.disconnect()).await;
    println!("Disconnected.");
}

#[test]
fn test_types_are_serializable() {
    // Test that types can be serialized to JSON
    let reading = CurrentReading {
        co2: 800,
        temperature: 22.5,
        pressure: 1013.2,
        humidity: 45,
        battery: 85,
        status: Status::Green,
        interval: 300,
        age: 120,
        captured_at: None,
        radon: None,
        radiation_rate: None,
        radiation_total: None,
        radon_avg_24h: None,
        radon_avg_7d: None,
        radon_avg_30d: None,
    };

    let json = serde_json::to_string(&reading).unwrap();
    let parsed: CurrentReading = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.co2, reading.co2);
    assert_eq!(parsed.temperature, reading.temperature);
}

#[test]
fn test_measurement_interval() {
    assert_eq!(
        MeasurementInterval::from_minutes(1),
        Some(MeasurementInterval::OneMinute)
    );
    assert_eq!(
        MeasurementInterval::from_minutes(5),
        Some(MeasurementInterval::FiveMinutes)
    );
    assert_eq!(MeasurementInterval::OneMinute.as_seconds(), 60);
    assert_eq!(MeasurementInterval::TenMinutes.as_seconds(), 600);
}

// =============================================================================
// Mock-based integration tests (no BLE hardware required)
// =============================================================================

use aranet_core::history::HistoryOptions;
use aranet_core::{AranetDevice, MockDevice, MockDeviceBuilder};
use aranet_types::{DeviceType, HistoryRecord};

/// Test full device lifecycle: connect -> read -> disconnect
#[tokio::test]
async fn test_mock_device_full_lifecycle() {
    // Create device (not connected)
    let device = MockDeviceBuilder::new()
        .name("Test Aranet4")
        .device_type(DeviceType::Aranet4)
        .co2(850)
        .temperature(23.5)
        .humidity(55)
        .battery(90)
        .auto_connect(false)
        .build();

    // Verify initially not connected
    assert!(!device.is_connected().await);

    // Connect
    device.connect().await.expect("Connection should succeed");
    assert!(device.is_connected().await);

    // Read current values
    let reading = device.read_current().await.expect("Read should succeed");
    assert_eq!(reading.co2, 850);
    assert!((reading.temperature - 23.5).abs() < 0.01);
    assert_eq!(reading.humidity, 55);
    assert_eq!(reading.battery, 90);

    // Read device info
    let info = device
        .read_device_info()
        .await
        .expect("Device info should succeed");
    assert_eq!(info.name, "Test Aranet4");
    assert!(info.manufacturer.contains("SAF"));

    // Read RSSI
    let rssi = device.read_rssi().await.expect("RSSI should succeed");
    assert!(rssi < 0); // RSSI is negative dBm

    // Disconnect
    device
        .disconnect()
        .await
        .expect("Disconnect should succeed");
    assert!(!device.is_connected().await);

    // Verify operations fail after disconnect
    let result = device.read_current().await;
    assert!(result.is_err());
}

/// Test history download with mock device
#[tokio::test]
async fn test_mock_device_history_download() {
    let device = MockDeviceBuilder::new()
        .device_type(DeviceType::Aranet4)
        .build();

    // Add some history records
    let now = time::OffsetDateTime::now_utc();
    let records: Vec<HistoryRecord> = (0..10)
        .map(|i| HistoryRecord {
            timestamp: now - time::Duration::minutes(i * 5),
            co2: 800 + (i as u16 * 10),
            temperature: 22.0 + (i as f32 * 0.1),
            pressure: 1013.0,
            humidity: 50,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
        })
        .collect();

    device.add_history(records.clone()).await;

    // Get history info
    let info = device
        .get_history_info()
        .await
        .expect("History info should succeed");
    assert_eq!(info.total_readings, 10);

    // Download all history
    let downloaded = device
        .download_history()
        .await
        .expect("Download should succeed");
    assert_eq!(downloaded.len(), 10);
    assert_eq!(downloaded[0].co2, 800);

    // Download with options (partial range)
    let options = HistoryOptions::default().start_index(2).end_index(5);
    let partial = device
        .download_history_with_options(options)
        .await
        .expect("Partial download should succeed");
    assert_eq!(partial.len(), 3); // indices 2, 3, 4
}

/// Test transient failure handling (simulates retry scenarios)
#[tokio::test]
async fn test_mock_device_transient_failures() {
    let device = MockDevice::new("Test", DeviceType::Aranet4);

    // Configure 2 transient failures before success
    device.set_transient_failures(2);

    // First connect attempt should fail
    let result1 = device.connect().await;
    assert!(result1.is_err());
    assert_eq!(device.remaining_failures(), 1);

    // Second connect attempt should fail
    let result2 = device.connect().await;
    assert!(result2.is_err());
    assert_eq!(device.remaining_failures(), 0);

    // Third connect attempt should succeed
    let result3 = device.connect().await;
    assert!(result3.is_ok());
    assert!(device.is_connected().await);
}

/// Test permanent failure mode
#[tokio::test]
async fn test_mock_device_permanent_failure() {
    let device = MockDeviceBuilder::new().build();

    // Verify initial reads work
    let reading = device.read_current().await;
    assert!(reading.is_ok());

    // Set permanent failure mode
    device
        .set_should_fail(true, Some("Simulated BLE error"))
        .await;

    // All operations should now fail
    let result = device.read_current().await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Simulated BLE error")
    );

    let result = device.read_battery().await;
    assert!(result.is_err());

    // Disable failure mode
    device.set_should_fail(false, None).await;

    // Operations should work again
    let reading = device.read_current().await;
    assert!(reading.is_ok());
}

/// Test reading updates during device lifetime
#[tokio::test]
async fn test_mock_device_reading_updates() {
    let device = MockDeviceBuilder::new().co2(800).temperature(22.0).build();

    // Initial reading
    let reading1 = device.read_current().await.unwrap();
    assert_eq!(reading1.co2, 800);

    // Update CO2
    device.set_co2(1200).await;
    let reading2 = device.read_current().await.unwrap();
    assert_eq!(reading2.co2, 1200);

    // Update temperature
    device.set_temperature(25.5).await;
    let reading3 = device.read_current().await.unwrap();
    assert!((reading3.temperature - 25.5).abs() < 0.01);

    // Verify read count tracking
    assert_eq!(device.read_count(), 3);

    // Reset and verify
    device.reset_read_count();
    assert_eq!(device.read_count(), 0);
}

/// Test settings operations
#[tokio::test]
async fn test_mock_device_settings() {
    let device = MockDeviceBuilder::new().build();

    // Get initial interval
    let interval = device
        .get_interval()
        .await
        .expect("Get interval should succeed");
    assert_eq!(interval, MeasurementInterval::FiveMinutes);

    // Set new interval
    device
        .set_interval(MeasurementInterval::TenMinutes)
        .await
        .expect("Set interval should succeed");

    // Verify change
    let new_interval = device
        .get_interval()
        .await
        .expect("Get interval should succeed");
    assert_eq!(new_interval, MeasurementInterval::TenMinutes);

    // Get calibration data
    let calibration = device
        .get_calibration()
        .await
        .expect("Get calibration should succeed");
    assert!(calibration.co2_offset.is_some() || calibration.co2_offset.is_none()); // Just verify it returns
}

/// Test trait polymorphism - same code works with mock and real devices
#[tokio::test]
async fn test_aranet_device_trait_polymorphism() {
    // This function works with any AranetDevice implementation
    async fn read_via_trait<D: AranetDevice>(device: &D) -> u16 {
        device.read_current().await.unwrap().co2
    }

    async fn get_identity<D: AranetDevice>(device: &D) -> (Option<String>, String) {
        (
            device.name().map(String::from),
            device.address().to_string(),
        )
    }

    let device = MockDeviceBuilder::new()
        .name("Polymorphic Test")
        .co2(999)
        .build();

    // Use through trait bounds
    let co2 = read_via_trait(&device).await;
    assert_eq!(co2, 999);

    let (name, address) = get_identity(&device).await;
    assert_eq!(name.as_deref(), Some("Polymorphic Test"));
    assert!(address.starts_with("MOCK-"));
}

/// Test latency simulation
#[tokio::test]
async fn test_mock_device_latency_simulation() {
    let device = MockDeviceBuilder::new().build();

    // Set 50ms read latency
    device.set_read_latency(Duration::from_millis(50));

    let start = std::time::Instant::now();
    let _ = device.read_current().await;
    let elapsed = start.elapsed();

    // Should take at least 50ms (with some tolerance)
    assert!(
        elapsed >= Duration::from_millis(40),
        "Expected at least 40ms, got {:?}",
        elapsed
    );
}
