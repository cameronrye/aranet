//! Aranet Service - Background collector and HTTP API.
//!
//! Run with: `cargo run -p aranet-service`

use std::path::PathBuf;

use aranet_service::{RunOptions, init_tracing, run};
use clap::{Parser, Subcommand};

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
    /// Install aranet-service as a service (system-wide installs need Linux with systemd).
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

impl ServiceAction {
    /// The level the action applies to: `--user` or system-wide.
    fn level(&self) -> service::Level {
        let (Self::Install { user }
        | Self::Uninstall { user }
        | Self::Start { user }
        | Self::Stop { user }
        | Self::Status { user }) = self;
        if *user {
            service::Level::User
        } else {
            service::Level::System
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let run_options = RunOptions {
        config: args.config.clone(),
        bind: args.bind.clone(),
        database: args.database.clone(),
        no_collector: args.no_collector,
    };

    match args.command {
        Some(Command::Service { action }) => handle_service_action(action, run_options),
        Some(Command::Run) | None => {
            init_tracing()?;
            run(run_options).await
        }
    }
}

fn handle_service_action(action: ServiceAction, run_options: RunOptions) -> anyhow::Result<()> {
    let level = action.level();
    let result = run_service_action(action, level, run_options);

    // Services from 0.2.1 or earlier that this action didn't replace or remove,
    // one line each. On stderr: the GUI reads `service status` from stdout.
    if let Some(warnings) = service::legacy_service_warning(level) {
        for warning in warnings.lines() {
            eprintln!("Warning: {warning}");
        }
    }

    result
}

/// Run `action` at `level` and print its result. A failure is returned, not
/// printed: `main` prints the error it returns.
fn run_service_action(
    action: ServiceAction,
    level: service::Level,
    run_options: RunOptions,
) -> anyhow::Result<()> {
    use service::{Level, ServiceStatus};

    let mut install_notes = None;
    let (what, success_message, result) = match action {
        ServiceAction::Install { .. } => {
            let result = service::install(level, &run_options);
            if result.is_ok() && matches!(level, Level::System) {
                // Only systemd accepts a system-level install.
                install_notes = Some(service::system_install_notes(&run_options));
            }
            (
                "install service",
                "Successfully installed aranet-service",
                result,
            )
        }
        ServiceAction::Uninstall { .. } => (
            "uninstall service",
            "Successfully uninstalled aranet-service",
            service::uninstall(level),
        ),
        ServiceAction::Start { .. } => (
            "start service",
            "Successfully started aranet-service",
            service::start(level),
        ),
        ServiceAction::Stop { .. } => (
            "stop service",
            "Successfully stopped aranet-service",
            service::stop(level),
        ),
        ServiceAction::Status { .. } => {
            let status =
                service::status(level).map_err(|e| service_action_error("get status", e))?;
            println!(
                "{}",
                match status {
                    ServiceStatus::Running => "aranet-service is running",
                    ServiceStatus::Stopped => "aranet-service is stopped",
                    ServiceStatus::NotInstalled => "aranet-service is not installed",
                }
            );
            return Ok(());
        }
    };

    result.map_err(|e| service_action_error(what, e))?;
    println!("{success_message}");
    if let Some(notes) = install_notes {
        print!("{notes}");
    }
    Ok(())
}

/// The error `main` returns when a service action failed: `Failed to <what>`,
/// with `error`'s message as its cause.
///
/// The cause is the message, not the error itself. Every `ServiceError` that has
/// a source prints that source in its own message, so `main`, which prints the
/// whole chain, would show the source a second time.
fn service_action_error(what: &str, error: service::ServiceError) -> anyhow::Error {
    anyhow::Error::msg(error.to_string()).context(format!("Failed to {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `main` prints for a failed action is the `Debug` text of the error it
    /// returns. Each part of the error is in it once.
    #[test]
    fn failed_service_action_prints_its_error_once() {
        use service::ServiceError;
        use std::io;

        let errors = [
            ServiceError::InvalidConfig("x marks".into()),
            // These two print their source in their own message and also have it
            // as their source.
            ServiceError::Manager(io::Error::other("x marks")),
            ServiceError::CurrentDirectory(io::Error::other("x marks")),
        ];
        for error in errors {
            let name = format!("{error:?}");
            let printed = format!("{:?}", service_action_error("install service", error));
            assert!(
                printed.starts_with("Failed to install service\n"),
                "{name}: {printed}"
            );
            assert_eq!(printed.matches("x marks").count(), 1, "{name}: {printed}");
        }

        // `starts_with`: with `RUST_BACKTRACE` set, a backtrace follows.
        let printed = format!(
            "{:?}",
            service_action_error("get status", ServiceError::NoServiceManager)
        );
        assert!(
            printed.starts_with(
                "Failed to get status\n\nCaused by:\n    No service manager available on this platform"
            ),
            "{printed}"
        );
    }

    #[test]
    fn service_action_level_follows_the_user_flag() {
        for user in [false, true] {
            let actions = [
                ServiceAction::Install { user },
                ServiceAction::Uninstall { user },
                ServiceAction::Start { user },
                ServiceAction::Stop { user },
                ServiceAction::Status { user },
            ];
            for action in actions {
                let level = action.level();
                assert_eq!(
                    matches!(level, service::Level::User),
                    user,
                    "{action:?}: {level:?}"
                );
                assert_eq!(
                    matches!(level, service::Level::System),
                    !user,
                    "{action:?}: {level:?}"
                );
            }
        }
    }
}
