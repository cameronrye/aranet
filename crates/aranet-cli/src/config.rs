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
}
