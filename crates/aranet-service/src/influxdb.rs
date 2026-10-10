//! InfluxDB line protocol exporter for Aranet sensor readings.
//!
//! This module writes sensor readings to InfluxDB using the v2 HTTP API
//! and line protocol format.
//!
//! # Example Configuration
//!
//! ```toml
//! [influxdb]
//! enabled = true
//! url = "http://localhost:8086"
//! token = "my-influxdb-token"
//! org = "my-org"
//! bucket = "aranet"
//! measurement = "aranet"
//! precision = "s"
//! ```
//!
//! # Line Protocol Format
//!
//! Readings are written as:
//!
//! ```text
//! aranet,device=Office,address=Aranet4\ 17C3C co2=450i,temperature=22.50,humidity=45i,pressure=1013.20,battery=85i 1711612800
//! ```
//!
//! The measurement name and the tag values are escaped as the line protocol
//! reference specifies: backslashes, commas and spaces (and `=` in tag values)
//! get a backslash, and control characters, which line protocol can't hold,
//! become an escaped space. An alias can therefore never add tags or start a
//! second point. The `device` tag is left out when it would be empty.
//!
//! Escaping can't make a backslash storable, though. Measured on InfluxDB
//! 1.8.10, 2.7.12 and 3 Core 3.12.0: 1.8.10 and 2.7.12 store a doubled
//! backslash as two and read a separator after any backslash as escaped,
//! 3 Core stores a doubled backslash as one, and none of them stores a tag
//! value or a measurement that ends in a backslash correctly. No encoding
//! works on all three, so config validation refuses backslashes in device
//! addresses, aliases and the measurement name.
//!
//! Validation also refuses a measurement name that starts with `#`. The name
//! starts the line, InfluxDB reads a line that starts with `#` as a comment
//! and stores nothing, and no escape changes that.
//!
//! # Delivery
//!
//! Readings are queued, up to 10000 lines, and written in batches of up to
//! 500 lines. A write that fails because InfluxDB can't be reached, or that
//! InfluxDB answers with 5xx, 408 or 429, is retried after 1 s, then 2 s, 4 s
//! and so on up to 5 minutes, so the queue carries readings through an
//! outage. A reading that arrives while the queue is full is dropped, with a
//! warning at most once a minute. Lines that InfluxDB rejects with any other
//! 4xx are dropped. Lines still queued when the service stops are not written.

use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::DropLog;
use crate::config::InfluxDbConfig;
use crate::redact::{redact_url, sanitize_body};
use crate::state::{AppState, ReadingEvent};

/// InfluxDB writer that exports readings to InfluxDB.
pub struct InfluxDbWriter {
    state: Arc<AppState>,
}

impl InfluxDbWriter {
    /// Create a new InfluxDB writer.
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Start the InfluxDB writer.
    ///
    /// Spawns a background task that listens to the readings broadcast channel
    /// and writes the readings to InfluxDB in batches, retrying while InfluxDB
    /// is unreachable. Returns the task's handle, or `None` when InfluxDB
    /// export is disabled. The task stops when the service shuts down.
    pub async fn start(&self) -> Option<JoinHandle<()>> {
        self.start_with(InfluxSettings::default()).await
    }

    /// [`Self::start`] with non-default queue, batch and retry settings.
    pub(crate) async fn start_with(&self, settings: InfluxSettings) -> Option<JoinHandle<()>> {
        let config = self.state.config.read().await;
        let influxdb_config = config.influxdb.clone();
        drop(config);

        if !influxdb_config.enabled {
            info!("InfluxDB export is disabled");
            return None;
        }

        info!(
            "Starting InfluxDB writer to {}",
            redact_url(&influxdb_config.url)
        );

        let state = Arc::clone(&self.state);
        let shutdown_rx = self.state.subscribe_shutdown();

        Some(tokio::spawn(run_influxdb_writer(
            state,
            influxdb_config,
            shutdown_rx,
            settings,
        )))
    }
}

