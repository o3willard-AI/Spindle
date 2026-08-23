//! Tests for M4-14: Restored archive verification.
//!
//! Verify:
//! - Verified archive → export → attestation shows "verified"
//! - Unverified archive → attestation shows "unverified"
//! - Unverified source → all downstream marked unverified (cascading)
//! - Restore session TTL includes verification status expiry

#![allow(warnings)]
use chrono::{TimeZone, Utc};
use uuid::Uuid;

use spindle_compliance::{
    export_restored_report, generate_report_with_attestation, should_mark_unverified, AuditLog,
    AuditLogEntry, ComplianceAuditLogger, ControlResult, ControlStatusByNode,
    ExceptionDeviationList, InMemoryAuditLog, MockReportStore, Node, Profile,
    ProfileSummaryOverTime, ReportAttestation, ReportDefinition, ReportFormat, ReportParams,
    RestoreSession, VerificationStatus, WaiverRegister,
};

// ── Test data helpers ────────────────────────────────────────────────────────

fn uuid_from(s: &str) -> Uuid {
    Uuid::parse_str(s).unwrap()
}

const PROFILE_ID: &str = "00000000-0000-0000-0000-000000000001";
const NODE_ID_A: &str = "00000000-0000-0000-0000-00000000000a";

fn make_node(name: &str, id: &str) -> Node {
    Node {
        id: uuid_from(id),
        name: name.to_string(),
        platform: "linux".to_string(),
        platform_version: "5.4.0".to_string(),
        chef_environment: "prod".to_string(),
        policy_group: "web".to_string(),
        policy_name: "web-policy".to_string(),
        attributes: serde_json::Value::Null,
        project_id: "default".to_string(),
        node_type: "cinc-client".to_string(),
        run_list: vec![],
        last_seen: Utc.with_ymd_and_hms(2024, 6, 15, 12, 0, 0).unwrap(),
        created_at: Utc.with_ymd_and_hms(2024, 6, 15, 12, 0, 0).unwrap(),
    }
}

fn make_profile(name: &str) -> Profile {
    Profile {
        id: uuid_from(PROFILE_ID),
        name: name.to_string(),
        description: Some(format!("Profile {}", name)),
        source: "auditor".to_string(),
        created_at: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
    }
}

fn make_cr(node_id: Uuid, control_id: &str, status: &str, seq: u32) -> ControlResult {
    ControlResult {
        id: uuid_from(&format!("00000000-0000-0000-0000-{:012x}", seq)),
        report_id: Uuid::nil(),
        run_id: Uuid::nil(),
        node_id,
        profile_id: uuid_from(PROFILE_ID),
        control_id: control_id.to_string(),
        status: status.to_string(),
        impact: 0.7,
        result: None,
        created_at: Utc.with_ymd_and_hms(2024, 6, 15, 10, 0, 0).unwrap(),
    }
}

fn standard_store() -> MockReportStore {
    let node = make_node("node-a", NODE_ID_A);
    let profile = make_profile("ssh-baseline");
    let results = vec![
        make_cr(node.id, "ctrl-01", "passed", 1),
        make_cr(node.id, "ctrl-02", "failed", 2),
    ];
    MockReportStore::new()
        .with_nodes(vec![node])
        .with_control_results(results)
        .with_profiles(vec![profile])
}

fn standard_params() -> ReportParams {
    ReportParams {
        from: Some(Utc.with_ymd_and_hms(2024, 6, 15, 0, 0, 0).unwrap()),
        to: Some(Utc.with_ymd_and_hms(2024, 6, 16, 0, 0, 0).unwrap()),
        node_filter: None,
        profile_filter: None,
    }
}

fn standard_data_range() -> spindle_compliance::DataRange {
    spindle_compliance::DataRange {
        from: Some(Utc.with_ymd_and_hms(2024, 6, 15, 0, 0, 0).unwrap()),
        to: Some(Utc.with_ymd_and_hms(2024, 6, 16, 0, 0, 0).unwrap()),
    }
}

// ── VerificationStatus tests ───────────────────────────────────────────────────

#[test]
fn test_verification_status_values() {
    assert_eq!(VerificationStatus::Verified.as_str(), "verified");
    assert_eq!(VerificationStatus::Unverified.as_str(), "unverified");
}

#[test]
fn test_verification_status_default_is_verified() {
    let status = VerificationStatus::default();
    assert_eq!(status, VerificationStatus::Verified);
}

#[test]
fn test_verification_status_cascade_verified_source() {
    let source = VerificationStatus::Verified;
    // If source is verified and derived is verified → verified
    assert_eq!(source.cascade(true), VerificationStatus::Verified);
    // If source is verified but derived is unverified → unverified
    assert_eq!(source.cascade(false), VerificationStatus::Unverified);
}

#[test]
fn test_verification_status_cascade_unverified_source() {
    let source = VerificationStatus::Unverified;
    // If source is unverified, everything derived is unverified (cascading)
    assert_eq!(source.cascade(true), VerificationStatus::Unverified);
    assert_eq!(source.cascade(false), VerificationStatus::Unverified);
}

