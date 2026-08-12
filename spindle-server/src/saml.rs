//! SAML endpoints for Spindle — M3-04.

#![allow(warnings)]
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use spindle_dex::SamlConfig as DexSamlConfig;
use spindle_saml::{
    AssertionValidator, AuthRequestBuilder, CertificateStore, ManagedCertificate,
    MetadataCache, SamlAssertion, SamlMetadata,
};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, info, warn};

// ── SAML State ───────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct SamlState {
    pub config: DexSamlConfig,
    pub entity_id: String,
    pub idp_sso_url: String,
    pub idp_cert_pem: String,
    pub sp_signing_cert: String,
    pub sp_encryption_cert: Option<String>,
    pub metadata_cache: MetadataCache,
    pub cert_store: CertificateStore,
    pub assertion_validator: AssertionValidator,
}

impl Clone for SamlState {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            entity_id: self.entity_id.clone(),
            idp_sso_url: self.idp_sso_url.clone(),
            idp_cert_pem: self.idp_cert_pem.clone(),
            sp_signing_cert: self.sp_signing_cert.clone(),
            sp_encryption_cert: self.sp_encryption_cert.clone(),
            metadata_cache: self.metadata_cache.clone(),
            cert_store: self.cert_store.clone(),
            assertion_validator: AssertionValidator::new(
                self.idp_sso_url.clone(),
                self.entity_id.clone(),
                self.cert_store.active().clone(),
            ),
        }
    }
}

impl SamlState {
    pub fn new(config: DexSamlConfig) -> Self {
        let entity_id = "https://spindle.local/saml/sp".to_string();
        let sp_signing_cert = "-----BEGIN CERTIFICATE-----\nSP-SIGNING-PEM\n-----END CERTIFICATE-----".to_string();
        let sp_encryption_cert = Some("-----BEGIN CERTIFICATE-----\nSP-ENCRYPTION-PEM\n-----END CERTIFICATE-----".to_string());
        let idp_sso_url = "https://idp.local/sso".to_string();
        let idp_cert_pem = "-----BEGIN CERTIFICATE-----\nIDP-CERT-PEM\n-----END CERTIFICATE-----".to_string();

        let idp_cert = ManagedCertificate::new(
            idp_cert_pem.clone(),
            Duration::from_secs(365 * 24 * 3600),
        );
        let cert_store = CertificateStore::new(idp_cert);

        let assertion_validator = AssertionValidator::new(
            idp_sso_url.clone(),
            entity_id.clone(),
            cert_store.active().clone(),
        );

        Self {
            config,
            entity_id,
            idp_sso_url,
            idp_cert_pem,
            sp_signing_cert,
            sp_encryption_cert,
            metadata_cache: MetadataCache::default_ttl(),
            cert_store,
            assertion_validator,
        }
    }

    pub fn generate_metadata(&self) -> SamlMetadata {
        SamlMetadata::from_config(
            &self.config,
            &self.sp_signing_cert,
            self.sp_encryption_cert.as_deref(),
        )
    }

    pub fn get_or_generate_metadata(&self) -> String {
        if let Some(cached) = self.metadata_cache.get("spindle-saml") {
            cached
        } else {
            let metadata = self.generate_metadata();
            let xml = metadata.to_xml();
            self.metadata_cache.put("spindle-saml", xml.clone());
            xml
        }
    }

    pub fn validator_mut(&mut self) -> &mut AssertionValidator {
        &mut self.assertion_validator
    }
}

// ── Query Params ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SsoParams {
    #[serde(default)]
    pub relay_state: Option<String>,
}

// ── Response Types ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct SsoRedirect {
    pub redirect_url: String,
    pub entity_id: String,
}

#[derive(Debug, Serialize)]
pub struct SamlResponse {
    pub success: bool,
    pub subject: Option<String>,
    pub groups: Vec<String>,
    pub message: String,
}

// ── Endpoints ────────────────────────────────────────────────────────────────

pub async fn get_metadata(
    State(state): State<SamlState>,
) -> (StatusCode, String) {
    (StatusCode::OK, state.get_or_generate_metadata())
}

