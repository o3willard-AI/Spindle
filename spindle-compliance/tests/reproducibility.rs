//! Tests for M4-12: Reproducibility from raw archive.
//!
//! Verify:
//! - Reprocess 24h window → byte-identical to original (SHA256 match)
//! - Different worker count → still byte-identical
//! - Pipeline parallelism doesn't affect output ordering

#![allow(warnings)]
use chrono::{TimeZone, Utc};
use uuid::Uuid;

use spindle_compliance::{
    report_hash, verify_all_reports_reproducible, verify_reproducibility,
    ControlStatusByNode, ExceptionDeviationList, MockReprocessor, MockReportStore,
    ProfileSummaryOverTime, ReproPipeline, ReproduceParams, ReportDefinition, ReportParams,
    WaiverRegister, Node, Profile, ControlResult, Waiver, canonical_serialize_report,
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
        make_cr(node_b.id, "ctrl-03", "passed", 4),
    ];

    let waivers = vec![make_waiver("ctrl-02", "project-a", "admin", 1)];

    MockReportStore::new()
        .with_nodes(vec![node_a, node_b])
        .with_control_results(results)
        .with_profiles(vec![profile])
        .with_waivers(waivers)
}

// ── Basic reproducibility tests ────────────────────────────────────────────────

#[tokio::test]
async fn test_repro_control_status_by_node_identical() {
    let store = standard_store();
    let reprocessor = MockReprocessor::new(store);
    let params = ReproduceParams {
        from: Utc.with_ymd_and_hms(2024, 6, 15, 0, 0, 0).unwrap(),
        to: Utc.with_ymd_and_hms(2024, 6, 16, 0, 0, 0).unwrap(),
        workers: 1,
        temp_schema: "spindle_repro_test".to_string(),
    };

    let result = verify_reproducibility(
        &reprocessor,
        &ControlStatusByNode,
        &params,
    )
    .await
    .unwrap();

    assert!(result.identical, "Reports must be identical: {} vs {}", result.original_hash, result.reprocessed_hash);
    assert_eq!(result.original_hash, result.reprocessed_hash);
    assert_eq!(result.report_type, "control_status_by_node");
}

#[tokio::test]
async fn test_repro_profile_summary_over_time_identical() {
    let store = standard_store();
    let reprocessor = MockReprocessor::new(store);
    let params = ReproduceParams {
        from: Utc.with_ymd_and_hms(2024, 6, 15, 0, 0, 0).unwrap(),
        to: Utc.with_ymd_and_hms(2024, 6, 16, 0, 0, 0).unwrap(),
        workers: 1,
        temp_schema: "spindle_repro_test".to_string(),
    };

    let result = verify_reproducibility(
        &reprocessor,
        &ProfileSummaryOverTime,
        &params,
    )
    .await
    .unwrap();

    assert!(result.identical);
    assert_eq!(result.report_type, "profile_summary_over_time");
}

#[tokio::test]
async fn test_repro_waiver_register_identical() {
    let store = standard_store();
    let reprocessor = MockReprocessor::new(store);
    let params = ReproduceParams {
        from: Utc.with_ymd_and_hms(2024, 6, 15, 0, 0, 0).unwrap(),
        to: Utc.with_ymd_and_hms(2024, 6, 16, 0, 0, 0).unwrap(),
        workers: 1,
        temp_schema: "spindle_repro_test".to_string(),
    };

    let result = verify_reproducibility(&reprocessor, &WaiverRegister, &params).await.unwrap();
    assert!(result.identical);
    assert_eq!(result.report_type, "waiver_register");
}

#[tokio::test]
async fn test_repro_exception_deviation_list_identical() {
    let store = standard_store();
    let reprocessor = MockReprocessor::new(store);
    let params = ReproduceParams {
        from: Utc.with_ymd_and_hms(2024, 6, 15, 0, 0, 0).unwrap(),
        to: Utc.with_ymd_and_hms(2024, 6, 16, 0, 0, 0).unwrap(),
        workers: 1,
        temp_schema: "spindle_repro_test".to_string(),
    };

    let result = verify_reproducibility(&reprocessor, &ExceptionDeviationList, &params).await.unwrap();
    assert!(result.identical);
    assert_eq!(result.report_type, "exception_deviation_list");
}

