//! Aranet device connection and communication.
//!
//! This module provides the main interface for connecting to and
//! communicating with Aranet sensors over Bluetooth Low Energy.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use btleplug::api::{Characteristic, Peripheral as _, WriteType};
use btleplug::platform::{Adapter, Peripheral};
use tokio::sync::RwLock;
use tokio::time::timeout;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::scan::ScanOptions;
use crate::traits::AranetDevice;
use crate::util::{create_identifier, format_peripheral_id};
use crate::uuid::{
    BATTERY_LEVEL, BATTERY_SERVICE, CURRENT_READINGS_DETAIL, CURRENT_READINGS_DETAIL_ALT,
    DEVICE_INFO_SERVICE, DEVICE_NAME, FIRMWARE_REVISION, GAP_SERVICE, HARDWARE_REVISION,
    MANUFACTURER_NAME, MODEL_NUMBER, SAF_TEHNIKA_SERVICE_NEW, SAF_TEHNIKA_SERVICE_OLD,
    SERIAL_NUMBER, SOFTWARE_REVISION,
};
use aranet_types::{CurrentReading, DeviceInfo, DeviceType};

/// Represents a connected Aranet device.
///
/// # Note on Clone
///
/// This struct intentionally does not implement `Clone`. A `Device` represents
/// an active BLE connection with associated state (services discovered, notification
/// handlers, etc.). Cloning would create ambiguity about connection ownership and
/// could lead to resource conflicts. If you need to share a device across multiple
/// tasks, wrap it in `Arc<Device>`.
///
/// # Cleanup
///
/// You MUST call [`Device::disconnect`] before dropping the device to properly
/// release BLE resources. If a Device is dropped without calling disconnect,
/// a warning will be logged.
///
/// # Timeouts
///
/// With default settings, [`Device::connect`] gives up after about 30 s of scanning
/// on a device that the adapter doesn't already know and that isn't advertising. A
/// device that the adapter still lists (on Linux a paired device, or one seen in the
/// last 30 s or so; on macOS one that this process found in an earlier scan and
/// hasn't disconnected from since) is connected to without scanning. If it has gone,
/// the connect fails within about 20 s (15 s connecting, up to 5 s to confirm the
/// disconnect). A device that is found by scanning but refuses the connection fails
/// after about 50 s (30 s scanning, 15 s connecting, up to 5 s to confirm the
/// disconnect). One that connects slowly and then stops answering fails after at
/// most about 70 s (adding up to 10 s each for service discovery and for reading its
/// properties), and the rare retry after an empty service discovery can take about
/// 102 s. On Linux, service discovery can take up to 20 s instead of 10 s while BlueZ
/// is still discovering a device it has just connected (a first connection to an
/// Aranet4, for example), so those two cases can take about 80 s and 122 s there. A
/// search can also wait for another scan in the same process to finish. Use
/// [`Device::connect_with_scan_options`] to change the scan time and
/// [`ConnectionConfig`] to change the connect timeouts.
///
/// On Linux, a connection to a sensor that BlueZ doesn't list as paired pairs
/// it first, which can add up to `connection_timeout` plus the wait for
/// BlueZ's service discovery (`discovery_timeout`, but at least 20 s), plus
/// 5 s: 40 s by default. A sensor that refused to pair is connected without
/// pairing for 10 minutes.
pub struct Device {
    /// The BLE adapter used for connection.
    ///
    /// This field is stored to keep the adapter alive for the lifetime of the
    /// peripheral connection. The peripheral may hold internal references to
    /// the adapter, and dropping the adapter could invalidate the connection.
    #[allow(dead_code)]
    adapter: Adapter,
    /// The underlying BLE peripheral.
    peripheral: Peripheral,
    /// Cached device name.
    name: Option<String>,
    /// Device address or identifier (MAC address on Linux/Windows, UUID on macOS).
    address: String,
    /// Detected device type.
    device_type: Option<DeviceType>,
    /// Whether services have been discovered.
    services_discovered: bool,
    /// Cache of discovered characteristics by UUID for O(1) lookup.
    /// Built after service discovery to avoid searching through services on each read.
    characteristics_cache: RwLock<HashMap<Uuid, Characteristic>>,
    /// The notification task of each subscribed characteristic.
    notification_tasks: crate::link::NotificationTasks,
    /// Whether disconnect has been called (for Drop warning).
    disconnected: AtomicBool,
    /// Connection configuration (timeouts, etc.).
    config: ConnectionConfig,
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Provide a clean debug output that excludes internal BLE details
        // (adapter, peripheral, notification_tasks, characteristics_cache)
        // which are not useful for debugging application logic.
        f.debug_struct("Device")
            .field("name", &self.name)
            .field("address", &self.address)
            .field("device_type", &self.device_type)
            .field("services_discovered", &self.services_discovered)
            .finish_non_exhaustive()
    }
}

/// Default timeout for BLE characteristic read operations.
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Default timeout for BLE characteristic write operations.
const DEFAULT_WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// Default timeout for BLE connection operations.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Default timeout for service discovery.
const DEFAULT_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);

/// Default timeout for connection validation (keepalive check).
const DEFAULT_VALIDATION_TIMEOUT: Duration = Duration::from_secs(3);

/// Scan duration of the one device search that `connect`, `connect_with_timeout`,
/// `connect_with_config` and `connect_with_adapter` run: scans of 5, 10 and 15 s.
const CONNECT_SCAN_DURATION: Duration = Duration::from_secs(10);

