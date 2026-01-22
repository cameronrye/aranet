#!/usr/bin/env bash
# Generate screenshots for documentation
#
# This script generates all screenshots for the Aranet project:
# - CLI demos using VHS (terminal recordings -> GIFs)
# - TUI demos using VHS
# - GUI screenshots using demo mode
#
# Requirements:
# - VHS (https://github.com/charmbracelet/vhs) for terminal recordings
# - Rust toolchain for building aranet-gui
#
# Usage:
#   ./scripts/generate-screenshots.sh [--vhs-only] [--gui-only]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ASSETS_DIR="$PROJECT_ROOT/assets/screenshots"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Parse arguments
VHS_ONLY=false
GUI_ONLY=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --vhs-only)
            VHS_ONLY=true
            shift
            ;;
        --gui-only)
            GUI_ONLY=true
            shift
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

echo -e "${GREEN}=== Aranet Screenshot Generator ===${NC}"
echo "Project root: $PROJECT_ROOT"
echo "Output directory: $ASSETS_DIR"
echo ""

# Create output directory
mkdir -p "$ASSETS_DIR"

# Check for VHS
check_vhs() {
    if ! command -v vhs &> /dev/null; then
        echo -e "${YELLOW}Warning: VHS not found. Install from https://github.com/charmbracelet/vhs${NC}"
        echo "  brew install vhs  # macOS"
        echo "  go install github.com/charmbracelet/vhs@latest  # Go"
        return 1
    fi
    return 0
}

# Generate VHS recordings
generate_vhs() {
    echo -e "${GREEN}Generating VHS recordings...${NC}"
    
    local tapes_dir="$PROJECT_ROOT/scripts/tapes"
    
    if [[ ! -d "$tapes_dir" ]]; then
        echo -e "${YELLOW}No tapes directory found at $tapes_dir${NC}"
        return
    fi
    
    for tape in "$tapes_dir"/*.tape; do
        if [[ -f "$tape" ]]; then
            local name=$(basename "$tape" .tape)
            echo "  Recording: $name"
            if vhs "$tape" 2>/dev/null; then
                echo -e "    ${GREEN}[OK]${NC} $name.gif"
            else
                echo -e "    ${RED}[FAIL]${NC} Failed to record $name"
            fi
        fi
    done
}

# Generate GUI screenshot
generate_gui() {
    echo -e "${GREEN}Generating GUI screenshot...${NC}"
    
    # Build the GUI in release mode
    echo "  Building aranet-gui..."
    if ! cargo build -p aranet-gui --release 2>/dev/null; then
        echo -e "  ${RED}[FAIL]${NC} Failed to build aranet-gui"
        return 1
    fi
    
    local gui_binary="$PROJECT_ROOT/target/release/aranet-gui"
    local screenshot_path="$ASSETS_DIR/gui-main.png"
    
    echo "  Taking screenshot..."
    if "$gui_binary" --demo --screenshot "$screenshot_path" --screenshot-delay 20 2>/dev/null; then
        echo -e "  ${GREEN}[OK]${NC} gui-main.png"
    else
        echo -e "  ${RED}[FAIL]${NC} Failed to capture GUI screenshot"
        return 1
    fi
}

# Optimize images
optimize_images() {
    echo -e "${GREEN}Optimizing images...${NC}"
    
    # Optimize PNGs
    if command -v optipng &> /dev/null; then
        for png in "$ASSETS_DIR"/*.png; do
            if [[ -f "$png" ]]; then
                echo "  Optimizing $(basename "$png")..."
                optipng -quiet "$png" 2>/dev/null || true
            fi
        done
    else
        echo -e "  ${YELLOW}optipng not found, skipping PNG optimization${NC}"
    fi
    
    # Optimize GIFs
    if command -v gifsicle &> /dev/null; then
        for gif in "$ASSETS_DIR"/*.gif; do
            if [[ -f "$gif" ]]; then
                echo "  Optimizing $(basename "$gif")..."
                gifsicle -O3 --colors 256 "$gif" -o "$gif" 2>/dev/null || true
            fi
        done
    else
        echo -e "  ${YELLOW}gifsicle not found, skipping GIF optimization${NC}"
    fi
}

# Main
main() {
    cd "$PROJECT_ROOT"
    
    if [[ "$GUI_ONLY" == "false" ]] && check_vhs; then
        generate_vhs
    fi
    
    if [[ "$VHS_ONLY" == "false" ]]; then
        generate_gui
    fi
    
    optimize_images
    
    echo ""
    echo -e "${GREEN}Done!${NC} Screenshots saved to: $ASSETS_DIR/"
    ls -la "$ASSETS_DIR/"
}

main

