# Sergey — Spindle M0-07: Graceful Shutdown

Build `spindle-shutdown` crate (or module in spindle-obs). Requirement: X-04.

## What to build
- `shutdown_signal()`: future that resolves on SIGTERM/SIGINT
- `GracefulShutdown` struct: tracks in-flight requests, drains connections within deadline (default 30s), then force-exits
- Workers: finish current job or requeue before exit

## Tests
- SIGTERM during active request → request completes, then exit
- SIGTERM during idle → exits within 100ms
- No race between drain and new connections

## Stretch
- Drain progress as Prometheus metric

## Verify
`cargo test` → green, then push. Keep it focused — just the shutdown primitives.
