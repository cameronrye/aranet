//! Historical data download.
//!
//! This module provides functionality to download historical sensor
//! readings stored on an Aranet device.
//!
//! # Supported Devices
//!
//! | Device | History Support | Notes |
//! |--------|-----------------|-------|
//! | Aranet4 | Full | CO₂, temperature, pressure, humidity |
//! | Aranet2 | Full | Temperature, humidity |
//! | AranetRn+ (Radon) | Full | Radon, temperature, pressure, humidity |
//! | Aranet Radiation | Not supported | Returns error - protocol undocumented |
//!
//! **Note:** Aranet Radiation devices do not support history download. Attempting
//! to download history from an Aranet Radiation device will return an error.
//! Use [`Device::read_current()`](crate::device::Device::read_current) for
//! current radiation readings. The `radiation_rate` and `radiation_total` fields
//! in [`HistoryRecord`] are reserved for future implementation.
//!
//! # Index Convention
//!
//! **All history indices are 1-based**, following the Aranet device protocol:
//! - Index 1 = oldest reading
//! - Index N = newest reading (where N = total_readings)
//!
//! This matches the device's internal indexing. When specifying ranges:
//! ```ignore
//! let options = HistoryOptions {
//!     start_index: Some(1),    // First reading
//!     end_index: Some(100),    // 100th reading
//!     ..Default::default()
//! };
//! ```
//!
//! # Protocols
//!
//! Aranet devices support two history protocols:
//! - **V1**: Notification-based (older devices) - uses characteristic notifications
//! - **V2**: Read-based (newer devices, preferred) - direct read/write operations

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Buf;
use time::OffsetDateTime;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::commands::{HISTORY_V1_REQUEST, HISTORY_V2_REQUEST};
use crate::device::Device;
use crate::error::{Error, Result};
use crate::uuid::{COMMAND, HISTORY_V2, READ_INTERVAL, SECONDS_SINCE_UPDATE, TOTAL_READINGS};
use aranet_types::HistoryRecord;

/// Progress information for history download.
#[derive(Debug, Clone)]
pub struct HistoryProgress {
    /// Current parameter being downloaded.
    pub current_param: HistoryParam,
    /// Parameter index (1-based, e.g., 1 of 4).
    pub param_index: usize,
    /// Total number of parameters to download.
    pub total_params: usize,
    /// Number of values downloaded for current parameter.
    pub values_downloaded: usize,
    /// Total values to download for current parameter.
    pub total_values: usize,
    /// Overall progress (0.0 to 1.0).
    pub overall_progress: f32,
}

impl HistoryProgress {
    /// Create a new progress struct.
    pub fn new(
        param: HistoryParam,
        param_idx: usize,
        total_params: usize,
        total_values: usize,
    ) -> Self {
        Self {
            current_param: param,
            param_index: param_idx,
            total_params,
            values_downloaded: 0,
            total_values,
            overall_progress: 0.0,
        }
    }

    fn update(&mut self, values_downloaded: usize) {
        self.values_downloaded = values_downloaded;
        let param_progress = if self.total_values > 0 {
            values_downloaded as f32 / self.total_values as f32
        } else {
            1.0
        };
        // Guard against division by zero when total_params is 0
        if self.total_params == 0 {
            self.overall_progress = 1.0;
            return;
        }
        let base_progress = (self.param_index - 1) as f32 / self.total_params as f32;
        let param_contribution = param_progress / self.total_params as f32;
        self.overall_progress = base_progress + param_contribution;
    }
}

/// Type alias for progress callback function.
pub type ProgressCallback = Arc<dyn Fn(HistoryProgress) + Send + Sync>;

/// Type alias for checkpoint callback function.
pub type CheckpointCallback = Arc<dyn Fn(HistoryCheckpoint) + Send + Sync>;

/// Checkpoint data for resuming interrupted history downloads.
///
/// This can be serialized and saved to disk to allow resuming downloads
/// after disconnection or application restart.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HistoryCheckpoint {
    /// Device identifier this checkpoint belongs to.
    pub device_id: String,
    /// The parameter currently being downloaded.
    pub current_param: HistoryParamCheckpoint,
    /// Index where download should resume for current parameter.
    pub resume_index: u16,
    /// Total readings on the device when checkpoint was created.
    pub total_readings: u16,
    /// Which parameters have been fully downloaded.
    pub completed_params: Vec<HistoryParamCheckpoint>,
    /// Timestamp when checkpoint was created.
    pub created_at: time::OffsetDateTime,
    /// Downloaded values for completed parameters (serialized).
    pub downloaded_data: Option<PartialHistoryData>,
}

/// Serializable version of HistoryParam for checkpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HistoryParamCheckpoint {
    Temperature,
    Humidity,
    Pressure,
    Co2,
    Humidity2,
    Radon,
}

impl From<HistoryParam> for HistoryParamCheckpoint {
    fn from(param: HistoryParam) -> Self {
        match param {
            HistoryParam::Temperature => HistoryParamCheckpoint::Temperature,
            HistoryParam::Humidity => HistoryParamCheckpoint::Humidity,
            HistoryParam::Pressure => HistoryParamCheckpoint::Pressure,
            HistoryParam::Co2 => HistoryParamCheckpoint::Co2,
            HistoryParam::Humidity2 => HistoryParamCheckpoint::Humidity2,
            HistoryParam::Radon => HistoryParamCheckpoint::Radon,
        }
    }
}

