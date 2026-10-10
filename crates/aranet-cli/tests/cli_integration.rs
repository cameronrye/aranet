//! CLI Integration Tests
//!
//! These tests run the real `aranet` binary. Every run goes through [`TestEnv`],
//! which gives the binary its own empty config and data directories and removes
//! `ARANET_*` variables inherited from your shell. The tests therefore never read
//! or write your real config or database, and behave the same on every machine.
//!
//! Tests that need Bluetooth are `#[ignore]`d. A non-ignored test that takes more
//! than [`COMMAND_TIMEOUT`] fails, because that almost always means it is scanning.
//!
//! Run the hermetic tests:
//! ```text
//! cargo test --package aranet-cli --test cli_integration
//! ```
//!
//! Run the hardware tests one at a time: they all scan for and connect to the
//! same sensor, and in parallel they fail with connection errors and timeouts.
//! `ARANET_DEVICE` must be an address or device name: aliases from your own
//! config are not visible to the tests.
//! ```text
//! ARANET_DEVICE="Aranet4 12345" cargo test --package aranet-cli --test cli_integration -- --ignored --test-threads=1 --nocapture
//! ```

use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use aranet_store::Store;
use aranet_types::HistoryRecord;
use tempfile::TempDir;
use time::OffsetDateTime;

// =============================================================================
// Test environment
// =============================================================================

/// Longest a hermetic `aranet` run may take. None of them touch Bluetooth, and a
/// device lookup scans for up to 90 s, so hitting this means a test is scanning.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);

/// Longest an `#[ignore]`d hardware run may take (history downloads are slow).
const HARDWARE_TIMEOUT: Duration = Duration::from_secs(300);

/// Inherited variables, other than `ARANET_*`, that change what `aranet` prints
/// (the log filter, and colour settings that `aranet` and clap's `anstream` read).
const REMOVED_VARS: &[&str] = &["RUST_LOG", "NO_COLOR", "CLICOLOR", "CLICOLOR_FORCE"];

/// An isolated config directory, data directory and home directory for one test.
///
/// This is the only place that starts the `aranet` binary
/// (see `test_binary_is_only_started_by_test_env`).
struct TestEnv {
    root: TempDir,
    config_dir: PathBuf,
    data_dir: PathBuf,
    timeout: Duration,
}

impl TestEnv {
    /// Environment for tests that must not touch Bluetooth.
    fn new() -> Self {
        Self::with_timeout(COMMAND_TIMEOUT)
    }

    /// Environment for `#[ignore]`d tests that talk to a real device.
    fn for_hardware() -> Self {
        Self::with_timeout(HARDWARE_TIMEOUT)
    }

    fn with_timeout(timeout: Duration) -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let config_dir = root.path().join("config").join("aranet");
        let data_dir = root.path().join("data").join("aranet");
        fs::create_dir_all(&config_dir).expect("config dir");
        fs::create_dir_all(&data_dir).expect("data dir");
        fs::create_dir_all(root.path().join("home")).expect("home dir");
        Self {
            root,
            config_dir,
            data_dir,
            timeout,
        }
    }

    fn config_path(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    fn db_path(&self) -> PathBuf {
        self.data_dir.join("data.db")
    }

    fn write_config(&self, contents: &str) {
        fs::write(self.config_path(), contents).expect("write config");
    }

    /// Build an `aranet` command that can only see this environment.
    fn command(&self, args: &[&str]) -> Command {
        let home = self.root.path().join("home");
        let mut command = Command::new(env!("CARGO_BIN_EXE_aranet"));
        command.args(args);

        // ARANET_DEVICE (read by clap), ARANET_STYLE and any future ARANET_*
        // setting in the developer's shell must not leak into the tests.
        // Compare in upper case: Windows looks variables up case-insensitively.
        for (key, _) in env::vars_os() {
            if key
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("ARANET_")
            {
                command.env_remove(&key);
            }
        }
        for key in REMOVED_VARS {
            command.env_remove(key);
        }

        command
            .env("ARANET_CONFIG_DIR", &self.config_dir)
            .env("ARANET_DATA_DIR", &self.data_dir)
            // Anything that asks `dirs` directly lands in the temp dir too. This
            // covers macOS and Linux; on Windows `dirs` ignores the environment,
            // so the two ARANET_* variables above are what isolate the tests.
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_DATA_HOME", home.join(".local").join("share"))
            // Never a terminal, so `aranet` can't fall back to an interactive scan.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// Run `aranet` in this environment, failing the test if it runs too long.
    fn run(&self, args: &[&str]) -> Output {
        let mut child = self.command(args).spawn().expect("failed to start aranet");
        let stdout = read_in_background(child.stdout.take().expect("stdout is piped"));
        let stderr = read_in_background(child.stderr.take().expect("stderr is piped"));

        let deadline = Instant::now() + self.timeout;
        let status = loop {
            if let Some(status) = child.try_wait().expect("failed to poll aranet") {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "`aranet {}` did not exit within {:?}. If this test needs Bluetooth, \
                     mark it #[ignore = \"requires BLE hardware\"] and use TestEnv::for_hardware().",
                    args.join(" "),
                    self.timeout
                );
            }
            thread::sleep(Duration::from_millis(10));
        };

        Output {
            status,
            stdout: stdout.join().expect("stdout reader"),
            stderr: stderr.join().expect("stderr reader"),
        }
    }
}

