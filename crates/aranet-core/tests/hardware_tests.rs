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
use btleplug::api::Central as _;
use futures::{Stream, StreamExt};
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

/// Reads `events` until none arrives for `gap`. Returns how many it read, or
/// `Err` with that count if events were still arriving after `limit`.
///
/// Each read waits up to `gap`: on BlueZ, btleplug makes a D-Bus call for each
/// event before it yields it, so even an event that is already queued isn't
/// ready at once, and a `now_or_never` drain would stop at the first one.
async fn read_until_quiet<S: Stream + Unpin>(
    events: &mut S,
    gap: Duration,
    limit: Duration,
) -> Result<usize, usize> {
    let deadline = tokio::time::Instant::now() + limit;
    let mut count = 0;
    while tokio::time::Instant::now() < deadline {
        match timeout(gap, events.next()).await {
            Ok(Some(_)) => count += 1,
            Ok(None) => panic!("the Bluetooth event stream ended"),
            Err(_) => return Ok(count),
        }
    }
    Err(count)
}

/// A scan whose caller gives up must stop the radio, and the process must be
/// able to scan again. The test gives up on a 30 s scan after 3 s and reads
/// the events the scan left queued until the stream has been quiet for 1 s; a
/// scan that still runs never lets it go quiet, and the test fails after 15 s.
/// It then checks that no event arrives in the next 5 s, and scans again.
/// Before the fix, a dropped scan never called `stop_scan`: CoreBluetooth kept
/// scanning (with duplicates) and events kept arriving, and on Linux every
/// later scan in the process failed with `org.bluez.Error.InProgress`.
#[tokio::test]
#[ignore = "requires BLE hardware: Aranet devices advertising nearby"]
async fn test_cancelled_scan_stops_scanning() {
    if get_any_device().is_none() {
        println!("SKIP: No device configured (set ARANET_DEVICE env var)");
        return;
    }
    let adapter = aranet_core::scan::get_adapter().await.expect("adapter");
    let mut events = adapter.events().await.expect("event stream");

    let options = ScanOptions::default().duration_secs(30);
    let scan = timeout(
        Duration::from_secs(3),
        aranet_core::scan::scan_with_adapter(&adapter, options),
    )
    .await;
    assert!(
        scan.is_err(),
        "the 30 s scan should still be running after 3 s"
    );

    // The events the scan saw are queued in the stream. Count them for at most
    // 1 s, or until 0.5 s passes without one.
    let (Ok(while_scanning) | Err(while_scanning)) = read_until_quiet(
        &mut events,
        Duration::from_millis(500),
        Duration::from_secs(1),
    )
    .await;
    assert!(
        while_scanning > 0,
        "no Bluetooth events while scanning; are sensors advertising nearby?"
    );

    // Read the rest of what the scan left until the stream goes quiet, which it
    // never does while a scan runs.
    let quiet =
        read_until_quiet(&mut events, Duration::from_secs(1), Duration::from_secs(15)).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut after_cancel = 0;
    while let Ok(Some(_)) = tokio::time::timeout_at(deadline, events.next()).await {
        after_cancel += 1;
    }

    // Scan again in the same process: on Linux, a scan left running makes this
    // fail with org.bluez.Error.InProgress, and a scan permit that is never
    // released would make it wait forever. 40 s covers a stop that BlueZ
    // answers only at bluez-async's 30 s D-Bus call timeout.
    let options = ScanOptions::default().duration_secs(2);
    let rescan = timeout(
        Duration::from_secs(40),
        aranet_core::scan::scan_with_adapter(&adapter, options),
    )
    .await;
    let rescan_result = match &rescan {
        Ok(Ok(devices)) => format!("found {} devices", devices.len()),
        Ok(Err(e)) => format!("failed: {e}"),
        Err(_) => "did not finish within 40 s".to_string(),
    };
    let drained = match quiet {
        Ok(n) => format!("{n} more until the stream went quiet"),
        Err(n) => format!("{n} more in 15 s without a quiet second"),
    };
    println!(
        "{while_scanning} events while scanning, {drained}, {after_cancel} in the 5 s after \
         that; the next scan {rescan_result}"
    );
    assert!(
        quiet.is_ok(),
        "Bluetooth events were still arriving 15 s after the scan was cancelled: it is still scanning"
    );
    assert_eq!(
        after_cancel, 0,
        "Bluetooth events kept arriving after the scan was cancelled: it is still scanning"
    );
    rescan
        .expect("the scan after the cancelled one never finished: the permit is still held")
        .expect("a scan after the cancelled one failed");
}

