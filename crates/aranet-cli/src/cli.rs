//! CLI argument definitions using clap.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Output format for commands
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
    Csv,
}

/// Visual styling mode for output
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum StyleMode {
    /// Standard styling with colors
    Minimal,
    /// Rich styling with tables, icons, and full formatting (default)
    #[default]
    Rich,
    /// Plain text with no decorations (for scripting)
    Plain,
}

/// Reusable device connection arguments
#[derive(Debug, Clone, Args)]
pub struct DeviceArgs {
    /// Device address (MAC address or UUID), or use ARANET_DEVICE env var
    #[arg(short, long, env = "ARANET_DEVICE")]
    pub device: Option<String>,

    /// Connection timeout in seconds
    #[arg(short = 'T', long, default_value = "30")]
    pub timeout: u64,
}

/// Device arguments that support multiple devices
#[derive(Debug, Clone, Args)]
pub struct MultiDeviceArgs {
    /// Device address(es) - can be specified multiple times, or comma-separated
    #[arg(short, long, value_delimiter = ',', env = "ARANET_DEVICE")]
    pub device: Vec<String>,

    /// Connection timeout in seconds (per device)
    #[arg(short = 'T', long, default_value = "30")]
    pub timeout: u64,
}

/// Reusable output format arguments
#[derive(Debug, Clone, Args)]
pub struct OutputArgs {
    /// Output format
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,

    /// Use Fahrenheit for temperature display (overrides --celsius and config)
    #[arg(long, conflicts_with = "celsius")]
    pub fahrenheit: bool,

    /// Use Celsius for temperature display (default, overrides config)
    #[arg(long, conflicts_with = "fahrenheit")]
    pub celsius: bool,

    /// Use Bq/m³ for radon (SI units, overrides --pci and config)
    #[arg(long, conflicts_with = "pci")]
    pub bq: bool,

    /// Use pCi/L for radon (default US units, overrides config)
    #[arg(long, conflicts_with = "bq")]
    pub pci: bool,

    /// Use inHg for pressure display (overrides --hpa and config)
    #[arg(long, conflicts_with = "hpa")]
    pub inhg: bool,

    /// Use hPa for pressure display (default, overrides config)
    #[arg(long, conflicts_with = "inhg")]
    pub hpa: bool,

    /// Omit header row in CSV output (useful for appending)
    #[arg(long)]
    pub no_header: bool,
}

impl OutputArgs {
    /// Resolve fahrenheit setting: explicit flags override config
    pub fn resolve_fahrenheit(&self, config_fahrenheit: bool) -> bool {
        if self.fahrenheit {
            true
        } else if self.celsius {
            false
        } else {
            config_fahrenheit
        }
    }

    /// Resolve bq setting: explicit flags override config
    /// (Currently config doesn't have bq, but this future-proofs it)
    pub fn resolve_bq(&self, config_bq: bool) -> bool {
        if self.bq {
            true
        } else if self.pci {
            false
        } else {
            config_bq
        }
    }

    /// Resolve inhg setting: explicit flags override config
    pub fn resolve_inhg(&self, config_inhg: bool) -> bool {
        if self.inhg {
            true
        } else if self.hpa {
            false
        } else {
            config_inhg
        }
    }
}

#[derive(Parser)]
#[command(name = "aranet")]
#[command(author, version, about = "CLI for Aranet environmental sensors", long_about = None)]
pub struct Cli {
    /// Enable verbose output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Suppress non-essential output
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Output as JSON (shorthand for --format json)
    #[arg(long, global = true)]
    pub json: bool,

    /// Output compact JSON (no pretty-printing)
    #[arg(long, global = true)]
    pub compact: bool,

    /// Disable colored output
    #[arg(long, global = true, env = "NO_COLOR")]
    pub no_color: bool,

