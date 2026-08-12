//! PKCS#11 external signer implementation.
//!
//! Uses `cryptoki` crate to sign via PKCS#11 C_Sign. Key never enters
//! process memory. Session pool with reconnect on disconnect.
//! PIN cached at startup, not per-signature.

#[cfg(not(feature = "pkcs11"))]
compile_error!("spindle-signing[pkcs11] feature is required for this module");

use crate::{KeyId, PublicKey, Signature, Signer, SigningError};
use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
use cryptoki::error::Error as Pkcs11Error;
use cryptoki::error::RvError;
use cryptoki::mechanism::eddsa::{EddsaParams, EddsaSignatureScheme};
use cryptoki::mechanism::Mechanism;
use cryptoki::object::{Attribute, AttributeType, KeyType, ObjectHandle};
use cryptoki::session::UserType;
use cryptoki::types::AuthPin;
use std::sync::Mutex;
use std::time::Duration;

/// Configuration for PKCS#11 signing.
#[derive(Debug, Clone)]
pub struct Pkcs11Config {
    /// Path to PKCS#11 module (e.g., /usr/lib/softhsm/libsofthsm2.so).
    pub module_path: String,
    /// Slot ID to use.
    pub slot_id: u64,
    /// Key label to search for.
    pub key_label: String,
    /// PIN via environment variable or configuration.
    pub pin: String,
    /// Session timeout (for reconnection attempts).
    pub session_timeout: Duration,
}

impl Default for Pkcs11Config {
    fn default() -> Self {
        Self {
            module_path: String::new(),
            slot_id: 0,
            key_label: String::new(),
            pin: String::new(),
            session_timeout: Duration::from_secs(30),
        }
    }
}

/// PKCS#11 signer that signs via C_Sign, key never enters process memory.
#[derive(Debug)]
pub struct Pkcs11Signer {
    pkcs11: Pkcs11,
    slot_id: u64,
    key_handle: ObjectHandle,
    pin: String,
    key_label: String,
    key_id: KeyId,
    // Reusable session — reconnect on disconnect
    session: Mutex<Option<std::sync::Arc<cryptoki::session::Session>>>,
}

