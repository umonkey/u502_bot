use crate::infra::config::get_secret;

const TELEGRAM_BOT_API: &str = "TELEGRAM_BOT_API";

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    tracing::info!("Starting the Telegram bot.");

    // Temporary scaffolding for issue #2: printing the token proves the loader works. Remove this
    // once the bot loop reads the token instead.
    match get_secret(TELEGRAM_BOT_API) {
        Ok(token) => println!("{token}"),
        Err(error) => {
            tracing::error!(%error, "Failed to load {TELEGRAM_BOT_API}");
            std::process::exit(1);
        }
    }
}