/// Drain a pipe on its own thread so a chatty child can't block on a full pipe.
fn read_in_background(mut pipe: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        buf
    })
}

fn seed_history_database(path: &Path, device_id: &str, alias: Option<&str>) {
    let store = Store::open(path).expect("open store");
    store
        .upsert_device(device_id, alias)
        .expect("upsert device");

    let now = OffsetDateTime::now_utc();
    let records = vec![
        HistoryRecord {
            timestamp: now - time::Duration::minutes(90),
            co2: 810,
            temperature: 21.4,
            pressure: 1012.4,
            humidity: 43,
            ..Default::default()
        },
        HistoryRecord {
            timestamp: now - time::Duration::minutes(30),
            co2: 920,
            temperature: 22.1,
            pressure: 1013.1,
            humidity: 46,
            ..Default::default()
        },
    ];
    store
        .insert_history(device_id, &records)
        .expect("insert history");
}

/// Device for the hardware tests, from the test process's own environment.
fn get_device() -> Option<String> {
    env::var("ARANET_DEVICE").ok().filter(|s| !s.is_empty())
}

// =============================================================================
// Guard
// =============================================================================

/// Every `aranet` run must go through `TestEnv::command`. A second spawn site in
/// this file, or any other test file that starts the binary, bypasses the isolation.
#[test]
fn test_binary_is_only_started_by_test_env() {
    // `concat!` keeps these patterns from matching this test's own source.
    let spawn = concat!("Command", "::new(");
    let binary = concat!("CARGO_BIN_EXE", "_aranet");

    let this_file = include_str!("cli_integration.rs");
    assert_eq!(
        this_file.matches(spawn).count(),
        1,
        "only TestEnv::command may create a Command; use TestEnv::run"
    );
    assert_eq!(
        this_file.matches(binary).count(),
        1,
        "only TestEnv::command may refer to the aranet binary"
    );

    let tests_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    for entry in fs::read_dir(&tests_dir).expect("read tests dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_some_and(|ext| ext == "rs")
            && path
                .file_name()
                .is_some_and(|name| name != "cli_integration.rs")
        {
            let source = fs::read_to_string(&path).expect("read test file");
            assert!(
                !source.contains(binary),
                "{} starts the aranet binary directly; move TestEnv into tests/common \
                 and use it there",
                path.display()
            );
        }
    }
}

// =============================================================================
// Help and Version Tests (no hardware required)
// =============================================================================

#[test]
fn test_help_command() {
    let env = TestEnv::new();
    let output = env.run(&["--help"]);

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
    let env = TestEnv::new();
    let output = env.run(&["--version"]);

    assert!(output.status.success(), "Version should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim(),
        format!("aranet {}", env!("CARGO_PKG_VERSION")),
        "Version should be the crate version"
    );
}

