//! Device discovery and scanning.
//!
//! This module provides functionality to scan for Aranet devices
//! using Bluetooth Low Energy.

use std::time::Duration;

use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Adapter, Manager, Peripheral, PeripheralId};
use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::error::{Error, Result};
use crate::util::{create_identifier, format_peripheral_id};
use crate::uuid::{MANUFACTURER_ID, SAF_TEHNIKA_SERVICE_NEW, SAF_TEHNIKA_SERVICE_OLD};
use aranet_types::DeviceType;

/// Progress update for device finding operations.
#[derive(Debug, Clone)]
pub enum FindProgress {
    /// Found device in cache, no scan needed.
    CacheHit,
    /// Starting scan attempt.
    ScanAttempt {
        /// Current attempt number (1-based).
        attempt: u32,
        /// Total number of attempts.
        total: u32,
        /// Duration of this scan attempt.
        duration_secs: u64,
    },
    /// Device found on specific attempt.
    Found { attempt: u32 },
    /// Attempt failed, will retry.
    RetryNeeded { attempt: u32 },
}

/// Callback type for progress updates during device finding.
pub type ProgressCallback = Box<dyn Fn(FindProgress) + Send + Sync>;

/// Information about a discovered Aranet device.
#[derive(Debug, Clone)]
pub struct DiscoveredDevice {
    /// The device name (e.g., "Aranet4 12345").
    pub name: Option<String>,
    /// The peripheral ID for connecting.
    pub id: PeripheralId,
    /// The BLE address as a string (may be zeros on macOS, use `id` instead).
    pub address: String,
    /// A connection identifier (peripheral ID on macOS, address on other platforms).
    pub identifier: String,
    /// RSSI signal strength.
    pub rssi: Option<i16>,
    /// Device type if detected from advertisement.
    pub device_type: Option<DeviceType>,
    /// Whether the device is connectable.
    pub is_aranet: bool,
    /// Raw manufacturer data from advertisement (if available).
    pub manufacturer_data: Option<Vec<u8>>,
}

/// Options for scanning.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// How long to scan for devices.
    pub duration: Duration,
    /// Only return devices that appear to be Aranet devices.
    pub filter_aranet_only: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            duration: Duration::from_secs(5),
            filter_aranet_only: true,
        }
    }
}

impl ScanOptions {
    /// Create new scan options with defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the scan duration.
    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }

    /// Set scan duration in seconds.
    pub fn duration_secs(mut self, secs: u64) -> Self {
        self.duration = Duration::from_secs(secs);
        self
    }

    /// Set whether to filter for Aranet devices only.
    pub fn filter_aranet_only(mut self, filter: bool) -> Self {
        self.filter_aranet_only = filter;
        self
    }

    /// Scan for all BLE devices, not just Aranet.
    pub fn all_devices(self) -> Self {
        self.filter_aranet_only(false)
    }
}

/// Get the first available Bluetooth adapter.
pub async fn get_adapter() -> Result<Adapter> {
    use crate::error::DeviceNotFoundReason;

    let manager = Manager::new().await?;
    let adapters = manager.adapters().await?;

    adapters
        .into_iter()
        .next()
        .ok_or(Error::DeviceNotFound(DeviceNotFoundReason::NoAdapter))
}

/// Scan for Aranet devices in range.
///
/// Returns a list of discovered devices, or an error if the scan failed.
/// An empty list indicates no devices were found (not an error).
///
/// # Errors
///
/// Returns an error if:
/// - No Bluetooth adapter is available
/// - Bluetooth is not enabled
/// - The scan could not be started or stopped
pub async fn scan_for_devices() -> Result<Vec<DiscoveredDevice>> {
    scan_with_options(ScanOptions::default()).await
}

/// Scan for devices with custom options.
pub async fn scan_with_options(options: ScanOptions) -> Result<Vec<DiscoveredDevice>> {
    let adapter = get_adapter().await?;
    scan_with_adapter(&adapter, options).await
}

