//! Query builders for readings and history.

use time::OffsetDateTime;

/// Query builder for readings.
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
    /// Create a new reading query.
    pub fn new() -> Self {
        Self {
            newest_first: true,
            ..Default::default()
        }
    }

    /// Filter by device ID.
    pub fn device(mut self, device_id: &str) -> Self {
        self.device_id = Some(device_id.to_string());
        self
    }

    /// Filter readings after this time.
    pub fn since(mut self, time: OffsetDateTime) -> Self {
        self.since = Some(time);
        self
    }

    /// Filter readings before this time.
    pub fn until(mut self, time: OffsetDateTime) -> Self {
        self.until = Some(time);
        self
    }

    /// Limit the number of results.
    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Offset for pagination.
    pub fn offset(mut self, offset: u32) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Order by oldest first.
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

/// Query builder for history records.
#[derive(Debug, Default, Clone)]
pub struct HistoryQuery {
    /// Filter by device ID.
    pub device_id: Option<String>,
    /// Filter records after this time.
    pub since: Option<OffsetDateTime>,
    /// Filter records before this time.
    pub until: Option<OffsetDateTime>,
    /// Maximum number of results.
    pub limit: Option<u32>,
    /// Offset for pagination.
    pub offset: Option<u32>,
    /// Order by timestamp descending (newest first).
    pub newest_first: bool,
}

impl HistoryQuery {
    /// Create a new history query.
    pub fn new() -> Self {
        Self {
            newest_first: true,
            ..Default::default()
        }
    }

    /// Filter by device ID.
    pub fn device(mut self, device_id: &str) -> Self {
        self.device_id = Some(device_id.to_string());
        self
    }

    /// Filter records after this time.
    pub fn since(mut self, time: OffsetDateTime) -> Self {
        self.since = Some(time);
        self
    }

    /// Filter records before this time.
    pub fn until(mut self, time: OffsetDateTime) -> Self {
        self.until = Some(time);
        self
    }

    /// Limit the number of results.
    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Offset for pagination.
    pub fn offset(mut self, offset: u32) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Order by oldest first.
    pub fn oldest_first(mut self) -> Self {
        self.newest_first = false;
        self
    }
}
