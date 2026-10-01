//! The explicit BlueZ pairing sequence, written against a small bus trait so
//! that it is tested on every OS.
//!
//! On Linux, aranet pairs a sensor that BlueZ doesn't list as paired by calling
//! `org.bluez.Device1.Pair` itself before it connects, through an agent that it
//! registers for that one call on a private D-Bus connection. `run_pairing` is
//! that sequence and `PairingBus` is the connection. `PairingCooldown` keeps a
//! sensor that rejects pairing from costing a pairing attempt on every connect.
//!
//! Why the sequence looks like this (BlueZ `src/adapter.c`, `src/agent.c` and
//! `src/device.c` on master; `device-5.66.c` is `src/device.c` in the 5.66
//! release):
//!
//! - The agent is registered only for one `Pair`, on a private connection:
//!   since BlueZ 5.51 the first agent registered becomes the default agent,
//!   which answers pairing requests from any device (agent.c:278-281, commit
//!   9213ff7642).
//! - `Pair` uses the agent that the calling connection registered
//!   (device.c:3438-3446, `agent_get(sender)`), so the agent and `Pair` share
//!   one connection.
//! - `Pair` fails with `org.bluez.Error.InProgress` while a `Connect` is
//!   pending (device.c:3410), so it runs before the connect.
//! - `Paired` is read first (BlueZ 5.64 has no `Bonded`). On BlueZ 5.64 and
//!   5.66, `Pair` on an LE-only sensor that is already bonded picks the BR/EDR
//!   bearer and can stall (device-5.66.c:2815-2818); only newer BlueZ answers
//!   `org.bluez.Error.AlreadyExists` there (the guard at device.c:3422).
//!   `AlreadyExists` still counts as paired (device.c:3435).
//! - A `Pair` that runs out of time is cancelled with `CancelPairing`, but
//!   only while `Paired` is still false. BlueZ answers `Pair` only after
//!   service discovery: once the bond is made it sets `Paired` and drops the
//!   bonding request, then waits for the GATT client (device.c:7397-7434,
//!   device-5.66.c:6060-6093). On an Aranet4 whose services BlueZ hasn't
//!   cached, that wait can outlast the budget, and `CancelPairing` with no
//!   bonding request unpairs and disconnects the sensor (device.c:3549-3566,
//!   `btd_adapter_remove_bonding` at adapter.c:8334-8350).
//! - That `Paired` session still has BlueZ's answer to `Pair` coming, and
//!   closing the connection before it arrives cancels BlueZ's service
//!   discovery (the browse request's disconnect watch, device.c:6755-6763 and
//!   :2011-2018). So `run_pairing` reports `Paired` at once, unregisters the
//!   agent, and waits up to one more budget for that answer.
//! - Errors from `CancelPairing` and `UnregisterAgent` are ignored: closing the
//!   connection removes its agent (the disconnect watch that `agent_create`
//!   sets, agent.c:274-276, calls `agent_disconnect`, agent.c:174-188), and
//!   ends a bonding it started and disconnects the sensor (device.c:3346-3361).
//! - Only a sensor that refused to pair is left alone for 10 minutes. Until a
//!   sensor is paired, BlueZ asks for pairing itself at every connection (its
//!   battery plugin reads the Battery Level, which needs encryption) and finds
//!   no agent on a headless host, so after a timeout or a lost link the next
//!   connect pairs again.
//! - btleplug 0.11.8 has no pairing call.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tokio::time::Instant;

/// The D-Bus error that `Device1.Pair` answers with when the device is
/// already bonded.
pub(crate) const ALREADY_EXISTS: &str = "org.bluez.Error.AlreadyExists";

/// The `Device1.Pair` errors that mean the sensor refused to pair (BlueZ's
/// `new_authentication_return`, device.c:3491-3525): the pairing itself failed,
/// as when a sensor wants a PIN that a `NoInputNoOutput` agent can't give, or
/// was rejected. Pairing again won't help until someone pairs it by hand.
/// Other errors say nothing about the sensor: `AuthenticationCanceled` is also
/// what a lost link gives, and `ConnectionAttemptFailed`, `InProgress` or a bus
/// error can pass. `AuthenticationRejected` also covers a busy local controller
/// (BlueZ maps the kernel's `MGMT_STATUS_BUSY` to it), so a pairing that
/// collides with other Bluetooth work can start the cooldown too.
const REFUSED_BY_SENSOR: [&str; 2] = [
    "org.bluez.Error.AuthenticationFailed",
    "org.bluez.Error.AuthenticationRejected",
];

