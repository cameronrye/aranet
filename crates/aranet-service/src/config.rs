//! Server configuration.

use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};

use aranet_store::TomlFile;
use serde::{Deserialize, Serialize};

/// Push a validation error onto `$errors` with the given field and message.
macro_rules! validate {
    ($errors:expr, $field:expr, $msg:expr) => {
        $errors.push(ValidationError {
            field: $field.to_string(),
            message: $msg.to_string(),
        })
    };
    ($errors:expr, $field:expr, $($arg:tt)+) => {
        $errors.push(ValidationError {
            field: $field.to_string(),
            message: format!($($arg)+),
        })
    };
}

/// Server configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Server settings.
    pub server: ServerConfig,
    /// Storage settings.
    pub storage: StorageConfig,
    /// Security settings.
    #[serde(default)]
    pub security: SecurityConfig,
    /// Devices to monitor.
    #[serde(default)]
    pub devices: Vec<DeviceConfig>,
    /// Prometheus metrics settings.
    #[serde(default)]
    pub prometheus: PrometheusConfig,
    /// MQTT publisher settings.
    #[serde(default)]
    pub mqtt: MqttConfig,
    /// Desktop notification settings.
    #[serde(default)]
    pub notifications: NotificationConfig,
    /// Webhook notification settings.
    #[serde(default)]
    pub webhooks: WebhookConfig,
    /// InfluxDB export settings.
    #[serde(default)]
    pub influxdb: InfluxDbConfig,
}

impl Config {
    /// Load configuration from the default path.
    pub fn load_default() -> Result<Self, ConfigError> {
        let path = default_config_path();
        if path.exists() {
            Self::load(&path)
        } else {
            Ok(Self::default())
        }
    }

    /// Load configuration from a file.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path.as_ref()).map_err(|e| ConfigError::Read {
            path: path.as_ref().to_path_buf(),
            source: e,
        })?;
        toml::from_str(&content).map_err(|e| ConfigError::Parse {
            path: path.as_ref().to_path_buf(),
            source: e,
        })
    }

    /// Write this configuration to `path`, changing only the keys whose values
    /// differ from what the file holds.
    ///
    /// Comments, key order and keys this version doesn't know are kept, and
    /// settings still at their defaults are not added. A missing file is
    /// created only when something differs from the defaults. The file is
    /// replaced atomically and is readable only by its owner (0600 on Unix). A
    /// file that doesn't parse is an error and is left unchanged.
    ///
    /// Everything in `self` that differs from the file is written, including
    /// command-line overrides held in a running configuration. A running service
    /// changes its file with [`Config::save_devices`] or [`Config::update_file`].
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), ConfigError> {
        let path = path.as_ref();
        TomlFile::<Config>::open(path)
            .and_then(|file| file.save(self))
            .map(|_| ())
            .map_err(|e| config_file_error(path, e))
    }

    /// Apply `change` to the configuration stored at `path` (not to a running
    /// configuration) and write back only the keys it changed.
    ///
    /// A missing file reads as the defaults. Returns whether the file was
    /// written: `false` means nothing changed and the file was not touched. A
    /// file that doesn't parse is an error and is left unchanged.
    pub fn update_file<P: AsRef<Path>>(
        path: P,
        change: impl FnOnce(&mut Config),
    ) -> Result<bool, ConfigError> {
        let path = path.as_ref();
        let file = TomlFile::<Config>::open(path).map_err(|e| config_file_error(path, e))?;
        let mut desired = file.value().clone();
        change(&mut desired);
        file.save(&desired).map_err(|e| config_file_error(path, e))
    }

    /// Store `devices` as the device list in the file at `path`, leaving every
    /// other setting as the file has it.
    ///
    /// The API saves device changes through this, because the running
    /// configuration differs from the file (`--bind`, `--database`, defaults) and
    /// none of that belongs in it. The whole list is written, so it replaces
    /// edits made to `[[devices]]` by hand while the service runs.
    pub fn save_devices<P: AsRef<Path>>(
        path: P,
        devices: &[DeviceConfig],
    ) -> Result<(), ConfigError> {
        Self::update_file(path, |config| config.devices = devices.to_vec()).map(|_| ())
    }

    /// Validate the configuration and return any errors.
    ///
    /// This checks:
    /// - Server bind address is an IP address and port (host names are rejected)
    /// - Storage path is not empty
    /// - Retention limits, when set, are 1 to 36500 days
    /// - Device addresses are not empty
    /// - Device addresses and aliases have no control characters or
    ///   backslashes and are at most [`MAX_DEVICE_TEXT_CHARS`] characters long
    /// - Device poll intervals are within reasonable bounds (10s - 1 hour)
    /// - No duplicate device addresses
    /// - When enabled, the MQTT topic prefix (and, with Home Assistant
    ///   discovery on, the discovery prefix) has no control characters or
    ///   wildcards, and the InfluxDB measurement name has no control characters
    ///   or backslashes and doesn't start with `_` or `#`
    /// - A non-loopback bind address has API-key authentication, unless
    ///   `security.allow_unauthenticated_network` is set
    ///
    /// # Example
    ///
    /// ```
    /// use aranet_service::Config;
    ///
    /// let config = Config::default();
    /// config.validate().expect("Default config should be valid");
    /// ```
    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut errors = Vec::new();

        // Validate server config
        errors.extend(self.server.validate());

        // Validate storage config
        errors.extend(self.storage.validate());

        // Validate security config
        errors.extend(self.security.validate());

        // Never serve an unauthenticated API beyond this machine by accident.
        // `to_canonical` makes an IPv4-mapped loopback (`[::ffff:127.0.0.1]`)
        // count as loopback. A bind that isn't an `IP:port` has its own error.
        if let Some(addr) = self.server.socket_addr()
            && !addr.ip().to_canonical().is_loopback()
            && !self.security.api_key_enabled
            && !self.security.allow_unauthenticated_network
        {
            validate!(
                errors,
                "server.bind",
                "refusing to listen on {} without API-key authentication: set \
                 security.api_key_enabled = true, bind to 127.0.0.1, or set \
                 security.allow_unauthenticated_network = true if a firewall or \
                 authenticating proxy protects the port",
                self.server.bind
            );
        }

        // Validate devices
        let mut seen_addresses = std::collections::HashSet::new();
        for (i, device) in self.devices.iter().enumerate() {
            let prefix = format!("devices[{}]", i);
            errors.extend(device.validate(&prefix));

            // Check for duplicate addresses
            let addr_lower = device.address.to_lowercase();
            if !seen_addresses.insert(addr_lower.clone()) {
                validate!(
                    errors,
                    format!("{}.address", prefix),
                    "duplicate device address '{}'",
                    device.address
                );
            }
        }

        // Validate Prometheus config
        errors.extend(self.prometheus.validate());

        // Validate MQTT config
        errors.extend(self.mqtt.validate());

        // Validate webhook config
        errors.extend(self.webhooks.validate());

        // Validate InfluxDB config
        errors.extend(self.influxdb.validate());

        if errors.is_empty() {
            Ok(())
        } else {
            Err(ConfigError::Validation(errors))
        }
    }

    /// Load and validate configuration from a file.
    ///
    /// This is a convenience method that combines `load()` and `validate()`.
    pub fn load_validated<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let config = Self::load(path)?;
        config.validate()?;
        Ok(config)
    }
}

/// Server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// IP address and port to listen on, such as `127.0.0.1:8080` or
    /// `[::1]:8080`. Host names such as `localhost` are rejected.
    pub bind: String,
    /// Broadcast channel buffer size for real-time reading updates.
    ///
    /// This determines how many messages can be buffered before slow
    /// subscribers start missing messages. A larger buffer uses more memory
    /// but is more tolerant of slow WebSocket clients.
    ///
    /// Default: 100
    #[serde(default = "default_broadcast_buffer")]
    pub broadcast_buffer: usize,
}

/// Default broadcast buffer size.
pub const DEFAULT_BROADCAST_BUFFER: usize = 100;

fn default_broadcast_buffer() -> usize {
    DEFAULT_BROADCAST_BUFFER
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8080".to_string(),
            broadcast_buffer: DEFAULT_BROADCAST_BUFFER,
        }
    }
}

impl ServerConfig {
    /// The bind address as a socket address, or `None` when it isn't an
    /// `IP:port` pair. Host names such as `localhost` give `None`: the listener
    /// binds exactly this address and never resolves names.
    pub fn socket_addr(&self) -> Option<SocketAddr> {
        self.bind.parse().ok()
    }

    /// Validate server configuration.
    ///
    /// `bind` must be an IP address and a port from 1 to 65535, such as
    /// `127.0.0.1:8080` or `[::1]:8080`. A bad `bind` gives exactly one error.
    pub fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        match self.bind.parse::<SocketAddr>() {
            Ok(addr) if addr.port() == 0 => {
                validate!(errors, "server.bind", "port cannot be 0");
            }
            Ok(_) => {}
            Err(_) if self.bind.is_empty() => {
                validate!(errors, "server.bind", "bind address cannot be empty");
            }
            Err(_) => {
                validate!(
                    errors,
                    "server.bind",
                    "invalid bind address '{}': expected IP:port such as 127.0.0.1:8080 or \
                     [::1]:8080 (host names such as 'localhost' are not resolved)",
                    self.bind
                );
            }
        }

        if self.broadcast_buffer == 0 {
            validate!(
                errors,
                "server.broadcast_buffer",
                "broadcast buffer must be greater than 0"
            );
        } else if self.broadcast_buffer > 10_000 {
            validate!(
                errors,
                "server.broadcast_buffer",
                "broadcast buffer {} exceeds maximum of 10000",
                self.broadcast_buffer
            );
        }

        errors
    }
}

/// Storage configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Database file path. A leading `~` is expanded to the home directory
    /// when the service loads its configuration.
    pub path: PathBuf,
    /// Deletion of old rows (`[storage.retention]`). Off by default, and not
    /// written to the file while off.
    #[serde(default, skip_serializing_if = "RetentionConfig::is_disabled")]
    pub retention: RetentionConfig,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            path: aranet_store::default_db_path(),
            retention: RetentionConfig::default(),
        }
    }
}

impl StorageConfig {
    /// Validate storage configuration.
    pub fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        if self.path.as_os_str().is_empty() {
            validate!(errors, "storage.path", "database path cannot be empty");
        }

        errors.extend(self.retention.validate());
        errors
    }
}

