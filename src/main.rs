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
    let local_addr = listener.local_addr().with_context(|| {
        format!(
            "failed to read local address of bound listener on {}",
            config.bind_addr
        )
    })?;
    info!(addr = %local_addr, "relaybox listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        Ok(mut terminate) => {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to register SIGTERM handler; falling back to Ctrl-C only");
            let _ = tokio::signal::ctrl_c().await;
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }

    info!("shutdown signal received");
}
