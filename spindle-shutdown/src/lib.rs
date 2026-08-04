//! Graceful shutdown management for Spindle workers and servers.
//!
//! # Usage
//! ```ignore
//! use spindle_shutdown::{shutdown_signal, GracefulShutdown};
//!
//! let signal = shutdown_signal();
//! let shutdown = GracefulShutdown::new();
//!
//! tokio::select! {
//!     _ = signal => {
//!         let _ = shutdown.shutdown();
//!     }
//!     _ = some_async_work => {
//!         // Normal completion
//!     }
//! }
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::signal::unix::{signal, SignalKind};
use tokio::time::{Duration, sleep};

/// Signals the process should shut down (SIGTERM, SIGINT, SIGQUIT).
pub fn shutdown_signal() -> impl Future<Output = ()> {
    let mut sigterm = signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
    let mut sigint = signal(SignalKind::interrupt()).expect("failed to register SIGINT handler");
    let mut sigquit = signal(SignalKind::quit()).expect("failed to register SIGQUIT handler");

    async move {
        tokio::select! {
            _ = sigterm.recv() => tracing::info!("Received SIGTERM, initiating graceful shutdown"),
            _ = sigint.recv() => tracing::info!("Received SIGINT, initiating graceful shutdown"),
            _ = sigquit.recv() => tracing::info!("Received SIGQUIT, initiating graceful shutdown"),
        }
    }
}

/// Tracks in-flight requests and manages graceful shutdown.
pub struct GracefulShutdown {
    in_flight: Arc<AtomicBool>,
    shutdown_complete: Arc<AtomicBool>,
    deadline: Duration,
    drain_in_progress: Arc<AtomicBool>,
}

impl GracefulShutdown {
    /// Create a new `GracefulShutdown` with the given drain deadline.
    pub fn new(deadline: Duration) -> Self {
        Self {
            in_flight: Arc::new(AtomicBool::new(false)),
            shutdown_complete: Arc::new(AtomicBool::new(false)),
            deadline,
            drain_in_progress: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create a new `GracefulShutdown` with the default 30-second drain deadline.
    pub fn with_default_deadline() -> Self {
        Self::new(Duration::from_secs(30))
    }

    /// Mark a request as in-flight.
    pub fn mark_in_flight(&self) {
        self.in_flight.store(true, Ordering::SeqCst);
    }

    /// Mark a request as complete.
    pub fn mark_complete(&self) {
        if self.in_flight.load(Ordering::SeqCst) {
            self.in_flight.store(false, Ordering::SeqCst);
        }
    }

    /// Number of in-flight requests.
    pub fn in_flight_count(&self) -> usize {
        if self.in_flight.load(Ordering::SeqCst) {
            1
        } else {
            0
        }
    }

    /// Whether any requests are currently in-flight.
    pub fn has_in_flight(&self) -> bool {
        self.in_flight.load(Ordering::SeqCst)
    }

    /// Start the drain process (called when shutdown signal is received).
    pub fn start_drain(&self) {
        if !self.shutdown_complete.load(Ordering::SeqCst) {
            self.drain_in_progress.store(true, Ordering::SeqCst);
            self.shutdown();
        }
    }

    /// Block until all in-flight requests complete or the deadline expires.
    pub fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        let deadline = self.deadline;
        let shutdown_complete = self.shutdown_complete.clone();
        let in_flight = self.in_flight.clone();
        Box::pin(async move {
            tracing::info!("Starting graceful shutdown, deadline: {:?}", deadline);
            let deadline = tokio::time::Instant::now() + deadline;

            loop {
                let in_flight = in_flight.load(Ordering::SeqCst);
                if !in_flight {
                    tracing::info!("All in-flight requests completed");
                    break;
                }

                let remaining = deadline - tokio::time::Instant::now();
                if remaining <= Duration::from_secs(0) {
                    tracing::warn!("Shutdown deadline reached, force exiting");
                    break;
                }

                sleep(remaining).await;
            }

            shutdown_complete.store(true, Ordering::SeqCst);
            tracing::info!("Graceful shutdown complete");
        })
    }

    /// Whether the shutdown is in progress.
    pub fn is_shutting_down(&self) -> bool {
        self.shutdown_complete.load(Ordering::SeqCst)
    }
}

impl Default for GracefulShutdown {
    fn default() -> Self {
        Self::with_default_deadline()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graceful_shutdown_creation() {
        let shutdown = GracefulShutdown::new(Duration::from_secs(30));
        assert!(!shutdown.has_in_flight());
        assert!(!shutdown.is_shutting_down());
    }

    #[test]
    fn test_in_flight_tracking() {
        let shutdown = GracefulShutdown::new(Duration::from_secs(30));

        // Initially no in-flight
        assert_eq!(shutdown.in_flight_count(), 0);
        assert!(!shutdown.has_in_flight());

        // Mark as in-flight
        shutdown.mark_in_flight();
        assert_eq!(shutdown.in_flight_count(), 1);
        assert!(shutdown.has_in_flight());

        // Mark as complete
        shutdown.mark_complete();
        assert_eq!(shutdown.in_flight_count(), 0);
        assert!(!shutdown.has_in_flight());
    }

    #[test]
    fn test_shutdown_deadline() {
        let shutdown = GracefulShutdown::new(Duration::from_millis(100));
        assert_eq!(shutdown.deadline, Duration::from_millis(100));
    }

    #[tokio::test]
    async fn test_graceful_shutdown_with_in_flight() {
        let shutdown = GracefulShutdown::new(Duration::from_millis(100));
        shutdown.mark_in_flight();

        // Should not complete while in-flight
        shutdown.shutdown().await;
        // After deadline, should complete
        assert!(shutdown.is_shutting_down());
    }

    #[tokio::test]
    async fn test_graceful_shutdown_idle() {
        let shutdown = GracefulShutdown::new(Duration::from_millis(50));

        // Should complete immediately when idle
        shutdown.shutdown().await;
        assert!(shutdown.is_shutting_down());
    }
}
