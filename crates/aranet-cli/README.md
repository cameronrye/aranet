<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg">
    <img alt="Aranet" src="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg" height="60">
  </picture>
</p>

# aranet-cli

[![crates.io](https://img.shields.io/crates/v/aranet-cli.svg)](https://crates.io/crates/aranet-cli)
[![docs.rs](https://docs.rs/aranet-cli/badge.svg)](https://docs.rs/aranet-cli)

Command-line interface for Aranet environmental sensors.

**[Full Documentation](https://cameronrye.github.io/aranet/)**

A fast, scriptable CLI for reading sensor data, downloading history, and configuring Aranet devices (Aranet4, Aranet2, AranetRn+, Aranet Radiation).

## Installation

```bash
cargo install aranet-cli
```

Or build from source:

```bash
git clone https://github.com/cameronrye/aranet.git
cd aranet
cargo build --release --package aranet-cli
```

## Demo

![Aranet CLI Scan Demo](https://raw.githubusercontent.com/cameronrye/aranet/main/assets/screenshots/cli-scan.gif)

## Usage

### Scan for devices

```bash
aranet scan
```

### Read current measurements

```bash
aranet read --device <DEVICE_ADDRESS>
```

### Download measurement history

```bash
aranet history --device <DEVICE_ADDRESS>
aranet history --device <DEVICE_ADDRESS> --count 100 --format csv --output history.csv

# Filter by date range
aranet history --device <DEVICE_ADDRESS> --since 2026-01-15 --until 2026-01-16
```

### Read from multiple devices

```bash
# Specify multiple devices
aranet read -d device1 -d device2

# Or comma-separated
aranet read -d living-room,bedroom,office
```

### Passive read mode

```bash
# Read from BLE advertisements without connecting (requires Smart Home enabled)
aranet read --device <DEVICE_ADDRESS> --passive
```

### Watch real-time data

```bash
# Watch a specific device
aranet watch --device <DEVICE_ADDRESS> --interval 60

# Watch all devices passively (requires Smart Home enabled)
aranet watch --passive

# Watch a specific device passively
aranet watch --passive --device <DEVICE_ADDRESS>
```

### View device information

```bash
aranet info --device <DEVICE_ADDRESS>
```

### Configure device settings

```bash
aranet set --device <DEVICE_ADDRESS> interval 5
aranet set --device <DEVICE_ADDRESS> range extended
```

### Manage device aliases

```bash
# Create an alias for a device
aranet alias set living-room AA:BB:CC:DD:EE:FF

# List all aliases
aranet alias list

# Use aliases instead of addresses
aranet read -d living-room

# Remove an alias
aranet alias remove living-room
```

### Diagnose BLE issues

```bash
aranet doctor
```

### Sync history to local database

```bash
# Sync device history (incremental - only new records)
aranet sync --device <DEVICE_ADDRESS>

# Full sync (re-download all history)
aranet sync --device <DEVICE_ADDRESS> --full
```

### Query cached data

```bash
# List cached devices
aranet cache devices

# Show cache statistics
aranet cache stats

# Query cached history
aranet cache history --device <DEVICE_ADDRESS> --count 100

# Show database info
aranet cache info
```

### Pressure units

```bash
# Display pressure in inches of mercury
aranet read --device <DEVICE_ADDRESS> --inhg

# Explicitly use hPa (default)
aranet read --device <DEVICE_ADDRESS> --hpa
```

## Configuration

The CLI supports persistent configuration via a TOML file:

```bash
# Initialize config file
aranet config init

# Set a default device
aranet config set device <DEVICE_ADDRESS>

# Set default output format
aranet config set format json

# Show current config
aranet config show
```

Configuration options:

- `device` — Default device address
- `format` — Default output format (`text`, `json`, `csv`)
- `timeout` — Connection timeout in seconds
- `no_color` — Disable colored output
- `fahrenheit` — Use Fahrenheit for temperature display
- `inhg` — Use inHg for pressure display
- `bq` — Use Bq/m3 for radon (instead of pCi/L)

## Output Formats

| Format | Description |
|--------|-------------|
| `text` | Human-readable colored output (default) |
| `json` | JSON for scripting and APIs |
| `csv` | CSV for spreadsheets and data analysis |

```bash
aranet read --device <DEVICE> --format json
aranet read --device <DEVICE> --json    # shorthand
```

## Visual Styling

The CLI uses rich styling by default with color-coded values, spinners, and table formatting.

### Style Modes

| Mode | Description |
|------|-------------|
| `rich` | Full styling with tables, spinners, colored values (default) |
| `minimal` | Colors only, no tables or spinners |
| `plain` | No styling, suitable for scripting |

```bash
# Use minimal styling
aranet read --device <DEVICE> --style minimal

# Plain output for scripts
aranet history --device <DEVICE> --style plain

# Set via environment variable
export ARANET_STYLE=minimal
```

### Color-Coded Values

Sensor readings are color-coded based on thresholds:

| Metric | Green | Yellow | Red |
|--------|-------|--------|-----|
| CO2 | < 800 ppm | 800-1000 ppm | > 1000 ppm |
| Radon | < 100 Bq/m3 | 100-150 Bq/m3 | > 150 Bq/m3 |
| Battery | > 50% | 20-50% | < 20% |
| Humidity | 30-60% | Outside range | - |

### Brief Mode

Get a compact one-line status:

```bash
aranet status --device <DEVICE> --brief
# Output: Aranet4 17C3C: 800 ppm [GREEN] | 22.5C | 45% | 85%
```

## Shell Completions

Generate shell completions for your preferred shell:

```bash
aranet completions bash > ~/.local/share/bash-completion/completions/aranet
aranet completions zsh > ~/.zfunc/_aranet
aranet completions fish > ~/.config/fish/completions/aranet.fish
```

## TUI Dashboard

Launch the interactive terminal dashboard:

```bash
aranet tui
```

The TUI provides real-time monitoring with sparkline charts, threshold alerts, multi-device support, and more. See [aranet-tui](../aranet-tui/) for the complete feature list and keybindings.

## Related Crates

This CLI is part of the [aranet](https://github.com/cameronrye/aranet) workspace:

| Crate | crates.io | Description |
|-------|-----------|-------------|
| [aranet-core](../aranet-core/) | [![crates.io](https://img.shields.io/crates/v/aranet-core.svg)](https://crates.io/crates/aranet-core) | Core BLE library for device communication |
| [aranet-types](../aranet-types/) | [![crates.io](https://img.shields.io/crates/v/aranet-types.svg)](https://crates.io/crates/aranet-types) | Shared types for sensor data |
| [aranet-store](../aranet-store/) | [![crates.io](https://img.shields.io/crates/v/aranet-store.svg)](https://crates.io/crates/aranet-store) | Local data persistence |
| [aranet-tui](../aranet-tui/) | [![crates.io](https://img.shields.io/crates/v/aranet-tui.svg)](https://crates.io/crates/aranet-tui) | Terminal UI dashboard |
| [aranet-service](../aranet-service/) | [![crates.io](https://img.shields.io/crates/v/aranet-service.svg)](https://crates.io/crates/aranet-service) | Background collector and REST API |
| [aranet-gui](../aranet-gui/) | [![crates.io](https://img.shields.io/crates/v/aranet-gui.svg)](https://crates.io/crates/aranet-gui) | Desktop GUI application |

## License

MIT

---

Made with ❤️ by [Cameron Rye](https://rye.dev/)
