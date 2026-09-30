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
/// and discover once more.
async fn connect_and_discover<L: GattLink>(
    link: &L,
    config: &ConnectionConfig,
) -> Result<(BTreeSet<Service>, Option<PeripheralProperties>)> {
    // Connect to the device with timeout
    info!("Connecting to device...");
    bounded(
        "connect to device",
        config.connection_timeout,
        link.connect(),
    )
    .await?;
    info!("Connected!");

    // Discover services with timeout
    info!("Discovering services...");
    bounded(
        "discover services",
        config.discovery_timeout,
        link.discover_services(),
    )
    .await?;

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

        bounded(
            "reconnect to device",
            config.connection_timeout,
            link.connect(),
        )
        .await?;

        bounded(
            "rediscover services",
            config.discovery_timeout,
            link.discover_services(),
        )
        .await?;

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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::Duration;

    use tokio::runtime::Handle;
    use tokio::time::Instant;

    use super::fake::{Call, FakeGatt, Outcome, aranet_services};
    use super::{DISCONNECT_TIMEOUT, connect};
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
    fn assert_timeout(error: &Error, operation: &str, duration: Duration) {
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
}
