//! Server command - start the HTTP API server.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Arguments for the server command.
pub struct ServerArgs {
    pub bind: String,
    pub database: Option<PathBuf>,
    pub no_collector: bool,
    pub daemon: bool,
}

/// Execute the server command.
pub async fn cmd_server(args: ServerArgs) -> Result<()> {
    use aranet_service::{AppState, Collector, Config};
    use aranet_store::Store;
    use axum::Router;
    use std::sync::Arc;
    use tower_http::cors::{Any, CorsLayer};
    use tower_http::trace::TraceLayer;
    use tracing::info;

    // Load or create config
    let mut config = Config::load_default().unwrap_or_default();

    // Override with CLI args
    config.server.bind = args.bind.clone();
    if let Some(db_path) = args.database {
        config.storage.path = db_path;
    }

    // Handle daemon mode
    if args.daemon {
        return run_daemon(&config);
    }

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("aranet_service=info".parse()?)
                .add_directive("tower_http=debug".parse()?),
        )
        .init();

    // Open the database
    info!("Opening database at {:?}", config.storage.path);
    let store = Store::open(&config.storage.path).context("Failed to open database")?;

    // Create application state
    let state = AppState::new(store, config.clone());

    // Start the background collector
    if !args.no_collector {
        let mut collector = Collector::new(Arc::clone(&state));
        collector.start().await;
        info!("Background collector started");
    } else {
        info!("Background collector disabled");
    }

    // Build the router
    let app = Router::new()
        .merge(aranet_service::api::router())
        .merge(aranet_service::ws::router())
        .layer(TraceLayer::new_for_http())
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state);

    // Parse bind address
    let addr: std::net::SocketAddr = config.server.bind.parse().context("Invalid bind address")?;

    println!("Starting Aranet API server on http://{}", addr);
    println!("API endpoints:");
    println!("  GET  /api/health              - Health check");
    println!("  GET  /api/devices             - List all devices");
    println!("  GET  /api/devices/:id         - Get device info");
    println!("  GET  /api/devices/:id/current - Latest reading");
    println!("  GET  /api/devices/:id/history - Query history");
    println!("  WS   /api/ws                  - Real-time stream");
    println!();
    println!("Press Ctrl+C to stop");

    // Run the server
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Run the server as a background daemon.
fn run_daemon(config: &aranet_service::Config) -> Result<()> {
    use std::process::Command;

    // Get the current executable path
    let exe = std::env::current_exe().context("Failed to get current executable path")?;

    // Build args without --daemon to avoid infinite recursion
    let mut args = vec!["server".to_string()];
    args.push("--bind".to_string());
    args.push(config.server.bind.clone());

    if config.storage.path != aranet_store::default_db_path() {
        args.push("--database".to_string());
        args.push(config.storage.path.display().to_string());
    }

    // Spawn detached process
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        // Use setsid to create a new session
        let child = Command::new(&exe)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .context("Failed to spawn daemon process")?;

        println!("Aranet server started in background (PID: {})", child.id());
        println!("Listening on http://{}", config.server.bind);
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        const DETACHED_PROCESS: u32 = 0x00000008;

        let child = Command::new(&exe)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
            .spawn()
            .context("Failed to spawn daemon process")?;

        println!("Aranet server started in background (PID: {})", child.id());
        println!("Listening on http://{}", config.server.bind);
    }

    Ok(())
}
