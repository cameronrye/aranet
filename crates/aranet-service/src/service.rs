//! Service management for aranet-service.
//!
//! Provides cross-platform service installation/management using the service-manager crate.

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

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
    Manager(String),

    #[error("Could not find aranet-service executable")]
    ExecutableNotFound,

    #[error("User-level services not supported on this platform")]
    UserLevelNotSupported,
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
pub fn install(level: Level) -> Result<(), ServiceError> {
    let manager = get_manager(level)?;
    let program = get_executable_path()?;
    let label = get_label();

    manager
        .install(ServiceInstallCtx {
            label,
            program,
            args: vec![OsString::from("run")],
            contents: None,
            username: None,
            working_directory: None,
            environment: None,
            autostart: true,
            restart_policy: RestartPolicy::OnFailure {
                delay_secs: Some(5),
            },
        })
        .map_err(|e| ServiceError::Manager(e.to_string()))?;

    Ok(())
}

/// Uninstall the aranet-service system service.
pub fn uninstall(level: Level) -> Result<(), ServiceError> {
    let manager = get_manager(level)?;
    let label = get_label();

    manager
        .uninstall(ServiceUninstallCtx { label })
        .map_err(|e| ServiceError::Manager(e.to_string()))?;

    Ok(())
}

/// Start the aranet-service system service.
pub fn start(level: Level) -> Result<(), ServiceError> {
    let manager = get_manager(level)?;
    let label = get_label();

    manager
        .start(ServiceStartCtx { label })
        .map_err(|e| ServiceError::Manager(e.to_string()))?;

    Ok(())
}

/// Stop the aranet-service system service.
pub fn stop(level: Level) -> Result<(), ServiceError> {
    let manager = get_manager(level)?;
    let label = get_label();

    manager
        .stop(ServiceStopCtx { label })
        .map_err(|e| ServiceError::Manager(e.to_string()))?;

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
fn is_service_reachable() -> bool {
    use std::net::TcpStream;
    use std::time::Duration;

    // Try to connect to the default service port
    TcpStream::connect_timeout(&"127.0.0.1:8080".parse().unwrap(), Duration::from_secs(2)).is_ok()
}

/// Service status
#[derive(Debug, Clone, Copy)]
pub enum ServiceStatus {
    Running,
    Stopped,
}
