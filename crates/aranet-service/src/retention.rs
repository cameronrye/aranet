//! Optional data retention (`[storage.retention]`).
//!
//! With a limit set, [`spawn`] starts a task that deletes readings and cached
//! history older than the limit at startup and then every hour. The limits
//! apply to every row, whoever stored it. Retention is off by default: unless
//! the service has its own database, it shares it with the readings that the
//! `aranet` CLI, TUI and GUI store and with the CLI's history cache, and
//! aranet keeps data until the user deletes it.
//!
//! Rows are deleted [`PRUNE_BATCH_ROWS`] at a time, each batch its own store
//! write on the blocking pool. The writer lock is first-come first-served, so
//! a collector insert that queues during a batch runs before the next batch,
//! and other processes can take the database's write lock between batches.
//! (Nothing puts another process next in line: that it gets the lock in time
//! holds by timing, not by construction.) After shutdown is signalled, no new
//! batch starts, so a long first pass doesn't hold up a restart. The service
//! never runs `VACUUM`, so the file stops growing but doesn't shrink.

use std::sync::Arc;
use std::time::Duration;

use aranet_store::{PRUNE_BATCH_ROWS, Store};
use time::OffsetDateTime;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tracing::{info, warn};

use crate::config::RetentionConfig;
use crate::state::AppState;

/// Time between retention passes after the one at startup.
pub(crate) const RETENTION_INTERVAL: Duration = Duration::from_secs(3600);

/// Rows deleted by one retention pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PruneReport {
    /// Readings deleted.
    pub(crate) readings: u64,
    /// Cached history records deleted.
    pub(crate) history: u64,
}

/// One table's batch delete: `Store::prune_readings_batch` or
/// `Store::prune_history_batch`.
type PruneBatch = fn(&Store, OffsetDateTime, u32) -> aranet_store::Result<u64>;

/// Delete the rows older than the retention limits, counted back from `now`.
///
/// A table without a limit is left alone. Each table is pruned in batches
/// until a batch deletes fewer than [`PRUNE_BATCH_ROWS`] rows. Once
/// [`AppState::signal_shutdown`] has been called, no new batch starts: the
/// pass returns what it has deleted so far, and the next start deletes the
/// rest.
pub(crate) async fn prune_once(
    state: &AppState,
    config: &RetentionConfig,
    now: OffsetDateTime,
) -> aranet_store::Result<PruneReport> {
    let mut report = PruneReport::default();
    if let Some(days) = config.readings_days {
        let cutoff = now - time::Duration::days(i64::from(days));
        report.readings = prune_older_than(state, Store::prune_readings_batch, cutoff).await?;
    }
    if let Some(days) = config.history_days {
        let cutoff = now - time::Duration::days(i64::from(days));
        report.history = prune_older_than(state, Store::prune_history_batch, cutoff).await?;
    }
    Ok(report)
}

/// Run `prune_batch` until it deletes fewer than [`PRUNE_BATCH_ROWS`] rows or
/// shutdown is signalled, and return the total deleted.
async fn prune_older_than(
    state: &AppState,
    prune_batch: PruneBatch,
    cutoff: OffsetDateTime,
) -> aranet_store::Result<u64> {
    let shutdown = state.subscribe_shutdown();
    let mut total = 0;
    // The flag is read before every batch, so shutdown waits for at most the
    // batch in flight, not for the whole backlog.
    while !*shutdown.borrow() {
        let deleted = state
            .with_store_write(move |store| prune_batch(store, cutoff, PRUNE_BATCH_ROWS))
            .await?;
        total += deleted;
        if deleted < u64::from(PRUNE_BATCH_ROWS) {
            break;
        }
        // Let other tasks run between batches; a collector insert that queued
        // on the writer lock during this batch goes first.
        tokio::task::yield_now().await;
    }
    Ok(total)
}

