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
//! # An IP address and port; host names such as "localhost" are rejected.
//! bind = "127.0.0.1:8080"
//!
//! [storage]
//! # A leading "~" is expanded to the home directory.
//! path = "~/.local/share/aranet/data.db"
//!
//! [[devices]]
//! address = "Aranet4 17C3C"
//! alias = "office"
//! poll_interval = 60
//! ```
//!
//! Readings and cached history are kept until deleted by hand. To make the
//! service delete older rows at startup and then every hour, set retention
//! limits in days, and restart the service after changing them:
//!
//! ```toml
//! [storage.retention]
//! readings_days = 365 # readings the service polled
//! history_days = 730  # history cached by `aranet sync`, shared with the CLI
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
//! # At least 32 characters. Leave it out and the service generates a key when
//! # it starts and saves it here.
//! api_key = "your-secure-random-key-at-least-32-chars"
//!
//! # Rate limit requests per IP address
//! rate_limit_enabled = true
//! rate_limit_requests = 100   # max requests per window
//! rate_limit_window_secs = 60 # window duration
//! ```
//!
//! The service refuses to listen on an address other than loopback (such as
//! `0.0.0.0` or a LAN address) while `api_key_enabled` is false. Set
//! `allow_unauthenticated_network = true` under `[security]` only when a
//! firewall or an authenticating reverse proxy protects the port; the service
//! then logs a warning at startup.
//!
//! The dashboard shell routes (`/` and `/dashboard`) remain public so browsers can
//! load the UI. Protected API, metrics, and WebSocket requests still honor the
//! configured security settings.
//!
//! While API-key authentication is off, the service also checks where each
//! request comes from, so that other web pages open in a browser can't use it
//! (DNS rebinding and cross-site requests):
//!
//! - A `Host` other than `localhost`, an IP address, this machine's name,
//!   `<name>.local` or a name in `allowed_hosts` gets `421 Misdirected Request`.
//! - A request whose `Origin` is neither the service's own nor listed in
//!   `cors_origins` gets `403 Forbidden`, WebSocket connections to `/api/ws`
//!   included. Requests without `Origin`, such as curl's, are not affected.
//!
//! ```toml
//! [security]
//! allowed_hosts = ["aranet-service"]       # for example a Docker Compose service name
//! cors_origins = ["http://localhost:3000"] # pages served elsewhere that call the API
//! ```
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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::Router;
use tokio::task::JoinHandle;
use tower_http::trace::TraceLayer;

pub mod api;
pub mod collector;
pub mod config;
pub mod dashboard;
mod db;
pub mod middleware;
mod redact;
mod retention;
pub mod state;
pub mod ws;

#[cfg(test)]
mod test_support;

pub use collector::Collector;
pub use config::{
    Config, ConfigError, DeviceConfig, InfluxDbConfig, MqttConfig, NotificationConfig,
    PrometheusConfig, RetentionConfig, SecurityConfig, ServerConfig, StorageConfig, WebhookConfig,
    WebhookEndpoint,
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

/// A validated service configuration and the file it came from, as returned by
/// [`load_config`].
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    /// The configuration, with the command-line overrides applied.
    pub config: Config,
    /// The config file: [`RunOptions::config`], or
    /// [`config::default_config_path`]. It need not exist. The service saves
    /// device changes made through the API here.
    pub path: PathBuf,
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
///
/// Requests pass through, outermost first: CORS (which answers preflight
/// requests for the origins in `security.cors_origins`), request tracing, rate
/// limiting, the Host and Origin checks that apply while API-key
/// authentication is off ([`middleware::origin_guard`]), and API-key
/// authentication.
pub fn app(
    state: Arc<AppState>,
    security_config: Arc<SecurityConfig>,
    rate_limit_state: Arc<middleware::RateLimitState>,
) -> Router {
    let origin_policy = Arc::new(middleware::OriginPolicy::new(
        &security_config,
        mdns::machine_hostname().as_deref(),
    ));
    let cors = middleware::cors_layer(&security_config);
    Router::new()
        .merge(api::router())
        .merge(ws::router())
        .merge(dashboard::router())
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&security_config),
            middleware::api_key_auth,
        ))
        .layer(axum::middleware::from_fn_with_state(
            origin_policy,
            middleware::origin_guard,
        ))
        .layer(axum::middleware::from_fn_with_state(
            (security_config, rate_limit_state),
            middleware::rate_limit,
        ))
        .layer(TraceLayer::new_for_http().make_span_with(request_span))
        .layer(cors)
        .with_state(state)
}

