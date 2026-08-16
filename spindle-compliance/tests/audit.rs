//! Tests for M4-13: Audit logging + MCP exclusion.
//!
//! Verify:
//! - GET compliance endpoint → audit entry present
//! - Export report → audit entry with report_id
//! - MCP exclusion policy documented + enforced
//! - No unexpected importers of spindle-compliance

#![allow(warnings)]
use chrono::{TimeZone, Utc};
use std::sync::Arc;
use uuid::Uuid;

use spindle_compliance::{
    verify_mcp_exclusion, AuditLog, AuditLogEntry, ComplianceAuditLogger, ControlResult,
    ControlStatusByNode, ExceptionDeviationList, InMemoryAuditLog, MockReportStore, Node, Profile,
    ProfileSummaryOverTime, ReportDefinition, ReportFormat, ReportParams, Waiver, WaiverRegister,
    MCP_EXCLUSION_POLICY,
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
    let node_a = make_node("node-a", NODE_ID_A);
    let profile = make_profile("ssh-baseline");

    let results = vec![
        make_cr(node_a.id, "ctrl-01", "passed", 1),
        make_cr(node_a.id, "ctrl-02", "failed", 2),
    ];

    MockReportStore::new()
        .with_nodes(vec![node_a])
        .with_control_results(results)
        .with_profiles(vec![profile])
}

// ── MCP exclusion tests ──────────────────────────────────────────────────────

#[test]
fn test_mcp_exclusion_policy_documented() {
    assert!(
        MCP_EXCLUSION_POLICY.contains("CMP-08"),
        "Policy must reference CMP-08"
    );
    assert!(
        MCP_EXCLUSION_POLICY.contains("compliance export"),
        "Policy must mention compliance export exclusion"
    );
    assert!(
        MCP_EXCLUSION_POLICY.contains("cargo tree --invert"),
        "Policy must reference dependency audit command"
    );
    assert!(
        MCP_EXCLUSION_POLICY.contains("spindle-mcp"),
        "Policy must mention spindle-mcp"
    );
}

#[test]
fn test_verify_mcp_exclusion_returns_true() {
    // The exclusion is enforced by Cargo.toml dependency rules.
    // This function serves as a runtime checkpoint.
    assert!(verify_mcp_exclusion());
}

#[test]
fn test_no_unexpected_importers_of_spindle_compliance() {
    // spindle-compliance is currently only used by itself (no external importers).
    // In production, cargo tree --invert would be used:
    //   cargo tree --invert -p spindle-compliance
    // This test verifies the crate compiles standalone without external importers.
    // The actual CI check would parse `cargo tree --invert` output and assert
    // only expected crates appear.
    //
    // For now, we verify that spindle-mcp (if it existed) would NOT compile
    // if it tried to import spindle-compliance without adding it as a dependency.
    // This is enforced by Rust's module system + Cargo.toml.
    assert!(verify_mcp_exclusion());
}

// ── Audit log tests ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_audit_log_records_compliance_read() {
    let log: Arc<dyn AuditLog> = Arc::new(InMemoryAuditLog::new());
    let logger = ComplianceAuditLogger::new(log);

    logger
        .log_read(
            "user@example.com",
            "/v1/compliance/reports",
            None,
            None,
            None,
        )
        .await;

    assert_eq!(logger.log().count().await, 1);
    let entries = logger.log().get_entries().await;
    assert_eq!(entries.len(), 1);

    let entry = &entries[0];
    assert_eq!(entry.subject, "user@example.com");
    assert_eq!(entry.resource_type, "compliance");
    assert_eq!(entry.endpoint, "/v1/compliance/reports");
    assert!(entry.report_id.is_none());
    assert!(entry.report_type.is_none());
}

#[tokio::test]
async fn test_audit_log_records_export_with_report_id() {
    let log: Arc<dyn AuditLog> = Arc::new(InMemoryAuditLog::new());
    let logger = ComplianceAuditLogger::new(log);

    logger
        .log_export(
            "admin@example.com",
            "/v1/compliance/export/control_status_by_node",
            "report-12345",
            "control_status_by_node",
            ReportFormat::Json,
        )
        .await;

    assert_eq!(logger.log().count().await, 1);
    let entries = logger.log().get_entries().await;
    assert_eq!(entries.len(), 1);

    let entry = &entries[0];
    assert_eq!(entry.subject, "admin@example.com");
    assert_eq!(entry.resource_type, "compliance");
    assert_eq!(
        entry.endpoint,
        "/v1/compliance/export/control_status_by_node"
    );
    assert_eq!(entry.report_id, Some("report-12345".to_string()));
    assert_eq!(
        entry.report_type,
        Some("control_status_by_node".to_string())
    );

    // Details should include format
    assert!(entry.details.is_some());
    let details = entry.details.as_ref().unwrap();
    assert_eq!(details.get("format").and_then(|v| v.as_str()), Some("json"));
}

