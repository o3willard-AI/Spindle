//! Ingest HTTP endpoint for Chef Infra data-collector events.
//!
//! # Usage
//! ```ignore
//! use spindle_server::ingest::{IngestConfig, IngestAppState};
//! use std::sync::Arc;
//!
//! let state = IngestAppState::new(
//!     IngestConfig::new("super-secret-token"),
//!     Arc::new(LocalArchive::new("/var/lib/spindle/archive")?),
//!     Arc::new(InMemoryIdempotencyStore::new()),
//!     DEFAULT_MAX_INGEST_LAG_SECONDS * 2, // TTL
//! );
//! let app = ingest_routes(state);
//! ```
//!
//! ## Endpoints
//! - `POST /ingest/events/data-collector` — accepts Chef Infra data-collector payloads
//!
//! ## Processing pipeline
//! 1. Validate payload size (≤ max_size)
//! 2. Check idempotency key — if duplicate, return 202 but skip enqueue (replay is normal)
//! 3. Write verbatim payload to raw archive (write-before-parse)
//! 4. Enqueue for async processing (Postgres-backed job queue)
//! 5. Return 202 with receipt token
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
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use subtle::ConstantTimeEq;
use uuid::Uuid;

use spindle_rawarchive::{Archive, ArchiveMetadata};

/// Maximum payload size in bytes (32 MB default).
pub const DEFAULT_MAX_PAYLOAD_SIZE: u64 = 32 * 1024 * 1024; // 32 MB

/// Default max ingest lag in seconds for TTL calculation.
/// TTL = max_ingest_lag × 2 (default: 300 × 2 = 600s = 10 minutes)
pub const DEFAULT_MAX_INGEST_LAG_SECONDS: u64 = 300;

/// Configuration for the ingest endpoint.
/// Token is compared using constant-time comparison to prevent timing attacks.
#[derive(Debug, Clone)]
pub struct IngestConfig {
    /// The expected bearer token for authentication.
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

/// Idempotency key derived from payload structure.
/// Key components: (chef_server_url, organization, node_name, run_id, message_type)
/// chef_server_url and organization are optional (may be absent in data-collector payloads).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct IdempotencyKey {
    pub chef_server_url: Option<String>,
    pub organization: Option<String>,
    pub node_name: String,
    pub run_id: String,
    pub message_type: MessageType,
}

impl IdempotencyKey {
    /// Extract the idempotency key from a parsed JSON payload and message type.
    pub fn from_json(payload: &Value, msg_type: MessageType) -> Option<Self> {
        let obj = payload.as_object()?;

        let node_name = obj.get("node_name")
            .and_then(|v| v.as_str())
            .or_else(|| obj.get("id").and_then(|v| v.as_str()))
            .map(|s| s.to_string())?;

        let run_id = obj.get("run_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())?;

        let chef_server_url = obj.get("chef_server_url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let organization = obj.get("organization")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Some(IdempotencyKey {
            chef_server_url,
            organization,
            node_name,
            run_id,
            message_type: msg_type,
        })
    }
}

impl std::fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}|{}|{}|{}|{}",
            self.chef_server_url.as_deref().unwrap_or(""),
            self.organization.as_deref().unwrap_or(""),
            self.node_name,
            self.run_id,
            self.message_type
        )
    }
}

/// Message type derived from payload structure.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum MessageType {
    RunStart,
    RunConverge,
    ComplianceReport,
}

impl std::fmt::Display for MessageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageType::RunStart => write!(f, "run-start"),
            MessageType::RunConverge => write!(f, "run-converge"),
            MessageType::ComplianceReport => write!(f, "compliance-report"),
        }
    }
}

/// Trait for idempotency storage backends.
/// Implementations should be thread-safe and provide O(1) lookups.
pub trait IdempotencyStore: Send + Sync + std::fmt::Debug {
    /// Check if a key has been seen before (key-level check).
    /// Returns Some(receipt_token) if duplicate, None if fresh.
    fn check_duplicate(&self, key: &IdempotencyKey, payload_sha256: &str) -> Option<String>;

