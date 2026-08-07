//! spindle-pipeline: Parse + normalize Chef data-collector and InSpec payloads.
//!
//! Parses Cinc/Chef `data-collector` JSON into typed structs, normalizes
//! timestamps to UTC, maps status strings to enums, extracts resource events
//! with action/status classification, and detects no-op resources.
//!
//! Three parser modes:
//! - `RunStartParser`: initial run metadata (node identity, timestamp, run list)
//! - `RunConvergeParser`: resource management results
//! - `ComplianceReportParser`: InSpec/audit run results
//!
//! Pipeline trait: `fn process(payload) -> Result<ProcessedRun>`.
//! No raw SQL — pure parse + normalize. DB operations are in `spindle-store`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

// ── Resource status ──────────────────────────────────────────────────────

/// Status of a Chef Infra resource event.
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
    pub name: String,
    #[serde(rename = "status")]
    pub status: String,
    #[serde(default)]
    pub cookbook: Option<String>,
    #[serde(default)]
    pub recipe: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Value>,
}

/// Parsed resource event with typed status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedResourceEvent {
    pub name: String,
    pub status: ResourceStatus,
    pub cookbook: Option<String>,
    pub recipe: Option<String>,
    pub properties: Option<Value>,
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

// ── Pipeline processing ──────────────────────────────────────────────────

/// Result of pipeline processing: filtered events to persist + run statistics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PipelineResult {
    pub persistable_events: Vec<ParsedResourceEvent>,
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
    #[error("parse error: {0}")]
    ParseError(String),
}

/// Process a list of resource events from a normalized Chef run-converge payload.
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
    stats.reconcile()?;

    Ok(PipelineResult {
        persistable_events,
        stats,
    })
}

/// Extract resource events from a normalized Chef run-converge JSON payload.
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
pub fn process_payload(
    payload: &Value,
) -> Result<PipelineResult, PipelineError> {
    let events = extract_resource_events(payload)?;
    process_resource_events(events)
}

// ── Compliance report parsing (InSpec) ───────────────────────────────────

/// Status of an InSpec control result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum InSpecStatus {
    Passed,
    Failed,
    Skipped,
    /// Any status not recognized by the parser.
    Unknown,
}

impl std::fmt::Display for InSpecStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InSpecStatus::Passed => write!(f, "passed"),
            InSpecStatus::Failed => write!(f, "failed"),
            InSpecStatus::Skipped => write!(f, "skipped"),
            InSpecStatus::Unknown => write!(f, "unknown"),
        }
    }
}

/// Parse an InSpec status string into `InSpecStatus`.
///
/// InSpec JSON reporter uses: "passed", "failed", "skipped".
/// Any other string maps to `Unknown`.
pub fn parse_inspec_status(s: &str) -> InSpecStatus {
    match s.to_lowercase().as_str() {
        "passed" => InSpecStatus::Passed,
        "failed" => InSpecStatus::Failed,
        "skipped" => InSpecStatus::Skipped,
        _ => InSpecStatus::Unknown,
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

/// A single control result from an InSpec profile.
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
}

/// A control definition from an InSpec profile.
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

/// An InSpec profile within a compliance report.
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

/// Statistics from the InSpec run.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct InSpecStatistics {
    #[serde(default)]
    pub duration: Option<f64>,
}

/// A parsed InSpec compliance report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceReport {
    pub platform: Platform,
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub statistics: Option<InSpecStatistics>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub organization: Option<String>,
}

/// Parser for InSpec compliance report JSON.
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
            .and_then(|s| serde_json::from_value::<InSpecStatistics>(s.clone()).ok());

        let version = payload
            .get("version")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let organization = payload
            .get("organization")
            .and_then(|o| o.as_str())
            .map(|s| s.to_string());

        Ok(ComplianceReport {
            platform,
            profiles,
            statistics,
            version,
            organization,
        })
    }

    /// Extract all control results from a parsed compliance report.
    ///
    /// Returns a flat list of `ParsedControlResult` entries, one per
    /// control result in every profile.
    pub fn extract_control_results(&self, report: &ComplianceReport) -> Vec<ParsedControlResult> {
        let mut results = Vec::new();
        for profile in &report.profiles {
            for control in &profile.controls {
                for result in &control.results {
                    results.push(ParsedControlResult {
                        control_id: control.id.clone(),
                        status: parse_inspec_status(&result.status),
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
                    });
                }
            }
        }
        results
    }
}