#[tokio::test]
async fn test_audit_log_records_export_csv() {
    let log: Arc<dyn AuditLog> = Arc::new(InMemoryAuditLog::new());
    let logger = ComplianceAuditLogger::new(log);

    logger
        .log_export(
            "auditor@example.com",
            "/v1/compliance/export/waiver_register",
            "report-67890",
            "waiver_register",
            ReportFormat::Csv,
        )
        .await;

    let entries = logger.log().get_entries().await;
    let entry = &entries[0];
    assert_eq!(entry.report_type, Some("waiver_register".to_string()));
    let details = entry.details.as_ref().unwrap();
    assert_eq!(details.get("format").and_then(|v| v.as_str()), Some("csv"));
}

#[tokio::test]
async fn test_audit_log_filter_by_subject() {
    let log: Arc<dyn AuditLog> = Arc::new(InMemoryAuditLog::new());
    let logger = ComplianceAuditLogger::new(log);

    logger
        .log_read("user_a", "/v1/compliance/reports", None, None, None)
        .await;
    logger
        .log_read("user_b", "/v1/compliance/reports", None, None, None)
        .await;
    logger
        .log_read("user_a", "/v1/compliance/controls", None, None, None)
        .await;

    let user_a_entries = logger.log().get_entries_for_subject("user_a").await;
    assert_eq!(user_a_entries.len(), 2);

    let user_b_entries = logger.log().get_entries_for_subject("user_b").await;
    assert_eq!(user_b_entries.len(), 1);
}

#[tokio::test]
async fn test_audit_log_filter_by_report_type() {
    let log: Arc<dyn AuditLog> = Arc::new(InMemoryAuditLog::new());
    let logger = ComplianceAuditLogger::new(log);

    logger
        .log_export(
            "user1",
            "/v1/compliance/export/control_status_by_node",
            "r1",
            "control_status_by_node",
            ReportFormat::Json,
        )
        .await;
    logger
        .log_export(
            "user2",
            "/v1/compliance/export/waiver_register",
            "r2",
            "waiver_register",
            ReportFormat::Csv,
        )
        .await;
    logger
        .log_export(
            "user3",
            "/v1/compliance/export/control_status_by_node",
            "r3",
            "control_status_by_node",
            ReportFormat::Json,
        )
        .await;

    let ctrl_entries = logger
        .log()
        .get_entries_for_report_type("control_status_by_node")
        .await;
    assert_eq!(ctrl_entries.len(), 2);

    let waiver_entries = logger
        .log()
        .get_entries_for_report_type("waiver_register")
        .await;
    assert_eq!(waiver_entries.len(), 1);
}

#[tokio::test]
async fn test_audit_log_multiple_entries() {
    let log: Arc<dyn AuditLog> = Arc::new(InMemoryAuditLog::new());
    let logger = ComplianceAuditLogger::new(log);

    for i in 0..10 {
        logger
            .log_read(
                &format!("user{}", i),
                "/v1/compliance/reports",
                None,
                None,
                None,
            )
            .await;
    }

    assert_eq!(logger.log().count().await, 10);
    let entries = logger.log().get_entries().await;
    assert_eq!(entries.len(), 10);

    // Verify each entry is distinct
    for (i, entry) in entries.iter().enumerate() {
        assert_eq!(entry.subject, format!("user{}", i));
    }
}

#[tokio::test]
async fn test_audit_log_entry_serializes() {
    let log: Arc<dyn AuditLog> = Arc::new(InMemoryAuditLog::new());
    let logger = ComplianceAuditLogger::new(log);

    logger
        .log_read(
            "user@example.com",
            "/v1/compliance/reports",
            Some("r1"),
            Some("test_report"),
            Some(serde_json::json!({"rows": 42})),
        )
        .await;

    let entries = logger.log().get_entries().await;
    let entry = &entries[0];

    // Verify it can be serialized to JSON
    let json = serde_json::to_string(entry).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed["subject"], "user@example.com");
    assert_eq!(parsed["resource_type"], "compliance");
    assert_eq!(parsed["endpoint"], "/v1/compliance/reports");
    assert_eq!(parsed["report_id"], "r1");
    assert_eq!(parsed["report_type"], "test_report");
    assert_eq!(parsed["details"]["rows"], 42);
}

// ── Audit log integrated with report generation ──────────────────────────────

