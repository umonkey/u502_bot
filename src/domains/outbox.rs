//! Outbox model and the repository that owns every statement against `outbox`.
//!
//! A reply is queued as an outbox row instead of being sent inline, so a restart cannot lose it; a
//! separate service delivers the unsent rows and marks them sent. The row type stays independent of
//! Telegram: the producer maps its text into [`NewOutboxMessage`] before enqueueing.
//! [`OutboxRepository`] must stay in sync with the `outbox` table in `dev/schema.sql`.

use std::fmt;

use sqlx::{Row, SqlitePool};

/// One message to write to the `outbox` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOutboxMessage {
    /// Telegram chat the message is delivered to.
    pub chat_id: i64,
    /// Telegram message id the delivery replies to; `None` for a plain message.
    pub thread_id: Option<i64>,
    /// Enqueue time as Unix epoch seconds, UTC.
    pub added_at: i64,
    /// Text to deliver.
    pub message: String,
}

/// One queued row that has not been delivered yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingMessage {
    /// Primary key of the row.
    pub id: i64,
    /// Telegram chat the message is delivered to.
    pub chat_id: i64,
    /// Telegram message id the delivery replies to; `None` for a plain message.
    pub thread_id: Option<i64>,
    /// Text to deliver.
    pub message: String,
}

/// Failure modes of reading or writing the outbox.
#[derive(Debug)]
pub enum OutboxError {
    /// SQLite rejected the statement.
    Sqlite(sqlx::Error),
}

impl fmt::Display for OutboxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(source) => write!(formatter, "database error: {source}"),
        }
    }
}

impl std::error::Error for OutboxError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(source) => Some(source),
        }
    }
}

impl From<sqlx::Error> for OutboxError {
    fn from(source: sqlx::Error) -> Self {
        Self::Sqlite(source)
    }
}

/// Queues and reads the messages waiting to be delivered.
pub struct OutboxRepository {
    pool: SqlitePool,
}

impl OutboxRepository {
    /// Creates a repository over a pool the database client already opened.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Queues one message and returns the id of the new row.
    ///
    /// Must stay in sync with the `outbox` table in `dev/schema.sql`: `sent_at` is written as `NULL`
    /// because a queued row is by definition undelivered.
    pub async fn enqueue(&self, message: &NewOutboxMessage) -> Result<i64, OutboxError> {
        let id = sqlx::query_scalar(
            "INSERT INTO outbox (chat_id, thread_id, added_at, sent_at, message) \
             VALUES (?, ?, ?, NULL, ?) RETURNING id",
        )
        .bind(message.chat_id)
        .bind(message.thread_id)
        .bind(message.added_at)
        .bind(message.message.as_str())
        .fetch_one(&self.pool)
        .await?;

        Ok(id)
    }

