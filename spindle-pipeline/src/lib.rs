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
    #[serde(default)]
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
///
/// **Schema evolution**: When the InSpec JSON reporter adds new top-level fields
/// (new metadata, new top-level keys), add them to this struct and create a
/// migration that adds the corresponding columns. Until the migration is applied,
/// unrecognized fields are captured in `extra_fields` below.
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
    /// All unrecognized fields from the InSpec JSON reporter payload.
    #[serde(flatten)]
    pub extra_fields: Value,
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

        // Capture unrecognized top-level fields for schema evolution
        let known_keys = ["platform", "profiles", "statistics", "version", "organization"];
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
        assert!(extra.contains_key("version"), "version should be in extra_fields");
        assert!(extra.contains_key("checksum"), "checksum should be in extra_fields");
        assert!(extra.contains_key("extra_metadata"), "extra_metadata should be in extra_fields");
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
        assert!(extra.is_empty(), "known-only payload should have empty extra_fields");
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
