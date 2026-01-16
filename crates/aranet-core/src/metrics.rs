//! Connection and operation metrics tracking.
//!
//! This module provides types for tracking BLE connection statistics,
//! operation latencies, and retry counts.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Metrics for a single operation type.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OperationMetrics {
    /// Total number of operations.
    pub count: u64,
    /// Number of successful operations.
    pub success_count: u64,
    /// Number of failed operations.
    pub failure_count: u64,
    /// Total duration of all operations.
    pub total_duration_ms: u64,
    /// Minimum operation duration.
    pub min_duration_ms: Option<u64>,
    /// Maximum operation duration.
    pub max_duration_ms: Option<u64>,
    /// Average operation duration.
    pub avg_duration_ms: Option<f64>,
}

/// Thread-safe atomic operation metrics tracker.
#[derive(Debug, Default)]
pub struct AtomicOperationMetrics {
    count: AtomicU64,
    success_count: AtomicU64,
    failure_count: AtomicU64,
    total_duration_ms: AtomicU64,
    min_duration_ms: AtomicU64,
    max_duration_ms: AtomicU64,
}

impl AtomicOperationMetrics {
    /// Create new empty metrics.
    pub fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            success_count: AtomicU64::new(0),
            failure_count: AtomicU64::new(0),
            total_duration_ms: AtomicU64::new(0),
            min_duration_ms: AtomicU64::new(u64::MAX),
            max_duration_ms: AtomicU64::new(0),
        }
    }

    /// Record a successful operation.
    pub fn record_success(&self, duration: Duration) {
        let ms = duration.as_millis() as u64;
        self.count.fetch_add(1, Ordering::Relaxed);
        self.success_count.fetch_add(1, Ordering::Relaxed);
        self.total_duration_ms.fetch_add(ms, Ordering::Relaxed);
        self.update_min_max(ms);
    }

    /// Record a failed operation.
    pub fn record_failure(&self, duration: Duration) {
        let ms = duration.as_millis() as u64;
        self.count.fetch_add(1, Ordering::Relaxed);
        self.failure_count.fetch_add(1, Ordering::Relaxed);
        self.total_duration_ms.fetch_add(ms, Ordering::Relaxed);
        self.update_min_max(ms);
    }

    fn update_min_max(&self, ms: u64) {
        // Update min (compare-and-swap loop)
        let mut current = self.min_duration_ms.load(Ordering::Relaxed);
        while ms < current {
            match self.min_duration_ms.compare_exchange_weak(
                current,
                ms,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(c) => current = c,
            }
        }

        // Update max
        let mut current = self.max_duration_ms.load(Ordering::Relaxed);
        while ms > current {
            match self.max_duration_ms.compare_exchange_weak(
                current,
                ms,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(c) => current = c,
            }
        }
    }

    /// Get a snapshot of the current metrics.
    pub fn snapshot(&self) -> OperationMetrics {
        let count = self.count.load(Ordering::Relaxed);
        let success_count = self.success_count.load(Ordering::Relaxed);
        let failure_count = self.failure_count.load(Ordering::Relaxed);
        let total_duration_ms = self.total_duration_ms.load(Ordering::Relaxed);
        let min = self.min_duration_ms.load(Ordering::Relaxed);
        let max = self.max_duration_ms.load(Ordering::Relaxed);

        let min_duration_ms = if min == u64::MAX { None } else { Some(min) };
        let max_duration_ms = if max == 0 && count == 0 {
            None
        } else {
            Some(max)
        };
        let avg_duration_ms = if count > 0 {
            Some(total_duration_ms as f64 / count as f64)
        } else {
            None
        };

        OperationMetrics {
            count,
            success_count,
            failure_count,
            total_duration_ms,
            min_duration_ms,
            max_duration_ms,
            avg_duration_ms,
        }
    }

    /// Reset all metrics to zero.
    pub fn reset(&self) {
        self.count.store(0, Ordering::Relaxed);
        self.success_count.store(0, Ordering::Relaxed);
        self.failure_count.store(0, Ordering::Relaxed);
        self.total_duration_ms.store(0, Ordering::Relaxed);
        self.min_duration_ms.store(u64::MAX, Ordering::Relaxed);
        self.max_duration_ms.store(0, Ordering::Relaxed);
    }
}

/// Comprehensive connection metrics for a device.
#[derive(Debug)]
pub struct ConnectionMetrics {
    /// When the connection was established.
    connected_at: Option<Instant>,
    /// Connection attempts.
    pub connect: AtomicOperationMetrics,
    /// Disconnect events.
    pub disconnect: AtomicOperationMetrics,
    /// Read operations.
    pub reads: AtomicOperationMetrics,
    /// Write operations.
    pub writes: AtomicOperationMetrics,
    /// Total reconnection attempts.
    pub reconnects: AtomicOperationMetrics,
    /// Total bytes read.
    bytes_read: AtomicU64,
    /// Total bytes written.
    bytes_written: AtomicU64,
}

