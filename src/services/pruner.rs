//! Retention service: deletes rows no service can act on again.
//!
//! One pass deletes delivered outbox rows, then attachments whose message is missing or older than
//! the retention, then messages older than the retention. The order keeps every join valid: the
//! attachment age predicate reads `messages`, so attachments are aged while their message still
//! exists. The `transcribed` flag is not consulted, because past the transcriber's one-hour window an
//! unprocessed row can never be attempted, and a legacy row without a message can never be joined
//! again. Each table is an independent bounded statement, so a failure in one does not roll back the
//! others. Deletion bounds growth but does not shrink the file: SQLite keeps the freed pages, and
//! reclaiming them is an operator's `VACUUM`.

use std::time::Duration;

use crate::domains::attachment::AttachmentRepository;
use crate::domains::message::MessageRepository;
use crate::domains::outbox::OutboxRepository;
use crate::infra::sentry;
use crate::services::shutdown::Shutdown;
use crate::services::{PRUNE_BATCH, PRUNE_PASS_BATCHES, unix_now};

/// Pause between two passes: once an hour is far more often than rows age out.
const INTERVAL: Duration = Duration::from_secs(3_600);

/// Seconds in one day, the unit `RETENTION_DAYS` is resolved in.
const SECONDS_PER_DAY: i64 = 86_400;

/// Deletes rows older than the retention, once an hour, in bounded batches.
pub struct Pruner {
    outbox: OutboxRepository,
    attachments: AttachmentRepository,
    messages: MessageRepository,
    retention_days: Option<u32>,
}

impl Pruner {
    /// Creates the service over the repositories and the retention the command resolved.
    pub fn new(
        outbox: OutboxRepository,
        attachments: AttachmentRepository,
        messages: MessageRepository,
        retention_days: Option<u32>,
    ) -> Self {
        Self {
            outbox,
            attachments,
            messages,
            retention_days,
        }
    }

    /// Prunes once immediately, then once per [`INTERVAL`] until shutdown is requested.
    ///
    /// `None` means retention is disabled: the service waits for shutdown instead of looping, so the
    /// composition always spawns the same four tasks.
    pub async fn run(self, mut shutdown: Shutdown) {
        if self.retention_days.is_some() {
            while !shutdown.is_requested() {
                self.tick(&mut shutdown).await;

                if !shutdown.sleep(INTERVAL).await {
                    break;
                }
            }
        } else {
            shutdown.wait().await;
        }

        tracing::info!("Stopped the pruner");
    }

    /// Deletes one bounded pass per table, oldest rows first, in the order that keeps every join valid.
    ///
    /// Reads the retention from `self`, so a disabled pruner is a no-op here and the loop above only
    /// has to decide whether to stay alive.
    async fn tick(&self, shutdown: &mut Shutdown) {
        let Some(retention_days) = self.retention_days else {
            return;
        };

        let cutoff = unix_now() - i64::from(retention_days) * SECONDS_PER_DAY;
        // outbox, then attachments, then messages: the attachment age predicate joins messages.
        self.prune_outbox(cutoff, shutdown).await;
        self.prune_attachments(cutoff, shutdown).await;
        self.prune_messages(cutoff, shutdown).await;
    }

    /// Deletes delivered outbox rows in bounded statements until there is none left in reach.
    async fn prune_outbox(&self, cutoff: i64, shutdown: &mut Shutdown) {
        for _ in 0..PRUNE_PASS_BATCHES {
            if shutdown.is_requested() {
                break;
            }

            match self.outbox.prune_sent_before(cutoff, PRUNE_BATCH).await {
                Ok(0) => break,
                Ok(rows) => {
                    sentry::outbox_pruned(rows);

                    if rows < PRUNE_BATCH.unsigned_abs() {
                        break;
                    }
                }
                Err(error) => {
                    tracing::error!(
                        error = &error as &dyn std::error::Error,
                        "Failed to prune the outbox"
                    );

                    break;
                }
            }
        }
    }

    /// Deletes attachments of old or missing messages in bounded statements.
    async fn prune_attachments(&self, cutoff: i64, shutdown: &mut Shutdown) {
        for _ in 0..PRUNE_PASS_BATCHES {
            if shutdown.is_requested() {
                break;
            }

            match self
                .attachments
                .prune_for_messages_before(cutoff, PRUNE_BATCH)
                .await
            {
                Ok(0) => break,
                Ok(rows) => {
                    sentry::attachments_pruned(rows);

                    if rows < PRUNE_BATCH.unsigned_abs() {
                        break;
                    }
                }
                Err(error) => {
                    tracing::error!(
                        error = &error as &dyn std::error::Error,
                        "Failed to prune attachments"
                    );

                    break;
                }
            }
        }
    }

