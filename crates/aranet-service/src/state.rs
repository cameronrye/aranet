//! Application state shared across handlers.

use std::sync::Arc;

use aranet_store::Store;
use tokio::sync::{Mutex, broadcast};

use crate::config::Config;

/// Shared application state.
pub struct AppState {
    /// The data store (wrapped in Mutex for thread-safe access).
    pub store: Mutex<Store>,
    /// Configuration.
    pub config: Config,
    /// Broadcast channel for real-time reading updates.
    pub readings_tx: broadcast::Sender<ReadingEvent>,
}

impl AppState {
    /// Create new application state.
    pub fn new(store: Store, config: Config) -> Arc<Self> {
        let (readings_tx, _) = broadcast::channel(100);
        Arc::new(Self {
            store: Mutex::new(store),
            config,
            readings_tx,
        })
    }
}

/// A reading event for WebSocket broadcast.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReadingEvent {
    /// Device ID.
    pub device_id: String,
    /// The reading data.
    pub reading: aranet_store::StoredReading,
}

#[cfg(test)]
mod tests {
    use super::*;
    use aranet_types::Status;
    use crate::config::Config;

    fn create_test_reading(device_id: &str, co2: u16) -> aranet_store::StoredReading {
        aranet_store::StoredReading {
            id: 1,
            device_id: device_id.to_string(),
            co2,
            temperature: 22.5,
            humidity: 45,
            pressure: 1013.0,
            battery: 85,
            status: Status::Green,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
            captured_at: time::OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn test_app_state_new() {
        let store = Store::open_in_memory().unwrap();
        let config = Config::default();
        let state = AppState::new(store, config);

        assert_eq!(state.config.server.bind, "127.0.0.1:8080");
    }

    #[tokio::test]
    async fn test_app_state_store_access() {
        let store = Store::open_in_memory().unwrap();
        let config = Config::default();
        let state = AppState::new(store, config);

        let store = state.store.lock().await;
        let devices = store.list_devices().unwrap();
        assert!(devices.is_empty());
    }

    #[tokio::test]
    async fn test_app_state_broadcast_channel() {
        let store = Store::open_in_memory().unwrap();
        let config = Config::default();
        let state = AppState::new(store, config);

        let mut rx = state.readings_tx.subscribe();

        let reading = create_test_reading("test", 450);

        let event = ReadingEvent {
            device_id: "test".to_string(),
            reading: reading.clone(),
        };

        // Send should succeed (at least one subscriber)
        state.readings_tx.send(event.clone()).unwrap();

        // Receive and verify
        let received = rx.recv().await.unwrap();
        assert_eq!(received.device_id, "test");
        assert_eq!(received.reading.co2, 450);
    }

    #[test]
    fn test_reading_event_serialization() {
        let reading = create_test_reading("AA:BB:CC:DD:EE:FF", 500);

        let event = ReadingEvent {
            device_id: "AA:BB:CC:DD:EE:FF".to_string(),
            reading,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("AA:BB:CC:DD:EE:FF"));
        assert!(json.contains("500"));
    }

    #[test]
    fn test_reading_event_debug() {
        let reading = create_test_reading("test", 400);

        let event = ReadingEvent {
            device_id: "test".to_string(),
            reading,
        };

        let debug = format!("{:?}", event);
        assert!(debug.contains("ReadingEvent"));
        assert!(debug.contains("test"));
    }
}

