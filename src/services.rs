//! Long-running tasks the `start` command composes.
//!
//! A service is a loop over time rather than a single command invocation: `ingestor::Ingestor`
//! reads Telegram, `transcriber::Transcriber` turns pending attachments into outbox rows, and
//! `outbox::Outbox` delivers queued replies. Each one holds handles to clients and repositories but
//! owns neither: the clients live in `src/infra/`, the SQL lives in `src/domains/`, and the
//! composition lives in `src/cli/`.
//!
//! The module also carries [`shutdown::Shutdown`], the stop signal the services share, so a command
//! can ask every loop to end without knowing how any of them is written.

use std::time::{SystemTime, UNIX_EPOCH};

pub mod ingestor;
pub mod outbox;
pub mod shutdown;
pub mod transcriber;

/// Current Unix time in seconds, UTC, which is how the schema stores every timestamp.
pub(crate) fn unix_now() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}
