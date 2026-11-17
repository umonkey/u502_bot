//! OpenRouter transcription client.
//!
//! [`OpenRouter::transcribe_audio`] sends one audio file to OpenRouter's Chat Completions endpoint
//! and returns the transcript. Chat Completions is used rather than the Responses API because its
//! `input_audio.format` accepts any provider-supported container name, including the `ogg` that a
//! Telegram voice note carries, so no transcode step and no `ffmpeg` in the image is needed.
//!
//! The client records its own telemetry: the call counter and duration are captured for failed
//! calls too, and only this module knows that a call happened. Failures are returned to the caller,
//! which logs them once with the attachment they belong to.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};

use crate::infra::sentry;

/// Environment variable holding the OpenRouter API key.
pub const OPENROUTER_API_KEY: &str = "OPENROUTER_API_KEY";

/// Environment variable naming the model used to transcribe.
pub const TRANSCRIBE_MODEL_NAME: &str = "TRANSCRIBE_MODEL_NAME";

/// Model used when [`TRANSCRIBE_MODEL_NAME`] is unset or empty.
pub const DEFAULT_TRANSCRIBE_MODEL: &str = "google/gemini-2.5-flash";

/// Chat Completions endpoint.
const ENDPOINT: &str = "https://openrouter.ai/api/v1/chat/completions";

/// Request timeout: long enough for a slow model, short enough to bound a stuck call.
const TIMEOUT: Duration = Duration::from_secs(120);

/// Output cap for one transcription.
const MAX_TOKENS: u32 = 4_096;

/// Characters of an error body kept for the logs.
const MAX_ERROR_BODY: usize = 500;

/// Resolves the transcription model from [`TRANSCRIBE_MODEL_NAME`].
pub fn model_from_env() -> String {
    resolve_model(std::env::var(TRANSCRIBE_MODEL_NAME).ok())
}

/// Resolves a raw model setting, falling back to [`DEFAULT_TRANSCRIBE_MODEL`] when it is absent,
/// empty, or whitespace-only.
///
/// A configured value is returned verbatim: only the emptiness check trims, so a model id an
/// operator set on purpose is never rewritten.
fn resolve_model(raw: Option<String>) -> String {
    match raw {
        Some(model) if !model.trim().is_empty() => model,
        _ => DEFAULT_TRANSCRIBE_MODEL.to_owned(),
    }
}

/// Failure modes of building a client or transcribing audio.
#[derive(Debug)]
pub enum OpenRouterError {
    /// The key or the model is empty after trimming, so the client cannot be used.
    EmptyConfig { name: &'static str },
    /// The request could not be sent, or its body could not be read.
    Request(reqwest::Error),
    /// OpenRouter answered with a non-success status.
    Status { status: u16, body: String },
    /// The audio file could not be read.
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The response body is not the JSON shape this module expects.
    Json(serde_json::Error),
    /// The response carries no transcript text.
    NoText,
}

impl fmt::Display for OpenRouterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyConfig { name } => write!(formatter, "{name} is empty or only whitespace"),
            Self::Request(source) => write!(formatter, "OpenRouter request failed: {source}"),
            Self::Status { status, body } => {
                write!(formatter, "OpenRouter returned HTTP {status}: {body}")
            }
            Self::ReadFile { path, source } => {
                write!(
                    formatter,
                    "failed to read audio file {}: {source}",
                    path.display()
                )
            }
            Self::Json(source) => {
                write!(
                    formatter,
                    "failed to decode the OpenRouter response: {source}"
                )
            }
            Self::NoText => write!(formatter, "OpenRouter returned no transcript text"),
        }
    }
}

impl std::error::Error for OpenRouterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Request(source) => Some(source),
            Self::ReadFile { source, .. } => Some(source),
            Self::Json(source) => Some(source),
            Self::EmptyConfig { .. } | Self::Status { .. } | Self::NoText => None,
        }
    }
}

impl From<reqwest::Error> for OpenRouterError {
    fn from(source: reqwest::Error) -> Self {
        Self::Request(source)
    }
}

impl From<serde_json::Error> for OpenRouterError {
    fn from(source: serde_json::Error) -> Self {
        Self::Json(source)
    }
}

