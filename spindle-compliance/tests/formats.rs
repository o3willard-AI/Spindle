//! Tests for M4-11: Report formats + types.
//!
//! Verify:
//! - Each report type × JSON + CSV = 8 output variants, all deterministic
//! - Filters work correctly (node_filter, profile_filter)
//! - CSV has deterministic column ordering and proper escaping
//! - Export headers include Content-Disposition, signing placeholders

use chrono::{TimeZone, Utc};
use std::str::FromStr;
use uuid::Uuid;

use spindle_compliance::{
    canonical_serialize_report, report_hash, export_report, Report, ReportDefinition, ReportFormat,
    ReportParams, ControlStatusByNode, ExceptionDeviationList, ProfileSummaryOverTime,
    WaiverRegister, MockReportStore, Node, Profile, ControlResult, Waiver,
};

// ── Fixed UUID constants for deterministic testing ───────────────────────────

fn uuid_from(s: &str) -> Uuid {
    Uuid::parse_str(s).unwrap()
}

const PROFILE_ID: &str = "00000000-0000-0000-0000-000000000001";
const NODE_ID_A: &str = "00000000-0000-0000-0000-00000000000a";
const NODE_ID_B: &str = "00000000-0000-0000-0000-00000000000b";

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
        source: "inspec".to_string(),
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

fn make_waiver(control_id: &str, scope: &str, approver: &str, seq: u32) -> Waiver {
    Waiver {
        id: uuid_from(&format!("00000000-0000-0000-0064-{:012x}", seq)),
        control_id: control_id.to_string(),
        profile_id: uuid_from(PROFILE_ID),
        scope: scope.to_string(),
        justification: Some("false positive".to_string()),
        approver: Some(approver.to_string()),
        start_date: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        expiry_date: Utc.with_ymd_and_hms(2025, 12, 31, 23, 59, 59).unwrap(),
        created_at: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
    }
}

fn standard_store() -> MockReportStore {
    let node_a = make_node("node-a", NODE_ID_A);
    let node_b = make_node("node-b", NODE_ID_B);
    let profile = make_profile("ssh-baseline");

    let results = vec![
        make_cr(node_a.id, "ctrl-01", "passed", 1),
        make_cr(node_a.id, "ctrl-02", "failed", 2),
        make_cr(node_b.id, "ctrl-01", "failed", 3),
    ];

    let waivers = vec![make_waiver("ctrl-02", "project-a", "admin", 1)];

    MockReportStore::new()
        .with_nodes(vec![node_a, node_b])
        .with_control_results(results)
        .with_profiles(vec![profile])
        .with_waivers(waivers)
}

// ── ReportFormat tests ───────────────────────────────────────────────────────

#[test]
fn test_report_format_parsing() {
    assert_eq!(ReportFormat::from_str("json").unwrap(), ReportFormat::Json);
    assert_eq!(ReportFormat::from_str("JSON").unwrap(), ReportFormat::Json);
    assert_eq!(ReportFormat::from_str("csv").unwrap(), ReportFormat::Csv);
    assert_eq!(ReportFormat::from_str("CSV").unwrap(), ReportFormat::Csv);
    assert!(ReportFormat::from_str("xml").is_err());
}

#[test]
fn test_report_format_extension() {
    assert_eq!(ReportFormat::Json.extension(), "json");
    assert_eq!(ReportFormat::Csv.extension(), "csv");
}

#[test]
fn test_report_format_as_str() {
    assert_eq!(ReportFormat::Json.as_str(), "json");
    assert_eq!(ReportFormat::Csv.as_str(), "csv");
}

#[test]
fn test_report_format_default() {
    assert_eq!(ReportFormat::default(), ReportFormat::Json);
}

// ── Export: JSON format (8 deterministic variants) ───────────────────────────

#[tokio::test]
async fn test_export_json_deterministic_all_reports() {
    let store = standard_store();
    let params = ReportParams::default();

    for report_def in &[
        &ControlStatusByNode as &dyn ReportDefinition,
        &ProfileSummaryOverTime,
        &WaiverRegister,
        &ExceptionDeviationList,
    ] {
        let report = report_def.generate(&store, &params).await.unwrap();

        // Export JSON twice
        let export1 = export_report(&report, ReportFormat::Json).unwrap();
        let export2 = export_report(&report, ReportFormat::Json).unwrap();

        // Byte-identical
        assert_eq!(
            export1.bytes, export2.bytes,
            "JSON export must be byte-identical for {}",
            report_def.report_type()
        );

        // SHA256 match
        assert_eq!(
            report_hash(&report),
            report_hash(&report),
            "Report hash must be consistent"
        );
    }
}

