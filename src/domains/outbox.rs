//! Outbox model and the repository that owns every statement against `outbox`.
//!
//! A reply is queued as an outbox row instead of being sent inline, so a restart cannot lose it; a
//! separate service delivers the unsent rows and marks them sent. The row type stays independent of
//! Telegram: the producer maps its text into [`NewOutboxMessage`] before enqueueing. The stored text
//! is escaped Telegram HTML, because the sender posts every row with `ParseMode::Html`.
//! [`OutboxRepository`] must stay in sync with the `outbox` table in `dev/schema.sql`.

use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::shared::result::Result;

/// One message to write to the `outbox` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOutboxMessage {
    /// Telegram chat the message is delivered to.
    pub chat_id: i64,
    /// Telegram message id the delivery replies to; `None` for a plain message.
    pub thread_id: Option<i64>,
    /// Enqueue time as Unix epoch seconds, UTC.
    pub added_at: i64,
    /// Text to deliver: escaped Telegram HTML, rendered by the producer.
    pub message: String,
}

/// What [`OutboxRepository::enqueue_transcript`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueOutcome {
    Enqueued,
    /// The attachment was already processed; nothing was written.
    AlreadyProcessed,
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
    /// Telegram send time as Unix epoch seconds, UTC, of the message this row answers.
    ///
    /// `None` when the row answers no message, or when the `messages` row is missing.
    pub source_at: Option<i64>,
    /// Text to deliver: escaped Telegram HTML, rendered by the producer.
    pub message: String,
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

    /// Queues every reply of one transcript and marks the attachment processed, atomically.
    ///
    /// The marker and the rows commit together; [`EnqueueOutcome::AlreadyProcessed`] means another
    /// writer finished this attachment first.
    pub async fn enqueue_transcript(
        &self,
        attachment_id: i64,
        messages: &[NewOutboxMessage],
    ) -> Result<EnqueueOutcome> {
        let mut transaction = self.pool.begin().await?;

        let marked =
            sqlx::query("UPDATE attachments SET transcribed = 1 WHERE id = ? AND transcribed = 0")
                .bind(attachment_id)
                .execute(&mut *transaction)
                .await?
                .rows_affected();

        if marked == 0 {
            transaction.rollback().await?;

            return Ok(EnqueueOutcome::AlreadyProcessed);
        }

        for message in messages {
            if let Err(error) = insert(&mut transaction, message).await {
                let _ = transaction.rollback().await;

                return Err(error);
            }
        }

        transaction.commit().await?;

        Ok(EnqueueOutcome::Enqueued)
    }

    /// Reads up to `limit` undelivered rows, oldest first.
    ///
    /// The left join to `messages` fills [`PendingMessage::source_at`] with the source message's
    /// Telegram send time. A join, rather than an outbox column, needs no migration: the startup
    /// schema only creates missing tables. The column is `NULL` when the row answers no message
    /// (`thread_id IS NULL`) or when the `(chat_id, thread_id)` message row is missing.
    pub async fn pending(&self, limit: i64) -> Result<Vec<PendingMessage>> {
        let rows = sqlx::query(
            "SELECT o.id, o.chat_id, o.thread_id, o.message, m.received_at AS source_at \
             FROM outbox AS o \
             LEFT JOIN messages AS m ON m.chat_id = o.chat_id AND m.message_id = o.thread_id \
             WHERE o.sent_at IS NULL ORDER BY o.id LIMIT ?",
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
                source_at: row.get("source_at"),
                message: row.get("message"),
            })
            .collect();

        Ok(pending)
    }

    /// Marks one row delivered.
    ///
    /// The `sent_at IS NULL` guard makes a second call for the same row a no-op, so a retry after a
    /// successful delivery cannot rewrite the delivery time.
    pub async fn mark_sent(&self, id: i64, sent_at: i64) -> Result<()> {
        sqlx::query("UPDATE outbox SET sent_at = ? WHERE id = ? AND sent_at IS NULL")
            .bind(sent_at)
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Deletes up to `limit` delivered rows sent before `sent_before`, returning how many were deleted.
    ///
    /// `sent_at IS NOT NULL` is what makes this idempotent: a second call over the same rows finds
    /// nothing, because the rows it would have matched are already gone. Unsent rows are never touched,
    /// so a queued reply cannot be lost to retention.
    pub async fn prune_sent_before(&self, sent_before: i64, limit: i64) -> Result<u64> {
        let deleted = sqlx::query(
            "DELETE FROM outbox WHERE id IN ( \
             SELECT id FROM outbox WHERE sent_at IS NOT NULL AND sent_at < ? ORDER BY id LIMIT ?)",
        )
        .bind(sent_before)
        .bind(limit)
        .execute(&self.pool)
        .await?
        .rows_affected();

        Ok(deleted)
    }
}

