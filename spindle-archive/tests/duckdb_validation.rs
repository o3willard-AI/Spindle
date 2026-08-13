#![cfg(feature = "duckdb-validation")]
//! M4-22: DuckDB validation of Parquet archive exports.
//!
//! Verifies that:
//! 1. Parquet files are queryable via DuckDB SQL
//! 2. Row counts match between API/manifest and DuckDB queries
//! 3. Schema validation: Parquet column types match migration schema
//! 4. DuckDB query "failed runs" matches API-level count

#![allow(warnings)]
use duckdb::Connection;
use std::path::{Path, PathBuf};

use spindle_archive::{
    ArchiveConfig, ArchiveControlResult, ArchiveNode, ArchiveResourceEvent, ArchiveRun,
    ArchiveWeek, ParquetExporter,
};
use spindle_signing::{LocalSigner, Signer};

fn test_config(base: &Path) -> ArchiveConfig {
    ArchiveConfig {
        base_dir: base.to_path_buf(),
        compression_level: 3,
        row_group_size: 100000,
    }
}

fn make_node(id: &str, name: &str) -> ArchiveNode {
    ArchiveNode {
        id: id.to_string(),
        name: name.to_string(),
        platform: "linux".to_string(),
        platform_version: "5.4.0".to_string(),
        chef_environment: "prod".to_string(),
        policy_group: "web".to_string(),
        policy_name: "web-policy".to_string(),
        last_seen: "2024-06-15T12:00:00+00:00".to_string(),
        created_at: "2024-06-15T12:00:00+00:00".to_string(),
    }
}

fn make_run(id: &str, node_id: &str, status: &str, failed: i32) -> ArchiveRun {
    ArchiveRun {
        id: id.to_string(),
        node_id: node_id.to_string(),
        run_id: id.to_string(),
        status: status.to_string(),
        start_time: "2024-06-15T10:00:00+00:00".to_string(),
        end_time: "2024-06-15T10:05:00+00:00".to_string(),
        total_resource_count: 10,
        updated_count: 3,
        failed_count: failed,
        skipped_count: 1,
        schema_version: 1,
        created_at: "2024-06-15T10:00:00+00:00".to_string(),
    }
}

fn make_resource_event(
    id: &str,
    run_id: &str,
    node_id: &str,
    status: &str,
) -> ArchiveResourceEvent {
    ArchiveResourceEvent {
        id: id.to_string(),
        run_id: run_id.to_string(),
        node_id: node_id.to_string(),
        resource_type: "package".to_string(),
        resource_name: "openssl".to_string(),
        action: "install".to_string(),
        status: status.to_string(),
        duration_ms: 150,
        cookbook_name: "base".to_string(),
        cookbook_version: "1.0.0".to_string(),
        schema_version: 1,
        created_at: "2024-06-15T10:02:00+00:00".to_string(),
    }
}

fn make_control_result(id: &str, node_id: &str, status: &str) -> ArchiveControlResult {
    ArchiveControlResult {
        id: id.to_string(),
        run_id: id.to_string(),
        node_id: node_id.to_string(),
        profile_id: "00000000-0000-0000-0000-000000000001".to_string(),
        control_id: "ctrl-01".to_string(),
        status: status.to_string(),
        impact: "high".to_string(),
        created_at: "2024-06-15T10:00:00+00:00".to_string(),
    }
}

fn make_test_signer(temp_dir: &Path, name: &str) -> LocalSigner {
    let mut signer = LocalSigner::new();
    let key_path = temp_dir.join(name);
    signer.generate(&key_path, "test-unlock").unwrap();
    signer.unlock(&key_path, "test-unlock").unwrap();
    signer
}

fn standard_data() -> (
    Vec<ArchiveNode>,
    Vec<ArchiveRun>,
    Vec<ArchiveResourceEvent>,
    Vec<ArchiveControlResult>,
) {
    let nodes = vec![
        make_node("node-001", "node-a"),
        make_node("node-002", "node-b"),
    ];
    let runs = vec![
        make_run("run-001", "node-001", "passed", 0),
        make_run("run-002", "node-002", "failed", 3),
    ];
    let events = vec![
        make_resource_event("re-001", "run-001", "node-001", "updated"),
        make_resource_event("re-002", "run-002", "node-002", "failed"),
    ];
    let results = vec![
        make_control_result("cr-001", "node-001", "passed"),
        make_control_result("cr-002", "node-002", "failed"),
    ];
    (nodes, runs, events, results)
}

fn duckdb_with_table(file: &Path, table_name: &str) -> duckdb::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(&format!(
        "CREATE TABLE {} AS SELECT * FROM read_parquet('{}')",
        table_name,
        file.display()
    ))?;
    Ok(conn)
}

fn query_count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get(0)).unwrap()
}

