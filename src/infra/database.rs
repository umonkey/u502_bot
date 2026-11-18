//! Asynchronous, domain-agnostic SQLite client.
//!
//! [`Database`] owns the pooled connection lifecycle and applies the startup schema. It knows no
//! tables and offers no statement helpers: callers own their SQL and run it against [`Database::pool`],
//! so repositories live in `src/domains/` and domain semantics stay outside this module. The startup
//! schema is embedded at build time in [`SCHEMA`]; nothing is read from disk at runtime. Under WAL a
//! reader sees the last committed snapshot while another connection writes, which the file-backed pool
//! sizes for; in exchange, a transaction must take the write lock with its first statement.

use std::path::Path;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::{Sqlite, pool::PoolOptions};

use crate::shared::result::{Error, Operation, Result};

/// Startup schema, baked into the binary at build time.
///
/// `dev/schema.sql` remains the authored source; the relative path resolves from this file, so a
/// schema edit requires a rebuild. [`Database::apply_schema`] applies it on every startup.
pub const SCHEMA: &str = include_str!("../../dev/schema.sql");

/// A pooled SQLite connection shared with the repositories that own their SQL.
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    /// Opens the database file at `path`, creating it and its parent directory when missing.
    ///
    /// The connection enables WAL journaling, a five-second busy timeout, and foreign keys, and the pool
    /// holds up to [`MAX_CONNECTIONS`] of them, so a reader proceeds while another connection writes.
    /// SQLite admits one writer, and a deferred `BEGIN` takes the write lock on the first write, so a
    /// transaction must write first: a read followed by a write can instead fail with
    /// `SQLITE_BUSY_SNAPSHOT`, which the busy timeout does not retry.
    ///
    /// WAL is applied on the first connection, before any service exists, so no two connections race to
    /// switch a new file out of rollback mode.
    pub async fn connect(path: &Path) -> Result<Self> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|source| Error::CreateDatabaseDirectory {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let options = base_options()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal);

        Self::open(pool_options(MAX_CONNECTIONS), options).await
    }

    #[cfg(test)]
    pub(crate) async fn connect_in_memory() -> Result<Self> {
        // Each connection to `:memory:` is its own database, so a test keeps a single one.
        Self::open(pool_options(1), base_options().in_memory(true)).await
    }

    /// Applies the embedded startup schema.
    pub async fn apply_schema(&self) -> Result<()> {
        sqlx::raw_sql(SCHEMA).execute(&self.pool).await?;

        Ok(())
    }

    /// Hands the pool to a repository.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Closes every pooled connection, checkpointing the write-ahead log.
    ///
    /// Callers invoke it after the services stopped, so no repository is still holding a connection.
    pub async fn close(self) {
        self.pool.close().await;
    }

    async fn open(pool: PoolOptions<Sqlite>, options: SqliteConnectOptions) -> Result<Self> {
        let pool = pool.connect_with(options).await?;

        Ok(Self { pool })
    }
}

impl From<sqlx::Error> for Error {
    fn from(source: sqlx::Error) -> Self {
        Self::External {
            operation: Operation::Database,
            source: Box::new(source),
        }
    }
}

/// Connection settings shared by file-backed and in-memory databases.
fn base_options() -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true)
}

/// File-backed connections the pool may open.
///
/// WAL lets a reader see the last committed snapshot while another connection writes, so the pool no
/// longer has to make every read wait for the ingestor's commit. SQLite still admits one writer, so the
/// cap buys read concurrency, not write concurrency.
const MAX_CONNECTIONS: u32 = 4;