/// Queue, batch and retry settings of the InfluxDB writer.
#[derive(Debug, Clone, Copy)]
pub(crate) struct InfluxSettings {
    /// Most lines waiting to be written. A reading that arrives while the
    /// queue is full is dropped.
    pub(crate) queue_lines: usize,
    /// Most lines sent in one write request.
    pub(crate) batch_lines: usize,
    /// Wait before retrying a failed write. It doubles with every further
    /// failure in a row.
    pub(crate) base_backoff: Duration,
    /// Longest wait between retries.
    pub(crate) max_backoff: Duration,
    /// Timeout of one write request, connecting included.
    pub(crate) request_timeout: Duration,
}

impl Default for InfluxSettings {
    fn default() -> Self {
        Self {
            queue_lines: 10_000,
            batch_lines: 500,
            base_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(300),
            request_timeout: Duration::from_secs(30),
        }
    }
}

/// Run the InfluxDB writer: this receive loop turns readings into lines and
/// queues them, and a writer task sends the queue to InfluxDB in batches.
///
/// The receive loop never waits on InfluxDB. A line that doesn't fit in the
/// queue (`settings.queue_lines`) is dropped, with a warning at most once a
/// minute. The writer sends up to `settings.batch_lines` lines per request.
/// A batch that failed with a request error, a 5xx, 408 or 429 is sent again
/// after `settings.base_backoff`, doubling up to `settings.max_backoff`;
/// InfluxDB rejects the lines of any other failure for good, so that batch is
/// dropped. On shutdown the writer stops at once and warns how many queued
/// lines were not written; this function returns after it.
async fn run_influxdb_writer(
    state: Arc<AppState>,
    config: InfluxDbConfig,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    settings: InfluxSettings,
) {
    let client = match Client::builder().timeout(settings.request_timeout).build() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to create HTTP client for InfluxDB: {e}");
            return;
        }
    };

    let (line_tx, line_rx) = mpsc::channel(settings.queue_lines);
    let writer = tokio::spawn(write_queued_lines(
        client,
        config.clone(),
        line_rx,
        state.subscribe_shutdown(),
        settings,
    ));

    let mut readings_rx = state.readings_tx.subscribe();
    let mut drops = DropLog::default();

    loop {
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                info!("InfluxDB writer received stop signal");
                break;
            }
            result = readings_rx.recv() => {
                match result {
                    Ok(event) => {
                        let alias = configured_alias(&state, &event.device_id).await;
                        let line = to_line_protocol(&config, &event, alias.as_deref());
                        if queue_line(&line_tx, line, &mut drops) {
                            debug!("Queued reading for {} for InfluxDB", event.device_id);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("InfluxDB writer lagged, missed {} readings", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("Readings channel closed, stopping InfluxDB writer");
                        break;
                    }
                }
            }
        }
    }

    // Closing the queue ends the writer once it has written what is left,
    // unless shutdown stops it first.
    drop(line_tx);
    if let Err(e) = writer.await {
        warn!("InfluxDB write task failed: {e}");
    }

    info!("InfluxDB writer stopped");
}

/// Queue `line` for the writer task without waiting. Returns `false` when the
/// line was dropped: the queue is full, or the writer task has stopped. Either
/// way the drop is counted, with a warning at most once a minute.
fn queue_line(line_tx: &mpsc::Sender<String>, line: String, drops: &mut DropLog) -> bool {
    match line_tx.try_send(line) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(_)) => {
            if drops.record() {
                warn!(
                    "InfluxDB write queue is full ({} lines); dropped {} readings so far",
                    line_tx.max_capacity(),
                    drops.dropped
                );
            }
            false
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            // The writer task failed, or shutdown stopped it just before this
            // loop saw the signal. A failed task's error is logged once this
            // loop ends.
            if drops.record() {
                warn!(
                    "InfluxDB write task has stopped; dropped {} readings so far",
                    drops.dropped
                );
            }
            false
        }
    }
}

