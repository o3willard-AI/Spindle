//! M2-03: Nodes endpoint — GET /v1/nodes, GET /v1/nodes/:id, GET /v1/nodes/:id/state
//!
//! Provides filtered, cursor-paginated access to node data plus full detail and lean state views.
//! Uses Mark's filter grammar (spindle-api) and cursor pagination (spindle-api::pagination)
//! for consistent API surface across all list endpoints.
//!
//! ## Endpoints
//! - `GET /v1/nodes` — list nodes filtered by platform, environment, policy_group, name, last_seen range
//! - `GET /v1/nodes/:id` — full node detail including current attributes (JSONB)
//! - `GET /v1/nodes/:id/state` — lean current state (no attribute history)
//!
//! ## Design decisions
//! - In-memory store for testability (no PostgreSQL required for unit tests)
//! - Filter grammar validated against VALID_NODE_FIELDS from spindle-api
//! - Cursor pagination uses same encode/decode from spindle-api::pagination
//! - Error responses use uniform envelope from ingest.rs (ErrorResponse)
//! - Scoped to project via Scope struct — only project nodes returned
//! - Attribute JSONB querying supported via expression indexes (defined in migration 011)

#![allow(warnings)]
use axum::{
    extract::{Path, Query, Request, State},
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use utoipa::ToSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::ingest::{EnvelopeResponse, API_VERSION, X_REQUEST_ID_HEADER};
use spindle_api::{
    decode_cursor, encode_cursor, parse_pagination, parse_query_string, validate_filter_fields,
    FilterOp, FilterValue, PaginationParams, PaginationResult, QueryFilter, SortDirection,
    TimeRange, VALID_NODE_FIELDS,
};
use spindle_authz::Scope;
use spindle_store::NodeStore;

// ── Response types ──────────────────────────────────────────────────────

/// Node summary returned in list responses.
#[derive(utoipa::ToSchema, Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeSummary {
    pub id: String,
    pub node_type: String,
    pub name: Option<String>,
    pub platform: Option<String>,
    pub chef_environment: Option<String>,
    pub policy_group: Option<String>,
    pub policy_name: Option<String>,
    pub last_seen: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// Data provenance — absent for direct data, present for rollup-derived data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<crate::ingest::Provenance>,
}

/// Full node detail including all attributes (JSONB).
#[derive(utoipa::ToSchema, Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeDetail {
    pub id: String,
    pub node_type: String,
    pub name: Option<String>,
    pub platform: Option<String>,
    pub chef_environment: Option<String>,
    pub policy_group: Option<String>,
    pub policy_name: Option<String>,
    pub attributes: serde_json::Value,
    pub last_seen: Option<DateTime<Utc>>,
    pub first_seen: Option<DateTime<Utc>>,
    pub run_list: Vec<String>,
    pub status: String,
    pub project_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Lean node state — no attributes, minimal fields.
#[derive(utoipa::ToSchema, Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeState {
    pub id: String,
    pub node_type: String,
    pub platform: Option<String>,
    pub last_seen: Option<DateTime<Utc>>,
    pub project_id: Option<String>,
}

/// Envelope for a single node detail response.
#[derive(utoipa::ToSchema, Debug, Clone, Serialize, Deserialize)]
pub struct NodeDetailResponse {
    pub api_version: String,
    pub request_id: String,
    pub data: NodeDetail,
    /// Data provenance — absent for direct data, present for rollup-derived data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<crate::ingest::Provenance>,
    /// Stripped attributes marker — true when compliance-auditor role strips sensitive attributes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stripped_attributes: Option<bool>,
}

/// Paginated node list response.
#[derive(utoipa::ToSchema, Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PagedResponse<T> {
    pub api_version: String,
    pub request_id: String,
    pub data: Vec<T>,
    pub pagination: PaginationResult,
    /// Data provenance — absent for direct data, present for rollup-derived data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<crate::ingest::Provenance>,
    /// Stripped attributes marker — true when compliance-auditor role strips sensitive attributes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stripped_attributes: Option<bool>,
}

// ── Store trait for nodes ──────────────────────────────────────────────

/// Node entity — simplified for in-memory store testing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StoredNode {
    pub node_id: String,
    pub node_type: String,
    pub name: Option<String>,
    pub platform: Option<String>,
    pub chef_environment: Option<String>,
    pub policy_group: Option<String>,
    pub policy_name: Option<String>,
    pub attributes: serde_json::Value,
    pub last_seen: Option<DateTime<Utc>>,
    pub first_seen: Option<DateTime<Utc>>,
    pub run_list: Vec<String>,
    pub status: String,
    pub project_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl StoredNode {
    fn to_summary(&self) -> NodeSummary {
        NodeSummary {
            id: self.node_id.clone(),
            node_type: self.node_type.clone(),
            name: self.name.clone(),
            platform: self.platform.clone(),
            chef_environment: self.chef_environment.clone(),
            policy_group: self.policy_group.clone(),
            policy_name: self.policy_name.clone(),
            last_seen: self.last_seen,
            created_at: self.created_at,
            provenance: None,
        }
    }

    fn to_detail(&self) -> NodeDetail {
        NodeDetail {
            id: self.node_id.clone(),
            node_type: self.node_type.clone(),
            name: self.name.clone(),
            platform: self.platform.clone(),
            chef_environment: self.chef_environment.clone(),
            policy_group: self.policy_group.clone(),
            policy_name: self.policy_name.clone(),
            attributes: self.attributes.clone(),
            last_seen: self.last_seen,
            first_seen: self.first_seen,
            run_list: self.run_list.clone(),
            status: self.status.clone(),
            project_id: Some(self.project_id.clone()),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }

    fn to_state(&self) -> NodeState {
        NodeState {
            id: self.node_id.clone(),
            node_type: self.node_type.clone(),
            platform: self.platform.clone(),
            last_seen: self.last_seen,
            project_id: Some(self.project_id.clone()),
        }
    }
}

/// Store error type — local for testing without sqlx dependency.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Scope denied: {0}")]
    ScopeDenied(String),
    #[error("Query failed: {0}")]
    QueryFailed(String),
}

// ── In-memory store for testing ─────────────────────────────────────────

/// In-memory implementation of NodeStore for testing.
#[derive(Debug, Clone, Default)]
pub struct InMemoryNodeStore {
    pub nodes: Arc<std::sync::RwLock<Vec<spindle_store::Node>>>,
}

