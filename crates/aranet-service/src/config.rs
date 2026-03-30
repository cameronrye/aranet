//! Server configuration.

use std::path::{Path, PathBuf};

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

    /// Save configuration to a file.
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), ConfigError> {
        let content = toml::to_string_pretty(self).map_err(ConfigError::Serialize)?;

        // Create parent directories if needed
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent).map_err(|e| ConfigError::Write {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }

        std::fs::write(path.as_ref(), content).map_err(|e| ConfigError::Write {
            path: path.as_ref().to_path_buf(),
            source: e,
        })?;

        // Restrict permissions to owner-only since config may contain API keys
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            let _ = std::fs::set_permissions(path.as_ref(), perms);
        }

        Ok(())
    }

    /// Validate the configuration and return any errors.
    ///
    /// This checks:
    /// - Server bind address is valid (host:port format)
    /// - Storage path is not empty
    /// - Device addresses are not empty
    /// - Device poll intervals are within reasonable bounds (10s - 1 hour)
    /// - No duplicate device addresses
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
    /// Bind address (e.g., "127.0.0.1:8080").
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
    /// Validate server configuration.
    pub fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        if self.bind.is_empty() {
            validate!(errors, "server.bind", "bind address cannot be empty");
        } else {
            // Check for valid host:port format
            let parts: Vec<&str> = self.bind.rsplitn(2, ':').collect();
            if parts.len() != 2 {
                validate!(
                    errors,
                    "server.bind",
                    "invalid bind address '{}': expected format 'host:port'",
                    self.bind
                );
            } else {
                // Validate port
                let port_str = parts[0];
                match port_str.parse::<u16>() {
                    Ok(0) => {
                        validate!(errors, "server.bind", "port cannot be 0");
                    }
                    Err(_) => {
                        validate!(
                            errors,
                            "server.bind",
                            "invalid port '{}': must be a number 1-65535",
                            port_str
                        );
                    }
                    Ok(_) => {} // Valid port
                }
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
    /// Database file path.
    pub path: PathBuf,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            path: aranet_store::default_db_path(),
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

        errors
    }
}

/// Security configuration for API protection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    /// Enable API key authentication.
    /// When enabled, clients must provide the API key in the `X-API-Key` header.
    pub api_key_enabled: bool,
    /// The API key required for authentication (if enabled).
    /// Should be a secure random string of at least 32 characters.
    pub api_key: Option<String>,
    /// Enable rate limiting.
    pub rate_limit_enabled: bool,
    /// Maximum requests per window.
    #[serde(default = "default_rate_limit_requests")]
    pub rate_limit_requests: u32,
    /// Rate limit window in seconds.
    #[serde(default = "default_rate_limit_window")]
    pub rate_limit_window_secs: u64,
    /// Maximum number of tracked IPs for rate limiting.
    ///
    /// When the number of tracked IPs exceeds this limit, the oldest entries
    /// are evicted to prevent unbounded memory growth from many unique IPs.
    #[serde(default = "default_rate_limit_max_entries")]
    pub rate_limit_max_entries: usize,
    /// Allowed CORS origins.
    ///
    /// By default, only localhost origins are allowed. Set to `["*"]` to allow
    /// all origins (not recommended for production).
    ///
    /// Examples: `["http://localhost:3000", "http://127.0.0.1:8080"]`
    #[serde(default = "default_cors_origins")]
    pub cors_origins: Vec<String>,
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

fn default_cors_origins() -> Vec<String> {
    vec![
        "http://localhost".to_string(),
        "http://127.0.0.1".to_string(),
    ]
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            api_key_enabled: false,
            api_key: None,
            // Rate limiting enabled by default to prevent DoS attacks
            rate_limit_enabled: true,
            rate_limit_requests: default_rate_limit_requests(),
            rate_limit_window_secs: default_rate_limit_window(),
            rate_limit_max_entries: default_rate_limit_max_entries(),
            cors_origins: default_cors_origins(),
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

            if self.topic_prefix.is_empty() {
                validate!(errors, "mqtt.topic_prefix", "topic prefix cannot be empty");
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
    /// Device address or name.
    pub address: String,
    /// Friendly alias for the device.
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

fn default_poll_interval() -> u64 {
    60
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

        // Alias validation (if provided)
        if let Some(alias) = &self.alias
            && alias.is_empty()
        {
            validate!(
                errors,
                format!("{}.alias", prefix),
                "alias cannot be empty string (use null/omit instead)"
            );
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
    /// CO2 threshold in ppm (notify when exceeded).
    #[serde(default = "default_co2_threshold")]
    pub co2_threshold: u16,
    /// Radon threshold in Bq/m³ (notify when exceeded).
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

/// Default configuration file path.
pub fn default_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("aranet")
        .join("server.toml")
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
    fn test_config_error_display() {
        let error = ConfigError::Read {
            path: PathBuf::from("/test/path"),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "not found"),
        };
        let display = format!("{}", error);
        assert!(display.contains("/test/path"));
        assert!(display.contains("not found"));
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
    fn test_server_bind_validation() {
        // Valid bind addresses
        let valid = ServerConfig {
            bind: "127.0.0.1:8080".to_string(),
            ..Default::default()
        };
        assert!(valid.validate().is_empty());

        let valid_ipv6 = ServerConfig {
            bind: "[::1]:8080".to_string(),
            ..Default::default()
        };
        assert!(valid_ipv6.validate().is_empty());

        let valid_hostname = ServerConfig {
            bind: "localhost:8080".to_string(),
            ..Default::default()
        };
        assert!(valid_hostname.validate().is_empty());

        // Invalid: empty
        let empty = ServerConfig {
            bind: "".to_string(),
            ..Default::default()
        };
        let errors = empty.validate();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("cannot be empty"));

        // Invalid: no port
        let no_port = ServerConfig {
            bind: "127.0.0.1".to_string(),
            ..Default::default()
        };
        let errors = no_port.validate();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("host:port"));

        // Invalid: port 0
        let port_zero = ServerConfig {
            bind: "127.0.0.1:0".to_string(),
            ..Default::default()
        };
        let errors = port_zero.validate();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("cannot be 0"));

        // Invalid: non-numeric port
        let bad_port = ServerConfig {
            bind: "127.0.0.1:abc".to_string(),
            ..Default::default()
        };
        let errors = bad_port.validate();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("must be a number"));

        // Invalid: zero broadcast buffer
        let zero_buffer = ServerConfig {
            broadcast_buffer: 0,
            ..Default::default()
        };
        let errors = zero_buffer.validate();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].field.contains("broadcast_buffer"));
    }

    #[test]
    fn test_storage_path_validation() {
        // Valid path
        let valid = StorageConfig {
            path: PathBuf::from("/data/aranet.db"),
        };
        assert!(valid.validate().is_empty());

        // Invalid: empty path
        let empty = StorageConfig {
            path: PathBuf::new(),
        };
        let errors = empty.validate();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("cannot be empty"));
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
}
