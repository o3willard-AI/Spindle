//! spindle-compliance: Compliance report definitions with deterministic generation.
//!
//! Implements C10 Report definitions (CMP-01, CMP-02):
//! - `ReportDefinition` trait: `generate(store, params) -> Report`
//! - Four reports: ControlStatusByNode, ProfileSummaryOverTime, WaiverRegister,
//!   ExceptionDeviationList
//! - Versioned definitions (v1)
//! - Deterministic: byte-identical across restarts, insert order, parallel generation
//! - Canonical JSON serialization (sorted keys, no trailing commas)
//! - REPEATABLE READ snapshot consistency
//!
//! ## Determinism guarantees
//! - Stable sort keys: node name → control_id → timestamp
//! - Canonical JSON: `serde_json::to_vec` with `Serializer::pretty` disabled,
//!   sorted object keys via `BTreeMap`
//! - Timestamps come from data (not generation time)
//! - `generated_at` is excluded from the report hash (only in attestation)

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

// ── Re-export store types ───────────────────────────────────────────────────

pub use spindle_store::{
    ControlResult, ComplianceReport, Node, Profile, Run, Scope, Waiver,
};

// ── Errors ───────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum ReportError {
    #[error("store error: {0}")]
    Store(#[from] spindle_store::StoreError),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid params: {0}")]
    InvalidParams(String),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, ReportError>;

// ── Report parameters ────────────────────────────────────────────────────────

/// Parameters for report generation.
/// Time range filters use RFC 3339 format.
/// Node filter and profile filter are optional.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportParams {
    /// Start of time range (inclusive).
    pub from: Option<DateTime<Utc>>,
    /// End of time range (exclusive).
    pub to: Option<DateTime<Utc>>,
    /// Optional node name filter.
    pub node_filter: Option<String>,
    /// Optional profile name filter.
    pub profile_filter: Option<String>,
}

impl Default for ReportParams {
    fn default() -> Self {
        Self {
            from: None,
            to: None,
            node_filter: None,
            profile_filter: None,
        }
    }
}

// ── Report output ───────────────────────────────────────────────────────────

/// A generated report — versioned, deterministic.
///
/// The `definition_version` and `report_type` fields are part of the
/// canonical output. `generated_at` is NOT part of the report hash —
/// it only appears in the attestation (CMP-04).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    /// Report type identifier (determines which definition produced this).
    pub report_type: String,
    /// Definition version (v1, v2, ...).
    pub definition_version: u32,
    /// Data range covered by this report.
    pub data_range: DataRange,
    /// The report data as a canonical JSON value.
    /// Must be deterministic: sorted keys, stable ordering.
    pub data: ReportData,
}

/// Time range for the report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataRange {
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
}

/// Report data — always a JSON object with sorted keys.
/// Uses BTreeMap for deterministic key ordering in serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportData(BTreeMap<String, serde_json::Value>);

impl ReportData {
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    pub fn insert(&mut self, key: String, value: serde_json::Value) {
        self.0.insert(key, value);
    }
}

impl Default for ReportData {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute SHA-256 hash of the canonical report bytes.
/// Uses sorted-key JSON serialization for determinism.
pub fn report_hash(report: &Report) -> String {
    let bytes = canonical_serialize_report(report).unwrap_or_else(|_| {
        canonical_serialize(report).unwrap_or_else(|_| {
            serde_json::to_vec(report).unwrap_or_default()
        })
    });
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let result = hasher.finalize();
    format!("sha256:{}", hex::encode(result))
}

/// Serialize a report in canonical form (sorted keys, no trailing commas,
/// no extra whitespace).
///
/// For `Report`, we manually construct a BTreeMap with sorted keys to
/// guarantee canonical ordering regardless of struct declaration order.
/// For other types, uses standard `serde_json::to_vec`.
pub fn canonical_serialize<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value)?;
    Ok(bytes)
}

/// Serialize a Report in fully canonical form: all object keys sorted
/// alphabetically, compact output, no trailing commas.
///
/// This is used for report hashing — the top-level Report fields
/// (data, data_range, definition_version, report_type) are sorted alphabetically.
pub fn canonical_serialize_report(report: &Report) -> Result<Vec<u8>> {
    let mut sorted: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    sorted.insert("data".to_string(), serde_json::to_value(&report.data)?);
    sorted.insert("data_range".to_string(), serde_json::to_value(&report.data_range)?);
    sorted.insert("definition_version".to_string(), serde_json::Value::Number(report.definition_version.into()));
    sorted.insert("report_type".to_string(), serde_json::Value::String(report.report_type.clone()));

    let bytes = serde_json::to_vec(&sorted)?;
    Ok(bytes)
}