/// Send the queued lines to InfluxDB in batches, until the queue is closed and
/// empty or the service shuts down.
async fn write_queued_lines(
    client: Client,
    config: InfluxDbConfig,
    mut line_rx: mpsc::Receiver<String>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    settings: InfluxSettings,
) {
    let mut batch: Vec<String> = Vec::with_capacity(settings.batch_lines);
    let mut backoff = settings.base_backoff;
    let mut failures: u32 = 0;

    loop {
        // A batch that failed is sent again as it is; new lines wait for the
        // next one.
        if batch.is_empty() && line_rx.recv_many(&mut batch, settings.batch_lines).await == 0 {
            // The receive loop has stopped and the queue is empty.
            break;
        }

        let result = tokio::select! {
            biased;
            _ = shutdown_rx.changed() => break,
            result = write_lines(&client, &config, &batch) => result,
        };

        match result {
            Ok(()) => {
                if failures > 0 {
                    info!("InfluxDB write succeeded after {failures} failed attempts");
                }
                debug!("Wrote {} lines to InfluxDB", batch.len());
                batch.clear();
                backoff = settings.base_backoff;
                failures = 0;
            }
            Err(e) if is_transient(&e) => {
                failures += 1;
                warn!(
                    "Failed to write {} lines to InfluxDB: {e}; retrying in {}s",
                    batch.len(),
                    backoff.as_secs_f32()
                );
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => break,
                    () = tokio::time::sleep(backoff) => {}
                }
                backoff = backoff.saturating_mul(2).min(settings.max_backoff);
            }
            Err(e) => {
                warn!(
                    "InfluxDB rejected {} lines, dropping them: {e}",
                    batch.len()
                );
                batch.clear();
                backoff = settings.base_backoff;
                failures = 0;
            }
        }
    }

    let unwritten = batch.len() + line_rx.len();
    if unwritten > 0 {
        warn!("InfluxDB writer stopped with {unwritten} queued lines not written");
    }
}

async fn configured_alias(state: &AppState, device_id: &str) -> Option<String> {
    let config = state.config.read().await;
    config
        .devices
        .iter()
        .find(|device| device.address == device_id)
        .and_then(|device| device.alias.clone())
}

/// Convert a reading event to InfluxDB line protocol.
fn to_line_protocol(config: &InfluxDbConfig, event: &ReadingEvent, alias: Option<&str>) -> String {
    let reading = &event.reading;
    let measurement = escape_measurement(&config.measurement);

    // Tags. InfluxDB rejects a line with an empty tag value, so an empty alias
    // leaves the device tag out.
    let device_tag = escape_tag_value(alias.unwrap_or(&event.device_id));
    let device_tag = if device_tag.is_empty() {
        String::new()
    } else {
        format!(",device={device_tag}")
    };
    let address_tag = escape_tag_value(&event.device_id);

    // Build field set - only include non-zero/available fields
    let mut fields = Vec::new();

    if reading.co2 > 0 {
        fields.push(format!("co2={}i", reading.co2));
    }
    fields.push(format!("temperature={:.2}", reading.temperature));
    if reading.humidity > 0 {
        fields.push(format!("humidity={}i", reading.humidity));
    }
    if reading.pressure > 0.0 {
        fields.push(format!("pressure={:.2}", reading.pressure));
    }
    fields.push(format!("battery={}i", reading.battery));

    if let Some(radon) = reading.radon {
        fields.push(format!("radon={}i", radon));
    }
    if let Some(rate) = reading.radiation_rate {
        fields.push(format!("radiation_rate={:.4}", rate));
    }
    if let Some(total) = reading.radiation_total {
        fields.push(format!("radiation_total={:.6}", total));
    }
    if let Some(avg) = reading.radon_avg_24h {
        fields.push(format!("radon_avg_24h={}i", avg));
    }
    if let Some(avg) = reading.radon_avg_7d {
        fields.push(format!("radon_avg_7d={}i", avg));
    }
    if let Some(avg) = reading.radon_avg_30d {
        fields.push(format!("radon_avg_30d={}i", avg));
    }

    let field_set = fields.join(",");

    let timestamp = timestamp_for_precision(reading.captured_at, &config.precision);

    format!("{measurement}{device_tag},address={address_tag} {field_set} {timestamp}")
}

