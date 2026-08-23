//! spindle-pipeline: Parse + normalize Cinc data-collector and Cinc Auditor payloads.
//!
//! Parses Cinc `data-collector` JSON into typed structs, normalizes
//! timestamps, maps status strings to enums, extracts resource events with
//! action/status classification, and detects no-op resources (M1-21).
//!
//! Parses Cinc Auditor compliance reports and extracts control results (M1-23).
//!
//! Dead-letter queue for failed payloads with retry logic (M1-25).
//!
//! Schema version stamping and cookbook usage extraction (M1-26).
//!
//! No raw SQL — pure parse + normalize. DB operations are in `spindle-store`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
#[allow(unused_imports)]
use tracing::{debug, info, trace};

// ── Schema version (M1-26) ────────────────────────────────────────────────

/// Current schema version for derived tables.
/// Increment this ONLY when table structure (columns, data types) changes.
/// Do NOT increment for index or partition changes.
pub const SCHEMA_VERSION: i32 = 1;

/// Marker trait for types that should carry a `schema_version` column.
/// Derived tables stamped with `SCHEMA_VERSION` on every row.
pub trait SchemaVersioned {
    /// Returns the current schema version.
    fn schema_version(&self) -> i32 {
        SCHEMA_VERSION
    }
}

// ── Resource status (M1-21) ────────────────────────────────────────────────

/// Status of a Cinc resource event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceStatus {
    UpToDate,
    Updated,
    Failed,
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

/// Attempt to parse a Cinc status string into `ResourceStatus`.
pub fn parse_status(s: &str) -> Option<ResourceStatus> {
    match s {
        "up-to-date" | "up_to_date" => Some(ResourceStatus::UpToDate),
        "updated" => Some(ResourceStatus::Updated),
        "failed" => Some(ResourceStatus::Failed),
        "skipped" => Some(ResourceStatus::Skipped),
        _ => None,
    }
}

// ── Resource event (M1-21) ────────────────────────────────────────────────

/// A single resource event from a Cinc run-converge payload.
///
/// **Schema evolution**: Any unrecognized JSON field is captured in `extra_fields`.
/// To promote an extra field to a typed column, add it as a named field here and
/// create a migration that adds the corresponding DB column. Old payloads will
/// continue parsing -- the field will stop being captured in `extra_fields` once
/// the migration is applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceEvent {
    pub name: String,
    #[serde(rename = "status")]
    pub status: String,
    #[serde(default, alias = "cookbook_name")]
    pub cookbook: Option<String>,
    #[serde(default)]
    pub recipe: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Value>,
    /// All unrecognized fields from the original JSON payload.
    #[serde(flatten)]
    pub extra_fields: Value,
}

/// Parsed resource event with typed status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedResourceEvent {
    pub name: String,
    pub status: ResourceStatus,
    pub cookbook: Option<String>,
    pub recipe: Option<String>,
    pub properties: Option<Value>,
    /// Unrecognized fields from the original JSON payload.
    pub extra_fields: Value,
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
            extra_fields: event.extra_fields,
        })
    }
}

// ── Run statistics (M1-21) ────────────────────────────────────────────────

/// Aggregated statistics for a Cinc run after pipeline processing.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunResourceStats {
    pub total_resource_count: u64,
    pub updated_count: u64,
    pub failed_count: u64,
    pub skipped_count: u64,
    pub up_to_date_count: u64,
    pub persisted_count: u64,
}

impl RunResourceStats {
    pub fn is_consistent(&self) -> bool {
        self.updated_count + self.failed_count + self.skipped_count + self.up_to_date_count
            == self.total_resource_count
    }

    pub fn is_persisted_consistent(&self) -> bool {
        self.persisted_count == self.updated_count + self.failed_count + self.skipped_count
    }

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

// ── Pipeline processing (M1-21) ────────────────────────────────────────────

/// Result of pipeline processing: filtered events to persist + run statistics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PipelineResult {
    pub persistable_events: Vec<ParsedResourceEvent>,
    pub stats: RunResourceStats,
}

/// Metrics for pipeline processing throughput.
#[derive(Debug, Clone, Default)]
pub struct PipelineMetrics {
    pub processed_total: u64,
    pub error_total: u64,
    pub avg_latency_ms: f64,
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
    #[error("parse error: {0}")]
    ParseError(String),
}

/// Process a list of resource events from a normalized Cinc run-converge payload.
///
/// No-op filtering: resources with status "up-to-date" are counted but NOT
/// inserted into resource_events. Resources with "updated", "failed", or
/// "skipped" statuses ARE persisted.
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

    // L2: per-resource breakdown (debug-level details)
    let mut resources: Vec<(String, String, Option<String>, Option<String>)> =
        Vec::with_capacity(events.len());

    for event in events {
        let parsed = ParsedResourceEvent::from_event(event).ok_or_else(|| {
            PipelineError::UnknownStatus("unable to parse resource status".to_string())
        })?;

        // Collect resource info for L2 logging
        resources.push((
            parsed.name.clone(),
            parsed.status.to_string(),
            parsed.cookbook.clone(),
            parsed.recipe.clone(),
        ));

        match parsed.status {
            ResourceStatus::UpToDate => {
                stats.up_to_date_count += 1;
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

    // L2: per-resource breakdown log
    tracing::debug!(
        resources = ?resources,
        status_counts = %format!(
            "updated={}, failed={}, skipped={}, up_to_date={}",
            stats.updated_count, stats.failed_count, stats.skipped_count, stats.up_to_date_count
        ),
        filtered_out = stats.up_to_date_count,
        "per-resource breakdown"
    );

    stats.persisted_count = persistable_events.len() as u64;
    stats.reconcile()?;

    // L1: events processed
    tracing::info!(
        events_processed = total,
        outcome = "ok",
        updated = stats.updated_count,
        failed = stats.failed_count,
        skipped = stats.skipped_count,
        up_to_date = stats.up_to_date_count,
        "pipeline processed run"
    );

    Ok(PipelineResult {
        persistable_events,
        stats,
    })
}

/// Extract resource events from a normalized Cinc run-converge JSON payload.
pub fn extract_resource_events(payload: &Value) -> Result<Vec<ResourceEvent>, PipelineError> {
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
pub fn process_payload(payload: &Value) -> Result<PipelineResult, PipelineError> {
    let events = extract_resource_events(payload)?;
    // L3: intermediate state — parsed vector before processing
    tracing::trace!(
        parsed_event_count = events.len(),
        parsed = ?events.iter().map(|e| (&e.name, &e.status)).collect::<Vec<_>>(),
        "pipeline parsed resource events"
    );
    process_resource_events(events)
}

// ── Cookbook usage extraction (M1-26) ──────────────────────────────────────

/// Cookbook usage entry for deduplication per run per node.
/// Represents a unique (node_id, run_id, cookbook_name, cookbook_version) tuple.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CookbookUsage {
    pub node_id: String,
    pub run_id: String,
    pub cookbook_name: String,
    pub cookbook_version: String,
    pub first_seen: String,
    pub last_seen: String,
    /// Schema version — stamped on every row for tracking table evolution.
    #[serde(default = "default_schema_version")]
    pub schema_version: i32,
}

fn default_schema_version() -> i32 {
    SCHEMA_VERSION
}

impl CookbookUsage {
    pub fn new(node_id: &str, run_id: &str, cookbook_name: &str, cookbook_version: &str) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            node_id: node_id.to_string(),
            run_id: run_id.to_string(),
            cookbook_name: cookbook_name.to_string(),
            cookbook_version: cookbook_version.to_string(),
            first_seen: now.clone(),
            last_seen: now,
            schema_version: SCHEMA_VERSION,
        }
    }
}

