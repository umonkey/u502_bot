//! Long-running tasks the `start` command composes.
//!
//! A service is a loop over time rather than a single command invocation: `ingestor::Ingestor`
//! reads Telegram, `transcriber::Transcriber` turns pending attachments into outbox rows,
//! `outbox::Outbox` delivers queued replies, and `pruner::Pruner` deletes rows older than the
//! retention. Each one holds handles to clients and repositories but owns neither: the clients live
//! in `src/infra/`, the SQL lives in `src/domains/`, and the composition lives in `src/cli/`.
//! `admin::Admin` is not a loop: it is the owner's command interface, which the ingestor calls.
//!
//! The module also carries two one-way edges. [`shutdown`] carries the [`shutdown::ShutdownTrigger`]
//! the command holds and the [`shutdown::Shutdown`] side each loop clones, so a command can ask
//! every loop to end without knowing how any of them is written. [`wake`] carries the
//! [`wake::WakeSender`] a producer pulses and the [`wake::WakeReceiver`] its peer waits on, so an
//! idle pause ends at once.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub mod admin;
pub mod ingestor;
pub mod outbox;
pub mod pruner;
pub mod shutdown;
pub mod transcriber;
pub mod wake;

/// Work items one scan reads, whether pending attachments or outbox rows.
pub(crate) const BATCH: i64 = 10;

/// Rows one pruning statement deletes, which bounds how long it holds SQLite's write lock.
pub(crate) const PRUNE_BATCH: i64 = 500;

/// Pruning statements one table may run per pass, which bounds one hourly wake.
pub(crate) const PRUNE_PASS_BATCHES: usize = 20;

/// Pause after a scan that found no work, or could not do any, before retrying.
pub(crate) const IDLE: Duration = Duration::from_secs(5);

/// Current Unix time in seconds, UTC, which is how the schema stores every timestamp.
pub(crate) fn unix_now() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

/// Current Unix time in seconds, UTC, with the sub-second part kept.
///
/// Used to time a measurement whose other end, Telegram's own timestamp, is whole seconds. Falls
/// back to `0.0` like [`unix_now`] when the clock is before the epoch.
pub(crate) fn unix_now_f64() -> f64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_secs_f64(),
        Err(_) => 0.0,
    }
}