    /// Reads up to `limit` undelivered rows, oldest first.
    pub async fn pending(&self, limit: i64) -> Result<Vec<PendingMessage>, OutboxError> {
        let rows = sqlx::query(
            "SELECT id, chat_id, thread_id, message FROM outbox \
             WHERE sent_at IS NULL ORDER BY id LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let pending = rows
            .into_iter()
            .map(|row| PendingMessage {
                id: row.get("id"),
                chat_id: row.get("chat_id"),
                thread_id: row.get("thread_id"),
                message: row.get("message"),
            })
            .collect();

        Ok(pending)
    }

    /// Marks one row delivered.
    ///
    /// The `sent_at IS NULL` guard makes a second call for the same row a no-op, so a retry after a
    /// successful delivery cannot rewrite the delivery time.
    pub async fn mark_sent(&self, id: i64, sent_at: i64) -> Result<(), OutboxError> {
        sqlx::query("UPDATE outbox SET sent_at = ? WHERE id = ? AND sent_at IS NULL")
            .bind(sent_at)
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::infra::database::Database;

    use super::*;

    fn new_outbox_message() -> NewOutboxMessage {
        NewOutboxMessage {
            chat_id: 42,
            thread_id: Some(7),
            added_at: 1_700_000_000,
            message: "hello".to_owned(),
        }
    }

    async fn in_memory_repository() -> OutboxRepository {
        let database = Database::connect_in_memory()
            .await
            .expect("in-memory database should open");
        database.apply_schema().await.expect("schema should apply");

        OutboxRepository::new(database.pool().clone())
    }

    #[tokio::test]
    async fn enqueue_writes_an_unsent_row() {
        let repository = in_memory_repository().await;

        let id = repository
            .enqueue(&new_outbox_message())
            .await
            .expect("enqueue should succeed");
        assert_eq!(id, 1);

        let row = sqlx::query("SELECT chat_id, thread_id, added_at, sent_at, message FROM outbox")
            .fetch_one(&repository.pool)
            .await
            .expect("queued row should exist");
        assert_eq!(row.get::<i64, _>("chat_id"), 42);
        assert_eq!(row.get::<Option<i64>, _>("thread_id"), Some(7));
        assert_eq!(row.get::<i64, _>("added_at"), 1_700_000_000);
        assert_eq!(row.get::<Option<i64>, _>("sent_at"), None);
        assert_eq!(row.get::<String, _>("message"), "hello");
    }

    #[tokio::test]
    async fn pending_returns_only_unsent_rows_in_id_order() {
        let repository = in_memory_repository().await;

        for text in ["first", "second", "third"] {
            let message = NewOutboxMessage {
                message: text.to_owned(),
                ..new_outbox_message()
            };
            repository
                .enqueue(&message)
                .await
                .expect("enqueue should succeed");
        }

        let pending = repository
            .pending(10)
            .await
            .expect("pending should succeed");
        let texts: Vec<&str> = pending
            .iter()
            .map(|message| message.message.as_str())
            .collect();
        assert_eq!(texts, vec!["first", "second", "third"]);

        repository
            .mark_sent(pending[1].id, 1_700_000_100)
            .await
            .expect("mark_sent should succeed");

        let pending = repository
            .pending(10)
            .await
            .expect("pending should succeed");
        let texts: Vec<&str> = pending
            .iter()
            .map(|message| message.message.as_str())
            .collect();
        assert_eq!(texts, vec!["first", "third"]);
    }

    #[tokio::test]
    async fn pending_honours_the_limit() {
        let repository = in_memory_repository().await;

        for text in ["first", "second", "third"] {
            let message = NewOutboxMessage {
                message: text.to_owned(),
                ..new_outbox_message()
            };
            repository
                .enqueue(&message)
                .await
                .expect("enqueue should succeed");
        }

        let pending = repository.pending(2).await.expect("pending should succeed");
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].message, "first");
        assert_eq!(pending[1].message, "second");
    }

    #[tokio::test]
    async fn mark_sent_is_idempotent() {
        let repository = in_memory_repository().await;

        let id = repository
            .enqueue(&new_outbox_message())
            .await
            .expect("enqueue should succeed");

        repository
            .mark_sent(id, 1_700_000_100)
            .await
            .expect("first mark_sent should succeed");
        repository
            .mark_sent(id, 1_700_000_200)
            .await
            .expect("second mark_sent should be a no-op");

        let sent_at: Option<i64> = sqlx::query_scalar("SELECT sent_at FROM outbox WHERE id = ?")
            .bind(id)
            .fetch_one(&repository.pool)
            .await
            .expect("queued row should exist");
        assert_eq!(sent_at, Some(1_700_000_100));

        let pending = repository
            .pending(10)
            .await
            .expect("pending should succeed");
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn pending_round_trips_a_missing_thread_id() {
        let repository = in_memory_repository().await;
        let message = NewOutboxMessage {
            thread_id: None,
            ..new_outbox_message()
        };

        repository
            .enqueue(&message)
            .await
            .expect("enqueue should succeed");

        let pending = repository
            .pending(10)
            .await
            .expect("pending should succeed");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].thread_id, None);
    }
}
