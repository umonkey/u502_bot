//! Transcription service: turns pending audio attachments into outbox rows.
//!
//! The service runs beside the ingestor, so ingestion never waits on an LLM call. Every scan reads
//! a small batch of audio attachments that are still pending and whose message is at most an hour
//! old, downloads each file to a temporary directory, asks OpenRouter for a transcript, queues the
//! text in the outbox, and marks the attachment processed.
//!
//! The one-hour window bounds both cost and retries: a file that keeps failing is retried on every
//! scan until its message ages out, and then left alone. `TRANSCRIBE_CHATS` limits which chats are
//! transcribed at all, because a transcription spends credits and sends audio to a third party. The
//! variable is optional: an empty list transcribes nothing and the skip warnings publish the chat ids
//! an operator needs before the list is known.

use std::collections::HashSet;
use std::env::VarError;
use std::fmt;
use std::time::Duration;

use crate::domains::attachment::{AttachmentRepository, PendingAudio};
use crate::domains::outbox::{NewOutboxMessage, OutboxError, OutboxRepository};
use crate::infra::openrouter::OpenRouter;
use crate::infra::sentry;
use crate::infra::telegram_bot::Telegram;
use crate::services::unix_now;

/// Instruction sent with every file; it asks for a verbatim transcript and nothing else.
const TRANSCRIPTION_PROMPT: &str = "Transcribe this voice message verbatim in the language it is spoken. Return only the transcript: no commentary, no timestamps, no speaker labels, no translation. If there is no speech, return an empty string.";

/// Plain-text header placed above the transcript in the first reply.
const REPLY_HEADER: &str = "Message transcript:";

/// Environment variable holding the comma-separated list of chats to transcribe.
const TRANSCRIBE_CHATS: &str = "TRANSCRIBE_CHATS";

/// Only messages at most this old are transcribed, which bounds cost and retries.
const WINDOW_SECONDS: i64 = 3_600;

/// Attachments read per scan.
const BATCH: i64 = 10;

/// Pause after a scan that found nothing, or that could not do any work.
const IDLE: Duration = Duration::from_secs(5);

/// Maximum visible characters per outbox row: Telegram rejects a message longer than 4096.
///
/// The limit budgets the text Telegram counts after entity parsing, not the markup, so chunking
/// stays on the raw transcript and escaping is applied per chunk, where a chunk boundary can never
/// split a tag.
const CHUNK_LIMIT: usize = 4_000;

/// The chats whose audio is transcribed.
///
/// Membership is a configuration decision, not data, so the check happens after the scan: a chat
/// that is not listed is logged once and marked processed. An empty whitelist is allowed and
/// transcribes nothing, which is how an operator discovers the chat ids from the skip warnings.
#[derive(Debug, Clone)]
pub struct Whitelist(HashSet<i64>);

impl Whitelist {
    /// Parses a comma-separated list of numeric chat ids.
    ///
    /// Entries are trimmed, empty entries are ignored, and duplicates collapse. An empty list is
    /// valid and means "transcribe nothing": the bot still runs, so the skip warnings can publish
    /// the chat ids before the list is known. A non-numeric entry is an error, so a typo cannot
    /// silently narrow the list.
    pub fn parse(raw: &str) -> Result<Self, WhitelistError> {
        let mut chats = HashSet::new();

        for token in raw.split(',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }

            let chat_id = token
                .parse()
                .map_err(|_| WhitelistError::NotANumber(token.to_owned()))?;
            chats.insert(chat_id);
        }

        Ok(Self(chats))
    }

    /// Reads the whitelist from [`TRANSCRIBE_CHATS`].
    ///
    /// An unset variable yields an empty whitelist rather than an error; see [`Self::parse`].
    pub fn from_env() -> Result<Self, WhitelistError> {
        match std::env::var(TRANSCRIBE_CHATS) {
            Ok(raw) => Self::from_value(Some(raw)),
            Err(VarError::NotPresent) => Self::from_value(None),
            Err(VarError::NotUnicode(_)) => {
                Err(WhitelistError::NotUnicode(TRANSCRIBE_CHATS.to_owned()))
            }
        }
    }

