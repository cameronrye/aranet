//! REST API endpoints for the aranet-service.
//!
//! This module provides HTTP endpoints for managing devices, readings, and the collector.
//!
//! # Concurrency and Lock Acquisition
//!
//! All async handlers that access shared state acquire locks in a consistent order:
//!
//! - **`state.store`** (Mutex): Acquired for database operations. Held briefly during
//!   queries; avoid long-running operations while holding this lock.
//! - **`state.config`** (RwLock): Read lock for `get_*` endpoints, write lock for mutations.
//!   Multiple readers allowed; writers are exclusive.
//! - **`state.collector.device_stats`** (RwLock): Per-device collection statistics.
//!
//! ## Lock Ordering
//!
//! When multiple locks are needed, acquire in this order to prevent deadlocks:
//! 1. `config` (if needed)
//! 2. `store` (if needed)
//! 3. `device_stats` (if needed)
//!
//! ## Error Handling
//!
//! All endpoints return structured JSON errors via [`AppError`]. Store errors are
//! automatically converted and return HTTP 500. Client errors (not found, bad request,
//! conflict) return appropriate 4xx status codes.
//!
//! # Example
//!
//! ```ignore
//! use axum::Router;
//! use aranet_service::api;
//!
//! let app = api::router().with_state(state);
//! ```

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::collector::Collector;
use crate::config::DeviceConfig;
use crate::state::{AppState, DeviceCollectionStats};

/// Create the API router.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        // Health and status
        .route("/api/health", get(health))
        .route("/api/health/detailed", get(health_detailed))
        .route("/api/status", get(get_status))
        // Prometheus metrics
        .route("/metrics", get(prometheus_metrics))
        // Collector control
        .route("/api/collector/start", post(collector_start))
        .route("/api/collector/stop", post(collector_stop))
        // Configuration
        .route("/api/config", get(get_config).put(update_config))
        // Device management (monitored devices)
        .route("/api/config/devices", post(add_device))
        .route(
            "/api/config/devices/{id}",
            put(update_device).delete(remove_device),
        )
        // Data endpoints
        .route("/api/devices", get(list_devices))
        .route("/api/devices/{id}", get(get_device))
        .route("/api/devices/{id}/current", get(get_current_reading))
        .route("/api/devices/{id}/readings", get(get_readings))
        .route("/api/devices/{id}/history", get(get_history))
        .route("/api/readings", get(get_all_readings))
}

/// Health check response.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

/// Health check endpoint.
async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        timestamp: OffsetDateTime::now_utc(),
    })
}

/// Detailed health check response with diagnostics.
#[derive(Debug, Serialize)]
pub struct DetailedHealthResponse {
    pub status: &'static str,
    pub version: &'static str,
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
    /// Database health status
    pub database: DatabaseHealth,
    /// Collector health status
    pub collector: CollectorHealth,
    /// Platform information
    pub platform: PlatformInfo,
}

/// Database health information.
#[derive(Debug, Serialize)]
pub struct DatabaseHealth {
    /// Whether the database is accessible
    pub ok: bool,
    /// Number of devices in database
    pub device_count: usize,
    /// Number of readings in database (if available)
    pub reading_count: Option<usize>,
    /// Error message if database is not ok
    pub error: Option<String>,
}

/// Collector health information.
#[derive(Debug, Serialize)]
pub struct CollectorHealth {
    /// Whether the collector is running
    pub running: bool,
    /// Number of configured devices
    pub configured_devices: usize,
    /// Number of devices with recent successful polls
    pub healthy_devices: usize,
    /// Number of devices with recent failures
    pub failing_devices: usize,
}

/// Platform information.
#[derive(Debug, Serialize)]
pub struct PlatformInfo {
    /// Operating system
    pub os: &'static str,
    /// CPU architecture
    pub arch: &'static str,
}

/// Detailed health check endpoint with diagnostics.
///
/// This endpoint performs actual health checks on subsystems:
/// - Database connectivity and counts
/// - Collector status and device health
/// - Platform information
///
/// Note: This endpoint acquires locks on store and device_stats.
/// For high-frequency monitoring, prefer `/api/health`.
async fn health_detailed(
    State(state): State<Arc<AppState>>,
) -> Json<DetailedHealthResponse> {
    // Check database health
    let database = {
        let store = state.store.lock().await;
        match store.list_devices() {
            Ok(devices) => DatabaseHealth {
                ok: true,
                device_count: devices.len(),
                reading_count: None, // Skip count for performance
                error: None,
            },
            Err(e) => DatabaseHealth {
                ok: false,
                device_count: 0,
                reading_count: None,
                error: Some(e.to_string()),
            },
        }
    };

    // Check collector health
    let collector = {
        let config = state.config.read().await;
        let configured_devices = config.devices.len();
        drop(config);

        let stats = state.collector.device_stats.read().await;
        let healthy_devices = stats
            .iter()
            .filter(|s| {
                // Consider healthy if last poll was within 3x poll interval
                s.last_poll_at.map_or(false, |t| {
                    let age = (OffsetDateTime::now_utc() - t).whole_seconds();
                    age < (s.poll_interval as i64 * 3)
                })
            })
            .count();
        let failing_devices = stats
            .iter()
            .filter(|s| s.failure_count > 0 && s.last_error.is_some())
            .count();

        CollectorHealth {
            running: state.collector.is_running(),
            configured_devices,
            healthy_devices,
            failing_devices,
        }
    };

    // Platform info
    let platform = PlatformInfo {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
    };

    // Determine overall status
    let status = if database.ok && collector.running {
        "ok"
    } else if database.ok {
        "degraded"
    } else {
        "unhealthy"
    };

    Json(DetailedHealthResponse {
        status,
        version: env!("CARGO_PKG_VERSION"),
        timestamp: OffsetDateTime::now_utc(),
        database,
        collector,
        platform,
    })
}

// ==========================================================================
// Prometheus Metrics
// ==========================================================================

