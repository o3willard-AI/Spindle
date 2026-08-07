//! M2-05: Resource event aggregates + drift detection endpoints.
//!
//! Endpoints:
//! - GET /v1/resource-events/aggregates — group by cookbook (+version), resource_type, platform
//! - GET /v1/resource-events/drift — resources by update frequency (convergence storms)

use axum::{
    extract::{Query, Request, State},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use std::sync::Arc;
use uuid::Uuid;

use spindle_api::{
    parse_query_string, validate_filter_fields, VALID_RESOURCE_EVENT_FIELDS,
    QueryFilter, FilterOp, FilterValue, TimeRange, PaginationParams, PaginationResult,
};
use spindle_authz::Scope;
use crate::ingest::{EnvelopeResponse, X_REQUEST_ID_HEADER, API_VERSION};

// ── Aggregate types ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AggregatesResponse {
    pub api_version: String,
    pub request_id: String,
    pub data: Vec<AggregateRow>,
    pub pagination: PaginationResult,
}

// ── Drift types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DriftRow {
    pub resource_id: String,
    pub resource_type: String,
    pub cookbook_name: Option<String>,
    pub platform: Option<String>,
    pub last_updated: DateTime<Utc>,
    pub update_count_24h: i32,
    pub update_count_1h: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DriftResponse {
    pub api_version: String,
    pub request_id: String,
    pub data: Vec<DriftRow>,
}

// ── In-memory store ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct RollupStore {
    rollups: Arc<std::sync::RwLock<Vec<AggregateRow>>>,
    drift_events: Arc<std::sync::RwLock<Vec<DriftRow>>>,
}

impl RollupStore {
    pub fn new() -> Self {
        let mut rollups = Vec::new();
        let mut drift_events = Vec::new();
        let now = Utc::now();
        let hour = now - chrono::Duration::hours(1);

        let entries: Vec<AggregateRow> = vec![
            AggregateRow { id: "ru-001".into(), hour: hour.clone(), cookbook_name: "apache2".into(), cookbook_version: Some("3.1.0".into()), resource_type: "service".into(), platform: "ubuntu".into(), count: 42, sum_duration_ms: 21000, avg_duration_ms: 500.0, p50_ms: Some(480), p95_ms: Some(920), p99_ms: Some(1050), max_ms: 1200 },
            AggregateRow { id: "ru-002".into(), hour: hour.clone(), cookbook_name: "apache2".into(), cookbook_version: Some("3.1.0".into()), resource_type: "file".into(), platform: "ubuntu".into(), count: 30, sum_duration_ms: 9000, avg_duration_ms: 300.0, p50_ms: Some(290), p95_ms: Some(580), p99_ms: Some(650), max_ms: 700 },
            AggregateRow { id: "ru-003".into(), hour: hour.clone(), cookbook_name: "postgresql".into(), cookbook_version: Some("5.2.1".into()), resource_type: "package".into(), platform: "centos".into(), count: 18, sum_duration_ms: 10800, avg_duration_ms: 600.0, p50_ms: Some(580), p95_ms: Some(1100), p99_ms: Some(1300), max_ms: 1500 },
            AggregateRow { id: "ru-004".into(), hour: hour.clone(), cookbook_name: "monitoring".into(), cookbook_version: Some("1.0.0".into()), resource_type: "service".into(), platform: "ubuntu".into(), count: 25, sum_duration_ms: 5000, avg_duration_ms: 200.0, p50_ms: Some(190), p95_ms: Some(400), p99_ms: Some(460), max_ms: 500 },
            AggregateRow { id: "ru-005".into(), hour: hour.clone(), cookbook_name: "nginx".into(), cookbook_version: Some("2.5.0".into()), resource_type: "service".into(), platform: "ubuntu".into(), count: 35, sum_duration_ms: 14000, avg_duration_ms: 400.0, p50_ms: Some(380), p95_ms: Some(780), p99_ms: Some(850), max_ms: 900 },
        ];
        rollups.extend(entries);

        let drift_entries: Vec<DriftRow> = vec![
            DriftRow { resource_id: "node-001".into(), resource_type: "chef-client".into(), cookbook_name: Some("apache2".into()), platform: Some("ubuntu".into()), last_updated: now - chrono::Duration::minutes(5), update_count_24h: 48, update_count_1h: 12 },
            DriftRow { resource_id: "node-002".into(), resource_type: "chef-client".into(), cookbook_name: Some("postgresql".into()), platform: Some("centos".into()), last_updated: now - chrono::Duration::hours(2), update_count_24h: 12, update_count_1h: 2 },
            DriftRow { resource_id: "node-003".into(), resource_type: "chef-client".into(), cookbook_name: Some("nginx".into()), platform: Some("ubuntu".into()), last_updated: now - chrono::Duration::minutes(2), update_count_24h: 60, update_count_1h: 15 },
        ];
        drift_events.extend(drift_entries);

        Self {
            rollups: Arc::new(std::sync::RwLock::new(rollups)),
            drift_events: Arc::new(std::sync::RwLock::new(drift_events)),
        }
    }

