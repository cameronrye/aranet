//! Pairing Aranet sensors with BlueZ on Linux.
//!
//! Before `aranet-core` connects to a sensor on Linux, it pairs the sensor if
//! BlueZ doesn't list it as paired. Otherwise BlueZ asks for pairing by itself
//! on every connection: its battery plugin reads the sensor's Battery Level,
//! which needs encryption. That request needs an agent. With none registered,
//! as on a headless host, BlueZ refuses it and the connection's reads can stall
//! until they time out; a desktop's agent shows a pairing dialog instead. Each
//! pairing is one short session:
//!
//! 1. If the sensor refused to pair less than 10 minutes ago in this process
//!    (`AuthenticationFailed` or `AuthenticationRejected`, as when it wants a
//!    PIN), skip it and connect without pairing.
//! 2. On aranet-core's own runtime, open a private connection to the system bus
//!    and read the sensor's `org.bluez.Device1.Paired` property. A paired
//!    sensor ends the session.
//! 3. Register a `NoInputNoOutput` agent at `/dev/rye/aranet/agent` on that
//!    connection (`org.bluez.AgentManager1.RegisterAgent`).
//! 4. Call `org.bluez.Device1.Pair` on the same connection. It gets as long as
//!    a connect that waits for BlueZ's service discovery would. If it runs out
//!    while BlueZ still lists the sensor as not paired, `CancelPairing` stops
//!    it. If BlueZ doesn't answer whether it is paired, closing the connection
//!    stops a pairing that is still running.
//! 5. Unregister the agent and close the connection.
//!
//! When BlueZ connects the sensor for `Pair`, it answers `Pair` only after its
//! service discovery has finished, which can take an Aranet4 about 20 s, but it
//! lists the sensor as paired as soon as the bond exists. A `Pair` that runs
//! out of time after that counts as paired and isn't cancelled, which would
//! remove the new bond. The connect goes ahead, and the session keeps its
//! connection open, with the agent unregistered, until BlueZ answers `Pair`:
//! closing it earlier would cancel that discovery.
//!
//! BlueZ sends the agent requests of a `Pair` call to the agent that the
//! calling connection registered. On a host with no other agent, BlueZ also
//! makes the session's agent the default one while it is registered, so the
//! agent approves requests only for the object path of the sensor being paired
//! and rejects the rest. It always rejects `AuthorizeService`, and
//! `RequestPasskey`, which would need a keyboard. Closing the connection
//! removes the agent and cancels an unfinished pairing, even when the session
//! is cut short.
//!
//! aranet never asks to be BlueZ's default agent, and it keeps no agent
//! registered between connects. Since BlueZ 5.51 the first agent to register
//! becomes the default one, so a long-lived agent on a host without a desktop
//! agent would receive the pairing requests of every nearby device. Scans, and
//! sensors that are already paired, never register an agent.
//!
//! A pairing that fails is logged as a warning, and the connect goes ahead
//! unpaired, with the stall or the dialog described above. The next connect
//! pairs again, unless the sensor refused.
//!
//! The session's agent has no display or keyboard, so it can't pair a sensor
//! that asks for its PIN. Some such sensors don't refuse (an Aranet2 with
//! BlueZ 5.82, for example): the pairing finishes without the PIN, then the
//! sensor rejects that bond at every later connect, so its reads fail while
//! BlueZ lists it as paired, and a new bond made the same way fails the same
//! way. Pair such a sensor once by hand, with its PIN; aranet then uses that
//! bond. To do that, stop every aranet process that uses it (a connect during
//! the pairing cancels it) and run `bluetoothctl remove <MAC>`. Then run
//! `bluetoothctl` and, at its prompt, `scan on` until the sensor is listed,
//! `scan off` and `pair <MAC>`, entering the PIN when asked. A one-line
//! `bluetoothctl pair <MAC>` registers no agent, so on a host without a
//! desktop agent it can't ask for the PIN. A sensor that has lost its bond
//! with this computer (after a reset, for example) is still listed as paired,
//! and its encrypted reads fail too: remove the stale bond with
//! `bluetoothctl remove <MAC>`. The next connect then pairs it again, unless
//! it asks for its PIN; then pair it by hand as above.

use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use btleplug::api::Peripheral as _;
use btleplug::platform::Peripheral;
use dbus::channel::MatchingReceiver;
use dbus::message::MatchRule;
use dbus::nonblock::stdintf::org_freedesktop_dbus::Properties;
use dbus::nonblock::{Proxy, SyncConnection};
use dbus_crossroads::{Crossroads, IfaceBuilder, MethodErr};
use dbus_tokio::connection::IOResource;
use tokio::sync::oneshot;
use tokio_util::task::AbortOnDropHandle;
use tracing::{debug, info, warn};

use crate::pairing::{BluezError, PAIRING_RETRY_AFTER, PairOutcome, PairingBus, PairingCooldown};

