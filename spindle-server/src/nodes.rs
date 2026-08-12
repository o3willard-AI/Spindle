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
use spindle_store::NodeStore as _;

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

/// Store trait for Node queries.
#[async_trait::async_trait]
pub trait NodeStore: Send + Sync + std::fmt::Debug {
    async fn list_nodes_filtered(
        &self,
        filter: &QueryFilter,
        pagination: &PaginationParams,
        scope: &Scope,
    ) -> Result<(Vec<NodeSummary>, PaginationResult), StoreError>;

    async fn get_node_detail(&self, id: &str, scope: &Scope) -> Result<NodeDetail, StoreError>;

    async fn get_node_state(&self, id: &str, scope: &Scope) -> Result<NodeState, StoreError>;

    async fn count_nodes(&self, scope: &Scope) -> Result<usize, StoreError>;
}

// ── In-memory store for testing ─────────────────────────────────────────

/// In-memory implementation of NodeStore for testing.
#[derive(Debug, Clone, Default)]
pub struct InMemoryNodeStore {
    pub nodes: Arc<std::sync::RwLock<Vec<StoredNode>>>,
}

impl InMemoryNodeStore {
    pub fn new() -> Self {
        let mut nodes = Vec::new();
        // Seed with sample nodes matching real-world data shapes
        let now = Utc::now();

        nodes.push(StoredNode {
            node_id: "node-ubuntu-web-01".to_string(),
            node_type: "chef-client".to_string(),
            name: Some("web-server-01".to_string()),
            platform: Some("ubuntu".to_string()),
            chef_environment: Some("production".to_string()),
            policy_group: Some("web".to_string()),
            policy_name: Some("apache2".to_string()),
            attributes: serde_json::json!({
                "hostname": "web-01.example.com",
                "fqdn": "web-01.example.com",
                "ipaddress": "203.0.113.10"
            }),
            last_seen: Some(now),
            first_seen: Some(now - chrono::Duration::days(365)),
            run_list: vec![
                "recipe[apache2]".to_string(),
                "recipe[monitoring]".to_string(),
            ],
            status: "active".to_string(),
            project_id: "acme".to_string(),
            created_at: now - chrono::Duration::days(365),
            updated_at: now,
        });

        nodes.push(StoredNode {
            node_id: "node-centos-db-01".to_string(),
            node_type: "chef-client".to_string(),
            name: Some("db-server-01".to_string()),
            platform: Some("centos".to_string()),
            chef_environment: Some("production".to_string()),
            policy_group: Some("database".to_string()),
            policy_name: Some("postgresql".to_string()),
            attributes: serde_json::json!({
                "hostname": "db-01.example.com",
                "os": "centos"
            }),
            last_seen: Some(now - chrono::Duration::hours(2)),
            first_seen: Some(now - chrono::Duration::days(90)),
            run_list: vec!["recipe[postgresql]".to_string()],
            status: "active".to_string(),
            project_id: "acme".to_string(),
            created_at: now - chrono::Duration::days(90),
            updated_at: now - chrono::Duration::hours(2),
        });

        nodes.push(StoredNode {
            node_id: "node-ubuntu-app-01".to_string(),
            node_type: "chef-client".to_string(),
            name: Some("app-server-01".to_string()),
            platform: Some("ubuntu".to_string()),
            chef_environment: Some("staging".to_string()),
            policy_group: Some("application".to_string()),
            policy_name: Some("myapp".to_string()),
            attributes: serde_json::json!({}),
            last_seen: Some(now - chrono::Duration::days(1)),
            first_seen: Some(now - chrono::Duration::days(30)),
            run_list: vec!["recipe[myapp]".to_string()],
            status: "active".to_string(),
            project_id: "acme".to_string(),
            created_at: now - chrono::Duration::days(30),
            updated_at: now - chrono::Duration::days(1),
        });

        nodes.push(StoredNode {
            node_id: "node-ubuntu-web-02".to_string(),
            node_type: "chef-client".to_string(),
            name: Some("web-server-02".to_string()),
            platform: Some("ubuntu".to_string()),
            chef_environment: Some("production".to_string()),
            policy_group: Some("web".to_string()),
            policy_name: Some("nginx".to_string()),
            attributes: serde_json::json!({"hostname": "web-02.example.com"}),
            last_seen: Some(now - chrono::Duration::minutes(5)),
            first_seen: Some(now - chrono::Duration::days(180)),
            run_list: vec!["recipe[nginx]".to_string()],
            status: "active".to_string(),
            project_id: "globex".to_string(),
            created_at: now - chrono::Duration::days(180),
            updated_at: now - chrono::Duration::minutes(5),
        });

        Self {
            nodes: Arc::new(std::sync::RwLock::new(nodes)),
        }
    }
}