    fn query_aggregates(&self, filter: &QueryFilter) -> (Vec<AggregateRow>, PaginationResult) {
        let all = self.rollups.read().unwrap().clone();
        let mut filtered: Vec<AggregateRow> = all;

        // Apply field filters
        for f in &filter.filters {
            filtered = filtered.into_iter().filter(|row| {
                if let Some(ref val) = f.value {
                    match f.field.as_str() {
                        "cookbook_name" => {
                            match f.operator {
                                FilterOp::Eq => &val.to_string() == &row.cookbook_name,
                                FilterOp::Like => row.cookbook_name.contains(&val.to_string()),
                                _ => true,
                            }
                        }
                        "resource_type" => {
                            match f.operator {
                                FilterOp::Eq => &val.to_string() == &row.resource_type,
                                _ => true,
                            }
                        }
                        "platform" => {
                            match f.operator {
                                FilterOp::Eq => &val.to_string() == &row.platform,
                                _ => true,
                            }
                        }
                        _ => true,
                    }
                } else {
                    true
                }
            }).collect();
        }

        // Time range filter on hour
        if let Ok(tr) = filter.time_range {
            filtered = filtered.into_iter().filter(|row| {
                if let Some(ref start) = tr.start_time {
                    if row.hour < *start { return false; }
                }
                if let Some(ref end) = tr.end_time {
                    if row.hour > *end { return false; }
                }
                true
            }).collect();
        }

        let total_count = filtered.len();
        filtered.sort_by(|a, b| b.count.cmp(&a.count));

        let limit = 1000;
        let has_more = total_count > limit;
        let data = if filtered.len() > limit {
            filtered[..limit].to_vec()
        } else {
            filtered
        };

        (data, PaginationResult {
            total_count,
            has_more,
            next_cursor: if has_more { Some("cursor-next".into()) } else { None },
        })
    }

    fn query_drift(&self) -> Vec<DriftRow> {
        self.drift_events.read().unwrap().clone()
    }
}

// ── App state ────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct AggregatesAppState {
    pub store: Arc<RollupStore>,
}

impl AggregatesAppState {
    pub fn new(store: Arc<RollupStore>) -> Self {
        Self { store }
    }
}

#[derive(Debug)]
pub struct DriftAppState {
    pub store: Arc<RollupStore>,
}

impl DriftAppState {
    pub fn new(store: Arc<RollupStore>) -> Self {
        Self { store }
    }
}

// ── Route builder ────────────────────────────────────────────────────────

pub fn resource_events_routes(agg_state: AggregatesAppState, drift_state: DriftAppState) -> Router {
    Router::new()
        .route("/v1/resource-events/aggregates", get(get_aggregates))
        .route("/v1/resource-events/drift", get(get_drift))
        .with_state(agg_state)
        .with_state(drift_state)
        .route_layer(middleware::from_fn(crate::ingest::request_id_middleware))
}

// ── Handlers ─────────────────────────────────────────────────────────────

async fn get_aggregates(
    State(state): State<AggregatesAppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    request: Request,
) -> impl IntoResponse {
    let request_id = get_request_id(&request);

    let raw_query = build_query_string(&params);
    let filter = match parse_query_string(&raw_query, VALID_RESOURCE_EVENT_FIELDS) {
        Ok(f) => f,
        Err(e) => {
            return EnvelopeResponse::bad_request("bad_request", &format!("Invalid filter: {}", e), &request_id).into_response();
        }
    };

    if let Err(e) = validate_filter_fields(&filter.filters, &filter.time_range, VALID_RESOURCE_EVENT_FIELDS) {
        return EnvelopeResponse::bad_request("bad_request", &format!("Invalid field: {}", e), &request_id).into_response();
    }

    let (rows, pagination_result) = state.store.query_aggregates(&filter);
    let response = AggregatesResponse {
        api_version: API_VERSION.to_string(),
        request_id,
        data: rows,
        pagination: pagination_result,
    };
    Json(response).into_response()
}