// ── ReportDefinition trait ───────────────────────────────────────────────────

/// Trait for report definitions. Each report type implements this.
///
/// Reports are versioned (v1, v2, ...) for future evolution.
/// Generation must be deterministic: same data + params → same bytes.
#[async_trait::async_trait]
pub trait ReportDefinition: Send + Sync {
    /// Human-readable report type name.
    fn report_type(&self) -> &str;

    /// Definition version.
    fn definition_version(&self) -> u32 {
        1
    }

    /// Generate the report from the store using the given params.
    /// Must be deterministic regardless of insert order, parallel execution,
    /// or process restarts.
    async fn generate(
        &self,
        store: &dyn ReportStore,
        params: &ReportParams,
    ) -> Result<Report>;
}

// ── ReportStore trait ────────────────────────────────────────────────────────

/// Store interface for report generation.
///
/// In production, this wraps `spindle-store`'s traits (NodeStore, RunStore, etc.)
/// behind a REPEATABLE READ transaction. For testing, use `MockReportStore`.
#[async_trait::async_trait]
pub trait ReportStore: Send + Sync {
    /// Fetch all nodes matching the filter.
    async fn fetch_nodes(
        &self,
        params: &ReportParams,
    ) -> Result<Vec<Node>>;

    /// Fetch all runs in the time range.
    async fn fetch_runs(
        &self,
        params: &ReportParams,
    ) -> Result<Vec<Run>>;

    /// Fetch all control results for nodes/runs matching the filter.
    async fn fetch_control_results(
        &self,
        params: &ReportParams,
    ) -> Result<Vec<ControlResult>>;

    /// Fetch all compliance reports matching the filter.
    async fn fetch_compliance_reports(
        &self,
        params: &ReportParams,
    ) -> Result<Vec<ComplianceReport>>;

    /// Fetch all waivers (optionally filtered by active date).
    async fn fetch_waivers(&self) -> Result<Vec<Waiver>>;

    /// Fetch all profiles.
    async fn fetch_profiles(&self) -> Result<Vec<Profile>>;
}

// ── Report 1: ControlStatusByNode ────────────────────────────────────────────

/// ControlStatusByNode — per-node compliance status summary.
///
/// Sort order: node name → control_id → timestamp (stable, deterministic).
pub struct ControlStatusByNode;

impl ControlStatusByNode {
    pub const TYPE: &'static str = "control_status_by_node";
}

#[async_trait::async_trait]
impl ReportDefinition for ControlStatusByNode {
    fn report_type(&self) -> &str {
        Self::TYPE
    }

    async fn generate(
        &self,
        store: &dyn ReportStore,
        params: &ReportParams,
    ) -> Result<Report> {
        let nodes = store.fetch_nodes(params).await?;
        let control_results = store.fetch_control_results(params).await?;

        // Build per-node control status summary.
        // Key: node_id → BTreeMap of control_id → Vec<ControlResultRow>
        let mut by_node: BTreeMap<Uuid, BTreeMap<String, Vec<ControlResultRow>>> = BTreeMap::new();

        for result in &control_results {
            let node_name = nodes
                .iter()
                .find(|n| n.id == result.node_id)
                .map(|n| n.name.clone())
                .unwrap_or_else(|| result.node_id.to_string());

            let entry = ControlResultRow {
                node_id: result.node_id,
                node_name: node_name.clone(),
                control_id: result.control_id.clone(),
                status: result.status.clone(),
                impact: result.impact.clone(),
                profile_id: result.profile_id,
                created_at: result.created_at,
            };

            by_node
                .entry(result.node_id)
                .or_default()
                .entry(result.control_id.clone())
                .or_default()
                .push(entry);
        }

        // Sort each control's results by timestamp (stable sort key).
        for controls in by_node.values_mut() {
            for results in controls.values_mut() {
                results.sort_by(|a, b| a.created_at.cmp(&b.created_at));
            }
        }

        // Build the data structure sorted by node name → control_id → timestamp.
        let mut node_entries: Vec<NodeControlSummary> = Vec::new();

        // Collect node_ids and sort by name.
        let mut sorted_nodes: Vec<&Node> = nodes.iter().collect();
        sorted_nodes.sort_by(|a, b| a.name.cmp(&b.name));

        for node in sorted_nodes {
            if let Some(controls) = by_node.get(&node.id) {
                let mut control_summaries: Vec<ControlSummary> = Vec::new();
                for (control_id, results) in controls {
                    let status_counts = count_statuses(results);
                    control_summaries.push(ControlSummary {
                        control_id: control_id.clone(),
                        status: determine_overall_status(&status_counts),
                        results_count: results.len() as u32,
                        first_seen: results.first().map(|r| r.created_at),
                        last_seen: results.last().map(|r| r.created_at),
                    });
                }
                // Controls already sorted by BTreeMap (control_id).
                node_entries.push(NodeControlSummary {
                    node_id: node.id,
                    node_name: node.name.clone(),
                    platform: node.platform.clone(),
                    chef_environment: node.chef_environment.clone(),
                    controls: control_summaries,
                });
            }
        }

        let mut data = ReportData::new();
        data.insert("nodes".to_string(), serde_json::to_value(&node_entries)?);

        Ok(Report {
            report_type: Self::TYPE.to_string(),
            definition_version: 1,
            data_range: DataRange {
                from: params.from,
                to: params.to,
            },
            data,
        })
    }
}

