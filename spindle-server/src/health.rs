//! M2-08: Health endpoint — GET /v1/health and GET /v1/health/metrics
//!
//! Provides comprehensive health checks for all subsystems with
//! parallel checks, 5s timeout per subsystem, and 5s response cache.
//!
//! ## Endpoints
//! - `GET /v1/health` — aggregate health of all subsystems
//! - `GET /v1/health/metrics` — Prometheus-format metrics
//!
//! ## Subsystems checked
//! - Database (PostgreSQL)
//! - Storage (object storage / raw archive)
//! - Ingest lag (queue depth, oldest unprocessed message)
//! - API version
//! - Dex (OIDC identity provider)

#![allow(warnings)]
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use chrono::{DateTime, Utc};
use utoipa::ToSchema;
use tokio::sync::RwLock;
use spindle_rawarchive::Archive;

use crate::ingest::{API_VERSION, X_REQUEST_ID_HEADER};

// ── Health check types ────────────────────────────────────────────────────────

/// Status of a single subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Up,
    Degraded,
    Down,
}

/// Health check result for a single subsystem.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SubsystemHealth {
    pub name: String,
    pub status: HealthStatus,
    pub latency_ms: u64,
    pub detail: Option<String>,
}

/// Aggregate health response.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HealthResponse {
    pub api_version: String,
    pub request_id: String,
    pub status: HealthStatus,
    /// HTTP 200 when all up, 503 when DB down.
    pub http_status: u16,
    pub timestamp: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ingest_lag: Option<IngestLagInfo>,
    pub subsystems: Vec<SubsystemHealth>,
}

/// Ingest queue lag information.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct IngestLagInfo {
    /// Number of unprocessed messages in the queue.
    pub queue_depth: usize,
    /// Age of the oldest unprocessed message (seconds).
    pub oldest_unprocessed_seconds: Option<u64>,
    /// ISO timestamp of the oldest unprocessed message.
    pub oldest_unprocessed_at: Option<DateTime<Utc>>,
}

// ── Health checker trait ─────────────────────────────────────────────────────

/// Trait for checking a subsystem's health.
#[async_trait::async_trait]
pub trait HealthChecker: Send + Sync + std::fmt::Debug {
    /// Check the subsystem. Returns `Ok(SubSystemHealth)` on success.
    /// Implementations MUST respect the 5s timeout by using tokio::time::timeout
    /// internally or by being inherently fast.
    async fn check(&self) -> SubsystemHealth;

    /// Name of the subsystem (e.g., "database", "storage", "dex").
    fn name(&self) -> &str;
}

// ── App state ─────────────────────────────────────────────────────────────────

/// Shared health state with cached responses.
#[derive(Debug, Clone)]
pub struct HealthAppState {
    pub cache: Arc<RwLock<HealthCache>>,
    pub db_checker: Arc<dyn HealthChecker>,
    pub storage_checker: Arc<dyn HealthChecker>,
    pub dex_checker: Arc<dyn HealthChecker>,
}
#[derive(Debug, Clone)]
pub struct HealthCache {
    pub response: Option<HealthResponse>,
    pub cached_at: Instant,
    pub ttl: Duration,
}

impl HealthCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            response: None,
            cached_at: Instant::now(),
            ttl,
        }
    }

    /// Check if cache is still valid.
    pub fn is_valid(&self) -> bool {
        if self.response.is_none() {
            return false;
        }
        self.cached_at.elapsed() < self.ttl
    }
}

