//! Environment configuration: how the variables this crate reads resolve.
//!
//! The names live in [`crate::shared::config::VariableName`], which this module resolves but never
//! redefines. [`get_optional`] is the only resolver, and every variable takes the same path:
//!
//! 1. `NAME_FILE` is consulted first: when set, its value is the path of a file holding the
//!    configuration, and that file's contents are the value. `NAME` is the fallback.
//! 2. The value is trimmed of surrounding whitespace, so a trailing newline in a file never reaches
//!    a caller, and a whitespace-only value means the variable is unset.
//! 3. Values are UTF-8: a non-Unicode variable is reported as [`Error::VariableNotUnicode`], and an
//!    unreadable file, or one that is not UTF-8, as [`Error::ReadConfiguration`], instead of being
//!    silently treated as absent.
//!
//! `Ok(None)` means the variable is not configured. This module is also the only place the
//! environment is read: [`Settings::load`] resolves everything once at startup, and absence is fatal
//! only for the fields startup cannot run without. `RUST_LOG` is the one exception:
//! `tracing_subscriber`'s `EnvFilter` reads it, not this crate, and it has no file form.

use std::env::VarError;
use std::io;
use std::path::PathBuf;

use crate::shared::config::{SentryDsn, VariableName};
use crate::shared::result::{Error, Result};

/// Model used when [`VariableName::TranscribeModelName`] is unset or empty.
pub const DEFAULT_TRANSCRIBE_MODEL: &str = "google/gemini-3.5-transcribe";

/// Days of rows kept when [`VariableName::RetentionDays`] is unset; a configured `0` disables pruning.
pub const DEFAULT_RETENTION_DAYS: u32 = 30;

/// Everything the bot reads from the environment, resolved and validated once.
///
/// Holds secrets: it deliberately implements no `Debug`, `Display`, or `Clone`.
pub struct Settings {
    pub telegram_api_token: String,
    pub openrouter_api_key: String,
    pub transcribe_model: String,
    pub owner_user_id: i64,
    /// `Some(days)` keeps that many days of rows, `None` keeps every row.
    pub retention_days: Option<u32>,
    pub database_path: PathBuf,
    pub sentry_dsn: SentryDsn,
}

impl Settings {
    /// Resolves every setting; the only place the environment is read.
    pub async fn load() -> Result<Self> {
        Self::from_lookup(get_optional).await
    }

    async fn from_lookup(
        lookup: impl AsyncFn(VariableName) -> Result<Option<String>>,
    ) -> Result<Self> {
        let database_path = require(&lookup, VariableName::DatabasePath).await?.into();
        let owner_user_id = owner_user_id(&require(&lookup, VariableName::OwnerUserId).await?)?;
        Ok(Self {
            telegram_api_token: require(&lookup, VariableName::TelegramApiToken).await?,
            openrouter_api_key: require(&lookup, VariableName::OpenRouterApiKey).await?,
            transcribe_model: lookup(VariableName::TranscribeModelName)
                .await?
                .unwrap_or_else(|| DEFAULT_TRANSCRIBE_MODEL.to_owned()),
            owner_user_id,
            retention_days: match lookup(VariableName::RetentionDays).await? {
                Some(raw) => retention_days(&raw)?,
                None => Some(DEFAULT_RETENTION_DAYS),
            },
            database_path,
            sentry_dsn: classify_dsn(lookup(VariableName::SentryDsn).await),
        })
    }
}

/// Resolves the DSN without failing: ADR 0004 keeps observability from stopping the bot.
pub async fn sentry_dsn() -> SentryDsn {
    classify_dsn(get_optional(VariableName::SentryDsn).await)
}

/// Turns a DSN resolution into a destination or the reason reporting is off.
fn classify_dsn(value: Result<Option<String>>) -> SentryDsn {
    match value {
        Ok(Some(raw)) => SentryDsn::Configured(raw),
        Ok(None) => SentryDsn::Disabled(format!("{} is not set", VariableName::SentryDsn)),
        Err(error) => SentryDsn::Disabled(format!("{}: {error}", VariableName::SentryDsn)),
    }
}

/// Resolves a setting startup cannot run without.
async fn require(
    lookup: &impl AsyncFn(VariableName) -> Result<Option<String>>,
    name: VariableName,
) -> Result<String> {
    lookup(name).await?.ok_or(Error::MissingVariable(name))
}