/// Object path of the session's agent, on the session's own connection.
const AGENT_PATH: &str = "/dev/rye/aranet/agent";
/// aranet has no display or keyboard to offer, so sensors pair with "Just Works".
const AGENT_CAPABILITY: &str = "NoInputNoOutput";
const BLUEZ_SERVICE: &str = "org.bluez";
const BLUEZ_ROOT: &str = "/org/bluez";
const AGENT_MANAGER_IFACE: &str = "org.bluez.AgentManager1";
const DEVICE_IFACE: &str = "org.bluez.Device1";
/// Limit for connecting to the system bus and for every BlueZ call but `Pair`:
/// `bluetoothd` answers each of them at once. It answers `Pair` only when the
/// pairing ends, so `run_pairing` limits `Pair` by the session's budget
/// instead. A session therefore reports its outcome at most three of these
/// after its budget: the bus connect, and `Paired` and `CancelPairing` after
/// `Pair` ran out.
const CALL_TIMEOUT: Duration = Duration::from_secs(1);
/// `Pair`'s D-Bus reply timeout. `run_pairing` puts its own limits on `Pair`,
/// so this only has to be longer than they can be. dbus adds it to
/// `Instant::now()` without checking, so it can't be `Duration::MAX`.
const PAIR_REPLY_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
/// How long a sensor that refused to pair is left unpaired, for log messages.
const RETRY_MINUTES: u64 = PAIRING_RETRY_AFTER.as_secs() / 60;

/// Sensors that refused to pair, by BlueZ object path. Such a sensor costs at
/// most one pairing attempt every 10 minutes.
static COOLDOWN: LazyLock<Mutex<PairingCooldown>> =
    LazyLock::new(|| Mutex::new(PairingCooldown::default()));

/// Does nothing.
///
/// Up to 0.2.1 this registered a BlueZ agent for the whole process. aranet-core
/// now pairs a sensor itself while connecting to it, through an agent that
/// exists only for that pairing, so there is nothing to set up. This function
/// will be removed in 0.4.0.
#[deprecated(
    since = "0.3.0",
    note = "aranet-core now pairs devices itself while connecting; no process-wide agent is registered"
)]
pub fn ensure_agent() {}

/// Pairs `peripheral` if BlueZ doesn't list it as paired, and logs how that
/// went. `None` when the cooldown skipped the attempt.
///
/// It returns within about 3 s of `budget`. A failure is returned for the
/// caller's information only: the caller connects either way.
pub(crate) async fn pair_if_needed(
    peripheral: &Peripheral,
    budget: Duration,
) -> Option<PairOutcome> {
    let address = peripheral.address().to_string();
    let Some(device) = device_path(&peripheral.id().to_string()) else {
        let outcome = PairOutcome::Unavailable(BluezError {
            name: "org.freedesktop.DBus.Error.InvalidArgs".into(),
            message: format!("no BlueZ object path for {}", peripheral.id()),
        });
        log_outcome(&address, &outcome);
        return Some(outcome);
    };
    let session = run_session(device.clone(), budget);
    let outcome = crate::pairing::pair_with_cooldown(&COOLDOWN, &device, session).await;
    match &outcome {
        Some(outcome) => log_outcome(&address, outcome),
        None => debug!(
            "Not pairing with {address}: it refused to pair less than {RETRY_MINUTES} minutes ago"
        ),
    }
    outcome
}

/// Runs a pairing session for `device` on aranet-core's runtime and returns
/// its outcome as soon as the session reports it; the session may go on after
/// that (see the module docs). If this future is dropped before the outcome is
/// known, the session is aborted, which closes its connection.
async fn run_session(device: dbus::Path<'static>, budget: Duration) -> PairOutcome {
    // The runtime that polls the session's D-Bus connection drives its socket,
    // so the session runs on aranet-ble rather than on the caller's runtime,
    // which may have no I/O driver, and it can outlive the caller's wait.
    let Some(runtime) = crate::link::cleanup_runtime() else {
        return PairOutcome::Unavailable(failed("no tokio runtime to pair on"));
    };
    let (report, outcome) = oneshot::channel();
    let session = AbortOnDropHandle::new(runtime.spawn(pair_session(device, budget, report)));
    let outcome = outcome.await;
    // The session has reported: let it finish on its own.
    drop(session.detach());
    outcome.unwrap_or_else(|_| {
        PairOutcome::Unavailable(failed("the pairing session ended without an outcome"))
    })
}

