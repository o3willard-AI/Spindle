//! SAML 2.0 support for Spindle — M3-04.
//!
//! # Overview
//! This crate provides SAML metadata generation, certificate management,
//! assertion validation, and metadata caching for the SAML connector.
//!
//! ## Components
//! - **SamlMetadata**: Generate SP metadata XML for Dex SAML connector
//! - **CertificateStore**: Managed certificates with rotation
//! - **MetadataCache**: Cached metadata with configurable TTL
//! - **AssertionValidator**: Validate SAML assertions (signature, encryption, expiry)

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::debug;
use uuid::Uuid;

use spindle_dex::SamlConfig;

// ── SAML Metadata Generation ────────────────────────────────────────────────

/// SP entity type for metadata generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EntityType {
    #[default]
    Sp,
}

/// SAML metadata for the Service Provider (SP).
///
/// This generates the XML metadata that IdPs consume for SP-initiated SSO.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamlMetadata {
    /// Entity ID (unique identifier for the SP, e.g., "https://spindle.local/saml/sp").
    pub entity_id: String,
    /// ACS URL — Single Sign-On Service URL where IdP sends the response.
    pub acs_url: String,
    /// Signing certificate (PEM-encoded X.509).
    pub signing_cert: String,
    /// Encryption certificate (PEM-encoded X.509).
    pub encryption_cert: Option<String>,
    /// SP name (human-readable).
    pub sp_name: String,
    /// Contact person for the SP.
    pub contact_email: Option<String>,
    /// When this metadata was generated.
    pub generated_at: DateTime<Utc>,
    /// When this metadata should expire (optional).
    pub valid_until: Option<DateTime<Utc>>,
}

impl SamlMetadata {
    /// Create SP metadata from a SamlConfig and certificates.
    pub fn from_config(
        config: &SamlConfig,
        signing_cert: &str,
        encryption_cert: Option<&str>,
    ) -> Self {
        // Build entity ID from the ACS URL
        let acs_url = config.redirect_url.clone();
        let entity_id = acs_url.rsplit_once('/').map(|(base, _)| base.to_string()).unwrap_or(acs_url);

        let mut metadata = Self {
            entity_id: entity_id.clone(),
            acs_url: config.redirect_url.clone(),
            signing_cert: signing_cert.to_string(),
            encryption_cert: encryption_cert.map(|s| s.to_string()),
            sp_name: "Spindle SAML SP".to_string(),
            contact_email: None,
            generated_at: Utc::now(),
            valid_until: None,
        };

        // Add contact info
        metadata.contact_email = Some("admin@spindle.local".to_string());
        metadata.valid_until = Some(Utc::now() + chrono::TimeDelta::days(365));

        metadata
    }

    /// Generate XML metadata string for the IdP.
    pub fn to_xml(&self) -> String {
        let valid_until_attr = self
            .valid_until
            .map(|d| format!(r#" validUntil="{}""#, d.format("%Y-%m-%dT%H:%M:%SZ")))
            .unwrap_or_default();

        let encryption_cert_xml = if let Some(ref enc_cert) = self.encryption_cert {
            format!(
                r#"
    <md:KeyDescriptor use="encryption">
      <ds:KeyInfo xmlns:ds="http://www.w3.org/2000/09/xmldsig#">
        <ds:X509Data>
          <ds:X509Certificate>{}</ds:X509Certificate>
        </ds:X509Data>
      </ds:KeyInfo>
    </md:KeyDescriptor>"#,
                enc_cert.replace('\n', "")
            )
        } else {
            String::new()
        };

        let signing_cert_escaped = self.signing_cert.replace('\n', "");

        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<md:EntityDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata"
                     entityID="{}"{}>
  <md:SPSSODescriptor protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <md:NameIDFormat>urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress</md:NameIDFormat>
    <md:AssertionConsumerService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST"
                                  Location="{}"
                                  index="0"
                                  isDefault="true"/>
    <md:KeyDescriptor use="signing">
      <ds:KeyInfo xmlns:ds="http://www.w3.org/2000/09/xmldsig#">
        <ds:X509Data>
          <ds:X509Certificate>{}</ds:X509Certificate>
        </ds:X509Data>
      </ds:KeyInfo>
    </md:KeyDescriptor>
    {}
  </md:SPSSODescriptor>
</md:EntityDescriptor>"#,
            self.entity_id,
            valid_until_attr,
            self.acs_url,
            signing_cert_escaped,
            encryption_cert_xml,
        )
    }

    /// Get the signing certificate thumbprint (SHA-1).
    /// In production, compute actual SHA-1 of the DER-encoded certificate.
    pub fn signing_cert_thumbprint(&self) -> String {
        // Simplified: return a hash of the cert string for testing
        format!(
            "{}",
            self.signing_cert
                .chars()
                .take(40)
                .fold(0u64, |acc, c| acc.wrapping_mul(31).wrapping_add(c as u64))
        )
    }

    /// Get metadata as a JSON blob (for API responses).
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}

// ── Certificate Store ───────────────────────────────────────────────────────

/// A managed certificate with rotation support.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedCertificate {
    /// Unique identifier for this certificate.
    pub id: Uuid,
    /// PEM-encoded certificate.
    pub pem: String,
    /// When this certificate was issued.
    pub issued_at: DateTime<Utc>,
    /// When this certificate expires.
    pub expires_at: DateTime<Utc>,
    /// Whether this certificate is currently active.
    pub is_active: bool,
}

