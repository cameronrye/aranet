//! A scripted `GattLink` for the connect-sequence and notification tests.

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use btleplug::api::{
    CharPropFlags, Characteristic, PeripheralProperties, Service, ValueNotification,
};
use futures::channel::mpsc;
use tokio::sync::Notify;
use uuid::Uuid;

use super::{GattLink, NotificationStream};
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
    /// Fail at once as BlueZ refuses a `Device1.Connect` while an earlier one
    /// to the same device is still pending (`org.bluez.Error.InProgress`),
    /// with `btleplug::Error::Other("In Progress")`.
    InProgress,
    /// Wait this long on the tokio clock, then succeed like `Ok`. A
    /// `disconnect` that ends this way notifies `disconnected()` at the end
    /// of the wait.
    After(std::time::Duration),
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
    IsConnected,
    Subscribe(Uuid),
    Unsubscribe(Uuid),
    /// The Linux pairing step, with the budget it was given. Recorded only
    /// once `script_pair` has been called.
    Pair(std::time::Duration),
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
    is_connected: VecDeque<Outcome>,
    service_rounds: VecDeque<BTreeSet<Service>>,
    services: BTreeSet<Service>,
    calls: Vec<Call>,
    /// What a successful `is_connected` answers.
    connected: bool,
    /// What `is_bluez` answers.
    bluez: bool,
    subscribe: VecDeque<Outcome>,
    unsubscribe: VecDeque<Outcome>,
    notifications: VecDeque<Outcome>,
    /// The sending half of every stream `notifications()` has returned.
    streams: Vec<mpsc::UnboundedSender<ValueNotification>>,
    /// What each `subscribe` sends to the open streams before it answers.
    emit_on_subscribe: Option<Vec<u8>>,
    /// The pairing step's script. `None` until `script_pair` is called: until
    /// then the fake has no pairing step, so the call logs of the other link
    /// tests stay as they are.
    pair: Option<VecDeque<Outcome>>,
}

impl State {
    /// Send a notification from characteristic `uuid` to every open stream.
    fn send(&self, uuid: Uuid, value: &[u8]) {
        for stream in &self.streams {
            // Sending fails only to a stream that has been dropped.
            let _ = stream.unbounded_send(ValueNotification {
                uuid,
                value: value.to_vec(),
            });
        }
    }
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
                is_connected: VecDeque::new(),
                service_rounds: VecDeque::new(),
                services: BTreeSet::new(),
                calls: Vec::new(),
                connected: true,
                bluez: false,
                subscribe: VecDeque::new(),
                unsubscribe: VecDeque::new(),
                notifications: VecDeque::new(),
                streams: Vec::new(),
                emit_on_subscribe: None,
                pair: None,
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

    /// Script the next `is_connected` calls. Calls past the script succeed.
    pub(super) fn script_is_connected(&self, outcomes: impl IntoIterator<Item = Outcome>) {
        self.lock().is_connected.extend(outcomes);
    }

    /// Set what a successful `is_connected` answers (`true` until set).
    pub(super) fn set_connected(&self, connected: bool) {
        self.lock().connected = connected;
    }

    /// Set whether the fake is a BlueZ link (`is_bluez`): `false`, as on
    /// macOS and Windows, until set.
    pub(super) fn set_bluez(&self, bluez: bool) {
        self.lock().bluez = bluez;
    }

    /// Give the fake a pairing step and script its next runs. Runs past the
    /// script return at once. Pairing never fails a connect, so a `Fail` is
    /// ignored; `Hang` never returns.
    pub(super) fn script_pair(&self, outcomes: impl IntoIterator<Item = Outcome>) {
        self.lock()
            .pair
            .get_or_insert_with(VecDeque::new)
            .extend(outcomes);
    }

    /// Script what the next discoveries find. Discoveries past the script find
    /// `aranet_services()`.
    pub(super) fn service_rounds(&self, rounds: impl IntoIterator<Item = BTreeSet<Service>>) {
        self.lock().service_rounds.extend(rounds);
    }

