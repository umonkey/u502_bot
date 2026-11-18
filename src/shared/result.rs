//! The crate-wide error type.
//!
//! One [`Error`] enum so every failure mode the bot can produce is discoverable from this one
//! file, one [`Result`] alias so every fallible signature reads the same, one variant per failure
//! mode, and one `Display` implementation that names what failed while [`Error::source`] chains
//! the cause. The enum is flat rather than nested per module so a caller never has to import a
//! second type to match a failure; the comment above each group names the area.

use std::fmt;
use std::io;
use std::path::PathBuf;

use crate::infra::config::VariableName;

/// Every result in this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Every failure mode the bot can produce.
#[derive(Debug)]
pub enum Error {
    // Configuration
    /// Neither `NAME_FILE` nor `NAME` is set, or the value is blank.
    MissingVariable(String),
    /// A variable, or the name in `NAME_FILE`, is not valid Unicode.
    VariableNotUnicode(String),
    /// A file named by `NAME_FILE` could not be read or is not UTF-8.
    ReadConfiguration { path: PathBuf, source: io::Error },
    /// A configured value that must not be blank is blank.
    EmptyConfiguration(VariableName),
    /// One `TRANSCRIBE_CHATS` entry is not a numeric chat id.
    InvalidChatId(String),

    // Database
    /// SQLite rejected a statement or a connection setting.
    Database(sqlx::Error),
    /// The directory that should hold the database file could not be created.
    CreateDatabaseDirectory { path: PathBuf, source: io::Error },

    // Telegram
    /// Telegram rejected an API call.
    TelegramRequest(teloxide::RequestError),
    /// Telegram accepted the call but the file transfer failed.
    TelegramDownload(teloxide::DownloadError),
    /// A local destination file could not be created.
    WriteFile { path: PathBuf, source: io::Error },
    /// A stored message id does not fit in Telegram's 32-bit message id.
    MessageIdOutOfRange(i64),

    // OpenRouter
    /// The request could not be sent, or its body could not be read.
    HttpRequest(reqwest::Error),
    /// OpenRouter answered with a non-success status.
    HttpStatus { status: u16, body: String },
    /// The response body is not the JSON shape expected.
    ResponseJson(serde_json::Error),
    /// The audio file could not be read.
    ReadAudioFile { path: PathBuf, source: io::Error },