    /// Check if a payload (by SHA256) has been seen before (payload-level check).
    /// This is used for malformed payloads where key extraction may not be possible.
    fn check_duplicate_by_sha(&self, payload_sha256: &str) -> Option<String>;

    /// Record a new key-level entry (first sighting).
    fn record(&self, key: &IdempotencyKey, payload_sha256: &str, receipt: &str);

    /// Record a payload-level entry by SHA256 (for malformed/duplicate detection).
    fn record_by_sha(&self, payload_sha256: &str, receipt: &str);

    /// Record a key-level entry (alias for record — for clarity in handler).
    fn record_key(&self, key: &IdempotencyKey, payload_sha256: &str, receipt: &str) {
        self.record(key, payload_sha256, receipt);
    }

    /// Report a duplicate (increment counter, update timestamp).
    fn report_duplicate(&self, key: &IdempotencyKey);
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
    Duplicate(ReceiptToken),
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
            IngestResponse::Duplicate(receipt) => {
                let body = serde_json::json!({
                    "status": "duplicate",
                    "receipt_token": receipt.to_string(),
                    "message": "Duplicate payload — already processed"
                });
                // 202 not 409 — replay is normal
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
/// Contains the config, raw archive, idempotency store, and TTL.
#[derive(Debug, Clone)]
pub struct IngestAppState {
    pub config: IngestConfig,
    pub archive: Arc<dyn Archive>,
    pub idempotency: Arc<dyn IdempotencyStore>,
    pub ttl_seconds: u64,
}

impl IngestAppState {
    pub fn new(
        config: IngestConfig,
        archive: Arc<dyn Archive>,
        idempotency: Arc<dyn IdempotencyStore>,
        ttl_seconds: u64,
    ) -> Self {
        Self {
            config,
            archive,
            idempotency,
            ttl_seconds,
        }
    }
}

/// Handler for POST /ingest/events/data-collector
///
/// Processing pipeline:
/// 1. Validate payload size (≤ max_size)
/// 2. Compute payload SHA-256 for idempotency check
/// 3. Check payload-level idempotency (by SHA256) — if duplicate, return 202
/// 4. Write verbatim payload to raw archive (write-before-parse)
/// 5. Attempt JSON parse + idempotency key extraction
/// 6. If parse fails → archive raw bytes, record malformed_payloads entry, return 202
/// 7. If parse succeeds but type unknown → still archive and return 202
/// 8. If parse succeeds and type known → record idempotency key, return 202
/// 9. Archive write fails → 503
/// Error messages NEVER leak payload content
pub async fn data_collector_handler(
    State(state): State<IngestAppState>,
    headers: header::HeaderMap,
    request_body: axum::body::Body,
) -> Response {
    let start = Instant::now();

    // Step 1: Extract and verify bearer token (constant-time)
    let auth_header = headers.get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    if !verify_bearer_token(&state.config, auth_header) {
        tracing::warn!("Unauthorized ingest attempt — token mismatch");
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }

    // Step 2: Read body as bytes (for size validation and verbatim archiving)
    let payload_bytes = match axum::body::to_bytes(request_body, state.config.max_payload_size as usize).await {
        Ok(bytes) => bytes,
        Err(_) => {
            tracing::warn!("Payload exceeds max size — rejected");
            return (StatusCode::PAYLOAD_TOO_LARGE, "Payload too large").into_response();
        }
    };

    // Step 3: Validate payload size
    if payload_bytes.len() as u64 > state.config.max_payload_size {
        tracing::warn!(
            payload_size = payload_bytes.len(),
            max_size = state.config.max_payload_size,
            "Payload exceeds size limit"
        );
        let body = serde_json::json!({
            "status": "payload_too_large",
            "error": "Payload exceeds maximum allowed size"
        });
        return (StatusCode::PAYLOAD_TOO_LARGE, axum::Json(body)).into_response();
    }

    let payload_sha = compute_sha256(&payload_bytes);
    let token_id = extract_bearer(&headers).unwrap_or_else(|| "unknown".to_string());
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();

    // Step 4: Check payload-level idempotency (by SHA256)
    // If this exact payload was already seen, return 202 with original receipt
    if let Some(existing_receipt) = state.idempotency.check_duplicate_by_sha(&payload_sha) {
        let elapsed = start.elapsed();
        tracing::info!(
            original_receipt = %existing_receipt,
            total_latency_ms = %elapsed.as_millis(),
            "Duplicate payload (by SHA256) detected — returning original receipt"
        );
        tracing::warn!(metric = "spindle_ingest_duplicate_count", "increment");

        let body = serde_json::json!({
            "status": "duplicate",
            "receipt_token": existing_receipt,
            "message": "Duplicate payload — already processed"
        });
        return (StatusCode::ACCEPTED, axum::Json(body)).into_response();
    }

    // Step 5: Write verbatim payload to raw archive (write-before-parse)
    let metadata = ArchiveMetadata::new(
        payload_sha.clone(),
        content_type,
        token_id,
        chrono::Utc::now(),
    );

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
            tracing::warn!(metric = "spindle_archive_write_seconds", error = %e, "archive_write_failed");

            // Error message sanitized — no payload content leaked
            let body = serde_json::json!({
                "status": "service_unavailable",
                "error": "Archive write failed"
            });
            return (StatusCode::SERVICE_UNAVAILABLE, axum::Json(body)).into_response();
        }
    };

