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

    /// Like [`Self::with_config`], but the store is a database file at
    /// [`Self::db_path`], as in production: reads go through the read-only
    /// connection pool, and a second connection can lock the file.
    pub(crate) fn file_backed(mut config: Config) -> Self {
        let dir = tempfile::tempdir().expect("create the test directory");
        config.storage.path = dir.path().join("data.db");
        let store =
            aranet_store::Store::open(&config.storage.path).expect("create the test database");
        let state = AppState::with_collector(
            store,
            config,
            dir.path().join("server.toml"),
            CollectorState::with_reader(FakeReader::new()),
        );
        Self { state, dir }
    }

    /// The database file of a [`Self::file_backed`] state: `data.db` in
    /// [`Self::dir`]. Other constructors never create it.
    pub(crate) fn db_path(&self) -> PathBuf {
        self.dir.path().join("data.db")
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
    /// Addresses whose reads fail to connect, each with the identifier that
    /// the "device not found" error names.
    not_found: HashMap<String, String>,
    /// What a successful read of each address returns, if not
    /// `CurrentReading::default()`.
    readings: HashMap<String, CurrentReading>,
    /// Held addresses, each with the number of the `hold` call that holds it.
    held: HashMap<String, u64>,
    holds: u64,
    records: Vec<ReadRecord>,
}

