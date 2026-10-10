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
//! # Delivery
//!
//! Each alert is delivered in the background, to all of its endpoints at
//! once, so a slow or unreachable endpoint never delays other alerts or the
//! readings after it. An endpoint gets up to three attempts, each with a 10 s
//! timeout, 2 s and then 4 s apart. At most 16 alerts are delivered at the
//! same time, and only one per device and event. An alert over the limit of
//! 16 is dropped, with a warning at most once a minute. An alert for a device
//! and event that is still being delivered is skipped, with a line in the
//! debug log only. The cooldown starts once an alert has reached at least one
//! endpoint. When the service stops, deliveries still running get 5 s to
//! finish.
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
use tokio::task::{JoinHandle, JoinSet};
use tracing::{debug, info, warn};

use crate::DropLog;
use crate::config::{WebhookConfig, WebhookEndpoint};
use crate::redact::{redact_url, sanitize_body};
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
    /// and sends webhook notifications when thresholds are exceeded. Returns the
    /// task's handle, or `None` when webhooks are disabled or no endpoint is
    /// configured. The task stops when the service shuts down, after giving
    /// deliveries still in progress 5 s to finish.
    pub async fn start(&self) -> Option<JoinHandle<()>> {
        self.start_with(WebhookSettings::default()).await
    }

    /// [`Self::start`] with non-default timeouts and limits.
    pub(crate) async fn start_with(&self, settings: WebhookSettings) -> Option<JoinHandle<()>> {
        let config = self.state.config.read().await;
        let webhook_config = config.webhooks.clone();
        drop(config);

        if !webhook_config.enabled {
            info!("Webhook notifications are disabled");
            return None;
        }

        if webhook_config.endpoints.is_empty() {
            info!("No webhook endpoints configured");
            return None;
        }

        info!(
            "Starting webhook dispatcher with {} endpoint(s)",
            webhook_config.endpoints.len()
        );

        let state = Arc::clone(&self.state);
        let shutdown_rx = self.state.subscribe_shutdown();

        Some(tokio::spawn(run_webhook_dispatcher(
            state,
            webhook_config,
            shutdown_rx,
            settings,
        )))
    }
}

/// Most alerts delivered at the same time. Alerts beyond this are dropped.
pub(crate) const MAX_IN_FLIGHT_DELIVERIES: usize = 16;

/// Timeouts and limits of the webhook dispatcher.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WebhookSettings {
    /// Attempts per endpoint, and the wait between them.
    pub(crate) retry: RetryPolicy,
    /// Timeout of one request to an endpoint, connecting included.
    pub(crate) request_timeout: Duration,
    /// Most alerts delivered at the same time.
    pub(crate) max_in_flight: usize,
    /// How long shutdown waits for deliveries still in progress before
    /// cancelling them.
    pub(crate) shutdown_grace: Duration,
}

impl Default for WebhookSettings {
    fn default() -> Self {
        Self {
            retry: RetryPolicy::DEFAULT,
            request_timeout: Duration::from_secs(10),
            max_in_flight: MAX_IN_FLIGHT_DELIVERIES,
            shutdown_grace: Duration::from_secs(5),
        }
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

/// A device and alert event, such as `("Aranet4 17C3C", "co2_high")`: the
/// unit of the cooldown and of the in-flight check.
type AlertKey = (String, String);

/// Run the webhook dispatcher loop.
///
/// The loop only evaluates readings and hands alerts off: each alert is
/// delivered by its own task in `deliveries`, so a slow or unreachable
/// endpoint never delays other alerts, later readings or shutdown. At most
/// `settings.max_in_flight` alerts are delivered at once, and only one per
/// [`AlertKey`]; an alert beyond the limit is dropped with a warning (at most
/// one a minute) and doesn't start the cooldown. The cooldown starts when a
/// delivery reaches at least one endpoint. On shutdown, deliveries still
/// running get `settings.shutdown_grace` to finish and are then cancelled.
async fn run_webhook_dispatcher(
    state: Arc<AppState>,
    config: WebhookConfig,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    settings: WebhookSettings,
) {
    let client = match Client::builder().timeout(settings.request_timeout).build() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to create HTTP client for webhooks: {e}");
            return;
        }
    };

    let mut readings_rx = state.readings_tx.subscribe();
    let cooldown = Duration::from_secs(config.cooldown_secs);

