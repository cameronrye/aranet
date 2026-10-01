//! Soak driver for Bluetooth connection lifecycles.
//!
//! One sensor ("M") is connected through a `DeviceManager` whose health monitor
//! runs every `--health-secs`, and another ("R") through a `ReconnectingDevice`
//! that reads the current values once per sample. Every `--fault-every` the
//! example cuts one of the two links behind aranet's back, alternating M and R,
//! the way a sensor that moves out of range drops it, and checks that the link
//! comes back:
//!
//! - M recovers when the manager emits `ReconnectSucceeded` within
//!   `2 × --health-secs + 70 s` of the cut;
//! - R recovers when the first read that starts after the cut succeeds;
//! - either way, the next sample must find the new link up and held by aranet.
//!
//! Each sample is one JSON line on stdout (`"kind": "sample"`). `os_connected`
//! lists the sensors that the Bluetooth stack reports as connected, and `orphans`
//! those of them that no aranet handle holds: the manager has no handle for M, or
//! R's state isn't `Connected`. A reconnect in progress can make a sensor an
//! orphan for a moment, so only an orphan in two samples in a row fails the run.
//! On macOS the check sees only this process's connections. On Linux it reads
//! BlueZ's `Device1.Connected`, which is global, so another program's connection
//! to the sensor counts too.
//!
//! After `--duration`, or on Ctrl-C, the example stops the health monitor,
//! disconnects both sensors, waits 5 s, prints a final sample (`"kind": "final"`)
//! and a summary line (`"kind": "summary"`), and exits with:
//!
//! - 0 (PASS);
//! - 1 (FAIL): an orphan in two samples in a row, a sensor still connected after
//!   shutdown, a link cut that wasn't recovered (or whose new link was gone by
//!   the next sample) or couldn't be made, or a shutdown step that hung;
//! - 2: bad arguments, or a sensor that couldn't be found, or connected in
//!   three attempts.
//!
//! `scripts/soak.sh` runs it and also samples its threads, file descriptors and
//! memory. A sensor that stays connected drains its battery faster, and the
//! phone app can't use it meanwhile.
//!
//! ```text
//! cargo run --locked --release -p aranet-core --example lifecycle_soak -- \
//!     --manager-device "Aranet2 2751B" [--reconnecting-device "AranetRn+ 306B8"] \
//!     --duration 24h --sample-secs 60 --fault-every 15m --health-secs 10
//! ```
//!
//! At least one of the two devices is required, as a name or as the identifier
//! that `aranet scan` prints. Durations take an `s`, `m` or `h` suffix, or none
//! for seconds, and `--fault-every 0` cuts no links. Logs go to stderr; set
//! `RUST_LOG=info` to see them.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use aranet_core::events::event_channel;
use aranet_core::reconnect::ConnectionState;
use aranet_core::scan::{ScanOptions, get_adapter, scan_with_options};
use aranet_core::{
    AranetDevice, DeviceEvent, DeviceManager, EventReceiver, ManagerConfig, ReconnectOptions,
    ReconnectingDevice, create_identifier,
};
use btleplug::api::{Central as _, Peripheral as _};
use btleplug::platform::Peripheral;
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;
use tokio::time::{
    Instant, Interval, MissedTickBehavior, interval_at, sleep, sleep_until, timeout,
};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// How long the start-up scan that finds the sensors lasts, in seconds.
const SCAN_SECS: u64 = 10;
/// Limit for one connection-state query to the Bluetooth stack.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// Limit for listing the adapter's peripherals.
const LIST_TIMEOUT: Duration = Duration::from_secs(5);
/// Limit for cutting a link.
const CUT_TIMEOUT: Duration = Duration::from_secs(10);
/// Time the health monitor gets, on top of two check intervals, to repair a cut
/// link: the check (at most 3 s), closing the dead link (at most 5 s), and up to
/// two searches and connects, because a connect sometimes times out after 15 s
/// and the next check then tries again. At the default `--health-secs 10` the
/// window is 90 s.
const REPAIR_BUDGET: Duration = Duration::from_secs(70);
/// Start-up connects per sensor: a first connect sometimes times out after 15 s,
/// and the next one works.
const CONNECT_ATTEMPTS: u32 = 3;
/// Pause between two start-up connects.
const CONNECT_PAUSE: Duration = Duration::from_secs(5);
/// Wait between disconnecting everything and the final sample.
const SETTLE: Duration = Duration::from_secs(5);
/// Limit for each shutdown step: stopping the monitor, `disconnect_all`, `disconnect`.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
/// Attempts, 50 ms apart, to read the manager's handle without blocking.
const HANDLE_ATTEMPTS: u32 = 20;
/// Longest accepted duration: a year, so that no deadline can overflow.
const MAX_SECS: u64 = 366 * 24 * 3600;

const USAGE: &str = "\
Usage: lifecycle_soak [--manager-device NAME_OR_ID] [--reconnecting-device NAME_OR_ID]
                      [--duration 24h] [--sample-secs 60] [--fault-every 15m] [--health-secs 10]

Connects one sensor through a DeviceManager with its health monitor running and another
through a ReconnectingDevice, cuts one of their links every --fault-every, and prints one
JSON line per sample on stdout. At least one device is required, and the two must be
different sensors. Durations take an s, m or h suffix (none means seconds);
--fault-every 0 cuts no links.

Exit status: 0 PASS, 1 FAIL, 2 bad arguments or setup failure.
";

/// Command-line settings.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Args {
    manager_device: Option<String>,
    reconnecting_device: Option<String>,
    duration: Duration,
    sample: Duration,
    fault_every: Duration,
    health: Duration,
}

impl Args {
    /// How long after a cut the manager has to report `ReconnectSucceeded`.
    fn manager_window(&self) -> Duration {
        self.health * 2 + REPAIR_BUDGET
    }

    /// No link is cut this close to the end, so that every cut can be checked.
    fn quiet_tail(&self) -> Duration {
        self.manager_window() + self.sample * 2
    }
}