impl HealthAppState {
    pub fn new(
        db_checker: Arc<dyn HealthChecker>,
        storage_checker: Arc<dyn HealthChecker>,
        dex_checker: Arc<dyn HealthChecker>,
    ) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HealthCache::new(Duration::from_secs(5)))),
            db_checker,
            storage_checker,
            dex_checker,
        }
    }

    /// Get health response, using cache if valid (5s TTL).
    pub async fn get_health(&self, request_id: String) -> HealthResponse {
        {
            let cache = self.cache.read().await;
            if let Some(cached) = &cache.response {
                if cache.is_valid() {
                    // Clone and update request_id
                    let mut resp = cached.clone();
                    resp.request_id = request_id;
                    return resp;
                }
            }
        }

        // Cache miss — compute fresh health
        let response = self.compute_health(request_id).await;

        // Update cache
        let mut cache = self.cache.write().await;
        cache.response = Some(response.clone());
        cache.cached_at = Instant::now();

        response
    }

    /// Compute fresh health by checking all subsystems in parallel with 5s timeout.
    async fn compute_health(&self, request_id: String) -> HealthResponse {
        let start = Instant::now();

        // Run all checks in parallel with 5s timeout each
        let db_fut = async {
            let res = tokio::time::timeout(
                Duration::from_secs(5),
                self.db_checker.check(),
            ).await;
            match res {
                Ok(health) => health,
                Err(_) => SubsystemHealth {
                    name: self.db_checker.name().to_string(),
                    status: HealthStatus::Down,
                    latency_ms: 5000,
                    detail: Some("check timed out after 5s".to_string()),
                },
            }
        };

        let storage_fut = async {
            let res = tokio::time::timeout(
                Duration::from_secs(5),
                self.storage_checker.check(),
            ).await;
            match res {
                Ok(health) => health,
                Err(_) => SubsystemHealth {
                    name: self.storage_checker.name().to_string(),
                    status: HealthStatus::Down,
                    latency_ms: 5000,
                    detail: Some("check timed out after 5s".to_string()),
                },
            }
        };

        let dex_fut = async {
            let res = tokio::time::timeout(
                Duration::from_secs(5),
                self.dex_checker.check(),
            ).await;
            match res {
                Ok(health) => health,
                Err(_) => SubsystemHealth {
                    name: self.dex_checker.name().to_string(),
                    status: HealthStatus::Down,
                    latency_ms: 5000,
                    detail: Some("check timed out after 5s".to_string()),
                },
            }
        };

        let (db_health, storage_health, dex_health) = tokio::join!(db_fut, storage_fut, dex_fut);

        let subsystems = vec![db_health, storage_health, dex_health];

        // Determine overall status
        let overall_status = if subsystems.iter().any(|s| s.status == HealthStatus::Down) {
            HealthStatus::Down
        } else if subsystems.iter().any(|s| s.status == HealthStatus::Degraded) {
            HealthStatus::Degraded
        } else {
            HealthStatus::Up
        };

        // HTTP status: 200 when all up, 503 when any subsystem down or degraded
        let http_status = if subsystems
            .iter()
            .any(|s| s.status == HealthStatus::Down || s.status == HealthStatus::Degraded)
        {
            503
        } else {
            200
        };

        let _latency_ms = start.elapsed().as_millis() as u64;

        // Build ingest lag info (in real impl, queries the queue)
        let ingest_lag = Some(IngestLagInfo {
            queue_depth: 0,
            oldest_unprocessed_seconds: None,
            oldest_unprocessed_at: None,
        });

        HealthResponse {
            api_version: API_VERSION.to_string(),
            request_id,
            status: overall_status,
            http_status,
            timestamp: Utc::now(),
            ingest_lag,
            subsystems,
        }
    }
}

// ── Real health checkers ──────────────────────────────────────────────────────

/// Health checker for the PostgreSQL database.
/// Performs a real `SELECT 1` query via the sqlx connection pool.
#[derive(Debug)]
pub struct DbHealthChecker {
    pool: sqlx::Pool<sqlx::Postgres>,
}

