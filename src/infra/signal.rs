//! Shutdown signals.
//!
//! [`wait`] resolves on the first signal that asks the process to stop: `SIGINT`, which is Ctrl-C,
//! or, on Unix, `SIGTERM`, which Compose, Docker, and supervisord send. This module is the only
//! place that registers a handler; the services observe a cooperative flag instead.

use std::fmt;

/// A signal that asks the process to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// Ctrl-C, or `SIGINT`.
    Interrupt,
    /// `SIGTERM`, the stop signal of Compose, Docker, and supervisord.
    Terminate,
}

impl fmt::Display for Signal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Interrupt => formatter.write_str("SIGINT"),
            Self::Terminate => formatter.write_str("SIGTERM"),
        }
    }
}

/// Waits for the first signal that asks the process to stop.
pub async fn wait() -> Signal {
    #[cfg(unix)]
    {
        let mut terminate =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(terminate) => terminate,
                Err(error) => {
                    tracing::error!(
                        error = &error as &dyn std::error::Error,
                        "Failed to listen for SIGTERM; only Ctrl-C stops the bot"
                    );

                    return interrupt().await;
                }
            };

        tokio::select! {
            signal = interrupt() => signal,
            _ = terminate.recv() => Signal::Terminate,
        }
    }

    #[cfg(not(unix))]
    {
        interrupt().await
    }
}

/// Resolves on Ctrl-C.
///
/// A handler that cannot be installed parks forever instead of resolving, so a broken registration
/// can never fake a shutdown.
async fn interrupt() -> Signal {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(
            error = &error as &dyn std::error::Error,
            "Failed to listen for Ctrl-C; it will not stop the bot"
        );

        std::future::pending().await
    }

    Signal::Interrupt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signals_render_their_names() {
        assert_eq!(Signal::Interrupt.to_string(), "SIGINT");
        assert_eq!(Signal::Terminate.to_string(), "SIGTERM");
    }
}
