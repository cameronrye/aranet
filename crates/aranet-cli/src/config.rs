//! Configuration file management.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Configuration file structure
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Default device address
    #[serde(default)]
    pub device: Option<String>,

    /// Default output format
    #[serde(default)]
    pub format: Option<String>,

    /// Disable colored output
    #[serde(default)]
    pub no_color: bool,

    /// Use Fahrenheit for temperature
    #[serde(default)]
    pub fahrenheit: bool,

    /// Use inHg for pressure (instead of hPa)
    #[serde(default)]
    pub inhg: bool,

    /// Use Bq/m³ for radon (instead of pCi/L)
    #[serde(default)]
    pub bq: bool,

    /// Connection timeout in seconds
    #[serde(default)]
    pub timeout: Option<u64>,

    /// Device aliases (friendly name -> device address)
    #[serde(default)]
    pub aliases: HashMap<String, String>,

    /// Last successfully connected device (auto-updated)
    #[serde(default)]
    pub last_device: Option<String>,

    /// Name of the last connected device (for display)
    #[serde(default)]
    pub last_device_name: Option<String>,

    /// Behavior settings for unified data architecture
    #[serde(default)]
    pub behavior: BehaviorConfig,

    /// GUI-specific settings
    #[serde(default)]
    pub gui: GuiConfig,
}

/// GUI-specific configuration settings.
///
/// Controls appearance and behavior of the native GUI application.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiConfig {
    /// Theme preference: "dark", "light", or "system"
    #[serde(default = "default_theme")]
    pub theme: String,

    /// Show colored tray icon for elevated CO2 levels.
    /// When false, always uses native template icon (auto dark/light).
    /// When true, shows colored icons (yellow/orange/red) for elevated CO2.
    #[serde(default = "default_true")]
    pub colored_tray_icon: bool,

    /// Enable desktop notifications for CO2 threshold alerts.
    #[serde(default = "default_true")]
    pub notifications_enabled: bool,

    /// Play sound with desktop notifications.
    #[serde(default = "default_true")]
    pub notification_sound: bool,

    /// Start the application minimized to system tray.
    #[serde(default)]
    pub start_minimized: bool,

    /// Minimize to tray instead of quitting when closing window.
    #[serde(default = "default_true")]
    pub close_to_tray: bool,

    /// Temperature unit preference: "celsius" or "fahrenheit".
    /// Used when device settings are not available.
    #[serde(default = "default_celsius")]
    pub temperature_unit: String,

    /// Pressure unit preference: "hpa" or "inhg".
    /// Used for pressure display.
    #[serde(default = "default_hpa")]
    pub pressure_unit: String,

    /// Whether the sidebar is collapsed.
    #[serde(default)]
    pub sidebar_collapsed: bool,

    /// Enable compact mode for denser layout on smaller screens.
    #[serde(default)]
    pub compact_mode: bool,

    /// Remembered window width.
    #[serde(default)]
    pub window_width: Option<f32>,

    /// Remembered window height.
    #[serde(default)]
    pub window_height: Option<f32>,

    /// Remembered window X position.
    #[serde(default)]
    pub window_x: Option<f32>,

    /// Remembered window Y position.
    #[serde(default)]
    pub window_y: Option<f32>,

    /// CO2 warning threshold in ppm (yellow/amber indicator).
    #[serde(default = "default_co2_warning")]
    pub co2_warning_threshold: u16,

    /// CO2 danger threshold in ppm (red indicator).
    #[serde(default = "default_co2_danger")]
    pub co2_danger_threshold: u16,

    /// Radon warning threshold in Bq/m³.
    #[serde(default = "default_radon_warning")]
    pub radon_warning_threshold: u32,

    /// Radon danger threshold in Bq/m³.
    #[serde(default = "default_radon_danger")]
    pub radon_danger_threshold: u32,

    /// Default export format: "csv" or "json".
    #[serde(default = "default_export_format")]
    pub default_export_format: String,

    /// Custom export directory path. Empty string means use default (Downloads).
    #[serde(default)]
    pub export_directory: String,

    /// URL for the aranet-service REST API.
    /// Default: "http://localhost:8080"
    #[serde(default = "default_service_url")]
    pub service_url: String,

    /// Show CO2 readings in dashboard.
    #[serde(default = "default_true")]
    pub show_co2: bool,

    /// Show temperature readings in dashboard.
    #[serde(default = "default_true")]
    pub show_temperature: bool,

    /// Show humidity readings in dashboard.
    #[serde(default = "default_true")]
    pub show_humidity: bool,

    /// Show pressure readings in dashboard.
    #[serde(default = "default_true")]
    pub show_pressure: bool,
}