/// Two scans in one process must run one after the other. Before the fix, the
/// second scan's stop also ended the first on macOS (`get_adapter` gives every
/// caller the same `CBCentralManager`), and on Linux the second failed with
/// `org.bluez.Error.InProgress`.
#[tokio::test]
#[ignore = "requires BLE hardware"]
async fn test_concurrent_scans_are_serialised() {
    // Create the adapter first: creating it can take over a second, and the
    // second scan must not start before the first has the permit.
    aranet_core::scan::get_adapter().await.expect("adapter");
    let started = std::time::Instant::now();
    let first = scan_with_options(ScanOptions::default().duration_secs(10));
    let second = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let devices = scan_with_options(ScanOptions::default().duration_secs(2)).await;
        (devices, started.elapsed())
    };
    let (first, (second, second_done_at)) = timeout(Duration::from_secs(60), async {
        tokio::join!(first, second)
    })
    .await
    .expect("scans did not finish within 60 s");
    let first = first.expect("first scan failed");
    let second = second.expect("second scan failed");
    println!(
        "first scan found {}, second found {} and finished after {second_done_at:?}",
        first.len(),
        second.len()
    );

    assert!(
        second_done_at >= Duration::from_secs(11),
        "the second scan ended after {second_done_at:?}, so it ran during the first"
    );
    if get_any_device().is_some() {
        assert!(
            !first.is_empty() && !second.is_empty(),
            "both scans should find the configured sensors"
        );
    } else {
        println!("No device configured: not checking that both scans found sensors");
    }
}

/// Two searches that need a scan at the same time share it: the one that waits
/// for the scan permit looks at what the running scan found before scanning
/// itself (CL-25). Before the fix both searches scanned. Both look for the same
/// sensor, so it doesn't matter which scan first sees it. Run the test on its
/// own: in a process that has scanned already, the sensor is known before
/// either search starts, and the test skips.
///
/// When a scan ends, the scanning search's post-scan check and the waiting
/// search's re-check run at the same moment. Rarely, CoreBluetooth reports the
/// sensor just after the stop, and only one of the two checks sees it. If the
/// scanning search misses it, it finds the sensor in its next attempt's
/// re-check, both searches end with `CacheHit`, and the test passes. If the
/// waiting search misses it, it scans for itself and the test fails: rerun it
/// once. Without the re-check the test fails on every run.
#[tokio::test]
#[ignore = "requires BLE hardware: Aranet devices advertising nearby"]
async fn test_concurrent_finds_share_one_scan() {
    use std::sync::{Arc, Mutex};

    use aranet_core::scan::{FindProgress, ProgressCallback, find_device_with_progress};

    let Some(device) = get_any_device() else {
        println!("SKIP: No device configured (set ARANET_DEVICE env var)");
        return;
    };
    let adapter = aranet_core::scan::get_adapter().await.expect("adapter");
    let known = adapter.peripherals().await.expect("known devices").len();
    if known > 0 {
        println!("SKIP: this process knows {known} devices already; run this test on its own");
        return;
    }

    let search = |identifier: String| async move {
        let events = Arc::new(Mutex::new(Vec::new()));
        let progress: ProgressCallback = Box::new({
            let events = Arc::clone(&events);
            move |event| events.lock().unwrap().push(event)
        });
        let options = ScanOptions::default().duration_secs(10);
        if let Err(e) = find_device_with_progress(&identifier, options, Some(progress)).await {
            panic!("could not find {identifier}: {e}");
        }
        events.lock().unwrap().clone()
    };
    let started = std::time::Instant::now();
    let (a, b) = timeout(Duration::from_secs(90), async {
        tokio::join!(search(device.clone()), search(device))
    })
    .await
    .expect("the searches did not finish within 90 s");
    println!(
        "the searches reported {a:?} and {b:?}, and finished after {:?}",
        started.elapsed()
    );

    // The device list was empty when both searches began, so a `CacheHit` can
    // only come from the check made after waiting for the permit. Two are fine:
    // see the rare case in the doc comment.
    let reused = [&a, &b]
        .into_iter()
        .filter(|events| matches!(events.last(), Some(FindProgress::CacheHit)))
        .count();
    assert!(
        reused >= 1,
        "neither search found the sensor in the other's scan: each scanned for it itself"
    );
}

