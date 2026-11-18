//! OpenRouter speech-to-text client.
//!
//! [`OpenRouter::transcribe_audio`] sends one audio file to OpenRouter's Speech-to-Text endpoint and
//! returns the transcript. That endpoint is used because it is what serves a transcription model:
//! `input_audio` carries the base64 bytes of the file, and the caller declares the container format
//! alongside them, so a Telegram voice note's `ogg` needs no transcode step.
//!
//! The caller declares the [`AudioFormat`] because the caller is what knows the bytes: the audio
//! branch sends the downloaded file untouched and maps its MIME type, while the video branch
//! extracts the audio track to Ogg and passes [`AudioFormat::Ogg`] unconditionally.
//!
//! The client refuses audio above its own input ceiling before allocating, so the peak of one
//! request is the request body.
//!
//! The client records its own telemetry: the call counter and duration are captured for failed
//! calls too, and only this module knows that a call happened. Failures are returned to the caller,
//! which logs them once with the attachment they belong to.

use std::path::Path;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;
use tokio::io::AsyncReadExt;

use crate::infra::sentry;
use crate::shared::config::VariableName;
use crate::shared::result::{Error, Operation, Result};

/// Speech-to-Text endpoint.
const ENDPOINT: &str = "https://openrouter.ai/api/v1/audio/transcriptions";

/// Request timeout: long enough for a slow model, short enough to bound a stuck call.
const TIMEOUT: Duration = Duration::from_secs(120);

/// Characters of an error body kept for the logs.
const MAX_ERROR_BODY: usize = 500;

/// Largest audio this client will upload: the Bot API `getFile` ceiling, so no attachment can
/// exceed it.
const MAX_AUDIO_BYTES: u64 = 20 * 1024 * 1024;

/// Bytes read per encode step. A multiple of three, so every chunk but the last encodes without
/// padding.
const IO_CHUNK: usize = 3 * 32 * 1024;

/// The container name sent as `input_audio.format`.
///
/// The model receives exactly the bytes of the file, so the format has to match what the caller
/// wrote: the audio branch maps it from the Telegram MIME type, and the video branch always uses
/// [`Self::Ogg`] because that is what the extractor produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    /// `audio/ogg`, what a Telegram voice note carries and what the video branch extracts.
    Ogg,
    /// `audio/mpeg` or `audio/mp3`.
    Mp3,
    /// `audio/wav` or `audio/x-wav`.
    Wav,
    /// `audio/mp4`, `audio/m4a`, or `audio/x-m4a`.
    M4a,
    /// `audio/flac` or `audio/x-flac`.
    Flac,
    /// `audio/aac`.
    Aac,
    /// `audio/webm`.
    Webm,
}

impl AudioFormat {
    /// The container name OpenRouter expects.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ogg => "ogg",
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
            Self::M4a => "m4a",
            Self::Flac => "flac",
            Self::Aac => "aac",
            Self::Webm => "webm",
        }
    }

    /// Maps a Telegram MIME type to the container name the model will receive.
    ///
    /// Anything unknown falls back to [`Self::Ogg`]: a Telegram voice note is Ogg/Opus, which is the
    /// dominant case for this bot.
    pub fn from_mime_type(mime_type: Option<&str>) -> Self {
        match mime_type {
            Some("audio/ogg") => Self::Ogg,
            Some("audio/mpeg" | "audio/mp3") => Self::Mp3,
            Some("audio/wav" | "audio/x-wav") => Self::Wav,
            Some("audio/mp4" | "audio/m4a" | "audio/x-m4a") => Self::M4a,
            Some("audio/flac" | "audio/x-flac") => Self::Flac,
            Some("audio/aac") => Self::Aac,
            Some("audio/webm") => Self::Webm,
            _ => Self::Ogg,
        }
    }
}

/// Client for OpenRouter's Speech-to-Text endpoint.
pub struct OpenRouter {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

impl OpenRouter {
    /// Builds a client with a 120-second request timeout, using the key and the model the
    /// composition root resolved.
    ///
    /// Returns [`Error::EmptyConfiguration`] when either value is empty after trimming. Direct
    /// construction is the only way to reach that guard: configured values never arrive blank.
    pub fn new(api_key: String, model: String) -> Result<Self> {
        if api_key.trim().is_empty() {
            return Err(Error::EmptyConfiguration(VariableName::OpenRouterApiKey));
        }

        if model.trim().is_empty() {
            return Err(Error::EmptyConfiguration(VariableName::TranscribeModelName));
        }

        let client = reqwest::Client::builder().timeout(TIMEOUT).build()?;

        Ok(Self {
            client,
            api_key,
            model,
        })
    }

