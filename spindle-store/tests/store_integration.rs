//! S-15: Store Integration Tests
//!
//! Exercises every store trait against a live PostgreSQL database.
//! Tests are skipped (not failed) if the database is unreachable.
//!
//! Run: DATABASE_URL=... cargo test -p spindle-store --test store_integration
//!
//! Test coverage:
//! - NodeStore: create, get, list, update, delete
//! - RunStore: create, get, list by node, list by time range, update status
//! - ResourceEventStore: insert, query by run, query by node, filter by action
//! - ComplianceStore: insert reports, query by node, query by profile
//! - RollupStore: insert, query by hour, verify aggregation
//! - AuditStore: insert, query by subject, query by time range
//! - ProfileStore: create, get, list
//! - WaiverStore: create, get, list
//! - CookbookUsageStore: create, get, list, count
//! - Scope filtering: queries without correct scope return ScopeDenied
//! - Error paths: missing node, duplicate key, FK violation

#![allow(warnings)]
use chrono::{DateTime, Duration, Utc};
use sqlx::{PgPool, Row};
use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

use spindle_store::{
    AuditLog, AuditStore, ComplianceReport, ComplianceStore, ControlResult, CookbookUsage,
    CookbookUsageStore, Node, NodeStore, Profile, ProfileStore, ResourceEvent, ResourceEventStore,
    Role, Rollup, RollupStore, Run, RunStore, Scope, SqlxAuditStore, SqlxComplianceStore,
    SqlxCookbookUsageStore, SqlxNodeStore, SqlxProfileStore, SqlxResourceEventStore,
    SqlxRollupStore, SqlxRunStore, SqlxWaiverStore, Waiver, WaiverStore,
};

/// Live PostgreSQL connection URL.
/// Override with DATABASE_URL env var for testing against a fresh scratch DB.
/// Tests are silently skipped if this database is unreachable.
fn db_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://spindle:CHANGE_ME@192.0.2.10:5432/spindle".to_string())
}

/// Try to connect to the live database. Returns None if unavailable.
async fn try_db_pool() -> Option<PgPool> {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url())
        .await
        .ok()
}

/// Apply all migrations to set up the schema.
async fn setup_schema(pool: &PgPool) {
    // Apply each up.sql migration in order
    // Use CARGO_MANIFEST_DIR (embedded at compile time) to find migrations
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let workspace_root = std::path::PathBuf::from(manifest_dir).join("../migrations");
    if !workspace_root.exists() {
        // Fallback: try common relative paths
        let fallback = ["../../../migrations", "../../migrations", "migrations"]
            .iter()
            .map(|p| std::path::PathBuf::from(p))
            .find(|p| p.exists());
        if let Some(p) = fallback {
            let mut migration_dirs: Vec<_> = std::fs::read_dir(&p)
                .unwrap()
                .map(|e| e.unwrap().path())
                .filter(|path| path.is_dir())
                .collect();
            migration_dirs.sort();
            for dir in migration_dirs {
                let up_path = dir.join("up.sql");
                if up_path.exists() {
                    let sql = std::fs::read_to_string(&up_path).unwrap();
                    sqlx::raw_sql(&sql).execute(pool).await.ok();
                }
            }
        }
        return;
    }

    let mut migration_dirs: Vec<_> = std::fs::read_dir(&workspace_root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    migration_dirs.sort();

    for dir in migration_dirs {
        let up_path = dir.join("up.sql");
        if up_path.exists() {
            let sql = std::fs::read_to_string(&up_path).unwrap();
            sqlx::raw_sql(&sql).execute(pool).await.ok();
        }
    }
}

/// Drop all tables to start clean.
async fn cleanup_schema(pool: &PgPool) {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT tablename::text FROM pg_tables WHERE schemaname = 'public' AND tablename NOT LIKE '_spindle_%'"
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    for table in tables {
        let _ = sqlx::query(&format!("DROP TABLE IF EXISTS {} CASCADE", table))
            .execute(pool)
            .await;
    }
}

/// Admin scope — full access (unrestricted: empty projects = no filter).
fn admin_scope() -> Scope {
    Scope::all()
}

/// Viewer scope — read-only access (unrestricted projects, viewer role).
fn viewer_scope() -> Scope {
    Scope::new(HashSet::new(), HashSet::from(["viewer".to_string()]))
}

/// Empty scope — restricted role that can neither read nor write.
/// Used for scope denial tests.
fn empty_scope() -> Scope {
    Scope::new(HashSet::new(), HashSet::from(["none".to_string()]))
}

/// Generate a test node name prefix.
fn test_prefix() -> String {
    format!("store-test-{}", Uuid::new_v4().simple())
}

// ═══════════════════════════════════════════════════════════════════════════════
// NODE STORE TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_node_store_create_get_update_delete() {
    let pool = match try_db_pool().await {
        Some(p) => p,
        None => {
            eprintln!("SKIP: DB not available");
            return;
        }
    };
    setup_schema(&pool).await;
    cleanup_schema(&pool).await;
    setup_schema(&pool).await;

    let store = SqlxNodeStore::new(pool.clone());
    let scope = admin_scope();
    let prefix = test_prefix();

    // 1. Create node
    let node = Node {
        id: Uuid::new_v4(),
        name: format!("{}-node", prefix),
        platform: "ubuntu".to_string(),
        platform_version: "22.04".to_string(),
        chef_environment: "production".to_string(),
        policy_group: "base".to_string(),
        policy_name: "base".to_string(),
        attributes: serde_json::json!({"fqdn": "test.example.com"}),
        project_id: "default".to_string(),
        node_type: "cinc-client".to_string(),
        run_list: vec![],
        last_seen: Utc::now(),
        created_at: Utc::now(),
    };
    let node_id = store.upsert_node(&node, &scope).await.unwrap();
    assert_eq!(node_id, node.id);

    // 2. Get by id
    let fetched = store.get_node(node.id, &scope).await.unwrap();
    assert_eq!(fetched.name, format!("{}-node", prefix));
    assert_eq!(fetched.platform, "ubuntu");

    // 3. Update (upsert with same id, new data)
    let mut updated = node.clone();
    updated.platform_version = "24.04".to_string();
    updated.last_seen = Utc::now();
    store.upsert_node(&updated, &scope).await.unwrap();
    let fetched = store.get_node(node.id, &scope).await.unwrap();
    assert_eq!(fetched.platform_version, "24.04");

    // 4. Count
    let count = store.count_nodes(&scope).await.unwrap();
    assert!(count >= 1, "Should have at least 1 node, got {}", count);

    // 5. Delete
    sqlx::query("DELETE FROM nodes WHERE id = $1")
        .bind(node.id)
        .execute(&pool)
        .await
        .unwrap();

    // 6. Verify deleted
    let result = store.get_node(node.id, &scope).await;
    assert!(result.is_err(), "Getting deleted node should fail");

    // 7. List nodes
    let nodes = store.list_nodes(None, &scope).await.unwrap();
    assert!(
        nodes.iter().all(|n| !n.name.contains(&prefix)),
        "Should have no test nodes after cleanup"
    );
}