impl SchemaVersioned for CookbookUsage {}

/// Extract cookbook usage from resource events.
///
/// For each resource event that has a cookbook_name, produces a
/// `CookbookUsage` entry with schema_version stamped.
///
/// Deduplication is per (node_id, run_id, cookbook_name, cookbook_version).
/// If the same cookbook appears in multiple resources within the same run,
/// only one entry is produced (first_seen / last_seen updated to latest).
pub fn extract_cookbook_usage(
    events: &[ParsedResourceEvent],
    node_id: &str,
    run_id: &str,
) -> Vec<CookbookUsage> {
    let mut map: std::collections::BTreeMap<(String, String), CookbookUsage> =
        std::collections::BTreeMap::new();
    let now = chrono::Utc::now().to_rfc3339();

    for event in events {
        let cb_name = match &event.cookbook {
            Some(c) => c,
            None => continue,
        };

        let cb_version = extract_cookbook_version(event);

        let key = (cb_name.clone(), cb_version.clone());
        let entry = map
            .entry(key)
            .or_insert_with(|| CookbookUsage::new(node_id, run_id, cb_name, &cb_version));
        entry.last_seen = now.clone();
        entry.schema_version = SCHEMA_VERSION;
    }

    map.into_values().collect()
}

/// Extract cookbook version from a resource event's properties.
///
/// Best-effort extraction; returns "unknown" if not found.
fn extract_cookbook_version(event: &ParsedResourceEvent) -> String {
    if let Some(ref props) = event.properties {
        if let Some(obj) = props.as_object() {
            if let Some(v) = obj.get("cookbook_version") {
                if let Some(s) = v.as_str() {
                    return s.to_string();
                }
            }
            if let Some(v) = obj.get("version") {
                if let Some(s) = v.as_str() {
                    return s.to_string();
                }
            }
        }
    }
    "unknown".to_string()
}

// ── Compliance report parsing (Cinc Auditor) (M1-23) ─────────────────────────────

/// Status of a Cinc Auditor control result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum AuditorStatus {
    Passed,
    Failed,
    Skipped,
    Unknown,
}

impl std::fmt::Display for AuditorStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditorStatus::Passed => write!(f, "passed"),
            AuditorStatus::Failed => write!(f, "failed"),
            AuditorStatus::Skipped => write!(f, "skipped"),
            AuditorStatus::Unknown => write!(f, "unknown"),
        }
    }
}

/// Parse a Cinc Auditor status string into `AuditorStatus`.
pub fn parse_auditor_status(s: &str) -> AuditorStatus {
    match s.to_lowercase().as_str() {
        "passed" => AuditorStatus::Passed,
        "failed" => AuditorStatus::Failed,
        "skipped" => AuditorStatus::Skipped,
        _ => AuditorStatus::Unknown,
    }
}

/// Source code location of a control.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceLocation {
    #[serde(alias = "ref", default)]
    pub ref_text: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub line: Option<u64>,
}

/// A reference from a control to external documentation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControlRef {
    pub url: Option<String>,
    pub pub_id: Option<String>,
    pub requirement: Option<String>,
}

/// A single control result from a Cinc Auditor profile.
///
/// **Schema evolution**: Unknown fields land in `extra_fields`. Promote by adding
/// a typed field to this struct and creating a migration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlResult {
    pub status: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub run_time: Option<f64>,
    #[serde(default)]
    pub start_time: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub skip_reason: Option<String>,
    /// All unrecognized fields from the original control result JSON.
    #[serde(flatten)]
    pub extra_fields: Value,
}

/// A control definition from a Cinc Auditor profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Control {
    pub id: String,
    pub title: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub impact: Option<f64>,
    pub tags: Option<Value>,
    #[serde(default)]
    pub refs: Vec<ControlRef>,
    #[serde(default)]
    pub source_location: Option<SourceLocation>,
    #[serde(default)]
    pub code: Option<String>,
    pub results: Vec<ControlResult>,
}

/// A Cinc Auditor profile within a compliance report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub maintainer: Option<String>,
    #[serde(default)]
    pub copyright: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub supports: Option<Value>,
    #[serde(default)]
    pub controls: Vec<Control>,
    #[serde(default)]
    pub attributes: Option<Value>,
    #[serde(default)]
    pub groups: Option<Value>,
}

/// Platform information from the reporting node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Platform {
    pub name: String,
    #[serde(default)]
    pub release: Option<String>,
}

/// Statistics from the Cinc Auditor run.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct AuditorStatistics {
    #[serde(default)]
    pub duration: Option<f64>,
}

/// A parsed Cinc Auditor compliance report.
///
/// **Schema evolution**: When the Cinc Auditor JSON reporter adds new top-level fields
/// (new metadata, new top-level keys), add them to this struct and create a
/// migration that adds the corresponding columns. Until the migration is applied,
/// unrecognized fields are captured in `extra_fields` below.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceReport {
    pub platform: Platform,
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub statistics: Option<AuditorStatistics>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub organization: Option<String>,
    /// All unrecognized fields from the Cinc Auditor JSON reporter payload.
    #[serde(flatten)]
    pub extra_fields: Value,
}

/// Parser for Cinc Auditor compliance report JSON.
#[derive(Debug, Clone, Default)]
pub struct ComplianceReportParser;

impl ComplianceReportParser {
    pub fn new() -> Self {
        Self
    }

    /// Parse a raw JSON value into a typed `ComplianceReport`.
    pub fn parse(&self, payload: &Value) -> Result<ComplianceReport, PipelineError> {
        let platform = payload
            .get("platform")
            .ok_or(PipelineError::ParseError("missing platform".to_string()))?
            .clone();
        let platform: Platform = serde_json::from_value(platform)
            .map_err(|e| PipelineError::ParseError(format!("platform: {}", e)))?;

        let profiles: Vec<Profile> = payload
            .get("profiles")
            .and_then(|p| p.as_array())
            .ok_or(PipelineError::EmptyResources)?
            .iter()
            .map(|p| serde_json::from_value::<Profile>(p.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| PipelineError::ParseError(format!("profile: {}", e)))?;

        let statistics = payload
            .get("statistics")
            .and_then(|s| serde_json::from_value::<AuditorStatistics>(s.clone()).ok());

        let version = payload
            .get("version")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let organization = payload
            .get("organization")
            .and_then(|o| o.as_str())
            .map(|s| s.to_string());

        // Capture unrecognized top-level fields for schema evolution
        let known_keys = [
            "platform",
            "profiles",
            "statistics",
            "version",
            "organization",
        ];
        let mut extra = serde_json::Map::new();
        if let Value::Object(map) = payload {
            for (k, v) in map {
                if !known_keys.contains(&k.as_str()) {
                    extra.insert(k.clone(), v.clone());
                }
            }
        }

        Ok(ComplianceReport {
            platform,
            profiles,
            statistics,
            version,
            organization,
            extra_fields: Value::Object(extra),
        })
    }

    /// Extract all control results from a parsed compliance report.
    ///
    /// Returns a flat list of `ParsedControlResult` entries, one per
    /// control result in every profile. Unknown fields from the original
    /// JSON are captured in `extra_fields` (captured by serde(flatten)
    /// during Profile/ControlResult deserialization).
    pub fn extract_control_results(&self, report: &ComplianceReport) -> Vec<ParsedControlResult> {
        let mut results = Vec::new();
        for profile in &report.profiles {
            for control in &profile.controls {
                for result in &control.results {
                    // extra_fields already captured by #[serde(flatten)] on ControlResult
                    results.push(ParsedControlResult {
                        control_id: control.id.clone(),
                        status: parse_auditor_status(&result.status),
                        title: control.title.clone(),
                        description: control.description.clone(),
                        impact: control.impact,
                        code: result.code.clone().or_else(|| control.code.clone()),
                        run_time: result.run_time,
                        start_time: result.start_time.clone(),
                        message: result.message.clone(),
                        skip_reason: result.skip_reason.clone(),
                        refs: control.refs.clone(),
                        source_location: control.source_location.clone(),
                        profile_name: profile.name.clone(),
                        profile_version: profile.version.clone(),
                        extra_fields: result.extra_fields.clone(),
                    });
                }
            }
        }
        results
    }
}

/// A control result with typed status, ready for insertion into `control_results` table.
///
/// **Schema evolution**: Unknown fields land in `extra_fields`. Promote by adding
/// a typed field to this struct and creating a migration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParsedControlResult {
    pub control_id: String,
    pub status: AuditorStatus,
    pub title: Option<String>,
    pub description: Option<String>,
    pub impact: Option<f64>,
    pub code: Option<String>,
    pub run_time: Option<f64>,
    pub start_time: Option<String>,
    pub message: Option<String>,
    pub skip_reason: Option<String>,
    pub refs: Vec<ControlRef>,
    pub source_location: Option<SourceLocation>,
    pub profile_name: String,
    pub profile_version: Option<String>,
    /// Unrecognized fields from the original control result JSON.
    pub extra_fields: Value,
}