/// Load the service configuration: read the config file, apply the
/// command-line overrides, expand `~` in `storage.path` and validate the
/// result.
///
/// The file is [`RunOptions::config`], or [`config::default_config_path`] when
/// that is `None`. A file that doesn't exist gives the defaults; a file that
/// exists but can't be read, or whose directory can't be searched, is an
/// error. [`RunOptions::bind`] and [`RunOptions::database`] replace
/// `server.bind` and `storage.path`. A leading `~` in `storage.path` becomes
/// the home directory. The overrides and the expanded path exist only in the
/// returned config; they are never written to the file.
///
/// If `security.api_key_enabled` is true and `security.api_key` is missing or
/// empty, a key is generated with [`config::generate_api_key`] and, once the
/// configuration is valid, only that key is saved to the file: owner-only on
/// Unix (mode 0600), with the file's comments and other settings kept. The
/// returned config holds the key. If the key can't be saved, for example to a
/// read-only file, loading fails with an error that names the file and
/// `security.api_key`.
///
/// Errors from reading, parsing or validating the configuration wrap
/// [`ConfigError`]; use `anyhow::Error::downcast_ref` to inspect them.
pub fn load_config(options: &RunOptions) -> anyhow::Result<LoadedConfig> {
    prepare_config(options, true)
}

/// Check the service configuration exactly as [`load_config`] does, without
/// writing anything.
///
/// With authentication enabled and no key set, it validates with a
/// placeholder key and never generates or saves one; [`load_config`]
/// generates the key when the service starts.
///
/// `aranet server --daemon` calls this before it detaches, so configuration
/// errors reach the terminal instead of a process whose output is discarded.
pub fn check_config(options: &RunOptions) -> anyhow::Result<()> {
    prepare_config(options, false)?;
    Ok(())
}

/// The body of [`load_config`] (`save_generated_key` true) and
/// [`check_config`] (false).
fn prepare_config(options: &RunOptions, save_generated_key: bool) -> anyhow::Result<LoadedConfig> {
    let path = options
        .config
        .clone()
        .unwrap_or_else(config::default_config_path);

    // `Path::exists` reports "missing" when it can't tell, for example when
    // the directory can't be searched, which silently gave the defaults.
    let mut config = match path.try_exists() {
        Ok(true) => Config::load(&path)?,
        Ok(false) => {
            tracing::info!("No config file at {}; using defaults", path.display());
            Config::default()
        }
        Err(source) => return Err(ConfigError::Read { path, source }.into()),
    };

    if let Some(bind) = &options.bind {
        config.server.bind = bind.clone();
    }
    if let Some(database) = &options.database {
        config.storage.path = database.clone();
    }

    let expanded = config::expand_home(&config.storage.path);
    if let Some(legacy) = unexpanded_database(Path::new("."), &config.storage.path, &expanded) {
        tracing::warn!(
            "Found a database at {}, created by an earlier version that did not expand '~'; \
             the service now uses {}. To keep the old data, stop the service and move the \
             old file there (this replaces the database at that path) or merge the two; \
             otherwise remove the old file",
            legacy.display(),
            expanded.display()
        );
    }
    config.storage.path = expanded;

    // Authentication enabled without a key: fill one in before validating.
    // load_config generates the real key now and saves it only once the config
    // is known to be valid. check_config never writes, so it validates with a
    // placeholder, and the key is generated when the service first starts.
    let generated_key = if config.security.api_key_enabled
        && config.security.api_key.as_deref().is_none_or(str::is_empty)
    {
        let key = if save_generated_key {
            config::generate_api_key()?
        } else {
            "0".repeat(64)
        };
        config.security.api_key = Some(key.clone());
        save_generated_key.then_some(key)
    } else {
        None
    };

    config.validate()?;

    if let Some(key) = generated_key {
        // Only this key is written: comments, other settings and the
        // --bind/--database overrides are left as they are.
        Config::update_file(&path, |file| file.security.api_key = Some(key)).map_err(|e| {
            anyhow::anyhow!(
                "authentication is enabled but security.api_key is not set, and a generated key \
                 could not be saved to {}: {e}. Set security.api_key in that file.",
                path.display()
            )
        })?;
        // Never log the key itself.
        tracing::info!("Generated an API key and saved it to {}", path.display());
    }

    tracing::info!("Using config {}", path.display());
    Ok(LoadedConfig { config, path })
}