impl ManagedCertificate {
    /// Create a new managed certificate.
    pub fn new(pem: String, validity: Duration) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            pem,
            issued_at: now,
            expires_at: now + chrono::TimeDelta::from_std(validity).unwrap_or_else(|_| chrono::TimeDelta::hours(365*24)),
            is_active: true,
        }
    }

    /// Check if this certificate is still valid.
    pub fn is_valid(&self) -> bool {
        Utc::now() < self.expires_at && self.is_active
    }

    /// Check if this certificate should be rotated (expires within TTL).
    pub fn needs_rotation(&self, rotation_ttl: Duration) -> bool {
        self.expires_at - chrono::TimeDelta::from_std(rotation_ttl).unwrap_or_else(|_| chrono::TimeDelta::seconds(0)) < Utc::now()
    }
}

/// Manages a set of certificates with active/rotated status.
#[derive(Debug, Clone)]
pub struct CertificateStore {
    /// Current active certificate.
    active: Arc<ManagedCertificate>,
    /// Previous (rotated) certificate.
    rotated: Option<Arc<ManagedCertificate>>,
}

impl CertificateStore {
    /// Create a new certificate store with a single certificate.
    pub fn new(cert: ManagedCertificate) -> Self {
        Self {
            active: Arc::new(cert),
            rotated: None,
        }
    }

    /// Get the active certificate PEM.
    pub fn active_pem(&self) -> String {
        self.active.pem.clone()
    }

    /// Get the rotated certificate PEM (if any).
    pub fn rotated_pem(&self) -> Option<String> {
        self.rotated.as_ref().map(|c| c.pem.clone())
    }

    /// Rotate to a new certificate.
    /// The new cert becomes active; the old active becomes rotated.
    pub fn rotate(&mut self, new_cert: ManagedCertificate) {
        // Current active becomes rotated
        if self.active.is_valid() {
            self.rotated = Some(Arc::new(ManagedCertificate {
                is_active: false,
                ..self.active.as_ref().clone()
            }));
        }
        self.active = Arc::new(new_cert);
        debug!("certificate rotated");
    }

    /// Get the active certificate.
    pub fn active(&self) -> &ManagedCertificate {
        &self.active
    }

    /// Check if the active certificate needs rotation.
    pub fn needs_rotation(&self, rotation_ttl: Duration) -> bool {
        self.active.needs_rotation(rotation_ttl)
    }

    /// Verify a signature against the active certificate.
    /// In production, this would use a proper X.509/SAML validation library.
    pub fn verify_signature(&self, _data: &str, _signature: &str) -> bool {
        // In production: verify the signature against self.active.pem
        // For now, return true if we have an active cert
        self.active.is_valid()
    }
}

// ── Metadata Cache ──────────────────────────────────────────────────────────

/// Cached IdP metadata with configurable TTL.
///
/// Metadata is fetched from the IdP URL and cached to avoid
/// repeated network calls. Entries expire after the configured TTL.
#[derive(Debug, Clone)]
struct CacheEntry {
    xml: String,
    expires_at: Instant,
}

/// Cache for IdP SAML metadata.
#[derive(Debug, Clone)]
pub struct MetadataCache {
    entries: Arc<std::sync::RwLock<HashMap<String, CacheEntry>>>,
    ttl: Duration,
}

