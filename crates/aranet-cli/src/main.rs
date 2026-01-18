//! Aranet CLI - Command-line interface for Aranet environmental sensors.
//!
//! This binary supports multiple feature configurations:
//! - `cli` feature: Provides command-line interface with subcommands
//! - `tui` feature: Provides interactive terminal dashboard
//! - Both features: CLI with `tui` subcommand to launch dashboard
//! - Neither feature: Compile error (at least one required)

// Ensure at least one feature is enabled
#[cfg(not(any(feature = "cli", feature = "tui")))]
compile_error!("At least one of 'cli' or 'tui' features must be enabled");

// CLI modules (conditionally compiled)
#[cfg(feature = "cli")]
mod cli;
#[cfg(feature = "cli")]
mod commands;
#[cfg(feature = "cli")]
mod config;
#[cfg(feature = "cli")]
mod format;
#[cfg(feature = "cli")]
mod style;
#[cfg(feature = "cli")]
mod util;

// TUI module (conditionally compiled)
#[cfg(feature = "tui")]
mod tui;

use anyhow::Result;

#[cfg(feature = "cli")]
use std::io;
#[cfg(feature = "cli")]
use std::time::Duration;
#[cfg(feature = "cli")]
use clap::{CommandFactory, Parser};
#[cfg(feature = "cli")]
use tracing_subscriber::EnvFilter;
#[cfg(feature = "cli")]
use cli::{AliasSubcommand, Cli, Commands, ConfigAction, ConfigKey, OutputFormat};
#[cfg(feature = "cli")]
use commands::{
    AliasAction, HistoryArgs, WatchArgs, cmd_alias, cmd_doctor, cmd_history, cmd_info, cmd_read,
    cmd_scan, cmd_set, cmd_status, cmd_watch,
};
#[cfg(feature = "cli")]
use config::{Config, get_device_source, resolve_devices, resolve_timeout};
#[cfg(feature = "cli")]
use format::FormatOptions;

// =============================================================================
// Main Entry Point
// =============================================================================

/// TUI-only mode: Launch the dashboard directly
#[cfg(all(feature = "tui", not(feature = "cli")))]
#[tokio::main]
async fn main() -> Result<()> {
    tui::run().await
}