/// One pairing session for `device` on a private system-bus connection. It
/// sends the outcome on `report` as soon as it is known, then unregisters the
/// agent and, if BlueZ is holding `Pair`'s reply, waits for it.
async fn pair_session(
    device: dbus::Path<'static>,
    budget: Duration,
    report: oneshot::Sender<PairOutcome>,
) {
    let (resource, conn) = match connect_system_bus().await {
        Ok(connection) => connection,
        Err(e) => {
            let _ = report.send(PairOutcome::Unavailable(e));
            return;
        }
    };
    let mut cr = agent_crossroads(device.clone());
    conn.start_receive(
        MatchRule::new_method_call(),
        Box::new(move |msg, conn| {
            if cr.handle_message(msg, conn).is_err() {
                warn!("BlueZ agent: failed to handle a D-Bus message");
            }
            true
        }),
    );
    let bus = DbusPairingBus {
        conn,
        device,
        report: Mutex::new(Some(report)),
    };
    let mut resource = std::pin::pin!(resource);
    tokio::select! {
        // Poll the connection's I/O first, so tokio watches its socket before
        // the first call goes out: dbus-tokio fails a call that libdbus can't
        // write at once while the I/O future has never been polled.
        biased;
        err = &mut resource => bus.report(&PairOutcome::Unavailable(BluezError {
            name: "org.freedesktop.DBus.Error.Disconnected".into(),
            message: err.to_string(),
        })),
        _ = crate::pairing::run_pairing(&bus, budget) => {}
    }
    // `bus` and `resource` drop here and close the private connection. BlueZ
    // then removes the agent even if `UnregisterAgent` failed, and cancels a
    // pairing that is still running.
}

/// Connects to the system bus on the blocking pool (the connect blocks until
/// the bus has answered `Hello`), giving up after `CALL_TIMEOUT`.
async fn connect_system_bus()
-> Result<(IOResource<SyncConnection>, Arc<SyncConnection>), BluezError> {
    let connect = tokio::task::spawn_blocking(dbus_tokio::connection::new_system_sync);
    match tokio::time::timeout(CALL_TIMEOUT, connect).await {
        Ok(Ok(connection)) => connection.map_err(BluezError::from),
        Ok(Err(e)) => Err(failed(e.to_string())),
        Err(_) => Err(BluezError {
            name: "org.freedesktop.DBus.Error.Timeout".into(),
            message: "the system bus did not answer".into(),
        }),
    }
}

/// A failure that has no D-Bus error name of its own.
fn failed(message: impl Into<String>) -> BluezError {
    BluezError {
        name: "org.freedesktop.DBus.Error.Failed".into(),
        message: message.into(),
    }
}

/// The BlueZ object path of a device, from the text of btleplug's Linux
/// `PeripheralId`: the object path without `/org/bluez/`, such as
/// `hci0/dev_AA_BB_CC_DD_EE_FF`.
fn device_path(id_display: &str) -> Option<dbus::Path<'static>> {
    if id_display.is_empty() {
        return None;
    }
    dbus::Path::new(format!("{BLUEZ_ROOT}/{id_display}")).ok()
}

/// State of a session's agent: the one device it may approve.
struct AgentData {
    device: dbus::Path<'static>,
}

/// Reject agent requests for any device but the one being paired.
fn check_session_device(data: &AgentData, device: &dbus::Path) -> Result<(), MethodErr> {
    if *device == data.device {
        Ok(())
    } else {
        warn!("BlueZ agent: rejecting request for {device} (not the device aranet is pairing)");
        Err((
            "org.bluez.Error.Rejected",
            "Device is not being paired by aranet",
        )
            .into())
    }
}

/// Answer BlueZ's `AuthorizeService` request: always reject.
///
/// BlueZ asks this when a remote device connects to one of the host's own
/// profiles (HID input, audio, PAN, ...). aranet is only ever a GATT client of
/// the sensor, so the Aranet flow never needs it. The device being paired is
/// known by an address that Aranet devices broadcast in the clear, so approving
/// here would let anyone spoofing that address reach those profiles, for
/// example to inject keystrokes over HID.
fn authorize_service(device: &dbus::Path, uuid: &str) -> Result<(), MethodErr> {
    warn!(
        "BlueZ agent: rejecting AuthorizeService {uuid} for {device} (aranet never accepts host profile connections)"
    );
    Err((
        "org.bluez.Error.Rejected",
        "aranet does not authorize host profile connections",
    )
        .into())
}

/// Answer BlueZ's `RequestPasskey` request: always reject.
///
/// BlueZ asks for a passkey when this side has to type in the one that the
/// device shows, which takes a keyboard. The agent has none
/// (`NoInputNoOutput`), so BlueZ pairs sensors with "Just Works", which never
/// asks. If BlueZ asks anyway, aranet has no passkey to give for any device,
/// and rejects the request rather than make one up.
fn request_passkey(device: &dbus::Path) -> Result<(u32,), MethodErr> {
    warn!("BlueZ agent: rejecting RequestPasskey for {device} (aranet can't enter a passkey)");
    Err(("org.bluez.Error.Rejected", "aranet can't enter a passkey").into())
}

