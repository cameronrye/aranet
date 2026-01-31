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