/// Current-readings characteristics to try, in order, for a device type.
///
/// Connection checks read them too, instead of Battery Level (0x2A19), which
/// needs pairing.
fn readings_characteristics(device_type: Option<DeviceType>) -> &'static [Uuid] {
    match device_type {
        Some(DeviceType::Aranet4) => &[CURRENT_READINGS_DETAIL],
        Some(DeviceType::Aranet2 | DeviceType::AranetRadon | DeviceType::AranetRadiation) => {
            &[CURRENT_READINGS_DETAIL_ALT]
        }
        // Unknown, or a type newer than this crate (`DeviceType` is
        // `#[non_exhaustive]`): try the Aranet4 characteristic first.
        None | Some(_) => &[CURRENT_READINGS_DETAIL, CURRENT_READINGS_DETAIL_ALT],
    }
}

/// Reads the first of `candidates` that the device has, using `read`.
///
/// Moves to the next candidate only when `read` returns `CharacteristicNotFound`.
/// Any other error, such as a timeout or a lost link, is returned at once.
async fn read_first_available<F, Fut>(candidates: &[Uuid], mut read: F) -> Result<Vec<u8>>
where
    F: FnMut(Uuid) -> Fut,
    Fut: Future<Output = Result<Vec<u8>>>,
{
    let Some((&last, earlier)) = candidates.split_last() else {
        return Err(Error::Unsupported(
            "no current-readings characteristic for this device type".into(),
        ));
    };
    for &uuid in earlier {
        match read(uuid).await {
            Ok(data) => return Ok(data),
            Err(Error::CharacteristicNotFound { .. }) => {
                debug!("Reading characteristic {uuid} not found, trying the next one");
            }
            Err(e) => return Err(e),
        }
    }
    read(last).await
}

/// Configuration for BLE connection timeouts and behavior.
///
/// Use this to customize timeout values for different environments.
/// For example, increase timeouts in challenging RF environments
/// (concrete walls, electromagnetic interference).
///
/// # Example
///
/// ```no_run
/// use std::time::Duration;
/// use aranet_core::device::ConnectionConfig;
///
/// // Create a config for challenging RF environments
/// let config = ConnectionConfig::default()
///     .connection_timeout(Duration::from_secs(20))
///     .read_timeout(Duration::from_secs(15));
/// ```
#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    /// Timeout for establishing a BLE connection.
    pub connection_timeout: Duration,
    /// Timeout for BLE read operations.
    pub read_timeout: Duration,
    /// Timeout for BLE write operations.
    pub write_timeout: Duration,
    /// Timeout for service discovery after connection.
    ///
    /// On Linux, when BlueZ is still discovering the services of a device it
    /// has just connected (a first connection to a sensor with many
    /// characteristics, such as an Aranet4), a connect waits for BlueZ to
    /// finish for up to this long, but at least 20 s.
    pub discovery_timeout: Duration,
    /// Timeout for connection validation (keepalive) checks.
    pub validation_timeout: Duration,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            connection_timeout: DEFAULT_CONNECT_TIMEOUT,
            read_timeout: DEFAULT_READ_TIMEOUT,
            write_timeout: DEFAULT_WRITE_TIMEOUT,
            discovery_timeout: DEFAULT_DISCOVERY_TIMEOUT,
            validation_timeout: DEFAULT_VALIDATION_TIMEOUT,
        }
    }
}

impl ConnectionConfig {
    /// Create a new connection config with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a config optimized for the current platform.
    pub fn for_current_platform() -> Self {
        let platform = crate::platform::PlatformConfig::for_current_platform();
        Self {
            connection_timeout: platform.recommended_connection_timeout,
            read_timeout: platform.recommended_operation_timeout,
            write_timeout: platform.recommended_operation_timeout,
            discovery_timeout: platform.recommended_operation_timeout,
            validation_timeout: DEFAULT_VALIDATION_TIMEOUT,
        }
    }

    /// Create a config for challenging RF environments.
    ///
    /// Uses longer timeouts to accommodate signal interference,
    /// thick walls, or long distances.
    ///
    /// These are the connection's timeouts only: [`Device::connect_with_config`]
    /// still searches with scans of 5, 10 and 15 s (up to about 30 s).
    /// To search longer for a weak sensor, use [`Device::connect_with_scan_options`]
    /// with a longer `ScanOptions::duration`.
    pub fn challenging_environment() -> Self {
        Self {
            connection_timeout: Duration::from_secs(90),
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(15),
            discovery_timeout: Duration::from_secs(30),
            validation_timeout: Duration::from_secs(5),
        }
    }

    /// Create a config for fast, reliable environments.
    ///
    /// Uses shorter timeouts for quicker failure detection
    /// when devices are nearby with strong signals.
    pub fn fast() -> Self {
        Self {
            connection_timeout: Duration::from_secs(8),
            read_timeout: Duration::from_secs(5),
            write_timeout: Duration::from_secs(5),
            discovery_timeout: Duration::from_secs(5),
            validation_timeout: Duration::from_secs(2),
        }
    }

    /// Set the connection timeout.
    #[must_use]
    pub fn connection_timeout(mut self, timeout: Duration) -> Self {
        self.connection_timeout = timeout;
        self
    }

    /// Set the read timeout.
    #[must_use]
    pub fn read_timeout(mut self, timeout: Duration) -> Self {
        self.read_timeout = timeout;
        self
    }

    /// Set the write timeout.
    #[must_use]
    pub fn write_timeout(mut self, timeout: Duration) -> Self {
        self.write_timeout = timeout;
        self
    }

    /// Set the service discovery timeout.
    #[must_use]
    pub fn discovery_timeout(mut self, timeout: Duration) -> Self {
        self.discovery_timeout = timeout;
        self
    }

    /// Set the validation timeout.
    #[must_use]
    pub fn validation_timeout(mut self, timeout: Duration) -> Self {
        self.validation_timeout = timeout;
        self
    }
}

/// Signal strength quality levels based on RSSI values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SignalQuality {
    /// Signal too weak for reliable operation (< -85 dBm).
    Poor,
    /// Usable but may have issues (-85 to -75 dBm).
    Fair,
    /// Good signal strength (-75 to -60 dBm).
    Good,
    /// Excellent signal strength (> -60 dBm).
    Excellent,
}