fn timestamp_for_precision(timestamp: time::OffsetDateTime, precision: &str) -> i128 {
    let nanos = timestamp.unix_timestamp_nanos();
    match precision {
        "ns" => nanos,
        "us" => nanos / 1_000,
        "ms" => nanos / 1_000_000,
        _ => nanos / 1_000_000_000,
    }
}

/// Write `lines` to InfluxDB in one request, one point per line.
async fn write_lines(
    client: &Client,
    config: &InfluxDbConfig,
    lines: &[String],
) -> Result<(), InfluxDbError> {
    let url = format!(
        "{}/api/v2/write?org={}&bucket={}&precision={}",
        config.url.trim_end_matches('/'),
        urlencoding(&config.org),
        urlencoding(&config.bucket),
        config.precision
    );

    let mut request = client
        .post(&url)
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(lines.join("\n"));

    if let Some(token) = &config.token {
        request = request.header("Authorization", format!("Token {}", token));
    }

    let response = request.send().await.map_err(InfluxDbError::from)?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(InfluxDbError::Response {
            status: status.as_u16(),
            body: sanitize_body(&body),
        });
    }

    Ok(())
}

/// Whether a failed write may work when sent again: the request failed
/// (InfluxDB unreachable or too slow) or InfluxDB answered 5xx, 408 or 429.
/// Any other status means InfluxDB rejected the lines themselves.
fn is_transient(e: &InfluxDbError) -> bool {
    match e {
        InfluxDbError::Request(_) => true,
        InfluxDbError::Response { status, .. } => {
            *status >= 500 || *status == 408 || *status == 429
        }
    }
}

/// Percent-encode a string for use in URL query parameters.
fn urlencoding(s: &str) -> String {
    let mut encoded = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            // Unreserved characters (RFC 3986 §2.3)
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    encoded
}

/// Escape a tag value for InfluxDB line protocol.
///
/// Backslashes are doubled, as the line protocol reference specifies. Commas,
/// equals signs and spaces get a backslash, so a value can't end its tag early.
/// Line protocol can't hold a newline or any other control character in a tag
/// value, so each becomes an escaped space and a value can never start a second
/// point.
///
/// Doubling doesn't make a backslash storable. Measured on InfluxDB 1.8.10,
/// 2.7.12 and 3 Core 3.12.0: 1.8.10 and 2.7.12 store a doubled backslash as
/// two and read a separator after any backslash as escaped, 3 Core stores it as
/// one in the middle of a value, and all three answer 400 when a tag value ends
/// in it. No encoding stores every backslash the same on all three. That is why
/// `DeviceConfig::validate` rejects backslashes, as well as control characters,
/// in addresses and aliases; the escaping here is the second line of defence.
/// For a value that ends in a backslash, it turns a point that 3 Core would
/// store with two tags merged into a write that all three refuse.
fn escape_tag_value(s: &str) -> String {
    escape_line_protocol(s, &['\\', ',', '=', ' '])
}

/// Escape a measurement name for InfluxDB line protocol: backslashes, commas
/// and spaces get a backslash, and control characters become an escaped space.
fn escape_measurement(s: &str) -> String {
    escape_line_protocol(s, &['\\', ',', ' '])
}

/// Put a backslash before each character in `special` (which includes the
/// backslash itself) and turn every control character into an escaped space.
fn escape_line_protocol(s: &str, special: &[char]) -> String {
    let mut escaped = String::with_capacity(s.len());
    for c in s.chars() {
        if special.contains(&c) {
            escaped.push('\\');
            escaped.push(c);
        } else if c.is_control() {
            escaped.push_str("\\ ");
        } else {
            escaped.push(c);
        }
    }
    escaped
}

