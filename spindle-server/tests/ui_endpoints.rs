//! UI aggregate endpoint tests (#fix/ui-endpoints).
//!
//! Covers the three dashboard-facing endpoints added in `spindle_server::ui`:
//! - `GET /v1/summary`   — fleet rollup incl. online/offline, converge outcome,
//!   latest-report compliance classification, and "flipped" nodes
//!   (latest report failed AND penultimate passed).
//! - `GET /v1/compliance/trend?days=N` — daily {date, passRate, passed, failed}.
//! - `GET /v1/runs/trend?days=N`       — daily {date, success, failed}.
//!
//! Strategy: snapshot each aggregate BEFORE seeding (the scratch database may
//! be shared), then assert precise deltas after seeding. All timestamps are
//! truncated to microsecond precision to match Postgres TIMESTAMPTZ.
//!
//! Also guards issue #54: `/v1/compliance/reports` must still return
//! newest-first by default after these additions.
//!
//! Tests are skipped when the database is unreachable (same pattern as
//! tests/compliance_order.rs).

#![allow(warnings)]
use std::sync::Arc;

use axum::body::Body as AxumBody;
use axum::http::Request;
use chrono::{SubsecRound, Utc};
use tower::ServiceExt;
use uuid::Uuid;

use spindle_server::compliance::{compliance_router, ComplianceState};
use spindle_server::ui::{ui_routes, UiAppState};
use spindle_store::{Scope, SqlxComplianceStore, SqlxProfileStore};

/// These tests INSERT/DELETE rows in the shared scratch database. Run them
/// sequentially — concurrent aggregate snapshots (e.g. /v1/summary totals)
/// would otherwise count each other's fixtures mid-test.
static DB_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn db_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://spindle:CHANGE_ME@192.0.2.10:5432/spindle".to_string())
}

async fn try_db_pool() -> Option<sqlx::PgPool> {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url())
        .await
        .ok()
}

/// Apply every workspace migration on a DEDICATED throwaway connection.
/// Idempotent re-runs produce expected errors; an aborted session must never
/// be recycled into the shared pool (see compliance_order.rs note).
async fn ensure_schema() {
    let Ok(conn) = sqlx::PgPool::connect(&db_url()).await else {
        return;
    };
    let migrations_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../migrations");
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

/// Remove every row this file's tests created.
async fn cleanup_test_data(pool: &sqlx::PgPool, node_ids: &[Uuid], profile_name: &str) {
    for node_id in node_ids {
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
        let _ = sqlx::query("DELETE FROM runs WHERE node_id = $1")
            .bind(node_id)
            .execute(pool)
            .await;
        let _ = sqlx::query("DELETE FROM nodes WHERE id = $1")
            .bind(node_id)
            .execute(pool)
            .await;
    }
    let _ = sqlx::query("DELETE FROM profiles WHERE name = $1")
        .bind(profile_name)
        .execute(pool)
        .await;
}

async fn insert_node(
    pool: &sqlx::PgPool,
    node_id: Uuid,
    name: &str,
    last_seen: Option<chrono::DateTime<Utc>>,
) {
    sqlx::query(
        "INSERT INTO nodes (id, name, platform, status, last_seen) \
         VALUES ($1, $2, 'ubuntu', 'active', $3)",
    )
    .bind(node_id)
    .bind(name)
    .bind(last_seen)
    .execute(pool)
    .await
    .expect("insert node");
}

#[allow(clippy::too_many_arguments)]
async fn insert_report(
    pool: &sqlx::PgPool,
    id: Uuid,
    node_id: Uuid,
    profile_id: Uuid,
    profile_name: &str,
    status: &str,
    created_at: chrono::DateTime<Utc>,
) {
    sqlx::query(
        "INSERT INTO compliance_reports \
         (id, run_id, node_id, profile_id, profile_name, status, \
          passed_count, failed_count, warning_count, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, 5, 0, 0, $7)",
    )
    .bind(id)
    .bind(Uuid::new_v4()) // run_id has no FK constraint
    .bind(node_id)
    .bind(profile_id)
    .bind(profile_name)
    .bind(status)
    .bind(created_at)
    .execute(pool)
    .await
    .expect("insert compliance report");
}

/// Seed a run row; `start_time` drives /v1/runs/trend bucketing deterministically.
async fn insert_run(
    pool: &sqlx::PgPool,
    id: Uuid,
    node_id: Uuid,
    status: &str,
    start_time: chrono::DateTime<Utc>,
) {
    sqlx::query(
        "INSERT INTO runs (id, node_id, run_id, status, start_time, created_at) \
         VALUES ($1, $2, $3, $4, $5, $5)",
    )
    .bind(id)
    .bind(node_id)
    .bind(format!("ui-trend-{id}"))
    .bind(status)
    .bind(start_time)
    .execute(pool)
    .await
    .expect("insert run");
}

async fn get_json(router: axum::Router, uri: &str) -> (u16, serde_json::Value) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .body(AxumBody::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, body)
}

