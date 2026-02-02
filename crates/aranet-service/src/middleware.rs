//! Security middleware for the aranet-service API.
//!
//! This module provides middleware for:
//! - API key authentication
//! - Rate limiting
//! - Input sanitization

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    Json,
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use tokio::sync::RwLock;
use tracing::warn;

use crate::config::SecurityConfig;

/// State for rate limiting.
#[derive(Debug, Default)]
pub struct RateLimitState {
    /// Request counts per IP address.
    requests: RwLock<HashMap<IpAddr, RateLimitEntry>>,
}

#[derive(Debug, Clone)]
struct RateLimitEntry {
    count: u32,
    window_start: Instant,
}

impl RateLimitState {
    /// Create a new rate limit state.
    pub fn new() -> Self {
        Self {
            requests: RwLock::new(HashMap::new()),
        }
    }

    /// Check if a request from the given IP should be rate limited.
    pub async fn check_rate_limit(
        &self,
        ip: IpAddr,
        max_requests: u32,
        window_secs: u64,
    ) -> Result<(), (u32, u64)> {
        let window = Duration::from_secs(window_secs);
        let now = Instant::now();

        let mut requests = self.requests.write().await;

        let entry = requests.entry(ip).or_insert_with(|| RateLimitEntry {
            count: 0,
            window_start: now,
        });

        // Reset window if expired
        if now.duration_since(entry.window_start) >= window {
            entry.count = 0;
            entry.window_start = now;
        }

        entry.count += 1;

        if entry.count > max_requests {
            let remaining_secs = window
                .checked_sub(now.duration_since(entry.window_start))
                .map(|d| d.as_secs())
                .unwrap_or(0);
            Err((max_requests, remaining_secs))
        } else {
            Ok(())
        }
    }

    /// Clean up expired entries to prevent memory leaks.
    pub async fn cleanup(&self, window_secs: u64) {
        let window = Duration::from_secs(window_secs);
        let now = Instant::now();

        let mut requests = self.requests.write().await;
        requests.retain(|_, entry| now.duration_since(entry.window_start) < window * 2);
    }
}

/// API key authentication middleware.
///
/// Checks for the `X-API-Key` header and validates against the configured key.
/// For WebSocket connections (which cannot set custom headers from browsers),
/// also accepts a `token` query parameter.
///
/// Returns 401 Unauthorized if the key is missing or invalid.
pub async fn api_key_auth(
    headers: HeaderMap,
    State(config): State<Arc<SecurityConfig>>,
    request: Request,
    next: Next,
) -> Response {
    // Skip auth if not enabled
    if !config.api_key_enabled {
        return next.run(request).await;
    }

    // Skip auth for health endpoint (monitoring should work without auth)
    if request.uri().path() == "/api/health" {
        return next.run(request).await;
    }

    // Get the API key from header first
    let mut provided_key = headers.get("X-API-Key").and_then(|v| v.to_str().ok());

    // For WebSocket connections, also check query parameter
    // (browsers cannot set custom headers during WebSocket upgrade)
    if provided_key.is_none()
        && let Some(query) = request.uri().query()
    {
        provided_key = query.split('&').find_map(|param| {
            let mut parts = param.splitn(2, '=');
            match (parts.next(), parts.next()) {
                (Some("token"), Some(value)) => Some(value),
                _ => None,
            }
        });
    }

    // Validate
    let valid = match (&config.api_key, provided_key) {
        (Some(expected), Some(provided)) => {
            // Use constant-time comparison to prevent timing attacks
            constant_time_eq(expected.as_bytes(), provided.as_bytes())
        }
        _ => false,
    };

    if valid {
        next.run(request).await
    } else {
        warn!("API key authentication failed for {}", request.uri().path());
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "Invalid or missing API key",
                "hint": "Provide a valid API key in the X-API-Key header or as a 'token' query parameter"
            })),
        )
            .into_response()
    }
}

/// Rate limiting middleware.
///
/// Limits requests per IP address within a time window.
/// Returns 429 Too Many Requests if the limit is exceeded.
pub async fn rate_limit(
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    State((config, state)): State<(Arc<SecurityConfig>, Arc<RateLimitState>)>,
    request: Request,
    next: Next,
) -> Response {
    // Skip if not enabled
    if !config.rate_limit_enabled {
        return next.run(request).await;
    }

    let ip = addr.ip();

    match state
        .check_rate_limit(
            ip,
            config.rate_limit_requests,
            config.rate_limit_window_secs,
        )
        .await
    {
        Ok(()) => next.run(request).await,
        Err((limit, retry_after)) => {
            warn!("Rate limit exceeded for {} on {}", ip, request.uri().path());
            (
                StatusCode::TOO_MANY_REQUESTS,
                [
                    ("Retry-After", retry_after.to_string()),
                    ("X-RateLimit-Limit", limit.to_string()),
                    ("X-RateLimit-Remaining", "0".to_string()),
                ],
                Json(serde_json::json!({
                    "error": "Too many requests",
                    "retry_after": retry_after
                })),
            )
                .into_response()
        }
    }
}

/// Constant-time byte comparison to prevent timing attacks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    let mut result = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

/// Sanitize a device name to prevent XSS and injection attacks.
///
/// This removes or escapes potentially dangerous characters while preserving
/// the essential information needed to identify devices.
///
/// # Examples
///
/// ```
/// use aranet_service::middleware::sanitize_device_name;
///
/// assert_eq!(sanitize_device_name("Aranet4 12345"), "Aranet4 12345");
/// assert_eq!(sanitize_device_name("<script>alert('xss')</script>"), "scriptalertxssscript");
/// assert_eq!(sanitize_device_name("Device\"with'quotes"), "Devicewithquotes");
/// ```
pub fn sanitize_device_name(name: &str) -> String {
    name.chars()
        .filter(|c| {
            // Allow alphanumeric, spaces, dashes, underscores, colons (for MAC addresses)
            c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_' || *c == ':'
        })
        .take(64) // Limit length
        .collect()
}

