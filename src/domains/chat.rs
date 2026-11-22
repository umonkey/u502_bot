//! Chat registry and the repository that owns every statement against `chats`.
//!
//! The bot records every chat it receives a message from, and the owner's decision on one row gates
//! transcription for that chat. [`ChatRepository`] must stay in sync with the `chats` table in
//! `dev/schema.sql`.

use std::collections::HashSet;

use sqlx::{Row, SqlitePool};

use crate::domains::InsertOutcome;
use crate::shared::result::Result;

/// The owner's decision on one chat.
///
/// Unknown values read back as [`ChatStatus::Pending`]: two services read this column, and a
/// hand-edited row must never become approved by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatStatus {
    /// Recorded, not approved yet: its attachments are skipped.
    Pending,
    /// Transcribed.
    Approved,
    /// Deliberately refused.
    Blocked,
}

impl ChatStatus {
    /// The value stored in the `status` column.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Blocked => "blocked",
        }
    }

    /// Reads the value stored in the `status` column, or `None` when it is unknown.
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "approved" => Some(Self::Approved),
            "blocked" => Some(Self::Blocked),
            _ => None,
        }
    }
}

/// One chat to write to the `chats` table.
///
/// `kind` stays a plain string because nothing branches on it and it only names what Telegram
/// reported; `status` is typed because both services branch on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewChat {
    /// Telegram chat id.
    pub chat_id: i64,
    /// What Telegram reported: `"private"`, `"group"`, `"supergroup"`, or `"channel"`.
    pub kind: &'static str,
    /// Chat title; absent for a private chat.
    pub title: Option<String>,
    /// First observation time as Unix epoch seconds, UTC.
    pub added_at: i64,
    /// Latest observation time as Unix epoch seconds, UTC.
    pub seen_at: i64,
}

/// One recorded chat, as the owner's interface lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatRow {
    /// Telegram chat id.
    pub chat_id: i64,
    /// What Telegram reported when the chat was first seen.
    pub kind: String,
    /// Chat title; absent for a private chat.
    pub title: Option<String>,
    /// The owner's decision.
    pub status: ChatStatus,
}

/// Records chats and reads the owner's decisions.
pub struct ChatRepository {
    pool: SqlitePool,
}

impl ChatRepository {
    /// Creates a repository over a pool the database client already opened.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Records one chat, reporting whether this delivery created the row.
    ///
    /// A row is inserted pending; on a later delivery only `kind`, `title`, and `seen_at` are
    /// refreshed, so `status` and `added_at` keep the operator's decision and the first sighting.
    /// The insert and the update cannot interleave with a competing insert, because the ingestor is
    /// the only writer of this method in this process.
    ///
    /// Must stay in sync with the `chats` table in `dev/schema.sql`.
    pub async fn record(&self, chat: &NewChat) -> Result<InsertOutcome> {
        let inserted = sqlx::query(
            "INSERT INTO chats (chat_id, kind, title, status, added_at, seen_at) \
             VALUES (?, ?, ?, 'pending', ?, ?) ON CONFLICT (chat_id) DO NOTHING",
        )
        .bind(chat.chat_id)
        .bind(chat.kind)
        .bind(chat.title.as_deref())
        .bind(chat.added_at)
        .bind(chat.seen_at)
        .execute(&self.pool)
        .await?
        .rows_affected();

        if inserted == 1 {
            return Ok(InsertOutcome::Stored);
        }

        sqlx::query("UPDATE chats SET kind = ?, title = ?, seen_at = ? WHERE chat_id = ?")
            .bind(chat.kind)
            .bind(chat.title.as_deref())
            .bind(chat.seen_at)
            .bind(chat.chat_id)
            .execute(&self.pool)
            .await?;

        Ok(InsertOutcome::Duplicate)
    }

    /// Returns the ids of the chats whose attachments may be transcribed.
    pub async fn approved(&self) -> Result<HashSet<i64>> {
        let approved = sqlx::query_scalar("SELECT chat_id FROM chats WHERE status = 'approved'")
            .fetch_all(&self.pool)
            .await?;

        Ok(approved.into_iter().collect())
    }

