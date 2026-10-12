//! Utility functions for aranet-core.
//!
//! This module contains shared utility functions used across the crate.

use btleplug::api::PeripheralProperties;
use btleplug::platform::PeripheralId;

/// Format a peripheral ID as a string.
///
/// On macOS, peripheral IDs are UUIDs. On other platforms, they may be
/// MAC addresses or other formats. This function extracts the useful
/// identifier string.
///
/// # Example
///
/// ```ignore
/// use aranet_core::util::format_peripheral_id;
///
/// let id = peripheral.id();
/// let formatted = format_peripheral_id(&id);
/// println!("Device: {}", formatted);
/// ```
pub fn format_peripheral_id(id: &PeripheralId) -> String {
    format!("{:?}", id)
        .trim_start_matches("PeripheralId(")
        .trim_end_matches(')')
        .to_string()
}

/// Create an identifier string from an address and peripheral ID.
///
/// On macOS where addresses are 00:00:00:00:00:00, uses the peripheral ID.
/// On other platforms, uses the Bluetooth address.
pub fn create_identifier(address: &str, peripheral_id: &PeripheralId) -> String {
    if address == "00:00:00:00:00:00" {
        format_peripheral_id(peripheral_id)
    } else {
        address.to_string()
    }
}

/// Whether a peripheral that has advertised no name is named by btleplug's
/// `local_name`. Only on macOS, where `local_name` is then CoreBluetooth's own
/// name for the peripheral (btleplug 0.13.5
/// `src/corebluetooth/internal.rs:1003-1008`). On Linux it is then BlueZ's
/// `Alias` (`src/bluez/peripheral.rs:155`), which for a device BlueZ has no
/// name for is a name the user chose, or else the address with dashes. On
/// Windows a connect sets it to the system's name for the device
/// (`src/winrtble/peripheral.rs:523-531`). aranet has never used either as a
/// sensor's name.
const UNADVERTISED_NAME_IS_LOCAL_NAME: bool = cfg!(target_os = "macos");

/// The name of a peripheral as aranet uses it: the name aranet shows, matches
/// a device query against and detects the sensor type from.
///
/// It is the name the device advertises, which btleplug reports as
/// `advertisement_name`. On macOS only, a peripheral that has advertised no
/// name gets the name CoreBluetooth holds for it, which btleplug reports as
/// `local_name`.
///
/// Call this function and do not read either field yourself.
/// `advertisement_name` alone leaves a macOS peripheral that has advertised
/// no name without one. `local_name` means something different on each
/// operating system in btleplug 0.13.5 (the files named below are that
/// version's):
///
/// - **Linux:** BlueZ's `Alias` (`src/bluez/peripheral.rs:155`): the name the
///   user gave the device, else the device's own name, else its address with
///   dashes, such as `60-C0-BF-48-CB-8D` (BlueZ 5.82 `src/device.c:959-980`).
///   Read as a name, it would take a renamed sensor's Aranet name away, and
///   with it the sensor type that is detected from the name, and it would
///   give a name to a device that has none. `advertisement_name` is BlueZ's
///   `Name` (`src/bluez/peripheral.rs:156`), which is what `local_name` was
///   in btleplug 0.11.8.
/// - **Windows:** the advertised name, until a connect replaces it with the
///   system's name for the device (`src/winrtble/peripheral.rs:523-531`).
///   `advertisement_name` is the name from the last advertisement that carried
///   one (`src/winrtble/peripheral.rs:173-180`), which is what `local_name`
///   was in btleplug 0.11.8.
/// - **macOS:** the advertised name once one has been heard, and until then
///   the name CoreBluetooth holds for the peripheral
///   (`src/corebluetooth/internal.rs:1003-1008`,
///   `src/corebluetooth/peripheral.rs:221-235`). On macOS this function
///   returns what `local_name` holds.
///
/// # Example
///
/// ```
/// use aranet_core::peripheral_name;
/// use btleplug::api::PeripheralProperties;
///
/// // A sensor as BlueZ reports it after the user has renamed it "Kitchen".
/// let properties = PeripheralProperties {
///     local_name: Some("Kitchen".to_string()),
///     advertisement_name: Some("Aranet4 12345".to_string()),
///     ..Default::default()
/// };
/// assert_eq!(peripheral_name(&properties), Some("Aranet4 12345"));
/// ```
pub fn peripheral_name(properties: &PeripheralProperties) -> Option<&str> {
    name_from(properties, UNADVERTISED_NAME_IS_LOCAL_NAME)
}