impl From<HistoryParamCheckpoint> for HistoryParam {
    fn from(param: HistoryParamCheckpoint) -> Self {
        match param {
            HistoryParamCheckpoint::Temperature => HistoryParam::Temperature,
            HistoryParamCheckpoint::Humidity => HistoryParam::Humidity,
            HistoryParamCheckpoint::Pressure => HistoryParam::Pressure,
            HistoryParamCheckpoint::Co2 => HistoryParam::Co2,
            HistoryParamCheckpoint::Humidity2 => HistoryParam::Humidity2,
            HistoryParamCheckpoint::Radon => HistoryParam::Radon,
        }
    }
}

/// Partially downloaded history data for checkpoint resume.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PartialHistoryData {
    pub co2_values: Vec<u16>,
    pub temp_values: Vec<u16>,
    pub pressure_values: Vec<u16>,
    pub humidity_values: Vec<u16>,
    pub radon_values: Vec<u32>,
}

impl HistoryCheckpoint {
    /// Create a new checkpoint for starting a fresh download.
    pub fn new(device_id: &str, total_readings: u16, first_param: HistoryParam) -> Self {
        Self {
            device_id: device_id.to_string(),
            current_param: first_param.into(),
            resume_index: 1,
            total_readings,
            completed_params: Vec::new(),
            created_at: time::OffsetDateTime::now_utc(),
            downloaded_data: Some(PartialHistoryData::default()),
        }
    }

    /// Check if this checkpoint is still valid for the given device state.
    pub fn is_valid(&self, current_total_readings: u16) -> bool {
        // Checkpoint is valid if the device hasn't collected more readings
        // (which would shift the indices)
        self.total_readings == current_total_readings
    }

    /// Update the checkpoint after completing a parameter.
    pub fn complete_param(&mut self, param: HistoryParam, values: Vec<u16>) {
        self.completed_params.push(param.into());
        if let Some(ref mut data) = self.downloaded_data {
            match param {
                HistoryParam::Co2 => data.co2_values = values,
                HistoryParam::Temperature => data.temp_values = values,
                HistoryParam::Pressure => data.pressure_values = values,
                HistoryParam::Humidity | HistoryParam::Humidity2 => data.humidity_values = values,
                HistoryParam::Radon => {} // Radon uses u32, handled separately
            }
        }
    }

    /// Update the checkpoint after completing a radon parameter.
    pub fn complete_radon_param(&mut self, values: Vec<u32>) {
        self.completed_params.push(HistoryParamCheckpoint::Radon);
        if let Some(ref mut data) = self.downloaded_data {
            data.radon_values = values;
        }
    }
}

/// Parameter types for history requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HistoryParam {
    Temperature = 1,
    Humidity = 2,
    Pressure = 3,
    Co2 = 4,
    /// Humidity for Aranet2/Radon (different encoding).
    Humidity2 = 5,
    /// Radon concentration (Bq/m³) for AranetRn+.
    Radon = 10,
}

/// Options for downloading history.
///
/// # Index Convention
///
/// Indices are **1-based** to match the Aranet device protocol:
/// - `start_index: Some(1)` means the first (oldest) reading
/// - `end_index: Some(100)` means the 100th reading
/// - `start_index: None` defaults to 1 (beginning)
/// - `end_index: None` defaults to total_readings (end)
///
/// # Progress Reporting
///
/// Use `with_progress` to receive updates during download:
/// ```ignore
/// let options = HistoryOptions::default()
///     .with_progress(|p| println!("Progress: {:.1}%", p.overall_progress * 100.0));
/// ```
///
/// # Adaptive Read Delay
///
/// Use `adaptive_delay` to automatically adjust delay based on signal quality:
/// ```ignore
/// let options = HistoryOptions::default().adaptive_delay(true);
/// ```
///
/// # Resume Support
///
/// For long downloads, use checkpointing to allow resume on failure:
/// ```ignore
/// let checkpoint = HistoryCheckpoint::load("device_123")?;
/// let options = HistoryOptions::default().resume_from(checkpoint);
/// ```
#[derive(Clone)]
pub struct HistoryOptions {
    /// Starting index (1-based, inclusive). If None, downloads from the beginning (index 1).
    pub start_index: Option<u16>,
    /// Ending index (1-based, inclusive). If None, downloads to the end (index = total_readings).
    pub end_index: Option<u16>,
    /// Delay between read operations to avoid overwhelming the device.
    pub read_delay: Duration,
    /// Progress callback (optional).
    pub progress_callback: Option<ProgressCallback>,
    /// Whether to use adaptive delay based on signal quality.
    pub use_adaptive_delay: bool,
    /// Checkpoint callback for saving progress during download (optional).
    /// Called periodically with the current checkpoint state.
    pub checkpoint_callback: Option<CheckpointCallback>,
    /// How often to call the checkpoint callback (in records).
    pub checkpoint_interval: usize,
}

impl std::fmt::Debug for HistoryOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistoryOptions")
            .field("start_index", &self.start_index)
            .field("end_index", &self.end_index)
            .field("read_delay", &self.read_delay)
            .field("progress_callback", &self.progress_callback.is_some())
            .field("use_adaptive_delay", &self.use_adaptive_delay)
            .field("checkpoint_callback", &self.checkpoint_callback.is_some())
            .field("checkpoint_interval", &self.checkpoint_interval)
            .finish()
    }
}

