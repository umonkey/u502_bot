//! Telegram client.
//!
//! [`Telegram::connect`] opens the connection, [`Telegram::messages`] delivers incoming messages
//! through a bounded channel, [`Telegram::download`] fetches a file, and [`Telegram::send`] posts a
//! text reply. Storage, metrics, and domain decisions belong to the caller: this module knows nothing
//! about a database, a repository, a metric, or message content beyond forwarding it.

use std::fmt;
use std::path::Path;

use teloxide::dptree;
use teloxide::net::Download;
use teloxide::prelude::*;
use teloxide::types::{FileId, MessageId, ReplyParameters};
use tokio::sync::mpsc;

/// Handle over the messages the Telegram dispatcher delivers.
pub struct Messages {
    receiver: mpsc::Receiver<Message>,
}

impl Messages {
    /// Waits for the next incoming message; `None` means polling stopped.
    pub async fn next(&mut self) -> Option<Message> {
        self.receiver.recv().await
    }
}

/// Client for one Telegram bot.
///
/// Cheap to clone because the underlying [`Bot`] is, so a task can hold its own handle.
#[derive(Clone)]
pub struct Telegram {
    bot: Bot,
}

impl Telegram {
    /// Opens the connection and verifies the token.
    ///
    /// Returns `Err` only when the initial `getMe` call fails, which usually means an invalid token
    /// or no network.
    pub async fn connect(token: String) -> Result<Self, teloxide::RequestError> {
        let bot = Bot::new(token);

        let me = bot.get_me().await?;
        tracing::info!(username = me.username(), "Connected to Telegram");

        Ok(Self { bot })
    }

    /// Starts long polling and returns the stream of incoming messages.
    ///
    /// Transient polling failures are retried by the dispatcher's update listener. The dispatcher
    /// runs in the background and ends with the runtime.
    pub fn messages(&self) -> Messages {
        let (sender, receiver) = mpsc::channel(64);
        let handler = Update::filter_message().endpoint(forward_message);
        let bot = self.bot.clone();

        tokio::spawn(async move {
            Dispatcher::builder(bot, handler)
                .dependencies(dptree::deps![sender])
                .build()
                .dispatch()
                .await;
        });

        Messages { receiver }
    }

    /// Downloads the file a Telegram `file_id` names into `destination`, replacing its contents.
    pub async fn download(&self, file_id: &str, destination: &Path) -> Result<(), TelegramError> {
        let file = self.bot.get_file(FileId(file_id.to_owned())).await?;
        let mut destination_file = tokio::fs::File::create(destination).await?;
        self.bot
            .download_file(&file.path, &mut destination_file)
            .await?;

        Ok(())
    }

    /// Sends `text` to `chat_id`, answering `reply_to` when it is set.
    pub async fn send(
        &self,
        chat_id: i64,
        text: &str,
        reply_to: Option<i64>,
    ) -> Result<(), TelegramError> {
        let mut request = self.bot.send_message(ChatId(chat_id), text);

        if let Some(reply) = reply_parameters(reply_to)? {
            request = request.reply_parameters(reply);
        }

        request.await?;

        Ok(())
    }
}

/// Failure modes of a Telegram file download or a sent message.
#[derive(Debug)]
pub enum TelegramError {
    /// Telegram rejected an API call.
    Request(teloxide::RequestError),
    /// Telegram accepted the call but the file transfer failed.
    Download(teloxide::DownloadError),
    /// The local destination file could not be created.
    Write(std::io::Error),
    /// A message id from the database does not fit in Telegram's 32-bit message id.
    MessageId(i64),
}

impl fmt::Display for TelegramError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(source) => write!(formatter, "telegram request error: {source}"),
            Self::Download(source) => write!(formatter, "telegram download error: {source}"),
            Self::Write(source) => write!(formatter, "file write error: {source}"),
            Self::MessageId(id) => {
                write!(
                    formatter,
                    "message id {id} does not fit in a Telegram message id"
                )
            }
        }
    }
}

impl std::error::Error for TelegramError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Request(source) => Some(source),
            Self::Download(source) => Some(source),
            Self::Write(source) => Some(source),
            Self::MessageId(_) => None,
        }
    }
}

impl From<teloxide::RequestError> for TelegramError {
    fn from(source: teloxide::RequestError) -> Self {
        Self::Request(source)
    }
}

impl From<teloxide::DownloadError> for TelegramError {
    fn from(source: teloxide::DownloadError) -> Self {
        Self::Download(source)
    }
}

impl From<std::io::Error> for TelegramError {
    fn from(source: std::io::Error) -> Self {
        Self::Write(source)
    }
}

/// Maps a reply target to the parameters Telegram needs, rejecting an id it cannot carry.
fn reply_parameters(reply_to: Option<i64>) -> Result<Option<ReplyParameters>, TelegramError> {
    reply_to
        .map(|id| {
            let message_id = i32::try_from(id).map_err(|_| TelegramError::MessageId(id))?;

            Ok(ReplyParameters::new(MessageId(message_id)))
        })
        .transpose()
}

async fn forward_message(message: Message, sender: mpsc::Sender<Message>) -> ResponseResult<()> {
    if sender.send(message).await.is_err() {
        tracing::debug!("Message receiver is gone; stopping forwarding");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_reply_target_asks_for_no_reply_parameters() {
        assert!(
            reply_parameters(None)
                .expect("no reply target is valid")
                .is_none()
        );
    }

    #[test]
    fn a_reply_target_becomes_the_answered_message() {
        let parameters = reply_parameters(Some(42))
            .expect("a small message id is valid")
            .expect("a reply target yields parameters");

        assert_eq!(parameters.message_id, MessageId(42));
    }

    #[test]
    fn a_reply_target_beyond_the_telegram_range_is_rejected() {
        let id = i64::from(i32::MAX) + 1;

        match reply_parameters(Some(id)) {
            Err(TelegramError::MessageId(reported)) => assert_eq!(reported, id),
            other => panic!("expected a message id error, got {other:?}"),
        }
    }
}
