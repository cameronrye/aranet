//! The BLE connect sequence and notification subscriptions, behind a
//! crate-private seam.
//!
//! `GattLink` is the part of a btleplug peripheral that connecting and
//! notifications use. `Device` calls `connect` and the `NotificationTasks`
//! methods with btleplug's platform `Peripheral`, and the unit tests call
//! them with a scripted fake (`fake::FakeGatt`), so the connect,
//! notification, timeout and cleanup paths can be tested without Bluetooth.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use btleplug::api::{Characteristic, PeripheralProperties, Service};
use futures::StreamExt;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::device::ConnectionConfig;
use crate::error::{Error, Result};

#[cfg(test)]
mod fake;

/// The operations on a BLE peripheral that connecting and notifications use.
///
/// btleplug's platform `Peripheral` implements it for real connections, and
/// `fake::FakeGatt` implements it for tests.
///
/// `device.rs` must never import this trait. It imports
/// `btleplug::api::Peripheral`, whose methods have the same names, so every
/// `peripheral.connect()` there would become ambiguous (E0034). `device.rs`
/// calls the free functions of this module instead.
pub(crate) trait GattLink: Clone + Send + Sync + 'static {
    /// Open the connection.
    fn connect(&self) -> impl Future<Output = btleplug::Result<()>> + Send;

    /// Close the connection.
    fn disconnect(&self) -> impl Future<Output = btleplug::Result<()>> + Send;

    /// Discover the GATT services and their characteristics.
    fn discover_services(&self) -> impl Future<Output = btleplug::Result<()>> + Send;

    /// The services found by the last discovery (empty before the first one).
    fn services(&self) -> BTreeSet<Service>;

    /// What the peripheral advertised: its name, address and so on.
    fn properties(
        &self,
    ) -> impl Future<Output = btleplug::Result<Option<PeripheralProperties>>> + Send;

    /// Whether the Bluetooth stack reports the link as up. On macOS nobody
    /// answers this once CoreBluetooth has dropped the peripheral, so callers
    /// use this module's time-limited `is_connected` function instead.
    fn is_connected(&self) -> impl Future<Output = btleplug::Result<bool>> + Send;

    /// Whether the connection goes through BlueZ. There `connect` doesn't
    /// return when the link comes up: BlueZ answers `Device1.Connect` only
    /// once the ATT channel is up, a few seconds later, and bluez-async then
    /// waits up to 5 s more for BlueZ to resolve the services. So a `connect`
    /// can run out of time with the link already up and BlueZ still setting
    /// up the ATT channel or discovering.
    fn is_bluez(&self) -> bool;

    /// Enable notifications on `characteristic` (the CCCD write).
    fn subscribe(
        &self,
        characteristic: &Characteristic,
    ) -> impl Future<Output = btleplug::Result<()>> + Send;

    /// Disable notifications on `characteristic`.
    fn unsubscribe(
        &self,
        characteristic: &Characteristic,
    ) -> impl Future<Output = btleplug::Result<()>> + Send;

    /// Open a stream of the notifications the peripheral sends from now on,
    /// from every characteristic.
    fn notifications(&self) -> impl Future<Output = btleplug::Result<NotificationStream>> + Send;

    /// Pair with the device before connecting, if the platform needs that,
    /// giving the pairing about `budget`. Only Linux pairs (the `bluez_agent`
    /// module); everywhere else this does nothing. It never fails: a pairing
    /// problem is logged and the connect goes ahead.
    fn pair_if_needed(&self, _budget: Duration) -> impl Future<Output = ()> + Send {
        async {}
    }
}

// Every call names btleplug's trait: `GattLink` is the trait in scope here, so
// a plain `self.connect()` would call this method itself.
impl GattLink for btleplug::platform::Peripheral {
    async fn connect(&self) -> btleplug::Result<()> {
        btleplug::api::Peripheral::connect(self).await
    }

    async fn disconnect(&self) -> btleplug::Result<()> {
        btleplug::api::Peripheral::disconnect(self).await
    }

    async fn discover_services(&self) -> btleplug::Result<()> {
        btleplug::api::Peripheral::discover_services(self).await
    }

    fn services(&self) -> BTreeSet<Service> {
        btleplug::api::Peripheral::services(self)
    }

    async fn properties(&self) -> btleplug::Result<Option<PeripheralProperties>> {
        btleplug::api::Peripheral::properties(self).await
    }

    async fn is_connected(&self) -> btleplug::Result<bool> {
        btleplug::api::Peripheral::is_connected(self).await
    }

    fn is_bluez(&self) -> bool {
        // btleplug's Linux backend is bluez-async; macOS and Windows have
        // their own.
        cfg!(target_os = "linux")
    }

    async fn subscribe(&self, characteristic: &Characteristic) -> btleplug::Result<()> {
        btleplug::api::Peripheral::subscribe(self, characteristic).await
    }

    async fn unsubscribe(&self, characteristic: &Characteristic) -> btleplug::Result<()> {
        btleplug::api::Peripheral::unsubscribe(self, characteristic).await
    }

    async fn notifications(&self) -> btleplug::Result<NotificationStream> {
        btleplug::api::Peripheral::notifications(self).await
    }

    /// Pairs the sensor through BlueZ first if BlueZ doesn't list it as paired.
    #[cfg(target_os = "linux")]
    async fn pair_if_needed(&self, budget: Duration) {
        let _ = crate::bluez_agent::pair_if_needed(self, budget).await;
    }
}

/// The stream of every notification a peripheral sends, as btleplug's
/// `Peripheral::notifications` returns it.
pub(crate) type NotificationStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = btleplug::api::ValueNotification> + Send>>;

/// The notification tasks of one connection: at most one per characteristic.
///
/// Each task reads its own notification stream and passes the values from its
/// characteristic to that characteristic's callback. The map's lock is never
/// held across an `.await`, so `Drop for Device` can always take it.
#[derive(Default)]
pub(crate) struct NotificationTasks(Mutex<HashMap<Uuid, JoinHandle<()>>>);

impl NotificationTasks {
    /// Lock the task map. Never hold the guard across an `.await`.
    fn tasks(&self) -> MutexGuard<'_, HashMap<Uuid, JoinHandle<()>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Call `callback` with the value of every notification from
    /// `characteristic`, on a task of the current runtime.
    ///
    /// Opens the notification stream before enabling notifications, so nothing
    /// the device sends straight after the CCCD write is missed. Replaces the
    /// characteristic's previous task; if this fails, the previous task keeps
    /// running. Each Bluetooth call gives up after `limit`.
    pub(crate) async fn subscribe<L, F>(
        &self,
        link: &L,
        characteristic: &Characteristic,
        limit: Duration,
        callback: F,
    ) -> Result<()>
    where
        L: GattLink,
        F: Fn(&[u8]) + Send + Sync + 'static,
    {
        let mut stream = bounded("open notification stream", limit, link.notifications()).await?;
        bounded(
            "subscribe to notifications",
            limit,
            link.subscribe(characteristic),
        )
        .await?;

        let uuid = characteristic.uuid;
        let task = tokio::spawn(async move {
            while let Some(notification) = stream.next().await {
                if notification.uuid == uuid {
                    callback(&notification.value);
                }
            }
        });
        // The guard is dropped at the end of this statement.
        let previous = self.tasks().insert(uuid, task);
        if let Some(previous) = previous {
            // An aborted task is never polled again.
            previous.abort();
        }
        Ok(())
    }

    /// Stop the characteristic's callback, then disable its notifications.
    ///
    /// The callback is not called again once this returns, even if the device
    /// call fails. Waiting for the task to end and the device call each give
    /// up after `limit`.
    pub(crate) async fn unsubscribe<L: GattLink>(
        &self,
        link: &L,
        characteristic: &Characteristic,
        limit: Duration,
    ) -> Result<()> {
        let task = self.tasks().remove(&characteristic.uuid);
        if let Some(task) = task {
            task.abort();
            // Once aborted, the task is never polled again, so once it has
            // ended, a callback that was running on another thread has
            // returned. The wait is limited: an aborted task on a runtime that
            // is alive but not being driven never reports back.
            if tokio::time::timeout(limit, task).await.is_err() {
                warn!(
                    "Notification task for {} did not stop within {limit:?}",
                    characteristic.uuid
                );
            }
        }
        bounded(
            "unsubscribe from notifications",
            limit,
            link.unsubscribe(characteristic),
        )
        .await
    }

    /// Abort every task without waiting for it to end (`Device::disconnect`
    /// and `Drop`).
    pub(crate) fn abort_all(&self) {
        for (_, task) in self.tasks().drain() {
            task.abort();
        }
    }

    /// How many notification tasks are kept.
    #[cfg(test)]
    pub(crate) fn task_count(&self) -> usize {
        self.tasks().len()
    }
}

/// Await the btleplug call `future` for at most `limit`.
///
/// A btleplug error becomes `Error::Bluetooth`. Running out of time becomes
/// `Error::Timeout { operation, duration: limit }`.
pub(crate) async fn bounded<T>(
    operation: &str,
    limit: Duration,
    future: impl Future<Output = btleplug::Result<T>>,
) -> Result<T> {
    match tokio::time::timeout(limit, future).await {
        Ok(result) => Ok(result?),
        Err(_) => Err(Error::timeout(operation, limit)),
    }
}

/// How long a disconnect may wait for the Bluetooth stack to confirm it: the
/// 5 s that `DeviceGuard`'s drop has always allowed.
pub(crate) const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How much longer than its budget the pairing step may take before the
/// connect goes ahead without it (see `connect_and_discover`). The Linux
/// session reports its outcome within 3 s of its budget.
const PAIRING_GRACE: Duration = Duration::from_secs(5);

/// Disconnect `link`, giving up after `DISCONNECT_TIMEOUT` with
/// `Error::Timeout` ("disconnect from device").
pub(crate) async fn disconnect<L: GattLink>(link: &L) -> Result<()> {
    bounded(
        "disconnect from device",
        DISCONNECT_TIMEOUT,
        link.disconnect(),
    )
    .await
}