/// Longest accepted retention limit, in days (100 years).
pub const MAX_RETENTION_DAYS: u32 = 36_500;

/// Data retention settings (`[storage.retention]`).
///
/// Retention is off by default: the service keeps every reading and history
/// record until they are deleted by hand (`aranet cache prune`). With a limit
/// set, the service deletes older rows at startup and then every hour, in
/// small batches. It never runs `VACUUM`, so the database file stops growing
/// but doesn't shrink. To shrink it, stop the service and run
/// `sqlite3 <database> 'VACUUM;'`. (`aranet cache prune --vacuum` shrinks it
/// too, but it first deletes cached history, and readings unless
/// `--history-only` is given, older than its `--older-than` age, whatever
/// these limits say.) Changes take effect when the service restarts.
///
/// ```toml
/// [storage.retention]
/// readings_days = 365
/// history_days = 730
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RetentionConfig {
    /// Delete readings once they are older than this many days. This covers
    /// every reading in the database, including those the `aranet` CLI, TUI
    /// and GUI stored there; they share it unless the service has its own
    /// database, as under the systemd system service and Docker. Unset keeps
    /// them forever.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readings_days: Option<u32>,
    /// Delete cached device history (shared with `aranet sync`) once it is
    /// older than this many days. Unset keeps it forever. The next sync may
    /// download deleted records again while the device still holds them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_days: Option<u32>,
}

impl RetentionConfig {
    /// Whether any retention limit is set.
    pub fn is_enabled(&self) -> bool {
        self.readings_days.is_some() || self.history_days.is_some()
    }

    /// Whether no retention limit is set (the default).
    pub fn is_disabled(&self) -> bool {
        !self.is_enabled()
    }

    /// Validate the limits: each one that is set must be 1 to
    /// [`MAX_RETENTION_DAYS`] days.
    pub fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        for (field, days) in [
            ("storage.retention.readings_days", self.readings_days),
            ("storage.retention.history_days", self.history_days),
        ] {
            match days {
                Some(0) => validate!(
                    errors,
                    field,
                    "must be at least 1 day (remove the key to keep data forever)"
                ),
                Some(days) if days > MAX_RETENTION_DAYS => {
                    validate!(errors, field, "must be at most {} days", MAX_RETENTION_DAYS)
                }
                _ => {}
            }
        }

        errors
    }
}

/// Security configuration for API protection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    /// Enable API key authentication.
    /// When enabled, clients must provide the API key in the `X-API-Key` header
    /// or as `Authorization: Bearer <key>`.
    pub api_key_enabled: bool,
    /// The API key required for authentication (if enabled).
    /// Should be a secure random string of at least 32 characters.
    ///
    /// If authentication is enabled and no key is set, the service generates
    /// one ([`generate_api_key`]) when it starts and saves it to its config
    /// file.
    pub api_key: Option<String>,
    /// Allow listening on a non-loopback address without API-key authentication.
    ///
    /// Only for ports protected by a firewall or an authenticating reverse
    /// proxy. Without it, [`Config::validate`] refuses a `server.bind` that isn't
    /// a loopback address while `api_key_enabled` is false. The service logs a
    /// warning at startup when this setting is what lets it listen.
    #[serde(default)]
    pub allow_unauthenticated_network: bool,
    /// Enable rate limiting, per client: an IPv4 address or an IPv6 /64.
    /// IPv6 clients on one network share one budget, and so do all
    /// link-local clients (`fe80::/64`): each group is one /64.
    pub rate_limit_enabled: bool,
    /// Requests a client may send at once. After that, one more request is
    /// allowed every `rate_limit_window_secs / rate_limit_requests`, so a
    /// client that keeps sending gets this many requests per window. Must be
    /// greater than 0.
    #[serde(default = "default_rate_limit_requests")]
    pub rate_limit_requests: u32,
    /// Seconds for a client's whole burst to refill, from 1 to 86400 (one
    /// day).
    #[serde(default = "default_rate_limit_window")]
    pub rate_limit_window_secs: u64,
    /// Number of tracked clients above which requests also forget idle
    /// clients, at most once per refill period. Must be at least 1.
    ///
    /// A client is forgotten once its budget has been full again for one
    /// refill period: at most one window plus one period after its last
    /// request. The service also sweeps idle clients once per window. A
    /// throttled client is never forgotten, so more clients than this are
    /// tracked while more than this are active.
    #[serde(default = "default_rate_limit_max_entries")]
    pub rate_limit_max_entries: usize,
    /// Browser origins, other than the service's own, that may call the API.
    ///
    /// Empty by default. While API-key authentication is off, a request whose
    /// `Origin` header is neither the service's own origin nor listed here is
    /// refused with `403 Forbidden`, WebSocket connections included, and
    /// `Origin: null` is always refused. Listed origins also get CORS headers.
    /// Origins are compared as browsers send them: lowercase, with no trailing
    /// `/`. `["*"]` allows every origin, which lets any web page you visit
    /// control a service without authentication; the service warns about it
    /// at startup.
    ///
    /// Example: `["http://localhost:3000"]`
    #[serde(default = "default_cors_origins")]
    pub cors_origins: Vec<String>,
    /// Extra host names the service answers while API-key authentication is
    /// off.
    ///
    /// Without authentication, a request whose `Host` names any other host is
    /// refused with `421 Misdirected Request`, so a web page can't reach the
    /// service through DNS rebinding. `localhost` and `*.localhost`, IP
    /// addresses, this machine's name and `<name>.local` are always allowed.
    /// This machine's name is the first label of its host name (`server1` for
    /// `server1.example.com`), so a fully qualified name has to be listed.
    /// List bare host names, without scheme, port, spaces or wildcards: for
    /// example `["aranet-service"]` for a Docker Compose service name.
    /// `["*"]` disables the check; the service warns about it at startup.
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
}

fn default_rate_limit_requests() -> u32 {
    100
}

fn default_rate_limit_window() -> u64 {
    60
}

fn default_rate_limit_max_entries() -> usize {
    10_000
}

/// Longest accepted `security.rate_limit_window_secs`: one day. The rate
/// limiter counts time in `u64` nanoseconds, which a window of centuries
/// overflows.
pub(crate) const MAX_RATE_LIMIT_WINDOW_SECS: u64 = 86_400;

fn default_cors_origins() -> Vec<String> {
    Vec::new()
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            api_key_enabled: false,
            api_key: None,
            allow_unauthenticated_network: false,
            // Rate limiting enabled by default to prevent DoS attacks
            rate_limit_enabled: true,
            rate_limit_requests: default_rate_limit_requests(),
            rate_limit_window_secs: default_rate_limit_window(),
            rate_limit_max_entries: default_rate_limit_max_entries(),
            cors_origins: default_cors_origins(),
            allowed_hosts: Vec::new(),
        }
    }
}

impl SecurityConfig {
    /// Validate security configuration.
    pub fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        if self.api_key_enabled {
            match &self.api_key {
                None => {
                    validate!(
                        errors,
                        "security.api_key",
                        "API key must be set when authentication is enabled"
                    );
                }
                Some(key) if key.len() < 32 => {
                    validate!(
                        errors,
                        "security.api_key",
                        "API key must be at least 32 characters for security"
                    );
                }
                _ => {}
            }
        }

        if self.rate_limit_enabled {
            if self.rate_limit_requests == 0 {
                validate!(
                    errors,
                    "security.rate_limit_requests",
                    "rate limit requests must be greater than 0"
                );
            }
            if self.rate_limit_window_secs < 1 {
                validate!(
                    errors,
                    "security.rate_limit_window_secs",
                    "rate limit window must be at least 1 second"
                );
            }
            if self.rate_limit_window_secs > MAX_RATE_LIMIT_WINDOW_SECS {
                validate!(
                    errors,
                    "security.rate_limit_window_secs",
                    "rate limit window {} exceeds maximum of {} seconds",
                    self.rate_limit_window_secs,
                    MAX_RATE_LIMIT_WINDOW_SECS
                );
            }
            if self.rate_limit_max_entries < 1 {
                validate!(
                    errors,
                    "security.rate_limit_max_entries",
                    "rate limit max entries must be at least 1"
                );
            }
        }

        for host in &self.allowed_hosts {
            if host != "*"
                && (host.is_empty()
                    || host.contains(char::is_whitespace)
                    || host.contains([':', '/', '@', '*']))
            {
                validate!(
                    errors,
                    "security.allowed_hosts",
                    "invalid host name '{}': list bare host names without scheme, port, spaces or wildcards, such as \"aranet-service\" (localhost, IP addresses and this machine's name are always allowed)",
                    host
                );
            }
        }

        errors
    }
}

/// Prometheus metrics configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PrometheusConfig {
    /// Whether Prometheus metrics endpoint is enabled.
    pub enabled: bool,
    /// Optional push gateway URL for pushing metrics.
    /// If not set, metrics are only available via the /metrics endpoint.
    pub push_gateway: Option<String>,
    /// Push interval in seconds (only used with push_gateway).
    #[serde(default = "default_push_interval")]
    pub push_interval: u64,
}

fn default_push_interval() -> u64 {
    60
}

impl Default for PrometheusConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            push_gateway: None,
            push_interval: default_push_interval(),
        }
    }
}

impl PrometheusConfig {
    /// Validate Prometheus configuration.
    pub fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        if let Some(url) = &self.push_gateway
            && url.is_empty()
        {
            validate!(
                errors,
                "prometheus.push_gateway",
                "push gateway URL cannot be empty (use null/omit instead)"
            );
        }

        if self.push_interval < 10 {
            validate!(
                errors,
                "prometheus.push_interval",
                "push interval {} is too short (minimum 10 seconds)",
                self.push_interval
            );
        }

        errors
    }
}

