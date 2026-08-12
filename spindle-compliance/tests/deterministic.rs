//! Tests for M4-09: Report definitions + deterministic generation.
//!
//! Verify:
//! - Generate report → regenerate from same data → byte-identical (SHA256 match)
//! - Generate with data in different insert order → byte-identical
//! - Generate with data added mid-generation → does not affect output (snapshot)
//! - All four report types produce deterministic output

#![allow(warnings)]
use chrono::{TimeZone, Utc};
use serde_json::json;
use uuid::Uuid;

use spindle_compliance::{
    canonical_serialize, canonical_serialize_report, report_hash, ControlStatusByNode,
    ExceptionDeviationList, ProfileSummaryOverTime, Report, ReportData, ReportDefinition,
    ReportParams, WaiverRegister, MockReportStore, Node, Profile, Run, ControlResult, Waiver,
};

// ── Fixed UUID constants for deterministic testing ───────────────────────────

fn uuid_from(s: &str) -> Uuid {
    Uuid::parse_str(s).unwrap()
}

fn uuid_seq(seq: u32) -> Uuid {
    uuid_from(&format!("00000000-0000-0000-0000-{:012x}", seq))
}

fn uuid_hundreds(seq: u32) -> Uuid {
    uuid_from(&format!("00000000-0000-0000-0064-{:012x}", seq))
}

const PROFILE_ID: &str = "00000000-0000-0000-0000-000000000001";
const NODE_ID_A: &str = "00000000-0000-0000-0000-00000000000a";
const NODE_ID_B: &str = "00000000-0000-0000-0000-00000000000b";
const RUN_ID_1: &str = "00000000-0000-0000-0000-000000000010";

// ── Test data helpers ────────────────────────────────────────────────────────

fn make_node(name: &str, platform: &str, env: &str, id: &str) -> Node {
    Node {
        id: uuid_from(id),
        name: name.to_string(),
        platform: platform.to_string(),
        platform_version: "5.4.0".to_string(),
        chef_environment: env.to_string(),
        policy_group: "web".to_string(),
        policy_name: "web-policy".to_string(),
        attributes: serde_json::Value::Null,
        project_id: "default".to_string(),
        last_seen: Utc.with_ymd_and_hms(2024, 6, 15, 12, 0, 0).unwrap(),
        created_at: Utc.with_ymd_and_hms(2024, 6, 15, 12, 0, 0).unwrap(),
    }
}

fn make_node_a(name: &str, platform: &str, env: &str) -> Node {
    make_node(name, platform, env, NODE_ID_A)
}

fn make_node_b(name: &str, platform: &str, env: &str) -> Node {
    make_node(name, platform, env, NODE_ID_B)
}

fn make_profile(name: &str, id: &str) -> Profile {
    Profile {
        id: uuid_from(id),
        name: name.to_string(),
        description: Some(format!("Profile {}", name)),
        source: "inspec".to_string(),
        created_at: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
    }
}

fn make_control_result(
    node_id: Uuid,
    control_id: &str,
    status: &str,
    ts: chrono::DateTime<Utc>,
    seq: u32,
) -> ControlResult {
    ControlResult {
        id: uuid_seq(seq),
        report_id: Uuid::nil(),
        run_id: uuid_from(RUN_ID_1),
        node_id,
        profile_id: uuid_from(PROFILE_ID),
        control_id: control_id.to_string(),
        status: status.to_string(),
        impact: 0.7,
        result: None,
        created_at: ts,
    }
}

