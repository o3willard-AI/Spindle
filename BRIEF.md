# Sergey — Spindle M0-05: Observability

Build `spindle-obs` crate in the workspace. Requirements: X-03, OPS-05.

## What to build
- `tracing` + `tracing-subscriber`: JSON to stdout, text for TTY
- `request_id` generation at edge (UUIDv7), propagated via tracing spans
- Axum middleware: inject `X-Request-Id` into every response
- Single entry point: `spindle_obs::init(config)`

## Tests
- All log lines for a request carry matching request_id
- Regex scan logs: no secrets, token plaintext, or passwords

## Stretch
- OTel trace exporter behind feature flag

## Verify
`cargo test -p spindle-obs` → all green, then push.
