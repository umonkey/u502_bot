//! The application metric names this crate reports.
//!
//! [`MetricName`] names every metric the bot records, so the complete vocabulary is discoverable
//! from one enum. The names live here because they are pure vocabulary: the recording functions,
//! and the Sentry client they call, stay in `crate::infra::sentry`, which this module never
//! imports. Each variant renders to the exact wire name Sentry stores, and the kind and unit of a
//! metric stay with the recording function, not with the name.

use std::fmt;

/// Names of the application metrics this crate records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricName {
    // Telegram
    /// Counting every incoming Telegram message.
    TelegramMessagesReceived,
    /// Counting every message whose storage statement succeeded.
    TelegramMessagesStored,
    /// Counting every received audio (voice or audio) attachment.
    TelegramAttachmentsAudio,
    /// Counting every received video (video or video note) attachment.
    TelegramAttachmentsVideo,
    /// Counting every attachment whose storage statement succeeded.
    TelegramAttachmentsStored,
    /// Counting every attachment the transcriber finished with.
    TelegramAttachmentsTranscribed,
    /// Counting every message pruning deleted.
    TelegramMessagesPruned,
    /// Counting every attachment pruning deleted.
    TelegramAttachmentsPruned,

    // Outbox
    /// Counting every message queued in the outbox.
    OutboxMessagesAdded,
    /// Counting every outbox message Telegram accepted.
    OutboxMessagesSent,
    /// Counting every delivered outbox row pruning deleted.
    OutboxMessagesPruned,

    // Transcription
    /// Measuring the delay between a source message and its reply, in seconds.
    TranscriptionReplyLatency,

    // OpenRouter
    /// Counting every OpenRouter call, successful or not.
    OpenRouterCalls,
    /// Measuring every OpenRouter call, in milliseconds.
    OpenRouterDuration,
    /// Recording the prompt tokens OpenRouter reported for a call.
    OpenRouterInputTokens,
    /// Recording the completion tokens OpenRouter reported for a call.
    OpenRouterOutputTokens,
    /// Recording what a call cost, in micro-USD.
    OpenRouterCostMicroUsd,

    // ffmpeg
    /// Counting every ffmpeg call, successful or not.
    FfmpegCalls,
    /// Measuring every ffmpeg call, in milliseconds.
    FfmpegDuration,
}

impl MetricName {
    /// The Sentry wire name this metric is recorded under.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TelegramMessagesReceived => "telegram.messages.received",
            Self::TelegramMessagesStored => "telegram.messages.stored",
            Self::TelegramAttachmentsAudio => "telegram.attachments.audio",
            Self::TelegramAttachmentsVideo => "telegram.attachments.video",
            Self::TelegramAttachmentsStored => "telegram.attachments.stored",
            Self::TelegramAttachmentsTranscribed => "telegram.attachments.transcribed",
            Self::TelegramMessagesPruned => "telegram.messages.pruned",
            Self::TelegramAttachmentsPruned => "telegram.attachments.pruned",
            Self::OutboxMessagesAdded => "outbox.messages.added",
            Self::OutboxMessagesSent => "outbox.messages.sent",
            Self::OutboxMessagesPruned => "outbox.messages.pruned",
            Self::TranscriptionReplyLatency => "transcription.reply.latency",
            Self::OpenRouterCalls => "openrouter.calls",
            Self::OpenRouterDuration => "openrouter.duration",
            Self::OpenRouterInputTokens => "openrouter.tokens.input",
            Self::OpenRouterOutputTokens => "openrouter.tokens.output",
            Self::OpenRouterCostMicroUsd => "openrouter.cost_micro_usd",
            Self::FfmpegCalls => "ffmpeg.calls",
            Self::FfmpegDuration => "ffmpeg.duration",
        }
    }
}

impl fmt::Display for MetricName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn metric_names_render_their_sentry_names() {
        let cases = [
            (
                MetricName::TelegramMessagesReceived,
                "telegram.messages.received",
            ),
            (
                MetricName::TelegramMessagesStored,
                "telegram.messages.stored",
            ),
            (
                MetricName::TelegramAttachmentsAudio,
                "telegram.attachments.audio",
            ),
            (
                MetricName::TelegramAttachmentsVideo,
                "telegram.attachments.video",
            ),
            (
                MetricName::TelegramAttachmentsStored,
                "telegram.attachments.stored",
            ),
            (
                MetricName::TelegramAttachmentsTranscribed,
                "telegram.attachments.transcribed",
            ),
            (
                MetricName::TelegramMessagesPruned,
                "telegram.messages.pruned",
            ),
            (
                MetricName::TelegramAttachmentsPruned,
                "telegram.attachments.pruned",
            ),
            (MetricName::OutboxMessagesAdded, "outbox.messages.added"),
            (MetricName::OutboxMessagesSent, "outbox.messages.sent"),
            (MetricName::OutboxMessagesPruned, "outbox.messages.pruned"),
            (
                MetricName::TranscriptionReplyLatency,
                "transcription.reply.latency",
            ),
            (MetricName::OpenRouterCalls, "openrouter.calls"),
            (MetricName::OpenRouterDuration, "openrouter.duration"),
            (MetricName::OpenRouterInputTokens, "openrouter.tokens.input"),
            (
                MetricName::OpenRouterOutputTokens,
                "openrouter.tokens.output",
            ),
            (
                MetricName::OpenRouterCostMicroUsd,
                "openrouter.cost_micro_usd",
            ),
            (MetricName::FfmpegCalls, "ffmpeg.calls"),
            (MetricName::FfmpegDuration, "ffmpeg.duration"),
        ];

        for (name, wire) in cases {
            assert_eq!(name.as_str(), wire, "name {name:?}");
            assert_eq!(name.to_string(), wire, "name {name:?}");
        }

        let unique: HashSet<&str> = cases.iter().map(|(_, wire)| *wire).collect();
        assert_eq!(unique.len(), cases.len(), "wire names should be unique");
    }
}