    let archive_elapsed = archive_start.elapsed();
    tracing::info!(
        archive_key = %archive_key,
        archive_write_ms = %archive_elapsed.as_millis(),
        metric = "spindle_archive_write_seconds",
        value = %archive_elapsed.as_secs_f64(),
        "archive_write_latency"
    );

    // Step 6: Attempt JSON parse
    let receipt = ReceiptToken::new();

    let payload_json: Value = match serde_json::from_slice(&payload_bytes) {
        Ok(v) => v,
        Err(parse_err) => {
            // Parse failure → archive already done above, now record malformed payload
            // Sanitized error: only the error category, NOT the payload content
            let error_category = "parse_error";
            let error_summary = sanitize_error_message(&parse_err);

            tracing::warn!(
                error_category = error_category,
                error_summary = %error_summary,
                archive_key = %archive_key,
                receipt = %receipt,
                "Malformed payload (JSON parse failure) — archived, returning 202"
            );

            // Record idempotency by SHA256 so duplicates are caught
            state.idempotency.record_by_sha(&payload_sha, &receipt.to_string());

            // Track malformed count metric
            tracing::warn!(metric = "spindle_ingest_malformed_count", category = error_category, "malformed_payload");

            let elapsed = start.elapsed();
            tracing::info!(
                payload_size = payload_bytes.len(),
                receipt = %receipt,
                total_latency_ms = %elapsed.as_millis(),
                archive_key = %archive_key,
                "Malformed payload acknowledged (202)"
            );

            let body = serde_json::json!({
                "status": "accepted",
                "receipt_token": receipt.to_string(),
                "archive_key": archive_key,
                "message": "Malformed payload archived — awaiting manual review"
            });
            return (StatusCode::ACCEPTED, axum::Json(body)).into_response();
        }
    };

    // Step 7: Detect payload type and extract idempotency key
    let payload_type = detect_payload_type(&payload_json);

    if payload_type == PayloadType::Unknown {
        // Valid JSON but unknown structure — still archive (done), record as unknown type
        tracing::warn!(
            archive_key = %archive_key,
            receipt = %receipt,
            "Unknown payload type — valid JSON but unrecognized structure"
        );

        // Record idempotency by SHA256
        state.idempotency.record_by_sha(&payload_sha, &receipt.to_string());

        // Track malformed count metric (unknown type counts as malformed)
        tracing::warn!(metric = "spindle_ingest_malformed_count", category = "unknown_structure", "malformed_payload");

        let body = serde_json::json!({
            "status": "accepted",
            "receipt_token": receipt.to_string(),
            "archive_key": archive_key,
            "message": "Unknown payload structure archived — awaiting review"
        });
        return (StatusCode::ACCEPTED, axum::Json(body)).into_response();
    }

