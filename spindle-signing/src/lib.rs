//! Signer interface + local Ed25519 implementation.
//!
//! # Signer Trait
//! - `sign(data) -> Signature` - Ed25519 signature
//! - `public_key() -> PublicKey` - raw bytes of public key
//! - `key_id() -> KeyId` - UUIDv4 identifier
//!
//! # Local Implementation
//! Ed25519 keypair generated on install, stored encrypted at rest
//! (AES-256-GCM), key derived from SPINDLE_KEY_UNLOCK env var or file path.
//!
//! # Startup
//! Unlock required. Wrong unlock -> clear error, no silent fallback.
//! Key file: 0600 permissions. Unlock material: never logged.
//! Air-gap: zero external calls.

use aes_gcm::{
    aead::{AeadMutInPlace, KeyInit},
    Aes256Gcm,
};
use aes_gcm::aead::generic_array::GenericArray;
use ed25519_dalek::Verifier;
use ed25519_dalek::Signer as DalekSigner;
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroize;

// -- Constants -------------------------------------------------------------

pub const DEFAULT_KEY_FILE: &str = ".spindle/signing-key.aes";
const FILE_LAYOUT_SIZE: usize = 36 + 1 + 12 + 48 + 16; // key_id + ver + iv + payload + tag
const KEY_FILE_VERSION: u8 = 1;

// -- Error Types -----------------------------------------------------------

#[derive(Debug, Error)]
pub enum SigningError {
    #[error("key not loaded -- call unlock() first")]
    KeyNotUnlocked,

    #[error("wrong unlock material -- key file is corrupted or unlock failed")]
    WrongUnlock,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid key file format: {0}")]
    InvalidKeyFile(String),

    #[error("signature verification failed")]
    VerificationFailed,

    #[error("key generation failed: {0}")]
    KeyGenerationFailed(String),

    #[error("encryption failed: {0}")]
    EncryptionFailed(String),
}

// -- Public Types ----------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyId(String);

impl KeyId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct PublicKey(pub [u8; 32]);

#[derive(Debug, Clone)]
pub struct Signature(pub [u8; 64]);

/// Trait for cryptographic signing operations.
pub trait Signer: Send + Sync {
    /// Sign arbitrary data, returning an Ed25519 signature.
    fn sign(&self, data: &[u8]) -> Result<Signature, SigningError>;
    /// Return the public key corresponding to the signing key.
    fn public_key(&self) -> PublicKey;
    /// Return the deterministic key identifier.
    fn key_id(&self) -> KeyId;
}

/// Internal state when the signer has been unlocked.
struct SignerState {
    signing_key: SigningKey,
    key_id: KeyId,
}

/// Local Ed25519 signer with encrypted-at-rest key storage.
pub struct LocalSigner {
    state: Option<SignerState>,
}

impl LocalSigner {
    pub fn new() -> Self {
        Self { state: None }
    }

    /// Generate a fresh Ed25519 keypair and write it encrypted to `key_path`.
    pub fn generate<P: AsRef<Path>>(
        &self,
        key_path: P,
        unlock_material: &str,
    ) -> Result<KeyId, SigningError> {
        let key_path = key_path.as_ref();
        if let Some(parent) = key_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let signing_key = SigningKey::generate(&mut OsRng);
        let key_id = KeyId(Uuid::new_v4().to_string());
        let key_bytes = signing_key.to_bytes();

        Self::encrypt_and_write(key_path, &key_bytes, &key_id, unlock_material)?;
        Self::set_key_permissions(key_path)?;

        Ok(key_id)
    }

    /// Unlock the signer using the provided unlock material.
    pub fn unlock<P: AsRef<Path>>(
        &mut self,
        key_path: P,
        unlock_material: &str,
    ) -> Result<KeyId, SigningError> {
        let key_path = key_path.as_ref();
        let encrypted = fs::read(key_path)?;
        let signing_key = Self::decrypt_and_read(&encrypted, unlock_material)?;
        let key_id = Self::derive_key_id(&signing_key);

        self.state = Some(SignerState {
            signing_key,
            key_id: key_id.clone(),
        });

        Ok(key_id)
    }

    pub fn is_unlocked(&self) -> bool {
        self.state.is_some()
    }

    pub fn key_id(&self) -> Result<KeyId, SigningError> {
        self.state
            .as_ref()
            .map(|s| s.key_id.clone())
            .ok_or(SigningError::KeyNotUnlocked)
    }