/// A D-Bus error reply, such as `org.bluez.Error.AuthenticationFailed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BluezError {
    /// The D-Bus error name.
    pub(crate) name: String,
    /// The message that came with it.
    pub(crate) message: String,
}

impl fmt::Display for BluezError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.name, self.message)
    }
}

/// How one run of `run_pairing` ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PairOutcome {
    /// `Paired` was already true, or `Pair` answered `AlreadyExists`.
    AlreadyPaired,
    /// `Pair` succeeded, or ran out of time after BlueZ had made the bond (it
    /// answers `Pair` only after service discovery).
    Paired,
    /// `Pair` answered with this error. That includes errors that say nothing
    /// about the sensor, such as `org.freedesktop.DBus.Error.NoReply` after a
    /// `bluetoothd` restart, or `org.bluez.Error.InProgress` while another
    /// D-Bus client connects or pairs the same sensor.
    Failed(BluezError),
    /// `Pair` didn't answer within the budget and `Paired` was still false or
    /// couldn't be read, so it was cancelled.
    TimedOut,
    /// BlueZ couldn't be asked: reading `Paired` or registering the agent
    /// failed, so `Pair` was never sent.
    Unavailable(BluezError),
}

impl PairOutcome {
    /// Whether the sensor refused to pair: `Pair` failed with one of
    /// `REFUSED_BY_SENSOR`.
    pub(crate) fn refused_by_sensor(&self) -> bool {
        matches!(self, PairOutcome::Failed(e) if REFUSED_BY_SENSOR.contains(&e.name.as_str()))
    }
}

/// The BlueZ calls that pairing one device makes, all on one D-Bus connection.
///
/// `run_pairing` puts time limits on `pair` only, so every other call must
/// return within the implementation's own limit; `is_paired` is called again
/// when `pair` runs out of time. `pair`'s own reply timeout must outlast
/// `run_pairing`'s waits for it: up to twice the budget, plus two other calls.
/// `Sync` makes `&Self` `Send`, which keeps `run_pairing`'s future `Send`: the
/// collector and the TUI spawn their connects.
pub(crate) trait PairingBus: Sync {
    /// Reads the device's `org.bluez.Device1.Paired` property: before pairing,
    /// and again when `pair` runs out of time.
    fn is_paired(&self) -> impl Future<Output = Result<bool, BluezError>> + Send;

    /// Registers the pairing agent (`org.bluez.AgentManager1.RegisterAgent`)
    /// on the connection that sends `pair`.
    fn register_agent(&self) -> impl Future<Output = Result<(), BluezError>> + Send;

    /// Pairs with the device (`org.bluez.Device1.Pair`).
    fn pair(&self) -> impl Future<Output = Result<(), BluezError>> + Send;

    /// Cancels a `pair` that is still running (`org.bluez.Device1.CancelPairing`).
    /// With no bonding in progress, BlueZ unpairs and disconnects the device
    /// instead.
    fn cancel_pairing(&self) -> impl Future<Output = Result<(), BluezError>> + Send;

    /// Unregisters the agent (`org.bluez.AgentManager1.UnregisterAgent`).
    fn unregister_agent(&self) -> impl Future<Output = Result<(), BluezError>> + Send;

    /// Hands the outcome on as soon as it is known, before `unregister_agent`
    /// and before any wait for a `Pair` reply that BlueZ holds back.
    /// `run_pairing` calls it once. Does nothing by default.
    fn report(&self, _outcome: &PairOutcome) {}
}

