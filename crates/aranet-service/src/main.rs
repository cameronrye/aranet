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

    // Load configuration
    let mut config = match &args.config {
        Some(path) => Config::load(path)?,
        None => Config::load_default().unwrap_or_default(),
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

    // Create application state
    let state = AppState::new(store, config.clone());

    // Start the background collector
    if !args.no_collector {
        let collector = Collector::new(Arc::clone(&state));
        collector.start();
    } else {
        info!("Background collector disabled");
    }

    // Build the router
    let app = Router::new()
        .merge(api::router())
        .merge(ws::router())
        .layer(TraceLayer::new_for_http())
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state);

    // Parse bind address
    let addr: SocketAddr = config.server.bind.parse()?;

    info!("Starting server on {}", addr);

    // Run the server
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
