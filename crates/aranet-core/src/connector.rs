//! The seam between the reconnect and device-manager logic and a live sensor
//! link.
//!
//! `ReconnectCore` (in `reconnect.rs`) and `ManagerCore` (in `manager.rs`) are
//! generic over `SensorLink`, so their races can be tested against `FakeRadio`
//! in `test_support.rs`. In the library they always run on `Device`.

use std::sync::Arc;

use futures::FutureExt;
use futures::future::BoxFuture;

use aranet_types::{CurrentReading, DeviceInfo, DeviceType};

use crate::device::Device;
use crate::error::Result;

/// A connected sensor that can be checked and closed.
pub(crate) trait SensorLink: Send + Sync + 'static {
    /// Bounded; false when the stack doesn't answer.
    fn is_connected(&self) -> impl Future<Output = bool> + Send;
    /// Bounded; completes even if the caller is dropped.
    fn disconnect(&self) -> impl Future<Output = Result<()>> + Send;
    /// The device name read when the link was made.
    fn name(&self) -> Option<&str>;
    /// The device model read when the link was made.
    fn device_type(&self) -> Option<DeviceType>;
    /// Reads from the sensor to check that the link still works
    /// (`Device::validate_connection`). Bounded; false when the read fails or
    /// runs out of time.
    fn is_alive(&self) -> impl Future<Output = bool> + Send;
    fn read_current(&self) -> impl Future<Output = Result<CurrentReading>> + Send;
    fn read_device_info(&self) -> impl Future<Output = Result<DeviceInfo>> + Send;
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

    fn name(&self) -> Option<&str> {
        Device::name(self)
    }

    fn device_type(&self) -> Option<DeviceType> {
        Device::device_type(self)
    }

    async fn is_alive(&self) -> bool {
        Device::validate_connection(self).await
    }

    async fn read_current(&self) -> Result<CurrentReading> {
        Device::read_current(self).await
    }

    async fn read_device_info(&self) -> Result<DeviceInfo> {
        Device::read_device_info(self).await
    }
}
