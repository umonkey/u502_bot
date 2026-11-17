use std::process::ExitCode;

use teloxide::types::{Message, User};

use crate::domains::message::{MessageRepository, NewMessage, StoreOutcome};
use crate::infra::{config::get_secret, database, sentry, telegram_bot};

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

    let database = match init_database().await {
        Ok(database) => database,
        Err(error) => {
            tracing::error!(
                error = &error as &dyn std::error::Error,
                "Failed to open the database"
            );
            return ExitCode::FAILURE;
        }
    };
    let repository = MessageRepository::new(database.pool().clone());

    let mut messages = match telegram_bot::messages(token).await {
        Ok(messages) => messages,
        Err(error) => {
            tracing::error!(
                error = &error as &dyn std::error::Error,
                "Failed to start the Telegram bot"
            );
            return ExitCode::FAILURE;
        }
    };

    while let Some(message) = messages.next().await {
        sentry::message_received();
        tracing::info!(
            chat_id = message.chat.id.0,
            message_id = message.id.0,
            text = message.text().unwrap_or("<no text>"),
            "Received message"
        );

        match repository.store(&new_message(&message)).await {
            Ok(StoreOutcome::Stored) => sentry::message_stored(),
            Ok(StoreOutcome::Duplicate) => {
                sentry::message_stored();
                tracing::debug!(
                    chat_id = message.chat.id.0,
                    message_id = message.id.0,
                    "Skipped an already stored message"
                );
            }
            Err(error) => tracing::error!(
                error = &error as &dyn std::error::Error,
                "Failed to store the message"
            ),
        }
    }

    ExitCode::SUCCESS
}

/// Opens the database named by `DATABASE_PATH` and applies the embedded schema.
async fn init_database() -> Result<database::Database, database::DatabaseError> {
    let path = database::database_path()?;

    let database = database::Database::connect(&path).await?;
    database.apply_schema().await?;
    tracing::info!("Using database {}", path.display());

    Ok(database)
}

/// Maps a Telegram message to the row the repository stores.
fn new_message(message: &Message) -> NewMessage {
    NewMessage {
        chat_id: message.chat.id.0,
        message_id: i64::from(message.id.0),
        received_at: message.date.timestamp(),
        sender_id: message
            .from
            .as_ref()
            .and_then(|user| i64::try_from(user.id.0).ok()),
        sender_name: message.from.as_ref().map(User::full_name),
        text: message
            .text()
            .or_else(|| message.caption())
            .map(str::to_owned),
    }
}
