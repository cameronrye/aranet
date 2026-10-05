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

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use aranet_store::Store;
use time::OffsetDateTime;
use tokio::sync::{Mutex, RwLock, Semaphore, broadcast, watch};
use tokio_util::task::TaskTracker;

use crate::collector::{BleDeviceReader, DeviceReader, Lifecycle};
use crate::config::{Config, default_config_path};

/// Shared application state.
pub struct AppState {
    /// The data store (wrapped in Mutex for thread-safe access).
    pub store: Mutex<Store>,
    /// File-backed database path for opening parallel read connections.
    ///
    /// In-memory stores leave this as `None` and fall back to the shared mutex.
    store_path: Option<PathBuf>,
    /// Configuration (RwLock for runtime updates).
    pub config: RwLock<Config>,
    /// Path to the configuration file (for saving changes).
    pub config_path: PathBuf,
    /// Broadcast channel for real-time reading updates.
    pub readings_tx: broadcast::Sender<ReadingEvent>,
    /// Semaphore to serialize BLE adapter access (only one device at a time).
    pub ble_semaphore: Semaphore,
    /// Collector control state.
    pub collector: CollectorState,
    /// Total number of broadcast messages dropped due to slow subscribers.
    pub ws_messages_dropped: AtomicU64,
    /// Global application shutdown signal for background integrations.
    shutdown_tx: watch::Sender<bool>,
    /// Receiver side of the application shutdown signal.
    shutdown_rx: watch::Receiver<bool>,
}

impl AppState {
    /// Create new application state.
    ///
    /// Configuration changes made through this state, such as a device added
    /// through the API, are saved to the per-user file from
    /// [`default_config_path()`]: the real `server.toml`. Tests must not use
    /// this constructor; use [`Self::with_config_path`] for any other location.
    ///
    /// The broadcast channel buffer size is determined by `config.server.broadcast_buffer`.
    /// If the buffer fills (slow subscribers), old messages are dropped without blocking.
    pub fn new(store: Store, config: Config) -> Arc<Self> {
        Self::with_config_path(store, config, default_config_path())
    }

    /// Create new application state with a custom config path.
    pub fn with_config_path(store: Store, config: Config, config_path: PathBuf) -> Arc<Self> {
        Self::build(store, config, config_path, CollectorState::new())
    }

    /// Like [`Self::with_config_path`], with a prepared collector state (for
    /// example one whose collector reads devices through a fake).
    #[cfg(test)]
    pub(crate) fn with_collector(
        store: Store,
        config: Config,
        config_path: PathBuf,
        collector: CollectorState,
    ) -> Arc<Self> {
        Self::build(store, config, config_path, collector)
    }

    fn build(
        store: Store,
        config: Config,
        config_path: PathBuf,
        collector: CollectorState,
    ) -> Arc<Self> {
        let buffer_size = config.server.broadcast_buffer;
        let (readings_tx, _) = broadcast::channel(buffer_size);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let store_path = store.database_path().map(PathBuf::from);
        Arc::new(Self {
            store: Mutex::new(store),
            store_path,
            config: RwLock::new(config),
            config_path,
            readings_tx,
            ble_semaphore: Semaphore::new(1),
            collector,
            ws_messages_dropped: AtomicU64::new(0),
            shutdown_tx,
            shutdown_rx,
        })
    }

    /// Save the current configuration to disk.
    ///
    /// This should be called after any configuration changes made via the API.
    pub async fn save_config(&self) -> Result<(), crate::config::ConfigError> {
        let config = self.config.read().await;
        config.save(&self.config_path)
    }

    /// Execute a read-only store operation.
    ///
    /// File-backed stores open a separate connection per call so concurrent API reads
    /// do not block on the shared write mutex. In-memory stores fall back to the shared
    /// store because they cannot be reopened.
    pub async fn with_store_read<T, F>(&self, f: F) -> aranet_store::Result<T>
    where
        F: FnOnce(&Store) -> aranet_store::Result<T>,
    {
        if let Some(path) = &self.store_path {
            let store = Store::open(path)?;
            f(&store)
        } else {
            let store = self.store.lock().await;
            f(&store)
        }
    }