/// A [`DeviceReader`] for tests: no Bluetooth, scripted behaviour per address
/// and a record of every read.
///
/// A read records its start, waits while its address is held, sleeps its
/// latency on the tokio clock, records its end and returns the address's
/// reading (`CurrentReading::default()` unless one was set), or for a failing
/// address `CollectorError::Read(aranet_core::Error::NotConnected)`, or for
/// one set as not found `CollectorError::Connect` with aranet-core's "device
/// not found" error. Latency, failure and the reading are looked up when the
/// read starts, so changing them affects later reads only. Addresses match
/// exactly (case-sensitive).
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

    /// Make reads of `address` that start from now on fail to connect, with
    /// aranet-core's "device not found" error for `identifier`. That error's
    /// text quotes the identifier, as its "did you mean" form quotes the
    /// names of the devices in range.
    pub(crate) fn set_not_found(&self, address: &str, identifier: &str) {
        self.lock()
            .not_found
            .insert(address.to_owned(), identifier.to_owned());
    }

    /// Make reads of `address` that start from now on return `reading`
    /// instead of `CurrentReading::default()`. Only the notification tests
    /// call it, hence the `cfg`: without the feature it would be dead code.
    #[cfg(feature = "notifications")]
    pub(crate) fn set_reading(&self, address: &str, reading: CurrentReading) {
        self.lock().readings.insert(address.to_owned(), reading);
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

    /// Wait until at least `n` reads have started. Panics after one day on
    /// the tokio clock, naming `n` and the reads so far.
    ///
    /// For `start_paused` tests, where a day passes at once when nothing else
    /// is left to run. Without the limit, a collector that stopped polling
    /// would hang the test, and with it the whole test run, instead of
    /// failing it.
    pub(crate) async fn wait_for_reads(&self, n: usize) {
        let waited = tokio::time::timeout(
            Duration::from_secs(24 * 60 * 60),
            self.wait_until(|state| state.records.len() >= n),
        )
        .await;
        if waited.is_err() {
            panic!(
                "waited a day (tokio clock) for {n} reads to start; {} started: {:?}",
                self.reads().len(),
                self.reads()
            );
        }
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
            let (index, hold, latency, failing, not_found, reading) = {
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
                    state.not_found.get(address).cloned(),
                    state.readings.get(address).copied(),
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

            let result = if let Some(identifier) = not_found {
                Err(CollectorError::Connect(
                    aranet_core::Error::device_not_found(identifier),
                ))
            } else if failing {
                Err(CollectorError::Read(aranet_core::Error::NotConnected))
            } else {
                Ok(reading.unwrap_or_default())
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

/// Log output captured on the current thread, for tests that check what the
/// service writes to its logs.
///
/// [`CapturedLogs::start`] installs a thread-local default subscriber that
/// records every event from this crate, at every level, as plain text. On the
/// default `#[tokio::test]` runtime (current thread), tasks the test spawns run
/// on the test's thread, so their events are captured too. Don't use it with
/// the multi-thread flavour.
///
/// Other tests log the same lines on their own threads, with no subscriber.
/// `start` calls [`register_silent_dispatcher`] first, so that none of those
/// threads can hide a line from the capture.
#[derive(Clone)]
pub(crate) struct CapturedLogs {
    buf: Arc<std::sync::Mutex<Vec<u8>>>,
}

impl CapturedLogs {
    /// Start capturing. Capturing stops when the returned guard is dropped.
    ///
    /// The filter is `aranet_service=trace`, not the production
    /// `DEFAULT_LOG_DIRECTIVES`, which logs this crate at `info`, so a secret
    /// in a debug line is caught too.
    pub(crate) fn start() -> (Self, tracing::subscriber::DefaultGuard) {
        register_silent_dispatcher();
        let logs = Self {
            buf: Arc::default(),
        };
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new("aranet_service=trace"))
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        (logs, guard)
    }

    /// Everything captured so far.
    pub(crate) fn text(&self) -> String {
        let buf = self
            .buf
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        String::from_utf8_lossy(&buf).into_owned()
    }
}

impl std::io::Write for CapturedLogs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.buf
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Keep a second, silent dispatcher registered for the rest of the test run.
///
/// Call this before a test installs a thread-local subscriber and checks its
/// output. [`CapturedLogs::start`] does.
///
/// The first time a log statement runs, tracing caches whether any subscriber
/// wants it, for the whole process. While only one dispatcher is registered,
/// it asks just the default subscriber of the thread that got there first. A
/// test thread with no subscriber then caches "never", and a capture on
/// another thread misses the line until the next dispatcher registers. With
/// this second dispatcher, which wants nothing, a line that a capture wants is
/// cached as "sometimes", and every event is checked against its own thread's
/// subscriber. Registering a dispatcher also recomputes every cached answer,
/// so a line cached as "never" before the capture started is fixed too.
pub(crate) fn register_silent_dispatcher() {
    static SILENT: std::sync::OnceLock<tracing::Dispatch> = std::sync::OnceLock::new();
    SILENT.get_or_init(|| tracing::Dispatch::new(tracing::subscriber::NoSubscriber::new()));
}

/// How a [`RecordingServer`] route answers.
pub(crate) enum Reply {
    /// This status code with an empty body, every time.
    Status(u16),
    /// This status code with this response body, every time.
    StatusWithBody(u16, String),
    /// These status codes in turn, one per request, and then the last one
    /// again for every later request.
    Sequence(Vec<u16>),
    /// Never. The request is recorded and then left waiting until the client
    /// gives up or the test ends.
    Hang,
    /// This redirect status code with this `Location` header and an empty
    /// body, every time.
    Redirect(u16, String),
    /// This status code, and then a response body that never ends: 1 KiB of
    /// `x` every 10 ms, until the client closes the connection.
    EndlessBody(u16),
}

/// One request that a [`RecordingServer`] route received.
#[derive(Debug, Clone)]
pub(crate) struct RecordedRequest {
    /// The request path, without the query string.
    pub(crate) path: String,
    /// The request body.
    pub(crate) body: String,
}

/// A loopback HTTP server that stands in for a webhook, InfluxDB or push
/// gateway endpoint.
///
/// Each route answers `POST` as its [`Reply`] says and records the request.
/// Any other path gets 404 and is not recorded. The server stops when the
/// value is dropped.
pub(crate) struct RecordingServer {
    addr: std::net::SocketAddr,
    requests: Arc<std::sync::Mutex<Vec<RecordedRequest>>>,
    notify: Arc<tokio::sync::Notify>,
    in_flight: Arc<InFlight>,
    task: tokio::task::JoinHandle<()>,
}

/// Requests a [`RecordingServer`] is answering right now, and the most it
/// was ever answering at once.
#[derive(Default)]
struct InFlight {
    now: std::sync::atomic::AtomicUsize,
    max: std::sync::atomic::AtomicUsize,
}

impl InFlight {
    /// Count a request until the returned guard is dropped: when the handler
    /// returns, or when a hanging handler is dropped because its client went
    /// away.
    fn enter(this: &Arc<Self>) -> InFlightGuard {
        use std::sync::atomic::Ordering::SeqCst;
        let now = this.now.fetch_add(1, SeqCst) + 1;
        this.max.fetch_max(now, SeqCst);
        InFlightGuard(Arc::clone(this))
    }
}

struct InFlightGuard(Arc<InFlight>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.now.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// What a [`RecordingServer`] route sends with its status code.
#[derive(Clone)]
enum Extra {
    Nothing,
    /// This response body.
    Body(String),
    /// This `Location` header.
    Location(String),
    /// [`endless_body`].
    EndlessBody,
}

/// A response body that never ends: 1 KiB of `x` every 10 ms. The server
/// drops it when the client closes the connection.
fn endless_body()
-> impl futures::Stream<Item = Result<Vec<u8>, std::convert::Infallible>> + Send + 'static {
    futures::stream::unfold((), |()| async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        Some((Ok(vec![b'x'; 1024]), ()))
    })
}

impl RecordingServer {
    /// Serve `routes`, each a path and its reply, on `127.0.0.1:0`.
    pub(crate) async fn start(routes: Vec<(&'static str, Reply)>) -> Self {
        use axum::response::IntoResponse;

        let requests: Arc<std::sync::Mutex<Vec<RecordedRequest>>> = Arc::default();
        let notify = Arc::new(tokio::sync::Notify::new());
        let in_flight: Arc<InFlight> = Arc::default();

        let mut router: axum::Router = axum::Router::new();
        for (path, reply) in routes {
            let status =
                |code| axum::http::StatusCode::from_u16(code).expect("a valid status code");
            // What every answer carries besides its status code.
            let extra = match &reply {
                Reply::StatusWithBody(_, body) => Extra::Body(body.clone()),
                Reply::Redirect(_, location) => Extra::Location(location.clone()),
                Reply::EndlessBody(_) => Extra::EndlessBody,
                _ => Extra::Nothing,
            };
            // The answer to each request in turn, the last one repeating for
            // every later request. `None` leaves the request waiting.
            let script: Arc<Vec<Option<axum::http::StatusCode>>> = Arc::new(match reply {
                Reply::Status(code)
                | Reply::StatusWithBody(code, _)
                | Reply::Redirect(code, _)
                | Reply::EndlessBody(code) => vec![Some(status(code))],
                Reply::Sequence(codes) => {
                    assert!(!codes.is_empty(), "a Reply::Sequence needs a status");
                    codes.into_iter().map(|code| Some(status(code))).collect()
                }
                Reply::Hang => vec![None],
            });
            let served = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let requests = Arc::clone(&requests);
            let notify = Arc::clone(&notify);
            let in_flight = Arc::clone(&in_flight);
            let handler = move |uri: axum::http::Uri, body: String| async move {
                let _in_flight = InFlight::enter(&in_flight);
                let turn = served.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                requests
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(RecordedRequest {
                        path: uri.path().to_owned(),
                        body,
                    });
                notify.notify_waiters();
                match (script[turn.min(script.len() - 1)], extra) {
                    (None, _) => std::future::pending().await,
                    (Some(status), Extra::Nothing) => status.into_response(),
                    (Some(status), Extra::Body(text)) => (status, text).into_response(),
                    (Some(status), Extra::Location(location)) => {
                        (status, [(axum::http::header::LOCATION, location)]).into_response()
                    }
                    (Some(status), Extra::EndlessBody) => {
                        (status, axum::body::Body::from_stream(endless_body())).into_response()
                    }
                }
            };
            router = router.route(path, axum::routing::post(handler));
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a loopback port");
        let addr = listener.local_addr().expect("read the bound address");
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("the recording server failed");
        });
        Self {
            addr,
            requests,
            notify,
            in_flight,
            task,
        }
    }

    /// The URL of `path` on this server: `http://127.0.0.1:<port><path>`.
    pub(crate) fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    /// The requests received so far, oldest first.
    pub(crate) fn requests(&self) -> Vec<RecordedRequest> {
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The most requests this server was ever answering at the same time.
    pub(crate) fn max_in_flight(&self) -> usize {
        self.in_flight.max.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Wait until at least `n` requests have arrived, and return all of them.
    /// Panics after 5 s.
    pub(crate) async fn wait_for_requests(&self, n: usize) -> Vec<RecordedRequest> {
        let arrived = async {
            loop {
                // Created before the check, so a request recorded between the
                // check and the `.await` still wakes this loop.
                let notified = self.notify.notified();
                let requests = self.requests();
                if requests.len() >= n {
                    return requests;
                }
                notified.await;
            }
        };
        tokio::time::timeout(Duration::from_secs(5), arrived)
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "timed out waiting for {n} requests; received {:?}",
                    self.requests()
                )
            })
    }
}

impl Drop for RecordingServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A loopback URL that refuses connections: `http://127.0.0.1:<port><path>`
/// for a port that was just bound and released again.
pub(crate) async fn closed_port_url(path: &str) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback port");
    let port = listener
        .local_addr()
        .expect("read the bound address")
        .port();
    drop(listener);
    format!("http://127.0.0.1:{port}{path}")
}