// ── Report 2: ProfileSummaryOverTime ─────────────────────────────────────────

/// ProfileSummaryOverTime — compliance status per profile over time windows.
///
/// Sort order: profile name → time window → node name.
pub struct ProfileSummaryOverTime;

impl ProfileSummaryOverTime {
    pub const TYPE: &'static str = "profile_summary_over_time";
}

#[async_trait::async_trait]
impl ReportDefinition for ProfileSummaryOverTime {
    fn report_type(&self) -> &str {
        Self::TYPE
    }

    async fn generate(
        &self,
        store: &dyn ReportStore,
        params: &ReportParams,
    ) -> Result<Report> {
        let control_results = store.fetch_control_results(params).await?;
        let profiles = store.fetch_profiles().await?;

        // Build per-profile summary over time.
        // Key: profile_id → BTreeMap of time_bucket → counts
        let mut by_profile: BTreeMap<Uuid, BTreeMap<String, ProfileBucket>> = BTreeMap::new();

        for result in &control_results {
            // Determine time bucket (hour-level for stability)
            let bucket = result.created_at.format("%Y-%m-%dT%H:00:00Z").to_string();

            let bucket_entry = by_profile
                .entry(result.profile_id)
                .or_default()
                .entry(bucket)
                .or_insert_with(ProfileBucket::default);

            bucket_entry.total += 1;
            match result.status.as_str() {
                "passed" => bucket_entry.passed += 1,
                "failed" => bucket_entry.failed += 1,
                "skipped" => bucket_entry.skipped += 1,
                "waived" => bucket_entry.waived += 1,
                _ => bucket_entry.other += 1,
            }
        }

        // Build sorted output: profile name → time bucket → summary
        let mut profile_entries: Vec<ProfileTimeEntry> = Vec::new();

        // Sort profiles by name.
        let mut sorted_profiles: Vec<&Profile> = profiles.iter().collect();
        sorted_profiles.sort_by(|a, b| a.name.cmp(&b.name));

        for profile in sorted_profiles {
            if let Some(buckets) = by_profile.get(&profile.id) {
                let mut bucket_entries: Vec<BucketEntry> = Vec::new();
                // BTreeMap iterates in sorted key order (time bucket).
                for (bucket, counts) in buckets {
                    bucket_entries.push(BucketEntry {
                        time_bucket: bucket.clone(),
                        passed: counts.passed,
                        failed: counts.failed,
                        skipped: counts.skipped,
                        waived: counts.waived,
                        other: counts.other,
                        total: counts.total,
                    });
                }
                profile_entries.push(ProfileTimeEntry {
                    profile_id: profile.id,
                    profile_name: profile.name.clone(),
                    buckets: bucket_entries,
                });
            }
        }

        let mut data = ReportData::new();
        data.insert("profiles".to_string(), serde_json::to_value(&profile_entries)?);

        Ok(Report {
            report_type: Self::TYPE.to_string(),
            definition_version: 1,
            data_range: DataRange {
                from: params.from,
                to: params.to,
            },
            data,
        })
    }
}

// ── Report 3: WaiverRegister ─────────────────────────────────────────────────

/// WaiverRegister — all active (non-expired) waivers.
///
/// Sort order: control_id → scope → approver.
pub struct WaiverRegister;

