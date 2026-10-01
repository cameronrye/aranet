//! The seam between the reconnect and device-manager logic and a live sensor
//! link.
//!
//! `ReconnectCore` (in `reconnect.rs`) and `ManagerCore` (in `manager.rs`) are
//! generic over `SensorLink`, so their races can be tested against `FakeRadio`
//! in `test_support.rs`. In the library they always run on `Device`.

use std::sync::Arc;

use futures::FutureExt;
use futures::future::BoxFuture;
use tracing::{debug, warn};

use aranet_types::{CurrentReading, DeviceInfo, DeviceType};

use crate::device::Device;
use crate::error::{Error, Result};

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
/// the caller is dropped. `hold` (what must stay held until the link is down,
/// such as a connection slot or a share of a lock guard) is dropped only after
/// the disconnect has finished.
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

/// Whether closing a link that has already dropped on its own times out on
/// this platform. Where it does, a close that times out usually means only
/// that the link had dropped, not that the sensor is still connected. True
/// only on macOS: once CoreBluetooth reports a link down, btleplug's
/// CoreBluetooth backend forgets the peripheral and never answers a disconnect
/// for it, so the close runs into its time limit. Links to unpaired sensors
/// can drop on their own there a few minutes after they come up.
const CLOSES_TIME_OUT_AFTER_DROP: bool = cfg!(target_os = "macos");

/// Whether `error`, from a failed `release_link`, is expected and so no sign
/// that the sensor is still connected: true only for a close that timed out,
/// and only when `closes_time_out_after_drop` (the library passes
/// `CLOSES_TIME_OUT_AFTER_DROP`). Any other failure may have left the sensor
/// connected with no handle.
fn release_failure_is_expected(error: &Error, closes_time_out_after_drop: bool) -> bool {
    closes_time_out_after_drop && matches!(error, Error::Timeout { .. })
}

/// Logs that `what` (such as "Closing the old link") to `identifier` failed
/// with `error`, an error from `release_link`: at debug level if the failure
/// is expected (`release_failure_is_expected`), and otherwise as a warning,
/// since the sensor may still be connected with no handle.
pub(crate) fn log_failed_release(what: &str, identifier: &str, error: &Error) {
    if release_failure_is_expected(error, CLOSES_TIME_OUT_AFTER_DROP) {
        debug!("{what} to {identifier} failed: {error}");
    } else {
        warn!("{what} to {identifier} failed: {error}");
    }
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

#[cfg(test)]
mod tests {
    use super::release_failure_is_expected;
    use crate::error::Error;
    use crate::link::DISCONNECT_TIMEOUT;

    #[test]
    fn a_timed_out_release_is_expected_only_where_closes_time_out_after_a_drop() {
        let timed_out = Error::timeout("disconnect from device", DISCONNECT_TIMEOUT);
        assert!(release_failure_is_expected(&timed_out, true));
        assert!(!release_failure_is_expected(&timed_out, false));
    }

    #[test]
    fn a_release_that_failed_otherwise_is_never_expected() {
        let failures = [
            Error::Bluetooth(btleplug::Error::NotConnected),
            Error::Io(std::io::Error::other("the disconnect task panicked")),
        ];
        for error in &failures {
            for closes_time_out_after_drop in [true, false] {
                assert!(
                    !release_failure_is_expected(error, closes_time_out_after_drop),
                    "{error} (closes_time_out_after_drop: {closes_time_out_after_drop})"
                );
            }
        }
    }
}
