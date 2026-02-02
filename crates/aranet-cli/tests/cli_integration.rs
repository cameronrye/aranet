//! CLI Integration Tests
//!
//! These tests verify the CLI binary output formats and command behaviors.
//! Some tests require actual hardware and are marked with #[ignore].
//!
//! Run mock tests:
//! ```
//! cargo test --package aranet-cli --test cli_integration
//! ```
//!
//! Run hardware tests:
//! ```
//! ARANET_DEVICE="Aranet4 12345" cargo test --package aranet-cli --test cli_integration -- --ignored --nocapture
//! ```

use std::env;
use std::process::Command;

/// Get path to the aranet binary
fn get_binary_path() -> String {
    // Try release first, then debug
    let release_path = env!("CARGO_MANIFEST_DIR").to_string() + "/../../target/release/Aranet";
    let debug_path = env!("CARGO_MANIFEST_DIR").to_string() + "/../../target/debug/Aranet";

    if std::path::Path::new(&release_path).exists() {
        release_path
    } else if std::path::Path::new(&debug_path).exists() {
        debug_path
    } else {
        // Fall back to cargo run
        "cargo".to_string()
    }
}

/// Run aranet command and return output
fn run_aranet(args: &[&str]) -> std::process::Output {
    let binary = get_binary_path();

    if binary == "cargo" {
        Command::new("cargo")
            .args(["run", "--package", "aranet-cli", "--"])
            .args(args)
            .output()
            .expect("Failed to run aranet via cargo")
    } else {
        Command::new(&binary)
            .args(args)
            .output()
            .expect("Failed to run aranet binary")
    }
}

/// Get device from environment
fn get_device() -> Option<String> {
    env::var("ARANET_DEVICE").ok().filter(|s| !s.is_empty())
}

// =============================================================================
// Help and Version Tests (no hardware required)
// =============================================================================

#[test]
fn test_help_command() {
    let output = run_aranet(&["--help"]);

    assert!(output.status.success(), "Help should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Aranet") || stdout.contains("aranet"),
        "Help should mention Aranet"
    );
    assert!(stdout.contains("scan"), "Help should list scan command");
    assert!(stdout.contains("read"), "Help should list read command");
    assert!(
        stdout.contains("history"),
        "Help should list history command"
    );
}

#[test]
fn test_version_command() {
    let output = run_aranet(&["--version"]);

    assert!(output.status.success(), "Version should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Version output should contain the binary name
    assert!(
        stdout.contains("Aranet") || stdout.contains("aranet"),
        "Version should contain aranet"
    );
}

#[test]
fn test_subcommand_help() {
    let subcommands = [
        "scan", "read", "watch", "history", "info", "status", "sync", "cache", "doctor",
    ];

    for cmd in subcommands {
        let output = run_aranet(&[cmd, "--help"]);

        assert!(output.status.success(), "{} --help should succeed", cmd);

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.is_empty(), "{} --help should produce output", cmd);
    }
}

// =============================================================================
// Doctor Command (no device required, tests BLE availability)
// =============================================================================

#[test]
fn test_doctor_runs() {
    let output = run_aranet(&["doctor"]);

    // Doctor may return non-zero if there are issues, but should not crash
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);

    // Should produce some diagnostic output
    assert!(
        combined.contains("Bluetooth")
            || combined.contains("BLE")
            || combined.contains("adapter")
            || combined.contains("permission")
            || combined.contains("check"),
        "Doctor should produce diagnostic output"
    );
}

// =============================================================================
// Config Commands (no device required)
// =============================================================================

#[test]
fn test_config_path() {
    let output = run_aranet(&["config", "path"]);

    assert!(output.status.success(), "Config path should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("config") || stdout.contains(".toml"),
        "Should show config path"
    );
}

#[test]
fn test_config_show() {
    let output = run_aranet(&["config", "show"]);

    // May fail if no config exists, that's OK
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should not crash
    assert!(
        output.status.success() || stderr.contains("not found") || stderr.contains("No config"),
        "Config show should succeed or indicate no config"
    );
}

// =============================================================================
// Cache Commands (no device required)
// =============================================================================