impl SignalQuality {
    /// Determine signal quality from RSSI value in dBm.
    ///
    /// # Arguments
    ///
    /// * `rssi` - Signal strength in dBm (typically -30 to -100)
    ///
    /// # Returns
    ///
    /// The signal quality category.
    pub fn from_rssi(rssi: i16) -> Self {
        match rssi {
            r if r > -60 => SignalQuality::Excellent,
            r if r > -75 => SignalQuality::Good,
            r if r > -85 => SignalQuality::Fair,
            _ => SignalQuality::Poor,
        }
    }

    /// Get a human-readable description of the signal quality.
    pub fn description(&self) -> &'static str {
        match self {
            SignalQuality::Excellent => "Excellent signal",
            SignalQuality::Good => "Good signal",
            SignalQuality::Fair => "Fair signal - connection may be unstable",
            SignalQuality::Poor => "Poor signal - consider moving closer",
        }
    }

    /// Get recommended read delay for history downloads based on signal quality.
    pub fn recommended_read_delay(&self) -> Duration {
        match self {
            SignalQuality::Excellent => Duration::from_millis(30),
            SignalQuality::Good => Duration::from_millis(50),
            SignalQuality::Fair => Duration::from_millis(100),
            SignalQuality::Poor => Duration::from_millis(200),
        }
    }

    /// Check if the signal is strong enough for reliable operations.
    pub fn is_usable(&self) -> bool {
        matches!(
            self,
            SignalQuality::Excellent | SignalQuality::Good | SignalQuality::Fair
        )
    }
}

impl Device {
    /// Connect to an Aranet device by name or MAC address.
    ///
    /// The device is searched for with scans of 5, 10 and 15 s (up to about 30 s) and
    /// connected with the default [`ConnectionConfig`]. See the "Timeouts" section of
    /// [`Device`] for how long that can take.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use aranet_core::device::Device;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let device = Device::connect("Aranet4 12345").await?;
    ///     println!("Connected to {:?}", device);
    ///     Ok(())
    /// }
    /// ```
    #[tracing::instrument(level = "info", skip_all, fields(identifier = %identifier))]
    pub async fn connect(identifier: &str) -> Result<Self> {
        Self::connect_with_config(identifier, ConnectionConfig::default()).await
    }

    /// Connect with a custom connection timeout.
    ///
    /// The device search uses scans of 5, 10 and 15 s (up to about 30 s).
    /// `timeout` replaces only the connect timeout (`connection_timeout`); every other
    /// timeout keeps its default. Use [`Device::connect_with_scan_options`] to change
    /// the scan time as well.
    #[tracing::instrument(level = "info", skip_all, fields(identifier = %identifier, timeout_secs = timeout.as_secs()))]
    pub async fn connect_with_timeout(identifier: &str, timeout: Duration) -> Result<Self> {
        Self::connect_with_scan_options(
            identifier,
            ScanOptions::default().duration(CONNECT_SCAN_DURATION),
            ConnectionConfig::default().connection_timeout(timeout),
        )
        .await
    }

    /// Connect to an Aranet device with full configuration.
    ///
    /// `config` sets every timeout of the connection itself. The device is searched for
    /// with scans of 5, 10 and 15 s (up to about 30 s); use
    /// [`Device::connect_with_scan_options`] to change the scan time too.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    /// use aranet_core::device::{Device, ConnectionConfig};
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     // Longer connect timeouts for a challenging RF environment. They don't
    ///     // lengthen the search for the device.
    ///     let config = ConnectionConfig::challenging_environment();
    ///     let device = Device::connect_with_config("Aranet4 12345", config).await?;
    ///     Ok(())
    /// }
    /// ```
    #[tracing::instrument(level = "info", skip_all, fields(identifier = %identifier))]
    pub async fn connect_with_config(identifier: &str, config: ConnectionConfig) -> Result<Self> {
        Self::connect_with_scan_options(
            identifier,
            ScanOptions::default().duration(CONNECT_SCAN_DURATION),
            config,
        )
        .await
    }

    /// Find `identifier` with `scan`, then connect with `config`.
    ///
    /// The search makes up to three scans of `scan.duration / 2`, `scan.duration` and
    /// `1.5 × scan.duration` (at least 2, 4 and 6 s), so it takes up to about
    /// 3 × `scan.duration`, plus any wait for another scan in the same process.
    /// Only `scan.duration` is used: as in
    /// [`find_device_with_options`](crate::scan::find_device_with_options), the search
    /// ignores `scan`'s filter flags and keeps its default filter.
    /// A device that the adapter already knows from an earlier scan is used without
    /// scanning. `config` sets the timeouts of the connection itself; see the
    /// "Timeouts" section of [`Device`].
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    /// use aranet_core::device::{ConnectionConfig, Device};
    /// use aranet_core::scan::ScanOptions;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     // Scan for 5, 10 and 15 s (up to about 30 s), then allow 30 s to connect.
    ///     let scan = ScanOptions::default().duration(Duration::from_secs(10));
    ///     let config = ConnectionConfig::default().connection_timeout(Duration::from_secs(30));
    ///     let device = Device::connect_with_scan_options("Aranet4 12345", scan, config).await?;
    ///     device.disconnect().await?;
    ///     Ok(())
    /// }
    /// ```
    #[tracing::instrument(level = "info", skip_all, fields(identifier = %identifier, scan_secs = scan.duration.as_secs()))]
    pub async fn connect_with_scan_options(
        identifier: &str,
        scan: ScanOptions,
        config: ConnectionConfig,
    ) -> Result<Self> {
        let (adapter, peripheral) = crate::scan::find_device_with_options(identifier, scan).await?;
        Self::from_peripheral_with_config(adapter, peripheral, config).await
    }

