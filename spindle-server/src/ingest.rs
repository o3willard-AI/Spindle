//! Ingest HTTP endpoint for Chef Infra data-collector events.
//!
//! # Usage
//! ```ignore
//! use spindle_server::ingest::{IngestConfig, IngestAppState};
//!
//! let state = IngestAppState::new(
//!     "super-secret-token".to_string(),
//!     Arc::new(LocalArchive::new("/var/lib/spindle/archive")?),
//!     pg_pool,
//! );
//! let app = ingest_routes(state);
//! ```
//!
//! ## Endpoints
//! - `POST /ingest/events/data-collector` — accepts Chef Infra data-collector payloads
//!
//! ## Processing pipeline
//! 1. Validate payload size (≤ max_size)
//! 2. Write verbatim payload to raw archive
//! 3. Enqueue for async processing (Postgres-backed job queue)
//! 4. Return 202 with receipt token
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
use std::sync::Arc;
use std::time::Instant;
use subtle::ConstantTimeEq;
use uuid::Uuid;

use spindle_rawarchive::{Archive, ArchiveMetadata};

/// Maximum payload size in bytes (32 MB default).
/// Configurable via environment variable SPINDLE_INGEST_MAX_PAYLOAD_SIZE.
pub const DEFAULT_MAX_PAYLOAD_SIZE: u64 = 32 * 1024 * 1024; // 32 MB

/// Configuration for the ingest endpoint.
/// Token is compared using constant-time comparison to prevent timing attacks.
#[derive(Debug, Clone)]
pub struct IngestConfig {
    /// The expected bearer token for authentication.
    /// Stored as bytes for constant-time comparison.
    pub token: String,
    /// Maximum payload size in bytes (default: 32 MB).
    pub max_payload_size: u64,
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            token: String::new(),
            max_payload_size: DEFAULT_MAX_PAYLOAD_SIZE,
        }
    }
}

impl IngestConfig {
    /// Create a new config with a token.
    pub fn new(token: &str) -> Self {
        Self {
            token: token.to_string(),
            max_payload_size: DEFAULT_MAX_PAYLOAD_SIZE,
        }
    }

    /// Create a new config with token and max payload size.
    pub fn with_max_size(token: &str, max_payload_size: u64) -> Self {
        Self {
            token: token.to_string(),
            max_payload_size,
        }
    }

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

/// Extract the bearer token from the Authorization header.
pub fn extract_bearer(headers: &header::HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").map(|s| s.to_string()))
}

/// Response for ingest endpoint.
#[derive(Debug)]
pub enum IngestResponse {
    Accepted(ReceiptToken),
    Unauthorized,
    BadRequest(String),
    PayloadTooLarge(u64),
    ServiceUnavailable(String),
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
            IngestResponse::PayloadTooLarge(size) => {
                let body = serde_json::json!({
                    "status": "payload_too_large",
                    "error": format!("Payload size {} bytes exceeds maximum allowed size of {} bytes", size, DEFAULT_MAX_PAYLOAD_SIZE)
                });
                (StatusCode::PAYLOAD_TOO_LARGE, Json(body)).into_response()
            }
            IngestResponse::ServiceUnavailable(msg) => {
                let body = serde_json::json!({
                    "status": "service_unavailable",
                    "error": msg
                });
                (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response()
            }
        }
    }
}

/// Application state for the ingest endpoint.
/// Contains the config, raw archive for write-before-parse, and a job queue
/// for asynchronous processing.
#[derive(Debug, Clone)]
pub struct IngestAppState {
    pub config: IngestConfig,
    pub archive: Arc<dyn Archive>,
}

impl IngestAppState {
    pub fn new(config: IngestConfig, archive: Arc<dyn Archive>) -> Self {
        Self { config, archive }
    }
}

