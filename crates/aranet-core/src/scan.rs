//! Device discovery and scanning.
//!
//! This module provides functionality to scan for Aranet devices
//! using Bluetooth Low Energy.
//!
//! Scans in one process run one at a time: each scan window takes a
//! process-wide permit and releases it only after the scan has stopped. A
//! window runs on aranet-core's background runtime and stops as soon as its
//! caller is dropped.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Adapter, Manager, Peripheral, PeripheralId};
use tokio::runtime::Handle;
use tokio::sync::RwLock;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Cached BLE manager — avoids creating a new D-Bus connection on every call.
///
/// Using `RwLock<Option<Manager>>` instead of `OnceCell` so the manager can
/// be re-created if the underlying D-Bus connection dies (e.g., dbus-daemon
/// restart, adapter reset).
static MANAGER: RwLock<Option<Manager>> = RwLock::const_new(None);

/// Get or create the shared BLE manager.
async fn shared_manager() -> Result<Manager> {
    // Fast path: read lock to return existing manager.
    {
        let guard = MANAGER.read().await;
        if let Some(m) = guard.as_ref() {
            return Ok(m.clone());
        }
    }
    // Slow path: create a new manager under write lock.
    let mut guard = MANAGER.write().await;
    // Double-check after acquiring write lock.
    if let Some(m) = guard.as_ref() {
        return Ok(m.clone());
    }
    // bluez-async spawns the D-Bus connection's only I/O task on the runtime
    // that creates the manager. On the caller's runtime that task would die when
    // the runtime shuts down, and every later call on the cached manager would
    // wait out the 30 s D-Bus timeout and fail.
    let m = crate::runtime::run(Manager::new()).await??;
    *guard = Some(m.clone());
    Ok(m)
}

/// Reset the cached manager, forcing the next call to create a fresh one.
///
/// Call this when the D-Bus connection appears to be dead (e.g., adapter
/// enumeration fails with a connection error).
async fn reset_manager() {
    let mut guard = MANAGER.write().await;
    if guard.take().is_some() {
        warn!("BLE manager reset — next operation will create a new D-Bus connection");
    }
}

use crate::error::{Error, Result};
use crate::util::{create_identifier, format_peripheral_id};
use crate::uuid::{MANUFACTURER_ID, SAF_TEHNIKA_SERVICE_NEW, SAF_TEHNIKA_SERVICE_OLD};
use aranet_types::DeviceType;

/// Progress update for device finding operations.
#[derive(Debug, Clone)]
pub enum FindProgress {
    /// Found the device among the devices the adapter already knows, without
    /// scanning for it: before the first scan, or after waiting for another
    /// search's scan.
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
    /// Use targeted BLE scan filter for Aranet service UUIDs.
    /// This reduces noise from non-Aranet devices but may not work on all platforms.
    pub use_service_filter: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            duration: Duration::from_secs(5),
            filter_aranet_only: true,
            // Default to false for maximum compatibility - service filtering
            // may not work on all platforms/adapters
            use_service_filter: false,
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

    /// Enable or disable BLE service UUID filtering.
    ///
    /// When enabled, the BLE scan will filter for Aranet service UUIDs at the
    /// adapter level, reducing noise from non-Aranet devices. This may not
    /// work on all platforms or with all BLE adapters.
    ///
    /// Default: `false` (for maximum compatibility)
    pub fn use_service_filter(mut self, enable: bool) -> Self {
        self.use_service_filter = enable;
        self
    }

    /// Create optimized scan options for finding Aranet devices quickly.
    ///
    /// Uses service UUID filtering if available and a shorter scan duration.
    pub fn optimized() -> Self {
        Self {
            duration: Duration::from_secs(3),
            filter_aranet_only: true,
            use_service_filter: true,
        }
    }
}

/// Adapter reused by [`get_adapter`] on macOS.
///
/// On CoreBluetooth every `Manager::adapters()` call starts a new
/// `CBCentralManager` on its own OS thread that never exits, so creating an
/// adapter per connection leaks a thread per poll. Other platforms create
/// adapters cheaply and stay uncached, so a dead D-Bus connection can still
/// be recovered by `reset_manager`.
///
/// The adapter is created on aranet-core's background runtime (`crate::runtime`),
/// because btleplug runs its event loop (device discovery, and each peripheral's
/// notification task) on the runtime that creates it. Created on a caller's
/// runtime, it would stop seeing devices, with no error, once that runtime shut
/// down, which happens after every `#[tokio::test]` and in any program that
/// builds a runtime per call.
static ADAPTER: RwLock<Option<Adapter>> = RwLock::const_new(None);

