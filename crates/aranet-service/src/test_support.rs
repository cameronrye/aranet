//! Fixtures shared by this crate's unit tests.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::Config;
use crate::state::AppState;

/// Application state for one unit test: an in-memory store, and a config file
/// and database path inside a temporary directory that is deleted on drop.
///
/// Keep the value alive for the whole test. Bind it as `t`, `_t` or `_dir`,
/// never as `_`, which drops it at once and deletes the directory.
pub(crate) struct TestState {
    pub(crate) state: Arc<AppState>,
    // Declared last, so dropping the fixture drops `state` before it deletes
    // the directory. That releases the `AppState` only if nothing else (a
    // router, a spawned task) still holds a clone. Windows can't delete a
    // file that is still open, and `TempDir` ignores the error.
    dir: tempfile::TempDir,
}

impl TestState {
    /// The default config and an empty in-memory store.
    pub(crate) fn in_memory() -> Self {
        Self::with_config(Config::default())
    }

    /// `config` and an empty in-memory store. `storage.path` is set to
    /// `data.db` in [`Self::dir`], so a saved config never names the real
    /// database. The config file is `server.toml` in [`Self::dir`]; it doesn't
    /// exist until a handler saves the config.
    pub(crate) fn with_config(mut config: Config) -> Self {
        let dir = tempfile::tempdir().expect("create the test directory");
        config.storage.path = dir.path().join("data.db");
        let store = aranet_store::Store::open_in_memory().expect("open an in-memory store");
        let state = AppState::with_config_path(store, config, dir.path().join("server.toml"));
        Self { state, dir }
    }

    /// Where the state saves its config: `server.toml` in [`Self::dir`].
    pub(crate) fn config_path(&self) -> PathBuf {
        self.dir.path().join("server.toml")
    }

    /// The temporary directory that holds this test's files.
    pub(crate) fn dir(&self) -> &Path {
        self.dir.path()
    }
}

mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn test_state_config_file_is_removed_on_drop() {
        let t = TestState::in_memory();
        let state = Arc::clone(&t.state);

        let response = crate::api::router()
            .with_state(Arc::clone(&t.state))
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/config/devices")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"address":"Aranet4 12345"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(state.config.read().await.devices.len(), 1);

        let config_path = t.config_path();
        assert!(config_path.starts_with(t.dir()));
        assert!(
            config_path.exists(),
            "the handler saves into the test directory"
        );
        let saved = Config::load(&config_path).unwrap();
        assert_eq!(saved.devices[0].address, "Aranet4 12345");
        // If the saved file names a database, it must be the fixture's. (A save
        // that writes only the changed keys names none.)
        let table: toml::Table = std::fs::read_to_string(&config_path)
            .unwrap()
            .parse()
            .unwrap();
        if let Some(path) = table
            .get("storage")
            .and_then(|storage| storage.get("path"))
            .and_then(|path| path.as_str())
        {
            assert!(
                Path::new(path).starts_with(t.dir()),
                "the saved config must not name the real database"
            );
        }

        let dir = t.dir().to_path_buf();
        drop(state);
        drop(t);
        assert!(!dir.exists(), "dropping the fixture deletes its directory");
    }

    #[test]
    fn test_state_never_uses_default_config_path() {
        let t = TestState::in_memory();
        assert!(t.state.config_path.starts_with(t.dir()));
        assert_ne!(t.state.config_path, crate::config::default_config_path());
    }
}
