//! UI aggregate endpoints: fleet summary + daily trend charts.
//!
//! ## Endpoints
//! - `GET /v1/summary` — fleet rollup:
//!   `{total, online, offline, convergeSuccess, convergeFailed, compliant,
//!    nonCompliant, unknownCompliance, flipped:[{id,name}]}`
//!   - online/offline mirror the dashboard's rule: `last_seen` within the
//!     last 300 seconds ⇒ online; anything else (including NULL) ⇒ offline.
//!   - convergeSuccess/convergeFailed count runs by `status`
//!     ('success' / 'failed'); other statuses are ignored.
//!   - compliant/nonCompliant classify each node by its LATEST compliance
//!     report status ('passed' / 'failed'). Every remaining node (no
//!     reports at all, or a latest status outside passed/failed) falls into
//!     `unknownCompliance`.
//!   - flipped = nodes whose LATEST report failed while their PENULTIMATE
//!     report passed (last two reports per node) — recently-regressed nodes
//!     worth surfacing at the top of a dashboard.
//!
//! - `GET /v1/compliance/trend?days=14` — daily buckets
//!   `{date, passRate, passed, failed}` (passRate = passed/(passed+failed)*100).
//! - `GET /v1/runs/trend?days=7` — daily buckets `{date, success, failed}`
//!   bucketed on `COALESCE(start_time, created_at)`.
//!
//! Trend windows default to 14 (compliance) / 7 (runs) days and are clamped
//! to 1..=365; invalid values are rejected with 400. All queries are
//! parameterized (`make_interval(days => $n)`), never string-interpolated.
//!
//! When no database pool is available (dev mode) the endpoints degrade
//! gracefully: summary returns all zeros with an empty `flipped` list and
//! trends return empty arrays — same behavior as resource_events.

#![allow(warnings)]
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use utoipa::ToSchema;

use crate::metrics::MetricsRegistry;

/// Seconds within which a node's `last_seen` must fall to count as online.
/// Mirrors the dashboard's node-list rule so both surfaces agree.
const ONLINE_THRESHOLD_SECS: i64 = 300;

/// App state for the UI aggregate endpoints.
#[derive(Clone)]
pub struct UiAppState {
    pub db_pool: Option<sqlx::PgPool>,
    pub metrics: Arc<MetricsRegistry>,
}

impl UiAppState {
    pub fn new(db_pool: Option<sqlx::PgPool>, metrics: Arc<MetricsRegistry>) -> Self {
        Self { db_pool, metrics }
    }
}

/// Build the UI router with all aggregate endpoints.
pub fn ui_routes(state: UiAppState) -> Router {
    Router::new()
        .route("/v1/summary", get(summary))
        .route("/v1/compliance/trend", get(compliance_trend))
        .route("/v1/runs/trend", get(runs_trend))
        .with_state(state)
}

// ── Response models ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FlippedNode {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FleetSummary {
    pub total: i64,
    pub online: i64,
    pub offline: i64,
    pub converge_success: i64,
    pub converge_failed: i64,
    pub compliant: i64,
    pub non_compliant: i64,
    pub unknown_compliance: i64,
    pub flipped: Vec<FlippedNode>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ComplianceTrendBucket {
    /// UTC calendar day, e.g. "2026-08-22".
    pub date: chrono::NaiveDate,
    /// passed / (passed + failed) * 100, rounded to 2 decimals; 0 when empty.
    pub pass_rate: f64,
    pub passed: i64,
    pub failed: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunsTrendBucket {
    /// UTC calendar day, e.g. "2026-08-22".
    pub date: chrono::NaiveDate,
    pub success: i64,
    pub failed: i64,
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Heap-indirected error type for the UI handlers: the raw
/// `axum::response::Response` is >128 bytes, which trips
/// `clippy::result_large_err` under the crate's `#![deny(clippy::all)]`.
/// Boxing keeps the `Result`'s `Err` variant one pointer wide -- the error
/// path (DB failure / bad request) is cold.
struct UiError(Box<axum::response::Response>);

impl axum::response::IntoResponse for UiError {
    fn into_response(self) -> axum::response::Response {
        *self.0
    }
}

fn internal_error(e: impl std::fmt::Display) -> UiError {
    UiError(Box::new(
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "internal_error",
                "message": e.to_string(),
            })),
        )
            .into_response(),
    ))
}

fn bad_request(message: String) -> UiError {
    UiError(Box::new(
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "bad_request",
                "message": message,
            })),
        )
            .into_response(),
    ))
}