/// A control result with typed status, ready for insertion into `control_results` table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParsedControlResult {
    pub control_id: String,
    pub status: InSpecStatus,
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
}

/// Convenience: parse + extract control results in one call.
pub fn process_compliance_report(
    payload: &Value,
) -> Result<Vec<ParsedControlResult>, PipelineError> {
    let parser = ComplianceReportParser::new();
    let report = parser.parse(payload)?;
    Ok(parser.extract_control_results(&report))
}

// ── Dead-letter queue ────────────────────────────────────────────────────

/// Types of errors that can cause a payload to be sent to the dead-letter queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DeadLetterErrorType {
    /// Unrecoverable parse error (malformed JSON, schema violation).
    ParseError,
    /// Pipeline processing failure (reconciliation, validation).
    ProcessingError,
    /// Database constraint violation after all retries exhausted.
    DbConstraintViolation,
    /// Panic during processing.
    Panic,
    /// Unknown or unexpected error.
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
///
/// Tracks enough information to diagnose and optionally reprocess later.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadLetterEntry {
    /// Unique identifier for this dead-letter entry.
    pub id: String,
    /// Archive reference to the original payload (e.g., archive path or receipt token).
    pub archive_reference: String,
    /// The error message explaining why processing failed.
    pub error_message: String,
    /// Categorized error type for metric labels.
    pub error_type: DeadLetterErrorType,
    /// Number of times this payload was retried before landing in dead-letter.
    pub retry_count: u32,
    /// When the dead-letter entry was created (UTC ISO 8601).
    pub created_at: String,
    /// Original payload type (e.g., "run-converge", "compliance-report").
    #[serde(default)]
    pub payload_type: Option<String>,
    /// Node name from the payload, if extractable.
    #[serde(default)]
    pub node_name: Option<String>,
    /// Run ID from the payload, if extractable.
    #[serde(default)]
    pub run_id: Option<String>,
    /// Whether this entry is eligible for reprocessing (not permanently failed).
    #[serde(default)]
    pub reprocessable: bool,
}

impl DeadLetterEntry {
    /// Create a new dead-letter entry.
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

    /// Age of this dead-letter entry in seconds.
    pub fn age_seconds(&self) -> i64 {
        let created = chrono::DateTime::parse_from_rfc3339(&self.created_at)
            .map(|dt| dt.timestamp())
            .unwrap_or(0);
        chrono::Utc::now().timestamp() - created
    }

    /// Whether this entry has exceeded the retention period (30 days).
    pub fn is_expired(&self) -> bool {
        self.age_seconds() > DEAD_LETTER_RETENTION_SECONDS
    }
}

/// Default dead-letter retention period: 30 days in seconds.
pub const DEAD_LETTER_RETENTION_SECONDS: i64 = 30 * 24 * 60 * 60; // 2,592,000

/// Trait for dead-letter queue storage backends.
/// Implementations should be thread-safe and persist entries durably.
pub trait DeadLetterStore: Send + Sync + std::fmt::Debug {
    /// Record a failed payload in the dead-letter queue.
    fn record_failure(&self, entry: DeadLetterEntry);

    /// Retrieve dead-letter entries eligible for retry (within retention).
    fn list_reprocessable(&self) -> Vec<DeadLetterEntry>;

    /// Mark an entry as permanently failed (no longer retryable).
    fn mark_permanent(&self, id: &str);

    /// Remove an expired or resolved entry from the dead-letter queue.
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
        let mut store = self.inner.lock().unwrap();
        store.push(entry);
    }

    fn list_reprocessable(&self) -> Vec<DeadLetterEntry> {
        let store = self.inner.lock().unwrap();
        store
            .iter()
            .filter(|e| e.reprocessable && !e.is_expired())
            .cloned()
            .collect()
    }

    fn mark_permanent(&self, id: &str) {
        let mut store = self.inner.lock().unwrap();
        for entry in store.iter_mut() {
            if entry.id == id {
                entry.reprocessable = false;
            }
        }
    }

    fn remove(&self, id: &str) {
        let mut store = self.inner.lock().unwrap();
        store.retain(|e| e.id != id);
    }
}

/// Result of a retry attempt for a dead-letter entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryResult {
    /// Retry succeeded — entry should be removed from dead-letter queue.
    Succeeded,
    /// Retry failed — increment retry count, keep in queue.
    Failed { new_retry_count: u32 },
    /// Retry failed permanently — mark as non-reprocessable.
    PermanentFailure { error_message: String },
}

