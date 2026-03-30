//! Webhook notification system for threshold alerts.
//!
//! This module sends HTTP POST requests to configured webhook URLs when sensor
//! readings exceed defined thresholds. Useful for integrating with Slack, Discord,
//! PagerDuty, or any HTTP-based notification service.
//!
//! # Example Configuration
//!
//! ```toml
//! [webhooks]
//! enabled = true
//! cooldown_secs = 300  # Minimum 5 minutes between alerts per device
//!
//! [[webhooks.endpoints]]
//! url = "https://hooks.slack.com/services/T00/B00/xxx"
//! events = ["co2_high", "radon_high", "battery_low"]
//!
//! [[webhooks.endpoints]]
//! url = "https://ntfy.sh/my-aranet-alerts"
//! events = ["co2_high"]
//! ```
//!
//! # Payload Format
//!
//! Webhooks send a JSON POST with the following structure:
//!
//! ```json
//! {
//!   "event": "co2_high",
//!   "device_id": "Aranet4 17C3C",
//!   "alias": "Office",
//!   "value": 1450,
//!   "threshold": 1000,
//!   "unit": "ppm",
//!   "reading": { ... },
//!   "timestamp": "2026-03-28T12:00:00Z"
//! }
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::future::join_all;
use reqwest::Client;
use serde::Serialize;
use time::OffsetDateTime;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::config::WebhookConfig;
use crate::state::{AppState, ReadingEvent};

/// Webhook dispatcher that monitors readings and fires alerts.
pub struct WebhookDispatcher {
    state: Arc<AppState>,
}

impl WebhookDispatcher {
    /// Create a new webhook dispatcher.
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Start the webhook dispatcher.
    ///
    /// Spawns a background task that listens to the readings broadcast channel
    /// and sends webhook notifications when thresholds are exceeded.
    pub async fn start(&self) {
        let config = self.state.config.read().await;
        let webhook_config = config.webhooks.clone();
        drop(config);

        if !webhook_config.enabled {
            info!("Webhook notifications are disabled");
            return;
        }

        if webhook_config.endpoints.is_empty() {
            info!("No webhook endpoints configured");
            return;
        }

        info!(
            "Starting webhook dispatcher with {} endpoint(s)",
            webhook_config.endpoints.len()
        );

        let state = Arc::clone(&self.state);
        let shutdown_rx = self.state.subscribe_shutdown();

        tokio::spawn(async move {
            run_webhook_dispatcher(state, webhook_config, shutdown_rx).await;
        });
    }
}

