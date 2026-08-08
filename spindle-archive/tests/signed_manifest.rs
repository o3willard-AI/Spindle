//! Tests for M4-16: Signed manifest + verification tool.
//!
//! Verify:
//! - Export → manifest in DB (simulated) → verify matches
//! - Corrupt one file → verify fails with "mismatch: <filename>"
//! - Kill mid-export → no manifest written, no deleted rows
//! - Idempotency: re-export returns AlreadyExists
//! - Signature verification

use std::path::PathBuf;

use spindle_signing::{LocalSigner, Signer};

use spindle_archive::{
    ArchiveConfig, ArchiveControlResult, ArchiveManifest, ArchiveNode,
    ArchiveResourceEvent, ArchiveRun, ArchiveWeek, ParquetExporter,
    SignedManifest, VerifyResult, ArchiveError,
    sign_manifest, verify_manifest, verify_archive,
    export_week_signed, simulate_failed_export,
};

fn test_config(base: &std::path::Path) -> ArchiveConfig {
    ArchiveConfig {
        base_dir: base.to_path_buf(),
        compression_level: 3,
        row_group_size: 100000,
    }
}

fn make_test_signer() -> LocalSigner {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);

    let key_path = std::env::temp_dir().join(format!(
        "spindle-test-key-{}-{}.aes",
        std::process::id(),
        id
    ));
    let unlock = "test-unlock-material-12345";

    let mut signer = LocalSigner::new();
    signer.generate(&key_path, unlock).unwrap();
    signer.unlock(&key_path, unlock).unwrap();
    signer
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
    (
        vec![make_node("node-001", "node-a"), make_node("node-002", "node-b")],
        vec![
            make_run("run-001", "node-001", "passed", 0),
            make_run("run-002", "node-002", "failed", 3),
        ],
        vec![
            make_resource_event("re-001", "run-001", "node-001", "updated"),
            make_resource_event("re-002", "run-002", "node-002", "failed"),
        ],
        vec![
            make_control_result("cr-001", "node-001", "passed"),
            make_control_result("cr-002", "node-002", "failed"),
        ],
    )
}

// ── Signed manifest tests ────────────────────────────────────────────────────

#[test]
fn test_export_week_signed_creates_manifest_and_sig() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();
    let signer = make_test_signer();

    let signed = export_week_signed(
        &exporter,
        &week,
        &nodes,
        &runs,
        &events,
        &results,
        vec!["raw-digest-1".to_string()],
        &signer,
    ).unwrap();

    // Manifest files written
    let archive_dir = temp.path().join(&week.path);
    assert!(archive_dir.join("manifest.json").exists());
    assert!(archive_dir.join("manifest.sig").exists());

    // Signed manifest has key ID and signature
    assert!(!signed.signing_key_id.is_empty());
    assert!(!signed.signature.is_empty());

    // Signature is 64 bytes hex-encoded (128 hex chars)
    assert_eq!(signed.signature.len(), 128);

    // Manifest content is correct
    assert_eq!(signed.manifest.archive_week, "2024-W24");
    assert_eq!(signed.manifest.record_counts.get("nodes.parquet"), Some(&2));
    assert_eq!(signed.manifest.source_raw_digests, vec!["raw-digest-1".to_string()]);
}

#[test]
fn test_verify_signed_manifest_valid() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();
    let signer = make_test_signer();

    let signed = export_week_signed(
        &exporter,
        &week,
        &nodes,
        &runs,
        &events,
        &results,
        vec![],
        &signer,
    ).unwrap();

    // Verify using the signer's public key
    let public_key = signer.public_key();
    let result = verify_manifest(&signed, &temp.path().join(&week.path), &public_key);

    assert_eq!(result, VerifyResult::Valid);
    assert!(result.is_valid());
}

#[test]
fn test_verify_archive_valid() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();
    let signer = make_test_signer();

    export_week_signed(
        &exporter,
        &week,
        &nodes,
        &runs,
        &events,
        &results,
        vec![],
        &signer,
    ).unwrap();

    let public_key = signer.public_key();
    let archive_path = temp.path().join(&week.path);
    let result = verify_archive(&archive_path, &public_key).unwrap();

    assert_eq!(result, VerifyResult::Valid);
}