#[tokio::test]
async fn test_export_json_content_type() {
    let store = standard_store();
    let params = ReportParams::default();

    let report = ControlStatusByNode.generate(&store, &params).await.unwrap();
    let export = export_report(&report, ReportFormat::Json).unwrap();

    assert_eq!(export.headers.content_type, "application/json");
    assert_eq!(
        export.headers.content_disposition,
        "attachment; filename=\"control_status_by_node.json\""
    );
}

#[tokio::test]
async fn test_export_json_sorted_keys() {
    let store = standard_store();
    let params = ReportParams::default();

    let report = ControlStatusByNode.generate(&store, &params).await.unwrap();
    let export = export_report(&report, ReportFormat::Json).unwrap();

    let json_str = std::str::from_utf8(&export.bytes).unwrap();
    // First key after { should be "data_range" (alphabetically before "data" and "definition_version")
    // Actually: "data", "data_range", "definition_version", "report_type" — but BTreeMap sorts them
    // "data" < "data_range" < "definition_version" < "report_type"
    assert!(json_str.starts_with('{'));
    // Check that "data_range" comes before "data" — no, "data" < "data_range" lexicographically
    // Actually "data" is a prefix of "data_range", so "data" < "data_range"
    let data_pos = json_str.find("\"data\"").unwrap();
    let range_pos = json_str.find("\"data_range\"").unwrap();
    let version_pos = json_str.find("\"definition_version\"").unwrap();
    let type_pos = json_str.find("\"report_type\"").unwrap();

    assert!(data_pos < range_pos);
    assert!(range_pos < version_pos);
    assert!(version_pos < type_pos);
}

// ── Export: CSV format (8 deterministic variants) ────────────────────────────

#[tokio::test]
async fn test_export_csv_deterministic_all_reports() {
    let store = standard_store();
    let params = ReportParams::default();

    for report_def in &[
        &ControlStatusByNode as &dyn ReportDefinition,
        &ProfileSummaryOverTime,
        &WaiverRegister,
        &ExceptionDeviationList,
    ] {
        let report = report_def.generate(&store, &params).await.unwrap();

        // Export CSV twice
        let export1 = export_report(&report, ReportFormat::Csv).unwrap();
        let export2 = export_report(&report, ReportFormat::Csv).unwrap();

        // Byte-identical
        assert_eq!(
            export1.bytes, export2.bytes,
            "CSV export must be byte-identical for {}",
            report_def.report_type()
        );
    }
}

#[tokio::test]
async fn test_export_csv_content_type() {
    let store = standard_store();
    let params = ReportParams::default();

    let report = ControlStatusByNode.generate(&store, &params).await.unwrap();
    let export = export_report(&report, ReportFormat::Csv).unwrap();

    assert_eq!(export.headers.content_type, "text/csv");
    assert_eq!(
        export.headers.content_disposition,
        "attachment; filename=\"control_status_by_node.csv\""
    );
}

#[tokio::test]
async fn test_csv_header_order_control_status_by_node() {
    let store = standard_store();
    let params = ReportParams::default();

    let report = ControlStatusByNode.generate(&store, &params).await.unwrap();
    let export = export_report(&report, ReportFormat::Csv).unwrap();

    let header = std::str::from_utf8(&export.bytes).unwrap().lines().next().unwrap();
    assert_eq!(
        header,
        "node_name,platform,chef_environment,control_id,status,results_count,first_seen,last_seen"
    );
}

#[tokio::test]
async fn test_csv_header_order_profile_summary_over_time() {
    let store = standard_store();
    let params = ReportParams::default();

    let report = ProfileSummaryOverTime.generate(&store, &params).await.unwrap();
    let export = export_report(&report, ReportFormat::Csv).unwrap();

    let header = std::str::from_utf8(&export.bytes).unwrap().lines().next().unwrap();
    assert_eq!(
        header,
        "profile_name,time_bucket,passed,failed,skipped,waived,other,total"
    );
}

#[tokio::test]
async fn test_csv_header_order_waiver_register() {
    let store = standard_store();
    let params = ReportParams::default();

    let report = WaiverRegister.generate(&store, &params).await.unwrap();
    let export = export_report(&report, ReportFormat::Csv).unwrap();

    let header = std::str::from_utf8(&export.bytes).unwrap().lines().next().unwrap();
    assert_eq!(
        header,
        "control_id,profile_id,scope,approver,start_date,expiry_date,justification"
    );
}

#[tokio::test]
async fn test_csv_header_order_exception_deviation_list() {
    let store = standard_store();
    let params = ReportParams::default();

    let report = ExceptionDeviationList.generate(&store, &params).await.unwrap();
    let export = export_report(&report, ReportFormat::Csv).unwrap();

    let header = std::str::from_utf8(&export.bytes).unwrap().lines().next().unwrap();
    assert_eq!(
        header,
        "control_id,total_results,passed,failed,skipped,waived,first_seen,last_seen"
    );
}

