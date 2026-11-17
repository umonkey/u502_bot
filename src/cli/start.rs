use std::process::ExitCode;

use crate::infra::{config::get_secret, sentry, telegram_bot};

const TELEGRAM_API_TOKEN: &str = "TELEGRAM_API_TOKEN";

pub async fn run() -> ExitCode {
    let _sentry_guard = sentry::init();
    tracing::info!("Starting the Telegram bot.");

    let token = match get_secret(TELEGRAM_API_TOKEN) {
        Ok(token) => token,
        Err(error) => {
            tracing::error!(
                error = &error as &dyn std::error::Error,
                "Failed to load {TELEGRAM_API_TOKEN}"
            );
            return ExitCode::FAILURE;
        }
    };

    if let Err(error) = telegram_bot::run(token).await {
        tracing::error!(
            error = &error as &dyn std::error::Error,
            "Telegram bot stopped"
        );
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
