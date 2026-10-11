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
    /// waits up to 30 s more for BlueZ to resolve the services (bluez-async
    /// 0.9.0 `src/lib.rs:61`). So a `connect` can run out of time with the
    /// link already up and BlueZ still setting up the ATT channel or
    /// discovering. Under a limit of 30 s or less, such as the 15 s default,
    /// that is how every connect ends whose discovery outlasts the limit.
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
/// the connect future is dropped before it finishes, even during that
/// disconnect (a caller's timeout or `select!`, an aborted task), `Drop` runs
/// the disconnect as a task on `cleanup`.
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
    ///
    /// The link stays armed until the disconnect has run, so if this future
    /// is dropped part-way, `Drop` still disconnects it.
    async fn close(mut self) {
        if let Some(link) = self.link.clone() {
            if let Err(e) = disconnect(&link).await {
                debug!("Disconnect after a failed connect failed: {e}");
            }
            self.link = None;
        }
    }
}

impl<L: GattLink> Drop for PendingLink<L> {
    fn drop(&mut self) {
        // `disarm` takes the link, and `close` clears it once its disconnect
        // has run, so it is still here only when the connect future was
        // dropped before it finished, which includes during that disconnect.
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
/// link, and bluez-async then waits 30 s for the services (bluez-async 0.9.0
/// `src/lib.rs:61`). It waited 5 s before btleplug 0.13.5
/// (<https://github.com/deviceplug/btleplug/issues/496>). So under a
/// `connection_timeout` of 30 s or less, the 15 s default included, a connect
/// whose discovery is unfinished at that limit runs out of time first, and
/// `connect_link` then asks whether the link is up. bluez-async gives up
/// first ("Service discovery timed out") only under a longer limit. Either
/// way the discovery after it first waits for BlueZ to finish
/// (`wait_for_bluez_discovery`), and that connection-state query, the wait
/// and the discovery share one `bluez_discovery_limit` (20 s at defaults),
/// whose end fails the connect with a "discover services" (or "rediscover
/// services") timeout.
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
/// services 30 s after `Device1.Connect` returned, although the link is up
/// and BlueZ is still discovering (`BluetoothError::ServiceDiscoveryTimedOut`,
/// bluez-async 0.9.0 `src/lib.rs:61`, `:90-92`, `:699-727`). btleplug passes
/// it on as `btleplug::Error::Other` (btleplug 0.13.5
/// `src/bluez/adapter.rs:182-186`). No other btleplug backend produces it.
/// aranet-core can't name bluez-async's type (it isn't a dependency), so this
/// matches the message. A connect gets it only under a `connection_timeout`
/// longer than those 30 s; a shorter limit ends the call first.
const BLUEZ_DISCOVERY_TIMED_OUT: &str = "Service discovery timed out";

/// The least time a connect waits for BlueZ to finish discovering the services
/// once the connect has run out of time with the link up, or bluez-async has
/// given up (BR-25). An Aranet4 that BlueZ hasn't cached needs about 30-48 ATT
/// requests at 0.3-0.45 s each: 9-22 s from the moment BlueZ answers
/// `Device1.Connect`, which on an LE link it does once the ATT channel is up,
/// a few seconds after the link.
///
/// Counted from that answer, BlueZ's discovery gets
/// `min(a + 30 s, connection_timeout) + wait - a` in all, where `a` is the
/// time BlueZ took to answer and the 30 s are bluez-async's own wait for the
/// services (bluez-async 0.9.0 `src/lib.rs:61`). At the default limits that
/// is `15 s + 20 s - a`: 33 s when BlueZ answers after 2 s, and little more
/// than this wait alone when it answers just before the connect's limit. If
/// BlueZ hasn't answered by then, the wait also has to cover the rest of the
/// ATT set-up. When bluez-async gives up first, which needs a
/// `connection_timeout` above 30 s, the discovery has already had 30 s.
///
/// bluez-async waited only 5 s until btleplug 0.13.5 moved to bluez-async
/// 0.9.0 (<https://github.com/deviceplug/btleplug/issues/496>), and this wait
/// was added to make up for that. It is still needed: a connect whose limit
/// is 30 s or less ends before bluez-async's wait does, and without this wait
/// it would fail and disconnect in the middle of BlueZ's discovery.
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
    /// answered `Device1.Connect`. bluez-async waits 30 s (bluez-async 0.9.0
    /// `src/lib.rs:61`), so this needs a `connection_timeout` above 30 s.
    DiscoveryTimedOut,
    /// The connect ran out of time with the link up. Under a
    /// `connection_timeout` of 30 s or less this is the reason for every
    /// wait. BlueZ has usually answered that `Device1.Connect` by then, but
    /// it may not have answered yet.
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
        // up to 30 s more for the services (bluez-async 0.9.0
        // `src/lib.rs:61`). So under a `connection_timeout` of 30 s or less,
        // every connect whose discovery is unfinished at the limit runs out
        // of time here, with BlueZ still setting up the ATT channel or
        // discovering on the link. Disconnecting would cancel that, and
        // every retry would start it again.
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
/// `dev_connect`), and bluez-async then waits up to 30 s more for
/// `ServicesResolved` (bluez-async 0.9.0 `src/lib.rs:61`), or returns at once
/// if it is already set. Those 30 s are longer than the default 20 s wait, so
/// at the default limits the wait makes one such call: BlueZ finishes during
/// it, or the caller's deadline ends it. Another call after a "Service
/// discovery timed out" needs a `discovery_timeout` above 30 s.
///
/// If the link has dropped, a `Device1.Connect` made after the drop opens a
/// new one, as the caller's own retry would. A call that is already waiting
/// for the services does not notice a drop (bluez-async 0.9.0
/// `src/lib.rs:711-726`), so at the default limits a link that drops during
/// the wait is not reopened.
///
/// After a connect timeout (`BluezWaitReason::ConnectTimedOut`), BlueZ may not
/// have answered the timed-out `Device1.Connect` yet, and until it does it
/// refuses the next one with `In Progress`. Then the wait asks again after
/// `BLUEZ_IN_PROGRESS_PAUSE`. Under a `connection_timeout` of 30 s or less
/// every wait starts this way, so there an `In Progress` that comes from
/// another connect to the device or a pairing of it is asked again as well,
/// until the caller's deadline. After bluez-async's discovery timeout, which
/// needs a longer `connection_timeout`, BlueZ has answered, so an
/// `In Progress` there comes from another connect to the device or a pairing
/// of it, and ends the wait.
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
    use crate::util::peripheral_name;

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
            // The name, read as every other place in the crate reads it. The
            // test double reports it in both name fields: with `local_name`
            // alone this would fail on Linux and Windows, where
            // `peripheral_name` takes only the advertised name.
            assert_eq!(
                open.properties.as_ref().and_then(peripheral_name),
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

    /// A caller that drops the connect during the disconnect after a failed
    /// step still gets the link disconnected: the guard stays armed until
    /// that disconnect has run, so `Drop` disconnects again in the
    /// background.
    #[tokio::test(start_paused = true)]
    async fn cancelled_cleanup_still_disconnects() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.script_connect([Outcome::Fail]);
            // The cleanup's disconnect takes 2 s, and the caller gives up 1 s
            // into it.
            fake.script_disconnect([Outcome::After(Duration::from_secs(2))]);
            let rt = Handle::current();
            let cfg = ConnectionConfig::default();
            let start = Instant::now();

            let attempt =
                tokio::time::timeout(Duration::from_secs(1), connect(&fake, &cfg, &rt)).await;
            assert!(
                attempt.is_err(),
                "the connect should still be disconnecting after 1 s"
            );

            within(Duration::from_secs(10), fake.disconnected().notified()).await;
            assert_eq!(start.elapsed(), Duration::from_secs(1));
            assert_eq!(
                fake.calls(),
                [Call::Connect, Call::Disconnect, Call::Disconnect]
            );
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
            super::assert_timeout(error, operation, LIMIT);
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

/// BR-25: bluez-async's `connect` gives BlueZ 30 s to resolve the services
/// after BlueZ answers `Device1.Connect`, which on an LE link it does once the
/// ATT channel is up, a few seconds after the link (bluez-async 0.9.0
/// `src/lib.rs:61`). It gave 5 s before btleplug 0.13.5
/// (<https://github.com/deviceplug/btleplug/issues/496>). A connect's own
/// time limit is 15 s by default, so when a sensor with a large GATT table
/// and a slow link, such as an Aranet4 that BlueZ hasn't cached yet, needs
/// longer than that, the connect's limit runs out first, with the link up
/// and BlueZ still setting up the ATT channel or discovering on it. The
/// connect then waits for BlueZ by asking it to connect again. bluez-async
/// gives up before the connect's limit only when that limit is longer than
/// its 30 s; the tests of that path set such a limit (`LONG_CONNECT_LIMIT`,
/// or `ConnectionConfig::challenging_environment()`). Either way BlueZ keeps
/// discovering on the live link.
///
/// The fake's `fake::BLUEZ_ASYNC_WAIT` models bluez-async's wait, and its
/// doc says how a first connect must be scripted.
#[cfg(test)]
mod bluez_discovery_tests {
    use std::collections::BTreeSet;
    use std::time::Duration;

    use tokio::runtime::Handle;
    use tokio::time::Instant;

    use super::connect;
    use super::fake::{BLUEZ_ASYNC_WAIT, BLUEZ_DBUS_TIMEOUT, Call, FakeGatt, Outcome};
    use super::tests::assert_timeout;
    use crate::device::ConnectionConfig;
    use crate::error::{Error, Result};
    use crate::test_support::within;

    /// The least time a connect waits for BlueZ once the connect has run out
    /// of time with the link up, or bluez-async has given up
    /// (`BLUEZ_DISCOVERY_MIN_WAIT`).
    const MIN_WAIT: Duration = Duration::from_secs(20);

    /// How long a wait after a connect timeout pauses before it asks again
    /// when BlueZ answered `In Progress` (`BLUEZ_IN_PROGRESS_PAUSE`).
    const IN_PROGRESS_PAUSE: Duration = Duration::from_secs(1);

    /// A connect limit longer than bluez-async's 30 s wait for the services
    /// (`BLUEZ_ASYNC_WAIT`). Only under such a limit can bluez-async's own
    /// "Service discovery timed out" end a first connect: under the default
    /// 15 s, and under `aranet watch`'s 30 s, the connect's limit ends it
    /// first. The tests of the paths that start with the library's timeout
    /// use this limit.
    const LONG_CONNECT_LIMIT: Duration = Duration::from_secs(45);

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

    /// Under a connect limit longer than bluez-async's wait, a connect that
    /// bluez-async gives up on while BlueZ is still discovering asks again
    /// until BlueZ has resolved the services, then discovers them. The second
    /// "Service discovery timed out" here comes from a re-ask inside the
    /// wait, which needs a `discovery_timeout` above bluez-async's 30 s too:
    /// at the default limits the wait's own limit ends the first re-ask
    /// (`a_connect_timeout_with_the_link_up_gives_up_after_the_wait`).
    #[tokio::test(start_paused = true)]
    async fn waits_while_bluez_is_still_discovering() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.script_connect([
                Outcome::DiscoveryTimedOut,
                Outcome::DiscoveryTimedOut,
                Outcome::Ok,
            ]);
            let config = ConnectionConfig::default()
                .connection_timeout(LONG_CONNECT_LIMIT)
                .discovery_timeout(Duration::from_secs(70));
            let start = Instant::now();

            connect_with(&fake, &config)
                .await
                .expect("the connect should wait for BlueZ's service discovery");

            // The library gives up at 30 s. The wait then lasts until 100 s
            // (30 + 70): the first re-ask gives up at 60 s (30 + 30) and the
            // second succeeds at once.
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
    /// `discovery_timeout`, and disconnects. The wait makes one re-ask:
    /// bluez-async waits 30 s on it, longer than the 20 s the wait has.
    #[tokio::test(start_paused = true)]
    async fn gives_up_after_the_minimum_wait_and_disconnects() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.script_connect(std::iter::repeat_with(|| Outcome::DiscoveryTimedOut).take(10));
            let config = ConnectionConfig::default()
                .connection_timeout(LONG_CONNECT_LIMIT)
                .discovery_timeout(Duration::from_secs(5));
            let start = Instant::now();

            let result = connect_with(&fake, &config).await;

            assert_discovery_timeout(result, MIN_WAIT);
            // The library gives up at 30 s. The wait ends at 50 s (30 + 20),
            // before the re-ask's own 30 s are over (60 s).
            assert_eq!(start.elapsed(), BLUEZ_ASYNC_WAIT + MIN_WAIT);
            assert_eq!(
                fake.calls(),
                [Call::Connect, Call::Connect, Call::Disconnect]
            );
        })
        .await;
    }

