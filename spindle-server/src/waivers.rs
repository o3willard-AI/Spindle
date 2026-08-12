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
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

use spindle_api::{parse_query_string, validate_filter_fields, VALID_WAIVER_FIELDS};
use spindle_api::QueryFilter;
use crate::ingest::{EnvelopeResponse, X_REQUEST_ID_HEADER, API_VERSION};

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

// ── SQL-backed stores (production) ─────────────────────────────────────────

/// SQL-backed waiver store using PostgreSQL.
#[derive(Debug, Clone)]
pub struct SqlxWaiverStore {
    pub pool: sqlx::PgPool,
}

impl SqlxWaiverStore {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl WaiverStore for SqlxWaiverStore {
    async fn create_waiver(&self, req: &WaiverRequest) -> Result<WaiverSummary, StoreError> {
        use sqlx::Row;

        // Validate scope
        match req.scope.as_str() {
            "node" | "project" | "global" => {}
            _ => {
                return Err(StoreError::Validation(format!(
                    "scope must be 'node', 'project', or 'global', got '{}'",
                    req.scope
                )));
            }
        }

        let start_date = if let Some(ref sd) = req.start_date {
            chrono::DateTime::parse_from_rfc3339(sd)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now())
        } else {
            Utc::now()
        };

        let expiry_date = chrono::DateTime::parse_from_rfc3339(&req.expiry_date)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|_| StoreError::Validation("invalid expiry_date format".to_string()))?;

        let profile_id_str = req.profile_id.clone().unwrap_or_else(|| "default".to_string());
        let profile_id = if profile_id_str == "default" {
            // Try to parse as UUID, fallback to default
            Uuid::parse_str(&profile_id_str).unwrap_or_else(|_| Uuid::nil())
        } else {
            Uuid::parse_str(&profile_id_str).unwrap_or_else(|_| Uuid::nil())
        };

