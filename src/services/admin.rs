//! The owner's direct-message interface.
//!
//! It records nothing and queues nothing: it changes the chat registry and answers inline, so the
//! outbox stays reserved for transcripts. It is reachable only from the ingestor's message stream.

use teloxide::types::{Chat, Message, User};

use crate::domains::chat::{ChatRepository, ChatRow, ChatStatus};
use crate::infra::telegram_bot::Telegram;
use crate::shared::text::{TELEGRAM_MESSAGE_LIMIT, escape_html};

/// Longest chat title rendered in a reply, in characters.
///
/// A title is chosen by whoever created the chat, so it is capped before it joins a message bound
/// for the owner: one absurd title must not fill the whole reply.
const MAX_TITLE_CHARS: usize = 120;

/// The owner's command interface over the chat registry.
pub struct Admin {
    telegram: Telegram,
    chats: ChatRepository,
    owner_user_id: i64,
}

impl Admin {
    /// Creates the interface over the client that answers and the registry that holds decisions.
    pub fn new(telegram: Telegram, chats: ChatRepository, owner_user_id: i64) -> Self {
        Self {
            telegram,
            chats,
            owner_user_id,
        }
    }

    /// Returns whether `message` is a command the owner sent in a private chat.
    pub fn accepts(&self, message: &Message) -> bool {
        is_owner_message(&message.chat, message.from.as_ref(), self.owner_user_id)
    }

    /// Answers one command the owner sent; anything that is not a command is ignored.
    pub async fn handle(&self, message: &Message) {
        let Some(command) = message.text().and_then(parse_command) else {
            return;
        };

        match command {
            Command::Chats => self.handle_chats(message).await,
            Command::Approve(None) | Command::Block(None) => self.reply(message, &usage()).await,
            Command::Approve(Some(chat_id)) => {
                self.decide(message, chat_id, ChatStatus::Approved).await
            }
            Command::Block(Some(chat_id)) => {
                self.decide(message, chat_id, ChatStatus::Blocked).await
            }
            Command::Help | Command::Unknown => self.reply(message, &usage()).await,
        }
    }

    /// Tells the owner about a chat recorded for the first time.
    ///
    /// A private chat is silent: that is the owner's own conversation with the bot, and announcing
    /// it would only echo their message back. Nothing is retried, because a lost notice is
    /// recoverable with `/chats` while a blocking send would hold up ingestion.
    pub async fn announce(&self, chat: &Chat) {
        if chat.is_private() {
            return;
        }

        let notice = new_chat_notice(chat);
        if let Err(error) = self.telegram.send(self.owner_user_id, &notice, None).await {
            tracing::warn!(
                chat_id = chat.id.0,
                error = &error as &dyn std::error::Error,
                "Failed to tell the owner about a new chat: the owner must start a conversation with the bot first"
            );
        }
    }

    async fn handle_chats(&self, message: &Message) {
        match self.chats.list().await {
            Ok(rows) => {
                for (index, html) in chats_messages(&rows).into_iter().enumerate() {
                    self.send(message, &html, index == 0).await;
                }
            }
            Err(error) => {
                tracing::error!(
                    error = &error as &dyn std::error::Error,
                    "Failed to read the chat registry"
                );
                self.reply(
                    message,
                    "Failed to read the chat list; the error was logged.",
                )
                .await;
            }
        }
    }

    async fn decide(&self, message: &Message, chat_id: i64, status: ChatStatus) {
        match self.chats.set_status(chat_id, status).await {
            Ok(true) => {
                tracing::info!(chat_id, status = status.as_str(), "Changed a chat status");
                self.reply(message, &confirmation(chat_id, status)).await;
            }
            Ok(false) => {
                self.reply(
                    message,
                    &format!("No chat with id {chat_id} is recorded yet."),
                )
                .await;
            }
            Err(error) => {
                tracing::error!(
                    chat_id,
                    status = status.as_str(),
                    error = &error as &dyn std::error::Error,
                    "Failed to change a chat status"
                );
                self.reply(
                    message,
                    "Failed to change the chat status; the error was logged.",
                )
                .await;
            }
        }
    }

