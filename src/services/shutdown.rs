//! Cooperative shutdown flag shared by the long-running services.
//!
//! The process observes signals in `infra::signal`; this module is the service-lifetime side of it.
//! [`Shutdown`] is a cloneable handle over one sticky flag, so a request can arrive before a service
//! starts and is still visible when it does. The services check the flag at work boundaries, and
//! [`Shutdown::sleep`] replaces every idle pause so a wait is interrupted instead of outliving the
//! stop.

use std::time::Duration;

use tokio::sync::watch;

/// A cooperative shutdown request shared by the services.
///
/// Cloning is cheap and every clone observes the same flag. The request is sticky: once it is set,
/// [`Shutdown::is_requested`] stays true, so nothing has to await the transition to notice it.
#[derive(Clone)]
pub struct Shutdown {
    sender: watch::Sender<bool>,
    receiver: watch::Receiver<bool>,
}

impl Shutdown {
    /// Creates a handle whose shutdown has not been requested.
    pub fn new() -> Self {
        let (sender, receiver) = watch::channel(false);

        Self { sender, receiver }
    }

    /// Requests shutdown; every clone observes it from now on.
    pub fn request(&self) {
        // An error only means no receiver is left, so nothing is waiting for the flag.
        let _ = self.sender.send(true);
    }

    /// Returns whether shutdown was requested.
    pub fn is_requested(&self) -> bool {
        *self.receiver.borrow()
    }

    /// Waits until shutdown is requested.
    ///
    /// Also returns when the sender is gone, so a dropped handle can never hang the caller.
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

impl Default for Shutdown {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_fresh_handle_is_not_requested() {
        assert!(!Shutdown::new().is_requested());
    }

    #[tokio::test]
    async fn a_request_is_visible_to_a_clone() {
        let shutdown = Shutdown::new();
        let clone = shutdown.clone();

        shutdown.request();

        assert!(clone.is_requested());
    }

    #[tokio::test]
    async fn wait_returns_after_a_request() {
        let mut shutdown = Shutdown::new();
        let requester = shutdown.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            requester.request();
        });

        tokio::time::timeout(Duration::from_millis(100), shutdown.wait())
            .await
            .expect("wait should return after the request");
    }

    #[tokio::test]
    async fn wait_returns_at_once_when_the_request_came_first() {
        let mut shutdown = Shutdown::new();
        shutdown.request();

        tokio::time::timeout(Duration::from_millis(100), shutdown.wait())
            .await
            .expect("wait should return immediately");
    }

    #[tokio::test]
    async fn sleep_returns_false_when_shutdown_was_already_requested() {
        let mut shutdown = Shutdown::new();
        shutdown.request();

        assert!(!shutdown.sleep(Duration::from_secs(60)).await);
    }
}
