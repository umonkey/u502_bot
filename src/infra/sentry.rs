//! Error reporting, application logs, and metrics through Sentry.
//!
//! [`init`] installs the global `tracing` subscriber and, when `SENTRY_DSN` resolves to a valid
//! DSN, a Sentry client. A missing, empty, or invalid DSN only logs a warning: observability must
//! never keep the bot from running. Every `ERROR` event and every panic becomes an issue, and every
//! `INFO`-or-higher event from this crate becomes a structured log, so those logs include the
//! message content the command layer records.
//!
//! The metric functions in this module are the only place metrics are recorded. They carry no
//! attributes: a counter or distribution name alone identifies what is measured, so an operator
//! reads volume and cost without message content, chat ids, or file names leaving the host.

use std::time::Duration;

use sentry::protocol::Unit;
use tracing_subscriber::filter::{FilterExt, LevelFilter, filter_fn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, filter::EnvFilter};

use crate::infra::config::get_secret;

/// Environment variable holding the Sentry DSN.
pub const SENTRY_DSN: &str = "SENTRY_DSN";

/// Log level forwarded to Sentry. Fixed so `RUST_LOG` stays a console-only control.
const SENTRY_LOG_LEVEL: LevelFilter = LevelFilter::INFO;

/// Application metric counting every incoming Telegram message.
pub const TELEGRAM_MESSAGES_RECEIVED: &str = "telegram.messages.received";

/// Application metric counting every message whose storage statement succeeded.
pub const TELEGRAM_MESSAGES_STORED: &str = "telegram.messages.stored";

/// Application metric counting every received audio (voice or audio) attachment.
pub const TELEGRAM_ATTACHMENTS_AUDIO: &str = "telegram.attachments.audio";

/// Application metric counting every received video (video or video note) attachment.
pub const TELEGRAM_ATTACHMENTS_VIDEO: &str = "telegram.attachments.video";

/// Application metric counting every attachment whose storage statement succeeded.
pub const TELEGRAM_ATTACHMENTS_STORED: &str = "telegram.attachments.stored";

/// Application metric counting every audio attachment the transcriber finished with.
pub const TELEGRAM_ATTACHMENTS_TRANSCRIBED: &str = "telegram.attachments.transcribed";

/// Application metric counting every message queued in the outbox.
pub const OUTBOX_MESSAGES_ADDED: &str = "outbox.messages.added";

/// Application metric counting every outbox message Telegram accepted.
pub const OUTBOX_MESSAGES_SENT: &str = "outbox.messages.sent";

/// Application metric counting every OpenRouter call, successful or not.
pub const OPENROUTER_CALLS: &str = "openrouter.calls";

/// Application metric measuring every OpenRouter call, in milliseconds.
pub const OPENROUTER_DURATION: &str = "openrouter.duration";

/// Application metric recording the prompt tokens OpenRouter reported for a call.
pub const OPENROUTER_INPUT_TOKENS: &str = "openrouter.tokens.input";

/// Application metric recording the completion tokens OpenRouter reported for a call.
pub const OPENROUTER_OUTPUT_TOKENS: &str = "openrouter.tokens.output";

/// Application metric recording what a call cost, in micro-USD.
pub const OPENROUTER_COST_MICRO_USD: &str = "openrouter.cost_micro_usd";

/// Installs logging and optional Sentry reporting.
///
/// The returned guard must be bound for the lifetime of the process; dropping it flushes queued
/// events. `None` means Sentry is disabled.
pub fn init() -> Option<sentry::ClientInitGuard> {
    let resolved = resolve_dsn();
    let guard = match &resolved {
        Ok(dsn) => {
            let mut options = sentry::ClientOptions::new();
            options.dsn = Some(dsn.clone());
            options.release = sentry::release_name!();
            Some(sentry::init(options))
        }
        Err(_) => None,
    };

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    let log_filter = SENTRY_LOG_LEVEL.and(filter_fn(|metadata: &tracing::Metadata<'_>| {
        is_application_target(metadata.target())
    }));
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(env_filter))
        .with(
            guard
                .as_ref()
                .map(|_| sentry::integrations::tracing::LogLayer::new().with_filter(log_filter)),
        )
        .with(guard.as_ref().map(|_| {
            sentry::integrations::tracing::ErrorLayer::new().with_filter(LevelFilter::ERROR)
        }))
        .init();

    match resolved {
        Ok(_) => tracing::info!("Sentry reporting is enabled"),
        Err(reason) => tracing::warn!(%reason, "Sentry reporting is disabled"),
    }

    guard
}

/// Records one received Telegram message.
pub fn message_received() {
    sentry::metrics::counter(TELEGRAM_MESSAGES_RECEIVED, 1).capture();
}

/// Records one Telegram message whose storage statement succeeded.
///
/// A duplicate delivery counts too: the row is already durable, so the gap between the received
/// and stored counters is exactly the number of messages that failed to persist.
pub fn message_stored() {
    sentry::metrics::counter(TELEGRAM_MESSAGES_STORED, 1).capture();
}

/// Records one received audio (voice or audio) attachment.
pub fn attachment_audio() {
    sentry::metrics::counter(TELEGRAM_ATTACHMENTS_AUDIO, 1).capture();
}

