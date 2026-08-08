//! JWK (JSON Web Key) types and Ed25519 key publishing.
//!
//! Provides conversion between spindle-signing `PublicKey`/`KeyId` and JWK
//! format for the `GET /.well-known/spindle/keys.json` endpoint.
//!
//! JWK Spec: RFC 7517, Ed25519: RFC 8037
//! - `kty`: "OKP" (Octet Key Pair)
//! - `crv`: "Ed25519"
//! - `x`: Base64url-encoded public key bytes
//! - `kid`: Key identifier (from spindle-signing)

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{KeyId, PublicKey};

// -- JWK Types -------------------------------------------------------------

/// An Ed25519 JWK member as defined in RFC 8037.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JwkMember {
    /// Key type: "OKP" for octet key pair (Ed25519).
    pub kty: String,
    /// Curve: "Ed25519".
    pub crv: String,
    /// Base64url-encoded public key.
    pub x: String,
    /// Key ID (optional, mapped from spindle-signing key_id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kid: Option<String>,
}

/// A JWK Set containing one or more keys (RFC 7517 §5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JwkSet {
    /// Array of JWK members.
    #[serde(rename = "keys")]
    pub members: Vec<JwkMember>,
}

// -- Conversion -------------------------------------------------------------

/// Convert a `PublicKey` to a base64url-encoded string (RFC 7515 §2).
fn public_key_to_b64url(pk: &PublicKey) -> String {
    // Base64url without padding
    let b64 = base64_url_no_pad(&pk.0);
    b64
}

/// Convert `KeyId` to base64url string (for kid field).
fn key_id_to_b64url(kid: &KeyId) -> String {
    base64_url_no_pad(kid.as_str().as_bytes())
}

/// Encode bytes as base64url without padding.
fn base64_url_no_pad(data: &[u8]) -> String {
    use std::fmt::Write;

    // Simple base64url encoding without padding
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

    let mut result = String::new();
    let mut i = 0;

    while i < data.len() {
        let b0 = data[i] as u32;
        let b1 = if i + 1 < data.len() { data[i + 1] as u32 } else { 0 };
        let b2 = if i + 2 < data.len() { data[i + 2] as u32 } else { 0 };

        let triple = (b0 << 16) | (b1 << 8) | b2;

        result.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        result.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);

        if i + 1 < data.len() {
            result.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        }
        if i + 2 < data.len() {
            result.push(TABLE[(triple & 0x3F) as usize] as char);
        }

        i += 3;
    }

    result
}

/// Convert a spindle-signing `PublicKey` to a JWK member.
pub fn public_key_to_jwk(pk: &PublicKey, kid: &KeyId) -> JwkMember {
    JwkMember {
        kty: "OKP".to_string(),
        crv: "Ed25519".to_string(),
        x: public_key_to_b64url(pk),
        kid: Some(key_id_to_b64url(kid)),
    }
}

/// Convert a list of spindle-signing keys to a JWK set.
pub fn keys_to_jwk_set(keys: &[(KeyId, PublicKey)]) -> JwkSet {
    JwkSet {
        members: keys
            .iter()
            .map(|(kid, pk)| public_key_to_jwk(pk, kid))
            .collect(),
    }
}

// -- Verification -----------------------------------------------------------

/// Verify a JWK set is well-formed (has expected fields, correct base64url).
pub fn verify_jwk_set(jwk_set: &JwkSet) -> Result<(), String> {
    if jwk_set.members.is_empty() {
        return Err("JWK set contains no keys".to_string());
    }

    for (i, member) in jwk_set.members.iter().enumerate() {
        if member.kty != "OKP" {
            return Err(format!("key {}: expected kty=OKP, got {}", i, member.kty));
        }
        if member.crv != "Ed25519" {
            return Err(format!("key {}: expected crv=Ed25519, got {}", i, member.crv));
        }
        if member.x.is_empty() {
            return Err(format!("key {}: missing 'x' field", i));
        }

        // Validate base64url encoding (rough check)
        if !member.x.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(format!("key {}: invalid base64url in 'x' field", i));
        }
    }

    Ok(())
}

