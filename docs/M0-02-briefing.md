# M0-02: Cargo workspace + repository skeleton

**Your task:** Create the full Cargo workspace for Spindle per the spec.

## What to build

1. **Workspace `Cargo.toml`** — workspace with these members:

**Binaries:**
- `spindle-server` — HTTP API + ingest endpoint
- `spindle-worker` — queue consumers, rollups, exports
- `spindle-cli` — operator CLI

**Library crates:**
- `spindle-config` — figment-based config (M0-04, scaffold now)
- `spindle-obs` — tracing + observability (M0-05, scaffold now)
- `spindle-error` — thiserror error types (M0-06, scaffold now)
- `spindle-rawarchive` — raw archive trait (M1-01, scaffold now)
- `spindle-store` — sqlx store layer (M1-10, scaffold now)
- `spindle-pipeline` — ingest pipeline (M2, scaffold now)
- `spindle-ingest` — ingest HTTP handler (M1-11, scaffold now)
- `spindle-api` — REST API (M3, scaffold now)
- `spindle-identity` — identity model (M3, scaffold now)
- `spindle-tokens` — token management (M3, scaffold now)
- `spindle-authz` — authorization (M3, scaffold now)
- `spindle-signing` — hash chain signing (M4, scaffold now)
- `spindle-compliance` — compliance reporting (M4, scaffold now)
- `spindle-archive` — archive management (M4, scaffold now)

2. **Each crate**: `Cargo.toml` with `[package]` name, version "0.1.0", edition "2021". Empty `src/lib.rs` or `src/main.rs` with `fn main() {}` for binaries.

3. **Workspace-level dependencies** in workspace `Cargo.toml` `[workspace.dependencies]`:
- tokio = "1", axum = "0.8", tower = "0.5", sqlx = "0.8"
- serde = "1", serde_json = "1", uuid = "1" (v7 feature)
- tracing = "0.1", tracing-subscriber = "0.3"
- clap = "4" (derive), figment = "0.10"
- thiserror = "2", reqwest = "0.12"
- time = "0.3", object_store = "0.11"
- openidconnect = "4", jsonwebtoken = "9"
- argon2 = "0.5", parquet = "54", arrow = "54"

4. **`.gitignore`** for Rust: target/, Cargo.lock (for libraries — keep for binaries), .env, *.log

5. **Rust toolchain**: `rust-toolchain.toml` pinning stable, with components: rustfmt, clippy

## Verify
- [ ] `cargo build` succeeds from workspace root
- [ ] `cargo test` runs zero tests (green, no failures)
- [ ] `cargo fmt --check` passes on all code
- [ ] 18 crate directories created with correct structure

## Push
```bash
git add -A && git commit -m "M0-02: Cargo workspace skeleton — 18 crates" && git push origin main
```

## Notes
- Only scaffold structure — actual code comes in M0-04 through M0-10
- Each lib crate gets one `pub fn placeholder() -> &'static str { "TODO" }` so it compiles
- Don't implement anything beyond the skeleton
- Your context is fresh — 131K available
