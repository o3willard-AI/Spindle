//! Spindle pipeline processing — resource event filtering and run statistics.
//!
//! # Overview
//!
//! After a Chef Infra run-converge payload is parsed and normalized, the pipeline
//! iterates over resource events and applies **no-op filtering**:
//!
//! - **Status `up-to-date`** → increment `total_resource_count` only; skip
//!   `resource_events` insert (the resource was not changed).
//! - **Status `updated`, `failed`, or `skipped`** → insert into `resource_events`
//!   AND increment the corresponding status-specific counter.
//!
//! # Count reconciliation
//!
//! ```text
//! updated_count + failed_count + skipped_count + (total_resource_count - persisted_count)
//!   = total_resource_count
//! ```
//!
//! i.e. up-to-date resources are excluded from `resource_events` inserts but
//! still counted in the total.

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ── Resource status ──────────────────────────────────────────────────────

/// Status of a Chef Infra resource event.
///
/// Chef uses these strings in the `status` field of resource entries within
/// a `run-converge` data-collector payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceStatus {
    /// Resource was already in the desired state — no change made.
    /// Filtered: NOT inserted into resource_events, only counted in total.
    #[serde(rename = "up-to-date")]
    UpToDate,
    /// Resource was successfully updated.
    Updated,
    /// Resource update failed.
    Failed,
    /// Resource was intentionally skipped.
    Skipped,
}

impl std::fmt::Display for ResourceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResourceStatus::UpToDate => write!(f, "up-to-date"),
            ResourceStatus::Updated => write!(f, "updated"),
            ResourceStatus::Failed => write!(f, "failed"),
            ResourceStatus::Skipped => write!(f, "skipped"),
        }
    }
}

/// Attempt to parse a Chef status string into `ResourceStatus`.
pub fn parse_status(s: &str) -> Option<ResourceStatus> {
    match s {
        "up-to-date" | "up_to_date" => Some(ResourceStatus::UpToDate),
        "updated" => Some(ResourceStatus::Updated),
        "failed" => Some(ResourceStatus::Failed),
        "skipped" => Some(ResourceStatus::Skipped),
        _ => None,
    }
}

// ── Resource event ───────────────────────────────────────────────────────

/// A single resource event from a Chef run-converge payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceEvent {
    /// Name of the Chef resource (e.g. "package[nginx]").
    pub name: String,
    /// Status of the resource after convergence.
    #[serde(rename = "status")]
    pub status: String,
    /// Optional cookbook name.
    #[serde(default)]
    pub cookbook: Option<String>,
    /// Optional recipe name.
    #[serde(default)]
    pub recipe: Option<String>,
    /// Optional JSON-serialized resource properties delta.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<serde_json::Value>,
}

/// Parsed resource event with typed status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedResourceEvent {
    pub name: String,
    pub status: ResourceStatus,
    pub cookbook: Option<String>,
    pub recipe: Option<String>,
    pub properties: Option<serde_json::Value>,
}

impl ParsedResourceEvent {
    pub fn from_event(event: ResourceEvent) -> Option<Self> {
        let status = parse_status(&event.status)?;
        Some(Self {
            name: event.name,
            status,
            cookbook: event.cookbook,
            recipe: event.recipe,
            properties: event.properties,
        })
    }
}

// ── Run statistics ───────────────────────────────────────────────────────

/// Aggregated statistics for a Chef run after pipeline processing.
///
/// # Reconciliation invariant
///
/// ```text
/// updated_count + failed_count + skipped_count + up_to_date_count
///   = total_resource_count
/// ```
///
/// Where `up_to_date_count = total_resource_count - persisted_count`
/// (resources inserted into `resource_events`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunResourceStats {
    /// Total number of resources in the payload (all statuses).
    pub total_resource_count: u64,
    /// Resources with status `updated` — inserted into resource_events.
    pub updated_count: u64,
    /// Resources with status `failed` — inserted into resource_events.
    pub failed_count: u64,
    /// Resources with status `skipped` — inserted into resource_events.
    pub skipped_count: u64,
    /// Resources with status `up-to-date` — NOT inserted, only counted.
    pub up_to_date_count: u64,
    /// Number of rows actually persisted in resource_events.
    pub persisted_count: u64,
}

