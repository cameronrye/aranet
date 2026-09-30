//! The BLE connect sequence, behind a crate-private seam.
//!
//! `GattLink` is the part of a btleplug peripheral that connecting uses.
//! `Device` runs `connect` on btleplug's platform `Peripheral`, and the unit
//! tests run it on a scripted fake (`fake::FakeGatt`), so the connect, timeout
//! and cleanup paths can be tested without Bluetooth.

use std::collections::BTreeSet;
use std::time::Duration;

use btleplug::api::{PeripheralProperties, Service};
use tokio::runtime::Handle;
use tracing::{debug, info, warn};

use crate::device::ConnectionConfig;
use crate::error::{Error, Result};

#[cfg(test)]
mod fake;

/// The operations on a BLE peripheral that the connect sequence uses.
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
    /// return when the link comes up: bluez-async then waits up to 5 s for
    /// BlueZ to resolve the services, so a `connect` can run out of time with
    /// the link already up and BlueZ still discovering.
    fn is_bluez(&self) -> bool;
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

/// Connect, discover the services and read the properties.
///
/// If the first discovery finds no services, disconnect, wait 2 s, and connect
/// and discover once more. If that discovery finds none either, fail with a
/// retryable `Error::ConnectionFailed` ("the device reported no GATT
/// services").
///
/// On BlueZ, a connect (the first one or the retry's) can end while BlueZ is
/// still discovering the services on the live link: bluez-async stops waiting
/// for them 5 s after the link comes up, and a link that is slow to come up
/// can use up `connection_timeout` first. Either way the discovery after it
/// first waits for BlueZ to finish, and the wait and the discovery share
/// `bluez_discovery_limit` (`connect_link`, `discover`).
async fn connect_and_discover<L: GattLink>(
    link: &L,
    config: &ConnectionConfig,
) -> Result<(BTreeSet<Service>, Option<PeripheralProperties>)> {
    // Connect to the device with timeout
    info!("Connecting to device...");
    let bluez_discovering = connect_link(link, "connect to device", config).await?;
    info!("Connected!");

    // Discover services with timeout
    info!("Discovering services...");
    discover(link, "discover services", bluez_discovering, config).await?;

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

        let bluez_discovering = connect_link(link, "reconnect to device", config).await?;
        discover(link, "rediscover services", bluez_discovering, config).await?;

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
/// requests at 0.3-0.45 s each, 9-22 s from the moment the link comes up;
/// bluez-async has already waited 5 s of that when it gives up, and up to 5 s
/// of it has passed when the connect runs out of time.
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

/// Connect, giving up after `connection_timeout` (reported as `operation`).
///
/// Returns `true` when the link is up but BlueZ may still be discovering the
/// services: bluez-async stopped waiting for them, or, on BlueZ, the connect
/// ran out of time with the link already up.
async fn connect_link<L: GattLink>(
    link: &L,
    operation: &str,
    config: &ConnectionConfig,
) -> Result<bool> {
    match bounded(operation, config.connection_timeout, link.connect()).await {
        Ok(()) => return Ok(false),
        Err(Error::Bluetooth(e)) if is_bluez_discovery_timeout(&e) => {}
        // bluez-async's `connect` waits up to 5 s for the services after the
        // link comes up, so a link that takes longer than
        // `connection_timeout` minus 5 s to come up runs out of time here
        // while BlueZ is discovering on it. Disconnecting would cancel that
        // discovery, and every retry would start it again.
        Err(timeout @ Error::Timeout { .. }) if link.is_bluez() => {
            if !is_connected(link, config.validation_timeout).await {
                return Err(timeout);
            }
            debug!("The connect ran out of time with the link up");
        }
        Err(e) => return Err(e),
    }
    info!(
        "BlueZ is still discovering services; waiting up to {:?}",
        bluez_discovery_limit(config)
    );
    Ok(true)
}

/// Discover the services, giving up after `discovery_timeout` (reported as
/// `operation`). If `bluez_discovering`, first wait for BlueZ to finish; the
/// wait and the discovery then share `bluez_discovery_limit`.
async fn discover<L: GattLink>(
    link: &L,
    operation: &str,
    bluez_discovering: bool,
    config: &ConnectionConfig,
) -> Result<()> {
    if !bluez_discovering {
        return bounded(
            operation,
            config.discovery_timeout,
            link.discover_services(),
        )
        .await;
    }
    bounded(operation, bluez_discovery_limit(config), async {
        wait_for_bluez_discovery(link).await?;
        link.discover_services().await
    })
    .await
}

/// Call `connect` again until BlueZ has resolved the services.
///
/// BlueZ answers `Device1.Connect` at once while the device is connected
/// (`src/device.c`, `dev_connect`), and bluez-async then waits up to 5 s more
/// for `ServicesResolved`, or returns at once if it is already set. If the link
/// has dropped, `Device1.Connect` opens a new one, as the caller's own retry
/// would. Any result other than the discovery timeout ends the wait; the
/// caller limits it.
async fn wait_for_bluez_discovery<L: GattLink>(link: &L) -> btleplug::Result<()> {
    loop {
        match link.connect().await {
            Err(e) if is_bluez_discovery_timeout(&e) => {
                debug!("BlueZ is still discovering services");
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
}

/// BR-25: bluez-async's `connect` gives BlueZ 5 s to resolve the services
/// after the link comes up. A sensor with a large GATT table and a slow link,
/// such as an Aranet4 that BlueZ hasn't cached yet, needs longer, and BlueZ
/// keeps discovering on the live link after bluez-async has given up. When the
/// link itself is slow to come up, the connect's own time limit can run out
/// first, again with BlueZ discovering on the live link.
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

    /// Connect `fake` with `config` and the test's runtime for cleanup, and
    /// disarm the link if the connect succeeds.
    async fn connect_with(fake: &FakeGatt, config: &ConnectionConfig) -> Result<()> {
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
    /// waits for BlueZ's service discovery in the same way. The wait gets its
    /// whole limit after the connect's.
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
}
