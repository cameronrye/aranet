//! Discover all services and characteristics on a device
//!
//! Run with: `cargo run --example discover_services -- <DEVICE>`
//!
//! The device is matched exactly, as `aranet_core::scan` does: by the
//! identifier that `aranet scan` prints, its Bluetooth address (with or
//! without colons) or its full name, in any case. Part of a name matches
//! nothing.

use std::env;
use std::time::Duration;

use aranet_core::create_identifier;
use btleplug::api::{Central, CharPropFlags, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::Manager;
use tokio::time::sleep;

/// The address CoreBluetooth reports for every peripheral, which names none.
const UNKNOWN_ADDRESS: &str = "00:00:00:00:00:00";

/// Whether the advertised `name` is `query` (in lower case): the whole name,
/// or either half of CoreBluetooth's combined `"<GAP name> [<advertised name>]"`.
fn name_is(name: &str, query: &str) -> bool {
    let name = name.trim().to_lowercase();
    name == query
        || name
            .strip_suffix(']')
            .and_then(|combined| combined.rsplit_once(" ["))
            .is_some_and(|(gap, advertised)| gap.trim() == query || advertised.trim() == query)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let Some(identifier) = args
        .get(1)
        .map(|arg| arg.trim())
        .filter(|arg| !arg.is_empty())
    else {
        eprintln!("Usage: {} <DEVICE>", args[0]);
        eprintln!();
        eprintln!("DEVICE is the identifier that `aranet scan` prints, the device's Bluetooth");
        eprintln!("address (with or without colons) or its full name, in any case. Only an");
        eprintln!("exact match counts: part of a name matches nothing.");
        std::process::exit(1);
    };

    println!("Scanning for {}...", identifier);

    let manager = Manager::new().await?;
    let adapters = manager.adapters().await?;
    let adapter = adapters.into_iter().next().ok_or("No adapter")?;

    adapter.start_scan(ScanFilter::default()).await?;
    sleep(Duration::from_secs(10)).await;
    adapter.stop_scan().await?;

    let query = identifier.to_lowercase();
    let bare_query = query.replace(':', "");
    // As in aranet-core, a match by identifier or address beats a match by
    // name, and a name that two devices share names neither.
    let mut by_identifier = Vec::new();
    let mut by_name = Vec::new();
    for p in adapter.peripherals().await? {
        let Some(props) = p.properties().await? else {
            continue;
        };
        let id = p.id();
        let address = props.address.to_string();
        if create_identifier(&address, &id).to_lowercase() == query
            || id.to_string().to_lowercase() == query
            || (address != UNKNOWN_ADDRESS && address.to_lowercase().replace(':', "") == bare_query)
        {
            by_identifier.push((p, props.local_name));
        } else if props
            .local_name
            .as_deref()
            .is_some_and(|name| name_is(name, &query))
        {
            by_name.push((p, props.local_name));
        }
    }
    let mut found = if by_identifier.is_empty() {
        by_name
    } else {
        by_identifier
    };
    if found.len() > 1 {
        eprintln!(
            "{} devices match {}; give the identifier that `aranet scan` prints instead",
            found.len(),
            identifier
        );
        std::process::exit(1);
    }
    let Some((p, name)) = found.pop() else {
        println!("Device not found: {}", identifier);
        return Ok(());
    };

    println!("\nFound: {}", name.as_deref().unwrap_or(identifier));
    println!("Connecting...");

    p.connect().await?;
    println!("Connected!");

    println!("Discovering services...");
    p.discover_services().await?;

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

            // Try to read if readable
            if char.properties.contains(CharPropFlags::READ) {
                match p.read(char).await {
                    Ok(data) => {
                        if data.len() <= 20 {
                            // Try as string
                            if let Ok(s) = String::from_utf8(data.clone()) {
                                let s = s.trim_end_matches('\0');
                                if !s.is_empty()
                                    && s.chars().all(|c| c.is_ascii_graphic() || c == ' ')
                                {
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
                    Err(e) => println!("        -> (read error: {})", e),
                }
            }
        }
        println!();
    }

    p.disconnect().await?;
    println!("Disconnected.");
    Ok(())
}
