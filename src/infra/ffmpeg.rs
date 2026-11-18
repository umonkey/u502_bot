//! ffmpeg subprocess client.
//!
//! [`Ffmpeg::extract_audio`] reduces one video file to the mono 16 kHz Ogg/Opus speech audio the
//! model accepts, and [`Ffmpeg::probe`] reports whether the binary runs at all. ffmpeg is spawned as
//! a child process rather than linked: the build stays unchanged, a missing binary degrades to
//! [`FfmpegError::Unavailable`] instead of failing startup, and `kill_on_drop` keeps an abandoned
//! extraction from outliving the bot.
//!
//! The client records its own telemetry: the call counter and duration are captured for failed and
//! timed-out calls too, and only this module knows that a call happened. Failures are returned to
//! the caller, which logs them once with the attachment they belong to.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::infra::sentry;

/// Environment variable naming the ffmpeg binary.
pub const FFMPEG_PATH: &str = "FFMPEG_PATH";

/// Binary used when [`FFMPEG_PATH`] is unset or empty.
const DEFAULT_FFMPEG: &str = "ffmpeg";

/// Extraction timeout: a safety net for a pathological input, not a working constraint.
const TIMEOUT: Duration = Duration::from_secs(120);

/// Characters of an error body kept for the logs.
const MAX_ERROR_BODY: usize = 500;

/// Failure modes of probing or running ffmpeg.
#[derive(Debug)]
pub enum FfmpegError {
    /// The binary could not be spawned: it is missing, or it is not on `PATH`.
    Unavailable(std::io::Error),
    /// ffmpeg exited with a non-success status.
    Status { code: Option<i32>, stderr: String },
    /// The child process could not be started or waited for.
    Io(std::io::Error),
    /// The input carries no audio stream, so there is nothing to extract.
    NoAudioStream { stderr: String },
    /// The extraction did not finish within [`TIMEOUT`].
    Timeout,
}

impl fmt::Display for FfmpegError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(source) => write!(formatter, "ffmpeg is not available: {source}"),
            Self::Status { code, stderr } => match code {
                Some(code) => write!(formatter, "ffmpeg exited with status {code}: {stderr}"),
                None => write!(formatter, "ffmpeg was terminated by a signal: {stderr}"),
            },
            Self::Io(source) => write!(formatter, "failed to run ffmpeg: {source}"),
            Self::NoAudioStream { stderr } => {
                write!(formatter, "the input carries no audio stream: {stderr}")
            }
            Self::Timeout => write!(
                formatter,
                "ffmpeg did not finish within {} seconds",
                TIMEOUT.as_secs()
            ),
        }
    }
}

impl std::error::Error for FfmpegError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unavailable(source) | Self::Io(source) => Some(source),
            Self::Status { .. } | Self::NoAudioStream { .. } | Self::Timeout => None,
        }
    }
}

impl From<std::io::Error> for FfmpegError {
    fn from(source: std::io::Error) -> Self {
        Self::Io(source)
    }
}

/// Client for the `ffmpeg` command-line program.
pub struct Ffmpeg {
    binary: PathBuf,
}

impl Ffmpeg {
    /// Builds a client for the given binary.
    pub fn new(binary: PathBuf) -> Self {
        Self { binary }
    }

    /// Builds a client from [`FFMPEG_PATH`], falling back to [`DEFAULT_FFMPEG`] when it is unset or
    /// empty.
    pub fn from_env() -> Self {
        Self::new(resolve_binary(std::env::var_os(FFMPEG_PATH)))
    }

    /// Reports whether the binary runs.
    ///
    /// `ffmpeg -version` exits immediately, so it needs no timeout of its own. A non-zero exit is a
    /// [`FfmpegError::Status`], which is what an unusable install or a broken dynamic link looks
    /// like.
    pub async fn probe(&self) -> Result<(), FfmpegError> {
        let completed = tokio::process::Command::new(&self.binary)
            .arg("-version")
            .kill_on_drop(true)
            .output()
            .await
            .map_err(spawn_error)?;

        if completed.status.success() {
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&completed.stderr);
        Err(FfmpegError::Status {
            code: completed.status.code(),
            stderr: truncate(&stderr),
        })
    }