#[test]
fn test_subcommand_help() {
    let env = TestEnv::new();
    let subcommands = [
        "scan", "read", "watch", "history", "info", "status", "sync", "cache", "doctor",
    ];

    for cmd in subcommands {
        let output = env.run(&[cmd, "--help"]);

        assert!(output.status.success(), "{} --help should succeed", cmd);

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.is_empty(), "{} --help should produce output", cmd);
    }
}

// =============================================================================
// Doctor Command (checks the Bluetooth adapter and scans for 3 s)
// =============================================================================

#[test]
#[ignore = "requires BLE hardware"]
fn test_doctor_runs() {
    let env = TestEnv::for_hardware();
    let output = env.run(&["doctor"]);

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
    let env = TestEnv::new();
    let output = env.run(&["config", "path"]);

    assert!(output.status.success(), "Config path should succeed");

    // Also proves the binary honours ARANET_CONFIG_DIR, which every other test relies on.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), env.config_path().display().to_string());
}

#[test]
fn test_config_show() {
    let env = TestEnv::new();
    let output = env.run(&["config", "show"]);

    assert!(
        output.status.success(),
        "config show without a config file should print the defaults: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let shown: toml::Table = toml::from_str(&stdout).expect("config show should print TOML");
    assert!(shown.get("device").is_none(), "no default device: {stdout}");
    assert!(
        shown.get("last_device").is_none(),
        "no last device: {stdout}"
    );
}

#[test]
fn test_config_show_fails_on_invalid_config() {
    let env = TestEnv::new();
    env.write_config("device = [");

    let output = env.run(&["config", "show"]);
    assert!(!output.status.success(), "Invalid config should fail");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Failed to parse config file"),
        "stderr should mention parse failure: {stderr}"
    );
}

/// The permission bits of `path` in octal, such as "600".
#[cfg(unix)]
fn mode_of(path: &Path) -> String {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path).expect("metadata").permissions().mode() & 0o777;
    format!("{mode:o}")
}

#[test]
fn test_config_set_keeps_unknown_keys() {
    let env = TestEnv::new();
    env.write_config("# keep\nfuture_option = 1\nformat = \"text\"\n");

    let output = env.run(&["config", "set", "format", "json"]);
    assert!(
        output.status.success(),
        "config set should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Set Format = json\n"
    );

    assert_eq!(
        fs::read_to_string(env.config_path()).expect("read config"),
        "# keep\nfuture_option = 1\nformat = \"json\"\n"
    );
}

