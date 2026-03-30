//! HTTP client for the aranet-service REST API.
//!
//! This module provides a client for interacting with the aranet-service
//! background service. It allows checking service status, controlling the
//! collector, and managing monitored devices.
//!
//! # Example
//!
//! ```no_run
//! use aranet_core::service_client::ServiceClient;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let client = ServiceClient::new("http://localhost:8080")?;
//!
//! // Check if service is running
//! let status = client.status().await?;
//! println!("Collector running: {}", status.collector.running);
//!
//! // Start the collector
//! client.start_collector().await?;
//!
//! Ok(())
//! # }
//! ```

use reqwest::{Client, Method, RequestBuilder};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// HTTP client for the aranet-service API.
#[derive(Debug, Clone)]
pub struct ServiceClient {
    client: Client,
    base_url: String,
    api_key: Option<String>,
}

/// Error type for service client operations.
#[derive(Debug, thiserror::Error)]
pub enum ServiceClientError {
    /// The service is not reachable.
    #[error("Service not reachable at {url}: {source}")]
    NotReachable {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    /// HTTP request failed.
    #[error("HTTP request failed: {0}")]
    Request(#[from] reqwest::Error),

    /// Invalid URL.
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    /// API returned an error response.
    #[error("API error: {message}")]
    ApiError { status: u16, message: String },
}

/// Result type for service client operations.
pub type Result<T> = std::result::Result<T, ServiceClientError>;

const REJECTED_ACTION_STATUS: u16 = 409;

// ==========================================================================
// Response Types
// ==========================================================================

/// Service status response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStatus {
    /// Service version.
    pub version: String,
    /// Current timestamp.
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
    /// Collector status.
    pub collector: CollectorStatus,
    /// Per-device collection statistics.
    pub devices: Vec<DeviceCollectionStats>,
}

/// Collector status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorStatus {
    /// Whether the collector is running.
    pub running: bool,
    /// When the collector was started (if running).
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    /// How long the collector has been running (in seconds).
    pub uptime_seconds: Option<u64>,
}

/// Collection statistics for a single device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCollectionStats {
    /// Device ID/address.
    pub device_id: String,
    /// Device alias.
    pub alias: Option<String>,
    /// Poll interval in seconds.
    pub poll_interval: u64,
    /// Time of last successful poll.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub last_poll_at: Option<OffsetDateTime>,
    /// Time of last failed poll.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub last_error_at: Option<OffsetDateTime>,
    /// Last error message.
    pub last_error: Option<String>,
    /// Total successful polls.
    pub success_count: u64,
    /// Total failed polls.
    pub failure_count: u64,
    /// Whether the device is currently being polled.
    pub polling: bool,
}

/// Response from collector control actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorActionResponse {
    pub success: bool,
    pub message: String,
    pub running: bool,
}

/// Service configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    pub server: ServerConfig,
    pub devices: Vec<DeviceConfig>,
}

/// Server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub bind: String,
}

/// Device configuration for monitoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub address: String,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default = "default_poll_interval")]
    pub poll_interval: u64,
}

fn default_poll_interval() -> u64 {
    60
}

/// Health check response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

// ==========================================================================
// ServiceClient Implementation
// ==========================================================================

impl ServiceClient {
    /// Create a new service client.
    ///
    /// # Arguments
    ///
    /// * `base_url` - The base URL of the aranet-service (e.g., "http://localhost:8080")
    pub fn new(base_url: &str) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(ServiceClientError::Request)?;

