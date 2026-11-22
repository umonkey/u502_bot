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
//! scan until its message ages out, and then left alone. A failure a retry could not change — a
//! video with no audio track, a host without ffmpeg, or audio above the client's input limit — is
//! skipped and marked processed instead. Only the chats the owner approved are transcribed, because
//! a transcription spends credits and sends audio to a third party: the approved set is read once per
//! tick, a failed read is logged and retried on the next tick, and an unapproved chat is skipped and
//! marked processed.

use std::collections::HashSet;

use crate::domains::attachment::{AttachmentKind, AttachmentRepository, PendingAttachment};
use crate::domains::chat::ChatRepository;
use crate::domains::outbox::{EnqueueOutcome, NewOutboxMessage, OutboxRepository};
use crate::infra::ffmpeg::Ffmpeg;
use crate::infra::openrouter::{AudioFormat, OpenRouter};
use crate::infra::sentry;
use crate::infra::telegram_bot::Telegram;
use crate::services::shutdown::Shutdown;
use crate::services::wake::{WakeReceiver, WakeSender};
use crate::services::{BATCH, IDLE, unix_now};
use crate::shared::result::{Error, Result};
use crate::shared::text::{TELEGRAM_MESSAGE_LIMIT, chunks, escape_html};

/// Plain-text header placed above the transcript in the first reply.
const REPLY_HEADER: &str = "Расшифровка:";

/// Only messages at most this old are transcribed, which bounds cost and retries.
const WINDOW_SECONDS: i64 = 3_600;

/// The Bot API refuses `getFile` above 20 MB, so a larger attachment can never be downloaded; it is
/// skipped and marked processed instead of failing once per scan for the length of the window.
const MAX_DOWNLOAD_BYTES: i64 = 20 * 1024 * 1024;

/// Reaction set on a message as soon as its attachment is picked up.
///
/// It marks pickup before the download and the model call, which are the slow part, so a chat can
/// tell a queued attachment from one the bot never saw.
const PROCESSING_REACTION: &str = "👀";

/// Transcribes pending audio and video attachments and queues the transcripts for delivery.
pub struct Transcriber {
    telegram: Telegram,
    openrouter: OpenRouter,
    ffmpeg: Ffmpeg,
    attachments: AttachmentRepository,
    outbox: OutboxRepository,
    chats: ChatRepository,
    pending_attachments: WakeReceiver,
    pending_replies: WakeSender,
}

impl Transcriber {
    /// Creates the service over the clients and repositories the command composed.
    // One parameter per client, repository, and wake edge: the wiring is the whole constructor, so
    // grouping the inputs would only hide it.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        telegram: Telegram,
        openrouter: OpenRouter,
        ffmpeg: Ffmpeg,
        attachments: AttachmentRepository,
        outbox: OutboxRepository,
        chats: ChatRepository,
        pending_attachments: WakeReceiver,
        pending_replies: WakeSender,
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
        let approved = match self.chats.approved().await {
            Ok(approved) => approved,
            Err(error) => {
                tracing::error!(
                    error = &error as &dyn std::error::Error,
                    "Failed to read the approved chats"
                );

                return false;
            }
        };
        let mut processed = false;

        for kind in [AttachmentKind::Audio, AttachmentKind::Video] {
            if shutdown.is_requested() {
                break;
            }

            processed |= self.scan(kind, &approved, now, shutdown).await;
        }

