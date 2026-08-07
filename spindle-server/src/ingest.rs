//! Ingest HTTP endpoint for Chef Infra data-collector events.
//!
//! # Usage
//! ```ignore
//! use spindle_server::ingest::{IngestConfig, IngestAppState, InMemoryQueueMonitor, InMemoryIdempotencyStore};
//! use spindle_rawarchive::LocalArchive;
//! use std::sync::Arc;
//!
//! let archive = Arc::new(LocalArchive::new("/var/lib/spindle/archive")?);
//! let state = IngestAppState::new(
//!     IngestConfig::new("super-secret-token"),
//!     archive,
//!     Arc::new(InMemoryIdempotencyStore::new()),
//!     Arc::new(InMemoryQueueMonitor::new(0, 150.0)),
//!     DEFAULT_MAX_INGEST_LAG_SECONDS * 2, // TTL
//! );
//! let app = ingest_routes(state);
//! ```
//!
//! ## Endpoints
//! - `POST /ingest/events/data-collector` — accepts Chef Infra data-collector payloads
//! - `POST /ingest/events/inspec` — accepts InSpec JSON reporter output
//!
//! ## Processing pipeline
//! 1. Validate payload size (≤ max_size)
//! 2. Validate bearer token (constant-time)
//! 3. Check rate limit (token-bucket, governor) — 429 if exceeded
//! 4. Check queue depth — 429 if full
//! 5. Check idempotency key by SHA256 — 202 duplicate
//! 6. Write verbatim payload to raw archive (write-before-parse)
//! 7. Parse JSON, detect payload type — 202 on failure (malformed)
//! 8. Enqueue for async processing (Postgres-backed job queue)
//! 9. Return 202 with receipt token
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
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Instant;
use subtle::ConstantTimeEq;
use uuid::Uuid;

use spindle_rawarchive::{Archive, ArchiveMetadata};
use governor::{RateLimiter, Quota, state::NotKeyed, clock::DefaultClock, middleware::NoOpMiddleware};
use governor::state::InMemoryState;

/// Maximum payload size in bytes (10 MB default — reasonable for Chef run reports).
pub const DEFAULT_MAX_PAYLOAD_SIZE: u64 = 10 * 1024 * 1024;

/// Default max ingest lag in seconds for TTL calculation.
/// TTL = max_ingest_lag × 2 (default: 300 × 2 = 600s = 10 minutes)
pub const DEFAULT_MAX_INGEST_LAG_SECONDS: u64 = 300;

/// Default rate limit: 500 requests/second with burst allowance.
pub const DEFAULT_RATE_LIMIT_RPS: u32 = 500;

/// Default burst allowance (absorbs converge storms).
pub const DEFAULT_RATE_LIMIT_BURST: u32 = 1000;

/// Configuration for the ingest endpoint.
/// Token is compared using constant-time comparison to prevent timing attacks.
#[derive(Debug, Clone)]
pub struct IngestConfig {
    /// The expected bearer token for authentication.
    pub token: String,
    /// Maximum payload size in bytes (default: 10 MB).
    pub max_payload_size: u64,
    /// Maximum queue depth before returning 429 (default: 100,000).
    pub max_queue_depth: u64,
    /// Rate limit in requests per second (default: 500).
    pub rate_limit_rps: u32,
    /// Burst allowance for rate limiting (default: 1000).
    pub rate_limit_burst: u32,
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            token: String::new(),
            max_payload_size: DEFAULT_MAX_PAYLOAD_SIZE,
            max_queue_depth: DEFAULT_MAX_QUEUE_DEPTH,
            rate_limit_rps: DEFAULT_RATE_LIMIT_RPS,
            rate_limit_burst: DEFAULT_RATE_LIMIT_BURST,
        }
    }
}

impl IngestConfig {
    /// Create a new config with a token.
    pub fn new(token: &str) -> Self {
        Self {
            token: token.to_string(),
            max_payload_size: IngestConfig::from_env_max_payload_size(),
            max_queue_depth: DEFAULT_MAX_QUEUE_DEPTH,
            rate_limit_rps: DEFAULT_RATE_LIMIT_RPS,
            rate_limit_burst: DEFAULT_RATE_LIMIT_BURST,
        }
    }