    async fn reply(&self, message: &Message, html: &str) {
        self.send(message, html, true).await;
    }

    /// Sends one piece of an answer, replying to the command only for the first piece.
    ///
    /// A send failure is logged and not retried: the path is interactive, so the owner can repeat
    /// the command, and a queue would put an admin answer behind a transcript.
    async fn send(&self, message: &Message, html: &str, first: bool) {
        let reply_to = first.then(|| i64::from(message.id.0));

        if let Err(error) = self.telegram.send(message.chat.id.0, html, reply_to).await {
            tracing::warn!(
                chat_id = message.chat.id.0,
                error = &error as &dyn std::error::Error,
                "Failed to answer the owner"
            );
        }
    }
}

/// Whether a message is the configured owner writing in a private chat.
///
/// A private chat means only the owner could have sent it, and the sender check keeps a bot out;
/// `from` is absent for a channel post, which is never a private chat either.
fn is_owner_message(chat: &Chat, from: Option<&User>, owner_user_id: i64) -> bool {
    chat.is_private()
        && from.is_some_and(|user| {
            !user.is_bot && i64::try_from(user.id.0).is_ok_and(|id| id == owner_user_id)
        })
}

/// One command the owner may send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    /// `/chats`: list what is recorded.
    Chats,
    /// `/approve <id>`; the target is absent when the owner omitted it.
    Approve(Option<i64>),
    /// `/block <id>`; the target is absent when the owner omitted it.
    Block(Option<i64>),
    /// `/help`.
    Help,
    /// Any other `/word`.
    Unknown,
}

/// Parses a command out of a message body, or `None` when the body is not one.
///
/// Ordinary text is silence. The first token is split on `@`, so `/chats@u502bot` reaches the same
/// branch as `/chats`; the second token is the target and stays absent when it is not a number, so
/// a typo answers with the usage line instead of commanding a wrong chat.
fn parse_command(text: &str) -> Option<Command> {
    let mut tokens = text.split_whitespace();
    let name = tokens.next()?.strip_prefix('/')?.split('@').next()?;
    let target = tokens.next().and_then(|token| token.parse::<i64>().ok());

    match name {
        "chats" => Some(Command::Chats),
        "approve" => Some(Command::Approve(target)),
        "block" => Some(Command::Block(target)),
        "help" => Some(Command::Help),
        _ => Some(Command::Unknown),
    }
}

/// The commands, as the help answer and a missing target render them.
fn usage() -> String {
    "Commands:\n\
     /chats — list the recorded chats\n\
     /approve &lt;id&gt; — transcribe the chat\n\
     /block &lt;id&gt; — stop transcribing the chat\n\
     /help — show this help"
        .to_owned()
}

/// The answer to a status change that took effect.
fn confirmation(chat_id: i64, status: ChatStatus) -> String {
    match status {
        ChatStatus::Approved => {
            format!("Chat {chat_id} is approved: its audio and video are transcribed.")
        }
        ChatStatus::Blocked => {
            format!("Chat {chat_id} is blocked: its audio and video are skipped.")
        }
        ChatStatus::Pending => {
            format!("Chat {chat_id} is pending: its audio and video are skipped.")
        }
    }
}