fn default_service_url() -> String {
    "http://localhost:8080".to_string()
}

fn default_theme() -> String {
    "dark".to_string()
}

fn default_celsius() -> String {
    "celsius".to_string()
}

fn default_hpa() -> String {
    "hpa".to_string()
}

fn default_co2_warning() -> u16 {
    1000
}

fn default_co2_danger() -> u16 {
    1400
}

fn default_radon_warning() -> u32 {
    100
}

fn default_radon_danger() -> u32 {
    150
}

fn default_export_format() -> String {
    "csv".to_string()
}

impl Default for GuiConfig {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            colored_tray_icon: true,
            notifications_enabled: true,
            notification_sound: true,
            start_minimized: false,
            close_to_tray: true,
            temperature_unit: default_celsius(),
            pressure_unit: default_hpa(),
            sidebar_collapsed: false,
            compact_mode: false,
            window_width: None,
            window_height: None,
            window_x: None,
            window_y: None,
            co2_warning_threshold: default_co2_warning(),
            co2_danger_threshold: default_co2_danger(),
            radon_warning_threshold: default_radon_warning(),
            radon_danger_threshold: default_radon_danger(),
            default_export_format: default_export_format(),
            export_directory: String::new(),
            service_url: default_service_url(),
            show_co2: true,
            show_temperature: true,
            show_humidity: true,
            show_pressure: true,
        }
    }
}

/// Behavior configuration for unified data architecture.
///
/// Controls automatic connection, sync, and device memory across all tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehaviorConfig {
    /// Auto-connect to known devices on startup (TUI/GUI)
    #[serde(default = "default_true")]
    pub auto_connect: bool,

    /// Auto-sync history on connection
    #[serde(default = "default_true")]
    pub auto_sync: bool,

    /// Remember devices in database after connection
    #[serde(default = "default_true")]
    pub remember_devices: bool,

    /// Load cached data (devices, readings) on startup
    #[serde(default = "default_true")]
    pub load_cache: bool,
}

fn default_true() -> bool {
    true
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            auto_connect: true,
            auto_sync: true,
            remember_devices: true,
            load_cache: true,
        }
    }
}

impl Config {
    /// Get the config file path
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("aranet")
            .join("config.toml")
    }

    /// Load config from file, or return default if not found
    pub fn load() -> Self {
        let path = Self::path();
        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(content) => match toml::from_str(&content) {
                    Ok(config) => return config,
                    Err(e) => {
                        eprintln!("Warning: Failed to parse config: {}", e);
                    }
                },
                Err(e) => {
                    eprintln!("Warning: Failed to read config: {}", e);
                }
            }
        }
        Self::default()
    }

    /// Save config to file
    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory: {}", parent.display())
            })?;
        }
        let content = toml::to_string_pretty(self).context("Failed to serialize config")?;
        fs::write(&path, content)
            .with_context(|| format!("Failed to write config: {}", path.display()))?;
        Ok(())
    }
}

/// Resolve device from arg, env var, or config.
/// Also resolves aliases: if the device matches an alias name, returns the address.
/// Falls back to last_device if no default device is set.
#[allow(dead_code)]
pub fn resolve_device(device: Option<String>, config: &Config) -> Option<String> {
    device
        .map(|d| resolve_alias(&d, config))
        .or_else(|| config.device.clone())
        .or_else(|| config.last_device.clone())
}

/// Resolve multiple devices, applying alias resolution to each.
/// Returns an empty Vec if no devices are specified.
pub fn resolve_devices(devices: Vec<String>, config: &Config) -> Vec<String> {
    devices
        .into_iter()
        .map(|d| resolve_alias(&d, config))
        .collect()
}

/// Resolve an alias to its device address, or return the original if not an alias.
pub fn resolve_alias(device: &str, config: &Config) -> String {
    config
        .aliases
        .get(device)
        .cloned()
        .unwrap_or_else(|| device.to_string())
}

/// Resolve an alias and return information about the resolution.
/// Returns (resolved_address, was_alias, original_alias_name).
pub fn resolve_alias_with_info(device: &str, config: &Config) -> (String, bool, Option<String>) {
    if let Some(address) = config.aliases.get(device) {
        (address.clone(), true, Some(device.to_string()))
    } else {
        (device.to_string(), false, None)
    }
}

/// Print alias resolution feedback if the user is not in quiet mode.
/// Call this after resolving an alias to inform the user which device was selected.
pub fn print_alias_feedback(original: &str, resolved: &str, quiet: bool) {
    if !quiet && original != resolved {
        eprintln!("Using device '{}' -> {}", original, resolved);
    }
}