/// MQTT publisher configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MqttConfig {
    /// Whether MQTT publishing is enabled.
    pub enabled: bool,
    /// MQTT broker URL (e.g., "mqtt://localhost:1883" or "mqtts://broker.example.com:8883").
    pub broker: String,
    /// Topic prefix for published messages (e.g., "aranet" -> "aranet/{device}/co2").
    ///
    /// While MQTT is enabled, the prefix can't contain control characters or
    /// the MQTT wildcards `+` and `#`.
    #[serde(default = "default_topic_prefix")]
    pub topic_prefix: String,
    /// MQTT client ID.
    #[serde(default = "default_client_id")]
    pub client_id: String,
    /// Quality of Service level (0 = AtMostOnce, 1 = AtLeastOnce, 2 = ExactlyOnce).
    #[serde(default = "default_qos")]
    pub qos: u8,
    /// Whether to retain messages on the broker.
    #[serde(default)]
    pub retain: bool,
    /// Optional username for authentication.
    pub username: Option<String>,
    /// Optional password for authentication.
    pub password: Option<String>,
    /// Keep-alive interval in seconds.
    #[serde(default = "default_keep_alive")]
    pub keep_alive: u64,
    /// Enable Home Assistant MQTT auto-discovery.
    /// When enabled, discovery messages are published to the HA discovery topic
    /// so devices appear automatically in Home Assistant.
    #[serde(default)]
    pub homeassistant: bool,
    /// Home Assistant discovery topic prefix.
    ///
    /// While MQTT is enabled and `homeassistant` is `true`, the prefix can't
    /// contain control characters or the MQTT wildcards `+` and `#`.
    #[serde(default = "default_ha_discovery_prefix")]
    pub ha_discovery_prefix: String,
}

fn default_topic_prefix() -> String {
    "aranet".to_string()
}

fn default_client_id() -> String {
    "aranet-service".to_string()
}

fn default_qos() -> u8 {
    1
}

fn default_keep_alive() -> u64 {
    60
}

fn default_ha_discovery_prefix() -> String {
    "homeassistant".to_string()
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            broker: "mqtt://localhost:1883".to_string(),
            topic_prefix: default_topic_prefix(),
            client_id: default_client_id(),
            qos: default_qos(),
            retain: false,
            username: None,
            password: None,
            keep_alive: default_keep_alive(),
            homeassistant: false,
            ha_discovery_prefix: default_ha_discovery_prefix(),
        }
    }
}

impl MqttConfig {
    /// Validate MQTT configuration.
    pub fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        if self.enabled {
            if self.broker.is_empty() {
                validate!(
                    errors,
                    "mqtt.broker",
                    "broker URL cannot be empty when MQTT is enabled"
                );
            } else if !self.broker.starts_with("mqtt://") && !self.broker.starts_with("mqtts://") {
                validate!(
                    errors,
                    "mqtt.broker",
                    "invalid broker URL '{}': must start with mqtt:// or mqtts://",
                    self.broker
                );
            }

            // rumqttc refuses to publish to a topic containing a wildcard, and
            // MQTT forbids U+0000 in topic names.
            let unusable = |prefix: &str| {
                prefix
                    .chars()
                    .any(|c| c == '#' || c == '+' || is_forbidden_text_char(c))
            };
            if self.topic_prefix.is_empty() {
                validate!(errors, "mqtt.topic_prefix", "topic prefix cannot be empty");
            } else if unusable(&self.topic_prefix) {
                validate!(
                    errors,
                    "mqtt.topic_prefix",
                    "topic prefix cannot contain control characters or MQTT wildcards"
                );
            }
            // The discovery prefix is only used with Home Assistant discovery on.
            if self.homeassistant && unusable(&self.ha_discovery_prefix) {
                validate!(
                    errors,
                    "mqtt.ha_discovery_prefix",
                    "Home Assistant discovery prefix cannot contain control characters or MQTT wildcards"
                );
            }

            if self.client_id.is_empty() {
                validate!(errors, "mqtt.client_id", "client ID cannot be empty");
            }

            if self.qos > 2 {
                validate!(
                    errors,
                    "mqtt.qos",
                    "invalid QoS level {}: must be 0, 1, or 2",
                    self.qos
                );
            }

            if self.keep_alive < 5 {
                validate!(
                    errors,
                    "mqtt.keep_alive",
                    "keep-alive interval {} is too short (minimum 5 seconds)",
                    self.keep_alive
                );
            }
        }

        errors
    }
}

/// Configuration for a device to monitor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    /// Device address or name. At most [`MAX_DEVICE_TEXT_CHARS`] characters,
    /// with no control characters or backslashes.
    pub address: String,
    /// Friendly alias for the device. At most [`MAX_DEVICE_TEXT_CHARS`]
    /// characters, with no control characters or backslashes.
    #[serde(default)]
    pub alias: Option<String>,
    /// Poll interval in seconds.
    #[serde(default = "default_poll_interval")]
    pub poll_interval: u64,
}

/// Minimum poll interval in seconds (10 seconds).
pub const MIN_POLL_INTERVAL: u64 = 10;
/// Maximum poll interval in seconds (1 hour).
pub const MAX_POLL_INTERVAL: u64 = 3600;
/// Maximum length of a device address or alias, in characters (not bytes).
pub const MAX_DEVICE_TEXT_CHARS: usize = 64;

fn default_poll_interval() -> u64 {
    60
}

/// Whether `c` is refused in device addresses and aliases (and in the InfluxDB
/// measurement name and the MQTT topic prefixes).
///
/// These are the control characters (`char::is_control`: U+0000-U+001F and
/// U+007F-U+009F), the line and paragraph separators U+2028/U+2029, and the
/// bidi embedding, override and isolate controls U+202A-U+202E and
/// U+2066-U+2069. A newline would split an InfluxDB line or a log line in two,
/// and a bidi control can make a name or a log line read differently from what
/// it is. Everything else is allowed here: real names contain `+`
/// (`AranetRn+ 306B8`) and non-ASCII characters (U+2622 in Aranet Radiation
/// names), and aliases are free text such as `Küche`.
fn is_forbidden_text_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        )
}

/// Check a device address or alias: no forbidden characters (see
/// `is_forbidden_text_char`), no backslash and at most
/// [`MAX_DEVICE_TEXT_CHARS`] characters.
///
/// Backslashes are refused because InfluxDB 1.x and 2.x can't store one in a
/// tag faithfully: they keep an escaped `\\` as two backslashes, and they read a
/// separator after any backslash as escaped. InfluxDB 3 Core also stores a
/// value that ends in a backslash wrongly. The [`crate::influxdb`] module docs
/// have what was measured on each.
fn validate_device_text(field: String, what: &str, value: &str, errors: &mut Vec<ValidationError>) {
    if value.chars().any(is_forbidden_text_char) {
        errors.push(ValidationError {
            field: field.clone(),
            message: format!("{what} cannot contain control characters"),
        });
    }
    if value.contains('\\') {
        errors.push(ValidationError {
            field: field.clone(),
            message: format!("{what} cannot contain backslashes"),
        });
    }
    let chars = value.chars().count();
    if chars > MAX_DEVICE_TEXT_CHARS {
        errors.push(ValidationError {
            field,
            message: format!(
                "{what} is too long ({chars} characters, maximum {MAX_DEVICE_TEXT_CHARS})"
            ),
        });
    }
}

impl DeviceConfig {
    /// Validate device configuration.
    pub fn validate(&self, prefix: &str) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        // Address validation
        if self.address.is_empty() {
            validate!(
                errors,
                format!("{}.address", prefix),
                "device address cannot be empty"
            );
        } else if self.address.len() < 3 {
            validate!(
                errors,
                format!("{}.address", prefix),
                "device address '{}' is too short (minimum 3 characters)",
                self.address
            );
        }
        validate_device_text(
            format!("{prefix}.address"),
            "device address",
            &self.address,
            &mut errors,
        );

        // Alias validation (if provided)
        if let Some(alias) = &self.alias {
            if alias.is_empty() {
                validate!(
                    errors,
                    format!("{}.alias", prefix),
                    "alias cannot be empty string (use null/omit instead)"
                );
            } else {
                validate_device_text(format!("{prefix}.alias"), "alias", alias, &mut errors);
            }
        }

        // Poll interval validation
        if self.poll_interval < MIN_POLL_INTERVAL {
            validate!(
                errors,
                format!("{}.poll_interval", prefix),
                "poll interval {} is too short (minimum {} seconds)",
                self.poll_interval,
                MIN_POLL_INTERVAL
            );
        } else if self.poll_interval > MAX_POLL_INTERVAL {
            validate!(
                errors,
                format!("{}.poll_interval", prefix),
                "poll interval {} is too long (maximum {} seconds / 1 hour)",
                self.poll_interval,
                MAX_POLL_INTERVAL
            );
        }

        errors
    }
}

/// Desktop notification settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationConfig {
    /// Whether desktop notifications are enabled.
    pub enabled: bool,
    /// CO2 threshold in ppm (notify when reached or exceeded).
    #[serde(default = "default_co2_threshold")]
    pub co2_threshold: u16,
    /// Radon threshold in Bq/m³ (notify when reached or exceeded).
    #[serde(default = "default_radon_threshold")]
    pub radon_threshold: u32,
    /// Minimum interval between notifications per device (in seconds).
    #[serde(default = "default_notification_cooldown")]
    pub cooldown_secs: u64,
}

fn default_co2_threshold() -> u16 {
    1000
}

fn default_radon_threshold() -> u32 {
    300
}

fn default_notification_cooldown() -> u64 {
    300
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            co2_threshold: default_co2_threshold(),
            radon_threshold: default_radon_threshold(),
            cooldown_secs: default_notification_cooldown(),
        }
    }
}

/// Webhook notification configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebhookConfig {
    /// Whether webhook notifications are enabled.
    pub enabled: bool,
    /// CO2 threshold in ppm (triggers "co2_high" event).
    #[serde(default = "default_co2_threshold")]
    pub co2_threshold: u16,
    /// Radon threshold in Bq/m³ (triggers "radon_high" event).
    #[serde(default = "default_radon_threshold")]
    pub radon_threshold: u32,
    /// Battery threshold in % (triggers "battery_low" event when at or below).
    #[serde(default = "default_battery_threshold")]
    pub battery_threshold: u8,
    /// Minimum interval between alerts per device per event type (in seconds).
    #[serde(default = "default_webhook_cooldown")]
    pub cooldown_secs: u64,
    /// Webhook endpoints to notify.
    #[serde(default)]
    pub endpoints: Vec<WebhookEndpoint>,
}

fn default_battery_threshold() -> u8 {
    10
}

fn default_webhook_cooldown() -> u64 {
    300
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            co2_threshold: default_co2_threshold(),
            radon_threshold: default_radon_threshold(),
            battery_threshold: default_battery_threshold(),
            cooldown_secs: default_webhook_cooldown(),
            endpoints: Vec::new(),
        }
    }
}