/// Convenience: parse + extract control results in one call.
pub fn process_compliance_report(
    payload: &Value,
) -> Result<Vec<ParsedControlResult>, PipelineError> {
    let parser = ComplianceReportParser::new();
    let report = parser.parse(payload)?;
    Ok(parser.extract_control_results(&report))
}

// ── Dead-letter queue (M1-25) ─────────────────────────────────────────────

/// Types of errors that can cause a payload to be sent to the dead-letter queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DeadLetterErrorType {
    ParseError,
    ProcessingError,
    DbConstraintViolation,
    Panic,
    Unknown,
}

impl std::fmt::Display for DeadLetterErrorType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeadLetterErrorType::ParseError => write!(f, "parse_error"),
            DeadLetterErrorType::ProcessingError => write!(f, "processing_error"),
            DeadLetterErrorType::DbConstraintViolation => write!(f, "db_constraint_violation"),
            DeadLetterErrorType::Panic => write!(f, "panic"),
            DeadLetterErrorType::Unknown => write!(f, "unknown"),
        }
    }
}

/// A dead-letter entry: a payload that failed processing after retries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadLetterEntry {
    pub id: String,
    pub archive_reference: String,
    pub error_message: String,
    pub error_type: DeadLetterErrorType,
    pub retry_count: u32,
    pub created_at: String,
    #[serde(default)]
    pub payload_type: Option<String>,
    #[serde(default)]
    pub node_name: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub reprocessable: bool,
}

impl DeadLetterEntry {
    pub fn new(
        archive_reference: &str,
        error_message: &str,
        error_type: DeadLetterErrorType,
        retry_count: u32,
        payload_type: Option<&str>,
        node_name: Option<&str>,
        run_id: Option<&str>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            archive_reference: archive_reference.to_string(),
            error_message: error_message.to_string(),
            error_type,
            retry_count,
            created_at: chrono::Utc::now().to_rfc3339(),
            payload_type: payload_type.map(|s| s.to_string()),
            node_name: node_name.map(|s| s.to_string()),
            run_id: run_id.map(|s| s.to_string()),
            reprocessable: true,
        }
    }

    pub fn age_seconds(&self) -> i64 {
        let created = chrono::DateTime::parse_from_rfc3339(&self.created_at)
            .map(|dt| dt.timestamp())
            .unwrap_or(0);
        chrono::Utc::now().timestamp() - created
    }

    pub fn is_expired(&self) -> bool {
        self.age_seconds() > DEAD_LETTER_RETENTION_SECONDS
    }
}

/// Default dead-letter retention period: 30 days in seconds.
pub const DEAD_LETTER_RETENTION_SECONDS: i64 = 30 * 24 * 60 * 60;

/// Trait for dead-letter queue storage backends.
pub trait DeadLetterStore: Send + Sync + std::fmt::Debug {
    fn record_failure(&self, entry: DeadLetterEntry);
    fn list_reprocessable(&self) -> Vec<DeadLetterEntry>;
    fn mark_permanent(&self, id: &str);
    fn remove(&self, id: &str);
}

/// In-memory dead-letter store for testing and single-node deployments.
///
/// ⚠️ **Single-instance only**: entries are not shared across instances.
#[derive(Debug, Default)]
pub struct InMemoryDeadLetterStore {
    inner: std::sync::Mutex<Vec<DeadLetterEntry>>,
}

impl InMemoryDeadLetterStore {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl DeadLetterStore for InMemoryDeadLetterStore {
    fn record_failure(&self, entry: DeadLetterEntry) {
        let mut store = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        store.push(entry);
    }

    fn list_reprocessable(&self) -> Vec<DeadLetterEntry> {
        let store = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        store
            .iter()
            .filter(|e| e.reprocessable && !e.is_expired())
            .cloned()
            .collect()
    }

    fn mark_permanent(&self, id: &str) {
        let mut store = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        for entry in store.iter_mut() {
            if entry.id == id {
                entry.reprocessable = false;
            }
        }
    }

    fn remove(&self, id: &str) {
        let mut store = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        store.retain(|e| e.id != id);
    }
}

/// Result of a retry attempt for a dead-letter entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryResult {
    Succeeded,
    Failed { new_retry_count: u32 },
    PermanentFailure { error_message: String },
}

/// Attempt to reprocess a dead-letter entry.
pub fn attempt_retry(
    entry: &DeadLetterEntry,
    max_retries: u32,
    retry_fn: impl FnOnce() -> Result<(), String>,
) -> RetryResult {
    if entry.retry_count >= max_retries {
        return RetryResult::PermanentFailure {
            error_message: "max retries exceeded".to_string(),
        };
    }

    match retry_fn() {
        Ok(()) => RetryResult::Succeeded,
        Err(e) => {
            let new_count = entry.retry_count + 1;
            if new_count >= max_retries {
                RetryResult::PermanentFailure { error_message: e }
            } else {
                RetryResult::Failed {
                    new_retry_count: new_count,
                }
            }
        }
    }
}

/// Admin list endpoint stub — lists dead-letter entries.
pub fn admin_list_dead_letters(
    store: &dyn DeadLetterStore,
    limit: Option<usize>,
) -> Vec<DeadLetterEntry> {
    let mut entries = store.list_reprocessable();
    entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    if let Some(n) = limit {
        entries.truncate(n);
    }
    entries
}