    fn from_value(value: Option<String>) -> Result<Self, WhitelistError> {
        match value {
            Some(raw) => Self::parse(&raw),
            None => Ok(Self(HashSet::new())),
        }
    }

    /// Returns whether `chat_id` may be transcribed.
    pub fn contains(&self, chat_id: i64) -> bool {
        self.0.contains(&chat_id)
    }

    /// Returns whether no chat is white-listed, in which case every audio attachment is skipped.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Failure modes of reading the transcription whitelist.
#[derive(Debug)]
pub enum WhitelistError {
    /// [`TRANSCRIBE_CHATS`] is not valid Unicode.
    NotUnicode(String),
    /// One entry is not a numeric chat id.
    NotANumber(String),
}

impl fmt::Display for WhitelistError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotUnicode(name) => write!(formatter, "{name} is not valid Unicode"),
            Self::NotANumber(token) => write!(
                formatter,
                "{TRANSCRIBE_CHATS} entry {token:?} is not a numeric chat id"
            ),
        }
    }
}

impl std::error::Error for WhitelistError {}

/// Transcribes pending audio attachments and queues the transcripts for delivery.
pub struct Transcriber {
    telegram: Telegram,
    openrouter: OpenRouter,
    attachments: AttachmentRepository,
    outbox: OutboxRepository,
    chats: Whitelist,
}

impl Transcriber {
    /// Creates the service over the clients and repositories the command composed.
    pub fn new(
        telegram: Telegram,
        openrouter: OpenRouter,
        attachments: AttachmentRepository,
        outbox: OutboxRepository,
        chats: Whitelist,
    ) -> Self {
        Self {
            telegram,
            openrouter,
            attachments,
            outbox,
            chats,
        }
    }

    /// Transcribes pending audio forever, pausing only when there is nothing to do.
    pub async fn run(self) {
        loop {
            if !self.tick().await {
                tokio::time::sleep(IDLE).await;
            }
        }
    }

    /// Processes one batch and returns whether any attachment was finished.
    ///
    /// A failed scan sleeps here rather than returning: the pool is shared with the ingestor, and a
    /// failed read must not spin. A batch in which every row failed returns `false`, so a file that
    /// keeps failing is retried once per [`IDLE`] instead of as fast as the failure returns.
    async fn tick(&self) -> bool {
        let now = unix_now();
        let pending = match self
            .attachments
            .pending_audio(now - WINDOW_SECONDS, BATCH)
            .await
        {
            Ok(pending) => pending,
            Err(error) => {
                tracing::error!(
                    error = &error as &dyn std::error::Error,
                    "Failed to scan for pending audio"
                );
                tokio::time::sleep(IDLE).await;

                return false;
            }
        };

        let mut processed = false;

        for attachment in pending {
            processed |= self.process(attachment, now).await;
        }

        processed
    }

    /// Transcribes one attachment and queues its transcript; returns whether it was finished.
    ///
    /// No database transaction spans the download or the LLM call: each statement returns the pooled
    /// connection immediately, so holding one would stall the ingestor for the length of the call.
    async fn process(&self, attachment: PendingAudio, now: i64) -> bool {
        if !self.chats.contains(attachment.chat_id) {
            tracing::warn!(
                chat_id = attachment.chat_id,
                "Voice message from chat {} skipped: not a white listed chat, add to TRANSCRIBE_CHATS if needed.",
                attachment.chat_id
            );

            return self.mark_processed(&attachment).await;
        }

        let directory = match tempfile::tempdir() {
            Ok(directory) => directory,
            Err(error) => {
                log_failure(&attachment, &error);

                return false;
            }
        };
        let path = directory.path().join("audio");

        if let Err(error) = self.telegram.download(&attachment.file_id, &path).await {
            log_failure(&attachment, &error);

            return false;
        }

        let transcript = match self
            .openrouter
            .transcribe_audio(&path, TRANSCRIPTION_PROMPT)
            .await
        {
            Ok(transcript) => transcript,
            Err(error) => {
                log_failure(&attachment, &error);

                return false;
            }
        };
        let transcript = transcript.trim();

        if transcript.is_empty() {
            tracing::info!(
                attachment_id = attachment.id,
                chat_id = attachment.chat_id,
                message_id = attachment.message_id,
                "Transcribed an audio attachment with no speech"
            );

            return self.mark_processed(&attachment).await;
        }

        if let Err(error) = self.enqueue(&attachment, transcript, now).await {
            log_failure(&attachment, &error);

            return false;
        }

        self.mark_processed(&attachment).await
    }

