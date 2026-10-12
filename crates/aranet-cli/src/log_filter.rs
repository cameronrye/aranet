//! The log filter that the `aranet` commands and the GUI use when the user has chosen
//! none.
//!
//! This file is a module of two crates. The library compiles it for the GUI (feature
//! `gui`), and the `aranet` binary compiles it again for the CLI (feature `cli`): the
//! binary cannot call a private function of the library, and the filter is not part of
//! the library's public API. Everything in this file must therefore be used by both. An
//! item that only one of them uses is dead code in the other, and clippy fails on it.

/// Turns off the log target of btleplug's CoreBluetooth peripheral module.
///
/// On macOS, btleplug 0.13.5 logs `Shouldn't get anything but Ok!` at error level for
/// every disconnect that CoreBluetooth confirms, and then returns `Ok`
/// (btleplug 0.13.5 `src/corebluetooth/peripheral.rs:468-476`). The disconnect's own
/// reply is answered with "Device disconnected", like every other pending operation
/// (`src/corebluetooth/internal.rs:678-698`), and `disconnect()` logs any reply but `Ok`
/// as an error. Every command that connects ends with a disconnect, so without this
/// directive each of them would log an error after it had succeeded.
///
/// The line is that module's only warning or error. The target is not widened to
/// `btleplug`: other btleplug modules warn when Bluetooth is off or not permitted and
/// when a characteristic discovery fails, and those warnings are true. btleplug has the
/// module only on Apple platforms, so the directive does nothing on Linux and Windows.
///
/// Remove the directive when aranet moves to a btleplug release that no longer logs the
/// line.
const BTLEPLUG_DISCONNECT_ERROR_OFF: &str = "btleplug::corebluetooth::peripheral=off";

/// The filter directives to use when neither a flag nor `RUST_LOG` chooses the filter:
/// `level` for every target, without btleplug's disconnect error.
pub(crate) fn default_directives(level: &str) -> String {
    format!("{level},{BTLEPLUG_DISCONNECT_ERROR_OFF}")
}

/// What the tests of the CLI's and the GUI's filters share: a fixed set of log lines, and
/// which of them a filter shows.
#[cfg(test)]
pub(crate) mod testing {
    use std::sync::{Arc, Mutex, OnceLock};

    use tracing_subscriber::EnvFilter;

    /// How btleplug's disconnect error starts, as `lines_shown_by` returns it.
    const DISCONNECT_ERROR: &str = "ERROR btleplug::corebluetooth::peripheral: ";

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Keep a second, silent dispatcher registered for the rest of the test run.
    ///
    /// Tests on several threads call `lines_shown_by`, each with its own filter, and all
    /// of them reach the log statements in it. tracing caches, for the whole process,
    /// whether any subscriber wants a statement. While only one dispatcher is registered,
    /// it fills that cache from the calling thread's subscriber alone and without a lock,
    /// so the answer can overwrite the one just computed for a subscriber that another
    /// thread registered. With a second dispatcher always registered, the answer is
    /// computed over all of them, under the lock that registering takes. aranet-service's
    /// tests keep one for the same reason.
    fn register_silent_dispatcher() {
        static SILENT: OnceLock<tracing::Dispatch> = OnceLock::new();
        SILENT.get_or_init(|| tracing::Dispatch::new(tracing::subscriber::NoSubscriber::new()));
    }

    /// Logs a fixed set of lines under a subscriber with `filter`, and returns the lines
    /// the subscriber wrote, each as `LEVEL target: message`.
    ///
    /// The set has btleplug's disconnect error and an info line from the same module,
    /// warnings and errors from btleplug's other CoreBluetooth modules, an error from the
    /// crate's root and from its other backends, and lines of aranet's own.
    ///
    /// btleplug logs through the `log` crate. The bridge that the subscriber's `init`
    /// installs asks the subscriber about each record with the record's own target
    /// (tracing-log 0.2.0 `src/lib.rs:227-242`), so a filter treats these events as it
    /// treats btleplug's lines.
    pub(crate) fn lines_shown_by(filter: EnvFilter) -> Vec<String> {
        register_silent_dispatcher();
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        tracing::error!(
            target: "btleplug::corebluetooth::peripheral",
            "Shouldn't get anything but Ok!"
        );
        tracing::info!(
            target: "btleplug::corebluetooth::peripheral",
            "Event receiver died, breaking out of corebluetooth device loop."
        );
        tracing::warn!(
            target: "btleplug::corebluetooth::internal",
            "Adapter powered off, canceling all pending operations"
        );
        tracing::error!(
            target: "btleplug::corebluetooth::internal",
            "Error sending notification event"
        );
        tracing::warn!(
            target: "btleplug::corebluetooth::central_delegate",
            "Error discovering characteristics"
        );
        tracing::error!(
            target: "btleplug::corebluetooth::central_delegate",
            "an error from the delegate"
        );
        tracing::error!(target: "btleplug::corebluetooth::adapter", "an error from the adapter");
        tracing::error!(target: "btleplug::corebluetooth", "an error from the backend");
        tracing::error!(target: "btleplug", "an error from the crate root");
        tracing::error!(target: "btleplug::bluez::peripheral", "an error from BlueZ");
        tracing::error!(target: "btleplug::winrtble::peripheral", "an error from WinRT");
        tracing::warn!(target: "aranet_core::link", "a warning from aranet-core");
        tracing::error!(target: "aranet_cli::gui", "an error from aranet-cli");
        tracing::info!(target: "aranet_core::link", "an info line from aranet-core");

        let text = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        text.lines()
            .map(|line| line.trim_start().to_string())
            .collect()
    }

    /// Whether `lines`, as `lines_shown_by` returns them, hold btleplug's disconnect error.
    pub(crate) fn shows_disconnect_error(lines: &[String]) -> bool {
        lines.iter().any(|line| line.starts_with(DISCONNECT_ERROR))
    }

    /// `lines` without btleplug's disconnect error.
    pub(crate) fn without_disconnect_error(lines: Vec<String>) -> Vec<String> {
        lines
            .into_iter()
            .filter(|line| !line.starts_with(DISCONNECT_ERROR))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use tracing_subscriber::EnvFilter;

    use super::default_directives;
    use super::testing::{lines_shown_by, shows_disconnect_error, without_disconnect_error};

    /// At the CLI's level and at the GUI's, the default shows what the level alone shows
    /// minus one line. btleplug's other modules and aranet's own lines are untouched.
    #[test]
    fn the_default_hides_only_btleplugs_disconnect_error() {
        // Of the 14 lines `lines_shown_by` logs, 12 are warnings or errors and 9 are errors.
        for (level, lines_at_level) in [("warn", 12), ("error", 9)] {
            let plain = lines_shown_by(EnvFilter::new(level));
            assert_eq!(plain.len(), lines_at_level, "{level}: {plain:?}");
            assert!(shows_disconnect_error(&plain), "{level}: {plain:?}");

            let shown = lines_shown_by(EnvFilter::new(default_directives(level)));
            assert_eq!(shown, without_disconnect_error(plain), "{level}");
        }
    }
}
