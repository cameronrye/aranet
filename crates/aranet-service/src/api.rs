//! REST API endpoints.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::state::AppState;

/// Create the API router.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/health", get(health))
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
async fn list_devices(State(state): State<Arc<AppState>>) -> Result<Json<Vec<DeviceResponse>>, AppError> {
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
        .ok_or(AppError::NotFound(format!("No readings for device: {}", id)))?;
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

/// Get readings for a device.
async fn get_readings(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<ReadingsQuery>,
) -> Result<Json<Vec<aranet_store::StoredReading>>, AppError> {
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
    if let Some(limit) = params.limit {
        query = query.limit(limit);
    }
    if let Some(offset) = params.offset {
        query = query.offset(offset);
    }

    let store = state.store.lock().await;
    let readings = store.query_readings(&query)?;
    Ok(Json(readings))
}

/// Get history for a device.
async fn get_history(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<ReadingsQuery>,
) -> Result<Json<Vec<aranet_store::StoredHistoryRecord>>, AppError> {
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
    if let Some(limit) = params.limit {
        query = query.limit(limit);
    }

    let store = state.store.lock().await;
    let history = store.query_history(&query)?;
    Ok(Json(history))
}

/// Get all readings across devices.
async fn get_all_readings(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ReadingsQuery>,
) -> Result<Json<Vec<aranet_store::StoredReading>>, AppError> {
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
    if let Some(limit) = params.limit {
        query = query.limit(limit);
    }
    if let Some(offset) = params.offset {
        query = query.offset(offset);
    }

    let store = state.store.lock().await;
    let readings = store.query_readings(&query)?;
    Ok(Json(readings))
}

/// Application error type.
#[derive(Debug)]
pub enum AppError {
    NotFound(String),
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
            .oneshot(Request::builder().uri("/api/health").body(Body::empty()).unwrap())
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
            .oneshot(Request::builder().uri("/api/devices").body(Body::empty()).unwrap())
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

        assert!(json.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_get_all_readings_empty() {
        let state = create_test_state();
        let app = router().with_state(state);

        let response = app
            .oneshot(Request::builder().uri("/api/readings").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert!(json.as_array().unwrap().is_empty());
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

        assert!(json.as_array().unwrap().is_empty());
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
}