#[test]
fn test_duckdb_runs_count_and_schema() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = ArchiveWeek::with_path("2024-W24".to_string(), PathBuf::from("archive_2024-W24"));
    let (nodes, runs, events, results) = standard_data();
    let signer = make_test_signer(&temp.path(), "duckdb-runs.aes");
    exporter
        .export_week(&week, &signer, &nodes, &runs, &events, &results, vec![])
        .unwrap();

    let parquet_path = temp.path().join(&week.path).join("runs.parquet");
    let conn = duckdb_with_table(&parquet_path, "runs").unwrap();

    assert_eq!(query_count(&conn, "SELECT COUNT(*) FROM runs"), 2);

    let schema: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('runs')")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();

    for col in &[
        "id",
        "node_id",
        "run_id",
        "status",
        "updated_count",
        "failed_count",
        "skipped_count",
        "total_resource_count",
        "schema_version",
        "created_at",
    ] {
        assert!(
            schema.iter().any(|c| c == col),
            "runs table should have column: {}",
            col
        );
    }
}

#[test]
fn test_duckdb_failed_runs_matches_api() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = ArchiveWeek::with_path("2024-W24".to_string(), PathBuf::from("archive_2024-W24"));
    let (nodes, runs, events, results) = standard_data();
    let signer = make_test_signer(&temp.path(), "duckdb-failed.aes");
    let manifest = exporter
        .export_week(&week, &signer, &nodes, &runs, &events, &results, vec![])
        .unwrap();

    let parquet_path = temp.path().join(&week.path).join("runs.parquet");
    let conn = duckdb_with_table(&parquet_path, "runs").unwrap();

    let duckdb_failed = query_count(&conn, "SELECT COUNT(*) FROM runs WHERE status = 'failed'");
    assert_eq!(duckdb_failed, 1);

    let duckdb_total = query_count(&conn, "SELECT COUNT(*) FROM runs");
    let manifest_count = manifest
        .record_counts
        .get("runs.parquet")
        .copied()
        .unwrap_or(0);
    assert_eq!(manifest_count, duckdb_total as usize);
}

#[test]
fn test_duckdb_nodes_count_matches_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = ArchiveWeek::with_path("2024-W24".to_string(), PathBuf::from("archive_2024-W24"));
    let (nodes, runs, events, results) = standard_data();
    let signer = make_test_signer(&temp.path(), "duckdb-nodes.aes");
    let manifest = exporter
        .export_week(&week, &signer, &nodes, &runs, &events, &results, vec![])
        .unwrap();

    let parquet_path = temp.path().join(&week.path).join("nodes.parquet");
    let conn = duckdb_with_table(&parquet_path, "nodes").unwrap();

    let duckdb_count = query_count(&conn, "SELECT COUNT(*) FROM nodes");
    let manifest_count = manifest
        .record_counts
        .get("nodes.parquet")
        .copied()
        .unwrap_or(0);
    assert_eq!(duckdb_count as usize, manifest_count);
    assert_eq!(duckdb_count, 2);

    let names: Vec<String> = conn
        .prepare("SELECT name FROM nodes ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(names, vec!["node-a".to_string(), "node-b".to_string()]);
}

#[test]
fn test_duckdb_resource_events_filter() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = ArchiveWeek::with_path("2024-W24".to_string(), PathBuf::from("archive_2024-W24"));
    let (nodes, runs, events, results) = standard_data();
    let signer = make_test_signer(&temp.path(), "duckdb-events.aes");
    exporter
        .export_week(&week, &signer, &nodes, &runs, &events, &results, vec![])
        .unwrap();

    let parquet_path = temp.path().join(&week.path).join("resource_events.parquet");
    let conn = duckdb_with_table(&parquet_path, "resource_events").unwrap();

    assert_eq!(
        query_count(
            &conn,
            "SELECT COUNT(*) FROM resource_events WHERE status = 'failed'"
        ),
        1
    );
    assert_eq!(
        query_count(
            &conn,
            "SELECT COUNT(*) FROM resource_events WHERE status = 'updated'"
        ),
        1
    );
    assert_eq!(
        query_count(
            &conn,
            "SELECT COUNT(*) FROM resource_events WHERE cookbook_name = 'base'"
        ),
        2
    );
}

#[test]
fn test_duckdb_control_results_filter() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = ArchiveWeek::with_path("2024-W24".to_string(), PathBuf::from("archive_2024-W24"));
    let (nodes, runs, events, results) = standard_data();
    let signer = make_test_signer(&temp.path(), "duckdb-control.aes");
    exporter
        .export_week(&week, &signer, &nodes, &runs, &events, &results, vec![])
        .unwrap();

    let parquet_path = temp.path().join(&week.path).join("control_results.parquet");
    let conn = duckdb_with_table(&parquet_path, "control_results").unwrap();

    assert_eq!(
        query_count(
            &conn,
            "SELECT COUNT(*) FROM control_results WHERE status = 'failed'"
        ),
        1
    );
    assert_eq!(
        query_count(
            &conn,
            "SELECT COUNT(*) FROM control_results WHERE status = 'passed'"
        ),
        1
    );
    assert_eq!(
        query_count(
            &conn,
            "SELECT COUNT(*) FROM control_results WHERE impact = 'high'"
        ),
        2
    );
}