// ── Different worker count tests ─────────────────────────────────────────────

#[tokio::test]
async fn test_repro_different_worker_count_identical() {
    let store = standard_store();
    let reprocessor = MockReprocessor::new(store);
    let base_params = ReproduceParams {
        from: Utc.with_ymd_and_hms(2024, 6, 15, 0, 0, 0).unwrap(),
        to: Utc.with_ymd_and_hms(2024, 6, 16, 0, 0, 0).unwrap(),
        workers: 1,
        temp_schema: "spindle_repro_test".to_string(),
    };

    for workers in &[1, 2, 4, 8, 16] {
        let params = ReproduceParams {
            workers: *workers,
            ..base_params.clone()
        };

        for report_def in &[
            &ControlStatusByNode as &dyn ReportDefinition,
            &ProfileSummaryOverTime,
            &WaiverRegister,
            &ExceptionDeviationList,
        ] {
            let result = verify_reproducibility(&reprocessor, *report_def, &params).await.unwrap();
            assert!(
                result.identical,
                "Report {} must be identical with {} workers: {} vs {}",
                report_def.report_type(),
                workers,
                result.original_hash,
                result.reprocessed_hash
            );
        }
    }
}

#[tokio::test]
async fn test_repro_all_reports_different_workers() {
    let store = standard_store();
    let reprocessor = MockReprocessor::new(store);

    for workers in &[1, 4, 8] {
        let params = ReproduceParams {
            from: Utc.with_ymd_and_hms(2024, 6, 15, 0, 0, 0).unwrap(),
            to: Utc.with_ymd_and_hms(2024, 6, 16, 0, 0, 0).unwrap(),
            workers: *workers,
            temp_schema: format!("spindle_repro_{}", workers),
        };

        let results = verify_all_reports_reproducible(&reprocessor, &params).await.unwrap();
        assert_eq!(results.len(), 4);

        for result in &results {
            assert!(result.identical, "Report {} failed reproducibility with {} workers", result.report_type, workers);
        }
    }
}

// ── Byte-identical to original tests ─────────────────────────────────────────

#[tokio::test]
async fn test_repro_byte_identical_across_runs() {
    let store = standard_store();
    let reprocessor = MockReprocessor::new(store);
    let params = ReproduceParams {
        from: Utc.with_ymd_and_hms(2024, 6, 15, 0, 0, 0).unwrap(),
        to: Utc.with_ymd_and_hms(2024, 6, 16, 0, 0, 0).unwrap(),
        workers: 4,
        temp_schema: "spindle_repro_test".to_string(),
    };

    // Run 3 times with different worker counts, all must match
    let result1 = verify_reproducibility(&reprocessor, &ControlStatusByNode, &params).await.unwrap();

    let params2 = ReproduceParams { workers: 8, ..params.clone() };
    let result2 = verify_reproducibility(&reprocessor, &ControlStatusByNode, &params2).await.unwrap();

    let params3 = ReproduceParams { workers: 16, ..params.clone() };
    let result3 = verify_reproducibility(&reprocessor, &ControlStatusByNode, &params3).await.unwrap();

    assert_eq!(result1.original_hash, result2.original_hash);
    assert_eq!(result2.original_hash, result3.original_hash);
    assert_eq!(result1.reprocessed_hash, result2.reprocessed_hash);
    assert_eq!(result2.reprocessed_hash, result3.reprocessed_hash);
}

// ── Parallel pipeline ordering tests ─────────────────────────────────────────