/// CLI mode (with or without TUI): Parse commands and dispatch
#[cfg(feature = "cli")]
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle completions command early (before tracing init)
    if let Commands::Completions { shell } = cli.command {
        let mut cmd = Cli::command();
        clap_complete::generate(shell, &mut cmd, env!("CARGO_BIN_NAME"), &mut io::stdout());
        return Ok(());
    }

    // Handle config commands early
    if let Commands::Config { ref action } = cli.command {
        return handle_config_command(action);
    }

    // Handle alias commands early
    if let Commands::Alias { ref action } = cli.command {
        return handle_alias_command(action, cli.quiet);
    }

    // Handle TUI command early (when both features enabled)
    #[cfg(feature = "tui")]
    if let Commands::Tui = cli.command {
        return tui::run().await;
    }

    // Load config for device resolution
    let config = Config::load();

    // Initialize tracing (write to stderr so stdout is clean for data)
    let filter = if cli.quiet {
        EnvFilter::new("warn")
    } else if cli.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"))
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    let output = cli.output.as_ref();
    let no_color = cli.no_color || config.no_color;
    let quiet = cli.quiet;
    let compact = cli.compact;
    let style = cli.style;
    // Base fahrenheit from config (can be overridden per-command)
    let config_fahrenheit = config.fahrenheit;
    // Base bq from config (currently always false, but future-proofed)
    let config_bq = false;
    // Base inhg from config (currently always false, but future-proofed)
    let config_inhg = false;
    // Parse config format (used as fallback when command format is default)
    let config_format = config.format.as_deref().and_then(parse_format);

    match cli.command {
        Commands::Scan {
            timeout,
            format,
            no_header,
            alias,
        } => {
            let format = resolve_format_with_config(cli.json, format, config_format);
            let timeout = resolve_timeout(timeout, &config, 10);
            let opts = FormatOptions::new(no_color, config_fahrenheit, style)
                .with_no_header(no_header)
                .with_compact(compact);
            cmd_scan(timeout, format, output, quiet, alias, &opts, &config).await?;
        }
        Commands::Examples => {
            print_examples();
        }
        Commands::Read {
            device,
            output: out,
            passive,
        } => {
            let format = resolve_format_with_config(cli.json, out.format, config_format);
            // If no devices specified, try last device before falling back to interactive
            let devices = if device.device.is_empty() {
                if let Some(dev) = resolve_device_with_hint(None, &config, quiet) {
                    vec![dev]
                } else {
                    vec![]
                }
            } else {
                resolve_devices(device.device, &config)
            };
            let timeout = Duration::from_secs(resolve_timeout(device.timeout, &config, 30));
            let opts = FormatOptions::new(no_color, out.resolve_fahrenheit(config_fahrenheit), style)
                .with_no_header(out.no_header)
                .with_compact(compact)
                .with_bq(out.resolve_bq(config_bq))
                .with_inhg(out.resolve_inhg(config_inhg));
            cmd_read(devices, timeout, format, output, quiet, passive, &opts).await?;
        }
        Commands::Status {
            device,
            output: out,
            brief,
        } => {
            let format = resolve_format_with_config(cli.json, out.format, config_format);
            let dev = resolve_device_with_hint(device.device, &config, quiet);
            let timeout = Duration::from_secs(resolve_timeout(device.timeout, &config, 30));
            let opts = FormatOptions::new(no_color, out.resolve_fahrenheit(config_fahrenheit), style)
                .with_no_header(out.no_header)
                .with_compact(compact)
                .with_bq(out.resolve_bq(config_bq))
                .with_inhg(out.resolve_inhg(config_inhg));
            cmd_status(dev, timeout, format, output, &opts, brief).await?;
        }
        Commands::History {
            device,
            output: out,
            count,
            since,
            until,
        } => {
            let format = resolve_format_with_config(cli.json, out.format, config_format);
            let dev = resolve_device_with_hint(device.device, &config, quiet);
            // History uses a longer default timeout (60s)
            let timeout = Duration::from_secs(resolve_timeout(device.timeout, &config, 30));
            let opts = FormatOptions::new(no_color, out.resolve_fahrenheit(config_fahrenheit), style)
                .with_no_header(out.no_header)
                .with_compact(compact)
                .with_bq(out.resolve_bq(config_bq))
                .with_inhg(out.resolve_inhg(config_inhg));
            cmd_history(HistoryArgs {
                device: dev,
                count,
                since,
                until,
                timeout,
                format,
                output,
                quiet,
                opts: &opts,
            })
            .await?;
        }
        Commands::Info {
            device,
            format,
            no_header,
        } => {
            let format = resolve_format_with_config(cli.json, format, config_format);
            let dev = resolve_device_with_hint(device.device, &config, quiet);
            let timeout = Duration::from_secs(resolve_timeout(device.timeout, &config, 30));
            let opts = FormatOptions::new(no_color, config_fahrenheit, style)
                .with_no_header(no_header)
                .with_compact(compact);
            cmd_info(dev, timeout, format, output, quiet, &opts).await?;
        }
        Commands::Set { device, setting } => {
            let dev = resolve_device_with_hint(device.device, &config, quiet);
            let timeout = Duration::from_secs(resolve_timeout(device.timeout, &config, 30));
            cmd_set(dev, timeout, setting, quiet).await?;
        }
        Commands::Watch {
            device,
            output: out,
            interval,
            count,
            passive,
        } => {
            let format = resolve_format_with_config(cli.json, out.format, config_format);
            // For passive mode without explicit device, don't resolve to last device
            // This allows watching ALL devices via advertisements
            let dev = if passive && device.device.is_none() {
                None
            } else {
                resolve_device_with_hint(device.device, &config, quiet)
            };
            let timeout = Duration::from_secs(resolve_timeout(device.timeout, &config, 30));
            let opts = FormatOptions::new(no_color, out.resolve_fahrenheit(config_fahrenheit), style)
                .with_no_header(out.no_header)
                .with_compact(compact)
                .with_bq(out.resolve_bq(config_bq))
                .with_inhg(out.resolve_inhg(config_inhg));
            cmd_watch(WatchArgs {
                device: dev,
                interval,
                count,
                timeout,
                format,
                output,
                passive,
                opts: &opts,
            })
            .await?;
        }
        Commands::Doctor => {
            cmd_doctor(cli.verbose, no_color).await?;
        }
        Commands::Config { .. } => unreachable!(),
        Commands::Alias { .. } => unreachable!(),
        Commands::Completions { .. } => unreachable!(),
        #[cfg(feature = "tui")]
        Commands::Tui => unreachable!(), // Handled above
    }

    Ok(())
}