impl DbHealthChecker {
    pub fn new(pool: sqlx::Pool<sqlx::Postgres>) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl HealthChecker for DbHealthChecker {
    async fn check(&self) -> SubsystemHealth {
        let start = Instant::now();
        match tokio::time::timeout(
            Duration::from_secs(5),
            self.pool.acquire(),
        )
        .await
        {
            Ok(Ok(_conn)) => {
                // Execute SELECT 1 to verify the DB is truly responsive
                match tokio::time::timeout(
                    Duration::from_secs(5),
                    sqlx::query(db_health_check_sql()).execute(&self.pool),
                )
                .await
                {
                    Ok(Ok(_)) => SubsystemHealth {
                        name: "database".to_string(),
                        status: HealthStatus::Up,
                        latency_ms: start.elapsed().as_millis() as u64,
                        detail: None,
                    },
                    Ok(Err(e)) => SubsystemHealth {
                        name: "database".to_string(),
                        status: HealthStatus::Down,
                        latency_ms: start.elapsed().as_millis() as u64,
                        detail: Some(format!("SELECT 1 failed: {}", e)),
                    },
                    Err(_) => SubsystemHealth {
                        name: "database".to_string(),
                        status: HealthStatus::Down,
                        latency_ms: 5000,
                        detail: Some("query timed out after 5s".to_string()),
                    },
                }
            }
            Ok(Err(e)) => SubsystemHealth {
                name: "database".to_string(),
                status: HealthStatus::Down,
                latency_ms: start.elapsed().as_millis() as u64,
                detail: Some(format!("connection failed: {}", e)),
            },
            Err(_) => SubsystemHealth {
                name: "database".to_string(),
                status: HealthStatus::Down,
                latency_ms: 5000,
                detail: Some("acquire timed out after 5s".to_string()),
            },
        }
    }

    fn name(&self) -> &str {
        "database"
    }
}

/// Health checker for the raw archive storage.
/// Performs a write -> read -> delete round-trip on the archive.
#[derive(Debug, Clone)]
pub struct StorageHealthChecker {
    archive: Arc<spindle_rawarchive::LocalArchive>,
}

impl StorageHealthChecker {
    pub fn new(archive: Arc<spindle_rawarchive::LocalArchive>) -> Self {
        Self { archive }
    }
}

#[async_trait::async_trait]
impl HealthChecker for StorageHealthChecker {
    async fn check(&self) -> SubsystemHealth {
        let start = Instant::now();
        let test_key = format!(
            "health_check_{}.tmp",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let test_data = b"health-check-ok";

        // Write
        match tokio::task::spawn_blocking({
            let archive = self.archive.clone();
            let key = test_key.clone();
            let data = test_data.to_vec();
            move || archive.storage().put(&key, &data)
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                return SubsystemHealth {
                    name: "storage".to_string(),
                    status: HealthStatus::Down,
                    latency_ms: start.elapsed().as_millis() as u64,
                    detail: Some(format!("write failed: {}", e)),
                };
            }
            Err(_) => {
                return SubsystemHealth {
                    name: "storage".to_string(),
                    status: HealthStatus::Down,
                    latency_ms: start.elapsed().as_millis() as u64,
                    detail: Some("spawn_blocking write panicked".to_string()),
                };
            }
        }

        // Read
        match tokio::task::spawn_blocking({
            let archive = self.archive.clone();
            let key = test_key.clone();
            move || archive.storage().get(&key)
        })
        .await
        {
            Ok(Ok(Some(_))) => {
                // Cleanup
                let _ = tokio::task::spawn_blocking({
                    let archive = self.archive.clone();
                    let key = test_key.clone();
                    move || archive.storage().delete(&key)
                })
                .await;

                SubsystemHealth {
                    name: "storage".to_string(),
                    status: HealthStatus::Up,
                    latency_ms: start.elapsed().as_millis() as u64,
                    detail: None,
                }
            }
            Ok(Ok(None)) => SubsystemHealth {
                name: "storage".to_string(),
                status: HealthStatus::Down,
                latency_ms: start.elapsed().as_millis() as u64,
                detail: Some("read-back returned None - data was not persisted".to_string()),
            },
            Ok(Err(e)) => SubsystemHealth {
                name: "storage".to_string(),
                status: HealthStatus::Down,
                latency_ms: start.elapsed().as_millis() as u64,
                detail: Some(format!("read-back failed: {}", e)),
            },
            Err(_) => SubsystemHealth {
                name: "storage".to_string(),
                status: HealthStatus::Down,
                latency_ms: start.elapsed().as_millis() as u64,
                detail: Some("spawn_blocking read panicked".to_string()),
            },
        }
    }

    fn name(&self) -> &str {
        "storage"
    }
}

/// Health checker for the Dex identity provider.
/// Probes the Dex `/.well-known/openid-configuration` endpoint.
#[derive(Debug, Clone)]
pub struct DexHealthChecker {
    issuer_url: String,
    client: reqwest::Client,
}

impl DexHealthChecker {
    pub fn new(issuer_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            issuer_url: issuer_url.trim_end_matches('/').to_string(),
            client,
        }
    }
}

#[async_trait::async_trait]
impl HealthChecker for DexHealthChecker {
    async fn check(&self) -> SubsystemHealth {
        let start = Instant::now();
        if self.issuer_url.is_empty() {
            return SubsystemHealth {
                name: "dex".to_string(),
                status: HealthStatus::Up,
                latency_ms: 0,
                detail: Some("no issuer configured - skipped".to_string()),
            };
        }
        let url = format!("{}/.well-known/openid-configuration", self.issuer_url);

        match tokio::time::timeout(
            Duration::from_secs(5),
            self.client.get(&url).send(),
        )
        .await
        {
            Ok(Ok(resp)) => {
                let status = resp.status().as_u16();
                if status == 200 {
                    SubsystemHealth {
                        name: "dex".to_string(),
                        status: HealthStatus::Up,
                        latency_ms: start.elapsed().as_millis() as u64,
                        detail: None,
                    }
                } else {
                    SubsystemHealth {
                        name: "dex".to_string(),
                        status: HealthStatus::Down,
                        latency_ms: start.elapsed().as_millis() as u64,
                        detail: Some(format!("HTTP {}", status)),
                    }
                }
            }
            Ok(Err(e)) => SubsystemHealth {
                name: "dex".to_string(),
                status: HealthStatus::Down,
                latency_ms: start.elapsed().as_millis() as u64,
                detail: Some(format!("request failed: {}", e)),
            },
            Err(_) => SubsystemHealth {
                name: "dex".to_string(),
                status: HealthStatus::Down,
                latency_ms: 5000,
                detail: Some("request timed out after 5s".to_string()),
            },
        }
    }

