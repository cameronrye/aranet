#![deny(unsafe_code)]

//! Background collector and HTTP REST API for Aranet sensors.
//!
//! This crate provides a service that:
//! - Polls configured Aranet devices on a schedule
//! - Stores readings in the local database
//! - Exposes a REST API for querying data
//! - Provides WebSocket connections for real-time updates
//! - Optional API key authentication and rate limiting
//!
//! # REST API Endpoints
//!
//! - `GET /api/health` - Lightweight service health check (no auth required)
//! - `GET /api/health/detailed` - Database, collector, and platform diagnostics
//! - `GET /api/status` - Collector status plus per-device polling statistics
//! - `GET /api/devices` - List all known devices
//! - `GET /api/devices/current` - Latest reading for every known device
//! - `GET /api/devices/:id` - Get device info
//! - `GET /api/devices/:id/current` - Latest reading wrapped in `CurrentReadingResponse`
//! - `GET /api/devices/:id/readings` - Query readings with filters
//! - `GET /api/devices/:id/history` - Query cached history
//! - `GET /api/readings` - All readings across devices
//! - `GET /api/config`, `PUT /api/config` - Read or update runtime configuration
//! - `POST /api/config/devices`, `PUT/DELETE /api/config/devices/:id` - Manage monitored devices
//! - `POST /api/collector/start`, `POST /api/collector/stop` - Control the background collector
//! - `GET /metrics` - Prometheus metrics export
//! - `WS /api/ws` - Real-time readings stream
//! - `GET /`, `GET /dashboard` - Embedded dashboard shell
//!
//! The readings and history endpoints return one page, newest first: 1000 rows
//! unless `limit` asks for 1 to 10000. `pagination.has_more` says whether
//! another page exists. Fetch it with `offset` (at most 1000000), or narrow the
//! range with `since` and `until`.
//!
//! # Configuration
//!
//! The service reads configuration from `~/.config/aranet/server.toml`:
//!
//! ```toml
//! [server]
//! bind = "127.0.0.1:8080"
//!
//! [storage]
//! path = "~/.local/share/aranet/data.db"
//!
//! [[devices]]
//! address = "Aranet4 17C3C"
//! alias = "office"
//! poll_interval = 60
//! ```
//!
//! # Security
//!
//! Optional security features can be enabled:
//!
//! ```toml
//! [security]
//! # Require X-API-Key header for protected API, WebSocket, and metrics requests
//! api_key_enabled = true
//! api_key = "your-secure-random-key-at-least-32-chars"
//!
//! # Rate limit requests per IP address
//! rate_limit_enabled = true
//! rate_limit_requests = 100   # max requests per window
//! rate_limit_window_secs = 60 # window duration
//! ```
//!
//! The dashboard shell routes (`/` and `/dashboard`) remain public so browsers can
//! load the UI. Protected API, metrics, and WebSocket requests still honor the
//! configured security settings.
//!
//! For WebSocket connections, browsers cannot set custom headers. Use the `token`
//! query parameter only on `/api/ws` instead, URL-encoding the key (for example
//! with `encodeURIComponent(key)` in JavaScript):
//! `ws://localhost:8080/api/ws?token=your-api-key`
//!
//! A key sent unencoded is also accepted: `+` in the token is kept as `+`, not
//! read as a space, so base64 keys containing `+`, `/` or `=` work either way.
//!
//! **Note**: Query parameters may be logged by proxies or appear in browser history.
//! For sensitive deployments, consider using a short-lived token exchange endpoint
//! rather than passing the API key directly in the query string.
//!
//! # Platform Setup
//!
//! ## macOS
//!
//! ### Bluetooth Permissions
//!
//! The Aranet devices use Bluetooth Low Energy. On macOS, you need to grant
//! Bluetooth permissions:
//!
//! 1. **Terminal App**: When running from Terminal, the Terminal app must have
//!    Bluetooth permission in System Preferences > Privacy & Security > Bluetooth.
//!
//! 2. **VS Code Terminal**: Add VS Code to the Bluetooth permissions list.
//!
//! 3. **LaunchAgent**: For background services, add `aranet-service` to the
//!    Bluetooth permission list. You may need to use a signed binary or run
//!    with appropriate entitlements.
//!
//! ### User-Level Service (Recommended)
//!
//! Install as a user LaunchAgent (no root required):
//!
//! ```bash
//! # Install the service
//! aranet-service service install --user
//!
//! # Start the service
//! aranet-service service start --user
//!
//! # Check status
//! aranet-service service status --user
//!
//! # Stop and uninstall
//! aranet-service service stop --user
//! aranet-service service uninstall --user
//! ```
//!
//! The LaunchAgent plist is created at `~/Library/LaunchAgents/dev.rye.aranet.plist`.
//!
//! ## Linux
//!
//! ### BlueZ D-Bus Access
//!
//! The service needs access to the BlueZ D-Bus interface. For user-level services:
//!
//! 1. **Ensure your user is in the bluetooth group:**
//!    ```bash
//!    sudo usermod -a -G bluetooth $USER
//!    # Log out and back in for group changes to take effect
//!    ```
//!
//! 2. **D-Bus session access**: User-level systemd services need the D-Bus session.
//!    Create a drop-in config if needed:
//!    ```bash
//!    mkdir -p ~/.config/systemd/user/aranet.service.d
//!    cat > ~/.config/systemd/user/aranet.service.d/dbus.conf << EOF
//!    [Service]
//!    Environment="DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/%U/bus"
//!    EOF
//!    systemctl --user daemon-reload
//!    ```
//!
//! ### User-Level Service
//!
//! ```bash
//! # Install the user service
//! aranet-service service install --user
//!
//! # Enable and start
//! systemctl --user enable --now dev.rye.aranet
//!
//! # Check status
//! systemctl --user status dev.rye.aranet
//!
//! # View logs
//! journalctl --user -u dev.rye.aranet -f
//!
//! # Stop and uninstall
//! systemctl --user stop dev.rye.aranet
//! aranet-service service uninstall --user
//! ```
//!
//! ### System-Level Service
//!
//! For system services, you need to create a dedicated user:
//!
//! ```bash
//! # Create aranet user (with bluetooth group membership)
//! sudo useradd -r -s /sbin/nologin -G bluetooth aranet
//!
//! # Install as system service
//! sudo aranet-service service install
//!
//! # Start the service
//! sudo systemctl enable --now dev.rye.aranet
//! ```
//!
//! ## Windows
//!
//! ### Bluetooth Permissions
//!
//! Windows requires the app to be granted Bluetooth access through Settings:
//! - Settings > Privacy & Security > Bluetooth > Allow apps to access your Bluetooth
//!
//! ### Running as a Service
//!
//! On Windows, the service runs as a Windows Service. Install and manage via:
//!
//! ```powershell
//! # Run as Administrator
//! aranet-service service install
//! aranet-service service start
//!
//! # Check status (in Services panel or via)
//! aranet-service service status
//!
//! # Stop and uninstall
//! aranet-service service stop
//! aranet-service service uninstall
//! ```
//!
//! **Note**: Windows Services run in session 0 without a desktop, which may affect
//! Bluetooth access. Consider using Task Scheduler to run the service at logon
//! if you encounter Bluetooth issues:
//!
//! ```powershell
//! # Create a scheduled task to run at logon
//! schtasks /create /tn "AranetService" /tr "aranet-service run" /sc onlogon /rl highest
//! ```

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use tokio::task::JoinHandle;
use tower_http::trace::TraceLayer;

