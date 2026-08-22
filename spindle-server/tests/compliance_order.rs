//! Issue #54 regression tests — compliance-reports list ordering + effective limit.
//!
//! Two coupled bugs made the dashboard node-detail compliance section show
//! stale (oldest) data, rendering green while the node was actually failing:
//!
//! 1. `list_reports()` ordered ASC (oldest-first) by default, contradicting
//!    its own comment ("compliance reports default to DESC"), so the default
//!    50-row window kept the OLDEST reports.
//! 2. The dashboard requested `page_size=1000`, a dead param since #49 (the
//!    server reads `limit`), so it silently fell back to the default 50.
//!
//! Net effect: aggregation ran over the 50 oldest reports and a recent failure
//! never surfaced. These tests exercise `/v1/compliance/reports` through the
//! real axum router against a live PostgreSQL and are skipped when the DB is
//! unreachable (same pattern as tests/e2e.rs and spindle-store integration
//! tests).
//!
//! RED/GREEN: both primary assertions fail on pre-fix code —
//! - default request returned the oldest 50 (first item = oldest report),
//! - `limit=1000` returned only 50 rows.

#![allow(warnings)]
use std::sync::Arc;

use axum::body::Body as AxumBody;
use axum::http::Request;
use chrono::SubsecRound;
use tower::ServiceExt;
use uuid::Uuid;

use spindle_server::compliance::{compliance_router, ComplianceState};
use spindle_store::{Scope, SqlxComplianceStore, SqlxProfileStore};

/// Live PostgreSQL connection URL. Override with DATABASE_URL.
fn db_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://spindle:CHANGE_ME@192.0.2.10:5432/spindle".to_string())
}

/// Try to connect to the live database. Returns None if unavailable.
async fn try_db_pool() -> Option<sqlx::PgPool> {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url())
        .await
        .ok()
}

/// Apply every workspace migration (ignoring per-statement errors) so the
/// test is self-sufficient against an empty scratch database. Same approach
/// as spindle-store/tests/store_integration.rs.
///
/// Runs on a DEDICATED throwaway connection: idempotent re-runs necessarily
/// produce errors (CREATE EXTENSION etc.), and a Postgres session stays in
/// "aborted transaction" state after any failure — handing such a connection
/// back to the shared pool would poison whatever parallel test grabs it next.
async fn ensure_schema() {
    let Ok(conn) = sqlx::PgPool::connect(&db_url()).await else {
        return;
    };
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let migrations_dir = std::path::PathBuf::from(manifest_dir).join("../migrations");
    let Ok(dirs) = std::fs::read_dir(&migrations_dir) else {
        return;
    };
    let mut migration_dirs: Vec<_> = dirs
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    migration_dirs.sort();
    for dir in migration_dirs {
        let up_path = dir.join("up.sql");
        if up_path.exists() {
            if let Ok(sql) = std::fs::read_to_string(&up_path) {
                let _ = sqlx::raw_sql(&sql).execute(&conn).await;
            }
        }
    }
    conn.close().await;
}