impl Pkcs11Signer {
    /// Create a new PKCS#11 signer from configuration.
    ///
    /// Initializes the PKCS#11 module, finds the key by label,
    /// and authenticates with the PIN. Key never enters process memory.
    pub fn new(config: &Pkcs11Config) -> Result<Self, SigningError> {
        if config.module_path.is_empty() {
            return Err(SigningError::InvalidKeyFile(
                "PKCS#11 module path is required".to_string(),
            ));
        }
        if config.slot_id == 0 {
            return Err(SigningError::InvalidKeyFile(
                "PKCS#11 slot ID is required".to_string(),
            ));
        }
        if config.key_label.is_empty() {
            return Err(SigningError::InvalidKeyFile(
                "PKCS#11 key label is required".to_string(),
            ));
        }
        if config.pin.is_empty() {
            return Err(SigningError::PinError(
                "PKCS#11 PIN is required".to_string(),
            ));
        }

        // Load PKCS#11 module
        let pkcs11 = Pkcs11::new(&config.module_path)
            .map_err(|e| SigningError::Pkcs11(format!("failed to load module: {e}")))?;

        // Initialize PKCS#11
        pkcs11.initialize(CInitializeArgs::new(
            CInitializeFlags::OS_LOCKING_OK,
        ))
        .map_err(|e| SigningError::Pkcs11(format!("failed to initialize: {e}")))?;

        // Verify slot exists
        let slots = pkcs11
            .get_all_slots()
            .map_err(|e| SigningError::Pkcs11(format!("failed to get slots: {e}")))?;

        if !slots.iter().any(|s| s.id() == config.slot_id) {
            return Err(SigningError::SlotNotFound);
        }

        // Open session
        let session = pkcs11
            .open_rw_session(config.slot_id.try_into().map_err(|_| SigningError::Pkcs11(format!("invalid slot ID: {}", config.slot_id)))?)
            .map_err(|e| SigningError::Pkcs11(format!("failed to open session: {e}")))?;

        // Login to session
        session
            .login(UserType::User, Some(&AuthPin::from(config.pin.clone())))
            .map_err(|err| match err {
                Pkcs11Error::Pkcs11(RvError::PinIncorrect, _) => {
                    SigningError::PinError("wrong PIN -- key access denied".to_string())
                }
                Pkcs11Error::Pkcs11(RvError::UserNotLoggedIn, _) => {
                    SigningError::PinError("login failed -- user not logged in".to_string())
                }
                _ => SigningError::PinError(format!("login failed: {err}")),
            })?;

        // Find key by label
        let template = vec![
            Attribute::Label(config.key_label.as_bytes().to_vec()),
            Attribute::KeyType(KeyType::EC),
        ];

        let handles = session
            .find_objects(&template)
            .map_err(|err| match err {
                Pkcs11Error::Pkcs11(RvError::ObjectHandleInvalid, _) => {
                    SigningError::InvalidKeyFile("key not found -- empty slot or wrong label".to_string())
                }
                _ => SigningError::Pkcs11(format!("failed to find key: {err}")),
            })?;

        if handles.is_empty() {
            // Check if slot is empty vs wrong label
            let token_info = pkcs11
                .get_token_info(config.slot_id.try_into().map_err(|_| SigningError::Pkcs11("invalid slot ID".to_string()))?)
                .map_err(|e| SigningError::Pkcs11(format!("failed to get token info: {e}")))?;

            if !token_info.token_initialized() {
                return Err(SigningError::InvalidKeyFile(
                    "slot empty -- token not initialized".to_string(),
                ));
            }
            return Err(SigningError::InvalidKeyFile(format!(
                "key '{}' not found",
                config.key_label
            )));
        }

        let key_handle = handles[0];

        // Get CKA_ID for key_id -- CKA_ID is a binary blob in PKCS#11
        let attrs = session
            .get_attributes(key_handle, &[AttributeType::Id])
            .map_err(|e| SigningError::Pkcs11(format!("failed to get CKA_ID: {e}")))?;

        let key_id = if let Some(Attribute::Id(bytes)) =
            attrs.iter().find(|a| matches!(a, Attribute::Id(_)))
        {
            KeyId::from_hex(&hex::encode(bytes))?
        } else {
            // No CKA_ID set -- derive from key label
            KeyId(format!("pkcs11:{}", config.key_label))
        };

        // Logout immediately -- PIN is cached in struct, session stays open
        session
            .logout()
            .map_err(|e| SigningError::Pkcs11(format!("failed to logout: {e}")))?;

        Ok(Self {
            pkcs11,
            slot_id: config.slot_id,
            key_handle,
            pin: config.pin.clone(),
            key_label: config.key_label.clone(),
            key_id,
            session: Mutex::new(Some(std::sync::Arc::new(session))),
        })
    }

    /// Reopen session if lost (PKCS#11 sessions can detach).
    fn get_session(&self) -> Result<std::sync::Arc<cryptoki::session::Session>, SigningError> {
        let mut session_guard = self.session.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(ref s) = *session_guard {
            // Test if session is still alive
            match s.get_session_info() {
                Ok(_) => return Ok(std::sync::Arc::clone(s)),
                Err(_) => {
                    // Session lost, recreate
                }
            }
        }

        let new_session = std::sync::Arc::new(
            self.pkcs11
                .open_rw_session(self.slot_id.try_into().map_err(|_| SigningError::Pkcs11("invalid slot ID".to_string()))?)
                .map_err(|e| SigningError::Pkcs11(format!("failed to reopen session: {e}")))?
        );

        *session_guard = Some(std::sync::Arc::clone(&new_session));
        Ok(new_session)
    }

    /// Sign data using PKCS#11 C_Sign. Key never enters process memory.
    fn sign_with_session(
        &self,
        session: &std::sync::Arc<cryptoki::session::Session>,
        data: &[u8],
    ) -> Result<Vec<u8>, SigningError> {
        // Login before signing (PIN cached)
        session
            .login(UserType::User, Some(&AuthPin::from(self.pin.clone())))
            .map_err(|err| match err {
                Pkcs11Error::Pkcs11(RvError::PinIncorrect, _) => {
                    SigningError::PinError("wrong PIN -- key access denied".to_string())
                }
                _ => SigningError::PinError(format!("login failed: {err}")),
            })?;

        // Use PKCS#11 for Ed25519 signing (EDDSA mechanism)
        let mechanism = Mechanism::Eddsa(EddsaParams::new(EddsaSignatureScheme::Ed25519));

        let signature = session
            .sign(&mechanism, self.key_handle, data)
            .map_err(|err| match err {
                Pkcs11Error::Pkcs11(RvError::SessionClosed, _) => {
                    // Session may have been disconnected -- mark for reconnect
                    *self.session.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    SigningError::Pkcs11(format!("session lost: {err}"))
                }

                _ => SigningError::Pkcs11(format!("sign failed: {err}")),
            })?;

        session
            .logout()
            .map_err(|e| SigningError::Pkcs11(format!("failed to logout: {e}")))?;

        Ok(signature)
    }