/// Admin reprocess endpoint stub.
pub fn admin_reprocess_dead_letter(
    entry: &DeadLetterEntry,
    max_retries: u32,
    reprocess_fn: impl FnOnce() -> Result<(), String>,
) -> RetryResult {
    attempt_retry(entry, max_retries, reprocess_fn)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_converge_payload(
        up_to_date: usize,
        updated: usize,
        failed: usize,
        skipped: usize,
    ) -> Value {
        let mut resources: Vec<Value> = Vec::new();
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

    fn make_auditor_report() -> Value {
        serde_json::json!({
            "platform": {
                "name": "ubuntu",
                "release": "22.04"
            },
            "profiles": [
                {
                    "name": "linux-baseline",
                    "version": "1.0.0",
                    "title": "Linux Security Baseline",
                    "controls": [
                        {
                            "id": "ssh-01",
                            "title": "SSH Configuration",
                            "description": "SSH should be configured securely",
                            "impact": 1.0,
                            "tags": {"severity": 1},
                            "refs": [{"url": "https://example.com/ssh"}],
                            "source_location": {"ref": "controls/ssh.rb:10", "file": "controls/ssh.rb", "line": 10},
                            "code": "control 'ssh-01' do\n  impact 1.0\nend",
                            "results": [
                                {
                                    "status": "passed",
                                    "code": "describe sshd_config do\n  its('PermitRootLogin') { should eq 'no' }\nend",
                                    "run_time": 0.05,
                                    "start_time": "2024-01-01T00:00:00+00:00"
                                }
                            ]
                        },
                        {
                            "id": "ssh-02",
                            "title": null,
                            "description": null,
                            "impact": null,
                            "tags": null,
                            "refs": [],
                            "source_location": null,
                            "code": null,
                            "results": [
                                {
                                    "status": "failed",
                                    "message": "expected: yes, got: no"
                                }
                            ]
                        },
                        {
                            "id": "ssh-03",
                            "title": "SSH Port",
                            "description": null,
                            "impact": 0.5,
                            "tags": {},
                            "refs": [{"url": null, "requirement": "SSH should run on port 22"}],
                            "source_location": null,
                            "code": null,
                            "results": [
                                {
                                    "status": "skipped",
                                    "skip_reason": "Port 22 is not in use on this system"
                                }
                            ]
                        },
                        {
                            "id": "ssh-04",
                            "title": null,
                            "description": null,
                            "impact": null,
                            "tags": null,
                            "refs": [],
                            "source_location": null,
                            "code": null,
                            "results": [
                                {
                                    "status": "weird"
                                }
                            ]
                        }
                    ]
                }
            ],
            "statistics": {
                "duration": 1.5
            },
            "version": "4.56.29"
        })
    }

    // ── M1-21: No-op filtering tests ───────────────────────────────────────

    #[test]
    fn test_resource_status_display() {
        assert_eq!(ResourceStatus::UpToDate.to_string(), "up-to-date");
        assert_eq!(ResourceStatus::Updated.to_string(), "updated");
        assert_eq!(ResourceStatus::Failed.to_string(), "failed");
        assert_eq!(ResourceStatus::Skipped.to_string(), "skipped");
    }

    #[test]
    fn test_parse_status() {
        assert_eq!(parse_status("up-to-date"), Some(ResourceStatus::UpToDate));
        assert_eq!(parse_status("up_to_date"), Some(ResourceStatus::UpToDate));
        assert_eq!(parse_status("updated"), Some(ResourceStatus::Updated));
        assert_eq!(parse_status("failed"), Some(ResourceStatus::Failed));
        assert_eq!(parse_status("skipped"), Some(ResourceStatus::Skipped));
        assert_eq!(parse_status("unknown"), None);
    }

    #[test]
    fn test_process_events_mixed_statuses() {
        let payload = make_converge_payload(95, 3, 2, 0);
        let result = process_payload(&payload).unwrap();

        // Only 5 events should be persistable (3 updated + 2 failed)
        assert_eq!(result.persistable_events.len(), 5);

        // All counts should be correct
        assert_eq!(result.stats.total_resource_count, 100);
        assert_eq!(result.stats.updated_count, 3);
        assert_eq!(result.stats.failed_count, 2);
        assert_eq!(result.stats.skipped_count, 0);
        assert_eq!(result.stats.up_to_date_count, 95);
        assert_eq!(result.stats.persisted_count, 5);
    }

    #[test]
    fn test_process_events_all_up_to_date() {
        let payload = make_converge_payload(10, 0, 0, 0);
        let result = process_payload(&payload).unwrap();
        assert_eq!(result.stats.total_resource_count, 10);
        assert_eq!(result.stats.up_to_date_count, 10);
        assert_eq!(result.stats.updated_count, 0);
        assert_eq!(result.stats.failed_count, 0);
        assert_eq!(result.stats.skipped_count, 0);
        assert_eq!(result.stats.persisted_count, 0);
        assert!(result.persistable_events.is_empty());
    }

    #[test]
    fn test_process_events_all_skipped() {
        let payload = make_converge_payload(0, 0, 0, 10);
        let result = process_payload(&payload).unwrap();
        assert_eq!(result.stats.skipped_count, 10);
        assert_eq!(result.stats.persisted_count, 10);
    }

    #[test]
    fn test_process_events_reconciliation_passes() {
        let payload = make_converge_payload(95, 3, 2, 0);
        let result = process_payload(&payload).unwrap();
        assert!(result.stats.reconcile().is_ok());
    }

    #[test]
    fn test_process_events_empty_resources() {
        let payload = serde_json::json!({
            "run_id": "run-abc",
            "resources": []
        });
        let result = process_payload(&payload);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), PipelineError::EmptyResources);
    }

    #[test]
    fn test_process_events_unknown_status() {
        let payload = serde_json::json!({
            "resources": [
                {"name": "test-resource", "status": "borked"}
            ]
        });
        let result = process_payload(&payload);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            PipelineError::UnknownStatus(_)
        ));
    }

    #[test]
    fn test_extract_resource_events() {
        let payload = make_converge_payload(5, 3, 2, 1);
        let events = extract_resource_events(&payload).unwrap();
        assert_eq!(events.len(), 11);
    }

    #[test]
    fn test_run_stats_default() {
        let stats = RunResourceStats::default();
        assert_eq!(stats.total_resource_count, 0);
        assert!(stats.reconcile().is_ok());
    }

    #[test]
    fn test_run_stats_reconcile_fails_on_mismatch() {
        let stats = RunResourceStats {
            total_resource_count: 100,
            updated_count: 3,
            failed_count: 2,
            skipped_count: 0,
            up_to_date_count: 94,
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
            persisted_count: 4,
        };
        assert!(stats.reconcile().is_err());
    }

    #[test]
    fn test_reconciliation_invariant() {
        let payload = make_converge_payload(50, 20, 15, 15);
        let result = process_payload(&payload).unwrap();
        assert!(result.stats.is_consistent());
        assert!(result.stats.is_persisted_consistent());
    }

    #[test]
    fn test_count_reconciliation_formula() {
        let payload = make_converge_payload(70, 15, 10, 5);
        let result = process_payload(&payload).unwrap();
        let s = &result.stats;
        let computed = s.updated_count
            + s.failed_count
            + s.skipped_count
            + (s.total_resource_count - s.persisted_count);
        assert_eq!(computed, s.total_resource_count);
    }

    // ── M1-24: Unknown field preservation tests ───────────────────────────

    #[test]
    fn test_resource_event_extra_fields_preserved() {
        let payload = serde_json::json!({
            "name": "my-resource",
            "status": "updated",
            "cookbook": "apache2",
            "recipe": "default",
            "version": "2.0.0",
            "checksum": "abc123",
            "extra_metadata": { "foo": "bar" }
        });
        let event: ResourceEvent = serde_json::from_value(payload).unwrap();
        assert_eq!(event.name, "my-resource");
        assert_eq!(event.cookbook, Some("apache2".to_string()));
        let extra = event.extra_fields.as_object().unwrap();
        assert!(
            extra.contains_key("version"),
            "version should be in extra_fields"
        );
        assert!(
            extra.contains_key("checksum"),
            "checksum should be in extra_fields"
        );
        assert!(
            extra.contains_key("extra_metadata"),
            "extra_metadata should be in extra_fields"
        );
        assert_eq!(extra.get("version").unwrap(), "2.0.0");
    }

    #[test]
    fn test_control_result_extra_fields_preserved() {
        let payload = serde_json::json!({
            "status": "passed",
            "run_time": 0.1,
            "custom_field": "custom_value",
            "nested_extra": { "a": 1, "b": [1, 2, 3] }
        });
        let result: ControlResult = serde_json::from_value(payload).unwrap();
        assert_eq!(result.status, "passed");
        let extra = result.extra_fields.as_object().unwrap();
        assert!(extra.contains_key("custom_field"));
        assert!(extra.contains_key("nested_extra"));
        assert_eq!(extra.get("custom_field").unwrap(), "custom_value");
    }

    #[test]
    fn test_compliance_report_extra_fields_preserved() {
        let payload = serde_json::json!({
            "platform": { "name": "ubuntu", "release": "22.04" },
            "profiles": [],
            "custom_report_meta": "some_value",
            "run_context": { "chef_version": "18.0" }
        });
        let report = ComplianceReportParser::new().parse(&payload).unwrap();
        let extra = report.extra_fields.as_object().unwrap();
        assert!(extra.contains_key("custom_report_meta"));
        assert!(extra.contains_key("run_context"));
        assert_eq!(extra.get("custom_report_meta").unwrap(), "some_value");
    }

    #[test]
    fn test_parsed_resource_event_preserves_extra_fields() {
        let payload = serde_json::json!({
            "name": "pkg-foo",
            "status": "updated",
            "unknown_prop": true
        });
        let event: ResourceEvent = serde_json::from_value(payload).unwrap();
        let parsed = ParsedResourceEvent::from_event(event).unwrap();
        let extra = parsed.extra_fields.as_object().unwrap();
        assert!(extra.contains_key("unknown_prop"));
        assert_eq!(extra.get("unknown_prop").unwrap(), &serde_json::json!(true));
    }

    #[test]
    fn test_parsed_control_result_preserves_extra_fields() {
        let payload = serde_json::json!({
            "platform": { "name": "debian" },
            "profiles": [{
                "name": "test",
                "version": "1.0",
                "controls": [{
                    "id": "ctrl-1",
                    "title": "Test Control",
                    "impact": 0.5,
                    "tags": {},
                    "refs": [],
                    "results": [{
                        "status": "passed",
                        "run_time": 0.2,
                        "my_custom_field": 42,
                        "another_unknown": "value"
                    }]
                }]
            }]
        });
        let results = process_compliance_report(&payload).unwrap();
        assert!(!results.is_empty());
        let ctrl = &results[0];
        let extra = ctrl.extra_fields.as_object().unwrap();
        assert!(extra.contains_key("my_custom_field"));
        assert!(extra.contains_key("another_unknown"));
        assert_eq!(extra.get("my_custom_field").unwrap(), 42);
    }

    #[test]
    fn test_resource_event_no_extra_fields_when_known() {
        let payload = serde_json::json!({
            "name": "res",
            "status": "updated"
        });
        let event: ResourceEvent = serde_json::from_value(payload).unwrap();
        let extra = event.extra_fields.as_object().unwrap();
        assert!(
            extra.is_empty(),
            "known-only payload should have empty extra_fields"
        );
    }

    // ── M1-23: Compliance report parsing tests ──────────────────────────────

    #[test]
    fn test_auditor_status_display() {
        assert_eq!(AuditorStatus::Passed.to_string(), "passed");
        assert_eq!(AuditorStatus::Failed.to_string(), "failed");
        assert_eq!(AuditorStatus::Skipped.to_string(), "skipped");
        assert_eq!(AuditorStatus::Unknown.to_string(), "unknown");
    }

    #[test]
    fn test_parse_auditor_status_all_variants() {
        assert_eq!(parse_auditor_status("passed"), AuditorStatus::Passed);
        assert_eq!(parse_auditor_status("failed"), AuditorStatus::Failed);
        assert_eq!(parse_auditor_status("skipped"), AuditorStatus::Skipped);
        assert_eq!(parse_auditor_status("weird"), AuditorStatus::Unknown);
        assert_eq!(parse_auditor_status("PASSED"), AuditorStatus::Passed); // case insensitive
    }

    #[test]
    fn test_parse_compliance_report_valid() {
        let payload = make_auditor_report();
        let parser = ComplianceReportParser::new();
        let report = parser.parse(&payload).unwrap();

        assert_eq!(report.platform.name, "ubuntu");
        assert_eq!(report.profiles.len(), 1);
        assert_eq!(report.profiles[0].name, "linux-baseline");
        assert_eq!(report.profiles[0].version, Some("1.0.0".to_string()));
        assert_eq!(report.profiles[0].controls.len(), 4);
    }

    #[test]
    fn test_parse_compliance_report_statistics() {
        let payload = make_auditor_report();
        let parser = ComplianceReportParser::new();
        let report = parser.parse(&payload).unwrap();
        assert_eq!(
            report.statistics,
            Some(AuditorStatistics {
                duration: Some(1.5)
            })
        );
    }

    #[test]
    fn test_extract_control_results() {
        let payload = make_auditor_report();
        let parser = ComplianceReportParser::new();
        let report = parser.parse(&payload).unwrap();
        let results = parser.extract_control_results(&report);

        // 4 controls, each with 1 result = 4 control results
        assert_eq!(results.len(), 4);

        // ssh-01: passed
        assert_eq!(results[0].control_id, "ssh-01");
        assert_eq!(results[0].status, AuditorStatus::Passed);
        assert_eq!(results[0].title, Some("SSH Configuration".to_string()));
        assert_eq!(
            results[0].description,
            Some("SSH should be configured securely".to_string())
        );
        assert_eq!(results[0].impact, Some(1.0));
        assert!(results[0].code.is_some());
        assert_eq!(results[0].run_time, Some(0.05));
        assert_eq!(results[0].profile_name, "linux-baseline");
        assert_eq!(results[0].profile_version, Some("1.0.0".to_string()));

        // ssh-02: failed
        assert_eq!(results[1].control_id, "ssh-02");
        assert_eq!(results[1].status, AuditorStatus::Failed);
        assert!(results[1].message.is_some());

        // ssh-03: skipped
        assert_eq!(results[2].control_id, "ssh-03");
        assert_eq!(results[2].status, AuditorStatus::Skipped);
        assert!(results[2].skip_reason.is_some());

        // ssh-04: unknown
        assert_eq!(results[3].control_id, "ssh-04");
        assert_eq!(results[3].status, AuditorStatus::Unknown);
    }

    #[test]
    fn test_control_result_preserves_metadata() {
        let payload = make_auditor_report();
        let parser = ComplianceReportParser::new();
        let report = parser.parse(&payload).unwrap();
        let results = parser.extract_control_results(&report);

        let ssh01 = &results[0];
        assert_eq!(ssh01.title, Some("SSH Configuration".to_string()));
        assert_eq!(
            ssh01.description,
            Some("SSH should be configured securely".to_string())
        );
        assert_eq!(ssh01.impact, Some(1.0));
        assert!(ssh01.code.is_some());
        assert_eq!(ssh01.run_time, Some(0.05));
        assert_eq!(
            ssh01.start_time,
            Some("2024-01-01T00:00:00+00:00".to_string())
        );
        assert_eq!(ssh01.refs.len(), 1);
        assert_eq!(
            ssh01.refs[0].url,
            Some("https://example.com/ssh".to_string())
        );
        assert!(ssh01.source_location.is_some());
    }

    #[test]
    fn test_control_result_ref_fields() {
        let payload = make_auditor_report();
        let parser = ComplianceReportParser::new();
        let report = parser.parse(&payload).unwrap();
        let results = parser.extract_control_results(&report);

        let ssh01 = &results[0];
        assert_eq!(
            ssh01.refs[0].url,
            Some("https://example.com/ssh".to_string())
        );
    }

    #[test]
    fn test_control_result_source_location() {
        let payload = make_auditor_report();
        let parser = ComplianceReportParser::new();
        let report = parser.parse(&payload).unwrap();
        let results = parser.extract_control_results(&report);

        let ssh01 = &results[0];
        let loc = ssh01.source_location.as_ref().unwrap();
        assert_eq!(loc.ref_text, Some("controls/ssh.rb:10".to_string()));
    }

    #[test]
    fn test_process_compliance_report_convenience() {
        let payload = make_auditor_report();
        let results = process_compliance_report(&payload).unwrap();
        assert_eq!(results.len(), 4);
        assert_eq!(results[0].status, AuditorStatus::Passed);
        assert_eq!(results[1].status, AuditorStatus::Failed);
        assert_eq!(results[2].status, AuditorStatus::Skipped);
        assert_eq!(results[3].status, AuditorStatus::Unknown);
    }

    #[test]
    fn test_parse_compliance_report_no_profiles() {
        let payload = serde_json::json!({
            "platform": {"name": "ubuntu", "release": "22.04"},
            "profiles": []
        });
        let parser = ComplianceReportParser::new();
        let report = parser.parse(&payload).unwrap();
        assert!(report.profiles.is_empty());
        let results = parser.extract_control_results(&report);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_compliance_report_missing_platform() {
        let payload = serde_json::json!({
            "profiles": []
        });
        let parser = ComplianceReportParser::new();
        let result = parser.parse(&payload);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PipelineError::ParseError(_)));
    }

    #[test]
    fn test_parse_compliance_report_missing_profiles() {
        let payload = serde_json::json!({
            "platform": {"name": "ubuntu", "release": "22.04"}
        });
        let parser = ComplianceReportParser::new();
        let result = parser.parse(&payload);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), PipelineError::EmptyResources);
    }

    #[test]
    fn test_parsed_control_result_serialization() {
        let payload = make_auditor_report();
        let results = process_compliance_report(&payload).unwrap();
        let json = serde_json::to_string(&results[0]).unwrap();
        let deserialized: ParsedControlResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, results[0]);
    }

    // ── M1-25: Dead-letter queue tests ─────────────────────────────────────

    #[test]
    fn test_dead_letter_error_type_display() {
        assert_eq!(DeadLetterErrorType::ParseError.to_string(), "parse_error");
        assert_eq!(
            DeadLetterErrorType::ProcessingError.to_string(),
            "processing_error"
        );
        assert_eq!(
            DeadLetterErrorType::DbConstraintViolation.to_string(),
            "db_constraint_violation"
        );
        assert_eq!(DeadLetterErrorType::Panic.to_string(), "panic");
        assert_eq!(DeadLetterErrorType::Unknown.to_string(), "unknown");
    }

    #[test]
    fn test_dead_letter_entry_creation() {
        let entry = DeadLetterEntry::new(
            "archive/path/to/payload.json",
            "malformed JSON: unexpected token",
            DeadLetterErrorType::ParseError,
            0,
            Some("run-converge"),
            Some("web-server-01"),
            Some("run-abc-123"),
        );

        assert!(!entry.id.is_empty());
        assert_eq!(entry.archive_reference, "archive/path/to/payload.json");
        assert_eq!(entry.error_message, "malformed JSON: unexpected token");
        assert_eq!(entry.error_type, DeadLetterErrorType::ParseError);
        assert_eq!(entry.retry_count, 0);
        assert_eq!(entry.payload_type, Some("run-converge".to_string()));
        assert_eq!(entry.node_name, Some("web-server-01".to_string()));
        assert_eq!(entry.run_id, Some("run-abc-123".to_string()));
        assert!(entry.reprocessable);
        assert!(!entry.is_expired());
    }

    #[test]
    fn test_dead_letter_entry_serialization() {
        let entry = DeadLetterEntry::new(
            "archive/path.json",
            "parse error",
            DeadLetterErrorType::ParseError,
            2,
            Some("compliance-report"),
            None,
            None,
        );
        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: DeadLetterEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.archive_reference, entry.archive_reference);
        assert_eq!(deserialized.error_message, entry.error_message);
        assert_eq!(deserialized.error_type, entry.error_type);
        assert_eq!(deserialized.retry_count, entry.retry_count);
    }

    #[test]
    fn test_dead_letter_recording_and_listing() {
        let store = InMemoryDeadLetterStore::new();

        let entry1 = DeadLetterEntry::new(
            "archive/payload1.json",
            "parse error",
            DeadLetterErrorType::ParseError,
            0,
            Some("run-converge"),
            Some("node-1"),
            Some("run-1"),
        );
        let entry2 = DeadLetterEntry::new(
            "archive/payload2.json",
            "db constraint violation",
            DeadLetterErrorType::DbConstraintViolation,
            3,
            Some("compliance-report"),
            Some("node-2"),
            Some("run-2"),
        );

        store.record_failure(entry1.clone());
        store.record_failure(entry2.clone());

        let list = store.list_reprocessable();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_dead_letter_mark_permanent() {
        let store = InMemoryDeadLetterStore::new();
        let entry = DeadLetterEntry::new(
            "archive/payload.json",
            "permanent failure",
            DeadLetterErrorType::Panic,
            5,
            None,
            None,
            None,
        );
        store.record_failure(entry.clone());

        assert_eq!(store.list_reprocessable().len(), 1);

        store.mark_permanent(&entry.id);
        assert_eq!(store.list_reprocessable().len(), 0);
    }

    #[test]
    fn test_dead_letter_remove() {
        let store = InMemoryDeadLetterStore::new();
        let entry = DeadLetterEntry::new(
            "archive/payload.json",
            "failure",
            DeadLetterErrorType::ProcessingError,
            0,
            None,
            None,
            None,
        );
        store.record_failure(entry.clone());
        assert_eq!(store.list_reprocessable().len(), 1);

        store.remove(&entry.id);
        assert_eq!(store.list_reprocessable().len(), 0);
    }

    #[test]
    fn test_dead_letter_retry_succeeds() {
        let entry = DeadLetterEntry {
            id: "test-id".to_string(),
            archive_reference: "archive/payload.json".to_string(),
            error_message: "transient error".to_string(),
            error_type: DeadLetterErrorType::DbConstraintViolation,
            retry_count: 1,
            created_at: chrono::Utc::now().to_rfc3339(),
            payload_type: Some("run-converge".to_string()),
            node_name: None,
            run_id: None,
            reprocessable: true,
        };

        let result = attempt_retry(&entry, 5, || Ok(()));
        assert_eq!(result, RetryResult::Succeeded);
    }

    #[test]
    fn test_dead_letter_retry_fails_transient() {
        let entry = DeadLetterEntry {
            id: "test-id".to_string(),
            archive_reference: "archive/payload.json".to_string(),
            error_message: "transient error".to_string(),
            error_type: DeadLetterErrorType::DbConstraintViolation,
            retry_count: 0,
            created_at: chrono::Utc::now().to_rfc3339(),
            payload_type: Some("run-converge".to_string()),
            node_name: None,
            run_id: None,
            reprocessable: true,
        };

        let result = attempt_retry(&entry, 5, || Err("connection timeout".to_string()));
        assert_eq!(result, RetryResult::Failed { new_retry_count: 1 });
    }

    #[test]
    fn test_dead_letter_retry_permanent_failure() {
        let entry = DeadLetterEntry {
            id: "test-id".to_string(),
            archive_reference: "archive/payload.json".to_string(),
            error_message: "permanent error".to_string(),
            error_type: DeadLetterErrorType::ParseError,
            retry_count: 5,
            created_at: chrono::Utc::now().to_rfc3339(),
            payload_type: Some("run-converge".to_string()),
            node_name: None,
            run_id: None,
            reprocessable: true,
        };

        let result = attempt_retry(&entry, 5, || Ok(()));
        assert!(matches!(result, RetryResult::PermanentFailure { .. }));
    }

    #[test]
    fn test_dead_letter_retry_exhausted_after_max() {
        let entry = DeadLetterEntry {
            id: "test-id".to_string(),
            archive_reference: "archive/payload.json".to_string(),
            error_message: "error".to_string(),
            error_type: DeadLetterErrorType::ParseError,
            retry_count: 2,
            created_at: chrono::Utc::now().to_rfc3339(),
            payload_type: None,
            node_name: None,
            run_id: None,
            reprocessable: true,
        };

        // max_retries = 3, retry_count = 2, retry succeeds → Succeeded
        let result = attempt_retry(&entry, 3, || Ok(()));
        assert_eq!(result, RetryResult::Succeeded);

        // max_retries = 3, retry_count = 2, retry fails → permanent (2+1=3 >= 3)
        let entry2 = DeadLetterEntry {
            retry_count: 2,
            ..entry.clone()
        };
        let result2 = attempt_retry(&entry2, 3, || Err("fail".to_string()));
        assert!(matches!(result2, RetryResult::PermanentFailure { .. }));
    }

    #[test]
    fn test_dead_letter_admin_list_stub() {
        let store = InMemoryDeadLetterStore::new();

        for i in 0..5 {
            let entry = DeadLetterEntry::new(
                &format!("archive/payload_{}.json", i),
                "error",
                DeadLetterErrorType::ParseError,
                0,
                Some("run-converge"),
                Some(&format!("node-{}", i)),
                None,
            );
            store.record_failure(entry);
        }

        let list = admin_list_dead_letters(&store, Some(3));
        assert_eq!(list.len(), 3);

        let list_all = admin_list_dead_letters(&store, None);
        assert_eq!(list_all.len(), 5);
    }

    #[test]
    fn test_dead_letter_admin_reprocess_stub() {
        let entry = DeadLetterEntry {
            id: "test-id".to_string(),
            archive_reference: "archive/payload.json".to_string(),
            error_message: "error".to_string(),
            error_type: DeadLetterErrorType::ParseError,
            retry_count: 0,
            created_at: chrono::Utc::now().to_rfc3339(),
            payload_type: None,
            node_name: None,
            run_id: None,
            reprocessable: true,
        };

        let result = admin_reprocess_dead_letter(&entry, 3, || Ok(()));
        assert_eq!(result, RetryResult::Succeeded);
    }

    #[test]
    fn test_dead_letter_malformed_payload_lands_in_dlq() {
        let store = InMemoryDeadLetterStore::new();

        let payload = serde_json::json!({
            "run_id": "run-abc-123",
            "node_name": "web-server-01",
            "resources": [
                {"name": "test-resource", "status": "borked"}
            ]
        });

        let result = process_payload(&payload);
        assert!(result.is_err());

        let entry = DeadLetterEntry::new(
            "archive/malformed-payload.json",
            &format!("{}", result.unwrap_err()),
            DeadLetterErrorType::ParseError,
            0,
            Some("run-converge"),
            Some("web-server-01"),
            Some("run-abc-123"),
        );
        store.record_failure(entry.clone());

        let list = store.list_reprocessable();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].error_type, DeadLetterErrorType::ParseError);
        assert_eq!(list[0].node_name, Some("web-server-01".to_string()));
        assert_eq!(list[0].run_id, Some("run-abc-123".to_string()));
    }

    #[test]
    fn test_dead_letter_retention_period() {
        assert_eq!(DEAD_LETTER_RETENTION_SECONDS, 30 * 24 * 60 * 60);
    }

    #[test]
    fn test_dead_letter_expired_entry_not_listed() {
        let store = InMemoryDeadLetterStore::new();

        let old_time = (chrono::Utc::now() - chrono::Duration::days(40)).to_rfc3339();
        let entry = DeadLetterEntry {
            id: "old-entry".to_string(),
            archive_reference: "archive/old.json".to_string(),
            error_message: "old error".to_string(),
            error_type: DeadLetterErrorType::ParseError,
            retry_count: 0,
            created_at: old_time,
            payload_type: None,
            node_name: None,
            run_id: None,
            reprocessable: true,
        };
        store.record_failure(entry);

        assert_eq!(store.list_reprocessable().len(), 0);
    }

    #[test]
    fn test_dead_letter_store_trait_object_safe() {
        fn _accepts_store(_store: Box<dyn DeadLetterStore>) {}
        let store: Box<dyn DeadLetterStore> = Box::new(InMemoryDeadLetterStore::new());
        _accepts_store(store);
    }

    // ── M1-26: Schema version + cookbook usage tests ───────────────────────

    #[test]
    fn test_schema_version_is_one() {
        assert_eq!(SCHEMA_VERSION, 1);
    }

    #[test]
    fn test_schema_version_trait_default() {
        let usage = CookbookUsage::new("node-1", "run-1", "apache", "1.0.0");
        assert_eq!(usage.schema_version(), SCHEMA_VERSION);
        assert_eq!(usage.schema_version, 1);
    }

    #[test]
    fn test_extract_cookbook_usage_deduplication() {
        // 5 events, all using cookbook "test-cookbook" → 1 unique cookbook usage entry
        let payload = make_converge_payload(2, 2, 1, 0);
        let events = extract_resource_events(&payload).unwrap();
        let parsed: Vec<ParsedResourceEvent> = events
            .into_iter()
            .filter_map(ParsedResourceEvent::from_event)
            .collect();

        let usage = extract_cookbook_usage(&parsed, "node-1", "run-abc-123");
        assert_eq!(usage.len(), 1);
        let entry = &usage[0];
        assert_eq!(entry.cookbook_name, "test-cookbook");
        assert_eq!(entry.node_id, "node-1");
        assert_eq!(entry.run_id, "run-abc-123");
        assert_eq!(entry.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn test_extract_cookbook_usage_multiple_cookbooks() {
        let payload = serde_json::json!({
            "run_id": "run-abc",
            "node_name": "web-01",
            "resources": [
                {"name": "r1", "status": "updated", "cookbook": "apache", "recipe": "default"},
                {"name": "r2", "status": "updated", "cookbook": "nginx", "recipe": "default"},
                {"name": "r3", "status": "updated", "cookbook": "apache", "recipe": "default"},
                {"name": "r4", "status": "updated", "cookbook": "mysql", "recipe": "default"},
            ]
        });

        let events = extract_resource_events(&payload).unwrap();
        let parsed: Vec<ParsedResourceEvent> = events
            .into_iter()
            .filter_map(ParsedResourceEvent::from_event)
            .collect();

        let usage = extract_cookbook_usage(&parsed, "node-1", "run-abc");
        // 3 unique cookbooks: apache, nginx, mysql
        assert_eq!(usage.len(), 3);
        let names: Vec<&str> = usage.iter().map(|u| u.cookbook_name.as_str()).collect();
        assert!(names.contains(&"apache"));
        assert!(names.contains(&"nginx"));
        assert!(names.contains(&"mysql"));
        for entry in &usage {
            assert_eq!(entry.schema_version, SCHEMA_VERSION);
        }
    }

    #[test]
    fn test_extract_cookbook_usage_no_cookbook() {
        let payload = serde_json::json!({
            "run_id": "run-abc",
            "resources": [
                {"name": "r1", "status": "updated"},
                {"name": "r2", "status": "failed"},
            ]
        });

        let events = extract_resource_events(&payload).unwrap();
        let parsed: Vec<ParsedResourceEvent> = events
            .into_iter()
            .filter_map(ParsedResourceEvent::from_event)
            .collect();

        let usage = extract_cookbook_usage(&parsed, "node-1", "run-abc");
        assert!(usage.is_empty());
    }

    #[test]
    fn test_cookbook_usage_serialization() {
        let usage = CookbookUsage::new("node-1", "run-abc", "apache", "2.3.0");
        let json = serde_json::to_string(&usage).unwrap();
        let deserialized: CookbookUsage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.node_id, usage.node_id);
        assert_eq!(deserialized.cookbook_name, usage.cookbook_name);
        assert_eq!(deserialized.schema_version, usage.schema_version);
    }

    #[test]
    fn test_cookbook_usage_extracts_version_from_properties() {
        let event = ParsedResourceEvent {
            name: "test-resource".to_string(),
            status: ResourceStatus::Updated,
            cookbook: Some("apache".to_string()),
            recipe: Some("default".to_string()),
            properties: Some(serde_json::json!({
                "cookbook_version": "3.1.4",
                "path": "/etc/httpd.conf"
            })),
            extra_fields: Value::Null,
        };

        let usage = extract_cookbook_usage(&[event], "node-1", "run-1");
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].cookbook_version, "3.1.4");
    }

    #[test]
    fn test_process_payload_with_m1_21_95_3_2_scenario() {
        // Exact scenario from M1-21: 100 events (95 up-to-date, 3 updated, 2 failed)
        let payload = make_converge_payload(95, 3, 2, 0);
        let result = process_payload(&payload).unwrap();

        // resource_events should have 5 rows (3 updated + 2 failed)
        assert_eq!(result.persistable_events.len(), 5);
        assert_eq!(result.stats.total_resource_count, 100);
        assert_eq!(result.stats.updated_count, 3);
        assert_eq!(result.stats.failed_count, 2);
        assert_eq!(result.stats.skipped_count, 0);

        // Cookbook usage: only 1 cookbook ("test-cookbook"), deduplicated
        let parsed: Vec<ParsedResourceEvent> = result.persistable_events;
        let usage = extract_cookbook_usage(&parsed, "web-server-01", "run-abc-123");
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].cookbook_name, "test-cookbook");
        assert_eq!(usage[0].schema_version, 1);
    }

    #[test]
    fn test_process_payload_skips_up_to_date() {
        let payload = make_converge_payload(10, 5, 0, 3);
        let result = process_payload(&payload).unwrap();
        assert_eq!(result.persistable_events.len(), 8); // 5 updated + 3 skipped
        assert_eq!(result.stats.up_to_date_count, 10);
    }

    #[test]
    fn test_process_payload_empty_fails() {
        let payload = serde_json::json!({"resources": []});
        let result = process_payload(&payload);
        assert_eq!(result.unwrap_err(), PipelineError::EmptyResources);
    }

    #[test]
    fn test_admin_list_dead_letters_paginated() {
        let store = InMemoryDeadLetterStore::new();
        let now = chrono::Utc::now();
        let ts1 = (now - chrono::Duration::seconds(10)).to_rfc3339();
        let ts2 = now.to_rfc3339();

        store.record_failure(DeadLetterEntry {
            id: "fail-1".to_string(),
            archive_reference: "key1".to_string(),
            error_message: "bad json".to_string(),
            error_type: DeadLetterErrorType::ParseError,
            retry_count: 3,
            created_at: ts1,
            payload_type: Some("converge".to_string()),
            node_name: Some("node-1".to_string()),
            run_id: Some("run-1".to_string()),
            reprocessable: true,
        });
        store.record_failure(DeadLetterEntry {
            id: "fail-2".to_string(),
            archive_reference: "key2".to_string(),
            error_message: "processing failed".to_string(),
            error_type: DeadLetterErrorType::ProcessingError,
            retry_count: 3,
            created_at: ts2,
            payload_type: Some("converge".to_string()),
            node_name: Some("node-2".to_string()),
            run_id: Some("run-2".to_string()),
            reprocessable: true,
        });

        let list = admin_list_dead_letters(&store, Some(1));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "fail-2"); // newest first

        let all = admin_list_dead_letters(&store, None);
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_attempt_retry_succeeds_after_failure() {
        let entry = DeadLetterEntry {
            id: "test-retry".to_string(),
            archive_reference: "key".to_string(),
            error_message: "failed".to_string(),
            error_type: DeadLetterErrorType::ProcessingError,
            retry_count: 1,
            created_at: chrono::Utc::now().to_rfc3339(),
            payload_type: None,
            node_name: None,
            run_id: None,
            reprocessable: true,
        };

        let result = admin_reprocess_dead_letter(&entry, 3, || Ok(()));
        assert_eq!(result, RetryResult::Succeeded);

        let result2 = admin_reprocess_dead_letter(&entry, 3, || Err("still broken".to_string()));
        assert_eq!(result2, RetryResult::Failed { new_retry_count: 2 });
    }

    #[test]
    fn test_attempt_retry_permanent_failure() {
        let entry = DeadLetterEntry {
            id: "test-perm".to_string(),
            archive_reference: "key".to_string(),
            error_message: "perm fail".to_string(),
            error_type: DeadLetterErrorType::Unknown,
            retry_count: 3,
            created_at: chrono::Utc::now().to_rfc3339(),
            payload_type: None,
            node_name: None,
            run_id: None,
            reprocessable: false,
        };

        let result = admin_reprocess_dead_letter(&entry, 3, || Err("broken".to_string()));
        assert_eq!(
            result,
            RetryResult::PermanentFailure {
                error_message: "max retries exceeded".to_string()
            }
        );
    }

    #[test]
    #[cfg(feature = "worker")]
    fn test_pipeline_metrics_struct_exists() {
        let metrics = PipelineMetrics {
            processed_total: 0,
            error_total: 0,
            avg_latency_ms: 0.0,
        };
        assert_eq!(metrics.processed_total, 0);
        assert_eq!(metrics.error_total, 0);
        assert_eq!(metrics.avg_latency_ms, 0.0);
    }
}