/// Scan for devices with retry logic for flaky Bluetooth environments.
///
/// This function will retry the scan up to `max_retries` times if:
/// - The scan fails due to a Bluetooth error
/// - No devices are found (when `retry_on_empty` is true)
///
/// A delay is applied between retries, starting at 500ms and doubling each attempt.
///
/// # Arguments
///
/// * `options` - Scan options
/// * `max_retries` - Maximum number of retry attempts
/// * `retry_on_empty` - Whether to retry if no devices are found
///
/// # Example
///
/// ```ignore
/// use aranet_core::scan::{ScanOptions, scan_with_retry};
///
/// // Retry up to 3 times, including when no devices found
/// let devices = scan_with_retry(ScanOptions::default(), 3, true).await?;
/// ```
pub async fn scan_with_retry(
    options: ScanOptions,
    max_retries: u32,
    retry_on_empty: bool,
) -> Result<Vec<DiscoveredDevice>> {
    let mut attempt = 0;
    let mut delay = Duration::from_millis(500);

    loop {
        match scan_with_options(options.clone()).await {
            Ok(devices) if devices.is_empty() && retry_on_empty && attempt < max_retries => {
                attempt += 1;
                warn!(
                    "No devices found, retrying ({}/{})...",
                    attempt, max_retries
                );
                sleep(delay).await;
                delay = delay.saturating_mul(2).min(Duration::from_secs(5));
            }
            Ok(devices) => return Ok(devices),
            Err(e) if attempt < max_retries => {
                attempt += 1;
                warn!(
                    "Scan failed ({}), retrying ({}/{})...",
                    e, attempt, max_retries
                );
                sleep(delay).await;
                delay = delay.saturating_mul(2).min(Duration::from_secs(5));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Scan for devices using a specific adapter.
pub async fn scan_with_adapter(
    adapter: &Adapter,
    options: ScanOptions,
) -> Result<Vec<DiscoveredDevice>> {
    info!(
        "Starting BLE scan for {} seconds...",
        options.duration.as_secs()
    );

    // Start scanning
    adapter.start_scan(ScanFilter::default()).await?;

    // Wait for the scan duration
    sleep(options.duration).await;

    // Stop scanning
    adapter.stop_scan().await?;

    // Get discovered peripherals
    let peripherals = adapter.peripherals().await?;
    let mut discovered = Vec::new();

    for peripheral in peripherals {
        match process_peripheral(&peripheral, options.filter_aranet_only).await {
            Ok(Some(device)) => {
                info!("Found Aranet device: {:?}", device.name);
                discovered.push(device);
            }
            Ok(None) => {
                // Not an Aranet device or filtered out
            }
            Err(e) => {
                debug!("Error processing peripheral: {}", e);
            }
        }
    }

    info!("Scan complete. Found {} device(s)", discovered.len());
    Ok(discovered)
}

/// Process a peripheral and determine if it's an Aranet device.
async fn process_peripheral(
    peripheral: &Peripheral,
    filter_aranet_only: bool,
) -> Result<Option<DiscoveredDevice>> {
    let properties = peripheral.properties().await?;
    let properties = match properties {
        Some(p) => p,
        None => return Ok(None),
    };

    let id = peripheral.id();
    let address = properties.address.to_string();
    let name = properties.local_name.clone();
    let rssi = properties.rssi;

    // Check if this is an Aranet device
    let is_aranet = is_aranet_device(&properties);

    if filter_aranet_only && !is_aranet {
        return Ok(None);
    }

    // Try to determine device type from name
    let device_type = name.as_ref().and_then(|n| DeviceType::from_name(n));

    // Get manufacturer data if available
    let manufacturer_data = properties.manufacturer_data.get(&MANUFACTURER_ID).cloned();

    // Create identifier: use peripheral ID string on macOS (where address is 00:00:00:00:00:00)
    // On other platforms, use the address
    let identifier = create_identifier(&address, &id);

    Ok(Some(DiscoveredDevice {
        name,
        id,
        address,
        identifier,
        rssi,
        device_type,
        is_aranet,
        manufacturer_data,
    }))
}

/// Check if a peripheral is an Aranet device based on its properties.
fn is_aranet_device(properties: &btleplug::api::PeripheralProperties) -> bool {
    // Check manufacturer data for Aranet manufacturer ID
    if properties.manufacturer_data.contains_key(&MANUFACTURER_ID) {
        return true;
    }

    // Check service UUIDs for Aranet services
    for service_uuid in properties.service_data.keys() {
        if *service_uuid == SAF_TEHNIKA_SERVICE_NEW || *service_uuid == SAF_TEHNIKA_SERVICE_OLD {
            return true;
        }
    }

    // Check advertised services
    for service_uuid in &properties.services {
        if *service_uuid == SAF_TEHNIKA_SERVICE_NEW || *service_uuid == SAF_TEHNIKA_SERVICE_OLD {
            return true;
        }
    }

    // Check device name for Aranet
    if let Some(name) = &properties.local_name {
        let name_lower = name.to_lowercase();
        if name_lower.contains("aranet") {
            return true;
        }
    }

    false
}

/// Find a specific device by name or address.
pub async fn find_device(identifier: &str) -> Result<(Adapter, Peripheral)> {
    find_device_with_options(identifier, ScanOptions::default()).await
}

/// Find a specific device by name or address with custom options.
///
/// This function uses a retry strategy to improve reliability:
/// 1. First checks if the device is already known (cached from previous scans)
/// 2. Performs up to 3 scan attempts with increasing durations
///
/// This helps with BLE reliability issues where devices may not appear
/// on every scan due to advertisement timing.
pub async fn find_device_with_options(
    identifier: &str,
    options: ScanOptions,
) -> Result<(Adapter, Peripheral)> {
    find_device_with_progress(identifier, options, None).await
}

/// Find a specific device with progress callback for UI feedback.
///
/// The progress callback is called with updates about the search progress,
/// including cache hits, scan attempts, and retry information.
pub async fn find_device_with_progress(
    identifier: &str,
    options: ScanOptions,
    progress: Option<ProgressCallback>,
) -> Result<(Adapter, Peripheral)> {
    let adapter = get_adapter().await?;
    let identifier_lower = identifier.to_lowercase();

    info!("Looking for device: {}", identifier);

    // First, check if device is already known (cached from previous scans)
    if let Some(peripheral) = find_peripheral_by_identifier(&adapter, &identifier_lower).await? {
        info!("Found device in cache (no scan needed)");
        if let Some(ref cb) = progress {
            cb(FindProgress::CacheHit);
        }
        return Ok((adapter, peripheral));
    }

    // Retry with multiple scan attempts for better reliability
    // BLE advertisements can be missed due to timing, so we try multiple times
    let max_attempts: u32 = 3;
    let base_duration = options.duration.as_millis() as u64 / 2;
    let base_duration = Duration::from_millis(base_duration.max(2000)); // At least 2 seconds

    for attempt in 1..=max_attempts {
        let scan_duration = base_duration * attempt;
        let duration_secs = scan_duration.as_secs();

        info!(
            "Scan attempt {}/{} ({}s)...",
            attempt, max_attempts, duration_secs
        );

        if let Some(ref cb) = progress {
            cb(FindProgress::ScanAttempt {
                attempt,
                total: max_attempts,
                duration_secs,
            });
        }

        // Start scanning
        adapter.start_scan(ScanFilter::default()).await?;
        sleep(scan_duration).await;
        adapter.stop_scan().await?;

        // Check if we found the device
        if let Some(peripheral) = find_peripheral_by_identifier(&adapter, &identifier_lower).await?
        {
            info!("Found device on attempt {}", attempt);
            if let Some(ref cb) = progress {
                cb(FindProgress::Found { attempt });
            }
            return Ok((adapter, peripheral));
        }

        if attempt < max_attempts {
            warn!("Device not found, retrying...");
            if let Some(ref cb) = progress {
                cb(FindProgress::RetryNeeded { attempt });
            }
        }
    }

    warn!(
        "Device not found after {} attempts: {}",
        max_attempts, identifier
    );
    Err(Error::device_not_found(identifier))
}

/// Search through known peripherals to find one matching the identifier.
async fn find_peripheral_by_identifier(
    adapter: &Adapter,
    identifier_lower: &str,
) -> Result<Option<Peripheral>> {
    let peripherals = adapter.peripherals().await?;

    for peripheral in peripherals {
        if let Ok(Some(props)) = peripheral.properties().await {
            let address = props.address.to_string().to_lowercase();
            let peripheral_id = format_peripheral_id(&peripheral.id()).to_lowercase();

            // Check peripheral ID match (macOS uses UUIDs)
            if peripheral_id.contains(identifier_lower) {
                debug!("Matched by peripheral ID: {}", peripheral_id);
                return Ok(Some(peripheral));
            }

            // Check address match (Linux/Windows use MAC addresses)
            if address != "00:00:00:00:00:00"
                && (address == identifier_lower
                    || address.replace(':', "") == identifier_lower.replace(':', ""))
            {
                debug!("Matched by address: {}", address);
                return Ok(Some(peripheral));
            }

            // Check name match (partial match supported)
            if let Some(name) = &props.local_name
                && name.to_lowercase().contains(identifier_lower)
            {
                debug!("Matched by name: {}", name);
                return Ok(Some(peripheral));
            }
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== ScanOptions Tests ====================

    #[test]
    fn test_scan_options_default() {
        let options = ScanOptions::default();
        assert_eq!(options.duration, Duration::from_secs(5));
        assert!(options.filter_aranet_only);
    }

    #[test]
    fn test_scan_options_new() {
        let options = ScanOptions::new();
        assert_eq!(options.duration, Duration::from_secs(5));
        assert!(options.filter_aranet_only);
    }

    #[test]
    fn test_scan_options_duration() {
        let options = ScanOptions::new().duration(Duration::from_secs(10));
        assert_eq!(options.duration, Duration::from_secs(10));
    }

    #[test]
    fn test_scan_options_duration_secs() {
        let options = ScanOptions::new().duration_secs(15);
        assert_eq!(options.duration, Duration::from_secs(15));
    }

    #[test]
    fn test_scan_options_filter_aranet_only() {
        let options = ScanOptions::new().filter_aranet_only(false);
        assert!(!options.filter_aranet_only);

        let options = ScanOptions::new().filter_aranet_only(true);
        assert!(options.filter_aranet_only);
    }

    #[test]
    fn test_scan_options_all_devices() {
        let options = ScanOptions::new().all_devices();
        assert!(!options.filter_aranet_only);
    }

    #[test]
    fn test_scan_options_chaining() {
        let options = ScanOptions::new()
            .duration_secs(20)
            .filter_aranet_only(false);

        assert_eq!(options.duration, Duration::from_secs(20));
        assert!(!options.filter_aranet_only);
    }

    #[test]
    fn test_scan_options_clone() {
        let options1 = ScanOptions::new().duration_secs(8);
        let options2 = options1.clone();

        assert_eq!(options1.duration, options2.duration);
        assert_eq!(options1.filter_aranet_only, options2.filter_aranet_only);
    }

    #[test]
    fn test_scan_options_debug() {
        let options = ScanOptions::new();
        let debug = format!("{:?}", options);
        assert!(debug.contains("ScanOptions"));
        assert!(debug.contains("duration"));
        assert!(debug.contains("filter_aranet_only"));
    }

    // ==================== FindProgress Tests ====================

    #[test]
    fn test_find_progress_cache_hit() {
        let progress = FindProgress::CacheHit;
        let debug = format!("{:?}", progress);
        assert!(debug.contains("CacheHit"));
    }

    #[test]
    fn test_find_progress_scan_attempt() {
        let progress = FindProgress::ScanAttempt {
            attempt: 2,
            total: 3,
            duration_secs: 5,
        };

        if let FindProgress::ScanAttempt {
            attempt,
            total,
            duration_secs,
        } = progress
        {
            assert_eq!(attempt, 2);
            assert_eq!(total, 3);
            assert_eq!(duration_secs, 5);
        } else {
            panic!("Unexpected variant");
        }
    }

    #[test]
    fn test_find_progress_found() {
        let progress = FindProgress::Found { attempt: 1 };
        if let FindProgress::Found { attempt } = progress {
            assert_eq!(attempt, 1);
        } else {
            panic!("Unexpected variant");
        }
    }

    #[test]
    fn test_find_progress_retry_needed() {
        let progress = FindProgress::RetryNeeded { attempt: 2 };
        if let FindProgress::RetryNeeded { attempt } = progress {
            assert_eq!(attempt, 2);
        } else {
            panic!("Unexpected variant");
        }
    }

    #[test]
    fn test_find_progress_clone() {
        let progress1 = FindProgress::ScanAttempt {
            attempt: 1,
            total: 3,
            duration_secs: 4,
        };
        let progress2 = progress1.clone();

        if let (
            FindProgress::ScanAttempt {
                attempt: a1,
                total: t1,
                duration_secs: d1,
            },
            FindProgress::ScanAttempt {
                attempt: a2,
                total: t2,
                duration_secs: d2,
            },
        ) = (progress1, progress2)
        {
            assert_eq!(a1, a2);
            assert_eq!(t1, t2);
            assert_eq!(d1, d2);
        } else {
            panic!("Clone should preserve variant");
        }
    }

    // ==================== DiscoveredDevice Tests ====================

    #[test]
    fn test_discovered_device_debug() {
        let device = DiscoveredDevice {
            name: Some("Aranet4 12345".to_string()),
            id: btleplug::platform::PeripheralId::from(uuid::Uuid::new_v4()),
            address: "AA:BB:CC:DD:EE:FF".to_string(),
            identifier: "AA:BB:CC:DD:EE:FF".to_string(),
            rssi: Some(-60),
            device_type: Some(DeviceType::Aranet4),
            is_aranet: true,
            manufacturer_data: None,
        };

        let debug = format!("{:?}", device);
        assert!(debug.contains("DiscoveredDevice"));
        assert!(debug.contains("Aranet4 12345"));
    }

    #[test]
    fn test_discovered_device_clone() {
        let device = DiscoveredDevice {
            name: Some("Test".to_string()),
            id: btleplug::platform::PeripheralId::from(uuid::Uuid::new_v4()),
            address: "11:22:33:44:55:66".to_string(),
            identifier: "11:22:33:44:55:66".to_string(),
            rssi: Some(-75),
            device_type: Some(DeviceType::Aranet2),
            is_aranet: true,
            manufacturer_data: Some(vec![1, 2, 3]),
        };

        let cloned = device.clone();
        assert_eq!(cloned.name, device.name);
        assert_eq!(cloned.address, device.address);
        assert_eq!(cloned.rssi, device.rssi);
        assert_eq!(cloned.device_type, device.device_type);
        assert_eq!(cloned.is_aranet, device.is_aranet);
        assert_eq!(cloned.manufacturer_data, device.manufacturer_data);
    }

    #[test]
    fn test_discovered_device_all_device_types() {
        for device_type in [
            DeviceType::Aranet4,
            DeviceType::Aranet2,
            DeviceType::AranetRadon,
            DeviceType::AranetRadiation,
        ] {
            let device = DiscoveredDevice {
                name: None,
                id: btleplug::platform::PeripheralId::from(uuid::Uuid::new_v4()),
                address: "00:00:00:00:00:00".to_string(),
                identifier: "test".to_string(),
                rssi: None,
                device_type: Some(device_type),
                is_aranet: true,
                manufacturer_data: None,
            };

            assert_eq!(device.device_type, Some(device_type));
        }
    }

    #[test]
    fn test_discovered_device_without_optional_fields() {
        let device = DiscoveredDevice {
            name: None,
            id: btleplug::platform::PeripheralId::from(uuid::Uuid::new_v4()),
            address: "00:00:00:00:00:00".to_string(),
            identifier: "minimal".to_string(),
            rssi: None,
            device_type: None,
            is_aranet: false,
            manufacturer_data: None,
        };

        assert!(device.name.is_none());
        assert!(device.rssi.is_none());
        assert!(device.device_type.is_none());
        assert!(!device.is_aranet);
        assert!(device.manufacturer_data.is_none());
    }
}