    // When each (device_id, event) last reached an endpoint, for the cooldown.
    let mut last_alert: HashMap<AlertKey, OffsetDateTime> = HashMap::new();
    // Alerts being delivered, by the task delivering them.
    let mut deliveries: JoinSet<bool> = JoinSet::new();
    let mut in_flight: HashMap<tokio::task::Id, AlertKey> = HashMap::new();
    let mut drops = DropLog::default();

    loop {
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                info!("Webhook dispatcher received stop signal");
                break;
            }
            Some(joined) = deliveries.join_next_with_id(), if !deliveries.is_empty() => {
                let (id, delivered) = match joined {
                    Ok((id, delivered)) => (id, delivered),
                    Err(e) => {
                        warn!("Webhook delivery task failed: {e}");
                        (e.id(), false)
                    }
                };
                if let Some((device_id, event)) = in_flight.remove(&id) {
                    if delivered {
                        last_alert.insert((device_id, event), OffsetDateTime::now_utc());
                    } else {
                        warn!(
                            "All webhook deliveries failed for {} alert on {}",
                            event, device_id
                        );
                    }
                }
            }
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

                            if in_flight.values().any(|pending| *pending == key) {
                                debug!(
                                    "Skipping {} alert for {} (the previous one is still being delivered)",
                                    payload.event, payload.device_id
                                );
                                continue;
                            }

                            let endpoints: Vec<(String, WebhookEndpoint)> = config
                                .endpoints
                                .iter()
                                .enumerate()
                                .filter(|(_, endpoint)| endpoint.events.iter().any(|event| event == &payload.event))
                                .map(|(index, endpoint)| (endpoint_label(index, &endpoint.url), endpoint.clone()))
                                .collect();

                            if endpoints.is_empty() {
                                debug!(
                                    "No webhook endpoints configured for {} alerts",
                                    payload.event
                                );
                                continue;
                            }

                            if deliveries.len() >= settings.max_in_flight {
                                if drops.record() {
                                    warn!(
                                        "{} webhook deliveries already in flight; dropping new alerts ({} dropped so far)",
                                        deliveries.len(),
                                        drops.dropped
                                    );
                                }
                                continue;
                            }

                            let task = deliveries.spawn(deliver(
                                client.clone(),
                                endpoints,
                                payload,
                                settings.retry,
                            ));
                            in_flight.insert(task.id(), key);
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
        }
    }

    if !deliveries.is_empty() {
        let finished = tokio::time::timeout(settings.shutdown_grace, async {
            while deliveries.join_next().await.is_some() {}
        })
        .await;
        if finished.is_err() {
            warn!(
                "Cancelling {} webhook deliveries still running {}s after shutdown",
                deliveries.len(),
                settings.shutdown_grace.as_secs_f32()
            );
        }
    }
    // Dropping the set aborts any delivery still running.
    drop(deliveries);

    info!("Webhook dispatcher stopped");
}

/// Send one alert to its endpoints, each given with its log label, at the same
/// time. Returns `true` when at least one endpoint accepted it.
async fn deliver(
    client: Client,
    endpoints: Vec<(String, WebhookEndpoint)>,
    payload: WebhookPayload,
    retry: RetryPolicy,
) -> bool {
    join_all(endpoints.iter().map(|(label, endpoint)| {
        send_webhook_with_retry(
            &client,
            label,
            &endpoint.url,
            &endpoint.headers,
            &payload,
            retry,
        )
    }))
    .await
    .into_iter()
    .any(|delivered| delivered)
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

/// How many times a webhook delivery is attempted, and how long to wait
/// between attempts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RetryPolicy {
    /// Attempts in total, including the first.
    pub(crate) attempts: u32,
    /// The wait after the first failed attempt, doubled after each further one.
    pub(crate) base_delay: Duration,
}

impl RetryPolicy {
    /// Three attempts, 2 s and then 4 s apart.
    pub(crate) const DEFAULT: RetryPolicy = RetryPolicy {
        attempts: 3,
        base_delay: Duration::from_secs(2),
    };
}

/// How the logs name a webhook endpoint: its 1-based position in
/// `webhooks.endpoints`, and its URL reduced to `scheme://host[:port]/…`.
/// Slack and Discord webhook URLs carry their secret in the path, and an ntfy
/// topic URL is the secret.
fn endpoint_label(index: usize, url: &str) -> String {
    format!("endpoint #{} ({})", index + 1, redact_url(url))
}

