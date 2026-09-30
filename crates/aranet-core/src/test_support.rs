//! Helpers shared by aranet-core's unit tests.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::FutureExt;
use tokio::time::Instant;

use crate::connector::{ConnectFn, SensorLink};
use crate::error::{Error, Result};

/// Awaits `future`, panicking with "did not finish within {limit:?}" if it takes longer.
///
/// Under `#[tokio::test(start_paused = true)]` the clock jumps ahead whenever
/// every task is waiting, so a future that never finishes reaches `limit` at
/// once and the test fails instead of hanging the suite.
pub(crate) async fn within<F: Future>(limit: Duration, future: F) -> F::Output {
    tokio::time::timeout(limit, future)
        .await
        .unwrap_or_else(|_| panic!("did not finish within {limit:?}"))
}

// ---------------------------------------------------------------------------
// FakeRadio: sensors behind the `SensorLink` seam
// ---------------------------------------------------------------------------

/// What a `FakeRadio` saw, in order, with the paused-clock time since `new()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FakeEvent {
    ConnectStarted {
        id: String,
    },
    Connected {
        id: String,
        handle: u64,
    },
    ConnectFailed {
        id: String,
    },
    Disconnect {
        id: String,
        handle: u64,
    },
    /// A handle was dropped without `disconnect()` and took the link down,
    /// as `Device`'s `Drop` does.
    DropTeardown {
        id: String,
        handle: u64,
    },
    Op {
        id: String,
        handle: u64,
        ok: bool,
    },
}

/// Sensors as BlueZ and (since Phase 1) CoreBluetooth see them: one link per
/// sensor, and a disconnect from any handle, or the `Drop` of a handle that
/// was never disconnected, takes down whichever link is up.
#[derive(Clone)]
pub(crate) struct FakeRadio {
    inner: Arc<Mutex<RadioState>>,
    start: Instant,
}

#[derive(Default)]
struct RadioState {
    sensors: HashMap<String, FakeSensor>,
    events: Vec<(Duration, FakeEvent)>,
    last_handle: u64,
}

#[derive(Default)]
struct FakeSensor {
    up: bool,
    connects: usize,
    /// Outcomes of the next connects; `false` fails. Unscripted connects succeed.
    script: VecDeque<bool>,
}

impl RadioState {
    fn sensor(&mut self, id: &str) -> &mut FakeSensor {
        self.sensors.entry(id.to_owned()).or_default()
    }
}

impl FakeRadio {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::default(),
            start: Instant::now(),
        }
    }

    /// Never held across an `.await`. Survives a poisoned lock so a `FakeConn`
    /// dropped while a test panics doesn't abort the process.
    fn state(&self) -> MutexGuard<'_, RadioState> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn log(&self, state: &mut RadioState, event: FakeEvent) {
        state.events.push((self.start.elapsed(), event));
    }

    /// A `ConnectFn` that connects to this radio's sensors.
    pub(crate) fn connector(&self) -> ConnectFn<FakeConn> {
        let radio = self.clone();
        Arc::new(move |id: &str| {
            let radio = radio.clone();
            let id = id.to_owned();
            async move { radio.connect(id).await }.boxed()
        })
    }

    async fn connect(&self, id: String) -> Result<FakeConn> {
        let mut state = self.state();
        self.log(&mut state, FakeEvent::ConnectStarted { id: id.clone() });
        state.sensor(&id).connects += 1;
        if !state.sensor(&id).script.pop_front().unwrap_or(true) {
            self.log(&mut state, FakeEvent::ConnectFailed { id: id.clone() });
            return Err(Error::Timeout {
                operation: "connect to device".into(),
                duration: Duration::from_secs(15),
            });
        }
        state.last_handle += 1;
        let handle = state.last_handle;
        state.sensor(&id).up = true;
        self.log(
            &mut state,
            FakeEvent::Connected {
                id: id.clone(),
                handle,
            },
        );
        Ok(FakeConn {
            radio: self.clone(),
            id,
            handle,
            disconnected: AtomicBool::new(false),
        })
    }

    /// Outcomes of the next connects to `id`: `false` fails with a connect timeout.
    pub(crate) fn script_connects(&self, id: &str, outcomes: impl IntoIterator<Item = bool>) {
        self.state().sensor(id).script.extend(outcomes);
    }

    /// The sensor went out of range: its link is down, and no handle knows yet.
    pub(crate) fn lose_link(&self, id: &str) {
        self.state().sensor(id).up = false;
    }

    pub(crate) fn link_up(&self, id: &str) -> bool {
        self.state().sensor(id).up
    }

    /// Connect attempts started for `id`, including the first link.
    pub(crate) fn connect_count(&self, id: &str) -> usize {
        self.state().sensor(id).connects
    }

    pub(crate) fn events(&self) -> Vec<(Duration, FakeEvent)> {
        self.state().events.clone()
    }

    #[track_caller]
    pub(crate) fn assert_no_drop_teardown(&self) {
        let events = self.events();
        assert!(
            !events
                .iter()
                .any(|(_, e)| matches!(e, FakeEvent::DropTeardown { .. })),
            "a handle was dropped without being disconnected: {events:#?}"
        );
    }
}

/// One handle on a `FakeRadio` sensor's link.
pub(crate) struct FakeConn {
    radio: FakeRadio,
    id: String,
    handle: u64,
    disconnected: AtomicBool,
}

impl FakeConn {
    /// Any operation on the sensor: it works while the sensor's link is up.
    pub(crate) async fn op(&self) -> Result<()> {
        let mut state = self.radio.state();
        let ok = state.sensor(&self.id).up;
        self.radio.log(
            &mut state,
            FakeEvent::Op {
                id: self.id.clone(),
                handle: self.handle,
                ok,
            },
        );
        if ok { Ok(()) } else { Err(Error::NotConnected) }
    }
}

impl SensorLink for FakeConn {
    async fn is_connected(&self) -> bool {
        self.radio.link_up(&self.id)
    }

    async fn disconnect(&self) -> Result<()> {
        self.disconnected.store(true, Ordering::SeqCst);
        let mut state = self.radio.state();
        state.sensor(&self.id).up = false;
        self.radio.log(
            &mut state,
            FakeEvent::Disconnect {
                id: self.id.clone(),
                handle: self.handle,
            },
        );
        Ok(())
    }
}

impl Drop for FakeConn {
    fn drop(&mut self) {
        if !self.disconnected.load(Ordering::SeqCst) {
            let mut state = self.radio.state();
            state.sensor(&self.id).up = false;
            self.radio.log(
                &mut state,
                FakeEvent::DropTeardown {
                    id: self.id.clone(),
                    handle: self.handle,
                },
            );
        }
    }
}
