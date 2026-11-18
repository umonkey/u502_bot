//! Outbox service: delivers queued replies to Telegram.
//!
//! The transcriber queues a transcript instead of sending it, so a restart cannot lose the work and
//! a slow or failing Telegram call cannot stall transcription. This service reads the unsent rows,
//! sends them in id order, and marks each one delivered. Delivery is at-least-once: a crash between
//! the send and the mark re-sends one message.

use std::time::Duration;

use crate::domains::outbox::{OutboxRepository, PendingMessage};
use crate::infra::sentry;
use crate::infra::telegram_bot::Telegram;
use crate::services::shutdown::Shutdown;
use crate::services::wake::WakeReceiver;
use crate::services::{BATCH, IDLE, unix_now, unix_now_f64};

/// Pause between two sends, so a burst does not hit Telegram's rate limit.
const SEND_DELAY: Duration = Duration::from_secs(2);

/// Delivers the messages queued in the outbox.
pub struct Outbox {
    telegram: Telegram,
    repository: OutboxRepository,
    pending_replies: WakeReceiver,
}

impl Outbox {
    /// Creates the service over the client and the repository the command composed.
    pub fn new(
        telegram: Telegram,
        repository: OutboxRepository,
        pending_replies: WakeReceiver,
    ) -> Self {
        Self {
            telegram,
            repository,
            pending_replies,
        }
    }

    /// Delivers queued messages until shutdown is requested, pausing only when there is nothing to
    /// do.
    ///
    /// The pause ends early when the transcriber queues a reply, so a fresh transcript does not
    /// wait out [`IDLE`]. The [`SEND_DELAY`] between two sends is untouched: a pulse there stays
    /// pending and the next scan reads the rows anyway.
    pub async fn run(mut self, mut shutdown: Shutdown) {
        while !shutdown.is_requested() {
            if !self.tick(&mut shutdown).await
                && !self.pending_replies.sleep(IDLE, &mut shutdown).await
            {
                break;
            }
        }

        tracing::info!("Stopped the outbox");
    }

    /// Delivers one batch and returns whether any row was delivered.
    ///
    /// The pause lives in the caller, where it is interruptible: a failed read returns `false`, so
    /// the pool shared with the other services is retried once per [`IDLE`] instead of spinning. A
    /// rejected send is logged and left unsent, so one bad message does not block the queue; the next
    /// scan retries it. A shutdown interrupts a send by dropping it, which may duplicate that one
    /// reply per the at-least-once delivery the outbox already promises.
    async fn tick(&self, shutdown: &mut Shutdown) -> bool {
        let pending = match self.repository.pending(BATCH).await {
            Ok(pending) => pending,
            Err(error) => {
                tracing::error!(
                    error = &error as &dyn std::error::Error,
                    "Failed to read the outbox"
                );

                return false;
            }
        };

        let mut delivered = false;

        for message in pending {
            if shutdown.is_requested() {
                break;
            }

            match tokio::select! {
                result = self
                    .telegram
                    .send(message.chat_id, &message.message, message.thread_id) => result,
                () = shutdown.wait() => break,
            } {
                Ok(()) => {
                    if let Some(seconds) = reply_latency(message.source_at, unix_now_f64()) {
                        sentry::transcription_reply_latency(seconds);
                    }

                    delivered |= self.mark_sent(&message).await;
                }
                Err(error) => tracing::error!(
                    outbox_id = message.id,
                    chat_id = message.chat_id,
                    error = &error as &dyn std::error::Error,
                    "Failed to send an outbox message"
                ),
            }

            if !shutdown.sleep(SEND_DELAY).await {
                break;
            }
        }

        delivered
    }

    /// Marks one delivered row sent and counts it; returns whether the row was marked.
    async fn mark_sent(&self, message: &PendingMessage) -> bool {
        match self.repository.mark_sent(message.id, unix_now()).await {
            Ok(()) => {
                sentry::outbox_sent();

                true
            }
            Err(error) => {
                tracing::error!(
                    outbox_id = message.id,
                    error = &error as &dyn std::error::Error,
                    "Failed to mark an outbox message sent"
                );

                false
            }
        }
    }
}

/// Seconds from a source message's Telegram send time to `now`, when the row answers a message.
///
/// The raw difference is reported rather than clamped at zero: a persistent negative sample means
/// the host clock is ahead of Telegram's, which is worth seeing.
fn reply_latency(source_at: Option<i64>, now: f64) -> Option<f64> {
    source_at.map(|source_at| now - source_at as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_latency_measures_from_the_source_send_time() {
        assert_eq!(
            reply_latency(Some(1_700_000_000), 1_700_000_012.5),
            Some(12.5)
        );
    }

    #[test]
    fn reply_latency_is_absent_without_a_source_message() {
        assert_eq!(reply_latency(None, 1_700_000_012.5), None);
    }
}