    /// A `discovery_timeout` longer than `MIN_WAIT` lengthens the wait. With
    /// 40 s the wait ends between two of bluez-async's 30 s rounds, so the
    /// end of the wait and the end of a round can't be mistaken for each
    /// other.
    #[tokio::test(start_paused = true)]
    async fn a_longer_discovery_timeout_waits_longer() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.script_connect(std::iter::repeat_with(|| Outcome::DiscoveryTimedOut).take(10));
            let config = ConnectionConfig::default()
                .connection_timeout(Duration::from_secs(90))
                .discovery_timeout(Duration::from_secs(40));
            let start = Instant::now();

            let result = connect_with(&fake, &config).await;

            assert_discovery_timeout(result, Duration::from_secs(40));
            // The library gives up at 30 s and the wait ends at 70 s
            // (30 + 40). The first re-ask gives up at 60 s; the second would
            // at 90 s.
            assert_eq!(start.elapsed(), BLUEZ_ASYNC_WAIT + Duration::from_secs(40));
            assert_eq!(
                fake.calls(),
                [
                    Call::Connect,
                    Call::Connect,
                    Call::Connect,
                    Call::Disconnect,
                ]
            );
        })
        .await;
    }

    /// Any other error ends the wait at once and fails the connect: here
    /// bluez-async gave up under a long connect limit, the link had dropped,
    /// and BlueZ's new connection attempt failed.
    #[tokio::test(start_paused = true)]
    async fn another_error_ends_the_wait() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.script_connect([Outcome::DiscoveryTimedOut, Outcome::Fail]);
            let config = ConnectionConfig::default().connection_timeout(LONG_CONNECT_LIMIT);
            let start = Instant::now();

            let result = connect_with(&fake, &config).await;

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
            // The library gives up at 30 s and the re-ask fails at once.
            assert_eq!(start.elapsed(), BLUEZ_ASYNC_WAIT);
        })
        .await;
    }

    /// The same at the default limits, where the wait starts at the connect's
    /// own time limit: any other error from the re-ask ends the wait at once
    /// and fails the connect.
    #[tokio::test(start_paused = true)]
    async fn another_error_ends_the_wait_after_a_connect_timeout() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(true);
            fake.script_connect([Outcome::Hang, Outcome::Fail]);
            let config = ConnectionConfig::default();
            let start = Instant::now();

            let result = connect_with(&fake, &config).await;

            assert_eq!(
                fake.calls(),
                [
                    Call::Connect,
                    Call::IsConnected,
                    Call::Connect,
                    Call::Disconnect,
                ]
            );
            assert!(
                matches!(
                    result,
                    Err(Error::Bluetooth(btleplug::Error::RuntimeError(_)))
                ),
                "got {result:?}"
            );
            // The connect's limit ends at 15 s and the re-ask fails at once.
            assert_eq!(start.elapsed(), config.connection_timeout);
        })
        .await;
    }

    /// The reconnect of the zero-services retry waits for BlueZ too: here
    /// after bluez-async gave up on it, under a long connect limit.
    #[tokio::test(start_paused = true)]
    async fn the_zero_services_retry_waits_too() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.service_rounds([BTreeSet::new()]);
            fake.script_connect([Outcome::Ok, Outcome::DiscoveryTimedOut, Outcome::Ok]);
            let config = ConnectionConfig::default().connection_timeout(LONG_CONNECT_LIMIT);
            let start = Instant::now();

            connect_with(&fake, &config)
                .await
                .expect("the retry should wait for BlueZ's service discovery");

            // The retry's 2 s pause, then the library gives up 30 s into the
            // reconnect and the re-ask succeeds at once: 32 s.
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

    /// The same at the default limits: the retry's reconnect runs out of time
    /// with the link up, the wait asks again, and BlueZ resolves the services
    /// 4 s into the wait.
    #[tokio::test(start_paused = true)]
    async fn the_zero_services_retry_connects_after_a_connect_timeout() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(true);
            fake.service_rounds([BTreeSet::new()]);
            fake.script_connect([
                Outcome::Ok,
                Outcome::Hang,
                Outcome::After(Duration::from_secs(4)),
            ]);
            let config = ConnectionConfig::default();
            let start = Instant::now();

            connect_with(&fake, &config)
                .await
                .expect("the retry should wait for BlueZ's service discovery");

            // 2 s pause + 15 s connect limit + 4 s into the wait = 21 s.
            assert_eq!(start.elapsed(), Duration::from_secs(21));
            assert_eq!(
                fake.calls(),
                [
                    Call::Connect,
                    Call::Discover,
                    Call::Disconnect,
                    Call::Connect,
                    Call::IsConnected,
                    Call::Connect,
                    Call::Discover,
                ]
            );
        })
        .await;
    }

    /// On BlueZ at the default limits, a connect whose discovery outlasts the
    /// connect's 15 s runs out of time with the link up: bluez-async is still
    /// inside its own 30 s wait for the services then. The connect waits for
    /// BlueZ's service discovery by asking again. The wait's limit starts
    /// when the connect's ends and also covers the connection-state query,
    /// which answers at once here, so the wait and the discovery get all of
    /// it: BlueZ may take 19 of its 20 s.
    #[tokio::test(start_paused = true)]
    async fn a_connect_timeout_with_the_link_up_waits_on_bluez() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(true);
            fake.script_connect([Outcome::Hang, Outcome::After(Duration::from_secs(19))]);
            let config = ConnectionConfig::default();
            let start = Instant::now();

            connect_with(&fake, &config)
                .await
                .expect("the connect should wait for BlueZ's service discovery");

            // BlueZ resolves the services 19 s into the wait: 1 s inside it
            // (15 + 19 = 34 s).
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
                    Call::Discover,
                ]
            );
        })
        .await;
    }

    /// On BlueZ, when that wait never ends, the connect fails as a "discover
    /// services" timeout after the wait's limit, and disconnects. The wait
    /// makes one re-ask, which is still inside bluez-async's 30 s when the
    /// wait's 20 s are over. This is also how a link ends that BlueZ still
    /// reports as connected after it has died: the connection check passes,
    /// and no re-ask ever finds the services.
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
            // The connect's limit ends at 15 s and the wait at 35 s
            // (15 + 20). The re-ask's own 30 s would last until 45 s.
            assert_eq!(start.elapsed(), config.connection_timeout + MIN_WAIT);
            assert_eq!(
                fake.calls(),
                [
                    Call::Connect,
                    Call::IsConnected,
                    Call::Connect,
                    Call::Disconnect,
                ]
            );
        })
        .await;
    }

    /// On BlueZ, a connect that runs out of time with the link down fails as
    /// before, with the "connect to device" timeout, and disconnects without
    /// waiting. The link may never have come up, or it came up and dropped
    /// while bluez-async was still waiting for the services: bluez-async
    /// does not watch the link during that wait. Neither case is asked again
    /// inside the connect.
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
    /// for the discovery as before: here BlueZ takes another 5 s once it has
    /// accepted a re-ask.
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
                Outcome::After(Duration::from_secs(5)),
            ]);
            let config = ConnectionConfig::default();
            let start = Instant::now();

            connect_with(&fake, &config)
                .await
                .expect("the connect should wait while BlueZ is still connecting");

            // 15 s connect limit + three 1 s pauses + 5 s = 23 s.
            assert_eq!(
                start.elapsed(),
                config.connection_timeout + 3 * IN_PROGRESS_PAUSE + Duration::from_secs(5)
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
    /// This path needs a connect limit above bluez-async's 30 s wait. Under
    /// the default limits every wait starts at the connect's own time limit,
    /// and there `In Progress` is asked again
    /// (`a_connect_timeout_gives_up_while_bluez_stays_in_progress`).
    #[tokio::test(start_paused = true)]
    async fn in_progress_ends_the_wait_after_bluez_async_gave_up() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(true);
            fake.script_connect([Outcome::DiscoveryTimedOut, Outcome::InProgress]);
            let config = ConnectionConfig::default().connection_timeout(LONG_CONNECT_LIMIT);
            let start = Instant::now();

            let result = connect_with(&fake, &config).await;

            assert_in_progress(result);
            // The library gives up at 30 s and the re-ask is refused at once.
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

    /// The usual slow first connect on BlueZ at the default limits: BlueZ is
    /// still discovering when the connect's 15 s end, the link is up, and
    /// BlueZ resolves the services 8 s into the wait. The connect succeeds
    /// with one connection-state query and one re-ask.
    #[tokio::test(start_paused = true)]
    async fn a_discovery_that_outlasts_the_connect_limit_connects_through_the_wait() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(true);
            fake.script_connect([Outcome::Hang, Outcome::After(Duration::from_secs(8))]);
            let config = ConnectionConfig::default();
            let start = Instant::now();

            connect_with(&fake, &config)
                .await
                .expect("the connect should wait for BlueZ's service discovery");

            // 15 s connect limit + 8 s into the wait = 23 s.
            assert_eq!(start.elapsed(), Duration::from_secs(23));
            assert_eq!(
                fake.calls(),
                [
                    Call::Connect,
                    Call::IsConnected,
                    Call::Connect,
                    Call::Discover,
                ]
            );
        })
        .await;
    }

    /// `aranet watch` connects with a 30 s limit by default. There a
    /// discovery that takes 20 s finishes inside bluez-async's own wait for
    /// the services, so the connect succeeds without a connection-state
    /// query or a re-ask.
    #[tokio::test(start_paused = true)]
    async fn a_slow_discovery_inside_a_30_s_connect_limit_needs_no_wait() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(true);
            fake.script_connect([Outcome::After(Duration::from_secs(20))]);
            let config = ConnectionConfig::default().connection_timeout(Duration::from_secs(30));
            let start = Instant::now();

            connect_with(&fake, &config)
                .await
                .expect("bluez-async's own wait should cover the discovery");

            // The one connect returns at 20 s, inside the 30 s limit.
            assert_eq!(start.elapsed(), Duration::from_secs(20));
            assert_eq!(fake.calls(), [Call::Connect, Call::Discover]);
        })
        .await;
    }

    /// At `aranet watch`'s 30 s limit too, a connect that runs out of time
    /// with the link down fails with its own timeout and is not asked again
    /// inside the connect. That covers a link that came up and dropped while
    /// bluez-async was waiting for the services: under a limit of 30 s or
    /// less nothing re-dials it here, and the caller's own retry does.
    #[tokio::test(start_paused = true)]
    async fn a_30_s_connect_timeout_with_the_link_down_is_not_asked_again() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(true);
            fake.set_connected(false);
            fake.script_connect([Outcome::Hang]);
            let config = ConnectionConfig::default().connection_timeout(Duration::from_secs(30));
            let start = Instant::now();

            let error = connect_with(&fake, &config)
                .await
                .expect_err("the connect should fail");

            assert_timeout(&error, "connect to device", Duration::from_secs(30));
            // The connect's limit ends at 30 s; the query finds the link down.
            assert_eq!(start.elapsed(), Duration::from_secs(30));
            assert_eq!(
                fake.calls(),
                [Call::Connect, Call::IsConnected, Call::Disconnect]
            );
        })
        .await;
    }

    /// Under a connect limit longer than bluez-async's wait
    /// (`ConnectionConfig::challenging_environment()`: a 90 s connect limit
    /// and a 30 s `discovery_timeout`), bluez-async gives up first, 30 s in.
    /// The wait asks again, and BlueZ resolves the services 6 s later. No
    /// connection-state query is made: bluez-async's timeout itself shows
    /// that BlueZ had answered `Device1.Connect`. Under such a limit this is
    /// also how a link that dropped during bluez-async's wait is dialled
    /// again: the re-ask makes BlueZ open a new one.
    #[tokio::test(start_paused = true)]
    async fn bluez_async_gives_up_first_under_a_long_connect_limit() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(true);
            fake.script_connect([
                Outcome::DiscoveryTimedOut,
                Outcome::After(Duration::from_secs(6)),
            ]);
            let config = ConnectionConfig::challenging_environment();
            let start = Instant::now();

            connect_with(&fake, &config)
                .await
                .expect("the connect should wait for BlueZ's service discovery");

            // The library gives up at 30 s; the re-ask succeeds 6 s later.
            assert_eq!(start.elapsed(), BLUEZ_ASYNC_WAIT + Duration::from_secs(6));
            assert_eq!(fake.calls(), [Call::Connect, Call::Connect, Call::Discover]);
        })
        .await;
    }

    /// bluez-async gives each D-Bus call 30 s. When BlueZ sends no reply to
    /// `Device1.Connect` in that time, a connect whose own limit is longer
    /// fails with bluez-async's error, "Timeout waiting for reply". That is
    /// not a timeout of the connect itself, so no connection-state query is
    /// made and no wait starts: the connect disconnects and returns the
    /// error.
    #[tokio::test(start_paused = true)]
    async fn a_dbus_reply_timeout_fails_the_connect_without_a_wait() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(true);
            fake.script_connect([Outcome::DbusTimeout]);
            let config = ConnectionConfig::challenging_environment();
            let start = Instant::now();

            let result = connect_with(&fake, &config).await;

            match result {
                Err(Error::Bluetooth(btleplug::Error::Other(e))) => {
                    assert_eq!(e.to_string(), "Timeout waiting for reply");
                }
                other => panic!("expected the D-Bus reply timeout, got {other:?}"),
            }
            // The D-Bus call's limit ends at 30 s, inside the 90 s connect
            // limit.
            assert_eq!(start.elapsed(), BLUEZ_DBUS_TIMEOUT);
            assert_eq!(fake.calls(), [Call::Connect, Call::Disconnect]);
        })
        .await;
    }

    /// A guard on the fake. bluez-async waits 30 s for the services, so at
    /// the default 15 s connect limit its "Service discovery timed out" can
    /// never arrive before the connect's own limit ends. This script would
    /// let it: with `BLUEZ_ASYNC_WAIT` at the default connect limit or below,
    /// the library's timeout would start the wait, with no connection-state
    /// query, and the test fails. With the wait at 30 s the connect's limit
    /// ends the first call and the scripted timeout is never seen.
    #[tokio::test(start_paused = true)]
    async fn bluez_async_cannot_give_up_first_at_the_default_connect_limit() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(true);
            fake.script_connect([
                Outcome::DiscoveryTimedOut,
                Outcome::After(Duration::from_secs(2)),
            ]);
            let config = ConnectionConfig::default();
            let start = Instant::now();

            connect_with(&fake, &config)
                .await
                .expect("the connect should wait for BlueZ's service discovery");

            // 15 s connect limit + 2 s into the wait = 17 s. The scripted
            // library timeout would have come at 30 s.
            assert_eq!(start.elapsed(), Duration::from_secs(17));
            assert_eq!(
                fake.calls(),
                [
                    Call::Connect,
                    Call::IsConnected,
                    Call::Connect,
                    Call::Discover,
                ]
            );
        })
        .await;
    }

    /// A discovery that stalls is ended by BlueZ itself: BlueZ 5.82 gives an
    /// ATT request 30 s (`src/shared/att.c`, `ATT_TIMEOUT_INTERVAL`), then
    /// shuts the channel down and marks the services as resolved although it
    /// found none. A connect at the default limits waits up to 35 s in all
    /// (15 s to its own limit, then a 20 s wait), so it can still be waiting
    /// at that moment. Then the re-ask returns, the discovery finds no
    /// services, and the connect goes into the zero-services retry. Here the
    /// retry's fresh connection finds them.
    #[tokio::test(start_paused = true)]
    async fn a_discovery_that_bluez_ends_with_no_services_goes_into_the_retry() {
        within(Duration::from_secs(600), async {
            let fake = FakeGatt::new();
            fake.set_bluez(true);
            fake.service_rounds([BTreeSet::new()]);
            fake.script_connect([Outcome::Hang, Outcome::After(Duration::from_secs(17))]);
            let config = ConnectionConfig::default();
            let start = Instant::now();

            connect_with(&fake, &config)
                .await
                .expect("the retry's connection should find the services");

            // 15 s connect limit + 17 s into the wait (3 s inside it) + the
            // retry's 2 s pause = 34 s.
            assert_eq!(start.elapsed(), Duration::from_secs(34));
            assert_eq!(
                fake.calls(),
                [
                    Call::Connect,
                    Call::IsConnected,
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
    /// deadline `discovery_timeout` away. The wait starts after bluez-async
    /// gave up waiting for the services, which a connect without a limit
    /// lets it do after its 30 s, or after a finite connect limit ran out.
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

            // On BlueZ, a connect without a limit lasts until bluez-async
            // gives up waiting for the services, 30 s in (the fake's
            // `BLUEZ_ASYNC_WAIT`), so the connect waits for BlueZ to finish.
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

    /// Pairing, then the usual slow first connect on BlueZ: the pairing step
    /// uses its whole 35 s budget, the connect after it runs out of its 15 s
    /// with the link up, and BlueZ resolves the services 10 s into the wait.
    /// The connect's limit and the wait start only when the pairing step has
    /// returned.
    #[tokio::test(start_paused = true)]
    async fn pairing_then_a_discovery_that_outlasts_the_connect_limit_connects() {
        within(Duration::from_secs(600), async {
            let link = FakeGatt::new();
            link.set_bluez(true);
            link.script_pair([Outcome::After(DEFAULT_BUDGET)]);
            link.script_connect([Outcome::Hang, Outcome::After(Duration::from_secs(10))]);
            let start = Instant::now();

            connect_with(&link, &ConnectionConfig::default())
                .await
                .expect("the connect should wait for BlueZ's service discovery");

            // 35 s of pairing + 15 s connect limit + 10 s into the wait
            // = 60 s.
            assert_eq!(start.elapsed(), Duration::from_secs(60));
            assert_eq!(
                link.calls(),
                [
                    Call::Pair(DEFAULT_BUDGET),
                    Call::Connect,
                    Call::IsConnected,
                    Call::Connect,
                    Call::Discover,
                ]
            );
        })
        .await;
    }
}