/// Run `disconnect(link)` as a task on `runtime` and wait for its result.
///
/// The task is spawned when this future is first polled and keeps running if
/// this future is dropped, so a caller that times out or is cancelled still
/// releases the sensor. Like `disconnect`, it gives up after
/// `DISCONNECT_TIMEOUT`.
pub(crate) async fn disconnect_detached<L: GattLink>(
    link: &L,
    runtime: &tokio::runtime::Handle,
) -> Result<()> {
    let link = link.clone();
    runtime
        .spawn(async move { disconnect(&link).await })
        .await
        .map_err(std::io::Error::from)?
}

/// The connection state that the Bluetooth stack reports, or `false`, with a
/// warning, if the stack fails or doesn't answer within `limit`.
pub(crate) async fn is_connected<L: GattLink>(link: &L, limit: Duration) -> bool {
    match bounded("query connection state", limit, link.is_connected()).await {
        Ok(connected) => connected,
        Err(e) => {
            warn!("Failed to query connection state: {e}");
            false
        }
    }
}

/// Where cleanup that must outlive its caller runs: aranet-core's `aranet-ble`
/// runtime, or the current runtime if that one can't be started. `None` only
/// outside any tokio runtime.
pub(crate) fn cleanup_runtime() -> Option<Handle> {
    crate::runtime::handle()
        .ok()
        .or_else(|| Handle::try_current().ok())
}

/// Owns a link from the first Bluetooth call of a connect until the `Device`
/// takes it over, and disconnects it if the connect doesn't get that far.
///
/// `connect` closes it, and waits for the disconnect, when a step fails. If
/// the connect future is dropped first (a caller's timeout or `select!`, an
/// aborted task), `Drop` runs the disconnect as a task on `cleanup`.
#[must_use = "dropping a PendingLink disconnects the link"]
pub(crate) struct PendingLink<L: GattLink> {
    /// The link to disconnect; `None` once disarmed or closed.
    link: Option<L>,
    /// The runtime that `Drop` runs the disconnect on.
    cleanup: Handle,
}

impl<L: GattLink> PendingLink<L> {
    fn arm(link: &L, cleanup: &Handle) -> Self {
        Self {
            link: Some(link.clone()),
            cleanup: cleanup.clone(),
        }
    }

    /// The `Device` owns the link now.
    pub(crate) fn disarm(mut self) {
        self.link = None;
    }

    /// Disconnect now, waiting at most `DISCONNECT_TIMEOUT`.
    async fn close(mut self) {
        if let Some(link) = self.link.take()
            && let Err(e) = disconnect(&link).await
        {
            debug!("Disconnect after a failed connect failed: {e}");
        }
    }
}

impl<L: GattLink> Drop for PendingLink<L> {
    fn drop(&mut self) {
        // `disarm` and `close` take the link, so it is still here only when
        // the connect future was dropped before it finished.
        if let Some(link) = self.link.take() {
            warn!("Connect cancelled before it finished; disconnecting");
            self.cleanup.spawn(async move {
                if let Err(e) = disconnect(&link).await {
                    debug!("Disconnect after a cancelled connect failed: {e}");
                }
            });
        }
    }
}

/// A connected peripheral whose services have been discovered.
pub(crate) struct OpenLink<L: GattLink> {
    /// Disconnects the link unless the caller disarms it.
    pub(crate) pending: PendingLink<L>,
    /// The GATT services the peripheral reported.
    pub(crate) services: BTreeSet<Service>,
    /// What the peripheral advertised, if the Bluetooth stack knows it.
    pub(crate) properties: Option<PeripheralProperties>,
}

/// Connect to `link`, discover its services and read its properties.
///
/// The returned `OpenLink` is still armed: the caller disarms its `pending`
/// once it owns the link. If a step fails or times out, `link` is disconnected
/// (waiting at most `DISCONNECT_TIMEOUT`) before the error is returned. If this
/// future is dropped first, the disconnect runs as a task on `cleanup`.
pub(crate) async fn connect<L: GattLink>(
    link: &L,
    config: &ConnectionConfig,
    cleanup: &Handle,
) -> Result<OpenLink<L>> {
    // Armed before the first Bluetooth call: on BlueZ, `connect()` can fail
    // with the link already up.
    let pending = PendingLink::arm(link, cleanup);
    match connect_and_discover(link, config).await {
        Ok((services, properties)) => Ok(OpenLink {
            pending,
            services,
            properties,
        }),
        Err(e) => {
            // Awaited, so the caller's next attempt can't race the disconnect.
            pending.close().await;
            Err(e)
        }
    }
}

/// Pair if needed, connect, discover the services and read the properties:
///
/// 1. On Linux, pair (`GattLink::pair_if_needed`, a no-op elsewhere) with a
///    budget of `connection_timeout` plus `bluez_discovery_limit`, 35 s at
///    defaults: when BlueZ connects the sensor for `Pair`, it answers only
///    after its service discovery. A pairing step still running
///    `PAIRING_GRACE` after its budget is abandoned. Pairing never fails the
///    connect.
/// 2. Connect, giving up after `connection_timeout` ("connect to device").
/// 3. Discover the services, giving up after `discovery_timeout` ("discover
///    services").
/// 4. If that discovery finds no services, disconnect, wait 2 s, and connect
///    and discover once more ("reconnect to device", "rediscover services").
///    If that discovery finds none either, fail with a retryable
///    `Error::ConnectionFailed` ("the device reported no GATT services").
/// 5. Read the properties, giving up after `read_timeout`.
///
/// On BlueZ, a connect (the first one or the retry's) can end while BlueZ is
/// still discovering the services on the live link. BlueZ answers
/// `Device1.Connect` only once the ATT channel is up, a few seconds after the
/// link, and bluez-async stops waiting for the services 5 s after that answer.
/// A link that comes up late can also use up `connection_timeout` first, while
/// BlueZ is still setting up the ATT channel or discovering; `connect_link`
/// then asks whether the link is up. Either way the discovery after it first
/// waits for BlueZ to finish (`wait_for_bluez_discovery`), and that
/// connection-state query, the wait and the discovery share one
/// `bluez_discovery_limit` (20 s at defaults), whose end fails the connect
/// with a "discover services" (or "rediscover services") timeout.
///
/// `connect` disconnects the link when any step fails.
async fn connect_and_discover<L: GattLink>(
    link: &L,
    config: &ConnectionConfig,
) -> Result<(BTreeSet<Service>, Option<PeripheralProperties>)> {
    // Pair first (Linux): BlueZ refuses `Pair` while a `Connect` is in flight,
    // and `Pair` connects the device itself, so this runs inside the caller's
    // `PendingLink` guard. When BlueZ connects the sensor for `Pair`, it answers
    // only after its service discovery, so the budget covers the connect and
    // that discovery (`bluez_discovery_limit`). A pairing problem never stops
    // the connect; what an unpaired sensor does on Linux is described in
    // `bluez_agent`. `saturating_add`, because callers may pass `Duration::MAX`
    // to mean "no limit".
    let pairing_budget = config
        .connection_timeout
        .saturating_add(bluez_discovery_limit(config));
    let pairing_limit = pairing_budget.saturating_add(PAIRING_GRACE);
    if tokio::time::timeout(pairing_limit, link.pair_if_needed(pairing_budget))
        .await
        .is_err()
    {
        warn!("Pairing did not finish within {pairing_limit:?}; connecting anyway");
    }

    // Connect to the device with timeout
    info!("Connecting to device...");
    let bluez_wait = connect_link(link, "connect to device", config).await?;
    info!("Connected!");

    // Discover services with timeout
    info!("Discovering services...");
    discover(link, "discover services", bluez_wait, config).await?;

    let mut services = link.services();

    // If service discovery returned nothing, BlueZ may have stale state
    // from a previous failed connection (e.g., auth failure during GATT
    // discovery). Disconnect, wait, and retry once with a clean connection.
    if services.is_empty() {
        warn!("Service discovery returned 0 services — retrying with fresh connection");
        if let Err(e) = disconnect(link).await {
            debug!("Disconnect before reconnecting failed: {e}");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;

        let bluez_wait = connect_link(link, "reconnect to device", config).await?;
        discover(link, "rediscover services", bluez_wait, config).await?;

        services = link.services();
        if services.is_empty() {
            // Retrying on this link again won't help. Fail with a retryable
            // error instead of returning a device whose every read would fail
            // with "characteristic not found".
            return Err(Error::connection_failed_str(
                None,
                "the device reported no GATT services",
            ));
        }
    }

    debug!("Found {} services", services.len());

    // Get device properties
    let properties = bounded(
        "read device properties",
        config.read_timeout,
        link.properties(),
    )
    .await?;

    Ok((services, properties))
}

/// What bluez-async's `connect` fails with when BlueZ hasn't resolved the
/// services 5 s after `Device1.Connect` returned, although the link is up and
/// BlueZ is still discovering (`BluetoothError::ServiceDiscoveryTimedOut`,
/// bluez-async 0.8.2 `src/lib.rs:61`, `:90-92`, `:696-724`). btleplug passes
/// it on as `btleplug::Error::Other` (`src/bluez/adapter.rs:125-129`). No
/// other btleplug backend produces it. aranet-core can't name bluez-async's
/// type (it isn't a dependency), so this matches the message.
const BLUEZ_DISCOVERY_TIMED_OUT: &str = "Service discovery timed out";

/// The least time a connect waits for BlueZ to finish discovering the services
/// once bluez-async has given up, or the connect has run out of time with the
/// link up (BR-25). An Aranet4 that BlueZ hasn't cached needs about 30-48 ATT
/// requests at 0.3-0.45 s each: 9-22 s from the moment BlueZ answers
/// `Device1.Connect`, which on an LE link it does once the ATT channel is up,
/// a few seconds after the link. bluez-async has already waited 5 s of that
/// when it gives up. When the connect runs out of time with the link up, less
/// than 5 s of it has passed, or none if BlueZ hasn't answered yet.
pub(crate) const BLUEZ_DISCOVERY_MIN_WAIT: Duration = Duration::from_secs(20);

/// How long a connect waits for BlueZ to finish discovering the services once
/// `connect_link` has found it still discovering: `discovery_timeout`, but at
/// least `BLUEZ_DISCOVERY_MIN_WAIT`.
pub(crate) fn bluez_discovery_limit(config: &ConnectionConfig) -> Duration {
    config.discovery_timeout.max(BLUEZ_DISCOVERY_MIN_WAIT)
}

/// Whether `error` is bluez-async's service discovery timeout.
fn is_bluez_discovery_timeout(error: &btleplug::Error) -> bool {
    matches!(error, btleplug::Error::Other(e) if e.to_string() == BLUEZ_DISCOVERY_TIMED_OUT)
}

/// What BlueZ refuses a `Device1.Connect` with while an earlier one to the
/// same device is still pending, or while the device is being paired
/// (`org.bluez.Error.InProgress`). On an LE link BlueZ answers a
/// `Device1.Connect` only once the ATT channel is up, a few seconds after the
/// link (`src/device.c`: `dev_connect` refuses while `dev->connect` is set,
/// `att_connect_cb` answers it). bluez-async passes the D-Bus error on with
/// its message as its text, and btleplug wraps it in `btleplug::Error::Other`,
/// so this matches the message, like `BLUEZ_DISCOVERY_TIMED_OUT`.
const BLUEZ_IN_PROGRESS: &str = "In Progress";

/// How long a wait after a connect timeout pauses before it asks again when
/// BlueZ answered `BLUEZ_IN_PROGRESS`.
const BLUEZ_IN_PROGRESS_PAUSE: Duration = Duration::from_secs(1);

/// Whether `error` is BlueZ's `org.bluez.Error.InProgress`.
fn is_bluez_in_progress(error: &btleplug::Error) -> bool {
    matches!(error, btleplug::Error::Other(e) if e.to_string() == BLUEZ_IN_PROGRESS)
}

/// Why `connect_link` started a `BluezWait`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BluezWaitReason {
    /// bluez-async's `connect` gave up waiting for the services, so BlueZ has
    /// answered `Device1.Connect`.
    DiscoveryTimedOut,
    /// The connect ran out of time with the link up. BlueZ may not have
    /// answered that `Device1.Connect` yet.
    ConnectTimedOut,
}

