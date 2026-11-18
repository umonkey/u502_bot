//! Message model and the repository that owns every statement against `messages` and the one
//! transaction that also writes a message's attachment.
//!
//! The row type stays independent of Telegram: the command that reads updates maps them into
//! [`NewMessage`] before storing. [`insert`] and [`MessageRepository::store`] must stay in sync with
//! both tables in `dev/schema.sql`.

use sqlx::{SqliteConnection, SqlitePool};

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
    use sqlx::Row;

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
}