/// Content type for Prometheus metrics.
const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Prometheus metrics endpoint.
///
/// Returns metrics in Prometheus text format for scraping by Prometheus/Grafana.
///
/// # Metrics Exported
///
/// ## Sensor Readings (per device)
/// - `aranet_co2_ppm` - CO2 concentration in parts per million
/// - `aranet_temperature_celsius` - Temperature in degrees Celsius
/// - `aranet_humidity_percent` - Relative humidity percentage
/// - `aranet_pressure_hpa` - Atmospheric pressure in hectopascals
/// - `aranet_battery_percent` - Battery level percentage
/// - `aranet_reading_age_seconds` - Age of the reading in seconds
///
/// ## Radon/Radiation (if available)
/// - `aranet_radon_bqm3` - Radon concentration in Bq/m³
/// - `aranet_radiation_rate_usvh` - Radiation rate in µSv/h
/// - `aranet_radiation_total_msv` - Total radiation dose in mSv
///
/// ## Collector Stats
/// - `aranet_collector_running` - Whether the collector is running (1 or 0)
/// - `aranet_collector_uptime_seconds` - Collector uptime in seconds
/// - `aranet_device_poll_success_total` - Total successful polls per device
/// - `aranet_device_poll_failure_total` - Total failed polls per device
///
/// # Lock Acquisition
///
/// Acquires read locks on config, store, and device_stats to gather metrics.
async fn prometheus_metrics(
    State(state): State<Arc<AppState>>,
) -> Result<
    (
        StatusCode,
        [(axum::http::header::HeaderName, &'static str); 1],
        String,
    ),
    AppError,
> {
    // Check if Prometheus is enabled
    let config = state.config.read().await;
    if !config.prometheus.enabled {
        return Err(AppError::NotFound(
            "Prometheus metrics endpoint is disabled".to_string(),
        ));
    }
    drop(config);

    let mut output = String::with_capacity(4096);

    // Add metadata header
    output.push_str("# Aranet sensor metrics\n");
    output.push_str(&format!(
        "# Generated at {}\n\n",
        OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default()
    ));

    // Collector status metrics
    let running = state.collector.is_running();
    let uptime = state.collector.started_at().map(|s| {
        let now = OffsetDateTime::now_utc();
        (now - s).whole_seconds().max(0)
    });

    output.push_str(
        "# HELP aranet_collector_running Whether the collector is running (1=running, 0=stopped)\n",
    );
    output.push_str("# TYPE aranet_collector_running gauge\n");
    output.push_str(&format!(
        "aranet_collector_running {}\n\n",
        if running { 1 } else { 0 }
    ));

    if let Some(uptime_secs) = uptime {
        output.push_str(
            "# HELP aranet_collector_uptime_seconds How long the collector has been running\n",
        );
        output.push_str("# TYPE aranet_collector_uptime_seconds gauge\n");
        output.push_str(&format!(
            "aranet_collector_uptime_seconds {}\n\n",
            uptime_secs
        ));
    }

    // Device collection stats
    let device_stats = state.collector.device_stats.read().await;
    if !device_stats.is_empty() {
        output
            .push_str("# HELP aranet_device_poll_success_total Total number of successful polls\n");
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
        output.push('\n');

        output.push_str("# HELP aranet_device_poll_failure_total Total number of failed polls\n");
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
        output.push('\n');

        output.push_str("# HELP aranet_device_polling Whether the device is currently being polled (1=yes, 0=no)\n");
        output.push_str("# TYPE aranet_device_polling gauge\n");
        for stat in device_stats.iter() {
            let alias = stat.alias.as_deref().unwrap_or(&stat.device_id);
            output.push_str(&format!(
                "aranet_device_polling{{device=\"{}\",address=\"{}\"}} {}\n",
                escape_label_value(alias),
                escape_label_value(&stat.device_id),
                if stat.polling { 1 } else { 0 }
            ));
        }
        output.push('\n');
    }
    drop(device_stats);

    // Get latest readings for all devices
    // Clone the data we need while holding the lock briefly, then release it
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
        // Sensor reading metrics
        output.push_str("# HELP aranet_co2_ppm CO2 concentration in parts per million\n");
        output.push_str("# TYPE aranet_co2_ppm gauge\n");

        let mut co2_metrics = Vec::new();
        let mut temp_metrics = Vec::new();
        let mut humidity_metrics = Vec::new();
        let mut pressure_metrics = Vec::new();
        let mut battery_metrics = Vec::new();
        let mut age_metrics = Vec::new();
        let mut radon_metrics = Vec::new();
        let mut radiation_rate_metrics = Vec::new();
        let mut radiation_total_metrics = Vec::new();

        for (device, reading) in &device_readings {
            let device_name = device.name.as_deref().unwrap_or(&device.id);

            let labels = format!(
                "device=\"{}\",address=\"{}\"",
                escape_label_value(device_name),
                escape_label_value(&device.id)
            );

            co2_metrics.push(format!("aranet_co2_ppm{{{}}} {}", labels, reading.co2));
            temp_metrics.push(format!(
                "aranet_temperature_celsius{{{}}} {:.2}",
                labels, reading.temperature
            ));
            humidity_metrics.push(format!(
                "aranet_humidity_percent{{{}}} {}",
                labels, reading.humidity
            ));
            pressure_metrics.push(format!(
                "aranet_pressure_hpa{{{}}} {:.2}",
                labels, reading.pressure
            ));
            battery_metrics.push(format!(
                "aranet_battery_percent{{{}}} {}",
                labels, reading.battery
            ));

            // Calculate reading age
            let age_secs = (OffsetDateTime::now_utc() - reading.captured_at)
                .whole_seconds()
                .max(0);
            age_metrics.push(format!(
                "aranet_reading_age_seconds{{{}}} {}",
                labels, age_secs
            ));

            // Radon (if available)
            if let Some(radon) = reading.radon {
                radon_metrics.push(format!("aranet_radon_bqm3{{{}}} {}", labels, radon));
            }

            // Radiation (if available)
            if let Some(rate) = reading.radiation_rate {
                radiation_rate_metrics.push(format!(
                    "aranet_radiation_rate_usvh{{{}}} {:.4}",
                    labels, rate
                ));
            }
            if let Some(total) = reading.radiation_total {
                radiation_total_metrics.push(format!(
                    "aranet_radiation_total_msv{{{}}} {:.6}",
                    labels, total
                ));
            }
        }

        // Output CO2 metrics
        for m in &co2_metrics {
            output.push_str(m);
            output.push('\n');
        }
        output.push('\n');

        // Temperature
        if !temp_metrics.is_empty() {
            output.push_str("# HELP aranet_temperature_celsius Temperature in degrees Celsius\n");
            output.push_str("# TYPE aranet_temperature_celsius gauge\n");
            for m in &temp_metrics {
                output.push_str(m);
                output.push('\n');
            }
            output.push('\n');
        }

        // Humidity
        if !humidity_metrics.is_empty() {
            output.push_str("# HELP aranet_humidity_percent Relative humidity percentage\n");
            output.push_str("# TYPE aranet_humidity_percent gauge\n");
            for m in &humidity_metrics {
                output.push_str(m);
                output.push('\n');
            }
            output.push('\n');
        }

        // Pressure
        if !pressure_metrics.is_empty() {
            output.push_str("# HELP aranet_pressure_hpa Atmospheric pressure in hectopascals\n");
            output.push_str("# TYPE aranet_pressure_hpa gauge\n");
            for m in &pressure_metrics {
                output.push_str(m);
                output.push('\n');
            }
            output.push('\n');
        }

        // Battery
        if !battery_metrics.is_empty() {
            output.push_str("# HELP aranet_battery_percent Battery level percentage\n");
            output.push_str("# TYPE aranet_battery_percent gauge\n");
            for m in &battery_metrics {
                output.push_str(m);
                output.push('\n');
            }
            output.push('\n');
        }

        // Reading age
        if !age_metrics.is_empty() {
            output.push_str("# HELP aranet_reading_age_seconds Age of the reading in seconds\n");
            output.push_str("# TYPE aranet_reading_age_seconds gauge\n");
            for m in &age_metrics {
                output.push_str(m);
                output.push('\n');
            }
            output.push('\n');
        }

        // Radon
        if !radon_metrics.is_empty() {
            output.push_str("# HELP aranet_radon_bqm3 Radon concentration in Bq/m³\n");
            output.push_str("# TYPE aranet_radon_bqm3 gauge\n");
            for m in &radon_metrics {
                output.push_str(m);
                output.push('\n');
            }
            output.push('\n');
        }

        // Radiation rate
        if !radiation_rate_metrics.is_empty() {
            output.push_str("# HELP aranet_radiation_rate_usvh Radiation rate in µSv/h\n");
            output.push_str("# TYPE aranet_radiation_rate_usvh gauge\n");
            for m in &radiation_rate_metrics {
                output.push_str(m);
                output.push('\n');
            }
            output.push('\n');
        }

        // Radiation total
        if !radiation_total_metrics.is_empty() {
            output.push_str("# HELP aranet_radiation_total_msv Total radiation dose in mSv\n");
            output.push_str("# TYPE aranet_radiation_total_msv gauge\n");
            for m in &radiation_total_metrics {
                output.push_str(m);
                output.push('\n');
            }
            output.push('\n');
        }
    }

    Ok((
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, PROMETHEUS_CONTENT_TYPE)],
        output,
    ))
}