/// Inserts one row on the caller's connection, so the caller can insert inside its own transaction.
async fn insert(connection: &mut SqliteConnection, message: &NewOutboxMessage) -> Result<i64> {
    let id = sqlx::query_scalar(
        "INSERT INTO outbox (chat_id, thread_id, added_at, sent_at, message) \
         VALUES (?, ?, ?, NULL, ?) RETURNING id",
    )
    .bind(message.chat_id)
    .bind(message.thread_id)
    .bind(message.added_at)
    .bind(message.message.as_str())
    .fetch_one(&mut *connection)
    .await?;

    Ok(id)
}

#[cfg(test)]
mod tests {
    use crate::domains::message::{MessageRepository, NewMessage};
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

    /// Stores the message that [`new_outbox_message`] answers, so the join finds it.
    async fn store_source_message(repository: &OutboxRepository) {
        let message = NewMessage {
            chat_id: 42,
            message_id: 7,
            received_at: 1_700_000_000,
            sender_id: None,
            sender_name: None,
            text: Some("transcribe me".to_owned()),
        };

        MessageRepository::new(repository.pool.clone())
            .store(&message, None)
            .await
            .expect("source message should store");
    }

    async fn in_memory_repository() -> OutboxRepository {
        let database = Database::connect_in_memory()
            .await
            .expect("in-memory database should open");
        database.apply_schema().await.expect("schema should apply");

        OutboxRepository::new(database.pool().clone())
    }

    /// Queues one row through the production insert.
    async fn queue(repository: &OutboxRepository, message: &NewOutboxMessage) -> i64 {
        let mut connection = repository
            .pool
            .acquire()
            .await
            .expect("connection should be acquired");

        insert(&mut connection, message)
            .await
            .expect("enqueue should succeed")
    }

    async fn store_attachment(repository: &OutboxRepository) -> i64 {
        sqlx::query(
            "INSERT INTO attachments (chat_id, message_id, kind, file_id) VALUES (?, ?, 'audio', ?) \
             RETURNING id",
        )
        .bind(42_i64)
        .bind(7_i64)
        .bind("file-1")
        .fetch_one(&repository.pool)
        .await
        .expect("attachment should store")
        .get("id")
    }

    async fn transcribed(repository: &OutboxRepository, attachment_id: i64) -> i64 {
        sqlx::query_scalar("SELECT transcribed FROM attachments WHERE id = ?")
            .bind(attachment_id)
            .fetch_one(&repository.pool)
            .await
            .expect("attachment should exist")
    }

