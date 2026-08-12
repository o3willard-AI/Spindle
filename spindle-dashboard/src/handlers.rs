//! Page handlers: fetch data from the Spindle REST API (proxying the caller's
//! bearer token) and render askama templates. Every page is stateless.

#![allow(warnings)]

use crate::api::{api_get, api_list, extract_token, ApiError};
use crate::models::*;
use crate::AppState;
use askama::Template;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap};
use axum::response::{Html, IntoResponse};
use axum::Json;

// ── Rendering helpers ───────────────────────────────────────────────────────

/// A single key/value row for rendering arbitrary JSON (attributes, reports).
pub struct InfoRow {
    pub key: String,
    pub value: String,
}

fn info_rows(value: &serde_json::Value) -> Vec<InfoRow> {
    match value {
        serde_json::Value::Object(map) => map
            .iter()
            .filter_map(|(k, v)| {
                let val = if v.is_null() {
                    "—".to_string()
                } else {
                    v.to_string()
                };
                Some(InfoRow { key: k.clone(), value: val })
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn render<T: Template>(t: &T) -> Html<String> {
    Html(t.render().unwrap_or_else(|e| format!("<pre>template error: {e}</pre>")))
}

/// Timestamp parsing tolerant of RFC3339 forms (incl. trailing `Z`).
fn ts_epoch(s: &Option<String>) -> Option<i64> {
    s.as_ref().and_then(|v| {
        chrono::DateTime::parse_from_rfc3339(v)
            .ok()
            .map(|d| d.timestamp())
    })
}

fn fmt_opt(s: &Option<String>) -> String {
    s.clone().unwrap_or_else(|| "—".into())
}

// ── Node rows ───────────────────────────────────────────────────────────────

pub struct NodeRow {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub chef_environment: String,
    pub policy_group: String,
    pub policy_name: String,
    pub last_seen: String,
    pub status: String,
}

impl NodeRow {
    pub fn display_status(&self) -> &str {
        &self.status
    }
    pub fn status_class(&self) -> &str {
        if self.status == "online" {
            "ok"
        } else {
            "down"
        }
    }
}

/// Build a fleet-oriented row (deduped by name, keeping the most recent).
pub fn fleet_rows(nodes: &[NodeSummary]) -> Vec<NodeRow> {
    use std::collections::HashMap;
    let mut by_name: HashMap<String, (i64, NodeRow)> = HashMap::new();
    for n in nodes {
        let name = n.name.as_deref().unwrap_or("unnamed").to_string();
        let seen = ts_epoch(&n.last_seen).unwrap_or(0);
        let status = if ts_epoch(&n.last_seen).map(|t| (chrono::Utc::now().timestamp() - t) < 300).unwrap_or(false) {
            "online"
        } else {
            "offline"
        };
        let row = NodeRow {
            id: n.id.clone(),
            name: name.clone(),
            platform: n.platform.as_deref().unwrap_or("unknown").to_string(),
            chef_environment: fmt_opt(&n.chef_environment),
            policy_group: fmt_opt(&n.policy_group),
            policy_name: fmt_opt(&n.policy_name),
            last_seen: fmt_opt(&n.last_seen),
            status: status.to_string(),
        };
        let prev = by_name.get(&name).map(|(s, _)| *s).unwrap_or(-1);
        if seen > prev {
            by_name.insert(name, (seen, row));
        }
    }
    let mut rows: Vec<NodeRow> = by_name.into_values().map(|(_, r)| r).collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows
}

// ── Run rows ────────────────────────────────────────────────────────────────

pub struct RunRow {
    pub id: String,
    pub node_id: String,
    pub status: String,
    pub start_time: String,
    pub end_time: String,
    pub duration_ms: i64,
    pub total_resources: i64,
    pub updated: i64,
    pub failed: i64,
    pub skipped: i64,
    pub cookbook_name: String,
    pub cookbook_version: String,
}

impl RunRow {
    pub fn display_status(&self) -> &str {
        &self.status
    }
    pub fn status_class(&self) -> &str {
        match self.status.as_str() {
            "success" | "completed" => "ok",
            "failed" | "error" => "bad",
            _ => "warn",
        }
    }
}

fn run_rows(runs: &[RunSummary]) -> Vec<RunRow> {
    runs.iter()
        .map(|r| RunRow {
            id: r.id.clone(),
            node_id: fmt_opt(&r.node_id),
            status: r.status.clone().unwrap_or_default(),
            start_time: fmt_opt(&r.start_time),
            end_time: fmt_opt(&r.end_time),
            duration_ms: r.duration_ms.unwrap_or(0),
            total_resources: r.total_resource_count.unwrap_or(0),
            updated: r.updated_count.unwrap_or(0),
            failed: r.failed_count.unwrap_or(0),
            skipped: r.skipped_count.unwrap_or(0),
            cookbook_name: fmt_opt(&r.cookbook_name),
            cookbook_version: fmt_opt(&r.cookbook_version),
        })
        .collect()
}

/// Pre-resolved resource event for display (no `Option`s in the template).
pub struct EventRow {
    pub resource_type: String,
    pub resource_name: String,
    pub action: String,
    pub status: String,
    pub duration_ms: String,
    pub cookbook: String,
}

fn event_rows(events: &[ResourceEvent]) -> Vec<EventRow> {
    events
        .iter()
        .map(|e| EventRow {
            resource_type: fmt_opt(&e.resource_type),
            resource_name: fmt_opt(&e.resource_name),
            action: fmt_opt(&e.action),
            status: fmt_opt(&e.status),
            duration_ms: e.duration_ms.map(|v| format!("{v} ms")).unwrap_or_else(|| "—".into()),
            cookbook: format!(
                "{} {}",
                fmt_opt(&e.cookbook_name),
                fmt_opt(&e.cookbook_version)
            )
            .trim()
            .to_string(),
        })
        .collect()
}

// ── Cookbook rows ───────────────────────────────────────────────────────────

pub struct CookbookRow {
    pub name: String,
    pub total_nodes: String,
    pub versions: String,
    pub last_seen: String,
}

fn cookbook_rows(books: &[CookbookInventoryEntry]) -> Vec<CookbookRow> {
    books
        .iter()
        .map(|b| CookbookRow {
            name: b.name.clone().unwrap_or_default(),
            total_nodes: b.total_nodes.map(|v| v.to_string()).unwrap_or("0".into()),
            versions: b
                .versions
                .iter()
                .filter_map(|v| v.cookbook_version.clone())
                .collect::<Vec<_>>()
                .join(", "),
            last_seen: fmt_opt(&b.last_seen),
        })
        .collect()
}

/// A single cookbook version for the detail page (no `Option`s).
pub struct VersionRow {
    pub version: String,
    pub node_count: String,
    pub total_resources: String,
    pub first_seen: String,
    pub last_seen: String,
}

fn version_rows(versions: &[CookbookVersionInfo]) -> Vec<VersionRow> {
    versions
        .iter()
        .map(|v| VersionRow {
            version: fmt_opt(&v.cookbook_version),
            node_count: v.node_count.map(|n| n.to_string()).unwrap_or_else(|| "0".into()),
            total_resources: v
                .total_resource_count
                .map(|n| n.to_string())
                .unwrap_or_else(|| "0".into()),
            first_seen: fmt_opt(&v.first_seen),
            last_seen: fmt_opt(&v.last_seen),
        })
        .collect()
}

// ── Login / error views ─────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginView {
    pub api_url: String,
}

#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorView {
    pub api_url: String,
    pub message: String,
}

// ── Dashboard ───────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardView {
    pub api_url: String,
    pub page_name: &'static str,
    pub nodes: Vec<NodeRow>,
    pub total_nodes: usize,
    pub online_nodes: usize,
    pub offline_nodes: usize,
    pub total_runs: usize,
    pub success_runs: usize,
    pub failed_runs: usize,
    pub api_status: String,
}

pub async fn dashboard(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let token = extract_token(&headers);
    let nodes = match api_list::<NodeSummary>(&st, "/v1/nodes", &token).await {
        Ok(v) => v,
        Err(ApiError::Unauthorized(..)) => return render(&LoginView { api_url: st.api_url }).into_response(),
        Err(e) => return render(&ErrorView { api_url: st.api_url, message: e.to_string() }).into_response(),
    };
    let rows = fleet_rows(&nodes);
    let online = rows.iter().filter(|r| r.status == "online").count();

    let runs = match api_list::<RunSummary>(&st, "/v1/runs", &token).await {
        Ok(v) => v,
        Err(ApiError::Unauthorized(..)) => return render(&LoginView { api_url: st.api_url }).into_response(),
        Err(e) => return render(&ErrorView { api_url: st.api_url, message: e.to_string() }).into_response(),
    };
    let success = runs.iter().filter(|r| matches!(r.status.as_deref(), Some("success") | Some("completed"))).count();
    let failed = runs.iter().filter(|r| matches!(r.status.as_deref(), Some("failed") | Some("error"))).count();

    let api_status = match api_get::<serde_json::Value>(&st, "/v1/health", &token).await {
        Ok(h) => h
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown")
            .to_string(),
        Err(_) => "unknown".to_string(),
    };

    let view = DashboardView {
        api_url: st.api_url,
        page_name: "dashboard",
        total_nodes: rows.len(),
        online_nodes: online,
        offline_nodes: rows.len() - online,
        total_runs: runs.len(),
        success_runs: success,
        failed_runs: failed,
        nodes: rows,
        api_status,
    };
    render(&view).into_response()
}

// ── htmx live-poll partial ──────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "partials/fleet.html")]
pub struct FleetPartial {
    pub nodes: Vec<NodeRow>,
}

pub async fn fleet_partial(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let token = extract_token(&headers);
    let nodes = match api_list::<NodeSummary>(&st, "/v1/nodes?limit=20", &token).await {
        Ok(v) => v,
        Err(_) => Vec::new(),
    };
    render(&FleetPartial { nodes: fleet_rows(&nodes) }).into_response()
}

// ── Nodes ───────────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "nodes.html")]
pub struct NodesView {
    pub api_url: String,
    pub page_name: &'static str,
    pub nodes: Vec<NodeRow>,
}

pub async fn nodes_list(State(st): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let token = extract_token(&headers);
    let nodes = match api_list::<NodeSummary>(&st, "/v1/nodes", &token).await {
        Ok(v) => v,
        Err(ApiError::Unauthorized(..)) => return render(&LoginView { api_url: st.api_url }).into_response(),
        Err(e) => return render(&ErrorView { api_url: st.api_url, message: e.to_string() }).into_response(),
    };
    render(&NodesView { api_url: st.api_url, page_name: "nodes", nodes: fleet_rows(&nodes) }).into_response()
}

#[derive(Template)]
#[template(path = "node.html")]
pub struct NodeView {
    pub api_url: String,
    pub page_name: &'static str,
    pub node: NodeRow,
    pub status: String,
    pub attributes: Vec<InfoRow>,
    pub run_list: Vec<String>,
}

pub async fn node_detail(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let token = extract_token(&headers);
    let nodes = match api_list::<NodeSummary>(&st, "/v1/nodes", &token).await {
        Ok(v) => v,
        Err(ApiError::Unauthorized(..)) => return render(&LoginView { api_url: st.api_url }).into_response(),
        Err(e) => return render(&ErrorView { api_url: st.api_url, message: e.to_string() }).into_response(),
    };
    // Resolve name → most recent summary (the API resolves detail by UUID).
    let summary = fleet_rows(&nodes)
        .into_iter()
        .find(|r| r.name.eq_ignore_ascii_case(&name));
    let summary = match summary {
        Some(s) => s,
        None => {
            return render(&ErrorView { api_url: st.api_url, message: format!("Node '{name}' not found") })
                .into_response()
        }
    };
    let detail = match api_get::<NodeDetailEnvelope>(&st, &format!("/v1/nodes/{}", summary.id), &token).await {
        Ok(d) => d.data,
        Err(ApiError::Unauthorized(..)) => return render(&LoginView { api_url: st.api_url }).into_response(),
        Err(e) => return render(&ErrorView { api_url: st.api_url, message: e.to_string() }).into_response(),
    };
    let row = NodeRow {
        id: detail.id.clone(),
        name: detail.name.clone().unwrap_or_else(|| name.clone()),
        platform: detail.platform.as_deref().unwrap_or("unknown").to_string(),
        chef_environment: fmt_opt(&detail.chef_environment),
        policy_group: fmt_opt(&detail.policy_group),
        policy_name: fmt_opt(&detail.policy_name),
        last_seen: fmt_opt(&detail.last_seen),
        status: summary.status,
    };
    let view = NodeView {
        api_url: st.api_url,
        page_name: "nodes",
        node: row,
        status: if detail.status.is_empty() { "—".to_string() } else { detail.status.clone() },
        attributes: info_rows(&detail.attributes),
        run_list: detail.run_list.clone(),
    };
    render(&view).into_response()
}

// ── Runs ────────────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "runs.html")]
pub struct RunsView {
    pub api_url: String,
    pub page_name: &'static str,
    pub runs: Vec<RunRow>,
}

pub async fn runs_list(State(st): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let token = extract_token(&headers);
    let runs = match api_list::<RunSummary>(&st, "/v1/runs", &token).await {
        Ok(v) => v,
        Err(ApiError::Unauthorized(..)) => return render(&LoginView { api_url: st.api_url }).into_response(),
        Err(e) => return render(&ErrorView { api_url: st.api_url, message: e.to_string() }).into_response(),
    };
    render(&RunsView { api_url: st.api_url, page_name: "runs", runs: run_rows(&runs) }).into_response()
}

#[derive(Template)]
#[template(path = "run.html")]
pub struct RunView {
    pub api_url: String,
    pub page_name: &'static str,
    pub run: RunRow,
    pub events: Vec<EventRow>,
}

pub async fn run_detail(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let token = extract_token(&headers);
    let detail = match api_get::<RunDetailEnvelope>(&st, &format!("/v1/runs/{}", id), &token).await {
        Ok(d) => d.data,
        Err(ApiError::Unauthorized(..)) => return render(&LoginView { api_url: st.api_url }).into_response(),
        Err(e) => return render(&ErrorView { api_url: st.api_url, message: e.to_string() }).into_response(),
    };
    let row = RunRow {
        id: detail.id.unwrap_or_else(|| id.clone()),
        node_id: fmt_opt(&detail.node_id),
        status: detail.status.unwrap_or_default(),
        start_time: fmt_opt(&detail.start_time),
        end_time: fmt_opt(&detail.end_time),
        duration_ms: detail.duration_ms.unwrap_or(0),
        total_resources: detail.total_resource_count.unwrap_or(0),
        updated: detail.updated_count.unwrap_or(0),
        failed: detail.failed_count.unwrap_or(0),
        skipped: detail.skipped_count.unwrap_or(0),
        cookbook_name: fmt_opt(&detail.cookbook_name),
        cookbook_version: fmt_opt(&detail.cookbook_version),
    };
    let view = RunView {
        api_url: st.api_url,
        page_name: "runs",
        run: row,
        events: event_rows(&detail.resource_events.items),
    };
    render(&view).into_response()
}

// ── Compliance ──────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "compliance.html")]
pub struct ComplianceView {
    pub api_url: String,
    pub page_name: &'static str,
    pub total: i64,
    pub reports: Vec<Vec<InfoRow>>,
}

pub async fn compliance_list(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let token = extract_token(&headers);
    let list = match api_get::<serde_json::Value>(&st, "/v1/compliance/reports", &token).await {
        Ok(v) => v,
        Err(ApiError::Unauthorized(..)) => return render(&LoginView { api_url: st.api_url }).into_response(),
        Err(e) => return render(&ErrorView { api_url: st.api_url, message: e.to_string() }).into_response(),
    };
    let list: ComplianceList = match serde_json::from_value(list.get("data").cloned().unwrap_or_default()) {
        Ok(l) => l,
        Err(e) => {
            return render(&ErrorView { api_url: st.api_url, message: format!("compliance parse error: {e}") })
                .into_response()
        }
    };
    let reports = list.items.iter().map(info_rows).collect();
    let view = ComplianceView {
        api_url: st.api_url,
        page_name: "compliance",
        total: list.total,
        reports,
    };
    render(&view).into_response()
}

#[derive(Template)]
#[template(path = "compliance_detail.html")]
pub struct ComplianceDetailView {
    pub api_url: String,
    pub page_name: &'static str,
    pub report_id: String,
    pub rows: Vec<InfoRow>,
}

pub async fn compliance_detail(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let token = extract_token(&headers);
    let v = match api_get::<serde_json::Value>(&st, &format!("/v1/compliance/reports/{}", id), &token).await {
        Ok(v) => v,
        Err(ApiError::Unauthorized(..)) => return render(&LoginView { api_url: st.api_url }).into_response(),
        Err(e) => return render(&ErrorView { api_url: st.api_url, message: e.to_string() }).into_response(),
    };
    let rows = info_rows(&v);
    let view = ComplianceDetailView { api_url: st.api_url, page_name: "compliance", report_id: id, rows };
    render(&view).into_response()
}

// ── Cookbooks ───────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "cookbooks.html")]
pub struct CookbooksView {
    pub api_url: String,
    pub page_name: &'static str,
    pub books: Vec<CookbookRow>,
}

pub async fn cookbooks_list(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let token = extract_token(&headers);
    let books = match api_list::<CookbookInventoryEntry>(&st, "/v1/cookbooks", &token).await {
        Ok(v) => v,
        Err(ApiError::Unauthorized(..)) => return render(&LoginView { api_url: st.api_url }).into_response(),
        Err(e) => return render(&ErrorView { api_url: st.api_url, message: e.to_string() }).into_response(),
    };
    render(&CookbooksView { api_url: st.api_url, page_name: "cookbooks", books: cookbook_rows(&books) })
        .into_response()
}

#[derive(Template)]
#[template(path = "cookbook.html")]
pub struct CookbookView {
    pub api_url: String,
    pub page_name: &'static str,
    pub name: String,
    pub total_nodes: String,
    pub last_seen: String,
    pub versions: Vec<VersionRow>,
}

pub async fn cookbook_detail(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let token = extract_token(&headers);
    let books = match api_list::<CookbookInventoryEntry>(&st, "/v1/cookbooks", &token).await {
        Ok(v) => v,
        Err(ApiError::Unauthorized(..)) => return render(&LoginView { api_url: st.api_url }).into_response(),
        Err(e) => return render(&ErrorView { api_url: st.api_url, message: e.to_string() }).into_response(),
    };
    let book = books.into_iter().find(|b| b.name.as_deref() == Some(name.as_str()));
    let book = match book {
        Some(b) => b,
        None => {
            return render(&ErrorView { api_url: st.api_url, message: format!("Cookbook '{name}' not found") })
                .into_response()
        }
    };
    let view = CookbookView {
        api_url: st.api_url,
        page_name: "cookbooks",
        name: book.name.clone().unwrap_or_else(|| name.clone()),
        total_nodes: book.total_nodes.map(|v| v.to_string()).unwrap_or("0".into()),
        last_seen: fmt_opt(&book.last_seen),
        versions: version_rows(&book.versions),
    };
    render(&view).into_response()
}

// ── Login ───────────────────────────────────────────────────────────────────

pub async fn login(State(st): State<AppState>) -> impl IntoResponse {
    render(&LoginView { api_url: st.api_url }).into_response()
}

// ── Static assets (embedded at compile time — no runtime files needed) ──────

pub async fn static_asset(Path(path): Path<String>) -> impl IntoResponse {
    match path.as_str() {
        "style.css" => {
            ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], STATIC_CSS).into_response()
        }
        "app.js" => {
            ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], STATIC_JS).into_response()
        }
        "htmx.min.js" => (
            [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
            include_str!("../static/htmx.min.js"),
        )
            .into_response(),
        _ => (axum::http::StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

pub const STATIC_CSS: &str = include_str!("../static/style.css");
pub const STATIC_JS: &str = include_str!("../static/app.js");

/// JSON helper used nowhere at runtime but keeps `Json` referenced in case
/// future hooks return JSON. (Prevents unused-import churn.)
#[allow(dead_code)]
pub fn json_discard() -> Json<()> {
    Json(())
}