//! Background data collector for polling Aranet devices.
//!
//! This module provides a background collector that polls configured devices at their
//! specified intervals and stores readings in the database.
//!
//! # Concurrency Model
//!
//! The collector uses a "task per device" model:
//!
//! - A started collector is one *generation*: a [`CancellationToken`], a
//!   reload watcher task and one task per configured device, each with a child
//!   token. `CollectorState::lifecycle`, a `tokio::sync::Mutex`, owns it.
//!   [`Collector::start`], [`Collector::stop`] and the reload watcher take that
//!   lock, so they never interleave.
//! - A device task waits for its next tick inside a `select!` on its token.
//!   Each poll runs as its own task, tracked in `CollectorState::polls`: it
//!   waits for the Bluetooth adapter (or the token), then connects, reads and
//!   disconnects within `BLE_POLL_TIMEOUT` (180 s), and stores the reading.
//! - A device change ([`AppState::on_devices_changed`]) wakes the reload
//!   watcher. It compares the configured devices with the running tasks by
//!   address, case-insensitively. It stops the tasks of removed devices and of
//!   devices whose address spelling or poll interval changed, then starts tasks
//!   for devices that have none. Alias edits restart nothing, and per-device
//!   statistics keep their counters.
//!
//! ## Lock Acquisition
//!
//! Locks are taken in this order: `CollectorState::lifecycle`, then
//! `AppState::config`, then `CollectorState::device_stats`. Device tasks never
//! take `lifecycle`, and API handlers take it only through [`Collector::start`]
//! and [`Collector::stop`], holding no other lock, so `stop` and a reload can
//! wait for device tasks while holding it. A device task takes `device_stats`,
//! `config` (to look up the alias) and the store one at a time.
//!
//! ## Stopping
//!
//! A cancelled token stays cancelled, so neither a reload nor a racing start
//! can undo a stop. [`Collector::stop`] cancels the generation, waits up to
//! `TASK_STOP_TIMEOUT` for its tasks and aborts any that are left. A device
//! task stopped during a poll exits at once and leaves the poll to finish:
//! it disconnects and stores its reading, which is not broadcast. At service
//! shutdown the collector is stopped for good: a later [`Collector::start`]
//! starts nothing.
//!
//! ## Error Handling
//!
//! A failed poll (a connect, read or store error, or a poll that timed out) is
//! counted in the device's statistics and retried with exponential backoff:
//! after the n-th failure in a row, the next attempt comes
//! `poll_interval × 2^(n-1)` after the failed poll ended, capped at 5 minutes
//! or at the poll interval, whichever is longer. One successful poll restores
//! the normal cadence. A sensor that is switched off or out of range costs about
//! 30 s of Bluetooth scanning per attempt, during which no other device can
//! be polled, so backing off keeps it from occupying the adapter.
//!
//! A poll that overruns its interval is followed by one poll straight away, and
//! the interval counts from there
//! ([`MissedTickBehavior::Delay`](tokio::time::MissedTickBehavior::Delay)), so
//! missed ticks never fire back to back.
//!
//! Failures are logged at WARN for the first three attempts, at ERROR on the
//! fourth ("reducing log frequency") and on every 12th after that (about hourly
//! at the 5-minute cap), and at DEBUG otherwise. Each line names the device and
//! the time until the next attempt.
//!
//! # Example
//!
//! ```ignore
//! let collector = Collector::new(Arc::clone(&state));
//! collector.start().await;  // Returns immediately, collection is background
//!
//! // Later...
//! collector.stop().await;  // Cancels every task and waits for them
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::future::{BoxFuture, join_all};
use time::OffsetDateTime;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use aranet_core::Device;
use aranet_store::StoredReading;
use aranet_types::CurrentReading;

use crate::config::DeviceConfig;
use crate::state::{AppState, DeviceCollectionStats, ReadingEvent};

/// Per-device stagger interval to avoid BLE adapter contention on startup.
const DEVICE_STAGGER_SECS: u64 = 5;

/// Longest one poll (connect, read and disconnect) may take before it is
/// abandoned and counted as a failure, so a hung Bluetooth call can't hold the
/// adapter. The slowest successful poll, a first connect on Linux that pairs
/// the sensor and then waits for BlueZ's service discovery, takes up to about
/// 177 s.
pub(crate) const BLE_POLL_TIMEOUT: Duration = Duration::from_secs(180);

/// Longest wait between attempts to poll a device whose polls keep failing,
/// unless its poll interval is longer.
pub(crate) const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// Delay before the next attempt after the `failures`-th consecutive failure
/// (`failures >= 1`): the poll interval, doubled for each failure after the
/// first, capped at `MAX_BACKOFF` or the poll interval, whichever is longer.
///
/// An unreachable sensor costs about 30 s of Bluetooth scanning per
/// attempt, during which no other device can be polled, so it must not be
/// retried every interval.
pub(crate) fn backoff_delay(poll_interval: Duration, failures: u32) -> Duration {
    let cap = MAX_BACKOFF.max(poll_interval);
    poll_interval
        .saturating_mul(2u32.saturating_pow(failures.saturating_sub(1).min(16)))
        .min(cap)
}

/// Log level for the `failures`-th consecutive failure: WARN for 1-3, ERROR at
/// 4 and every 12th after, otherwise `None` (logged at DEBUG).
///
/// Every 12th failure is about once an hour once the delay has reached
/// `MAX_BACKOFF`.
pub(crate) fn failure_log_level(failures: u32) -> Option<tracing::Level> {
    match failures {
        1..=3 => Some(tracing::Level::WARN),
        4 => Some(tracing::Level::ERROR),
        n if n > 4 && n.is_multiple_of(12) => Some(tracing::Level::ERROR),
        _ => None,
    }
}

/// Log the `failures`-th consecutive failed poll of `address` at the level
/// `failure_log_level` picks, with the error text and the time until the next
/// attempt.
fn log_poll_failure(address: &str, error: &str, failures: u32, next_attempt: Duration) {
    let next = next_attempt.as_secs();
    match failure_log_level(failures) {
        Some(tracing::Level::WARN) => {
            warn!(
                "Failed to poll {address}: {error} (attempt {failures}); next attempt in {next}s"
            );
        }
        Some(_) if failures == 4 => {
            error!(
                "Failed to poll {address} after {failures} attempts, reducing log frequency; next attempt in {next}s"
            );
        }
        Some(_) => {
            error!(
                "Failed to poll {address} ({failures} consecutive failures): {error}; next attempt in {next}s"
            );
        }
        None => {
            debug!(
                "Failed to poll {address} ({failures} consecutive failures): {error}; next attempt in {next}s"
            );
        }
    }
}

/// How long `stop` and a reload wait for cancelled tasks before aborting them.
/// Cancelled tasks exit at their next `select!`, so this is a safety net.
pub(crate) const TASK_STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// How long shutdown waits for polls still in flight after the collector
/// stopped, so they can disconnect and store their reading.
pub(crate) const POLL_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// The collector's current generation, if it has been started.
///
/// Lives in `CollectorState::lifecycle`. Only [`Collector::start`],
/// [`Collector::stop`] (with [`Collector::shutdown`]) and the reload watcher
/// lock it.
#[derive(Default)]
pub(crate) struct Lifecycle {
    generation: Option<Generation>,
    /// Id of the next generation, so a reload watcher recognises its own.
    next_id: u64,
    /// Set by [`Collector::shutdown`] and never cleared: `start` starts
    /// nothing once the service is shutting down.
    shutting_down: bool,
}

/// The tasks one `start` created and `stop` tears down.
struct Generation {
    id: u64,
    /// Parent of every device task's token.
    cancel: CancellationToken,
    watcher: JoinHandle<()>,
    /// Device tasks, keyed by lowercase address.
    devices: HashMap<String, DeviceTask>,
}

/// A running device task and the settings it was started with.
struct DeviceTask {
    address: String,
    poll_interval: u64,
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}

/// Result of attempting to start the collector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectorStartResult {
    Started,
    AlreadyRunning,
    NoDevicesConfigured,
}

/// Result of stopping the collector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectorStopResult {
    /// The collector was polling at least one device and has stopped.
    Stopped,
    /// The collector wasn't polling any device. A collector started without
    /// devices is still stopped, so devices added later aren't polled.
    NotRunning,
}

/// Background collector that polls devices on their configured intervals.
pub struct Collector {
    state: Arc<AppState>,
}

impl Collector {
    /// Create a new collector.
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Start collecting data from all configured devices.
    ///
    /// This spawns a separate task for each device that polls at the configured interval.
    /// Returns immediately; collection happens in the background.
    ///
    /// Also spawns a reload watcher task that applies later device changes made
    /// via the API. With no devices configured this returns
    /// [`CollectorStartResult::NoDevicesConfigured`] but keeps the watcher, so
    /// devices added later are polled.
    ///
    /// Once the service has begun shutting down, this starts nothing and
    /// returns [`CollectorStartResult::NoDevicesConfigured`].
    pub async fn start(&self) -> CollectorStartResult {
        let mut lifecycle = self.state.collector.lifecycle.lock().await;
        if lifecycle.shutting_down {
            info!("Not starting the collector: the service is shutting down");
            return CollectorStartResult::NoDevicesConfigured;
        }
        if lifecycle
            .generation
            .as_ref()
            .is_some_and(|generation| !generation.devices.is_empty())
        {
            return CollectorStartResult::AlreadyRunning;
        }

        let Lifecycle {
            generation,
            next_id,
            ..
        } = &mut *lifecycle;
        let generation = generation.get_or_insert_with(|| {
            let id = *next_id;
            *next_id += 1;
            let cancel = CancellationToken::new();
            // Subscribe before the config is read below, so any later edit
            // wakes the watcher.
            let reload_rx = self.state.collector.subscribe_reload();
            let watcher = tokio::spawn(watch_for_reload(
                Arc::clone(&self.state),
                id,
                cancel.clone(),
                reload_rx,
            ));
            Generation {
                id,
                cancel,
                watcher,
                devices: HashMap::new(),
            }
        });

        let devices = self.state.config.read().await.devices.clone();
        reconcile(&self.state, generation, &devices).await;

        if generation.devices.is_empty() {
            info!("No devices configured for collection");
            CollectorStartResult::NoDevicesConfigured
        } else {
            info!(
                "Collector started for {} device(s)",
                generation.devices.len()
            );
            CollectorStartResult::Started
        }
    }

