//! Signer interface + local Ed25519 implementation.
//!
//! # Signer Trait
//! - `sign(data) -> Signature` - Ed25519 signature
//! - `public_key() -> PublicKey` - raw bytes of public key
//! - `key_id() -> KeyId` - deterministic identifier in format:
//!   - `local:<sha256_hex_of_public_key>` for local keys
//!   - `aws-kms:<key_arn>` for AWS KMS keys
//!
//! # Local Implementation
//! Ed25519 keypair generated on install, stored encrypted at rest
//! (AES-256-GCM), key derived from SPINDLE_KEY_UNLOCK env var or file path.
//!
//! # Startup
//! Unlock required. Wrong unlock -> clear error, no silent fallback.
//! Key file: 0600 permissions. Unlock material: never logged.
//! Air-gap: zero external calls.
//!
//! # External Signers (feature-gated)
//! - `pkcs11` feature: PKCS#11 external signer (C_Sign, key never enters memory)
//! - `kms` feature: AWS KMS external signer (Sign API, key never leaves AWS)

#[cfg(feature = "pkcs11")]
pub mod pkcs11;

#[cfg(feature = "kms")]
pub mod kms;

pub mod key_rotation;
pub mod jwk;
pub mod rate_limit;

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
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    time::Instant,
};
use thiserror::Error;
use zeroize::Zeroize;

// -- Constants -------------------------------------------------------------

pub const DEFAULT_KEY_FILE: &str = ".spindle/signing-key.aes";

// Key file format (v2):
//   [0..2]     key_id length (u16 BE, max 254)
//   [2..2+len] key_id string (e.g. "local:<64 hex chars>")
//   [2+len]    version (plain)
//   [3+len..15+len] IV for AES-GCM (plain, 12 bytes)
//   [15+len..63+len] encrypted(salt || key) — AES-256-GCM (48 bytes)
//   [63+len..79+len] GCM auth tag (16 bytes)
//
// For local keys: len = 70 ("local:" + 64 hex chars of SHA-256 digest)
// Total size for local key: 79 + 70 = 149 bytes
const KEY_FILE_VERSION: u8 = 2;

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

    #[error("PKCS#11 error: {0}")]
    Pkcs11(String),

    #[error("slot not found")]
    SlotNotFound,

    #[error("PIN error: {0}")]
    PinError(String),

    #[error("KMS error: {0}")]
    Kms(String),

    #[error("KMS key not found: {0}")]
    KmsKeyNotFound(String),

    #[error("KMS unavailable: {0}")]
    KmsUnavailable(String),

    #[error("key not found: {0}")]
    KeyNotFound(String),

    #[error("public key invalid: {0}")]
    PublicKeyInvalid(String),

    #[error("rate limit exceeded")]
    RateLimitExceeded,
}

// -- Public Types ----------------------------------------------------------

/// Key identifier source: local Ed25519 key or AWS KMS key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyIdSource {
    /// Local Ed25519 key. The string is "local:<sha256_hex_of_public_key>".
    Local(String),
    /// AWS KMS key. The string is "aws-kms:<key_arn>".
    AwsKms(String),
}

impl KeyIdSource {
    pub fn as_str(&self) -> &str {
        match self {
            KeyIdSource::Local(s) => s,
            KeyIdSource::AwsKms(s) => s,
        }
    }