/// Get the first available Bluetooth adapter.
///
/// The adapter, and the Bluetooth manager behind it, are created on aranet-core's
/// background runtime (thread `aranet-ble`), so they keep working after the caller's
/// tokio runtime shuts down. On macOS the adapter is also created once, shared by
/// every caller in the process, and replaced if its CoreBluetooth thread stops.
pub async fn get_adapter() -> Result<Adapter> {
    if cfg!(target_os = "macos") {
        cached_adapter().await
    } else {
        crate::runtime::run(create_adapter()).await?
    }
}

async fn cached_adapter() -> Result<Adapter> {
    let cached = ADAPTER.read().await.clone();
    if let Some(adapter) = cached
        && adapter_thread_is_running(&adapter).await
    {
        return Ok(adapter);
    }
    let mut guard = ADAPTER.write().await;
    if let Some(adapter) = guard.as_ref() {
        if adapter_thread_is_running(adapter).await {
            return Ok(adapter.clone());
        }
        warn!("CoreBluetooth adapter thread has stopped; creating a new adapter");
    }
    let adapter = crate::runtime::run(create_adapter()).await??;
    *guard = Some(adapter.clone());
    Ok(adapter)
}

/// Whether the cached adapter's CoreBluetooth thread still takes requests.
///
/// btleplug's CoreBluetooth thread can panic (for example when services are
/// discovered after a connect has timed out). Every later request on that
/// adapter then fails with "Channel closed", so it has to be replaced. A reply
/// that is merely slow keeps the adapter: only a closed channel proves the
/// thread is gone.
async fn adapter_thread_is_running(adapter: &Adapter) -> bool {
    let adapter = adapter.clone();
    // Run on aranet-core's runtime so the timeout works even if the caller's
    // runtime has no time driver.
    let state = crate::runtime::run(async move {
        tokio::time::timeout(Duration::from_secs(2), adapter.adapter_state()).await
    })
    .await;
    match state {
        Ok(Ok(Err(e))) => {
            debug!("Cached Bluetooth adapter is unusable: {e}");
            false
        }
        _ => true,
    }
}

async fn create_adapter() -> Result<Adapter> {
    use crate::error::DeviceNotFoundReason;

    // On Linux, register a BlueZ agent to handle authentication during service
    // discovery. Without this, BlueZ hangs when it encounters characteristics
    // that require authentication (e.g., Battery Level on Aranet devices).
    #[cfg(target_os = "linux")]
    crate::bluez_agent::ensure_agent();

    let manager = shared_manager().await?;
    let adapters = match manager.adapters().await {
        Ok(a) => a,
        Err(e) => {
            // The D-Bus connection may have died — reset the cached manager
            // so the next call creates a fresh connection.
            reset_manager().await;
            return Err(e.into());
        }
    };

    adapters
        .into_iter()
        .next()
        .ok_or(Error::DeviceNotFound(DeviceNotFoundReason::NoAdapter))
}

/// Starts and stops a Bluetooth scan: btleplug's [`Adapter`] in production, a
/// fake in the tests.
pub(crate) trait ScanControl: Clone + Send + Sync + 'static {
    /// Start scanning. BlueZ fails with `org.bluez.Error.InProgress` if this
    /// process is already scanning.
    fn start(&self, filter: ScanFilter) -> impl Future<Output = Result<()>> + Send;

    /// Stop scanning.
    fn stop(&self) -> impl Future<Output = Result<()>> + Send;
}

impl ScanControl for Adapter {
    async fn start(&self, filter: ScanFilter) -> Result<()> {
        Central::start_scan(self, filter).await?;
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        Central::stop_scan(self).await?;
        Ok(())
    }
}

/// Lets one scan window run at a time. Whoever holds its [`ScanPermit`] may scan.
///
/// A scan is process-wide: BlueZ gives each D-Bus client (this whole process)
/// one discovery session, and on macOS the adapter from `get_adapter` is one
/// `CBCentralManager` shared by the whole process, so one caller's stop ends
/// every caller's scan. tokio's mutex hands the permit out in the order it was
/// asked for.
#[derive(Clone, Default)]
pub(crate) struct ScanLock(Arc<tokio::sync::Mutex<()>>);

