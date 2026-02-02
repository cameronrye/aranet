<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg">
    <img alt="Aranet" src="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg" height="60">
  </picture>
</p>

# aranet-service

[![crates.io](https://img.shields.io/crates/v/aranet-service.svg)](https://crates.io/crates/aranet-service)
[![docs.rs](https://docs.rs/aranet-service/badge.svg)](https://docs.rs/aranet-service)

Background collector and HTTP REST API for Aranet sensors.

**[Full Documentation](https://cameronrye.github.io/aranet/)**

A service daemon that continuously monitors Aranet devices and exposes sensor data via a REST API. Built with [Axum](https://github.com/tokio-rs/axum) for high-performance async HTTP handling.

## Features

- **Background collection** - Automatically poll configured devices at regular intervals
- **REST API** - Query current readings, history, and device information via HTTP
- **WebSocket support** - Real-time streaming of sensor updates
- **Prometheus metrics** - `/metrics` endpoint for Grafana dashboards and alerting
- **MQTT publisher** - Broadcast readings to MQTT brokers for IoT integration
- **Local persistence** - Store readings in SQLite via aranet-store
- **Configurable** - TOML-based configuration for devices, intervals, and server settings
- **Health endpoint** - Monitor service status for integration with monitoring systems
- **Service management** - Install as system service (launchd/systemd/Windows Service)

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

# Prometheus metrics (optional)
[prometheus]
enabled = true
# push_gateway = "http://localhost:9091"  # Optional push gateway
# push_interval = 60  # Push interval in seconds

# MQTT publishing (optional)
[mqtt]
enabled = true
broker = "mqtt://localhost:1883"  # or mqtts:// for TLS
topic_prefix = "aranet"
client_id = "aranet-service"
qos = 1  # 0=AtMostOnce, 1=AtLeastOnce, 2=ExactlyOnce
retain = true
# username = "user"  # Optional authentication
# password = "secret"
```

## API Endpoints

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/health` | Service health check |
| GET | `/api/health/detailed` | Detailed health with database, collector, and platform diagnostics |
| GET | `/api/status` | Full service status with collector state |
| GET | `/api/devices` | List all configured devices |
| GET | `/api/devices/:id` | Get device details |
| GET | `/api/devices/:id/current` | Get current reading |
| GET | `/api/devices/:id/readings` | Query stored readings |
| GET | `/api/devices/:id/history` | Query device history |
| GET | `/api/readings` | Query all readings across devices |
| POST | `/api/collector/start` | Start background collector |
| POST | `/api/collector/stop` | Stop background collector |
| GET | `/api/config` | Get current configuration |
| PUT | `/api/config` | Update configuration |
| POST | `/api/config/devices` | Add device to monitoring |
| PUT | `/api/config/devices/:id` | Update device config |
| DELETE | `/api/config/devices/:id` | Remove device |
| GET | `/metrics` | Prometheus metrics endpoint |
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

# Get Prometheus metrics
curl http://localhost:3000/metrics
```

## Prometheus Metrics

When enabled, the `/metrics` endpoint exports sensor data in Prometheus format:

**Sensor readings (per device):**

- `aranet_co2_ppm` - CO2 concentration
- `aranet_temperature_celsius` - Temperature
- `aranet_humidity_percent` - Humidity
- `aranet_pressure_hpa` - Pressure
- `aranet_battery_percent` - Battery level
- `aranet_reading_age_seconds` - Age of reading

**Radon/radiation (if available):**

- `aranet_radon_bqm3` - Radon concentration
- `aranet_radiation_rate_usvh` - Radiation dose rate
- `aranet_radiation_total_msv` - Total radiation dose

**Collector statistics:**

- `aranet_collector_running` - Collector status (1=running, 0=stopped)
- `aranet_collector_uptime_seconds` - Collector uptime
- `aranet_device_poll_success_total` - Successful polls per device
- `aranet_device_poll_failure_total` - Failed polls per device

## MQTT Topics

When MQTT is enabled, readings are published to the following topics:

```
{prefix}/{device}/json           - Full reading as JSON
{prefix}/{device}/co2            - CO2 (ppm)
{prefix}/{device}/temperature    - Temperature (°C)
{prefix}/{device}/humidity       - Humidity (%)
{prefix}/{device}/pressure       - Pressure (hPa)
{prefix}/{device}/battery        - Battery level (%)
{prefix}/{device}/status         - Status (green/yellow/red/error)
{prefix}/{device}/radon          - Radon (Bq/m³, if available)
{prefix}/{device}/radiation_rate - Radiation rate (µSv/h, if available)
{prefix}/{device}/radiation_total - Total radiation (mSv, if available)
```

Where `{prefix}` is the configured topic prefix (default: "aranet") and `{device}` is the device alias or address.

## Service Management

Install and manage aranet-service as a system service:

```bash
# Install as user service
aranet-service service install --user

# Start/stop/status
aranet-service service start --user
aranet-service service stop --user
aranet-service service status --user

# Install as system service (requires root/admin)
sudo aranet-service service install
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
| [aranet-gui](../aranet-gui/) | [![crates.io](https://img.shields.io/crates/v/aranet-gui.svg)](https://crates.io/crates/aranet-gui) | Desktop GUI application |

## License

MIT

---

Made with ❤️ by [Cameron Rye](https://rye.dev/)

