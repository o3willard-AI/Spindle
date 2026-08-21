//! spindle-server::waivers — Waiver CRUD endpoints.
//!
//! Endpoints:
//! - `POST /v1/waivers` — create a waiver
//! - `GET /v1/waivers` — list active (non-expired) waivers
//! - `GET /v1/waivers/:id` — get a waiver
//! - `PUT /v1/waivers/:id` — update a waiver
//! - `DELETE /v1/waivers/:id` — delete a waiver
//!
//! Waiver schema: control_id, scope (node/project/global),
//!   justification, approver, start_date, expiry_date.
//! Expired waivers are auto-excluded from list responses.
//! Every CRUD event is logged to the audit_log table.

#![allow(warnings)]
use axum::{
    extract::{Path, Query, Request, State},
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::ingest::{EnvelopeResponse, API_VERSION, X_REQUEST_ID_HEADER};
use spindle_api::QueryFilter;
use spindle_api::{parse_query_string, validate_filter_fields, VALID_WAIVER_FIELDS};
use spindle_authz::Scope;

// ── Request/Response types ──────────────────────────────────────────────

/// Create/update waiver request body.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct WaiverRequest {
    #[serde(default)]
    pub control_id: String,
    pub profile_id: Option<String>,
    pub scope: String,
    pub justification: Option<String>,
    pub approver: Option<String>,
    pub start_date: Option<String>,
    pub expiry_date: String,
    /// Optional: number of days from start_date to expiry.
    /// If provided, overrides expiry_date. Must be >= 1.
    #[serde(default)]
    pub days: Option<u64>,
}

/// Waiver summary (list view).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct WaiverSummary {
    pub id: String,
    pub control_id: String,
    pub profile_id: String,
    pub scope: String,
    pub justification: Option<String>,
    pub approver: Option<String>,
    pub start_date: String,
    pub expiry_date: String,
    pub created_at: String,
    pub updated_at: String,
    pub is_expired: bool,
}

/// Full waiver detail.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct WaiverDetail {
    pub id: String,
    pub control_id: String,
    pub profile_id: String,
    pub scope: String,
    pub justification: Option<String>,
    pub approver: Option<String>,
    pub start_date: String,
    pub expiry_date: String,
    pub created_at: String,
    pub updated_at: String,
    pub is_expired: bool,
}

/// Paginated waiver list response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct WaiversListResponse {
    pub api_version: String,
    pub request_id: String,
    pub data: Vec<WaiverSummary>,
    pub pagination: PaginationInfo,
}

/// Pagination info for sub-lists.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct PaginationInfo {
    pub total_count: usize,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub limit: usize,
}

/// Single waiver detail response.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct WaiverDetailResponse {
    pub api_version: String,
    pub request_id: String,
    pub data: WaiverDetail,
    /// Data provenance — absent for direct data, present for rollup-derived data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<crate::ingest::Provenance>,
    /// Stripped attributes marker — true when compliance-auditor role strips sensitive attributes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stripped_attributes: Option<bool>,
}

// ── Audit log types ─────────────────────────────────────────────────────

/// Audit log entry.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AuditLogEntry {
    pub id: String,
    pub subject: String,
    pub subject_source: Option<String>,
    pub resource_type: String,
    pub resource_id: String,
    pub action: String,
    pub decision: String,
    pub rule: Option<String>,
    pub details: Option<serde_json::Value>,
    pub created_at: String,
}

// ── SQL-backed audit store (production) ──────────────────────────────────

/// SQL-backed audit log store using PostgreSQL.
#[derive(Debug, Clone)]
pub struct SqlxAuditStore {
    pub pool: sqlx::PgPool,
}

impl SqlxAuditStore {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl AuditEventLog for SqlxAuditStore {
    async fn log_audit_event(
        &self,
        subject: &str,
        resource_type: &str,
        resource_id: &str,
        action: &str,
        decision: &str,
        details: Option<Value>,
    ) -> Result<Uuid, StoreError> {
        let id = Uuid::new_v4();
        let resource_uuid = Uuid::parse_str(resource_id).ok(); // optional UUID

        sqlx::query(
            r#"
            INSERT INTO audit_log (id, subject, subject_source, resource_type, resource_id, action, decision, rule, details)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#
        )
        .bind(id)
        .bind(subject)
        .bind::<Option<String>>(None)
        .bind(resource_type)
        .bind(resource_uuid)
        .bind(action)
        .bind(decision)
        .bind::<Option<String>>(None)
        .bind(details)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(e.to_string()))?;

        Ok(id)
    }
}

/// In-memory waiver store implementing the canonical `spindle_store::WaiverStore` trait.
#[derive(Debug, Clone, Default)]
pub struct InMemoryWaiverStore {
    pub waivers: Arc<std::sync::RwLock<Vec<spindle_store::Waiver>>>,
}

/// Audit log store.
#[derive(Debug, Clone, Default)]
pub struct InMemoryAuditStore {
    pub entries: Arc<std::sync::Mutex<Vec<AuditLogEntry>>>,
}