/// Renders the registry as messages fit for Telegram.
///
/// Pending chats come first, because they are the ones waiting on a decision. Lines accumulate
/// whole, so a split can never cut an escaped entity in half.
fn chats_messages(rows: &[ChatRow]) -> Vec<String> {
    if rows.is_empty() {
        return vec![
            "No chat is recorded yet. Add the bot to a chat, or send it a message there, and the chat \
             appears here for /approve."
                .to_owned(),
        ];
    }

    let mut lines = Vec::new();
    for (status, heading) in [
        (ChatStatus::Pending, "Pending:"),
        (ChatStatus::Approved, "Approved:"),
    ] {
        let mut group = rows.iter().filter(|row| row.status == status).peekable();
        if group.peek().is_none() {
            continue;
        }

        lines.push(heading.to_owned());
        for row in group {
            lines.push(format!(
                "• {} ({}, {})",
                escape_html(&short_title(row.title.as_deref())),
                row.kind,
                row.chat_id
            ));
        }
    }

    pack(lines)
}

/// Tells the owner which chat was recorded and how to approve it.
fn new_chat_notice(chat: &Chat) -> String {
    let chat_id = chat.id.0;

    format!(
        "New chat recorded: {} ({chat_id}). Approve it with /approve {chat_id}, or ignore it.",
        escape_html(&short_title(chat.title()))
    )
}

/// Keeps at most [`MAX_TITLE_CHARS`] characters of a chat title.
fn short_title(title: Option<&str>) -> String {
    match title {
        Some(title) if !title.trim().is_empty() => title.chars().take(MAX_TITLE_CHARS).collect(),
        _ => "<no title>".to_owned(),
    }
}

