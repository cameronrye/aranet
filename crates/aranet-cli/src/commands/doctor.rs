//! Doctor command implementation.
//!
//! Performs BLE diagnostics and permission checks to help troubleshoot
//! connectivity issues.

use std::time::Duration;

use anyhow::Result;
use aranet_core::scan::{self, ScanOptions};
use owo_colors::OwoColorize;

use crate::style;

/// Check result with status and message.
struct Check {
    #[allow(dead_code)]
    name: &'static str,
    passed: bool,
    warning: bool,
    message: String,
}

impl Check {
    fn pass(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            passed: true,
            warning: false,
            message: message.into(),
        }
    }

    fn warn(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            passed: true,
            warning: true,
            message: message.into(),
        }
    }

    fn fail(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            passed: false,
            warning: false,
            message: message.into(),
        }
    }
}

pub async fn cmd_doctor(verbose: bool, no_color: bool) -> Result<()> {
    println!(
        "{}",
        style::format_title("Aranet Doctor - BLE Diagnostics", no_color)
    );
    println!();

    let mut checks: Vec<Check> = Vec::new();
    let total_checks = 2;

    // Check 1: Bluetooth adapter availability
    print_check_start(1, total_checks, "Bluetooth Adapter", no_color);
    let adapter_check = check_adapter().await;
    print_check_result(&adapter_check, no_color);
    let adapter_ok = adapter_check.passed;
    checks.push(adapter_check);

    // Check 2: Scan for devices (only if adapter is available)
    if adapter_ok {
        print_check_start(2, total_checks, "Device Scan", no_color);
        let scan_check = check_scan().await;
        print_check_result(&scan_check, no_color);
        checks.push(scan_check);
    }

    println!();
    println!("{}", "─".repeat(50));

    // Summary
    let passed = checks.iter().filter(|c| c.passed && !c.warning).count();
    let warnings = checks.iter().filter(|c| c.warning).count();
    let failed = checks.iter().filter(|c| !c.passed).count();

    let summary = if no_color {
        format!(
            "Summary: {} passed, {} warnings, {} failed",
            passed, warnings, failed
        )
    } else {
        format!(
            "Summary: {} passed, {} warnings, {} failed",
            format!("{}", passed).green(),
            format!("{}", warnings).yellow(),
            format!("{}", failed).red()
        )
    };
    println!("{}", summary);
    println!();

    // Print platform-specific help if there are failures
    if failed > 0 {
        print_troubleshooting_help(verbose, no_color);
    } else if warnings > 0 {
        println!("System is functional but some checks had warnings.");
        println!("Run with --verbose for more details.");
    } else {
        let msg = "All checks passed! Your system is ready to use Aranet devices.";
        println!("{}", style::format_success(msg, no_color));
    }

    Ok(())
}

fn print_check_start(num: usize, total: usize, name: &str, no_color: bool) {
    // Use simple static output instead of a spinner that can't animate during sync blocking
    use std::io::{Write, stdout};
    if no_color {
        print!("[{}/{}] {} ... ", num, total, name);
    } else {
        print!("{} {} ... ", format!("[{}/{}]", num, total).dimmed(), name);
    }
    // Flush to ensure the message appears before the blocking operation
    let _ = stdout().flush();
}

fn print_check_result(check: &Check, no_color: bool) {
    let (icon, msg) = if check.passed && !check.warning {
        if no_color {
            ("[OK]".to_string(), check.message.clone())
        } else {
            (format!("{}", "[OK]".green()), check.message.clone())
        }
    } else if check.warning {
        if no_color {
            ("[!!]".to_string(), check.message.clone())
        } else {
            (
                format!("{}", "[!!]".yellow()),
                format!("{}", check.message.yellow()),
            )
        }
    } else if no_color {
        ("[FAIL]".to_string(), check.message.clone())
    } else {
        (
            format!("{}", "[FAIL]".red()),
            format!("{}", check.message.red()),
        )
    };
    println!("{} {}", icon, msg);
}

async fn check_adapter() -> Check {
    match scan::get_adapter().await {
        Ok(_adapter) => Check::pass("Bluetooth Adapter", "Found and accessible"),
        Err(e) => {
            let msg = format!("Not available ({})", e);
            Check::fail("Bluetooth Adapter", msg)
        }
    }
}

async fn check_scan() -> Check {
    let options = ScanOptions {
        duration: Duration::from_secs(3),
        filter_aranet_only: true,
    };

    match scan::scan_with_options(options).await {
        Ok(devices) => {
            if devices.is_empty() {
                Check::warn("BLE Scanning", "No Aranet devices found nearby")
            } else {
                let names: Vec<String> = devices.iter().filter_map(|d| d.name.clone()).collect();
                Check::pass(
                    "BLE Scanning",
                    format!("Found {} device(s): {}", devices.len(), names.join(", ")),
                )
            }
        }
        Err(e) => Check::fail("BLE Scanning", format!("Failed ({})", e)),
    }
}

fn print_troubleshooting_help(verbose: bool, no_color: bool) {
    let title = if no_color {
        "Troubleshooting Tips:".to_string()
    } else {
        format!("{}", "Troubleshooting Tips:".yellow())
    };
    println!("{}", title);
    println!();

    #[cfg(target_os = "macos")]
    {
        println!("macOS:");
        println!("  • Ensure Bluetooth is enabled in System Settings");
        println!("  • Grant Bluetooth permission to Terminal/your app");
        println!("  • Try: System Settings → Privacy & Security → Bluetooth");
        if verbose {
            println!("  • Check if other BLE apps work (e.g., LightBlue)");
            println!("  • Try resetting Bluetooth: sudo pkill bluetoothd");
        }
    }

    #[cfg(target_os = "linux")]
    {
        println!("Linux:");
        println!("  • Ensure BlueZ is installed: sudo apt install bluez");
        println!("  • Check Bluetooth service: systemctl status bluetooth");
        println!("  • Add user to bluetooth group: sudo usermod -aG bluetooth $USER");
        if verbose {
            println!("  • Check adapter: hciconfig -a");
            println!("  • Restart Bluetooth: sudo systemctl restart bluetooth");
        }
    }

    #[cfg(target_os = "windows")]
    {
        println!("Windows:");
        println!("  • Ensure Bluetooth is enabled in Settings");
        println!("  • Check Device Manager for Bluetooth adapter");
        println!("  • Update Bluetooth drivers if needed");
        if verbose {
            println!("  • Try: Settings → Bluetooth & devices → Bluetooth → On");
        }
    }

    println!();
}
