# Spindle Dependency Security Hardening — Audit Report

Date: 2026-08-11
Branch: `deps/security-hardening`

## Result

- `cargo audit` → **0 vulnerabilities, 0 unmaintained** (exit 0)
- `cargo deny check` → **advisories ok, bans ok, licenses ok, sources ok**

## What changed

### Dependency major bumps
| Crate | From | To | Reason |
|---|---|---|---|
| `sqlx` | 0.7.4 | 0.8.6 | RUSTSEC-2024-0363; `default-features = false` + `postgres` et al. |
| `reqwest` | 0.11 | 0.13.4 | TLS renamed `rustls-tls`→`rustls` (aws-lc-rs; needs cmake); drops rustls-pemfile |
| `object_store` | 0.9 | 0.14.1 | quick-xml ≥0.41 (2 HIGH CVEs); drops rustls-pemfile |
| `parquet` / `arrow` | 54 | 59.2.x | first parquet release that dropped `paste` (0-unmaintained gate) |
| `hyper` | 0.14 | 1 | (workspace decl; not used in code) — compatible with reqwest 0.13 |
| `ed25519-dalek` / `ed25519` | 2.1 / 2 | 3.0 / 3 | dependency alignment |
| `base64` | 0.21 | 0.22 | dependency alignment |
| `rand_core` | 0.6 | `rand` 0.10 | rand_core 0.10 removed `OsRng`; `thread_rng()` used |

### Dependency replacements
- `tabled` → `comfy-table` 8 (`spindle-cli/format_util.rs`) — evicts `proc-macro-error2` (unmaintained).
- KMS / aws-sdk-kms chain removed from `spindle-signing` (unused `kms` feature) — evicts the
  `rustls 0.21 → rustls-webpki 0.101.7` + `aws-smithy-http-client` advisory set.

### Configuration added
- `deny.toml` — advisories, licenses (incl. `[licenses.private]` for internal crates), bans (with
  documented `skip` for transitive-only duplicate majors), sources.
- `.cargo/audit.toml` — documented ignore for the single residual advisory (below).
- `license = "MIT OR Apache-2.0"` added to all 25 internal member manifests.

## Residual risk (§RUSTSEC-2023-0071)

The only remaining advisory in the original run — `rsa` ("Marvin Attack" timing side-channel) —
has **no fixed version** in RustSec's advisory DB. It enters the lockfile purely as a transitive,
feature-gated dependency of `sqlx-mysql` (the MySQL backend of sqlx), which this workspace does
**not** enable (all consumers use sqlx with `postgres`, facade is `default-features=false`).

- cargo-audit reports it because it scans the lockfile's declared-optional edges.
- cargo-deny's reachability analysis does **not** even see it (crate never compiled).
- `cargo tree -i rsa` → *"nothing to print"*.

The edge is intrinsic to the upstream sqlx manifest and cannot be removed by version alignment, so
it is documented and ignored (in both `deny.toml` context and `.cargo/audit.toml`) with the
reasoning above.

## Unmaintained crates evicted (all `patched = []`)
`paste` (via parquet 59.2), `proc-macro-error2` (via comfy-table), `rustls-pemfile` (via
reqwest 0.13 / sqlx 0.8 / object_store 0.14).