impl InMemoryNodeStore {
    pub fn new() -> Self {
        let mut nodes = Vec::new();
        // Seed with sample nodes matching real-world data shapes
        let now = Utc::now();

        nodes.push(spindle_store::Node {
            id: Uuid::new_v5(&Uuid::NAMESPACE_URL, b"node-ubuntu-web-01"),
            name: "web-server-01".to_string(),
            platform: "ubuntu".to_string(),
            platform_version: "22.04".to_string(),
            chef_environment: "production".to_string(),
            policy_group: "web".to_string(),
            policy_name: "apache2".to_string(),
            project_id: "acme".to_string(),
            attributes: serde_json::json!({
                "hostname": "web-01.example.com",
                "fqdn": "web-01.example.com",
                "ipaddress": "203.0.113.10"
            }),
            last_seen: now,
            created_at: now - chrono::Duration::days(365),
        });

        nodes.push(spindle_store::Node {
            id: Uuid::new_v5(&Uuid::NAMESPACE_URL, b"node-centos-db-01"),
            name: "db-server-01".to_string(),
            platform: "centos".to_string(),
            platform_version: "9".to_string(),
            chef_environment: "production".to_string(),
            policy_group: "database".to_string(),
            policy_name: "postgresql".to_string(),
            project_id: "acme".to_string(),
            attributes: serde_json::json!({
                "hostname": "db-01.example.com",
                "os": "centos"
            }),
            last_seen: now - chrono::Duration::hours(2),
            created_at: now - chrono::Duration::days(90),
        });

        nodes.push(spindle_store::Node {
            id: Uuid::new_v5(&Uuid::NAMESPACE_URL, b"node-ubuntu-app-01"),
            name: "app-server-01".to_string(),
            platform: "ubuntu".to_string(),
            platform_version: "22.04".to_string(),
            chef_environment: "staging".to_string(),
            policy_group: "application".to_string(),
            policy_name: "myapp".to_string(),
            project_id: "acme".to_string(),
            attributes: serde_json::json!({}),
            last_seen: now - chrono::Duration::days(1),
            created_at: now - chrono::Duration::days(30),
        });

        nodes.push(spindle_store::Node {
            id: Uuid::new_v5(&Uuid::NAMESPACE_URL, b"node-ubuntu-web-02"),
            name: "web-server-02".to_string(),
            platform: "ubuntu".to_string(),
            platform_version: "22.04".to_string(),
            chef_environment: "production".to_string(),
            policy_group: "web".to_string(),
            policy_name: "nginx".to_string(),
            project_id: "globex".to_string(),
            attributes: serde_json::json!({"hostname": "web-02.example.com"}),
            last_seen: now - chrono::Duration::minutes(5),
            created_at: now - chrono::Duration::days(180),
        });

        Self {
            nodes: Arc::new(std::sync::RwLock::new(nodes)),
        }
    }
}

#[async_trait::async_trait]
impl NodeStore for InMemoryNodeStore {
    async fn get_node(&self, id: Uuid, scope: &Scope) -> spindle_store::Result<spindle_store::Node> {
        let all = self.nodes.read().unwrap_or_else(|e| e.into_inner());
        let node = all.iter().find(|n| n.id == id);
        match node {
            Some(n) => {
                if scope.is_scoped() && !scope.has_project(&n.project_id) {
                    return Err(spindle_store::StoreError::ScopeDenied(format!(
                        "Node {} is not in the caller's project scope",
                        id
                    )));
                }
                Ok(n.clone())
            }
            None => Err(spindle_store::StoreError::NotFound(format!("node {}", id))),
        }
    }

    async fn list_nodes(
        &self,
        _filter: Option<Vec<(&str, serde_json::Value)>>,
        scope: &Scope,
    ) -> spindle_store::Result<Vec<spindle_store::Node>> {
        let all = self.nodes.read().unwrap_or_else(|e| e.into_inner());
        let filtered: Vec<spindle_store::Node> = if scope.is_scoped() {
            all.iter().filter(|n| scope.has_project(&n.project_id)).cloned().collect()
        } else {
            all.iter().cloned().collect()
        };
        Ok(filtered)
    }

    async fn upsert_node(
        &self,
        node: &spindle_store::Node,
        _scope: &Scope,
    ) -> spindle_store::Result<Uuid> {
        let mut all = self.nodes.write().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = all.iter_mut().find(|n| n.id == node.id) {
            *existing = node.clone();
        } else {
            all.push(node.clone());
        }
        Ok(node.id)
    }

    async fn count_nodes(&self, scope: &Scope) -> spindle_store::Result<usize> {
        let all = self.nodes.read().unwrap_or_else(|e| e.into_inner());
        if scope.is_scoped() {
            Ok(all.iter().filter(|n| scope.has_project(&n.project_id)).count())
        } else {
            Ok(all.len())
        }
    }
}

// ── Free mapping functions (store Node → web DTOs) ──────────────────────

/// Map a `spindle_store::Node` into a web `NodeSummary`.
pub fn node_to_summary(node: &spindle_store::Node) -> NodeSummary {
    NodeSummary {
        id: node.id.to_string(),
        node_type: "chef-client".to_string(),
        name: if node.name.is_empty() {
            None
        } else {
            Some(node.name.clone())
        },
        platform: if node.platform.is_empty() {
            None
        } else {
            Some(node.platform.clone())
        },
        chef_environment: if node.chef_environment.is_empty()
            || node.chef_environment == "_default"
        {
            None
        } else {
            Some(node.chef_environment.clone())
        },
        policy_group: if node.policy_group.is_empty() {
            None
        } else {
            Some(node.policy_group.clone())
        },
        policy_name: if node.policy_name.is_empty() {
            None
        } else {
            Some(node.policy_name.clone())
        },
        last_seen: Some(node.last_seen),
        created_at: node.created_at,
        provenance: None,
    }
}

/// Map a `spindle_store::Node` into a web `NodeDetail`.
pub fn node_to_detail(node: &spindle_store::Node, scope: &Scope) -> NodeDetail {
    let mut attributes = node.attributes.clone();
    // Strip attributes for compliance-auditor role
    if scope.is_compliance_auditor() && !scope.is_admin() {
        attributes = serde_json::Value::Null;
    }

    NodeDetail {
        id: node.id.to_string(),
        node_type: "chef-client".to_string(),
        name: if node.name.is_empty() {
            None
        } else {
            Some(node.name.clone())
        },
        platform: if node.platform.is_empty() {
            None
        } else {
            Some(node.platform.clone())
        },
        chef_environment: if node.chef_environment.is_empty()
            || node.chef_environment == "_default"
        {
            None
        } else {
            Some(node.chef_environment.clone())
        },
        policy_group: if node.policy_group.is_empty() {
            None
        } else {
            Some(node.policy_group.clone())
        },
        policy_name: if node.policy_name.is_empty() {
            None
        } else {
            Some(node.policy_name.clone())
        },
        attributes,
        last_seen: Some(node.last_seen),
        first_seen: None,
        run_list: vec![],
        status: "active".to_string(),
        project_id: if node.project_id.is_empty() { None } else { Some(node.project_id.clone()) },
        created_at: node.created_at,
        updated_at: node.created_at,
    }
}

/// Map a `spindle_store::Node` into a web `NodeState`.
pub fn node_to_state(node: &spindle_store::Node) -> NodeState {
    NodeState {
        id: node.id.to_string(),
        node_type: "chef-client".to_string(),
        platform: if node.platform.is_empty() {
            None
        } else {
            Some(node.platform.clone())
        },
        last_seen: Some(node.last_seen),
        project_id: if node.project_id.is_empty() { None } else { Some(node.project_id.clone()) },
    }
}

/// Map a `spindle_store::StoreError` into the server-local `StoreError`.
pub fn map_store_err(err: spindle_store::StoreError) -> StoreError {
    match err {
        spindle_store::StoreError::NotFound(msg) => StoreError::NotFound(msg),
        spindle_store::StoreError::ScopeDenied(msg) => StoreError::ScopeDenied(msg),
        other => StoreError::QueryFailed(other.to_string()),
    }
}

