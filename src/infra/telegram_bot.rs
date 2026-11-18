//! Telegram client.
//!
//! [`Telegram::connect`] opens the connection, [`Telegram::messages`] delivers incoming messages
//! through a bounded channel, [`Telegram::download`] fetches a file, [`Telegram::send`] posts a
//! text reply, and [`Telegram::react`] sets one emoji reaction on a message. Storage, metrics, and
//! domain decisions belong to the caller: this module knows nothing about a database, a repository,
//! a metric, or message content beyond forwarding it.

use std::path::Path;
use std::time::Duration;

use teloxide::dispatching::ShutdownToken;
use teloxide::dptree;
use teloxide::net::Download;
use teloxide::prelude::*;
use teloxide::types::{FileId, MessageId, ParseMode, ReactionType, ReplyParameters};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::shared::result::{Error, Operation, Result};

/// How long [`Messages::stop`] waits for the dispatcher task before giving up.
const DISPATCHER_STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// Handle over the messages the Telegram dispatcher delivers.
pub struct Messages {
    receiver: mpsc::Receiver<Message>,
    poller: Poller,
}

/// Handle over the dispatcher task, so polling can be stopped from outside.
struct Poller {
    token: ShutdownToken,
    task: JoinHandle<()>,
}

impl Messages {
    /// Waits for the next incoming message; `None` means polling stopped.
    pub async fn next(&mut self) -> Option<Message> {
        self.receiver.recv().await
    }

    /// Stops forwarding, leaving buffered messages readable for a drain.
    ///
    /// Closing the receiver makes a blocked `send` fail, which releases a dispatcher worker that is
    /// stuck on a full channel, so awaiting the dispatcher afterwards cannot deadlock.
    pub fn close(&mut self) {
        self.receiver.close();
    }

    /// Stops the dispatcher and waits for its task, bounded by [`DISPATCHER_STOP_TIMEOUT`].
    pub async fn stop(mut self) {
        // Idempotent after `close`, and required here so a blocked worker can finish.
        self.receiver.close();

        if self.poller.token.shutdown().is_err() {
            tracing::debug!("The Telegram dispatcher is not running");
        }

        match tokio::time::timeout(DISPATCHER_STOP_TIMEOUT, self.poller.task).await {
            Ok(Ok(())) => tracing::info!("Stopped polling Telegram"),
            Ok(Err(error)) => tracing::error!(
                error = &error as &dyn std::error::Error,
                "The Telegram dispatcher panicked"
            ),
            Err(_) => tracing::warn!(
                timeout_seconds = DISPATCHER_STOP_TIMEOUT.as_secs(),
                "The Telegram dispatcher did not stop in time"
            ),
        }
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
    pub async fn connect(token: String) -> Result<Self> {
        let bot = Bot::new(token);

        let me = bot.get_me().await?;
        tracing::info!(username = me.username(), "Connected to Telegram");

        Ok(Self { bot })
    }

    /// Starts long polling and returns the stream of incoming messages.
    ///
    /// Transient polling failures are retried by the dispatcher's update listener. The dispatcher
    /// runs in the background until [`Messages::stop`] asks it to stop, or until the runtime ends.
    pub fn messages(&self) -> Messages {
        let (sender, receiver) = mpsc::channel(64);
        let handler = Update::filter_message().endpoint(forward_message);
        let bot = self.bot.clone();

        let mut dispatcher = Dispatcher::builder(bot, handler)
            .dependencies(dptree::deps![sender])
            .build();
        // Taken before the dispatcher is moved, because the token is the only outward handle.
        let token = dispatcher.shutdown_token();

        let task = tokio::spawn(async move {
            dispatcher.dispatch().await;
        });

        Messages {
            receiver,
            poller: Poller { token, task },
        }
    }

    /// Downloads the file a Telegram `file_id` names into `destination`, replacing its contents.
    pub async fn download(&self, file_id: &str, destination: &Path) -> Result<()> {
        let file = self.bot.get_file(FileId(file_id.to_owned())).await?;
        let mut destination_file =
            tokio::fs::File::create(destination)
                .await
                .map_err(|source| Error::WriteFile {
                    path: destination.to_path_buf(),
                    source,
                })?;
        self.bot
            .download_file(&file.path, &mut destination_file)
            .await?;

        Ok(())
    }

    /// Sends `text` to `chat_id`, answering `reply_to` when it is set.
    ///
    /// `text` is Telegram HTML, already escaped by the producer; this method only forwards it.
    pub async fn send(&self, chat_id: i64, text: &str, reply_to: Option<i64>) -> Result<()> {
        let mut request = self
            .bot
            .send_message(ChatId(chat_id), text)
            .parse_mode(ParseMode::Html);

        if let Some(reply) = reply_parameters(reply_to)? {
            request = request.reply_parameters(reply);
        }

        request.await?;

        Ok(())
    }

    /// Sets `emoji` as the bot's reaction on one message.
    ///
    /// A bot holds one reaction per message, so repeating the call replaces the previous emoji
    /// instead of stacking another. A chat may forbid reactions, which is why the caller treats a
    /// failure as cosmetic.
    pub async fn react(&self, chat_id: i64, message_id: i64, emoji: &str) -> Result<()> {
        self.bot
            .set_message_reaction(ChatId(chat_id), message_id_of(message_id)?)
            .reaction(vec![ReactionType::Emoji {
                emoji: emoji.to_owned(),
            }])
            .await?;

        Ok(())
    }
}

impl From<teloxide::RequestError> for Error {
    fn from(source: teloxide::RequestError) -> Self {
        Self::External {
            operation: Operation::TelegramRequest,
            source: Box::new(source),
        }
    }
}

impl From<teloxide::DownloadError> for Error {
    fn from(source: teloxide::DownloadError) -> Self {
        Self::External {
            operation: Operation::TelegramDownload,
            source: Box::new(source),
        }
    }
}

/// Converts a stored message id to the width the Bot API carries, rejecting one it cannot.
fn message_id_of(id: i64) -> Result<MessageId> {
    let message_id = i32::try_from(id).map_err(|_| Error::MessageIdOutOfRange(id))?;

    Ok(MessageId(message_id))
}

/// Maps a reply target to the parameters Telegram needs, rejecting an id it cannot carry.
fn reply_parameters(reply_to: Option<i64>) -> Result<Option<ReplyParameters>> {
    reply_to
        .map(|id| Ok(ReplyParameters::new(message_id_of(id)?)))
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
            Err(Error::MessageIdOutOfRange(reported)) => assert_eq!(reported, id),
            other => panic!("expected a message id error, got {other:?}"),
        }
    }

    #[test]
    fn a_small_message_id_converts_for_a_reaction() {
        assert_eq!(
            message_id_of(42).expect("a small message id is valid"),
            MessageId(42)
        );
    }

    #[test]
    fn a_message_id_beyond_the_telegram_range_is_rejected_with_its_value() {
        let id = i64::from(i32::MAX) + 1;

        match message_id_of(id) {
            Err(Error::MessageIdOutOfRange(reported)) => assert_eq!(reported, id),
            other => panic!("expected a message id error, got {other:?}"),
        }
    }
}
