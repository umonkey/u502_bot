//! Cross-cutting vocabulary and the crate-wide error type every layer may use; never a client, SQL,
//! or a loop, and nothing here names a layer above it.

pub mod config;
pub mod metrics;
pub mod result;
pub mod text;
