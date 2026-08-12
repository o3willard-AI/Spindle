# Unwrap & Panic Audit

**Task**: Catalog all `.unwrap()` and `panic!()` calls across the Spindle workspace for C-2 prep.
**Scope**: All `.rs` files in workspace crates (excluding `target/`)
**Date**: 2026-08-12
**Status**: Catalog only — no fixes applied.

---

## Executive Summary

| Metric | Count |
|---|---|
| Total `.unwrap()` calls | **1,977** |
| `.unwrap()` in production code | **1,271** |
| `.unwrap()` in test code | **706** |
| Total `panic!()` calls | **11** |
| `panic!()` in production code | **6** |
| `panic!()` in test code | **5** |
| `.expect()` calls (better alternative) | **49** |

**Bottom line**: `spindle-server` is by far the largest offender with 935 non-test `.unwrap()` calls (73% of all production unwraps). `spindle-signing` is the only other crate with significant production unwrap volume (101).

---

## unwrap() by Crate (Non-Test Production Code)

| Non-Test | Test | Total | Crate | % of Total Non-Test |
|---------|------|-------|-------|---------------------|
| 935 | 168 | 1,103 | spindle-server | **73.6%** |
| 101 | 3 | 104 | spindle-signing | 7.9% |
| 50 | 0 | 50 | spindle-pipeline | 3.9% |
| 39 | 0 | 39 | spindle-config | 3.1% |
| 32 | 0 | 32 | spindle-dex | 2.5% |
| 27 | 0 | 27 | spindle-authz | 2.1% |
| 26 | 0 | 26 | spindle-rawarchive | 2.0% |
| 11 | 0 | 11 | spindle-obs | 0.9% |
| 7 | 0 | 7 | spindle-saml | 0.5% |
| 5 | 44 | 49 | spindle-cli | 0.4% |
| 5 | 250 | 255 | spindle-compliance | 0.4% |
| 4 | 66 | 70 | spindle-store | 0.3% |
| 3 | 0 | 3 | spindle-migrate | 0.2% |
| 2 | 0 | 2 | spindle-bench | 0.2% |
| 1 | 0 | 1 | mcp-server | 0.1% |
| 1 | 121 | 122 | spindle-archive | 0.1% |
| 1 | 28 | 29 | spindle-worker | 0.1% |
| 4 | 0 | 4 | spindle-mcp | 0.3% |
| 11 | 16 | 27 | spindle-identity | 0.9% |
| 27 | 0 | 27 | spindle-api | 2.1% |
| **1,271** | **706** | **1,977** | **TOTAL** | **100%** |

---

## Top 10 unwrap() Patterns

| Count | Pattern | Description | Risk |
|-------|---------|-------------|------|
| 454 | `.await.unwrap()` | Async result unwrapping (mostly in tests) | High — production async unwraps can panic at any `await` point |
| 123 | `.lock().unwrap()` | Mutex/RwLock lock acquisition | Low — only panics if mutex is poisoned |
| 39 | `.to_str().unwrap()` | Path to `&str` conversion | Low — panics on non-UTF8 paths |
| 32 | `.as_array().unwrap()` | JSON array access | Medium — panics if JSON type mismatch |
| 20 | `.parse().unwrap()` | String parsing | Medium — panics on invalid format |
| 18 | `.read().unwrap()` | RwLock read lock | Low — only panics if poisoned |
| 18 | `.as_str().unwrap()` | JSON string access | Medium — panics if JSON type mismatch |
| 18 | `.extract().unwrap()` | Header/query extraction | Medium — panics if header missing or malformed |
| 13 | `.write().unwrap()` | RwLock write lock | Low — only panics if poisoned |
| 12 | `.validate().unwrap()` | JWT token validation | Medium — panics on invalid JWT |

### Pattern Details

