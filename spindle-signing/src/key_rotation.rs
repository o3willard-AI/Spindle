//! Historical key retention + rotation management.
//!
//! Mirrors the `public_keys` relational table schema in memory:
//! - `key_id TEXT PK`
//! - `public_key BYTEA`
//! - `created_at`
//! - `retired_at` (nullable — absent means currently active)
//!
//! The registry accepts key_id from the caller (derived from the Signer's
//! public key), enabling the `verify()` path to look up by the same key_id
//! stored in manifests and audit entries.
//!
//! Rotation adds a new key with `created_at`, sets `retired_at` on the old
//! key. Retired keys are retained for signature verification.

use crate::{KeyId, PublicKey, Signature, SigningError};
use ed25519_dalek::{Verifier, VerifyingKey};
use std::collections::BTreeMap;
use std::sync::RwLock;
use time::OffsetDateTime;

// -- Types -----------------------------------------------------------------

/// Metadata for a single key in the public_keys registry.
#[derive(Debug, Clone)]
pub struct KeyEntry {
    /// The key identifier (unique PK).
    pub key_id: KeyId,
    /// The Ed25519 public key bytes.
    pub public_key: PublicKey,
    /// When this key was created.
    pub created_at: OffsetDateTime,
    /// When this key was retired (None = currently active).
    pub retired_at: Option<OffsetDateTime>,
}

impl KeyEntry {
    /// Whether this key is currently active (not retired).
    pub fn is_active(&self) -> bool {
        self.retired_at.is_none()
    }
}

/// Error types for key lifecycle operations.
#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("key not found: {0}")]
    KeyNotFound(String),

    #[error("no active key to retire — registry is empty")]
    NoActiveKey,

    #[error("key already retired: {0}")]
    AlreadyRetired(String),

    #[error("rotation aborted: {0}")]
    RotationAborted(String),
}

/// In-memory key registry that manages key lifecycle.
///
/// Thread-safe: RwLock-protected BTreeMap keyed by key_id.
/// Concurrent operations are safe: rotation acquires write lock briefly;
/// sign operations acquire read lock.
///
/// Rotation semantics:
/// 1. New key registered with `created_at = now()`
/// 2. Previous active key gets `retired_at = now()`
/// 3. Old key remains in registry (still verifiable)
/// 4. Audit event logged at each step
pub struct KeyRegistry {
    keys: RwLock<BTreeMap<String, KeyEntry>>,
}

impl KeyRegistry {
    /// Create a new empty key registry.
    pub fn new() -> Self {
        Self {
            keys: RwLock::new(BTreeMap::new()),
        }
    }

    /// Register a key in the registry with the given key_id.
    ///
    /// If a previous active key exists, it remains active unless `retire_active`
    /// is true (used during rotation).
    ///
    /// # Arguments
    /// * `key_id` — the authoritative key ID (from the Signer's public key)
    /// * `public_key` — the Ed25519 public key bytes (32 bytes)
    /// * `retire_active` — if true, retire the current active key first
    pub fn register(
        &self,
        key_id: KeyId,
        public_key: PublicKey,
        retire_active: bool,
    ) -> Result<KeyEntry, KeyError> {
        let now = OffsetDateTime::now_utc();
        let mut keys = self.keys.write().map_err(|e| {
            KeyError::RotationAborted(format!("write lock: {e}"))
        })?;

        if public_key.0.len() != 32 {
            return Err(KeyError::RotationAborted(format!(
                "public key must be 32 bytes, got {}",
                public_key.0.len()
            )));
        }

        // Retire current active key if requested
        if retire_active {
            if let Some(active) = keys.values_mut().find(|e| e.is_active()) {
                active.retired_at = Some(now);
                tracing::info!(key_id = %active.key_id.as_str(), "retired active key");
            } else {
                return Err(KeyError::NoActiveKey);
            }
        }

        let entry = KeyEntry {
            key_id: key_id.clone(),
            public_key,
            created_at: now,
            retired_at: None,
        };

        keys.insert(key_id.as_str().to_string(), entry.clone());
        tracing::info!(key_id = %key_id.as_str(), "registered new key");

        Ok(entry)
    }

    /// Rotate to a new key: register with new key_id + retire current active.
    pub fn rotate(
        &self,
        new_key_id: KeyId,
        new_public_key: PublicKey,
    ) -> Result<KeyEntry, KeyError> {
        self.register(new_key_id, new_public_key, true)
    }