/// Start the retention task, or return `None` when retention is off.
///
/// The task prunes at once, then every [`RETENTION_INTERVAL`], and returns
/// after [`AppState::signal_shutdown`], within one batch when a pass is
/// running. It reads `config` once, so changing the limits needs a restart.
pub(crate) fn spawn(state: Arc<AppState>, config: RetentionConfig) -> Option<JoinHandle<()>> {
    if config.is_disabled() {
        return None;
    }
    info!(
        "Retention: readings are deleted {}, history {}; pruning at startup and every hour",
        describe(config.readings_days),
        describe(config.history_days),
    );

    let mut shutdown = state.subscribe_shutdown();
    Some(tokio::spawn(async move {
        // The first tick completes at once, so the first pass runs at startup.
        let mut interval = tokio::time::interval(RETENTION_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow_and_update() {
                        break;
                    }
                }
                _ = interval.tick() => {
                    match prune_once(&state, &config, OffsetDateTime::now_utc()).await {
                        Ok(report) if report != PruneReport::default() => info!(
                            "Retention: deleted {} readings and {} history records older than the retention limits",
                            report.readings, report.history
                        ),
                        Ok(_) => {}
                        Err(e) => warn!("Retention: pruning failed, retrying in an hour: {e}"),
                    }
                }
            }
        }
    }))
}

