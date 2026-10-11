# Aranet Docker Setup

This directory contains the Docker image for aranet-service (`Dockerfile`), its default configuration (`server.toml`) and a Compose stack that adds Prometheus and Grafana (`docker-compose.yml`).

## Quick Start

```bash
# From the repository root
docker compose -f docker/docker-compose.yml up -d

# View logs
docker compose -f docker/docker-compose.yml logs -f aranet-service
```

To build only the image, run `docker build -f docker/Dockerfile -t aranet-service .` from the repository root. There is no `Dockerfile` at the root. The Dockerfile needs BuildKit (it uses cache mounts and `COPY --chmod`), the default builder since Docker 23 when the buildx plugin is installed (on Debian and Raspberry Pi OS, the `docker-buildx` package). An older engine may build it with `DOCKER_BUILDKIT=1 docker build …`; if that fails on `--chmod`, update Docker. `docker compose` needs the Compose v2 plugin.

Both compose files give aranet-service 30 seconds to stop (`stop_grace_period: 30s`): its own shutdown can take up to 20 seconds while a Bluetooth poll in flight disconnects and the background tasks finish, and Docker's default kills a container after 10.

## Services

| Service | Port | Description |
|---------|------|-------------|
| aranet-service | 8080 | Aranet API, dashboard and collector |
| prometheus | 9090 | Metrics storage |
| grafana | 3000 | Visualization |

Every port is published on `127.0.0.1` only, so the stack is reachable from this machine and not from the network. Prometheus reaches aranet-service over the Compose network.

To reach aranet-service from other machines, change its `ports:` entry from `"127.0.0.1:8080:8080"` to `"8080:8080"`, and turn on API-key authentication first (`api_key_enabled = true`; see "Server Configuration"). The entries of Prometheus and Grafana change the same way, but the API key doesn't protect them: this Prometheus has no login, and Grafana's password is `changeme` until you set `GRAFANA_ADMIN_PASSWORD`.

## Access

- **Aranet dashboard**: <http://127.0.0.1:8080/dashboard> (health check: <http://127.0.0.1:8080/api/health>)
- **Prometheus**: <http://127.0.0.1:9090>
- **Grafana**: <http://127.0.0.1:3000> (user `admin`, password `changeme`; set `GRAFANA_ADMIN_USER` and `GRAFANA_ADMIN_PASSWORD` to change them)

## Configuration

### Server Configuration

The image runs aranet-service as uid 999 and always reads `/app/server.toml`. The image holds a copy of `server.toml` from this directory, and the Compose file mounts that file over it read-only, so your edits take effect after `docker compose -f docker/docker-compose.yml restart aranet-service`. Edit `server.toml` to configure:

- Devices to poll
- Prometheus, MQTT, webhooks and InfluxDB
- Database path (`/data/aranet.db`, in the `aranet-data` volume)

A config that fails validation stops the container, and its logs say why.

The image build copies `docker/server.toml` into the image (as `/app/server.toml`, mode 0644), so an image built after you put an API key, an MQTT password or a webhook URL into that file contains them. Don't push or share such an image. To keep secrets out of the image, leave `docker/server.toml` as it is and mount a config from somewhere else: a file outside the checkout, or the `server.toml` at the repository root described below, which the build leaves out.

Because the mount is read-only, adding, changing or removing a sensor through the API, the TUI or the GUI fails with `500 Internal Server Error`: edit `server.toml` instead. To manage sensors through the API, mount a writable copy that uid 999 owns, without `:ro`:

```bash
cp docker/server.toml server.toml
sudo chown 999:999 server.toml
# docker-compose.yml: - ../server.toml:/app/server.toml
```

The service then rewrites that file in place, keeping its comments. It can't replace a single mounted file atomically, so keep a copy. Its first write also makes the file readable only by its owner, uid 999, so reading or editing it on the host then needs `sudo`.

These settings in `server.toml` keep the service reachable inside Docker without an API key:

```toml
[server]
bind = "0.0.0.0:8080"

[security]
allow_unauthenticated_network = true
allowed_hosts = ["aranet-service"]
```