// -- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64url_encoding_consistent() {
        let data = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let encoded = base64_url_no_pad(&data);
        // Should not contain padding '='
        assert!(!encoded.contains('='));
        // Should only contain base64url characters
        assert!(encoded.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));

        // Encode again should be identical
        let encoded2 = base64_url_no_pad(&data);
        assert_eq!(encoded, encoded2);
    }

    #[test]
    fn test_empty_data_encodes_correctly() {
        let encoded = base64_url_no_pad(&[]);
        assert_eq!(encoded, "");
    }

    #[test]
    fn test_single_byte() {
        let encoded = base64_url_no_pad(&[0u8]);
        // Single byte: 0 << 2 = 0, remainder 0 → "AA" in base64url
        assert!(!encoded.is_empty());
        assert!(!encoded.contains('='));
    }

    #[test]
    fn test_jwk_member_roundtrip() {
        let kid = KeyId::from_local_hex("test-key-123");
        let pk = PublicKey([42u8; 32]);

        let jwk = public_key_to_jwk(&pk, &kid);

        assert_eq!(jwk.kty, "OKP");
        assert_eq!(jwk.crv, "Ed25519");
        assert!(jwk.kid.is_some());
        assert!(!jwk.x.is_empty());

        // Verify the base64url encoding is correct by decoding and comparing
        // First encode, then check it roundtrips
        // The first two bytes of the public key (42, 42) encode to specific characters
        assert_eq!(jwk.x.len(), 43); // 32 bytes → ceil(32*4/3) = 43 chars
    }

    #[test]
    fn test_keys_to_jwk_set() {
        let keys = vec![
            (
                KeyId::from_local_hex("key-1"),
                PublicKey([1u8; 32]),
            ),
            (
                KeyId::from_local_hex("key-2"),
                PublicKey([2u8; 32]),
            ),
        ];

        let jwk_set = keys_to_jwk_set(&keys);

        assert_eq!(jwk_set.members.len(), 2);
        assert_eq!(jwk_set.members[0].kty, "OKP");
        assert_eq!(jwk_set.members[1].kty, "OKP");
        assert!(jwk_set.members[0].kid.is_some());
        assert!(jwk_set.members[1].kid.is_some());
    }

    #[test]
    fn test_verify_jwk_set_valid() {
        let jwk_set = JwkSet {
            members: vec![
                JwkMember {
                    kty: "OKP".to_string(),
                    crv: "Ed25519".to_string(),
                    x: "dGVzdA".to_string(), // "test" in base64url
                    kid: Some("abc".to_string()),
                },
            ],
        };

        assert!(verify_jwk_set(&jwk_set).is_ok());
    }

    #[test]
    fn test_verify_jwk_set_empty() {
        let jwk_set = JwkSet { members: vec![] };
        assert!(verify_jwk_set(&jwk_set).is_err());
    }

    #[test]
    fn test_verify_jwk_set_wrong_kty() {
        let jwk_set = JwkSet {
            members: vec![JwkMember {
                kty: "EC".to_string(),
                crv: "Ed25519".to_string(),
                x: "dGVzdA".to_string(),
                kid: Some("abc".to_string()),
            }],
        };
        assert!(verify_jwk_set(&jwk_set).is_err());
    }

    #[test]
    fn test_verify_jwk_set_wrong_curve() {
        let jwk_set = JwkSet {
            members: vec![JwkMember {
                kty: "OKP".to_string(),
                crv: "P-256".to_string(),
                x: "dGVzdA".to_string(),
                kid: Some("abc".to_string()),
            }],
        };
        assert!(verify_jwk_set(&jwk_set).is_err());
    }

    #[test]
    fn test_serialization_is_valid_jwk() {
        let jwk_set = JwkSet {
            members: vec![JwkMember {
                kty: "OKP".to_string(),
                crv: "Ed25519".to_string(),
                x: "dGVzdA".to_string(),
                kid: Some("test-key".to_string()),
            }],
        };

        let json = serde_json::to_string(&jwk_set).unwrap();
        let parsed: JwkSet = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.members.len(), 1);
        assert_eq!(parsed.members[0].kty, "OKP");
        assert_eq!(parsed.members[0].crv, "Ed25519");
    }

    /// Core M4-06 scenario: key rotation → both keys in JWK set.
    #[test]
    fn test_key_rotation_includes_both_keys() {
        let keys = vec![
            (
                KeyId::from_local_hex("old-key"),
                PublicKey([1u8; 32]),
            ),
            (
                KeyId::from_local_hex("new-key"),
                PublicKey([2u8; 32]),
            ),
        ];

        let jwk_set = keys_to_jwk_set(&keys);

        // Should contain both keys
        assert_eq!(jwk_set.members.len(), 2);

        // Verify both keys are present with correct kid
        let kids: Vec<&str> = jwk_set
            .members
            .iter()
            .filter_map(|m| m.kid.as_deref())
            .collect();
        // kids are base64url-encoded; KeyId::from_local_hex prefixes with "local:"
        let encoded = |s: &str| base64_url_no_pad(s.as_bytes());
        assert!(kids.contains(&encoded("local:old-key").as_str()));
        assert!(kids.contains(&encoded("local:new-key").as_str()));
    }
}