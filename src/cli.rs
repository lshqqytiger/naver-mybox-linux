use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::{Result, api::MyboxApiClient, auth::TokenStore};

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
            eprintln!(
                "Create a personal access token in MYBOX Settings > Account and personal access token management."
            );
            let token = rpassword::prompt_password("MYBOX personal access token: ")?;
            let token = token.trim();
            if token.is_empty() {
                return Err("access token cannot be empty".into());
            }

            MyboxApiClient::new(token).health_check()?;
            let store = TokenStore::new(TokenStore::default_path()?);
            store.save(token)?;
            tracing::info!(path = %store.config_path.display(), "MYBOX login succeeded; token saved");
            Ok(())
        }
        Commands::HealthCheck => {
            let store = TokenStore::new(TokenStore::default_path()?);
            let token = store.load()?.ok_or("not logged in; run `myboxfs login`")?;
            let api = MyboxApiClient::new(token);
            api.health_check()?;
            tracing::info!("API health check succeeded");
            Ok(())
        }
    }
}