/// A webhook alert payload.
#[derive(Debug, Clone, Serialize)]
pub struct WebhookPayload {
    /// The event type (e.g., "co2_high", "battery_low").
    pub event: String,
    /// Device ID/address.
    pub device_id: String,
    /// Device alias (if configured).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// The value that triggered the alert.
    pub value: f64,
    /// The threshold that was exceeded.
    pub threshold: f64,
    /// Unit of measurement.
    pub unit: String,
    /// The full reading that triggered the alert.
    pub reading: aranet_store::StoredReading,
    /// When the alert was generated.
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

/// Run the webhook dispatcher loop.
async fn run_webhook_dispatcher(
    state: Arc<AppState>,
    config: WebhookConfig,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    let client = match Client::builder().timeout(Duration::from_secs(30)).build() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to create HTTP client for webhooks: {e}");
            return;
        }
    };

    let mut readings_rx = state.readings_tx.subscribe();
    let cooldown = Duration::from_secs(config.cooldown_secs);

    // Track last alert time per (device_id, event) to enforce cooldown
    let mut last_alert: HashMap<(String, String), OffsetDateTime> = HashMap::new();

    loop {
        tokio::select! {
            result = readings_rx.recv() => {
                match result {
                    Ok(event) => {
                        let alias = configured_alias(&state, &event.device_id).await;
                        let alerts = evaluate_thresholds(&config, &event, alias);
                        let now = OffsetDateTime::now_utc();
                        let cooldown_duration = time::Duration::try_from(cooldown)
                            .unwrap_or(time::Duration::seconds(300));

                        for payload in alerts {
                            let key = (payload.device_id.clone(), payload.event.clone());

                            // Check cooldown
                            if let Some(last) = last_alert.get(&key) {
                                let elapsed = now - *last;
                                if elapsed < cooldown_duration {
                                    debug!(
                                        "Skipping {} alert for {} (cooldown: {}s remaining)",
                                        payload.event,
                                        payload.device_id,
                                        (cooldown_duration - elapsed).whole_seconds()
                                    );
                                    continue;
                                }
                            }

                            let matching_endpoints: Vec<_> = config
                                .endpoints
                                .iter()
                                .filter(|endpoint| endpoint.events.iter().any(|event| event == &payload.event))
                                .cloned()
                                .collect();

                            if matching_endpoints.is_empty() {
                                debug!(
                                    "No webhook endpoints configured for {} alerts",
                                    payload.event
                                );
                                continue;
                            }

                            let results = join_all(matching_endpoints.into_iter().map(|endpoint| {
                                let client = client.clone();
                                let payload = payload.clone();
                                async move {
                                    send_webhook_with_retry(
                                        &client,
                                        &endpoint.url,
                                        &endpoint.headers,
                                        &payload,
                                    )
                                    .await
                                }
                            }))
                            .await;

                            if results.into_iter().any(|delivered| delivered) {
                                last_alert.insert(key, now);
                            } else {
                                warn!(
                                    "All webhook deliveries failed for {} alert on {}",
                                    payload.event, payload.device_id
                                );
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("Webhook dispatcher lagged, missed {} readings", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("Readings channel closed, stopping webhook dispatcher");
                        break;
                    }
                }
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("Webhook dispatcher received stop signal");
                    break;
                }
            }
        }
    }

    info!("Webhook dispatcher stopped");
}

async fn configured_alias(state: &AppState, device_id: &str) -> Option<String> {
    let config = state.config.read().await;
    config
        .devices
        .iter()
        .find(|device| device.address == device_id)
        .and_then(|device| device.alias.clone())
}

/// Evaluate thresholds for a reading and return any triggered alerts.
fn evaluate_thresholds(
    config: &WebhookConfig,
    event: &ReadingEvent,
    alias: Option<String>,
) -> Vec<WebhookPayload> {
    let mut alerts = Vec::new();
    let reading = &event.reading;
    let now = OffsetDateTime::now_utc();

    // CO2 threshold
    if reading.co2 > 0 && reading.co2 >= config.co2_threshold {
        alerts.push(WebhookPayload {
            event: "co2_high".to_string(),
            device_id: event.device_id.clone(),
            alias: alias.clone(),
            value: reading.co2 as f64,
            threshold: config.co2_threshold as f64,
            unit: "ppm".to_string(),
            reading: reading.clone(),
            timestamp: now,
        });
    }

    // Radon threshold
    if let Some(radon) = reading.radon
        && radon >= config.radon_threshold
    {
        alerts.push(WebhookPayload {
            event: "radon_high".to_string(),
            device_id: event.device_id.clone(),
            alias: alias.clone(),
            value: f64::from(radon),
            threshold: config.radon_threshold as f64,
            unit: "Bq/m\u{b3}".to_string(),
            reading: reading.clone(),
            timestamp: now,
        });
    }

    // Battery low threshold
    if reading.battery > 0 && reading.battery <= config.battery_threshold {
        alerts.push(WebhookPayload {
            event: "battery_low".to_string(),
            device_id: event.device_id.clone(),
            alias,
            value: reading.battery as f64,
            threshold: config.battery_threshold as f64,
            unit: "%".to_string(),
            reading: reading.clone(),
            timestamp: now,
        });
    }

    alerts
}

/// Maximum number of delivery attempts per webhook (initial + retries).
const MAX_WEBHOOK_ATTEMPTS: u32 = 3;

/// Send a webhook with exponential backoff retry.
///
/// Attempts delivery up to [`MAX_WEBHOOK_ATTEMPTS`] times with delays of
/// 2s, 4s between retries. Logs a warning on each failed attempt and an
/// error if all attempts are exhausted.
async fn send_webhook_with_retry(
    client: &Client,
    url: &str,
    headers: &HashMap<String, String>,
    payload: &WebhookPayload,
) -> bool {
    let mut delay = Duration::from_secs(2);

    for attempt in 1..=MAX_WEBHOOK_ATTEMPTS {
        match send_webhook(client, url, headers, payload).await {
            Ok(()) => {
                info!(
                    "Sent {} webhook for {} to {}",
                    payload.event, payload.device_id, url
                );
                return true;
            }
            Err(e) if attempt < MAX_WEBHOOK_ATTEMPTS => {
                warn!(
                    "Webhook to {} failed (attempt {}/{}): {}. Retrying in {}s",
                    url,
                    attempt,
                    MAX_WEBHOOK_ATTEMPTS,
                    e,
                    delay.as_secs()
                );
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
            Err(e) => {
                warn!(
                    "Webhook to {} failed after {} attempts: {}",
                    url, MAX_WEBHOOK_ATTEMPTS, e
                );
            }
        }
    }

    false
}

/// Send a webhook POST request.
async fn send_webhook(
    client: &Client,
    url: &str,
    headers: &HashMap<String, String>,
    payload: &WebhookPayload,
) -> Result<(), WebhookError> {
    let mut request = client
        .post(url)
        .header("Content-Type", "application/json")
        .json(payload);

    for (key, value) in headers {
        request = request.header(key.as_str(), value.as_str());
    }

    let response = request.send().await.map_err(WebhookError::Request)?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(WebhookError::Response {
            status: status.as_u16(),
            body,
        });
    }

    Ok(())
}

/// Errors that can occur when sending webhooks.
#[derive(Debug, thiserror::Error)]
pub enum WebhookError {
    #[error("Request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Webhook returned error {status}: {body}")]
    Response { status: u16, body: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{WebhookConfig, WebhookEndpoint};
    use aranet_types::Status;

    fn test_reading(co2: u16, battery: u8) -> ReadingEvent {
        ReadingEvent {
            device_id: "Aranet4 12345".to_string(),
            reading: aranet_store::StoredReading {
                id: 1,
                device_id: "Aranet4 12345".to_string(),
                co2,
                temperature: 22.5,
                humidity: 45,
                pressure: 1013.0,
                battery,
                status: Status::Green,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
                radon_avg_24h: None,
                radon_avg_7d: None,
                radon_avg_30d: None,
                captured_at: OffsetDateTime::now_utc(),
            },
        }
    }

    #[test]
    fn test_evaluate_thresholds_co2_high() {
        let config = WebhookConfig {
            enabled: true,
            co2_threshold: 1000,
            radon_threshold: 300,
            battery_threshold: 10,
            cooldown_secs: 300,
            endpoints: vec![],
        };

        // Below threshold - no alert
        let event = test_reading(800, 85);
        let alerts = evaluate_thresholds(&config, &event, None);
        assert!(alerts.is_empty());

        // At threshold - alert
        let event = test_reading(1000, 85);
        let alerts = evaluate_thresholds(&config, &event, None);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].event, "co2_high");
        assert_eq!(alerts[0].value, 1000.0);

        // Above threshold - alert
        let event = test_reading(1500, 85);
        let alerts = evaluate_thresholds(&config, &event, None);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].event, "co2_high");
    }

    #[test]
    fn test_evaluate_thresholds_battery_low() {
        let config = WebhookConfig {
            enabled: true,
            co2_threshold: 1000,
            radon_threshold: 300,
            battery_threshold: 20,
            cooldown_secs: 300,
            endpoints: vec![],
        };

        // Battery ok - no alert
        let event = test_reading(500, 85);
        let alerts = evaluate_thresholds(&config, &event, None);
        assert!(alerts.is_empty());

        // Battery low - alert
        let event = test_reading(500, 15);
        let alerts = evaluate_thresholds(&config, &event, None);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].event, "battery_low");
    }

    #[test]
    fn test_evaluate_thresholds_multiple() {
        let config = WebhookConfig {
            enabled: true,
            co2_threshold: 1000,
            radon_threshold: 300,
            battery_threshold: 20,
            cooldown_secs: 300,
            endpoints: vec![],
        };

        // CO2 high AND battery low
        let event = test_reading(1500, 10);
        let alerts = evaluate_thresholds(&config, &event, None);
        assert_eq!(alerts.len(), 2);
    }

    #[test]
    fn test_evaluate_thresholds_with_alias() {
        let config = WebhookConfig {
            enabled: true,
            co2_threshold: 1000,
            radon_threshold: 300,
            battery_threshold: 10,
            cooldown_secs: 300,
            endpoints: vec![],
        };
        let event = test_reading(1500, 85);
        let alerts = evaluate_thresholds(&config, &event, Some("Office".to_string()));
        assert_eq!(alerts[0].alias, Some("Office".to_string()));
    }

    #[test]
    fn test_webhook_payload_serialization() {
        let payload = WebhookPayload {
            event: "co2_high".to_string(),
            device_id: "Aranet4 12345".to_string(),
            alias: Some("Office".to_string()),
            value: 1500.0,
            threshold: 1000.0,
            unit: "ppm".to_string(),
            reading: test_reading(1500, 85).reading,
            timestamp: OffsetDateTime::now_utc(),
        };

        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("co2_high"));
        assert!(json.contains("Office"));
        assert!(json.contains("1500"));
    }

    #[test]
    fn test_webhook_endpoint_event_matching() {
        let endpoint = WebhookEndpoint {
            url: "https://example.com/hook".to_string(),
            events: vec!["co2_high".to_string(), "battery_low".to_string()],
            headers: HashMap::new(),
        };

        assert!(endpoint.events.iter().any(|e| e == "co2_high"));
        assert!(endpoint.events.iter().any(|e| e == "battery_low"));
        assert!(!endpoint.events.iter().any(|e| e == "radon_high"));
    }
}
