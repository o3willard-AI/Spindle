# UAT Task 6: Persistent Signing Key Registry

## Summary

Verified that the `public_keys` PostgreSQL table correctly stores Ed25519 public keys,
manages key lifecycle (active/inactive), and supports signature verification lookups.

**Result: PASS ✅**

## Database Schema

| Column | Type | Description |
|--------|------|-------------|
| key_id | TEXT PK | Deterministic ID (`local:<sha256_hex>` or `aws-kms:<arn>`) |
| public_key | BYTEA | Raw 32-byte Ed25519 public key |
| algorithm | TEXT | Crypto algorithm (`ed25519` default) |
| active | BOOLEAN | Current signing key flag |
| created_at | TIMESTAMPTZ | Registration timestamp |
| key_spec | JSONB | Extra metadata (type, replaced_by, etc.) |

## Test Results

### Phase 1: Key Storage
- Table `public_keys` exists on `192.0.2.10` PostgreSQL
- Original signing key successfully inserted
- **PASS** ✅

### Phase 2: Key Rotation
- Generated new keypair with distinct unlock material
- Decrypted and registered new public key with `"type": "rotated"` metadata
- Old key preserved in database for audit trail
- **PASS** ✅

### Phase 3: Key Lifecycle Semantics
- Total keys in registry: `2`
- Inactive keys: `1`
- Oldest keys persist for audit trail and old artifact verification
- **PASS** ✅

### Phase 4: Signature Verification via DB Lookup
- Public key retrieved from `public_keys` table matches locally-extracted key
- Manifest signature verified using Ed25519 with PyNaCl backend
- **PASS** ✅

## Archive Signatures Tested

| File | Status | Key ID |
|------|--------|--------|
| manifest.json + manifest.sig | VERIFIED | `local:ad64d57e6bb424366116f2b1178fd2e22403280ebff9bbddb90c6cfd3be90ba4` |

All signatures produced by Rust `spindle-signing` crate verified by Python `PyNaCl/libsodium`.

---

*Generated: 2026-08-09 05:54 UTC*
*Pipeline executed by: automated agent (UAT Task 6)*
