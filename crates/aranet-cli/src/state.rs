//! State the CLI records for itself, kept out of the user's `config.toml`.
//!
//! The state file sits next to the database in [`aranet_store::default_data_dir`], so
//! `ARANET_DATA_DIR` moves both.

use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Machine-written CLI state, stored as `cli-state.toml` in the data directory.
///
/// Connecting to a sensor updates this file, so `config.toml` changes only when the user
/// changes a setting.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct CliState {
    /// Identifier (address or UUID) of the last device the CLI connected to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_device: Option<String>,

    /// Name of that device, for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_device_name: Option<String>,
}

impl CliState {
    /// The state file: `cli-state.toml` in [`aranet_store::default_data_dir`].
    pub fn path() -> PathBuf {
        aranet_store::default_data_dir().join("cli-state.toml")
    }

    /// Load the state from [`CliState::path`].
    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    /// Load the state from `path`.
    ///
    /// The state is only a convenience: a missing, unreadable or corrupt file gives the
    /// default (empty) state, and the reason is logged at debug level.
    pub fn load_from(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) => {
                if err.kind() != io::ErrorKind::NotFound {
                    tracing::debug!("Ignoring CLI state file {}: {err}", path.display());
                }
                return Self::default();
            }
        };
        toml::from_str(&text).unwrap_or_else(|err| {
            tracing::debug!("Ignoring corrupt CLI state file {}: {err}", path.display());
            Self::default()
        })
    }

    /// Record a successful connection to `identifier` (named `name`) in the state file at `path`.
    ///
    /// The file is written, atomically and owner-only, only when the recorded device changes.
    pub fn record_connection_at(path: &Path, identifier: &str, name: Option<&str>) -> Result<()> {
        let new = Self {
            last_device: Some(identifier.to_string()),
            last_device_name: name.map(str::to_string),
        };
        if Self::load_from(path) == new {
            return Ok(());
        }
        let text = toml::to_string(&new).context("Failed to serialize CLI state")?;
        aranet_store::write_private(path, text.as_bytes())
            .with_context(|| format!("Failed to write CLI state file: {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn record_connection_writes_0600_state_and_skips_unchanged() {
        let dir = tempdir().unwrap();
        // The data directory doesn't exist before the first connection.
        let path = dir.path().join("aranet").join("cli-state.toml");

        CliState::record_connection_at(&path, "AA:BB:CC:DD:EE:FF", Some("Aranet4 TEST")).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "last_device = \"AA:BB:CC:DD:EE:FF\"\nlast_device_name = \"Aranet4 TEST\"\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(format!("{mode:o}"), "600");
        }

        // The same device again leaves the file alone.
        let before = fs::metadata(&path).unwrap();
        CliState::record_connection_at(&path, "AA:BB:CC:DD:EE:FF", Some("Aranet4 TEST")).unwrap();
        let after = fs::metadata(&path).unwrap();
        assert_eq!(after.modified().unwrap(), before.modified().unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(after.ino(), before.ino());
        }

        // Another device replaces both fields; its missing name isn't inherited.
        CliState::record_connection_at(&path, "11:22:33:44:55:66", None).unwrap();
        assert_eq!(
            CliState::load_from(&path),
            CliState {
                last_device: Some("11:22:33:44:55:66".to_string()),
                last_device_name: None,
            }
        );
    }

    #[test]
    fn load_from_tolerates_missing_and_corrupt() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cli-state.toml");
        assert_eq!(CliState::load_from(&path), CliState::default());

        let corrupt: [&[u8]; 3] = [b"last_device = [", b"last_device = 5", b"\xff\xfe"];
        for contents in corrupt {
            fs::write(&path, contents).unwrap();
            assert_eq!(
                CliState::load_from(&path),
                CliState::default(),
                "{contents:?}"
            );
        }

        // Keys a later version adds don't stop this one from reading the file.
        fs::write(&path, "last_device = \"AA:BB:CC:DD:EE:FF\"\nfuture = 1\n").unwrap();
        assert_eq!(
            CliState::load_from(&path).last_device.as_deref(),
            Some("AA:BB:CC:DD:EE:FF")
        );
    }

    #[test]
    fn record_connection_never_touches_config() {
        // ARANET_CONFIG_DIR and ARANET_DATA_DIR can name the same directory.
        let dir = tempdir().unwrap();
        let config = dir.path().join("config.toml");
        fs::write(&config, "# mine\nfahrenheit = true\n").unwrap();
        let before = fs::metadata(&config).unwrap();

        CliState::record_connection_at(
            &dir.path().join("cli-state.toml"),
            "AA:BB:CC:DD:EE:FF",
            Some("Aranet4 TEST"),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(&config).unwrap(),
            "# mine\nfahrenheit = true\n"
        );
        let after = fs::metadata(&config).unwrap();
        assert_eq!(after.modified().unwrap(), before.modified().unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(after.ino(), before.ino());
        }
        let mut names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(names, ["cli-state.toml", "config.toml"]);
    }
}
