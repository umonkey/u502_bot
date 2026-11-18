//! Transcription service: turns pending attachments into outbox rows.
//!
//! The service runs beside the ingestor, so ingestion never waits on a model call. Every scan reads
//! a small batch of pending attachments — audio first, then video — whose message is at most an hour
//! old. An audio attachment is downloaded as-is and sent to OpenRouter's Speech-to-Text API; a video
//! attachment is downloaded, its audio track is extracted with ffmpeg, and the resulting Ogg joins
//! the audio flow. Either way the transcript is queued in the outbox and the attachment is marked
//! processed. Those two writes are one transaction committed after the model call. The bot also
//! reacts to the message with 👀 when it picks the attachment up, so the chat sees work in
//! progress; a reaction that fails only logs a warning, because it must never cost a transcript.
//!
//! The one-hour window bounds both cost and retries: a file that keeps failing is retried on every
//! scan until its message ages out, and then left alone. The whitelist the composition root resolved
//! limits which chats are transcribed at all, because a transcription spends credits and sends audio
//! to a third party. An empty one transcribes nothing and the skip warnings publish the chat ids an
//! operator needs before the list is known.

use std::time::Duration;

use crate::domains::attachment::{AttachmentKind, AttachmentRepository, PendingAttachment};
use crate::domains::outbox::{EnqueueOutcome, NewOutboxMessage, OutboxRepository};
use crate::infra::config::Whitelist;
use crate::infra::ffmpeg::Ffmpeg;
use crate::infra::openrouter::{AudioFormat, OpenRouter};
use crate::infra::sentry;
use crate::infra::telegram_bot::Telegram;
use crate::services::shutdown::Shutdown;
use crate::services::unix_now;
use crate::services::wake::Wake;
use crate::shared::result::Error;

/// Plain-text header placed above the transcript in the first reply.
const REPLY_HEADER: &str = "Расшифровка:";

/// Only messages at most this old are transcribed, which bounds cost and retries.
const WINDOW_SECONDS: i64 = 3_600;

/// Attachments read per scan.
const BATCH: i64 = 10;

/// The Bot API refuses `getFile` above 20 MB, so a larger attachment can never be downloaded; it is
/// skipped and marked processed instead of failing once per scan for the length of the window.
const MAX_DOWNLOAD_BYTES: i64 = 20 * 1024 * 1024;

/// Reaction set on a message as soon as its attachment is picked up.
///
/// It marks pickup before the download and the model call, which are the slow part, so a chat can
/// tell a queued attachment from one the bot never saw.
const PROCESSING_REACTION: &str = "👀";

/// Pause after a scan that found nothing, or that could not do any work.
const IDLE: Duration = Duration::from_secs(5);

/// Maximum visible characters per outbox row: Telegram rejects a message longer than 4096.
///
/// The limit budgets the text Telegram counts after entity parsing, not the markup, so chunking
/// stays on the raw transcript and escaping is applied per chunk, where a chunk boundary can never
/// split a tag.
const CHUNK_LIMIT: usize = 4_000;

/// Transcribes pending audio and video attachments and queues the transcripts for delivery.
pub struct Transcriber {
    telegram: Telegram,
    openrouter: OpenRouter,
    ffmpeg: Ffmpeg,
    attachments: AttachmentRepository,
    outbox: OutboxRepository,
    chats: Whitelist,
    pending_attachments: Wake,
    pending_replies: Wake,
}

