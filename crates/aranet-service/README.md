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

A service daemon that continuously monitors Aranet devices and exposes sensor data via a REST API. Built with [Axum](https://github.com/tokio-rs/axum) for high-performance async HTTP handling. The embedded dashboard is served at `/` and `/dashboard`.

## Features

- **Background collection** - Automatically poll configured devices at regular intervals
- **REST API** - Query current readings, history, and device information via HTTP
- **WebSocket support** - Real-time streaming of sensor updates
- **Prometheus metrics** - `/metrics` endpoint for Grafana dashboards and alerting
- **MQTT publisher** - Broadcast readings to MQTT brokers for IoT integration and Home Assistant auto-discovery
- **Webhook notifications** - Send HTTP alerts for CO2, radon, and battery thresholds
- **InfluxDB export** - Stream readings to InfluxDB v2 using line protocol
- **mDNS discovery** - Advertise `_aranet._tcp.local.` and `_http._tcp.local.` on the LAN, as `Aranet Service on <name>` at `aranet-<name>.local.`, when the service listens on a network address with API-key authentication
- **Embedded dashboard** - Serve a built-in monitoring UI at `/` and `/dashboard`
- **Local persistence** - Store readings in SQLite via aranet-store
- **Configurable** - TOML-based configuration for devices, intervals, and server settings
- **Health endpoint** - Monitor service status for integration with monitoring systems
- **Service management** - Install as a per-user service (launchd on macOS, systemd on Linux) or as an unprivileged systemd system service

## Installation

```bash
cargo install aranet-service --features full
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

# Listen on another address (an IP address and port)
aranet-service --bind 127.0.0.1:9090
```

Listening on a network address such as `0.0.0.0:8080` requires API-key authentication (`api_key_enabled = true` under `[security]`), or `security.allow_unauthenticated_network = true` when a firewall or an authenticating proxy protects the port. Without either, the service refuses to start. The service also refuses host names such as `localhost:8080`: use `127.0.0.1:8080`.

## Configuration

Create a configuration file at `~/.config/aranet/server.toml` (macOS: `~/Library/Application Support/aranet/server.toml`), or pass one with `--config`:

```toml
[server]
bind = "127.0.0.1:8080"

[storage]
path = "~/.local/share/aranet/data.db"  # `~/` is your home directory

# Optional: delete old data at startup and hourly (off by default).
# history_days also prunes the history that `aranet sync` cached.
# [storage.retention]
# readings_days = 90
# history_days = 365

[[devices]]
address = "AA:BB:CC:DD:EE:FF"
alias = "Living Room"
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
homeassistant = true
ha_discovery_prefix = "homeassistant"
# username = "user"  # Optional authentication
# password = "secret"

[webhooks]
enabled = true
co2_threshold = 1000
radon_threshold = 300
battery_threshold = 10
cooldown_secs = 300

[[webhooks.endpoints]]
url = "https://hooks.slack.com/services/T00/B00/xxx"
events = ["co2_high", "radon_high", "battery_low"]

[influxdb]
enabled = true
url = "http://localhost:8086"
token = "your-influxdb-token"
org = "my-org"
bucket = "aranet"
measurement = "aranet"
precision = "s"
```

## API Endpoints

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/health` | Service health check |
| GET | `/api/health/detailed` | Detailed health with database, collector, and platform diagnostics |
| GET | `/api/status` | Full service status with collector state |
| GET | `/api/devices` | List devices known to the database |
| GET | `/api/devices/current` | List latest readings for all devices |
| GET | `/api/devices/:id` | Get device details |
| GET | `/api/devices/:id/current` | Get current reading (includes `age_seconds`, `stale`) |
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

The dashboard shell routes `/` and `/dashboard` are public so browsers can load the UI. API, WebSocket, and metrics requests still honor the configured security settings.

If API key authentication is enabled, send the key as `X-API-Key` or `Authorization: Bearer <key>`; WebSocket clients can also use the `token` query parameter for `/api/ws`. A repeated `X-API-Key` or `Authorization` header, or an `X-API-Key` that isn't plain text, is never read as a key, so a request that sends its key that way is refused. While authentication is off, the service answers only local host names, IP addresses, this machine's name and `security.allowed_hosts` (421 otherwise), and rejects browser requests from origins other than its own and `security.cors_origins` (403).

### Query Parameters

For `/readings` and `/history` endpoints:

| Parameter | Type | Description |
|-----------|------|-------------|
| `since` | Unix timestamp | Filter records after this time |
| `until` | Unix timestamp | Filter records before this time |
| `limit` | Integer | Maximum number of records, 1 to 10000 (default 1000) |
| `offset` | Integer | Skip this many records, at most 1000000 (default 0) |

Records come newest first. Without `limit`, a response holds at most 1000 of them; `pagination.has_more` says whether there are more, and `pagination.limit` is the page size that was used. `limit=0`, a `limit` above 10000 and an `offset` above 1000000 are rejected with `400`; to go further back, narrow the range with `since` and `until`.

## Example Requests

```bash
# Check service health
curl http://localhost:8080/api/health

# List devices
curl http://localhost:8080/api/devices

# Latest readings for all devices
curl http://localhost:8080/api/devices/current

# Get current reading
curl http://localhost:8080/api/devices/Aranet4%2017C3C/current

# Query history with time range
curl "http://localhost:8080/api/devices/Aranet4%2017C3C/history?since=1705320000&limit=100"

# Get Prometheus metrics
curl http://localhost:8080/metrics
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
- `aranet_device_poll_success_total` - Successful polls per device since the service started
- `aranet_device_poll_failure_total` - Failed polls per device since the service started
- `aranet_device_poll_duration_ms` - Duration of the last poll in milliseconds

> **Note:** Sensor metrics are only emitted for capabilities a device actually has.
> For example, an Aranet2 (temperature/humidity only) will not emit `aranet_co2_ppm`
> or `aranet_pressure_hpa`.

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
{prefix}/{device}/radon_avg_24h  - 24-hour radon average (if available)
{prefix}/{device}/radon_avg_7d   - 7-day radon average (if available)
{prefix}/{device}/radon_avg_30d  - 30-day radon average (if available)
{prefix}/{device}/radiation_rate - Radiation rate (µSv/h, if available)
{prefix}/{device}/radiation_total - Total radiation (mSv, if available)
```

Where `{prefix}` is the configured topic prefix (default: "aranet") and `{device}` is the device alias or address.

## Service Management

Install and manage aranet-service as a background service:

```bash
# Install as a user service (a LaunchAgent on macOS, a systemd user service on Linux)
aranet-service service install --user

# Start/stop/status
aranet-service service start --user
aranet-service service stop --user
aranet-service service status --user

# Linux with systemd only: install the unprivileged system service aranet.service.
# The binary must be one that only root can change, such as /usr/local/bin/aranet-service.
sudo install -o root -g root -m 0755 ~/.cargo/bin/aranet-service /usr/local/bin/aranet-service
sudo /usr/local/bin/aranet-service service install
sudo systemctl start aranet
```

The system service runs as a dynamic `aranet` user without extra capabilities, with its config (`server.toml`) and database in `/var/lib/aranet`; `journalctl -u aranet` shows its log. System-level installs are refused on macOS, Windows, OpenRC and rc.d, where the service would run as root. See the [deployment guide](https://cameronrye.github.io/aranet/docs/service/deployment/) for Bluetooth group access, running `aranet server` under the unit, and Windows.

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