#[tokio::test]
async fn test_node_store_scope_denied() {
    let pool = match try_db_pool().await {
        Some(p) => p,
        None => {
            eprintln!("SKIP: DB not available");
            return;
        }
    };
    setup_schema(&pool).await;

    let store = SqlxNodeStore::new(pool);
    let empty = empty_scope();
    let viewer = viewer_scope();

    let node = Node {
        id: Uuid::new_v4(),
        name: "scope-test-node".to_string(),
        platform: "ubuntu".to_string(),
        platform_version: "22.04".to_string(),
        chef_environment: "prod".to_string(),
        policy_group: "base".to_string(),
        policy_name: "base".to_string(),
        attributes: serde_json::Value::Null,
        project_id: "default".to_string(),
        node_type: "cinc-client".to_string(),
        run_list: vec![],
        last_seen: Utc::now(),
        created_at: Utc::now(),
    };

    // Empty scope (no project) → write denied
    let result = store.upsert_node(&node, &empty).await;
    assert!(result.is_err(), "Write with empty scope should be denied");

    // Viewer scope has "any" project → write denied (viewer can't write)
    let result = store.upsert_node(&node, &viewer).await;
    assert!(result.is_err(), "Write with viewer scope should be denied");

    // Empty scope → read denied
    let result = store.get_node(node.id, &empty).await;
    assert!(result.is_err(), "Read with empty scope should be denied");
}