/// Parses `90`, `90s`, `15m` or `24h`.
fn parse_duration(text: &str) -> Result<Duration, String> {
    let (number, unit_secs) = if let Some(number) = text.strip_suffix('h') {
        (number, 3600)
    } else if let Some(number) = text.strip_suffix('m') {
        (number, 60)
    } else if let Some(number) = text.strip_suffix('s') {
        (number, 1)
    } else {
        (text, 1)
    };
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "invalid duration '{text}': use a whole number with an optional s, m or h suffix"
        ));
    }
    number
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(unit_secs))
        .filter(|&secs| secs <= MAX_SECS)
        .map(Duration::from_secs)
        .ok_or_else(|| format!("duration '{text}' is longer than a year"))
}

fn device_name(flag: &str, value: &str) -> Result<String, String> {
    let name = value.trim();
    if name.is_empty() {
        return Err(format!("{flag} needs a device name or identifier"));
    }
    Ok(name.to_string())
}

/// Parses the arguments after the program name. `Ok(None)` means `--help`.
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Args>, String> {
    let mut parsed = Args {
        manager_device: None,
        reconnecting_device: None,
        duration: Duration::from_secs(24 * 3600),
        sample: Duration::from_secs(60),
        fault_every: Duration::from_secs(15 * 60),
        health: Duration::from_secs(10),
    };
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        if flag == "--help" || flag == "-h" {
            return Ok(None);
        }
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--manager-device" => parsed.manager_device = Some(device_name(&flag, &value()?)?),
            "--reconnecting-device" => {
                parsed.reconnecting_device = Some(device_name(&flag, &value()?)?);
            }
            "--duration" => parsed.duration = parse_duration(&value()?)?,
            "--sample-secs" => parsed.sample = parse_duration(&value()?)?,
            "--fault-every" => parsed.fault_every = parse_duration(&value()?)?,
            "--health-secs" => parsed.health = parse_duration(&value()?)?,
            _ => return Err(format!("unknown argument '{flag}'")),
        }
    }

    if parsed.manager_device.is_none() && parsed.reconnecting_device.is_none() {
        return Err("give --manager-device, --reconnecting-device or both".to_string());
    }
    if let (Some(m), Some(r)) = (&parsed.manager_device, &parsed.reconnecting_device)
        && m.eq_ignore_ascii_case(r)
    {
        return Err(
            "--manager-device and --reconnecting-device must be different sensors".to_string(),
        );
    }
    if parsed.duration.is_zero() {
        return Err("--duration must be longer than 0".to_string());
    }
    if parsed.sample < Duration::from_secs(5) {
        return Err("--sample-secs must be at least 5".to_string());
    }
    if parsed.health.is_zero() {
        return Err("--health-secs must be at least 1".to_string());
    }
    if !parsed.fault_every.is_zero() {
        let shortest = parsed.manager_window().max(parsed.sample * 2);
        if parsed.fault_every < shortest {
            return Err(format!(
                "--fault-every must be 0 or at least {}s, so that each cut can recover before the next",
                shortest.as_secs()
            ));
        }
        let needed = parsed.fault_every + parsed.quiet_tail();
        if parsed.duration < needed {
            return Err(format!(
                "--duration must be at least {}s with --fault-every {}s, so that a cut can be checked",
                needed.as_secs(),
                parsed.fault_every.as_secs()
            ));
        }
    }
    Ok(Some(parsed))
}

/// Whether `name` is `query`, ignoring case, or either half of CoreBluetooth's
/// combined `"<GAP name> [<advertised name>]"`.
fn name_matches(name: &str, query: &str) -> bool {
    name.eq_ignore_ascii_case(query)
        || name
            .strip_suffix(']')
            .and_then(|rest| rest.split_once(" ["))
            .is_some_and(|(gap, advertised)| {
                gap.eq_ignore_ascii_case(query) || advertised.eq_ignore_ascii_case(query)
            })
}

/// Which driver a link cut or an event belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    /// The `DeviceManager` sensor.
    Manager,
    /// The `ReconnectingDevice` sensor.
    Reconnecting,
}

