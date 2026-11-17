//! Secret configuration loaded from the environment.
//!
//! [`get_secret`] resolves a secret named `NAME` in this order:
//!
//! 1. `NAME_FILE`: when set, its value is the path of a file holding the secret.
//! 2. `NAME`: the secret value itself.
//!
//! The file variant takes precedence so that mounted Docker or Kubernetes secrets stay
//! authoritative even when the direct variable is also set. File contents have one trailing
//! `\r\n` or `\n` removed, because secret files written by shells and editors usually end in a
//! newline; values read directly from the environment are returned verbatim.

use std::env::VarError;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// Failure modes of [`get_secret`].
#[derive(Debug)]
pub enum SecretError {
    /// Neither `NAME_FILE` nor `NAME` is set.
    Missing(String),
    /// The value of a variable, or its name, is not valid Unicode.
    NotUnicode(String),
    /// The file named by `NAME_FILE` could not be read.
    ReadFile { path: PathBuf, source: io::Error },
}

impl fmt::Display for SecretError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(name) => {
                write!(
                    formatter,
                    "secret {name} is not set: neither {name}_FILE nor {name} exists"
                )
            }
            Self::NotUnicode(name) => write!(formatter, "secret {name} is not valid Unicode"),
            Self::ReadFile { path, source } => {
                write!(
                    formatter,
                    "failed to read secret file {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for SecretError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadFile { source, .. } => Some(source),
            Self::Missing(_) | Self::NotUnicode(_) => None,
        }
    }
}

/// Resolves a secret from the environment.
///
/// Returns the contents of the file named by `<name>_FILE` when that variable is set, otherwise
/// the value of `<name>`. See the module documentation for the exact precedence and trimming
/// rules.
pub fn get_secret(name: &str) -> Result<String, SecretError> {
    resolve_secret(
        name,
        |key| std::env::var(key),
        |path| std::fs::read_to_string(path),
    )
}

fn resolve_secret(
    name: &str,
    lookup: impl Fn(&str) -> Result<String, VarError>,
    read_file: impl Fn(&Path) -> io::Result<String>,
) -> Result<String, SecretError> {
    let file_key = format!("{name}_FILE");
    match lookup(&file_key) {
        Ok(path) => {
            let path = PathBuf::from(path);
            match read_file(&path) {
                Ok(contents) => Ok(trim_trailing_newline(&contents).to_owned()),
                Err(source) => Err(SecretError::ReadFile { path, source }),
            }
        }
        Err(VarError::NotUnicode(_)) => Err(SecretError::NotUnicode(file_key)),
        Err(VarError::NotPresent) => match lookup(name) {
            Ok(value) => Ok(value),
            Err(VarError::NotUnicode(_)) => Err(SecretError::NotUnicode(name.to_owned())),
            Err(VarError::NotPresent) => Err(SecretError::Missing(name.to_owned())),
        },
    }
}

