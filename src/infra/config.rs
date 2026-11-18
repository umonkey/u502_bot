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
//! `Ok(None)` means the variable is not configured, and the caller decides whether that is fatal,
//! a default, or a disable; only [`require_secret`] treats absence as an error. `RUST_LOG` is the one
//! exception: `tracing_subscriber`'s `EnvFilter` reads it, not this crate, and it has no file form.

use std::env::VarError;
use std::io;
use std::path::PathBuf;

use crate::shared::config::VariableName;
use crate::shared::result::{Error, Result};

/// Resolves one environment variable.
///
/// `Ok(None)` means the variable is not configured. `Err` means a value is present but unusable:
/// a non-Unicode variable, or a `NAME_FILE` path that cannot be read or is not UTF-8. See the
/// module documentation for the precedence and the trimming rule.
pub fn get_optional(name: VariableName) -> Result<Option<String>> {
    resolve(name.as_str(), |key| std::env::var(key), read_file)
}

/// Resolves a secret that startup cannot run without.
///
/// This is the only resolver that treats absence as an error, and it is what a caller uses instead
/// of checking [`get_optional`] for `None`.
pub fn require_secret(name: VariableName) -> Result<String> {
    get_optional(name)?.ok_or_else(|| Error::MissingVariable(name.as_str().to_owned()))
}

/// The one algorithm behind [`get_optional`], with the lookups injected so tests need not touch the
/// process environment.
fn resolve(
    name: &str,
    lookup: impl Fn(&str) -> Result<String, VarError>,
    read_file: impl Fn(&str) -> Result<String>,
) -> Result<Option<String>> {
    let file_key = format!("{name}_FILE");
    let raw = match lookup(&file_key) {
        Ok(path) => read_file(&path)?,
        Err(VarError::NotPresent) => match lookup(name) {
            Ok(value) => value,
            Err(VarError::NotPresent) => return Ok(None),
            Err(VarError::NotUnicode(_)) => {
                return Err(Error::VariableNotUnicode(name.to_owned()));
            }
        },
        Err(VarError::NotUnicode(_)) => return Err(Error::VariableNotUnicode(file_key)),
    };

    let trimmed = raw.trim();

    Ok((!trimmed.is_empty()).then(|| trimmed.to_owned()))
}