    /// Stop the collector and wait for its tasks to finish.
    ///
    /// Cancels every device task and the reload watcher, also when no device
    /// is configured, so devices added later aren't polled until the next
    /// start. A Bluetooth poll in flight finishes in the background; its
    /// reading is stored but not broadcast. Returns
    /// [`CollectorStopResult::NotRunning`] if no device was being polled.
    pub async fn stop(&self) -> CollectorStopResult {
        self.stop_generation(false).await
    }

    /// Stop the collector for good, at service shutdown: what
    /// [`stop`](Self::stop) does, and every later [`start`](Self::start)
    /// starts nothing, so a start sent while the service shuts down can't
    /// leave a collector running.
    pub(crate) async fn shutdown(&self) -> CollectorStopResult {
        self.stop_generation(true).await
    }

    /// Tear down the current generation, holding the lifecycle lock until
    /// its tasks have finished. With `shutting_down`, also refuse later starts.
    async fn stop_generation(&self, shutting_down: bool) -> CollectorStopResult {
        let mut lifecycle = self.state.collector.lifecycle.lock().await;
        lifecycle.shutting_down |= shutting_down;
        let Some(generation) = lifecycle.generation.take() else {
            return CollectorStopResult::NotRunning;
        };

        info!("Stopping collector");
        generation.cancel.cancel();
        self.state.collector.set_running(false);
        let result = if generation.devices.is_empty() {
            CollectorStopResult::NotRunning
        } else {
            CollectorStopResult::Stopped
        };

        let mut tasks = vec![generation.watcher];
        tasks.extend(generation.devices.into_values().map(|task| task.handle));
        join_or_abort("collector task", tasks, TASK_STOP_TIMEOUT).await;

        // After the join, so no device task can set it again.
        for stat in self.state.collector.device_stats.write().await.iter_mut() {
            stat.polling = false;
        }
        info!("Collector stopped");
        result
    }

    /// Check if the collector is running.
    pub fn is_running(&self) -> bool {
        self.state.collector.is_running()
    }
}

/// Wait up to `limit` for `tasks` to finish, then abort the ones left.
pub(crate) async fn join_or_abort(what: &str, mut tasks: Vec<JoinHandle<()>>, limit: Duration) {
    match tokio::time::timeout(limit, join_all(tasks.iter_mut())).await {
        Ok(results) => {
            for result in results {
                if let Err(e) = result
                    && e.is_panic()
                {
                    error!("A {what} panicked: {e}");
                }
            }
        }
        Err(_) => {
            let left = tasks.iter().filter(|task| !task.is_finished()).count();
            warn!(
                "{left} {what}(s) did not stop within {}s, aborting",
                limit.as_secs()
            );
            for task in &tasks {
                task.abort();
            }
        }
    }
}

/// The alias configured for `address` right now (exact address match).
pub(crate) async fn configured_alias(state: &AppState, address: &str) -> Option<String> {
    state
        .config
        .read()
        .await
        .devices
        .iter()
        .find(|device| device.address == address)
        .and_then(|device| device.alias.clone())
}

/// Make `generation`'s device tasks match `desired`, the configured devices.
///
/// Tasks are keyed by lowercase address, like the config's duplicate check and
/// the API's device lookups. A task is stopped only when its device is gone or
/// its exact address or poll interval changed, so alias edits restart nothing.
/// New tasks' first polls are `DEVICE_STAGGER_SECS` apart.
async fn reconcile(state: &Arc<AppState>, generation: &mut Generation, desired: &[DeviceConfig]) {
    let wanted: HashMap<String, &DeviceConfig> = desired
        .iter()
        .map(|device| (device.address.to_lowercase(), device))
        .collect();

    let outdated: Vec<String> = generation
        .devices
        .iter()
        .filter(|(key, task)| {
            wanted.get(key.as_str()).is_none_or(|device| {
                device.address != task.address || device.poll_interval != task.poll_interval
            })
        })
        .map(|(key, _)| key.clone())
        .collect();
    let mut stopped = Vec::new();
    let mut handles = Vec::new();
    for key in outdated {
        if let Some(task) = generation.devices.remove(&key) {
            info!("Stopping collector for {}", task.address);
            task.cancel.cancel();
            stopped.push(key);
            handles.push(task.handle);
        }
    }
    join_or_abort("device task", handles, TASK_STOP_TIMEOUT).await;

    sync_device_stats(state, desired).await;
    // A task stopped during a poll never cleared its flag. Match by key: when
    // only the address's spelling changed, `sync_device_stats` has already
    // renamed the kept entry to the new spelling.
    if !stopped.is_empty() {
        for stat in state.collector.device_stats.write().await.iter_mut() {
            if stopped.contains(&stat.device_id.to_lowercase()) {
                stat.polling = false;
            }
        }
    }

    let mut spawned = 0;
    for device in desired {
        let key = device.address.to_lowercase();
        if generation.devices.contains_key(&key) {
            continue;
        }
        let first_poll = Duration::from_secs(spawned * DEVICE_STAGGER_SECS);
        spawned += 1;
        let cancel = generation.cancel.child_token();
        let handle = tokio::spawn(collect_device(
            Arc::clone(state),
            device.address.clone(),
            Duration::from_secs(device.poll_interval),
            first_poll,
            cancel.clone(),
        ));
        generation.devices.insert(
            key,
            DeviceTask {
                address: device.address.clone(),
                poll_interval: device.poll_interval,
                cancel,
                handle,
            },
        );
    }

    state.collector.set_running(!generation.devices.is_empty());
}

/// Rebuild the per-device statistics in config order.
///
/// A device that stays configured keeps its counters, timestamps and last
/// error, and takes the config's spelling, alias and poll interval. A new
/// device starts from zero, and a removed one is dropped.
async fn sync_device_stats(state: &AppState, desired: &[DeviceConfig]) {
    let mut stats = state.collector.device_stats.write().await;
    let mut previous: HashMap<String, DeviceCollectionStats> = stats
        .drain(..)
        .map(|stat| (stat.device_id.to_lowercase(), stat))
        .collect();
    *stats = desired
        .iter()
        .map(|device| {
            let kept = previous.remove(&device.address.to_lowercase());
            let fresh = || DeviceCollectionStats {
                device_id: String::new(),
                alias: None,
                poll_interval: 0,
                last_poll_at: None,
                last_error_at: None,
                last_error: None,
                last_poll_duration_ms: None,
                success_count: 0,
                failure_count: 0,
                polling: false,
            };
            DeviceCollectionStats {
                device_id: device.address.clone(),
                alias: device.alias.clone(),
                poll_interval: device.poll_interval,
                ..kept.unwrap_or_else(fresh)
            }
        })
        .collect();
}

/// Apply device changes to generation `id` until `cancel` is cancelled.
async fn watch_for_reload(
    state: Arc<AppState>,
    id: u64,
    cancel: CancellationToken,
    mut reload_rx: watch::Receiver<u64>,
) {
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return,
            changed = reload_rx.changed() => {
                if changed.is_err() {
                    return;
                }
            }
        }

        // `stop` holds the lock while it waits for this task, so stop waiting
        // for the lock as soon as the token is cancelled.
        let mut lifecycle = tokio::select! {
            biased;
            _ = cancel.cancelled() => return,
            lifecycle = state.collector.lifecycle.lock() => lifecycle,
        };
        // The config read below includes every change signalled so far.
        reload_rx.mark_unchanged();
        let Some(generation) = lifecycle
            .generation
            .as_mut()
            .filter(|generation| generation.id == id)
        else {
            continue;
        };

        info!("Device configuration changed, updating collector tasks");
        let devices = state.config.read().await.devices.clone();
        reconcile(&state, generation, &devices).await;
    }
}

/// Collect readings from a single device every `poll_interval` while polls
/// succeed, the first time after `first_poll`, until `cancel` is cancelled.
/// After a failed poll, the next attempt waits for `backoff_delay` instead.
/// The module docs' Error Handling section has the details.
async fn collect_device(
    state: Arc<AppState>,
    address: String,
    poll_interval: Duration,
    first_poll: Duration,
    cancel: CancellationToken,
) {
    let alias = configured_alias(&state, &address)
        .await
        .unwrap_or_else(|| address.clone());
    info!(
        "Starting collector for {} (alias: {}, interval: {}s)",
        address,
        alias,
        poll_interval.as_secs()
    );

    let mut ticker = tokio::time::interval_at(Instant::now() + first_poll, poll_interval);
    // A poll that overran the interval is followed by one poll straight away,
    // and the interval counts from there, instead of every missed tick firing
    // back to back (the default, `Burst`).
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut consecutive_failures = 0u32;

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = ticker.tick() => {}
        }

        // Update stats: mark as polling
        update_device_stat(&state, &address, |stat| {
            stat.polling = true;
        })
        .await;

        // The poll is its own task, so stopping never waits for Bluetooth.
        let started = Instant::now();
        let poll = state.collector.polls.spawn(poll_once(
            Arc::clone(&state),
            address.clone(),
            cancel.clone(),
        ));
        let outcome = tokio::select! {
            biased;
            // Dropping the handle detaches the poll, which finishes on its own.
            _ = cancel.cancelled() => break,
            outcome = poll => outcome,
        };
        let poll_duration = started.elapsed();
        let result = match outcome {
            Ok(Some(result)) => result.map_err(|e| e.to_string()),
            // Cancelled while waiting for the Bluetooth adapter.
            Ok(None) => break,
            Err(e) => Err(format!("poll task failed: {e}")),
        };

        match result {
            Ok(reading) => {
                consecutive_failures = 0;
                debug!(
                    "Collected reading from {}: CO2={} (took {:.1}s)",
                    address,
                    reading.co2,
                    poll_duration.as_secs_f64()
                );

                // Update stats
                update_device_stat(&state, &address, |stat| {
                    stat.last_poll_at = Some(OffsetDateTime::now_utc());
                    stat.last_error_at = None;
                    stat.last_error = None;
                    stat.last_poll_duration_ms = Some(poll_duration.as_millis() as u64);
                    stat.success_count += 1;
                    stat.polling = false;
                })
                .await;

                // Broadcast the reading to WebSocket clients
                let event = ReadingEvent {
                    device_id: address.clone(),
                    reading,
                };
                // Check thresholds and send desktop notification
                #[cfg(feature = "notifications")]
                notify_if_needed(&state, &address, &event.reading).await;
                if state.readings_tx.send(event).is_err() {
                    debug!("No active WebSocket subscribers for reading broadcast");
                }
            }
            Err(e) => {
                consecutive_failures += 1;

                // Update stats
                update_device_stat(&state, &address, |stat| {
                    stat.last_error_at = Some(OffsetDateTime::now_utc());
                    stat.last_error = Some(e.clone());
                    stat.last_poll_duration_ms = Some(poll_duration.as_millis() as u64);
                    stat.failure_count += 1;
                    stat.polling = false;
                })
                .await;

                // Back off: the next attempt comes `delay` after this failed
                // poll ended (`reset_after` counts from now), not at the next
                // tick. This also drops any tick that fell due while it ran.
                let delay = backoff_delay(poll_interval, consecutive_failures);
                ticker.reset_after(delay);
                log_poll_failure(&address, &e, consecutive_failures, delay);
            }
        }
    }

    info!("Collector for {} stopped", address);
}