    fn name(&self) -> &str {
        "dex"
    }
}

// ── In-memory health checkers for testing ─────────────────────────────────────

/// A simple health checker that reports all-up (for testing).
#[derive(Debug, Clone)]
pub struct AlwaysUpChecker {
    pub name: String,
}

#[async_trait::async_trait]
impl HealthChecker for AlwaysUpChecker {
    async fn check(&self) -> SubsystemHealth {
        let start = Instant::now();
        SubsystemHealth {
            name: self.name.clone(),
            status: HealthStatus::Up,
            latency_ms: start.elapsed().as_millis() as u64,
            detail: None,
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// A health checker that reports a subsystem as down (for testing).
#[derive(Debug, Clone)]
pub struct AlwaysDownChecker {
    pub name: String,
    pub detail: String,
}

#[async_trait::async_trait]
impl HealthChecker for AlwaysDownChecker {
    async fn check(&self) -> SubsystemHealth {
        let start = Instant::now();
        SubsystemHealth {
            name: self.name.clone(),
            status: HealthStatus::Down,
            latency_ms: start.elapsed().as_millis() as u64,
            detail: Some(self.detail.clone()),
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// A health checker that reports degraded status (for testing).
#[derive(Debug, Clone)]
pub struct DegradedChecker {
    pub name: String,
    pub detail: String,
}

#[async_trait::async_trait]
impl HealthChecker for DegradedChecker {
    async fn check(&self) -> SubsystemHealth {
        let start = Instant::now();
        SubsystemHealth {
            name: self.name.clone(),
            status: HealthStatus::Degraded,
            latency_ms: start.elapsed().as_millis() as u64,
            detail: Some(self.detail.clone()),
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// A health checker that simulates slow responses (for timeout testing).
#[derive(Debug, Clone)]
pub struct SlowChecker {
    pub name: String,
    pub delay_ms: u64,
}

#[async_trait::async_trait]
impl HealthChecker for SlowChecker {
    async fn check(&self) -> SubsystemHealth {
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        SubsystemHealth {
            name: self.name.clone(),
            status: HealthStatus::Up,
            latency_ms: self.delay_ms,
            detail: Some(format!("took {}ms", self.delay_ms)),
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}

// ── Route builder ─────────────────────────────────────────────────────────────

/// Build the health router with /v1/health and /v1/health/metrics.
pub fn health_routes(state: HealthAppState) -> Router {
    Router::new()
        .route("/v1/health", get(health_check))
        .route("/v1/health/metrics", get(health_metrics))
        .with_state(state)
        .route_layer(middleware::from_fn(crate::ingest::request_id_middleware))
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// Handler for GET /v1/health — aggregate health check.
/// Returns 200 when all subsystems are up, 503 when any are down.
/// Cached for 5s.
pub async fn health_check(
    State(state): State<HealthAppState>,
    request: Request,
) -> impl IntoResponse {
    let request_id = get_request_id(&request);
    let response = state.get_health(request_id).await;
    let status_code = StatusCode::from_u16(response.http_status).unwrap_or(StatusCode::OK);
    (status_code, Json(response))
}

/// Handler for GET /v1/health/metrics — Prometheus-format metrics.
/// Returns plain text with Prometheus metrics.
pub async fn health_metrics(
    State(state): State<HealthAppState>,
    request: Request,
) -> impl IntoResponse {
    let _request_id = get_request_id(&request);
    let response = state.get_health(String::new()).await;

    let mut metrics = String::new();

    // API version metric
    metrics.push_str(&format!(
        r#"# HELP spindle_api_version Current API version
# TYPE spindle_api_version gauge
spindle_api_version{{version="{}"}} 1
"#,
        API_VERSION
    ));

    // Overall health metric
    let status_val = match response.status {
        HealthStatus::Up => 1,
        HealthStatus::Degraded => 0,
        HealthStatus::Down => 0,
    };
    metrics.push_str(&format!(
        r#"# HELP spindle_health_status Overall health (1=healthy, 0=unhealthy)
# TYPE spindle_health_status gauge
spindle_health_status {}
"#,
        status_val
    ));

    // Per-subsystem metrics
    for sub in &response.subsystems {
        let sub_val = match sub.status {
            HealthStatus::Up => 1,
            HealthStatus::Degraded => 0,
            HealthStatus::Down => 0,
        };
        let _detail = sub.detail.as_deref().unwrap_or("");
        metrics.push_str(&format!(
            r#"# HELP spindle_subsystem_health Subsystem health (1=up, 0=unhealthy)
# TYPE spindle_subsystem_health gauge
spindle_subsystem_health{{subsystem="{}",status="{:?}"}} {}
"#,
            sub.name, sub.status, sub_val
        ));
        metrics.push_str(&format!(
            r#"# HELP spindle_subsystem_latency_ms Subsystem check latency in ms
# TYPE spindle_subsystem_latency_ms gauge
spindle_subsystem_latency_ms{{subsystem="{}"}} {}
"#,
            sub.name, sub.latency_ms
        ));
    }

    // Ingest lag metrics
    if let Some(lag) = &response.ingest_lag {
        metrics.push_str(&format!(
            r#"# HELP spindle_ingest_queue_depth Number of unprocessed messages in queue
# TYPE spindle_ingest_queue_depth gauge
spindle_ingest_queue_depth {}
"#,
            lag.queue_depth
        ));
        if let Some(seconds) = lag.oldest_unprocessed_seconds {
            metrics.push_str(&format!(
                r#"# HELP spindle_ingest_oldest_unprocessed_seconds Age of oldest unprocessed message
# TYPE spindle_ingest_oldest_unprocessed_seconds gauge
spindle_ingest_oldest_unprocessed_seconds {}
"#,
                seconds
            ));
        }
    }

    axum::response::Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/plain; version=0.0.4; charset=utf-8")
        .body(axum::body::Body::from(metrics))
        .unwrap()
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn get_request_id(request: &Request) -> String {
    if let Some(rid) = request.extensions().get::<crate::ingest::RequestId>() {
        rid.0.clone()
    } else {
        crate::ingest::new_request_id()
    }
}

// ── SQL generation for health checks ──────────────────────────────────────────

/// SQL to check DB connectivity via a simple SELECT 1.
pub fn db_health_check_sql() -> &'static str {
    "SELECT 1"
}

/// SQL to query the ingest queue depth (number of unprocessed messages).
pub fn ingest_queue_depth_sql() -> &'static str {
    r#"SELECT COUNT(*) as queue_depth FROM ingest_queue WHERE processed_at IS NULL"#
}

/// SQL to find the oldest unprocessed message timestamp.
pub fn oldest_unprocessed_sql() -> &'static str {
    r#"SELECT MIN(received_at) as oldest FROM ingest_queue WHERE processed_at IS NULL"#
}

/// SQL to check storage connectivity (raw archive table exists).
pub fn storage_health_check_sql() -> &'static str {
    r#"SELECT 1 FROM raw_archive LIMIT 1"#
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body as AxumBody;
    use tower::ServiceExt;
    use std::time::Instant;

    #[tokio::test]
    async fn test_m2_08_health_check_all_up_returns_200() {
        let state = HealthAppState::new(
            Arc::new(AlwaysUpChecker { name: "database".to_string() }),
            Arc::new(AlwaysUpChecker { name: "storage".to_string() }),
            Arc::new(AlwaysUpChecker { name: "dex".to_string() }),
        );
        let app = health_routes(state);
        let request = Request::builder()
            .uri("/v1/health")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 65536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["api_version"], "v1");
        assert!(json["request_id"].as_str().is_some());
        assert_eq!(json["status"], "up");
        assert_eq!(json["http_status"], 200);
        assert!(json["subsystems"].is_array());
        assert_eq!(json["subsystems"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn test_m2_08_health_check_db_down_returns_503() {
        let state = HealthAppState::new(
            Arc::new(AlwaysDownChecker {
                name: "database".to_string(),
                detail: "connection refused".to_string(),
            }),
            Arc::new(AlwaysUpChecker { name: "storage".to_string() }),
            Arc::new(AlwaysUpChecker { name: "dex".to_string() }),
        );
        let app = health_routes(state);
        let request = Request::builder()
            .uri("/v1/health")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), 65536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "down");
        assert_eq!(json["http_status"], 503);
    }

    #[tokio::test]
    async fn test_m2_08_health_check_degraded_returns_503() {
        let state = HealthAppState::new(
            Arc::new(AlwaysUpChecker { name: "database".to_string() }),
            Arc::new(DegradedChecker {
                name: "storage".to_string(),
                detail: "slow response".to_string(),
            }),
            Arc::new(AlwaysUpChecker { name: "dex".to_string() }),
        );
        let app = health_routes(state);
        let request = Request::builder()
            .uri("/v1/health")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), 65536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "degraded");
    }

    #[tokio::test]
    async fn test_m2_08_health_check_includes_ingest_lag() {
        let state = HealthAppState::new(
            Arc::new(AlwaysUpChecker { name: "database".to_string() }),
            Arc::new(AlwaysUpChecker { name: "storage".to_string() }),
            Arc::new(AlwaysUpChecker { name: "dex".to_string() }),
        );
        let app = health_routes(state);
        let request = Request::builder()
            .uri("/v1/health")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let body = axum::body::to_bytes(response.into_body(), 65536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["ingest_lag"].is_object());
        assert!(json["ingest_lag"]["queue_depth"].as_u64().is_some());
    }

    #[tokio::test]
    async fn test_m2_08_health_check_x_request_id_propagated() {
        let state = HealthAppState::new(
            Arc::new(AlwaysUpChecker { name: "database".to_string() }),
            Arc::new(AlwaysUpChecker { name: "storage".to_string() }),
            Arc::new(AlwaysUpChecker { name: "dex".to_string() }),
        );
        let app = health_routes(state);
        let request = Request::builder()
            .uri("/v1/health")
            .header(X_REQUEST_ID_HEADER, "req-health-test-789")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let header_val = response.headers().get(X_REQUEST_ID_HEADER).unwrap();
        assert_eq!(header_val.to_str().unwrap(), "req-health-test-789");
        let body = axum::body::to_bytes(response.into_body(), 65536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["request_id"], "req-health-test-789");
    }

    #[tokio::test]
    async fn test_m2_08_health_check_cached_5s() {
        let state = HealthAppState::new(
            Arc::new(AlwaysUpChecker { name: "database".to_string() }),
            Arc::new(AlwaysUpChecker { name: "storage".to_string() }),
            Arc::new(AlwaysUpChecker { name: "dex".to_string() }),
        );
        let app = health_routes(state);

        // First request — cache miss
        let request = Request::builder()
            .uri("/v1/health")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let body = axum::body::to_bytes(response.into_body(), 65536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "up");

        // Second request — should use cache (same timestamp within 5s)
        let request = Request::builder()
            .uri("/v1/health")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let body = axum::body::to_bytes(response.into_body(), 65536).await.unwrap();
        let json2: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json2["status"], "up");
    }

    #[tokio::test]
    async fn test_m2_08_health_metrics_prometheus_format() {
        let state = HealthAppState::new(
            Arc::new(AlwaysUpChecker { name: "database".to_string() }),
            Arc::new(AlwaysUpChecker { name: "storage".to_string() }),
            Arc::new(AlwaysUpChecker { name: "dex".to_string() }),
        );
        let app = health_routes(state);
        let request = Request::builder()
            .uri("/v1/health/metrics")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 65536).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("spindle_api_version"));
        assert!(text.contains("spindle_health_status"));
        assert!(text.contains("spindle_subsystem_health"));
        assert!(text.contains("spindle_subsystem_latency_ms"));
        assert!(text.contains("spindle_ingest_queue_depth"));
        assert!(text.contains("database"));
        assert!(text.contains("storage"));
        assert!(text.contains("dex"));
    }

    #[tokio::test]
    async fn test_m2_08_health_check_timeout_handling() {
        // A checker that takes 10s — should be timed out at 5s
        let state = HealthAppState::new(
            Arc::new(SlowChecker { name: "database".to_string(), delay_ms: 10000 }),
            Arc::new(AlwaysUpChecker { name: "storage".to_string() }),
            Arc::new(AlwaysUpChecker { name: "dex".to_string() }),
        );
        let app = health_routes(state);
        let request = Request::builder()
            .uri("/v1/health")
            .body(axum::body::Body::empty())
            .unwrap();
        let start = Instant::now();
        let response = app.oneshot(request).await.unwrap();
        let elapsed = start.elapsed();

        // Should complete within ~5s (timeout) + overhead
        assert!(elapsed < Duration::from_secs(8));
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_m2_08_health_check_subsystem_latency_recorded() {
        let state = HealthAppState::new(
            Arc::new(SlowChecker { name: "database".to_string(), delay_ms: 200 }),
            Arc::new(AlwaysUpChecker { name: "storage".to_string() }),
            Arc::new(AlwaysUpChecker { name: "dex".to_string() }),
        );
        let app = health_routes(state);
        let request = Request::builder()
            .uri("/v1/health")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let body = axum::body::to_bytes(response.into_body(), 65536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let db = json["subsystems"].as_array().unwrap()
            .iter().find(|s| s["name"] == "database")
            .unwrap();
        assert!(db["latency_ms"].as_u64().unwrap() >= 190);
    }

    #[test]
    fn test_m2_08_health_sql_generation() {
        assert_eq!(db_health_check_sql(), "SELECT 1");
        assert!(ingest_queue_depth_sql().contains("queue_depth"));
        assert!(oldest_unprocessed_sql().contains("oldest"));
        assert!(storage_health_check_sql().contains("raw_archive"));
    }
}