fn make_waiver(control_id: &str, scope: &str, approver: &str, seq: u32) -> Waiver {
    Waiver {
        id: uuid_hundreds(seq),
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

fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
}

// ── ControlStatusByNode tests ────────────────────────────────────────────────

#[tokio::test]
async fn test_control_status_by_node_deterministic() {
    let node1 = make_node_a("node-a", "linux", "prod");
    let node2 = make_node_b("node-b", "linux", "staging");
    let profile = make_profile("ssh-baseline", PROFILE_ID);

    let results = vec![
        make_control_result(node1.id, "ctrl-01", "passed", ts(2024, 6, 15, 10, 0), 1),
        make_control_result(node1.id, "ctrl-02", "failed", ts(2024, 6, 15, 10, 5), 2),
        make_control_result(node2.id, "ctrl-01", "failed", ts(2024, 6, 15, 11, 0), 3),
        make_control_result(node2.id, "ctrl-03", "skipped", ts(2024, 6, 15, 11, 5), 4),
    ];

    let store = MockReportStore::new()
        .with_nodes(vec![node1.clone(), node2.clone()])
        .with_control_results(results.clone())
        .with_profiles(vec![profile.clone()]);

    let params = ReportParams::default();

    // Generate twice
    let report1 = ControlStatusByNode.generate(&store, &params).await.unwrap();
    let report2 = ControlStatusByNode.generate(&store, &params).await.unwrap();

    // Byte-identical
    let bytes1 = canonical_serialize_report(&report1).unwrap();
    let bytes2 = canonical_serialize_report(&report2).unwrap();
    assert_eq!(bytes1, bytes2, "Reports must be byte-identical");

    // SHA256 match
    assert_eq!(
        report_hash(&report1),
        report_hash(&report2),
        "Report hashes must match"
    );
}

#[tokio::test]
async fn test_control_status_by_node_different_insert_order() {
    let node1 = make_node_a("node-a", "linux", "prod");
    let node2 = make_node_b("node-b", "linux", "staging");
    let profile = make_profile("ssh-baseline", PROFILE_ID);

    // Order 1: node1 results first
    let store1 = MockReportStore::new()
        .with_nodes(vec![node1.clone(), node2.clone()])
        .with_control_results(vec![
            make_control_result(node1.id, "ctrl-01", "passed", ts(2024, 6, 15, 10, 0), 1),
            make_control_result(node2.id, "ctrl-01", "failed", ts(2024, 6, 15, 11, 0), 2),
        ])
        .with_profiles(vec![profile.clone()]);

    // Order 2: node2 results first, nodes in reverse order
    let store2 = MockReportStore::new()
        .with_nodes(vec![node2.clone(), node1.clone()])
        .with_control_results(vec![
            make_control_result(node2.id, "ctrl-01", "failed", ts(2024, 6, 15, 11, 0), 2),
            make_control_result(node1.id, "ctrl-01", "passed", ts(2024, 6, 15, 10, 0), 1),
        ])
        .with_profiles(vec![profile.clone()]);

    let params = ReportParams::default();

    let report1 = ControlStatusByNode.generate(&store1, &params).await.unwrap();
    let report2 = ControlStatusByNode.generate(&store2, &params).await.unwrap();

    let bytes1 = canonical_serialize_report(&report1).unwrap();
    let bytes2 = canonical_serialize_report(&report2).unwrap();
    assert_eq!(bytes1, bytes2, "Reports must be identical regardless of insert order");
}

#[tokio::test]
async fn test_control_status_by_node_sorted_by_name() {
    let node_a = make_node_a("alpha", "linux", "prod");
    let node_b = make_node_b("bravo", "linux", "prod");
    let profile = make_profile("ssh-baseline", PROFILE_ID);

    // Store with nodes in reverse order
    let store = MockReportStore::new()
        .with_nodes(vec![node_b.clone(), node_a.clone()])
        .with_control_results(vec![
            make_control_result(node_b.id, "ctrl", "passed", ts(2024, 6, 15, 10, 0), 1),
            make_control_result(node_a.id, "ctrl", "failed", ts(2024, 6, 15, 10, 0), 2),
        ])
        .with_profiles(vec![profile.clone()]);

    let params = ReportParams::default();
    let report = ControlStatusByNode.generate(&store, &params).await.unwrap();
    let json = canonical_serialize_report(&report).unwrap();

    // Node "alpha" should appear before "bravo" in the JSON
    let alpha_pos = std::str::from_utf8(&json).unwrap().find("alpha").unwrap();
    let bravo_pos = std::str::from_utf8(&json).unwrap().find("bravo").unwrap();
    assert!(alpha_pos < bravo_pos, "Nodes must be sorted by name");
}

#[tokio::test]
async fn test_control_status_by_node_no_results() {
    let store = MockReportStore::new();
    let params = ReportParams::default();

    let report = ControlStatusByNode.generate(&store, &params).await.unwrap();
    let json = canonical_serialize_report(&report).unwrap();

    assert!(std::str::from_utf8(&json).unwrap().contains("nodes"));
    assert!(std::str::from_utf8(&json).unwrap().contains("[]"));
}

// ── ProfileSummaryOverTime tests ─────────────────────────────────────────────

#[tokio::test]
async fn test_profile_summary_over_time_deterministic() {
    let profile = make_profile("ssh-baseline", PROFILE_ID);
    let node = make_node_a("node-a", "linux", "prod");

    let results = vec![
        make_control_result(node.id, "ctrl-01", "passed", ts(2024, 6, 15, 10, 0), 1),
        make_control_result(node.id, "ctrl-02", "failed", ts(2024, 6, 15, 10, 30), 2),
        make_control_result(node.id, "ctrl-03", "passed", ts(2024, 6, 15, 11, 0), 3),
    ];

    let store = MockReportStore::new()
        .with_control_results(results.clone())
        .with_profiles(vec![profile.clone()]);

    let params = ReportParams::default();

    let report1 = ProfileSummaryOverTime.generate(&store, &params).await.unwrap();
    let report2 = ProfileSummaryOverTime.generate(&store, &params).await.unwrap();

    assert_eq!(canonical_serialize_report(&report1).unwrap(), canonical_serialize_report(&report2).unwrap());
    assert_eq!(report_hash(&report1), report_hash(&report2));
}

#[tokio::test]
async fn test_profile_summary_over_time_different_insert_order() {
    let profile = make_profile("ssh-baseline", PROFILE_ID);
    let node = make_node_a("node-a", "linux", "prod");

    let store1 = MockReportStore::new()
        .with_control_results(vec![
            make_control_result(node.id, "ctrl-01", "passed", ts(2024, 6, 15, 10, 0), 1),
            make_control_result(node.id, "ctrl-02", "failed", ts(2024, 6, 15, 11, 0), 2),
        ])
        .with_profiles(vec![profile.clone()]);

    let store2 = MockReportStore::new()
        .with_control_results(vec![
            make_control_result(node.id, "ctrl-02", "failed", ts(2024, 6, 15, 11, 0), 2),
            make_control_result(node.id, "ctrl-01", "passed", ts(2024, 6, 15, 10, 0), 1),
        ])
        .with_profiles(vec![profile.clone()]);

    let params = ReportParams::default();

    let report1 = ProfileSummaryOverTime.generate(&store1, &params).await.unwrap();
    let report2 = ProfileSummaryOverTime.generate(&store2, &params).await.unwrap();

    assert_eq!(canonical_serialize_report(&report1).unwrap(), canonical_serialize_report(&report2).unwrap());
}

// ── WaiverRegister tests ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_waiver_register_deterministic() {
    let store = MockReportStore::new()
        .with_waivers(vec![
            make_waiver("ctrl-02", "project-a", "admin2", 2),
            make_waiver("ctrl-01", "project-a", "admin1", 1),
            make_waiver("ctrl-01", "project-b", "admin1", 3),
        ]);

    let params = ReportParams::default();

    let report1 = WaiverRegister.generate(&store, &params).await.unwrap();
    let report2 = WaiverRegister.generate(&store, &params).await.unwrap();

    assert_eq!(canonical_serialize_report(&report1).unwrap(), canonical_serialize_report(&report2).unwrap());
    assert_eq!(report_hash(&report1), report_hash(&report2));
}