Inside the container the service has to listen on `0.0.0.0`, which it refuses without API-key authentication unless `allow_unauthenticated_network` is set. That is safe here only because the port is published on `127.0.0.1`. `allowed_hosts` lets Prometheus use the name `aranet-service`. If you publish the port on any other address, turn on API-key authentication instead, and set `api_key` yourself (the service can't save a generated key to the read-only file):

```toml
[security]
api_key_enabled = true
api_key = "replace-with-a-random-string-at-least-32-characters"
```

Then give Prometheus the key in `prometheus.yml`:

```yaml
scrape_configs:
  - job_name: 'aranet'
    authorization:
      type: Bearer
      credentials: 'replace-with-a-random-string-at-least-32-characters'
    static_configs:
      - targets: ['aranet-service:8080']
```

### Bluetooth Access

#### Linux

Bluetooth works on a Linux host running BlueZ. Uncomment the D-Bus mount of `aranet-service` in `docker-compose.yml`:

```yaml
    volumes:
      - aranet-data:/data
      - ./server.toml:/app/server.toml:ro
      - /run/dbus:/run/dbus:ro
```

The service talks to the host's BlueZ over the system bus, so it needs no privileged mode, host networking or device nodes. The mount gives the container the host's whole system bus, with the rights of uid 999 on the host: BlueZ (pairing and removing devices, switching the adapter off) and every other service on the bus that admits that uid. `:ro` only keeps the container from changing the directory: it can still send requests through the socket in it.

If the log shows `AccessDenied` for `org.bluez` or the system bus:

- BlueZ may accept only the host's `bluetooth` group: add its id (`getent group bluetooth`) with `group_add: ["<gid>"]`.
- AppArmor (Ubuntu) or SELinux (Fedora, RHEL) may block the container's D-Bus messages: add `security_opt: ["apparmor=unconfined"]` (AppArmor) or `security_opt: ["label=disable"]` (SELinux) to `aranet-service`.

#### macOS / Windows

Docker Desktop can't reach the host's Bluetooth adapter. Run aranet-service natively and use Docker only for Prometheus and Grafana:

```bash
# Run only the monitoring stack
docker compose -f docker/docker-compose.yml up -d --no-deps prometheus grafana

# Run aranet-service natively
cargo run -p aranet-service --features full
```

`--no-deps` matters: without it, Compose also builds and starts the `aranet-service` container, because Prometheus depends on it, and that container takes port 8080 on `127.0.0.1`, which the native service needs. Then point Prometheus at the native service (see "External aranet-service" below).

## Prometheus Scraping

The default configuration scrapes aranet-service every 60 seconds. To customize:

1. Edit `prometheus.yml`
2. Restart Prometheus: `docker compose -f docker/docker-compose.yml restart prometheus`

### External aranet-service

If aranet-service runs outside Docker:

```yaml
# In prometheus.yml
scrape_configs:
  - job_name: 'aranet'
    static_configs:
      - targets: ['host.docker.internal:8080']
```

Prometheus then sends `Host: host.docker.internal:8080`. If that service runs without an API key, allow the name in its `server.toml`:

```toml
[security]
allowed_hosts = ["host.docker.internal"]
```

## Grafana Dashboards

The pre-built dashboard (`grafana/aranet-dashboard.json` at the repository root) is loaded automatically. Features:

- **Current Readings**: CO2, temperature, humidity, battery gauges
- **Historical Data**: Time series graphs with thresholds
- **Collector Status**: Health and polling statistics

### Custom Dashboards

1. Create a dashboard in the Grafana UI
2. Export its JSON from the dashboard settings
3. Save it in the `grafana/` directory at the repository root, which is mounted at `/var/lib/grafana/dashboards`
4. Restart Grafana: `docker compose -f docker/docker-compose.yml restart grafana`

## Data Persistence

Data is stored in Docker volumes:

- `aranet-data`: SQLite database
- `aranet-prometheus-data`: Prometheus TSDB
- `aranet-grafana-data`: Grafana settings and custom dashboards

These are Docker's names for them, as `docker volume ls` shows them. (In `docker/docker-compose.yml` the last two have the keys `prometheus-data` and `grafana-data`.) The `docker-compose.yml` at the repository root gives its volume no name of its own, so Docker calls it `<project>_aranet-data`, where `<project>` is the Compose project, by default the name of the checkout's directory.

To back up:

```bash
docker run --rm -v aranet-data:/data -v $(pwd):/backup alpine tar czf /backup/aranet-data.tar.gz /data
```

Use the volume's name from `docker volume ls` in that command: for a name that doesn't exist, Docker creates a new, empty volume and archives that.

## Troubleshooting

### No metrics in Prometheus

1. Check aranet-service is running: `docker compose -f docker/docker-compose.yml logs aranet-service`
2. Check that `server.toml` has `enabled = true` under `[prometheus]`
3. Check Prometheus targets: <http://127.0.0.1:9090/targets>. A `421` error means the target's host name isn't allowed: add it to `allowed_hosts` under `[security]` in `server.toml`. A `401` means authentication is on: give Prometheus the key (see "Server Configuration")

### The container keeps restarting

The service exits when its config is invalid; `docker compose -f docker/docker-compose.yml logs aranet-service` shows the error. Listening on `0.0.0.0` without an API key needs `allow_unauthenticated_network = true`, as in the provided `server.toml`, and `bind` must be an IP address and port such as `0.0.0.0:8080`.

If the log shows `error: the argument '--config <CONFIG>' cannot be used multiple times`, remove `--config` from your own `command:` or `docker run` arguments: the image passes `--config /app/server.toml` itself. To use another file, mount it over `/app/server.toml`.

### Connection refused

1. Inside the container the service must listen on `0.0.0.0:8080` (`bind` in `server.toml`): on `127.0.0.1`, neither Prometheus nor the published port can reach it
2. Check the Docker network: `docker network inspect aranet-network`

### Bluetooth not working

See the "Bluetooth Access" section above. Docker containers have limited hardware access.