    // Convert PayloadType to MessageType for idempotency key
    let msg_type = match payload_type {
        PayloadType::RunStart => MessageType::RunStart,
        PayloadType::RunConverge => MessageType::RunConverge,
        PayloadType::ComplianceReport => MessageType::ComplianceReport,
        PayloadType::Unknown => unreachable!(),
    };

    // Extract idempotency key from payload
    let idempotency_key = IdempotencyKey::from_json(&payload_json, msg_type);

    // Step 8: Record idempotency key (payload-level + key-level for proper dedup)
    state.idempotency.record_by_sha(&payload_sha, &receipt.to_string());

    if let Some(ref key) = idempotency_key {
        state.idempotency.record_key(key, &payload_sha, &receipt.to_string());
    } else {
        tracing::warn!("Could not extract idempotency key from payload — using SHA256 only");
    }

    let elapsed = start.elapsed();
    tracing::info!(
        payload_type = ?payload_type,
        idempotency_key = ?idempotency_key,
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
        "message": format!("{} payload received, archived, and queued for processing", msg_type)
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

/// Sanitizes error messages to prevent payload content leakage.
/// Extracts only the error category (e.g., "expected value", "invalid escape", etc.)
/// and removes any reference to payload bytes, offset, or line content.
fn sanitize_error_message(err: &serde_json::Error) -> String {
    let err_str = err.to_string();
    // serde_json errors look like: "expected value at line 1 column 1"
    // We extract the category part before "at line"
    if let Some(pos) = err_str.rfind(" at ") {
        let category = &err_str[..pos];
        // Remove any potential payload content that might be in the error
        // (serde_json can include line/byte info, but not actual payload content)
        format!("{}", category)
    } else {
        // Fallback: return a generic category
        "parse_error".to_string()
    }
}

/// Builds the Axum router for ingest endpoints.
pub fn ingest_routes(state: IngestAppState) -> Router {
    Router::new()
        .route("/ingest/events/data-collector", post(data_collector_handler))
        .with_state(state)
}

// ============================================================================
// In-memory implementations for testing
// ============================================================================

/// Thread-safe in-memory idempotency store for testing and single-node deployments.
/// Maintains two maps:
/// - `sha_store`: payload SHA256 → receipt (catches byte-identical duplicates, including malformed)
/// - `key_store`: idempotency key string → receipt (catches logical duplicates)
#[derive(Debug, Default)]
pub struct InMemoryIdempotencyStore {
    inner: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

impl InMemoryIdempotencyStore {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

impl IdempotencyStore for InMemoryIdempotencyStore {
    fn check_duplicate(&self, key: &IdempotencyKey, payload_sha256: &str) -> Option<String> {
        let store = self.inner.lock().unwrap();
        // Check both key-level and SHA-level stores
        let key_str = key.to_string();
        store.get(&format!("key:{}", key_str))
            .or_else(|| store.get(&format!("sha:{}", payload_sha256)))
            .cloned()
    }

    fn check_duplicate_by_sha(&self, payload_sha256: &str) -> Option<String> {
        let store = self.inner.lock().unwrap();
        store.get(&format!("sha:{}", payload_sha256)).cloned()
    }

    fn record(&self, key: &IdempotencyKey, payload_sha256: &str, receipt: &str) {
        let mut store = self.inner.lock().unwrap();
        // Store under both key and SHA for flexible lookup
        store.insert(format!("key:{}", key.to_string()), receipt.to_string());
        store.insert(format!("sha:{}", payload_sha256), receipt.to_string());
    }

    fn record_by_sha(&self, payload_sha256: &str, receipt: &str) {
        let mut store = self.inner.lock().unwrap();
        store.insert(format!("sha:{}", payload_sha256), receipt.to_string());
    }

    fn report_duplicate(&self, _key: &IdempotencyKey) {
        // In-memory store doesn't track counts — Postgres store does
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body as AxumBody;
    use axum::http::Request;
    use tower::ServiceExt;

    /// Helper: Create a test app state with in-memory archive and idempotency store
    fn create_test_state(token: &str, max_size: u64) -> (IngestAppState, tempfile::TempDir) {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let archive = Arc::new(spindle_rawarchive::LocalArchive::new(tmp_dir.path().to_str().unwrap()).unwrap());
        let idempotency = Arc::new(InMemoryIdempotencyStore::new());
        let config = IngestConfig::with_max_size(token, max_size);
        let state = IngestAppState::new(config, archive, idempotency, DEFAULT_MAX_INGEST_LAG_SECONDS * 2);
        (state, tmp_dir)
    }

    /// Helper: Create a run-start payload
    fn make_run_start() -> Value {
        serde_json::json!({
            "run_id": "2026-08-06T12:00:00Z",
            "node_name": "web-server-01",
            "chef_version": "18.0.0",
            "chef_server_url": "https://chef.example.com",
            "organization": "prod"
        })
    }

    /// Helper: Create a run-converge payload
    fn make_run_converge() -> Value {
        serde_json::json!({
            "run_id": "2026-08-06T12:00:00Z",
            "node_name": "web-server-01",
            "resources": [
                {
                    "type": "package",
                    "name": "nginx",
                    "status": "updated"
                }
            ]
        })
    }

    /// Helper: Create a compliance-report payload
    fn make_compliance_report() -> Value {
        serde_json::json!({
            "profiles": [
                {
                    "name": "ssh-baseline",
                    "controls": []
                }
            ]
        })
    }

    // === Payload type detection tests ===

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

    // === Idempotency key tests ===

    #[test]
    fn test_idempotency_key_from_json_run_start() {
        let json = make_run_start();
        let key = IdempotencyKey::from_json(&json, MessageType::RunStart).unwrap();
        assert_eq!(key.node_name, "web-server-01");
        assert_eq!(key.run_id, "2026-08-06T12:00:00Z");
        assert_eq!(key.chef_server_url, Some("https://chef.example.com".to_string()));
        assert_eq!(key.organization, Some("prod".to_string()));
        assert_eq!(key.message_type, MessageType::RunStart);
    }

    #[test]
    fn test_idempotency_key_from_json_run_converge() {
        let json = make_run_converge();
        let key = IdempotencyKey::from_json(&json, MessageType::RunConverge).unwrap();
        assert_eq!(key.node_name, "web-server-01");
        assert_eq!(key.run_id, "2026-08-06T12:00:00Z");
        assert_eq!(key.organization, None);
        assert_eq!(key.message_type, MessageType::RunConverge);
    }

    #[test]
    fn test_idempotency_key_missing_node_name() {
        let json = serde_json::json!({
            "run_id": "2026-08-06T12:00:00Z",
            "chef_version": "18.0.0"
        });
        assert!(IdempotencyKey::from_json(&json, MessageType::RunStart).is_none());
    }

    #[test]
    fn test_idempotency_key_display() {
        let json = make_run_start();
        let key = IdempotencyKey::from_json(&json, MessageType::RunStart).unwrap();
        let s = key.to_string();
        assert!(s.contains("web-server-01"));
        assert!(s.contains("2026-08-06T12:00:00Z"));
        assert!(s.contains("run-start"));
    }

    // === Constant-time token comparison tests ===

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

    // === Bearer extraction tests ===

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

    // === Receipt token tests ===

    #[test]
    fn test_receipt_token_format() {
        let receipt = ReceiptToken::new();
        let s = receipt.to_string();
        assert!(s.starts_with("receipt:"));
        let uuid_str = &s[8..];
        assert!(uuid::Uuid::parse_str(uuid_str).is_ok());
    }

    // === Config tests ===

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

    // === SHA256 tests ===

    #[test]
    fn test_compute_sha256() {
        let data = b"hello world";
        let hash = compute_sha256(data);
        assert_eq!(hash, "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
    }

    // === Route building tests ===

    #[test]
    fn test_ingest_routes_builds() {
        let config = IngestConfig::new("test");
        let archive = Arc::new(spindle_rawarchive::LocalArchive::new("/tmp").unwrap());
        let idempotency = Arc::new(InMemoryIdempotencyStore::new());
        let state = IngestAppState::new(config, archive, idempotency, 600);
        let _app = ingest_routes(state);
    }

    // === HTTP integration tests ===

    #[tokio::test]
    async fn test_handler_valid_token_run_start() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

        let payload = make_run_start();

        let request = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        // Verify response body contains receipt token
        let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "accepted");
        assert!(json["receipt_token"].as_str().unwrap().starts_with("receipt:"));
    }

    #[tokio::test]
    async fn test_handler_valid_token_compliance_report() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

        let payload = make_compliance_report();

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

        let payload = make_run_start();

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

        let payload = make_run_start();

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

        let payload = serde_json::json!({"foo": "bar"});

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
    async fn test_handler_payload_too_large() {
        let (state, _tmp) = create_test_state("valid-secret-token", 10);
        let app = ingest_routes(state);

        let payload = serde_json::json!({"run_id": "r1", "node_name": "n1"});

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
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn test_handler_malformed_json_duplicate_detected() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

        let bad_json = "this is not valid json{{{}}}";

        // First malformed request
        let request1 = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(AxumBody::from(bad_json))
            .unwrap();
        let response1 = app.clone().oneshot(request1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::ACCEPTED);

        let body1 = axum::body::to_bytes(response1.into_body(), 4096).await.unwrap();
        let json1: Value = serde_json::from_slice(&body1).unwrap();
        let receipt1 = json1["receipt_token"].as_str().unwrap().to_string();

        // Second identical malformed request — should be duplicate
        let request2 = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(AxumBody::from(bad_json))
            .unwrap();
        let response2 = app.oneshot(request2).await.unwrap();
        assert_eq!(response2.status(), StatusCode::ACCEPTED);

        let body2 = axum::body::to_bytes(response2.into_body(), 4096).await.unwrap();
        let json2: Value = serde_json::from_slice(&body2).unwrap();
        // Duplicate detection via SHA256 should catch it
        assert_eq!(json2["status"], "duplicate");
        assert_eq!(json2["receipt_token"].as_str().unwrap(), receipt1);
    }

    #[tokio::test]
    async fn test_handler_missing_required_fields_acknowledged() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

        // Valid JSON but missing node_name and run_id — can't extract idempotency key
        let payload = serde_json::json!({
            "chef_version": "18.0.0",
            "resources": []
        });

        let request = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        // Should be 202 — acknowledged, archived, but no idempotency key extracted
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "accepted");
    }

    #[tokio::test]
    async fn test_handler_missing_required_fields_duplicate_detected() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

        // Valid JSON but missing node_name and run_id
        let payload = serde_json::json!({
            "chef_version": "18.0.0",
            "resources": []
        });

        // First request
        let request1 = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();
        let response1 = app.clone().oneshot(request1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::ACCEPTED);

        // Second identical request — should be duplicate (detected via SHA256)
        let request2 = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();
        let response2 = app.oneshot(request2).await.unwrap();
        assert_eq!(response2.status(), StatusCode::ACCEPTED);

        let body2 = axum::body::to_bytes(response2.into_body(), 4096).await.unwrap();
        let json2: Value = serde_json::from_slice(&body2).unwrap();
        assert_eq!(json2["status"], "duplicate");
    }

    #[test]
    fn test_sanitize_error_message() {
        // Test that sanitize_error_message extracts category without payload content
        let bad_json = "this is not valid json{{{}}}";
        let parse_result: Result<Value, _> = serde_json::from_str(bad_json);
        assert!(parse_result.is_err());

        let err = parse_result.unwrap_err();
        let sanitized = sanitize_error_message(&err);

        // Should contain "expected" (the category) but NOT the payload content
        assert!(sanitized.contains("expected"));
        // The sanitized message should NOT contain the raw payload
        assert!(!sanitized.contains("this is not valid json"));
    }

    #[test]
    fn test_in_memory_idempotency_store() {
        let store = InMemoryIdempotencyStore::new();
        let key = IdempotencyKey {
            chef_server_url: Some("https://chef.example.com".to_string()),
            organization: Some("prod".to_string()),
            node_name: "web-server-01".to_string(),
            run_id: "run-123".to_string(),
            message_type: MessageType::RunStart,
        };

        let sha = "abc123def456";

        // Fresh key — no duplicate
        assert!(store.check_duplicate(&key, sha).is_none());
        assert!(store.check_duplicate_by_sha(sha).is_none());

        // Record it
        store.record(&key, sha, "receipt:123");

        // Now it should be detected as duplicate
        assert_eq!(store.check_duplicate(&key, sha), Some("receipt:123".to_string()));
        assert_eq!(store.check_duplicate_by_sha(sha), Some("receipt:123".to_string()));
    }

    #[test]
    fn test_in_memory_idempotency_store_record_by_sha() {
        let store = InMemoryIdempotencyStore::new();
        let sha = "xyz789";

        // Record by SHA (for malformed payloads)
        store.record_by_sha(sha, "receipt:456");

        // Should be detected as duplicate by SHA
        assert_eq!(store.check_duplicate_by_sha(sha), Some("receipt:456".to_string()));
    }

    // === Idempotency integration tests ===

    #[tokio::test]
    async fn test_duplicate_payload_returns_202_with_original_receipt() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

        let payload = make_run_start();

        // First request
        let request1 = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();

        let response1 = app.clone().oneshot(request1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::ACCEPTED);

        let body1 = axum::body::to_bytes(response1.into_body(), 4096).await.unwrap();
        let json1: Value = serde_json::from_slice(&body1).unwrap();
        let receipt1 = json1["receipt_token"].as_str().unwrap().to_string();
        assert_eq!(json1["status"], "accepted");

        // Second identical request — should be duplicate
        let request2 = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();

        let response2 = app.oneshot(request2).await.unwrap();
        assert_eq!(response2.status(), StatusCode::ACCEPTED);

        let body2 = axum::body::to_bytes(response2.into_body(), 4096).await.unwrap();
        let json2: Value = serde_json::from_slice(&body2).unwrap();
        assert_eq!(json2["status"], "duplicate");
        // Duplicate should return the SAME receipt token
        assert_eq!(json2["receipt_token"].as_str().unwrap(), receipt1);
    }