/// Parses `OWNER_USER_ID`.
///
/// Telegram user ids are positive, so a negative or zero value is a configuration mistake: it would
/// never match a private chat, and the registry could never be managed. A non-numeric value fails for
/// the same reason every other unusable required value does.
fn owner_user_id(raw: &str) -> Result<i64> {
    let id: i64 = raw
        .parse()
        .map_err(|_| Error::InvalidOwnerUserId(raw.to_owned()))?;

    if id <= 0 {
        return Err(Error::InvalidOwnerUserId(raw.to_owned()));
    }

    Ok(id)
}

/// Parses `RETENTION_DAYS`: a whole number of days, `0` meaning keep every row.
fn retention_days(raw: &str) -> Result<Option<u32>> {
    let days: u32 = raw
        .parse()
        .map_err(|_| Error::InvalidRetentionDays(raw.to_owned()))?;

    Ok((days > 0).then_some(days))
}

/// Resolves one environment variable.
///
/// `Ok(None)` means the variable is not configured. `Err` means a value is present but unusable:
/// a non-Unicode variable, or a `NAME_FILE` path that cannot be read or is not UTF-8. See the
/// module documentation for the precedence and the trimming rule.
async fn get_optional(name: VariableName) -> Result<Option<String>> {
    resolve(name, |key| std::env::var(key), read_file).await
}

/// The one algorithm behind [`get_optional`], with the lookups injected so tests need not touch the
/// process environment.
async fn resolve(
    name: VariableName,
    lookup: impl Fn(&str) -> Result<String, VarError>,
    read_file: impl AsyncFn(&str) -> Result<String>,
) -> Result<Option<String>> {
    let file_key = name.file_key();
    let raw = match lookup(file_key) {
        Ok(path) => read_file(&path).await?,
        Err(VarError::NotPresent) => match lookup(name.as_str()) {
            Ok(value) => value,
            Err(VarError::NotPresent) => return Ok(None),
            Err(VarError::NotUnicode(_)) => {
                return Err(Error::VariableNotUnicode(name));
            }
        },
        Err(VarError::NotUnicode(_)) => {
            return Err(Error::VariableNotUnicode(name));
        }
    };

    let trimmed = raw.trim();

    Ok((!trimmed.is_empty()).then(|| trimmed.to_owned()))
}

