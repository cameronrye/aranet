//! Set command implementation.

use std::io::{self, Write};
use std::time::Duration;

use anyhow::Result;
use aranet_core::{BluetoothRange, MeasurementInterval};

use crate::cli::{BluetoothRangeSetting, DeviceSetting};
use crate::util::{connect_device_with_progress, require_device_interactive};

/// Prompt user for confirmation before making changes.
/// Returns true if user confirms, false otherwise.
fn confirm_change(message: &str, force: bool) -> Result<bool> {
    if force {
        return Ok(true);
    }

    // Check if stdin is a terminal (interactive mode)
    if !atty::is(atty::Stream::Stdin) {
        // Non-interactive mode - require --force flag
        eprintln!("Error: Cannot prompt for confirmation in non-interactive mode.");
        eprintln!("Use --force to skip confirmation.");
        return Ok(false);
    }

    print!("{} [y/N]: ", message);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    let response = input.trim().to_lowercase();
    Ok(response == "y" || response == "yes")
}

/// Format the setting change description for confirmation prompt.
fn describe_setting_change(setting: &DeviceSetting) -> String {
    match setting {
        DeviceSetting::Interval { minutes } => {
            format!("Change measurement interval to {} minute(s)?", minutes)
        }
        DeviceSetting::Range { range } => {
            let range_str = match range {
                BluetoothRangeSetting::Standard => "Standard",
                BluetoothRangeSetting::Extended => "Extended",
            };
            format!("Change Bluetooth range to {}?", range_str)
        }
        DeviceSetting::SmartHome { enabled } => {
            if *enabled {
                "Enable Smart Home integration?".to_string()
            } else {
                "Disable Smart Home integration?".to_string()
            }
        }
    }
}

pub async fn cmd_set(
    device: Option<String>,
    timeout: Duration,
    setting: DeviceSetting,
    quiet: bool,
    force: bool,
) -> Result<()> {
    let identifier = require_device_interactive(device).await?;

    // Confirm before making changes (unless --force is used)
    let confirmation_msg = describe_setting_change(&setting);
    if !confirm_change(&confirmation_msg, force)? {
        eprintln!("Cancelled.");
        return Ok(());
    }

    // Use connect_device_with_progress which has its own spinner
    let device = connect_device_with_progress(&identifier, timeout, !quiet).await?;

    match setting {
        DeviceSetting::Interval { minutes } => {
            // Validation already done by clap parser
            let interval = MeasurementInterval::from_minutes(minutes).ok_or_else(|| {
                anyhow::anyhow!(
                    "Invalid interval: {}. Valid values: 1, 2, 5, 10 minutes.",
                    minutes
                )
            })?;
            device.set_interval(interval).await?;
            if !quiet {
                println!("Measurement interval set to {} minute(s)", minutes);
            }
        }
        DeviceSetting::Range { range } => {
            let bt_range = match range {
                BluetoothRangeSetting::Standard => BluetoothRange::Standard,
                BluetoothRangeSetting::Extended => BluetoothRange::Extended,
            };
            device.set_bluetooth_range(bt_range).await?;
            if !quiet {
                println!("Bluetooth range set to {:?}", bt_range);
            }
        }
        DeviceSetting::SmartHome { enabled } => {
            device.set_smart_home(enabled).await?;
            if !quiet {
                println!(
                    "Smart Home integration {}",
                    if enabled { "enabled" } else { "disabled" }
                );
            }
        }
    }

    device.disconnect().await.ok();
    Ok(())
}