/// Escape special characters in Prometheus label values.
fn escape_label_value(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

// ==========================================================================
// Service Status and Collector Control
// ==========================================================================

/// Service status response.
#[derive(Debug, Serialize)]
pub struct StatusResponse {
    /// Service version.
    pub version: &'static str,
    /// Current timestamp.
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
    /// Collector status.
    pub collector: CollectorStatus,
    /// Per-device collection statistics.
    pub devices: Vec<DeviceCollectionStats>,
}

/// Collector status.
#[derive(Debug, Serialize)]
pub struct CollectorStatus {
    /// Whether the collector is running.
    pub running: bool,
    /// When the collector was started (if running).
    #[serde(with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    /// How long the collector has been running (in seconds).
    pub uptime_seconds: Option<u64>,
}

/// Get service status including collector state and device stats.
///
/// # Lock Acquisition
///
/// Acquires a read lock on `collector.device_stats` to clone current statistics.
/// The lock is held only during the clone operation.
async fn get_status(State(state): State<Arc<AppState>>) -> Json<StatusResponse> {
    let running = state.collector.is_running();
    let started_at = state.collector.started_at();
    let uptime_seconds = started_at.map(|s| {
        let now = OffsetDateTime::now_utc();
        (now - s).whole_seconds().max(0) as u64
    });

    let devices = state.collector.device_stats.read().await.clone();

    Json(StatusResponse {
        version: env!("CARGO_PKG_VERSION"),
        timestamp: OffsetDateTime::now_utc(),
        collector: CollectorStatus {
            running,
            started_at,
            uptime_seconds,
        },
        devices,
    })
}

/// Response for collector control actions.
#[derive(Debug, Serialize)]
pub struct CollectorActionResponse {
    pub success: bool,
    pub message: String,
    pub running: bool,
}

/// Start the collector.
async fn collector_start(State(state): State<Arc<AppState>>) -> Json<CollectorActionResponse> {
    if state.collector.is_running() {
        return Json(CollectorActionResponse {
            success: false,
            message: "Collector is already running".to_string(),
            running: true,
        });
    }

    let mut collector = Collector::new(Arc::clone(&state));
    collector.start().await;

    Json(CollectorActionResponse {
        success: true,
        message: "Collector started".to_string(),
        running: true,
    })
}

/// Stop the collector.
async fn collector_stop(State(state): State<Arc<AppState>>) -> Json<CollectorActionResponse> {
    use std::time::Duration;

    if !state.collector.is_running() {
        return Json(CollectorActionResponse {
            success: false,
            message: "Collector is not running".to_string(),
            running: false,
        });
    }

    // Signal all collector tasks to stop through the state
    state.collector.signal_stop();

    // Wait for device tasks to complete (with timeout)
    let stopped_cleanly = state
        .collector
        .wait_for_device_tasks(Duration::from_secs(10))
        .await;

    if stopped_cleanly {
        Json(CollectorActionResponse {
            success: true,
            message: "Collector stopped".to_string(),
            running: false,
        })
    } else {
        Json(CollectorActionResponse {
            success: true,
            message: "Collector stopped (some tasks timed out and were aborted)".to_string(),
            running: false,
        })
    }
}

// ==========================================================================
// Configuration Endpoints
// ==========================================================================

/// Configuration response (excludes storage path for security).
#[derive(Debug, Serialize)]
pub struct ConfigResponse {
    pub server: ServerConfigResponse,
    pub devices: Vec<DeviceConfigResponse>,
}

/// Server configuration response.
#[derive(Debug, Serialize)]
pub struct ServerConfigResponse {
    pub bind: String,
}

/// Device configuration response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfigResponse {
    pub address: String,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default = "default_poll_interval")]
    pub poll_interval: u64,
}