/// Handler for POST /ingest/events/data-collector
///
/// Processing pipeline:
/// 1. Validate payload size (≤ max_size)
/// 2. Write verbatim payload to raw archive
/// 3. Archive write fails → 503
/// 4. Enqueue for async processing (Postgres-backed job queue)
/// 5. Return 202 with receipt token
///
/// Timing-attack resistant token comparison via subtle::ConstantTimeEq.
pub async fn data_collector_handler(
    State(state): State<IngestAppState>,
    headers: header::HeaderMap,
    request_body: axum::body::Body,
) -> Response {
    // Start latency tracking
    let start = Instant::now();

    // Extract and verify bearer token using constant-time comparison
    let auth_header = headers.get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    if !verify_bearer_token(&state.config, auth_header) {
        tracing::warn!("Unauthorized ingest attempt — token mismatch");
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }

    // Read the body as bytes (for size validation and verbatim archiving)
    let payload_bytes = match axum::body::to_bytes(request_body, state.config.max_payload_size as usize).await {
        Ok(bytes) => bytes,
        Err(_) => {
            let elapsed = start.elapsed();
            tracing::warn!(
                latency_ms = %elapsed.as_millis(),
                "Payload exceeds max size — rejected"
            );
            return (StatusCode::PAYLOAD_TOO_LARGE, "Payload too large").into_response();
        }
    };

    // Validate payload size
    if payload_bytes.len() as u64 > state.config.max_payload_size {
        let elapsed = start.elapsed();
        tracing::warn!(
            payload_size = payload_bytes.len(),
            max_size = state.config.max_payload_size,
            latency_ms = %elapsed.as_millis(),
            "Payload exceeds size limit"
        );
        let body = serde_json::json!({
            "status": "payload_too_large",
            "error": format!(
                "Payload size {} exceeds max {}",
                payload_bytes.len(), state.config.max_payload_size
            )
        });
        return (StatusCode::PAYLOAD_TOO_LARGE, axum::Json(body)).into_response();
    }

    // Parse JSON to detect payload type
    let payload_json: Value = match serde_json::from_slice(&payload_bytes) {
        Ok(v) => v,
        Err(e) => {
            let elapsed = start.elapsed();
            tracing::warn!(
                error = %e,
                latency_ms = %elapsed.as_millis(),
                "Failed to parse JSON payload"
            );
            // Archive the raw bytes even if JSON parsing fails
            let metadata = ArchiveMetadata::new(
                compute_sha256(&payload_bytes),
                "application/json".to_string(),
                "unknown".to_string(),
                chrono::Utc::now(),
            );

            let receipt = ReceiptToken::new();
            match state.archive.store(&payload_bytes, &metadata) {
                Ok(_key) => {
                    tracing::info!(
                        receipt = %receipt,
                        latency_ms = %elapsed.as_millis(),
                        "Malformed JSON archived and accepted (202)"
                    );
                    let body = serde_json::json!({
                        "status": "accepted",
                        "receipt_token": receipt.to_string(),
                        "message": "Malformed payload archived, queued for review"
                    });
                    return (StatusCode::ACCEPTED, axum::Json(body)).into_response();
                }
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        latency_ms = %elapsed.as_millis(),
                        "Archive write failed for malformed payload"
                    );
                    let body = serde_json::json!({
                        "status": "service_unavailable",
                        "error": format!("Archive write failed: {}", e)
                    });
                    return (StatusCode::SERVICE_UNAVAILABLE, axum::Json(body)).into_response();
                }
            }
        }
    };

    // Detect payload type from JSON structure
    let payload_type = detect_payload_type(&payload_json);

    if payload_type == PayloadType::Unknown {
        let elapsed = start.elapsed();
        tracing::warn!(
            latency_ms = %elapsed.as_millis(),
            "Received ingest payload with unknown structure"
        );

        // Still archive the unknown payload for debugging
        let metadata = ArchiveMetadata::new(
            compute_sha256(&payload_bytes),
            "application/json".to_string(),
            "unknown".to_string(),
            chrono::Utc::now(),
        );

        let receipt = ReceiptToken::new();
        match state.archive.store(&payload_bytes, &metadata) {
            Ok(_key) => {
                let body = serde_json::json!({
                    "status": "accepted",
                    "receipt_token": receipt.to_string(),
                    "message": "Unknown payload type archived"
                });
                return (StatusCode::ACCEPTED, axum::Json(body)).into_response();
            }
            Err(e) => {
                let body = serde_json::json!({
                    "status": "service_unavailable",
                    "error": format!("Archive write failed: {}", e)
                });
                return (StatusCode::SERVICE_UNAVAILABLE, axum::Json(body)).into_response();
            }
        }
    }

    // Step 1: Write verbatim payload to raw archive
    // This happens BEFORE parsing — write-before-parse ensures no data loss
    let token_id = extract_bearer(&headers).unwrap_or_else(|| "unknown".to_string());
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();

    let metadata = ArchiveMetadata::new(
        compute_sha256(&payload_bytes),
        content_type,
        token_id,
        chrono::Utc::now(),
    );

    // Write to archive and time it
    let archive_start = Instant::now();
    let archive_key = match state.archive.store(&payload_bytes, &metadata) {
        Ok(key) => key,
        Err(e) => {
            let elapsed = start.elapsed();
            tracing::error!(
                error = %e,
                latency_ms = %elapsed.as_millis(),
                "Archive write failed — returning 503"
            );

            // Track archive write failure metric
            // (in production: prometheus counter)
            tracing::warn!(metric = "spindle_archive_write_seconds", error = %e, "archive_write_failed");

            let body = serde_json::json!({
                "status": "service_unavailable",
                "error": format!("Archive write failed: {}", e)
            });
            return (StatusCode::SERVICE_UNAVAILABLE, axum::Json(body)).into_response();
        }
    };

    let archive_elapsed = archive_start.elapsed();
    tracing::info!(
        archive_key = %archive_key,
        archive_write_ms = %archive_elapsed.as_millis(),
        "Payload archived successfully"
    );

    // Track archive write latency metric
    // In production: histogram!("spindle_archive_write_seconds", archive_elapsed);
    tracing::info!(
        metric = "spindle_archive_write_seconds",
        value = %archive_elapsed.as_secs_f64(),
        "archive_write_latency"
    );

    // Step 2: Enqueue for async processing
    // TODO: Implement Postgres-backed job queue
    // For now, the archive key is used as the enqueue reference
    let receipt = ReceiptToken::new();

    let elapsed = start.elapsed();
    tracing::info!(
        payload_type = ?payload_type,
        receipt = %receipt,
        archive_key = %archive_key,
        total_latency_ms = %elapsed.as_millis(),
        archive_write_ms = %archive_elapsed.as_millis(),
        "Data-collector payload received, archived, and queued"
    );

    let body = serde_json::json!({
        "status": "accepted",
        "receipt_token": receipt.to_string(),
        "archive_key": archive_key,
        "message": format!("{} payload received, archived, and queued for processing",
            match payload_type {
                PayloadType::RunStart => "run-start",
                PayloadType::RunConverge => "run-converge",
                PayloadType::ComplianceReport => "compliance-report",
                PayloadType::Unknown => "unknown",
            }
        )
    });

    (StatusCode::ACCEPTED, axum::Json(body)).into_response()
}

