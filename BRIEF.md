# Spindle Progress Tracker

## M0 — Foundation ✅
10/10 complete (Sergey)

## M1 — Ingest to Storage ✅
26/26 complete

## M2 — Query + Authorization ✅
14/14 complete

## M3 — Identity ✅
14/14 complete

| Task | Agent | Status |
|---|---|---|
| M3-01 Dex deployment | Mark | ✅ |
| M3-02 Principal model | Sergey | ✅ |
| M3-03 OIDC connector | Mark | ✅ |
| M3-04 SAML connector | Sergey | ✅ |
| M3-05 LDAP/AD connector | Mike | ✅ |
| M3-06 Local accounts | Mark | ✅ |
| M3-07 JIT provisioning | Sergey | ✅ |
| M3-08 Group/claim mappings | Mike | ✅ |
| M3-09 Mapping preview | Mark | ✅ |
| M3-10 Session management | Mike | ✅ |
| M3-11 Token types + creation | Mike | ✅ |
| M3-12 Token lifecycle | Mike | ✅ |
| M3-13 Idle token report | Sergey | ✅ |
| M3-14 Token reconciliation | Mike | ✅ |

## M4 — Evidence (15/16 complete)

### C9 Signing
| Task | Agent | Status |
|---|---|---|
| M4-01 Signer trait + Ed25519 | Mark | ✅ |
| M4-02 PKCS#11 | Mark | ✅ |
| M4-03 KMS | Mark | ✅ |
| M4-04 Key ID recording | Sergey | ✅ |
| M4-05 Key rotation | Mark | ✅ |
| M4-06 JWK publishing | Mark | ✅ |
| M4-07 Retry + hard fail | Sergey | ✅ |
| M4-08 Rate limiting + audit | Sergey | 🏃 |

### C10 Compliance
| Task | Agent | Status |
|---|---|---|
| M4-09 Report definitions | Mike | ✅ |
| M4-10 Signed attestation | — | ⬜ deferred |
| M4-11 Report formats | Mike | ✅ |
| M4-12 Reproducibility | Mike | ✅ |
| M4-13 Audit logging | Mike | ✅ |
| M4-14 Restored archive verification | Mike | ✅ |

### C11 Archive
| Task | Agent | Status |
|---|---|---|
| M4-15 Parquet export | Mike | ✅ |
| M4-16 Signed manifest | Mike | ✅ |

## M5 — Delivery (6/8 complete)

| Task | Agent | Status |
|---|---|---|
| M5-01 CLI API commands | Mike | ✅ |
| M5-02 CLI operator commands | Mike | ✅ |
| M5-03 CLI config profiles | Mike | ✅ |
| M5-04 Single binary + config | Mike | ✅ |
| M5-05 Air-gapped install | Mike | ✅ |
| M5-06 Metrics + health | Mike | ✅ |
| M5-07 Backup/restore | Mike | 🏃 |
| M5-08 Storage doc + load test | Mark | 🏃 |

## Current Assignments

| Agent | Task | Model |
|---|---|---|
| Mike | M5-07 Backup/restore | Laguna s-2.1 (free) |
| Mark | M5-08 Storage doc + load test | xiaomi/mimo-v2.5-pro |
| Sergey | M4-08 Rate limiting + audit | qwen3-235b-a22b |

## Pre-[DONE] Checklist

1. `git pull --rebase` — integrate latest
2. `cargo test` — must be green (no disk-full excuses)
3. `git status` — must be clean
4. `git push` — must land on origin
5. If disk > 90%, `cargo clean` first

## Last Updated
2026-08-08 07:40 UTC