    /// Connect to a device using an existing BLE adapter.
    ///
    /// This avoids creating a new btleplug `Manager` (and D-Bus connection) on
    /// every call.  Prefer this over [`connect_with_config`](Self::connect_with_config)
    /// in long-running services that poll devices repeatedly.
    ///
    /// Like [`connect_with_config`](Self::connect_with_config), it searches once with
    /// scans of 5, 10 and 15 s (up to about 30 s), and `config` sets the timeouts of
    /// the connection itself.
    #[tracing::instrument(level = "info", skip_all, fields(identifier = %identifier))]
    pub async fn connect_with_adapter(
        adapter: Adapter,
        identifier: &str,
        config: ConnectionConfig,
    ) -> Result<Self> {
        let peripheral = crate::scan::find_device_with_adapter(
            &adapter,
            identifier,
            ScanOptions::default().duration(CONNECT_SCAN_DURATION),
        )
        .await?;
        Self::from_peripheral_with_config(adapter, peripheral, config).await
    }

    /// Create a Device from an already-discovered peripheral.
    #[tracing::instrument(level = "info", skip_all)]
    pub async fn from_peripheral(adapter: Adapter, peripheral: Peripheral) -> Result<Self> {
        Self::from_peripheral_with_config(adapter, peripheral, ConnectionConfig::default()).await
    }

    /// Create a Device from an already-discovered peripheral with custom timeout.
    #[tracing::instrument(level = "info", skip_all, fields(timeout_secs = connect_timeout.as_secs()))]
    pub async fn from_peripheral_with_timeout(
        adapter: Adapter,
        peripheral: Peripheral,
        connect_timeout: Duration,
    ) -> Result<Self> {
        let config = ConnectionConfig::default().connection_timeout(connect_timeout);
        Self::from_peripheral_with_config(adapter, peripheral, config).await
    }

    /// Create a Device from an already-discovered peripheral with full configuration.
    ///
    /// If the connect fails or times out, the peripheral is disconnected before
    /// the error is returned. If this future is dropped before it finishes (a
    /// caller's timeout, a cancelled task), the peripheral is disconnected in
    /// the background; that is best effort if the process is exiting. Either
    /// way the disconnect waits at most 5 s for the Bluetooth stack to confirm
    /// it.
    ///
    /// On Linux, if BlueZ doesn't list the sensor as paired, this pairs it first
    /// (BlueZ's `Device1.Pair`, through an agent that exists only for that
    /// pairing and approves only this sensor), which takes at most
    /// `connection_timeout` plus the wait for BlueZ's service discovery
    /// (`discovery_timeout`, but at least 20 s), plus 5 s. If pairing fails, a
    /// warning with the `bluetoothctl` commands that fix it is logged and the
    /// connection goes ahead unpaired. BlueZ then asks for pairing itself, so on
    /// a host without a Bluetooth agent this connection's reads can time out. A
    /// sensor that refused to pair isn't asked again for 10 minutes; after any
    /// other failure, the next connection pairs again.
    #[tracing::instrument(level = "info", skip_all, fields(connect_timeout = ?config.connection_timeout))]
    pub async fn from_peripheral_with_config(
        adapter: Adapter,
        peripheral: Peripheral,
        config: ConnectionConfig,
    ) -> Result<Self> {
        let cleanup = crate::link::cleanup_runtime().ok_or_else(|| {
            Error::Io(std::io::Error::other(
                "no tokio runtime available for Bluetooth cleanup",
            ))
        })?;

        let crate::link::OpenLink {
            pending,
            services,
            properties,
        } = crate::link::connect(&peripheral, &config, &cleanup).await?;

        // Build characteristics cache for O(1) lookups
        let mut characteristics_cache = HashMap::new();
        for service in &services {
            debug!("  Service: {}", service.uuid);
            for char in &service.characteristics {
                debug!("    Characteristic: {}", char.uuid);
                characteristics_cache.insert(char.uuid, char.clone());
            }
        }
        debug!(
            "Cached {} characteristics for fast lookup",
            characteristics_cache.len()
        );

        let name = properties.as_ref().and_then(|p| p.local_name.clone());

        // Get address - on macOS this may be 00:00:00:00:00:00, so we use peripheral ID as fallback
        let address = properties
            .as_ref()
            .map(|p| create_identifier(&p.address.to_string(), &peripheral.id()))
            .unwrap_or_else(|| format_peripheral_id(&peripheral.id()));

        // Determine device type from name
        let device_type = name.as_ref().and_then(|n| DeviceType::from_name(n));

        let device = Self {
            adapter,
            peripheral,
            name,
            address,
            device_type,
            services_discovered: true,
            characteristics_cache: RwLock::new(characteristics_cache),
            notification_tasks: crate::link::NotificationTasks::default(),
            disconnected: AtomicBool::new(false),
            config,
        };
        // Nothing between `connect` returning and here awaits, so the link is
        // never left without an owner.
        pending.disarm();
        Ok(device)
    }

    /// Check if the device is connected (queries BLE stack state).
    ///
    /// Returns `false` if the Bluetooth stack reports an error or doesn't answer
    /// within the connection's `validation_timeout` (3 s by default, set with
    /// [`ConnectionConfig::validation_timeout`]). On macOS the stack stops
    /// answering once the sensor has dropped the connection.
    ///
    /// Note: This only checks the BLE stack's connection state, which may be stale,
    /// especially on macOS. For a more reliable check, use [`Self::validate_connection`].
    pub async fn is_connected(&self) -> bool {
        crate::link::is_connected(&self.peripheral, self.config.validation_timeout).await
    }

