//! Asynchronous, domain-agnostic SQLite client.
//!
//! [`Database`] owns the pooled connection lifecycle and applies the startup schema. It knows no
//! tables and offers no statement helpers: callers own their SQL and run it against [`Database::pool`],
//! so repositories live in `src/domains/` and domain semantics stay outside this module. The startup
//! schema is embedded at build time in [`SCHEMA`]; nothing is read from disk at runtime.

use std::env::VarError;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::{Sqlite, pool::PoolOptions};

/// Environment variable holding the path of the SQLite database file.
pub const DATABASE_PATH: &str = "DATABASE_PATH";

/// Startup schema, baked into the binary at build time.
///
/// `dev/schema.sql` remains the authored source; the relative path resolves from this file, so a
/// schema edit requires a rebuild. [`Database::apply_schema`] applies it on every startup.
pub const SCHEMA: &str = include_str!("../../dev/schema.sql");

/// Failure modes of opening a database or applying its schema.
#[derive(Debug)]
pub enum DatabaseError {
    /// The database path is not set or not valid Unicode.
    MissingPath(String),
    /// The directory that should hold the database file could not be created.
    CreateDir { path: PathBuf, source: io::Error },
    /// SQLite rejected a connection setting or a statement.
    Sqlite(sqlx::Error),
}

impl fmt::Display for DatabaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPath(name) => {
                write!(formatter, "{name} is not set or is not valid Unicode")
            }
            Self::CreateDir { path, source } => {
                write!(
                    formatter,
                    "failed to create database directory {}: {source}",
                    path.display()
                )
            }
            Self::Sqlite(source) => write!(formatter, "database error: {source}"),
        }
    }
}

impl std::error::Error for DatabaseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CreateDir { source, .. } => Some(source),
            Self::Sqlite(source) => Some(source),
            Self::MissingPath(_) => None,
        }
    }
}

impl From<sqlx::Error> for DatabaseError {
    fn from(source: sqlx::Error) -> Self {
        Self::Sqlite(source)
    }
}

/// Resolves the database file path from the environment.
///
/// Returns [`DatabaseError::MissingPath`] when [`DATABASE_PATH`] is unset or not valid Unicode.
pub fn database_path() -> Result<PathBuf, DatabaseError> {
    resolve_database_path(|key| std::env::var(key))
}

fn resolve_database_path(
    lookup: impl Fn(&str) -> Result<String, VarError>,
) -> Result<PathBuf, DatabaseError> {
    match lookup(DATABASE_PATH) {
        Ok(path) => Ok(PathBuf::from(path)),
        Err(VarError::NotPresent | VarError::NotUnicode(_)) => {
            Err(DatabaseError::MissingPath(DATABASE_PATH.to_owned()))
        }
    }
}

/// A pooled SQLite connection shared with the repositories that own their SQL.
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    /// Opens the database file at `path`, creating it and its parent directory when missing.
    ///
    /// The connection enables WAL journaling, a five-second busy timeout, and foreign keys. The pool
    /// holds a single connection, which matches SQLite's single-writer reality and keeps in-memory
    /// databases working; the cap can grow when a reader appears.
    pub async fn connect(path: &Path) -> Result<Self, DatabaseError> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|source| DatabaseError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let options = base_options()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal);

        Self::open(pool_options(), options).await
    }

    #[cfg(test)]
    pub(crate) async fn connect_in_memory() -> Result<Self, DatabaseError> {
        Self::open(pool_options(), base_options().in_memory(true)).await
    }

    /// Applies the embedded startup schema.
    pub async fn apply_schema(&self) -> Result<(), DatabaseError> {
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

    async fn open(
        pool: PoolOptions<Sqlite>,
        options: SqliteConnectOptions,
    ) -> Result<Self, DatabaseError> {
        let pool = pool.connect_with(options).await?;

        Ok(Self { pool })
    }
}

/// Connection settings shared by file-backed and in-memory databases.
fn base_options() -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true)
}

fn pool_options() -> PoolOptions<Sqlite> {
    SqlitePoolOptions::new().max_connections(1)
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
    async fn missing_database_path_is_an_error() {
        let error = resolve_database_path(|_| Err(VarError::NotPresent))
            .expect_err("missing database path should not resolve");
        assert!(matches!(error, DatabaseError::MissingPath(name) if name == DATABASE_PATH));
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
}
