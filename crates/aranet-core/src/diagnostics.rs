//! Bluetooth diagnostics and troubleshooting utilities.
//!
//! This module provides tools for diagnosing Bluetooth connectivity issues
//! and gathering information about the BLE environment.
//!
//! # Example
//!
//! ```ignore
//! use aranet_core::diagnostics::{BluetoothDiagnostics, DiagnosticsCollector};
//!
//! let collector = DiagnosticsCollector::new();
//! let diagnostics = collector.collect().await?;
//!
//! println!("Platform: {:?}", diagnostics.platform);
//! println!("Adapter: {:?}", diagnostics.adapter_info);
//! println!("Connection stats: {:?}", diagnostics.connection_stats);
//! ```

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::Error;
use crate::events::DisconnectReason;
use crate::platform::{Platform, PlatformConfig};

/// Maximum number of recent errors to keep in the diagnostics buffer.
const MAX_RECENT_ERRORS: usize = 100;

/// Maximum number of recent operations to track.
const MAX_RECENT_OPERATIONS: usize = 50;

/// Bluetooth adapter state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdapterState {
    /// Adapter is available and powered on.
    Available,
    /// Adapter is available but powered off.
    PoweredOff,
    /// No adapter found.
    NotFound,
    /// Adapter state is unknown.
    Unknown,
}

/// Information about the Bluetooth adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterInfo {
    /// Adapter state.
    pub state: AdapterState,
    /// Adapter name/identifier if available.
    pub name: Option<String>,
    /// Whether the adapter supports BLE.
    pub supports_ble: bool,
    /// Number of currently connected devices (if known).
    pub connected_device_count: Option<usize>,
}

impl Default for AdapterInfo {
    fn default() -> Self {
        Self {
            state: AdapterState::Unknown,
            name: None,
            supports_ble: true,
            connected_device_count: None,
        }
    }
}

/// Statistics about connection operations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConnectionStats {
    /// Total number of connection attempts.
    pub total_attempts: u64,
    /// Number of successful connections.
    pub successful: u64,
    /// Number of failed connections.
    pub failed: u64,
    /// Average connection time in milliseconds (for successful connections).
    pub avg_connection_time_ms: Option<u64>,
    /// Minimum connection time in milliseconds.
    pub min_connection_time_ms: Option<u64>,
    /// Maximum connection time in milliseconds.
    pub max_connection_time_ms: Option<u64>,
    /// Count of disconnection reasons.
    pub disconnection_reasons: HashMap<String, u64>,
    /// Number of reconnection attempts.
    pub reconnect_attempts: u64,
    /// Number of successful reconnections.
    pub reconnect_successes: u64,
}

impl ConnectionStats {
    /// Calculate the success rate as a percentage.
    pub fn success_rate(&self) -> f64 {
        if self.total_attempts == 0 {
            0.0
        } else {
            (self.successful as f64 / self.total_attempts as f64) * 100.0
        }
    }

    /// Calculate the reconnection success rate as a percentage.
    pub fn reconnect_success_rate(&self) -> f64 {
        if self.reconnect_attempts == 0 {
            0.0
        } else {
            (self.reconnect_successes as f64 / self.reconnect_attempts as f64) * 100.0
        }
    }
}

/// Statistics about read/write operations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OperationStats {
    /// Total read operations.
    pub total_reads: u64,
    /// Successful read operations.
    pub successful_reads: u64,
    /// Failed read operations.
    pub failed_reads: u64,
    /// Total write operations.
    pub total_writes: u64,
    /// Successful write operations.
    pub successful_writes: u64,
    /// Failed write operations.
    pub failed_writes: u64,
    /// Average read time in milliseconds.
    pub avg_read_time_ms: Option<u64>,
    /// Average write time in milliseconds.
    pub avg_write_time_ms: Option<u64>,
    /// Number of timeout errors.
    pub timeout_count: u64,
}