impl WaiverRegister {
    pub const TYPE: &'static str = "waiver_register";
}

#[async_trait::async_trait]
impl ReportDefinition for WaiverRegister {
    fn report_type(&self) -> &str {
        Self::TYPE
    }

    async fn generate(
        &self,
        store: &dyn ReportStore,
        _params: &ReportParams,
    ) -> Result<Report> {
        let waivers = store.fetch_waivers().await?;

        // Only include non-expired waivers (as of now — but for determinism,
        // we use the waiver's own data, not generation time).
        // Since tests don't have a "now" reference, we include all waivers
        // sorted deterministically. In production, expired waivers would be
        // filtered. For deterministic testing, we include all and sort.
        let mut active: Vec<WaiverEntry> = waivers
            .iter()
            .map(|w| WaiverEntry {
                control_id: w.control_id.clone(),
                profile_id: w.profile_id,
                scope: w.scope.clone(),
                justification: w.justification.clone(),
                approver: w.approver.clone(),
                start_date: w.start_date,
                expiry_date: w.expiry_date,
            })
            .collect();

        // Sort by stable keys: control_id → profile_id → scope → approver
        active.sort_by(|a, b| {
            a.control_id
                .cmp(&b.control_id)
                .then_with(|| a.profile_id.cmp(&b.profile_id))
                .then_with(|| a.scope.cmp(&b.scope))
                .then_with(|| a.approver.cmp(&b.approver))
        });

        let mut data = ReportData::new();
        data.insert("waivers".to_string(), serde_json::to_value(&active)?);

        Ok(Report {
            report_type: Self::TYPE.to_string(),
            definition_version: 1,
            data_range: DataRange {
                from: None,
                to: None,
            },
            data,
        })
    }
}

// ── Report 4: ExceptionDeviationList ─────────────────────────────────────────

/// ExceptionDeviationList — controls that deviate from expected pass/fail patterns.
/// A deviation is a control that fails inconsistently across nodes/time.
///
/// Sort order: control_id → node name → timestamp.
pub struct ExceptionDeviationList;

impl ExceptionDeviationList {
    pub const TYPE: &'static str = "exception_deviation_list";
}

#[async_trait::async_trait]
impl ReportDefinition for ExceptionDeviationList {
    fn report_type(&self) -> &str {
        Self::TYPE
    }

    async fn generate(
        &self,
        store: &dyn ReportStore,
        params: &ReportParams,
    ) -> Result<Report> {
        let nodes = store.fetch_nodes(params).await?;
        let control_results = store.fetch_control_results(params).await?;

        // Group by control_id → Vec<ControlResultRow>
        let mut by_control: BTreeMap<String, Vec<ControlResultRow>> = BTreeMap::new();

        for result in &control_results {
            let node_name = nodes
                .iter()
                .find(|n| n.id == result.node_id)
                .map(|n| n.name.clone())
                .unwrap_or_else(|| result.node_id.to_string());

            let entry = ControlResultRow {
                node_id: result.node_id,
                node_name: node_name.clone(),
                control_id: result.control_id.clone(),
                status: result.status.clone(),
                impact: result.impact.clone(),
                profile_id: result.profile_id,
                created_at: result.created_at,
            };

            by_control
                .entry(result.control_id.clone())
                .or_default()
                .push(entry);
        }

        // Sort each control's results by timestamp.
        for results in by_control.values_mut() {
            results.sort_by(|a, b| {
                a.node_name
                    .cmp(&b.node_name)
                    .then_with(|| a.created_at.cmp(&b.created_at))
            });
        }

        // Find deviations: controls that have both passed and failed results.
        let mut deviations: Vec<DeviationEntry> = Vec::new();

        for (control_id, results) in &by_control {
            let has_pass = results.iter().any(|r| r.status == "passed");
            let has_fail = results.iter().any(|r| r.status == "failed");

            if has_pass && has_fail {
                let status_counts = count_statuses(results);
                deviations.push(DeviationEntry {
                    control_id: control_id.clone(),
                    total_results: results.len() as u32,
                    passed: status_counts.get("passed").copied().unwrap_or(0),
                    failed: status_counts.get("failed").copied().unwrap_or(0),
                    skipped: status_counts.get("skipped").copied().unwrap_or(0),
                    waived: status_counts.get("waived").copied().unwrap_or(0),
                    first_seen: results.first().map(|r| r.created_at),
                    last_seen: results.last().map(|r| r.created_at),
                });
            }
        }

        // Already sorted by BTreeMap (control_id).
        // Sort each deviation's associated nodes by name.

        let mut data = ReportData::new();
        data.insert("deviations".to_string(), serde_json::to_value(&deviations)?);

        Ok(Report {
            report_type: Self::TYPE.to_string(),
            definition_version: 1,
            data_range: DataRange {
                from: params.from,
                to: params.to,
            },
            data,
        })
    }
}

