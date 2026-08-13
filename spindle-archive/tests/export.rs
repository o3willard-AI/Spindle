//! Tests for M4-04: Key identifier recording.
//!
//! Verify:
//! - Every manifest stores signing_key_id
//! - Key rotation produces new key_id, old artifacts retain old
//! - export_week requires a Signer trait bound (compile-time enforcement)

#![allow(warnings)]
use std::path::PathBuf;

use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::file::reader::{FileReader, SerializedFileReader};

use spindle_archive::{
    ArchiveConfig, ArchiveControlResult, ArchiveError, ArchiveManifest, ArchiveNode,
    ArchiveResourceEvent, ArchiveRun, ArchiveWeek, ParquetExporter,
};
use spindle_signing::{KeyId, LocalSigner, Signer};

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

fn standard_week() -> ArchiveWeek {
    ArchiveWeek::with_path("2024-W24".to_string(), PathBuf::from("archive_2024-W24"))
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

// ── Helper: create and unlock a test signer ────────────────────────────────────

fn make_test_signer(temp_dir: &std::path::Path, name: &str) -> LocalSigner {
    let mut signer = LocalSigner::new();
    let key_path = temp_dir.join(name);
    signer.generate(&key_path, "test-unlock").unwrap();
    signer.unlock(&key_path, "test-unlock").unwrap();
    signer
}

// ── Key ID present in manifest ─────────────────────────────────────────────────

#[test]
fn test_manifest_has_key_id() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();

    let signer = make_test_signer(&temp.path(), "key1.aes");
    let original_key_id = signer.key_id().unwrap().as_str().to_string();

    let manifest = exporter
        .export_week(
            &week,
            &signer,
            &nodes,
            &runs,
            &events,
            &results,
            vec!["raw-digest-1".to_string()],
        )
        .unwrap();

    // Key ID must be present in the manifest
    assert!(
        !manifest.signing_key_id.is_empty(),
        "signing_key_id must not be empty"
    );
    assert_eq!(manifest.signing_key_id, original_key_id);

    // Format must be "local:<sha256_hex>"
    assert!(
        manifest.signing_key_id.starts_with("local:"),
        "signing_key_id must start with 'local:', got: {}",
        manifest.signing_key_id
    );

    // Verify the key_id matches what the signer reports
    let kid = signer.key_id().unwrap();
    let signer_kid = kid.as_str();
    assert_eq!(manifest.signing_key_id, signer_kid);
}

#[test]
fn test_manifest_key_id_from_json() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();

    let signer = make_test_signer(&temp.path(), "key-json.aes");

    let manifest = exporter
        .export_week(
            &week,
            &signer,
            &nodes,
            &runs,
            &events,
            &results,
            vec!["digest-a".to_string(), "digest-b".to_string()],
        )
        .unwrap();

    // Read manifest.json and verify key_id survives serialization
    let manifest_path = temp.path().join(&week.path).join("manifest.json");
    let manifest_str = std::fs::read_to_string(&manifest_path).unwrap();
    let parsed: ArchiveManifest = serde_json::from_str(&manifest_str).unwrap();

    assert_eq!(parsed.signing_key_id, manifest.signing_key_id);
    assert!(parsed.signing_key_id.starts_with("local:"));
}

// ── Key rotation → new artifacts show new key_id ────────────────────────────────

#[test]
fn test_key_rotation_new_key_id_in_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);

    let week1 = ArchiveWeek::with_path("2024-W24".to_string(), PathBuf::from("archive_2024-W24"));
    let week2 = ArchiveWeek::with_path("2024-W25".to_string(), PathBuf::from("archive_2024-W25"));
    let (nodes, runs, events, results) = standard_data();

    // First export with original signer
    let mut signer = make_test_signer(&temp.path(), "rotate-key.aes");
    let key_id_before = signer.key_id().unwrap().as_str().to_string();

    let manifest1 = exporter
        .export_week(
            &week1,
            &signer,
            &nodes,
            &runs,
            &events,
            &results,
            vec!["digest-1".to_string()],
        )
        .unwrap();

    assert_eq!(manifest1.signing_key_id, key_id_before);

    // Rotate the key
    signer
        .rotate(&temp.path().join("rotate-key.aes"), "test-unlock")
        .unwrap();
    let key_id_after = signer.key_id().unwrap().as_str().to_string();

    // Key ID must have changed
    assert_ne!(
        key_id_before, key_id_after,
        "Rotated key must produce different key_id"
    );

    // Second export with rotated signer must show new key_id
    let manifest2 = exporter
        .export_week(
            &week2,
            &signer,
            &nodes,
            &runs,
            &events,
            &results,
            vec!["digest-2".to_string()],
        )
        .unwrap();

    assert_eq!(manifest2.signing_key_id, key_id_after);
    assert_ne!(manifest1.signing_key_id, manifest2.signing_key_id);

    // Old manifest still has old key_id (not overwritten)
    assert_eq!(manifest1.signing_key_id, key_id_before);
}