/// The `org.bluez.Agent1` object of one pairing session, which approves
/// requests for `device` only.
fn agent_crossroads(device: dbus::Path<'static>) -> Crossroads {
    let mut cr = Crossroads::new();
    let token = cr.register("org.bluez.Agent1", |b: &mut IfaceBuilder<AgentData>| {
        b.method("Release", (), (), |_, _, ()| {
            debug!("BlueZ agent: Release");
            Ok(())
        });

        b.method(
            "RequestPasskey",
            ("device",),
            ("passkey",),
            |_, _, (device,): (dbus::Path,)| request_passkey(&device),
        );

        b.method(
            "RequestConfirmation",
            ("device", "passkey"),
            (),
            |_, data, (device, passkey): (dbus::Path, u32)| {
                debug!("BlueZ agent: RequestConfirmation for {device}, passkey {passkey}");
                check_session_device(data, &device)
            },
        );

        b.method(
            "RequestAuthorization",
            ("device",),
            (),
            |_, data, (device,): (dbus::Path,)| {
                debug!("BlueZ agent: RequestAuthorization for {device}");
                check_session_device(data, &device)
            },
        );

        b.method(
            "AuthorizeService",
            ("device", "uuid"),
            (),
            |_, _, (device, uuid): (dbus::Path, String)| authorize_service(&device, &uuid),
        );

        b.method("Cancel", (), (), |_, _, ()| {
            debug!("BlueZ agent: Cancel");
            Ok(())
        });
    });
    cr.insert(AGENT_PATH, &[token], AgentData { device });
    cr
}

/// BlueZ's D-Bus API for one pairing session, on the session's connection.
struct DbusPairingBus {
    conn: Arc<SyncConnection>,
    /// The sensor's BlueZ object path.
    device: dbus::Path<'static>,
    /// Where `report` sends the outcome; `None` once it has.
    report: Mutex<Option<oneshot::Sender<PairOutcome>>>,
}

impl DbusPairingBus {
    fn proxy(
        &self,
        path: dbus::Path<'static>,
        timeout: Duration,
    ) -> Proxy<'static, Arc<SyncConnection>> {
        Proxy::new(BLUEZ_SERVICE, path, timeout, Arc::clone(&self.conn))
    }
}

impl PairingBus for DbusPairingBus {
    async fn is_paired(&self) -> Result<bool, BluezError> {
        // `Paired`, not `Bonded`: BlueZ 5.64 (Ubuntu 22.04) has no `Bonded` property.
        self.proxy(self.device.clone(), CALL_TIMEOUT)
            .get::<bool>(DEVICE_IFACE, "Paired")
            .await
            .map_err(BluezError::from)
    }

    async fn register_agent(&self) -> Result<(), BluezError> {
        self.proxy(dbus::Path::from(BLUEZ_ROOT), CALL_TIMEOUT)
            .method_call(
                AGENT_MANAGER_IFACE,
                "RegisterAgent",
                (dbus::Path::from(AGENT_PATH), AGENT_CAPABILITY),
            )
            .await
            .map_err(BluezError::from)
    }

    async fn pair(&self) -> Result<(), BluezError> {
        self.proxy(self.device.clone(), PAIR_REPLY_TIMEOUT)
            .method_call(DEVICE_IFACE, "Pair", ())
            .await
            .map_err(BluezError::from)
    }

    async fn cancel_pairing(&self) -> Result<(), BluezError> {
        self.proxy(self.device.clone(), CALL_TIMEOUT)
            .method_call(DEVICE_IFACE, "CancelPairing", ())
            .await
            .map_err(BluezError::from)
    }

    async fn unregister_agent(&self) -> Result<(), BluezError> {
        self.proxy(dbus::Path::from(BLUEZ_ROOT), CALL_TIMEOUT)
            .method_call(
                AGENT_MANAGER_IFACE,
                "UnregisterAgent",
                (dbus::Path::from(AGENT_PATH),),
            )
            .await
            .map_err(BluezError::from)
    }

    fn report(&self, outcome: &PairOutcome) {
        let report = self
            .report
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(report) = report {
            let _ = report.send(outcome.clone());
        }
    }
}

impl From<dbus::Error> for BluezError {
    fn from(e: dbus::Error) -> Self {
        BluezError {
            name: e
                .name()
                .unwrap_or("org.freedesktop.DBus.Error.Failed")
                .to_owned(),
            message: e.message().unwrap_or("").to_owned(),
        }
    }
}