/// Print device source feedback (e.g., "Using last connected device: ...").
pub fn print_device_source_feedback(device: &str, source: Option<&str>, quiet: bool) {
    if quiet {
        return;
    }
    match source {
        Some("default") => eprintln!("Using default device: {}", device),
        Some("last") => eprintln!("Using last connected device: {}", device),
        Some("store") => eprintln!("Using known device from database: {}", device),
        _ => {}
    }
}

/// Update the last connected device in config.
/// This is called after a successful connection.
pub fn update_last_device(identifier: &str, name: Option<&str>) -> Result<()> {
    let mut config = Config::load();
    config.last_device = Some(identifier.to_string());
    config.last_device_name = name.map(|n| n.to_string());
    config.save()
}

/// Get info about whether we're using a fallback device.
/// Returns (device_identifier, fallback_source) where fallback_source is:
/// - None if device was explicitly provided
/// - Some("default") if using default device
/// - Some("last") if using last connected device
/// - Some("store") if using known device from database
pub fn get_device_source(
    device: Option<&str>,
    config: &Config,
) -> (Option<String>, Option<&'static str>) {
    if let Some(d) = device {
        (Some(resolve_alias(d, config)), None)
    } else if let Some(d) = &config.device {
        (Some(d.clone()), Some("default"))
    } else if let Some(d) = &config.last_device {
        (Some(d.clone()), Some("last"))
    } else if config.behavior.load_cache {
        // Try to get a known device from the store
        if let Some(d) = get_first_known_device() {
            (Some(d), Some("store"))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    }
}

/// Get the first known device from the store database.
///
/// Returns the device ID of the most recently connected device in the store,
/// or None if the store is empty or cannot be opened.
fn get_first_known_device() -> Option<String> {
    let store_path = aranet_store::default_db_path();
    let store = aranet_store::Store::open(&store_path).ok()?;
    let devices = store.list_devices().ok()?;
    devices.first().map(|d| d.id.clone())
}

/// Resolve timeout: use provided value, fall back to config, then default
pub fn resolve_timeout(cmd_timeout: u64, config: &Config, default: u64) -> u64 {
    // If the command timeout differs from clap's default, use it
    // Otherwise, check config, then fall back to the provided default
    if cmd_timeout != default {
        cmd_timeout
    } else {
        config.timeout.unwrap_or(default)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_device_prefers_arg() {
        let config = Config {
            device: Some("config-device".to_string()),
            ..Default::default()
        };
        let result = resolve_device(Some("arg-device".to_string()), &config);
        assert_eq!(result, Some("arg-device".to_string()));
    }

    #[test]
    fn test_resolve_device_falls_back_to_config() {
        let config = Config {
            device: Some("config-device".to_string()),
            ..Default::default()
        };
        let result = resolve_device(None, &config);
        assert_eq!(result, Some("config-device".to_string()));
    }

    #[test]
    fn test_resolve_device_none_when_both_empty() {
        let config = Config::default();
        let result = resolve_device(None, &config);
        assert_eq!(result, None);
    }

    #[test]
    fn test_resolve_timeout_uses_explicit_value() {
        let config = Config {
            timeout: Some(60),
            ..Default::default()
        };
        // Explicit value differs from default, so use it
        let result = resolve_timeout(45, &config, 30);
        assert_eq!(result, 45);
    }

    #[test]
    fn test_resolve_timeout_uses_config_when_default() {
        let config = Config {
            timeout: Some(60),
            ..Default::default()
        };
        // Value equals default, so use config
        let result = resolve_timeout(30, &config, 30);
        assert_eq!(result, 60);
    }

    #[test]
    fn test_resolve_timeout_uses_default_when_no_config() {
        let config = Config::default();
        // Value equals default and no config, so use default
        let result = resolve_timeout(30, &config, 30);
        assert_eq!(result, 30);
    }

    #[test]
    fn test_behavior_config_defaults_to_true() {
        let behavior = BehaviorConfig::default();
        assert!(behavior.auto_connect);
        assert!(behavior.auto_sync);
        assert!(behavior.remember_devices);
        assert!(behavior.load_cache);
    }

    #[test]
    fn test_config_has_default_behavior() {
        let config = Config::default();
        assert!(config.behavior.auto_connect);
        assert!(config.behavior.auto_sync);
        assert!(config.behavior.remember_devices);
        assert!(config.behavior.load_cache);
    }

    #[test]
    fn test_behavior_config_serialization() {
        let behavior = BehaviorConfig {
            auto_connect: false,
            auto_sync: true,
            remember_devices: false,
            load_cache: true,
        };
        let toml_str = toml::to_string(&behavior).unwrap();
        assert!(toml_str.contains("auto_connect = false"));
        assert!(toml_str.contains("auto_sync = true"));

        // Deserialize back
        let parsed: BehaviorConfig = toml::from_str(&toml_str).unwrap();
        assert!(!parsed.auto_connect);
        assert!(parsed.auto_sync);
        assert!(!parsed.remember_devices);
        assert!(parsed.load_cache);
    }

    // ========================================================================
    // resolve_alias tests
    // ========================================================================

    #[test]
    fn test_resolve_alias_found() {
        let mut aliases = std::collections::HashMap::new();
        aliases.insert("living-room".to_string(), "AA:BB:CC:DD:EE:FF".to_string());
        aliases.insert("bedroom".to_string(), "11:22:33:44:55:66".to_string());

        let config = Config {
            aliases,
            ..Default::default()
        };

        let result = resolve_alias("living-room", &config);
        assert_eq!(result, "AA:BB:CC:DD:EE:FF");
    }

    #[test]
    fn test_resolve_alias_not_found() {
        let config = Config::default();
        let result = resolve_alias("unknown-alias", &config);
        assert_eq!(result, "unknown-alias");
    }

    #[test]
    fn test_resolve_alias_empty_aliases() {
        let config = Config::default();
        let result = resolve_alias("some-device", &config);
        assert_eq!(result, "some-device");
    }

    #[test]
    fn test_resolve_alias_returns_address_unchanged() {
        let config = Config::default();
        // If you pass an actual address, it should return unchanged
        let result = resolve_alias("AA:BB:CC:DD:EE:FF", &config);
        assert_eq!(result, "AA:BB:CC:DD:EE:FF");
    }

    // ========================================================================
    // resolve_devices tests
    // ========================================================================

    #[test]
    fn test_resolve_devices_empty() {
        let config = Config::default();
        let result = resolve_devices(vec![], &config);
        assert!(result.is_empty());
    }

    #[test]
    fn test_resolve_devices_multiple() {
        let mut aliases = std::collections::HashMap::new();
        aliases.insert("room1".to_string(), "AA:BB:CC:DD:EE:FF".to_string());
        aliases.insert("room2".to_string(), "11:22:33:44:55:66".to_string());

        let config = Config {
            aliases,
            ..Default::default()
        };

        let devices = vec![
            "room1".to_string(),
            "room2".to_string(),
            "direct-address".to_string(),
        ];
        let result = resolve_devices(devices, &config);

        assert_eq!(result.len(), 3);
        assert_eq!(result[0], "AA:BB:CC:DD:EE:FF");
        assert_eq!(result[1], "11:22:33:44:55:66");
        assert_eq!(result[2], "direct-address");
    }

    #[test]
    fn test_resolve_devices_no_aliases() {
        let config = Config::default();
        let devices = vec!["device1".to_string(), "device2".to_string()];
        let result = resolve_devices(devices, &config);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0], "device1");
        assert_eq!(result[1], "device2");
    }

    // ========================================================================
    // get_device_source tests
    // ========================================================================

    #[test]
    fn test_get_device_source_explicit() {
        let config = Config::default();
        let (device, source) = get_device_source(Some("explicit-device"), &config);

        assert_eq!(device, Some("explicit-device".to_string()));
        assert_eq!(source, None); // No fallback source when explicit
    }

    #[test]
    fn test_get_device_source_from_default() {
        let config = Config {
            device: Some("default-device".to_string()),
            ..Default::default()
        };
        let (device, source) = get_device_source(None, &config);

        assert_eq!(device, Some("default-device".to_string()));
        assert_eq!(source, Some("default"));
    }

    #[test]
    fn test_get_device_source_from_last() {
        let config = Config {
            last_device: Some("last-device".to_string()),
            ..Default::default()
        };
        let (device, source) = get_device_source(None, &config);

        assert_eq!(device, Some("last-device".to_string()));
        assert_eq!(source, Some("last"));
    }

    #[test]
    fn test_get_device_source_prefers_default_over_last() {
        let config = Config {
            device: Some("default-device".to_string()),
            last_device: Some("last-device".to_string()),
            ..Default::default()
        };
        let (device, source) = get_device_source(None, &config);

        // Default should take precedence over last
        assert_eq!(device, Some("default-device".to_string()));
        assert_eq!(source, Some("default"));
    }

    #[test]
    fn test_get_device_source_resolves_alias() {
        let mut aliases = std::collections::HashMap::new();
        aliases.insert("my-sensor".to_string(), "AA:BB:CC:DD:EE:FF".to_string());

        let config = Config {
            aliases,
            ..Default::default()
        };
        let (device, source) = get_device_source(Some("my-sensor"), &config);

        assert_eq!(device, Some("AA:BB:CC:DD:EE:FF".to_string()));
        assert_eq!(source, None);
    }
}