impl Default for ConnectionMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectionMetrics {
    /// Create new empty connection metrics.
    pub fn new() -> Self {
        Self {
            connected_at: None,
            connect: AtomicOperationMetrics::new(),
            disconnect: AtomicOperationMetrics::new(),
            reads: AtomicOperationMetrics::new(),
            writes: AtomicOperationMetrics::new(),
            reconnects: AtomicOperationMetrics::new(),
            bytes_read: AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
        }
    }

    /// Create shared connection metrics.
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Mark the connection as established.
    pub fn mark_connected(&mut self) {
        self.connected_at = Some(Instant::now());
    }

    /// Get the connection uptime.
    pub fn uptime(&self) -> Option<Duration> {
        self.connected_at.map(|t| t.elapsed())
    }

    /// Record bytes read.
    pub fn record_bytes_read(&self, bytes: u64) {
        self.bytes_read.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Record bytes written.
    pub fn record_bytes_written(&self, bytes: u64) {
        self.bytes_written.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Get total bytes read.
    pub fn total_bytes_read(&self) -> u64 {
        self.bytes_read.load(Ordering::Relaxed)
    }

    /// Get total bytes written.
    pub fn total_bytes_written(&self) -> u64 {
        self.bytes_written.load(Ordering::Relaxed)
    }

    /// Get a summary of all metrics.
    pub fn summary(&self) -> ConnectionMetricsSummary {
        ConnectionMetricsSummary {
            uptime_ms: self.uptime().map(|d| d.as_millis() as u64),
            connect: self.connect.snapshot(),
            disconnect: self.disconnect.snapshot(),
            reads: self.reads.snapshot(),
            writes: self.writes.snapshot(),
            reconnects: self.reconnects.snapshot(),
            bytes_read: self.total_bytes_read(),
            bytes_written: self.total_bytes_written(),
        }
    }

    /// Reset all metrics.
    pub fn reset(&mut self) {
        self.connected_at = None;
        self.connect.reset();
        self.disconnect.reset();
        self.reads.reset();
        self.writes.reset();
        self.reconnects.reset();
        self.bytes_read.store(0, Ordering::Relaxed);
        self.bytes_written.store(0, Ordering::Relaxed);
    }
}

/// Serializable summary of connection metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionMetricsSummary {
    /// Connection uptime in milliseconds.
    pub uptime_ms: Option<u64>,
    /// Connection attempt metrics.
    pub connect: OperationMetrics,
    /// Disconnect metrics.
    pub disconnect: OperationMetrics,
    /// Read operation metrics.
    pub reads: OperationMetrics,
    /// Write operation metrics.
    pub writes: OperationMetrics,
    /// Reconnection metrics.
    pub reconnects: OperationMetrics,
    /// Total bytes read.
    pub bytes_read: u64,
    /// Total bytes written.
    pub bytes_written: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operation_metrics_success() {
        let metrics = AtomicOperationMetrics::new();
        metrics.record_success(Duration::from_millis(100));
        metrics.record_success(Duration::from_millis(200));

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.count, 2);
        assert_eq!(snapshot.success_count, 2);
        assert_eq!(snapshot.failure_count, 0);
        assert_eq!(snapshot.min_duration_ms, Some(100));
        assert_eq!(snapshot.max_duration_ms, Some(200));
    }

    #[test]
    fn test_operation_metrics_failure() {
        let metrics = AtomicOperationMetrics::new();
        metrics.record_failure(Duration::from_millis(50));

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.count, 1);
        assert_eq!(snapshot.success_count, 0);
        assert_eq!(snapshot.failure_count, 1);
    }

    #[test]
    fn test_operation_metrics_reset() {
        let metrics = AtomicOperationMetrics::new();
        metrics.record_success(Duration::from_millis(100));
        metrics.reset();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.count, 0);
        assert_eq!(snapshot.success_count, 0);
    }

    #[test]
    fn test_connection_metrics() {
        let mut metrics = ConnectionMetrics::new();
        metrics.mark_connected();
        metrics.reads.record_success(Duration::from_millis(10));
        metrics.record_bytes_read(100);

        assert!(metrics.uptime().is_some());
        assert_eq!(metrics.total_bytes_read(), 100);

        let summary = metrics.summary();
        assert_eq!(summary.reads.count, 1);
        assert_eq!(summary.bytes_read, 100);
    }
}
