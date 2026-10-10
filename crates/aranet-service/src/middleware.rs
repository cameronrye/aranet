//! Security middleware for the aranet-service API.
//!
//! This module provides middleware for:
//! - API key authentication
//! - Host and Origin checks while authentication is off (DNS rebinding and
//!   cross-site requests)
//! - Rate limiting

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    Json,
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header, uri::Authority},
    middleware::Next,
    response::{IntoResponse, Response},
};
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};
use tracing::{debug, warn};

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
    ///
    /// Also enforces `max_entries` cap to prevent unbounded growth from many unique IPs.
    pub async fn cleanup(&self, window_secs: u64, max_entries: usize) {
        let window = Duration::from_secs(window_secs);
        let now = Instant::now();

        let mut requests = self.requests.write().await;
        // Remove expired entries
        requests.retain(|_, entry| now.duration_since(entry.window_start) < window * 2);

        // Evict oldest entries if we exceed the cap
        if requests.len() > max_entries {
            let mut entries: Vec<(IpAddr, Instant)> = requests
                .iter()
                .map(|(ip, entry)| (*ip, entry.window_start))
                .collect();
            entries.sort_by_key(|(_, start)| *start);
            let to_remove = requests.len() - max_entries;
            for (ip, _) in entries.into_iter().take(to_remove) {
                requests.remove(&ip);
            }
        }
    }
}

/// The `?token=` value on `/api/ws`, in each form that may match the API key.
///
/// Browsers can't set headers on a WebSocket upgrade, so the key travels in the
/// query. The dashboard sends `encodeURIComponent(key)`, while other clients
/// paste a base64 key in as it is, leaving `+`, `/` and `=` unencoded. The value
/// is percent-decoded without form decoding's `+`-to-space rule, so both forms
/// give back the key. The raw value is returned too when it differs, so a key
/// containing a literal `%` still matches when sent unencoded, as in 0.2.0.
/// A repeated `token` parameter yields nothing, so the request fails closed.
fn ws_query_tokens(uri: &axum::http::Uri) -> Vec<String> {
    if uri.path() != "/api/ws" {
        return Vec::new();
    }
    let mut values = uri
        .query()
        .unwrap_or_default()
        .split('&')
        .map(|param| param.split_once('=').unwrap_or((param, "")))
        .filter(|(name, _)| *name == "token")
        .map(|(_, value)| value);
    let (Some(raw), None) = (values.next(), values.next()) else {
        return Vec::new();
    };

    let mut tokens = Vec::with_capacity(2);
    if let Ok(decoded) = percent_encoding::percent_decode_str(raw).decode_utf8() {
        tokens.push(decoded.into_owned());
    }
    if tokens.first().map(String::as_str) != Some(raw) {
        tokens.push(raw.to_owned());
    }
    tokens
}

/// The token of an `Authorization: Bearer <token>` header (RFC 6750), which is
/// how Prometheus and most HTTP clients send a key. The scheme is matched ASCII
/// case-insensitively, whitespace around the token is ignored, and an empty
/// token counts as none.
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, token) = value.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then(|| token.to_owned())
}

/// API key authentication middleware.
///
/// Validates the `X-API-Key` header against the configured key. Without that
/// header, the request passes when the token of an `Authorization: Bearer <key>`
/// header matches or, on `/api/ws`, when a `token` query parameter does:
/// browsers can't set custom headers on a WebSocket upgrade, and a proxy in
/// front of the service may add its own `Authorization` header.
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

    // Skip auth for the lightweight health endpoint and the dashboard shell.
    // The dashboard still authenticates its API and WebSocket requests separately.
    if matches!(request.uri().path(), "/api/health" | "/" | "/dashboard") {
        return next.run(request).await;
    }

    // SECURITY NOTE: Query parameters may be logged by reverse proxies,
    // appear in browser history, and leak via Referer headers. Prefer a
    // header (X-API-Key or Authorization: Bearer) for non-browser clients.
    let header_key = headers
        .get("X-API-Key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let provided_keys = match header_key {
        Some(key) => vec![key],
        // Without X-API-Key, a bearer token and the /api/ws query token are
        // both candidates: a proxy in front of the service may add its own
        // Authorization header to the dashboard's WebSocket upgrade.
        None => {
            let tokens = ws_query_tokens(request.uri());
            if !tokens.is_empty() {
                debug!("WebSocket auth via query parameter (prefer X-API-Key header)");
            }
            bearer_token(&headers).into_iter().chain(tokens).collect()
        }
    };

    // Validate with constant-time comparisons to prevent timing attacks. Every
    // candidate is compared, so the timing doesn't reveal which form matched.
    let valid = config.api_key.as_deref().is_some_and(|expected| {
        provided_keys.iter().fold(false, |valid, provided| {
            valid | constant_time_eq(expected.as_bytes(), provided.as_bytes())
        })
    });

    if valid {
        next.run(request).await
    } else {
        warn!("API key authentication failed for {}", request.uri().path());
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "Invalid or missing API key",
                "hint": "Provide the API key in the X-API-Key header or as Authorization: Bearer <key>, or use the 'token' query parameter only for /api/ws"
            })),
        )
            .into_response()
    }
}