    /// Look up a key by its ID (retired keys are still accessible).
    pub fn get(&self, key_id: &KeyId) -> Result<KeyEntry, KeyError> {
        let keys = self.keys.read().map_err(|e| {
            KeyError::RotationAborted(format!("read lock: {e}"))
        })?;

        keys.get(key_id.as_str())
            .cloned()
            .ok_or_else(|| KeyError::KeyNotFound(key_id.as_str().to_string()))
    }

    /// Verify an Ed25519 signature against data using a specific key.
    ///
    /// The key may be retired — retired keys are still verifiable.
    pub fn verify(
        &self,
        signature: &Signature,
        data: &[u8],
        key_id: &KeyId,
    ) -> Result<(), SigningError> {
        let entry = self.get(key_id).map_err(|e| match e {
            KeyError::KeyNotFound(ref msg) => SigningError::KeyNotFound(msg.clone()),
            _ => SigningError::InvalidKeyFile(format!("key lookup failed: {e}")),
        })?;

        let verifying_key = VerifyingKey::from_bytes(&entry.public_key.0)
            .map_err(|e| {
                SigningError::InvalidKeyFile(format!("invalid public key bytes: {e}"))
            })?;

        let dalek_sig = ed25519_dalek::Signature::try_from(signature.0.as_ref())
            .map_err(|_| SigningError::VerificationFailed)?;

        verifying_key.verify(data, &dalek_sig)
            .map_err(|_| SigningError::VerificationFailed)
    }

    /// Verify a signature and return the KeyEntry for audit purposes.
    pub fn verify_with_entry(
        &self,
        signature: &Signature,
        data: &[u8],
        key_id: &KeyId,
    ) -> Result<KeyEntry, SigningError> {
        let entry = self.get(key_id).map_err(|e| match e {
            KeyError::KeyNotFound(ref msg) => SigningError::KeyNotFound(msg.clone()),
            _ => SigningError::InvalidKeyFile(format!("key lookup failed: {e}")),
        })?;

        let verifying_key = VerifyingKey::from_bytes(&entry.public_key.0)
            .map_err(|e| {
                SigningError::InvalidKeyFile(format!("invalid public key bytes: {e}"))
            })?;

        let dalek_sig = ed25519_dalek::Signature::try_from(signature.0.as_ref())
            .map_err(|_| SigningError::VerificationFailed)?;

        verifying_key.verify(data, &dalek_sig)
            .map_err(|_| SigningError::VerificationFailed)?;

        Ok(entry)
    }

    /// List all keys (active + retired), sorted by key_id.
    pub fn list_keys(&self) -> Vec<KeyEntry> {
        let guard = match self.keys.read() {
            Ok(g) => g,
            Err(e) => {
                tracing::error!(error = %e, "read lock failed for list_keys");
                return Vec::new();
            }
        };
        guard.values().cloned().collect()
    }

    /// List currently active (non-retired) keys.
    pub fn list_active_keys(&self) -> Vec<KeyEntry> {
        self.list_keys()
            .into_iter()
            .filter(|e| e.is_active())
            .collect()
    }

    /// List retired keys.
    pub fn list_retired_keys(&self) -> Vec<KeyEntry> {
        self.list_keys()
            .into_iter()
            .filter(|e| !e.is_active())
            .collect()
    }

    /// Count of all keys.
    pub fn total_key_count(&self) -> usize {
        self.keys.read().map(|k| k.len()).unwrap_or(0)
    }

    /// Count of currently active keys.
    pub fn active_key_count(&self) -> usize {
        self.list_active_keys().len()
    }
}

unsafe impl Send for KeyRegistry {}
unsafe impl Sync for KeyRegistry {}

