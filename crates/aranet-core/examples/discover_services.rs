//! Discover all services and characteristics on a device

use std::env;
use std::time::Duration;

use btleplug::api::{Central, CharPropFlags, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::Manager;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let identifier = if args.len() > 1 {
        &args[1]
    } else {
        eprintln!("Usage: {} <DEVICE_NAME>", args[0]);
        std::process::exit(1);
    };

    println!("Scanning for {}...", identifier);

    let manager = Manager::new().await?;
    let adapters = manager.adapters().await?;
    let adapter = adapters.into_iter().next().ok_or("No adapter")?;

    adapter.start_scan(ScanFilter::default()).await?;
    sleep(Duration::from_secs(10)).await;
    adapter.stop_scan().await?;

    let peripherals = adapter.peripherals().await?;
    let identifier_lower = identifier.to_lowercase();

    for p in peripherals {
        if let Some(props) = p.properties().await?
            && let Some(name) = &props.local_name
            && name.to_lowercase().contains(&identifier_lower)
        {
            println!("\nFound: {}", name);
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
            return Ok(());
        }
    }

    println!("Device not found: {}", identifier);
    Ok(())
}
