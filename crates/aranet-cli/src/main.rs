//! Aranet CLI - Command-line interface for Aranet environmental sensors.

mod cli;
mod commands;
mod config;
mod format;
mod style;
mod util;

use std::io;
use std::time::Duration;

use anyhow::Result;
use clap::{CommandFactory, Parser};
use tracing_subscriber::EnvFilter;

use cli::{AliasSubcommand, Cli, Commands, ConfigAction, ConfigKey, OutputFormat};
use commands::{
    AliasAction, HistoryArgs, WatchArgs, cmd_alias, cmd_doctor, cmd_history, cmd_info, cmd_read,
    cmd_scan, cmd_set, cmd_status, cmd_watch,
};
use config::{Config, resolve_device, resolve_devices, resolve_timeout};
use format::FormatOptions;

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
        } => {
            let format = resolve_format_with_config(cli.json, format, config_format);
            let timeout = resolve_timeout(timeout, &config, 10);
            let opts = FormatOptions::new(no_color, config_fahrenheit)
                .with_no_header(no_header)
                .with_compact(compact);
            cmd_scan(timeout, format, output, quiet, &opts).await?;
        }
        Commands::Read {
            device,
            output: out,
            passive,
        } => {
            let format = resolve_format_with_config(cli.json, out.format, config_format);
            let devices = resolve_devices(device.device, &config);
            let timeout = Duration::from_secs(resolve_timeout(device.timeout, &config, 30));
            let opts = FormatOptions::new(no_color, out.resolve_fahrenheit(config_fahrenheit))
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
            let dev = resolve_device(device.device, &config);
            let timeout = Duration::from_secs(resolve_timeout(device.timeout, &config, 30));
            let opts = FormatOptions::new(no_color, out.resolve_fahrenheit(config_fahrenheit))
                .with_no_header(out.no_header)
                .with_compact(compact)
                .with_bq(out.resolve_bq(config_bq))
                .with_inhg(out.resolve_inhg(config_inhg))
                .with_style(cli.style);
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
            let dev = resolve_device(device.device, &config);
            // History uses a longer default timeout (60s)
            let timeout = Duration::from_secs(resolve_timeout(device.timeout, &config, 30));
            let opts = FormatOptions::new(no_color, out.resolve_fahrenheit(config_fahrenheit))
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
            let dev = resolve_device(device.device, &config);
            let timeout = Duration::from_secs(resolve_timeout(device.timeout, &config, 30));
            let opts = FormatOptions::new(no_color, config_fahrenheit)
                .with_no_header(no_header)
                .with_compact(compact);
            cmd_info(dev, timeout, format, output, quiet, &opts).await?;
        }
        Commands::Set { device, setting } => {
            let dev = resolve_device(device.device, &config);
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
            let dev = resolve_device(device.device, &config);
            let timeout = Duration::from_secs(resolve_timeout(device.timeout, &config, 30));
            let opts = FormatOptions::new(no_color, out.resolve_fahrenheit(config_fahrenheit))
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
    }

    Ok(())
}

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
fn parse_bool(s: &str) -> std::result::Result<bool, ()> {
    match s.to_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => Err(()),
    }
}

/// Parse an output format from a config string.
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

// ============================================================================
// CLI Tests
// ============================================================================

#[cfg(test)]
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