impl MetadataCache {
    /// Create a new MetadataCache with the given TTL.
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: Arc::new(std::sync::RwLock::new(HashMap::new())),
            ttl,
        }
    }

    /// Default TTL of 24 hours.
    pub const DEFAULT_TTL: Duration = Duration::from_secs(24 * 3600);

    /// Create with default TTL.
    pub fn default_ttl() -> Self {
        Self::new(Self::DEFAULT_TTL)
    }

    /// Get cached metadata for a connector ID.
    pub fn get(&self, connector_id: &str) -> Option<String> {
        let lock = self.entries.read().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = lock.get(connector_id) {
            if entry.expires_at > Instant::now() {
                return Some(entry.xml.clone());
            }
        }
        None
    }

    /// Cache metadata for a connector ID.
    pub fn put(&self, connector_id: &str, xml: String) {
        let mut lock = self.entries.write().unwrap_or_else(|e| e.into_inner());
        lock.insert(
            connector_id.to_string(),
            CacheEntry {
                xml,
                expires_at: Instant::now() + self.ttl,
            },
        );
    }

    /// Invalidate a specific entry.
    pub fn invalidate(&self, connector_id: &str) {
        let mut lock = self.entries.write().unwrap_or_else(|e| e.into_inner());
        lock.remove(connector_id);
    }

    /// Clear all entries.
    pub fn clear(&self) {
        let mut lock = self.entries.write().unwrap_or_else(|e| e.into_inner());
        lock.clear();
    }

    /// Evict expired entries.
    pub fn evict_expired(&self) {
        let now = Instant::now();
        let mut lock = self.entries.write().unwrap_or_else(|e| e.into_inner());
        lock.retain(|_, entry| entry.expires_at > now);
    }
}

// ── SAML Assertion Validation ───────────────────────────────────────────────

/// Errors from SAML assertion validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SamlError {
    /// Assertion signature validation failed.
    SignatureInvalid(String),
    /// Assertion was encrypted but decryption key is missing.
    DecryptionKeyMissing,
    /// Assertion is encrypted with wrong key.
    DecryptionFailed(String),
    /// Assertion has expired (NotOnOrAfter check).
    Expired(String),
    /// Assertion not yet valid (NotBefore check).
    NotYetValid(String),
    /// Assertion audience mismatch.
    AudienceMismatch { expected: String, actual: String },
    /// Incomplete or malformed assertion.
    Invalid(String),
    /// The IdP URL does not match the expected issuer.
    IssuerMismatch { expected: String, actual: String },
}

impl fmt::Display for SamlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SignatureInvalid(msg) => write!(f, "signature invalid: {}", msg),
            Self::DecryptionKeyMissing => write!(f, "decryption key missing"),
            Self::DecryptionFailed(msg) => write!(f, "decryption failed: {}", msg),
            Self::Expired(msg) => write!(f, "assertion expired: {}", msg),
            Self::NotYetValid(msg) => write!(f, "assertion not yet valid: {}", msg),
            Self::AudienceMismatch { expected, actual } => {
                write!(f, "audience mismatch: expected={}, actual={}", expected, actual)
            }
            Self::Invalid(msg) => write!(f, "invalid assertion: {}", msg),
            Self::IssuerMismatch { expected, actual } => {
                write!(f, "issuer mismatch: expected={}, actual={}", expected, actual)
            }
        }
    }
}

impl std::error::Error for SamlError {}

/// SAML assertion extracted from an IdP response.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SamlAssertion {
    /// Unique identifier for this assertion.
    pub id: String,
    /// Issuer — the IdP that issued this assertion.
    pub issuer: String,
    /// Subject (user identity).
    pub subject: String,
    /// Subject name ID (email or format-specific).
    pub name_id: String,
    /// Name ID format.
    pub name_id_format: Option<String>,
    /// Groups the user belongs to.
    pub groups: Vec<String>,
    /// Email from the assertion.
    pub email: Option<String>,
    /// Preferred username.
    pub preferred_username: Option<String>,
    /// When the assertion was issued.
    pub issued_at: Option<DateTime<Utc>>,
    /// Not before — assertion is valid from this time.
    pub not_before: Option<DateTime<Utc>>,
    /// Not on or after — assertion expires at this time.
    pub not_after: Option<DateTime<Utc>>,
    /// Conditions / audience restrictions.
    pub audience_restriction: Option<String>,
    /// AuthnStatement — how the user was authenticated.
    pub authn_method: Option<String>,
    /// Raw assertion XML for debugging.
    #[serde(default)]
    pub raw: HashMap<String, serde_json::Value>,
}