#[test]
fn test_cache_info() {
    let output = run_aranet(&["cache", "info"]);

    assert!(output.status.success(), "Cache info should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should show database info
    assert!(
        stdout.contains("database")
            || stdout.contains("path")
            || stdout.contains("cache")
            || stdout.contains("store"),
        "Should show cache/database info"
    );
}

#[test]
fn test_cache_devices() {
    let output = run_aranet(&["cache", "devices"]);

    // May have no devices, that's OK
    assert!(output.status.success(), "Cache devices should succeed");
}

// =============================================================================
// Alias Commands (no device required)
// =============================================================================

#[test]
fn test_alias_list() {
    let output = run_aranet(&["alias", "list"]);

    // May have no aliases, that's OK
    assert!(
        output.status.success(),
        "Alias list should succeed (even if empty)"
    );
}

// =============================================================================
// Scan Tests (requires BLE but not specific device)
// =============================================================================

#[test]
#[ignore = "requires BLE hardware"]
fn test_scan_text_output() {
    let output = run_aranet(&["scan", "--timeout", "5"]);

    // Scan may find no devices, but should complete
    assert!(output.status.success(), "Scan should complete");
}

#[test]
#[ignore = "requires BLE hardware"]
fn test_scan_json_output() {
    let output = run_aranet(&["scan", "--timeout", "5", "--format", "json"]);

    assert!(output.status.success(), "Scan JSON should complete");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should be valid JSON (array, possibly empty)
    if !stdout.trim().is_empty() {
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(&stdout);
        assert!(
            parsed.is_ok(),
            "Scan JSON output should be valid JSON: {}",
            stdout
        );
    }
}

#[test]
#[ignore = "requires BLE hardware"]
fn test_scan_csv_output() {
    let output = run_aranet(&["scan", "--timeout", "5", "--format", "csv"]);

    assert!(output.status.success(), "Scan CSV should complete");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // If there's output, first line should be header
    if !stdout.trim().is_empty() {
        let first_line = stdout.lines().next().unwrap_or("");
        assert!(
            first_line.contains("name")
                || first_line.contains("address")
                || first_line.contains(','),
            "CSV should have header or be comma-separated"
        );
    }
}

// =============================================================================
// Read Tests (requires specific device)
// =============================================================================

#[test]
#[ignore = "requires BLE hardware and device"]
fn test_read_text_output() {
    let device = match get_device() {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET_DEVICE not set");
            return;
        }
    };

    let output = run_aranet(&["read", "--device", &device]);

    assert!(output.status.success(), "Read should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should contain sensor readings
    assert!(
        stdout.contains("CO2")
            || stdout.contains("ppm")
            || stdout.contains("Temperature")
            || stdout.contains("°C")
            || stdout.contains("Humidity"),
        "Read output should contain sensor data"
    );
}

#[test]
#[ignore = "requires BLE hardware and device"]
fn test_read_json_output() {
    let device = match get_device() {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET_DEVICE not set");
            return;
        }
    };

    let output = run_aranet(&["read", "--device", &device, "--format", "json"]);

    assert!(output.status.success(), "Read JSON should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("Read JSON should be valid JSON");

    // Should contain expected fields
    assert!(
        parsed.get("co2").is_some()
            || parsed.get("temperature").is_some()
            || parsed.as_array().map(|a| !a.is_empty()).unwrap_or(false),
        "JSON should contain reading data"
    );
}

#[test]
#[ignore = "requires BLE hardware and device"]
fn test_read_json_compact() {
    let device = match get_device() {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET_DEVICE not set");
            return;
        }
    };

    let output = run_aranet(&["--compact", "read", "--device", &device, "--format", "json"]);

    assert!(output.status.success(), "Read JSON compact should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Compact JSON should not have pretty-printing (no leading spaces)
    assert!(
        !stdout.contains("\n  "),
        "Compact JSON should not be pretty-printed"
    );

    // But should still be valid JSON
    let _: serde_json::Value = serde_json::from_str(&stdout).expect("Compact JSON should be valid");
}

#[test]
#[ignore = "requires BLE hardware and device"]
fn test_read_csv_output() {
    let device = match get_device() {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET_DEVICE not set");
            return;
        }
    };

    let output = run_aranet(&["read", "--device", &device, "--format", "csv"]);

    assert!(output.status.success(), "Read CSV should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();

    assert!(lines.len() >= 2, "CSV should have header and data rows");

    // First line should be header
    let header = lines[0];
    assert!(
        header.contains("co2") || header.contains("temperature") || header.contains(','),
        "CSV should have recognizable header"
    );

    // Data row should have same number of columns
    let header_cols = header.split(',').count();
    let data_cols = lines[1].split(',').count();
    assert_eq!(
        header_cols, data_cols,
        "Header and data should have same column count"
    );
}

#[test]
#[ignore = "requires BLE hardware and device"]
fn test_read_csv_no_header() {
    let device = match get_device() {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET_DEVICE not set");
            return;
        }
    };

    let output = run_aranet(&[
        "read",
        "--device",
        &device,
        "--format",
        "csv",
        "--no-header",
    ]);

    assert!(output.status.success(), "Read CSV no-header should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // First line should NOT be a header (should be data)
    let first_line = stdout.lines().next().unwrap_or("");

    // Data line typically starts with a number (CO2 or timestamp)
    // or doesn't contain column names
    assert!(
        !first_line.contains("co2")
            || first_line
                .chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false),
        "CSV no-header should not have header row"
    );
}