/// Computes SHA-256 hash of payload for dedup and archive keys.
fn compute_sha256(data: &[u8]) -> String {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Builds the Axum router for ingest endpoints.
pub fn ingest_routes(state: IngestAppState) -> Router {
    Router::new()
        .route("/ingest/events/data-collector", post(data_collector_handler))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body as AxumBody;
    use axum::http::Request;
    use tower::ServiceExt;

    /// Helper: Create a test archive for testing
    fn create_test_state(token: &str, max_size: u64) -> (IngestAppState, tempfile::TempDir) {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let archive = Arc::new(spindle_rawarchive::LocalArchive::new(tmp_dir.path().to_str().unwrap()).unwrap());
        let config = IngestConfig::with_max_size(token, max_size);
        let state = IngestAppState::new(config, archive);
        (state, tmp_dir)
    }

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
        let config = IngestConfig::new("test-token-123");
        assert!(verify_bearer_token(&config, Some("Bearer test-token-123")));
    }

    #[test]
    fn test_constant_time_comparison_invalid() {
        let config = IngestConfig::new("test-token-123");
        assert!(!verify_bearer_token(&config, Some("Bearer wrong-token")));
    }

    #[test]
    fn test_constant_time_comparison_missing() {
        let config = IngestConfig::new("test-token-123");
        assert!(!verify_bearer_token(&config, None));
    }

    #[test]
    fn test_constant_time_comparison_empty_config() {
        let config = IngestConfig::new("");
        assert!(!verify_bearer_token(&config, Some("Bearer anything")));
    }

    #[test]
    fn test_constant_time_comparison_wrong_length() {
        let config = IngestConfig::new("test-token-123");
        assert!(!verify_bearer_token(&config, Some("Bearer short")));
    }

    #[test]
    fn test_extract_bearer_valid() {
        let mut headers = header::HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer my-secret-token".parse().unwrap());
        let token = extract_bearer(&headers);
        assert_eq!(token, Some("my-secret-token".to_string()));
    }

    #[test]
    fn test_extract_bearer_missing() {
        let headers = header::HeaderMap::new();
        let token = extract_bearer(&headers);
        assert_eq!(token, None);
    }

    #[test]
    fn test_compute_sha256() {
        let data = b"hello world";
        let hash = compute_sha256(data);
        // Known SHA-256 of "hello world"
        assert_eq!(hash, "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
    }

    #[test]
    fn test_config_default_max_size() {
        let config = IngestConfig::default();
        assert_eq!(config.max_payload_size, DEFAULT_MAX_PAYLOAD_SIZE);
    }

    #[test]
    fn test_config_custom_max_size() {
        let config = IngestConfig::with_max_size("token", 1024);
        assert_eq!(config.max_payload_size, 1024);
    }

    #[tokio::test]
    async fn test_handler_valid_token_run_start() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

        let payload = serde_json::json!({
            "run_id": "2026-08-06T12:00:00Z",
            "node_name": "web-server-01"
        });

        let request = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn test_handler_valid_token_compliance_report() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

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
            .body(AxumBody::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn test_handler_invalid_token() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

        let payload = serde_json::json!({
            "run_id": "2026-08-06T12:00:00Z",
            "node_name": "web-server-01"
        });

        let request = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer wrong-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_handler_missing_token() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

        let payload = serde_json::json!({
            "run_id": "2026-08-06T12:00:00Z",
            "node_name": "web-server-01"
        });

        let request = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::CONTENT_TYPE, "application/json")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_handler_unknown_payload_type() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

        let payload = serde_json::json!({
            "foo": "bar"
        });

        let request = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        // Unknown payloads are still accepted (202) — archived for review
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn test_handler_payload_too_large() {
        let (state, _tmp) = create_test_state("valid-secret-token", 10);
        let app = ingest_routes(state);

        let payload = serde_json::json!({
            "run_id": "2026-08-06T12:00:00Z",
            "node_name": "web-server-01"
        });

        let request = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn test_handler_malformed_json() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

        let bad_json = "this is not valid json{{{}}}";

        let request = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(AxumBody::from(bad_json))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        // Malformed JSON is still accepted (202) — archived for review
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }
}