#[async_trait::async_trait]
impl NodeStore for InMemoryNodeStore {
    /// List nodes with filtering, sorting, and cursor pagination.
    /// Respects project scope — only nodes in the caller's projects are returned.
    async fn list_nodes_filtered(
        &self,
        query_filter: &QueryFilter,
        pagination: &PaginationParams,
        scope: &Scope,
    ) -> Result<(Vec<NodeSummary>, PaginationResult), StoreError> {
        let all = self.nodes.read().unwrap().clone();

        // Filter by scope (project access)
        let mut filtered: Vec<&StoredNode> = all.iter().collect();
        if scope.is_scoped() {
            filtered.retain(|n| scope.has_project(&n.project_id));
        }

        // Apply field filters
        for filter in &query_filter.filters {
            filtered = apply_node_filter(&filtered, filter);
        }

        // Filter: time range on last_seen
        let tr = query_filter.time_range.clone();
        filtered = apply_time_range(&filtered, &tr);

        // Sort: default is last_seen desc, but also support explicit sort
        let sort_field = query_filter
            .sort
            .as_ref()
            .map(|s| s.field.as_str())
            .unwrap_or("last_seen");
        let sort_direction = query_filter
            .sort
            .as_ref()
            .map(|s| &s.direction)
            .unwrap_or(&SortDirection::Desc);

        sort_nodes(&mut filtered, sort_field, sort_direction);

        let total_count = filtered.len();

        // Apply cursor-based pagination (keyset)
        let limit = pagination.limit;
        let mut result: Vec<NodeSummary> = Vec::new();
        let mut has_more = false;
        let mut next_cursor: Option<String> = None;

        let items: Vec<&StoredNode> = if let Some(ref cursor) = pagination.cursor {
            match decode_cursor(cursor) {
                Some((cursor_val, cursor_id, _direction)) => {
                    let mut remaining = Vec::new();
                    for node in &filtered {
                        if compare_for_cursor(
                            node,
                            sort_field,
                            &cursor_val,
                            &cursor_id.to_string(),
                            sort_direction,
                        ) == std::cmp::Ordering::Greater
                        {
                            remaining.push(*node);
                        }
                    }
                    remaining
                }
                None => filtered, // Bad cursor → return from start
            }
        } else {
            filtered
        };

        if items.len() > limit {
            let page = items[..limit].to_vec();
            has_more = true;
            if let Some(last) = page.last() {
                // Encode cursor with the sort field value (not hardcoded last_seen)
                let sort_val = node_field_value(last, sort_field);
                let cursor_val = match sort_val {
                    spindle_api::FilterValue::Str(s) => s,
                    spindle_api::FilterValue::Timestamp(dt) => dt.to_rfc3339(),
                    _ => last.node_id.clone(),
                };
                next_cursor = Some(encode_cursor(
                    &cursor_val,
                    Uuid::new_v5(&Uuid::NAMESPACE_URL, cursor_val.as_bytes()),
                    sort_direction.as_str(),
                ));
            }
            result.extend(page.into_iter().map(|n| {
                if scope.is_compliance_auditor() && !scope.is_admin() {
                    // Auditor: strip sensitive fields from summary
                    let mut s = n.to_summary();
                    s.provenance = None;
                    s
                } else {
                    n.to_summary()
                }
            }));
        } else {
            result.extend(items.into_iter().map(|n| {
                if scope.is_compliance_auditor() && !scope.is_admin() {
                    let mut s = n.to_summary();
                    s.provenance = None;
                    s
                } else {
                    n.to_summary()
                }
            }));
        }

        let pagination_result = PaginationResult {
            total_count,
            has_more,
            next_cursor,
        };

        Ok((result, pagination_result))
    }

