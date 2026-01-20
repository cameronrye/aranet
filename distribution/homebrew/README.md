# Homebrew Tap for Aranet CLI

This is the official Homebrew tap for [Aranet CLI](https://github.com/cameronrye/aranet), a command-line interface for Aranet environmental sensors.

## Installation

```bash
brew tap cameronrye/aranet
brew install aranet
```

Or install directly:

```bash
brew install cameronrye/aranet/aranet
```

## Available Formulas

| Formula | Description |
|---------|-------------|
| `aranet` | CLI for Aranet environmental sensors (CO2, radon, temperature, humidity) |

## Supported Devices

- **Aranet4** - CO2, temperature, humidity, pressure
- **Aranet2** - Temperature, humidity
- **AranetRn+** - Radon, temperature, humidity, pressure
- **Aranet Radiation** - Radiation rate/total, temperature, humidity

## Usage

```bash
# Scan for nearby devices
aranet scan

# Read current sensor values
aranet read

# Continuous monitoring
aranet watch

# Interactive terminal dashboard
aranet tui
```

## More Information

- [GitHub Repository](https://github.com/cameronrye/aranet)
- [Documentation](https://cameronrye.github.io/aranet/)
- [Release Notes](https://github.com/cameronrye/aranet/releases)

---

Made with ❤️ by [Cameron Rye](https://rye.dev/)

