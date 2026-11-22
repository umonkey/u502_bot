//! Message model and the repository that owns every statement against `messages` and the one
//! transaction that also writes a message's attachment.
//!
//! The row type stays independent of Telegram: the command that reads updates maps them into
//! [`NewMessage`] before storing. [`insert`] and [`MessageRepository::store`] must stay in sync with
//! both tables in `dev/schema.sql`.

use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::domains::InsertOutcome;
use crate::domains::attachment::{self, NewAttachment};
use crate::shared::result::Result;

/// One message to write to the `messages` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessage {
    /// Telegram chat the message arrived in.
    pub chat_id: i64,
    /// Telegram message id, unique within its chat.
    pub message_id: i64,
    /// Telegram send time as Unix epoch seconds, UTC.
    pub received_at: i64,
    /// Telegram id of the sender, when the update carries one.
    pub sender_id: Option<i64>,
    /// Display name of the sender, when the update carries one.
    pub sender_name: Option<String>,
    /// Message text or caption; absent for photos, stickers, and service events.
    pub text: Option<String>,
}

/// What [`MessageRepository::store`] wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreOutcome {
    /// What happened to the `messages` row.
    pub message: InsertOutcome,
    /// What happened to the `attachments` row, when the message carried media.
    pub attachment: Option<InsertOutcome>,
}

/// One stored message as the export renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedMessage {
    /// Telegram message id, unique within its chat.
    pub message_id: i64,
    /// Telegram send time as Unix epoch seconds, UTC.
    pub received_at: i64,
    /// Display name of the sender, when the update carried one.
    pub sender_name: Option<String>,
    /// Message text or caption; never empty, because the query skips NULL rows.
    pub text: String,
}

/// Writes an arriving message and its attachment together.
pub struct MessageRepository {
    pool: SqlitePool,
}

impl MessageRepository {
    /// Creates a repository over a pool the database client already opened.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Stores one message and, when it carries media, its attachment in one transaction.
    ///
    /// Telegram acknowledges an update when the dispatcher fetches it, so a message whose write
    /// fails is lost: two independent statements could store the message and then lose its media
    /// forever. Both inserts always run, even when the message is a duplicate: a database written
    /// before this transaction existed can hold a message whose attachment insert failed, and a
    /// redelivery is the only chance to repair it. The attachment is still written after the message
    /// so a later reader can age-bound it by the time Telegram reported for the message that carries
    /// it.
    pub async fn store(
        &self,
        message: &NewMessage,
        attachment: Option<&NewAttachment>,
    ) -> Result<StoreOutcome> {
        let mut transaction = self.pool.begin().await?;

        let message_outcome = match insert(&mut transaction, message).await {
            Ok(outcome) => outcome,
            Err(error) => {
                let _ = transaction.rollback().await;

                return Err(error);
            }
        };

        let attachment_outcome = match attachment {
            Some(attachment) => match attachment::insert(&mut transaction, attachment).await {
                Ok(outcome) => Some(outcome),
                Err(error) => {
                    let _ = transaction.rollback().await;

                    return Err(error);
                }
            },
            None => None,
        };

        transaction.commit().await?;

        Ok(StoreOutcome {
            message: message_outcome,
            attachment: attachment_outcome,
        })
    }

    /// Deletes up to `limit` messages received before `received_before`.
    ///
    /// Run after the attachment prune, so each attachment is aged while its message still exists.
    pub async fn prune_received_before(&self, received_before: i64, limit: i64) -> Result<u64> {
        let deleted = sqlx::query(
            "DELETE FROM messages WHERE id IN ( \
             SELECT id FROM messages WHERE received_at < ? ORDER BY id LIMIT ?)",
        )
        .bind(received_before)
        .bind(limit)
        .execute(&self.pool)
        .await?
        .rows_affected();

        Ok(deleted)
    }

