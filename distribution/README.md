# Distribution Files

Configuration files and templates for distributing Aranet binaries via package managers and running services.

## Package Manager Installation

### Homebrew (macOS/Linux)

```bash
# Add the tap and install
brew tap cameronrye/aranet
brew install aranet

# Or install directly
brew install cameronrye/aranet/aranet
```

### Shell Script (macOS/Linux)

```bash
curl --proto '=https' --tlsv1.2 -sSf https://github.com/cameronrye/aranet/releases/latest/download/aranet-installer.sh | sh
```

### PowerShell (Windows)

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://github.com/cameronrye/aranet/releases/latest/download/aranet-installer.ps1 | iex"
```

### Debian/Ubuntu (apt)

Download the `.deb` package from [GitHub Releases](https://github.com/cameronrye/aranet/releases):

```bash
# Download the latest .deb
curl -LO https://github.com/cameronrye/aranet/releases/latest/download/aranet_0.1.8_amd64.deb

# Install
sudo dpkg -i aranet_0.1.8_amd64.deb

# Install dependencies if needed
sudo apt-get install -f
```

### Cargo (from source)

```bash
cargo install aranet-cli
```

## Building Debian Packages

Use `cargo-deb` to build `.deb` packages:

```bash
# Install cargo-deb
cargo install cargo-deb

# Build the package
cargo deb -p aranet-cli

# The .deb file will be in target/debian/
```

## Homebrew Formula

The Homebrew formula template is in `distribution/homebrew/aranet.rb`. To set up the tap:

1. Create a GitHub repository: `cameronrye/homebrew-aranet`
2. Copy the formula to `Formula/aranet.rb` in that repo
3. Update SHA256 checksums after each release

---

## Service Configuration

Pre-built service configuration files for running `aranet-service` as a background daemon.

## Linux (systemd)

```bash
# Copy service file
sudo cp distribution/systemd/aranet.service /etc/systemd/system/

# Create user and directories
sudo useradd -r -s /bin/false aranet
sudo mkdir -p /var/lib/aranet
sudo chown aranet:aranet /var/lib/aranet

# Copy binary
sudo cp target/release/aranet-service /usr/local/bin/

# Enable and start
sudo systemctl daemon-reload
sudo systemctl enable aranet
sudo systemctl start aranet

# Check status
sudo systemctl status aranet
journalctl -u aranet -f
```

## macOS (launchd)

### User-level service (recommended)

```bash
# Create directories
mkdir -p ~/Library/LaunchAgents
mkdir -p ~/.local/share/aranet
mkdir -p ~/.local/var/log

# Copy plist (modify paths for user-level)
cp distribution/launchd/dev.rye.aranet.plist ~/Library/LaunchAgents/

# Edit plist to use user paths:
# - WorkingDirectory: ~/.local/share/aranet
# - StandardOutPath: ~/.local/var/log/aranet.log
# - StandardErrorPath: ~/.local/var/log/aranet.err

# Load service
launchctl load ~/Library/LaunchAgents/dev.rye.aranet.plist

# Check status
launchctl list | grep aranet
```

### System-level service

```bash
# Copy plist
sudo cp distribution/launchd/dev.rye.aranet.plist /Library/LaunchDaemons/

# Create directories
sudo mkdir -p /usr/local/var/aranet
sudo mkdir -p /usr/local/var/log

# Copy binary
sudo cp target/release/aranet-service /usr/local/bin/

# Load service
sudo launchctl load /Library/LaunchDaemons/dev.rye.aranet.plist

# Check status
sudo launchctl list | grep aranet
```

## Using the CLI (Recommended)

The easiest way to manage the service is using the built-in CLI commands:

```bash
# Install as a service (auto-detects platform)
aranet-service service install

# Start the service
aranet-service service start

# Check status
aranet-service service status

# Stop the service
aranet-service service stop

# Uninstall the service
aranet-service service uninstall
```

## Configuration

The service reads configuration from `~/.config/aranet/server.toml`:

```toml
[server]
bind = "127.0.0.1:8080"

[storage]
path = "~/.local/share/aranet/data.db"

[[devices]]
address = "Aranet4 17C3C"
alias = "office"
poll_interval = 60
```

---

Made with ❤️ by [Cameron Rye](https://rye.dev/)