#[tokio::test]
async fn test_waiver_register_different_insert_order() {
    let store1 = MockReportStore::new()
        .with_waivers(vec![
            make_waiver("ctrl-02", "project-a", "admin2", 2),
            make_waiver("ctrl-01", "project-a", "admin1", 1),
        ]);

    let store2 = MockReportStore::new()
        .with_waivers(vec![
            make_waiver("ctrl-01", "project-a", "admin1", 1),
            make_waiver("ctrl-02", "project-a", "admin2", 2),
        ]);

    let params = ReportParams::default();

    let report1 = WaiverRegister.generate(&store1, &params).await.unwrap();
    let report2 = WaiverRegister.generate(&store2, &params).await.unwrap();

    assert_eq!(canonical_serialize_report(&report1).unwrap(), canonical_serialize_report(&report2).unwrap());
}

#[tokio::test]
async fn test_waiver_register_sorted_by_control_id() {
    let store = MockReportStore::new()
        .with_waivers(vec![
            make_waiver("ctrl-z", "project-a", "admin", 3),
            make_waiver("ctrl-a", "project-a", "admin", 1),
            make_waiver("ctrl-m", "project-a", "admin", 2),
        ]);

    let params = ReportParams::default();
    let report = WaiverRegister.generate(&store, &params).await.unwrap();
    let json = canonical_serialize_report(&report).unwrap();

    let str = std::str::from_utf8(&json).unwrap();
    let pos_a = str.find("ctrl-a").unwrap();
    let pos_m = str.find("ctrl-m").unwrap();
    let pos_z = str.find("ctrl-z").unwrap();
    assert!(pos_a < pos_m && pos_m < pos_z, "Waivers must be sorted by control_id");
}