/// Reads a file and decodes it as UTF-8.
async fn read_file(path: &str) -> Result<String> {
    tokio::fs::read(path)
        .await
        .and_then(|bytes| {
            String::from_utf8(bytes)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not valid UTF-8"))
        })
        .map_err(|source| Error::ReadConfiguration {
            path: PathBuf::from(path),
            source,
        })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    fn required_values() -> Vec<(VariableName, &'static str)> {
        vec![
            (VariableName::TelegramApiToken, "token"),
            (VariableName::OpenRouterApiKey, "key"),
            (VariableName::DatabasePath, "/tmp/u502.sqlite"),
            (VariableName::OwnerUserId, "99"),
        ]
    }

    async fn settings(values: &[(VariableName, &str)]) -> Result<Settings> {
        Settings::from_lookup(async |name| {
            Ok(values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned()))
        })
        .await
    }

    async fn settings_error(values: &[(VariableName, &str)]) -> Error {
        match settings(values).await {
            Ok(_) => panic!("the settings should not load"),
            Err(error) => error,
        }
    }

    #[tokio::test]
    async fn required_values_are_loaded() {
        let mut values = required_values();
        values.extend([(VariableName::TranscribeModelName, "openai/whisper-1")]);

        let settings = settings(&values)
            .await
            .expect("required values should load");

        assert_eq!(settings.telegram_api_token, "token");
        assert_eq!(settings.openrouter_api_key, "key");
        assert_eq!(settings.transcribe_model, "openai/whisper-1");
        assert_eq!(settings.database_path, PathBuf::from("/tmp/u502.sqlite"));
        assert_eq!(settings.owner_user_id, 99);
        assert!(matches!(&settings.sentry_dsn, SentryDsn::Disabled(_)));
    }

    #[tokio::test]
    async fn missing_token_fails() {
        let values = [
            (VariableName::OpenRouterApiKey, "key"),
            (VariableName::DatabasePath, "/tmp/u502.sqlite"),
            (VariableName::OwnerUserId, "99"),
        ];

        assert!(matches!(
            settings_error(&values).await,
            Error::MissingVariable(VariableName::TelegramApiToken)
        ));
    }

    #[tokio::test]
    async fn missing_openrouter_key_fails() {
        let values = [
            (VariableName::TelegramApiToken, "token"),
            (VariableName::DatabasePath, "/tmp/u502.sqlite"),
            (VariableName::OwnerUserId, "99"),
        ];

        assert!(matches!(
            settings_error(&values).await,
            Error::MissingVariable(VariableName::OpenRouterApiKey)
        ));
    }

    #[tokio::test]
    async fn missing_database_path_fails() {
        let values = [
            (VariableName::TelegramApiToken, "token"),
            (VariableName::OpenRouterApiKey, "key"),
        ];

        assert!(matches!(
            settings_error(&values).await,
            Error::MissingVariable(VariableName::DatabasePath)
        ));
    }

    #[tokio::test]
    async fn missing_owner_fails() {
        let values = [
            (VariableName::TelegramApiToken, "token"),
            (VariableName::OpenRouterApiKey, "key"),
            (VariableName::DatabasePath, "/tmp/u502.sqlite"),
        ];

        assert!(matches!(
            settings_error(&values).await,
            Error::MissingVariable(VariableName::OwnerUserId)
        ));
    }

    #[tokio::test]
    async fn non_numeric_owner_fails() {
        let mut values = required_values();
        values.retain(|(name, _)| *name != VariableName::OwnerUserId);
        values.push((VariableName::OwnerUserId, "not-a-number"));

        match settings_error(&values).await {
            Error::InvalidOwnerUserId(value) => assert_eq!(value, "not-a-number"),
            other => panic!("expected an invalid owner id, got {other}"),
        }
    }

    #[tokio::test]
    async fn non_positive_owner_fails() {
        for raw in ["0", "-1"] {
            let mut values = required_values();
            values.retain(|(name, _)| *name != VariableName::OwnerUserId);
            values.push((VariableName::OwnerUserId, raw));

            match settings_error(&values).await {
                Error::InvalidOwnerUserId(value) => assert_eq!(value, raw),
                other => panic!("expected an invalid owner id, got {other}"),
            }
        }
    }

    #[tokio::test]
    async fn an_unusable_optional_value_fails() {
        let error = match Settings::from_lookup(async |name| match name {
            VariableName::TranscribeModelName => Err(Error::VariableNotUnicode(name)),
            VariableName::OwnerUserId => Ok(Some("99".to_owned())),
            _ => Ok(Some("value".to_owned())),
        })
        .await
        {
            Ok(_) => panic!("an unusable optional value should fail"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            Error::VariableNotUnicode(VariableName::TranscribeModelName)
        ));
    }

    #[tokio::test]
    async fn unset_model_resolves_to_the_default() {
        let settings = settings(&required_values())
            .await
            .expect("required values should load");

        assert_eq!(settings.transcribe_model, DEFAULT_TRANSCRIBE_MODEL);
    }

    #[tokio::test]
    async fn configured_model_is_kept_verbatim() {
        let mut values = required_values();
        values.push((VariableName::TranscribeModelName, "openai/whisper-1"));

        let settings = settings(&values)
            .await
            .expect("a configured model should load");

        assert_eq!(settings.transcribe_model, "openai/whisper-1");
    }

    #[tokio::test]
    async fn unset_retention_resolves_to_the_default() {
        let settings = settings(&required_values())
            .await
            .expect("required values should load");

        assert_eq!(settings.retention_days, Some(DEFAULT_RETENTION_DAYS));
    }

    #[tokio::test]
    async fn zero_retention_disables_pruning() {
        let mut values = required_values();
        values.push((VariableName::RetentionDays, "0"));

        let settings = settings(&values)
            .await
            .expect("a zero retention should load");

        assert_eq!(settings.retention_days, None);
    }

    #[tokio::test]
    async fn configured_retention_is_kept() {
        let mut values = required_values();
        values.push((VariableName::RetentionDays, "7"));

        let settings = settings(&values)
            .await
            .expect("a configured retention should load");

        assert_eq!(settings.retention_days, Some(7));
    }

    #[tokio::test]
    async fn non_numeric_retention_fails() {
        for raw in ["-1", "1d"] {
            let mut values = required_values();
            values.push((VariableName::RetentionDays, raw));

            match settings_error(&values).await {
                Error::InvalidRetentionDays(value) => assert_eq!(value, raw),
                other => panic!("expected an invalid retention, got {other}"),
            }
        }
    }

    #[test]
    fn a_configured_dsn_is_a_destination() {
        let dsn = classify_dsn(Ok(Some("https://public@example.com/1".to_owned())));

        assert!(matches!(dsn, SentryDsn::Configured(_)));
    }

    #[test]
    fn an_absent_dsn_is_disabled_with_the_variable_name() {
        match classify_dsn(Ok(None)) {
            SentryDsn::Disabled(reason) => {
                assert!(reason.contains("SENTRY_DSN"), "reason {reason}")
            }
            SentryDsn::Configured(_) => panic!("an absent DSN should disable reporting"),
        }
    }

    #[test]
    fn an_unusable_dsn_is_disabled_with_the_error() {
        let error = Error::ReadConfiguration {
            path: PathBuf::from("/run/secrets/sentry_dsn"),
            source: io::Error::new(io::ErrorKind::NotFound, "no such file"),
        };
        let expected = format!("SENTRY_DSN: {error}");

        match classify_dsn(Err(error)) {
            SentryDsn::Disabled(reason) => assert_eq!(reason, expected),
            SentryDsn::Configured(_) => panic!("an unusable DSN should disable reporting"),
        }
    }

    #[test]
    fn missing_error_names_both_forms() {
        let name = VariableName::TelegramApiToken;
        let message = Error::MissingVariable(name).to_string();

        assert!(message.contains(name.as_str()), "message {message:?}");
        assert!(message.contains(name.file_key()), "message {message:?}");
        assert!(!message.contains("secret"), "message {message:?}");
    }

    #[test]
    fn not_unicode_error_names_the_variable() {
        let name = VariableName::TelegramApiToken;
        let message = Error::VariableNotUnicode(name).to_string();

        assert_eq!(message, format!("{} is not valid Unicode", name.as_str()));
    }

    #[test]
    fn read_file_error_names_the_path() {
        let error = Error::ReadConfiguration {
            path: PathBuf::from("/run/secrets/token"),
            source: io::Error::new(io::ErrorKind::NotFound, "no such file"),
        };

        assert_eq!(
            error.to_string(),
            "failed to read /run/secrets/token: no such file"
        );
    }

    #[tokio::test]
    async fn direct_value_resolves_when_no_file_variable_is_set() {
        let lookup = |key: &str| match key {
            "TELEGRAM_API_TOKEN" => Ok("direct".to_owned()),
            _ => Err(VarError::NotPresent),
        };
        let read_file = async |_: &str| -> Result<String> {
            panic!("no file should be read without TELEGRAM_API_TOKEN_FILE")
        };

        let value = resolve(VariableName::TelegramApiToken, lookup, read_file)
            .await
            .expect("direct value should resolve");
        assert_eq!(value, Some("direct".to_owned()));
    }

    #[tokio::test]
    async fn file_variant_takes_precedence_when_both_are_set() {
        let lookup = |key: &str| match key {
            "TELEGRAM_API_TOKEN_FILE" => Ok("/run/secrets/token".to_owned()),
            "TELEGRAM_API_TOKEN" => Ok("direct".to_owned()),
            _ => Err(VarError::NotPresent),
        };
        let read_file = async |path: &str| -> Result<String> {
            assert_eq!(path, "/run/secrets/token");
            Ok("from-file".to_owned())
        };

        let value = resolve(VariableName::TelegramApiToken, lookup, read_file)
            .await
            .expect("file value should resolve");
        assert_eq!(value, Some("from-file".to_owned()));
    }

    #[tokio::test]
    async fn both_sources_are_trimmed() {
        let from_file = |key: &str| match key {
            "TELEGRAM_API_TOKEN_FILE" => Ok("/run/secrets/token".to_owned()),
            _ => Err(VarError::NotPresent),
        };
        let read_file = async |_: &str| -> Result<String> { Ok(" from-file \n".to_owned()) };
        let value = resolve(VariableName::TelegramApiToken, from_file, read_file)
            .await
            .expect("file value should resolve");
        assert_eq!(value, Some("from-file".to_owned()));

        let direct = |key: &str| match key {
            "TELEGRAM_API_TOKEN" => Ok("\t direct value \r\n".to_owned()),
            _ => Err(VarError::NotPresent),
        };
        let read_file = async |_: &str| -> Result<String> {
            panic!("no file should be read without TELEGRAM_API_TOKEN_FILE")
        };
        let value = resolve(VariableName::TelegramApiToken, direct, read_file)
            .await
            .expect("direct value should resolve");
        assert_eq!(value, Some("direct value".to_owned()));
    }

    #[tokio::test]
    async fn whitespace_only_values_mean_unset() {
        let from_file = |key: &str| match key {
            "TELEGRAM_API_TOKEN_FILE" => Ok("/run/secrets/token".to_owned()),
            _ => Err(VarError::NotPresent),
        };
        for contents in ["", "   ", "\n\t\r\n"] {
            let read_file = async |_: &str| -> Result<String> { Ok(contents.to_owned()) };
            let value = resolve(VariableName::TelegramApiToken, from_file, read_file)
                .await
                .expect("a blank file should not be an error");
            assert_eq!(value, None, "contents {contents:?}");
        }

        for value in ["", "   ", "\n\t"] {
            let direct = |key: &str| match key {
                "TELEGRAM_API_TOKEN" => Ok(value.to_owned()),
                _ => Err(VarError::NotPresent),
            };
            let read_file = async |_: &str| -> Result<String> {
                panic!("no file should be read without TELEGRAM_API_TOKEN_FILE")
            };
            let resolved = resolve(VariableName::TelegramApiToken, direct, read_file)
                .await
                .expect("a blank value should not be an error");
            assert_eq!(resolved, None, "value {value:?}");
        }
    }

    #[tokio::test]
    async fn absent_variable_resolves_to_none() {
        let lookup = |_: &str| Err::<String, VarError>(VarError::NotPresent);
        let read_file = async |_: &str| -> Result<String> {
            panic!("no file should be read with neither variable set")
        };

        let value = resolve(VariableName::TelegramApiToken, lookup, read_file)
            .await
            .expect("an absent variable should not be an error");
        assert_eq!(value, None);
    }

    #[tokio::test]
    async fn unreadable_file_is_reported() {
        let lookup = |key: &str| match key {
            "TELEGRAM_API_TOKEN_FILE" => Ok("/run/secrets/missing".to_owned()),
            _ => Err(VarError::NotPresent),
        };
        let read_file = async |_: &str| -> Result<String> {
            Err(Error::ReadConfiguration {
                path: PathBuf::from("/run/secrets/missing"),
                source: io::Error::new(io::ErrorKind::NotFound, "no such file"),
            })
        };

        let error = resolve(VariableName::TelegramApiToken, lookup, read_file)
            .await
            .expect_err("unreadable file should not resolve");
        match error {
            Error::ReadConfiguration { path, source } => {
                assert_eq!(path, PathBuf::from("/run/secrets/missing"));
                assert_eq!(source.kind(), io::ErrorKind::NotFound);
            }
            other => panic!("expected ReadConfiguration, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn non_unicode_variable_is_reported() {
        let file_lookup = |key: &str| match key {
            "TELEGRAM_API_TOKEN_FILE" => Err(VarError::NotUnicode(OsString::from("\u{fffd}"))),
            _ => Err(VarError::NotPresent),
        };
        let read_file =
            async |_: &str| -> Result<String> { panic!("non-Unicode path cannot be read") };
        let error = resolve(VariableName::TelegramApiToken, file_lookup, read_file)
            .await
            .expect_err("non-Unicode file variable should not resolve");
        assert!(matches!(
            error,
            Error::VariableNotUnicode(VariableName::TelegramApiToken)
        ));

        let direct_lookup = |key: &str| match key {
            "TELEGRAM_API_TOKEN_FILE" => Err(VarError::NotPresent),
            "TELEGRAM_API_TOKEN" => Err(VarError::NotUnicode(OsString::from("\u{fffd}"))),
            _ => Err(VarError::NotPresent),
        };
        let read_file = async |_: &str| -> Result<String> {
            panic!("no file should be read without TELEGRAM_API_TOKEN_FILE")
        };
        let error = resolve(VariableName::TelegramApiToken, direct_lookup, read_file)
            .await
            .expect_err("non-Unicode direct variable should not resolve");
        assert!(matches!(
            error,
            Error::VariableNotUnicode(VariableName::TelegramApiToken)
        ));
    }

    #[tokio::test]
    async fn file_contents_are_read_and_decoded_as_utf8() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let path = directory.path().join("token");
        std::fs::write(&path, "  from-file\n").expect("file should be written");
        let path = path.to_str().expect("temporary paths are Unicode");

        let value = read_file(path).await.expect("a UTF-8 file should be read");
        assert_eq!(value.trim(), "from-file");
    }

    #[tokio::test]
    async fn missing_file_is_a_read_file_error() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let path = directory.path().join("absent");
        let path = path.to_str().expect("temporary paths are Unicode");

        match read_file(path).await {
            Err(Error::ReadConfiguration { source, .. }) => {
                assert_eq!(source.kind(), io::ErrorKind::NotFound);
            }
            other => panic!("expected ReadConfiguration, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn file_contents_that_are_not_utf8_are_invalid_data() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let path = directory.path().join("token");
        std::fs::write(&path, [0xff, 0xfe, 0x00]).expect("file should be written");
        let path = path.to_str().expect("temporary paths are Unicode");

        match read_file(path).await {
            Err(Error::ReadConfiguration { source, .. }) => {
                assert_eq!(source.kind(), io::ErrorKind::InvalidData);
            }
            other => panic!("expected ReadConfiguration, got {other:?}"),
        }
    }
}