    /// Visual styling mode (minimal, rich, plain)
    #[arg(
        long,
        global = true,
        value_enum,
        default_value = "rich",
        env = "ARANET_STYLE"
    )]
    pub style: StyleMode,

    /// Write output to file instead of stdout
    #[arg(short, long, global = true)]
    pub output: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Scan for nearby Aranet devices
    Scan {
        /// Scan timeout in seconds
        #[arg(short, long, default_value = "10")]
        timeout: u64,

        /// Output format
        #[arg(short, long, value_enum, default_value = "text")]
        format: OutputFormat,

        /// Omit header row in CSV output (useful for appending)
        #[arg(long)]
        no_header: bool,

        /// Interactively save aliases for discovered devices
        #[arg(short, long)]
        alias: bool,
    },

    /// Read current sensor values from one or more devices
    Read {
        #[command(flatten)]
        device: MultiDeviceArgs,

        #[command(flatten)]
        output: OutputArgs,

        /// Read from BLE advertisements without connecting (requires Smart Home enabled)
        #[arg(long)]
        passive: bool,
    },

    /// Quick one-line status from a device
    Status {
        #[command(flatten)]
        device: DeviceArgs,

        #[command(flatten)]
        output: OutputArgs,

        /// Super-compact single-line output for scripting
        #[arg(long)]
        brief: bool,
    },

    /// Retrieve historical data from a device
    History {
        #[command(flatten)]
        device: DeviceArgs,

        #[command(flatten)]
        output: OutputArgs,

        /// Number of records to retrieve (0 for all)
        #[arg(short, long, default_value = "0")]
        count: u32,

        /// Filter records since this date/time (RFC3339 or YYYY-MM-DD)
        #[arg(long)]
        since: Option<String>,

        /// Filter records until this date/time (RFC3339 or YYYY-MM-DD)
        #[arg(long)]
        until: Option<String>,
    },

    /// Display device information
    Info {
        #[command(flatten)]
        device: DeviceArgs,

        /// Output format
        #[arg(short, long, value_enum, default_value = "text")]
        format: OutputFormat,

        /// Omit header row in CSV output (useful for appending)
        #[arg(long)]
        no_header: bool,
    },

    /// Configure device settings
    Set {
        #[command(flatten)]
        device: DeviceArgs,

        #[command(subcommand)]
        setting: DeviceSetting,
    },

    /// Continuously monitor a device
    Watch {
        #[command(flatten)]
        device: DeviceArgs,

        #[command(flatten)]
        output: OutputArgs,

        /// Polling interval in seconds
        #[arg(short, long, default_value = "60")]
        interval: u64,

        /// Number of readings to take before exiting (0 for unlimited)
        #[arg(short = 'n', long, default_value = "0")]
        count: u32,

        /// Watch from BLE advertisements without connecting (requires Smart Home enabled)
        #[arg(long)]
        passive: bool,
    },

    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Manage device aliases (friendly names)
    Alias {
        #[command(subcommand)]
        action: AliasSubcommand,
    },

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },

    /// Run BLE diagnostics and permission checks
    Doctor,

    /// Show common usage examples
    Examples,

    /// Launch interactive terminal dashboard
    #[cfg(feature = "tui")]
    Tui,
}

/// Alias subcommands
#[derive(Debug, Clone, Subcommand)]
pub enum AliasSubcommand {
    /// List all device aliases
    List,

    /// Set a device alias
    Set {
        /// Friendly name for the device (e.g., "living-room", "office")
        name: String,

        /// Device address (MAC address or UUID)
        address: String,
    },

    /// Remove a device alias
    #[command(alias = "rm")]
    Remove {
        /// Alias name to remove
        name: String,
    },
}

/// Device settings that can be configured
#[derive(Debug, Clone, Subcommand)]
pub enum DeviceSetting {
    /// Set measurement interval
    Interval {
        /// Interval in minutes (valid: 1, 2, 5, 10)
        #[arg(value_parser = parse_interval)]
        minutes: u8,
    },

    /// Set Bluetooth range
    Range {
        /// Range setting
        #[arg(value_enum)]
        range: BluetoothRangeSetting,
    },

    /// Enable or disable Smart Home integration
    SmartHome {
        /// Enable Smart Home mode
        #[arg(value_parser = parse_bool_arg)]
        enabled: bool,
    },
}

/// Bluetooth range setting values
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum BluetoothRangeSetting {
    /// Standard range (lower power consumption)
    Standard,
    /// Extended range (higher power consumption)
    Extended,
}

/// Parse interval value with validation
fn parse_interval(s: &str) -> Result<u8, String> {
    let minutes: u8 = s
        .parse()
        .map_err(|_| format!("'{}' is not a valid number", s))?;
    match minutes {
        1 | 2 | 5 | 10 => Ok(minutes),
        _ => Err(format!(
            "Invalid interval '{}'. Valid values: 1, 2, 5, 10 minutes",
            minutes
        )),
    }
}

/// Parse boolean argument with flexible input
fn parse_bool_arg(s: &str) -> Result<bool, String> {
    match s.to_lowercase().as_str() {
        "true" | "yes" | "on" | "1" | "enable" | "enabled" => Ok(true),
        "false" | "no" | "off" | "0" | "disable" | "disabled" => Ok(false),
        _ => Err(format!(
            "Invalid boolean value '{}'. Use: true/false, yes/no, on/off, 1/0",
            s
        )),
    }
}

/// Configuration keys
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ConfigKey {
    /// Default device address
    Device,
    /// Default output format
    Format,
    /// Default connection timeout in seconds
    Timeout,
    /// Disable colored output
    NoColor,
    /// Use Fahrenheit for temperature
    Fahrenheit,
}

/// Configuration subcommands
#[derive(Subcommand)]
pub enum ConfigAction {
    /// Show current configuration
    Show,

    /// Get a configuration value
    Get {
        /// Configuration key
        #[arg(value_enum)]
        key: ConfigKey,
    },

    /// Set a configuration value
    Set {
        /// Configuration key
        #[arg(value_enum)]
        key: ConfigKey,
        /// Configuration value
        value: String,
    },

    /// Unset (remove) a configuration value
    Unset {
        /// Configuration key to remove
        #[arg(value_enum)]
        key: ConfigKey,
    },

    /// Show configuration file path
    Path,

    /// Initialize default configuration
    Init,
}