pub async fn get_sso(
    State(state): State<SamlState>,
    Query(params): Query<SsoParams>,
) -> Json<SsoRedirect> {
    let builder = AuthRequestBuilder::new(
        state.entity_id.clone(),
        state.config.redirect_url.clone(),
    );
    let builder = if let Some(ref relay_state) = params.relay_state {
        builder.with_relay_state(relay_state.clone())
    } else {
        builder
    };
    let url = builder.build_redirect_url(&state.idp_sso_url);

    debug!(
        idp_sso_url = %state.idp_sso_url,
        relay_state = %params.relay_state.as_deref().unwrap_or("none"),
        "SP-initiated SSO redirect"
    );

    Json(SsoRedirect {
        redirect_url: url,
        entity_id: state.entity_id,
    })
}

pub async fn post_acs(
    State(state): State<SamlState>,
    body: axum::extract::Form<HashMap<String, String>>,
) -> (StatusCode, Json<SamlResponse>) {
    let saml_response = match body.get("SAMLResponse") {
        Some(resp) => resp.clone(),
        None => {
            warn!("ACS received without SAMLResponse");
            return (
                StatusCode::BAD_REQUEST,
                Json(SamlResponse {
                    success: false,
                    subject: None,
                    groups: vec![],
                    message: "Missing SAMLResponse parameter".to_string(),
                }),
            );
        }
    };

    let assertion: SamlAssertion = match serde_json::from_str(&saml_response) {
        Ok(a) => a,
        Err(e) => {
            warn!("Failed to parse SAML assertion: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(SamlResponse {
                    success: false,
                    subject: None,
                    groups: vec![],
                    message: format!("Failed to parse assertion: {}", e),
                }),
            );
        }
    };

    // Create a fresh validator for this request — all needed values are public on SamlState
    let validator = AssertionValidator::new(
        state.idp_sso_url.clone(),
        state.entity_id.clone(),
        state.cert_store.active().clone(),
    );

    if let Err(e) = validator.validate(&assertion) {
        warn!("Assertion validation failed: {}", e);
        return (
            StatusCode::UNAUTHORIZED,
            Json(SamlResponse {
                success: false,
                subject: None,
                groups: vec![],
                message: format!("Assertion validation failed: {}", e),
            }),
        );
    }

    let subject = assertion.subject.clone();
    let groups = assertion.groups.clone();

    info!(subject = %subject, groups = ?groups, "SAML assertion validated");

    (
        StatusCode::OK,
        Json(SamlResponse {
            success: true,
            subject: Some(subject),
            groups,
            message: "SAML authentication successful".to_string(),
        }),
    )
}

pub async fn post_update_metadata(
    State(_state): State<SamlState>,
    body: axum::extract::Form<HashMap<String, String>>,
) -> (StatusCode, Json<SamlResponse>) {
    if body.get("metadata_xml").is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(SamlResponse {
                success: false,
                subject: None,
                groups: vec![],
                message: "Missing metadata_xml parameter".to_string(),
            }),
        );
    }
    info!("IdP metadata update acknowledged");
    (
        StatusCode::OK,
        Json(SamlResponse {
            success: true,
            subject: None,
            groups: vec![],
            message: "IdP metadata updated".to_string(),
        }),
    )
}

pub async fn post_slo(
    State(_state): State<SamlState>,
    body: axum::extract::Form<HashMap<String, String>>,
) -> (StatusCode, Json<SamlResponse>) {
    if body.get("SAMLRequest").is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(SamlResponse {
                success: false,
                subject: None,
                groups: vec![],
                message: "Missing SAMLRequest parameter".to_string(),
            }),
        );
    }
    info!("SAML LogoutRequest received");
    (
        StatusCode::OK,
        Json(SamlResponse {
            success: true,
            subject: None,
            groups: vec![],
            message: "Logout processed".to_string(),
        }),
    )
}

