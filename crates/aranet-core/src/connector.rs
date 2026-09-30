//! The seam between the reconnect logic and a live sensor link.
//!
//! `ReconnectCore` (in `reconnect.rs`) is generic over `SensorLink`, so its
//! races can be tested against `FakeRadio` in `test_support.rs`. In the library
//! it always runs on `Device`.

use std::sync::Arc;

use futures::FutureExt;
use futures::future::BoxFuture;

use crate::device::Device;
use crate::error::Result;

/// A connected sensor that can be checked and closed.
pub(crate) trait SensorLink: Send + Sync + 'static {
    /// Bounded; false when the stack doesn't answer.
    fn is_connected(&self) -> impl Future<Output = bool> + Send;
    /// Bounded; completes even if the caller is dropped.
    fn disconnect(&self) -> impl Future<Output = Result<()>> + Send;
}

/// Opens a new link to the sensor with the given identifier.
pub(crate) type ConnectFn<L> = Arc<dyn Fn(&str) -> BoxFuture<'static, Result<L>> + Send + Sync>;

/// Connects with `Device::connect` (default scan and connection settings).
pub(crate) fn ble_connector() -> ConnectFn<Device> {
    Arc::new(|identifier: &str| {
        let identifier = identifier.to_owned();
        async move { Device::connect(&identifier).await }.boxed()
    })
}

/// Disconnects `link` on a spawned task and awaits it, so it finishes even if
/// the caller is dropped. `hold` (a connection slot, or `()`) is dropped only
/// after the disconnect has finished.
pub(crate) async fn release_link<L: SensorLink, H: Send + 'static>(
    link: Arc<L>,
    hold: H,
) -> Result<()> {
    tokio::spawn(async move {
        let result = link.disconnect().await;
        drop(hold);
        result
    })
    .await
    .map_err(std::io::Error::from)?
}

impl SensorLink for Device {
    async fn is_connected(&self) -> bool {
        Device::is_connected(self).await
    }

    async fn disconnect(&self) -> Result<()> {
        Device::disconnect(self).await
    }
}
