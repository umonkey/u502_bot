//! The environment variable names this crate reads.
//!
//! [`VariableName`] names every environment variable the bot consumes, so the complete set is
//! discoverable from one enum. The names live here because they are pure vocabulary: resolution,
//! the `<NAME>_FILE` precedence, the trimming, and the file read, stay in `crate::infra::config`,
//! which this module never imports. Each variant renders to the environment key it resolves to, and
//! what a value means stays with the resolver, not with the name.

use std::fmt;

/// Names of the environment variables this crate reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariableName {
    /// `TELEGRAM_API_TOKEN`: the bot token BotFather issued; required.
    TelegramApiToken,
    /// `SENTRY_DSN`: the Sentry DSN; optional, and absent or blank disables reporting.
    SentryDsn,
    /// `OPENROUTER_API_KEY`: the OpenRouter key; required.
    OpenRouterApiKey,
    /// `DATABASE_PATH`: the path of the SQLite database file; required.
    DatabasePath,
    /// `TRANSCRIBE_CHATS`: the comma-separated chat ids to transcribe; optional.
    TranscribeChats,
    /// `TRANSCRIBE_MODEL_NAME`: the transcription model; optional, with a compiled-in default.
    TranscribeModelName,
    /// `FFMPEG_PATH`: the ffmpeg binary; optional, with a `PATH` lookup as the default.
    FfmpegPath,
}

impl VariableName {
    /// The environment key this name resolves to.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TelegramApiToken => "TELEGRAM_API_TOKEN",
            Self::SentryDsn => "SENTRY_DSN",
            Self::OpenRouterApiKey => "OPENROUTER_API_KEY",
            Self::DatabasePath => "DATABASE_PATH",
            Self::TranscribeChats => "TRANSCRIBE_CHATS",
            Self::TranscribeModelName => "TRANSCRIBE_MODEL_NAME",
            Self::FfmpegPath => "FFMPEG_PATH",
        }
    }
}

impl fmt::Display for VariableName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn variable_names_render_their_environment_keys() {
        let cases = [
            (VariableName::TelegramApiToken, "TELEGRAM_API_TOKEN"),
            (VariableName::SentryDsn, "SENTRY_DSN"),
            (VariableName::OpenRouterApiKey, "OPENROUTER_API_KEY"),
            (VariableName::DatabasePath, "DATABASE_PATH"),
            (VariableName::TranscribeChats, "TRANSCRIBE_CHATS"),
            (VariableName::TranscribeModelName, "TRANSCRIBE_MODEL_NAME"),
            (VariableName::FfmpegPath, "FFMPEG_PATH"),
        ];

        for (name, key) in cases {
            assert_eq!(name.as_str(), key, "name {name:?}");
            assert_eq!(name.to_string(), key, "name {name:?}");
        }

        let unique: HashSet<&str> = cases.iter().map(|(_, key)| *key).collect();
        assert_eq!(
            unique.len(),
            cases.len(),
            "environment keys should be unique"
        );
    }
}
