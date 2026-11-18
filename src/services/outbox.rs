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
use crate::services::unix_now;
use crate::services::wake::Wake;

/// Rows read per scan.
const BATCH: i64 = 10;

/// Pause between two sends, so a burst does not hit Telegram's rate limit.
const SEND_DELAY: Duration = Duration::from_secs(2);

/// Pause when the queue is empty, or when nothing in the batch could be delivered.
const IDLE: Duration = Duration::from_secs(5);

/// Delivers the messages queued in the outbox.
pub struct Outbox {
    telegram: Telegram,
    repository: OutboxRepository,
    pending_replies: Wake,
}

impl Outbox {
    /// Creates the service over the client and the repository the command composed.
    pub fn new(telegram: Telegram, repository: OutboxRepository, pending_replies: Wake) -> Self {
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
                Ok(()) => delivered |= self.mark_sent(&message).await,
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
