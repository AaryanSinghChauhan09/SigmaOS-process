//! SigmaOS-process - SigmaOS Component CLI
//!
//! Command-line interface for SigmaOS-process component.

use anyhow::Result;
use clap::{Parser, Subcommand};
use log::info;
use process::{Component, ComponentStatus, Config, process};

#[derive(Parser)]
#[command(name = "process")]
#[command(about = "SigmaOS SigmaOS-process component", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize the component
    Init {
        /// Enable debug mode
        #[arg(short, long)]
        debug: bool,
    },
    /// Start the component
    Start,
    /// Stop the component
    Stop,
    /// Get component status
    Status,
    /// Run the component
    Run {
        /// Enable debug mode
        #[arg(short, long)]
        debug: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    
    let config = Config::default();
    let mut component = process::new(config);

    match cli.command {
        Commands::Init { debug } => {
            if debug {
                println!("Initializing SigmaOS-process in debug mode...");
            } else {
                println!("Initializing SigmaOS-process...");
            }
            component.initialize()?;
            println!("SigmaOS-process initialized successfully");
        }
        Commands::Start => {
            component.initialize()?;
            component.start()?;
            println!("SigmaOS-process started successfully");
        }
        Commands::Stop => {
            component.stop()?;
            println!("SigmaOS-process stopped successfully");
        }
        Commands::Status => {
            let status = component.status();
            println!("SigmaOS-process status: {:?}", status);
        }
        Commands::Run { debug } => {
            if debug {
                println!("Running SigmaOS-process in debug mode...");
            } else {
                println!("Running SigmaOS-process...");
            }
            component.initialize()?;
            component.start()?;
            println!("SigmaOS-process is running. Press Ctrl+C to stop.");
            
            // Keep running
            tokio::runtime::Runtime::new()?.block_on(async {
                tokio::signal::ctrl_c().await.unwrap();
                info!("Received Ctrl+C, shutting down...");
            });
            
            component.stop()?;
            println!("SigmaOS-process stopped");
        }
    }

    Ok(())
}