async fn get_drift(
    State(state): State<DriftAppState>,
    request: Request,
) -> impl IntoResponse {
    let request_id = get_request_id(&request);
    let rows = state.store.query_drift();
    let response = DriftResponse {
        api_version: API_VERSION.to_string(),
        request_id,
        data: rows,
    };
    Json(response).into_response()
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn build_query_string(params: &std::collections::HashMap<String, String>) -> String {
    params.iter().map(|(k, v)| format!("{}={}", k, v)).collect::<Vec<_>>().join("&")
}

fn get_request_id(request: &Request) -> String {
    request
        .headers()
        .get(X_REQUEST_ID_HEADER)
        .and_then(|h| h.to_str().ok())
        .unwrap_or(&Uuid::new_v4().to_string())
        .to_string()
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_agg_state() -> AggregatesAppState {
        let store = Arc::new(RollupStore::new());
        AggregatesAppState::new(store)
    }

    fn make_drift_state() -> DriftAppState {
        let store = Arc::new(RollupStore::new());
        DriftAppState::new(store)
    }

    fn make_agg_app() -> Router {
        let state = make_agg_state();
        Router::new()
            .route("/v1/resource-events/aggregates", get(get_aggregates))
            .with_state(state)
            .route_layer(middleware::from_fn(crate::ingest::request_id_middleware))
    }

    fn make_drift_app() -> Router {
        let state = make_drift_state();
        Router::new()
            .route("/v1/resource-events/drift", get(get_drift))
            .with_state(state)
            .route_layer(middleware::from_fn(crate::ingest::request_id_middleware))
    }

    // ── Aggregates tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_aggregates_returns_all() {
        let app = make_agg_app();
        let resp = app.clone().oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/v1/resource-events/aggregates")
                .header("accept", "application/json")
                .body(axum::body::Body::empty())
                .unwrap(),
        ).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let response: AggregatesResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(response.data.len(), 5);
        assert_eq!(response.api_version, API_VERSION);
    }

    #[tokio::test]
    async fn test_get_aggregates_filter_by_cookbook() {
        let app = make_agg_app();
        let resp = app.clone().oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/v1/resource-events/aggregates?filter[cookbook_name]=apache2")
                .header("accept", "application/json")
                .body(axum::body::Body::empty())
                .unwrap(),
        ).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let response: AggregatesResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(response.data.len(), 2);
        for row in &response.data {
            assert_eq!(row.cookbook_name, "apache2");
        }
    }

    #[tokio::test]
    async fn test_get_aggregates_filter_by_platform() {
        let app = make_agg_app();
        let resp = app.clone().oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/v1/resource-events/aggregates?filter[platform]=centos")
                .header("accept", "application/json")
                .body(axum::body::Body::empty())
                .unwrap(),
        ).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let response: AggregatesResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(response.data.len(), 1);
        assert_eq!(response.data[0].platform, "centos");
    }

    #[tokio::test]
    async fn test_get_aggregates_filter_by_resource_type() {
        let app = make_agg_app();
        let resp = app.clone().oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/v1/resource-events/aggregates?filter[resource_type]=service")
                .header("accept", "application/json")
                .body(axum::body::Body::empty())
                .unwrap(),
        ).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let response: AggregatesResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(response.data.len(), 3);
        for row in &response.data {
            assert_eq!(row.resource_type, "service");
        }
    }

    #[tokio::test]
    async fn test_get_aggregates_unknown_field_rejected() {
        let app = make_agg_app();
        let resp = app.clone().oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/v1/resource-events/aggregates?filter[garbage]=value")
                .header("accept", "application/json")
                .body(axum::body::Body::empty())
                .unwrap(),
        ).await.unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_get_aggregates_metrics_fields_present() {
        let app = make_agg_app();
        let resp = app.clone().oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/v1/resource-events/aggregates")
                .header("accept", "application/json")
                .body(axum::body::Body::empty())
                .unwrap(),
        ).await.unwrap();

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let response: AggregatesResponse = serde_json::from_slice(&body).unwrap();

        let row = &response.data[0];
        assert!(row.count > 0);
        assert!(row.sum_duration_ms > 0);
        assert!(row.avg_duration_ms > 0.0);
        assert!(row.max_ms > 0);
    }

    // ── Drift tests ───────────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_drift_returns_entries() {
        let app = make_drift_app();
        let resp = app.clone().oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/v1/resource-events/drift")
                .header("accept", "application/json")
                .body(axum::body::Body::empty())
                .unwrap(),
        ).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let response: DriftResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(response.data.len(), 3);
        assert_eq!(response.api_version, API_VERSION);
    }

    #[tokio::test]
    async fn test_drift_entries_have_fields() {
        let app = make_drift_app();
        let resp = app.clone().oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/v1/resource-events/drift")
                .header("accept", "application/json")
                .body(axum::body::Body::empty())
                .unwrap(),
        ).await.unwrap();

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let response: DriftResponse = serde_json::from_slice(&body).unwrap();

        for entry in &response.data {
            assert!(!entry.resource_id.is_empty());
            assert!(!entry.resource_type.is_empty());
            assert!(entry.update_count_24h > 0);
            assert!(entry.update_count_1h > 0);
        }
    }

    // ── Store unit tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_store_new_has_seed_data() {
        let store = RollupStore::new();
        let rows = store.rollups.read().unwrap().clone();
        assert_eq!(rows.len(), 5);
    }

    #[tokio::test]
    async fn test_store_query_drift() {
        let store = RollupStore::new();
        let drift = store.query_drift();
        assert_eq!(drift.len(), 3);
    }

    #[tokio::test]
    async fn test_store_query_aggregates_with_filter() {
        let store = RollupStore::new();
        let mut filter = QueryFilter::default();
        filter.filters = vec![
            spindle_api::Filter {
                field: "cookbook_name".into(),
                operator: FilterOp::Eq,
                value: Some(FilterValue::Str("nginx".into())),
            },
        ];
        let (rows, _) = store.query_aggregates(&filter);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cookbook_name, "nginx");
    }
}