#[test]
fn test_verify_result_describe() {
    assert_eq!(VerifyResult::Valid.describe(), "valid");
    assert_eq!(
        VerifyResult::Mismatch(vec!["runs.parquet".to_string()]).describe(),
        "mismatch: runs.parquet"
    );
    assert_eq!(
        VerifyResult::Mismatch(vec!["a.parquet".to_string(), "b.parquet".to_string()]).describe(),
        "mismatch: a.parquet, b.parquet"
    );
    assert_eq!(VerifyResult::SignatureInvalid.describe(), "signature invalid");
    assert_eq!(VerifyResult::ManifestNotFound.describe(), "manifest not found");
}

// ── Corruption detection tests ─────────────────────────────────────────────────

#[test]
fn test_corrupt_file_detected_as_mismatch() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();
    let signer = make_test_signer();

    export_week_signed(
        &exporter,
        &week,
        &nodes,
        &runs,
        &events,
        &results,
        vec![],
        &signer,
    ).unwrap();

    // Corrupt runs.parquet
    let parquet_path = temp.path().join(&week.path).join("runs.parquet");
    let mut data = std::fs::read(&parquet_path).unwrap();
    // Flip some bytes in the middle of the file
    if data.len() > 10 {
        data[5] ^= 0xFF;
    }
    std::fs::write(&parquet_path, &data).unwrap();

    // Verify should detect the mismatch
    let public_key = signer.public_key();
    let archive_dir = temp.path().join(&week.path);

    // Read manifest.sig and manifest.json to reconstruct SignedManifest
    let manifest_str = std::fs::read_to_string(archive_dir.join("manifest.json")).unwrap();
    let manifest: ArchiveManifest = serde_json::from_str(&manifest_str).unwrap();
    let sig_str = std::fs::read_to_string(archive_dir.join("manifest.sig")).unwrap();
    let sig_json: serde_json::Value = serde_json::from_str(&sig_str).unwrap();

    let signed = SignedManifest {
        manifest,
        signing_key_id: sig_json["signing_key_id"].as_str().unwrap().to_string(),
        signature: sig_json["signature"].as_str().unwrap().to_string(),
    };

    let result = verify_manifest(&signed, &archive_dir, &public_key);
    assert_eq!(result, VerifyResult::Mismatch(vec!["runs.parquet".to_string()]));
    assert!(!result.is_valid());

    // cli_verify should return error with "mismatch: runs.parquet"
    let cli_result = verify_archive(&archive_dir, &public_key).unwrap();
    assert!(matches!(cli_result, VerifyResult::Mismatch(_)));
}

// ── Idempotency test ──────────────────────────────────────────────────────────

#[test]
fn test_signed_export_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();
    let signer = make_test_signer();

    // First export succeeds
    export_week_signed(
        &exporter,
        &week,
        &nodes,
        &runs,
        &events,
        &results,
        vec![],
        &signer,
    ).unwrap();

    // Second export returns AlreadyExists
    let result = export_week_signed(
        &exporter,
        &week,
        &nodes,
        &runs,
        &events,
        &results,
        vec![],
        &signer,
    );

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), ArchiveError::AlreadyExists(_)));
}

// ── Mid-export failure test ───────────────────────────────────────────────────

#[test]
fn test_failed_export_no_manifest_no_is_exported() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();

    // Simulate crash mid-export (writes files but NOT manifest)
    simulate_failed_export(
        &exporter,
        &week,
        &nodes,
        &runs,
        &events,
        &results,
    ).unwrap();

    // is_exported should return false (no manifest.json)
    assert!(!exporter.is_exported(&week));

    let archive_dir = temp.path().join(&week.path);

    // Parquet files exist but manifest does not
    assert!(archive_dir.join("nodes.parquet").exists());
    assert!(archive_dir.join("runs.parquet").exists());
    assert!(!archive_dir.join("manifest.json").exists());
    assert!(!archive_dir.join("manifest.sig").exists());
}

#[test]
fn test_failed_export_can_be_redone() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();
    let signer = make_test_signer();

    // Simulate failed export
    simulate_failed_export(
        &exporter,
        &week,
        &nodes,
        &runs,
        &events,
        &results,
    ).unwrap();

    assert!(!exporter.is_exported(&week));

    // Now do a proper export — should succeed and create manifest
    let result = export_week_signed(
        &exporter,
        &week,
        &nodes,
        &runs,
        &events,
        &results,
        vec![],
        &signer,
    );

    // Note: is_exported checks for manifest.json, which wasn't written by
    // simulate_failed_export, so the signed export should succeed.
    // (The Parquet files already exist and will be overwritten.)
    assert!(result.is_ok());

    // Now is_exported should return true
    assert!(exporter.is_exported(&week));

    let archive_dir = temp.path().join(&week.path);
    assert!(archive_dir.join("manifest.json").exists());
    assert!(archive_dir.join("manifest.sig").exists());
}