/// Client for OpenRouter's Chat Completions endpoint.
pub struct OpenRouter {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

impl OpenRouter {
    /// Builds a client with a 120-second request timeout.
    ///
    /// Returns [`OpenRouterError::EmptyConfig`] when the key or the model is empty after trimming:
    /// an empty secret file must fail startup instead of producing a 401 on every transcription.
    pub fn new(api_key: String, model: String) -> Result<Self, OpenRouterError> {
        if api_key.trim().is_empty() {
            return Err(OpenRouterError::EmptyConfig {
                name: OPENROUTER_API_KEY,
            });
        }

        if model.trim().is_empty() {
            return Err(OpenRouterError::EmptyConfig {
                name: TRANSCRIBE_MODEL_NAME,
            });
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
    /// The file is read whole and base64-encoded, because that is what `input_audio` carries. One
    /// request, one choice, no tools, bounded output, and reasoning disabled keep it a single turn.
    /// The call counter and duration are recorded on every path; usage is recorded only when
    /// OpenRouter reports it. Failures are returned so the caller can log them with context.
    pub async fn transcribe_audio(
        &self,
        path: &Path,
        prompt: &str,
    ) -> Result<String, OpenRouterError> {
        let started = Instant::now();
        let result = self.request(path, prompt).await;
        let elapsed = started.elapsed();
        sentry::llm_call(elapsed);

        let (text, usage) = result?;
        if let Some(usage) = usage {
            sentry::llm_usage(
                usage.prompt_tokens,
                usage.completion_tokens,
                usage.cost_micro_usd(),
            );
        }

        let usage = usage.unwrap_or_default();
        tracing::info!(
            model = %self.model,
            duration_ms = elapsed.as_millis() as u64,
            input_tokens = usage.prompt_tokens,
            output_tokens = usage.completion_tokens,
            cost_micro_usd = usage.cost_micro_usd(),
            "Transcribed audio",
        );

        Ok(text)
    }

    async fn request(
        &self,
        path: &Path,
        prompt: &str,
    ) -> Result<(String, Option<Usage>), OpenRouterError> {
        let audio = tokio::fs::read(path)
            .await
            .map_err(|source| OpenRouterError::ReadFile {
                path: path.to_path_buf(),
                source,
            })?;

        let data = STANDARD.encode(audio);
        let request = Request {
            model: &self.model,
            messages: [Message {
                role: "user",
                content: [
                    Content::Text { text: prompt },
                    Content::Audio {
                        input_audio: InputAudio {
                            data: &data,
                            // A Telegram voice note is Ogg/Opus and carries no MIME type here.
                            format: audio_format(None),
                        },
                    },
                ],
            }],
            temperature: 0,
            max_tokens: MAX_TOKENS,
            // Models that declare reasoning mandatory reject this field; drop it for those.
            reasoning: Reasoning { enabled: false },
        };

        let response = self
            .client
            .post(ENDPOINT)
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            return Err(OpenRouterError::Status {
                status: status.as_u16(),
                body: truncate(&body),
            });
        }

        parse_response(&body)
    }
}

/// One Chat Completions request: a single user turn with the prompt and the audio.
#[derive(Serialize)]
struct Request<'a> {
    model: &'a str,
    messages: [Message<'a>; 1],
    temperature: u8,
    max_tokens: u32,
    reasoning: Reasoning,
}

#[derive(Serialize)]
struct Message<'a> {
    role: &'static str,
    content: [Content<'a>; 2],
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum Content<'a> {
    #[serde(rename = "text")]
    Text { text: &'a str },
    #[serde(rename = "input_audio")]
    Audio { input_audio: InputAudio<'a> },
}

#[derive(Serialize)]
struct InputAudio<'a> {
    data: &'a str,
    format: &'static str,
}

#[derive(Serialize)]
struct Reasoning {
    enabled: bool,
}

/// The part of a Chat Completions response this module reads.
#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: Option<String>,
}

/// Token and cost accounting OpenRouter reports for a call.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
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
fn parse_response(body: &str) -> Result<(String, Option<Usage>), OpenRouterError> {
    let response: Response = serde_json::from_str(body)?;

    let text = response
        .choices
        .first()
        .and_then(|choice| choice.message.content.as_deref())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or(OpenRouterError::NoText)?;

    Ok((text.to_owned(), response.usage))
}

/// Maps a Telegram MIME type to the container name OpenRouter expects.
///
/// Anything unknown falls back to `ogg`: a Telegram voice note is Ogg/Opus, which is the dominant
/// case for this bot.
fn audio_format(mime_type: Option<&str>) -> &'static str {
    match mime_type {
        Some("audio/ogg") => "ogg",
        Some("audio/mpeg" | "audio/mp3") => "mp3",
        Some("audio/wav" | "audio/x-wav") => "wav",
        Some("audio/mp4" | "audio/m4a" | "audio/x-m4a") => "m4a",
        Some("audio/flac" | "audio/x-flac") => "flac",
        Some("audio/aac") => "aac",
        Some("audio/webm") => "webm",
        _ => "ogg",
    }
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
    fn unset_model_resolves_to_the_default() {
        assert_eq!(resolve_model(None), DEFAULT_TRANSCRIBE_MODEL);
    }

