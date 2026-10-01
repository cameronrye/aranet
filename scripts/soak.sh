#!/usr/bin/env bash
#
# Soak test for aranet's Bluetooth connection lifecycles and for aranet-service.
#
# Runs the lifecycle_soak example (a DeviceManager and a ReconnectingDevice
# whose links are cut on purpose) and/or aranet-service for hours, samples
# their threads, file descriptors and memory, and prints PASS or FAIL against
# the criteria (a)-(f) that --help lists.
#
# Usage:
#   ./scripts/soak.sh --target lifecycle --duration 1h --lifecycle-devices "Aranet2 2751B,AranetRn+ 306B8"
#   ./scripts/soak.sh --target service --duration 24h --service-device "AranetRn+ 306B8"
#   ./scripts/soak.sh --target service --duration 24h --service-device <MAC> --service-bin ./aranet-service
#                                       # a prebuilt binary: no cargo needed
#   ./scripts/soak.sh --evaluate DIR    # apply the criteria again to a finished run
#   ./scripts/soak.sh --help
#
# Works with the bash 3.2 that macOS ships.
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Fixed settings
SERVICE_BIND="127.0.0.1:18099"
# HEALTH_SECS, REPAIR_BUDGET_SECS and validate_options' --fault-every and
# --duration checks repeat crates/aranet-core/examples/lifecycle_soak.rs (its
# REPAIR_BUDGET and parse_args), which is their source: change both together.
HEALTH_SECS=10                  # --health-secs for lifecycle_soak
REPAIR_BUDGET_SECS=70           # lifecycle_soak's REPAIR_BUDGET
THREAD_TOLERANCE=2              # (a)
FD_TOLERANCE=4                  # (b)
RSS_GROWTH_LIMIT_PCT=20         # (c)
STREAK_LIMIT_SECS=45            # (f), Linux only
PROBE_EVERY_SECS=7              # BlueZ Connected probe period
# (f): how long a correct aranet-service may take to exit after SIGINT. Its
# collector waits up to 10 s for a poll in progress, then up to 2 s for its
# config reload watcher, and the SIGINT usually lands early in a poll (the
# samples and the polls share the 60 s phase). The rest is exit time and
# stop_secs counting whole seconds. A collector that stopped waiting for its
# poll would exit within a few seconds.
STOP_LIMIT_SECS=15
STOP_WAIT_SECS=30               # SIGKILL aranet-service this long after SIGINT
LIFECYCLE_KILL_SECS=60          # SIGKILL lifecycle_soak this long after SIGINT
START_LIMIT_SECS=600            # lifecycle_soak's scan, and up to three connects per sensor
LIFECYCLE_STOP_SECS=150         # its shutdown: three 30 s steps, 5 s settle, final sample
SERVICE_LOG_FILTER="aranet_core=info,aranet_service=info"
LIFECYCLE_LOG_FILTER="aranet_core=info,lifecycle_soak=info"

# Options
TARGET="both"
DURATION="24h"
INTERVAL="60"
WARMUP="10m"
FAULT_EVERY="15m"
OUT=""
SERVICE_DEVICE=""
LIFECYCLE_DEVICES=""
EVALUATE_DIR=""
SERVICE_BIN=""                  # --service-bin, or build_binaries' release build
LIFECYCLE_BIN=""                # --lifecycle-bin, or build_binaries' release build

# State
OS_NAME="$(uname -s)"
DURATION_S=0
WARMUP_S=0
FAULT_EVERY_S=0
M_DEVICE=""
R_DEVICE=""
PROBE_NOTE=""
WORK_DIR=""
RUN_START=0
SERVICE_PID=""
LIFECYCLE_PID=""
LIFECYCLE_T0=0
PROBE_PID=""
FAILED=0

if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then
    RED=$'\033[0;31m'
    GREEN=$'\033[0;32m'
    YELLOW=$'\033[1;33m'
    BLUE=$'\033[0;34m'
    BOLD=$'\033[1m'
    NC=$'\033[0m'
else
    RED="" GREEN="" YELLOW="" BLUE="" BOLD="" NC=""
fi

# ==============================================================================
# Utility Functions
# ==============================================================================

print_header() {
    printf '\n%s%s== %s ==%s\n' "$BOLD" "$BLUE" "$1" "$NC"
}

print_info() {
    printf '  %s\n' "$1"
}

print_warning() {
    printf '  %sWarning:%s %s\n' "$YELLOW" "$NC" "$1" >&2
}

# die MESSAGE: prints MESSAGE and exits 2, the setup-error status. Every setup
# step fails through it, so that a setup error never exits with the failing
# command's own status (1 would read as FAIL).
die() {
    printf '%sError:%s %s\n' "$RED" "$NC" "$1" >&2
    exit 2
}

is_running() {
    kill -0 "$1" 2>/dev/null
}

wants() {
    [[ $TARGET == both || $TARGET == "$1" ]]
}

# needs_build service|lifecycle: true if --target runs that program and no
# prebuilt binary was given for it.
needs_build() {
    case $1 in
        service) wants service && [[ -z $SERVICE_BIN ]] ;;
        lifecycle) wants lifecycle && [[ -z $LIFECYCLE_BIN ]] ;;
        *) return 1 ;;
    esac
}

# abs_bin OPTION PATH: prints PATH as an absolute path, or fails if it isn't
# an executable file.
abs_bin() {
    if [[ ! -f $2 || ! -x $2 ]]; then
        printf 'Error: %s %s is not an executable file\n' "$1" "$2" >&2
        return 1
    fi
    printf '%s/%s\n' "$(cd "$(dirname "$2")" && pwd)" "$(basename "$2")"
}

