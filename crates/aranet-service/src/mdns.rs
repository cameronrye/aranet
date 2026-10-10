//! mDNS/DNS-SD service discovery for the Aranet service.
//!
//! This module advertises the Aranet HTTP API on the local network using
//! mDNS (multicast DNS) and DNS-SD (DNS Service Discovery), allowing clients
//! to automatically discover the service without manual IP configuration.
//!
//! The service is advertised as `_aranet._tcp.local.` and also as `_http._tcp.local.`
//! for generic HTTP service discovery.
//!
//! # Discovery from clients
//!
//! ```bash
//! # macOS
//! dns-sd -B _aranet._tcp
//!
//! # Linux (avahi)
//! avahi-browse -r _aranet._tcp
//!
//! # Any platform with Python
//! python3 -m zeroconf browse _aranet._tcp.local.
//! ```

use std::net::SocketAddr;
use std::sync::Arc;

use mdns_sd::{Error as MdnsError, ServiceDaemon, ServiceInfo};
use tracing::{info, warn};

use crate::state::AppState;

/// Handle to the mDNS service daemon. When dropped, the service is unregistered.
pub struct MdnsHandle {
    _daemon: ServiceDaemon,
}

/// mDNS service advertiser.
pub struct MdnsAdvertiser {
    state: Arc<AppState>,
}

impl MdnsAdvertiser {
    /// Create a new mDNS advertiser.
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Start advertising the service via mDNS.
    ///
    /// Returns a handle that keeps the advertisement alive. When the handle
    /// is dropped, the service is unregistered from mDNS.
    pub async fn start(&self) -> Option<MdnsHandle> {
        let config = self.state.config.read().await;
        let bind = config.server.bind.clone();
        drop(config);

        let bind_addr = match bind.parse::<SocketAddr>() {
            Ok(addr) => addr,
            Err(e) => {
                warn!(
                    "Cannot parse bind address '{}' for mDNS advertisement: {}",
                    bind, e
                );
                return None;
            }
        };

        if bind_addr.ip().is_loopback() {
            info!(
                "Skipping mDNS advertisement for loopback-only bind address '{}'",
                bind
            );
            return None;
        }

        let port = bind_addr.port();

        // Create mDNS daemon
        let daemon = match ServiceDaemon::new() {
            Ok(d) => d,
            Err(e) => {
                warn!(
                    "Failed to create mDNS daemon: {}. Service discovery disabled.",
                    e
                );
                return None;
            }
        };

        let hostname = os_hostname().unwrap_or_else(|| "aranet-service".to_string());

        let instance_name = format!("Aranet Service on {}", hostname);

        // Register the aranet-specific service type
        let service_type = "_aranet._tcp.local.";
        let properties = [
            ("version", env!("CARGO_PKG_VERSION")),
            ("path", "/api"),
            ("dashboard", "/dashboard"),
        ];

        match build_service_info(service_type, &instance_name, &hostname, port, &properties) {
            Ok(service_info) => {
                if let Err(e) = daemon.register(service_info) {
                    warn!("Failed to register mDNS service: {}", e);
                } else {
                    info!(
                        "Advertising via mDNS as '{}' on port {}",
                        instance_name, port
                    );
                }
            }
            Err(e) => {
                warn!("Failed to create mDNS service info: {}", e);
            }
        }

        // Also register as a generic HTTP service
        let http_service_type = "_http._tcp.local.";
        match build_service_info(
            http_service_type,
            &instance_name,
            &hostname,
            port,
            &properties,
        ) {
            Ok(service_info) => {
                if let Err(e) = daemon.register(service_info) {
                    warn!("Failed to register HTTP mDNS service: {}", e);
                }
            }
            Err(e) => {
                warn!("Failed to create HTTP mDNS service info: {}", e);
            }
        }

        Some(MdnsHandle { _daemon: daemon })
    }
}

/// This machine's host name as the operating system reports it, for example
/// `Mac.local` or `raspberrypi`.
fn os_hostname() -> Option<String> {
    hostname::get()
        .ok()
        .and_then(|name| name.into_string().ok())
        .filter(|name| !name.is_empty())
}

/// This machine's name as browsers on the local network reach it: the first
/// label of its host name, lowercase (`Mac.local` gives `mac`). The origin
/// guard also accepts `<name>.local`.
pub(crate) fn machine_hostname() -> Option<String> {
    os_hostname().as_deref().and_then(first_label)
}

fn first_label(hostname: &str) -> Option<String> {
    let label = hostname.split('.').next()?.trim().to_ascii_lowercase();
    (!label.is_empty()).then_some(label)
}

fn build_service_info(
    service_type: &str,
    instance_name: &str,
    hostname: &str,
    port: u16,
    properties: &[(&str, &str)],
) -> Result<ServiceInfo, MdnsError> {
    ServiceInfo::new(
        service_type,
        instance_name,
        &format!("{hostname}."),
        "",
        port,
        properties,
    )
    .map(ServiceInfo::enable_addr_auto)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_info_enables_address_auto_population() {
        let service_info = build_service_info(
            "_aranet._tcp.local.",
            "Aranet Service on test-host",
            "test-host",
            8080,
            &[("version", env!("CARGO_PKG_VERSION"))],
        )
        .unwrap();

        assert!(service_info.is_addr_auto());
    }

    #[test]
    fn machine_name_is_first_label_lowercased() {
        assert_eq!(
            first_label("Camerons-Mac.local").as_deref(),
            Some("camerons-mac")
        );
        assert_eq!(first_label("raspberrypi").as_deref(), Some("raspberrypi"));
        assert_eq!(first_label("DESKTOP-1234").as_deref(), Some("desktop-1234"));
        assert_eq!(first_label("host.example.com.").as_deref(), Some("host"));
        assert_eq!(first_label(""), None);
        assert_eq!(first_label(".local"), None);
    }
}