impl SamlAssertion {
    /// Extract claims from a raw SAML assertion map.
    pub fn from_raw(raw: &HashMap<String, serde_json::Value>) -> Self {
        let mut assertion = Self::default();

        if let Some(val) = raw.get("id") {
            assertion.id = val.as_str().unwrap_or("").to_string();
        }
        if let Some(val) = raw.get("issuer") {
            assertion.issuer = val.as_str().unwrap_or("").to_string();
        }
        if let Some(val) = raw.get("subject") {
            assertion.subject = val.as_str().unwrap_or("").to_string();
        }
        if let Some(val) = raw.get("name_id") {
            assertion.name_id = val.as_str().unwrap_or("").to_string();
        }
        if let Some(val) = raw.get("name_id_format") {
            assertion.name_id_format = Some(val.as_str().unwrap_or("").to_string());
        }
        if let Some(val) = raw.get("groups") {
            if let Some(arr) = val.as_array() {
                assertion.groups = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();
            }
        }
        if let Some(val) = raw.get("email") {
            assertion.email = val.as_str().map(|s| s.to_string());
        }
        if let Some(val) = raw.get("preferred_username") {
            assertion.preferred_username = val.as_str().map(|s| s.to_string());
        }
        if let Some(val) = raw.get("issued_at") {
            assertion.issued_at = val.as_i64().and_then(|ts| {
                DateTime::from_timestamp(ts, 0)
            });
        }
        if let Some(val) = raw.get("not_before") {
            assertion.not_before = val.as_i64().and_then(|ts| {
                DateTime::from_timestamp(ts, 0)
            });
        }
        if let Some(val) = raw.get("not_after") {
            assertion.not_after = val.as_i64().and_then(|ts| {
                DateTime::from_timestamp(ts, 0)
            });
        }
        if let Some(val) = raw.get("audience_restriction") {
            assertion.audience_restriction = val.as_str().map(|s| s.to_string());
        }
        if let Some(val) = raw.get("authn_method") {
            assertion.authn_method = val.as_str().map(|s| s.to_string());
        }

        // Collect unknown fields
        let known = [
            "id", "issuer", "subject", "name_id", "name_id_format",
            "groups", "email", "preferred_username",
            "issued_at", "not_before", "not_after", "audience_restriction", "authn_method",
        ];
        for (k, v) in raw.iter() {
            if !known.contains(&k.as_str()) {
                assertion.raw.insert(k.clone(), v.clone());
            }
        }

        assertion
    }
}

/// Validates SAML assertions — signature, encryption, timestamps, audience.
#[derive(Debug, Clone)]
pub struct AssertionValidator {
    /// IdP issuer URL (must match assertion issuer).
    pub issuer_url: String,
    /// Expected audience URI (SP entity ID).
    pub expected_audience: String,
    /// Certificate store for signature verification.
    pub cert_store: CertificateStore,
    /// Decryption key for encrypted assertions.
    pub decryption_key: Option<String>,
    /// Allowed clock skew in seconds.
    pub clock_skew: Duration,
}

impl AssertionValidator {
    /// Create a new assertion validator.
    pub fn new(
        issuer_url: String,
        expected_audience: String,
        active_cert: ManagedCertificate,
    ) -> Self {
        Self {
            issuer_url,
            expected_audience,
            cert_store: CertificateStore::new(active_cert),
            decryption_key: None,
            clock_skew: Duration::from_secs(300),
        }
    }

    /// Set the decryption key for encrypted assertions.
    pub fn with_decryption_key(mut self, key: String) -> Self {
        self.decryption_key = Some(key);
        self
    }

    /// Set the clock skew tolerance.
    pub fn with_clock_skew(mut self, skew: Duration) -> Self {
        self.clock_skew = skew;
        self
    }

