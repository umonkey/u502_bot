//! Attachment model and the repository that owns every statement against `attachments`.
//!
//! The row type stays independent of Telegram: the command that reads updates maps them into
//! [`NewAttachment`] before storing. [`AttachmentRepository`] must stay in sync with the
//! `attachments` table in `dev/schema.sql`.
//!
//! There is deliberately no foreign key to `messages`: both rows are written by the same loop in the
//! same order, and a constraint would only turn an attachment write into a failure mode of the
//! message write.

use std::fmt;

use sqlx::SqlitePool;

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

/// What [`AttachmentRepository::store`] did with an attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOutcome {
    /// The row was written.
    Stored,
    /// The row was already there, so the redelivery was ignored.
    Duplicate,
}

/// Failure modes of storing an attachment.
#[derive(Debug)]
pub enum AttachmentError {
    /// SQLite rejected the statement.
    Sqlite(sqlx::Error),
}

impl fmt::Display for AttachmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(source) => write!(formatter, "database error: {source}"),
        }
    }
}

impl std::error::Error for AttachmentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(source) => Some(source),
        }
    }
}

impl From<sqlx::Error> for AttachmentError {
    fn from(source: sqlx::Error) -> Self {
        Self::Sqlite(source)
    }
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
    /// Must stay in sync with the `attachments` table in `dev/schema.sql`: the column order and the
    /// `UNIQUE (chat_id, message_id)` constraint are what make this statement idempotent. The
    /// `transcribed` flag is written as a literal because nothing sets it yet.
    pub async fn store(&self, attachment: &NewAttachment) -> Result<StoreOutcome, AttachmentError> {
        let result = sqlx::query(
            "INSERT OR IGNORE INTO attachments \
             (chat_id, message_id, kind, file_id, mime_type, duration_seconds, file_size, transcribed) \
             VALUES (?, ?, ?, ?, ?, ?, ?, 0)",
        )
        .bind(attachment.chat_id)
        .bind(attachment.message_id)
        .bind(attachment.kind.as_str())
        .bind(attachment.file_id.as_str())
        .bind(attachment.mime_type.as_deref())
        .bind(attachment.duration_seconds)
        .bind(attachment.file_size)
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

    #[tokio::test]
    async fn store_persists_an_attachment() {
        let repository = in_memory_repository().await;

        let outcome = repository
            .store(&new_attachment())
            .await
            .expect("store should succeed");
        assert_eq!(outcome, StoreOutcome::Stored);

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
        assert_eq!(first, StoreOutcome::Stored);
        assert_eq!(second, StoreOutcome::Duplicate);

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
        assert_eq!(outcome, StoreOutcome::Stored);

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
        assert_eq!(first, StoreOutcome::Stored);
        assert_eq!(second, StoreOutcome::Stored);

        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM attachments")
            .fetch_one(&repository.pool)
            .await
            .expect("count should succeed");
        assert_eq!(rows, 2);
    }
}