#[tokio::test]
async fn test_csv_deterministic_row_order() {
    let store = standard_store();
    let params = ReportParams::default();

    let report = ControlStatusByNode.generate(&store, &params).await.unwrap();
    let export1 = export_report(&report, ReportFormat::Csv).unwrap();
    let export2 = export_report(&report, ReportFormat::Csv).unwrap();

    assert_eq!(export1.bytes, export2.bytes);
}

// ── CSV escaping tests ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_csv_escaping_with_commas() {
    // Create a waiver with a justification containing a comma
    let node = make_node("node-a", NODE_ID_A);
    let profile = make_profile("ssh-baseline");

    let results = vec![
        make_cr(node.id, "ctrl-01", "passed", 1),
        make_cr(node.id, "ctrl-01", "failed", 2), // deviation
    ];

    let waiver = Waiver {
        id: uuid_from("00000000-0000-0000-0064-000000000003"),
        control_id: "ctrl-01".to_string(),
        profile_id: uuid_from(PROFILE_ID),
        scope: "project-a".to_string(),
        justification: Some("false, positive, with commas".to_string()), // contains commas
        approver: Some("admin, user".to_string()), // contains comma
        start_date: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        expiry_date: Utc.with_ymd_and_hms(2025, 12, 31, 23, 59, 59).unwrap(),
        created_at: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
    };

    let store = MockReportStore::new()
        .with_nodes(vec![node])
        .with_control_results(results)
        .with_profiles(vec![profile])
        .with_waivers(vec![waiver]);

    let params = ReportParams::default();
    let report = WaiverRegister.generate(&store, &params).await.unwrap();
    let export = export_report(&report, ReportFormat::Csv).unwrap();

    let csv = std::str::from_utf8(&export.bytes).unwrap();

    // The fields with commas should be quoted
    assert!(
        csv.contains("\"false, positive, with commas\""),
        "Comma-containing fields should be quoted: {}",
        csv
    );
    assert!(
        csv.contains("\"admin, user\""),
        "Comma-containing approver should be quoted: {}",
        csv
    );
}

#[tokio::test]
async fn test_csv_escaping_with_quotes() {
    let waiver = Waiver {
        id: uuid_from("00000000-0000-0000-0064-000000000004"),
        control_id: "ctrl-01".to_string(),
        profile_id: uuid_from(PROFILE_ID),
        scope: "project-a".to_string(),
        justification: Some("has \"quotes\" inside".to_string()), // contains quotes
        approver: Some("admin".to_string()),
        start_date: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        expiry_date: Utc.with_ymd_and_hms(2025, 12, 31, 23, 59, 59).unwrap(),
        created_at: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
    };

    let store = MockReportStore::new()
        .with_waivers(vec![waiver]);

    let params = ReportParams::default();
    let report = WaiverRegister.generate(&store, &params).await.unwrap();
    let export = export_report(&report, ReportFormat::Csv).unwrap();

    let csv = std::str::from_utf8(&export.bytes).unwrap();

    // Quotes should be doubled: "has ""quotes"" inside"
    assert!(
        csv.contains("\"has \"\"quotes\"\" inside\""),
        "Quotes should be doubled in CSV: {}",
        csv
    );
}

// ── Filter tests ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_node_filter_limits_results() {
    let store = standard_store();

    // Filter by "node-a" → only node-a's results
    let params = ReportParams {
        from: None,
        to: None,
        node_filter: Some("node-a".to_string()),
        profile_filter: None,
    };

    let report = ControlStatusByNode.generate(&store, &params).await.unwrap();
    let json = canonical_serialize_report(&report).unwrap();

    let json_str = std::str::from_utf8(&json).unwrap();
    assert!(json_str.contains("node-a"));
    assert!(!json_str.contains("node-b"));
}

#[tokio::test]
async fn test_node_filter_no_match() {
    let store = standard_store();
    let params = ReportParams {
        from: None,
        to: None,
        node_filter: Some("nonexistent".to_string()),
        profile_filter: None,
    };

    let report = ControlStatusByNode.generate(&store, &params).await.unwrap();
    let json = canonical_serialize_report(&report).unwrap();
    let json_str = std::str::from_utf8(&json).unwrap();

    // Should have empty nodes array
    assert!(!json_str.contains("node-a"));
    assert!(!json_str.contains("node-b"));
}