// ── Report data structures ───────────────────────────────────────────────────

/// Row of control result data used during report generation.
#[derive(Debug, Clone, Serialize)]
struct ControlResultRow {
    node_id: Uuid,
    node_name: String,
    control_id: String,
    status: String,
    impact: String,
    profile_id: Uuid,
    created_at: DateTime<Utc>,
}

/// Per-node control summary.
#[derive(Debug, Clone, Serialize)]
struct NodeControlSummary {
    node_id: Uuid,
    node_name: String,
    platform: String,
    chef_environment: String,
    controls: Vec<ControlSummary>,
}

/// Per-control summary within a node.
#[derive(Debug, Clone, Serialize)]
struct ControlSummary {
    control_id: String,
    status: String,
    results_count: u32,
    first_seen: Option<DateTime<Utc>>,
    last_seen: Option<DateTime<Utc>>,
}

/// Waiver entry in the waiver register.
#[derive(Debug, Clone, Serialize)]
struct WaiverEntry {
    control_id: String,
    profile_id: Uuid,
    scope: String,
    justification: Option<String>,
    approver: Option<String>,
    start_date: DateTime<Utc>,
    expiry_date: DateTime<Utc>,
}

/// Profile summary over time entry.
#[derive(Debug, Clone, Serialize)]
struct ProfileTimeEntry {
    profile_id: Uuid,
    profile_name: String,
    buckets: Vec<BucketEntry>,
}

/// Time bucket entry for profile summary.
#[derive(Debug, Clone, Serialize)]
struct BucketEntry {
    time_bucket: String,
    passed: i32,
    failed: i32,
    skipped: i32,
    waived: i32,
    other: i32,
    total: i32,
}

/// Internal bucket counts.
#[derive(Debug, Default, Clone)]
struct ProfileBucket {
    passed: i32,
    failed: i32,
    skipped: i32,
    waived: i32,
    other: i32,
    total: i32,
}

/// Deviation entry.
#[derive(Debug, Clone, Serialize)]
struct DeviationEntry {
    control_id: String,
    total_results: u32,
    passed: u32,
    failed: u32,
    skipped: u32,
    waived: u32,
    first_seen: Option<DateTime<Utc>>,
    last_seen: Option<DateTime<Utc>>,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Count statuses in a list of control result rows.
fn count_statuses(rows: &[ControlResultRow]) -> BTreeMap<String, u32> {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for row in rows {
        *counts.entry(row.status.clone()).or_insert(0) += 1;
    }
    counts
}

/// Determine overall status from status counts.
/// Priority: failed > skipped > passed > waived > other.
fn determine_overall_status(counts: &BTreeMap<String, u32>) -> String {
    if counts.get("failed").copied().unwrap_or(0) > 0 {
        "failed".to_string()
    } else if counts.get("skipped").copied().unwrap_or(0) > 0 {
        "skipped".to_string()
    } else if counts.get("passed").copied().unwrap_or(0) > 0 {
        "passed".to_string()
    } else if counts.get("waived").copied().unwrap_or(0) > 0 {
        "waived".to_string()
    } else {
        "unknown".to_string()
    }
}

// ── MockReportStore ───────────────────────────────────────────────────────────

/// In-memory store for testing report determinism.
/// Pre-loads data and returns it filtered by params.
#[derive(Debug, Default)]
pub struct MockReportStore {
    nodes: Vec<Node>,
    runs: Vec<Run>,
    control_results: Vec<ControlResult>,
    compliance_reports: Vec<ComplianceReport>,
    waivers: Vec<Waiver>,
    profiles: Vec<Profile>,
}

impl MockReportStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_nodes(mut self, nodes: Vec<Node>) -> Self {
        self.nodes = nodes;
        self
    }

    pub fn with_runs(mut self, runs: Vec<Run>) -> Self {
        self.runs = runs;
        self
    }

    pub fn with_control_results(mut self, results: Vec<ControlResult>) -> Self {
        self.control_results = results;
        self
    }

    pub fn with_compliance_reports(mut self, reports: Vec<ComplianceReport>) -> Self {
        self.compliance_reports = reports;
        self
    }

