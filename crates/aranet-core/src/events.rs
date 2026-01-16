//! Device event system for connection and reading notifications.
//!
//! This module provides an event-based system for receiving notifications
//! about device connections, disconnections, readings, and errors.

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use aranet_types::{CurrentReading, DeviceInfo, DeviceType};

/// Device identifier for events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceId {
    /// Unique identifier (peripheral ID or MAC address).
    pub id: String,
    /// Device name if known.
    pub name: Option<String>,
    /// Device type if known.
    pub device_type: Option<DeviceType>,
}

impl DeviceId {
    /// Create a new device ID.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: None,
            device_type: None,
        }
    }

    /// Create a device ID with name.
    pub fn with_name(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: Some(name.into()),
            device_type: None,
        }
    }
}

/// Events that can be emitted by devices.
///
/// All events are serializable for logging, persistence, and IPC.
///
/// This enum is marked `#[non_exhaustive]` to allow adding new event types
/// in future versions without breaking downstream code.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum DeviceEvent {
    /// Device was discovered during scanning.
    Discovered { device: DeviceId, rssi: Option<i16> },
    /// Successfully connected to device.
    Connected {
        device: DeviceId,
        info: Option<DeviceInfo>,
    },
    /// Disconnected from device.
    Disconnected {
        device: DeviceId,
        reason: DisconnectReason,
    },
    /// New reading received from device.
    Reading {
        device: DeviceId,
        reading: CurrentReading,
    },
    /// Error occurred during device operation.
    Error { device: DeviceId, error: String },
    /// Reconnection attempt started.
    ReconnectStarted { device: DeviceId, attempt: u32 },
    /// Reconnection succeeded.
    ReconnectSucceeded { device: DeviceId, attempts: u32 },
    /// Battery level changed significantly.
    BatteryLow { device: DeviceId, level: u8 },
}

/// Reason for disconnection.
///
/// This enum is marked `#[non_exhaustive]` to allow adding new reasons
/// in future versions without breaking downstream code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DisconnectReason {
    /// Normal disconnection requested by user.
    UserRequested,
    /// Device went out of range.
    OutOfRange,
    /// Connection timed out.
    Timeout,
    /// Device was powered off.
    DevicePoweredOff,
    /// BLE error occurred.
    BleError(String),
    /// Unknown reason.
    Unknown,
}

/// Sender for device events.
pub type EventSender = broadcast::Sender<DeviceEvent>;

/// Receiver for device events.
pub type EventReceiver = broadcast::Receiver<DeviceEvent>;

/// Create a new event channel with the given capacity.
pub fn event_channel(capacity: usize) -> (EventSender, EventReceiver) {
    broadcast::channel(capacity)
}

/// Create a default event channel with capacity 100.
pub fn default_event_channel() -> (EventSender, EventReceiver) {
    event_channel(100)
}

/// Event dispatcher for sending events to multiple receivers.
#[derive(Debug, Clone)]
pub struct EventDispatcher {
    sender: EventSender,
}

impl EventDispatcher {
    /// Create a new event dispatcher.
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Subscribe to events.
    pub fn subscribe(&self) -> EventReceiver {
        self.sender.subscribe()
    }

    /// Send an event.
    pub fn send(&self, event: DeviceEvent) {
        // Ignore error if no receivers
        let _ = self.sender.send(event);
    }

    /// Get the number of active receivers.
    pub fn receiver_count(&self) -> usize {
        self.sender.receiver_count()
    }

    /// Get the sender for direct use.
    pub fn sender(&self) -> EventSender {
        self.sender.clone()
    }
}

impl Default for EventDispatcher {
    fn default() -> Self {
        Self::new(100)
    }
}
