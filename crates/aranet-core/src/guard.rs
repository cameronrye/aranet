//! Connection guard for automatic disconnect on drop.
//!
//! This module provides RAII-style connection management for Aranet devices,
//! ensuring that connections are properly closed when the guard goes out of scope.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use tokio::runtime::Handle;
use tracing::warn;

use crate::device::Device;

/// A guard that automatically disconnects from the device when dropped.
///
/// This provides RAII-style management of BLE connections. When the guard
/// is dropped, it will attempt to disconnect from the device.
///
/// # Example
///
/// ```ignore
/// use aranet_core::{Device, DeviceGuard};
///
/// async fn read_with_guard() -> Result<(), Box<dyn std::error::Error>> {
///     let device = Device::connect("Aranet4 12345").await?;
///     let guard = DeviceGuard::new(device);
///
///     // Use the device through the guard
///     let reading = guard.read_current().await?;
///     println!("CO2: {}", reading.co2);
///
///     // Device is automatically disconnected when guard goes out of scope
///     Ok(())
/// }
/// ```
pub struct DeviceGuard {
    device: Option<Device>,
}

impl DeviceGuard {
    /// Create a new device guard.
    pub fn new(device: Device) -> Self {
        Self {
            device: Some(device),
        }
    }

    /// Take ownership of the device, preventing automatic disconnect.
    ///
    /// After calling this, you are responsible for disconnecting the device.
    /// This consumes the guard, so the device cannot be "already taken" —
    /// the `Option` is only `None` during `Drop`.
    pub fn into_inner(mut self) -> Device {
        // SAFETY: `device` is always `Some` except inside `Drop`.
        // Since `into_inner` takes `self` by value, Drop hasn't run yet.
        self.device
            .take()
            .expect("DeviceGuard invariant violated: device is None outside of Drop")
    }

    /// Get a reference to the device.
    fn device(&self) -> &Device {
        // SAFETY: Same invariant as above — always `Some` outside of `Drop`.
        self.device
            .as_ref()
            .expect("DeviceGuard invariant violated: device is None outside of Drop")
    }

    /// Get a mutable reference to the device.
    fn device_mut(&mut self) -> &mut Device {
        self.device
            .as_mut()
            .expect("DeviceGuard invariant violated: device is None outside of Drop")
    }
}

impl Deref for DeviceGuard {
    type Target = Device;

    fn deref(&self) -> &Self::Target {
        self.device()
    }
}

impl DerefMut for DeviceGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.device_mut()
    }
}

impl Drop for DeviceGuard {
    fn drop(&mut self) {
        if let Some(device) = self.device.take() {
            // Try to get a runtime handle to perform async disconnect
            if let Ok(handle) = Handle::try_current() {
                handle.spawn(async move {
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        device.disconnect(),
                    )
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            warn!("Failed to disconnect device in guard drop: {}", e);
                        }
                        Err(_) => {
                            warn!("Timeout disconnecting device in guard drop");
                        }
                    }
                });
            } else {
                // No runtime available, log warning
                warn!("No tokio runtime available for device disconnect in guard drop");
            }
        }
    }
}

/// A guard for Arc-wrapped devices.
///
/// Similar to `DeviceGuard` but for shared device references.
pub struct SharedDeviceGuard {
    device: Arc<Device>,
}

impl SharedDeviceGuard {
    /// Create a new shared device guard.
    pub fn new(device: Arc<Device>) -> Self {
        Self { device }
    }

    /// Consume the guard and return the underlying Arc.
    ///
    /// After calling this, the device will NOT be automatically disconnected
    /// when the returned Arc is dropped. You are responsible for managing
    /// the device lifecycle.
    pub fn into_inner(self) -> Arc<Device> {
        // Use ManuallyDrop to prevent Drop from running
        let guard = std::mem::ManuallyDrop::new(self);
        Arc::clone(&guard.device)
    }
}

impl Deref for SharedDeviceGuard {
    type Target = Device;

    fn deref(&self) -> &Self::Target {
        &self.device
    }
}

impl Drop for SharedDeviceGuard {
    fn drop(&mut self) {
        let device = Arc::clone(&self.device);
        if let Ok(handle) = Handle::try_current() {
            handle.spawn(async move {
                if let Err(e) = device.disconnect().await {
                    warn!("Failed to disconnect shared device in guard drop: {}", e);
                }
            });
        }
    }
}