    async fn outbox_rows(repository: &OutboxRepository) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM outbox")
            .fetch_one(&repository.pool)
            .await
            .expect("count should succeed")
    }

    const SENT_AT: i64 = 1_700_000_000;

    async fn queued_ids(repository: &OutboxRepository) -> Vec<i64> {
        sqlx::query_scalar("SELECT id FROM outbox ORDER BY id")
            .fetch_all(&repository.pool)
            .await
            .expect("rows should exist")
    }

    #[tokio::test]
    async fn prune_deletes_sent_rows_before_the_cutoff() {
        let repository = in_memory_repository().await;
        let old = queue(&repository, &new_outbox_message()).await;
        let recent = queue(&repository, &new_outbox_message()).await;
        let unsent = queue(&repository, &new_outbox_message()).await;
        repository
            .mark_sent(old, SENT_AT)
            .await
            .expect("mark_sent should succeed");
        repository
            .mark_sent(recent, SENT_AT + 100)
            .await
            .expect("mark_sent should succeed");

        let deleted = repository
            .prune_sent_before(SENT_AT + 50, 10)
            .await
            .expect("prune should succeed");

        assert_eq!(deleted, 1);
        assert_eq!(queued_ids(&repository).await, vec![recent, unsent]);
    }

    #[tokio::test]
    async fn prune_honours_the_limit() {
        let repository = in_memory_repository().await;
        let mut ids = Vec::new();
        for _ in 0..3 {
            ids.push(queue(&repository, &new_outbox_message()).await);
        }
        for id in &ids {
            repository
                .mark_sent(*id, SENT_AT)
                .await
                .expect("mark_sent should succeed");
        }

        let deleted = repository
            .prune_sent_before(SENT_AT + 1, 2)
            .await
            .expect("prune should succeed");

        assert_eq!(deleted, 2);
        assert_eq!(outbox_rows(&repository).await, 1);
        assert_eq!(queued_ids(&repository).await, vec![ids[2]]);
    }

    #[tokio::test]
    async fn prune_is_idempotent() {
        let repository = in_memory_repository().await;
        let id = queue(&repository, &new_outbox_message()).await;
        repository
            .mark_sent(id, SENT_AT)
            .await
            .expect("mark_sent should succeed");

        let first = repository
            .prune_sent_before(SENT_AT + 1, 10)
            .await
            .expect("first prune should succeed");
        let second = repository
            .prune_sent_before(SENT_AT + 1, 10)
            .await
            .expect("second prune should succeed");

        assert_eq!(first, 1);
        assert_eq!(second, 0);
        assert_eq!(outbox_rows(&repository).await, 0);
    }

    #[tokio::test]
    async fn enqueue_writes_an_unsent_row() {
        let repository = in_memory_repository().await;

        let id = queue(&repository, &new_outbox_message()).await;
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
            queue(&repository, &message).await;
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
            queue(&repository, &message).await;
        }

        let pending = repository.pending(2).await.expect("pending should succeed");
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].message, "first");
        assert_eq!(pending[1].message, "second");
    }

    #[tokio::test]
    async fn mark_sent_is_idempotent() {
        let repository = in_memory_repository().await;

        let id = queue(&repository, &new_outbox_message()).await;

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

        queue(&repository, &message).await;

        let pending = repository
            .pending(10)
            .await
            .expect("pending should succeed");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].thread_id, None);
    }

    #[tokio::test]
    async fn pending_reads_the_source_send_time() {
        let repository = in_memory_repository().await;
        store_source_message(&repository).await;

        queue(&repository, &new_outbox_message()).await;

        let pending = repository
            .pending(10)
            .await
            .expect("pending should succeed");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].source_at, Some(1_700_000_000));
    }

    #[tokio::test]
    async fn pending_reports_no_source_time_without_a_thread() {
        let repository = in_memory_repository().await;
        store_source_message(&repository).await;
        let message = NewOutboxMessage {
            thread_id: None,
            ..new_outbox_message()
        };

        queue(&repository, &message).await;

        let pending = repository
            .pending(10)
            .await
            .expect("pending should succeed");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].source_at, None);
    }

    #[tokio::test]
    async fn pending_reports_no_source_time_when_the_message_is_missing() {
        let repository = in_memory_repository().await;
        let message = NewOutboxMessage {
            thread_id: Some(99),
            ..new_outbox_message()
        };

        queue(&repository, &message).await;

        let pending = repository
            .pending(10)
            .await
            .expect("pending should succeed");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].source_at, None);
    }

    fn transcript_chunks(texts: &[&str]) -> Vec<NewOutboxMessage> {
        texts
            .iter()
            .map(|text| NewOutboxMessage {
                message: (*text).to_owned(),
                ..new_outbox_message()
            })
            .collect()
    }

    #[tokio::test]
    async fn enqueue_transcript_writes_every_chunk_and_marks_the_attachment() {
        let repository = in_memory_repository().await;
        let attachment_id = store_attachment(&repository).await;
        let messages = transcript_chunks(&["first", "second"]);

        let outcome = repository
            .enqueue_transcript(attachment_id, &messages)
            .await
            .expect("enqueue should succeed");

        assert_eq!(outcome, EnqueueOutcome::Enqueued);
        assert_eq!(outbox_rows(&repository).await, 2);
        assert_eq!(transcribed(&repository, attachment_id).await, 1);

        let unsent: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbox WHERE sent_at IS NULL")
            .fetch_one(&repository.pool)
            .await
            .expect("count should succeed");
        assert_eq!(unsent, 2);
    }

    #[tokio::test]
    async fn enqueue_transcript_answers_only_the_first_chunk() {
        let repository = in_memory_repository().await;
        let attachment_id = store_attachment(&repository).await;
        let messages = vec![
            NewOutboxMessage {
                thread_id: Some(7),
                ..new_outbox_message()
            },
            NewOutboxMessage {
                thread_id: None,
                ..new_outbox_message()
            },
        ];

        repository
            .enqueue_transcript(attachment_id, &messages)
            .await
            .expect("enqueue should succeed");

        let thread_ids: Vec<Option<i64>> =
            sqlx::query_scalar("SELECT thread_id FROM outbox ORDER BY id")
                .fetch_all(&repository.pool)
                .await
                .expect("rows should exist");
        assert_eq!(thread_ids, vec![Some(7), None]);
    }

    #[tokio::test]
    async fn enqueue_transcript_rolls_back_a_failed_chunk() {
        let repository = in_memory_repository().await;
        let attachment_id = store_attachment(&repository).await;
        sqlx::query(
            "CREATE TRIGGER reject_boom BEFORE INSERT ON outbox \
             WHEN NEW.message = 'boom' \
             BEGIN SELECT RAISE(ABORT, 'boom'); END;",
        )
        .execute(&repository.pool)
        .await
        .expect("trigger should be created");
        let messages = transcript_chunks(&["ok", "boom"]);

        let outcome = repository
            .enqueue_transcript(attachment_id, &messages)
            .await;

        assert!(outcome.is_err(), "a rejected chunk should fail the call");
        assert_eq!(outbox_rows(&repository).await, 0);
        assert_eq!(transcribed(&repository, attachment_id).await, 0);
    }

    #[tokio::test]
    async fn enqueue_transcript_skips_an_already_processed_attachment() {
        let repository = in_memory_repository().await;
        let attachment_id = store_attachment(&repository).await;
        let messages = transcript_chunks(&["first", "second"]);
        let first = repository
            .enqueue_transcript(attachment_id, &messages)
            .await
            .expect("first enqueue should succeed");
        assert_eq!(first, EnqueueOutcome::Enqueued);

        let second = repository
            .enqueue_transcript(attachment_id, &messages)
            .await
            .expect("second enqueue should succeed");

        assert_eq!(second, EnqueueOutcome::AlreadyProcessed);
        assert_eq!(outbox_rows(&repository).await, 2);
    }

    #[tokio::test]
    async fn enqueue_transcript_treats_a_missing_attachment_as_processed() {
        let repository = in_memory_repository().await;
        let messages = transcript_chunks(&["first"]);

        let outcome = repository
            .enqueue_transcript(99, &messages)
            .await
            .expect("enqueue should succeed");

        assert_eq!(outcome, EnqueueOutcome::AlreadyProcessed);
        assert_eq!(outbox_rows(&repository).await, 0);
    }
}
