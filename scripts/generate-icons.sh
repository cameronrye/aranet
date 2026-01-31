#!/bin/bash
# Generate all icon sizes from master SVG
# Requires: rsvg-convert (librsvg), iconutil (macOS), ImageMagick (optional for ICO)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
ASSETS_DIR="$PROJECT_ROOT/assets"
ICON_DIR="$ASSETS_DIR/icon"
LOGO_DIR="$ASSETS_DIR/logo"

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}Generating Aranet icons from master SVG...${NC}"

# Check for required tools
check_tool() {
    if ! command -v "$1" &> /dev/null; then
        echo -e "${YELLOW}Warning: $1 not found. Some icons may not be generated.${NC}"
        return 1
    fi
    return 0
}

# Generate PNG from SVG at specific size
generate_png() {
    local svg="$1"
    local size="$2"
    local output="$3"

    if check_tool rsvg-convert; then
        rsvg-convert -w "$size" -h "$size" "$svg" -o "$output"
        echo "  Generated: $output (${size}x${size})"
    elif check_tool convert; then
        convert -background none -resize "${size}x${size}" "$svg" "$output"
        echo "  Generated: $output (${size}x${size})"
    else
        echo -e "${YELLOW}  Skipped: $output (no converter available)${NC}"
        return 1
    fi
}

# Main icon (for use in the simplified version at small sizes)
MASTER_ICON="$ICON_DIR/aranet-icon.svg"
SIMPLE_ICON="$ICON_DIR/aranet-icon-simple.svg"

if [ ! -f "$MASTER_ICON" ]; then
    echo "Error: Master icon not found at $MASTER_ICON"
    exit 1
fi

# Generate standard PNG sizes
echo -e "\n${GREEN}Generating PNG icons...${NC}"

# Small sizes use simplified icon
for size in 16 22 24 32; do
    generate_png "$SIMPLE_ICON" "$size" "$ICON_DIR/aranet-icon-${size}.png"
done

# Larger sizes use full detail icon
for size in 48 64 128 256 512 1024; do
    generate_png "$MASTER_ICON" "$size" "$ICON_DIR/aranet-icon-${size}.png"
done

# Generate macOS .icns file
echo -e "\n${GREEN}Generating macOS .icns...${NC}"
if check_tool iconutil; then
    ICONSET_DIR="$ICON_DIR/aranet-icon.iconset"
    mkdir -p "$ICONSET_DIR"

    # macOS iconset requires specific naming
    generate_png "$SIMPLE_ICON" 16 "$ICONSET_DIR/icon_16x16.png"
    generate_png "$SIMPLE_ICON" 32 "$ICONSET_DIR/icon_16x16@2x.png"
    generate_png "$SIMPLE_ICON" 32 "$ICONSET_DIR/icon_32x32.png"
    generate_png "$MASTER_ICON" 64 "$ICONSET_DIR/icon_32x32@2x.png"
    generate_png "$MASTER_ICON" 128 "$ICONSET_DIR/icon_128x128.png"
    generate_png "$MASTER_ICON" 256 "$ICONSET_DIR/icon_128x128@2x.png"
    generate_png "$MASTER_ICON" 256 "$ICONSET_DIR/icon_256x256.png"
    generate_png "$MASTER_ICON" 512 "$ICONSET_DIR/icon_256x256@2x.png"
    generate_png "$MASTER_ICON" 512 "$ICONSET_DIR/icon_512x512.png"
    generate_png "$MASTER_ICON" 1024 "$ICONSET_DIR/icon_512x512@2x.png"

    iconutil -c icns "$ICONSET_DIR" -o "$ICON_DIR/aranet-icon.icns"
    echo "  Generated: $ICON_DIR/aranet-icon.icns"

    # Clean up iconset directory
    rm -rf "$ICONSET_DIR"
else
    echo -e "${YELLOW}  Skipped: .icns generation (iconutil not available)${NC}"
fi

