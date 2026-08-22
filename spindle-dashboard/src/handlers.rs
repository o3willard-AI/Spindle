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
                Some(InfoRow {
                    key: k.clone(),
                    value: val,
                })
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn render<T: Template>(t: &T) -> Html<String> {
    Html(
        t.render()
            .unwrap_or_else(|e| format!("<pre>template error: {e}</pre>")),
    )
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
        let status = if ts_epoch(&n.last_seen)
            .map(|t| (chrono::Utc::now().timestamp() - t) < 300)
            .unwrap_or(false)
        {
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
            duration_ms: e
                .duration_ms
                .map(|v| format!("{v} ms"))
                .unwrap_or_else(|| "—".into()),
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
            node_count: v
                .node_count
                .map(|n| n.to_string())
                .unwrap_or_else(|| "0".into()),
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

pub async fn dashboard(State(st): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let token = extract_token(&headers);
    let nodes = match api_list::<NodeSummary>(&st, "/v1/nodes", &token).await {
        Ok(v) => v,
        Err(ApiError::Unauthorized(..)) => {
            return render(&LoginView {
                api_url: st.api_url,
            })
            .into_response()
        }
        Err(e) => {
            return render(&ErrorView {
                api_url: st.api_url,
                message: e.to_string(),
            })
            .into_response()
        }
    };
    let rows = fleet_rows(&nodes);
    let online = rows.iter().filter(|r| r.status == "online").count();

    let runs = match api_list::<RunSummary>(&st, "/v1/runs", &token).await {
        Ok(v) => v,
        Err(ApiError::Unauthorized(..)) => {
            return render(&LoginView {
                api_url: st.api_url,
            })
            .into_response()
        }
        Err(e) => {
            return render(&ErrorView {
                api_url: st.api_url,
                message: e.to_string(),
            })
            .into_response()
        }
    };
    let success = runs
        .iter()
        .filter(|r| matches!(r.status.as_deref(), Some("success") | Some("completed")))
        .count();
    let failed = runs
        .iter()
        .filter(|r| matches!(r.status.as_deref(), Some("failed") | Some("error")))
        .count();

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

pub async fn fleet_partial(State(st): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let token = extract_token(&headers);
    let nodes = match api_list::<NodeSummary>(&st, "/v1/nodes?limit=20", &token).await {
        Ok(v) => v,
        Err(_) => Vec::new(),
    };
    render(&FleetPartial {
        nodes: fleet_rows(&nodes),
    })
    .into_response()
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
        Err(ApiError::Unauthorized(..)) => {
            return render(&LoginView {
                api_url: st.api_url,
            })
            .into_response()
        }
        Err(e) => {
            return render(&ErrorView {
                api_url: st.api_url,
                message: e.to_string(),
            })
            .into_response()
        }
    };
    render(&NodesView {
        api_url: st.api_url,
        page_name: "nodes",
        nodes: fleet_rows(&nodes),
    })
    .into_response()
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
    /// Latest compliance summary for this node (None when no reports exist).
    pub compliance: Option<NodeCompliance>,
}

/// A single failed control result rendered on the node page.
#[derive(Debug, Clone)]
pub struct ControlRow {
    pub control_id: String,
    pub status: String,
}

/// Node-level compliance summary. `None` on the handler means the node has no
/// compliance reports yet (renders "No compliance reports").
pub struct NodeCompliance {
    /// Report id(s) aggregated into this summary (newest per profile).
    pub report_id: String,
    /// Profile name(s) aggregated into this summary.
    pub profile_name: String,
    /// Aggregated status: `non-compliant`, `warn`, or `compliant`.
    pub status: String,
    pub passed_count: i64,
    pub failed_count: i64,
    pub warning_count: i64,
    pub failed_controls: Vec<ControlRow>,
}

impl NodeCompliance {
    /// Non-compliant when ANY aggregated profile has a failed control.
    pub fn is_non_compliant(&self) -> bool {
        self.failed_count > 0
    }
    /// Warn when no profile failed but at least one produced warnings.
    pub fn is_warn(&self) -> bool {
        self.failed_count == 0 && self.warning_count > 0
    }
    /// Compliant only when every aggregated profile fully passed.
    pub fn is_compliant(&self) -> bool {
        self.failed_count == 0 && self.warning_count == 0
    }
    /// CSS class for the compliance pill: `bad`, `warn`, or `ok`.
    pub fn compliance_class(&self) -> &'static str {
        if self.is_non_compliant() {
            "bad"
        } else if self.is_warn() {
            "warn"
        } else {
            "ok"
        }
    }
    /// Label shown next to the pill.
    pub fn compliance_label(&self) -> &'static str {
        if self.is_non_compliant() {
            "non-compliant"
        } else if self.is_warn() {
            "warn"
        } else {
            "compliant"
        }
    }
}