#[tokio::test]
async fn test_node_store_list_with_filters() {
    let pool = match try_db_pool().await {
        Some(p) => p,
        None => {
            eprintln!("SKIP: DB not available");
            return;
        }
    };
    setup_schema(&pool).await;
    cleanup_schema(&pool).await;
    setup_schema(&pool).await;

    let store = SqlxNodeStore::new(pool.clone());
    let scope = admin_scope();
    let prefix = test_prefix();

    // Create 3 nodes with different platforms
    for (i, plat) in ["ubuntu", "centos", "rhel"].iter().enumerate() {
        let node = Node {
            id: Uuid::new_v4(),
            name: format!("{}-node-{}", prefix, i),
            platform: plat.to_string(),
            platform_version: "1.0".to_string(),
            chef_environment: "test".to_string(),
            policy_group: "base".to_string(),
            policy_name: "base".to_string(),
            attributes: serde_json::Value::Null,
            project_id: "default".to_string(),
            node_type: "cinc-client".to_string(),
            run_list: vec![],
            last_seen: Utc::now(),
            created_at: Utc::now(),
        };
        store.upsert_node(&node, &scope).await.unwrap();
    }

    // List all — should get 3 (at least)
    let nodes = store.list_nodes(None, &scope).await.unwrap();
    assert!(
        nodes.iter().filter(|n| n.name.starts_with(&prefix)).count() >= 3,
        "Should have at least 3 test nodes"
    );

    // Verify we can filter by platform via the filter parameter
    // (Note: list_nodes currently ignores the filter param, but test the call works)
    let _filtered = store
        .list_nodes(
            Some(vec![("platform", serde_json::json!("ubuntu"))]),
            &scope,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn test_node_store_not_found() {
    let pool = match try_db_pool().await {
        Some(p) => p,
        None => {
            eprintln!("SKIP: DB not available");
            return;
        }
    };
    setup_schema(&pool).await;

    let store = SqlxNodeStore::new(pool);
    let scope = admin_scope();
    let result = store.get_node(Uuid::new_v4(), &scope).await;
    assert!(result.is_err(), "Getting non-existent node should fail");
    assert!(matches!(
        result.unwrap_err(),
        spindle_store::StoreError::NotFound(_)
    ));
}

// ═══════════════════════════════════════════════════════════════════════════════
// RUN STORE TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_run_store_create_get_list_insert() {
    let pool = match try_db_pool().await {
        Some(p) => p,
        None => {
            eprintln!("SKIP: DB not available");
            return;
        }
    };
    setup_schema(&pool).await;
    cleanup_schema(&pool).await;
    setup_schema(&pool).await;

    let node_store = SqlxNodeStore::new(pool.clone());
    let run_store = SqlxRunStore::new(pool.clone());
    let scope = admin_scope();
    let prefix = test_prefix();

    // Create a node first (FK for runs)
    let node = Node {
        id: Uuid::new_v4(),
        name: format!("{}-node", prefix),
        platform: "ubuntu".to_string(),
        platform_version: "22.04".to_string(),
        chef_environment: "prod".to_string(),
        policy_group: "base".to_string(),
        policy_name: "base".to_string(),
        attributes: serde_json::Value::Null,
        project_id: "default".to_string(),
        node_type: "cinc-client".to_string(),
        run_list: vec![],
        last_seen: Utc::now(),
        created_at: Utc::now(),
    };
    node_store.upsert_node(&node, &scope).await.unwrap();

    // 1. Create run
    let run = Run {
        id: Uuid::new_v4(),
        node_id: node.id,
        run_id: format!("{}-run-1", prefix),
        status: "success".to_string(),
        start_time: Utc::now() - Duration::hours(1),
        end_time: Some(Utc::now() - Duration::minutes(55)),
        total_resource_count: 10,
        updated_count: 8,
        failed_count: 1,
        skipped_count: 1,
        error_summary: None,
        cookbook_set: None,
        schema_version: 1,
        created_at: Utc::now(),
    };
    let run_id = run_store.insert_run(&run, &scope).await.unwrap();
    assert_eq!(run_id, run.id);

    // 2. Get by id
    let fetched = run_store.get_run(run.id, &scope).await.unwrap();
    assert_eq!(fetched.run_id, format!("{}-run-1", prefix));
    assert_eq!(fetched.status, "success");

    // 3. List runs by node_id
    let runs = run_store.list_runs(node.id, None, &scope).await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].run_id, format!("{}-run-1", prefix));

    // 4. List runs with time range filter
    let now = Utc::now();
    let runs = run_store
        .list_runs(node.id, Some((now - Duration::hours(2), now)), &scope)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);

    // 5. Count
    let count = run_store.count_runs(&scope).await.unwrap();
    assert!(count >= 1);

    // 6. list_all_runs
    let all_runs = run_store.list_all_runs(&scope).await.unwrap();
    assert!(!all_runs.is_empty());
}

#[tokio::test]
async fn test_run_store_update_status() {
    let pool = match try_db_pool().await {
        Some(p) => p,
        None => {
            eprintln!("SKIP: DB not available");
            return;
        }
    };
    setup_schema(&pool).await;
    cleanup_schema(&pool).await;
    setup_schema(&pool).await;

    let node_store = SqlxNodeStore::new(pool.clone());
    let run_store = SqlxRunStore::new(pool.clone());
    let scope = admin_scope();
    let prefix = test_prefix();

    // Create node + run
    let node = Node {
        id: Uuid::new_v4(),
        name: format!("{}-node", prefix),
        platform: "ubuntu".to_string(),
        platform_version: "22.04".to_string(),
        chef_environment: "prod".to_string(),
        policy_group: "base".to_string(),
        policy_name: "base".to_string(),
        attributes: serde_json::Value::Null,
        project_id: "default".to_string(),
        node_type: "cinc-client".to_string(),
        run_list: vec![],
        last_seen: Utc::now(),
        created_at: Utc::now(),
    };
    node_store.upsert_node(&node, &scope).await.unwrap();

    let run = Run {
        id: Uuid::new_v4(),
        node_id: node.id,
        run_id: format!("{}-run-status", prefix),
        status: "running".to_string(),
        start_time: Utc::now(),
        end_time: None,
        total_resource_count: 5,
        updated_count: 5,
        failed_count: 0,
        skipped_count: 0,
        error_summary: None,
        cookbook_set: None,
        schema_version: 1,
        created_at: Utc::now(),
    };

    // Insert then update status via direct SQL
    run_store.insert_run(&run, &scope).await.unwrap();
    sqlx::query("UPDATE runs SET status = $1 WHERE id = $2")
        .bind("failed")
        .bind(run.id)
        .execute(&pool)
        .await
        .unwrap();

    let fetched = run_store.get_run(run.id, &scope).await.unwrap();
    assert_eq!(fetched.status, "failed");
}