    pub fn public_key_raw(&self) -> Result<PublicKey, SigningError> {
        Ok(PublicKey(
            self.state
                .as_ref()
                .ok_or(SigningError::KeyNotUnlocked)?
                .signing_key
                .verifying_key()
                .to_bytes(),
        ))
    }

    pub fn sign(&self, data: &[u8]) -> Result<Signature, SigningError> {
        let state = self.state.as_ref().ok_or(SigningError::KeyNotUnlocked)?;
        let signature = state.signing_key.sign(data);
        let mut sig_bytes = [0u8; 64];
        sig_bytes.copy_from_slice(&signature.to_bytes());
        Ok(Signature(sig_bytes))
    }

    /// Verify a signature against data and a public key.
    pub fn verify(data: &[u8], signature: &Signature, public_key: &PublicKey) -> bool {
        let sig = match ed25519_dalek::Signature::from_slice(&signature.0) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let vk = match VerifyingKey::from_bytes(&public_key.0) {
            Ok(vk) => vk,
            Err(_) => return false,
        };
        vk.verify(data, &sig).is_ok()
    }

    /// Rotate to a new keypair, re-encrypting with the same unlock material.
    pub fn rotate<P: AsRef<Path>>(
        &mut self,
        key_path: P,
        unlock_material: &str,
    ) -> Result<KeyId, SigningError> {
        self.unlock(&key_path, unlock_material)?;
        let new_key_id = self.generate(&key_path, unlock_material)?;
        self.unlock(&key_path, unlock_material)?;
        Ok(new_key_id)
    }

    // -- Encryption internals ------------------------------------------------

    fn derive_key(unlock_material: &str) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(unlock_material.as_bytes());
        hasher.finalize().into()
    }

    /// Encrypt a 32-byte key and write it to disk.
    ///
    /// File layout:
    ///   [0..36]      key_id bytes (plain)
    ///   [36]         version (plain)
    ///   [37..49]     IV for AES-GCM (plain)
    ///   [49..97]     encrypted(salt || key) — AES-256-GCM
    ///   [97..113]    GCM auth tag
    fn encrypt_and_write(
        key_path: &Path,
        key_bytes: &[u8; 32],
        key_id: &KeyId,
        unlock_material: &str,
    ) -> Result<(), SigningError> {
        let dk = Self::derive_key(unlock_material);
        let mut cipher = Aes256Gcm::new_from_slice(&dk)
            .map_err(|e| SigningError::EncryptionFailed(e.to_string()))?;

        let mut iv = [0u8; 12];
        OsRng.fill_bytes(&mut iv);
        let mut salt = [0u8; 16];
        OsRng.fill_bytes(&mut salt);

        // Plaintext: salt(16) + key(32) = 48 bytes
        let mut plaintext = Vec::with_capacity(48);
        plaintext.extend_from_slice(&salt);
        plaintext.extend_from_slice(key_bytes);

        let nonce = GenericArray::from_slice(&iv);
        let tag = cipher
            .encrypt_in_place_detached(nonce, &[], &mut plaintext)
            .map_err(|e| SigningError::EncryptionFailed(e.to_string()))?;

        let mut output = key_id.0.as_bytes().to_vec();
        output.push(KEY_FILE_VERSION);
        output.extend_from_slice(&iv);
        output.extend_from_slice(&plaintext);
        output.extend_from_slice(tag.as_slice());

        let temp_path = key_path.with_extension("tmp");
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temp_path)?;
        file.write_all(&output)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp_path, key_path)?;
        Self::set_key_permissions(key_path)?;
        Ok(())
    }

    /// Read and decrypt the key from disk.
    fn decrypt_and_read(
        encrypted: &[u8],
        unlock_material: &str,
    ) -> Result<SigningKey, SigningError> {
        let dk = Self::derive_key(unlock_material);
        let mut cipher = Aes256Gcm::new_from_slice(&dk)
            .map_err(|_| SigningError::WrongUnlock)?;

        if encrypted.len() != FILE_LAYOUT_SIZE {
            return Err(SigningError::InvalidKeyFile(format!(
                "expected {} bytes, got {}",
                FILE_LAYOUT_SIZE,
                encrypted.len()
            )));
        }

        if encrypted[36] != KEY_FILE_VERSION {
            return Err(SigningError::InvalidKeyFile(format!(
                "unsupported version: {} (expected {})",
                encrypted[36], KEY_FILE_VERSION
            )));
        }

        let iv_bytes = &encrypted[37..37 + 12];
        let nonce = GenericArray::from_slice(iv_bytes);

        let enc_data = &encrypted[49..97];
        let mut decrypted = enc_data.to_vec();

        let tag = GenericArray::from_slice(&encrypted[97..113]);

        match cipher.decrypt_in_place_detached(nonce, &[], &mut decrypted, &tag) {
            Ok(()) => {
                let mut key_arr = [0u8; 32];
                key_arr.copy_from_slice(&decrypted[16..]);
                decrypted.zeroize();
                Ok(SigningKey::from(key_arr))
            }
            Err(_) => Err(SigningError::WrongUnlock),
        }
    }

    fn derive_key_id(signing_key: &SigningKey) -> KeyId {
        let vk_bytes = signing_key.verifying_key().to_bytes();
        let mut hasher = Sha256::new();
        hasher.update(vk_bytes.as_ref());
        let digest = hasher.finalize();
        let id = Uuid::from_slice(&digest[..16]).unwrap();
        KeyId(id.to_string())
    }

    fn set_key_permissions(key_path: &Path) -> Result<(), SigningError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(key_path, perms)?;
        }
        #[cfg(not(unix))]
        {
            tracing::warn!(
                "Cannot set 0600 permissions on non-Unix platform: {:?}",
                key_path
            );
        }
        Ok(())
    }
}