    /// Transcribes one audio file and returns the text the model produced.
    ///
    /// A file above [`MAX_AUDIO_BYTES`] is rejected with [`Error::AudioTooLarge`] before anything is
    /// allocated. One request, one transcript, no options that would change the answer. `format`
    /// describes the bytes the caller wrote; nothing here transcodes.
    /// The call counter and duration are recorded on every path; usage is recorded only when
    /// OpenRouter reports it. Failures are returned so the caller can log them with context.
    pub async fn transcribe_audio(&self, path: &Path, format: AudioFormat) -> Result<String> {
        let started = Instant::now();
        let result = self.request(path, format).await;
        let elapsed = started.elapsed();
        sentry::llm_call(elapsed);

        let (text, usage) = result?;
        if let Some(usage) = usage {
            sentry::llm_usage(
                usage.input_tokens,
                usage.output_tokens,
                usage.cost_micro_usd(),
            );
        }

        let usage = usage.unwrap_or_default();
        tracing::info!(
            model = %self.model,
            duration_ms = elapsed.as_millis() as u64,
            input_tokens = usage.input_tokens,
            output_tokens = usage.output_tokens,
            cost_micro_usd = usage.cost_micro_usd(),
            "Transcribed audio",
        );

        Ok(text)
    }

    async fn request(&self, path: &Path, format: AudioFormat) -> Result<(String, Option<Usage>)> {
        let body = request_body(path, &self.model, format).await?;

        let response = self
            .client
            .post(ENDPOINT)
            .bearer_auth(&self.api_key)
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            return Err(Error::HttpStatus {
                status: status.as_u16(),
                body: truncate(&body),
            });
        }

        parse_response(&body)
    }
}