impl Transcriber {
    /// Creates the service over the clients and repositories the command composed.
    // One parameter per client, repository, whitelist, and wake edge: the wiring is the whole
    // constructor, so grouping the inputs would only hide it.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        telegram: Telegram,
        openrouter: OpenRouter,
        ffmpeg: Ffmpeg,
        attachments: AttachmentRepository,
        outbox: OutboxRepository,
        chats: Whitelist,
        pending_attachments: Wake,
        pending_replies: Wake,
    ) -> Self {
        Self {
            telegram,
            openrouter,
            ffmpeg,
            attachments,
            outbox,
            chats,
            pending_attachments,
            pending_replies,
        }
    }

    /// Transcribes pending attachments until shutdown is requested, pausing only when there is
    /// nothing to do.
    ///
    /// The pause ends early when the ingestor stores an attachment, so a fresh voice note or video
    /// does not wait out [`IDLE`]; a failing attachment is still retried on the pause's cadence.
    pub async fn run(mut self, mut shutdown: Shutdown) {
        while !shutdown.is_requested() {
            if !self.tick(&mut shutdown).await
                && !self.pending_attachments.sleep(IDLE, &mut shutdown).await
            {
                break;
            }
        }

        tracing::info!("Stopped the transcriber");
    }

    /// Processes one batch of each kind and returns whether any attachment was finished.
    ///
    /// Audio is scanned before video so a known-good path runs first; because the two scans are
    /// separate batches, a permanently failing audio backlog cannot starve video.
    ///
    /// The pause lives in the caller, where it is interruptible: a failed scan and a batch in which
    /// every row failed both return `false`, so a file that keeps failing or a pool that rejects a
    /// read is retried once per [`IDLE`] instead of as fast as the failure returns.
    async fn tick(&self, shutdown: &mut Shutdown) -> bool {
        let now = unix_now();
        let mut processed = false;

        for kind in [AttachmentKind::Audio, AttachmentKind::Video] {
            if shutdown.is_requested() {
                break;
            }

            processed |= self.scan(kind, now, shutdown).await;
        }

        processed
    }

    /// Scans one kind and processes its batch; returns whether any attachment was finished.
    async fn scan(&self, kind: AttachmentKind, now: i64, shutdown: &mut Shutdown) -> bool {
        let pending = match self
            .attachments
            .pending(kind, now - WINDOW_SECONDS, BATCH)
            .await
        {
            Ok(pending) => pending,
            Err(error) => {
                let message = match kind {
                    AttachmentKind::Audio => "Failed to scan for pending audio",
                    AttachmentKind::Video => "Failed to scan for pending video",
                };
                tracing::error!(
                    error = &error as &dyn std::error::Error,
                    kind = kind.as_str(),
                    "{message}"
                );

                return false;
            }
        };

        let mut processed = false;

        for attachment in pending {
            if shutdown.is_requested() {
                break;
            }

            processed |= self.process(attachment, now, shutdown).await;
        }

        processed
    }

    /// Transcribes one attachment and queues its transcript; returns whether it was finished.
    ///
    /// No database transaction spans the download or the LLM call: each statement returns the pooled
    /// connection immediately, so holding one would stall the ingestor for the length of the call.
    /// A shutdown interrupts any of those calls by dropping it; the attachment stays pending and is
    /// retried on the next start, while the writes that follow are never cancelled.
    async fn process(
        &self,
        attachment: PendingAttachment,
        now: i64,
        shutdown: &mut Shutdown,
    ) -> bool {
        if !self.chats.contains(attachment.chat_id) {
            tracing::warn!(
                chat_id = attachment.chat_id,
                kind = attachment.kind.as_str(),
                "Attachment from chat {} skipped: not a white listed chat, add to TRANSCRIBE_CHATS if needed.",
                attachment.chat_id
            );

            return self.mark_processed(&attachment).await;
        }

        if let Some(file_size) = attachment.file_size
            && file_size > MAX_DOWNLOAD_BYTES
        {
            tracing::warn!(
                attachment_id = attachment.id,
                chat_id = attachment.chat_id,
                message_id = attachment.message_id,
                kind = attachment.kind.as_str(),
                file_size,
                max_download_bytes = MAX_DOWNLOAD_BYTES,
                "Skipped an attachment: {file_size} bytes is above the Bot API limit of {MAX_DOWNLOAD_BYTES} bytes"
            );

            return self.mark_processed(&attachment).await;
        }

        let reacted = tokio::select! {
            result = self.telegram.react(
                attachment.chat_id,
                attachment.message_id,
                PROCESSING_REACTION,
            ) => result,
            () = shutdown.wait() => return false,
        };
        if let Err(error) = reacted {
            tracing::warn!(
                attachment_id = attachment.id,
                chat_id = attachment.chat_id,
                message_id = attachment.message_id,
                error = &error as &dyn std::error::Error,
                "Failed to react to a message: the transcript is unaffected"
            );
        }

        match attachment.kind {
            AttachmentKind::Audio => self.process_audio(&attachment, now, shutdown).await,
            AttachmentKind::Video => self.process_video(&attachment, now, shutdown).await,
        }
    }

    /// Transcribes the bytes Telegram stored for an audio attachment.
    ///
    /// The downloaded file is sent untouched, so the declared [`AudioFormat`] is whatever MIME type
    /// Telegram reported; only the video branch extracts and re-encodes.
    async fn process_audio(
        &self,
        attachment: &PendingAttachment,
        now: i64,
        shutdown: &mut Shutdown,
    ) -> bool {
        let directory = match tempfile::tempdir() {
            Ok(directory) => directory,
            Err(error) => {
                log_failure(attachment, &error);

                return false;
            }
        };
        let path = directory.path().join("audio");

        let downloaded = tokio::select! {
            result = self.telegram.download(&attachment.file_id, &path) => result,
            () = shutdown.wait() => return false,
        };
        if let Err(error) = downloaded {
            log_failure(attachment, &error);

            return false;
        }

        let transcribed = tokio::select! {
            result = self.openrouter.transcribe_audio(
                &path,
                AudioFormat::from_mime_type(attachment.mime_type.as_deref()),
            ) => result,
            () = shutdown.wait() => return false,
        };
        let transcript = match transcribed {
            Ok(transcript) => transcript,
            Err(error) => {
                log_failure(attachment, &error);

                return false;
            }
        };

        self.finish(attachment, &transcript, now).await
    }

    /// Transcribes a video attachment by extracting its audio track first.
    ///
    /// The extracted stream is fixed at Ogg/Opus 16 kHz mono, so the request declares
    /// [`AudioFormat::Ogg`] unconditionally. A video with no audio track or a host without ffmpeg is
    /// skipped and marked processed: retrying it could not change the outcome, and the window would
    /// keep paying for it.
    async fn process_video(
        &self,
        attachment: &PendingAttachment,
        now: i64,
        shutdown: &mut Shutdown,
    ) -> bool {
        let directory = match tempfile::tempdir() {
            Ok(directory) => directory,
            Err(error) => {
                log_failure(attachment, &error);

                return false;
            }
        };
        let video = directory.path().join("video");

        let downloaded = tokio::select! {
            result = self.telegram.download(&attachment.file_id, &video) => result,
            () = shutdown.wait() => return false,
        };
        if let Err(error) = downloaded {
            log_failure(attachment, &error);

            return false;
        }

        let audio = directory.path().join("audio.ogg");
        let extracted = tokio::select! {
            result = self.ffmpeg.extract_audio(&video, &audio) => result,
            () = shutdown.wait() => return false,
        };
        if let Err(error) = extracted {
            match &error {
                Error::FfmpegUnavailable(_) | Error::NoAudioStream { .. } => {
                    tracing::warn!(
                        attachment_id = attachment.id,
                        chat_id = attachment.chat_id,
                        message_id = attachment.message_id,
                        error = &error as &dyn std::error::Error,
                        "Skipped a video attachment: ffmpeg cannot extract its audio track"
                    );

                    return self.mark_processed(attachment).await;
                }
                _ => {
                    log_failure(attachment, &error);

                    return false;
                }
            }
        }

        let transcribed = tokio::select! {
            result = self.openrouter.transcribe_audio(&audio, AudioFormat::Ogg) => result,
            () = shutdown.wait() => return false,
        };
        let transcript = match transcribed {
            Ok(transcript) => transcript,
            Err(error) => {
                log_failure(attachment, &error);

                return false;
            }
        };

        self.finish(attachment, &transcript, now).await
    }

    /// Queues a transcript and marks the attachment processed in one transaction.
    async fn finish(&self, attachment: &PendingAttachment, transcript: &str, now: i64) -> bool {
        let transcript = transcript.trim();

        if transcript.is_empty() {
            tracing::info!(
                attachment_id = attachment.id,
                chat_id = attachment.chat_id,
                message_id = attachment.message_id,
                kind = attachment.kind.as_str(),
                "Transcribed an attachment with no speech"
            );

            return self.mark_processed(attachment).await;
        }

        let messages = replies(attachment, transcript, now);

        match self
            .outbox
            .enqueue_transcript(attachment.id, &messages)
            .await
        {
            Ok(EnqueueOutcome::Enqueued) => {
                for _ in &messages {
                    sentry::outbox_added();
                }
                sentry::attachment_transcribed();
                // The outbox is idle-waiting on this edge, so one pulse covers the whole commit.
                self.pending_replies.notify();

                true
            }
            Ok(EnqueueOutcome::AlreadyProcessed) => {
                tracing::info!(
                    attachment_id = attachment.id,
                    "Attachment was already processed; not queueing its transcript again"
                );

                true
            }
            Err(error) => {
                log_failure(attachment, &error);

                false
            }
        }
    }

    /// Marks one attachment processed and counts it, on every path that finishes with it.
    async fn mark_processed(&self, attachment: &PendingAttachment) -> bool {
        match self.attachments.mark_transcribed(attachment.id).await {
            Ok(()) => {
                sentry::attachment_transcribed();

                true
            }
            Err(error) => {
                tracing::error!(
                    attachment_id = attachment.id,
                    kind = attachment.kind.as_str(),
                    error = &error as &dyn std::error::Error,
                    "Failed to mark an attachment processed"
                );

                false
            }
        }
    }
}

