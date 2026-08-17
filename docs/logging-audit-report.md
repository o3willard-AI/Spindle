# Logging Architecture Audit Report

**Date:** 2026-08-17 · **Auditor:** Release Engineer · **Scope:** Post-refactor audit of the three-tier logging scheme spec'd in `docs/logging-architecture.md`

---

## Executive Summary

The three-tier logging scheme is **partially implemented and has drifted**. The tier-mapping logic is duplicated in 3+ places with no single source of truth. `spindle-obs::init()` exists and is correct but is **never called by any binary** — every binary inlines its own subscriber setup. The secret scanner exists but is **dead code** (never wired into the subscriber pipeline). Several new code paths added by the refactors have no logging at all. No rename drift was found in log messages (the trademark refactors were thorough).

---

## Finding 1 — Duplicated LogLevel enum (REAL inconsistency)

**Severity: Medium**

Two independent `LogLevel` enums exist:

| Location | File | Lines |
|---|---|---|
| `spindle-obs::LogLevel` | `spindle-obs/src/lib.rs:39-70` | Has `FromStr`, `as_tracing_level()`, `Config.log_level: Option<LogLevel>` |
| `spindle-config::LogLevel` | `spindle-config/src/lib.rs:791-814` | Has `as_tracing_level()`, `Serialize`/`Deserialize`, used by `ObservabilityConfig` |

Both define the same three variants (`Operational`, `Diagnostic`, `Debug`) with the same mapping (`info`, `debug`, `trace`). But they are **completely separate types** — a `spindle_config::LogLevel` cannot be passed to `spindle_obs::Config`.

**What the spec said:** `spindle-obs` owns the log level. `spindle-config` reads it from TOML/env and passes it to `spindle-obs::init()`.

**What happened:** `spindle-config` grew its own `LogLevel` + `ObservabilityConfig` for TOML deserialization, but nothing bridges it to `spindle-obs`. The two enums have diverged slightly: `spindle-obs::LogLevel` has `FromStr` (accepts `"l1"`, `"l2"`, `"l3"` aliases); `spindle-config::LogLevel` uses `#[serde(rename_all = "kebab-case")]` and has `#[derive(Default)]`.

**Recommended fix:** Delete one. Keep `spindle-config::LogLevel` (it has serde support for TOML). Add a `From<spindle_config::LogLevel> for spindle_obs::LogLevel` impl, or re-export `spindle_config::LogLevel` from `spindle-obs` and delete the duplicate. Then have each `main.rs` call `spindle_obs::init()` with a `Config` built from `spindle_config::Config`.

---

## Finding 2 — `spindle-obs::init()` is never called (REAL inconsistency)

**Severity: High**

The spec (`docs/logging-architecture.md:36-53`) says: *"`spindle-obs::init(&Config)` already exists. Wire from `spindle-config`."*

**Reality:** No binary calls `spindle_obs::init()`. Every binary inlines its own subscriber:

| Binary | File | What it does |
|---|---|---|
| spindle-server | `main.rs:157-191` | Inline match on `SPINDLE_LOG_LEVEL`, builds subscriber with `.json()` or text, `set_global_default`. Does NOT use `spindle-obs`. |
| spindle-worker | `main.rs:35-50` | Inline match on `SPINDLE_LOG_LEVEL` (identical logic), `.json().init()`. Does NOT use `spindle-obs`. |
| spindle-migrate | `main.rs:37-38` | `EnvFilter::try_from_default_env().unwrap_or("info")`, `.init()`. No tier mapping at all — ignores `SPINDLE_LOG_LEVEL`. |
| spindle-dashboard | `main.rs:87-89` | `EnvFilter::try_from_default_env()`. No tier mapping. |
| spindle-cli | (none) | No tracing initialization. |
| spindle-mcp | (none) | No tracing initialization. |

