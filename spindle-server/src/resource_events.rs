//! M2-05: Resource event aggregates + drift detection endpoints.
//!
//! Endpoints:
//! - GET /v1/resource-events/aggregates — group by cookbook (+version), resource_type, platform
//! - GET /v1/resource-events/drift — resources by update frequency (convergence storms)
//!
//! Both endpoints query the real `resource_events` table (joined with `nodes`
//! for platform). No in-memory seed data — all results come from the DB.
//! When no DB pool is available (dev mode), endpoints return empty results.

#![allow(warnings)]
use axum::{
    extract::{Query, Request, State},
    middleware,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::ingest::{EnvelopeResponse, API_VERSION, X_REQUEST_ID_HEADER};
use spindle_api::{
    parse_query_string, validate_filter_fields, FilterOp, FilterValue, PaginationResult,
    QueryFilter, VALID_RESOURCE_EVENT_FIELDS,
};

// ── Aggregate types ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct AggregateRow {
    pub id: String,
    pub hour: DateTime<Utc>,
    pub cookbook_name: String,
    pub cookbook_version: Option<String>,
    pub resource_type: String,
    pub platform: String,
    pub count: i32,
    pub sum_duration_ms: i64,
    pub avg_duration_ms: f64,
    pub p50_ms: Option<i32>,
    pub p95_ms: Option<i32>,
    pub p99_ms: Option<i32>,
    pub max_ms: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct AggregatesResponse {
    pub api_version: String,
    pub request_id: String,
    pub data: Vec<AggregateRow>,
    pub pagination: PaginationResult,
}

// ── Drift types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct DriftRow {
    pub resource_id: String,
    pub resource_type: String,
    pub cookbook_name: Option<String>,
    pub platform: Option<String>,
    pub last_updated: DateTime<Utc>,
    pub update_count_24h: i32,
    pub update_count_1h: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct DriftResponse {
    pub api_version: String,
    pub request_id: String,
    pub data: Vec<DriftRow>,
}

// ── App state ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ResourceEventsAppState {
    pub db_pool: Option<sqlx::PgPool>,
    pub metrics: Arc<crate::metrics::MetricsRegistry>,
}

impl ResourceEventsAppState {
    pub fn new(db_pool: Option<sqlx::PgPool>, metrics: Arc<crate::metrics::MetricsRegistry>) -> Self {
        Self { db_pool, metrics }
    }
}

// ── Route builder ────────────────────────────────────────────────────────

pub fn resource_events_routes(state: ResourceEventsAppState) -> Router {
    Router::new()
        .route("/v1/resource-events/aggregates", get(get_aggregates))
        .route("/v1/resource-events/drift", get(get_drift))
        .with_state(state)
        .route_layer(middleware::from_fn(crate::ingest::request_id_middleware))
}

