//! Application state shared across handlers.
//!
//! # Broadcast Channel Behavior
//!
//! The `readings_tx` broadcast channel is used for real-time updates to WebSocket clients.
//! Key characteristics:
//!
//! - **Buffer size**: Configurable via `server.broadcast_buffer` (default: 100)
//! - **Message loss**: If a subscriber falls behind and the buffer fills, old messages are dropped
//! - **No blocking**: Senders never block; they succeed or drop messages for slow receivers
//!
//! ## Tuning the Buffer Size
//!
//! - **Increase** if WebSocket clients frequently miss messages (e.g., slow network)
//! - **Decrease** to reduce memory usage in resource-constrained environments
//! - **Monitor** using the `/api/status` endpoint to track message delivery
//!
//! ## Example Configuration
//!
//! ```toml
//! [server]
//! bind = "127.0.0.1:8080"
//! broadcast_buffer = 200  # Larger buffer for slow clients
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use aranet_store::Store;
use time::OffsetDateTime;
use tokio::sync::{Mutex, RwLock, broadcast, watch};

use crate::config::Config;

/// Shared application state.
pub struct AppState {
    /// The data store (wrapped in Mutex for thread-safe access).
    pub store: Mutex<Store>,
    /// Configuration (RwLock for runtime updates).
    pub config: RwLock<Config>,
    /// Broadcast channel for real-time reading updates.
    pub readings_tx: broadcast::Sender<ReadingEvent>,
    /// Collector control state.
    pub collector: CollectorState,
}

impl AppState {
    /// Create new application state.
    ///
    /// The broadcast channel buffer size is determined by `config.server.broadcast_buffer`.
    /// If the buffer fills (slow subscribers), old messages are dropped without blocking.
    pub fn new(store: Store, config: Config) -> Arc<Self> {
        let buffer_size = config.server.broadcast_buffer;
        let (readings_tx, _) = broadcast::channel(buffer_size);
        Arc::new(Self {
            store: Mutex::new(store),
            config: RwLock::new(config),
            readings_tx,
            collector: CollectorState::new(),
        })
    }
}

/// State for tracking and controlling the collector.
pub struct CollectorState {
    /// Whether the collector is currently running.
    running: AtomicBool,
    /// When the collector was started (Unix timestamp).
    started_at: AtomicU64,
    /// Channel to signal collector tasks to stop.
    stop_tx: watch::Sender<bool>,
    /// Receiver for stop signal (cloned by collector tasks).
    stop_rx: watch::Receiver<bool>,
    /// Per-device collection stats.
    pub device_stats: RwLock<Vec<DeviceCollectionStats>>,
}

impl CollectorState {
    /// Create a new collector state.
    pub fn new() -> Self {
        let (stop_tx, stop_rx) = watch::channel(false);
        Self {
            running: AtomicBool::new(false),
            started_at: AtomicU64::new(0),
            stop_tx,
            stop_rx,
            device_stats: RwLock::new(Vec::new()),
        }
    }

    /// Check if the collector is running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Mark the collector as started.
    pub fn set_running(&self, running: bool) {
        self.running.store(running, Ordering::SeqCst);
        if running {
            let now = OffsetDateTime::now_utc().unix_timestamp() as u64;
            self.started_at.store(now, Ordering::SeqCst);
        }
    }

    /// Get the collector start time.
    pub fn started_at(&self) -> Option<OffsetDateTime> {
        let ts = self.started_at.load(Ordering::SeqCst);
        if ts == 0 {
            None
        } else {
            OffsetDateTime::from_unix_timestamp(ts as i64).ok()
        }
    }

    /// Get a receiver for the stop signal.
    pub fn subscribe_stop(&self) -> watch::Receiver<bool> {
        self.stop_rx.clone()
    }

    /// Signal all collector tasks to stop.
    pub fn signal_stop(&self) {
        let _ = self.stop_tx.send(true);
        self.running.store(false, Ordering::SeqCst);
    }

    /// Reset the stop signal (for restarting).
    pub fn reset_stop(&self) {
        let _ = self.stop_tx.send(false);
    }
}