pub mod api;
pub mod collector;
pub mod config;
pub mod dashboard;
mod db;
pub mod middleware;
pub mod state;
pub mod ws;

#[cfg(test)]
mod test_support;

pub use collector::Collector;
pub use config::{
    Config, ConfigError, DeviceConfig, InfluxDbConfig, MqttConfig, NotificationConfig,
    PrometheusConfig, SecurityConfig, ServerConfig, StorageConfig, WebhookConfig, WebhookEndpoint,
};
pub use state::{AppState, ReadingEvent};

#[cfg(feature = "mqtt")]
pub mod mqtt;

#[cfg(feature = "prometheus")]
pub mod prometheus;

pub mod influxdb;
pub mod mdns;
pub mod webhook;

/// Runtime options for starting the HTTP service.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Optional path to a `server.toml` file.
    pub config: Option<PathBuf>,
    /// Optional bind address override.
    pub bind: Option<String>,
    /// Optional database path override.
    pub database: Option<PathBuf>,
    /// Disable the background collector.
    pub no_collector: bool,
}

/// Default log directives for the service binaries: request/response tracing at
/// debug, everything else in this crate at info. Shared by `init_tracing()` and
/// its tests so the tests exercise the same filter production runs under.
const DEFAULT_LOG_DIRECTIVES: &str = "aranet_service=info,tower_http=debug";

/// How long shutdown waits for background tasks after signalling them to
/// stop, before aborting the ones left.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// Initialize the default tracing subscriber used by the service binaries.
pub fn init_tracing() -> anyhow::Result<()> {
    let filter = with_default_directives(tracing_subscriber::EnvFilter::from_default_env())?;
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
    Ok(())
}

