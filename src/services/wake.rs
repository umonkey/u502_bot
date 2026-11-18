//! One-way pulse between services, so an idle pause ends as soon as there is work.
//!
//! A service that produces work for a peer pulses its [`WakeSender`]; the peer waits on the
//! [`WakeReceiver`] instead of sitting out the whole pause. The two ends are distinct types, so a
//! consumer cannot pulse and a producer cannot wait: the direction is a compile-time property of the
//! wiring. A pulse that arrives before the pause begins is remembered until it is observed and is
//! not lost, and a pulse is consumed once, so an observed pulse cannot turn a pause into a spin.
//! Pulses collapse: two faster than the peer observes them are one wake, which is what the loops
//! want because a productive batch already loops without pausing.

use std::time::Duration;

use tokio::sync::watch;

use crate::services::shutdown::Shutdown;

/// The producing half of a wake edge.
///
/// A pulse with no receiver left is dropped. A pulse is a hint, so losing one costs a scan at most,
/// and the peer it would have woken is gone anyway.
pub struct WakeSender {
    sender: watch::Sender<u64>,
}

/// The consuming half of a wake edge.
///
/// A pulse is sticky until observed: a pulse sent before [`WakeReceiver::sleep`] is called ends the
/// next pause at once.
pub struct WakeReceiver {
    receiver: watch::Receiver<u64>,
}

/// Creates the two ends of one wake edge, with no pending pulse.
pub fn channel() -> (WakeSender, WakeReceiver) {
    let (sender, receiver) = watch::channel(0u64);

    (WakeSender { sender }, WakeReceiver { receiver })
}

impl WakeSender {
    /// Pulses the peer, ending its next pause at once.
    pub fn notify(&self) {
        self.sender
            .send_modify(|pulses| *pulses = pulses.wrapping_add(1));
    }
}

impl WakeReceiver {
    /// Pauses for `duration`, ending early on a pulse; `false` when shutdown was requested.
    pub async fn sleep(&mut self, duration: Duration, shutdown: &mut Shutdown) -> bool {
        tokio::select! {
            () = shutdown.wait() => return false,
            // The error only exists when no sender is left, so a closed channel is a pulse.
            _ = self.receiver.changed() => {}
            _ = tokio::time::sleep(duration) => {}
        }

        true
    }
}

// The API absence the types enforce — a consumer has no `notify` and a producer has no `sleep` — is
// not asserted from here, because `trybuild` would be a dev-dependency for three checks the
// signatures already make. The tests below cover each direction behaviourally instead.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::shutdown;

    #[tokio::test]
    async fn a_pulse_sent_before_the_pause_ends_it_at_once() {
        let (sender, mut receiver) = channel();
        let (_trigger, mut shutdown) = shutdown::channel();

        sender.notify();

        let slept = tokio::time::timeout(
            Duration::from_millis(100),
            receiver.sleep(Duration::from_secs(60), &mut shutdown),
        )
        .await
        .expect("the pause should end on the pending pulse");

        assert!(slept);
    }

    #[tokio::test]
    async fn a_pulse_sent_during_the_pause_ends_it_early() {
        let (sender, mut receiver) = channel();
        let (_trigger, mut shutdown) = shutdown::channel();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            sender.notify();
        });

        let slept = tokio::time::timeout(
            Duration::from_millis(1_000),
            receiver.sleep(Duration::from_secs(60), &mut shutdown),
        )
        .await
        .expect("the pause should end on the arriving pulse");

        assert!(slept);
    }

    #[tokio::test]
    async fn a_pulse_is_consumed_once() {
        let (sender, mut receiver) = channel();
        let (_trigger, mut shutdown) = shutdown::channel();

        sender.notify();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(100),
                receiver.sleep(Duration::from_secs(1), &mut shutdown)
            )
            .await
            .expect("the first pause should end on the pulse")
        );

        let second = tokio::time::timeout(
            Duration::from_millis(50),
            receiver.sleep(Duration::from_secs(1), &mut shutdown),
        )
        .await;

        assert!(
            second.is_err(),
            "the second pause should block instead of spinning"
        );
    }

    #[tokio::test]
    async fn a_shutdown_request_ends_the_pause() {
        let (_sender, mut receiver) = channel();
        let (trigger, mut shutdown) = shutdown::channel();

        trigger.request();

        assert!(!receiver.sleep(Duration::from_secs(60), &mut shutdown).await);
    }

    #[tokio::test]
    async fn an_unpulsed_pause_falls_back_to_its_duration() {
        let (_sender, mut receiver) = channel();
        let (_trigger, mut shutdown) = shutdown::channel();

        let slept = tokio::time::timeout(
            Duration::from_secs(1),
            receiver.sleep(Duration::from_millis(1), &mut shutdown),
        )
        .await
        .expect("the pause should end on its own duration");

        assert!(slept);
    }

    #[tokio::test]
    async fn a_pulse_sent_after_the_receiver_dropped_is_dropped() {
        let (sender, receiver) = channel();
        drop(receiver);

        sender.notify();
    }
}