lower() {
    printf '%s' "$1" | tr '[:upper:]' '[:lower:]'
}

# trim TEXT: prints TEXT without its leading and trailing whitespace.
trim() {
    local text=$1
    text=${text#"${text%%[![:space:]]*}"}
    text=${text%"${text##*[![:space:]]}"}
    printf '%s' "$text"
}

# to_secs NAME VALUE: prints VALUE (90, 90s, 15m or 24h) in seconds.
to_secs() {
    if [[ ! $2 =~ ^([0-9]+)([smh]?)$ ]]; then
        printf 'Error: %s must be a whole number with an optional s, m or h suffix, not "%s"\n' "$1" "$2" >&2
        return 1
    fi
    local n=$((10#${BASH_REMATCH[1]}))
    case ${BASH_REMATCH[2]} in
        h) echo $((n * 3600)) ;;
        m) echo $((n * 60)) ;;
        *) echo "$n" ;;
    esac
}

# wait_for_exit PID SECONDS: true once PID has exited, false if it still runs.
wait_for_exit() {
    local pid=$1 ticks=$(($2 * 5))
    while ((ticks > 0)); do
        if ! is_running "$pid"; then
            return 0
        fi
        sleep 0.2
        ticks=$((ticks - 1))
    done
    ! is_running "$pid"
}

# shellcheck disable=SC2329 # run by the EXIT trap
cleanup() {
    local status=$? pid wait_secs
    trap - EXIT INT TERM
    set +e
    if [[ -n $PROBE_PID ]]; then
        kill -TERM "$PROBE_PID" 2>/dev/null
    fi
    # Each program gets as long as stop_all gives it. lifecycle_soak's
    # shutdown (stop the monitor, disconnect both sensors, wait 5 s, check
    # them) takes longer when a disconnect waits out its limit, and a SIGKILL
    # before it ends loses its last lines and can leave a sensor connected.
    for pid in "$LIFECYCLE_PID" "$SERVICE_PID"; do
        if [[ -n $pid ]] && is_running "$pid"; then
            wait_secs=$STOP_WAIT_SECS
            if [[ $pid == "$LIFECYCLE_PID" ]]; then
                wait_secs=$LIFECYCLE_KILL_SECS
            fi
            kill -INT "$pid" 2>/dev/null
            wait_for_exit "$pid" "$wait_secs" || kill -KILL "$pid" 2>/dev/null
        fi
    done
    if [[ -n $WORK_DIR ]]; then
        rm -rf "$WORK_DIR"
    fi
    exit "$status"
}

# ==============================================================================
# Options
# ==============================================================================

show_help() {
    cat <<EOF
Aranet soak test

Usage: $0 [OPTIONS]
       $0 --evaluate DIR

Runs aranet-service and/or the lifecycle_soak example (a DeviceManager and a
ReconnectingDevice whose links are cut on purpose) for a long time, samples
their threads, file descriptors and memory every --interval seconds, and
prints PASS or FAIL. Exits 0 on PASS, 1 on FAIL and 2 on a setup error. The
CSV, the logs and the summary stay in --out.

WARNING: for the whole run the sensors stay connected (lifecycle) or are
polled every minute (service). That drains their batteries faster and blocks
the phone app. Run nothing else against these sensors: another connection
makes the run fail. On Linux the orphan checks read BlueZ's global state, so
a connection from any other program counts too.

On macOS, run it under 'caffeinate -i' so that the Mac doesn't sleep. Ctrl-C
stops a run early (use kill -TERM when it runs in the background, where bash
ignores SIGINT); the samples taken so far stay in --out.

Options:
  --target service|lifecycle|both   What to run (default: both)
  --duration DURATION               Length of the run (default: 24h)
  --interval SECONDS                Sampling interval, at least 5 (default: 60)
  --warmup DURATION                 Samples before this aren't judged (default: 10m)
  --fault-every DURATION            Link cut interval for lifecycle_soak, 0 for
                                    none (default: 15m)
  --out DIR                         Output directory (default: target/soak/<time>)
  --service-device NAME_OR_MAC      Sensor that aranet-service polls every 60 s;
                                    give a MAC address on Linux for check (f)'s
                                    BlueZ probe
  --lifecycle-devices M[,R]         Sensor for the DeviceManager, and optionally
                                    a second one for the ReconnectingDevice
  --service-bin PATH                Run this aranet-service binary instead of
                                    building one with cargo
  --lifecycle-bin PATH              Run this lifecycle_soak binary instead of
                                    building one with cargo
  --evaluate DIR                    Apply the criteria again to a finished run
  --help                            Show this help

DURATION is a whole number with an s, m or h suffix (none means seconds).
Without --service-bin and --lifecycle-bin, the script first builds release
binaries of this checkout with cargo. Given a prebuilt binary for each
program that --target runs (for example, ones built for Linux in Docker and
copied to a host without Rust), it needs no cargo; jq is always needed.
With --target both, the service and lifecycle_soak must use different sensors.
lifecycle_soak needs --fault-every to be 0 or at least the larger of
$((2 * HEALTH_SECS + REPAIR_BUDGET_SECS)) s and two intervals, and --duration to be at least --fault-every
plus $((2 * HEALTH_SECS + REPAIR_BUDGET_SECS)) s plus two intervals, so that every link cut can be judged.

Pass criteria (samples before --warmup aren't judged):
  (a) threads: max - value at warm-up <= $THREAD_TOLERANCE, and end <= value at warm-up + $THREAD_TOLERANCE
  (b) file descriptors: the same, with a tolerance of $FD_TOLERANCE
  (c) RSS: growth after warm-up is reported; FAIL only if it is above
      $RSS_GROWTH_LIMIT_PCT% and never decreased between two samples
  (d) lifecycle: lifecycle_soak finished (exit 0 or 1), no sensor was an
      orphan (connected with no aranet handle) in two samples in a row,
      nothing was still connected after its shutdown, and no shutdown step hung
  (e) lifecycle: every link cut recovered (DeviceManager: ReconnectSucceeded
      within 2 x ${HEALTH_SECS} s + ${REPAIR_BUDGET_SECS} s; ReconnectingDevice: the next read
      succeeded) and the next sample found the new link up and held. A cut
      that finds its sensor already down (a drop it didn't cause) cuts
      nothing, but that drop must recover the same way
  (f) service: at least one successful poll, still running at the end, and
      exited within $STOP_LIMIT_SECS s of SIGINT; the failure rate is reported. On
      Linux, with a MAC address and busctl, BlueZ answered the probe after the
      warm-up and never reported the sensor connected for $STREAK_LIMIT_SECS s or more
      (the collector disconnects after every poll).

Files in --out: samples.csv, summary.txt, run.env, service.log,
service.stop, lifecycle.jsonl, lifecycle.log, lifecycle.exit and, on Linux,
bluez-connected.csv. The logs use RUST_LOG if it is set, and otherwise
$SERVICE_LOG_FILTER (service) and
$LIFECYCLE_LOG_FILTER (lifecycle_soak).

Examples:
  $0 --target lifecycle --duration 1h --fault-every 5m \\
      --lifecycle-devices "Aranet2 2751B,AranetRn+ 306B8" --out ~/soak-lifecycle
  $0 --target service --duration 24h --service-device "AranetRn+ 306B8"
EOF
}

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --target | --duration | --interval | --warmup | --fault-every | --out | \
                --service-device | --lifecycle-devices | --service-bin | --lifecycle-bin | --evaluate)
                if [[ $# -lt 2 ]]; then
                    die "$1 needs a value"
                fi
                case "$1" in
                    --target) TARGET=$2 ;;
                    --duration) DURATION=$2 ;;
                    --interval) INTERVAL=$2 ;;
                    --warmup) WARMUP=$2 ;;
                    --fault-every) FAULT_EVERY=$2 ;;
                    --out) OUT=$2 ;;
                    --service-device) SERVICE_DEVICE=$2 ;;
                    --lifecycle-devices) LIFECYCLE_DEVICES=$2 ;;
                    --service-bin) SERVICE_BIN=$2 ;;
                    --lifecycle-bin) LIFECYCLE_BIN=$2 ;;
                    --evaluate) EVALUATE_DIR=$2 ;;
                esac
                shift 2
                ;;
            --help | -h)
                show_help
                exit 0
                ;;
            *)
                printf '%sError:%s unknown option: %s\n\n' "$RED" "$NC" "$1" >&2
                show_help >&2
                exit 2
                ;;
        esac
    done
}