    /// Validate the connection by reading the current measurements.
    ///
    /// This reads the characteristic that [`Self::read_current`] uses, so the
    /// check needs no pairing that a reading doesn't need (unpaired Aranet2 and
    /// AranetRn+ sensors answer it) and never starts a pairing that a reading
    /// wouldn't. It fails only if the read fails or takes longer than the
    /// connection's `validation_timeout` (3 s by default, set with
    /// [`ConnectionConfig::validation_timeout`]).
    ///
    /// This is more reliable than `is_connected()` as it actively verifies
    /// the connection is working. It detects "zombie connections", where the
    /// BLE stack thinks it's connected but the device is actually out of range.
    ///
    /// # Returns
    ///
    /// `true` if the connection is active and responsive, `false` otherwise.
    pub async fn validate_connection(&self) -> bool {
        matches!(
            timeout(self.config.validation_timeout, self.read_current_bytes()).await,
            Ok(Ok(_))
        )
    }

    /// Check if the connection is alive by reading the current measurements.
    ///
    /// This is an alias for [`Self::validate_connection`] that better describes
    /// the intent when used for connection health monitoring. Like it, it needs
    /// no pairing that a reading doesn't need, and fails only if the read fails
    /// or takes longer than the connection's `validation_timeout`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // In a health monitor loop
    /// if !device.is_connection_alive().await {
    ///     // Connection lost, need to reconnect
    /// }
    /// ```
    pub async fn is_connection_alive(&self) -> bool {
        self.validate_connection().await
    }

    /// Get the current connection configuration.
    pub fn config(&self) -> &ConnectionConfig {
        &self.config
    }

    /// Get the current signal quality based on RSSI.
    ///
    /// Returns `None` if RSSI cannot be read.
    pub async fn signal_quality(&self) -> Option<SignalQuality> {
        self.read_rssi().await.ok().map(SignalQuality::from_rssi)
    }

    /// Disconnect from the device.
    ///
    /// This will:
    /// 1. Abort all active notification handlers
    /// 2. Disconnect from the BLE peripheral
    ///
    /// Returns [`Error::Timeout`] if the Bluetooth stack doesn't confirm the
    /// disconnect within 5 s. The disconnect runs as a background task (on
    /// aranet-core's runtime), so it completes even if this future is dropped,
    /// for example by a caller's timeout.
    ///
    /// **Important:** You MUST call this method before dropping the Device
    /// to ensure proper cleanup of BLE resources.
    #[tracing::instrument(level = "info", skip(self), fields(device_name = ?self.name))]
    pub async fn disconnect(&self) -> Result<()> {
        info!("Disconnecting from device...");
        self.disconnected.store(true, Ordering::SeqCst);

        // Stop every notification callback.
        self.notification_tasks.abort_all();

        // `cleanup_runtime` is `None` only if aranet-core's runtime can't be
        // started and this future isn't polled on a tokio runtime either.
        // The time limit needs a tokio timer, so give up instead of panicking.
        let runtime = crate::link::cleanup_runtime().ok_or_else(|| {
            Error::Io(std::io::Error::other(
                "no tokio runtime available to disconnect from the device",
            ))
        })?;
        crate::link::disconnect_detached(&self.peripheral, &runtime).await
    }

    /// Get the device name.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Get the device address or identifier.
    ///
    /// On Linux and Windows, this returns the Bluetooth MAC address (e.g., "AA:BB:CC:DD:EE:FF").
    /// On macOS, this returns a UUID identifier since MAC addresses are not exposed.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Get the detected device type.
    pub fn device_type(&self) -> Option<DeviceType> {
        self.device_type
    }

    /// Read the current RSSI (signal strength) of the connection.
    ///
    /// Returns the RSSI in dBm. More negative values indicate weaker signals.
    /// Typical values range from -30 (strong) to -90 (weak).
    ///
    /// Returns [`Error::Timeout`] if the Bluetooth stack doesn't answer within
    /// the connection's `read_timeout` (10 s by default, set with
    /// [`ConnectionConfig::read_timeout`]).
    pub async fn read_rssi(&self) -> Result<i16> {
        let properties = crate::link::bounded(
            "read device properties",
            self.config.read_timeout,
            self.peripheral.properties(),
        )
        .await?;
        properties
            .and_then(|p| p.rssi)
            .ok_or_else(|| Error::InvalidData("RSSI not available".to_string()))
    }

    /// Find a characteristic by UUID using the cached lookup table.
    ///
    /// Uses O(1) lookup from the characteristics cache built during service discovery.
    /// Falls back to searching through services if the cache is empty (shouldn't happen
    /// normally, but provides robustness).
    async fn find_characteristic(&self, uuid: Uuid) -> Result<Characteristic> {
        // Try cache first (O(1) lookup)
        {
            let cache = self.characteristics_cache.read().await;
            if let Some(char) = cache.get(&uuid) {
                return Ok(char.clone());
            }

            // If cache is populated but characteristic not found, it doesn't exist
            if !cache.is_empty() {
                return Err(Error::characteristic_not_found(
                    uuid.to_string(),
                    self.peripheral.services().len(),
                ));
            }
        }

        // Fallback: search services directly (shouldn't happen in normal operation)
        warn!(
            "Characteristics cache empty, falling back to service search for {}",
            uuid
        );
        let services = self.peripheral.services();
        let service_count = services.len();

        // First try Aranet-specific services
        for service in &services {
            if service.uuid == SAF_TEHNIKA_SERVICE_NEW || service.uuid == SAF_TEHNIKA_SERVICE_OLD {
                for char in &service.characteristics {
                    if char.uuid == uuid {
                        return Ok(char.clone());
                    }
                }
            }
        }

        // Then try standard services (GAP, Device Info, Battery)
        for service in &services {
            if service.uuid == GAP_SERVICE
                || service.uuid == DEVICE_INFO_SERVICE
                || service.uuid == BATTERY_SERVICE
            {
                for char in &service.characteristics {
                    if char.uuid == uuid {
                        return Ok(char.clone());
                    }
                }
            }
        }

        // Finally search all services
        for service in &services {
            for char in &service.characteristics {
                if char.uuid == uuid {
                    return Ok(char.clone());
                }
            }
        }

        Err(Error::characteristic_not_found(
            uuid.to_string(),
            service_count,
        ))
    }