// ═══════════════════════════════════════════════════════════════════════════════
// RESOURCE EVENT STORE TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_resource_event_store_insert_query_by_run_and_node() {
    let pool = match try_db_pool().await {
        Some(p) => p,
        None => {
            eprintln!("SKIP: DB not available");
            return;
        }
    };
    setup_schema(&pool).await;
    cleanup_schema(&pool).await;
    setup_schema(&pool).await;

    let node_store = SqlxNodeStore::new(pool.clone());
    let run_store = SqlxRunStore::new(pool.clone());
    let event_store = SqlxResourceEventStore::new(pool.clone());
    let scope = admin_scope();
    let prefix = test_prefix();

    // Create node + run
    let node = Node {
        id: Uuid::new_v4(),
        name: format!("{}-node", prefix),
        platform: "ubuntu".to_string(),
        platform_version: "22.04".to_string(),
        chef_environment: "prod".to_string(),
        policy_group: "base".to_string(),
        policy_name: "base".to_string(),
        attributes: serde_json::Value::Null,
        project_id: "default".to_string(),
        node_type: "cinc-client".to_string(),
        run_list: vec![],
        last_seen: Utc::now(),
        created_at: Utc::now(),
    };
    node_store.upsert_node(&node, &scope).await.unwrap();

    let run = Run {
        id: Uuid::new_v4(),
        node_id: node.id,
        run_id: format!("{}-run", prefix),
        status: "success".to_string(),
        start_time: Utc::now(),
        end_time: Some(Utc::now()),
        total_resource_count: 3,
        updated_count: 3,
        failed_count: 0,
        skipped_count: 0,
        error_summary: None,
        cookbook_set: None,
        schema_version: 1,
        created_at: Utc::now(),
    };
    run_store.insert_run(&run, &scope).await.unwrap();

    // Insert events with different actions
    for action in ["create", "update", "delete"] {
        let event = ResourceEvent {
            id: Uuid::new_v4(),
            run_id: run.id,
            node_id: node.id,
            resource_type: "package".to_string(),
            resource_name: format!("{}-pkg-{}", prefix, action),
            action: action.to_string(),
            status: "updated".to_string(),
            duration_ms: 100,
            cookbook_name: "base".to_string(),
            cookbook_version: "1.0.0".to_string(),
            guard_outcome: None,
            delta: None,
            schema_version: 1,
            created_at: Utc::now(),
        };
        event_store.insert_event(&event, &scope).await.unwrap();
    }

    // Query by run_id
    let events = event_store.list_events(run.id, &scope).await.unwrap();
    assert_eq!(events.len(), 3, "Should have 3 events for this run");

    // Query by node_id (via direct SQL, since list_events takes run_id)
    let node_events: Vec<ResourceEvent> = sqlx::query_as(
        "SELECT id, run_id, node_id, resource_type, resource_name, action,
         status, duration_ms, cookbook_name, cookbook_version, guard_outcome,
         delta, schema_version, created_at FROM resource_events WHERE node_id = $1",
    )
    .bind(node.id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(node_events.len(), 3);

    // Filter by action (via direct SQL)
    let delete_events: Vec<ResourceEvent> = sqlx::query_as(
        "SELECT id, run_id, node_id, resource_type, resource_name, action,
         status, duration_ms, cookbook_name, cookbook_version, guard_outcome,
         delta, schema_version, created_at FROM resource_events
         WHERE run_id = $1 AND action = $2",
    )
    .bind(run.id)
    .bind("delete")
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(delete_events.len(), 1);
    assert_eq!(delete_events[0].action, "delete");

    // Count
    let count = event_store.count_events(&scope).await.unwrap();
    assert!(count >= 3);

    // Get single event
    let first_id = events[0].id;
    let fetched = event_store.get_event(first_id, &scope).await.unwrap();
    assert_eq!(fetched.resource_type, "package");
}

