//! Service management for aranet-service.
//!
//! Provides cross-platform service installation/management using the service-manager crate.

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

use service_manager::{
    RestartPolicy, ServiceInstallCtx, ServiceLabel, ServiceLevel, ServiceManager, ServiceStartCtx,
    ServiceStopCtx, ServiceUninstallCtx,
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
fn get_executable_path() -> Result<PathBuf, ServiceError> {
    env::current_exe().map_err(|_| ServiceError::ExecutableNotFound)
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
pub fn status(level: Level) -> Result<ServiceStatus, ServiceError> {
    let manager = get_manager(level)?;
    let label = get_label();

    // Try to query status - service-manager doesn't have a direct status method,
    // so we check if we can interact with the service
    match manager.stop(ServiceStopCtx {
        label: label.clone(),
    }) {
        Ok(_) => {
            // Was running, start it back up
            let _ = manager.start(ServiceStartCtx { label });
            Ok(ServiceStatus::Running)
        }
        Err(_) => Ok(ServiceStatus::Stopped),
    }
}

/// Service status
#[derive(Debug, Clone, Copy)]
pub enum ServiceStatus {
    Running,
    Stopped,
}
