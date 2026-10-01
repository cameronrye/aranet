//! Discover all services and characteristics on a device
//!
//! Run with: `cargo run --example discover_services -- <DEVICE> [--read-battery]`
//!
//! The device is found with `aranet_core::scan::find_device`, so it is matched
//! exactly, as everywhere in aranet: by the identifier that `aranet scan`
//! prints, its Bluetooth address (with or without colons) or its full name, in
//! any case. Part of a name matches nothing.
//!
//! The example then connects with btleplug directly, without the pairing step
//! that aranet's own connections take on Linux, and gives each step the time
//! limit that aranet's own connections use (`ConnectionConfig::default()`).
//! Once it has asked to connect, it disconnects whatever happens next, also
//! when a step fails or times out, or on Ctrl-C: a connect that fails can leave
//! the link up, and BlueZ can keep a link up after the process that asked for
//! it has exited. It reads every readable characteristic except Battery Level
//! (0x2A19): an Aranet sensor answers that read only once paired, so the read
//! starts a pairing (on macOS, a pairing dialog). `--read-battery` reads it
//! too.

use std::env;
use std::error::Error;
use std::time::Duration;

use aranet_core::ConnectionConfig;
use aranet_core::uuid::BATTERY_LEVEL;
use btleplug::api::{CharPropFlags, Peripheral as _};
use btleplug::platform::Peripheral;
use tokio::time::timeout;

/// How long the disconnect may wait for the Bluetooth stack to confirm it, as
/// aranet's own disconnects do.
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(5);

const USAGE: &str = "\
Usage: discover_services <DEVICE> [--read-battery]

DEVICE is the identifier that `aranet scan` prints, the device's Bluetooth
address (with or without colons) or its full name, in any case. Only an
exact match counts: part of a name matches nothing.

--read-battery also reads Battery Level (0x2A19). An Aranet sensor answers
that read only once paired, so it starts a pairing.";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut identifier = None;
    let mut read_battery = false;
    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--read-battery" => read_battery = true,
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ if identifier.is_none() && !arg.starts_with('-') => identifier = Some(arg),
            _ => {
                eprintln!("{USAGE}");
                std::process::exit(1);
            }
        }
    }
    let Some(identifier) = identifier else {
        eprintln!("{USAGE}");
        std::process::exit(1);
    };

    println!("Looking for {}...", identifier);

    // The error says what went wrong: an empty DEVICE, several devices that
    // match it, none (listing the Aranet names that contain it), or a
    // Bluetooth failure.
    let (_adapter, p) = match aranet_core::scan::find_device(&identifier).await {
        Ok(found) => found,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };
    let name = p.properties().await?.and_then(|props| props.local_name);

    println!("\nFound: {}", name.as_deref().unwrap_or(&identifier));

    let result = tokio::select! {
        result = inspect(&p, read_battery) => result,
        Ok(()) = tokio::signal::ctrl_c() => Err("interrupted".into()),
    };
    // Even after a connect that failed or timed out: the link can be up
    // anyway, and on macOS an abandoned connect can still complete later.
    match timeout(DISCONNECT_TIMEOUT, p.disconnect()).await {
        Ok(Ok(())) => println!("Disconnected."),
        Ok(Err(e)) => eprintln!("Disconnect failed: {e}"),
        Err(_) => eprintln!(
            "Disconnect didn't finish within {}s",
            DISCONNECT_TIMEOUT.as_secs()
        ),
    }
    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
    Ok(())
}

/// Connects to `p`, lists its services and characteristics and reads the
/// readable ones. `main` disconnects afterwards, whatever this returns.
async fn inspect(p: &Peripheral, read_battery: bool) -> Result<(), Box<dyn Error>> {
    let limits = ConnectionConfig::default();

    println!("Connecting...");
    limited("connect", limits.connection_timeout, p.connect()).await?;
    println!("Connected!");

    println!("Discovering services...");
    limited(
        "service discovery",
        limits.discovery_timeout,
        p.discover_services(),
    )
    .await?;

    println!("\n=== SERVICES AND CHARACTERISTICS ===\n");

    for service in p.services() {
        println!("Service: {}", service.uuid);
        for char in &service.characteristics {
            let mut flags = Vec::new();
            if char.properties.contains(CharPropFlags::READ) {
                flags.push("R");
            }
            if char.properties.contains(CharPropFlags::WRITE) {
                flags.push("W");
            }
            if char
                .properties
                .contains(CharPropFlags::WRITE_WITHOUT_RESPONSE)
            {
                flags.push("Wn");
            }
            if char.properties.contains(CharPropFlags::NOTIFY) {
                flags.push("N");
            }
            if char.properties.contains(CharPropFlags::INDICATE) {
                flags.push("I");
            }

            println!("  Char: {} [{}]", char.uuid, flags.join(","));

            if !char.properties.contains(CharPropFlags::READ) {
                continue;
            }
            if char.uuid == BATTERY_LEVEL && !read_battery {
                println!(
                    "        -> (not read: the read starts a pairing; --read-battery reads it)"
                );
                continue;
            }
            match timeout(limits.read_timeout, p.read(char)).await {
                Ok(Ok(data)) => print_value(&data),
                Ok(Err(e)) => println!("        -> (read error: {})", e),
                Err(_) => println!(
                    "        -> (read timed out after {}s)",
                    limits.read_timeout.as_secs()
                ),
            }
        }
        println!();
    }
    Ok(())
}

/// Runs one btleplug step, giving up after `limit`.
async fn limited<T>(
    step: &str,
    limit: Duration,
    future: impl Future<Output = btleplug::Result<T>>,
) -> Result<T, Box<dyn Error>> {
    match timeout(limit, future).await {
        Ok(result) => Ok(result?),
        Err(_) => Err(format!("{step} didn't finish within {}s", limit.as_secs()).into()),
    }
}

/// Prints a value read from a characteristic: as text if it is short and
/// printable, otherwise as bytes.
fn print_value(data: &[u8]) {
    if data.len() <= 20 {
        // Try as string
        if let Ok(s) = String::from_utf8(data.to_vec()) {
            let s = s.trim_end_matches('\0');
            if !s.is_empty() && s.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
                println!("        -> \"{}\"", s);
            } else {
                println!("        -> {:02X?}", data);
            }
        } else {
            println!("        -> {:02X?}", data);
        }
    } else {
        println!(
            "        -> [{} bytes] {:02X?}...",
            data.len(),
            &data[..20.min(data.len())]
        );
    }
}