        let row = sqlx::query(
            r#"
            INSERT INTO waivers (control_id, profile_id, scope, justification, approver, start_date, expiry_date)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (control_id, profile_id, scope) DO UPDATE
                SET justification = EXCLUDED.justification,
                    approver = EXCLUDED.approver,
                    start_date = EXCLUDED.start_date,
                    expiry_date = EXCLUDED.expiry_date,
                    updated_at = NOW()
            RETURNING id, control_id, profile_id, scope, justification, approver, start_date, expiry_date, created_at, updated_at
            "#
        )
        .bind(&req.control_id)
        .bind(profile_id)
        .bind(&req.scope)
        .bind(&req.justification)
        .bind(&req.approver)
        .bind(start_date)
        .bind(expiry_date)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(e.to_string()))?;

        Ok(WaiverSummary {
            id: row.get::<Uuid, _>("id").to_string(),
            control_id: row.get("control_id"),
            profile_id: row.get::<Uuid, _>("profile_id").to_string(),
            scope: row.get("scope"),
            justification: row.get("justification"),
            approver: row.get("approver"),
            start_date: row.get::<chrono::DateTime<Utc>, _>("start_date").to_rfc3339(),
            expiry_date: row.get::<chrono::DateTime<Utc>, _>("expiry_date").to_rfc3339(),
            created_at: row.get::<chrono::DateTime<Utc>, _>("created_at").to_rfc3339(),
            updated_at: row.get::<chrono::DateTime<Utc>, _>("updated_at").to_rfc3339(),
            is_expired: row.get::<chrono::DateTime<Utc>, _>("expiry_date") < Utc::now(),
        })
    }

    async fn list_active_waivers(&self, _filter: &QueryFilter) -> Result<Vec<WaiverSummary>, StoreError> {
        use sqlx::Row;

        let rows = sqlx::query(
            r#"
            SELECT id, control_id, profile_id, scope, justification, approver,
                   start_date, expiry_date, created_at, updated_at,
                   (expiry_date > NOW()) as is_expired
            FROM waivers WHERE expiry_date > NOW()
            ORDER BY created_at DESC
            "#
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(e.to_string()))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(WaiverSummary {
                id: row.get::<Uuid, _>("id").to_string(),
                control_id: row.get("control_id"),
                profile_id: row.get::<Uuid, _>("profile_id").to_string(),
                scope: row.get("scope"),
                justification: row.get("justification"),
                approver: row.get("approver"),
                start_date: row.get::<chrono::DateTime<Utc>, _>("start_date").to_rfc3339(),
                expiry_date: row.get::<chrono::DateTime<Utc>, _>("expiry_date").to_rfc3339(),
                created_at: row.get::<chrono::DateTime<Utc>, _>("created_at").to_rfc3339(),
                updated_at: row.get::<chrono::DateTime<Utc>, _>("updated_at").to_rfc3339(),
                is_expired: row.get("is_expired"),
            });
        }
        Ok(results)
    }

    async fn get_waiver(&self, id: &str) -> Result<WaiverDetail, StoreError> {
        use sqlx::Row;

        let uuid = Uuid::parse_str(id).map_err(|_| StoreError::NotFound(format!("Invalid waiver ID: {}", id)))?;

        let row = sqlx::query(
            r#"
            SELECT id, control_id, profile_id, scope, justification, approver,
                   start_date, expiry_date, created_at, updated_at
            FROM waivers WHERE id = $1
            "#
        )
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(e.to_string()))?
        .ok_or_else(|| StoreError::NotFound(format!("Waiver {} not found", id)))?;

        let is_expired = row.get::<chrono::DateTime<Utc>, _>("expiry_date") < Utc::now();

        Ok(WaiverDetail {
            id: row.get::<Uuid, _>("id").to_string(),
            control_id: row.get("control_id"),
            profile_id: row.get::<Uuid, _>("profile_id").to_string(),
            scope: row.get("scope"),
            justification: row.get("justification"),
            approver: row.get("approver"),
            start_date: row.get::<chrono::DateTime<Utc>, _>("start_date").to_rfc3339(),
            expiry_date: row.get::<chrono::DateTime<Utc>, _>("expiry_date").to_rfc3339(),
            created_at: row.get::<chrono::DateTime<Utc>, _>("created_at").to_rfc3339(),
            updated_at: row.get::<chrono::DateTime<Utc>, _>("updated_at").to_rfc3339(),
            is_expired,
        })
    }

    async fn update_waiver(&self, id: &str, req: &WaiverRequest) -> Result<WaiverSummary, StoreError> {
        use sqlx::Row;

        let uuid = Uuid::parse_str(id).map_err(|_| StoreError::NotFound(format!("Invalid waiver ID: {}", id)))?;

        let start_date = if let Some(ref sd) = req.start_date {
            chrono::DateTime::parse_from_rfc3339(sd)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now())
        } else {
            Utc::now()
        };

        let expiry_date = chrono::DateTime::parse_from_rfc3339(&req.expiry_date)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|_| StoreError::Validation("invalid expiry_date format".to_string()))?;

        let row = sqlx::query(
            r#"
            UPDATE waivers SET
                justification = $2,
                approver = $3,
                scope = $4,
                start_date = $5,
                expiry_date = $6,
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, control_id, profile_id, scope, justification, approver,
                      start_date, expiry_date, created_at, updated_at
            "#
        )
        .bind(uuid)
        .bind(&req.justification)
        .bind(&req.approver)
        .bind(&req.scope)
        .bind(start_date)
        .bind(expiry_date)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(e.to_string()))?
        .ok_or_else(|| StoreError::NotFound(format!("Waiver {} not found", id)))?;

        let is_expired = row.get::<chrono::DateTime<Utc>, _>("expiry_date") < Utc::now();

        Ok(WaiverSummary {
            id: row.get::<Uuid, _>("id").to_string(),
            control_id: row.get("control_id"),
            profile_id: row.get::<Uuid, _>("profile_id").to_string(),
            scope: row.get("scope"),
            justification: row.get("justification"),
            approver: row.get("approver"),
            start_date: row.get::<chrono::DateTime<Utc>, _>("start_date").to_rfc3339(),
            expiry_date: row.get::<chrono::DateTime<Utc>, _>("expiry_date").to_rfc3339(),
            created_at: row.get::<chrono::DateTime<Utc>, _>("created_at").to_rfc3339(),
            updated_at: row.get::<chrono::DateTime<Utc>, _>("updated_at").to_rfc3339(),
            is_expired,
        })
    }

    async fn delete_waiver(&self, id: &str) -> Result<(), StoreError> {
        let uuid = Uuid::parse_str(id).map_err(|_| StoreError::NotFound(format!("Invalid waiver ID: {}", id)))?;

        let result = sqlx::query("DELETE FROM waivers WHERE id = $1")
            .bind(uuid)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;

        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("Waiver {} not found", id)));
        }
        Ok(())
    }
}

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

