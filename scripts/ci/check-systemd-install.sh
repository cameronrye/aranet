#!/usr/bin/env bash
# Check `aranet-service service install` against a real systemd:
#
# - `service status` reports a service that isn't installed.
# - The unit it writes is distribution/systemd/aranet.service (with this
#   machine's program path), passes `systemd-analyze verify` and scores at
#   most 5.0 in `systemd-analyze security`.
# - It replaces a running rye-aranet.service, the unit 0.2.1 installed, and
#   `service status` warns about that unit until then.
# - The service answers /api/health, runs as the dynamic user `aranet` and
#   creates its database in /var/lib/aranet.
# - A copy of the binary in the invoking user's home directory is refused.
# - `service uninstall` removes the unit.
#
# Usage, from the repository root:
#   sudo scripts/ci/check-systemd-install.sh target/debug/aranet-service
#
# It installs /usr/bin/aranet-service and systemd units, so run it only on a
# throwaway machine: a CI runner or a container that boots systemd. Needs curl,
# and libnss-systemd so that `ps` can name the dynamic user.
set -euo pipefail

fail() {
  echo "::error::$*" >&2
  exit 1
}

[ "$#" -eq 1 ] || fail "usage: sudo $0 <aranet-service binary>"
[ "$(id -u)" -eq 0 ] || fail "run it as root (sudo)"
[ -d /run/systemd/system ] || fail "systemd is not the init system"
[ -f distribution/systemd/aranet.service ] || fail "run it from the repository root"

built=$1
# Not /usr/local/bin: GitHub's Ubuntu images make it world-writable, so the
# install would rightly refuse a program there.
program=/usr/bin/aranet-service
unit=/etc/systemd/system/aranet.service
legacy_unit=/etc/systemd/system/rye-aranet.service
health=http://127.0.0.1:8080/api/health
invoking_user=${SUDO_USER:-root}
user_home=$(getent passwd "$invoking_user" | cut -d: -f6)
home_copy="$user_home/aranet-service"
work=$(mktemp -d)

on_exit() {
  local status=$?
  if [ "$status" -ne 0 ]; then
    systemctl status --no-pager aranet.service rye-aranet.service || true
    journalctl --no-pager -n 100 -u aranet.service || true
  fi
  rm -rf "$work" "$home_copy"
  exit "$status"
}
trap on_exit EXIT

echo "== Install the binary where only root can change it"
install -o root -g root -m 0755 "$built" "$program"

echo "== Nothing is installed yet"
"$program" service status | tee "$work/status.out"
grep -qx 'aranet-service is not installed' "$work/status.out" ||
  fail "service status doesn't report the missing service"

echo "== systemd-analyze verify the shipped unit"
# verify checks that the ExecStart= program exists, and the shipped unit runs
# /usr/local/bin/aranet-service, so check it with this machine's path.
sed "s#^ExecStart=/usr/local/bin/aranet-service #ExecStart=$program #" \
  distribution/systemd/aranet.service > "$work/aranet.service"
grep -qx "ExecStart=$program run --config /var/lib/aranet/server.toml" "$work/aranet.service" ||
  fail "distribution/systemd/aranet.service has an unexpected ExecStart="
if ! verify_log=$(systemd-analyze verify "$work/aranet.service" 2>&1); then
  echo "$verify_log"
  fail "systemd-analyze verify failed"
fi
if grep -F aranet.service <<< "$verify_log"; then
  fail "systemd-analyze verify warned about aranet.service"
fi

echo "== A running rye-aranet.service from 0.2.1"
printf '%s\n' '[Unit]' 'Description=Stand-in for the unit aranet-service 0.2.1 installed' '' \
  '[Service]' 'ExecStart=/bin/sleep infinity' '' '[Install]' 'WantedBy=multi-user.target' \
  > "$legacy_unit"
systemctl daemon-reload
systemctl enable --now rye-aranet.service
"$program" service status 2> "$work/status.err"
cat "$work/status.err"
grep -qF 'Warning: rye-aranet.service' "$work/status.err" ||
  fail "service status doesn't warn about rye-aranet.service"

echo "== service install"
"$program" service install | tee "$work/install.out"
grep -qF 'sudo systemctl start aranet' "$work/install.out" ||
  fail "service install printed no notes"
cmp "$work/aranet.service" "$unit" || fail "$unit differs from distribution/systemd/aranet.service"
systemctl is-enabled --quiet aranet.service || fail "aranet.service is not enabled"
[ ! -e "$legacy_unit" ] || fail "$legacy_unit is still there"
if systemctl is-active --quiet rye-aranet.service; then
  fail "rye-aranet.service is still running"
fi

echo "== systemd-analyze security"
# --threshold uses the 0-100 scale behind the 0.0-10.0 exposure it prints: 50 is 5.0.
systemd-analyze security --offline=true --threshold=50 --no-pager "$unit" | tail -n 1

echo "== The service runs as the dynamic user aranet"
systemctl start aranet.service
for _ in $(seq 1 30); do
  if curl -fsS "$health" > /dev/null 2>&1; then
    break
  fi
  sleep 1
done
curl -fsS "$health" > /dev/null || fail "$health did not answer within 30 s"
pid=$(systemctl show -p MainPID --value aranet.service)
[ "$pid" -gt 0 ] || fail "aranet.service has no main process"
user=$(ps -o user= -p "$pid" | tr -d ' ')
echo "aranet.service runs as $user, uid $(ps -o uid= -p "$pid" | tr -d ' ')"
[ "$user" = aranet ] || fail "aranet.service runs as $user, not as aranet"
[ -f /var/lib/private/aranet/data.db ] || fail "/var/lib/private/aranet/data.db was not created"

echo "== A copy in $user_home is refused"
install -o "$invoking_user" -m 0755 "$built" "$home_copy"
if "$home_copy" service install 2> "$work/refusal.err"; then
  fail "service install accepted $home_copy"
fi
cat "$work/refusal.err"
grep -qF "sudo install -o root -g root -m 0755 $home_copy /usr/local/bin/aranet-service" \
  "$work/refusal.err" || fail "the refusal doesn't say how to copy the binary"
cmp "$work/aranet.service" "$unit" || fail "the refused install changed $unit"
systemctl is-active --quiet aranet.service || fail "the refused install stopped aranet.service"

echo "== service uninstall"
"$program" service stop
"$program" service uninstall
[ ! -e "$unit" ] || fail "$unit is still there"
if curl -fsS "$health" > /dev/null 2>&1; then
  fail "the service still answers after uninstall"
fi

echo "All systemd install checks passed"
