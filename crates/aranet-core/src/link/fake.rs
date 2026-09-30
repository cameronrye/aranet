//! A scripted `GattLink` for the connect-sequence tests.

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use btleplug::api::{CharPropFlags, Characteristic, PeripheralProperties, Service};

use super::GattLink;
use crate::uuid::{CURRENT_READINGS_DETAIL, SAF_TEHNIKA_SERVICE_NEW};

/// What a scripted call does when it is awaited.
pub(super) enum Outcome {
    /// Succeed at once.
    Ok,
    /// Never answer, as btleplug does on macOS once CoreBluetooth has dropped
    /// the peripheral.
    Hang,
}

/// A call made on a `FakeGatt`, in the order it was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Call {
    Connect,
    Discover,
    Disconnect,
}

/// A peripheral whose calls follow a script. Clones share the script and the
/// call log, as clones of a btleplug `Peripheral` share one device.
#[derive(Clone)]
pub(super) struct FakeGatt {
    state: Arc<Mutex<State>>,
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
        }
    }

    /// Script the next `connect` calls. Calls past the script succeed.
    pub(super) fn script_connect(&self, outcomes: impl IntoIterator<Item = Outcome>) {
        self.lock().connect.extend(outcomes);
    }

    /// Script what the next discoveries find. Each discovery takes its round
    /// when it is called, whatever its outcome; discoveries past the script
    /// find `aranet_services()`.
    pub(super) fn service_rounds(&self, rounds: impl IntoIterator<Item = BTreeSet<Service>>) {
        self.lock().service_rounds.extend(rounds);
    }

    /// The calls made so far, in order.
    pub(super) fn calls(&self) -> Vec<Call> {
        self.lock().calls.clone()
    }

    /// The state, even if a test panicked while holding the lock.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Await a scripted outcome. An empty script means `Ok`.
async fn run(outcome: Option<Outcome>) -> btleplug::Result<()> {
    match outcome.unwrap_or(Outcome::Ok) {
        Outcome::Ok => Ok(()),
        Outcome::Hang => std::future::pending().await,
    }
}

// Each call records itself and takes its outcome under the lock, then awaits
// the outcome with the lock released.
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
        run(outcome).await
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
            local_name: Some("Aranet4 12345".into()),
            ..Default::default()
        }))
    }
}

/// A sensor's services, cut down to one: the primary Aranet service with its
/// readable current-readings characteristic.
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
