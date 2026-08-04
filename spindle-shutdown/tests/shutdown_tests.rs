use spindle_shutdown::*;
use std::time::Duration;

#[test]
fn test_graceful_shutdown_lifecycle() {
    let shutdown = GracefulShutdown::new(Duration::from_millis(100));

    // Initially idle
    assert!(!shutdown.has_in_flight());
    assert!(!shutdown.is_shutting_down());

    // Mark in-flight
    shutdown.mark_in_flight();
    assert!(shutdown.has_in_flight());

    // Complete the request
    shutdown.mark_complete();
    assert!(!shutdown.has_in_flight());
}

#[tokio::test]
async fn test_shutdown_with_in_flight_requests() {
    let shutdown = GracefulShutdown::new(Duration::from_millis(200));

    // Simulate in-flight work
    shutdown.mark_in_flight();

    // Start shutdown and wait for it to complete
    shutdown.shutdown().await;

    // Should have completed after the deadline
    assert!(shutdown.is_shutting_down());
}

#[tokio::test]
async fn test_shutdown_idle_exits_quickly() {
    let shutdown = GracefulShutdown::new(Duration::from_millis(100));

    // Start shutdown while idle
    let start = std::time::Instant::now();
    shutdown.shutdown().await;
    let elapsed = start.elapsed();

    assert!(elapsed < Duration::from_millis(200));
    assert!(shutdown.is_shutting_down());
}