/// Add [`DEFAULT_LOG_DIRECTIVES`] to `filter`, which `init_tracing()` builds
/// from `RUST_LOG`.
///
/// `EnvFilter::add_directive` parses a single directive, not a comma-separated
/// list (only `EnvFilter::new`/`from_str` split on commas), so the defaults are
/// split and each directive added in turn.
fn with_default_directives(
    mut filter: tracing_subscriber::EnvFilter,
) -> anyhow::Result<tracing_subscriber::EnvFilter> {
    for directive in DEFAULT_LOG_DIRECTIVES.split(',') {
        filter = filter.add_directive(directive.parse()?);
    }
    Ok(filter)
}

/// Request span that records the path only. The default span records the full
/// URI, which would write `/api/ws?token=<API key>` into the service logs.
///
/// The target is set to `tower_http::trace::make_span` — the target
/// `TraceLayer`'s own default span uses — so the `tower_http=debug` directive in
/// [`DEFAULT_LOG_DIRECTIVES`] still enables it. Without an explicit target this
/// span's target would be `aranet_service` (this module's path), which
/// `aranet_service=info` disables at DEBUG, silently dropping the method/path
/// context from production request logs.
fn request_span(request: &axum::http::Request<axum::body::Body>) -> tracing::Span {
    tracing::debug_span!(
        target: "tower_http::trace::make_span",
        "request",
        method = %request.method(),
        path = %request.uri().path(),
        version = ?request.version(),
    )
}

/// Build the fully layered HTTP application used in production and end-to-end tests.
pub fn app(
    state: Arc<AppState>,
    security_config: Arc<SecurityConfig>,
    rate_limit_state: Arc<middleware::RateLimitState>,
) -> Router {
    Router::new()
        .merge(api::router())
        .merge(ws::router())
        .merge(dashboard::router())
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&security_config),
            middleware::api_key_auth,
        ))
        .layer(axum::middleware::from_fn_with_state(
            (security_config, rate_limit_state),
            middleware::rate_limit,
        ))
        .layer(TraceLayer::new_for_http().make_span_with(request_span))
        .with_state(state)
}

/// Run the HTTP service until shutdown.
pub async fn run(options: RunOptions) -> anyhow::Result<()> {
    let config_path = options
        .config
        .clone()
        .unwrap_or_else(config::default_config_path);

    let mut config = if config_path.exists() {
        Config::load(&config_path)?
    } else {
        Config::default()
    };

    if let Some(bind) = options.bind {
        config.server.bind = bind;
    }
    if let Some(db_path) = options.database {
        config.storage.path = db_path;
    }

    config.validate()?;

    tracing::info!("Opening database at {:?}", config.storage.path);
    // Opening creates the file and migrates the schema, and can wait for
    // another process's lock: like every database call, it runs on a
    // blocking thread.
    let db_path = config.storage.path.clone();
    let store = db::run_blocking(move || aranet_store::Store::open(db_path)).await?;
    let state = AppState::with_config_path(store, config.clone(), config_path);

    let security_config = Arc::new(config.security.clone());
    let rate_limit_state = Arc::new(middleware::RateLimitState::new());

    {
        let rate_limit_state = Arc::clone(&rate_limit_state);
        let window_secs = config.security.rate_limit_window_secs;
        let max_entries = config.security.rate_limit_max_entries;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            loop {
                interval.tick().await;
                rate_limit_state.cleanup(window_secs, max_entries).await;
            }
        });
    }

    // Background tasks that stop on `AppState::signal_shutdown`; shutdown
    // waits for them.
    #[cfg_attr(not(feature = "prometheus"), allow(unused_mut))]
    let mut background: Vec<JoinHandle<()>> = Vec::new();

    if !options.no_collector {
        Collector::new(Arc::clone(&state)).start().await;
    } else {
        tracing::info!("Background collector disabled");
    }

    #[cfg(feature = "mqtt")]
    {
        use crate::mqtt::MqttPublisher;
        let mqtt_publisher = MqttPublisher::new(Arc::clone(&state));
        mqtt_publisher.start().await;
    }

    #[cfg(feature = "prometheus")]
    {
        use crate::prometheus::PrometheusPusher;
        let prometheus_pusher = PrometheusPusher::new(Arc::clone(&state));
        if let Some(handle) = prometheus_pusher.start().await {
            background.push(handle);
        }
    }

    {
        use crate::webhook::WebhookDispatcher;
        let webhook_dispatcher = WebhookDispatcher::new(Arc::clone(&state));
        webhook_dispatcher.start().await;
    }

    {
        use crate::influxdb::InfluxDbWriter;
        let influxdb_writer = InfluxDbWriter::new(Arc::clone(&state));
        influxdb_writer.start().await;
    }

    let _mdns_handle = {
        use crate::mdns::MdnsAdvertiser;
        let advertiser = MdnsAdvertiser::new(Arc::clone(&state));
        advertiser.start().await
    };

    let app = app(
        Arc::clone(&state),
        Arc::clone(&security_config),
        Arc::clone(&rate_limit_state),
    )
    .layer(middleware::cors_layer(&config.security));

    let addr: SocketAddr = config.server.bind.parse()?;
    tracing::info!("Starting server on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(Arc::clone(&state), background))
    .await?;

    Ok(())
}

