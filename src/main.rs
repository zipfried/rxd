#![warn(clippy::unwrap_used)]

mod task;

use std::sync::Arc;

use tracing::info;
use tracing_indicatif::IndicatifLayer;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    let config: task::Config = toml::from_str(include_str!("../config.toml"))?;

    for task_config in config.tasks.iter() {
        let task = Arc::new(
            task::Task::new(
                &task_config.screen_name,
                &config.auth_token,
                &config.ct0,
                config.concurrent_downloads,
            )
            .await?,
        );
        task.execute().await?;
    }

    Ok(())
}