impl OperationStats {
    /// Calculate the read success rate as a percentage.
    pub fn read_success_rate(&self) -> f64 {
        if self.total_reads == 0 {
            0.0
        } else {
            (self.successful_reads as f64 / self.total_reads as f64) * 100.0
        }
    }

    /// Calculate the write success rate as a percentage.
    pub fn write_success_rate(&self) -> f64 {
        if self.total_writes == 0 {
            0.0
        } else {
            (self.successful_writes as f64 / self.total_writes as f64) * 100.0
        }
    }
}

/// A recorded error with timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedError {
    /// When the error occurred (Unix timestamp millis).
    pub timestamp_ms: u64,
    /// Error message.
    pub message: String,
    /// Error category.
    pub category: ErrorCategory,
    /// Device identifier if applicable.
    pub device_id: Option<String>,
}

/// Categories of errors for classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCategory {
    /// Connection-related errors.
    Connection,
    /// Read/write operation errors.
    Operation,
    /// Timeout errors.
    Timeout,
    /// Device not found errors.
    DeviceNotFound,
    /// Data parsing errors.
    DataParsing,
    /// Configuration errors.
    Configuration,
    /// Other/unknown errors.
    Other,
}

impl From<&Error> for ErrorCategory {
    fn from(error: &Error) -> Self {
        match error {
            Error::ConnectionFailed { .. } | Error::NotConnected => ErrorCategory::Connection,
            Error::Timeout { .. } => ErrorCategory::Timeout,
            Error::DeviceNotFound(_) => ErrorCategory::DeviceNotFound,
            Error::InvalidData(_)
            | Error::InvalidHistoryData { .. }
            | Error::InvalidReadingFormat { .. } => ErrorCategory::DataParsing,
            Error::InvalidConfig(_) => ErrorCategory::Configuration,
            Error::CharacteristicNotFound { .. } | Error::WriteFailed { .. } => {
                ErrorCategory::Operation
            }
            Error::Bluetooth(_) | Error::Io(_) | Error::Cancelled => ErrorCategory::Other,
        }
    }
}

/// A recorded operation for timing analysis.
/// Reserved for future use in operation timing analysis.
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct RecordedOperation {
    operation_type: OperationType,
    start_time: Instant,
    duration_ms: u64,
    success: bool,
    device_id: Option<String>,
}

/// Types of operations being tracked.
/// Reserved for future use in operation timing analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum OperationType {
    Connect,
    Disconnect,
    Read,
    Write,
    Scan,
}

/// Complete Bluetooth diagnostics snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BluetoothDiagnostics {
    /// Current platform.
    pub platform: String,
    /// Platform-specific configuration.
    pub platform_config: PlatformConfigSnapshot,
    /// Adapter information.
    pub adapter_info: AdapterInfo,
    /// Connection statistics.
    pub connection_stats: ConnectionStats,
    /// Operation statistics.
    pub operation_stats: OperationStats,
    /// Recent errors (most recent first).
    pub recent_errors: Vec<RecordedError>,
    /// Timestamp when diagnostics were collected (Unix millis).
    pub collected_at: u64,
    /// Uptime of the diagnostics collector in seconds.
    pub uptime_secs: u64,
}

/// Serializable snapshot of platform configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformConfigSnapshot {
    pub recommended_scan_duration_ms: u64,
    pub recommended_connection_timeout_ms: u64,
    pub max_concurrent_connections: usize,
    pub exposes_mac_address: bool,
}

impl From<&PlatformConfig> for PlatformConfigSnapshot {
    fn from(config: &PlatformConfig) -> Self {
        Self {
            recommended_scan_duration_ms: config.recommended_scan_duration.as_millis() as u64,
            recommended_connection_timeout_ms: config.recommended_connection_timeout.as_millis()
                as u64,
            max_concurrent_connections: config.max_concurrent_connections,
            exposes_mac_address: config.exposes_mac_address,
        }
    }
}