    /// Extracts the first audio stream of `input` into an Ogg/Opus file at `output`.
    ///
    /// The stopwatch wraps the whole timeout, so a timed-out extraction still increments
    /// `ffmpeg.calls` and records a `ffmpeg.duration`; a call is a call whether it succeeds or not.
    /// Failures are returned so the caller can log them with context.
    pub async fn extract_audio(&self, input: &Path, output: &Path) -> Result<(), FfmpegError> {
        let started = Instant::now();
        let outcome = tokio::time::timeout(TIMEOUT, self.run(input, output)).await;
        let elapsed = started.elapsed();
        sentry::ffmpeg_call(elapsed);

        match outcome {
            Ok(result) => result,
            Err(_) => Err(FfmpegError::Timeout),
        }
    }

    async fn run(&self, input: &Path, output: &Path) -> Result<(), FfmpegError> {
        // `Command::output` captures both pipes, so ffmpeg cannot block on a full buffer.
        let completed = tokio::process::Command::new(&self.binary)
            .args(extraction_args(input, output))
            .kill_on_drop(true)
            .output()
            .await
            .map_err(spawn_error)?;

        if completed.status.success() {
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&completed.stderr);
        if is_missing_audio(&stderr) {
            return Err(FfmpegError::NoAudioStream {
                stderr: truncate(&stderr),
            });
        }

        Err(FfmpegError::Status {
            code: completed.status.code(),
            stderr: truncate(&stderr),
        })
    }
}

/// Maps a failure to start the child process onto the error the caller matches on.
fn spawn_error(source: std::io::Error) -> FfmpegError {
    match source.kind() {
        std::io::ErrorKind::NotFound => FfmpegError::Unavailable(source),
        _ => FfmpegError::Io(source),
    }
}

/// Resolves a raw [`FFMPEG_PATH`] setting, falling back to [`DEFAULT_FFMPEG`] when it is absent or
/// blank.
///
/// A configured value is returned verbatim: only the emptiness check trims, so a path an operator
/// set on purpose is never rewritten.
fn resolve_binary(raw: Option<OsString>) -> PathBuf {
    match raw {
        Some(binary) if !binary.to_string_lossy().trim().is_empty() => PathBuf::from(binary),
        _ => PathBuf::from(DEFAULT_FFMPEG),
    }
}

/// The argv after the binary name: one pass that maps the first audio stream to Ogg/Opus, 16 kHz
/// mono.
///
/// `-map 0:a:0` selects the first audio stream and the demuxer skips video packets without decoding
/// them. `-f opus` names the muxer that guarantees Opus; the bytes are still Ogg, which is the
/// format the request declares.
fn extraction_args(input: &Path, output: &Path) -> Vec<OsString> {
    vec![
        OsString::from("-nostdin"),
        OsString::from("-hide_banner"),
        OsString::from("-loglevel"),
        OsString::from("error"),
        OsString::from("-y"),
        OsString::from("-i"),
        input.into(),
        OsString::from("-map"),
        OsString::from("0:a:0"),
        OsString::from("-ac"),
        OsString::from("1"),
        OsString::from("-ar"),
        OsString::from("16000"),
        OsString::from("-c:a"),
        OsString::from("libopus"),
        OsString::from("-b:a"),
        OsString::from("24k"),
        OsString::from("-f"),
        OsString::from("opus"),
        output.into(),
    ]
}

/// Reports whether ffmpeg's stderr says the input has no audio stream.
///
/// The wording is ffmpeg's own; if it changes, the attachment is retried until the age window
/// closes instead of being skipped.
fn is_missing_audio(stderr: &str) -> bool {
    [
        "matches no streams",
        "does not contain any stream",
        "Output file does not contain any stream",
    ]
    .into_iter()
    .any(|marker| stderr.contains(marker))
}