impl Default for HistoryOptions {
    fn default() -> Self {
        Self {
            start_index: None,
            end_index: None,
            read_delay: Duration::from_millis(50),
            progress_callback: None,
            use_adaptive_delay: false,
            checkpoint_callback: None,
            checkpoint_interval: 100, // Checkpoint every 100 records
        }
    }
}

impl HistoryOptions {
    /// Create new history options with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the starting index (1-based).
    #[must_use]
    pub fn start_index(mut self, index: u16) -> Self {
        self.start_index = Some(index);
        self
    }

    /// Set the ending index (1-based).
    #[must_use]
    pub fn end_index(mut self, index: u16) -> Self {
        self.end_index = Some(index);
        self
    }

    /// Set the delay between read operations.
    #[must_use]
    pub fn read_delay(mut self, delay: Duration) -> Self {
        self.read_delay = delay;
        self
    }

    /// Set a progress callback.
    #[must_use]
    pub fn with_progress<F>(mut self, callback: F) -> Self
    where
        F: Fn(HistoryProgress) + Send + Sync + 'static,
    {
        self.progress_callback = Some(Arc::new(callback));
        self
    }

    /// Report progress if a callback is set.
    pub fn report_progress(&self, progress: &HistoryProgress) {
        if let Some(cb) = &self.progress_callback {
            cb(progress.clone());
        }
    }

    /// Enable or disable adaptive delay based on signal quality.
    ///
    /// When enabled, the read delay will be automatically adjusted based on
    /// the connection's signal strength:
    /// - Excellent signal: 30ms delay
    /// - Good signal: 50ms delay
    /// - Fair signal: 100ms delay
    /// - Poor signal: 200ms delay
    #[must_use]
    pub fn adaptive_delay(mut self, enable: bool) -> Self {
        self.use_adaptive_delay = enable;
        self
    }

    /// Set a checkpoint callback for saving download progress.
    ///
    /// The callback will be invoked periodically (based on `checkpoint_interval`)
    /// with the current checkpoint state, allowing recovery from interruptions.
    #[must_use]
    pub fn with_checkpoint<F>(mut self, callback: F) -> Self
    where
        F: Fn(HistoryCheckpoint) + Send + Sync + 'static,
    {
        self.checkpoint_callback = Some(Arc::new(callback));
        self
    }

    /// Set how often to call the checkpoint callback (in records).
    ///
    /// Default: 100 records
    #[must_use]
    pub fn checkpoint_interval(mut self, interval: usize) -> Self {
        self.checkpoint_interval = interval;
        self
    }

    /// Resume from a previous checkpoint.
    ///
    /// This sets the start_index based on the checkpoint's resume position.
    #[must_use]
    pub fn resume_from(mut self, checkpoint: &HistoryCheckpoint) -> Self {
        self.start_index = Some(checkpoint.resume_index);
        self
    }

    /// Report a checkpoint if a callback is set.
    pub fn report_checkpoint(&self, checkpoint: &HistoryCheckpoint) {
        if let Some(cb) = &self.checkpoint_callback {
            cb(checkpoint.clone());
        }
    }

    /// Get the effective read delay, optionally adjusted for signal quality.
    pub fn effective_read_delay(
        &self,
        signal_quality: Option<crate::device::SignalQuality>,
    ) -> Duration {
        if self.use_adaptive_delay {
            if let Some(quality) = signal_quality {
                return quality.recommended_read_delay();
            }
        }
        self.read_delay
    }
}

/// Information about the device's stored history.
#[derive(Debug, Clone)]
pub struct HistoryInfo {
    /// Total number of readings stored.
    pub total_readings: u16,
    /// Measurement interval in seconds.
    pub interval_seconds: u16,
    /// Seconds since the last reading.
    pub seconds_since_update: u16,
}

impl Device {
    /// Get information about the stored history.
    pub async fn get_history_info(&self) -> Result<HistoryInfo> {
        // Read total readings count
        let total_data = self.read_characteristic(TOTAL_READINGS).await?;
        let total_readings = if total_data.len() >= 2 {
            u16::from_le_bytes([total_data[0], total_data[1]])
        } else {
            return Err(Error::InvalidData(
                "Invalid total readings data".to_string(),
            ));
        };

        // Read interval
        let interval_data = self.read_characteristic(READ_INTERVAL).await?;
        let interval_seconds = if interval_data.len() >= 2 {
            u16::from_le_bytes([interval_data[0], interval_data[1]])
        } else {
            return Err(Error::InvalidData("Invalid interval data".to_string()));
        };

        // Read seconds since update
        let age_data = self.read_characteristic(SECONDS_SINCE_UPDATE).await?;
        let seconds_since_update = if age_data.len() >= 2 {
            u16::from_le_bytes([age_data[0], age_data[1]])
        } else {
            0
        };

        Ok(HistoryInfo {
            total_readings,
            interval_seconds,
            seconds_since_update,
        })
    }

    /// Download all historical readings from the device.
    pub async fn download_history(&self) -> Result<Vec<HistoryRecord>> {
        self.download_history_with_options(HistoryOptions::default())
            .await
    }

