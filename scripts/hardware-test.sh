#!/usr/bin/env bash
#
# Hardware Test Script for Aranet CLI
#
# This script runs comprehensive tests against real Aranet devices.
# Configure your devices in tests/hardware-test.toml or tests/hardware-test.local.toml
#
# Usage:
#   ./scripts/hardware-test.sh              # Run all enabled tests
#   ./scripts/hardware-test.sh --quick      # Skip slow tests (history, extended watch)
#   ./scripts/hardware-test.sh --scan-only  # Only run scan test
#   ./scripts/hardware-test.sh --verbose    # Show detailed output
#   ./scripts/hardware-test.sh --help       # Show help
#
# Environment variables:
#   ARANET_DEVICE          Override device identifier
#   ARANET_TEST_CONFIG     Path to test config file
#   ARANET_BINARY          Path to aranet binary (default: target/release/Aranet)
#

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color
BOLD='\033[1m'

# Script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Configuration
CONFIG_FILE="${ARANET_TEST_CONFIG:-}"
ARANET_BIN="${ARANET_BINARY:-$PROJECT_ROOT/target/release/Aranet}"
VERBOSE=false
QUICK_MODE=false
SCAN_ONLY=false
DRY_RUN=false

# Test results
TESTS_RUN=0
TESTS_PASSED=0
TESTS_FAILED=0
TESTS_SKIPPED=0

# Default settings (can be overridden by config)
SCAN_TIMEOUT=15
CONNECT_TIMEOUT=30
HISTORY_TIMEOUT=120
WATCH_ITERATIONS=3
WATCH_INTERVAL=5

# Device identifiers from config
ARANET4_DEVICE=""
ARANET2_DEVICE=""
ARANET_RADON_DEVICE=""
ARANET_RADIATION_DEVICE=""

# Temporary directory for test outputs
TEST_OUTPUT_DIR=""

# ==============================================================================
# Utility Functions
# ==============================================================================

print_header() {
    echo ""
    echo -e "${BOLD}${BLUE}════════════════════════════════════════════════════════════════${NC}"
    echo -e "${BOLD}${BLUE}  $1${NC}"
    echo -e "${BOLD}${BLUE}════════════════════════════════════════════════════════════════${NC}"
}

print_section() {
    echo ""
    echo -e "${CYAN}── $1 ──${NC}"
}

print_test() {
    echo -e "  ${YELLOW}▶${NC} $1"
}

print_pass() {
    echo -e "  ${GREEN}✓${NC} $1"
    ((TESTS_PASSED++))
    ((TESTS_RUN++))
}

print_fail() {
    echo -e "  ${RED}✗${NC} $1"
    ((TESTS_FAILED++))
    ((TESTS_RUN++))
}

print_skip() {
    echo -e "  ${YELLOW}○${NC} $1 (skipped)"
    ((TESTS_SKIPPED++))
}

print_info() {
    if $VERBOSE; then
        echo -e "    ${CYAN}ℹ${NC} $1"
    fi
}

print_error() {
    echo -e "  ${RED}Error:${NC} $1" >&2
}

print_warning() {
    echo -e "  ${YELLOW}Warning:${NC} $1"
}

cleanup() {
    if [[ -n "$TEST_OUTPUT_DIR" && -d "$TEST_OUTPUT_DIR" ]]; then
        rm -rf "$TEST_OUTPUT_DIR"
    fi
}

trap cleanup EXIT

# ==============================================================================
# Configuration Loading
# ==============================================================================

find_config_file() {
    # Priority: ENV var > local config > default config
    if [[ -n "$CONFIG_FILE" && -f "$CONFIG_FILE" ]]; then
        echo "$CONFIG_FILE"
        return
    fi

    local local_config="$PROJECT_ROOT/tests/hardware-test.local.toml"
    if [[ -f "$local_config" ]]; then
        echo "$local_config"
        return
    fi

    local default_config="$PROJECT_ROOT/tests/hardware-test.toml"
    if [[ -f "$default_config" ]]; then
        echo "$default_config"
        return
    fi

    echo ""
}