**The inline match in server and worker is a copy-paste of the same 8 lines** with the same tier→level mapping. `spindle-obs::init()` already does this correctly (lines 129-161) and also handles `RUST_LOG` overrides, `scan_secrets`, and the `log_level` field — but nobody calls it.

**Recommended fix:** Replace the inline subscriber setup in every `main.rs` with a call to `spindle_obs::init(&Config::from_tier(&log_level))`. Add tier mapping to migrate and dashboard. Add minimal tracing init to CLI and MCP.

---

## Finding 3 — Tier mapping is N divergent copies (REAL inconsistency)

**Severity: Medium**

The `operational|diagnostic|debug → info|debug|trace` mapping exists in **4 copies**:

1. `spindle-obs/src/lib.rs:49-52` — `LogLevel::from_str()` (accepts `l1`/`l2`/`l3` aliases)
2. `spindle-obs/src/lib.rs:63-68` — `LogLevel::as_tracing_level()`
3. `spindle-config/src/lib.rs:807-810` — `LogLevel::as_tracing_level()` (same logic, different type)
4. `spindle-server/src/main.rs:162-167` — inline `match` (no aliases, no `l1`/`l3`)
5. `spindle-worker/src/main.rs:37-42` — inline `match` (identical copy of #4)

The inline matches (#4, #5) are **missing the `"debug"` → `"trace"` case** — they map `"trace"` → `"trace"` but don't map `"debug"` (the L3 tier name) to `"trace"`. Wait — let me re-check:

```rust
// main.rs:162-167
"operational" | "info" => "info",
"diagnostic" | "debug" => "debug",
"trace" => "trace",
_ => "info",
```

This maps `"debug"` → `"debug"` (L2), not `"trace"` (L3). But the spec says `SPINDLE_LOG_LEVEL=debug` should mean L3 (trace). The `spindle-obs::LogLevel::from_str()` correctly maps `"debug"` → `Debug` (→ `"trace"`). **The inline copies disagree with the spec and with `spindle-obs`.**

**Impact:** An operator who sets `SPINDLE_LOG_LEVEL=debug` expecting L3 (full payload bodies, SQL) will get L2 (metadata only) instead. The `spindle-obs` enum would give them L3.

**Recommended fix:** Delete the inline matches. Use `spindle-obs::LogLevel::from_str()` everywhere.

---

## Finding 4 — Secret scanner is dead code (REAL inconsistency)

**Severity: High**

`spindle-obs/src/secret_scan.rs` defines `scan_log_line()` and `ScanResult`. The `Config.scan_secrets` flag is set to `true` by default in `spindle-obs::Config::default()`. The `init()` function computes `scan_secrets: cfg.scan_secrets && cfg.target == "stdout"` and logs it.

**But `scan_log_line` is never called.** It's not wired into the tracing subscriber as a `MakeWriter` or layer. The subscriber is built as a plain `tracing_subscriber::fmt::Subscriber` with no secret-scanning layer. A grep for `scan_log_line` across the entire codebase returns zero call sites outside the definition.

**The spec said:** *"The `spindle-obs` secret scanner is the backstop"* (§8-Auth, line 320). It is not.

**Impact:** If `RUST_LOG=trace` is set accidentally, raw tokens and payload bodies in `tracing::trace!` calls will be logged unredacted. The only protection is the `tracing::trace!` filter itself — there is no runtime secret-scanning guard.

**Recommended fix:** Implement a `tracing_subscriber::Layer` that wraps the fmt layer and runs `scan_log_line` on each event's formatted output. Or use a `MakeWriter` wrapper. This is the "hard guard" the spec requires.

---

## Finding 5 — L3 auth guard is a comment, not code (REAL inconsistency)

**Severity: Medium**

The spec (§8, lines 316-320) says: *"A hard guard (not just the filter) so a mis-set `RUST_LOG` cannot leak tokens."*

**What exists** (`jit_auth.rs:417-430`):
```rust
// L3: full token contents — HARD GUARDED. Only logged when tracing level is
// trace (L3/debug mode). The tracing filter ensures this line only fires
// when explicitly enabled.
tracing::trace!(
    token_jti = "redacted",
    decoded_claims = ?"{subject, session_id, connector, token_type, iat, exp, scope, iss}",
    "auth full token contents (L3 only)"
);
```

This is **not a hard guard** — it's a `tracing::trace!` call that relies entirely on the `EnvFilter` to suppress it. If someone sets `RUST_LOG=trace`, the line fires. The values logged are safe (jti is `"redacted"`, claims are a string description not the actual claims) — so the implementation is actually conservative. But the spec's requirement for a **runtime check** (e.g. `if cfg.level <= trace { … }`) is not met.

Similarly in `ingest.rs:1116-1123`, the L3 payload body dump:
```rust
tracing::trace!(
    body = %serde_json::to_string(&payload_json)...,
    "ingest full payload body"
);
```
This logs the full payload at `trace` level with no hard guard. If `RUST_LOG=trace`, payload bodies (which may contain node attributes, secrets in Chef data bags, etc.) are logged in plaintext. The dead secret scanner was supposed to be the backstop.

**Recommended fix:** Either (a) wire the secret scanner into the subscriber (Finding 4), or (b) add a runtime `if log::max_level() >= LevelFilter::Trace` gate before sensitive `trace!` calls. Option (a) is better — it protects all `trace!` calls globally.

---

## Finding 6 — L1/L2/L3 comments are correct but some reference wrong fields (MINOR)

**Severity: Low**

The `// L1:`, `// L2:`, `// L3:` comments across `ingest.rs`, `jit_auth.rs`, `main.rs`, `resource_events.rs`, `spindle-store/src/lib.rs`, and `spindle-pipeline/src/lib.rs` are **mostly accurate** after the refactors. Specific findings:

- **`ingest.rs:1552`** — `// L2: Cinc Auditor payload metadata` — correct (was "InSpec", renamed properly).
- **`ingest.rs:1560`** — `// L3: full Cinc Auditor payload body` — correct.
- **`spindle-store/src/lib.rs:386`** — `// L1: row written` followed by `tracing::info!(table = "node", ...)` — correct.
- **`spindle-store/src/lib.rs:388`** — `// L2: per-table latency` followed by `tracing::debug!(...)` — correct.
- **`spindle-pipeline/src/lib.rs:266`** — `// L1: events processed` — correct.
- **`jit_auth.rs:417`** — `// L3: full token contents` — comment says "HARD GUARDED" but it isn't (see Finding 5).

No stale field references were found in the tier comments — the field names in `tracing::info!`/`debug!`/`trace!` calls match the current struct fields.

---

## Finding 7 — No rename drift in log messages (CLEAN)

**Severity: None**

A comprehensive grep for `Chef`, `InSpec`, `inspec`, and `ingest_queue` in `tracing::` call strings across `spindle-server/src/`, `spindle-worker/src/`, `spindle-store/src/`, and `spindle-pipeline/src/` found **zero matches** (excluding wire-format field names like `chef_environment`, `chef_server_url`, which are correctly exempt).

The trademark refactors (Chef→Cinc, InSpec→Auditor, ingest_queue→jobs) were applied thoroughly to log message strings.

---

## Finding 8 — Coverage gaps: new code paths with no logging (REAL inconsistency)

**Severity: Medium**

Several code paths added by the refactors have **no logging at all**:

| Code path | File | What's missing |
|---|---|---|
| `find_node_id_by_name()` | `spindle-store/src/lib.rs:282-289` | No L1/L2 log for the node-dedup lookup. The worker's `process_compliance_job` calls this and logs at `debug` (L2), but the store method itself is silent. |
| `pipeline_trigger.rs` | `spindle-server/src/pipeline_trigger.rs` | **Zero `tracing::` calls** in the entire file. The `--process-payload` one-shot pipeline path has no logging at any tier. |
| `spindle-mcp` | `spindle-mcp/src/main.rs` | No tracing init, no logging. |
| `spindle-cli` | `spindle-cli/src/main.rs` | No tracing init, no logging. |
| `count_nodes` | `spindle-store/src/lib.rs:391-396` | No L1 log (unlike `upsert_node` which has L1). |
| Worker `process_compliance_job` success | `spindle-worker/src/lib.rs` | The compliance job path has `debug` (L2) for node lookup but **no L1 `info!`** when the job completes. The data-collector path (`process_run_job`) also lacks L1 completion logging (spec §4 says "L1 is the fix" for "worker currently logs nothing on success"). |

**Recommended fix:** Add L1 `tracing::info!` to: `process_job` completion, `process_compliance_job` completion, `find_node_id_by_name` (at L2), `pipeline_trigger` process steps, and `count_nodes`. Add minimal tracing init to CLI and MCP.

---

## Finding 9 — `spindle-config::ObservabilityConfig` is loaded but never used (REAL inconsistency)

**Severity: Low**

`spindle-config/src/lib.rs:817-845` defines `ObservabilityConfig` with `log_level: LogLevel` and `scan_secrets: bool`. The root `Config` struct (line 887) includes `pub observability: ObservabilityConfig`. `Config::load()` reads it from TOML/env.

**But `main.rs` never reads `config.observability`**. The server's `main.rs` reads `SPINDLE_LOG_LEVEL` directly from `std::env::var()` (line 161), bypassing the config system entirely. An operator who sets `[observability] log_level = "diagnostic"` in `config.toml` will have it ignored.

**Recommended fix:** In `main.rs`, after `Config::load()`, use `config.observability.log_level.as_tracing_level()` to initialize the subscriber instead of reading `SPINDLE_LOG_LEVEL` directly.

---

## Summary Table

| # | Finding | Severity | Type | Fix effort |
|---|---|---|---|---|
| 1 | Duplicated `LogLevel` enum in obs + config | Medium | Real | Small — delete one, add bridge |
| 2 | `spindle-obs::init()` never called | High | Real | Medium — rewrite 6 `main.rs` init blocks |
| 3 | N divergent tier-mapping copies, inline copies disagree with spec | Medium | Real | Small — delete inline matches, use `spindle-obs` |
| 4 | Secret scanner is dead code, never wired into subscriber | High | Real | Medium — implement tracing `Layer` or `MakeWriter` |
| 5 | L3 auth "hard guard" is a comment, not runtime code | Medium | Real | Small (if Finding 4 fixed) or Medium (standalone) |
| 6 | L1/L2/L3 comments mostly correct, one false "HARD GUARDED" | Low | Cosmetic | Trivial — update comment |
| 7 | No rename drift in log messages | None | Clean | — |
| 8 | New code paths with no logging (pipeline_trigger, worker completion, MCP, CLI) | Medium | Real | Medium — add L1 calls + tracing init |
| 9 | `ObservabilityConfig` loaded but never read by any binary | Low | Real | Small — wire config → init |

---

## Prioritized Recommendations

1. **[High] Wire `spindle-obs::init()` into all binaries** (Finding 2). This also fixes Finding 3 (tier mapping) and Finding 9 (config usage) in one pass.
2. **[High] Implement the secret scanner as a tracing layer** (Finding 4). This is the spec's "hard guard" backstop and fixes Finding 5.
3. **[Medium] Add L1 logging to worker job completion + pipeline_trigger + count_nodes** (Finding 8). The spec explicitly calls out "worker logs nothing on success" as a gap to fix.
4. **[Medium] Consolidate the `LogLevel` enum** (Finding 1). Delete `spindle-obs::LogLevel`, re-export `spindle-config::LogLevel`.
5. **[Low] Fix the false "HARD GUARDED" comment** in `jit_auth.rs:417` (Finding 6) — or better, make it true by implementing Finding 4.