    /// Execute a write-capable store operation through the shared connection.
    pub async fn with_store_write<T, F>(&self, f: F) -> aranet_store::Result<T>
    where
        F: FnOnce(&Store) -> aranet_store::Result<T>,
    {
        let store = self.store.lock().await;
        f(&store)
    }

    /// Signal that the device configuration has changed.
    ///
    /// A started collector applies the change, also while it has no devices,
    /// so devices added to an empty config are polled. It restarts only the
    /// devices that were added, removed, or had their address or poll interval
    /// changed. A stopped collector ignores the signal.
    pub async fn on_devices_changed(&self) {
        self.collector.signal_reload();
    }

    /// Subscribe to the application shutdown signal.
    pub fn subscribe_shutdown(&self) -> watch::Receiver<bool> {
        self.shutdown_rx.clone()
    }

    /// Signal all long-lived background integrations to stop.
    pub fn signal_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// State for tracking and controlling the collector.
///
/// The collector's tasks are started and stopped through
/// [`Collector`](crate::collector::Collector).
pub struct CollectorState {
    /// Whether the collector is polling at least one device.
    running: AtomicBool,
    /// When the collector last went from not running to running (Unix
    /// timestamp; 0 while it isn't running).
    started_at: AtomicU64,
    /// Reload signal; the value counts device configuration changes.
    reload_tx: watch::Sender<u64>,
    /// Per-device collection stats, in config order.
    pub device_stats: RwLock<Vec<DeviceCollectionStats>>,
    /// The collector's current generation of tasks. Only `Collector::start`,
    /// `Collector::stop` (with `Collector::shutdown`) and the reload watcher
    /// lock it.
    pub(crate) lifecycle: Mutex<Lifecycle>,
    /// Bluetooth polls in flight, including those a stopped collector left to
    /// finish. Shutdown waits for them.
    pub(crate) polls: TaskTracker,
    /// How the collector reads devices: over Bluetooth, or a fake in tests.
    reader: Arc<dyn DeviceReader>,
    /// Desktop alerts and their per-device cooldowns.
    #[cfg(feature = "notifications")]
    pub(crate) notifier: crate::collector::Notifier,
}

impl CollectorState {
    /// Create a new collector state.
    pub fn new() -> Self {
        Self {
            running: AtomicBool::new(false),
            started_at: AtomicU64::new(0),
            reload_tx: watch::Sender::new(0),
            device_stats: RwLock::new(Vec::new()),
            lifecycle: Mutex::new(Lifecycle::default()),
            polls: TaskTracker::new(),
            reader: Arc::new(BleDeviceReader),
            #[cfg(feature = "notifications")]
            notifier: crate::collector::Notifier::desktop(),
        }
    }

    /// A collector state whose collector reads devices through `reader`.
    #[cfg(test)]
    pub(crate) fn with_reader(reader: Arc<dyn DeviceReader>) -> Self {
        Self {
            reader,
            ..Self::new()
        }
    }

    /// The reader the collector polls devices through.
    pub(crate) fn reader(&self) -> &Arc<dyn DeviceReader> {
        &self.reader
    }

    /// Check if the collector is running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Record whether the collector is polling any device.
    ///
    /// The start time is set only when the collector goes from not running to
    /// running, so device edits don't reset the uptime, and cleared when it
    /// stops.
    pub(crate) fn set_running(&self, running: bool) {
        match (self.running.swap(running, Ordering::SeqCst), running) {
            (false, true) => {
                let now = OffsetDateTime::now_utc().unix_timestamp() as u64;
                self.started_at.store(now, Ordering::SeqCst);
            }
            (true, false) => self.started_at.store(0, Ordering::SeqCst),
            _ => {}
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

    /// Get a receiver for the reload signal.
    ///
    /// The receiver is woken only by reloads signalled after this call.
    pub fn subscribe_reload(&self) -> watch::Receiver<u64> {
        self.reload_tx.subscribe()
    }

    /// Signal the collector to reload its configuration.
    ///
    /// This is used when devices are added, removed, or modified via the API.
    /// A started collector restarts only the devices whose address or poll
    /// interval changed. The signal is recorded even while nothing listens.
    pub fn signal_reload(&self) {
        self.reload_tx
            .send_modify(|count| *count = count.wrapping_add(1));
    }
}

impl Default for CollectorState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(test, feature = "notifications"))]
impl CollectorState {
    /// This state, with its alerts going to `notifier` instead of the desktop.
    pub(crate) fn with_notifier(self, notifier: crate::collector::Notifier) -> Self {
        Self { notifier, ..self }
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
    /// Duration of the last poll in milliseconds.
    pub last_poll_duration_ms: Option<u64>,
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
    use crate::test_support::TestState;
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
            radon_avg_24h: None,
            radon_avg_7d: None,
            radon_avg_30d: None,
            captured_at: time::OffsetDateTime::now_utc(),
        }
    }

