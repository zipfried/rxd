#![warn(clippy::unwrap_used)]

mod db;
mod task;

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use tracing::info;
use tracing_indicatif::IndicatifLayer;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Download with a config file
    Download {
        /// Path to config file
        config_path: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();

    let indicatif_layer = IndicatifLayer::new();
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(indicatif_layer.get_stderr_writer())
                .with_timer(tracing_subscriber::fmt::time::LocalTime::new(
                    time::macros::format_description!("[hour]:[minute]:[second]"),
                )),
        )
        .with(indicatif_layer)
        .with(LevelFilter::INFO)
        .init();
    info!("tracing initialized");

    let (raw_config, config_dir) = match &cli.command {
        Command::Download { config_path } => {
            info!("reading {}", config_path.display());
            let content = fs::read_to_string(config_path)?;
            let dir = config_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            (content, dir)
        }
    };
    let config: task::Config = toml::from_str(&raw_config)?;

    // Initialize database in the same directory as config file
    let db_path = config_dir.join("rxd.db");
    let db = db::init_db(&db_path).await?;

    for task_config in config.tasks.iter() {
        let task = Arc::new(
            task::Task::new(
                &task_config.screen_name,
                &config.auth_token,
                &config.ct0,
                config.concurrent_downloads,
                task_config.save_path.as_deref(),
                db.clone(),
            )
            .await?,
        );
        task.execute().await?;
    }

    Ok(())
}