/// Keeps at most [`MAX_ERROR_BODY`] characters of an error body, on a character boundary.
fn truncate(body: &str) -> String {
    match body.char_indices().nth(MAX_ERROR_BODY) {
        Some((index, _)) => body[..index].to_owned(),
        None => body.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_arguments_are_exact() {
        let input = Path::new("/tmp/video.mp4");
        let output = Path::new("/tmp/audio.ogg");

        let expected: Vec<OsString> = [
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-i",
            "/tmp/video.mp4",
            "-map",
            "0:a:0",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "libopus",
            "-b:a",
            "24k",
            "-f",
            "opus",
            "/tmp/audio.ogg",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();

        assert_eq!(extraction_args(input, output), expected);
    }

    #[test]
    fn ffmpeg_wording_for_a_missing_audio_stream_is_recognized() {
        for stderr in [
            "Stream map '0:a:0' matches no streams.",
            "Output file #0 does not contain any stream",
            "[out#0/opus] Output file does not contain any stream",
        ] {
            assert!(is_missing_audio(stderr), "stderr {stderr:?}");
        }
    }

    #[test]
    fn other_failures_are_not_reported_as_a_missing_audio_stream() {
        for stderr in [
            "",
            "Unknown encoder 'libopus'",
            "Invalid data found when processing input",
            "/tmp/video.mp4: No such file or directory",
        ] {
            assert!(!is_missing_audio(stderr), "stderr {stderr:?}");
        }
    }

    #[test]
    fn long_error_bodies_are_truncated() {
        let body = "x".repeat(MAX_ERROR_BODY * 2);
        assert_eq!(truncate(&body).chars().count(), MAX_ERROR_BODY);
        assert_eq!(truncate("short"), "short");
    }

    #[test]
    fn absent_or_blank_binary_falls_back_to_the_default() {
        assert_eq!(resolve_binary(None), PathBuf::from(DEFAULT_FFMPEG));

        for raw in [OsString::new(), OsString::from("   \n")] {
            assert_eq!(resolve_binary(Some(raw)), PathBuf::from(DEFAULT_FFMPEG));
        }
    }

    #[test]
    fn configured_binary_is_kept_verbatim() {
        assert_eq!(
            resolve_binary(Some(OsString::from("/opt/ffmpeg/bin/ffmpeg"))),
            PathBuf::from("/opt/ffmpeg/bin/ffmpeg")
        );
    }

    #[tokio::test]
    async fn missing_binary_is_reported_as_unavailable() {
        let ffmpeg = Ffmpeg::new(PathBuf::from("u502-no-such-ffmpeg"));

        let error = ffmpeg
            .extract_audio(Path::new("input.mp4"), Path::new("output.ogg"))
            .await
            .expect_err("a missing binary should not extract audio");
        match error {
            FfmpegError::Unavailable(source) => {
                assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
            }
            other => panic!("expected Unavailable, got {other}"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failing_binary_reports_its_status() {
        use std::os::unix::fs::PermissionsExt;

        /// `ETXTBSY`: the freshly written script is briefly open for writing in a forked child.
        const TEXT_FILE_BUSY: i32 = 26;

        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let binary = directory.path().join("ffmpeg-stub");
        std::fs::write(&binary, "#!/bin/sh\necho 'simulated failure' >&2\nexit 3\n")
            .expect("stub should be written");
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))
            .expect("stub should be executable");

        let input = directory.path().join("input.mp4");
        let output = directory.path().join("output.ogg");
        let ffmpeg = Ffmpeg::new(binary);

        // Another test can hold the script open through a forked child for a moment, which makes
        // `exec` fail with `ETXTBSY`; retry instead of reporting a flake.
        let mut attempt = 0;
        let error = loop {
            match ffmpeg.extract_audio(&input, &output).await {
                Ok(()) => panic!("a failing binary should not extract audio"),
                Err(FfmpegError::Io(source))
                    if source.raw_os_error() == Some(TEXT_FILE_BUSY) && attempt < 20 =>
                {
                    attempt += 1;
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                Err(error) => break error,
            }
        };

        match error {
            FfmpegError::Status { code, stderr } => {
                assert_eq!(code, Some(3));
                assert!(stderr.contains("simulated failure"), "stderr {stderr:?}");
            }
            other => panic!("expected Status, got {other}"),
        }
    }
}