    #[tokio::test]
    async fn test_app_state_new() {
        let t = TestState::in_memory();
        let state = Arc::clone(&t.state);

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

        collector.set_running(false);
        assert!(!collector.is_running());
        assert!(collector.started_at().is_none());
    }

    #[tokio::test]
    async fn test_app_state_store_access() {
        let t = TestState::in_memory();
        let state = Arc::clone(&t.state);

        let devices = state
            .with_store_read(|store| store.list_devices())
            .await
            .unwrap();
        assert!(devices.is_empty());
    }

    #[tokio::test]
    async fn test_app_state_broadcast_channel() {
        let t = TestState::in_memory();
        let state = Arc::clone(&t.state);

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
    fn subscribe_reload_does_not_replay_earlier_signals() {
        let collector = CollectorState::new();
        collector.signal_reload();

        let rx = collector.subscribe_reload();
        assert!(!rx.has_changed().unwrap());
    }

    #[test]
    fn test_collector_state_running_toggle() {
        let collector = CollectorState::new();

        assert!(!collector.is_running());
        assert!(collector.started_at().is_none());

        collector.set_running(true);
        assert!(collector.is_running());
        assert!(collector.started_at().is_some());

        // Marking a running collector as running keeps its start time. Pretend
        // it has been running since 2023-11-14.
        collector.started_at.store(1_700_000_000, Ordering::SeqCst);
        collector.set_running(true);
        assert_eq!(
            collector.started_at().map(|t| t.unix_timestamp()),
            Some(1_700_000_000)
        );

        collector.set_running(false);
        assert!(!collector.is_running());
        assert!(collector.started_at().is_none());
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
                last_poll_duration_ms: None,
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
            last_poll_duration_ms: None,
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
            last_poll_duration_ms: None,
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
            last_poll_duration_ms: None,
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
            last_poll_duration_ms: None,
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
        let t = TestState::in_memory();
        let state = Arc::clone(&t.state);

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
        let t = TestState::in_memory();
        let state = Arc::clone(&t.state);

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
        let t = TestState::in_memory();
        let state = Arc::clone(&t.state);

        state
            .with_store_write(|store| store.upsert_device("test-device", Some("Test")).map(|_| ()))
            .await
            .unwrap();

        let device = state
            .with_store_read(|store| store.get_device("test-device"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(device.name, Some("Test".to_string()));
    }

    #[test]
    fn test_collector_state_reload_signal() {
        let collector = CollectorState::new();
        let rx = collector.subscribe_reload();

        // Initial value
        assert_eq!(*rx.borrow(), 0);

        // Signal reload
        collector.signal_reload();
        assert_eq!(*rx.borrow(), 1);

        // Signal reload again
        collector.signal_reload();
        assert_eq!(*rx.borrow(), 2);
    }
}

/// Database access through `with_store_read` and `with_store_write`.
#[cfg(test)]
mod store_tests {
    use aranet_store::Store;

    use super::AppState;
    use crate::config::Config;

    #[tokio::test]
    async fn memory_database_reads_see_writes() {
        // `aranet-service run --database :memory:` opens the store like this.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(":memory:").unwrap();
        let state =
            AppState::with_config_path(store, Config::default(), dir.path().join("server.toml"));

        state
            .with_store_write(|store| store.upsert_device("Aranet4 17C3C", None).map(|_| ()))
            .await
            .unwrap();
        let devices = state
            .with_store_read(|store| store.list_devices())
            .await
            .unwrap();
        assert_eq!(
            devices.len(),
            1,
            "reads must see the database the writes went to"
        );
    }
}
