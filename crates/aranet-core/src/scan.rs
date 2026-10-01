//! Device discovery and scanning.
//!
//! This module provides functionality to scan for Aranet devices
//! using Bluetooth Low Energy.
//!
//! Scans in one process run one at a time: each scan window takes a
//! process-wide permit and releases it only after the scan has stopped. A
//! window runs on aranet-core's background runtime and stops as soon as its
//! caller is dropped.

use std::ops::ControlFlow;
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
use crate::util::create_identifier;
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
        aranet_service_filter()
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

/// A scan filter that asks the Bluetooth stack only for devices that advertise
/// an Aranet service.
fn aranet_service_filter() -> ScanFilter {
    ScanFilter {
        services: vec![SAF_TEHNIKA_SERVICE_NEW, SAF_TEHNIKA_SERVICE_OLD],
    }
}

/// Whether device searches ask the Bluetooth stack only for Aranet sensors until
/// their last attempt (see `search_filter`). Only on macOS: btleplug's
/// CoreBluetooth backend keeps every device a scan reports for the rest of the
/// process and leaks a little memory for every advertisement it receives, so
/// unfiltered searches near many Bluetooth devices make a long-running program
/// grow by 1-2 MB an hour. Linux and Windows searches ask for every device.
///
/// Workaround for btleplug 0.11.8
/// (<https://github.com/deviceplug/btleplug/issues/494>); re-check when
/// upgrading btleplug.
const FILTER_SEARCH_UNTIL_LAST_ATTEMPT: bool = cfg!(target_os = "macos");

/// The scan filter of attempt `attempt` (counted from 1) of a device search that
/// makes `max_attempts`. With `filter_until_last`, every attempt but the last
/// asks only for devices that advertise an Aranet service, and the last asks for
/// every device, so a sensor that doesn't advertise one is still found. Without
/// it, every attempt asks for every device.
fn search_filter(attempt: u32, max_attempts: u32, filter_until_last: bool) -> ScanFilter {
    if filter_until_last && attempt < max_attempts {
        aranet_service_filter()
    } else {
        ScanFilter::default()
    }
}

/// What a device search does after its scan window `attempt` (counted from 1) of
/// `max_attempts` found no device that `query` names, where `similar` are the
/// names that contain the query (from `Search::Missing`): `Continue` to scan
/// again, or `Break` with the error the search fails with.
///
/// Similar names end the search at once with `NoExactMatch`, so a query that is
/// part of a name, the commonest mistake, fails after the first window that
/// leaves such names known instead of after the last window. Scanning on could
/// still find a device that the query names only if no window has heard that
/// device yet and its whole name is part of the name of one that has been
/// heard. Without similar names the search scans again until its last window,
/// then fails with `NotFound`.
fn after_missed_scan(
    query: &str,
    similar: Vec<String>,
    attempt: u32,
    max_attempts: u32,
) -> ControlFlow<Error> {
    use crate::error::DeviceNotFoundReason;

    if !similar.is_empty() {
        return ControlFlow::Break(Error::DeviceNotFound(DeviceNotFoundReason::NoExactMatch {
            identifier: query.to_string(),
            similar,
        }));
    }
    if attempt < max_attempts {
        return ControlFlow::Continue(());
    }
    ControlFlow::Break(Error::device_not_found(query))
}

