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

#[cfg(test)]
mod tests {
    use super::*;
    use aranet_types::{CurrentReading, DeviceType, Status};

    // ==================== DeviceId Tests ====================

    #[test]
    fn test_device_id_new() {
        let id = DeviceId::new("AA:BB:CC:DD:EE:FF");
        assert_eq!(id.id, "AA:BB:CC:DD:EE:FF");
        assert!(id.name.is_none());
        assert!(id.device_type.is_none());
    }

    #[test]
    fn test_device_id_with_name() {
        let id = DeviceId::with_name("AA:BB:CC:DD:EE:FF", "Kitchen Sensor");
        assert_eq!(id.id, "AA:BB:CC:DD:EE:FF");
        assert_eq!(id.name, Some("Kitchen Sensor".to_string()));
        assert!(id.device_type.is_none());
    }

    #[test]
    fn test_device_id_with_device_type() {
        let mut id = DeviceId::new("test-id");
        id.device_type = Some(DeviceType::Aranet4);
        assert_eq!(id.device_type, Some(DeviceType::Aranet4));
    }

    #[test]
    fn test_device_id_equality() {
        let id1 = DeviceId::new("test");
        let id2 = DeviceId::new("test");
        let id3 = DeviceId::new("different");

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_device_id_clone() {
        let id1 = DeviceId::with_name("test", "name");
        let id2 = id1.clone();
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_device_id_serialization() {
        let id = DeviceId::with_name("device-123", "My Device");
        let json = serde_json::to_string(&id).unwrap();
        assert!(json.contains("device-123"));
        assert!(json.contains("My Device"));

        let deserialized: DeviceId = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, id);
    }

    // ==================== DisconnectReason Tests ====================

    #[test]
    fn test_disconnect_reason_equality() {
        assert_eq!(
            DisconnectReason::UserRequested,
            DisconnectReason::UserRequested
        );
        assert_eq!(DisconnectReason::OutOfRange, DisconnectReason::OutOfRange);
        assert_eq!(DisconnectReason::Timeout, DisconnectReason::Timeout);
        assert_eq!(
            DisconnectReason::DevicePoweredOff,
            DisconnectReason::DevicePoweredOff
        );
        assert_eq!(DisconnectReason::Unknown, DisconnectReason::Unknown);

        assert_ne!(DisconnectReason::UserRequested, DisconnectReason::Timeout);
    }

    #[test]
    fn test_disconnect_reason_ble_error() {
        let reason1 = DisconnectReason::BleError("error 1".to_string());
        let reason2 = DisconnectReason::BleError("error 1".to_string());
        let reason3 = DisconnectReason::BleError("error 2".to_string());

        assert_eq!(reason1, reason2);
        assert_ne!(reason1, reason3);
    }

    #[test]
    fn test_disconnect_reason_serialization() {
        for reason in [
            DisconnectReason::UserRequested,
            DisconnectReason::OutOfRange,
            DisconnectReason::Timeout,
            DisconnectReason::DevicePoweredOff,
            DisconnectReason::BleError("test error".to_string()),
            DisconnectReason::Unknown,
        ] {
            let json = serde_json::to_string(&reason).unwrap();
            let deserialized: DisconnectReason = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, reason);
        }
    }

    #[test]
    fn test_disconnect_reason_clone() {
        let reason = DisconnectReason::BleError("connection lost".to_string());
        let cloned = reason.clone();
        assert_eq!(reason, cloned);
    }

    // ==================== DeviceEvent Tests ====================

    fn create_test_reading() -> CurrentReading {
        CurrentReading {
            co2: 800,
            temperature: 22.5,
            pressure: 1013.0,
            humidity: 45,
            battery: 85,
            status: Status::Green,
            interval: 60,
            age: 30,
            captured_at: None,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
            radon_avg_24h: None,
            radon_avg_7d: None,
            radon_avg_30d: None,
        }
    }

    #[test]
    fn test_device_event_discovered() {
        let event = DeviceEvent::Discovered {
            device: DeviceId::new("test"),
            rssi: Some(-65),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("discovered"));
        assert!(json.contains("-65"));
    }

    #[test]
    fn test_device_event_connected() {
        let event = DeviceEvent::Connected {
            device: DeviceId::new("test"),
            info: None,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("connected"));
    }

    #[test]
    fn test_device_event_disconnected() {
        let event = DeviceEvent::Disconnected {
            device: DeviceId::new("test"),
            reason: DisconnectReason::UserRequested,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("disconnected"));
    }