/// How far ahead a deadline is set when its limit is too long to add to the
/// current time, such as `Duration::MAX`: about 30 years, as
/// `tokio::time::timeout` does for such a limit.
const FAR_FUTURE: Duration = Duration::from_secs(86_400 * 365 * 30);

/// A wait for BlueZ to finish discovering the services: `connect_link` starts
/// it, and `discover` waits.
struct BluezWait {
    /// Why the wait started.
    reason: BluezWaitReason,
    /// When the wait and the discovery after it run out of time:
    /// `bluez_discovery_limit` after the wait started, or `FAR_FUTURE` after
    /// it when that limit is too long to add. The connection-state query that
    /// `connect_link` makes before it starts a wait after a connect timeout
    /// counts against it too.
    deadline: Instant,
}

impl BluezWait {
    fn start(reason: BluezWaitReason, config: &ConnectionConfig) -> Self {
        // `checked_add`, because callers may pass `Duration::MAX` to mean "no
        // limit", and adding that to an `Instant` panics.
        let now = Instant::now();
        Self {
            reason,
            deadline: now
                .checked_add(bluez_discovery_limit(config))
                .unwrap_or_else(|| now + FAR_FUTURE),
        }
    }

    /// The time left until `deadline`.
    fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
}

/// Connect, giving up after `connection_timeout` (reported as `operation`).
///
/// Returns a `BluezWait` when the link is up but BlueZ may still be
/// discovering the services: bluez-async stopped waiting for them, or, on
/// BlueZ, the connect ran out of time with the link already up.
async fn connect_link<L: GattLink>(
    link: &L,
    operation: &str,
    config: &ConnectionConfig,
) -> Result<Option<BluezWait>> {
    let wait = match bounded(operation, config.connection_timeout, link.connect()).await {
        Ok(()) => return Ok(None),
        Err(Error::Bluetooth(e)) if is_bluez_discovery_timeout(&e) => {
            BluezWait::start(BluezWaitReason::DiscoveryTimedOut, config)
        }
        // BlueZ answers `Device1.Connect` only once the ATT channel is up, a
        // few seconds after the link, and bluez-async's `connect` then waits
        // up to 5 s more for the services. So a link that comes up late in
        // `connection_timeout` runs out of time here while BlueZ is still
        // setting up the ATT channel or discovering on it. Disconnecting
        // would cancel that, and every retry would start it again.
        Err(timeout @ Error::Timeout { .. }) if link.is_bluez() => {
            // The query counts against the wait's limit, so the connect still
            // takes at most `connection_timeout`, and the query and the wait
            // together at most `bluez_discovery_limit`.
            let wait = BluezWait::start(BluezWaitReason::ConnectTimedOut, config);
            let query_limit = config.validation_timeout.min(wait.remaining());
            if !is_connected(link, query_limit).await {
                return Err(timeout);
            }
            debug!("The connect ran out of time with the link up");
            wait
        }
        Err(e) => return Err(e),
    };
    // The limit also covers the connection-state query after a connect
    // timeout, and the discovery after the wait.
    info!(
        "BlueZ is still discovering services; waiting up to {:?} in all",
        bluez_discovery_limit(config)
    );
    Ok(Some(wait))
}

/// Discover the services, giving up after `discovery_timeout` (reported as
/// `operation`).
///
/// With a `bluez_wait`, first wait for BlueZ to finish. The wait and the
/// discovery then share the wait's deadline, and running out of it is
/// reported as a timeout of `bluez_discovery_limit`.
async fn discover<L: GattLink>(
    link: &L,
    operation: &str,
    bluez_wait: Option<BluezWait>,
    config: &ConnectionConfig,
) -> Result<()> {
    let Some(wait) = bluez_wait else {
        return bounded(
            operation,
            config.discovery_timeout,
            link.discover_services(),
        )
        .await;
    };
    let waited = tokio::time::timeout_at(wait.deadline, async {
        wait_for_bluez_discovery(link, wait.reason).await?;
        link.discover_services().await
    });
    match waited.await {
        Ok(result) => Ok(result?),
        Err(_) => Err(Error::timeout(operation, bluez_discovery_limit(config))),
    }
}

