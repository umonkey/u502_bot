//! Models and the repositories that own their SQL.
//!
//! A repository is the only place its table's statements live; the module root holds the vocabulary
//! two repositories share and nothing that belongs to one of them.

pub mod attachment;
pub mod chat;
pub mod message;
pub mod outbox;

/// What an idempotent insert did with a row.
///
/// Every repository inserts with `ON CONFLICT (<key>) DO NOTHING`, so the only silent case is a
/// redelivery, and one type says so for the two `(chat_id, message_id)` tables and for the chat
/// registry's `chat_id` insert alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOutcome {
    /// The row was written.
    Stored,
    /// The row was already there, so the redelivery was ignored.
    Duplicate,
}