/// Records one received video (video or video note) attachment.
pub fn attachment_video() {
    sentry::metrics::counter(TELEGRAM_ATTACHMENTS_VIDEO, 1).capture();
}

/// Records one attachment whose storage statement succeeded.
///
/// A duplicate delivery counts too: the row is already durable, so the gap between the received
/// and stored counters is exactly the number of attachments that failed to persist.
pub fn attachment_stored() {
    sentry::metrics::counter(TELEGRAM_ATTACHMENTS_STORED, 1).capture();
}

/// Records one audio attachment the transcriber finished with.
///
/// The `transcribed` flag means "processed", so an attachment skipped because its chat is not
/// white-listed counts too.
pub fn attachment_transcribed() {
    sentry::metrics::counter(TELEGRAM_ATTACHMENTS_TRANSCRIBED, 1).capture();
}

/// Records one message queued in the outbox.
pub fn outbox_added() {
    sentry::metrics::counter(OUTBOX_MESSAGES_ADDED, 1).capture();
}

/// Records one outbox message Telegram accepted.
pub fn outbox_sent() {
    sentry::metrics::counter(OUTBOX_MESSAGES_SENT, 1).capture();
}

/// Records one OpenRouter call, successful or not, and how long it took.
pub fn llm_call(duration: Duration) {
    sentry::metrics::counter(OPENROUTER_CALLS, 1).capture();
    sentry::metrics::distribution(OPENROUTER_DURATION, duration.as_secs_f64() * 1000.0)
        .unit(Unit::Millisecond)
        .capture();
}

/// Records the tokens and cost OpenRouter reported for one call.
///
/// `cost_micro_usd` is already in micro-USD: OpenRouter reports USD credits, so the caller
/// converts before calling this.
pub fn llm_usage(input_tokens: u64, output_tokens: u64, cost_micro_usd: f64) {
    sentry::metrics::distribution(OPENROUTER_INPUT_TOKENS, input_tokens as f64).capture();
    sentry::metrics::distribution(OPENROUTER_OUTPUT_TOKENS, output_tokens as f64).capture();
    sentry::metrics::distribution(OPENROUTER_COST_MICRO_USD, cost_micro_usd).capture();
}

/// Reads the DSN, mapping every failure to a reason to disable reporting.
fn resolve_dsn() -> Result<sentry::types::Dsn, String> {
    let raw =
        get_secret(SENTRY_DSN).map_err(|error| format!("{SENTRY_DSN} is not set: {error}"))?;
    match parse_dsn(&raw) {
        Ok(Some(dsn)) => Ok(dsn),
        Ok(None) => Err(format!("{SENTRY_DSN} is empty")),
        Err(error) => Err(format!("{SENTRY_DSN} is not a valid DSN: {error}")),
    }
}

/// Parses a raw DSN, treating blank values as "not configured".
fn parse_dsn(raw: &str) -> Result<Option<sentry::types::Dsn>, sentry::types::ParseDsnError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed.parse().map(Some)
}

/// Returns whether `target` belongs to this crate, so only its events are reported as logs.
///
/// The prefix is the binary target's crate name, not the package name: the manifest names the
/// binary `u502`, so its module paths are `u502::…`.
fn is_application_target(target: &str) -> bool {
    match target.strip_prefix(env!("CARGO_CRATE_NAME")) {
        Some(rest) => rest.is_empty() || rest.starts_with("::"),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_targets_are_reported() {
        let application = env!("CARGO_CRATE_NAME");
        assert!(is_application_target(application));
        assert!(is_application_target(&format!("{application}::cli::start")));
    }

    #[test]
    fn foreign_targets_are_not_reported() {
        let application = env!("CARGO_CRATE_NAME");
        assert!(!is_application_target(&format!("{application}_extra")));
        assert!(!is_application_target("teloxide::dispatching"));
        assert!(!is_application_target(""));
    }

    #[test]
    fn blank_dsn_is_treated_as_not_configured() {
        for raw in ["", "  \n"] {
            assert!(matches!(parse_dsn(raw), Ok(None)));
        }
    }

    #[test]
    fn valid_dsn_is_parsed() {
        let dsn = parse_dsn("https://public@example.com/1")
            .expect("valid DSN should parse")
            .expect("valid DSN should be configured");
        assert_eq!(dsn.host(), "example.com");
    }

    #[test]
    fn invalid_dsn_is_rejected() {
        assert!(parse_dsn("not-a-dsn").is_err());
    }

    #[test]
    fn message_received_is_safe_without_client() {
        message_received();
    }

    #[test]
    fn stored_metric_is_safe_without_client() {
        message_stored();
    }

    #[test]
    fn attachment_metrics_are_safe_without_client() {
        attachment_audio();
        attachment_video();
        attachment_stored();
        attachment_transcribed();
    }

    #[test]
    fn outbox_metrics_are_safe_without_client() {
        outbox_added();
        outbox_sent();
    }

    #[test]
    fn llm_metrics_are_safe_without_client() {
        llm_call(Duration::from_millis(1_234));
        llm_usage(1_200, 42, 123.0);
    }
}
