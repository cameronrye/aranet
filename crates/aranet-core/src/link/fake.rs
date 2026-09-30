//! A scripted `GattLink` for the connect-sequence tests.

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use btleplug::api::{CharPropFlags, Characteristic, PeripheralProperties, Service};
use tokio::sync::Notify;

use super::GattLink;
use crate::uuid::{CURRENT_READINGS_DETAIL, SAF_TEHNIKA_SERVICE_NEW};

/// What a scripted call does when it is awaited.
pub(super) enum Outcome {
    /// Succeed at once.
    Ok,
    /// Fail at once with a Bluetooth error, as a refused connection or a
    /// failed discovery does.
    Fail,
    /// Never answer, as btleplug does on macOS once CoreBluetooth has dropped
    /// the peripheral.
    Hang,
    /// Fail as bluez-async's `connect` does while BlueZ is still discovering
    /// the services of a connected device: after `BLUEZ_ASYNC_WAIT`, with
    /// `btleplug::Error::Other("Service discovery timed out")`.
    DiscoveryTimedOut,
}

/// How long bluez-async's `connect` waits for BlueZ's `ServicesResolved`
/// before it fails (`SERVICE_DISCOVERY_TIMEOUT`, bluez-async 0.8.2
/// `src/lib.rs:61`).
pub(super) const BLUEZ_ASYNC_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// A call made on a `FakeGatt`, in the order it was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Call {
    Connect,
    Discover,
    Disconnect,
}

/// A peripheral whose calls follow a script. Clones share the script and the
/// call log, like clones of a btleplug `Peripheral` share one device.
#[derive(Clone)]
pub(super) struct FakeGatt {
    state: Arc<Mutex<State>>,
    disconnected: Arc<Notify>,
}

struct State {
    connect: VecDeque<Outcome>,
    discover: VecDeque<Outcome>,
    disconnect: VecDeque<Outcome>,
    properties: VecDeque<Outcome>,
    service_rounds: VecDeque<BTreeSet<Service>>,
    services: BTreeSet<Service>,
    calls: Vec<Call>,
}

impl FakeGatt {
    /// A peripheral on which every call succeeds and every discovery finds
    /// `aranet_services()`.
    pub(super) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                connect: VecDeque::new(),
                discover: VecDeque::new(),
                disconnect: VecDeque::new(),
                properties: VecDeque::new(),
                service_rounds: VecDeque::new(),
                services: BTreeSet::new(),
                calls: Vec::new(),
            })),
            disconnected: Arc::new(Notify::new()),
        }
    }

    /// Script the next `connect` calls. Calls past the script succeed.
    pub(super) fn script_connect(&self, outcomes: impl IntoIterator<Item = Outcome>) {
        self.lock().connect.extend(outcomes);
    }

    /// Script the next `discover_services` calls. Calls past the script
    /// succeed. A discovery takes its service round whatever its outcome.
    pub(super) fn script_discover(&self, outcomes: impl IntoIterator<Item = Outcome>) {
        self.lock().discover.extend(outcomes);
    }

    /// Script the next `disconnect` calls. Calls past the script succeed.
    pub(super) fn script_disconnect(&self, outcomes: impl IntoIterator<Item = Outcome>) {
        self.lock().disconnect.extend(outcomes);
    }

    /// Script the next `properties` calls. Calls past the script succeed.
    pub(super) fn script_properties(&self, outcomes: impl IntoIterator<Item = Outcome>) {
        self.lock().properties.extend(outcomes);
    }

    /// Script what the next discoveries find. Discoveries past the script find
    /// `aranet_services()`.
    pub(super) fn service_rounds(&self, rounds: impl IntoIterator<Item = BTreeSet<Service>>) {
        self.lock().service_rounds.extend(rounds);
    }

    /// The calls made so far, in order.
    pub(super) fn calls(&self) -> Vec<Call> {
        self.lock().calls.clone()
    }

    /// Notified each time a `disconnect` succeeds. `notify_one` stores a
    /// permit when nobody is waiting yet, so a `notified().await` that starts
    /// after the disconnect still completes.
    pub(super) fn disconnected(&self) -> Arc<Notify> {
        Arc::clone(&self.disconnected)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Await a scripted outcome. An empty script means `Ok`.
async fn run(outcome: Option<Outcome>) -> btleplug::Result<()> {
    match outcome.unwrap_or(Outcome::Ok) {
        Outcome::Ok => Ok(()),
        Outcome::Fail => Err(btleplug::Error::RuntimeError("scripted failure".into())),
        Outcome::Hang => std::future::pending().await,
        Outcome::DiscoveryTimedOut => {
            tokio::time::sleep(BLUEZ_ASYNC_WAIT).await;
            // bluez-async's `BluetoothError::ServiceDiscoveryTimedOut`
            // (`src/lib.rs:90-92`), which btleplug wraps in `Error::Other`
            // (`src/bluez/adapter.rs:125-129`).
            Err(btleplug::Error::Other("Service discovery timed out".into()))
        }
    }
}

impl GattLink for FakeGatt {
    async fn connect(&self) -> btleplug::Result<()> {
        let outcome = {
            let mut state = self.lock();
            state.calls.push(Call::Connect);
            state.connect.pop_front()
        };
        run(outcome).await
    }

    async fn disconnect(&self) -> btleplug::Result<()> {
        let outcome = {
            let mut state = self.lock();
            state.calls.push(Call::Disconnect);
            state.disconnect.pop_front()
        };
        run(outcome).await?;
        self.disconnected.notify_one();
        Ok(())
    }

    async fn discover_services(&self) -> btleplug::Result<()> {
        let outcome = {
            let mut state = self.lock();
            state.calls.push(Call::Discover);
            state.services = state
                .service_rounds
                .pop_front()
                .unwrap_or_else(aranet_services);
            state.discover.pop_front()
        };
        run(outcome).await
    }

    fn services(&self) -> BTreeSet<Service> {
        self.lock().services.clone()
    }

    async fn properties(&self) -> btleplug::Result<Option<PeripheralProperties>> {
        let outcome = self.lock().properties.pop_front();
        run(outcome).await?;
        Ok(Some(PeripheralProperties {
            local_name: Some("Aranet4 12345".to_string()),
            ..Default::default()
        }))
    }
}

/// The services of a sensor, cut down to one: the primary Aranet service with
/// its readable current-readings characteristic.
pub(super) fn aranet_services() -> BTreeSet<Service> {
    let current_readings = Characteristic {
        uuid: CURRENT_READINGS_DETAIL,
        service_uuid: SAF_TEHNIKA_SERVICE_NEW,
        properties: CharPropFlags::READ,
        descriptors: BTreeSet::new(),
    };
    BTreeSet::from([Service {
        uuid: SAF_TEHNIKA_SERVICE_NEW,
        primary: true,
        characteristics: BTreeSet::from([current_readings]),
    }])
}