#[test]
fn test_config_init_writes_owner_only_commented_template() {
    let env = TestEnv::new();

    let output = env.run(&["config", "init"]);
    assert!(
        output.status.success(),
        "config init should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let text = fs::read_to_string(env.config_path()).expect("read config");
    // Only the empty [section] headers are active, so the file loads as the defaults.
    let active: toml::Table = toml::from_str(&text).expect("config init should write TOML");
    let mut sections: Vec<&str> = active.keys().map(String::as_str).collect();
    sections.sort_unstable();
    assert_eq!(
        sections,
        ["aliases", "behavior", "gui"],
        "only the [section] headers should be active:\n{text}"
    );
    assert!(
        active
            .values()
            .all(|section| section.as_table().is_some_and(toml::Table::is_empty)),
        "every setting should be commented out:\n{text}"
    );
    // Uncommenting every setting below the header gives exactly the defaults.
    let uncommented: String = text
        .lines()
        .skip_while(|line| !line.is_empty())
        .map(|line| format!("{}\n", line.strip_prefix("# ").unwrap_or(line)))
        .collect();
    let uncommented: toml::Table =
        toml::from_str(&uncommented).expect("the uncommented template should be TOML");
    let defaults =
        toml::Table::try_from(aranet_cli::config::Config::default()).expect("serialize defaults");
    assert_eq!(uncommented, defaults, "{text}");

    #[cfg(unix)]
    assert_eq!(mode_of(&env.config_path()), "600");

    // Uncommenting one line is enough: the setting lands in its section and takes effect.
    let edited = text.replacen("\n# theme = \"dark\"\n", "\ntheme = \"light\"\n", 1);
    assert_ne!(
        edited, text,
        "the template should list `# theme = \"dark\"`"
    );
    env.write_config(&edited);
    let output = env.run(&["config", "show"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "config show should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let shown: toml::Table = toml::from_str(&stdout).expect("config show should print TOML");
    let theme = shown
        .get("gui")
        .and_then(|gui| gui.get("theme"))
        .and_then(toml::Value::as_str);
    assert_eq!(theme, Some("light"), "{edited}");
}

/// The comment lines of a config file's text, in order.
fn comment_lines(text: &str) -> Vec<&str> {
    text.lines().filter(|line| line.starts_with('#')).collect()
}

#[test]
fn test_config_init_template_takes_updates_under_its_headers() {
    let env = TestEnv::new();

    let output = env.run(&["config", "init"]);
    assert!(
        output.status.success(),
        "config init should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let template = fs::read_to_string(env.config_path()).expect("read config");

    // A section key and a root-level key, written by the updater into the template.
    for args in [
        ["alias", "set", "office", "AA:BB:CC:DD:EE:FF"],
        ["config", "set", "fahrenheit", "true"],
    ] {
        let output = env.run(&args);
        assert!(
            output.status.success(),
            "`aranet {}` should succeed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let text = fs::read_to_string(env.config_path()).expect("read config");
    let written: toml::Table = toml::from_str(&text).expect("config should stay valid TOML");

    // The alias went under the template's own `[aliases]` header: every header is there
    // once, in its place, and nothing else became active. Which line the root-level key
    // is on is left open: today it is the first one, above the title, and that may change.
    let fahrenheit = "fahrenheit = true";
    let mut active: Vec<&str> = text
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    assert_eq!(
        active.iter().filter(|line| **line == fahrenheit).count(),
        1,
        "{text}"
    );
    active.retain(|line| *line != fahrenheit);
    assert_eq!(
        active,
        [
            "[aliases]",
            "office = \"AA:BB:CC:DD:EE:FF\"",
            "[behavior]",
            "[gui]"
        ],
        "{text}"
    );
    let aliases = written["aliases"].as_table().expect("[aliases] table");
    assert_eq!(aliases.len(), 1, "{text}");
    assert_eq!(
        aliases.get("office").and_then(toml::Value::as_str),
        Some("AA:BB:CC:DD:EE:FF")
    );

    // The template's comments are all kept, in order. Among them are the title and the
    // commented default of the setting that is now live.
    for comment in [
        "# aranet CLI configuration. Uncomment a setting to change it.",
        "# fahrenheit = false",
    ] {
        assert!(
            text.lines().any(|line| line == comment),
            "{comment:?} is gone:\n{text}"
        );
    }
    assert_eq!(comment_lines(&text), comment_lines(&template), "{text}");

    let output = env.run(&["config", "get", "fahrenheit"]);
    assert!(
        output.status.success(),
        "config get should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "true\n");
}

#[test]
fn test_config_show_redacts_service_api_key() {
    let env = TestEnv::new();
    let config = "[gui]\nservice_api_key = \"s3cret-key\"\n";
    env.write_config(config);

    let output = env.run(&["config", "show"]);
    assert!(
        output.status.success(),
        "config show should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("s3cret-key"),
        "the key was printed:\n{stdout}"
    );
    let shown: toml::Table = toml::from_str(&stdout).expect("config show should print TOML");
    let key = shown
        .get("gui")
        .and_then(|gui| gui.get("service_api_key"))
        .and_then(toml::Value::as_str);
    assert_eq!(key, Some("<redacted>"), "{stdout}");

    // Showing the config never changes the file.
    assert_eq!(
        fs::read_to_string(env.config_path()).expect("read config"),
        config
    );
}

// =============================================================================
// Cache Commands (no device required)
// =============================================================================

#[test]
fn test_cache_info() {
    let env = TestEnv::new();
    let output = env.run(&["cache", "info"]);

    assert!(output.status.success(), "Cache info should succeed");

    // Also proves the binary honours ARANET_DATA_DIR.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let expected = format!("Database path: {}", env.db_path().display());
    assert!(
        stdout.contains(&expected),
        "expected {expected:?} in {stdout:?}"
    );
}

#[test]
fn test_cache_devices() {
    let env = TestEnv::new();
    let output = env.run(&["cache", "devices"]);

    assert!(
        output.status.success(),
        "Cache devices should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No devices in cache"), "{stdout}");
}

// =============================================================================
// Alias Commands (no device required)
// =============================================================================

#[test]
fn test_alias_list() {
    let env = TestEnv::new();
    let output = env.run(&["alias", "list"]);

    assert!(
        output.status.success(),
        "Alias list should succeed (even if empty)"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No aliases configured."), "{stdout}");
}

#[test]
fn test_alias_set_preserves_config_comments_and_mode() {
    let env = TestEnv::new();
    env.write_config("# mine\nfahrenheit = true\n");
    // Start from the mode files got before this change, whatever the umask.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(env.config_path(), fs::Permissions::from_mode(0o644))
            .expect("chmod config");
    }

    let output = env.run(&["alias", "set", "office", "AA:BB:CC:DD:EE:FF"]);
    assert!(
        output.status.success(),
        "alias set should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Added alias 'office' → AA:BB:CC:DD:EE:FF\n"
    );

    let text = fs::read_to_string(env.config_path()).expect("read config");
    assert!(text.starts_with("# mine\nfahrenheit = true\n"), "{text}");
    let written: toml::Table = toml::from_str(&text).expect("config should stay valid TOML");
    let mut keys: Vec<&str> = written.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["aliases", "fahrenheit"], "{text}");
    let aliases = written["aliases"].as_table().expect("[aliases] table");
    assert_eq!(aliases.len(), 1, "{text}");
    assert_eq!(
        aliases.get("office").and_then(toml::Value::as_str),
        Some("AA:BB:CC:DD:EE:FF")
    );

    #[cfg(unix)]
    assert_eq!(mode_of(&env.config_path()), "600");
}

#[test]
fn test_alias_set_refuses_unparseable_config() {
    let env = TestEnv::new();
    env.write_config("device = [");

    let output = env.run(&["alias", "set", "office", "AA:BB:CC:DD:EE:FF"]);
    assert!(
        !output.status.success(),
        "alias set should fail on a config file that doesn't parse"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Failed to parse"), "{stderr}");

    assert_eq!(
        fs::read_to_string(env.config_path()).expect("read config"),
        "device = ["
    );
}

// =============================================================================
// Report Commands (no device required, uses cached data)
// =============================================================================

#[test]
fn test_report_json_resolves_aliases() {
    let env = TestEnv::new();
    env.write_config(
        r#"
[aliases]
office = "Aranet4 12345"
"#,
    );
    seed_history_database(&env.db_path(), "Aranet4 12345", Some("Office"));

    let output = env.run(&["report", "--device", "office", "--format", "json"]);
    assert!(
        output.status.success(),
        "report should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("report JSON should be valid");
    let reports = parsed.as_array().expect("report output should be an array");
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0]["device_id"], "Aranet4 12345");
    assert_eq!(reports[0]["record_count"], 2);
    assert!(reports[0]["co2"].is_object());
}

#[test]
fn test_report_uses_default_device_from_config() {
    let env = TestEnv::new();
    env.write_config(
        r#"
device = "Aranet4 12345"
"#,
    );
    seed_history_database(&env.db_path(), "Aranet4 12345", Some("Office"));

    let output = env.run(&["report", "--format", "json"]);
    assert!(
        output.status.success(),
        "report should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("report JSON should be valid");
    let reports = parsed.as_array().expect("report output should be an array");
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0]["device_id"], "Aranet4 12345");
    assert_eq!(reports[0]["record_count"], 2);
}

#[test]
fn test_report_all_outputs_all_cached_devices() {
    let env = TestEnv::new();
    seed_history_database(&env.db_path(), "Aranet4 12345", Some("Office"));
    seed_history_database(&env.db_path(), "Aranet4 67890", Some("Bedroom"));

    let output = env.run(&["report", "--all", "--format", "json"]);
    assert!(
        output.status.success(),
        "report --all should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("report --all JSON should be valid");
    let reports = parsed.as_array().expect("report output should be an array");
    assert_eq!(reports.len(), 2);
}

#[test]
fn test_report_uses_last_device_from_state_file() {
    let env = TestEnv::new();
    fs::write(
        env.data_dir.join("cli-state.toml"),
        "last_device = \"AA:BB:CC:DD:EE:FF\"\nlast_device_name = \"Aranet4 TEST\"\n",
    )
    .expect("write state file");
    // Two cached devices: the database fallback picks the most recently seen one, which can be
    // either, so the "Using last connected device" line is what shows the state file was read.
    seed_history_database(&env.db_path(), "Aranet4 67890", Some("Bedroom"));
    seed_history_database(&env.db_path(), "AA:BB:CC:DD:EE:FF", Some("Office"));

    let output = env.run(&["report", "--format", "json"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "report should succeed: {stderr}");
    assert!(
        stderr.contains("Using last connected device: Aranet4 TEST (AA:BB:CC:DD:EE:FF)"),
        "{stderr}"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("report JSON should be valid");
    let reports = parsed.as_array().expect("report output should be an array");
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0]["device_id"], "AA:BB:CC:DD:EE:FF");
}

#[test]
fn test_sync_all_json_empty_is_machine_readable() {
    let env = TestEnv::new();

    let output = env.run(&["sync", "--all", "--format", "json"]);
    assert!(
        output.status.success(),
        "sync --all --format json should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("sync JSON should be valid");
    assert_eq!(parsed["total_devices"], 0);
    assert_eq!(parsed["successful"], 0);
    assert_eq!(parsed["failed"], 0);
}

// =============================================================================
// Scan Tests (requires BLE but not specific device)
// =============================================================================

#[test]
#[ignore = "requires BLE hardware"]
fn test_scan_text_output() {
    let env = TestEnv::for_hardware();
    let output = env.run(&["scan", "--timeout", "5"]);

    // Scan may find no devices, but should complete
    assert!(output.status.success(), "Scan should complete");
}

#[test]
#[ignore = "requires BLE hardware"]
fn test_scan_json_output() {
    let env = TestEnv::for_hardware();
    let output = env.run(&["scan", "--timeout", "5", "--format", "json"]);

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
    let env = TestEnv::for_hardware();
    let output = env.run(&["scan", "--timeout", "5", "--format", "csv"]);

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

    let env = TestEnv::for_hardware();
    let output = env.run(&["read", "--device", &device]);

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

    let env = TestEnv::for_hardware();
    let output = env.run(&["read", "--device", &device, "--format", "json"]);

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

    let env = TestEnv::for_hardware();
    let output = env.run(&["--compact", "read", "--device", &device, "--format", "json"]);

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

    let env = TestEnv::for_hardware();
    let output = env.run(&["read", "--device", &device, "--format", "csv"]);

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

    let env = TestEnv::for_hardware();
    let output = env.run(&[
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

    let env = TestEnv::for_hardware();
    let output = env.run(&["read", "--device", &device, "--fahrenheit"]);

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

    let env = TestEnv::for_hardware();
    let output = env.run(&["--quiet", "read", "--device", &device, "--format", "json"]);

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

    let env = TestEnv::for_hardware();
    let output = env.run(&["status", "--device", &device]);

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

    let env = TestEnv::for_hardware();
    let output = env.run(&["status", "--device", &device, "--brief"]);

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

    let env = TestEnv::for_hardware();
    let output = env.run(&["info", "--device", &device]);

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

    let env = TestEnv::for_hardware();
    let output = env.run(&["info", "--device", &device, "--format", "json"]);

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

    let env = TestEnv::for_hardware();
    let output = env.run(&["history", "--device", &device, "--count", "5"]);

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

    let env = TestEnv::for_hardware();
    let output = env.run(&[
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

/// `watch` fails at once on an identifier that no search can fix, with the
/// message `read` gives, instead of retrying it forever. An empty identifier is
/// the one such mistake that needs no Bluetooth: aranet-core rejects it before
/// it scans.
#[test]
fn test_watch_fails_at_once_on_an_identifier_mistake() {
    let env = TestEnv::new();
    let output = env.run(&["watch", "--device", "", "--count", "1"]);

    assert!(
        !output.status.success(),
        "watch with an empty identifier should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "Failed to find device: \n\nCause: Invalid configuration: device identifier is empty"
        ),
        "should fail the way read does: {stderr}"
    );
    assert!(!stderr.contains("Retrying"), "should not retry: {stderr}");
}

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

    let env = TestEnv::for_hardware();
    // Watch with 2 readings at 2-second interval
    let output = env.run(&[
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

    let env = TestEnv::for_hardware();
    let output = env.run(&[
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
    let env = TestEnv::new();
    let output = env.run(&["notacommand"]);

    assert!(!output.status.success(), "Invalid subcommand should fail");
}

/// `read` with no device, no configured or remembered device and an empty cache
/// must fail straight away. It used to pick up the developer's last-used device
/// from their real config and scan for it for 90 s.
#[test]
fn test_missing_required_args() {
    let env = TestEnv::new();
    let output = env.run(&["read"]);

    assert!(
        !output.status.success(),
        "read without a device should fail when nothing is configured"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("No device specified"),
        "should explain that no device was given: {stderr}"
    );
}

/// `aranet server --daemon` must check the configuration before it detaches.
/// The detached process's output goes nowhere, so it used to print "started"
/// for a server that had already failed.
#[test]
fn test_server_daemon_reports_invalid_bind() {
    let env = TestEnv::new();
    // The service's default config path ignores ARANET_CONFIG_DIR, so pass
    // --config (the file needn't exist). --database and --no-collector keep a
    // server that does start inside this environment and away from Bluetooth.
    let server_config = env.config_dir.join("server.toml");
    let db_path = env.db_path();
    let output = env.run(&[
        "server",
        "--daemon",
        "--no-collector",
        "--bind",
        "localhost:8080",
        "--config",
        server_config.to_str().expect("UTF-8 temp path"),
        "--database",
        db_path.to_str().expect("UTF-8 temp path"),
    ]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "--daemon accepted an invalid bind address\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("invalid bind address 'localhost:8080'"),
        "should explain what is wrong with the bind address: {stderr}"
    );
    assert!(
        !stdout.contains("started"),
        "nothing was started, but the output says so: {stdout}"
    );
    assert!(!db_path.exists(), "the database was created");
}

/// Before it detaches, `aranet server --daemon` must check the configuration
/// the detached server will get: the file `--config` names, with `--database`
/// applied to it. A mistake in that file is reported, and a value that the
/// command line replaces is not.
#[test]
fn test_server_daemon_checks_config_file_with_overrides() {
    let env = TestEnv::new();
    // The file's broadcast_buffer is invalid, and its empty storage.path is
    // replaced by --database. --bind is a host name, so that a server which
    // is started anyway exits by itself.
    let server_config = env.config_dir.join("server.toml");
    fs::write(
        &server_config,
        "[server]\nbroadcast_buffer = 0\n\n[storage]\npath = \"\"\n",
    )
    .expect("write server.toml");
    let db_path = env.db_path();
    let output = env.run(&[
        "server",
        "--daemon",
        "--no-collector",
        "--bind",
        "localhost:8080",
        "--config",
        server_config.to_str().expect("UTF-8 temp path"),
        "--database",
        db_path.to_str().expect("UTF-8 temp path"),
    ]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "--daemon accepted an invalid configuration\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("Invalid server configuration"),
        "should say that the server configuration is invalid: {stderr}"
    );
    assert!(
        stderr.contains("server.broadcast_buffer"),
        "should report the mistake in the file --config names: {stderr}"
    );
    assert!(
        !stderr.contains("storage.path"),
        "--database replaces storage.path, so the file's value is no error: {stderr}"
    );
    assert!(
        !stdout.contains("started"),
        "nothing was started, but the output says so: {stdout}"
    );
}

#[test]
#[ignore = "requires BLE hardware"]
fn test_invalid_device() {
    let env = TestEnv::for_hardware();
    // -T 2 keeps the three lookup scans to 2 + 4 + 6 s.
    let output = env.run(&["read", "--device", "NonExistentDevice12345", "-T", "2"]);

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

    let env = TestEnv::for_hardware();
    let output_path = env.root.path().join("output.json");

    let output = env.run(&[
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