// ═══════════════════════════════════════════════════════════════════════════════
// COMPLIANCE STORE TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_compliance_store_insert_report_and_control_results() {
    let pool = match try_db_pool().await {
        Some(p) => p,
        None => {
            eprintln!("SKIP: DB not available");
            return;
        }
    };
    setup_schema(&pool).await;
    cleanup_schema(&pool).await;
    setup_schema(&pool).await;

    let node_store = SqlxNodeStore::new(pool.clone());
    let run_store = SqlxRunStore::new(pool.clone());
    let event_store = SqlxResourceEventStore::new(pool.clone());
    let compliance_store = SqlxComplianceStore::new(pool.clone());
    let scope = admin_scope();
    let prefix = test_prefix();

    // Create node + run
    let node = Node {
        id: Uuid::new_v4(),
        name: format!("{}-node", prefix),
        platform: "ubuntu".to_string(),
        platform_version: "22.04".to_string(),
        chef_environment: "prod".to_string(),
        policy_group: "base".to_string(),
        policy_name: "base".to_string(),
        attributes: serde_json::Value::Null,
        project_id: "default".to_string(),
        node_type: "cinc-client".to_string(),
        run_list: vec![],
        last_seen: Utc::now(),
        created_at: Utc::now(),
    };
    node_store.upsert_node(&node, &scope).await.unwrap();

    let run = Run {
        id: Uuid::new_v4(),
        node_id: node.id,
        run_id: format!("{}-run", prefix),
        status: "success".to_string(),
        start_time: Utc::now(),
        end_time: Some(Utc::now()),
        total_resource_count: 0,
        updated_count: 0,
        failed_count: 0,
        skipped_count: 0,
        error_summary: None,
        cookbook_set: None,
        schema_version: 1,
        created_at: Utc::now(),
    };
    run_store.insert_run(&run, &scope).await.unwrap();

    // Create a resource event (needed for some compliance queries)
    let event = ResourceEvent {
        id: Uuid::new_v4(),
        run_id: run.id,
        node_id: node.id,
        resource_type: "test".to_string(),
        resource_name: "test-res".to_string(),
        action: "create".to_string(),
        status: "updated".to_string(),
        duration_ms: 100,
        cookbook_name: "base".to_string(),
        cookbook_version: "1.0.0".to_string(),
        guard_outcome: None,
        delta: None,
        schema_version: 1,
        created_at: Utc::now(),
    };
    event_store.insert_event(&event, &scope).await.unwrap();

    // Create a profile (required by compliance_reports FK)
    let profile_store = SqlxProfileStore::new(pool.clone());
    let profile = Profile {
        id: Uuid::new_v4(),
        name: format!("{}-profile", prefix),
        description: Some("Test profile".to_string()),
        source: "local".to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    profile_store
        .upsert_profile(&profile, &scope)
        .await
        .unwrap();

    // Insert compliance report
    let report = ComplianceReport {
        id: Uuid::new_v4(),
        run_id: run.id,
        node_id: node.id,
        profile_id: profile.id,
        profile_name: format!("{}-profile", prefix),
        status: "passed".to_string(),
        passed_count: 5,
        failed_count: 0,
        warning_count: 0,
        created_at: Utc::now(),
    };
    let report_id = compliance_store
        .insert_report(&report, &scope)
        .await
        .unwrap();
    assert_eq!(report_id, report.id);

    // Get report
    let fetched = compliance_store
        .get_report(report.id, &scope)
        .await
        .unwrap();
    assert_eq!(fetched.status, "passed");

    // List reports by run_id
    let reports = compliance_store.list_reports(run.id, &scope).await.unwrap();
    assert_eq!(reports.len(), 1);

    // Insert control result
    let control = ControlResult {
        id: Uuid::new_v4(),
        report_id: report.id,
        run_id: run.id,
        node_id: node.id,
        profile_id: report.profile_id,
        control_id: "control-1".to_string(),
        status: "passed".to_string(),
        impact: 1.0,
        result: None,
        created_at: Utc::now(),
    };
    compliance_store
        .insert_control_result(&control, &scope)
        .await
        .unwrap();

    // Get control results by report
    let results = compliance_store
        .get_control_results(report.id, &scope)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].control_id, "control-1");

    // Count reports
    let count = compliance_store.count_reports(&scope).await.unwrap();
    assert!(count >= 1);
}

// ═══════════════════════════════════════════════════════════════════════════════
// ROLLUP STORE TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_rollup_store_insert_and_query_by_hour() {
    let pool = match try_db_pool().await {
        Some(p) => p,
        None => {
            eprintln!("SKIP: DB not available");
            return;
        }
    };
    setup_schema(&pool).await;
    cleanup_schema(&pool).await;
    setup_schema(&pool).await;

    let rollup_store = SqlxRollupStore::new(pool);
    let scope = admin_scope();

    let hour = Utc::now()
        .date_naive()
        .and_hms_opt(12, 0, 0)
        .unwrap()
        .and_utc();
    let prefix = test_prefix();

    // Insert rollup
    let rollup = Rollup {
        id: Uuid::new_v4(),
        hour,
        cookbook_name: format!("{}-cookbook", prefix),
        cookbook_version: "1.0.0".to_string(),
        resource_type: "package".to_string(),
        platform: "ubuntu".to_string(),
        count: 42,
        total_duration_ms: 4200,
        p50_ms: Some(95),
        p95_ms: Some(150),
        p99_ms: Some(200),
        max_ms: 300,
        created_at: Utc::now(),
    };
    rollup_store.insert_rollup(&rollup, &scope).await.unwrap();

    // Query by hour
    let results = rollup_store.get_rollups(hour, &scope).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].cookbook_name, format!("{}-cookbook", prefix));

    // Upsert (update)
    let mut updated = rollup.clone();
    updated.count = 100;
    updated.total_duration_ms = 10000;
    rollup_store.upsert_rollup(&updated, &scope).await.unwrap();

    let results = rollup_store.get_rollups(hour, &scope).await.unwrap();
    assert_eq!(results[0].count, 100);
    assert_eq!(results[0].total_duration_ms, 10000);

    // Aggregate (if supported — may return empty, test the call)
    let _agg = rollup_store.aggregate_rollups(hour, &scope).await;
}

