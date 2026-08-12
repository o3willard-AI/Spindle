//! Minimal one-shot pipeline trigger (Task 5).
//!
//! Processes a single archived Chef run-converge payload end-to-end:
//!   1. read the raw payload from the archive (by key, e.g. `2026-08-09/<sha256>.json.gz`)
//!   2. parse + normalize + filter it via `spindle_pipeline::process_payload`
//!   3. build a `Node`, `Run`, and `ResourceEvent`s from the payload + parsed events
//!   4. write them to the store tables (`nodes`, `runs`, `resource_events`) via
//!      `spindle_store::SqlxNodeStore` / `SqlxRunStore` / `SqlxResourceEventStore`
//!   5. print the inserted node/run IDs and the run summary
//!
//! This is intentionally a *trigger* — the full background daemon (queue consumer +
//! rollups + reconciliation) is tracked separately. This proves the ingest→parse→store
//! chain works against the live DB.

use std::sync::Arc;
use chrono::{DateTime, Utc};
use serde_json::Value;

use spindle_rawarchive::Archive;
use spindle_store::Scope;
use spindle_store::{NodeStore, RunStore, ResourceEventStore};

/// Time parsing fallback helper.
fn parse_ts(v: Option<&Value>) -> Option<DateTime<Utc>> {
    let s = v?.as_str()?;
    DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.with_timezone(&Utc))
}

