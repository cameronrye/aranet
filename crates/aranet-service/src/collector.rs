//! Background data collector.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::interval;
use tracing::{debug, error, info, warn};

use aranet_core::Device;
use aranet_store::StoredReading;

use crate::config::DeviceConfig;
use crate::state::{AppState, ReadingEvent};

/// Background collector that polls devices on their configured intervals.
pub struct Collector {
    state: Arc<AppState>,
}

impl Collector {
    /// Create a new collector.
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Start collecting data from all configured devices.
    ///
    /// This spawns a separate task for each device that polls at the configured interval.
    /// Returns immediately; collection happens in the background.
    pub fn start(&self) {
        let devices = self.state.config.devices.clone();

        if devices.is_empty() {
            info!("No devices configured for collection");
            return;
        }

        info!("Starting collector for {} device(s)", devices.len());

        for device_config in devices {
            let state = Arc::clone(&self.state);
            tokio::spawn(async move {
                collect_device(state, device_config).await;
            });
        }
    }
}

/// Collect readings from a single device.
async fn collect_device(state: Arc<AppState>, config: DeviceConfig) {
    let device_id = &config.address;
    let alias = config.alias.as_deref().unwrap_or(device_id);
    let poll_interval = Duration::from_secs(config.poll_interval);

    info!(
        "Starting collector for {} (alias: {}, interval: {}s)",
        device_id, alias, config.poll_interval
    );

    let mut interval_timer = interval(poll_interval);
    let mut consecutive_failures = 0u32;

    loop {
        interval_timer.tick().await;

        match poll_device(&state, device_id).await {
            Ok(reading) => {
                consecutive_failures = 0;
                debug!("Collected reading from {}: CO2={}", device_id, reading.co2);

                // Broadcast the reading to WebSocket clients
                let event = ReadingEvent {
                    device_id: device_id.to_string(),
                    reading,
                };
                let _ = state.readings_tx.send(event);
            }
            Err(e) => {
                consecutive_failures += 1;
                if consecutive_failures <= 3 {
                    warn!("Failed to poll {}: {} (attempt {})", device_id, e, consecutive_failures);
                } else if consecutive_failures == 4 {
                    error!(
                        "Failed to poll {} after {} attempts, will continue trying silently",
                        device_id, consecutive_failures
                    );
                }
                // Continue trying - the device may come back online
            }
        }
    }
}

/// Poll a single device and store the reading.
async fn poll_device(state: &AppState, device_id: &str) -> Result<StoredReading, CollectorError> {
    // Connect to the device
    let device = Device::connect(device_id)
        .await
        .map_err(CollectorError::Connect)?;

    // Read current values
    let reading = device.read_current().await.map_err(CollectorError::Read)?;

    // Disconnect
    let _ = device.disconnect().await;

    // Store the reading
    {
        let store = state.store.lock().await;
        store
            .insert_reading(device_id, &reading)
            .map_err(CollectorError::Store)?;
    }

    // Return the stored reading
    Ok(StoredReading::from_reading(device_id, &reading))
}

/// Collector errors.
#[derive(Debug, thiserror::Error)]
pub enum CollectorError {
    #[error("Failed to connect: {0}")]
    Connect(aranet_core::Error),
    #[error("Failed to read: {0}")]
    Read(aranet_core::Error),
    #[error("Failed to store: {0}")]
    Store(aranet_store::Error),
}