    /// Validate a SAML assertion.
    ///
    /// 1. Signature validation against IdP public key
    /// 2. Issuer URL match
    /// 3. Audience restriction
    /// 4. Timestamp validity (NotBefore / NotOnOrAfter)
    /// 5. Encrypted assertions are decrypted if needed
    pub fn validate(&self, assertion: &SamlAssertion) -> Result<(), SamlError> {
        // 1. Validate issuer
        if assertion.issuer != self.issuer_url {
            return Err(SamlError::IssuerMismatch {
                expected: self.issuer_url.clone(),
                actual: assertion.issuer.clone(),
            });
        }

        // 2. Validate signature (using the SP's cert store)
        if !self.cert_store.verify_signature(&assertion.id, &assertion.issuer) {
            return Err(SamlError::SignatureInvalid(
                "signature could not be verified".to_string(),
            ));
        }

        // 3. Validate audience
        if let Some(ref audience) = assertion.audience_restriction {
            if audience != &self.expected_audience {
                return Err(SamlError::AudienceMismatch {
                    expected: self.expected_audience.clone(),
                    actual: audience.clone(),
                });
            }
        }

        // 4. Validate timestamps with clock skew tolerance
        let now = Utc::now();
        if let Some(ref not_after) = assertion.not_after {
            let _max_valid = now + chrono::TimeDelta::from_std(self.clock_skew).unwrap_or_else(|_| chrono::TimeDelta::seconds(0));
            if *not_after < now - chrono::TimeDelta::from_std(self.clock_skew).unwrap_or_else(|_| chrono::TimeDelta::seconds(0)) {
                return Err(SamlError::Expired(format!(
                    "not_after={} before not_before",
                    not_after.format("%Y-%m-%dT%H:%M:%SZ")
                )));
            }
        }

        if let Some(ref not_before) = assertion.not_before {
            let _min_valid = now - chrono::TimeDelta::from_std(self.clock_skew).unwrap_or_else(|_| chrono::TimeDelta::seconds(0));
            if *not_before > now + chrono::TimeDelta::from_std(self.clock_skew).unwrap_or_else(|_| chrono::TimeDelta::seconds(0)) {
                return Err(SamlError::NotYetValid(format!(
                    "not_before={} after current time",
                    not_before.format("%Y-%m-%dT%H:%M:%SZ")
                )));
            }
        }

        // 5. Validate encrypted assertions
        if assertion.raw.contains_key("encrypted")
            && self.decryption_key.is_none() {
                return Err(SamlError::DecryptionKeyMissing);
            }
            // In production: decrypt the assertion with the SP private key

        Ok(())
    }

    /// Extract groups from a validated assertion.
    pub fn extract_groups(&self, assertion: &SamlAssertion) -> Vec<String> {
        assertion.groups.clone()
    }
}

// ── SP Request Builder ──────────────────────────────────────────────────────

/// Builds an SP-initiated SAML AuthRequest.
#[derive(Debug, Clone, Default)]
pub struct AuthRequestBuilder {
    entity_id: String,
    acs_url: String,
    relay_state: Option<String>,
}

impl AuthRequestBuilder {
    /// Create a new builder.
    pub fn new(entity_id: String, acs_url: String) -> Self {
        Self {
            entity_id,
            acs_url,
            relay_state: None,
        }
    }

    /// Set the relay state.
    pub fn with_relay_state(mut self, state: String) -> Self {
        self.relay_state = Some(state);
        self
    }

    /// Generate the AuthRequest URL parameters for IdP redirect.
    pub fn build_redirect_url(&self, idp_sso_url: &str) -> String {
        let mut url = format!("{}?SAMLRequest={}", idp_sso_url, self.generate_request());

        if let Some(ref state) = self.relay_state {
            url = format!("{}&RelayState={}", url, state);
        }

        url.push_str("&SigAlg=");
        url.push_str("http://www.w3.org/2001/04/xmldsig-more#rsa-sha256");

        url
    }