#[cfg(feature = "cli")]
fn handle_alias_command(action: &AliasSubcommand, quiet: bool) -> Result<()> {
    let alias_action = match action {
        AliasSubcommand::List => AliasAction::List,
        AliasSubcommand::Set { name, address } => AliasAction::Set {
            name: name.clone(),
            address: address.clone(),
        },
        AliasSubcommand::Remove { name } => AliasAction::Remove { name: name.clone() },
    };
    cmd_alias(alias_action, quiet)
}

#[cfg(feature = "cli")]
fn handle_config_command(action: &ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Path => {
            println!("{}", Config::path().display());
        }
        ConfigAction::Show => {
            let config = Config::load();
            println!("{}", toml::to_string_pretty(&config)?);
        }
        ConfigAction::Init => {
            let path = Config::path();
            if path.exists() {
                eprintln!("Config file already exists: {}", path.display());
            } else {
                Config::default().save()?;
                println!("Created config file: {}", path.display());
            }
        }
        ConfigAction::Get { key } => {
            let config = Config::load();
            let value = match key {
                ConfigKey::Device => config.device.unwrap_or_default(),
                ConfigKey::Format => config.format.unwrap_or_else(|| "text".to_string()),
                ConfigKey::Timeout => config.timeout.map(|t| t.to_string()).unwrap_or_default(),
                ConfigKey::NoColor => config.no_color.to_string(),
                ConfigKey::Fahrenheit => config.fahrenheit.to_string(),
            };
            println!("{}", value);
        }
        ConfigAction::Set { key, value } => {
            let mut config = Config::load();
            match key {
                ConfigKey::Device => config.device = Some(value.clone()),
                ConfigKey::Format => {
                    // Validate format value
                    match value.to_lowercase().as_str() {
                        "text" | "json" | "csv" => config.format = Some(value.to_lowercase()),
                        _ => anyhow::bail!(
                            "Invalid format: {}. Valid values: text, json, csv",
                            value
                        ),
                    }
                }
                ConfigKey::Timeout => {
                    let seconds: u64 = value.parse().map_err(|_| {
                        anyhow::anyhow!(
                            "Invalid timeout value: {}. Must be a positive integer (seconds).",
                            value
                        )
                    })?;
                    config.timeout = Some(seconds);
                }
                ConfigKey::NoColor => {
                    config.no_color = parse_bool(value).map_err(|_| {
                        anyhow::anyhow!("Invalid no_color value: {}. Use 'true' or 'false'.", value)
                    })?;
                }
                ConfigKey::Fahrenheit => {
                    config.fahrenheit = parse_bool(value).map_err(|_| {
                        anyhow::anyhow!(
                            "Invalid fahrenheit value: {}. Use 'true' or 'false'.",
                            value
                        )
                    })?;
                }
            }
            config.save()?;
            println!("Set {:?} = {}", key, value);
        }
        ConfigAction::Unset { key } => {
            let mut config = Config::load();
            match key {
                ConfigKey::Device => config.device = None,
                ConfigKey::Format => config.format = None,
                ConfigKey::Timeout => config.timeout = None,
                ConfigKey::NoColor => config.no_color = false,
                ConfigKey::Fahrenheit => config.fahrenheit = false,
            }
            config.save()?;
            println!("Unset {:?}", key);
        }
    }
    Ok(())
}

