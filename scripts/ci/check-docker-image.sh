#!/usr/bin/env bash
# Build the aranet-service image from docker/Dockerfile and check that
#   - it serves /metrics out of the box to the Host header Prometheus sends,
#     because it reads its baked /app/server.toml,
#   - the service runs as uid:gid 999:999 and keeps its database in /data,
#   - `--no-collector` after the image name keeps the config, and a file
#     mounted over /app/server.toml replaces the baked one,
# and that both compose files are valid.
#
# Needs Docker with BuildKit (the default builder since Docker 23), the compose
# plugin, curl, and network access for the base images and crates. Docker picks
# the host ports, on 127.0.0.1, so the checks never collide with a running
# service.
#
# Usage: scripts/ci/check-docker-image.sh
set -euo pipefail

cd "$(dirname "$0")/../.."

image=aranet-service:ci
containers=(aranet-ci aranet-ci-mounted)
workdir="$(mktemp -d)"

cleanup() {
  local status=$?
  if [ "$status" -ne 0 ]; then
    for container in "${containers[@]}"; do
      if docker container inspect "$container" > /dev/null 2>&1; then
        echo "--- docker logs $container ---" >&2
        docker logs "$container" >&2 || true
      fi
    done
  fi
  docker rm -f "${containers[@]}" > /dev/null 2>&1 || true
  rm -rf "$workdir"
  exit "$status"
}
trap cleanup EXIT

fail() {
  echo "FAILED: $*" >&2
  exit 1
}

# The host address (127.0.0.1:<port>) that Docker published port 8080 of
# container $1 on.
published() {
  docker port "$1" 8080/tcp | head -n 1
}

# Wait for container $1 to answer GET http://$2/api/health: 60 tries, a second apart, each limited to 2 s.
wait_healthy() {
  local container=$1 addr=$2
  for _ in $(seq 60); do
    if curl -fsS --max-time 2 -o /dev/null "http://$addr/api/health" 2> /dev/null; then
      echo "ok: $container answers /api/health on $addr"
      return 0
    fi
    if [ "$(docker container inspect -f '{{.State.Running}}' "$container")" != true ]; then
      fail "$container exited before answering /api/health"
    fi
    sleep 1
  done
  fail "$container did not answer /api/health on $addr after 60 tries"
}

docker rm -f "${containers[@]}" > /dev/null 2>&1 || true

echo "== Build $image"
docker build -f docker/Dockerfile -t "$image" .

echo "== Baked config"
docker run -d --name aranet-ci -p 127.0.0.1::8080 "$image" > /dev/null
addr="$(published aranet-ci)"
wait_healthy aranet-ci "$addr"
# docker/prometheus.yml scrapes the target aranet-service:8080. The output is
# captured first: `curl | grep -q` can fail under pipefail when grep exits early.
metrics="$(curl -fsS --max-time 10 -H 'Host: aranet-service:8080' "http://$addr/metrics")" \
  || fail "GET /metrics with Host: aranet-service:8080"
grep -q '^aranet_collector_running ' <<< "$metrics" \
  || fail "GET /metrics has no aranet_collector_running line"
echo "ok: GET /metrics answers Prometheus's Host with the collector metrics"

echo "== User and data directory"
# The owner of PID 1, the service itself (`docker exec … id -u` would only
# restate USER).
owner="$(docker exec aranet-ci stat -c %u:%g /proc/1)"
[ "$owner" = 999:999 ] || fail "the service runs as uid:gid $owner, expected 999:999"
echo "ok: the service runs as uid:gid 999:999"
docker exec aranet-ci test -f /data/aranet.db || fail "no database at /data/aranet.db"
echo "ok: the database is /data/aranet.db"

echo "== Mounted config and --no-collector"
cat > "$workdir/server.toml" << 'TOML'
[server]
bind = "0.0.0.0:8080"

[storage]
path = "/data/aranet.db"

[security]
allowed_hosts = ["aranet-service"]
allow_unauthenticated_network = true

[[devices]]
address = "Aranet4 CI000"
alias = "ci-sensor"
TOML
chmod 644 "$workdir/server.toml" # the service runs as uid 999
docker run -d --name aranet-ci-mounted -p 127.0.0.1::8080 \
  -v "$workdir/server.toml:/app/server.toml:ro" "$image" --no-collector > /dev/null
addr="$(published aranet-ci-mounted)"
wait_healthy aranet-ci-mounted "$addr"
config="$(curl -fsS --max-time 10 -H 'Host: 127.0.0.1' "http://$addr/api/config")" \
  || fail "GET /api/config"
grep -qF '"address":"Aranet4 CI000"' <<< "$config" \
  || fail "GET /api/config does not list the mounted config's device: $config"
echo "ok: --no-collector serves the mounted config's device"

echo "== Compose files"
for compose in docker/docker-compose.yml docker-compose.yml; do
  docker compose -f "$compose" config -q
  echo "ok: $compose is valid"
done

echo "ok: every Docker image check passed"