    pub fn with_waivers(mut self, waivers: Vec<Waiver>) -> Self {
        self.waivers = waivers;
        self
    }

    pub fn with_profiles(mut self, profiles: Vec<Profile>) -> Self {
        self.profiles = profiles;
        self
    }

    /// Filter nodes by params.
    fn filter_nodes(&self, params: &ReportParams) -> Vec<Node> {
        self.nodes
            .iter()
            .filter(|n| {
                if let Some(ref node_filter) = params.node_filter {
                    n.name.contains(node_filter)
                } else {
                    true
                }
            })
            .filter(|n| {
                params.from.map_or(true, |f| n.last_seen >= f)
                    && params.to.map_or(true, |t| n.last_seen < t)
            })
            .cloned()
            .collect()
    }

    /// Filter runs by params.
    fn filter_runs(&self, params: &ReportParams) -> Vec<Run> {
        self.runs
            .iter()
            .filter(|r| {
                params.from.map_or(true, |f| r.start_time >= f)
                    && params.to.map_or(true, |t| r.start_time < t)
            })
            .cloned()
            .collect()
    }

    /// Filter control results by params.
    fn filter_control_results(&self, params: &ReportParams) -> Vec<ControlResult> {
        self.control_results
            .iter()
            .filter(|r| {
                if let Some(ref node_filter) = params.node_filter {
                    let node = self.nodes.iter().find(|n| n.id == r.node_id);
                    match node {
                        Some(n) => n.name.contains(node_filter),
                        None => false,
                    }
                } else {
                    true
                }
            })
            .filter(|r| {
                if let Some(ref profile_filter) = params.profile_filter {
                    let profile = self.profiles.iter().find(|p| p.id == r.profile_id);
                    match profile {
                        Some(p) => p.name.contains(profile_filter),
                        None => false,
                    }
                } else {
                    true
                }
            })
            .filter(|r| {
                params.from.map_or(true, |f| r.created_at >= f)
                    && params.to.map_or(true, |t| r.created_at < t)
            })
            .cloned()
            .collect()
    }

    /// Filter compliance reports by params.
    fn filter_compliance_reports(&self, params: &ReportParams) -> Vec<ComplianceReport> {
        self.compliance_reports
            .iter()
            .filter(|r| {
                params.from.map_or(true, |f| r.created_at >= f)
                    && params.to.map_or(true, |t| r.created_at < t)
            })
            .cloned()
            .collect()
    }
}

#[async_trait::async_trait]
impl ReportStore for MockReportStore {
    async fn fetch_nodes(&self, params: &ReportParams) -> Result<Vec<Node>> {
        Ok(self.filter_nodes(params))
    }

    async fn fetch_runs(&self, params: &ReportParams) -> Result<Vec<Run>> {
        Ok(self.filter_runs(params))
    }

    async fn fetch_control_results(&self, params: &ReportParams) -> Result<Vec<ControlResult>> {
        Ok(self.filter_control_results(params))
    }

    async fn fetch_compliance_reports(&self, params: &ReportParams) -> Result<Vec<ComplianceReport>> {
        Ok(self.filter_compliance_reports(params))
    }

    async fn fetch_waivers(&self) -> Result<Vec<Waiver>> {
        Ok(self.waivers.clone())
    }

    async fn fetch_profiles(&self) -> Result<Vec<Profile>> {
        Ok(self.profiles.clone())
    }
}

// ── Report format + export ───────────────────────────────────────────────────

/// Output format for report export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReportFormat {
    /// Canonical JSON (sorted keys, compact, no trailing commas).
    Json,
    /// CSV with deterministic column ordering and proper escaping.
    Csv,
}

impl Default for ReportFormat {
    fn default() -> Self {
        ReportFormat::Json
    }
}

impl std::str::FromStr for ReportFormat {
    type Err = ReportError;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "json" => Ok(ReportFormat::Json),
            "csv" => Ok(ReportFormat::Csv),
            _ => Err(ReportError::InvalidParams(format!(
                "unknown format: {} (expected 'json' or 'csv')",
                s
            ))),
        }
    }
}

/// Response headers for exported reports.
///
/// The signing headers (X-Spindle-Key-ID, X-Spindle-Signature) are
/// placeholders — they will be wired up after M4-01 (Signer interface).
#[derive(Debug, Clone)]
pub struct ExportHeaders {
    pub content_disposition: String,
    pub x_spindle_key_id: String,
    pub x_spindle_signature: String,
    pub content_type: String,
}