# The sensors in --lifecycle-devices, trimmed as lifecycle_soak trims them.
split_lifecycle_devices() {
    M_DEVICE=$(trim "${LIFECYCLE_DEVICES%%,*}")
    R_DEVICE=""
    if [[ $LIFECYCLE_DEVICES == *,* ]]; then
        R_DEVICE=$(trim "${LIFECYCLE_DEVICES#*,}")
    fi
}

validate_options() {
    local window shortest needed
    case $TARGET in
        service | lifecycle | both) ;;
        *) die "--target must be service, lifecycle or both, not '$TARGET'" ;;
    esac
    case $OS_NAME in
        Darwin | Linux) ;;
        *) die "the soak runs on macOS and Linux only" ;;
    esac
    DURATION_S=$(to_secs --duration "$DURATION") || exit 2
    WARMUP_S=$(to_secs --warmup "$WARMUP") || exit 2
    FAULT_EVERY_S=$(to_secs --fault-every "$FAULT_EVERY") || exit 2
    if [[ ! $INTERVAL =~ ^[0-9]+$ ]] || ((10#$INTERVAL < 5)); then
        die "--interval must be a whole number of seconds, at least 5"
    fi
    INTERVAL=$((10#$INTERVAL))
    if ((DURATION_S <= WARMUP_S + 2 * INTERVAL)); then
        die "--duration must be longer than --warmup plus two intervals"
    fi
    if wants service; then
        SERVICE_DEVICE=$(trim "$SERVICE_DEVICE")
        if [[ -z $SERVICE_DEVICE ]]; then
            die "--target $TARGET needs --service-device"
        fi
        case $SERVICE_DEVICE in
            *\"* | *\\*) die "--service-device must not contain quotes or backslashes" ;;
        esac
    fi
    if wants lifecycle; then
        if [[ -z $LIFECYCLE_DEVICES ]]; then
            die "--target $TARGET needs --lifecycle-devices"
        fi
        split_lifecycle_devices
        if [[ -z $M_DEVICE || $R_DEVICE == *,* || ($LIFECYCLE_DEVICES == *,* && -z $R_DEVICE) ]]; then
            die "--lifecycle-devices takes one or two sensors: M[,R]"
        fi
        # lifecycle_soak's own rules (parse_args in
        # crates/aranet-core/examples/lifecycle_soak.rs), with its messages,
        # checked before the build.
        if ((FAULT_EVERY_S > 0)); then
            window=$((2 * HEALTH_SECS + REPAIR_BUDGET_SECS))
            shortest=$window
            if ((2 * INTERVAL > shortest)); then
                shortest=$((2 * INTERVAL))
            fi
            if ((FAULT_EVERY_S < shortest)); then
                die "--fault-every must be 0 or at least ${shortest}s, so that each cut can recover before the next"
            fi
            needed=$((FAULT_EVERY_S + window + 2 * INTERVAL))
            if ((DURATION_S < needed)); then
                die "--duration must be at least ${needed}s with --fault-every ${FAULT_EVERY_S}s, so that a cut can be checked"
            fi
        fi
    fi
    if [[ $TARGET == both ]]; then
        # All three names are trimmed by now, as the programs trim them.
        local service manager reconnecting
        service=$(lower "$SERVICE_DEVICE")
        manager=$(lower "$M_DEVICE")
        reconnecting=$(lower "$R_DEVICE")
        if [[ $service == "$manager" || $service == "$reconnecting" ]]; then
            die "the service and lifecycle_soak must use different sensors"
        fi
    fi
    # Prebuilt binaries, as absolute paths: build_binaries may cd.
    if wants service && [[ -n $SERVICE_BIN ]]; then
        SERVICE_BIN=$(abs_bin --service-bin "$SERVICE_BIN") || exit 2
    fi
    if wants lifecycle && [[ -n $LIFECYCLE_BIN ]]; then
        LIFECYCLE_BIN=$(abs_bin --lifecycle-bin "$LIFECYCLE_BIN") || exit 2
    fi
}

check_tools() {
    if needs_build service || needs_build lifecycle; then
        command -v cargo >/dev/null 2>&1 ||
            die "cargo is required to build the binaries (or give prebuilt ones with --service-bin and --lifecycle-bin)"
    fi
    command -v jq >/dev/null 2>&1 || die "jq is required"
    if wants service; then
        command -v curl >/dev/null 2>&1 || die "curl is required for --target $TARGET"
    fi
    if [[ $OS_NAME == Darwin ]]; then
        command -v lsof >/dev/null 2>&1 || die "lsof is required on macOS"
    fi
    [[ -x /bin/ps ]] || die "/bin/ps is required"
}

prepare_out() {
    if [[ -z $OUT ]]; then
        OUT="$PROJECT_ROOT/target/soak/$(date +%Y%m%d-%H%M%S)"
    fi
    mkdir -p "$OUT" || die "can't create --out $OUT"
    OUT=$(cd "$OUT" && pwd) || die "can't use --out $OUT"
    if [[ -e $OUT/samples.csv ]]; then
        die "$OUT already holds a soak run; choose another --out"
    fi
    {
        printf 'TARGET=%s\n' "$TARGET"
        printf 'OS_NAME=%s\n' "$OS_NAME"
        printf 'DURATION_S=%s\n' "$DURATION_S"
        printf 'INTERVAL=%s\n' "$INTERVAL"
        printf 'WARMUP_S=%s\n' "$WARMUP_S"
        printf 'FAULT_EVERY_S=%s\n' "$FAULT_EVERY_S"
        printf 'SERVICE_DEVICE=%s\n' "$SERVICE_DEVICE"
        printf 'LIFECYCLE_DEVICES=%s\n' "$LIFECYCLE_DEVICES"
    } >"$OUT/run.env" || die "can't write $OUT/run.env"
    echo "elapsed_s,target,threads,fds,rss_kb,success,failure,os_connected,orphans" >"$OUT/samples.csv" ||
        die "can't write $OUT/samples.csv"
}

load_run_env() {
    local key value
    [[ -f $OUT/run.env ]] || die "$OUT/run.env not found: is $OUT a soak output directory?"
    while IFS='=' read -r key value; do
        case $key in
            TARGET) TARGET=$value ;;
            OS_NAME) OS_NAME=$value ;;
            DURATION_S) DURATION_S=$value ;;
            INTERVAL) INTERVAL=$value ;;
            WARMUP_S) WARMUP_S=$value ;;
            FAULT_EVERY_S) FAULT_EVERY_S=$value ;;
            SERVICE_DEVICE) SERVICE_DEVICE=$value ;;
            LIFECYCLE_DEVICES) LIFECYCLE_DEVICES=$value ;;
            PROBE_NOTE) PROBE_NOTE=$value ;;
        esac
    done <"$OUT/run.env"
    split_lifecycle_devices
}