    /// Download historical readings with custom options.
    ///
    /// # Device Support
    ///
    /// - **Aranet4**: Downloads CO₂, temperature, pressure, humidity
    /// - **Aranet2**: Downloads temperature, humidity
    /// - **AranetRn+ (Radon)**: Downloads radon, temperature, pressure, humidity
    /// - **Aranet Radiation**: **Not supported** - returns an error. The device protocol
    ///   for historical radiation data requires additional documentation. Use
    ///   [`Device::read_current()`](crate::device::Device::read_current) to get
    ///   current radiation readings.
    ///
    /// # Adaptive Delay
    ///
    /// If `options.use_adaptive_delay` is enabled, the read delay will be
    /// automatically adjusted based on the connection's signal quality.
    ///
    /// # Checkpointing
    ///
    /// If a checkpoint callback is set, progress will be saved periodically
    /// to allow resuming interrupted downloads.
    pub async fn download_history_with_options(
        &self,
        options: HistoryOptions,
    ) -> Result<Vec<HistoryRecord>> {
        use aranet_types::DeviceType;

        let info = self.get_history_info().await?;
        info!(
            "Device has {} readings, interval {}s, last update {}s ago",
            info.total_readings, info.interval_seconds, info.seconds_since_update
        );

        if info.total_readings == 0 {
            return Ok(Vec::new());
        }

        let start_idx = options.start_index.unwrap_or(1);
        let end_idx = options.end_index.unwrap_or(info.total_readings);

        // Get signal quality for adaptive delay if enabled
        let signal_quality = if options.use_adaptive_delay {
            match self.signal_quality().await {
                Some(quality) => {
                    info!(
                        "Signal quality: {:?} - using {} ms read delay",
                        quality,
                        quality.recommended_read_delay().as_millis()
                    );
                    Some(quality)
                }
                None => {
                    debug!("Could not read signal quality, using default delay");
                    None
                }
            }
        } else {
            None
        };

        // Calculate effective read delay
        let effective_delay = options.effective_read_delay(signal_quality);

        // Dispatch based on device type
        match self.device_type() {
            Some(DeviceType::AranetRadiation) => {
                // Aranet Radiation history download is not yet implemented.
                // The device protocol for historical radiation data is different from
                // other Aranet devices and requires additional research/documentation.
                Err(Error::InvalidData(
                    "History download for Aranet Radiation devices is not yet implemented. \
                     Current readings are available via read_current()."
                        .to_string(),
                ))
            }
            Some(DeviceType::AranetRadon) => {
                // For radon devices, download radon instead of CO2, and use Humidity2
                self.download_radon_history_internal(
                    &info,
                    start_idx,
                    end_idx,
                    &options,
                    effective_delay,
                )
                .await
            }
            _ => {
                // For Aranet4 and Aranet2, download CO2 (or 0 for Aranet2) and standard humidity
                self.download_aranet4_history_internal(
                    &info,
                    start_idx,
                    end_idx,
                    &options,
                    effective_delay,
                )
                .await
            }
        }
    }

    /// Download history for Aranet4 devices (CO2, temp, pressure, humidity).
    async fn download_aranet4_history_internal(
        &self,
        info: &HistoryInfo,
        start_idx: u16,
        end_idx: u16,
        options: &HistoryOptions,
        effective_delay: Duration,
    ) -> Result<Vec<HistoryRecord>> {
        let total_values = (end_idx - start_idx + 1) as usize;

        // Create checkpoint if callback is set
        let device_id = self.address().to_string();
        let mut checkpoint = if options.checkpoint_callback.is_some() {
            Some(HistoryCheckpoint::new(
                &device_id,
                info.total_readings,
                HistoryParam::Co2,
            ))
        } else {
            None
        };

        // Download each parameter type with progress reporting
        let mut progress = HistoryProgress::new(HistoryParam::Co2, 1, 4, total_values);
        options.report_progress(&progress);

        let co2_values = self
            .download_param_history_with_progress(
                HistoryParam::Co2,
                start_idx,
                end_idx,
                effective_delay,
                |downloaded| {
                    progress.update(downloaded);
                    options.report_progress(&progress);
                },
            )
            .await?;

        // Update checkpoint after CO2
        if let Some(ref mut cp) = checkpoint {
            cp.complete_param(HistoryParam::Co2, co2_values.clone());
            cp.current_param = HistoryParamCheckpoint::Temperature;
            cp.resume_index = start_idx;
            options.report_checkpoint(cp);
        }

        progress = HistoryProgress::new(HistoryParam::Temperature, 2, 4, total_values);
        options.report_progress(&progress);

        let temp_values = self
            .download_param_history_with_progress(
                HistoryParam::Temperature,
                start_idx,
                end_idx,
                effective_delay,
                |downloaded| {
                    progress.update(downloaded);
                    options.report_progress(&progress);
                },
            )
            .await?;

        // Update checkpoint after Temperature
        if let Some(ref mut cp) = checkpoint {
            cp.complete_param(HistoryParam::Temperature, temp_values.clone());
            cp.current_param = HistoryParamCheckpoint::Pressure;
            cp.resume_index = start_idx;
            options.report_checkpoint(cp);
        }

        progress = HistoryProgress::new(HistoryParam::Pressure, 3, 4, total_values);
        options.report_progress(&progress);

        let pressure_values = self
            .download_param_history_with_progress(
                HistoryParam::Pressure,
                start_idx,
                end_idx,
                effective_delay,
                |downloaded| {
                    progress.update(downloaded);
                    options.report_progress(&progress);
                },
            )
            .await?;

        // Update checkpoint after Pressure
        if let Some(ref mut cp) = checkpoint {
            cp.complete_param(HistoryParam::Pressure, pressure_values.clone());
            cp.current_param = HistoryParamCheckpoint::Humidity;
            cp.resume_index = start_idx;
            options.report_checkpoint(cp);
        }

        progress = HistoryProgress::new(HistoryParam::Humidity, 4, 4, total_values);
        options.report_progress(&progress);

        let humidity_values = self
            .download_param_history_with_progress(
                HistoryParam::Humidity,
                start_idx,
                end_idx,
                effective_delay,
                |downloaded| {
                    progress.update(downloaded);
                    options.report_progress(&progress);
                },
            )
            .await?;

        // Update checkpoint after Humidity (download complete)
        if let Some(ref mut cp) = checkpoint {
            cp.complete_param(HistoryParam::Humidity, humidity_values.clone());
            options.report_checkpoint(cp);
        }

        // Calculate timestamps for each record
        let now = OffsetDateTime::now_utc();
        let latest_reading_time = now - time::Duration::seconds(info.seconds_since_update as i64);

        // Build history records by combining all parameters
        let mut records = Vec::new();
        let count = co2_values.len();

        for i in 0..count {
            // Calculate timestamp: most recent reading is at the end
            let readings_ago = (count - 1 - i) as i64;
            let timestamp = latest_reading_time
                - time::Duration::seconds(readings_ago * info.interval_seconds as i64);

            let record = HistoryRecord {
                timestamp,
                co2: co2_values.get(i).copied().unwrap_or(0),
                temperature: raw_to_temperature(temp_values.get(i).copied().unwrap_or(0)),
                pressure: raw_to_pressure(pressure_values.get(i).copied().unwrap_or(0)),
                humidity: humidity_values.get(i).copied().unwrap_or(0) as u8,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            };
            records.push(record);
        }

        info!("Downloaded {} history records", records.len());
        Ok(records)
    }