/// The database that an earlier version, which didn't expand `~`, created for
/// `raw` under `cwd`, for as long as it is there.
///
/// It is reported even when `expanded` exists, because the first start of
/// this version creates a new database there. It isn't reported when it is
/// the file at `expanded` itself, for example through a `~` symlink to the
/// home directory.
fn unexpanded_database(cwd: &Path, raw: &Path, expanded: &Path) -> Option<PathBuf> {
    if !raw.starts_with("~") || raw == expanded {
        return None;
    }
    let legacy = cwd.join(raw);
    let same_file = matches!(
        (legacy.canonicalize(), expanded.canonicalize()),
        (Ok(old), Ok(new)) if old == new
    );
    (legacy.exists() && !same_file).then_some(legacy)
}

/// The warning [`run`] logs when the service listens on `bound`, a network
/// address, without API-key authentication. [`Config::validate`] allows that
/// only with `security.allow_unauthenticated_network`.
fn unauthenticated_network_warning(bound: SocketAddr, security: &SecurityConfig) -> Option<String> {
    (!security.api_key_enabled && !bound.ip().to_canonical().is_loopback()).then(|| {
        format!(
            "Listening on {bound} without API-key authentication \
             (security.allow_unauthenticated_network = true): anyone who can reach \
             this port can read the readings and change the configuration"
        )
    })
}