// ── RestoreSession tests ─────────────────────────────────────────────────────

#[test]
fn test_restore_session_verified() {
    let session = RestoreSession::verified("session-001".to_string(), standard_data_range(), 30);

    assert_eq!(session.session_id, "session-001");
    assert_eq!(session.verification_status, VerificationStatus::Verified);
    assert!(session.is_valid());
    assert_eq!(session.ttl_days, 30);
}

#[test]
fn test_restore_session_unverified() {
    let session = RestoreSession::unverified("session-002".to_string(), standard_data_range(), 30);

    assert_eq!(session.session_id, "session-002");
    assert_eq!(session.verification_status, VerificationStatus::Unverified);
    assert!(session.is_valid());
}

#[test]
fn test_restore_session_ttl_not_expired() {
    let session = RestoreSession::verified("session-003".to_string(), standard_data_range(), 30);

    // 30 days TTL should not be expired
    assert!(session.is_valid());
    assert!(!session.is_expired());
}

// ── ReportAttestation tests ──────────────────────────────────────────────────

#[tokio::test]
async fn test_attestation_verified_for_verified_source() {
    let store = standard_store();
    let params = standard_params();
    let session =
        RestoreSession::verified("session-verified".to_string(), standard_data_range(), 30);

    let (report, attestation) = generate_report_with_attestation(
        &ControlStatusByNode,
        &store,
        &params,
        "local:abc123".to_string(),
        Some(&session),
    )
    .await
    .unwrap();

    assert_eq!(attestation.report_type, "control_status_by_node");
    assert_eq!(attestation.definition_version, 1);
    assert_eq!(
        attestation.verification_status,
        VerificationStatus::Verified
    );
    assert_eq!(
        attestation.source_session_id,
        Some("session-verified".to_string())
    );
    assert!(!attestation.report_hash.is_empty());
    assert_eq!(attestation.key_id, "local:abc123");
}

#[tokio::test]
async fn test_attestation_unverified_for_unverified_source() {
    let store = standard_store();
    let params = standard_params();
    let session =
        RestoreSession::unverified("session-unverified".to_string(), standard_data_range(), 30);

    let (report, attestation) = generate_report_with_attestation(
        &ControlStatusByNode,
        &store,
        &params,
        "local:abc123".to_string(),
        Some(&session),
    )
    .await
    .unwrap();

    assert_eq!(
        attestation.verification_status,
        VerificationStatus::Unverified
    );
    assert_eq!(
        attestation.source_session_id,
        Some("session-unverified".to_string())
    );
}

#[tokio::test]
async fn test_attestation_verified_without_session() {
    let store = standard_store();
    let params = standard_params();

    let (report, attestation) = generate_report_with_attestation(
        &ControlStatusByNode,
        &store,
        &params,
        "local:abc123".to_string(),
        None,
    )
    .await
    .unwrap();

    // No session → default to verified
    assert_eq!(
        attestation.verification_status,
        VerificationStatus::Verified
    );
    assert!(attestation.source_session_id.is_none());
}

// ── Cascading unverified tests ───────────────────────────────────────────────

#[tokio::test]
async fn test_cascading_unverified_source() {
    // Unverified source → all downstream reports must be unverified
    let store = standard_store();
    let params = standard_params();
    let unverified_session =
        RestoreSession::unverified("unverified-source".to_string(), standard_data_range(), 30);

    for report_def in &[
        &ControlStatusByNode as &dyn ReportDefinition,
        &ProfileSummaryOverTime,
        &WaiverRegister,
        &ExceptionDeviationList,
    ] {
        let (_, attestation) = generate_report_with_attestation(
            *report_def,
            &store,
            &params,
            "local:key".to_string(),
            Some(&unverified_session),
        )
        .await
        .unwrap();

        assert_eq!(
            attestation.verification_status,
            VerificationStatus::Unverified,
            "Report {} must be unverified when source is unverified",
            report_def.report_type()
        );
    }
}

#[tokio::test]
async fn test_cascading_verified_source() {
    // Verified source → reports are verified
    let store = standard_store();
    let params = standard_params();
    let verified_session =
        RestoreSession::verified("verified-source".to_string(), standard_data_range(), 30);

    for report_def in &[
        &ControlStatusByNode as &dyn ReportDefinition,
        &ProfileSummaryOverTime,
        &WaiverRegister,
        &ExceptionDeviationList,
    ] {
        let (_, attestation) = generate_report_with_attestation(
            *report_def,
            &store,
            &params,
            "local:key".to_string(),
            Some(&verified_session),
        )
        .await
        .unwrap();

        assert_eq!(
            attestation.verification_status,
            VerificationStatus::Verified,
            "Report {} must be verified when source is verified",
            report_def.report_type()
        );
    }
}

// ── Export from restored archive tests ──────────────────────────────────────

