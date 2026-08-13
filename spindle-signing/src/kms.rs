//! AWS KMS external signer implementation.
//!
//! Uses `aws-sdk-kms` crate to sign via the KMS Sign API. Key never leaves
//! AWS KMS. Credential chain: env vars → instance profile → config file.
//! Configurable region. Connection pooling via shared client.
//!
//! Errors:
//! - KmsKeyNotFound → wrong key_id
//! - KmsUnavailable → KMS service down / timeout
//! - Kms(String) → other KMS errors

#[cfg(not(feature = "kms"))]
compile_error!("spindle-signing[kms] feature is required for this module");

use crate::{KeyId, KeyIdSource, PublicKey, Signature, Signer, SigningError};
use aws_sdk_kms::config::Region;
use aws_sdk_kms::primitives::Blob;
use aws_sdk_kms::types::MessageType;
use aws_sdk_kms::Client;
use std::sync::Arc;

/// Configuration for AWS KMS signing.
#[derive(Debug, Clone)]
pub struct KmsConfig {
    /// AWS KMS key ARN or key ID (e.g., "arn:aws:kms:us-east-1:123456789012:key/12345678-1234-1234-1234-123456789012" or "12345678-1234-1234-1234-123456789012").
    pub key_id: String,
    /// AWS region (e.g., "us-east-1").
    pub region: String,
    /// Optional endpoint override (e.g., for localstack: "http://localhost:4566").
    pub endpoint_override: Option<String>,
}

impl KmsConfig {
    pub fn new(key_id: impl Into<String>, region: impl Into<String>) -> Self {
        Self {
            key_id: key_id.into(),
            region: region.into(),
            endpoint_override: None,
        }
    }

    /// Set a custom endpoint (useful for localstack or VPC endpoints).
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint_override = Some(endpoint.into());
        self
    }
}

/// AWS KMS signer that signs via the KMS Sign API.
///
/// The signing key never leaves AWS KMS. The signer holds a reference to
/// an AWS KMS client (Arc-shared for connection pooling) and the key ID.
#[derive(Debug)]
pub struct KmsSigner {
    client: Arc<Client>,
    key_id: String,
    /// Derived key identifier from the KMS key ARN.
    key_id_label: KeyId,
}

impl KmsSigner {
    /// Create a new KMS signer from configuration.
    ///
    /// Uses the standard AWS credential chain:
    /// 1. Environment variables (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, etc.)
    /// 2. Shared credentials file (~/.aws/credentials)
    /// 3. Instance profile credentials (EC2/ECS)
    ///
    /// # Arguments
    /// * `config` — KMS configuration with key_id and region
    ///
    /// # Errors
    /// * `SigningError::InvalidKeyFile` — if key_id is empty
    /// * `SigningError::Kms` — if AWS client creation fails
    pub fn new(config: &KmsConfig) -> Result<Self, SigningError> {
        if config.key_id.is_empty() {
            return Err(SigningError::InvalidKeyFile(
                "KMS key_id is required".to_string(),
            ));
        }
        if config.region.is_empty() {
            return Err(SigningError::InvalidKeyFile(
                "KMS region is required".to_string(),
            ));
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| SigningError::Kms(format!("failed to create runtime: {e}")))?;

        let client = runtime.block_on(Self::build_client(config));

        Ok(Self {
            client: Arc::new(client),
            key_id: config.key_id.clone(),
            key_id_label: KeyId(KeyIdSource::AwsKms(format!(
                "kms:{}",
                &config.key_id[..32.min(config.key_id.len())]
            ))),
        })
    }

    async fn build_client(config: &KmsConfig) -> Client {
        let region = Region::new(config.region.clone());

        let mut builder = aws_sdk_kms::config::Builder::default();
        builder.set_region(Some(region));
        if let Some(ref endpoint) = config.endpoint_override {
            builder.set_endpoint_url(Some(endpoint.clone()));
        }
        let config = builder.build();
        Client::from_conf(config)
    }

    /// Get the underlying AWS KMS client (for testing).
    pub fn client(&self) -> &Arc<Client> {
        &self.client
    }
}

/// Async wrapper around KMS sign operation.
pub async fn kms_sign(client: &Client, key_id: &str, data: &[u8]) -> Result<Vec<u8>, SigningError> {
    let result = client
        .sign()
        .key_id(key_id)
        .message(Blob::new(data.to_vec()))
        .message_type(MessageType::Raw)
        .send()
        .await;

    match result {
        Ok(output) => {
            let signature = output
                .signature()
                .ok_or_else(|| SigningError::Kms("KMS returned no signature".to_string()))?
                .as_ref()
                .to_vec();

            tracing::debug!(
                key_id = %key_id,
                sig_len = signature.len(),
                "signed data via KMS"
            );

            Ok(signature)
        }
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("NotFoundException")
                || err_str.contains("ResourceNotFoundException")
            {
                Err(SigningError::KmsKeyNotFound(format!(
                    "KMS key not found: {err_str}"
                )))
            } else if err_str.contains("ServiceUnavailable")
                || err_str.contains("Throttling")
                || err_str.contains("Timeout")
            {
                Err(SigningError::KmsUnavailable(format!(
                    "KMS unavailable: {err_str}"
                )))
            } else {
                Err(SigningError::Kms(format!("KMS sign failed: {err_str}")))
            }
        }
    }
}

impl Signer for KmsSigner {
    /// Sign data via AWS KMS. Key never leaves AWS.
    fn sign(&self, data: &[u8]) -> Result<Signature, SigningError> {
        self.sign_with_artifact(data, "sign")
    }