/// Who may use the service while API-key authentication is off.
///
/// Without authentication, anything that can send the service a request can
/// read and change it, and a browser sends requests on behalf of every web
/// page its user visits. [`origin_guard`] therefore only lets through:
///
/// - requests whose `Host` is `localhost` or `*.localhost`, an IP address,
///   this machine's name or `<name>.local`, or a name listed in
///   `security.allowed_hosts`. This stops DNS rebinding, where a page's own
///   host name is made to resolve to the service.
/// - requests without an `Origin` header (curl, the CLI, the TUI and the
///   GUI), same-origin requests (the dashboard) and requests from origins
///   listed in `security.cors_origins`. This stops cross-site requests,
///   including WebSocket connections and body-less form posts, which CORS
///   alone doesn't block.
#[derive(Debug, Clone)]
pub struct OriginPolicy {
    active: bool,
    any_host: bool,
    hosts: HashSet<String>,
    any_origin: bool,
    origins: HashSet<String>,
}

impl OriginPolicy {
    /// Build the policy for `config`. `machine_hostname` is this machine's
    /// name, which is allowed as a host together with `<name>.local`. The
    /// policy is inactive when `config` enables API-key authentication.
    pub fn new(config: &SecurityConfig, machine_hostname: Option<&str>) -> Self {
        // Normalise first and then look for "*", so that an entry such as
        // "*/" or " *" means what `cors_layer` makes of it.
        let mut hosts: HashSet<String> = config
            .allowed_hosts
            .iter()
            .map(String::as_str)
            .map(normalize_host_name)
            .collect();
        let any_host = hosts.remove("*");
        if let Some(name) = machine_hostname
            .map(normalize_host_name)
            .filter(|name| !name.is_empty())
        {
            hosts.insert(format!("{name}.local"));
            hosts.insert(name);
        }
        let mut origins: HashSet<String> = config
            .cors_origins
            .iter()
            .map(String::as_str)
            .map(normalize_origin)
            .collect();
        let any_origin = origins.remove("*");
        Self {
            active: !config.api_key_enabled,
            any_host,
            hosts,
            any_origin,
            origins,
        }
    }

    /// Whether a request for `host`, a `Host` header value that may include a
    /// port, may reach the service.
    pub fn allows_host(&self, host: &str) -> bool {
        if self.any_host {
            return true;
        }
        let Some(name) = host_name(host) else {
            return false;
        };
        name == "localhost"
            || name.ends_with(".localhost")
            || is_ip_literal(&name)
            || self.hosts.contains(&name)
    }

    /// Whether a request carrying `Origin: origin` may reach the service.
    /// `host` is the request's `Host` value: an origin whose host and port
    /// equal it is the service's own. `null` is never allowed.
    pub fn allows_origin(&self, origin: &str, host: Option<&str>) -> bool {
        let origin = normalize_origin(origin);
        if origin == "null" {
            return false;
        }
        // Compare the text, not parsed ports, so the service's own origin is
        // recognised behind TLS proxies and port mappings.
        let same_origin = host.is_some_and(|host| {
            origin
                .split_once("://")
                .is_some_and(|(_, authority)| authority.eq_ignore_ascii_case(host))
        });
        same_origin || self.any_origin || self.origins.contains(&origin)
    }