/// Run the HTTP service until shutdown.
///
/// Loads the configuration with [`load_config`] and listens on `server.bind`
/// before anything else, so an invalid configuration or a port that is
/// already in use fails before the database is opened, the collector uses
/// Bluetooth or any integration starts.
pub async fn run(options: RunOptions) -> anyhow::Result<()> {
    let LoadedConfig { config, path } = load_config(&options)?;
    for warning in middleware::OriginPolicy::new(&config.security, None).startup_warnings() {
        tracing::warn!("{warning}");
    }

    let addr = config
        .server
        .socket_addr()
        .expect("load_config validated server.bind");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to listen on {addr}"))?;
    let bound = listener.local_addr()?;
    if let Some(warning) = unauthenticated_network_warning(bound, &config.security) {
        tracing::warn!("{warning}");
    }
    tracing::info!("Listening on {bound}");

    tracing::info!("Opening database at {:?}", config.storage.path);
    // Opening creates the file and migrates the schema, and can wait for
    // another process's lock: like every database call, it runs on a
    // blocking thread.
    let db_path = config.storage.path.clone();
    let store = db::run_blocking(move || aranet_store::Store::open(db_path)).await?;
    let state = AppState::with_config_path(store, config.clone(), path);

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

    if let Some(handle) = retention::spawn(Arc::clone(&state), config.storage.retention.clone()) {
        background.push(handle);
    }

    if !options.no_collector {
        Collector::new(Arc::clone(&state)).start().await;
    } else {
        tracing::info!("Background collector disabled");
    }

    #[cfg(feature = "mqtt")]
    {
        use crate::mqtt::MqttPublisher;
        let mqtt_publisher = MqttPublisher::new(Arc::clone(&state));
        if let Some(handle) = mqtt_publisher.start().await {
            background.push(handle);
        }
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
        if let Some(handle) = webhook_dispatcher.start().await {
            background.push(handle);
        }
    }

    {
        use crate::influxdb::InfluxDbWriter;
        let influxdb_writer = InfluxDbWriter::new(Arc::clone(&state));
        if let Some(handle) = influxdb_writer.start().await {
            background.push(handle);
        }
    }

    let _mdns_handle = {
        use crate::mdns::MdnsAdvertiser;
        let advertiser = MdnsAdvertiser::new(Arc::clone(&state));
        advertiser.start(bound).await
    };

    let app = app(
        Arc::clone(&state),
        Arc::clone(&security_config),
        Arc::clone(&rate_limit_state),
    );

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

/// Counts what a sink had to drop, and says when to warn about it: at most
/// once a minute, so an outage that drops data on every reading can't flood
/// the log.
#[derive(Debug, Default)]
pub(crate) struct DropLog {
    /// Items dropped since the sink started.
    pub(crate) dropped: u64,
    last_warn: Option<std::time::Instant>,
}

impl DropLog {
    /// Shortest time between two warnings.
    const WARN_INTERVAL: Duration = Duration::from_secs(60);

    /// Count one dropped item. Returns `true` when the caller should warn
    /// now, with [`Self::dropped`] as the total.
    pub(crate) fn record(&mut self) -> bool {
        self.record_at(std::time::Instant::now())
    }

    fn record_at(&mut self, now: std::time::Instant) -> bool {
        self.dropped += 1;
        let due = self
            .last_warn
            .is_none_or(|last| now.duration_since(last) >= Self::WARN_INTERVAL);
        if due {
            self.last_warn = Some(now);
        }
        due
    }
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
        // API tests on other threads reach the same log statements.
        crate::test_support::register_silent_dispatcher();
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

    #[tokio::test(start_paused = true)]
    async fn stop_background_work_awaits_sink_handles() {
        let config = Config {
            webhooks: WebhookConfig {
                enabled: true,
                endpoints: vec![WebhookEndpoint {
                    url: crate::test_support::closed_port_url("/hook").await,
                    events: vec!["co2_high".to_string()],
                    headers: std::collections::HashMap::new(),
                }],
                ..WebhookConfig::default()
            },
            influxdb: InfluxDbConfig {
                enabled: true,
                url: crate::test_support::closed_port_url("").await,
                org: "aranet".to_string(),
                ..InfluxDbConfig::default()
            },
            ..Config::default()
        };
        let t = crate::test_support::TestState::with_config(config);

        let mut background = Vec::new();
        if let Some(handle) = webhook::WebhookDispatcher::new(Arc::clone(&t.state))
            .start()
            .await
        {
            background.push(handle);
        }
        if let Some(handle) = influxdb::InfluxDbWriter::new(Arc::clone(&t.state))
            .start()
            .await
        {
            background.push(handle);
        }
        let sinks = background.len();
        assert_eq!(sinks, 2, "every enabled sink returns its handle");
        crate::test_support::wait_until("every sink to subscribe to readings", || {
            t.state.readings_tx.receiver_count() == sinks
        })
        .await;

        // A task that ignores shutdown, and so must be aborted.
        let (stuck_tx, stuck_rx) = tokio::sync::oneshot::channel::<()>();
        background.push(tokio::spawn(async move {
            let _stuck_tx = stuck_tx;
            std::future::pending::<()>().await;
        }));

        let begun = tokio::time::Instant::now();
        let stopping = tokio::spawn({
            let state = Arc::clone(&t.state);
            async move { stop_background_work(&state, background).await }
        });

        // The sinks stop as soon as shutdown is signalled, long before the
        // grace period that the stuck task runs into.
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(
            t.state.readings_tx.receiver_count(),
            0,
            "a sink was still running 1 s after shutdown"
        );
        assert!(!stopping.is_finished());

        stopping.await.unwrap();
        let waited = begun.elapsed();
        assert!(
            waited >= SHUTDOWN_GRACE && waited <= SHUTDOWN_GRACE + Duration::from_secs(1),
            "waited {waited:?}"
        );
        assert!(stuck_rx.await.is_err(), "the stuck task was not aborted");
    }

    #[test]
    fn drop_log_warns_at_most_once_a_minute() {
        let mut drops = DropLog::default();
        let start = std::time::Instant::now();
        let at = |secs| start + Duration::from_secs(secs);

        assert!(drops.record_at(at(0)), "the first drop warns");
        assert!(!drops.record_at(at(1)));
        assert!(!drops.record_at(at(59)));
        assert!(drops.record_at(at(60)), "a minute later it warns again");
        assert!(!drops.record_at(at(61)));
        assert_eq!(drops.dropped, 5);
    }

    #[test]
    fn load_config_uses_defaults_when_config_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("server.toml");

        let loaded = load_config(&RunOptions {
            config: Some(missing.clone()),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(loaded.path, missing);
        assert_eq!(loaded.config.server.bind, ServerConfig::default().bind);
        assert!(loaded.config.devices.is_empty());
        assert!(!missing.exists(), "loading created the config file");
    }

    #[test]
    fn load_config_applies_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("server.toml");
        let contents = "[server]\nbind = \"127.0.0.1:8080\"\n";
        std::fs::write(&config_path, contents).unwrap();
        let database = dir.path().join("x.db");

        let loaded = load_config(&RunOptions {
            config: Some(config_path.clone()),
            bind: Some("127.0.0.1:9".to_string()),
            database: Some(database.clone()),
            no_collector: true,
        })
        .unwrap();

        assert_eq!(loaded.config.server.bind, "127.0.0.1:9");
        assert_eq!(loaded.config.storage.path, database);
        assert_eq!(loaded.path, config_path);
        // The overrides live only in the returned value.
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), contents);
        assert!(!database.exists(), "loading opened the database");
    }

    #[test]
    fn check_config_reports_what_load_config_rejects() {
        let dir = tempfile::tempdir().unwrap();
        let options = |bind: &str| RunOptions {
            config: Some(dir.path().join("server.toml")),
            bind: Some(bind.to_string()),
            database: Some(dir.path().join("x.db")),
            no_collector: false,
        };

        check_config(&options("127.0.0.1:9")).unwrap();
        let err = check_config(&options("localhost:8080")).unwrap_err();

        assert!(
            format!("{err:#}").contains("invalid bind address 'localhost:8080'"),
            "{err:#}"
        );
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "check_config created a file"
        );
    }

    /// `Path::exists` is false when it can't check, so a config file in a
    /// directory the service can't search used to count as missing, and the
    /// service ran on the defaults without a word.
    #[cfg(unix)]
    #[test]
    fn load_config_fails_on_unreadable_config() {
        use std::os::unix::fs::PermissionsExt;

        /// Gives the directory its permissions back, so the `TempDir` can be
        /// deleted even when an assertion fails.
        struct RestoreMode<'a>(&'a std::path::Path);
        impl Drop for RestoreMode<'_> {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o700));
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        let config_path = locked.join("server.toml");
        std::fs::write(&config_path, "[server]\nbind = \"127.0.0.1:8080\"\n").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let _restore = RestoreMode(&locked);

        if std::fs::read_dir(&locked).is_ok() {
            // Root ignores directory permissions (the aranet-ci image runs as root).
            eprintln!("skipped: running as root");
            return;
        }

        let result = load_config(&RunOptions {
            config: Some(config_path.clone()),
            ..Default::default()
        });

        let Err(err) = result else {
            panic!("an unreadable config file fell back to the defaults");
        };
        assert!(
            format!("{err:#}").contains(&config_path.display().to_string()),
            "{err:#}"
        );
    }

    #[test]
    fn load_config_expands_home_in_storage_path() {
        let home = dirs::home_dir().expect("a home directory");
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("server.toml");
        let contents = "[storage]\npath = \"~/aranet-load-config-test/data.db\"\n";
        std::fs::write(&config_path, contents).unwrap();

        let loaded = load_config(&RunOptions {
            config: Some(config_path.clone()),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(
            loaded.config.storage.path,
            home.join("aranet-load-config-test").join("data.db")
        );
        // Runtime only: the file keeps its `~`.
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), contents);
    }

    /// `--database '~/x.db'` (quoted, so the shell leaves the `~` alone) is
    /// expanded like a `storage.path` from the file: the overrides are applied
    /// before the expansion.
    #[test]
    fn load_config_expands_home_in_database_override() {
        let home = dirs::home_dir().expect("a home directory");
        let dir = tempfile::tempdir().unwrap();

        let loaded = load_config(&RunOptions {
            config: Some(dir.path().join("server.toml")),
            database: Some(PathBuf::from("~/aranet-load-config-test/override.db")),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(
            loaded.config.storage.path,
            home.join("aranet-load-config-test").join("override.db")
        );
    }

    /// A config file that is there but can't be parsed stops startup. Running
    /// on the defaults instead would move the service to the default bind
    /// address and database without a word.
    #[test]
    fn load_config_fails_on_unparseable_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("server.toml");
        std::fs::write(&config_path, "[server\nbind = \"127.0.0.1:8080\"\n").unwrap();

        let result = load_config(&RunOptions {
            config: Some(config_path.clone()),
            database: Some(dir.path().join("x.db")),
            ..Default::default()
        });

        let Err(err) = result else {
            panic!("an unparseable config file fell back to the defaults");
        };
        assert!(
            matches!(
                err.downcast_ref::<ConfigError>(),
                Some(ConfigError::Parse { path, .. }) if *path == config_path
            ),
            "{err:#}"
        );
    }

    #[test]
    fn unexpanded_database_is_reported_until_it_is_moved() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("cwd");
        let raw = std::path::Path::new("~/aranet/data.db");
        let expanded = dir.path().join("home").join("aranet").join("data.db");

        // Nothing at ./~/aranet/data.db: nothing to report.
        assert_eq!(unexpanded_database(&cwd, raw, &expanded), None);

        // An earlier version created ./~/aranet/data.db.
        let legacy = cwd.join("~").join("aranet").join("data.db");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, b"").unwrap();
        assert_eq!(
            unexpanded_database(&cwd, raw, &expanded),
            Some(legacy.clone())
        );

        // The first start then opens a new database at the expanded path. The
        // old file is still reported on every later start.
        std::fs::create_dir_all(expanded.parent().unwrap()).unwrap();
        std::fs::write(&expanded, b"").unwrap();
        assert_eq!(
            unexpanded_database(&cwd, raw, &expanded),
            Some(legacy.clone())
        );

        // Once the old file is moved there (or removed), the warning stops.
        std::fs::rename(&legacy, &expanded).unwrap();
        assert_eq!(unexpanded_database(&cwd, raw, &expanded), None);

        // A `~` in the working directory that links to the home directory
        // holds no second database.
        #[cfg(unix)]
        {
            let linked_cwd = dir.path().join("linked");
            std::fs::create_dir(&linked_cwd).unwrap();
            std::os::unix::fs::symlink(dir.path().join("home"), linked_cwd.join("~")).unwrap();
            assert!(linked_cwd.join(raw).exists());
            assert_eq!(unexpanded_database(&linked_cwd, raw, &expanded), None);
        }

        // Paths without a leading `~` are never reported.
        let plain = std::path::Path::new("aranet/data.db");
        assert_eq!(unexpanded_database(&cwd, plain, plain), None);
    }

    /// Write `dir/server.toml` with `bind` and a database at `dir/sub/aranet.db`.
    /// Returns the config path. A single-quoted TOML string needs no escaping,
    /// so Windows paths work too.
    fn write_server_toml(dir: &std::path::Path, bind: &str) -> PathBuf {
        let path = dir.join("server.toml");
        let database = dir.join("sub").join("aranet.db");
        let contents = format!(
            "[server]\nbind = \"{bind}\"\n\n[storage]\npath = '{}'\n",
            database.display()
        );
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// Run the service and return its error. The limit turns a regression that
    /// starts serving into a failure instead of a hung test.
    ///
    /// The database override names the path `write_server_toml` puts in the
    /// file, so it changes nothing while the file is honoured. It is there for
    /// a regression that ignores the file: the service these tests then start
    /// opens its database in the `TempDir`, not at the default path.
    async fn run_until_error(config: PathBuf) -> anyhow::Error {
        let database = config.with_file_name("sub").join("aranet.db");
        let options = RunOptions {
            config: Some(config),
            database: Some(database),
            no_collector: true,
            ..Default::default()
        };
        tokio::time::timeout(std::time::Duration::from_secs(5), run(options))
            .await
            .expect("run() started serving instead of failing")
            .expect_err("run() returned Ok")
    }

    #[tokio::test]
    async fn test_run_rejects_host_name_bind_before_opening_database() {
        let dir = tempfile::tempdir().unwrap();
        let config = write_server_toml(dir.path(), "localhost:8080");

        let err = run_until_error(config).await;

        assert!(
            !dir.path().join("sub").exists(),
            "the database was opened before the bind address was checked: {err:#}"
        );
        match err.downcast_ref::<ConfigError>() {
            Some(ConfigError::Validation(errors)) => {
                assert_eq!(errors.len(), 1, "{errors:?}");
                assert_eq!(errors[0].field, "server.bind");
                assert!(errors[0].message.contains("'localhost:8080'"), "{errors:?}");
            }
            _ => panic!("expected a validation error, got: {err:#}"),
        }
    }

    #[tokio::test]
    async fn test_run_fails_before_side_effects_when_port_in_use() {
        let dir = tempfile::tempdir().unwrap();
        // Neither this listener nor the service sets SO_REUSEPORT, so the
        // service's bind fails on Linux, macOS and Windows alike.
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let bind = taken.local_addr().unwrap().to_string();
        let config = write_server_toml(dir.path(), &bind);

        let err = run_until_error(config).await;

        assert!(
            !dir.path().join("sub").exists(),
            "the database was opened before the port was bound: {err:#}"
        );
        assert!(
            format!("{err:#}").starts_with(&format!("failed to listen on {bind}: ")),
            "{err:#}"
        );
    }

    /// A `server.toml` in a new temporary directory, and options that read it
    /// with the database overridden into the same directory.
    fn auth_config_file(contents: &str) -> (tempfile::TempDir, std::path::PathBuf, RunOptions) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.toml");
        std::fs::write(&path, contents).unwrap();
        let options = RunOptions {
            config: Some(path.clone()),
            database: Some(dir.path().join("data.db")),
            ..RunOptions::default()
        };
        (dir, path, options)
    }

    fn looks_generated(key: &str) -> bool {
        key.len() == 64 && key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    }

    #[test]
    fn load_config_generates_and_saves_missing_api_key() {
        let (_dir, path, options) =
            auth_config_file("# keep\n[security]\napi_key_enabled = true\n");

        let loaded = load_config(&options).unwrap();
        let key = loaded.config.security.api_key.clone().unwrap();
        // A failed assertion must not print the key, so the messages below
        // say what was wrong and leave out the key and the file that holds it.
        assert!(
            looks_generated(&key),
            "the generated key is not 64 lowercase hex characters"
        );

        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.starts_with("# keep\n"), "the comment was lost");
        assert!(
            saved.contains(&format!("api_key = \"{key}\"")),
            "the file has no api_key line with the generated key"
        );
        assert!(!saved.contains("data.db"), "an override was saved");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        // Not unwrap(): a parse error quotes a line of the file, and the file
        // now holds the key.
        let Ok(again) = load_config(&options) else {
            panic!("the file with the saved key could not be loaded again");
        };
        assert!(
            again.config.security.api_key.as_deref() == Some(key.as_str()),
            "the second load did not return the saved key"
        );
    }

    #[test]
    fn generated_api_key_is_not_logged() {
        let (_dir, path, options) = auth_config_file("[security]\napi_key_enabled = true\n");
        let (logs, _guard) = crate::test_support::CapturedLogs::start();

        let key = load_config(&options)
            .unwrap()
            .config
            .security
            .api_key
            .unwrap();

        // Neither message prints the log: when the key was logged, the log
        // holds it.
        let text = logs.text();
        assert!(
            text.contains(&format!(
                "Generated an API key and saved it to {}",
                path.display()
            )),
            "the log does not say that a key was generated and saved to {}",
            path.display()
        );
        assert!(!text.contains(&key), "the key reached the log");
    }

    #[cfg(unix)]
    #[test]
    fn load_config_fails_clearly_when_key_cannot_be_saved() {
        use std::os::unix::fs::PermissionsExt;

        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("read-only");
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("server.toml");
        let original = "[security]\napi_key_enabled = true\n";
        std::fs::write(&path, original).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();

        // Root ignores file modes (the aranet-ci image runs as root).
        let root = std::fs::OpenOptions::new().append(true).open(&path).is_ok();
        let result = (!root).then(|| {
            load_config(&RunOptions {
                config: Some(path.clone()),
                database: Some(t.path().join("data.db")),
                ..RunOptions::default()
            })
        });
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let Some(result) = result else {
            eprintln!("skipped: file modes don't apply to root");
            return;
        };

        // Not unwrap_err(): it would print the loaded config, which holds the
        // generated key.
        let Err(error) = result else {
            panic!("loading succeeded although the key could not be saved");
        };
        let error = format!("{error:#}");
        assert!(error.contains("could not be saved"), "{error}");
        assert!(error.contains(&path.display().to_string()), "{error}");
        assert!(error.contains("security.api_key"), "{error}");
        // Not assert_eq!: a file that was changed after all may hold the key.
        assert!(
            std::fs::read_to_string(&path).unwrap() == original,
            "the config file was changed although the key could not be saved"
        );
    }

    #[test]
    fn check_config_never_writes_a_key() {
        let original = "[security]\napi_key_enabled = true\n";
        let (dir, path, options) = auth_config_file(original);

        check_config(&options).unwrap();

        // Not assert_eq!: a file that was written after all may hold a key.
        assert!(
            std::fs::read_to_string(&path).unwrap() == original,
            "check_config changed the config file"
        );
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["server.toml"]);
    }

    #[test]
    fn load_config_saves_no_key_for_a_short_key_or_an_invalid_config() {
        for (contents, field) in [
            // A key that is set but too short is rejected, never replaced.
            (
                "[security]\napi_key_enabled = true\napi_key = \"too-short\"\n",
                "security.api_key",
            ),
            // A key is saved only once the rest of the config is valid.
            (
                "[server]\nbind = \"localhost:8080\"\n\n[security]\napi_key_enabled = true\n",
                "server.bind",
            ),
        ] {
            let (_dir, path, options) = auth_config_file(contents);

            let Err(err) = load_config(&options) else {
                panic!("{field}: the config was accepted");
            };

            match err.downcast_ref::<ConfigError>() {
                Some(ConfigError::Validation(errors)) => assert!(
                    errors.iter().any(|error| error.field == field),
                    "{errors:?}"
                ),
                _ => panic!("expected a validation error, got: {err:#}"),
            }
            assert!(
                std::fs::read_to_string(&path).unwrap() == contents,
                "{field}: the config file was changed"
            );
        }
    }

    #[test]
    fn load_config_replaces_an_empty_api_key_with_a_random_one() {
        let generated_key = |contents: &str| {
            let (_dir, path, options) = auth_config_file(contents);
            let key = load_config(&options)
                .unwrap()
                .config
                .security
                .api_key
                .unwrap();
            (key, std::fs::read_to_string(&path).unwrap())
        };

        let (key, saved) = generated_key("[security]\napi_key_enabled = true\napi_key = \"\"\n");
        assert!(looks_generated(&key), "the empty key was kept");
        assert!(
            saved.contains(&format!("api_key = \"{key}\"")),
            "the key was not saved"
        );
        assert_eq!(
            saved.matches("api_key = ").count(),
            1,
            "the empty key is still in the file"
        );

        // Every file gets a key of its own, never check_config's placeholder.
        let (other, _) = generated_key("[security]\napi_key_enabled = true\n");
        assert!(key != other, "two config files got the same key");
        assert!(
            key != "0".repeat(64) && other != "0".repeat(64),
            "the placeholder was saved as the key"
        );
    }

    #[tokio::test]
    async fn test_run_logs_startup_warnings() {
        let dir = tempfile::tempdir().unwrap();
        // The port is taken, so run() stops right after the warnings.
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let bind = taken.local_addr().unwrap().to_string();
        let config = write_server_toml(dir.path(), &bind);
        let mut contents = std::fs::read_to_string(&config).unwrap();
        contents.push_str("\n[security]\nallowed_hosts = [\"*\"]\ncors_origins = [\"*/\"]\n");
        std::fs::write(&config, contents).unwrap();
        let (logs, _guard) = crate::test_support::CapturedLogs::start();

        let err = run_until_error(config).await;

        assert!(
            format!("{err:#}").starts_with(&format!("failed to listen on {bind}: ")),
            "{err:#}"
        );
        let text = logs.text();
        assert!(
            text.contains("security.allowed_hosts contains \"*\""),
            "{text}"
        );
        assert!(
            text.contains("security.cors_origins contains \"*\""),
            "{text}"
        );
    }

    #[test]
    fn test_load_config_refuses_bind_override_without_auth() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("server.toml");
        std::fs::write(&config_path, "[server]\nbind = \"127.0.0.1:8080\"\n").unwrap();
        let options = RunOptions {
            config: Some(config_path),
            bind: Some("0.0.0.0:8080".into()),
            database: Some(dir.path().join("data.db")),
            ..Default::default()
        };

        // The file on its own is valid: the --bind override is what is refused.
        let file_only = RunOptions {
            bind: None,
            ..options.clone()
        };
        load_config(&file_only).expect("a loopback bind needs no API key");

        let Err(error) = load_config(&options) else {
            panic!("load_config accepted --bind 0.0.0.0:8080 without an API key");
        };
        let message = format!("{error:#}");
        assert!(
            message.contains("server.bind") && message.contains("allow_unauthenticated_network"),
            "{message}"
        );
        // `aranet server --daemon` and `service install` check with this.
        let Err(error) = check_config(&options) else {
            panic!("check_config accepted --bind 0.0.0.0:8080 without an API key");
        };
        assert!(
            format!("{error:#}").contains("allow_unauthenticated_network"),
            "{error:#}"
        );
        assert!(!dir.path().join("data.db").exists());
    }

    #[test]
    fn test_unauthenticated_network_warning() {
        let addr = |text: &str| text.parse::<SocketAddr>().unwrap();
        let opted_out = SecurityConfig {
            allow_unauthenticated_network: true,
            ..Default::default()
        };
        for bind in ["0.0.0.0:8080", "[::]:8080", "192.168.1.10:8080"] {
            let warning = unauthenticated_network_warning(addr(bind), &opted_out)
                .unwrap_or_else(|| panic!("no warning for {bind}"));
            assert!(
                warning.contains(bind) && warning.contains("allow_unauthenticated_network"),
                "{warning}"
            );
        }

        // Nothing to warn about on loopback, or with authentication on.
        for bind in ["127.0.0.1:8080", "[::1]:8080", "[::ffff:127.0.0.1]:8080"] {
            assert_eq!(
                unauthenticated_network_warning(addr(bind), &opted_out),
                None,
                "{bind}"
            );
        }
        let authenticated = SecurityConfig {
            api_key_enabled: true,
            api_key: Some("0123456789abcdef".repeat(4)),
            ..Default::default()
        };
        assert_eq!(
            unauthenticated_network_warning(addr("0.0.0.0:8080"), &authenticated),
            None
        );
    }
}
