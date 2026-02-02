//! Platform-specific Bluetooth configuration and tuning.
//!
//! This module provides platform-aware defaults for BLE operations,
//! accounting for differences between operating systems and BLE stacks.
//!
//! # Platform Differences
//!
//! | Platform | BLE Stack | Device ID Format | Notes |
//! |----------|-----------|------------------|-------|
//! | macOS | CoreBluetooth | UUID | Longer scan times needed (ads ~4s apart) |
//! | Linux | BlueZ | MAC Address | May need longer connection timeouts |
//! | Windows | WinRT | MAC Address | Generally reliable defaults |
//!
//! # macOS UUID Behavior
//!
//! On macOS, CoreBluetooth does **not** expose Bluetooth MAC addresses. Instead, it assigns
//! a UUID to each discovered device. This has important implications:
//!
//! ## UUID Stability
//!
//! - **Same Mac, same device**: The UUID is stable for a given device on a given Mac.
//!   You can reconnect to the same device using the UUID.
//!
//! - **Different Macs**: Each Mac assigns a **different** UUID to the same physical device.
//!   The UUID `A1B2C3D4-...` on Mac A will be different from the UUID assigned on Mac B.
//!
//! - **Bluetooth reset**: The UUID may change if you reset Bluetooth settings or unpair
//!   all devices. This is rare but can happen.
//!
//! ## Cross-Platform Considerations
//!
//! For applications that need to identify devices across platforms or machines:
//!
//! 1. **Use device names**: Device names (e.g., "Aranet4 12345") are consistent across
//!    platforms and machines. However, names can be changed by users.
//!
//! 2. **Use serial numbers**: Each device has a unique serial number accessible via
//!    `device.read_device_info().serial`. This is the most reliable cross-platform ID.
//!
//! 3. **Use the aliasing system**: Create user-friendly aliases (e.g., "Living Room")
//!    that map to the appropriate platform-specific identifier.
//!
//! ## Example: Cross-Platform Device Storage
//!
//! ```ignore
//! use aranet_core::platform::{DeviceAlias, AliasStore};
//!
//! // Create an alias that works across platforms
//! let alias = DeviceAlias::new("Living Room CO2 Sensor")
//!     .with_serial("123456")                    // Primary: serial number
//!     .with_name("Aranet4 12345")               // Fallback: device name
//!     .with_mac("AA:BB:CC:DD:EE:FF")            // Linux/Windows: MAC address
//!     .with_uuid("A1B2C3D4-E5F6-...");          // macOS: CoreBluetooth UUID
//!
//! // Resolve the alias on the current platform
//! let identifier = alias.resolve();  // Returns appropriate ID for this platform
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use aranet_core::platform::{PlatformConfig, current_platform};
//!
//! let config = PlatformConfig::for_current_platform();
//! let scan_options = ScanOptions::default()
//!     .duration(config.recommended_scan_duration);
//! ```

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Platform identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// macOS with CoreBluetooth
    MacOS,
    /// Linux with BlueZ
    Linux,
    /// Windows with WinRT
    Windows,
    /// Unknown or unsupported platform
    Unknown,
}

impl Platform {
    /// Detect the current platform.
    pub fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Platform::MacOS
        }
        #[cfg(target_os = "linux")]
        {
            Platform::Linux
        }
        #[cfg(target_os = "windows")]
        {
            Platform::Windows
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            Platform::Unknown
        }
    }
}

/// Platform-specific BLE configuration.
#[derive(Debug, Clone)]
pub struct PlatformConfig {
    /// The platform this configuration is for.
    pub platform: Platform,

    /// Recommended scan duration for device discovery.
    ///
    /// - macOS: Longer (8s) because advertisements can be 4+ seconds apart
    /// - Linux: Medium (5s) with BlueZ
    /// - Windows: Medium (5s)
    pub recommended_scan_duration: Duration,

    /// Minimum scan duration for quick scans.
    pub minimum_scan_duration: Duration,

    /// Recommended connection timeout.
    ///
    /// - macOS: Shorter (10s) as CoreBluetooth is generally faster
    /// - Linux: Longer (15s) as BlueZ may have overhead
    /// - Windows: Medium (12s)
    pub recommended_connection_timeout: Duration,

    /// Recommended read/write operation timeout.
    pub recommended_operation_timeout: Duration,

    /// Delay between consecutive BLE operations to avoid overwhelming the stack.
    pub operation_delay: Duration,

    /// Whether the platform exposes MAC addresses (false on macOS).
    pub exposes_mac_address: bool,