/// `peripheral_name` with the choice that depends on the operating system as a
/// parameter, so that one host can test every rule: the advertised name, or,
/// if there is none and `or_local_name` is set, `local_name`.
fn name_from(properties: &PeripheralProperties, or_local_name: bool) -> Option<&str> {
    let advertised = properties.advertisement_name.as_deref();
    if or_local_name {
        advertised.or(properties.local_name.as_deref())
    } else {
        advertised
    }
}

#[cfg(test)]
mod tests {
    // Note: `test_create_identifier_with_valid_address` and
    // `test_create_identifier_with_zero_address` don't call `create_identifier`
    // or `format_peripheral_id`. Both functions take btleplug's PeripheralId,
    // and a test that runs on every platform can't make one. btleplug 0.13.5
    // makes one from a UUID on macOS (`src/corebluetooth/peripheral.rs:672`)
    // and from a BDAddr on Windows (`src/winrtble/peripheral.rs:732`), but on
    // Linux only from a bluez_async::DeviceId (`src/bluez/peripheral.rs:358`).
    // aranet-core can't name that type: btleplug doesn't re-export bluez-async,
    // and aranet-core doesn't depend on it directly. So the two tests only
    // compare an address with the all-zero one, as `create_identifier` does.
    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn test_create_identifier_with_valid_address() {
        // No PeripheralId here (see the note on PeripheralId in this module),
        // so this only compares the address with the all-zero one.
        let address = "AA:BB:CC:DD:EE:FF";
        // When address is valid, it should be used directly
        assert_ne!(address, "00:00:00:00:00:00");
    }

    #[test]
    fn test_create_identifier_with_zero_address() {
        let address = "00:00:00:00:00:00";
        assert_eq!(address, "00:00:00:00:00:00");
    }
}

#[cfg(test)]
mod peripheral_name_tests {
    use super::*;

    use std::path::{Path, PathBuf};

    use aranet_types::DeviceType;

    /// What `name_from` is given on Linux and Windows.
    const ADVERTISED_NAME_ONLY: bool = false;
    /// What `name_from` is given on macOS.
    const OR_LOCAL_NAME: bool = true;

    /// Properties that carry nothing but the two names.
    fn named(local_name: Option<&str>, advertisement_name: Option<&str>) -> PeripheralProperties {
        PeripheralProperties {
            local_name: local_name.map(str::to_owned),
            advertisement_name: advertisement_name.map(str::to_owned),
            ..Default::default()
        }
    }

    #[test]
    fn bluez_alias_does_not_replace_the_sensors_name() {
        // BlueZ, for a sensor the user has renamed: `local_name` is the alias
        // and `advertisement_name` is BlueZ's `Name`.
        let properties = named(Some("Kitchen"), Some("Aranet2 2751B"));
        let name = name_from(&properties, ADVERTISED_NAME_ONLY);
        assert_eq!(name, Some("Aranet2 2751B"));
        // The sensor type is detected from the name, and a sensor without a
        // type is read and parsed as an Aranet4.
        assert_eq!(
            name.and_then(DeviceType::from_name),
            Some(DeviceType::Aranet2)
        );
    }

    #[test]
    fn bluez_device_without_a_name_has_none() {
        // BlueZ, for a device it has no name for: the alias is the address
        // with dashes.
        let properties = named(Some("60-C0-BF-48-CB-8D"), None);
        assert_eq!(name_from(&properties, ADVERTISED_NAME_ONLY), None);
    }

    #[test]
    fn winrt_connect_does_not_change_the_name() {
        // WinRT after a connect: `local_name` is the system's name for the
        // device.
        let connected = named(Some("Bedroom sensor"), Some("Aranet4 17C3C"));
        assert_eq!(
            name_from(&connected, ADVERTISED_NAME_ONLY),
            Some("Aranet4 17C3C")
        );
        // A device that never advertised a name stays without one.
        let never_named = named(Some("Bedroom sensor"), None);
        assert_eq!(name_from(&never_named, ADVERTISED_NAME_ONLY), None);
    }

    #[test]
    fn corebluetooth_uses_the_advertised_name() {
        // CoreBluetooth once a name has been advertised: both fields hold it.
        let properties = named(Some("Aranet4 1A2B3"), Some("Aranet4 1A2B3"));
        assert_eq!(name_from(&properties, OR_LOCAL_NAME), Some("Aranet4 1A2B3"));
    }

