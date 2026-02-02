//! Passive monitoring via BLE advertisements.
//!
//! This module provides functionality to monitor Aranet devices without
//! establishing a connection, using BLE advertisement data instead.
//!
//! # Benefits
//!
//! - **Lower power consumption**: No connection overhead
//! - **More devices**: Can monitor more than the BLE connection limit
//! - **Simpler**: No connection management needed
//!
//! # Requirements
//!
//! Smart Home integration must be enabled on each device:
//! - Go to device Settings > Smart Home > Enable
//!
//! # Example
//!
//! ```ignore
//! use aranet_core::passive::{PassiveMonitor, PassiveMonitorOptions};
//! use tokio_util::sync::CancellationToken;
//!
//! let monitor = PassiveMonitor::new(PassiveMonitorOptions::default());
//! let cancel = CancellationToken::new();
//!
//! // Start monitoring in background
//! let handle = monitor.start(cancel.clone());
//!
//! // Receive readings
//! let mut rx = monitor.subscribe();
//! while let Ok(reading) = rx.recv().await {
//!     println!("Device: {} CO2: {:?}", reading.device_name, reading.data.co2);
//! }
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use btleplug::api::{Central, Peripheral as _, ScanFilter};
use tokio::sync::{broadcast, RwLock};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::advertisement::{AdvertisementData, parse_advertisement_with_name};
use crate::error::Result;
use crate::scan::get_adapter;
use crate::uuid::MANUFACTURER_ID;

/// A reading from passive advertisement monitoring.
#[derive(Debug, Clone)]
pub struct PassiveReading {
    /// Device identifier (MAC address or UUID).
    pub device_id: String,
    /// Device name if available.
    pub device_name: Option<String>,
    /// RSSI signal strength.
    pub rssi: Option<i16>,
    /// Parsed advertisement data.
    pub data: AdvertisementData,
    /// When this reading was received.
    pub received_at: std::time::Instant,
}

/// Options for passive monitoring.
#[derive(Debug, Clone)]
pub struct PassiveMonitorOptions {
    /// How long to scan between processing cycles.
    pub scan_duration: Duration,
    /// Delay between scan cycles.
    pub scan_interval: Duration,
    /// Channel capacity for readings.
    pub channel_capacity: usize,
    /// Only emit readings when values change (deduplicate).
    pub deduplicate: bool,
    /// Maximum age of cached readings before re-emitting (if deduplicate is true).
    pub max_reading_age: Duration,
    /// Filter to only these device IDs (empty = all Aranet devices).
    pub device_filter: Vec<String>,
}

impl Default for PassiveMonitorOptions {
    fn default() -> Self {
        Self {
            scan_duration: Duration::from_secs(5),
            scan_interval: Duration::from_secs(1),
            channel_capacity: 100,
            deduplicate: true,
            max_reading_age: Duration::from_secs(60),
            device_filter: Vec::new(),
        }
    }
}

impl PassiveMonitorOptions {
    /// Create new options with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the scan duration.
    pub fn scan_duration(mut self, duration: Duration) -> Self {
        self.scan_duration = duration;
        self
    }

    /// Set the interval between scan cycles.
    pub fn scan_interval(mut self, interval: Duration) -> Self {
        self.scan_interval = interval;
        self
    }

    /// Enable or disable deduplication.
    pub fn deduplicate(mut self, enable: bool) -> Self {
        self.deduplicate = enable;
        self
    }

    /// Filter to specific device IDs.
    pub fn filter_devices(mut self, device_ids: Vec<String>) -> Self {
        self.device_filter = device_ids;
        self
    }
}

/// Cached reading for deduplication.
struct CachedReading {
    data: AdvertisementData,
    received_at: std::time::Instant,
}

/// Passive monitor for Aranet devices using BLE advertisements.
///
/// This allows monitoring multiple devices without establishing connections,
/// which is useful for scenarios where:
/// - You need to monitor more devices than the BLE connection limit
/// - Low power consumption is important
/// - Real-time data isn't critical (advertisement interval is typically 4+ seconds)
pub struct PassiveMonitor {
    options: PassiveMonitorOptions,
    /// Broadcast sender for readings.
    sender: broadcast::Sender<PassiveReading>,
    /// Cache of last readings for deduplication.
    cache: Arc<RwLock<HashMap<String, CachedReading>>>,
}