/// Wait for shutdown signal and perform cleanup.
async fn shutdown_signal(state: Arc<AppState>, background: Vec<JoinHandle<()>>) {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!("Failed to install Ctrl+C handler: {}", e);
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(e) => {
                tracing::error!("Failed to install SIGTERM handler: {}", e);
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("Shutdown signal received, stopping services...");

    stop_background_work(&state, background).await;

    tracing::info!("Graceful shutdown complete");
}

/// Stop the collector for good, whether it was started at startup or through
/// the API, then signal every background task to stop and wait for them.
///
/// A collector start that arrives after this began (the server keeps serving
/// until shutdown returns) starts nothing. Bluetooth polls still in flight get
/// `POLL_DRAIN_TIMEOUT` to disconnect and store their reading. `background`
/// gets `SHUTDOWN_GRACE`, after which the tasks left are aborted.
pub(crate) async fn stop_background_work(state: &Arc<AppState>, background: Vec<JoinHandle<()>>) {
    Collector::new(Arc::clone(state)).shutdown().await;
    state.signal_shutdown();

    state.collector.polls.close();
    if tokio::time::timeout(collector::POLL_DRAIN_TIMEOUT, state.collector.polls.wait())
        .await
        .is_err()
    {
        tracing::info!(
            "Bluetooth polls still running after {}s; not waiting for them",
            collector::POLL_DRAIN_TIMEOUT.as_secs()
        );
    }

    collector::join_or_abort("background task", background, SHUTDOWN_GRACE).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use tower::ServiceExt;

    use crate::test_support::{FakeReader, TestState};

    #[derive(Clone, Default)]
    struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_request_span_omits_query_string() {
        let captured = Captured::default();
        let writer = captured.clone();
        // Use the same directives init_tracing() applies in production (rather
        // than RUST_LOG or a blanket max level), so this test catches a span
        // whose target the production filter would silently disable.
        let filter = tracing_subscriber::EnvFilter::new(DEFAULT_LOG_DIRECTIVES);
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let app = Router::new()
            .route("/api/ws", get(|| async { "ok" }))
            .layer(TraceLayer::new_for_http().make_span_with(request_span));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/ws?token=supersecret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        assert!(logs.contains("/api/ws"), "request was not logged: {logs}");
        assert!(
            !logs.contains("supersecret"),
            "query string leaked into logs: {logs}"
        );
    }

    #[test]
    fn test_init_tracing_builds_valid_filter() {
        // Regression test for `DEFAULT_LOG_DIRECTIVES` being fed whole into
        // `EnvFilter::add_directive`, which parses a single directive and errors
        // on the comma-separated list, making every call fail at runtime.
        // Tests the filter init_tracing() builds rather than calling it, which
        // would install a process-wide subscriber printing to stdout for every
        // later test in this binary (and would depend on RUST_LOG).
        let filter = with_default_directives(tracing_subscriber::EnvFilter::default())
            .expect("init_tracing() failed to build its EnvFilter");
        let directives = filter.to_string();
        assert!(
            directives.contains("aranet_service=info") && directives.contains("tower_http=debug"),
            "default directives missing from filter: {directives}"
        );
    }

    /// A config that monitors one device every 60 s.
    fn one_device_config() -> Config {
        Config {
            devices: vec![DeviceConfig {
                address: "Aranet4 AAAAA".to_string(),
                alias: None,
                poll_interval: 60,
            }],
            ..Config::default()
        }
    }

    /// `POST /api/collector/start` through the API router.
    async fn post_collector_start(state: &Arc<AppState>) -> StatusCode {
        api::router()
            .with_state(Arc::clone(state))
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/collector/start")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_stops_collector_started_through_api() {
        let reader = FakeReader::new();
        let t = TestState::with_reader(one_device_config(), reader.clone());
        assert_eq!(post_collector_start(&t.state).await, StatusCode::OK);
        reader.wait_for_reads(1).await;

        stop_background_work(&t.state, Vec::new()).await;

        assert!(!t.state.collector.is_running());
        assert!(*t.state.subscribe_shutdown().borrow());
        tokio::time::sleep(Duration::from_secs(600)).await;
        assert_eq!(
            reader.reads().len(),
            1,
            "the collector kept polling after shutdown"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn collector_start_during_shutdown_starts_nothing() {
        let reader = FakeReader::new();
        let t = TestState::with_reader(one_device_config(), reader.clone());
        // A background task that ignores the signal keeps the shutdown busy
        // for `SHUTDOWN_GRACE`, as a slow sink would. The server still serves
        // requests meanwhile.
        let stuck = tokio::spawn(std::future::pending::<()>());
        let shutdown = tokio::spawn({
            let state = Arc::clone(&t.state);
            async move { stop_background_work(&state, vec![stuck]).await }
        });
        let mut signalled = t.state.subscribe_shutdown();
        signalled.wait_for(|down| *down).await.unwrap();

        assert_eq!(post_collector_start(&t.state).await, StatusCode::OK);
        assert!(
            !shutdown.is_finished(),
            "the start was handled after the shutdown, not during it"
        );
        assert!(
            !t.state.collector.is_running(),
            "a start during shutdown started the collector"
        );

        shutdown.await.unwrap();
        tokio::time::sleep(Duration::from_secs(600)).await;
        assert!(
            reader.reads().is_empty(),
            "a collector started during shutdown polled"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stop_background_work_signals_then_aborts_stragglers() {
        let t = TestState::with_reader(Config::default(), FakeReader::new());
        let mut shutdown = t.state.subscribe_shutdown();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
        let prompt = tokio::spawn(async move {
            let _ = shutdown.changed().await;
            // Reached only if the signal comes before shutdown gives up on this task.
            let _ = done_tx.send(());
        });
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let stuck = tokio::spawn(async move {
            let _tx = tx;
            std::future::pending::<()>().await;
        });

        let begun = tokio::time::Instant::now();
        stop_background_work(&t.state, vec![prompt, stuck]).await;

        let waited = begun.elapsed();
        assert!(
            waited >= SHUTDOWN_GRACE && waited <= SHUTDOWN_GRACE + Duration::from_secs(1),
            "waited {waited:?}"
        );
        assert!(
            done_rx.await.is_ok(),
            "the task that stops on the shutdown signal was aborted: the signal came after the join"
        );
        assert!(rx.await.is_err(), "the stuck task was not aborted");
    }

    #[tokio::test(start_paused = true)]
    async fn stop_background_work_drains_polls_in_flight() {
        let reader = FakeReader::new();
        reader.hold("Aranet4 AAAAA");
        let t = TestState::with_reader(one_device_config(), reader.clone());
        assert_eq!(post_collector_start(&t.state).await, StatusCode::OK);
        reader.wait_for_reads(1).await;

        let begun = tokio::time::Instant::now();
        let shutdown = tokio::spawn({
            let state = Arc::clone(&t.state);
            async move { stop_background_work(&state, Vec::new()).await }
        });
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(
            !shutdown.is_finished(),
            "shutdown did not wait for the Bluetooth poll in flight"
        );

        reader.release("Aranet4 AAAAA");
        shutdown.await.unwrap();
        let waited = begun.elapsed();
        assert!(
            waited < collector::POLL_DRAIN_TIMEOUT,
            "shutdown returned {waited:?} after it began, not when the poll finished"
        );
        let reads = reader.reads();
        assert!(
            reads[0].finished.is_some() && reads[0].ok,
            "shutdown cancelled the poll mid-read"
        );
        let stored = t
            .state
            .with_store_read(|store| store.count_readings(Some("Aranet4 AAAAA")))
            .await
            .unwrap();
        assert_eq!(stored, 1, "the drained poll did not store its reading");
    }

    #[tokio::test(start_paused = true)]
    async fn stop_background_work_gives_up_on_a_hung_poll() {
        let reader = FakeReader::new();
        // Never released: the poll outlives the drain.
        reader.hold("Aranet4 AAAAA");
        let t = TestState::with_reader(one_device_config(), reader.clone());
        assert_eq!(post_collector_start(&t.state).await, StatusCode::OK);
        reader.wait_for_reads(1).await;

        let begun = tokio::time::Instant::now();
        stop_background_work(&t.state, Vec::new()).await;

        let waited = begun.elapsed();
        assert!(
            waited >= collector::POLL_DRAIN_TIMEOUT
                && waited <= collector::POLL_DRAIN_TIMEOUT + Duration::from_secs(1),
            "waited {waited:?} for a poll that never ends"
        );
    }
}