    /// Lists the chats the owner can still decide on.
    ///
    /// A blocked row is omitted: it is settled. An unreadable `status` reads back as pending, so a
    /// damaged row stays visible and unapproved instead of disappearing from the interface.
    pub async fn list(&self) -> Result<Vec<ChatRow>> {
        let rows = sqlx::query(
            "SELECT chat_id, kind, title, status FROM chats \
             WHERE status IN ('pending', 'approved') \
             ORDER BY status, COALESCE(title, ''), chat_id",
        )
        .fetch_all(&self.pool)
        .await?;

        let chats = rows
            .into_iter()
            .map(|row| ChatRow {
                chat_id: row.get("chat_id"),
                kind: row.get("kind"),
                title: row.get("title"),
                status: ChatStatus::from_str(&row.get::<String, _>("status"))
                    .unwrap_or(ChatStatus::Pending),
            })
            .collect();

        Ok(chats)
    }

    /// Sets the owner's decision on one chat; `false` means no such chat is recorded.
    pub async fn set_status(&self, chat_id: i64, status: ChatStatus) -> Result<bool> {
        let updated = sqlx::query("UPDATE chats SET status = ? WHERE chat_id = ?")
            .bind(status.as_str())
            .bind(chat_id)
            .execute(&self.pool)
            .await?
            .rows_affected();

        Ok(updated == 1)
    }
}

#[cfg(test)]
mod tests {
    use crate::infra::database::Database;

    use super::*;

    const ADDED_AT: i64 = 1_700_000_000;

    async fn in_memory_repository() -> ChatRepository {
        let database = Database::connect_in_memory()
            .await
            .expect("in-memory database should open");
        database.apply_schema().await.expect("schema should apply");

        ChatRepository::new(database.pool().clone())
    }

    fn new_chat() -> NewChat {
        NewChat {
            chat_id: 42,
            kind: "group",
            title: Some("Car talk".to_owned()),
            added_at: ADDED_AT,
            seen_at: ADDED_AT,
        }
    }

    async fn status_of(repository: &ChatRepository, chat_id: i64) -> String {
        sqlx::query_scalar("SELECT status FROM chats WHERE chat_id = ?")
            .bind(chat_id)
            .fetch_one(&repository.pool)
            .await
            .expect("chat should exist")
    }

    #[test]
    fn chat_status_round_trips_and_rejects_an_unknown_value() {
        for status in [
            ChatStatus::Pending,
            ChatStatus::Approved,
            ChatStatus::Blocked,
        ] {
            assert_eq!(ChatStatus::from_str(status.as_str()), Some(status));
        }

        assert_eq!(ChatStatus::from_str("approved-ish"), None);
        assert_eq!(ChatStatus::from_str(""), None);
    }

    #[tokio::test]
    async fn record_creates_a_pending_row() {
        let repository = in_memory_repository().await;

        let outcome = repository
            .record(&new_chat())
            .await
            .expect("record should succeed");

        assert_eq!(outcome, InsertOutcome::Stored);
        let row = sqlx::query("SELECT kind, title, status, added_at, seen_at FROM chats")
            .fetch_one(&repository.pool)
            .await
            .expect("row should exist");
        assert_eq!(row.get::<String, _>("kind"), "group");
        assert_eq!(
            row.get::<Option<String>, _>("title"),
            Some("Car talk".to_owned())
        );
        assert_eq!(row.get::<String, _>("status"), ChatStatus::Pending.as_str());
        assert_eq!(row.get::<i64, _>("added_at"), ADDED_AT);
        assert_eq!(row.get::<i64, _>("seen_at"), ADDED_AT);
    }