/// Attempt to reprocess a dead-letter entry.
///
/// On success → `RetryResult::Succeeded` (caller should remove from DLQ).
/// On transient failure → `RetryResult::Failed` (increment retry count).
/// After max retries → `RetryResult::PermanentFailure` (mark non-reprocessable).
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
                RetryResult::PermanentFailure {
                    error_message: e,
                }
            } else {
                RetryResult::Failed {
                    new_retry_count: new_count,
                }
            }
        }
    }
}

/// Stub for admin list endpoint — lists dead-letter entries.
///
/// In production, this would be exposed as an HTTP endpoint (e.g., via
/// spindle-api) requiring admin authentication. For now, it provides
/// the data layer only.
pub fn admin_list_dead_letters(
    store: &dyn DeadLetterStore,
    limit: Option<usize>,
) -> Vec<DeadLetterEntry> {
    let mut entries = store.list_reprocessable();
    // Sort by creation time (newest first)
    entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    if let Some(n) = limit {
        entries.truncate(n);
    }
    entries
}

/// Stub for admin reprocess endpoint — attempts to reprocess a dead-letter entry.
///
/// Returns the retry result. The caller is responsible for updating the store
/// (remove on success, update retry count on failure, mark permanent on
/// permanent failure).
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

    /// Build a sample InSpec compliance report payload.
    fn make_inspec_report() -> Value {
        serde_json::json!({
            "platform": {
                "name": "ubuntu",
                "release": "22.04"
            },
            "profiles": [
                {
                    "name": "linux-baseline",
                    "version": "1.0.0",
                    "sha256": "abc123def456",
                    "controls": [
                        {
                            "id": "ssh-01",
                            "title": "SSH Configuration",
                            "description": "SSH should be configured securely",
                            "impact": 1.0,
                            "tags": {"severity": "high"},
                            "refs": [{"url": "https://example.com/ssh"}],
                            "source_location": {"ref": "controls/ssh.rb:10"},
                            "code": "describe sshd_config do\n  it { should exist }\nend",
                            "results": [
                                {
                                    "status": "passed",
                                    "code": "describe sshd_config do\n  it { should exist }\nend",
                                    "run_time": 0.05,
                                    "start_time": "2024-01-01T00:00:00+00:00"
                                }
                            ]
                        },
                        {
                            "id": "ssh-02",
                            "title": "SSH Port",
                            "impact": 0.5,
                            "results": [
                                {
                                    "status": "failed",
                                    "code": "describe port(22) do\n  it { should_not be_listening }\nend",
                                    "run_time": 0.03,
                                    "start_time": "2024-01-01T00:00:01+00:00",
                                    "message": "expected port(22) not to be listening"
                                }
                            ]
                        },
                        {
                            "id": "ssh-03",
                            "title": "SSH Root Login",
                            "impact": 0.7,
                            "results": [
                                {
                                    "status": "skipped",
                                    "skip_reason": "Not applicable on this system"
                                }
                            ]
                        },
                        {
                            "id": "ssh-04",
                            "title": "Unknown Status Test",
                            "results": [
                                {
                                    "status": "weird",
                                    "code": "describe something do\n  it { should exist }\nend"
                                }
                            ]
                        }
                    ]
                }
            ],
            "statistics": {
                "duration": 1.5
            },
            "version": "5.21.0",
            "organization": "test-org"
        })
    }

    // ── InSpec status tests ──────────────────────────────────────────────

    #[test]
    fn test_parse_inspec_status_all_variants() {
        assert_eq!(parse_inspec_status("passed"), InSpecStatus::Passed);
        assert_eq!(parse_inspec_status("failed"), InSpecStatus::Failed);
        assert_eq!(parse_inspec_status("skipped"), InSpecStatus::Skipped);
        assert_eq!(parse_inspec_status("PASSED"), InSpecStatus::Passed); // case insensitive
        assert_eq!(parse_inspec_status("weird"), InSpecStatus::Unknown);
        assert_eq!(parse_inspec_status(""), InSpecStatus::Unknown);
    }

    #[test]
    fn test_inspec_status_display() {
        assert_eq!(InSpecStatus::Passed.to_string(), "passed");
        assert_eq!(InSpecStatus::Failed.to_string(), "failed");
        assert_eq!(InSpecStatus::Skipped.to_string(), "skipped");
        assert_eq!(InSpecStatus::Unknown.to_string(), "unknown");
    }

    // ── ComplianceReportParser tests ─────────────────────────────────────

    #[test]
    fn test_parse_compliance_report_valid() {
        let payload = make_inspec_report();
        let parser = ComplianceReportParser::new();
        let report = parser.parse(&payload).unwrap();

        assert_eq!(report.platform.name, "ubuntu");
        assert_eq!(report.platform.release, Some("22.04".to_string()));
        assert_eq!(report.profiles.len(), 1);
        assert_eq!(report.profiles[0].name, "linux-baseline");
        assert_eq!(report.profiles[0].version, Some("1.0.0".to_string()));
        assert_eq!(report.profiles[0].sha256, Some("abc123def456".to_string()));
        assert_eq!(report.version, Some("5.21.0".to_string()));
        assert_eq!(report.organization, Some("test-org".to_string()));
    }

    #[test]
    fn test_parse_compliance_report_statistics() {
        let payload = make_inspec_report();
        let parser = ComplianceReportParser::new();
        let report = parser.parse(&payload).unwrap();
        assert_eq!(report.statistics, Some(InSpecStatistics { duration: Some(1.5) }));
    }

    #[test]
    fn test_extract_control_results() {
        let payload = make_inspec_report();
        let parser = ComplianceReportParser::new();
        let report = parser.parse(&payload).unwrap();
        let results = parser.extract_control_results(&report);

        // 4 controls, each with 1 result = 4 control results
        assert_eq!(results.len(), 4);

        // ssh-01: passed
        assert_eq!(results[0].control_id, "ssh-01");
        assert_eq!(results[0].status, InSpecStatus::Passed);
        assert_eq!(results[0].title, Some("SSH Configuration".to_string()));
        assert_eq!(results[0].description, Some("SSH should be configured securely".to_string()));
        assert_eq!(results[0].impact, Some(1.0));
        assert!(results[0].code.is_some());
        assert_eq!(results[0].run_time, Some(0.05));
        assert_eq!(results[0].profile_name, "linux-baseline");
        assert_eq!(results[0].profile_version, Some("1.0.0".to_string()));

        // ssh-02: failed
        assert_eq!(results[1].control_id, "ssh-02");
        assert_eq!(results[1].status, InSpecStatus::Failed);
        assert!(results[1].message.is_some());

        // ssh-03: skipped
        assert_eq!(results[2].control_id, "ssh-03");
        assert_eq!(results[2].status, InSpecStatus::Skipped);
        assert!(results[2].skip_reason.is_some());

        // ssh-04: unknown
        assert_eq!(results[3].control_id, "ssh-04");
        assert_eq!(results[3].status, InSpecStatus::Unknown);
    }

    #[test]
    fn test_control_result_preserves_metadata() {
        let payload = make_inspec_report();
        let parser = ComplianceReportParser::new();
        let report = parser.parse(&payload).unwrap();
        let results = parser.extract_control_results(&report);

        let ssh01 = &results[0];
        // Verify all metadata fields are preserved
        assert_eq!(ssh01.title, Some("SSH Configuration".to_string()));
        assert_eq!(ssh01.description, Some("SSH should be configured securely".to_string()));
        assert_eq!(ssh01.impact, Some(1.0));
        assert!(ssh01.code.is_some());
        assert_eq!(ssh01.run_time, Some(0.05));
        assert_eq!(ssh01.start_time, Some("2024-01-01T00:00:00+00:00".to_string()));
        assert_eq!(ssh01.refs.len(), 1);
        assert_eq!(ssh01.refs[0].url, Some("https://example.com/ssh".to_string()));
        assert!(ssh01.source_location.is_some());
    }

    #[test]
    fn test_control_result_ref_fields() {
        let payload = make_inspec_report();
        let parser = ComplianceReportParser::new();
        let report = parser.parse(&payload).unwrap();
        let results = parser.extract_control_results(&report);

        let ssh01 = &results[0];
        assert_eq!(ssh01.refs[0].url, Some("https://example.com/ssh".to_string()));
    }

    #[test]
    fn test_control_result_source_location() {
        let payload = make_inspec_report();
        let parser = ComplianceReportParser::new();
        let report = parser.parse(&payload).unwrap();
        let results = parser.extract_control_results(&report);

        let ssh01 = &results[0];
        let loc = ssh01.source_location.as_ref().unwrap();
        assert_eq!(loc.ref_text, Some("controls/ssh.rb:10".to_string()));
    }

    #[test]
    fn test_process_compliance_report_convenience() {
        let payload = make_inspec_report();
        let results = process_compliance_report(&payload).unwrap();
        assert_eq!(results.len(), 4);
        assert_eq!(results[0].status, InSpecStatus::Passed);
        assert_eq!(results[1].status, InSpecStatus::Failed);
        assert_eq!(results[2].status, InSpecStatus::Skipped);
        assert_eq!(results[3].status, InSpecStatus::Unknown);
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
        let payload = make_inspec_report();
        let results = process_compliance_report(&payload).unwrap();
        // Verify we can serialize back to JSON
        let json = serde_json::to_string(&results[0]).unwrap();
        let deserialized: ParsedControlResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, results[0]);
    }

    // ── Existing resource event tests (unchanged) ─────────────────────────

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

    #[test]
    fn test_process_events_mixed_statuses() {
        let payload = make_converge_payload(95, 3, 2, 0);
        let result = process_payload(&payload).unwrap();
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
        assert!(matches!(result.unwrap_err(), PipelineError::UnknownStatus(_)));
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
        let computed = s.updated_count + s.failed_count + s.skipped_count
            + (s.total_resource_count - s.persisted_count);
        assert_eq!(computed, s.total_resource_count);
    }

    // ── Dead-letter queue tests (M1-25) ─────────────────────────────────────

    #[test]
    fn test_dead_letter_error_type_display() {
        assert_eq!(DeadLetterErrorType::ParseError.to_string(), "parse_error");
        assert_eq!(DeadLetterErrorType::ProcessingError.to_string(), "processing_error");
        assert_eq!(DeadLetterErrorType::DbConstraintViolation.to_string(), "db_constraint_violation");
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

        // Initially reprocessable
        assert_eq!(store.list_reprocessable().len(), 1);

        // Mark as permanent
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
            retry_count: 5, // already at max_retries, next attempt triggers permanent
            created_at: chrono::Utc::now().to_rfc3339(),
            payload_type: Some("run-converge".to_string()),
            node_name: None,
            run_id: None,
            reprocessable: true,
        };

        // retry_count >= max_retries → immediate permanent failure
        let result = attempt_retry(&entry, 5, || Ok(()));
        if let RetryResult::PermanentFailure { error_message } = result {
            assert_eq!(error_message, "max retries exceeded");
        } else {
            panic!("expected PermanentFailure, got {:?}", result);
        }
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
            created_at: chrono::Utc::now().to_rfc3339(),
            ..entry.clone()
        };
        let result2 = attempt_retry(&entry2, 3, || Err("fail".to_string()));
        if let RetryResult::PermanentFailure { error_message } = result2 {
            assert_eq!(error_message, "fail");
        } else {
            panic!("expected PermanentFailure, got {:?}", result2);
        }
    }

    #[test]
    fn test_dead_letter_admin_list_stub() {
        let store = InMemoryDeadLetterStore::new();

        // Record a few entries
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

        // List with limit
        let list = admin_list_dead_letters(&store, Some(3));
        assert_eq!(list.len(), 3);

        // List without limit
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
        // Simulate: malformed payload → processing fails → lands in dead letter
        let store = InMemoryDeadLetterStore::new();

        // The payload has a resource with an unrecognized status
        let payload = serde_json::json!({
            "run_id": "run-abc-123",
            "node_name": "web-server-01",
            "resources": [
                {"name": "test-resource", "status": "borked"}
            ]
        });

        let result = process_payload(&payload);
        assert!(result.is_err());

        // Record the failure in dead-letter queue
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

        // Verify it's in the DLQ
        let list = store.list_reprocessable();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].error_type, DeadLetterErrorType::ParseError);
        assert_eq!(list[0].node_name, Some("web-server-01".to_string()));
        assert_eq!(list[0].run_id, Some("run-abc-123".to_string()));
    }

    #[test]
    fn test_dead_letter_retention_period() {
        // Verify the retention constant is 30 days
        assert_eq!(DEAD_LETTER_RETENTION_SECONDS, 30 * 24 * 60 * 60);
    }

    #[test]
    fn test_dead_letter_expired_entry_not_listed() {
        let store = InMemoryDeadLetterStore::new();

        // Create an entry with an old timestamp (40 days ago)
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

        // Expired entries should not appear in reprocessable list
        assert_eq!(store.list_reprocessable().len(), 0);
    }

    #[test]
    fn test_dead_letter_store_trait_object_safe() {
        // Verify DeadLetterStore trait can be used as trait object
        fn _accepts_store(_store: Box<dyn DeadLetterStore>) {}
        let store: Box<dyn DeadLetterStore> = Box::new(InMemoryDeadLetterStore::new());
        _accepts_store(store);
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