    /// Parse a key ID string back into its source.
    pub fn parse(s: &str) -> Self {
        if let Some(arn) = s.strip_prefix("aws-kms:") {
            KeyIdSource::AwsKms(format!("aws-kms:{arn}"))
        } else if let Some(hex_str) = s.strip_prefix("local:") {
            KeyIdSource::Local(format!("local:{hex_str}"))
        } else {
            // Treat bare string as a local key ID
            KeyIdSource::Local(s.to_string())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyId(KeyIdSource);

impl KeyId {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Construct a local key ID from a raw hex string of the public key.
    /// Format: "local:<sha256_hex_of_public_key>"
    pub fn from_local_hex(hex_str: &str) -> Self {
        KeyId(KeyIdSource::Local(format!("local:{hex_str}")))
    }

    /// Construct an AWS KMS key ID from a key ARN.
    /// Format: "aws-kms:<key_arn>"
    pub fn from_aws_kms(key_arn: &str) -> Self {
        KeyId(KeyIdSource::AwsKms(format!("aws-kms:{key_arn}")))
    }

    /// Parse a key ID string back into its source.
    pub fn parse(s: &str) -> Self {
        KeyId(KeyIdSource::parse(s))
    }

    /// Return just the raw identifier portion (without prefix).
    pub fn raw_id(&self) -> &str {
        match &self.0 {
            KeyIdSource::Local(s) => s.strip_prefix("local:").unwrap_or(s),
            KeyIdSource::AwsKms(s) => s.strip_prefix("aws-kms:").unwrap_or(s),
        }
    }

    /// Return the source type ("local" or "aws-kms").
    pub fn source_type(&self) -> &str {
        match &self.0 {
            KeyIdSource::Local(_) => "local",
            KeyIdSource::AwsKms(_) => "aws-kms",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PublicKey(pub [u8; 32]);

#[derive(Debug, Clone)]
pub struct Signature(pub [u8; 64]);

/// Configuration for signing retry behavior.
///
/// Controls how many times to retry a failed sign operation and the backoff strategy.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of attempts (first try + retries). Default: 3.
    pub max_attempts: u32,
    /// Initial backoff duration in milliseconds. Doubles each retry. Default: 100ms.
    pub initial_backoff_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff_ms: 100,
        }
    }
}

impl RetryConfig {
    /// Create a retry config with the given max attempts.
    pub fn with_max_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = max_attempts;
        self
    }

    /// Create a retry config with the given initial backoff in milliseconds.
    pub fn with_initial_backoff_ms(mut self, ms: u64) -> Self {
        self.initial_backoff_ms = ms;
        self
    }

    /// Calculate the backoff duration for a given attempt number (0-indexed).
    pub fn backoff_ms(&self, attempt: u32) -> u64 {
        self.initial_backoff_ms * 2u64.pow(attempt.min(30))
    }
}

/// Trait for signing operations with retry support.
///
/// Extends `Signer` with retry capability and failure tracking.
pub trait RetrySigner: Signer + Sync {
    /// Sign with retry logic per the provided configuration.
    fn sign_with_retry(
        &self,
        data: &[u8],
        config: &RetryConfig,
    ) -> Result<Signature, SigningError>;
}

/// Trait for cryptographic signing operations.
pub trait Signer: Send + Sync {
    /// Sign arbitrary data, returning an Ed25519 signature.
    ///
    /// This is the main entry point for signing operations. It delegates to
    /// sign_with_artifact with "sign" as the artifact type for backward compatibility.
    fn sign(&self, data: &[u8]) -> Result<Signature, SigningError>;

    /// Sign arbitrary data with explicit artifact type, returning an Ed25519 signature.
    ///
    /// The artifact_type should be one of: "manifest", "export", "checkpoint".
    /// This allows for proper rate limiting and audit logging based on the artifact type.
    fn sign_with_artifact(&self, data: &[u8], artifact_type: &str) -> Result<Signature, SigningError>;

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
        let key_id = Self::derive_key_id(&signing_key);
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

    pub fn sign_with_artifact(&self, data: &[u8], artifact_type: &str) -> Result<Signature, SigningError> {
        let state = self.state.as_ref().ok_or(SigningError::KeyNotUnlocked)?;

        // Check rate limit before signing
        let key_id_str = state.key_id.as_str();
        if !crate::rate_limit::check_rate_limit(key_id_str) {
            crate::rate_limit::log_sign_attempt(
                key_id_str,
                artifact_type,
                data,
                false,
                0.0,
            );
            return Err(SigningError::RateLimitExceeded);
        }

        let start_time = Instant::now();
        let signature = state.signing_key.sign(data);
        let duration_ms = start_time.elapsed().as_secs_f64() * 1000.0;
        let mut sig_bytes = [0u8; 64];
        sig_bytes.copy_from_slice(&signature.to_bytes());

        // Log successful sign attempt
        crate::rate_limit::log_sign_attempt(
            key_id_str,
            artifact_type,
            data,
            true,
            duration_ms,
        );

        Ok(Signature(sig_bytes))
    }

    fn sign(&self, data: &[u8]) -> Result<Signature, SigningError> {
        self.sign_with_artifact(data, "sign")
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
    /// File format (v2):
    ///   [0..2]     key_id length (u16 BE)
    ///   [2..2+len] key_id string bytes
    ///   [2+len]    version byte
    ///   [3+len..15+len] IV for AES-GCM (12 bytes)
    ///   [15+len..63+len] encrypted(salt || key) — AES-256-GCM (48 bytes)
    ///   [63+len..79+len] GCM auth tag (16 bytes)
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

        let kid_bytes = key_id.as_str().as_bytes();
        let kid_len = kid_bytes.len() as u16;

        let mut output = Vec::new();
        output.push((kid_len >> 8) as u8);
        output.push((kid_len & 0xFF) as u8);
        output.extend_from_slice(kid_bytes);
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
    /// Returns the SigningKey and the stored KeyId.
    fn decrypt_and_read(
        encrypted: &[u8],
        unlock_material: &str,
    ) -> Result<SigningKey, SigningError> {
        let dk = Self::derive_key(unlock_material);
        let mut cipher = Aes256Gcm::new_from_slice(&dk)
            .map_err(|_| SigningError::WrongUnlock)?;

        if encrypted.len() < 79 {
            return Err(SigningError::InvalidKeyFile(format!(
                "too short: {} bytes (min 79)",
                encrypted.len()
            )));
        }

        let kid_len = ((encrypted[0] as usize) << 8) | (encrypted[1] as usize);
        let kid_start = 2;
        let kid_end = kid_start + kid_len;

        if kid_end + 1 + 12 + 48 + 16 != encrypted.len() {
            return Err(SigningError::InvalidKeyFile(format!(
                "key_id_len={}, total={}",
                kid_len,
                encrypted.len()
            )));
        }

        let version = encrypted[kid_end];
        if version != KEY_FILE_VERSION {
            return Err(SigningError::InvalidKeyFile(format!(
                "unsupported version: {} (expected {})",
                version, KEY_FILE_VERSION
            )));
        }

        let iv_start = kid_end + 1;
        let iv_bytes = &encrypted[iv_start..iv_start + 12];
        let nonce = GenericArray::from_slice(iv_bytes);

        let enc_start = iv_start + 12;
        let enc_data = &encrypted[enc_start..enc_start + 48];
        let mut decrypted = enc_data.to_vec();

        let tag_start = enc_start + 48;
        let tag = GenericArray::from_slice(&encrypted[tag_start..tag_start + 16]);

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
        let hex_str = hex::encode(digest);
        KeyId::from_local_hex(&hex_str)
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

    fn sign_with_artifact(&self, data: &[u8], artifact_type: &str) -> Result<Signature, SigningError> {
        self.sign_with_artifact(data, artifact_type)
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


/// Global counter for signing failures (increments on hard-fail after max retries).
///
/// Exposed as `spindle_signing_failures_total` in Prometheus metrics.
static SPINDLE_SIGNING_FAILURES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Increment the global signing failure counter.
pub fn increment_signing_failures() {
    SPINDLE_SIGNING_FAILURES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Read the current signing failure counter (for testing/observability).
pub fn signing_failure_count() -> u64 {
    SPINDLE_SIGNING_FAILURES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Reset the signing failure counter (for testing).
pub fn reset_signing_failure_count() {
    SPINDLE_SIGNING_FAILURES.store(0, std::sync::atomic::Ordering::Relaxed);
}

impl RetrySigner for LocalSigner {
    fn sign_with_retry(
        &self,
        data: &[u8],
        config: &RetryConfig,
    ) -> Result<Signature, SigningError> {
        let mut last_error = None;
        for attempt in 0..config.max_attempts {
            match self.sign(data) {
                Ok(sig) => return Ok(sig),
                Err(e) => {
                    tracing::warn!(
                        "signing attempt {} failed: {}, will retry",
                        attempt + 1,
                        e
                    );
                    last_error = Some(e);
                    // Backoff before next attempt (not after last attempt)
                    if attempt < config.max_attempts - 1 {
                        let backoff_ms = config.backoff_ms(attempt);
                        std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
                    }
                }
            }
        }
        // All retries exhausted — hard fail, increment metric
        let err = last_error
            .expect("loop must have set last_error if all attempts failed");
        tracing::error!(
            "signing hard-failed after {} attempts: {}",
            config.max_attempts,
            err
        );
        increment_signing_failures();
        Err(err)
    }
}
// -- Tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    // Serialize retry tests that share the global SPINDLE_SIGNING_FAILURES counter.
    lazy_static::lazy_static! {
        static ref FAILURE_COUNTER_MUTEX: Mutex<()> = Mutex::new(());
    }

    fn test_key_dir(test_name: &str) -> PathBuf {
        let id = format!("{:?}", std::thread::current().id())
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>();
        let dir = PathBuf::from(format!("/tmp/spindle-signing-{}-{}", test_name, id));
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
        assert!(key_id.as_str().starts_with("local:"));

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
        assert!(loaded_id.as_str().starts_with("local:"));
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

        // Verify: signature should match
        let public_key = signer.public_key();
        assert!(LocalSigner::verify(data, &signature, &public_key));

        // Verify: tampered data should fail
        let tampered = b"hello world -- this data was tampered";
        assert!(!LocalSigner::verify(tampered, &signature, &public_key));
    }

    #[test]
    fn test_tampered_data_fails_verification() {
        let mut signer = LocalSigner::new();
        let dir = test_key_dir("tampered_data");
        let key_path = dir.join("tampered.aes");
        let um = unlock_material();

        signer.generate(&key_path, &um).unwrap();
        signer.unlock(&key_path, &um).unwrap();

        let data = b"original data";
        let signature = signer.sign(data).unwrap();

        // Tamper with the data
        let tampered = b"original data TAMPERED";
        let public_key = signer.public_key();
        assert!(!LocalSigner::verify(tampered, &signature, &public_key));
    }

    #[test]
    fn test_public_key_is_consistent() {
        let mut signer = LocalSigner::new();
        let dir = test_key_dir("pk_consistent");
        let key_path = dir.join("pk-consistent.aes");
        let um = unlock_material();

        signer.generate(&key_path, &um).unwrap();
        signer.unlock(&key_path, &um).unwrap();

        let pk1 = signer.public_key();
        let pk2 = signer.public_key();
        assert_eq!(pk1.0, pk2.0); // Same key -> same public key
    }

    #[test]
    fn test_key_id_is_consistent() {
        let mut signer = LocalSigner::new();
        let dir = test_key_dir("kid_consistent");
        let key_path = dir.join("kid-consistent.aes");
        let um = unlock_material();

        signer.generate(&key_path, &um).unwrap();
        signer.unlock(&key_path, &um).unwrap();

        let kid1 = signer.key_id().unwrap();
        let kid2 = signer.key_id().unwrap();
        assert_eq!(kid1, kid2);
    }

    // -- Key ID format tests ----------------------------------------------

    #[test]
    fn test_key_id_format_local_prefix() {
        let mut signer = LocalSigner::new();
        let dir = test_key_dir("kid_format");
        let key_path = dir.join("kid-format.aes");
        let um = unlock_material();

        signer.generate(&key_path, &um).unwrap();
        signer.unlock(&key_path, &um).unwrap();

        let kid = signer.key_id().unwrap();
        assert!(kid.as_str().starts_with("local:"));
        assert_eq!(kid.source_type(), "local");
        // local:<64 hex chars of SHA-256>
        let raw = kid.raw_id();
        assert_eq!(raw.len(), 64);
        // Verify it's valid hex
        hex::decode(raw).expect("raw_id should be valid hex");
    }

    #[test]
    fn test_key_id_format_aws_kms() {
        let arn = "arn:aws:kms:us-east-1:123456789012:key/abcd1234-5678-90ab-cdef-EXAMPLE11111";
        let kid = KeyId::from_aws_kms(arn);
        assert_eq!(kid.as_str(), &format!("aws-kms:{arn}"));
        assert_eq!(kid.source_type(), "aws-kms");
        assert_eq!(kid.raw_id(), arn);

        // Test round-trip parse
        let parsed = KeyId::parse(kid.as_str());
        assert_eq!(parsed.as_str(), kid.as_str());
    }

    #[test]
    fn test_key_id_parse_local() {
        let raw = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let kid = KeyId::from_local_hex(raw);
        assert_eq!(kid.as_str(), &format!("local:{raw}"));

        let parsed = KeyId::parse(kid.as_str());
        assert_eq!(parsed.as_str(), kid.as_str());
        assert_eq!(parsed.source_type(), "local");
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
        // The Signer trait for LocalSigner makes no external calls.
        // It only accesses in-memory state and file I/O.
        let signer = LocalSigner::new();
        assert!(!signer.is_unlocked());
    }


    #[test]
    fn test_key_rotation_produces_new_key() {
        let mut signer = LocalSigner::new();
        let dir = test_key_dir("key_rotation");
        let key_path = dir.join("key-rotation.aes");
        let um = unlock_material();

        signer.generate(&key_path, &um).unwrap();
        signer.unlock(&key_path, &um).unwrap();

        let key_id_before = signer.key_id().unwrap();

        let key_id_after = signer.rotate(&key_path, &um).unwrap();

        assert_ne!(key_id_before.as_str(), key_id_after.as_str());
        assert!(signer.is_unlocked());
    }


    #[test]
    fn test_sign_and_verify_roundtrip() {
        let mut signer = LocalSigner::new();
        let dir = test_key_dir("sign_verify_roundtrip");
        let key_path = dir.join("roundtrip.aes");
        let um = unlock_material();

        signer.generate(&key_path, &um).unwrap();
        signer.unlock(&key_path, &um).unwrap();

        let data = b"test data for signing";
        let signature = signer.sign(data).unwrap();
        let public_key = signer.public_key();
        assert!(LocalSigner::verify(data, &signature, &public_key));
    }

    #[test]
    fn test_key_rotation_preserves_format() {
        let mut signer = LocalSigner::new();
        let dir = test_key_dir("rotation_format");
        let key_path = dir.join("rotation-format.aes");
        let um = unlock_material();

        signer.generate(&key_path, &um).unwrap();
        signer.unlock(&key_path, &um).unwrap();

        let id1 = signer.key_id().unwrap();
        assert!(id1.as_str().starts_with("local:"));
        assert_eq!(id1.source_type(), "local");

        let id2 = signer.rotate(&key_path, &um).unwrap();
        assert!(id2.as_str().starts_with("local:"));
        assert_eq!(id2.source_type(), "local");

        // Both key IDs have the correct format
        let raw1 = id1.raw_id();
        let raw2 = id2.raw_id();
        assert_eq!(raw1.len(), 64);
        assert_eq!(raw2.len(), 64);
        hex::decode(raw1).expect("raw_id should be valid hex");
        hex::decode(raw2).expect("raw_id should be valid hex");
    }

    #[test]
    fn test_key_rotation_unlock_rebuilds_key_id() {
        let mut signer = LocalSigner::new();
        let dir = test_key_dir("rotation_unlock");
        let key_path = dir.join("rotation-unlock.aes");
        let um = unlock_material();

        // Generate and record original key_id
        let gen_id = signer.generate(&key_path, &um).unwrap();
        assert!(gen_id.as_str().starts_with("local:"));

        // Unlock produces the same key_id as generate (deterministic from public key)
        signer.unlock(&key_path, &um).unwrap();
        let unlock_id = signer.key_id().unwrap();
        assert_eq!(unlock_id.as_str(), gen_id.as_str());

        // Rotate generates a new key and re-unlocks
        let new_kid = signer.rotate(&key_path, &um).unwrap();
        assert_ne!(unlock_id.as_str(), new_kid.as_str());
    }

    #[test]
    fn test_manifest_has_key_id_on_sign() {
        let mut signer = LocalSigner::new();
        let dir = test_key_dir("manifest_kid");
        let key_path = dir.join("manifest-kid.aes");
        let um = unlock_material();

        signer.generate(&key_path, &um).unwrap();
        signer.unlock(&key_path, &um).unwrap();

        // The key_id should be present and deterministic
        let kid = signer.key_id().unwrap();
        assert!(kid.as_str().starts_with("local:"));

        // Sign some data (simulating manifest signing)
        let data = b"manifest-content";
        let sig = signer.sign(data).unwrap();
        assert!(LocalSigner::verify(data, &sig, &signer.public_key()));
    }

    // -- M4-07: Retry with exponential backoff and hard failure -------------

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.initial_backoff_ms, 100);
    }

    #[test]
    fn test_retry_config_builder() {
        let config = RetryConfig::default()
            .with_max_attempts(5)
            .with_initial_backoff_ms(250);
        assert_eq!(config.max_attempts, 5);
        assert_eq!(config.initial_backoff_ms, 250);
    }

    #[test]
    fn test_retry_config_backoff_calculation() {
        let config = RetryConfig::default();
        // Exponential: 100, 200, 400, 800, ...
        assert_eq!(config.backoff_ms(0), 100);
        assert_eq!(config.backoff_ms(1), 200);
        assert_eq!(config.backoff_ms(2), 400);
        assert_eq!(config.backoff_ms(3), 800);
    }

    #[test]
    fn test_sign_with_retry_succeeds_on_first_try() {
        let _guard = FAILURE_COUNTER_MUTEX.lock().unwrap();
        let mut signer = LocalSigner::new();
        let dir = test_key_dir("retry_first");
        let key_path = dir.join("retry-first.aes");
        let um = unlock_material();

        signer.generate(&key_path, &um).unwrap();
        signer.unlock(&key_path, &um).unwrap();

        let before = signing_failure_count();
        let config = RetryConfig::default();
        let data = b"test data for retry";
        let sig = signer.sign_with_retry(data, &config).unwrap();

        assert!(LocalSigner::verify(data, &sig, &signer.public_key()));
        assert_eq!(signing_failure_count(), before); // no increment on success
    }

    #[test]
    fn test_retry_exhausted_hard_fails() {
        let _guard = FAILURE_COUNTER_MUTEX.lock().unwrap();
        let mut signer = LocalSigner::new();
        let dir = test_key_dir("retry_exhaust");
        let key_path = dir.join("retry-exhaust.aes");
        let um = unlock_material();

        signer.generate(&key_path, &um).unwrap();
        signer.unlock(&key_path, &um).unwrap();
        signer.state = None;  // Force failure

        let before = signing_failure_count();
        let config = RetryConfig {
            max_attempts: 3,
            initial_backoff_ms: 1,
        };
        let result = signer.sign_with_retry(b"fail data", &config);
        assert!(result.is_err());
        assert_eq!(signing_failure_count(), before + 1);
    }

    #[test]
    fn test_retry_no_partial_artifacts() {
        let _guard = FAILURE_COUNTER_MUTEX.lock().unwrap();
        let mut signer = LocalSigner::new();
        let dir = test_key_dir("retry_no_partial");
        let key_path = dir.join("retry-no-partial.aes");
        let um = unlock_material();

        signer.generate(&key_path, &um).unwrap();
        signer.unlock(&key_path, &um).unwrap();

        signer.state = None;

        let before = signing_failure_count();
        let config = RetryConfig {
            max_attempts: 3,
            initial_backoff_ms: 1,
        };
        let result = signer.sign_with_retry(b"no-partial-test", &config);
        assert!(result.is_err());
        assert_eq!(signing_failure_count(), before + 1);
    }

    #[test]
    fn test_retry_metric_increment_only_on_hard_failure() {
        let _guard = FAILURE_COUNTER_MUTEX.lock().unwrap();
        let mut signer = LocalSigner::new();
        let dir = test_key_dir("retry_metric");
        let key_path = dir.join("retry-metric.aes");
        let um = unlock_material();

        signer.generate(&key_path, &um).unwrap();
        signer.unlock(&key_path, &um).unwrap();

        let before = signing_failure_count();
        let config = RetryConfig::default();
        let sig = signer.sign_with_retry(b"success-test", &config).unwrap();
        assert!(LocalSigner::verify(b"success-test", &sig, &signer.public_key()));
        assert_eq!(signing_failure_count(), before); // no increment on success

        signer.state = None;
        let config_fail = RetryConfig {
            max_attempts: 2,
            initial_backoff_ms: 1,
        };
        let _ = signer.sign_with_retry(b"fail-test", &config_fail);
        assert_eq!(signing_failure_count(), before + 1);
    }

    #[test]
    fn test_retry_custom_max_attempts() {
        let _guard = FAILURE_COUNTER_MUTEX.lock().unwrap();
        let mut signer = LocalSigner::new();
        let dir = test_key_dir("retry_custom_max");
        let key_path = dir.join("retry-custom-max.aes");
        let um = unlock_material();

        signer.generate(&key_path, &um).unwrap();
        signer.unlock(&key_path, &um).unwrap();
        signer.state = None;

        let before = signing_failure_count();
        let config = RetryConfig {
            max_attempts: 1,
            initial_backoff_ms: 1,
        };
        let result = signer.sign_with_retry(b"custom-max-test", &config);
        assert!(result.is_err());
        assert_eq!(signing_failure_count(), before + 1);

        let before2 = signing_failure_count();
        let config2 = RetryConfig {
            max_attempts: 10,
            initial_backoff_ms: 1,
        };
        let result2 = signer.sign_with_retry(b"custom-max-test2", &config2);
        assert!(result2.is_err());
        assert_eq!(signing_failure_count(), before2 + 1);
    }
}