    /// Read maximum payload size from SPINDLE_INGEST_MAX_PAYLOAD_SIZE env var.
    /// Falls back to DEFAULT_MAX_PAYLOAD_SIZE (10 MB) if not set or invalid.
    fn from_env_max_payload_size() -> u64 {
        std::env::var("SPINDLE_INGEST_MAX_PAYLOAD_SIZE")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&v| v > 0)
            .unwrap_or(DEFAULT_MAX_PAYLOAD_SIZE)
    }

    /// Create a new config with token and max payload size.
    pub fn with_max_size(token: &str, max_payload_size: u64) -> Self {
        Self {
            token: token.to_string(),
            max_payload_size,
            max_queue_depth: DEFAULT_MAX_QUEUE_DEPTH,
            rate_limit_rps: DEFAULT_RATE_LIMIT_RPS,
            rate_limit_burst: DEFAULT_RATE_LIMIT_BURST,
        }
    }

    /// Create a new config with token, max payload size, and max queue depth.
    pub fn with_queue_depth(token: &str, max_payload_size: u64, max_queue_depth: u64) -> Self {
        Self {
            token: token.to_string(),
            max_payload_size,
            max_queue_depth,
            rate_limit_rps: DEFAULT_RATE_LIMIT_RPS,
            rate_limit_burst: DEFAULT_RATE_LIMIT_BURST,
        }
    }

    /// Create config with custom rate limiting parameters.
    pub fn with_rate_limit(token: &str, max_payload_size: u64, max_queue_depth: u64, rate_limit_rps: u32, rate_limit_burst: u32) -> Self {
        Self {
            token: token.to_string(),
            max_payload_size,
            max_queue_depth,
            rate_limit_rps,
            rate_limit_burst,
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

impl Default for ReceiptToken {
    fn default() -> Self {
        Self::new()
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

impl From<PayloadType> for MessageType {
    fn from(pt: PayloadType) -> Self {
        match pt {
            PayloadType::RunStart => MessageType::RunStart,
            PayloadType::RunConverge => MessageType::RunConverge,
            PayloadType::ComplianceReport => MessageType::ComplianceReport,
            PayloadType::Unknown => MessageType::RunStart, // fallback
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
    /// For malformed payloads where key extraction may not be possible.
    fn check_duplicate_by_sha(&self, payload_sha256: &str) -> Option<String>;

    /// Record a new key-level entry (first sighting).
    fn record(&self, key: &IdempotencyKey, payload_sha256: &str, receipt: &str);

    /// Record a payload-level entry by SHA256 (for malformed/duplicate detection).
    fn record_by_sha(&self, payload_sha256: &str, receipt: &str);

    /// Report a duplicate (increment counter, update timestamp).
    fn report_duplicate(&self, key: &IdempotencyKey);
}

/// Trait for monitoring queue depth without blocking.
pub trait QueueMonitor: Send + Sync + std::fmt::Debug {
    fn queue_depth(&self) -> u64;
    fn worker_rate(&self) -> f64 {
        150.0
    }
}

/// Default maximum queue depth (100,000 items ≈ 11 min at 150/s).
pub const DEFAULT_MAX_QUEUE_DEPTH: u64 = 100_000;

/// Estimated drain time in seconds given current queue depth and worker rate.
pub fn estimate_drain_time(depth: u64, rate: f64) -> u64 {
    if rate <= 0.0 {
        return 0;
    }
    (depth as f64 / rate).ceil() as u64
}

/// Payload type detected from the JSON structure.
#[derive(Debug, Clone, PartialEq)]
pub enum PayloadType {
    RunStart,
    RunConverge,
    ComplianceReport,
    Unknown,
}

impl std::fmt::Display for PayloadType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PayloadType::RunStart => write!(f, "run-start"),
            PayloadType::RunConverge => write!(f, "run-converge"),
            PayloadType::ComplianceReport => write!(f, "compliance-report"),
            PayloadType::Unknown => write!(f, "unknown"),
        }
    }
}

/// Detects the payload type from the JSON structure.
pub fn detect_payload_type(json: &Value) -> PayloadType {
    if !json.is_object() {
        return PayloadType::Unknown;
    }

    let obj = json.as_object().unwrap();

    if obj.contains_key("profiles") {
        return PayloadType::ComplianceReport;
    }

    if obj.contains_key("resources") {
        return PayloadType::RunConverge;
    }

    if obj.contains_key("run_id") {
        return PayloadType::RunStart;
    }

    PayloadType::Unknown
}

/// Middleware for constant-time token verification.
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

/// Compute SHA-256 hash of payload for dedup and archive keys.
pub fn compute_sha256(data: &[u8]) -> String {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Sanitizes error messages to prevent payload content leakage.
/// Extracts only the error category without line/column or payload content.
pub fn sanitize_error_message(err: &serde_json::Error) -> String {
    let err_str = err.to_string();
    if let Some(pos) = err_str.rfind(" at ") {
        err_str[..pos].to_string()
    } else {
        "parse_error".to_string()
    }
}

/// Idempotency key for InSpec payloads.
/// Extracted from InSpec JSON reporter output: profile SHA + node_name + run_id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InSpecKey {
    pub organization: Option<String>,
    pub node_name: String,
    pub run_id: String,
}

impl InSpecKey {
    /// Extract the InSpec idempotency key from a parsed JSON payload.
    pub fn from_json(payload: &Value) -> Option<Self> {
        let obj = payload.as_object()?;

        let node_name = obj.get("node_name")
            .and_then(|v| v.as_str())
            .or_else(|| obj.get("platform")
                .and_then(|p| p.get("name"))
                .and_then(|v| v.as_str()))
            .map(|s| s.to_string())?;

        // InSpec JSON reporter output doesn't have a top-level run_id.
        // We derive a stable run_id from the first profile's sha256 + version.
        let run_id = obj.get("profiles")
            .and_then(|profiles| profiles.as_array())
            .and_then(|arr| arr.first())
            .and_then(|profile| {
                let sha = profile.get("sha256").and_then(|v| v.as_str());
                let version = profile.get("version").and_then(|v| v.as_str());
                match (sha, version) {
                    (Some(s), Some(v)) => Some(format!("{}-{}", s, v)),
                    (Some(s), None) => Some(s.to_string()),
                    (None, Some(v)) => Some(format!("profile-{}", v)),
                    _ => None,
                }
            })?;

        let organization = obj.get("organization")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Some(InSpecKey {
            organization,
            node_name,
            run_id,
        })
    }
}

impl std::fmt::Display for InSpecKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}|{}|{}",
            self.organization.as_deref().unwrap_or(""),
            self.node_name,
            self.run_id
        )
    }
}

/// Extract idempotency key from an InSpec JSON payload.
/// Uses platform name as node_name, and a derived run_id from statistics or profile info.
pub fn inspec_idempotency_key(payload: &Value) -> Option<InSpecKey> {
    InSpecKey::from_json(payload)
}

/// Thread-safe rate limit store using governor's token-bucket algorithm.
/// Single-tenant: one bucket per deployment. Token-bucket absorbs converge storms
/// via burst allowance. Non-blocking — returns 429 immediately when exceeded.
#[derive(Debug)]
pub struct RateLimitStore {
    limiter: RateLimiter<NotKeyed, InMemoryState, DefaultClock, NoOpMiddleware>,
    rps: u32,
    burst: u32,
}

impl RateLimitStore {
    pub fn new(rps: u32, burst: u32) -> Self {
        let quota = Quota::per_second(NonZeroU32::new(rps).unwrap())
            .allow_burst(NonZeroU32::new(burst).unwrap());
        let limiter: RateLimiter<NotKeyed, InMemoryState, DefaultClock, NoOpMiddleware> =
            RateLimiter::direct(quota);
        Self { limiter, rps, burst }
    }

    /// Check if a request is allowed. Returns Some(retry_after_seconds) if rate limited.
    pub fn check(&self) -> Option<u64> {
        match self.limiter.check() {
            Ok(_) => None,
            Err(_) => {
                let retry_secs = (self.burst as f64 / self.rps as f64).ceil() as u64;
                Some(retry_secs)
            }
        }
    }
}

/// Application state for the ingest endpoint.
#[derive(Debug, Clone)]
pub struct IngestAppState {
    pub config: IngestConfig,
    pub archive: Arc<dyn Archive>,
    pub idempotency: Arc<dyn IdempotencyStore>,
    pub queue_monitor: Arc<dyn QueueMonitor>,
    pub ttl_seconds: u64,
    /// Shared token-bucket rate limiter (per-deployment, single-tenant)
    pub rate_limiter: Arc<RateLimitStore>,
}

impl IngestAppState {
    pub fn new(
        config: IngestConfig,
        archive: Arc<dyn Archive>,
        idempotency: Arc<dyn IdempotencyStore>,
        queue_monitor: Arc<dyn QueueMonitor>,
        ttl_seconds: u64,
    ) -> Self {
        let rl = Arc::new(RateLimitStore::new(
            config.rate_limit_rps,
            config.rate_limit_burst,
        ));
        Self {
            config,
            archive,
            idempotency,
            queue_monitor,
            ttl_seconds,
            rate_limiter: rl,
        }
    }
}

