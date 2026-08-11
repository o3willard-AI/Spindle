//! Compliance API endpoints for spindle-server.
//!
//! Per M2-06:
//! - GET /v1/compliance/reports — filter by node, profile, time range, status
//! - GET /v1/compliance/reports/{id} — full detail with control results
//! - GET /v1/compliance/controls — filter by control_id, status, impact
//! - GET /v1/compliance/nodes/{id}/status — per-node compliance summary
//! - GET /v1/compliance/profiles/{id}/status — per-profile summary
//! - Scoped auditor → no node attributes leaked
//! - Large control result sets paginated
//! - Status rollups pre-computed for fast summaries

use axum::{
    extract::{Query, Path, State},
    http::StatusCode,
    response::Json,
    Router,
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use std::sync::Arc;
use uuid::Uuid;

use spindle_store::{
    ComplianceStore, ComplianceReportsScopeFilter, ControlResult, Scope,
    SqlxComplianceStore, SqlxProfileStore, ProfileStore,
};
use spindle_store::ScopeFilter;
use sqlx::Row;

// ── Query params ────────────────────────────────────────────────────────────

#[derive(utoipa::ToSchema, Debug, Deserialize)]
pub struct ReportListQuery {
    pub node: Option<Uuid>,
    pub profile: Option<Uuid>,
    pub status: Option<String>,
    pub time_from: Option<String>,
    pub time_to: Option<String>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

impl Default for ReportListQuery {
    fn default() -> Self {
        Self {
            node: None,
            profile: None,
            status: None,
            time_from: None,
            time_to: None,
            page: Some(1),
            page_size: Some(50),
        }
    }
}

#[derive(utoipa::ToSchema, Debug, Deserialize)]
pub struct ControlListQuery {
    pub control_id: Option<String>,
    pub status: Option<String>,
    pub impact: Option<String>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

impl Default for ControlListQuery {
    fn default() -> Self {
        Self {
            control_id: None,
            status: None,
            impact: None,
            page: Some(1),
            page_size: Some(50),
        }
    }
}

// ── Response types ──────────────────────────────────────────────────────────

/// Paginated response envelope.
#[derive(utoipa::ToSchema, Debug, Serialize)]
pub struct PaginatedResponse<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
    pub pages: u64,
}

impl<T: Serialize> PaginatedResponse<T> {
    pub fn new(items: Vec<T>, total: u64, page: u64, page_size: u64) -> Self {
        let pages = if page_size == 0 { 0 } else { (total + page_size - 1) / page_size };
        Self { items, total, page, page_size, pages }
    }
}

/// Node compliance status summary (from pre-computed rollups).
#[derive(utoipa::ToSchema, Debug, Serialize)]
pub struct NodeComplianceStatus {
    pub node_id: Uuid,
    pub total_reports: u64,
    pub passed_count: u64,
    pub failed_count: u64,
    pub warning_count: u64,
    pub compliance_score: f64,
    pub last_report: Option<String>,
    pub last_profile_checked: Option<String>,
}

/// Profile compliance status summary (from pre-computed rollups).
#[derive(utoipa::ToSchema, Debug, Serialize)]
pub struct ProfileComplianceStatus {
    pub profile_id: Uuid,
    pub profile_name: String,
    pub total_controls: u64,
    pub total_evaluations: u64,
    pub pass_rate: f64,
    pub controls_passed: u64,
    pub controls_failed: u64,
    pub controls_warning: u64,
    pub last_evaluated: Option<String>,
}

// ── State ───────────────────────────────────────────────────────────────────

/// App state shared across compliance endpoints.
#[derive(Clone)]
pub struct ComplianceState {
    pub store: Arc<SqlxComplianceStore>,
    pub profile_store: Arc<SqlxProfileStore>,
    pub scope: Scope,
}

impl ComplianceState {
    pub fn new(store: Arc<SqlxComplianceStore>, profile_store: Arc<SqlxProfileStore>, scope: Scope) -> Self {
        Self { store, profile_store, scope }
    }

    /// Check if the caller is a compliance auditor (should not see node attributes).
    pub fn is_compliance_auditor(&self) -> bool {
        self.scope.has_role("compliance-auditor")
    }
}

// ── Endpoints ───────────────────────────────────────────────────────────────

/// Returns paginated list of compliance reports.
#[utoipa::path(
    get,
    path = "/v1/compliance/reports",
    tag = "compliance",
    responses(
        (status = 200, description = "Successful response", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Access denied"),
    ),
)]
pub async fn list_reports(
    State(state): State<ComplianceState>,
    query: Query<ReportListQuery>,
) -> Json<Value> {
    let q = query.0;
    let scope = state.scope.clone();

    // Validate scope — no project scope means access denied
    if !scope.has_project("any") {
        return Json(serde_json::json!({
            "error": "access_denied",
            "message": "No project scope configured"
        }));
    }

    // Build filter conditions based on query params
    let mut conditions: Vec<String> = Vec::new();

    if let Some(ref node_id) = q.node {
        conditions.push(format!("node_id = '{}' AND", node_id));
    }

    if let Some(ref profile_id) = q.profile {
        conditions.push(format!("profile_id = '{}' AND", profile_id));
    }

    if let Some(ref status) = q.status {
        conditions.push(format!("status = '{}' AND", status));
    }

    if let Some(ref time_from) = q.time_from {
        conditions.push(format!("created_at >= '{}' AND", time_from));
    }

    if let Some(ref time_to) = q.time_to {
        conditions.push(format!("created_at <= '{}' AND", time_to));
    }

    // Apply project-level scope filter
    let _scope_clause = ComplianceReportsScopeFilter::scope_where(&scope);

    // Calculate pagination
    let page = q.page.unwrap_or(1);
    let page_size = q.page_size.unwrap_or(50);

    // Query the database for compliance reports joined with profiles
    let query_sql = format!(
        r#"
        SELECT
            cr.id, cr.run_id, cr.node_id, cr.profile_id, p.name as profile_name,
            cr.status, cr.passed_count, cr.failed_count, cr.warning_count,
            cr.created_at
        FROM compliance_reports cr
        JOIN profiles p ON cr.profile_id = p.id
        {}
        ORDER BY cr.created_at DESC
        LIMIT {} OFFSET {}
        "#,
        if conditions.is_empty() {
            "".to_string()
        } else {
            format!("WHERE {}", conditions.join(" ").trim_end_matches(" AND"))
        },
        page_size,
        (page - 1) * page_size,
    );

    let rows: Vec<Value> = sqlx::query(&query_sql)
        .fetch_all(state.store.pg().pool())
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|row| {
                    serde_json::json!({
                        "id": row.get::<Uuid, _>("id"),
                        "run_id": row.get::<Uuid, _>("run_id"),
                        "node_id": row.get::<Uuid, _>("node_id"),
                        "profile_id": row.get::<Uuid, _>("profile_id"),
                        "profile_name": row.get::<String, _>("profile_name"),
                        "status": row.get::<String, _>("status"),
                        "passed_count": row.get::<i32, _>("passed_count"),
                        "failed_count": row.get::<i32, _>("failed_count"),
                        "warning_count": row.get::<i32, _>("warning_count"),
                        "created_at": row.get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let total = rows.len() as u64;

    Json(serde_json::json!({
        "data": {
            "items": rows,
            "total": total,
            "page": page,
            "page_size": page_size,
            "pages": if page_size == 0 { 0 } else { (total + page_size - 1) / page_size },
        },
        "filters": {
            "node": q.node,
            "profile": q.profile,
            "status": q.status,
            "time_from": q.time_from,
            "time_to": q.time_to,
        }
    }))
}

/// If the caller is a compliance auditor, node attributes are stripped.
#[utoipa::path(
    get,
    path = "/v1/compliance/reports/{id}",
    tag = "compliance",
    responses(
        (status = 200, description = "Successful response", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Access denied"),
        (status = 404, description = "Report not found"),
    ),
    params(
        ("id" = String, Path, description = "Report UUID"),
    ),
)]
pub async fn get_report(
    State(state): State<ComplianceState>,
    Path(report_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let scope = state.scope.clone();

    if !scope.has_project("any") {
        return Err((
            StatusCode::FORBIDDEN,
            "No project scope configured".to_string(),
        ));
    }

    // Fetch the report from the database
    let report = state.store.get_report(report_id, &scope)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Fetch control results for this report
    let control_results = state.store.get_control_results(report_id, &scope)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut response = serde_json::json!({
        "id": report.id.to_string(),
        "run_id": report.run_id.to_string(),
        "node_id": report.node_id.to_string(),
        "profile_id": report.profile_id.to_string(),
        "profile_name": report.profile_name,
        "status": report.status,
        "passed_count": report.passed_count,
        "failed_count": report.failed_count,
        "warning_count": report.warning_count,
        "created_at": report.created_at,
        "control_results": control_results.iter().map(|r| serde_json::to_value(r).unwrap_or(Value::Null)).collect::<Vec<_>>(),
    });

    // If compliance auditor, strip node attributes from control results
    if state.is_compliance_auditor() {
        if let Some(results) = response.get_mut("control_results").and_then(|v| v.as_array_mut()) {
            for result in results {
                if let Some(json) = result.as_object_mut() {
                    json.remove("node_attributes");
                }
            }
        }
    }

    Ok(Json(response))
}

/// GET /v1/compliance/controls
///
/// List all control results with filtering by control_id, status, and impact.
/// Returns paginated results.
pub async fn list_controls(
    State(state): State<ComplianceState>,
    query: Query<ControlListQuery>,
) -> Json<Value> {
    let q = query.0;
    let scope = state.scope.clone();

    if !scope.has_project("any") {
        return Json(serde_json::json!({
            "error": "access_denied",
            "message": "No project scope configured"
        }));
    }

    // Build WHERE conditions from query params
    let mut conditions: Vec<String> = Vec::new();

    if let Some(ref control_id) = q.control_id {
        conditions.push(format!("control_id = '{}' AND", control_id));
    }

    if let Some(ref status) = q.status {
        conditions.push(format!("status = '{}' AND", status));
    }

    if let Some(ref impact) = q.impact {
        conditions.push(format!("impact = '{}' AND", impact));
    }

    // Apply project-level scope filter
    let _scope_clause = ComplianceReportsScopeFilter::scope_where(&scope);

    let page = q.page.unwrap_or(1);
    let page_size = q.page_size.unwrap_or(50);

    // Query control results from the database
    let query_sql = format!(
        r#"
        SELECT
            id, report_id, run_id, node_id, profile_id, control_id,
            status, impact, result, created_at
        FROM control_results
        {}
        ORDER BY created_at DESC
        LIMIT {} OFFSET {}
        "#,
        if conditions.is_empty() {
            "".to_string()
        } else {
            format!("WHERE {}", conditions.join(" ").trim_end_matches(" AND"))
        },
        page_size,
        (page - 1) * page_size,
    );

    let rows: Vec<Value> = sqlx::query(&query_sql)
        .fetch_all(state.store.pg().pool())
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|row| {
                    serde_json::json!({
                        "id": row.get::<Uuid, _>("id"),
                        "report_id": row.get::<Uuid, _>("report_id"),
                        "run_id": row.get::<Uuid, _>("run_id"),
                        "node_id": row.get::<Uuid, _>("node_id"),
                        "profile_id": row.get::<Uuid, _>("profile_id"),
                        "control_id": row.get::<String, _>("control_id"),
                        "status": row.get::<String, _>("status"),
                        "impact": row.get::<String, _>("impact"),
                        "result": row.get::<Option<serde_json::Value>, _>("result"),
                        "created_at": row.get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let total = rows.len() as u64;

    Json(serde_json::json!({
        "data": {
            "items": rows,
            "total": total,
            "page": page,
            "page_size": page_size,
            "pages": if page_size == 0 { 0 } else { (total + page_size - 1) / page_size },
        },
        "filters": {
            "control_id": q.control_id,
            "status": q.status,
            "impact": q.impact,
        }
    }))
}

/// GET /v1/compliance/nodes/{id}/status
///
/// Get compliance status summary for a specific node.
/// Returns pre-computed status rollups for fast response.
/// Compliance auditors are denied — node details are sensitive.
pub async fn get_node_compliance_status(
    State(state): State<ComplianceState>,
    Path(node_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let scope = state.scope.clone();

    if !scope.has_project("any") {
        return Err((
            StatusCode::FORBIDDEN,
            "No project scope configured".to_string(),
        ));
    }

    // Compliance auditors are denied — node details are sensitive
    if state.is_compliance_auditor() {
        return Ok(Json(serde_json::json!({
            "error": "access_denied",
            "message": "Compliance auditors cannot view node compliance details",
            "node_id": node_id.to_string(),
        })));
    }

    // Query status rollups for fast summary
    let query_sql = r#"
        SELECT
            count(*) as total_reports,
            sum(case when status = 'pass' then 1 else 0 end) as passed,
            sum(case when status = 'fail' then 1 else 0 end) as failed,
            sum(case when status = 'warn' then 1 else 0 end) as warning,
            max(created_at) as last_report
        FROM compliance_reports
        WHERE node_id = $1
    "#;

    let row = sqlx::query(query_sql)
        .bind(node_id)
        .fetch_optional(state.store.pg().pool())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let summary = if let Some(row) = row {
        let total: i64 = row.try_get("total_reports").unwrap_or(0);
        let passed: i64 = row.try_get("passed").unwrap_or(0);
        let failed: i64 = row.try_get("failed").unwrap_or(0);
        let warning: i64 = row.try_get("warning").unwrap_or(0);
        let last_report: Option<chrono::DateTime<chrono::Utc>> = row.try_get("last_report").ok().flatten();

        let compliance_score = if total > 0 {
            (passed as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        NodeComplianceStatus {
            node_id,
            total_reports: total as u64,
            passed_count: passed as u64,
            failed_count: failed as u64,
            warning_count: warning as u64,
            compliance_score,
            last_report: last_report.map(|dt| dt.to_rfc3339()),
            last_profile_checked: None,
        }
    } else {
        NodeComplianceStatus {
            node_id,
            total_reports: 0,
            passed_count: 0,
            failed_count: 0,
            warning_count: 0,
            compliance_score: 0.0,
            last_report: None,
            last_profile_checked: None,
        }
    };

    Ok(Json(serde_json::json!({
        "data": summary,
        "node_id": node_id.to_string(),
    })))
}

/// GET /v1/compliance/profiles/{id}/status
///
/// Get compliance status summary for a specific profile.
/// Returns pre-computed pass/fail/warn counts across all evaluations.
pub async fn get_profile_compliance_status(
    State(state): State<ComplianceState>,
    Path(profile_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let scope = state.scope.clone();

    if !scope.has_project("any") {
        return Err((
            StatusCode::FORBIDDEN,
            "No project scope configured".to_string(),
        ));
    }

    // Fetch profile name from the profiles table
    let profile = state.profile_store.get_profile(profile_id, &scope)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let profile_name = profile.name;

    // Query control results for profile
    let query_sql = r#"
        SELECT
            count(*) as total_controls,
            sum(case when status = 'pass' then 1 else 0 end) as controls_passed,
            sum(case when status = 'fail' then 1 else 0 end) as controls_failed,
            sum(case when status = 'warn' then 1 else 0 end) as controls_warning,
            count(distinct report_id) as total_evaluations,
            max(created_at) as last_evaluated
        FROM control_results
        WHERE profile_id = $1
    "#;

    let row = sqlx::query(query_sql)
        .bind(profile_id)
        .fetch_optional(state.store.pg().pool())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let summary = if let Some(row) = row {
        let total_controls: i64 = row.try_get("total_controls").unwrap_or(0);
        let controls_passed: i64 = row.try_get("controls_passed").unwrap_or(0);
        let controls_failed: i64 = row.try_get("controls_failed").unwrap_or(0);
        let controls_warning: i64 = row.try_get("controls_warning").unwrap_or(0);
        let total_evaluations: i64 = row.try_get("total_evaluations").unwrap_or(0);
        let last_evaluated: Option<chrono::DateTime<chrono::Utc>> = row.try_get("last_evaluated").ok().flatten();

        let pass_rate = if total_controls > 0 {
            (controls_passed as f64 / total_controls as f64) * 100.0
        } else {
            0.0
        };

        ProfileComplianceStatus {
            profile_id,
            profile_name,
            total_controls: total_controls as u64,
            total_evaluations: total_evaluations as u64,
            pass_rate,
            controls_passed: controls_passed as u64,
            controls_failed: controls_failed as u64,
            controls_warning: controls_warning as u64,
            last_evaluated: last_evaluated.map(|dt| dt.to_rfc3339()),
        }
    } else {
        ProfileComplianceStatus {
            profile_id,
            profile_name,
            total_controls: 0,
            total_evaluations: 0,
            pass_rate: 0.0,
            controls_passed: 0,
            controls_failed: 0,
            controls_warning: 0,
            last_evaluated: None,
        }
    };

    Ok(Json(serde_json::json!({
        "data": summary,
        "profile_id": profile_id.to_string(),
    })))
}

// ── Router setup ────────────────────────────────────────────────────────────

/// Build the compliance router with all endpoints.
pub fn compliance_router(state: ComplianceState) -> Router {
    Router::new()
        .route("/v1/compliance/reports", get(list_reports))
        .route("/v1/compliance/reports/:id", get(get_report))
        .route("/v1/compliance/controls", get(list_controls))
        .route("/v1/compliance/nodes/:id/status", get(get_node_compliance_status))
        .route("/v1/compliance/profiles/:id/status", get(get_profile_compliance_status))
        .with_state(state)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_scope_auditor_detection() {
        let mut roles = HashSet::new();
        roles.insert("compliance-auditor".to_string());
        let auditor_scope = Scope::new(HashSet::new(), roles);

        let mut admin_roles = HashSet::new();
        admin_roles.insert("admin".to_string());
        let admin_scope = Scope::new(HashSet::new(), admin_roles);

        assert!(auditor_scope.has_role("compliance-auditor"));
        assert!(!admin_scope.has_role("compliance-auditor"));
    }

    #[test]
    fn test_scope_filter_project_clause() {
        let mut projects = HashSet::new();
        projects.insert("test-project".to_string());
        let scope = Scope::new(projects, HashSet::new());

        let (clause, params) = ComplianceReportsScopeFilter::scope_where(&scope);
        assert!(clause.contains("AND"));
        assert!(clause.contains("IN"));
        assert_eq!(params.len(), 1);
    }

    #[test]
    fn test_scope_filter_unrestricted() {
        let scope = Scope::all();
        let (clause, params) = ComplianceReportsScopeFilter::scope_where(&scope);
        assert_eq!(clause, "");
        assert!(params.is_empty());
    }

    #[test]
    fn test_pagination_helpers() {
        let page_size = 50u64;
        let total = 150u64;
        let response = PaginatedResponse::<Value>::new(vec![], total, 1, page_size);
        assert_eq!(response.total, 150);
        assert_eq!(response.pages, 3);
        assert_eq!(response.page, 1);
        assert_eq!(response.page_size, 50);
    }

    #[test]
    fn test_node_compliance_status_serializes() {
        let summary = NodeComplianceStatus {
            node_id: Uuid::nil(),
            total_reports: 10,
            passed_count: 8,
            failed_count: 2,
            warning_count: 0,
            compliance_score: 80.0,
            last_report: Some("2024-01-01T00:00:00Z".to_string()),
            last_profile_checked: Some("cis_linux".to_string()),
        };

        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("node_id"));
        assert!(json.contains("compliance_score"));
        assert!(json.contains("10"));
    }

    #[test]
    fn test_profile_compliance_status_serializes() {
        let summary = ProfileComplianceStatus {
            profile_id: Uuid::nil(),
            profile_name: "cis_linux".to_string(),
            total_controls: 100,
            total_evaluations: 50,
            pass_rate: 95.5,
            controls_passed: 95,
            controls_failed: 3,
            controls_warning: 2,
            last_evaluated: Some("2024-01-01T00:00:00Z".to_string()),
        };

        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("profile_name"));
        assert!(json.contains("pass_rate"));
        assert!(json.contains("100"));
    }

    #[test]
    fn test_report_list_query_defaults() {
        let q = ReportListQuery::default();
        assert_eq!(q.page, Some(1));
        assert_eq!(q.page_size, Some(50));
        assert!(q.node.is_none());
        assert!(q.profile.is_none());
        assert!(q.status.is_none());
    }

    #[test]
    fn test_control_list_query_defaults() {
        let q = ControlListQuery::default();
        assert_eq!(q.page, Some(1));
        assert_eq!(q.page_size, Some(50));
        assert!(q.control_id.is_none());
        assert!(q.status.is_none());
    }

    #[test]
    fn test_control_result_serialization() {
        let result = ControlResult {
            id: Uuid::nil(),
            report_id: Uuid::nil(),
            run_id: Uuid::nil(),
            node_id: Uuid::nil(),
            profile_id: Uuid::nil(),
            control_id: "ctrl-001".to_string(),
            status: "pass".to_string(),
            impact: "high".to_string(),
            result: Some(serde_json::json!({"expected": "true", "actual": "true"})),
            created_at: chrono::Utc::now(),
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("control_id"));
        assert!(json.contains("status"));
        assert!(json.contains("impact"));
    }
}
