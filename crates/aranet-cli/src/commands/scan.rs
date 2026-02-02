//! Scan command implementation.

use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use aranet_core::{ScanOptions, scan};

use crate::cli::OutputFormat;
use crate::config::Config;
use crate::format::{
    FormatOptions, format_scan_csv, format_scan_json, format_scan_text_with_aliases,
};
use crate::style;
use crate::util::write_output;

pub async fn cmd_scan(
    timeout: u64,
    format: OutputFormat,
    output: Option<&PathBuf>,
    quiet: bool,
    save_alias: bool,
    opts: &FormatOptions,
    config: &Config,
) -> Result<()> {
    // Show spinner for text output (unless quiet)
    let spinner = if !quiet && matches!(format, OutputFormat::Text) {
        Some(style::scanning_spinner(timeout))
    } else {
        None
    };

    let options = ScanOptions::default()
        .duration_secs(timeout)
        .filter_aranet_only(true);

    let devices = scan::scan_with_options(options)
        .await
        .context("Failed to scan for devices")?;

    // Clear spinner before output
    if let Some(sp) = spinner {
        sp.finish_and_clear();
    }

    // For text format, show aliases and tips
    let content = match format {
        OutputFormat::Json => format_scan_json(&devices, opts)?,
        OutputFormat::Text => {
            // Show aliases column if any devices have aliases, and show tips
            format_scan_text_with_aliases(&devices, opts, Some(&config.aliases), !quiet)
        }
        OutputFormat::Csv => format_scan_csv(&devices, opts),
    };

    write_output(output, &content)?;

    // Handle --alias flag for interactive alias saving
    if save_alias && !devices.is_empty() && matches!(format, OutputFormat::Text) {
        save_aliases_interactive(&devices, config)?;
    }

    Ok(())
}

/// Generate a suggested alias from a device name.
/// Converts "Aranet4 12ABC" to "aranet4-12abc" style.
fn suggest_alias(device_name: &str) -> String {
    device_name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Interactively prompt user to save aliases for discovered devices.
fn save_aliases_interactive(
    devices: &[aranet_core::scan::DiscoveredDevice],
    config: &Config,
) -> Result<()> {
    use std::io::BufRead;

    let mut config = config.clone();
    let mut saved_count = 0;

    // Show existing aliases first
    if !config.aliases.is_empty() {
        println!("\nExisting aliases:");
        for (alias, address) in &config.aliases {
            println!("  {} -> {}", alias, address);
        }
    }

    println!("\nSave device aliases:");
    for device in devices {
        let name = device.name.as_deref().unwrap_or("Unknown");
        let id_lower = device.identifier.to_lowercase();

        // Check if already has an alias
        let existing_alias = config
            .aliases
            .iter()
            .find(|(_, v)| v.to_lowercase() == id_lower)
            .map(|(k, _)| k.clone());

        if let Some(alias) = existing_alias {
            println!("  {} - already aliased as '{}'", name, alias);
            continue;
        }

        // Generate a suggested alias
        let suggested = suggest_alias(name);
        let suggestion_hint = if suggested.is_empty() || suggested == "-" {
            String::new()
        } else {
            format!(" [suggested: {}]", suggested)
        };

        print!("  {} - alias (enter to skip){}: ", name, suggestion_hint);
        io::stdout().flush()?;

        let stdin = io::stdin();
        let mut input = String::new();
        stdin.lock().read_line(&mut input)?;
        let alias = input.trim();

        if !alias.is_empty() {
            config
                .aliases
                .insert(alias.to_string(), device.identifier.clone());
            saved_count += 1;
        }
    }

    if saved_count > 0 {
        config.save()?;
        println!("\nSaved {} alias(es).", saved_count);
    } else {
        println!("\nNo aliases saved.");
    }

    Ok(())
}