    /// Get full node detail by ID.
    /// Respects project scope — returns 403 if node is in a different project.
    /// For compliance-auditor role, attributes are stripped (set to null).
    async fn get_node_detail(&self, id: &str, scope: &Scope) -> Result<NodeDetail, StoreError> {
        let all = self.nodes.read().unwrap();
        let node = all.iter().find(|n| n.node_id == id);
        match node {
            Some(n) => {
                // Check scope
                if scope.is_scoped() && !scope.has_project(&n.project_id) {
                    return Err(StoreError::ScopeDenied(format!(
                        "Node {} is not in the caller's project scope",
                        id
                    )));
                }
                let mut detail = n.to_detail();
                // Strip attributes for compliance-auditor role
                if scope.is_compliance_auditor() && !scope.is_admin() {
                    detail.attributes = serde_json::Value::Null;
                }
                Ok(detail)
            }
            None => Err(StoreError::NotFound(format!("Node {} not found", id))),
        }
    }

    /// Get lean node state by ID (no attributes).
    /// Respects project scope.
    async fn get_node_state(&self, id: &str, scope: &Scope) -> Result<NodeState, StoreError> {
        let all = self.nodes.read().unwrap();
        let node = all.iter().find(|n| n.node_id == id);
        match node {
            Some(n) => {
                if scope.is_scoped() && !scope.has_project(&n.project_id) {
                    return Err(StoreError::ScopeDenied(format!(
                        "Node {} is not in the caller's project scope",
                        id
                    )));
                }
                Ok(n.to_state())
            }
            None => Err(StoreError::NotFound(format!("Node {} not found", id))),
        }
    }

    /// Count nodes in the caller's project scope.
    async fn count_nodes(&self, scope: &Scope) -> Result<usize, StoreError> {
        let all = self.nodes.read().unwrap();
        if scope.is_scoped() {
            let count = all
                .iter()
                .filter(|n| scope.has_project(&n.project_id))
                .count();
            Ok(count)
        } else {
            Ok(all.len())
        }
    }
}

// ── DB-backed store (spindle-store) ─────────────────────────────────────

/// PostgreSQL-backed implementation of `NodeStore`, backed by
/// `spindle_store::SqlxNodeStore`. Queries the `nodes` table and maps the
/// store-crate `Node` rows into the web DTOs (so /v1/nodes reflects real
/// ingested Postgres rows rather than the seeded in-memory sample data).
pub struct DbNodeStore {
    inner: Arc<spindle_store::SqlxNodeStore>,
}

impl std::fmt::Debug for DbNodeStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The innermost SqlxNodeStore is opaque; report the adapter name only.
        f.debug_struct("DbNodeStore").finish_non_exhaustive()
    }
}

impl DbNodeStore {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self {
            inner: Arc::new(spindle_store::SqlxNodeStore::new(pool)),
        }
    }

    fn map_store_err(err: spindle_store::StoreError) -> StoreError {
        match err {
            spindle_store::StoreError::NotFound(msg) => StoreError::NotFound(msg),
            spindle_store::StoreError::ScopeDenied(msg) => StoreError::ScopeDenied(msg),
            other => StoreError::QueryFailed(other.to_string()),
        }
    }

    /// Map a store-crate `Node` into a web `NodeSummary`.
    fn to_summary(node: &spindle_store::Node) -> NodeSummary {
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
}