    /// Queues the transcript in chunks Telegram will accept; returns whether every row was written.
    ///
    /// Only the first chunk answers the voice message, so the reply thread stays on it.
    async fn enqueue(
        &self,
        attachment: &PendingAudio,
        transcript: &str,
        now: i64,
    ) -> Result<(), OutboxError> {
        for (index, chunk) in chunks(transcript, CHUNK_LIMIT).into_iter().enumerate() {
            let message = NewOutboxMessage {
                chat_id: attachment.chat_id,
                thread_id: (index == 0).then_some(attachment.message_id),
                added_at: now,
                message: reply_html(chunk, index == 0),
            };

            self.outbox.enqueue(&message).await?;
            sentry::outbox_added();
        }

        Ok(())
    }

    /// Marks one attachment processed and counts it, on every path that finishes with it.
    async fn mark_processed(&self, attachment: &PendingAudio) -> bool {
        match self.attachments.mark_transcribed(attachment.id).await {
            Ok(()) => {
                sentry::attachment_transcribed();

                true
            }
            Err(error) => {
                tracing::error!(
                    attachment_id = attachment.id,
                    error = &error as &dyn std::error::Error,
                    "Failed to mark an audio attachment processed"
                );

                false
            }
        }
    }
}

/// Logs one attachment that could not be processed; it stays pending, so the next scan retries it.
fn log_failure(attachment: &PendingAudio, error: &(dyn std::error::Error + 'static)) {
    tracing::error!(
        attachment_id = attachment.id,
        chat_id = attachment.chat_id,
        message_id = attachment.message_id,
        error = error,
        "Failed to process an audio attachment"
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
            "{REPLY_HEADER}\n\n<blockquote>{}</blockquote>",
            escape_html(chunk)
        )
    } else {
        format!("<blockquote>{}</blockquote>", escape_html(chunk))
    }
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
    fn whitelist_parses_trimmed_and_spaced_ids() {
        let whitelist = Whitelist::parse(" 1 , 2 ,3 ").expect("a spaced list should parse");

        assert!(whitelist.contains(1));
        assert!(whitelist.contains(2));
        assert!(whitelist.contains(3));
        assert!(!whitelist.contains(4));
    }

    #[test]
    fn whitelist_collapses_duplicates_and_trailing_commas() {
        let whitelist = Whitelist::parse("1,1,2,").expect("duplicates should parse");

        assert!(whitelist.contains(1));
        assert!(whitelist.contains(2));
    }

    #[test]
    fn whitelist_allows_an_empty_list() {
        for raw in ["", "   ", ",", " , "] {
            let whitelist = Whitelist::parse(raw).expect("an empty list should parse");

            assert!(whitelist.is_empty(), "raw {raw:?}");
            assert!(!whitelist.contains(1));
        }
    }

    #[test]
    fn whitelist_treats_an_unset_variable_as_empty() {
        let whitelist = Whitelist::from_value(None).expect("an unset variable should parse");

        assert!(whitelist.is_empty());
    }

    #[test]
    fn whitelist_rejects_a_non_numeric_entry() {
        match Whitelist::parse("1,two,3") {
            Err(WhitelistError::NotANumber(token)) => assert_eq!(token, "two"),
            other => panic!("expected a non-numeric entry, got {other:?}"),
        }
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
            "Message transcript:\n\n<blockquote>Hello guys I am leaving</blockquote>"
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
            "Message transcript:\n\n<blockquote>&lt;/blockquote&gt;&lt;b&gt;x&lt;/b&gt;</blockquote>"
        );
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