    /// Read a characteristic value by UUID.
    ///
    /// This method includes a timeout to prevent indefinite hangs on BLE operations.
    /// The timeout is controlled by [`ConnectionConfig::read_timeout`].
    pub async fn read_characteristic(&self, uuid: Uuid) -> Result<Vec<u8>> {
        let characteristic = self.find_characteristic(uuid).await?;
        let data = timeout(
            self.config.read_timeout,
            self.peripheral.read(&characteristic),
        )
        .await
        .map_err(|_| Error::Timeout {
            operation: format!("read characteristic {}", uuid),
            duration: self.config.read_timeout,
        })??;
        Ok(data)
    }

    /// Read a characteristic value with a custom timeout.
    ///
    /// Use this when you need a different timeout than the default,
    /// for example when reading large data.
    pub async fn read_characteristic_with_timeout(
        &self,
        uuid: Uuid,
        read_timeout: Duration,
    ) -> Result<Vec<u8>> {
        let characteristic = self.find_characteristic(uuid).await?;
        let data = timeout(read_timeout, self.peripheral.read(&characteristic))
            .await
            .map_err(|_| Error::Timeout {
                operation: format!("read characteristic {}", uuid),
                duration: read_timeout,
            })??;
        Ok(data)
    }

    /// Write a value to a characteristic.
    ///
    /// This method includes a timeout to prevent indefinite hangs on BLE operations.
    /// The timeout is controlled by [`ConnectionConfig::write_timeout`].
    pub async fn write_characteristic(&self, uuid: Uuid, data: &[u8]) -> Result<()> {
        let characteristic = self.find_characteristic(uuid).await?;
        timeout(
            self.config.write_timeout,
            self.peripheral
                .write(&characteristic, data, WriteType::WithResponse),
        )
        .await
        .map_err(|_| Error::Timeout {
            operation: format!("write characteristic {}", uuid),
            duration: self.config.write_timeout,
        })??;
        Ok(())
    }

    /// Write a value to a characteristic with a custom timeout.
    pub async fn write_characteristic_with_timeout(
        &self,
        uuid: Uuid,
        data: &[u8],
        write_timeout: Duration,
    ) -> Result<()> {
        let characteristic = self.find_characteristic(uuid).await?;
        timeout(
            write_timeout,
            self.peripheral
                .write(&characteristic, data, WriteType::WithResponse),
        )
        .await
        .map_err(|_| Error::Timeout {
            operation: format!("write characteristic {}", uuid),
            duration: write_timeout,
        })??;
        Ok(())
    }

    /// Raw current-readings bytes; on CharacteristicNotFound tries the next candidate.
    ///
    /// Reads the characteristics from `readings_characteristics` in order, as
    /// `read_first_available` describes.
    async fn read_current_bytes(&self) -> Result<Vec<u8>> {
        read_first_available(readings_characteristics(self.device_type), |uuid| {
            self.read_characteristic(uuid)
        })
        .await
    }

    /// Read current sensor measurements.
    ///
    /// Automatically selects the correct characteristic UUID based on device type:
    /// - Aranet4 uses `f0cd3001`
    /// - Aranet2, Radon, Radiation use `f0cd3003`
    /// - an unknown type tries `f0cd3001`, then `f0cd3003` if the device doesn't have it
    #[tracing::instrument(level = "debug", skip(self), fields(device_name = ?self.name, device_type = ?self.device_type))]
    pub async fn read_current(&self) -> Result<CurrentReading> {
        let data = self.read_current_bytes().await?;

        // Parse based on device type.
        let device_type = match self.device_type {
            Some(dt) => dt,
            None => {
                warn!(
                    "Device type unknown for {}; defaulting to Aranet4 — \
                     readings may be incorrect if this is a different model",
                    self.name().unwrap_or("unknown")
                );
                DeviceType::Aranet4
            }
        };
        crate::readings::parse_reading_for_device(&data, device_type)
    }

    /// Read the battery level (0-100).
    #[tracing::instrument(level = "debug", skip(self))]
    pub async fn read_battery(&self) -> Result<u8> {
        let data = self.read_characteristic(BATTERY_LEVEL).await?;
        if data.is_empty() {
            return Err(Error::InvalidData("Empty battery data".to_string()));
        }
        Ok(data[0])
    }

    /// Read device information.
    ///
    /// This method reads all device info characteristics in parallel for better performance.
    #[tracing::instrument(level = "debug", skip(self))]
    pub async fn read_device_info(&self) -> Result<DeviceInfo> {
        fn read_string(data: Vec<u8>) -> String {
            String::from_utf8(data)
                .unwrap_or_default()
                .trim_end_matches('\0')
                .to_string()
        }

        // Read all characteristics in parallel for better performance
        let (
            name_result,
            model_result,
            serial_result,
            firmware_result,
            hardware_result,
            software_result,
            manufacturer_result,
        ) = tokio::join!(
            self.read_characteristic(DEVICE_NAME),
            self.read_characteristic(MODEL_NUMBER),
            self.read_characteristic(SERIAL_NUMBER),
            self.read_characteristic(FIRMWARE_REVISION),
            self.read_characteristic(HARDWARE_REVISION),
            self.read_characteristic(SOFTWARE_REVISION),
            self.read_characteristic(MANUFACTURER_NAME),
        );

        let name = name_result
            .map(read_string)
            .unwrap_or_else(|_| self.name.clone().unwrap_or_default());

        let model = model_result.map(read_string).unwrap_or_default();
        let serial = serial_result.map(read_string).unwrap_or_default();
        let firmware = firmware_result.map(read_string).unwrap_or_default();
        let hardware = hardware_result.map(read_string).unwrap_or_default();
        let software = software_result.map(read_string).unwrap_or_default();
        let manufacturer = manufacturer_result.map(read_string).unwrap_or_default();

        Ok(DeviceInfo {
            name,
            model,
            serial,
            firmware,
            hardware,
            software,
            manufacturer,
        })
    }