/// A single compliance report list item, parsed for aggregation.
#[derive(Debug, Clone)]
pub struct ReportSummary {
    pub id: String,
    pub profile_name: String,
    pub status: String,
    pub passed_count: i64,
    pub failed_count: i64,
    pub warning_count: i64,
    pub created_at: Option<String>,
    /// Failed controls (resolved from the report detail).
    pub failed_controls: Vec<ControlRow>,
}

impl ReportSummary {
    /// Parse a raw list item from `/v1/compliance/reports`.
    fn from_value(item: &serde_json::Value) -> Self {
        ReportSummary {
            id: item
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            profile_name: item
                .get("profile_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            status: item
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            passed_count: item
                .get("passed_count")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
            failed_count: item
                .get("failed_count")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
            warning_count: item
                .get("warning_count")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
            created_at: item
                .get("created_at")
                .and_then(|v| v.as_str())
                .map(String::from),
            failed_controls: Vec::new(),
        }
    }
}

/// Compare two RFC3339 created_at values (newest wins). Missing timestamps fall
/// back to "oldest", so an earlier list position (newest-first ordering) wins.
fn is_newer_than(candidate: &Option<String>, current: &Option<String>) -> bool {
    let ts = |s: &Option<String>| {
        s.as_ref()
            .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
            .map(|d| d.timestamp())
    };
    match (ts(candidate), ts(current)) {
        (Some(c), Some(k)) => c > k,
        (Some(_), None) => true,
        _ => false,
    }
}

/// Keep only the newest report per `profile_name`. Input may be in any order.
pub fn select_newest_per_profile(reports: &[ReportSummary]) -> Vec<ReportSummary> {
    use std::collections::HashMap;
    let mut newest: HashMap<String, ReportSummary> = HashMap::new();
    for r in reports {
        match newest.get(&r.profile_name) {
            Some(cur) => {
                if is_newer_than(&r.created_at, &cur.created_at) {
                    newest.insert(r.profile_name.clone(), r.clone());
                }
            }
            None => {
                newest.insert(r.profile_name.clone(), r.clone());
            }
        }
    }
    let mut out: Vec<ReportSummary> = newest.into_values().collect();
    out.sort_by(|a, b| a.profile_name.cmp(&b.profile_name));
    out
}

/// Aggregate per-profile reports into a single node-level compliance summary.
///
/// Node compliance is the WORST across profiles: non-compliant if ANY profile
/// has a failed control, `warn` if none failed but one warned, and `compliant`
/// only when every profile fully passed. Counts are summed and failed controls
/// concatenated across profiles.
pub fn aggregate_node_compliance(selected: &[ReportSummary]) -> NodeCompliance {
    let passed_count = selected.iter().map(|r| r.passed_count).sum();
    let failed_count = selected.iter().map(|r| r.failed_count).sum();
    let warning_count = selected.iter().map(|r| r.warning_count).sum();
    let report_id = selected
        .iter()
        .map(|r| r.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let profile_name = selected
        .iter()
        .map(|r| r.profile_name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let failed_controls: Vec<ControlRow> = selected
        .iter()
        .flat_map(|r| r.failed_controls.iter().cloned())
        .collect();
    let status = if failed_count > 0 {
        "non-compliant"
    } else if warning_count > 0 {
        "warn"
    } else {
        "compliant"
    }
    .to_string();
    NodeCompliance {
        report_id,
        profile_name,
        status,
        passed_count,
        failed_count,
        warning_count,
        failed_controls,
    }
}

/// Fetch all compliance reports for a node and aggregate them into the WORST
/// per-profile result.
///
/// Uses `/v1/compliance/reports?filter[node_id]=<id>` (newest-first), groups by
/// `profile_name`, keeps the newest report per profile, then aggregates across
/// profiles — so a node with a failed `fleet-services` scan is never masked by
/// a passing `linux-baseline` scan regardless of scan order. Report details for
/// each selected profile are resolved to surface the failed control ids. Any
/// upstream error other than 401/403 degrades gracefully to `None` so the node
/// page still renders if compliance is unavailable.
async fn node_compliance(
    st: &AppState,
    node_id: &str,
    token: &Option<String>,
) -> Result<Option<NodeCompliance>, ApiError> {
    let list: serde_json::Value = api_get(
        st,
        &format!("/v1/compliance/reports?filter%5Bnode_id%5D={node_id}&page_size=1000"),
        token,
    )
    .await?;
    let items: Vec<serde_json::Value> = list
        .get("data")
        .and_then(|d| d.get("items"))
        .and_then(|items| items.as_array())
        .cloned()
        .unwrap_or_default();
    if items.is_empty() {
        return Ok(None);
    }

    // Reduce to the newest report per profile.
    let summaries: Vec<ReportSummary> = items.iter().map(ReportSummary::from_value).collect();
    let selected = select_newest_per_profile(&summaries);

    // Resolve each selected report's detail to surface its failed control ids.
    let mut selected = selected;
    for rep in &mut selected {
        if rep.id.is_empty() {
            continue;
        }
        if let Ok(detail) =
            api_get::<serde_json::Value>(st, &format!("/v1/compliance/reports/{}", rep.id), token)
                .await
        {
            if let Some(results) = detail.get("control_results").and_then(|v| v.as_array()) {
                for r in results {
                    let cid = r.get("control_id").and_then(|v| v.as_str()).unwrap_or("");
                    let cstatus = r.get("status").and_then(|v| v.as_str()).unwrap_or("");
                    if cstatus == "failed" && !cid.is_empty() {
                        rep.failed_controls.push(ControlRow {
                            control_id: cid.to_string(),
                            status: cstatus.to_string(),
                        });
                    }
                }
            }
        }
    }

    Ok(Some(aggregate_node_compliance(&selected)))
}

pub async fn node_detail(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let token = extract_token(&headers);
    let nodes = match api_list::<NodeSummary>(&st, "/v1/nodes", &token).await {
        Ok(v) => v,
        Err(ApiError::Unauthorized(..)) => {
            return render(&LoginView {
                api_url: st.api_url,
            })
            .into_response()
        }
        Err(e) => {
            return render(&ErrorView {
                api_url: st.api_url,
                message: e.to_string(),
            })
            .into_response()
        }
    };
    // Resolve name → most recent summary (the API resolves detail by UUID).
    let summary = fleet_rows(&nodes)
        .into_iter()
        .find(|r| r.name.eq_ignore_ascii_case(&name));
    let summary = match summary {
        Some(s) => s,
        None => {
            return render(&ErrorView {
                api_url: st.api_url,
                message: format!("Node '{name}' not found"),
            })
            .into_response()
        }
    };
    let detail = match api_get::<NodeDetailEnvelope>(
        &st,
        &format!("/v1/nodes/{}", summary.id),
        &token,
    )
    .await
    {
        Ok(d) => d.data,
        Err(ApiError::Unauthorized(..)) => {
            return render(&LoginView {
                api_url: st.api_url,
            })
            .into_response()
        }
        Err(e) => {
            return render(&ErrorView {
                api_url: st.api_url,
                message: e.to_string(),
            })
            .into_response()
        }
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
    // Fetch the node's latest compliance summary; degrade gracefully so a
    // broken compliance upstream still renders the node page.
    let compliance = match node_compliance(&st, &summary.id, &token).await {
        Ok(c) => c,
        Err(ApiError::Unauthorized(..)) => {
            return render(&LoginView {
                api_url: st.api_url,
            })
            .into_response()
        }
        Err(_) => None,
    };
    let view = NodeView {
        api_url: st.api_url,
        page_name: "nodes",
        node: row,
        status: if detail.status.is_empty() {
            "—".to_string()
        } else {
            detail.status.clone()
        },
        attributes: info_rows(&detail.attributes),
        run_list: detail.run_list.clone(),
        compliance,
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
        Err(ApiError::Unauthorized(..)) => {
            return render(&LoginView {
                api_url: st.api_url,
            })
            .into_response()
        }
        Err(e) => {
            return render(&ErrorView {
                api_url: st.api_url,
                message: e.to_string(),
            })
            .into_response()
        }
    };
    render(&RunsView {
        api_url: st.api_url,
        page_name: "runs",
        runs: run_rows(&runs),
    })
    .into_response()
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
    let detail = match api_get::<RunDetailEnvelope>(&st, &format!("/v1/runs/{}", id), &token).await
    {
        Ok(d) => d.data,
        Err(ApiError::Unauthorized(..)) => {
            return render(&LoginView {
                api_url: st.api_url,
            })
            .into_response()
        }
        Err(e) => {
            return render(&ErrorView {
                api_url: st.api_url,
                message: e.to_string(),
            })
            .into_response()
        }
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

pub async fn compliance_list(State(st): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let token = extract_token(&headers);
    let list = match api_get::<serde_json::Value>(&st, "/v1/compliance/reports", &token).await {
        Ok(v) => v,
        Err(ApiError::Unauthorized(..)) => {
            return render(&LoginView {
                api_url: st.api_url,
            })
            .into_response()
        }
        Err(e) => {
            return render(&ErrorView {
                api_url: st.api_url,
                message: e.to_string(),
            })
            .into_response()
        }
    };
    let list: ComplianceList =
        match serde_json::from_value(list.get("data").cloned().unwrap_or_default()) {
            Ok(l) => l,
            Err(e) => {
                return render(&ErrorView {
                    api_url: st.api_url,
                    message: format!("compliance parse error: {e}"),
                })
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
    let v =
        match api_get::<serde_json::Value>(&st, &format!("/v1/compliance/reports/{}", id), &token)
            .await
        {
            Ok(v) => v,
            Err(ApiError::Unauthorized(..)) => {
                return render(&LoginView {
                    api_url: st.api_url,
                })
                .into_response()
            }
            Err(e) => {
                return render(&ErrorView {
                    api_url: st.api_url,
                    message: e.to_string(),
                })
                .into_response()
            }
        };
    let rows = info_rows(&v);
    let view = ComplianceDetailView {
        api_url: st.api_url,
        page_name: "compliance",
        report_id: id,
        rows,
    };
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

pub async fn cookbooks_list(State(st): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let token = extract_token(&headers);
    let books = match api_list::<CookbookInventoryEntry>(&st, "/v1/cookbooks", &token).await {
        Ok(v) => v,
        Err(ApiError::Unauthorized(..)) => {
            return render(&LoginView {
                api_url: st.api_url,
            })
            .into_response()
        }
        Err(e) => {
            return render(&ErrorView {
                api_url: st.api_url,
                message: e.to_string(),
            })
            .into_response()
        }
    };
    render(&CookbooksView {
        api_url: st.api_url,
        page_name: "cookbooks",
        books: cookbook_rows(&books),
    })
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
        Err(ApiError::Unauthorized(..)) => {
            return render(&LoginView {
                api_url: st.api_url,
            })
            .into_response()
        }
        Err(e) => {
            return render(&ErrorView {
                api_url: st.api_url,
                message: e.to_string(),
            })
            .into_response()
        }
    };
    let book = books
        .into_iter()
        .find(|b| b.name.as_deref() == Some(name.as_str()));
    let book = match book {
        Some(b) => b,
        None => {
            return render(&ErrorView {
                api_url: st.api_url,
                message: format!("Cookbook '{name}' not found"),
            })
            .into_response()
        }
    };
    let view = CookbookView {
        api_url: st.api_url,
        page_name: "cookbooks",
        name: book.name.clone().unwrap_or_else(|| name.clone()),
        total_nodes: book
            .total_nodes
            .map(|v| v.to_string())
            .unwrap_or("0".into()),
        last_seen: fmt_opt(&book.last_seen),
        versions: version_rows(&book.versions),
    };
    render(&view).into_response()
}

// ── Login ───────────────────────────────────────────────────────────────────

pub async fn login(State(st): State<AppState>) -> impl IntoResponse {
    render(&LoginView {
        api_url: st.api_url,
    })
    .into_response()
}

// ── Static assets (embedded at compile time — no runtime files needed) ──────

pub async fn static_asset(Path(path): Path<String>) -> impl IntoResponse {
    match path.as_str() {
        "style.css" => (
            [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
            STATIC_CSS,
        )
            .into_response(),
        "app.js" => (
            [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
            STATIC_JS,
        )
            .into_response(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn report(
        id: &str,
        profile: &str,
        passed: i64,
        failed: i64,
        warning: i64,
        created_at: &str,
    ) -> ReportSummary {
        ReportSummary {
            id: id.to_string(),
            profile_name: profile.to_string(),
            status: String::new(),
            passed_count: passed,
            failed_count: failed,
            warning_count: warning,
            created_at: Some(created_at.to_string()),
            failed_controls: Vec::new(),
        }
    }

    // (a) failed fleet-services + passed linux-baseline => non-compliant.
    // Simulates the scan-order flip from issue #47: linux-baseline is NEWEST
    // and passed, but fleet-services (older) failed — the node must NOT be
    // masked as compliant.
    #[test]
    fn worst_across_profiles_is_non_compliant() {
        let reports = vec![
            report("r1", "linux-baseline", 10, 0, 0, "2026-08-22T10:00:05Z"),
            report("r2", "fleet-services", 8, 2, 1, "2026-08-22T09:59:58Z"),
        ];
        let selected = select_newest_per_profile(&reports);
        // Newest per profile survives regardless of list order.
        assert_eq!(selected.len(), 2);
        let agg = aggregate_node_compliance(&selected);
        assert!(agg.is_non_compliant());
        assert!(!agg.is_compliant());
        assert_eq!(agg.compliance_class(), "bad");
        assert_eq!(agg.compliance_label(), "non-compliant");
        assert_eq!(agg.failed_count, 2);
        assert_eq!(agg.warning_count, 1);
    }

    // A passing linux-baseline must not absorb a failed fleet-services even
    // when the passing scan is the more recent one.
    #[test]
    fn passed_newest_plus_failed_older_still_non_compliant() {
        let reports = vec![
            report("r-p", "linux-baseline", 12, 0, 0, "2026-08-22T10:00:05Z"),
            report("r-f", "fleet-services", 8, 3, 0, "2026-08-22T09:58:10Z"),
            report("r-p2", "linux-baseline", 9, 1, 0, "2026-08-22T09:57:00Z"),
        ];
        let selected = select_newest_per_profile(&reports);
        // linux-baseline picks r-p (newest, passed); fleet-services picks r-f (failed).
        let lb = selected
            .iter()
            .find(|r| r.profile_name == "linux-baseline")
            .expect("linux-baseline present");
        assert_eq!(lb.id, "r-p", "newest linux-baseline must be chosen");
        let agg = aggregate_node_compliance(&selected);
        assert!(agg.is_non_compliant());
        assert_eq!(agg.failed_count, 3);
    }

    // (b) all profiles fully passed => compliant.
    #[test]
    fn all_passed_is_compliant() {
        let reports = vec![
            report("r1", "linux-baseline", 12, 0, 0, "2026-08-22T10:00:05Z"),
            report("r2", "fleet-services", 9, 0, 0, "2026-08-22T09:59:58Z"),
        ];
        let selected = select_newest_per_profile(&reports);
        let agg = aggregate_node_compliance(&selected);
        assert!(agg.is_compliant());
        assert!(!agg.is_non_compliant());
        assert!(!agg.is_warn());
        assert_eq!(agg.compliance_class(), "ok");
        assert_eq!(agg.compliance_label(), "compliant");
        assert_eq!(agg.passed_count, 21);
    }

    // (c) a report with warnings but no failures => warn, NOT compliant.
    #[test]
    fn warn_is_not_compliant() {
        let reports = vec![
            report("r1", "linux-baseline", 10, 0, 0, "2026-08-22T10:00:05Z"),
            report("r2", "fleet-services", 7, 0, 4, "2026-08-22T09:59:58Z"),
        ];
        let selected = select_newest_per_profile(&reports);
        let agg = aggregate_node_compliance(&selected);
        assert!(agg.is_warn());
        assert!(!agg.is_compliant(), "warn must not render as compliant");
        assert!(!agg.is_non_compliant());
        assert_eq!(agg.compliance_class(), "warn");
        assert_eq!(agg.compliance_label(), "warn");
        assert_eq!(agg.warning_count, 4);
        // A lone warn profile with no failures at all is also `warn`.
        let single = vec![report(
            "r3",
            "fleet-services",
            6,
            0,
            2,
            "2026-08-22T10:00:05Z",
        )];
        assert_eq!(
            aggregate_node_compliance(&single).compliance_label(),
            "warn"
        );
    }

    // Failed controls concatenate across the selected profiles.
    #[test]
    fn failed_controls_concatenated_across_profiles() {
        let mut fs = report("r2", "fleet-services", 8, 2, 0, "2026-08-22T09:59:58Z");
        fs.failed_controls.push(ControlRow {
            control_id: "svc-001".into(),
            status: "failed".into(),
        });
        let mut lb = report("r1", "linux-baseline", 10, 1, 0, "2026-08-22T10:00:05Z");
        lb.failed_controls.push(ControlRow {
            control_id: "os-100".into(),
            status: "failed".into(),
        });
        let agg = aggregate_node_compliance(&[lb, fs]);
        assert_eq!(agg.failed_controls.len(), 2);
        let ids: Vec<&str> = agg
            .failed_controls
            .iter()
            .map(|c| c.control_id.as_str())
            .collect();
        assert!(ids.contains(&"svc-001"));
        assert!(ids.contains(&"os-100"));
    }
}