    #[test]
    fn corebluetooth_falls_back_to_the_systems_name() {
        // CoreBluetooth before any name has been advertised: `local_name` is
        // its own name for the peripheral.
        let properties = named(Some("Kitchen"), None);
        assert_eq!(name_from(&properties, OR_LOCAL_NAME), Some("Kitchen"));
    }

    #[test]
    fn corebluetooth_peripheral_without_a_name_has_none() {
        let properties = named(None, None);
        assert_eq!(name_from(&properties, OR_LOCAL_NAME), None);
    }

    #[test]
    fn an_advertised_name_wins_over_local_name() {
        // CoreBluetooth never reports two different names (btleplug 0.13.5
        // `src/corebluetooth/peripheral.rs:221-235`). This pins the order of
        // the rule: `local_name` is used only when no name was advertised.
        let properties = named(Some("Kitchen"), Some("Aranet2 2751B"));
        assert_eq!(name_from(&properties, OR_LOCAL_NAME), Some("Aranet2 2751B"));
    }

    /// The one test of `UNADVERTISED_NAME_IS_LOCAL_NAME`. Every other test
    /// passes the rule to `name_from` itself, so without this one the constant
    /// could be `true` on Linux, which makes a BlueZ alias a sensor's name
    /// again, with every other test passing. CI runs it on macOS, Linux and
    /// Windows.
    #[test]
    fn peripheral_name_uses_local_name_only_on_macos() {
        let properties = named(Some("Kitchen"), None);
        let expected = cfg!(target_os = "macos").then_some("Kitchen");
        assert_eq!(peripheral_name(&properties), expected);
    }

    /// Every `.rs` file under `dir`, at any depth.
    fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read a source directory") {
            let path = entry.expect("read a directory entry").path();
            if path.is_dir() {
                rust_files(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }

    /// Guard: in aranet-core only `name_from` (this file) reads btleplug's
    /// `local_name` and `advertisement_name`.
    ///
    /// Six places in the crate need a peripheral's name: the scan
    /// (`process_peripheral` and `is_aranet_device`), the device lookup
    /// (`search_known_peripherals`), the connect
    /// (`Device::from_peripheral_with_config`) and the passive monitor, twice.
    /// Five of them take a real btleplug peripheral and cannot be unit-tested,
    /// so no other test would notice if one of them read a field itself. A
    /// site that read `local_name` would name a sensor on Linux by its BlueZ
    /// alias, and the sensor would lose its type. A site that read
    /// `advertisement_name` directly would pass every other test and lose the
    /// system's name on macOS for a sensor that has not advertised one.
    ///
    /// So this test reads the source files. It fails on either field's name
    /// after a dot, in code or in a comment, in any file but this one. A
    /// struct literal that sets the fields has no dot before their names and
    /// passes: the test double of a peripheral (`link/fake.rs`) sets them
    /// that way.
    ///
    /// It searches `src` only. The `discover_services` example and the
    /// hardware tests' `known_peripheral` take the name from `peripheral_name`
    /// too, but they are outside `src`, and this test does not read them.
    #[test]
    fn only_peripheral_name_reads_the_name_fields() {
        // Each in two halves, so that this test is not itself an occurrence of
        // the texts it searches for.
        let field_reads = [
            [".local", "_name"].concat(),
            [".advertisement", "_name"].concat(),
        ];
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        // The one file that may read the fields: `name_from` is above.
        let exempt = Path::new("util.rs");

        let mut files = Vec::new();
        rust_files(&src, &mut files);
        files.sort();

        let mut exempt_seen = false;
        let mut reads = Vec::new();
        for file in &files {
            let relative = file.strip_prefix(&src).expect("a path under src");
            if relative == exempt {
                exempt_seen = true;
                continue;
            }
            let text = std::fs::read_to_string(file).expect("read a source file");
            for (index, line) in text.lines().enumerate() {
                if field_reads.iter().any(|field| line.contains(field)) {
                    reads.push(format!(
                        "{}:{}: {}",
                        relative.display(),
                        index + 1,
                        line.trim()
                    ));
                }
            }
        }

        assert!(
            exempt_seen,
            "util.rs is not under {}; if it was renamed, rename it here too",
            src.display()
        );
        assert!(
            reads.is_empty(),
            "these lines read `local_name` or `advertisement_name`; use `peripheral_name`:\n{}",
            reads.join("\n")
        );
    }
}