/// Collector for Bluetooth diagnostics.
///
/// This struct accumulates statistics and errors over time, providing
/// insights into Bluetooth connectivity patterns.
pub struct DiagnosticsCollector {
    /// When the collector was created.
    start_time: Instant,
    /// Connection statistics (atomic counters).
    connection_attempts: AtomicU64,
    connection_successes: AtomicU64,
    connection_failures: AtomicU64,
    reconnect_attempts: AtomicU64,
    reconnect_successes: AtomicU64,
    /// Operation statistics (atomic counters).
    read_attempts: AtomicU64,
    read_successes: AtomicU64,
    write_attempts: AtomicU64,
    write_successes: AtomicU64,
    timeout_count: AtomicU64,
    /// Connection times for averaging (protected by RwLock).
    connection_times: RwLock<Vec<u64>>,
    read_times: RwLock<Vec<u64>>,
    write_times: RwLock<Vec<u64>>,
    /// Disconnection reason counts.
    disconnection_reasons: RwLock<HashMap<String, u64>>,
    /// Recent errors buffer.
    recent_errors: RwLock<VecDeque<RecordedError>>,
    /// Recent operations for timing analysis.
    recent_operations: RwLock<VecDeque<RecordedOperation>>,
}

impl Default for DiagnosticsCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl DiagnosticsCollector {
    /// Create a new diagnostics collector.
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            connection_attempts: AtomicU64::new(0),
            connection_successes: AtomicU64::new(0),
            connection_failures: AtomicU64::new(0),
            reconnect_attempts: AtomicU64::new(0),
            reconnect_successes: AtomicU64::new(0),
            read_attempts: AtomicU64::new(0),
            read_successes: AtomicU64::new(0),
            write_attempts: AtomicU64::new(0),
            write_successes: AtomicU64::new(0),
            timeout_count: AtomicU64::new(0),
            connection_times: RwLock::new(Vec::new()),
            read_times: RwLock::new(Vec::new()),
            write_times: RwLock::new(Vec::new()),
            disconnection_reasons: RwLock::new(HashMap::new()),
            recent_errors: RwLock::new(VecDeque::with_capacity(MAX_RECENT_ERRORS)),
            recent_operations: RwLock::new(VecDeque::with_capacity(MAX_RECENT_OPERATIONS)),
        }
    }

    /// Record a connection attempt.
    pub fn record_connection_attempt(&self) {
        self.connection_attempts.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a successful connection with duration.
    pub async fn record_connection_success(&self, duration: Duration) {
        self.connection_successes.fetch_add(1, Ordering::Relaxed);
        self.connection_times
            .write()
            .await
            .push(duration.as_millis() as u64);
    }

    /// Record a failed connection.
    pub fn record_connection_failure(&self) {
        self.connection_failures.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a reconnection attempt.
    pub fn record_reconnect_attempt(&self) {
        self.reconnect_attempts.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a successful reconnection.
    pub fn record_reconnect_success(&self) {
        self.reconnect_successes.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a read operation.
    pub async fn record_read(&self, success: bool, duration: Option<Duration>) {
        self.read_attempts.fetch_add(1, Ordering::Relaxed);
        if success {
            self.read_successes.fetch_add(1, Ordering::Relaxed);
            if let Some(d) = duration {
                self.read_times.write().await.push(d.as_millis() as u64);
            }
        }
    }

    /// Record a write operation.
    pub async fn record_write(&self, success: bool, duration: Option<Duration>) {
        self.write_attempts.fetch_add(1, Ordering::Relaxed);
        if success {
            self.write_successes.fetch_add(1, Ordering::Relaxed);
            if let Some(d) = duration {
                self.write_times.write().await.push(d.as_millis() as u64);
            }
        }
    }

    /// Record a timeout.
    pub fn record_timeout(&self) {
        self.timeout_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a disconnection with reason.
    pub async fn record_disconnection(&self, reason: &DisconnectReason) {
        let reason_str = format!("{:?}", reason);
        let mut reasons = self.disconnection_reasons.write().await;
        *reasons.entry(reason_str).or_insert(0) += 1;
    }

    /// Record an error.
    pub async fn record_error(&self, error: &Error, device_id: Option<String>) {
        let recorded = RecordedError {
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            message: error.to_string(),
            category: ErrorCategory::from(error),
            device_id,
        };

        // Track timeout specifically
        if matches!(error, Error::Timeout { .. }) {
            self.record_timeout();
        }

        let mut errors = self.recent_errors.write().await;
        if errors.len() >= MAX_RECENT_ERRORS {
            errors.pop_back();
        }
        errors.push_front(recorded);
    }

    /// Collect current diagnostics snapshot.
    pub async fn collect(&self) -> BluetoothDiagnostics {
        let platform = Platform::current();
        let platform_config = PlatformConfig::for_current_platform();

        // Calculate connection time statistics
        let connection_times = self.connection_times.read().await;
        let (avg_conn, min_conn, max_conn) = calculate_time_stats(&connection_times);

        // Calculate read/write time statistics
        let read_times = self.read_times.read().await;
        let (avg_read, _, _) = calculate_time_stats(&read_times);
        let write_times = self.write_times.read().await;
        let (avg_write, _, _) = calculate_time_stats(&write_times);

        // Build disconnection reasons map
        let disconnection_reasons = self.disconnection_reasons.read().await.clone();

        // Collect recent errors
        let recent_errors: Vec<RecordedError> =
            self.recent_errors.read().await.iter().cloned().collect();

        BluetoothDiagnostics {
            platform: format!("{:?}", platform),
            platform_config: PlatformConfigSnapshot::from(&platform_config),
            adapter_info: AdapterInfo::default(), // Would need async adapter query
            connection_stats: ConnectionStats {
                total_attempts: self.connection_attempts.load(Ordering::Relaxed),
                successful: self.connection_successes.load(Ordering::Relaxed),
                failed: self.connection_failures.load(Ordering::Relaxed),
                avg_connection_time_ms: avg_conn,
                min_connection_time_ms: min_conn,
                max_connection_time_ms: max_conn,
                disconnection_reasons,
                reconnect_attempts: self.reconnect_attempts.load(Ordering::Relaxed),
                reconnect_successes: self.reconnect_successes.load(Ordering::Relaxed),
            },
            operation_stats: OperationStats {
                total_reads: self.read_attempts.load(Ordering::Relaxed),
                successful_reads: self.read_successes.load(Ordering::Relaxed),
                failed_reads: self.read_attempts.load(Ordering::Relaxed)
                    - self.read_successes.load(Ordering::Relaxed),
                total_writes: self.write_attempts.load(Ordering::Relaxed),
                successful_writes: self.write_successes.load(Ordering::Relaxed),
                failed_writes: self.write_attempts.load(Ordering::Relaxed)
                    - self.write_successes.load(Ordering::Relaxed),
                avg_read_time_ms: avg_read,
                avg_write_time_ms: avg_write,
                timeout_count: self.timeout_count.load(Ordering::Relaxed),
            },
            recent_errors,
            collected_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            uptime_secs: self.start_time.elapsed().as_secs(),
        }
    }

    /// Reset all statistics.
    pub async fn reset(&self) {
        self.connection_attempts.store(0, Ordering::Relaxed);
        self.connection_successes.store(0, Ordering::Relaxed);
        self.connection_failures.store(0, Ordering::Relaxed);
        self.reconnect_attempts.store(0, Ordering::Relaxed);
        self.reconnect_successes.store(0, Ordering::Relaxed);
        self.read_attempts.store(0, Ordering::Relaxed);
        self.read_successes.store(0, Ordering::Relaxed);
        self.write_attempts.store(0, Ordering::Relaxed);
        self.write_successes.store(0, Ordering::Relaxed);
        self.timeout_count.store(0, Ordering::Relaxed);

        self.connection_times.write().await.clear();
        self.read_times.write().await.clear();
        self.write_times.write().await.clear();
        self.disconnection_reasons.write().await.clear();
        self.recent_errors.write().await.clear();
        self.recent_operations.write().await.clear();
    }

    /// Get a summary string suitable for logging.
    pub async fn summary(&self) -> String {
        let diag = self.collect().await;
        format!(
            "Connections: {}/{} ({:.1}% success), Reconnects: {}/{} ({:.1}% success), \
             Reads: {}/{} ({:.1}% success), Writes: {}/{} ({:.1}% success), \
             Timeouts: {}, Errors: {}",
            diag.connection_stats.successful,
            diag.connection_stats.total_attempts,
            diag.connection_stats.success_rate(),
            diag.connection_stats.reconnect_successes,
            diag.connection_stats.reconnect_attempts,
            diag.connection_stats.reconnect_success_rate(),
            diag.operation_stats.successful_reads,
            diag.operation_stats.total_reads,
            diag.operation_stats.read_success_rate(),
            diag.operation_stats.successful_writes,
            diag.operation_stats.total_writes,
            diag.operation_stats.write_success_rate(),
            diag.operation_stats.timeout_count,
            diag.recent_errors.len(),
        )
    }
}

/// Calculate min, max, and average from a slice of times.
fn calculate_time_stats(times: &[u64]) -> (Option<u64>, Option<u64>, Option<u64>) {
    if times.is_empty() {
        return (None, None, None);
    }

    let sum: u64 = times.iter().sum();
    let avg = sum / times.len() as u64;
    let min = *times.iter().min().unwrap();
    let max = *times.iter().max().unwrap();

    (Some(avg), Some(min), Some(max))
}

/// Global diagnostics collector instance.
///
/// This can be used to collect diagnostics across the entire application.
pub static GLOBAL_DIAGNOSTICS: std::sync::LazyLock<Arc<DiagnosticsCollector>> =
    std::sync::LazyLock::new(|| Arc::new(DiagnosticsCollector::new()));

/// Get a reference to the global diagnostics collector.
pub fn global_diagnostics() -> &'static Arc<DiagnosticsCollector> {
    &GLOBAL_DIAGNOSTICS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_stats_success_rate() {
        let mut stats = ConnectionStats::default();
        assert_eq!(stats.success_rate(), 0.0);

        stats.total_attempts = 10;
        stats.successful = 8;
        assert!((stats.success_rate() - 80.0).abs() < 0.01);
    }

    #[test]
    fn test_operation_stats_success_rate() {
        let mut stats = OperationStats::default();
        assert_eq!(stats.read_success_rate(), 0.0);

        stats.total_reads = 100;
        stats.successful_reads = 95;
        assert!((stats.read_success_rate() - 95.0).abs() < 0.01);
    }

    #[test]
    fn test_error_category_from_error() {
        let timeout_err = Error::Timeout {
            operation: "test".to_string(),
            duration: Duration::from_secs(1),
        };
        assert_eq!(ErrorCategory::from(&timeout_err), ErrorCategory::Timeout);

        let not_connected = Error::NotConnected;
        assert_eq!(
            ErrorCategory::from(&not_connected),
            ErrorCategory::Connection
        );
    }

    #[tokio::test]
    async fn test_diagnostics_collector() {
        let collector = DiagnosticsCollector::new();

        collector.record_connection_attempt();
        collector
            .record_connection_success(Duration::from_millis(500))
            .await;

        let diag = collector.collect().await;
        assert_eq!(diag.connection_stats.total_attempts, 1);
        assert_eq!(diag.connection_stats.successful, 1);
        assert_eq!(diag.connection_stats.avg_connection_time_ms, Some(500));
    }

    #[tokio::test]
    async fn test_diagnostics_collector_reset() {
        let collector = DiagnosticsCollector::new();

        collector.record_connection_attempt();
        collector.record_connection_failure();

        let diag = collector.collect().await;
        assert_eq!(diag.connection_stats.failed, 1);

        collector.reset().await;

        let diag = collector.collect().await;
        assert_eq!(diag.connection_stats.failed, 0);
    }
}
