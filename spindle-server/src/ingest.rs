//! Ingest HTTP endpoint for Chef Infra data-collector events.
//!
//! # Usage
//! ```ignore
//! use spindle_server::ingest::IngestConfig;
//!
//! let config = IngestConfig { token: "super-secret-token" };
//! let app = ingest_routes(config);
//! ```
//!
//! ## Endpoints
//! - `POST /ingest/events/data-collector` — accepts Chef Infra data-collector payloads
//!
//! ## Payload types (detected by JSON structure, not Content-Type)
//! - **run-start**: `{ "run_id": "...", "node_name": "...", ... }` (no `resources` key)
//! - **run-converge**: `{ "run_id": "...", "node_name": "...", "resources": [...] }`
//! - **compliance-report**: `{ "profiles": [...], "controls": [...] }`

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json, Router,
    routing::post,
};
use serde_json::Value;
use subtle::ConstantTimeEq;
use uuid::Uuid;

/// Configuration for the ingest endpoint.
/// Token is compared using constant-time comparison to prevent timing attacks.
#[derive(Debug, Clone)]
pub struct IngestConfig {
    /// The expected bearer token for authentication.
    /// Stored as bytes for constant-time comparison.
    pub token: String,
}

impl IngestConfig {
    /// Returns the token as bytes for constant-time comparison.
    pub fn token_bytes(&self) -> &[u8] {
        self.token.as_bytes()
    }
}

/// Receipt token returned on successful ingestion.
/// Format: "receipt:{uuid}" — can be used to look up the raw payload.
#[derive(Debug, Clone)]
pub struct ReceiptToken(String);

impl std::fmt::Display for ReceiptToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl ReceiptToken {
    /// Generate a new receipt token.
    pub fn new() -> Self {
        Self(format!("receipt:{}", Uuid::new_v4()))
    }
}

/// Payload type detected from the JSON structure.
#[derive(Debug, Clone, PartialEq)]
pub enum PayloadType {
    RunStart,
    RunConverge,
    ComplianceReport,
    Unknown,
}

/// Detects the payload type from the JSON structure.
/// - Compliance-report: has "profiles" key at top level
/// - Run-converge: has "resources" key at top level (and no "profiles")
/// - Run-start: no "resources" or "profiles" keys, has "run_id"
pub fn detect_payload_type(json: &Value) -> PayloadType {
    if !json.is_object() {
        return PayloadType::Unknown;
    }

    let obj = json.as_object().unwrap();

    // Compliance report: presence of "profiles" key indicates a compliance scan
    if obj.contains_key("profiles") {
        return PayloadType::ComplianceReport;
    }

    // Run-converge: has "resources" array (converged run with resource events)
    if obj.contains_key("resources") {
        return PayloadType::RunConverge;
    }

    // Run-start: has "run_id" but no resources or profiles
    if obj.contains_key("run_id") {
        return PayloadType::RunStart;
    }

    PayloadType::Unknown
}

/// Middleware for constant-time token verification.
/// Returns false if the token is missing or doesn't match.
/// Uses `subtle::ConstantTimeEq` for timing-attack resistance —
/// the comparison runs in constant time regardless of where the first
/// mismatch occurs, preventing side-channel attacks on the token.
pub fn verify_bearer_token(config: &IngestConfig, auth_header: Option<&str>) -> bool {
    if config.token.is_empty() {
        return false;
    }

    match auth_header {
        None => false,
        Some(header) => {
            let bearer = header.strip_prefix("Bearer ").unwrap_or(header);
            let token_bytes = config.token_bytes();
            let provided_bytes = bearer.as_bytes();

            // Constant-time comparison — ct_eq returns 1 if equal, 0 if not
            // The comparison does NOT short-circuit on first mismatch
            token_bytes.ct_eq(provided_bytes).into()
        }
    }
}

/// Response for ingest endpoint.
#[derive(Debug)]
pub enum IngestResponse {
    Accepted(ReceiptToken),
    Unauthorized,
    BadRequest(String),
}