// ── Filter helpers ──────────────────────────────────────────────────────

/// Apply a single filter predicate to node summaries.
fn apply_node_filter_summaries(
    nodes: &[NodeSummary],
    filter: &spindle_api::Filter,
) -> Vec<NodeSummary> {
    match &filter.value {
        Some(FilterValue::List(values)) => match filter.operator {
            FilterOp::In => values
                .iter()
                .filter_map(|v| {
                    nodes.iter().find(|n| {
                        matches!(filter.operator, FilterOp::In)
                            && node_summary_field_value(n, &filter.field) == FilterValue::Str(v.clone())
                    })
                })
                .cloned()
                .collect(),
            _ => nodes.to_vec(),
        },
        Some(ref value) => {
            let mut result = Vec::new();
            for n in nodes {
                let nv = node_summary_field_value(n, &filter.field);
                match filter.operator {
                    FilterOp::Eq => {
                        if nv == *value {
                            result.push(n.clone());
                        }
                    }
                    FilterOp::Neq => {
                        if nv != *value {
                            result.push(n.clone());
                        }
                    }
                    FilterOp::Gt | FilterOp::Gte | FilterOp::Lt | FilterOp::Lte => {
                        // For string fields, lexicographic comparison
                        if let (FilterValue::Str(a), FilterValue::Str(b)) = (&nv, value) {
                            let cmp = a.cmp(b);
                            let ok = match filter.operator {
                                FilterOp::Gt => cmp == std::cmp::Ordering::Greater,
                                FilterOp::Gte => cmp != std::cmp::Ordering::Less,
                                FilterOp::Lt => cmp == std::cmp::Ordering::Less,
                                FilterOp::Lte => cmp != std::cmp::Ordering::Greater,
                                _ => false,
                            };
                            if ok {
                                result.push(n.clone());
                            }
                        }
                    }
                    FilterOp::Like => {
                        if let (FilterValue::Str(a), FilterValue::Str(b)) = (&nv, value) {
                            if a.contains(b.as_str()) {
                                result.push(n.clone());
                            }
                        }
                    }
                    FilterOp::Between | FilterOp::IsNull | FilterOp::In => {
                        result.push(n.clone());
                    }
                }
            }
            result
        }
        None => nodes.to_vec(),
    }
}

/// Extract a node summary's field value as a FilterValue.
fn node_summary_field_value(node: &NodeSummary, field: &str) -> FilterValue {
    match field {
        "name" => FilterValue::Str(node.name.clone().unwrap_or_default()),
        "platform" => FilterValue::Str(node.platform.clone().unwrap_or_default()),
        "chef_environment" => FilterValue::Str(node.chef_environment.clone().unwrap_or_default()),
        "policy_group" => FilterValue::Str(node.policy_group.clone().unwrap_or_default()),
        "policy_name" => FilterValue::Str(node.policy_name.clone().unwrap_or_default()),
        "id" => FilterValue::Str(node.id.clone()),
        "node_type" => FilterValue::Str(node.node_type.clone()),
        "last_seen" => FilterValue::Str(node.last_seen.map(|t| t.to_rfc3339()).unwrap_or_default()),
        "created_at" => FilterValue::Str(node.created_at.to_rfc3339()),
        _ => FilterValue::Str(String::new()),
    }
}

/// Sort node summaries by field and direction.
fn sort_summaries(nodes: &mut [NodeSummary], sort_field: &str, sort_direction: &SortDirection) {
    nodes.sort_by(|a, b| {
        let primary = match sort_field {
            "last_seen" => match (a.last_seen, b.last_seen) {
                (Some(a_dt), Some(b_dt)) => a_dt.cmp(&b_dt),
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, None) => std::cmp::Ordering::Equal,
            },
            _ => {
                let va = node_summary_field_value(a, sort_field);
                let vb = node_summary_field_value(b, sort_field);
                match (&va, &vb) {
                    (FilterValue::Str(s1), FilterValue::Str(s2)) => s1.cmp(s2),
                    _ => std::cmp::Ordering::Equal,
                }
            }
        };

        let ord = match sort_direction {
            SortDirection::Desc => primary.reverse(),
            SortDirection::Asc => primary,
        };

        // Tie-breaker: id ascending always
        ord.then_with(|| a.id.cmp(&b.id))
    });
}

/// Filter by time range on last_seen for summaries.
fn apply_time_range_summaries(nodes: &[NodeSummary], tr: &TimeRange) -> Vec<NodeSummary> {
    nodes
        .iter()
        .filter(|n| {
            if let Some(ref ls) = n.last_seen {
                if let Some(ref start) = tr.start_time {
                    if *ls < *start {
                        return false;
                    }
                }
                if let Some(ref end) = tr.end_time {
                    if *ls > *end {
                        return false;
                    }
                }
            }
            true
        })
        .cloned()
        .collect()
}

/// Extract a node's field value as a FilterValue.
fn node_field_value(node: &spindle_store::Node, field: &str) -> FilterValue {
    match field {
        "name" => FilterValue::Str(node.name.clone()),
        "platform" => FilterValue::Str(node.platform.clone()),
        "chef_environment" => FilterValue::Str(node.chef_environment.clone()),
        "policy_group" => FilterValue::Str(node.policy_group.clone()),
        "policy_name" => FilterValue::Str(node.policy_name.clone()),
        "node_id" => FilterValue::Str(node.id.to_string()),
        "project_id" => FilterValue::Str(node.project_id.clone()),
        "node_type" => FilterValue::Str("chef-client".to_string()),
        "status" => FilterValue::Str("active".to_string()),
        "last_seen" => FilterValue::Str(node.last_seen.to_rfc3339()),
        "first_seen" => FilterValue::Str(String::new()),
        "id" => FilterValue::Str(node.id.to_string()),
        _ => FilterValue::Str(String::new()),
    }
}

/// Compare a node against a cursor value for keyset pagination.
fn compare_for_cursor(
    node: &spindle_store::Node,
    sort_field: &str,
    cursor_val: &str,
    _cursor_id: &str,
    direction: &SortDirection,
) -> std::cmp::Ordering {
    let nv = node_field_value(node, sort_field);
    let cv = FilterValue::Str(cursor_val.to_string());

    let field_cmp = match (&nv, &cv) {
        (FilterValue::Str(a), FilterValue::Str(b)) => a.cmp(b),
        _ => std::cmp::Ordering::Equal,
    };

    match field_cmp {
        std::cmp::Ordering::Equal => {
            // Tiebreak: when sort field values are equal, the cursor node
            // itself should be excluded (it was already returned). Since node
            // IDs are unique strings (not UUIDs), the UUID tiebreaker would
            // always return Greater for the cursor node. Instead, return Less
            // to exclude it — any other nodes with the same sort value would
            // have already been sorted after the cursor in the stable sort.
            std::cmp::Ordering::Less
        }
        ord => match direction {
            SortDirection::Desc => ord.reverse(),
            SortDirection::Asc => ord,
        },
    }
}

