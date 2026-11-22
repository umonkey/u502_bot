//! Ingest service: reads incoming Telegram messages and stores them.
//!
//! The message and its attachment commit together through [`MessageRepository`], so an attachment
//! failure leaves neither row. Both are written in one transaction because a later reader age-bounds
//! an attachment by the message that carries it. Every delivery also records the chat it came from
//! and, when it is a command from the owner, answers it.

use teloxide::types::{Chat, Message, User};

use crate::domains::InsertOutcome;
use crate::domains::attachment::{AttachmentKind, NewAttachment};
use crate::domains::chat::{ChatRepository, NewChat};
use crate::domains::message::{MessageRepository, NewMessage};
use crate::infra::sentry;
use crate::infra::telegram_bot::Messages;
use crate::services::admin::Admin;
use crate::services::shutdown::Shutdown;
use crate::services::unix_now;
use crate::services::wake::WakeSender;

/// Reads incoming Telegram messages and stores each one with its attachment.
pub struct Ingestor {
    messages: Messages,
    messages_repo: MessageRepository,
    chats: ChatRepository,
    admin: Admin,
    pending_attachments: WakeSender,
}

impl Ingestor {
    /// Creates the service over a message stream and the repositories the deliveries touch.
    pub fn new(
        messages: Messages,
        messages_repo: MessageRepository,
        chats: ChatRepository,
        admin: Admin,
        pending_attachments: WakeSender,
    ) -> Self {
        Self {
            messages,
            messages_repo,
            chats,
            admin,
            pending_attachments,
        }
    }

    /// Stores every message the dispatcher delivers until shutdown is requested.
    ///
    /// A signal is followed by a drain rather than a return: Telegram acknowledges an update when
    /// the dispatcher fetches it, so a message already buffered would be lost if the buffer were
    /// dropped. The receiver is closed before the drain, so it ends when the buffer is empty and a
    /// worker blocked on a full channel is released; only then does the dispatcher stop, which is
    /// what keeps that wait from deadlocking.
    pub async fn run(mut self, mut shutdown: Shutdown) {
        loop {
            let message = tokio::select! {
                () = shutdown.wait() => break,
                message = self.messages.next() => message,
            };

            let Some(message) = message else {
                break;
            };

            self.ingest(&message).await;
        }

        self.messages.close();

        while let Some(message) = self.messages.next().await {
            self.ingest(&message).await;
        }

        self.messages.stop().await;
        tracing::info!("Stopped the ingestor");
    }

    /// Stores one delivered message and, when it carries media, its attachment.
    ///
    /// Both rows commit together, so an attachment the database rejects leaves no message row
    /// either. Nothing retries a rolled-back delivery: Telegram acknowledged the update when the
    /// dispatcher fetched it.
    ///
    /// A registry failure is logged and never fails the delivery: losing a message to a registry
    /// write would lose it forever, because Telegram acknowledged the update when the dispatcher
    /// fetched it.
    async fn ingest(&self, message: &Message) {
        sentry::message_received();
        tracing::info!(
            chat_id = message.chat.id.0,
            message_id = message.id.0,
            kind = message_kind(message),
            text = message
                .text()
                .or_else(|| message.caption())
                .unwrap_or("<no text>"),
            "Received message"
        );
        log_media(message);

        let now = unix_now();
        let chat = NewChat {
            chat_id: message.chat.id.0,
            kind: chat_kind(&message.chat),
            title: message.chat.title().map(str::to_owned),
            added_at: now,
            seen_at: now,
        };
        match self.chats.record(&chat).await {
            Ok(InsertOutcome::Stored) => {
                tracing::info!(
                    chat_id = chat.chat_id,
                    kind = chat.kind,
                    title = chat.title.as_deref(),
                    "Recorded a chat"
                );
                self.admin.announce(&message.chat).await;
            }
            Ok(InsertOutcome::Duplicate) => {}
            Err(error) => tracing::error!(
                error = &error as &dyn std::error::Error,
                chat_id = chat.chat_id,
                "Failed to record the chat"
            ),
        }

        if self.admin.accepts(message) {
            self.admin.handle(message).await;
        }

        let attachment = new_attachment(message);
        if let Some(attachment) = &attachment {
            match attachment.kind {
                AttachmentKind::Audio => sentry::attachment_audio(),
                AttachmentKind::Video => sentry::attachment_video(),
            }
        }

        match self
            .messages_repo
            .store(&new_message(message), attachment.as_ref())
            .await
        {
            Ok(outcome) => {
                sentry::message_stored();
                if outcome.message == InsertOutcome::Duplicate {
                    tracing::info!(
                        chat_id = message.chat.id.0,
                        message_id = message.id.0,
                        "Skipped an already stored message"
                    );
                }

                match outcome.attachment {
                    Some(InsertOutcome::Stored) => {
                        sentry::attachment_stored();
                        // The transcriber is idle-waiting on this edge; end its pause now.
                        self.pending_attachments.notify();
                    }
                    Some(InsertOutcome::Duplicate) => {
                        sentry::attachment_stored();
                        tracing::info!(
                            chat_id = message.chat.id.0,
                            message_id = message.id.0,
                            "Skipped an already stored attachment"
                        );
                    }
                    None => {}
                }
            }
            Err(error) => tracing::error!(
                error = &error as &dyn std::error::Error,
                has_attachment = attachment.is_some(),
                "Failed to store the message and its attachment"
            ),
        }
    }
}

