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
}

/// What the environment said about Sentry: a destination, or the reason reporting is off.
pub enum SentryDsn {
    /// A resolved DSN; the client decides whether it parses.
    Configured(String),
    /// No usable destination, with the reason logged once when reporting is set up.
    Disabled(String),
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
        }
    }

    /// The key that names a file holding this variable's value.
    pub const fn file_key(self) -> &'static str {
        match self {
            Self::TelegramApiToken => "TELEGRAM_API_TOKEN_FILE",
            Self::SentryDsn => "SENTRY_DSN_FILE",
            Self::OpenRouterApiKey => "OPENROUTER_API_KEY_FILE",
            Self::DatabasePath => "DATABASE_PATH_FILE",
            Self::TranscribeChats => "TRANSCRIBE_CHATS_FILE",
            Self::TranscribeModelName => "TRANSCRIBE_MODEL_NAME_FILE",
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

    #[test]
    fn file_keys_are_the_direct_keys_with_the_suffix() {
        let names = [
            VariableName::TelegramApiToken,
            VariableName::SentryDsn,
            VariableName::OpenRouterApiKey,
            VariableName::DatabasePath,
            VariableName::TranscribeChats,
            VariableName::TranscribeModelName,
        ];

        for name in names {
            assert_eq!(
                name.file_key(),
                format!("{}_FILE", name.as_str()),
                "name {name:?}"
            );
        }

        let unique: HashSet<&str> = names.iter().map(|name| name.file_key()).collect();
        assert_eq!(unique.len(), names.len(), "file keys should be unique");
    }

    #[test]
    fn sentry_dsn_carries_the_reason_it_is_disabled() {
        let configured = SentryDsn::Configured("https://public@example.com/1".to_owned());
        assert!(matches!(configured, SentryDsn::Configured(_)));

        match SentryDsn::Disabled("SENTRY_DSN is not set".to_owned()) {
            SentryDsn::Disabled(reason) => assert_eq!(reason, "SENTRY_DSN is not set"),
            SentryDsn::Configured(_) => panic!("expected a disabled destination"),
        }
    }
}