/// Validate and sanitize a device identifier.
///
/// Returns an error if the identifier is empty or contains only invalid characters.
pub fn validate_device_id(id: &str) -> Result<String, &'static str> {
    let sanitized = sanitize_device_name(id);

    if sanitized.is_empty() {
        return Err("Device ID cannot be empty");
    }

    if sanitized.len() < 3 {
        return Err("Device ID too short");
    }

    Ok(sanitized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_device_name_normal() {
        assert_eq!(sanitize_device_name("Aranet4 12345"), "Aranet4 12345");
        assert_eq!(
            sanitize_device_name("AA:BB:CC:DD:EE:FF"),
            "AA:BB:CC:DD:EE:FF"
        );
        assert_eq!(sanitize_device_name("office-sensor_1"), "office-sensor_1");
    }

    #[test]
    fn test_sanitize_device_name_xss() {
        assert_eq!(
            sanitize_device_name("<script>alert('xss')</script>"),
            "scriptalertxssscript"
        );
        assert_eq!(sanitize_device_name("onclick=\"evil()\""), "onclickevil");
        // Colons are allowed for MAC addresses, so "data:" is preserved
        assert_eq!(
            sanitize_device_name("data:text/html,<script>"),
            "data:texthtmlscript"
        );
    }

    #[test]
    fn test_sanitize_device_name_length() {
        let long_name = "a".repeat(100);
        let sanitized = sanitize_device_name(&long_name);
        assert_eq!(sanitized.len(), 64);
    }

    #[test]
    fn test_sanitize_device_name_empty() {
        assert_eq!(sanitize_device_name(""), "");
        assert_eq!(sanitize_device_name("<>"), "");
    }

    #[test]
    fn test_validate_device_id_valid() {
        assert!(validate_device_id("Aranet4 12345").is_ok());
        assert!(validate_device_id("AA:BB:CC:DD:EE:FF").is_ok());
    }

    #[test]
    fn test_validate_device_id_empty() {
        assert!(validate_device_id("").is_err());
        assert!(validate_device_id("<>").is_err());
    }

    #[test]
    fn test_validate_device_id_too_short() {
        assert!(validate_device_id("AB").is_err());
        assert!(validate_device_id("A").is_err());
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq(b"hello", b"hello"));
        assert!(!constant_time_eq(b"hello", b"world"));
        assert!(!constant_time_eq(b"short", b"longer"));
    }

    #[tokio::test]
    async fn test_rate_limit_state_allows_requests() {
        let state = RateLimitState::new();
        let ip: IpAddr = "127.0.0.1".parse().unwrap();

        // First request should succeed
        assert!(state.check_rate_limit(ip, 10, 60).await.is_ok());

        // Second request should succeed
        assert!(state.check_rate_limit(ip, 10, 60).await.is_ok());
    }

    #[tokio::test]
    async fn test_rate_limit_state_blocks_excess() {
        let state = RateLimitState::new();
        let ip: IpAddr = "127.0.0.1".parse().unwrap();

        // Make 3 requests (limit is 2)
        assert!(state.check_rate_limit(ip, 2, 60).await.is_ok());
        assert!(state.check_rate_limit(ip, 2, 60).await.is_ok());
        assert!(state.check_rate_limit(ip, 2, 60).await.is_err());
    }

    #[tokio::test]
    async fn test_rate_limit_state_per_ip() {
        let state = RateLimitState::new();
        let ip1: IpAddr = "127.0.0.1".parse().unwrap();
        let ip2: IpAddr = "127.0.0.2".parse().unwrap();

        // Exhaust IP1's limit
        assert!(state.check_rate_limit(ip1, 1, 60).await.is_ok());
        assert!(state.check_rate_limit(ip1, 1, 60).await.is_err());

        // IP2 should still be allowed
        assert!(state.check_rate_limit(ip2, 1, 60).await.is_ok());
    }

    #[tokio::test]
    async fn test_rate_limit_state_cleanup() {
        let state = RateLimitState::new();
        let ip: IpAddr = "127.0.0.1".parse().unwrap();

        // Add an entry
        state.check_rate_limit(ip, 10, 60).await.ok();

        // Should have one entry
        assert_eq!(state.requests.read().await.len(), 1);

        // Cleanup (entries within 2x window are kept)
        state.cleanup(60).await;
        assert_eq!(state.requests.read().await.len(), 1);
    }

    #[test]
    fn test_extract_token_from_query() {
        // Helper to extract token from query string (mirrors middleware logic)
        fn extract_token(query: &str) -> Option<&str> {
            query.split('&').find_map(|param| {
                let mut parts = param.splitn(2, '=');
                match (parts.next(), parts.next()) {
                    (Some("token"), Some(value)) => Some(value),
                    _ => None,
                }
            })
        }

        assert_eq!(extract_token("token=abc123"), Some("abc123"));
        assert_eq!(extract_token("foo=bar&token=abc123"), Some("abc123"));
        assert_eq!(extract_token("token=abc123&foo=bar"), Some("abc123"));
        assert_eq!(extract_token("foo=bar"), None);
        assert_eq!(extract_token(""), None);
        assert_eq!(extract_token("tokenx=abc123"), None);
    }
}