fn default_poll_interval() -> u64 {
    60
}

/// Get current configuration.
///
/// # Lock Acquisition
///
/// Acquires a read lock on `config`. Multiple concurrent readers are allowed.
async fn get_config(State(state): State<Arc<AppState>>) -> Json<ConfigResponse> {
    let config = state.config.read().await;
    Json(ConfigResponse {
        server: ServerConfigResponse {
            bind: config.server.bind.clone(),
        },
        devices: config
            .devices
            .iter()
            .map(|d| DeviceConfigResponse {
                address: d.address.clone(),
                alias: d.alias.clone(),
                poll_interval: d.poll_interval,
            })
            .collect(),
    })
}

/// Request to update configuration.
#[derive(Debug, Deserialize)]
pub struct UpdateConfigRequest {
    #[serde(default)]
    pub devices: Option<Vec<DeviceConfigResponse>>,
}

/// Update configuration.
///
/// # Lock Acquisition
///
/// Acquires an exclusive write lock on `config`. This blocks other readers and writers
/// until the lock is released after validation and update complete.
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] if the new configuration fails validation.
async fn update_config(
    State(state): State<Arc<AppState>>,
    Json(request): Json<UpdateConfigRequest>,
) -> Result<Json<ConfigResponse>, AppError> {
    let response = {
        let mut config = state.config.write().await;

        if let Some(devices) = request.devices {
            config.devices = devices
                .into_iter()
                .map(|d| DeviceConfig {
                    address: d.address,
                    alias: d.alias,
                    poll_interval: d.poll_interval,
                })
                .collect();
        }

        // Validate the new config
        if let Err(e) = config.validate() {
            return Err(AppError::BadRequest(format!(
                "Invalid configuration: {}",
                e
            )));
        }

        ConfigResponse {
            server: ServerConfigResponse {
                bind: config.server.bind.clone(),
            },
            devices: config
                .devices
                .iter()
                .map(|d| DeviceConfigResponse {
                    address: d.address.clone(),
                    alias: d.alias.clone(),
                    poll_interval: d.poll_interval,
                })
                .collect(),
        }
    };

    // Persist config and signal reload if collector is running
    state.on_devices_changed().await;

    Ok(Json(response))
}

/// Request to add a device.
#[derive(Debug, Deserialize)]
pub struct AddDeviceRequest {
    pub address: String,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default = "default_poll_interval")]
    pub poll_interval: u64,
}

