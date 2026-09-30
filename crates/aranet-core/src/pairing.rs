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
//! - Errors from `CancelPairing` and `UnregisterAgent` are ignored: closing the
//!   connection removes its agent (the disconnect watch that `agent_create`
//!   sets, agent.c:274-276, calls `agent_disconnect`, agent.c:174-188), and
//!   ends a bonding it started and disconnects the sensor (device.c:3346-3361).
//! - btleplug 0.11.8 has no pairing call.

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use tokio::time::Instant;

/// The D-Bus error that `Device1.Pair` answers with when the device is
/// already bonded.
pub(crate) const ALREADY_EXISTS: &str = "org.bluez.Error.AlreadyExists";

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

/// The BlueZ calls that pairing one device makes, all on one D-Bus connection.
///
/// `run_pairing` puts a time limit on `pair` only, so every other call must
/// return within the implementation's own limit; `is_paired` is called again
/// when `pair` runs out of time. `Sync` makes `&Self` `Send`, which keeps
/// `run_pairing`'s future `Send`: the collector and the TUI spawn their
/// connects.
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
}

/// Pair once: skip if paired; otherwise register the agent, `Pair` within
/// `budget`, then unregister. When `budget` runs out, a sensor whose `Paired`
/// has turned true counts as paired, and any other gets `CancelPairing`.
/// Errors from `CancelPairing` and `UnregisterAgent` are ignored: closing the
/// connection removes the agent anyway.
pub(crate) async fn run_pairing<B: PairingBus>(bus: &B, budget: Duration) -> PairOutcome {
    match bus.is_paired().await {
        Ok(true) => return PairOutcome::AlreadyPaired,
        Ok(false) => {}
        Err(e) => return PairOutcome::Unavailable(e),
    }
    if let Err(e) = bus.register_agent().await {
        return PairOutcome::Unavailable(e);
    }
    let outcome = match tokio::time::timeout(budget, bus.pair()).await {
        Ok(Ok(())) => PairOutcome::Paired,
        Ok(Err(e)) if e.name == ALREADY_EXISTS => PairOutcome::AlreadyPaired,
        Ok(Err(e)) => PairOutcome::Failed(e),
        // BlueZ answers `Pair` only after service discovery, so time can run
        // out after the bond is made. `CancelPairing` would then unpair and
        // disconnect the sensor, so it is sent only while `Paired` is false.
        Err(_) => match bus.is_paired().await {
            Ok(true) => PairOutcome::Paired,
            Ok(false) | Err(_) => {
                let _ = bus.cancel_pairing().await;
                PairOutcome::TimedOut
            }
        },
    };
    let _ = bus.unregister_agent().await;
    outcome
}

/// How long a sensor whose pairing failed or timed out is left unpaired before
/// aranet tries again.
pub(crate) const PAIRING_RETRY_AFTER: Duration = Duration::from_secs(600);

/// Per-device memory of failed pairings, keyed by the device's BlueZ object
/// path.
///
/// A sensor that rejects Just Works pairing (possibly an Aranet4 that wants its
/// PIN) would otherwise cost up to the whole pairing budget and a warning on
/// every connect, which for the service is every poll.
#[derive(Debug, Default)]
pub(crate) struct PairingCooldown {
    until: HashMap<String, Instant>,
}

impl PairingCooldown {
    /// True while `device`'s last pairing failed or timed out less than
    /// `PAIRING_RETRY_AFTER` before `now`.
    pub(crate) fn blocks(&self, device: &str, now: Instant) -> bool {
        self.until.get(device).is_some_and(|until| now < *until)
    }

    /// Remembers how pairing `device` ended.
    ///
    /// `Failed` or `TimedOut` blocks it until `now + PAIRING_RETRY_AFTER`. Any
    /// error reply to `Pair` counts, including bus errors such as `NoReply`
    /// after a `bluetoothd` restart and BlueZ's `InProgress` while another
    /// client connects the sensor: the connect goes ahead unpaired either way,
    /// and only the next pairing attempt waits. `AlreadyPaired` or `Paired`
    /// forgets it. `Unavailable` changes nothing: `Pair` was never sent, so
    /// nothing was learned about the sensor. Entries whose time has passed stay
    /// in the map: there is one per sensor at most.
    pub(crate) fn record(&mut self, device: &str, outcome: &PairOutcome, now: Instant) {
        match outcome {
            PairOutcome::Failed(_) | PairOutcome::TimedOut => {
                self.until
                    .insert(device.to_owned(), now + PAIRING_RETRY_AFTER);
            }
            PairOutcome::AlreadyPaired | PairOutcome::Paired => {
                self.until.remove(device);
            }
            PairOutcome::Unavailable(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::test_support::within;

    /// The default pairing budget: `connection_timeout` (15 s) plus
    /// `discovery_timeout` (10 s).
    const BUDGET: Duration = Duration::from_secs(25);

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
    }

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
    }

    impl PairingBus for FakeBus {
        async fn is_paired(&self) -> Result<bool, BluezError> {
            let pair_sent = self.calls().contains(&"Pair");
            self.record("IsPaired");
            if pair_sent {
                self.paired_after_timeout.clone()
            } else {
                self.paired.clone()
            }
        }

        async fn register_agent(&self) -> Result<(), BluezError> {
            self.record("RegisterAgent");
            self.register.clone()
        }

        async fn pair(&self) -> Result<(), BluezError> {
            self.record("Pair");
            match &self.pair {
                PairScript::Ok => Ok(()),
                PairScript::Err(e) => Err(e.clone()),
                PairScript::Hang => std::future::pending().await,
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
        assert_eq!(start.elapsed(), budget);
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
            PairOutcome::TimedOut,
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
}