#[tokio::test]
async fn test_time_range_filter() {
    let node = make_node("node-a", NODE_ID_A);
    let profile = make_profile("ssh-baseline");

    let results = vec![
        make_cr(node.id, "ctrl-01", "passed", 1), // created_at: 2024-06-15T10:00
    ];

    let store = MockReportStore::new()
        .with_nodes(vec![node])
        .with_control_results(results)
        .with_profiles(vec![profile]);

    // Filter to only include results before 2024-06-15T10:00
    let params = ReportParams {
        from: None,
        to: Some(Utc.with_ymd_and_hms(2024, 6, 15, 10, 0, 0).unwrap()),
        node_filter: None,
        profile_filter: None,
    };

    let report = ControlStatusByNode.generate(&store, &params).await.unwrap();
    let json = canonical_serialize_report(&report).unwrap();
    let json_str = std::str::from_utf8(&json).unwrap();

    // Result at exactly 10:00 should be excluded (to is exclusive)
    assert!(json_str.contains("\"controls\":[]") || json_str.contains("\"nodes\":[]"),
        "Time range filter should exclude results at boundary");
}

// ── Export headers tests ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_export_headers_signed_placeholders() {
    let store = standard_store();
    let params = ReportParams::default();

    let report = ControlStatusByNode.generate(&store, &params).await.unwrap();
    let export = export_report(&report, ReportFormat::Json).unwrap();

    // Unsigned export leaves signing headers empty; real signatures are applied
    // via export_report_with_signer (S-phase replaced the old "placeholder" str).
    assert_eq!(export.headers.x_spindle_key_id, "");
    assert_eq!(export.headers.x_spindle_signature, "");
    assert_eq!(export.headers.content_disposition, "attachment; filename=\"control_status_by_node.json\"");
}

#[tokio::test]
async fn test_export_headers_csv_filename() {
    let store = standard_store();
    let params = ReportParams::default();

    let report = WaiverRegister.generate(&store, &params).await.unwrap();
    let export = export_report(&report, ReportFormat::Csv).unwrap();

    assert_eq!(
        export.headers.content_disposition,
        "attachment; filename=\"waiver_register.csv\""
    );
}

// ── Empty data tests ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_export_empty_report_json() {
    let store = MockReportStore::new();
    let params = ReportParams::default();

    let report = ControlStatusByNode.generate(&store, &params).await.unwrap();
    let export = export_report(&report, ReportFormat::Json).unwrap();

    // Should produce valid JSON even with no data
    let json_str = std::str::from_utf8(&export.bytes).unwrap();
    assert!(json_str.starts_with('{'));
    assert!(json_str.contains("\"nodes\""));
    assert!(json_str.contains("[]"));
}

#[tokio::test]
async fn test_export_empty_report_csv() {
    let store = MockReportStore::new();
    let params = ReportParams::default();

    let report = ControlStatusByNode.generate(&store, &params).await.unwrap();
    let export = export_report(&report, ReportFormat::Csv).unwrap();

    let csv = std::str::from_utf8(&export.bytes).unwrap();
    // Should have header row only
    assert!(csv.starts_with("node_name,platform,"));
    // Should have only one line (the header)
    assert_eq!(csv.lines().count(), 1);
}

// ── All 8 variants determinism ───────────────────────────────────────────────

#[tokio::test]
async fn test_all_8_variants_byte_identical() {
    let store = standard_store();
    let params = ReportParams::default();

    let report_defs: Vec<&dyn ReportDefinition> = vec![
        &ControlStatusByNode,
        &ProfileSummaryOverTime,
        &WaiverRegister,
        &ExceptionDeviationList,
    ];

    for report_def in &report_defs {
        let report = report_def.generate(&store, &params).await.unwrap();

        // JSON export twice
        let json1 = export_report(&report, ReportFormat::Json).unwrap();
        let json2 = export_report(&report, ReportFormat::Json).unwrap();
        assert_eq!(json1.bytes, json2.bytes, "JSON must be deterministic for {}", report_def.report_type());

        // CSV export twice
        let csv1 = export_report(&report, ReportFormat::Csv).unwrap();
        let csv2 = export_report(&report, ReportFormat::Csv).unwrap();
        assert_eq!(csv1.bytes, csv2.bytes, "CSV must be deterministic for {}", report_def.report_type());
    }
}

// ── Unknown report type CSV ────────────────────────────────────────────────────

#[tokio::test]
async fn test_export_csv_unknown_type_fails() {
    let mut data = spindle_compliance::ReportData::new();
    data.insert("test".to_string(), serde_json::json!([]));

    let report = Report {
        report_type: "unknown_type".to_string(),
        definition_version: 1,
        data_range: spindle_compliance::DataRange { from: None, to: None },
        data,
    };

    assert!(export_report(&report, ReportFormat::Csv).is_err());
}