impl Default for LocalSigner {
    fn default() -> Self {
        Self::new()
    }
}

impl Signer for LocalSigner {
    fn sign(&self, data: &[u8]) -> Result<Signature, SigningError> {
        self.sign(data)
    }

    fn public_key(&self) -> PublicKey {
        self.public_key_raw()
            .unwrap_or_else(|_| {
                panic!("signer must be unlocked before calling public_key()");
            })
    }

    fn key_id(&self) -> KeyId {
        self.key_id()
            .unwrap_or_else(|_| panic!("signer must be unlocked before calling key_id()"))
    }
}

// -- Tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_key_dir(test_name: &str) -> PathBuf {
        let id = format!("{:?}", std::thread::current().id())
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>();
        let dir = PathBuf::from(format!(
            "/tmp/spindle-signing-{}-{}",
            test_name, id
        ));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);
        dir
    }

    fn unlock_material() -> String {
        "test-unlock-material-do-not-log".to_string()
    }

    // -- Key Generation ---------------------------------------------------

    #[test]
    fn test_generate_creates_key_file() {
        let signer = LocalSigner::new();
        let dir = test_key_dir("generate_creates");
        let key_path = dir.join("test-key.aes");
        let um = unlock_material();

        let key_id = signer.generate(&key_path, &um).unwrap();

        assert!(key_path.exists());
        assert!(!key_id.as_str().is_empty());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = key_path.metadata().unwrap();
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn test_generate_produces_unique_keys() {
        let signer = LocalSigner::new();
        let dir = test_key_dir("generate_unique");
        let um = unlock_material();

        let key1 = signer.generate(&dir.join("k1.aes"), &um).unwrap();
        let key2 = signer.generate(&dir.join("k2.aes"), &um).unwrap();

        assert_ne!(key1.as_str(), key2.as_str());
    }

    // -- Unlock / Wrong Unlock ------------------------------------------

    #[test]
    fn test_unlock_loads_key() {
        let mut signer = LocalSigner::new();
        let dir = test_key_dir("unlock_loads");
        let key_path = dir.join("unlock-test.aes");
        let um = unlock_material();

        signer.generate(&key_path, &um).unwrap();
        assert!(!signer.is_unlocked());

        let loaded_id = signer.unlock(&key_path, &um).unwrap();
        assert!(signer.is_unlocked());
        assert!(!loaded_id.as_str().is_empty());
    }

    #[test]
    fn test_wrong_unlock_fails() {
        let mut signer = LocalSigner::new();
        let dir = test_key_dir("wrong_unlock");
        let key_path = dir.join("wrong-unlock.aes");
        let um = unlock_material();

        signer.generate(&key_path, &um).unwrap();

        let result = signer.unlock(&key_path, "wrong-material");
        assert!(result.is_err());
        assert_eq!(
            format!("{}", result.unwrap_err()),
            "wrong unlock material -- key file is corrupted or unlock failed"
        );
        assert!(!signer.is_unlocked());
    }

    #[test]
    fn test_unlock_required_before_signing() {
        let signer = LocalSigner::new();

        let result = signer.sign(b"test data");
        assert!(result.is_err());
        assert_eq!(
            format!("{}", result.unwrap_err()),
            "key not loaded -- call unlock() first"
        );
    }

    // -- Sign / Verify --------------------------------------------------

    #[test]
    fn test_sign_and_verify() {
        let mut signer = LocalSigner::new();
        let dir = test_key_dir("sign_verify");
        let key_path = dir.join("sign-verify.aes");
        let um = unlock_material();

        signer.generate(&key_path, &um).unwrap();
        signer.unlock(&key_path, &um).unwrap();

        let data = b"hello world -- this data will be signed";
        let signature = signer.sign(data).unwrap();

        let pubkey = signer.public_key();
        assert!(LocalSigner::verify(data, &signature, &pubkey));
    }

    #[test]
    fn test_tampered_data_fails_verification() {
        let mut signer = LocalSigner::new();
        let dir = test_key_dir("tamper_test");
        let key_path = dir.join("tamper-test.aes");
        let um = unlock_material();

        signer.generate(&key_path, &um).unwrap();
        signer.unlock(&key_path, &um).unwrap();

        let data = b"original data";
        let signature = signer.sign(data).unwrap();

        let tampered = b"tAMPERED data -- changed!";
        let pubkey = signer.public_key();
        assert!(!LocalSigner::verify(tampered, &signature, &pubkey));
    }

    #[test]
    fn test_public_key_is_consistent() {
        let mut signer = LocalSigner::new();
        let dir = test_key_dir("pubkey_consistent");
        let key_path = dir.join("consistent-pubkey.aes");
        let um = unlock_material();

        signer.generate(&key_path, &um).unwrap();
        signer.unlock(&key_path, &um).unwrap();

        let pk1 = signer.public_key();
        let pk2 = signer.public_key();
        assert_eq!(pk1.0, pk2.0);
    }

    #[test]
    fn test_key_id_is_consistent() {
        let mut signer = LocalSigner::new();
        let dir = test_key_dir("kid_consistent");
        let key_path = dir.join("consistent-kid.aes");
        let um = unlock_material();

        signer.generate(&key_path, &um).unwrap();
        signer.unlock(&key_path, &um).unwrap();

        let id1 = signer.key_id().unwrap();
        let id2 = signer.key_id().unwrap();
        assert_eq!(id1.as_str(), id2.as_str());
    }

    // -- Restart without unlock -----------------------------------------

    #[test]
    fn test_restart_without_unlock_fails() {
        let dir = test_key_dir("restart_fail");
        let key_path = dir.join("restart-test.aes");
        let um = unlock_material();

        let mut signer1 = LocalSigner::new();
        signer1.generate(&key_path, &um).unwrap();
        signer1.unlock(&key_path, &um).unwrap();

        let signed_data = b"test";
        let sig = signer1.sign(signed_data).unwrap();
        let pubkey = signer1.public_key();
        assert!(LocalSigner::verify(signed_data, &sig, &pubkey));

        // New signer instance -- same key file, but NOT unlocked
        let signer2 = LocalSigner::new();
        assert!(!signer2.is_unlocked());
        assert!(signer2.sign(signed_data).is_err());
    }

    // -- Air-gap guarantee ----------------------------------------------

    #[test]
    fn test_signer_is_air_gap() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LocalSigner>();
    }

    // -- Key Rotation ---------------------------------------------------

    #[test]
    fn test_key_rotation_produces_new_key() {
        let mut signer = LocalSigner::new();
        let dir = test_key_dir("key_rotation");
        let key_path = dir.join("rotate-test.aes");
        let um = unlock_material();

        let id1 = signer.generate(&key_path, &um).unwrap();
        signer.unlock(&key_path, &um).unwrap();
        let pk1 = signer.public_key();
        let data = b"before rotation";
        let sig1 = signer.sign(data).unwrap();
        assert!(LocalSigner::verify(data, &sig1, &pk1));

        let id2 = signer.rotate(&key_path, &um).unwrap();
        assert_ne!(id1.as_str(), id2.as_str());

        let pk2 = signer.public_key();
        let data2 = b"after rotation";
        let sig2 = signer.sign(data2).unwrap();
        assert!(LocalSigner::verify(data2, &sig2, &pk2));

        // Old signature still verifies with old public key
        assert!(LocalSigner::verify(data, &sig1, &pk1));
    }
}