impl PassiveMonitor {
    /// Create a new passive monitor with the given options.
    pub fn new(options: PassiveMonitorOptions) -> Self {
        let (sender, _) = broadcast::channel(options.channel_capacity);
        Self {
            options,
            sender,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Subscribe to passive readings.
    ///
    /// Returns a receiver that will receive readings as they are detected.
    pub fn subscribe(&self) -> broadcast::Receiver<PassiveReading> {
        self.sender.subscribe()
    }

    /// Get the number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }

    /// Start the passive monitor.
    ///
    /// This spawns a background task that continuously scans for BLE
    /// advertisements and parses Aranet device data.
    ///
    /// The task runs until the cancellation token is triggered.
    pub fn start(self: &Arc<Self>, cancel_token: CancellationToken) -> tokio::task::JoinHandle<()> {
        let monitor = Arc::clone(self);

        tokio::spawn(async move {
            info!("Starting passive monitor");

            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        info!("Passive monitor cancelled");
                        break;
                    }
                    result = monitor.scan_cycle() => {
                        if let Err(e) = result {
                            warn!("Passive monitor scan error: {}", e);
                        }
                        // Wait before next scan cycle
                        sleep(monitor.options.scan_interval).await;
                    }
                }
            }
        })
    }

    /// Perform a single scan cycle.
    async fn scan_cycle(&self) -> Result<()> {
        let adapter = get_adapter().await?;

        // Start scanning
        adapter.start_scan(ScanFilter::default()).await?;
        sleep(self.options.scan_duration).await;
        adapter.stop_scan().await?;

        // Process discovered peripherals
        let peripherals = adapter.peripherals().await?;

        for peripheral in peripherals {
            if let Ok(Some(props)) = peripheral.properties().await {
                // Check if this is an Aranet device by manufacturer data
                if let Some(data) = props.manufacturer_data.get(&MANUFACTURER_ID) {
                    let device_id = crate::util::create_identifier(
                        &props.address.to_string(),
                        &peripheral.id(),
                    );

                    // Check device filter
                    if !self.options.device_filter.is_empty()
                        && !self.options.device_filter.contains(&device_id)
                    {
                        continue;
                    }

                    // Try to parse the advertisement
                    match parse_advertisement_with_name(data, props.local_name.as_deref()) {
                        Ok(adv_data) => {
                            // Check for deduplication
                            let should_emit = if self.options.deduplicate {
                                self.should_emit(&device_id, &adv_data).await
                            } else {
                                true
                            };

                            if should_emit {
                                let reading = PassiveReading {
                                    device_id: device_id.clone(),
                                    device_name: props.local_name.clone(),
                                    rssi: props.rssi,
                                    data: adv_data.clone(),
                                    received_at: std::time::Instant::now(),
                                };

                                // Update cache
                                self.cache.write().await.insert(
                                    device_id,
                                    CachedReading {
                                        data: adv_data,
                                        received_at: std::time::Instant::now(),
                                    },
                                );

                                // Send to subscribers (ignore if no receivers)
                                let _ = self.sender.send(reading);
                            }
                        }
                        Err(e) => {
                            debug!(
                                "Failed to parse advertisement from {}: {}",
                                device_id, e
                            );
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Check if a reading should be emitted (for deduplication).
    async fn should_emit(&self, device_id: &str, data: &AdvertisementData) -> bool {
        let cache = self.cache.read().await;

        if let Some(cached) = cache.get(device_id) {
            // Check if reading is too old
            if cached.received_at.elapsed() > self.options.max_reading_age {
                return true;
            }

            // Check if values have changed
            if cached.data.co2 != data.co2
                || cached.data.temperature != data.temperature
                || cached.data.humidity != data.humidity
                || cached.data.pressure != data.pressure
                || cached.data.radon != data.radon
                || cached.data.radiation_dose_rate != data.radiation_dose_rate
                || cached.data.battery != data.battery
            {
                return true;
            }

            // Check if counter changed (new measurement)
            if cached.data.counter != data.counter {
                return true;
            }

            false
        } else {
            // Not in cache, emit
            true
        }
    }

    /// Get the last known reading for a device.
    pub async fn get_last_reading(&self, device_id: &str) -> Option<AdvertisementData> {
        let cache = self.cache.read().await;
        cache.get(device_id).map(|c| c.data.clone())
    }

    /// Get all known device IDs.
    pub async fn known_devices(&self) -> Vec<String> {
        let cache = self.cache.read().await;
        cache.keys().cloned().collect()
    }

    /// Clear the reading cache.
    pub async fn clear_cache(&self) {
        self.cache.write().await.clear();
    }
}

impl Default for PassiveMonitor {
    fn default() -> Self {
        Self::new(PassiveMonitorOptions::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_passive_monitor_options_default() {
        let opts = PassiveMonitorOptions::default();
        assert_eq!(opts.scan_duration, Duration::from_secs(5));
        assert!(opts.deduplicate);
        assert!(opts.device_filter.is_empty());
    }

    #[test]
    fn test_passive_monitor_options_builder() {
        let opts = PassiveMonitorOptions::new()
            .scan_duration(Duration::from_secs(10))
            .deduplicate(false)
            .filter_devices(vec!["device1".to_string()]);

        assert_eq!(opts.scan_duration, Duration::from_secs(10));
        assert!(!opts.deduplicate);
        assert_eq!(opts.device_filter, vec!["device1"]);
    }

    #[test]
    fn test_passive_monitor_subscribe() {
        let monitor = Arc::new(PassiveMonitor::default());
        let _rx1 = monitor.subscribe();
        let _rx2 = monitor.subscribe();
        assert_eq!(monitor.subscriber_count(), 2);
    }
}