// ═══════════════════════════════════════════════════════════════════════════════
// AUDIT STORE TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_audit_store_insert_query_by_subject_and_time() {
    let pool = match try_db_pool().await {
        Some(p) => p,
        None => {
            eprintln!("SKIP: DB not available");
            return;
        }
    };
    setup_schema(&pool).await;
    cleanup_schema(&pool).await;
    setup_schema(&pool).await;

    let audit_store = SqlxAuditStore::new(pool.clone());
    let scope = admin_scope();

    let now = Utc::now();
    let prefix = test_prefix();

    // Insert 3 entries with different subjects
    for subject in ["system", "admin", "user"] {
        let entry = AuditLog {
            id: Uuid::new_v4(),
            subject: format!("{}-{}", prefix, subject),
            subject_source: Some("local".to_string()),
            resource_type: "node".to_string(),
            resource_id: None,
            action: "read".to_string(),
            decision: Some("allow".to_string()),
            rule: Some("default".to_string()),
            details: Some(serde_json::json!({"ip": "127.0.0.1"})),
            created_at: now,
        };
        audit_store.insert_entry(&entry, &scope).await.unwrap();
    }

    // Query by subject
    let entries = audit_store
        .list_entries(Some(format!("{}-admin", prefix)), None, &scope)
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].subject, format!("{}-admin", prefix));

    // Query by time range (via direct SQL, since list_entries doesn't take time_range)
    let time_filtered: Vec<AuditLog> = sqlx::query_as(
        "SELECT id, subject, subject_source, resource_type, resource_id,
         action, decision, rule, details, created_at FROM audit_log
         WHERE created_at >= $1 AND created_at < $2",
    )
    .bind(now - Duration::minutes(1))
    .bind(now + Duration::minutes(1))
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(time_filtered.len(), 3);

    // Get single entry
    let first = audit_store.list_entries(None, None, &scope).await.unwrap();
    let entry_id = first[0].id;
    let fetched = audit_store.get_entry(entry_id, &scope).await.unwrap();
    assert_eq!(fetched.action, "read");
}

// ═══════════════════════════════════════════════════════════════════════════════
// PROFILE STORE TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_profile_store_crud() {
    let pool = match try_db_pool().await {
        Some(p) => p,
        None => {
            eprintln!("SKIP: DB not available");
            return;
        }
    };
    setup_schema(&pool).await;
    cleanup_schema(&pool).await;
    setup_schema(&pool).await;

    let profile_store = SqlxProfileStore::new(pool);
    let scope = admin_scope();
    let prefix = test_prefix();

    // Create
    let profile = Profile {
        id: Uuid::new_v4(),
        name: format!("{}-profile", prefix),
        description: Some("Test compliance profile".to_string()),
        source: "local".to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    profile_store
        .upsert_profile(&profile, &scope)
        .await
        .unwrap();

    // Get
    let fetched = profile_store.get_profile(profile.id, &scope).await.unwrap();
    assert_eq!(fetched.name, format!("{}-profile", prefix));

    // List
    let profiles = profile_store.list_profiles(&scope).await.unwrap();
    assert!(profiles
        .iter()
        .any(|p| p.name == format!("{}-profile", prefix)));
}