fn ui_router(pool: sqlx::PgPool) -> axum::Router {
    let metrics = Arc::new(spindle_server::metrics::MetricsRegistry::new());
    ui_routes(UiAppState::new(Some(pool.clone()), metrics))
}

// ── PART2_ANCHOR ──

#[tokio::test]
async fn ui_summary_counts_and_flipped_nodes() {
    let _db_guard = DB_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(pool) = try_db_pool().await else {
        eprintln!("SKIP: Live database not available at {}", db_url());
        return;
    };
    ensure_schema().await;

    // Three scratch nodes: A online+flipped, B offline+flipped, C online+stable.
    let node_a = Uuid::new_v4();
    let node_b = Uuid::new_v4();
    let node_c = Uuid::new_v4();
    let profile_name = format!("ui-summary-{node_a}");
    cleanup_test_data(&pool, &[node_a, node_b, node_c], &profile_name).await;

    // Snapshot BEFORE seeding so assertions hold on a shared database.
    // (A concurrent writer could still perturb totals; the deltas below are
    // what our seeds contribute, which is stable for counts we fully control.)
    let router = ui_router(pool.clone());
    let (status, before) = get_json(router.clone(), "/v1/summary").await;
    assert_eq!(status, 200, "summary must return 200");
    let before_total = before["total"].as_i64().unwrap_or(0);
    let before_online = before["online"].as_i64().unwrap_or(0);
    let before_cs = before["convergeSuccess"].as_i64().unwrap_or(0);
    let before_cf = before["convergeFailed"].as_i64().unwrap_or(0);
    let before_comp = before["compliant"].as_i64().unwrap_or(0);
    let before_nc = before["nonCompliant"].as_i64().unwrap_or(0);
    let before_flipped = before["flipped"].as_array().cloned().unwrap_or_default();

    let now = Utc::now();
    insert_node(&pool, node_a, "ui-summary-a", Some(now)).await;
    insert_node(
        &pool,
        node_b,
        "ui-summary-b",
        Some(now - chrono::Duration::hours(2)),
    )
    .await;
    insert_node(&pool, node_c, "ui-summary-c", Some(now)).await;

    let profile_id: Uuid = sqlx::query_scalar(
        "INSERT INTO profiles (name) VALUES ($1) \
         ON CONFLICT (name) DO UPDATE SET updated_at = NOW() RETURNING id",
    )
    .bind(&profile_name)
    .fetch_one(&pool)
    .await
    .expect("insert profile");

    let t0 = (now - chrono::Duration::hours(6)).trunc_subsecs(6);
    let t1 = (now - chrono::Duration::hours(5)).trunc_subsecs(6);
    // A: passed then failed  -> flipped
    insert_report(
        &pool,
        Uuid::new_v4(),
        node_a,
        profile_id,
        &profile_name,
        "passed",
        t0,
    )
    .await;
    insert_report(
        &pool,
        Uuid::new_v4(),
        node_a,
        profile_id,
        &profile_name,
        "failed",
        t1,
    )
    .await;
    // B: passed then failed  -> flipped
    insert_report(
        &pool,
        Uuid::new_v4(),
        node_b,
        profile_id,
        &profile_name,
        "passed",
        t0,
    )
    .await;
    insert_report(
        &pool,
        Uuid::new_v4(),
        node_b,
        profile_id,
        &profile_name,
        "failed",
        t1,
    )
    .await;
    // C: passed then passed  -> NOT flipped
    insert_report(
        &pool,
        Uuid::new_v4(),
        node_c,
        profile_id,
        &profile_name,
        "passed",
        t0,
    )
    .await;
    insert_report(
        &pool,
        Uuid::new_v4(),
        node_c,
        profile_id,
        &profile_name,
        "passed",
        t1,
    )
    .await;

    // Runs: 2 success, 1 failed for A; 1 failed for B.
    insert_run(&pool, Uuid::new_v4(), node_a, "success", t0).await;
    insert_run(&pool, Uuid::new_v4(), node_a, "success", t1).await;
    insert_run(&pool, Uuid::new_v4(), node_a, "failed", t1).await;
    insert_run(&pool, Uuid::new_v4(), node_b, "failed", t1).await;

    // Re-fetch after seeding and compare against the pre-seed snapshot.
    let (_, after) = get_json(router.clone(), "/v1/summary").await;

    assert_eq!(
        after["total"].as_i64().unwrap() - before_total,
        3,
        "total delta must be 3"
    );
    assert_eq!(
        after["online"].as_i64().unwrap() - before_online,
        2,
        "online delta must be 2 (A and C seen <300s ago; B seen 2h ago)"
    );
    assert_eq!(
        after["offline"].as_i64().unwrap() - (before_total - before_online),
        1,
        "offline delta must be 1"
    );
    assert_eq!(
        after["convergeSuccess"].as_i64().unwrap() - before_cs,
        2,
        "convergeSuccess delta must be 2"
    );
    assert_eq!(
        after["convergeFailed"].as_i64().unwrap() - before_cf,
        2,
        "convergeFailed delta must be 2"
    );
    // Latest reports: A failed, B failed, C passed.
    assert_eq!(
        after["compliant"].as_i64().unwrap() - before_comp,
        1,
        "compliant delta must be 1 (C latest passed)"
    );
    assert_eq!(
        after["nonCompliant"].as_i64().unwrap() - before_nc,
        2,
        "nonCompliant delta must be 2 (A and B latest failed)"
    );

    // Flipped: exactly A and B, identified by id.
    let flipped_ids: Vec<&str> = after["flipped"]
        .as_array()
        .expect("flipped array")
        .iter()
        .filter_map(|f| f["id"].as_str())
        .collect();
    assert!(
        flipped_ids.contains(&node_a.to_string().as_str()),
        "A must be flipped"
    );
    assert!(
        flipped_ids.contains(&node_b.to_string().as_str()),
        "B must be flipped"
    );
    assert!(
        !flipped_ids.contains(&node_c.to_string().as_str()),
        "C (passed→passed) must NOT be flipped"
    );
    // Delta check against the pre-seed snapshot.
    let before_flip_ids: Vec<&str> = before_flipped
        .iter()
        .filter_map(|f| f["id"].as_str())
        .collect();
    let new_flips: Vec<&&str> = flipped_ids
        .iter()
        .filter(|id| !before_flip_ids.contains(id))
        .collect();
    assert_eq!(
        new_flips.len(),
        2,
        "exactly 2 new flipped nodes expected, got {new_flips:?}"
    );
    // Each flipped entry carries the node name.
    for f in after["flipped"].as_array().unwrap() {
        assert!(f["name"].is_string(), "flipped entry must include name");
    }

    cleanup_test_data(&pool, &[node_a, node_b, node_c], &profile_name).await;
}

