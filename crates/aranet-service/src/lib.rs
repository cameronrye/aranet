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
//! query parameter only on `/api/ws` instead:
//! `ws://localhost:8080/api/ws?token=your-api-key`
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

use axum::Router;
use tower_http::trace::TraceLayer;

pub mod api;
pub mod collector;
pub mod config;
pub mod dashboard;
pub mod middleware;
pub mod state;
pub mod ws;

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

/// Initialize the default tracing subscriber used by the service binaries.
pub fn init_tracing() -> anyhow::Result<()> {
    let filter = tracing_subscriber::EnvFilter::from_default_env()
        .add_directive("aranet_service=info".parse()?)
        .add_directive("tower_http=debug".parse()?);

    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
    Ok(())
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
        .layer(TraceLayer::new_for_http())
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
    let store = aranet_store::Store::open(&config.storage.path)?;
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

    let collector = if !options.no_collector {
        let collector = Collector::new(Arc::clone(&state));
        collector.start().await;
        Some(collector)
    } else {
        tracing::info!("Background collector disabled");
        None
    };

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
        prometheus_pusher.start().await;
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
    .with_graceful_shutdown(shutdown_signal(collector, state))
    .await?;

    Ok(())
}

/// Wait for shutdown signal and perform cleanup.
async fn shutdown_signal(mut collector: Option<Collector>, state: Arc<AppState>) {
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

    if let Some(ref mut collector) = collector {
        collector.stop().await;
    }

    state.signal_shutdown();
    state.collector.signal_stop();

    tracing::info!("Graceful shutdown complete");
}