/// Logs one attachment that could not be processed; it stays pending, so the next scan retries it.
fn log_failure(attachment: &PendingAttachment, error: &(dyn std::error::Error + 'static)) {
    tracing::error!(
        attachment_id = attachment.id,
        chat_id = attachment.chat_id,
        message_id = attachment.message_id,
        kind = attachment.kind.as_str(),
        error = error,
        "Failed to process an attachment"
    );
}

/// Escapes the characters Telegram's HTML parse mode treats as markup.
///
/// The ampersand is replaced first, so the entities introduced by the later replacements are not
/// escaped twice. Everything else is left untouched.
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Renders one chunk as Telegram HTML: the header above the transcript, the transcript quoted.
///
/// Only the first chunk carries [`REPLY_HEADER`]; the rest are the bare blockquote. The transcript
/// is escaped, so its content can never inject markup.
fn reply_html(chunk: &str, first: bool) -> String {
    if first {
        format!(
            "{REPLY_HEADER}\n<blockquote>{}</blockquote>",
            escape_html(chunk)
        )
    } else {
        format!("<blockquote>{}</blockquote>", escape_html(chunk))
    }
}

/// Renders a transcript as outbox rows, answering only the first chunk so the thread stays on it.
fn replies(attachment: &PendingAttachment, transcript: &str, now: i64) -> Vec<NewOutboxMessage> {
    chunks(transcript, CHUNK_LIMIT)
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| NewOutboxMessage {
            chat_id: attachment.chat_id,
            thread_id: (index == 0).then_some(attachment.message_id),
            added_at: now,
            message: reply_html(chunk, index == 0),
        })
        .collect()
}