/// Errors that can occur when writing to InfluxDB.
///
/// Their text is safe to log: a request error converted with `From` carries no
/// URL, and a stored response body is at most 256 characters, with line
/// breaks, control characters and bidi controls replaced by spaces.
#[derive(Debug, thiserror::Error)]
pub enum InfluxDbError {
    #[error("Request failed: {0}")]
    Request(#[source] reqwest::Error),
    #[error("InfluxDB returned error {status}: {body}")]
    Response { status: u16, body: String },
}

impl From<reqwest::Error> for InfluxDbError {
    /// Drops the URL that reqwest attaches to its errors: their `Display` and
    /// `Debug` output print it, and it can carry a secret path or query.
    fn from(e: reqwest::Error) -> Self {
        Self::Request(e.without_url())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{CapturedLogs, TestState, closed_port_url};
    use aranet_types::Status;
    use time::OffsetDateTime;

    fn test_event(device_id: &str, co2: u16) -> ReadingEvent {
        ReadingEvent {
            device_id: device_id.to_string(),
            reading: aranet_store::StoredReading {
                id: 1,
                device_id: device_id.to_string(),
                co2,
                temperature: 22.5,
                humidity: 45,
                pressure: 1013.25,
                battery: 85,
                status: Status::Green,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
                radon_avg_24h: None,
                radon_avg_7d: None,
                radon_avg_30d: None,
                captured_at: OffsetDateTime::from_unix_timestamp(1711612800).unwrap(),
            },
        }
    }

    #[test]
    fn test_to_line_protocol_basic() {
        let config = InfluxDbConfig::default();
        let event = test_event("Aranet4 12345", 800);

        let line = to_line_protocol(&config, &event, None);

        assert!(line.starts_with("aranet,"));
        assert!(line.contains("device=Aranet4\\ 12345"));
        assert!(line.contains("co2=800i"));
        assert!(line.contains("temperature=22.50"));
        assert!(line.contains("humidity=45i"));
        assert!(line.contains("pressure=1013.25"));
        assert!(line.contains("battery=85i"));
        assert!(line.contains("1711612800"));
    }

    #[test]
    fn test_to_line_protocol_with_alias() {
        let config = InfluxDbConfig::default();
        let event = test_event("Aranet4 12345", 800);

        let line = to_line_protocol(&config, &event, Some("Office"));

        assert!(line.contains("device=Office"));
        assert!(line.contains("address=Aranet4\\ 12345"));
    }

    #[test]
    fn test_to_line_protocol_with_radon() {
        let config = InfluxDbConfig::default();
        let mut event = test_event("AranetRn 12345", 0);
        event.reading.co2 = 0;
        event.reading.radon = Some(150);

        let line = to_line_protocol(&config, &event, None);

        assert!(!line.contains("co2="));
        assert!(line.contains("radon=150i"));
    }

    #[test]
    fn test_timestamp_precision_respected() {
        let config = InfluxDbConfig {
            precision: "ms".to_string(),
            ..Default::default()
        };
        let event = test_event("Aranet4 12345", 800);

        let line = to_line_protocol(&config, &event, None);
        assert!(line.ends_with("1711612800000"));
    }

    #[test]
    fn test_escape_tag_value() {
        assert_eq!(escape_tag_value("Office"), "Office");
        assert_eq!(escape_tag_value("Living Room"), "Living\\ Room");
        assert_eq!(escape_tag_value("a,b=c"), "a\\,b\\=c");
    }

    #[test]
    fn escape_tag_table() {
        for (input, expected) in [
            ("a=b", "a\\=b"),
            ("Living Room", "Living\\ Room"),
            ("a,b", "a\\,b"),
            // A backslash is doubled before anything else is escaped, as the
            // line protocol reference specifies.
            ("a\\b", "a\\\\b"),
            ("Office\\", "Office\\\\"),
            ("a\\ b", "a\\\\\\ b"),
            // Line protocol can't hold control characters: each becomes an
            // escaped space.
            ("a\nb", "a\\ b"),
            ("a\r\n\tb\u{0}", "a\\ \\ \\ b\\ "),
            // Everything else passes through.
            ("AranetRn+ 306B8", "AranetRn+\\ 306B8"),
            ("Küche \u{2622}", "Küche\\ \u{2622}"),
        ] {
            assert_eq!(escape_tag_value(input), expected, "input {input:?}");
        }
    }

    #[test]
    fn alias_newline_cannot_inject_points() {
        let config = InfluxDbConfig::default();
        let event = test_event("Aranet4 12345", 800);

        let line = to_line_protocol(&config, &event, Some("Office\nevil,device=x co2=1i 0"));

        assert!(!line.contains('\n'), "{line:?}");
        assert_eq!(line.lines().count(), 1, "{line:?}");
        assert!(
            line.starts_with(
                "aranet,device=Office\\ evil\\,device\\=x\\ co2\\=1i\\ 0,address=Aranet4\\ 12345 co2=800i,"
            ),
            "{line}"
        );
    }

    #[test]
    fn trailing_backslash_does_not_escape_separator() {
        let config = InfluxDbConfig::default();
        let event = test_event("Aranet4 12345", 800);

        // A trailing backslash must never swallow the comma after it into the
        // value. Sent undoubled, InfluxDB 3 Core 3.12.0 stores the alias and
        // the address as one tag value, `Office,address=Aranet4 12345`. Doubled,
        // as here, the line is refused with HTTP 400 by InfluxDB 1.8.10, 2.7.12
        // and 3 Core, which reads `\\` as one backslash only in the middle of a
        // value. The doubling turns a silently wrong point into a refused
        // write. It doesn't make the value storable, which is why
        // `DeviceConfig::validate` keeps backslashes out of aliases.
        let line = to_line_protocol(&config, &event, Some("Office\\"));

        assert!(
            line.starts_with("aranet,device=Office\\\\,address=Aranet4\\ 12345 co2=800i,"),
            "{line}"
        );
    }

    #[test]
    fn measurement_is_escaped() {
        let config = InfluxDbConfig {
            measurement: "air quality,x".to_string(),
            ..Default::default()
        };
        let event = test_event("Aranet4 12345", 800);

        let line = to_line_protocol(&config, &event, Some("Office"));

        assert!(
            line.starts_with("air\\ quality\\,x,device=Office,address=Aranet4\\ 12345 "),
            "{line}"
        );
    }

    #[test]
    fn measurement_keeps_equals_sign_unescaped() {
        // Only a tag value escapes `=`. A measurement sent as `a\=b` is stored
        // on InfluxDB 2.7.12 as a point that no query returns and on 3 Core
        // 3.12.0 under the name `a\=b`; sent as `a=b` it is stored as `a=b` on
        // 1.8.10, 2.7.12 and 3 Core.
        let event = test_event("Aranet4 12345", 800);

        for (text, as_measurement, as_tag_value) in [
            ("a=b", "a=b", "a\\=b"),
            ("a b=c,d", "a\\ b=c\\,d", "a\\ b\\=c\\,d"),
        ] {
            assert_eq!(escape_measurement(text), as_measurement, "{text:?}");
            assert_eq!(escape_tag_value(text), as_tag_value, "{text:?}");

            // The same text as the measurement and as the alias of one line.
            let config = InfluxDbConfig {
                measurement: text.to_string(),
                ..Default::default()
            };
            let line = to_line_protocol(&config, &event, Some(text));
            assert!(
                line.starts_with(&format!(
                    "{as_measurement},device={as_tag_value},address=Aranet4\\ 12345 "
                )),
                "{line}"
            );
        }
    }

    #[test]
    fn empty_alias_omits_device_tag() {
        let config = InfluxDbConfig::default();
        let event = test_event("Aranet4 12345", 800);

        // InfluxDB rejects a line with an empty tag value.
        let line = to_line_protocol(&config, &event, Some(""));

        assert!(
            line.starts_with("aranet,address=Aranet4\\ 12345 co2=800i,"),
            "{line}"
        );
    }

    #[test]
    fn test_urlencoding() {
        assert_eq!(urlencoding("my org"), "my%20org");
        assert_eq!(urlencoding("a&b"), "a%26b");
        assert_eq!(urlencoding("simple"), "simple");
    }

    #[tokio::test]
    async fn influxdb_write_error_has_no_url() {
        // InfluxDB behind a reverse proxy that routes on a secret path prefix.
        let config = InfluxDbConfig {
            enabled: true,
            url: closed_port_url("/influxdb-SECRET").await,
            ..Default::default()
        };
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();

        let error = write_lines(
            &client,
            &config,
            &["aranet co2=800i 1711612800".to_string()],
        )
        .await
        .expect_err("nothing listens on the port");

        for text in [error.to_string(), format!("{error:?}")] {
            assert!(
                !text.contains("SECRET") && !text.contains("/api/v2/write"),
                "the InfluxDB URL leaked into the error: {text}"
            );
        }
    }

    #[tokio::test]
    async fn influxdb_start_log_hides_userinfo_and_path() {
        let url = closed_port_url("/influxdb-SECRET-PATH").await.replacen(
            "http://",
            "http://admin:SECRET-PW@",
            1,
        );
        let t = TestState::with_config(crate::config::Config {
            influxdb: InfluxDbConfig {
                enabled: true,
                url,
                ..Default::default()
            },
            ..Default::default()
        });

        let (logs, _g) = CapturedLogs::start();
        InfluxDbWriter::new(Arc::clone(&t.state)).start().await;
        t.state.signal_shutdown();

        let logs = logs.text();
        assert!(
            !logs.contains("SECRET"),
            "the InfluxDB URL leaked into the logs:\n{logs}"
        );
        assert!(
            logs.contains("Starting InfluxDB writer to http://127.0.0.1:"),
            "start not logged:\n{logs}"
        );
    }

    #[test]
    fn influxdb_queue_drops_when_full_or_closed() {
        let (line_tx, mut line_rx) = tokio::sync::mpsc::channel(2);
        let mut drops = DropLog::default();

        let queued: Vec<bool> = ["a", "b", "c"]
            .into_iter()
            .map(|line| queue_line(&line_tx, line.to_string(), &mut drops))
            .collect();

        assert_eq!(queued, [true, true, false]);
        assert_eq!(drops.dropped, 1);
        assert_eq!(line_rx.try_recv().unwrap(), "a");
        assert_eq!(line_rx.try_recv().unwrap(), "b");

        // The writer task has stopped, so the queue is closed.
        drop(line_rx);
        assert!(!queue_line(&line_tx, "d".to_string(), &mut drops));
        assert_eq!(drops.dropped, 2);
    }

    #[test]
    fn influxdb_is_transient_table() {
        for (status, transient) in [
            (500, true),
            (503, true),
            (408, true),
            (429, true),
            (400, false),
            (401, false),
            (404, false),
            (413, false),
            (422, false),
        ] {
            let e = InfluxDbError::Response {
                status,
                body: String::new(),
            };
            assert_eq!(is_transient(&e), transient, "{status}");
        }
    }

    /// The writer against a loopback InfluxDB, on real time. A module of its
    /// own, so that its imports can't clash with the ones above.
    mod writer {
        use super::*;
        use crate::config::Config;
        use crate::test_support::{CapturedLogs, RecordingServer, Reply, TestState, wait_until};

        const DEVICE: &str = "Aranet4 12345";
        const WRITE: &str = "/api/v2/write";

        /// InfluxDB export to `server`.
        fn influx_config(server: &RecordingServer) -> Config {
            Config {
                influxdb: InfluxDbConfig {
                    enabled: true,
                    url: server.url(""),
                    org: "aranet".to_string(),
                    ..InfluxDbConfig::default()
                },
                ..Config::default()
            }
        }

        /// The line the writer sends for `event` (no alias is configured).
        fn line_of(event: &ReadingEvent) -> String {
            to_line_protocol(&InfluxDbConfig::default(), event, None)
        }

        /// Start the writer and wait until it listens for readings.
        async fn start_writer(t: &TestState, settings: InfluxSettings) -> JoinHandle<()> {
            let handle = InfluxDbWriter::new(Arc::clone(&t.state))
                .start_with(settings)
                .await
                .expect("InfluxDB export is enabled");
            wait_until("the writer to subscribe to readings", || {
                t.state.readings_tx.receiver_count() >= 1
            })
            .await;
            handle
        }

        /// Signal shutdown and wait for the writer to return.
        async fn shut_down(t: &TestState, writer: JoinHandle<()>) {
            t.state.signal_shutdown();
            tokio::time::timeout(Duration::from_secs(5), writer)
                .await
                .expect("the writer stops within 5 s of shutdown")
                .expect("the writer does not panic");
        }

        #[tokio::test]
        async fn influxdb_retries_failed_batch() {
            let server =
                RecordingServer::start(vec![(WRITE, Reply::Sequence(vec![503, 503, 204]))]).await;
            let t = TestState::with_config(influx_config(&server));
            let settings = InfluxSettings {
                base_backoff: Duration::from_millis(10),
                ..InfluxSettings::default()
            };
            let writer = start_writer(&t, settings).await;

            let (first, second) = (test_event(DEVICE, 801), test_event(DEVICE, 802));
            t.state.readings_tx.send(first.clone()).unwrap();
            t.state.readings_tx.send(second.clone()).unwrap();

            // Two 503s, then InfluxDB accepts the third attempt of the batch.
            let requests = server.wait_for_requests(3).await;
            let batch = format!("{}\n{}", line_of(&first), line_of(&second));
            let bodies: Vec<&str> = requests.iter().map(|r| r.body.as_str()).collect();
            assert_eq!(bodies, [batch.as_str(); 3]);

            shut_down(&t, writer).await;
        }

        #[tokio::test]
        async fn influxdb_permanent_4xx_drops_batch() {
            let server = RecordingServer::start(vec![(WRITE, Reply::Status(400))]).await;
            let t = TestState::with_config(influx_config(&server));
            let settings = InfluxSettings {
                base_backoff: Duration::from_millis(10),
                ..InfluxSettings::default()
            };
            let writer = start_writer(&t, settings).await;

            let (first, second) = (test_event(DEVICE, 801), test_event(DEVICE, 802));
            t.state.readings_tx.send(first.clone()).unwrap();
            server.wait_for_requests(1).await;
            t.state.readings_tx.send(second.clone()).unwrap();

            // The rejected line is not sent again: the next request carries
            // the next reading.
            let requests = server.wait_for_requests(2).await;
            assert_eq!(requests[0].body, line_of(&first));
            assert_eq!(requests[1].body, line_of(&second));

            shut_down(&t, writer).await;
            assert_eq!(server.requests().len(), 2, "{:?}", server.requests());
        }

        #[tokio::test]
        async fn influxdb_writer_stops_promptly_and_counts_unwritten_lines() {
            let (logs, _guard) = CapturedLogs::start();
            let server = RecordingServer::start(vec![(WRITE, Reply::Status(503))]).await;
            let t = TestState::with_config(influx_config(&server));
            let settings = InfluxSettings {
                base_backoff: Duration::from_secs(60),
                ..InfluxSettings::default()
            };
            let writer = start_writer(&t, settings).await;

            t.state.readings_tx.send(test_event(DEVICE, 801)).unwrap();
            server.wait_for_requests(1).await;

            // The writer is waiting for the 503 or has started its 60 s
            // backoff. Either way shutdown ends it at once, and the line
            // counts as not written.
            shut_down(&t, writer).await;
            let text = logs.text();
            assert!(
                text.contains("InfluxDB writer stopped with 1 queued lines not written"),
                "{text}"
            );
        }
    }
}