    /// Settings that let other web pages use the service while API-key
    /// authentication is off, as warnings for the service to log at startup.
    /// Empty when authentication is on.
    pub fn startup_warnings(&self) -> Vec<&'static str> {
        let mut warnings = Vec::new();
        if !self.active {
            return warnings;
        }
        if self.any_host {
            warnings.push(
                "security.allowed_hosts contains \"*\" while API-key authentication is off: web \
                 pages can reach this service through DNS rebinding. Enable \
                 security.api_key_enabled or list only the host names you use.",
            );
        }
        if self.any_origin {
            warnings.push(
                "security.cors_origins contains \"*\" while API-key authentication is off: any web \
                 page you visit can read and control this service. Enable \
                 security.api_key_enabled or list only the origins that need access.",
            );
        }
        // Until 0.3.0 every save through the API wrote the old default into
        // server.toml. Under the Origin check it would trust port-80 pages.
        let old_default = HashSet::from([
            "http://localhost".to_string(),
            "http://127.0.0.1".to_string(),
        ]);
        if self.origins == old_default {
            warnings.push(
                "security.cors_origins is [\"http://localhost\", \"http://127.0.0.1\"], the default \
                 that aranet-service 0.2 and earlier saved to server.toml: while API-key \
                 authentication is off, pages served on port 80 of this machine can use this \
                 service. Delete the cors_origins line unless you need those origins.",
            );
        }
        warnings
    }
}

/// The host name in a `Host` value: lowercase, without port or trailing dot.
/// IPv6 addresses keep their brackets. `None` if the value doesn't parse.
fn host_name(host: &str) -> Option<String> {
    let authority: Authority = host.parse().ok()?;
    Some(normalize_host_name(authority.host()))
}

fn normalize_host_name(name: &str) -> String {
    name.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Whether a host name is an IPv4 address or a bracketed IPv6 address.
fn is_ip_literal(name: &str) -> bool {
    name.strip_prefix('[')
        .and_then(|name| name.strip_suffix(']'))
        .unwrap_or(name)
        .parse::<IpAddr>()
        .is_ok()
}

/// An origin as browsers send it: lowercase, without a trailing `/`.
fn normalize_origin(origin: &str) -> String {
    origin.trim().trim_end_matches('/').to_ascii_lowercase()
}

/// Refuse requests for other host names (421) and cross-site browser requests
/// (403) while API-key authentication is off; see [`OriginPolicy`].
///
/// 421 rather than 403 for the host check, because the TUI and GUI report
/// every 403 as an API-key problem. When authentication is on, this does
/// nothing: the API key already keeps other web pages out, and authenticated
/// deployments behind a proxy may use any host name.
pub async fn origin_guard(
    State(policy): State<Arc<OriginPolicy>>,
    request: Request,
    next: Next,
) -> Response {
    if !policy.active {
        return next.run(request).await;
    }

    // An absolute-form request target (and HTTP/2's `:authority`) carries the
    // host in the URI; otherwise it is the Host header. Browsers always send
    // one, so a request with neither passes.
    let host = request
        .uri()
        .authority()
        .map(|authority| authority.as_str().to_owned())
        .or_else(|| {
            request
                .headers()
                .get(header::HOST)
                .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
        })
        .filter(|host| !host.is_empty());

    if let Some(host) = &host
        && !policy.allows_host(host)
    {
        let name = host_name(host).unwrap_or_else(|| host.clone());
        warn!(
            "Refused {} {}: host {:?} is not allowed while API-key authentication is off",
            request.method(),
            request.uri().path(),
            name
        );
        return (
            StatusCode::MISDIRECTED_REQUEST,
            Json(serde_json::json!({
                "error": format!("Host '{name}' is not allowed"),
                "hint": "add it to security.allowed_hosts or enable API-key authentication"
            })),
        )
            .into_response();
    }

    if let Some(origin) = request.headers().get(header::ORIGIN) {
        let origin = String::from_utf8_lossy(origin.as_bytes());
        if !policy.allows_origin(&origin, host.as_deref()) {
            warn!(
                "Refused {} {} from origin {:?}: not the service's own origin or listed in security.cors_origins",
                request.method(),
                request.uri().path(),
                origin
            );
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({
                    "error": "Cross-origin request blocked",
                    "hint": "add the page origin to security.cors_origins or enable API-key authentication"
                })),
            )
                .into_response();
        }
    }

    next.run(request).await
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
///
/// Delegates to the `subtle` crate which uses compiler barriers to prevent
/// the optimizer from introducing timing side channels.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.ct_eq(b).into()
}

