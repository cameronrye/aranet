//! Background collector and HTTP REST API for Aranet sensors.
//!
//! This crate provides a service that:
//! - Polls configured Aranet devices on a schedule
//! - Stores readings in the local database
//! - Exposes a REST API for querying data
//! - Provides WebSocket connections for real-time updates
//!
//! # REST API Endpoints
//!
//! - `GET /api/health` - Service health check
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

pub mod api;
pub mod collector;
pub mod config;
pub mod state;
pub mod ws;

pub use collector::Collector;
pub use config::{Config, ConfigError, DeviceConfig, ServerConfig, StorageConfig};
pub use state::{AppState, ReadingEvent};
