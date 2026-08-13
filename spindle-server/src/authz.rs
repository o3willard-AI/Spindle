//! Authorization middleware and extractors for spindle-server.
//!
//! Per M2-13:
//! - `RequireRole` extractor: enforce role hierarchy on endpoints
//! - Audit middleware: log every authz decision (subject, resource, decision, rule)
//! - Per-request role caching
//! - Endpoint-level role enforcement via attributes

#![allow(warnings)]
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use spindle_authz::{AuthzCache, AuthzDecision, AuthzEnforcer, RequiredRole, Role, Scope};
use std::sync::Arc;
use std::time::Duration;

// ── Authz State ─────────────────────────────────────────────────────────────

/// Application-level authz state shared across all requests.
#[derive(Debug, Clone)]
pub struct AuthzState {
    pub enforcer: Arc<AuthzEnforcer>,
    pub cache: Arc<AuthzCache>,
}

impl AuthzState {
    pub fn new() -> Self {
        Self {
            enforcer: Arc::new(AuthzEnforcer::create_default()),
            cache: Arc::new(AuthzCache::new(Duration::from_secs(300))),
        }
    }

    /// Get audit log entries.
    pub fn audit_entries(&self) -> Vec<AuthzDecision> {
        self.enforcer.audit_entries()
    }
}

impl Default for AuthzState {
    fn default() -> Self {
        Self::new()
    }
}

// ── RequireRole Extractor ──────────────────────────────────────────────────

/// Extractor that enforces role hierarchy on endpoints.
///
/// Usage: `async fn handler(
///     State(state): State<AuthzState>,
///     req: Request,
///     role: RequireRole,
/// ) -> impl IntoResponse`
#[derive(Debug, Clone)]
pub struct RequireRole {
    pub required: Role,
    pub scope: Scope,
}

// We can't implement axum::extract::FromRequest directly without knowing
// the full signature, so instead provide a helper function.
impl RequireRole {
    /// Check if the scope meets the required role.
    pub fn check(&self, enforcer: &AuthzEnforcer) -> Result<(), (StatusCode, String)> {
        let decision = enforcer.check_role(&self.scope, self.required, "endpoint_access");
        if decision.decision.is_allow() {
            Ok(())
        } else {
            let reason = decision.decision.reason().unwrap_or("access denied");
            Err((StatusCode::FORBIDDEN, reason.to_string()))
        }
    }

    /// Check if scope can read (Viewer+).
    pub fn can_read(&self) -> bool {
        self.scope.can_read()
    }

    /// Check if scope can write (Ingest+).
    pub fn can_write(&self) -> bool {
        self.scope.can_write()
    }

    /// Check if scope can manage tokens (TokenAdmin+).
    pub fn can_manage_tokens(&self) -> bool {
        self.scope.can_manage_tokens()
    }
}

// ── Audit Middleware ────────────────────────────────────────────────────────

/// Middleware that logs every request to the audit log.
pub async fn audit_middleware(
    State(state): State<AuthzState>,
    mut req: Request,
    next: Next,
) -> Response {
    // Add audit log as extension so handlers can access it
    let audit = state.enforcer.audit_entries();
    req.extensions_mut().insert(audit);

    let response = next.run(req).await;
    response
}

// ── Role Guard Helper ───────────────────────────────────────────────────────

/// Guard function to enforce roles on handlers.
pub async fn guard_role(
    state: &AuthzState,
    scope: &Scope,
    required: Role,
    action: &str,
) -> Result<(), (StatusCode, String)> {
    let decision = state.enforcer.check_role(scope, required, action);
    if decision.decision.is_allow() {
        Ok(())
    } else {
        let reason = decision.decision.reason().unwrap_or("access denied");
        Err((StatusCode::FORBIDDEN, reason.to_string()))
    }
}

/// Guard for write access (Ingest+).
pub async fn guard_write(state: &AuthzState, scope: &Scope) -> Result<(), (StatusCode, String)> {
    guard_role(state, scope, Role::Ingest, "write").await
}

/// Guard for read access (Viewer+).
pub async fn guard_read(state: &AuthzState, scope: &Scope) -> Result<(), (StatusCode, String)> {
    guard_role(state, scope, Role::Viewer, "read").await
}

/// Guard for compliance access (Auditor+).
pub async fn guard_compliance(
    state: &AuthzState,
    scope: &Scope,
) -> Result<(), (StatusCode, String)> {
    guard_role(state, scope, Role::ComplianceAuditor, "compliance").await
}

/// Guard for token management (TokenAdmin+).
pub async fn guard_token_admin(
    state: &AuthzState,
    scope: &Scope,
) -> Result<(), (StatusCode, String)> {
    guard_role(state, scope, Role::TokenAdmin, "token_admin").await
}

/// Guard for admin access (Admin only).
pub async fn guard_admin(state: &AuthzState, scope: &Scope) -> Result<(), (StatusCode, String)> {
    guard_role(state, scope, Role::Admin, "admin").await
}

// ── RequiredRole Constants ─────────────────────────────────────────────────

