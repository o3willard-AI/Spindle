//! M2: Admin endpoints — GET /v1/admin/dead-letter
//!
//! Provides read-only access to the `pipeline_dead_letter` table for
//! operators and administrators. Requires admin scope.
//!
//! ## Endpoints
//! - `GET /v1/admin/dead-letter` — list dead-lettered jobs with pagination
//!
//! ## Design decisions
//! - Requires admin role (checked via X-User-Role header set by require_bearer_token)
//! - Uses limit/offset pagination
//! - Returns { items: [...], total: N } envelope
//! - DB-backed: only mounted when a Postgres pool is available

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json, Router,
    routing::get,
    middleware::Next,
    response::Response,
    extract::Request,
};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, FromRow};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::ingest::{API_VERSION, X_USER_ROLE_HEADER, ErrorResponse};

// ── Response types ──────────────────────────────────────────────────────

/// A single dead-letter entry from the pipeline_dead_letter table.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct DeadLetterEntry {
    pub id: Uuid,
    pub archive_reference: String,
    pub error_message: String,
    pub error_type: String,
    pub retry_count: i32,
    pub payload_type: String,
    pub node_name: Option<String>,
    pub run_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Paginated response for dead-letter entries.
#[derive(Debug, Serialize)]
pub struct DeadLetterResponse {
    pub api_version: String,
    pub items: Vec<DeadLetterEntry>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
    pub has_more: bool,
}

// ── Query params ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DeadLetterParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

// ── App state ───────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AdminAppState {
    pub pool: PgPool,
}

impl AdminAppState {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

// ── Routes ──────────────────────────────────────────────────────────────

/// Build the admin router. Mounted only when a Postgres pool is available.
pub fn admin_routes(state: AdminAppState) -> Router {
    Router::new()
        .route("/v1/admin/dead-letter", get(list_dead_letter))
        .route_layer(axum::middleware::from_fn(require_admin))
        .with_state(state)
}

// ── Middleware ──────────────────────────────────────────────────────────

/// Middleware that requires the caller to have the "admin" role.
/// Reads the X-User-Role header set by require_bearer_token.
pub async fn require_admin(
    request: Request,
    next: Next,
) -> Response {
    let role = request
        .headers()
        .get(X_USER_ROLE_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("viewer");

    if role != "admin" {
        let body = serde_json::json!({
            "api_version": API_VERSION,
            "error": {
                "code": "forbidden",
                "message": "admin role required",
            }
        });
        return (StatusCode::FORBIDDEN, Json(body)).into_response();
    }

    next.run(request).await
}

// ── Handlers ────────────────────────────────────────────────────────────

/// GET /v1/admin/dead-letter — list dead-lettered jobs.
///
/// Requires admin role. Returns paginated entries from the
/// `pipeline_dead_letter` table, ordered by `created_at DESC`.
///
/// Query params:
/// - `limit` (default 50, max 200): number of entries to return
/// - `offset` (default 0): number of entries to skip
pub async fn list_dead_letter(
    State(state): State<AdminAppState>,
    Query(params): Query<DeadLetterParams>,
) -> axum::response::Response {
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let offset = params.offset.unwrap_or(0).max(0);

    let total: i64 = match sqlx::query_scalar("SELECT COUNT(*) FROM pipeline_dead_letter")
        .fetch_one(&state.pool)
        .await
    {
        Ok(count) => count,
        Err(e) => {
            tracing::error!("Failed to count dead-letter entries: {}", e);
            let body = serde_json::json!({
                "api_version": API_VERSION,
                "error": {
                    "code": "internal_error",
                    "message": "Failed to query dead-letter table",
                }
            });
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response();
        }
    };

    let rows: Vec<DeadLetterEntry> = match sqlx::query_as::<_, DeadLetterEntry>(
        r#"
        SELECT id, archive_reference, error_message, error_type, retry_count,
               payload_type, node_name, run_id, created_at, updated_at
        FROM pipeline_dead_letter
        ORDER BY created_at DESC
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("Failed to query dead-letter entries: {}", e);
            let body = serde_json::json!({
                "api_version": API_VERSION,
                "error": {
                    "code": "internal_error",
                    "message": "Failed to query dead-letter table",
                }
            });
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response();
        }
    };

    let has_more = offset + limit < total;

    Json(DeadLetterResponse {
        api_version: API_VERSION.to_string(),
        items: rows,
        total,
        limit,
        offset,
        has_more,
    })
    .into_response()
}