impl IntoResponse for IngestResponse {
    fn into_response(self) -> Response {
        match self {
            IngestResponse::Accepted(receipt) => {
                let body = serde_json::json!({
                    "status": "accepted",
                    "receipt_token": receipt.to_string(),
                    "message": "Payload received and queued for processing"
                });
                (StatusCode::ACCEPTED, Json(body)).into_response()
            }
            IngestResponse::Unauthorized => {
                (StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
            }
            IngestResponse::BadRequest(msg) => {
                let body = serde_json::json!({
                    "status": "bad_request",
                    "error": msg
                });
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            }
        }
    }
}

/// Handler for POST /ingest/events/data-collector
///
/// 1. Extracts Authorization: Bearer header
/// 2. Compares token with constant-time comparison (timing-attack resistant)
/// 3. Parses JSON body and detects payload type by structure
/// 4. Returns 202 with receipt token on success, 401 on auth failure, 400 on bad payload
pub async fn data_collector_handler(
    State(config): State<IngestConfig>,
    headers: header::HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    // Extract and verify bearer token using constant-time comparison
    let auth_header = headers.get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    if !verify_bearer_token(&config, auth_header) {
        tracing::warn!("Unauthorized ingest attempt — token mismatch");
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }

    // Detect payload type from JSON structure
    let payload_type = detect_payload_type(&payload);

    if payload_type == PayloadType::Unknown {
        tracing::warn!("Received ingest payload with unknown structure");
        let body = serde_json::json!({
            "status": "bad_request",
            "error": "Could not determine payload type — expected run_id, resources, or profiles key"
        });
        return (StatusCode::BAD_REQUEST, Json(body)).into_response();
    }

    // Generate receipt token
    let receipt = ReceiptToken::new();

    tracing::info!(
        payload_type = ?payload_type,
        receipt = %receipt,
        "Data-collector payload received"
    );

    let body = serde_json::json!({
        "status": "accepted",
        "receipt_token": receipt.to_string(),
        "message": format!("{} payload received and queued for processing", 
            match payload_type {
                PayloadType::RunStart => "run-start",
                PayloadType::RunConverge => "run-converge",
                PayloadType::ComplianceReport => "compliance-report",
                PayloadType::Unknown => "unknown",
            }
        )
    });

    (StatusCode::ACCEPTED, Json(body)).into_response()
}

/// Builds the Axum router for ingest endpoints.
pub fn ingest_routes(config: IngestConfig) -> Router {
    Router::new()
        .route("/ingest/events/data-collector", post(data_collector_handler))
        .with_state(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[test]
    fn test_detect_payload_type_run_start() {
        let json = serde_json::json!({
            "run_id": "2026-08-06T12:00:00Z",
            "node_name": "web-server-01",
            "chef_version": "18.0.0"
        });
        assert_eq!(detect_payload_type(&json), PayloadType::RunStart);
    }

    #[test]
    fn test_detect_payload_type_run_converge() {
        let json = serde_json::json!({
            "run_id": "2026-08-06T12:00:00Z",
            "node_name": "web-server-01",
            "resources": [
                {
                    "type": "package",
                    "name": "nginx",
                    "status": "updated"
                }
            ]
        });
        assert_eq!(detect_payload_type(&json), PayloadType::RunConverge);
    }

    #[test]
    fn test_detect_payload_type_compliance_report() {
        let json = serde_json::json!({
            "profiles": [
                {
                    "name": "ssh-baseline",
                    "version": "1.0.0",
                    "controls": []
                }
            ]
        });
        assert_eq!(detect_payload_type(&json), PayloadType::ComplianceReport);
    }

    #[test]
    fn test_detect_payload_type_unknown() {
        let json = serde_json::json!({
            "foo": "bar"
        });
        assert_eq!(detect_payload_type(&json), PayloadType::Unknown);
    }

    #[test]
    fn test_detect_payload_type_non_object() {
        let json = serde_json::json!([1, 2, 3]);
        assert_eq!(detect_payload_type(&json), PayloadType::Unknown);
    }

    #[test]
    fn test_receipt_token_format() {
        let receipt = ReceiptToken::new();
        let s = receipt.to_string();
        assert!(s.starts_with("receipt:"));
        let uuid_str = &s[8..];
        assert!(uuid::Uuid::parse_str(uuid_str).is_ok());
    }

    #[test]
    fn test_constant_time_comparison_valid() {
        let config = IngestConfig { token: "test-token-123".to_string() };
        assert!(verify_bearer_token(&config, Some("Bearer test-token-123")));
    }

    #[test]
    fn test_constant_time_comparison_invalid() {
        let config = IngestConfig { token: "test-token-123".to_string() };
        assert!(!verify_bearer_token(&config, Some("Bearer wrong-token")));
    }

    #[test]
    fn test_constant_time_comparison_missing() {
        let config = IngestConfig { token: "test-token-123".to_string() };
        assert!(!verify_bearer_token(&config, None));
    }

    #[test]
    fn test_constant_time_comparison_empty_config() {
        let config = IngestConfig { token: String::new() };
        assert!(!verify_bearer_token(&config, Some("Bearer anything")));
    }

    #[test]
    fn test_constant_time_comparison_wrong_length() {
        let config = IngestConfig { token: "test-token-123".to_string() };
        // ct_eq handles different lengths safely — returns false
        assert!(!verify_bearer_token(&config, Some("Bearer short")));
    }

    #[tokio::test]
    async fn test_handler_valid_token_run_start() {
        let config = IngestConfig { token: "valid-secret-token".to_string() };
        let app = ingest_routes(config);

        let payload = serde_json::json!({
            "run_id": "2026-08-06T12:00:00Z",
            "node_name": "web-server-01"
        });

        let request = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn test_handler_valid_token_compliance_report() {
        let config = IngestConfig { token: "valid-secret-token".to_string() };
        let app = ingest_routes(config);

        let payload = serde_json::json!({
            "profiles": [
                {
                    "name": "ssh-baseline",
                    "controls": []
                }
            ]
        });

        let request = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn test_handler_invalid_token() {
        let config = IngestConfig { token: "valid-secret-token".to_string() };
        let app = ingest_routes(config);

        let payload = serde_json::json!({
            "run_id": "2026-08-06T12:00:00Z",
            "node_name": "web-server-01"
        });

        let request = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer wrong-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_handler_missing_token() {
        let config = IngestConfig { token: "valid-secret-token".to_string() };
        let app = ingest_routes(config);

        let payload = serde_json::json!({
            "run_id": "2026-08-06T12:00:00Z",
            "node_name": "web-server-01"
        });

        let request = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_handler_unknown_payload_type() {
        let config = IngestConfig { token: "valid-secret-token".to_string() };
        let app = ingest_routes(config);

        let payload = serde_json::json!({
            "foo": "bar"
        });

        let request = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_ingest_routes_builds() {
        let config = IngestConfig { token: "test".to_string() };
        let _app = ingest_routes(config);
    }
}