impl WebhookConfig {
    /// Validate webhook configuration.
    pub fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        if self.enabled && self.endpoints.is_empty() {
            validate!(
                errors,
                "webhooks.endpoints",
                "at least one endpoint must be configured when webhooks are enabled"
            );
        }

        for (i, endpoint) in self.endpoints.iter().enumerate() {
            let prefix = format!("webhooks.endpoints[{}]", i);
            if endpoint.url.is_empty() {
                validate!(errors, format!("{}.url", prefix), "URL cannot be empty");
            } else if !endpoint.url.starts_with("http://") && !endpoint.url.starts_with("https://")
            {
                validate!(
                    errors,
                    format!("{}.url", prefix),
                    "URL must start with http:// or https://"
                );
            }
            if endpoint.events.is_empty() {
                validate!(
                    errors,
                    format!("{}.events", prefix),
                    "at least one event type must be specified"
                );
            }
            for event in &endpoint.events {
                if !["co2_high", "radon_high", "battery_low"].contains(&event.as_str()) {
                    validate!(
                        errors,
                        format!("{}.events", prefix),
                        "unknown event type '{}' (valid: co2_high, radon_high, battery_low)",
                        event
                    );
                }
            }
        }

        if self.cooldown_secs < 10 {
            validate!(
                errors,
                "webhooks.cooldown_secs",
                "cooldown {} is too short (minimum 10 seconds)",
                self.cooldown_secs
            );
        }

        errors
    }
}

/// A webhook endpoint configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEndpoint {
    /// The URL to POST alerts to.
    pub url: String,
    /// Event types to send to this endpoint.
    /// Valid values: "co2_high", "radon_high", "battery_low"
    pub events: Vec<String>,
    /// Optional HTTP headers to include in requests (e.g., authorization tokens).
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,
}

/// InfluxDB export configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InfluxDbConfig {
    /// Whether InfluxDB export is enabled.
    pub enabled: bool,
    /// InfluxDB server URL (e.g., "http://localhost:8086").
    pub url: String,
    /// InfluxDB API token for authentication.
    pub token: Option<String>,
    /// Organization name (InfluxDB 2.x).
    pub org: String,
    /// Bucket name (InfluxDB 2.x) or database name (1.x).
    pub bucket: String,
    /// Measurement name for sensor readings.
    ///
    /// Write the name plain (`air quality`, not `air\ quality`): its spaces and
    /// commas are escaped for you. While the exporter is enabled, the name
    /// can't contain control characters or backslashes and can't start with
    /// `_` or `#`.
    #[serde(default = "default_influxdb_measurement")]
    pub measurement: String,
    /// Write precision.
    #[serde(default = "default_influxdb_precision")]
    pub precision: String,
}

fn default_influxdb_measurement() -> String {
    "aranet".to_string()
}

fn default_influxdb_precision() -> String {
    "s".to_string()
}

impl Default for InfluxDbConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: "http://localhost:8086".to_string(),
            token: None,
            org: String::new(),
            bucket: "aranet".to_string(),
            measurement: default_influxdb_measurement(),
            precision: default_influxdb_precision(),
        }
    }
}

impl InfluxDbConfig {
    /// Validate InfluxDB configuration.
    pub fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        if self.enabled {
            if self.url.trim().is_empty() {
                validate!(
                    errors,
                    "influxdb.url",
                    "URL cannot be empty when InfluxDB export is enabled"
                );
            }
            if self.org.trim().is_empty() {
                validate!(
                    errors,
                    "influxdb.org",
                    "organization cannot be empty when InfluxDB export is enabled"
                );
            }
            if self.bucket.trim().is_empty() {
                validate!(errors, "influxdb.bucket", "bucket name cannot be empty");
            }
            if self.measurement.trim().is_empty() {
                validate!(
                    errors,
                    "influxdb.measurement",
                    "measurement name cannot be empty"
                );
            } else {
                if self.measurement.chars().any(is_forbidden_text_char) {
                    validate!(
                        errors,
                        "influxdb.measurement",
                        "measurement name cannot contain control characters"
                    );
                }
                // InfluxDB 1.x and 2.x can't store a backslash in a measurement
                // faithfully, and 3 Core also stores one that ends in a
                // backslash wrongly (see `validate_device_text` and the
                // `influxdb` module docs).
                if self.measurement.contains('\\') {
                    validate!(
                        errors,
                        "influxdb.measurement",
                        "measurement name cannot contain backslashes"
                    );
                }
                if self.measurement.starts_with('_') {
                    validate!(
                        errors,
                        "influxdb.measurement",
                        "measurement name cannot start with '_' (reserved by InfluxDB)"
                    );
                }
                // The measurement starts the line, and InfluxDB reads a line
                // that starts with `#` as a comment: 1.8.10 and 2.7.12 answer
                // 204 and store nothing. No escape exists, so it is refused.
                if self.measurement.starts_with('#') {
                    validate!(
                        errors,
                        "influxdb.measurement",
                        "measurement name cannot start with '#' (InfluxDB reads the line as a comment)"
                    );
                }
            }
            if !["s", "ms", "us", "ns"].contains(&self.precision.as_str()) {
                validate!(
                    errors,
                    "influxdb.precision",
                    "invalid precision '{}' (valid: s, ms, us, ns)",
                    self.precision
                );
            }
        }

        errors
    }
}

/// Configuration errors.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Failed to read config file {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Failed to parse config file {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("Failed to serialize config: {0}")]
    Serialize(toml::ser::Error),
    #[error("Failed to write config file {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Configuration validation failed:\n{}", format_validation_errors(.0))]
    Validation(Vec<ValidationError>),
}

/// The [`ConfigError`] for a failed read or write of the config file at `path`.
fn config_file_error(path: &Path, error: aranet_store::TomlFileError) -> ConfigError {
    use aranet_store::TomlFileError;
    match error {
        TomlFileError::Read { path, source } => ConfigError::Read { path, source },
        TomlFileError::Parse { path, source } => ConfigError::Parse { path, source },
        TomlFileError::Serialize(source) => ConfigError::Serialize(source),
        TomlFileError::Write { path, source } => ConfigError::Write { path, source },
        // `TomlFileError` is `#[non_exhaustive]`. The error itself, not its
        // text, so a future variant keeps its chain of sources.
        other => ConfigError::Write {
            path: path.to_path_buf(),
            source: std::io::Error::other(other),
        },
    }
}

/// A single validation error with context.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{field}: {message}")]
pub struct ValidationError {
    /// The field path (e.g., `server.bind` or `devices[0].address`).
    pub field: String,
    /// Description of the validation failure.
    pub message: String,
}

fn format_validation_errors(errors: &[ValidationError]) -> String {
    errors
        .iter()
        .map(|e| format!("  - {}", e))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Generate an API key: 32 random bytes from the operating system, as 64
/// lowercase hexadecimal characters.
///
/// # Errors
///
/// Fails only when the operating system's random number generator does.
pub fn generate_api_key() -> anyhow::Result<String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0u8; 32];
    // getrandom::Error implements std::error::Error only with getrandom's
    // `std` feature, so convert it through its Display text.
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("could not read random bytes: {e}"))?;
    Ok(bytes
        .iter()
        .flat_map(|byte| [HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 0x0f)]])
        .map(char::from)
        .collect())
}

/// Default configuration file path.
pub fn default_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("aranet")
        .join("server.toml")
}

/// Expand a leading `~` path component to `home`.
///
/// `~` becomes `home` and `~/x` becomes `home/x`. Every other path, including
/// `~user/x`, is returned unchanged, and so is every path when `home` is
/// `None`.
pub(crate) fn expand_home_with(path: &Path, home: Option<&Path>) -> PathBuf {
    let mut components = path.components();
    match (components.next(), home) {
        (Some(Component::Normal(first)), Some(home)) if first == "~" => {
            let rest = components.as_path();
            if rest.as_os_str().is_empty() {
                home.to_path_buf()
            } else {
                home.join(rest)
            }
        }
        _ => path.to_path_buf(),
    }
}

