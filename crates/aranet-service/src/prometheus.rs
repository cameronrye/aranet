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
//! - `aranet_collector_running`
//! - `aranet_collector_uptime_seconds`
//! - `aranet_ws_messages_dropped_total`
//! - `aranet_device_poll_success_total`
//! - `aranet_device_poll_failure_total`
//! - `aranet_device_poll_duration_ms`
//! - `aranet_co2_ppm`
//! - `aranet_temperature_celsius`
//! - `aranet_humidity_percent`
//! - `aranet_pressure_hpa`
//! - `aranet_battery_percent`
//! - `aranet_reading_age_seconds`
//! - `aranet_radon_bqm3`
//! - `aranet_radiation_rate_usvh`
//! - `aranet_radiation_total_msv`

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
        let shutdown_rx = self.state.subscribe_shutdown();

        tokio::spawn(async move {
            run_prometheus_pusher(state, prometheus_config, push_gateway, shutdown_rx).await;
        });
    }
}

/// Run the Prometheus push loop.
async fn run_prometheus_pusher(
    state: Arc<AppState>,
    config: PrometheusConfig,
    push_gateway: String,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    let client = match Client::builder().timeout(Duration::from_secs(30)).build() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to create HTTP client for Prometheus pusher: {e}");
            return;
        }
    };

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
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
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
        output.push_str(&format!(
            "aranet_collector_uptime_seconds {}\n",
            uptime_secs
        ));
    }

    // WebSocket broadcast lag metric
    let dropped = state
        .ws_messages_dropped
        .load(std::sync::atomic::Ordering::Relaxed);
    output.push_str(
        "# HELP aranet_ws_messages_dropped_total Broadcast messages dropped due to slow WebSocket subscribers\n",
    );
    output.push_str("# TYPE aranet_ws_messages_dropped_total counter\n");
    output.push_str(&format!("aranet_ws_messages_dropped_total {}\n", dropped));

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

        output.push_str(
            "# HELP aranet_device_poll_duration_ms Duration of the last poll in milliseconds\n",
        );
        output.push_str("# TYPE aranet_device_poll_duration_ms gauge\n");
        for stat in device_stats.iter() {
            if let Some(duration_ms) = stat.last_poll_duration_ms {
                let alias = stat.alias.as_deref().unwrap_or(&stat.device_id);
                output.push_str(&format!(
                    "aranet_device_poll_duration_ms{{device=\"{}\",address=\"{}\"}} {}\n",
                    escape_label_value(alias),
                    escape_label_value(&stat.device_id),
                    duration_ms
                ));
            }
        }
    }
    drop(device_stats);

    // Get latest readings for all devices in a single pass
    // This avoids O(N²) behavior from calling get_latest_reading for each metric type
    let device_readings = state
        .with_store_read(|store| store.list_latest_readings())
        .await
        .unwrap_or_default();

    if !device_readings.is_empty() {
        // Build per-device metrics, filtering by device capabilities
        let mut co2_lines = Vec::new();
        let mut temp_lines = Vec::new();
        let mut humidity_lines = Vec::new();
        let mut pressure_lines = Vec::new();
        let mut battery_lines = Vec::new();
        let mut radon_lines = Vec::new();
        let mut reading_age_lines = Vec::new();
        let mut radiation_rate_lines = Vec::new();
        let mut radiation_total_lines = Vec::new();

        for (device, reading) in &device_readings {
            let device_name = device.name.as_deref().unwrap_or(&device.id);
            let labels = format!(
                "device=\"{}\",address=\"{}\"",
                escape_label_value(device_name),
                escape_label_value(&device.id)
            );

            let device_type = resolve_device_type(device);

            if device_type.is_none_or(|dt| dt.has_co2()) && reading.co2 > 0 {
                co2_lines.push(format!("aranet_co2_ppm{{{}}} {}\n", labels, reading.co2));
            }
            if device_type.is_none_or(|dt| dt.has_temperature()) {
                temp_lines.push(format!(
                    "aranet_temperature_celsius{{{}}} {:.2}\n",
                    labels, reading.temperature
                ));
            }
            if device_type.is_none_or(|dt| dt.has_humidity()) {
                humidity_lines.push(format!(
                    "aranet_humidity_percent{{{}}} {}\n",
                    labels, reading.humidity
                ));
            }
            if device_type.is_none_or(|dt| dt.has_pressure()) {
                pressure_lines.push(format!(
                    "aranet_pressure_hpa{{{}}} {:.2}\n",
                    labels, reading.pressure
                ));
            }
            battery_lines.push(format!(
                "aranet_battery_percent{{{}}} {}\n",
                labels, reading.battery
            ));
            if reading.age > 0 {
                reading_age_lines.push(format!(
                    "aranet_reading_age_seconds{{{}}} {}\n",
                    labels, reading.age
                ));
            }
            if let Some(radon) = reading.radon {
                radon_lines.push(format!("aranet_radon_bqm3{{{}}} {}\n", labels, radon));
            }
            if let Some(rate) = reading.radiation_rate {
                radiation_rate_lines.push(format!(
                    "aranet_radiation_rate_usvh{{{}}} {:.4}\n",
                    labels, rate
                ));
            }
            if let Some(total) = reading.radiation_total {
                radiation_total_lines.push(format!(
                    "aranet_radiation_total_msv{{{}}} {:.6}\n",
                    labels, total
                ));
            }
        }

        if !co2_lines.is_empty() {
            output.push_str("# HELP aranet_co2_ppm CO2 concentration in ppm\n");
            output.push_str("# TYPE aranet_co2_ppm gauge\n");
            for line in &co2_lines {
                output.push_str(line);
            }
        }
        if !temp_lines.is_empty() {
            output.push_str("# HELP aranet_temperature_celsius Temperature\n");
            output.push_str("# TYPE aranet_temperature_celsius gauge\n");
            for line in &temp_lines {
                output.push_str(line);
            }
        }
        if !humidity_lines.is_empty() {
            output.push_str("# HELP aranet_humidity_percent Relative humidity\n");
            output.push_str("# TYPE aranet_humidity_percent gauge\n");
            for line in &humidity_lines {
                output.push_str(line);
            }
        }
        if !pressure_lines.is_empty() {
            output.push_str("# HELP aranet_pressure_hpa Atmospheric pressure\n");
            output.push_str("# TYPE aranet_pressure_hpa gauge\n");
            for line in &pressure_lines {
                output.push_str(line);
            }
        }
        if !battery_lines.is_empty() {
            output.push_str("# HELP aranet_battery_percent Battery level\n");
            output.push_str("# TYPE aranet_battery_percent gauge\n");
            for line in &battery_lines {
                output.push_str(line);
            }
        }
        if !reading_age_lines.is_empty() {
            output.push_str(
                "# HELP aranet_reading_age_seconds Time since the last measurement in seconds\n",
            );
            output.push_str("# TYPE aranet_reading_age_seconds gauge\n");
            for line in &reading_age_lines {
                output.push_str(line);
            }
        }
        if !radon_lines.is_empty() {
            output.push_str("# HELP aranet_radon_bqm3 Radon concentration in Bq/m³\n");
            output.push_str("# TYPE aranet_radon_bqm3 gauge\n");
            for line in &radon_lines {
                output.push_str(line);
            }
        }
        if !radiation_rate_lines.is_empty() {
            output.push_str("# HELP aranet_radiation_rate_usvh Radiation rate in µSv/h\n");
            output.push_str("# TYPE aranet_radiation_rate_usvh gauge\n");
            for line in &radiation_rate_lines {
                output.push_str(line);
            }
        }
        if !radiation_total_lines.is_empty() {
            output.push_str("# HELP aranet_radiation_total_msv Total radiation dose in mSv\n");
            output.push_str("# TYPE aranet_radiation_total_msv gauge\n");
            for line in &radiation_total_lines {
                output.push_str(line);
            }
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
    let url = format!(
        "{}/metrics/job/{}",
        push_gateway.trim_end_matches('/'),
        job_name
    );

    let response = client
        .post(&url)
        .header("Content-Type", "text/plain; version=0.0.4")
        .body(metrics.to_string())
        .send()
        .await
        .map_err(PushError::Request)?;

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

/// Resolve a device's type from stored metadata, falling back to name-based detection.
fn resolve_device_type(device: &aranet_store::StoredDevice) -> Option<aranet_types::DeviceType> {
    device.device_type.or_else(|| {
        device
            .name
            .as_deref()
            .and_then(aranet_types::DeviceType::from_name)
            .or_else(|| aranet_types::DeviceType::from_name(&device.id))
    })
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
    Request(#[from] reqwest::Error),
    #[error("Push gateway returned error {status}: {body}")]
    Response { status: u16, body: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use aranet_store::Store;
    use aranet_types::{CurrentReading, Status};

    #[test]
    fn test_escape_label_value() {
        assert_eq!(escape_label_value("hello"), "hello");
        assert_eq!(escape_label_value("hello\"world"), "hello\\\"world");
        assert_eq!(escape_label_value("hello\\world"), "hello\\\\world");
        assert_eq!(escape_label_value("hello\nworld"), "hello\\nworld");
    }

    /// Verify that metrics are filtered by device capabilities:
    /// - An Aranet2 device should NOT emit `aranet_co2_ppm` or `aranet_pressure_hpa`.
    /// - An Aranet4 device should emit all sensor metrics.
    #[tokio::test]
    async fn test_metrics_filtered_by_device_capability() {
        let store = Store::open_in_memory().unwrap();
        let config = Config::default();
        let state = AppState::new(store, config);

        // Insert an Aranet4 device with a reading.
        {
            let store = state.store.lock().await;
            store
                .upsert_device("Aranet4 AAAAA", Some("Aranet4 AAAAA"))
                .unwrap();
            let reading = CurrentReading {
                co2: 800,
                temperature: 22.5,
                pressure: 1013.2,
                humidity: 45,
                battery: 85,
                status: Status::Green,
                interval: 300,
                age: 60,
                ..Default::default()
            };
            store.insert_reading("Aranet4 AAAAA", &reading).unwrap();
        }

        // Insert an Aranet2 device with a reading (no CO2 or pressure).
        {
            let store = state.store.lock().await;
            store
                .upsert_device("Aranet2 BBBBB", Some("Aranet2 BBBBB"))
                .unwrap();
            let reading = CurrentReading {
                co2: 0,
                temperature: 21.0,
                pressure: 0.0,
                humidity: 55,
                battery: 90,
                status: Status::Green,
                interval: 300,
                age: 60,
                ..Default::default()
            };
            store.insert_reading("Aranet2 BBBBB", &reading).unwrap();
        }

        let metrics = generate_metrics(&state).await;

        // Aranet4 should have CO2 and pressure metrics.
        assert!(
            metrics.contains("aranet_co2_ppm{device=\"Aranet4 AAAAA\""),
            "Aranet4 should emit CO2 metric"
        );
        assert!(
            metrics.contains("aranet_pressure_hpa{device=\"Aranet4 AAAAA\""),
            "Aranet4 should emit pressure metric"
        );

        // Aranet2 should NOT have CO2 or pressure metrics.
        assert!(
            !metrics.contains("aranet_co2_ppm{device=\"Aranet2 BBBBB\""),
            "Aranet2 should not emit CO2 metric"
        );
        assert!(
            !metrics.contains("aranet_pressure_hpa{device=\"Aranet2 BBBBB\""),
            "Aranet2 should not emit pressure metric"
        );

        // Aranet2 should still have temperature and humidity.
        assert!(
            metrics.contains("aranet_temperature_celsius{device=\"Aranet2 BBBBB\""),
            "Aranet2 should emit temperature metric"
        );
        assert!(
            metrics.contains("aranet_humidity_percent{device=\"Aranet2 BBBBB\""),
            "Aranet2 should emit humidity metric"
        );

        // Both should have battery.
        assert!(
            metrics.contains("aranet_battery_percent{device=\"Aranet4 AAAAA\""),
            "Aranet4 should emit battery metric"
        );
        assert!(
            metrics.contains("aranet_battery_percent{device=\"Aranet2 BBBBB\""),
            "Aranet2 should emit battery metric"
        );
    }

    /// Verify that poll duration metric is emitted when stats have a value.
    #[tokio::test]
    async fn test_poll_duration_metric_emitted() {
        let store = Store::open_in_memory().unwrap();
        let config = Config::default();
        let state = AppState::new(store, config);

        // Add device stats with a poll duration.
        {
            let mut stats = state.collector.device_stats.write().await;
            stats.push(crate::state::DeviceCollectionStats {
                device_id: "test-device".to_string(),
                alias: Some("Test".to_string()),
                poll_interval: 60,
                last_poll_at: None,
                last_error_at: None,
                last_error: None,
                last_poll_duration_ms: Some(1234),
                success_count: 1,
                failure_count: 0,
                polling: false,
            });
        }

        let metrics = generate_metrics(&state).await;
        assert!(
            metrics.contains("aranet_device_poll_duration_ms{device=\"Test\""),
            "Should emit poll duration metric"
        );
        assert!(
            metrics.contains("1234"),
            "Poll duration should contain the value"
        );
    }
}
