use anyhow::{Context, Result};
use tokio::net::TcpListener;
use tracing::info;

use relaybox::api::router;
use relaybox::application::DeliveryService;
use relaybox::config::Config;
use relaybox::infrastructure::SqliteDeliveryRepository;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env()?;

    let repository = SqliteDeliveryRepository::connect(&config.database_url)
        .await
        .with_context(|| format!("failed to open database {}", config.database_url))?;

    let service = DeliveryService::new(repository);
    let app = router(service);

    let listener = TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("failed to bind {}", config.bind_addr))?;
    info!(addr = %config.bind_addr, "relaybox listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("shutdown signal received");
}