/// Update stats for a specific device.
async fn update_device_stat<F>(state: &AppState, device_id: &str, update_fn: F)
where
    F: FnOnce(&mut DeviceCollectionStats),
{
    let mut stats = state.collector.device_stats.write().await;
    if let Some(stat) = stats.iter_mut().find(|s| s.device_id == device_id) {
        update_fn(stat);
    }
}

/// Take the Bluetooth adapter, read `address` once and store the reading.
///
/// Returns `None`, without touching the adapter, if `cancel` fires while this
/// waits for the adapter. Once the read has started it runs until it finishes
/// or `BLE_POLL_TIMEOUT` passes, even if `cancel` fires, so the device is
/// disconnected and a reading that arrives is stored.
async fn poll_once(
    state: Arc<AppState>,
    address: String,
    cancel: CancellationToken,
) -> Option<Result<StoredReading, CollectorError>> {
    // Serialize BLE adapter access — only one device at a time
    let permit = tokio::select! {
        biased;
        _ = cancel.cancelled() => return None,
        permit = state.ble_semaphore.acquire() => match permit {
            Ok(permit) => permit,
            Err(_) => return Some(Err(CollectorError::BleBusy)),
        },
    };

    // Connect, read and disconnect, within the time limit
    let read = tokio::time::timeout(
        BLE_POLL_TIMEOUT,
        state.collector.reader().read_current(&address),
    )
    .await;

    // Drop the BLE permit so other devices can poll
    drop(permit);
    let reading = match read {
        Ok(Ok(reading)) => reading,
        Ok(Err(e)) => return Some(Err(e)),
        Err(_) => return Some(Err(CollectorError::Timeout(BLE_POLL_TIMEOUT))),
    };

    // Store the reading
    let stored = state
        .with_store_write({
            let address = address.clone();
            move |store| store.insert_reading(&address, &reading)
        })
        .await
        .map(|row_id| StoredReading::from_reading_with_id(&address, &reading, row_id))
        .map_err(CollectorError::Store);
    Some(stored)
}

/// Collector errors.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CollectorError {
    #[error("BLE adapter busy (semaphore closed)")]
    BleBusy,
    #[error("Failed to connect: {0}")]
    Connect(aranet_core::Error),
    #[error("Failed to read: {0}")]
    Read(aranet_core::Error),
    #[error("Failed to store: {0}")]
    Store(aranet_store::Error),
    /// The poll didn't finish within its time limit.
    #[error("Bluetooth poll timed out after {}s", .0.as_secs())]
    Timeout(Duration),
}

/// Reads the current values of one device for the collector.
///
/// The collector calls it while holding the BLE permit
/// (`AppState::ble_semaphore`), so two reads never overlap. Production uses
/// [`BleDeviceReader`]; tests put a fake behind `CollectorState::reader()`
/// so they never touch Bluetooth.
pub(crate) trait DeviceReader: Send + Sync + 'static {
    /// Connect to `address`, read the current values and disconnect.
    fn read_current<'a>(
        &'a self,
        address: &'a str,
    ) -> BoxFuture<'a, Result<CurrentReading, CollectorError>>;
}

/// Reads devices over Bluetooth with [`aranet_core::Device`].
pub(crate) struct BleDeviceReader;

impl DeviceReader for BleDeviceReader {
    fn read_current<'a>(
        &'a self,
        address: &'a str,
    ) -> BoxFuture<'a, Result<CurrentReading, CollectorError>> {
        Box::pin(async move {
            // Connect with moderate timeouts — fail fast and retry rather than blocking
            let config = aranet_core::device::ConnectionConfig::default();
            let device = Device::connect_with_config(address, config)
                .await
                .map_err(CollectorError::Connect)?;

            // Read current values
            let reading_result = device.read_current().await;

            // Always disconnect after the read attempt to avoid relying on best-effort Drop cleanup.
            if let Err(e) = device.disconnect().await {
                debug!("Failed to disconnect {} after poll: {}", address, e);
            }

            reading_result.map_err(CollectorError::Read)
        })
    }
}

/// Desktop alerts for readings that reach a configured threshold.
#[cfg(feature = "notifications")]
mod notifications {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::{Duration, Instant};

    use aranet_store::StoredReading;
    use tokio::sync::Semaphore;

    use super::configured_alias;
    use crate::config::NotificationConfig;
    use crate::state::AppState;

    /// Shows one desktop notification, given its title and body. It may block.
    pub(crate) type ShowFn = Arc<dyn Fn(String, String) -> Result<(), String> + Send + Sync>;

    /// Dispatches desktop alerts, at most one per device per cooldown.
    ///
    /// `CollectorState` owns it, so every `AppState` has its own cooldowns.
    pub(crate) struct Notifier {
        /// When the last alert for each device address was dispatched.
        last: Mutex<HashMap<String, Instant>>,
        /// One permit for each device address. A `show()` job holds it until
        /// `show()` returns, so a notification service that stops answering
        /// ties up at most one blocking thread per device.
        running: Mutex<HashMap<String, Arc<Semaphore>>>,
        show: ShowFn,
    }

    impl Notifier {
        /// A notifier that shows alerts on the desktop through `notify-rust`.
        pub(crate) fn desktop() -> Self {
            Self {
                last: Mutex::new(HashMap::new()),
                running: Mutex::new(HashMap::new()),
                show: Arc::new(show_desktop_notification),
            }
        }

        /// A notifier that hands each alert's title and body to `show`.
        #[cfg(test)]
        pub(crate) fn with_show(show: ShowFn) -> Self {
            Self {
                last: Mutex::new(HashMap::new()),
                running: Mutex::new(HashMap::new()),
                show,
            }
        }

        /// Alert on `reading` from the device at `address`, titled with `name`,
        /// unless it reaches no threshold in `config`, the device's cooldown is
        /// still running, or its last `show()` hasn't returned. Returns true
        /// when an alert was dispatched.
        ///
        /// The cooldown starts when the alert is dispatched, so a `show()` that
        /// fails or hangs isn't retried on every poll. `show()` runs on Tokio's
        /// blocking pool and nothing waits for it, so this must be called
        /// inside a Tokio runtime.
        pub(crate) fn maybe_notify(
            &self,
            address: &str,
            name: &str,
            reading: &StoredReading,
            config: &NotificationConfig,
        ) -> bool {
            let cooldown = Duration::from_secs(config.cooldown_secs);
            let (body, permit) = {
                let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
                if last
                    .get(address)
                    .is_some_and(|sent| sent.elapsed() < cooldown)
                {
                    return false;
                }
                let Some(body) = alert_message(reading, config) else {
                    return false;
                };
                let running = Arc::clone(
                    self.running
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .entry(address.to_string())
                        .or_insert_with(|| Arc::new(Semaphore::new(1))),
                );
                let Ok(permit) = running.try_acquire_owned() else {
                    tracing::debug!(
                        "Skipping desktop notification for {address}: the last one hasn't returned"
                    );
                    return false;
                };
                last.insert(address.to_string(), Instant::now());
                (body, permit)
            };

            let title = format!("Aranet Alert: {name}");
            let show = Arc::clone(&self.show);
            // show() blocks: a D-Bus round trip on Linux, synchronous system
            // calls on macOS and Windows. Keep it off the async workers.
            tokio::task::spawn_blocking(move || {
                // Held until show() returns or panics.
                let _running = permit;
                if let Err(e) = show(title, body) {
                    tracing::warn!("Failed to send desktop notification: {e}");
                }
            });
            true
        }
    }