/// Packs lines into messages of at most [`TELEGRAM_MESSAGE_LIMIT`] bytes.
fn pack(lines: Vec<String>) -> Vec<String> {
    let mut messages = Vec::new();
    let mut current = String::new();

    for line in lines {
        if !current.is_empty() && current.len() + 1 + line.len() > TELEGRAM_MESSAGE_LIMIT {
            messages.push(std::mem::take(&mut current));
        }

        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(&line);
    }

    if !current.is_empty() {
        messages.push(current);
    }

    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    fn private_chat() -> Chat {
        serde_json::from_str(r#"{"id": 99, "type": "private", "first_name": "Owner"}"#)
            .expect("a private chat should deserialize")
    }

    fn supergroup_chat(title: &str) -> Chat {
        serde_json::from_value(serde_json::json!({
            "id": -100,
            "type": "supergroup",
            "title": title,
        }))
        .expect("a supergroup should deserialize")
    }

    fn owner() -> User {
        serde_json::from_str(r#"{"id": 99, "is_bot": false, "first_name": "Owner"}"#)
            .expect("the owner should deserialize")
    }

    fn bot() -> User {
        serde_json::from_str(r#"{"id": 99, "is_bot": true, "first_name": "Bot"}"#)
            .expect("a bot should deserialize")
    }

    fn stranger() -> User {
        serde_json::from_str(r#"{"id": 7, "is_bot": false, "first_name": "Stranger"}"#)
            .expect("a stranger should deserialize")
    }

    fn chat_row(chat_id: i64, title: Option<&str>, status: ChatStatus) -> ChatRow {
        ChatRow {
            chat_id,
            kind: "supergroup".to_owned(),
            title: title.map(str::to_owned),
            status,
        }
    }

    #[test]
    fn parse_command_recognises_every_command() {
        assert_eq!(parse_command("/chats"), Some(Command::Chats));
        assert_eq!(parse_command("/help"), Some(Command::Help));
        assert_eq!(
            parse_command("/approve 42"),
            Some(Command::Approve(Some(42)))
        );
        assert_eq!(parse_command("/block 42"), Some(Command::Block(Some(42))));
    }

    #[test]
    fn parse_command_accepts_the_bot_suffix_and_surrounding_space() {
        assert_eq!(parse_command("  /chats@u502bot  "), Some(Command::Chats));
        assert_eq!(
            parse_command("/approve@u502bot -100123"),
            Some(Command::Approve(Some(-100123)))
        );
    }

    #[test]
    fn parse_command_reports_a_missing_or_unusable_target() {
        assert_eq!(parse_command("/approve"), Some(Command::Approve(None)));
        assert_eq!(parse_command("/block"), Some(Command::Block(None)));
        assert_eq!(parse_command("/approve two"), Some(Command::Approve(None)));
    }

    #[test]
    fn parse_command_falls_back_to_unknown_for_another_word() {
        assert_eq!(parse_command("/unknown"), Some(Command::Unknown));
        assert_eq!(parse_command("/"), Some(Command::Unknown));
    }

    #[test]
    fn parse_command_ignores_ordinary_text() {
        assert_eq!(parse_command("hello"), None);
        assert_eq!(parse_command(""), None);
        assert_eq!(parse_command("   \n"), None);
    }

    #[test]
    fn is_owner_message_accepts_only_the_owner_in_a_private_chat() {
        assert!(is_owner_message(&private_chat(), Some(&owner()), 99));
        assert!(!is_owner_message(
            &supergroup_chat("Car talk"),
            Some(&owner()),
            99
        ));
        assert!(!is_owner_message(&private_chat(), Some(&stranger()), 99));
        assert!(!is_owner_message(&private_chat(), Some(&bot()), 99));
        assert!(!is_owner_message(&private_chat(), None, 99));
    }

    #[test]
    fn chats_messages_explains_an_empty_registry() {
        let messages = chats_messages(&[]);

        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains("No chat is recorded yet"));
    }

    #[test]
    fn chats_messages_groups_pending_before_approved() {
        let rows = [
            chat_row(3, Some("Golf"), ChatStatus::Approved),
            chat_row(2, Some("Bravo"), ChatStatus::Pending),
        ];

        let messages = chats_messages(&rows);

        assert_eq!(messages.len(), 1);
        let pending = messages[0].find("Pending:").expect("pending heading");
        let approved = messages[0].find("Approved:").expect("approved heading");
        assert!(pending < approved);
        assert!(messages[0].contains("Bravo (supergroup, 2)"));
        assert!(messages[0].contains("Golf (supergroup, 3)"));
    }

    #[test]
    fn chats_messages_escapes_a_title_containing_markup() {
        let rows = [chat_row(2, Some("<b>Bold</b> & co"), ChatStatus::Pending)];

        let messages = chats_messages(&rows);

        assert!(
            messages[0].contains("&lt;b&gt;Bold&lt;/b&gt; &amp; co"),
            "message {:?}",
            messages[0]
        );
        assert!(!messages[0].contains("<b>"));
    }

    #[test]
    fn chats_messages_labels_a_chat_without_a_title() {
        let rows = [chat_row(2, None, ChatStatus::Pending)];

        let messages = chats_messages(&rows);

        assert!(messages[0].contains("&lt;no title&gt; (supergroup, 2)"));
    }

    #[test]
    fn chats_messages_splits_a_long_list_within_the_limit() {
        let rows: Vec<ChatRow> = (0..200_i64)
            .map(|chat_id| {
                chat_row(
                    chat_id,
                    Some(&"t".repeat(MAX_TITLE_CHARS)),
                    ChatStatus::Pending,
                )
            })
            .collect();

        let messages = chats_messages(&rows);

        assert!(messages.len() > 1, "a long list should split");
        for message in &messages {
            assert!(
                message.len() <= TELEGRAM_MESSAGE_LIMIT,
                "message of {} bytes exceeds the limit",
                message.len()
            );
        }
    }

    #[test]
    fn new_chat_notice_names_the_chat_and_the_command() {
        let notice = new_chat_notice(&supergroup_chat("Car <talk>"));

        assert!(notice.contains("Car &lt;talk&gt;"));
        assert!(notice.contains("(-100)"));
        assert!(notice.contains("/approve -100"));
    }

    #[test]
    fn new_chat_notice_caps_an_absurd_title() {
        let title = "x".repeat(MAX_TITLE_CHARS * 2);

        let notice = new_chat_notice(&supergroup_chat(&title));

        assert!(!notice.contains(&title));
        assert!(notice.contains(&"x".repeat(MAX_TITLE_CHARS)));
    }
}