// ── Idempotency / basic export tests (unchanged from M4-15) ─────────────────────

#[test]
fn test_export_week_creates_all_files() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();

    let signer = make_test_signer(&temp.path(), "basic-key.aes");

    let manifest = exporter
        .export_week(
            &week,
            &signer,
            &nodes,
            &runs,
            &events,
            &results,
            vec!["raw-digest-1".to_string()],
        )
        .unwrap();

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
    assert_eq!(
        manifest.record_counts.get("resource_events.parquet"),
        Some(&2)
    );
    assert_eq!(
        manifest.record_counts.get("control_results.parquet"),
        Some(&2)
    );
    assert_eq!(
        manifest.source_raw_digests,
        vec!["raw-digest-1".to_string()]
    );

    // All file hashes should be present
    assert!(manifest.file_hashes.contains_key("nodes.parquet"));
    assert!(manifest.file_hashes.contains_key("runs.parquet"));
    assert!(manifest.file_hashes.contains_key("resource_events.parquet"));
    assert!(manifest.file_hashes.contains_key("control_results.parquet"));
}

#[test]
fn test_idempotent_rerun_returns_already_exists() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();

    let signer = make_test_signer(&temp.path(), "idem-key.aes");

    // First export — should succeed
    exporter
        .export_week(
            &week,
            &signer,
            &nodes,
            &runs,
            &events,
            &results,
            vec!["digest-1".to_string()],
        )
        .unwrap();

    // Second export — should return AlreadyExists
    let result2 = exporter.export_week(
        &week,
        &signer,
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
}

#[test]
fn test_is_exported_before_and_after() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();

    let signer = make_test_signer(&temp.path(), "is-exported-key.aes");

    // Before export: not exported
    assert!(!exporter.is_exported(&week));

    // Export
    exporter
        .export_week(&week, &signer, &nodes, &runs, &events, &results, vec![])
        .unwrap();

    // After export: is exported
    assert!(exporter.is_exported(&week));
}

// ── Parquet file validation tests ──────────────────────────────────────────────

#[test]
fn test_nodes_parquet_schema_and_row_count() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();

    let signer = make_test_signer(&temp.path(), "nodes-key.aes");

    exporter
        .export_week(&week, &signer, &nodes, &runs, &events, &results, vec![])
        .unwrap();

    let parquet_path = temp.path().join(&week.path).join("nodes.parquet");
    let metadata = std::fs::metadata(&parquet_path).unwrap();
    assert!(metadata.len() > 0, "Parquet file should not be empty");

    let schema = read_arrow_schema(&parquet_path);
    assert_eq!(schema.fields().len(), 9);
    assert_eq!(schema.field(0).name(), "id");
    assert_eq!(schema.field(1).name(), "name");

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

    let signer = make_test_signer(&temp.path(), "runs-key.aes");

    exporter
        .export_week(&week, &signer, &nodes, &runs, &events, &results, vec![])
        .unwrap();

    let parquet_path = temp.path().join(&week.path).join("runs.parquet");
    let metadata = std::fs::metadata(&parquet_path).unwrap();
    assert!(metadata.len() > 0);

    let schema = read_arrow_schema(&parquet_path);
    assert_eq!(schema.fields().len(), 12);
    assert_eq!(schema.field(0).name(), "id");
    assert_eq!(schema.field(1).name(), "node_id");
    assert_eq!(schema.field(2).name(), "run_id");

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

    let signer = make_test_signer(&temp.path(), "cr-key.aes");

    exporter
        .export_week(&week, &signer, &nodes, &runs, &events, &results, vec![])
        .unwrap();

    let parquet_path = temp.path().join(&week.path).join("control_results.parquet");
    let schema = read_arrow_schema(&parquet_path);
    assert_eq!(schema.fields().len(), 8);
    assert_eq!(schema.field(4).name(), "control_id");

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

    let signer = make_test_signer(&temp.path(), "re-key.aes");

    exporter
        .export_week(&week, &signer, &nodes, &runs, &events, &results, vec![])
        .unwrap();

    let parquet_path = temp.path().join(&week.path).join("resource_events.parquet");
    let schema = read_arrow_schema(&parquet_path);
    assert_eq!(schema.fields().len(), 12);
    assert_eq!(schema.field(7).name(), "duration_ms");

    let meta = read_parquet_metadata(&parquet_path);
    assert_eq!(meta.file_metadata().num_rows(), 2);
}