/// Parse a boolean value from a string, supporting common representations.
#[cfg(feature = "cli")]
fn parse_bool(s: &str) -> std::result::Result<bool, ()> {
    match s.to_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => Err(()),
    }
}

/// Parse an output format from a config string.
#[cfg(feature = "cli")]
fn parse_format(s: &str) -> Option<OutputFormat> {
    match s.to_lowercase().as_str() {
        "text" => Some(OutputFormat::Text),
        "json" => Some(OutputFormat::Json),
        "csv" => Some(OutputFormat::Csv),
        _ => None,
    }
}

/// Resolve output format with config fallback.
/// Priority: --json flag > --format arg (if not default) > config format > default (text)
#[cfg(feature = "cli")]
fn resolve_format_with_config(
    cli_json: bool,
    cmd_format: OutputFormat,
    config_format: Option<OutputFormat>,
) -> OutputFormat {
    if cli_json {
        OutputFormat::Json
    } else if !matches!(cmd_format, OutputFormat::Text) {
        // Command explicitly specified a non-default format
        cmd_format
    } else {
        // Use config format if available, otherwise default to text
        config_format.unwrap_or(OutputFormat::Text)
    }
}

/// Show a message about which device source is being used.
/// Returns the resolved device identifier.
#[cfg(feature = "cli")]
fn resolve_device_with_hint(
    device: Option<String>,
    config: &Config,
    quiet: bool,
) -> Option<String> {
    let (resolved, source) = get_device_source(device.as_deref(), config);

    // Show hint about device source (unless quiet mode)
    if !quiet {
        if let Some(source) = source {
            if let Some(ref dev) = resolved {
                let name = config
                    .last_device_name
                    .as_deref()
                    .filter(|_| source == "last");
                match (source, name) {
                    ("last", Some(name)) => {
                        eprintln!("Using last connected device: {} ({})", name, dev);
                    }
                    ("last", None) => {
                        eprintln!("Using last connected device: {}", dev);
                    }
                    ("default", _) => {
                        // Don't show message for default device - user explicitly configured it
                    }
                    _ => {}
                }
            }
        }
    }

    resolved
}

/// Print common usage examples.
#[cfg(feature = "cli")]
fn print_examples() {
    use owo_colors::OwoColorize;

    println!("{}", "Aranet CLI Examples".bold().underline());
    println!();
    println!("{}", "Getting Started:".bold());
    println!("  aranet scan                      # Find nearby Aranet devices");
    println!("  aranet scan --alias              # Scan and save device aliases interactively");
    println!("  aranet doctor                    # Check Bluetooth connectivity");
    println!();
    println!("{}", "Reading Data:".bold());
    println!("  aranet read                      # Read from default/last device");
    println!("  aranet read -d living-room       # Read using device alias");
    println!("  aranet status                    # Quick one-line status");
    println!("  aranet status --brief            # Super-compact status for scripting");
    println!();
    println!("{}", "Monitoring:".bold());
    println!("  aranet watch                     # Continuously monitor (60s intervals)");
    println!("  aranet watch -i 30               # Monitor every 30 seconds");
    println!("  aranet watch -n 5                # Take 5 readings then exit");
    println!();
    println!("{}", "History & Export:".bold());
    println!("  aranet history                   # Show all stored readings");
    println!("  aranet history --since 2024-01-01");
    println!("  aranet history -f csv > data.csv # Export to CSV file");
    println!("  aranet history -f json           # Export as JSON");
    println!();
    println!("{}", "Device Management:".bold());
    println!("  aranet alias list                # Show saved aliases");
    println!("  aranet alias set office <uuid>   # Create an alias");
    println!("  aranet config set device <uuid>  # Set default device");
    println!("  aranet config show               # Show current configuration");
    println!();
    println!("{}", "Output Options:".bold());
    println!("  aranet read --json               # Output as JSON");
    println!("  aranet read --fahrenheit         # Use Fahrenheit for temperature");
    println!("  aranet read --bq                 # Use Bq/m3 for radon (instead of pCi/L)");
    println!("  aranet read --no-color           # Disable colored output");
    println!();
}

