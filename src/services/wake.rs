//! One-way pulse between services, so an idle pause ends as soon as there is work.
//!
//! A service that produces work for a peer pulses its [`Wake`]; the peer waits on it instead of
//! sitting out the whole pause. [`Wake`] is a cloneable handle over one `watch` counter, so a pulse
//! that arrives before the pause begins is remembered until it is observed and is not lost, and a
//! pulse is consumed once, so an observed pulse cannot turn a pause into a spin. Pulses collapse: two
//! faster than the peer observes them are one wake, which is what the loops want because a
//! productive batch already loops without pausing. The channel is one-way by construction — a handle
//! carries both ends, and the direction is fixed by the field names in the services that share it.

use std::time::Duration;

use tokio::sync::watch;

use crate::services::shutdown::Shutdown;

/// A one-way wake signal shared by a producing service and its consumer.
///
/// Cloning is cheap and every clone observes the same pulses. A pulse is sticky until observed: a
/// pulse sent before [`Wake::sleep`] is called ends the next pause at once.
#[derive(Clone)]
pub struct Wake {
    sender: watch::Sender<u64>,
    receiver: watch::Receiver<u64>,
}

impl Wake {
    /// Creates a handle with no pending pulse.
    pub fn new() -> Self {
        let (sender, receiver) = watch::channel(0u64);

        Self { sender, receiver }
    }

    /// Pulses the peer, ending its next pause at once.
    pub fn notify(&self) {
        // An error only means no receiver is left, which cannot happen because every handle owns
        // both ends of the channel.
        self.sender
            .send_modify(|pulses| *pulses = pulses.wrapping_add(1));
    }

    /// Pauses for `duration`, ending early on a pulse; `false` when shutdown was requested.
    pub async fn sleep(&mut self, duration: Duration, shutdown: &mut Shutdown) -> bool {
        tokio::select! {
            () = shutdown.wait() => return false,
            // The error only exists when no sender is left, which cannot happen here, so a closed
            // channel is treated as a pulse.
            _ = self.receiver.changed() => {}
            _ = tokio::time::sleep(duration) => {}
        }

        true
    }
}

impl Default for Wake {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_pulse_sent_before_the_pause_ends_it_at_once() {
        let mut wake = Wake::new();
        let mut shutdown = Shutdown::new();

        wake.notify();

        let slept = tokio::time::timeout(
            Duration::from_millis(100),
            wake.sleep(Duration::from_secs(60), &mut shutdown),
        )
        .await
        .expect("the pause should end on the pending pulse");

        assert!(slept);
    }

    #[tokio::test]
    async fn a_pulse_sent_during_the_pause_ends_it_early() {
        let mut wake = Wake::new();
        let mut shutdown = Shutdown::new();
        let pulser = wake.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            pulser.notify();
        });

        let slept = tokio::time::timeout(
            Duration::from_millis(1_000),
            wake.sleep(Duration::from_secs(60), &mut shutdown),
        )
        .await
        .expect("the pause should end on the arriving pulse");

        assert!(slept);
    }

    #[tokio::test]
    async fn a_pulse_is_consumed_once() {
        let mut wake = Wake::new();
        let mut shutdown = Shutdown::new();

        wake.notify();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(100),
                wake.sleep(Duration::from_secs(1), &mut shutdown)
            )
            .await
            .expect("the first pause should end on the pulse")
        );

        let second = tokio::time::timeout(
            Duration::from_millis(50),
            wake.sleep(Duration::from_secs(1), &mut shutdown),
        )
        .await;

        assert!(
            second.is_err(),
            "the second pause should block instead of spinning"
        );
    }

    #[tokio::test]
    async fn a_shutdown_request_ends_the_pause() {
        let mut wake = Wake::new();
        let mut shutdown = Shutdown::new();

        shutdown.request();

        assert!(!wake.sleep(Duration::from_secs(60), &mut shutdown).await);
    }

    #[tokio::test]
    async fn an_unpulsed_pause_falls_back_to_its_duration() {
        let mut wake = Wake::new();
        let mut shutdown = Shutdown::new();

        let slept = tokio::time::timeout(
            Duration::from_secs(1),
            wake.sleep(Duration::from_millis(1), &mut shutdown),
        )
        .await
        .expect("the pause should end on its own duration");

        assert!(slept);
    }
}