    /// Lists the distinct chats that received a message in `[start, end)`, ascending.
    ///
    /// Served by the `messages_received_at` index; a day's rows are small enough that filtering by
    /// chat afterwards is cheaper than a second index.
    pub async fn chat_ids_between(&self, start: i64, end: i64) -> Result<Vec<i64>> {
        let rows = sqlx::query(
            "SELECT DISTINCT chat_id FROM messages \
             WHERE received_at >= ? AND received_at < ? \
             ORDER BY chat_id",
        )
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|row| row.get("chat_id")).collect())
    }

    /// Reads one chat's renderable messages in `[start, end)`, oldest first.
    ///
    /// Rows whose `message` is NULL cannot be rendered and are skipped; captions share that column,
    /// so only genuinely text-less rows are omitted.
    pub async fn messages_between(
        &self,
        chat_id: i64,
        start: i64,
        end: i64,
    ) -> Result<Vec<ExportedMessage>> {
        let rows = sqlx::query(
            "SELECT message_id, received_at, sender_name, message FROM messages \
             WHERE chat_id = ? AND received_at >= ? AND received_at < ? \
               AND message IS NOT NULL \
             ORDER BY received_at, message_id",
        )
        .bind(chat_id)
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| ExportedMessage {
                message_id: row.get("message_id"),
                received_at: row.get("received_at"),
                sender_name: row.get("sender_name"),
                text: row.get("message"),
            })
            .collect())
    }
}

/// Inserts one row on the caller's connection, so the caller can insert inside its own transaction.
///
/// Must stay in sync with the `messages` table in `dev/schema.sql`: the column order and the
/// `UNIQUE (chat_id, message_id)` constraint are what make this statement idempotent. Naming that
/// conflict target keeps the redelivery the only silent case, so any other constraint failure
/// returns an error instead of a false [`InsertOutcome::Duplicate`], which the ingestor would count
/// as stored.
pub(crate) async fn insert(
    connection: &mut SqliteConnection,
    message: &NewMessage,
) -> Result<InsertOutcome> {
    let result = sqlx::query(
        "INSERT INTO messages (chat_id, message_id, received_at, sender_id, sender_name, message) \
         VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT (chat_id, message_id) DO NOTHING",
    )
    .bind(message.chat_id)
    .bind(message.message_id)
    .bind(message.received_at)
    .bind(message.sender_id)
    .bind(message.sender_name.as_deref())
    .bind(message.text.as_deref())
    .execute(&mut *connection)
    .await?;

    if result.rows_affected() == 0 {
        Ok(InsertOutcome::Duplicate)
    } else {
        Ok(InsertOutcome::Stored)
    }
}

#[cfg(test)]
mod tests {
    use crate::domains::attachment::{AttachmentKind, AttachmentRepository};
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

    fn new_attachment() -> NewAttachment {
        NewAttachment {
            chat_id: 42,
            message_id: 7,
            kind: AttachmentKind::Audio,
            file_id: "file-1".to_owned(),
            mime_type: Some("audio/ogg".to_owned()),
            duration_seconds: Some(3),
            file_size: Some(2048),
        }
    }

    async fn in_memory_repository() -> MessageRepository {
        let database = Database::connect_in_memory()
            .await
            .expect("in-memory database should open");
        database.apply_schema().await.expect("schema should apply");

        MessageRepository::new(database.pool().clone())
    }