# ==============================================================================
# Running
# ==============================================================================

# Builds the release binaries that --service-bin and --lifecycle-bin didn't give.
build_binaries() {
    local target_dir
    if ! needs_build service && ! needs_build lifecycle; then
        print_header "Prebuilt binaries (no build)"
        if wants service; then
            print_info "aranet-service: $SERVICE_BIN"
        fi
        if wants lifecycle; then
            print_info "lifecycle_soak: $LIFECYCLE_BIN"
        fi
        return 0
    fi
    print_header "Building (release)"
    cd "$PROJECT_ROOT" || die "can't enter $PROJECT_ROOT"
    if needs_build service; then
        cargo build --locked --release -p aranet-service || die "building aranet-service failed"
    fi
    if needs_build lifecycle; then
        cargo build --locked --release -p aranet-core --example lifecycle_soak ||
            die "building lifecycle_soak failed"
    fi
    target_dir=$(cargo metadata --locked --format-version 1 --no-deps | jq -r .target_directory) ||
        die "can't find cargo's target directory"
    if needs_build service; then
        SERVICE_BIN="$target_dir/release/aranet-service"
    fi
    if needs_build lifecycle; then
        LIFECYCLE_BIN="$target_dir/release/examples/lifecycle_soak"
    fi
}

start_service() {
    local config="$WORK_DIR/server.toml" waited=0
    if curl -s --max-time 2 "http://$SERVICE_BIND/api/status" >/dev/null 2>&1; then
        die "something already answers on $SERVICE_BIND; stop it first"
    fi
    cat >"$config" <<EOF || die "can't write $config"
[server]
bind = "$SERVICE_BIND"

[storage]
path = "$WORK_DIR/data/data.db"

[[devices]]
address = "$SERVICE_DEVICE"
alias = "soak"
poll_interval = 60
EOF
    print_info "starting aranet-service on $SERVICE_BIND (log: $OUT/service.log)"
    NO_COLOR=1 RUST_LOG="${RUST_LOG:-$SERVICE_LOG_FILTER}" \
        "$SERVICE_BIN" --config "$config" run >"$OUT/service.log" 2>&1 &
    SERVICE_PID=$!
    until curl -s --max-time 2 "http://$SERVICE_BIND/api/status" >/dev/null 2>&1; do
        if ! is_running "$SERVICE_PID"; then
            die "aranet-service stopped during start-up; see $OUT/service.log"
        fi
        if ((waited >= 30)); then
            die "aranet-service didn't answer on $SERVICE_BIND within 30 s; see $OUT/service.log"
        fi
        sleep 1
        waited=$((waited + 1))
    done
}

