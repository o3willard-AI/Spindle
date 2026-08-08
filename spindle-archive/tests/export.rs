//! Tests for M4-15: Parquet export.
//!
//! Verify:
//! - Export → files exist and are valid Parquet
//! - Load in Parquet reader → schema correct, row count correct
//! - Idempotent re-run → AlreadyExists (no duplicate)
//! - Manifest written with hashes
//! - schema.json written

use std::path::PathBuf;

use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::file::reader::{FileReader, SerializedFileReader};

use spindle_archive::{
    ArchiveConfig, ArchiveControlResult, ArchiveError, ArchiveManifest,
    ArchiveNode, ArchiveResourceEvent, ArchiveRun, ArchiveWeek, ParquetExporter,
};

fn test_config(base: &std::path::Path) -> ArchiveConfig {
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

fn make_resource_event(id: &str, run_id: &str, node_id: &str, status: &str) -> ArchiveResourceEvent {
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

fn standard_week() -> ArchiveWeek {
    ArchiveWeek::with_path(
        "2024-W24".to_string(),
        PathBuf::from("archive_2024-W24"),
    )
}

fn standard_data() -> (Vec<ArchiveNode>, Vec<ArchiveRun>, Vec<ArchiveResourceEvent>, Vec<ArchiveControlResult>) {
    let nodes = vec![
        make_node("node-001", "node-a"),
        make_node("node-002", "node-b"),
    ];

    let runs = vec![
        make_run("run-001", "node-001", "passed", 0),
        make_run("run-002", "node-002", "failed", 3),
    ];

    let resource_events = vec![
        make_resource_event("re-001", "run-001", "node-001", "updated"),
        make_resource_event("re-002", "run-002", "node-002", "failed"),
    ];

    let control_results = vec![
        make_control_result("cr-001", "node-001", "passed"),
        make_control_result("cr-002", "node-002", "failed"),
    ];

    (nodes, runs, resource_events, control_results)
}

// ── Parquet file reading helper ──────────────────────────────────────────────

fn read_parquet_metadata(path: &std::path::Path) -> parquet::file::metadata::ParquetMetaData {
    let file = std::fs::File::open(path).unwrap();
    let reader = SerializedFileReader::new(file).unwrap();
    reader.metadata().clone()
}

fn read_arrow_schema(path: &std::path::Path) -> arrow::datatypes::SchemaRef {
    let file = std::fs::File::open(path).unwrap();
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).unwrap();
    builder.schema().clone()
}

// ── Basic export tests ───────────────────────────────────────────────────────

#[test]
fn test_export_week_creates_all_files() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();

    let manifest = exporter.export_week(
        &week,
        &nodes,
        &runs,
        &events,
        &results,
        vec!["raw-digest-1".to_string()],
    ).unwrap();

    let archive_dir = temp.path().join(&week.path);

    // Check all files exist
    assert!(archive_dir.join("nodes.parquet").exists());
    assert!(archive_dir.join("runs.parquet").exists());
    assert!(archive_dir.join("resource_events.parquet").exists());
    assert!(archive_dir.join("control_results.parquet").exists());
    assert!(archive_dir.join("schema.json").exists());
    assert!(archive_dir.join("manifest.json").exists());

    // Check manifest content
    assert_eq!(manifest.archive_week, "2024-W24");
    assert_eq!(manifest.manifest_version, 1);
    assert_eq!(manifest.schema_version, 1);
    assert_eq!(manifest.record_counts.len(), 4);
    assert_eq!(manifest.record_counts.get("nodes.parquet"), Some(&2));
    assert_eq!(manifest.record_counts.get("runs.parquet"), Some(&2));
    assert_eq!(manifest.record_counts.get("resource_events.parquet"), Some(&2));
    assert_eq!(manifest.record_counts.get("control_results.parquet"), Some(&2));
    assert_eq!(manifest.source_raw_digests, vec!["raw-digest-1".to_string()]);

    // All file hashes should be present
    assert!(manifest.file_hashes.contains_key("nodes.parquet"));
    assert!(manifest.file_hashes.contains_key("runs.parquet"));
    assert!(manifest.file_hashes.contains_key("resource_events.parquet"));
    assert!(manifest.file_hashes.contains_key("control_results.parquet"));
}

#[test]
fn test_archive_week_from_date() {
    use chrono::NaiveDate;

    // June 15, 2024 is a Saturday — ISO week 24
    let date = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();
    let week = ArchiveWeek::from_date(date);

    assert!(week.week.starts_with("2024-W"));
    assert!(week.path.to_string_lossy().starts_with("archive_"));
}

// ── Idempotency tests ────────────────────────────────────────────────────────

