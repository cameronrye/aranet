//! Query builders for readings and history.
//!
//! This module provides fluent query builders for filtering and paginating
//! stored sensor data. Both [`ReadingQuery`] and [`HistoryQuery`] follow
//! the builder pattern for ergonomic query construction.
//!
//! # Example
//!
//! ```
//! use aranet_store::{Store, ReadingQuery, HistoryQuery};
//! use time::{OffsetDateTime, Duration};
//!
//! let store = Store::open_in_memory()?;
//! let yesterday = OffsetDateTime::now_utc() - Duration::hours(24);
//!
//! // Query recent readings with pagination
//! let query = ReadingQuery::new()
//!     .device("Aranet4 17C3C")
//!     .since(yesterday)
//!     .limit(50)
//!     .offset(0);
//!
//! let readings = store.query_readings(&query)?;
//!
//! // Query all history for export
//! let history_query = HistoryQuery::new()
//!     .device("Aranet4 17C3C")
//!     .oldest_first();
//!
//! let history = store.query_history(&history_query)?;
//! # Ok::<(), aranet_store::Error>(())
//! ```

use time::OffsetDateTime;

/// Fluent query builder for current readings.
///
/// Use this to construct queries for [`Store::query_readings`](crate::Store::query_readings).
/// All filter methods are optional and can be chained in any order.
///
/// By default, queries return results ordered by `captured_at` descending
/// (newest first).
///
/// # Example
///
/// ```
/// use aranet_store::ReadingQuery;
/// use time::{OffsetDateTime, Duration};
///
/// let now = OffsetDateTime::now_utc();
///
/// // Query last hour's readings for a device
/// let query = ReadingQuery::new()
///     .device("Aranet4 17C3C")
///     .since(now - Duration::hours(1))
///     .limit(100);
///
/// // Query with pagination
/// let page_2 = ReadingQuery::new()
///     .device("Aranet4 17C3C")
///     .limit(50)
///     .offset(50);
///
/// // Query oldest first (chronological order)
/// let chronological = ReadingQuery::new()
///     .device("Aranet4 17C3C")
///     .oldest_first();
/// ```
#[derive(Debug, Default, Clone)]
pub struct ReadingQuery {
    /// Filter by device ID.
    pub device_id: Option<String>,
    /// Filter readings after this time.
    pub since: Option<OffsetDateTime>,
    /// Filter readings before this time.
    pub until: Option<OffsetDateTime>,
    /// Maximum number of results.
    pub limit: Option<u32>,
    /// Offset for pagination.
    pub offset: Option<u32>,
    /// Order by captured_at descending (newest first).
    pub newest_first: bool,
}

impl ReadingQuery {
    /// Create a new query with default settings.
    ///
    /// Default behavior:
    /// - No device filter (all devices)
    /// - No time range filter
    /// - No limit (all matching records)
    /// - Ordered by newest first
    pub fn new() -> Self {
        Self {
            newest_first: true,
            ..Default::default()
        }
    }

    /// Filter by device ID.
    ///
    /// Only include readings from the specified device.
    pub fn device(mut self, device_id: &str) -> Self {
        self.device_id = Some(device_id.to_string());
        self
    }

    /// Filter to readings captured at or after this time.
    ///
    /// Useful for querying "last N hours" or "since last sync".
    pub fn since(mut self, time: OffsetDateTime) -> Self {
        self.since = Some(time);
        self
    }

    /// Filter to readings captured at or before this time.
    ///
    /// Use with `since()` to query a specific time range.
    pub fn until(mut self, time: OffsetDateTime) -> Self {
        self.until = Some(time);
        self
    }

    /// Limit the maximum number of results returned.
    ///
    /// Use with `offset()` for pagination.
    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Skip the first N results.
    ///
    /// Use with `limit()` for pagination. For example, to get page 2
    /// with 50 items per page: `.limit(50).offset(50)`.
    pub fn offset(mut self, offset: u32) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Order results by oldest first (ascending by `captured_at`).
    ///
    /// By default, queries return newest first. Use this for chronological
    /// ordering, useful when exporting or processing data sequentially.
    pub fn oldest_first(mut self) -> Self {
        self.newest_first = false;
        self
    }

    /// Build the SQL WHERE clause and parameters.
    pub(crate) fn build_where(&self) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
        let mut conditions = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(ref device_id) = self.device_id {
            conditions.push("device_id = ?");
            params.push(Box::new(device_id.clone()));
        }

        if let Some(since) = self.since {
            conditions.push("captured_at >= ?");
            params.push(Box::new(since.unix_timestamp()));
        }

        if let Some(until) = self.until {
            conditions.push("captured_at <= ?");
            params.push(Box::new(until.unix_timestamp()));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        (where_clause, params)
    }