#[async_trait::async_trait]
impl NodeStore for DbNodeStore {
    /// List nodes from Postgres, sorted by `last_seen` desc, then cursor-paginate.
    /// Passing `None` for the store filter (the web QueryFilter is not pushed down
    /// to the store layer — filtering/pagination happen here on the mapped rows).
    async fn list_nodes_filtered(
        &self,
        _filter: &QueryFilter,
        pagination: &PaginationParams,
        scope: &Scope,
    ) -> Result<(Vec<NodeSummary>, PaginationResult), StoreError> {
        let nodes = self
            .inner
            .list_nodes(None, scope)
            .await
            .map_err(Self::map_store_err)?;

        let mut summaries: Vec<NodeSummary> = nodes.iter().map(Self::to_summary).collect();

        // Sort by last_seen desc (mirror the default ordering in the in-memory store).
        summaries.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));

        let total_count = summaries.len();

        // Resolve start index from an optional cursor (keyset by id).
        let start_idx = if let Some(cursor) = &pagination.cursor {
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

        let end_idx = (start_idx + pagination.limit).min(total_count);
        let items: Vec<NodeSummary> = summaries[start_idx..end_idx].to_vec();
        let next_cursor = if end_idx < total_count {
            let last = &summaries[end_idx - 1];
            let last_id = Uuid::parse_str(&last.id).unwrap_or_default();
            Some(encode_cursor(&last.id, last_id, &pagination.sort_direction))
        } else {
            None
        };

        let pagination_result =
            PaginationResult::from_query(pagination.limit, items.len(), total_count, next_cursor);

        Ok((items, pagination_result))
    }

    /// Get full node detail from Postgres by UUID.
    async fn get_node_detail(&self, id: &str, scope: &Scope) -> Result<NodeDetail, StoreError> {
        let id_parsed = Uuid::parse_str(id)
            .map_err(|_| StoreError::NotFound(format!("Node {} not found", id)))?;
        let node = self
            .inner
            .get_node(id_parsed, scope)
            .await
            .map_err(Self::map_store_err)?;

        let mut attributes = node.attributes;
        // Mirror in-memory behavior: strip attributes for compliance-auditor.
        if scope.is_compliance_auditor() && !scope.is_admin() {
            attributes = serde_json::Value::Null;
        }

        Ok(NodeDetail {
            id: node.id.to_string(),
            node_type: "chef-client".to_string(),
            name: if node.name.is_empty() {
                None
            } else {
                Some(node.name)
            },
            platform: if node.platform.is_empty() {
                None
            } else {
                Some(node.platform)
            },
            chef_environment: if node.chef_environment.is_empty()
                || node.chef_environment == "_default"
            {
                None
            } else {
                Some(node.chef_environment)
            },
            policy_group: if node.policy_group.is_empty() {
                None
            } else {
                Some(node.policy_group)
            },
            policy_name: if node.policy_name.is_empty() {
                None
            } else {
                Some(node.policy_name)
            },
            attributes,
            last_seen: Some(node.last_seen),
            first_seen: None,
            run_list: vec![],
            status: "active".to_string(),
            project_id: Some("default".to_string()),
            created_at: node.created_at,
            updated_at: node.created_at,
        })
    }

    /// Get lean node state from Postgres by UUID (no attributes).
    async fn get_node_state(&self, id: &str, scope: &Scope) -> Result<NodeState, StoreError> {
        let id_parsed = Uuid::parse_str(id)
            .map_err(|_| StoreError::NotFound(format!("Node {} not found", id)))?;
        let node = self
            .inner
            .get_node(id_parsed, scope)
            .await
            .map_err(Self::map_store_err)?;

        Ok(NodeState {
            id: node.id.to_string(),
            node_type: "chef-client".to_string(),
            platform: if node.platform.is_empty() {
                None
            } else {
                Some(node.platform)
            },
            last_seen: Some(node.last_seen),
            project_id: Some("default".to_string()),
        })
    }

    /// Count nodes in the caller's scope.
    async fn count_nodes(&self, scope: &Scope) -> Result<usize, StoreError> {
        self.inner
            .count_nodes(scope)
            .await
            .map_err(Self::map_store_err)
    }
}

// ── Filter helpers ──────────────────────────────────────────────────────

