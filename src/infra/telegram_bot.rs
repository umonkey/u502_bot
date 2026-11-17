//! Telegram bot connection and update loop.

use teloxide::prelude::*;

/// Long-polls Telegram and logs every incoming message.
///
/// Returns `Err` only when the initial `getMe` call fails, which usually means an invalid token
/// or no network. Transient polling failures are retried by the dispatcher's update listener.
pub async fn run(token: String) -> Result<(), teloxide::RequestError> {
    let bot = Bot::new(token);

    let me = bot.get_me().await?;
    tracing::info!(username = me.username(), "Connected to Telegram");

    let handler = Update::filter_message().endpoint(handle_message);

    Dispatcher::builder(bot, handler).build().dispatch().await;

    Ok(())
}

async fn handle_message(message: Message) -> ResponseResult<()> {
    tracing::info!(
        chat_id = message.chat.id.0,
        message_id = message.id.0,
        text = message.text().unwrap_or("<no text>"),
        "Received message"
    );

    Ok(())
}