impl RunResourceStats {
    /// Verify the reconciliation invariant:
    /// updated + failed + skipped + up_to_date = total
    pub fn is_consistent(&self) -> bool {
        self.updated_count + self.failed_count + self.skipped_count + self.up_to_date_count
            == self.total_resource_count
    }

    /// Verify that persisted_count equals updated + failed + skipped
    /// (only non-up-to-date resources are persisted).
    pub fn is_persisted_consistent(&self) -> bool {
        self.persisted_count == self.updated_count + self.failed_count + self.skipped_count
    }

    /// Full reconciliation: both invariants hold.
    pub fn reconcile(&self) -> Result<(), PipelineError> {
        if !self.is_consistent() {
            return Err(PipelineError::ReconciliationFailed(format!(
                "count mismatch: {} + {} + {} + {} != {}",
                self.updated_count,
                self.failed_count,
                self.skipped_count,
                self.up_to_date_count,
                self.total_resource_count,
            )));
        }
        if !self.is_persisted_consistent() {
            return Err(PipelineError::ReconciliationFailed(format!(
                "persist mismatch: persisted={} != updated+failed+skipped={}",
                self.persisted_count,
                self.updated_count + self.failed_count + self.skipped_count,
            )));
        }
        Ok(())
    }
}

// ── Pipeline processing ──────────────────────────────────────────────────

/// Result of pipeline processing: filtered events to persist + run statistics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PipelineResult {
    /// Resource events that should be inserted into the `resource_events` table.
    /// Only `updated`, `failed`, and `skipped` resources appear here.
    pub persistable_events: Vec<ParsedResourceEvent>,
    /// Aggregated run statistics.
    pub stats: RunResourceStats,
}

/// Errors returned by the pipeline.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PipelineError {
    #[error("resource status is not recognized: {0}")]
    UnknownStatus(String),
    #[error("reconciliation failed: {0}")]
    ReconciliationFailed(String),
    #[error("no resources in payload")]
    EmptyResources,
}

/// Process a list of resource events from a normalized Chef run-converge payload.
///
/// # Behavior
///
/// - `up-to-date` → skipped from `persistable_events`, counted in `up_to_date_count`
/// - `updated` / `failed` / `skipped` → added to `persistable_events`, counted by status
/// - Unknown statuses → returned as `PipelineError::UnknownStatus`
///
/// # Reconciliation
///
/// After processing, `RunResourceStats::reconcile()` is called to verify
/// count invariants. If reconciliation fails, an error is returned.
pub fn process_resource_events(
    events: Vec<ResourceEvent>,
) -> Result<PipelineResult, PipelineError> {
    if events.is_empty() {
        return Err(PipelineError::EmptyResources);
    }

    let total = events.len() as u64;
    let mut persistable_events = Vec::new();
    let mut stats = RunResourceStats {
        total_resource_count: total,
        ..Default::default()
    };

    for event in events {
        let parsed = ParsedResourceEvent::from_event(event).ok_or_else(|| {
            PipelineError::UnknownStatus("unable to parse resource status".to_string())
        })?;

        match parsed.status {
            ResourceStatus::UpToDate => {
                stats.up_to_date_count += 1;
                // NOT persisted — no-op filtering
            }
            ResourceStatus::Updated => {
                stats.updated_count += 1;
                persistable_events.push(parsed);
            }
            ResourceStatus::Failed => {
                stats.failed_count += 1;
                persistable_events.push(parsed);
            }
            ResourceStatus::Skipped => {
                stats.skipped_count += 1;
                persistable_events.push(parsed);
            }
        }
    }

    stats.persisted_count = persistable_events.len() as u64;

    // Verify reconciliation
    stats.reconcile()?;

    Ok(PipelineResult {
        persistable_events,
        stats,
    })
}

