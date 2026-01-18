<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg">
    <img alt="Aranet" src="https://raw.githubusercontent.com/cameronrye/aranet/main/assets/aranet-logo-light.svg" height="60">
  </picture>
</p>

# aranet-tui

Terminal UI dashboard for Aranet environmental sensors.

A real-time terminal dashboard built with [ratatui](https://ratatui.rs/) for monitoring Aranet sensor data.

## Features

- **Real-time monitoring** - Live sensor readings displayed in the terminal
- **Multiple device support** - Monitor several Aranet devices simultaneously
- **Color-coded values** - Visual indicators for CO2, radon, and battery levels
- **Keyboard navigation** - Easy navigation with vim-style keybindings
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

## Usage

```bash
# Launch the TUI dashboard
aranet-tui

# Or via the main CLI
aranet tui
```

### Keyboard Controls

| Key | Action |
|-----|--------|
| `q` | Quit the application |
| `r` | Refresh readings |
| `↑/↓` | Navigate between devices |
| `Tab` | Switch between panels |

## Screenshots

```
┌─ Aranet Dashboard ───────────────────────────────────────────┐
│                                                               │
│  Aranet4 17C3C                                               │
│  ─────────────────────────────────────────────────────────── │
│  CO2:         847 ppm   [GREEN]                              │
│  Temperature: 22.4 C                                         │
│  Humidity:    45%                                            │
│  Pressure:    1013.2 hPa                                     │
│  Battery:     87%                                            │
│                                                               │
│  Last update: 2 minutes ago                                  │
│                                                               │
└───────────────────────────────────────────────────────────────┘
```

## Related Crates

This crate is part of the [aranet](https://github.com/cameronrye/aranet) workspace:

| Crate | crates.io | Description |
|-------|-----------|-------------|
| [aranet-core](../aranet-core/) | [![crates.io](https://img.shields.io/crates/v/aranet-core.svg)](https://crates.io/crates/aranet-core) | Core BLE library for device communication |
| [aranet-types](../aranet-types/) | [![crates.io](https://img.shields.io/crates/v/aranet-types.svg)](https://crates.io/crates/aranet-types) | Shared types for sensor data |
| [aranet-cli](../aranet-cli/) | [![crates.io](https://img.shields.io/crates/v/aranet-cli.svg)](https://crates.io/crates/aranet-cli) | Command-line interface |
| [aranet-store](../aranet-store/) | [![crates.io](https://img.shields.io/crates/v/aranet-store.svg)](https://crates.io/crates/aranet-store) | Local data persistence |

## License

MIT

---

Made with love by [Cameron Rye](https://rye.dev/)