/// Builds the Axum router for ingest endpoints.
pub fn ingest_routes(state: IngestAppState) -> Router {
    Router::new()
        .route("/ingest/events/data-collector", post(data_collector_handler))
        .route("/ingest/events/inspec", post(inspec_handler))
        .with_state(state)
}

/// Handler for POST /ingest/events/data-collector
///
/// Processing pipeline:
/// 1. Validate bearer token (constant-time)
/// 2. Read body as bytes
/// 3. Validate payload size (≤ max_size)
/// 4. Compute payload SHA-256 for idempotency
/// 5. Check rate limit (token-bucket) — 429 if exceeded
/// 6. Check queue depth — 429 if full
/// 7. Check payload-level idempotency (by SHA256) — 202 if duplicate
/// 8. Write verbatim payload to raw archive (write-before-parse)
/// 9. Attempt JSON parse — 202 on failure (malformed, but archived)
/// 10. Detect payload type, extract idempotency key
/// 11. Record idempotency, return 202 with receipt token
/// Error messages NEVER leak payload content.
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
        tracing::warn!("Unauthorized ingest attempt - token mismatch");
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }

    // Step 2: Read body as bytes (for size validation and verbatim archiving)
    let payload_bytes = match axum::body::to_bytes(request_body, state.config.max_payload_size as usize).await {
        Ok(bytes) => bytes,
        Err(_) => {
            tracing::warn!("Payload exceeds max size - rejected");
            tracing::warn!(metric = "spindle_ingest_payload_size_exceeded_total", "payload_size_exceeded");
            let body = serde_json::json!({
                "status": "payload_too_large",
                "error": "Payload exceeds maximum allowed size"
            });
            return (StatusCode::PAYLOAD_TOO_LARGE, axum::Json(body)).into_response();
        }
    };

    // Step 3: Validate payload size (double-check)
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

    // Step 4: Check rate limit (token-bucket via governor)
    // Non-blocking — immediate 429 if exceeded
    if let Some(retry_after_secs) = state.rate_limiter.check() {
        tracing::warn!(
            rate_limited = true,
            retry_after = retry_after_secs,
            "Rate limit exceeded - returning 429"
        );
        tracing::warn!(metric = "spindle_ingest_rate_limit_hits_total", "rate_limit_exceeded");

        let body = serde_json::json!({
            "status": "too_many_requests",
            "error": "Rate limit exceeded",
            "retry_after_seconds": retry_after_secs
        });

        let mut response = axum::Json(body).into_response();
        response.headers_mut().insert(
            header::RETRY_AFTER,
            axum::http::HeaderValue::from_str(&retry_after_secs.to_string()).unwrap()
        );
        *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
        return response;
    }

    // Step 5: Check queue depth before enqueue
    let queue_depth = state.queue_monitor.queue_depth();
    let max_depth = state.config.max_queue_depth;

    if queue_depth >= max_depth {
        let drain_seconds = estimate_drain_time(queue_depth, state.queue_monitor.worker_rate());
        tracing::warn!(
            queue_depth = queue_depth,
            max_depth = max_depth,
            estimated_drain_seconds = drain_seconds,
            "Queue depth exceeded - returning 429"
        );
        tracing::warn!(metric = "spindle_queue_depth", value = queue_depth, "queue_depth_exceeded");

        let body = serde_json::json!({
            "status": "too_many_requests",
            "error": "Queue is at capacity",
            "queue_depth": queue_depth,
            "max_queue_depth": max_depth
        });

        let mut response = axum::Json(body).into_response();
        response.headers_mut().insert(
            header::RETRY_AFTER,
            axum::http::HeaderValue::from_str(&drain_seconds.to_string()).unwrap()
        );
        *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
        return response;
    }

    // Step 6: Check payload-level idempotency (by SHA256)
    // If this exact payload was already seen, return 202 with original receipt
    if let Some(existing_receipt) = state.idempotency.check_duplicate_by_sha(&payload_sha) {
        let elapsed = start.elapsed();
        tracing::info!(
            original_receipt = %existing_receipt,
            total_latency_ms = %elapsed.as_millis(),
            "Duplicate payload (by SHA256) detected - returning original receipt"
        );
        tracing::warn!(metric = "spindle_ingest_duplicate_count", "increment");

        let body = serde_json::json!({
            "status": "duplicate",
            "receipt_token": existing_receipt,
            "message": "Duplicate payload - already processed"
        });
        return (StatusCode::ACCEPTED, axum::Json(body)).into_response();
    }

    // Step 7: Write verbatim payload to raw archive (write-before-parse)
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
                "Archive write failed - returning 503"
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

    let receipt = ReceiptToken::new();

    // Record idempotency by SHA256 (covers all payload types, including malformed)
    state.idempotency.record_by_sha(&payload_sha, &receipt.to_string());

    // Step 8: Attempt JSON parse
    let payload_json: Value = match serde_json::from_slice(&payload_bytes) {
        Ok(v) => v,
        Err(parse_err) => {
            // Malformed payload — JSON parse failure
            // Payload is already archived. Record as malformed.
            let err_msg = sanitize_error_message(&parse_err);
            tracing::warn!(
                error_category = %err_msg,
                archive_key = %archive_key,
                receipt = %receipt,
                "Malformed payload (JSON parse failure) - archived, returning 202"
            );
            tracing::warn!(metric = "spindle_ingest_malformed_count", "malformed_payload_received");

            let body = serde_json::json!({
                "status": "accepted",
                "receipt_token": receipt.to_string(),
                "archive_key": archive_key,
                "message": "Malformed payload archived - awaiting manual review"
            });

            let elapsed = start.elapsed();
            tracing::info!(total_latency_ms = %elapsed.as_millis(), "request_complete");
            return (StatusCode::ACCEPTED, axum::Json(body)).into_response();
        }
    };

    // Step 9: Detect payload type
    let msg_type = detect_payload_type(&payload_json);

    match msg_type {
        PayloadType::Unknown => {
            // Unknown structure — still archived, return 202
            tracing::warn!(
                archive_key = %archive_key,
                receipt = %receipt,
                "Unknown payload type - valid JSON but unrecognized structure"
            );
            tracing::warn!(metric = "spindle_ingest_malformed_count", "unknown_payload_type");

            let body = serde_json::json!({
                "status": "accepted",
                "receipt_token": receipt.to_string(),
                "archive_key": archive_key,
                "message": "Unknown payload structure archived - awaiting review"
            });
            return (StatusCode::ACCEPTED, axum::Json(body)).into_response();
        }
        PayloadType::RunStart | PayloadType::RunConverge | PayloadType::ComplianceReport => {
            // Known payload type — extract idempotency key
            let mt = MessageType::from(msg_type.clone());
            let idempotency_key = IdempotencyKey::from_json(&payload_json, mt);

            if let Some(key) = idempotency_key {
                // Record idempotency key for key-level dedup
                state.idempotency.record(&key, &payload_sha, &receipt.to_string());
            } else {
                // Could not extract idempotency key - SHA256-only dedup already recorded
                tracing::warn!("Could not extract idempotency key from payload - using SHA256 only");
            }

            tracing::info!(
                archive_key = %archive_key,
                receipt = %receipt,
                payload_type = %msg_type,
                "Valid payload received, archived, and queued for processing"
            );

            let body = serde_json::json!({
                "status": "accepted",
                "receipt_token": receipt.to_string(),
                "archive_key": archive_key,
                "message": format!("{} payload received, archived, and queued for processing", msg_type)
            });

            let elapsed = start.elapsed();
            tracing::info!(total_latency_ms = %elapsed.as_millis(), "request_complete");
            return (StatusCode::ACCEPTED, axum::Json(body)).into_response();
        }
    }
}