impl Side {
    fn label(self) -> &'static str {
        match self {
            Side::Manager => "manager",
            Side::Reconnecting => "reconnecting",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recovery {
    Pending,
    /// Recovered this long after the cut.
    Recovered(Duration),
    /// The first read that started after the cut failed.
    ReadFailed,
}

#[derive(Debug)]
struct Fault {
    side: Side,
    id: String,
    at: Duration,
    recovery: Recovery,
    /// Whether the first sample after the recovery found the link up and held.
    held_at_next_sample: Option<bool>,
}

/// Why a run fails, split the way `scripts/soak.sh` reports it.
#[derive(Debug, Default, PartialEq, Eq)]
struct Verdict {
    /// Criterion (d): orphans, sensors left connected, blind probes, hung shutdown steps.
    connection: Vec<String>,
    /// Criterion (e): link cuts that weren't recovered or couldn't be made.
    faults: Vec<String>,
}

impl Verdict {
    fn passed(&self) -> bool {
        self.connection.is_empty() && self.faults.is_empty()
    }
}

/// Counters and pass/fail bookkeeping. It never touches Bluetooth, so the unit
/// tests drive it directly.
#[derive(Debug)]
struct Tracker {
    manager_window: Duration,
    faults_enabled: bool,
    faults: Vec<Fault>,
    fault_errors: u64,
    fault_skips: u64,
    m_reconnects: u64,
    r_reconnects: u64,
    reads_ok: u64,
    reads_err: u64,
    samples: u64,
    /// Current orphan streak per sensor: samples in a row, and when it began.
    streaks: BTreeMap<String, (u32, Duration)>,
    /// Longest streak of two or more samples per sensor, and when it began.
    orphaned: BTreeMap<String, (u32, Duration)>,
    blind_streak: u32,
    longest_blind_streak: u32,
}

impl Tracker {
    fn new(manager_window: Duration, faults_enabled: bool) -> Self {
        Self {
            manager_window,
            faults_enabled,
            faults: Vec::new(),
            fault_errors: 0,
            fault_skips: 0,
            m_reconnects: 0,
            r_reconnects: 0,
            reads_ok: 0,
            reads_err: 0,
            samples: 0,
            streaks: BTreeMap::new(),
            orphaned: BTreeMap::new(),
            blind_streak: 0,
            longest_blind_streak: 0,
        }
    }

    fn fault_injected(&mut self, side: Side, id: &str, at: Duration) {
        self.faults.push(Fault {
            side,
            id: id.to_string(),
            at,
            recovery: Recovery::Pending,
            held_at_next_sample: None,
        });
    }

    fn reconnect_succeeded(&mut self, side: Side, at: Duration) {
        match side {
            Side::Manager => {
                self.m_reconnects += 1;
                let window = self.manager_window;
                for fault in &mut self.faults {
                    if fault.side == Side::Manager
                        && fault.recovery == Recovery::Pending
                        && fault.at <= at
                        && at <= fault.at + window
                    {
                        fault.recovery = Recovery::Recovered(at - fault.at);
                    }
                }
            }
            Side::Reconnecting => self.r_reconnects += 1,
        }
    }

    /// Records a read of the `ReconnectingDevice`. The first read that starts
    /// after a cut decides whether that cut recovered.
    fn read_finished(&mut self, started: Duration, finished: Duration, ok: bool) {
        if ok {
            self.reads_ok += 1;
        } else {
            self.reads_err += 1;
        }
        for fault in &mut self.faults {
            if fault.side == Side::Reconnecting
                && fault.recovery == Recovery::Pending
                && fault.at <= started
            {
                fault.recovery = if ok {
                    Recovery::Recovered(finished.saturating_sub(fault.at))
                } else {
                    Recovery::ReadFailed
                };
            }
        }
    }

    /// Records one sample. `os_connected` is `None` when the Bluetooth stack
    /// couldn't be asked; such a sample neither ends nor extends an orphan streak.
    fn sample(
        &mut self,
        at: Duration,
        ids: &[String],
        os_connected: Option<&[String]>,
        orphans: &[String],
    ) {
        self.samples += 1;
        let Some(os_connected) = os_connected else {
            self.blind_streak += 1;
            self.longest_blind_streak = self.longest_blind_streak.max(self.blind_streak);
            return;
        };
        self.blind_streak = 0;
        for id in ids {
            if orphans.contains(id) {
                let (count, since) = self.streaks.entry(id.clone()).or_insert((0, at));
                *count += 1;
                if *count >= 2 {
                    let worst = self.orphaned.entry(id.clone()).or_insert((0, *since));
                    if *count > worst.0 {
                        *worst = (*count, *since);
                    }
                }
            } else {
                self.streaks.remove(id);
            }
        }
        // A recovered link must still be up, and held, at the next sample: an old
        // handle's cleanup that tears down the new link shows up here.
        for fault in &mut self.faults {
            if let Recovery::Recovered(after) = fault.recovery
                && fault.held_at_next_sample.is_none()
                && fault.at + after < at
            {
                fault.held_at_next_sample =
                    Some(os_connected.contains(&fault.id) && !orphans.contains(&fault.id));
            }
        }
    }

    fn unrecovered(&self) -> usize {
        self.faults
            .iter()
            .filter(|fault| {
                !matches!(fault.recovery, Recovery::Recovered(_))
                    || fault.held_at_next_sample == Some(false)
            })
            .count()
    }

    fn verdict(&self, connected_after_shutdown: &[String], problems: &[String]) -> Verdict {
        let mut verdict = Verdict::default();
        for (id, (count, since)) in &self.orphaned {
            verdict.connection.push(format!(
                "{id} was connected with no aranet handle in {count} samples in a row from t={}s",
                since.as_secs()
            ));
        }
        for id in connected_after_shutdown {
            verdict.connection.push(format!(
                "{id} was still connected {}s after shutdown",
                SETTLE.as_secs()
            ));
        }
        if self.longest_blind_streak >= 2 {
            verdict.connection.push(format!(
                "the Bluetooth stack couldn't be checked in {} samples in a row",
                self.longest_blind_streak
            ));
        }
        verdict.connection.extend(problems.iter().cloned());

        for fault in &self.faults {
            let problem = match fault.recovery {
                Recovery::Recovered(after) if fault.held_at_next_sample == Some(false) => format!(
                    "it recovered after {}s, but the next sample found the link down or not held by aranet",
                    after.as_secs()
                ),
                Recovery::Recovered(_) => continue,
                Recovery::Pending if fault.side == Side::Manager => format!(
                    "no ReconnectSucceeded within {}s",
                    self.manager_window.as_secs()
                ),
                Recovery::Pending => "no read finished after it".to_string(),
                Recovery::ReadFailed => "the next read failed".to_string(),
            };
            verdict.faults.push(format!(
                "link cut on the {} sensor {} at t={}s: {problem}",
                fault.side.label(),
                fault.id,
                fault.at.as_secs()
            ));
        }
        if self.fault_errors > 0 {
            verdict.faults.push(format!(
                "{} link cuts could not be made (see the log)",
                self.fault_errors
            ));
        }
        if self.faults_enabled && self.faults.is_empty() {
            verdict.faults.push("no link was cut".to_string());
        }
        verdict
    }

    fn fault_report(&self) -> Vec<Value> {
        self.faults
            .iter()
            .map(|fault| {
                let recovered_after = match fault.recovery {
                    Recovery::Recovered(after) => Some(after.as_secs()),
                    Recovery::Pending | Recovery::ReadFailed => None,
                };
                json!({
                    "side": fault.side.label(),
                    "id": fault.id,
                    "t": fault.at.as_secs(),
                    "recovered_after_s": recovered_after,
                    "held_at_next_sample": fault.held_at_next_sample,
                })
            })
            .collect()
    }
}

/// What one sample saw.
#[derive(Debug)]
struct Snapshot {
    /// `DeviceManager::try_is_connected` for M; `None` if M isn't driven or the
    /// manager's map stayed locked.
    manager_handle: Option<bool>,
    rd_state: Option<ConnectionState>,
    os_connected: Vec<String>,
    orphans: Vec<String>,
    probe_error: Option<String>,
}

impl Snapshot {
    fn to_json(&self, kind: &str, at: Duration, tracker: &Tracker) -> Value {
        json!({
            "kind": kind,
            "t": at.as_secs(),
            "manager_handle": self.manager_handle,
            "rd_state": self.rd_state.map(|state| format!("{state:?}")),
            "os_connected": self.os_connected,
            "orphans": self.orphans,
            "probe_error": self.probe_error,
            "faults": tracker.faults.len(),
            "fault_skips": tracker.fault_skips,
            "fault_errors": tracker.fault_errors,
            "m_reconnects": tracker.m_reconnects,
            "r_reconnects": tracker.r_reconnects,
            "rd_reads_ok": tracker.reads_ok,
            "rd_reads_err": tracker.reads_err,
        })
    }
}

/// The adapter's peripherals among `ids` that the Bluetooth stack reports as
/// connected, each with the identifier it matched. A sensor that the adapter
/// doesn't list, or whose query doesn't answer within 2 s, counts as not
/// connected: on macOS a disconnect removes the peripheral from the adapter's
/// list until a scan finds it again, and a handle kept from before the
/// disconnect gets no answer. `Err` means the adapter couldn't list its
/// peripherals.
async fn connected_peripherals(ids: &[String]) -> Result<Vec<(String, Peripheral)>, String> {
    let adapter = get_adapter()
        .await
        .map_err(|e| format!("no Bluetooth adapter: {e}"))?;
    let peripherals = timeout(LIST_TIMEOUT, adapter.peripherals())
        .await
        .map_err(|_| "listing peripherals timed out".to_string())?
        .map_err(|e| format!("listing peripherals failed: {e}"))?;
    let mut connected: Vec<(String, Peripheral)> = Vec::new();
    for peripheral in peripherals {
        let identifier = create_identifier(&peripheral.address().to_string(), &peripheral.id());
        if let Some(id) = ids.iter().find(|id| id.eq_ignore_ascii_case(&identifier))
            && !connected.iter().any(|(seen, _)| seen == id)
            && matches!(
                timeout(PROBE_TIMEOUT, peripheral.is_connected()).await,
                Ok(Ok(true))
            )
        {
            connected.push((id.clone(), peripheral));
        }
    }
    Ok(connected)
}

/// The identifiers among `ids` whose peripheral the Bluetooth stack reports as
/// connected (see `connected_peripherals`), sorted.
async fn os_connected(ids: &[String]) -> Result<Vec<String>, String> {
    let mut connected: Vec<String> = connected_peripherals(ids)
        .await?
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    connected.sort();
    Ok(connected)
}

/// Disconnects `id` behind aranet's back, through the adapter's own peripheral
/// (the same CoreBluetooth peripheral or BlueZ object that aranet's handle uses),
/// as if the sensor had moved out of range. `Ok(false)` means the sensor wasn't
/// connected, or the adapter didn't list it, so nothing was cut. `Err` means the
/// adapter couldn't list its peripherals or the disconnect failed.
async fn force_link_loss(id: &str) -> Result<bool, String> {
    let Some((_, peripheral)) = connected_peripherals(&[id.to_string()]).await?.pop() else {
        return Ok(false);
    };
    match timeout(CUT_TIMEOUT, peripheral.disconnect()).await {
        Ok(Ok(())) => Ok(true),
        Ok(Err(e)) => Err(format!("disconnect failed: {e}")),
        Err(_) => Err(format!(
            "disconnect didn't finish within {}s",
            CUT_TIMEOUT.as_secs()
        )),
    }
}

/// `DeviceManager::try_is_connected`, retried while the manager's map is locked.
async fn manager_handle(manager: &DeviceManager, id: &str) -> Option<bool> {
    for _ in 0..HANDLE_ATTEMPTS {
        if let Some(held) = manager.try_is_connected(id) {
            return Some(held);
        }
        sleep(Duration::from_millis(50)).await;
    }
    None
}

fn ticker(first: Instant, period: Duration) -> Interval {
    let mut ticker = interval_at(first, period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticker
}

async fn tick(ticker: &mut Option<Interval>) {
    match ticker {
        Some(ticker) => {
            ticker.tick().await;
        }
        None => std::future::pending().await,
    }
}

async fn recv(events: &mut Option<EventReceiver>) -> Result<DeviceEvent, RecvError> {
    match events {
        Some(events) => events.recv().await,
        None => std::future::pending().await,
    }
}

/// The two drivers and the shared bookkeeping. Everything runs on one task, so
/// the tracker is a `RefCell`, and no borrow of it is held across an `.await`.
struct Soak {
    start: Instant,
    manager: Option<(Arc<DeviceManager>, String)>,
    reconnecting: Option<(ReconnectingDevice, String)>,
    tracker: RefCell<Tracker>,
}

impl Soak {
    fn ids(&self) -> Vec<String> {
        self.manager
            .iter()
            .map(|(_, id)| id.clone())
            .chain(self.reconnecting.iter().map(|(_, id)| id.clone()))
            .collect()
    }

    fn id_of(&self, side: Side) -> Option<&str> {
        match side {
            Side::Manager => self.manager.as_ref().map(|(_, id)| id.as_str()),
            Side::Reconnecting => self.reconnecting.as_ref().map(|(_, id)| id.as_str()),
        }
    }

    /// The side to cut after `side`: the other one, if both are driven.
    fn after(&self, side: Side) -> Side {
        match side {
            Side::Manager if self.reconnecting.is_some() => Side::Reconnecting,
            Side::Reconnecting if self.manager.is_some() => Side::Manager,
            _ => side,
        }
    }

    async fn snapshot(&self) -> Snapshot {
        // Ask the Bluetooth stack first: a reconnect that finishes meanwhile then
        // shows up as a held handle rather than as an orphan.
        let probe = os_connected(&self.ids()).await;
        let manager_handle = match &self.manager {
            Some((manager, id)) => manager_handle(manager, id).await,
            None => None,
        };
        let rd_state = match &self.reconnecting {
            Some((rd, _)) => Some(rd.state().await),
            None => None,
        };
        let (os_connected, probe_error) = match probe {
            Ok(connected) => (connected, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        // A manager handle that couldn't be read counts as held.
        let held = |id: &String| {
            self.manager
                .as_ref()
                .is_some_and(|(_, m)| m == id && manager_handle != Some(false))
                || self
                    .reconnecting
                    .as_ref()
                    .is_some_and(|(_, r)| r == id && rd_state == Some(ConnectionState::Connected))
        };
        let orphans = os_connected
            .iter()
            .filter(|id| !held(id))
            .cloned()
            .collect();
        Snapshot {
            manager_handle,
            rd_state,
            os_connected,
            orphans,
            probe_error,
        }
    }

    async fn sample(&self) -> Value {
        // The time comes first: a recovery that finishes while the probe runs
        // is then judged at the next sample, not against this earlier probe.
        let at = self.start.elapsed();
        let snapshot = self.snapshot().await;
        let os_connected = snapshot
            .probe_error
            .is_none()
            .then_some(snapshot.os_connected.as_slice());
        let mut tracker = self.tracker.borrow_mut();
        tracker.sample(at, &self.ids(), os_connected, &snapshot.orphans);
        snapshot.to_json("sample", at, &tracker)
    }

    async fn cut(&self, side: Side) {
        let Some(id) = self.id_of(side) else {
            return;
        };
        match force_link_loss(id).await {
            Ok(true) => {
                // The cut counts from when the disconnect returned: a read that
                // started while it ran may still have used the old link, so it
                // mustn't count as the first read after the cut.
                let at = self.start.elapsed();
                info!("Cut the link to the {} sensor {id}", side.label());
                self.tracker.borrow_mut().fault_injected(side, id, at);
            }
            Ok(false) => {
                warn!(
                    "Skipped the link cut: the {} sensor {id} isn't connected",
                    side.label()
                );
                self.tracker.borrow_mut().fault_skips += 1;
            }
            Err(e) => {
                warn!(
                    "Couldn't cut the link to the {} sensor {id}: {e}",
                    side.label()
                );
                self.tracker.borrow_mut().fault_errors += 1;
            }
        }
    }

    /// Returns `false` once the channel is closed.
    fn on_event(&self, side: Side, event: Result<DeviceEvent, RecvError>) -> bool {
        match event {
            Ok(DeviceEvent::ReconnectSucceeded { device, attempts }) => {
                info!(
                    "The {} sensor {} reconnected after {attempts} attempt(s)",
                    side.label(),
                    device.id
                );
                self.tracker
                    .borrow_mut()
                    .reconnect_succeeded(side, self.start.elapsed());
                true
            }
            Ok(_) => true,
            Err(RecvError::Lagged(missed)) => {
                warn!("Missed {missed} events of the {} sensor", side.label());
                true
            }
            Err(RecvError::Closed) => false,
        }
    }

    /// Samples, cuts links and counts reconnects until `--duration` or Ctrl-C,
    /// then cancels `stop`.
    async fn drive(
        &self,
        args: &Args,
        stop: &CancellationToken,
        mut m_events: Option<EventReceiver>,
        mut r_events: Option<EventReceiver>,
    ) {
        let deadline = sleep_until(self.start + args.duration);
        tokio::pin!(deadline);
        let ctrl_c = tokio::signal::ctrl_c();
        tokio::pin!(ctrl_c);
        let mut listen_for_ctrl_c = true;
        let mut samples = ticker(self.start, args.sample);
        let mut cuts = (!args.fault_every.is_zero())
            .then(|| ticker(self.start + args.fault_every, args.fault_every));
        let mut next_side = if self.manager.is_some() {
            Side::Manager
        } else {
            Side::Reconnecting
        };

        loop {
            tokio::select! {
                () = &mut deadline => break,
                result = &mut ctrl_c, if listen_for_ctrl_c => match result {
                    Ok(()) => {
                        info!("Ctrl-C: stopping");
                        break;
                    }
                    Err(e) => {
                        warn!("Can't listen for Ctrl-C: {e}");
                        listen_for_ctrl_c = false;
                    }
                },
                _ = samples.tick() => println!("{}", self.sample().await),
                () = tick(&mut cuts) => {
                    if self.start.elapsed() + args.quiet_tail() <= args.duration {
                        self.cut(next_side).await;
                        next_side = self.after(next_side);
                    }
                }
                event = recv(&mut m_events) => {
                    if !self.on_event(Side::Manager, event) {
                        m_events = None;
                    }
                }
                event = recv(&mut r_events) => {
                    if !self.on_event(Side::Reconnecting, event) {
                        r_events = None;
                    }
                }
            }
        }
        stop.cancel();
    }

    /// Reads the `ReconnectingDevice` once per sample until `stop` is cancelled.
    async fn read_loop(&self, every: Duration, stop: &CancellationToken) {
        let Some((rd, id)) = &self.reconnecting else {
            return;
        };
        let mut reads = ticker(self.start, every);
        while stop.run_until_cancelled(reads.tick()).await.is_some() {
            let started = self.start.elapsed();
            let Some(result) = stop.run_until_cancelled(rd.read_current()).await else {
                break;
            };
            if let Err(e) = &result {
                warn!("Reading the reconnecting sensor {id} failed: {e}");
            }
            self.tracker
                .borrow_mut()
                .read_finished(started, self.start.elapsed(), result.is_ok());
        }
    }

    /// Stops the monitor, disconnects both sensors and waits `SETTLE`. Returns
    /// the shutdown steps that failed or hung.
    async fn shutdown(
        &self,
        monitor_cancel: &CancellationToken,
        monitor: Option<JoinHandle<()>>,
    ) -> Vec<String> {
        let mut problems = Vec::new();
        monitor_cancel.cancel();
        if let Some(monitor) = monitor {
            match timeout(SHUTDOWN_TIMEOUT, monitor).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => problems.push(format!("the health monitor task failed: {e}")),
                Err(_) => problems.push(format!(
                    "the health monitor didn't stop within {}s of being cancelled",
                    SHUTDOWN_TIMEOUT.as_secs()
                )),
            }
        }
        if let Some((manager, _)) = &self.manager {
            match timeout(SHUTDOWN_TIMEOUT, manager.disconnect_all()).await {
                Ok(results) => {
                    for (id, result) in results {
                        if let Err(e) = result {
                            warn!("Disconnecting the manager sensor {id} failed: {e}");
                        }
                    }
                }
                Err(_) => problems.push(format!(
                    "DeviceManager::disconnect_all didn't finish within {}s",
                    SHUTDOWN_TIMEOUT.as_secs()
                )),
            }
        }
        if let Some((rd, id)) = &self.reconnecting {
            match timeout(SHUTDOWN_TIMEOUT, rd.disconnect()).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => warn!("Disconnecting the reconnecting sensor {id} failed: {e}"),
                Err(_) => problems.push(format!(
                    "ReconnectingDevice::disconnect didn't finish within {}s",
                    SHUTDOWN_TIMEOUT.as_secs()
                )),
            }
        }
        sleep(SETTLE).await;
        problems
    }
}

/// Runs a start-up connect up to `CONNECT_ATTEMPTS` times, `CONNECT_PAUSE` apart.
async fn connect_with_retries<T, F>(what: &str, mut connect: impl FnMut() -> F) -> Result<T, String>
where
    F: Future<Output = aranet_core::Result<T>>,
{
    let mut attempt = 1;
    loop {
        match connect().await {
            Ok(connected) => return Ok(connected),
            Err(e) if attempt < CONNECT_ATTEMPTS => {
                warn!("{what} failed (attempt {attempt} of {CONNECT_ATTEMPTS}): {e}");
                attempt += 1;
                sleep(CONNECT_PAUSE).await;
            }
            Err(e) => return Err(format!("{what} failed {CONNECT_ATTEMPTS} times: {e}")),
        }
    }
}

/// Runs the soak. `Ok(true)` is PASS, `Ok(false)` FAIL, `Err` a setup failure.
async fn run(args: Args) -> Result<bool, String> {
    let found = scan_with_options(ScanOptions::default().duration_secs(SCAN_SECS))
        .await
        .map_err(|e| format!("scan failed: {e}"))?;
    let resolve = |query: &str| {
        found
            .iter()
            .find(|device| {
                device.identifier.eq_ignore_ascii_case(query)
                    || device
                        .name
                        .as_deref()
                        .is_some_and(|name| name_matches(name, query))
            })
            .map(|device| device.identifier.clone())
            .ok_or_else(|| format!("'{query}' wasn't found in a {SCAN_SECS} s scan"))
    };
    let m_id = args.manager_device.as_deref().map(resolve).transpose()?;
    let r_id = args
        .reconnecting_device
        .as_deref()
        .map(resolve)
        .transpose()?;
    if m_id.is_some() && m_id == r_id {
        return Err("both devices name the same sensor".to_string());
    }

    let mut manager = None;
    let mut m_events = None;
    if let Some(id) = &m_id {
        let config = ManagerConfig::default()
            .health_check_interval(args.health)
            .adaptive_interval(false);
        let m = Arc::new(DeviceManager::with_config(config));
        m_events = Some(m.events().subscribe());
        connect_with_retries(&format!("DeviceManager's connect to {id}"), || {
            m.connect(id)
        })
        .await?;
        manager = Some((m, id.clone()));
    }
    let mut reconnecting = None;
    let mut r_events = None;
    if let Some(id) = &r_id {
        let (sender, receiver) = event_channel(64);
        let connected =
            connect_with_retries(&format!("ReconnectingDevice's connect to {id}"), || {
                ReconnectingDevice::connect_with_events(
                    id,
                    ReconnectOptions::unlimited(),
                    sender.clone(),
                )
            })
            .await;
        let rd = match connected {
            Ok(rd) => rd,
            Err(e) => {
                // Don't leave M connected: on Linux, BlueZ can keep a link up
                // after the process that asked for it has exited.
                if let Some((m, _)) = &manager {
                    let _ = timeout(SHUTDOWN_TIMEOUT, m.disconnect_all()).await;
                }
                return Err(e);
            }
        };
        r_events = Some(receiver);
        reconnecting = Some((rd, id.clone()));
    }
    let monitor_cancel = CancellationToken::new();
    let monitor = manager
        .as_ref()
        .map(|(m, _)| m.start_health_monitor(monitor_cancel.clone()));

    println!(
        "{}",
        json!({
            "kind": "start",
            "manager_device": m_id,
            "reconnecting_device": r_id,
            "duration_s": args.duration.as_secs(),
            "sample_s": args.sample.as_secs(),
            "fault_every_s": args.fault_every.as_secs(),
            "health_s": args.health.as_secs(),
            "manager_window_s": args.manager_window().as_secs(),
        })
    );

    let soak = Soak {
        start: Instant::now(),
        manager,
        reconnecting,
        tracker: RefCell::new(Tracker::new(
            args.manager_window(),
            !args.fault_every.is_zero(),
        )),
    };
    let stop = CancellationToken::new();
    tokio::join!(
        soak.drive(&args, &stop, m_events, r_events),
        soak.read_loop(args.sample, &stop),
    );
    let mut problems = soak.shutdown(&monitor_cancel, monitor).await;

    let last = soak.snapshot().await;
    if let Some(e) = &last.probe_error {
        problems.push(format!(
            "couldn't check the Bluetooth stack after shutdown: {e}"
        ));
    }
    let tracker = soak.tracker.borrow();
    println!("{}", last.to_json("final", soak.start.elapsed(), &tracker));
    let verdict = tracker.verdict(&last.os_connected, &problems);
    let reasons: Vec<&String> = verdict.connection.iter().chain(&verdict.faults).collect();
    println!(
        "{}",
        json!({
            "kind": "summary",
            "result": if verdict.passed() { "PASS" } else { "FAIL" },
            "reasons": reasons,
            "connection_problems": verdict.connection,
            "fault_problems": verdict.faults,
            "faults": tracker.fault_report(),
            "unrecovered": tracker.unrecovered(),
            "fault_errors": tracker.fault_errors,
            "fault_skips": tracker.fault_skips,
            "orphaned": tracker.orphaned.keys().collect::<Vec<_>>(),
            "connected_after_shutdown": last.os_connected,
            "samples": tracker.samples,
            "m_reconnects": tracker.m_reconnects,
            "r_reconnects": tracker.r_reconnects,
            "rd_reads_ok": tracker.reads_ok,
            "rd_reads_err": tracker.reads_err,
        })
    );
    Ok(verdict.passed())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let args = match parse_args(std::env::args().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("error: {message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(args).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn args(list: &[&str]) -> Result<Option<Args>, String> {
        parse_args(list.iter().map(|arg| arg.to_string()))
    }

    fn ids() -> Vec<String> {
        vec!["M".to_string(), "R".to_string()]
    }

    fn only(id: &str) -> Vec<String> {
        vec![id.to_string()]
    }

    #[test]
    fn parse_duration_accepts_whole_numbers_with_an_optional_unit() {
        assert_eq!(parse_duration("90"), Ok(secs(90)));
        assert_eq!(parse_duration("45s"), Ok(secs(45)));
        assert_eq!(parse_duration("15m"), Ok(secs(900)));
        assert_eq!(parse_duration("24h"), Ok(secs(86_400)));
        assert_eq!(parse_duration("0"), Ok(Duration::ZERO));
        assert_eq!(parse_duration("8784h"), Ok(secs(MAX_SECS)));
        for bad in [
            "",
            "m",
            "1d",
            "-5s",
            "1.5h",
            " 5m",
            "5 m",
            "8785h",
            "99999999999999999999",
        ] {
            assert!(parse_duration(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn parse_args_uses_the_documented_defaults() {
        let parsed = args(&["--manager-device", "Aranet2 2751B"])
            .unwrap()
            .unwrap();
        assert_eq!(
            parsed,
            Args {
                manager_device: Some("Aranet2 2751B".to_string()),
                reconnecting_device: None,
                duration: secs(86_400),
                sample: secs(60),
                fault_every: secs(900),
                health: secs(10),
            }
        );
        assert_eq!(parsed.manager_window(), secs(90));
        assert_eq!(parsed.quiet_tail(), secs(210));
    }

    #[test]
    fn parse_args_reads_every_option() {
        let parsed = args(&[
            "--reconnecting-device",
            " AranetRn+ 306B8 ",
            "--duration",
            "1h",
            "--sample-secs",
            "30",
            "--fault-every",
            "5m",
            "--health-secs",
            "5s",
        ])
        .unwrap()
        .unwrap();
        assert_eq!(parsed.manager_device, None);
        assert_eq!(
            parsed.reconnecting_device.as_deref(),
            Some("AranetRn+ 306B8")
        );
        assert_eq!(
            (
                parsed.duration,
                parsed.sample,
                parsed.fault_every,
                parsed.health
            ),
            (secs(3600), secs(30), secs(300), secs(5))
        );
        assert_eq!(args(&["--help"]), Ok(None));
        let quiet = args(&[
            "--manager-device",
            "a",
            "--duration",
            "15m",
            "--fault-every",
            "0",
        ]);
        assert!(matches!(quiet, Ok(Some(_))), "{quiet:?}");
    }

    #[test]
    fn parse_args_rejects_bad_combinations() {
        let cases: [(&[&str], &str); 9] = [
            (
                &["--duration", "1h"],
                "--manager-device, --reconnecting-device or both",
            ),
            (
                &[
                    "--manager-device",
                    "Aranet2 2751B",
                    "--reconnecting-device",
                    "aranet2 2751b",
                ],
                "different sensors",
            ),
            (&["--manager-device", "  "], "needs a device name"),
            (&["--manager-device"], "needs a value"),
            (
                &["--manager-device", "a", "--verbose"],
                "unknown argument '--verbose'",
            ),
            (
                &["--manager-device", "a", "--sample-secs", "4"],
                "at least 5",
            ),
            (
                &["--manager-device", "a", "--health-secs", "0"],
                "at least 1",
            ),
            (
                &["--manager-device", "a", "--fault-every", "1m"],
                "at least 120s",
            ),
            (
                &["--manager-device", "a", "--duration", "15m"],
                "at least 1110s",
            ),
        ];
        for (list, needle) in cases {
            let error = args(list).unwrap_err();
            assert!(error.contains(needle), "{list:?}: {error}");
        }
    }

    #[test]
    fn name_matching_is_exact_but_accepts_either_half_of_a_corebluetooth_name() {
        assert!(name_matches("Aranet2 2751B", "aranet2 2751b"));
        assert!(!name_matches("Aranet2 2751B", "2751B"));
        assert!(name_matches("Kitchen [Aranet4 1A2B3]", "Aranet4 1A2B3"));
        assert!(name_matches("Kitchen [Aranet4 1A2B3]", "kitchen"));
        assert!(!name_matches("Kitchen [Aranet4 1A2B3]", "Aranet4"));
    }

    #[test]
    fn an_orphan_in_one_sample_is_tolerated() {
        let mut tracker = Tracker::new(secs(90), false);
        tracker.sample(secs(0), &ids(), Some(&ids()), &only("M"));
        tracker.sample(secs(60), &ids(), Some(&ids()), &[]);
        tracker.sample(secs(120), &ids(), Some(&ids()), &only("M"));
        tracker.sample(secs(180), &ids(), Some(&[]), &[]);
        assert_eq!(tracker.verdict(&[], &[]), Verdict::default());
    }

    #[test]
    fn an_orphan_in_two_samples_in_a_row_fails() {
        let mut tracker = Tracker::new(secs(90), false);
        tracker.sample(secs(0), &ids(), Some(&[]), &[]);
        for t in [60, 120, 180] {
            tracker.sample(secs(t), &ids(), Some(&only("R")), &only("R"));
        }
        tracker.sample(secs(240), &ids(), Some(&[]), &[]);
        let verdict = tracker.verdict(&[], &[]);
        assert_eq!(
            verdict.connection,
            ["R was connected with no aranet handle in 3 samples in a row from t=60s"]
        );
        assert!(verdict.faults.is_empty());
    }

    #[test]
    fn a_blind_sample_neither_ends_a_streak_nor_hides_two_in_a_row() {
        let mut tracker = Tracker::new(secs(90), false);
        tracker.sample(secs(0), &ids(), Some(&only("M")), &only("M"));
        tracker.sample(secs(60), &ids(), None, &[]);
        tracker.sample(secs(120), &ids(), Some(&only("M")), &only("M"));
        tracker.sample(secs(180), &ids(), None, &[]);
        tracker.sample(secs(240), &ids(), None, &[]);
        assert_eq!(
            tracker.verdict(&[], &[]).connection,
            [
                "M was connected with no aranet handle in 2 samples in a row from t=0s",
                "the Bluetooth stack couldn't be checked in 2 samples in a row",
            ]
        );
    }

    #[test]
    fn a_manager_cut_recovers_only_within_the_window() {
        let mut tracker = Tracker::new(secs(90), true);
        tracker.fault_injected(Side::Manager, "M", secs(300));
        tracker.reconnect_succeeded(Side::Manager, secs(250));
        tracker.reconnect_succeeded(Side::Manager, secs(314));
        tracker.fault_injected(Side::Manager, "M", secs(900));
        tracker.reconnect_succeeded(Side::Manager, secs(991));
        assert_eq!(
            tracker.verdict(&[], &[]).faults,
            ["link cut on the manager sensor M at t=900s: no ReconnectSucceeded within 90s"]
        );
        assert_eq!(tracker.m_reconnects, 3);
        assert_eq!(tracker.unrecovered(), 1);
        let report = tracker.fault_report();
        assert_eq!(report[0]["recovered_after_s"], json!(14));
        assert_eq!(report[1]["recovered_after_s"], Value::Null);
    }

    #[test]
    fn a_reconnecting_cut_is_decided_by_the_first_read_that_starts_after_it() {
        let mut tracker = Tracker::new(secs(90), true);
        tracker.fault_injected(Side::Reconnecting, "R", secs(600));
        tracker.read_finished(secs(590), secs(605), false);
        tracker.read_finished(secs(650), secs(671), true);
        tracker.read_finished(secs(710), secs(711), false);
        tracker.fault_injected(Side::Reconnecting, "R", secs(1200));
        tracker.read_finished(secs(1250), secs(1290), false);
        tracker.read_finished(secs(1310), secs(1311), true);
        assert_eq!(
            tracker.verdict(&[], &[]).faults,
            ["link cut on the reconnecting sensor R at t=1200s: the next read failed"]
        );
        assert_eq!((tracker.reads_ok, tracker.reads_err), (2, 3));
        assert_eq!(tracker.fault_report()[0]["recovered_after_s"], json!(71));
    }

    #[test]
    fn a_cut_still_pending_at_shutdown_fails() {
        let mut tracker = Tracker::new(secs(90), true);
        tracker.fault_injected(Side::Reconnecting, "R", secs(3000));
        assert_eq!(
            tracker.verdict(&[], &[]).faults,
            ["link cut on the reconnecting sensor R at t=3000s: no read finished after it"]
        );
    }

    #[test]
    fn leftovers_hung_shutdown_steps_and_failed_cuts_fail() {
        let mut tracker = Tracker::new(secs(90), true);
        tracker.fault_errors = 1;
        let hung = only("the health monitor didn't stop within 30s of being cancelled");
        let verdict = tracker.verdict(&only("M"), &hung);
        assert_eq!(
            verdict.connection,
            [
                "M was still connected 5s after shutdown",
                "the health monitor didn't stop within 30s of being cancelled",
            ]
        );
        assert_eq!(
            verdict.faults,
            [
                "1 link cuts could not be made (see the log)",
                "no link was cut"
            ]
        );
    }

    #[test]
    fn a_clean_run_passes() {
        let mut tracker = Tracker::new(secs(90), true);
        tracker.sample(secs(0), &ids(), Some(&ids()), &[]);
        tracker.fault_injected(Side::Manager, "M", secs(900));
        tracker.reconnect_succeeded(Side::Manager, secs(921));
        tracker.sample(secs(960), &ids(), Some(&ids()), &[]);
        tracker.fault_injected(Side::Reconnecting, "R", secs(1800));
        tracker.read_finished(secs(1860), secs(1875), true);
        tracker.sample(secs(1920), &ids(), Some(&ids()), &[]);
        let verdict = tracker.verdict(&[], &[]);
        assert!(verdict.passed(), "{verdict:?}");
        assert_eq!(tracker.unrecovered(), 0);
        let report = tracker.fault_report();
        assert_eq!(report[0]["held_at_next_sample"], json!(true));
        assert_eq!(report[1]["held_at_next_sample"], json!(true));
    }

    #[test]
    fn a_recovered_link_must_still_be_up_and_held_at_the_next_sample() {
        let mut tracker = Tracker::new(secs(90), true);
        tracker.fault_injected(Side::Manager, "M", secs(900));
        tracker.reconnect_succeeded(Side::Manager, secs(921));
        tracker.sample(secs(960), &ids(), Some(&only("R")), &[]);
        tracker.fault_injected(Side::Reconnecting, "R", secs(1800));
        tracker.read_finished(secs(1860), secs(1875), true);
        tracker.sample(secs(1920), &ids(), None, &[]);
        tracker.sample(secs(1980), &ids(), Some(&ids()), &only("R"));
        assert_eq!(
            tracker.verdict(&[], &[]).faults,
            [
                "link cut on the manager sensor M at t=900s: it recovered after 21s, \
                 but the next sample found the link down or not held by aranet",
                "link cut on the reconnecting sensor R at t=1800s: it recovered after 75s, \
                 but the next sample found the link down or not held by aranet",
            ]
        );
        assert_eq!(tracker.unrecovered(), 2);
    }
}
