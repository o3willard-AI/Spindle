//! Typed response models mirroring the Spindle REST API payloads.
//!
//! Field access is defensive (all `Option`/defaulted) so the dashboard keeps
//! serving pages even when the upstream API adds or omits fields.
#![allow(dead_code)] // model fields exist to tolerate/ignore upstream fields

use serde::Deserialize;

// ── Nodes ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NodeSummary {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub node_type: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub chef_environment: Option<String>,
    #[serde(default)]
    pub policy_group: Option<String>,
    #[serde(default)]
    pub policy_name: Option<String>,
    #[serde(default)]
    pub last_seen: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NodeDetail {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub node_type: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub chef_environment: Option<String>,
    #[serde(default)]
    pub policy_group: Option<String>,
    #[serde(default)]
    pub policy_name: Option<String>,
    #[serde(default)]
    pub attributes: serde_json::Value,
    #[serde(default)]
    pub last_seen: Option<String>,
    #[serde(default)]
    pub first_seen: Option<String>,
    #[serde(default)]
    pub run_list: Vec<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NodeDetailEnvelope {
    pub data: NodeDetail,
}

// ── Runs ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RunSummary {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub start_time: Option<String>,
    #[serde(default)]
    pub end_time: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub total_resource_count: Option<i64>,
    #[serde(default)]
    pub updated_count: Option<i64>,
    #[serde(default)]
    pub failed_count: Option<i64>,
    #[serde(default)]
    pub skipped_count: Option<i64>,
    #[serde(default)]
    pub cookbook_name: Option<String>,
    #[serde(default)]
    pub cookbook_version: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResourceEvent {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub resource_type: Option<String>,
    #[serde(default)]
    pub resource_name: Option<String>,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub cookbook_name: Option<String>,
    #[serde(default)]
    pub cookbook_version: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResourceEventPage {
    #[serde(default)]
    pub items: Vec<ResourceEvent>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RunDetail {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub start_time: Option<String>,
    #[serde(default)]
    pub end_time: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub total_resource_count: Option<i64>,
    #[serde(default)]
    pub updated_count: Option<i64>,
    #[serde(default)]
    pub failed_count: Option<i64>,
    #[serde(default)]
    pub skipped_count: Option<i64>,
    #[serde(default)]
    pub cookbook_name: Option<String>,
    #[serde(default)]
    pub cookbook_version: Option<String>,
    #[serde(default)]
    pub error_summary: Option<serde_json::Value>,
    #[serde(default)]
    pub resource_events: ResourceEventPage,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunDetailEnvelope {
    pub data: RunDetail,
}

// ── Cookbooks ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CookbookVersionInfo {
    #[serde(default)]
    pub cookbook_name: Option<String>,
    #[serde(default)]
    pub cookbook_version: Option<String>,
    #[serde(default)]
    pub node_count: Option<i64>,
    #[serde(default)]
    pub node_ids: Vec<String>,
    #[serde(default)]
    pub first_seen: Option<String>,
    #[serde(default)]
    pub last_seen: Option<String>,
    #[serde(default)]
    pub total_resource_count: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CookbookInventoryEntry {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub versions: Vec<CookbookVersionInfo>,
    #[serde(default)]
    pub total_nodes: Option<i64>,
    #[serde(default)]
    pub last_seen: Option<String>,
}

// ── Compliance (list payload is generic/stub upstream) ───────────────────

/// The compliance reports list returns `{ data: { items, total, page, ... } }`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ComplianceList {
    #[serde(default)]
    pub items: Vec<serde_json::Value>,
    #[serde(default)]
    pub total: i64,
    #[serde(default)]
    pub page: i64,
    #[serde(default)]
    pub pages: i64,
}