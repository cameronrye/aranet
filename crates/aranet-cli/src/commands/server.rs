//! Server command - start the HTTP API server.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Arguments for the server command.
pub struct ServerArgs {
    pub config: Option<PathBuf>,
    pub bind: Option<String>,
    pub database: Option<PathBuf>,
    pub no_collector: bool,
    pub daemon: bool,
}

/// Execute the server command.
pub async fn cmd_server(args: ServerArgs) -> Result<()> {
    // Handle daemon mode
    if args.daemon {
        return run_daemon(&args);
    }

    aranet_service::init_tracing()?;
    aranet_service::run(aranet_service::RunOptions {
        config: args.config,
        bind: args.bind,
        database: args.database,
        no_collector: args.no_collector,
    })
    .await
    .context("Failed to run aranet-service")
}

/// Run the server as a background daemon.
fn run_daemon(args: &ServerArgs) -> Result<()> {
    use std::process::Command;

    // Get the current executable path
    let exe = std::env::current_exe().context("Failed to get current executable path")?;

    // Build args without --daemon to avoid infinite recursion
    let mut daemon_args = vec!["server".to_string()];

    if let Some(config_path) = &args.config {
        daemon_args.push("--config".to_string());
        daemon_args.push(config_path.display().to_string());
    }

    if let Some(bind) = &args.bind {
        daemon_args.push("--bind".to_string());
        daemon_args.push(bind.clone());
    }

    if let Some(database) = &args.database {
        daemon_args.push("--database".to_string());
        daemon_args.push(database.display().to_string());
    }

    if args.no_collector {
        daemon_args.push("--no-collector".to_string());
    }

    // Spawn detached process
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        // Use setsid to create a new session
        let child = Command::new(&exe)
            .args(&daemon_args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .context("Failed to spawn daemon process")?;

        println!("Aranet server started in background (PID: {})", child.id());
        println!("Detached process started successfully");
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        const DETACHED_PROCESS: u32 = 0x00000008;

        let child = Command::new(&exe)
            .args(&daemon_args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
            .spawn()
            .context("Failed to spawn daemon process")?;

        println!("Aranet server started in background (PID: {})", child.id());
        println!("Detached process started successfully");
    }

    Ok(())
}