/// Extract resource events from a normalized Chef run-converge JSON payload.
///
/// Expects the payload to contain a `resources` array, where each entry
/// has at least `name` and `status` fields.
pub fn extract_resource_events(payload: &serde_json::Value) -> Result<Vec<ResourceEvent>, PipelineError> {
    let resources = payload
        .get("resources")
        .and_then(|r| r.as_array())
        .ok_or(PipelineError::EmptyResources)?;

    let mut events = Vec::with_capacity(resources.len());
    for resource in resources {
        let event: ResourceEvent = serde_json::from_value(resource.clone())
            .map_err(|e| PipelineError::UnknownStatus(format!("resource parse error: {}", e)))?;
        events.push(event);
    }
    Ok(events)
}

/// Convenience: extract + process in one call.
pub fn process_payload(
    payload: &serde_json::Value,
) -> Result<PipelineResult, PipelineError> {
    let events = extract_resource_events(payload)?;
    process_resource_events(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a sample run-converge payload with configurable resource counts.
    fn make_converge_payload(
        up_to_date: usize,
        updated: usize,
        failed: usize,
        skipped: usize,
    ) -> serde_json::Value {
        let mut resources: Vec<serde_json::Value> = Vec::new();
        for i in 0..up_to_date {
            resources.push(serde_json::json!({
                "name": format!("up-to-date-resource-{}", i),
                "status": "up-to-date",
                "cookbook": "test-cookbook",
                "recipe": "test-recipe",
            }));
        }
        for i in 0..updated {
            resources.push(serde_json::json!({
                "name": format!("updated-resource-{}", i),
                "status": "updated",
                "cookbook": "test-cookbook",
                "recipe": "test-recipe",
                "properties": {"path": "/usr/bin/test"}
            }));
        }
        for i in 0..failed {
            resources.push(serde_json::json!({
                "name": format!("failed-resource-{}", i),
                "status": "failed",
                "cookbook": "test-cookbook",
            }));
        }
        for i in 0..skipped {
            resources.push(serde_json::json!({
                "name": format!("skipped-resource-{}", i),
                "status": "skipped",
                "cookbook": "test-cookbook",
            }));
        }

        serde_json::json!({
            "run_id": "run-abc-123",
            "node_name": "web-server-01",
            "resources": resources,
        })
    }

    #[test]
    fn test_status_parse_all_variants() {
        assert_eq!(parse_status("up-to-date"), Some(ResourceStatus::UpToDate));
        assert_eq!(parse_status("updated"), Some(ResourceStatus::Updated));
        assert_eq!(parse_status("failed"), Some(ResourceStatus::Failed));
        assert_eq!(parse_status("skipped"), Some(ResourceStatus::Skipped));
        assert_eq!(parse_status("unknown"), None);
    }

    #[test]
    fn test_status_display() {
        assert_eq!(ResourceStatus::UpToDate.to_string(), "up-to-date");
        assert_eq!(ResourceStatus::Updated.to_string(), "updated");
        assert_eq!(ResourceStatus::Failed.to_string(), "failed");
        assert_eq!(ResourceStatus::Skipped.to_string(), "skipped");
    }

    #[test]
    fn test_process_events_mixed_statuses() {
        // 95 up-to-date, 3 updated, 2 failed, 0 skipped = 100 total
        let payload = make_converge_payload(95, 3, 2, 0);
        let result = process_payload(&payload).unwrap();

        // Only 5 rows should be persistable (3 updated + 2 failed)
        assert_eq!(result.persistable_events.len(), 5);
        assert_eq!(result.stats.total_resource_count, 100);
        assert_eq!(result.stats.updated_count, 3);
        assert_eq!(result.stats.failed_count, 2);
        assert_eq!(result.stats.skipped_count, 0);
        assert_eq!(result.stats.up_to_date_count, 95);
        assert_eq!(result.stats.persisted_count, 5);
    }

    #[test]
    fn test_process_events_reconciliation_passes() {
        // The exact scenario from the requirements:
        // 100 events (95 up-to-date, 3 updated, 2 failed)
        let payload = make_converge_payload(95, 3, 2, 0);
        let result = process_payload(&payload).unwrap();
        assert!(result.stats.reconcile().is_ok());
        assert!(result.stats.is_consistent());
        assert!(result.stats.is_persisted_consistent());
    }

    #[test]
    fn test_process_events_all_up_to_date() {
        let payload = make_converge_payload(100, 0, 0, 0);
        let result = process_payload(&payload).unwrap();
        assert_eq!(result.persistable_events.len(), 0);
        assert_eq!(result.stats.total_resource_count, 100);
        assert_eq!(result.stats.up_to_date_count, 100);
        assert_eq!(result.stats.updated_count, 0);
        assert_eq!(result.stats.failed_count, 0);
        assert_eq!(result.stats.skipped_count, 0);
        assert_eq!(result.stats.persisted_count, 0);
        assert!(result.stats.reconcile().is_ok());
    }

    #[test]
    fn test_process_events_all_skipped() {
        let payload = make_converge_payload(0, 0, 0, 100);
        let result = process_payload(&payload).unwrap();
        assert_eq!(result.persistable_events.len(), 100);
        assert_eq!(result.stats.skipped_count, 100);
        assert_eq!(result.stats.up_to_date_count, 0);
        assert_eq!(result.stats.persisted_count, 100);
        assert!(result.stats.reconcile().is_ok());
    }

    #[test]
    fn test_reconciliation_invariant() {
        let payload = make_converge_payload(50, 20, 15, 15);
        let result = process_payload(&payload).unwrap();
        assert!(result.stats.is_consistent());
        assert!(result.stats.is_persisted_consistent());
        // Verify: updated + failed + skipped + up_to_date = total
        let sum = result.stats.updated_count
            + result.stats.failed_count
            + result.stats.skipped_count
            + result.stats.up_to_date_count;
        assert_eq!(sum, result.stats.total_resource_count);
    }

    #[test]
    fn test_count_reconciliation_formula() {
        // From the spec: updated + failed + skipped + (total - persisted) = total
        let payload = make_converge_payload(70, 15, 10, 5);
        let result = process_payload(&payload).unwrap();
        let s = &result.stats;
        let computed = s.updated_count + s.failed_count + s.skipped_count
            + (s.total_resource_count - s.persisted_count);
        assert_eq!(computed, s.total_resource_count);
    }

    #[test]
    fn test_process_events_empty_resources() {
        let payload = serde_json::json!({
            "run_id": "run-abc",
            "node_name": "node-1",
            "resources": []
        });
        let result = process_payload(&payload);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), PipelineError::EmptyResources);
    }

    #[test]
    fn test_process_events_missing_resources_key() {
        let payload = serde_json::json!({
            "run_id": "run-abc",
            "node_name": "node-1"
        });
        let result = process_payload(&payload);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), PipelineError::EmptyResources);
    }

    #[test]
    fn test_process_events_unknown_status() {
        let payload = serde_json::json!({
            "run_id": "run-abc",
            "node_name": "node-1",
            "resources": [
                {"name": "test-resource", "status": "borked"}
            ]
        });
        let result = process_payload(&payload);
        assert!(result.is_err());
        match result.unwrap_err() {
            PipelineError::UnknownStatus(s) => assert!(s.contains("borked") || s.contains("parse")),
            other => panic!("expected UnknownStatus, got {:?}", other),
        }
    }

    #[test]
    fn test_extract_resource_events() {
        let payload = make_converge_payload(5, 3, 2, 1);
        let events = extract_resource_events(&payload).unwrap();
        assert_eq!(events.len(), 11);
    }

    #[test]
    fn test_process_events_persistable_only_non_up_to_date() {
        let payload = make_converge_payload(10, 5, 3, 2);
        let result = process_payload(&payload).unwrap();
        // Only updated + failed + skipped should be in persistable_events
        assert_eq!(result.persistable_events.len(), 10);
        assert_eq!(result.stats.up_to_date_count, 10);
        // None of the persistable events should be up-to-date
        assert!(result.persistable_events.iter().all(|e| e.status != ResourceStatus::UpToDate));
    }

    #[test]
    fn test_run_stats_default() {
        let stats = RunResourceStats::default();
        assert_eq!(stats.total_resource_count, 0);
        assert_eq!(stats.updated_count, 0);
        assert!(stats.reconcile().is_ok());
    }

    #[test]
    fn test_run_stats_reconcile_fails_on_mismatch() {
        let stats = RunResourceStats {
            total_resource_count: 100,
            updated_count: 3,
            failed_count: 2,
            skipped_count: 0,
            up_to_date_count: 94, // should be 95
            persisted_count: 5,
        };
        assert!(stats.reconcile().is_err());
    }

    #[test]
    fn test_run_stats_reconcile_fails_on_persist_mismatch() {
        let stats = RunResourceStats {
            total_resource_count: 100,
            updated_count: 3,
            failed_count: 2,
            skipped_count: 0,
            up_to_date_count: 95,
            persisted_count: 4, // should be 5
        };
        assert!(stats.reconcile().is_err());
    }

    #[test]
    fn test_parsed_resource_event_from_event() {
        let event = ResourceEvent {
            name: "package[nginx]".to_string(),
            status: "updated".to_string(),
            cookbook: Some("nginx".to_string()),
            recipe: Some("default".to_string()),
            properties: Some(serde_json::json!({"path": "/usr/sbin/nginx"})),
        };
        let parsed = ParsedResourceEvent::from_event(event).unwrap();
        assert_eq!(parsed.status, ResourceStatus::Updated);
        assert_eq!(parsed.name, "package[nginx]");
        assert_eq!(parsed.cookbook, Some("nginx".to_string()));
    }

    #[test]
    fn test_parsed_resource_event_from_event_up_to_date() {
        let event = ResourceEvent {
            name: "package[openssl]".to_string(),
            status: "up-to-date".to_string(),
            cookbook: None,
            recipe: None,
            properties: None,
        };
        let parsed = ParsedResourceEvent::from_event(event).unwrap();
        assert_eq!(parsed.status, ResourceStatus::UpToDate);
    }

    #[test]
    fn test_parsed_resource_event_from_event_unknown_status() {
        let event = ResourceEvent {
            name: "test".to_string(),
            status: "weird".to_string(),
            cookbook: None,
            recipe: None,
            properties: None,
        };
        assert!(ParsedResourceEvent::from_event(event).is_none());
    }
}