fn pool_options(max_connections: u32) -> PoolOptions<Sqlite> {
    SqlitePoolOptions::new().max_connections(max_connections)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn schema_is_idempotent() {
        let database = Database::connect_in_memory()
            .await
            .expect("in-memory database should open");

        database.apply_schema().await.expect("schema should apply");
        database
            .apply_schema()
            .await
            .expect("schema should apply twice");

        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'messages'",
        )
        .fetch_all(database.pool())
        .await
        .expect("table lookup should succeed");
        assert_eq!(tables, vec!["messages".to_owned()]);
    }

    #[tokio::test]
    async fn schema_contains_the_messages_table() {
        assert!(SCHEMA.contains("CREATE TABLE IF NOT EXISTS messages"));
    }

    #[tokio::test]
    async fn schema_contains_the_attachments_table() {
        assert!(SCHEMA.contains("CREATE TABLE IF NOT EXISTS attachments"));
    }

    #[tokio::test]
    async fn connect_creates_the_database_file_and_parent_directory() {
        let directory =
            std::env::temp_dir().join(format!("u502-database-test-{}", std::process::id()));
        let path = directory.join("nested").join("database.sqlite");

        let database = Database::connect(&path)
            .await
            .expect("database should open");
        database.apply_schema().await.expect("schema should apply");

        let id: i64 = sqlx::query_scalar(
            "INSERT INTO messages (chat_id, message_id, received_at) VALUES (?, ?, ?) RETURNING id",
        )
        .bind(1_i64)
        .bind(2_i64)
        .bind(3_i64)
        .fetch_one(database.pool())
        .await
        .expect("insert should succeed");
        assert_eq!(id, 1);
        assert!(path.exists());

        drop(database);
        std::fs::remove_dir_all(&directory).expect("test directory should be removable");
    }

    async fn connect_temporary() -> (tempfile::TempDir, Database) {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let database = Database::connect(&directory.path().join("database.sqlite"))
            .await
            .expect("database should open");
        database.apply_schema().await.expect("schema should apply");

        (directory, database)
    }

    #[tokio::test]
    async fn file_database_uses_wal() {
        let (_directory, database) = connect_temporary().await;

        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(database.pool())
            .await
            .expect("journal mode lookup should succeed");
        assert_eq!(journal_mode, "wal");

        database.close().await;
    }

    #[tokio::test]
    async fn a_read_completes_while_a_write_transaction_is_open() {
        let (_directory, database) = connect_temporary().await;

        let mut transaction = database
            .pool()
            .begin()
            .await
            .expect("transaction should begin");
        sqlx::query("INSERT INTO messages (chat_id, message_id, received_at) VALUES (?, ?, ?)")
            .bind(1_i64)
            .bind(2_i64)
            .bind(3_i64)
            .execute(&mut *transaction)
            .await
            .expect("insert should succeed");

        let count: i64 = tokio::time::timeout(
            Duration::from_secs(2),
            sqlx::query_scalar("SELECT COUNT(*) FROM messages").fetch_one(database.pool()),
        )
        .await
        .expect("read should not wait for the open write transaction")
        .expect("read should succeed");

        assert_eq!(count, 0);

        transaction
            .rollback()
            .await
            .expect("rollback should succeed");
        database.close().await;
    }

    #[tokio::test]
    async fn concurrent_writers_and_readers_do_not_fail() {
        let (_directory, database) = connect_temporary().await;

        let mut tasks = Vec::new();
        for writer in 0..4_i64 {
            let pool = database.pool().clone();
            tasks.push(tokio::spawn(async move {
                for row in 0..10_i64 {
                    sqlx::query(
                        "INSERT INTO messages (chat_id, message_id, received_at) VALUES (?, ?, ?)",
                    )
                    .bind(writer)
                    .bind(row)
                    .bind(row)
                    .execute(&pool)
                    .await?;
                }

                Ok::<(), sqlx::Error>(())
            }));
        }
        for _ in 0..2 {
            let pool = database.pool().clone();
            tasks.push(tokio::spawn(async move {
                for _ in 0..10 {
                    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM messages")
                        .fetch_one(&pool)
                        .await?;
                }

                Ok::<(), sqlx::Error>(())
            }));
        }

        for task in tasks {
            task.await
                .expect("task should not panic")
                .expect("statement should not fail");
        }

        database.close().await;
    }
}