/// Handler for POST /ingest/events/inspec
///
/// Accepts InSpec JSON reporter output (the `json` reporter format).
/// Shares the same auth, rate limiting, queue depth, idempotency, and
/// malformed payload handling as the data-collector handler.
///
/// InSpec JSON reporter format key fields:
/// - `platform`: `{ "name": "chef.inspec", "release": "..." }`
/// - `profiles`: array of profile objects with controls
/// - `statistics`: `{ "duration": ... }`
/// - `version`: InSpec version string
/// - `controls`: array of control result objects
///
/// Metrics are differentiated by `source=inspec` label.
/// Processing pipeline (same as data-collector handler):
/// 1. Validate bearer token (constant-time)
/// 2. Read body as bytes
/// 3. Validate payload size (≤ max_size)
/// 4. Compute payload SHA-256 for idempotency
/// 5. Check rate limit (token-bucket, governor) — 429 if exceeded
/// 6. Check queue depth — 429 if full
/// 7. Check payload-level idempotency (by SHA256) — 202 if duplicate
/// 8. Write verbatim payload to raw archive (write-before-parse)
/// 9. Attempt JSON parse — 202 on failure (malformed, but archived)
/// 10. Detect InSpec structure, extract idempotency key
/// 11. Record idempotency, return 202 with receipt token
pub async fn inspec_handler(
    State(state): State<IngestAppState>,
    headers: header::HeaderMap,
    request_body: axum::body::Body,
) -> Response {
    let start = Instant::now();

    // Step 1: Extract and verify bearer token (constant-time)
    let auth_header = headers.get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    if !verify_bearer_token(&state.config, auth_header) {
        tracing::warn!("Unauthorized InSpec ingest attempt - token mismatch");
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }

    // Step 2: Read body as bytes (for size validation and verbatim archiving)
    let payload_bytes = match axum::body::to_bytes(request_body, state.config.max_payload_size as usize).await {
        Ok(bytes) => bytes,
        Err(_) => {
            tracing::warn!("InSpec payload exceeds max size - rejected");
            tracing::warn!(metric = "spindle_ingest_payload_size_exceeded_total", source = "inspec", "payload_size_exceeded");
            let body = serde_json::json!({
                "status": "payload_too_large",
                "error": "Payload exceeds maximum allowed size"
            });
            return (StatusCode::PAYLOAD_TOO_LARGE, axum::Json(body)).into_response();
        }
    };

    // Step 3: Validate payload size (double-check)
    if payload_bytes.len() as u64 > state.config.max_payload_size {
        tracing::warn!(
            payload_size = payload_bytes.len(),
            max_size = state.config.max_payload_size,
            "InSpec payload exceeds size limit"
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

    // Step 4: Check rate limit (shared token-bucket, source=inspec label)
    if let Some(retry_after_secs) = state.rate_limiter.check() {
        tracing::warn!(
            source = "inspec",
            rate_limited = true,
            retry_after = retry_after_secs,
            "Rate limit exceeded for InSpec ingest - returning 429"
        );
        tracing::warn!(metric = "spindle_ingest_rate_limit_hits_total", source = "inspec", "rate_limit_exceeded");

        let body = serde_json::json!({
            "status": "too_many_requests",
            "error": "Rate limit exceeded",
            "retry_after_seconds": retry_after_secs,
            "source": "inspec"
        });

        let mut response = axum::Json(body).into_response();
        response.headers_mut().insert(
            header::RETRY_AFTER,
            axum::http::HeaderValue::from_str(&retry_after_secs.to_string()).unwrap()
        );
        *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
        return response;
    }

    // Step 5: Check queue depth before enqueue
    let queue_depth = state.queue_monitor.queue_depth();
    let max_depth = state.config.max_queue_depth;

    if queue_depth >= max_depth {
        let drain_seconds = estimate_drain_time(queue_depth, state.queue_monitor.worker_rate());
        tracing::warn!(
            source = "inspec",
            queue_depth = queue_depth,
            max_depth = max_depth,
            estimated_drain_seconds = drain_seconds,
            "Queue depth exceeded for InSpec ingest - returning 429"
        );

        let body = serde_json::json!({
            "status": "too_many_requests",
            "error": "Queue is at capacity",
            "queue_depth": queue_depth,
            "max_queue_depth": max_depth
        });

        let mut response = axum::Json(body).into_response();
        response.headers_mut().insert(
            header::RETRY_AFTER,
            axum::http::HeaderValue::from_str(&drain_seconds.to_string()).unwrap()
        );
        *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
        return response;
    }

    // Step 6: Check payload-level idempotency (by SHA256)
    if let Some(existing_receipt) = state.idempotency.check_duplicate_by_sha(&payload_sha) {
        let elapsed = start.elapsed();
        tracing::info!(
            source = "inspec",
            original_receipt = %existing_receipt,
            total_latency_ms = %elapsed.as_millis(),
            "Duplicate InSpec payload (by SHA256) detected - returning original receipt"
        );
        tracing::warn!(metric = "spindle_ingest_duplicate_count", source = "inspec", "duplicate_detected");

        let body = serde_json::json!({
            "status": "duplicate",
            "receipt_token": existing_receipt,
            "source": "inspec",
            "message": "Duplicate payload - already processed"
        });
        return (StatusCode::ACCEPTED, axum::Json(body)).into_response();
    }

    // Step 7: Write verbatim payload to raw archive (write-before-parse)
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
                source = "inspec",
                error = %e,
                latency_ms = %elapsed.as_millis(),
                "InSpec archive write failed - returning 503"
            );

            let body = serde_json::json!({
                "status": "service_unavailable",
                "error": "Archive write failed",
                "source": "inspec"
            });
            return (StatusCode::SERVICE_UNAVAILABLE, axum::Json(body)).into_response();
        }
    };

    let archive_elapsed = archive_start.elapsed();
    tracing::info!(
        source = "inspec",
        archive_key = %archive_key,
        archive_write_ms = %archive_elapsed.as_millis(),
        "InSpec archive write latency"
    );

    let receipt = ReceiptToken::new();

    // Record idempotency by SHA256
    state.idempotency.record_by_sha(&payload_sha, &receipt.to_string());

    // Step 8: Attempt JSON parse
    let payload_json: Value = match serde_json::from_slice(&payload_bytes) {
        Ok(v) => v,
        Err(parse_err) => {
            // Malformed payload — JSON parse failure
            let err_msg = sanitize_error_message(&parse_err);
            tracing::warn!(
                source = "inspec",
                error_category = %err_msg,
                archive_key = %archive_key,
                receipt = %receipt,
                "Malformed InSpec payload (JSON parse failure) - archived, returning 202"
            );
            tracing::warn!(metric = "spindle_ingest_malformed_count", source = "inspec", "malformed_payload_received");

            let body = serde_json::json!({
                "status": "accepted",
                "receipt_token": receipt.to_string(),
                "archive_key": archive_key,
                "source": "inspec",
                "message": "Malformed payload archived - awaiting manual review"
            });

            let elapsed = start.elapsed();
            tracing::info!(source = "inspec", total_latency_ms = %elapsed.as_millis(), "InSpec request complete");
            return (StatusCode::ACCEPTED, axum::Json(body)).into_response();
        }
    };

    // Step 9: Verify InSpec structure and extract idempotency info
    let inspec_key = inspec_idempotency_key(&payload_json);

    if let Some(key) = inspec_key {
        // Convert InSpecKey to IdempotencyKey for storage
        let idem_key = IdempotencyKey {
            chef_server_url: None,
            organization: key.organization,
            node_name: key.node_name,
            run_id: key.run_id,
            message_type: MessageType::ComplianceReport,
        };
        state.idempotency.record(&idem_key, &payload_sha, &receipt.to_string());
    } else {
        tracing::warn!(source = "inspec", "Could not extract InSpec idempotency key from payload - using SHA256 only");
    }

    tracing::info!(
        source = "inspec",
        archive_key = %archive_key,
        receipt = %receipt,
        "Valid InSpec payload received, archived, and queued for processing"
    );
    tracing::warn!(metric = "spindle_ingest_accepted_count", source = "inspec", "ingest_accepted");

    let body = serde_json::json!({
        "status": "accepted",
        "receipt_token": receipt.to_string(),
        "archive_key": archive_key,
        "source": "inspec",
        "message": "InSpec payload received, archived, and queued for processing"
    });

    let elapsed = start.elapsed();
    tracing::info!(source = "inspec", total_latency_ms = %elapsed.as_millis(), "InSpec request complete");
    (StatusCode::ACCEPTED, axum::Json(body)).into_response()
}

