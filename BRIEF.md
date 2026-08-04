# Sergey — Spindle M0-06: Error Handling

Build `spindle-error` crate. Requirements: X-02, API-07.

## What to build
- `Error` enum wrapping domain errors: `Ingest(Error)`, `Store(Error)`, etc. — use `thiserror`
- `ApiError` with `code` (machine-readable), `message` (human), optional `details`, `request_id`
- `impl Into<axum::response::Response>` for `ApiError` → uniform JSON envelope + correct HTTP status

## Tests
- Every error variant → correct HTTP status
- JSON envelope matches API-07 spec
- No bare `anyhow` across crate boundaries

## Stretch
- Error doc generator from code

## Verify
`cargo test -p spindle-error` → green, then push. Use `thiserror` derive, keep it simple.