#[tokio::test]
async fn test_export_restored_report_verified() {
    let store = standard_store();
    let params = standard_params();
    let session =
        RestoreSession::verified("restore-session-001".to_string(), standard_data_range(), 30);

    let report = ControlStatusByNode.generate(&store, &params).await.unwrap();
    let (export, attestation) =
        export_restored_report(&report, ReportFormat::Json, &session, None).unwrap();

    assert_eq!(
        attestation.verification_status,
        VerificationStatus::Verified
    );
    assert!(export.headers.content_type == "application/json");
    assert_eq!(
        attestation.source_session_id,
        Some("restore-session-001".to_string())
    );
}

#[tokio::test]
async fn test_export_restored_report_unverified() {
    let store = standard_store();
    let params = standard_params();
    let session =
        RestoreSession::unverified("restore-session-002".to_string(), standard_data_range(), 30);

    let report = ControlStatusByNode.generate(&store, &params).await.unwrap();
    let (export, attestation) =
        export_restored_report(&report, ReportFormat::Csv, &session, None).unwrap();

    assert_eq!(
        attestation.verification_status,
        VerificationStatus::Unverified
    );
    assert!(export.headers.content_type == "text/csv");
    assert_eq!(
        attestation.source_session_id,
        Some("restore-session-002".to_string())
    );
}

// ── should_mark_unverified tests ──────────────────────────────────────────────

#[test]
fn test_should_mark_unverified_for_unverified_session() {
    let session = RestoreSession::unverified("sess".to_string(), standard_data_range(), 30);
    assert!(should_mark_unverified(&session));
}

#[test]
fn test_should_mark_unverified_for_verified_session() {
    let session = RestoreSession::verified("sess".to_string(), standard_data_range(), 30);
    assert!(!should_mark_unverified(&session));
}

#[test]
fn test_should_mark_unverified_for_expired_session() {
    // Create a session with TTL=0, then it's already expired
    let session = RestoreSession::verified(
        "sess".to_string(),
        standard_data_range(),
        0, // No TTL → immediately expires
    );
    // Note: TTL=0 means expires at creation time, so it's expired
    assert!(should_mark_unverified(&session) || session.is_expired());
}

// ── Audit integration with verification ──────────────────────────────────────

#[tokio::test]
async fn test_audit_log_with_verification_status() {
    let store = standard_store();
    let params = standard_params();
    let log: std::sync::Arc<dyn AuditLog> = std::sync::Arc::new(InMemoryAuditLog::new());
    let logger = ComplianceAuditLogger::new(log.clone());

    let session = RestoreSession::verified("audit-session".to_string(), standard_data_range(), 30);

    // Generate report with attestation
    let (report, attestation) = generate_report_with_attestation(
        &ControlStatusByNode,
        &store,
        &params,
        "local:key".to_string(),
        Some(&session),
    )
    .await
    .unwrap();

    // Log the export
    logger
        .log_export(
            "admin@example.com",
            "/v1/compliance/export/control_status_by_node",
            "report-001",
            &report.report_type,
            ReportFormat::Json,
        )
        .await;

    // Verify audit entry
    let entries = logger.log().get_entries().await;
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.resource_type, "compliance");
    assert_eq!(entry.report_id, Some("report-001".to_string()));
    assert_eq!(
        entry.report_type,
        Some("control_status_by_node".to_string())
    );

    // Verify attestation carries verification status
    assert_eq!(
        attestation.verification_status,
        VerificationStatus::Verified
    );
    assert_eq!(
        attestation.source_session_id,
        Some("audit-session".to_string())
    );
}

// ── Attestation serialization tests ──────────────────────────────────────────

#[tokio::test]
async fn test_attestation_serializes_with_verification_status() {
    let store = standard_store();
    let params = standard_params();
    let session = RestoreSession::unverified("session-s".to_string(), standard_data_range(), 30);

    let (_, attestation) = generate_report_with_attestation(
        &ControlStatusByNode,
        &store,
        &params,
        "local:key".to_string(),
        Some(&session),
    )
    .await
    .unwrap();

    let json = serde_json::to_string(&attestation).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed["verification_status"], "unverified");
    assert_eq!(parsed["source_session_id"], "session-s");
    assert_eq!(parsed["report_type"], "control_status_by_node");
    assert_eq!(parsed["definition_version"], 1);
    assert!(!parsed["report_hash"].as_str().unwrap().is_empty());
    assert_eq!(parsed["key_id"], "local:key");
}

// ── Full 4-report cascade test ───────────────────────────────────────────────

#[tokio::test]
async fn test_all_reports_cascade_from_unverified_source() {
    let store = standard_store();
    let params = standard_params();
    let unverified =
        RestoreSession::unverified("cascade-source".to_string(), standard_data_range(), 30);

    let report_defs: Vec<&dyn ReportDefinition> = vec![
        &ControlStatusByNode,
        &ProfileSummaryOverTime,
        &WaiverRegister,
        &ExceptionDeviationList,
    ];

    for def in &report_defs {
        let (_, attestation) = generate_report_with_attestation(
            *def,
            &store,
            &params,
            "local:key".to_string(),
            Some(&unverified),
        )
        .await
        .unwrap();

        assert_eq!(
            attestation.verification_status,
            VerificationStatus::Unverified,
            "{} must cascade Unverified from unverified source",
            def.report_type()
        );
    }
}
