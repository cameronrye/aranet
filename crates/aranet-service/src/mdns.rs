//! mDNS/DNS-SD service discovery for the Aranet service.
//!
//! This module advertises the Aranet HTTP API on the local network using
//! mDNS (multicast DNS) and DNS-SD (DNS Service Discovery), allowing clients
//! to automatically discover the service without manual IP configuration.
//!
//! The service is advertised as `_aranet._tcp.local.` and also as `_http._tcp.local.`
//! for generic HTTP service discovery, as `Aranet Service on <name>` with the
//! host name `aranet-<name>.local.`, where `<name>` is the first label of this
//! machine's host name. The `aranet-` prefix keeps the service's address
//! records apart from the ones the system's own mDNS responder (avahi,
//! mDNSResponder) announces for `<name>.local`.
//!
//! Nothing is advertised while the service listens on a loopback address only,
//! or while API-key authentication is off: announcing an API that anyone on the
//! network could read and change would invite them to use it.
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

use crate::config::SecurityConfig;
use crate::state::AppState;

/// Handle to the mDNS service daemon. Dropping it doesn't unregister the service.
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
    /// `bound` is the address the HTTP listener is bound to. Nothing is
    /// advertised when it is a loopback address, or while API-key
    /// authentication is off.
    ///
    /// Dropping the returned handle doesn't unregister the service: mdns-sd
    /// keeps advertising it until the process exits and sends no goodbye.
    pub async fn start(&self, bound: SocketAddr) -> Option<MdnsHandle> {
        let advertise = should_advertise(bound, &self.state.config.read().await.security);
        if !advertise {
            if bound.ip().to_canonical().is_loopback() {
                info!("Not advertising over mDNS: the service listens on {bound} only");
            } else {
                info!("Not advertising over mDNS: API-key authentication is off");
            }
            return None;
        }

        let port = bound.port();

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

        let hostname = os_hostname().unwrap_or_default();

        let instance_name = mdns_instance_name(&hostname);

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

/// Whether to advertise a service whose listener is bound to `bound`: only a
/// listener that isn't loopback, with API-key authentication on. An unspecified
/// address (`0.0.0.0`, `[::]`) is advertised with every interface's addresses.
pub(crate) fn should_advertise(bound: SocketAddr, security: &SecurityConfig) -> bool {
    !bound.ip().to_canonical().is_loopback() && security.api_key_enabled
}

/// The longest DNS label, in bytes. mdns-sd writes each `.`-separated part of
/// a name as one label and panics on a longer one.
const MAX_LABEL_LEN: usize = 63;

/// The first label of `hostname`, cut to at most `max_len` bytes on a
/// character boundary: `raspberrypi`, `raspberrypi.local` and
/// `raspberrypi.example.com` all give `raspberrypi`.
fn host_label(hostname: &str, max_len: usize) -> &str {
    let label = hostname.split('.').next().unwrap_or_default();
    let mut end = label.len().min(max_len);
    while !label.is_char_boundary(end) {
        end -= 1;
    }
    &label[..end]
}

/// The mDNS host name the service's addresses are registered under:
/// `aranet-<first label>.local.`, so `raspberrypi` gives
/// `aranet-raspberrypi.local.`.
///
/// `ServiceDaemon::register` refuses a host name that doesn't end in
/// `.local.`. The `aranet-` prefix keeps it apart from the name this machine's
/// own mDNS responder (avahi, mDNSResponder) announces: address records for
/// that name that differ from the responder's make avahi rename the machine.
pub(crate) fn mdns_host_name(hostname: &str) -> String {
    const PREFIX: &str = "aranet-";
    match host_label(hostname, MAX_LABEL_LEN - PREFIX.len()) {
        "" => "aranet-service.local.".to_owned(),
        label => format!("{PREFIX}{label}.local."),
    }
}

/// The service instance name, `Aranet Service on <first label>`. It must be a
/// single DNS label: mdns-sd would send `Aranet Service on Mac.local` as two,
/// which DNS-SD browsers reject.
pub(crate) fn mdns_instance_name(hostname: &str) -> String {
    const PREFIX: &str = "Aranet Service on ";
    match host_label(hostname, MAX_LABEL_LEN - PREFIX.len()) {
        "" => "Aranet Service".to_owned(),
        label => format!("{PREFIX}{label}"),
    }
}

/// Service info for `instance_name` under this machine's `hostname`, as the
/// OS reports it, with the addresses of every interface.
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
        &mdns_host_name(hostname),
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
    fn test_service_info_host_name_ends_in_local() {
        let service_info = build_service_info(
            "_aranet._tcp.local.",
            "Aranet Service on raspberrypi",
            "raspberrypi",
            8080,
            &[("version", env!("CARGO_PKG_VERSION"))],
        )
        .unwrap();

        // ServiceDaemon::register rejects a host name without `.local.`, and
        // the machine's own name belongs to the system's mDNS responder.
        assert_eq!(service_info.get_hostname(), "aranet-raspberrypi.local.");
    }

    #[test]
    fn test_mdns_host_name_table() {
        for (hostname, expected) in [
            ("raspberrypi", "aranet-raspberrypi.local."),
            ("Mac.local", "aranet-Mac.local."),
            ("Mac.LOCAL.", "aranet-Mac.local."),
            ("host.example.com", "aranet-host.local."),
            ("DESKTOP-1234.", "aranet-DESKTOP-1234.local."),
            ("", "aranet-service.local."),
            (".local.", "aranet-service.local."),
        ] {
            assert_eq!(mdns_host_name(hostname), expected, "{hostname:?}");
        }

        // A DNS label is at most 63 bytes, and mdns-sd panics on a longer one
        // (dns_parser.rs:826). The cut falls on a character boundary.
        assert_eq!(
            mdns_host_name(&"a".repeat(63)),
            format!("aranet-{}.local.", "a".repeat(56))
        );
        assert_eq!(
            mdns_host_name(&format!("x{}", "é".repeat(40))),
            format!("aranet-x{}.local.", "é".repeat(27))
        );
    }

    #[test]
    fn test_mdns_instance_name_is_one_label() {
        for (hostname, expected) in [
            ("raspberrypi", "Aranet Service on raspberrypi"),
            ("M3.local", "Aranet Service on M3"),
            ("host.example.com", "Aranet Service on host"),
            ("", "Aranet Service"),
        ] {
            assert_eq!(mdns_instance_name(hostname), expected, "{hostname:?}");
        }
        let long = mdns_instance_name(&"a".repeat(63));
        assert_eq!(long, format!("Aranet Service on {}", "a".repeat(45)));
    }

    #[test]
    fn test_should_advertise() {
        let addr = |text: &str| text.parse::<SocketAddr>().unwrap();
        let authenticated = SecurityConfig {
            api_key_enabled: true,
            api_key: Some("0123456789abcdef".repeat(4)),
            ..Default::default()
        };
        let opted_out = SecurityConfig {
            allow_unauthenticated_network: true,
            ..Default::default()
        };

        for bind in ["127.0.0.1:8080", "[::1]:8080", "[::ffff:127.0.0.1]:8080"] {
            assert!(!should_advertise(addr(bind), &authenticated), "{bind}");
        }
        for bind in ["0.0.0.0:8080", "[::]:8080", "192.168.1.10:8080"] {
            assert!(should_advertise(addr(bind), &authenticated), "{bind}");
            assert!(
                !should_advertise(addr(bind), &opted_out),
                "{bind} without a key"
            );
        }
    }

    /// Authentication alone decides, whatever the opt-out says: the opt-out
    /// allows listening without a key, not announcing it.
    #[test]
    fn test_should_advertise_ignores_the_opt_out() {
        let bound: SocketAddr = "0.0.0.0:8080".parse().unwrap();
        assert!(
            !should_advertise(bound, &SecurityConfig::default()),
            "advertised without authentication"
        );

        let authenticated_and_opted_out = SecurityConfig {
            api_key_enabled: true,
            api_key: Some("0123456789abcdef".repeat(4)),
            allow_unauthenticated_network: true,
            ..Default::default()
        };
        assert!(
            should_advertise(bound, &authenticated_and_opted_out),
            "the opt-out stopped an authenticated service from advertising"
        );
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
