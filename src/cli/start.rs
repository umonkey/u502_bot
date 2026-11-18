//! The `start` command: compose the clients, the repositories, and the long-running services.
//!
//! Everything fallible is resolved before the first task is spawned, so a configuration failure
//! exits with nothing started: no ingestor, no transcriber, no outbox, and no database write.

use std::process::ExitCode;

use crate::domains::attachment::AttachmentRepository;
use crate::domains::message::MessageRepository;
use crate::domains::outbox::OutboxRepository;
use crate::infra::ffmpeg::Ffmpeg;
use crate::infra::openrouter::{self, OpenRouter};
use crate::infra::telegram_bot::Telegram;
use crate::infra::{config::get_secret, database, sentry, signal};
use crate::services::ingestor::Ingestor;
use crate::services::outbox::Outbox;
use crate::services::shutdown::Shutdown;
use crate::services::transcriber::{Transcriber, Whitelist};

const TELEGRAM_API_TOKEN: &str = "TELEGRAM_API_TOKEN";

pub async fn run() -> ExitCode {
    let _sentry_guard = sentry::init();
    tracing::info!("Starting the Telegram bot.");

    match start().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

/// Builds every client and repository, then runs the services until a signal or a service ends them.
async fn start() -> Result<(), ExitCode> {
    // Registered first, so a signal that arrives during the slow startup work is remembered by the
    // sticky flag instead of killing the process.
    let shutdown = Shutdown::new();
    let _signals = tokio::spawn(watch_for_shutdown(shutdown.clone()));

    let token = resolve(
        get_secret(TELEGRAM_API_TOKEN),
        "Failed to load TELEGRAM_API_TOKEN",
    )?;
    let database = resolve(init_database().await, "Failed to open the database")?;

    let openrouter_key = resolve(
        get_secret(openrouter::OPENROUTER_API_KEY),
        "Failed to load OPENROUTER_API_KEY",
    )?;
    let model = openrouter::model_from_env();
    let chats = resolve(Whitelist::from_env(), "Failed to load TRANSCRIBE_CHATS")?;
    if chats.is_empty() {
        tracing::warn!(
            "TRANSCRIBE_CHATS is empty: no chat is transcribed. Audio and video are skipped and each skip logs its chat id, so add the ids you want transcribed to TRANSCRIBE_CHATS."
        );
    }

    tracing::info!(model = %model, "Using model {model} to transcribe; override with TRANSCRIBE_MODEL_NAME");

    let ffmpeg = Ffmpeg::from_env();
    if let Err(error) = ffmpeg.probe().await {
        tracing::warn!(
            error = &error as &dyn std::error::Error,
            "ffmpeg is not available: video transcription will be skipped; install ffmpeg or set FFMPEG_PATH"
        );
    }

    let openrouter = resolve(
        OpenRouter::new(openrouter_key, model),
        "Failed to build the OpenRouter client",
    )?;
    let telegram = resolve(
        Telegram::connect(token).await,
        "Failed to start the Telegram bot",
    )?;

    let pool = database.pool().clone();
    let ingestor = Ingestor::new(
        telegram.messages(),
        MessageRepository::new(pool.clone()),
        AttachmentRepository::new(pool.clone()),
    );
    let transcriber = Transcriber::new(
        telegram.clone(),
        openrouter,
        ffmpeg,
        AttachmentRepository::new(pool.clone()),
        OutboxRepository::new(pool.clone()),
        chats,
    );
    let outbox = Outbox::new(telegram, OutboxRepository::new(pool));

    let (ingestor, transcriber, outbox) = tokio::join!(
        tokio::spawn(ingestor.run(shutdown.clone())),
        tokio::spawn(transcriber.run(shutdown.clone())),
        tokio::spawn(outbox.run(shutdown.clone())),
    );

    let shutting_down = shutdown.is_requested();

    log_service_exit("ingestor", ingestor, shutting_down);
    log_service_exit("transcriber", transcriber, shutting_down);
    log_service_exit("outbox", outbox, shutting_down);

    database.close().await;
    tracing::info!("Stopped the bot.");

    Ok(())
}

/// Requests shutdown when the process receives a signal that asks it to stop.
async fn watch_for_shutdown(shutdown: Shutdown) {
    let signal = signal::wait().await;

    tracing::info!(
        signal = %signal,
        "Received a shutdown signal; stopping the services"
    );
    shutdown.request();
}

/// Opens the database named by `DATABASE_PATH` and applies the embedded schema.
async fn init_database() -> Result<database::Database, database::DatabaseError> {
    let path = database::database_path()?;

    let database = database::Database::connect(&path).await?;
    database.apply_schema().await?;
    tracing::info!("Using database {}", path.display());

    Ok(database)
}

/// Logs a startup failure and turns it into the exit code `start` propagates.
fn resolve<T, E>(result: Result<T, E>, message: &'static str) -> Result<T, ExitCode>
where
    E: std::error::Error + 'static,
{
    result.map_err(|error| {
        tracing::error!(error = &error as &dyn std::error::Error, "{message}");

        ExitCode::FAILURE
    })
}

/// Logs a service task that returned or panicked; the other two keep running unless shutdown was
/// requested.
fn log_service_exit(
    service: &'static str,
    result: Result<(), tokio::task::JoinError>,
    shutting_down: bool,
) {
    match result {
        Ok(()) if shutting_down => tracing::debug!(service, "A service stopped"),
        Ok(()) => tracing::error!(
            service,
            "A service returned; the remaining services keep running"
        ),
        Err(error) => tracing::error!(
            service,
            error = &error as &dyn std::error::Error,
            "A service panicked; the remaining services keep running"
        ),
    }
}
