//! Public key publishing endpoint: GET /.well-known/spindle/keys.json.
//!
//! JWK Set format (RFC 7517) with Ed25519 keys. Cacheable via ETag and
//! max-age=3600. Includes both active and retired keys for rotation support.
//!
//! Uses spindle-signing's JWK types directly for consistent serialization.

use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json},
};
use spindle_signing::jwk::{JwkMember, JwkSet};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// JWK set with caching metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedJwkSet {
    /// The JWK set itself.
    pub jwks: JwkSet,
    /// When this set was last updated.
    pub updated_at: String,
    /// Number of keys.
    pub key_count: usize,
}

/// Shared state for the keys.json endpoint.
#[derive(Clone)]
pub struct KeysAppState {
    /// Current key set: (key_id, public_key).
    pub keys: Arc<Vec<(String, String)>>,
    /// ETag for cache validation.
    pub etag: Arc<String>,
    /// Last update timestamp.
    pub updated_at: Arc<String>,
}

impl KeysAppState {
    pub fn new(keys: Vec<(String, String)>) -> Self {
        // Simple hash of keys for ETag generation
        let mut hasher = sha2::Sha256::new();
        use sha2::Digest;
        for (kid, pk) in &keys {
            hasher.update(kid.as_bytes());
            hasher.update(pk.as_bytes());
        }
        let etag = format!(
            "\"{}\"",
            hex::encode(hasher.finalize())
        );
        let updated_at = chrono::Utc::now().to_rfc3339();
        Self {
            keys: Arc::new(keys),
            etag: Arc::new(etag),
            updated_at: Arc::new(updated_at),
        }
    }

    /// Convert stored keys to JWK members.
    fn to_jwks(&self) -> JwkSet {
        JwkSet {
            members: self.keys.iter().map(|(kid, pk)| JwkMember {
                kty: "OKP".to_string(),
                crv: "Ed25519".to_string(),
                x: pk.clone(),
                kid: Some(kid.clone()),
            }).collect(),
        }
    }
}

/// Handler for GET /.well-known/spindle/keys.json
///
/// Returns JWK set with:
/// - ETag header for cache validation
/// - Cache-Control: max-age=3600
/// - Both active and retired keys (for rotation)
pub async fn keys_json(
    State(state): State<KeysAppState>,
    request: Request<axum::body::Body>,
) -> impl IntoResponse {
    let headers = request.headers();

    // Check ETag / If-None-Match
    if let Some(if_none_match) = headers.get(header::IF_NONE_MATCH) {
        if if_none_match == state.etag.as_str() {
            return (StatusCode::NOT_MODIFIED, "").into_response();
        }
    }

    let jwks = state.to_jwks();
    let cached = CachedJwkSet {
        jwks,
        updated_at: state.updated_at.as_str().to_string(),
        key_count: state.keys.len(),
    };

    let mut response = Json(cached).into_response();
    response.headers_mut().insert(
        header::ETAG,
        state.etag.as_str().parse().unwrap_or_else(|_| header::HeaderValue::from_static("\"\"")),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("max-age=3600"),
    );
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );

    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn test_state() -> KeysAppState {
        KeysAppState::new(vec![
            ("key-a".to_string(), "dGVzdGtleUE".to_string()),
            ("key-b".to_string(), "dGVzdGtleUI".to_string()),
        ])
    }

    #[test]
    fn test_jwks_converts_keys() {
        let state = test_state();
        let jwks = state.to_jwks();
        assert_eq!(jwks.members.len(), 2);
        assert_eq!(jwks.members[0].kty, "OKP");
        assert_eq!(jwks.members[0].crv, "Ed25519");
        assert!(jwks.members[0].kid.is_some());
    }

    #[test]
    fn test_etag_changes_with_keys() {
        let state_a = KeysAppState::new(vec![
     ("key-a".to_string(), "dGVzdA".to_string()),
 ]);
 let state_b = KeysAppState::new(vec![
     ("key-b".to_string(), "dGVzdA".to_string()),
 ]);
        assert_ne!(state_a.etag, state_b.etag);
    }

    #[tokio::test]
    async fn test_keys_json_endpoint() {
        let state = test_state();
        let app = axum::Router::new()
            .route("/.well-known/spindle/keys.json", axum::routing::get(keys_json))
            .with_state(state.clone());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/.well-known/spindle/keys.json")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            resp.headers()
                .get(header::CACHE_CONTROL)
                .map(|v| v.to_str().unwrap_or(""))
                .unwrap_or("")
                .contains("max-age=3600")
        );
        assert!(resp.headers().get(header::ETAG).is_some());
    }

    #[tokio::test]
    async fn test_keys_json_etag_caching() {
        let state = test_state();
        let app = axum::Router::new()
            .route("/.well-known/spindle/keys.json", axum::routing::get(keys_json))
            .with_state(state.clone());

        // First request - should be 200
        let resp1 = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/.well-known/spindle/keys.json")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp1.status(), StatusCode::OK);

        // Second request with matching ETag - should be 304
        let etag = resp1
            .headers()
            .get(header::ETAG)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let resp2 = app
            .oneshot(
                Request::builder()
                    .uri("/.well-known/spindle/keys.json")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::IF_NONE_MATCH, &etag)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::NOT_MODIFIED);
    }

    #[test]
    fn test_empty_keys_returns_empty_set() {
        let state = KeysAppState::new(vec![]);
        let jwks = state.to_jwks();
        assert_eq!(jwks.members.len(), 0);
    }
}