# Unsafe `impl Send/Sync` Audit

**Date:** 2026-08-12  
**Auditor:** Automated grep + manual review  
**Scope:** `spindle-signing` crate (the only crate with `unsafe impl Send/Sync`)

## Summary

| # | File | Line | Type | Trait | Feature Gate | Notes |
|---|------|------|------|-------|-------------|-------|
| 1 | `spindle-signing/src/key_rotation.rs` | 245 | `KeyRegistry` | `Send` | (none — always compiled) | In-memory key registry backed by `RwLock<BTreeMap>`. Safe: `RwLock` provides interior mutability with thread-safe locking. |
| 2 | `spindle-signing/src/key_rotation.rs` | 246 | `KeyRegistry` | `Sync` | (none — always compiled) | Same type. Safe: `BTreeMap` and `String` keys are `Send + Sync`; `RwLock` is `Sync`. |
| 3 | `spindle-signing/src/key_rotation.rs` | 480 | `PostgresKeyRegistry` | `Send` | `#[cfg(feature = "postgres")]` | Wraps `sqlx::PgPool` which is already `Send + Sync`. The `unsafe impl` is redundant — `PgPool` is thread-safe by design. |
| 4 | `spindle-signing/src/key_rotation.rs` | 482 | `PostgresKeyRegistry` | `Sync` | `#[cfg(feature = "postgres")]` | Same type. Same as above — redundant `unsafe impl` since `PgPool` is already `Sync`. |
| 5 | `spindle-signing/src/kms.rs` | 218 | `KmsSigner` | `Send` | (none — always compiled, but `aws_sdk_kms::Client` may be behind `#[cfg(feature = "kms")]`) | Wraps `Arc<Client>` (AWS KMS SDK client). `Arc<T>` is `Send + Sync` iff `T: Send + Sync`. AWS SDK `Client` is designed to be thread-safe. The `unsafe impl` may be needed because the `crypt` dependency or FFI layer doesn't provide auto-traits. |
| 6 | `spindle-signing/src/kms.rs` | 219 | `KmsSigner` | `Sync` | (same) | Same type. Safe if `Client: Sync` (which it is in the AWS SDK). |
| 7 | `spindle-signing/src/pkcs11.rs` | 315 | `Pkcs11Signer` | `Send` | (none — always compiled, but `Pkcs11` from `cryptoki` crate may be behind `#[cfg(feature = "pkcs11")]`) | Contains: `Pkcs11` (cryptoki context), `ObjectHandle`, `String`, `KeyId`, `Mutex<Option<Arc<Session>>>`. The `Pkcs11` context type from the `cryptoki` crate is `!Send + !Sync` by default because it wraps a raw PKCS#11 `CK_FUNCTION_LIST` pointer. The `Mutex<Option<Arc<Session>>>` field is `Send + Sync` if `Session` is. |
| 8 | `spindle-signing/src/pkcs11.rs` | 316 | `Pkcs11Signer` | `Sync` | (same) | Same type. **Highest risk**: `Pkcs11` context is not natively `Send/Sync`. The `unsafe impl` overrides the compiler's safety guarantee. Correctness depends on the PKCS#11 library (e.g., SoftHSM2, AWS CloudHSM) being thread-safe. The comment says "sessions are recreated as needed" — if `Session` is `Send + Sync`, this may be sound, but the `Pkcs11` context itself needs verification. |

## Detailed Analysis

### 1. `KeyRegistry` (key_rotation.rs:72)

```rust
pub struct KeyRegistry {
    keys: RwLock<BTreeMap<String, KeyEntry>>,
}
```

- **Fields:** Single field `RwLock<BTreeMap<String, KeyEntry>>`
- **Safety assessment:** **SAFE** — `RwLock<T>` is `Send` when `T: Send`, and `Sync` when `T: Send + Sync`. `BTreeMap<String, KeyEntry>` and `String` are both `Send + Sync`. The `unsafe impl` is actually **redundant** here — the compiler would auto-derive `Send` and `Sync` for this type.
- **Risk:** None. The `unsafe impl` could be removed without behavioral change.

### 2. `PostgresKeyRegistry` (key_rotation.rs:255)

```rust
#[cfg(feature = "postgres")]
pub struct PostgresKeyRegistry {
    pool: sqlx::PgPool,
}
```

- **Fields:** Single field `sqlx::PgPool`
- **Safety assessment:** **REDUNDANT** — `sqlx::PgPool` is already `Send + Sync` (it wraps `Arc<Pool<Postgres>>`). The compiler would auto-derive `Send` and `Sync` for this struct.
- **Risk:** None. The `unsafe impl` is unnecessary and could be removed.

