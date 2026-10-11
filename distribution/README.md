# Distribution Files

Configuration files and templates for distributing Aranet binaries via package managers and running services.

## Package Manager Installation

### Homebrew (macOS, Linux x86_64)

```bash
# Add the tap and install
brew tap cameronrye/aranet
brew install aranet

# Or install directly
brew install cameronrye/aranet/aranet
```

### Shell Script (macOS/Linux)

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/cameronrye/aranet/releases/latest/download/aranet-cli-installer.sh | sh
```

On a minimal Linux system, such as a container image, first install `curl`, `ca-certificates`, `xz-utils` and the D-Bus library (for example `apt-get update && apt-get install -y curl ca-certificates xz-utils libdbus-1-3`, as root or with `sudo`): the installer downloads with `curl` and unpacks a `.tar.xz` archive, and `aranet` needs `libdbus-1.so.3` to talk to BlueZ.

The Linux builds need glibc 2.35 or newer: Ubuntu 22.04, Debian 12, Raspberry Pi OS Bookworm, Fedora 36 or later (not RHEL 9 or its rebuilds, which have glibc 2.34).

### PowerShell (Windows)

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://github.com/cameronrye/aranet/releases/latest/download/aranet-cli-installer.ps1 | iex"
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

The tap, [cameronrye/homebrew-aranet](https://github.com/cameronrye/homebrew-aranet), is updated after
every release by `.github/workflows/homebrew.yml`. It renders two templates from the release's assets:
`.github/homebrew/aranet.rb.template` (the `aranet` formula) and `.github/homebrew/aranet-gui.rb.template`
(the `aranet-gui` cask). Pre-releases are skipped. To publish an existing release again, run
`gh workflow run homebrew.yml -f tag=vX.Y.Z` (or "Publish Homebrew Formula" in the Actions tab).

---

## Service Configuration

Pre-built service configuration files for running `aranet-service` as a background daemon.

## Linux (systemd)

`systemd/aranet.service` is the unit that `sudo aranet-service service install` writes. It runs the service as a dynamic, unprivileged `aranet` user with no extra capabilities, reads `/var/lib/aranet/server.toml` and keeps the database in `/var/lib/aranet` (`/var/lib/private/aranet`, which systemd links there). systemd creates the user and the directory when the service starts.

```bash
# Build, then install the binary where only root can change it
cargo build --release --locked -p aranet-service --features full
sudo install -o root -g root -m 0755 target/release/aranet-service /usr/local/bin/aranet-service

# Install the unit (or run: sudo /usr/local/bin/aranet-service service install)
sudo install -m 0644 distribution/systemd/aranet.service /etc/systemd/system/aranet.service
sudo systemctl daemon-reload
sudo systemctl enable --now aranet

# Add your sensors (the first start created /var/lib/aranet), then restart
sudoedit /var/lib/aranet/server.toml
sudo systemctl restart aranet

# Check status
systemctl status aranet
journalctl -u aranet -f
```

The unit reads environment settings from `/etc/default/aranet` if that file exists (`EnvironmentFile=-/etc/default/aranet`). It is the place for `RUST_LOG` and other variables, one `NAME=value` per line, for example `RUST_LOG=aranet_core=info`; restart the service after changing it. (A `RUST_LOG` entry for exactly `aranet_service`, `tower_http` or `aranet_store` has no effect: the service sets the levels of those three itself.)

`sudo aranet-service service install --database <path>` is refused unless the path is in `/var/lib/aranet`, the only directory the service can write.

The [deployment guide](https://cameronrye.github.io/aranet/docs/service/deployment/#linux-systemd) covers Bluetooth group access (`SupplementaryGroups=bluetooth`), running the released `aranet` binary (`aranet server`) under this unit, and moving the data of a system service installed by 0.2.x.

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

Not supported: a LaunchDaemon runs the service as root. `sudo aranet-service service install` refuses to install one on macOS; use the user-level service above, or `aranet-service service install --user`.

A LaunchDaemon that 0.2.x installed keeps running the service as root after an upgrade. Remove it with `sudo aranet-service service uninstall`, then run `aranet-service service install --user`. Every `aranet-service service` command prints a warning while it is still there.

## Using the CLI (Recommended)

The easiest way to manage the service is using the built-in CLI commands:

```bash
# macOS (LaunchAgent) and Linux (systemd user service): a service that runs as you
aranet-service service install --user
aranet-service service start --user
aranet-service service status --user
aranet-service service stop --user
aranet-service service uninstall --user

# Linux with systemd: the system service aranet.service (binary in /usr/local/bin, see above)
sudo /usr/local/bin/aranet-service service install
sudo systemctl start aranet
```

System-level installs are refused on macOS, Windows, OpenRC and rc.d, where the service would run as root. On Windows, where `service install` doesn't work yet, start `aranet server` (or `aranet-service run`) at logon with Task Scheduler. From a Command Prompt opened with **Run as administrator**, for the `aranet` CLI from the installer:

```text
schtasks /create /tn "Aranet Service" /tr "\"%USERPROFILE%\.cargo\bin\aranet.exe\" server" /sc onlogon /ru "%USERNAME%" /it
```

With `aranet-service` from `cargo install`, use `"\"%USERPROFILE%\.cargo\bin\aranet-service.exe\" run"` as the `/tr` value. `/ru` and `/it` make the task run as you, in your session. Remove it with `schtasks /delete /tn "Aranet Service"`.

## Configuration

The service reads `~/.config/aranet/server.toml` (macOS: `~/Library/Application Support/aranet/server.toml`; the systemd system service: `/var/lib/aranet/server.toml`):

```toml
[server]
bind = "127.0.0.1:8080"

[storage]
path = "~/.local/share/aranet/data.db"  # `~/` is your home directory; leave it out under the system service

# Optional: delete old data at startup and hourly (off by default)
# [storage.retention]
# readings_days = 90
# history_days = 365

[[devices]]
address = "Aranet4 17C3C"
alias = "office"
poll_interval = 60
```

---

Made with ❤️ by [Cameron Rye](https://rye.dev/)
