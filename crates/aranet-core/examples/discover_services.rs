//! Discover all services and characteristics on a device
//!
//! Run with: `cargo run --example discover_services -- <DEVICE>`
//!
//! The device is found with `aranet_core::scan::find_device`, so it is matched
//! exactly, as everywhere in aranet: by the identifier that `aranet scan`
//! prints, its Bluetooth address (with or without colons) or its full name, in
//! any case. Part of a name matches nothing.

use std::env;

use btleplug::api::{CharPropFlags, Peripheral as _};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let Some(identifier) = args.get(1) else {
        eprintln!("Usage: {} <DEVICE>", args[0]);
        eprintln!();
        eprintln!("DEVICE is the identifier that `aranet scan` prints, the device's Bluetooth");
        eprintln!("address (with or without colons) or its full name, in any case. Only an");
        eprintln!("exact match counts: part of a name matches nothing.");
        std::process::exit(1);
    };

    println!("Looking for {}...", identifier);

    // The error says what went wrong: an empty DEVICE, several devices that
    // match it, none (listing the Aranet names that contain it), or a
    // Bluetooth failure.
    let (_adapter, p) = match aranet_core::scan::find_device(identifier).await {
        Ok(found) => found,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };
    let name = p.properties().await?.and_then(|props| props.local_name);

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
