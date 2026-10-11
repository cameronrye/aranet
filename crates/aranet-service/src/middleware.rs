//! Security middleware for the aranet-service API.
//!
//! This module provides middleware for:
//! - API key authentication
//! - Host and Origin checks while authentication is off (DNS rebinding and
//!   cross-site requests)
//! - Rate limiting

use std::collections::HashSet;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr};
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    Json,
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header, uri::Authority},
    middleware::Next,
    response::{IntoResponse, Response},
};
use governor::clock::{Clock, DefaultClock, Reference};
use governor::middleware::NoOpMiddleware;
use governor::state::keyed::DashMapStateStore;
use governor::{Quota, RateLimiter};
use tower_http::cors::{Any, CorsLayer};
use tracing::{debug, warn};

use crate::config::{MAX_RATE_LIMIT_WINDOW_SECS, SecurityConfig};

/// The client a request is rate-limited as: its IPv4 address, or the /64
/// network of its IPv6 address.
///
/// A host can use any address in its network's /64 (SLAAC and privacy
/// addresses), so keying on the full address would let it send every request
/// from a new address. Every IPv6 client on that network therefore shares one
/// budget, as do all link-local clients (`fe80::/64`). An IPv4-mapped IPv6
/// address (`::ffff:a.b.c.d`, which a dual-stack listener reports for IPv4
/// clients) counts as its IPv4 address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RateLimitKey {
    /// An IPv4 client.
    V4(Ipv4Addr),
    /// The upper 64 bits (the network prefix) of an IPv6 client's address.
    V6Net64(u64),
}

impl From<IpAddr> for RateLimitKey {
    fn from(ip: IpAddr) -> Self {
        match ip.to_canonical() {
            IpAddr::V4(ip) => Self::V4(ip),
            IpAddr::V6(ip) => Self::V6Net64((ip.to_bits() >> 64) as u64),
        }
    }
}

/// Per-client rate limiting state (GCRA, from `governor`).
///
/// Each client ([`RateLimitKey`]) may send `security.rate_limit_requests`
/// requests at once, and then one more every
/// `rate_limit_window_secs / rate_limit_requests`. A check never waits for a
/// global lock: it locks the map shard that holds the client, and an allowed
/// request also reads each shard's length.
///
/// A client is forgotten only once its whole budget has been back for at
/// least one refill period, so a throttled client never is.
/// [`cleanup`](Self::cleanup) forgets such clients, and the service runs it
/// once per window. While more than `security.rate_limit_max_entries` clients
/// are tracked, allowed requests also sweep, at most once per refill period,
/// so the map holds more entries than that only while that many clients are
/// active.
///
/// Behind a reverse proxy, or Docker's port proxy, every client has the
/// proxy's address and so shares one budget. So do the IPv6 clients of one
/// /64, which is normally every IPv6 client on the service's own network, and
/// all link-local clients (`fe80::/64`). Over IPv4, each of them has a budget
/// of its own.
pub struct RateLimitState<C: Clock = DefaultClock> {
    limiter:
        RateLimiter<RateLimitKey, DashMapStateStore<RateLimitKey>, C, NoOpMiddleware<C::Instant>>,
    /// The burst size, reported in `X-RateLimit-Limit`.
    limit: u32,
    /// How long one request takes to refill: the window divided by `limit`.
    period: Duration,
    max_entries: usize,
    /// When the last over-the-cap sweep ran. `try_lock` keeps concurrent
    /// requests from sweeping at the same time.
    last_sweep: Mutex<Option<C::Instant>>,
}

impl RateLimitState {
    /// Create the rate limiting state for `config`, on the system's monotonic
    /// clock.
    pub fn new(config: &SecurityConfig) -> Self {
        Self::with_clock(config, DefaultClock::default())
    }
}

impl Default for RateLimitState {
    /// The state for [`SecurityConfig::default`]'s limits.
    fn default() -> Self {
        Self::new(&SecurityConfig::default())
    }
}

impl<C: Clock> RateLimitState<C> {
    /// Create the rate limiting state for `config` on `clock` (tests use
    /// [`governor::clock::FakeRelativeClock`]).
    ///
    /// Values that validation rejects are clamped instead, because the state
    /// is built even while rate limiting is off: a `rate_limit_requests` of 0
    /// counts as 1, and the window as 1 second to 1 day.
    pub fn with_clock(config: &SecurityConfig, clock: C) -> Self {
        let requests = NonZeroU32::new(config.rate_limit_requests).unwrap_or(NonZeroU32::MIN);
        let window = Duration::from_secs(
            config
                .rate_limit_window_secs
                .clamp(1, MAX_RATE_LIMIT_WINDOW_SECS),
        );
        let period = (window / requests.get()).max(Duration::from_nanos(1));
        let quota = Quota::with_period(period)
            .expect("the refill period is at least 1 ns")
            .allow_burst(requests);
        Self {
            limiter: RateLimiter::dashmap_with_clock(quota, clock),
            limit: requests.get(),
            period,
            max_entries: config.rate_limit_max_entries,
            last_sweep: Mutex::new(None),
        }
    }