    /// Download history for AranetRn+ devices (radon, temp, pressure, humidity).
    async fn download_radon_history_internal(
        &self,
        info: &HistoryInfo,
        start_idx: u16,
        end_idx: u16,
        options: &HistoryOptions,
        effective_delay: Duration,
    ) -> Result<Vec<HistoryRecord>> {
        let total_values = (end_idx - start_idx + 1) as usize;

        // Create checkpoint if callback is set
        let device_id = self.address().to_string();
        let mut checkpoint = if options.checkpoint_callback.is_some() {
            Some(HistoryCheckpoint::new(
                &device_id,
                info.total_readings,
                HistoryParam::Radon,
            ))
        } else {
            None
        };

        // Download radon values (4 bytes each)
        let mut progress = HistoryProgress::new(HistoryParam::Radon, 1, 4, total_values);
        options.report_progress(&progress);

        let radon_values = self
            .download_param_history_u32_with_progress(
                HistoryParam::Radon,
                start_idx,
                end_idx,
                effective_delay,
                |downloaded| {
                    progress.update(downloaded);
                    options.report_progress(&progress);
                },
            )
            .await?;

        // Update checkpoint after Radon
        if let Some(ref mut cp) = checkpoint {
            cp.complete_radon_param(radon_values.clone());
            cp.current_param = HistoryParamCheckpoint::Temperature;
            cp.resume_index = start_idx;
            options.report_checkpoint(cp);
        }

        progress = HistoryProgress::new(HistoryParam::Temperature, 2, 4, total_values);
        options.report_progress(&progress);

        let temp_values = self
            .download_param_history_with_progress(
                HistoryParam::Temperature,
                start_idx,
                end_idx,
                effective_delay,
                |downloaded| {
                    progress.update(downloaded);
                    options.report_progress(&progress);
                },
            )
            .await?;

        // Update checkpoint after Temperature
        if let Some(ref mut cp) = checkpoint {
            cp.complete_param(HistoryParam::Temperature, temp_values.clone());
            cp.current_param = HistoryParamCheckpoint::Pressure;
            options.report_checkpoint(cp);
        }

        progress = HistoryProgress::new(HistoryParam::Pressure, 3, 4, total_values);
        options.report_progress(&progress);

        let pressure_values = self
            .download_param_history_with_progress(
                HistoryParam::Pressure,
                start_idx,
                end_idx,
                effective_delay,
                |downloaded| {
                    progress.update(downloaded);
                    options.report_progress(&progress);
                },
            )
            .await?;

        // Update checkpoint after Pressure
        if let Some(ref mut cp) = checkpoint {
            cp.complete_param(HistoryParam::Pressure, pressure_values.clone());
            cp.current_param = HistoryParamCheckpoint::Humidity2;
            options.report_checkpoint(cp);
        }

        // Radon devices use Humidity2 (different encoding, 2 bytes, divide by 10)
        progress = HistoryProgress::new(HistoryParam::Humidity2, 4, 4, total_values);
        options.report_progress(&progress);

        let humidity_values = self
            .download_param_history_with_progress(
                HistoryParam::Humidity2,
                start_idx,
                end_idx,
                effective_delay,
                |downloaded| {
                    progress.update(downloaded);
                    options.report_progress(&progress);
                },
            )
            .await?;

        // Update checkpoint after Humidity2 (download complete)
        if let Some(ref mut cp) = checkpoint {
            cp.complete_param(HistoryParam::Humidity2, humidity_values.clone());
            options.report_checkpoint(cp);
        }

        // Calculate timestamps for each record
        let now = OffsetDateTime::now_utc();
        let latest_reading_time = now - time::Duration::seconds(info.seconds_since_update as i64);

        // Build history records by combining all parameters
        let mut records = Vec::new();
        let count = radon_values.len();

        for i in 0..count {
            // Calculate timestamp: most recent reading is at the end
            let readings_ago = (count - 1 - i) as i64;
            let timestamp = latest_reading_time
                - time::Duration::seconds(readings_ago * info.interval_seconds as i64);

            // Humidity2 is stored as tenths of a percent
            let humidity_raw = humidity_values.get(i).copied().unwrap_or(0);
            let humidity = (humidity_raw / 10).min(100) as u8;

            let record = HistoryRecord {
                timestamp,
                co2: 0, // Not applicable for radon devices
                temperature: raw_to_temperature(temp_values.get(i).copied().unwrap_or(0)),
                pressure: raw_to_pressure(pressure_values.get(i).copied().unwrap_or(0)),
                humidity,
                radon: Some(radon_values.get(i).copied().unwrap_or(0)),
                radiation_rate: None,
                radiation_total: None,
            };
            records.push(record);
        }

        info!("Downloaded {} radon history records", records.len());
        Ok(records)
    }

