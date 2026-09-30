//! Helpers shared by aranet-core's unit tests.

use std::time::Duration;

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