/// Parse + validate the ?days= parameter. Defaults when absent, clamps to 1..=365.
/// Returns i32 because make_interval(days => int) is an int4 parameter —
/// binding i64 fails function resolution ("function make_interval(days => bigint)
/// does not exist").
fn parse_days(params: &HashMap<String, String>, default: i64) -> Result<i32, String> {
    match params.get("days") {
        None => Ok(default as i32),
        Some(raw) => {
            let d: i64 = raw
                .parse()
                .map_err(|_| format!("Invalid days: {raw} (expected an integer)"))?;
            if d < 1 {
                return Err("days must be >= 1".to_string());
            }
            Ok((d.min(365)) as i32)
        }
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

// ── Handlers ────────────────────────────────────────────────────────────────

/// GET /v1/summary — fleet rollup for dashboards.
#[utoipa::path(
    get,
    path = "/v1/summary",
    tag = "ui",
    responses(
        (status = 200, description = "Fleet summary", body = FleetSummary),
        (status = 500, description = "Database error"),
    ),
)]
pub async fn summary(State(state): State<UiAppState>) -> Result<Json<FleetSummary>, UiError> {
    let Some(pool) = state.db_pool.clone() else {
        // Dev mode without DB — zeros across the board.
        return Ok(Json(FleetSummary {
            total: 0,
            online: 0,
            offline: 0,
            converge_success: 0,
            converge_failed: 0,
            compliant: 0,
            non_compliant: 0,
            unknown_compliance: 0,
            flipped: Vec::new(),
        }));
    };

    // Node inventory + online/offline split (dashboard-equivalent 300s rule).
    let node_row: (i64, i64) = sqlx::query_as(
        "SELECT \
             COUNT(*) AS total, \
             COUNT(*) FILTER (\
                 WHERE last_seen IS NOT NULL \
                 AND last_seen >= NOW() - make_interval(secs => $1)\
             ) AS online \
         FROM nodes",
    )
    .bind(ONLINE_THRESHOLD_SECS)
    .fetch_one(&pool)
    .await
    .map_err(internal_error)?;

    // Converge outcome counters from run history.
    let run_row: (i64, i64) = sqlx::query_as(
        "SELECT \
             COUNT(*) FILTER (WHERE status = 'success') AS ok, \
             COUNT(*) FILTER (WHERE status = 'failed') AS failed \
         FROM runs",
    )
    .fetch_one(&pool)
    .await
    .map_err(internal_error)?;

    // Aggregate compliance classification per node.
    // A node is non-compliant if ANY of its reports has status='failed';
    // otherwise compliant if ANY report has status='passed' or 'warn';
    // otherwise unknown (no reports or only unknown-status reports).
    // This fixes the issue where nodes whose latest report is a "warn"
    // (warning_count>0, failed_count=0) were dumped into unknownCompliance.
    let class_row: (i64, i64) = sqlx::query_as(
        "WITH node_status AS ( \
             SELECT node_id, \
                    bool_or(status = 'failed') AS has_failed, \
                    bool_or(status IN ('passed', 'warn')) AS has_ok \
             FROM compliance_reports \
             GROUP BY node_id \
         ) \
         SELECT \
             COALESCE(SUM((NOT has_failed AND has_ok)::int), 0) AS compliant, \
             COALESCE(SUM(has_failed::int), 0) AS non_compliant \
         FROM node_status",
    )
    .fetch_one(&pool)
    .await
    .map_err(internal_error)?;

    // Recently-flipped nodes: latest report failed, penultimate passed.
    let flipped: Vec<(String, String)> = sqlx::query_as(
        "WITH ranked AS ( \
             SELECT cr.node_id, n.name, cr.status, \
                    ROW_NUMBER() OVER ( \
                        PARTITION BY cr.node_id ORDER BY cr.created_at DESC, cr.id DESC \
                    ) AS rn \
             FROM compliance_reports cr \
             JOIN nodes n ON n.id = cr.node_id \
         ) \
         SELECT r1.node_id::text, r1.name \
         FROM ranked r1 \
         WHERE r1.rn = 1 AND r1.status = 'failed' \
           AND EXISTS ( \
               SELECT 1 FROM ranked r2 \
               WHERE r2.node_id = r1.node_id AND r2.rn = 2 AND r2.status = 'passed' \
           ) \
         ORDER BY r1.name",
    )
    .fetch_all(&pool)
    .await
    .map_err(internal_error)?;

    let (total, online) = node_row;
    let (converge_success, converge_failed) = run_row;
    let (compliant, non_compliant) = class_row;
    // Everything not classified passed/failed on its latest report — nodes
    // with no reports at all included — lands in the unknown bucket.
    let unknown_compliance = (total - compliant - non_compliant).max(0);

    Ok(Json(FleetSummary {
        total,
        online,
        offline: total - online,
        converge_success,
        converge_failed,
        compliant,
        non_compliant,
        unknown_compliance,
        flipped: flipped
            .into_iter()
            .map(|(id, name)| FlippedNode { id, name })
            .collect(),
    }))
}