    /// Recommended number of scan retries.
    pub recommended_scan_retries: u32,

    /// Recommended delay between scan retries.
    pub scan_retry_delay: Duration,

    /// Maximum recommended concurrent connections.
    ///
    /// Most BLE adapters support 5-7 concurrent connections.
    pub max_concurrent_connections: usize,
}

impl PlatformConfig {
    /// Get the configuration for the current platform.
    pub fn for_current_platform() -> Self {
        Self::for_platform(Platform::current())
    }

    /// Get the configuration for a specific platform.
    pub fn for_platform(platform: Platform) -> Self {
        match platform {
            Platform::MacOS => Self::macos(),
            Platform::Linux => Self::linux(),
            Platform::Windows => Self::windows(),
            Platform::Unknown => Self::default(),
        }
    }

    /// Configuration optimized for macOS with CoreBluetooth.
    pub fn macos() -> Self {
        Self {
            platform: Platform::MacOS,
            // Aranet devices advertise every ~4 seconds, need longer scans
            recommended_scan_duration: Duration::from_secs(8),
            minimum_scan_duration: Duration::from_secs(5),
            // CoreBluetooth is generally efficient
            recommended_connection_timeout: Duration::from_secs(10),
            recommended_operation_timeout: Duration::from_secs(8),
            // CoreBluetooth handles queuing well
            operation_delay: Duration::from_millis(20),
            // macOS uses UUIDs instead of MAC addresses
            exposes_mac_address: false,
            recommended_scan_retries: 3,
            scan_retry_delay: Duration::from_millis(500),
            // CoreBluetooth typically supports ~5 connections
            max_concurrent_connections: 5,
        }
    }

    /// Configuration optimized for Linux with BlueZ.
    pub fn linux() -> Self {
        Self {
            platform: Platform::Linux,
            // BlueZ can scan faster
            recommended_scan_duration: Duration::from_secs(5),
            minimum_scan_duration: Duration::from_secs(3),
            // BlueZ may have more overhead
            recommended_connection_timeout: Duration::from_secs(15),
            recommended_operation_timeout: Duration::from_secs(10),
            // BlueZ benefits from slightly longer delays
            operation_delay: Duration::from_millis(30),
            // Linux exposes MAC addresses
            exposes_mac_address: true,
            recommended_scan_retries: 3,
            scan_retry_delay: Duration::from_millis(500),
            // Linux adapters typically support ~7 connections
            max_concurrent_connections: 7,
        }
    }

    /// Configuration optimized for Windows with WinRT.
    pub fn windows() -> Self {
        Self {
            platform: Platform::Windows,
            recommended_scan_duration: Duration::from_secs(5),
            minimum_scan_duration: Duration::from_secs(3),
            recommended_connection_timeout: Duration::from_secs(12),
            recommended_operation_timeout: Duration::from_secs(10),
            operation_delay: Duration::from_millis(25),
            // Windows exposes MAC addresses
            exposes_mac_address: true,
            recommended_scan_retries: 3,
            scan_retry_delay: Duration::from_millis(500),
            // Windows adapters typically support ~5-6 connections
            max_concurrent_connections: 5,
        }
    }
}

impl Default for PlatformConfig {
    /// Default configuration that works reasonably on all platforms.
    fn default() -> Self {
        Self {
            platform: Platform::Unknown,
            recommended_scan_duration: Duration::from_secs(6),
            minimum_scan_duration: Duration::from_secs(4),
            recommended_connection_timeout: Duration::from_secs(15),
            recommended_operation_timeout: Duration::from_secs(10),
            operation_delay: Duration::from_millis(30),
            exposes_mac_address: true,
            recommended_scan_retries: 3,
            scan_retry_delay: Duration::from_millis(500),
            max_concurrent_connections: 5,
        }
    }
}

/// Get the current platform.
pub fn current_platform() -> Platform {
    Platform::current()
}

/// Get platform-specific configuration for the current platform.
pub fn platform_config() -> PlatformConfig {
    PlatformConfig::for_current_platform()
}

// ==================== Device Aliasing System ====================