    #[tokio::test]
    async fn test_different_run_ids_not_duplicated() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

        // First request with run_id 1
        let payload1 = serde_json::json!({
            "run_id": "run-id-1",
            "node_name": "web-server-01"
        });
        let request1 = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(AxumBody::from(payload1.to_string()))
            .unwrap();
        let response1 = app.clone().oneshot(request1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::ACCEPTED);

        // Second request with different run_id
        let payload2 = serde_json::json!({
            "run_id": "run-id-2",
            "node_name": "web-server-01"
        });
        let request2 = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(AxumBody::from(payload2.to_string()))
            .unwrap();
        let response2 = app.oneshot(request2).await.unwrap();
        assert_eq!(response2.status(), StatusCode::ACCEPTED);

        let body2 = axum::body::to_bytes(response2.into_body(), 4096).await.unwrap();
        let json2: Value = serde_json::from_slice(&body2).unwrap();
        assert_eq!(json2["status"], "accepted"); // Not a duplicate
    }

    #[tokio::test]
    async fn test_different_message_types_not_duplicated() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

        let node_name = "web-server-01";
        let run_id = "2024-01-01T00:00:00Z";

        // run-start
        let payload1 = serde_json::json!({
            "run_id": run_id,
            "node_name": node_name
        });
        let request1 = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .body(AxumBody::from(payload1.to_string()))
            .unwrap();
        let _ = app.clone().oneshot(request1).await.unwrap();

        // run-converge (same node_name + run_id, but has "resources")
        let payload2 = serde_json::json!({
            "run_id": run_id,
            "node_name": node_name,
            "resources": []
        });
        let request2 = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .body(AxumBody::from(payload2.to_string()))
            .unwrap();
        let response2 = app.oneshot(request2).await.unwrap();
        assert_eq!(response2.status(), StatusCode::ACCEPTED);

        let body2 = axum::body::to_bytes(response2.into_body(), 4096).await.unwrap();
        let json2: Value = serde_json::from_slice(&body2).unwrap();
        assert_eq!(json2["status"], "accepted"); // Different message type = not a duplicate
    }
}