    /// Sign data with explicit artifact type via AWS KMS.
    /// Includes rate limiting and audit logging via the shared rate_limit module.
    fn sign_with_artifact(
        &self,
        data: &[u8],
        artifact_type: &str,
    ) -> Result<Signature, SigningError> {
        let key_id_str = self.key_id_label.as_str();

        // Check rate limit before signing
        if !crate::rate_limit::check_rate_limit(key_id_str) {
            crate::rate_limit::log_sign_attempt(key_id_str, artifact_type, data, false, 0.0);
            return Err(SigningError::RateLimitExceeded);
        }

        let start_time = std::time::Instant::now();

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| SigningError::KmsUnavailable(format!("failed to create runtime: {e}")))?;

        let signature_bytes = runtime.block_on(kms_sign(&self.client, &self.key_id, data))?;

        let duration_ms = start_time.elapsed().as_secs_f64() * 1000.0;

        if signature_bytes.len() != 64 {
            crate::rate_limit::log_sign_attempt(
                key_id_str,
                artifact_type,
                data,
                false,
                duration_ms,
            );
            return Err(SigningError::Kms(format!(
                "expected 64-byte Ed25519 signature, got {} bytes",
                signature_bytes.len()
            )));
        }

        let mut sig = [0u8; 64];
        sig.copy_from_slice(&signature_bytes);

        // Log successful sign attempt
        crate::rate_limit::log_sign_attempt(key_id_str, artifact_type, data, true, duration_ms);

        Ok(Signature(sig))
    }

    /// KMS doesn't expose public keys directly via the Sign API.
    /// For Ed25519 keys in KMS, the public key must be retrieved separately
    /// (e.g., via DescribeKey or from a certificate stored in KMS).
    fn public_key(&self) -> Result<PublicKey, SigningError> {
        Err(SigningError::Kms(
            "public_key() not implemented for KMS signer -- retrieve via DescribeKey API"
                .to_string(),
        ))
    }

    /// Return the key identifier from the KMS key ARN.
    fn key_id(&self) -> Result<KeyId, SigningError> {
        Ok(self.key_id_label.clone())
    }
}

// KmsSigner is Send + Sync: Arc<aws_sdk_kms::Client> is Send + Sync,
// and String/KeyId are Send + Sync. The compiler auto-derives these
// traits, so no unsafe impl is needed.

// -- Tests -----------------------------------------------------------------

#[cfg(test)]
#[cfg(feature = "kms")]
mod tests {
    use super::*;

    #[test]
    fn test_kms_signer_rejects_empty_key_id() {
        let config = KmsConfig {
            key_id: String::new(),
            region: "us-east-1".to_string(),
            endpoint_override: None,
        };
        let result = KmsSigner::new(&config);
        assert!(result.is_err());
        assert!(format!("{}", result.as_ref().unwrap_err()).contains("key_id is required"));
    }

    #[test]
    fn test_kms_signer_rejects_empty_region() {
        let config = KmsConfig {
            key_id: "test-key-id".to_string(),
            region: String::new(),
            endpoint_override: None,
        };
        let result = KmsSigner::new(&config);
        assert!(result.is_err());
        assert!(format!("{}", result.as_ref().unwrap_err()).contains("region is required"));
    }

    #[test]
    fn test_kms_signer_key_id_format() {
        let config = KmsConfig {
            key_id: "arn:aws:kms:us-east-1:123456789012:key/12345678-1234-1234-1234-123456789012"
                .to_string(),
            region: "us-east-1".to_string(),
            endpoint_override: None,
        };
        let signer = KmsSigner::new(&config);
        assert!(signer.is_ok());
        let signer = signer.unwrap();
        let kid = signer.key_id().unwrap();
        assert!(kid.as_str().starts_with("kms:"));
    }

    #[test]
    fn test_kms_signer_custom_endpoint() {
        let config = KmsConfig::new("test-key", "us-east-1").with_endpoint("http://localhost:4566");
        let signer = KmsSigner::new(&config);
        assert!(signer.is_ok());
    }

    // Note: Full signing tests require a KMS endpoint (localstack or real AWS).
    // These are ignored by default -- run with `--include-ignored` to test.
    #[test]
    #[ignore = "requires KMS endpoint (localstack or real AWS)"]
    fn test_kms_sign_and_verify() {
        let config =
            KmsConfig::new("test-key-id", "us-east-1").with_endpoint("http://localhost:4566");
        let signer = KmsSigner::new(&config).expect("should create signer");

        let data = b"hello world -- this data will be signed via KMS";
        let signature = signer.sign(data).expect("should sign");

        assert_eq!(signature.0.len(), 64); // Ed25519 signature is 64 bytes
    }

    #[test]
    #[ignore = "requires KMS endpoint (localstack or real AWS)"]
    fn test_kms_key_not_found_error() {
        let config = KmsConfig::new("nonexistent-key-id", "us-east-1")
            .with_endpoint("http://localhost:4566");
        let signer = KmsSigner::new(&config).expect("should create signer");

        let result = signer.sign(b"test");
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("not found") || err_msg.contains("KmsKeyNotFound"));
    }

    #[test]
    #[ignore = "requires KMS endpoint (localstack or real AWS)"]
    fn test_kms_unavailable_error() {
        // Test with invalid endpoint that will timeout
        let config = KmsConfig::new("test-key", "us-east-1").with_endpoint("http://127.0.0.1:1");
        let signer = KmsSigner::new(&config).expect("should create signer");

        let result = signer.sign(b"test");
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        // Should contain unavailable/connection error
        assert!(
            err_msg.contains("unavailable")
                || err_msg.contains("connection")
                || err_msg.contains("timeout")
        );
    }
}