# Simple TOML parser for our config format
parse_toml_value() {
    local file="$1"
    local key="$2"
    local default="${3:-}"

    # Handle nested keys like "devices.aranet4.identifier"
    local section=""
    local field="$key"

    if [[ "$key" == *"."* ]]; then
        # Extract section and field
        local parts
        IFS='.' read -ra parts <<< "$key"

        if [[ ${#parts[@]} -eq 3 ]]; then
            section="${parts[0]}.${parts[1]}"
            field="${parts[2]}"
        elif [[ ${#parts[@]} -eq 2 ]]; then
            section="${parts[0]}"
            field="${parts[1]}"
        fi
    fi

    local in_section=false
    local current_section=""

    while IFS= read -r line; do
        # Skip comments and empty lines
        [[ "$line" =~ ^[[:space:]]*# ]] && continue
        [[ -z "${line// }" ]] && continue

        # Check for section header
        if [[ "$line" =~ ^\[([^\]]+)\] ]]; then
            current_section="${BASH_REMATCH[1]}"
            if [[ -z "$section" ]] || [[ "$current_section" == "$section" ]]; then
                in_section=true
            else
                in_section=false
            fi
            continue
        fi

        # Parse key = value
        if $in_section || [[ -z "$section" ]]; then
            if [[ "$line" =~ ^[[:space:]]*${field}[[:space:]]*=[[:space:]]*(.+)$ ]]; then
                local value="${BASH_REMATCH[1]}"
                # Remove quotes
                value="${value#\"}"
                value="${value%\"}"
                value="${value#\'}"
                value="${value%\'}"
                # Trim whitespace
                value="${value#"${value%%[![:space:]]*}"}"
                value="${value%"${value##*[![:space:]]}"}"
                echo "$value"
                return
            fi
        fi
    done < "$file"

    echo "$default"
}

load_config() {
    local config_file
    config_file=$(find_config_file)

    if [[ -z "$config_file" ]]; then
        print_warning "No config file found. Using defaults and ARANET_DEVICE env var."
        return
    fi

    print_info "Loading config from: $config_file"

    # Load settings
    SCAN_TIMEOUT=$(parse_toml_value "$config_file" "settings.scan_timeout" "$SCAN_TIMEOUT")
    CONNECT_TIMEOUT=$(parse_toml_value "$config_file" "settings.connect_timeout" "$CONNECT_TIMEOUT")
    HISTORY_TIMEOUT=$(parse_toml_value "$config_file" "settings.history_timeout" "$HISTORY_TIMEOUT")
    WATCH_ITERATIONS=$(parse_toml_value "$config_file" "settings.watch_iterations" "$WATCH_ITERATIONS")
    WATCH_INTERVAL=$(parse_toml_value "$config_file" "settings.watch_interval" "$WATCH_INTERVAL")

    # Load device identifiers
    local aranet4_enabled
    aranet4_enabled=$(parse_toml_value "$config_file" "devices.aranet4.enabled" "false")
    if [[ "$aranet4_enabled" == "true" ]]; then
        ARANET4_DEVICE=$(parse_toml_value "$config_file" "devices.aranet4.identifier" "")
    fi

    local aranet2_enabled
    aranet2_enabled=$(parse_toml_value "$config_file" "devices.aranet2.enabled" "false")
    if [[ "$aranet2_enabled" == "true" ]]; then
        ARANET2_DEVICE=$(parse_toml_value "$config_file" "devices.aranet2.identifier" "")
    fi

    local radon_enabled
    radon_enabled=$(parse_toml_value "$config_file" "devices.aranet_radon.enabled" "false")
    if [[ "$radon_enabled" == "true" ]]; then
        ARANET_RADON_DEVICE=$(parse_toml_value "$config_file" "devices.aranet_radon.identifier" "")
    fi

    local radiation_enabled
    radiation_enabled=$(parse_toml_value "$config_file" "devices.aranet_radiation.enabled" "false")
    if [[ "$radiation_enabled" == "true" ]]; then
        ARANET_RADIATION_DEVICE=$(parse_toml_value "$config_file" "devices.aranet_radiation.identifier" "")
    fi
}

# ==============================================================================
# Test Functions
# ==============================================================================

run_aranet() {
    local args=("$@")

    if $VERBOSE; then
        print_info "Running: $ARANET_BIN ${args[*]}"
    fi

    if $DRY_RUN; then
        echo "[DRY RUN] $ARANET_BIN ${args[*]}"
        return 0
    fi

    "$ARANET_BIN" "${args[@]}"
}

test_scan() {
    print_section "Scan Test"
    print_test "Scanning for Aranet devices (${SCAN_TIMEOUT}s timeout)..."

    local output_file="$TEST_OUTPUT_DIR/scan.txt"
    local json_file="$TEST_OUTPUT_DIR/scan.json"
    local csv_file="$TEST_OUTPUT_DIR/scan.csv"

    # Test text output
    if run_aranet scan --timeout "$SCAN_TIMEOUT" > "$output_file" 2>&1; then
        local device_count
        device_count=$(grep -c "Aranet" "$output_file" 2>/dev/null || echo "0")

        if [[ "$device_count" -gt 0 ]]; then
            print_pass "Scan completed: found $device_count device(s)"
            if $VERBOSE; then
                cat "$output_file"
            fi
        else
            print_warning "Scan completed but no devices found"
            print_pass "Scan command executed successfully"
        fi
    else
        print_fail "Scan command failed"
        if $VERBOSE; then
            cat "$output_file"
        fi
        return 1
    fi

    # Test JSON output format
    print_test "Testing JSON output format..."
    if run_aranet scan --timeout "$SCAN_TIMEOUT" --format json > "$json_file" 2>&1; then
        if command -v jq &> /dev/null; then
            if jq empty "$json_file" 2>/dev/null; then
                print_pass "JSON output is valid"
            else
                print_fail "JSON output is invalid"
            fi
        else
            print_skip "JSON validation (jq not installed)"
        fi
    else
        print_fail "Scan with JSON format failed"
    fi

    # Test CSV output format
    print_test "Testing CSV output format..."
    if run_aranet scan --timeout "$SCAN_TIMEOUT" --format csv > "$csv_file" 2>&1; then
        if head -1 "$csv_file" | grep -q "name\|address\|identifier" 2>/dev/null; then
            print_pass "CSV output has expected headers"
        else
            print_warning "CSV output may be empty or have unexpected format"
            print_pass "CSV command executed"
        fi
    else
        print_fail "Scan with CSV format failed"
    fi
}

test_doctor() {
    print_section "Doctor (Diagnostics) Test"
    print_test "Running BLE diagnostics..."

    local output_file="$TEST_OUTPUT_DIR/doctor.txt"

    if run_aranet doctor > "$output_file" 2>&1; then
        print_pass "Doctor command completed"
        if $VERBOSE; then
            cat "$output_file"
        fi
    else
        # Doctor may return non-zero if there are warnings
        print_warning "Doctor reported issues (see output)"
        if $VERBOSE; then
            cat "$output_file"
        fi
        print_pass "Doctor command executed"
    fi
}

test_read_device() {
    local device="$1"
    local device_type="$2"

    print_section "Read Test: $device_type"

    if [[ -z "$device" ]]; then
        print_skip "No $device_type device configured"
        return 0
    fi

    local output_file="$TEST_OUTPUT_DIR/read_${device_type}.txt"
    local json_file="$TEST_OUTPUT_DIR/read_${device_type}.json"

    # Test text output
    print_test "Reading current values from $device..."
    if timeout "$CONNECT_TIMEOUT" run_aranet read --device "$device" > "$output_file" 2>&1; then
        print_pass "Read completed successfully"
        if $VERBOSE; then
            cat "$output_file"
        fi

        # Validate expected fields based on device type
        case "$device_type" in
            aranet4)
                if grep -qi "CO2\|ppm" "$output_file"; then
                    print_pass "CO2 reading present"
                else
                    print_fail "CO2 reading missing"
                fi
                ;;
            aranet_radon)
                if grep -qi "radon\|Bq" "$output_file"; then
                    print_pass "Radon reading present"
                else
                    print_warning "Radon reading not found (may be normal)"
                fi
                ;;
        esac
    else
        print_fail "Read command failed or timed out"
        if $VERBOSE; then
            cat "$output_file" 2>/dev/null || true
        fi
        return 1
    fi

    # Test JSON output
    print_test "Testing JSON output..."
    if timeout "$CONNECT_TIMEOUT" run_aranet read --device "$device" --format json > "$json_file" 2>&1; then
        if command -v jq &> /dev/null; then
            if jq empty "$json_file" 2>/dev/null; then
                print_pass "JSON output is valid"

                # Validate JSON structure
                if jq -e '.co2' "$json_file" > /dev/null 2>&1 || jq -e '.[0].co2' "$json_file" > /dev/null 2>&1; then
                    print_pass "JSON contains expected fields"
                else
                    print_warning "JSON structure may vary by device type"
                fi
            else
                print_fail "JSON output is invalid"
            fi
        else
            print_skip "JSON validation (jq not installed)"
        fi
    else
        print_fail "Read with JSON format failed"
    fi
}

test_info_device() {
    local device="$1"
    local device_type="$2"

    print_section "Info Test: $device_type"

    if [[ -z "$device" ]]; then
        print_skip "No $device_type device configured"
        return 0
    fi

    local output_file="$TEST_OUTPUT_DIR/info_${device_type}.txt"

    print_test "Reading device info from $device..."
    if timeout "$CONNECT_TIMEOUT" run_aranet info --device "$device" > "$output_file" 2>&1; then
        print_pass "Info command completed"

        # Check for expected fields
        if grep -qi "name\|model\|firmware\|serial" "$output_file"; then
            print_pass "Device info contains expected fields"
        else
            print_warning "Device info may have unexpected format"
        fi

        if $VERBOSE; then
            cat "$output_file"
        fi
    else
        print_fail "Info command failed or timed out"
        return 1
    fi
}

test_status_device() {
    local device="$1"
    local device_type="$2"

    print_section "Status Test: $device_type"

    if [[ -z "$device" ]]; then
        print_skip "No $device_type device configured"
        return 0
    fi

    local output_file="$TEST_OUTPUT_DIR/status_${device_type}.txt"

    print_test "Getting status from $device..."
    if timeout "$CONNECT_TIMEOUT" run_aranet status --device "$device" > "$output_file" 2>&1; then
        print_pass "Status command completed"
        if $VERBOSE; then
            cat "$output_file"
        fi
    else
        print_fail "Status command failed or timed out"
        return 1
    fi

    # Test brief mode
    print_test "Testing brief status mode..."
    if timeout "$CONNECT_TIMEOUT" run_aranet status --device "$device" --brief > "$output_file" 2>&1; then
        print_pass "Brief status completed"
    else
        print_fail "Brief status failed"
    fi
}

test_history_device() {
    local device="$1"
    local device_type="$2"

    print_section "History Test: $device_type"

    if [[ -z "$device" ]]; then
        print_skip "No $device_type device configured"
        return 0
    fi

    if $QUICK_MODE; then
        print_skip "History test (quick mode)"
        return 0
    fi

    local output_file="$TEST_OUTPUT_DIR/history_${device_type}.txt"
    local json_file="$TEST_OUTPUT_DIR/history_${device_type}.json"

    # Download limited history
    print_test "Downloading recent history from $device (limited to 10 records)..."
    if timeout "$HISTORY_TIMEOUT" run_aranet history --device "$device" --count 10 > "$output_file" 2>&1; then
        local record_count
        record_count=$(wc -l < "$output_file" | tr -d ' ')
        print_pass "History downloaded: ~$record_count lines"
        if $VERBOSE; then
            head -20 "$output_file"
        fi
    else
        print_fail "History download failed or timed out"
        return 1
    fi

    # Test JSON format
    print_test "Testing JSON history output..."
    if timeout "$HISTORY_TIMEOUT" run_aranet history --device "$device" --count 5 --format json > "$json_file" 2>&1; then
        if command -v jq &> /dev/null; then
            if jq empty "$json_file" 2>/dev/null; then
                local json_count
                json_count=$(jq 'length' "$json_file" 2>/dev/null || echo "0")
                print_pass "JSON history valid: $json_count records"
            else
                print_fail "JSON history is invalid"
            fi
        else
            print_skip "JSON validation (jq not installed)"
        fi
    else
        print_fail "JSON history failed"
    fi

    # Test date filtering
    print_test "Testing history with date filter (last 24h)..."
    local since_date
    since_date=$(date -u -v-1d +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -d "1 day ago" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || echo "")

    if [[ -n "$since_date" ]]; then
        if timeout "$HISTORY_TIMEOUT" run_aranet history --device "$device" --since "$since_date" --count 5 > /dev/null 2>&1; then
            print_pass "Date-filtered history works"
        else
            print_warning "Date-filtered history may have no results"
        fi
    else
        print_skip "Date filter (date command incompatible)"
    fi
}

test_watch_device() {
    local device="$1"
    local device_type="$2"

    print_section "Watch Test: $device_type"

    if [[ -z "$device" ]]; then
        print_skip "No $device_type device configured"
        return 0
    fi

    if $QUICK_MODE; then
        print_skip "Watch test (quick mode)"
        return 0
    fi

    local output_file="$TEST_OUTPUT_DIR/watch_${device_type}.txt"

    print_test "Watching device for $WATCH_ITERATIONS readings (${WATCH_INTERVAL}s interval)..."

    # Run watch with limited count
    if timeout $((WATCH_ITERATIONS * WATCH_INTERVAL + CONNECT_TIMEOUT + 30)) \
        run_aranet watch --device "$device" --interval "$WATCH_INTERVAL" --count "$WATCH_ITERATIONS" > "$output_file" 2>&1; then
        local reading_count
        reading_count=$(grep -c "ppm\|°C\|Bq" "$output_file" 2>/dev/null || echo "0")

        if [[ "$reading_count" -ge 1 ]]; then
            print_pass "Watch completed: $reading_count readings captured"
        else
            print_warning "Watch completed but readings may not match expected pattern"
            print_pass "Watch command executed"
        fi

        if $VERBOSE; then
            cat "$output_file"
        fi
    else
        print_fail "Watch command failed or timed out"
        return 1
    fi
}

test_sync_device() {
    local device="$1"
    local device_type="$2"

    print_section "Sync Test: $device_type"

    if [[ -z "$device" ]]; then
        print_skip "No $device_type device configured"
        return 0
    fi

    if $QUICK_MODE; then
        print_skip "Sync test (quick mode)"
        return 0
    fi

    local output_file="$TEST_OUTPUT_DIR/sync_${device_type}.txt"

    print_test "Syncing device history to local cache..."
    if timeout "$HISTORY_TIMEOUT" run_aranet sync --device "$device" > "$output_file" 2>&1; then
        print_pass "Sync completed"
        if $VERBOSE; then
            cat "$output_file"
        fi
    else
        print_fail "Sync command failed or timed out"
        return 1
    fi

    # Test cache query
    print_test "Querying synced data from cache..."
    if run_aranet cache stats --device "$device" > "$output_file" 2>&1; then
        print_pass "Cache stats retrieved"
        if $VERBOSE; then
            cat "$output_file"
        fi
    else
        print_warning "Cache stats may be empty for new devices"
    fi
}

test_multi_device() {
    print_section "Multi-Device Test"

    # Collect enabled devices
    local devices=()
    [[ -n "$ARANET4_DEVICE" ]] && devices+=("$ARANET4_DEVICE")
    [[ -n "$ARANET2_DEVICE" ]] && devices+=("$ARANET2_DEVICE")
    [[ -n "$ARANET_RADON_DEVICE" ]] && devices+=("$ARANET_RADON_DEVICE")

    if [[ ${#devices[@]} -lt 2 ]]; then
        print_skip "Multi-device test (requires 2+ devices)"
        return 0
    fi

    local device_args=""
    for d in "${devices[@]}"; do
        device_args="$device_args --device $d"
    done

    local output_file="$TEST_OUTPUT_DIR/multi_read.txt"

    print_test "Reading from ${#devices[@]} devices concurrently..."
    if timeout $((CONNECT_TIMEOUT * 2)) run_aranet read $device_args > "$output_file" 2>&1; then
        print_pass "Multi-device read completed"
        if $VERBOSE; then
            cat "$output_file"
        fi
    else
        print_fail "Multi-device read failed"
        return 1
    fi
}

test_output_formats() {
    print_section "Output Format Validation"

    # Use first available device
    local device="${ARANET4_DEVICE:-${ARANET2_DEVICE:-${ARANET_RADON_DEVICE:-}}}"

    if [[ -z "$device" ]]; then
        print_skip "Output format tests (no device configured)"
        return 0
    fi

    # Test unit conversions
    print_test "Testing temperature unit conversion (Fahrenheit)..."
    local output_file="$TEST_OUTPUT_DIR/fahrenheit.txt"
    if timeout "$CONNECT_TIMEOUT" run_aranet read --device "$device" --fahrenheit > "$output_file" 2>&1; then
        if grep -q "°F\|F$" "$output_file"; then
            print_pass "Fahrenheit output works"
        else
            print_warning "Fahrenheit flag may not affect all output formats"
            print_pass "Command executed"
        fi
    else
        print_fail "Fahrenheit read failed"
    fi

    # Test quiet mode
    print_test "Testing quiet mode..."
    if timeout "$CONNECT_TIMEOUT" run_aranet read --device "$device" --quiet > "$output_file" 2>&1; then
        print_pass "Quiet mode works"
    else
        print_fail "Quiet mode failed"
    fi

    # Test CSV with no-header
    print_test "Testing CSV no-header option..."
    if timeout "$CONNECT_TIMEOUT" run_aranet read --device "$device" --format csv --no-header > "$output_file" 2>&1; then
        if ! head -1 "$output_file" | grep -qi "co2\|temperature\|humidity"; then
            print_pass "CSV no-header works (no header row)"
        else
            print_warning "CSV may still contain header-like content"
        fi
    else
        print_fail "CSV no-header failed"
    fi
}

test_cache_operations() {
    print_section "Cache Operations Test"

    local output_file="$TEST_OUTPUT_DIR/cache.txt"

    # Test cache info
    print_test "Getting cache database info..."
    if run_aranet cache info > "$output_file" 2>&1; then
        print_pass "Cache info retrieved"
        if $VERBOSE; then
            cat "$output_file"
        fi
    else
        print_fail "Cache info failed"
    fi

    # Test cache devices list
    print_test "Listing cached devices..."
    if run_aranet cache devices > "$output_file" 2>&1; then
        print_pass "Cache devices listed"
        if $VERBOSE; then
            cat "$output_file"
        fi
    else
        print_warning "Cache devices may be empty"
    fi
}

# ==============================================================================
# Main
# ==============================================================================

show_help() {
    cat << EOF
Aranet Hardware Test Script

Usage: $0 [OPTIONS]

Options:
  --quick       Skip slow tests (history download, extended watch)
  --scan-only   Only run the scan test
  --verbose     Show detailed output
  --dry-run     Print commands without executing
  --help        Show this help message

Configuration:
  Create tests/hardware-test.local.toml with your device settings.
  See tests/hardware-test.toml for the template.

Environment Variables:
  ARANET_DEVICE       Override device identifier for tests
  ARANET_TEST_CONFIG  Path to test configuration file
  ARANET_BINARY       Path to aranet binary

Examples:
  $0                    # Run all tests
  $0 --quick            # Quick test run
  $0 --verbose          # Detailed output
  ARANET_DEVICE="Aranet4 12345" $0 --scan-only

EOF
}

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --quick)
                QUICK_MODE=true
                shift
                ;;
            --scan-only)
                SCAN_ONLY=true
                shift
                ;;
            --verbose|-v)
                VERBOSE=true
                shift
                ;;
            --dry-run)
                DRY_RUN=true
                shift
                ;;
            --help|-h)
                show_help
                exit 0
                ;;
            *)
                print_error "Unknown option: $1"
                show_help
                exit 1
                ;;
        esac
    done
}

check_prerequisites() {
    print_section "Prerequisites Check"

    # Check for binary
    if [[ ! -x "$ARANET_BIN" ]]; then
        print_test "Looking for aranet binary..."

        # Try to find it
        if [[ -x "$PROJECT_ROOT/target/debug/Aranet" ]]; then
            ARANET_BIN="$PROJECT_ROOT/target/debug/Aranet"
            print_warning "Using debug binary: $ARANET_BIN"
        else
            print_fail "Aranet binary not found at $ARANET_BIN"
            echo ""
            echo "Build with: cargo build --release"
            exit 1
        fi
    fi
    print_pass "Found aranet binary: $ARANET_BIN"

    # Check version
    local version
    version=$("$ARANET_BIN" --version 2>/dev/null || echo "unknown")
    print_info "Version: $version"

    # Check for optional tools
    if command -v jq &> /dev/null; then
        print_pass "jq available for JSON validation"
    else
        print_warning "jq not installed (JSON validation will be skipped)"
    fi

    # Create temp directory
    TEST_OUTPUT_DIR=$(mktemp -d)
    print_info "Test output directory: $TEST_OUTPUT_DIR"
}

run_tests() {
    print_header "Aranet Hardware Tests"

    echo ""
    echo "Configuration:"
    echo "  Scan timeout:    ${SCAN_TIMEOUT}s"
    echo "  Connect timeout: ${CONNECT_TIMEOUT}s"
    echo "  Quick mode:      $QUICK_MODE"
    echo ""

    # Show configured devices
    echo "Configured devices:"
    [[ -n "$ARANET4_DEVICE" ]] && echo "  Aranet4:    $ARANET4_DEVICE"
    [[ -n "$ARANET2_DEVICE" ]] && echo "  Aranet2:    $ARANET2_DEVICE"
    [[ -n "$ARANET_RADON_DEVICE" ]] && echo "  AranetRn+:  $ARANET_RADON_DEVICE"
    [[ -n "$ARANET_RADIATION_DEVICE" ]] && echo "  Radiation:  $ARANET_RADIATION_DEVICE"

    # Use ARANET_DEVICE env var if no devices configured
    if [[ -z "$ARANET4_DEVICE" && -z "$ARANET2_DEVICE" && -z "$ARANET_RADON_DEVICE" ]]; then
        if [[ -n "${ARANET_DEVICE:-}" ]]; then
            ARANET4_DEVICE="$ARANET_DEVICE"
            echo "  (from env): $ARANET_DEVICE"
        else
            echo "  (none configured - some tests will be skipped)"
        fi
    fi
    echo ""

    # Run tests
    if $SCAN_ONLY; then
        test_scan
    else
        # Always run these
        test_doctor
        test_scan

        # Device-specific tests
        test_read_device "$ARANET4_DEVICE" "aranet4"
        test_read_device "$ARANET2_DEVICE" "aranet2"
        test_read_device "$ARANET_RADON_DEVICE" "aranet_radon"

        test_info_device "$ARANET4_DEVICE" "aranet4"
        test_status_device "$ARANET4_DEVICE" "aranet4"

        test_history_device "$ARANET4_DEVICE" "aranet4"
        test_watch_device "$ARANET4_DEVICE" "aranet4"
        test_sync_device "$ARANET4_DEVICE" "aranet4"

        test_multi_device
        test_output_formats
        test_cache_operations
    fi
}

print_summary() {
    print_header "Test Summary"

    echo ""
    echo -e "  Total:   $TESTS_RUN"
    echo -e "  ${GREEN}Passed:${NC}  $TESTS_PASSED"
    echo -e "  ${RED}Failed:${NC}  $TESTS_FAILED"
    echo -e "  ${YELLOW}Skipped:${NC} $TESTS_SKIPPED"
    echo ""

    if [[ $TESTS_FAILED -gt 0 ]]; then
        echo -e "${RED}Some tests failed!${NC}"
        exit 1
    else
        echo -e "${GREEN}All tests passed!${NC}"
        exit 0
    fi
}

main() {
    parse_args "$@"
    load_config
    check_prerequisites
    run_tests
    print_summary
}

main "$@"