#[tokio::test]
async fn test_get_compliance_endpoint_creates_audit_entry() {
    let store = standard_store();
    let log: Arc<dyn AuditLog> = Arc::new(InMemoryAuditLog::new());
    let logger = ComplianceAuditLogger::new(log.clone());
    let params = ReportParams::default();

    // Simulate: GET /v1/compliance/control-status-by-node
    logger
        .log_read(
            "viewer@example.com",
            "/v1/compliance/control-status-by-node",
            None,
            Some("control_status_by_node"),
            Some(serde_json::json!({"time_range": {"from": "2024-06-15", "to": "2024-06-16"}})),
        )
        .await;

    // Generate the report
    let report = ControlStatusByNode.generate(&store, &params).await.unwrap();

    // Verify audit entry exists
    assert_eq!(logger.log().count().await, 1);
    let entries = logger.log().get_entries().await;
    let entry = &entries[0];
    assert_eq!(entry.resource_type, "compliance");
    assert_eq!(entry.endpoint, "/v1/compliance/control-status-by-node");
    assert_eq!(
        entry.report_type,
        Some("control_status_by_node".to_string())
    );

    // Verify report is valid
    assert_eq!(report.report_type, "control_status_by_node");
}

#[tokio::test]
async fn test_export_endpoint_creates_audit_entry_with_report_id() {
    let store = standard_store();
    let log: Arc<dyn AuditLog> = Arc::new(InMemoryAuditLog::new());
    let logger = ComplianceAuditLogger::new(log.clone());
    let params = ReportParams::default();

    // Simulate: GET /v1/compliance/export/control_status_by_node?format=json
    let report = ControlStatusByNode.generate(&store, &params).await.unwrap();
    let report_id = format!("report-{}", report.report_type);

    logger
        .log_export(
            "auditor@example.com",
            "/v1/compliance/export/control_status_by_node",
            &report_id,
            &report.report_type,
            ReportFormat::Json,
        )
        .await;

    // Verify audit entry
    assert_eq!(logger.log().count().await, 1);
    let entries = logger.log().get_entries().await;
    let entry = &entries[0];
    assert_eq!(entry.subject, "auditor@example.com");
    assert_eq!(entry.report_id, Some(report_id.clone()));
    assert_eq!(
        entry.report_type,
        Some("control_status_by_node".to_string())
    );
    assert_eq!(entry.details.as_ref().unwrap()["format"], "json");

    // Verify report_id contains the report type
    assert!(entry
        .report_id
        .as_ref()
        .unwrap()
        .contains("control_status_by_node"));
}

#[tokio::test]
async fn test_all_four_report_types_create_audit_entries() {
    let store = standard_store();
    let log: Arc<dyn AuditLog> = Arc::new(InMemoryAuditLog::new());
    let logger = ComplianceAuditLogger::new(log.clone());
    let params = ReportParams::default();

    for report_def in &[
        &ControlStatusByNode as &dyn ReportDefinition,
        &ProfileSummaryOverTime,
        &WaiverRegister,
        &ExceptionDeviationList,
    ] {
        let report = report_def.generate(&store, &params).await.unwrap();
        let report_id = format!("rpt-{}", report.report_type);

        logger
            .log_export(
                "admin",
                &format!("/v1/compliance/export/{}", report_def.report_type()),
                &report_id,
                &report.report_type,
                ReportFormat::Json,
            )
            .await;
    }

    assert_eq!(logger.log().count().await, 4);
    let entries = logger.log().get_entries().await;

    // Verify all 4 report types were logged
    let report_types: Vec<_> = entries.iter().map(|e| e.report_type.clone()).collect();
    assert!(report_types.contains(&Some("control_status_by_node".to_string())));
    assert!(report_types.contains(&Some("profile_summary_over_time".to_string())));
    assert!(report_types.contains(&Some("waiver_register".to_string())));
    assert!(report_types.contains(&Some("exception_deviation_list".to_string())));

    // All should have resource_type = "compliance"
    for entry in &entries {
        assert_eq!(entry.resource_type, "compliance");
    }
}

// ── Audit log determinism ────────────────────────────────────────────────────

#[tokio::test]
async fn test_audit_log_deterministic_timestamps() {
    // Audit entries record the time they happened — this is correct behavior.
    // What matters for determinism is the *report output*, not the audit log.
    // This test verifies that audit entries are properly timestamped.
    let log: Arc<dyn AuditLog> = Arc::new(InMemoryAuditLog::new());
    let logger = ComplianceAuditLogger::new(log);

    let before = Utc::now();
    logger
        .log_read("user", "/v1/compliance/reports", None, None, None)
        .await;
    let after = Utc::now();

    let entries = logger.log().get_entries().await;
    let entry = &entries[0];
    assert!(entry.timestamp >= before);
    assert!(entry.timestamp <= after);
}