// ── Signature verification tests ───────────────────────────────────────────────

#[test]
fn test_signature_invalid_with_wrong_key() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();
    let signer = make_test_signer();
    let wrong_signer = make_test_signer(); // Different key

    export_week_signed(
        &exporter,
        &week,
        &nodes,
        &runs,
        &events,
        &results,
        vec![],
        &signer,
    ).unwrap();

    // Verify with wrong public key → SignatureInvalid
    let wrong_pubkey = wrong_signer.public_key();

    let archive_dir = temp.path().join(&week.path);
    let manifest_str = std::fs::read_to_string(archive_dir.join("manifest.json")).unwrap();
    let manifest: ArchiveManifest = serde_json::from_str(&manifest_str).unwrap();
    let sig_str = std::fs::read_to_string(archive_dir.join("manifest.sig")).unwrap();
    let sig_json: serde_json::Value = serde_json::from_str(&sig_str).unwrap();

    let signed = SignedManifest {
        manifest,
        signing_key_id: sig_json["signing_key_id"].as_str().unwrap().to_string(),
        signature: sig_json["signature"].as_str().unwrap().to_string(),
    };

    let result = verify_manifest(&signed, &archive_dir, &wrong_pubkey);
    assert_eq!(result, VerifyResult::SignatureInvalid);
}

#[test]
fn test_sign_manifest_produces_valid_signature() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(&temp.path());
    let exporter = ParquetExporter::new(config);
    let week = standard_week();
    let (nodes, runs, events, results) = standard_data();
    let signer = make_test_signer();

    let signed = export_week_signed(
        &exporter,
        &week,
        &nodes,
        &runs,
        &events,
        &results,
        vec![],
        &signer,
    ).unwrap();

    // Verify the signature independently
    let public_key = signer.public_key();
    let result = verify_manifest(&signed, &temp.path().join(&week.path), &public_key);
    assert_eq!(result, VerifyResult::Valid);
}

// ── CLI tests ──────────────────────────────────────────────────────────────────

#[test]
fn test_cli_export_and_verify() {
    let temp = tempfile::tempdir().unwrap();
    let (nodes, runs, events, results) = standard_data();
    let signer = make_test_signer();

    let result = spindle_archive::cli_export(
        "2024-W24",
        temp.path().to_str().unwrap(),
        &nodes,
        &runs,
        &events,
        &results,
        vec!["digest-1".to_string()],
        &signer,
    );

    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.contains("nodes.parquet"));

    // Verify
    let pubkey = signer.public_key();
    let archive_path = temp.path().join("archive_2024-W24");
    let verify_result = spindle_archive::cli_verify(
        archive_path.to_str().unwrap(),
        &pubkey,
    );

    assert!(verify_result.is_ok());
    assert!(verify_result.unwrap().contains("OK"));
}

#[test]
fn test_cli_verify_corrupt_fails() {
    let temp = tempfile::tempdir().unwrap();
    let (nodes, runs, events, results) = standard_data();
    let signer = make_test_signer();

    spindle_archive::cli_export(
        "2024-W24",
        temp.path().to_str().unwrap(),
        &nodes,
        &runs,
        &events,
        &results,
        vec![],
        &signer,
    ).unwrap();

    // Corrupt a file
    let parquet_path = temp.path().join("archive_2024-W24").join("nodes.parquet");
    let mut data = std::fs::read(&parquet_path).unwrap();
    if data.len() > 5 {
        data[5] ^= 0xFF;
    }
    std::fs::write(&parquet_path, &data).unwrap();

    let pubkey = signer.public_key();
    let archive_path = temp.path().join("archive_2024-W24");
    let result = spindle_archive::cli_verify(
        archive_path.to_str().unwrap(),
        &pubkey,
    );

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("mismatch"));
}

#[test]
fn test_cli_verify_missing_manifest_fails() {
    let temp = tempfile::tempdir().unwrap();
    let signer = make_test_signer();
    let pubkey = signer.public_key();

    let nonexistent = temp.path().join("nonexistent_archive");
    let result = spindle_archive::cli_verify(
        nonexistent.to_str().unwrap(),
        &pubkey,
    );

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}