    /// Build the full SQL query.
    pub(crate) fn build_sql(&self) -> String {
        let (where_clause, _) = self.build_where();
        let order = if self.newest_first { "DESC" } else { "ASC" };

        let mut sql = format!(
            "SELECT id, device_id, captured_at, co2, temperature, pressure, humidity, \
             battery, status, radon, radiation_rate, radiation_total \
             FROM readings {} ORDER BY captured_at {}",
            where_clause, order
        );

        if let Some(limit) = self.limit {
            sql.push_str(&format!(" LIMIT {}", limit));
        }

        if let Some(offset) = self.offset {
            sql.push_str(&format!(" OFFSET {}", offset));
        }

        sql
    }
}

/// Fluent query builder for history records.
///
/// Use this to construct queries for [`Store::query_history`](crate::Store::query_history),
/// [`Store::history_stats`](crate::Store::history_stats), and export methods.
/// All filter methods are optional and can be chained in any order.
///
/// By default, queries return results ordered by `timestamp` descending
/// (newest first).
///
/// # Example
///
/// ```
/// use aranet_store::HistoryQuery;
/// use time::{OffsetDateTime, Duration};
///
/// let now = OffsetDateTime::now_utc();
///
/// // Query last week's history
/// let query = HistoryQuery::new()
///     .device("Aranet4 17C3C")
///     .since(now - Duration::days(7));
///
/// // Query specific date range for export
/// let export_query = HistoryQuery::new()
///     .device("Aranet4 17C3C")
///     .since(now - Duration::days(30))
///     .until(now - Duration::days(7))
///     .oldest_first();
/// ```
#[derive(Debug, Default, Clone)]
pub struct HistoryQuery {
    /// Filter by device ID (optional).
    pub device_id: Option<String>,
    /// Include only records at or after this time (optional).
    pub since: Option<OffsetDateTime>,
    /// Include only records at or before this time (optional).
    pub until: Option<OffsetDateTime>,
    /// Maximum number of results to return (optional).
    pub limit: Option<u32>,
    /// Number of results to skip for pagination (optional).
    pub offset: Option<u32>,
    /// If true, order by timestamp descending (newest first). Default: true.
    pub newest_first: bool,
}

impl HistoryQuery {
    /// Create a new query with default settings.
    ///
    /// Default behavior:
    /// - No device filter (all devices)
    /// - No time range filter
    /// - No limit (all matching records)
    /// - Ordered by newest first
    pub fn new() -> Self {
        Self {
            newest_first: true,
            ..Default::default()
        }
    }

    /// Filter by device ID.
    ///
    /// Only include history records from the specified device.
    pub fn device(mut self, device_id: &str) -> Self {
        self.device_id = Some(device_id.to_string());
        self
    }

    /// Filter to records at or after this time.
    ///
    /// Useful for querying "last N days" or data after a specific point.
    pub fn since(mut self, time: OffsetDateTime) -> Self {
        self.since = Some(time);
        self
    }

    /// Filter to records at or before this time.
    ///
    /// Use with `since()` to query a specific time range.
    pub fn until(mut self, time: OffsetDateTime) -> Self {
        self.until = Some(time);
        self
    }