/// Add a device to monitor.
///
/// # Lock Acquisition
///
/// Acquires an exclusive write lock on `config` to check for duplicates and add the device.
///
/// # Errors
///
/// - [`AppError::Conflict`] if a device with the same address already exists (case-insensitive).
/// - [`AppError::BadRequest`] if the device configuration fails validation.
async fn add_device(
    State(state): State<Arc<AppState>>,
    Json(request): Json<AddDeviceRequest>,
) -> Result<(StatusCode, Json<DeviceConfigResponse>), AppError> {
    {
        let mut config = state.config.write().await;

        // Check if device already exists
        let addr_lower = request.address.to_lowercase();
        if config
            .devices
            .iter()
            .any(|d| d.address.to_lowercase() == addr_lower)
        {
            return Err(AppError::Conflict(format!(
                "Device {} is already being monitored",
                request.address
            )));
        }

        let device = DeviceConfig {
            address: request.address.clone(),
            alias: request.alias.clone(),
            poll_interval: request.poll_interval,
        };

        // Validate the device config
        let errors = device.validate("device");
        if !errors.is_empty() {
            return Err(AppError::BadRequest(
                errors
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }

        config.devices.push(device);
    }

    // Persist config and signal reload if collector is running
    state.on_devices_changed().await;

    Ok((
        StatusCode::CREATED,
        Json(DeviceConfigResponse {
            address: request.address,
            alias: request.alias,
            poll_interval: request.poll_interval,
        }),
    ))
}

/// Request to update a device.
#[derive(Debug, Deserialize)]
pub struct UpdateDeviceRequest {
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub poll_interval: Option<u64>,
}

/// Update a device configuration.
async fn update_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<UpdateDeviceRequest>,
) -> Result<Json<DeviceConfigResponse>, AppError> {
    let response = {
        let mut config = state.config.write().await;

        // Find the device (case-insensitive)
        let id_lower = id.to_lowercase();
        let device = config
            .devices
            .iter_mut()
            .find(|d| d.address.to_lowercase() == id_lower)
            .ok_or_else(|| AppError::NotFound(format!("Device {} not found in config", id)))?;

        // Update fields if provided
        if request.alias.is_some() {
            device.alias = request.alias;
        }
        if let Some(poll_interval) = request.poll_interval {
            device.poll_interval = poll_interval;
        }

        // Validate the updated device
        let errors = device.validate("device");
        if !errors.is_empty() {
            return Err(AppError::BadRequest(
                errors
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }

        DeviceConfigResponse {
            address: device.address.clone(),
            alias: device.alias.clone(),
            poll_interval: device.poll_interval,
        }
    };

    // Persist config and signal reload if collector is running
    state.on_devices_changed().await;

    Ok(Json(response))
}

/// Remove a device from monitoring.
async fn remove_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    {
        let mut config = state.config.write().await;

        // Find and remove the device (case-insensitive)
        let id_lower = id.to_lowercase();
        let original_len = config.devices.len();
        config
            .devices
            .retain(|d| d.address.to_lowercase() != id_lower);

        if config.devices.len() == original_len {
            return Err(AppError::NotFound(format!(
                "Device {} not found in config",
                id
            )));
        }
    }

    // Persist config and signal reload if collector is running
    state.on_devices_changed().await;

    Ok(StatusCode::NO_CONTENT)
}

/// Device response.
#[derive(Debug, Serialize)]
pub struct DeviceResponse {
    pub id: String,
    pub name: Option<String>,
    pub device_type: Option<String>,
    pub serial: Option<String>,
    pub firmware: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub first_seen: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub last_seen: OffsetDateTime,
}

impl From<aranet_store::StoredDevice> for DeviceResponse {
    fn from(d: aranet_store::StoredDevice) -> Self {
        Self {
            id: d.id,
            name: d.name,
            device_type: d.device_type.map(|dt| format!("{:?}", dt)),
            serial: d.serial,
            firmware: d.firmware,
            first_seen: d.first_seen,
            last_seen: d.last_seen,
        }
    }
}

/// List all devices.
///
/// # Lock Acquisition
///
/// Acquires the store mutex for the duration of the database query.
///
/// # Errors
///
/// Returns [`AppError::Store`] if the database query fails.
async fn list_devices(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<DeviceResponse>>, AppError> {
    let store = state.store.lock().await;
    let devices = store.list_devices()?;
    Ok(Json(devices.into_iter().map(Into::into).collect()))
}

/// Get a single device.
async fn get_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<DeviceResponse>, AppError> {
    let store = state.store.lock().await;
    let device = store
        .get_device(&id)?
        .ok_or(AppError::NotFound(format!("Device not found: {}", id)))?;
    Ok(Json(device.into()))
}

/// Get the latest reading for a device.
async fn get_current_reading(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<aranet_store::StoredReading>, AppError> {
    let store = state.store.lock().await;
    let reading = store
        .get_latest_reading(&id)?
        .ok_or(AppError::NotFound(format!(
            "No readings for device: {}",
            id
        )))?;
    Ok(Json(reading))
}

/// Query parameters for readings.
#[derive(Debug, Deserialize, Default)]
pub struct ReadingsQuery {
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

impl ReadingsQuery {
    /// Validate the query parameters.
    /// Returns an error if `since > until`.
    pub fn validate(&self) -> Result<(), AppError> {
        if let (Some(since), Some(until)) = (self.since, self.until)
            && since > until
        {
            return Err(AppError::BadRequest(format!(
                "Invalid time range: 'since' ({}) must be less than or equal to 'until' ({})",
                since, until
            )));
        }
        Ok(())
    }
}

/// Paginated response wrapper with metadata.
#[derive(Debug, Serialize)]
pub struct PaginatedResponse<T> {
    /// The data items.
    pub data: Vec<T>,
    /// Pagination metadata.
    pub pagination: PaginationMeta,
}

/// Pagination metadata.
#[derive(Debug, Serialize)]
pub struct PaginationMeta {
    /// Number of items returned.
    pub count: usize,
    /// Offset from the beginning.
    pub offset: u32,
    /// Maximum items requested (if specified).
    pub limit: Option<u32>,
    /// Whether there are more items available.
    pub has_more: bool,
}

/// Get readings for a device.
///
/// Returns a paginated response with readings and metadata about the results.
///
/// # Query Parameters
///
/// - `since`: Unix timestamp to filter readings from (inclusive)
/// - `until`: Unix timestamp to filter readings until (inclusive)
/// - `limit`: Maximum number of readings to return
/// - `offset`: Number of readings to skip (for pagination)
///
/// # Lock Acquisition
///
/// Acquires the store mutex for the duration of the database query.
/// Query parameters are validated before the lock is acquired.
///
/// # Errors
///
/// - Returns [`AppError::BadRequest`] if `since > until`
/// - Returns [`AppError::Store`] if the database query fails
async fn get_readings(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<ReadingsQuery>,
) -> Result<Json<PaginatedResponse<aranet_store::StoredReading>>, AppError> {
    // Validate query parameters
    params.validate()?;

    let mut query = aranet_store::ReadingQuery::new().device(&id);

    if let Some(since) = params.since
        && let Ok(dt) = OffsetDateTime::from_unix_timestamp(since)
    {
        query = query.since(dt);
    }
    if let Some(until) = params.until
        && let Ok(dt) = OffsetDateTime::from_unix_timestamp(until)
    {
        query = query.until(dt);
    }

    // Request one extra item to determine if there are more
    let request_limit = params.limit.map(|l| l + 1);
    if let Some(limit) = request_limit {
        query = query.limit(limit);
    }
    if let Some(offset) = params.offset {
        query = query.offset(offset);
    }

    let store = state.store.lock().await;
    let mut readings = store.query_readings(&query)?;

    // Check if there are more items
    let has_more = params.limit.is_some_and(|l| readings.len() > l as usize);
    if has_more {
        readings.pop(); // Remove the extra item
    }

    Ok(Json(PaginatedResponse {
        pagination: PaginationMeta {
            count: readings.len(),
            offset: params.offset.unwrap_or(0),
            limit: params.limit,
            has_more,
        },
        data: readings,
    }))
}

/// Get history for a device.
///
/// Returns a paginated response with history records and metadata.
///
/// # Errors
///
/// - Returns [`AppError::BadRequest`] if `since > until`
/// - Returns [`AppError::Store`] if the database query fails
async fn get_history(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<ReadingsQuery>,
) -> Result<Json<PaginatedResponse<aranet_store::StoredHistoryRecord>>, AppError> {
    // Validate query parameters
    params.validate()?;

    let mut query = aranet_store::HistoryQuery::new().device(&id);

    if let Some(since) = params.since
        && let Ok(dt) = OffsetDateTime::from_unix_timestamp(since)
    {
        query = query.since(dt);
    }
    if let Some(until) = params.until
        && let Ok(dt) = OffsetDateTime::from_unix_timestamp(until)
    {
        query = query.until(dt);
    }

    // Request one extra item to determine if there are more
    let request_limit = params.limit.map(|l| l + 1);
    if let Some(limit) = request_limit {
        query = query.limit(limit);
    }

    let store = state.store.lock().await;
    let mut history = store.query_history(&query)?;

    // Check if there are more items
    let has_more = params.limit.is_some_and(|l| history.len() > l as usize);
    if has_more {
        history.pop(); // Remove the extra item
    }

    Ok(Json(PaginatedResponse {
        pagination: PaginationMeta {
            count: history.len(),
            offset: params.offset.unwrap_or(0),
            limit: params.limit,
            has_more,
        },
        data: history,
    }))
}

/// Get all readings across devices.
///
/// Returns a paginated response with readings from all devices.
///
/// # Errors
///
/// - Returns [`AppError::BadRequest`] if `since > until`
/// - Returns [`AppError::Store`] if the database query fails
async fn get_all_readings(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ReadingsQuery>,
) -> Result<Json<PaginatedResponse<aranet_store::StoredReading>>, AppError> {
    // Validate query parameters
    params.validate()?;

    let mut query = aranet_store::ReadingQuery::new();

    if let Some(since) = params.since
        && let Ok(dt) = OffsetDateTime::from_unix_timestamp(since)
    {
        query = query.since(dt);
    }
    if let Some(until) = params.until
        && let Ok(dt) = OffsetDateTime::from_unix_timestamp(until)
    {
        query = query.until(dt);
    }

    // Request one extra item to determine if there are more
    let request_limit = params.limit.map(|l| l + 1);
    if let Some(limit) = request_limit {
        query = query.limit(limit);
    }
    if let Some(offset) = params.offset {
        query = query.offset(offset);
    }

    let store = state.store.lock().await;
    let mut readings = store.query_readings(&query)?;

    // Check if there are more items
    let has_more = params.limit.is_some_and(|l| readings.len() > l as usize);
    if has_more {
        readings.pop(); // Remove the extra item
    }

    Ok(Json(PaginatedResponse {
        pagination: PaginationMeta {
            count: readings.len(),
            offset: params.offset.unwrap_or(0),
            limit: params.limit,
            has_more,
        },
        data: readings,
    }))
}

/// Application error type.
#[derive(Debug)]
pub enum AppError {
    NotFound(String),
    BadRequest(String),
    Conflict(String),
    Store(aranet_store::Error),
    Internal(String),
}

impl From<aranet_store::Error> for AppError {
    fn from(e: aranet_store::Error) -> Self {
        AppError::Store(e)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::Conflict(msg) => (StatusCode::CONFLICT, msg),
            AppError::Store(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        let body = serde_json::json!({
            "error": message,
        });

        (status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::config::Config;

    fn create_test_state() -> Arc<AppState> {
        let store = aranet_store::Store::open_in_memory().unwrap();
        let config = Config::default();
        AppState::new(store, config)
    }

    async fn response_body(response: axum::response::Response) -> String {
        let body = response.into_body();
        let bytes = body.collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let state = create_test_state();
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert_eq!(json["status"], "ok");
        assert!(json["version"].is_string());
        assert!(json["timestamp"].is_string());
    }

    #[tokio::test]
    async fn test_list_devices_empty() {
        let state = create_test_state();
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/devices")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert!(json.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_get_device_not_found() {
        let state = create_test_state();
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/devices/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert!(json["error"].as_str().unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn test_get_current_reading_not_found() {
        let state = create_test_state();
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/devices/test-device/current")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_get_readings_empty() {
        let state = create_test_state();
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/devices/test/readings")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert!(json["data"].as_array().unwrap().is_empty());
        assert_eq!(json["pagination"]["count"], 0);
    }

    #[tokio::test]
    async fn test_get_all_readings_empty() {
        let state = create_test_state();
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/readings")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert!(json["data"].as_array().unwrap().is_empty());
        assert_eq!(json["pagination"]["count"], 0);
    }

    #[tokio::test]
    async fn test_get_history_empty() {
        let state = create_test_state();
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/devices/test/history")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert!(json["data"].as_array().unwrap().is_empty());
        assert_eq!(json["pagination"]["count"], 0);
    }

    #[tokio::test]
    async fn test_readings_query_params() {
        let state = create_test_state();
        let app = router().with_state(state);

        // Test with query parameters
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/devices/test/readings?since=1704067200&until=1704153600&limit=10&offset=0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn test_health_response_serialization() {
        let response = HealthResponse {
            status: "ok",
            version: "0.1.0",
            timestamp: time::OffsetDateTime::now_utc(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("ok"));
        assert!(json.contains("0.1.0"));
    }

    #[test]
    fn test_device_response_from_stored_device() {
        let stored = aranet_store::StoredDevice {
            id: "AA:BB:CC:DD:EE:FF".to_string(),
            name: Some("Test Device".to_string()),
            device_type: Some(aranet_types::DeviceType::Aranet4),
            serial: Some("12345".to_string()),
            firmware: Some("1.2.3".to_string()),
            hardware: Some("2.0".to_string()),
            first_seen: time::OffsetDateTime::now_utc(),
            last_seen: time::OffsetDateTime::now_utc(),
        };

        let response: DeviceResponse = stored.into();

        assert_eq!(response.id, "AA:BB:CC:DD:EE:FF");
        assert_eq!(response.name, Some("Test Device".to_string()));
        assert_eq!(response.device_type, Some("Aranet4".to_string()));
        assert_eq!(response.serial, Some("12345".to_string()));
        assert_eq!(response.firmware, Some("1.2.3".to_string()));
    }

    #[test]
    fn test_readings_query_default() {
        let query = ReadingsQuery::default();
        assert!(query.since.is_none());
        assert!(query.until.is_none());
        assert!(query.limit.is_none());
        assert!(query.offset.is_none());
    }

    #[test]
    fn test_app_error_not_found() {
        let error = AppError::NotFound("test".to_string());
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_app_error_internal() {
        let error = AppError::Internal("internal error".to_string());
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_app_error_debug() {
        let error = AppError::NotFound("test".to_string());
        let debug = format!("{:?}", error);
        assert!(debug.contains("NotFound"));
        assert!(debug.contains("test"));
    }

    #[test]
    fn test_app_error_bad_request() {
        let error = AppError::BadRequest("invalid input".to_string());
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_app_error_conflict() {
        let error = AppError::Conflict("resource exists".to_string());
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn test_get_status_endpoint() {
        let state = create_test_state();
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert!(json["version"].is_string());
        assert!(json["timestamp"].is_string());
        assert!(json["collector"].is_object());
        assert!(json["collector"]["running"].is_boolean());
        assert!(json["devices"].is_array());
    }

    #[tokio::test]
    async fn test_get_config_endpoint() {
        let state = create_test_state();
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert!(json["server"].is_object());
        assert!(json["server"]["bind"].is_string());
        assert!(json["devices"].is_array());
    }

    #[tokio::test]
    async fn test_add_device_endpoint() {
        let state = create_test_state();
        let app = router().with_state(Arc::clone(&state));

        let request_body = serde_json::json!({
            "address": "AA:BB:CC:DD:EE:FF",
            "alias": "Test Device",
            "poll_interval": 120
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/config/devices")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);

        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert_eq!(json["address"], "AA:BB:CC:DD:EE:FF");
        assert_eq!(json["alias"], "Test Device");
        assert_eq!(json["poll_interval"], 120);
    }

    #[tokio::test]
    async fn test_add_duplicate_device() {
        let state = create_test_state();

        // Add first device
        {
            let mut config = state.config.write().await;
            config.devices.push(DeviceConfig {
                address: "AA:BB:CC:DD:EE:FF".to_string(),
                alias: Some("First".to_string()),
                poll_interval: 60,
            });
        }

        let app = router().with_state(state);

        let request_body = serde_json::json!({
            "address": "AA:BB:CC:DD:EE:FF",
            "alias": "Duplicate"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/config/devices")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn test_update_device_endpoint() {
        let state = create_test_state();

        // Add a device first
        {
            let mut config = state.config.write().await;
            config.devices.push(DeviceConfig {
                address: "AA:BB:CC:DD:EE:FF".to_string(),
                alias: Some("Original".to_string()),
                poll_interval: 60,
            });
        }

        let app = router().with_state(state);

        let request_body = serde_json::json!({
            "alias": "Updated Name",
            "poll_interval": 300
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/config/devices/AA:BB:CC:DD:EE:FF")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert_eq!(json["alias"], "Updated Name");
        assert_eq!(json["poll_interval"], 300);
    }

    #[tokio::test]
    async fn test_update_nonexistent_device() {
        let state = create_test_state();
        let app = router().with_state(state);

        let request_body = serde_json::json!({
            "alias": "New Name"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/config/devices/NONEXISTENT")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_remove_device_endpoint() {
        let state = create_test_state();

        // Add a device first
        {
            let mut config = state.config.write().await;
            config.devices.push(DeviceConfig {
                address: "AA:BB:CC:DD:EE:FF".to_string(),
                alias: Some("To Remove".to_string()),
                poll_interval: 60,
            });
        }

        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/config/devices/AA:BB:CC:DD:EE:FF")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn test_remove_nonexistent_device() {
        let state = create_test_state();
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/config/devices/NONEXISTENT")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_collector_start_stop() {
        let state = create_test_state();
        let app = router().with_state(Arc::clone(&state));

        // Start collector (no devices, so just validates the endpoint)
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/collector/start")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert!(json["success"].as_bool().unwrap());
        assert_eq!(json["message"], "Collector started");
    }

    #[tokio::test]
    async fn test_collector_start_already_running() {
        let state = create_test_state();
        state.collector.set_running(true);

        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/collector/start")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert!(!json["success"].as_bool().unwrap());
        assert_eq!(json["message"], "Collector is already running");
    }

    #[tokio::test]
    async fn test_collector_stop_not_running() {
        let state = create_test_state();
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/collector/stop")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert!(!json["success"].as_bool().unwrap());
        assert_eq!(json["message"], "Collector is not running");
    }

    #[tokio::test]
    async fn test_update_config_with_devices() {
        let state = create_test_state();
        let app = router().with_state(state);

        let request_body = serde_json::json!({
            "devices": [
                {
                    "address": "AA:BB:CC:DD:EE:01",
                    "alias": "Device 1",
                    "poll_interval": 60
                },
                {
                    "address": "AA:BB:CC:DD:EE:02",
                    "alias": "Device 2",
                    "poll_interval": 120
                }
            ]
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/config")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        let devices = json["devices"].as_array().unwrap();
        assert_eq!(devices.len(), 2);
    }

    #[test]
    fn test_status_response_serialization() {
        let status = StatusResponse {
            version: "1.0.0",
            timestamp: time::OffsetDateTime::now_utc(),
            collector: CollectorStatus {
                running: true,
                started_at: Some(time::OffsetDateTime::now_utc()),
                uptime_seconds: Some(3600),
            },
            devices: vec![],
        };

        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("1.0.0"));
        assert!(json.contains("3600"));
    }

    #[test]
    fn test_collector_action_response_serialization() {
        let response = CollectorActionResponse {
            success: true,
            message: "Test message".to_string(),
            running: true,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("true"));
        assert!(json.contains("Test message"));
    }

    #[test]
    fn test_config_response_serialization() {
        let config = ConfigResponse {
            server: ServerConfigResponse {
                bind: "0.0.0.0:8080".to_string(),
            },
            devices: vec![DeviceConfigResponse {
                address: "AA:BB:CC:DD:EE:FF".to_string(),
                alias: Some("Test".to_string()),
                poll_interval: 60,
            }],
        };

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("0.0.0.0:8080"));
        assert!(json.contains("AA:BB:CC:DD:EE:FF"));
    }

    #[test]
    fn test_device_config_response_deserialization() {
        let json = r#"{"address": "TEST", "alias": "My Device", "poll_interval": 180}"#;
        let config: DeviceConfigResponse = serde_json::from_str(json).unwrap();

        assert_eq!(config.address, "TEST");
        assert_eq!(config.alias, Some("My Device".to_string()));
        assert_eq!(config.poll_interval, 180);
    }

    #[test]
    fn test_device_config_response_default_poll_interval() {
        let json = r#"{"address": "TEST"}"#;
        let config: DeviceConfigResponse = serde_json::from_str(json).unwrap();

        assert_eq!(config.address, "TEST");
        assert_eq!(config.poll_interval, 60); // Default
    }

    #[test]
    fn test_add_device_request_deserialization() {
        let json = r#"{"address": "TEST-ADDR", "alias": "Kitchen", "poll_interval": 90}"#;
        let request: AddDeviceRequest = serde_json::from_str(json).unwrap();

        assert_eq!(request.address, "TEST-ADDR");
        assert_eq!(request.alias, Some("Kitchen".to_string()));
        assert_eq!(request.poll_interval, 90);
    }

    #[test]
    fn test_update_device_request_deserialization() {
        let json = r#"{"alias": "New Name", "poll_interval": 300}"#;
        let request: UpdateDeviceRequest = serde_json::from_str(json).unwrap();

        assert_eq!(request.alias, Some("New Name".to_string()));
        assert_eq!(request.poll_interval, Some(300));
    }

    #[test]
    fn test_update_config_request_deserialization() {
        let json = r#"{"devices": [{"address": "DEV1"}]}"#;
        let request: UpdateConfigRequest = serde_json::from_str(json).unwrap();

        assert!(request.devices.is_some());
        assert_eq!(request.devices.unwrap().len(), 1);
    }

    // ==================== API Integration Tests ====================
    //
    // These tests verify end-to-end API behavior including error scenarios
    // and data persistence across requests.

    #[tokio::test]
    async fn test_full_device_lifecycle() {
        let state = create_test_state();
        let app = router().with_state(Arc::clone(&state));

        // 1. Add a device
        let add_body = serde_json::json!({
            "address": "AA:BB:CC:DD:EE:FF",
            "alias": "Living Room",
            "poll_interval": 90
        });

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/config/devices")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&add_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);

        // 2. Verify device appears in config
        let app = router().with_state(Arc::clone(&state));
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["devices"].as_array().unwrap().len(), 1);

        // 3. Update the device
        let app = router().with_state(Arc::clone(&state));
        let update_body = serde_json::json!({
            "alias": "Kitchen",
            "poll_interval": 120
        });

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/config/devices/AA:BB:CC:DD:EE:FF")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&update_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["alias"], "Kitchen");
        assert_eq!(json["poll_interval"], 120);

        // 4. Remove the device
        let app = router().with_state(Arc::clone(&state));
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/config/devices/AA:BB:CC:DD:EE:FF")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        // 5. Verify device is gone
        let app = router().with_state(Arc::clone(&state));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(json["devices"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_invalid_json_body() {
        let state = create_test_state();
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/config/devices")
                    .header("content-type", "application/json")
                    .body(Body::from("{ invalid json }"))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Axum returns 422 Unprocessable Entity for invalid JSON
        assert!(response.status().is_client_error());
    }

    #[tokio::test]
    async fn test_missing_required_field() {
        let state = create_test_state();
        let app = router().with_state(state);

        // Missing required "address" field
        let body = serde_json::json!({
            "alias": "Test Device"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/config/devices")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert!(response.status().is_client_error());
    }

    #[tokio::test]
    async fn test_case_insensitive_device_lookup() {
        let state = create_test_state();

        // Add device with uppercase
        {
            let mut config = state.config.write().await;
            config.devices.push(DeviceConfig {
                address: "AA:BB:CC:DD:EE:FF".to_string(),
                alias: Some("Test".to_string()),
                poll_interval: 60,
            });
        }

        let app = router().with_state(Arc::clone(&state));

        // Update with lowercase - should find it
        let update_body = serde_json::json!({
            "alias": "Updated"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/config/devices/aa:bb:cc:dd:ee:ff")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&update_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_data_endpoints_with_stored_data() {
        let state = create_test_state();

        // Insert some data into the store
        {
            let store = state.store.lock().await;
            store
                .upsert_device("test-sensor", Some("Test Sensor"))
                .unwrap();

            let reading = aranet_types::CurrentReading {
                co2: 750,
                temperature: 23.5,
                pressure: 1015.0,
                humidity: 48,
                battery: 90,
                status: aranet_types::Status::Green,
                interval: 60,
                age: 5,
                captured_at: Some(time::OffsetDateTime::now_utc()),
                radon: None,
                radiation_rate: None,
                radiation_total: None,
                radon_avg_24h: None,
                radon_avg_7d: None,
                radon_avg_30d: None,
            };
            store.insert_reading("test-sensor", &reading).unwrap();
        }

        // Test list devices
        let app = router().with_state(Arc::clone(&state));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/devices")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 1);
        assert_eq!(json[0]["id"], "test-sensor");

        // Test get device
        let app = router().with_state(Arc::clone(&state));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/devices/test-sensor")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["name"], "Test Sensor");

        // Test get current reading
        let app = router().with_state(Arc::clone(&state));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/devices/test-sensor/current")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["co2"], 750);
        assert_eq!(json["temperature"], 23.5);
    }

    #[tokio::test]
    async fn test_error_response_format() {
        let state = create_test_state();
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/devices/nonexistent-device")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        // Verify error response has expected structure
        assert!(json.get("error").is_some());
        assert!(json["error"].as_str().unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn test_concurrent_api_requests() {
        let state = create_test_state();

        // Spawn multiple concurrent requests
        let mut handles = Vec::new();

        for _ in 0..10 {
            let state = Arc::clone(&state);
            handles.push(tokio::spawn(async move {
                let app = router().with_state(Arc::clone(&state));

                let response = app
                    .oneshot(
                        Request::builder()
                            .uri("/api/health")
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();

                assert_eq!(response.status(), StatusCode::OK);
            }));
        }

        // All requests should complete successfully
        for handle in handles {
            handle.await.unwrap();
        }
    }

    #[tokio::test]
    async fn test_health_detailed_endpoint() {
        let state = create_test_state();
        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/health/detailed")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        // Check basic fields
        assert!(json["status"].is_string());
        assert!(json["version"].is_string());
        assert!(json["timestamp"].is_string());

        // Check database health
        assert!(json["database"]["ok"].is_boolean());
        assert!(json["database"]["device_count"].is_number());

        // Check collector health
        assert!(json["collector"]["running"].is_boolean());
        assert!(json["collector"]["configured_devices"].is_number());

        // Check platform info
        assert!(json["platform"]["os"].is_string());
        assert!(json["platform"]["arch"].is_string());
    }

    #[tokio::test]
    async fn test_health_detailed_status_degraded_when_collector_stopped() {
        let state = create_test_state();
        // Collector is not running by default

        let app = router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/health/detailed")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        // Should be degraded since collector is not running
        assert_eq!(json["status"], "degraded");
        assert!(!json["collector"]["running"].as_bool().unwrap());
    }
}
