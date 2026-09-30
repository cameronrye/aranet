//! The BLE connect sequence, behind a crate-private seam.
//!
//! `GattLink` is the part of a btleplug peripheral that connecting uses.
//! `Device` runs `connect` on btleplug's platform `Peripheral`, and the unit
//! tests run it on a scripted fake (`fake::FakeGatt`), so the connect, timeout
//! and cleanup paths can be tested without Bluetooth.

use std::collections::BTreeSet;
use std::time::Duration;

use btleplug::api::{PeripheralProperties, Service};
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

/// A connected peripheral whose services have been discovered.
pub(crate) struct OpenLink {
    /// The GATT services the peripheral reported.
    pub(crate) services: BTreeSet<Service>,
    /// What the peripheral advertised, if the Bluetooth stack knows it.
    pub(crate) properties: Option<PeripheralProperties>,
}

/// Connect to `link`, discover its services and read its properties.
///
/// If the first discovery finds no services, disconnect, wait 2 s, then
/// connect and discover once more.
pub(crate) async fn connect<L: GattLink>(link: &L, config: &ConnectionConfig) -> Result<OpenLink> {
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
        let _ = link.disconnect().await;
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
    }

    debug!("Found {} services", services.len());

    // Get device properties
    let properties = link.properties().await?;

    Ok(OpenLink {
        services,
        properties,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::Duration;

    use tokio::time::Instant;

    use super::connect;
    use super::fake::{Call, FakeGatt, Outcome, aranet_services};
    use crate::device::ConnectionConfig;
    use crate::error::Error;
    use crate::test_support::within;

    /// A plain connect makes one connect and one discovery, and returns the
    /// services and properties the peripheral reported.
    #[tokio::test(start_paused = true)]
    async fn successful_connect_leaves_link_up() {
        within(Duration::from_secs(600), async {
            let link = FakeGatt::new();

            let open = connect(&link, &ConnectionConfig::default())
                .await
                .expect("connect should succeed");

            assert_eq!(link.calls(), [Call::Connect, Call::Discover]);
            assert_eq!(open.services, aranet_services());
            assert_eq!(
                open.properties.and_then(|p| p.local_name).as_deref(),
                Some("Aranet4 12345")
            );
        })
        .await;
    }

    /// An empty first discovery is retried once on a fresh connection, 2 s
    /// after disconnecting, and the second discovery's services are used.
    #[tokio::test(start_paused = true)]
    async fn zero_services_then_services_connects() {
        within(Duration::from_secs(600), async {
            let link = FakeGatt::new();
            link.service_rounds([BTreeSet::new(), aranet_services()]);
            let start = Instant::now();

            let open = connect(&link, &ConnectionConfig::default())
                .await
                .expect("the second discovery should find the services");

            assert_eq!(
                link.calls(),
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
    /// operation name that today's error messages carry.
    #[tokio::test(start_paused = true)]
    async fn connect_timeout_reports_the_existing_operation_name() {
        within(Duration::from_secs(600), async {
            let link = FakeGatt::new();
            link.script_connect([Outcome::Hang]);
            let start = Instant::now();

            match connect(&link, &ConnectionConfig::default()).await {
                Err(Error::Timeout {
                    operation,
                    duration,
                }) => {
                    assert_eq!(operation, "connect to device");
                    assert_eq!(duration, Duration::from_secs(15));
                }
                other => panic!("expected a connect timeout, got {:?}", other.err()),
            }
            assert_eq!(start.elapsed(), Duration::from_secs(15));
            assert_eq!(link.calls(), [Call::Connect]);
        })
        .await;
    }
}