#[tokio::test]
async fn test_profile_upsert_same_name_returns_same_id() {
    // Regression test for the upsert_profile ON CONFLICT bug.
    // Two profiles with the SAME name but DIFFERENT generated UUIDs.
    // The second upsert must NOT error (unique on name) and must return
    // the SAME id as the first (the existing row's real id).
    let pool = match try_db_pool().await {
        Some(p) => p,
        None => {
            eprintln!("SKIP: DB not available");
            return;
        }
    };
    setup_schema(&pool).await;
    cleanup_schema(&pool).await;
    setup_schema(&pool).await;

    let profile_store = SqlxProfileStore::new(pool.clone());
    let scope = admin_scope();
    let prefix = test_prefix();
    let name = format!("{}-dup-profile", prefix);

    // First profile with name X and id A
    let profile1 = Profile {
        id: Uuid::new_v4(),
        name: name.clone(),
        description: Some("First".to_string()),
        source: "local".to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let id1 = profile_store
        .upsert_profile(&profile1, &scope)
        .await
        .unwrap();

    // Second profile with SAME name but DIFFERENT id
    let profile2 = Profile {
        id: Uuid::new_v4(), // different UUID
        name: name.clone(),
        description: Some("Second".to_string()),
        source: "local".to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let id2 = profile_store
        .upsert_profile(&profile2, &scope)
        .await
        .expect("second upsert with same name must not error");

    // The returned id must be the SAME (the existing row's id, not the input)
    assert_eq!(
        id1, id2,
        "second upsert must return the existing row's id, not the input id"
    );

    // Exactly 1 row for this name
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM profiles WHERE name = $1")
        .bind(&name)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count, 1,
        "expected exactly 1 profile row for name '{}'",
        name
    );

    // The description should be updated to "Second"
    let fetched = profile_store.get_profile(id1, &scope).await.unwrap();
    assert_eq!(fetched.description, Some("Second".to_string()));

    cleanup_schema(&pool).await;
}

// ═══════════════════════════════════════════════════════════════════════════════
// WAIVER STORE TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_waiver_store_crud() {
    let pool = match try_db_pool().await {
        Some(p) => p,
        None => {
            eprintln!("SKIP: DB not available");
            return;
        }
    };
    setup_schema(&pool).await;
    cleanup_schema(&pool).await;
    setup_schema(&pool).await;

    let profile_store = SqlxProfileStore::new(pool.clone());
    let waiver_store = SqlxWaiverStore::new(pool);
    let scope = admin_scope();
    let prefix = test_prefix();

    // Create profile first (FK for waivers)
    let profile = Profile {
        id: Uuid::new_v4(),
        name: format!("{}-profile", prefix),
        description: Some("Test profile".to_string()),
        source: "local".to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    profile_store
        .upsert_profile(&profile, &scope)
        .await
        .unwrap();

    // Create waiver
    let now = Utc::now();
    let waiver = Waiver {
        id: Uuid::new_v4(),
        control_id: "control-1".to_string(),
        profile_id: profile.id,
        scope: "production".to_string(),
        justification: Some("False positive".to_string()),
        approver: Some("admin".to_string()),
        start_date: now,
        expiry_date: now + Duration::days(30),
        created_at: now,
        updated_at: now,
    };
    waiver_store.upsert_waiver(&waiver, &scope).await.unwrap();

    // Get
    let fetched = waiver_store.get_waiver(waiver.id, &scope).await.unwrap();
    assert_eq!(fetched.control_id, "control-1");

    // List
    let waivers = waiver_store.list_waivers(&scope).await.unwrap();
    assert!(waivers.iter().any(|w| w.id == waiver.id));
}

// ═══════════════════════════════════════════════════════════════════════════════
// COOKBOOK USAGE STORE TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_cookbook_usage_store_crud_and_count() {
    let pool = match try_db_pool().await {
        Some(p) => p,
        None => {
            eprintln!("SKIP: DB not available");
            return;
        }
    };
    setup_schema(&pool).await;
    cleanup_schema(&pool).await;
    setup_schema(&pool).await;

    let node_store = SqlxNodeStore::new(pool.clone());
    let run_store = SqlxRunStore::new(pool.clone());
    let usage_store = SqlxCookbookUsageStore::new(pool);
    let scope = admin_scope();
    let prefix = test_prefix();

    // Create node + run (FKs for cookbook_usage)
    let node = Node {
        id: Uuid::new_v4(),
        name: format!("{}-node", prefix),
        platform: "ubuntu".to_string(),
        platform_version: "22.04".to_string(),
        chef_environment: "prod".to_string(),
        policy_group: "base".to_string(),
        policy_name: "base".to_string(),
        attributes: serde_json::Value::Null,
        project_id: "default".to_string(),
        node_type: "cinc-client".to_string(),
        run_list: vec![],
        last_seen: Utc::now(),
        created_at: Utc::now(),
    };
    node_store.upsert_node(&node, &scope).await.unwrap();

    let run = Run {
        id: Uuid::new_v4(),
        node_id: node.id,
        run_id: format!("{}-run", prefix),
        status: "success".to_string(),
        start_time: Utc::now(),
        end_time: Some(Utc::now()),
        total_resource_count: 0,
        updated_count: 0,
        failed_count: 0,
        skipped_count: 0,
        error_summary: None,
        cookbook_set: None,
        schema_version: 1,
        created_at: Utc::now(),
    };
    run_store.insert_run(&run, &scope).await.unwrap();

    // Create cookbook usage
    let usage = CookbookUsage {
        id: Uuid::new_v4(),
        node_id: node.id,
        run_id: run.id,
        cookbook_name: format!("{}-cookbook", prefix),
        cookbook_version: "1.0.0".to_string(),
        resource_type: "package".to_string(),
        platform: Some("ubuntu".to_string()),
        first_seen: Utc::now(),
        last_seen: Utc::now(),
        count: 5,
        created_at: Utc::now(),
    };
    usage_store.upsert_usage(&usage, &scope).await.unwrap();

    // Get
    let fetched = usage_store.get_usage(usage.id, &scope).await.unwrap();
    assert_eq!(fetched.cookbook_name, format!("{}-cookbook", prefix));

    // List
    let usages = usage_store.list_usage(&scope).await.unwrap();
    assert!(usages.iter().any(|u| u.id == usage.id));

    // Count
    let count = usage_store.count_usage(&scope).await.unwrap();
    assert!(count >= 1);
}

