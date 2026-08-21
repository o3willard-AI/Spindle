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

#![allow(warnings)]
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

use spindle_api::{
    decode_cursor, encode_cursor, parse_pagination, parse_query_string, validate_filter_fields,
    VALID_COMPLIANCE_REPORT_FIELDS,
};
use spindle_store::ScopeFilter;
use spindle_store::{
    ComplianceReportsScopeFilter, ComplianceStore, ControlResult, ProfileStore, Scope,
    SqlxComplianceStore, SqlxProfileStore,
};
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
    pub impact: Option<f64>,
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
        let pages = if page_size == 0 {
            0
        } else {
            total.div_ceil(page_size)
        };
        Self {
            items,
            total,
            page,
            page_size,
            pages,
        }
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
    pub fn new(
        store: Arc<SqlxComplianceStore>,
        profile_store: Arc<SqlxProfileStore>,
        scope: Scope,
    ) -> Self {
        Self {
            store,
            profile_store,
            scope,
        }
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
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
    let scope = state.scope.clone();

    // Validate scope — no project scope means access denied
    if !scope.has_project("any") {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({
            "error": "access_denied",
            "message": "No project scope configured"
        }))).into_response();
    }

    // Parse the filter[] grammar from the raw query string (same as nodes).
    let raw_query = params.iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");

    let filter = match spindle_api::parse_query_string(&raw_query, spindle_api::VALID_COMPLIANCE_REPORT_FIELDS) {
        Ok(f) => f,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                "error": "bad_request",
                "message": format!("Invalid filter: {}", e)
            }))).into_response();
        }
    };

    if let Err(e) = spindle_api::validate_filter_fields(&filter.filters, &filter.time_range, spindle_api::VALID_COMPLIANCE_REPORT_FIELDS) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
            "error": "bad_request",
            "message": format!("Invalid field: {}", e)
        }))).into_response();
    }

    // Build WHERE conditions from parsed filters + bare query params (backward compat).
    // Supported filter fields: status, node_id, profile_name (via JOIN).
    let mut conditions: Vec<String> = Vec::new();
    let mut binds: Vec<String> = Vec::new();
    let mut bind_idx = 1u32;

    // Check filter[] params
    for f in &filter.filters {
        if let Some(ref val) = f.value {
            let val_str = val.to_string();
            match f.field.as_str() {
                "status" if f.operator == spindle_api::FilterOp::Eq => {
                    conditions.push(format!("cr.status = ${bind_idx}"));
                    binds.push(val_str);
                    bind_idx += 1;
                }
                "node_id" if f.operator == spindle_api::FilterOp::Eq => {
                    conditions.push(format!("cr.node_id = ${bind_idx}::uuid"));
                    binds.push(val_str);
                    bind_idx += 1;
                }
                "profile_name" if f.operator == spindle_api::FilterOp::Eq => {
                    conditions.push(format!("p.name = ${bind_idx}"));
                    binds.push(val_str);
                    bind_idx += 1;
                }
                _ => {}
            }
        }
    }

    // Apply time range filter (from ?since= / ?until= via parse_query_string)
    if let Some(ref start) = filter.time_range.start_time {
        conditions.push(format!("cr.created_at >= ${bind_idx}::timestamptz"));
        binds.push(start.to_rfc3339());
        bind_idx += 1;
    }
    if let Some(ref end) = filter.time_range.end_time {
        conditions.push(format!("cr.created_at <= ${bind_idx}::timestamptz"));
        binds.push(end.to_rfc3339());
        bind_idx += 1;
    }

    // Also support bare ?status=, ?node=, ?profile=, ?time_from=, ?time_to= for backward compat
    if let Some(status) = params.get("status") {
        if !conditions.iter().any(|c| c.contains("cr.status")) {
            conditions.push(format!("cr.status = ${bind_idx}"));
            binds.push(status.clone());
            bind_idx += 1;
        }
    }
    if let Some(node) = params.get("node") {
        if !conditions.iter().any(|c| c.contains("cr.node_id")) {
            conditions.push(format!("cr.node_id = ${bind_idx}::uuid"));
            binds.push(node.clone());
            bind_idx += 1;
        }
    }
    if let Some(profile) = params.get("profile") {
        if !conditions.iter().any(|c| c.contains("p.name")) {
            conditions.push(format!("p.name = ${bind_idx}"));
            binds.push(profile.clone());
            bind_idx += 1;
        }
    }
    // Bare ?time_from= and ?time_to= backward compat
    if let Some(tf) = params.get("time_from") {
        if !conditions.iter().any(|c| c.contains("cr.created_at >=")) {
            conditions.push(format!("cr.created_at >= ${bind_idx}::timestamptz"));
            binds.push(tf.clone());
            bind_idx += 1;
        }
    }
    if let Some(tt) = params.get("time_to") {
        if !conditions.iter().any(|c| c.contains("cr.created_at <=")) {
            conditions.push(format!("cr.created_at <= ${bind_idx}::timestamptz"));
            binds.push(tt.clone());
            bind_idx += 1;
        }
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    // Parse pagination params (cursor-based, matching nodes.rs / runs.rs)
    let pagination = match parse_pagination(&raw_query, "created_at") {
        Ok(p) => p,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                "error": "bad_request",
                "message": format!("Invalid pagination: {}", e)
            }))).into_response();
        }
    };

    // Validate cursor — return 400 if the cursor is present but malformed
    if let Some(ref cursor) = pagination.cursor {
        if decode_cursor(cursor).is_none() {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                "error": "bad_request",
                "message": "Invalid cursor format"
            }))).into_response();
        }
    }

    // Query the database for compliance reports joined with profiles.
    // Fetch all matching rows ordered deterministically; cursor-based slicing
    // is applied in-memory (same pattern as nodes.rs / cookbooks.rs).
    let query_sql = format!(
        r#"
        SELECT
            cr.id, cr.run_id, cr.node_id, cr.profile_id, p.name as profile_name,
            cr.status, cr.passed_count, cr.failed_count, cr.warning_count,
            cr.created_at
        FROM compliance_reports cr
        JOIN profiles p ON cr.profile_id = p.id
        {}
        ORDER BY cr.created_at
        "#,
        where_clause,
    );

    // Run a separate COUNT(*) for the true total
    let count_sql = format!(
        r#"
        SELECT COUNT(*) as cnt
        FROM compliance_reports cr
        JOIN profiles p ON cr.profile_id = p.id
        {}
        "#,
        where_clause,
    );

    let pool = state.store.pg().pool();

    // Execute count query with the same binds
    let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql);
    for b in &binds {
        count_query = count_query.bind(b);
    }
    let total: u64 = count_query
        .fetch_one(pool)
        .await
        .unwrap_or(0) as u64;

    // Execute data query with binds (no LIMIT/OFFSET — cursor slicing is in-memory)
    let mut data_query = sqlx::query(&query_sql);
    for b in &binds {
        data_query = data_query.bind(b);
    }

    let rows: Vec<Value> = data_query
        .fetch_all(pool)
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

    // Apply cursor-based pagination in-memory (matches nodes.rs pattern)
    // Sort direction: compliance reports default to DESC (newest first)
    let sort_desc = pagination.sort_direction == "desc";
    let mut rows = rows;
    if sort_desc {
        rows.sort_by(|a, b| {
            b["created_at"].as_str()
                .unwrap_or("")
                .cmp(a["created_at"].as_str().unwrap_or(""))
        });
    } else {
        rows.sort_by(|a, b| {
            a["created_at"].as_str()
                .unwrap_or("")
                .cmp(b["created_at"].as_str().unwrap_or(""))
        });
    }

    let total_count = rows.len();
    let limit = pagination.limit;

    // Determine start index from cursor
    let start_idx = if let Some(ref cursor) = pagination.cursor {
        match decode_cursor(cursor) {
            Some((_sort_val, cursor_id, _direction)) => {
                match rows.iter().position(|r| {
                    r["id"].as_str().and_then(|s| Uuid::parse_str(s).ok()) == Some(cursor_id)
                }) {
                    Some(idx) => idx + 1,
                    None => {
                        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                            "error": "bad_request",
                            "message": "Cursor references a report that is not in the current result set"
                        }))).into_response();
                    }
                }
            }
            None => {
                return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                    "error": "bad_request",
                    "message": "Invalid cursor format"
                }))).into_response();
            }
        }
    } else {
        0
    };

    let end_idx = (start_idx + limit).min(total_count);
    let page_items: Vec<Value> = rows[start_idx..end_idx].to_vec();
    let has_more = end_idx < total_count;

    let next_cursor = if has_more && !page_items.is_empty() {
        let last = &page_items[page_items.len() - 1];
        let last_id = last["id"].as_str().and_then(|s| Uuid::parse_str(s).ok()).unwrap_or_else(Uuid::nil);
        let cursor_val = last["created_at"].as_str().unwrap_or("").to_string();
        Some(encode_cursor(&cursor_val, last_id, &pagination.sort_direction))
    } else {
        None
    };

    Json(serde_json::json!({
        "data": {
            "items": page_items,
            "total": total,
            "total_count": total_count as u64,
            "limit": limit,
            "has_more": has_more,
            "next_cursor": next_cursor,
        },
        "filters": {
            "status": params.get("status").cloned(),
            "node": params.get("node").cloned(),
            "profile": params.get("profile").cloned(),
        }
    })).into_response()
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
    let report = state
        .store
        .get_report(report_id, &scope)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Fetch control results for this report
    let control_results = state
        .store
        .get_control_results(report_id, &scope)
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
        if let Some(results) = response
            .get_mut("control_results")
            .and_then(|v| v.as_array_mut())
        {
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
#[utoipa::path(
    get,
    path = "/v1/compliance/controls",
    tag = "compliance",
    responses(
        (status = 200, description = "List of control results", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Access denied"),
    ),
)]
pub async fn list_controls(
    State(state): State<ComplianceState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
    let scope = state.scope.clone();

    if !scope.has_project("any") {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({
            "error": "access_denied",
            "message": "No project scope configured"
        }))).into_response();
    }

    // Parse filter[] grammar
    let raw_query = params.iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");

    // Valid fields for control_results filtering
    let valid_fields = &["id", "control_id", "status", "impact", "profile_id", "node_id"];

    let filter = match spindle_api::parse_query_string(&raw_query, valid_fields) {
        Ok(f) => f,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                "error": "bad_request",
                "message": format!("Invalid filter: {}", e)
            }))).into_response();
        }
    };

    if let Err(e) = spindle_api::validate_filter_fields(&filter.filters, &filter.time_range, valid_fields) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
            "error": "bad_request",
            "message": format!("Invalid field: {}", e)
        }))).into_response();
    }

    // Build WHERE conditions
    let mut conditions: Vec<String> = Vec::new();
    let mut binds: Vec<String> = Vec::new();
    let mut bind_idx = 1u32;

    for f in &filter.filters {
        if let Some(ref val) = f.value {
            let val_str = val.to_string();
            match f.field.as_str() {
                "control_id" if f.operator == spindle_api::FilterOp::Eq => {
                    conditions.push(format!("control_id = ${bind_idx}"));
                    binds.push(val_str);
                    bind_idx += 1;
                }
                "status" if f.operator == spindle_api::FilterOp::Eq => {
                    conditions.push(format!("status = ${bind_idx}"));
                    binds.push(val_str);
                    bind_idx += 1;
                }
                "impact" if f.operator == spindle_api::FilterOp::Eq => {
                    conditions.push(format!("impact = ${bind_idx}"));
                    binds.push(val_str);
                    bind_idx += 1;
                }
                "node_id" if f.operator == spindle_api::FilterOp::Eq => {
                    conditions.push(format!("node_id = ${bind_idx}::uuid"));
                    binds.push(val_str);
                    bind_idx += 1;
                }
                "profile_id" if f.operator == spindle_api::FilterOp::Eq => {
                    conditions.push(format!("profile_id = ${bind_idx}::uuid"));
                    binds.push(val_str);
                    bind_idx += 1;
                }
                _ => {}
            }
        }
    }

    // Backward compat: bare ?control_id=, ?status=, ?impact=
    if let Some(v) = params.get("control_id") {
        if !conditions.iter().any(|c| c.contains("control_id")) {
            conditions.push(format!("control_id = ${bind_idx}"));
            binds.push(v.clone());
            bind_idx += 1;
        }
    }
    if let Some(v) = params.get("status") {
        if !conditions.iter().any(|c| c.starts_with("status")) {
            conditions.push(format!("status = ${bind_idx}"));
            binds.push(v.clone());
            bind_idx += 1;
        }
    }
    if let Some(v) = params.get("impact") {
        if !conditions.iter().any(|c| c.contains("impact")) {
            conditions.push(format!("impact = ${bind_idx}"));
            binds.push(v.clone());
            bind_idx += 1;
        }
    }

    // Apply time range filter (from ?since= / ?until= via parse_query_string)
    if let Some(ref start) = filter.time_range.start_time {
        conditions.push(format!("created_at >= ${bind_idx}::timestamptz"));
        binds.push(start.to_rfc3339());
        bind_idx += 1;
    }
    if let Some(ref end) = filter.time_range.end_time {
        conditions.push(format!("created_at <= ${bind_idx}::timestamptz"));
        binds.push(end.to_rfc3339());
        bind_idx += 1;
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let page: u64 = params.get("page").and_then(|v| v.parse().ok()).unwrap_or(1);
    let page_size: u64 = params.get("page_size").and_then(|v| v.parse().ok()).unwrap_or(50);

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
        where_clause,
        page_size,
        (page - 1) * page_size,
    );

    let count_sql = format!(
        "SELECT COUNT(*) FROM control_results {}",
        where_clause,
    );

    let pool = state.store.pg().pool();

    // Count query
    let mut count_q = sqlx::query_scalar::<_, i64>(&count_sql);
    for b in &binds {
        count_q = count_q.bind(b);
    }
    let total: u64 = count_q.fetch_one(pool).await.unwrap_or(0) as u64;

    // Data query
    let mut data_q = sqlx::query(&query_sql);
    for b in &binds {
        data_q = data_q.bind(b);
    }

    let rows: Vec<Value> = data_q
        .fetch_all(pool)
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
                        "impact": row.get::<Option<f64>, _>("impact"),
                        "result": row.get::<Option<serde_json::Value>, _>("result"),
                        "created_at": row.get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Json(serde_json::json!({
        "data": {
            "items": rows,
            "total": total,
            "page": page,
            "page_size": page_size,
            "pages": if page_size == 0 { 0 } else { total.div_ceil(page_size) },
        },
        "filters": {
            "control_id": params.get("control_id").cloned(),
            "status": params.get("status").cloned(),
            "impact": params.get("impact").cloned(),
        }
    })).into_response()
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
        let last_report: Option<chrono::DateTime<chrono::Utc>> =
            row.try_get("last_report").ok().flatten();

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
    let profile = state
        .profile_store
        .get_profile(profile_id, &scope)
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
        let last_evaluated: Option<chrono::DateTime<chrono::Utc>> =
            row.try_get("last_evaluated").ok().flatten();

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
        .route(
            "/v1/compliance/nodes/:id/status",
            get(get_node_compliance_status),
        )
        .route(
            "/v1/compliance/profiles/:id/status",
            get(get_profile_compliance_status),
        )
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
        // page/page_size are legacy fields retained on the struct for type
        // compatibility; the handler now uses cursor-based pagination via
        // parse_pagination(). These defaults remain unchanged.
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
            impact: 0.7,
            result: Some(serde_json::json!({"expected": "true", "actual": "true"})),
            created_at: chrono::Utc::now(),
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("control_id"));
        assert!(json.contains("status"));
        assert!(json.contains("impact"));
    }
}