    /// Count a request from `ip` against its client's budget.
    ///
    /// `Err` carries how long until the client's next request is allowed.
    pub fn check(&self, ip: IpAddr) -> Result<(), Duration> {
        match self.limiter.check_key(&RateLimitKey::from(ip)) {
            Ok(()) => {
                self.sweep_if_over_cap();
                Ok(())
            }
            Err(not_until) => Err(not_until.wait_time_from(self.limiter.clock().now())),
        }
    }

    /// Forget every client whose whole budget has been back for at least one
    /// refill period (at most a window plus a period after its last request),
    /// and release the memory. A throttled client is never forgotten.
    pub fn cleanup(&self) {
        self.limiter.retain_recent();
        self.limiter.shrink_to_fit();
    }

    /// The number of clients tracked.
    pub fn len(&self) -> usize {
        self.limiter.len()
    }

    /// Whether no client is tracked.
    pub fn is_empty(&self) -> bool {
        self.limiter.is_empty()
    }

    /// Forget idle clients while more than `max_entries` are tracked, at most
    /// once per refill period.
    ///
    /// Only an allowed request can add a client (a client's first request is
    /// always allowed), so only allowed requests call this.
    fn sweep_if_over_cap(&self) {
        if self.limiter.len() <= self.max_entries {
            return;
        }
        let Ok(mut last_sweep) = self.last_sweep.try_lock() else {
            return;
        };
        let now = self.limiter.clock().now();
        if last_sweep.is_some_and(|at| Duration::from(now.duration_since(at)) < self.period) {
            return;
        }
        self.limiter.retain_recent();
        *last_sweep = Some(now);
    }
}