/// GET /v1/compliance/trend?days=14 — daily pass/fail buckets for compliance reports.
#[utoipa::path(
    get,
    path = "/v1/compliance/trend",
    tag = "ui",
    params(
        ("days" = Option<i64>, Query, description = "Window size in days (default 14, clamped 1..=365)"),
    ),
    responses(
        (status = 200, description = "Daily compliance buckets (data.items envelope)", body = serde_json::Value),
        (status = 400, description = "Invalid days parameter"),
        (status = 500, description = "Database error"),
    ),
)]
pub async fn compliance_trend(
    State(state): State<UiAppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, UiError> {
    let days = parse_days(&params, 14).map_err(bad_request)?;

    let Some(pool) = state.db_pool.clone() else {
        return Ok(Json(serde_json::json!({"data": {"items": []}})));
    };

    let rows: Vec<(chrono::NaiveDate, i64, i64)> = sqlx::query_as(
        "SELECT \
             (created_at AT TIME ZONE 'UTC')::date AS day, \
             COALESCE(SUM((status = 'passed')::int), 0) AS passed, \
             COALESCE(SUM((status = 'failed')::int), 0) AS failed \
         FROM compliance_reports \
         WHERE created_at >= NOW() - make_interval(days => $1) \
         GROUP BY day \
         ORDER BY day",
    )
    .bind(days)
    .fetch_all(&pool)
    .await
    .map_err(internal_error)?;

    // Wrap in the standard list envelope to match /v1/compliance/reports.
    let items: Vec<ComplianceTrendBucket> = rows
        .into_iter()
        .map(|(date, passed, failed)| {
            let denom = passed + failed;
            let pass_rate = if denom > 0 {
                round2(passed as f64 / denom as f64 * 100.0)
            } else {
                0.0
            };
            ComplianceTrendBucket {
                date,
                pass_rate,
                passed,
                failed,
            }
        })
        .collect();

    Ok(Json(serde_json::json!({ "data": { "items": items } })))
}

/// GET /v1/runs/trend?days=7 — daily success/fail buckets for converge runs.
#[utoipa::path(
    get,
    path = "/v1/runs/trend",
    tag = "ui",
    params(
        ("days" = Option<i64>, Query, description = "Window size in days (default 7, clamped 1..=365)"),
    ),
    responses(
        (status = 200, description = "Daily run buckets (data.items envelope)", body = serde_json::Value),
        (status = 400, description = "Invalid days parameter"),
        (status = 500, description = "Database error"),
    ),
)]
pub async fn runs_trend(
    State(state): State<UiAppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, UiError> {
    let days = parse_days(&params, 7).map_err(bad_request)?;

    let Some(pool) = state.db_pool.clone() else {
        return Ok(Json(serde_json::json!({"data": {"items": []}})));
    };

    let rows: Vec<(chrono::NaiveDate, i64, i64)> = sqlx::query_as(
        "SELECT \
             (COALESCE(start_time, created_at) AT TIME ZONE 'UTC')::date AS day, \
             COALESCE(SUM((status = 'success')::int), 0) AS success, \
             COALESCE(SUM((status = 'failed')::int), 0) AS failed \
         FROM runs \
         WHERE COALESCE(start_time, created_at) >= NOW() - make_interval(days => $1) \
         GROUP BY day \
         ORDER BY day",
    )
    .bind(days)
    .fetch_all(&pool)
    .await
    .map_err(internal_error)?;

    // Wrap in the standard list envelope to match /v1/runs.
    let items: Vec<RunsTrendBucket> = rows
        .into_iter()
        .map(|(date, success, failed)| RunsTrendBucket {
            date,
            success,
            failed,
        })
        .collect();

    Ok(Json(serde_json::json!({ "data": { "items": items } })))
}