    /// Read essential device information only.
    ///
    /// This is a faster alternative to [`Self::read_device_info`] that only reads
    /// the most critical characteristics: name, serial number, and firmware version.
    /// Use this for faster startup when full device info isn't needed immediately.
    #[tracing::instrument(level = "debug", skip(self))]
    pub async fn read_device_info_essential(&self) -> Result<DeviceInfo> {
        fn read_string(data: Vec<u8>) -> String {
            String::from_utf8(data)
                .unwrap_or_default()
                .trim_end_matches('\0')
                .to_string()
        }

        // Only read the essential characteristics in parallel
        let (name_result, serial_result, firmware_result) = tokio::join!(
            self.read_characteristic(DEVICE_NAME),
            self.read_characteristic(SERIAL_NUMBER),
            self.read_characteristic(FIRMWARE_REVISION),
        );

        let name = name_result
            .map(read_string)
            .unwrap_or_else(|_| self.name.clone().unwrap_or_default());
        let serial = serial_result.map(read_string).unwrap_or_default();
        let firmware = firmware_result.map(read_string).unwrap_or_default();

        Ok(DeviceInfo {
            name,
            model: String::new(),
            serial,
            firmware,
            hardware: String::new(),
            software: String::new(),
            manufacturer: String::new(),
        })
    }

    /// Subscribe to notifications on a characteristic.
    ///
    /// The callback is called on a background task with the value of each
    /// notification the characteristic sends, until
    /// [`unsubscribe_from_notifications`](Self::unsubscribe_from_notifications)
    /// or [`disconnect`](Self::disconnect) is called or the device is dropped.
    ///
    /// Subscribing to the same characteristic again replaces its callback. If
    /// subscribing fails, the previous callback keeps running.
    ///
    /// Opening the notification stream and enabling notifications on the
    /// device each give up with [`Error::Timeout`] after the connection's
    /// `write_timeout` (set with [`ConnectionConfig::write_timeout`]).
    pub async fn subscribe_to_notifications<F>(&self, uuid: Uuid, callback: F) -> Result<()>
    where
        F: Fn(&[u8]) + Send + Sync + 'static,
    {
        let characteristic = self.find_characteristic(uuid).await?;
        self.notification_tasks
            .subscribe(
                &self.peripheral,
                &characteristic,
                self.config.write_timeout,
                callback,
            )
            .await
    }

    /// Unsubscribe from notifications on a characteristic.
    ///
    /// Stops the characteristic's callback first, then tells the device to
    /// stop sending notifications. Once this returns, the callback is not
    /// called again, even if the device call fails.
    ///
    /// Waiting for the callback to stop and the device call each give up
    /// after the connection's `write_timeout` (set with
    /// [`ConnectionConfig::write_timeout`]); the device call then fails with
    /// [`Error::Timeout`].
    pub async fn unsubscribe_from_notifications(&self, uuid: Uuid) -> Result<()> {
        let characteristic = self.find_characteristic(uuid).await?;
        self.notification_tasks
            .unsubscribe(&self.peripheral, &characteristic, self.config.write_timeout)
            .await
    }

    /// Get the number of cached characteristics.
    ///
    /// This is useful for debugging and testing to verify service discovery worked.
    pub async fn cached_characteristic_count(&self) -> usize {
        self.characteristics_cache.read().await.len()
    }
}

// NOTE: Drop performs best-effort cleanup if disconnect() was not called.
// The disconnect is spawned on aranet-core's own runtime (`aranet-ble`) when it
// can be started, so it runs even if the caller's runtime is shutting down, and
// it gives up after 5 s if the Bluetooth stack never confirms. It can still be
// cut short when the process exits. For reliable cleanup, callers SHOULD
// explicitly call `device.disconnect().await` before dropping the Device.
//
// The cleanup behavior:
// 1. Aborts all notification handlers (sync operation)
// 2. Spawns a time-limited disconnect of the peripheral on aranet-core's runtime (best-effort)
// 3. Logs a warning about the implicit cleanup
//
// For automatic cleanup, consider using `ReconnectingDevice` which manages the lifecycle.

impl Drop for Device {
    fn drop(&mut self) {
        if !self.disconnected.load(Ordering::SeqCst) {
            // Mark as disconnected to prevent double-cleanup
            self.disconnected.store(true, Ordering::SeqCst);

            // Log warning about implicit cleanup
            warn!(
                device_name = ?self.name,
                device_address = %self.address,
                "Device dropped without calling disconnect() - performing best-effort cleanup. \
                 For reliable cleanup, call device.disconnect().await before dropping."
            );

            // Abort every notification task. The task map's lock is never
            // held across an await, so unlike the old `try_lock` this is
            // never skipped.
            self.notification_tasks.abort_all();

            // Spawn a best-effort, time-limited disconnect, on aranet-core's own
            // runtime when it can be started, so it outlives the caller's runtime
            let peripheral = self.peripheral.clone();
            let address = self.address.clone();

            if let Some(runtime) = crate::link::cleanup_runtime() {
                runtime.spawn(async move {
                    match crate::link::disconnect(&peripheral).await {
                        Ok(()) => debug!(
                            device_address = %address,
                            "Best-effort disconnect completed"
                        ),
                        Err(e) => debug!(
                            device_address = %address,
                            error = %e,
                            "Best-effort disconnect failed (device may already be disconnected)"
                        ),
                    }
                });
            }
        }
    }
}

impl AranetDevice for Device {
    // --- Connection Management ---