#[tokio::test]
async fn test_repro_parallelism_doesnt_affect_ordering() {
    // Create data with many entries to ensure shuffling actually changes order
    let mut nodes = Vec::new();
    let mut results = Vec::new();

    for i in 0..10 {
        let id_str = format!("00000000-0000-0000-0000-{:012x}", 0xa + i);
        let node = make_node(&format!("node-{}", i), &id_str);
        for j in 0..5 {
            let ctrl_id = format!("ctrl-{:02x}", j);
            let status = if j % 2 == 0 { "passed" } else { "failed" };
            let cr_id = format!("00000000-0000-0000-0000-{:012x}", i * 10 + j);
            results.push(ControlResult {
                id: uuid_from(&cr_id),
                report_id: Uuid::nil(),
                run_id: Uuid::nil(),
                node_id: node.id,
                profile_id: uuid_from(PROFILE_ID),
                control_id: ctrl_id,
                status: status.to_string(),
                impact: 0.7,
                result: None,
                created_at: Utc.with_ymd_and_hms(2024, 6, 15, 10, i as u32, j as u32).unwrap(),
            });
        }
        nodes.push(node);
    }

    let profile = make_profile("ssh-baseline");
    let store = MockReportStore::new()
        .with_nodes(nodes)
        .with_control_results(results)
        .with_profiles(vec![profile]);

    let reprocessor = MockReprocessor::new(store);
    let params = ReproduceParams {
        from: Utc.with_ymd_and_hms(2024, 6, 15, 0, 0, 0).unwrap(),
        to: Utc.with_ymd_and_hms(2024, 6, 16, 0, 0, 0).unwrap(),
        workers: 8,
        temp_schema: "spindle_repro_parallel".to_string(),
    };

    let result = verify_reproducibility(&reprocessor, &ControlStatusByNode, &params).await.unwrap();
    assert!(result.identical, "Parallelism must not affect report output");

    // Also verify actual bytes match
    let original_store = reprocessor.process(&ReproduceParams { workers: 1, ..params.clone() }).await.unwrap();
    let repro_store = reprocessor.process(&params).await.unwrap();

    let report_params = ReportParams {
        from: Some(params.from),
        to: Some(params.to),
        node_filter: None,
        profile_filter: None,
    };

    let orig_report = ControlStatusByNode.generate(original_store.as_ref(), &report_params).await.unwrap();
    let repro_report = ControlStatusByNode.generate(repro_store.as_ref(), &report_params).await.unwrap();

    assert_eq!(
        canonical_serialize_report(&orig_report).unwrap(),
        canonical_serialize_report(&repro_report).unwrap(),
        "Reports must be byte-identical even with shuffled data"
    );
}

// ── Temp schema tests ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_repro_temp_schema_name() {
    let store = standard_store();
    let reprocessor = MockReprocessor::new(store);
    let params = ReproduceParams {
        from: Utc.with_ymd_and_hms(2024, 6, 15, 0, 0, 0).unwrap(),
        to: Utc.with_ymd_and_hms(2024, 6, 16, 0, 0, 0).unwrap(),
        workers: 4,
        temp_schema: "spindle_repro_2024_06_15".to_string(),
    };

    // Should not fail with various schema names
    let result = verify_reproducibility(&reprocessor, &ControlStatusByNode, &params).await.unwrap();
    assert!(result.identical);
}

#[tokio::test]
async fn test_verify_all_reports_reproducible() {
    let store = standard_store();
    let reprocessor = MockReprocessor::new(store);
    let params = ReproduceParams {
        from: Utc.with_ymd_and_hms(2024, 6, 15, 0, 0, 0).unwrap(),
        to: Utc.with_ymd_and_hms(2024, 6, 16, 0, 0, 0).unwrap(),
        workers: 4,
        temp_schema: "spindle_repro_test".to_string(),
    };

    let results = verify_all_reports_reproducible(&reprocessor, &params).await.unwrap();

    assert_eq!(results.len(), 4);
    for result in &results {
        assert!(result.identical, "Report {} must be reproducible", result.report_type);
    }

    // Verify all four report types are present
    let types: Vec<&str> = results.iter().map(|r| r.report_type.as_str()).collect();
    assert!(types.contains(&"control_status_by_node"));
    assert!(types.contains(&"profile_summary_over_time"));
    assert!(types.contains(&"waiver_register"));
    assert!(types.contains(&"exception_deviation_list"));
}

// ── Empty data reproducibility ───────────────────────────────────────────────

#[tokio::test]
async fn test_repro_empty_store_identical() {
    let store = MockReportStore::new();
    let reprocessor = MockReprocessor::new(store);
    let params = ReproduceParams {
        from: Utc.with_ymd_and_hms(2024, 6, 15, 0, 0, 0).unwrap(),
        to: Utc.with_ymd_and_hms(2024, 6, 16, 0, 0, 0).unwrap(),
        workers: 4,
        temp_schema: "spindle_repro_empty".to_string(),
    };

    let result = verify_reproducibility(&reprocessor, &ControlStatusByNode, &params).await.unwrap();
    assert!(result.identical, "Empty store reports must still be identical");
}
