//! Set command implementation.

use std::time::Duration;

use anyhow::Result;
use aranet_core::{BluetoothRange, MeasurementInterval};

use crate::cli::{BluetoothRangeSetting, DeviceSetting};
use crate::util::{connect_device_with_progress, require_device_interactive};

pub async fn cmd_set(
    device: Option<String>,
    timeout: Duration,
    setting: DeviceSetting,
    quiet: bool,
) -> Result<()> {
    let identifier = require_device_interactive(device).await?;

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