impl ScanLock {
    /// Wait for the permit.
    pub(crate) async fn acquire(&self) -> ScanPermit {
        ScanPermit {
            _guard: Arc::clone(&self.0).lock_owned().await,
        }
    }
}

/// Permission to scan, from [`ScanLock::acquire`]. The next caller can scan once
/// it is dropped, so it must be held until the scan has stopped.
///
/// While holding a permit, never wait for another lock or another scan: only
/// start, stop, sleep and read the adapter's known peripherals. Callers may hold
/// their own locks while they wait for the permit, because its holder never
/// waits for them.
pub(crate) struct ScanPermit {
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

static SCAN_LOCK: LazyLock<ScanLock> = LazyLock::new(ScanLock::default);

/// The [`ScanLock`] that every scan in this process takes. See [`ScanPermit`]
/// for what its holder may wait for.
pub(crate) fn scan_lock() -> &'static ScanLock {
    &SCAN_LOCK
}

/// One scan window: start, wait `duration`, stop.
///
/// The window runs as a task on `runtime`, so it finishes even if this future is
/// dropped (a timeout, an aborted task, or a caller whose runtime shuts down).
/// Dropping the future ends the wait early, but the scan is still stopped, and
/// `permit` is released only once `stop` has returned, so the next window can't
/// start while this one is still stopping.
async fn scan_window<S: ScanControl>(
    runtime: &Handle,
    scanner: &S,
    permit: ScanPermit,
    filter: ScanFilter,
    duration: Duration,
) -> Result<()> {
    let scanner = scanner.clone();
    let cancel = CancellationToken::new();
    // Dropping the caller's future drops this guard, which ends the window early.
    let _end_early_on_drop = cancel.clone().drop_guard();
    let window = runtime.spawn(async move {
        let _permit = permit; // released only after stop has returned
        if cancel.is_cancelled() {
            return Ok(());
        }
        scanner.start(filter).await?; // a failed start has nothing to stop
        let _ = cancel.run_until_cancelled(sleep(duration)).await;
        let stopped = scanner.stop().await;
        if let Err(e) = &stopped {
            // Nobody may be awaiting this task any more.
            warn!("Failed to stop the Bluetooth scan: {e}");
        }
        stopped
    });
    window.await.map_err(std::io::Error::from)?
}

/// Scan with `scanner` (the adapter, in production) for `duration`, holding
/// `permit` (from [`scan_lock`]) until the scan has stopped. The window runs on
/// aranet-core's runtime.
pub(crate) async fn run_scan<S: ScanControl>(
    scanner: &S,
    permit: ScanPermit,
    filter: ScanFilter,
    duration: Duration,
) -> Result<()> {
    scan_window(
        &crate::runtime::handle()?,
        scanner,
        permit,
        filter,
        duration,
    )
    .await
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
        "Starting BLE scan for {} seconds (service_filter={})...",
        options.duration.as_secs(),
        options.use_service_filter
    );

    // Create scan filter - optionally filter for Aranet service UUIDs
    let scan_filter = if options.use_service_filter {
        ScanFilter {
            services: vec![SAF_TEHNIKA_SERVICE_NEW, SAF_TEHNIKA_SERVICE_OLD],
        }
    } else {
        ScanFilter::default()
    };

    let permit = scan_lock().acquire().await;
    run_scan(adapter, permit, scan_filter, options.duration).await?;

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

/// Find a specific device using a pre-existing adapter.
///
/// This avoids creating a new btleplug `Manager` (and D-Bus connection) on
/// every call.  The caller is responsible for keeping the `Adapter` alive.
pub async fn find_device_with_adapter(
    adapter: &Adapter,
    identifier: &str,
    options: ScanOptions,
) -> Result<Peripheral> {
    find_device_with_adapter_progress(adapter, identifier, options, None).await
}