/// A cancelled passive monitor must stop at once, even while it waits for its
/// next scan cycle. Before the fix it finished its current wait first, which
/// could be up to 300 s.
#[tokio::test]
#[ignore = "requires BLE hardware: a Bluetooth adapter"]
async fn test_cancelled_passive_monitor_stops_during_its_wait() {
    use aranet_core::{PassiveMonitor, PassiveMonitorOptions};
    use tokio_util::sync::CancellationToken;

    // Fail here rather than let the monitor wait for an adapter, a wait that
    // already honours cancel.
    aranet_core::scan::get_adapter().await.expect("adapter");

    let options = PassiveMonitorOptions::default()
        .scan_duration(Duration::from_millis(500))
        .scan_interval(Duration::from_secs(300));
    let monitor = std::sync::Arc::new(PassiveMonitor::new(options));
    let cancel = CancellationToken::new();
    let handle = monitor.start(cancel.clone());

    // After one 0.5 s scan cycle the monitor waits 300 s for the next one.
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(!handle.is_finished(), "the monitor stopped on its own");
    cancel.cancel();
    timeout(Duration::from_secs(2), handle)
        .await
        .expect("the passive monitor was still running 2 s after it was cancelled")
        .expect("the passive monitor task panicked");
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

/// BR-15: a connection check must pass on an unpaired sensor whose current
/// readings work, and must not start pairing. It used to read Battery Level
/// (0x2A19), which needs pairing.
#[tokio::test]
#[ignore = "requires BLE hardware (unpaired Aranet2 / AranetRn+)"]
async fn test_validate_connection_without_pairing() {
    let mut devices: Vec<String> = [get_device("aranet2"), get_device("aranet_radon")]
        .into_iter()
        .flatten()
        .collect();
    // Both lookups fall back to ARANET_DEVICE, so they can name the same sensor.
    devices.dedup();
    if devices.is_empty() {
        println!("SKIP: ARANET2_DEVICE / ARANET_RADON_DEVICE not set");
        return;
    }

    let mut failed = Vec::new();
    for dev in devices {
        // A failed connect says nothing about the check: record it and go on
        // with the next sensor.
        let device = match timeout(BLE_TIMEOUT, Device::connect(&dev)).await {
            Ok(Ok(device)) => device,
            Ok(Err(e)) => {
                println!("{dev}: connect failed: {e}");
                failed.push(format!("{dev} (connect failed)"));
                continue;
            }
            Err(_) => {
                println!("{dev}: connect timed out after {BLE_TIMEOUT:?}");
                failed.push(format!("{dev} (connect timed out)"));
                continue;
            }
        };
        let valid = device.validate_connection().await;
        println!("{dev}: validate_connection = {valid}");
        if !valid {
            // Why it failed: the read the check makes, with a longer limit.
            // Never read the battery level here. It needs pairing, so on an
            // unpaired sensor it starts a pairing (a passkey window on macOS,
            // a pairing request on Linux), which could pair the sensors this
            // test needs unpaired.
            let current = timeout(Duration::from_secs(10), device.read_current())
                .await
                .map(|r| r.map(|_| ()));
            println!("{dev}: read_current = {current:?}");
            failed.push(dev);
        }
        let _ = device.disconnect().await;
    }
    assert!(
        failed.is_empty(),
        "no passing connection check for {failed:?}"
    );
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

/// An absent device costs one search (scans of 5, 10 and 15 s), not a second
/// search sized by the connect timeout (BR-10).
#[tokio::test]
#[ignore = "requires BLE hardware (a Bluetooth adapter; no sensor needed)"]
async fn test_connect_to_missing_device_gives_up_within_scan_budget() {
    let started = std::time::Instant::now();
    let result = timeout(
        Duration::from_secs(120),
        Device::connect("NonExistent12345"),
    )
    .await
    .expect("Device::connect did not return within 120 s");
    assert!(
        matches!(result, Err(aranet_core::Error::DeviceNotFound(_))),
        "{result:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(45),
        "took {:?}",
        started.elapsed()
    );
}

#[tokio::test]
#[ignore = "requires BLE hardware"]
async fn test_connect_invalid_address() {
    // Try various invalid address formats
    let invalid_addresses = ["", "invalid", "XX:XX:XX:XX:XX:XX", "not-a-uuid"];

    for addr in invalid_addresses {
        let result = timeout(Duration::from_secs(5), Device::connect(addr)).await;

        if addr.is_empty() {
            // Rejected before Bluetooth is used, so it can neither time out nor
            // connect to whichever device the adapter lists first.
            assert!(
                matches!(result, Ok(Err(aranet_core::Error::InvalidConfig(_)))),
                "the empty identifier was not rejected: {result:?}"
            );
            println!("Rejected the empty identifier");
            continue;
        }

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

// =============================================================================
// Connect Cleanup Tests (BR-3)
// =============================================================================

/// Finds `device` and fails the test if the Bluetooth stack already reports it
/// connected: an earlier connection would make the cleanup check meaningless.
async fn find_disconnected_sensor(
    device: &str,
) -> (btleplug::platform::Adapter, btleplug::platform::Peripheral) {
    let (adapter, peripheral) = aranet_core::scan::find_device(device).await.expect("find");
    let state = timeout(
        Duration::from_secs(5),
        btleplug::api::Peripheral::is_connected(&peripheral),
    )
    .await;
    assert!(
        !matches!(state, Ok(Ok(true))),
        "{device} is already connected to this computer; wait for that connection to end and run the test again"
    );
    (adapter, peripheral)
}

/// Waits long enough for CoreBluetooth to finish a connect nobody cancelled,
/// finds `device` again and asks the Bluetooth stack whether it is connected.
///
/// The search scans for up to 30 s: after a disconnect, CoreBluetooth forgets
/// the peripheral, and the sensor can take a while to advertise again.
async fn connection_state_after_settling(
    device: &str,
) -> Result<btleplug::Result<bool>, tokio::time::error::Elapsed> {
    tokio::time::sleep(Duration::from_secs(8)).await;
    let options = aranet_core::scan::ScanOptions::default().duration_secs(10);
    let (_adapter, peripheral) = aranet_core::scan::find_device_with_options(device, options)
        .await
        .expect("find the sensor again");
    timeout(
        Duration::from_secs(5),
        btleplug::api::Peripheral::is_connected(&peripheral),
    )
    .await
}

/// A connect that times out must release the sensor (BR-3). Dropping a
/// btleplug connect future doesn't cancel CoreBluetooth's `connectPeripheral`,
/// so before the fix the abandoned connect completed a few seconds later and
/// the sensor stayed connected to this computer.
#[tokio::test]
#[ignore = "requires BLE hardware and an Aranet2 device"]
async fn test_timed_out_connect_leaves_sensor_disconnected() {
    let Some(device_name) = get_device("aranet2") else {
        println!("SKIP: ARANET2_DEVICE not set");
        return;
    };
    let (adapter, peripheral) = find_disconnected_sensor(&device_name).await;

    let config =
        aranet_core::ConnectionConfig::default().connection_timeout(Duration::from_millis(50));
    match Device::from_peripheral_with_config(adapter, peripheral, config).await {
        Err(aranet_core::Error::Timeout { .. }) => {}
        Err(e) => panic!("expected the 50 ms connect to time out, got: {e}"),
        Ok(device) => {
            let _ = device.disconnect().await;
            panic!("connected within 50 ms, so the timeout path wasn't tested");
        }
    }

    let state = connection_state_after_settling(&device_name).await;
    assert!(
        matches!(state, Ok(Ok(false))),
        "the timed-out connect should have released {device_name}, got {state:?}"
    );
}

/// A connect whose caller gives up must release the sensor too (BR-3): here
/// the caller's own `timeout` drops the connect future, as the GUI's connect
/// timeout, Esc in the TUI and the service's poll time limit do.
#[tokio::test]
#[ignore = "requires BLE hardware and an Aranet2 device"]
async fn test_cancelled_connect_leaves_sensor_disconnected() {
    let Some(device_name) = get_device("aranet2") else {
        println!("SKIP: ARANET2_DEVICE not set");
        return;
    };
    let (adapter, peripheral) = find_disconnected_sensor(&device_name).await;

    let connect = Device::from_peripheral_with_config(
        adapter,
        peripheral,
        aranet_core::ConnectionConfig::default(),
    );
    match timeout(Duration::from_millis(50), connect).await {
        Err(_elapsed) => {}
        Ok(Err(e)) => panic!("expected the connect to be cancelled after 50 ms, got: {e}"),
        Ok(Ok(device)) => {
            let _ = device.disconnect().await;
            panic!("connected within 50 ms, so the cancel path wasn't tested");
        }
    }

    let state = connection_state_after_settling(&device_name).await;
    assert!(
        matches!(state, Ok(Ok(false))),
        "the cancelled connect should have released {device_name}, got {state:?}"
    );
}

// =============================================================================
// Adapter Reuse Tests
// =============================================================================

/// Number of OS threads in this process (macOS `ps -M` prints a header plus one line per thread).
#[cfg(target_os = "macos")]
fn thread_count() -> usize {
    let out = std::process::Command::new("ps")
        .args(["-M", "-p", &std::process::id().to_string()])
        .output()
        .expect("failed to run ps");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .count()
        .saturating_sub(1)
}

#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "requires Bluetooth permission (macOS)"]
async fn test_get_adapter_does_not_spawn_a_thread_per_call() {
    let _ = aranet_core::scan::get_adapter().await.expect("adapter");
    let baseline = thread_count();
    for _ in 0..5 {
        let _ = aranet_core::scan::get_adapter().await.expect("adapter");
    }
    let after = thread_count();
    assert!(
        after <= baseline,
        "get_adapter leaked threads: {baseline} -> {after}"
    );
}

/// Each `#[tokio::test]` (and any library user that builds a runtime per call)
/// drops its runtime when done, and what aranet caches across calls must keep
/// working after the runtime that first used it has shut down:
/// - on macOS, the adapter: btleplug runs the CoreBluetooth adapter's event
///   loop on the runtime that created it, so a cached adapter created on the
///   first runtime would silently stop discovering devices;
/// - on Linux, the Bluetooth manager: bluez-async runs the D-Bus connection's
///   only I/O task on the runtime that created the manager, so every later
///   call on it would wait out the 30 s D-Bus timeout and fail.
#[test]
#[ignore = "requires BLE hardware and an Aranet device in range"]
fn test_get_adapter_still_discovers_after_its_runtime_shuts_down() {
    // Current-thread runtimes, like `#[tokio::test]`, so no extra worker threads.
    let runtime = || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
    };

    let first = runtime();
    first.block_on(async {
        aranet_core::scan::get_adapter().await.expect("adapter");
    });
    drop(first);

    let second = runtime();
    let devices = second.block_on(async {
        let options = ScanOptions::default()
            .duration_secs(10)
            .filter_aranet_only(true);
        timeout(Duration::from_secs(30), scan_with_options(options))
            .await
            .expect("scan timed out")
            .expect("scan failed")
    });
    assert!(
        !devices.is_empty(),
        "no Aranet devices discovered after the adapter's first runtime shut down"
    );
}

// =============================================================================
// Reconnect Tests
// =============================================================================

/// The adapter's peripheral for `device`: its scan identifier, its btleplug ID, or its
/// name (either half of CoreBluetooth's "GAP [advertised]" form), any case. `None`
/// when the adapter doesn't know it: CoreBluetooth forgets a peripheral once it
/// has disconnected, until a scan finds it again.
async fn known_peripheral(device: &str) -> Option<btleplug::platform::Peripheral> {
    use btleplug::api::{Central as _, Peripheral as _};

    let adapter = aranet_core::scan::get_adapter().await.expect("adapter");
    for p in adapter.peripherals().await.expect("peripherals") {
        let name = p
            .properties()
            .await
            .ok()
            .flatten()
            .and_then(|props| props.local_name);
        let id = aranet_core::create_identifier(&p.address().to_string(), &p.id());
        let is_device = |n: &str| {
            n.eq_ignore_ascii_case(device)
                || n.strip_suffix(']')
                    .and_then(|n| n.split_once(" ["))
                    .is_some_and(|(gap, adv)| {
                        gap.trim().eq_ignore_ascii_case(device)
                            || adv.trim().eq_ignore_ascii_case(device)
                    })
        };
        if id.eq_ignore_ascii_case(device)
            || p.id().to_string().eq_ignore_ascii_case(device)
            || name.as_deref().is_some_and(is_device)
        {
            return Some(p);
        }
    }
    None
}

/// The adapter's peripheral for `device`, which must be known to the adapter.
async fn find_peripheral(device: &str) -> btleplug::platform::Peripheral {
    known_peripheral(device)
        .await
        .unwrap_or_else(|| panic!("{device} is not known to the adapter"))
}

/// Whether the Bluetooth stack reports `device` as connected. `false` when the
/// adapter doesn't know it (see `known_peripheral`) or doesn't answer within
/// 5 s: btleplug never answers for a peripheral that CoreBluetooth has dropped.
async fn peripheral_connected(device: &str) -> bool {
    let Some(p) = known_peripheral(device).await else {
        return false;
    };
    matches!(
        timeout(
            Duration::from_secs(5),
            btleplug::api::Peripheral::is_connected(&p)
        )
        .await,
        Ok(Ok(true))
    )
}

/// Disconnect `device` behind aranet's back (same CBPeripheral / BlueZ object path), as if it went out of range.
/// Panics if the stack doesn't confirm the disconnect, so a test never blames the code under test for a cut that didn't happen.
async fn force_link_loss(device: &str) {
    let p = find_peripheral(device).await;
    timeout(
        Duration::from_secs(10),
        btleplug::api::Peripheral::disconnect(&p),
    )
    .await
    .expect("the forced disconnect timed out")
    .expect("the forced disconnect failed");
}

/// After the link drops behind its back, a `ReconnectingDevice` reconnects
/// once and the new link survives: before the fix, the old handle's cleanup
/// tore down each new link, so every read reconnected again.
#[tokio::test]
#[ignore = "requires BLE hardware and an Aranet2 device"]
async fn test_reconnecting_device_recovers_from_link_loss() {
    use aranet_core::events::{DeviceEvent, event_channel};
    use aranet_core::{AranetDevice, ReconnectOptions, ReconnectingDevice};

    let Some(dev) = get_device("aranet2") else {
        println!("SKIP: ARANET2_DEVICE not set");
        return;
    };

    let (tx, mut rx) = event_channel(16);
    let device = timeout(
        BLE_TIMEOUT,
        ReconnectingDevice::connect_with_events(&dev, ReconnectOptions::unlimited(), tx),
    )
    .await
    .expect("connect timeout")
    .expect("connect failed");
    timeout(Duration::from_secs(10), device.read_current())
        .await
        .expect("first read timeout")
        .expect("first read failed");

    println!("Connected to {dev}; dropping the link behind its back");
    force_link_loss(&dev).await;

    // A read may have to notice the loss (up to 3 s), close the old link (up
    // to 5 s), back off (1 s), then find the sensor and connect again.
    let mut reads = Vec::new();
    for i in 1..=3 {
        if i > 1 {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        let started = std::time::Instant::now();
        let read = timeout(Duration::from_secs(90), device.read_current()).await;
        let ok = matches!(read, Ok(Ok(_)));
        println!("read {i}: ok={ok} after {:?}: {read:?}", started.elapsed());
        reads.push(ok);
    }

    let (mut started, mut reconnects) = (0, 0);
    while let Ok(event) = rx.try_recv() {
        println!("event: {event:?}");
        match event {
            DeviceEvent::ReconnectStarted { .. } => started += 1,
            DeviceEvent::ReconnectSucceeded { .. } => reconnects += 1,
            _ => {}
        }
    }
    let _ = device.disconnect().await;

    assert_eq!(
        reads, [true; 3],
        "every read after the link loss should succeed"
    );
    assert!(
        started > 0,
        "the forced link loss should have made it reconnect"
    );
    assert_eq!(reconnects, 1, "the new link should survive the old one");
}

/// BR-5, BR-8: the health monitor replaces a connection that dropped behind
/// its back, and leaves the device alone once `disconnect()` was called.
#[tokio::test]
#[ignore = "requires BLE hardware and an Aranet2 device"]
async fn test_health_monitor_repairs_link_loss_and_respects_disconnect() {
    use std::sync::Arc;

    use aranet_core::{DeviceEvent, DeviceManager, ManagerConfig};
    use tokio_util::sync::CancellationToken;

    /// How long the monitor gets to repair the lost link: up to two ticks
    /// (5 s apart), the validation read (at most 3 s), closing the dead link
    /// (at most 5 s), and up to two searches and connects. On macOS the first
    /// connect after a forced disconnect can time out after 15 s; the monitor
    /// then tries again at the next tick.
    const REPAIR_TIMEOUT: Duration = Duration::from_secs(90);

    let Some(dev) = get_device("aranet2") else {
        println!("SKIP: ARANET2_DEVICE not set");
        return;
    };

    let manager = Arc::new(DeviceManager::with_config(
        ManagerConfig::default()
            .health_check_interval(Duration::from_secs(5))
            .adaptive_interval(false),
    ));
    let mut events = manager.events().subscribe();
    timeout(BLE_TIMEOUT, manager.connect(&dev))
        .await
        .expect("connect timed out")
        .expect("connect failed");
    let cancel = CancellationToken::new();
    let monitor = manager.start_health_monitor(cancel.clone());

    // 1. The link drops behind the manager's back: the monitor closes the dead
    //    handle and connects again.
    force_link_loss(&dev).await;
    let lost_at = std::time::Instant::now();
    let saw_disconnect = timeout(REPAIR_TIMEOUT, async {
        let mut saw_disconnect = false;
        loop {
            match events.recv().await.expect("event channel") {
                DeviceEvent::Disconnected { device, reason } if device.id == dev => {
                    println!("{:.1?}: Disconnected ({reason:?})", lost_at.elapsed());
                    saw_disconnect = true;
                }
                DeviceEvent::ReconnectSucceeded { device, attempts } if device.id == dev => {
                    println!(
                        "{:.1?}: ReconnectSucceeded after {attempts} attempt(s)",
                        lost_at.elapsed()
                    );
                    return saw_disconnect;
                }
                other => println!("{:.1?}: {other:?}", lost_at.elapsed()),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the monitor did not repair the link within {REPAIR_TIMEOUT:?}"));
    assert!(
        saw_disconnect,
        "ReconnectSucceeded came without a Disconnected"
    );
    let reading = timeout(BLE_TIMEOUT, manager.read_current(&dev))
        .await
        .expect("read timed out")
        .expect("read after the repair failed");
    println!("read after the repair: {reading:?}");

    // 2. The user disconnects: the monitor must leave the device alone.
    while events.try_recv().is_ok() {}
    timeout(BLE_TIMEOUT, manager.disconnect(&dev))
        .await
        .expect("disconnect timed out")
        .expect("disconnect failed");
    tokio::time::sleep(Duration::from_secs(20)).await;
    let mut reconnects = Vec::new();
    while let Ok(event) = events.try_recv() {
        println!("after disconnect(): {event:?}");
        if matches!(
            event,
            DeviceEvent::Connected { .. } | DeviceEvent::ReconnectStarted { .. }
        ) {
            reconnects.push(event);
        }
    }
    assert!(
        reconnects.is_empty(),
        "the monitor reconnected the device after disconnect(): {reconnects:?}"
    );
    assert!(
        !peripheral_connected(&dev).await,
        "the sensor is still connected after disconnect()"
    );

    cancel.cancel();
    timeout(Duration::from_secs(5), monitor)
        .await
        .expect("the health monitor did not stop within 5 s")
        .expect("the health monitor panicked");
}