# Records BlueZ's Device1.Connected for the service's sensor every few seconds,
# for check (f). Linux only, and only for a MAC address.
start_bluez_probe() {
    if [[ $OS_NAME != Linux ]]; then
        PROBE_NOTE="Linux only"
    elif [[ ! $SERVICE_DEVICE =~ ^([0-9A-Fa-f]{2}:){5}[0-9A-Fa-f]{2}$ ]]; then
        PROBE_NOTE="--service-device is not a MAC address"
    elif ! command -v busctl >/dev/null 2>&1; then
        PROBE_NOTE="busctl is not installed"
    fi
    printf 'PROBE_NOTE=%s\n' "$PROBE_NOTE" >>"$OUT/run.env" || die "can't write $OUT/run.env"
    if [[ -n $PROBE_NOTE ]]; then
        return 0
    fi
    local mac path
    mac=$(printf '%s' "$SERVICE_DEVICE" | tr '[:lower:]' '[:upper:]')
    path="/org/bluez/hci0/dev_${mac//:/_}"
    echo "elapsed_s,connected" >"$OUT/bluez-connected.csv" || die "can't write $OUT/bluez-connected.csv"
    (
        while is_running "$SERVICE_PID"; do
            value=$(busctl get-property org.bluez "$path" org.bluez.Device1 Connected 2>/dev/null || true)
            case $value in
                "b true") connected=1 ;;
                "b false") connected=0 ;;
                *) connected="" ;;
            esac
            printf '%s,%s\n' "$((SECONDS - RUN_START))" "$connected" >>"$OUT/bluez-connected.csv"
            sleep "$PROBE_EVERY_SECS"
        done
    ) &
    PROBE_PID=$!
}

start_lifecycle() {
    local args=(--manager-device "$M_DEVICE" --duration "$DURATION_S" --sample-secs "$INTERVAL"
        --fault-every "$FAULT_EVERY_S" --health-secs "$HEALTH_SECS")
    local waited=0
    if [[ -n $R_DEVICE ]]; then
        args+=(--reconnecting-device "$R_DEVICE")
    fi
    print_info "starting lifecycle_soak (log: $OUT/lifecycle.log)"
    NO_COLOR=1 RUST_LOG="${RUST_LOG:-$LIFECYCLE_LOG_FILTER}" "$LIFECYCLE_BIN" "${args[@]}" \
        >"$OUT/lifecycle.jsonl" 2>"$OUT/lifecycle.log" &
    LIFECYCLE_PID=$!
    until grep -q '"kind":"start"' "$OUT/lifecycle.jsonl" 2>/dev/null; do
        if ! is_running "$LIFECYCLE_PID"; then
            tail -n 5 "$OUT/lifecycle.log" >&2 || true
            die "lifecycle_soak stopped during start-up; see $OUT/lifecycle.log"
        fi
        if ((waited >= START_LIMIT_SECS)); then
            die "lifecycle_soak didn't connect within $START_LIMIT_SECS s; see $OUT/lifecycle.log"
        fi
        sleep 1
        waited=$((waited + 1))
    done
    # lifecycle_soak starts its --duration clock when it prints the start line.
    LIFECYCLE_T0=$SECONDS
}

count_threads() {
    if [[ $OS_NAME == Darwin ]]; then
        { /bin/ps -M -p "$1" 2>/dev/null || true; } | tail -n +2 | wc -l | tr -d ' '
    else
        { find "/proc/$1/task" -mindepth 1 -maxdepth 1 2>/dev/null || true; } | wc -l | tr -d ' '
    fi
}

count_fds() {
    if [[ $OS_NAME == Darwin ]]; then
        { lsof -n -P -a -p "$1" -d 0-65535 2>/dev/null || true; } | tail -n +2 | wc -l | tr -d ' '
    else
        { find "/proc/$1/fd" -mindepth 1 -maxdepth 1 2>/dev/null || true; } | wc -l | tr -d ' '
    fi
}