impl Default for CollectorState {
    fn default() -> Self {
        Self::new()
    }
}

/// Collection statistics for a single device.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeviceCollectionStats {
    /// Device ID/address.
    pub device_id: String,
    /// Device alias.
    pub alias: Option<String>,
    /// Poll interval in seconds.
    pub poll_interval: u64,
    /// Time of last successful poll.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_poll_at: Option<OffsetDateTime>,
    /// Time of last failed poll.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_error_at: Option<OffsetDateTime>,
    /// Last error message.
    pub last_error: Option<String>,
    /// Total successful polls.
    pub success_count: u64,
    /// Total failed polls.
    pub failure_count: u64,
    /// Whether the device is currently being polled.
    pub polling: bool,
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
    use crate::config::Config;
    use aranet_types::Status;

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

    #[tokio::test]
    async fn test_app_state_new() {
        let store = Store::open_in_memory().unwrap();
        let config = Config::default();
        let state = AppState::new(store, config);

        let config = state.config.read().await;
        assert_eq!(config.server.bind, "127.0.0.1:8080");
    }

    #[test]
    fn test_collector_state() {
        let collector = CollectorState::new();
        assert!(!collector.is_running());
        assert!(collector.started_at().is_none());

        collector.set_running(true);
        assert!(collector.is_running());
        assert!(collector.started_at().is_some());

        collector.signal_stop();
        assert!(!collector.is_running());
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

    #[test]
    fn test_collector_state_default() {
        let collector = CollectorState::default();
        assert!(!collector.is_running());
        assert!(collector.started_at().is_none());
    }

    #[test]
    fn test_collector_state_subscribe_stop() {
        let collector = CollectorState::new();

        // Get multiple receivers
        let rx1 = collector.subscribe_stop();
        let rx2 = collector.subscribe_stop();

        // Both should see the initial value (false)
        assert!(!*rx1.borrow());
        assert!(!*rx2.borrow());
    }

    #[test]
    fn test_collector_state_stop_and_reset() {
        let collector = CollectorState::new();
        let rx = collector.subscribe_stop();

        // Initially not stopped
        assert!(!*rx.borrow());

        // Signal stop
        collector.signal_stop();
        assert!(*rx.borrow());

        // Reset
        collector.reset_stop();
        assert!(!*rx.borrow());
    }

    #[test]
    fn test_collector_state_running_toggle() {
        let collector = CollectorState::new();

        assert!(!collector.is_running());
        assert!(collector.started_at().is_none());

        collector.set_running(true);
        assert!(collector.is_running());
        let started = collector.started_at();
        assert!(started.is_some());

        // Set running again - should update timestamp
        std::thread::sleep(std::time::Duration::from_secs(1));
        collector.set_running(true);
        let started2 = collector.started_at();
        assert!(started2 >= started);

        collector.set_running(false);
        assert!(!collector.is_running());
        // Note: started_at is not reset when set_running(false)
    }

    #[tokio::test]
    async fn test_collector_state_device_stats_rw_lock() {
        let collector = CollectorState::new();

        // Write to stats
        {
            let mut stats = collector.device_stats.write().await;
            stats.push(DeviceCollectionStats {
                device_id: "test-1".to_string(),
                alias: Some("Test 1".to_string()),
                poll_interval: 60,
                last_poll_at: None,
                last_error_at: None,
                last_error: None,
                success_count: 0,
                failure_count: 0,
                polling: false,
            });
        }

        // Read from stats
        let stats = collector.device_stats.read().await;
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].device_id, "test-1");
    }

    #[test]
    fn test_device_collection_stats_serialization() {
        let stats = DeviceCollectionStats {
            device_id: "AA:BB:CC:DD:EE:FF".to_string(),
            alias: Some("Kitchen Sensor".to_string()),
            poll_interval: 120,
            last_poll_at: Some(time::OffsetDateTime::now_utc()),
            last_error_at: None,
            last_error: None,
            success_count: 42,
            failure_count: 3,
            polling: true,
        };

        let json = serde_json::to_string(&stats).unwrap();

        assert!(json.contains("AA:BB:CC:DD:EE:FF"));
        assert!(json.contains("Kitchen Sensor"));
        assert!(json.contains("120"));
        assert!(json.contains("42"));
        assert!(json.contains("3"));
        assert!(json.contains("true"));
    }

    #[test]
    fn test_device_collection_stats_with_error() {
        let stats = DeviceCollectionStats {
            device_id: "test".to_string(),
            alias: None,
            poll_interval: 60,
            last_poll_at: None,
            last_error_at: Some(time::OffsetDateTime::now_utc()),
            last_error: Some("Connection timeout".to_string()),
            success_count: 10,
            failure_count: 5,
            polling: false,
        };

        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("Connection timeout"));
    }

    #[test]
    fn test_device_collection_stats_clone() {
        let original = DeviceCollectionStats {
            device_id: "clone-test".to_string(),
            alias: Some("Clone".to_string()),
            poll_interval: 90,
            last_poll_at: Some(time::OffsetDateTime::now_utc()),
            last_error_at: None,
            last_error: None,
            success_count: 100,
            failure_count: 2,
            polling: true,
        };

        let cloned = original.clone();

        assert_eq!(cloned.device_id, original.device_id);
        assert_eq!(cloned.alias, original.alias);
        assert_eq!(cloned.poll_interval, original.poll_interval);
        assert_eq!(cloned.success_count, original.success_count);
        assert_eq!(cloned.polling, original.polling);
    }

    #[test]
    fn test_device_collection_stats_debug() {
        let stats = DeviceCollectionStats {
            device_id: "debug-test".to_string(),
            alias: Some("Debug".to_string()),
            poll_interval: 60,
            last_poll_at: None,
            last_error_at: None,
            last_error: None,
            success_count: 5,
            failure_count: 1,
            polling: false,
        };

        let debug = format!("{:?}", stats);
        assert!(debug.contains("DeviceCollectionStats"));
        assert!(debug.contains("debug-test"));
        assert!(debug.contains("Debug"));
    }

    #[test]
    fn test_reading_event_clone() {
        let reading = create_test_reading("original", 750);
        let event = ReadingEvent {
            device_id: "original".to_string(),
            reading,
        };

        let cloned = event.clone();
        assert_eq!(cloned.device_id, event.device_id);
        assert_eq!(cloned.reading.co2, event.reading.co2);
    }

    #[tokio::test]
    async fn test_app_state_config_write() {
        let store = Store::open_in_memory().unwrap();
        let config = Config::default();
        let state = AppState::new(store, config);

        // Modify config
        {
            let mut config = state.config.write().await;
            config.server.bind = "0.0.0.0:9090".to_string();
        }

        // Read and verify
        let config = state.config.read().await;
        assert_eq!(config.server.bind, "0.0.0.0:9090");
    }

    #[tokio::test]
    async fn test_broadcast_channel_multiple_receivers() {
        let store = Store::open_in_memory().unwrap();
        let config = Config::default();
        let state = AppState::new(store, config);

        let mut rx1 = state.readings_tx.subscribe();
        let mut rx2 = state.readings_tx.subscribe();

        let reading = create_test_reading("multi", 888);
        let event = ReadingEvent {
            device_id: "multi".to_string(),
            reading,
        };

        state.readings_tx.send(event).unwrap();

        // Both receivers should get the message
        let received1 = rx1.recv().await.unwrap();
        let received2 = rx2.recv().await.unwrap();

        assert_eq!(received1.reading.co2, 888);
        assert_eq!(received2.reading.co2, 888);
    }

    #[tokio::test]
    async fn test_app_state_store_operations() {
        let store = Store::open_in_memory().unwrap();
        let config = Config::default();
        let state = AppState::new(store, config);

        // Insert a device via store
        {
            let store = state.store.lock().await;
            store.upsert_device("test-device", Some("Test")).unwrap();
        }

        // Query the device
        {
            let store = state.store.lock().await;
            let device = store.get_device("test-device").unwrap().unwrap();
            assert_eq!(device.name, Some("Test".to_string()));
        }
    }
}
