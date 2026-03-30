//! End-to-end integration tests for the aranet-service HTTP API.
//!
//! These tests spin up the full Axum application with an in-memory database
//! and exercise every REST endpoint.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use aranet_service::config::{Config, DeviceConfig, SecurityConfig};
use aranet_service::middleware::RateLimitState;
use aranet_service::state::AppState;
use aranet_service::{ReadingEvent, app};
use aranet_store::Store;
use aranet_types::{CurrentReading, HistoryRecord, Status};
use time::OffsetDateTime;

fn test_config_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "aranet-service-api-test-{}-{}.toml",
        std::process::id(),
        nanos
    ))
}

/// Create a test app with an in-memory store and default config.
/// Includes MockConnectInfo so rate limiting middleware can extract client IP.
fn test_app() -> (axum::Router, Arc<AppState>) {
    let store = Store::open_in_memory().unwrap();
    let config = Config::default();
    let state = AppState::with_config_path(store, config.clone(), test_config_path());
    let security_config = Arc::new(config.security.clone());
    let rate_limit_state = Arc::new(RateLimitState::new());
    let router = app(Arc::clone(&state), security_config, rate_limit_state)
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
    (router, state)
}

/// Create a test app with API key authentication enabled.
fn test_app_with_auth() -> (axum::Router, Arc<AppState>) {
    let store = Store::open_in_memory().unwrap();
    let config = Config {
        security: SecurityConfig {
            api_key_enabled: true,
            api_key: Some("test-api-key-that-is-at-least-32-characters-long".to_string()),
            rate_limit_enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let state = AppState::with_config_path(store, config.clone(), test_config_path());
    let security_config = Arc::new(config.security.clone());
    let rate_limit_state = Arc::new(RateLimitState::new());
    let router = app(Arc::clone(&state), security_config, rate_limit_state)
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
    (router, state)
}

/// Helper to make GET requests and return (status, body_string).
async fn get(app: &axum::Router, path: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();

    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    (status, body_str)
}

/// Helper to make POST requests.
async fn post(app: &axum::Router, path: &str, body: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    (status, body_str)
}

/// Helper to make DELETE requests.
async fn delete(app: &axum::Router, path: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    (status, body_str)
}

/// Helper to make PUT requests.
async fn put(app: &axum::Router, path: &str, body: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(path)
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    (status, body_str)
}

/// Helper to make GET requests with API key.
async fn get_with_key(app: &axum::Router, path: &str, key: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header("X-API-Key", key)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    (status, body_str)
}

/// Insert a test reading into the store.
async fn insert_test_reading(state: &AppState, device_id: &str, alias: Option<&str>, co2: u16) {
    state
        .with_store_write(|store| {
            store.upsert_device(device_id, alias)?;
            let reading = CurrentReading {
                co2,
                temperature: 22.5,
                pressure: 1013.25,
                humidity: 45,
                battery: 85,
                status: Status::Green,
                interval: 60,
                age: 10,
                ..Default::default()
            };
            store.insert_reading(device_id, &reading)?;
            Ok(())
        })
        .await
        .unwrap();
}

// ==========================================================================
// Health endpoints
// ==========================================================================

#[tokio::test]
async fn test_health_endpoint() {
    let (app, _) = test_app();
    let (status, body) = get(&app, "/api/health").await;

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "ok");
    assert!(json["version"].is_string());
    assert!(json["timestamp"].is_string());
}

#[tokio::test]
async fn test_health_detailed_endpoint() {
    let (app, _) = test_app();
    let (status, body) = get(&app, "/api/health/detailed").await;

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(json["database"]["ok"].as_bool().unwrap());
    assert!(json["platform"]["os"].is_string());
    assert!(json["platform"]["arch"].is_string());
}

// ==========================================================================
// Device endpoints
// ==========================================================================

#[tokio::test]
async fn test_list_devices_empty() {
    let (app, _) = test_app();
    let (status, body) = get(&app, "/api/devices").await;

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(json.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_list_devices_with_data() {
    let (app, state) = test_app();
    insert_test_reading(&state, "Aranet4 12345", Some("Office"), 800).await;

    let (status, body) = get(&app, "/api/devices").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let devices = json.as_array().unwrap();
    assert_eq!(devices.len(), 1);
}

#[tokio::test]
async fn test_get_device() {
    let (app, state) = test_app();
    insert_test_reading(&state, "Aranet4 12345", Some("Office"), 800).await;

    let (status, body) = get(&app, "/api/devices/Aranet4%2012345").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["id"], "Aranet4 12345");
}

#[tokio::test]
async fn test_get_device_not_found() {
    let (app, _) = test_app();
    let (status, _) = get(&app, "/api/devices/nonexistent").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_get_current_reading() {
    let (app, state) = test_app();
    insert_test_reading(&state, "Aranet4 12345", Some("Office"), 800).await;

    let (status, body) = get(&app, "/api/devices/Aranet4%2012345/current").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    // Response uses #[serde(flatten)] - reading fields are at top level
    assert_eq!(json["co2"], 800);
    assert!(json["age_seconds"].is_number());
    assert!(json["stale"].is_boolean());
}

#[tokio::test]
async fn test_get_current_reading_not_found() {
    let (app, state) = test_app();
    // Device exists but no readings
    state
        .with_store_write(|store| store.upsert_device("empty-device", None).map(|_| ()))
        .await
        .unwrap();

    let (status, _) = get(&app, "/api/devices/empty-device/current").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ==========================================================================
// Readings endpoints
// ==========================================================================

#[tokio::test]
async fn test_get_all_readings() {
    let (app, state) = test_app();
    insert_test_reading(&state, "Aranet4 AAAAA", Some("Office"), 800).await;
    insert_test_reading(&state, "Aranet4 BBBBB", Some("Kitchen"), 600).await;

    let (status, body) = get(&app, "/api/readings").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    // Response is paginated: { pagination: {...}, data: [...] }
    let data = json["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
}

#[tokio::test]
async fn test_get_latest_device_readings() {
    let (app, state) = test_app();
    insert_test_reading(&state, "Aranet4 AAAAA", Some("Office"), 800).await;
    insert_test_reading(&state, "Aranet4 BBBBB", Some("Kitchen"), 600).await;

    let (status, body) = get(&app, "/api/devices/current").await;
    assert_eq!(status, StatusCode::OK);

    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let data = json.as_array().unwrap();
    assert_eq!(data.len(), 2);
    assert_eq!(data[0]["reading"]["device_id"], data[0]["device_id"]);
    assert!(data[0]["age_seconds"].is_number());
    assert!(data[0]["stale"].is_boolean());
}

#[tokio::test]
async fn test_get_device_readings() {
    let (app, state) = test_app();
    insert_test_reading(&state, "Aranet4 12345", Some("Office"), 800).await;

    let (status, body) = get(&app, "/api/devices/Aranet4%2012345/readings").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    // Response is paginated: { pagination: {...}, data: [...] }
    assert!(!json["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_get_device_history_without_current_reading() {
    let (app, state) = test_app();
    state
        .with_store_write(|store| {
            store.upsert_device("Aranet4 HIST1", Some("Archive"))?;
            let records = vec![HistoryRecord {
                timestamp: OffsetDateTime::now_utc() - time::Duration::hours(1),
                co2: 900,
                temperature: 21.5,
                pressure: 1012.0,
                humidity: 44,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            }];
            store.insert_history("Aranet4 HIST1", &records)?;
            Ok(())
        })
        .await
        .unwrap();

    let (status, body) = get(&app, "/api/devices/Aranet4%20HIST1/history").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["data"].as_array().unwrap().len(), 1);
}

// ==========================================================================
// Collector control endpoints
// ==========================================================================

#[tokio::test]
async fn test_collector_stop_when_not_running() {
    let (app, _) = test_app();
    let (status, body) = post(&app, "/api/collector/stop", "").await;

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["running"], false);
}

// ==========================================================================
// Configuration endpoints
// ==========================================================================

#[tokio::test]
async fn test_get_config() {
    let (app, _) = test_app();
    let (status, body) = get(&app, "/api/config").await;

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(json["server"].is_object());
    assert!(json["devices"].is_array());
}

#[tokio::test]
async fn test_add_device() {
    let (app, state) = test_app();
    let body = r#"{"address": "Aranet4 12345", "alias": "Office", "poll_interval": 60}"#;
    let (status, resp) = post(&app, "/api/config/devices", body).await;

    assert_eq!(status, StatusCode::CREATED);
    let json: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(json["address"], "Aranet4 12345");

    // Verify it's in config
    let config = state.config.read().await;
    assert_eq!(config.devices.len(), 1);
    assert_eq!(config.devices[0].address, "Aranet4 12345");
}

#[tokio::test]
async fn test_add_device_fails_when_config_cannot_be_saved() {
    let store = Store::open_in_memory().unwrap();
    let config = Config::default();
    let bad_parent = tempfile::NamedTempFile::new().unwrap();
    let config_path = bad_parent.path().join("server.toml");
    let state = AppState::with_config_path(store, config.clone(), config_path);
    let security_config = Arc::new(config.security.clone());
    let rate_limit_state = Arc::new(RateLimitState::new());
    let app = app(Arc::clone(&state), security_config, rate_limit_state)
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));

    let body = r#"{"address": "Aranet4 FAIL1", "alias": "Office", "poll_interval": 60}"#;
    let (status, resp) = post(&app, "/api/config/devices", body).await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(resp.contains("Failed to save configuration"));

    let config = state.config.read().await;
    assert!(
        config.devices.is_empty(),
        "in-memory config should roll back on save failure"
    );
}

#[tokio::test]
async fn test_add_duplicate_device() {
    let (app, state) = test_app();

    // Add initial config
    {
        let mut config = state.config.write().await;
        config.devices.push(DeviceConfig {
            address: "Aranet4 12345".to_string(),
            alias: Some("Office".to_string()),
            poll_interval: 60,
        });
    }

    let body = r#"{"address": "Aranet4 12345", "alias": "Kitchen", "poll_interval": 60}"#;
    let (status, _) = post(&app, "/api/config/devices", body).await;

    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn test_update_device_preserves_alias_when_omitted() {
    let (app, state) = test_app();

    {
        let mut config = state.config.write().await;
        config.devices.push(DeviceConfig {
            address: "Aranet4 12345".to_string(),
            alias: Some("Office".to_string()),
            poll_interval: 60,
        });
    }

    let (status, body) = put(
        &app,
        "/api/config/devices/Aranet4%2012345",
        r#"{"poll_interval": 300}"#,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["alias"], "Office");
    assert_eq!(json["poll_interval"], 300);

    let config = state.config.read().await;
    assert_eq!(config.devices[0].alias.as_deref(), Some("Office"));
    assert_eq!(config.devices[0].poll_interval, 300);
}

#[tokio::test]
async fn test_update_device_can_clear_alias_with_null() {
    let (app, state) = test_app();

    {
        let mut config = state.config.write().await;
        config.devices.push(DeviceConfig {
            address: "Aranet4 12345".to_string(),
            alias: Some("Office".to_string()),
            poll_interval: 60,
        });
    }

    let (status, body) = put(
        &app,
        "/api/config/devices/Aranet4%2012345",
        r#"{"alias": null}"#,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(json["alias"].is_null());

    let config = state.config.read().await;
    assert!(config.devices[0].alias.is_none());
}

#[tokio::test]
async fn test_remove_device() {
    let (app, state) = test_app();

    // Add a device first
    {
        let mut config = state.config.write().await;
        config.devices.push(DeviceConfig {
            address: "Aranet4 12345".to_string(),
            alias: Some("Office".to_string()),
            poll_interval: 60,
        });
    }

    let (status, _) = delete(&app, "/api/config/devices/Aranet4%2012345").await;
    // Remove returns 204 No Content on success
    assert!(status == StatusCode::OK || status == StatusCode::NO_CONTENT);

    let config = state.config.read().await;
    assert!(config.devices.is_empty());
}

#[tokio::test]
async fn test_remove_device_not_found() {
    let (app, _) = test_app();
    let (status, _) = delete(&app, "/api/config/devices/nonexistent").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ==========================================================================
// Status endpoint
// ==========================================================================

#[tokio::test]
async fn test_get_status() {
    let (app, _) = test_app();
    let (status, body) = get(&app, "/api/status").await;

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(json["collector"]["running"].is_boolean());
    assert!(json["devices"].is_array());
}

// ==========================================================================
// Dashboard
// ==========================================================================

#[tokio::test]
async fn test_dashboard_root() {
    let (app, _) = test_app();
    let (status, body) = get(&app, "/").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Aranet Dashboard"));
    assert!(body.contains("<canvas"));
}

#[tokio::test]
async fn test_dashboard_route() {
    let (app, _) = test_app();
    let (status, body) = get(&app, "/dashboard").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Aranet Dashboard"));
}

#[tokio::test]
async fn test_dashboard_root_is_public_when_auth_enabled() {
    let (app, _) = test_app_with_auth();
    let (status, body) = get(&app, "/").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Aranet Dashboard"));
}

#[tokio::test]
async fn test_dashboard_route_is_public_when_auth_enabled() {
    let (app, _) = test_app_with_auth();
    let (status, body) = get(&app, "/dashboard").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Aranet Dashboard"));
}

#[tokio::test]
async fn test_dashboard_history_uses_cached_history_endpoint() {
    let (app, _) = test_app();
    let (status, body) = get(&app, "/dashboard").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("/api/devices/${encodeURIComponent(device)}/history"));
    assert!(!body.contains("/api/devices/${encodeURIComponent(device)}/readings"));
    assert!(body.contains("r.timestamp || r.captured_at || r.synced_at"));
    assert!(body.contains("fetchJson('/api/config')"));
    assert!(body.contains("fetchJson('/api/devices')"));
}

// ==========================================================================
// Authentication
// ==========================================================================

#[tokio::test]
async fn test_health_no_auth_required() {
    let (app, _) = test_app_with_auth();
    let (status, _) = get(&app, "/api/health").await;

    // Health endpoint should work without auth
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_api_requires_auth_when_enabled() {
    let (app, _) = test_app_with_auth();
    let (status, _) = get(&app, "/api/devices").await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_api_with_valid_key() {
    let (app, _) = test_app_with_auth();
    let (status, _) = get_with_key(
        &app,
        "/api/devices",
        "test-api-key-that-is-at-least-32-characters-long",
    )
    .await;

    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_api_with_invalid_key() {
    let (app, _) = test_app_with_auth();
    let (status, _) = get_with_key(&app, "/api/devices", "wrong-key").await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_api_auth_query_param_only_for_ws() {
    let (app, _) = test_app_with_auth();
    // Token query parameter is only allowed for WebSocket routes, not REST
    let (status, _) = get(
        &app,
        "/api/devices?token=test-api-key-that-is-at-least-32-characters-long",
    )
    .await;

    // Should be rejected for REST endpoints
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ==========================================================================
// Broadcast channel integration
// ==========================================================================

#[tokio::test]
async fn test_broadcast_reading_updates_api() {
    let (app, state) = test_app();

    // Insert a device first
    state
        .with_store_write(|store| store.upsert_device("test-device", Some("Test")).map(|_| ()))
        .await
        .unwrap();

    // Simulate a reading being broadcast (as the collector would do)
    let reading = aranet_store::StoredReading {
        id: 1,
        device_id: "test-device".to_string(),
        co2: 950,
        temperature: 23.0,
        humidity: 50,
        pressure: 1015.0,
        battery: 90,
        status: Status::Green,
        radon: None,
        radiation_rate: None,
        radiation_total: None,
        radon_avg_24h: None,
        radon_avg_7d: None,
        radon_avg_30d: None,
        captured_at: time::OffsetDateTime::now_utc(),
    };

    // Insert via store and broadcast
    state
        .with_store_write(|store| {
            let cr = CurrentReading {
                co2: 950,
                temperature: 23.0,
                humidity: 50,
                pressure: 1015.0,
                battery: 90,
                status: Status::Green,
                interval: 60,
                age: 5,
                ..Default::default()
            };
            store.insert_reading("test-device", &cr)
        })
        .await
        .unwrap();

    let _ = state.readings_tx.send(ReadingEvent {
        device_id: "test-device".to_string(),
        reading,
    });

    // Now fetch the reading via API
    let (status, body) = get(&app, "/api/devices/test-device/current").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["co2"], 950);
}

// ==========================================================================
// Prometheus metrics
// ==========================================================================

#[tokio::test]
async fn test_prometheus_metrics_disabled_by_default() {
    let (app, _) = test_app();
    let (status, body) = get(&app, "/metrics").await;

    // When prometheus is disabled, should return 404 or empty
    // The actual behavior depends on the handler
    assert!(status == StatusCode::NOT_FOUND || body.is_empty() || body.contains("disabled"));
}

#[tokio::test]
async fn test_prometheus_metrics_enabled() {
    let store = Store::open_in_memory().unwrap();
    let mut config = Config::default();
    config.prometheus.enabled = true;
    let state = AppState::with_config_path(store, config.clone(), test_config_path());
    let security_config = Arc::new(config.security.clone());
    let rate_limit_state = Arc::new(RateLimitState::new());
    let router = app(Arc::clone(&state), security_config, rate_limit_state)
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));

    // Insert a reading so metrics have data
    insert_test_reading(&state, "Aranet4 AAAAA", Some("Office"), 800).await;

    let (status, body) = get(&router, "/metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("aranet_collector_running"));
}
