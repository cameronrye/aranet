use std::io;
use std::path::PathBuf;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "aranet")]
#[command(author, version, about = "CLI for Aranet environmental sensors", long_about = None)]
struct Cli {
    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Suppress non-essential output
    #[arg(short, long, global = true)]
    quiet: bool,

    /// Write output to file instead of stdout
    #[arg(short, long, global = true)]
    output: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan for nearby Aranet devices
    Scan {
        /// Scan timeout in seconds
        #[arg(short, long, default_value = "10")]
        timeout: u64,
    },

    /// Read current sensor values from a device
    Read {
        /// Device address (MAC address or UUID)
        #[arg(short, long)]
        device: Option<String>,

        /// Output format (text, json)
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// Retrieve historical data from a device
    History {
        /// Device address (MAC address or UUID)
        #[arg(short, long)]
        device: Option<String>,

        /// Number of records to retrieve (0 for all)
        #[arg(short, long, default_value = "0")]
        count: u32,

        /// Output format (text, json, csv)
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// Display device information
    Info {
        /// Device address (MAC address or UUID)
        #[arg(short, long)]
        device: Option<String>,
    },

    /// Configure device settings
    Set {
        /// Device address (MAC address or UUID)
        #[arg(short, long)]
        device: Option<String>,

        /// Setting name
        #[arg(short, long)]
        setting: String,

        /// Setting value
        #[arg(short, long)]
        value: String,
    },

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle completions command early (before tracing init)
    if let Commands::Completions { shell } = cli.command {
        let mut cmd = Cli::command();
        clap_complete::generate(shell, &mut cmd, "aranet", &mut io::stdout());
        return Ok(());
    }

    // Initialize tracing
    // When quiet mode is enabled, suppress info-level logging
    let filter = if cli.quiet {
        EnvFilter::new("warn")
    } else if cli.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
    };

    tracing_subscriber::fmt().with_env_filter(filter).init();

    // Note: When output flag is set, file output would be written to cli.output
    // For now, we just acknowledge where output would go
    if let Some(ref path) = cli.output {
        tracing::debug!("Output will be written to: {}", path.display());
    }

    match cli.command {
        Commands::Scan { timeout } => {
            if !cli.quiet {
                tracing::info!("Scanning for Aranet devices (timeout: {}s)...", timeout);
            }
            // TODO: When cli.output is Some, write to file instead of stdout
            println!("Scan command not yet implemented");
        }
        Commands::Read { device, format } => {
            let device_str = device.as_deref().unwrap_or("auto-detect");
            if !cli.quiet {
                tracing::info!("Reading from device: {} (format: {})", device_str, format);
            }
            // TODO: When cli.output is Some, write to file instead of stdout
            println!("Read command not yet implemented");
        }
        Commands::History {
            device,
            count,
            format,
        } => {
            let device_str = device.as_deref().unwrap_or("auto-detect");
            if !cli.quiet {
                tracing::info!(
                    "Retrieving history from device: {} (count: {}, format: {})",
                    device_str,
                    count,
                    format
                );
            }
            // TODO: When cli.output is Some, write to file instead of stdout
            println!("History command not yet implemented");
        }
        Commands::Info { device } => {
            let device_str = device.as_deref().unwrap_or("auto-detect");
            if !cli.quiet {
                tracing::info!("Getting info for device: {}", device_str);
            }
            // TODO: When cli.output is Some, write to file instead of stdout
            println!("Info command not yet implemented");
        }
        Commands::Set {
            device,
            setting,
            value,
        } => {
            let device_str = device.as_deref().unwrap_or("auto-detect");
            if !cli.quiet {
                tracing::info!("Setting {} = {} on device: {}", setting, value, device_str);
            }
            // TODO: When cli.output is Some, write to file instead of stdout
            println!("Set command not yet implemented");
        }
        Commands::Completions { .. } => {
            // Already handled above
            unreachable!()
        }
    }

    Ok(())
}