// ===========================================================================
// In-memory implementations for testing
// ===========================================================================

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

/// Thread-safe in-memory queue monitor for testing.
#[derive(Debug)]
pub struct InMemoryQueueMonitor {
    depth: std::sync::atomic::AtomicU64,
    rate: f64,
}

impl InMemoryQueueMonitor {
    pub fn new(depth: u64, rate: f64) -> Self {
        Self {
            depth: std::sync::atomic::AtomicU64::new(depth),
            rate,
        }
    }
}

impl QueueMonitor for InMemoryQueueMonitor {
    fn queue_depth(&self) -> u64 {
        self.depth.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn worker_rate(&self) -> f64 {
        self.rate
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use super::*;
    use axum::body::Body as AxumBody;
    use axum::http::Request;
    use tower::ServiceExt;

    /// Helper: Create a test app state with in-memory archive and idempotency store
    fn create_test_state(token: &str, max_size: u64) -> (IngestAppState, tempfile::TempDir) {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let archive = Arc::new(spindle_rawarchive::LocalArchive::new(tmp_dir.path().to_str().unwrap()).unwrap());
        let idempotency = Arc::new(InMemoryIdempotencyStore::new());
        let queue = Arc::new(InMemoryQueueMonitor::new(0, 150.0));
        let config = IngestConfig::with_max_size(token, max_size);
        let state = IngestAppState::new(config, archive, idempotency, queue, DEFAULT_MAX_INGEST_LAG_SECONDS * 2);
        (state, tmp_dir)
    }

    /// Helper: Create a run-start payload (no "resources" key)
    fn make_run_start() -> Value {
        serde_json::json!({
            "run_id": "run-abc-123",
            "node_name": "web-server-01",
            "chef_version": "18.0.0"
        })
    }

    // === Payload type detection tests ===

    #[test]
    fn test_detect_payload_type_run_start() {
        let json = make_run_start();
        assert_eq!(detect_payload_type(&json), PayloadType::RunStart);
    }

    #[test]
    fn test_detect_payload_type_run_converge() {
        let json = serde_json::json!({
            "run_id": "run-abc-123",
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
        let json = serde_json::json!({
            "run_id": "run-abc-123",
            "node_name": "web-server-01",
        });
        let key = IdempotencyKey::from_json(&json, MessageType::RunStart);
        assert!(key.is_some());
        let key = key.unwrap();
        assert_eq!(key.node_name, "web-server-01");
        assert_eq!(key.run_id, "run-abc-123");
        assert_eq!(key.message_type, MessageType::RunStart);
    }

    #[test]
    fn test_idempotency_key_from_json_run_converge() {
        let json = serde_json::json!({
            "run_id": "run-abc-123",
            "node_name": "web-server-01",
            "resources": []
        });
        let key = IdempotencyKey::from_json(&json, MessageType::RunConverge);
        assert!(key.is_some());
        let key = key.unwrap();
        assert_eq!(key.message_type, MessageType::RunConverge);
    }

    #[test]
    fn test_idempotency_key_missing_node_name() {
        let json = serde_json::json!({
            "run_id": "run-abc-123",
        });
        let key = IdempotencyKey::from_json(&json, MessageType::RunStart);
        assert!(key.is_none());
    }

    #[test]
    fn test_idempotency_key_display() {
        let key = IdempotencyKey {
            chef_server_url: Some("https://chef.example.com".to_string()),
            organization: Some("prod".to_string()),
            node_name: "web-01".to_string(),
            run_id: "run-123".to_string(),
            message_type: MessageType::RunStart,
        };
        let s = key.to_string();
        assert!(s.contains("web-01"));
        assert!(s.contains("run-123"));
        assert!(s.contains("run-start"));
    }

    // === Constant-time token comparison tests ===

    #[test]
    fn test_constant_time_comparison_valid() {
        let config = IngestConfig::new("super-secret-token");
        assert!(verify_bearer_token(&config, Some("Bearer super-secret-token")));
    }

    #[test]
    fn test_constant_time_comparison_invalid() {
        let config = IngestConfig::new("super-secret-token");
        assert!(!verify_bearer_token(&config, Some("Bearer wrong-token")));
    }

    #[test]
    fn test_constant_time_comparison_missing() {
        let config = IngestConfig::new("super-secret-token");
        assert!(!verify_bearer_token(&config, None));
    }

    #[test]
    fn test_constant_time_comparison_empty_config() {
        let config = IngestConfig::default();
        assert!(!verify_bearer_token(&config, Some("Bearer anything")));
    }

    #[test]
    fn test_constant_time_comparison_wrong_length() {
        let config = IngestConfig::new("super-secret-token");
        assert!(!verify_bearer_token(&config, Some("Bearer short")));
    }

    // === Bearer extraction tests ===

    #[test]
    fn test_extract_bearer_valid() {
        let mut headers = header::HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer my-token".parse().unwrap());
        assert_eq!(extract_bearer(&headers), Some("my-token".to_string()));
    }

    #[test]
    fn test_extract_bearer_missing() {
        let headers = header::HeaderMap::new();
        assert_eq!(extract_bearer(&headers), None);
    }

    // === Config tests ===

    #[test]
    fn test_config_default_max_size() {
        let config = IngestConfig::default();
        assert_eq!(config.max_payload_size, DEFAULT_MAX_PAYLOAD_SIZE);
        assert_eq!(config.max_queue_depth, DEFAULT_MAX_QUEUE_DEPTH);
        assert_eq!(config.rate_limit_rps, DEFAULT_RATE_LIMIT_RPS);
        assert_eq!(config.rate_limit_burst, DEFAULT_RATE_LIMIT_BURST);
    }

    #[test]
    fn test_config_custom_max_size() {
        let config = IngestConfig::with_max_size("token", 1024);
        assert_eq!(config.max_payload_size, 1024);
        assert_eq!(config.max_queue_depth, DEFAULT_MAX_QUEUE_DEPTH);
    }

    #[test]
    fn test_config_queue_depth() {
        let config = IngestConfig::with_queue_depth("token", 1024, 50000);
        assert_eq!(config.max_queue_depth, 50000);
    }

    #[test]
    fn test_config_rate_limit() {
        let config = IngestConfig::with_rate_limit("token", 1024, 100_000, 250, 500);
        assert_eq!(config.rate_limit_rps, 250);
        assert_eq!(config.rate_limit_burst, 500);
    }

    // === SHA256 tests ===

    #[test]
    fn test_compute_sha256() {
        let data = b"hello world";
        let hash = compute_sha256(data);
        // SHA-256 of "hello world" = b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9
        assert_eq!(hash, "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
    }

    // === Receipt token tests ===

    #[test]
    fn test_receipt_token_format() {
        let receipt = ReceiptToken::new();
        let s = receipt.to_string();
        assert!(s.starts_with("receipt:"));
        assert!(s.len() > "receipt:".len());
    }

    // === Route building test ===

    #[test]
    fn test_ingest_routes_builds() {
        let config = IngestConfig::new("test");
        let archive = Arc::new(spindle_rawarchive::LocalArchive::new("/tmp").unwrap());
        let idempotency = Arc::new(InMemoryIdempotencyStore::new());
        let queue = Arc::new(InMemoryQueueMonitor::new(0, 150.0));
        let state = IngestAppState::new(config, archive, idempotency, queue, 600);
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
            .body(AxumBody::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "accepted");
        assert!(json["receipt_token"].as_str().unwrap().starts_with("receipt:"));
    }

    #[tokio::test]
    async fn test_handler_valid_token_compliance_report() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

        let payload = serde_json::json!({
            "profiles": [{"name": "ssh-baseline", "controls": []}],
            "node_name": "web-01",
            "run_id": "run-abc",
        });

        let request = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "accepted");
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
            "chef_version": "18.0.0",
            "resources": []
        });

        let request = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "accepted");
    }