# sample_target TARGET PID ELAPSED: appends one row to samples.csv.
sample_target() {
    local target=$1 pid=$2 elapsed=$3
    local threads fds rss fields="" line status
    local success="" failure="" os_connected="" orphans=""
    threads=$(count_threads "$pid")
    fds=$(count_fds "$pid")
    rss=$({ /bin/ps -o rss= -p "$pid" 2>/dev/null || true; } | tr -d ' ')
    if ! is_running "$pid" || [[ $threads == 0 ]]; then
        return 0
    fi
    if [[ $target == service ]]; then
        status=$(curl -s --max-time 5 "http://$SERVICE_BIND/api/status" 2>/dev/null || true)
        fields=$(jq -r '[.devices[0].success_count, .devices[0].failure_count] | @csv' \
            <<<"$status" 2>/dev/null || true)
        IFS=, read -r success failure <<<"$fields"
    else
        line=$({ grep '"kind":"sample"' "$OUT/lifecycle.jsonl" 2>/dev/null || true; } | tail -n 1)
        if [[ -n $line ]]; then
            fields=$(jq -r '[.rd_reads_ok, .rd_reads_err, (.os_connected | length), (.orphans | length)] | @csv' \
                <<<"$line" 2>/dev/null || true)
        fi
        IFS=, read -r success failure os_connected orphans <<<"$fields"
    fi
    printf '%s,%s,%s,%s,%s,%s,%s,%s,%s\n' "$elapsed" "$target" "$threads" "$fds" "$rss" \
        "$success" "$failure" "$os_connected" "$orphans" >>"$OUT/samples.csv"
}

sample_loop() {
    local next=$SECONDS elapsed running wait_secs
    while :; do
        elapsed=$((SECONDS - RUN_START))
        if ((elapsed >= DURATION_S)); then
            break
        fi
        running=0
        if [[ -n $SERVICE_PID ]] && is_running "$SERVICE_PID"; then
            sample_target service "$SERVICE_PID" "$elapsed"
            running=1
        fi
        if [[ -n $LIFECYCLE_PID ]] && is_running "$LIFECYCLE_PID"; then
            sample_target lifecycle "$LIFECYCLE_PID" "$elapsed"
            running=1
        fi
        if ((running == 0)); then
            print_warning "nothing left to sample after ${elapsed}s; see the logs in $OUT"
            break
        fi
        next=$((next + INTERVAL))
        wait_secs=$((next - SECONDS))
        if ((wait_secs > 0)); then
            sleep "$wait_secs"
        fi
    done
}

stop_all() {
    local code=0 alive=0 started stop_secs="" grace
    print_header "Stopping"
    if [[ -n $LIFECYCLE_PID ]]; then
        grace=$((LIFECYCLE_T0 + DURATION_S + LIFECYCLE_STOP_SECS - SECONDS))
        if ((grace < 10)); then
            grace=10
        fi
        print_info "waiting up to ${grace}s for lifecycle_soak to finish and shut down"
        if ! wait_for_exit "$LIFECYCLE_PID" "$grace"; then
            print_warning "lifecycle_soak is still running; sending SIGINT"
            kill -INT "$LIFECYCLE_PID" 2>/dev/null || true
            if ! wait_for_exit "$LIFECYCLE_PID" "$LIFECYCLE_KILL_SECS"; then
                kill -KILL "$LIFECYCLE_PID" 2>/dev/null || true
            fi
        fi
        wait "$LIFECYCLE_PID" 2>/dev/null || code=$?
        echo "$code" >"$OUT/lifecycle.exit"
        LIFECYCLE_PID=""
    fi
    if [[ -n $SERVICE_PID ]]; then
        if is_running "$SERVICE_PID"; then
            alive=1
            started=$SECONDS
            kill -INT "$SERVICE_PID" 2>/dev/null || true
            if wait_for_exit "$SERVICE_PID" "$STOP_WAIT_SECS"; then
                stop_secs=$((SECONDS - started))
            else
                stop_secs="timeout"
                kill -KILL "$SERVICE_PID" 2>/dev/null || true
            fi
        fi
        wait "$SERVICE_PID" 2>/dev/null || true
        printf 'alive_at_end=%s\nstop_secs=%s\n' "$alive" "$stop_secs" >"$OUT/service.stop"
        SERVICE_PID=""
    fi
    if [[ -n $PROBE_PID ]]; then
        kill -TERM "$PROBE_PID" 2>/dev/null || true
        wait "$PROBE_PID" 2>/dev/null || true
        PROBE_PID=""
    fi
}

# ==============================================================================
# Evaluation
# ==============================================================================

# report LABEL STATUS DETAILS: one summary line. STATUS is PASS, FAIL or SKIP.
report() {
    local color=$YELLOW
    case $2 in
        PASS) color=$GREEN ;;
        FAIL)
            color=$RED
            FAILED=1
            ;;
    esac
    printf '  %-15s %s%-4s%s  %s\n' "$1" "$color" "$2" "$NC" "$3"
    printf '  %-15s %-4s  %s\n' "$1" "$2" "$3" >>"$OUT/summary.txt"
}

note() {
    printf '%s\n' "$1"
    printf '%s\n' "$1" >>"$OUT/summary.txt"
}

# note_list LINES: each non-empty line as an indented bullet.
note_list() {
    local line
    while IFS= read -r line; do
        if [[ -n $line ]]; then
            note "                        - $line"
        fi
    done <<<"$1"
}