/// Find a specific device using a pre-existing adapter, with progress callback.
pub async fn find_device_with_adapter_progress(
    adapter: &Adapter,
    identifier: &str,
    options: ScanOptions,
    progress: Option<ProgressCallback>,
) -> Result<Peripheral> {
    let identifier_lower = identifier.to_lowercase();

    info!("Looking for device: {}", identifier);

    if let Some(peripheral) = find_peripheral_by_identifier(adapter, &identifier_lower).await? {
        info!("Found device in cache (no scan needed)");
        if let Some(ref cb) = progress {
            cb(FindProgress::CacheHit);
        }
        return Ok(peripheral);
    }

    let max_attempts: u32 = 3;
    let base_duration = options.duration.as_millis() as u64 / 2;
    let base_duration = Duration::from_millis(base_duration.max(2000));

    for attempt in 1..=max_attempts {
        let scan_duration = base_duration * attempt;
        let duration_secs = scan_duration.as_secs();

        let permit = scan_lock().acquire().await;
        // Another search may have scanned while this one waited for the permit.
        if let Some(peripheral) = find_peripheral_by_identifier(adapter, &identifier_lower).await? {
            info!("Found device while waiting to scan");
            if let Some(ref cb) = progress {
                cb(FindProgress::CacheHit);
            }
            return Ok(peripheral);
        }

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

        run_scan(adapter, permit, ScanFilter::default(), scan_duration).await?;

        if let Some(peripheral) = find_peripheral_by_identifier(adapter, &identifier_lower).await? {
            info!("Found device on attempt {}", attempt);
            if let Some(ref cb) = progress {
                cb(FindProgress::Found { attempt });
            }
            return Ok(peripheral);
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
    let peripheral =
        find_device_with_adapter_progress(&adapter, identifier, options, progress).await?;
    Ok((adapter, peripheral))
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

    use std::sync::atomic::{AtomicBool, Ordering};

    use futures::FutureExt;

    use crate::test_support::within;

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
            panic!("Expected ScanAttempt variant");
        }
    }

    #[test]
    fn test_find_progress_found() {
        let progress = FindProgress::Found { attempt: 1 };
        assert!(matches!(progress, FindProgress::Found { attempt: 1 }));
    }

    #[test]
    fn test_find_progress_retry_needed() {
        let progress = FindProgress::RetryNeeded { attempt: 2 };
        assert!(matches!(progress, FindProgress::RetryNeeded { attempt: 2 }));
    }

    #[test]
    fn test_find_progress_clone() {
        let progress1 = FindProgress::ScanAttempt {
            attempt: 1,
            total: 3,
            duration_secs: 4,
        };
        let progress2 = progress1.clone();

        assert!(matches!(
            (&progress1, &progress2),
            (
                FindProgress::ScanAttempt {
                    attempt: 1,
                    total: 3,
                    duration_secs: 4,
                },
                FindProgress::ScanAttempt {
                    attempt: 1,
                    total: 3,
                    duration_secs: 4,
                },
            )
        ));
    }

    // ==================== Bluetooth Manager Tests ====================

    /// bluez-async spawns the D-Bus connection's only I/O task on the runtime
    /// that creates the manager, and `shared_manager` caches the manager for the
    /// whole process, so that task has to outlive the runtime that created it.
    /// Needs a system bus but not BlueZ: an error reply (no `org.bluez` on the
    /// bus) still proves the connection works; only a missing reply fails.
    ///
    /// To run it on a Linux host:
    /// `cargo test --locked -p aranet-core --lib manager_still_answers -- --ignored`.
    /// In a Debian container, start a throwaway system bus first:
    /// `apt-get install dbus; mkdir -p /run/dbus; dbus-daemon --system --fork`.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "needs a system D-Bus (BlueZ not required)"]
    fn manager_still_answers_after_its_first_runtime_shuts_down() {
        let runtime = || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
        };

        let first = runtime();
        let manager = first.block_on(shared_manager()).expect("manager");
        drop(first);

        let second = runtime();
        let answered = second.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), manager.adapters()).await
        });
        assert!(
            answered.is_ok(),
            "the manager's D-Bus connection died with the runtime that created it"
        );
    }

    // ==================== Scan Window Tests ====================

    /// Longest any scan-window test may take on the paused clock.
    const TEST_LIMIT: Duration = Duration::from_secs(600);

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// A scanner that behaves like BlueZ: one discovery session per D-Bus
    /// client, so a second `start` before `stop` fails with InProgress. It logs
    /// when each start and stop happened, measured from its creation.
    #[derive(Clone)]
    struct FakeScanner {
        started_at: tokio::time::Instant,
        log: Arc<std::sync::Mutex<Vec<(Duration, &'static str)>>>,
        scanning: Arc<AtomicBool>,
        fail_start: bool,
        /// How long `stop` takes to end the session: StopDiscovery is a D-Bus
        /// round trip on BlueZ.
        stop_latency: Duration,
        on_stop: Option<std::sync::mpsc::Sender<()>>,
    }

    impl FakeScanner {
        fn new() -> Self {
            Self {
                started_at: tokio::time::Instant::now(),
                log: Arc::default(),
                scanning: Arc::default(),
                fail_start: false,
                stop_latency: Duration::ZERO,
                on_stop: None,
            }
        }

        fn record(&self, event: &'static str) {
            self.log
                .lock()
                .unwrap()
                .push((self.started_at.elapsed(), event));
        }

        fn log(&self) -> Vec<(Duration, &'static str)> {
            self.log.lock().unwrap().clone()
        }

        fn is_scanning(&self) -> bool {
            self.scanning.load(Ordering::SeqCst)
        }
    }

    impl ScanControl for FakeScanner {
        async fn start(&self, _filter: ScanFilter) -> Result<()> {
            if self.fail_start {
                return Err(Error::InvalidData("start failed".into()));
            }
            if self.scanning.swap(true, Ordering::SeqCst) {
                return Err(Error::InvalidData("org.bluez.Error.InProgress".into()));
            }
            self.record("start");
            Ok(())
        }

        async fn stop(&self) -> Result<()> {
            self.record("stop");
            sleep(self.stop_latency).await;
            self.scanning.store(false, Ordering::SeqCst);
            if let Some(on_stop) = &self.on_stop {
                let _ = on_stop.send(());
            }
            Ok(())
        }
    }

    #[tokio::test(start_paused = true)]
    async fn scan_window_stops_the_scan_when_the_window_ends() {
        within(TEST_LIMIT, async {
            let rt = tokio::runtime::Handle::current();
            let lock = ScanLock::default();
            let scanner = FakeScanner::new();

            let permit = lock.acquire().await;
            scan_window(&rt, &scanner, permit, ScanFilter::default(), secs(5))
                .await
                .unwrap();

            assert_eq!(scanner.log(), [(secs(0), "start"), (secs(5), "stop")]);
            assert!(!scanner.is_scanning());
        })
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_the_caller_stops_the_scan_immediately() {
        within(TEST_LIMIT, async {
            let rt = tokio::runtime::Handle::current();
            let lock = ScanLock::default();
            let scanner = FakeScanner::new();

            let permit = lock.acquire().await;
            let window = scan_window(&rt, &scanner, permit, ScanFilter::default(), secs(5));
            assert!(tokio::time::timeout(secs(1), window).await.is_err());
            tokio::time::sleep(Duration::from_millis(10)).await;

            assert_eq!(scanner.log(), [(secs(0), "start"), (secs(1), "stop")]);
            assert!(!scanner.is_scanning());
        })
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn aborting_the_task_stops_the_scan() {
        within(TEST_LIMIT, async {
            let rt = tokio::runtime::Handle::current();
            let lock = ScanLock::default();
            let scanner = FakeScanner::new();

            // A service reload or stop aborts its tasks like this (`abort_all`).
            let task = tokio::spawn({
                let scanner = scanner.clone();
                let lock = lock.clone();
                async move {
                    let permit = lock.acquire().await;
                    scan_window(&rt, &scanner, permit, ScanFilter::default(), secs(5)).await
                }
            });
            tokio::time::sleep(secs(1)).await;
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            tokio::time::sleep(Duration::from_millis(10)).await;

            assert_eq!(scanner.log(), [(secs(0), "start"), (secs(1), "stop")]);
            assert!(!scanner.is_scanning());
        })
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn failed_start_releases_the_permit_without_stopping() {
        within(TEST_LIMIT, async {
            let rt = tokio::runtime::Handle::current();
            let lock = ScanLock::default();
            let scanner = FakeScanner {
                fail_start: true,
                ..FakeScanner::new()
            };

            let permit = lock.acquire().await;
            let result = scan_window(&rt, &scanner, permit, ScanFilter::default(), secs(5)).await;

            assert!(matches!(result, Err(Error::InvalidData(ref m)) if m == "start failed"));
            assert!(
                scanner.log().is_empty(),
                "a scan that never started must not be stopped"
            );
            within(secs(1), lock.acquire()).await;
        })
        .await;
    }

    #[test]
    fn scan_stops_after_the_callers_runtime_shuts_down() {
        let (tx, rx) = std::sync::mpsc::channel();
        let caller = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        caller.block_on(async {
            let lock = ScanLock::default();
            let scanner = FakeScanner {
                on_stop: Some(tx),
                ..FakeScanner::new()
            };
            let permit = lock.acquire().await;
            // Through `run_scan`, which picks the runtime the window runs on.
            let window = run_scan(&scanner, permit, ScanFilter::default(), secs(10));
            let started = async {
                while !scanner.is_scanning() {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            };
            // Give up on the window once its scan is running, however late the
            // aranet-ble thread gets to it.
            tokio::select! {
                result = window => panic!("the 10 s window ended early: {result:?}"),
                started = tokio::time::timeout(secs(5), started) => {
                    started.expect("the scan never started");
                }
            }
        });
        drop(caller);

        rx.recv_timeout(secs(5))
            .expect("scan never stopped after the caller's runtime shut down");
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_scans_run_one_after_another() {
        within(TEST_LIMIT, async {
            let rt = tokio::runtime::Handle::current();
            let lock = ScanLock::default();
            let scanner = FakeScanner {
                stop_latency: Duration::from_millis(100),
                ..FakeScanner::new()
            };
            let scan = || async {
                let permit = lock.acquire().await;
                scan_window(&rt, &scanner, permit, ScanFilter::default(), secs(2)).await
            };

            let (first, second) = tokio::join!(scan(), scan());

            first.expect("first scan");
            second.expect("second scan should wait for the first instead of failing");
            assert_eq!(
                scanner.log(),
                [
                    (secs(0), "start"),
                    (secs(2), "stop"),
                    (Duration::from_millis(2100), "start"),
                    (Duration::from_millis(4100), "stop"),
                ]
            );
        })
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn next_scan_waits_until_a_cancelled_scan_has_stopped() {
        within(TEST_LIMIT, async {
            let rt = tokio::runtime::Handle::current();
            let lock = ScanLock::default();
            let scanner = FakeScanner::new();

            let first = tokio::time::timeout(secs(1), async {
                let permit = lock.acquire().await;
                scan_window(&rt, &scanner, permit, ScanFilter::default(), secs(5)).await
            });
            let second = async {
                tokio::task::yield_now().await;
                let permit = lock.acquire().await;
                scan_window(&rt, &scanner, permit, ScanFilter::default(), secs(2)).await
            };
            let (first, second) = tokio::join!(first, second);

            assert!(first.is_err(), "the first scan should have been cancelled");
            second.expect("the second scan should start after the first has stopped");
            assert_eq!(
                scanner.log(),
                [
                    (secs(0), "start"),
                    (secs(1), "stop"),
                    (secs(1), "start"),
                    (secs(3), "stop"),
                ]
            );
        })
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_while_waiting_for_the_permit_never_starts_a_scan() {
        within(TEST_LIMIT, async {
            let rt = tokio::runtime::Handle::current();
            let lock = ScanLock::default();
            let scanner = FakeScanner::new();

            let permit = lock.acquire().await;
            let first = tokio::spawn({
                let (rt, scanner) = (rt.clone(), scanner.clone());
                async move {
                    scan_window(&rt, &scanner, permit, ScanFilter::default(), secs(5)).await
                }
            });
            let second = tokio::time::timeout(secs(1), async {
                let permit = lock.acquire().await;
                scan_window(&rt, &scanner, permit, ScanFilter::default(), secs(1)).await
            });
            assert!(
                second.await.is_err(),
                "the second scan should still be waiting for the permit"
            );
            tokio::time::sleep(secs(6)).await;
            first.await.unwrap().unwrap();
            assert_eq!(scanner.log(), [(secs(0), "start"), (secs(5), "stop")]);

            // A caller dropped after taking the permit, before its window task
            // first ran: `now_or_never` polls the window once, then drops it.
            let permit = lock.acquire().await;
            let window = scan_window(&rt, &scanner, permit, ScanFilter::default(), secs(1));
            assert!(window.now_or_never().is_none());
            tokio::time::sleep(Duration::from_millis(10)).await;
            assert_eq!(scanner.log(), [(secs(0), "start"), (secs(5), "stop")]);
            within(secs(1), lock.acquire()).await;
        })
        .await;
    }

    // ==================== DiscoveredDevice Tests ====================
    // Note: DiscoveredDevice tests are removed because PeripheralId from btleplug
    // has platform-specific implementations that cannot be easily mocked in tests.
    // - macOS: PeripheralId wraps a UUID
    // - Linux: PeripheralId wraps bluez_async::DeviceId (not directly accessible)
    // - Windows: PeripheralId wraps a u64
    //
    // The DiscoveredDevice struct derives Clone and Debug, so these traits are
    // guaranteed to work correctly by the compiler.
}