// -- Tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LocalSigner;
    use std::fs;

    fn test_key_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "spindle-key-rotation-{}",
            uuid::Uuid::new_v4()
        ));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);
        dir
    }

    fn unlock_material() -> String {
        "test-unlock-material-do-not-log".to_string()
    }

    fn generate_local_key(
        dir: &std::path::Path,
        name: &str,
    ) -> (LocalSigner, KeyId, PublicKey) {
        let mut signer = LocalSigner::new();
        let key_path = dir.join(name);
        let um = unlock_material();
        signer.generate(&key_path, &um).unwrap();
        signer.unlock(&key_path, &um).unwrap();
        let key_id = signer.key_id().unwrap();
        let pub_key = signer.public_key_raw().unwrap();
        (signer, key_id, pub_key)
    }

    #[test]
    fn test_register_active_key() {
        let registry = KeyRegistry::new();
        let kid = KeyId("test-key-id".to_string());
        let pub_key = PublicKey([42u8; 32]);

        let entry = registry.register(kid, pub_key.clone(), false).unwrap();

        assert!(entry.is_active());
        assert!(entry.retired_at.is_none());
        assert_eq!(entry.key_id.as_str(), "test-key-id");
        assert_eq!(registry.active_key_count(), 1);
        assert_eq!(registry.total_key_count(), 1);
    }

    #[test]
    fn test_rotate_new_key() {
        let registry = KeyRegistry::new();
        let dir = test_key_dir("rotate");

        // Generate and register key A
        let (_signer_a, key_id_a, pub_key_a) = generate_local_key(&dir, "key_a");
        let entry_a = registry.register(key_id_a.clone(), pub_key_a.clone(), false).unwrap();
        assert!(entry_a.is_active());

        // Generate and rotate to key B
        let (_signer_b, key_id_b, pub_key_b) = generate_local_key(&dir, "key_b");
        let entry_b = registry.rotate(key_id_b, pub_key_b).unwrap();
        assert!(entry_b.is_active());

        // Key A should now be retired
        let entry_a_after = registry.get(&entry_a.key_id).unwrap();
        assert!(!entry_a_after.is_active());
        assert!(entry_a_after.retired_at.is_some());

        // Counts: 2 total, 1 active
        assert_eq!(registry.total_key_count(), 2);
        assert_eq!(registry.active_key_count(), 1);
        assert_eq!(registry.list_retired_keys().len(), 1);
    }

    #[test]
    fn test_rotate_without_active_key_fails() {
        let registry = KeyRegistry::new();
        let kid = KeyId("new".to_string());
        let pub_key = PublicKey([42u8; 32]);

        let result = registry.rotate(kid, pub_key);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), KeyError::NoActiveKey));
    }

    #[test]
    fn test_key_lookup_by_id() {
        let registry = KeyRegistry::new();
        let kid = KeyId("lookup-test".to_string());
        let pub_key = PublicKey([42u8; 32]);

        registry.register(kid.clone(), pub_key.clone(), false).unwrap();

        let lookup = registry.get(&kid).unwrap();
        assert_eq!(lookup.key_id.as_str(), "lookup-test");
        assert_eq!(lookup.public_key.0, pub_key.0);
    }

    #[test]
    fn test_key_not_found() {
        let registry = KeyRegistry::new();
        let fake_key_id = KeyId("nonexistent-key".to_string());

        let result = registry.get(&fake_key_id);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            KeyError::KeyNotFound(_)
        ));
    }

    /// Core M4-05 scenario: sign with key A → rotate to B → verify with A still works.
    #[test]
    fn test_sign_with_key_a_rotate_verify_still_works() {
        let registry = KeyRegistry::new();
        let dir = test_key_dir("rotation_verify");

        // Generate and register key A
        let (signer_a, key_id_a, pub_key_a) = generate_local_key(&dir, "key_a");
        registry.register(key_id_a.clone(), pub_key_a, false).unwrap();

        // Sign data with key A
        let data = b"test data for rotation verify";
        let signature = signer_a.sign(data).unwrap();

        // Rotate to key B
        let (signer_b, key_id_b, pub_key_b) = generate_local_key(&dir, "key_b");
        let _entry_b = registry.rotate(key_id_b.clone(), pub_key_b).unwrap();

        // Verify with key A's key_id — should still work (retired key retained)
        let result = registry.verify(&signature, data, &key_id_a);
        assert!(result.is_ok());

        // Sign data with key B
        let signature_b = signer_b.sign(data).unwrap();

        // Verify with key B's key_id — should work
        let result_b = registry.verify(&signature_b, data, &key_id_b);
        assert!(result_b.is_ok());

        // Verify signature B with key A's key_id — should FAIL
        let result_wrong = registry.verify(&signature_b, data, &key_id_a);
        assert!(result_wrong.is_err());
        assert!(matches!(
            result_wrong.unwrap_err(),
            SigningError::VerificationFailed
        ));
    }

    #[test]
    fn test_verify_with_wrong_key_fails() {
        let registry = KeyRegistry::new();
        let dir = test_key_dir("wrong_key_verify");

        let (signer_a, key_id_a, pub_key_a) = generate_local_key(&dir, "wrong_key");
        registry.register(key_id_a.clone(), pub_key_a, false).unwrap();

        let data = b"data signed with key A";
        let signature = signer_a.sign(data).unwrap();

        // Create a second key and register it
        let (_signer_b, key_id_b, pub_key_b) = generate_local_key(&dir, "wrong_key_2");
        registry.register(key_id_b.clone(), pub_key_b, false).unwrap();

        // Verify signature A with key B's key_id — should FAIL
        let result = registry.verify(&signature, data, &key_id_b);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SigningError::VerificationFailed
        ));
    }

    /// Core M4-05 scenario: retired key is still verifiable.
    #[test]
    fn test_retired_key_still_verifiable() {
        let registry = KeyRegistry::new();
        let dir = test_key_dir("retired_verify");

        let (signer_a, key_id_a, pub_key_a) = generate_local_key(&dir, "retired_a");
        registry.register(key_id_a.clone(), pub_key_a, false).unwrap();

        let data = b"retired key data";
        let signature = signer_a.sign(data).unwrap();

        // Rotate to key B (retires A)
        let (_signer_b, key_id_b, pub_key_b) = generate_local_key(&dir, "retired_b");
        registry.rotate(key_id_b, pub_key_b).unwrap();

        // Verify with retired key A — should succeed
        let result = registry.verify(&signature, data, &key_id_a.clone());
        assert!(
            result.is_ok(),
            "retired key should still be verifiable, got: {:?}",
            result
        );

        // List retired keys — should include A
        let retired = registry.list_retired_keys();
        assert!(retired.iter().any(|e| e.key_id.as_str() == key_id_a.as_str()));
    }

    #[test]
    fn test_verify_with_entry_returns_metadata() {
        let registry = KeyRegistry::new();
        let dir = test_key_dir("verify_entry");

        let (signer, key_id, pub_key) = generate_local_key(&dir, "verify_entry");
        let entry = registry.register(key_id.clone(), pub_key, false).unwrap();

        let data = b"verify entry test";
        let signature = signer.sign(data).unwrap();

        let result_entry = registry
            .verify_with_entry(&signature, data, &key_id.clone())
            .unwrap();

        assert_eq!(result_entry.key_id.as_str(), key_id.as_str());
        assert!(result_entry.is_active());
        assert!(result_entry.created_at.le(&OffsetDateTime::now_utc()));
    }

    #[test]
    fn test_multiple_rotations() {
        let registry = KeyRegistry::new();
        let dir = test_key_dir("multi_rotate");

        let (_s1, kid1, pk1) = generate_local_key(&dir, "multi_a");
        registry.register(kid1, pk1, false).unwrap();

        let (_s2, kid2, pk2) = generate_local_key(&dir, "multi_b");
        registry.rotate(kid2, pk2).unwrap();

        let (_s3, kid3, pk3) = generate_local_key(&dir, "multi_c");
        registry.rotate(kid3, pk3).unwrap();

        // Total: 3 keys, 1 active (C), 2 retired (A, B)
        assert_eq!(registry.total_key_count(), 3);
        assert_eq!(registry.active_key_count(), 1);
        assert_eq!(registry.list_retired_keys().len(), 2);
    }

    #[test]
    fn test_list_keys_sorted() {
        let registry = KeyRegistry::new();
        let dir = test_key_dir("list_sorted");

        let (_s1, kid1, pk1) = generate_local_key(&dir, "sorted_1");
        registry.register(kid1.clone(), pk1, false).unwrap();

        let (_s2, kid2, pk2) = generate_local_key(&dir, "sorted_2");
        registry.register(kid2.clone(), pk2, false).unwrap();

        let keys = registry.list_keys();
        assert_eq!(keys.len(), 2);
        assert!(keys[0].key_id.as_str() <= keys[1].key_id.as_str());
    }

    /// Concurrency test: multiple threads rotating simultaneously.
    #[test]
    fn test_concurrent_rotation_safety() {
        use std::sync::Arc;
        use std::thread;

        let registry = Arc::new(KeyRegistry::new());
        let dir = test_key_dir("concurrent");

        let (_s0, kid0, pk0) = generate_local_key(&dir, "concurrent_init");
        registry.register(kid0, pk0, false).unwrap();

        let mut handles = vec![];
        for i in 0..5 {
            let reg = Arc::clone(&registry);
            let dir_clone = dir.clone();
            handles.push(thread::spawn(move || {
                let (_s, kid, pk) = generate_local_key(&dir_clone, &format!("concurrent_{}", i));
                let result = reg.rotate(kid, pk);
                (result.is_ok(), result)
            }));
        }

        let mut successes = 0;
        let mut failures = 0;
        for handle in handles {
            let (ok, result) = handle.join().unwrap();
            if ok {
                successes += 1;
            } else {
                failures += 1;
                // Expected: lock contention means some rotations fail with NoActiveKey
                // (another thread already retired the previous key)
                assert!(matches!(result.unwrap_err(), KeyError::NoActiveKey));
            }
        }

        // Some rotations should succeed (at least 1, at most all 5)
        assert!(successes >= 1);
        assert!(successes <= 5);

        // Total keys = 1 (initial) + successes (rotations)
        assert_eq!(registry.total_key_count(), 1 + successes);
        assert_eq!(registry.active_key_count(), 1);
    }
}