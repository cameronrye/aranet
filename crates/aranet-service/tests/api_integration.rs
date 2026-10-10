//! End-to-end integration tests for the aranet-service HTTP API.
//!
//! These tests spin up the full Axum application with an in-memory database
//! and exercise every REST endpoint.

use std::net::SocketAddr;
use std::sync::Arc;

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
use tempfile::TempDir;
use time::OffsetDateTime;

/// Create a test app with an in-memory store and `config`.
///
/// The config file is `server.toml` and `storage.path` is `data.db`, both in
/// the returned temporary directory, which is deleted when it is dropped: bind
/// it as `_dir`, never as `_`. `let (app, state, _dir)` drops the directory
/// first; that is safe only because the store is in memory and the config
/// file is closed after each save. Includes MockConnectInfo so rate limiting
/// middleware can extract client IP.
fn test_app_with_config(mut config: Config) -> (axum::Router, Arc<AppState>, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    config.storage.path = dir.path().join("data.db");
    let store = Store::open_in_memory().unwrap();
    let security_config = Arc::new(config.security.clone());
    let state = AppState::with_config_path(store, config, dir.path().join("server.toml"));
    let rate_limit_state = Arc::new(RateLimitState::new());
    let router = app(Arc::clone(&state), security_config, rate_limit_state)
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
    (router, state, dir)
}

/// Create a test app with an in-memory store and default config.
fn test_app() -> (axum::Router, Arc<AppState>, TempDir) {
    test_app_with_config(Config::default())
}