/// [`expand_home_with`] with the current user's home directory.
pub(crate) fn expand_home(path: &Path) -> PathBuf {
    expand_home_with(path, dirs::home_dir().as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert_eq!(config.server.bind, "127.0.0.1:8080");
        assert!(config.devices.is_empty());
    }

    #[test]
    fn test_server_config_default() {
        let config = ServerConfig::default();
        assert_eq!(config.bind, "127.0.0.1:8080");
        assert_eq!(config.broadcast_buffer, DEFAULT_BROADCAST_BUFFER);
    }

    #[test]
    fn test_storage_config_default() {
        let config = StorageConfig::default();
        assert_eq!(config.path, aranet_store::default_db_path());
    }

    #[test]
    fn test_device_config_serde() {
        let toml = r#"
            address = "AA:BB:CC:DD:EE:FF"
            alias = "Living Room"
            poll_interval = 120
        "#;
        let config: DeviceConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.address, "AA:BB:CC:DD:EE:FF");
        assert_eq!(config.alias, Some("Living Room".to_string()));
        assert_eq!(config.poll_interval, 120);
    }

    #[test]
    fn test_device_config_default_poll_interval() {
        let toml = r#"address = "AA:BB:CC:DD:EE:FF""#;
        let config: DeviceConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.poll_interval, 60);
        assert_eq!(config.alias, None);
    }

    #[test]
    fn test_config_save_and_load() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("test_config.toml");

        let config = Config {
            server: ServerConfig {
                bind: "0.0.0.0:9090".to_string(),
                ..Default::default()
            },
            storage: StorageConfig {
                path: PathBuf::from("/tmp/test.db"),
                ..Default::default()
            },
            devices: vec![DeviceConfig {
                address: "AA:BB:CC:DD:EE:FF".to_string(),
                alias: Some("Test Device".to_string()),
                poll_interval: 30,
            }],
            ..Default::default()
        };

        config.save(&config_path).unwrap();
        let loaded = Config::load(&config_path).unwrap();

        assert_eq!(loaded.server.bind, "0.0.0.0:9090");
        assert_eq!(loaded.storage.path, PathBuf::from("/tmp/test.db"));
        assert_eq!(loaded.devices.len(), 1);
        assert_eq!(loaded.devices[0].address, "AA:BB:CC:DD:EE:FF");
        assert_eq!(loaded.devices[0].alias, Some("Test Device".to_string()));
        assert_eq!(loaded.devices[0].poll_interval, 30);
    }

    #[test]
    fn test_save_replaces_file_atomically() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("server.toml");
        let original = "[server]\nbind = \"127.0.0.1:8080\"\n";
        std::fs::write(&config_path, original).unwrap();
        // A hard link keeps the old contents only if the file was replaced, not
        // truncated and rewritten in place (which a crash could leave half-written).
        let link = temp_dir.path().join("server.toml.link");
        std::fs::hard_link(&config_path, &link).unwrap();

        let mut config = Config::load(&config_path).unwrap();
        config.server.bind = "127.0.0.1:9090".to_string();
        config.save(&config_path).unwrap();

        assert_eq!(std::fs::read_to_string(&link).unwrap(), original);
        assert_eq!(
            std::fs::read_to_string(&config_path).unwrap(),
            "[server]\nbind = \"127.0.0.1:9090\"\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_save_file_is_owner_only_from_creation() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("server.toml");
        std::fs::write(&config_path, "").unwrap();
        std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let old_inode = std::fs::metadata(&config_path).unwrap().ino();

        let mut config = Config::default();
        config.security.api_key = Some("k".repeat(32));
        config.save(&config_path).unwrap();

        let metadata = std::fs::metadata(&config_path).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        // A new file, not the 0644 one made private after the key was written to
        // it. aranet-store's `write_private_temp_file_is_0600_from_creation` shows
        // that the new file is owner-only before anything is written to it.
        assert_ne!(metadata.ino(), old_inode);
    }

    #[cfg(unix)]
    #[test]
    fn test_save_leaves_symlink_in_place() {
        let temp_dir = tempfile::tempdir().unwrap();
        let real = temp_dir.path().join("real.toml");
        std::fs::write(&real, "[server]\nbind = \"127.0.0.1:8080\"\n").unwrap();
        let link = temp_dir.path().join("server.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let mut config = Config::load(&link).unwrap();
        config.server.bind = "127.0.0.1:9090".to_string();
        config.save(&link).unwrap();

        let link_type = std::fs::symlink_metadata(&link).unwrap().file_type();
        assert!(link_type.is_symlink());
        assert_eq!(Config::load(&real).unwrap().server.bind, "127.0.0.1:9090");
    }

    #[test]
    fn test_update_file_changes_only_the_named_key() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("server.toml");
        let original = r#"# Aranet service
[server]
bind = "127.0.0.1:8080" # local only

[storage]
path = "/data/aranet.db"
"#;
        std::fs::write(&config_path, original).unwrap();

        let written =
            Config::update_file(&config_path, |config| config.server.broadcast_buffer = 200)
                .unwrap();

        assert!(written);
        assert_eq!(
            std::fs::read_to_string(&config_path).unwrap(),
            original.replace("# local only\n", "# local only\nbroadcast_buffer = 200\n")
        );
        assert!(!Config::update_file(&config_path, |_| {}).unwrap());
    }

    #[test]
    fn test_config_load_nonexistent() {
        let result = Config::load("/nonexistent/path/config.toml");
        assert!(matches!(result, Err(ConfigError::Read { .. })));
    }

    #[test]
    fn test_config_load_invalid_toml() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("invalid.toml");
        std::fs::write(&config_path, "this is not valid { toml").unwrap();

        let result = Config::load(&config_path);
        assert!(matches!(result, Err(ConfigError::Parse { .. })));
    }

    #[test]
    fn test_config_full_toml() {
        let toml = r#"
            [server]
            bind = "192.168.1.1:8888"

            [storage]
            path = "/data/aranet.db"

            [[devices]]
            address = "AA:BB:CC:DD:EE:FF"
            alias = "Living Room"
            poll_interval = 60

            [[devices]]
            address = "11:22:33:44:55:66"
            poll_interval = 120
        "#;

        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.server.bind, "192.168.1.1:8888");
        assert_eq!(config.storage.path, PathBuf::from("/data/aranet.db"));
        assert_eq!(config.devices.len(), 2);
        assert_eq!(config.devices[0].alias, Some("Living Room".to_string()));
        assert_eq!(config.devices[1].alias, None);
    }

    #[test]
    fn test_default_config_path() {
        let path = default_config_path();
        assert!(path.ends_with("aranet/server.toml"));
    }

    #[test]
    fn expand_home_table() {
        let home = Path::new("/home/u");
        let cases = [
            ("~", Some(home), home.to_path_buf()),
            ("~/a/b", Some(home), home.join("a").join("b")),
            ("/abs", Some(home), PathBuf::from("/abs")),
            ("rel/x", Some(home), PathBuf::from("rel/x")),
            ("~user/x", Some(home), PathBuf::from("~user/x")),
            ("rel/~/x", Some(home), PathBuf::from("rel/~/x")),
            ("~/a", None, PathBuf::from("~/a")),
        ];
        for (path, home, expected) in cases {
            assert_eq!(
                expand_home_with(Path::new(path), home),
                expected,
                "{path} with home {home:?}"
            );
        }
        // Exactly the home directory, without a trailing separator.
        assert_eq!(
            expand_home_with(Path::new("~"), Some(home)).as_os_str(),
            home.as_os_str()
        );

        // `expand_home` uses the current user's home directory.
        let real_home = dirs::home_dir().expect("a home directory");
        assert_eq!(expand_home(Path::new("~/x")), real_home.join("x"));
        assert_eq!(expand_home(Path::new("rel/x")), PathBuf::from("rel/x"));
    }

    #[test]
    fn test_config_error_display() {
        let error = ConfigError::Read {
            path: PathBuf::from("/test/path"),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "not found"),
        };
        let display = format!("{}", error);
        assert!(display.contains("/test/path"));
        assert!(display.contains("not found"));
    }

    #[test]
    fn test_config_file_error_keeps_why_a_save_was_refused() {
        use std::error::Error;

        // The file names the field by its alias, so a save adds the field's own
        // name next to it. The updater then refuses its own text, which has the
        // field twice.
        #[derive(Default, Serialize, Deserialize)]
        #[serde(default)]
        struct Aliased {
            #[serde(alias = "colour")]
            color: String,
        }
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("server.toml");
        std::fs::write(&path, "colour = \"red\"\n").unwrap();
        let desired = Aliased {
            color: "blue".to_string(),
        };
        let refused = TomlFile::<Aliased>::open(&path)
            .unwrap()
            .save(&desired)
            .unwrap_err();

        let error = config_file_error(&path, refused);

        // The message ends with the direct cause. The reason is the cause below
        // that, which the API logs and keeps out of its response.
        assert_eq!(
            error.to_string(),
            format!(
                "Failed to write config file {}: rendered TOML does not read back as the \
                 intended values; the file was not changed",
                path.display()
            )
        );
        let reason = error
            .source()
            .and_then(Error::source)
            .map(ToString::to_string);
        assert!(
            reason
                .as_deref()
                .is_some_and(|reason| reason.contains("duplicate field `color`")),
            "{reason:?}"
        );
    }

    // ==========================================================================
    // Validation tests
    // ==========================================================================

    #[test]
    fn test_default_config_validates() {
        let config = Config::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn security_defaults_trust_no_extra_origins_or_hosts() {
        let defaults = SecurityConfig::default();
        assert!(
            defaults.cors_origins.is_empty(),
            "{:?}",
            defaults.cors_origins
        );
        assert!(
            defaults.allowed_hosts.is_empty(),
            "{:?}",
            defaults.allowed_hosts
        );

        // A [security] table without the keys gets the same defaults.
        let config: Config = toml::from_str("[security]\napi_key_enabled = false\n").unwrap();
        assert!(config.security.cors_origins.is_empty());
        assert!(config.security.allowed_hosts.is_empty());

        // Values already saved in server.toml stay in effect.
        let config: Config = toml::from_str(
            "[security]\ncors_origins = [\"http://localhost\"]\nallowed_hosts = [\"aranet-service\"]\n",
        )
        .unwrap();
        assert_eq!(config.security.cors_origins, ["http://localhost"]);
        assert_eq!(config.security.allowed_hosts, ["aranet-service"]);
    }

    #[test]
    fn generate_api_key_is_64_hex_and_unique() {
        let first = generate_api_key().unwrap();
        let second = generate_api_key().unwrap();
        // A failed assertion must not print a key: the messages say what was
        // wrong, and the keys are compared without assert_ne!.
        for key in [&first, &second] {
            assert_eq!(key.len(), 64, "a generated key has the wrong length");
            assert!(
                key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
                "a generated key has characters other than 0-9 and a-f"
            );
        }
        assert!(first != second, "two generated keys are the same");

        let security = SecurityConfig {
            api_key_enabled: true,
            api_key: Some(first),
            ..SecurityConfig::default()
        };
        assert!(security.validate().is_empty());
    }

    #[test]
    fn generated_api_keys_use_all_32_random_bytes() {
        let keys: Vec<String> = (0..64).map(|_| generate_api_key().unwrap()).collect();

        // Every character changes from key to key, so no byte is left
        // unfilled. (A random character repeats in all 64 keys once in 16^63.)
        for position in 0..64 {
            let first = keys[0].as_bytes()[position];
            assert!(
                keys.iter().any(|key| key.as_bytes()[position] != first),
                "character {position} is the same in 64 keys"
            );
        }

        // The two characters of a byte differ somewhere, so both of its
        // halves are written.
        assert!(
            keys.iter()
                .any(|key| key.as_bytes().chunks(2).any(|pair| pair[0] != pair[1])),
            "both characters of every byte are the same"
        );
    }

    #[test]
    fn allowed_hosts_validation() {
        let with_hosts = |hosts: &[&str]| SecurityConfig {
            allowed_hosts: hosts.iter().map(|host| host.to_string()).collect(),
            ..SecurityConfig::default()
        };

        let valid: [&[&str]; 4] = [
            &[],
            &["*"],
            &["aranet-service"],
            &["Aranet.Example.COM", "pi.local"],
        ];
        for hosts in valid {
            let errors = with_hosts(hosts).validate();
            assert!(errors.is_empty(), "{hosts:?}: {errors:?}");
        }

        // None of these can ever equal a Host a browser sends.
        for host in [
            "",
            "  ",
            " aranet-service",
            "my host",
            "*.home.arpa",
            "aranet-*",
            "aranet-service:8080",
            "http://aranet-service",
            "aranet-service/",
            "user@aranet-service",
            "[::1]",
        ] {
            let errors = with_hosts(&[host]).validate();
            assert_eq!(errors.len(), 1, "{host:?}: {errors:?}");
            assert_eq!(errors[0].field, "security.allowed_hosts");
            assert!(errors[0].message.contains("bare host names"), "{errors:?}");
        }
    }

    #[test]
    fn test_server_bind_validation() {
        let with_bind = |bind: &str| ServerConfig {
            bind: bind.to_string(),
            ..Default::default()
        };

        // An IP address and a port: valid.
        for bind in ["127.0.0.1:8080", "[::1]:8080", "0.0.0.0:8080"] {
            let errors = with_bind(bind).validate();
            assert!(errors.is_empty(), "{bind}: {errors:?}");
        }

        // The listener binds exactly this address and never resolves names, so
        // a host name, IPv6 without brackets, an octet above 255, a missing port
        // and a non-numeric port each get one error that names the expected form.
        for bind in [
            "localhost:8080",
            "::1:8080",
            "999.1.1.1:80",
            "127.0.0.1",
            "127.0.0.1:abc",
        ] {
            let errors = with_bind(bind).validate();
            assert_eq!(errors.len(), 1, "{bind}: {errors:?}");
            assert_eq!(errors[0].field, "server.bind");
            assert!(
                errors[0]
                    .message
                    .starts_with(&format!("invalid bind address '{bind}': expected IP:port")),
                "{bind}: {}",
                errors[0].message
            );
        }

        let errors = with_bind("127.0.0.1:0").validate();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].message, "port cannot be 0");

        let errors = with_bind("").validate();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].message, "bind address cannot be empty");

        let zero_buffer = ServerConfig {
            broadcast_buffer: 0,
            ..Default::default()
        };
        let errors = zero_buffer.validate();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].field.contains("broadcast_buffer"));
    }

    #[test]
    fn test_socket_addr() {
        use std::net::{Ipv6Addr, SocketAddr};

        let with_bind = |bind: &str| ServerConfig {
            bind: bind.to_string(),
            ..Default::default()
        };
        assert_eq!(
            ServerConfig::default().socket_addr(),
            Some(SocketAddr::from(([127, 0, 0, 1], 8080)))
        );
        assert_eq!(
            with_bind("[::1]:9090").socket_addr(),
            Some(SocketAddr::from((Ipv6Addr::LOCALHOST, 9090)))
        );
        assert_eq!(with_bind("localhost:8080").socket_addr(), None);
        assert_eq!(with_bind("").socket_addr(), None);
    }

    #[test]
    fn test_storage_path_validation() {
        // Valid path
        let valid = StorageConfig {
            path: PathBuf::from("/data/aranet.db"),
            ..Default::default()
        };
        assert!(valid.validate().is_empty());

        // Invalid: empty path
        let empty = StorageConfig {
            path: PathBuf::new(),
            ..Default::default()
        };
        let errors = empty.validate();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("cannot be empty"));
    }

    #[test]
    fn test_retention_off_by_default_and_not_serialized() {
        let config = Config::default();
        assert_eq!(config.storage.retention, RetentionConfig::default());
        assert!(config.storage.retention.is_disabled());
        assert!(!config.storage.retention.is_enabled());

        let toml = toml::to_string(&config).unwrap();
        assert!(!toml.contains("retention"), "{toml}");

        // Only the limits that are set are written.
        let mut config = Config::default();
        config.storage.retention.readings_days = Some(30);
        let toml = toml::to_string(&config).unwrap();
        assert!(
            toml.contains("[storage.retention]\nreadings_days = 30\n"),
            "{toml}"
        );
        assert!(!toml.contains("history_days"), "{toml}");
    }

    #[test]
    fn test_retention_parses() {
        let config: Config = toml::from_str(
            r#"
            [storage]
            path = "/data/aranet.db"

            [storage.retention]
            readings_days = 30
            "#,
        )
        .unwrap();
        assert_eq!(config.storage.path, PathBuf::from("/data/aranet.db"));
        assert_eq!(
            config.storage.retention,
            RetentionConfig {
                readings_days: Some(30),
                history_days: None,
            }
        );
        assert!(config.storage.retention.is_enabled());
        assert!(config.validate().is_ok());

        // The table on its own keeps the default database path.
        let config: Config = toml::from_str("[storage.retention]\nhistory_days = 90\n").unwrap();
        assert_eq!(config.storage.path, aranet_store::default_db_path());
        assert_eq!(config.storage.retention.readings_days, None);
        assert_eq!(config.storage.retention.history_days, Some(90));
    }

    #[test]
    fn test_retention_rejects_zero_and_too_many_days() {
        for (days, message) in [
            (
                0,
                "must be at least 1 day (remove the key to keep data forever)",
            ),
            (MAX_RETENTION_DAYS + 1, "must be at most 36500 days"),
        ] {
            let retention = RetentionConfig {
                readings_days: Some(days),
                history_days: Some(days),
            };
            let errors = retention.validate();
            let fields: Vec<&str> = errors.iter().map(|e| e.field.as_str()).collect();
            assert_eq!(
                fields,
                [
                    "storage.retention.readings_days",
                    "storage.retention.history_days"
                ],
                "{days} days"
            );
            assert!(errors.iter().all(|e| e.message == message), "{errors:?}");
        }

        for days in [1, MAX_RETENTION_DAYS] {
            let retention = RetentionConfig {
                readings_days: Some(days),
                history_days: Some(days),
            };
            assert!(retention.validate().is_empty(), "{days} days");
        }

        // Config::validate reports them through StorageConfig::validate.
        let mut config = Config::default();
        config.storage.retention.readings_days = Some(0);
        let error = config.validate().unwrap_err().to_string();
        assert!(
            error.contains("storage.retention.readings_days: must be at least 1 day"),
            "{error}"
        );
    }

    #[test]
    fn test_device_config_validation() {
        // Valid device
        let valid = DeviceConfig {
            address: "AA:BB:CC:DD:EE:FF".to_string(),
            alias: Some("Living Room".to_string()),
            poll_interval: 60,
        };
        assert!(valid.validate("devices[0]").is_empty());

        // Invalid: empty address
        let empty_addr = DeviceConfig {
            address: "".to_string(),
            alias: None,
            poll_interval: 60,
        };
        let errors = empty_addr.validate("devices[0]");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("cannot be empty"));

        // Invalid: address too short
        let short_addr = DeviceConfig {
            address: "AB".to_string(),
            alias: None,
            poll_interval: 60,
        };
        let errors = short_addr.validate("devices[0]");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("too short"));

        // Invalid: empty alias (should be null instead)
        let empty_alias = DeviceConfig {
            address: "Aranet4 12345".to_string(),
            alias: Some("".to_string()),
            poll_interval: 60,
        };
        let errors = empty_alias.validate("devices[0]");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("cannot be empty string"));

        // Invalid: poll interval too short
        let short_poll = DeviceConfig {
            address: "Aranet4 12345".to_string(),
            alias: None,
            poll_interval: 5,
        };
        let errors = short_poll.validate("devices[0]");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("too short"));

        // Invalid: poll interval too long
        let long_poll = DeviceConfig {
            address: "Aranet4 12345".to_string(),
            alias: None,
            poll_interval: 7200,
        };
        let errors = long_poll.validate("devices[0]");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("too long"));
    }

    fn test_device(address: &str, alias: Option<&str>) -> DeviceConfig {
        DeviceConfig {
            address: address.to_string(),
            alias: alias.map(str::to_string),
            poll_interval: 60,
        }
    }

    /// Characters no device address or alias may contain: C0 and C1 controls
    /// (`\u{85}` is NEL), the Unicode line and paragraph separators, and the
    /// bidi embedding, override and isolate controls.
    const FORBIDDEN_SAMPLES: [char; 10] = [
        '\n', '\r', '\t', '\0', '\u{7f}', '\u{85}', '\u{2028}', '\u{2029}', '\u{202e}', '\u{2066}',
    ];

    #[test]
    fn device_alias_rejects_control_characters() {
        for c in FORBIDDEN_SAMPLES {
            let alias = format!("Office{c}X");
            let errors = test_device("Aranet4 12345", Some(&alias)).validate("device");
            assert_eq!(errors.len(), 1, "{c:?}: {errors:?}");
            assert_eq!(errors[0].field, "device.alias", "{c:?}");
            assert_eq!(
                errors[0].message, "alias cannot contain control characters",
                "{c:?}"
            );
        }
    }

    #[test]
    fn device_address_rejects_control_characters() {
        for c in FORBIDDEN_SAMPLES {
            let address = format!("Aranet4{c}12345");
            let errors = test_device(&address, None).validate("devices[3]");
            assert_eq!(errors.len(), 1, "{c:?}: {errors:?}");
            assert_eq!(errors[0].field, "devices[3].address", "{c:?}");
            assert_eq!(
                errors[0].message, "device address cannot contain control characters",
                "{c:?}"
            );
        }
    }

    #[test]
    fn device_text_rejects_backslashes() {
        // InfluxDB 1.x and 2.x keep `\\` as two backslashes in a tag value and
        // read a separator after any backslash as escaped, so no escaping can
        // write one faithfully.
        for alias in ["Office\\", "C:\\temp", "a\\ b"] {
            let errors = test_device("Aranet4 12345", Some(alias)).validate("device");
            assert_eq!(errors.len(), 1, "{alias:?}: {errors:?}");
            assert_eq!(errors[0].field, "device.alias", "{alias:?}");
            assert_eq!(
                errors[0].message, "alias cannot contain backslashes",
                "{alias:?}"
            );
        }

        let errors = test_device("Aranet4\\12345", None).validate("devices[3]");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].field, "devices[3].address");
        assert_eq!(
            errors[0].message,
            "device address cannot contain backslashes"
        );
    }

    #[test]
    fn device_text_length_limit() {
        // The limit counts characters, not bytes: 'ü' is 2 bytes in UTF-8, so
        // this alias is 128 bytes long and still allowed.
        let at_limit = "ü".repeat(64);
        assert!(
            test_device(&at_limit, Some(&at_limit))
                .validate("device")
                .is_empty()
        );

        let too_long = "ü".repeat(65);
        let errors = test_device("Aranet4 12345", Some(&too_long)).validate("device");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].field, "device.alias");
        assert_eq!(
            errors[0].message,
            "alias is too long (65 characters, maximum 64)"
        );

        let errors = test_device(&"A".repeat(65), None).validate("device");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].field, "device.address");
        assert_eq!(
            errors[0].message,
            "device address is too long (65 characters, maximum 64)"
        );

        // Every problem in one value is reported.
        let errors =
            test_device("Aranet4 12345", Some(&format!("{too_long}\n\\"))).validate("device");
        let messages: Vec<&str> = errors.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "alias cannot contain control characters",
                "alias cannot contain backslashes",
                "alias is too long (67 characters, maximum 64)"
            ]
        );
    }

    #[test]
    fn device_text_accepts_real_names() {
        for name in [
            "AranetRn+ 306B8",
            "Aranet\u{2622} 30ED1",
            "Küche",
            // A CoreBluetooth peripheral identifier (the address on macOS).
            "1f8893bf-9f7e-02b4-ef4a-7718f4f5d4be",
            "AA:BB:CC:DD:EE:FF",
        ] {
            let errors = test_device(name, Some(name)).validate("device");
            assert!(errors.is_empty(), "{name}: {errors:?}");
        }
    }

    #[test]
    fn forbidden_text_chars_are_exactly_the_documented_set() {
        // Both ends of every range that `is_forbidden_text_char`'s doc lists,
        // and its two single code points.
        for c in [
            '\u{0}', '\u{1f}', '\u{7f}', '\u{9f}', '\u{2028}', '\u{2029}', '\u{202a}', '\u{202e}',
            '\u{2066}', '\u{2069}',
        ] {
            assert!(
                is_forbidden_text_char(c),
                "U+{:04X} must be refused",
                u32::from(c)
            );
        }

        // The nearest code points outside each range are allowed, and so are
        // characters that turn up in aliases: a typographic apostrophe
        // (U+2019), a no-break space (U+00A0) and a zero-width space (U+200B).
        for c in [
            ' ', '~', '\u{a0}', '\u{2027}', '\u{202f}', '\u{2065}', '\u{206a}', '\u{2019}',
            '\u{200b}',
        ] {
            assert!(
                !is_forbidden_text_char(c),
                "U+{:04X} must be allowed",
                u32::from(c)
            );
        }

        // Nothing in between differs either: a code point is refused exactly
        // when the doc lists it.
        for c in '\0'..=char::MAX {
            let documented = matches!(
                u32::from(c),
                0x00..=0x1f | 0x7f..=0x9f | 0x2028 | 0x2029 | 0x202a..=0x202e | 0x2066..=0x2069
            );
            assert_eq!(
                is_forbidden_text_char(c),
                documented,
                "U+{:04X}",
                u32::from(c)
            );
        }
    }

    #[test]
    fn test_duplicate_device_addresses() {
        let config = Config {
            server: ServerConfig::default(),
            storage: StorageConfig::default(),
            devices: vec![
                DeviceConfig {
                    address: "Aranet4 12345".to_string(),
                    alias: Some("Office".to_string()),
                    poll_interval: 60,
                },
                DeviceConfig {
                    address: "Aranet4 12345".to_string(), // Duplicate
                    alias: Some("Bedroom".to_string()),
                    poll_interval: 60,
                },
            ],
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        if let Err(ConfigError::Validation(errors)) = result {
            assert!(errors.iter().any(|e| e.message.contains("duplicate")));
        }
    }

    #[test]
    fn test_duplicate_addresses_case_insensitive() {
        let config = Config {
            server: ServerConfig::default(),
            storage: StorageConfig::default(),
            devices: vec![
                DeviceConfig {
                    address: "Aranet4 12345".to_string(),
                    alias: None,
                    poll_interval: 60,
                },
                DeviceConfig {
                    address: "ARANET4 12345".to_string(), // Same, different case
                    alias: None,
                    poll_interval: 60,
                },
            ],
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_error_display() {
        let error = ValidationError {
            field: "server.bind".to_string(),
            message: "invalid port".to_string(),
        };
        assert_eq!(format!("{}", error), "server.bind: invalid port");
    }

    #[test]
    fn test_config_validation_error_display() {
        let errors = vec![
            ValidationError {
                field: "server.bind".to_string(),
                message: "port cannot be 0".to_string(),
            },
            ValidationError {
                field: "devices[0].address".to_string(),
                message: "cannot be empty".to_string(),
            },
        ];
        let error = ConfigError::Validation(errors);
        let display = format!("{}", error);
        assert!(display.contains("server.bind"));
        assert!(display.contains("devices[0].address"));
    }

    // ==========================================================================
    // Network exposure tests
    // ==========================================================================

    /// A default config that listens on `bind`.
    fn config_listening_on(bind: &str) -> Config {
        Config {
            server: ServerConfig {
                bind: bind.to_string(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// The errors `config.validate()` reports, or none when it is valid.
    fn all_validation_errors(config: &Config) -> Vec<ValidationError> {
        match config.validate() {
            Ok(()) => Vec::new(),
            Err(ConfigError::Validation(errors)) => errors,
            Err(e) => panic!("expected a validation result, got {e}"),
        }
    }

    #[test]
    fn test_unauthenticated_network_bind_is_refused() {
        for bind in ["0.0.0.0:8080", "[::]:8080", "192.168.1.10:8080"] {
            let errors = all_validation_errors(&config_listening_on(bind));
            assert_eq!(errors.len(), 1, "{bind}: {errors:?}");
            assert_eq!(errors[0].field, "server.bind", "{bind}");
            let message = &errors[0].message;
            for fix in [
                bind,
                "security.api_key_enabled = true",
                "127.0.0.1",
                "security.allow_unauthenticated_network = true",
            ] {
                assert!(message.contains(fix), "{bind}: {message:?} lacks {fix:?}");
            }
        }
    }

    #[test]
    fn test_loopback_binds_need_no_auth() {
        for bind in [
            "127.0.0.1:8080",
            "127.1.2.3:8080",
            "[::1]:8080",
            "[::ffff:127.0.0.1]:8080",
        ] {
            let errors = all_validation_errors(&config_listening_on(bind));
            assert!(errors.is_empty(), "{bind}: {errors:?}");
        }
    }

    #[test]
    fn test_network_bind_allowed_with_auth_or_opt_out() {
        let mut authenticated = config_listening_on("0.0.0.0:8080");
        authenticated.security.api_key_enabled = true;
        authenticated.security.api_key = Some("0123456789abcdef".repeat(4));
        let errors = all_validation_errors(&authenticated);
        assert!(errors.is_empty(), "with an API key: {errors:?}");

        // Parsed from TOML, so this also pins the key's name in server.toml.
        let opted_out: Config = toml::from_str(
            r#"
            [server]
            bind = "[::]:8080"

            [security]
            allow_unauthenticated_network = true
            "#,
        )
        .unwrap();
        let errors = all_validation_errors(&opted_out);
        assert!(errors.is_empty(), "with the opt-out: {errors:?}");
    }

    /// The whole refusal, as the service prints it at startup.
    #[test]
    fn test_unauthenticated_network_bind_message() {
        let errors = all_validation_errors(&config_listening_on("0.0.0.0:8080"));
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(
            errors[0].to_string(),
            "server.bind: refusing to listen on 0.0.0.0:8080 without API-key authentication: \
             set security.api_key_enabled = true, bind to 127.0.0.1, or set \
             security.allow_unauthenticated_network = true if a firewall or authenticating \
             proxy protects the port"
        );
    }

    /// The rule looks only at `api_key_enabled`: an enabled key that is missing
    /// or too short is reported as that, and the bind is not refused as well.
    #[test]
    fn test_network_bind_with_unusable_key_reports_only_the_key() {
        for (case, key) in [
            ("no key", None),
            ("a short key", Some("too-short".to_string())),
        ] {
            let mut config = config_listening_on("0.0.0.0:8080");
            config.security.api_key_enabled = true;
            config.security.api_key = key;
            let errors = all_validation_errors(&config);
            let fields: Vec<_> = errors.iter().map(|error| error.field.as_str()).collect();
            assert_eq!(fields, ["security.api_key"], "{case}");
        }
    }

    /// The opt-out is written to the config file like any other setting.
    #[test]
    fn test_opt_out_is_written_to_the_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.toml");
        std::fs::write(&path, "[server]\nbind = \"0.0.0.0:8080\"\n").unwrap();

        let written = Config::update_file(&path, |config| {
            config.security.allow_unauthenticated_network = true;
        })
        .unwrap();

        assert!(written, "the file was not changed");
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(
            saved.contains("allow_unauthenticated_network = true"),
            "{saved}"
        );
        let errors = all_validation_errors(&Config::load(&path).unwrap());
        assert!(errors.is_empty(), "{errors:?}");
    }

    /// The Docker image bakes in docker/server.toml, so a config the service
    /// refuses would make the container exit at startup.
    #[test]
    fn docker_server_toml_validates() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        if !repo.join("crates/aranet-service/Cargo.toml").is_file() {
            // The published crate has no docker/.
            eprintln!("skipped: not a repository checkout, so there is no docker/server.toml");
            return;
        }
        let config =
            Config::load(repo.join("docker/server.toml")).unwrap_or_else(|e| panic!("{e}"));
        if let Err(e) = config.validate() {
            panic!("the Docker image would exit at startup with docker/server.toml: {e}");
        }
    }

    // ==========================================================================
    // Prometheus config tests
    // ==========================================================================

    #[test]
    fn test_prometheus_config_default() {
        let config = PrometheusConfig::default();
        assert!(!config.enabled);
        assert!(config.push_gateway.is_none());
        assert_eq!(config.push_interval, 60);
    }

    #[test]
    fn test_prometheus_config_validates() {
        let config = PrometheusConfig::default();
        assert!(config.validate().is_empty());
    }

    #[test]
    fn test_prometheus_config_empty_push_gateway() {
        let config = PrometheusConfig {
            enabled: true,
            push_gateway: Some("".to_string()),
            push_interval: 60,
        };
        let errors = config.validate();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("cannot be empty"));
    }

    #[test]
    fn test_prometheus_config_short_push_interval() {
        let config = PrometheusConfig {
            enabled: true,
            push_gateway: None,
            push_interval: 5,
        };
        let errors = config.validate();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("too short"));
    }

    #[test]
    fn test_prometheus_config_serde() {
        let toml = r#"
            enabled = true
            push_gateway = "http://localhost:9091"
            push_interval = 30
        "#;
        let config: PrometheusConfig = toml::from_str(toml).unwrap();
        assert!(config.enabled);
        assert_eq!(
            config.push_gateway,
            Some("http://localhost:9091".to_string())
        );
        assert_eq!(config.push_interval, 30);
    }

    // ==========================================================================
    // MQTT config tests
    // ==========================================================================

    #[test]
    fn test_mqtt_config_default() {
        let config = MqttConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.broker, "mqtt://localhost:1883");
        assert_eq!(config.topic_prefix, "aranet");
        assert_eq!(config.client_id, "aranet-service");
        assert_eq!(config.qos, 1);
        assert!(!config.retain);
        assert!(config.username.is_none());
        assert!(config.password.is_none());
        assert_eq!(config.keep_alive, 60);
    }

    #[test]
    fn test_mqtt_config_validates_when_disabled() {
        let config = MqttConfig::default();
        assert!(config.validate().is_empty());
    }

    #[test]
    fn test_mqtt_config_validates_when_enabled() {
        let config = MqttConfig {
            enabled: true,
            ..Default::default()
        };
        assert!(config.validate().is_empty());
    }

    #[test]
    fn test_mqtt_config_empty_broker() {
        let config = MqttConfig {
            enabled: true,
            broker: "".to_string(),
            ..Default::default()
        };
        let errors = config.validate();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("broker URL cannot be empty"))
        );
    }

    #[test]
    fn test_mqtt_config_invalid_broker_scheme() {
        let config = MqttConfig {
            enabled: true,
            broker: "http://localhost:1883".to_string(),
            ..Default::default()
        };
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.message.contains("mqtt://")));
    }

    #[test]
    fn test_mqtt_config_empty_topic_prefix() {
        let config = MqttConfig {
            enabled: true,
            topic_prefix: "".to_string(),
            ..Default::default()
        };
        let errors = config.validate();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("topic prefix cannot be empty"))
        );
    }

    #[test]
    fn test_mqtt_config_empty_client_id() {
        let config = MqttConfig {
            enabled: true,
            client_id: "".to_string(),
            ..Default::default()
        };
        let errors = config.validate();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("client ID cannot be empty"))
        );
    }

    #[test]
    fn test_mqtt_config_invalid_qos() {
        let config = MqttConfig {
            enabled: true,
            qos: 5,
            ..Default::default()
        };
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.message.contains("invalid QoS")));
    }

    #[test]
    fn test_mqtt_config_short_keep_alive() {
        let config = MqttConfig {
            enabled: true,
            keep_alive: 2,
            ..Default::default()
        };
        let errors = config.validate();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("keep-alive interval"))
        );
    }

    #[test]
    fn mqtt_prefix_validation() {
        let config = MqttConfig {
            enabled: true,
            topic_prefix: "aranet/#".to_string(),
            homeassistant: true,
            ha_discovery_prefix: "a\u{0}".to_string(),
            ..Default::default()
        };
        let errors = config.validate();
        let fields: Vec<&str> = errors.iter().map(|e| e.field.as_str()).collect();
        assert_eq!(fields, ["mqtt.topic_prefix", "mqtt.ha_discovery_prefix"]);
        assert_eq!(
            errors[0].message,
            "topic prefix cannot contain control characters or MQTT wildcards"
        );
        assert_eq!(
            errors[1].message,
            "Home Assistant discovery prefix cannot contain control characters or MQTT wildcards"
        );

        let swapped = MqttConfig {
            enabled: true,
            topic_prefix: "a\u{0}".to_string(),
            homeassistant: true,
            ha_discovery_prefix: "homeassistant/+".to_string(),
            ..Default::default()
        };
        let fields: Vec<String> = swapped.validate().into_iter().map(|e| e.field).collect();
        assert_eq!(fields, ["mqtt.topic_prefix", "mqtt.ha_discovery_prefix"]);

        // The discovery prefix is only used, and only checked, with Home
        // Assistant discovery on.
        let no_discovery = MqttConfig {
            enabled: true,
            homeassistant: false,
            ha_discovery_prefix: "x/#".to_string(),
            ..Default::default()
        };
        assert!(no_discovery.validate().is_empty());

        // Topic levels are fine, and a disabled publisher isn't checked.
        let nested = MqttConfig {
            enabled: true,
            topic_prefix: "home/sensors/aranet".to_string(),
            ..Default::default()
        };
        assert!(nested.validate().is_empty());
        let disabled = MqttConfig {
            topic_prefix: "aranet/#".to_string(),
            ..Default::default()
        };
        assert!(disabled.validate().is_empty());
    }

    #[test]
    fn test_mqtt_config_serde() {
        let toml = r#"
            enabled = true
            broker = "mqtts://broker.example.com:8883"
            topic_prefix = "home/sensors"
            client_id = "my-service"
            qos = 2
            retain = true
            username = "user"
            password = "secret"
            keep_alive = 30
        "#;
        let config: MqttConfig = toml::from_str(toml).unwrap();
        assert!(config.enabled);
        assert_eq!(config.broker, "mqtts://broker.example.com:8883");
        assert_eq!(config.topic_prefix, "home/sensors");
        assert_eq!(config.client_id, "my-service");
        assert_eq!(config.qos, 2);
        assert!(config.retain);
        assert_eq!(config.username, Some("user".to_string()));
        assert_eq!(config.password, Some("secret".to_string()));
        assert_eq!(config.keep_alive, 30);
    }

    #[test]
    fn test_influxdb_config_requires_org_when_enabled() {
        let config = InfluxDbConfig {
            enabled: true,
            org: "   ".to_string(),
            ..Default::default()
        };
        let errors = config.validate();
        assert!(errors.iter().any(
            |e| e.field == "influxdb.org" && e.message.contains("organization cannot be empty")
        ));
    }

    #[test]
    fn test_influxdb_config_validates_when_enabled_with_org() {
        let config = InfluxDbConfig {
            enabled: true,
            org: "aranet".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_empty());
    }

    #[test]
    fn influxdb_measurement_validation() {
        let with_measurement = |measurement: &str| InfluxDbConfig {
            enabled: true,
            org: "aranet".to_string(),
            measurement: measurement.to_string(),
            ..Default::default()
        };

        let errors = with_measurement("_x").validate();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].field, "influxdb.measurement");
        assert_eq!(
            errors[0].message,
            "measurement name cannot start with '_' (reserved by InfluxDB)"
        );

        let errors = with_measurement("a\nb").validate();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].field, "influxdb.measurement");
        assert_eq!(
            errors[0].message,
            "measurement name cannot contain control characters"
        );

        // InfluxDB 1.x and 2.x can't read a backslash in a measurement back.
        for measurement in ["aranet\\", "a\\b"] {
            let errors = with_measurement(measurement).validate();
            assert_eq!(errors.len(), 1, "{measurement:?}: {errors:?}");
            assert_eq!(errors[0].field, "influxdb.measurement");
            assert_eq!(
                errors[0].message,
                "measurement name cannot contain backslashes"
            );
        }

        // Spaces, commas and non-ASCII are fine: the writer escapes them.
        assert!(with_measurement("air quality,indoor").validate().is_empty());
        assert!(with_measurement("Luftqualität").validate().is_empty());

        // A disabled exporter isn't checked.
        let disabled = InfluxDbConfig {
            measurement: "_x".to_string(),
            ..Default::default()
        };
        assert!(disabled.validate().is_empty());
    }

    #[test]
    fn influxdb_measurement_rejects_leading_hash() {
        let with_measurement = |measurement: &str| InfluxDbConfig {
            enabled: true,
            org: "aranet".to_string(),
            measurement: measurement.to_string(),
            ..Default::default()
        };

        // InfluxDB reads a line that starts with `#` as a comment. For
        // `#aranet`, 1.8.10 and 2.7.12 answer 204 and store nothing, and 3 Core
        // 3.12.0 answers 400. No escape exists: `\#aranet` is stored under that
        // name, backslash included.
        for measurement in ["#aranet", "#"] {
            let errors = with_measurement(measurement).validate();
            assert_eq!(errors.len(), 1, "{measurement:?}: {errors:?}");
            assert_eq!(errors[0].field, "influxdb.measurement");
            assert_eq!(
                errors[0].message,
                "measurement name cannot start with '#' (InfluxDB reads the line as a comment)"
            );
        }

        // Only the first character of the line matters, so a `#` further in,
        // or after a leading space, is fine.
        for measurement in ["a#b", " #aranet"] {
            let errors = with_measurement(measurement).validate();
            assert!(errors.is_empty(), "{measurement:?}: {errors:?}");
        }
        // An alias is a tag value, never the start of the line.
        let errors = test_device("Aranet4 12345", Some("#Office")).validate("device");
        assert!(errors.is_empty(), "{errors:?}");

        // A disabled exporter isn't checked.
        let disabled = InfluxDbConfig {
            measurement: "#aranet".to_string(),
            ..Default::default()
        };
        assert!(disabled.validate().is_empty());
    }

    #[test]
    fn test_config_with_prometheus_and_mqtt() {
        let toml = r#"
            [server]
            bind = "127.0.0.1:8080"

            [prometheus]
            enabled = true

            [mqtt]
            enabled = true
            broker = "mqtt://localhost:1883"
            topic_prefix = "aranet"
        "#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(config.prometheus.enabled);
        assert!(config.mqtt.enabled);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn rate_limit_window_over_a_day_and_zero_max_entries_rejected() {
        let over_a_day = SecurityConfig {
            rate_limit_window_secs: 86_401,
            ..Default::default()
        };
        let errors = over_a_day.validate();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].field, "security.rate_limit_window_secs");
        assert_eq!(
            errors[0].message,
            "rate limit window 86401 exceeds maximum of 86400 seconds"
        );

        let no_entries = SecurityConfig {
            rate_limit_max_entries: 0,
            ..Default::default()
        };
        let errors = no_entries.validate();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].field, "security.rate_limit_max_entries");
        assert_eq!(
            errors[0].message,
            "rate limit max entries must be at least 1"
        );

        // The limits themselves are accepted.
        let at_the_limits = SecurityConfig {
            rate_limit_window_secs: 86_400,
            rate_limit_max_entries: 1,
            ..Default::default()
        };
        assert!(at_the_limits.validate().is_empty());

        // Nothing is checked while rate limiting is off.
        let disabled = SecurityConfig {
            rate_limit_enabled: false,
            rate_limit_window_secs: u64::MAX,
            rate_limit_max_entries: 0,
            ..Default::default()
        };
        assert!(disabled.validate().is_empty());
    }
}