    /// Limit the maximum number of results returned.
    ///
    /// Use with `offset()` for pagination.
    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Skip the first N results.
    ///
    /// Use with `limit()` for pagination. For example, to get page 3
    /// with 100 items per page: `.limit(100).offset(200)`.
    pub fn offset(mut self, offset: u32) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Order results by oldest first (ascending by `timestamp`).
    ///
    /// By default, queries return newest first. Use this for chronological
    /// ordering, which is useful for CSV export or time-series analysis.
    pub fn oldest_first(mut self) -> Self {
        self.newest_first = false;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    // ==================== ReadingQuery Tests ====================

    #[test]
    fn test_reading_query_new_defaults() {
        let query = ReadingQuery::new();
        assert!(query.device_id.is_none());
        assert!(query.since.is_none());
        assert!(query.until.is_none());
        assert!(query.limit.is_none());
        assert!(query.offset.is_none());
        assert!(query.newest_first);
    }

    #[test]
    fn test_reading_query_default_is_different_from_new() {
        let default_query = ReadingQuery::default();
        let new_query = ReadingQuery::new();

        // Default doesn't set newest_first, but new() does
        assert!(!default_query.newest_first);
        assert!(new_query.newest_first);
    }

    #[test]
    fn test_reading_query_device_filter() {
        let query = ReadingQuery::new().device("test-device-123");
        assert_eq!(query.device_id, Some("test-device-123".to_string()));
    }

    #[test]
    fn test_reading_query_since_filter() {
        let time = datetime!(2024-01-15 10:30:00 UTC);
        let query = ReadingQuery::new().since(time);
        assert_eq!(query.since, Some(time));
    }

    #[test]
    fn test_reading_query_until_filter() {
        let time = datetime!(2024-01-15 18:30:00 UTC);
        let query = ReadingQuery::new().until(time);
        assert_eq!(query.until, Some(time));
    }

    #[test]
    fn test_reading_query_limit() {
        let query = ReadingQuery::new().limit(100);
        assert_eq!(query.limit, Some(100));
    }

    #[test]
    fn test_reading_query_offset() {
        let query = ReadingQuery::new().offset(50);
        assert_eq!(query.offset, Some(50));
    }

    #[test]
    fn test_reading_query_oldest_first() {
        let query = ReadingQuery::new().oldest_first();
        assert!(!query.newest_first);
    }

    #[test]
    fn test_reading_query_chaining() {
        let since = datetime!(2024-01-01 00:00:00 UTC);
        let until = datetime!(2024-12-31 23:59:59 UTC);

        let query = ReadingQuery::new()
            .device("my-device")
            .since(since)
            .until(until)
            .limit(10)
            .offset(5)
            .oldest_first();

        assert_eq!(query.device_id, Some("my-device".to_string()));
        assert_eq!(query.since, Some(since));
        assert_eq!(query.until, Some(until));
        assert_eq!(query.limit, Some(10));
        assert_eq!(query.offset, Some(5));
        assert!(!query.newest_first);
    }

    #[test]
    fn test_reading_query_build_where_empty() {
        let query = ReadingQuery::new();
        let (where_clause, params) = query.build_where();
        assert_eq!(where_clause, "");
        assert!(params.is_empty());
    }

    #[test]
    fn test_reading_query_build_where_device_only() {
        let query = ReadingQuery::new().device("test-device");
        let (where_clause, params) = query.build_where();
        assert_eq!(where_clause, "WHERE device_id = ?");
        assert_eq!(params.len(), 1);
    }

    #[test]
    fn test_reading_query_build_where_time_range() {
        let since = datetime!(2024-01-01 00:00:00 UTC);
        let until = datetime!(2024-12-31 23:59:59 UTC);

        let query = ReadingQuery::new().since(since).until(until);
        let (where_clause, params) = query.build_where();

        assert_eq!(where_clause, "WHERE captured_at >= ? AND captured_at <= ?");
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn test_reading_query_build_where_all_filters() {
        let since = datetime!(2024-01-01 00:00:00 UTC);
        let until = datetime!(2024-12-31 23:59:59 UTC);

        let query = ReadingQuery::new()
            .device("device-1")
            .since(since)
            .until(until);
        let (where_clause, params) = query.build_where();

        assert!(where_clause.contains("device_id = ?"));
        assert!(where_clause.contains("captured_at >= ?"));
        assert!(where_clause.contains("captured_at <= ?"));
        assert_eq!(params.len(), 3);
    }

    #[test]
    fn test_reading_query_build_sql_basic() {
        let query = ReadingQuery::new();
        let sql = query.build_sql();

        assert!(sql.contains("SELECT"));
        assert!(sql.contains("FROM readings"));
        assert!(sql.contains("ORDER BY captured_at DESC"));
        assert!(!sql.contains("WHERE"));
        assert!(!sql.contains("LIMIT"));
        assert!(!sql.contains("OFFSET"));
    }

    #[test]
    fn test_reading_query_build_sql_with_limit() {
        let query = ReadingQuery::new().limit(50);
        let sql = query.build_sql();

        assert!(sql.contains("LIMIT 50"));
    }

    #[test]
    fn test_reading_query_build_sql_with_offset() {
        let query = ReadingQuery::new().offset(25);
        let sql = query.build_sql();

        assert!(sql.contains("OFFSET 25"));
    }

    #[test]
    fn test_reading_query_build_sql_oldest_first() {
        let query = ReadingQuery::new().oldest_first();
        let sql = query.build_sql();

        assert!(sql.contains("ORDER BY captured_at ASC"));
    }

    #[test]
    fn test_reading_query_build_sql_complete() {
        let since = datetime!(2024-06-01 00:00:00 UTC);
        let query = ReadingQuery::new()
            .device("my-sensor")
            .since(since)
            .limit(100)
            .offset(10)
            .oldest_first();

        let sql = query.build_sql();

        assert!(sql.contains("WHERE"));
        assert!(sql.contains("device_id = ?"));
        assert!(sql.contains("captured_at >= ?"));
        assert!(sql.contains("ORDER BY captured_at ASC"));
        assert!(sql.contains("LIMIT 100"));
        assert!(sql.contains("OFFSET 10"));
    }

    #[test]
    fn test_reading_query_build_sql_selects_all_columns() {
        let query = ReadingQuery::new();
        let sql = query.build_sql();

        assert!(sql.contains("id"));
        assert!(sql.contains("device_id"));
        assert!(sql.contains("captured_at"));
        assert!(sql.contains("co2"));
        assert!(sql.contains("temperature"));
        assert!(sql.contains("pressure"));
        assert!(sql.contains("humidity"));
        assert!(sql.contains("battery"));
        assert!(sql.contains("status"));
        assert!(sql.contains("radon"));
        assert!(sql.contains("radiation_rate"));
        assert!(sql.contains("radiation_total"));
    }

    // ==================== HistoryQuery Tests ====================

    #[test]
    fn test_history_query_new_defaults() {
        let query = HistoryQuery::new();
        assert!(query.device_id.is_none());
        assert!(query.since.is_none());
        assert!(query.until.is_none());
        assert!(query.limit.is_none());
        assert!(query.offset.is_none());
        assert!(query.newest_first);
    }

    #[test]
    fn test_history_query_default_is_different_from_new() {
        let default_query = HistoryQuery::default();
        let new_query = HistoryQuery::new();

        // Default doesn't set newest_first, but new() does
        assert!(!default_query.newest_first);
        assert!(new_query.newest_first);
    }

    #[test]
    fn test_history_query_device_filter() {
        let query = HistoryQuery::new().device("aranet4-abc123");
        assert_eq!(query.device_id, Some("aranet4-abc123".to_string()));
    }

    #[test]
    fn test_history_query_since_filter() {
        let time = datetime!(2024-03-15 08:00:00 UTC);
        let query = HistoryQuery::new().since(time);
        assert_eq!(query.since, Some(time));
    }

    #[test]
    fn test_history_query_until_filter() {
        let time = datetime!(2024-03-15 20:00:00 UTC);
        let query = HistoryQuery::new().until(time);
        assert_eq!(query.until, Some(time));
    }

    #[test]
    fn test_history_query_limit() {
        let query = HistoryQuery::new().limit(500);
        assert_eq!(query.limit, Some(500));
    }

    #[test]
    fn test_history_query_offset() {
        let query = HistoryQuery::new().offset(200);
        assert_eq!(query.offset, Some(200));
    }

    #[test]
    fn test_history_query_oldest_first() {
        let query = HistoryQuery::new().oldest_first();
        assert!(!query.newest_first);
    }

    #[test]
    fn test_history_query_chaining() {
        let since = datetime!(2024-01-01 00:00:00 UTC);
        let until = datetime!(2024-06-30 23:59:59 UTC);

        let query = HistoryQuery::new()
            .device("sensor-xyz")
            .since(since)
            .until(until)
            .limit(1000)
            .offset(100)
            .oldest_first();

        assert_eq!(query.device_id, Some("sensor-xyz".to_string()));
        assert_eq!(query.since, Some(since));
        assert_eq!(query.until, Some(until));
        assert_eq!(query.limit, Some(1000));
        assert_eq!(query.offset, Some(100));
        assert!(!query.newest_first);
    }

    #[test]
    fn test_history_query_clone() {
        let query = HistoryQuery::new().device("device-1").limit(50);
        let cloned = query.clone();

        assert_eq!(cloned.device_id, query.device_id);
        assert_eq!(cloned.limit, query.limit);
    }

    #[test]
    fn test_reading_query_clone() {
        let query = ReadingQuery::new().device("device-1").limit(50);
        let cloned = query.clone();

        assert_eq!(cloned.device_id, query.device_id);
        assert_eq!(cloned.limit, query.limit);
    }

    #[test]
    fn test_reading_query_debug() {
        let query = ReadingQuery::new().device("test");
        let debug_str = format!("{:?}", query);
        assert!(debug_str.contains("ReadingQuery"));
        assert!(debug_str.contains("test"));
    }

    #[test]
    fn test_history_query_debug() {
        let query = HistoryQuery::new().device("test");
        let debug_str = format!("{:?}", query);
        assert!(debug_str.contains("HistoryQuery"));
        assert!(debug_str.contains("test"));
    }

    #[test]
    fn test_reading_query_limit_zero() {
        let query = ReadingQuery::new().limit(0);
        let sql = query.build_sql();
        assert!(sql.contains("LIMIT 0"));
    }

    #[test]
    fn test_reading_query_large_pagination() {
        let query = ReadingQuery::new().limit(u32::MAX).offset(u32::MAX);
        let sql = query.build_sql();
        assert!(sql.contains(&format!("LIMIT {}", u32::MAX)));
        assert!(sql.contains(&format!("OFFSET {}", u32::MAX)));
    }
}
