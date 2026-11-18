//! Error reporting, application logs, and metrics through Sentry.
//!
//! [`init`] installs the global `tracing` subscriber and, when the caller supplies a
//! [`SentryDsn::Configured`] destination that parses, a Sentry client. [`SentryDsn::Disabled`] only
//! logs the reason it carries: observability must never keep the bot from running. Every `ERROR`
//! event and every panic becomes an issue, and every `INFO`-or-higher event from this crate becomes
//! a structured log, so those logs include the message content the command layer records.
//!
//! The metric functions in this module are the only place metrics are recorded. Their names come
//! from [`MetricName`], so the complete vocabulary lives in one place. They carry no attributes: a
//! counter or distribution name alone identifies what is measured, so an operator reads volume and
//! cost without message content, chat ids, or file names leaving the host.

use std::time::Duration;

use sentry::protocol::Unit;
use tracing_subscriber::filter::{FilterExt, LevelFilter, filter_fn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, filter::EnvFilter};

use crate::shared::config::{SentryDsn, VariableName};
use crate::shared::metrics::MetricName;

/// Log level forwarded to Sentry. Fixed so `RUST_LOG` stays a console-only control.
const SENTRY_LOG_LEVEL: LevelFilter = LevelFilter::INFO;

/// Installs logging and optional Sentry reporting.
///
/// The returned guard must be bound for the lifetime of the process; dropping it flushes queued
/// events. `None` means Sentry is disabled.
pub fn init(dsn: &SentryDsn) -> Option<sentry::ClientInitGuard> {
    let (guard, disabled) = match dsn {
        SentryDsn::Configured(raw) => match parse_dsn(raw) {
            Ok(parsed) => {
                let mut options = sentry::ClientOptions::new();
                options.dsn = Some(parsed);
                options.release = sentry::release_name!();
                (Some(sentry::init(options)), None)
            }
            Err(error) => (
                None,
                Some(format!(
                    "{} is not a valid DSN: {error}",
                    VariableName::SentryDsn
                )),
            ),
        },
        SentryDsn::Disabled(reason) => (None, Some(reason.clone())),
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

    match disabled {
        Some(reason) => tracing::warn!(reason, "Sentry reporting is disabled"),
        None => tracing::info!("Sentry reporting is enabled"),
    }

    guard
}

/// Captures one increment of a counter metric.
fn count(metric: MetricName) {
    sentry::metrics::counter(metric.as_str(), 1).capture();
}

/// Captures a counter increment of more than one.
fn count_rows(metric: MetricName, rows: u64) {
    sentry::metrics::counter(metric.as_str(), rows as f64).capture();
}

/// Records one received Telegram message.
pub fn message_received() {
    count(MetricName::TelegramMessagesReceived);
}

/// Records one Telegram message whose transaction committed.
///
/// A duplicate delivery counts too: the row is already durable. A transaction rolled back over an
/// attachment failure counts for neither this counter nor [`attachment_stored`], so the gap between
/// the received and stored counters is exactly the messages whose transaction was rolled back.
pub fn message_stored() {
    count(MetricName::TelegramMessagesStored);
}

/// Records one received audio (voice or audio) attachment.
pub fn attachment_audio() {
    count(MetricName::TelegramAttachmentsAudio);
}

/// Records one received video (video or video note) attachment.
pub fn attachment_video() {
    count(MetricName::TelegramAttachmentsVideo);
}

/// Records one attachment whose transaction committed.
///
/// A duplicate delivery counts too: the row is already durable. A transaction rolled back over an
/// attachment failure counts for neither this counter nor [`message_stored`], so the gap between
/// the received and stored counters is exactly the attachments whose transaction was rolled back.
pub fn attachment_stored() {
    count(MetricName::TelegramAttachmentsStored);
}

/// Records one audio attachment the transcriber finished with.
///
/// The `transcribed` flag means "processed", so an attachment skipped because its chat is not
/// white-listed counts too.
pub fn attachment_transcribed() {
    count(MetricName::TelegramAttachmentsTranscribed);
}

/// Records one message queued in the outbox.
pub fn outbox_added() {
    count(MetricName::OutboxMessagesAdded);
}

/// Records one outbox message Telegram accepted.
pub fn outbox_sent() {
    count(MetricName::OutboxMessagesSent);
}

/// Records how many delivered outbox rows one pruning statement deleted.
pub fn outbox_pruned(rows: u64) {
    count_rows(MetricName::OutboxMessagesPruned, rows);
}

/// Records how many messages one pruning statement deleted.
pub fn messages_pruned(rows: u64) {
    count_rows(MetricName::TelegramMessagesPruned, rows);
}

/// Records how many attachments one pruning statement deleted.
pub fn attachments_pruned(rows: u64) {
    count_rows(MetricName::TelegramAttachmentsPruned, rows);
}

/// Records the delay between the source message Telegram saw and the reply Telegram accepted.
pub fn transcription_reply_latency(seconds: f64) {
    sentry::metrics::distribution(MetricName::TranscriptionReplyLatency.as_str(), seconds)
        .unit(Unit::Second)
        .capture();
}

/// Records one OpenRouter call, successful or not, and how long it took.
pub fn llm_call(duration: Duration) {
    count(MetricName::OpenRouterCalls);
    sentry::metrics::distribution(
        MetricName::OpenRouterDuration.as_str(),
        duration.as_secs_f64() * 1000.0,
    )
    .unit(Unit::Millisecond)
    .capture();
}

/// Records the tokens and cost OpenRouter reported for one call.
///
/// `cost_micro_usd` is already in micro-USD: OpenRouter reports USD credits, so the caller
/// converts before calling this.
pub fn llm_usage(input_tokens: u64, output_tokens: u64, cost_micro_usd: f64) {
    sentry::metrics::distribution(
        MetricName::OpenRouterInputTokens.as_str(),
        input_tokens as f64,
    )
    .capture();
    sentry::metrics::distribution(
        MetricName::OpenRouterOutputTokens.as_str(),
        output_tokens as f64,
    )
    .capture();
    sentry::metrics::distribution(MetricName::OpenRouterCostMicroUsd.as_str(), cost_micro_usd)
        .capture();
}

/// Records one ffmpeg call, successful or not, and how long it took.
///
/// The stopwatch wraps the whole extraction, so a timed-out child process still counts.
pub fn ffmpeg_call(duration: Duration) {
    count(MetricName::FfmpegCalls);
    sentry::metrics::distribution(
        MetricName::FfmpegDuration.as_str(),
        duration.as_secs_f64() * 1000.0,
    )
    .unit(Unit::Millisecond)
    .capture();
}

/// Parses a raw DSN.
///
/// A blank value never arrives from the resolver: it counts as unset, so the caller receives
/// [`SentryDsn::Disabled`] instead. A blank string still fails to parse.
fn parse_dsn(raw: &str) -> Result<sentry::types::Dsn, sentry::types::ParseDsnError> {
    raw.parse()
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
    fn blank_dsn_is_not_a_valid_dsn() {
        // Opting out happens one layer up: the resolver reports a blank value as unset, so it
        // arrives here as `SentryDsn::Disabled`. A blank string still fails to parse.
        for raw in ["", "  \n"] {
            assert!(parse_dsn(raw).is_err(), "raw {raw:?}");
        }
    }

    #[test]
    fn valid_dsn_is_parsed() {
        let dsn = parse_dsn("https://public@example.com/1").expect("valid DSN should parse");
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
        transcription_reply_latency(1.5);
    }

    #[test]
    fn retention_metrics_are_safe_without_client() {
        outbox_pruned(7);
        messages_pruned(11);
        attachments_pruned(13);
    }

    #[test]
    fn llm_metrics_are_safe_without_client() {
        llm_call(Duration::from_millis(1_234));
        llm_usage(1_200, 42, 123.0);
    }

    #[test]
    fn ffmpeg_metrics_are_safe_without_client() {
        ffmpeg_call(Duration::from_millis(432));
    }
}