    #[test]
    fn test_device_event_reading() {
        let event = DeviceEvent::Reading {
            device: DeviceId::new("test"),
            reading: create_test_reading(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("reading"));
        assert!(json.contains("800")); // CO2 value
    }

    #[test]
    fn test_device_event_error() {
        let event = DeviceEvent::Error {
            device: DeviceId::new("test"),
            error: "Connection timeout".to_string(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("error"));
        assert!(json.contains("Connection timeout"));
    }

    #[test]
    fn test_device_event_reconnect_started() {
        let event = DeviceEvent::ReconnectStarted {
            device: DeviceId::new("test"),
            attempt: 3,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("reconnect_started"));
        assert!(json.contains("3"));
    }

    #[test]
    fn test_device_event_reconnect_succeeded() {
        let event = DeviceEvent::ReconnectSucceeded {
            device: DeviceId::new("test"),
            attempts: 2,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("reconnect_succeeded"));
    }

    #[test]
    fn test_device_event_battery_low() {
        let event = DeviceEvent::BatteryLow {
            device: DeviceId::new("test"),
            level: 10,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("battery_low"));
        assert!(json.contains("10"));
    }

    #[test]
    fn test_device_event_clone() {
        let event = DeviceEvent::Reading {
            device: DeviceId::new("test"),
            reading: create_test_reading(),
        };

        let cloned = event.clone();
        match cloned {
            DeviceEvent::Reading { device, reading } => {
                assert_eq!(device.id, "test");
                assert_eq!(reading.co2, 800);
            }
            _ => unreachable!("Clone should preserve event type as Reading"),
        }
    }

    // ==================== Event Channel Tests ====================

    #[test]
    fn test_event_channel() {
        let (tx, rx) = event_channel(50);
        // Receiver is returned by channel(), so count starts at 1
        assert_eq!(tx.receiver_count(), 1);
        drop(rx);
        assert_eq!(tx.receiver_count(), 0);
    }

    #[test]
    fn test_default_event_channel() {
        let (tx, rx) = default_event_channel();
        // Receiver is returned by channel(), so count starts at 1
        assert_eq!(tx.receiver_count(), 1);
        drop(rx);
        assert_eq!(tx.receiver_count(), 0);
    }

    #[tokio::test]
    async fn test_event_channel_send_receive() {
        let (tx, mut rx) = event_channel(10);

        let event = DeviceEvent::Discovered {
            device: DeviceId::new("test"),
            rssi: Some(-70),
        };

        tx.send(event.clone()).unwrap();

        let received = rx.recv().await.unwrap();
        match received {
            DeviceEvent::Discovered { device, rssi } => {
                assert_eq!(device.id, "test");
                assert_eq!(rssi, Some(-70));
            }
            _ => unreachable!("Expected Discovered event"),
        }
    }

    // ==================== EventDispatcher Tests ====================

    #[test]
    fn test_event_dispatcher_new() {
        let dispatcher = EventDispatcher::new(50);
        assert_eq!(dispatcher.receiver_count(), 0);
    }

    #[test]
    fn test_event_dispatcher_default() {
        let dispatcher = EventDispatcher::default();
        assert_eq!(dispatcher.receiver_count(), 0);
    }

    #[test]
    fn test_event_dispatcher_subscribe() {
        let dispatcher = EventDispatcher::new(10);
        assert_eq!(dispatcher.receiver_count(), 0);

        let _rx1 = dispatcher.subscribe();
        assert_eq!(dispatcher.receiver_count(), 1);

        let _rx2 = dispatcher.subscribe();
        assert_eq!(dispatcher.receiver_count(), 2);
    }

    #[tokio::test]
    async fn test_event_dispatcher_send_receive() {
        let dispatcher = EventDispatcher::new(10);
        let mut rx = dispatcher.subscribe();

        let event = DeviceEvent::Connected {
            device: DeviceId::with_name("test", "Test Device"),
            info: None,
        };

        dispatcher.send(event);

        let received = rx.recv().await.unwrap();
        match received {
            DeviceEvent::Connected { device, .. } => {
                assert_eq!(device.id, "test");
                assert_eq!(device.name, Some("Test Device".to_string()));
            }
            _ => unreachable!("Expected Connected event"),
        }
    }

    #[tokio::test]
    async fn test_event_dispatcher_multiple_receivers() {
        let dispatcher = EventDispatcher::new(10);
        let mut rx1 = dispatcher.subscribe();
        let mut rx2 = dispatcher.subscribe();

        let event = DeviceEvent::Discovered {
            device: DeviceId::new("multi-test"),
            rssi: Some(-50),
        };

        dispatcher.send(event);

        // Both receivers should get the event
        let received1 = rx1.recv().await.unwrap();
        let received2 = rx2.recv().await.unwrap();

        match (received1, received2) {
            (
                DeviceEvent::Discovered { device: d1, .. },
                DeviceEvent::Discovered { device: d2, .. },
            ) => {
                assert_eq!(d1.id, "multi-test");
                assert_eq!(d2.id, "multi-test");
            }
            _ => unreachable!("Expected Discovered events from both receivers"),
        }
    }

    #[test]
    fn test_event_dispatcher_send_no_receivers() {
        let dispatcher = EventDispatcher::new(10);

        // Should not panic even with no receivers
        let event = DeviceEvent::Error {
            device: DeviceId::new("test"),
            error: "no one listening".to_string(),
        };

        dispatcher.send(event);
    }

    #[test]
    fn test_event_dispatcher_sender() {
        let dispatcher = EventDispatcher::new(10);
        let sender = dispatcher.sender();

        // Sender should work independently
        assert_eq!(sender.receiver_count(), 0);
    }

    #[test]
    fn test_event_dispatcher_clone() {
        let dispatcher1 = EventDispatcher::new(10);
        let _rx1 = dispatcher1.subscribe();

        let dispatcher2 = dispatcher1.clone();

        // Both dispatchers share the same channel
        assert_eq!(dispatcher1.receiver_count(), 1);
        assert_eq!(dispatcher2.receiver_count(), 1);

        let _rx2 = dispatcher2.subscribe();
        assert_eq!(dispatcher1.receiver_count(), 2);
        assert_eq!(dispatcher2.receiver_count(), 2);
    }

    #[test]
    fn test_event_dispatcher_debug() {
        let dispatcher = EventDispatcher::new(10);
        let debug = format!("{:?}", dispatcher);
        assert!(debug.contains("EventDispatcher"));
    }
}