    /// Download a single parameter's history using V2 protocol with progress callback.
    ///
    /// This is a generic implementation that handles different value sizes:
    /// - 1 byte: humidity
    /// - 2 bytes: CO2, temperature, pressure, humidity2
    /// - 4 bytes: radon
    #[allow(clippy::too_many_arguments)]
    async fn download_param_history_generic_with_progress<T, F>(
        &self,
        param: HistoryParam,
        start_idx: u16,
        end_idx: u16,
        read_delay: Duration,
        value_parser: impl Fn(&[u8], usize) -> Option<T>,
        value_size: usize,
        mut on_progress: F,
    ) -> Result<Vec<T>>
    where
        T: Default + Clone,
        F: FnMut(usize),
    {
        debug!(
            "Downloading {:?} history from {} to {} (value_size={})",
            param, start_idx, end_idx, value_size
        );

        let mut values: BTreeMap<u16, T> = BTreeMap::new();
        let mut current_idx = start_idx;

        while current_idx <= end_idx {
            // Send V2 history request using command constant
            let cmd = [
                HISTORY_V2_REQUEST,
                param as u8,
                (current_idx & 0xFF) as u8,
                ((current_idx >> 8) & 0xFF) as u8,
            ];

            self.write_characteristic(COMMAND, &cmd).await?;
            sleep(read_delay).await;

            // Read response
            let response = self.read_characteristic(HISTORY_V2).await?;

            // V2 response format (10-byte header):
            // Byte 0: param (1 byte)
            // Bytes 1-2: interval (2 bytes, little-endian)
            // Bytes 3-4: total_readings (2 bytes, little-endian)
            // Bytes 5-6: ago (2 bytes, little-endian)
            // Bytes 7-8: start index (2 bytes, little-endian)
            // Byte 9: count (1 byte)
            // Bytes 10+: data values
            if response.len() < 10 {
                warn!(
                    "Invalid history response: too short ({} bytes)",
                    response.len()
                );
                break;
            }

            let resp_param = response[0];
            if resp_param != param as u8 {
                warn!("Unexpected parameter in response: {}", resp_param);
                // Wait and retry - device may not have processed command yet
                sleep(read_delay).await;
                continue;
            }

            // Parse header
            let resp_start = u16::from_le_bytes([response[7], response[8]]);
            let resp_count = response[9] as usize;

            debug!(
                "History response: param={}, start={}, count={}",
                resp_param, resp_start, resp_count
            );

            // Check if we've reached the end (count == 0)
            if resp_count == 0 {
                debug!("Reached end of history (count=0)");
                break;
            }

            // Parse data values
            let data = &response[10..];
            let num_values = (data.len() / value_size).min(resp_count);

            for i in 0..num_values {
                let idx = resp_start + i as u16;
                if idx > end_idx {
                    break;
                }
                if let Some(value) = value_parser(data, i) {
                    values.insert(idx, value);
                }
            }

            current_idx = resp_start + num_values as u16;
            debug!(
                "Downloaded {} values, next index: {}",
                num_values, current_idx
            );

            // Report progress
            on_progress(values.len());

            // Check if we've downloaded all available data
            if (resp_start as usize + resp_count) >= end_idx as usize {
                debug!("Reached end of requested range");
                break;
            }
        }

        // Convert to ordered vector (BTreeMap already maintains order)
        Ok(values.into_values().collect())
    }

    /// Download a single parameter's history using V2 protocol (u16 values) with progress.
    async fn download_param_history_with_progress<F>(
        &self,
        param: HistoryParam,
        start_idx: u16,
        end_idx: u16,
        read_delay: Duration,
        on_progress: F,
    ) -> Result<Vec<u16>>
    where
        F: FnMut(usize),
    {
        let value_size = if param == HistoryParam::Humidity {
            1
        } else {
            2
        };

        self.download_param_history_generic_with_progress(
            param,
            start_idx,
            end_idx,
            read_delay,
            |data, i| {
                if param == HistoryParam::Humidity {
                    data.get(i).map(|&b| b as u16)
                } else {
                    let offset = i * 2;
                    if offset + 1 < data.len() {
                        Some(u16::from_le_bytes([data[offset], data[offset + 1]]))
                    } else {
                        None
                    }
                }
            },
            value_size,
            on_progress,
        )
        .await
    }

