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

use reqwest::Client;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// HTTP client for the aranet-service API.
#[derive(Debug, Clone)]
pub struct ServiceClient {
    client: Client,
    base_url: String,
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
        // Normalize URL (remove trailing slash)
        let base_url = base_url.trim_end_matches('/').to_string();

        // Validate URL format
        if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
            return Err(ServiceClientError::InvalidUrl(format!(
                "URL must start with http:// or https://, got: {}",
                base_url
            )));
        }

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(ServiceClientError::Request)?;

        Ok(Self { client, base_url })
    }

    /// Create a client with a custom reqwest Client.
    pub fn with_client(base_url: &str, client: Client) -> Result<Self> {
        let base_url = base_url.trim_end_matches('/').to_string();

        if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
            return Err(ServiceClientError::InvalidUrl(format!(
                "URL must start with http:// or https://, got: {}",
                base_url
            )));
        }

        Ok(Self { client, base_url })
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
        self.post_empty(&url).await
    }

    /// Stop the collector.
    pub async fn stop_collector(&self) -> Result<CollectorActionResponse> {
        let url = format!("{}/api/collector/stop", self.base_url);
        self.post_empty(&url).await
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
        let url = format!("{}/api/config/devices/{}", self.base_url, device_id);
        let body = serde_json::json!({
            "alias": alias,
            "poll_interval": poll_interval,
        });
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

    async fn get<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        let response =
            self.client
                .get(url)
                .send()
                .await
                .map_err(|e| ServiceClientError::NotReachable {
                    url: url.to_string(),
                    source: e,
                })?;

        self.handle_response(response).await
    }

    async fn post_empty<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        let response =
            self.client
                .post(url)
                .send()
                .await
                .map_err(|e| ServiceClientError::NotReachable {
                    url: url.to_string(),
                    source: e,
                })?;

        self.handle_response(response).await
    }

    async fn post_json<T: serde::de::DeserializeOwned, B: Serialize>(
        &self,
        url: &str,
        body: &B,
    ) -> Result<T> {
        let response = self.client.post(url).json(body).send().await.map_err(|e| {
            ServiceClientError::NotReachable {
                url: url.to_string(),
                source: e,
            }
        })?;

        self.handle_response(response).await
    }

    async fn put_json<T: serde::de::DeserializeOwned, B: Serialize>(
        &self,
        url: &str,
        body: &B,
    ) -> Result<T> {
        let response = self.client.put(url).json(body).send().await.map_err(|e| {
            ServiceClientError::NotReachable {
                url: url.to_string(),
                source: e,
            }
        })?;

        self.handle_response(response).await
    }

    async fn delete(&self, url: &str) -> Result<()> {
        let response =
            self.client
                .delete(url)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = ServiceClient::new("http://localhost:8080");
        assert!(client.is_ok());

        let client = client.unwrap();
        assert_eq!(client.base_url(), "http://localhost:8080");
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
    fn test_device_config_default_poll_interval() {
        let json = r#"{"address": "test"}"#;
        let config: DeviceConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.poll_interval, 60);
    }
}