/// Apply a single filter predicate to nodes.
fn apply_node_filter<'a>(
    nodes: &[&'a StoredNode],
    filter: &spindle_api::Filter,
) -> Vec<&'a StoredNode> {
    match &filter.value {
        Some(FilterValue::List(values)) => match filter.operator {
            FilterOp::In => values
                .iter()
                .filter_map(|v| {
                    nodes.iter().find(|n| {
                        matches!(filter.operator, FilterOp::In)
                            && node_field_value(n, &filter.field) == FilterValue::Str(v.clone())
                    })
                })
                .copied()
                .collect(),
            _ => nodes.to_vec(),
        },
        Some(ref value) => {
            let mut result = Vec::new();
            for n in nodes {
                let nv = node_field_value(n, &filter.field);
                match filter.operator {
                    FilterOp::Eq => {
                        if nv == *value {
                            result.push(*n);
                        }
                    }
                    FilterOp::Neq => {
                        if nv != *value {
                            result.push(*n);
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
                                result.push(*n);
                            }
                        }
                    }
                    FilterOp::Like => {
                        if let (FilterValue::Str(a), FilterValue::Str(b)) = (&nv, value) {
                            if a.contains(b.as_str()) {
                                result.push(*n);
                            }
                        }
                    }
                    FilterOp::Between | FilterOp::IsNull | FilterOp::In => {
                        result.push(*n);
                    }
                }
            }
            result
        }
        None => nodes.to_vec(),
    }
}

/// Extract a node's field value as a FilterValue.
fn node_field_value(node: &StoredNode, field: &str) -> FilterValue {
    match field {
        "name" => FilterValue::Str(node.name.clone().unwrap_or_default()),
        "platform" => FilterValue::Str(node.platform.clone().unwrap_or_default()),
        "chef_environment" => FilterValue::Str(node.chef_environment.clone().unwrap_or_default()),
        "policy_group" => FilterValue::Str(node.policy_group.clone().unwrap_or_default()),
        "policy_name" => FilterValue::Str(node.policy_name.clone().unwrap_or_default()),
        "node_id" => FilterValue::Str(node.node_id.clone()),
        "project_id" => FilterValue::Str(node.project_id.clone()),
        "node_type" => FilterValue::Str(node.node_type.clone()),
        "status" => FilterValue::Str(node.status.clone()),
        "last_seen" => FilterValue::Str(node.last_seen.map(|t| t.to_rfc3339()).unwrap_or_default()),
        "first_seen" => {
            FilterValue::Str(node.first_seen.map(|t| t.to_rfc3339()).unwrap_or_default())
        }
        "id" => FilterValue::Str(node.node_id.clone()),
        _ => FilterValue::Str(String::new()),
    }
}

/// Compare a node against a cursor value for keyset pagination.
fn compare_for_cursor(
    node: &StoredNode,
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
fn sort_nodes(nodes: &mut Vec<&StoredNode>, sort_field: &str, sort_direction: &SortDirection) {
    nodes.sort_by(|a, b| {
        let primary = match sort_field {
            "last_seen" => match (a.last_seen, b.last_seen) {
                (Some(a_dt), Some(b_dt)) => a_dt.cmp(&b_dt),
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, None) => std::cmp::Ordering::Equal,
            },
            "first_seen" => match (a.first_seen, b.first_seen) {
                (Some(a_dt), Some(b_dt)) => a_dt.cmp(&b_dt),
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, None) => std::cmp::Ordering::Equal,
            },
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

        // Tie-breaker: node_id ascending always
        ord.then_with(|| a.node_id.cmp(&b.node_id))
    });
}

/// Filter by time range on last_seen.
fn apply_time_range<'a>(nodes: &[&'a StoredNode], tr: &TimeRange) -> Vec<&'a StoredNode> {
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
    pub store: Arc<dyn NodeStore>,
}