// ── Duration rollups (M1-22) ──────────────────────────────────────────────

/// Streaming quantile estimator: cluster-based percentile approximation.
/// Maintains up to `max_raw` unsorted values, then compresses into
/// (mean, count) clusters for memory-efficient p50/p95/p99 queries.
pub struct StreamingQuantile {
    raw: Vec<f64>,
    max_raw: usize,
    clusters: Vec<(f64, usize)>,
}

impl StreamingQuantile {
    pub fn new(max_raw: usize) -> Self {
        Self {
            raw: Vec::with_capacity(max_raw),
            max_raw,
            clusters: Vec::new(),
        }
    }

    pub fn add(&mut self, value: f64) {
        if self.raw.len() < self.max_raw {
            self.raw.push(value);
            if self.raw.len() == self.max_raw {
                self.compress();
            }
        } else {
            self.insert_into_clusters(value);
        }
    }

    fn compress(&mut self) {
        self.raw.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = self.raw.len();
        let cluster_size = (n as f64 / 100.0).ceil() as usize;
        let mut clusters = Vec::new();
        let mut i = 0;
        while i < n {
            let end = (i + cluster_size).min(n);
            let chunk = &self.raw[i..end];
            let mean = chunk.iter().sum::<f64>() / chunk.len() as f64;
            clusters.push((mean, end - i));
            i = end;
        }
        self.clusters = clusters;
    }