#[test]
#[ignore = "requires BLE hardware and device"]
fn test_read_fahrenheit() {
    let device = match get_device() {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET_DEVICE not set");
            return;
        }
    };

    let output = run_aranet(&["read", "--device", &device, "--fahrenheit"]);

    assert!(
        output.status.success(),
        "Read with --fahrenheit should succeed"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should show Fahrenheit
    assert!(
        stdout.contains("°F") || stdout.contains("F"),
        "Output should show Fahrenheit"
    );
}

#[test]
#[ignore = "requires BLE hardware and device"]
fn test_read_quiet_mode() {
    let device = match get_device() {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET_DEVICE not set");
            return;
        }
    };

    let output = run_aranet(&["--quiet", "read", "--device", &device, "--format", "json"]);

    assert!(output.status.success(), "Quiet read should succeed");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Quiet mode should suppress informational messages on stderr
    assert!(
        stderr.is_empty() || !stderr.contains("Connecting"),
        "Quiet mode should suppress connection messages"
    );
}

// =============================================================================
// Status Tests
// =============================================================================

#[test]
#[ignore = "requires BLE hardware and device"]
fn test_status_output() {
    let device = match get_device() {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET_DEVICE not set");
            return;
        }
    };

    let output = run_aranet(&["status", "--device", &device]);

    assert!(output.status.success(), "Status should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.is_empty(), "Status should produce output");
}

#[test]
#[ignore = "requires BLE hardware and device"]
fn test_status_brief() {
    let device = match get_device() {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET_DEVICE not set");
            return;
        }
    };

    let output = run_aranet(&["status", "--device", &device, "--brief"]);

    assert!(output.status.success(), "Brief status should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Brief should be more compact
    let line_count = stdout.lines().count();
    assert!(
        line_count <= 3,
        "Brief status should be compact (got {} lines)",
        line_count
    );
}

// =============================================================================
// Info Tests
// =============================================================================

#[test]
#[ignore = "requires BLE hardware and device"]
fn test_info_output() {
    let device = match get_device() {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET_DEVICE not set");
            return;
        }
    };

    let output = run_aranet(&["info", "--device", &device]);

    assert!(output.status.success(), "Info should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should contain device info fields
    assert!(
        stdout.contains("Name")
            || stdout.contains("Model")
            || stdout.contains("Firmware")
            || stdout.contains("Serial"),
        "Info should show device details"
    );
}

#[test]
#[ignore = "requires BLE hardware and device"]
fn test_info_json() {
    let device = match get_device() {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET_DEVICE not set");
            return;
        }
    };

    let output = run_aranet(&["info", "--device", &device, "--format", "json"]);

    assert!(output.status.success(), "Info JSON should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("Info JSON should be valid");

    // Should have expected fields
    assert!(
        parsed.get("name").is_some() || parsed.get("model").is_some(),
        "Info JSON should contain device fields"
    );
}

// =============================================================================
// History Tests
// =============================================================================

#[test]
#[ignore = "requires BLE hardware and device - slow"]
fn test_history_limited_count() {
    let device = match get_device() {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET_DEVICE not set");
            return;
        }
    };

    let output = run_aranet(&["history", "--device", &device, "--count", "5"]);

    assert!(output.status.success(), "History should succeed");
}