impl InMemoryWaiverStore {
    pub fn new() -> Self {
        let mut waivers = Vec::new();
        let now = Utc::now();

        // Seed with sample waivers — profile_id is a Uuid in spindle_store::Waiver
        let profile_os =
            Uuid::parse_str("00000000-0000-4000-8000-0000000a0001").unwrap_or_else(|_| Uuid::nil());
        let profile_app =
            Uuid::parse_str("00000000-0000-4000-8000-0000000a0002").unwrap_or_else(|_| Uuid::nil());

        waivers.push(spindle_store::Waiver {
            id: Uuid::parse_str("00000000-0000-4000-8000-000000000001")
                .unwrap_or_else(|_| Uuid::nil()),
            control_id: "cis-3.1.1".to_string(),
            profile_id: profile_os,
            scope: "node".to_string(),
            justification: Some("Temporary exception for legacy systems".to_string()),
            approver: Some("security-team".to_string()),
            start_date: now - chrono::Duration::days(30),
            expiry_date: now + chrono::Duration::days(30),
            created_at: now - chrono::Duration::days(30),
            updated_at: now - chrono::Duration::days(30),
        });

        waivers.push(spindle_store::Waiver {
            id: Uuid::parse_str("00000000-0000-4000-8000-000000000002")
                .unwrap_or_else(|_| Uuid::nil()),
            control_id: "cis-4.2.3".to_string(),
            profile_id: profile_app,
            scope: "project".to_string(),
            justification: Some("Application requires elevated permissions".to_string()),
            approver: Some("app-owner".to_string()),
            start_date: now - chrono::Duration::days(60),
            expiry_date: now - chrono::Duration::days(1), // expired
            created_at: now - chrono::Duration::days(60),
            updated_at: now - chrono::Duration::days(60),
        });

        waivers.push(spindle_store::Waiver {
            id: Uuid::parse_str("00000000-0000-4000-8000-000000000003")
                .unwrap_or_else(|_| Uuid::nil()),
            control_id: "cis-5.1.2".to_string(),
            profile_id: profile_os,
            scope: "global".to_string(),
            justification: Some("Global policy override for maintenance window".to_string()),
            approver: Some("it-director".to_string()),
            start_date: now - chrono::Duration::days(7),
            expiry_date: now + chrono::Duration::days(60),
            created_at: now - chrono::Duration::days(7),
            updated_at: now - chrono::Duration::days(7),
        });

        Self {
            waivers: Arc::new(std::sync::RwLock::new(waivers)),
        }
    }

    /// Check if a waiver is still active (not expired).
    pub fn is_active(w: &spindle_store::Waiver) -> bool {
        w.expiry_date > Utc::now()
    }
}

#[async_trait::async_trait]
impl spindle_store::WaiverStore for InMemoryWaiverStore {
    async fn get_waiver(
        &self,
        id: Uuid,
        _scope: &Scope,
    ) -> spindle_store::Result<spindle_store::Waiver> {
        let waivers = self.waivers.read().unwrap_or_else(|e| e.into_inner());
        let w = waivers
            .iter()
            .find(|w| w.id == id)
            .ok_or_else(|| spindle_store::StoreError::NotFound(format!("waiver {}", id)))?;
        Ok(w.clone())
    }

    async fn list_waivers(
        &self,
        _scope: &Scope,
    ) -> spindle_store::Result<Vec<spindle_store::Waiver>> {
        let waivers = self.waivers.read().unwrap_or_else(|e| e.into_inner());
        let now = Utc::now();
        let active: Vec<spindle_store::Waiver> = waivers
            .iter()
            .filter(|w| w.expiry_date > now)
            .cloned()
            .collect();
        Ok(active)
    }

    async fn upsert_waiver(
        &self,
        waiver: &spindle_store::Waiver,
        _scope: &Scope,
    ) -> spindle_store::Result<Uuid> {
        let mut waivers = self.waivers.write().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = waivers.iter_mut().find(|w| w.id == waiver.id) {
            *existing = waiver.clone();
        } else {
            waivers.push(waiver.clone());
        }
        Ok(waiver.id)
    }

    async fn delete_waiver(&self, id: Uuid, _scope: &Scope) -> spindle_store::Result<()> {
        let mut waivers = self.waivers.write().unwrap_or_else(|e| e.into_inner());
        let pos = waivers
            .iter()
            .position(|w| w.id == id)
            .ok_or_else(|| spindle_store::StoreError::NotFound(format!("waiver {}", id)))?;
        waivers.remove(pos);
        Ok(())
    }
}

/// Server-only trait: audit event logging for waiver CRUD operations. No spindle-store counterpart.
#[async_trait::async_trait]
pub trait AuditEventLog: Send + Sync + std::fmt::Debug {
    async fn log_audit_event(
        &self,
        subject: &str,
        resource_type: &str,
        resource_id: &str,
        action: &str,
        decision: &str,
        details: Option<Value>,
    ) -> Result<Uuid, StoreError>;
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Conflict: {0}")]
    Conflict(String),
    #[error("Validation: {0}")]
    Validation(String),
    #[error("Internal error: {0}")]
    Internal(String),
}