    fn insert_into_clusters(&mut self, value: f64) {
        let mut best_idx = 0;
        let mut best_dist = f64::INFINITY;
        for (idx, (mean, _)) in self.clusters.iter().enumerate() {
            let dist = (value - mean).abs();
            if dist < best_dist {
                best_dist = dist;
                best_idx = idx;
            }
        }
        self.clusters[best_idx].1 += 1;
        let (mean, count) = self.clusters[best_idx];
        self.clusters[best_idx].0 = mean + (value - mean) / (count + 1) as f64;
    }

    pub fn quantile(&self, q: f64) -> f64 {
        if self.raw.is_empty() && self.clusters.is_empty() {
            return 0.0;
        }
        let total: usize = if self.raw.is_empty() {
            self.clusters.iter().map(|(_, c)| c).sum()
        } else {
            self.raw.len()
        };
        let target = (q * total as f64) as usize;

        if !self.raw.is_empty() && self.clusters.is_empty() {
            let mut sorted = self.raw.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let idx = target.min(sorted.len() - 1);
            sorted[idx]
        } else {
            let mut acc = 0usize;
            for (mean, count) in &self.clusters {
                if acc + *count > target {
                    return *mean;
                }
                acc += *count;
            }
            self.clusters.last().map(|(m, _)| *m).unwrap_or(0.0)
        }
    }

