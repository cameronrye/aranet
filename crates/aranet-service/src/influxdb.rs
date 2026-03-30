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
//! aranet,device=Office,address=Aranet4_17C3C co2=450i,temperature=22.5,humidity=45i,pressure=1013.2,battery=85i 1711612800
//! ```

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
    let measurement = &config.measurement;

    // Tags
    let device_tag = escape_tag_value(alias.unwrap_or(&event.device_id));
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

    format!("{measurement},device={device_tag},address={address_tag} {field_set} {timestamp}")
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
/// Tag values cannot contain spaces, commas, or equals signs unescaped.
fn escape_tag_value(s: &str) -> String {
    s.replace(' ', "\\ ")
        .replace(',', "\\,")
        .replace('=', "\\=")
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
    fn test_urlencoding() {
        assert_eq!(urlencoding("my org"), "my%20org");
        assert_eq!(urlencoding("a&b"), "a%26b");
        assert_eq!(urlencoding("simple"), "simple");
    }
}