fn trim_trailing_newline(value: &str) -> &str {
    value
        .strip_suffix("\r\n")
        .or_else(|| value.strip_suffix('\n'))
        .unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    #[test]
    fn returns_direct_value_when_file_variant_absent() {
        let lookup = |key: &str| match key {
            "TOKEN" => Ok("direct".to_owned()),
            _ => Err(VarError::NotPresent),
        };
        let read_file = |_: &Path| panic!("no file should be read without TOKEN_FILE");

        let value =
            resolve_secret("TOKEN", lookup, read_file).expect("direct value should resolve");
        assert_eq!(value, "direct");
    }

    #[test]
    fn file_variant_takes_precedence_when_both_are_set() {
        let lookup = |key: &str| match key {
            "TOKEN_FILE" => Ok("/run/secrets/token".to_owned()),
            "TOKEN" => Ok("direct".to_owned()),
            _ => Err(VarError::NotPresent),
        };
        let read_file = |path: &Path| {
            assert_eq!(path, Path::new("/run/secrets/token"));
            Ok("from-file".to_owned())
        };

        let value = resolve_secret("TOKEN", lookup, read_file).expect("file value should resolve");
        assert_eq!(value, "from-file");
    }

    #[test]
    fn trims_trailing_lf_and_crlf_from_file_contents() {
        for contents in ["from-file\n", "from-file\r\n"] {
            let lookup = |key: &str| match key {
                "TOKEN_FILE" => Ok("/run/secrets/token".to_owned()),
                _ => Err(VarError::NotPresent),
            };
            let read_file = |_: &Path| Ok(contents.to_owned());

            let value =
                resolve_secret("TOKEN", lookup, read_file).expect("file value should resolve");
            assert_eq!(value, "from-file");
        }

        let lookup = |key: &str| match key {
            "TOKEN_FILE" => Ok("/run/secrets/token".to_owned()),
            _ => Err(VarError::NotPresent),
        };
        let read_file = |_: &Path| Ok("from-file  ".to_owned());
        let value = resolve_secret("TOKEN", lookup, read_file)
            .expect("file value with trailing spaces should resolve");
        assert_eq!(value, "from-file  ");
    }

    #[test]
    fn missing_secret_returns_missing_error() {
        let lookup = |_: &str| Err::<String, VarError>(VarError::NotPresent);
        let read_file = |_: &Path| panic!("no file should be read without TOKEN_FILE");

        let error = resolve_secret("TOKEN", lookup, read_file)
            .expect_err("missing secret should not resolve");
        assert!(matches!(error, SecretError::Missing(name) if name == "TOKEN"));
    }

    #[test]
    fn unreadable_file_returns_read_file_error() {
        let lookup = |key: &str| match key {
            "TOKEN_FILE" => Ok("/run/secrets/missing".to_owned()),
            _ => Err(VarError::NotPresent),
        };
        let read_file = |_: &Path| Err(io::Error::new(io::ErrorKind::NotFound, "no such file"));

        let error = resolve_secret("TOKEN", lookup, read_file)
            .expect_err("unreadable file should not resolve");
        match error {
            SecretError::ReadFile { path, source } => {
                assert_eq!(path, Path::new("/run/secrets/missing"));
                assert_eq!(source.kind(), io::ErrorKind::NotFound);
            }
            other => panic!("expected ReadFile, got {other:?}"),
        }
    }

    #[test]
    fn non_unicode_variable_returns_not_unicode_error() {
        let file_lookup = |key: &str| match key {
            "TOKEN_FILE" => Err(VarError::NotUnicode(OsString::from("\u{fffd}"))),
            _ => Err(VarError::NotPresent),
        };
        let read_file = |_: &Path| panic!("non-Unicode path cannot be read");
        let error = resolve_secret("TOKEN", file_lookup, read_file)
            .expect_err("non-Unicode file variable should not resolve");
        assert!(matches!(error, SecretError::NotUnicode(name) if name == "TOKEN_FILE"));

        let direct_lookup = |key: &str| match key {
            "TOKEN_FILE" => Err(VarError::NotPresent),
            "TOKEN" => Err(VarError::NotUnicode(OsString::from("\u{fffd}"))),
            _ => Err(VarError::NotPresent),
        };
        let read_file = |_: &Path| panic!("no file should be read without TOKEN_FILE");
        let error = resolve_secret("TOKEN", direct_lookup, read_file)
            .expect_err("non-Unicode direct variable should not resolve");
        assert!(matches!(error, SecretError::NotUnicode(name) if name == "TOKEN"));
    }

    #[test]
    fn empty_file_contents_are_returned_as_empty_string() {
        let lookup = |key: &str| match key {
            "TOKEN_FILE" => Ok("/run/secrets/token".to_owned()),
            _ => Err(VarError::NotPresent),
        };
        let read_file = |_: &Path| Ok(String::new());

        let value = resolve_secret("TOKEN", lookup, read_file)
            .expect("empty file should resolve to an empty secret");
        assert_eq!(value, "");
    }
}