// ── Empty data tests ───────────────────────────────────────────────────────────

#[test]
fn test_export_empty_data() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();

    let signer = make_test_signer(&temp.path(), "empty-key.aes");

    let manifest = exporter
        .export_week(&week, &signer, &[], &[], &[], &[], vec![])
        .unwrap();

    assert_eq!(manifest.record_counts.get("nodes.parquet"), Some(&0));
    assert_eq!(manifest.record_counts.get("runs.parquet"), Some(&0));
    assert!(manifest.signing_key_id.starts_with("local:"));

    let dir = temp.path().join(&week.path);
    assert!(dir.join("nodes.parquet").exists());
    assert!(dir.join("manifest.json").exists());
}

// ── File hash consistency ──────────────────────────────────────────────────────

#[test]
fn test_file_hashes_are_valid() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();

    let signer = make_test_signer(&temp.path(), "hash-key.aes");

    let manifest = exporter
        .export_week(&week, &signer, &nodes, &runs, &events, &results, vec![])
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

// ── Multiple weeks test ────────────────────────────────────────────────────────

#[test]
fn test_multiple_weeks() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let (nodes, runs, events, results) = standard_data();

    let week1 = ArchiveWeek::with_path("2024-W01".to_string(), PathBuf::from("archive_2024-W01"));
    let week2 = ArchiveWeek::with_path("2024-W02".to_string(), PathBuf::from("archive_2024-W02"));

    let signer1 = make_test_signer(&temp.path(), "mweek1-key.aes");
    let signer2 = make_test_signer(&temp.path(), "mweek2-key.aes");

    let manifest1 = exporter
        .export_week(
            &week1,
            &signer1,
            &nodes,
            &runs,
            &events,
            &results,
            vec!["d1".to_string()],
        )
        .unwrap();
    let manifest2 = exporter
        .export_week(
            &week2,
            &signer2,
            &nodes,
            &runs,
            &events,
            &results,
            vec!["d2".to_string()],
        )
        .unwrap();

    assert_eq!(manifest1.archive_week, "2024-W01");
    assert_eq!(manifest2.archive_week, "2024-W02");

    // Different signers → different key_ids
    assert_ne!(manifest1.signing_key_id, manifest2.signing_key_id);

    let dir1 = temp.path().join(&week1.path);
    let dir2 = temp.path().join(&week2.path);
    assert!(dir1.join("manifest.json").exists());
    assert!(dir2.join("manifest.json").exists());
}

// ── Compile-time enforcement: Signer trait required ────────────────────────────

// The export_week signature is:
//   pub fn export_week<S: Signer + 'static>(&self, week: &ArchiveWeek, signer: &S, ...)
// This means:
//   - A Signer must be passed (cannot export unsigned)
//   - The Signer type must be known at compile time (S: 'static)
//   - Passing &dyn Signer still works (dynamic dispatch), but &String would not
//   - If the Signer trait didn't exist, the function wouldn't compile

#[test]
fn test_signer_trait_required() {
    // This test verifies the trait requirement by successfully passing
    // a LocalSigner (which implements Signer) to export_week.
    //
    // If we tried to pass something that doesn't implement Signer,
    // this would be a compile-time error, not a runtime error.
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();

    // Must create and pass a real Signer — no way around it
    let signer = make_test_signer(&temp.path(), "trait-key.aes");

    let manifest = exporter
        .export_week(&week, &signer, &nodes, &runs, &events, &results, vec![])
        .unwrap();

    // Verify the key_id is from a local signer
    assert!(manifest.signing_key_id.starts_with("local:"));

    // Verify key_id is deterministic: re-create from same key should match
    // (The signer was already written to disk, so we can reload)
    let mut reload_signer = LocalSigner::new();
    let key_path = temp.path().join("trait-key.aes");
    reload_signer.unlock(&key_path, "test-unlock").unwrap();
    let kid = reload_signer.key_id().unwrap();
    let reload_key_id = kid.as_str();
    assert_eq!(manifest.signing_key_id, reload_key_id);
}