// ── PART3_ANCHOR ──

#[tokio::test]
async fn ui_trends_bucket_by_day() {
    let _db_guard = DB_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(pool) = try_db_pool().await else {
        eprintln!("SKIP: Live database not available at {}", db_url());
        return;
    };
    ensure_schema().await;

    let node_a = Uuid::new_v4();
    let node_b = Uuid::new_v4();
    let profile_name = format!("ui-trend-{node_a}");
    cleanup_test_data(&pool, &[node_a, node_b], &profile_name).await;

    insert_node(&pool, node_a, "ui-trend-a", Some(Utc::now())).await;
    insert_node(&pool, node_b, "ui-trend-b", Some(Utc::now())).await;

    let profile_id: Uuid = sqlx::query_scalar(
        "INSERT INTO profiles (name) VALUES ($1) \
         ON CONFLICT (name) DO UPDATE SET updated_at = NOW() RETURNING id",
    )
    .bind(&profile_name)
    .fetch_one(&pool)
    .await
    .expect("insert profile");

    let today = Utc::now().trunc_subsecs(6);
    let yesterday = (Utc::now() - chrono::Duration::hours(30)).trunc_subsecs(6);
    let today_day = today.date_naive();
    let yesterday_day = yesterday.date_naive();

    // Compliance: today 1 passed + 1 failed; yesterday 2 passed.
    insert_report(
        &pool,
        Uuid::new_v4(),
        node_a,
        profile_id,
        &profile_name,
        "passed",
        today,
    )
    .await;
    insert_report(
        &pool,
        Uuid::new_v4(),
        node_a,
        profile_id,
        &profile_name,
        "failed",
        today,
    )
    .await;
    insert_report(
        &pool,
        Uuid::new_v4(),
        node_b,
        profile_id,
        &profile_name,
        "passed",
        yesterday,
    )
    .await;
    insert_report(
        &pool,
        Uuid::new_v4(),
        node_b,
        profile_id,
        &profile_name,
        "passed",
        yesterday,
    )
    .await;

    // Runs: today 2 success + 1 failed; yesterday 1 failed.
    insert_run(&pool, Uuid::new_v4(), node_a, "success", today).await;
    insert_run(&pool, Uuid::new_v4(), node_a, "success", today).await;
    insert_run(&pool, Uuid::new_v4(), node_a, "failed", today).await;
    insert_run(&pool, Uuid::new_v4(), node_b, "failed", yesterday).await;

    let router = ui_router(pool.clone());

    // ── compliance trend ──
    let (status, trend) = get_json(router.clone(), "/v1/compliance/trend?days=14").await;
    assert_eq!(status, 200);
    let buckets = trend.as_array().expect("compliance trend array");
    let find_bucket =
        |arr: &[serde_json::Value], day: chrono::NaiveDate| -> Option<(i64, i64, f64)> {
            arr.iter()
                .find(|b| b["date"].as_str() == Some(day.format("%Y-%m-%d").to_string().as_str()))
                .map(|b| {
                    (
                        b["passed"].as_i64().unwrap_or(0),
                        b["failed"].as_i64().unwrap_or(0),
                        b["passRate"].as_f64().unwrap_or(0.0),
                    )
                })
        };

    // Delta vs a no-seed fetch would race other tests; instead assert the
    // buckets for our two days contain AT LEAST our seeded counts.
    let (p, f, rate) =
        find_bucket(buckets, today_day).expect("today bucket must exist for compliance trend");
    assert!(
        p >= 1 && f >= 1,
        "today bucket must contain >=1 passed and >=1 failed, got p={p} f={f}"
    );
    let expected_rate = (p as f64) / ((p + f) as f64) * 100.0;
    assert!(
        (rate - expected_rate).abs() < 0.011,
        "passRate must equal passed/(passed+failed)*100 rounded: {rate} vs {expected_rate}"
    );
    assert!(rate <= 100.0, "passRate must be <= 100");
    let (yp, yf, _) = find_bucket(buckets, yesterday_day).expect("yesterday bucket must exist");
    assert!(
        yp >= 2 && yf >= 0,
        "yesterday bucket must contain our 2 passed reports"
    );
    // Buckets must be date-ascending.
    let dates: Vec<&str> = buckets.iter().filter_map(|b| b["date"].as_str()).collect();
    let mut sorted = dates.clone();
    sorted.sort();
    assert_eq!(
        dates, sorted,
        "compliance trend buckets must be ascending by date"
    );

    // ── runs trend ──
    let (status, rtrend) = get_json(router.clone(), "/v1/runs/trend?days=7").await;
    assert_eq!(status, 200);
    let rbuckets = rtrend.as_array().expect("runs trend array");
    let find_run_bucket =
        |arr: &[serde_json::Value], day: chrono::NaiveDate| -> Option<(i64, i64)> {
            arr.iter()
                .find(|b| b["date"].as_str() == Some(day.format("%Y-%m-%d").to_string().as_str()))
                .map(|b| {
                    (
                        b["success"].as_i64().unwrap_or(0),
                        b["failed"].as_i64().unwrap_or(0),
                    )
                })
        };
    let (s, f) =
        find_run_bucket(rbuckets, today_day).expect("today bucket must exist for runs trend");
    assert!(
        s >= 2 && f >= 1,
        "today runs bucket must contain our seeds, got s={s} f={f}"
    );
    let (ys, yf) =
        find_run_bucket(rbuckets, yesterday_day).expect("yesterday runs bucket must exist");
    assert!(
        ys >= 0 && yf >= 1,
        "yesterday runs bucket must contain our failed run"
    );
    let dates: Vec<&str> = rbuckets.iter().filter_map(|b| b["date"].as_str()).collect();
    let mut sorted = dates.clone();
    sorted.sort();
    assert_eq!(
        dates, sorted,
        "runs trend buckets must be ascending by date"
    );

    // Window exclusion, made robust against pre-existing rows on shared
    // databases: a report seeded 96h back must appear under days=14 but can
    // never appear under days=1 (the last-24h window's bucket dates can only
    // be yesterday/today, never three days ago).
    let four_days_ago = (Utc::now() - chrono::Duration::hours(96)).trunc_subsecs(6);
    insert_report(
        &pool,
        Uuid::new_v4(),
        node_a,
        profile_id,
        &profile_name,
        "passed",
        four_days_ago,
    )
    .await;
    let old_day = four_days_ago.date_naive();

    let (_, wide) = get_json(router.clone(), "/v1/compliance/trend?days=14").await;
    let wide_buckets = wide.as_array().expect("wide compliance trend array");
    assert!(
        find_bucket(wide_buckets, old_day).is_some(),
        "days=14 must include the 4-day-old report"
    );

    let (_, narrow) = get_json(router.clone(), "/v1/compliance/trend?days=1").await;
    let narrow_buckets = narrow.as_array().expect("narrow compliance trend array");
    assert!(
        find_bucket(narrow_buckets, old_day).is_none(),
        "days=1 must exclude the 4-day-old report"
    );
    assert!(
        find_bucket(narrow_buckets, today_day).is_some(),
        "days=1 must still include today's reports"
    );

    cleanup_test_data(&pool, &[node_a, node_b], &profile_name).await;
}

