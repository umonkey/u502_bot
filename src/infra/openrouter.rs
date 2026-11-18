//! OpenRouter transcription client.
//!
//! [`OpenRouter::transcribe_audio`] sends one audio file to OpenRouter's Chat Completions endpoint
//! and returns the transcript. Chat Completions is used rather than the Responses API because its
//! `input_audio.format` accepts any provider-supported container name, so a Telegram voice note's
//! `ogg` needs no transcode step.
//!
//! The caller declares the [`AudioFormat`] because the caller is what knows the bytes: the audio
//! branch sends the downloaded file untouched and maps its MIME type, while the video branch
//! extracts the audio track to Ogg and passes [`AudioFormat::Ogg`] unconditionally.
//!
//! The client records its own telemetry: the call counter and duration are captured for failed
//! calls too, and only this module knows that a call happened. Failures are returned to the caller,
//! which logs them once with the attachment they belong to.

use std::path::Path;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};

use crate::infra::config::{VariableName, get_optional};
use crate::infra::sentry;
use crate::shared::result::{Error, Result};

/// Model used when [`VariableName::TranscribeModelName`] is unset or empty.
pub const DEFAULT_TRANSCRIBE_MODEL: &str = "google/gemini-2.5-flash";

/// Chat Completions endpoint.
const ENDPOINT: &str = "https://openrouter.ai/api/v1/chat/completions";

/// Request timeout: long enough for a slow model, short enough to bound a stuck call.
const TIMEOUT: Duration = Duration::from_secs(120);

/// Output cap for one transcription.
const MAX_TOKENS: u32 = 4_096;

/// Characters of an error body kept for the logs.
const MAX_ERROR_BODY: usize = 500;

/// Resolves the transcription model from [`VariableName::TranscribeModelName`].
///
/// An absent value falls back to [`DEFAULT_TRANSCRIBE_MODEL`]. A value that cannot be read, such as
/// a broken `<NAME>_FILE`, logs one warning and falls back the same way.
pub fn model_from_env() -> String {
    match get_optional(VariableName::TranscribeModelName) {
        Ok(model) => resolve_model(model),
        Err(error) => {
            tracing::warn!(
                error = &error as &dyn std::error::Error,
                variable = %VariableName::TranscribeModelName,
                "Cannot read the transcription model; falling back to {}",
                DEFAULT_TRANSCRIBE_MODEL
            );
            resolve_model(None)
        }
    }
}

/// Resolves a raw model setting, falling back to [`DEFAULT_TRANSCRIBE_MODEL`] when it is absent.
///
/// A configured value is returned verbatim: the resolver already trimmed it and rejected blank
/// values, so a model id an operator set on purpose is never rewritten.
fn resolve_model(raw: Option<String>) -> String {
    raw.unwrap_or_else(|| DEFAULT_TRANSCRIBE_MODEL.to_owned())
}

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

/// Client for OpenRouter's Chat Completions endpoint.
pub struct OpenRouter {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

impl OpenRouter {
    /// Builds a client with a 120-second request timeout.
    ///
    /// Returns [`Error::EmptyConfiguration`] when the key or the model is empty after trimming, so a
    /// directly constructed client cannot send a request that is doomed to a 401. Configured values
    /// never arrive blank: the resolver trims them and treats blank as missing.
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
    /// The file is read whole and base64-encoded, because that is what `input_audio` carries. One
    /// request, one choice, no tools, bounded output, and reasoning disabled keep it a single turn.
    /// `format` describes the bytes the caller wrote; nothing here transcodes.
    /// The call counter and duration are recorded on every path; usage is recorded only when
    /// OpenRouter reports it. Failures are returned so the caller can log them with context.
    pub async fn transcribe_audio(
        &self,
        path: &Path,
        prompt: &str,
        format: AudioFormat,
    ) -> Result<String> {
        let started = Instant::now();
        let result = self.request(path, prompt, format).await;
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
        format: AudioFormat,
    ) -> Result<(String, Option<Usage>)> {
        let audio = tokio::fs::read(path)
            .await
            .map_err(|source| Error::ReadAudioFile {
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
                            // The caller downloaded or extracted these bytes, so it knows the format.
                            format: format.as_str(),
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
            return Err(Error::HttpStatus {
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
fn parse_response(body: &str) -> Result<(String, Option<Usage>)> {
    let response: Response = serde_json::from_str(body)?;

    let text = response
        .choices
        .first()
        .and_then(|choice| choice.message.content.as_deref())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or(Error::MissingTranscript)?;

    Ok((text.to_owned(), response.usage))
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
                matches!(parse_response(body), Err(Error::MissingTranscript)),
                "body {body}"
            );
        }
    }

    #[test]
    fn response_that_is_not_json_is_rejected() {
        assert!(matches!(
            parse_response("<html>bad gateway</html>"),
            Err(Error::ResponseJson(_))
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
            .transcribe_audio(&path, "Transcribe.", AudioFormat::Ogg)
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
