<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg">
    <img alt="Aranet" src="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg" height="60">
  </picture>
</p>

# aranet-service

Background collector and HTTP REST API for Aranet sensors.

A service daemon that continuously monitors Aranet devices and exposes sensor data via a REST API. Built with [Axum](https://github.com/tokio-rs/axum) for high-performance async HTTP handling.

## Features

- **Background collection** - Automatically poll configured devices at regular intervals
- **REST API** - Query current readings, history, and device information via HTTP
- **WebSocket support** - Real-time streaming of sensor updates
- **Local persistence** - Store readings in SQLite via aranet-store
- **Configurable** - TOML-based configuration for devices, intervals, and server settings
- **Health endpoint** - Monitor service status for integration with monitoring systems

## Installation

```bash
cargo install aranet-service
```

Or build from source:

```bash
cargo build --release -p aranet-service
```

## Usage

```bash
# Start the service with default configuration
aranet-service

# Specify a custom config file
aranet-service --config /path/to/config.toml

# Specify bind address and port
aranet-service --bind 0.0.0.0:8080
```

## Configuration

Create a configuration file at `~/.config/aranet/service.toml`:

```toml
[server]
bind = "127.0.0.1:3000"

[storage]
path = "~/.local/share/aranet/data.db"

[[devices]]
address = "AA:BB:CC:DD:EE:FF"
name = "Living Room"
poll_interval = 60  # seconds
```

## API Endpoints

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/health` | Service health check |
| GET | `/api/devices` | List all configured devices |
| GET | `/api/devices/:id` | Get device details |
| GET | `/api/devices/:id/current` | Get current reading |
| GET | `/api/devices/:id/readings` | Query stored readings |
| GET | `/api/devices/:id/history` | Query device history |
| GET | `/api/readings` | Query all readings across devices |
| WS | `/api/ws` | WebSocket for real-time updates |

### Query Parameters

For `/readings` and `/history` endpoints:

| Parameter | Type | Description |
|-----------|------|-------------|
| `since` | Unix timestamp | Filter records after this time |
| `until` | Unix timestamp | Filter records before this time |
| `limit` | Integer | Maximum number of records |
| `offset` | Integer | Skip this many records (pagination) |

## Example Requests

```bash
# Check service health
curl http://localhost:3000/health

# List devices
curl http://localhost:3000/api/devices

# Get current reading
curl http://localhost:3000/api/devices/living-room/current

# Query history with time range
curl "http://localhost:3000/api/devices/living-room/history?since=1705320000&limit=100"
```

## Related Crates

This crate is part of the [aranet](https://github.com/cameronrye/aranet) workspace:

| Crate | crates.io | Description |
|-------|-----------|-------------|
| [aranet-core](../aranet-core/) | [![crates.io](https://img.shields.io/crates/v/aranet-core.svg)](https://crates.io/crates/aranet-core) | Core BLE library for device communication |
| [aranet-types](../aranet-types/) | [![crates.io](https://img.shields.io/crates/v/aranet-types.svg)](https://crates.io/crates/aranet-types) | Shared types for sensor data |
| [aranet-store](../aranet-store/) | [![crates.io](https://img.shields.io/crates/v/aranet-store.svg)](https://crates.io/crates/aranet-store) | Local data persistence |
| [aranet-cli](../aranet-cli/) | [![crates.io](https://img.shields.io/crates/v/aranet-cli.svg)](https://crates.io/crates/aranet-cli) | Command-line interface |
| [aranet-tui](../aranet-tui/) | [![crates.io](https://img.shields.io/crates/v/aranet-tui.svg)](https://crates.io/crates/aranet-tui) | Terminal UI dashboard |
| [aranet-gui](../aranet-gui/) | - | Desktop GUI application |
| [aranet-wasm](../aranet-wasm/) | - | WebAssembly module |

## License

MIT

---

Made with ❤️ by [Cameron Rye](https://rye.dev/)