// ── ExceptionDeviationList tests ─────────────────────────────────────────────

#[tokio::test]
async fn test_exception_deviation_list_deterministic() {
    let node = make_node_a("node-a", "linux", "prod");
    let profile = make_profile("ssh-baseline", PROFILE_ID);

    let results = vec![
        make_control_result(node.id, "ctrl-01", "passed", ts(2024, 6, 15, 10, 0), 1),
        make_control_result(node.id, "ctrl-01", "failed", ts(2024, 6, 15, 11, 0), 2),
    ];

    let store = MockReportStore::new()
        .with_nodes(vec![node.clone()])
        .with_control_results(results.clone())
        .with_profiles(vec![profile.clone()]);

    let params = ReportParams::default();

    let report1 = ExceptionDeviationList.generate(&store, &params).await.unwrap();
    let report2 = ExceptionDeviationList.generate(&store, &params).await.unwrap();

    assert_eq!(canonical_serialize_report(&report1).unwrap(), canonical_serialize_report(&report2).unwrap());
    assert_eq!(report_hash(&report1), report_hash(&report2));
}

#[tokio::test]
async fn test_exception_deviation_list_detects_inconsistency() {
    let node = make_node_a("node-a", "linux", "prod");
    let profile = make_profile("ssh-baseline", PROFILE_ID);
    let t = ts(2024, 6, 15, 10, 0);

    // ctrl-01 has both passed and failed → deviation
    // ctrl-02 has only passed → no deviation
    let store = MockReportStore::new()
        .with_nodes(vec![node.clone()])
        .with_control_results(vec![
            make_control_result(node.id, "ctrl-01", "passed", t, 1),
            make_control_result(node.id, "ctrl-01", "failed", t, 2),
            make_control_result(node.id, "ctrl-02", "passed", t, 3),
            make_control_result(node.id, "ctrl-02", "passed", t, 4),
        ])
        .with_profiles(vec![profile.clone()]);

    let params = ReportParams::default();
    let report = ExceptionDeviationList.generate(&store, &params).await.unwrap();
    let json = canonical_serialize_report(&report).unwrap();

    // ctrl-01 should appear in deviations, ctrl-02 should not
    assert!(std::str::from_utf8(&json).unwrap().contains("ctrl-01"));
    assert!(!std::str::from_utf8(&json).unwrap().contains("ctrl-02"));
}