    #[test]
    fn blank_model_resolves_to_the_default() {
        for raw in ["", "   ", "\n\t"] {
            assert_eq!(
                resolve_model(Some(raw.to_owned())),
                DEFAULT_TRANSCRIBE_MODEL
            );
        }
    }

    #[test]
    fn configured_model_is_kept_verbatim() {
        assert_eq!(
            resolve_model(Some("openai/whisper-1".to_owned())),
            "openai/whisper-1"
        );
    }

    #[test]
    fn client_rejects_an_empty_key() {
        for key in ["", "   "] {
            assert!(matches!(
                OpenRouter::new(key.to_owned(), DEFAULT_TRANSCRIBE_MODEL.to_owned()),
                Err(OpenRouterError::EmptyConfig { name }) if name == OPENROUTER_API_KEY
            ));
        }
    }

    #[test]
    fn client_rejects_an_empty_model() {
        for model in ["", "   "] {
            assert!(matches!(
                OpenRouter::new("key".to_owned(), model.to_owned()),
                Err(OpenRouterError::EmptyConfig { name }) if name == TRANSCRIBE_MODEL_NAME
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
            (Some("audio/ogg"), "ogg"),
            (Some("audio/mpeg"), "mp3"),
            (Some("audio/mp3"), "mp3"),
            (Some("audio/wav"), "wav"),
            (Some("audio/x-wav"), "wav"),
            (Some("audio/mp4"), "m4a"),
            (Some("audio/m4a"), "m4a"),
            (Some("audio/x-m4a"), "m4a"),
            (Some("audio/flac"), "flac"),
            (Some("audio/x-flac"), "flac"),
            (Some("audio/aac"), "aac"),
            (Some("audio/webm"), "webm"),
            (None, "ogg"),
            (Some("application/octet-stream"), "ogg"),
        ];

        for (mime_type, expected) in cases {
            assert_eq!(audio_format(mime_type), expected, "mime type {mime_type:?}");
        }
    }

    #[test]
    fn response_yields_the_transcript_and_usage() {
        let body = r#"{
            "id": "gen-1",
            "model": "google/gemini-2.5-flash",
            "choices": [
                { "index": 0, "message": { "role": "assistant", "content": "Привет, мир" }, "finish_reason": "stop" }
            ],
            "usage": { "prompt_tokens": 1200, "completion_tokens": 42, "total_tokens": 1242, "cost": 0.000123 }
        }"#;

        let (text, usage) = parse_response(body).expect("response should parse");
        assert_eq!(text, "Привет, мир");

        let usage = usage.expect("usage should be present");
        assert_eq!(usage.prompt_tokens, 1_200);
        assert_eq!(usage.completion_tokens, 42);
        assert_eq!(usage.cost_micro_usd(), 123.0);
    }

    #[test]
    fn response_without_usage_still_yields_the_transcript() {
        let body = r#"{ "choices": [{ "message": { "content": "hi" } }] }"#;

        let (text, usage) = parse_response(body).expect("response should parse");
        assert_eq!(text, "hi");
        assert!(usage.is_none());
    }

    #[test]
    fn response_without_content_is_rejected() {
        for body in [
            r#"{ "choices": [{ "message": { "content": null } }] }"#,
            r#"{ "choices": [{ "message": { "content": "   " } }] }"#,
            r#"{ "choices": [] }"#,
            r#"{}"#,
        ] {
            assert!(
                matches!(parse_response(body), Err(OpenRouterError::NoText)),
                "body {body}"
            );
        }
    }

    #[test]
    fn response_that_is_not_json_is_rejected() {
        assert!(matches!(
            parse_response("<html>bad gateway</html>"),
            Err(OpenRouterError::Json(_))
        ));
    }

    #[test]
    fn long_error_bodies_are_truncated() {
        let body = "x".repeat(MAX_ERROR_BODY * 2);
        assert_eq!(truncate(&body).chars().count(), MAX_ERROR_BODY);
        assert_eq!(truncate("short"), "short");
    }

    #[tokio::test]
    async fn missing_file_is_reported_without_a_request() {
        let client =
            OpenRouter::new("key".to_owned(), "model".to_owned()).expect("client should build");
        let path = std::env::temp_dir().join("u502-openrouter-missing-audio");

        let error = client
            .transcribe_audio(&path, "Transcribe.")
            .await
            .expect_err("missing file should not transcribe");
        match error {
            OpenRouterError::ReadFile {
                path: reported,
                source,
            } => {
                assert_eq!(reported, path);
                assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
            }
            other => panic!("expected ReadFile, got {other}"),
        }
    }
}
