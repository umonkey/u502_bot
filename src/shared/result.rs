//! The crate-wide error type.
//!
//! One [`Error`] enum so every failure mode the program acts on is discoverable from this one
//! file, one [`Result`] alias so every fallible signature reads the same, one variant per failure
//! mode, and one `Display` implementation that names what failed while
//! [`Error::source`](std::error::Error::source) chains the cause. The enum is flat rather than
//! nested per module so a caller never has to import a second type to match a failure; the comment
//! above each group names the area.
//!
//! A failure of an external client is carried by the one [`Error::External`] variant, whose
//! `operation` says what was being done and whose `source` keeps the cause, so this file names no
//! integration crate. Each `From` conversion lives in the module that owns the client producing it.
//! Configuration failures carry the [`VariableName`] that failed and render its key.

use std::fmt;
use std::io;
use std::path::PathBuf;

use crate::shared::config::VariableName;

/// Every result in this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// What an external client was being asked to do when it failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// A database statement or connection.
    Database,
    /// A Telegram Bot API request.
    TelegramRequest,
    /// A Telegram file download.
    TelegramDownload,
    /// A request to OpenRouter.
    OpenRouterRequest,
    /// Decoding the body OpenRouter answered with.
    OpenRouterResponse,
}

impl fmt::Display for Operation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Database => "database operation failed",
            Self::TelegramRequest => "Telegram request failed",
            Self::TelegramDownload => "Telegram download failed",
            Self::OpenRouterRequest => "OpenRouter request failed",
            Self::OpenRouterResponse => "failed to decode the OpenRouter response",
        })
    }
}

/// Every failure mode the bot can produce.
#[derive(Debug)]
pub enum Error {
    // Configuration
    /// Neither `NAME_FILE` nor `NAME` is set, or the value is blank.
    MissingVariable(VariableName),
    /// The value of `NAME` or `NAME_FILE` is not valid Unicode.
    VariableNotUnicode(VariableName),
    /// A file named by `NAME_FILE` could not be read or is not UTF-8.
    ReadConfiguration { path: PathBuf, source: io::Error },
    /// A configured value that must not be blank is blank.
    EmptyConfiguration(VariableName),
    /// One `TRANSCRIBE_CHATS` entry is not a numeric chat id.
    InvalidChatId(String),

    // External clients
    /// A call failed; `operation` names what was being done, and `source` keeps the cause.
    External {
        operation: Operation,
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    // Database
    /// The directory that should hold the database file could not be created.
    CreateDatabaseDirectory { path: PathBuf, source: io::Error },

    // Telegram
    /// A local destination file could not be created.
    WriteFile { path: PathBuf, source: io::Error },
    /// A stored message id does not fit in Telegram's 32-bit message id.
    MessageIdOutOfRange(i64),

    // OpenRouter
    /// An HTTP request returned a non-success status.
    HttpStatus { status: u16, body: String },
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
                    "{name} is not set: neither {} nor {name} exists",
                    name.file_key()
                )
            }
            Self::VariableNotUnicode(name) => {
                write!(formatter, "{name} is not valid Unicode")
            }
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
            Self::External { operation, source } => write!(formatter, "{operation}: {source}"),
            Self::CreateDatabaseDirectory { path, source } => {
                write!(
                    formatter,
                    "failed to create database directory {}: {source}",
                    path.display()
                )
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
            Self::HttpStatus { status, body } => {
                write!(
                    formatter,
                    "HTTP request failed with status {status}: {body}"
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
            Self::External { source, .. } => Some(&**source),
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn missing_variable_names_both_forms() {
        let error = Error::MissingVariable(VariableName::DatabasePath);

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
        let error = Error::External {
            operation: Operation::Database,
            source: Box::new(io::Error::other("no rows returned")),
        };

        assert_eq!(
            error.to_string(),
            "database operation failed: no rows returned"
        );
    }

    #[test]
    fn wrapped_sources_are_exposed() {
        let wrapped = Error::External {
            operation: Operation::Database,
            source: Box::new(io::Error::other("no rows returned")),
        };
        let plain = Error::MissingVariable(VariableName::DatabasePath);

        assert!(std::error::Error::source(&wrapped).is_some());
        assert!(std::error::Error::source(&plain).is_none());
    }

    #[test]
    fn each_operation_renders_a_distinct_message() {
        let operations = [
            Operation::Database,
            Operation::TelegramRequest,
            Operation::TelegramDownload,
            Operation::OpenRouterRequest,
            Operation::OpenRouterResponse,
        ];
        let messages: HashSet<String> = operations
            .iter()
            .map(std::string::ToString::to_string)
            .collect();

        assert_eq!(messages.len(), operations.len());
        assert!(messages.iter().all(|message| !message.is_empty()));
    }
}