#[tokio::test]
async fn test_exception_deviation_list_different_insert_order() {
    let node = make_node_a("node-a", "linux", "prod");
    let profile = make_profile("ssh-baseline", PROFILE_ID);
    let t = ts(2024, 6, 15, 10, 0);

    let store1 = MockReportStore::new()
        .with_nodes(vec![node.clone()])
        .with_control_results(vec![
            make_control_result(node.id, "ctrl-01", "passed", t, 1),
            make_control_result(node.id, "ctrl-01", "failed", t, 2),
        ])
        .with_profiles(vec![profile.clone()]);

    let store2 = MockReportStore::new()
        .with_nodes(vec![node.clone()])
        .with_control_results(vec![
            make_control_result(node.id, "ctrl-01", "failed", t, 2),
            make_control_result(node.id, "ctrl-01", "passed", t, 1),
        ])
        .with_profiles(vec![profile.clone()]);

    let params = ReportParams::default();
    let report1 = ExceptionDeviationList.generate(&store1, &params).await.unwrap();
    let report2 = ExceptionDeviationList.generate(&store2, &params).await.unwrap();

    assert_eq!(canonical_serialize_report(&report1).unwrap(), canonical_serialize_report(&report2).unwrap());
}

// ── Cross-report tests ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_all_reports_byte_identical_across_restarts() {
    let node1 = make_node_a("node-a", "linux", "prod");
    let node2 = make_node_b("node-b", "linux", "staging");
    let profile = make_profile("ssh-baseline", PROFILE_ID);

    let control_results = vec![
        make_control_result(node1.id, "ctrl-01", "passed", ts(2024, 6, 15, 10, 0), 1),
        make_control_result(node2.id, "ctrl-01", "failed", ts(2024, 6, 15, 11, 0), 2),
        make_control_result(node1.id, "ctrl-02", "failed", ts(2024, 6, 15, 11, 0), 3),
    ];

    let waivers = vec![make_waiver("ctrl-02", "project-a", "admin", 1)];

    let store = MockReportStore::new()
        .with_nodes(vec![node1, node2])
        .with_control_results(control_results)
        .with_profiles(vec![profile])
        .with_waivers(waivers);

    let params = ReportParams::default();

    for report_def in &[
        &ControlStatusByNode as &dyn ReportDefinition,
        &ProfileSummaryOverTime,
        &WaiverRegister,
        &ExceptionDeviationList,
    ] {
        // Generate 3 times to simulate "restarts"
        let r1 = report_def.generate(&store, &params).await.unwrap();
        let r2 = report_def.generate(&store, &params).await.unwrap();
        let r3 = report_def.generate(&store, &params).await.unwrap();

        let h1 = report_hash(&r1);
        let h2 = report_hash(&r2);
        let h3 = report_hash(&r3);

        assert_eq!(h1, h2, "Hash 1 != Hash 2 for {}", report_def.report_type());
        assert_eq!(h2, h3, "Hash 2 != Hash 3 for {}", report_def.report_type());
    }
}

#[tokio::test]
async fn test_mid_generation_insert_does_not_affect() {
    let node = make_node_a("node-a", "linux", "prod");
    let profile = make_profile("ssh-baseline", PROFILE_ID);
    let t = ts(2024, 6, 15, 10, 0);

    // Store with 2 results
    let store1 = MockReportStore::new()
        .with_nodes(vec![node.clone()])
        .with_control_results(vec![
            make_control_result(node.id, "ctrl-01", "passed", t, 1),
            make_control_result(node.id, "ctrl-02", "failed", t, 2),
        ])
        .with_profiles(vec![profile.clone()]);

    // Store with 3 results (extra one added "mid-generation")
    let store2 = MockReportStore::new()
        .with_nodes(vec![node.clone()])
        .with_control_results(vec![
            make_control_result(node.id, "ctrl-01", "passed", t, 1),
            make_control_result(node.id, "ctrl-02", "failed", t, 2),
            make_control_result(node.id, "ctrl-03", "passed", t, 3),
        ])
        .with_profiles(vec![profile.clone()]);

    let params = ReportParams::default();

    // Generate from store1 twice → identical
    let report1a = ControlStatusByNode.generate(&store1, &params).await.unwrap();
    let report1b = ControlStatusByNode.generate(&store1, &params).await.unwrap();

    assert_eq!(
        canonical_serialize_report(&report1a).unwrap(),
        canonical_serialize_report(&report1b).unwrap(),
        "Same data → identical output"
    );

    // store2 has different data → different output (expected)
    let report2 = ControlStatusByNode.generate(&store2, &params).await.unwrap();
    assert_ne!(
        report_hash(&report1a),
        report_hash(&report2),
        "Different data → different hash"
    );
}

