//! Service management for aranet-service.
//!
//! Provides cross-platform service installation/management using the service-manager crate.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use service_manager::{
    RestartPolicy, ServiceInstallCtx, ServiceLabel, ServiceLevel, ServiceManager, ServiceStartCtx,
    ServiceStatusCtx, ServiceStopCtx, ServiceUninstallCtx,
};
use thiserror::Error;

/// Service label for aranet
const SERVICE_LABEL: &str = "dev.rye.aranet";

/// Errors that can occur during service management.
#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("No service manager available on this platform")]
    NoServiceManager,

    #[error("Service manager error: {0}")]
    Manager(#[from] std::io::Error),

    #[error("Could not find aranet-service executable")]
    ExecutableNotFound,

    #[error("User-level services not supported on this platform")]
    UserLevelNotSupported,

    #[error("Failed to resolve current working directory: {0}")]
    CurrentDirectory(#[source] std::io::Error),
}

/// Service management level
#[derive(Debug, Clone, Copy, Default)]
pub enum Level {
    /// System-level service (requires root/admin)
    #[default]
    System,
    /// User-level service (no elevated privileges needed)
    User,
}

/// Get the native service manager for this platform.
fn get_manager(level: Level) -> Result<Box<dyn ServiceManager>, ServiceError> {
    let mut manager = <dyn ServiceManager>::native().map_err(|_| ServiceError::NoServiceManager)?;

    let service_level = match level {
        Level::System => ServiceLevel::System,
        Level::User => ServiceLevel::User,
    };

    manager
        .set_level(service_level)
        .map_err(|_| ServiceError::UserLevelNotSupported)?;

    Ok(manager)
}

/// Get the path to the aranet-service executable.
///
/// Prefers well-known install locations over the current executable path
/// to ensure the service uses a stable binary location:
/// - macOS: /usr/local/bin/aranet-service or ~/Library/bin/aranet-service
/// - Linux: /usr/local/bin/aranet-service or ~/.local/bin/aranet-service
/// - Windows: Program Files paths
///
/// Falls back to current_exe() if no installed binary is found.
fn get_executable_path() -> Result<PathBuf, ServiceError> {
    // Check well-known install locations
    let candidates = get_install_candidates();

    for candidate in candidates {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    // Fall back to the current executable (may be a debug build)
    env::current_exe().map_err(|_| ServiceError::ExecutableNotFound)
}

/// Get candidate paths where aranet-service might be installed.
fn get_install_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    #[cfg(target_os = "macos")]
    {
        // Homebrew or manual install
        candidates.push(PathBuf::from("/usr/local/bin/aranet-service"));
        candidates.push(PathBuf::from("/opt/homebrew/bin/aranet-service"));

        // User-local installs
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join(".cargo/bin/aranet-service"));
            candidates.push(home.join("Library/bin/aranet-service"));
        }
    }

    #[cfg(target_os = "linux")]
    {
        candidates.push(PathBuf::from("/usr/local/bin/aranet-service"));
        candidates.push(PathBuf::from("/usr/bin/aranet-service"));

        // User-local installs
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join(".cargo/bin/aranet-service"));
            candidates.push(home.join(".local/bin/aranet-service"));
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Program Files
        if let Ok(program_files) = env::var("ProgramFiles") {
            candidates
                .push(PathBuf::from(&program_files).join("aranet-service/aranet-service.exe"));
        }

        // User-local installs
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join(".cargo/bin/aranet-service.exe"));
        }
    }

    candidates
}

/// Get the service label.
fn get_label() -> ServiceLabel {
    SERVICE_LABEL.parse().expect("Invalid service label")
}

/// Install aranet-service as a system service.
pub fn install(level: Level, options: &aranet_service::RunOptions) -> Result<(), ServiceError> {
    let manager = get_manager(level)?;
    let program = get_executable_path()?;
    let label = get_label();
    let args = build_install_args(options)?;

    manager
        .install(ServiceInstallCtx {
            label,
            program,
            args,
            contents: None,
            username: None,
            working_directory: None,
            environment: None,
            autostart: true,
            restart_policy: RestartPolicy::OnFailure {
                delay_secs: Some(5),
            },
        })
        .map_err(ServiceError::Manager)?;

    Ok(())
}