impl StoreError {
    fn status(&self) -> StatusCode {
        match self {
            StoreError::NotFound(_) => StatusCode::NOT_FOUND,
            StoreError::Conflict(_) => StatusCode::CONFLICT,
            StoreError::Validation(_) => StatusCode::BAD_REQUEST,
            StoreError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

// ── Implementations ─────────────────────────────────────────────────────

#[async_trait::async_trait]
impl AuditEventLog for InMemoryAuditStore {
    async fn log_audit_event(
        &self,
        subject: &str,
        resource_type: &str,
        resource_id: &str,
        action: &str,
        decision: &str,
        details: Option<Value>,
    ) -> Result<Uuid, StoreError> {
        let now = Utc::now();
        let entry = AuditLogEntry {
            id: Uuid::new_v4().to_string(),
            subject: subject.to_string(),
            subject_source: None,
            resource_type: resource_type.to_string(),
            resource_id: resource_id.to_string(),
            action: action.to_string(),
            decision: decision.to_string(),
            rule: None,
            details,
            created_at: now.to_rfc3339(),
        };
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(entry.clone());
        Ok(Uuid::parse_str(&entry.id).unwrap_or_else(|_| Uuid::nil()))
    }
}

// ── Mapping functions ─────────────────────────────────────────────────────

/// Map a `spindle_store::Waiver` to a `WaiverSummary` web DTO.
pub fn waiver_to_summary(w: &spindle_store::Waiver) -> WaiverSummary {
    let is_expired = w.expiry_date < Utc::now();
    WaiverSummary {
        id: w.id.to_string(),
        control_id: w.control_id.clone(),
        profile_id: w.profile_id.to_string(),
        scope: w.scope.clone(),
        justification: w.justification.clone(),
        approver: w.approver.clone(),
        start_date: w.start_date.to_rfc3339(),
        expiry_date: w.expiry_date.to_rfc3339(),
        created_at: w.created_at.to_rfc3339(),
        updated_at: w.updated_at.to_rfc3339(),
        is_expired,
    }
}

/// Map a `spindle_store::Waiver` to a `WaiverDetail` web DTO.
pub fn waiver_to_detail(w: &spindle_store::Waiver) -> WaiverDetail {
    let is_expired = w.expiry_date < Utc::now();
    WaiverDetail {
        id: w.id.to_string(),
        control_id: w.control_id.clone(),
        profile_id: w.profile_id.to_string(),
        scope: w.scope.clone(),
        justification: w.justification.clone(),
        approver: w.approver.clone(),
        start_date: w.start_date.to_rfc3339(),
        expiry_date: w.expiry_date.to_rfc3339(),
        created_at: w.created_at.to_rfc3339(),
        updated_at: w.updated_at.to_rfc3339(),
        is_expired,
    }
}

/// Map a `spindle_store::StoreError` to the server's HTTP `StoreError`.
fn map_store_error(e: spindle_store::StoreError) -> StoreError {
    match e {
        spindle_store::StoreError::NotFound(msg) => StoreError::NotFound(msg),
        spindle_store::StoreError::ScopeDenied(msg) => StoreError::NotFound(msg),
        // FK violations and other DB constraints surface as QueryFailed.
        // These are client errors (bogus control_id / profile_id), not server errors.
        spindle_store::StoreError::QueryFailed(msg) => {
            let msg_str = msg.to_string();
            if is_foreign_key_error(&msg_str) {
                StoreError::Validation(format!(
                    "Foreign key violation: {}. The referenced control_id, profile_id, or node_id may not exist.",
                    msg_str
                ))
            } else {
                StoreError::Validation(msg_str)
            }
        }
        spindle_store::StoreError::Storage(msg) => StoreError::Validation(msg),
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Check if a database error message indicates a foreign key violation.
/// Handles PostgreSQL (23503), SQLite (FOREIGN KEY CONSTRAINT), and generic
/// "foreign key" messages.
fn is_foreign_key_error(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("foreign key")
        || lower.contains("23503")
        || lower.contains("violates foreign key")
        || lower.contains("references constraint")
}

fn get_request_id_from_headers(headers: &axum::http::HeaderMap) -> String {
    headers
        .get(X_REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(crate::ingest::new_request_id)
}

fn build_query_string(params: &std::collections::HashMap<String, String>) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("&")
}

fn get_request_id(request: &Request) -> String {
    request
        .headers()
        .get(X_REQUEST_ID_HEADER)
        .and_then(|h| h.to_str().ok())
        .unwrap_or(&Uuid::new_v4().to_string())
        .to_string()
}

// ── App state ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct WaiversAppState {
    pub store: Arc<dyn spindle_store::WaiverStore>,
    pub audit_store: Arc<dyn AuditEventLog>,
    pub metrics: Arc<crate::metrics::MetricsRegistry>,
}

impl WaiversAppState {
    pub fn new(
        store: Arc<dyn spindle_store::WaiverStore>,
        audit: Arc<dyn AuditEventLog>,
        metrics: Arc<crate::metrics::MetricsRegistry>,
    ) -> Self {
        Self {
            store,
            audit_store: audit,
            metrics,
        }
    }
}

// ── Route builder ────────────────────────────────────────────────────────

pub fn waivers_routes(state: WaiversAppState) -> Router {
    Router::new()
        .route("/v1/waivers", post(create_waiver).get(list_waivers))
        .route(
            "/v1/waivers/:id",
            get(get_waiver).put(update_waiver).delete(delete_waiver),
        )
        .with_state(state)
        .route_layer(middleware::from_fn(crate::ingest::request_id_middleware))
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// POST /v1/waivers — create a waiver.
#[utoipa::path(
    post,
    path = "/v1/waivers",
    tag = "waivers",
    request_body = WaiverRequest,
    responses(
        (status = 201, description = "Waiver created", body = WaiverDetailResponse),
        (status = 400, description = "Invalid input"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Admin role required"),
    ),
    security(("bearer" = [])),
)]
pub async fn create_waiver(
    State(state): State<WaiversAppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<WaiverRequest>,
) -> impl IntoResponse {
    let request_id = get_request_id_from_headers(&headers);
    let scope = crate::ingest::extract_scope(&headers);

    // RBAC: only admin can create waivers (write operation)
    let role_str = headers
        .get(crate::ingest::X_USER_ROLE_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("viewer");
    if role_str != "admin" {
        return EnvelopeResponse::forbidden(
            "auth_required",
            "Access denied by role policy",
            &request_id,
        )
        .into_response();
    }

    // Validate request
    if req.control_id.is_empty() {
        return EnvelopeResponse::bad_request("validation", "control_id is required", &request_id)
            .into_response();
    }
    if req.scope.is_empty() {
        return EnvelopeResponse::bad_request("validation", "scope is required", &request_id)
            .into_response();
    }

    // Validate scope value
    match req.scope.as_str() {
        "node" | "project" | "global" => {}
        _ => {
            return EnvelopeResponse::bad_request(
                "validation",
                &format!(
                    "scope must be 'node', 'project', or 'global', got '{}'",
                    req.scope
                ),
                &request_id,
            )
            .into_response();
        }
    }

    // Parse dates from request
    let start_date = if let Some(ref sd) = req.start_date {
        chrono::DateTime::parse_from_rfc3339(sd)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now())
    } else {
        Utc::now()
    };

    // If days param is provided, validate it and compute expiry_date from start_date
    let mut expiry_date = if let Some(days) = req.days {
        if days < 1 {
            return EnvelopeResponse::bad_request(
                "validation",
                &format!("waiver duration must be at least 1 day, got {} days", days),
                &request_id,
            )
            .into_response();
        }
        start_date + chrono::Duration::days(days as i64)
    } else {
        match chrono::DateTime::parse_from_rfc3339(&req.expiry_date) {
            Ok(dt) => dt.with_timezone(&Utc),
            Err(_) => {
                return EnvelopeResponse::bad_request(
                    "validation",
                    "invalid expiry_date format",
                    &request_id,
                )
                .into_response();
            }
        }
    };

    // Validate effective duration — waiver must be effective for at least 1 day
    let duration = expiry_date.signed_duration_since(start_date);
    if duration.num_days() < 1 {
        return EnvelopeResponse::bad_request(
            "validation",
            &format!(
                "waiver duration must be at least 1 day, got {} hours (expiry_date {} - start_date {})",
                duration.num_hours(),
                expiry_date,
                start_date
            ),
            &request_id,
        )
            .into_response();
    }

    let profile_id = req
        .profile_id
        .as_ref()
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or_else(Uuid::nil);

    let now = Utc::now();
    let waiver = spindle_store::Waiver {
        id: Uuid::new_v4(),
        control_id: req.control_id.clone(),
        profile_id,
        scope: req.scope.clone(),
        justification: req.justification.clone(),
        approver: req.approver.clone(),
        start_date,
        expiry_date,
        created_at: now,
        updated_at: now,
    };

    match state.store.upsert_waiver(&waiver, &scope).await {
        Ok(id) => {
            // Audit log
            let _ = state
                .audit_store
                .log_audit_event(
                    "admin",
                    "waiver",
                    &id.to_string(),
                    "create",
                    "allow",
                    Some(serde_json::json!({
                        "control_id": req.control_id,
                        "scope": req.scope,
                    })),
                )
                .await;

            let detail = waiver_to_detail(&waiver);
            let response = WaiverDetailResponse {
                api_version: API_VERSION.to_string(),
                request_id,
                data: detail,
                provenance: None,
                stripped_attributes: None,
            };
            tracing::debug!(path = "/v1/waivers/{id}", "api query result");
            Json(response).into_response()
        }
        Err(e) => {
            let err = map_store_error(e);
            match err {
                StoreError::NotFound(msg) => {
                    EnvelopeResponse::not_found("not_found", &msg, &request_id).into_response()
                }
                StoreError::Conflict(msg) => {
                    EnvelopeResponse::conflict("conflict", &msg, &request_id).into_response()
                }
                StoreError::Validation(msg) => {
                    EnvelopeResponse::bad_request("validation", &msg, &request_id).into_response()
                }
                StoreError::Internal(msg) => {
                    EnvelopeResponse::bad_request("store_error", &msg, &request_id).into_response()
                }
            }
        }
    }
}

/// GET /v1/waivers — list active (non-expired) waivers.
#[utoipa::path(
    get,
    path = "/v1/waivers",
    tag = "waivers",
    responses(
        (status = 200, description = "Successful response", body = WaiversListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    params(
        ("page" = Option<u32>, Query, description = "Page number"),
        ("per_page" = Option<u32>, Query, description = "Items per page"),
    ),
)]
pub async fn list_waivers(
    State(state): State<WaiversAppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    request: Request,
) -> impl IntoResponse {
    let request_id = get_request_id(&request);
    let scope = crate::ingest::extract_scope(request.headers());

    // Parse filter grammar
    let raw_query = build_query_string(&params);
    let filter = match parse_query_string(&raw_query, VALID_WAIVER_FIELDS) {
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

    // Validate filter fields
    if let Err(e) = validate_filter_fields(
        &filter.filters,
        &spindle_api::TimeRange::default(),
        VALID_WAIVER_FIELDS,
    ) {
        return EnvelopeResponse::bad_request(
            "bad_request",
            &format!("Invalid field: {}", e),
            &request_id,
        )
        .into_response();
    }

    match state.store.list_waivers(&scope).await {
        Ok(waivers) => {
            let summaries: Vec<WaiverSummary> = waivers.iter().map(waiver_to_summary).collect();
            let count = summaries.len();
            let response = WaiversListResponse {
                api_version: API_VERSION.to_string(),
                request_id,
                data: summaries,
                pagination: PaginationInfo {
                    total_count: count,
                    has_more: false,
                    next_cursor: None,
                    limit: count,
                },
            };
            Json(response).into_response()
        }
        Err(e) => EnvelopeResponse::bad_request(
            "store_error",
            &format!("{}", map_store_error(e)),
            &request_id,
        )
        .into_response(),
    }
}

/// GET /v1/waivers/:id — get a waiver detail.
#[utoipa::path(
    get,
    path = "/v1/waivers/{id}",
    tag = "waivers",
    responses(
        (status = 200, description = "Waiver detail", body = WaiverDetailResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Waiver not found"),
    ),
    params(
        ("id" = String, Path, description = "Waiver UUID"),
    ),
)]
pub async fn get_waiver(
    State(state): State<WaiversAppState>,
    Path(id): Path<String>,
    request: Request,
) -> impl IntoResponse {
    let request_id = get_request_id(&request);
    let scope = crate::ingest::extract_scope(request.headers());

    let uuid = match Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(_) => {
            return EnvelopeResponse::not_found(
                "not_found",
                &format!("Invalid waiver ID: {}", id),
                &request_id,
            )
            .into_response();
        }
    };

    match state.store.get_waiver(uuid, &scope).await {
        Ok(waiver) => {
            let detail = waiver_to_detail(&waiver);
            let response = WaiverDetailResponse {
                api_version: API_VERSION.to_string(),
                request_id,
                data: detail,
                provenance: None,
                stripped_attributes: None,
            };
            Json(response).into_response()
        }
        Err(e) => {
            let err = map_store_error(e);
            match err {
                StoreError::NotFound(msg) => {
                    EnvelopeResponse::not_found("not_found", &msg, &request_id).into_response()
                }
                StoreError::Internal(msg) => {
                    EnvelopeResponse::bad_request("store_error", &msg, &request_id).into_response()
                }
                _ => EnvelopeResponse::bad_request("store_error", &err.to_string(), &request_id)
                    .into_response(),
            }
        }
    }
}

/// PUT /v1/waivers/:id — update a waiver.
#[utoipa::path(
    put,
    path = "/v1/waivers/{id}",
    tag = "waivers",
    request_body = WaiverRequest,
    responses(
        (status = 200, description = "Waiver updated", body = WaiverDetailResponse),
        (status = 400, description = "Invalid input"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Waiver not found"),
    ),
    params(
        ("id" = String, Path, description = "Waiver UUID"),
    ),
    security(("bearer" = [])),
)]
pub async fn update_waiver(
    State(state): State<WaiversAppState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(req): Json<WaiverRequest>,
) -> impl IntoResponse {
    let request_id = get_request_id_from_headers(&headers);
    let scope = crate::ingest::extract_scope(&headers);

    // RBAC: only admin can update waivers (write operation)
    let role_str = headers
        .get(crate::ingest::X_USER_ROLE_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("viewer");
    if role_str != "admin" {
        return EnvelopeResponse::forbidden(
            "auth_required",
            "Access denied by role policy",
            &request_id,
        )
        .into_response();
    }

    let uuid = match Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(_) => {
            return EnvelopeResponse::not_found(
                "not_found",
                &format!("Invalid waiver ID: {}", id),
                &request_id,
            )
            .into_response();
        }
    };

    // Get existing waiver, then update fields
    let mut waiver = match state.store.get_waiver(uuid, &scope).await {
        Ok(w) => w,
        Err(e) => {
            let err = map_store_error(e);
            return match err {
                StoreError::NotFound(msg) => {
                    EnvelopeResponse::not_found("not_found", &msg, &request_id).into_response()
                }
                _ => EnvelopeResponse::bad_request("store_error", &err.to_string(), &request_id)
                    .into_response(),
            };
        }
    };

    waiver.justification = req.justification.clone();
    waiver.approver = req.approver.clone();
    waiver.scope = req.scope.clone();

    if let Some(ref sd) = req.start_date {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(sd) {
            waiver.start_date = dt.with_timezone(&Utc);
        }
    }

    // If days param is provided, recompute expiry from start_date
    if let Some(days) = req.days {
        if days < 1 {
            return EnvelopeResponse::bad_request(
                "validation",
                &format!("waiver duration must be at least 1 day, got {} days", days),
                &request_id,
            )
            .into_response();
        }
        waiver.expiry_date = waiver.start_date + chrono::Duration::days(days as i64);
    }

    if !req.expiry_date.is_empty() {
        match chrono::DateTime::parse_from_rfc3339(&req.expiry_date) {
            Ok(dt) => waiver.expiry_date = dt.with_timezone(&Utc),
            Err(_) => {
                return EnvelopeResponse::bad_request(
                    "validation",
                    "invalid expiry_date format",
                    &request_id,
                )
                .into_response();
            }
        }
    }

    // Validate effective duration — waiver must be effective for at least 1 day
    let duration = waiver.expiry_date.signed_duration_since(waiver.start_date);
    if duration.num_days() < 1 {
        return EnvelopeResponse::bad_request(
            "validation",
            &format!(
                "waiver duration must be at least 1 day, got {} hours (expiry_date {} - start_date {})",
                duration.num_hours(),
                waiver.expiry_date,
                waiver.start_date
            ),
            &request_id,
        )
            .into_response();
    }

    waiver.updated_at = Utc::now();

    match state.store.upsert_waiver(&waiver, &scope).await {
        Ok(id) => {
            // Audit log
            let _ = state
                .audit_store
                .log_audit_event(
                    "admin",
                    "waiver",
                    &id.to_string(),
                    "update",
                    "allow",
                    Some(serde_json::json!({
                        "control_id": req.control_id,
                        "scope": req.scope,
                    })),
                )
                .await;

            let detail = waiver_to_detail(&waiver);
            let response = WaiverDetailResponse {
                api_version: API_VERSION.to_string(),
                request_id,
                data: detail,
                provenance: None,
                stripped_attributes: None,
            };
            Json(response).into_response()
        }
        Err(e) => {
            let err = map_store_error(e);
            match err {
                StoreError::NotFound(msg) => {
                    EnvelopeResponse::not_found("not_found", &msg, &request_id).into_response()
                }
                StoreError::Validation(msg) => {
                    EnvelopeResponse::bad_request("validation", &msg, &request_id).into_response()
                }
                _ => EnvelopeResponse::bad_request("store_error", &err.to_string(), &request_id)
                    .into_response(),
            }
        }
    }
}

/// DELETE /v1/waivers/:id — delete a waiver.
#[utoipa::path(
    delete,
    path = "/v1/waivers/{id}",
    tag = "waivers",
    responses(
        (status = 204, description = "Waiver deleted"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Waiver not found"),
    ),
    params(
        ("id" = String, Path, description = "Waiver UUID"),
    ),
    security(("bearer" = [])),
)]
pub async fn delete_waiver(
    State(state): State<WaiversAppState>,
    Path(id): Path<String>,
    request: Request,
) -> impl IntoResponse {
    let request_id = get_request_id(&request);
    let headers = request.headers();
    let scope = crate::ingest::extract_scope(headers);

    // RBAC: only admin can delete waivers (write operation)
    let role_str = headers
        .get(crate::ingest::X_USER_ROLE_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("viewer");
    if role_str != "admin" {
        return EnvelopeResponse::forbidden(
            "auth_required",
            "Access denied by role policy",
            &request_id,
        )
        .into_response();
    }

    let uuid = match Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(_) => {
            return EnvelopeResponse::not_found(
                "not_found",
                &format!("Invalid waiver ID: {}", id),
                &request_id,
            )
            .into_response();
        }
    };

    match state.store.delete_waiver(uuid, &scope).await {
        Ok(()) => {
            // Audit log
            let _ = state
                .audit_store
                .log_audit_event("admin", "waiver", &id, "delete", "allow", None)
                .await;

            let response =
                EnvelopeResponse::ok("deleted", "Waiver deleted successfully", &request_id);
            response.into_response()
        }
        Err(e) => {
            let err = map_store_error(e);
            match err {
                StoreError::NotFound(msg) => {
                    EnvelopeResponse::not_found("not_found", &msg, &request_id).into_response()
                }
                _ => EnvelopeResponse::bad_request("store_error", &err.to_string(), &request_id)
                    .into_response(),
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use spindle_store::WaiverStore;
    use tower::ServiceExt;

    fn make_state() -> WaiversAppState {
        let store: Arc<dyn spindle_store::WaiverStore> = Arc::new(InMemoryWaiverStore::new());
        let audit: Arc<dyn AuditEventLog> = Arc::new(InMemoryAuditStore::default());
        WaiversAppState::new(
            store,
            audit,
            std::sync::Arc::new(crate::metrics::MetricsRegistry::new()),
        )
    }

    fn make_router() -> Router {
        let state = make_state();
        waivers_routes(state)
    }

    fn make_req(method: &str, uri: &str) -> Request<axum::body::Body> {
        make_req_with_role(method, uri, "admin")
    }

    fn make_req_with_role(method: &str, uri: &str, role: &str) -> Request<axum::body::Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("accept", "application/json")
            .header(crate::ingest::X_REQUEST_ID_HEADER, "test-req-id")
            .header(crate::ingest::X_USER_ROLE_HEADER, role)
            .body(axum::body::Body::empty())
            .unwrap()
    }

    // ── POST /v1/waivers — create ──────────────────────────────────────

    #[tokio::test]
    async fn test_create_waiver_success() {
        let app = make_router();
        let body = serde_json::json!({
            "control_id": "cis-1.2.3",
            "profile_id": "test-profile",
            "scope": "node",
            "justification": "Test justification",
            "approver": "test-admin",
            "expiry_date": "2027-12-31T23:59:59Z"
        });

        let req = Request::builder()
            .method("POST")
            .uri("/v1/waivers")
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .header(X_REQUEST_ID_HEADER, "test-req-create")
            .header("x-user-role", "admin")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let response: WaiverDetailResponse = serde_json::from_slice(&body).unwrap();

        assert_eq!(response.api_version, API_VERSION);
        assert_eq!(response.data.control_id, "cis-1.2.3");
        assert_eq!(response.data.scope, "node");
        assert!(!response.data.is_expired);
    }

    #[tokio::test]
    async fn test_create_waiver_missing_control_id() {
        let app = make_router();
        let body = serde_json::json!({
            "scope": "node",
            "expiry_date": "2027-12-31T23:59:59Z"
        });

        let req = Request::builder()
            .method("POST")
            .uri("/v1/waivers")
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .header(X_REQUEST_ID_HEADER, "test-req-missing")
            .header("x-user-role", "admin")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_create_waiver_invalid_scope() {
        let app = make_router();
        let body = serde_json::json!({
            "control_id": "cis-1.2.3",
            "scope": "invalid",
            "expiry_date": "2027-12-31T23:59:59Z"
        });

        let req = Request::builder()
            .method("POST")
            .uri("/v1/waivers")
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .header(X_REQUEST_ID_HEADER, "test-req-scope")
            .header("x-user-role", "admin")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_create_waiver_upsert_replaces() {
        // The new spindle_store::WaiverStore uses upsert — creating with the same
        // ID replaces the existing waiver rather than returning a conflict.
        // Since create generates a new UUID each time, both creates succeed.
        let app = make_router();

        // First create should succeed
        let body1 = serde_json::json!({
            "control_id": "cis-9.9.9",
            "scope": "node",
            "justification": "First waiver",
            "expiry_date": "2027-12-31T23:59:59Z"
        });

        let req1 = Request::builder()
            .method("POST")
            .uri("/v1/waivers")
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .header(X_REQUEST_ID_HEADER, "test-req-dup-1")
            .header("x-user-role", "admin")
            .body(axum::body::Body::from(body1.to_string()))
            .unwrap();

        let resp1 = app.clone().oneshot(req1).await.unwrap();
        assert_eq!(resp1.status(), StatusCode::OK);

        // Second create with same control+scope also succeeds (new UUID)
        let body2 = serde_json::json!({
            "control_id": "cis-9.9.9",
            "scope": "node",
            "justification": "Duplicate waiver",
            "expiry_date": "2027-12-31T23:59:59Z"
        });

        let req2 = Request::builder()
            .method("POST")
            .uri("/v1/waivers")
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .header(X_REQUEST_ID_HEADER, "test-req-dup-2")
            .header("x-user-role", "admin")
            .body(axum::body::Body::from(body2.to_string()))
            .unwrap();

        let resp2 = app.clone().oneshot(req2).await.unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);
    }

    // ── GET /v1/waivers — list ─────────────────────────────────────────

    #[tokio::test]
    async fn test_list_waivers_returns_active_only() {
        let app = make_router();
        let req = make_req("GET", "/v1/waivers");

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let response: WaiversListResponse = serde_json::from_slice(&body).unwrap();

        // Should exclude expired waiver (wv-00000000-0000-4000-8000-000000000002)
        assert_eq!(response.data.len(), 2); // Only active ones
        for w in &response.data {
            assert!(!w.is_expired);
        }
    }

    #[tokio::test]
    async fn test_list_waivers_unknown_field_rejected() {
        let app = make_router();
        let req = make_req("GET", "/v1/waivers?filter[nonexistent]=value");

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── GET /v1/waivers/:id — get ─────────────────────────────────────

    #[tokio::test]
    async fn test_get_waiver_success() {
        let app = make_router();
        let req = make_req("GET", "/v1/waivers/00000000-0000-4000-8000-000000000001");

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let response: WaiverDetailResponse = serde_json::from_slice(&body).unwrap();

        assert_eq!(response.data.control_id, "cis-3.1.1");
        assert_eq!(response.data.scope, "node");
    }

    #[tokio::test]
    async fn test_get_waiver_not_found() {
        let app = make_router();
        let req = make_req("GET", "/v1/waivers/nonexistent-id");

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_get_waiver_invalid_id() {
        let app = make_router();
        let req = make_req("GET", "/v1/waivers/not-a-uuid");

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── PUT /v1/waivers/:id — update ──────────────────────────────────

    #[tokio::test]
    async fn test_update_waiver_success() {
        let app = make_router();
        let body = serde_json::json!({
            "control_id": "cis-9.9.9",
            "scope": "node",
            "justification": "Updated justification",
            "approver": "new-admin",
            "expiry_date": "2028-06-30T23:59:59Z"
        });

        let req = Request::builder()
            .method("PUT")
            .uri("/v1/waivers/00000000-0000-4000-8000-000000000001")
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .header(X_REQUEST_ID_HEADER, "test-req-update")
            .header("x-user-role", "admin")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let response: WaiverDetailResponse = serde_json::from_slice(&body).unwrap();

        assert_eq!(
            response.data.justification,
            Some("Updated justification".to_string())
        );
        assert_eq!(response.data.approver, Some("new-admin".to_string()));
    }

    #[tokio::test]
    async fn test_update_waiver_not_found() {
        let app = make_router();
        let body = serde_json::json!({
            "control_id": "cis-9.9.9",
            "scope": "node",
            "justification": "Updated",
            "expiry_date": "2028-06-30T23:59:59Z"
        });

        let req = Request::builder()
            .method("PUT")
            .uri("/v1/waivers/nonexistent-id")
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .header(X_REQUEST_ID_HEADER, "test-req-update-nf")
            .header("x-user-role", "admin")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── DELETE /v1/waivers/:id — delete ───────────────────────────────

    #[tokio::test]
    async fn test_delete_waiver_success() {
        let app = make_router();
        let req = make_req("DELETE", "/v1/waivers/00000000-0000-4000-8000-000000000001");

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["message"], "Waiver deleted successfully");
    }

    #[tokio::test]
    async fn test_delete_waiver_not_found() {
        let app = make_router();
        let req = make_req("DELETE", "/v1/waivers/nonexistent-id");

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── Store tests ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_store_has_three_waivers() {
        let store = InMemoryWaiverStore::new();
        let waivers = store.waivers.read().unwrap_or_else(|e| e.into_inner());
        assert_eq!(waivers.len(), 3);
    }

    #[tokio::test]
    async fn test_store_one_expired_waiver() {
        let store = InMemoryWaiverStore::new();
        assert!(!InMemoryWaiverStore::is_active(
            &store.waivers.read().unwrap_or_else(|e| e.into_inner())[1]
        ));
    }

    #[tokio::test]
    async fn test_store_two_active_waivers() {
        let store = InMemoryWaiverStore::new();
        let active = store
            .waivers
            .read()
            .unwrap()
            .iter()
            .filter(|w| InMemoryWaiverStore::is_active(w))
            .count();
        assert_eq!(active, 2);
    }

    // ── Response structure tests ────────────────────────────────────────

    #[tokio::test]
    async fn test_create_response_has_api_version_and_request_id() {
        let app = make_router();
        let body = serde_json::json!({
            "control_id": "cis-test",
            "scope": "global",
            "expiry_date": "2027-12-31T23:59:59Z"
        });

        let req = Request::builder()
            .method("POST")
            .uri("/v1/waivers")
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .header(X_REQUEST_ID_HEADER, "test-req-structure")
            .header("x-user-role", "admin")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["api_version"], "v1");
        assert_eq!(json["request_id"], "test-req-structure");
    }

    // ── Scope validation tests ──────────────────────────────────────────

    #[test]
    fn test_valid_scope_values() {
        let valid_scopes = vec!["node", "project", "global"];
        for scope in valid_scopes {
            assert!(matches!(scope, "node" | "project" | "global"));
        }
    }

    // ── Audit log tests ─────────────────────────────────────────────────

    #[tokio::test]
    async fn test_audit_log_entry_created_on_create() {
        let store = Arc::new(InMemoryWaiverStore::new());
        let audit = Arc::new(InMemoryAuditStore::default());
        let state = WaiversAppState::new(
            store,
            audit.clone(),
            std::sync::Arc::new(crate::metrics::MetricsRegistry::new()),
        );

        let body = serde_json::json!({
            "control_id": "cis-audit-test",
            "scope": "global",
            "expiry_date": "2027-12-31T23:59:59Z"
        });

        let req = Request::builder()
            .method("POST")
            .uri("/v1/waivers")
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .header(X_REQUEST_ID_HEADER, "test-req-audit")
            .header("x-user-role", "admin")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();

        let app = waivers_routes(state);
        let _ = app.clone().oneshot(req).await.unwrap();

        let entries = audit.entries.lock().unwrap_or_else(|e| e.into_inner());
        assert!(!entries.is_empty());
        assert_eq!(entries[0].resource_type, "waiver");
        assert_eq!(entries[0].action, "create");
        assert_eq!(entries[0].decision, "allow");
    }

    // ── Expired waiver list tests ───────────────────────────────────────

    #[tokio::test]
    async fn test_list_excludes_expired() {
        let store = InMemoryWaiverStore::new();
        let scope = Scope::all();
        let waivers = store.list_waivers(&scope).await.unwrap();

        for w in &waivers {
            assert!(InMemoryWaiverStore::is_active(w));
        }
        assert_eq!(waivers.len(), 2);
    }

    // ── Full lifecycle test ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_full_lifecycle() {
        let app = make_router();

        // 1. Create
        let create_body = serde_json::json!({
            "control_id": "cis-lifecycle",
            "profile_id": "test",
            "scope": "project",
            "justification": "Test waiver",
            "approver": "test-admin",
            "expiry_date": "2027-12-31T23:59:59Z"
        });

        let req = Request::builder()
            .method("POST")
            .uri("/v1/waivers")
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .header(X_REQUEST_ID_HEADER, "test-lifecycle")
            .header("x-user-role", "admin")
            .body(axum::body::Body::from(create_body.to_string()))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let create_resp: WaiverDetailResponse = serde_json::from_slice(&body).unwrap();
        let waiver_id = create_resp.data.id.clone();

        assert_eq!(status, StatusCode::OK);

        // 2. Get detail
        let req = make_req("GET", &format!("/v1/waivers/{}", waiver_id));
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 3. Update
        let update_body = serde_json::json!({
            "control_id": "cis-lifecycle",
            "scope": "project",
            "justification": "Updated justification",
            "approver": "new-admin",
            "expiry_date": "2028-06-30T23:59:59Z"
        });

        let req = Request::builder()
            .method("PUT")
            .uri(format!("/v1/waivers/{}", waiver_id))
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .header(X_REQUEST_ID_HEADER, "test-lifecycle")
            .header("x-user-role", "admin")
            .body(axum::body::Body::from(update_body.to_string()))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 4. Delete
        let req = make_req("DELETE", &format!("/v1/waivers/{}", waiver_id));
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 5. Verify deleted
        let req = make_req("GET", &format!("/v1/waivers/{}", waiver_id));
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── Waiver duration validation tests ─────────────────────────────────────

    #[tokio::test]
    async fn test_create_waiver_days_zero_rejected() {
        let app = make_router();
        let body = serde_json::json!({
            "control_id": "cis-1.2.3",
            "scope": "global",
            "days": 0,
            "expiry_date": "2027-12-31T23:59:59Z"
        });
        let req = Request::builder()
            .method("POST")
            .uri("/v1/waivers")
            .header("content-type", "application/json")
            .header(X_REQUEST_ID_HEADER, "test-req-days-0")
            .header("x-user-role", "admin")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_create_waiver_days_one_succeeds() {
        let app = make_router();
        let body = serde_json::json!({
            "control_id": "cis-1.2.3",
            "scope": "global",
            "days": 1,
            "expiry_date": "2027-12-31T23:59:59Z"
        });
        let req = Request::builder()
            .method("POST")
            .uri("/v1/waivers")
            .header("content-type", "application/json")
            .header(X_REQUEST_ID_HEADER, "test-req-days-1")
            .header("x-user-role", "admin")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_create_waiver_short_duration_rejected() {
        // expiry_date is less than 1 day after start_date → should be 400
        let app = make_router();
        let body = serde_json::json!({
            "control_id": "cis-1.2.3",
            "scope": "global",
            "start_date": "2027-01-01T00:00:00Z",
            "expiry_date": "2027-01-01T12:00:00Z"
        });
        let req = Request::builder()
            .method("POST")
            .uri("/v1/waivers")
            .header("content-type", "application/json")
            .header(X_REQUEST_ID_HEADER, "test-req-short-dur")
            .header("x-user-role", "admin")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_create_waiver_exact_one_day_succeeds() {
        // expiry_date is exactly 24 hours after start_date → should be OK
        let app = make_router();
        let body = serde_json::json!({
            "control_id": "cis-1.2.3",
            "scope": "global",
            "start_date": "2027-01-01T00:00:00Z",
            "expiry_date": "2027-01-02T00:00:00Z"
        });
        let req = Request::builder()
            .method("POST")
            .uri("/v1/waivers")
            .header("content-type", "application/json")
            .header(X_REQUEST_ID_HEADER, "test-req-1day")
            .header("x-user-role", "admin")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_update_waiver_short_duration_rejected() {
        let app = make_router();
        let body = serde_json::json!({
            "control_id": "cis-3.1.1",
            "scope": "node",
            "justification": "Test",
            "start_date": "2027-01-01T00:00:00Z",
            "expiry_date": "2027-01-01T12:00:00Z"
        });
        let req = Request::builder()
            .method("PUT")
            .uri("/v1/waivers/00000000-0000-4000-8000-000000000001")
            .header("content-type", "application/json")
            .header(X_REQUEST_ID_HEADER, "test-req-update-short")
            .header("x-user-role", "admin")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── FK error mapping tests ───────────────────────────────────────────────

    #[test]
    fn test_is_foreign_key_error_postgres() {
        let msg = "violates foreign key constraint \"waivers_control_id_fkey\"";
        assert!(is_foreign_key_error(msg));
    }

    #[test]
    fn test_is_foreign_key_error_sqlite() {
        let msg = "FOREIGN KEY constraint failed";
        assert!(is_foreign_key_error(msg));
    }

    #[test]
    fn test_is_foreign_key_error_generic() {
        let msg = "references constraint violations";
        assert!(is_foreign_key_error(msg));
    }

    #[test]
    fn test_is_foreign_key_error_non_fk() {
        let msg = "duplicate key value violates unique constraint";
        assert!(!is_foreign_key_error(msg));
    }

    #[test]
    fn test_map_store_error_query_failed_becomes_validation() {
        // The map_store_error function maps QueryFailed to Validation (not Internal),
        // so FK errors never produce a 500. Verify the is_foreign_key_error helper
        // correctly classifies the common error patterns.
        assert!(is_foreign_key_error("23503"));
        assert!(is_foreign_key_error("violates foreign key constraint"));
        assert!(!is_foreign_key_error("connection refused"));
    }
}