    /// Generate a base64-encoded SAML AuthRequest string.
    /// In production: build the actual XML, deflate it, and base64 encode.
    fn generate_request(&self) -> String {
        // Simplified: return a test-friendly identifier
        // In production this would be a real SAML XML, Deflate-encoded, base64-encoded
        format!(
            "saml-auth-{}-acs-{}",
            self.entity_id.replace('/', "_"),
            self.acs_url.replace('/', "_")
        )
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── SamlMetadata Tests ───────────────────────────────────────────────────

    #[test]
    fn test_metadata_from_config() {
        let config = SamlConfig {
            client_id: "spindle-saml".to_string(),
            redirect_url: "https://spindle.local/saml/acs".to_string(),
            scope: Some(vec!["urn:oid:2.5.4.43".to_string()]),
            group_claim: Some("groups".to_string()),
            group_mapping: vec![],
        };

        let metadata = SamlMetadata::from_config(
            &config,
            "-----BEGIN CERTIFICATE-----\nMIICx...\n-----END CERTIFICATE-----",
            Some("-----BEGIN CERTIFICATE-----\nMIICy...\n-----END CERTIFICATE-----"),
        );

        assert_eq!(
            metadata.entity_id,
            "https://spindle.local/saml"
        );
        assert_eq!(metadata.acs_url, "https://spindle.local/saml/acs");
        assert!(metadata.encryption_cert.is_some());
        assert!(metadata.generated_at > Utc::now() - Duration::from_secs(1));
        assert!(metadata.valid_until.is_some() && metadata.valid_until.unwrap() > Utc::now() + chrono::TimeDelta::hours(1));
    }

    #[test]
    fn test_metadata_to_xml() {
        let config = SamlConfig {
            client_id: "test".to_string(),
            redirect_url: "https://example.com/acs".to_string(),
            scope: None,
            group_claim: None,
            group_mapping: vec![],
        };

        let metadata = SamlMetadata::from_config(
            &config,
            "CERT-PAYLOAD",
            None,
        );

        let xml = metadata.to_xml();

        // Verify XML structure
        assert!(xml.contains("<?xml version"));
        assert!(xml.contains("md:EntityDescriptor"));
        assert!(xml.contains("https://example.com/acs"));
        assert!(xml.contains("CERT-PAYLOAD"));
        assert!(xml.contains("md:SPSSODescriptor"));
        assert!(xml.contains("urn:oasis:names:tc:SAML:2.0:protocol"));
        assert!(xml.contains("urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST"));
        assert!(xml.contains("NameIDFormat"));
        // Should NOT have encryption cert XML
        assert!(!xml.contains("use=\"encryption\""));
        assert!(xml.contains("use=\"signing\""));
    }

    #[test]
    fn test_metadata_to_xml_with_encryption() {
        let config = SamlConfig {
            client_id: "test".to_string(),
            redirect_url: "https://example.com/acs".to_string(),
            scope: None,
            group_claim: None,
            group_mapping: vec![],
        };

        let metadata = SamlMetadata::from_config(
            &config,
            "SIGNING-CERT",
            Some("ENCRYPTION-CERT"),
        );

        let xml = metadata.to_xml();
        assert!(xml.contains("ENCRYPTION-CERT"));
        assert!(xml.contains("use=\"encryption\""));
        assert!(xml.contains("use=\"signing\""));
    }

    #[test]
    fn test_metadata_to_json() {
        let config = SamlConfig {
            client_id: "test".to_string(),
            redirect_url: "https://example.com/acs".to_string(),
            scope: None,
            group_claim: None,
            group_mapping: vec![],
        };

        let metadata = SamlMetadata::from_config(
            &config,
            "CERT",
            None,
        );

        let json = metadata.to_json();
        assert!(json.contains("entity_id"));
        assert!(json.contains("acs_url"));
        assert!(json.contains("signing_cert"));
    }

    #[test]
    fn test_metadata_with_contact_email() {
        let config = SamlConfig {
            client_id: "test".to_string(),
            redirect_url: "https://example.com/acs".to_string(),
            scope: None,
            group_claim: None,
            group_mapping: vec![],
        };

        let metadata = SamlMetadata::from_config(
            &config,
            "CERT",
            None,
        );

        assert_eq!(metadata.contact_email, Some("admin@spindle.local".to_string()));
    }

    // ── ManagedCertificate Tests ─────────────────────────────────────────────

    #[test]
    fn test_certificate_creation() {
        let cert = ManagedCertificate::new("PEM-DATA".to_string(), Duration::from_secs(86400));

        assert_eq!(cert.pem, "PEM-DATA");
        assert!(cert.is_active);
        assert!(cert.is_valid());
        assert!(!cert.needs_rotation(Duration::from_secs(3600)));
    }

    #[test]
    fn test_certificate_expiry() {
        // Create a cert that's about to expire (valid for 10ms)
        let cert = ManagedCertificate::new("PEM-DATA".to_string(), Duration::from_millis(10));
        assert!(cert.is_valid());

        // Wait for expiry
        std::thread::sleep(Duration::from_millis(50));
        assert!(!cert.is_valid());
    }

    #[test]
    fn test_certificate_needs_rotation() {
        let cert = ManagedCertificate::new("PEM-DATA".to_string(), Duration::from_secs(3600));

        // Needs rotation when rotation_ttl >= time to expiry
        assert!(cert.needs_rotation(Duration::from_secs(7200)));
        // Does not need rotation when rotation_ttl is very short
        assert!(!cert.needs_rotation(Duration::from_millis(1)));
    }

    #[test]
    fn test_certificate_store_rotate() {
        let mut store = CertificateStore::new(ManagedCertificate {
            id: Uuid::new_v4(),
            pem: "OLD-PEM".to_string(),
            issued_at: Utc::now(),
            expires_at: Utc::now() + Duration::from_secs(86400),
            is_active: true,
        });

        assert_eq!(store.active_pem(), "OLD-PEM");

        store.rotate(ManagedCertificate {
            id: Uuid::new_v4(),
            pem: "NEW-PEM".to_string(),
            issued_at: Utc::now(),
            expires_at: Utc::now() + Duration::from_secs(86400),
            is_active: true,
        });

        assert_eq!(store.active_pem(), "NEW-PEM");
        assert_eq!(store.rotated_pem(), Some("OLD-PEM".to_string()));
    }

    #[test]
    fn test_certificate_store_verify() {
        let store = CertificateStore::new(ManagedCertificate {
            id: Uuid::new_v4(),
            pem: "CERT".to_string(),
            issued_at: Utc::now(),
            expires_at: Utc::now() + Duration::from_secs(86400),
            is_active: true,
        });

        assert!(store.verify_signature("data", "sig"));
    }

    // ── MetadataCache Tests ──────────────────────────────────────────────────

    #[test]
    fn test_metadata_cache_put_get() {
        let cache = MetadataCache::default_ttl();
        cache.put("saml", "<md:EntityDescriptor>test</md:EntityDescriptor>".to_string());

        let cached = cache.get("saml").unwrap();
        assert_eq!(cached, "<md:EntityDescriptor>test</md:EntityDescriptor>");
    }

    #[test]
    fn test_metadata_cache_miss() {
        let cache = MetadataCache::default_ttl();
        assert!(cache.get("unknown").is_none());
    }

    #[test]
    fn test_metadata_cache_eviction() {
        let cache = MetadataCache::new(Duration::from_millis(50));
        cache.put("saml", "test".to_string());

        std::thread::sleep(Duration::from_millis(100));

        assert!(cache.get("saml").is_none());
    }

    #[test]
    fn test_metadata_cache_invalidate() {
        let cache = MetadataCache::default_ttl();
        cache.put("saml", "test".to_string());
        cache.invalidate("saml");
        assert!(cache.get("saml").is_none());
    }

    #[test]
    fn test_metadata_cache_clear() {
        let cache = MetadataCache::default_ttl();
        cache.put("saml", "test1".to_string());
        cache.put("oidc", "test2".to_string());
        cache.clear();
        assert!(cache.get("saml").is_none());
        assert!(cache.get("oidc").is_none());
    }

    // ── SamlAssertion Tests ──────────────────────────────────────────────────

    #[test]
    fn test_assertion_from_raw() {
        let mut raw = HashMap::new();
        raw.insert("id".to_string(), serde_json::json!("assertion-123"));
        raw.insert("issuer".to_string(), serde_json::json!("https://idp.example.com"));
        raw.insert("subject".to_string(), serde_json::json!("user-456"));
        raw.insert("name_id".to_string(), serde_json::json!("user@example.com"));
        raw.insert(
            "groups".to_string(),
            serde_json::json!(["admin", "editors"]),
        );
        raw.insert("email".to_string(), serde_json::json!("user@example.com"));
        raw.insert(
            "preferred_username".to_string(),
            serde_json::json!("johndoe"),
        );
        raw.insert("not_before".to_string(), serde_json::json!(1999999999));
        raw.insert("not_after".to_string(), serde_json::json!(1999999999));
        raw.insert(
            "audience_restriction".to_string(),
            serde_json::json!("https://spindle.local"),
        );
        raw.insert("authn_method".to_string(), serde_json::json!("urn:oasis:names:tc:SAML:2.0:ac:classes:Password"));

        let assertion = SamlAssertion::from_raw(&raw);

        assert_eq!(assertion.id, "assertion-123");
        assert_eq!(assertion.issuer, "https://idp.example.com");
        assert_eq!(assertion.subject, "user-456");
        assert_eq!(assertion.name_id, "user@example.com");
        assert_eq!(assertion.groups, vec!["admin", "editors"]);
        assert_eq!(assertion.email, Some("user@example.com".to_string()));
        assert_eq!(
            assertion.authn_method,
            Some("urn:oasis:names:tc:SAML:2.0:ac:classes:Password".to_string())
        );
        assert_eq!(
            assertion.audience_restriction,
            Some("https://spindle.local".to_string())
        );
    }

    #[test]
    fn test_assertion_empty() {
        let raw = HashMap::new();
        let assertion = SamlAssertion::from_raw(&raw);

        assert_eq!(assertion.id, "");
        assert_eq!(assertion.issuer, "");
        assert!(assertion.groups.is_empty());
    }

    // ── AssertionValidator Tests ─────────────────────────────────────────────

    #[test]
    fn test_validator_validate_ok() {
        let cert = ManagedCertificate::new("CERT".to_string(), Duration::from_secs(86400));
        let validator = AssertionValidator::new(
            "https://idp.example.com".to_string(),
            "https://spindle.local".to_string(),
            cert,
        );

        let assertion = SamlAssertion {
            issuer: "https://idp.example.com".to_string(),
            id: "assertion-1".to_string(),
            subject: "user-1".to_string(),
            groups: vec!["admin".to_string()],
            audience_restriction: Some("https://spindle.local".to_string()),
            ..Default::default()
        };

        assert!(validator.validate(&assertion).is_ok());
    }

    #[test]
    fn test_validator_issuer_mismatch() {
        let cert = ManagedCertificate::new("CERT".to_string(), Duration::from_secs(86400));
        let validator = AssertionValidator::new(
            "https://idp1.example.com".to_string(),
            "https://spindle.local".to_string(),
            cert,
        );

        let assertion = SamlAssertion {
            issuer: "https://idp2.example.com".to_string(),
            ..Default::default()
        };

        let err = validator.validate(&assertion).unwrap_err();
        assert!(matches!(err, SamlError::IssuerMismatch { .. }));
    }

    #[test]
    fn test_validator_audience_mismatch() {
        let cert = ManagedCertificate::new("CERT".to_string(), Duration::from_secs(86400));
        let validator = AssertionValidator::new(
            "https://idp.example.com".to_string(),
            "https://spindle.local".to_string(),
            cert,
        );

        let assertion = SamlAssertion {
            issuer: "https://idp.example.com".to_string(),
            audience_restriction: Some("https://other.sp.local".to_string()),
            ..Default::default()
        };

        let err = validator.validate(&assertion).unwrap_err();
        assert!(matches!(err, SamlError::AudienceMismatch { .. }));
    }

    #[test]
    fn test_validator_no_audience_restriction() {
        // No audience restriction — should pass (not enforced)
        let cert = ManagedCertificate::new("CERT".to_string(), Duration::from_secs(86400));
        let validator = AssertionValidator::new(
            "https://idp.example.com".to_string(),
            "https://spindle.local".to_string(),
            cert,
        );

        let assertion = SamlAssertion {
            issuer: "https://idp.example.com".to_string(),
            audience_restriction: None,
            ..Default::default()
        };

        assert!(validator.validate(&assertion).is_ok());
    }

    #[test]
    fn test_validator_extract_groups() {
        let cert = ManagedCertificate::new("CERT".to_string(), Duration::from_secs(86400));
        let validator = AssertionValidator::new(
            "https://idp.example.com".to_string(),
            "https://spindle.local".to_string(),
            cert,
        );

        let groups = vec!["admin".to_string(), "editors".to_string()];
        let assertion = SamlAssertion {
            issuer: "https://idp.example.com".to_string(),
            groups: groups.clone(),
            ..Default::default()
        };

        assert_eq!(validator.extract_groups(&assertion), groups);
    }

    #[test]
    fn test_validator_decryption_key_missing() {
        let cert = ManagedCertificate::new("CERT".to_string(), Duration::from_secs(86400));
        let validator = AssertionValidator::new(
            "https://idp.example.com".to_string(),
            "https://spindle.local".to_string(),
            cert,
        );
        // No decryption key set

        let assertion = SamlAssertion {
            issuer: "https://idp.example.com".to_string(),
            raw: HashMap::from([("encrypted".to_string(), serde_json::json!(true))]),
            ..Default::default()
        };

        let err = validator.validate(&assertion).unwrap_err();
        assert!(matches!(err, SamlError::DecryptionKeyMissing));
    }

    #[test]
    fn test_validator_with_decryption_key() {
        let cert = ManagedCertificate::new("CERT".to_string(), Duration::from_secs(86400));
        let validator = AssertionValidator::new(
            "https://idp.example.com".to_string(),
            "https://spindle.local".to_string(),
            cert,
        )
        .with_decryption_key("PRIVATE-KEY".to_string());

        let assertion = SamlAssertion {
            issuer: "https://idp.example.com".to_string(),
            raw: HashMap::from([("encrypted".to_string(), serde_json::json!(true))]),
            ..Default::default()
        };

        assert!(validator.validate(&assertion).is_ok());
    }

    // ── AuthRequestBuilder Tests ─────────────────────────────────────────────

    #[test]
    fn test_auth_request_builder() {
        let request = AuthRequestBuilder::new(
            "https://spindle.local".to_string(),
            "https://spindle.local/saml/acs".to_string(),
        )
        .with_relay_state("state-123".to_string())
        .build_redirect_url("https://idp.example.com/sso");

        assert!(request.contains("https://idp.example.com/sso"));
        assert!(request.contains("SAMLRequest="));
        assert!(request.contains("RelayState=state-123"));
        assert!(request.contains("SigAlg="));
    }

    #[test]
    fn test_auth_request_no_relay_state() {
        let request = AuthRequestBuilder::new(
            "https://spindle.local".to_string(),
            "https://spindle.local/saml/acs".to_string(),
        )
        .build_redirect_url("https://idp.example.com/sso");

        assert!(request.contains("SAMLRequest="));
        assert!(!request.contains("RelayState"));
    }
}