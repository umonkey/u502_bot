use crate::infra::{config::get_secret, telegram_bot};

const TELEGRAM_API_TOKEN: &str = "TELEGRAM_API_TOKEN";

pub async fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    tracing::info!("Starting the Telegram bot.");

    let token = match get_secret(TELEGRAM_API_TOKEN) {
        Ok(token) => token,
        Err(error) => {
            tracing::error!(%error, "Failed to load {TELEGRAM_API_TOKEN}");
            std::process::exit(1);
        }
    };

    if let Err(error) = telegram_bot::run(token).await {
        tracing::error!(%error, "Telegram bot stopped");
        std::process::exit(1);
    }
}
