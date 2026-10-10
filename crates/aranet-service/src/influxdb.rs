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
//! InfluxDB 1.x and 2.x don't read `\\` back as one backslash in a tag value
//! or a measurement, and they treat a separator after any backslash as
//! escaped. Config validation therefore refuses backslashes in device
//! addresses, aliases and the measurement name, so every line reads the same
//! on InfluxDB 1.x, 2.x and 3.
//!
//! Validation also refuses a measurement name that starts with `#`. The name
//! starts the line, InfluxDB reads a line that starts with `#` as a comment
//! and stores nothing, and no escape changes that.

use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::config::InfluxDbConfig;
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
    /// and writes each reading to InfluxDB.
    pub async fn start(&self) {
        let config = self.state.config.read().await;
        let influxdb_config = config.influxdb.clone();
        drop(config);

        if !influxdb_config.enabled {
            info!("InfluxDB export is disabled");
            return;
        }

        info!("Starting InfluxDB writer to {}", influxdb_config.url);

        let state = Arc::clone(&self.state);
        let shutdown_rx = self.state.subscribe_shutdown();

        tokio::spawn(async move {
            run_influxdb_writer(state, influxdb_config, shutdown_rx).await;
        });
    }
}

/// Run the InfluxDB writer loop.
async fn run_influxdb_writer(
    state: Arc<AppState>,
    config: InfluxDbConfig,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    let client = match Client::builder().timeout(Duration::from_secs(30)).build() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to create HTTP client for InfluxDB: {e}");
            return;
        }
    };

    let mut readings_rx = state.readings_tx.subscribe();

    loop {
        tokio::select! {
            result = readings_rx.recv() => {
                match result {
                    Ok(event) => {
                        let alias = configured_alias(&state, &event.device_id).await;
                        let line = to_line_protocol(&config, &event, alias.as_deref());
                        if let Err(e) = write_line(&client, &config, &line).await {
                            warn!("Failed to write to InfluxDB: {}", e);
                        } else {
                            debug!("Wrote reading for {} to InfluxDB", event.device_id);
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
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("InfluxDB writer received stop signal");
                    break;
                }
            }
        }
    }

    info!("InfluxDB writer stopped");
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

/// Write a line protocol entry to InfluxDB.
async fn write_line(
    client: &Client,
    config: &InfluxDbConfig,
    line: &str,
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
        .body(line.to_string());

    if let Some(token) = &config.token {
        request = request.header("Authorization", format!("Token {}", token));
    }

    let response = request.send().await.map_err(InfluxDbError::Request)?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(InfluxDbError::Response {
            status: status.as_u16(),
            body,
        });
    }

    Ok(())
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
/// Backslashes are doubled first, as the line protocol reference specifies, so
/// on InfluxDB 3 a value can't escape the separator after it. Commas, equals
/// signs and spaces then get a backslash, so a value can't end its tag early.
/// Line protocol can't hold a newline or any other control character in a tag
/// value, so each becomes an escaped space and a value can never start a second
/// point.
///
/// InfluxDB 1.x and 2.x don't unescape `\\`: they would store a backslash
/// doubled, and they reject a line whose tag value ends in one. That is why
/// `DeviceConfig::validate` rejects backslashes, as well as control characters,
/// in addresses and aliases; the escaping here is the second line of defence.
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
#[derive(Debug, thiserror::Error)]
pub enum InfluxDbError {
    #[error("Request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("InfluxDB returned error {status}: {body}")]
    Response { status: u16, body: String },
}

#[cfg(test)]
mod tests {
    use super::*;
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

        // The line protocol reference (and InfluxDB 3) reads `\\` as one
        // literal backslash, so the comma after it still separates the tags.
        // InfluxDB 1.x and 2.x reject this line instead, which is why
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
}