        processed
    }

    /// Scans one kind and processes its batch; returns whether any attachment was finished.
    async fn scan(
        &self,
        kind: AttachmentKind,
        approved: &HashSet<i64>,
        now: i64,
        shutdown: &mut Shutdown,
    ) -> bool {
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

            processed |= self.process(attachment, approved, now, shutdown).await;
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
        approved: &HashSet<i64>,
        now: i64,
        shutdown: &mut Shutdown,
    ) -> bool {
        if !approved.contains(&attachment.chat_id) {
            tracing::warn!(
                chat_id = attachment.chat_id,
                kind = attachment.kind.as_str(),
                "Attachment from chat {} skipped: the chat is not approved, use /approve {} from the owner's direct messages.",
                attachment.chat_id,
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
            AttachmentKind::Audio => self.process_audio(attachment, now, shutdown).await,
            AttachmentKind::Video => self.process_video(attachment, now, shutdown).await,
        }
    }

    /// Transcribes the bytes Telegram stored for an audio attachment.
    ///
    /// The downloaded file is sent untouched, so the declared [`AudioFormat`] is whatever MIME type
    /// Telegram reported; only the video branch extracts and re-encodes.
    async fn process_audio(
        &self,
        attachment: PendingAttachment,
        now: i64,
        shutdown: &mut Shutdown,
    ) -> bool {
        let directory = match TemporaryDirectory::create().await {
            Ok(directory) => directory,
            Err(error) => {
                log_failure(&attachment, &error);

                return false;
            }
        };

        let outcome = self
            .process_audio_file(&attachment, &directory.path().join("audio"), now, shutdown)
            .await;
        directory.remove().await;

        outcome
    }

    async fn process_audio_file(
        &self,
        attachment: &PendingAttachment,
        path: &std::path::Path,
        now: i64,
        shutdown: &mut Shutdown,
    ) -> bool {
        let downloaded = tokio::select! {
            result = self.telegram.download(&attachment.file_id, path) => result,
            () = shutdown.wait() => return false,
        };
        if let Err(error) = downloaded {
            log_failure(attachment, &error);

            return false;
        }

        let transcribed = tokio::select! {
            result = self.openrouter.transcribe_audio(
                path,
                AudioFormat::from_mime_type(attachment.mime_type.as_deref()),
            ) => result,
            () = shutdown.wait() => return false,
        };

        self.handle_transcript(attachment, transcribed, now).await
    }

    /// Transcribes a video attachment by extracting its audio track first.
    ///
    /// The extracted stream is fixed at Ogg/Opus 16 kHz mono, so the request declares
    /// [`AudioFormat::Ogg`] unconditionally. A video with no audio track or a host without ffmpeg is
    /// skipped and marked processed: retrying it could not change the outcome, and the window would
    /// keep paying for it. The client's input limit applies to the extracted audio, which a long
    /// video can push past it even though the video itself downloads fine.
    async fn process_video(
        &self,
        attachment: PendingAttachment,
        now: i64,
        shutdown: &mut Shutdown,
    ) -> bool {
        let directory = match TemporaryDirectory::create().await {
            Ok(directory) => directory,
            Err(error) => {
                log_failure(&attachment, &error);

                return false;
            }
        };

        let outcome = self
            .process_video_file(&attachment, directory.path(), now, shutdown)
            .await;
        directory.remove().await;

        outcome
    }

    async fn process_video_file(
        &self,
        attachment: &PendingAttachment,
        directory: &std::path::Path,
        now: i64,
        shutdown: &mut Shutdown,
    ) -> bool {
        let video = directory.join("video");

        let downloaded = tokio::select! {
            result = self.telegram.download(&attachment.file_id, &video) => result,
            () = shutdown.wait() => return false,
        };
        if let Err(error) = downloaded {
            log_failure(attachment, &error);

            return false;
        }

        let audio = directory.join("audio.ogg");
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

        self.handle_transcript(attachment, transcribed, now).await
    }

    /// Queues a transcript, or finishes an attachment whose model call a retry could not change.
    ///
    /// Audio above the client's input limit is skipped and marked processed, because the next scan
    /// would pay for the same request again; any other failure stays pending for the next scan.
    async fn handle_transcript(
        &self,
        attachment: &PendingAttachment,
        transcribed: Result<String>,
        now: i64,
    ) -> bool {
        let transcript = match transcribed {
            Ok(transcript) => transcript,
            Err(error) => match &error {
                Error::AudioTooLarge { bytes, max } => {
                    tracing::warn!(
                        attachment_id = attachment.id,
                        chat_id = attachment.chat_id,
                        message_id = attachment.message_id,
                        kind = attachment.kind.as_str(),
                        bytes,
                        max,
                        "Skipped an attachment: its audio is above the transcription input limit"
                    );

                    return self.mark_processed(attachment).await;
                }
                _ => {
                    log_failure(attachment, &error);

                    return false;
                }
            },
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

/// A temporary directory created and removed off the runtime's workers.
///
/// `mkdtemp` and the recursive removal behind `TempDir`'s drop are filesystem calls; the fallback
/// drop keeps cleanup working if the task is cancelled between `create` and `remove`.
struct TemporaryDirectory {
    directory: tempfile::TempDir,
}

impl TemporaryDirectory {
    async fn create() -> std::io::Result<Self> {
        tokio::task::spawn_blocking(tempfile::tempdir)
            .await
            .map_err(std::io::Error::other)?
            .map(|directory| Self { directory })
    }

    fn path(&self) -> &std::path::Path {
        self.directory.path()
    }

    async fn remove(self) {
        let Self { directory } = self;
        if let Err(error) = tokio::task::spawn_blocking(move || drop(directory)).await {
            tracing::debug!(
                error = &error as &dyn std::error::Error,
                "Failed to remove a temporary directory"
            );
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
    chunks(transcript, TELEGRAM_MESSAGE_LIMIT)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_download_cap_is_the_bot_api_limit() {
        assert_eq!(MAX_DOWNLOAD_BYTES, 20 * 1024 * 1024);
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
        let transcript = "x".repeat(TELEGRAM_MESSAGE_LIMIT + 1);

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
}