check_resources() {
    local target=$1 stats detail trend
    local n=0 bt=0 mt=0 et=0 bf=0 mf=0 ef=0 br=0 er=0 mono=0 growth=0
    stats=$(awk -F, -v t="$target" -v w="$WARMUP_S" '
        NR > 1 && $2 == t && $1 + 0 >= w + 0 && $3 != "" && $4 != "" && $5 != "" {
            n++
            if (n == 1) { bt = $3 + 0; bf = $4 + 0; br = $5 + 0; mt = bt; mf = bf; mono = 1 }
            else if ($5 + 0 < pr) { mono = 0 }
            if ($3 + 0 > mt) { mt = $3 + 0 }
            if ($4 + 0 > mf) { mf = $4 + 0 }
            et = $3 + 0; ef = $4 + 0; er = $5 + 0; pr = $5 + 0
        }
        END {
            if (n == 0) { print 0; exit }
            growth = br > 0 ? (er - br) * 100 / br : 0
            printf "%d %d %d %d %d %d %d %d %d %d %.1f\n", n, bt, mt, et, bf, mf, ef, br, er, mono, growth
        }' "$OUT/samples.csv")
    read -r n bt mt et bf mf ef br er mono growth <<<"$stats"
    if ((n == 0)); then
        report "(a) threads" FAIL "no $target samples after the ${WARMUP_S}s warm-up"
        report "(b) fds" FAIL "no $target samples after the ${WARMUP_S}s warm-up"
        report "(c) rss" FAIL "no $target samples after the ${WARMUP_S}s warm-up"
        return 0
    fi

    detail="$bt at warm-up, max $mt, $et at the end (tolerance +$THREAD_TOLERANCE, $n samples)"
    if ((mt - bt <= THREAD_TOLERANCE && et <= bt + THREAD_TOLERANCE)); then
        report "(a) threads" PASS "$detail"
    else
        report "(a) threads" FAIL "$detail"
    fi

    detail="$bf at warm-up, max $mf, $ef at the end (tolerance +$FD_TOLERANCE)"
    if ((mf - bf <= FD_TOLERANCE && ef <= bf + FD_TOLERANCE)); then
        report "(b) fds" PASS "$detail"
    else
        report "(b) fds" FAIL "$detail"
    fi

    trend="decreased at least once"
    if ((mono == 1)); then
        trend="never decreased"
    fi
    detail="${br} kB at warm-up, ${er} kB at the end: ${growth}%, ${trend} (limit ${RSS_GROWTH_LIMIT_PCT}%)"
    if ((mono == 1)) && awk -v g="$growth" -v l="$RSS_GROWTH_LIMIT_PCT" 'BEGIN { exit !(g > l) }'; then
        report "(c) rss" FAIL "$detail"
    else
        report "(c) rss" PASS "$detail"
    fi
}

check_lifecycle() {
    local summary code fields problems fault_problems cuts unrecovered natural natural_unrecovered
    local times detail
    summary=$({ grep '"kind":"summary"' "$OUT/lifecycle.jsonl" 2>/dev/null || true; } | tail -n 1)
    code=$(cat "$OUT/lifecycle.exit" 2>/dev/null || true)
    if [[ -z $summary ]]; then
        report "(d) orphans" FAIL "lifecycle_soak printed no summary (exit status ${code:-not recorded}); see lifecycle.log"
        report "(e) link cuts" FAIL "lifecycle_soak printed no summary"
        return 0
    fi

    # One parse that checks every field (d) and (e) read. set -e is off in
    # evaluate (it runs as an if condition), so without this a jq error on a
    # summary that was cut short, or that lacks a field, leaves the fields
    # empty and passes both. Line 1 holds (e)'s counts and recovery times;
    # each problem follows on a line of its own, after "d " or "e ".
    # Cuts that found their sensor already down (natural_faults) are drops
    # lifecycle_soak didn't cause, judged by the same rules as a cut.
    if ! fields=$(jq -e -r '
        select((.connection_problems | type) == "array"
            and (.fault_problems | type) == "array"
            and (.faults | type) == "array"
            and (.natural_faults | type) == "array"
            and (.unrecovered | type) == "number"
            and (.natural_unrecovered | type) == "number")
        | "\(.faults | length) \(.unrecovered) \(.natural_faults | length) \(.natural_unrecovered) \(
            [.faults[].recovered_after_s | numbers]
            | if length == 0 then "none recovered" else "recovered after \(min)-\(max) s" end)",
          (.connection_problems[] | "d \(.)"),
          (.fault_problems[] | "e \(.)")' <<<"$summary" 2>/dev/null); then
        report "(d) orphans" FAIL "summary unreadable (exit status ${code:-not recorded}); see lifecycle.jsonl"
        report "(e) link cuts" FAIL "summary unreadable; see lifecycle.jsonl"
        return 0
    fi
    read -r cuts unrecovered natural natural_unrecovered times <<<"$fields"
    problems=$(sed -n '2,$s/^d //p' <<<"$fields")
    fault_problems=$(sed -n '2,$s/^e //p' <<<"$fields")

    if [[ ($code == 0 || $code == 1) && -z $problems ]]; then
        report "(d) orphans" PASS "no orphan in two samples in a row, nothing connected after shutdown (exit $code)"
    else
        report "(d) orphans" FAIL "exit status ${code:-not recorded}"
        note_list "$problems"
    fi

    if ((FAULT_EVERY_S == 0)); then
        report "(e) link cuts" SKIP "--fault-every 0"
        return 0
    fi
    detail="cuts: $cuts, unrecovered: $unrecovered, $times;"
    detail+=" already down when a cut was due: $natural, unrecovered: $natural_unrecovered"
    if [[ -z $fault_problems ]]; then
        report "(e) link cuts" PASS "$detail"
    else
        report "(e) link cuts" FAIL "$detail"
        note_list "$fault_problems"
    fi
}

check_service() {
    local fields success="" failure="" alive stop_secs rate stopped longest streak
    local problems=""
    fields=$(awk -F, 'NR > 1 && $2 == "service" && $6 != "" { s = $6; f = $7 } END { if (s != "") print s "," f }' \
        "$OUT/samples.csv")
    IFS=, read -r success failure <<<"$fields"
    alive=$(sed -n 's/^alive_at_end=//p' "$OUT/service.stop" 2>/dev/null || true)
    stop_secs=$(sed -n 's/^stop_secs=//p' "$OUT/service.stop" 2>/dev/null || true)

    if [[ -z $success || $success == 0 ]]; then
        problems+="no successful poll"$'\n'
        rate="no successful poll"
    else
        rate="$success polls ok, ${failure:-0} failed ($((${failure:-0} * 100 / (success + ${failure:-0})))%)"
    fi
    if [[ $alive != 1 ]]; then
        stopped="not running at the end"
        problems+="aranet-service wasn't running at the end; see service.log"$'\n'
    elif [[ ! $stop_secs =~ ^[0-9]+$ ]]; then
        stopped="killed ${STOP_WAIT_SECS}s after SIGINT"
        problems+="aranet-service was still running ${STOP_WAIT_SECS}s after SIGINT"$'\n'
    else
        stopped="exited ${stop_secs}s after SIGINT"
        if ((stop_secs > STOP_LIMIT_SECS)); then
            problems+="aranet-service took ${stop_secs}s to exit after SIGINT (limit ${STOP_LIMIT_SECS}s)"$'\n'
        fi
    fi

    streak="BlueZ probe not run: ${PROBE_NOTE:-no bluez-connected.csv}"
    if [[ -f $OUT/bluez-connected.csv ]]; then
        # Probe rows before the warm-up aren't judged, like the samples: a
        # correct service's first poll can keep the link up for longer than
        # the limit while BlueZ pairs the sensor or discovers its services.
        longest=$(awk -F, -v w="$WARMUP_S" '
            NR > 1 && $1 + 0 >= w + 0 && $2 == "1" { if (!run) { start = $1; run = 1 }; if ($1 - start > max) max = $1 - start; seen = 1 }
            NR > 1 && $1 + 0 >= w + 0 && $2 == "0" { run = 0; seen = 1 }
            END { print seen ? max + 0 : -1 }' "$OUT/bluez-connected.csv")
        if ((longest < 0)); then
            # The probe ran but busctl never answered it, for example because
            # the adapter isn't hci0: the check that matters most on Linux
            # didn't happen.
            streak="BlueZ probe got no answer from busctl after the warm-up"
            problems+="busctl never answered the BlueZ probe after the ${WARMUP_S}s warm-up, so BlueZ's connection streak wasn't checked; see bluez-connected.csv"$'\n'
        else
            streak="longest BlueZ Connected streak ${longest}s (limit ${STREAK_LIMIT_SECS}s)"
            if ((longest >= STREAK_LIMIT_SECS)); then
                problems+="BlueZ reported the sensor connected for ${longest}s in a row"$'\n'
            fi
        fi
    fi

    if [[ -z $problems ]]; then
        report "(f) service" PASS "$rate; $stopped; $streak"
    else
        report "(f) service" FAIL "$rate; $stopped; $streak"
        note_list "$problems"
    fi
}

# Applies the criteria to the files in $OUT. Returns 1 on FAIL.
evaluate() {
    FAILED=0
    : >"$OUT/summary.txt"
    print_header "Soak summary"
    note "Output: $OUT"
    note "Target: $TARGET on $OS_NAME, ${DURATION_S}s, samples every ${INTERVAL}s, warm-up ${WARMUP_S}s"
    if wants lifecycle; then
        note ""
        note "lifecycle_soak (DeviceManager: $M_DEVICE, ReconnectingDevice: ${R_DEVICE:-none}, link cut every ${FAULT_EVERY_S}s)"
        check_resources lifecycle
        check_lifecycle
    fi
    if wants service; then
        note ""
        note "aranet-service ($SERVICE_DEVICE)"
        check_resources service
        check_service
    fi
    note ""
    if ((FAILED)); then
        printf '%s%sRESULT: FAIL%s\n' "$BOLD" "$RED" "$NC"
        echo "RESULT: FAIL" >>"$OUT/summary.txt"
        return 1
    fi
    printf '%s%sRESULT: PASS%s\n' "$BOLD" "$GREEN" "$NC"
    echo "RESULT: PASS" >>"$OUT/summary.txt"
}

# ==============================================================================
# Main
# ==============================================================================

main() {
    parse_args "$@"
    if [[ -n $EVALUATE_DIR ]]; then
        [[ -d $EVALUATE_DIR ]] || die "$EVALUATE_DIR is not a directory"
        command -v jq >/dev/null 2>&1 || die "jq is required"
        OUT=$(cd "$EVALUATE_DIR" && pwd) || die "can't use $EVALUATE_DIR"
        load_run_env
        if evaluate; then
            exit 0
        fi
        exit 1
    fi

    validate_options
    check_tools
    prepare_out
    trap cleanup EXIT
    trap 'exit 130' INT TERM
    build_binaries

    WORK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/aranet-soak.XXXXXX") ||
        die "can't create a temporary directory in ${TMPDIR:-/tmp}"
    export ARANET_CONFIG_DIR="$WORK_DIR/config" ARANET_DATA_DIR="$WORK_DIR/data"
    mkdir -p "$ARANET_CONFIG_DIR" "$ARANET_DATA_DIR" || die "can't create directories in $WORK_DIR"

    print_header "Starting"
    RUN_START=$SECONDS
    if wants service; then
        start_service
        start_bluez_probe
    fi
    if wants lifecycle; then
        start_lifecycle
    fi

    print_header "Soaking for ${DURATION_S}s (samples every ${INTERVAL}s in $OUT/samples.csv)"
    sample_loop
    stop_all
    if evaluate; then
        exit 0
    fi
    exit 1
}

main "$@"
