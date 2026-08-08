//! M2-08: Cookbook inventory endpoint — GET /v1/cookbooks
//!
//! Provides an inventory of cookbook versions running across nodes,
//! including last-seen timestamps, node counts per version, and
//! platform breakdowns.
//!
//! ## Endpoint
//! - `GET /v1/cookbooks` — list cookbooks with version-to-node mappings
//!
//! ## Design decisions
//! - Uses Mark's filter grammar (spindle-api) for field selection
//! - Cursor pagination for large inventories
//! - In-memory store for testability (no PostgreSQL required)
//! - Cookbooks grouped by name, then by version, with node lists

use axum::{
    extract::{Query, Request, State},
    http::{StatusCode},
    middleware,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::collections::HashMap;
use uuid::Uuid;
use chrono::{DateTime, Utc};

use spindle_api::{
    parse_query_string, parse_pagination, VALID_COOKBOOK_FIELDS,
    encode_cursor, decode_cursor, PaginationParams, PaginationResult,
    QueryFilter, FilterOp, FilterValue,
};
use spindle_store::{CookbookUsage as StoreCookbookUsage, CookbookUsageStore};
use spindle_authz::Scope;

use crate::ingest::{EnvelopeResponse, X_REQUEST_ID_HEADER, API_VERSION};

// ── Response types ──────────────────────────────────────────────────────────

/// A single cookbook version running on one or more nodes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CookbookVersionInfo {
    pub cookbook_name: String,
    pub cookbook_version: String,
    /// Number of nodes running this version.
    pub node_count: usize,
    /// Nodes running this version (UUIDs).
    pub node_ids: Vec<Uuid>,
    /// First time this version was observed.
    pub first_seen: DateTime<Utc>,
    /// Most recent observation of this version.
    pub last_seen: DateTime<Utc>,
    /// Total resource count across all nodes for this version.
    pub total_resource_count: i32,
}

/// Cookbook inventory entry — one per cookbook name with all versions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CookbookInventoryEntry {
    pub name: String,
    pub versions: Vec<CookbookVersionInfo>,
    /// Total node count across all versions (unique nodes).
    pub total_nodes: usize,
    /// Last seen across all versions.
    pub last_seen: DateTime<Utc>,
}

/// Envelope for cookbook list responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookbookListResponse {
    pub api_version: String,
    pub request_id: String,
    pub data: Vec<CookbookInventoryEntry>,
    pub pagination: PaginationResult,
    /// Data provenance — absent for direct data, present for rollup-derived data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<crate::ingest::Provenance>,
    /// Stripped attributes marker — true when compliance-auditor role strips sensitive attributes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stripped_attributes: Option<bool>,
}

// ── Store trait ───────────────────────────────────────────────────────────────

/// Extended CookbookUsageStore with inventory aggregation.
#[async_trait::async_trait]
pub trait CookbookInventoryStore: Send + Sync + std::fmt::Debug {
    /// Get aggregated cookbook inventory matching the filter.
    /// Groups by cookbook name, then by version.
    async fn get_cookbook_inventory(
        &self,
        filter: &QueryFilter,
        pagination: &PaginationParams,
        scope: &Scope,
    ) -> std::result::Result<(Vec<CookbookInventoryEntry>, PaginationResult), StoreError>;
}

// ── Store error ──────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Scope denied: {0}")]
    ScopeDenied(String),
    #[error("Filter error: {0}")]
    FilterError(String),
    #[error("Query failed: {0}")]
    QueryFailed(String),
    #[error("Storage error: {0}")]
    Storage(String),
}

// ── In-memory store for testing ───────────────────────────────────────────────

/// In-memory implementation of CookbookInventoryStore.
#[derive(Debug, Clone, Default)]
pub struct InMemoryCookbookStore {
    pub usage: Arc<std::sync::Mutex<Vec<StoreCookbookUsage>>>,
}