    /// Download a single parameter's history using V2 protocol (u32 values) with progress.
    async fn download_param_history_u32_with_progress<F>(
        &self,
        param: HistoryParam,
        start_idx: u16,
        end_idx: u16,
        read_delay: Duration,
        on_progress: F,
    ) -> Result<Vec<u32>>
    where
        F: FnMut(usize),
    {
        self.download_param_history_generic_with_progress(
            param,
            start_idx,
            end_idx,
            read_delay,
            |data, i| {
                let offset = i * 4;
                if offset + 3 < data.len() {
                    Some(u32::from_le_bytes([
                        data[offset],
                        data[offset + 1],
                        data[offset + 2],
                        data[offset + 3],
                    ]))
                } else {
                    None
                }
            },
            4,
            on_progress,
        )
        .await
    }

    /// Download history using V1 protocol (notification-based).
    ///
    /// This is used for older devices that don't support the V2 read-based protocol.
    /// V1 uses notifications on the HISTORY_V1 characteristic.
    pub async fn download_history_v1(&self) -> Result<Vec<HistoryRecord>> {
        use crate::uuid::HISTORY_V1;
        use tokio::sync::mpsc;

        let info = self.get_history_info().await?;
        info!(
            "V1 download: {} readings, interval {}s",
            info.total_readings, info.interval_seconds
        );

        if info.total_readings == 0 {
            return Ok(Vec::new());
        }

        // Subscribe to notifications
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(256);

        // Set up notification handler
        self.subscribe_to_notifications(HISTORY_V1, move |data| {
            if let Err(e) = tx.try_send(data.to_vec()) {
                warn!(
                    "V1 history notification channel full or closed, data may be lost: {}",
                    e
                );
            }
        })
        .await?;

        // Request history for each parameter
        let mut co2_values = Vec::new();
        let mut temp_values = Vec::new();
        let mut pressure_values = Vec::new();
        let mut humidity_values = Vec::new();

        for param in [
            HistoryParam::Co2,
            HistoryParam::Temperature,
            HistoryParam::Pressure,
            HistoryParam::Humidity,
        ] {
            // Send V1 history request using command constant
            let cmd = [
                HISTORY_V1_REQUEST,
                param as u8,
                0x01,
                0x00,
                (info.total_readings & 0xFF) as u8,
                ((info.total_readings >> 8) & 0xFF) as u8,
            ];

            self.write_characteristic(COMMAND, &cmd).await?;

            // Collect notifications until we have all values
            let mut values = Vec::new();
            let expected = info.total_readings as usize;

            let mut consecutive_timeouts = 0;
            const MAX_CONSECUTIVE_TIMEOUTS: u32 = 3;

            while values.len() < expected {
                match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
                    Ok(Some(data)) => {
                        consecutive_timeouts = 0; // Reset on successful receive
                        // Parse notification data
                        if data.len() >= 3 {
                            let resp_param = data[0];
                            if resp_param == param as u8 {
                                let mut buf = &data[3..];
                                while buf.len() >= 2 && values.len() < expected {
                                    values.push(buf.get_u16_le());
                                }
                            }
                        }
                    }
                    Ok(None) => {
                        warn!(
                            "V1 history channel closed for {:?}: got {}/{} values",
                            param,
                            values.len(),
                            expected
                        );
                        break;
                    }
                    Err(_) => {
                        consecutive_timeouts += 1;
                        warn!(
                            "Timeout waiting for V1 history notification ({}/{}), {:?}: {}/{} values",
                            consecutive_timeouts,
                            MAX_CONSECUTIVE_TIMEOUTS,
                            param,
                            values.len(),
                            expected
                        );
                        if consecutive_timeouts >= MAX_CONSECUTIVE_TIMEOUTS {
                            warn!(
                                "Too many consecutive timeouts for {:?}, proceeding with partial data",
                                param
                            );
                            break;
                        }
                    }
                }
            }

            // Log if we got incomplete data
            if values.len() < expected {
                warn!(
                    "V1 history download incomplete for {:?}: got {}/{} values ({:.1}%)",
                    param,
                    values.len(),
                    expected,
                    (values.len() as f64 / expected as f64) * 100.0
                );
            }

            match param {
                HistoryParam::Co2 => co2_values = values,
                HistoryParam::Temperature => temp_values = values,
                HistoryParam::Pressure => pressure_values = values,
                HistoryParam::Humidity => humidity_values = values,
                // V1 protocol doesn't support radon or humidity2
                HistoryParam::Humidity2 | HistoryParam::Radon => {}
            }
        }

        // Unsubscribe from notifications
        self.unsubscribe_from_notifications(HISTORY_V1).await?;

        // Build history records
        let now = OffsetDateTime::now_utc();
        let latest_reading_time = now - time::Duration::seconds(info.seconds_since_update as i64);

        let mut records = Vec::new();
        let count = co2_values.len();

        for i in 0..count {
            let readings_ago = (count - 1 - i) as i64;
            let timestamp = latest_reading_time
                - time::Duration::seconds(readings_ago * info.interval_seconds as i64);

            let record = HistoryRecord {
                timestamp,
                co2: co2_values.get(i).copied().unwrap_or(0),
                temperature: raw_to_temperature(temp_values.get(i).copied().unwrap_or(0)),
                pressure: raw_to_pressure(pressure_values.get(i).copied().unwrap_or(0)),
                humidity: humidity_values.get(i).copied().unwrap_or(0) as u8,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            };
            records.push(record);
        }

        info!("V1 download complete: {} records", records.len());
        Ok(records)
    }
}

/// Convert raw temperature value to Celsius.
pub fn raw_to_temperature(raw: u16) -> f32 {
    raw as f32 / 20.0
}