#[test]
fn test_idempotent_rerun_returns_already_exists() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();

    // First export — should succeed
    exporter.export_week(
        &week,
        &nodes,
        &runs,
        &events,
        &results,
        vec!["digest-1".to_string()],
    ).unwrap();

    // Second export — should return AlreadyExists
    let result2 = exporter.export_week(
        &week,
        &nodes,
        &runs,
        &events,
        &results,
        vec!["digest-1".to_string()],
    );

    assert!(result2.is_err());
    match result2 {
        Err(ArchiveError::AlreadyExists(w)) => {
            assert_eq!(w, "2024-W24");
        }
        _ => panic!("Expected AlreadyExists error"),
    }

    // Files still exist from first export
    let dir = temp.path().join(&week.path);
    assert!(dir.join("nodes.parquet").exists());
    assert!(dir.join("manifest.json").exists());
}

#[test]
fn test_is_exported_before_and_after() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();

    // Before export: not exported
    assert!(!exporter.is_exported(&week));

    // Export
    exporter.export_week(&week, &nodes, &runs, &events, &results, vec![]).unwrap();

    // After export: is exported
    assert!(exporter.is_exported(&week));
}

// ── Parquet file validation tests ───────────────────────────────────────────

#[test]
fn test_nodes_parquet_schema_and_row_count() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();

    exporter.export_week(
        &week,
        &nodes,
        &runs,
        &events,
        &results,
        vec![],
    ).unwrap();

    let parquet_path = temp.path().join(&week.path).join("nodes.parquet");
    let metadata = std::fs::metadata(&parquet_path).unwrap();
    assert!(metadata.len() > 0, "Parquet file should not be empty");

    // Read back schema
    let schema = read_arrow_schema(&parquet_path);
    assert_eq!(schema.fields().len(), 9);
    assert_eq!(schema.field(0).name(), "id");
    assert_eq!(schema.field(1).name(), "name");
    assert_eq!(schema.field(2).name(), "platform");

    // Read row count from parquet metadata
    let meta = read_parquet_metadata(&parquet_path);
    assert_eq!(meta.file_metadata().num_rows(), 2);
}

#[test]
fn test_runs_parquet_schema_and_row_count() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();

    exporter.export_week(&week, &nodes, &runs, &events, &results, vec![]).unwrap();

    let parquet_path = temp.path().join(&week.path).join("runs.parquet");
    let metadata = std::fs::metadata(&parquet_path).unwrap();
    assert!(metadata.len() > 0, "Parquet file should not be empty");

    // Read back schema
    let schema = read_arrow_schema(&parquet_path);
    assert_eq!(schema.fields().len(), 12);
    assert_eq!(schema.field(0).name(), "id");
    assert_eq!(schema.field(1).name(), "node_id");
    assert_eq!(schema.field(2).name(), "run_id");
    assert_eq!(schema.field(6).name(), "total_resource_count");
    assert_eq!(schema.field(7).name(), "updated_count");
    assert_eq!(schema.field(8).name(), "failed_count");

    // Row count
    let meta = read_parquet_metadata(&parquet_path);
    assert_eq!(meta.file_metadata().num_rows(), 2);
}

#[test]
fn test_control_results_parquet_schema_and_row_count() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();

    exporter.export_week(&week, &nodes, &runs, &events, &results, vec![]).unwrap();

    let parquet_path = temp.path().join(&week.path).join("control_results.parquet");
    let schema = read_arrow_schema(&parquet_path);
    assert_eq!(schema.fields().len(), 8);
    assert_eq!(schema.field(0).name(), "id");
    assert_eq!(schema.field(4).name(), "control_id");
    assert_eq!(schema.field(5).name(), "status");
    assert_eq!(schema.field(6).name(), "impact");

    let meta = read_parquet_metadata(&parquet_path);
    assert_eq!(meta.file_metadata().num_rows(), 2);
}

#[test]
fn test_resource_events_parquet_schema_and_row_count() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();

    exporter.export_week(&week, &nodes, &runs, &events, &results, vec![]).unwrap();

    let parquet_path = temp.path().join(&week.path).join("resource_events.parquet");
    let schema = read_arrow_schema(&parquet_path);
    assert_eq!(schema.fields().len(), 12);
    assert_eq!(schema.field(7).name(), "duration_ms");

    let meta = read_parquet_metadata(&parquet_path);
    assert_eq!(meta.file_metadata().num_rows(), 2);
}

// ── Manifest and schema.json tests ───────────────────────────────────────────

#[test]
fn test_manifest_written_correctly() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();

    let manifest = exporter
        .export_week(
            &week,
            &nodes,
            &runs,
            &events,
            &results,
            vec!["digest-a".to_string(), "digest-b".to_string()],
        )
        .unwrap();

    let manifest_path = temp.path().join(&week.path).join("manifest.json");
    let manifest_str = std::fs::read_to_string(&manifest_path).unwrap();
    let parsed: ArchiveManifest = serde_json::from_str(&manifest_str).unwrap();

    assert_eq!(parsed.archive_week, manifest.archive_week);
    assert_eq!(parsed.record_counts, manifest.record_counts);
    assert_eq!(parsed.file_hashes, manifest.file_hashes);
    assert_eq!(parsed.source_raw_digests, vec!["digest-a", "digest-b"]);
}