impl InMemoryCookbookStore {
    pub fn new() -> Self {
        Self {
            usage: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    pub fn insert_usage(&self, usage: StoreCookbookUsage) {
        self.usage.lock().unwrap().push(usage);
    }
}

#[async_trait::async_trait]
impl CookbookInventoryStore for InMemoryCookbookStore {
    async fn get_cookbook_inventory(
        &self,
        filter: &QueryFilter,
        pagination: &PaginationParams,
        _scope: &Scope,
    ) -> std::result::Result<(Vec<CookbookInventoryEntry>, PaginationResult), StoreError> {
        let usage = self.usage.lock().unwrap();

        // Group by cookbook_name, then by cookbook_version
        let mut grouped: HashMap<String, HashMap<String, Vec<&StoreCookbookUsage>>> = HashMap::new();
        for u in usage.iter() {
            let name = &u.cookbook_name;
            let ver = &u.cookbook_version;
            grouped.entry(name.clone())
                .or_default()
                .entry(ver.clone())
                .or_default()
                .push(u);
        }

        // Apply cookbook filter
        let mut entries: Vec<CookbookInventoryEntry> = grouped.into_iter()
            .filter_map(|(name, versions)| {
                // Check cookbook name filter
                let mut matches = true;
                for f in &filter.filters {
                    if f.field == "name" || f.field == "cookbook" {
                        if let Some(FilterValue::Str(val)) = &f.value {
                            if name != *val {
                                matches = false;
                            }
                        }
                    }
                }
                if !matches {
                    return None;
                }

                let mut version_infos: Vec<CookbookVersionInfo> = versions.into_iter()
                    .map(|(ver, usages)| {
                        let node_ids: Vec<Uuid> = usages.iter().map(|u| u.node_id).collect::<std::collections::HashSet<_>>().into_iter().collect();
                        let first_seen = usages.iter().map(|u| u.first_seen).min().unwrap();
                        let last_seen = usages.iter().map(|u| u.last_seen).max().unwrap();
                        let total = usages.iter().map(|u| u.count).sum();
                        CookbookVersionInfo {
                            cookbook_name: name.clone(),
                            cookbook_version: ver,
                            node_count: node_ids.len(),
                            node_ids,
                            first_seen,
                            last_seen,
                            total_resource_count: total,
                        }
                    })
                    .collect();

                // Sort versions
                version_infos.sort_by(|a, b| a.cookbook_version.cmp(&b.cookbook_version));

                let total_nodes: std::collections::HashSet<Uuid> = version_infos.iter()
                    .flat_map(|v| &v.node_ids)
                    .cloned()
                    .collect();
                let last_seen = version_infos.iter()
                    .map(|v| v.last_seen)
                    .max()
                    .unwrap();

                Some(CookbookInventoryEntry {
                    name,
                    versions: version_infos,
                    total_nodes: total_nodes.len(),
                    last_seen,
                })
            })
            .collect();

        // Sort by cookbook name
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        // Cursor pagination
        let (items, total_count, next_cursor) = apply_cursor_pagination(
            &entries, pagination, &|e| e.name.clone(),
        );

        let result = PaginationResult::from_query(
            pagination.limit, items.len(), total_count, next_cursor,
        );

        Ok((items, result))
    }
}

// ── Cursor pagination helper ──────────────────────────────────────────────────

fn apply_cursor_pagination<T: Clone>(
    items: &[T],
    pagination: &PaginationParams,
    id_fn: &dyn Fn(&T) -> String,
) -> (Vec<T>, usize, Option<String>) {
    let total_count = items.len();
    if total_count == 0 {
        return (Vec::new(), 0, None);
    }

    let start_idx = if let Some(cursor) = &pagination.cursor {
        if let Some((_sort_val, _cursor_id, _direction)) = decode_cursor(cursor) {
            items.iter().position(|item| {
                id_fn(item) == cursor.as_str()
            }).map(|idx| idx + 1).unwrap_or(0)
        } else {
            0
        }
    } else {
        0
    };

    let end_idx = (start_idx + pagination.limit).min(total_count);
    let page_items: Vec<T> = items[start_idx..end_idx].to_vec();

    let next_cursor = if end_idx < total_count {
        let last = &items[end_idx - 1];
        Some(encode_cursor(&id_fn(last), Uuid::nil(), &pagination.sort_direction))
    } else {
        None
    };

    (page_items, total_count, next_cursor)
}

// ── App state ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CookbookAppState {
    pub store: Arc<dyn CookbookInventoryStore>,
}

impl CookbookAppState {
    pub fn new(store: Arc<dyn CookbookInventoryStore>) -> Self {
        Self { store }
    }
}

// ── Route builder ────────────────────────────────────────────────────────────

/// Build the cookbooks router with M2-08 routes.
pub fn cookbook_routes(state: CookbookAppState) -> Router {
    Router::new()
        .route("/v1/cookbooks", get(list_cookbooks))
        .with_state(state)
        .route_layer(middleware::from_fn(crate::ingest::request_id_middleware))
}

// ── Handler ───────────────────────────────────────────────────────────────────

/// Handler for GET /v1/cookbooks — list cookbook inventory.
pub async fn list_cookbooks(
    State(state): State<CookbookAppState>,
    Query(params): Query<HashMap<String, String>>,
    request: Request,
) -> impl IntoResponse {
    let request_id = get_request_id(&request);
    let headers = request.headers();
    let method = request.method().as_str();
    let path = request.uri().path();

    // RBAC: check role authorization
    if let Some(status) = crate::ingest::check_role_authorization(headers, method, path) {
        return EnvelopeResponse::forbidden("auth_required", "Access denied by role policy", &request_id).into_response();
    }

    let raw_query = build_query_string(&params);
    let filter = match parse_query_string(&raw_query, VALID_COOKBOOK_FIELDS) {
        Ok(f) => f,
        Err(e) => {
            return EnvelopeResponse::bad_request("bad_request", &format!("Invalid filter: {e}"), &request_id).into_response();
        }
    };

    let pagination = match parse_pagination(&raw_query, "name") {
        Ok(p) => p,
        Err(e) => {
            return EnvelopeResponse::bad_request("bad_request", &format!("Invalid pagination: {e}"), &request_id).into_response();
        }
    };

    // Extract scope from request headers
    let scope = crate::ingest::extract_scope(headers);
    let is_auditor = scope.is_compliance_auditor() && !scope.is_admin();
    match state.store.get_cookbook_inventory(&filter, &pagination, &scope).await {
        Ok((items, pagination_result)) => {
            let response = CookbookListResponse {
                api_version: API_VERSION.to_string(),
                request_id,
                data: items,
                pagination: pagination_result,
                provenance: None,
                stripped_attributes: if is_auditor { Some(true) } else { None },
            };
            Json(response).into_response()
        }
        Err(StoreError::ScopeDenied(msg)) => {
            EnvelopeResponse::forbidden("scope_denied", &msg, &request_id).into_response()
        }
        Err(e) => {
            EnvelopeResponse::bad_request("store_error", &format!("{e}"), &request_id).into_response()
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn get_request_id(request: &Request) -> String {
    if let Some(rid) = request.extensions().get::<crate::ingest::RequestId>() {
        rid.0.clone()
    } else {
        crate::ingest::new_request_id()
    }
}

fn build_query_string(params: &HashMap<String, String>) -> String {
    let mut pairs: Vec<String> = params.iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect();
    pairs.sort();
    pairs.join("&")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body as AxumBody;
    use tower::ServiceExt;
    use std::collections::HashSet;

    fn make_usage(node_id: Uuid, name: &str, version: &str, count: i32) -> StoreCookbookUsage {
        StoreCookbookUsage {
            id: Uuid::new_v4(),
            node_id,
            run_id: Uuid::new_v4(),
            cookbook_name: name.to_string(),
            cookbook_version: version.to_string(),
            resource_type: "package".to_string(),
            platform: Some("debian".to_string()),
            first_seen: Utc::now() - chrono::Duration::hours(1),
            last_seen: Utc::now(),
            count,
            created_at: Utc::now(),
        }
    }

    fn make_cookbook_app(num_nodes: usize) -> (CookbookAppState, Vec<Uuid>) {
        let store = InMemoryCookbookStore::new();
        let mut node_ids = Vec::new();
        for i in 0..num_nodes {
            let node_id = Uuid::new_v4();
            node_ids.push(node_id);
            store.insert_usage(make_usage(node_id, "base", "1.0.0", 10));
            store.insert_usage(make_usage(node_id, "base", "2.0.0", 15));
            store.insert_usage(make_usage(node_id, "app", "3.1.0", 5));
        }
        let state = CookbookAppState::new(Arc::new(store));
        (state, node_ids)
    }

    #[tokio::test]
    async fn test_m2_08_cookbook_inventory_returns_envelope() {
        let (state, _) = make_cookbook_app(3);
        let app = cookbook_routes(state);
        let request = Request::builder()
            .uri("/v1/cookbooks")
            .body(AxumBody::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 65536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["api_version"], "v1");
        assert!(json["request_id"].as_str().is_some());
        assert!(json["data"].is_array());
        assert!(json["pagination"].is_object());
    }

    #[tokio::test]
    async fn test_m2_08_cookbook_inventory_groups_by_name_and_version() {
        let (state, _) = make_cookbook_app(3);
        let app = cookbook_routes(state);
        let request = Request::builder()
            .uri("/v1/cookbooks")
            .body(AxumBody::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let body = axum::body::to_bytes(response.into_body(), 65536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let data = json["data"].as_array().unwrap();
        let cookbook_names: Vec<_> = data.iter()
            .map(|v| v["name"].as_str().unwrap())
            .collect();
        assert!(cookbook_names.contains(&"base"));
        assert!(cookbook_names.contains(&"app"));

        // base should have 2 versions
        let base = data.iter().find(|v| v["name"] == "base").unwrap();
        assert_eq!(base["versions"].as_array().unwrap().len(), 2);
        assert_eq!(base["total_nodes"], 3);

        // Each version should have 3 nodes
        for version in base["versions"].as_array().unwrap() {
            assert_eq!(version["node_count"], 3);
        }
    }

    #[tokio::test]
    async fn test_m2_08_cookbook_inventory_last_seen_and_first_seen() {
        let (state, _) = make_cookbook_app(2);
        let app = cookbook_routes(state);
        let request = Request::builder()
            .uri("/v1/cookbooks")
            .body(AxumBody::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let body = axum::body::to_bytes(response.into_body(), 65536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let data = json["data"].as_array().unwrap();
        let base = data.iter().find(|v| v["name"] == "base").unwrap();
        for version in base["versions"].as_array().unwrap() {
            assert!(version["first_seen"].is_string());
            assert!(version["last_seen"].is_string());
            assert!(version["total_resource_count"].as_i64().is_some());
        }
    }

    #[tokio::test]
    async fn test_m2_08_cookbook_inventory_filter_by_name() {
        let (state, _) = make_cookbook_app(2);
        let app = cookbook_routes(state);
        let request = Request::builder()
            .uri("/v1/cookbooks?filter[name]=app")
            .body(AxumBody::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let body = axum::body::to_bytes(response.into_body(), 65536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let data = json["data"].as_array().unwrap();
        assert_eq!(data.len(), 1);
        assert_eq!(data[0]["name"], "app");
    }

    #[tokio::test]
    async fn test_m2_08_cookbook_inventory_x_request_id() {
        let (state, _) = make_cookbook_app(1);
        let app = cookbook_routes(state);
        let request = Request::builder()
            .uri("/v1/cookbooks")
            .header(X_REQUEST_ID_HEADER, "req-test-abc-456")
            .body(AxumBody::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let header_val = response.headers().get(X_REQUEST_ID_HEADER).unwrap();
        assert_eq!(header_val.to_str().unwrap(), "req-test-abc-456");
    }

    #[tokio::test]
    async fn test_m2_08_cookbook_inventory_empty_store() {
        let store = InMemoryCookbookStore::new();
        let state = CookbookAppState::new(Arc::new(store));
        let app = cookbook_routes(state);
        let request = Request::builder()
            .uri("/v1/cookbooks")
            .body(AxumBody::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 65536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["data"].as_array().unwrap().len(), 0);
        assert_eq!(json["pagination"]["total_count"], 0);
        assert_eq!(json["pagination"]["has_more"], false);
    }

    #[tokio::test]
    async fn test_m2_08_cookbook_inventory_pagination() {
        let (state, _) = make_cookbook_app(1);
        let app = cookbook_routes(state);
        let request = Request::builder()
            .uri("/v1/cookbooks?limit=1")
            .body(AxumBody::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let body = axum::body::to_bytes(response.into_body(), 65536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["data"].as_array().unwrap().len(), 1);
        assert_eq!(json["pagination"]["has_more"], true);
        assert!(json["pagination"]["next_cursor"].is_string());
    }

    #[tokio::test]
    async fn test_m2_11_cookbook_response_has_api_version_no_provenance() {
        // Direct data (no rollup) should have api_version but no provenance
        let (state, _) = make_cookbook_app(1);
        let app = cookbook_routes(state);
        let request = Request::builder()
            .uri("/v1/cookbooks")
            .body(AxumBody::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let body = axum::body::to_bytes(response.into_body(), 65536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["api_version"], "v1");
        assert!(json.get("provenance").is_none(), "provenance should be absent for direct data");
        assert!(json.get("stripped_attributes").is_none(), "stripped_attributes should be absent for direct data");
    }
}