    /// Deletes old messages in bounded statements, after their attachments are gone.
    async fn prune_messages(&self, cutoff: i64, shutdown: &mut Shutdown) {
        for _ in 0..PRUNE_PASS_BATCHES {
            if shutdown.is_requested() {
                break;
            }

            match self
                .messages
                .prune_received_before(cutoff, PRUNE_BATCH)
                .await
            {
                Ok(0) => break,
                Ok(rows) => {
                    sentry::messages_pruned(rows);

                    if rows < PRUNE_BATCH.unsigned_abs() {
                        break;
                    }
                }
                Err(error) => {
                    tracing::error!(
                        error = &error as &dyn std::error::Error,
                        "Failed to prune messages"
                    );

                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use sqlx::SqlitePool;

    use crate::infra::database::Database;
    use crate::services::shutdown::{self, Shutdown};

    use super::*;

    const RETENTION_DAYS: u32 = 30;

    async fn in_memory_pruner(retention_days: Option<u32>) -> (SqlitePool, Pruner) {
        let database = Database::connect_in_memory()
            .await
            .expect("in-memory database should open");
        database.apply_schema().await.expect("schema should apply");
        let pool = database.pool().clone();
        let pruner = Pruner::new(
            OutboxRepository::new(pool.clone()),
            AttachmentRepository::new(pool.clone()),
            MessageRepository::new(pool.clone()),
            retention_days,
        );

        (pool, pruner)
    }

    async fn insert_message(pool: &SqlitePool, message_id: i64, received_at: i64) {
        sqlx::query("INSERT INTO messages (chat_id, message_id, received_at) VALUES (?, ?, ?)")
            .bind(42_i64)
            .bind(message_id)
            .bind(received_at)
            .execute(pool)
            .await
            .expect("message should store");
    }

    async fn insert_attachment(pool: &SqlitePool, message_id: i64) {
        sqlx::query(
            "INSERT INTO attachments (chat_id, message_id, kind, file_id) VALUES (?, ?, 'audio', ?)",
        )
        .bind(42_i64)
        .bind(message_id)
        .bind(format!("file-{message_id}"))
        .execute(pool)
        .await
        .expect("attachment should store");
    }

    async fn insert_outbox(pool: &SqlitePool, sent_at: Option<i64>) {
        sqlx::query(
            "INSERT INTO outbox (chat_id, added_at, sent_at, message) VALUES (?, ?, ?, 'reply')",
        )
        .bind(42_i64)
        .bind(0_i64)
        .bind(sent_at)
        .execute(pool)
        .await
        .expect("outbox row should store");
    }

    async fn count(pool: &SqlitePool, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(pool)
            .await
            .expect("count should succeed")
    }

    /// Seeds one old and one recent message with an attachment each, plus three outbox rows: one old
    /// and delivered, one recent and delivered, and one unsent.
    async fn seed_aged_rows(pool: &SqlitePool) {
        let now = unix_now();
        let old = now - (i64::from(RETENTION_DAYS) + 1) * SECONDS_PER_DAY;
        let recent = now - SECONDS_PER_DAY;

        insert_message(pool, 7, old).await;
        insert_message(pool, 8, recent).await;
        insert_attachment(pool, 7).await;
        insert_attachment(pool, 8).await;
        insert_outbox(pool, Some(old)).await;
        insert_outbox(pool, Some(recent)).await;
        insert_outbox(pool, None).await;
    }

    async fn tick(pruner: &Pruner) {
        let (_trigger, mut shutdown): (_, Shutdown) = shutdown::channel();

        pruner.tick(&mut shutdown).await;
    }

    #[tokio::test]
    async fn a_pass_prunes_delivered_attachments_and_messages_older_than_the_retention() {
        let (pool, pruner) = in_memory_pruner(Some(RETENTION_DAYS)).await;
        seed_aged_rows(&pool).await;

        tick(&pruner).await;

        assert_eq!(count(&pool, "messages").await, 1);
        assert_eq!(count(&pool, "attachments").await, 1);
        assert_eq!(count(&pool, "outbox").await, 2);

        let message_id: i64 = sqlx::query_scalar("SELECT message_id FROM messages")
            .fetch_one(&pool)
            .await
            .expect("recent message should survive");
        assert_eq!(message_id, 8);

        let attachment_message_id: i64 = sqlx::query_scalar("SELECT message_id FROM attachments")
            .fetch_one(&pool)
            .await
            .expect("recent attachment should survive");
        assert_eq!(attachment_message_id, 8);

        let unsent: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbox WHERE sent_at IS NULL")
            .fetch_one(&pool)
            .await
            .expect("count should succeed");
        assert_eq!(unsent, 1);
    }

    #[tokio::test]
    async fn a_second_pass_deletes_nothing() {
        let (pool, pruner) = in_memory_pruner(Some(RETENTION_DAYS)).await;
        seed_aged_rows(&pool).await;

        tick(&pruner).await;
        let counts = (
            count(&pool, "messages").await,
            count(&pool, "attachments").await,
            count(&pool, "outbox").await,
        );

        tick(&pruner).await;

        assert_eq!(
            counts,
            (
                count(&pool, "messages").await,
                count(&pool, "attachments").await,
                count(&pool, "outbox").await,
            )
        );
    }

    #[tokio::test]
    async fn a_disabled_pruner_deletes_nothing() {
        let (pool, pruner) = in_memory_pruner(None).await;
        seed_aged_rows(&pool).await;

        tick(&pruner).await;

        assert_eq!(count(&pool, "messages").await, 2);
        assert_eq!(count(&pool, "attachments").await, 2);
        assert_eq!(count(&pool, "outbox").await, 3);
    }
}