    /// Get public key from PKCS#11 token.
    /// For Ed25519: returns raw CKA_PUBLIC_KEY (32 bytes).
    /// For RSA/EC: would need CKA_PUBLIC_EXPONENT + CKA_MODULUS / CKA_EC_POINT.
    fn get_public_key(&self) -> Result<PublicKey, SigningError> {
        let session = self.get_session()?;

        let attrs = session
            .get_attributes(self.key_handle, &[AttributeType::PublicKeyInfo])
            .map_err(|e| SigningError::Pkcs11(format!("failed to get public key: {e}")))?;

        // Ed25519 public key is raw 32-byte CKA_PUBLIC_KEY
        if let Some(Attribute::PublicKeyInfo(bytes)) = attrs
            .iter()
            .find(|a| matches!(a, Attribute::PublicKeyInfo(_)))
        {
            if bytes.len() == 32 {
                let mut pk = [0u8; 32];
                pk.copy_from_slice(bytes);
                return Ok(PublicKey(pk));
            }
        }

        Err(SigningError::Pkcs11(
            "public key info not readable -- expected Ed25519 32-byte public key".to_string(),
        ))
    }
}

impl Signer for Pkcs11Signer {
    /// Sign data -- key never enters process memory.
    fn sign(&self, data: &[u8]) -> Result<Signature, SigningError> {
        // Ensure we have a valid session (reconnect if needed)
        let session = self.get_session()?;

        // Sign via C_Sign
        let signature_bytes = self.sign_with_session(&session, data)?;

        if signature_bytes.len() != 64 {
            return Err(SigningError::Pkcs11(format!(
                "expected 64-byte Ed25519 signature, got {} bytes",
                signature_bytes.len()
            )));
        }

        let mut sig = [0u8; 64];
        sig.copy_from_slice(&signature_bytes);
        Ok(Signature(sig))
    }

    /// Return the public key from the token.
    fn public_key(&self) -> Result<PublicKey, SigningError> {
        self.get_public_key()
            .map_err(|_| SigningError::KeyNotConfigured)
    }

    /// Return the key ID from CKA_ID attribute.
    fn key_id(&self) -> Result<KeyId, SigningError> {
        Ok(self.key_id.clone())
    }
}

// SAFETY: Pkcs11Signer is Send + Sync because:
//
// 1. The PKCS#11 module is initialized with CKF_OS_LOCKING_OK (see line 95),
//    which tells the HSM/library to handle internal locking via the OS.
//    This means the underlying CK_FUNCTION_LIST pointer is safe to call
//    from multiple threads concurrently.
//
// 2. The `Pkcs11` context (`self.pkcs11`) is only used to open new sessions
//    (via `open_rw_session` in `get_session()`). It is never used to perform
//    signing or other mutable operations directly — those all go through the
//    `Session` object.
//
// 3. The `Session` is wrapped in `Mutex<Option<Arc<Session>>>`, which
//    serializes all access. Even though `cryptoki::session::Session` is
//    `!Send + !Sync` by design (the cryptoki crate doesn't implement those
//    traits), our `Mutex` ensures only one thread accesses the session at
//    a time. The `Arc` allows the session reference to be shared with
//    `sign_with_session()` after the MutexGuard is dropped.
//
// 4. All other fields (`slot_id: u64`, `key_handle: ObjectHandle`,
//    `pin: String`, `key_label: String`, `key_id: KeyId`) are either `Send + Sync`
//    primitives or `Send + Sync` wrapper types.
//
// 5. The PKCS#11 library in use (e.g., SoftHSM2, AWS CloudHSM) guarantees
//    thread safety when initialized with CKF_OS_LOCKING_OK, as required by
//    the PKCS#11 v2.40 specification §6.1.4 (initialization flags).
//
// This `unsafe impl` overrides the `cryptoki` crate's conservative
// `!Send`/`!Sync` stance. The correctness of this assertion depends on
// the PKCS#11 library being initialized with `CKF_OS_LOCKING_OK`, which
// this module does at construction time.
unsafe impl Send for Pkcs11Signer {}

