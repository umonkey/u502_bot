//! Attachment model and the repository that owns every statement against `attachments`.
//!
//! The row type stays independent of Telegram: the command that reads updates maps them into
//! [`NewAttachment`] before storing. [`AttachmentRepository`] must stay in sync with the
//! `attachments` table in `dev/schema.sql`.
//!
//! There is deliberately no foreign key to `messages`: the message and its attachment commit
//! together inside [`crate::domains::message::MessageRepository::store`], and a constraint would
//! need a manual upgrade of existing volumes (ADR 0005 creates missing objects only, never
//! migrates).

use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::domains::InsertOutcome;
use crate::shared::result::Result;

/// The media family an attachment belongs to.
///
/// Telegram delivers at most one media payload per message, but a video note carries no MIME type,
/// so the family is recorded explicitly to let a later reader pick the right treatment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    /// A voice note or an audio file.
    Audio,
    /// A video or a video note.
    Video,
}

impl AttachmentKind {
    /// The value stored in the `kind` column.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Video => "video",
        }
    }

    /// Reads the value stored in the `kind` column, or `None` when it is unknown.
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "audio" => Some(Self::Audio),
            "video" => Some(Self::Video),
            _ => None,
        }
    }
}

/// One attachment to write to the `attachments` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAttachment {
    /// Telegram chat the message carrying the attachment arrived in.
    pub chat_id: i64,
    /// Telegram message id of the message carrying the attachment.
    pub message_id: i64,
    /// Media family, which selects the treatment a later reader applies.
    pub kind: AttachmentKind,
    /// Telegram handle used to fetch the file later.
    pub file_id: String,
    /// MIME type Telegram reported; absent for a video note.
    pub mime_type: Option<String>,
    /// Media duration in seconds, when Telegram reports one.
    pub duration_seconds: Option<i64>,
    /// Media size in bytes, when Telegram reports one.
    pub file_size: Option<i64>,
}

/// One attachment waiting to be processed.
///
/// Returned by [`AttachmentRepository::pending`]; it carries only what the transcription worker
/// needs to fetch the file, pick a treatment, and report progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAttachment {
    /// Primary key of the `attachments` row, used to mark it processed.
    pub id: i64,
    /// Telegram chat the message carrying the attachment arrived in.
    pub chat_id: i64,
    /// Telegram message id of the message carrying the attachment.
    pub message_id: i64,
    /// Media family, which selects the treatment the worker applies.
    pub kind: AttachmentKind,
    /// Telegram handle used to fetch the file.
    pub file_id: String,
    /// MIME type Telegram reported; absent for a video note.
    pub mime_type: Option<String>,
    /// Media size in bytes, when Telegram reported one.
    pub file_size: Option<i64>,
}

/// Writes attachments to the `attachments` table.
pub struct AttachmentRepository {
    pool: SqlitePool,
}

impl AttachmentRepository {
    /// Creates a repository over a pool the database client already opened.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Stores one attachment, ignoring a redelivery of a row the table already holds.
    ///
    /// Production writes the attachment inside
    /// [`crate::domains::message::MessageRepository::store`]'s transaction, so only the tests open
    /// this standalone form.
    #[cfg(test)]
    pub async fn store(&self, attachment: &NewAttachment) -> Result<InsertOutcome> {
        let mut connection = self.pool.acquire().await?;

        insert(&mut connection, attachment).await
    }