/// Build a CORS layer from the security configuration.
///
/// Only the origins in `cors_origins` get CORS headers, compared as browsers
/// send them (lowercase, without a trailing `/`); the list is empty by
/// default. If `cors_origins` contains `"*"`, all origins are permitted (not
/// recommended; the service warns about it at startup while API-key
/// authentication is off).
pub fn cors_layer(config: &SecurityConfig) -> CorsLayer {
    // Normalise first and then look for "*": an entry such as "*/" or " *"
    // becomes "*", which `AllowOrigin::list` refuses with a panic.
    let origins: Vec<String> = config
        .cors_origins
        .iter()
        .map(String::as_str)
        .map(normalize_origin)
        .collect();
    if origins.iter().any(|origin| origin == "*") {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        let origins: Vec<HeaderValue> = origins
            .iter()
            .filter_map(|origin| origin.parse().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods(Any)
            .allow_headers(Any)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
        routing::get,
    };
    use tower::ServiceExt;

    fn test_security_config() -> Arc<SecurityConfig> {
        Arc::new(SecurityConfig {
            api_key_enabled: true,
            api_key: Some("1234567890abcdef1234567890abcdef".to_string()),
            rate_limit_enabled: false,
            ..Default::default()
        })
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
        state.cleanup(60, 10_000).await;
        assert_eq!(state.requests.read().await.len(), 1);
    }

    #[tokio::test]
    async fn test_rate_limit_state_cleanup_evicts_over_cap() {
        let state = RateLimitState::new();

        // Add 5 entries from different IPs
        for i in 1..=5u8 {
            let ip: IpAddr = format!("10.0.0.{}", i).parse().unwrap();
            state.check_rate_limit(ip, 100, 60).await.ok();
        }
        assert_eq!(state.requests.read().await.len(), 5);

        // Cleanup with max_entries=3 should evict the 2 oldest
        state.cleanup(60, 3).await;
        assert_eq!(state.requests.read().await.len(), 3);
    }

    #[test]
    fn test_constant_time_eq_different_lengths() {
        // Different lengths should still return false without leaking length
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"abcd", b"abc"));
        assert!(constant_time_eq(b"", b""));
        assert!(!constant_time_eq(b"", b"a"));
        assert!(!constant_time_eq(b"a", b""));
    }

    #[test]
    fn test_cors_layer_wildcard() {
        let config = SecurityConfig {
            cors_origins: vec!["*".to_string()],
            ..Default::default()
        };
        // Should not panic
        let _layer = cors_layer(&config);
    }

    #[test]
    fn test_cors_layer_specific_origins() {
        let config = SecurityConfig {
            cors_origins: vec![
                "http://localhost:3000".to_string(),
                "http://127.0.0.1:8080".to_string(),
            ],
            ..Default::default()
        };
        let _layer = cors_layer(&config);
    }

    #[test]
    fn test_cors_layer_default() {
        let config = SecurityConfig::default();
        assert!(config.cors_origins.is_empty());
        let _layer = cors_layer(&config);
    }

    fn tokens_for(uri: &str) -> Vec<String> {
        ws_query_tokens(&uri.parse().unwrap())
    }

    #[test]
    fn test_ws_query_tokens_decodes_percent_encoding() {
        assert_eq!(
            tokens_for("/api/ws?token=abc%2Bdef%2Fghi%3D"),
            vec!["abc+def/ghi=", "abc%2Bdef%2Fghi%3D"]
        );
    }

    #[test]
    fn test_ws_query_tokens_keeps_raw_plus() {
        assert_eq!(
            tokens_for("/api/ws?foo=1&token=abc+def/ghi="),
            vec!["abc+def/ghi="]
        );
    }

    #[test]
    fn test_ws_query_tokens_keeps_literal_percent() {
        assert_eq!(tokens_for("/api/ws?token=50%"), vec!["50%"]);
        assert_eq!(tokens_for("/api/ws?token=a%41"), vec!["aA", "a%41"]);
    }

    #[test]
    fn test_ws_query_tokens_only_on_ws_route() {
        assert!(tokens_for("/api/devices?token=abc").is_empty());
    }

    #[test]
    fn test_ws_query_tokens_ignores_other_params() {
        assert!(tokens_for("/api/ws?tokenx=abc&foo=bar").is_empty());
        assert!(tokens_for("/api/ws").is_empty());
    }

    #[test]
    fn test_ws_query_tokens_rejects_repeated_token() {
        assert!(tokens_for("/api/ws?token=abc&token=abc").is_empty());
        assert!(tokens_for("/api/ws?token&token=abc").is_empty());
    }

    #[tokio::test]
    async fn test_api_key_query_token_accepts_percent_encoded_key() {
        let config = Arc::new(SecurityConfig {
            api_key_enabled: true,
            api_key: Some("abc+def/ghi=0123456789abcdef0123".to_string()),
            rate_limit_enabled: false,
            ..Default::default()
        });
        let app = Router::new()
            .route("/api/ws", get(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn_with_state(config, api_key_auth));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/ws?token=abc%2Bdef%2Fghi%3D0123456789abcdef0123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    /// Send `uri` through the auth middleware with a base64-style key (`+`, `/`, `=`).
    async fn ws_status_with_base64_key(uri: &str) -> StatusCode {
        let config = Arc::new(SecurityConfig {
            api_key_enabled: true,
            api_key: Some("abc+def/ghi=0123456789abcdef0123".to_string()),
            rate_limit_enabled: false,
            ..Default::default()
        });
        let app = Router::new()
            .route("/api/ws", get(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn_with_state(config, api_key_auth));

        app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn test_api_key_query_token_accepts_raw_base64_key() {
        // Clients that paste the key into the URL unencoded, as the docs show,
        // worked in 0.2.0 and must keep working: `+` is not a space here.
        assert_eq!(
            ws_status_with_base64_key("/api/ws?token=abc+def/ghi=0123456789abcdef0123").await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn test_api_key_query_token_accepts_mixed_encoding() {
        assert_eq!(
            ws_status_with_base64_key("/api/ws?token=abc%2Bdef/ghi%3d0123456789abcdef0123").await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn test_api_key_query_token_rejects_wrong_or_duplicate_token() {
        assert_eq!(
            ws_status_with_base64_key("/api/ws?token=abc+def/ghi=0123456789abcdef0124").await,
            StatusCode::UNAUTHORIZED
        );
        // `+` must not be read as a space and a space must not be read as `+`.
        assert_eq!(
            ws_status_with_base64_key("/api/ws?token=abc%20def/ghi=0123456789abcdef0123").await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            ws_status_with_base64_key(
                "/api/ws?token=abc+def/ghi=0123456789abcdef0123&token=abc+def/ghi=0123456789abcdef0123"
            )
            .await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn test_api_key_query_token_allowed_for_websocket_route_only() {
        let app = Router::new()
            .route("/api/ws", get(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn_with_state(
                test_security_config(),
                api_key_auth,
            ));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/ws?token=1234567890abcdef1234567890abcdef")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_api_key_query_token_rejected_for_rest_route() {
        let app = Router::new()
            .route("/api/devices", get(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn_with_state(
                test_security_config(),
                api_key_auth,
            ));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/devices?token=1234567890abcdef1234567890abcdef")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    fn policy(config: SecurityConfig) -> OriginPolicy {
        OriginPolicy::new(&config, Some("myhost"))
    }

    #[test]
    fn origin_policy_allows_local_hosts() {
        let policy = policy(SecurityConfig::default());
        for host in [
            "localhost",
            "localhost:8080",
            "LOCALHOST",
            "localhost.:8080",
            "foo.localhost",
            "127.0.0.1:8080",
            "[::1]:8080",
            "10.0.0.5",
            "myhost",
            "MyHost:8080",
            "myhost.local:8080",
        ] {
            assert!(policy.allows_host(host), "{host} should be allowed");
        }
    }

    #[test]
    fn origin_policy_rejects_foreign_hosts() {
        let policy = policy(SecurityConfig::default());
        for host in [
            "evil.example:8080",
            "localhost.evil.example",
            "127.0.0.1.nip.io",
            "myhost.example.com",
            "7f000001.rbndr.us:8080",
            "bad host",
        ] {
            assert!(!policy.allows_host(host), "{host} should be rejected");
        }
    }

    #[test]
    fn allowed_hosts_extend_and_star_disables() {
        let listed = policy(SecurityConfig {
            allowed_hosts: vec!["Aranet-Service".to_string()],
            ..SecurityConfig::default()
        });
        assert!(listed.allows_host("aranet-service:8080"));
        assert!(listed.allows_host("ARANET-SERVICE"));
        assert!(!listed.allows_host("evil.example"));

        let any = policy(SecurityConfig {
            allowed_hosts: vec!["*".to_string()],
            ..SecurityConfig::default()
        });
        assert!(any.allows_host("evil.example:8080"));
    }

    #[test]
    fn origin_same_origin_listed_null_star() {
        let own = policy(SecurityConfig::default());
        let host = Some("127.0.0.1:8080");
        assert!(own.allows_origin("http://127.0.0.1:8080", host));
        assert!(own.allows_origin("HTTP://127.0.0.1:8080/", host));
        assert!(own.allows_origin("https://127.0.0.1:8080", host));
        assert!(!own.allows_origin("http://localhost:8080", host));
        assert!(!own.allows_origin("http://127.0.0.1:9000", host));
        assert!(!own.allows_origin("https://evil.example", host));
        assert!(!own.allows_origin("null", host));
        assert!(!own.allows_origin("http://127.0.0.1:8080", None));

        let listed = policy(SecurityConfig {
            cors_origins: vec!["HTTP://LocalHost:3000/".to_string()],
            ..SecurityConfig::default()
        });
        assert!(listed.allows_origin("http://localhost:3000", host));
        assert!(listed.allows_origin("http://localhost:3000", None));
        assert!(!listed.allows_origin("http://localhost:3001", host));

        let any = policy(SecurityConfig {
            cors_origins: vec!["*".to_string()],
            ..SecurityConfig::default()
        });
        assert!(any.allows_origin("https://evil.example", host));
        assert!(!any.allows_origin("null", host));
    }

    /// Status of a request with a foreign `Host` and `Origin` through
    /// `origin_guard` alone.
    async fn guarded_status(config: SecurityConfig) -> StatusCode {
        let policy = Arc::new(OriginPolicy::new(&config, None));
        Router::new()
            .route("/api/devices", get(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn_with_state(policy, origin_guard))
            .oneshot(
                Request::builder()
                    .uri("/api/devices")
                    .header("Host", "evil.example")
                    .header("Origin", "https://evil.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn inactive_when_api_key_enabled() {
        let authenticated = SecurityConfig {
            api_key_enabled: true,
            api_key: Some("1234567890abcdef1234567890abcdef".to_string()),
            ..SecurityConfig::default()
        };
        assert!(!OriginPolicy::new(&authenticated, None).active);
        assert_eq!(guarded_status(authenticated).await, StatusCode::OK);

        assert!(OriginPolicy::new(&SecurityConfig::default(), None).active);
        assert_eq!(
            guarded_status(SecurityConfig::default()).await,
            StatusCode::MISDIRECTED_REQUEST
        );
    }

    /// The `Access-Control-Allow-Origin` value that `cors_layer(config)`
    /// answers a preflight from `origin` with.
    async fn preflight_allow_origin(config: &SecurityConfig, origin: &str) -> Option<String> {
        let response = Router::new()
            .route("/api/devices", get(|| async { StatusCode::OK }))
            .layer(cors_layer(config))
            .oneshot(
                Request::builder()
                    .method(axum::http::Method::OPTIONS)
                    .uri("/api/devices")
                    .header("Origin", origin)
                    .header("Access-Control-Request-Method", "POST")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        response
            .headers()
            .get("access-control-allow-origin")
            .map(|value| value.to_str().unwrap().to_owned())
    }

    #[tokio::test]
    async fn cors_layer_matches_normalised_origins() {
        let config = SecurityConfig {
            cors_origins: vec!["HTTP://LocalHost:3000/".to_string()],
            ..SecurityConfig::default()
        };
        assert_eq!(
            preflight_allow_origin(&config, "http://localhost:3000")
                .await
                .as_deref(),
            Some("http://localhost:3000")
        );
        assert_eq!(
            preflight_allow_origin(&config, "http://localhost:3001").await,
            None
        );
    }

    #[tokio::test]
    async fn star_variants_mean_every_origin() {
        // Each normalises to "*", which `AllowOrigin::list` panics on.
        for entry in ["*", "*/", " *", "* "] {
            let config = SecurityConfig {
                cors_origins: vec![entry.to_string()],
                ..SecurityConfig::default()
            };
            assert_eq!(
                preflight_allow_origin(&config, "https://evil.example")
                    .await
                    .as_deref(),
                Some("*"),
                "{entry:?}"
            );
            let policy = policy(config);
            assert!(
                policy.allows_origin("https://evil.example", None),
                "{entry:?}"
            );
            assert_eq!(policy.startup_warnings().len(), 1, "{entry:?}");
        }
    }

    #[test]
    fn startup_warnings_name_what_opens_the_service() {
        let defaults = policy(SecurityConfig::default());
        assert!(defaults.startup_warnings().is_empty());
        let listed = policy(SecurityConfig {
            allowed_hosts: vec!["aranet-service".to_string()],
            cors_origins: vec!["http://localhost:3000".to_string()],
            ..SecurityConfig::default()
        });
        assert!(listed.startup_warnings().is_empty());

        let open = SecurityConfig {
            allowed_hosts: vec!["*".to_string()],
            cors_origins: vec!["*".to_string()],
            ..SecurityConfig::default()
        };
        let warnings = policy(open.clone()).startup_warnings();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].starts_with("security.allowed_hosts contains \"*\""));
        assert!(warnings[1].starts_with("security.cors_origins contains \"*\""));

        // What aranet-service 0.2 saved on every device change.
        let old_default = policy(SecurityConfig {
            cors_origins: vec![
                "http://localhost".to_string(),
                "http://127.0.0.1/".to_string(),
            ],
            ..SecurityConfig::default()
        });
        let warnings = old_default.startup_warnings();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("aranet-service 0.2"), "{warnings:?}");

        // With authentication on, none of them lets a web page in.
        let authenticated = policy(SecurityConfig {
            api_key_enabled: true,
            api_key: Some("1234567890abcdef1234567890abcdef".to_string()),
            ..open
        });
        assert!(authenticated.startup_warnings().is_empty());
    }

    #[test]
    fn origin_policy_near_misses_and_spaced_star() {
        let defaults = policy(SecurityConfig::default());

        // Only `localhost` itself and names under `.localhost` are local.
        for host in ["notlocalhost", "evil-localhost:8080", "localhost.example"] {
            assert!(!defaults.allows_host(host), "{host} should be rejected");
        }

        // An origin is the service's own only when its whole authority is the
        // Host: a longer name that starts with it, another port and a missing
        // port are all other origins.
        assert!(defaults.allows_origin("http://myhost", Some("myhost")));
        for (origin, host) in [
            ("http://myhost.evil.example", "myhost"),
            ("http://myhost:8080", "myhost"),
            ("http://127.0.0.1:80801", "127.0.0.1:8080"),
            ("http://127.0.0.1", "127.0.0.1:8080"),
            ("http://my", "myhost"),
        ] {
            assert!(
                !defaults.allows_origin(origin, Some(host)),
                "{origin} should be rejected for Host {host}"
            );
        }

        // Only exactly the two origins that 0.2 saved are warned about: a list
        // that was extended or cut down was written on purpose.
        let near_old_default: [&[&str]; 3] = [
            &["http://localhost"],
            &["http://127.0.0.1"],
            &[
                "http://localhost",
                "http://127.0.0.1",
                "http://localhost:3000",
            ],
        ];
        for origins in near_old_default {
            let policy = policy(SecurityConfig {
                cors_origins: origins.iter().map(|origin| origin.to_string()).collect(),
                ..SecurityConfig::default()
            });
            let warnings = policy.startup_warnings();
            assert!(warnings.is_empty(), "{origins:?}: {warnings:?}");
        }

        // `app()` also takes configs that were never validated. There a "*"
        // with spaces around it means every host, as it does in
        // `cors_origins`, and is warned about.
        for entry in [" *", "* "] {
            let policy = policy(SecurityConfig {
                allowed_hosts: vec![entry.to_string()],
                ..SecurityConfig::default()
            });
            assert!(policy.allows_host("evil.example:8080"), "{entry:?}");
            let warnings = policy.startup_warnings();
            assert_eq!(warnings.len(), 1, "{entry:?}: {warnings:?}");
            assert!(warnings[0].starts_with("security.allowed_hosts contains \"*\""));
        }
    }

    /// The status of `GET uri` with `headers`, through the auth middleware
    /// with `test_security_config()`'s key. `/api/devices` and `/api/ws` are
    /// routed.
    async fn auth_status(uri: &str, headers: &[(&str, &str)]) -> StatusCode {
        let app = Router::new()
            .route("/api/devices", get(|| async { StatusCode::OK }))
            .route("/api/ws", get(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn_with_state(
                test_security_config(),
                api_key_auth,
            ));
        let mut request = Request::builder().uri(uri);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        app.oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    /// The key in `test_security_config()`.
    const CONFIGURED_KEY: &str = "1234567890abcdef1234567890abcdef";

    // The messages of the tests below name the case and leave out the header
    // value, which holds the key.

    #[tokio::test]
    async fn test_api_key_accepts_bearer_token() {
        for (case, value) in [
            ("one space", format!("Bearer {CONFIGURED_KEY}")),
            (
                "spaces around the token",
                format!("Bearer   {CONFIGURED_KEY}  "),
            ),
        ] {
            assert_eq!(
                auth_status("/api/devices", &[("Authorization", &value)]).await,
                StatusCode::OK,
                "{case}"
            );
        }
    }

    #[tokio::test]
    async fn test_bearer_scheme_is_case_insensitive() {
        for scheme in ["bearer", "BEARER", "bEaReR"] {
            let value = format!("{scheme} {CONFIGURED_KEY}");
            assert_eq!(
                auth_status("/api/devices", &[("Authorization", &value)]).await,
                StatusCode::OK,
                "{scheme:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_bearer_with_wrong_key_is_rejected() {
        for (case, value) in [
            ("another key", "Bearer wrong-key".to_string()),
            (
                "the key with a character added",
                format!("Bearer {CONFIGURED_KEY}0"),
            ),
            (
                "the key without its first character",
                format!("Bearer {}", &CONFIGURED_KEY[1..]),
            ),
            (
                "no space after the scheme",
                format!("Bearer{CONFIGURED_KEY}"),
            ),
            ("no token", "Bearer".to_string()),
            ("an empty token", "Bearer   ".to_string()),
        ] {
            assert_eq!(
                auth_status("/api/devices", &[("Authorization", &value)]).await,
                StatusCode::UNAUTHORIZED,
                "{case}"
            );
        }
    }

    #[tokio::test]
    async fn test_basic_auth_is_not_accepted() {
        for (case, value) in [
            ("the Basic scheme", format!("Basic {CONFIGURED_KEY}")),
            ("the key without a scheme", CONFIGURED_KEY.to_string()),
        ] {
            assert_eq!(
                auth_status("/api/devices", &[("Authorization", &value)]).await,
                StatusCode::UNAUTHORIZED,
                "{case}"
            );
        }
    }

    #[tokio::test]
    async fn test_x_api_key_is_checked_before_bearer() {
        let bearer = format!("Bearer {CONFIGURED_KEY}");
        assert_eq!(
            auth_status(
                "/api/devices",
                &[("X-API-Key", "wrong-key"), ("Authorization", &bearer)]
            )
            .await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            auth_status(
                "/api/devices",
                &[
                    ("X-API-Key", CONFIGURED_KEY),
                    ("Authorization", "Bearer wrong-key")
                ]
            )
            .await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn test_websocket_accepts_query_token_or_bearer() {
        let query = format!("/api/ws?token={CONFIGURED_KEY}");
        let bearer = format!("Bearer {CONFIGURED_KEY}");
        // An authenticating proxy may add its own bearer token to the
        // dashboard's WebSocket upgrade: the query token still counts.
        assert_eq!(
            auth_status(&query, &[("Authorization", "Bearer proxy-token")]).await,
            StatusCode::OK,
            "a foreign bearer token hid the query token"
        );
        assert_eq!(
            auth_status("/api/ws?token=wrong-key", &[("Authorization", &bearer)]).await,
            StatusCode::OK,
            "a wrong query token hid the bearer token"
        );
        // The query token still counts only on /api/ws, and X-API-Key alone
        // decides when it is present.
        assert_eq!(
            auth_status(
                &format!("/api/devices?token={CONFIGURED_KEY}"),
                &[("Authorization", "Bearer proxy-token")]
            )
            .await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            auth_status(&query, &[("X-API-Key", "wrong-key")]).await,
            StatusCode::UNAUTHORIZED
        );
    }

    /// An `Authorization` value that can't be read as text carries no token:
    /// the request is refused like one without the header, and on `/api/ws`
    /// the query token still counts.
    #[tokio::test]
    async fn test_unreadable_authorization_header_carries_no_token() {
        // 0xE9 is not visible ASCII, so `HeaderValue::to_str` fails.
        let mut unreadable = format!("Bearer {CONFIGURED_KEY}").into_bytes();
        unreadable.push(0xe9);
        for (uri, expected) in [
            ("/api/devices".to_string(), StatusCode::UNAUTHORIZED),
            (format!("/api/ws?token={CONFIGURED_KEY}"), StatusCode::OK),
        ] {
            let app = Router::new()
                .route("/api/devices", get(|| async { StatusCode::OK }))
                .route("/api/ws", get(|| async { StatusCode::OK }))
                .layer(axum::middleware::from_fn_with_state(
                    test_security_config(),
                    api_key_auth,
                ));
            let request = Request::builder()
                .uri(&uri)
                .header("Authorization", unreadable.as_slice())
                .body(Body::empty())
                .unwrap();
            let status = app.oneshot(request).await.unwrap().status();
            // The path only: the query holds the key.
            let path = uri.split('?').next().unwrap_or_default();
            assert_eq!(status, expected, "{path}");
        }
    }

    /// The whole hint of a 401, which names every way to send the key.
    #[tokio::test]
    async fn test_unauthorized_hint_names_every_way_to_send_the_key() {
        let app = Router::new()
            .route("/api/devices", get(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn_with_state(
                test_security_config(),
                api_key_auth,
            ));
        let request = Request::builder()
            .uri("/api/devices")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            body["hint"],
            "Provide the API key in the X-API-Key header or as Authorization: Bearer <key>, \
             or use the 'token' query parameter only for /api/ws"
        );
    }
}