/// Check `condition` every 10 ms until it holds. Panics after 5 s, naming
/// `what`.
///
/// For tests on real time, where the thing to wait for has no event of its
/// own, such as a sink subscribing to the readings channel.
pub(crate) async fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out after 5 s waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
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

    #[test]
    fn captured_logs_record_every_level_as_plain_text() {
        let (logs, guard) = CapturedLogs::start();
        tracing::trace!("a trace line");
        tracing::info!(attempt = 2, "an info line");
        drop(guard);
        tracing::info!("a line after the guard");

        let text = logs.text();
        assert!(text.contains("TRACE"), "{text}");
        assert!(text.contains("a trace line"), "{text}");
        assert!(text.contains("an info line attempt=2"), "{text}");
        assert!(!text.contains('\u{1b}'), "ANSI colours in {text:?}");
        assert!(!text.contains("after the guard"), "{text}");
    }

    #[tokio::test]
    async fn recording_server_records_posts_and_answers_as_scripted() {
        let server = RecordingServer::start(vec![
            ("/ok", Reply::Status(204)),
            ("/fail", Reply::Status(500)),
        ])
        .await;
        let urls = [
            server.url("/ok"),
            server.url("/fail?x=1"),
            server.url("/unknown"),
        ];
        // Built before the sender starts: building a client reads the system's
        // certificate store, which can take long enough on a busy machine to
        // use up `wait_for_requests`' 5 s.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        // Send from another task, so `wait_for_requests` really waits.
        let sender = tokio::spawn(async move {
            let mut statuses = Vec::new();
            for url in urls {
                let response = client
                    .post(url.as_str())
                    .body(format!("body for {url}"))
                    .send()
                    .await
                    .expect("the server answers");
                statuses.push(response.status().as_u16());
            }
            statuses
        });

        let requests = server.wait_for_requests(2).await;
        assert_eq!(sender.await.unwrap(), [204, 500, 404]);

        let paths: Vec<&str> = requests.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, ["/ok", "/fail"], "paths exclude the query");
        assert_eq!(
            requests[1].body,
            format!("body for {}", server.url("/fail?x=1"))
        );
        assert_eq!(server.requests().len(), 2, "unknown paths aren't recorded");
    }

    #[tokio::test]
    async fn closed_port_url_refuses_connections() {
        let url = closed_port_url("/x").await;
        assert!(
            url.starts_with("http://127.0.0.1:") && url.ends_with("/x"),
            "{url}"
        );

        let error = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap()
            .post(&url)
            .send()
            .await
            .expect_err("nothing listens on the port");
        assert!(error.is_connect(), "{error:?}");
    }

    #[test]
    fn captured_logs_see_lines_another_thread_reached_first() {
        /// A log statement that only this test reaches.
        fn log_line(n: u32) {
            tracing::warn!("line {n} from a shared log statement");
        }

        let (logs, _guard) = CapturedLogs::start();
        // A thread without a subscriber, such as another test's, reaches the
        // statement first.
        std::thread::spawn(|| log_line(1)).join().unwrap();
        log_line(2);

        let text = logs.text();
        assert!(
            text.contains("line 2 from a shared log statement"),
            "{text}"
        );
        assert!(!text.contains("line 1"), "{text}");
    }
}
