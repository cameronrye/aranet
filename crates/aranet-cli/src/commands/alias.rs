//! Alias command implementation.
//!
//! Manages friendly device names (aliases) that map to device addresses.

use anyhow::{Result, bail};
use tabled::{builder::Builder, settings::Style};

use crate::config::Config;

/// Alias subcommand actions
pub enum AliasAction {
    /// List all aliases
    List,
    /// Set an alias
    Set { name: String, address: String },
    /// Remove an alias
    Remove { name: String },
}

pub fn cmd_alias(action: AliasAction, quiet: bool) -> Result<()> {
    let mut config = Config::load();

    match action {
        AliasAction::List => {
            if config.aliases.is_empty() {
                if !quiet {
                    println!("No aliases configured.");
                    println!();
                    println!("Add an alias with: aranet alias set <name> <address>");
                }
            } else {
                let mut builder = Builder::default();
                builder.push_record(["Alias", "Device Address"]);

                let mut aliases: Vec<_> = config.aliases.iter().collect();
                aliases.sort_by_key(|(name, _)| name.as_str());
                for (name, address) in aliases {
                    builder.push_record([name.as_str(), address.as_str()]);
                }

                let mut table = builder.build();
                table.with(Style::rounded());
                println!("{}", table);
            }
        }
        AliasAction::Set { name, address } => {
            // Validate the name doesn't look like a MAC address
            if looks_like_address(&name) {
                bail!(
                    "Alias name '{}' looks like a device address. \
                     Use a friendly name instead (e.g., 'living-room', 'office').",
                    name
                );
            }

            let was_update = config.aliases.contains_key(&name);
            config.aliases.insert(name.clone(), address.clone());
            config.save()?;

            if !quiet {
                if was_update {
                    println!("Updated alias '{}' → {}", name, address);
                } else {
                    println!("Added alias '{}' → {}", name, address);
                }
            }
        }
        AliasAction::Remove { name } => {
            if config.aliases.remove(&name).is_some() {
                config.save()?;
                if !quiet {
                    println!("Removed alias '{}'", name);
                }
            } else {
                bail!("Alias '{}' not found", name);
            }
        }
    }

    Ok(())
}

/// Check if a string looks like a device address (MAC or UUID).
fn looks_like_address(s: &str) -> bool {
    // MAC address pattern: XX:XX:XX:XX:XX:XX or XX-XX-XX-XX-XX-XX
    let mac_pattern = s.chars().filter(|c| *c == ':' || *c == '-').count() >= 5
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '-');

    // UUID pattern: contains mostly hex and dashes, 32+ chars
    let uuid_pattern = s.len() >= 32 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-');

    mac_pattern || uuid_pattern
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_looks_like_address_mac() {
        assert!(looks_like_address("AA:BB:CC:DD:EE:FF"));
        assert!(looks_like_address("aa:bb:cc:dd:ee:ff"));
        assert!(looks_like_address("AA-BB-CC-DD-EE-FF"));
    }

    #[test]
    fn test_looks_like_address_uuid() {
        assert!(looks_like_address("12345678-1234-1234-1234-123456789abc"));
        assert!(looks_like_address("12345678123412341234123456789abc"));
    }

    #[test]
    fn test_looks_like_address_friendly_names() {
        assert!(!looks_like_address("living-room"));
        assert!(!looks_like_address("office"));
        assert!(!looks_like_address("bedroom-sensor"));
        assert!(!looks_like_address("Aranet4"));
    }

    #[test]
    fn test_looks_like_address_short_mac() {
        // Incomplete MAC addresses should not match
        assert!(!looks_like_address("AA:BB:CC"));
        assert!(!looks_like_address("AA:BB"));
    }

    #[test]
    fn test_looks_like_address_mixed_case() {
        assert!(looks_like_address("Aa:Bb:Cc:Dd:Ee:Ff"));
        assert!(looks_like_address("aA:bB:cC:dD:eE:fF"));
    }

    #[test]
    fn test_looks_like_address_empty_and_short() {
        assert!(!looks_like_address(""));
        assert!(!looks_like_address("a"));
        assert!(!looks_like_address("abc"));
    }

    #[test]
    fn test_looks_like_address_numbers_only() {
        // Just numbers shouldn't look like an address
        assert!(!looks_like_address("12345"));
        assert!(!looks_like_address("123456"));
    }

    #[test]
    fn test_looks_like_address_with_spaces() {
        // Spaces in name should not match
        assert!(!looks_like_address("my sensor"));
        assert!(!looks_like_address("living room"));
    }
}