impl NodesAppState {
    pub fn new(store: Arc<dyn NodeStore>) -> Self {
        Self { store }
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

    // Fetch from store
    let result = state
        .store
        .list_nodes_filtered(&filter, &pagination, &scope)
        .await;

    match result {
        Ok((items, pagination_result)) => {
            let is_auditor = scope.is_compliance_auditor() && !scope.is_admin();
            let response = PagedResponse {
                api_version: API_VERSION.to_string(),
                request_id,
                data: items,
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
        Err(StoreError::ScopeDenied(msg)) => {
            EnvelopeResponse::forbidden("scope_denied", &msg, &request_id).into_response()
        }
        Err(e) => EnvelopeResponse::bad_request("store_error", &format!("{}", e), &request_id)
            .into_response(),
    }
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

    match state.store.get_node_detail(&id, &scope).await {
        Ok(detail) => {
            let response = NodeDetailResponse {
                api_version: API_VERSION.to_string(),
                request_id: request_id.clone(),
                data: detail,
                provenance: None,
                stripped_attributes: if is_auditor { Some(true) } else { None },
            };
            Json(response).into_response()
        }
        Err(StoreError::NotFound(_)) => {
            EnvelopeResponse::not_found("not_found", &format!("Node {} not found", id), &request_id)
                .into_response()
        }
        Err(StoreError::ScopeDenied(msg)) => {
            EnvelopeResponse::forbidden("scope_denied", &msg, &request_id).into_response()
        }
        Err(e) => EnvelopeResponse::bad_request("store_error", &format!("{}", e), &request_id)
            .into_response(),
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

    match state.store.get_node_state(&id, &scope).await {
        Ok(state_data) => {
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
        Err(StoreError::NotFound(_)) => {
            EnvelopeResponse::not_found("not_found", &format!("Node {} not found", id), &request_id)
                .into_response()
        }
        Err(StoreError::ScopeDenied(msg)) => {
            EnvelopeResponse::forbidden("scope_denied", &msg, &request_id).into_response()
        }
        Err(e) => EnvelopeResponse::bad_request("store_error", &format!("{}", e), &request_id)
            .into_response(),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    fn make_state() -> NodesAppState {
        let store: Arc<dyn NodeStore> = Arc::new(InMemoryNodeStore::new());
        NodesAppState::new(store)
    }

    fn make_app() -> Router {
        let state = make_state();
        nodes_routes(state)
    }

    // ── DB-backed store (skipped when no live Postgres) ──────────────────

    /// Live PostgreSQL connection string mirroring the S9 e2e suite.
    const LIVE_DB_URL: &str =
        "postgres://spindle:CHANGE_ME@198.51.100.101:5432/spindle";

    async fn try_db_pool() -> Option<sqlx::PgPool> {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(LIVE_DB_URL)
            .await
            .ok()
    }

    #[tokio::test]
    async fn db_node_store_counts_rows_from_sqlx() {
        let pool = match try_db_pool().await {
            Some(p) => p,
            None => {
                eprintln!("SKIP: Live database not available");
                return;
            }
        };

        let store = DbNodeStore::new(pool);
        let scope = spindle_store::Scope::all();
        let count = store.count_nodes(&scope).await.expect("count query failed");
        assert!(count > 0, "expected at least one node in the live DB");
    }

    #[tokio::test]
    async fn db_node_store_detail_roundtrip() {
        let pool = match try_db_pool().await {
            Some(p) => p,
            None => {
                eprintln!("SKIP: Live database not available");
                return;
            }
        };

        let store = DbNodeStore::new(pool);
        let scope = spindle_store::Scope::all();
        let (summaries, _) = store
            .list_nodes_filtered(
                &QueryFilter::default(),
                &PaginationParams::default(),
                &scope,
            )
            .await
            .expect("list query failed");
        if summaries.is_empty() {
            eprintln!("SKIP: no nodes in DB to test detail");
            return;
        }
        let detail = store
            .get_node_detail(&summaries[0].id, &scope)
            .await
            .expect("detail query failed");
        assert_eq!(detail.id, summaries[0].id);
        assert_eq!(detail.node_type, "chef-client");
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

        // Most recently seen should be first (web-01 at now)
        assert_eq!(response.data[0].id, "node-ubuntu-web-01");
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

        // Oldest seen should be first (app-01 at 1 day ago)
        assert_eq!(response.data[0].id, "node-ubuntu-app-01");
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
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes/node-ubuntu-web-01")
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

        assert_eq!(response.data.id, "node-ubuntu-web-01");
        assert_eq!(response.data.name, Some("web-server-01".to_string()));
        assert_eq!(response.data.platform, Some("ubuntu".to_string()));
        assert_eq!(
            response.data.chef_environment,
            Some("production".to_string())
        );
        assert_eq!(response.data.policy_group, Some("web".to_string()));
        assert_eq!(response.data.policy_name, Some("apache2".to_string()));
        assert_eq!(response.data.run_list.len(), 2);
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
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes/node-ubuntu-web-01")
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
        assert_eq!(response.data.id, "node-ubuntu-web-01");
        assert_eq!(response.data.node_type, "chef-client");
        assert!(response.data.name.is_some());
        assert!(response.data.platform.is_some());
        assert!(response.data.chef_environment.is_some());
        assert!(response.data.policy_group.is_some());
        assert!(response.data.policy_name.is_some());
        assert!(response.data.attributes.is_object());
        assert!(response.data.last_seen.is_some());
        assert!(response.data.first_seen.is_some());
        assert!(!response.data.run_list.is_empty());
        assert_eq!(response.data.status, "active");
        assert!(response.data.project_id.is_some());
        assert!(response.data.created_at.le(&Utc::now()));
        assert!(response.data.updated_at.le(&Utc::now()));
    }

    // ── GET /v1/nodes/:id/state — lean state ─────────────────────────────

    #[tokio::test]
    async fn test_get_node_state_found() {
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes/node-ubuntu-web-01/state")
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
        assert_eq!(response.data[0].id, "node-ubuntu-web-01");
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
        let app = make_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/nodes/node-ubuntu-web-01/state")
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
        assert_eq!(state.id, "node-ubuntu-web-01");
        assert_eq!(state.node_type, "chef-client");
        assert!(state.platform.is_some());
        assert!(state.last_seen.is_some());
    }

    // ── In-memory store unit tests ──────────────────────────────────────

    #[tokio::test]
    async fn test_store_list_filters_by_platform() {
        let store = InMemoryNodeStore::new();

        let mut fake_filter = QueryFilter::default();
        fake_filter.filters = vec![spindle_api::Filter {
            field: "platform".to_string(),
            operator: FilterOp::Eq,
            value: Some(FilterValue::Str("ubuntu".to_string())),
        }];

        let pagination = PaginationParams::default();
        let scope = Scope::all();

        let (items, _) = store
            .list_nodes_filtered(&fake_filter, &pagination, &scope)
            .await
            .unwrap();

        assert_eq!(items.len(), 3); // ubuntu nodes
        for item in &items {
            assert_eq!(item.platform, Some("ubuntu".to_string()));
        }
    }

    #[tokio::test]
    async fn test_store_list_filters_by_policy_group() {
        let store = InMemoryNodeStore::new();

        let mut fake_filter = QueryFilter::default();
        fake_filter.filters = vec![spindle_api::Filter {
            field: "policy_group".to_string(),
            operator: FilterOp::Eq,
            value: Some(FilterValue::Str("web".to_string())),
        }];

        let pagination = PaginationParams::default();
        let scope = Scope::all();

        let (items, _) = store
            .list_nodes_filtered(&fake_filter, &pagination, &scope)
            .await
            .unwrap();

        assert_eq!(items.len(), 2); // web policy group nodes
        for item in &items {
            assert_eq!(item.policy_group, Some("web".to_string()));
        }
    }

    #[tokio::test]
    async fn test_store_get_detail() {
        let store = InMemoryNodeStore::new();
        let scope = Scope::all();

        let detail = store
            .get_node_detail("node-ubuntu-web-01", &scope)
            .await
            .unwrap();

        assert_eq!(detail.id, "node-ubuntu-web-01");
        assert_eq!(detail.attributes["hostname"], "web-01.example.com");
    }

    #[tokio::test]
    async fn test_store_get_detail_not_found() {
        let store = InMemoryNodeStore::new();
        let scope = Scope::all();

        let result = store.get_node_detail("nonexistent", &scope).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), StoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_store_get_state() {
        let store = InMemoryNodeStore::new();
        let scope = Scope::all();

        let state = store
            .get_node_state("node-ubuntu-web-01", &scope)
            .await
            .unwrap();

        assert_eq!(state.id, "node-ubuntu-web-01");
        assert_eq!(state.platform, Some("ubuntu".to_string()));
    }

    #[tokio::test]
    async fn test_store_get_state_not_found() {
        let store = InMemoryNodeStore::new();
        let scope = Scope::all();

        let result = store.get_node_state("nonexistent", &scope).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), StoreError::NotFound(_)));
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

        let mut fake_filter = QueryFilter::default();
        fake_filter.time_range = tr.clone();

        let pagination = PaginationParams::default();
        let scope = Scope::all();

        let (items, _) = store
            .list_nodes_filtered(&fake_filter, &pagination, &scope)
            .await
            .unwrap();

        // All 4 nodes have been seen within 7 days
        assert_eq!(items.len(), 4);
    }

    // ── Sort determinism ────────────────────────────────────────────────

    #[test]
    fn test_sort_deterministic_ordering() {
        let store = InMemoryNodeStore::new();
        let binding = store.nodes.read().unwrap();
        let mut nodes: Vec<&StoredNode> = binding.iter().collect();

        // Sort by platform asc — should be deterministic even when platforms equal
        sort_nodes(&mut nodes, "platform", &SortDirection::Asc);

        // First node should be centos (alphabetically before ubuntu)
        assert_eq!(nodes[0].platform, Some("centos".to_string()));
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
        let store = InMemoryNodeStore::new();

        // Create an ACME-scoped filter
        let mut fake_filter = QueryFilter::default();
        fake_filter.filters = vec![spindle_api::Filter {
            field: "project_id".to_string(),
            operator: FilterOp::Eq,
            value: Some(FilterValue::Str("acme".to_string())),
        }];

        let pagination = PaginationParams::default();
        let scope = Scope::all();

        let (items, _) = store
            .list_nodes_filtered(&fake_filter, &pagination, &scope)
            .await
            .unwrap();

        // acme: 3 nodes (web-01, db-01, app-01)
        assert_eq!(items.len(), 3);
    }

    #[tokio::test]
    async fn test_project_scoping_globex() {
        let store = InMemoryNodeStore::new();

        let mut fake_filter = QueryFilter::default();
        fake_filter.filters = vec![spindle_api::Filter {
            field: "project_id".to_string(),
            operator: FilterOp::Eq,
            value: Some(FilterValue::Str("globex".to_string())),
        }];

        let pagination = PaginationParams::default();
        let scope = Scope::all();

        let (items, _) = store
            .list_nodes_filtered(&fake_filter, &pagination, &scope)
            .await
            .unwrap();

        // globex: 1 node (web-02)
        assert_eq!(items.len(), 1);
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

        // Default sort is last_seen desc → most recent first (web-01 at now)
        assert_eq!(response.data[0].id, "node-ubuntu-web-01");
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
                        .uri(&format!("/v1/nodes/{}", node_id))
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
                        .uri(&format!("/v1/nodes/{}/state", node_id))
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

        let detail = store
            .get_node_detail("node-ubuntu-web-01", &scope)
            .await
            .unwrap();
        let state = store
            .get_node_state("node-ubuntu-web-01", &scope)
            .await
            .unwrap();

        // Detail has: name, chef_env, policy_group, policy_name, attributes, first_seen,
        //             run_list, status, project_id
        // State has: just id, node_type, platform, last_seen, project_id
        assert!(detail.name.is_some());
        assert!(detail.chef_environment.is_some());
        assert!(detail.policy_group.is_some());
        assert!(detail.policy_name.is_some());
        assert!(detail.attributes.is_object());
        assert!(detail.first_seen.is_some());
        assert!(!detail.run_list.is_empty());
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
        let app = make_app();
        let req = axum::http::Request::builder()
            .uri("/v1/nodes/node-ubuntu-web-01")
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
        let app = make_app();
        let req = axum::http::Request::builder()
            .uri("/v1/nodes/node-ubuntu-web-01/state")
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