/// Reads a file and decodes it as UTF-8.
fn read_file(path: &str) -> Result<String> {
    std::fs::read(path)
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

    #[test]
    fn missing_error_names_both_forms() {
        let message = Error::MissingVariable("TOKEN".to_owned()).to_string();

        assert!(message.contains("TOKEN"), "message {message:?}");
        assert!(message.contains("TOKEN_FILE"), "message {message:?}");
        assert!(!message.contains("secret"), "message {message:?}");
    }

    #[test]
    fn not_unicode_error_names_the_variable() {
        let message = Error::VariableNotUnicode("TOKEN".to_owned()).to_string();

        assert_eq!(message, "TOKEN is not valid Unicode");
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

    #[test]
    fn direct_value_resolves_when_no_file_variable_is_set() {
        let lookup = |key: &str| match key {
            "TOKEN" => Ok("direct".to_owned()),
            _ => Err(VarError::NotPresent),
        };
        let read_file = |_: &str| panic!("no file should be read without TOKEN_FILE");

        let value = resolve("TOKEN", lookup, read_file).expect("direct value should resolve");
        assert_eq!(value, Some("direct".to_owned()));
    }

    #[test]
    fn file_variant_takes_precedence_when_both_are_set() {
        let lookup = |key: &str| match key {
            "TOKEN_FILE" => Ok("/run/secrets/token".to_owned()),
            "TOKEN" => Ok("direct".to_owned()),
            _ => Err(VarError::NotPresent),
        };
        let read_file = |path: &str| {
            assert_eq!(path, "/run/secrets/token");
            Ok("from-file".to_owned())
        };

        let value = resolve("TOKEN", lookup, read_file).expect("file value should resolve");
        assert_eq!(value, Some("from-file".to_owned()));
    }

    #[test]
    fn both_sources_are_trimmed() {
        let from_file = |key: &str| match key {
            "TOKEN_FILE" => Ok("/run/secrets/token".to_owned()),
            _ => Err(VarError::NotPresent),
        };
        let read_file = |_: &str| Ok(" from-file \n".to_owned());
        let value = resolve("TOKEN", from_file, read_file).expect("file value should resolve");
        assert_eq!(value, Some("from-file".to_owned()));

        let direct = |key: &str| match key {
            "TOKEN" => Ok("\t direct value \r\n".to_owned()),
            _ => Err(VarError::NotPresent),
        };
        let read_file = |_: &str| panic!("no file should be read without TOKEN_FILE");
        let value = resolve("TOKEN", direct, read_file).expect("direct value should resolve");
        assert_eq!(value, Some("direct value".to_owned()));
    }

    #[test]
    fn whitespace_only_values_mean_unset() {
        let from_file = |key: &str| match key {
            "TOKEN_FILE" => Ok("/run/secrets/token".to_owned()),
            _ => Err(VarError::NotPresent),
        };
        for contents in ["", "   ", "\n\t\r\n"] {
            let read_file = |_: &str| Ok(contents.to_owned());
            let value = resolve("TOKEN", from_file, read_file)
                .expect("a blank file should not be an error");
            assert_eq!(value, None, "contents {contents:?}");
        }

        for value in ["", "   ", "\n\t"] {
            let direct = |key: &str| match key {
                "TOKEN" => Ok(value.to_owned()),
                _ => Err(VarError::NotPresent),
            };
            let read_file = |_: &str| panic!("no file should be read without TOKEN_FILE");
            let resolved =
                resolve("TOKEN", direct, read_file).expect("a blank value should not be an error");
            assert_eq!(resolved, None, "value {value:?}");
        }
    }

    #[test]
    fn absent_variable_resolves_to_none() {
        let lookup = |_: &str| Err::<String, VarError>(VarError::NotPresent);
        let read_file = |_: &str| panic!("no file should be read with neither variable set");

        let value =
            resolve("TOKEN", lookup, read_file).expect("an absent variable should not be an error");
        assert_eq!(value, None);
    }

    #[test]
    fn unreadable_file_is_reported() {
        let lookup = |key: &str| match key {
            "TOKEN_FILE" => Ok("/run/secrets/missing".to_owned()),
            _ => Err(VarError::NotPresent),
        };
        let read_file = |_: &str| {
            Err(Error::ReadConfiguration {
                path: PathBuf::from("/run/secrets/missing"),
                source: io::Error::new(io::ErrorKind::NotFound, "no such file"),
            })
        };

        let error =
            resolve("TOKEN", lookup, read_file).expect_err("unreadable file should not resolve");
        match error {
            Error::ReadConfiguration { path, source } => {
                assert_eq!(path, PathBuf::from("/run/secrets/missing"));
                assert_eq!(source.kind(), io::ErrorKind::NotFound);
            }
            other => panic!("expected ReadConfiguration, got {other:?}"),
        }
    }

    #[test]
    fn non_unicode_variable_is_reported() {
        let file_lookup = |key: &str| match key {
            "TOKEN_FILE" => Err(VarError::NotUnicode(OsString::from("\u{fffd}"))),
            _ => Err(VarError::NotPresent),
        };
        let read_file = |_: &str| panic!("non-Unicode path cannot be read");
        let error = resolve("TOKEN", file_lookup, read_file)
            .expect_err("non-Unicode file variable should not resolve");
        assert!(matches!(error, Error::VariableNotUnicode(name) if name == "TOKEN_FILE"));

        let direct_lookup = |key: &str| match key {
            "TOKEN_FILE" => Err(VarError::NotPresent),
            "TOKEN" => Err(VarError::NotUnicode(OsString::from("\u{fffd}"))),
            _ => Err(VarError::NotPresent),
        };
        let read_file = |_: &str| panic!("no file should be read without TOKEN_FILE");
        let error = resolve("TOKEN", direct_lookup, read_file)
            .expect_err("non-Unicode direct variable should not resolve");
        assert!(matches!(error, Error::VariableNotUnicode(name) if name == "TOKEN"));
    }

    #[test]
    fn file_contents_are_read_and_decoded_as_utf8() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let path = directory.path().join("token");
        std::fs::write(&path, "  from-file\n").expect("file should be written");
        let path = path.to_str().expect("temporary paths are Unicode");

        let value = read_file(path).expect("a UTF-8 file should be read");
        assert_eq!(value.trim(), "from-file");
    }

    #[test]
    fn missing_file_is_a_read_file_error() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let path = directory.path().join("absent");
        let path = path.to_str().expect("temporary paths are Unicode");

        match read_file(path) {
            Err(Error::ReadConfiguration { source, .. }) => {
                assert_eq!(source.kind(), io::ErrorKind::NotFound);
            }
            other => panic!("expected ReadConfiguration, got {other:?}"),
        }
    }

    #[test]
    fn file_contents_that_are_not_utf8_are_invalid_data() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let path = directory.path().join("token");
        std::fs::write(&path, [0xff, 0xfe, 0x00]).expect("file should be written");
        let path = path.to_str().expect("temporary paths are Unicode");

        match read_file(path) {
            Err(Error::ReadConfiguration { source, .. }) => {
                assert_eq!(source.kind(), io::ErrorKind::InvalidData);
            }
            other => panic!("expected ReadConfiguration, got {other:?}"),
        }
    }
}