/// Logs the media payloads whose content is not text, so an incoming voice note or video is visible
/// even though it stores no message body.
fn log_media(message: &Message) {
    let sender = message
        .from
        .as_ref()
        .map(User::full_name)
        .unwrap_or_else(|| "<unknown>".to_owned());

    if let Some(voice) = message.voice() {
        let mime_type = voice
            .mime_type
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "<unknown>".to_owned());
        tracing::info!(
            sender = sender.as_str(),
            duration_seconds = voice.duration.seconds(),
            mime_type = mime_type.as_str(),
            file_size = voice.file.size,
            file_id = %voice.file.id,
            "Voice message received"
        );
    } else if let Some(audio) = message.audio() {
        let mime_type = audio
            .mime_type
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "<unknown>".to_owned());
        let file_name = audio.file_name.as_deref().unwrap_or("<unknown>");
        tracing::info!(
            sender = sender.as_str(),
            duration_seconds = audio.duration.seconds(),
            file_name = file_name,
            mime_type = mime_type.as_str(),
            file_size = audio.file.size,
            file_id = %audio.file.id,
            "Audio received"
        );
    } else if let Some(video_note) = message.video_note() {
        tracing::info!(
            sender = sender.as_str(),
            duration_seconds = video_note.duration.seconds(),
            length = video_note.length,
            file_size = video_note.file.size,
            file_id = %video_note.file.id,
            "Video message received"
        );
    } else if let Some(video) = message.video() {
        let mime_type = video
            .mime_type
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "<unknown>".to_owned());
        let file_name = video.file_name.as_deref().unwrap_or("<unknown>");
        tracing::info!(
            sender = sender.as_str(),
            duration_seconds = video.duration.seconds(),
            width = video.width,
            height = video.height,
            file_name = file_name,
            mime_type = mime_type.as_str(),
            file_size = video.file.size,
            file_id = %video.file.id,
            "Video received"
        );
    }
}

/// Classifies a chat into the coarse kind the registry stores.
///
/// Telegram's own vocabulary is kept verbatim, because nothing branches on it and an operator reads
/// it in `/chats`; `"unknown"` only covers a kind this build does not name.
fn chat_kind(chat: &Chat) -> &'static str {
    if chat.is_private() {
        "private"
    } else if chat.is_group() {
        "group"
    } else if chat.is_supergroup() {
        "supergroup"
    } else if chat.is_channel() {
        "channel"
    } else {
        "unknown"
    }
}

/// Classifies a message into a coarse kind for the log line.
///
/// The order mirrors `new_attachment`: the first matching payload wins, so captioned media is
/// classified by its payload rather than by its caption, and an unrecognised message is
/// `"other"`.
fn message_kind(message: &Message) -> &'static str {
    if message.voice().is_some() {
        "voice"
    } else if message.audio().is_some() {
        "audio"
    } else if message.video_note().is_some() {
        "video_note"
    } else if message.video().is_some() {
        "video"
    } else if message.animation().is_some() {
        "animation"
    } else if message.photo().is_some() {
        "photo"
    } else if message.document().is_some() {
        "document"
    } else if message.sticker().is_some() {
        "sticker"
    } else if message.text().is_some() {
        "text"
    } else {
        "other"
    }
}

/// Maps a Telegram message to the row the repository stores.
fn new_message(message: &Message) -> NewMessage {
    NewMessage {
        chat_id: message.chat.id.0,
        message_id: i64::from(message.id.0),
        received_at: message.date.timestamp(),
        sender_id: message
            .from
            .as_ref()
            .and_then(|user| i64::try_from(user.id.0).ok()),
        sender_name: message.from.as_ref().map(User::full_name),
        text: message
            .text()
            .or_else(|| message.caption())
            .map(str::to_owned),
    }
}

/// Maps a media payload to the attachment the repository stores.
///
/// Telegram delivers at most one media payload per message, so the first match wins. Anything else,
/// including text, photos, documents, and stickers, carries no attachment.
fn new_attachment(message: &Message) -> Option<NewAttachment> {
    let chat_id = message.chat.id.0;
    let message_id = i64::from(message.id.0);

    if let Some(voice) = message.voice() {
        Some(NewAttachment {
            chat_id,
            message_id,
            kind: AttachmentKind::Audio,
            file_id: voice.file.id.to_string(),
            mime_type: voice.mime_type.as_ref().map(ToString::to_string),
            duration_seconds: Some(i64::from(voice.duration.seconds())),
            file_size: Some(i64::from(voice.file.size)),
        })
    } else if let Some(audio) = message.audio() {
        Some(NewAttachment {
            chat_id,
            message_id,
            kind: AttachmentKind::Audio,
            file_id: audio.file.id.to_string(),
            mime_type: audio.mime_type.as_ref().map(ToString::to_string),
            duration_seconds: Some(i64::from(audio.duration.seconds())),
            file_size: Some(i64::from(audio.file.size)),
        })
    } else if let Some(video_note) = message.video_note() {
        Some(NewAttachment {
            chat_id,
            message_id,
            kind: AttachmentKind::Video,
            file_id: video_note.file.id.to_string(),
            mime_type: None,
            duration_seconds: Some(i64::from(video_note.duration.seconds())),
            file_size: Some(i64::from(video_note.file.size)),
        })
    } else {
        message.video().map(|video| NewAttachment {
            chat_id,
            message_id,
            kind: AttachmentKind::Video,
            file_id: video.file.id.to_string(),
            mime_type: video.mime_type.as_ref().map(ToString::to_string),
            duration_seconds: Some(i64::from(video.duration.seconds())),
            file_size: Some(i64::from(video.file.size)),
        })
    }
}