fn build_install_args(options: &aranet_service::RunOptions) -> Result<Vec<OsString>, ServiceError> {
    let mut args = vec![OsString::from("run")];

    if let Some(config) = &options.config {
        args.push(OsString::from("--config"));
        args.push(resolve_service_path(config)?.into_os_string());
    }

    if let Some(bind) = &options.bind {
        args.push(OsString::from("--bind"));
        args.push(OsString::from(bind));
    }

    if let Some(database) = &options.database {
        args.push(OsString::from("--database"));
        args.push(resolve_service_path(database)?.into_os_string());
    }

    if options.no_collector {
        args.push(OsString::from("--no-collector"));
    }

    Ok(args)
}

fn resolve_service_path(path: &Path) -> Result<PathBuf, ServiceError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(env::current_dir()
            .map_err(ServiceError::CurrentDirectory)?
            .join(path))
    }
}

/// Uninstall the aranet-service system service.
pub fn uninstall(level: Level) -> Result<(), ServiceError> {
    let manager = get_manager(level)?;
    let label = get_label();

    manager
        .uninstall(ServiceUninstallCtx { label })
        .map_err(ServiceError::Manager)?;

    Ok(())
}

/// Start the aranet-service system service.
pub fn start(level: Level) -> Result<(), ServiceError> {
    let manager = get_manager(level)?;
    let label = get_label();

    manager
        .start(ServiceStartCtx { label })
        .map_err(ServiceError::Manager)?;

    Ok(())
}

/// Stop the aranet-service system service.
pub fn stop(level: Level) -> Result<(), ServiceError> {
    let manager = get_manager(level)?;
    let label = get_label();

    manager
        .stop(ServiceStopCtx { label })
        .map_err(ServiceError::Manager)?;

    Ok(())
}

/// Get the status of the aranet-service.
///
/// This checks if the service is running by querying the service manager
/// or falling back to checking if the HTTP API is reachable.
pub fn status(level: Level) -> Result<ServiceStatus, ServiceError> {
    let manager = get_manager(level)?;
    let label = get_label();

    // Use the service manager's status method
    match manager.status(ServiceStatusCtx { label }) {
        Ok(status) => match status {
            service_manager::ServiceStatus::Running => Ok(ServiceStatus::Running),
            service_manager::ServiceStatus::Stopped(_) => Ok(ServiceStatus::Stopped),
            service_manager::ServiceStatus::NotInstalled => Ok(ServiceStatus::Stopped),
        },
        Err(_) => {
            // Fall back to checking if the API is reachable
            if is_service_reachable() {
                Ok(ServiceStatus::Running)
            } else {
                Ok(ServiceStatus::Stopped)
            }
        }
    }
}

/// Check if the service API is reachable by attempting a TCP connection.
///
/// Uses the configured bind address from the service config, falling back
/// to the default address if config cannot be loaded.
fn is_service_reachable() -> bool {
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;

    let bind = aranet_service::Config::load_default()
        .map(|c| c.server.bind)
        .unwrap_or_else(|_| "127.0.0.1:8080".to_string());

    let addr: SocketAddr = match bind.parse() {
        Ok(addr) => addr,
        Err(_) => return false,
    };

    TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_ok()
}

/// Service status
#[derive(Debug, Clone, Copy)]
pub enum ServiceStatus {
    Running,
    Stopped,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_install_args_preserves_runtime_options() {
        let db_path = env::temp_dir().join("aranet-test.db");
        let options = aranet_service::RunOptions {
            config: Some(PathBuf::from("config/server.toml")),
            bind: Some("0.0.0.0:9090".to_string()),
            database: Some(db_path.clone()),
            no_collector: true,
        };

        let args = build_install_args(&options).unwrap();
        let cwd = env::current_dir().unwrap();

        assert_eq!(
            args,
            vec![
                OsString::from("run"),
                OsString::from("--config"),
                cwd.join("config/server.toml").into_os_string(),
                OsString::from("--bind"),
                OsString::from("0.0.0.0:9090"),
                OsString::from("--database"),
                db_path.into_os_string(),
                OsString::from("--no-collector"),
            ]
        );
    }

    #[test]
    fn test_build_install_args_omits_unset_options() {
        let args = build_install_args(&aranet_service::RunOptions::default()).unwrap();

        assert_eq!(args, vec![OsString::from("run")]);
    }
}