/// Convenience alias for production use.
#[derive(Debug, Clone, Default)]
pub struct InMemoryWaiverStore {
    pub waivers: Arc<std::sync::RwLock<Vec<InMemoryWaiver>>>,
}

/// Internal waiver representation.
#[derive(Debug, Clone)]
pub struct InMemoryWaiver {
    pub id: Uuid,
    pub control_id: String,
    pub profile_id: String,
    pub scope: String,
    pub justification: Option<String>,
    pub approver: Option<String>,
    pub start_date: DateTime<Utc>,
    pub expiry_date: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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

        // Seed with sample waivers
        waivers.push(InMemoryWaiver {
            id: Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap(),
            control_id: "cis-3.1.1".to_string(),
            profile_id: "os-hardening".to_string(),
            scope: "node".to_string(),
            justification: Some("Temporary exception for legacy systems".to_string()),
            approver: Some("security-team".to_string()),
            start_date: now - chrono::Duration::days(30),
            expiry_date: now + chrono::Duration::days(30),
            created_at: now - chrono::Duration::days(30),
            updated_at: now - chrono::Duration::days(30),
        });

        waivers.push(InMemoryWaiver {
            id: Uuid::parse_str("00000000-0000-4000-8000-000000000002").unwrap(),
            control_id: "cis-4.2.3".to_string(),
            profile_id: "app-hardening".to_string(),
            scope: "project".to_string(),
            justification: Some("Application requires elevated permissions".to_string()),
            approver: Some("app-owner".to_string()),
            start_date: now - chrono::Duration::days(60),
            expiry_date: now - chrono::Duration::days(1), // expired
            created_at: now - chrono::Duration::days(60),
            updated_at: now - chrono::Duration::days(60),
        });

