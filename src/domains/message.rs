//! Message model and the repository that owns every statement against `messages`.
//!
//! The row type stays independent of Telegram: the command that reads updates maps them into
//! [`NewMessage`] before storing. [`MessageRepository`] must stay in sync with the `messages` table
//! in `dev/schema.sql`.

use sqlx::SqlitePool;

use crate::shared::result::Result;

/// One message to write to the `messages` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessage {
    /// Telegram chat the message arrived in.
    pub chat_id: i64,
    /// Telegram message id, unique within its chat.
    pub message_id: i64,
    /// Arrival time as Unix epoch seconds, UTC.
    pub received_at: i64,
    /// Telegram id of the sender, when the update carries one.
    pub sender_id: Option<i64>,
    /// Display name of the sender, when the update carries one.
    pub sender_name: Option<String>,
    /// Message text or caption; absent for photos, stickers, and service events.
    pub text: Option<String>,
}

/// What [`MessageRepository::store`] did with a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOutcome {
    /// The row was written.
    Stored,
    /// The row was already there, so the redelivery was ignored.
    Duplicate,
}

/// Writes messages to the `messages` table.
pub struct MessageRepository {
    pool: SqlitePool,
}

impl MessageRepository {
    /// Creates a repository over a pool the database client already opened.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Stores one message, ignoring a redelivery of a row the table already holds.
    ///
    /// Must stay in sync with the `messages` table in `dev/schema.sql`: the column order and the
    /// `UNIQUE (chat_id, message_id)` constraint are what make this statement idempotent.
    pub async fn store(&self, message: &NewMessage) -> Result<StoreOutcome> {
        let result = sqlx::query(
            "INSERT OR IGNORE INTO messages (chat_id, message_id, received_at, sender_id, sender_name, message) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(message.chat_id)
        .bind(message.message_id)
        .bind(message.received_at)
        .bind(message.sender_id)
        .bind(message.sender_name.as_deref())
        .bind(message.text.as_deref())
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            Ok(StoreOutcome::Duplicate)
        } else {
            Ok(StoreOutcome::Stored)
        }
    }
}

#[cfg(test)]
mod tests {
    use sqlx::Row;

    use crate::infra::database::Database;

    use super::*;

    fn new_message() -> NewMessage {
        NewMessage {
            chat_id: 42,
            message_id: 7,
            received_at: 1_700_000_000,
            sender_id: Some(99),
            sender_name: Some("Ada".to_owned()),
            text: Some("hello".to_owned()),
        }
    }

    async fn in_memory_repository() -> MessageRepository {
        let database = Database::connect_in_memory()
            .await
            .expect("in-memory database should open");
        database.apply_schema().await.expect("schema should apply");

        MessageRepository::new(database.pool().clone())
    }

    #[tokio::test]
    async fn store_persists_a_message() {
        let repository = in_memory_repository().await;

        let outcome = repository
            .store(&new_message())
            .await
            .expect("store should succeed");
        assert_eq!(outcome, StoreOutcome::Stored);

        let row = sqlx::query(
            "SELECT chat_id, message_id, received_at, sender_id, sender_name, message FROM messages",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("stored row should exist");
        assert_eq!(row.get::<i64, _>("chat_id"), 42);
        assert_eq!(row.get::<i64, _>("message_id"), 7);
        assert_eq!(row.get::<i64, _>("received_at"), 1_700_000_000);
        assert_eq!(row.get::<Option<i64>, _>("sender_id"), Some(99));
        assert_eq!(
            row.get::<Option<String>, _>("sender_name"),
            Some("Ada".to_owned())
        );
        assert_eq!(
            row.get::<Option<String>, _>("message"),
            Some("hello".to_owned())
        );
    }

    #[tokio::test]
    async fn store_ignores_a_redelivery() {
        let repository = in_memory_repository().await;

        let first = repository
            .store(&new_message())
            .await
            .expect("first store should succeed");
        let second = repository
            .store(&new_message())
            .await
            .expect("second store should be ignored");
        assert_eq!(first, StoreOutcome::Stored);
        assert_eq!(second, StoreOutcome::Duplicate);

        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
            .fetch_one(&repository.pool)
            .await
            .expect("count should succeed");
        assert_eq!(rows, 1);
    }

    #[tokio::test]
    async fn store_accepts_a_message_without_text() {
        let repository = in_memory_repository().await;
        let message = NewMessage {
            text: None,
            ..new_message()
        };

        let outcome = repository
            .store(&message)
            .await
            .expect("store without text should succeed");
        assert_eq!(outcome, StoreOutcome::Stored);

        let text: Option<String> = sqlx::query_scalar("SELECT message FROM messages")
            .fetch_one(&repository.pool)
            .await
            .expect("stored row should exist");
        assert_eq!(text, None);
    }
}