    /// Script the next `subscribe` calls. Calls past the script succeed.
    pub(super) fn script_subscribe(&self, outcomes: impl IntoIterator<Item = Outcome>) {
        self.lock().subscribe.extend(outcomes);
    }

    /// Script the next `unsubscribe` calls. Calls past the script succeed.
    pub(super) fn script_unsubscribe(&self, outcomes: impl IntoIterator<Item = Outcome>) {
        self.lock().unsubscribe.extend(outcomes);
    }

    /// Script the next `notifications` calls, which open a stream once their
    /// outcome succeeds. Calls past the script open one at once.
    pub(super) fn script_notifications(&self, outcomes: impl IntoIterator<Item = Outcome>) {
        self.lock().notifications.extend(outcomes);
    }

    /// Send a notification from characteristic `uuid` to every open stream.
    pub(super) fn emit(&self, uuid: Uuid, value: &[u8]) {
        self.lock().send(uuid, value);
    }

    /// Make every later `subscribe` send `value`, as a notification from the
    /// characteristic being subscribed, to every open stream before it
    /// answers: a device that notifies as soon as the CCCD is written.
    pub(super) fn emit_on_subscribe(&self, value: &[u8]) {
        self.lock().emit_on_subscribe = Some(value.to_vec());
    }

    /// How many of the streams `notifications()` returned are still open.
    pub(super) fn open_streams(&self) -> usize {
        self.lock()
            .streams
            .iter()
            .filter(|stream| !stream.is_closed())
            .count()
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
        // bluez-async's `BluetoothError::DbusError`, whose text is the D-Bus
        // error's message, wrapped in `Error::Other` by btleplug.
        Outcome::InProgress => Err(btleplug::Error::Other("In Progress".into())),
        Outcome::After(delay) => {
            tokio::time::sleep(delay).await;
            Ok(())
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

    async fn is_connected(&self) -> btleplug::Result<bool> {
        let outcome = {
            let mut state = self.lock();
            state.calls.push(Call::IsConnected);
            state.is_connected.pop_front()
        };
        run(outcome).await?;
        Ok(self.lock().connected)
    }

    fn is_bluez(&self) -> bool {
        self.lock().bluez
    }

    async fn subscribe(&self, characteristic: &Characteristic) -> btleplug::Result<()> {
        let outcome = {
            let mut state = self.lock();
            state.calls.push(Call::Subscribe(characteristic.uuid));
            if let Some(value) = &state.emit_on_subscribe {
                state.send(characteristic.uuid, value);
            }
            state.subscribe.pop_front()
        };
        run(outcome).await
    }

    async fn unsubscribe(&self, characteristic: &Characteristic) -> btleplug::Result<()> {
        let outcome = {
            let mut state = self.lock();
            state.calls.push(Call::Unsubscribe(characteristic.uuid));
            state.unsubscribe.pop_front()
        };
        run(outcome).await
    }

    async fn notifications(&self) -> btleplug::Result<NotificationStream> {
        let outcome = self.lock().notifications.pop_front();
        run(outcome).await?;
        let (sender, receiver) = mpsc::unbounded();
        self.lock().streams.push(sender);
        Ok(Box::pin(receiver))
    }

    async fn pair_if_needed(&self, budget: std::time::Duration) {
        let outcome = {
            let mut state = self.lock();
            let Some(script) = state.pair.as_mut() else {
                return;
            };
            let outcome = script.pop_front();
            state.calls.push(Call::Pair(budget));
            outcome
        };
        // Pairing never fails a connect, so the result is ignored.
        let _ = run(outcome).await;
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

/// A characteristic that can notify, for the notification tests.
pub(super) fn characteristic(uuid: Uuid) -> Characteristic {
    Characteristic {
        uuid,
        service_uuid: SAF_TEHNIKA_SERVICE_NEW,
        properties: CharPropFlags::NOTIFY,
        descriptors: BTreeSet::new(),
    }
}