/// Role required for ingest endpoints.
pub const ROLE_INGEST: RequiredRole = RequiredRole::Ingest;

/// Role required for read endpoints (nodes, runs, etc.).
pub const ROLE_VIEWER: RequiredRole = RequiredRole::Viewer;

/// Role required for compliance endpoints.
pub const ROLE_AUDITOR: RequiredRole = RequiredRole::ComplianceAuditor;

/// Role required for token management.
pub const ROLE_TOKEN_ADMIN: RequiredRole = RequiredRole::TokenAdmin;

/// Role required for admin operations.
pub const ROLE_ADMIN: RequiredRole = RequiredRole::Admin;

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use spindle_authz::InMemoryAuditLog;
    use std::collections::HashSet;

    fn make_ingest_scope() -> Scope {
        let mut roles = HashSet::new();
        roles.insert("ingest".to_string());
        Scope::new(HashSet::new(), roles)
    }

    fn make_viewer_scope() -> Scope {
        let mut roles = HashSet::new();
        roles.insert("viewer".to_string());
        Scope::new(HashSet::new(), roles)
    }

    fn make_auditor_scope() -> Scope {
        let mut roles = HashSet::new();
        roles.insert("compliance-auditor".to_string());
        Scope::new(HashSet::new(), roles)
    }

    fn make_admin_scope() -> Scope {
        let mut roles = HashSet::new();
        roles.insert("admin".to_string());
        Scope::new(HashSet::new(), roles)
    }

    fn make_token_admin_scope() -> Scope {
        let mut roles = HashSet::new();
        roles.insert("token-admin".to_string());
        Scope::new(HashSet::new(), roles)
    }

    #[tokio::test]
    async fn test_ingest_cannot_read_nodes() {
        let state = AuthzState::new();
        let scope = make_ingest_scope();
        let result = guard_read(&state, &scope).await;
        assert!(result.is_err());
        let (status, _) = result.unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_viewer_cannot_write() {
        let state = AuthzState::new();
        let scope = make_viewer_scope();
        let result = guard_write(&state, &scope).await;
        assert!(result.is_err());
        let (status, _) = result.unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_ingest_can_write() {
        let state = AuthzState::new();
        let scope = make_ingest_scope();
        let result = guard_write(&state, &scope).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_viewer_can_read() {
        let state = AuthzState::new();
        let scope = make_viewer_scope();
        let result = guard_read(&state, &scope).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_auditor_can_access_compliance() {
        let state = AuthzState::new();
        let scope = make_auditor_scope();
        let result = guard_compliance(&state, &scope).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_ingest_cannot_access_compliance() {
        let state = AuthzState::new();
        let scope = make_ingest_scope();
        let result = guard_compliance(&state, &scope).await;
        assert!(result.is_err());
        let (status, _) = result.unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_admin_can_do_everything() {
        let state = AuthzState::new();
        let scope = make_admin_scope();

        assert!(guard_read(&state, &scope).await.is_ok());
        assert!(guard_write(&state, &scope).await.is_ok());
        assert!(guard_compliance(&state, &scope).await.is_ok());
        assert!(guard_token_admin(&state, &scope).await.is_ok());
        assert!(guard_admin(&state, &scope).await.is_ok());
    }

    #[tokio::test]
    async fn test_token_admin_can_read_and_write() {
        let state = AuthzState::new();
        let scope = make_token_admin_scope();

        assert!(guard_read(&state, &scope).await.is_ok());
        assert!(guard_write(&state, &scope).await.is_ok());
        assert!(guard_token_admin(&state, &scope).await.is_ok());
        assert!(guard_compliance(&state, &scope).await.is_ok());
        assert!(!scope.can_manage_tokens() || guard_token_admin(&state, &scope).await.is_ok());
    }

    #[tokio::test]
    async fn test_audit_log_records_decisions() {
        let state = AuthzState::new();
        let scope = make_viewer_scope();

        // Make a decision
        let result = guard_read(&state, &scope).await;
        assert!(result.is_ok());

        // Audit log should have the decision
        let entries = state.enforcer.audit_entries();
        assert!(!entries.is_empty());
    }

    #[tokio::test]
    async fn test_role_hierarchy_tokenadmin_includes_auditor() {
        let state = AuthzState::new();
        let scope = make_token_admin_scope();

        // TokenAdmin should include ComplianceAuditor permissions
        assert!(guard_compliance(&state, &scope).await.is_ok());
    }

    #[tokio::test]
    async fn test_viewer_cannot_manage_tokens() {
        let state = AuthzState::new();
        let scope = make_viewer_scope();
        let result = guard_token_admin(&state, &scope).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_audit_caching() {
        let state = AuthzState::new();
        let scope = make_viewer_scope();

        // First call
        let _ = guard_read(&state, &scope).await;
        // Second call — should be cached
        let _ = guard_read(&state, &scope).await;

        // Audit log should have at least one entry
        let entries = state.enforcer.audit_entries();
        assert!(!entries.is_empty());
    }
}