/// A cross-platform device alias that can store multiple identifiers.
///
/// This allows identifying the same physical device across different platforms
/// and machines, where the identifier format varies.
///
/// # Example
///
/// ```
/// use aranet_core::platform::DeviceAlias;
///
/// let alias = DeviceAlias::new("Living Room")
///     .with_serial("SN123456")
///     .with_name("Aranet4 12345")
///     .with_mac("AA:BB:CC:DD:EE:FF");
///
/// // Get the best identifier for the current platform
/// let id = alias.resolve();
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceAlias {
    /// User-friendly name for this device.
    pub alias: String,
    /// Device serial number (most reliable cross-platform ID).
    pub serial: Option<String>,
    /// Device name (e.g., "Aranet4 12345").
    pub name: Option<String>,
    /// Bluetooth MAC address (Linux/Windows).
    pub mac_address: Option<String>,
    /// CoreBluetooth UUID (macOS only).
    pub macos_uuid: Option<String>,
    /// Notes or description for this device.
    pub notes: Option<String>,
    /// When this alias was created.
    pub created_at: Option<String>,
    /// When this alias was last updated.
    pub updated_at: Option<String>,
}

impl DeviceAlias {
    /// Create a new device alias with the given user-friendly name.
    pub fn new(alias: impl Into<String>) -> Self {
        let now = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .ok();

        Self {
            alias: alias.into(),
            serial: None,
            name: None,
            mac_address: None,
            macos_uuid: None,
            notes: None,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    /// Set the device serial number.
    #[must_use]
    pub fn with_serial(mut self, serial: impl Into<String>) -> Self {
        self.serial = Some(serial.into());
        self
    }

    /// Set the device name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the MAC address (for Linux/Windows).
    #[must_use]
    pub fn with_mac(mut self, mac: impl Into<String>) -> Self {
        self.mac_address = Some(mac.into());
        self
    }

    /// Set the macOS UUID.
    #[must_use]
    pub fn with_uuid(mut self, uuid: impl Into<String>) -> Self {
        self.macos_uuid = Some(uuid.into());
        self
    }

    /// Set notes for this device.
    #[must_use]
    pub fn with_notes(mut self, notes: impl Into<String>) -> Self {
        self.notes = Some(notes.into());
        self
    }

    /// Resolve the alias to a platform-appropriate identifier.
    ///
    /// Resolution order:
    /// 1. On macOS: macos_uuid → name → serial
    /// 2. On Linux/Windows: mac_address → name → serial
    ///
    /// Returns `None` if no suitable identifier is available.
    pub fn resolve(&self) -> Option<String> {
        let platform = Platform::current();

        match platform {
            Platform::MacOS => {
                // On macOS, prefer UUID, then name, then serial
                self.macos_uuid
                    .clone()
                    .or_else(|| self.name.clone())
                    .or_else(|| self.serial.clone())
            }
            Platform::Linux | Platform::Windows => {
                // On Linux/Windows, prefer MAC address, then name, then serial
                self.mac_address
                    .clone()
                    .or_else(|| self.name.clone())
                    .or_else(|| self.serial.clone())
            }
            Platform::Unknown => {
                // Fall back to name or serial
                self.name.clone().or_else(|| self.serial.clone())
            }
        }
    }

    /// Check if this alias matches a given identifier.
    ///
    /// This checks against all stored identifiers (serial, name, MAC, UUID).
    pub fn matches(&self, identifier: &str) -> bool {
        self.serial.as_deref() == Some(identifier)
            || self.name.as_deref() == Some(identifier)
            || self.mac_address.as_deref() == Some(identifier)
            || self.macos_uuid.as_deref() == Some(identifier)
    }

    /// Update the platform-specific identifier.
    ///
    /// Call this after connecting to a device to update the alias with
    /// the current platform's identifier.
    pub fn update_identifier(&mut self, identifier: &str) {
        let platform = Platform::current();
        match platform {
            Platform::MacOS => {
                // On macOS, the identifier is a UUID
                self.macos_uuid = Some(identifier.to_string());
            }
            Platform::Linux | Platform::Windows => {
                // On Linux/Windows, the identifier is a MAC address
                // (unless it looks like a UUID)
                if identifier.contains('-') && identifier.len() > 20 {
                    // Looks like a UUID, might be running on macOS
                    self.macos_uuid = Some(identifier.to_string());
                } else {
                    self.mac_address = Some(identifier.to_string());
                }
            }
            Platform::Unknown => {
                // Store as name if we can't determine the platform
                self.name = Some(identifier.to_string());
            }
        }

        self.updated_at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .ok();
    }
}

/// An in-memory store for device aliases.
///
/// This provides a simple way to manage device aliases at runtime.
/// For persistent storage, serialize the aliases to a file.
///
/// # Thread Safety
///
/// This store is thread-safe and can be shared across tasks.
///
/// # Example
///
/// ```
/// use aranet_core::platform::{AliasStore, DeviceAlias};
///
/// let store = AliasStore::new();
///
/// // Add an alias
/// let alias = DeviceAlias::new("Kitchen")
///     .with_name("Aranet4 12345");
/// store.add(alias);
///
/// // Find by alias name
/// if let Some(alias) = store.get("Kitchen") {
///     println!("Found: {:?}", alias.resolve());
/// }
/// ```
#[derive(Debug, Default)]
pub struct AliasStore {
    aliases: RwLock<HashMap<String, DeviceAlias>>,
}

impl AliasStore {
    /// Create a new empty alias store.
    pub fn new() -> Self {
        Self {
            aliases: RwLock::new(HashMap::new()),
        }
    }

    /// Add or update an alias in the store.
    pub fn add(&self, alias: DeviceAlias) {
        let mut aliases = self
            .aliases
            .write()
            .expect("alias store lock poisoned - a thread panicked while holding the lock");
        aliases.insert(alias.alias.clone(), alias);
    }

    /// Get an alias by its user-friendly name.
    pub fn get(&self, alias_name: &str) -> Option<DeviceAlias> {
        let aliases = self
            .aliases
            .read()
            .expect("alias store lock poisoned - a thread panicked while holding the lock");
        aliases.get(alias_name).cloned()
    }

    /// Remove an alias by name.
    pub fn remove(&self, alias_name: &str) -> Option<DeviceAlias> {
        let mut aliases = self
            .aliases
            .write()
            .expect("alias store lock poisoned - a thread panicked while holding the lock");
        aliases.remove(alias_name)
    }

    /// Find an alias by any of its identifiers.
    pub fn find_by_identifier(&self, identifier: &str) -> Option<DeviceAlias> {
        let aliases = self
            .aliases
            .read()
            .expect("alias store lock poisoned - a thread panicked while holding the lock");
        aliases.values().find(|a| a.matches(identifier)).cloned()
    }

    /// Get all aliases.
    pub fn all(&self) -> Vec<DeviceAlias> {
        let aliases = self
            .aliases
            .read()
            .expect("alias store lock poisoned - a thread panicked while holding the lock");
        aliases.values().cloned().collect()
    }

    /// Get the number of aliases.
    pub fn len(&self) -> usize {
        let aliases = self
            .aliases
            .read()
            .expect("alias store lock poisoned - a thread panicked while holding the lock");
        aliases.len()
    }

    /// Check if the store is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clear all aliases.
    pub fn clear(&self) {
        let mut aliases = self
            .aliases
            .write()
            .expect("alias store lock poisoned - a thread panicked while holding the lock");
        aliases.clear();
    }

    /// Resolve an alias name to a platform-appropriate identifier.
    ///
    /// If the alias is found, returns its resolved identifier.
    /// If not found, returns the input string unchanged (it might already be an identifier).
    pub fn resolve(&self, alias_or_identifier: &str) -> String {
        if let Some(alias) = self.get(alias_or_identifier) {
            alias
                .resolve()
                .unwrap_or_else(|| alias_or_identifier.to_string())
        } else {
            alias_or_identifier.to_string()
        }
    }

    /// Export all aliases to JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        let aliases = self
            .aliases
            .read()
            .expect("alias store lock poisoned - a thread panicked while holding the lock");
        serde_json::to_string_pretty(&*aliases)
    }

    /// Import aliases from JSON.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let aliases: HashMap<String, DeviceAlias> = serde_json::from_str(json)?;
        Ok(Self {
            aliases: RwLock::new(aliases),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_detection() {
        let platform = Platform::current();
        // Just verify it returns a valid platform
        assert!(matches!(
            platform,
            Platform::MacOS | Platform::Linux | Platform::Windows | Platform::Unknown
        ));
    }

    #[test]
    fn test_platform_config_macos() {
        let config = PlatformConfig::macos();
        assert_eq!(config.platform, Platform::MacOS);
        assert!(!config.exposes_mac_address);
        assert!(config.recommended_scan_duration >= Duration::from_secs(5));
    }

    #[test]
    fn test_platform_config_linux() {
        let config = PlatformConfig::linux();
        assert_eq!(config.platform, Platform::Linux);
        assert!(config.exposes_mac_address);
    }

    #[test]
    fn test_platform_config_windows() {
        let config = PlatformConfig::windows();
        assert_eq!(config.platform, Platform::Windows);
        assert!(config.exposes_mac_address);
    }

    #[test]
    fn test_current_platform_config() {
        let config = PlatformConfig::for_current_platform();
        // Verify it returns sensible values
        assert!(config.recommended_scan_duration > Duration::ZERO);
        assert!(config.recommended_connection_timeout > Duration::ZERO);
        assert!(config.max_concurrent_connections > 0);
    }
}