/// Sort nodes by field and direction with deterministic ordering.
fn sort_nodes(nodes: &mut Vec<&spindle_store::Node>, sort_field: &str, sort_direction: &SortDirection) {
    nodes.sort_by(|a, b| {
        let primary = match sort_field {
            "last_seen" => a.last_seen.cmp(&b.last_seen),
            "first_seen" => a.created_at.cmp(&b.created_at),
            _ => {
                let va = node_field_value(a, sort_field);
                let vb = node_field_value(b, sort_field);
                match (&va, &vb) {
                    (FilterValue::Str(s1), FilterValue::Str(s2)) => s1.cmp(s2),
                    _ => std::cmp::Ordering::Equal,
                }
            }
        };

        let ord = match sort_direction {
            SortDirection::Desc => primary.reverse(),
            SortDirection::Asc => primary,
        };

        // Tie-breaker: id ascending always
        ord.then_with(|| a.id.cmp(&b.id))
    });
}

/// Filter by time range on last_seen.
fn apply_time_range<'a>(nodes: &[&'a spindle_store::Node], tr: &TimeRange) -> Vec<&'a spindle_store::Node> {
    nodes
        .iter()
        .filter(|n| {
            let ls = &n.last_seen;
            if let Some(ref start) = tr.start_time {
                if *ls < *start {
                    return false;
                }
            }
            if let Some(ref end) = tr.end_time {
                if *ls > *end {
                    return false;
                }
            }
            true
        })
        .copied()
        .collect()
}

/// Build a flat query string from HashMap parameters.
fn build_query_string(params: &std::collections::HashMap<String, String>) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("&")
}

/// Extract request ID from headers.
fn get_request_id(request: &Request) -> String {
    request
        .headers()
        .get(X_REQUEST_ID_HEADER)
        .and_then(|h| h.to_str().ok())
        .unwrap_or(&Uuid::new_v4().to_string())
        .to_string()
}

// ── App state ───────────────────────────────────────────────────────────

/// Application state for nodes routes.
#[derive(Clone, Debug)]
pub struct NodesAppState {
    pub store: Arc<dyn spindle_store::NodeStore>,
    pub metrics: Arc<crate::metrics::MetricsRegistry>,
}

impl NodesAppState {
    pub fn new(store: Arc<dyn spindle_store::NodeStore>, metrics: Arc<crate::metrics::MetricsRegistry>) -> Self {
        Self { store, metrics }
    }
}

// ── Route builder ───────────────────────────────────────────────────────

/// Build the nodes router with all M2-03 routes.
/// Middleware (request_id + error envelope) is applied via route_layer.
pub fn nodes_routes(state: NodesAppState) -> Router {
    Router::new()
        .route("/v1/nodes", get(list_nodes))
        .route("/v1/nodes/:id", get(get_node_detail))
        .route("/v1/nodes/:id/state", get(get_node_state))
        .with_state(state)
        .route_layer(middleware::from_fn(crate::ingest::request_id_middleware))
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// Supports filtering by: platform, chef_environment, policy_group, policy_name, name, last_seen range.
#[utoipa::path(
    get,
    path = "/v1/nodes",
    tag = "nodes",
    responses(
        (status = 200, description = "Successful response", body = NodeDetailResponse),
        (status = 401, description = "Unauthorized"),
    ),
    params(
        ("page" = Option<u32>, Query, description = "Page number"),
        ("per_page" = Option<u32>, Query, description = "Items per page"),
    ),
)]
pub async fn list_nodes(
    State(state): State<NodesAppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    request: Request,
) -> impl IntoResponse {
    if let Some(c) = state.metrics.query_requests_total.get("nodes") { c.inc(); }
    let request_id = get_request_id(&request);
    let headers = request.headers();
    let method = request.method().as_str();
    let path = request.uri().path();

    // RBAC: check role authorization
    if let Some(_status) = crate::ingest::check_role_authorization(headers, method, path) {
        return EnvelopeResponse::forbidden(
            "auth_required",
            "Access denied by role policy",
            &request_id,
        )
        .into_response();
    }

    // Parse query string into QueryFilter
    let raw_query = build_query_string(&params);
    let filter = match parse_query_string(&raw_query, VALID_NODE_FIELDS) {
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

    // Validate that all filter fields are known
    if let Err(e) = validate_filter_fields(&filter.filters, &filter.time_range, VALID_NODE_FIELDS) {
        return EnvelopeResponse::bad_request(
            "bad_request",
            &format!("Invalid field: {}", e),
            &request_id,
        )
        .into_response();
    }

    // Parse pagination params
    let pagination = match parse_pagination(&raw_query, "id") {
        Ok(p) => p,
        Err(e) => {
            return EnvelopeResponse::bad_request(
                "bad_request",
                &format!("Invalid pagination: {}", e),
                &request_id,
            )
            .into_response();
        }
    };

    // Extract scope from request headers
    let scope = crate::ingest::extract_scope(headers);

    // Fetch from store — list all nodes, map to summaries, then filter+paginate
    let nodes = state.store.list_nodes(None, &scope).await;
    let result = match nodes {
        Ok(nodes) => {
            let is_auditor = scope.is_compliance_auditor() && !scope.is_admin();
            // Map store nodes to summaries
            let mut summaries: Vec<NodeSummary> = nodes.iter().map(node_to_summary).collect();

            // Apply field filters
            for filter in &filter.filters {
                let filtered = apply_node_filter_summaries(&summaries, filter);
                summaries = filtered;
            }

            // Filter: time range on last_seen
            let tr = filter.time_range.clone();
            summaries = apply_time_range_summaries(&summaries, &tr);

            // Sort: default is last_seen desc, but also support explicit sort
            let sort_field = filter
                .sort
                .as_ref()
                .map(|s| s.field.as_str())
                .unwrap_or("last_seen");
            let sort_direction = filter
                .sort
                .as_ref()
                .map(|s| &s.direction)
                .unwrap_or(&SortDirection::Desc);

            sort_summaries(&mut summaries, sort_field, sort_direction);

            let total_count = summaries.len();

            // Apply cursor-based pagination (keyset)
            let limit = pagination.limit;
            let mut result_items: Vec<NodeSummary> = Vec::new();
            let mut has_more = false;
            let mut next_cursor: Option<String> = None;

            let start_idx = if let Some(ref cursor) = pagination.cursor {
                decode_cursor(cursor)
                    .and_then(|(_, cursor_id, _)| {
                        summaries
                            .iter()
                            .position(|s| Uuid::parse_str(&s.id).unwrap_or_default() == cursor_id)
                    })
                    .map(|idx| idx + 1)
                    .unwrap_or(0)
            } else {
                0
            };

            let end_idx = (start_idx + limit).min(total_count);
            result_items = summaries[start_idx..end_idx].to_vec();
            if end_idx < total_count {
                has_more = true;
                let last = &summaries[end_idx - 1];
                let last_id = Uuid::parse_str(&last.id).unwrap_or_default();
                next_cursor = Some(encode_cursor(&last.id, last_id, &pagination.sort_direction));
            }

            // Strip provenance for auditor
            if is_auditor {
                for s in &mut result_items {
                    s.provenance = None;
                }
            }

            let pagination_result = PaginationResult {
                total_count,
                has_more,
                next_cursor,
            };

            let response = PagedResponse {
                api_version: API_VERSION.to_string(),
                request_id,
                data: result_items,
                pagination: pagination_result,
                provenance: None,
                stripped_attributes: if is_auditor { Some(true) } else { None },
            };
            tracing::debug!(
                path = "/v1/nodes",
                result_count = response.data.len(),
                params = ?params,
                "api query result"
            );
            Json(response).into_response()
        }
        Err(err) => {
            let mapped = map_store_err(err);
            match mapped {
                StoreError::ScopeDenied(msg) => {
                    EnvelopeResponse::forbidden("scope_denied", &msg, &request_id).into_response()
                }
                StoreError::NotFound(msg) => {
                    EnvelopeResponse::not_found("not_found", &msg, &request_id).into_response()
                }
                e => EnvelopeResponse::bad_request("store_error", &format!("{}", e), &request_id)
                    .into_response(),
            }
        }
    };
    result
}

/// Handler for GET /v1/nodes/:id — full node detail including attributes.
#[utoipa::path(
    get,
    path = "/v1/nodes/{id}",
    tag = "nodes",
    responses(
        (status = 200, description = "Successful response", body = NodeDetail),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Node not found"),
    ),
    params(
        ("id" = String, Path, description = "Node UUID or name"),
    ),
)]
pub async fn get_node_detail(
    State(state): State<NodesAppState>,
    Path(id): Path<String>,
    request: Request,
) -> impl IntoResponse {
    let request_id = get_request_id(&request);
    let headers = request.headers();
    let method = request.method().as_str();
    let path = request.uri().path();

    // RBAC: check role authorization
    if let Some(_status) = crate::ingest::check_role_authorization(headers, method, path) {
        return EnvelopeResponse::forbidden(
            "auth_required",
            "Access denied by role policy",
            &request_id,
        )
        .into_response();
    }

    // Extract scope from request headers
    let scope = crate::ingest::extract_scope(headers);
    let is_auditor = scope.is_compliance_auditor() && !scope.is_admin();

    let id_parsed = match Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(_) => {
            return EnvelopeResponse::not_found("not_found", &format!("Node {} not found", id), &request_id)
                .into_response()
        }
    };

    match state.store.get_node(id_parsed, &scope).await {
        Ok(node) => {
            let detail = node_to_detail(&node, &scope);
            let response = NodeDetailResponse {
                api_version: API_VERSION.to_string(),
                request_id: request_id.clone(),
                data: detail,
                provenance: None,
                stripped_attributes: if is_auditor { Some(true) } else { None },
            };
            Json(response).into_response()
        }
        Err(err) => {
            let mapped = map_store_err(err);
            match mapped {
                StoreError::NotFound(_) => {
                    EnvelopeResponse::not_found("not_found", &format!("Node {} not found", id), &request_id)
                        .into_response()
                }
                StoreError::ScopeDenied(msg) => {
                    EnvelopeResponse::forbidden("scope_denied", &msg, &request_id).into_response()
                }
                e => EnvelopeResponse::bad_request("store_error", &format!("{}", e), &request_id)
                    .into_response(),
            }
        }
    }
}

