//! Aranet Service - Background collector and HTTP API.
//!
//! Run with: `cargo run -p aranet-service`

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use clap::{Parser, Subcommand};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;

use aranet_service::config::default_config_path;
use aranet_service::middleware::{self, RateLimitState};
use aranet_service::{AppState, Collector, Config, api, ws};
use aranet_store::Store;

mod service;

/// Aranet Service - Background collector and HTTP REST API.
#[derive(Parser, Debug)]
#[command(name = "aranet-service")]
#[command(version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    /// Path to configuration file.
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    /// Bind address (overrides config).
    #[arg(short, long, global = true)]
    bind: Option<String>,

    /// Database path (overrides config).
    #[arg(short, long, global = true)]
    database: Option<PathBuf>,

    /// Disable background collector (API only mode).
    #[arg(long, global = true)]
    no_collector: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run the service in the foreground (default behavior).
    Run,

    /// Manage the background service.
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
}

#[derive(Subcommand, Debug)]
enum ServiceAction {
    /// Install aranet-service as a system service.
    Install {
        /// Install as user-level service (no root/admin required).
        #[arg(long)]
        user: bool,
    },

    /// Uninstall the aranet-service system service.
    Uninstall {
        /// Uninstall user-level service.
        #[arg(long)]
        user: bool,
    },

    /// Start the aranet-service system service.
    Start {
        /// Start user-level service.
        #[arg(long)]
        user: bool,
    },

    /// Stop the aranet-service system service.
    Stop {
        /// Stop user-level service.
        #[arg(long)]
        user: bool,
    },

    /// Check the status of the aranet-service.
    Status {
        /// Check user-level service status.
        #[arg(long)]
        user: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    match args.command {
        Some(Command::Service { action }) => handle_service_action(action),
        Some(Command::Run) | None => run_server(args).await,
    }
}

fn handle_service_action(action: ServiceAction) -> anyhow::Result<()> {
    use service::{Level, ServiceStatus};

    let (action_name, result) = match action {
        ServiceAction::Install { user } => {
            let level = if user { Level::User } else { Level::System };
            ("install", service::install(level))
        }
        ServiceAction::Uninstall { user } => {
            let level = if user { Level::User } else { Level::System };
            ("uninstall", service::uninstall(level))
        }
        ServiceAction::Start { user } => {
            let level = if user { Level::User } else { Level::System };
            ("start", service::start(level))
        }
        ServiceAction::Stop { user } => {
            let level = if user { Level::User } else { Level::System };
            ("stop", service::stop(level))
        }
        ServiceAction::Status { user } => {
            let level = if user { Level::User } else { Level::System };
            match service::status(level) {
                Ok(ServiceStatus::Running) => {
                    println!("aranet-service is running");
                    return Ok(());
                }
                Ok(ServiceStatus::Stopped) => {
                    println!("aranet-service is stopped");
                    return Ok(());
                }
                Err(e) => {
                    eprintln!("Failed to get status: {}", e);
                    return Err(e.into());
                }
            }
        }
    };

    match result {
        Ok(()) => {
            println!("Successfully {}ed aranet-service", action_name);
            Ok(())
        }
        Err(e) => {
            eprintln!("Failed to {} service: {}", action_name, e);
            Err(e.into())
        }
    }
}

async fn run_server(args: Args) -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("aranet_service=info".parse()?)
                .add_directive("tower_http=debug".parse()?),
        )
        .init();

    // Determine config path
    let config_path = args.config.clone().unwrap_or_else(default_config_path);

    // Load configuration
    let mut config = if config_path.exists() {
        Config::load(&config_path)?
    } else {
        Config::default()
    };

    // Override config with CLI args
    if let Some(bind) = args.bind {
        config.server.bind = bind;
    }
    if let Some(db_path) = args.database {
        config.storage.path = db_path;
    }

    // Open the database
    info!("Opening database at {:?}", config.storage.path);
    let store = Store::open(&config.storage.path)?;

    // Create application state with config path for persistence
    let state = AppState::with_config_path(store, config.clone(), config_path);

    // Create security middleware state
    let security_config = Arc::new(config.security.clone());
    let rate_limit_state = Arc::new(RateLimitState::new());

    // Start periodic rate limit cleanup (every 5 minutes)
    {
        let rate_limit_state = Arc::clone(&rate_limit_state);
        let window_secs = config.security.rate_limit_window_secs;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            loop {
                interval.tick().await;
                rate_limit_state.cleanup(window_secs).await;
            }
        });
    }

    // Start the background collector
    let collector = if !args.no_collector {
        let mut collector = Collector::new(Arc::clone(&state));
        collector.start().await;
        Some(collector)
    } else {
        info!("Background collector disabled");
        None
    };

    // Start MQTT publisher if enabled
    #[cfg(feature = "mqtt")]
    {
        use aranet_service::mqtt::MqttPublisher;
        let mqtt_publisher = MqttPublisher::new(Arc::clone(&state));
        mqtt_publisher.start().await;
    }

    // Start Prometheus push gateway if enabled
    #[cfg(feature = "prometheus")]
    {
        use aranet_service::prometheus::PrometheusPusher;
        let prometheus_pusher = PrometheusPusher::new(Arc::clone(&state));
        prometheus_pusher.start().await;
    }

    // Build the router
    let app = Router::new()
        .merge(api::router())
        .merge(ws::router())
        .layer(axum::middleware::from_fn_with_state(
            security_config.clone(),
            middleware::api_key_auth,
        ))
        .layer(axum::middleware::from_fn_with_state(
            (security_config, rate_limit_state),
            middleware::rate_limit,
        ))
        .layer(TraceLayer::new_for_http())
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(Arc::clone(&state));

    // Parse bind address
    let addr: SocketAddr = config.server.bind.parse()?;

    info!("Starting server on {}", addr);

    // Run the server with graceful shutdown
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
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown signal received, stopping services...");

    // Stop the collector and wait for tasks to finish
    if let Some(ref mut collector) = collector {
        collector.stop().await;
    }

    // Signal any remaining collector tasks to stop (in case of config reload)
    state.collector.signal_stop();

    info!("Graceful shutdown complete");
}