/// Create a test app with API key authentication enabled.
fn test_app_with_auth() -> (axum::Router, Arc<AppState>, TempDir) {
    test_app_with_config(Config {
        security: SecurityConfig {
            api_key_enabled: true,
            api_key: Some("test-api-key-that-is-at-least-32-characters-long".to_string()),
            rate_limit_enabled: false,
            ..Default::default()
        },
        ..Default::default()
    })
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
    let device_id = device_id.to_owned();
    let alias = alias.map(str::to_owned);
    state
        .with_store_write(move |store| {
            store.upsert_device(&device_id, alias.as_deref())?;
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
            store.insert_reading(&device_id, &reading)?;
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
    let (app, _, _dir) = test_app();
    let (status, body) = get(&app, "/api/health").await;

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "ok");
    assert!(json["version"].is_string());
    assert!(json["timestamp"].is_string());
}

#[tokio::test]
async fn test_health_detailed_endpoint() {
    let (app, _, _dir) = test_app();
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
    let (app, _, _dir) = test_app();
    let (status, body) = get(&app, "/api/devices").await;

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(json.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_list_devices_with_data() {
    let (app, state, _dir) = test_app();
    insert_test_reading(&state, "Aranet4 12345", Some("Office"), 800).await;

    let (status, body) = get(&app, "/api/devices").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let devices = json.as_array().unwrap();
    assert_eq!(devices.len(), 1);
}

#[tokio::test]
async fn test_get_device() {
    let (app, state, _dir) = test_app();
    insert_test_reading(&state, "Aranet4 12345", Some("Office"), 800).await;

    let (status, body) = get(&app, "/api/devices/Aranet4%2012345").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["id"], "Aranet4 12345");
}

#[tokio::test]
async fn test_get_device_not_found() {
    let (app, _, _dir) = test_app();
    let (status, _) = get(&app, "/api/devices/nonexistent").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_get_current_reading() {
    let (app, state, _dir) = test_app();
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
    let (app, state, _dir) = test_app();
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
    let (app, state, _dir) = test_app();
    insert_test_reading(&state, "Aranet4 AAAAA", Some("Office"), 800).await;
    insert_test_reading(&state, "Aranet4 BBBBB", Some("Kitchen"), 600).await;

    let (status, body) = get(&app, "/api/readings").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    // Response is paginated: { pagination: {...}, data: [...] }
    let data = json["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
    assert_eq!(json["pagination"]["limit"], 1000);
    assert_eq!(json["pagination"]["has_more"], false);
}

#[tokio::test]
async fn test_get_latest_device_readings() {
    let (app, state, _dir) = test_app();
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
    let (app, state, _dir) = test_app();
    insert_test_reading(&state, "Aranet4 12345", Some("Office"), 800).await;

    let (status, body) = get(&app, "/api/devices/Aranet4%2012345/readings").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    // Response is paginated: { pagination: {...}, data: [...] }
    assert!(!json["data"].as_array().unwrap().is_empty());
    assert_eq!(json["pagination"]["limit"], 1000);
    assert_eq!(json["pagination"]["offset"], 0);
    assert_eq!(json["pagination"]["has_more"], false);
}

#[tokio::test]
async fn test_get_device_history_without_current_reading() {
    let (app, state, _dir) = test_app();
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
    assert_eq!(json["pagination"]["limit"], 1000);
    assert_eq!(json["pagination"]["has_more"], false);
}

#[tokio::test]
async fn file_backed_readings_endpoint_sees_writes() {
    // A database file, as in production: reads go through the read-only pool.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("data.db");
    let mut config = Config::default();
    config.storage.path = db_path.clone();
    let store = aranet_store::Store::open(&db_path).unwrap();
    let state = AppState::with_config_path(store, config, dir.path().join("server.toml"));
    let router = aranet_service::api::router().with_state(Arc::clone(&state));

    insert_test_reading(&state, "Aranet4 FILE1", Some("Office"), 812).await;

    let (status, body) = get(&router, "/api/devices/Aranet4%20FILE1/readings").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let data = json["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["co2"], 812);

    // Close every connection before the directory is deleted (Windows).
    drop(router);
    drop(state);
    drop(dir);
}

// ==========================================================================
// Collector control endpoints
// ==========================================================================

#[tokio::test]
async fn test_collector_stop_when_not_running() {
    let (app, _, _dir) = test_app();
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
    let (app, _, _dir) = test_app();
    let (status, body) = get(&app, "/api/config").await;

    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(json["server"].is_object());
    assert!(json["devices"].is_array());
}

#[tokio::test]
async fn test_add_device() {
    let (app, state, _dir) = test_app();
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
    // A file where the config's parent directory should be, so saving fails.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("not-a-dir"), b"").unwrap();
    let config_path = dir.path().join("not-a-dir").join("server.toml");
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
    let (app, state, _dir) = test_app();

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
    let (app, state, _dir) = test_app();

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
    let (app, state, _dir) = test_app();

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
    let (app, state, _dir) = test_app();

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
    let (app, _, _dir) = test_app();
    let (status, _) = delete(&app, "/api/config/devices/nonexistent").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_add_device_rejects_alias_with_newline() {
    let (app, state, dir) = test_app();
    let config_path = dir.path().join("server.toml");
    let file_before = std::fs::read(&config_path).ok();

    // `\n` is JSON's newline escape: the handler receives a real line feed.
    let body = r#"{"address": "Aranet4 12345", "alias": "Office\nevil", "poll_interval": 60}"#;
    let (status, resp) = post(&app, "/api/config/devices", body).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{resp}");
    let json: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(
        json["error"],
        "device.alias: alias cannot contain control characters"
    );

    assert!(state.config.read().await.devices.is_empty());
    assert_eq!(
        std::fs::read(&config_path).ok(),
        file_before,
        "a rejected device must not be saved"
    );
}

#[tokio::test]
async fn test_update_device_rejects_control_characters() {
    let (app, state, dir) = test_app();
    {
        let mut config = state.config.write().await;
        config.devices.push(DeviceConfig {
            address: "Aranet4 12345".to_string(),
            alias: Some("Office".to_string()),
            poll_interval: 60,
        });
    }
    let config_path = dir.path().join("server.toml");
    let file_before = std::fs::read(&config_path).ok();

    // U+202E RIGHT-TO-LEFT OVERRIDE would reverse the rest of a log line.
    for alias in ["Office\u{202e}", "Office\u{0}", "a\r\nb"] {
        let body = serde_json::json!({ "alias": alias }).to_string();
        let (status, resp) = put(&app, "/api/config/devices/Aranet4%2012345", &body).await;

        assert_eq!(status, StatusCode::BAD_REQUEST, "{alias:?}: {resp}");
        let json: serde_json::Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(
            json["error"], "device.alias: alias cannot contain control characters",
            "{alias:?}"
        );
    }

    // A trailing backslash would make InfluxDB 2.x reject every line.
    let backslash = serde_json::json!({ "alias": "Office\\" }).to_string();
    let (status, resp) = put(&app, "/api/config/devices/Aranet4%2012345", &backslash).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{resp}");
    let json: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(
        json["error"],
        "device.alias: alias cannot contain backslashes"
    );

    let too_long = serde_json::json!({ "alias": "a".repeat(200) }).to_string();
    let (status, resp) = put(&app, "/api/config/devices/Aranet4%2012345", &too_long).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{resp}");
    let json: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(
        json["error"],
        "device.alias: alias is too long (200 characters, maximum 64)"
    );

    let config = state.config.read().await;
    assert_eq!(config.devices[0].alias.as_deref(), Some("Office"));
    assert_eq!(
        std::fs::read(&config_path).ok(),
        file_before,
        "a rejected update must not be saved"
    );
}

#[tokio::test]
async fn test_update_config_rejects_control_characters() {
    let (app, state, dir) = test_app();
    {
        let mut config = state.config.write().await;
        config.devices.push(DeviceConfig {
            address: "Aranet4 12345".to_string(),
            alias: Some("Office".to_string()),
            poll_interval: 60,
        });
    }
    let config_path = dir.path().join("server.toml");
    let file_before = std::fs::read(&config_path).ok();

    let body = serde_json::json!({
        "devices": [{"address": "Aranet4\n12345", "alias": "Office\u{0}", "poll_interval": 60}]
    })
    .to_string();
    let (status, resp) = put(&app, "/api/config", &body).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{resp}");
    let json: serde_json::Value = serde_json::from_str(&resp).unwrap();
    let error = json["error"].as_str().unwrap();
    assert!(
        error.contains("devices[0].address: device address cannot contain control characters"),
        "{error}"
    );
    assert!(
        error.contains("devices[0].alias: alias cannot contain control characters"),
        "{error}"
    );

    let config = state.config.read().await;
    assert_eq!(config.devices.len(), 1);
    assert_eq!(config.devices[0].address, "Aranet4 12345");
    assert_eq!(config.devices[0].alias.as_deref(), Some("Office"));
    assert_eq!(
        std::fs::read(&config_path).ok(),
        file_before,
        "a rejected config must not be saved"
    );
}

// ==========================================================================
// Status endpoint
// ==========================================================================

#[tokio::test]
async fn test_get_status() {
    let (app, _, _dir) = test_app();
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
    let (app, _, _dir) = test_app();
    let (status, body) = get(&app, "/").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Aranet Dashboard"));
    assert!(body.contains("<canvas"));
}

#[tokio::test]
async fn test_dashboard_route() {
    let (app, _, _dir) = test_app();
    let (status, body) = get(&app, "/dashboard").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Aranet Dashboard"));
}

#[tokio::test]
async fn test_dashboard_root_is_public_when_auth_enabled() {
    let (app, _, _dir) = test_app_with_auth();
    let (status, body) = get(&app, "/").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Aranet Dashboard"));
}

#[tokio::test]
async fn test_dashboard_route_is_public_when_auth_enabled() {
    let (app, _, _dir) = test_app_with_auth();
    let (status, body) = get(&app, "/dashboard").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Aranet Dashboard"));
}

#[tokio::test]
async fn test_dashboard_history_uses_cached_history_endpoint() {
    let (app, _, _dir) = test_app();
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
    let (app, _, _dir) = test_app_with_auth();
    let (status, _) = get(&app, "/api/health").await;

    // Health endpoint should work without auth
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_api_requires_auth_when_enabled() {
    let (app, _, _dir) = test_app_with_auth();
    let (status, _) = get(&app, "/api/devices").await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_api_with_valid_key() {
    let (app, _, _dir) = test_app_with_auth();
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
    let (app, _, _dir) = test_app_with_auth();
    let (status, _) = get_with_key(&app, "/api/devices", "wrong-key").await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_api_auth_query_param_only_for_ws() {
    let (app, _, _dir) = test_app_with_auth();
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
    let (app, state, _dir) = test_app();

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
    let (app, _, _dir) = test_app();
    let (status, body) = get(&app, "/metrics").await;

    // When prometheus is disabled, should return 404 or empty
    // The actual behavior depends on the handler
    assert!(status == StatusCode::NOT_FOUND || body.is_empty() || body.contains("disabled"));
}

#[tokio::test]
async fn test_prometheus_metrics_enabled() {
    let mut config = Config::default();
    config.prometheus.enabled = true;
    let (router, state, _dir) = test_app_with_config(config);

    // Insert a reading so metrics have data
    insert_test_reading(&state, "Aranet4 AAAAA", Some("Office"), 800).await;

    let (status, body) = get(&router, "/metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("aranet_collector_running"));
}

// ==========================================================================
// Config file persistence
// ==========================================================================

/// An app over the config file at `path`, set up the way `run()` sets it up: the
/// file is loaded (the defaults if it is missing), then `--bind 127.0.0.1:9999`
/// and `--database <dir>/override.db` change the running config only.
fn app_for_config_file(path: &std::path::Path) -> (axum::Router, Arc<AppState>) {
    let mut config = if path.exists() {
        Config::load(path).unwrap()
    } else {
        Config::default()
    };
    config.server.bind = "127.0.0.1:9999".to_string();
    config.storage.path = path.with_file_name("override.db");
    let store = Store::open_in_memory().unwrap();
    let state = AppState::with_config_path(store, config.clone(), path.to_path_buf());
    let security_config = Arc::new(config.security.clone());
    let rate_limit_state = Arc::new(RateLimitState::new());
    let router = app(Arc::clone(&state), security_config, rate_limit_state)
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
    (router, state)
}

const OFFICE_DEVICE: &str =
    r#"{"address": "Aranet4 12345", "alias": "Office", "poll_interval": 60}"#;

/// `OFFICE_DEVICE` as a new `[[devices]]` entry at the end of a file.
const OFFICE_DEVICE_TOML: &str =
    "\n[[devices]]\naddress = \"Aranet4 12345\"\nalias = \"Office\"\npoll_interval = 60\n";

#[tokio::test]
async fn test_add_device_does_not_persist_runtime_overrides_or_drop_comments() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("server.toml");
    let original = "# Aranet service\n[server]\n# Only this machine\nbind = \"127.0.0.1:8080\" # default port\n";
    std::fs::write(&config_path, original).unwrap();
    let (app, _state) = app_for_config_file(&config_path);

    let (status, body) = post(&app, "/api/config/devices", OFFICE_DEVICE).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let saved = std::fs::read_to_string(&config_path).unwrap();
    for leaked in ["127.0.0.1:9999", "override.db", "[storage]", "[security]"] {
        assert!(!saved.contains(leaked), "{leaked:?} written to:\n{saved}");
    }
    assert_eq!(saved, format!("{original}{OFFICE_DEVICE_TOML}"));
}

#[tokio::test]
async fn test_update_config_does_not_persist_runtime_overrides_or_drop_comments() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("server.toml");
    let original = "# Aranet service\n[server]\n# Only this machine\nbind = \"127.0.0.1:8080\" # default port\n";
    std::fs::write(&config_path, original).unwrap();
    let (app, _state) = app_for_config_file(&config_path);

    let devices = format!(r#"{{"devices": [{OFFICE_DEVICE}]}}"#);
    let (status, body) = put(&app, "/api/config", &devices).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let saved = std::fs::read_to_string(&config_path).unwrap();
    for leaked in ["127.0.0.1:9999", "override.db", "[storage]", "[security]"] {
        assert!(!saved.contains(leaked), "{leaked:?} written to:\n{saved}");
    }
    assert_eq!(saved, format!("{original}{OFFICE_DEVICE_TOML}"));
}

#[tokio::test]
async fn test_update_device_keeps_comment_on_its_entry() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("server.toml");
    let original = r#"[server]
bind = "127.0.0.1:8080"

# kitchen
[[devices]]
address = "Aranet4 AAAAA"
alias = "Kitchen" # by the window

[[devices]]
address = "Aranet4 BBBBB"
"#;
    std::fs::write(&config_path, original).unwrap();
    let (app, _state) = app_for_config_file(&config_path);

    let (status, body) = put(
        &app,
        "/api/config/devices/Aranet4%20AAAAA",
        r#"{"alias": "Kitchen sensor"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert_eq!(
        std::fs::read_to_string(&config_path).unwrap(),
        original.replace("\"Kitchen\"", "\"Kitchen sensor\"")
    );
}

#[tokio::test]
async fn test_add_device_refuses_to_overwrite_unparseable_config() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("server.toml");
    std::fs::write(&config_path, "[server]\nbind = \"127.0.0.1:8080\"\n").unwrap();
    let (app, state) = app_for_config_file(&config_path);
    // Someone edits the file while the service runs and leaves a syntax error.
    let broken = "[server\nbind = \"127.0.0.1:8080\"\n";
    std::fs::write(&config_path, broken).unwrap();

    let (status, body) = post(&app, "/api/config/devices", OFFICE_DEVICE).await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    // The response names the file but quotes none of it; the parse error,
    // with the offending line, goes only to the service log.
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let message = json["error"].as_str().unwrap_or_default();
    let named = format!("config file {} is not valid TOML", config_path.display());
    assert!(message.contains(&named), "{body}");
    assert!(!message.contains("[server"), "{body}");
    assert_eq!(std::fs::read_to_string(&config_path).unwrap(), broken);
    assert!(state.config.read().await.devices.is_empty());
}

#[tokio::test]
async fn test_add_device_to_missing_file_writes_only_devices() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("aranet").join("server.toml");
    let (app, _state) = app_for_config_file(&config_path);

    let (status, body) = post(&app, "/api/config/devices", OFFICE_DEVICE).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let saved: toml::Table = std::fs::read_to_string(&config_path)
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(saved.keys().collect::<Vec<_>>(), ["devices"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode =
            |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&config_path), 0o600);
        assert_eq!(mode(config_path.parent().unwrap()), 0o700);
    }
}

/// The shipped `docker/server.toml` with LF line endings. A Windows checkout may
/// have converted it to CRLF (the repository has no `.gitattributes`), and the
/// tests compare it with text written with LF.
fn shipped_server_toml() -> String {
    include_str!("../../../docker/server.toml").replace("\r\n", "\n")
}

#[tokio::test]
async fn test_device_changes_preserve_shipped_docker_config() {
    let shipped = shipped_server_toml();
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("server.toml");
    std::fs::write(&config_path, &shipped).unwrap();
    let (app, _state) = app_for_config_file(&config_path);

    let (status, body) = post(&app, "/api/config/devices", OFFICE_DEVICE).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    // The commented-out InfluxDB settings at the end of the file stay under
    // [influxdb]; the new device goes after them.
    let added = std::fs::read_to_string(&config_path).unwrap();
    let influxdb = &added[added.find("\n[influxdb]\n").expect("[influxdb] kept")..];
    let device = influxdb.find("\n[[devices]]\n").unwrap_or(influxdb.len());
    assert!(influxdb[..device].contains("# url ="), "{added}");
    assert!(added.ends_with(OFFICE_DEVICE_TOML), "{added}");

    let (status, body) = put(
        &app,
        "/api/config/devices/Aranet4%2012345",
        r#"{"alias": "Kitchen"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let updated = Config::load(&config_path).unwrap();
    assert_eq!(updated.devices[0].alias.as_deref(), Some("Kitchen"));

    let (status, body) = delete(&app, "/api/config/devices/Aranet4%2012345").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    assert_eq!(std::fs::read_to_string(&config_path).unwrap(), shipped);
}

#[tokio::test]
async fn test_removing_first_of_two_devices_keeps_shipped_comments() {
    let shipped = shipped_server_toml();
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("server.toml");
    std::fs::write(&config_path, &shipped).unwrap();
    let (app, _state) = app_for_config_file(&config_path);
    for device in [
        r#"{"address": "Aranet4 AAAAA"}"#,
        r#"{"address": "Aranet4 BBBBB"}"#,
    ] {
        let (status, body) = post(&app, "/api/config/devices", device).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    // The commented-out settings at the end of [influxdb] are now above the first
    // device, and stay when it goes.
    let (status, body) = delete(&app, "/api/config/devices/Aranet4%20AAAAA").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(
        std::fs::read_to_string(&config_path).unwrap(),
        format!("{shipped}\n[[devices]]\naddress = \"Aranet4 BBBBB\"\npoll_interval = 60\n")
    );

    let (status, body) = delete(&app, "/api/config/devices/Aranet4%20BBBBB").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(std::fs::read_to_string(&config_path).unwrap(), shipped);
}

// ==========================================================================
// Startup
// ==========================================================================

/// An earlier version didn't expand `~` in `storage.path`, so it kept its
/// database under a directory named `~` in its working directory. The service
/// now uses another database, and every start must say so for as long as the
/// old file is there.
///
/// The warning depends on the working directory, so this runs the binary in a
/// directory of its own. Unix only: `dirs` takes the home directory from
/// `HOME` there, which keeps `~` inside the `TempDir`.
#[cfg(unix)]
#[test]
fn test_startup_warns_about_database_in_tilde_directory() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let cwd = dir.path().join("cwd");
    let legacy = cwd.join("~").join("old").join("data.db");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&legacy, b"").unwrap();
    // The host name stops the service once the configuration is loaded: after
    // the warning, and before it listens or opens a database. It is given
    // twice, so that neither a lost file nor a lost override starts a server.
    let config = dir.path().join("server.toml");
    std::fs::write(
        &config,
        "[server]\nbind = \"localhost:8080\"\n\n[storage]\npath = \"~/old/data.db\"\n",
    )
    .unwrap();

    let start = || {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_aranet-service"))
            .arg("--config")
            .arg(&config)
            .args(["--bind", "localhost:8080", "--no-collector", "run"])
            .current_dir(&cwd)
            .env("HOME", &home)
            .env("NO_COLOR", "1")
            .env_remove("RUST_LOG")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("failed to start aranet-service");
        let output = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.contains("invalid bind address 'localhost:8080'"),
            "{output}"
        );
        output
    };

    let output = start();
    let warning = format!(
        "Found a database at ./~/old/data.db, created by an earlier version that did not \
         expand '~'; the service now uses {}.",
        home.join("old").join("data.db").display()
    );
    assert!(output.contains(&warning), "{output}");

    // Moving or removing the old file is what ends the warning.
    std::fs::remove_file(&legacy).unwrap();
    let output = start();
    assert!(!output.contains("Found a database"), "{output}");
    assert!(!home.join("old").exists(), "a database was opened");
}
