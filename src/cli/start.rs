//! The `start` command: compose the clients, the repositories, and the long-running services.
//!
//! Everything fallible is resolved before the first task is spawned, so a configuration failure
//! exits with nothing started: no ingestor, no transcriber, no outbox, and no database write.
//! Configuration is loaded before the `tracing` subscriber exists, so that failure is reported after
//! a second best-effort DSN resolution.

use std::path::Path;
use std::process::ExitCode;

use crate::domains::attachment::AttachmentRepository;
use crate::domains::message::MessageRepository;
use crate::domains::outbox::OutboxRepository;
use crate::infra::config::{self, Settings};
use crate::infra::ffmpeg::Ffmpeg;
use crate::infra::openrouter::OpenRouter;
use crate::infra::telegram_bot::Telegram;
use crate::infra::{database, sentry, signal};
use crate::services::ingestor::Ingestor;
use crate::services::outbox::Outbox;
use crate::services::shutdown::Shutdown;
use crate::services::transcriber::Transcriber;
use crate::services::wake::Wake;
use crate::shared::result::{Error, Result};

pub async fn run() -> ExitCode {
    let settings = match Settings::load() {
        Ok(settings) => settings,
        Err(error) => return fail_to_load(error),
    };
    let _sentry_guard = sentry::init(&settings.sentry_dsn);
    tracing::info!("Starting the Telegram bot.");

    match start(settings).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

/// Reports a configuration failure that happened before the subscriber was installed.
///
/// The DSN is resolved a second time, on a path that is already exiting, so a usable destination
/// still receives the failure. `sentry::init` is called once per process: the success path has
/// already returned when this runs.
fn fail_to_load(error: Error) -> ExitCode {
    let _sentry_guard = sentry::init(&config::sentry_dsn());
    tracing::error!(
        error = &error as &dyn std::error::Error,
        "Failed to load the configuration"
    );

    ExitCode::FAILURE
}

/// Builds every client and repository, then runs the services until a signal or a service ends them.
async fn start(settings: Settings) -> Result<(), ExitCode> {
    // Registered first, so a signal that arrives during the slow startup work is remembered by the
    // sticky flag instead of killing the process.
    let shutdown = Shutdown::new();
    let _signals = tokio::spawn(watch_for_shutdown(shutdown.clone()));

    let database = resolve(
        init_database(&settings.database_path).await,
        "Failed to open the database",
    )?;

    if settings.transcribe_chats.is_empty() {
        tracing::warn!(
            "TRANSCRIBE_CHATS is empty: no chat is transcribed. Audio and video are skipped and each skip logs its chat id, so add the ids you want transcribed to TRANSCRIBE_CHATS."
        );
    }

    let model = &settings.transcribe_model;
    tracing::info!(model = %model, "Using model {model} to transcribe; override with TRANSCRIBE_MODEL_NAME");

    let ffmpeg = Ffmpeg::default();
    if let Err(error) = ffmpeg.probe().await {
        tracing::warn!(
            error = &error as &dyn std::error::Error,
            "ffmpeg is not available: video transcription will be skipped; install ffmpeg or put it on PATH"
        );
    }

    let openrouter = resolve(
        OpenRouter::new(settings.openrouter_api_key, settings.transcribe_model),
        "Failed to build the OpenRouter client",
    )?;
    let telegram = resolve(
        Telegram::connect(settings.telegram_api_token).await,
        "Failed to start the Telegram bot",
    )?;

    let pool = database.pool().clone();

    // One edge per pair of services: the ingestor wakes the transcriber when it stores an
    // attachment, and the transcriber wakes the outbox when it queues a reply. The two handles are
    // never shared, so neither consumer observes the other's work.
    let pending_attachments = Wake::new();
    let pending_replies = Wake::new();

    let ingestor = Ingestor::new(
        telegram.messages(),
        MessageRepository::new(pool.clone()),
        pending_attachments.clone(),
    );
    let transcriber = Transcriber::new(
        telegram.clone(),
        openrouter,
        ffmpeg,
        AttachmentRepository::new(pool.clone()),
        OutboxRepository::new(pool.clone()),
        settings.transcribe_chats,
        pending_attachments,
        pending_replies.clone(),
    );
    let outbox = Outbox::new(telegram, OutboxRepository::new(pool), pending_replies);

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

/// Opens the database at `path` and applies the embedded schema.
async fn init_database(path: &Path) -> Result<database::Database> {
    let database = database::Database::connect(path).await?;
    database.apply_schema().await?;
    tracing::info!("Using database {}", path.display());

    Ok(database)
}

/// Logs a startup failure and turns it into the exit code `start` propagates.
fn resolve<T>(result: Result<T>, message: &'static str) -> Result<T, ExitCode> {
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