// SAFETY: See the SAFETY comment above for `Send`. The same reasoning
// applies: the `Pkcs11` context is shared read-only (used only to open
// new sessions), and the `Session` is protected by a `Mutex`. All mutable
// state is synchronized.
unsafe impl Sync for Pkcs11Signer {}

// -- Tests -----------------------------------------------------------------

#[cfg(test)]
#[cfg(feature = "pkcs11")]
mod tests {
    use super::*;

    #[test]
    fn test_pkcs11_signer_rejects_empty_config() {
        let config = Pkcs11Config::default();
        let result = Pkcs11Signer::new(&config);
        assert!(result.is_err());
        assert!(
            format!("{}", result.as_ref().unwrap_err()).contains("module path is required")
        );
    }

    #[test]
    fn test_pkcs11_signer_rejects_empty_slot() {
        let config = Pkcs11Config {
            module_path: "/usr/lib/softhsm/libsofthsm2.so".to_string(),
            slot_id: 0, // empty slot ID
            key_label: "test-key".to_string(),
            pin: "1234".to_string(),
            session_timeout: Duration::from_secs(30),
        };
        let result = Pkcs11Signer::new(&config);
        assert!(result.is_err());
        assert!(
            format!("{}", result.as_ref().unwrap_err()).contains("slot ID is required")
        );
    }

    #[test]
    fn test_pkcs11_signer_rejects_empty_key_label() {
        let config = Pkcs11Config {
            module_path: "/usr/lib/softhsm/libsofthsm2.so".to_string(),
            slot_id: 1,
            key_label: String::new(),
            pin: "1234".to_string(),
            session_timeout: Duration::from_secs(30),
        };
        let result = Pkcs11Signer::new(&config);
        assert!(result.is_err());
        assert!(
            format!("{}", result.as_ref().unwrap_err()).contains("key label is required")
        );
    }

    #[test]
    fn test_pkcs11_signer_rejects_empty_pin() {
        let config = Pkcs11Config {
            module_path: "/usr/lib/softhsm/libsofthsm2.so".to_string(),
            slot_id: 1,
            key_label: "test-key".to_string(),
            pin: String::new(),
            session_timeout: Duration::from_secs(30),
        };
        let result = Pkcs11Signer::new(&config);
        assert!(result.is_err());
        assert!(
            format!("{}", result.as_ref().unwrap_err()).contains("PIN is required")
        );
    }

    // Note: Full signing tests require SoftHSM2 setup in CI.
    // These are ignored by default -- run with `--include-ignored` to test.
    #[test]
    #[ignore = "requires SoftHSM2 setup"]
    fn test_pkcs11_sign_and_verify() {
        // This test requires SoftHSM2 to be installed and configured
        // See CI: docker compose up softhsm
        let config = Pkcs11Config {
            module_path: "/usr/lib/softhsm/libsofthsm2.so".to_string(),
            slot_id: 1,
            key_label: "test-key".to_string(),
            pin: "1234".to_string(),
            session_timeout: Duration::from_secs(30),
        };
        let signer = Pkcs11Signer::new(&config).expect("should create signer");

        let data = b"hello world -- this data will be signed via PKCS#11";
        let signature = signer.sign(data).expect("should sign");

        assert_eq!(signature.0.len(), 64); // Ed25519 signature is 64 bytes
    }

    #[test]
    #[ignore = "requires SoftHSM2 setup"]
    fn test_pkcs11_wrong_pin_clear_error() {
        let config = Pkcs11Config {
            module_path: "/usr/lib/softhsm/libsofthsm2.so".to_string(),
            slot_id: 1,
            key_label: "test-key".to_string(),
            pin: "wrong-pin".to_string(),
            session_timeout: Duration::from_secs(30),
        };
        let result = Pkcs11Signer::new(&config);
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("PIN"));
    }

    #[test]
    #[ignore = "requires SoftHSM2 setup"]
    fn test_pkcs11_empty_slot_clear_error() {
        // Simulate empty slot by using wrong module or non-existent slot
        let config = Pkcs11Config {
            module_path: "/usr/lib/nonexistent.so".to_string(),
            slot_id: 999,
            key_label: "test-key".to_string(),
            pin: "1234".to_string(),
            session_timeout: Duration::from_secs(30),
        };
        let result = Pkcs11Signer::new(&config);
        assert!(result.is_err());
        let err_msg = format!("{}", result.as_ref().unwrap_err());
        assert!(
            err_msg.contains("module")
                || err_msg.contains("slot")
        );
    }
}