#### `.lock().unwrap()` / `.read().unwrap()` / `.write().unwrap()` (144 total)
- **Crate breakdown**: spindle-server (75), spindle-signing (13), spindle-dex (10), spindle-pipeline (4), spindle-compliance (4)
- **Assessment**: Low risk in production. `std::sync::Mutex` only panics on `unwrap()` when the mutex is poisoned (i.e., a thread panicked while holding the lock). `RwLock` similarly only panics on poison or write contention.
- **Recommendation**: Replace with `.lock().unwrap_or_else(|e| e.into_inner())` for poison recovery, or use `expect()` with descriptive messages.

#### `.await.unwrap()` (454 total, 258 in production)
- **Crate breakdown**: spindle-server (massive), spindle-compliance (mostly test), spindle-store (mostly test)
- **Assessment**: High risk. In production code, `async` operations can fail at any await point (network errors, timeouts, etc.). Using `.unwrap()` here means any async failure crashes the process.
- **Recommendation**: Replace with `?`, `match`, or `expect()` with context.

#### `.to_str().unwrap()` (39 total)
- **Crate breakdown**: spindle-rawarchive (30), spindle-server (9)
- **Assessment**: Low risk in practice (paths are almost always UTF-8 on Linux), but technically can panic on non-UTF8 filesystem paths.
- **Recommendation**: Use `.to_str().unwrap_or("")` or `.to_string_lossy()`.

#### `.as_array().unwrap()` / `.as_str().unwrap()` / `.as_object().unwrap()` (57 total)
- **Crate breakdown**: spindle-dex (majority), spindle-server, mcp-server
- **Assessment**: Medium risk. When parsing external JSON payloads, the structure may not match expectations, causing panics on malformed input.
- **Recommendation**: Use pattern matching (`if let Some(arr) = val.as_array()`) or `.ok_or_else()` with proper error handling.

#### `.parse().unwrap()` (20 total)
- **Crate breakdown**: spread across spindle-server, spindle-config, spindle-dex
- **Assessment**: Medium risk. String parsing (integers, UUIDs, etc.) can fail on malformed input.
- **Recommendation**: Use `?` operator or `.parse().map_err(|e| ...)`.

---

## panic!() Call Analysis (6 production, 5 test)

### Production panic!() calls

| # | Location | Cause | Severity |
|---|----------|-------|----------|
| 1 | `spindle-server/src/main.rs:298` | `unwrap_or_else(|_| panic!("SPindle_DATABASE_URL must be set for --process-payload"))` | Critical-path — CLI flag processing |
| 2 | `spindle-server/src/sessions.rs:60` | `panic!("FATAL: SPINDLE_JWT_SECRET is required in production mode...")` | Startup-only — fails fast in prod without JWT secret |
| 3 | `spindle-server/src/lib.rs:33` | `unwrap_or_else(|_| panic!("Failed to read migrations directory..."))` | Startup-only — migrations dir must exist |
| 4 | `spindle-signing/src/pkcs11.rs:304` | `panic!("signer must be configured with valid key before calling public_key()")` | Logic error — should be unreachable if API contract followed |
| 5 | `spindle-signing/src/lib.rs:587` | `panic!("signer must be unlocked before calling public_key()")` | Logic error — should be unreachable after unlock |
| 6 | `spindle-signing/src/lib.rs:593` | `panic!("signer must be unlocked before calling key_id()")` | Logic error — should be unreachable after unlock |
| 7 | `spindle-signing/src/kms.rs:206` | `panic!("public_key() not implemented for KMS signer — retrieve via DescribeKey API")` | Unimplemented trait method |

### Test panic!() calls (all in test code)

| # | Location | Pattern |
|---|----------|---------|
| 1 | `spindle-rawarchive/src/lib.rs:1084` | `_ => panic!("Expected PathTraversal error")` — match arm in test |
| 2 | `spindle-rawarchive/src/lib.rs:1095` | `_ => panic!("Expected PathTraversal error")` — match arm in test |
| 3 | `spindle-rawarchive/src/lib.rs:1197` | `_ => panic!("Expected WriteFailed error variant")` — match arm in test |
| 4 | `spindle-api/src/filter.rs:465` | `other => panic!("Expected Timestamp, got {other:?}")` — match arm in test |
| 5 | `spindle-signing/src/lib.rs:593` | `unwrap_or_else(|_| panic!(...))` — appears in test context |