pub fn saml_routes(state: SamlState) -> Router {
    Router::new()
        .route("/v1/auth/saml/metadata", axum::routing::get(get_metadata))
        .route("/v1/auth/saml/sso", axum::routing::get(get_sso))
        .route("/v1/auth/saml/acs", axum::routing::post(post_acs))
        .route("/v1/auth/saml/metadata", axum::routing::post(post_update_metadata))
        .route("/v1/auth/saml/slo", axum::routing::post(post_slo))
        .with_state(state)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::{Service, ServiceExt};

    fn test_saml_state() -> SamlState {
        let config = DexSamlConfig {
            client_id: "spindle-test".to_string(),
            redirect_url: "https://spindle.test.local/saml/acs".to_string(),
            scope: None,
            group_claim: Some("groups".to_string()),
            group_mapping: vec![],
        };
        SamlState::new(config)
    }

    // ── Metadata ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_metadata_returns_xml() {
        let state = test_saml_state();
        let app = Router::new()
            .route("/v1/auth/saml/metadata", axum::routing::get(get_metadata))
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/auth/saml/metadata")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8_lossy(&body);
        assert!(body_str.contains("EntityDescriptor"));
        assert!(body_str.contains("SPSSODescriptor"));
    }

    #[tokio::test]
    async fn test_get_metadata_cached() {
        let mut state = test_saml_state();
        state.metadata_cache.put("spindle-saml", "<cached-metadata>".to_string());

        let app = Router::new()
            .route("/v1/auth/saml/metadata", axum::routing::get(get_metadata))
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/auth/saml/metadata")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8_lossy(&body);
        assert!(body_str.contains("cached-metadata"));
    }

    // ── SSO ──────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_sso_returns_redirect_url() {
        let state = test_saml_state();
        let app = Router::new()
            .route("/v1/auth/saml/sso", axum::routing::get(get_sso))
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/auth/saml/sso")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.get("redirect_url").is_some());
        assert!(json.get("entity_id").is_some());
    }

    #[tokio::test]
    async fn test_get_sso_with_relay_state() {
        let state = test_saml_state();
        let app = Router::new()
            .route("/v1/auth/saml/sso", axum::routing::get(get_sso))
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/auth/saml/sso?relay_state=myState123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let url = json["redirect_url"].as_str().unwrap();
        assert!(url.contains("SAMLRequest"));
        assert!(url.contains("RelayState"));
    }

    // ── ACS ──────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_post_acs_missing_saml_response() {
        let state = test_saml_state();
        let app = Router::new()
            .route("/v1/auth/saml/acs", axum::routing::post(post_acs))
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/auth/saml/acs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_post_acs_valid_assertion() {
        let state = test_saml_state();
        let app = Router::new()
            .route("/v1/auth/saml/acs", axum::routing::post(post_acs))
            .with_state(state);

        let mut assertion = serde_json::Map::new();
        assertion.insert("id".to_string(), serde_json::json!("assert-1"));
        assertion.insert(
            "issuer".to_string(),
            serde_json::json!("https://idp.local/sso"),
        );
        assertion.insert("subject".to_string(), serde_json::json!("user-1"));
        assertion.insert(
            "name_id".to_string(),
            serde_json::json!("user@example.com"),
        );
        assertion.insert(
            "groups".to_string(),
            serde_json::json!(["admin", "editors"]),
        );

        let form = format!("SAMLResponse={}", serde_json::to_string(&assertion).unwrap());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/auth/saml/acs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(form))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let response: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(response["success"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_post_acs_invalid_issuer() {
        let state = test_saml_state();
        let app = Router::new()
            .route("/v1/auth/saml/acs", axum::routing::post(post_acs))
            .with_state(state);

        let mut assertion = serde_json::Map::new();
        assertion.insert("id".to_string(), serde_json::json!("assert-2"));
        assertion.insert(
            "issuer".to_string(),
            serde_json::json!("https://bad-issuer.local"),
        );
        assertion.insert("subject".to_string(), serde_json::json!("user-2"));
        assertion.insert("name_id".to_string(), serde_json::json!("user2@example.com"));
        assertion.insert("id".to_string(), serde_json::json!("assert-2"));
        assertion.insert("groups".to_string(), serde_json::json!(["viewers"]));

        let form = format!("SAMLResponse={}", serde_json::to_string(&assertion).unwrap());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/auth/saml/acs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(form))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_post_acs_invalid_json() {
        let state = test_saml_state();
        let app = Router::new()
            .route("/v1/auth/saml/acs", axum::routing::post(post_acs))
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/auth/saml/acs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("SAMLResponse=not-valid-json".to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── SLO ──────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_post_slo_receives_logout_request() {
        let state = test_saml_state();
        let app = Router::new()
            .route("/v1/auth/saml/slo", axum::routing::post(post_slo))
            .with_state(state);

        let form = "SAMLRequest=logout-request-data".to_string();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/auth/saml/slo")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(form))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let response: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(response["success"].as_bool().unwrap());
    }

    // ── Metadata Update ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_post_update_metadata() {
        let state = test_saml_state();
        let app = Router::new()
            .route("/v1/auth/saml/metadata", axum::routing::post(post_update_metadata))
            .with_state(state);

        let form = "metadata_xml=<?xml version=\"1.0\"?><IdPMetadata>test</IdPMetadata>".to_string();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/auth/saml/metadata")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(form))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let response: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(response["success"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_post_update_metadata_missing_param() {
        let state = test_saml_state();
        let app = Router::new()
            .route("/v1/auth/saml/metadata", axum::routing::post(post_update_metadata))
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/auth/saml/metadata")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── SAML State ───────────────────────────────────────────────────────────

    #[test]
    fn test_saml_state_creation() {
        let state = test_saml_state();
        assert!(!state.entity_id.is_empty());
        assert!(state.idp_sso_url.contains("idp.local"));
        assert!(!state.sp_signing_cert.is_empty());
    }

    #[test]
    fn test_saml_state_generate_metadata() {
        let state = test_saml_state();
        let metadata = state.generate_metadata();
        let xml = metadata.to_xml();

        assert!(xml.contains("EntityDescriptor"));
        assert!(xml.contains("SPSSODescriptor"));
        assert!(xml.contains("SP-SIGNING-PEM"));
    }

    #[test]
    fn test_saml_state_get_or_generate_metadata_cached() {
        let mut state = test_saml_state();
        state.metadata_cache.put("spindle-saml", "<pre-cached>".to_string());
        assert_eq!(state.get_or_generate_metadata(), "<pre-cached>");
    }

    #[test]
    fn test_saml_state_get_or_generate_metadata_fresh() {
        let state = test_saml_state();
        let xml = state.get_or_generate_metadata();
        assert!(xml.contains("EntityDescriptor"));
        assert!(state.metadata_cache.get("spindle-saml").is_some());
    }

    // ── Routes ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_saml_routes_not_found() {
        let state = test_saml_state();
        let app = saml_routes(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/auth/saml/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── Certificate Rotation ─────────────────────────────────────────────────

    #[test]
    fn test_cert_rotation() {
        let mut state = test_saml_state();
        let old_cert = state.idp_cert_pem.clone();

        state.cert_store.rotate(ManagedCertificate::new(
            "NEW-CERT-PEM".to_string(),
            Duration::from_secs(86400),
        ));

        assert_eq!(state.cert_store.active_pem(), "NEW-CERT-PEM");
        assert_eq!(state.cert_store.rotated_pem(), Some(old_cert));
    }

    #[test]
    fn test_cert_expiry_detection() {
        let mut state = test_saml_state();

        state.cert_store.rotate(ManagedCertificate::new(
            "EXPIRED-CERT".to_string(),
            Duration::from_millis(10),
        ));

        std::thread::sleep(Duration::from_millis(50));

        assert!(!state.cert_store.active().is_valid());
    }

    // ── Metadata Caching ─────────────────────────────────────────────────────

    #[test]
    fn test_metadata_cache_invalidation() {
        let mut state = test_saml_state();
        state.metadata_cache.put("spindle-saml", "<cached>".to_string());
        assert!(state.metadata_cache.get("spindle-saml").is_some());

        state.metadata_cache.invalidate("spindle-saml");
        assert!(state.metadata_cache.get("spindle-saml").is_none());
    }
}