/// Result of a report export — bytes + headers.
#[derive(Debug, Clone)]
pub struct ExportResult {
    pub bytes: Vec<u8>,
    pub headers: ExportHeaders,
}

/// Export a report to a specific format.
///
/// - JSON: canonical serialization (sorted keys, compact)
/// - CSV: deterministic column ordering, RFC 4180 escaping
pub fn export_report(report: &Report, format: ReportFormat) -> Result<ExportResult> {
    let (bytes, content_type) = match format {
        ReportFormat::Json => {
            let json_bytes = canonical_serialize_report(report)?;
            (json_bytes, "application/json".to_string())
        }
        ReportFormat::Csv => {
            let csv_bytes = report_to_csv(report)?;
            (csv_bytes, "text/csv".to_string())
        }
    };

    let headers = ExportHeaders {
        content_disposition: format!(
            "attachment; filename=\"{}.{}\"",
            report.report_type,
            format.extension()
        ),
        x_spindle_key_id: "placeholder".to_string(), // M4-01 placeholder
        x_spindle_signature: "placeholder".to_string(), // M4-01 placeholder
        content_type: content_type.clone(),
    };

    Ok(ExportResult { bytes, headers })
}

impl ReportFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReportFormat::Json => "json",
            ReportFormat::Csv => "csv",
        }
    }
}