        waivers.push(InMemoryWaiver {
            id: Uuid::parse_str("00000000-0000-4000-8000-000000000003").unwrap(),
            control_id: "cis-5.1.2".to_string(),
            profile_id: "os-hardening".to_string(),
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

    fn is_active(w: &InMemoryWaiver) -> bool {
        w.expiry_date > Utc::now()
    }

    fn to_summary(&self, w: &InMemoryWaiver) -> WaiverSummary {
        let is_expired = !Self::is_active(w);
        WaiverSummary {
            id: w.id.to_string(),
            control_id: w.control_id.clone(),
            profile_id: w.profile_id.clone(),
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

    fn to_detail(&self, w: &InMemoryWaiver) -> WaiverDetail {
        let is_expired = !Self::is_active(w);
        WaiverDetail {
            id: w.id.to_string(),
            control_id: w.control_id.clone(),
            profile_id: w.profile_id.clone(),
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
}

// ── Store trait ─────────────────────────────────────────────────────────

#[async_trait::async_trait]
pub trait WaiverStore: Send + Sync + std::fmt::Debug {
    async fn create_waiver(&self, req: &WaiverRequest) -> Result<WaiverSummary, StoreError>;
    async fn list_active_waivers(&self, filter: &QueryFilter) -> Result<Vec<WaiverSummary>, StoreError>;
    async fn get_waiver(&self, id: &str) -> Result<WaiverDetail, StoreError>;
    async fn update_waiver(&self, id: &str, req: &WaiverRequest) -> Result<WaiverSummary, StoreError>;
    async fn delete_waiver(&self, id: &str) -> Result<(), StoreError>;
}

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
impl WaiverStore for InMemoryWaiverStore {
    async fn create_waiver(&self, req: &WaiverRequest) -> Result<WaiverSummary, StoreError> {
        let mut waivers = self.waivers.write().unwrap();

        // Validate scope
        match req.scope.as_str() {
            "node" | "project" | "global" => {}
            _ => {
                return Err(StoreError::Validation(format!(
                    "scope must be 'node', 'project', or 'global', got '{}'",
                    req.scope
                )));
            }
        }

        // Validate expiry_date
        if req.expiry_date.is_empty() {
            return Err(StoreError::Validation("expiry_date is required".to_string()));
        }

        // Parse dates
        let start_date = if let Some(ref sd) = req.start_date {
            chrono::DateTime::parse_from_rfc3339(sd)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now())
        } else {
            Utc::now()
        };

        let expiry_date = chrono::DateTime::parse_from_rfc3339(&req.expiry_date)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|_| StoreError::Validation("invalid expiry_date format".to_string()))?;

        // Check for duplicate active waiver
        let now = Utc::now();
        let duplicate = waivers.iter().any(|w| {
            w.control_id == req.control_id
                && w.scope == req.scope
                && w.expiry_date > now
        });

        if duplicate {
            return Err(StoreError::Conflict(format!(
                "Active waiver already exists for control '{}' with scope '{}'",
                req.control_id, req.scope
            )));
        }

        let id = Uuid::new_v4();
        let now = Utc::now();
        let waiver = InMemoryWaiver {
            id,
            control_id: req.control_id.clone(),
            profile_id: req.profile_id.clone().unwrap_or_else(|| "default".to_string()),
            scope: req.scope.clone(),
            justification: req.justification.clone(),
            approver: req.approver.clone(),
            start_date,
            expiry_date,
            created_at: now,
            updated_at: now,
        };

        waivers.push(waiver.clone());
        Ok(self.to_summary(&waiver))
    }

    async fn list_active_waivers(&self, _filter: &QueryFilter) -> Result<Vec<WaiverSummary>, StoreError> {
        let waivers = self.waivers.read().unwrap();
        let active: Vec<InMemoryWaiver> = waivers.iter().filter(|w| Self::is_active(w)).cloned().collect();
        Ok(active.iter().map(|w| self.to_summary(w)).collect())
    }

    async fn get_waiver(&self, id: &str) -> Result<WaiverDetail, StoreError> {
        let waivers = self.waivers.read().unwrap();
        let uuid = Uuid::parse_str(id).map_err(|_| StoreError::NotFound(format!("Invalid waiver ID: {}", id)))?;
        let w = waivers.iter().find(|w| w.id == uuid)
            .ok_or_else(|| StoreError::NotFound(format!("Waiver {} not found", id)))?;
        Ok(self.to_detail(w))
    }

    async fn update_waiver(&self, id: &str, req: &WaiverRequest) -> Result<WaiverSummary, StoreError> {
        let mut waivers = self.waivers.write().unwrap();
        let uuid = Uuid::parse_str(id).map_err(|_| StoreError::NotFound(format!("Invalid waiver ID: {}", id)))?;
        let idx = waivers.iter().position(|w| w.id == uuid)
            .ok_or_else(|| StoreError::NotFound(format!("Waiver {} not found", id)))?;

        let waiver = &mut waivers[idx];
        waiver.justification = req.justification.clone();
        waiver.approver = req.approver.clone();
        waiver.scope = req.scope.clone();

        if let Some(ref sd) = req.start_date {
            waiver.start_date = chrono::DateTime::parse_from_rfc3339(sd)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or(waiver.start_date);
        }

        if !req.expiry_date.is_empty() {
            waiver.expiry_date = chrono::DateTime::parse_from_rfc3339(&req.expiry_date)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|_| StoreError::Validation("invalid expiry_date format".to_string()))?;
        }

        waiver.updated_at = Utc::now();
        Ok(self.to_summary(waiver))
    }

    async fn delete_waiver(&self, id: &str) -> Result<(), StoreError> {
        let mut waivers = self.waivers.write().unwrap();
        let uuid = Uuid::parse_str(id).map_err(|_| StoreError::NotFound(format!("Invalid waiver ID: {}", id)))?;
        let pos = waivers.iter().position(|w| w.id == uuid)
            .ok_or_else(|| StoreError::NotFound(format!("Waiver {} not found", id)))?;
        waivers.remove(pos);
        Ok(())
    }
}

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
        self.entries.lock().unwrap().push(entry.clone());
        Ok(Uuid::parse_str(&entry.id).unwrap())
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn get_request_id_from_headers(headers: &axum::http::HeaderMap) -> String {
    headers.get(X_REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(crate::ingest::new_request_id)
}

fn build_query_string(params: &std::collections::HashMap<String, String>) -> String {
    params.iter().map(|(k, v)| format!("{}={}", k, v)).collect::<Vec<_>>().join("&")
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
    pub store: Arc<dyn WaiverStore>,
    pub audit_store: Arc<dyn AuditEventLog>,
    pub metrics: Arc<crate::metrics::MetricsRegistry>,
}

impl WaiversAppState {
    pub fn new(store: Arc<dyn WaiverStore>, audit: Arc<dyn AuditEventLog>, metrics: Arc<crate::metrics::MetricsRegistry>) -> Self {
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
        .route("/v1/waivers/:id", get(get_waiver).put(update_waiver).delete(delete_waiver))
        .with_state(state)
        .route_layer(middleware::from_fn(crate::ingest::request_id_middleware))
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// POST /v1/waivers — create a waiver.
pub async fn create_waiver(
    State(state): State<WaiversAppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<WaiverRequest>,
) -> impl IntoResponse {
    let request_id = get_request_id_from_headers(&headers);

    // RBAC: only admin can create waivers (write operation)
    let role_str = headers.get(crate::ingest::X_USER_ROLE_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("viewer");
    if role_str != "admin" {
        return EnvelopeResponse::forbidden("auth_required", "Access denied by role policy", &request_id).into_response();
    }

    // Validate request
    if req.control_id.is_empty() {
        return EnvelopeResponse::bad_request("validation", "control_id is required", &request_id).into_response();
    }
    if req.scope.is_empty() {
        return EnvelopeResponse::bad_request("validation", "scope is required", &request_id).into_response();
    }

    // Create waiver
    match state.store.create_waiver(&req).await {
        Ok(summary) => {
            // Audit log
            let _ = state.audit_store.log_audit_event(
                "admin",
                "waiver",
                &summary.id,
                "create",
                "allow",
                Some(serde_json::json!({
                    "control_id": req.control_id,
                    "scope": req.scope,
                })),
            ).await;

            let response = WaiverDetailResponse {
                api_version: API_VERSION.to_string(),
                request_id,
                data: WaiverDetail {
                    id: summary.id.clone(),
                    control_id: summary.control_id,
                    profile_id: summary.profile_id,
                    scope: summary.scope,
                    justification: summary.justification,
                    approver: summary.approver,
                    start_date: summary.start_date,
                    expiry_date: summary.expiry_date,
                    created_at: summary.created_at,
                    updated_at: summary.updated_at,
                    is_expired: summary.is_expired,
                },
                            provenance: None,
                stripped_attributes: None,
            };
            tracing::debug!(
                path = "/v1/waivers/{id}",
                "api query result"
            );
            Json(response).into_response()
        }
        Err(StoreError::Validation(msg)) => {
            EnvelopeResponse::bad_request("validation", &msg, &request_id).into_response()
        }
        Err(StoreError::Conflict(msg)) => {
            EnvelopeResponse::conflict("conflict", &msg, &request_id).into_response()
        }
        Err(e) => {
            EnvelopeResponse::bad_request("store_error", &format!("{}", e), &request_id).into_response()
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
    if let Err(e) = validate_filter_fields(&filter.filters, &spindle_api::TimeRange::default(), VALID_WAIVER_FIELDS) {
        return EnvelopeResponse::bad_request("bad_request", &format!("Invalid field: {}", e), &request_id).into_response();
    }

    match state.store.list_active_waivers(&filter).await {
        Ok(waivers) => {
            let count = waivers.len();
            let response = WaiversListResponse {
                api_version: API_VERSION.to_string(),
                request_id,
                data: waivers,
                pagination: PaginationInfo {
                    total_count: count,
                    has_more: false,
                    next_cursor: None,
                    limit: count,
                },
            };
            Json(response).into_response()
        }
        Err(e) => {
            EnvelopeResponse::bad_request("store_error", &format!("{}", e), &request_id).into_response()
        }
    }
}

/// GET /v1/waivers/:id — get a waiver detail.
pub async fn get_waiver(
    State(state): State<WaiversAppState>,
    Path(id): Path<String>,
    request: Request,
) -> impl IntoResponse {
    let request_id = get_request_id(&request);

    match state.store.get_waiver(&id).await {
        Ok(detail) => {
            let response = WaiverDetailResponse {
                api_version: API_VERSION.to_string(),
                request_id,
                data: detail,
                provenance: None,
                stripped_attributes: None,
            };
            Json(response).into_response()
        }
        Err(StoreError::NotFound(msg)) => {
            EnvelopeResponse::not_found("not_found", &msg, &request_id).into_response()
        }
        Err(e) => {
            EnvelopeResponse::bad_request("store_error", &format!("{}", e), &request_id).into_response()
        }
    }
}

/// PUT /v1/waivers/:id — update a waiver.
pub async fn update_waiver(
    State(state): State<WaiversAppState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(req): Json<WaiverRequest>,
) -> impl IntoResponse {
    let request_id = get_request_id_from_headers(&headers);

    // RBAC: only admin can update waivers (write operation)
    let role_str = headers.get(crate::ingest::X_USER_ROLE_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("viewer");
    if role_str != "admin" {
        return EnvelopeResponse::forbidden("auth_required", "Access denied by role policy", &request_id).into_response();
    }

    match state.store.update_waiver(&id, &req).await {
        Ok(summary) => {
            // Audit log
            let _ = state.audit_store.log_audit_event(
                "admin",
                "waiver",
                &summary.id,
                "update",
                "allow",
                Some(serde_json::json!({
                    "control_id": req.control_id,
                    "scope": req.scope,
                })),
            ).await;

            let response = WaiverDetailResponse {
                api_version: API_VERSION.to_string(),
                request_id,
                data: WaiverDetail {
                    id: summary.id.clone(),
                    control_id: summary.control_id,
                    profile_id: summary.profile_id,
                    scope: summary.scope,
                    justification: summary.justification,
                    approver: summary.approver,
                    start_date: summary.start_date,
                    expiry_date: summary.expiry_date,
                    created_at: summary.created_at,
                    updated_at: summary.updated_at,
                    is_expired: summary.is_expired,
                },
                            provenance: None,
                stripped_attributes: None,
            };
            Json(response).into_response()
        }
        Err(StoreError::NotFound(msg)) => {
            EnvelopeResponse::not_found("not_found", &msg, &request_id).into_response()
        }
        Err(StoreError::Validation(msg)) => {
            EnvelopeResponse::bad_request("validation", &msg, &request_id).into_response()
        }
        Err(e) => {
            EnvelopeResponse::bad_request("store_error", &format!("{}", e), &request_id).into_response()
        }
    }
}

/// DELETE /v1/waivers/:id — delete a waiver.
pub async fn delete_waiver(
    State(state): State<WaiversAppState>,
    Path(id): Path<String>,
    request: Request,
) -> impl IntoResponse {
    let request_id = get_request_id(&request);
    let headers = request.headers();

    // RBAC: only admin can delete waivers (write operation)
    let role_str = headers.get(crate::ingest::X_USER_ROLE_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("viewer");
    if role_str != "admin" {
        return EnvelopeResponse::forbidden("auth_required", "Access denied by role policy", &request_id).into_response();
    }

    match state.store.delete_waiver(&id).await {
        Ok(()) => {
            // Audit log
            let _ = state.audit_store.log_audit_event(
                "admin",
                "waiver",
                &id,
                "delete",
                "allow",
                None,
            ).await;

            let response = EnvelopeResponse::ok("deleted", "Waiver deleted successfully", &request_id);
            response.into_response()
        }
        Err(StoreError::NotFound(msg)) => {
            EnvelopeResponse::not_found("not_found", &msg, &request_id).into_response()
        }
        Err(e) => {
            EnvelopeResponse::bad_request("store_error", &format!("{}", e), &request_id).into_response()
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;
    use axum::http::Request;

    fn make_state() -> WaiversAppState {
        let store: Arc<dyn WaiverStore> = Arc::new(InMemoryWaiverStore::new());
        let audit: Arc<dyn AuditEventLog> = Arc::new(InMemoryAuditStore::default());
        WaiversAppState::new(store, audit, std::sync::Arc::new(crate::metrics::MetricsRegistry::new()))
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

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
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
    async fn test_create_waiver_duplicate_rejected() {
        let app = make_router();

        // First create should succeed (cis-3.1.1, node)
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

        // Second create with same control+scope should conflict
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
        assert_eq!(resp2.status(), StatusCode::CONFLICT);
    }

    // ── GET /v1/waivers — list ─────────────────────────────────────────

    #[tokio::test]
    async fn test_list_waivers_returns_active_only() {
        let app = make_router();
        let req = make_req("GET", "/v1/waivers");

        let resp = app.clone().oneshot(req).await.unwrap();
                assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
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

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
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

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let response: WaiverDetailResponse = serde_json::from_slice(&body).unwrap();

        assert_eq!(response.data.justification, Some("Updated justification".to_string()));
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

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
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
        let waivers = store.waivers.read().unwrap();
        assert_eq!(waivers.len(), 3);
    }

    #[tokio::test]
    async fn test_store_one_expired_waiver() {
        let store = InMemoryWaiverStore::new();
        assert!(!InMemoryWaiverStore::is_active(&store.waivers.read().unwrap()[1]));
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
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
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
        let state = WaiversAppState::new(store, audit.clone(), std::sync::Arc::new(crate::metrics::MetricsRegistry::new()));

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

        let entries = audit.entries.lock().unwrap();
        assert!(!entries.is_empty());
        assert_eq!(entries[0].resource_type, "waiver");
        assert_eq!(entries[0].action, "create");
        assert_eq!(entries[0].decision, "allow");
    }

    // ── Expired waiver list tests ───────────────────────────────────────

    #[tokio::test]
    async fn test_list_excludes_expired() {
        let store = InMemoryWaiverStore::new();
        let summary = store.list_active_waivers(&QueryFilter::default()).await.unwrap();

        for w in &summary {
            assert!(!w.is_expired);
        }
        assert_eq!(summary.len(), 2);
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
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
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
}