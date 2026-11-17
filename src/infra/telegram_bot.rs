//! Telegram client.
//!
//! [`messages`] connects to Telegram and delivers incoming messages through a bounded channel.
//! Storage, metrics, and domain decisions belong to the caller: this module knows nothing about a
//! database, a repository, a metric, or message content beyond forwarding it.

use teloxide::dptree;
use teloxide::prelude::*;
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

/// Starts long polling and returns the stream of incoming messages.
///
/// Returns `Err` only when the initial `getMe` call fails, which usually means an invalid token or
/// no network. Transient polling failures are retried by the dispatcher's update listener. The
/// dispatcher runs in the background and ends with the runtime.
pub async fn messages(token: String) -> Result<Messages, teloxide::RequestError> {
    let bot = Bot::new(token);

    let me = bot.get_me().await?;
    tracing::info!(username = me.username(), "Connected to Telegram");

    let (sender, receiver) = mpsc::channel(64);
    let handler = Update::filter_message().endpoint(forward_message);

    tokio::spawn(async move {
        Dispatcher::builder(bot, handler)
            .dependencies(dptree::deps![sender])
            .build()
            .dispatch()
            .await;
    });

    Ok(Messages { receiver })
}

async fn forward_message(message: Message, sender: mpsc::Sender<Message>) -> ResponseResult<()> {
    if sender.send(message).await.is_err() {
        tracing::debug!("Message receiver is gone; stopping forwarding");
    }

    Ok(())
}