// ============================================================================
// CLI Tests
// ============================================================================

#[cfg(all(test, feature = "cli"))]
mod tests {
    use super::*;

    // ========================================================================
    // resolve_format_with_config tests
    // ========================================================================

    #[test]
    fn test_resolve_format_json_flag_overrides_text() {
        let result = resolve_format_with_config(true, OutputFormat::Text, None);
        assert!(matches!(result, OutputFormat::Json));
    }

    #[test]
    fn test_resolve_format_json_flag_overrides_csv() {
        let result = resolve_format_with_config(true, OutputFormat::Csv, None);
        assert!(matches!(result, OutputFormat::Json));
    }

    #[test]
    fn test_resolve_format_json_flag_overrides_config() {
        let result = resolve_format_with_config(true, OutputFormat::Text, Some(OutputFormat::Csv));
        assert!(matches!(result, OutputFormat::Json));
    }

    #[test]
    fn test_resolve_format_explicit_csv_used() {
        let result = resolve_format_with_config(false, OutputFormat::Csv, None);
        assert!(matches!(result, OutputFormat::Csv));
    }

    #[test]
    fn test_resolve_format_explicit_json_used() {
        let result = resolve_format_with_config(false, OutputFormat::Json, None);
        assert!(matches!(result, OutputFormat::Json));
    }

    #[test]
    fn test_resolve_format_config_fallback() {
        // When cmd format is default (Text) and no --json flag, use config
        let result =
            resolve_format_with_config(false, OutputFormat::Text, Some(OutputFormat::Json));
        assert!(matches!(result, OutputFormat::Json));
    }

    #[test]
    fn test_resolve_format_default_text() {
        // When no config and no explicit format, use Text
        let result = resolve_format_with_config(false, OutputFormat::Text, None);
        assert!(matches!(result, OutputFormat::Text));
    }

    // ========================================================================
    // parse_bool tests
    // ========================================================================

    #[test]
    fn test_parse_bool_true_variants() {
        assert_eq!(parse_bool("true"), Ok(true));
        assert_eq!(parse_bool("True"), Ok(true));
        assert_eq!(parse_bool("TRUE"), Ok(true));
        assert_eq!(parse_bool("yes"), Ok(true));
        assert_eq!(parse_bool("on"), Ok(true));
        assert_eq!(parse_bool("1"), Ok(true));
    }

    #[test]
    fn test_parse_bool_false_variants() {
        assert_eq!(parse_bool("false"), Ok(false));
        assert_eq!(parse_bool("False"), Ok(false));
        assert_eq!(parse_bool("FALSE"), Ok(false));
        assert_eq!(parse_bool("no"), Ok(false));
        assert_eq!(parse_bool("off"), Ok(false));
        assert_eq!(parse_bool("0"), Ok(false));
    }

    #[test]
    fn test_parse_bool_invalid() {
        assert!(parse_bool("invalid").is_err());
        assert!(parse_bool("maybe").is_err());
        assert!(parse_bool("").is_err());
    }

    // ========================================================================
    // parse_format tests
    // ========================================================================

    #[test]
    fn test_parse_format_valid() {
        assert!(matches!(parse_format("text"), Some(OutputFormat::Text)));
        assert!(matches!(parse_format("Text"), Some(OutputFormat::Text)));
        assert!(matches!(parse_format("json"), Some(OutputFormat::Json)));
        assert!(matches!(parse_format("JSON"), Some(OutputFormat::Json)));
        assert!(matches!(parse_format("csv"), Some(OutputFormat::Csv)));
        assert!(matches!(parse_format("CSV"), Some(OutputFormat::Csv)));
    }

    #[test]
    fn test_parse_format_invalid() {
        assert!(parse_format("xml").is_none());
        assert!(parse_format("").is_none());
        assert!(parse_format("invalid").is_none());
    }
}