    // ffmpeg
    /// The binary is missing or not on `PATH`.
    FfmpegUnavailable(io::Error),
    /// ffmpeg exited with a non-success status.
    FfmpegStatus { code: Option<i32>, stderr: String },
    /// The child process could not be started or waited for.
    FfmpegIo(io::Error),
    /// The input carries no audio stream.
    NoAudioStream { stderr: String },
    /// The extraction did not finish within the configured timeout.
    FfmpegTimeout { seconds: u64 },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingVariable(name) => {
                write!(
                    formatter,
                    "{name} is not set: neither {name}_FILE nor {name} exists"
                )
            }
            Self::VariableNotUnicode(name) => write!(formatter, "{name} is not valid Unicode"),
            Self::ReadConfiguration { path, source } => {
                write!(formatter, "failed to read {}: {source}", path.display())
            }
            Self::EmptyConfiguration(name) => {
                write!(formatter, "{name} is empty or only whitespace")
            }
            Self::InvalidChatId(token) => write!(
                formatter,
                "{} entry {token:?} is not a numeric chat id",
                VariableName::TranscribeChats
            ),
            Self::Database(source) => write!(formatter, "database error: {source}"),
            Self::CreateDatabaseDirectory { path, source } => {
                write!(
                    formatter,
                    "failed to create database directory {}: {source}",
                    path.display()
                )
            }
            Self::TelegramRequest(source) => write!(formatter, "telegram request error: {source}"),
            Self::TelegramDownload(source) => {
                write!(formatter, "telegram download error: {source}")
            }
            Self::WriteFile { path, source } => {
                write!(formatter, "failed to write {}: {source}", path.display())
            }
            Self::MessageIdOutOfRange(id) => {
                write!(
                    formatter,
                    "message id {id} does not fit in a Telegram message id"
                )
            }
            Self::HttpRequest(source) => write!(formatter, "OpenRouter request failed: {source}"),
            Self::HttpStatus { status, body } => {
                write!(formatter, "OpenRouter returned HTTP {status}: {body}")
            }
            Self::ResponseJson(source) => {
                write!(
                    formatter,
                    "failed to decode the OpenRouter response: {source}"
                )
            }
            Self::ReadAudioFile { path, source } => {
                write!(
                    formatter,
                    "failed to read audio file {}: {source}",
                    path.display()
                )
            }
            Self::FfmpegUnavailable(source) => {
                write!(formatter, "ffmpeg is not available: {source}")
            }
            Self::FfmpegStatus {
                code: Some(code),
                stderr,
            } => write!(formatter, "ffmpeg exited with status {code}: {stderr}"),
            Self::FfmpegStatus { code: None, stderr } => {
                write!(formatter, "ffmpeg was terminated by a signal: {stderr}")
            }
            Self::FfmpegIo(source) => write!(formatter, "failed to run ffmpeg: {source}"),
            Self::NoAudioStream { stderr } => {
                write!(formatter, "the input carries no audio stream: {stderr}")
            }
            Self::FfmpegTimeout { seconds } => {
                write!(formatter, "ffmpeg did not finish within {seconds} seconds")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadConfiguration { source, .. }
            | Self::CreateDatabaseDirectory { source, .. }
            | Self::WriteFile { source, .. }
            | Self::ReadAudioFile { source, .. } => Some(source),
            Self::Database(source) => Some(source),
            Self::TelegramRequest(source) => Some(source),
            Self::TelegramDownload(source) => Some(source),
            Self::HttpRequest(source) => Some(source),
            Self::ResponseJson(source) => Some(source),
            Self::FfmpegUnavailable(source) | Self::FfmpegIo(source) => Some(source),
            Self::MissingVariable(_)
            | Self::VariableNotUnicode(_)
            | Self::EmptyConfiguration(_)
            | Self::InvalidChatId(_)
            | Self::MessageIdOutOfRange(_)
            | Self::HttpStatus { .. }
            | Self::FfmpegStatus { .. }
            | Self::NoAudioStream { .. }
            | Self::FfmpegTimeout { .. } => None,
        }
    }
}

impl From<sqlx::Error> for Error {
    fn from(source: sqlx::Error) -> Self {
        Self::Database(source)
    }
}

impl From<teloxide::RequestError> for Error {
    fn from(source: teloxide::RequestError) -> Self {
        Self::TelegramRequest(source)
    }
}

impl From<teloxide::DownloadError> for Error {
    fn from(source: teloxide::DownloadError) -> Self {
        Self::TelegramDownload(source)
    }
}

impl From<reqwest::Error> for Error {
    fn from(source: reqwest::Error) -> Self {
        Self::HttpRequest(source)
    }
}

impl From<serde_json::Error> for Error {
    fn from(source: serde_json::Error) -> Self {
        Self::ResponseJson(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_variable_names_both_forms() {
        let error = Error::MissingVariable(VariableName::DatabasePath.as_str().to_owned());

        assert_eq!(
            error.to_string(),
            "DATABASE_PATH is not set: neither DATABASE_PATH_FILE nor DATABASE_PATH exists"
        );
    }

    #[test]
    fn invalid_chat_id_names_the_variable() {
        let error = Error::InvalidChatId("not-a-number".to_owned());

        assert_eq!(
            error.to_string(),
            "TRANSCRIBE_CHATS entry \"not-a-number\" is not a numeric chat id"
        );
    }

    #[test]
    fn database_error_names_the_cause() {
        let error = Error::Database(sqlx::Error::RowNotFound);
        let rendered = error.to_string();

        assert!(rendered.starts_with("database error: "), "{rendered}");
        assert!(rendered.contains("no rows returned"), "{rendered}");
    }

    #[test]
    fn wrapped_sources_are_exposed() {
        let wrapped = Error::Database(sqlx::Error::RowNotFound);
        let plain = Error::MissingVariable(VariableName::DatabasePath.as_str().to_owned());

        assert!(std::error::Error::source(&wrapped).is_some());
        assert!(std::error::Error::source(&plain).is_none());
    }
}