/// Call `connect` again until BlueZ has resolved the services.
///
/// Once BlueZ has answered the connect that started the wait, it answers
/// `Device1.Connect` at once while the device is connected (`src/device.c`,
/// `dev_connect`), and bluez-async then waits up to 5 s more for
/// `ServicesResolved`, or returns at once if it is already set. If the link
/// has dropped, `Device1.Connect` opens a new one, as the caller's own retry
/// would.
///
/// After a connect timeout (`BluezWaitReason::ConnectTimedOut`), BlueZ may not
/// have answered the timed-out `Device1.Connect` yet, and until it does it
/// refuses the next one with `In Progress`. Then the wait asks again after
/// `BLUEZ_IN_PROGRESS_PAUSE`. After bluez-async's discovery timeout BlueZ has
/// answered, so an `In Progress` there comes from another connect to the
/// device or a pairing of it, and ends the wait.
///
/// Any other result ends the wait. The caller limits it.
async fn wait_for_bluez_discovery<L: GattLink>(
    link: &L,
    reason: BluezWaitReason,
) -> btleplug::Result<()> {
    loop {
        match link.connect().await {
            Err(e) if is_bluez_discovery_timeout(&e) => {
                debug!("BlueZ is still discovering services");
            }
            Err(e) if reason == BluezWaitReason::ConnectTimedOut && is_bluez_in_progress(&e) => {
                debug!(
                    "BlueZ hasn't answered the connect yet; asking again in {:?}",
                    BLUEZ_IN_PROGRESS_PAUSE
                );
                tokio::time::sleep(BLUEZ_IN_PROGRESS_PAUSE).await;
            }
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::Duration;

    use tokio::runtime::Handle;
    use tokio::time::Instant;

    use super::fake::{Call, FakeGatt, Outcome, aranet_services};
    use super::{DISCONNECT_TIMEOUT, connect, disconnect_detached, is_connected};
    use crate::device::ConnectionConfig;
    use crate::error::{ConnectionFailureReason, Error};
    use crate::test_support::within;

    /// Run a connect on `fake` that must fail, with the default config and the
    /// test's runtime for cleanup, and return its error.
    async fn failed_connect(fake: &FakeGatt) -> Error {
        let rt = Handle::current();
        let cfg = ConnectionConfig::default();
        match connect(fake, &cfg, &rt).await {
            Ok(open) => {
                open.pending.disarm();
                panic!("the connect should have failed");
            }
            Err(e) => e,
        }
    }

    /// Check that `error` is a timeout of `operation` after `duration`.
    pub(super) fn assert_timeout(error: &Error, operation: &str, duration: Duration) {
        match error {
            Error::Timeout {
                operation: op,
                duration: d,
            } => {
                assert_eq!(op, operation);
                assert_eq!(*d, duration);
            }
            other => panic!("expected a '{operation}' timeout, got {other:?}"),
        }
    }

    /// A plain connect makes one connect and one discovery, and leaves the
    /// link up with the services and properties the peripheral reported. Once
    /// disarmed, nothing disconnects it in the background.
    #[tokio::test(start_paused = true)]
    async fn successful_connect_leaves_link_up() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            let rt = Handle::current();
            let cfg = ConnectionConfig::default();

            let open = connect(&fake, &cfg, &rt)
                .await
                .expect("connect should succeed");
            open.pending.disarm();

            assert_eq!(open.services, aranet_services());
            assert_eq!(
                open.properties.and_then(|p| p.local_name).as_deref(),
                Some("Aranet4 12345")
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
            assert_eq!(fake.calls(), [Call::Connect, Call::Discover]);
        })
        .await;
    }

    /// An empty first discovery is retried once on a fresh connection, 2 s
    /// after disconnecting, and the second discovery's services are used.
    #[tokio::test(start_paused = true)]
    async fn zero_services_then_services_connects() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.service_rounds([BTreeSet::new(), aranet_services()]);
            let rt = Handle::current();
            let cfg = ConnectionConfig::default();
            let start = Instant::now();

            let open = connect(&fake, &cfg, &rt)
                .await
                .expect("the second discovery should find the services");
            open.pending.disarm();

            assert_eq!(
                fake.calls(),
                [
                    Call::Connect,
                    Call::Discover,
                    Call::Disconnect,
                    Call::Connect,
                    Call::Discover,
                ]
            );
            assert_eq!(open.services, aranet_services());
            assert_eq!(start.elapsed(), Duration::from_secs(2));
        })
        .await;
    }

    /// A connect that never answers fails after `connection_timeout`, with the
    /// operation name callers already match on, and disconnects first.
    #[tokio::test(start_paused = true)]
    async fn connect_timeout_disconnects_before_returning() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.script_connect([Outcome::Hang]);
            let start = Instant::now();

            let error = failed_connect(&fake).await;

            assert_timeout(&error, "connect to device", Duration::from_secs(15));
            assert_eq!(start.elapsed(), Duration::from_secs(15));
            assert_eq!(fake.calls(), [Call::Connect, Call::Disconnect]);
        })
        .await;
    }

    /// A connect that fails disconnects before returning: on BlueZ, a failed
    /// `connect` can leave the link up.
    #[tokio::test(start_paused = true)]
    async fn connect_error_disconnects_before_returning() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.script_connect([Outcome::Fail]);

            let error = failed_connect(&fake).await;

            assert!(matches!(error, Error::Bluetooth(_)), "got {error:?}");
            assert_eq!(fake.calls(), [Call::Connect, Call::Disconnect]);
        })
        .await;
    }

    /// A discovery that never answers disconnects before the error returns.
    #[tokio::test(start_paused = true)]
    async fn discovery_timeout_disconnects() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.script_discover([Outcome::Hang]);
            let start = Instant::now();

            let error = failed_connect(&fake).await;

            assert_timeout(&error, "discover services", Duration::from_secs(10));
            assert_eq!(start.elapsed(), Duration::from_secs(10));
            assert_eq!(
                fake.calls(),
                [Call::Connect, Call::Discover, Call::Disconnect]
            );
        })
        .await;
    }

    /// A discovery that fails disconnects before the error returns.
    #[tokio::test(start_paused = true)]
    async fn discovery_error_disconnects() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.script_discover([Outcome::Fail]);

            let error = failed_connect(&fake).await;

            assert!(matches!(error, Error::Bluetooth(_)), "got {error:?}");
            assert_eq!(
                fake.calls(),
                [Call::Connect, Call::Discover, Call::Disconnect]
            );
        })
        .await;
    }

    /// Reading the properties at connect is limited by `read_timeout`, and a
    /// failure there disconnects too.
    #[tokio::test(start_paused = true)]
    async fn properties_failure_disconnects() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.script_properties([Outcome::Hang]);
            let start = Instant::now();

            let error = failed_connect(&fake).await;

            assert_timeout(&error, "read device properties", Duration::from_secs(10));
            assert_eq!(start.elapsed(), Duration::from_secs(10));
            assert_eq!(
                fake.calls(),
                [Call::Connect, Call::Discover, Call::Disconnect]
            );
        })
        .await;
    }

    /// A caller that drops the connect future part-way still gets the link
    /// disconnected, by a task on the cleanup runtime, even after the caller's
    /// own runtime has shut down.
    #[test]
    fn cancelled_connect_disconnects_in_background() {
        let paused_runtime = || {
            tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .start_paused(true)
                .build()
                .unwrap()
        };
        let caller = paused_runtime();
        let cleanup = paused_runtime();
        let fake = FakeGatt::new();
        fake.script_connect([Outcome::Hang]);
        let cfg = ConnectionConfig::default();

        caller.block_on(async {
            assert!(
                tokio::time::timeout(
                    Duration::from_secs(1),
                    connect(&fake, &cfg, cleanup.handle())
                )
                .await
                .is_err()
            );
        });
        // A disconnect spawned on the caller's runtime is dropped here unrun,
        // as it is when a `#[tokio::test]` or a per-call runtime ends.
        drop(caller);

        cleanup.block_on(within(
            Duration::from_secs(10),
            fake.disconnected().notified(),
        ));
        assert_eq!(fake.calls(), [Call::Connect, Call::Disconnect]);
    }

    /// The cleanup after a failed connect waits at most `DISCONNECT_TIMEOUT`
    /// for a disconnect that never answers.
    #[tokio::test(start_paused = true)]
    async fn cleanup_waits_at_most_disconnect_timeout() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.script_connect([Outcome::Hang]);
            fake.script_disconnect([Outcome::Hang]);
            let start = Instant::now();

            let error = failed_connect(&fake).await;

            assert_timeout(&error, "connect to device", Duration::from_secs(15));
            assert_eq!(
                start.elapsed(),
                Duration::from_secs(15) + DISCONNECT_TIMEOUT
            );
            assert_eq!(fake.calls(), [Call::Connect, Call::Disconnect]);
        })
        .await;
    }

    /// A reconnect that times out during the zero-services retry disconnects
    /// before the error returns.
    #[tokio::test(start_paused = true)]
    async fn failed_reconnect_during_retry_disconnects() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.service_rounds([BTreeSet::new()]);
            fake.script_connect([Outcome::Ok, Outcome::Hang]);
            let start = Instant::now();

            let error = failed_connect(&fake).await;

            assert_timeout(&error, "reconnect to device", Duration::from_secs(15));
            assert_eq!(
                fake.calls(),
                [
                    Call::Connect,
                    Call::Discover,
                    Call::Disconnect,
                    Call::Connect,
                    Call::Disconnect,
                ]
            );
            assert_eq!(start.elapsed(), Duration::from_secs(17));
        })
        .await;
    }

    /// The rediscovery of the zero-services retry is limited by
    /// `discovery_timeout`, and running out is reported as "rediscover
    /// services".
    #[tokio::test(start_paused = true)]
    async fn rediscovery_timeout_names_the_rediscovery() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.service_rounds([BTreeSet::new()]);
            fake.script_discover([Outcome::Ok, Outcome::Hang]);
            let start = Instant::now();

            let error = failed_connect(&fake).await;

            assert_timeout(&error, "rediscover services", Duration::from_secs(10));
            assert_eq!(start.elapsed(), Duration::from_secs(12));
            assert_eq!(
                fake.calls(),
                [
                    Call::Connect,
                    Call::Discover,
                    Call::Disconnect,
                    Call::Connect,
                    Call::Discover,
                    Call::Disconnect,
                ]
            );
        })
        .await;
    }

    /// Discovery that finds no services even after the retry fails the connect
    /// with a retryable error, and disconnects, instead of returning a device
    /// whose every read fails.
    #[tokio::test(start_paused = true)]
    async fn zero_services_after_retry_fails_and_disconnects() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.service_rounds([BTreeSet::new(), BTreeSet::new()]);

            let error = failed_connect(&fake).await;

            match &error {
                Error::ConnectionFailed {
                    reason: ConnectionFailureReason::Other(reason),
                    ..
                } => assert!(reason.contains("no GATT services"), "reason: {reason}"),
                other => panic!("expected a connection failure, got {other:?}"),
            }
            assert_eq!(
                fake.calls(),
                [
                    Call::Connect,
                    Call::Discover,
                    Call::Disconnect,
                    Call::Connect,
                    Call::Discover,
                    Call::Disconnect,
                ]
            );
        })
        .await;
    }

    /// The disconnect before the zero-services retry gives up after
    /// `DISCONNECT_TIMEOUT` instead of hanging the connect.
    #[tokio::test(start_paused = true)]
    async fn zero_services_disconnect_is_time_limited() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.service_rounds([BTreeSet::new(), aranet_services()]);
            fake.script_disconnect([Outcome::Hang]);
            let rt = Handle::current();
            let cfg = ConnectionConfig::default();
            let start = Instant::now();

            let open = connect(&fake, &cfg, &rt)
                .await
                .expect("the retry should connect");
            open.pending.disarm();

            assert_eq!(start.elapsed(), DISCONNECT_TIMEOUT + Duration::from_secs(2));
            assert_eq!(
                fake.calls(),
                [
                    Call::Connect,
                    Call::Discover,
                    Call::Disconnect,
                    Call::Connect,
                    Call::Discover,
                ]
            );
        })
        .await;
    }

    /// A detached disconnect that the stack never confirms gives up after
    /// `DISCONNECT_TIMEOUT`, with the operation name that `disconnect` uses.
    #[tokio::test(start_paused = true)]
    async fn detached_disconnect_gives_up_after_disconnect_timeout() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.script_disconnect([Outcome::Hang]);
            let rt = Handle::current();
            let start = Instant::now();

            match disconnect_detached(&fake, &rt).await {
                Err(Error::Timeout {
                    operation,
                    duration,
                }) => {
                    assert_eq!(operation, "disconnect from device");
                    assert_eq!(duration, DISCONNECT_TIMEOUT);
                }
                other => panic!("expected a disconnect timeout, got {other:?}"),
            }
            assert_eq!(start.elapsed(), Duration::from_secs(5));
            assert_eq!(fake.calls(), [Call::Disconnect]);
        })
        .await;
    }

    /// A caller that stops waiting doesn't cancel a detached disconnect: it
    /// still reaches the stack and completes.
    #[tokio::test(start_paused = true)]
    async fn detached_disconnect_finishes_after_the_caller_is_dropped() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.script_disconnect([Outcome::After(Duration::from_secs(2))]);
            let rt = Handle::current();
            let start = Instant::now();

            assert!(
                tokio::time::timeout(Duration::from_millis(10), disconnect_detached(&fake, &rt))
                    .await
                    .is_err()
            );

            within(Duration::from_secs(10), fake.disconnected().notified()).await;
            assert_eq!(start.elapsed(), Duration::from_secs(2));
            assert_eq!(fake.calls(), [Call::Disconnect]);
        })
        .await;
    }

    /// A connection-state query that the stack never answers (CoreBluetooth
    /// after the sensor has gone) reports `false` after `validation_timeout`.
    #[tokio::test(start_paused = true)]
    async fn is_connected_reports_false_when_the_stack_never_answers() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.script_is_connected([Outcome::Hang]);
            let cfg = ConnectionConfig::default();
            let start = Instant::now();

            assert!(!is_connected(&fake, cfg.validation_timeout).await);
            assert_eq!(start.elapsed(), Duration::from_secs(3));
            assert_eq!(fake.calls(), [Call::IsConnected]);
        })
        .await;
    }

    /// A connection-state query that fails reports `false` at once.
    #[tokio::test(start_paused = true)]
    async fn is_connected_reports_false_on_error() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.script_is_connected([Outcome::Fail]);
            let cfg = ConnectionConfig::default();
            let start = Instant::now();

            assert!(!is_connected(&fake, cfg.validation_timeout).await);
            assert_eq!(start.elapsed(), Duration::ZERO);
            assert_eq!(fake.calls(), [Call::IsConnected]);
        })
        .await;
    }

    /// A connection-state query that the stack answers returns that answer,
    /// `true` or `false`, at once.
    #[tokio::test(start_paused = true)]
    async fn is_connected_passes_the_answer_through() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            let cfg = ConnectionConfig::default();
            let start = Instant::now();

            fake.script_is_connected([Outcome::Ok]);
            assert!(is_connected(&fake, cfg.validation_timeout).await);

            fake.set_connected(false);
            fake.script_is_connected([Outcome::Ok]);
            assert!(!is_connected(&fake, cfg.validation_timeout).await);

            assert_eq!(start.elapsed(), Duration::ZERO);
            assert_eq!(fake.calls(), [Call::IsConnected, Call::IsConnected]);
        })
        .await;
    }

    /// One notification task per characteristic (BR-13, BR-12).
    mod notifications {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        use futures::FutureExt;
        use tokio::time::Instant;
        use uuid::Uuid;

        use crate::error::Error;
        use crate::link::NotificationTasks;
        use crate::link::fake::{Call, FakeGatt, Outcome, characteristic};
        use crate::test_support::within;

        /// The limit for every call, as `ConnectionConfig::default().write_timeout`.
        const LIMIT: Duration = Duration::from_secs(10);
        const C1: Uuid = Uuid::from_u128(0xc1);
        const C2: Uuid = Uuid::from_u128(0xc2);

        /// A callback that counts its calls in `count`.
        fn counter(count: &Arc<AtomicUsize>) -> impl Fn(&[u8]) + Send + Sync + 'static {
            let count = Arc::clone(count);
            move |_: &[u8]| {
                count.fetch_add(1, Ordering::SeqCst);
            }
        }

        /// Wait until `count` reaches `n`, failing after 10 s of paused time.
        ///
        /// Each sleep lets every ready task run. A `yield_now` loop would keep
        /// the paused clock from advancing, so a count that never arrived
        /// would hang the test instead of failing it.
        async fn wait_for(count: &AtomicUsize, n: usize) {
            within(Duration::from_secs(10), async {
                while count.load(Ordering::SeqCst) < n {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await;
        }

        /// Check that `error` is a timeout of `operation` after `LIMIT`.
        fn assert_limit_timeout(error: &Error, operation: &str) {
            match error {
                Error::Timeout {
                    operation: op,
                    duration,
                } => {
                    assert_eq!(op, operation);
                    assert_eq!(*duration, LIMIT);
                }
                other => panic!("expected a '{operation}' timeout, got {other:?}"),
            }
        }

        #[tokio::test(start_paused = true)]
        async fn unsubscribe_stops_callbacks() {
            within(Duration::from_secs(600), async {
                let fake = FakeGatt::new();
                let tasks = NotificationTasks::default();
                let count = Arc::new(AtomicUsize::new(0));
                tasks
                    .subscribe(&fake, &characteristic(C1), LIMIT, counter(&count))
                    .await
                    .expect("subscribe");
                fake.emit(C1, &[1]);
                wait_for(&count, 1).await;

                tasks
                    .unsubscribe(&fake, &characteristic(C1), LIMIT)
                    .await
                    .expect("unsubscribe");
                fake.emit(C1, &[2]);
                for _ in 0..10 {
                    tokio::task::yield_now().await;
                }

                assert_eq!(
                    count.load(Ordering::SeqCst),
                    1,
                    "the callback ran after unsubscribe returned"
                );
                assert_eq!(fake.open_streams(), 0, "the task outlived unsubscribe");
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn unsubscribe_stops_the_task_even_when_the_device_call_fails() {
            within(Duration::from_secs(600), async {
                let fake = FakeGatt::new();
                let tasks = NotificationTasks::default();
                let count = Arc::new(AtomicUsize::new(0));
                tasks
                    .subscribe(&fake, &characteristic(C1), LIMIT, counter(&count))
                    .await
                    .expect("subscribe");
                fake.script_unsubscribe([Outcome::Fail]);

                let error = tasks
                    .unsubscribe(&fake, &characteristic(C1), LIMIT)
                    .await
                    .expect_err("the device call fails");

                assert!(matches!(error, Error::Bluetooth(_)), "got {error:?}");
                assert_eq!(
                    fake.open_streams(),
                    0,
                    "the task outlived a failed unsubscribe"
                );
                assert_eq!(tasks.task_count(), 0);
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn resubscribing_replaces_the_previous_callback() {
            within(Duration::from_secs(600), async {
                let fake = FakeGatt::new();
                let tasks = NotificationTasks::default();
                let replaced = Arc::new(AtomicUsize::new(0));
                let current = Arc::new(AtomicUsize::new(0));
                tasks
                    .subscribe(&fake, &characteristic(C1), LIMIT, counter(&replaced))
                    .await
                    .expect("first subscribe");
                tasks
                    .subscribe(&fake, &characteristic(C1), LIMIT, counter(&current))
                    .await
                    .expect("second subscribe");

                fake.emit(C1, &[1]);
                wait_for(&current, 1).await;

                assert_eq!(
                    replaced.load(Ordering::SeqCst),
                    0,
                    "the replaced callback still ran"
                );
                assert_eq!(
                    fake.open_streams(),
                    1,
                    "the replaced task still holds its stream"
                );
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn abort_all_closes_every_stream() {
            within(Duration::from_secs(600), async {
                let fake = FakeGatt::new();
                let tasks = NotificationTasks::default();
                let count = Arc::new(AtomicUsize::new(0));
                tasks
                    .subscribe(&fake, &characteristic(C1), LIMIT, counter(&count))
                    .await
                    .expect("subscribe C1");
                tasks
                    .subscribe(&fake, &characteristic(C2), LIMIT, counter(&count))
                    .await
                    .expect("subscribe C2");
                assert_eq!(fake.open_streams(), 2);

                tasks.abort_all();

                assert_eq!(tasks.task_count(), 0);
                // `abort` only schedules the cancellation; the runtime then
                // drops each task's future, and with it its stream.
                within(Duration::from_secs(10), async {
                    while fake.open_streams() > 0 {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await;
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn subscribe_gives_up_after_limit_and_leaves_no_task() {
            within(Duration::from_secs(600), async {
                let fake = FakeGatt::new();
                let tasks = NotificationTasks::default();
                let count = Arc::new(AtomicUsize::new(0));
                fake.script_subscribe([Outcome::Hang]);
                let start = Instant::now();

                let error = tasks
                    .subscribe(&fake, &characteristic(C1), LIMIT, counter(&count))
                    .await
                    .expect_err("the CCCD write never answers");

                assert_limit_timeout(&error, "subscribe to notifications");
                assert_eq!(start.elapsed(), LIMIT);
                assert_eq!(tasks.task_count(), 0);
                assert_eq!(
                    fake.open_streams(),
                    0,
                    "the failed subscribe left its stream open"
                );
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn unsubscribe_gives_up_after_limit() {
            within(Duration::from_secs(600), async {
                let fake = FakeGatt::new();
                let tasks = NotificationTasks::default();
                let count = Arc::new(AtomicUsize::new(0));
                tasks
                    .subscribe(&fake, &characteristic(C1), LIMIT, counter(&count))
                    .await
                    .expect("subscribe");
                fake.script_unsubscribe([Outcome::Hang]);
                let start = Instant::now();

                let error = tasks
                    .unsubscribe(&fake, &characteristic(C1), LIMIT)
                    .await
                    .expect_err("the CCCD write never answers");

                assert_limit_timeout(&error, "unsubscribe from notifications");
                assert_eq!(start.elapsed(), LIMIT);
                assert_eq!(tasks.task_count(), 0);
                // The CCCD write was sent.
                assert_eq!(fake.calls().last(), Some(&Call::Unsubscribe(C1)));
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn unsubscribe_gives_up_waiting_for_a_task_nobody_runs() {
            // A runtime that nothing drives, like a current-thread runtime
            // between two `block_on` calls. Dropping a runtime inside another
            // one panics, so it lives outside `within` and is shut down at
            // the end.
            let idle = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .expect("runtime");
            within(Duration::from_secs(600), async {
                let fake = FakeGatt::new();
                let tasks = NotificationTasks::default();
                {
                    // With `idle` entered, `tokio::spawn` puts the task there.
                    // The fake answers at once, so one poll subscribes.
                    let _idle = idle.enter();
                    tasks
                        .subscribe(&fake, &characteristic(C1), LIMIT, |_: &[u8]| {})
                        .now_or_never()
                        .expect("the fake answers at once")
                        .expect("subscribe");
                }
                let start = Instant::now();

                tasks
                    .unsubscribe(&fake, &characteristic(C1), LIMIT)
                    .await
                    .expect("unsubscribe");

                assert_eq!(
                    start.elapsed(),
                    LIMIT,
                    "unsubscribe did not wait for the aborted task"
                );
                assert_eq!(tasks.task_count(), 0);
                // The CCCD write was sent once the wait gave up.
                assert_eq!(fake.calls().last(), Some(&Call::Unsubscribe(C1)));
            })
            .await;
            idle.shutdown_background();
        }

        #[tokio::test(start_paused = true)]
        async fn stream_is_open_before_the_cccd_write() {
            within(Duration::from_secs(600), async {
                let fake = FakeGatt::new();
                let tasks = NotificationTasks::default();
                let count = Arc::new(AtomicUsize::new(0));
                fake.emit_on_subscribe(&[7]);

                tasks
                    .subscribe(&fake, &characteristic(C1), LIMIT, counter(&count))
                    .await
                    .expect("subscribe");

                // The device notified during the CCCD write; only a stream
                // opened before the write can have received it.
                wait_for(&count, 1).await;
            })
            .await;
        }

        /// Opening the notification stream is limited too, and a stream that
        /// never opens leaves the device's notifications off.
        #[tokio::test(start_paused = true)]
        async fn opening_the_stream_gives_up_after_limit_and_writes_no_cccd() {
            within(Duration::from_secs(600), async {
                let fake = FakeGatt::new();
                let tasks = NotificationTasks::default();
                let count = Arc::new(AtomicUsize::new(0));
                fake.script_notifications([Outcome::Hang]);
                let start = Instant::now();

                let error = tasks
                    .subscribe(&fake, &characteristic(C1), LIMIT, counter(&count))
                    .await
                    .expect_err("the stream never opens");

                assert_limit_timeout(&error, "open notification stream");
                assert_eq!(start.elapsed(), LIMIT);
                assert_eq!(tasks.task_count(), 0);
                let calls = fake.calls();
                assert!(
                    !calls.contains(&Call::Subscribe(C1)),
                    "notifications were enabled with no stream to read them: {calls:?}"
                );
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn a_failed_resubscribe_keeps_the_previous_callback() {
            within(Duration::from_secs(600), async {
                let fake = FakeGatt::new();
                let tasks = NotificationTasks::default();
                let previous = Arc::new(AtomicUsize::new(0));
                let rejected = Arc::new(AtomicUsize::new(0));
                tasks
                    .subscribe(&fake, &characteristic(C1), LIMIT, counter(&previous))
                    .await
                    .expect("first subscribe");
                fake.script_subscribe([Outcome::Fail]);

                let error = tasks
                    .subscribe(&fake, &characteristic(C1), LIMIT, counter(&rejected))
                    .await
                    .expect_err("the CCCD write fails");
                assert!(matches!(error, Error::Bluetooth(_)), "got {error:?}");

                fake.emit(C1, &[1]);
                wait_for(&previous, 1).await;
                assert_eq!(
                    rejected.load(Ordering::SeqCst),
                    0,
                    "the callback of the failed subscribe ran"
                );
                assert_eq!(tasks.task_count(), 1);
                assert_eq!(
                    fake.open_streams(),
                    1,
                    "the failed subscribe left its stream open"
                );
            })
            .await;
        }

        /// `unsubscribe` stops the callback before it makes the device call,
        /// so no callback runs while that call hangs.
        #[tokio::test(start_paused = true)]
        async fn unsubscribe_stops_the_callback_before_the_device_call() {
            within(Duration::from_secs(600), async {
                let fake = FakeGatt::new();
                let tasks = NotificationTasks::default();
                let count = Arc::new(AtomicUsize::new(0));
                tasks
                    .subscribe(&fake, &characteristic(C1), LIMIT, counter(&count))
                    .await
                    .expect("subscribe");
                fake.emit(C1, &[1]);
                wait_for(&count, 1).await;
                fake.script_unsubscribe([Outcome::Hang]);
                let c1 = characteristic(C1);

                // The CCCD write hangs until `LIMIT`; the device notifies 1 s
                // into it.
                let notify_during_the_call = async {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    assert_eq!(
                        fake.calls().last(),
                        Some(&Call::Unsubscribe(C1)),
                        "the device call should be under way"
                    );
                    fake.emit(C1, &[2]);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    count.load(Ordering::SeqCst)
                };
                let (result, calls_during) =
                    tokio::join!(tasks.unsubscribe(&fake, &c1, LIMIT), notify_during_the_call);

                assert_eq!(
                    calls_during, 1,
                    "the callback ran while the device call hung"
                );
                let error = result.expect_err("the CCCD write never answers");
                assert_limit_timeout(&error, "unsubscribe from notifications");
            })
            .await;
        }
    }
}

/// BR-25: bluez-async's `connect` gives BlueZ 5 s to resolve the services
/// after BlueZ answers `Device1.Connect`, which on an LE link it does once the
/// ATT channel is up, a few seconds after the link. A sensor with a large GATT
/// table and a slow link, such as an Aranet4 that BlueZ hasn't cached yet,
/// needs longer, and BlueZ keeps discovering on the live link after
/// bluez-async has given up. When the link itself is slow to come up, the
/// connect's own time limit can run out first, again with the link up and
/// BlueZ still setting up the ATT channel or discovering on it.
#[cfg(test)]
mod bluez_discovery_tests {
    use std::collections::BTreeSet;
    use std::time::Duration;

    use tokio::runtime::Handle;
    use tokio::time::Instant;

    use super::connect;
    use super::fake::{BLUEZ_ASYNC_WAIT, Call, FakeGatt, Outcome};
    use super::tests::assert_timeout;
    use crate::device::ConnectionConfig;
    use crate::error::{Error, Result};
    use crate::test_support::within;

    /// The least time a connect waits for BlueZ once bluez-async has given up
    /// (`BLUEZ_DISCOVERY_MIN_WAIT`).
    const MIN_WAIT: Duration = Duration::from_secs(20);

    /// How long a wait after a connect timeout pauses before it asks again
    /// when BlueZ answered `In Progress` (`BLUEZ_IN_PROGRESS_PAUSE`).
    const IN_PROGRESS_PAUSE: Duration = Duration::from_secs(1);

    /// Connect `fake` with `config` and the test's runtime for cleanup, and
    /// disarm the link if the connect succeeds.
    pub(super) async fn connect_with(fake: &FakeGatt, config: &ConnectionConfig) -> Result<()> {
        let open = connect(fake, config, &Handle::current()).await?;
        open.pending.disarm();
        Ok(())
    }

    /// Check that `result` is a "discover services" timeout after `limit`.
    fn assert_discovery_timeout(result: Result<()>, limit: Duration) {
        match result {
            Err(Error::Timeout {
                operation,
                duration,
            }) => {
                assert_eq!(operation, "discover services");
                assert_eq!(duration, limit);
            }
            other => panic!("expected a 'discover services' timeout, got {other:?}"),
        }
    }

    /// Check that `result` is BlueZ's `In Progress` refusal.
    fn assert_in_progress(result: Result<()>) {
        match result {
            Err(Error::Bluetooth(btleplug::Error::Other(e))) => {
                assert_eq!(e.to_string(), "In Progress");
            }
            other => panic!("expected BlueZ's 'In Progress', got {other:?}"),
        }
    }

    /// A connect that bluez-async gives up on while BlueZ is still discovering
    /// asks again until BlueZ has resolved the services, then discovers them.
    #[tokio::test(start_paused = true)]
    async fn waits_while_bluez_is_still_discovering() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.script_connect([
                Outcome::DiscoveryTimedOut,
                Outcome::DiscoveryTimedOut,
                Outcome::Ok,
            ]);
            let start = Instant::now();

            connect_with(&fake, &ConnectionConfig::default())
                .await
                .expect("the connect should wait for BlueZ's service discovery");

            assert_eq!(start.elapsed(), 2 * BLUEZ_ASYNC_WAIT);
            assert_eq!(
                fake.calls(),
                [Call::Connect, Call::Connect, Call::Connect, Call::Discover]
            );
        })
        .await;
    }

    /// A discovery that never finishes fails as a "discover services" timeout
    /// `MIN_WAIT` after bluez-async gave up, even with a shorter
    /// `discovery_timeout`, and disconnects.
    #[tokio::test(start_paused = true)]
    async fn gives_up_after_the_minimum_wait_and_disconnects() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.script_connect(std::iter::repeat_with(|| Outcome::DiscoveryTimedOut).take(10));
            let config = ConnectionConfig::default().discovery_timeout(Duration::from_secs(5));
            let start = Instant::now();

            let result = connect_with(&fake, &config).await;

            assert_discovery_timeout(result, MIN_WAIT);
            assert_eq!(start.elapsed(), BLUEZ_ASYNC_WAIT + MIN_WAIT);
            let calls = fake.calls();
            assert_eq!(calls.last(), Some(&Call::Disconnect), "calls: {calls:?}");
            assert!(!calls.contains(&Call::Discover), "calls: {calls:?}");
            let connects = calls.iter().filter(|call| **call == Call::Connect).count();
            assert!(connects >= 5, "calls: {calls:?}");
        })
        .await;
    }

    /// A `discovery_timeout` longer than `MIN_WAIT` lengthens the wait.
    #[tokio::test(start_paused = true)]
    async fn a_longer_discovery_timeout_waits_longer() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.script_connect(std::iter::repeat_with(|| Outcome::DiscoveryTimedOut).take(10));
            let config = ConnectionConfig::challenging_environment();
            let start = Instant::now();

            let result = connect_with(&fake, &config).await;

            assert_discovery_timeout(result, Duration::from_secs(30));
            assert_eq!(start.elapsed(), BLUEZ_ASYNC_WAIT + Duration::from_secs(30));
            assert_eq!(fake.calls().last(), Some(&Call::Disconnect));
        })
        .await;
    }

    /// Any other error ends the wait at once and fails the connect: here the
    /// link dropped and BlueZ's new connection attempt failed.
    #[tokio::test(start_paused = true)]
    async fn another_error_ends_the_wait() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.script_connect([Outcome::DiscoveryTimedOut, Outcome::Fail]);
            let start = Instant::now();

            let result = connect_with(&fake, &ConnectionConfig::default()).await;

            assert_eq!(
                fake.calls(),
                [Call::Connect, Call::Connect, Call::Disconnect]
            );
            assert!(
                matches!(
                    result,
                    Err(Error::Bluetooth(btleplug::Error::RuntimeError(_)))
                ),
                "got {result:?}"
            );
            assert_eq!(start.elapsed(), BLUEZ_ASYNC_WAIT);
        })
        .await;
    }

    /// The reconnect of the zero-services retry waits for BlueZ too.
    #[tokio::test(start_paused = true)]
    async fn the_zero_services_retry_waits_too() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.service_rounds([BTreeSet::new()]);
            fake.script_connect([Outcome::Ok, Outcome::DiscoveryTimedOut, Outcome::Ok]);
            let start = Instant::now();

            connect_with(&fake, &ConnectionConfig::default())
                .await
                .expect("the retry should wait for BlueZ's service discovery");

            assert_eq!(start.elapsed(), Duration::from_secs(2) + BLUEZ_ASYNC_WAIT);
            assert_eq!(
                fake.calls(),
                [
                    Call::Connect,
                    Call::Discover,
                    Call::Disconnect,
                    Call::Connect,
                    Call::Connect,
                    Call::Discover,
                ]
            );
        })
        .await;
    }

    /// On BlueZ, a connect that runs out of time after the link came up, too
    /// late for bluez-async's 5 s wait for the services to run out first,
    /// waits for BlueZ's service discovery in the same way. The wait's limit
    /// starts when the connect's ends and also covers the connection-state
    /// query, which answers at once here, so the wait and the discovery get
    /// all of it.
    #[tokio::test(start_paused = true)]
    async fn a_connect_timeout_with_the_link_up_waits_on_bluez() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(true);
            fake.script_connect([
                Outcome::Hang,
                Outcome::DiscoveryTimedOut,
                Outcome::DiscoveryTimedOut,
                Outcome::DiscoveryTimedOut,
                Outcome::After(Duration::from_secs(4)),
            ]);
            let config = ConnectionConfig::default();
            let start = Instant::now();

            connect_with(&fake, &config)
                .await
                .expect("the connect should wait for BlueZ's service discovery");

            // BlueZ resolves the services 19 s into the wait: 1 s inside it.
            assert_eq!(
                start.elapsed(),
                config.connection_timeout + MIN_WAIT - Duration::from_secs(1)
            );
            assert_eq!(
                fake.calls(),
                [
                    Call::Connect,
                    Call::IsConnected,
                    Call::Connect,
                    Call::Connect,
                    Call::Connect,
                    Call::Connect,
                    Call::Discover,
                ]
            );
        })
        .await;
    }

    /// On BlueZ, when that wait never ends, the connect fails as a "discover
    /// services" timeout after the wait's limit, and disconnects.
    #[tokio::test(start_paused = true)]
    async fn a_connect_timeout_with_the_link_up_gives_up_after_the_wait() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(true);
            fake.script_connect([Outcome::Hang]);
            fake.script_connect(std::iter::repeat_with(|| Outcome::DiscoveryTimedOut).take(10));
            let config = ConnectionConfig::default();
            let start = Instant::now();

            let result = connect_with(&fake, &config).await;

            assert_discovery_timeout(result, MIN_WAIT);
            assert_eq!(start.elapsed(), config.connection_timeout + MIN_WAIT);
            let calls = fake.calls();
            assert_eq!(
                calls[..3],
                [Call::Connect, Call::IsConnected, Call::Connect],
                "calls: {calls:?}"
            );
            assert_eq!(calls.last(), Some(&Call::Disconnect), "calls: {calls:?}");
            assert!(!calls.contains(&Call::Discover), "calls: {calls:?}");
        })
        .await;
    }

    /// On BlueZ, a connect that runs out of time with the link down fails as
    /// before, with the "connect to device" timeout, and disconnects without
    /// waiting.
    #[tokio::test(start_paused = true)]
    async fn a_connect_timeout_with_the_link_down_fails_on_bluez() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(true);
            fake.set_connected(false);
            fake.script_connect([Outcome::Hang]);
            let config = ConnectionConfig::default();
            let start = Instant::now();

            let error = connect_with(&fake, &config)
                .await
                .expect_err("the connect should fail");

            assert_timeout(&error, "connect to device", config.connection_timeout);
            assert_eq!(start.elapsed(), config.connection_timeout);
            assert_eq!(
                fake.calls(),
                [Call::Connect, Call::IsConnected, Call::Disconnect]
            );
        })
        .await;
    }

    /// Elsewhere (macOS, Windows) a connect that runs out of time fails as
    /// before even with the link up: only bluez-async's `connect` also waits
    /// for the services, so there the connect itself didn't finish.
    #[tokio::test(start_paused = true)]
    async fn a_connect_timeout_with_the_link_up_fails_elsewhere() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(false);
            // The fake reports the link up (its default), which must not
            // matter here.
            fake.script_connect([Outcome::Hang]);
            let config = ConnectionConfig::default();
            let start = Instant::now();

            let error = connect_with(&fake, &config)
                .await
                .expect_err("the connect should fail");

            assert_timeout(&error, "connect to device", config.connection_timeout);
            assert_eq!(start.elapsed(), config.connection_timeout);
            assert_eq!(fake.calls(), [Call::Connect, Call::Disconnect]);
        })
        .await;
    }

    /// On BlueZ, the zero-services retry's reconnect that runs out of time
    /// with the link up waits too, and running out of the wait is reported as
    /// "rediscover services" after the wait's limit.
    #[tokio::test(start_paused = true)]
    async fn the_zero_services_retry_waits_after_a_connect_timeout_too() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(true);
            fake.service_rounds([BTreeSet::new()]);
            fake.script_connect([Outcome::Ok, Outcome::Hang]);
            fake.script_connect(std::iter::repeat_with(|| Outcome::DiscoveryTimedOut).take(10));
            let config = ConnectionConfig::default();
            let start = Instant::now();

            let error = connect_with(&fake, &config)
                .await
                .expect_err("the connect should fail");

            assert_timeout(&error, "rediscover services", MIN_WAIT);
            assert_eq!(
                start.elapsed(),
                Duration::from_secs(2) + config.connection_timeout + MIN_WAIT
            );
            let calls = fake.calls();
            assert_eq!(
                calls[..6],
                [
                    Call::Connect,
                    Call::Discover,
                    Call::Disconnect,
                    Call::Connect,
                    Call::IsConnected,
                    Call::Connect,
                ],
                "calls: {calls:?}"
            );
            assert_eq!(calls.last(), Some(&Call::Disconnect), "calls: {calls:?}");
            let discoveries = calls.iter().filter(|call| **call == Call::Discover).count();
            assert_eq!(discoveries, 1, "calls: {calls:?}");
        })
        .await;
    }

    /// On BlueZ, the connection-state query after a connect timeout counts
    /// against the wait's limit: a slow answer shortens the wait instead of
    /// lengthening the connect.
    #[tokio::test(start_paused = true)]
    async fn a_slow_connection_state_query_counts_against_the_wait() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(true);
            fake.script_is_connected([Outcome::After(Duration::from_secs(2))]);
            fake.script_connect([Outcome::Hang]);
            fake.script_connect(std::iter::repeat_with(|| Outcome::DiscoveryTimedOut).take(10));
            let config = ConnectionConfig::default();
            let start = Instant::now();

            let result = connect_with(&fake, &config).await;

            assert_discovery_timeout(result, MIN_WAIT);
            assert_eq!(start.elapsed(), config.connection_timeout + MIN_WAIT);
            let calls = fake.calls();
            assert_eq!(
                calls[..3],
                [Call::Connect, Call::IsConnected, Call::Connect],
                "calls: {calls:?}"
            );
            assert_eq!(calls.last(), Some(&Call::Disconnect), "calls: {calls:?}");
        })
        .await;
    }

    /// On BlueZ, a connection-state query that never answers ends within the
    /// wait's limit too, even when `validation_timeout` is longer, so the
    /// connect and the query together never take longer than the connect's
    /// and the wait's limits. The link isn't known to be up, so the connect
    /// fails with its own timeout and disconnects.
    #[tokio::test(start_paused = true)]
    async fn a_hanging_connection_state_query_ends_within_the_wait() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(true);
            fake.script_is_connected([Outcome::Hang]);
            fake.script_connect([Outcome::Hang]);
            let config = ConnectionConfig::default().validation_timeout(Duration::from_secs(60));
            let start = Instant::now();

            let error = connect_with(&fake, &config)
                .await
                .expect_err("the connect should fail");

            assert_timeout(&error, "connect to device", config.connection_timeout);
            assert_eq!(start.elapsed(), config.connection_timeout + MIN_WAIT);
            assert_eq!(
                fake.calls(),
                [Call::Connect, Call::IsConnected, Call::Disconnect]
            );
        })
        .await;
    }

    /// On BlueZ, after a connect that ran out of time with the link up, BlueZ
    /// refuses the wait's first re-asks with `In Progress` until it has
    /// answered the timed-out `Device1.Connect`, which it does once the ATT
    /// channel is up. The wait asks again after a pause each time, then waits
    /// for the discovery as before.
    #[tokio::test(start_paused = true)]
    async fn a_connect_timeout_waits_while_bluez_is_still_connecting() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(true);
            fake.script_connect([
                Outcome::Hang,
                Outcome::InProgress,
                Outcome::InProgress,
                Outcome::InProgress,
                Outcome::DiscoveryTimedOut,
                Outcome::Ok,
            ]);
            let config = ConnectionConfig::default();
            let start = Instant::now();

            connect_with(&fake, &config)
                .await
                .expect("the connect should wait while BlueZ is still connecting");

            assert_eq!(
                start.elapsed(),
                config.connection_timeout + 3 * IN_PROGRESS_PAUSE + BLUEZ_ASYNC_WAIT
            );
            assert!(start.elapsed() <= config.connection_timeout + MIN_WAIT);
            assert_eq!(
                fake.calls(),
                [
                    Call::Connect,
                    Call::IsConnected,
                    Call::Connect,
                    Call::Connect,
                    Call::Connect,
                    Call::Connect,
                    Call::Connect,
                    Call::Discover,
                ]
            );
        })
        .await;
    }

    /// On BlueZ, when BlueZ goes on refusing with `In Progress` after a
    /// connect timeout, the wait gives up after its limit with a "discover
    /// services" timeout, and the connect disconnects.
    #[tokio::test(start_paused = true)]
    async fn a_connect_timeout_gives_up_while_bluez_stays_in_progress() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(true);
            fake.script_connect([Outcome::Hang]);
            fake.script_connect(std::iter::repeat_with(|| Outcome::InProgress).take(100));
            let config = ConnectionConfig::default();
            let start = Instant::now();

            let result = connect_with(&fake, &config).await;

            assert_discovery_timeout(result, MIN_WAIT);
            assert_eq!(start.elapsed(), config.connection_timeout + MIN_WAIT);
            let calls = fake.calls();
            assert_eq!(
                calls[..4],
                [
                    Call::Connect,
                    Call::IsConnected,
                    Call::Connect,
                    Call::Connect
                ],
                "calls: {calls:?}"
            );
            assert_eq!(calls.last(), Some(&Call::Disconnect), "calls: {calls:?}");
            assert!(!calls.contains(&Call::Discover), "calls: {calls:?}");
        })
        .await;
    }

    /// After bluez-async's own discovery timeout, BlueZ has answered
    /// `Device1.Connect`, so an `In Progress` comes from another connect to
    /// the device or a pairing of it, and ends the wait at once, as before.
    #[tokio::test(start_paused = true)]
    async fn in_progress_ends_the_wait_after_bluez_async_gave_up() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(true);
            fake.script_connect([Outcome::DiscoveryTimedOut, Outcome::InProgress]);
            let start = Instant::now();

            let result = connect_with(&fake, &ConnectionConfig::default()).await;

            assert_in_progress(result);
            assert_eq!(start.elapsed(), BLUEZ_ASYNC_WAIT);
            assert_eq!(
                fake.calls(),
                [Call::Connect, Call::Connect, Call::Disconnect]
            );
        })
        .await;
    }

    /// A first connect that BlueZ refuses with `In Progress` fails at once and
    /// disconnects, as before: only a wait after a connect timeout asks again.
    #[tokio::test(start_paused = true)]
    async fn a_first_connect_refused_in_progress_fails_at_once() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(true);
            fake.script_connect([Outcome::InProgress]);
            let start = Instant::now();

            let result = connect_with(&fake, &ConnectionConfig::default()).await;

            assert_in_progress(result);
            assert_eq!(start.elapsed(), Duration::ZERO);
            assert_eq!(fake.calls(), [Call::Connect, Call::Disconnect]);
        })
        .await;
    }
}