/// Splits `text` into pieces of at most `limit` bytes without cutting a character in half.
fn chunks(text: &str, limit: usize) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = 0;

    while start < text.len() {
        let mut end = (start + limit).min(text.len());

        while !text.is_char_boundary(end) {
            end -= 1;
        }

        // A limit smaller than the next character still has to make progress.
        if end == start {
            end = start + text[start..].chars().next().map_or(0, char::len_utf8);
        }

        chunks.push(&text[start..end]);
        start = end;
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_download_cap_is_the_bot_api_limit() {
        assert_eq!(MAX_DOWNLOAD_BYTES, 20 * 1024 * 1024);
    }

    #[test]
    fn escape_html_escapes_markup_characters() {
        assert_eq!(escape_html("&"), "&amp;");
        assert_eq!(escape_html("<"), "&lt;");
        assert_eq!(escape_html(">"), "&gt;");
    }

    #[test]
    fn escape_html_escapes_the_ampersand_first() {
        assert_eq!(escape_html("a & b < c > d"), "a &amp; b &lt; c &gt; d");
        assert_eq!(escape_html("&lt;"), "&amp;lt;");
    }

    #[test]
    fn reply_html_renders_the_first_chunk_with_the_header() {
        assert_eq!(
            reply_html("Hello guys I am leaving", true),
            "Расшифровка:\n<blockquote>Hello guys I am leaving</blockquote>"
        );
    }

    #[test]
    fn reply_html_renders_a_continuation_chunk_without_the_header() {
        assert_eq!(
            reply_html("and another thing", false),
            "<blockquote>and another thing</blockquote>"
        );
    }

    #[test]
    fn reply_html_renders_a_transcript_containing_markup_inert() {
        assert_eq!(
            reply_html("</blockquote><b>x</b>", true),
            "Расшифровка:\n<blockquote>&lt;/blockquote&gt;&lt;b&gt;x&lt;/b&gt;</blockquote>"
        );
    }

    fn pending_attachment() -> PendingAttachment {
        PendingAttachment {
            id: 1,
            chat_id: 42,
            message_id: 7,
            kind: AttachmentKind::Audio,
            file_id: "file-1".to_owned(),
            mime_type: None,
            file_size: None,
        }
    }

    #[test]
    fn replies_marks_only_the_first_chunk_as_a_reply() {
        let attachment = pending_attachment();

        let messages = replies(&attachment, "Hello guys I am leaving", 1_700_000_000);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].chat_id, 42);
        assert_eq!(messages[0].thread_id, Some(7));
        assert_eq!(messages[0].added_at, 1_700_000_000);
        assert!(messages[0].message.starts_with(REPLY_HEADER));
    }

    #[test]
    fn replies_renders_one_header_for_a_long_transcript() {
        let attachment = pending_attachment();
        let transcript = "x".repeat(CHUNK_LIMIT + 1);

        let messages = replies(&attachment, &transcript, 1_700_000_000);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].thread_id, Some(7));
        assert!(messages[0].message.contains(REPLY_HEADER));
        assert_eq!(messages[1].thread_id, None);
        assert!(!messages[1].message.contains(REPLY_HEADER));
    }

    #[test]
    fn replies_escapes_the_transcript() {
        let attachment = pending_attachment();

        let messages = replies(&attachment, "a <b> c", 1_700_000_000);

        assert_eq!(messages.len(), 1);
        assert!(messages[0].message.contains("&lt;b&gt;"));
    }

    #[test]
    fn chunks_splits_ascii_text_at_the_limit() {
        assert_eq!(chunks("abcdef", 2), vec!["ab", "cd", "ef"]);
    }

    #[test]
    fn chunks_keeps_text_within_the_limit_in_one_piece() {
        assert_eq!(chunks("abcde", 5), vec!["abcde"]);
        assert_eq!(chunks("abc", 5), vec!["abc"]);
    }

    #[test]
    fn chunks_returns_nothing_for_empty_text() {
        assert!(chunks("", 5).is_empty());
    }

    #[test]
    fn chunks_never_splits_a_multibyte_character() {
        let text = "привет".repeat(2);

        let chunks = chunks(&text, 5);

        assert_eq!(chunks.concat(), text);
        for chunk in &chunks {
            assert!(!chunk.is_empty(), "a chunk must make progress");
            assert!(chunk.len() <= 5, "chunk {chunk:?} exceeds the limit");
        }
    }

    #[test]
    fn chunks_progress_even_below_one_character() {
        let chunks = chunks("привет", 1);

        assert_eq!(chunks.concat(), "привет");
        assert_eq!(chunks.len(), 6);
    }
}