/// Send a webhook, retrying with exponential backoff.
///
/// Makes up to `policy.attempts` attempts, waiting `policy.base_delay` after
/// the first failure and twice as long after each further one, and logs a
/// warning for each failed attempt. The log lines name the endpoint by `label`
/// (see [`endpoint_label`]). Returns whether an attempt succeeded.
async fn send_webhook_with_retry(
    client: &Client,
    label: &str,
    url: &str,
    headers: &HashMap<String, String>,
    payload: &WebhookPayload,
    policy: RetryPolicy,
) -> bool {
    let mut delay = policy.base_delay;

    for attempt in 1..=policy.attempts {
        match send_webhook(client, url, headers, payload).await {
            Ok(()) => {
                info!(
                    "Sent {} webhook for {} to {}",
                    payload.event, payload.device_id, label
                );
                return true;
            }
            Err(e) if attempt < policy.attempts => {
                warn!(
                    "Webhook to {} failed (attempt {}/{}): {}. Retrying in {}s",
                    label,
                    attempt,
                    policy.attempts,
                    e,
                    delay.as_secs()
                );
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
            Err(e) => {
                warn!(
                    "Webhook to {} failed after {} attempts: {}",
                    label, policy.attempts, e
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

    let response = request.send().await.map_err(WebhookError::from)?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(WebhookError::Response {
            status: status.as_u16(),
            body: sanitize_body(&body),
        });
    }

    Ok(())
}

/// Errors that can occur when sending webhooks.
///
/// Their text is safe to log: a request error converted with `From` carries no
/// URL, and a stored response body keeps only its first 256 characters,
/// followed by `…` when it was longer (so 257 at most), with line breaks,
/// control characters and bidi controls replaced by spaces.
#[derive(Debug, thiserror::Error)]
pub enum WebhookError {
    #[error("Request failed: {0}")]
    Request(#[source] reqwest::Error),
    #[error("Webhook returned error {status}: {body}")]
    Response { status: u16, body: String },
}

impl From<reqwest::Error> for WebhookError {
    /// Drops the URL that reqwest attaches to its errors: their `Display` and
    /// `Debug` output print it, and webhook URLs often contain their secret.
    fn from(e: reqwest::Error) -> Self {
        Self::Request(e.without_url())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{WebhookConfig, WebhookEndpoint};
    use crate::test_support::{CapturedLogs, RecordingServer, Reply, closed_port_url};
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

    fn co2_payload() -> WebhookPayload {
        let event = test_reading(1500, 85);
        WebhookPayload {
            event: "co2_high".to_string(),
            device_id: event.device_id,
            alias: None,
            value: 1500.0,
            threshold: 1000.0,
            unit: "ppm".to_string(),
            reading: event.reading,
            timestamp: OffsetDateTime::now_utc(),
        }
    }

    fn test_client() -> Client {
        Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn webhook_logs_never_contain_endpoint_secret() {
        let server = RecordingServer::start(vec![
            ("/hook/SECRET-OK", Reply::Status(204)),
            ("/hook/SECRET-PATH", Reply::Status(500)),
        ])
        .await;
        let urls = [
            server.url("/hook/SECRET-OK"),
            server.url("/hook/SECRET-PATH"),
            closed_port_url("/hook/SECRET-2?token=SECRET-3").await,
        ];
        let policy = RetryPolicy {
            attempts: 2,
            base_delay: Duration::ZERO,
        };
        let (client, headers, payload) = (test_client(), HashMap::new(), co2_payload());

        let (logs, _g) = CapturedLogs::start();
        let mut delivered = Vec::new();
        for (index, url) in urls.iter().enumerate() {
            let label = endpoint_label(index, url);
            delivered.push(
                send_webhook_with_retry(&client, &label, url, &headers, &payload, policy).await,
            );
        }

        let logs = logs.text();
        assert!(
            !logs.contains("SECRET"),
            "an endpoint secret reached the logs:\n{logs}"
        );
        assert_eq!(delivered, [true, false, false]);
        let paths: Vec<String> = server.requests().into_iter().map(|r| r.path).collect();
        assert_eq!(
            paths,
            ["/hook/SECRET-OK", "/hook/SECRET-PATH", "/hook/SECRET-PATH"],
            "requests still go to the full URL, and the 500 is retried once"
        );
        let base = server.url("");
        for line in [
            format!("Sent co2_high webhook for Aranet4 12345 to endpoint #1 ({base}/…)"),
            format!(
                "Webhook to endpoint #2 ({base}/…) failed (attempt 1/2): Webhook returned error 500"
            ),
            format!("Webhook to endpoint #2 ({base}/…) failed after 2 attempts"),
            "Webhook to endpoint #3 (http://127.0.0.1:".to_string(),
        ] {
            assert!(logs.contains(&line), "missing {line:?} in:\n{logs}");
        }
    }

    #[tokio::test]
    async fn webhook_error_display_has_no_url() {
        let url = closed_port_url("/hook/SECRET-PATH?token=SECRET-QUERY").await;
        let request_error = test_client()
            .post(&url)
            .send()
            .await
            .expect_err("nothing listens on the port");
        assert!(
            request_error.url().is_some(),
            "reqwest attaches the URL to its errors"
        );

        let error = WebhookError::from(request_error);
        for text in [error.to_string(), format!("{error:?}")] {
            assert!(
                !text.contains("/hook") && !text.contains("SECRET"),
                "the endpoint URL leaked into the error: {text}"
            );
        }
    }

    #[tokio::test]
    async fn webhook_error_body_is_sanitised_before_it_is_logged() {
        // A 500 whose body would start a forged log line (the newline),
        // reverse the text after it (U+202E) and flood the log (over 256
        // characters).
        let body = format!("bad\nrequest\u{202e}{}", "x".repeat(300));
        let server =
            RecordingServer::start(vec![("/hook", Reply::StatusWithBody(500, body))]).await;
        let url = server.url("/hook");
        let policy = RetryPolicy {
            attempts: 1,
            base_delay: Duration::ZERO,
        };

        let (logs, _g) = CapturedLogs::start();
        let delivered = send_webhook_with_retry(
            &test_client(),
            &endpoint_label(0, &url),
            &url,
            &HashMap::new(),
            &co2_payload(),
            policy,
        )
        .await;

        assert!(!delivered);
        // Both characters became spaces, and the log line ends after the
        // body's first 256 characters and a `…`.
        let logs = logs.text();
        let logged = format!(
            "Webhook returned error 500: bad request {}…\n",
            "x".repeat(244)
        );
        assert!(logs.contains(&logged), "{logs:?}");
        assert!(!logs.contains('\u{202e}'), "{logs:?}");
    }

    /// The dispatcher against a loopback endpoint, on real time. A module of
    /// its own, so that its imports can't clash with the ones above.
    mod dispatcher {
        use super::*;
        use crate::config::Config;
        use crate::test_support::{CapturedLogs, RecordingServer, Reply, TestState, wait_until};

        const X: &str = "Aranet4 XXXXX";
        const Y: &str = "Aranet4 YYYYY";

        fn reading_for(device_id: &str, co2: u16, battery: u8) -> ReadingEvent {
            let mut event = test_reading(co2, battery);
            event.device_id = device_id.to_string();
            event.reading.device_id = device_id.to_string();
            event
        }

        fn endpoint(url: String, event: &str) -> WebhookEndpoint {
            WebhookEndpoint {
                url,
                events: vec![event.to_string()],
                headers: HashMap::new(),
            }
        }

        /// Webhooks on, with the default thresholds (CO2 1000 ppm, battery
        /// 10 %) and cooldown (300 s).
        fn webhook_config(endpoints: Vec<WebhookEndpoint>) -> Config {
            Config {
                webhooks: WebhookConfig {
                    enabled: true,
                    endpoints,
                    ..WebhookConfig::default()
                },
                ..Config::default()
            }
        }

        /// The default settings, but only 500 ms for deliveries still running
        /// at shutdown, so a test that ends with a hanging delivery ends fast.
        fn test_settings() -> WebhookSettings {
            WebhookSettings {
                shutdown_grace: Duration::from_millis(500),
                ..WebhookSettings::default()
            }
        }

        /// Start the dispatcher and wait until it listens for readings.
        async fn start_dispatcher(t: &TestState, settings: WebhookSettings) -> JoinHandle<()> {
            let handle = WebhookDispatcher::new(Arc::clone(&t.state))
                .start_with(settings)
                .await
                .expect("webhooks are enabled");
            wait_until("the dispatcher to subscribe to readings", || {
                t.state.readings_tx.receiver_count() >= 1
            })
            .await;
            handle
        }

        /// Wait until the dispatcher has taken every reading sent so far.
        async fn wait_until_readings_taken(t: &TestState) {
            wait_until("the dispatcher to take every reading", || {
                t.state.readings_tx.is_empty()
            })
            .await;
        }

        /// Signal shutdown and wait for the dispatcher to return.
        async fn shut_down(t: &TestState, dispatcher: JoinHandle<()>) {
            t.state.signal_shutdown();
            tokio::time::timeout(Duration::from_secs(5), dispatcher)
                .await
                .expect("the dispatcher stops within 5 s of shutdown")
                .expect("the dispatcher does not panic");
        }

        fn device_of(request: &crate::test_support::RecordedRequest) -> String {
            let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
            body["device_id"].as_str().unwrap().to_string()
        }

        /// Wait until `device`'s CO2 alert has reached the first endpoint and
        /// its delivery has ended.
        ///
        /// A delivery logs this line and then returns without waiting on
        /// anything, and it runs on the test's thread, so it has ended by the
        /// time the test can see the line. The dispatcher takes a finished
        /// delivery before the next reading, so a reading sent after this is
        /// judged with that delivery accounted for.
        async fn wait_until_delivered(logs: &CapturedLogs, device: &str) {
            let sent = format!("Sent co2_high webhook for {device} to endpoint #1 ");
            wait_until("the delivery to finish", || logs.text().contains(&sent)).await;
        }

        #[tokio::test]
        async fn webhook_slow_endpoint_does_not_delay_other_alerts() {
            let server =
                RecordingServer::start(vec![("/slow", Reply::Hang), ("/fast", Reply::Status(200))])
                    .await;
            let t = TestState::with_config(webhook_config(vec![
                endpoint(server.url("/slow"), "co2_high"),
                endpoint(server.url("/fast"), "battery_low"),
            ]));
            let dispatcher = start_dispatcher(&t, test_settings()).await;

            t.state.readings_tx.send(reading_for(X, 1500, 80)).unwrap();
            t.state.readings_tx.send(reading_for(Y, 500, 5)).unwrap();

            // X's CO2 alert hangs at /slow; Y's battery alert must not wait
            // behind it.
            let requests = server.wait_for_requests(2).await;
            let fast: Vec<_> = requests.iter().filter(|r| r.path == "/fast").collect();
            assert_eq!(fast.len(), 1, "{requests:?}");
            assert_eq!(device_of(fast[0]), Y);

            shut_down(&t, dispatcher).await;
        }

        #[tokio::test]
        async fn webhook_same_alert_not_duplicated_while_in_flight() {
            let server = RecordingServer::start(vec![("/slow", Reply::Hang)]).await;
            let t = TestState::with_config(webhook_config(vec![endpoint(
                server.url("/slow"),
                "co2_high",
            )]));
            let dispatcher = start_dispatcher(&t, test_settings()).await;

            t.state.readings_tx.send(reading_for(X, 1500, 80)).unwrap();
            server.wait_for_requests(1).await;
            // The first alert is still being delivered, so its cooldown hasn't
            // started, and a second one must not be sent.
            t.state.readings_tx.send(reading_for(X, 1600, 80)).unwrap();
            wait_until_readings_taken(&t).await;

            // Shutdown gives deliveries 500 ms, long enough for a second
            // request to reach the server if one had been started.
            shut_down(&t, dispatcher).await;
            assert_eq!(server.requests().len(), 1, "{:?}", server.requests());
        }

        #[tokio::test]
        async fn webhook_in_flight_is_bounded() {
            let (logs, _guard) = CapturedLogs::start();
            let server = RecordingServer::start(vec![("/slow", Reply::Hang)]).await;
            let t = TestState::with_config(webhook_config(vec![endpoint(
                server.url("/slow"),
                "co2_high",
            )]));
            let dispatcher = start_dispatcher(&t, test_settings()).await;

            for i in 0..40 {
                let device = format!("Aranet4 {i:05}");
                t.state
                    .readings_tx
                    .send(reading_for(&device, 1500, 80))
                    .unwrap();
            }
            server.wait_for_requests(MAX_IN_FLIGHT_DELIVERIES).await;
            wait_until_readings_taken(&t).await;

            shut_down(&t, dispatcher).await;
            assert_eq!(server.requests().len(), MAX_IN_FLIGHT_DELIVERIES);
            assert_eq!(server.max_in_flight(), MAX_IN_FLIGHT_DELIVERIES);
            // 24 alerts were dropped, and the warning is rate-limited.
            let text = logs.text();
            assert_eq!(
                text.matches("webhook deliveries already in flight").count(),
                1,
                "{text}"
            );
        }

        #[tokio::test]
        async fn webhook_dispatcher_stops_promptly_on_shutdown() {
            let server = RecordingServer::start(vec![("/slow", Reply::Hang)]).await;
            let t = TestState::with_config(webhook_config(vec![endpoint(
                server.url("/slow"),
                "co2_high",
            )]));
            let dispatcher = start_dispatcher(&t, test_settings()).await;

            t.state.readings_tx.send(reading_for(X, 1500, 80)).unwrap();
            server.wait_for_requests(1).await;

            // The delivery hangs until its 10 s request timeout; shutdown
            // cancels it once the 500 ms grace is over.
            shut_down(&t, dispatcher).await;
        }

        #[tokio::test]
        async fn webhook_log_numbers_an_endpoint_by_its_place_in_the_config() {
            let (logs, _guard) = CapturedLogs::start();
            let server = RecordingServer::start(vec![("/co2", Reply::Status(200))]).await;
            // The first endpoint doesn't want CO2 alerts; the second does.
            let t = TestState::with_config(webhook_config(vec![
                endpoint(server.url("/battery"), "battery_low"),
                endpoint(server.url("/co2"), "co2_high"),
            ]));
            let dispatcher = start_dispatcher(&t, WebhookSettings::default()).await;

            t.state.readings_tx.send(reading_for(X, 1500, 80)).unwrap();
            let requests = server.wait_for_requests(1).await;
            assert_eq!(requests[0].path, "/co2");

            // Shutdown lets the delivery that reached the endpoint finish, so
            // its log line is written by the time the dispatcher returns.
            shut_down(&t, dispatcher).await;
            let text = logs.text();
            let line = format!(
                "Sent co2_high webhook for {X} to endpoint #2 ({}/…)",
                server.url("")
            );
            assert!(text.contains(&line), "{text}");
        }

        #[tokio::test]
        async fn webhook_cooldown_starts_when_an_alert_is_delivered() {
            let (logs, _guard) = CapturedLogs::start();
            let server = RecordingServer::start(vec![("/hook", Reply::Status(200))]).await;
            let t = TestState::with_config(webhook_config(vec![endpoint(
                server.url("/hook"),
                "co2_high",
            )]));
            let dispatcher = start_dispatcher(&t, WebhookSettings::default()).await;

            t.state.readings_tx.send(reading_for(X, 1500, 80)).unwrap();
            server.wait_for_requests(1).await;
            wait_until_delivered(&logs, X).await;

            // Still over the threshold, a moment into the 300 s cooldown.
            t.state.readings_tx.send(reading_for(X, 1600, 80)).unwrap();
            wait_until_readings_taken(&t).await;

            // Shutdown lets a delivery that was started finish, so a second
            // request would have reached the server by the time the dispatcher
            // returns.
            shut_down(&t, dispatcher).await;
            let text = logs.text();
            assert!(
                text.contains(&format!("Skipping co2_high alert for {X} (cooldown: ")),
                "the cooldown did not hold the second alert back:\n{text}"
            );
            assert_eq!(server.requests().len(), 1, "{:?}", server.requests());
        }

        #[tokio::test]
        async fn webhook_finished_delivery_frees_its_device_and_event() {
            let (logs, _guard) = CapturedLogs::start();
            let server = RecordingServer::start(vec![("/hook", Reply::Status(200))]).await;
            // No cooldown (a config file needs at least 10 s), so the second
            // alert is held back only if the first still counts as being
            // delivered.
            let mut config = webhook_config(vec![endpoint(server.url("/hook"), "co2_high")]);
            config.webhooks.cooldown_secs = 0;
            let t = TestState::with_config(config);
            let dispatcher = start_dispatcher(&t, WebhookSettings::default()).await;

            t.state.readings_tx.send(reading_for(X, 1500, 80)).unwrap();
            server.wait_for_requests(1).await;
            wait_until_delivered(&logs, X).await;

            t.state.readings_tx.send(reading_for(X, 1600, 80)).unwrap();
            let requests = server.wait_for_requests(2).await;
            let values: Vec<f64> = requests
                .iter()
                .map(|request| {
                    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
                    body["value"].as_f64().unwrap()
                })
                .collect();
            assert_eq!(values, [1500.0, 1600.0], "{requests:?}");

            shut_down(&t, dispatcher).await;
        }

        #[tokio::test]
        async fn webhook_failed_delivery_starts_no_cooldown() {
            let (logs, _guard) = CapturedLogs::start();
            let server = RecordingServer::start(vec![("/hook", Reply::Status(500))]).await;
            let t = TestState::with_config(webhook_config(vec![endpoint(
                server.url("/hook"),
                "co2_high",
            )]));
            // One attempt per alert, so the delivery has failed as soon as the
            // endpoint has answered 500, and a second request can only be a
            // second alert.
            let settings = WebhookSettings {
                retry: RetryPolicy {
                    attempts: 1,
                    base_delay: Duration::ZERO,
                },
                ..WebhookSettings::default()
            };
            let dispatcher = start_dispatcher(&t, settings).await;

            t.state.readings_tx.send(reading_for(X, 1500, 80)).unwrap();
            server.wait_for_requests(1).await;
            // The dispatcher says so once it has dealt with the failed
            // delivery; nothing else about that can be awaited.
            let failed = format!("All webhook deliveries failed for co2_high alert on {X}");
            wait_until("the dispatcher to report the failed delivery", || {
                logs.text().contains(&failed)
            })
            .await;

            // Still over the threshold. The first alert reached no endpoint,
            // so the 300 s cooldown must not hold this one back.
            t.state.readings_tx.send(reading_for(X, 1600, 80)).unwrap();
            let requests = server.wait_for_requests(2).await;
            let second: serde_json::Value = serde_json::from_str(&requests[1].body).unwrap();
            assert_eq!(second["value"], 1600.0, "{requests:?}");

            shut_down(&t, dispatcher).await;
            let text = logs.text();
            assert!(
                !text.contains(&format!("Skipping co2_high alert for {X} (cooldown: ")),
                "a delivery that failed started the cooldown:\n{text}"
            );
        }

        #[tokio::test]
        async fn webhook_cooldown_starts_when_one_of_two_endpoints_is_reached() {
            let (logs, _guard) = CapturedLogs::start();
            let server = RecordingServer::start(vec![
                ("/broken", Reply::Status(500)),
                ("/ok", Reply::Status(200)),
            ])
            .await;
            // Both endpoints want CO2 alerts, and the first refuses every one.
            let t = TestState::with_config(webhook_config(vec![
                endpoint(server.url("/broken"), "co2_high"),
                endpoint(server.url("/ok"), "co2_high"),
            ]));
            // One attempt per endpoint, so the delivery ends as soon as both
            // endpoints have answered.
            let settings = WebhookSettings {
                retry: RetryPolicy {
                    attempts: 1,
                    base_delay: Duration::ZERO,
                },
                ..WebhookSettings::default()
            };
            let dispatcher = start_dispatcher(&t, settings).await;

            t.state.readings_tx.send(reading_for(X, 1500, 80)).unwrap();
            server.wait_for_requests(2).await;
            // Each endpoint's answer has its own log line, and the delivery
            // returns with the later of the two without waiting on anything
            // else. So, as in `wait_until_delivered`, the delivery has ended
            // by the time the test can see both lines.
            let refused = format!(
                "Webhook to endpoint #1 ({}/…) failed after 1 attempts",
                server.url("")
            );
            let sent = format!("Sent co2_high webhook for {X} to endpoint #2 ");
            wait_until("both endpoints to answer", || {
                let text = logs.text();
                text.contains(&refused) && text.contains(&sent)
            })
            .await;

            // Still over the threshold, a moment into the 300 s cooldown.
            t.state.readings_tx.send(reading_for(X, 1600, 80)).unwrap();
            wait_until_readings_taken(&t).await;

            // Shutdown lets a delivery that was started finish, so a second
            // alert would have reached the endpoints by the time the
            // dispatcher returns.
            shut_down(&t, dispatcher).await;
            let text = logs.text();
            assert!(
                text.contains(&format!("Skipping co2_high alert for {X} (cooldown: ")),
                "an alert that reached one of its two endpoints started no cooldown:\n{text}"
            );
            let mut paths: Vec<String> = server.requests().into_iter().map(|r| r.path).collect();
            paths.sort();
            assert_eq!(paths, ["/broken", "/ok"], "{:?}", server.requests());
        }
    }
}