#[cfg(test)]
mod pairing_tests {
    use std::time::Duration;

    use tokio::runtime::Handle;
    use tokio::time::Instant;

    use super::bluez_discovery_tests::connect_with;
    use super::connect;
    use super::fake::{Call, FakeGatt, Outcome};
    use super::tests::assert_timeout;
    use crate::device::ConnectionConfig;
    use crate::test_support::within;

    /// The pairing budget at default settings: `connection_timeout` (15 s)
    /// plus `bluez_discovery_limit` (20 s).
    const DEFAULT_BUDGET: Duration = Duration::from_secs(35);

    /// The pairing step runs first and inside the guard: a connect that fails
    /// after it still disconnects, because `Pair` may have connected the sensor.
    #[tokio::test(start_paused = true)]
    async fn pairing_runs_before_connect_inside_the_guard() {
        within(Duration::from_secs(600), async {
            let config = ConnectionConfig::default();
            let runtime = Handle::current();

            let link = FakeGatt::new();
            link.script_pair([Outcome::Ok]);
            let open = connect(&link, &config, &runtime)
                .await
                .expect("the connect should succeed after pairing");
            open.pending.disarm();
            assert_eq!(
                link.calls(),
                [Call::Pair(DEFAULT_BUDGET), Call::Connect, Call::Discover]
            );

            let refused = FakeGatt::new();
            refused.script_pair([Outcome::Ok]);
            refused.script_connect([Outcome::Fail]);
            let result = connect(&refused, &config, &runtime).await;
            assert!(result.is_err(), "a refused connect must fail");
            assert_eq!(
                refused.calls(),
                [Call::Pair(DEFAULT_BUDGET), Call::Connect, Call::Disconnect]
            );
        })
        .await;
    }

