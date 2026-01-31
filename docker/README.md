# Aranet Docker Setup

This directory contains Docker configuration for running aranet-service with Prometheus and Grafana.

## Quick Start

```bash
# From the repository root
cd docker

# Start all services
docker-compose up -d

# View logs
docker-compose logs -f aranet-service
```

## Services

| Service | Port | Description |
|---------|------|-------------|
| aranet-service | 8080 | Aranet API and collector |
| prometheus | 9090 | Metrics storage |
| grafana | 3000 | Visualization (admin/admin) |

## Access

- **Aranet API**: http://localhost:8080/api/health
- **Prometheus**: http://localhost:9090
- **Grafana**: http://localhost:3000 (login: admin/admin)

## Configuration

### Server Configuration

Edit `server.toml` to configure:
- Bind address and port
- Database path
- MQTT settings
- Device list

### Bluetooth Access

Bluetooth Low Energy (BLE) is not available in Docker containers by default. To enable BLE:

#### Linux

```yaml
# In docker-compose.yml, add to aranet-service:
privileged: true
volumes:
  - /var/run/dbus:/var/run/dbus
network_mode: host
```

#### macOS / Windows

BLE passthrough is not supported. Run aranet-service natively and use Docker only for Prometheus/Grafana:

```bash
# Run only monitoring stack
docker-compose up -d prometheus grafana

# Run aranet-service natively
cargo run -p aranet-service --features full
```

## Prometheus Scraping

The default configuration scrapes aranet-service every 60 seconds. To customize:

1. Edit `prometheus.yml`
2. Restart prometheus: `docker-compose restart prometheus`

### External aranet-service

If running aranet-service outside Docker:

```yaml
# In prometheus.yml
scrape_configs:
  - job_name: 'aranet'
    static_configs:
      - targets: ['host.docker.internal:8080']
```

## Grafana Dashboards

The pre-built dashboard is automatically loaded. Features:

- **Current Readings**: CO2, temperature, humidity, battery gauges
- **Historical Data**: Time series graphs with thresholds
- **Collector Status**: Health and polling statistics

### Custom Dashboards

1. Create dashboard in Grafana UI
2. Export JSON from dashboard settings
3. Save to `../dashboards/` directory
4. Restart Grafana: `docker-compose restart grafana`

## Data Persistence

Data is stored in Docker volumes:
- `aranet-data`: SQLite database
- `prometheus-data`: Prometheus TSDB
- `grafana-data`: Grafana settings and custom dashboards

To back up:

```bash
docker run --rm -v aranet-data:/data -v $(pwd):/backup alpine tar czf /backup/aranet-data.tar.gz /data
```

## Troubleshooting

### No metrics in Prometheus

1. Check aranet-service is running: `docker-compose logs aranet-service`
2. Verify Prometheus config is enabled in server.toml
3. Check Prometheus targets: http://localhost:9090/targets

### Connection refused

1. Ensure service is bound to `0.0.0.0` (not `127.0.0.1`) in server.toml
2. Check Docker network: `docker network inspect aranet-network`

### Bluetooth not working

See the "Bluetooth Access" section above. Docker containers have limited hardware access.
