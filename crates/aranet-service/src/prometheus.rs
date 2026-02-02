//! Prometheus push gateway integration.
//!
//! This module provides a client that pushes metrics to a Prometheus push gateway
//! at configurable intervals.
//!
//! # Example Configuration
//!
//! ```toml
//! [prometheus]
//! enabled = true
//! push_gateway = "http://localhost:9091"
//! push_interval = 60  # seconds
//! ```
//!
//! # Metrics Pushed
//!
//! The same metrics exposed by `/metrics` endpoint are pushed:
//! - `aranet_co2_ppm`
//! - `aranet_temperature_celsius`
//! - `aranet_humidity_percent`
//! - `aranet_pressure_hpa`
//! - `aranet_battery_percent`
//! - `aranet_collector_running`
//! - `aranet_collector_uptime_seconds`

use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use time::OffsetDateTime;
use tracing::{debug, info, warn};

use crate::config::PrometheusConfig;
use crate::state::AppState;

/// Prometheus push gateway client.
pub struct PrometheusPusher {
    state: Arc<AppState>,
}

impl PrometheusPusher {
    /// Create a new Prometheus pusher.
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Start the Prometheus push gateway client.
    ///
    /// This spawns a background task that periodically pushes metrics to the
    /// configured push gateway URL.
    ///
    /// Returns immediately; pushing happens in the background.
    pub async fn start(&self) {
        let config = self.state.config.read().await;
        let prometheus_config = config.prometheus.clone();
        drop(config);

        if !prometheus_config.enabled {
            debug!("Prometheus metrics endpoint is enabled but push gateway is not configured");
            return;
        }

        let push_gateway = match &prometheus_config.push_gateway {
            Some(url) if !url.is_empty() => url.clone(),
            _ => {
                debug!("Prometheus push gateway URL not configured");
                return;
            }
        };

        info!(
            "Starting Prometheus pusher to {} (interval: {}s)",
            push_gateway, prometheus_config.push_interval
        );

        let state = Arc::clone(&self.state);
        let stop_rx = self.state.collector.subscribe_stop();

        tokio::spawn(async move {
            run_prometheus_pusher(state, prometheus_config, push_gateway, stop_rx).await;
        });
    }
}

/// Run the Prometheus push loop.
async fn run_prometheus_pusher(
    state: Arc<AppState>,
    config: PrometheusConfig,
    push_gateway: String,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
) {
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to create HTTP client");

    let push_interval = Duration::from_secs(config.push_interval);
    let mut interval = tokio::time::interval(push_interval);

    // Job name for the push gateway
    let job_name = "aranet-service";

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let metrics = generate_metrics(&state).await;
                if let Err(e) = push_metrics(&client, &push_gateway, job_name, &metrics).await {
                    warn!("Failed to push metrics to Prometheus: {}", e);
                } else {
                    debug!("Pushed metrics to Prometheus push gateway");
                }
            }
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    info!("Prometheus pusher received stop signal");
                    break;
                }
            }
        }
    }

    info!("Prometheus pusher stopped");
}

