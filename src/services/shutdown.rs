//! Cooperative shutdown flag shared by the long-running services.
//!
//! The process observes signals in `infra::signal`; this module is the service-lifetime side of it.
//! The two directions are distinct types: one [`ShutdownTrigger`] sets the flag, and the services
//! hold clones of the [`Shutdown`] side that only observes it. The request is sticky, so it can
//! arrive before a service starts and is still visible when it does. The services check the flag at
//! work boundaries, and [`Shutdown::sleep`] replaces every idle pause so a wait is interrupted
//! instead of outliving the stop.

use std::time::Duration;

use tokio::sync::watch;

/// The only side that can stop the services.
pub struct ShutdownTrigger {
    sender: watch::Sender<bool>,
}

/// A cooperative shutdown request shared by the services.
///
/// Cloning is cheap and every clone observes the same flag. The request is sticky: once it is set,
/// [`Shutdown::is_requested`] stays true, so nothing has to await the transition to notice it.
#[derive(Clone)]
pub struct Shutdown {
    receiver: watch::Receiver<bool>,
}

/// Creates a trigger and the observing side its request reaches.
pub fn channel() -> (ShutdownTrigger, Shutdown) {
    let (sender, receiver) = watch::channel(false);

    (ShutdownTrigger { sender }, Shutdown { receiver })
}

impl ShutdownTrigger {
    /// Requests shutdown; every [`Shutdown`] observes it from now on.
    pub fn request(&self) {
        self.sender.send_replace(true);
    }
}

impl Shutdown {
    /// Returns whether shutdown was requested.
    pub fn is_requested(&self) -> bool {
        *self.receiver.borrow()
    }

    /// Waits until shutdown is requested.
    ///
    /// Also returns when the trigger is gone, so a dropped handle can never hang the caller.
    pub async fn wait(&mut self) {
        while !self.is_requested() && self.receiver.changed().await.is_ok() {}
    }

    /// Sleeps for `duration`, returning `false` when shutdown was requested first.
    pub async fn sleep(&mut self, duration: Duration) -> bool {
        tokio::select! {
            () = self.wait() => false,
            () = tokio::time::sleep(duration) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_fresh_flag_is_not_requested() {
        let (_trigger, shutdown) = channel();

        assert!(!shutdown.is_requested());
    }

    #[tokio::test]
    async fn a_request_is_visible_to_a_clone() {
        let (trigger, shutdown) = channel();
        let clone = shutdown.clone();

        trigger.request();

        assert!(clone.is_requested());
    }

    #[tokio::test]
    async fn wait_returns_after_a_request() {
        let (trigger, mut shutdown) = channel();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            trigger.request();
        });

        tokio::time::timeout(Duration::from_millis(100), shutdown.wait())
            .await
            .expect("wait should return after the request");
    }

    #[tokio::test]
    async fn wait_returns_at_once_when_the_request_came_first() {
        let (trigger, shutdown) = channel();
        trigger.request();
        let mut late = shutdown.clone();

        tokio::time::timeout(Duration::from_millis(100), late.wait())
            .await
            .expect("wait should return immediately");
    }

    #[tokio::test]
    async fn sleep_returns_false_when_shutdown_was_already_requested() {
        let (trigger, mut shutdown) = channel();
        trigger.request();

        assert!(!shutdown.sleep(Duration::from_secs(60)).await);
    }
}