/// Pair once: skip if paired; otherwise register the agent, `Pair` within
/// what is left of `budget`, then unregister.
///
/// When `budget` runs out, `Paired` is read again. If it is true, BlueZ has
/// bonded and holds `Pair`'s reply until its service discovery ends: the
/// outcome is `Paired`, nothing is cancelled, and after unregistering the
/// agent this waits up to `budget` more for that reply, so that the caller's
/// connection stays open meanwhile. Otherwise `CancelPairing` stops it. The
/// outcome is reported (`PairingBus::report`) as soon as it is known, before
/// the cleanup and that wait. Errors from `CancelPairing` and
/// `UnregisterAgent` are ignored: closing the connection removes the agent
/// anyway.
pub(crate) async fn run_pairing<B: PairingBus>(bus: &B, budget: Duration) -> PairOutcome {
    let start = Instant::now();
    let before_pair = match bus.is_paired().await {
        Ok(true) => Some(PairOutcome::AlreadyPaired),
        Ok(false) => bus
            .register_agent()
            .await
            .err()
            .map(PairOutcome::Unavailable),
        Err(e) => Some(PairOutcome::Unavailable(e)),
    };
    if let Some(outcome) = before_pair {
        bus.report(&outcome);
        return outcome;
    }
    let mut pair = std::pin::pin!(bus.pair());
    let answer = tokio::time::timeout(budget.saturating_sub(start.elapsed()), &mut pair).await;
    let outcome = match answer {
        Ok(Ok(())) => PairOutcome::Paired,
        Ok(Err(e)) if e.name == ALREADY_EXISTS => PairOutcome::AlreadyPaired,
        Ok(Err(e)) => PairOutcome::Failed(e),
        // BlueZ answers `Pair` only after service discovery, so time can run
        // out after the bond is made. `CancelPairing` would then unpair and
        // disconnect the sensor, so it is sent only while `Paired` is false.
        Err(_) => {
            if matches!(bus.is_paired().await, Ok(true)) {
                bus.report(&PairOutcome::Paired);
                let _ = bus.unregister_agent().await;
                let _ = tokio::time::timeout(budget, &mut pair).await;
                return PairOutcome::Paired;
            }
            let _ = bus.cancel_pairing().await;
            PairOutcome::TimedOut
        }
    };
    bus.report(&outcome);
    let _ = bus.unregister_agent().await;
    outcome
}

/// How long a sensor that refused to pair is left unpaired before aranet tries
/// again.
pub(crate) const PAIRING_RETRY_AFTER: Duration = Duration::from_secs(600);

/// Per-device memory of failed pairings, keyed by the device's BlueZ object
/// path.
///
/// A sensor that rejects Just Works pairing (possibly an Aranet4 that wants its
/// PIN) would otherwise cost a pairing attempt and a warning on every connect,
/// which for the service is every poll.
#[derive(Debug, Default)]
pub(crate) struct PairingCooldown {
    until: HashMap<String, Instant>,
}

impl PairingCooldown {
    /// True while `device` refused to pair less than `PAIRING_RETRY_AFTER`
    /// before `now`.
    pub(crate) fn blocks(&self, device: &str, now: Instant) -> bool {
        self.until.get(device).is_some_and(|until| now < *until)
    }

    /// Remembers how pairing `device` ended.
    ///
    /// A sensor that refused (`PairOutcome::refused_by_sensor`) is blocked
    /// until `now + PAIRING_RETRY_AFTER`. `AlreadyPaired` or `Paired` forgets
    /// it. Anything else changes nothing: a timeout, a lost link, a bus error
    /// or another client's pairing say nothing about the sensor, and the next
    /// connect should pair it. Entries whose time has passed stay in the map:
    /// there is one per sensor at most.
    pub(crate) fn record(&mut self, device: &str, outcome: &PairOutcome, now: Instant) {
        if outcome.refused_by_sensor() {
            self.until
                .insert(device.to_owned(), now + PAIRING_RETRY_AFTER);
        } else if matches!(outcome, PairOutcome::AlreadyPaired | PairOutcome::Paired) {
            self.until.remove(device);
        }
    }
}