# Generate Windows .ico file (requires ImageMagick)
echo -e "\n${GREEN}Generating Windows .ico...${NC}"
if check_tool convert; then
    convert "$ICON_DIR/aranet-icon-16.png" \
            "$ICON_DIR/aranet-icon-24.png" \
            "$ICON_DIR/aranet-icon-32.png" \
            "$ICON_DIR/aranet-icon-48.png" \
            "$ICON_DIR/aranet-icon-64.png" \
            "$ICON_DIR/aranet-icon-128.png" \
            "$ICON_DIR/aranet-icon-256.png" \
            "$ICON_DIR/aranet-icon.ico"
    echo "  Generated: $ICON_DIR/aranet-icon.ico"
else
    echo -e "${YELLOW}  Skipped: .ico generation (ImageMagick not available)${NC}"
fi

# Generate template icons for macOS menu bar (22px)
echo -e "\n${GREEN}Generating macOS menu bar template icons...${NC}"
TEMPLATE_ICON="$ICON_DIR/aranet-icon-template.svg"
if [ -f "$TEMPLATE_ICON" ]; then
    generate_png "$TEMPLATE_ICON" 22 "$ICON_DIR/aranet-icon-template.png"
    generate_png "$TEMPLATE_ICON" 44 "$ICON_DIR/aranet-icon-template@2x.png"
fi

# Copy primary icon to legacy locations for backwards compatibility
echo -e "\n${GREEN}Updating legacy icon locations...${NC}"

# Main assets folder (for README, cargo-bundle, etc.)
cp "$ICON_DIR/aranet-icon-64.png" "$ASSETS_DIR/aranet-icon.png"
echo "  Copied: assets/aranet-icon.png"

cp "$ICON_DIR/aranet-icon.icns" "$ASSETS_DIR/aranet-icon.icns" 2>/dev/null || true

# Crate-specific icons
cp "$ICON_DIR/aranet-icon-64.png" "$PROJECT_ROOT/crates/aranet-cli/assets/aranet-icon.png"
echo "  Copied: crates/aranet-cli/assets/aranet-icon.png"

cp "$ICON_DIR/aranet-icon-64.png" "$PROJECT_ROOT/crates/aranet-service/assets/aranet-icon.png"
echo "  Copied: crates/aranet-service/assets/aranet-icon.png"

# Website favicon
cp "$ASSETS_DIR/favicon.svg" "$PROJECT_ROOT/website/public/favicon.svg"
echo "  Copied: website/public/favicon.svg"

# Website logos
cp "$LOGO_DIR/aranet-logo.svg" "$PROJECT_ROOT/website/src/assets/aranet-logo.svg"
cp "$LOGO_DIR/aranet-logo-dark.svg" "$PROJECT_ROOT/website/src/assets/aranet-logo-dark.svg"
cp "$LOGO_DIR/aranet-logo-light.svg" "$PROJECT_ROOT/website/src/assets/aranet-logo-light.svg"
echo "  Copied: website/src/assets/aranet-logo-*.svg"

# Root logos (for README)
cp "$LOGO_DIR/aranet-logo-light.svg" "$ASSETS_DIR/aranet-logo-light.svg"
cp "$LOGO_DIR/aranet-logo-dark.svg" "$ASSETS_DIR/aranet-logo-dark.svg"
echo "  Copied: assets/aranet-logo-*.svg"

echo -e "\n${GREEN}Icon generation complete!${NC}"
echo ""
echo "Generated assets:"
echo "  - PNG icons: 16, 22, 24, 32, 48, 64, 128, 256, 512, 1024px"
echo "  - macOS: aranet-icon.icns"
echo "  - Windows: aranet-icon.ico (if ImageMagick available)"
echo "  - Template: aranet-icon-template.png (for macOS menu bar)"
echo "  - Logos: aranet-logo.svg, aranet-logo-dark.svg, aranet-logo-light.svg"
echo "  - Favicon: favicon.svg"