/// Log how a pairing went, with the commands that fix a failed one.
fn log_outcome(address: &str, outcome: &PairOutcome) {
    let failure = match outcome {
        PairOutcome::AlreadyPaired => {
            debug!("{address} is already paired");
            return;
        }
        PairOutcome::Paired => {
            info!("Paired with {address}");
            return;
        }
        PairOutcome::Unavailable(e) => {
            warn!("Could not pair with {address}: {e}");
            return;
        }
        PairOutcome::Failed(e) => format!("failed ({e})"),
        PairOutcome::TimedOut => "timed out".to_owned(),
    };
    let retry = if outcome.refused_by_sensor() {
        format!("aranet will not try to pair it again for {RETRY_MINUTES} minutes")
    } else {
        "aranet will try again at the next connection".to_owned()
    };
    warn!(
        "Pairing with {address} {failure}; {retry}. Until it is paired, BlueZ asks for pairing \
         at each connection, which can make reads time out on a host without a Bluetooth agent. \
         aranet can't enter a PIN: if the sensor asks for one, pair it by hand once. Stop aranet \
         and the aranet service first (a connect during the pairing cancels it), run \
         `bluetoothctl remove {address}` if it was paired with this computer before, then run \
         `bluetoothctl` and, at its prompt, `scan on` until the sensor is listed, `scan off` \
         and `pair {address}`, entering the PIN when asked"
    );
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::ffi::CString;
    use std::sync::Weak;

    use dbus::Message;
    use dbus::arg::Variant;
    use dbus::channel::Sender;
    use dbus::nonblock::stdintf::org_freedesktop_dbus::RequestNameReply;
    use dbus::strings::ErrorName;
    use tokio::sync::Notify;

    use super::*;
    use crate::pairing::ALREADY_EXISTS;
    use crate::test_support::within;

    /// The sensor being paired, and another device nearby.
    const A: &str = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_01";
    const B: &str = "/org/bluez/hci0/dev_11_22_33_44_55_66";
    /// Human Interface Device: approving it would let a spoofed sensor type keystrokes.
    const HID_SERVICE: &str = "00001124-0000-1000-8000-00805f9b34fb";
    const PASSKEY: u32 = 123_456;

    fn path(p: &str) -> dbus::Path<'static> {
        dbus::Path::new(p.to_owned()).unwrap()
    }

    fn agent_call(method: &str) -> Message {
        Message::new_method_call("org.bluez", AGENT_PATH, "org.bluez.Agent1", method).unwrap()
    }

    /// Hand `call` to the agent of a session that pairs `session_device`, as
    /// BlueZ would, and return the agent's only reply.
    fn dispatch(session_device: &str, mut call: Message) -> Message {
        let mut cr = agent_crossroads(path(session_device));
        call.set_serial(1);
        let sent = RefCell::new(Vec::new());
        cr.handle_message(call, &sent).unwrap();
        let mut replies = sent.into_inner();
        assert_eq!(replies.len(), 1, "the agent should send exactly one reply");
        replies.remove(0)
    }

    fn assert_approved(mut reply: Message) {
        reply
            .as_result()
            .expect("the agent should approve this request");
    }

    fn assert_rejected(mut reply: Message) {
        let err = reply
            .as_result()
            .expect_err("the agent should reject this request");
        assert_eq!(err.name(), Some("org.bluez.Error.Rejected"));
    }

    /// Guards source compatibility: the deprecated `ensure_agent` stays
    /// callable, so existing callers keep compiling, and spawns nothing, so a
    /// call outside a tokio runtime doesn't panic.
    #[test]
    #[allow(deprecated)]
    fn deprecated_ensure_agent_is_a_no_op() {
        // No tokio runtime here: up to aranet-core 0.2.1, `ensure_agent`
        // spawned a task, which panics outside a runtime.
        ensure_agent();
    }

    #[test]
    fn agent_approves_only_the_device_being_paired() {
        assert_approved(dispatch(
            A,
            agent_call("RequestAuthorization").append1(path(A)),
        ));
        assert_approved(dispatch(
            A,
            agent_call("RequestConfirmation").append2(path(A), PASSKEY),
        ));

        assert_rejected(dispatch(
            A,
            agent_call("RequestAuthorization").append1(path(B)),
        ));
        assert_rejected(dispatch(
            A,
            agent_call("RequestConfirmation").append2(path(B), PASSKEY),
        ));
    }

    #[test]
    fn agent_sessions_do_not_share_approvals() {
        // In aranet-core 0.2.1, one agent for the whole process approved
        // every device aranet had connected to, so it kept approving A after
        // A's connect was over. Each session's agent approves only its own.
        assert_approved(dispatch(
            A,
            agent_call("RequestAuthorization").append1(path(A)),
        ));
        assert_rejected(dispatch(
            B,
            agent_call("RequestAuthorization").append1(path(A)),
        ));
    }

    #[test]
    fn agent_rejects_authorize_service_even_for_the_device_being_paired() {
        assert_rejected(dispatch(
            A,
            agent_call("AuthorizeService").append2(path(A), HID_SERVICE),
        ));
    }

    #[test]
    fn agent_rejects_request_passkey_even_for_the_device_being_paired() {
        // The agent has no keyboard, so it has no passkey to enter for any
        // device, and must not make one up.
        for device in [A, B] {
            assert_rejected(dispatch(
                A,
                agent_call("RequestPasskey").append1(path(device)),
            ));
        }
    }

    #[test]
    fn agent_acknowledges_release_and_cancel() {
        assert_approved(dispatch(A, agent_call("Release")));
        assert_approved(dispatch(A, agent_call("Cancel")));
    }

    #[test]
    fn device_path_from_peripheral_id() {
        assert_eq!(
            device_path("hci0/dev_AA_BB_CC_DD_EE_FF"),
            Some(path("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF"))
        );
        assert_eq!(device_path(""), None);
        assert_eq!(device_path("hci0/dev AA"), None);
    }

    #[test]
    fn bluez_error_keeps_dbus_name() {
        let error = BluezError::from(dbus::Error::new_custom(ALREADY_EXISTS, "Already Paired"));
        assert_eq!(error.name, ALREADY_EXISTS);
        assert_eq!(error.message, "Already Paired");
    }

    // The pairing session against a fake BlueZ on a throwaway bus.

    /// `interface.member` of the BlueZ calls a session makes.
    const GET: &str = "org.freedesktop.DBus.Properties.Get";
    const REGISTER: &str = "org.bluez.AgentManager1.RegisterAgent";
    const PAIR: &str = "org.bluez.Device1.Pair";
    const CANCEL: &str = "org.bluez.Device1.CancelPairing";
    const UNREGISTER: &str = "org.bluez.AgentManager1.UnregisterAgent";

    /// How the fake BlueZ answers `Device1.Pair`.
    #[derive(Clone, Copy)]
    enum PairAnswer {
        /// Ask the caller's agent to confirm a passkey for this device, then
        /// answer `Pair` with success, or with `AuthenticationRejected` if the
        /// agent refused.
        AskAbout(&'static str),
        /// Never answer.
        Hang,
        /// Bond at once (`Paired` turns true), but hold the reply until the
        /// test calls `answer_held_pair`, as BlueZ does until its service
        /// discovery has finished.
        BondThenHold,
    }

    /// What the fake BlueZ does, and what it has received, in one scenario.
    struct Scenario {
        /// The sensor's `Device1.Paired`.
        paired: bool,
        pair: PairAnswer,
        /// `(sender, interface, member)` of each method call, in order.
        received: Vec<(String, String, String)>,
        /// Notified when a `Pair` call is left unanswered.
        pair_held: Arc<Notify>,
        /// The `Pair` call that `BondThenHold` hasn't answered yet.
        held_pair: Option<Message>,
    }

    /// A fake `org.bluez` on its own connection to the test bus, with one
    /// sensor, `A`.
    struct FakeBluez {
        conn: Arc<SyncConnection>,
        scenario: Arc<Mutex<Scenario>>,
    }

    impl FakeBluez {
        async fn start() -> Self {
            let (resource, conn) =
                dbus_tokio::connection::new_system_sync().expect("connect to the test bus");
            tokio::spawn(async move {
                let err = resource.await;
                eprintln!("the fake BlueZ lost its bus connection: {err}");
            });
            let reply = conn
                .request_name("org.bluez", false, false, true)
                .await
                .expect("RequestName");
            assert_eq!(
                reply,
                RequestNameReply::PrimaryOwner,
                "another connection owns org.bluez on the test bus"
            );
            let scenario = Arc::new(Mutex::new(Scenario {
                paired: false,
                pair: PairAnswer::Hang,
                received: Vec::new(),
                pair_held: Arc::new(Notify::new()),
                held_pair: None,
            }));
            let state = Arc::clone(&scenario);
            let me = Arc::downgrade(&conn);
            conn.start_receive(
                MatchRule::new_method_call(),
                Box::new(move |msg, conn| {
                    fake_bluez_answer(msg, conn, &state, &me);
                    true
                }),
            );
            Self { conn, scenario }
        }

        /// Start a scenario: forget the calls received so far. Returns the
        /// notifier of this scenario's unanswered `Pair`.
        fn script(&self, paired: bool, pair: PairAnswer) -> Arc<Notify> {
            let mut scenario = self.scenario.lock().unwrap();
            scenario.paired = paired;
            scenario.pair = pair;
            scenario.received.clear();
            scenario.pair_held = Arc::new(Notify::new());
            scenario.held_pair = None;
            Arc::clone(&scenario.pair_held)
        }

        /// `interface.member` of each call in this scenario, in order, after
        /// checking that one connection made them all.
        fn calls(&self) -> Vec<String> {
            let scenario = self.scenario.lock().unwrap();
            let received = &scenario.received;
            assert!(
                received
                    .iter()
                    .all(|(sender, _, _)| *sender == received[0].0),
                "the calls came from more than one connection: {received:?}"
            );
            received
                .iter()
                .map(|(_, interface, member)| format!("{interface}.{member}"))
                .collect()
        }

        /// Answer the `Pair` call that `BondThenHold` held back.
        fn answer_held_pair(&self) {
            let pair = self.scenario.lock().unwrap().held_pair.take();
            let pair = pair.expect("no Pair call is being held");
            let _ = self.conn.send(pair.method_return());
        }

        /// Whether the connection that made this scenario's calls is still on
        /// the bus.
        async fn session_is_open(&self) -> bool {
            let name = self.scenario.lock().unwrap().received[0].0.clone();
            let dbus = Proxy::new(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                Duration::from_secs(5),
                Arc::clone(&self.conn),
            );
            let (owned,): (bool,) = dbus
                .method_call("org.freedesktop.DBus", "NameHasOwner", (name.as_str(),))
                .await
                .expect("NameHasOwner");
            owned
        }

        /// Fails unless the connection that made this scenario's calls leaves
        /// the bus within 2 s. After it has, every call it made has been
        /// received.
        async fn assert_session_closed(&self) {
            let closed = tokio::time::timeout(Duration::from_secs(2), async {
                while self.session_is_open().await {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await;
            assert!(
                closed.is_ok(),
                "the session's connection was still open 2 s after the session ended"
            );
        }
    }

    /// Answer one method call as BlueZ would on a host with the one sensor `A`.
    /// A call with other arguments, or to any other method (such as
    /// `RequestDefaultAgent`), gets an error.
    fn fake_bluez_answer(
        msg: Message,
        conn: &SyncConnection,
        scenario: &Mutex<Scenario>,
        me: &Weak<SyncConnection>,
    ) {
        let sender = msg.sender().map(|s| s.to_string()).unwrap_or_default();
        let interface = msg.interface().map(|i| i.to_string()).unwrap_or_default();
        let member = msg.member().map(|m| m.to_string()).unwrap_or_default();
        let on_sensor = msg.path().is_some_and(|p| &*p == A);
        let on_manager = msg.path().is_some_and(|p| &*p == "/org/bluez");
        let (paired, pair, pair_held) = {
            let mut scenario = scenario.lock().unwrap();
            scenario
                .received
                .push((sender.clone(), interface.clone(), member.clone()));
            (
                scenario.paired,
                scenario.pair,
                Arc::clone(&scenario.pair_held),
            )
        };
        let reply = match (interface.as_str(), member.as_str()) {
            ("org.freedesktop.DBus.Properties", "Get")
                if on_sensor
                    && msg
                        .read2::<&str, &str>()
                        .is_ok_and(|args| args == ("org.bluez.Device1", "Paired")) =>
            {
                msg.method_return().append1(Variant(paired))
            }
            ("org.bluez.AgentManager1", "RegisterAgent")
                if on_manager
                    && msg
                        .read2::<dbus::Path, &str>()
                        .is_ok_and(|(agent, capability)| {
                            &*agent == "/dev/rye/aranet/agent" && capability == "NoInputNoOutput"
                        }) =>
            {
                msg.method_return()
            }
            ("org.bluez.AgentManager1", "UnregisterAgent")
                if on_manager
                    && msg
                        .read1::<dbus::Path>()
                        .is_ok_and(|agent| &*agent == "/dev/rye/aranet/agent") =>
            {
                msg.method_return()
            }
            ("org.bluez.Device1", "CancelPairing") if on_sensor => msg.method_return(),
            ("org.bluez.Device1", "Pair") if on_sensor => {
                match pair {
                    PairAnswer::Hang => pair_held.notify_one(),
                    PairAnswer::BondThenHold => {
                        let mut scenario = scenario.lock().unwrap();
                        scenario.paired = true;
                        scenario.held_pair = Some(msg);
                        pair_held.notify_one();
                    }
                    PairAnswer::AskAbout(device) => {
                        if let Some(conn) = me.upgrade() {
                            tokio::spawn(answer_pair(conn, msg, sender, device));
                        }
                    }
                }
                return;
            }
            _ => msg.error(
                &ErrorName::from("org.bluez.Error.NotSupported"),
                &CString::new("not expected by this test").unwrap(),
            ),
        };
        let _ = conn.send(reply);
    }

    /// Ask the agent that `owner` registered to confirm a passkey for
    /// `device`, then answer the `Pair` call as BlueZ would.
    async fn answer_pair(
        conn: Arc<SyncConnection>,
        pair: Message,
        owner: String,
        device: &'static str,
    ) {
        let agent = Proxy::new(
            owner,
            "/dev/rye/aranet/agent",
            Duration::from_secs(5),
            Arc::clone(&conn),
        );
        let confirmed: Result<(), dbus::Error> = agent
            .method_call(
                "org.bluez.Agent1",
                "RequestConfirmation",
                (path(device), PASSKEY),
            )
            .await;
        let reply = match confirmed {
            Ok(()) => pair.method_return(),
            Err(_) => pair.error(
                &ErrorName::from("org.bluez.Error.AuthenticationRejected"),
                &CString::new("Authentication Rejected").unwrap(),
            ),
        };
        let _ = conn.send(reply);
    }

    /// Runs pairing sessions against `FakeBluez`, scenarios (a) to (f) below.
    /// The test takes the name `org.bluez` on the bus that
    /// `DBUS_SYSTEM_BUS_ADDRESS` names, so it needs a throwaway `dbus-daemon`
    /// whose policy lets any connection own any name, never the host's system
    /// bus. It runs on the real clock and takes about 6 s, most of it scenario
    /// (d)'s 1 s budget and scenario (f)'s 3 s budget and 1.5 s wait.
    ///
    /// To run it in Docker from the repository root, set `IMAGE` to a Debian
    /// image with Rust 1.90 or later, `pkg-config` and `libdbus-1-dev` (such as
    /// `rust:1.90` with those two packages added). The command installs `dbus`,
    /// for `dbus-daemon`, if the image doesn't have it:
    ///
    /// ```text
    /// docker run --rm -v "$PWD":/work -w /work -e CARGO_TARGET_DIR=/work/target/linux "$IMAGE" bash -c 'set -e
    /// command -v dbus-daemon > /dev/null || { apt-get update -qq && apt-get install -y -qq dbus > /dev/null; }
    /// cat > /tmp/aranet-test-bus.conf <<EOF
    /// <!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
    /// <busconfig><type>system</type><listen>unix:path=/tmp/aranet-test-bus</listen><auth>EXTERNAL</auth>
    /// <policy context="default"><allow user="*"/><allow own="*"/><allow send_destination="*"/><allow receive_sender="*"/></policy></busconfig>
    /// EOF
    /// dbus-daemon --config-file=/tmp/aranet-test-bus.conf --fork
    /// DBUS_SYSTEM_BUS_ADDRESS=unix:path=/tmp/aranet-test-bus cargo test --locked -p aranet-core --lib pairing_session_over_a_private_bus -- --ignored --nocapture'
    /// ```
    #[tokio::test]
    #[ignore = "needs a throwaway dbus-daemon; see this test's doc for the Docker command"]
    async fn pairing_session_over_a_private_bus() {
        assert!(
            std::env::var_os("DBUS_SYSTEM_BUS_ADDRESS").is_some(),
            "set DBUS_SYSTEM_BUS_ADDRESS to a throwaway dbus-daemon: this test takes the name org.bluez on that bus"
        );
        within(Duration::from_secs(120), async {
            let bluez = FakeBluez::start().await;
            let budget = Duration::from_secs(10);

            // (a) An unpaired sensor pairs through the session's own agent, and
            // nothing asks to be the default agent.
            bluez.script(false, PairAnswer::AskAbout(A));
            assert_eq!(run_session(path(A), budget).await, PairOutcome::Paired);
            bluez.assert_session_closed().await;
            assert_eq!(bluez.calls(), [GET, REGISTER, PAIR, UNREGISTER]);

            // (b) The agent refuses a request about another device.
            bluez.script(false, PairAnswer::AskAbout(B));
            let outcome = run_session(path(A), budget).await;
            assert!(
                matches!(&outcome, PairOutcome::Failed(e) if e.name == "org.bluez.Error.AuthenticationRejected"),
                "{outcome:?}"
            );
            bluez.assert_session_closed().await;
            assert_eq!(bluez.calls(), [GET, REGISTER, PAIR, UNREGISTER]);

            // (c) A paired sensor registers no agent and isn't paired again.
            bluez.script(true, PairAnswer::Hang);
            assert_eq!(
                run_session(path(A), budget).await,
                PairOutcome::AlreadyPaired
            );
            bluez.assert_session_closed().await;
            assert_eq!(bluez.calls(), [GET]);

            // (d) A Pair that never answers is cancelled when its budget runs
            // out, because the sensor still isn't paired.
            bluez.script(false, PairAnswer::Hang);
            assert_eq!(
                run_session(path(A), Duration::from_secs(1)).await,
                PairOutcome::TimedOut
            );
            bluez.assert_session_closed().await;
            assert_eq!(bluez.calls(), [GET, REGISTER, PAIR, GET, CANCEL, UNREGISTER]);

            // (e) A caller that drops the session during Pair closes the
            // connection too, so BlueZ drops the agent and the pairing.
            let pair_held = bluez.script(false, PairAnswer::Hang);
            tokio::select! {
                outcome = run_session(path(A), budget) => {
                    panic!("the session should still be pairing, got {outcome:?}")
                }
                () = pair_held.notified() => {}
            }
            bluez.assert_session_closed().await;
            assert_eq!(bluez.calls(), [GET, REGISTER, PAIR]);

            // (f) BlueZ bonded but holds Pair's reply for its service
            // discovery: the budget runs out, the session reports Paired
            // without cancelling, and keeps its connection open until BlueZ
            // answers. No event marks "still open", so this waits 1.5 s: a
            // session that closed after reporting, or whose Pair call timed
            // out 1 s after its budget, is gone by then.
            bluez.script(false, PairAnswer::BondThenHold);
            assert_eq!(
                run_session(path(A), Duration::from_secs(3)).await,
                PairOutcome::Paired
            );
            tokio::time::sleep(Duration::from_millis(1500)).await;
            assert!(
                bluez.session_is_open().await,
                "the session closed its connection before BlueZ answered Pair"
            );
            bluez.answer_held_pair();
            bluez.assert_session_closed().await;
            assert_eq!(bluez.calls(), [GET, REGISTER, PAIR, GET, UNREGISTER]);
        })
        .await;
    }
}
