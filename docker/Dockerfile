# aranet-service container image
#
# Build from the repository root (BuildKit, the default builder since Docker 23):
#   docker build -f docker/Dockerfile -t aranet-service .
# Run without Bluetooth (API, dashboard and integrations only):
#   docker run -d --name aranet -p 127.0.0.1:8080:8080 -v aranet-data:/data aranet-service --no-collector
#
# The service always reads /app/server.toml, a copy of docker/server.toml. To use
# another config, mount it over that path (-v "$PWD/server.toml:/app/server.toml:ro").
# `--config` after the image name fails, because ENTRYPOINT already passes it.
# Arguments after the image name replace only CMD (`run`), so `--no-collector`
# keeps the config.
#
# Bluetooth needs a Linux host with BlueZ and the host's /run/dbus mounted
# read-only (-v /run/dbus:/run/dbus:ro); see docker-compose.yml at the
# repository root. No privileged mode, host network or devices are needed.

FROM rust:1.90-slim-bookworm AS builder

# libssl-dev: the MQTT client in the `full` feature set uses native-tls (OpenSSL).
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libdbus-1-dev libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# The whole workspace: Cargo loads every member's manifest and targets, even to
# build one package. .dockerignore keeps target directories and docs out.
COPY . .

# The cache mounts keep downloaded crates and compiled dependencies between
# builds. They are not part of the image, so the binary is copied out of the
# target directory in the same step.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked -p aranet-service --features full \
    && cp target/release/aranet-service /usr/local/bin/aranet-service

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends libdbus-1-3 libssl3 ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

# uid and gid 999 are what `useradd -r` gave earlier images, so existing
# aranet-data volumes stay writable. The home directory is the data volume.
RUN groupadd --system --gid 999 aranet \
    && useradd --system --uid 999 --gid 999 --no-create-home --home-dir /data \
        --shell /usr/sbin/nologin aranet \
    && mkdir -p /data \
    && chown aranet:aranet /data

WORKDIR /app

COPY --from=builder /usr/local/bin/aranet-service /app/aranet-service
# 0644 whatever the mode in the checkout: the service (uid 999) must read it.
COPY --chmod=0644 docker/server.toml /app/server.toml

USER aranet

EXPOSE 8080

ENV RUST_LOG=aranet_service=info

# 127.0.0.1 because the service answers an IP address in Host without a
# security.allowed_hosts entry. Assumes the port in docker/server.toml.
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8080/api/health || exit 1

ENTRYPOINT ["/app/aranet-service", "--config", "/app/server.toml"]
CMD ["run"]