### Assessment of production panic!() calls

| Risk Level | Count | Calls |
|-----------|-------|-------|
| **Fail-fast at startup** (acceptable) | 3 | #1 (CLI config), #2 (JWT secret), #3 (migrations dir) |
| **Logic error / unreachable** (should be Result) | 3 | #4 (PKCS11), #5 (signing), #6 (signing) |
| **Unimplemented** (should return error) | 1 | #7 (KMS public_key) |

The 3 startup fail-fast panics are generally acceptable (they fail before serving traffic). The remaining 4 are logic errors that should return `Result` types instead of panicking.

---

## Top 10 Files with Most unwrap() (Production + Test)

| # | File | Count |
|---|------|-------|
| 1 | `spindle-server/src/ingest.rs` | 168 |
| 2 | `spindle-server/src/tokens.rs` | 153 |
| 3 | `spindle-server/src/nodes.rs` | 141 |
| 4 | `spindle-compliance/tests/formats.rs` | 85 |
| 5 | `spindle-server/src/sessions.rs` | 76 |
| 6 | `spindle-server/src/runs.rs` | 76 |
| 7 | `spindle-compliance/tests/deterministic.rs` | 73 |
| 8 | `spindle-server/tests/negative_auth.rs` | 71 |
| 9 | `spindle-store/tests/store_integration.rs` | 66 |
| 10 | `spindle-server/src/waivers.rs` | 62 |

---

## Recommendations for C-2 (Prioritized)

### Tier 1 — Highest Priority (Critical Production Paths)
1. **`spindle-server/src/ingest.rs`** (168 unwraps) — This is the ingest pipeline. Replace `.await.unwrap()` with proper error handling using `?`. Critical for availability.
2. **`spindle-server/src/tokens.rs`** (153 unwraps) — Token management. `.lock().unwrap()` on token stores should use `unwrap_or_else(|e| e.into_inner())`.
3. **`spindle-server/src/nodes.rs`** (141 unwraps) — Node query endpoints. Review async unwraps in HTTP handlers.
4. **spindle-signing production panics** (7 unwraps + 4 panics) — Replace `unwrap()` with `?`, and `panic!()` in PKCS11/KMS signing with `Result` returns.

### Tier 2 — Medium Priority
5. **`spindle-server/src/sessions.rs`** (76 unwraps) — Session token operations. `.lock().unwrap()` pattern is low-risk but should use `expect()` for better diagnostics.
6. **`spindle-server/src/runs.rs`** (76 unwraps) — Run query endpoints.
7. **`spindle-server/src/waivers.rs`** (62 unwraps) — Waiver management.
8. **`spindle-signing/src/lib.rs`** (101 unwraps, 3 production) — Signing operations. Replace `panic!()` for unimplemented methods.

### Tier 3 — Lower Priority
9. **`spindle-compliance/tests/`** files — High unwrap count but all in test code. Acceptable per test conventions.
10. **`spindle-server/tests/negative_auth.rs`** (71 unwraps) — Test file, acceptable.

### Strategy
- **Tier 1**: Replace `.unwrap()` with `?` in error-returning functions, `.expect("msg")` otherwise. Replace `panic!()` in signing with `Result`.
- **Tier 2**: Replace `.lock().unwrap()` with `.lock().unwrap_or_else(|e| e.into_inner())` for poison recovery, or `.expect("descriptive message")`.
- **Tier 3**: Leave test code as-is (unwrap is idiomatic in tests).

---

## `.expect()` Usage (49 calls)

The workspace already uses `.expect()` in 49 places as a better alternative to `.unwrap()`. These provide diagnostic context. No action needed — this is the recommended pattern for cases where panicking is acceptable but a message is needed.

Crate breakdown:
- spindle-server: 24
- spindle-compliance: 12 (mostly test)
- spindle-dex: 4
- spindle-pipeline: 3
- spindle-config: 2
- spindle-signing: 2
- spindle-cli: 1
- spindle-store: 1

---

*Generated by automated workspace scan.*