/// Generate metrics in Prometheus text format.
async fn generate_metrics(state: &AppState) -> String {
    let mut output = String::with_capacity(4096);

    // Collector status metrics
    let running = state.collector.is_running();
    let uptime = state.collector.started_at().map(|s| {
        let now = OffsetDateTime::now_utc();
        (now - s).whole_seconds().max(0)
    });

    output.push_str("# HELP aranet_collector_running Whether the collector is running\n");
    output.push_str("# TYPE aranet_collector_running gauge\n");
    output.push_str(&format!(
        "aranet_collector_running {}\n",
        if running { 1 } else { 0 }
    ));

    if let Some(uptime_secs) = uptime {
        output.push_str("# HELP aranet_collector_uptime_seconds Collector uptime\n");
        output.push_str("# TYPE aranet_collector_uptime_seconds gauge\n");
        output.push_str(&format!("aranet_collector_uptime_seconds {}\n", uptime_secs));
    }

    // Device collection stats
    let device_stats = state.collector.device_stats.read().await;
    if !device_stats.is_empty() {
        output.push_str("# HELP aranet_device_poll_success_total Successful polls\n");
        output.push_str("# TYPE aranet_device_poll_success_total counter\n");
        for stat in device_stats.iter() {
            let alias = stat.alias.as_deref().unwrap_or(&stat.device_id);
            output.push_str(&format!(
                "aranet_device_poll_success_total{{device=\"{}\",address=\"{}\"}} {}\n",
                escape_label_value(alias),
                escape_label_value(&stat.device_id),
                stat.success_count
            ));
        }

        output.push_str("# HELP aranet_device_poll_failure_total Failed polls\n");
        output.push_str("# TYPE aranet_device_poll_failure_total counter\n");
        for stat in device_stats.iter() {
            let alias = stat.alias.as_deref().unwrap_or(&stat.device_id);
            output.push_str(&format!(
                "aranet_device_poll_failure_total{{device=\"{}\",address=\"{}\"}} {}\n",
                escape_label_value(alias),
                escape_label_value(&stat.device_id),
                stat.failure_count
            ));
        }
    }
    drop(device_stats);

    // Get latest readings for all devices in a single pass
    // This avoids O(N²) behavior from calling get_latest_reading for each metric type
    let device_readings: Vec<_> = {
        let store = state.store.lock().await;
        let devices = store.list_devices().unwrap_or_default();
        devices
            .into_iter()
            .filter_map(|device| {
                store
                    .get_latest_reading(&device.id)
                    .ok()
                    .flatten()
                    .map(|reading| (device, reading))
            })
            .collect()
    }; // Lock released here

    if !device_readings.is_empty() {
        // CO2
        output.push_str("# HELP aranet_co2_ppm CO2 concentration in ppm\n");
        output.push_str("# TYPE aranet_co2_ppm gauge\n");
        for (device, reading) in &device_readings {
            let device_name = device.name.as_deref().unwrap_or(&device.id);
            let labels = format!(
                "device=\"{}\",address=\"{}\"",
                escape_label_value(device_name),
                escape_label_value(&device.id)
            );
            output.push_str(&format!("aranet_co2_ppm{{{}}} {}\n", labels, reading.co2));
        }

        // Temperature
        output.push_str("# HELP aranet_temperature_celsius Temperature\n");
        output.push_str("# TYPE aranet_temperature_celsius gauge\n");
        for (device, reading) in &device_readings {
            let device_name = device.name.as_deref().unwrap_or(&device.id);
            let labels = format!(
                "device=\"{}\",address=\"{}\"",
                escape_label_value(device_name),
                escape_label_value(&device.id)
            );
            output.push_str(&format!(
                "aranet_temperature_celsius{{{}}} {:.2}\n",
                labels, reading.temperature
            ));
        }

        // Humidity
        output.push_str("# HELP aranet_humidity_percent Relative humidity\n");
        output.push_str("# TYPE aranet_humidity_percent gauge\n");
        for (device, reading) in &device_readings {
            let device_name = device.name.as_deref().unwrap_or(&device.id);
            let labels = format!(
                "device=\"{}\",address=\"{}\"",
                escape_label_value(device_name),
                escape_label_value(&device.id)
            );
            output.push_str(&format!(
                "aranet_humidity_percent{{{}}} {}\n",
                labels, reading.humidity
            ));
        }

        // Pressure
        output.push_str("# HELP aranet_pressure_hpa Atmospheric pressure\n");
        output.push_str("# TYPE aranet_pressure_hpa gauge\n");
        for (device, reading) in &device_readings {
            let device_name = device.name.as_deref().unwrap_or(&device.id);
            let labels = format!(
                "device=\"{}\",address=\"{}\"",
                escape_label_value(device_name),
                escape_label_value(&device.id)
            );
            output.push_str(&format!(
                "aranet_pressure_hpa{{{}}} {:.2}\n",
                labels, reading.pressure
            ));
        }

        // Battery
        output.push_str("# HELP aranet_battery_percent Battery level\n");
        output.push_str("# TYPE aranet_battery_percent gauge\n");
        for (device, reading) in &device_readings {
            let device_name = device.name.as_deref().unwrap_or(&device.id);
            let labels = format!(
                "device=\"{}\",address=\"{}\"",
                escape_label_value(device_name),
                escape_label_value(&device.id)
            );
            output.push_str(&format!(
                "aranet_battery_percent{{{}}} {}\n",
                labels, reading.battery
            ));
        }
    }

    output
}

/// Push metrics to the Prometheus push gateway.
async fn push_metrics(
    client: &Client,
    push_gateway: &str,
    job_name: &str,
    metrics: &str,
) -> Result<(), PushError> {
    // Prometheus push gateway URL format: {gateway}/metrics/job/{job_name}
    let url = format!("{}/metrics/job/{}", push_gateway.trim_end_matches('/'), job_name);

    let response = client
        .post(&url)
        .header("Content-Type", "text/plain; version=0.0.4")
        .body(metrics.to_string())
        .send()
        .await
        .map_err(|e| PushError::Request(e.to_string()))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(PushError::Response {
            status: status.as_u16(),
            body,
        });
    }

    Ok(())
}

/// Escape special characters in Prometheus label values.
fn escape_label_value(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Errors that can occur when pushing metrics.
#[derive(Debug, thiserror::Error)]
pub enum PushError {
    #[error("Request failed: {0}")]
    Request(String),
    #[error("Push gateway returned error {status}: {body}")]
    Response { status: u16, body: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_label_value() {
        assert_eq!(escape_label_value("hello"), "hello");
        assert_eq!(escape_label_value("hello\"world"), "hello\\\"world");
        assert_eq!(escape_label_value("hello\\world"), "hello\\\\world");
        assert_eq!(escape_label_value("hello\nworld"), "hello\\nworld");
    }
}
