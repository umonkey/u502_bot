//! Error reporting and application metrics through Sentry.
//!
//! [`init`] installs the global `tracing` subscriber and, when `SENTRY_DSN` resolves to a valid
//! DSN, a Sentry client. A missing, empty, or invalid DSN only logs a warning: observability must
//! never keep the bot from running. Only `ERROR` events and panics are reported, so chat text
//! logged at `INFO` never leaves the host.

use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, filter::EnvFilter};

use crate::infra::config::get_secret;

/// Environment variable holding the Sentry DSN.
pub const SENTRY_DSN: &str = "SENTRY_DSN";

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
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(env_filter))
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

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