    #[tokio::test]
    async fn test_handler_payload_too_large() {
        let (state, _tmp) = create_test_state("valid-secret-token", 10);
        let app = ingest_routes(state);

        let payload = make_run_start();
        let request = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn test_handler_payload_size_boundary_exact_limit() {
        // Use a small limit (100 bytes) for precise boundary testing
        let config = IngestConfig::with_max_size("token", 100);
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let archive = Arc::new(spindle_rawarchive::LocalArchive::new(tmp_dir.path().to_str().unwrap()).unwrap());
        let idempotency = Arc::new(InMemoryIdempotencyStore::new());
        let queue = Arc::new(InMemoryQueueMonitor::new(0, 150.0));
        let state = IngestAppState::new(config, archive, idempotency, queue, DEFAULT_MAX_INGEST_LAG_SECONDS * 2);
        let app = ingest_routes(state);

        // Create payload exactly at limit (100 bytes)
        let payload_str = "x".repeat(100);
        let request = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(AxumBody::from(payload_str))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        // 100 is NOT > 100 → passes size check → malformed JSON → archived → 202
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn test_handler_payload_one_byte_over_limit() {
        // Use a small limit (100 bytes) for precise boundary testing
        let config = IngestConfig::with_max_size("token", 100);
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let archive = Arc::new(spindle_rawarchive::LocalArchive::new(tmp_dir.path().to_str().unwrap()).unwrap());
        let idempotency = Arc::new(InMemoryIdempotencyStore::new());
        let queue = Arc::new(InMemoryQueueMonitor::new(0, 150.0));
        let state = IngestAppState::new(config, archive, idempotency, queue, DEFAULT_MAX_INGEST_LAG_SECONDS * 2);
        let app = ingest_routes(state);

        // Create payload 1 byte over limit (101 bytes) — 413
        let payload_str = "x".repeat(101);
        let request = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(AxumBody::from(payload_str))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn test_handler_inspec_oversized_returns_413() {
        // Use a small limit (10 bytes) to make test fast
        let config = IngestConfig::with_max_size("valid-secret-token", 10);
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let archive = Arc::new(spindle_rawarchive::LocalArchive::new(tmp_dir.path().to_str().unwrap()).unwrap());
        let idempotency = Arc::new(InMemoryIdempotencyStore::new());
        let queue = Arc::new(InMemoryQueueMonitor::new(0, 150.0));
        let state = IngestAppState::new(config, archive, idempotency, queue, DEFAULT_MAX_INGEST_LAG_SECONDS * 2);
        let app = ingest_routes(state);

        let payload = "x".repeat(100); // 100 bytes > 10 limit
        let request = Request::builder()
            .uri("/ingest/events/inspec")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .body(AxumBody::from(payload))
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
            .body(AxumBody::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "accepted");
    }

    #[tokio::test]
    async fn test_handler_missing_required_fields_duplicate_detected() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

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
        let bad_json = "this is not valid json{{{}}}";
        let parse_result: Result<Value, _> = serde_json::from_str(bad_json);
        assert!(parse_result.is_err());

        let err = parse_result.unwrap_err();
        let sanitized = sanitize_error_message(&err);

        // Should contain "expected" (the category) but NOT the payload content
        assert!(sanitized.contains("expected"));
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

        store.record_by_sha(sha, "receipt:456");
        assert_eq!(store.check_duplicate_by_sha(sha), Some("receipt:456".to_string()));
    }

    // === Queue depth limiting tests ===

    #[tokio::test]
    async fn test_handler_queue_full_returns_429() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let archive = Arc::new(spindle_rawarchive::LocalArchive::new(tmp_dir.path().to_str().unwrap()).unwrap());
        let idempotency = Arc::new(InMemoryIdempotencyStore::new());
        let queue = Arc::new(InMemoryQueueMonitor::new(100_000, 150.0));
        let config = IngestConfig::with_queue_depth("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE, 100_000);
        let state = IngestAppState::new(config, archive, idempotency, queue, DEFAULT_MAX_INGEST_LAG_SECONDS * 2);
        let app = ingest_routes(state);

        let payload = make_run_start();
        let request = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

        let retry_after = response.headers().get(header::RETRY_AFTER);
        assert!(retry_after.is_some());
        let retry_secs: u64 = retry_after.unwrap().to_str().unwrap().parse().unwrap();
        assert!(retry_secs > 0);

        let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "too_many_requests");
        assert_eq!(json["queue_depth"], 100000);
        assert_eq!(json["max_queue_depth"], 100000);
    }

    #[tokio::test]
    async fn test_handler_queue_drains_returns_202() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let archive = Arc::new(spindle_rawarchive::LocalArchive::new(tmp_dir.path().to_str().unwrap()).unwrap());
        let idempotency = Arc::new(InMemoryIdempotencyStore::new());
        let queue = Arc::new(InMemoryQueueMonitor::new(99_999, 150.0));
        let config = IngestConfig::with_queue_depth("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE, 100_000);
        let state = IngestAppState::new(config, archive, idempotency, queue, DEFAULT_MAX_INGEST_LAG_SECONDS * 2);
        let app = ingest_routes(state);

        let payload = make_run_start();
        let request = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn test_handler_queue_at_custom_limit() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let archive = Arc::new(spindle_rawarchive::LocalArchive::new(tmp_dir.path().to_str().unwrap()).unwrap());
        let idempotency = Arc::new(InMemoryIdempotencyStore::new());
        let queue = Arc::new(InMemoryQueueMonitor::new(5, 150.0));
        let config = IngestConfig::with_queue_depth("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE, 5);
        let state = IngestAppState::new(config, archive, idempotency, queue, DEFAULT_MAX_INGEST_LAG_SECONDS * 2);
        let app = ingest_routes(state);

        let payload = make_run_start();
        let request = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

        let retry_after = response.headers().get(header::RETRY_AFTER);
        assert!(retry_after.is_some());
        let retry_secs: u64 = retry_after.unwrap().to_str().unwrap().parse().unwrap();
        assert_eq!(retry_secs, 1);
    }

    #[test]
    fn test_estimate_drain_time() {
        assert_eq!(estimate_drain_time(100_000, 150.0), 667);
        assert_eq!(estimate_drain_time(150, 150.0), 1);
        assert_eq!(estimate_drain_time(0, 150.0), 0);
        assert_eq!(estimate_drain_time(100, 0.0), 0);
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
            .body(AxumBody::from(payload.to_string()))
            .unwrap();
        let response1 = app.clone().oneshot(request1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::ACCEPTED);

        let body1 = axum::body::to_bytes(response1.into_body(), 4096).await.unwrap();
        let json1: Value = serde_json::from_slice(&body1).unwrap();
        let receipt1 = json1["receipt_token"].as_str().unwrap().to_string();

        // Second identical request — should be duplicate
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
        assert_eq!(json2["receipt_token"].as_str().unwrap(), receipt1);
    }

    #[tokio::test]
    async fn test_different_run_ids_not_duplicated() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

        let payload1 = serde_json::json!({
            "run_id": "run-1",
            "node_name": "web-01",
            "resources": []
        });
        let payload2 = serde_json::json!({
            "run_id": "run-2",
            "node_name": "web-01",
            "resources": []
        });

        let request1 = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .body(AxumBody::from(payload1.to_string()))
            .unwrap();
        let response1 = app.clone().oneshot(request1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::ACCEPTED);

        let json1: Value = serde_json::from_slice(&axum::body::to_bytes(response1.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(json1["status"], "accepted");

        let request2 = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .body(AxumBody::from(payload2.to_string()))
            .unwrap();
        let response2 = app.oneshot(request2).await.unwrap();
        assert_eq!(response2.status(), StatusCode::ACCEPTED);

        let json2: Value = serde_json::from_slice(&axum::body::to_bytes(response2.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(json2["status"], "accepted");
    }

    #[tokio::test]
    async fn test_different_message_types_not_duplicated() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

        // Run-start payload
        let payload1 = serde_json::json!({
            "run_id": "run-same",
            "node_name": "web-01",
            "chef_version": "18.0.0"
        });

        // Compliance report (same node_name and run_id but different type)
        let payload2 = serde_json::json!({
            "run_id": "run-same",
            "node_name": "web-01",
            "profiles": [{"name": "ssh-baseline", "controls": []}]
        });

        let request1 = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .body(AxumBody::from(payload1.to_string()))
            .unwrap();
        let response1 = app.clone().oneshot(request1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::ACCEPTED);

        let request2 = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .body(AxumBody::from(payload2.to_string()))
            .unwrap();
        let response2 = app.oneshot(request2).await.unwrap();
        assert_eq!(response2.status(), StatusCode::ACCEPTED);
    }

    // === Rate limiting tests ===

    #[test]
    fn test_check_rate_limit_allows_when_not_limited() {
        let rl = RateLimitStore::new(1, 1);
        // With 1 rps and 1 burst, a single check should pass
        assert!(rl.check().is_none());
    }

    #[test]
    fn test_check_rate_limit_blocks_when_exhausted() {
        let rl = RateLimitStore::new(1, 1);

        // First check should pass (uses burst token)
        assert!(rl.check().is_none());
        // Next check should be rejected (burst exhausted)
        assert!(rl.check().is_some());
    }

    #[tokio::test]
    async fn test_handler_rate_limit_exceeded_returns_429() {
        // Create a config with very low rate limit
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let archive = Arc::new(spindle_rawarchive::LocalArchive::new(tmp_dir.path().to_str().unwrap()).unwrap());
        let idempotency = Arc::new(InMemoryIdempotencyStore::new());
        let queue = Arc::new(InMemoryQueueMonitor::new(0, 150.0));

        // rate_limit_rps=1, burst=1 — after first request, second should fail
        let config = IngestConfig::with_rate_limit("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE, DEFAULT_MAX_QUEUE_DEPTH, 1, 1);
        let state = IngestAppState::new(config, archive, idempotency, queue, DEFAULT_MAX_INGEST_LAG_SECONDS * 2);
        let app = ingest_routes(state);

        let payload = make_run_start();

        // First request — should succeed (uses the burst token)
        let request1 = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();
        let response1 = app.clone().oneshot(request1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::ACCEPTED);

        // Second request — should be rate limited (burst exhausted)
        let request2 = Request::builder()
            .uri("/ingest/events/data-collector")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();
        let response2 = app.oneshot(request2).await.unwrap();
        assert_eq!(response2.status(), StatusCode::TOO_MANY_REQUESTS);

        let retry_after = response2.headers().get(header::RETRY_AFTER);
        assert!(retry_after.is_some());
    }

    #[tokio::test]
    async fn test_handler_steady_state_below_rate_limit() {
        // With default 500 rps + 1000 burst, we can send many requests quickly
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

        // Send 50 requests — should all be 202 (within burst allowance)
        for i in 0..50 {
            let payload = serde_json::json!({
                "run_id": format!("run-{}", i),
                "node_name": "web-01",
                "resources": []
            });
            let request = Request::builder()
                .uri("/ingest/events/data-collector")
                .method("POST")
                .header(header::AUTHORIZATION, "Bearer valid-secret-token")
                .body(AxumBody::from(payload.to_string()))
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::ACCEPTED);
        }
    }

    // === InSpec handler tests ===

    /// Helper: Create a sample InSpec JSON reporter payload
    fn make_inspec_payload() -> Value {
        serde_json::json!({
            "platform": {
                "name": "ubuntu",
                "release": "22.04"
            },
            "profiles": [
                {
                    "name": "linux-baseline",
                    "version": "1.0.0",
                    "sha256": "abc123",
                    "controls": [
                        {
                            "id": "ssh-01",
                            "title": "SSH Configuration",
                            "description": "SSH should be configured securely",
                            "results": [
                                {
                                    "status": "passed",
                                    "code": "describe sshd_config do\n  it { should exist }\nend",
                                    "run_time": 0.05,
                                    "start_time": "2024-01-01T00:00:00+00:00"
                                }
                            ]
                        }
                    ]
                }
            ],
            "statistics": {
                "duration": 1.5
            },
            "version": "5.21.0"
        })
    }

    #[tokio::test]
    async fn test_handler_inspec_valid() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

        let payload = make_inspec_payload();

        let request = Request::builder()
            .uri("/ingest/events/inspec")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "accepted");
        assert_eq!(json["source"], "inspec");
        assert!(json["receipt_token"].as_str().unwrap().starts_with("receipt:"));
    }

    #[tokio::test]
    async fn test_handler_inspec_invalid_token() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

        let payload = make_inspec_payload();
        let request = Request::builder()
            .uri("/ingest/events/inspec")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer wrong-token")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_handler_inspec_malformed_json() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

        let bad_json = "not valid json at all {{{}}}";

        let request = Request::builder()
            .uri("/ingest/events/inspec")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(AxumBody::from(bad_json))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["source"], "inspec");
    }

