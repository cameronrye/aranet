//! aranet-core's own tokio runtime, for Bluetooth work that must outlive its caller.
//!
//! The runtime has one worker thread, named `aranet-ble`. It is created on first
//! use and kept for the life of the process.
//!
//! btleplug runs an adapter's event loop, and on Linux the D-Bus connection's I/O
//! task, on the runtime that creates them. Created on a caller's runtime, they
//! stop, with no error, when that runtime shuts down, which happens after every
//! `#[tokio::test]` and in any program that builds a runtime per call. Created
//! here, they last as long as the process. Scan windows, connection cleanup and,
//! on Linux, pairing sessions run here too, so they finish when the caller's
//! future is dropped or its runtime shuts down.
//!
//! Never run blocking code on this runtime: its one thread drives every adapter
//! in the process. The one exception is the system-bus connect each time the
//! Linux Bluetooth manager is created (at first use, and again after a reset).

use crate::error::Result;

/// The runtime behind [`handle`]. Once created it is never dropped.
static PROCESS_RUNTIME: std::sync::Mutex<Option<tokio::runtime::Runtime>> =
    std::sync::Mutex::new(None);

/// A handle to aranet-core's runtime, which the first call creates.
///
/// Returns [`Error::Io`](crate::error::Error::Io) if the runtime can't be built.
pub(crate) fn handle() -> Result<tokio::runtime::Handle> {
    let mut guard = PROCESS_RUNTIME.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(runtime) = guard.as_ref() {
        return Ok(runtime.handle().clone());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("aranet-ble")
        .enable_all()
        .build()?;
    let handle = runtime.handle().clone();
    *guard = Some(runtime);
    Ok(handle)
}

/// Run `future` to completion on aranet-core's runtime and return its output.
///
/// `future`, and every task it spawns, keeps running after the caller's runtime
/// shuts down, and `future` runs to completion even if the caller stops waiting
/// for it.
///
/// Returns [`Error::Io`](crate::error::Error::Io) if the runtime can't be built
/// or `future` panics.
pub(crate) async fn run<F>(future: F) -> Result<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    let handle = handle()?;
    Ok(handle.spawn(future).await.map_err(std::io::Error::from)?)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn tasks_spawned_on_the_runtime_outlive_the_caller_runtime() {
        // btleplug spawns the adapter's event loop, and on Linux the D-Bus I/O
        // task, while the adapter or manager is created; those tasks must keep
        // running after the caller's runtime shuts down. The spawned task waits
        // for `release`, which is sent only once the caller's runtime is gone.
        let (release, released) = tokio::sync::oneshot::channel::<()>();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let caller = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        caller
            .block_on(run(async move {
                tokio::spawn(async move {
                    if released.await.is_ok() {
                        done_tx.send(()).unwrap();
                    }
                });
            }))
            .unwrap();
        drop(caller);

        // If the task died with the caller's runtime, `release` has no receiver
        // and `done_tx` is gone, so `recv_timeout` fails at once.
        let _ = release.send(());
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("task spawned on the aranet-ble runtime died with the caller's runtime");
    }

    #[test]
    fn run_executes_on_the_aranet_ble_thread() {
        let caller = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let name = caller
            .block_on(run(async {
                std::thread::current().name().map(str::to_owned)
            }))
            .unwrap();
        assert_eq!(name.as_deref(), Some("aranet-ble"));
    }
}