    pub fn count(&self) -> usize {
        // After compression, clusters hold the true count via summing
        if self.clusters.is_empty() {
            self.raw.len()
        } else {
            self.clusters.iter().map(|(_, c)| c).sum()
        }
    }
}

/// Key for duration rollup aggregation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RollupKey {
    pub hour: i64,
    pub cookbook_name: String,
    pub cookbook_version: Option<String>,
    pub resource_type: String,
    pub platform: Option<String>,
}

/// Streaming duration rollup accumulator.
pub struct DurationRollup {
    pub key: RollupKey,
    pub count: u64,
    pub total_ms: f64,
    pub max_ms: f64,
    estimator: StreamingQuantile,
}

impl DurationRollup {
    pub fn new(key: RollupKey) -> Self {
        Self {
            key,
            count: 0,
            total_ms: 0.0,
            max_ms: 0.0,
            estimator: StreamingQuantile::new(500),
        }
    }

    pub fn add(&mut self, duration_ms: f64) {
        self.count += 1;
        self.total_ms += duration_ms;
        if duration_ms > self.max_ms {
            self.max_ms = duration_ms;
        }
        self.estimator.add(duration_ms);
    }

    pub fn p50(&self) -> f64 {
        self.estimator.quantile(0.5)
    }

    pub fn p95(&self) -> f64 {
        self.estimator.quantile(0.95)
    }

    pub fn p99(&self) -> f64 {
        self.estimator.quantile(0.99)
    }

    pub fn flush(mut self) -> DurationRollupResult {
        let key = self.key.clone();
        let count = self.count;
        let total_ms = self.total_ms;
        let max_ms = self.max_ms;
        let p50 = self.p50();
        let p95 = self.p95();
        let p99 = self.p99();
        DurationRollupResult {
            key, count, total_ms: total_ms as i64,
            p50_ms: p50 as i64, p95_ms: p95 as i64,
            p99_ms: p99 as i64, max_ms: max_ms as i64,
        }
    }
}

/// Completed duration rollup ready for DB insert.
#[derive(Debug, Clone)]
pub struct DurationRollupResult {
    pub key: RollupKey,
    pub count: u64,
    pub total_ms: i64,
    pub p50_ms: i64,
    pub p95_ms: i64,
    pub p99_ms: i64,
    pub max_ms: i64,
}

/// Accumulates DurationRollups keyed by RollupKey, with periodic flush.
pub struct DurationRollupAccumulator {
    rollups: std::collections::HashMap<RollupKey, DurationRollup>,
    flush_interval_ms: u64,
    last_flush: i64,
}

impl DurationRollupAccumulator {
    pub fn new(flush_interval_ms: u64) -> Self {
        use chrono::Utc;
        Self {
            rollups: std::collections::HashMap::new(),
            flush_interval_ms,
            last_flush: Utc::now().timestamp_millis(),
        }
    }

    pub fn add(&mut self, duration_ms: f64, key: RollupKey) {
        self.rollups
            .entry(key.clone())
            .or_insert_with(|| DurationRollup::new(key))
            .add(duration_ms);
    }