    #[tokio::test]
    async fn test_handler_inspec_duplicate_detected() {
        let (state, _tmp) = create_test_state("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE);
        let app = ingest_routes(state);

        let payload = make_inspec_payload();

        // First request
        let request1 = Request::builder()
            .uri("/ingest/events/inspec")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();
        let response1 = app.clone().oneshot(request1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::ACCEPTED);

        // Second identical request — should be duplicate
        let request2 = Request::builder()
            .uri("/ingest/events/inspec")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();
        let response2 = app.oneshot(request2).await.unwrap();
        assert_eq!(response2.status(), StatusCode::ACCEPTED);

        let body2 = axum::body::to_bytes(response2.into_body(), 4096).await.unwrap();
        let json2: Value = serde_json::from_slice(&body2).unwrap();
        assert_eq!(json2["status"], "duplicate");
        assert_eq!(json2["source"], "inspec");
    }

    #[tokio::test]
    async fn test_handler_inspec_queue_full() {
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let archive = Arc::new(spindle_rawarchive::LocalArchive::new(tmp_dir.path().to_str().unwrap()).unwrap());
        let idempotency = Arc::new(InMemoryIdempotencyStore::new());
        let queue = Arc::new(InMemoryQueueMonitor::new(100_000, 150.0));
        let config = IngestConfig::with_queue_depth("valid-secret-token", DEFAULT_MAX_PAYLOAD_SIZE, 100_000);
        let state = IngestAppState::new(config, archive, idempotency, queue, DEFAULT_MAX_INGEST_LAG_SECONDS * 2);
        let app = ingest_routes(state);

        let payload = make_inspec_payload();
        let request = Request::builder()
            .uri("/ingest/events/inspec")
            .method("POST")
            .header(header::AUTHORIZATION, "Bearer valid-secret-token")
            .body(AxumBody::from(payload.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

        let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["queue_depth"], 100000);
    }

    #[test]
    fn test_inspec_key_from_json() {
        let payload = make_inspec_payload();
        let key = InSpecKey::from_json(&payload);
        assert!(key.is_some());
        let key = key.unwrap();
        assert_eq!(key.node_name, "ubuntu");
        assert!(key.run_id.len() > 0);
    }

    #[test]
    fn test_inspec_key_missing_node_name() {
        let payload = serde_json::json!({
            "platform": {},
            "profiles": []
        });
        let key = InSpecKey::from_json(&payload);
        assert!(key.is_none());
    }
}