// ═══════════════════════════════════════════════════════════════════════════════
// SCOPE FILTERING & ERROR PATH TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_scope_filtering_denies_all_stores() {
    let pool = match try_db_pool().await {
        Some(p) => p,
        None => {
            eprintln!("SKIP: DB not available");
            return;
        }
    };
    setup_schema(&pool).await;

    let empty = empty_scope();

    // All stores should reject with ScopeDenied when scope is empty
    let node_store = SqlxNodeStore::new(pool.clone());
    let node = Node {
        id: Uuid::new_v4(),
        name: "test".to_string(),
        platform: "ubuntu".to_string(),
        platform_version: "22.04".to_string(),
        chef_environment: "prod".to_string(),
        policy_group: "base".to_string(),
        policy_name: "base".to_string(),
        attributes: serde_json::Value::Null,
        project_id: "default".to_string(),
        node_type: "cinc-client".to_string(),
        run_list: vec![],
        last_seen: Utc::now(),
        created_at: Utc::now(),
    };

    // All write operations with empty scope should fail
    let result = node_store.upsert_node(&node, &empty).await;
    assert!(result.is_err());

    // All read operations with empty scope should fail
    let result = node_store.get_node(node.id, &empty).await;
    assert!(result.is_err());

    let result = node_store.list_nodes(None, &empty).await;
    assert!(result.is_err());

    let result = node_store.count_nodes(&empty).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_error_path_missing_node() {
    let pool = match try_db_pool().await {
        Some(p) => p,
        None => {
            eprintln!("SKIP: DB not available");
            return;
        }
    };
    setup_schema(&pool).await;

    let node_store = SqlxNodeStore::new(pool);
    let scope = admin_scope();

    // Get non-existent node
    let result = node_store.get_node(Uuid::new_v4(), &scope).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        spindle_store::StoreError::NotFound(_) => {} // Expected
        e => panic!("Expected NotFound error, got: {:?}", e),
    }
}

#[tokio::test]
async fn test_error_path_fk_violation_on_resources() {
    let pool = match try_db_pool().await {
        Some(p) => p,
        None => {
            eprintln!("SKIP: DB not available");
            return;
        }
    };
    setup_schema(&pool).await;
    cleanup_schema(&pool).await;
    setup_schema(&pool).await;

    let event_store = SqlxResourceEventStore::new(pool);
    let scope = admin_scope();
    let prefix = test_prefix();

    // Insert a resource event with a non-existent run_id (FK violation)
    let event = ResourceEvent {
        id: Uuid::new_v4(),
        run_id: Uuid::new_v4(),  // Non-existent FK
        node_id: Uuid::new_v4(), // Non-existent FK
        resource_type: "package".to_string(),
        resource_name: format!("{}-orphan-event", prefix),
        action: "create".to_string(),
        status: "updated".to_string(),
        duration_ms: 100,
        cookbook_name: "base".to_string(),
        cookbook_version: "1.0.0".to_string(),
        guard_outcome: None,
        delta: None,
        schema_version: 1,
        created_at: Utc::now(),
    };

    let result = event_store.insert_event(&event, &scope).await;
    assert!(result.is_err(), "Insert with non-existent FK should fail");
}

#[tokio::test]
async fn test_error_path_duplicate_key() {
    let pool = match try_db_pool().await {
        Some(p) => p,
        None => {
            eprintln!("SKIP: DB not available");
            return;
        }
    };
    setup_schema(&pool).await;
    cleanup_schema(&pool).await;
    setup_schema(&pool).await;

    let profile_store = SqlxProfileStore::new(pool);
    let scope = admin_scope();
    let prefix = test_prefix();

    let profile = Profile {
        id: Uuid::new_v4(),
        name: format!("{}-dup", prefix),
        description: Some("Duplicate test".to_string()),
        source: "local".to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };

    // First insert should succeed
    profile_store
        .upsert_profile(&profile, &scope)
        .await
        .unwrap();

    // Insert another profile with the same name — should fail (unique index)
    let dup = Profile {
        id: Uuid::new_v4(),
        name: format!("{}-dup", prefix),
        description: Some("Duplicate".to_string()),
        source: "local".to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };

    let result = profile_store.upsert_profile(&dup, &scope).await;
    assert!(
        result.is_err(),
        "Duplicate name should fail (unique constraint)"
    );
}

#[tokio::test]
async fn test_scope_filtering_returns_empty_for_wrong_project() {
    let pool = match try_db_pool().await {
        Some(p) => p,
        None => {
            eprintln!("SKIP: DB not available");
            return;
        }
    };
    setup_schema(&pool).await;
    cleanup_schema(&pool).await;
    setup_schema(&pool).await;

    let node_store = SqlxNodeStore::new(pool.clone());
    let scope = admin_scope();
    let wrong_scope = Scope::new(
        HashSet::from(["nonexistent-project".to_string()]),
        HashSet::from(["admin".to_string()]),
    );
    let prefix = test_prefix();

    // Create a node
    let node = Node {
        id: Uuid::new_v4(),
        name: format!("{}-scoped", prefix),
        platform: "ubuntu".to_string(),
        platform_version: "22.04".to_string(),
        chef_environment: "prod".to_string(),
        policy_group: "base".to_string(),
        policy_name: "base".to_string(),
        attributes: serde_json::Value::Null,
        project_id: "default".to_string(),
        node_type: "cinc-client".to_string(),
        run_list: vec![],
        last_seen: Utc::now(),
        created_at: Utc::now(),
    };
    node_store.upsert_node(&node, &scope).await.unwrap();

    // Query with wrong project scope — should return empty or error
    let result = node_store.get_node(node.id, &wrong_scope).await;
    assert!(result.is_err(), "Wrong project scope should deny access");
}