    pub fn flush(&mut self) -> Vec<DurationRollupResult> {
        use chrono::Utc;
        self.last_flush = Utc::now().timestamp_millis();
        self.rollups
            .drain()
            .filter_map(|(_key, rollup)| {
                if rollup.count > 0 {
                    Some(rollup.flush())
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn should_flush(&self) -> bool {
        use chrono::Utc;
        let now = Utc::now().timestamp_millis();
        (now - self.last_flush) as u64 >= self.flush_interval_ms
    }

    pub fn total_count(&self) -> u64 {
        self.rollups.values().map(|r| r.count).sum()
    }
}

// ── Duration rollup tests ─────────────────────────────────────────────────

#[cfg(test)]
mod rollup_tests {
    use super::*;

    fn sample_key() -> RollupKey {
        RollupKey {
            hour: 1705312800,
            cookbook_name: "apache".to_string(),
            cookbook_version: Some("1.0".to_string()),
            resource_type: "service".to_string(),
            platform: Some("debian".to_string()),
        }
    }

    #[test]
    fn test_quantile_small_set() {
        let mut sq = StreamingQuantile::new(500);
        for i in 1..=100 {
            sq.add(i as f64);
        }
        assert_eq!(sq.count(), 100);
        let p50 = sq.quantile(0.5);
        assert!(p50 >= 40.0 && p50 <= 60.0, "p50={}", p50);
        let p95 = sq.quantile(0.95);
        assert!(p95 >= 85.0 && p95 <= 105.0, "p95={}", p95);
    }

    #[test]
    fn test_quantile_all_same() {
        let mut sq = StreamingQuantile::new(10);
        for _ in 0..200 {
            sq.add(42.0);
        }
        assert_eq!(sq.count(), 200);
        assert!((sq.quantile(0.5) - 42.0).abs() < 1.0, "p50={}", sq.quantile(0.5));
        assert!((sq.quantile(0.99) - 42.0).abs() < 1.0, "p99={}", sq.quantile(0.99));
    }

    #[test]
    fn test_rollup_basic() {
        let key = sample_key();
        let mut rollup = DurationRollup::new(key);
        rollup.add(100.0);
        rollup.add(200.0);
        rollup.add(300.0);
        rollup.add(400.0);
        rollup.add(500.0);

        let result = rollup.flush();
        assert_eq!(result.count, 5);
        assert_eq!(result.total_ms, 1500);
        assert_eq!(result.max_ms, 500);
        assert_eq!(result.p50_ms, 300);
    }

    #[test]
    fn test_rollup_counts_include_filtered() {
        let key = sample_key();
        let mut rollup = DurationRollup::new(key);
        // 3 updated + 10 up-to-date (filtered from resource_events but still counted)
        rollup.add(15000.0);
        rollup.add(500.0);
        rollup.add(100.0);
        for _ in 0..10 {
            rollup.add(0.0);
        }
        let result = rollup.flush();
        assert_eq!(result.count, 13);
    }

    #[test]
    fn test_rollup_p95_within_tolerance() {
        let key = sample_key();
        let mut rollup = DurationRollup::new(key);
        for i in 1..=50 {
            rollup.add((i * 100) as f64);
        }
        let result = rollup.flush();
        assert_eq!(result.count, 50);
        assert_eq!(result.max_ms, 5000);
        let diff = (result.p95_ms as f64 - 4750.0).abs();
        assert!(diff < 100.0, "p95={}: expected ~4750, diff={}", result.p95_ms, diff);
    }

    #[test]
    fn test_accumulator_basic() {
        let mut acc = DurationRollupAccumulator::new(900_000);
        let key1 = sample_key();
        acc.add(100.0, key1.clone());
        acc.add(200.0, key1);

        let key2 = RollupKey {
            hour: 1705312800,
            cookbook_name: "php".to_string(),
            cookbook_version: Some("2.0".to_string()),
            resource_type: "file".to_string(),
            platform: Some("debian".to_string()),
        };
        acc.add(50.0, key2);

        let flushed = acc.flush();
        assert_eq!(flushed.len(), 2);
        let total: u64 = flushed.iter().map(|r| r.count).sum();
        assert_eq!(total, 3);
    }

    #[test]
    fn test_accumulator_total_matches_input() {
        let mut acc = DurationRollupAccumulator::new(900_000);
        let key = sample_key();
        for i in 1..=100 {
            acc.add(i as f64, key.clone());
        }
        assert_eq!(acc.total_count(), 100);
        let flushed = acc.flush();
        let total: u64 = flushed.iter().map(|r| r.count).sum();
        assert_eq!(total, 100);
    }
}