    /// A pairing step that never returns is abandoned after its budget,
    /// `connection_timeout` plus the wait for BlueZ's service discovery
    /// (`bluez_discovery_limit`), and `PAIRING_GRACE`; the connect goes ahead.
    #[tokio::test(start_paused = true)]
    async fn a_pairing_step_that_never_returns_is_abandoned() {
        within(Duration::from_secs(600), async {
            let link = FakeGatt::new();
            link.script_pair([Outcome::Hang]);
            let start = Instant::now();

            let open = connect(&link, &ConnectionConfig::default(), &Handle::current())
                .await
                .expect("the connect should go ahead without pairing");
            open.pending.disarm();

            // connection_timeout (15 s) + bluez_discovery_limit (20 s) + 5 s of grace.
            assert_eq!(start.elapsed(), Duration::from_secs(40));
            assert_eq!(
                link.calls(),
                [Call::Pair(DEFAULT_BUDGET), Call::Connect, Call::Discover]
            );
        })
        .await;
    }

    /// A caller that drops the connect while it is still pairing gets the
    /// sensor disconnected in the background.
    #[tokio::test(start_paused = true)]
    async fn cancelled_pairing_still_disconnects() {
        within(Duration::from_secs(600), async {
            let link = FakeGatt::new();
            link.script_pair([Outcome::Hang]);
            let config = ConnectionConfig::default();
            let runtime = Handle::current();

            let attempt =
                tokio::time::timeout(Duration::from_secs(1), connect(&link, &config, &runtime))
                    .await;
            assert!(
                attempt.is_err(),
                "the connect should still be pairing after 1 s"
            );

            within(Duration::from_secs(10), link.disconnected().notified()).await;
            assert_eq!(link.calls(), [Call::Pair(DEFAULT_BUDGET), Call::Disconnect]);
        })
        .await;
    }