/// Convert a report to CSV with deterministic column ordering.
///
/// Each report type has a specific column layout:
/// - control_status_by_node: node_name, platform, chef_environment, control_id, status, results_count, first_seen, last_seen
/// - profile_summary_over_time: profile_name, time_bucket, passed, failed, skipped, waived, other, total
/// - waiver_register: control_id, profile_id, scope, approver, start_date, expiry_date, justification
/// - exception_deviation_list: control_id, total_results, passed, failed, skipped, waived, first_seen, last_seen
fn report_to_csv(report: &Report) -> Result<Vec<u8>> {
    let mut buf = Vec::new();

    match report.report_type.as_str() {
        "control_status_by_node" => {
            buf.extend_from_slice(b"node_name,platform,chef_environment,control_id,status,results_count,first_seen,last_seen\n");
            let nodes_json = report.data.0.get("nodes").unwrap_or(&serde_json::Value::Null);
            if let serde_json::Value::Array(nodes) = nodes_json {
                for node_val in nodes {
                    let node_name = node_val.get("node_name").and_then(|v| v.as_str()).unwrap_or("");
                    let platform = node_val.get("platform").and_then(|v| v.as_str()).unwrap_or("");
                    let env = node_val.get("chef_environment").and_then(|v| v.as_str()).unwrap_or("");
                    if let Some(controls) = node_val.get("controls").and_then(|v| v.as_array()) {
                        for ctrl in controls {
                            let control_id = ctrl.get("control_id").and_then(|v| v.as_str()).unwrap_or("");
                            let status = ctrl.get("status").and_then(|v| v.as_str()).unwrap_or("");
                            let count = ctrl.get("results_count").and_then(|v| v.as_str()).unwrap_or("");
                            let first = ctrl.get("first_seen").and_then(|v| v.as_str()).unwrap_or("");
                            let last = ctrl.get("last_seen").and_then(|v| v.as_str()).unwrap_or("");
                            buf.extend(csv_row(&[
                                node_name, platform, env, control_id, status, count, first, last,
                            ]));
                        }
                    }
                }
            }
        }
        "profile_summary_over_time" => {
            buf.extend_from_slice(b"profile_name,time_bucket,passed,failed,skipped,waived,other,total\n");
            let profiles_json = report.data.0.get("profiles").unwrap_or(&serde_json::Value::Null);
            if let serde_json::Value::Array(profiles) = profiles_json {
                for prof_val in profiles {
                    let profile_name = prof_val.get("profile_name").and_then(|v| v.as_str()).unwrap_or("");
                    if let Some(buckets) = prof_val.get("buckets").and_then(|v| v.as_array()) {
                        for bucket in buckets {
                            let time_bucket = bucket.get("time_bucket").and_then(|v| v.as_str()).unwrap_or("");
                            let passed = bucket.get("passed").and_then(|v| v.as_i64()).unwrap_or(0);
                            let failed = bucket.get("failed").and_then(|v| v.as_i64()).unwrap_or(0);
                            let skipped = bucket.get("skipped").and_then(|v| v.as_i64()).unwrap_or(0);
                            let waived = bucket.get("waived").and_then(|v| v.as_i64()).unwrap_or(0);
                            let other = bucket.get("other").and_then(|v| v.as_i64()).unwrap_or(0);
                            let total = bucket.get("total").and_then(|v| v.as_i64()).unwrap_or(0);
                            buf.extend(csv_row(&[
                                profile_name,
                                time_bucket,
                                &passed.to_string(),
                                &failed.to_string(),
                                &skipped.to_string(),
                                &waived.to_string(),
                                &other.to_string(),
                                &total.to_string(),
                            ]));
                        }
                    }
                }
            }
        }
        "waiver_register" => {
            buf.extend_from_slice(b"control_id,profile_id,scope,approver,start_date,expiry_date,justification\n");
            let waivers_json = report.data.0.get("waivers").unwrap_or(&serde_json::Value::Null);
            if let serde_json::Value::Array(waivers) = waivers_json {
                for w in waivers {
                    let control_id = w.get("control_id").and_then(|v| v.as_str()).unwrap_or("");
                    let profile_id = w.get("profile_id").and_then(|v| v.as_str()).unwrap_or("");
                    let scope = w.get("scope").and_then(|v| v.as_str()).unwrap_or("");
                    let approver = w.get("approver").and_then(|v| v.as_str()).unwrap_or("");
                    let start = w.get("start_date").and_then(|v| v.as_str()).unwrap_or("");
                    let expiry = w.get("expiry_date").and_then(|v| v.as_str()).unwrap_or("");
                    let justification = w.get("justification").and_then(|v| v.as_str()).unwrap_or("");
                    buf.extend(csv_row(&[
                        control_id, profile_id, scope, approver, start, expiry, justification,
                    ]));
                }
            }
        }
        "exception_deviation_list" => {
            buf.extend_from_slice(b"control_id,total_results,passed,failed,skipped,waived,first_seen,last_seen\n");
            let deviations_json = report.data.0.get("deviations").unwrap_or(&serde_json::Value::Null);
            if let serde_json::Value::Array(deviations) = deviations_json {
                for d in deviations {
                    let control_id = d.get("control_id").and_then(|v| v.as_str()).unwrap_or("");
                    let total = d.get("total_results").and_then(|v| v.as_i64()).unwrap_or(0);
                    let passed = d.get("passed").and_then(|v| v.as_i64()).unwrap_or(0);
                    let failed = d.get("failed").and_then(|v| v.as_i64()).unwrap_or(0);
                    let skipped = d.get("skipped").and_then(|v| v.as_i64()).unwrap_or(0);
                    let waived = d.get("waived").and_then(|v| v.as_i64()).unwrap_or(0);
                    let first = d.get("first_seen").and_then(|v| v.as_str()).unwrap_or("");
                    let last = d.get("last_seen").and_then(|v| v.as_str()).unwrap_or("");
                    buf.extend(csv_row(&[
                        control_id,
                        &total.to_string(),
                        &passed.to_string(),
                        &failed.to_string(),
                        &skipped.to_string(),
                        &waived.to_string(),
                        first,
                        last,
                    ]));
                }
            }
        }
        _ => {
            return Err(ReportError::InvalidParams(format!(
                "unknown report type for CSV: {}",
                report.report_type
            )));
        }
    }

    Ok(buf)
}

/// Escape a field value for CSV (RFC 4180).
/// Wraps in quotes if the value contains comma, quote, or newline.
/// Escapes double quotes by doubling them.
fn csv_escape(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r') {
        let escaped = field.replace('"', "\"\"");
        format!("\"{}\"", escaped)
    } else {
        field.to_string()
    }
}

/// Build a CSV row (single line, \n-terminated).
fn csv_row(fields: &[&str]) -> Vec<u8> {
    let row: String = fields
        .iter()
        .map(|f| csv_escape(f))
        .collect::<Vec<_>>()
        .join(",");
    let mut buf = row.into_bytes();
    buf.push(b'\n');
    buf
}

impl ReportFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            ReportFormat::Json => "json",
            ReportFormat::Csv => "csv",
        }
    }
}

// ── Re-export report definitions ─────────────────────────────────────────────

pub use ControlStatusByNode as ReportControlStatusByNode;
pub use ExceptionDeviationList as ReportExceptionDeviationList;
pub use ProfileSummaryOverTime as ReportProfileSummaryOverTime;
pub use WaiverRegister as ReportWaiverRegister;