    async fn count(repository: &MessageRepository, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&repository.pool)
            .await
            .expect("count should succeed")
    }

    async fn store_message(
        repository: &MessageRepository,
        chat_id: i64,
        message_id: i64,
        received_at: i64,
        sender_name: Option<&str>,
        text: Option<&str>,
    ) {
        repository
            .store(
                &NewMessage {
                    chat_id,
                    message_id,
                    received_at,
                    sender_id: None,
                    sender_name: sender_name.map(str::to_owned),
                    text: text.map(str::to_owned),
                },
                None,
            )
            .await
            .expect("the message should store");
    }

    async fn message_ids(repository: &MessageRepository) -> Vec<i64> {
        sqlx::query_scalar("SELECT id FROM messages ORDER BY id")
            .fetch_all(&repository.pool)
            .await
            .expect("rows should exist")
    }

    #[tokio::test]
    async fn prune_deletes_old_messages() {
        let repository = in_memory_repository().await;
        repository
            .store(&new_message(), None)
            .await
            .expect("old message should store");
        repository
            .store(
                &NewMessage {
                    message_id: 8,
                    received_at: 1_700_000_100,
                    ..new_message()
                },
                None,
            )
            .await
            .expect("recent message should store");

        let deleted = repository
            .prune_received_before(1_700_000_050, 10)
            .await
            .expect("prune should succeed");

        assert_eq!(deleted, 1);
        assert_eq!(message_ids(&repository).await, vec![2]);
    }

    #[tokio::test]
    async fn prune_keeps_messages_at_the_cutoff() {
        let repository = in_memory_repository().await;
        repository
            .store(&new_message(), None)
            .await
            .expect("message should store");

        let deleted = repository
            .prune_received_before(new_message().received_at, 10)
            .await
            .expect("prune should succeed");

        assert_eq!(deleted, 0);
        assert_eq!(count(&repository, "messages").await, 1);
    }

    #[tokio::test]
    async fn prune_honours_the_limit() {
        let repository = in_memory_repository().await;
        for message_id in 7..10 {
            repository
                .store(
                    &NewMessage {
                        message_id,
                        ..new_message()
                    },
                    None,
                )
                .await
                .expect("message should store");
        }

        let deleted = repository
            .prune_received_before(1_700_000_001, 2)
            .await
            .expect("prune should succeed");

        assert_eq!(deleted, 2);
        assert_eq!(message_ids(&repository).await, vec![3]);
    }

    #[tokio::test]
    async fn prune_is_idempotent() {
        let repository = in_memory_repository().await;
        repository
            .store(&new_message(), None)
            .await
            .expect("message should store");

        let first = repository
            .prune_received_before(1_700_000_001, 10)
            .await
            .expect("first prune should succeed");
        let second = repository
            .prune_received_before(1_700_000_001, 10)
            .await
            .expect("second prune should succeed");

        assert_eq!(first, 1);
        assert_eq!(second, 0);
        assert_eq!(count(&repository, "messages").await, 0);
    }

    #[tokio::test]
    async fn store_persists_a_message() {
        let repository = in_memory_repository().await;

        let outcome = repository
            .store(&new_message(), None)
            .await
            .expect("store should succeed");
        assert_eq!(
            outcome,
            StoreOutcome {
                message: InsertOutcome::Stored,
                attachment: None,
            }
        );

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
            .store(&new_message(), None)
            .await
            .expect("first store should succeed");
        let second = repository
            .store(&new_message(), None)
            .await
            .expect("second store should be ignored");
        assert_eq!(first.message, InsertOutcome::Stored);
        assert_eq!(second.message, InsertOutcome::Duplicate);

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
            .store(&message, None)
            .await
            .expect("store without text should succeed");
        assert_eq!(outcome.message, InsertOutcome::Stored);

        let text: Option<String> = sqlx::query_scalar("SELECT message FROM messages")
            .fetch_one(&repository.pool)
            .await
            .expect("stored row should exist");
        assert_eq!(text, None);
    }

    #[tokio::test]
    async fn store_persists_a_message_and_its_attachment() {
        let repository = in_memory_repository().await;

        let outcome = repository
            .store(&new_message(), Some(&new_attachment()))
            .await
            .expect("store should succeed");

        assert_eq!(
            outcome,
            StoreOutcome {
                message: InsertOutcome::Stored,
                attachment: Some(InsertOutcome::Stored),
            }
        );
        assert_eq!(count(&repository, "messages").await, 1);
        assert_eq!(count(&repository, "attachments").await, 1);
    }

    #[tokio::test]
    async fn store_persists_a_message_without_an_attachment() {
        let repository = in_memory_repository().await;

        let outcome = repository
            .store(&new_message(), None)
            .await
            .expect("store should succeed");

        assert_eq!(
            outcome,
            StoreOutcome {
                message: InsertOutcome::Stored,
                attachment: None,
            }
        );
        assert_eq!(count(&repository, "messages").await, 1);
        assert_eq!(count(&repository, "attachments").await, 0);
    }

    #[tokio::test]
    async fn store_ignores_a_redelivery_with_an_attachment() {
        let repository = in_memory_repository().await;

        let first = repository
            .store(&new_message(), Some(&new_attachment()))
            .await
            .expect("first store should succeed");
        let second = repository
            .store(&new_message(), Some(&new_attachment()))
            .await
            .expect("second store should be ignored");

        assert_eq!(
            first,
            StoreOutcome {
                message: InsertOutcome::Stored,
                attachment: Some(InsertOutcome::Stored),
            }
        );
        assert_eq!(
            second,
            StoreOutcome {
                message: InsertOutcome::Duplicate,
                attachment: Some(InsertOutcome::Duplicate),
            }
        );
        assert_eq!(count(&repository, "messages").await, 1);
        assert_eq!(count(&repository, "attachments").await, 1);
    }

    #[tokio::test]
    async fn store_rolls_back_the_message_when_the_attachment_fails() {
        let repository = in_memory_repository().await;
        sqlx::query(
            "CREATE TRIGGER reject_boom BEFORE INSERT ON attachments \
             WHEN NEW.file_id = 'boom' \
             BEGIN SELECT RAISE(ABORT, 'boom'); END;",
        )
        .execute(&repository.pool)
        .await
        .expect("trigger should be created");
        let attachment = NewAttachment {
            file_id: "boom".to_owned(),
            ..new_attachment()
        };

        let outcome = repository.store(&new_message(), Some(&attachment)).await;

        assert!(
            outcome.is_err(),
            "a rejected attachment should fail the call"
        );
        assert_eq!(count(&repository, "messages").await, 0);
        assert_eq!(count(&repository, "attachments").await, 0);
    }

    #[tokio::test]
    async fn store_rolls_back_the_attachment_when_the_message_fails() {
        let repository = in_memory_repository().await;
        sqlx::query(
            "CREATE TRIGGER reject_boom BEFORE INSERT ON messages \
             WHEN NEW.message_id = 7 \
             BEGIN SELECT RAISE(ABORT, 'boom'); END;",
        )
        .execute(&repository.pool)
        .await
        .expect("trigger should be created");

        let outcome = repository
            .store(&new_message(), Some(&new_attachment()))
            .await;

        assert!(outcome.is_err(), "a rejected message should fail the call");
        assert_eq!(count(&repository, "messages").await, 0);
        assert_eq!(count(&repository, "attachments").await, 0);
    }

    #[tokio::test]
    async fn store_rejects_a_violation_other_than_the_unique_key() {
        let repository = in_memory_repository().await;
        // A second unique index models a constraint bug that `INSERT OR IGNORE` swallowed as a
        // duplicate, hiding the failure from the ingestor, which counts a duplicate as stored.
        sqlx::query("CREATE UNIQUE INDEX reject_sender ON messages (sender_name)")
            .execute(&repository.pool)
            .await
            .expect("index should be created");

        repository
            .store(&new_message(), None)
            .await
            .expect("first store should succeed");
        let outcome = repository
            .store(
                &NewMessage {
                    message_id: 8,
                    ..new_message()
                },
                None,
            )
            .await;

        assert!(
            outcome.is_err(),
            "a constraint violation other than the unique key should fail the call"
        );
        assert_eq!(count(&repository, "messages").await, 1);
    }

    #[tokio::test]
    async fn store_skips_a_duplicate_message_but_writes_a_missing_attachment() {
        let repository = in_memory_repository().await;
        sqlx::query("INSERT INTO messages (chat_id, message_id, received_at) VALUES (?, ?, ?)")
            .bind(42_i64)
            .bind(7_i64)
            .bind(1_700_000_000_i64)
            .execute(&repository.pool)
            .await
            .expect("legacy message should store");

        let outcome = repository
            .store(&new_message(), Some(&new_attachment()))
            .await
            .expect("store should succeed");

        assert_eq!(
            outcome,
            StoreOutcome {
                message: InsertOutcome::Duplicate,
                attachment: Some(InsertOutcome::Stored),
            }
        );

        let pending = AttachmentRepository::new(repository.pool.clone())
            .pending(AttachmentKind::Audio, 1_700_000_000, 10)
            .await
            .expect("pending should succeed");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].message_id, 7);
    }

    #[tokio::test]
    async fn store_leaves_one_message_and_attachment_per_delivery() {
        let repository = in_memory_repository().await;
        let first_message = new_message();
        let first_attachment = new_attachment();
        let second_message = NewMessage {
            message_id: 8,
            ..new_message()
        };
        let second_attachment = NewAttachment {
            message_id: 8,
            file_id: "file-2".to_owned(),
            ..new_attachment()
        };

        repository
            .store(&first_message, Some(&first_attachment))
            .await
            .expect("first store should succeed");
        repository
            .store(&second_message, Some(&second_attachment))
            .await
            .expect("second store should succeed");

        assert_eq!(count(&repository, "messages").await, 2);
        assert_eq!(count(&repository, "attachments").await, 2);

        let joined: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM attachments AS a \
             JOIN messages AS m ON m.chat_id = a.chat_id AND m.message_id = a.message_id",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("join should succeed");
        assert_eq!(joined, 2);
    }

    #[tokio::test]
    async fn chat_ids_between_returns_distinct_ascending_ids_inside_the_range() {
        let repository = in_memory_repository().await;
        store_message(&repository, 42, 1, 100, Some("Ada"), Some("first")).await;
        store_message(&repository, 7, 2, 200, None, Some("second")).await;
        store_message(&repository, 42, 3, 300, None, Some("third")).await;
        store_message(&repository, 9, 4, 400, None, Some("at the end")).await;

        let chats = repository
            .chat_ids_between(100, 400)
            .await
            .expect("the chat ids should load");

        assert_eq!(chats, vec![7, 42]);
    }

    #[tokio::test]
    async fn messages_between_orders_and_skips_text_less_rows() {
        let repository = in_memory_repository().await;
        store_message(&repository, 42, 9, 300, Some("Ada"), Some("later")).await;
        store_message(&repository, 42, 8, 200, None, Some("earlier")).await;
        store_message(
            &repository,
            42,
            7,
            200,
            Some("Ada"),
            Some("first of the same second"),
        )
        .await;
        store_message(&repository, 7, 1, 200, Some("Bob"), Some("other chat")).await;
        store_message(&repository, 42, 10, 200, Some("Ada"), None).await;

        let messages = repository
            .messages_between(42, 100, 400)
            .await
            .expect("the messages should load");

        assert_eq!(
            messages,
            vec![
                ExportedMessage {
                    message_id: 7,
                    received_at: 200,
                    sender_name: Some("Ada".to_owned()),
                    text: "first of the same second".to_owned(),
                },
                ExportedMessage {
                    message_id: 8,
                    received_at: 200,
                    sender_name: None,
                    text: "earlier".to_owned(),
                },
                ExportedMessage {
                    message_id: 9,
                    received_at: 300,
                    sender_name: Some("Ada".to_owned()),
                    text: "later".to_owned(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn messages_between_includes_start_and_excludes_end() {
        let repository = in_memory_repository().await;
        store_message(&repository, 42, 1, 100, None, Some("at the start")).await;
        store_message(&repository, 42, 2, 399, None, Some("inside")).await;
        store_message(&repository, 42, 3, 400, None, Some("at the end")).await;

        let ids: Vec<i64> = repository
            .messages_between(42, 100, 400)
            .await
            .expect("the messages should load")
            .into_iter()
            .map(|message| message.message_id)
            .collect();

        assert_eq!(ids, vec![1, 2]);
    }
}