    /// Reads up to `limit` attachments of one `kind` that are still pending and recent enough.
    ///
    /// Only rows whose message arrived at or after `received_since` (Unix epoch seconds, UTC) are
    /// returned, so the worker's retries stay bounded by a window instead of revisiting old
    /// attachments forever. The `INNER JOIN` is deliberate: an attachment without a message row is a
    /// legacy artifact from before [`crate::domains::message::MessageRepository::store`] wrote both
    /// rows in one transaction, and such a row cannot be age-bounded.
    ///
    /// Must stay in sync with the `attachments` and `messages` tables in `dev/schema.sql`.
    pub async fn pending(
        &self,
        kind: AttachmentKind,
        received_since: i64,
        limit: i64,
    ) -> Result<Vec<PendingAttachment>> {
        let rows = sqlx::query(
            "SELECT a.id, a.chat_id, a.message_id, a.kind, a.file_id, a.mime_type, a.file_size \
             FROM attachments AS a \
             JOIN messages AS m ON m.chat_id = a.chat_id AND m.message_id = a.message_id \
             WHERE a.kind = ? AND a.transcribed = 0 AND m.received_at >= ? \
             ORDER BY a.id \
             LIMIT ?",
        )
        .bind(kind.as_str())
        .bind(received_since)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let pending = rows
            .into_iter()
            .filter_map(|row| {
                let kind = AttachmentKind::from_str(&row.get::<String, _>("kind"))?;

                Some(PendingAttachment {
                    id: row.get("id"),
                    chat_id: row.get("chat_id"),
                    message_id: row.get("message_id"),
                    kind,
                    file_id: row.get("file_id"),
                    mime_type: row.get("mime_type"),
                    file_size: row.get("file_size"),
                })
            })
            .collect();

        Ok(pending)
    }