### 3. `KmsSigner` (kms.rs:54)

```rust
pub struct KmsSigner {
    client: Arc<Client>,  // aws_sdk_kms::Client
    key_id: String,
    key_id_label: KeyId,
}
```

- **Fields:** `Arc<Client>`, `String`, `KeyId`
- **Safety assessment:** **LIKELY SAFE** — `aws_sdk_kms::Client` is designed to be thread-safe and is `Send + Sync`. `Arc<Client>` is `Send + Sync` if `Client: Send + Sync`. The `unsafe impl` may be needed if the `cryptoki` or underlying FFI crate doesn't expose `Send/Sync` for some transitive dependency. Needs verification that `KeyId` (likely a `String` or `Cow<str>`) is also `Send + Sync`.
- **Risk:** Low. Standard AWS SDK usage pattern.

### 4. `Pkcs11Signer` (pkcs11.rs:51)

```rust
pub struct Pkcs11Signer {
    pkcs11: Pkcs11,                              // cryptoki::context::Pkcs11
    slot_id: u64,
    key_handle: ObjectHandle,
    pin: String,
    key_label: String,
    key_id: KeyId,
    session: Mutex<Option<Arc<cryptoki::session::Session>>>,
}
```

- **Fields:** `Pkcs11` (cryptoki context), `u64`, `ObjectHandle`, `String` × 2, `KeyId`, `Mutex<Option<Arc<Session>>>`
- **Safety assessment:** **MEDIUM RISK** — This is the most concerning case. The `cryptoki::context::Pkcs11` type wraps a raw PKCS#11 library context (a `CK_FUNCTION_LIST` pointer). The `cryptoki` crate intentionally does not implement `Send`/`Sync` for `Pkcs11` because PKCS#11 library thread-safety depends on the underlying HSM/driver implementation and the `CK_C_INITIALIZE_ARGS` flags used during `C_Initialize`.
  - The `unsafe impl Send + Sync` overrides this safety guard.
  - The `Mutex<Option<Arc<Session>>>` field is `Send + Sync` only if `Session: Send + Sync` (sessions from the `cryptoki` crate may also not be `Send/Sync` by default).
  - **Verification needed:** Confirm that the `cryptoki` crate's `Session` type is `Send + Sync`, and that the PKCS#11 library in use (e.g., SoftHSM2) is initialized with `CKF_OSLockingOK` or equivalent. If the HSM library is not thread-safe, concurrent calls through the shared `Pkcs11` context could cause undefined behavior.
- **Risk:** Medium. Requires runtime verification of the PKCS#11 library's threading model.

## Recommendations

1. **Redundant `unsafe impl`s:** Remove `unsafe impl Send/Sync` for `KeyRegistry` (lines 245-246) and `PostgresKeyRegistry` (lines 480-482). These types' fields are already thread-safe; the `unsafe impl`s add no value and obscure the fact (by implication) that there's something non-trivial to audit.

2. **`KmsSigner` (lines 218-219):** Verify that `KeyId` is `Send + Sync`. If it is, the `unsafe impl` may be removable. If the `cryptoki` crate's types transitively block auto-derivation, add a comment documenting why the `unsafe impl` is required.

3. **`Pkcs11Signer` (lines 315-316):** **Requires manual verification.** The `unsafe impl` overrides the `cryptoki` crate's deliberate `!Send` stance. Confirm:
   - The PKCS#11 library is initialized with `CKF_OSLockingOK`
   - The `cryptoki::session::Session` type is `Send + Sync`
   - The `cryptoki::object::ObjectHandle` and `cryptoki::context::Pkcs11` types have no internal mutable state accessed concurrently without synchronization
   - Document the threading assumption in a `// SAFETY:` comment

## Count

- **Total `unsafe impl` occurrences:** 8
- **Across 3 files:** `key_rotation.rs` (4), `kms.rs` (2), `pkcs11.rs` (2)
- **Across 4 types:** `KeyRegistry`, `PostgresKeyRegistry`, `KmsSigner`, `Pkcs11Signer`
- **Feature-gated:** 2 occurrences (`PostgresKeyRegistry`) gated behind `#[cfg(feature = "postgres")]`
- **Redundant (safe to remove):** 6 occurrences (`KeyRegistry` × 2, `PostgresKeyRegistry` × 2, possibly `KmsSigner` × 2)
- **Requires manual verification:** 2 occurrences (`Pkcs11Signer` × 2)
