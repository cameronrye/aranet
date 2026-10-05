//! Database access for the service: SQLite work runs off the async runtime.
//!
//! Every store call blocks its thread, and waits up to 5 s (rusqlite's busy
//! timeout) while another connection or process, such as `aranet sync`, the
//! TUI, the GUI or `aranet cache prune`, holds the database's write lock. So
//! store closures run on tokio's blocking thread pool through [`run_blocking`],
//! never on a runtime worker.
//!
//! - **Writes** use the one connection `run()` opened with `Store::open`. An
//!   owned `tokio::sync::Mutex` guard runs them one at a time, in the order they
//!   arrive; a write waiting for its turn holds no thread.
//! - **Reads** of a file-backed store use at most [`READ_POOL_SIZE`] connections
//!   opened with `Store::open_read_only` (`PRAGMA query_only = ON`). A semaphore
//!   bounds them, idle connections are kept for the next read, and WAL mode lets
//!   them read while the writer writes.
//! - **In-memory stores** can't be opened a second time, so reads use the writer.
//!
//! The service therefore keeps at most `1 + READ_POOL_SIZE` connections open.

use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, PoisonError};

use aranet_store::Store;
use tokio::sync::{Mutex, Semaphore};

/// Most read-only connections the service keeps open to a file-backed store.
pub(crate) const READ_POOL_SIZE: usize = 4;

/// The service's database: one writer connection and, for a file-backed store,
/// a bounded pool of read-only connections.
pub(crate) struct StoreAccess {
    writer: Arc<Mutex<Store>>,
    readers: Option<ReadPool>,
}

/// Read-only connections to the store's file, opened when first needed.
struct ReadPool {
    path: PathBuf,
    /// One permit per connection that may be in use at once.
    permits: Arc<Semaphore>,
    /// Connections not in use. Only a job that holds a permit takes one out or
    /// puts one back, so there are never more than `READ_POOL_SIZE`.
    idle: Arc<std::sync::Mutex<Vec<Store>>>,
    /// Connections opened so far.
    #[cfg(test)]
    opened: Arc<AtomicUsize>,
}

impl StoreAccess {
    /// Wrap the store `run()` opened. A file-backed store also gets a read
    /// pool on the same file.
    pub(crate) fn new(store: Store) -> Self {
        let readers = store.database_path().map(ReadPool::new);
        Self {
            writer: Arc::new(Mutex::new(store)),
            readers,
        }
    }

    /// Run `f` on the writer connection, on a blocking thread.
    pub(crate) async fn write<T, F>(&self, f: F) -> aranet_store::Result<T>
    where
        F: FnOnce(&Store) -> aranet_store::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        // Writers queue here without holding a thread. The guard moves into the
        // job, so the lock is held until `f` returns, even if the caller stops
        // waiting.
        let store = Arc::clone(&self.writer).lock_owned().await;
        run_blocking(move || f(&store)).await
    }

    /// Run `f` on a read-only connection, on a blocking thread. Without a read
    /// pool (an in-memory store), `f` runs on the writer.
    pub(crate) async fn read<T, F>(&self, f: F) -> aranet_store::Result<T>
    where
        F: FnOnce(&Store) -> aranet_store::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        match &self.readers {
            Some(pool) => pool.read(f).await,
            None => self.write(f).await,
        }
    }

    /// How many read-only connections have been opened so far.
    #[cfg(test)]
    pub(crate) fn read_connections_opened(&self) -> usize {
        self.readers
            .as_ref()
            .map_or(0, |pool| pool.opened.load(Ordering::SeqCst))
    }
}

impl ReadPool {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            permits: Arc::new(Semaphore::new(READ_POOL_SIZE)),
            idle: Arc::new(std::sync::Mutex::new(Vec::with_capacity(READ_POOL_SIZE))),
            #[cfg(test)]
            opened: Arc::new(AtomicUsize::new(0)),
        }
    }

    async fn read<T, F>(&self, f: F) -> aranet_store::Result<T>
    where
        F: FnOnce(&Store) -> aranet_store::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let permit = Arc::clone(&self.permits)
            .acquire_owned()
            .await
            .expect("the read pool's semaphore is never closed");
        let path = self.path.clone();
        let idle = Arc::clone(&self.idle);
        #[cfg(test)]
        let opened = Arc::clone(&self.opened);

        run_blocking(move || {
            let pooled = idle.lock().unwrap_or_else(PoisonError::into_inner).pop();
            let store = match pooled {
                Some(store) => store,
                None => {
                    let store = Store::open_read_only(&path)?;
                    #[cfg(test)]
                    opened.fetch_add(1, Ordering::SeqCst);
                    store
                }
            };
            // If `f` panics, unwinding closes this connection instead of
            // returning it, and releases the permit.
            let result = f(&store);
            idle.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(store);
            // Only now, so the next reader finds this connection idle.
            drop(permit);
            result
        })
        .await
    }
}

/// Run `f` on tokio's blocking thread pool and wait for its result.
///
/// A panic in `f` resumes in the caller, as if `f` had run inline. `f` runs to
/// the end even if the caller stops waiting.
pub(crate) async fn run_blocking<T, F>(f: F) -> aranet_store::Result<T>
where
    F: FnOnce() -> aranet_store::Result<T> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(result) => result,
        Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
        // The runtime shut down before the job started.
        Err(e) => Err(aranet_store::Error::Io(e.into())),
    }
}