/// When a table's rows are deleted: "after N days" ("after 1 day"), or "never"
/// without a limit.
fn describe(days: Option<u32>) -> String {
    match days {
        None => "never".to_string(),
        Some(1) => "after 1 day".to_string(),
        Some(days) => format!("after {days} days"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use aranet_store::{PRUNE_BATCH_ROWS, ReadingQuery};
    use aranet_types::{CurrentReading, HistoryRecord};
    use time::OffsetDateTime;

    use super::*;
    use crate::config::RetentionConfig;
    use crate::state::AppState;
    use crate::test_support::{TestState, settle};

    /// A fixed "now" (2027-01-15T08:00:00Z) for the tests that pass their own clock.
    fn fixed_now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
    }

    fn keep_days(readings_days: Option<u32>, history_days: Option<u32>) -> RetentionConfig {
        RetentionConfig {
            readings_days,
            history_days,
        }
    }

    async fn insert_reading_at(state: &AppState, captured_at: OffsetDateTime) {
        state
            .with_store_write(move |store| {
                let reading = CurrentReading {
                    co2: 800,
                    captured_at: Some(captured_at),
                    ..Default::default()
                };
                store.insert_reading("dev", &reading).map(|_| ())
            })
            .await
            .unwrap();
    }

    async fn insert_history_at(state: &AppState, timestamp: OffsetDateTime) {
        let record = HistoryRecord {
            timestamp,
            co2: 800,
            temperature: 21.0,
            pressure: 1013.0,
            humidity: 40,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
        };
        let inserted = state
            .with_store_write(move |store| store.insert_history("dev", &[record]))
            .await
            .unwrap();
        assert_eq!(inserted, 1);
    }

    /// (readings, history records) in the store.
    async fn counts(state: &AppState) -> (u64, u64) {
        state
            .with_store_read(|store| Ok((store.count_readings(None)?, store.count_history(None)?)))
            .await
            .unwrap()
    }

    /// Store `count` readings for `device`, all captured at `captured_at`, in
    /// one store write.
    async fn insert_readings_at(
        state: &AppState,
        device: &'static str,
        captured_at: OffsetDateTime,
        count: u64,
    ) {
        state
            .with_store_write(move |store| {
                let reading = CurrentReading {
                    captured_at: Some(captured_at),
                    ..Default::default()
                };
                for _ in 0..count {
                    store.insert_reading(device, &reading)?;
                }
                Ok(())
            })
            .await
            .unwrap();
    }

    /// Readings stored for `device`.
    async fn readings_of(state: &AppState, device: &'static str) -> u64 {
        state
            .with_store_read(move |store| store.count_readings(Some(device)))
            .await
            .unwrap()
    }

    /// Most probes a test sends while it waits for a retention pass to delete
    /// a batch. Correct code needs two.
    const MAX_PROBES: u64 = 100;

    /// Fail, instead of probing forever, when a retention pass deletes
    /// nothing. Call it after each probe that found every old reading still
    /// there, with `pass_ended` read before that probe started: a pass that
    /// had ended by then deleted nothing. `probes` counts those probes so far.
    async fn fail_if_pass_deletes_nothing(
        pass: &mut JoinHandle<aranet_store::Result<PruneReport>>,
        pass_ended: bool,
        probes: u64,
    ) {
        if pass_ended {
            let outcome = pass.await;
            panic!("the retention pass ended before deleting any old reading: {outcome:?}");
        }
        assert!(
            probes < MAX_PROBES,
            "the retention pass deleted no old reading in {MAX_PROBES} probes"
        );
    }

    #[tokio::test]
    async fn prune_once_deletes_only_rows_older_than_cutoff() {
        let t = TestState::in_memory();
        let now = fixed_now();
        insert_reading_at(&t.state, now - time::Duration::days(8)).await;
        insert_reading_at(&t.state, now - time::Duration::days(1)).await;

        let report = prune_once(&t.state, &keep_days(Some(7), None), now)
            .await
            .unwrap();

        assert_eq!(
            report,
            PruneReport {
                readings: 1,
                history: 0
            }
        );
        let left = t
            .state
            .with_store_read(|store| store.query_readings(&ReadingQuery::new()))
            .await
            .unwrap();
        let left: Vec<_> = left.iter().map(|r| r.captured_at).collect();
        assert_eq!(left, [now - time::Duration::days(1)]);
    }

    #[tokio::test]
    async fn prune_once_leaves_history_unless_history_days_set() {
        let t = TestState::in_memory();
        let now = fixed_now();
        for days in [8, 1] {
            insert_reading_at(&t.state, now - time::Duration::days(days)).await;
            insert_history_at(&t.state, now - time::Duration::days(days)).await;
        }

        let report = prune_once(&t.state, &keep_days(Some(7), None), now)
            .await
            .unwrap();
        assert_eq!(
            report,
            PruneReport {
                readings: 1,
                history: 0
            }
        );
        assert_eq!(counts(&t.state).await, (1, 2));

        let report = prune_once(&t.state, &keep_days(Some(7), Some(7)), now)
            .await
            .unwrap();
        assert_eq!(
            report,
            PruneReport {
                readings: 0,
                history: 1
            }
        );
        assert_eq!(counts(&t.state).await, (1, 1));
    }

    #[tokio::test]
    async fn prune_once_deletes_more_than_one_batch() {
        let t = TestState::in_memory();
        let now = fixed_now();
        let old = now - time::Duration::days(30);
        t.state
            .with_store_write(move |store| {
                for _ in 0..=PRUNE_BATCH_ROWS {
                    let reading = CurrentReading {
                        captured_at: Some(old),
                        ..Default::default()
                    };
                    store.insert_reading("dev", &reading)?;
                }
                Ok(())
            })
            .await
            .unwrap();
        insert_reading_at(&t.state, now).await;

        let report = prune_once(&t.state, &keep_days(Some(7), None), now)
            .await
            .unwrap();

        assert_eq!(report.readings, u64::from(PRUNE_BATCH_ROWS) + 1);
        assert_eq!(counts(&t.state).await, (1, 0));
    }

    #[tokio::test]
    async fn prune_once_stops_after_shutdown() {
        let t = TestState::in_memory();
        let now = fixed_now();
        insert_reading_at(&t.state, now - time::Duration::days(30)).await;
        insert_history_at(&t.state, now - time::Duration::days(30)).await;

        // The flag is read before every batch: a pass that is running when
        // shutdown arrives stops after the batch in flight, and one that
        // starts after it deletes nothing.
        t.state.signal_shutdown();
        let report = prune_once(&t.state, &keep_days(Some(7), Some(7)), now)
            .await
            .unwrap();

        assert_eq!(report, PruneReport::default());
        // Nothing was deleted: the next start deletes both rows.
        assert_eq!(counts(&t.state).await, (1, 1));
    }

    #[tokio::test(start_paused = true)]
    async fn prune_once_stops_within_a_batch_of_shutdown_mid_pass() {
        let t = TestState::in_memory();
        let now = fixed_now();
        let batch = u64::from(PRUNE_BATCH_ROWS);
        // Three full batches and one row.
        let old_rows = 3 * batch + 1;
        insert_readings_at(&t.state, "old", now - time::Duration::days(30), old_rows).await;

        let state = Arc::clone(&t.state);
        let mut pass =
            tokio::spawn(async move { prune_once(&state, &keep_days(Some(7), None), now).await });

        // Each probe is a store write, so it queues first-come first-served
        // with the pass's batches and runs between two of them. The first
        // probe that finds a batch gone signals shutdown while it still holds
        // the writer, so no batch runs between its count and the signal.
        let mut probes = 0;
        let left_at_shutdown = loop {
            let pass_ended = pass.is_finished();
            let state = Arc::clone(&t.state);
            let left = t
                .state
                .with_store_write(move |store| {
                    let left = store.count_readings(Some("old"))?;
                    if left < old_rows {
                        state.signal_shutdown();
                    }
                    Ok(left)
                })
                .await
                .unwrap();
            if left < old_rows {
                break left;
            }
            probes += 1;
            fail_if_pass_deletes_nothing(&mut pass, pass_ended, probes).await;
        };

        let report = tokio::time::timeout(Duration::from_secs(10), pass)
            .await
            .expect("the pass stops after shutdown")
            .expect("the pass does not panic")
            .unwrap();
        // A batch that read the flag before the signal still runs, but no
        // later one starts.
        let deleted_after_shutdown = report.readings - (old_rows - left_at_shutdown);
        assert!(
            deleted_after_shutdown <= batch,
            "{deleted_after_shutdown} readings deleted after shutdown, more than one batch"
        );
        // The rest is left for the next start.
        let left = readings_of(&t.state, "old").await;
        assert_eq!(left, old_rows - report.readings);
        assert!(
            left > 0,
            "the pass deleted every old reading after shutdown"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn prune_once_lets_queued_writes_run_between_batches() {
        let t = TestState::in_memory();
        let now = fixed_now();
        // Three full batches and one row.
        let old_rows = 3 * u64::from(PRUNE_BATCH_ROWS) + 1;
        insert_readings_at(&t.state, "old", now - time::Duration::days(30), old_rows).await;

        let state = Arc::clone(&t.state);
        let mut pass =
            tokio::spawn(async move { prune_once(&state, &keep_days(Some(7), None), now).await });

        // Collector inserts, one at a time, until one runs after a batch. It
        // queued while that batch held the writer, which is first-come
        // first-served, so it runs before the next batch.
        let mut inserted = 0;
        let left_at_insert = loop {
            let pass_ended = pass.is_finished();
            let left = t
                .state
                .with_store_write(move |store| {
                    let reading = CurrentReading {
                        captured_at: Some(now),
                        ..Default::default()
                    };
                    store.insert_reading("dev", &reading)?;
                    store.count_readings(Some("old"))
                })
                .await
                .unwrap();
            inserted += 1;
            if left < old_rows {
                break left;
            }
            fail_if_pass_deletes_nothing(&mut pass, pass_ended, inserted).await;
        };
        assert!(
            left_at_insert > 0,
            "the insert waited until the whole pass had finished"
        );

        let report = tokio::time::timeout(Duration::from_secs(10), pass)
            .await
            .expect("the pass finishes")
            .expect("the pass does not panic")
            .unwrap();
        assert_eq!(report.readings, old_rows);
        assert_eq!(readings_of(&t.state, "old").await, 0);
        assert_eq!(readings_of(&t.state, "dev").await, inserted);
    }

    #[tokio::test(start_paused = true)]
    async fn retention_runs_at_startup_and_every_hour() {
        let t = TestState::in_memory();
        let state = Arc::clone(&t.state);
        // `spawn` prunes against the wall clock, which the paused tokio clock
        // doesn't move.
        let two_days_ago = || OffsetDateTime::now_utc() - time::Duration::days(2);

        insert_reading_at(&state, two_days_ago()).await;
        let handle = spawn(Arc::clone(&state), keep_days(Some(1), None)).expect("retention is on");
        settle().await;
        assert_eq!(counts(&state).await.0, 0, "the first pass runs at startup");

        insert_reading_at(&state, two_days_ago()).await;
        tokio::time::sleep(Duration::from_secs(1800)).await;
        assert_eq!(counts(&state).await.0, 1, "no pass before the hour is up");

        tokio::time::sleep(Duration::from_secs(1801)).await;
        settle().await;
        assert_eq!(
            counts(&state).await.0,
            0,
            "the next pass runs an hour later"
        );

        state.signal_shutdown();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("the retention task stops on shutdown")
            .expect("the retention task does not panic");
    }

    #[tokio::test(start_paused = true)]
    async fn retention_stops_on_shutdown() {
        let t = TestState::in_memory();
        let handle =
            spawn(Arc::clone(&t.state), keep_days(Some(1), Some(1))).expect("retention is on");
        settle().await;

        t.state.signal_shutdown();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("the retention task stops on shutdown")
            .expect("the retention task does not panic");
    }

    #[tokio::test]
    async fn retention_not_spawned_when_disabled() {
        let t = TestState::in_memory();
        assert!(spawn(Arc::clone(&t.state), RetentionConfig::default()).is_none());
    }

    #[test]
    fn describe_says_day_for_a_one_day_limit() {
        assert_eq!(describe(None), "never");
        assert_eq!(describe(Some(1)), "after 1 day");
        assert_eq!(describe(Some(30)), "after 30 days");
    }
}