    /// Marks one attachment processed, so it leaves [`Self::pending`] for good.
    ///
    /// `transcribed` means "processed: transcribed or deliberately skipped", not "has a transcript":
    /// a row whose chat is not white-listed is marked here too, because leaving it pending would
    /// repeat the same skip on every scan for the length of the window. This form is for an
    /// attachment finished without a reply; the transcript path marks it inside its transaction.
    pub async fn mark_transcribed(&self, id: i64) -> Result<()> {
        sqlx::query("UPDATE attachments SET transcribed = 1 WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Deletes up to `limit` attachments whose message is missing or arrived before `received_before`.
    ///
    /// The `transcribed` flag is not consulted: past the transcriber's one-hour window an unprocessed
    /// row can never be attempted, so the flag no longer separates live work from dead weight. The left
    /// join also matches a legacy row written before a message and its attachment committed together;
    /// such a row has no timestamp to age, and no pending scan can reach it either.
    pub async fn prune_for_messages_before(&self, received_before: i64, limit: i64) -> Result<u64> {
        let deleted = sqlx::query(
            "DELETE FROM attachments WHERE id IN ( \
             SELECT a.id FROM attachments AS a \
             LEFT JOIN messages AS m ON m.chat_id = a.chat_id AND m.message_id = a.message_id \
             WHERE m.id IS NULL OR m.received_at < ? \
             ORDER BY a.id LIMIT ?)",
        )
        .bind(received_before)
        .bind(limit)
        .execute(&self.pool)
        .await?
        .rows_affected();

        Ok(deleted)
    }
}

/// Inserts one row on the caller's connection, so the caller can insert inside its own transaction.
///
/// Must stay in sync with the `attachments` table in `dev/schema.sql`: the column order and the
/// `UNIQUE (chat_id, message_id)` constraint are what make this statement idempotent. Naming that
/// conflict target keeps the redelivery the only silent case, so any other constraint failure
/// returns an error instead of a false [`InsertOutcome::Duplicate`], which the ingestor would count
/// as stored. The `transcribed` flag is written as a literal because nothing sets it here.
pub(crate) async fn insert(
    connection: &mut SqliteConnection,
    attachment: &NewAttachment,
) -> Result<InsertOutcome> {
    let result = sqlx::query(
        "INSERT INTO attachments \
         (chat_id, message_id, kind, file_id, mime_type, duration_seconds, file_size, transcribed) \
         VALUES (?, ?, ?, ?, ?, ?, ?, 0) ON CONFLICT (chat_id, message_id) DO NOTHING",
    )
    .bind(attachment.chat_id)
    .bind(attachment.message_id)
    .bind(attachment.kind.as_str())
    .bind(attachment.file_id.as_str())
    .bind(attachment.mime_type.as_deref())
    .bind(attachment.duration_seconds)
    .bind(attachment.file_size)
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
    use crate::infra::database::Database;

    use super::*;

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

    async fn in_memory_repository() -> AttachmentRepository {
        let database = Database::connect_in_memory()
            .await
            .expect("in-memory database should open");
        database.apply_schema().await.expect("schema should apply");

        AttachmentRepository::new(database.pool().clone())
    }

    const RECEIVED_AT: i64 = 1_700_000_000;

    fn audio_for(message_id: i64) -> NewAttachment {
        NewAttachment {
            message_id,
            ..new_attachment()
        }
    }

    async fn store_message(repository: &AttachmentRepository, message_id: i64, received_at: i64) {
        sqlx::query("INSERT INTO messages (chat_id, message_id, received_at) VALUES (?, ?, ?)")
            .bind(42_i64)
            .bind(message_id)
            .bind(received_at)
            .execute(&repository.pool)
            .await
            .expect("message should store");
    }

    async fn attachment_ids(repository: &AttachmentRepository) -> Vec<i64> {
        sqlx::query_scalar("SELECT id FROM attachments ORDER BY id")
            .fetch_all(&repository.pool)
            .await
            .expect("rows should exist")
    }

    #[tokio::test]
    async fn prune_deletes_old_and_orphan_attachments() {
        let repository = in_memory_repository().await;
        store_message(&repository, 7, RECEIVED_AT).await;
        store_message(&repository, 8, RECEIVED_AT + 1_000).await;
        repository
            .store(&audio_for(7))
            .await
            .expect("old audio should store");
        repository
            .store(&audio_for(8))
            .await
            .expect("recent audio should store");
        repository
            .store(&audio_for(99))
            .await
            .expect("orphan audio should store");
        repository
            .mark_transcribed(2)
            .await
            .expect("mark_transcribed should succeed");

        let deleted = repository
            .prune_for_messages_before(RECEIVED_AT + 500, 10)
            .await
            .expect("prune should succeed");

        assert_eq!(deleted, 2);
        assert_eq!(attachment_ids(&repository).await, vec![2]);
    }

    #[tokio::test]
    async fn prune_honours_the_limit() {
        let repository = in_memory_repository().await;
        for message_id in 7..10 {
            store_message(&repository, message_id, RECEIVED_AT).await;
            repository
                .store(&audio_for(message_id))
                .await
                .expect("audio should store");
        }

        let deleted = repository
            .prune_for_messages_before(RECEIVED_AT + 1, 2)
            .await
            .expect("prune should succeed");

        assert_eq!(deleted, 2);
        assert_eq!(attachment_ids(&repository).await, vec![3]);
    }

    #[tokio::test]
    async fn prune_is_idempotent() {
        let repository = in_memory_repository().await;
        store_message(&repository, 7, RECEIVED_AT).await;
        repository
            .store(&audio_for(7))
            .await
            .expect("audio should store");

        let first = repository
            .prune_for_messages_before(RECEIVED_AT + 1, 10)
            .await
            .expect("first prune should succeed");
        let second = repository
            .prune_for_messages_before(RECEIVED_AT + 1, 10)
            .await
            .expect("second prune should succeed");

        assert_eq!(first, 1);
        assert_eq!(second, 0);
        assert!(attachment_ids(&repository).await.is_empty());
    }

    #[tokio::test]
    async fn store_persists_an_attachment() {
        let repository = in_memory_repository().await;

        let outcome = repository
            .store(&new_attachment())
            .await
            .expect("store should succeed");
        assert_eq!(outcome, InsertOutcome::Stored);

        let row = sqlx::query(
            "SELECT chat_id, message_id, kind, file_id, mime_type, duration_seconds, file_size, transcribed FROM attachments",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("stored row should exist");
        assert_eq!(row.get::<i64, _>("chat_id"), 42);
        assert_eq!(row.get::<i64, _>("message_id"), 7);
        assert_eq!(row.get::<String, _>("kind"), "audio");
        assert_eq!(row.get::<String, _>("file_id"), "file-1");
        assert_eq!(
            row.get::<Option<String>, _>("mime_type"),
            Some("audio/ogg".to_owned())
        );
        assert_eq!(row.get::<Option<i64>, _>("duration_seconds"), Some(3));
        assert_eq!(row.get::<Option<i64>, _>("file_size"), Some(2048));
        assert_eq!(row.get::<i64, _>("transcribed"), 0);
    }

    #[tokio::test]
    async fn store_ignores_a_redelivery() {
        let repository = in_memory_repository().await;

        let first = repository
            .store(&new_attachment())
            .await
            .expect("first store should succeed");
        let second = repository
            .store(&new_attachment())
            .await
            .expect("second store should be ignored");
        assert_eq!(first, InsertOutcome::Stored);
        assert_eq!(second, InsertOutcome::Duplicate);

        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM attachments")
            .fetch_one(&repository.pool)
            .await
            .expect("count should succeed");
        assert_eq!(rows, 1);
    }

    #[tokio::test]
    async fn store_rejects_a_violation_other_than_the_unique_key() {
        let repository = in_memory_repository().await;
        // A second unique index models a constraint bug that `INSERT OR IGNORE` swallowed as a
        // duplicate, hiding the failure from the ingestor, which counts a duplicate as stored.
        sqlx::query("CREATE UNIQUE INDEX reject_file ON attachments (file_id)")
            .execute(&repository.pool)
            .await
            .expect("index should be created");

        let first = repository
            .store(&new_attachment())
            .await
            .expect("first store should succeed");
        assert_eq!(first, InsertOutcome::Stored);

        let outcome = repository
            .store(&NewAttachment {
                message_id: 8,
                ..new_attachment()
            })
            .await;

        assert!(
            outcome.is_err(),
            "a constraint violation other than the unique key should fail the call"
        );

        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM attachments")
            .fetch_one(&repository.pool)
            .await
            .expect("count should succeed");
        assert_eq!(rows, 1);
    }

    #[tokio::test]
    async fn store_accepts_missing_optional_metadata() {
        let repository = in_memory_repository().await;
        let attachment = NewAttachment {
            mime_type: None,
            duration_seconds: None,
            file_size: None,
            ..new_attachment()
        };

        let outcome = repository
            .store(&attachment)
            .await
            .expect("store without optional metadata should succeed");
        assert_eq!(outcome, InsertOutcome::Stored);

        let row = sqlx::query("SELECT mime_type, duration_seconds, file_size FROM attachments")
            .fetch_one(&repository.pool)
            .await
            .expect("stored row should exist");
        assert_eq!(row.get::<Option<String>, _>("mime_type"), None);
        assert_eq!(row.get::<Option<i64>, _>("duration_seconds"), None);
        assert_eq!(row.get::<Option<i64>, _>("file_size"), None);
    }

    #[tokio::test]
    async fn store_keeps_audio_and_video_for_different_messages() {
        let repository = in_memory_repository().await;
        let audio = new_attachment();
        let video = NewAttachment {
            message_id: 8,
            kind: AttachmentKind::Video,
            file_id: "file-2".to_owned(),
            mime_type: Some("video/mp4".to_owned()),
            ..new_attachment()
        };

        let first = repository
            .store(&audio)
            .await
            .expect("storing audio should succeed");
        let second = repository
            .store(&video)
            .await
            .expect("storing video should succeed");
        assert_eq!(first, InsertOutcome::Stored);
        assert_eq!(second, InsertOutcome::Stored);

        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM attachments")
            .fetch_one(&repository.pool)
            .await
            .expect("count should succeed");
        assert_eq!(rows, 2);
    }

    #[tokio::test]
    async fn pending_audio_returns_audio_rows_and_skips_video() {
        let repository = in_memory_repository().await;
        store_message(&repository, 7, RECEIVED_AT).await;
        store_message(&repository, 8, RECEIVED_AT).await;
        repository
            .store(&audio_for(7))
            .await
            .expect("audio should store");
        let video = NewAttachment {
            message_id: 8,
            kind: AttachmentKind::Video,
            file_id: "file-2".to_owned(),
            mime_type: Some("video/mp4".to_owned()),
            ..new_attachment()
        };
        repository.store(&video).await.expect("video should store");

        let pending = repository
            .pending(AttachmentKind::Audio, RECEIVED_AT, 10)
            .await
            .expect("pending should succeed");

        assert_eq!(
            pending,
            vec![PendingAttachment {
                id: 1,
                chat_id: 42,
                message_id: 7,
                kind: AttachmentKind::Audio,
                file_id: "file-1".to_owned(),
                mime_type: Some("audio/ogg".to_owned()),
                file_size: Some(2048),
            }]
        );
    }

    #[tokio::test]
    async fn pending_video_returns_video_rows_and_skips_audio() {
        let repository = in_memory_repository().await;
        store_message(&repository, 7, RECEIVED_AT).await;
        store_message(&repository, 8, RECEIVED_AT).await;
        repository
            .store(&audio_for(7))
            .await
            .expect("audio should store");
        let video = NewAttachment {
            message_id: 8,
            kind: AttachmentKind::Video,
            file_id: "file-2".to_owned(),
            mime_type: Some("video/mp4".to_owned()),
            ..new_attachment()
        };
        repository.store(&video).await.expect("video should store");

        let pending = repository
            .pending(AttachmentKind::Video, RECEIVED_AT, 10)
            .await
            .expect("pending should succeed");

        assert_eq!(
            pending,
            vec![PendingAttachment {
                id: 2,
                chat_id: 42,
                message_id: 8,
                kind: AttachmentKind::Video,
                file_id: "file-2".to_owned(),
                mime_type: Some("video/mp4".to_owned()),
                file_size: Some(2048),
            }]
        );
    }

    #[tokio::test]
    async fn pending_marks_the_kind_it_returns() {
        let repository = in_memory_repository().await;
        store_message(&repository, 7, RECEIVED_AT).await;
        repository
            .store(&audio_for(7))
            .await
            .expect("audio should store");

        let pending = repository
            .pending(AttachmentKind::Audio, RECEIVED_AT, 10)
            .await
            .expect("pending should succeed");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].kind, AttachmentKind::Audio);

        let pending = repository
            .pending(AttachmentKind::Video, RECEIVED_AT, 10)
            .await
            .expect("pending should succeed");
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn pending_audio_skips_a_transcribed_attachment() {
        let repository = in_memory_repository().await;
        store_message(&repository, 7, RECEIVED_AT).await;
        repository
            .store(&audio_for(7))
            .await
            .expect("audio should store");

        repository
            .mark_transcribed(1)
            .await
            .expect("mark_transcribed should succeed");

        let pending = repository
            .pending(AttachmentKind::Audio, RECEIVED_AT, 10)
            .await
            .expect("pending should succeed");
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn pending_audio_skips_messages_older_than_the_cutoff() {
        let repository = in_memory_repository().await;
        store_message(&repository, 7, RECEIVED_AT).await;
        store_message(&repository, 8, RECEIVED_AT + 60).await;
        repository
            .store(&audio_for(7))
            .await
            .expect("old audio should store");
        repository
            .store(&audio_for(8))
            .await
            .expect("recent audio should store");

        let pending = repository
            .pending(AttachmentKind::Audio, RECEIVED_AT + 60, 10)
            .await
            .expect("pending should succeed");

        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].message_id, 8);
    }

    #[tokio::test]
    async fn pending_audio_skips_an_attachment_without_a_message() {
        let repository = in_memory_repository().await;
        repository
            .store(&audio_for(7))
            .await
            .expect("audio should store");

        let pending = repository
            .pending(AttachmentKind::Audio, RECEIVED_AT, 10)
            .await
            .expect("pending should succeed");
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn pending_audio_drops_a_row_after_mark_transcribed() {
        let repository = in_memory_repository().await;
        store_message(&repository, 7, RECEIVED_AT).await;
        store_message(&repository, 8, RECEIVED_AT).await;
        repository
            .store(&audio_for(7))
            .await
            .expect("first audio should store");
        repository
            .store(&audio_for(8))
            .await
            .expect("second audio should store");

        repository
            .mark_transcribed(1)
            .await
            .expect("mark_transcribed should succeed");

        let pending = repository
            .pending(AttachmentKind::Audio, RECEIVED_AT, 10)
            .await
            .expect("pending should succeed");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, 2);

        let transcribed: i64 =
            sqlx::query_scalar("SELECT transcribed FROM attachments WHERE id = 1")
                .fetch_one(&repository.pool)
                .await
                .expect("first attachment should exist");
        assert_eq!(transcribed, 1);
    }
}