// ── Handlers ─────────────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/v1/resource-events/aggregates",
    tag = "resource-events",
    responses(
        (status = 200, description = "Successful response", body = AggregatesResponse),
        (status = 401, description = "Unauthorized"),
        (status = 400, description = "Bad request"),
    ),
    params(
        ("page" = Option<u32>, Query, description = "Page number"),
        ("per_page" = Option<u32>, Query, description = "Items per page"),
    ),
)]
async fn get_aggregates(
    State(state): State<ResourceEventsAppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    request: Request,
) -> impl IntoResponse {
    let request_id = get_request_id(&request);
    let headers = request.headers();
    let method = request.method().as_str();
    let path = request.uri().path();

    if let Some(_status) = crate::ingest::check_role_authorization(headers, method, path) {
        return EnvelopeResponse::forbidden(
            "auth_required",
            "Access denied by role policy",
            &request_id,
        )
        .into_response();
    }

    let raw_query = build_query_string(&params);
    let filter = match parse_query_string(&raw_query, VALID_RESOURCE_EVENT_FIELDS) {
        Ok(f) => f,
        Err(e) => {
            return EnvelopeResponse::bad_request(
                "bad_request",
                &format!("Invalid filter: {}", e),
                &request_id,
            )
            .into_response();
        }
    };

    if let Err(e) = validate_filter_fields(
        &filter.filters,
        &filter.time_range,
        VALID_RESOURCE_EVENT_FIELDS,
    ) {
        return EnvelopeResponse::bad_request(
            "bad_request",
            &format!("Invalid field: {}", e),
            &request_id,
        )
        .into_response();
    }

    let pool = match &state.db_pool {
        Some(p) => p,
        None => {
            // No DB — return empty results (dev mode fallback)
            let response = AggregatesResponse {
                api_version: API_VERSION.to_string(),
                request_id,
                data: vec![],
                pagination: PaginationResult {
                    total_count: 0,
                    has_more: false,
                    next_cursor: None,
                },
            };
            return Json(response).into_response();
        }
    };

    // Query real resource_events table, joined with nodes for platform
    let rows = query_aggregates_from_db(pool, &filter).await;
    let total_count = rows.len();
    let limit = 1000;
    let has_more = total_count > limit;
    let data = if rows.len() > limit {
        rows[..limit].to_vec()
    } else {
        rows
    };

    tracing::debug!(
        path = "/v1/resource-events/aggregates",
        result_count = data.len(),
        params = %build_query_string(&params),
        "api query result"
    );

    let response = AggregatesResponse {
        api_version: API_VERSION.to_string(),
        request_id,
        data,
        pagination: PaginationResult {
            total_count,
            has_more,
            next_cursor: if has_more {
                Some("cursor-next".into())
            } else {
                None
            },
        },
    };
    Json(response).into_response()
}

#[utoipa::path(
    get,
    path = "/v1/resource-events/drift",
    tag = "resource-events",
    responses(
        (status = 200, description = "Successful response", body = DriftResponse),
        (status = 401, description = "Unauthorized"),
    ),
)]
async fn get_drift(
    State(state): State<ResourceEventsAppState>,
    request: Request,
) -> impl IntoResponse {
    let request_id = get_request_id(&request);
    let headers = request.headers();
    let method = request.method().as_str();
    let path = request.uri().path();

    if let Some(_status) = crate::ingest::check_role_authorization(headers, method, path) {
        return EnvelopeResponse::forbidden(
            "auth_required",
            "Access denied by role policy",
            &request_id,
        )
        .into_response();
    }

    let pool = match &state.db_pool {
        Some(p) => p,
        None => {
            let response = DriftResponse {
                api_version: API_VERSION.to_string(),
                request_id,
                data: vec![],
            };
            return Json(response).into_response();
        }
    };

    let rows = query_drift_from_db(pool).await;

    tracing::debug!(
        path = "/v1/resource-events/drift",
        result_count = rows.len(),
        "api query result"
    );

    let response = DriftResponse {
        api_version: API_VERSION.to_string(),
        request_id,
        data: rows,
    };
    Json(response).into_response()
}

// ── DB queries ──────────────────────────────────────────────────────────