        Self::with_client_and_api_key(base_url, client, None)
    }

    /// Create a new service client with an optional API key.
    pub fn new_with_api_key(base_url: &str, api_key: Option<String>) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(ServiceClientError::Request)?;

        Self::with_client_and_api_key(base_url, client, api_key)
    }

    /// Create a client with a custom reqwest Client.
    pub fn with_client(base_url: &str, client: Client) -> Result<Self> {
        Self::with_client_and_api_key(base_url, client, None)
    }

    /// Create a client with a custom reqwest Client and optional API key.
    pub fn with_client_and_api_key(
        base_url: &str,
        client: Client,
        api_key: Option<String>,
    ) -> Result<Self> {
        let base_url = normalize_base_url(base_url)?;
        Ok(Self {
            client,
            base_url,
            api_key: sanitize_api_key(api_key),
        })
    }

    /// Get the base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Check if the service is reachable.
    pub async fn is_reachable(&self) -> bool {
        self.health().await.is_ok()
    }

    /// Get service health.
    pub async fn health(&self) -> Result<HealthResponse> {
        let url = format!("{}/api/health", self.base_url);
        self.get(&url).await
    }

    /// Get service status including collector state and device stats.
    pub async fn status(&self) -> Result<ServiceStatus> {
        let url = format!("{}/api/status", self.base_url);
        self.get(&url).await
    }

    /// Start the collector.
    pub async fn start_collector(&self) -> Result<CollectorActionResponse> {
        let url = format!("{}/api/collector/start", self.base_url);
        let response = self.post_empty(&url).await?;
        ensure_successful_action(response)
    }

    /// Stop the collector.
    pub async fn stop_collector(&self) -> Result<CollectorActionResponse> {
        let url = format!("{}/api/collector/stop", self.base_url);
        let response = self.post_empty(&url).await?;
        ensure_successful_action(response)
    }

    /// Get current configuration.
    pub async fn config(&self) -> Result<ServiceConfig> {
        let url = format!("{}/api/config", self.base_url);
        self.get(&url).await
    }

    /// Add a device to monitor.
    pub async fn add_device(&self, device: DeviceConfig) -> Result<DeviceConfig> {
        let url = format!("{}/api/config/devices", self.base_url);
        self.post_json(&url, &device).await
    }

    /// Update a device configuration.
    pub async fn update_device(
        &self,
        device_id: &str,
        alias: Option<String>,
        poll_interval: Option<u64>,
    ) -> Result<DeviceConfig> {
        self.update_device_with_alias_change(device_id, alias.map(Some), poll_interval)
            .await
    }

    /// Update a device configuration, distinguishing unchanged and cleared aliases.
    pub async fn update_device_with_alias_change(
        &self,
        device_id: &str,
        alias: Option<Option<String>>,
        poll_interval: Option<u64>,
    ) -> Result<DeviceConfig> {
        let url = format!("{}/api/config/devices/{}", self.base_url, device_id);
        let body = build_update_device_body(alias, poll_interval);
        self.put_json(&url, &body).await
    }

    /// Remove a device from monitoring.
    pub async fn remove_device(&self, device_id: &str) -> Result<()> {
        let url = format!("{}/api/config/devices/{}", self.base_url, device_id);
        self.delete(&url).await
    }

    // ======================================================================
    // Internal HTTP helpers
    // ======================================================================

    fn request(&self, method: Method, url: &str) -> RequestBuilder {
        let mut request = self.client.request(method, url);
        if let Some(api_key) = &self.api_key {
            request = request.header("X-API-Key", api_key);
        }
        request
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        let response = self.request(Method::GET, url).send().await.map_err(|e| {
            ServiceClientError::NotReachable {
                url: url.to_string(),
                source: e,
            }
        })?;

        self.handle_response(response).await
    }

    async fn post_empty<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        let response = self.request(Method::POST, url).send().await.map_err(|e| {
            ServiceClientError::NotReachable {
                url: url.to_string(),
                source: e,
            }
        })?;

        self.handle_response(response).await
    }

    async fn post_json<T: serde::de::DeserializeOwned, B: Serialize>(
        &self,
        url: &str,
        body: &B,
    ) -> Result<T> {
        let response = self
            .request(Method::POST, url)
            .json(body)
            .send()
            .await
            .map_err(|e| ServiceClientError::NotReachable {
                url: url.to_string(),
                source: e,
            })?;

        self.handle_response(response).await
    }

    async fn put_json<T: serde::de::DeserializeOwned, B: Serialize>(
        &self,
        url: &str,
        body: &B,
    ) -> Result<T> {
        let response = self
            .request(Method::PUT, url)
            .json(body)
            .send()
            .await
            .map_err(|e| ServiceClientError::NotReachable {
                url: url.to_string(),
                source: e,
            })?;

        self.handle_response(response).await
    }

    async fn delete(&self, url: &str) -> Result<()> {
        let response = self
            .request(Method::DELETE, url)
            .send()
            .await
            .map_err(|e| ServiceClientError::NotReachable {
                url: url.to_string(),
                source: e,
            })?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let message = response
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
                .unwrap_or_else(|| status.to_string());

            Err(ServiceClientError::ApiError {
                status: status.as_u16(),
                message,
            })
        }
    }

    async fn handle_response<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T> {
        let status = response.status();
        if status.is_success() {
            response.json().await.map_err(ServiceClientError::Request)
        } else {
            let message = response
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
                .unwrap_or_else(|| status.to_string());

            Err(ServiceClientError::ApiError {
                status: status.as_u16(),
                message,
            })
        }
    }
}