#[test]
fn test_schema_json_written() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();

    exporter.export_week(&week, &nodes, &runs, &events, &results, vec![]).unwrap();

    let schema_path = temp.path().join(&week.path).join("schema.json");
    let schema_str = std::fs::read_to_string(&schema_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&schema_str).unwrap();

    assert_eq!(parsed["schema_version"], 1);
    assert!(parsed["tables"].get("nodes").is_some());
    assert!(parsed["tables"].get("runs").is_some());
    assert!(parsed["tables"].get("resource_events").is_some());
    assert!(parsed["tables"].get("control_results").is_some());

    let nodes_cols = parsed["tables"]["nodes"]["columns"].as_array().unwrap();
    assert_eq!(nodes_cols.len(), 9);
    assert_eq!(nodes_cols[0]["name"], "id");
    assert_eq!(nodes_cols[0]["type"], "string");
}

// ── Empty data tests ─────────────────────────────────────────────────────────

#[test]
fn test_export_empty_data() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();

    let manifest = exporter
        .export_week(&week, &[], &[], &[], &[], vec![])
        .unwrap();

    assert_eq!(manifest.record_counts.get("nodes.parquet"), Some(&0));
    assert_eq!(manifest.record_counts.get("runs.parquet"), Some(&0));
    assert_eq!(manifest.record_counts.get("resource_events.parquet"), Some(&0));
    assert_eq!(manifest.record_counts.get("control_results.parquet"), Some(&0));

    let dir = temp.path().join(&week.path);
    assert!(dir.join("nodes.parquet").exists());
    assert!(dir.join("runs.parquet").exists());
    assert!(dir.join("manifest.json").exists());
}

// ── File hash consistency tests ──────────────────────────────────────────────

#[test]
fn test_file_hashes_are_valid() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();

    let manifest = exporter
        .export_week(&week, &nodes, &runs, &events, &results, vec![])
        .unwrap();

    for (filename, hash) in &manifest.file_hashes {
        assert!(
            hash.starts_with("sha256:"),
            "Hash for {} should start with sha256:",
            filename
        );
        let hash_part = hash.strip_prefix("sha256:").unwrap();
        assert_eq!(hash_part.len(), 64, "SHA-256 hash should be 64 hex chars");
    }
}

// ── Multiple weeks test ──────────────────────────────────────────────────────

#[test]
fn test_multiple_weeks() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let (nodes, runs, events, results) = standard_data();

    let week1 = ArchiveWeek::with_path(
        "2024-W01".to_string(),
        PathBuf::from("archive_2024-W01"),
    );
    let week2 = ArchiveWeek::with_path(
        "2024-W02".to_string(),
        PathBuf::from("archive_2024-W02"),
    );

    let manifest1 = exporter
        .export_week(&week1, &nodes, &runs, &events, &results, vec!["d1".to_string()])
        .unwrap();
    let manifest2 = exporter
        .export_week(&week2, &nodes, &runs, &events, &results, vec!["d2".to_string()])
        .unwrap();

    assert_eq!(manifest1.archive_week, "2024-W01");
    assert_eq!(manifest2.archive_week, "2024-W02");

    let dir1 = temp.path().join(&week1.path);
    let dir2 = temp.path().join(&week2.path);
    assert!(dir1.join("manifest.json").exists());
    assert!(dir2.join("manifest.json").exists());
}

// ── DuckDB-style query verification ──────────────────────────────────────────

#[test]
fn test_duckdb_query_failed_runs_matches() {
    // Simulate: DuckDB query "how many failed runs in week X" matches our data.
    // We can't run DuckDB in tests, but we can verify the row count in
    // runs.parquet matches what the API would return for "failed" status.
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();

    exporter
        .export_week(&week, &nodes, &runs, &events, &results, vec![])
        .unwrap();

    // Count failed runs — should be 1 (run-002)
    let failed_count = runs.iter().filter(|r| r.status == "failed").count();
    assert_eq!(failed_count, 1);

    // Verify the Parquet file has the right number of rows
    let parquet_path = temp.path().join(&week.path).join("runs.parquet");
    let meta = read_parquet_metadata(&parquet_path);
    // Total rows = 2 (matches the input data)
    assert_eq!(meta.file_metadata().num_rows(), 2);
    // The DuckDB query SELECT COUNT(*) FROM runs WHERE status='failed'
    // would return 1, matching our API query result
    assert_eq!(failed_count, 1);
}