/// Convert raw pressure value to hPa.
pub fn raw_to_pressure(raw: u16) -> f32 {
    raw as f32 / 10.0
}

// NOTE: The HistoryValueConverter trait was removed as it was dead code.
// Use the standalone functions raw_to_temperature, raw_to_pressure, etc. directly.

#[cfg(test)]
mod tests {
    use super::*;

    // --- raw_to_temperature tests ---

    #[test]
    fn test_raw_to_temperature_typical_values() {
        // 22.5°C = 450 raw (450/20 = 22.5)
        assert!((raw_to_temperature(450) - 22.5).abs() < 0.001);

        // 20.0°C = 400 raw
        assert!((raw_to_temperature(400) - 20.0).abs() < 0.001);

        // 25.0°C = 500 raw
        assert!((raw_to_temperature(500) - 25.0).abs() < 0.001);
    }

    #[test]
    fn test_raw_to_temperature_edge_cases() {
        // 0°C = 0 raw
        assert!((raw_to_temperature(0) - 0.0).abs() < 0.001);

        // Very cold: -10°C would be negative, but raw is u16 so minimum is 0
        // Raw values represent actual temperature * 20

        // Very hot: 50°C = 1000 raw
        assert!((raw_to_temperature(1000) - 50.0).abs() < 0.001);

        // Maximum u16 would be 65535/20 = 3276.75°C (unrealistic but tests overflow handling)
        assert!((raw_to_temperature(u16::MAX) - 3276.75).abs() < 0.01);
    }

    #[test]
    fn test_raw_to_temperature_precision() {
        // Test fractional values
        // 22.55°C = 451 raw
        assert!((raw_to_temperature(451) - 22.55).abs() < 0.001);

        // 22.05°C = 441 raw
        assert!((raw_to_temperature(441) - 22.05).abs() < 0.001);
    }

    // --- raw_to_pressure tests ---

    #[test]
    fn test_raw_to_pressure_typical_values() {
        // 1013.2 hPa = 10132 raw
        assert!((raw_to_pressure(10132) - 1013.2).abs() < 0.01);

        // 1000.0 hPa = 10000 raw
        assert!((raw_to_pressure(10000) - 1000.0).abs() < 0.01);

        // 1050.0 hPa = 10500 raw
        assert!((raw_to_pressure(10500) - 1050.0).abs() < 0.01);
    }

    #[test]
    fn test_raw_to_pressure_edge_cases() {
        // 0 hPa = 0 raw
        assert!((raw_to_pressure(0) - 0.0).abs() < 0.01);

        // Low pressure: 950 hPa = 9500 raw
        assert!((raw_to_pressure(9500) - 950.0).abs() < 0.01);

        // High pressure: 1100 hPa = 11000 raw
        assert!((raw_to_pressure(11000) - 1100.0).abs() < 0.01);

        // Maximum u16 would be 65535/10 = 6553.5 hPa (unrealistic but tests bounds)
        assert!((raw_to_pressure(u16::MAX) - 6553.5).abs() < 0.1);
    }

    // --- HistoryParam tests ---

    #[test]
    fn test_history_param_values() {
        assert_eq!(HistoryParam::Temperature as u8, 1);
        assert_eq!(HistoryParam::Humidity as u8, 2);
        assert_eq!(HistoryParam::Pressure as u8, 3);
        assert_eq!(HistoryParam::Co2 as u8, 4);
    }

    #[test]
    fn test_history_param_debug() {
        assert_eq!(format!("{:?}", HistoryParam::Temperature), "Temperature");
        assert_eq!(format!("{:?}", HistoryParam::Co2), "Co2");
    }

    // --- HistoryOptions tests ---

    #[test]
    fn test_history_options_default() {
        let options = HistoryOptions::default();

        assert!(options.start_index.is_none());
        assert!(options.end_index.is_none());
        assert_eq!(options.read_delay, Duration::from_millis(50));
    }

    #[test]
    fn test_history_options_custom() {
        let options = HistoryOptions::new()
            .start_index(10)
            .end_index(100)
            .read_delay(Duration::from_millis(100));

        assert_eq!(options.start_index, Some(10));
        assert_eq!(options.end_index, Some(100));
        assert_eq!(options.read_delay, Duration::from_millis(100));
    }

    #[test]
    fn test_history_options_with_progress() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = Arc::clone(&call_count);

        let options = HistoryOptions::new().with_progress(move |_progress| {
            call_count_clone.fetch_add(1, Ordering::SeqCst);
        });

        assert!(options.progress_callback.is_some());

        // Test that the callback can be invoked
        let progress = HistoryProgress::new(HistoryParam::Co2, 1, 4, 100);
        options.report_progress(&progress);
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    // --- HistoryInfo tests ---

    #[test]
    fn test_history_info_creation() {
        let info = HistoryInfo {
            total_readings: 1000,
            interval_seconds: 300,
            seconds_since_update: 120,
        };

        assert_eq!(info.total_readings, 1000);
        assert_eq!(info.interval_seconds, 300);
        assert_eq!(info.seconds_since_update, 120);
    }

    #[test]
    fn test_history_info_debug() {
        let info = HistoryInfo {
            total_readings: 500,
            interval_seconds: 60,
            seconds_since_update: 30,
        };

        let debug_str = format!("{:?}", info);
        assert!(debug_str.contains("total_readings"));
        assert!(debug_str.contains("500"));
    }
}