/// Builds the body without holding the file or an encoded copy.
async fn request_body(path: &Path, model: &str, format: AudioFormat) -> Result<Vec<u8>> {
    let mut audio = tokio::fs::File::open(path)
        .await
        .map_err(|source| Error::ReadAudioFile {
            path: path.to_path_buf(),
            source,
        })?;
    let bytes = audio
        .metadata()
        .await
        .map_err(|source| Error::ReadAudioFile {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    if bytes > MAX_AUDIO_BYTES {
        return Err(Error::AudioTooLarge {
            bytes,
            max: MAX_AUDIO_BYTES,
        });
    }
    let bytes = usize::try_from(bytes).map_err(|_| Error::AudioTooLarge {
        bytes,
        max: MAX_AUDIO_BYTES,
    })?;

    // A JSON string literal: `Value`'s Display escapes it, so a model id cannot break the body.
    let model = serde_json::Value::String(model.to_owned()).to_string();
    let head = format!("{{\"model\":{model},\"input_audio\":{{\"data\":\"");
    let tail = format!(
        "\",\"format\":\"{}\"}},\"temperature\":0}}",
        format.as_str()
    );

    let mut body = Vec::with_capacity(head.len() + bytes.div_ceil(3) * 4 + tail.len());
    body.extend_from_slice(head.as_bytes());

    let mut chunk = vec![0u8; IO_CHUNK];
    let mut filled = 0;
    loop {
        // A read can be short, so fill the buffer before encoding: padding is only valid once,
        // at the end of the stream.
        let read =
            audio
                .read(&mut chunk[filled..])
                .await
                .map_err(|source| Error::ReadAudioFile {
                    path: path.to_path_buf(),
                    source,
                })?;
        if read == 0 {
            break;
        }
        filled += read;
        if filled == chunk.len() {
            append_encoded(&mut body, &chunk, path)?;
            filled = 0;
        }
    }
    if filled > 0 {
        append_encoded(&mut body, &chunk[..filled], path)?;
    }

    body.extend_from_slice(tail.as_bytes());

    Ok(body)
}

/// Appends `input` as base64; the destination is sized from it, so the only failure the cap leaves
/// is an input too large for the platform.
fn append_encoded(body: &mut Vec<u8>, input: &[u8], path: &Path) -> Result<()> {
    let encoded = input.len().div_ceil(3) * 4;
    let start = body.len();
    body.resize(start + encoded, 0);
    STANDARD
        .encode_slice(input, &mut body[start..])
        .map_err(|source| Error::ReadAudioFile {
            path: path.to_path_buf(),
            source: std::io::Error::other(source),
        })?;

    Ok(())
}

impl From<reqwest::Error> for Error {
    fn from(source: reqwest::Error) -> Self {
        Self::External {
            operation: Operation::OpenRouterRequest,
            source: Box::new(source),
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(source: serde_json::Error) -> Self {
        Self::External {
            operation: Operation::OpenRouterResponse,
            source: Box::new(source),
        }
    }
}

/// The part of a Speech-to-Text response this module reads.
#[derive(Deserialize)]
struct Response {
    /// Required by serde: an absent `text` means the body is not the shape expected. An empty
    /// string is valid, because a silent attachment legitimately transcribes to nothing.
    text: String,
    #[serde(default)]
    usage: Option<Usage>,
}

/// Token and cost accounting OpenRouter reports for a call.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    /// Credits in USD.
    #[serde(default)]
    cost: f64,
}

impl Usage {
    /// The reported cost in micro-USD, rounded.
    fn cost_micro_usd(self) -> f64 {
        (self.cost * 1_000_000.0).round()
    }
}

/// Reads the transcript and usage out of a successful response body.
///
/// The text is returned verbatim: trimming, and treating an empty transcript as "no speech", is the
/// caller's decision.
fn parse_response(body: &str) -> Result<(String, Option<Usage>)> {
    let response: Response = serde_json::from_str(body)?;

    Ok((response.text, response.usage))
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
    fn client_rejects_an_empty_key() {
        for key in ["", "   "] {
            assert!(matches!(
                OpenRouter::new(key.to_owned(), "model".to_owned()),
                Err(Error::EmptyConfiguration(name)) if name == VariableName::OpenRouterApiKey
            ));
        }
    }

    #[test]
    fn client_rejects_an_empty_model() {
        for model in ["", "   "] {
            assert!(matches!(
                OpenRouter::new("key".to_owned(), model.to_owned()),
                Err(Error::EmptyConfiguration(name)) if name == VariableName::TranscribeModelName
            ));
        }
    }

    #[test]
    fn client_accepts_a_key_and_model() {
        assert!(OpenRouter::new("key".to_owned(), "model".to_owned()).is_ok());
    }

    #[test]
    fn audio_formats_are_mapped_from_mime_types() {
        let cases = [
            (Some("audio/ogg"), AudioFormat::Ogg),
            (Some("audio/mpeg"), AudioFormat::Mp3),
            (Some("audio/mp3"), AudioFormat::Mp3),
            (Some("audio/wav"), AudioFormat::Wav),
            (Some("audio/x-wav"), AudioFormat::Wav),
            (Some("audio/mp4"), AudioFormat::M4a),
            (Some("audio/m4a"), AudioFormat::M4a),
            (Some("audio/x-m4a"), AudioFormat::M4a),
            (Some("audio/flac"), AudioFormat::Flac),
            (Some("audio/x-flac"), AudioFormat::Flac),
            (Some("audio/aac"), AudioFormat::Aac),
            (Some("audio/webm"), AudioFormat::Webm),
            (None, AudioFormat::Ogg),
            (Some("application/octet-stream"), AudioFormat::Ogg),
        ];

        for (mime_type, expected) in cases {
            assert_eq!(
                AudioFormat::from_mime_type(mime_type),
                expected,
                "mime type {mime_type:?}"
            );
        }
    }

    #[test]
    fn audio_formats_render_the_container_name() {
        let cases = [
            (AudioFormat::Ogg, "ogg"),
            (AudioFormat::Mp3, "mp3"),
            (AudioFormat::Wav, "wav"),
            (AudioFormat::M4a, "m4a"),
            (AudioFormat::Flac, "flac"),
            (AudioFormat::Aac, "aac"),
            (AudioFormat::Webm, "webm"),
        ];

        for (format, expected) in cases {
            assert_eq!(format.as_str(), expected, "format {format:?}");
        }
    }

    #[test]
    fn response_yields_the_transcript_and_usage() {
        let body = r#"{
            "text": "Привет, мир",
            "usage": {
                "input_tokens": 1200,
                "output_tokens": 42,
                "total_tokens": 1242,
                "seconds": 3,
                "cost": 0.000123
            }
        }"#;

        let (text, usage) = parse_response(body).expect("response should parse");
        assert_eq!(text, "Привет, мир");

        let usage = usage.expect("usage should be present");
        assert_eq!(usage.input_tokens, 1_200);
        assert_eq!(usage.output_tokens, 42);
        assert_eq!(usage.cost_micro_usd(), 123.0);
    }

    #[test]
    fn response_without_usage_still_yields_the_transcript() {
        let body = r#"{ "text": "hi" }"#;

        let (text, usage) = parse_response(body).expect("response should parse");
        assert_eq!(text, "hi");
        assert!(usage.is_none());
    }

    #[test]
    fn an_empty_transcript_is_returned_as_is() {
        let body = r#"{ "text": "" }"#;

        let (text, usage) = parse_response(body).expect("an empty transcript should parse");
        assert!(text.is_empty(), "text {text:?}");
        assert!(usage.is_none());
    }

    #[test]
    fn response_without_text_is_rejected() {
        for body in [
            r#"{ "usage": { "input_tokens": 1200 } }"#,
            r#"{ "text": null }"#,
            r#"{}"#,
        ] {
            assert!(
                matches!(
                    parse_response(body),
                    Err(Error::External {
                        operation: Operation::OpenRouterResponse,
                        ..
                    })
                ),
                "body {body}"
            );
        }
    }

    #[test]
    fn response_that_is_not_json_is_rejected() {
        assert!(matches!(
            parse_response("<html>bad gateway</html>"),
            Err(Error::External {
                operation: Operation::OpenRouterResponse,
                ..
            })
        ));
    }

    #[test]
    fn long_error_bodies_are_truncated() {
        let body = "x".repeat(MAX_ERROR_BODY * 2);
        assert_eq!(truncate(&body).chars().count(), MAX_ERROR_BODY);
        assert_eq!(truncate("short"), "short");
    }

    /// The body [`request_body`] wrote, read back. `data` borrows from it, so a maximum-size test
    /// parses the body without a second copy.
    #[derive(Deserialize)]
    struct Body<'a> {
        model: String,
        #[serde(borrow)]
        input_audio: InputAudio<'a>,
        temperature: u8,
    }

    #[derive(Deserialize)]
    struct InputAudio<'a> {
        data: &'a str,
        format: &'a str,
    }

    #[tokio::test]
    async fn body_carries_the_model_format_and_audio() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let path = directory.path().join("audio");
        let input = [0u8, 1, 2, 3, 4];
        std::fs::write(&path, input).expect("audio file should be written");

        let body = request_body(&path, "a\"b", AudioFormat::Ogg)
            .await
            .expect("body should be built");
        let body: Body<'_> = serde_json::from_slice(&body).expect("body should parse");

        assert_eq!(body.model, "a\"b");
        assert_eq!(body.input_audio.format, "ogg");
        assert_eq!(body.temperature, 0);
        assert_eq!(
            STANDARD
                .decode(body.input_audio.data)
                .expect("data should decode"),
            input
        );
    }

    #[tokio::test]
    async fn a_maximum_size_audio_file_is_accepted() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let path = directory.path().join("audio");
        // Sparse, so the test does not write 20 MB; only the reads touch the file.
        std::fs::File::create(&path)
            .expect("audio file should be created")
            .set_len(MAX_AUDIO_BYTES)
            .expect("audio file should be sized");

        let body = request_body(&path, "model", AudioFormat::Ogg)
            .await
            .expect("the maximum size should be accepted");
        let body: Body<'_> = serde_json::from_slice(&body).expect("body should parse");

        assert_eq!(
            body.input_audio.data.len() as u64,
            MAX_AUDIO_BYTES.div_ceil(3) * 4
        );
    }

    #[tokio::test]
    async fn audio_above_the_maximum_is_rejected() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let path = directory.path().join("audio");
        std::fs::File::create(&path)
            .expect("audio file should be created")
            .set_len(MAX_AUDIO_BYTES + 1)
            .expect("audio file should be sized");

        let error = request_body(&path, "model", AudioFormat::Ogg)
            .await
            .expect_err("a file above the maximum should be rejected");

        assert!(matches!(
            error,
            Error::AudioTooLarge { bytes, max }
                if bytes == MAX_AUDIO_BYTES + 1 && max == MAX_AUDIO_BYTES
        ));
    }

    #[tokio::test]
    async fn missing_file_is_reported_without_a_request() {
        let client =
            OpenRouter::new("key".to_owned(), "model".to_owned()).expect("client should build");
        let path = std::env::temp_dir().join("u502-openrouter-missing-audio");

        let error = client
            .transcribe_audio(&path, AudioFormat::Ogg)
            .await
            .expect_err("missing file should not transcribe");
        match error {
            Error::ReadAudioFile {
                path: reported,
                source,
            } => {
                assert_eq!(reported, path);
                assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
            }
            other => panic!("expected ReadAudioFile, got {other}"),
        }
    }
}
