//! Helpers shared by aranet-core's unit tests.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::FutureExt;
use tokio::time::Instant;

use aranet_types::{CurrentReading, DeviceInfo, DeviceType};

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
    connect_delay: Duration,
    /// Outcomes of the next connects; `false` fails. Unscripted connects succeed.
    script: VecDeque<bool>,
    fail_disconnects: bool,
    /// How long each `disconnect()` waits before the link goes down.
    disconnect_delay: Duration,
    /// How long each `read_device_info()` takes.
    info_delay: Duration,
    /// The link is up but answers nothing (`FakeRadio::make_zombie`).
    zombie: bool,
    /// How long `is_connected` and `is_alive` take (`FakeRadio::set_probe_delay`).
    probe_delay: Duration,
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

    /// Dropping this future part-way changes nothing, like Task 5's `Device::connect`.
    async fn connect(&self, id: String) -> Result<FakeConn> {
        let delay = {
            let mut state = self.state();
            self.log(&mut state, FakeEvent::ConnectStarted { id: id.clone() });
            let sensor = state.sensor(&id);
            sensor.connects += 1;
            sensor.connect_delay
        };
        tokio::time::sleep(delay).await;

        let mut state = self.state();
        if !state.sensor(&id).script.pop_front().unwrap_or(true) {
            self.log(&mut state, FakeEvent::ConnectFailed { id: id.clone() });
            return Err(Error::Timeout {
                operation: "connect to device".into(),
                duration: Duration::from_secs(15),
            });
        }
        state.last_handle += 1;
        let handle = state.last_handle;
        let sensor = state.sensor(&id);
        sensor.up = true;
        sensor.zombie = false;
        self.log(
            &mut state,
            FakeEvent::Connected {
                id: id.clone(),
                handle,
            },
        );
        Ok(FakeConn {
            radio: self.clone(),
            name: format!("Aranet4 {id}"),
            id,
            handle,
            disconnected: AtomicBool::new(false),
        })
    }

    pub(crate) fn set_connect_delay(&self, id: &str, delay: Duration) {
        self.state().sensor(id).connect_delay = delay;
    }

    /// Outcomes of the next connects to `id`: `false` fails with a connect timeout.
    pub(crate) fn script_connects(&self, id: &str, outcomes: impl IntoIterator<Item = bool>) {
        self.state().sensor(id).script.extend(outcomes);
    }

    /// Every later `disconnect()` of `id` returns an error, but still takes the link down.
    pub(crate) fn fail_disconnects(&self, id: &str) {
        self.state().sensor(id).fail_disconnects = true;
    }

    /// Every later `disconnect()` of `id` waits `delay` before the link goes
    /// down, on its own task.
    pub(crate) fn set_disconnect_delay(&self, id: &str, delay: Duration) {
        self.state().sensor(id).disconnect_delay = delay;
    }

    /// Every later `read_device_info()` on `id` takes `delay`.
    pub(crate) fn set_info_delay(&self, id: &str, delay: Duration) {
        self.state().sensor(id).info_delay = delay;
    }

    /// Takes `id`'s link down for `handle`'s `disconnect()`.
    fn disconnect_now(&self, id: &str, handle: u64) -> Result<()> {
        let mut state = self.state();
        let sensor = state.sensor(id);
        sensor.up = false;
        let fail = sensor.fail_disconnects;
        self.log(
            &mut state,
            FakeEvent::Disconnect {
                id: id.to_owned(),
                handle,
            },
        );
        if fail {
            Err(Error::Timeout {
                operation: "disconnect from device".into(),
                duration: Duration::from_secs(5),
            })
        } else {
            Ok(())
        }
    }

    /// The sensor's link stays up, so `is_connected` is true, but it answers
    /// nothing: `is_alive` is false and reads time out. The next connect
    /// clears it.
    pub(crate) fn make_zombie(&self, id: &str) {
        self.state().sensor(id).zombie = true;
    }

    /// `is_connected` and `is_alive` on `id`'s links take `delay`.
    pub(crate) fn set_probe_delay(&self, id: &str, delay: Duration) {
        self.state().sensor(id).probe_delay = delay;
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

    /// The sensors whose link is up, sorted.
    pub(crate) fn up_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .state()
            .sensors
            .iter()
            .filter(|(_, sensor)| sensor.up)
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        ids
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
    /// What `SensorLink::name` returns: "Aranet4 <id>".
    name: String,
    handle: u64,
    disconnected: AtomicBool,
}

impl FakeConn {
    pub(crate) fn handle(&self) -> u64 {
        self.handle
    }

    /// Waits as long as `set_probe_delay` says.
    async fn probe_delay(&self) {
        let delay = self.radio.state().sensor(&self.id).probe_delay;
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }

    /// The error every read of a zombie link returns.
    fn zombie_timeout(&self) -> Result<()> {
        if self.radio.state().sensor(&self.id).zombie {
            return Err(Error::Timeout {
                operation: "read from device".into(),
                duration: Duration::from_secs(10),
            });
        }
        Ok(())
    }

    /// Any operation on the sensor: it works while the sensor's link is up.
    pub(crate) async fn op(&self) -> Result<()> {
        self.zombie_timeout()?;
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
        self.probe_delay().await;
        self.radio.link_up(&self.id)
    }

    async fn disconnect(&self) -> Result<()> {
        self.disconnected.store(true, Ordering::SeqCst);
        let delay = self.radio.state().sensor(&self.id).disconnect_delay;
        if delay.is_zero() {
            return self.radio.disconnect_now(&self.id, self.handle);
        }
        // Like `Device::disconnect` (Task 6), a slow disconnect runs on its own
        // task, so it takes the link down even if this future is dropped.
        let (radio, id, handle) = (self.radio.clone(), self.id.clone(), self.handle);
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            radio.disconnect_now(&id, handle)
        })
        .await
        .expect("the fake's disconnect task panicked")
    }

    fn name(&self) -> Option<&str> {
        Some(&self.name)
    }

    fn device_type(&self) -> Option<DeviceType> {
        Some(DeviceType::Aranet4)
    }

    async fn is_alive(&self) -> bool {
        self.probe_delay().await;
        let mut state = self.radio.state();
        let sensor = state.sensor(&self.id);
        sensor.up && !sensor.zombie
    }

    async fn read_current(&self) -> Result<CurrentReading> {
        self.zombie_timeout()?;
        if self.radio.link_up(&self.id) {
            Ok(CurrentReading::default())
        } else {
            Err(Error::NotConnected)
        }
    }

    async fn read_device_info(&self) -> Result<DeviceInfo> {
        self.zombie_timeout()?;
        let delay = self.radio.state().sensor(&self.id).info_delay;
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        if self.radio.link_up(&self.id) {
            Ok(DeviceInfo::default())
        } else {
            Err(Error::NotConnected)
        }
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