    async fn is_connected(&self) -> bool {
        Device::is_connected(self).await
    }

    async fn disconnect(&self) -> Result<()> {
        Device::disconnect(self).await
    }

    // --- Device Identity ---

    fn name(&self) -> Option<&str> {
        Device::name(self)
    }

    fn address(&self) -> &str {
        Device::address(self)
    }

    fn device_type(&self) -> Option<DeviceType> {
        Device::device_type(self)
    }

    // --- Current Readings ---

    async fn read_current(&self) -> Result<CurrentReading> {
        Device::read_current(self).await
    }

    async fn read_device_info(&self) -> Result<DeviceInfo> {
        Device::read_device_info(self).await
    }

    async fn read_rssi(&self) -> Result<i16> {
        Device::read_rssi(self).await
    }

    // --- Battery ---

    async fn read_battery(&self) -> Result<u8> {
        Device::read_battery(self).await
    }

    // --- History ---

    async fn get_history_info(&self) -> Result<crate::history::HistoryInfo> {
        Device::get_history_info(self).await
    }

    async fn download_history(&self) -> Result<Vec<aranet_types::HistoryRecord>> {
        Device::download_history(self).await
    }

    async fn download_history_with_options(
        &self,
        options: crate::history::HistoryOptions,
    ) -> Result<Vec<aranet_types::HistoryRecord>> {
        Device::download_history_with_options(self, options).await
    }

    // --- Settings ---

    async fn get_interval(&self) -> Result<crate::settings::MeasurementInterval> {
        Device::get_interval(self).await
    }

    async fn set_interval(&self, interval: crate::settings::MeasurementInterval) -> Result<()> {
        Device::set_interval(self, interval).await
    }

    async fn get_calibration(&self) -> Result<crate::settings::CalibrationData> {
        Device::get_calibration(self).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use futures::FutureExt;

    #[test]
    fn readings_characteristics_follow_the_device_type() {
        assert_eq!(
            readings_characteristics(Some(DeviceType::Aranet4)),
            &[CURRENT_READINGS_DETAIL]
        );
        for device_type in [
            DeviceType::Aranet2,
            DeviceType::AranetRadon,
            DeviceType::AranetRadiation,
        ] {
            assert_eq!(
                readings_characteristics(Some(device_type)),
                &[CURRENT_READINGS_DETAIL_ALT],
                "{device_type:?}"
            );
        }
        // Unknown type: the Aranet4 characteristic first, as `read_current` always did.
        assert_eq!(
            readings_characteristics(None),
            &[CURRENT_READINGS_DETAIL, CURRENT_READINGS_DETAIL_ALT]
        );

        // Battery Level (0x2A19) needs pairing, so a connection check must never read it.
        for device_type in [
            None,
            Some(DeviceType::Aranet4),
            Some(DeviceType::Aranet2),
            Some(DeviceType::AranetRadon),
            Some(DeviceType::AranetRadiation),
        ] {
            assert!(
                !readings_characteristics(device_type).contains(&BATTERY_LEVEL),
                "{device_type:?} reads Battery Level"
            );
        }
    }

    /// Runs `read_first_available` over `candidates` with a reader that gives
    /// the scripted `answers` in order. Returns the result and the
    /// characteristics the reader was asked for.
    fn read_scripted(
        candidates: &[Uuid],
        answers: Vec<Result<Vec<u8>>>,
    ) -> (Result<Vec<u8>>, Vec<Uuid>) {
        let mut answers = answers.into_iter();
        let mut asked = Vec::new();
        let result = read_first_available(candidates, |uuid| {
            asked.push(uuid);
            std::future::ready(answers.next().expect("read more often than scripted"))
        })
        .now_or_never()
        .expect("scripted reads finish at once");
        (result, asked)
    }

    fn not_found(uuid: Uuid) -> Error {
        Error::characteristic_not_found(uuid.to_string(), 6)
    }

    #[test]
    fn a_missing_characteristic_moves_on_to_the_next_candidate() {
        let both = [CURRENT_READINGS_DETAIL, CURRENT_READINGS_DETAIL_ALT];

        let (result, asked) = read_scripted(&both, vec![Ok(vec![1])]);
        assert_eq!(result.unwrap(), [1]);
        assert_eq!(asked, [CURRENT_READINGS_DETAIL]);

        let (result, asked) = read_scripted(
            &both,
            vec![Err(not_found(CURRENT_READINGS_DETAIL)), Ok(vec![2])],
        );
        assert_eq!(result.unwrap(), [2]);
        assert_eq!(asked, both);
    }

    #[test]
    fn any_other_error_ends_the_search() {
        let (result, asked) = read_scripted(
            &[CURRENT_READINGS_DETAIL, CURRENT_READINGS_DETAIL_ALT],
            vec![Err(Error::timeout(
                "read characteristic f0cd3001",
                Duration::from_secs(10),
            ))],
        );
        assert!(matches!(result, Err(Error::Timeout { .. })), "{result:?}");
        assert_eq!(asked, [CURRENT_READINGS_DETAIL]);
    }

    #[test]
    fn the_readings_read_never_asks_for_battery_level() {
        for device_type in [
            None,
            Some(DeviceType::Aranet4),
            Some(DeviceType::Aranet2),
            Some(DeviceType::AranetRadon),
            Some(DeviceType::AranetRadiation),
        ] {
            let candidates = readings_characteristics(device_type);
            let answers = candidates
                .iter()
                .map(|&uuid| Err(not_found(uuid)))
                .collect();
            let (result, asked) = read_scripted(candidates, answers);
            assert!(
                matches!(result, Err(Error::CharacteristicNotFound { .. })),
                "{device_type:?}: {result:?}"
            );
            assert_eq!(asked, candidates, "{device_type:?}");
            assert!(!asked.contains(&BATTERY_LEVEL), "{device_type:?}");
        }
    }
}