    #[tokio::test]
    async fn record_keeps_the_decision_and_refreshes_the_observed_facts() {
        let repository = in_memory_repository().await;
        repository
            .record(&new_chat())
            .await
            .expect("first record should succeed");
        repository
            .set_status(42, ChatStatus::Approved)
            .await
            .expect("set_status should succeed");

        let outcome = repository
            .record(&NewChat {
                kind: "supergroup",
                title: Some("Car talk 2".to_owned()),
                added_at: ADDED_AT + 1,
                seen_at: ADDED_AT + 60,
                ..new_chat()
            })
            .await
            .expect("second record should succeed");

        assert_eq!(outcome, InsertOutcome::Duplicate);
        let row = sqlx::query("SELECT kind, title, status, added_at, seen_at FROM chats")
            .fetch_one(&repository.pool)
            .await
            .expect("row should exist");
        assert_eq!(row.get::<String, _>("kind"), "supergroup");
        assert_eq!(
            row.get::<Option<String>, _>("title"),
            Some("Car talk 2".to_owned())
        );
        assert_eq!(row.get::<String, _>("status"), "approved");
        assert_eq!(row.get::<i64, _>("added_at"), ADDED_AT);
        assert_eq!(row.get::<i64, _>("seen_at"), ADDED_AT + 60);
    }

    #[tokio::test]
    async fn record_keeps_a_blocked_chat_blocked() {
        let repository = in_memory_repository().await;
        repository
            .record(&new_chat())
            .await
            .expect("first record should succeed");
        repository
            .set_status(42, ChatStatus::Blocked)
            .await
            .expect("set_status should succeed");

        repository
            .record(&new_chat())
            .await
            .expect("second record should succeed");

        assert_eq!(status_of(&repository, 42).await, "blocked");
    }

    #[tokio::test]
    async fn approved_returns_only_approved_ids() {
        let repository = in_memory_repository().await;
        for chat_id in [1_i64, 2, 3] {
            repository
                .record(&NewChat {
                    chat_id,
                    ..new_chat()
                })
                .await
                .expect("record should succeed");
        }
        repository
            .set_status(1, ChatStatus::Approved)
            .await
            .expect("set_status should succeed");
        repository
            .set_status(3, ChatStatus::Blocked)
            .await
            .expect("set_status should succeed");

        let approved = repository
            .approved()
            .await
            .expect("approved should succeed");

        assert_eq!(approved, HashSet::from([1]));
    }

    #[tokio::test]
    async fn list_returns_pending_then_approved_and_omits_blocked() {
        let repository = in_memory_repository().await;
        for (chat_id, title) in [(1_i64, "Zulu"), (2, "Alpha"), (3, "Bravo"), (4, "Golf")] {
            repository
                .record(&NewChat {
                    chat_id,
                    title: Some(title.to_owned()),
                    ..new_chat()
                })
                .await
                .expect("record should succeed");
        }
        repository
            .set_status(3, ChatStatus::Approved)
            .await
            .expect("set_status should succeed");
        repository
            .set_status(4, ChatStatus::Blocked)
            .await
            .expect("set_status should succeed");

        let listed = repository.list().await.expect("list should succeed");

        let ids: Vec<i64> = listed.iter().map(|chat| chat.chat_id).collect();
        assert_eq!(ids, vec![3, 2, 1]);
        assert_eq!(listed[0].status, ChatStatus::Approved);
        assert_eq!(listed[0].title.as_deref(), Some("Bravo"));
        assert_eq!(listed[0].kind, "group");
        assert_eq!(listed[1].status, ChatStatus::Pending);
        assert_eq!(listed[1].title.as_deref(), Some("Alpha"));
        assert_eq!(listed[2].status, ChatStatus::Pending);
        assert_eq!(listed[2].title.as_deref(), Some("Zulu"));
    }

    #[tokio::test]
    async fn set_status_reports_whether_the_chat_exists() {
        let repository = in_memory_repository().await;
        repository
            .record(&new_chat())
            .await
            .expect("record should succeed");

        assert!(
            repository
                .set_status(42, ChatStatus::Approved)
                .await
                .expect("set_status should succeed")
        );
        assert_eq!(status_of(&repository, 42).await, "approved");

        assert!(
            repository
                .set_status(42, ChatStatus::Blocked)
                .await
                .expect("set_status should succeed")
        );
        assert_eq!(status_of(&repository, 42).await, "blocked");

        assert!(
            !repository
                .set_status(7, ChatStatus::Approved)
                .await
                .expect("set_status should succeed")
        );
    }
}
