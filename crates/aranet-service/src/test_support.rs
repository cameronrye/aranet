//! Fixtures shared by this crate's unit tests.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use aranet_types::CurrentReading;
use futures::future::BoxFuture;
use tokio::sync::Notify;
use tokio::time::Instant;

use crate::collector::{CollectorError, DeviceReader};
use crate::config::Config;
use crate::state::{AppState, CollectorState};

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
    /// exist until a handler saves the config. The collector reads devices
    /// through a new [`FakeReader`], so a test that starts it with devices
    /// configured never reaches Bluetooth.
    pub(crate) fn with_config(config: Config) -> Self {
        Self::with_reader(config, FakeReader::new())
    }

    /// Like [`Self::with_config`], but the collector reads devices through
    /// `reader`, normally a [`FakeReader`] the test keeps to script and check
    /// the reads.
    pub(crate) fn with_reader(config: Config, reader: Arc<dyn DeviceReader>) -> Self {
        Self::with_collector(config, CollectorState::with_reader(reader))
    }

    /// Like [`Self::with_config`], with a prepared collector state. Build it
    /// with `CollectorState::with_reader` if the test starts the collector:
    /// `CollectorState::new()` reads devices over Bluetooth.
    pub(crate) fn with_collector(mut config: Config, collector: CollectorState) -> Self {
        let dir = tempfile::tempdir().expect("create the test directory");
        config.storage.path = dir.path().join("data.db");
        let store = aranet_store::Store::open_in_memory().expect("open an in-memory store");
        let state =
            AppState::with_collector(store, config, dir.path().join("server.toml"), collector);
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

/// Let every task that is ready to run, and every running `spawn_blocking`
/// job, run until it waits on something else.
///
/// Only for `#[tokio::test(start_paused = true)]` tests. It sleeps 1 ms of
/// paused time, and the paused clock moves only once every task is idle and
/// no `spawn_blocking` job is running. Timers due within that 1 ms fire too.
pub(crate) async fn settle() {
    tokio::time::sleep(Duration::from_millis(1)).await;
}

/// One `read_current` call on a [`FakeReader`].
#[derive(Debug, Clone)]
pub(crate) struct ReadRecord {
    /// The address exactly as the collector passed it.
    pub(crate) address: String,
    /// When the read started (tokio clock, so paused-clock tests can compare it).
    pub(crate) started: Instant,
    /// When the read returned. `None` while it is held or sleeping, and for a
    /// read whose future was dropped (timed out or cancelled).
    pub(crate) finished: Option<Instant>,
    /// Whether the read returned a reading (`false` until it finishes).
    pub(crate) ok: bool,
}

#[derive(Default)]
struct FakeReaderState {
    latency: HashMap<String, Duration>,
    failing: HashSet<String>,
    /// Held addresses, each with the number of the `hold` call that holds it.
    held: HashMap<String, u64>,
    holds: u64,
    records: Vec<ReadRecord>,
}

/// A [`DeviceReader`] for tests: no Bluetooth, scripted behaviour per address
/// and a record of every read.
///
/// A read records its start, waits while its address is held, sleeps its
/// latency on the tokio clock, records its end and returns
/// `CurrentReading::default()`, or for a failing address
/// `CollectorError::Read(aranet_core::Error::NotConnected)`. Latency and
/// failure are looked up when the read starts, so changing them affects later
/// reads only. Addresses match exactly (case-sensitive).
pub(crate) struct FakeReader {
    state: Mutex<FakeReaderState>,
    /// Woken (`notify_waiters`) when a read starts or finishes and on release.
    changed: Notify,
}

impl FakeReader {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(FakeReaderState::default()),
            changed: Notify::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, FakeReaderState> {
        // A failed assertion in one task must not turn every later call into a PoisonError.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Make reads of `address` that start from now on take `latency` (tokio clock).
    pub(crate) fn set_latency(&self, address: &str, latency: Duration) {
        self.lock().latency.insert(address.to_owned(), latency);
    }

    /// Make reads of `address` that start from now on fail, or succeed again.
    pub(crate) fn set_failing(&self, address: &str, failing: bool) {
        let mut state = self.lock();
        if failing {
            state.failing.insert(address.to_owned());
        } else {
            state.failing.remove(address);
        }
    }

    /// Hold reads of `address` that start from now on: each records its start
    /// and then waits for [`Self::release`].
    pub(crate) fn hold(&self, address: &str) {
        let mut state = self.lock();
        state.holds += 1;
        let hold = state.holds;
        state.held.entry(address.to_owned()).or_insert(hold);
    }

    /// Let the held reads of `address`, and all later ones, continue.
    pub(crate) fn release(&self, address: &str) {
        self.lock().held.remove(address);
        self.changed.notify_waiters();
    }

    /// Every read so far, in the order they started.
    pub(crate) fn reads(&self) -> Vec<ReadRecord> {
        self.lock().records.clone()
    }

    /// The reads of `address`, in the order they started.
    pub(crate) fn reads_of(&self, address: &str) -> Vec<ReadRecord> {
        self.reads()
            .into_iter()
            .filter(|record| record.address == address)
            .collect()
    }

    /// Wait until at least `n` reads have started.
    pub(crate) async fn wait_for_reads(&self, n: usize) {
        self.wait_until(|state| state.records.len() >= n).await;
    }

    async fn wait_until(&self, done: impl Fn(&FakeReaderState) -> bool) {
        loop {
            let mut notified = std::pin::pin!(self.changed.notified());
            // Register before checking, so a notification sent between the
            // check and the `.await` isn't lost.
            notified.as_mut().enable();
            let satisfied = done(&self.lock());
            if satisfied {
                return;
            }
            notified.await;
        }
    }
}

impl DeviceReader for FakeReader {
    fn read_current<'a>(
        &'a self,
        address: &'a str,
    ) -> BoxFuture<'a, Result<CurrentReading, CollectorError>> {
        Box::pin(async move {
            let (index, hold, latency, failing) = {
                let mut state = self.lock();
                state.records.push(ReadRecord {
                    address: address.to_owned(),
                    started: Instant::now(),
                    finished: None,
                    ok: false,
                });
                (
                    state.records.len() - 1,
                    state.held.get(address).copied(),
                    state.latency.get(address).copied(),
                    state.failing.contains(address),
                )
            };
            self.changed.notify_waiters();

            if let Some(hold) = hold {
                // Released, or released and held again by a later `hold` call.
                self.wait_until(|state| state.held.get(address) != Some(&hold))
                    .await;
            }
            if let Some(latency) = latency {
                tokio::time::sleep(latency).await;
            }

            let result = if failing {
                Err(CollectorError::Read(aranet_core::Error::NotConnected))
            } else {
                Ok(CurrentReading::default())
            };
            {
                let mut state = self.lock();
                let record = &mut state.records[index];
                record.finished = Some(Instant::now());
                record.ok = result.is_ok();
            }
            self.changed.notify_waiters();
            result
        })
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

    #[tokio::test(start_paused = true)]
    async fn fake_reader_records_reads_in_order() {
        let reader = FakeReader::new();
        reader.set_latency("Aranet4 BBBBB", Duration::from_secs(30));

        let slow = tokio::spawn({
            let reader = Arc::clone(&reader);
            async move { reader.read_current("Aranet4 BBBBB").await.is_ok() }
        });
        reader.wait_for_reads(1).await;
        assert!(reader.read_current("Aranet4 AAAAA").await.is_ok());
        assert!(slow.await.unwrap());

        let reads = reader.reads();
        let addresses: Vec<&str> = reads.iter().map(|r| r.address.as_str()).collect();
        assert_eq!(addresses, ["Aranet4 BBBBB", "Aranet4 AAAAA"], "start order");
        assert!(reads.iter().all(|r| r.ok));
        let slow_finished = reads[0].finished.expect("the slow read finished");
        let took = slow_finished - reads[0].started;
        assert!(took >= Duration::from_secs(30) && took < Duration::from_secs(31));
        assert!(reads[1].finished.expect("the fast read finished") < slow_finished);

        assert_eq!(reader.reads_of("Aranet4 AAAAA").len(), 1);
        assert!(
            reader.reads_of("aranet4 aaaaa").is_empty(),
            "addresses match exactly"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn fake_reader_hold_blocks_until_release() {
        let reader = FakeReader::new();
        reader.hold("Aranet4 AAAAA");
        let held = tokio::spawn({
            let reader = Arc::clone(&reader);
            async move { reader.read_current("Aranet4 AAAAA").await.is_ok() }
        });
        settle().await;

        let reads = reader.reads_of("Aranet4 AAAAA");
        assert_eq!(reads.len(), 1);
        assert!(reads[0].finished.is_none(), "a held read waits");
        // Holding is per address.
        assert!(reader.read_current("Aranet4 BBBBB").await.is_ok());

        reader.release("Aranet4 AAAAA");
        settle().await;
        let reads = reader.reads_of("Aranet4 AAAAA");
        assert!(reads[0].finished.is_some());
        assert!(reads[0].ok);
        assert!(held.await.unwrap());

        // A held read whose future is dropped stays unfinished.
        reader.hold("Aranet4 AAAAA");
        let timed_out =
            tokio::time::timeout(Duration::from_secs(1), reader.read_current("Aranet4 AAAAA"))
                .await;
        assert!(timed_out.is_err());
        let reads = reader.reads_of("Aranet4 AAAAA");
        assert_eq!(reads.len(), 2);
        assert!(reads[1].finished.is_none());
        assert!(!reads[1].ok);
    }

    #[tokio::test]
    async fn fake_reader_failing_address_returns_read_error() {
        let reader = FakeReader::new();
        reader.set_failing("Aranet4 AAAAA", true);

        let error = reader.read_current("Aranet4 AAAAA").await.unwrap_err();
        assert!(matches!(
            error,
            CollectorError::Read(aranet_core::Error::NotConnected)
        ));
        assert!(error.to_string().contains("Failed to read"), "{error}");
        let reads = reader.reads_of("Aranet4 AAAAA");
        assert!(reads[0].finished.is_some());
        assert!(!reads[0].ok);

        reader.set_failing("Aranet4 AAAAA", false);
        assert!(reader.read_current("Aranet4 AAAAA").await.is_ok());
    }
}