    /// Show `title` and `body` as a desktop notification. Blocks until the
    /// platform's notification service has taken it.
    fn show_desktop_notification(title: String, body: String) -> Result<(), String> {
        match notify_rust::Notification::new()
            .summary(&title)
            .body(&body)
            .icon("dialog-warning")
            .timeout(notify_rust::Timeout::Milliseconds(10000))
            .show()
        {
            // On macOS the notification is sent when the handle is dropped,
            // which happens here, on the same thread.
            Ok(_handle) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// The alert text for `reading`: one line for each threshold it reaches,
    /// or `None` if it reaches none. A CO2 value of 0 (a sensor without CO2)
    /// never alerts.
    pub(crate) fn alert_message(
        reading: &StoredReading,
        config: &NotificationConfig,
    ) -> Option<String> {
        let mut lines = Vec::new();
        if reading.co2 > 0 && reading.co2 >= config.co2_threshold {
            lines.push(format!(
                "CO\u{2082}: {} ppm (threshold: {})",
                reading.co2, config.co2_threshold
            ));
        }
        if let Some(radon) = reading.radon
            && radon >= config.radon_threshold
        {
            lines.push(format!(
                "Radon: {radon} Bq/m\u{00b3} (threshold: {})",
                config.radon_threshold
            ));
        }
        if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n"))
        }
    }

    /// Alert on `reading` from the device at `address` if notifications are
    /// enabled, naming the device by its current alias.
    ///
    /// Only copies of the settings and the alias are kept, so no config lock
    /// is held while the alert is dispatched.
    pub(crate) async fn notify_if_needed(state: &AppState, address: &str, reading: &StoredReading) {
        // The read guard is a temporary, released at the end of this statement.
        let config = state.config.read().await.notifications.clone();
        if !config.enabled {
            return;
        }
        let alias = configured_alias(state, address).await;
        let name = alias.as_deref().unwrap_or(address);
        state
            .collector
            .notifier
            .maybe_notify(address, name, reading, &config);
    }

    /// Every test here dispatches through a fake `show`, never the desktop.
    /// They run on real time: a running `spawn_blocking` job would stop a
    /// paused clock from advancing. The one exception starts the collector on
    /// the paused clock, like the other collector tests; its `show` returns at
    /// once.
    #[cfg(test)]
    mod tests {
        use std::sync::Arc;
        use std::time::Duration;

        use aranet_store::StoredReading;
        use aranet_types::CurrentReading;
        use tokio::sync::mpsc;
        use tokio::sync::mpsc::error::TryRecvError;
        use tokio::time::timeout;

        use super::{Notifier, ShowFn, alert_message, notify_if_needed};
        use crate::collector::{Collector, CollectorStartResult};
        use crate::config::{Config, DeviceConfig, NotificationConfig};
        use crate::state::CollectorState;
        use crate::test_support::{FakeReader, TestState, settle};

        const A: &str = "AA:BB:CC:DD:EE:FF";
        const B: &str = "11:22:33:44:55:66";

        type Alerts = mpsc::UnboundedReceiver<(String, String)>;

        /// Notifications on, CO2 threshold 1000 ppm, radon 300 Bq/m³, 300 s cooldown.
        fn enabled() -> NotificationConfig {
            NotificationConfig {
                enabled: true,
                co2_threshold: 1000,
                radon_threshold: 300,
                cooldown_secs: 300,
            }
        }

        fn reading(co2: u16, radon: Option<u32>) -> StoredReading {
            let current = CurrentReading {
                co2,
                radon,
                ..CurrentReading::default()
            };
            StoredReading::from_reading_with_id(A, &current, 1)
        }

        fn reading_with_co2(co2: u16) -> StoredReading {
            reading(co2, None)
        }

        /// Device A, called `alias`, with notifications on.
        fn config_with_alias(alias: &str) -> Config {
            Config {
                devices: vec![DeviceConfig {
                    address: A.to_string(),
                    alias: Some(alias.to_string()),
                    poll_interval: 60,
                }],
                notifications: enabled(),
                ..Config::default()
            }
        }

        /// A `show` that hands each (title, body) to the returned receiver.
        fn recording_show() -> (ShowFn, Alerts) {
            let (tx, rx) = mpsc::unbounded_channel::<(String, String)>();
            let show: ShowFn = Arc::new(move |title: String, body: String| {
                tx.send((title, body)).map_err(|e| e.to_string())
            });
            (show, rx)
        }

        /// Application state whose collector sends its alerts to `show`.
        fn state_with(config: Config, show: ShowFn) -> TestState {
            let collector = CollectorState::new().with_notifier(Notifier::with_show(show));
            TestState::with_collector(config, collector)
        }

        /// The next alert passed to `show`.
        async fn next_alert(alerts: &mut Alerts) -> (String, String) {
            timeout(Duration::from_secs(5), alerts.recv())
                .await
                .expect("show() ran within 5 s")
                .expect("the show function is alive")
        }

        /// Wait until no `show()` job for `address` is running, so its next
        /// alert can only be held back by its cooldown.
        async fn wait_for_show_job(notifier: &Notifier, address: &str) {
            // The map guard is dropped at the end of this statement, before the await.
            let running = notifier.running.lock().unwrap().get(address).cloned();
            if let Some(running) = running {
                let permit = timeout(Duration::from_secs(5), running.acquire())
                    .await
                    .expect("the show() job ended within 5 s")
                    .expect("the semaphore is never closed");
                drop(permit);
            }
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn notification_does_not_block_config_writers() {
            let (tx, mut alerts) = mpsc::unbounded_channel::<(String, String)>();
            // Like a notification service that takes 2 s to answer.
            let show: ShowFn = Arc::new(move |title: String, body: String| -> Result<(), String> {
                tx.send((title, body)).map_err(|e| e.to_string())?;
                std::thread::sleep(Duration::from_secs(2));
                Ok(())
            });
            let t = state_with(config_with_alias("Office"), show);

            let state = Arc::clone(&t.state);
            let notify = tokio::spawn(async move {
                notify_if_needed(&state, A, &reading_with_co2(1500)).await;
            });
            // show() has started and is now blocking.
            next_alert(&mut alerts).await;

            // Adding, editing or removing a device through the API takes this lock.
            let guard = timeout(Duration::from_millis(500), t.state.config.write())
                .await
                .expect("a config write waited for the desktop notification");
            drop(guard);
            // show() is still asleep, so returning now means it runs elsewhere.
            timeout(Duration::from_millis(500), notify)
                .await
                .expect("notify_if_needed waited for show()")
                .expect("notify_if_needed did not panic");
        }

        #[tokio::test]
        async fn notifier_cooldown_suppresses_repeat_alert() {
            let (show, mut alerts) = recording_show();
            let notifier = Notifier::with_show(show);
            let config = enabled();
            let reading = reading_with_co2(1500);

            assert!(notifier.maybe_notify(A, "Office", &reading, &config));
            let (title, _) = next_alert(&mut alerts).await;
            assert_eq!(title, "Aranet Alert: Office");
            // Only the cooldown may refuse the next alert, not A's running job.
            wait_for_show_job(&notifier, A).await;
            assert!(
                !notifier.maybe_notify(A, "Office", &reading, &config),
                "a second alert was dispatched inside the cooldown"
            );
            assert_eq!(alerts.try_recv(), Err(TryRecvError::Empty));
        }

        #[tokio::test]
        async fn notifier_cooldown_is_per_device() {
            let (show, mut alerts) = recording_show();
            let notifier = Notifier::with_show(show);
            let config = enabled();
            let reading = reading_with_co2(1500);

            assert!(notifier.maybe_notify(A, "Office", &reading, &config));
            assert!(
                notifier.maybe_notify(B, "Kitchen", &reading, &config),
                "device A's cooldown held back device B's alert"
            );

            // The two show() calls run on different blocking threads, in either order.
            let mut titles = vec![
                next_alert(&mut alerts).await.0,
                next_alert(&mut alerts).await.0,
            ];
            titles.sort();
            assert_eq!(titles, ["Aranet Alert: Kitchen", "Aranet Alert: Office"]);
        }

        #[tokio::test]
        async fn hung_show_holds_back_only_its_device() {
            let (tx, mut alerts) = mpsc::unbounded_channel::<(String, String)>();
            let (release, released) = std::sync::mpsc::channel::<()>();
            let released = std::sync::Mutex::new(Some(released));
            // The first show() hangs, like a notification service that stops
            // answering, until the test releases it (at most 5 s). Later ones
            // return at once.
            let show: ShowFn = Arc::new(move |title: String, body: String| -> Result<(), String> {
                let hang = released.lock().unwrap().take();
                tx.send((title, body)).map_err(|e| e.to_string())?;
                if let Some(released) = hang {
                    released.recv_timeout(Duration::from_secs(5)).ok();
                }
                Ok(())
            });
            let notifier = Notifier::with_show(show);
            // No cooldown, so only a running show() can hold an alert back.
            let config = NotificationConfig {
                cooldown_secs: 0,
                ..enabled()
            };
            let reading = reading_with_co2(1500);

            assert!(notifier.maybe_notify(A, "Office", &reading, &config));
            let dispatched = notifier.last.lock().unwrap()[A];
            // Device A's show() has started and hangs.
            next_alert(&mut alerts).await;
            assert!(
                !notifier.maybe_notify(A, "Office", &reading, &config),
                "a second show() started while device A's first had not returned"
            );
            assert_eq!(
                notifier.last.lock().unwrap()[A],
                dispatched,
                "an alert held back by a running show() restarted the cooldown"
            );
            assert!(
                notifier.maybe_notify(B, "Kitchen", &reading, &config),
                "device A's hung show() held back device B"
            );
            assert_eq!(next_alert(&mut alerts).await.0, "Aranet Alert: Kitchen");

            // Let A's show() return, and wait until its job has let go of A.
            release
                .send(())
                .expect("device A's show() is still waiting");
            wait_for_show_job(&notifier, A).await;
            assert!(
                notifier.maybe_notify(A, "Office", &reading, &config),
                "device A got no alert after its show() returned"
            );
            assert_eq!(next_alert(&mut alerts).await.0, "Aranet Alert: Office");
        }

        #[test]
        fn alert_message_thresholds() {
            let config = enabled();
            // Aranet2 and AranetRn+ readings carry co2 = 0: never a CO2 alert,
            // even with a threshold of 0.
            let zero_threshold = NotificationConfig {
                co2_threshold: 0,
                ..enabled()
            };
            assert_eq!(alert_message(&reading(0, None), &zero_threshold), None);
            assert_eq!(
                alert_message(&reading(1000, None), &config).as_deref(),
                Some("CO\u{2082}: 1000 ppm (threshold: 1000)")
            );
            assert_eq!(
                alert_message(&reading(0, Some(300)), &config).as_deref(),
                Some("Radon: 300 Bq/m\u{00b3} (threshold: 300)")
            );
            assert_eq!(
                alert_message(&reading(1500, Some(400)), &config).as_deref(),
                Some(
                    "CO\u{2082}: 1500 ppm (threshold: 1000)\nRadon: 400 Bq/m\u{00b3} (threshold: 300)"
                )
            );
            assert_eq!(alert_message(&reading(999, Some(299)), &config), None);
        }

        #[tokio::test]
        async fn failed_show_still_starts_cooldown() {
            let (tx, mut alerts) = mpsc::unbounded_channel::<(String, String)>();
            let show: ShowFn = Arc::new(move |title: String, body: String| -> Result<(), String> {
                tx.send((title, body)).map_err(|e| e.to_string())?;
                Err("no session bus".to_string())
            });
            let notifier = Notifier::with_show(show);
            let config = enabled();
            let reading = reading_with_co2(1500);

            assert!(notifier.maybe_notify(A, "Office", &reading, &config));
            next_alert(&mut alerts).await;
            // show() has failed; wait until its job has given A's permit back.
            wait_for_show_job(&notifier, A).await;
            assert!(
                !notifier.maybe_notify(A, "Office", &reading, &config),
                "a failed show() was retried inside the cooldown"
            );
            assert_eq!(alerts.try_recv(), Err(TryRecvError::Empty));
        }

        #[tokio::test]
        async fn notification_uses_current_alias() {
            let (show, mut alerts) = recording_show();
            let t = state_with(config_with_alias("Office"), show);
            // A rename through the API edits the config in place; the device
            // loop keeps running.
            t.state.config.write().await.devices[0].alias = Some("Kitchen".to_string());

            notify_if_needed(&t.state, A, &reading_with_co2(1500)).await;

            let (title, body) = next_alert(&mut alerts).await;
            assert_eq!(title, "Aranet Alert: Kitchen");
            assert_eq!(body, "CO\u{2082}: 1500 ppm (threshold: 1000)");
        }

        #[tokio::test]
        async fn disabled_notifications_dispatch_nothing() {
            let (show, _alerts) = recording_show();
            let mut config = config_with_alias("Office");
            config.notifications.enabled = false;
            let t = state_with(config, show);

            notify_if_needed(&t.state, A, &reading_with_co2(5000)).await;

            let last = t.state.collector.notifier.last.lock().unwrap();
            assert!(last.is_empty(), "an alert was dispatched: {last:?}");
        }

        /// The tests above call `notify_if_needed` and `maybe_notify`
        /// themselves. This one checks that the collector calls the notifier
        /// after a successful poll.
        #[tokio::test(start_paused = true)]
        async fn collector_poll_over_threshold_shows_one_alert() {
            let (show, mut alerts) = recording_show();
            let reader = FakeReader::new();
            reader.set_reading(
                A,
                CurrentReading {
                    co2: 1500,
                    ..CurrentReading::default()
                },
            );
            let t = TestState::with_collector(
                config_with_alias("Office"),
                CollectorState::with_reader(reader.clone())
                    .with_notifier(Notifier::with_show(show)),
            );
            let collector = Collector::new(Arc::clone(&t.state));

            assert_eq!(collector.start().await, CollectorStartResult::Started);
            // A's first poll is due at once. settle() lets it finish, and the
            // show() job it dispatched too: a running spawn_blocking job stops
            // the paused clock.
            settle().await;

            assert_eq!(reader.reads_of(A).len(), 1);
            assert_eq!(
                alerts.try_recv(),
                Ok((
                    "Aranet Alert: Office".to_string(),
                    "CO\u{2082}: 1500 ppm (threshold: 1000)".to_string()
                )),
                "the collector showed no alert, or the wrong one, for A's first poll"
            );
            assert_eq!(
                alerts.try_recv(),
                Err(TryRecvError::Empty),
                "A's first poll showed more than one alert"
            );

            collector.stop().await;
        }
    }
}

#[cfg(feature = "notifications")]
pub(crate) use notifications::{Notifier, notify_if_needed};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::test_support::{FakeReader, ReadRecord, TestState, settle};

    fn create_test_state() -> TestState {
        TestState::in_memory()
    }

    fn config_with_devices(devices: Vec<DeviceConfig>) -> Config {
        Config {
            devices,
            ..Config::default()
        }
    }

    #[test]
    fn test_collector_new() {
        let t = create_test_state();
        let state = Arc::clone(&t.state);
        let collector = Collector::new(Arc::clone(&state));
        assert!(!collector.is_running());
    }

    #[test]
    fn test_collector_is_running_initially_false() {
        let t = create_test_state();
        let state = Arc::clone(&t.state);
        let collector = Collector::new(state);
        assert!(!collector.is_running());
    }

    #[tokio::test(start_paused = true)]
    async fn test_collector_start_no_devices() {
        let (t, reader, collector) = fake_collector(Vec::new());

        // Start with no devices configured
        let result = collector.start().await;

        assert_eq!(result, CollectorStartResult::NoDevicesConfigured);
        assert!(!collector.is_running());
        assert!(t.state.collector.device_stats.read().await.is_empty());

        // The collector keeps watching the config: a device added later is polled.
        t.state.config.write().await.devices.push(device(A, 60));
        t.state.on_devices_changed().await;
        settle().await;
        assert_eq!(reader.reads_of(A).len(), 1);
        assert!(collector.is_running());

        assert_eq!(collector.stop().await, CollectorStopResult::Stopped);
    }

    #[tokio::test(start_paused = true)]
    async fn test_devices_changed_signals_reload_when_collector_is_idle() {
        let t = TestState::with_reader(Config::default(), FakeReader::new());
        let state = Arc::clone(&t.state);
        let collector = Collector::new(Arc::clone(&state));
        assert_eq!(
            collector.start().await,
            CollectorStartResult::NoDevicesConfigured
        );

        let mut reload_rx = state.collector.subscribe_reload();
        state.on_devices_changed().await;

        tokio::time::timeout(Duration::from_millis(100), reload_rx.changed())
            .await
            .expect("reload notification should be sent")
            .expect("reload channel should stay open");

        collector.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn test_collector_start_with_devices_initializes_stats() {
        let t = TestState::with_reader(
            config_with_devices(vec![DeviceConfig {
                address: "AA:BB:CC:DD:EE:FF".to_string(),
                alias: Some("Test Device".to_string()),
                poll_interval: 60,
            }]),
            FakeReader::new(),
        );
        let state = Arc::clone(&t.state);

        let collector = Collector::new(Arc::clone(&state));
        let result = collector.start().await;
        assert_eq!(result, CollectorStartResult::Started);

        // Let the first (fake) poll run
        settle().await;

        // Check that device stats were initialized
        let stats = state.collector.device_stats.read().await;
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].device_id, "AA:BB:CC:DD:EE:FF");
        assert_eq!(stats[0].alias, Some("Test Device".to_string()));
        assert_eq!(stats[0].poll_interval, 60);
        // The first (fake) poll has run; collector_polls_through_the_device_reader
        // checks the success_count, failure_count and polling flag it leaves.
        drop(stats);

        collector.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn test_collector_stop() {
        let (_t, reader, collector) = fake_collector(vec![device(A, 60)]);
        assert_eq!(collector.start().await, CollectorStartResult::Started);
        reader.wait_for_reads(1).await;
        assert!(collector.is_running());

        assert_eq!(collector.stop().await, CollectorStopResult::Stopped);
        assert!(!collector.is_running());
    }

    #[test]
    fn test_collector_error_display_connect() {
        let core_error = aranet_core::Error::NotConnected;
        let error = CollectorError::Connect(core_error);
        let display = format!("{}", error);
        assert!(display.contains("Failed to connect"));
    }

    #[test]
    fn test_collector_error_display_read() {
        let core_error = aranet_core::Error::NotConnected;
        let error = CollectorError::Read(core_error);
        let display = format!("{}", error);
        assert!(display.contains("Failed to read"));
    }

    #[test]
    fn test_collector_error_display_store() {
        let store_error = aranet_store::Error::DeviceNotFound("test".to_string());
        let error = CollectorError::Store(store_error);
        let display = format!("{}", error);
        assert!(display.contains("Failed to store"));
    }

    #[test]
    fn test_collector_error_debug() {
        let core_error = aranet_core::Error::NotConnected;
        let error = CollectorError::Connect(core_error);
        let debug = format!("{:?}", error);
        assert!(debug.contains("Connect"));
    }

    #[tokio::test]
    async fn test_device_collection_stats_initialization() {
        let stats = DeviceCollectionStats {
            device_id: "test-device".to_string(),
            alias: Some("Test Alias".to_string()),
            poll_interval: 120,
            last_poll_at: None,
            last_error_at: None,
            last_error: None,
            last_poll_duration_ms: None,
            success_count: 0,
            failure_count: 0,
            polling: false,
        };

        assert_eq!(stats.device_id, "test-device");
        assert_eq!(stats.alias, Some("Test Alias".to_string()));
        assert_eq!(stats.poll_interval, 120);
        assert!(stats.last_poll_at.is_none());
        assert_eq!(stats.success_count, 0);
        assert_eq!(stats.failure_count, 0);
        assert!(!stats.polling);
    }

    #[tokio::test]
    async fn test_update_device_stat() {
        let t = create_test_state();
        let state = Arc::clone(&t.state);

        // Initialize stats
        {
            let mut stats = state.collector.device_stats.write().await;
            stats.push(DeviceCollectionStats {
                device_id: "test-device".to_string(),
                alias: None,
                poll_interval: 60,
                last_poll_at: None,
                last_error_at: None,
                last_error: None,
                last_poll_duration_ms: None,
                success_count: 0,
                failure_count: 0,
                polling: false,
            });
        }

        // Update the stat
        update_device_stat(&state, "test-device", |stat| {
            stat.success_count = 5;
            stat.polling = true;
        })
        .await;

        // Verify the update
        let stats = state.collector.device_stats.read().await;
        assert_eq!(stats[0].success_count, 5);
        assert!(stats[0].polling);
    }

    #[tokio::test]
    async fn test_update_device_stat_nonexistent_device() {
        let t = create_test_state();
        let state = Arc::clone(&t.state);

        // Initialize stats with one device
        {
            let mut stats = state.collector.device_stats.write().await;
            stats.push(DeviceCollectionStats {
                device_id: "existing-device".to_string(),
                alias: None,
                poll_interval: 60,
                last_poll_at: None,
                last_error_at: None,
                last_error: None,
                last_poll_duration_ms: None,
                success_count: 0,
                failure_count: 0,
                polling: false,
            });
        }

        // Try to update a nonexistent device - should not panic
        update_device_stat(&state, "nonexistent-device", |stat| {
            stat.success_count = 10;
        })
        .await;

        // Verify existing device wasn't changed
        let stats = state.collector.device_stats.read().await;
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].success_count, 0);
    }

    #[test]
    fn test_collector_error_timeout_display() {
        let err = CollectorError::Timeout(BLE_POLL_TIMEOUT);
        assert_eq!(err.to_string(), "Bluetooth poll timed out after 180s");
    }

    #[test]
    fn test_collector_error_ble_busy_display() {
        let err = CollectorError::BleBusy;
        assert_eq!(err.to_string(), "BLE adapter busy (semaphore closed)");
    }

    #[tokio::test]
    async fn test_ble_semaphore_serializes_access() {
        let t = create_test_state();
        let state = Arc::clone(&t.state);

        // Acquire the single permit
        let permit = state.ble_semaphore.acquire().await.unwrap();

        // A second acquire should not succeed immediately
        let result =
            tokio::time::timeout(Duration::from_millis(50), state.ble_semaphore.acquire()).await;
        assert!(
            result.is_err(),
            "second acquire should timeout while first permit is held"
        );

        // After dropping, the next acquire succeeds
        drop(permit);
        let result =
            tokio::time::timeout(Duration::from_millis(50), state.ble_semaphore.acquire()).await;
        assert!(
            result.is_ok(),
            "acquire should succeed after permit is released"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_collector_multiple_devices() {
        let reader = FakeReader::new();
        let t = TestState::with_reader(
            config_with_devices(vec![
                DeviceConfig {
                    address: "DEVICE-1".to_string(),
                    alias: Some("First".to_string()),
                    poll_interval: 30,
                },
                DeviceConfig {
                    address: "DEVICE-2".to_string(),
                    alias: Some("Second".to_string()),
                    poll_interval: 60,
                },
                DeviceConfig {
                    address: "DEVICE-3".to_string(),
                    alias: None,
                    poll_interval: 120,
                },
            ]),
            reader.clone(),
        );
        let state = Arc::clone(&t.state);

        let collector = Collector::new(Arc::clone(&state));
        let result = collector.start().await;
        assert_eq!(result, CollectorStartResult::Started);

        // Devices start 5 s apart, so after 11 s each has been read once
        tokio::time::sleep(Duration::from_secs(11)).await;
        for address in ["DEVICE-1", "DEVICE-2", "DEVICE-3"] {
            assert_eq!(reader.reads_of(address).len(), 1, "{address}");
        }

        // Check all devices were initialized
        let stats = state.collector.device_stats.read().await;
        assert_eq!(stats.len(), 3);

        // Verify each device
        let device1 = stats.iter().find(|s| s.device_id == "DEVICE-1").unwrap();
        assert_eq!(device1.alias, Some("First".to_string()));
        assert_eq!(device1.poll_interval, 30);

        let device2 = stats.iter().find(|s| s.device_id == "DEVICE-2").unwrap();
        assert_eq!(device2.alias, Some("Second".to_string()));
        assert_eq!(device2.poll_interval, 60);

        let device3 = stats.iter().find(|s| s.device_id == "DEVICE-3").unwrap();
        assert!(device3.alias.is_none());
        assert_eq!(device3.poll_interval, 120);
        drop(stats);

        collector.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn collector_polls_through_the_device_reader() {
        const ADDRESS: &str = "AA:BB:CC:DD:EE:FF";
        let reader = FakeReader::new();
        let t = TestState::with_reader(
            config_with_devices(vec![DeviceConfig {
                address: ADDRESS.to_string(),
                alias: None,
                poll_interval: 60,
            }]),
            reader.clone(),
        );
        let state = Arc::clone(&t.state);

        let collector = Collector::new(Arc::clone(&state));
        assert_eq!(collector.start().await, CollectorStartResult::Started);
        settle().await;

        assert_eq!(reader.reads_of(ADDRESS).len(), 1);
        let stats = state.collector.device_stats.read().await;
        assert_eq!(stats[0].success_count, 1);
        assert_eq!(stats[0].failure_count, 0);
        assert!(!stats[0].polling);
        drop(stats);
        let stored = state
            .with_store_read(|store| store.count_readings(Some(ADDRESS)))
            .await
            .unwrap();
        assert_eq!(stored, 1, "the fake's reading is stored");

        collector.stop().await;
    }

    // ------------------------------------------------------------------
    // Lifecycle and reload tests. They run on the paused clock with the fake
    // reader, whose reads take no time unless held. With 60 s intervals, A is
    // read at 0, 60, 120, … s and B, staggered by 5 s, at 5, 65, 125, … s.
    // ------------------------------------------------------------------

    const A: &str = "Aranet4 AAAAA";
    const B: &str = "Aranet4 BBBBB";
    const C: &str = "Aranet4 CCCCC";

    fn device(address: &str, poll_interval: u64) -> DeviceConfig {
        DeviceConfig {
            address: address.to_string(),
            alias: None,
            poll_interval,
        }
    }

    /// A collector for `devices` that reads through a fake reader.
    fn fake_collector(devices: Vec<DeviceConfig>) -> (TestState, Arc<FakeReader>, Collector) {
        let reader = FakeReader::new();
        let t = TestState::with_reader(config_with_devices(devices), reader.clone());
        let collector = Collector::new(Arc::clone(&t.state));
        (t, reader, collector)
    }

    /// A copy of `address`'s statistics.
    async fn stat(t: &TestState, address: &str) -> DeviceCollectionStats {
        t.state
            .collector
            .device_stats
            .read()
            .await
            .iter()
            .find(|stat| stat.device_id == address)
            .cloned()
            .unwrap_or_else(|| panic!("no stats for {address}"))
    }

    /// The reads of `address` that started after `since`.
    fn reads_after(
        reader: &FakeReader,
        address: &str,
        since: tokio::time::Instant,
    ) -> Vec<ReadRecord> {
        reader
            .reads_of(address)
            .into_iter()
            .filter(|read| read.started > since)
            .collect()
    }

    /// Seconds from `t0` to the start of each read.
    fn start_secs(reads: &[ReadRecord], t0: tokio::time::Instant) -> Vec<f64> {
        reads
            .iter()
            .map(|read| (read.started - t0).as_secs_f64())
            .collect()
    }

    /// Asserts that the reads started `expected` seconds after `t0`, ±1 s each.
    fn assert_read_times(reads: &[ReadRecord], t0: tokio::time::Instant, expected: &[u64]) {
        let actual = start_secs(reads, t0);
        assert!(
            actual.len() == expected.len()
                && actual
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| (actual - *expected as f64).abs() <= 1.0),
            "reads started at {actual:?} s, expected {expected:?} s"
        );
    }

    /// Asserts that no two of `reads` started less than 59 s apart.
    fn assert_one_poller(reads: &[ReadRecord], t0: tokio::time::Instant) {
        assert!(
            reads
                .windows(2)
                .all(|pair| pair[1].started - pair[0].started >= Duration::from_secs(59)),
            "more than one poller: reads started at {:?} s",
            start_secs(reads, t0)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stop_returns_promptly_while_a_poll_is_in_flight() {
        let (t, reader, collector) = fake_collector(vec![device(A, 60)]);
        let mut events = t.state.readings_tx.subscribe();
        reader.hold(A);
        assert_eq!(collector.start().await, CollectorStartResult::Started);
        reader.wait_for_reads(1).await;

        let stopped = tokio::time::timeout(Duration::from_secs(2), collector.stop()).await;
        assert_eq!(
            stopped,
            Ok(CollectorStopResult::Stopped),
            "stop waited for the Bluetooth read"
        );
        assert!(!collector.is_running());

        // The read in flight isn't cancelled: it finishes in the background and
        // stores its reading, which isn't broadcast. Nothing polls again.
        reader.release(A);
        tokio::time::sleep(Duration::from_secs(600)).await;
        let reads = reader.reads_of(A);
        assert_eq!(reads.len(), 1);
        assert!(
            reads[0].finished.is_some() && reads[0].ok,
            "the stopped poll was cancelled mid-read"
        );
        let stored = t
            .state
            .with_store_read(|store| store.count_readings(Some(A)))
            .await
            .unwrap();
        assert_eq!(stored, 1, "the stopped poll did not store its reading");
        assert!(
            matches!(
                events.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ),
            "a stopped poll broadcast its reading"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stop_cancels_a_poll_waiting_for_the_adapter() {
        let (t, reader, collector) = fake_collector(vec![device(A, 60), device(B, 60)]);
        reader.hold(A);
        assert_eq!(collector.start().await, CollectorStartResult::Started);
        // A's read holds the Bluetooth adapter, and from 5 s B's poll waits for it.
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert!(reader.reads_of(B).is_empty());
        assert!(
            stat(&t, B).await.polling,
            "B's poll is not waiting for the adapter"
        );

        let stopped = tokio::time::timeout(Duration::from_secs(2), collector.stop()).await;
        assert_eq!(
            stopped,
            Ok(CollectorStopResult::Stopped),
            "stop waited for the polls"
        );

        // B's poll gives up its place in the queue: when A's read ends, B isn't
        // read, and nothing is stored for it.
        reader.release(A);
        tokio::time::sleep(Duration::from_secs(600)).await;
        assert!(reader.reads_of(B).is_empty(), "B was read after the stop");
        let stored = t
            .state
            .with_store_read(|store| store.count_readings(Some(B)))
            .await
            .unwrap();
        assert_eq!(stored, 0, "a reading of B was stored after the stop");
    }

    #[tokio::test(start_paused = true)]
    async fn start_racing_stop_leaves_exactly_one_poller() {
        let (t, reader, collector) = fake_collector(vec![device(A, 60)]);
        reader.hold(A);
        assert_eq!(collector.start().await, CollectorStartResult::Started);
        reader.wait_for_reads(1).await;

        // Stop from another task, then start again straight away.
        let stopper = Collector::new(Arc::clone(&t.state));
        let stop = tokio::spawn(async move { stopper.stop().await });
        tokio::task::yield_now().await;
        assert_eq!(collector.start().await, CollectorStartResult::Started);
        assert_eq!(stop.await.unwrap(), CollectorStopResult::Stopped);

        let released_at = tokio::time::Instant::now();
        reader.release(A);
        tokio::time::sleep(Duration::from_secs(590)).await;

        assert!(collector.is_running());
        let reads = reads_after(&reader, A, released_at);
        assert!(
            reads.len() >= 9,
            "A was not polled every 60 s after the restart: reads started at {:?} s",
            start_secs(&reads, released_at)
        );
        assert_one_poller(&reads, released_at);

        // The running collector's reload watcher is alive.
        t.state.config.write().await.devices.push(device(B, 60));
        t.state.on_devices_changed().await;
        settle().await;
        assert_eq!(
            reader.reads_of(B).len(),
            1,
            "a device added later is not polled"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn reload_racing_stop_does_not_resurrect_collector() {
        let (t, reader, collector) = fake_collector(vec![device(A, 60)]);
        reader.hold(A);
        assert_eq!(collector.start().await, CollectorStartResult::Started);
        reader.wait_for_reads(1).await;

        t.state.on_devices_changed().await;
        tokio::task::yield_now().await;
        assert_eq!(collector.stop().await, CollectorStopResult::Stopped);
        reader.release(A);
        tokio::time::sleep(Duration::from_secs(600)).await;

        assert_eq!(reader.reads_of(A).len(), 1, "A was polled after the stop");
        assert!(
            !collector.is_running(),
            "a reload restarted the stopped collector"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn start_during_reload_does_not_duplicate_pollers() {
        let (t, reader, collector) = fake_collector(vec![device(A, 60)]);
        reader.hold(A);
        assert_eq!(collector.start().await, CollectorStartResult::Started);
        reader.wait_for_reads(1).await;

        t.state.on_devices_changed().await;
        settle().await;
        assert_eq!(
            collector.start().await,
            CollectorStartResult::AlreadyRunning
        );

        let released_at = tokio::time::Instant::now();
        reader.release(A);
        tokio::time::sleep(Duration::from_secs(590)).await;

        let reads = reads_after(&reader, A, released_at);
        assert!(
            reads.len() >= 9,
            "reads started at {:?} s",
            start_secs(&reads, released_at)
        );
        assert_one_poller(&reads, released_at);
    }

    #[tokio::test(start_paused = true)]
    async fn start_after_earlier_reload_polls_once_per_interval() {
        // A replayed reload would race each device task's first poll, so repeat it.
        for round in 0..20 {
            let (t, reader, collector) = fake_collector(vec![device(A, 60), device(B, 60)]);
            // A device edit before the collector starts.
            t.state.collector.signal_reload();
            assert_eq!(collector.start().await, CollectorStartResult::Started);
            tokio::time::sleep(Duration::from_secs(59)).await;

            assert_eq!(reader.reads_of(A).len(), 1, "round {round}: reads of A");
            assert_eq!(reader.reads_of(B).len(), 1, "round {round}: reads of B");
            assert_eq!(collector.stop().await, CollectorStopResult::Stopped);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn stagger_wait_is_cancellable() {
        let (_t, reader, collector) = fake_collector(vec![device(A, 60), device(B, 60)]);
        assert_eq!(collector.start().await, CollectorStartResult::Started);

        // B is still waiting out its 5 s start stagger.
        let stopped = tokio::time::timeout(Duration::from_secs(1), collector.stop()).await;
        assert_eq!(
            stopped,
            Ok(CollectorStopResult::Stopped),
            "stop waited out B's start stagger"
        );
        tokio::time::sleep(Duration::from_secs(600)).await;
        assert!(reader.reads_of(B).is_empty(), "B was polled after the stop");
    }

    #[tokio::test(start_paused = true)]
    async fn hung_poll_times_out_and_frees_the_adapter() {
        let (t, reader, collector) = fake_collector(vec![device(A, 60), device(B, 60)]);
        reader.hold(A); // A never answers
        assert_eq!(collector.start().await, CollectorStartResult::Started);
        tokio::time::sleep(BLE_POLL_TIMEOUT + Duration::from_secs(60)).await;

        assert!(
            !reader.reads_of(B).is_empty(),
            "B never got the Bluetooth adapter"
        );
        let a = stat(&t, A).await;
        assert_eq!(a.failure_count, 1);
        let error = a.last_error.unwrap_or_default();
        assert!(
            error.contains("timed out"),
            "unexpected last_error: {error}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn alias_change_keeps_tasks_and_stats() {
        let (t, reader, collector) = fake_collector(vec![device(A, 60), device(B, 60)]);
        let t0 = tokio::time::Instant::now();
        assert_eq!(collector.start().await, CollectorStartResult::Started);
        tokio::time::sleep(Duration::from_secs(130)).await;
        assert_eq!(reader.reads_of(A).len(), 3);
        assert_eq!(reader.reads_of(B).len(), 3);
        assert_eq!(stat(&t, A).await.success_count, 3);
        assert_eq!(stat(&t, B).await.success_count, 3);

        t.state.config.write().await.devices[0].alias = Some("Kitchen".to_string());
        t.state.on_devices_changed().await;
        settle().await;

        assert_eq!(reader.reads_of(A).len(), 3, "the alias edit re-polled A");
        assert_eq!(reader.reads_of(B).len(), 3, "the alias edit re-polled B");
        let a = stat(&t, A).await;
        assert_eq!(a.success_count, 3, "the alias edit reset A's counters");
        assert_eq!(stat(&t, B).await.success_count, 3);
        assert_eq!(a.alias.as_deref(), Some("Kitchen"));

        tokio::time::sleep(Duration::from_secs(52)).await;
        assert_read_times(&reader.reads_of(A), t0, &[0, 60, 120, 180]);
        assert_eq!(reader.reads_of(B).len(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn adding_device_keeps_existing_schedule() {
        let (t, reader, collector) = fake_collector(vec![device(A, 60), device(B, 60)]);
        let t0 = tokio::time::Instant::now();
        assert_eq!(collector.start().await, CollectorStartResult::Started);
        tokio::time::sleep(Duration::from_secs(130)).await;

        t.state.config.write().await.devices.push(device(C, 60));
        t.state.on_devices_changed().await;
        settle().await;

        assert_eq!(reader.reads_of(A).len(), 3, "adding C re-polled A");
        assert_eq!(reader.reads_of(B).len(), 3, "adding C re-polled B");
        assert_read_times(&reader.reads_of(C), t0, &[130]);
        assert_eq!(stat(&t, A).await.success_count, 3);
        assert_eq!(stat(&t, B).await.success_count, 3);
        assert_eq!(stat(&t, C).await.success_count, 1);

        tokio::time::sleep(Duration::from_secs(52)).await;
        assert_read_times(&reader.reads_of(A), t0, &[0, 60, 120, 180]);
        assert_read_times(&reader.reads_of(B), t0, &[5, 65, 125]);
        assert_read_times(&reader.reads_of(C), t0, &[130]);
    }

    #[tokio::test(start_paused = true)]
    async fn removing_device_stops_only_that_device() {
        let (t, reader, collector) = fake_collector(vec![device(A, 60), device(B, 60)]);
        let t0 = tokio::time::Instant::now();
        assert_eq!(collector.start().await, CollectorStartResult::Started);
        tokio::time::sleep(Duration::from_secs(130)).await;

        t.state
            .config
            .write()
            .await
            .devices
            .retain(|d| d.address != B);
        t.state.on_devices_changed().await;
        settle().await;

        assert_eq!(reader.reads_of(A).len(), 3, "removing B re-polled A");
        let stats = t.state.collector.device_stats.read().await.clone();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].device_id, A);
        assert_eq!(stats[0].success_count, 3, "removing B reset A's counters");

        tokio::time::sleep(Duration::from_secs(120)).await;
        assert_read_times(&reader.reads_of(A), t0, &[0, 60, 120, 180, 240]);
        assert_eq!(
            reader.reads_of(B).len(),
            3,
            "B was polled after its removal"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn poll_interval_change_restarts_only_that_device() {
        let (t, reader, collector) = fake_collector(vec![device(A, 60), device(B, 60)]);
        let t0 = tokio::time::Instant::now();
        assert_eq!(collector.start().await, CollectorStartResult::Started);
        tokio::time::sleep(Duration::from_secs(130)).await;

        t.state.config.write().await.devices[0].poll_interval = 120;
        t.state.on_devices_changed().await;
        settle().await;

        // A restarts with its new interval and is polled at once; B is untouched.
        assert_read_times(&reader.reads_of(A), t0, &[0, 60, 120, 130]);
        assert_eq!(reader.reads_of(B).len(), 3, "B was restarted");
        assert_eq!(
            (
                stat(&t, A).await.success_count,
                stat(&t, B).await.success_count
            ),
            (4, 3),
            "the interval change reset the counters"
        );
        assert_eq!(stat(&t, A).await.poll_interval, 120);

        tokio::time::sleep(Duration::from_secs(250)).await;
        assert_read_times(&reader.reads_of(A), t0, &[0, 60, 120, 130, 250, 370]);
        assert_read_times(&reader.reads_of(B), t0, &[5, 65, 125, 185, 245, 305, 365]);
        assert_eq!(stat(&t, A).await.success_count, 6);
        assert_eq!(stat(&t, B).await.success_count, 7);
    }

    #[tokio::test(start_paused = true)]
    async fn device_edit_keeps_collector_uptime() {
        let (t, reader, collector) = fake_collector(vec![device(A, 60)]);
        // Hold A's poll, so a reload that restarts A can't finish before the check.
        reader.hold(A);
        assert_eq!(collector.start().await, CollectorStartResult::Started);
        reader.wait_for_reads(1).await;
        let before = t.state.collector.started_at();
        assert!(before.is_some());

        t.state.config.write().await.devices[0].alias = Some("Kitchen".to_string());
        t.state.on_devices_changed().await;
        settle().await;

        assert_eq!(
            t.state.collector.started_at(),
            before,
            "the alias edit reset the collector uptime"
        );
        assert!(collector.is_running());
    }

    #[tokio::test(start_paused = true)]
    async fn stop_does_not_wait_for_a_reload_queued_behind_it() {
        let (t, _reader, collector) = fake_collector(vec![device(A, 60)]);
        assert_eq!(collector.start().await, CollectorStartResult::Started);

        // Hold the lifecycle lock, so a stop and then the reload watcher queue for it.
        let lifecycle = t.state.collector.lifecycle.lock().await;
        let stopper = Collector::new(Arc::clone(&t.state));
        let stop = tokio::spawn(async move { stopper.stop().await });
        settle().await;
        t.state.on_devices_changed().await;
        settle().await;
        drop(lifecycle);

        // The stop gets the lock first and cancels the watcher, which has to
        // leave the queue instead of waiting for the lock the stop holds.
        let stopped = tokio::time::timeout(Duration::from_secs(1), stop)
            .await
            .expect("stop waited for the reload watcher it had cancelled");
        assert_eq!(stopped.unwrap(), CollectorStopResult::Stopped);
        assert!(!collector.is_running());
    }
}

/// SV-9: devices whose polls keep failing back off, and the poll ticker never
/// bursts. Every test runs on tokio's paused clock with a `FakeReader`, so the
/// schedules are exact; `sleep` lets virtual time pass timer by timer.
#[cfg(test)]
mod backoff_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::time::{Instant, sleep, sleep_until};

    use super::{Collector, CollectorStartResult, backoff_delay, failure_log_level};
    use crate::config::{Config, DeviceConfig};
    use crate::test_support::{FakeReader, TestState, settle};

    /// The device that fails in every test that sets it failing.
    const A: &str = "Aranet4 AAAAA";
    /// A device that always answers at once.
    const B: &str = "Aranet4 BBBBB";

    fn secs(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    /// A config with these `(address, poll_interval)` devices, in this order. The
    /// collector staggers their first polls 5 s apart.
    fn config_with(devices: &[(&str, u64)]) -> Config {
        Config {
            devices: devices
                .iter()
                .map(|&(address, poll_interval)| DeviceConfig {
                    address: address.to_string(),
                    alias: None,
                    poll_interval,
                })
                .collect(),
            ..Config::default()
        }
    }

    /// Start a collector that reads through `reader`. Returns the fixture (keep it
    /// alive for the whole test), the collector and the instant it was started.
    async fn start(config: Config, reader: &Arc<FakeReader>) -> (TestState, Collector, Instant) {
        let t = TestState::with_reader(config, reader.clone());
        let collector = Collector::new(Arc::clone(&t.state));
        let t0 = Instant::now();
        assert_eq!(collector.start().await, CollectorStartResult::Started);
        (t, collector, t0)
    }

    /// When each read of `address` started, in seconds after `t0`.
    fn start_secs(reader: &FakeReader, address: &str, t0: Instant) -> Vec<f64> {
        reader
            .reads_of(address)
            .iter()
            .map(|read| (read.started - t0).as_secs_f64())
            .collect()
    }

    /// Whether `actual` has as many entries as `expected` and each is within 1 s.
    fn near(actual: &[f64], expected: &[u32]) -> bool {
        actual.len() == expected.len()
            && actual
                .iter()
                .zip(expected)
                .all(|(&actual, &expected)| (actual - f64::from(expected)).abs() <= 1.0)
    }

    #[track_caller]
    fn assert_starts(reader: &FakeReader, address: &str, t0: Instant, expected: &[u32]) {
        let actual = start_secs(reader, address, t0);
        assert!(
            near(&actual, expected),
            "reads of {address} started at {actual:?} s, expected {expected:?} s (±1 s)"
        );
    }

    #[test]
    fn backoff_delay_table() {
        assert_eq!(backoff_delay(secs(60), 1), secs(60));
        assert_eq!(backoff_delay(secs(60), 2), secs(120));
        assert_eq!(backoff_delay(secs(60), 3), secs(240));
        assert_eq!(backoff_delay(secs(60), 4), secs(300));
        assert_eq!(backoff_delay(secs(60), u32::MAX), secs(300));
        assert_eq!(backoff_delay(secs(10), 1), secs(10));
        assert_eq!(backoff_delay(secs(10), 6), secs(300));
        // An interval above the cap is never stretched.
        assert_eq!(backoff_delay(secs(3600), 1), secs(3600));
        assert_eq!(backoff_delay(secs(3600), 5), secs(3600));
    }

    #[test]
    fn failure_log_level_table() {
        use tracing::Level;

        for failures in 1..=3 {
            assert_eq!(
                failure_log_level(failures),
                Some(Level::WARN),
                "failure {failures}"
            );
        }
        assert_eq!(failure_log_level(4), Some(Level::ERROR));
        for failures in 5..=11 {
            assert_eq!(failure_log_level(failures), None, "failure {failures}");
        }
        for failures in [12, 24, 36] {
            assert_eq!(
                failure_log_level(failures),
                Some(Level::ERROR),
                "failure {failures}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn failing_device_is_backed_off() {
        let reader = FakeReader::new();
        reader.set_failing(A, true);
        let (_t, _collector, t0) = start(config_with(&[(A, 60)]), &reader).await;

        sleep(secs(1800)).await;

        // Each read fails at once. The next one waits 60, 120, 240 s, then 300 s
        // (the cap) every time.
        assert_starts(&reader, A, t0, &[0, 60, 180, 420, 720, 1020, 1320, 1620]);
    }

    #[tokio::test(start_paused = true)]
    async fn slow_failing_device_does_not_burst() {
        let reader = FakeReader::new();
        reader.set_failing(A, true);
        // Every attempt overruns the 10 s interval tenfold.
        reader.set_latency(A, secs(100));
        let (_t, _collector, t0) = start(config_with(&[(A, 10)]), &reader).await;

        sleep(secs(3600)).await;

        // Each gap is the 100 s read plus a delay counted from its end: 10, 20,
        // 40, 80, 160 s, then 300 s.
        let starts = start_secs(&reader, A, t0);
        let gaps: Vec<f64> = starts.windows(2).map(|pair| pair[1] - pair[0]).collect();
        let expected = [110, 120, 140, 180, 260, 400, 400, 400, 400, 400, 400];
        assert!(
            near(&gaps, &expected),
            "reads of {A} started {gaps:?} s apart, expected {expected:?} s (±1 s)"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn healthy_device_keeps_cadence_next_to_unreachable_one() {
        let reader = FakeReader::new();
        // A is out of range: each attempt holds the Bluetooth permit for 60 s of
        // scanning and then fails. B answers at once.
        reader.set_failing(A, true);
        reader.set_latency(A, secs(60));
        let (_t, _collector, t0) = start(config_with(&[(A, 60), (B, 60)]), &reader).await;

        sleep(secs(3600)).await;

        // B's ticks fall at 5 + 60k s, 50 of them in [600 s, 3600 s). A read that
        // starts more than 1 s after its tick waited for A's attempt to end.
        let (on_time, late): (Vec<f64>, Vec<f64>) = start_secs(&reader, B, t0)
            .into_iter()
            .filter(|start| (600.0..3600.0).contains(start))
            .partition(|start| {
                let offset = (start - 5.0).rem_euclid(60.0);
                offset <= 1.0 || offset >= 59.0
            });
        assert!(
            on_time.len() >= 40,
            "only {} of B's 50 ticks in [600, 3600) s were read on time; late reads at {late:?} s",
            on_time.len()
        );
        // A tick that falls inside one of A's attempts is read late, not lost, so
        // B still reads once per tick.
        let reads = on_time.len() + late.len();
        assert_eq!(
            reads,
            50,
            "B lost {} of its 50 readings in [600, 3600) s; on-time reads at {on_time:?} s, late reads at {late:?} s",
            50_usize.saturating_sub(reads)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn success_resets_backoff() {
        let reader = FakeReader::new();
        reader.set_failing(A, true);
        let (_t, _collector, t0) = start(config_with(&[(A, 60)]), &reader).await;

        // The third failure, at 180 s, puts the next attempt 240 s later.
        reader.wait_for_reads(3).await;
        settle().await;
        reader.set_failing(A, false);
        sleep_until(t0 + secs(510)).await;

        // The success at 420 s restores the 60 s cadence.
        assert_starts(&reader, A, t0, &[0, 60, 180, 420, 480]);
        let ok: Vec<bool> = reader.reads_of(A).iter().map(|read| read.ok).collect();
        assert_eq!(ok, [false, false, false, true, true]);
    }

    #[tokio::test(start_paused = true)]
    async fn slow_first_poll_does_not_burst() {
        let reader = FakeReader::new();
        // The first read overruns the ticks at 60 and 120 s; later reads are instant.
        reader.set_latency(A, secs(130));
        let (_t, _collector, t0) = start(config_with(&[(A, 60)]), &reader).await;
        reader.wait_for_reads(1).await;
        reader.set_latency(A, Duration::ZERO);

        sleep_until(t0 + secs(400)).await;

        let starts = start_secs(&reader, A, t0);
        assert!(
            starts.windows(2).all(|pair| pair[1] - pair[0] >= 59.0),
            "reads of {A} started at {starts:?} s; two were less than 59 s apart"
        );
        // One read when the slow one ends, then every 60 s from there.
        assert_starts(&reader, A, t0, &[0, 130, 190, 250, 310, 370]);
    }
}