/// Find a device by its address, identifier or full name.
///
/// `identifier` is trimmed and case is ignored, but otherwise it must match one
/// of these exactly:
/// - the device's [`DiscoveredDevice::identifier`]: the MAC address on Linux
///   and Windows, the CoreBluetooth UUID on macOS;
/// - btleplug's device ID as [`DiscoveredDevice::id`] displays it
///   (`hci0/dev_AA_BB_CC_DD_EE_FF` on Linux);
/// - the MAC address, with or without colons;
/// - the whole advertised name. On macOS a name shown as
///   `"Kitchen [Aranet4 1A2B3]"` also matches `Kitchen` or `Aranet4 1A2B3`.
///
/// An address or identifier match wins over a name match.
///
/// The device is searched for as [`find_device_with_options`] describes, with the
/// default [`ScanOptions`].
///
/// # Errors
///
/// - [`Error::InvalidConfig`] if `identifier` is empty or blank, before
///   Bluetooth is used.
/// - [`Error::DeviceNotFound`] with
///   [`DeviceNotFoundReason::Ambiguous`](crate::error::DeviceNotFoundReason::Ambiguous)
///   at once if several nearby devices match;
///   [`DeviceNotFoundReason::NoExactMatch`](crate::error::DeviceNotFoundReason::NoExactMatch)
///   if none matches but some Aranet device names contain `identifier`, after
///   the first scan that ends that way rather than after the last; and
///   [`DeviceNotFoundReason::NotFound`](crate::error::DeviceNotFoundReason::NotFound)
///   otherwise, after the last scan.
/// - [`Error::Bluetooth`] if the adapter fails.
pub async fn find_device(identifier: &str) -> Result<(Adapter, Peripheral)> {
    find_device_with_options(identifier, ScanOptions::default()).await
}

/// Find a specific device by name or address with custom options.
///
/// `identifier` must match exactly, as [`find_device`] describes.
///
/// This function uses a retry strategy to improve reliability:
/// 1. First checks if the device is already known (cached from previous scans)
/// 2. Performs up to 3 scan attempts with increasing durations
///
/// This helps with BLE reliability issues where devices may not appear
/// on every scan due to advertisement timing.
///
/// Only `options.duration` is used: the search ignores the filter flags of
/// `options`. On macOS every scan attempt but the last asks the Bluetooth stack
/// only for devices that advertise an Aranet service, and the last asks for every
/// device, so a sensor that doesn't advertise one is still found. On other
/// platforms every attempt asks for every device.
pub async fn find_device_with_options(
    identifier: &str,
    options: ScanOptions,
) -> Result<(Adapter, Peripheral)> {
    find_device_with_progress(identifier, options, None).await
}