/// Build a `Node` from a run-converge payload.
fn build_node(payload: &Value, node_id: uuid::Uuid) -> spindle_store::Node {
    let node_obj = payload.get("node").cloned().unwrap_or(Value::Null);
    let name = payload
        .get("node_name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let platform_name = node_obj
        .pointer("/platform/name").unwrap_or(&Value::Null)
        .as_str().unwrap_or("unknown").to_string();
    let platform_version = node_obj
        .pointer("/platform/version").unwrap_or(&Value::Null)
        .as_str().unwrap_or("").to_string();
    let chef_env = node_obj
        .get("chef_environment").unwrap_or(&Value::Null)
        .as_str().unwrap_or("_default").to_string();
    let policy_group = node_obj
        .get("policy_group").unwrap_or(&Value::Null)
        .as_str().unwrap_or("").to_string();
    let policy_name = node_obj
        .get("policy_name").unwrap_or(&Value::Null)
        .as_str().unwrap_or("").to_string();
    let attributes = node_obj.get("attributes").cloned().unwrap_or(Value::Null);

    spindle_store::Node {
        id: node_id,
        name,
        platform: platform_name,
        platform_version,
        chef_environment: chef_env,
        policy_group,
        policy_name,
        attributes,
        last_seen: Utc::now(),
        created_at: Utc::now(),
    }
}

/// Build a `Run` from a run-converge payload + pipeline stats.
fn build_run(
    payload: &Value,
    run_row_id: uuid::Uuid,
    node_id: uuid::Uuid,
    stats: &spindle_pipeline::RunResourceStats,
) -> spindle_store::Run {
    let run_id = payload
        .get("run_id").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
    let status = payload
        .get("status").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
    let start_time = parse_ts(payload.get("start_time")).unwrap_or_else(Utc::now);
    let end_time = parse_ts(payload.get("end_time"));
    let error_summary = if status == "failure" || status == "failed" {
        payload.get("error").cloned()
    } else {
        None
    };

    spindle_store::Run {
        id: run_row_id,
        node_id,
        run_id,
        status,
        start_time,
        end_time,
        total_resource_count: stats.total_resource_count as i32,
        updated_count: stats.updated_count as i32,
        failed_count: stats.failed_count as i32,
        skipped_count: stats.skipped_count as i32,
        error_summary,
        cookbook_set: payload.get("cookbooks").cloned(),
        schema_version: spindle_pipeline::SCHEMA_VERSION,
        created_at: Utc::now(),
    }
}

/// Build `ResourceEvent`s from the payload's original `resources` array, coupled with
/// the pipeline's parsed (filtered) events so we preserve per-resource type/action/duration.
fn build_resource_events(
    payload: &Value,
    node_id: uuid::Uuid,
    run_row_id: uuid::Uuid,
    parsed: &[spindle_pipeline::ParsedResourceEvent],
) -> Vec<spindle_store::ResourceEvent> {
    // Map from the raw resources array (which has full type/action/duration/status) so we
    // can populate the store row faithfully. The pipeline already filtered out no-ops;
    // here we rebuild store `ResourceEvent`s for those that were marked persistable.
    let raw_resources = payload.get("resources").and_then(|r| r.as_array()).cloned().unwrap_or_default();

    parsed.iter().map(|ev| {
        // Find the matching raw resource (by name) to pull type/action/duration.
        let raw = raw_resources.iter().find(|r| r.get("name").and_then(|n| n.as_str()) == Some(ev.name.as_str()));
        let resource_type = raw.and_then(|r| r.get("type").and_then(|t| t.as_str())).unwrap_or("resource").to_string();
        let resource_name = ev.name.clone();
        let action_raw = raw.and_then(|r| r.get("action"))
            .and_then(|a| a.as_array())
            .and_then(|arr| arr.first())
            .and_then(|a| a.as_str())
            .unwrap_or("apply")
            .to_string();
        let duration_ms = raw.and_then(|r| r.get("duration").and_then(|d| d.as_f64()))
            .map(|d| (d * 1000.0) as i32)
            .unwrap_or(0);
        let cookbook_name = raw.and_then(|r| r.get("cookbook_name").and_then(|c| c.as_str())).unwrap_or("").to_string();
        let cookbook_version = raw.and_then(|r| r.get("cookbook_version").and_then(|c| c.as_str())).unwrap_or("").to_string();
        let delta = if status_is_changed(&ev.status) {
            raw.and_then(|r| r.get("delta").cloned())
        } else {
            None
        };

        spindle_store::ResourceEvent {
            id: uuid::Uuid::new_v4(),
            run_id: run_row_id,
            node_id,
            resource_type,
            resource_name,
            action: action_raw,
            status: ev.status.to_string(),
            duration_ms,
            cookbook_name,
            cookbook_version,
            guard_outcome: None,
            delta,
            schema_version: spindle_pipeline::SCHEMA_VERSION,
            created_at: Utc::now(),
        }
    }).collect()
}

fn status_is_changed(s: &spindle_pipeline::ResourceStatus) -> bool {
    matches!(s, spindle_pipeline::ResourceStatus::Updated | spindle_pipeline::ResourceStatus::Failed)
}

/// Process one archived payload key end-to-end.
pub async fn process_archive_key(
    pool: sqlx::PgPool,
    archive_root: &str,
    key: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let archive = Arc::new(spindle_rawarchive::LocalArchive::new(archive_root)?);
    let raw = archive.retrieve(key)?;

    // The archive stores gzipped JSON under a `.json.gz` key (content-addressed by SHA-256).
    // retrieve() decompresses automatically, yielding the original JSON bytes.
    let payload: Value = serde_json::from_slice(&raw)
        .map_err(|_| format!("payload is not valid JSON: {}", key))?;

    // Parse + normalize + filter via the pipeline.
    let result = spindle_pipeline::process_payload(&payload)
        .map_err(|e| format!("pipeline processing failed: {}", e))?;
    let stats = result.stats;

    // Build + insert Node (upsert by name), then Run, then ResourceEvents.
    let scope = Scope::all();
    let node_store = spindle_store::SqlxNodeStore::new(pool.clone());
    let run_store = spindle_store::SqlxRunStore::new(pool.clone());
    let event_store = spindle_store::SqlxResourceEventStore::new(pool.clone());

    let node_id = uuid::Uuid::new_v4();
    let node = build_node(&payload, node_id);
    let node_row = node_store.upsert_node(&node, &scope).await?;

    let run_row_id = uuid::Uuid::new_v4();
    let run = build_run(&payload, run_row_id, node_id, &stats);
    let run_row = run_store.insert_run(&run, &scope).await?;

    let events = build_resource_events(&payload, node_id, run_row_id, &result.persistable_events);
    let mut event_ids = Vec::new();
    for ev in &events {
        let id = event_store.insert_event(ev, &scope).await?;
        event_ids.push(id);
    }

    println!("=== one-shot pipeline trigger: processed archive key ===");
    println!("archive_key : {}", key);
    println!("node_name   : {}", node.name);
    println!("node_row    : {} (id {})", node_row, node_id);
    println!("run_row     : {}  run_id={} status={} total={} updated={} failed={} skipped={}",
        run_row, run.run_id, run.status,
        stats.total_resource_count, stats.updated_count, stats.failed_count, stats.skipped_count);
    println!("resource_events_persisted : {} (rows {})", events.len(), event_ids.len());
    for (i, eid) in event_ids.iter().enumerate() {
        println!("  [{}] {} id={} {} {} action={} status={} dur_ms={}",
            i, events[i].resource_type, eid, events[i].resource_name, events[i].cookbook_name,
            events[i].action, events[i].status, events[i].duration_ms);
    }
    Ok(())
}