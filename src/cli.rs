use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::{Result, api::MyboxApiClient};

#[derive(Debug, Parser)]
#[command(name = "myboxfs", about = "NAVER MYBOX adapter for Linux")]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Mount { mountpoint: PathBuf },
    Unmount { mountpoint: PathBuf },
    Login,
    HealthCheck,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Mount { mountpoint } => crate::fuse::mount(&mountpoint),
        Commands::Unmount { mountpoint } => crate::fuse::unmount(&mountpoint),
        Commands::Login => {
            tracing::info!("login flow not implemented yet");
            Ok(())
        }
        Commands::HealthCheck => {
            let api = MyboxApiClient::new();
            api.health_check()?;
            tracing::info!("API health check succeeded");
            Ok(())
        }
    }
}