#[tokio::test]
async fn ui_trend_days_validation() {
    let _db_guard = DB_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(pool) = try_db_pool().await else {
        eprintln!("SKIP: Live database not available at {}", db_url());
        return;
    };
    ensure_schema().await;
    let router = ui_router(pool);

    for uri in [
        "/v1/compliance/trend?days=0",
        "/v1/runs/trend?days=0",
        "/v1/compliance/trend?days=abc",
        "/v1/runs/trend?days=abc",
        "/v1/compliance/trend?days=-3",
    ] {
        let (status, body) = get_json(router.clone(), uri).await;
        assert_eq!(status, 400, "{uri} must reject invalid days");
        assert_eq!(body["error"], "bad_request", "{uri} error envelope");
    }

    // Defaults and clamping work.
    let (status, _) = get_json(router.clone(), "/v1/compliance/trend").await;
    assert_eq!(status, 200, "days omitted must default (14)");
    let (status, _) = get_json(router.clone(), "/v1/runs/trend").await;
    assert_eq!(status, 200, "days omitted must default (7)");
    let (status, _) = get_json(router.clone(), "/v1/compliance/trend?days=99999").await;
    assert_eq!(status, 200, "days must clamp to 365, not error");
}

#[tokio::test]
async fn ui_dev_mode_without_db_degrades_gracefully() {
    let metrics = Arc::new(spindle_server::metrics::MetricsRegistry::new());
    let router = ui_routes(UiAppState::new(None, metrics));

    let (status, body) = get_json(router.clone(), "/v1/summary").await;
    assert_eq!(status, 200);
    assert_eq!(body["total"], 0);
    assert_eq!(body["online"], 0);
    assert_eq!(body["offline"], 0);
    assert_eq!(body["convergeSuccess"], 0);
    assert_eq!(body["convergeFailed"], 0);
    assert_eq!(body["compliant"], 0);
    assert_eq!(body["nonCompliant"], 0);
    assert_eq!(body["unknownCompliance"], 0);
    assert_eq!(body["flipped"].as_array().map(Vec::len), Some(0));

    let (status, body) = get_json(router.clone(), "/v1/compliance/trend?days=14").await;
    assert_eq!(status, 200);
    assert_eq!(body.as_array().map(Vec::len), Some(0));

    let (status, body) = get_json(router.clone(), "/v1/runs/trend?days=7").await;
    assert_eq!(status, 200);
    assert_eq!(body.as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn issue54_compliance_reports_still_newest_first_default() {
    let _db_guard = DB_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Guard: the endpoints added here must not regress the #54 fix.
    let Some(pool) = try_db_pool().await else {
        eprintln!("SKIP: Live database not available at {}", db_url());
        return;
    };
    ensure_schema().await;

    let node_id = Uuid::new_v4();
    let profile_name = format!("ui-guard54-{node_id}");
    cleanup_test_data(&pool, &[node_id], &profile_name).await;

    let now = Utc::now();
    insert_node(&pool, node_id, "ui-guard54", Some(now)).await;
    let profile_id: Uuid = sqlx::query_scalar(
        "INSERT INTO profiles (name) VALUES ($1) \
         ON CONFLICT (name) DO UPDATE SET updated_at = NOW() RETURNING id",
    )
    .bind(&profile_name)
    .fetch_one(&pool)
    .await
    .expect("insert profile");

    let t0 = (now - chrono::Duration::hours(4)).trunc_subsecs(6);
    let t1 = (now - chrono::Duration::hours(3)).trunc_subsecs(6);
    insert_report(
        &pool,
        Uuid::new_v4(),
        node_id,
        profile_id,
        &profile_name,
        "passed",
        t0,
    )
    .await;
    insert_report(
        &pool,
        Uuid::new_v4(),
        node_id,
        profile_id,
        &profile_name,
        "failed",
        t1,
    )
    .await;

    let store = Arc::new(SqlxComplianceStore::new(pool.clone()));
    let profiles = Arc::new(SqlxProfileStore::new(pool.clone()));
    let router = compliance_router(ComplianceState::new(store, profiles, Scope::all()));
    let (status, body) = get_json(
        router,
        &format!("/v1/compliance/reports?filter%5Bnode_id%5D={node_id}"),
    )
    .await;
    assert_eq!(status, 200);

    let items = body["data"]["items"].as_array().expect("items array");
    assert_eq!(items.len(), 2);
    let first: chrono::DateTime<Utc> =
        chrono::DateTime::parse_from_rfc3339(items[0]["created_at"].as_str().unwrap())
            .unwrap()
            .with_timezone(&Utc);
    let last: chrono::DateTime<Utc> =
        chrono::DateTime::parse_from_rfc3339(items[1]["created_at"].as_str().unwrap())
            .unwrap()
            .with_timezone(&Utc);
    assert!(
        first > last,
        "#54 guard: default order must be newest-first (got {first} then {last})"
    );

    cleanup_test_data(&pool, &[node_id], &profile_name).await;
}
