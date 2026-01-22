<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg">
    <img alt="Aranet" src="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg" height="60">
  </picture>
</p>

# aranet-tui

Terminal UI dashboard for Aranet environmental sensors.

A feature-rich terminal dashboard built with [ratatui](https://ratatui.rs/) for real-time monitoring of Aranet sensor data.

## Features

- **Real-time monitoring** - Live sensor readings with auto-refresh
- **Multiple device support** - Monitor several Aranet devices simultaneously
- **Color-coded values** - Visual indicators for CO2, radon, and battery levels
- **Trend indicators** - Up/down/stable arrows showing reading trends
- **Sparkline charts** - Historical data visualization with min/max labels
- **Threshold alerts** - Audio and visual alerts when CO2/radon exceeds limits
- **Theme support** - Light and dark themes
- **Mouse support** - Click to select devices and tabs
- **Keyboard navigation** - Vim-style keybindings
- **Export to CSV** - Export history data directly from the TUI
- **Cross-platform** - Works on macOS, Linux, and Windows

## Installation

```bash
cargo install aranet-tui
```

Or run via the main CLI:

```bash
# Via aranet-cli (built-in TUI feature)
aranet tui
```

## Demo

![Aranet TUI Demo](https://raw.githubusercontent.com/cameronrye/aranet/main/assets/screenshots/tui-demo.gif)

## Usage

```bash
# Launch the TUI dashboard
aranet-tui

# Or via the main CLI
aranet tui
```

## Keyboard Controls

### Navigation

| Key | Action |
|-----|--------|
| `Tab` / `Shift+Tab` | Next/Previous tab |
| `j` / `k` / `↑` / `↓` | Navigate devices |
| `Enter` | Connect/Disconnect device or edit setting |
| `q` / `Ctrl+C` | Quit |

### Device Actions

| Key | Action |
|-----|--------|
| `s` | Scan for devices |
| `r` | Refresh current reading |
| `S` | Sync history from device |
| `C` | Connect all known devices |
| `n` | Set device alias/nickname |

### Views & Charts

| Key | Action |
|-----|--------|
| `g` | Full-screen chart view |
| `v` | Comparison view (side-by-side) |
| `<` / `>` | Cycle comparison device |
| `T` | Toggle temperature on chart |
| `H` | Toggle humidity on chart |
| `[` | Toggle sidebar visibility |
| `]` | Toggle sidebar width |

### History & Filters

| Key | Action |
|-----|--------|
| `0` - `4` | Time range (0=all, 1=today, 2=24h, 3=7d, 4=30d) |
| `PgUp` / `PgDn` | Scroll history records |
| `e` | Export history to CSV |
| `f` | Cycle device filter (All/Aranet4/Radon/Radiation/Connected) |

### Settings

| Key | Action |
|-----|--------|
| `+` / `-` | Adjust CO2/radon threshold |
| `B` | Toggle Bluetooth range (standard/extended) |
| `I` | Toggle Smart Home mode |

### Alerts & Notifications

| Key | Action |
|-----|--------|
| `a` | View alert history |
| `A` | Toggle sticky alerts |
| `b` | Toggle terminal bell |
| `Esc` | Dismiss current alert |

### Display

| Key | Action |
|-----|--------|
| `t` | Toggle light/dark theme |
| `?` | Show help/keyboard shortcuts |
| `E` | Show error details |
| `Y` / `N` | Confirm/Cancel dialogs |

## Layout

```
┌─ Aranet Monitor ─────────────── *2/3 CO2:847 !1 ─┐
│ ┌─ Devices ───────┐ ┌─ Readings ─────────────────┤
│ │ * Aranet4 17C3C │ │ CO2         847 ppm  ->    │
│ │   AranetRn+ 306 │ │ Temperature  22.4 C        │
│ │   Aranet2 A1B2  │ │ Humidity     45%           │
│ │                 │ │ Pressure  1013.2 hPa       │
│ │                 │ │ Battery      87%           │
│ │                 │ │                            │
│ │                 │ │ Min: 420  Max: 1205  Avg: 756
│ └─────────────────┘ └────────────────────────────┤
│ ┌─ History ──────────────────────────────────────┤
│ │ ▁▂▃▄▅▆▇█▇▆▅▄▃▂▁▂▃▄▅▆▇█▇▆▅▄▃  1205            │
│ │                                  420            │
│ │ 01/18 08:00              01/19 14:30           │
│ └────────────────────────────────────────────────┤
│ [Devices] [Readings] [History] [Settings]        │
│ Connected to Aranet4 17C3C                       │
└──────────────────────────────────────────────────┘
```

## Related Crates

This crate is part of the [aranet](https://github.com/cameronrye/aranet) workspace:

| Crate | crates.io | Description |
|-------|-----------|-------------|
| [aranet-core](../aranet-core/) | [![crates.io](https://img.shields.io/crates/v/aranet-core.svg)](https://crates.io/crates/aranet-core) | Core BLE library |
| [aranet-types](../aranet-types/) | [![crates.io](https://img.shields.io/crates/v/aranet-types.svg)](https://crates.io/crates/aranet-types) | Shared types |
| [aranet-cli](../aranet-cli/) | [![crates.io](https://img.shields.io/crates/v/aranet-cli.svg)](https://crates.io/crates/aranet-cli) | CLI tool |
| [aranet-store](../aranet-store/) | [![crates.io](https://img.shields.io/crates/v/aranet-store.svg)](https://crates.io/crates/aranet-store) | Local persistence |
| [aranet-service](../aranet-service/) | - | Background collector and REST API |
| [aranet-gui](../aranet-gui/) | - | Desktop GUI application |
| [aranet-wasm](../aranet-wasm/) | - | WebAssembly module |

## License

MIT

---

Made with ❤️ by [Cameron Rye](https://rye.dev/)
