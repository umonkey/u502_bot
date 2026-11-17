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
use crate::services::unix_now;

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
}

impl Outbox {
    /// Creates the service over the client and the repository the command composed.
    pub fn new(telegram: Telegram, repository: OutboxRepository) -> Self {
        Self {
            telegram,
            repository,
        }
    }

    /// Delivers queued messages forever, pausing only when there is nothing to do.
    pub async fn run(self) {
        loop {
            if !self.tick().await {
                tokio::time::sleep(IDLE).await;
            }
        }
    }

    /// Delivers one batch and returns whether any row was delivered.
    ///
    /// A failed read sleeps here rather than returning: the pool is shared with the other services,
    /// and a failed read must not spin. A rejected send is logged and left unsent, so one bad message
    /// does not block the queue; the next scan retries it.
    async fn tick(&self) -> bool {
        let pending = match self.repository.pending(BATCH).await {
            Ok(pending) => pending,
            Err(error) => {
                tracing::error!(
                    error = &error as &dyn std::error::Error,
                    "Failed to read the outbox"
                );
                tokio::time::sleep(IDLE).await;

                return false;
            }
        };

        let mut delivered = false;

        for message in pending {
            match self
                .telegram
                .send(message.chat_id, &message.message, message.thread_id)
                .await
            {
                Ok(()) => delivered |= self.mark_sent(&message).await,
                Err(error) => tracing::error!(
                    outbox_id = message.id,
                    chat_id = message.chat_id,
                    error = &error as &dyn std::error::Error,
                    "Failed to send an outbox message"
                ),
            }

            tokio::time::sleep(SEND_DELAY).await;
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