/// Runs `session` for `device` unless the sensor refused to pair less than
/// `PAIRING_RETRY_AFTER` ago, and records how it went. `None` when the
/// cooldown skipped it. The lock is taken only to check and to record, never
/// across the session.
pub(crate) async fn pair_with_cooldown(
    cooldown: &Mutex<PairingCooldown>,
    device: &str,
    session: impl Future<Output = PairOutcome>,
) -> Option<PairOutcome> {
    if cooldown
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .blocks(device, Instant::now())
    {
        return None;
    }
    let outcome = session.await;
    cooldown
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .record(device, &outcome, Instant::now());
    Some(outcome)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::test_support::within;

    /// The default pairing budget: `connection_timeout` (15 s) plus
    /// `link::bluez_discovery_limit` (20 s).
    const BUDGET: Duration = Duration::from_secs(35);

    /// The calls of a pairing whose `Pair` answered in time.
    const FULL_SEQUENCE: [&str; 4] = ["IsPaired", "RegisterAgent", "Pair", "UnregisterAgent"];

    /// BlueZ object paths of two sensors.
    const SENSOR_A: &str = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_01";
    const SENSOR_B: &str = "/org/bluez/hci0/dev_11_22_33_44_55_66";

    /// What the fake `Pair` does.
    enum PairScript {
        Ok,
        Err(BluezError),
        /// Never answers, like a `Pair` whose sensor stopped responding.
        Hang,
        /// Answers after this long, like BlueZ holding `Pair`'s reply until
        /// its service discovery ends.
        After(Duration),
    }

    /// One `report` call: the outcome, the calls made before it, and when.
    type Report = (PairOutcome, Vec<&'static str>, Instant);

    /// A scripted BlueZ connection that records every call it gets.
    struct FakeBus {
        calls: Mutex<Vec<&'static str>>,
        paired: Result<bool, BluezError>,
        /// What `is_paired` returns once `Pair` has been sent: `run_pairing`
        /// reads `Paired` again only when `Pair` ran out of time.
        paired_after_timeout: Result<bool, BluezError>,
        register: Result<(), BluezError>,
        pair: PairScript,
        /// What `cancel_pairing` and `unregister_agent` return.
        cleanup: Result<(), BluezError>,
        /// How long `is_paired` and `register_agent` take.
        delay: Duration,
        reports: Mutex<Vec<Report>>,
    }

    impl FakeBus {
        fn new(
            paired: Result<bool, BluezError>,
            register: Result<(), BluezError>,
            pair: PairScript,
        ) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                paired,
                paired_after_timeout: Ok(false),
                register,
                pair,
                cleanup: Ok(()),
                delay: Duration::ZERO,
                reports: Mutex::new(Vec::new()),
            }
        }

        /// A device that isn't paired, on a bus that accepts the agent.
        fn unpaired(pair: PairScript) -> Self {
            Self::new(Ok(false), Ok(()), pair)
        }

        fn record(&self, call: &'static str) {
            self.calls.lock().unwrap().push(call);
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().unwrap().clone()
        }

        fn reports(&self) -> Vec<Report> {
            self.reports.lock().unwrap().clone()
        }

        async fn take_time(&self) {
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
        }
    }

    impl PairingBus for FakeBus {
        async fn is_paired(&self) -> Result<bool, BluezError> {
            let pair_sent = self.calls().contains(&"Pair");
            self.record("IsPaired");
            self.take_time().await;
            if pair_sent {
                self.paired_after_timeout.clone()
            } else {
                self.paired.clone()
            }
        }

        async fn register_agent(&self) -> Result<(), BluezError> {
            self.record("RegisterAgent");
            self.take_time().await;
            self.register.clone()
        }

        async fn pair(&self) -> Result<(), BluezError> {
            self.record("Pair");
            match &self.pair {
                PairScript::Ok => Ok(()),
                PairScript::Err(e) => Err(e.clone()),
                PairScript::Hang => std::future::pending().await,
                PairScript::After(delay) => {
                    tokio::time::sleep(*delay).await;
                    Ok(())
                }
            }
        }

        async fn cancel_pairing(&self) -> Result<(), BluezError> {
            self.record("CancelPairing");
            self.cleanup.clone()
        }

        async fn unregister_agent(&self) -> Result<(), BluezError> {
            self.record("UnregisterAgent");
            self.cleanup.clone()
        }

        fn report(&self, outcome: &PairOutcome) {
            let report = (outcome.clone(), self.calls(), Instant::now());
            self.reports.lock().unwrap().push(report);
        }
    }

    fn err(name: &str) -> BluezError {
        BluezError {
            name: name.to_string(),
            message: "scripted failure".to_string(),
        }
    }

    #[tokio::test]
    async fn already_paired_device_skips_agent_and_pair() {
        let bus = FakeBus::new(Ok(true), Ok(()), PairScript::Ok);

        assert_eq!(run_pairing(&bus, BUDGET).await, PairOutcome::AlreadyPaired);
        assert_eq!(bus.calls(), ["IsPaired"]);
    }

    #[tokio::test]
    async fn unpaired_device_registers_pairs_then_unregisters() {
        let bus = FakeBus::unpaired(PairScript::Ok);

        assert_eq!(run_pairing(&bus, BUDGET).await, PairOutcome::Paired);
        assert_eq!(bus.calls(), FULL_SEQUENCE);
    }

    #[tokio::test]
    async fn pair_already_exists_counts_as_paired() {
        // Something else bonded the sensor after `Paired` was read. BlueZ newer
        // than 5.66 answers AlreadyExists for it (device.c:3422 and :3435).
        let bus = FakeBus::unpaired(PairScript::Err(err(ALREADY_EXISTS)));

        assert_eq!(run_pairing(&bus, BUDGET).await, PairOutcome::AlreadyPaired);
        assert_eq!(bus.calls(), FULL_SEQUENCE);
    }

    #[tokio::test]
    async fn failed_pair_still_unregisters_agent() {
        let bus = FakeBus::unpaired(PairScript::Err(err("org.bluez.Error.AuthenticationFailed")));

        assert_eq!(
            run_pairing(&bus, BUDGET).await,
            PairOutcome::Failed(err("org.bluez.Error.AuthenticationFailed"))
        );
        assert_eq!(bus.calls(), FULL_SEQUENCE);
    }

    #[tokio::test(start_paused = true)]
    async fn pair_timeout_cancels_and_unregisters() {
        // When time runs out, `Paired` is read again. Still false, or no reply
        // in time from bluetoothd, means the bonding may still be running, so
        // it is cancelled. CancelPairing answers DoesNotExist when the bonding
        // completed after that read (device.c:3557-3561). That must not change
        // the outcome or skip UnregisterAgent.
        let no_answer = err("org.freedesktop.DBus.Error.Timeout");
        for paired_after_timeout in [Ok(false), Err(no_answer)] {
            let bus = FakeBus {
                paired_after_timeout,
                cleanup: Err(err("org.bluez.Error.DoesNotExist")),
                ..FakeBus::unpaired(PairScript::Hang)
            };
            // Not the default budget, so a hard-coded limit fails this test.
            let budget = Duration::from_secs(7);
            let start = Instant::now();

            let outcome = within(Duration::from_secs(600), run_pairing(&bus, budget)).await;

            let reread = &bus.paired_after_timeout;
            assert_eq!(outcome, PairOutcome::TimedOut, "{reread:?}");
            assert_eq!(start.elapsed(), budget, "{reread:?}");
            assert_eq!(
                bus.calls(),
                [
                    "IsPaired",
                    "RegisterAgent",
                    "Pair",
                    "IsPaired",
                    "CancelPairing",
                    "UnregisterAgent"
                ],
                "{reread:?}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn pair_timeout_after_bonding_keeps_the_bond() {
        // BlueZ answers `Pair` only after service discovery, which can outlast
        // the budget on an Aranet4 whose services it hasn't cached, and `Paired`
        // turns true as soon as the bond is made (device.c:7397-7434). A
        // CancelPairing now would unpair and disconnect the sensor
        // (device.c:3557-3561).
        let bus = FakeBus {
            paired_after_timeout: Ok(true),
            ..FakeBus::unpaired(PairScript::Hang)
        };
        let budget = Duration::from_secs(7);
        let start = Instant::now();

        let outcome = within(Duration::from_secs(600), run_pairing(&bus, budget)).await;

        assert_eq!(outcome, PairOutcome::Paired);
        // Reported when the budget ran out, so the connect can go ahead. The
        // session then waits one more budget for BlueZ's answer to `Pair`
        // (none comes here): closing its connection earlier would cancel
        // BlueZ's service discovery (device.c:6755-6763 and :2011-2018).
        assert_eq!(
            bus.reports(),
            [(
                PairOutcome::Paired,
                vec!["IsPaired", "RegisterAgent", "Pair", "IsPaired"],
                start + budget
            )]
        );
        assert_eq!(start.elapsed(), budget * 2);
        assert_eq!(
            bus.calls(),
            [
                "IsPaired",
                "RegisterAgent",
                "Pair",
                "IsPaired",
                "UnregisterAgent"
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_bonded_pair_waits_for_bluez_to_answer() {
        // The usual end of a first Aranet4 pairing that outlasts its budget:
        // BlueZ answers once its service discovery is done.
        let bus = FakeBus {
            paired_after_timeout: Ok(true),
            ..FakeBus::unpaired(PairScript::After(Duration::from_secs(10)))
        };
        let budget = Duration::from_secs(7);
        let start = Instant::now();

        let outcome = within(Duration::from_secs(600), run_pairing(&bus, budget)).await;

        assert_eq!(outcome, PairOutcome::Paired);
        assert_eq!(bus.reports()[0].2, start + budget);
        assert_eq!(start.elapsed(), Duration::from_secs(10));
        assert!(!bus.calls().contains(&"CancelPairing"), "{:?}", bus.calls());
    }

    #[tokio::test(start_paused = true)]
    async fn every_outcome_is_reported_once_before_the_cleanup() {
        let failed = err("org.bluez.Error.AuthenticationFailed");
        let unknown = err("org.freedesktop.DBus.Error.ServiceUnknown");
        let cases = [
            (
                FakeBus::new(Ok(true), Ok(()), PairScript::Ok),
                PairOutcome::AlreadyPaired,
                &["IsPaired"][..],
            ),
            (
                FakeBus::new(Err(unknown.clone()), Ok(()), PairScript::Ok),
                PairOutcome::Unavailable(unknown.clone()),
                &["IsPaired"],
            ),
            (
                FakeBus::new(Ok(false), Err(unknown.clone()), PairScript::Ok),
                PairOutcome::Unavailable(unknown),
                &["IsPaired", "RegisterAgent"],
            ),
            (
                FakeBus::unpaired(PairScript::Ok),
                PairOutcome::Paired,
                &["IsPaired", "RegisterAgent", "Pair"],
            ),
            (
                FakeBus::unpaired(PairScript::Err(failed.clone())),
                PairOutcome::Failed(failed),
                &["IsPaired", "RegisterAgent", "Pair"],
            ),
            (
                FakeBus::unpaired(PairScript::Hang),
                PairOutcome::TimedOut,
                &[
                    "IsPaired",
                    "RegisterAgent",
                    "Pair",
                    "IsPaired",
                    "CancelPairing",
                ],
            ),
        ];
        for (bus, expected, before) in cases {
            let outcome = within(Duration::from_secs(600), run_pairing(&bus, BUDGET)).await;

            assert_eq!(outcome, expected);
            let reports = bus.reports();
            assert_eq!(reports.len(), 1, "{expected:?}: {reports:?}");
            assert_eq!(reports[0].0, expected);
            assert_eq!(reports[0].1, before, "{expected:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn pair_gets_what_is_left_of_the_budget() {
        // The calls before `Pair` count against the budget, so a session
        // reports within a few call limits of it (`link::PAIRING_GRACE`, the
        // time a connect gives the pairing step beyond its budget).
        let bus = FakeBus {
            delay: Duration::from_secs(2),
            ..FakeBus::unpaired(PairScript::Hang)
        };
        let start = Instant::now();

        let outcome = within(
            Duration::from_secs(600),
            run_pairing(&bus, Duration::from_secs(7)),
        )
        .await;

        assert_eq!(outcome, PairOutcome::TimedOut);
        // IsPaired and RegisterAgent (2 s each), `Pair` until the 7 s budget
        // runs out, then IsPaired again (2 s).
        assert_eq!(start.elapsed(), Duration::from_secs(9));
    }

    #[tokio::test]
    async fn bluez_unreachable_skips_pairing() {
        // bluetoothd isn't running, so nothing owns org.bluez on the system bus.
        let unknown = err("org.freedesktop.DBus.Error.ServiceUnknown");
        let bus = FakeBus::new(Err(unknown.clone()), Ok(()), PairScript::Ok);

        assert_eq!(
            run_pairing(&bus, BUDGET).await,
            PairOutcome::Unavailable(unknown)
        );
        assert_eq!(bus.calls(), ["IsPaired"]);
    }

    #[tokio::test]
    async fn register_failure_skips_pair() {
        // RegisterAgent answers AlreadyExists when the connection already has an
        // agent (agent.c:956-958). Unlike Pair's, that says nothing about the
        // sensor.
        let taken = err(ALREADY_EXISTS);
        let bus = FakeBus::new(Ok(false), Err(taken.clone()), PairScript::Ok);

        assert_eq!(
            run_pairing(&bus, BUDGET).await,
            PairOutcome::Unavailable(taken)
        );
        assert_eq!(bus.calls(), ["IsPaired", "RegisterAgent"]);
    }

    #[tokio::test]
    async fn each_pairing_registers_its_own_agent_after_a_failed_one() {
        // bluetoothd restarted while `Pair` was waiting: the bus answered for it,
        // and the cleanup calls failed too. That is still `Failed`: only a
        // failure before `Pair` is sent is `Unavailable`.
        let no_reply = err("org.freedesktop.DBus.Error.NoReply");
        let first = FakeBus {
            cleanup: Err(no_reply.clone()),
            ..FakeBus::unpaired(PairScript::Err(no_reply.clone()))
        };
        assert_eq!(
            run_pairing(&first, BUDGET).await,
            PairOutcome::Failed(no_reply)
        );
        assert_eq!(first.calls(), FULL_SEQUENCE);

        // The next attempt talks to the new bluetoothd, which has no agent from
        // aranet. Nothing may be remembered from the first run.
        let second = FakeBus::unpaired(PairScript::Ok);
        assert_eq!(run_pairing(&second, BUDGET).await, PairOutcome::Paired);
        assert_eq!(second.calls(), FULL_SEQUENCE);
    }

    #[test]
    fn bluez_error_displays_name_and_message() {
        let error = BluezError {
            name: "org.bluez.Error.Failed".into(),
            message: "boom".into(),
        };

        assert_eq!(error.to_string(), "org.bluez.Error.Failed: boom");
    }

    #[tokio::test(start_paused = true)]
    async fn cooldown_blocks_a_sensor_after_a_failed_pairing() {
        let failures = [
            PairOutcome::Failed(err("org.bluez.Error.AuthenticationFailed")),
            PairOutcome::Failed(err("org.bluez.Error.AuthenticationRejected")),
        ];
        for outcome in failures {
            let mut cooldown = PairingCooldown::default();
            let t0 = Instant::now();
            cooldown.record(SENSOR_A, &outcome, t0);

            assert!(cooldown.blocks(SENSOR_A, t0), "{outcome:?}");
            assert!(!cooldown.blocks(SENSOR_B, t0), "{outcome:?}");
            tokio::time::advance(Duration::from_secs(599)).await;
            assert!(
                cooldown.blocks(SENSOR_A, Instant::now()),
                "{outcome:?} after 599 s"
            );
            tokio::time::advance(Duration::from_secs(1)).await;
            assert!(
                !cooldown.blocks(SENSOR_A, Instant::now()),
                "{outcome:?} after 600 s"
            );

            // A sensor that fails again is held back again.
            cooldown.record(SENSOR_A, &outcome, Instant::now());
            assert!(
                cooldown.blocks(SENSOR_A, Instant::now()),
                "{outcome:?} failed again after its cooldown"
            );
        }
    }

    #[test]
    fn cooldown_forgets_a_sensor_once_it_pairs() {
        for paired in [PairOutcome::Paired, PairOutcome::AlreadyPaired] {
            let mut cooldown = PairingCooldown::default();
            let t0 = Instant::now();
            let failed = PairOutcome::Failed(err("org.bluez.Error.AuthenticationFailed"));
            cooldown.record(SENSOR_A, &failed, t0);
            cooldown.record(SENSOR_B, &failed, t0);

            // Another connect of the same sensor paired it meanwhile.
            let later = t0 + Duration::from_secs(1);
            cooldown.record(SENSOR_A, &paired, later);

            assert!(!cooldown.blocks(SENSOR_A, later), "{paired:?}");
            // The other sensor's cooldown still holds.
            assert!(cooldown.blocks(SENSOR_B, later), "{paired:?}");
        }
    }

    #[test]
    fn cooldown_ignores_bus_problems() {
        let unreachable =
            PairOutcome::Unavailable(err("org.freedesktop.DBus.Error.ServiceUnknown"));
        let t0 = Instant::now();
        let later = t0 + Duration::from_secs(1);

        // Nothing was learned about the sensor, so it isn't held back.
        let mut cooldown = PairingCooldown::default();
        cooldown.record(SENSOR_A, &unreachable, t0);
        assert!(!cooldown.blocks(SENSOR_A, t0));

        // Nor does a bus problem lift a cooldown the sensor earned.
        let failed = PairOutcome::Failed(err("org.bluez.Error.AuthenticationFailed"));
        cooldown.record(SENSOR_A, &failed, t0);
        cooldown.record(SENSOR_A, &unreachable, later);
        assert!(cooldown.blocks(SENSOR_A, later));
    }

    #[test]
    fn cooldown_ignores_failures_that_do_not_blame_the_sensor() {
        // Until the sensor is paired, BlueZ asks for pairing itself at every
        // connect, and finds no agent on a headless host, so the next connect
        // must try again.
        let t0 = Instant::now();
        let later = t0 + Duration::from_secs(1);
        let refused = PairOutcome::Failed(err("org.bluez.Error.AuthenticationFailed"));
        for outcome in [
            PairOutcome::TimedOut,
            PairOutcome::Failed(err("org.bluez.Error.AuthenticationCanceled")),
            PairOutcome::Failed(err("org.bluez.Error.ConnectionAttemptFailed")),
            PairOutcome::Failed(err("org.bluez.Error.InProgress")),
            PairOutcome::Failed(err("org.freedesktop.DBus.Error.NoReply")),
        ] {
            let mut cooldown = PairingCooldown::default();
            cooldown.record(SENSOR_A, &outcome, t0);
            assert!(!cooldown.blocks(SENSOR_A, t0), "{outcome:?}");

            // Nor does it lift a cooldown the sensor earned.
            cooldown.record(SENSOR_A, &refused, t0);
            cooldown.record(SENSOR_A, &outcome, later);
            assert!(cooldown.blocks(SENSOR_A, later), "{outcome:?}");
        }
    }

    /// A session that the cooldown must skip.
    async fn must_not_run() -> PairOutcome {
        panic!("the cooldown should have skipped this session")
    }

    #[tokio::test(start_paused = true)]
    async fn pair_with_cooldown_skips_a_sensor_that_refused_for_10_minutes() {
        let cooldown = Mutex::new(PairingCooldown::default());
        let refused = PairOutcome::Failed(err("org.bluez.Error.AuthenticationRejected"));
        let ready = std::future::ready;

        let first = pair_with_cooldown(&cooldown, SENSOR_A, ready(refused.clone())).await;
        assert_eq!(first, Some(refused.clone()));

        // For 10 minutes the sensor's session doesn't run; other sensors' do.
        tokio::time::advance(Duration::from_secs(599)).await;
        assert_eq!(
            pair_with_cooldown(&cooldown, SENSOR_A, must_not_run()).await,
            None
        );
        let other = pair_with_cooldown(&cooldown, SENSOR_B, ready(PairOutcome::Paired)).await;
        assert_eq!(other, Some(PairOutcome::Paired));

        // Then it runs again. A timeout doesn't hold the sensor back...
        tokio::time::advance(Duration::from_secs(1)).await;
        let timed_out = pair_with_cooldown(&cooldown, SENSOR_A, ready(PairOutcome::TimedOut)).await;
        assert_eq!(timed_out, Some(PairOutcome::TimedOut));
        // ...but refusing again does.
        let again = pair_with_cooldown(&cooldown, SENSOR_A, ready(refused.clone())).await;
        assert_eq!(again, Some(refused));
        assert_eq!(
            pair_with_cooldown(&cooldown, SENSOR_A, must_not_run()).await,
            None
        );
    }
}