impl<C: Clock> fmt::Debug for RateLimitState<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RateLimitState")
            .field("limit", &self.limit)
            .field("period", &self.period)
            .field("max_entries", &self.max_entries)
            .field("keys", &self.len())
            .finish_non_exhaustive()
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
/// token counts as none. More than one `Authorization` header yields nothing,
/// whatever their order or schemes, so the result never depends on which one
/// is read.
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let (Some(value), None) = (values.next(), values.next()) else {
        return None;
    };
    let value = value.to_str().ok()?.trim();
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
/// Each of the three carries a key only when it is unambiguous: an `X-API-Key`
/// header that is repeated or isn't visible ASCII carries none and, being
/// present, still rules out the other two, and a repeated `Authorization`
/// header or `token` parameter carries none.
///
/// Returns 401 Unauthorized if the key is missing or invalid, and always
/// while the configured key is missing or empty.
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
    let mut api_key_headers = headers.get_all("X-API-Key").iter();
    let provided_keys = match api_key_headers.next() {
        // With X-API-Key nothing else is tried, and it is a candidate only
        // when it is sent once and reads as text (visible ASCII): otherwise
        // no key is compared and the request is refused.
        Some(value) => match (value.to_str(), api_key_headers.next()) {
            (Ok(key), None) => vec![key.to_owned()],
            _ => Vec::new(),
        },
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
    // An empty configured key matches nothing: it would equal an empty
    // `X-API-Key` or `?token=`. The service never runs with one (loading
    // the configuration replaces it), but this function is public and takes
    // any `SecurityConfig`.
    let valid = config
        .api_key
        .as_deref()
        .filter(|expected| !expected.is_empty())
        .is_some_and(|expected| {
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
/// Counts each request against its client's budget in `state` (see
/// [`RateLimitState`]) and answers 429 Too Many Requests once the budget is
/// spent, with `Retry-After` in whole seconds, rounded up.
/// `config.rate_limit_enabled` switches it on; the limits are the ones `state`
/// was created with.
///
/// A request without a peer address (no [`ConnectInfo`], which
/// `into_make_service_with_connect_info` supplies) gets 500 Internal Server
/// Error, whether rate limiting is on or off.
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

    match state.check(ip) {
        Ok(()) => next.run(request).await,
        Err(wait) => {
            let retry_after = retry_after_secs(wait);
            warn!("Rate limit exceeded for {} on {}", ip, request.uri().path());
            (
                StatusCode::TOO_MANY_REQUESTS,
                [
                    ("Retry-After", retry_after.to_string()),
                    ("X-RateLimit-Limit", state.limit.to_string()),
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

/// `wait` in the whole seconds `Retry-After` needs: rounded up, and at least
/// 1, so a client that waits as told is never refused again.
pub(crate) fn retry_after_secs(wait: Duration) -> u64 {
    wait.as_secs()
        .saturating_add(u64::from(wait.subsec_nanos() > 0))
        .max(1)
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
    use governor::clock::FakeRelativeClock;
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

    // ---- Rate limiting (SV-14) ----
    //
    // The limiter tests run on governor's FakeRelativeClock, which only moves
    // when a test advances it: tokio's paused clock doesn't drive governor.
    // With a burst of `b` over `window`, one request refills every
    // `t = window / b`. governor keeps each client's theoretical arrival time
    // (TAT): a first request sets it to `now + t`, each allowed request adds
    // `t`, and a request is refused while `now < TAT - (b - 1) * t`.
    // `cleanup` forgets a client once `TAT <= now - t`.

    fn ip(addr: &str) -> IpAddr {
        addr.parse().unwrap()
    }

    /// A limiter for these limits on a fake clock that starts at 0, and a
    /// handle to that clock (clones share its time).
    fn limiter(
        requests: u32,
        window_secs: u64,
        max_entries: usize,
    ) -> (RateLimitState<FakeRelativeClock>, FakeRelativeClock) {
        let config = SecurityConfig {
            rate_limit_enabled: true,
            rate_limit_requests: requests,
            rate_limit_window_secs: window_secs,
            rate_limit_max_entries: max_entries,
            ..Default::default()
        };
        let clock = FakeRelativeClock::default();
        (RateLimitState::with_clock(&config, clock.clone()), clock)
    }

    #[test]
    fn burst_then_throttle_and_retry_after() {
        // A burst of 2, then one request every 30 s.
        let (state, clock) = limiter(2, 60, 100);
        let client = ip("192.0.2.7");
        assert_eq!(state.check(client), Ok(()));
        assert_eq!(state.check(client), Ok(()));
        assert_eq!(state.check(client), Err(Duration::from_secs(30)));

        clock.advance(Duration::from_secs(30));
        assert_eq!(state.check(client), Ok(()));
        // One request came back, not a new window's worth.
        assert_eq!(state.check(client), Err(Duration::from_secs(30)));
    }

    #[test]
    fn ipv6_clients_in_one_64_share_a_bucket() {
        let (state, _clock) = limiter(1, 60, 100);
        assert_eq!(state.check(ip("2001:db8::1")), Ok(()));
        assert!(
            state.check(ip("2001:db8::2")).is_err(),
            "a second address in the same /64 got a fresh budget"
        );
        // The next /64 is another client.
        assert_eq!(state.check(ip("2001:db8:0:1::1")), Ok(()));
    }

    #[test]
    fn ipv4_mapped_clients_stay_separate() {
        // Masking ::ffff:a.b.c.d to /64 would put every IPv4 client of a
        // dual-stack listener into ::/64.
        let (state, _clock) = limiter(1, 60, 100);
        assert_eq!(state.check(ip("::ffff:10.0.0.1")), Ok(()));
        assert_eq!(state.check(ip("::ffff:10.0.0.2")), Ok(()));
    }

    #[test]
    fn ipv4_mapped_address_shares_a_bucket_with_its_ipv4_address() {
        let (state, _clock) = limiter(1, 60, 100);
        assert_eq!(state.check(ip("::ffff:10.0.0.1")), Ok(()));
        assert!(
            state.check(ip("10.0.0.1")).is_err(),
            "::ffff:10.0.0.1 and 10.0.0.1 are one client"
        );
    }

    #[test]
    fn rate_limit_key_is_the_ipv4_address_or_the_ipv6_64() {
        assert_eq!(
            RateLimitKey::from(ip("2001:db8:0:1::5")),
            RateLimitKey::V6Net64(0x2001_0db8_0000_0001)
        );
        assert_eq!(
            RateLimitKey::from(ip("2001:db8:0:1:ffff:ffff:ffff:ffff")),
            RateLimitKey::from(ip("2001:db8:0:1::"))
        );
        assert_eq!(
            RateLimitKey::from(ip("::ffff:192.0.2.1")),
            RateLimitKey::V4(Ipv4Addr::new(192, 0, 2, 1))
        );
        assert_eq!(RateLimitKey::from(ip("::1")), RateLimitKey::V6Net64(0));
    }

    #[test]
    fn throttled_client_survives_overflow_sweep() {
        // One request per 60 s, and at most 2 clients before requests sweep.
        let (state, clock) = limiter(1, 60, 2);
        let throttled = ip("10.0.0.1");
        assert_eq!(state.check(throttled), Ok(()));
        assert_eq!(state.check(throttled), Err(Duration::from_secs(60)));
        // The third client triggers an early sweep; the rest come within
        // the same refill period, so they don't.
        for i in 2..=6u8 {
            assert_eq!(state.check(IpAddr::from([10, 0, 0, i])), Ok(()));
        }
        // Every client is still active, so the map stays over max_entries.
        assert_eq!(state.len(), 6);

        state.cleanup();
        assert_eq!(
            state.check(throttled),
            Err(Duration::from_secs(60)),
            "the sweep forgot a throttled client"
        );

        clock.advance(Duration::from_secs(59));
        state.cleanup();
        assert_eq!(state.check(throttled), Err(Duration::from_secs(1)));
    }

    #[test]
    fn sweep_drops_only_replenished_clients() {
        // A burst of 2, one request back every 30 s.
        let (state, clock) = limiter(2, 60, 100);
        let light = ip("10.0.0.1"); // one request: TAT 30 s
        let heavy = ip("10.0.0.2"); // the whole burst, then refused: TAT 60 s
        assert_eq!(state.check(light), Ok(()));
        assert_eq!(state.check(heavy), Ok(()));
        assert_eq!(state.check(heavy), Ok(()));
        assert!(state.check(heavy).is_err());

        state.cleanup();
        assert_eq!(state.len(), 2, "clients seen just now are kept");

        // One window later, the light client has been back to a full budget
        // for a whole period; the heavy one only just got there.
        clock.advance(Duration::from_secs(60));
        state.cleanup();
        assert_eq!(state.len(), 1);

        clock.advance(Duration::from_secs(30));
        state.cleanup();
        assert_eq!(state.len(), 0);
        assert!(state.is_empty());
    }

    #[test]
    fn early_sweep_runs_over_max_entries_at_most_once_per_period() {
        // A burst of 10, one request back every 6 s; sweep above 2 clients.
        let (state, clock) = limiter(10, 60, 2);
        assert_eq!(state.check(ip("10.0.0.1")), Ok(())); // TAT 6 s
        clock.advance(Duration::from_secs(49));
        assert_eq!(state.check(ip("10.0.0.2")), Ok(())); // TAT 55 s
        clock.advance(Duration::from_secs(11));

        // t = 60 s: a third client takes the map over the cap, and the sweep
        // forgets 10.0.0.1 (TAT 6 s <= 60 s - 6 s).
        assert_eq!(state.check(ip("10.0.0.3")), Ok(()));
        assert_eq!(state.len(), 2);

        // t = 61 s: over the cap again, and 10.0.0.2 could go (55 s <= 55 s),
        // but a sweep ran less than a period ago.
        clock.advance(Duration::from_secs(1));
        assert_eq!(state.check(ip("10.0.0.4")), Ok(()));
        assert_eq!(state.len(), 3);

        // t = 66 s: a period after the last sweep, the next one runs.
        clock.advance(Duration::from_secs(5));
        assert_eq!(state.check(ip("10.0.0.5")), Ok(()));
        assert_eq!(state.len(), 3);
    }

    #[test]
    fn out_of_range_config_is_clamped_instead_of_panicking() {
        // The service builds the limiter even while rate limiting is off, and
        // then nothing validates these values.
        let (state, _clock) = limiter(0, 0, 0);
        assert_eq!(state.limit, 1);
        assert_eq!(state.check(ip("10.0.0.1")), Ok(()));
        assert_eq!(state.check(ip("10.0.0.1")), Err(Duration::from_secs(1)));

        // A window of centuries would overflow governor's nanosecond clock.
        let (state, _clock) = limiter(1, u64::MAX, 10);
        assert_eq!(state.check(ip("10.0.0.1")), Ok(()));
        assert_eq!(
            state.check(ip("10.0.0.1")),
            Err(Duration::from_secs(86_400))
        );

        for window_secs in [1, u64::MAX] {
            let (state, _clock) = limiter(u32::MAX, window_secs, 10);
            assert_eq!(state.check(ip("10.0.0.1")), Ok(()));
        }
    }

    #[test]
    fn retry_after_is_rounded_up_and_at_least_one() {
        assert_eq!(retry_after_secs(Duration::ZERO), 1);
        assert_eq!(retry_after_secs(Duration::from_millis(1)), 1);
        assert_eq!(retry_after_secs(Duration::from_secs(1)), 1);
        assert_eq!(retry_after_secs(Duration::from_millis(1200)), 2);
        assert_eq!(retry_after_secs(Duration::from_secs(30)), 30);
    }

    /// One route behind `rate_limit`, with its own limiter state.
    fn rate_limited_app(config: SecurityConfig) -> Router {
        let state = Arc::new(RateLimitState::new(&config));
        Router::new()
            .route("/", get(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn_with_state(
                (Arc::new(config), state),
                rate_limit,
            ))
    }

    /// `GET /` from the client address `ip`.
    async fn get_from(app: &Router, ip: &str) -> Response {
        let peer = std::net::SocketAddr::new(ip.parse().unwrap(), 40_000);
        app.clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .extension(ConnectInfo(peer))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn rate_limited_response_has_retry_after_headers_and_body() {
        // A burst of 2, then one request every 30 s.
        let app = rate_limited_app(SecurityConfig {
            rate_limit_enabled: true,
            rate_limit_requests: 2,
            rate_limit_window_secs: 60,
            ..Default::default()
        });
        assert_eq!(get_from(&app, "192.0.2.1").await.status(), StatusCode::OK);
        assert_eq!(get_from(&app, "192.0.2.1").await.status(), StatusCode::OK);

        let limited = get_from(&app, "192.0.2.1").await;
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        let header = |name: &str| limited.headers()[name].to_str().unwrap().to_owned();
        let retry_after: u64 = header("retry-after").parse().unwrap();
        // 30 s minus the microseconds since the first request, rounded up;
        // the range only tolerates a stalled test machine.
        assert!(
            (1..=30).contains(&retry_after),
            "Retry-After {retry_after} s, but one request refills in 30 s"
        );
        assert_eq!(header("x-ratelimit-limit"), "2");
        assert_eq!(header("x-ratelimit-remaining"), "0");
        let body = axum::body::to_bytes(limited.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            body,
            serde_json::json!({"error": "Too many requests", "retry_after": retry_after})
        );
    }

    #[tokio::test]
    async fn disabled_rate_limit_passes_every_request() {
        // Limits that validation would reject are ignored while it is off.
        let app = rate_limited_app(SecurityConfig {
            rate_limit_enabled: false,
            rate_limit_requests: 0,
            rate_limit_window_secs: 0,
            rate_limit_max_entries: 0,
            ..Default::default()
        });
        for _ in 0..3 {
            assert_eq!(get_from(&app, "192.0.2.1").await.status(), StatusCode::OK);
        }
    }

    #[test]
    fn requests_sweep_only_above_max_entries() {
        // A burst of 10, one request back every 6 s; sweep above 2 clients.
        let (state, clock) = limiter(10, 60, 2);
        assert_eq!(state.check(ip("10.0.0.1")), Ok(())); // TAT 6 s
        clock.advance(Duration::from_secs(20));

        // t = 20 s: 10.0.0.1 is idle (6 s <= 20 s - 6 s), but two tracked
        // clients are not more than max_entries, so nothing sweeps.
        assert_eq!(state.check(ip("10.0.0.2")), Ok(()));
        assert_eq!(state.len(), 2, "swept at max_entries, not above it");

        // A third client takes the map over max_entries, and the sweep
        // forgets 10.0.0.1.
        assert_eq!(state.check(ip("10.0.0.3")), Ok(()));
        assert_eq!(state.len(), 2);
    }

    #[test]
    fn refused_request_does_not_sweep() {
        // A burst of 2, one request back every 30 s; sweep above 1 client.
        let (state, clock) = limiter(2, 60, 1);
        assert_eq!(state.check(ip("10.0.0.1")), Ok(())); // TAT 30 s
        clock.advance(Duration::from_secs(31));
        // t = 31 s: a second client takes the map over the cap. The sweep
        // finds nobody idle (30 s > 31 s - 30 s).
        assert_eq!(state.check(ip("10.0.0.2")), Ok(())); // TAT 61 s
        clock.advance(Duration::from_secs(19));
        // t = 50 s: a third client spends its burst. The last sweep was 19 s
        // ago, so these requests don't sweep.
        let throttled = ip("10.0.0.3");
        assert_eq!(state.check(throttled), Ok(())); // TAT 80 s
        assert_eq!(state.check(throttled), Ok(())); // TAT 110 s
        assert_eq!(state.len(), 3);
        clock.advance(Duration::from_secs(11));

        // t = 61 s: over the cap, a period after the last sweep, and
        // 10.0.0.1 is idle (30 s <= 61 s - 30 s). The refused request leaves
        // the sweep ...
        assert_eq!(state.check(throttled), Err(Duration::from_secs(19)));
        assert_eq!(state.len(), 3, "a refused request swept");
        // ... to the next allowed one.
        assert_eq!(state.check(ip("10.0.0.2")), Ok(()));
        assert_eq!(state.len(), 2);
    }

    #[test]
    fn request_does_not_wait_for_another_requests_sweep() {
        // A burst of 10, one request back every 6 s; sweep above 1 client.
        let (state, clock) = limiter(10, 60, 1);
        assert_eq!(state.check(ip("10.0.0.1")), Ok(())); // TAT 6 s
        clock.advance(Duration::from_secs(20)); // 10.0.0.1 is idle now

        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            // Another request is in the middle of a sweep: it holds the lock.
            let sweeping = state.last_sweep.lock().unwrap();
            scope.spawn(|| done_tx.send(state.check(ip("10.0.0.2"))));
            // The limit turns a request that waits for the lock into a
            // failure instead of a hung test.
            let checked = done_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("the request waited for the sweep in progress");
            assert_eq!(checked, Ok(()));
            assert_eq!(state.len(), 2, "a second sweep ran at the same time");
            drop(sweeping);
        });

        // With the lock free again, the next request over the cap sweeps.
        assert_eq!(state.check(ip("10.0.0.3")), Ok(()));
        assert_eq!(state.len(), 2);
    }

    #[tokio::test]
    async fn rate_limit_counts_each_client_address_separately() {
        let app = rate_limited_app(SecurityConfig {
            rate_limit_enabled: true,
            rate_limit_requests: 1,
            rate_limit_window_secs: 60,
            ..Default::default()
        });
        assert_eq!(get_from(&app, "192.0.2.1").await.status(), StatusCode::OK);
        assert_eq!(
            get_from(&app, "192.0.2.1").await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        // Another address has a budget of its own ...
        assert_eq!(get_from(&app, "192.0.2.2").await.status(), StatusCode::OK);
        // ... and the first one's IPv4-mapped form doesn't.
        assert_eq!(
            get_from(&app, "::ffff:192.0.2.1").await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[tokio::test]
    async fn retry_after_header_is_never_zero() {
        // One request per second: a refused request has less than a second
        // left to wait, which rounding down would report as 0.
        let app = rate_limited_app(SecurityConfig {
            rate_limit_enabled: true,
            rate_limit_requests: 1,
            rate_limit_window_secs: 1,
            ..Default::default()
        });
        // Of two requests in a row the second is refused, unless the test
        // machine stalled for a second between them: then the next one is.
        let mut limited = None;
        for _ in 0..20 {
            let response = get_from(&app, "192.0.2.1").await;
            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                limited = Some(response);
                break;
            }
        }
        let limited = limited.expect("none of 20 requests in a row was refused");
        assert_eq!(limited.headers()["retry-after"], "1");
        let body = axum::body::to_bytes(limited.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["retry_after"], 1);
    }

    #[tokio::test]
    async fn rate_limit_header_reports_the_burst_the_limiter_enforces() {
        // Enabled without validation: a `rate_limit_requests` of 0 counts
        // as 1.
        let app = rate_limited_app(SecurityConfig {
            rate_limit_enabled: true,
            rate_limit_requests: 0,
            ..Default::default()
        });
        assert_eq!(get_from(&app, "192.0.2.1").await.status(), StatusCode::OK);

        let limited = get_from(&app, "192.0.2.1").await;
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(limited.headers()["x-ratelimit-limit"], "1");
    }

    #[test]
    fn default_state_has_the_default_configs_limits() {
        let default = RateLimitState::default();
        let from_config = RateLimitState::new(&SecurityConfig::default());
        assert_eq!(default.limit, from_config.limit);
        assert_eq!(default.period, from_config.period);
        assert_eq!(default.max_entries, from_config.max_entries);

        // The documented defaults: 100 requests per 60 s, and requests sweep
        // above 10000 tracked clients.
        assert_eq!(default.limit, 100);
        assert_eq!(default.period, Duration::from_millis(600));
        assert_eq!(default.max_entries, 10_000);
    }

    #[tokio::test]
    async fn request_without_a_peer_address_gets_500_with_rate_limiting_on_or_off() {
        // `run()` gives every request its peer address. One that reaches
        // `rate_limit` without it is refused, never let through unlimited.
        let mut statuses = Vec::new();
        for rate_limit_enabled in [true, false] {
            let app = rate_limited_app(SecurityConfig {
                rate_limit_enabled,
                ..Default::default()
            });
            let request = Request::builder().uri("/").body(Body::empty()).unwrap();
            statuses.push(app.oneshot(request).await.unwrap().status());
        }
        assert_eq!(statuses, [StatusCode::INTERNAL_SERVER_ERROR; 2]);
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

    /// `/api/devices` and `/api/ws` behind the auth middleware with `config`.
    fn auth_router(config: Arc<SecurityConfig>) -> Router {
        Router::new()
            .route("/api/devices", get(|| async { StatusCode::OK }))
            .route("/api/ws", get(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn_with_state(config, api_key_auth))
    }

    /// The status of `GET uri` with `headers`, through [`auth_router`] with
    /// `test_security_config()`'s key. The headers are sent in the order
    /// given, a name given twice is sent twice, and a value may hold bytes
    /// that aren't text.
    async fn auth_status_bytes(uri: &str, headers: &[(&str, &[u8])]) -> StatusCode {
        let mut request = Request::builder().uri(uri);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        auth_router(test_security_config())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    /// [`auth_status_bytes`] for header values that are text.
    async fn auth_status(uri: &str, headers: &[(&str, &str)]) -> StatusCode {
        let headers: Vec<(&str, &[u8])> = headers
            .iter()
            .map(|(name, value)| (*name, value.as_bytes()))
            .collect();
        auth_status_bytes(uri, &headers).await
    }

    /// One request of a table test: the case's name, the URI, the headers in
    /// the order they are sent and the status the request must get.
    type AuthCase<'a> = (&'a str, &'a str, &'a [(&'a str, &'a [u8])], StatusCode);

    /// Sends every case and fails with the name and the status of each one
    /// that got another status, never with a header value or a URI: either
    /// may hold the key.
    async fn assert_auth_cases(cases: &[AuthCase<'_>]) {
        let mut wrong = Vec::new();
        for (case, uri, headers, expected) in cases {
            let status = auth_status_bytes(uri, headers).await;
            if status != *expected {
                wrong.push(format!("{case}: got {status}, expected {expected}"));
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
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
            let request = Request::builder()
                .uri(&uri)
                .header("Authorization", unreadable.as_slice())
                .body(Body::empty())
                .unwrap();
            let status = auth_router(test_security_config())
                .oneshot(request)
                .await
                .unwrap()
                .status();
            // The path only: the query holds the key.
            let path = uri.split('?').next().unwrap_or_default();
            assert_eq!(status, expected, "{path}");
        }
    }

    /// `X-API-Key` carries a key only when it is sent once and reads as text.
    /// Whenever the header is present, nothing else is tried.
    #[tokio::test]
    async fn test_x_api_key_counts_only_when_sent_once_as_text() {
        const OK: StatusCode = StatusCode::OK;
        const REFUSED: StatusCode = StatusCode::UNAUTHORIZED;
        let key = CONFIGURED_KEY.as_bytes();
        let wrong: &[u8] = b"wrong-key";
        // 0xE9 is not visible ASCII, so `HeaderValue::to_str` fails.
        let unreadable: &[u8] = b"wr\xe9ng";
        let bearer = format!("Bearer {CONFIGURED_KEY}");
        let bearer = bearer.as_bytes();
        let devices = "/api/devices";
        let ws = format!("/api/ws?token={CONFIGURED_KEY}");

        assert_auth_cases(&[
            ("the key, once", devices, &[("X-API-Key", key)], OK),
            (
                "a wrong key beside a right bearer token",
                devices,
                &[("X-API-Key", wrong), ("Authorization", bearer)],
                REFUSED,
            ),
            (
                "an unreadable value",
                devices,
                &[("X-API-Key", unreadable)],
                REFUSED,
            ),
            (
                "an unreadable value beside a right bearer token",
                devices,
                &[("X-API-Key", unreadable), ("Authorization", bearer)],
                REFUSED,
            ),
            (
                "an unreadable value beside a right query token on /api/ws",
                &ws,
                &[("X-API-Key", unreadable)],
                REFUSED,
            ),
            (
                "the key, then a wrong key",
                devices,
                &[("X-API-Key", key), ("X-API-Key", wrong)],
                REFUSED,
            ),
            (
                "a wrong key, then the key",
                devices,
                &[("X-API-Key", wrong), ("X-API-Key", key)],
                REFUSED,
            ),
            (
                "the key, twice",
                devices,
                &[("X-API-Key", key), ("X-API-Key", key)],
                REFUSED,
            ),
            (
                "the key, then an unreadable value",
                devices,
                &[("X-API-Key", key), ("X-API-Key", unreadable)],
                REFUSED,
            ),
            (
                "an unreadable value, then the key",
                devices,
                &[("X-API-Key", unreadable), ("X-API-Key", key)],
                REFUSED,
            ),
            (
                "two wrong keys beside a right bearer token",
                devices,
                &[
                    ("X-API-Key", wrong),
                    ("X-API-Key", wrong),
                    ("Authorization", bearer),
                ],
                REFUSED,
            ),
            (
                "two wrong keys beside a right query token on /api/ws",
                &ws,
                &[("X-API-Key", wrong), ("X-API-Key", wrong)],
                REFUSED,
            ),
        ])
        .await;
    }

    /// A header value outside visible ASCII is never compared, even when it
    /// is the configured key byte for byte: such a key can be sent only as
    /// the `/api/ws` query token.
    #[tokio::test]
    async fn test_header_value_outside_visible_ascii_is_never_compared() {
        // Valid UTF-8, so a lossy conversion of the header would give the key.
        let key = "cl\u{e9}-567890abcdef1234567890abcdef";
        let config = Arc::new(SecurityConfig {
            api_key_enabled: true,
            api_key: Some(key.to_string()),
            rate_limit_enabled: false,
            ..Default::default()
        });
        let bearer = format!("Bearer {key}");
        for (case, uri, header, expected) in [
            (
                "X-API-Key",
                "/api/devices",
                Some(("X-API-Key", key.as_bytes())),
                StatusCode::UNAUTHORIZED,
            ),
            (
                "Authorization",
                "/api/devices",
                Some(("Authorization", bearer.as_bytes())),
                StatusCode::UNAUTHORIZED,
            ),
            // The key itself can be matched: the headers are what is refused.
            (
                "the query token",
                "/api/ws?token=cl%C3%A9-567890abcdef1234567890abcdef",
                None,
                StatusCode::OK,
            ),
        ] {
            let mut request = Request::builder().uri(uri);
            if let Some((name, value)) = header {
                request = request.header(name, value);
            }
            let status = auth_router(config.clone())
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status();
            assert_eq!(status, expected, "{case}");
        }
    }

    /// A request with more than one `Authorization` header carries no bearer
    /// token, whatever their order or schemes. The `/api/ws` query token and
    /// `X-API-Key` still count beside them.
    #[tokio::test]
    async fn test_repeated_authorization_header_carries_no_token() {
        const OK: StatusCode = StatusCode::OK;
        const REFUSED: StatusCode = StatusCode::UNAUTHORIZED;
        let key = CONFIGURED_KEY.as_bytes();
        let bearer = format!("Bearer {CONFIGURED_KEY}");
        let bearer = bearer.as_bytes();
        let other: &[u8] = b"Bearer proxy-token";
        let basic: &[u8] = b"Basic dXNlcjpwYXNz";
        let devices = "/api/devices";
        let ws = format!("/api/ws?token={CONFIGURED_KEY}");

        assert_auth_cases(&[
            (
                "the right bearer token, once",
                devices,
                &[("Authorization", bearer)],
                OK,
            ),
            (
                "another bearer token, then the right one",
                devices,
                &[("Authorization", other), ("Authorization", bearer)],
                REFUSED,
            ),
            (
                "the right bearer token, then another",
                devices,
                &[("Authorization", bearer), ("Authorization", other)],
                REFUSED,
            ),
            (
                "the right bearer token, twice",
                devices,
                &[("Authorization", bearer), ("Authorization", bearer)],
                REFUSED,
            ),
            (
                "Basic, then the right bearer token",
                devices,
                &[("Authorization", basic), ("Authorization", bearer)],
                REFUSED,
            ),
            (
                "the right bearer token, then Basic",
                devices,
                &[("Authorization", bearer), ("Authorization", basic)],
                REFUSED,
            ),
            // A proxy's own headers still can't hide the dashboard's token.
            (
                "two headers beside a right query token on /api/ws",
                &ws,
                &[("Authorization", other), ("Authorization", basic)],
                OK,
            ),
            (
                "the right bearer token, then another, beside a wrong query token on /api/ws",
                "/api/ws?token=wrong-key",
                &[("Authorization", bearer), ("Authorization", other)],
                REFUSED,
            ),
            (
                "two headers beside a right X-API-Key",
                devices,
                &[
                    ("Authorization", other),
                    ("Authorization", basic),
                    ("X-API-Key", key),
                ],
                OK,
            ),
        ])
        .await;
    }

    /// The whole hint of a 401, which names every way to send the key.
    /// `run()` never serves with an empty key: loading the configuration
    /// replaces it with a generated one, and validation wants 32 characters.
    /// But `app()` and `api_key_auth` are public and take a config that was
    /// never validated, and two empty strings compare equal.
    #[tokio::test]
    async fn test_empty_configured_key_authenticates_nothing() {
        for (configured, api_key) in [("an empty", Some(String::new())), ("no", None)] {
            let config = Arc::new(SecurityConfig {
                api_key_enabled: true,
                api_key,
                ..Default::default()
            });
            let requests: [(&str, &[(&str, &str)]); 5] = [
                // `curl -H "X-API-Key;"` sends the header with an empty value.
                ("/api/devices", &[("X-API-Key", "")]),
                ("/api/ws?token=", &[]),
                ("/api/ws?token", &[]),
                ("/api/devices", &[("Authorization", "Bearer ")]),
                ("/api/devices", &[]),
            ];
            for (uri, headers) in requests {
                let mut request = Request::builder().uri(uri);
                for (name, value) in headers {
                    request = request.header(*name, *value);
                }
                let status = auth_router(Arc::clone(&config))
                    .oneshot(request.body(Body::empty()).unwrap())
                    .await
                    .unwrap()
                    .status();
                assert_eq!(
                    status,
                    StatusCode::UNAUTHORIZED,
                    "GET {uri} with {headers:?}, with {configured} key configured"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_unauthorized_hint_names_every_way_to_send_the_key() {
        let request = Request::builder()
            .uri("/api/devices")
            .body(Body::empty())
            .unwrap();

        let response = auth_router(test_security_config())
            .oneshot(request)
            .await
            .unwrap();

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