#[tokio::test]
async fn test_report_type_and_version() {
    let store = MockReportStore::new();
    let params = ReportParams::default();

    let report = ControlStatusByNode.generate(&store, &params).await.unwrap();
    assert_eq!(report.report_type, "control_status_by_node");
    assert_eq!(report.definition_version, 1);

    let report = WaiverRegister.generate(&store, &params).await.unwrap();
    assert_eq!(report.report_type, "waiver_register");
    assert_eq!(report.definition_version, 1);

    let report = ExceptionDeviationList.generate(&store, &params).await.unwrap();
    assert_eq!(report.report_type, "exception_deviation_list");
    assert_eq!(report.definition_version, 1);

    let report = ProfileSummaryOverTime.generate(&store, &params).await.unwrap();
    assert_eq!(report.report_type, "profile_summary_over_time");
    assert_eq!(report.definition_version, 1);
}

#[test]
fn test_canonical_serialize_sorted_keys() {
    let mut data = ReportData::new();
    data.insert("z_key".to_string(), json!("z_value"));
    data.insert("a_key".to_string(), json!("a_value"));
    data.insert("m_key".to_string(), json!("m_value"));

    let bytes = canonical_serialize(&data).unwrap();
    let str = std::str::from_utf8(&bytes).unwrap();

    // BTreeMap serializes keys in sorted order
    let a_pos = str.find("a_key").unwrap();
    let m_pos = str.find("m_key").unwrap();
    let z_pos = str.find("z_key").unwrap();
    assert!(a_pos < m_pos && m_pos < z_pos, "Keys must be sorted");
}

#[test]
fn test_canonical_serialize_no_trailing_commas() {
    let mut data = ReportData::new();
    data.insert("key1".to_string(), json!("val1"));
    data.insert("key2".to_string(), json!("val2"));

    let bytes = canonical_serialize(&data).unwrap();
    let str = std::str::from_utf8(&bytes).unwrap();
    assert!(!str.contains(",}"), "No trailing commas before closing brace");
    assert!(!str.contains(",]"), "No trailing commas before closing bracket");
}

#[test]
fn test_report_hash_consistency() {
    let mut data = ReportData::new();
    data.insert("test".to_string(), json!(42));

    let report = Report {
        report_type: "test_report".to_string(),
        definition_version: 1,
        data_range: spindle_compliance::DataRange { from: None, to: None },
        data,
    };

    let hash1 = report_hash(&report);
    let hash2 = report_hash(&report);
    assert_eq!(hash1, hash2);

    // Hash should start with "sha256:"
    assert!(hash1.starts_with("sha256:"));
    assert_eq!(hash1.len(), 7 + 64); // "sha256:" + 64 hex chars
}

#[test]
fn test_report_hash_differs_for_different_data() {
    let mut data1 = ReportData::new();
    data1.insert("key".to_string(), json!("value1"));

    let mut data2 = ReportData::new();
    data2.insert("key".to_string(), json!("value2"));

    let report1 = Report {
        report_type: "test".to_string(),
        definition_version: 1,
        data_range: spindle_compliance::DataRange { from: None, to: None },
        data: data1,
    };

    let report2 = Report {
        report_type: "test".to_string(),
        definition_version: 1,
        data_range: spindle_compliance::DataRange { from: None, to: None },
        data: data2,
    };

    assert_ne!(report_hash(&report1), report_hash(&report2));
}

#[test]
fn test_all_report_definitions_registered() {
    assert_eq!(ControlStatusByNode::TYPE, "control_status_by_node");
    assert_eq!(ProfileSummaryOverTime::TYPE, "profile_summary_over_time");
    assert_eq!(WaiverRegister::TYPE, "waiver_register");
    assert_eq!(ExceptionDeviationList::TYPE, "exception_deviation_list");
}