/// Remove every row this test may have created (reports, then their controls,
/// then the scratch profile).
async fn cleanup_test_data(pool: &sqlx::PgPool, node_id: Uuid, profile_name: &str) {
    let _ = sqlx::query(
        "DELETE FROM control_results WHERE report_id IN \
         (SELECT id FROM compliance_reports WHERE node_id = $1)",
    )
    .bind(node_id)
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM compliance_reports WHERE node_id = $1")
        .bind(node_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM profiles WHERE name = $1")
        .bind(profile_name)
        .execute(pool)
        .await;
}

/// Seed `count` compliance reports for `node_id`, one hour apart, oldest
/// first. Returns `(report_id, created_at_rfc3339)` sorted oldest→newest.
async fn seed_reports(pool: &sqlx::PgPool, node_id: Uuid, count: usize) -> Vec<(Uuid, String)> {
    // Derive the scratch profile name from the per-test node_id so parallel
    // test tasks never share (or race each other's cleanup of) a profile.
    let profile_name = format!("issue54-regression-{node_id}");
    let profile_id: Uuid = sqlx::query_scalar(
        "INSERT INTO profiles (name) VALUES ($1) \
         ON CONFLICT (name) DO UPDATE SET updated_at = NOW() \
         RETURNING id",
    )
    .bind(&profile_name)
    .fetch_one(pool)
    .await
    .expect("insert scratch profile");

    let base = chrono::Utc::now() - chrono::Duration::hours(count as i64 + 1);
    let mut seeded = Vec::with_capacity(count);
    for i in 0..count {
        let id = Uuid::new_v4();
        // Postgres TIMESTAMPTZ keeps only microseconds; truncate so the
        // in-memory expected values match exactly what comes back.
        let created_at = (base + chrono::Duration::hours(i as i64)).trunc_subsecs(6);
        sqlx::query(
            "INSERT INTO compliance_reports \
             (id, run_id, node_id, profile_id, profile_name, status, passed_count, \
              failed_count, warning_count, created_at) \
             VALUES ($1, $2, $3, $4, $5, 'passed', 5, 0, 0, $6)",
        )
        .bind(id)
        .bind(Uuid::new_v4()) // run_id (no FK on this column)
        .bind(node_id)
        .bind(profile_id)
        .bind(&profile_name)
        .bind(created_at)
        .execute(pool)
        .await
        .expect("insert compliance report");
        seeded.push((id, created_at.to_rfc3339()));
    }
    seeded
}

fn build_compliance_router(pool: sqlx::PgPool) -> axum::Router {
    let store = Arc::new(SqlxComplianceStore::new(pool.clone()));
    let profiles = Arc::new(SqlxProfileStore::new(pool));
    let scope = Scope::all();
    compliance_router(ComplianceState::new(store, profiles, scope))
}

/// GET /v1/compliance/reports<query> through the real handler; returns JSON.
async fn get_reports(router: axum::Router, query: &str) -> serde_json::Value {
    let request = Request::builder()
        .method("GET")
        .uri(format!("/v1/compliance/reports{query}"))
        .body(AxumBody::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(
        response.status().as_u16(),
        200,
        "list_reports returned non-200"
    );
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).expect("response body is JSON")
}

/// Extract created_at timestamps from data.items.
fn timestamps(body: &serde_json::Value) -> Vec<chrono::DateTime<chrono::Utc>> {
    body["data"]["items"]
        .as_array()
        .expect("data.items array")
        .iter()
        .map(|it| {
            chrono::DateTime::parse_from_rfc3339(it["created_at"].as_str().unwrap_or(""))
                .expect("rfc3339 created_at")
                .with_timezone(&chrono::Utc)
        })
        .collect()
}

fn assert_newest_first(ts: &[chrono::DateTime<chrono::Utc>], ctx: &str) {
    let mut sorted = ts.to_vec();
    sorted.sort();
    sorted.reverse();
    assert_eq!(ts, sorted.as_slice(), "{ctx}: expected newest-first order");
}

#[tokio::test]
async fn issue54_compliance_reports_default_to_newest_first() {
    let Some(pool) = try_db_pool().await else {
        eprintln!("SKIP: Live database not available at {}", db_url());
        return;
    };

    let node_id = Uuid::new_v4();
    let profile_name = format!("issue54-regression-{node_id}");
    ensure_schema().await;
    cleanup_test_data(&pool, node_id, &profile_name).await;
    let seeded = seed_reports(&pool, node_id, 60).await; // [0] oldest … [59] newest

    let router = build_compliance_router(pool.clone());
    let body = get_reports(router, &format!("?filter%5Bnode_id%5D={node_id}")).await;

    let ts = timestamps(&body);

    // Default window size unchanged…
    assert_eq!(ts.len(), 50, "default limit must remain 50");
    // …but the window must now hold the NEWEST 50, newest first.
    assert_newest_first(&ts, "default order");
    assert_eq!(
        ts[0],
        chrono::DateTime::parse_from_rfc3339(&seeded[59].1)
            .unwrap()
            .with_timezone(&chrono::Utc),
        "first item must be the single newest report"
    );
    assert!(
        !ts.contains(
            &chrono::DateTime::parse_from_rfc3339(&seeded[0].1)
                .unwrap()
                .with_timezone(&chrono::Utc)
        ),
        "the oldest report must fall outside the default window"
    );

    cleanup_test_data(&pool, node_id, &profile_name).await;
}

#[tokio::test]
async fn issue54_limit_over_50_returns_all_matching_rows() {
    let Some(pool) = try_db_pool().await else {
        eprintln!("SKIP: Live database not available at {}", db_url());
        return;
    };

    let node_id = Uuid::new_v4();
    let profile_name = format!("issue54-regression-{node_id}");
    ensure_schema().await;
    cleanup_test_data(&pool, node_id, &profile_name).await;
    let seeded = seed_reports(&pool, node_id, 60).await;

    let router = build_compliance_router(pool.clone());
    // This is exactly what the fixed dashboard sends.
    let body = get_reports(
        router,
        &format!("?filter%5Bnode_id%5D={node_id}&limit=1000&sort=created_at:desc"),
    )
    .await;

    let ts = timestamps(&body);
    assert_eq!(
        ts.len(),
        60,
        "limit=1000 must return all 60 matching rows (pre-fix: capped at default 50)"
    );
    assert_newest_first(&ts, "explicit desc");
    assert_eq!(
        ts.last(),
        Some(
            &chrono::DateTime::parse_from_rfc3339(&seeded[0].1)
                .unwrap()
                .with_timezone(&chrono::Utc)
        ),
        "the oldest report must appear last under explicit desc"
    );
    assert_eq!(body["data"]["total_count"], 60);
    assert_eq!(body["data"]["has_more"], false);

    cleanup_test_data(&pool, node_id, &profile_name).await;
}

#[tokio::test]
async fn issue54_explicit_sort_still_overrides_default() {
    let Some(pool) = try_db_pool().await else {
        eprintln!("SKIP: Live database not available at {}", db_url());
        return;
    };

    let node_id = Uuid::new_v4();
    let profile_name = format!("issue54-regression-{node_id}");
    ensure_schema().await;
    cleanup_test_data(&pool, node_id, &profile_name).await;
    let seeded = seed_reports(&pool, node_id, 55).await;

    let router = build_compliance_router(pool.clone());
    let body = get_reports(
        router,
        &format!("?filter%5Bnode_id%5D={node_id}&sort=created_at:asc"),
    )
    .await;

    let ts = timestamps(&body);
    assert_eq!(ts.len(), 50, "default limit applies to explicit sorts too");

    // Client-provided sort must still win over the new desc default.
    let mut ascending = ts.clone();
    ascending.sort();
    assert_eq!(ts, ascending.as_slice(), "expected oldest-first under :asc");
    assert_eq!(
        ts[0],
        chrono::DateTime::parse_from_rfc3339(&seeded[0].1)
            .unwrap()
            .with_timezone(&chrono::Utc),
        "first item must be the oldest report under explicit :asc"
    );

    cleanup_test_data(&pool, node_id, &profile_name).await;
}