fn normalize_base_url(base_url: &str) -> Result<String> {
    let base_url = base_url.trim_end_matches('/').to_string();

    if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
        return Err(ServiceClientError::InvalidUrl(format!(
            "URL must start with http:// or https://, got: {}",
            base_url
        )));
    }

    Ok(base_url)
}

fn sanitize_api_key(api_key: Option<String>) -> Option<String> {
    api_key
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
}

fn build_update_device_body(
    alias: Option<Option<String>>,
    poll_interval: Option<u64>,
) -> serde_json::Value {
    let mut body = serde_json::Map::new();

    if let Some(alias) = alias {
        body.insert("alias".to_string(), serde_json::Value::from(alias));
    }

    if let Some(poll_interval) = poll_interval {
        body.insert(
            "poll_interval".to_string(),
            serde_json::Value::from(poll_interval),
        );
    }

    serde_json::Value::Object(body)
}

fn ensure_successful_action(response: CollectorActionResponse) -> Result<CollectorActionResponse> {
    if response.success {
        Ok(response)
    } else {
        Err(ServiceClientError::ApiError {
            status: REJECTED_ACTION_STATUS,
            message: response.message,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = ServiceClient::new("http://localhost:8080");
        assert!(client.is_ok());

        let client = client.unwrap();
        assert_eq!(client.base_url(), "http://localhost:8080");
        assert!(client.api_key.is_none());
    }

    #[test]
    fn test_client_normalizes_url() {
        let client = ServiceClient::new("http://localhost:8080/").unwrap();
        assert_eq!(client.base_url(), "http://localhost:8080");
    }

    #[test]
    fn test_client_invalid_url() {
        let result = ServiceClient::new("localhost:8080");
        assert!(result.is_err());
        assert!(matches!(result, Err(ServiceClientError::InvalidUrl(_))));
    }

    #[test]
    fn test_client_sanitizes_api_key() {
        let client = ServiceClient::new_with_api_key(
            "http://localhost:8080",
            Some("  test-api-key  ".to_string()),
        )
        .unwrap();
        assert_eq!(client.api_key.as_deref(), Some("test-api-key"));

        let client =
            ServiceClient::new_with_api_key("http://localhost:8080", Some("   ".to_string()))
                .unwrap();
        assert!(client.api_key.is_none());
    }

    #[test]
    fn test_update_device_body_omits_unchanged_alias() {
        let body = build_update_device_body(None, Some(300));
        assert_eq!(body, serde_json::json!({ "poll_interval": 300 }));
    }

    #[test]
    fn test_update_device_body_can_clear_alias() {
        let body = build_update_device_body(Some(None), None);
        assert_eq!(body, serde_json::json!({ "alias": null }));
    }

    #[test]
    fn test_device_config_default_poll_interval() {
        let json = r#"{"address": "test"}"#;
        let config: DeviceConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.poll_interval, 60);
    }

    #[test]
    fn test_successful_collector_action_passes_through() {
        let response = CollectorActionResponse {
            success: true,
            message: "Collector started".to_string(),
            running: true,
        };

        let result = ensure_successful_action(response).unwrap();
        assert!(result.running);
        assert_eq!(result.message, "Collector started");
    }

    #[test]
    fn test_rejected_collector_action_returns_conflict_error() {
        let response = CollectorActionResponse {
            success: false,
            message: "No devices configured".to_string(),
            running: false,
        };

        let result = ensure_successful_action(response);

        assert!(matches!(
            result,
            Err(ServiceClientError::ApiError { status, message })
                if status == REJECTED_ACTION_STATUS && message == "No devices configured"
        ));
    }
}