#[test]
#[ignore = "requires BLE hardware and device - slow"]
fn test_history_json() {
    let device = match get_device() {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET_DEVICE not set");
            return;
        }
    };

    let output = run_aranet(&[
        "history", "--device", &device, "--count", "5", "--format", "json",
    ]);

    assert!(output.status.success(), "History JSON should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);

    if !stdout.trim().is_empty() {
        let parsed: serde_json::Value =
            serde_json::from_str(&stdout).expect("History JSON should be valid");

        // Should be an array
        assert!(parsed.is_array(), "History JSON should be an array");
    }
}

// =============================================================================
// Watch Tests
// =============================================================================

#[test]
#[ignore = "requires BLE hardware and device - slow"]
fn test_watch_limited_count() {
    let device = match get_device() {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET_DEVICE not set");
            return;
        }
    };

    // Watch with 2 readings at 2-second interval
    let output = run_aranet(&[
        "watch",
        "--device",
        &device,
        "--count",
        "2",
        "--interval",
        "2",
    ]);

    assert!(output.status.success(), "Watch should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should have produced some output
    assert!(!stdout.is_empty(), "Watch should produce output");
}

#[test]
#[ignore = "requires BLE hardware and device - slow"]
fn test_watch_json() {
    let device = match get_device() {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET_DEVICE not set");
            return;
        }
    };

    let output = run_aranet(&[
        "watch",
        "--device",
        &device,
        "--count",
        "2",
        "--interval",
        "2",
        "--format",
        "json",
    ]);

    assert!(output.status.success(), "Watch JSON should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Watch JSON output may be pretty-printed (multi-line JSON blocks)
    // Try to find and parse JSON objects from the output
    let mut json_count = 0;
    let mut in_json = false;
    let mut json_buffer = String::new();
    let mut brace_count = 0;

    for line in stdout.lines() {
        let trimmed = line.trim();

        // Start of JSON object
        if trimmed.starts_with('{') {
            in_json = true;
            json_buffer.clear();
            brace_count = 0;
        }

        if in_json {
            json_buffer.push_str(line);
            json_buffer.push('\n');
            brace_count += trimmed.matches('{').count();
            brace_count -= trimmed.matches('}').count();

            // End of JSON object
            if brace_count == 0 {
                let parsed: Result<serde_json::Value, _> = serde_json::from_str(&json_buffer);
                if parsed.is_ok() {
                    json_count += 1;
                }
                in_json = false;
            }
        }
    }

    // Should have at least one JSON reading
    assert!(
        json_count >= 1,
        "Should have at least one JSON reading (found {})",
        json_count
    );
}

// =============================================================================
// Error Handling Tests
// =============================================================================

#[test]
fn test_invalid_subcommand() {
    let output = run_aranet(&["notacommand"]);

    assert!(!output.status.success(), "Invalid subcommand should fail");
}

#[test]
fn test_missing_required_args() {
    // read without device should fail (unless default configured)
    let output = run_aranet(&["read"]);

    // May succeed if there's a default device, or fail if not
    // Just ensure it doesn't crash
    let _ = output.status;
}

#[test]
#[ignore = "requires BLE hardware"]
fn test_invalid_device() {
    let output = run_aranet(&["read", "--device", "NonExistentDevice12345"]);

    // Should fail with a reasonable error
    assert!(
        !output.status.success(),
        "Read with invalid device should fail"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found")
            || stderr.contains("failed")
            || stderr.contains("error")
            || stderr.contains("timeout")
            || stderr.to_lowercase().contains("could not"),
        "Should show helpful error message"
    );
}

// =============================================================================
// Output File Tests
// =============================================================================

#[test]
#[ignore = "requires BLE hardware and device"]
fn test_output_to_file() {
    let device = match get_device() {
        Some(d) => d,
        None => {
            println!("SKIP: ARANET_DEVICE not set");
            return;
        }
    };

    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let output_path = temp_dir.path().join("output.json");

    let output = run_aranet(&[
        "--output",
        output_path.to_str().unwrap(),
        "read",
        "--device",
        &device,
        "--format",
        "json",
    ]);

    assert!(
        output.status.success(),
        "Read with output file should succeed"
    );

    // File should exist and contain JSON
    assert!(output_path.exists(), "Output file should be created");

    let content = std::fs::read_to_string(&output_path).expect("Should read output file");
    let _: serde_json::Value =
        serde_json::from_str(&content).expect("File should contain valid JSON");
}