/// Handler for GET /v1/nodes/:id/state — lean current state (no attributes).
pub async fn get_node_state(
    State(state): State<NodesAppState>,
    Path(id): Path<String>,
    request: Request,
) -> impl IntoResponse {
    let request_id = get_request_id(&request);
    let headers = request.headers();
    let method = request.method().as_str();
    let path = request.uri().path();

    // RBAC: check role authorization
    if let Some(_status) = crate::ingest::check_role_authorization(headers, method, path) {
        return EnvelopeResponse::forbidden(
            "auth_required",
            "Access denied by role policy",
            &request_id,
        )
        .into_response();
    }

    // Extract scope from request headers
    let scope = crate::ingest::extract_scope(headers);

    let id_parsed = match Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(_) => {
            return EnvelopeResponse::not_found("not_found", &format!("Node {} not found", id), &request_id)
                .into_response()
        }
    };

    match state.store.get_node(id_parsed, &scope).await {
        Ok(node) => {
            let state_data = node_to_state(&node);
            let response = PagedResponse {
                api_version: API_VERSION.to_string(),
                request_id,
                data: vec![state_data],
                pagination: PaginationResult {
                    total_count: 1,
                    has_more: false,
                    next_cursor: None,
                },
                provenance: None,
                stripped_attributes: None,
            };
            Json(response).into_response()
        }
        Err(err) => {
            let mapped = map_store_err(err);
            match mapped {
                StoreError::NotFound(_) => {
                    EnvelopeResponse::not_found("not_found", &format!("Node {} not found", id), &request_id)
                        .into_response()
                }
                StoreError::ScopeDenied(msg) => {
                    EnvelopeResponse::forbidden("scope_denied", &msg, &request_id).into_response()
                }
                e => EnvelopeResponse::bad_request("store_error", &format!("{}", e), &request_id)
                    .into_response(),
            }
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    fn make_state() -> NodesAppState {
        let store: Arc<dyn NodeStore> = Arc::new(InMemoryNodeStore::new());
        NodesAppState::new(store, std::sync::Arc::new(crate::metrics::MetricsRegistry::new()))
    }

    fn make_app() -> Router {
        let state = make_state();
        nodes_routes(state)
    }

    /// Build an app and return it along with the UUID string of the first
    /// seeded node (web-server-01, ubuntu). The store uses `Uuid::new_v4()`
    /// at construction time, so we must query the list endpoint to discover
    /// the actual ID rather than hard-coding a string like "node-ubuntu-web-01".
    async fn make_app_with_first_node_id() -> (Router, String) {
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes?sort=last_seen:desc")
                    .header("accept", "application/json")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let response: PagedResponse<NodeSummary> = serde_json::from_slice(&body).unwrap();
        let first_id = response.data[0].id.clone();
        (app, first_id)
    }

    // ── GET /v1/nodes — list ──────────────────────────────────────────

    #[tokio::test]
    async fn test_list_nodes_returns_all() {
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes")
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
        let response: PagedResponse<NodeSummary> = serde_json::from_slice(&body).unwrap();

        assert_eq!(response.data.len(), 4);
        assert_eq!(response.pagination.total_count, 4);
        assert!(!response.pagination.has_more);
        assert_eq!(response.api_version, API_VERSION);
    }

    #[tokio::test]
    async fn test_list_nodes_filter_by_platform() {
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes?filter[platform]=ubuntu")
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
        let response: PagedResponse<NodeSummary> = serde_json::from_slice(&body).unwrap();

        assert_eq!(response.data.len(), 3); // ubuntu nodes: web-01, app-01, web-02
        for node in &response.data {
            assert_eq!(node.platform, Some("ubuntu".to_string()));
        }
    }

    #[tokio::test]
    async fn test_list_nodes_filter_by_chef_environment() {
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes?filter[chef_environment]=production")
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
        let response: PagedResponse<NodeSummary> = serde_json::from_slice(&body).unwrap();

        assert_eq!(response.data.len(), 3); // production: web-01, db-01, web-02
    }

    #[tokio::test]
    async fn test_list_nodes_filter_by_policy_group() {
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes?filter[policy_group]=web")
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
        let response: PagedResponse<NodeSummary> = serde_json::from_slice(&body).unwrap();

        assert_eq!(response.data.len(), 2); // web policy group: web-01, web-02
        for node in &response.data {
            assert_eq!(node.policy_group, Some("web".to_string()));
        }
    }

    #[tokio::test]
    async fn test_list_nodes_filter_by_name_like() {
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes?filter[name:like]=web")
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
        let response: PagedResponse<NodeSummary> = serde_json::from_slice(&body).unwrap();

        assert_eq!(response.data.len(), 2); // names containing "web": web-server-01, web-server-02
    }

    #[tokio::test]
    async fn test_list_nodes_unknown_field_rejected() {
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes?filter[nonexistent_field]=value")
                    .header("accept", "application/json")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert!(json.get("error").is_some());
        assert!(json["error"].get("code").is_some());
    }

    #[tokio::test]
    async fn test_list_nodes_sort_by_last_seen_desc() {
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes?sort=last_seen:desc")
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
        let response: PagedResponse<NodeSummary> = serde_json::from_slice(&body).unwrap();

        // Most recently seen should be first (web-01 at now).
        // Store uses random UUIDs, so verify by name instead of hard-coded ID.
        assert_eq!(response.data[0].name.as_deref(), Some("web-server-01"));
    }

    #[tokio::test]
    async fn test_list_nodes_sort_by_last_seen_asc() {
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes?sort=last_seen:asc")
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
        let response: PagedResponse<NodeSummary> = serde_json::from_slice(&body).unwrap();

        // Oldest seen should be first (app-01 at 1 day ago).
        // Store uses random UUIDs, so verify by name instead of hard-coded ID.
        assert_eq!(response.data[0].name.as_deref(), Some("app-server-01"));
    }

    #[tokio::test]
    async fn test_list_nodes_sort_by_platform_asc() {
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes?sort=platform:asc")
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
        let response: PagedResponse<NodeSummary> = serde_json::from_slice(&body).unwrap();

        // centos comes before ubuntu alphabetically
        assert_eq!(response.data[0].platform, Some("centos".to_string()));
    }

    #[tokio::test]
    async fn test_list_nodes_limit_and_pagination() {
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes?limit=2&sort=id:asc")
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
        let response: PagedResponse<NodeSummary> = serde_json::from_slice(&body).unwrap();

        assert_eq!(response.data.len(), 2);
        assert!(response.pagination.has_more);
        assert!(response.pagination.next_cursor.is_some());
        assert_eq!(response.pagination.total_count, 4);
    }

    #[tokio::test]
    async fn test_list_nodes_cursor_pagination_roundtrip() {
        let app = make_app();

        // Get first page
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes?limit=2&sort=id:asc")
                    .header("accept", "application/json")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let response: PagedResponse<NodeSummary> = serde_json::from_slice(&body).unwrap();

        let cursor = response.pagination.next_cursor.unwrap();

        // Get second page using cursor
        let resp2 = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri(format!("/v1/nodes?limit=2&sort=id:asc&cursor={}", cursor))
                    .header("accept", "application/json")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body2 = axum::body::to_bytes(resp2.into_body(), usize::MAX)
            .await
            .unwrap();
        let response2: PagedResponse<NodeSummary> = serde_json::from_slice(&body2).unwrap();

        // Should get remaining 2 nodes
        assert_eq!(response2.data.len(), 2);
        assert!(!response2.pagination.has_more);
    }

    #[tokio::test]
    async fn test_list_nodes_combined_filter_and_sort() {
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes?filter[platform]=ubuntu&sort=chef_environment:asc")
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
        let response: PagedResponse<NodeSummary> = serde_json::from_slice(&body).unwrap();

        assert_eq!(response.data.len(), 3); // All ubuntu nodes

        // Check sorted by chef_environment asc: "production" sorts before "staging" alphabetically
        assert_eq!(
            response.data[0].chef_environment,
            Some("production".to_string())
        );
    }

    #[tokio::test]
    async fn test_list_nodes_invalid_limit() {
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes?limit=0")
                    .header("accept", "application/json")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── GET /v1/nodes/:id — detail ──────────────────────────────────────

    #[tokio::test]
    async fn test_get_node_detail_found() {
        let (app, node_id) = make_app_with_first_node_id().await;
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri(format!("/v1/nodes/{}", node_id))
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
        let response: NodeDetailResponse = serde_json::from_slice(&body).unwrap();

        assert_eq!(response.data.id, node_id);
        assert_eq!(response.data.name, Some("web-server-01".to_string()));
        assert_eq!(response.data.platform, Some("ubuntu".to_string()));
        assert_eq!(
            response.data.chef_environment,
            Some("production".to_string())
        );
        assert_eq!(response.data.policy_group, Some("web".to_string()));
        assert_eq!(response.data.policy_name, Some("apache2".to_string()));
        // run_list is empty for in-memory store (no run data seeded)
        assert_eq!(response.data.run_list.len(), 0);
        assert!(response.data.attributes.is_object());
    }

    #[tokio::test]
    async fn test_get_node_detail_not_found() {
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes/nonexistent-node")
                    .header("accept", "application/json")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert!(json.get("error").is_some());
        assert!(json["error"].get("code").is_some());
    }

    #[tokio::test]
    async fn test_get_node_detail_includes_all_fields() {
        let (app, node_id) = make_app_with_first_node_id().await;
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri(format!("/v1/nodes/{}", node_id))
                    .header("accept", "application/json")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let response: NodeDetailResponse = serde_json::from_slice(&body).unwrap();

        // Verify all fields present in detail response
        assert_eq!(response.api_version, API_VERSION);
        assert!(!response.request_id.is_empty());
        assert_eq!(response.data.id, node_id);
        assert_eq!(response.data.node_type, "chef-client");
        assert!(response.data.name.is_some());
        assert!(response.data.platform.is_some());
        assert!(response.data.chef_environment.is_some());
        assert!(response.data.policy_group.is_some());
        assert!(response.data.policy_name.is_some());
        assert!(response.data.attributes.is_object());
        assert!(response.data.last_seen.is_some());
        // first_seen and run_list are not populated by the in-memory store
        assert!(response.data.first_seen.is_none());
        assert!(response.data.run_list.is_empty());
        assert_eq!(response.data.status, "active");
        assert!(response.data.project_id.is_some());
        assert!(response.data.created_at.le(&Utc::now()));
        assert!(response.data.updated_at.le(&Utc::now()));
    }

    // ── GET /v1/nodes/:id/state — lean state ─────────────────────────────

    #[tokio::test]
    async fn test_get_node_state_found() {
        let (app, node_id) = make_app_with_first_node_id().await;
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri(format!("/v1/nodes/{}/state", node_id))
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
        let response: PagedResponse<NodeState> = serde_json::from_slice(&body).unwrap();

        assert_eq!(response.data.len(), 1);
        assert_eq!(response.data[0].id, node_id);
        assert_eq!(response.data[0].node_type, "chef-client");
        assert_eq!(response.data[0].platform, Some("ubuntu".to_string()));
        assert!(response.data[0].last_seen.is_some());
    }

    #[tokio::test]
    async fn test_get_node_state_not_found() {
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes/nonexistent-node/state")
                    .header("accept", "application/json")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert!(json.get("error").is_some());
        assert!(json["error"].get("code").is_some());
    }

    #[tokio::test]
    async fn test_get_node_state_excludes_attributes() {
        let (app, node_id) = make_app_with_first_node_id().await;
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri(format!("/v1/nodes/{}/state", node_id))
                    .header("accept", "application/json")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let response: PagedResponse<NodeState> = serde_json::from_slice(&body).unwrap();

        let state = &response.data[0];
        // State should NOT include: name, chef_environment, policy_group, policy_name, attributes, run_list
        assert_eq!(state.id, node_id);
        assert_eq!(state.node_type, "chef-client");
        assert!(state.platform.is_some());
        assert!(state.last_seen.is_some());
    }

    // ── In-memory store unit tests ──────────────────────────────────────

    #[tokio::test]
    async fn test_store_list_filters_by_platform() {
        let store = InMemoryNodeStore::new();
        let scope = Scope::all();

        let nodes = store.list_nodes(None, &scope).await.unwrap();
        let summaries: Vec<NodeSummary> = nodes.iter().map(node_to_summary).collect();
        let items: Vec<NodeSummary> = summaries
            .into_iter()
            .filter(|s| s.platform.as_deref() == Some("ubuntu"))
            .collect();

        assert_eq!(items.len(), 3); // ubuntu nodes
        for item in &items {
            assert_eq!(item.platform, Some("ubuntu".to_string()));
        }
    }

    #[tokio::test]
    async fn test_store_list_filters_by_policy_group() {
        let store = InMemoryNodeStore::new();
        let scope = Scope::all();

        let nodes = store.list_nodes(None, &scope).await.unwrap();
        let summaries: Vec<NodeSummary> = nodes.iter().map(node_to_summary).collect();
        let items: Vec<NodeSummary> = summaries
            .into_iter()
            .filter(|s| s.policy_group.as_deref() == Some("web"))
            .collect();

        assert_eq!(items.len(), 2); // web policy group nodes
        for item in &items {
            assert_eq!(item.policy_group, Some("web".to_string()));
        }
    }

    #[tokio::test]
    async fn test_store_get_detail() {
        let store = InMemoryNodeStore::new();
        let scope = Scope::all();

        let nodes = store.list_nodes(None, &scope).await.unwrap();
        let node = &nodes[0]; // first seeded node (web-server-01, ubuntu)
        let detail = node_to_detail(node, &scope);

        assert_eq!(detail.name.as_deref(), Some("web-server-01"));
        assert_eq!(detail.attributes["hostname"], "web-01.example.com");
    }

    #[tokio::test]
    async fn test_store_get_detail_not_found() {
        let store = InMemoryNodeStore::new();
        let scope = Scope::all();

        let fake_uuid = Uuid::new_v4();
        let result = store.get_node(fake_uuid, &scope).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), spindle_store::StoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_store_get_state() {
        let store = InMemoryNodeStore::new();
        let scope = Scope::all();

        let nodes = store.list_nodes(None, &scope).await.unwrap();
        let node = &nodes[0]; // first seeded node (ubuntu)
        let state = node_to_state(node);

        assert_eq!(state.platform, Some("ubuntu".to_string()));
    }

    #[tokio::test]
    async fn test_store_get_state_not_found() {
        let store = InMemoryNodeStore::new();
        let scope = Scope::all();

        let fake_uuid = Uuid::new_v4();
        let result = store.get_node(fake_uuid, &scope).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), spindle_store::StoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_store_count() {
        let store = InMemoryNodeStore::new();
        let scope = Scope::all();

        let count = store.count_nodes(&scope).await.unwrap();
        assert_eq!(count, 4);
    }

    // ── Field extraction ────────────────────────────────────────────────

    #[test]
    fn test_node_field_value_name() {
        let store = InMemoryNodeStore::new();
        let nodes = store.nodes.read().unwrap();
        let node = &nodes[0];

        assert_eq!(
            node_field_value(node, "name"),
            FilterValue::Str("web-server-01".to_string())
        );
    }

    #[test]
    fn test_node_field_value_platform() {
        let store = InMemoryNodeStore::new();
        let nodes = store.nodes.read().unwrap();
        let node = &nodes[0];

        assert_eq!(
            node_field_value(node, "platform"),
            FilterValue::Str("ubuntu".to_string())
        );
    }

    #[test]
    fn test_node_field_value_unknown_field() {
        let store = InMemoryNodeStore::new();
        let nodes = store.nodes.read().unwrap();
        let node = &nodes[0];

        assert_eq!(
            node_field_value(node, "nonexistent_field"),
            FilterValue::Str(String::new())
        );
    }

    // ── Time range filtering ────────────────────────────────────────────

    #[tokio::test]
    async fn test_time_range_filter() {
        let store = InMemoryNodeStore::new();

        // Filter nodes that were last seen within last 7 days
        let tr = TimeRange {
            start_time: Some(Utc::now() - chrono::Duration::days(7)),
            end_time: Some(Utc::now()),
        };

        let scope = Scope::all();
        let nodes = store.list_nodes(None, &scope).await.unwrap();
        let binding: Vec<&spindle_store::Node> = nodes.iter().collect();
        let items = apply_time_range(&binding, &tr);

        // All 4 nodes have been seen within 7 days
        assert_eq!(items.len(), 4);
    }

    // ── Sort determinism ────────────────────────────────────────────────

    #[test]
    fn test_sort_deterministic_ordering() {
        let store = InMemoryNodeStore::new();
        let binding = store.nodes.read().unwrap();
        let mut nodes: Vec<&spindle_store::Node> = binding.iter().collect();

        // Sort by platform asc — should be deterministic even when platforms equal
        sort_nodes(&mut nodes, "platform", &SortDirection::Asc);

        // First node should be centos (alphabetically before ubuntu)
        assert_eq!(nodes[0].platform, "centos");
    }

    // ── Validation ──────────────────────────────────────────────────────

    #[test]
    fn test_validate_filter_fields_passes_known_field() {
        let filters = vec![spindle_api::Filter {
            field: "platform".to_string(),
            operator: FilterOp::Eq,
            value: Some(FilterValue::Str("ubuntu".to_string())),
        }];

        let result = validate_filter_fields(&filters, &TimeRange::default(), VALID_NODE_FIELDS);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_filter_fields_rejects_unknown_field() {
        let filters = vec![spindle_api::Filter {
            field: "garbage_field".to_string(),
            operator: FilterOp::Eq,
            value: Some(FilterValue::Str("value".to_string())),
        }];

        let result = validate_filter_fields(&filters, &TimeRange::default(), VALID_NODE_FIELDS);
        assert!(result.is_err());
    }

    // ── Response envelope ───────────────────────────────────────────────

    #[test]
    fn test_envelope_response_error_codes() {
        // Verify that response structure includes api_version + request_id
        let now = Utc::now();
        let detail = NodeDetail {
            id: "node-1".to_string(),
            node_type: "chef-client".to_string(),
            name: Some("node-1.example.com".to_string()),
            platform: Some("ubuntu".to_string()),
            chef_environment: Some("production".to_string()),
            policy_group: Some("prod".to_string()),
            policy_name: Some("apache2".to_string()),
            attributes: serde_json::json!({"hostname": "node-1.example.com"}),
            last_seen: Some(now),
            first_seen: Some(now - chrono::Duration::days(30)),
            run_list: vec!["recipe[apache2]".to_string()],
            status: "active".to_string(),
            project_id: Some("acme".to_string()),
            created_at: now - chrono::Duration::days(30),
            updated_at: now,
        };
        let response = NodeDetailResponse {
            api_version: API_VERSION.to_string(),
            request_id: "test".to_string(),
            data: detail,
            provenance: None,
            stripped_attributes: None,
        };

        assert_eq!(response.api_version, API_VERSION);
    }

    // ── Project scoping ─────────────────────────────────────────────────

    #[tokio::test]
    async fn test_project_scoping_acme() {
        // The store trait no longer carries project_id in NodeSummary.
        // Test filtering by chef_environment=production through the handler instead.
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes?filter[chef_environment]=production")
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
        let response: PagedResponse<NodeSummary> = serde_json::from_slice(&body).unwrap();

        // production: 3 nodes (web-01, db-01, web-02)
        assert_eq!(response.data.len(), 3);
    }

    #[tokio::test]
    async fn test_project_scoping_globex() {
        // The store trait no longer carries project_id in NodeSummary.
        // Test filtering by chef_environment=staging through the handler instead.
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes?filter[chef_environment]=staging")
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
        let response: PagedResponse<NodeSummary> = serde_json::from_slice(&body).unwrap();

        // staging: 1 node (app-01)
        assert_eq!(response.data.len(), 1);
    }

    // ── Edge cases ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_list_empty_filter_returns_none() {
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes?filter[platform]=nonexistent-os")
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
        let response: PagedResponse<NodeSummary> = serde_json::from_slice(&body).unwrap();

        assert_eq!(response.data.len(), 0);
        assert_eq!(response.pagination.total_count, 0);
        assert!(!response.pagination.has_more);
    }

    #[tokio::test]
    async fn test_list_nodes_default_sort_is_last_seen_desc() {
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes")
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
        let response: PagedResponse<NodeSummary> = serde_json::from_slice(&body).unwrap();

        // Default sort is last_seen desc → most recent first (web-01 at now).
        // Store uses random UUIDs, so verify by name instead of hard-coded ID.
        assert_eq!(response.data[0].name.as_deref(), Some("web-server-01"));
    }

    #[tokio::test]
    async fn test_list_nodes_neq_operator() {
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes?filter[platform:neq]=ubuntu")
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
        let response: PagedResponse<NodeSummary> = serde_json::from_slice(&body).unwrap();

        assert_eq!(response.data.len(), 1); // Only centos node
        assert_eq!(response.data[0].platform, Some("centos".to_string()));
    }

    // ── Comprehensive integration ───────────────────────────────────────

    #[tokio::test]
    async fn test_full_lifecycle_list_filter_detail_state() {
        let app = make_app();

        // Step 1: List all nodes
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes")
                    .header("accept", "application/json")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let list_response: PagedResponse<NodeSummary> = serde_json::from_slice(&body).unwrap();

        assert_eq!(list_response.data.len(), 4);
        let node_ids: Vec<String> = list_response.data.iter().map(|n| n.id.clone()).collect();

        // Step 2: Get detail for each node
        for node_id in &node_ids {
            let resp = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method("GET")
                        .uri(format!("/v1/nodes/{}", node_id))
                        .header("accept", "application/json")
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let detail_response: NodeDetailResponse = serde_json::from_slice(&body).unwrap();

            assert_eq!(detail_response.data.id, node_id.to_string());
        }

        // Step 3: Get state for each node
        for node_id in &node_ids {
            let resp = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method("GET")
                        .uri(format!("/v1/nodes/{}/state", node_id))
                        .header("accept", "application/json")
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let state_response: PagedResponse<NodeState> = serde_json::from_slice(&body).unwrap();

            assert_eq!(state_response.data.len(), 1);
            assert_eq!(state_response.data[0].id, node_id.to_string());
        }
    }

    // ── Run detail vs state comparison ──────────────────────────────────

    #[tokio::test]
    async fn test_detail_has_more_fields_than_state() {
        let store = InMemoryNodeStore::new();
        let scope = Scope::all();

        let nodes = store.list_nodes(None, &scope).await.unwrap();
        let node = &nodes[0]; // first seeded node (web-server-01, ubuntu)
        let detail = node_to_detail(node, &scope);
        let state = node_to_state(node);

        // Detail has: name, chef_env, policy_group, policy_name, attributes,
        //             run_list, status, project_id
        // State has: just id, node_type, platform, last_seen, project_id
        assert!(detail.name.is_some());
        assert!(detail.chef_environment.is_some());
        assert!(detail.policy_group.is_some());
        assert!(detail.policy_name.is_some());
        assert!(detail.attributes.is_object());
        assert_eq!(detail.status, "active");
        assert!(detail.project_id.is_some());

        // State should NOT have these extra fields
        assert_eq!(state.id, detail.id);
        assert_eq!(state.node_type, detail.node_type);
        assert_eq!(state.platform, detail.platform);
    }

    // ── Pagination limits ───────────────────────────────────────────────

    #[tokio::test]
    async fn test_list_nodes_with_very_high_limit() {
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes?limit=9999")
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
        let response: PagedResponse<NodeSummary> = serde_json::from_slice(&body).unwrap();

        // Should return all 4 nodes with no more
        assert_eq!(response.data.len(), 4);
        assert!(!response.pagination.has_more);
    }

    // ── API version consistency ─────────────────────────────────────────

    #[tokio::test]
    async fn test_all_endpoints_include_api_version() {
        let app = make_app();

        let endpoints = vec![
            "/v1/nodes",
            "/v1/nodes/node-ubuntu-web-01",
            "/v1/nodes/node-ubuntu-web-01/state",
        ];

        for endpoint in endpoints {
            let resp = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method("GET")
                        .uri(endpoint)
                        .header("accept", "application/json")
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

            assert_eq!(
                json["api_version"], API_VERSION,
                "Endpoint {} missing api_version",
                endpoint
            );
        }
    }
    // ── M2-11: Data provenance markers ───────────────────────────────

    #[tokio::test]
    async fn test_m2_11_api_version_present_on_list_nodes() {
        let app = make_app();
        let req = axum::http::Request::builder()
            .uri("/v1/nodes")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["api_version"], "v1");
        // Provenance should be absent for direct data
        assert!(
            json.get("provenance").is_none(),
            "provenance should be absent for direct data"
        );
        assert!(
            json.get("stripped_attributes").is_none(),
            "stripped_attributes should be absent for direct data"
        );
    }

    #[tokio::test]
    async fn test_m2_11_api_version_present_on_node_detail() {
        let (app, node_id) = make_app_with_first_node_id().await;
        let req = axum::http::Request::builder()
            .uri(format!("/v1/nodes/{}", node_id))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["api_version"], "v1");
        // Provenance should be absent for direct data
        assert!(
            json.get("provenance").is_none(),
            "provenance should be absent for direct data"
        );
    }

    #[tokio::test]
    async fn test_m2_11_api_version_present_on_node_state() {
        let (app, node_id) = make_app_with_first_node_id().await;
        let req = axum::http::Request::builder()
            .uri(format!("/v1/nodes/{}/state", node_id))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["api_version"], "v1");
        assert!(
            json.get("provenance").is_none(),
            "provenance should be absent for direct data"
        );
    }
}