/// Query the real resource_events table for aggregates grouped by
/// cookbook_name, resource_type, platform.
async fn query_aggregates_from_db(
    pool: &sqlx::PgPool,
    filter: &QueryFilter,
) -> Vec<AggregateRow> {
    // Build WHERE clause from filter
    let mut conditions: Vec<String> = Vec::new();
    let mut param_idx = 1u32;

    for f in &filter.filters {
        if let Some(ref val) = f.value {
            let val_str = val.to_string();
            match f.field.as_str() {
                "cookbook_name" => match f.operator {
                    FilterOp::Eq => {
                        conditions.push(format!("re.cookbook_name = ${param_idx}"));
                        param_idx += 1;
                    }
                    FilterOp::Like => {
                        conditions.push(format!("re.cookbook_name ILIKE ${param_idx}"));
                        param_idx += 1;
                    }
                    _ => {}
                },
                "resource_type" if f.operator == FilterOp::Eq => {
                    conditions.push(format!("re.resource_type = ${param_idx}"));
                    param_idx += 1;
                }
                "platform" if f.operator == FilterOp::Eq => {
                    conditions.push(format!("n.platform = ${param_idx}"));
                    param_idx += 1;
                }
                _ => {}
            }
        }
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    // We need to bind params dynamically — build the query with string interpolation
    // for the WHERE clause and bind values separately.
    // Since sqlx doesn't support dynamic param counts easily, we'll build the
    // query with a fixed structure and use optionals.

    // Simplified approach: use a raw query with the WHERE clause built from filter
    let sql = format!(
        r#"
        SELECT
            CONCAT(re.cookbook_name, '-', re.resource_type, '-', COALESCE(n.platform, 'unknown')) as id,
            date_trunc('hour', re.created_at) as hour,
            re.cookbook_name,
            MAX(re.cookbook_version) as cookbook_version,
            re.resource_type,
            COALESCE(n.platform, 'unknown') as platform,
            COUNT(*)::int as count,
            COALESCE(SUM(re.duration_ms), 0)::bigint as sum_duration_ms,
            COALESCE(AVG(re.duration_ms), 0)::float8 as avg_duration_ms,
            percentile_cont(0.5) WITHIN GROUP (ORDER BY re.duration_ms)::int as p50_ms,
            percentile_cont(0.95) WITHIN GROUP (ORDER BY re.duration_ms)::int as p95_ms,
            percentile_cont(0.99) WITHIN GROUP (ORDER BY re.duration_ms)::int as p99_ms,
            COALESCE(MAX(re.duration_ms), 0)::int as max_ms
        FROM resource_events re
        LEFT JOIN nodes n ON re.node_id = n.id
        {where_clause}
        GROUP BY re.cookbook_name, re.resource_type, COALESCE(n.platform, 'unknown'), date_trunc('hour', re.created_at)
        ORDER BY count DESC
        LIMIT 1000
        "#,
        where_clause = where_clause
    );

    // Build a query with dynamic binds
    let mut query = sqlx::query_as::<_, AggregateRowDb>(&sql);

    for f in &filter.filters {
        if let Some(ref val) = f.value {
            let val_str = val.to_string();
            match f.field.as_str() {
                "cookbook_name" if matches!(f.operator, FilterOp::Eq | FilterOp::Like) => {
                    query = query.bind(val_str);
                }
                "resource_type" if f.operator == FilterOp::Eq => {
                    query = query.bind(val_str);
                }
                "platform" if f.operator == FilterOp::Eq => {
                    query = query.bind(val_str);
                }
                _ => {}
            }
        }
    }

    match query.fetch_all(pool).await {
        Ok(rows) => rows.into_iter().map(|r| r.into()).collect(),
        Err(e) => {
            tracing::error!(error = %e, "aggregates query failed");
            vec![]
        }
    }
}

/// Intermediate type for sqlx row mapping
#[derive(sqlx::FromRow)]
struct AggregateRowDb {
    id: String,
    hour: DateTime<Utc>,
    cookbook_name: String,
    cookbook_version: Option<String>,
    resource_type: String,
    platform: String,
    count: i32,
    sum_duration_ms: i64,
    avg_duration_ms: f64,
    p50_ms: Option<i32>,
    p95_ms: Option<i32>,
    p99_ms: Option<i32>,
    max_ms: i32,
}

impl From<AggregateRowDb> for AggregateRow {
    fn from(db: AggregateRowDb) -> Self {
        AggregateRow {
            id: db.id,
            hour: db.hour,
            cookbook_name: db.cookbook_name,
            cookbook_version: db.cookbook_version,
            resource_type: db.resource_type,
            platform: db.platform,
            count: db.count,
            sum_duration_ms: db.sum_duration_ms,
            avg_duration_ms: db.avg_duration_ms,
            p50_ms: db.p50_ms,
            p95_ms: db.p95_ms,
            p99_ms: db.p99_ms,
            max_ms: db.max_ms,
        }
    }
}

/// Query the real resource_events table for drift detection.
/// Ranks resources by how often they change in the last 1h and 24h windows.
async fn query_drift_from_db(pool: &sqlx::PgPool) -> Vec<DriftRow> {
    let sql = r#"
        WITH counts AS (
            SELECT
                re.resource_name as resource_id,
                re.resource_type,
                MAX(re.cookbook_name) as cookbook_name,
                MAX(n.platform) as platform,
                MAX(re.created_at) as last_updated,
                COUNT(*) FILTER (WHERE re.created_at > NOW() - INTERVAL '24 hours')::int as update_count_24h,
                COUNT(*) FILTER (WHERE re.created_at > NOW() - INTERVAL '1 hour')::int as update_count_1h
            FROM resource_events re
            LEFT JOIN nodes n ON re.node_id = n.id
            WHERE re.status IN ('updated', 'failed')
            GROUP BY re.resource_name, re.resource_type
            HAVING COUNT(*) FILTER (WHERE re.created_at > NOW() - INTERVAL '24 hours') > 0
            ORDER BY update_count_24h DESC
            LIMIT 100
        )
        SELECT * FROM counts
    "#;

    match sqlx::query_as::<_, DriftRowDb>(sql)
        .fetch_all(pool)
        .await
    {
        Ok(rows) => rows.into_iter().map(|r| r.into()).collect(),
        Err(e) => {
            tracing::error!(error = %e, "drift query failed");
            vec![]
        }
    }
}

#[derive(sqlx::FromRow)]
struct DriftRowDb {
    resource_id: String,
    resource_type: String,
    cookbook_name: Option<String>,
    platform: Option<String>,
    last_updated: DateTime<Utc>,
    update_count_24h: i32,
    update_count_1h: i32,
}

impl From<DriftRowDb> for DriftRow {
    fn from(db: DriftRowDb) -> Self {
        DriftRow {
            resource_id: db.resource_id,
            resource_type: db.resource_type,
            cookbook_name: db.cookbook_name,
            platform: db.platform,
            last_updated: db.last_updated,
            update_count_24h: db.update_count_24h,
            update_count_1h: db.update_count_1h,
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn build_query_string(params: &std::collections::HashMap<String, String>) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("&")
}

fn get_request_id(request: &Request) -> String {
    request
        .headers()
        .get(X_REQUEST_ID_HEADER)
        .and_then(|h| h.to_str().ok())
        .unwrap_or(&uuid::Uuid::new_v4().to_string())
        .to_string()
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use tower::ServiceExt;

    fn make_state() -> ResourceEventsAppState {
        ResourceEventsAppState::new(
            None, // No DB — tests use the empty-results fallback
            std::sync::Arc::new(crate::metrics::MetricsRegistry::new()),
        )
    }

    fn make_app() -> Router {
        let state = make_state();
        Router::new()
            .route("/v1/resource-events/aggregates", get(get_aggregates))
            .route("/v1/resource-events/drift", get(get_drift))
            .with_state(state)
            .route_layer(middleware::from_fn(crate::ingest::request_id_middleware))
    }

    #[tokio::test]
    async fn test_get_aggregates_returns_empty_without_db() {
        let app = make_app();
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/resource-events/aggregates")
                    .header("accept", "application/json")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let response: AggregatesResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(response.data.len(), 0);
        assert_eq!(response.api_version, API_VERSION);
    }

    #[tokio::test]
    async fn test_get_drift_returns_empty_without_db() {
        let app = make_app();
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/resource-events/drift")
                    .header("accept", "application/json")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let response: DriftResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(response.data.len(), 0);
        assert_eq!(response.api_version, API_VERSION);
    }

    #[tokio::test]
    async fn test_get_aggregates_unknown_field_rejected() {
        let app = make_app();
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/resource-events/aggregates?filter[garbage]=value")
                    .header("accept", "application/json")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