    /// `Duration::MAX` is how a caller says "no limit". A connect must not
    /// panic on it: not when the pairing budget adds two of them, and not on
    /// BlueZ when the wait for BlueZ's service discovery starts with a
    /// deadline `discovery_timeout` away, after bluez-async gave up waiting
    /// for the services or after a connect timeout.
    #[tokio::test(start_paused = true)]
    async fn connect_with_unlimited_timeouts_does_not_panic() {
        within(Duration::from_secs(600), async {
            let unlimited = ConnectionConfig::default()
                .connection_timeout(Duration::MAX)
                .discovery_timeout(Duration::MAX);

            let link = FakeGatt::new();
            link.script_pair([Outcome::Ok]);
            connect_with(&link, &unlimited)
                .await
                .expect("the connect should succeed");
            assert_eq!(
                link.calls(),
                [Call::Pair(Duration::MAX), Call::Connect, Call::Discover]
            );

            // On BlueZ, bluez-async gives up waiting for the services, so the
            // connect waits for BlueZ to finish.
            let bluez = FakeGatt::new();
            bluez.set_bluez(true);
            bluez.script_pair([Outcome::Ok]);
            bluez.script_connect([Outcome::DiscoveryTimedOut, Outcome::Ok]);
            connect_with(&bluez, &unlimited)
                .await
                .expect("the connect should wait for BlueZ's service discovery");
            assert_eq!(
                bluez.calls(),
                [
                    Call::Pair(Duration::MAX),
                    Call::Connect,
                    Call::Connect,
                    Call::Discover,
                ]
            );

            // On BlueZ, a connect that runs out of its finite time starts the
            // wait before it asks whether the link is up. With the link up it
            // waits for BlueZ; with the link down it fails with its own
            // timeout, as a sensor out of range does.
            let finite_connect = ConnectionConfig::default().discovery_timeout(Duration::MAX);
            let up = FakeGatt::new();
            up.set_bluez(true);
            up.script_connect([Outcome::Hang, Outcome::Ok]);
            connect_with(&up, &finite_connect)
                .await
                .expect("the connect should wait for BlueZ's service discovery");
            assert_eq!(
                up.calls(),
                [
                    Call::Connect,
                    Call::IsConnected,
                    Call::Connect,
                    Call::Discover,
                ]
            );

            let down = FakeGatt::new();
            down.set_bluez(true);
            down.set_connected(false);
            down.script_connect([Outcome::Hang]);
            let error = connect_with(&down, &finite_connect)
                .await
                .expect_err("a connect timeout with the link down should fail");
            assert_timeout(
                &error,
                "connect to device",
                finite_connect.connection_timeout,
            );
            assert_eq!(
                down.calls(),
                [Call::Connect, Call::IsConnected, Call::Disconnect]
            );
        })
        .await;
    }

    /// The pairing step's budget is `connection_timeout` plus
    /// `bluez_discovery_limit`, whose 20 s floor applies here too.
    #[tokio::test(start_paused = true)]
    async fn pairing_gets_the_connect_and_discovery_wait_budget() {
        within(Duration::from_secs(600), async {
            let cases = [
                (ConnectionConfig::default(), DEFAULT_BUDGET),
                (
                    ConnectionConfig::default()
                        .connection_timeout(Duration::from_secs(30))
                        .discovery_timeout(Duration::from_secs(40)),
                    Duration::from_secs(70),
                ),
                // 8 s to connect, and a 5 s `discovery_timeout` under the floor.
                (ConnectionConfig::fast(), Duration::from_secs(28)),
            ];
            for (config, budget) in cases {
                let link = FakeGatt::new();
                link.script_pair([Outcome::Ok]);

                let open = connect(&link, &config, &Handle::current())
                    .await
                    .expect("the connect should succeed");
                open.pending.disarm();

                assert_eq!(
                    link.calls(),
                    [Call::Pair(budget), Call::Connect, Call::Discover],
                    "{config:?}"
                );
            }
        })
        .await;
    }
}
