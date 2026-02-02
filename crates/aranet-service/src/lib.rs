//! Background collector and HTTP REST API for Aranet sensors.
//!
//! This crate provides a service that:
//! - Polls configured Aranet devices on a schedule
//! - Stores readings in the local database
//! - Exposes a REST API for querying data
//! - Provides WebSocket connections for real-time updates
//! - Optional API key authentication and rate limiting
//!
//! # REST API Endpoints
//!
//! - `GET /api/health` - Service health check (no auth required)
//! - `GET /api/devices` - List all known devices
//! - `GET /api/devices/:id` - Get device info
//! - `GET /api/devices/:id/current` - Latest reading for device
//! - `GET /api/devices/:id/readings` - Query readings with filters
//! - `GET /api/devices/:id/history` - Query cached history
//! - `GET /api/readings` - All readings across devices
//! - `WS /api/ws` - Real-time readings stream
//!
//! # Configuration
//!
//! The service reads configuration from `~/.config/aranet/server.toml`:
//!
//! ```toml
//! [server]
//! bind = "127.0.0.1:8080"
//!
//! [storage]
//! path = "~/.local/share/aranet/data.db"
//!
//! [[devices]]
//! address = "Aranet4 17C3C"
//! alias = "office"
//! poll_interval = 60
//! ```
//!
//! # Security
//!
//! Optional security features can be enabled:
//!
//! ```toml
//! [security]
//! # Require X-API-Key header for all requests (except /api/health)
//! api_key_enabled = true
//! api_key = "your-secure-random-key-at-least-16-chars"
//!
//! # Rate limit requests per IP address
//! rate_limit_enabled = true
//! rate_limit_requests = 100   # max requests per window
//! rate_limit_window_secs = 60 # window duration
//! ```
//!
//! For WebSocket connections, browsers cannot set custom headers. Use the `token`
//! query parameter instead: `ws://localhost:8080/api/ws?token=your-api-key`
//!
//! # Platform Setup
//!
//! ## macOS
//!
//! ### Bluetooth Permissions
//!
//! The Aranet devices use Bluetooth Low Energy. On macOS, you need to grant
//! Bluetooth permissions:
//!
//! 1. **Terminal App**: When running from Terminal, the Terminal app must have
//!    Bluetooth permission in System Preferences > Privacy & Security > Bluetooth.
//!
//! 2. **VS Code Terminal**: Add VS Code to the Bluetooth permissions list.
//!
//! 3. **LaunchAgent**: For background services, add `aranet-service` to the
//!    Bluetooth permission list. You may need to use a signed binary or run
//!    with appropriate entitlements.
//!
//! ### User-Level Service (Recommended)
//!
//! Install as a user LaunchAgent (no root required):
//!
//! ```bash
//! # Install the service
//! aranet-service service install --user
//!
//! # Start the service
//! aranet-service service start --user
//!
//! # Check status
//! aranet-service service status --user
//!
//! # Stop and uninstall
//! aranet-service service stop --user
//! aranet-service service uninstall --user
//! ```
//!
//! The LaunchAgent plist is created at `~/Library/LaunchAgents/dev.rye.aranet.plist`.
//!
//! ## Linux
//!
//! ### BlueZ D-Bus Access
//!
//! The service needs access to the BlueZ D-Bus interface. For user-level services:
//!
//! 1. **Ensure your user is in the bluetooth group:**
//!    ```bash
//!    sudo usermod -a -G bluetooth $USER
//!    # Log out and back in for group changes to take effect
//!    ```
//!
//! 2. **D-Bus session access**: User-level systemd services need the D-Bus session.
//!    Create a drop-in config if needed:
//!    ```bash
//!    mkdir -p ~/.config/systemd/user/aranet.service.d
//!    cat > ~/.config/systemd/user/aranet.service.d/dbus.conf << EOF
//!    [Service]
//!    Environment="DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/%U/bus"
//!    EOF
//!    systemctl --user daemon-reload
//!    ```
//!
//! ### User-Level Service
//!
//! ```bash
//! # Install the user service
//! aranet-service service install --user
//!
//! # Enable and start
//! systemctl --user enable --now dev.rye.aranet
//!
//! # Check status
//! systemctl --user status dev.rye.aranet
//!
//! # View logs
//! journalctl --user -u dev.rye.aranet -f
//!
//! # Stop and uninstall
//! systemctl --user stop dev.rye.aranet
//! aranet-service service uninstall --user
//! ```
//!
//! ### System-Level Service
//!
//! For system services, you need to create a dedicated user:
//!
//! ```bash
//! # Create aranet user (with bluetooth group membership)
//! sudo useradd -r -s /sbin/nologin -G bluetooth aranet
//!
//! # Install as system service
//! sudo aranet-service service install
//!
//! # Start the service
//! sudo systemctl enable --now dev.rye.aranet
//! ```
//!
//! ## Windows
//!
//! ### Bluetooth Permissions
//!
//! Windows requires the app to be granted Bluetooth access through Settings:
//! - Settings > Privacy & Security > Bluetooth > Allow apps to access your Bluetooth
//!
//! ### Running as a Service
//!
//! On Windows, the service runs as a Windows Service. Install and manage via:
//!
//! ```powershell
//! # Run as Administrator
//! aranet-service service install
//! aranet-service service start
//!
//! # Check status (in Services panel or via)
//! aranet-service service status
//!
//! # Stop and uninstall
//! aranet-service service stop
//! aranet-service service uninstall
//! ```
//!
//! **Note**: Windows Services run in session 0 without a desktop, which may affect
//! Bluetooth access. Consider using Task Scheduler to run the service at logon
//! if you encounter Bluetooth issues:
//!
//! ```powershell
//! # Create a scheduled task to run at logon
//! schtasks /create /tn "AranetService" /tr "aranet-service run" /sc onlogon /rl highest
//! ```

pub mod api;
pub mod collector;
pub mod config;
pub mod middleware;
pub mod state;
pub mod ws;

pub use collector::Collector;
pub use config::{
    Config, ConfigError, DeviceConfig, MqttConfig, PrometheusConfig, SecurityConfig, ServerConfig,
    StorageConfig,
};
pub use state::{AppState, ReadingEvent};

#[cfg(feature = "mqtt")]
pub mod mqtt;

#[cfg(feature = "prometheus")]
pub mod prometheus;