/// Find a specific device using a pre-existing adapter.
///
/// `identifier` must match exactly, as [`find_device`] describes, and the device
/// is searched for as [`find_device_with_options`] describes.
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
///
/// `identifier` must match exactly, as [`find_device`] describes, and the device
/// is searched for as [`find_device_with_options`] describes.
pub async fn find_device_with_adapter_progress(
    adapter: &Adapter,
    identifier: &str,
    options: ScanOptions,
    progress: Option<ProgressCallback>,
) -> Result<Peripheral> {
    let query = parse_query(identifier)?;

    info!("Looking for device: {}", identifier);

    if let Search::Found(peripheral) = search_known_peripherals(adapter, query).await? {
        info!("Found device in cache (no scan needed)");
        if let Some(ref cb) = progress {
            cb(FindProgress::CacheHit);
        }
        return Ok(peripheral);
    }

    let max_attempts: u32 = 3;
    let base_duration = options.duration.as_millis() as u64 / 2;
    let base_duration = Duration::from_millis(base_duration.max(2000));

    // Ends after `max_attempts` windows at the latest: `after_missed_scan`
    // stops the search after the last one.
    let mut attempt = 0;
    loop {
        attempt += 1;
        let scan_duration = base_duration * attempt;
        let duration_secs = scan_duration.as_secs();

        let permit = scan_lock().acquire().await;
        // Another search may have scanned while this one waited for the permit.
        if let Search::Found(peripheral) = search_known_peripherals(adapter, query).await? {
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
        let filter = search_filter(attempt, max_attempts, FILTER_SEARCH_UNTIL_LAST_ATTEMPT);
        debug!(
            "Scan attempt {}/{} ({}s, {})",
            attempt,
            max_attempts,
            duration_secs,
            if filter.services.is_empty() {
                "all devices"
            } else {
                "Aranet sensors only"
            }
        );

        if let Some(ref cb) = progress {
            cb(FindProgress::ScanAttempt {
                attempt,
                total: max_attempts,
                duration_secs,
            });
        }

        run_scan(adapter, permit, filter, scan_duration).await?;

        let similar = match search_known_peripherals(adapter, query).await? {
            Search::Found(peripheral) => {
                info!("Found device on attempt {}", attempt);
                if let Some(ref cb) = progress {
                    cb(FindProgress::Found { attempt });
                }
                return Ok(peripheral);
            }
            Search::Missing { similar } => similar,
        };

        match after_missed_scan(query, similar, attempt, max_attempts) {
            ControlFlow::Break(error) => {
                warn!(
                    "Device not found after {} of {} attempts: {}",
                    attempt, max_attempts, identifier
                );
                return Err(error);
            }
            ControlFlow::Continue(()) => {
                warn!("Device not found, retrying...");
                if let Some(ref cb) = progress {
                    cb(FindProgress::RetryNeeded { attempt });
                }
            }
        }
    }
}

/// Find a specific device with progress callback for UI feedback.
///
/// `identifier` must match exactly, as [`find_device`] describes, and the device
/// is searched for as [`find_device_with_options`] describes.
///
/// The progress callback is called with updates about the search progress,
/// including cache hits, scan attempts, and retry information.
pub async fn find_device_with_progress(
    identifier: &str,
    options: ScanOptions,
    progress: Option<ProgressCallback>,
) -> Result<(Adapter, Peripheral)> {
    // Reject an empty identifier before Bluetooth is touched.
    parse_query(identifier)?;
    let adapter = get_adapter().await?;
    let peripheral =
        find_device_with_adapter_progress(&adapter, identifier, options, progress).await?;
    Ok((adapter, peripheral))
}

/// The address CoreBluetooth reports for every peripheral. It identifies
/// nothing, so a query never matches it.
const UNKNOWN_ADDRESS: &str = "00:00:00:00:00:00";

/// What a device lookup knows about one peripheral the adapter has seen.
#[derive(Debug, Clone, PartialEq, Eq)]
struct KnownPeripheral {
    /// What `aranet scan` prints (`create_identifier`): the MAC address on
    /// Linux and Windows, the CoreBluetooth UUID on macOS.
    identifier: String,
    /// btleplug's device ID in the form the TUI and GUI store
    /// (`peripheral.id().to_string()`): `hci0/dev_AA_BB_CC_DD_EE_FF` on Linux,
    /// the UUID on macOS, the MAC address on Windows.
    peripheral_id: String,
    /// The Bluetooth address; `00:00:00:00:00:00` on macOS.
    address: String,
    /// The advertised name.
    name: Option<String>,
}

/// The peripherals a query picks, as indices into the slice given to `lookup`.
#[derive(Debug, PartialEq, Eq)]
enum Lookup {
    /// Exactly one peripheral matches.
    Found(usize),
    /// Several peripherals match, in ascending order.
    Ambiguous(Vec<usize>),
    /// Nothing matches. `similar` are the peripherals whose name contains both
    /// the query and `aranet`, ordered by name and then identifier.
    NotFound { similar: Vec<usize> },
}

/// Trims `identifier`. An identifier that is empty after trimming is
/// `Error::InvalidConfig("device identifier is empty")`.
fn parse_query(identifier: &str) -> Result<&str> {
    let query = identifier.trim();
    if query.is_empty() {
        return Err(Error::invalid_config("device identifier is empty"));
    }
    Ok(query)
}

/// Pick the peripheral that `query` names, ignoring case. `query` comes from
/// `parse_query`, so it is trimmed and not empty.
///
/// The rules, in order. A later rule is used only when the earlier ones match
/// nothing, so a device's own address beats a device named like it:
/// 1. an identifier: the one `aranet scan` prints, btleplug's device ID (the
///    `hci0/dev_…` form on Linux), or the Bluetooth address with or without
///    colons, never `00:00:00:00:00:00`;
/// 2. the whole advertised name, or either half of CoreBluetooth's combined
///    `"<GAP name> [<advertised name>]"` (btleplug 0.11.8,
///    `corebluetooth/internal.rs:577-585`).
///
/// One match is `Found` and several are `Ambiguous`. With no match, `NotFound`
/// lists the peripherals whose name contains both `query` and `aranet`, so a
/// short query never lists every phone and headset nearby. The answer depends
/// only on which peripherals are known, never on their order.
fn lookup(query: &str, known: &[KnownPeripheral]) -> Lookup {
    let query = query.to_lowercase();
    let bare_query = query.replace(':', "");

    let by_identifier = matching(known, |peripheral| {
        peripheral.identifier.to_lowercase() == query
            || peripheral.peripheral_id.to_lowercase() == query
            || (peripheral.address != UNKNOWN_ADDRESS
                && peripheral.address.to_lowercase().replace(':', "") == bare_query)
    });
    if let Some(found) = decide(by_identifier) {
        return found;
    }

    let by_name = matching(known, |peripheral| {
        peripheral
            .name
            .as_deref()
            .is_some_and(|name| name_matches(name, &query))
    });
    if let Some(found) = decide(by_name) {
        return found;
    }

    let mut similar = matching(known, |peripheral| {
        peripheral.name.as_deref().is_some_and(|name| {
            let name = name.to_lowercase();
            name.contains("aranet") && name.contains(&query)
        })
    });
    similar.sort_by_key(|&index| (&known[index].name, &known[index].identifier));
    Lookup::NotFound { similar }
}

/// Indices of the peripherals that satisfy `predicate`, in ascending order.
fn matching(known: &[KnownPeripheral], predicate: impl Fn(&KnownPeripheral) -> bool) -> Vec<usize> {
    known
        .iter()
        .enumerate()
        .filter_map(|(index, peripheral)| predicate(peripheral).then_some(index))
        .collect()
}

/// `Found` for one index, `Ambiguous` for several, `None` for none.
fn decide(indices: Vec<usize>) -> Option<Lookup> {
    match indices.len() {
        0 => None,
        1 => Some(Lookup::Found(indices[0])),
        _ => Some(Lookup::Ambiguous(indices)),
    }
}

/// Whether the advertised `name` is `query` (lower case): the whole name, or
/// either half of CoreBluetooth's `"<GAP name> [<advertised name>]"`.
fn name_matches(name: &str, query: &str) -> bool {
    let name = name.trim().to_lowercase();
    name == query
        || name
            .strip_suffix(']')
            .and_then(|combined| combined.rsplit_once(" ["))
            .is_some_and(|(gap, advertised)| gap.trim() == query || advertised.trim() == query)
}

/// What a search of the known peripherals found.
#[derive(Debug, PartialEq, Eq)]
enum Search<T> {
    /// The one peripheral the query names.
    Found(T),
    /// Nothing matches; `similar` are the names that contain the query,
    /// trimmed, sorted and without duplicates.
    Missing { similar: Vec<String> },
}

/// `lookup`'s answer for `query`, with `Found` holding an index into `known`.
///
/// An ambiguous query is an error,
/// `Error::DeviceNotFound(DeviceNotFoundReason::Ambiguous { .. })`, whose
/// candidates are `"<name or 'unnamed'> (<identifier>)"`, sorted: scanning
/// again can't make it less ambiguous, so the find loop returns it at once.
fn resolve(query: &str, known: &[KnownPeripheral]) -> Result<Search<usize>> {
    use crate::error::DeviceNotFoundReason;

    match lookup(query, known) {
        Lookup::Found(index) => Ok(Search::Found(index)),
        Lookup::Ambiguous(indices) => {
            let mut candidates: Vec<String> = indices
                .iter()
                .map(|&index| {
                    let device = &known[index];
                    let name = device.name.as_deref().unwrap_or("unnamed");
                    format!("{name} ({})", device.identifier)
                })
                .collect();
            candidates.sort();
            Err(Error::DeviceNotFound(DeviceNotFoundReason::Ambiguous {
                identifier: query.to_string(),
                candidates,
            }))
        }
        Lookup::NotFound { similar } => {
            let mut names: Vec<String> = similar
                .iter()
                .filter_map(|&index| known[index].name.as_deref())
                .map(|name| name.trim().to_string())
                .collect();
            names.sort();
            names.dedup();
            Ok(Search::Missing { similar: names })
        }
    }
}

/// Look `query` up among the peripherals that `adapter` already knows, as
/// `resolve` decides.
async fn search_known_peripherals(adapter: &Adapter, query: &str) -> Result<Search<Peripheral>> {
    let mut peripherals = Vec::new();
    let mut known = Vec::new();
    for peripheral in adapter.peripherals().await? {
        if let Ok(Some(props)) = peripheral.properties().await {
            let id = peripheral.id();
            let address = props.address.to_string();
            known.push(KnownPeripheral {
                identifier: create_identifier(&address, &id),
                peripheral_id: id.to_string(),
                address,
                name: props.local_name,
            });
            peripherals.push(peripheral);
        }
    }

    match resolve(query, &known)? {
        Search::Found(index) => {
            let device = &known[index];
            debug!("Matched {:?} ({})", device.name, device.identifier);
            Ok(Search::Found(peripherals.swap_remove(index)))
        }
        Search::Missing { similar } => Ok(Search::Missing { similar }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicBool, Ordering};

    use futures::FutureExt;

    use crate::error::DeviceNotFoundReason;
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

    // ==================== Search Filter Tests ====================

    /// The filters that the attempts of a search with `max_attempts` use, in order.
    fn search_filters(max_attempts: u32, filter_until_last: bool) -> Vec<ScanFilter> {
        (1..=max_attempts)
            .map(|attempt| search_filter(attempt, max_attempts, filter_until_last))
            .collect()
    }

    #[test]
    fn search_filter_asks_only_for_aranet_sensors_before_the_last_attempt() {
        let aranet = || ScanFilter {
            services: vec![SAF_TEHNIKA_SERVICE_NEW, SAF_TEHNIKA_SERVICE_OLD],
        };
        let every_device = ScanFilter::default;
        assert_eq!(search_filters(1, true), [every_device()]);
        assert_eq!(search_filters(2, true), [aranet(), every_device()]);
        assert_eq!(
            search_filters(3, true),
            [aranet(), aranet(), every_device()]
        );
    }

    #[test]
    fn search_filter_asks_for_every_device_when_not_filtering() {
        for max_attempts in 1..=3 {
            assert_eq!(
                search_filters(max_attempts, false),
                vec![ScanFilter::default(); max_attempts as usize],
                "{max_attempts} attempts"
            );
        }
    }

    // ==================== Missed Scan Tests ====================

    #[test]
    fn a_search_fails_after_the_first_scan_that_finds_only_similar_names() {
        // A partial name such as `-d Aranet4` fails as soon as a scan window
        // ends with names that contain it, not after the last window.
        for attempt in 1..=3 {
            let similar = vec!["Aranet4 12345".to_string(), "Aranet4 1ABCD".to_string()];
            match after_missed_scan("Aranet4", similar, attempt, 3) {
                ControlFlow::Break(Error::DeviceNotFound(DeviceNotFoundReason::NoExactMatch {
                    identifier,
                    similar,
                })) => {
                    assert_eq!(identifier, "Aranet4", "attempt {attempt}");
                    assert_eq!(
                        similar,
                        ["Aranet4 12345", "Aranet4 1ABCD"],
                        "attempt {attempt}"
                    );
                }
                other => panic!("attempt {attempt} of 3: {other:?}"),
            }
        }
    }

    #[test]
    fn a_search_without_similar_names_scans_until_its_last_attempt() {
        for attempt in 1..3 {
            let decision = after_missed_scan("Aranet4 12345", Vec::new(), attempt, 3);
            assert!(
                matches!(decision, ControlFlow::Continue(())),
                "attempt {attempt} of 3: {decision:?}"
            );
        }
        match after_missed_scan("Aranet4 12345", Vec::new(), 3, 3) {
            ControlFlow::Break(Error::DeviceNotFound(DeviceNotFoundReason::NotFound {
                identifier,
            })) => assert_eq!(identifier, "Aranet4 12345"),
            other => panic!("attempt 3 of 3: {other:?}"),
        }
    }

    // ==================== Device Lookup Tests ====================

    /// CoreBluetooth UUIDs as `aranet scan` prints them on macOS. The first two
    /// are those of a real Aranet2 and AranetRn+; the other two are made up.
    const UUID_1: &str = "1f8893bf-9f7e-02b4-ef4a-7718f4f5d4be";
    const UUID_2: &str = "387c18c7-299f-cc32-d01c-6cf29a8d3ca5";
    const UUID_3: &str = "5b0e4c1d-7a3f-4e2b-9c6d-8f1a2b3c4d5e";
    const UUID_4: &str = "c4a7e2d9-3b1f-4c8e-a6d5-2f9b8e7c1a04";

    fn known(
        identifier: &str,
        peripheral_id: &str,
        address: &str,
        name: Option<&str>,
    ) -> KnownPeripheral {
        KnownPeripheral {
            identifier: identifier.to_string(),
            peripheral_id: peripheral_id.to_string(),
            address: address.to_string(),
            name: name.map(str::to_string),
        }
    }

    /// A peripheral as CoreBluetooth reports it: a UUID and no address.
    fn on_macos(uuid: &str, name: &str) -> KnownPeripheral {
        known(uuid, uuid, UNKNOWN_ADDRESS, Some(name))
    }

    /// A peripheral as BlueZ reports it: its address, and the device ID
    /// `hci0/dev_AA_BB_…` that btleplug's `PeripheralId` displays.
    fn on_linux(address: &str, name: &str) -> KnownPeripheral {
        let device_id = format!("hci0/dev_{}", address.replace(':', "_"));
        known(address, &device_id, address, Some(name))
    }

    /// `lookup`'s answer as its kind and the identifiers it picked, so answers
    /// for the same peripherals in different orders can be compared.
    fn outcome(query: &str, known: &[KnownPeripheral]) -> (&'static str, Vec<String>) {
        let (kind, indices) = match lookup(query, known) {
            Lookup::Found(index) => ("found", vec![index]),
            Lookup::Ambiguous(indices) => ("ambiguous", indices),
            Lookup::NotFound { similar } => ("not found", similar),
        };
        let mut identifiers: Vec<String> = indices
            .iter()
            .map(|&index| known[index].identifier.clone())
            .collect();
        if kind == "ambiguous" {
            // In slice order, which depends on the shuffle.
            identifiers.sort();
        }
        (kind, identifiers)
    }

    /// The candidates of the `Ambiguous` error that `resolve` returns.
    fn candidates(query: &str, known: &[KnownPeripheral]) -> Vec<String> {
        match resolve(query, known) {
            Err(Error::DeviceNotFound(crate::error::DeviceNotFoundReason::Ambiguous {
                identifier,
                candidates,
            })) => {
                assert_eq!(identifier, query);
                candidates
            }
            other => panic!("{query:?} is not ambiguous: {other:?}"),
        }
    }

    #[test]
    fn lookup_does_not_match_part_of_a_name() {
        let devices = [
            on_macos(UUID_1, "Aranet4 12345"),
            on_macos(UUID_2, "Aranet4 1ABCD"),
        ];
        assert_eq!(
            lookup("Aranet4 1", &devices),
            Lookup::NotFound {
                similar: vec![0, 1]
            }
        );
    }

    #[test]
    fn lookup_does_not_match_part_of_a_uuid() {
        let devices = [on_macos(UUID_1, "Aranet2 2751B")];
        for query in ["4", "1f8893bf"] {
            assert_eq!(
                lookup(query, &devices),
                Lookup::NotFound { similar: vec![] },
                "{query:?}"
            );
        }
    }

    #[test]
    fn lookup_ignores_linux_object_path_fragments() {
        let devices = [on_linux("AA:BB:CC:DD:EE:FF", "Aranet4 12345")];
        for query in ["hci0", "dev"] {
            assert_eq!(
                lookup(query, &devices),
                Lookup::NotFound { similar: vec![] },
                "{query:?}"
            );
        }
    }

    #[test]
    fn lookup_matches_the_bluez_device_id_display_form() {
        // The TUI and GUI keep `DiscoveredDevice::id.to_string()` as a device's
        // ID, store it in the database and connect with it later.
        let devices = [
            on_linux("11:22:33:44:55:66", "Aranet2 2751B"),
            on_linux("AA:BB:CC:DD:EE:FF", "Aranet4 12345"),
        ];
        for query in ["hci0/dev_AA_BB_CC_DD_EE_FF", "hci0/dev_aa_bb_cc_dd_ee_ff"] {
            assert_eq!(lookup(query, &devices), Lookup::Found(1), "{query:?}");
        }
    }

    #[test]
    fn lookup_matches_the_full_name_ignoring_case_and_whitespace() {
        let devices = [
            on_macos(UUID_2, "AranetRn+ 306B8"),
            on_macos(UUID_1, "Aranet2 2751B"),
        ];
        let query = parse_query(" aranet2 2751b ").unwrap();
        assert_eq!(lookup(query, &devices), Lookup::Found(1));
    }

    #[test]
    fn lookup_matches_a_uuid_in_any_case() {
        let devices = [
            on_macos(UUID_2, "AranetRn+ 306B8"),
            on_macos(UUID_1, "Aranet2 2751B"),
        ];
        assert_eq!(
            lookup("1F8893BF-9F7E-02B4-EF4A-7718F4F5D4BE", &devices),
            Lookup::Found(1)
        );
    }

    #[test]
    fn lookup_matches_an_address_with_or_without_colons() {
        let devices = [
            on_linux("11:22:33:44:55:66", "Aranet2 2751B"),
            on_linux("AA:BB:CC:DD:EE:FF", "Aranet4 12345"),
        ];
        for query in ["aa:bb:cc:dd:ee:ff", "AABBCCDDEEFF"] {
            assert_eq!(lookup(query, &devices), Lookup::Found(1), "{query:?}");
        }
    }

    #[test]
    fn lookup_never_matches_the_all_zero_address() {
        let devices = [on_macos(UUID_1, "Aranet2 2751B")];
        for query in ["00:00:00:00:00:00", "000000000000"] {
            assert_eq!(
                lookup(query, &devices),
                Lookup::NotFound { similar: vec![] },
                "{query:?}"
            );
        }
    }

    #[test]
    fn lookup_matches_either_half_of_a_corebluetooth_combined_name() {
        let devices = [
            on_macos(UUID_2, "AranetRn+ 306B8"),
            on_macos(UUID_1, "Kitchen [Aranet4 1A2B3]"),
        ];
        for query in ["Aranet4 1A2B3", "kitchen", "Kitchen [Aranet4 1A2B3]"] {
            assert_eq!(lookup(query, &devices), Lookup::Found(1), "{query:?}");
        }
    }

    #[test]
    fn lookup_prefers_an_identifier_match_over_a_name_match() {
        let devices = [
            on_macos(UUID_1, "AA:BB:CC:DD:EE:FF"),
            on_linux("AA:BB:CC:DD:EE:FF", "Aranet4 12345"),
        ];
        assert_eq!(lookup("aa:bb:cc:dd:ee:ff", &devices), Lookup::Found(1));
    }

    #[test]
    fn lookup_reports_duplicate_names_as_ambiguous() {
        let devices = [
            on_macos(UUID_1, "Aranet4 12345"),
            on_macos(UUID_2, "Aranet4 12345"),
        ];
        assert_eq!(
            lookup("Aranet4 12345", &devices),
            Lookup::Ambiguous(vec![0, 1])
        );
    }

    #[test]
    fn lookup_suggests_only_aranet_names() {
        // "Standing desk" contains the query too, but it isn't an Aranet.
        let devices = [
            on_macos(UUID_1, "Aranet4 12345"),
            on_macos(UUID_2, "Standing desk"),
            on_macos(UUID_3, "Aranet2 2751B"),
        ];
        assert_eq!(
            lookup("an", &devices),
            Lookup::NotFound {
                similar: vec![2, 0]
            }
        );
    }

    /// Every order of the indices `0..n`.
    fn orders(n: usize) -> Vec<Vec<usize>> {
        if n == 0 {
            return vec![Vec::new()];
        }
        let mut all = Vec::new();
        for shorter in orders(n - 1) {
            for at in 0..n {
                let mut order = shorter.clone();
                order.insert(at, n - 1);
                all.push(order);
            }
        }
        all
    }

    #[test]
    fn lookup_result_is_independent_of_order() {
        let devices = [
            on_macos(UUID_1, "Aranet4 12345"),
            on_macos(UUID_2, "Aranet4 1ABCD"),
            on_macos(UUID_3, "Aranet2 2751B"),
            // A second sensor with the first one's name.
            on_macos(UUID_4, "Aranet4 12345"),
        ];
        let cases = [
            ("Aranet4 1", "not found", vec![UUID_1, UUID_4, UUID_2]),
            ("aranet2 2751b", "found", vec![UUID_3]),
            ("Aranet4", "not found", vec![UUID_1, UUID_4, UUID_2]),
            ("Aranet4 12345", "ambiguous", vec![UUID_1, UUID_4]),
        ];
        for (query, kind, identifiers) in cases {
            let outcomes: Vec<(&str, Vec<String>)> = orders(devices.len())
                .iter()
                .map(|order| {
                    let shuffled: Vec<KnownPeripheral> =
                        order.iter().map(|&index| devices[index].clone()).collect();
                    outcome(query, &shuffled)
                })
                .collect();
            assert!(
                outcomes.iter().all(|answer| *answer == outcomes[0]),
                "the answer for {query:?} depends on the order: {outcomes:?}"
            );
            assert_eq!(outcomes[0].0, kind, "{query:?}");
            assert_eq!(outcomes[0].1, identifiers, "{query:?}");
        }
    }

    #[test]
    fn resolve_lists_candidates_and_similar_names_sorted() {
        // Two sensors share a name; the one listed second sorts first.
        let same_name = [
            on_macos(UUID_2, "Aranet4 12345"),
            on_macos(UUID_1, "Aranet4 12345"),
        ];
        assert_eq!(
            candidates("Aranet4 12345", &same_name),
            [
                format!("Aranet4 12345 ({UUID_1})"),
                format!("Aranet4 12345 ({UUID_2})"),
            ]
        );

        // Two entries with one address, the first without a name.
        let one_address = [
            known(
                "AA:BB:CC:DD:EE:FF",
                "hci1/dev_AA_BB_CC_DD_EE_FF",
                "AA:BB:CC:DD:EE:FF",
                None,
            ),
            on_linux("AA:BB:CC:DD:EE:FF", "Aranet4 12345"),
        ];
        assert_eq!(
            candidates("aa:bb:cc:dd:ee:ff", &one_address),
            [
                "Aranet4 12345 (AA:BB:CC:DD:EE:FF)",
                "unnamed (AA:BB:CC:DD:EE:FF)",
            ]
        );

        // Similar names come back trimmed, sorted and without duplicates.
        let similar = [
            on_macos(UUID_1, "Aranet4 1ABCD "),
            on_macos(UUID_2, "Aranet4 12345"),
            on_macos(UUID_3, "Aranet4 1ABCD"),
        ];
        assert_eq!(
            resolve("aranet4 1", &similar).unwrap(),
            Search::Missing {
                similar: vec!["Aranet4 12345".to_string(), "Aranet4 1ABCD".to_string()]
            }
        );
    }

    #[test]
    fn parse_query_rejects_empty_and_blank_identifiers() {
        for identifier in ["", "   ", "\t\n"] {
            assert!(
                matches!(parse_query(identifier), Err(Error::InvalidConfig(_))),
                "{identifier:?}"
            );
        }
        assert_eq!(parse_query(" x ").unwrap(), "x");
    }

    #[tokio::test]
    async fn find_device_rejects_an_empty_identifier_without_bluetooth() {
        // The real clock, unlike the other async tests here. Both calls must
        // return before they touch Bluetooth. If one reached the Bluetooth
        // stack, a paused clock would jump to the limit while the call waited
        // on the `aranet-ble` thread, and the failure would show a timeout
        // instead of the stack's own error. A correct call never waits, so the
        // 5 s limit matters only then.
        let found = within(Duration::from_secs(5), find_device("")).await;
        assert!(
            matches!(found, Err(Error::InvalidConfig(_))),
            "{:?}",
            found.err()
        );

        let connected = within(Duration::from_secs(5), crate::device::Device::connect("  ")).await;
        assert!(
            matches!(connected, Err(Error::InvalidConfig(_))),
            "{:?}",
            connected.err()
        );
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
