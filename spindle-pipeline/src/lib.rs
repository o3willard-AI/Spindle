//! spindle-pipeline: Parse + normalize Chef data-collector payloads.
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
use chrono::{DateTime, Utc};
use thiserror::Error;
use std::collections::HashMap;

// ── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Error, PartialEq)]
pub enum PipelineError {
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Missing required field: {0}")]
    MissingField(String),
    #[error("Unknown payload type")]
    UnknownPayloadType,
    #[error("Normalization failed: {0}")]
    NormalizationError(String),
}

pub type Result<T> = std::result::Result<T, PipelineError>;

// ── Payload type detection ──────────────────────────────────────────────────

/// Which kind of Chef data-collector payload we received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayloadType {
    /// Run start: node identity + run list (first request in a converges).
    RunStart,
    /// Run converge: resource management results.
    RunConverge,
    /// Compliance report: InSpec or audit results.
    ComplianceReport,
}

impl PayloadType {
    /// Detect payload type by examining JSON structure, not Content-Type.
    /// Fallback to RunConverge if ambiguous.
    pub fn detect(value: &Value) -> Result<Self> {
        match value.get("type") {
            Some(Value::String(t)) => match t.as_str() {
                "run_start" => Ok(PayloadType::RunStart),
                "run_converge" => Ok(PayloadType::RunConverge),
                "compliance_report" => Ok(PayloadType::ComplianceReport),
                _ => Err(PipelineError::UnknownPayloadType),
            },
            // Fallback heuristics for version differences:
            _ if value.get("run_type").is_some() => Ok(PayloadType::RunStart),
            _ if value.get("resources").is_some() => Ok(PayloadType::RunConverge),
            _ if value.get("controls").is_some() => Ok(PayloadType::ComplianceReport),
            _ if value.get("report_type").is_some() => Ok(PayloadType::ComplianceReport),
            _ => Ok(PayloadType::RunConverge), // default
        }
    }
}

// ── Normalized enums ────────────────────────────────────────────────────────

/// Resource status after normalization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceStatus {
    Updated,
    Failed,
    Skipped,
    UpToDate,
}

impl ResourceStatus {
    /// Map string value from Chef payload to our enum.
    /// Handles version variations (different capitalizations, spellings).
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().trim() {
            "updated" | "changed" | "updated_or_restarted" => Ok(ResourceStatus::Updated),
            "failed" | "error" | "failure" => Ok(ResourceStatus::Failed),
            "skipped" | "skip" => Ok(ResourceStatus::Skipped),
            "up-to-date" | "uptodate" | "up_to_date" | "skipped_by_guard" => Ok(ResourceStatus::UpToDate),
            _ => Err(PipelineError::ParseError(format!(
                "unrecognized resource status: '{}'", s
            ))),
        }
    }

    /// Whether this is a no-op (unchanged resource).
    pub fn is_noop(&self) -> bool {
        matches!(self, ResourceStatus::UpToDate)
    }
}

/// Resource action classification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceAction {
    Create,
    Delete,
    Modify,
    Noop,
    Other(String),
}

impl ResourceAction {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().trim() {
            "create" => ResourceAction::Create,
            "delete" => ResourceAction::Delete,
            "modify" | "change" => ResourceAction::Modify,
            "noop" | "no-op" => ResourceAction::Noop,
            _ => ResourceAction::Other(s.to_string()),
        }
    }
}

/// Run status after normalization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunStatus {
    Running,
    Succeeded,
    Failed,
    Partial,
    Other(String),
}

impl RunStatus {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().trim() {
            "success" | "succeeded" | "complete" => RunStatus::Succeeded,
            "failed" | "error" => RunStatus::Failed,
            "partial" | "partial_failure" => RunStatus::Partial,
            "running" | "in_progress" => RunStatus::Running,
            other => RunStatus::Other(other.to_string()),
        }
    }
}

// ── Run Start ───────────────────────────────────────────────────────────────

/// Parsed + normalized run start payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedRunStart {
    pub timestamp: DateTime<Utc>,
    pub node_name: String,
    pub platform_name: String,
    pub platform_version: String,
    pub platform_family: Option<String>,
    pub chef_version: String,
    pub run_list: Vec<String>,
    pub run_id: Option<String>,
    pub node_id: Option<String>,
    pub data_collector_token: Option<String>,
    pub data_collector_endpoint: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub extra: Value,
}

impl ParsedRunStart {
    /// Parse a raw JSON value into a typed ParsedRunStart.
    pub fn parse(value: &Value) -> Result<Self> {
        let get_str = |key: &str| -> Result<String> {
            value
                .get(key)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .ok_or_else(|| PipelineError::MissingField(key.to_string()))
        };

        let get_str_opt = |key: &str| -> Option<String> {
            value
                .get(key)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };

        let parse_ts = |key: &str| -> Result<DateTime<Utc>> {
            match value.get(key).and_then(|v| v.as_str()) {
                Some(s) => chrono::DateTime::parse_from_rfc3339(s)
                    .map(|dt| dt.with_timezone(&Utc))
                    .map_err(|_| PipelineError::ParseError(format!("bad timestamp: {}", key))),
                None => Err(PipelineError::MissingField(key.to_string())),
            }
        };

        let run_list = value
            .get("run_list")
            .or_else(|| value.get("run_list_members")) // version variation
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        // Platform family from platform hash (Chef version variation)
        let platform_family: Option<String> = match value.get("platform") {
            Some(Value::Object(map)) => map.get("family").and_then(|v| v.as_str()).map(|s| s.to_string()),
            Some(Value::String(_)) => None,
            _ => None,
        };

        Ok(ParsedRunStart {
            timestamp: parse_ts("timestamp")?,
            node_name: get_str("node_name")?,
            platform_name: match value.get("platform") {
                Some(Value::Object(map)) => {
                    map.get("name").and_then(|v| v.as_str()).unwrap_or("unknown").to_string()
                }
                Some(Value::String(s)) => s.clone(),
                _ => "unknown".to_string(),
            },
            platform_version: match value.get("platform") {
                Some(Value::Object(map)) => {
                    map.get("version").and_then(|v| v.as_str()).unwrap_or("unknown").to_string()
                }
                Some(Value::String(_)) => value.get("platform_version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                _ => "unknown".to_string(),
            },
            platform_family,
            chef_version: get_str_opt("chef_version")
                .or_else(|| get_str_opt("chef_client_version"))
                .unwrap_or("unknown".to_string()),
            run_list,
            run_id: get_str_opt("run_id"),
            node_id: get_str_opt("node_id"),
            data_collector_token: get_str_opt("data_collector_token"),
            data_collector_endpoint: get_str_opt("data_collector_endpoint"),
            started_at: parse_ts("started_at").ok(),
            extra: value.clone(),
        })
    }
}

// ── Resource Event ──────────────────────────────────────────────────────────

/// Single resource management result extracted from a converge payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceEvent {
    pub resource_type: String,
    pub resource_name: String,
    pub action: ResourceAction,
    pub status: ResourceStatus,
    pub duration_ms: i64,
    pub cookbook_name: String,
    pub cookbook_version: Option<String>,
    pub guard_outcome: Option<Value>,
    pub delta: Option<Value>,
    pub previous_state: Option<Value>,
    pub new_state: Option<Value>,
    pub metadata: Value,
    pub is_noop: bool,
}

impl ResourceEvent {
    /// Extract resource events from a run_converge payload's resource array.
    pub fn extract_from_array(resources: &[Value]) -> Vec<Self> {
        resources.iter().map(|r| Self::from_resource(r)).collect()
    }

    fn from_resource(value: &Value) -> Self {
        let get_str = |key: &str| -> String {
            value
                .get(key)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };

        let get_i64 = |key: &str| -> i64 {
            value
                .get(key)
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
        };

        let get_value = |key: &str| -> Option<Value> {
            value.get(key).cloned()
        };

        // Status: many possible field names across Chef versions
        let status_str = value
            .get("status")
            .or_else(|| value.get("result"))
            .or_else(|| value.get("outcome"))
            .and_then(|v| v.as_str())
            .unwrap_or("up-to-date");

        let status = ResourceStatus::from_str(status_str).unwrap_or(ResourceStatus::UpToDate);

        // Action: detect from property changes or explicit field
        let action = if let Some(Value::Object(properties)) = value.get("new_properties") {
            let prev = match value.get("previous_properties") {
                Some(Value::Object(p)) => p,
                _ => &serde_json::Map::new(),
            };
            if prev.is_empty() && properties.is_empty() {
                ResourceAction::Noop
            } else if !prev.is_empty() && !properties.is_empty() {
                ResourceAction::Modify
            } else if prev.is_empty() && !properties.is_empty() {
                ResourceAction::Create
            } else {
                ResourceAction::Delete
            }
        } else {
            let action_str = value
                .get("action")
                .or_else(|| value.get("action_name"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            ResourceAction::from_str(action_str)
        };

        // Guard outcome
        let guard_outcome = value.get("guard").and_then(|g| {
            if let Value::Bool(true) = g {
                Some(Value::String("guarded".to_string()))
            } else {
                None
            }
        });

        let is_noop = status.is_noop() || matches!(action, ResourceAction::Noop);

        ResourceEvent {
            resource_type: get_str("resource_type"),
            resource_name: get_str("resource_name"),
            action,
            status,
            duration_ms: get_i64("duration"),
            cookbook_name: get_str("cookbook_name"),
            cookbook_version: value.get("cookbook_version").and_then(|v| v.as_str()).map(|s| s.to_string()),
            guard_outcome,
            delta: value.get("delta").cloned(),
            previous_state: get_value("previous_properties"),
            new_state: get_value("new_properties"),
            metadata: get_value("metadata").unwrap_or(Value::Null),
            is_noop,
        }
    }
}

// ── Run Converge ────────────────────────────────────────────────────────────

/// Parsed + normalized run converge payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedRunConverge {
    pub timestamp: DateTime<Utc>,
    pub node_name: String,
    pub run_id: Option<String>,
    pub node_id: Option<String>,
    pub platform_name: String,
    pub platform_version: String,
    pub chef_version: String,
    pub run_list: Vec<String>,
    pub run_status: RunStatus,
    pub total_resource_count: i32,
    pub updated_count: i32,
    pub failed_count: i32,
    pub skipped_count: i32,
    pub resources: Vec<ResourceEvent>,
    pub error_summary: Option<Value>,
    pub cookbook_set: Option<Value>,
    pub started_at: Option<DateTime<Utc>>,
    pub elapsed_seconds: Option<f64>,
    pub extra: Value,
}

impl ParsedRunConverge {
    /// Parse a raw JSON value into a typed ParsedRunConverge.
    pub fn parse(value: &Value) -> Result<Self> {
        let get_str = |key: &str| -> Result<String> {
            value
                .get(key)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .ok_or_else(|| PipelineError::MissingField(key.to_string()))
        };

        let get_str_opt = |key: &str| -> Option<String> {
            value
                .get(key)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };

        let parse_ts = |key: &str| -> Result<DateTime<Utc>> {
            match value.get(key).and_then(|v| v.as_str()) {
                Some(s) => chrono::DateTime::parse_from_rfc3339(s)
                    .map(|dt| dt.with_timezone(&Utc))
                    .map_err(|_| PipelineError::ParseError(format!("bad timestamp: {}", key))),
                None => Err(PipelineError::MissingField(key.to_string())),
            }
        };

        let parse_ts_opt = |key: &str| -> Option<DateTime<Utc>> {
            match value.get(key).and_then(|v| v.as_str()) {
                Some(s) => chrono::DateTime::parse_from_rfc3339(s)
                    .ok()
                    .map(|dt| dt.with_timezone(&Utc)),
                None => None,
            }
        };

        let run_list = value
            .get("run_list")
            .or_else(|| value.get("run_list_members"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        // Extract resource events
        let raw_resources = value
            .get("resources")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|r| serde_json::to_value(r).ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let resources = ResourceEvent::extract_from_array(&raw_resources);

        // Compute counts from extracted events
        let total = resources.len() as i32;
        let updated = resources.iter().filter(|r| matches!(r.status, ResourceStatus::Updated)).count() as i32;
        let failed = resources.iter().filter(|r| matches!(r.status, ResourceStatus::Failed)).count() as i32;
        let skipped = resources.iter().filter(|r| matches!(r.status, ResourceStatus::Skipped)).count() as i32;

        // Also count up-to-date resources that might not appear in the resources array
        let total_from_report = value.get("total_resources")
            .or_else(|| value.get("resource_counts"))
            .and_then(|v| v.as_i64())
            .unwrap_or(total as i64) as i32;
        let total_resource_count = if total_from_report > 0 { total_from_report } else { total };

        // Platform info
        let (platform_name, platform_version) = match value.get("platform") {
            Some(Value::Object(map)) => (
                map.get("name").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
                map.get("version").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
            ),
            Some(Value::String(s)) => (s.clone(), "unknown".to_string()),
            _ => ("unknown".to_string(), "unknown".to_string()),
        };

        Ok(ParsedRunConverge {
            timestamp: parse_ts("timestamp")?,
            node_name: get_str("node_name")?,
            run_id: get_str_opt("run_id"),
            node_id: get_str_opt("node_id"),
            platform_name,
            platform_version,
            chef_version: get_str_opt("chef_version")
                .or_else(|| get_str_opt("chef_client_version"))
                .unwrap_or("unknown".to_string()),
            run_list,
            run_status: RunStatus::from_str(
                &value.get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("failed")
                    .to_string(),
            ),
            total_resource_count,
            updated_count: updated,
            failed_count: failed,
            skipped_count: skipped,
            resources,
            error_summary: value.get("error").or_else(|| value.get("failures")).cloned(),
            cookbook_set: value.get("cookbooks").cloned(),
            started_at: parse_ts_opt("started_at"),
            elapsed_seconds: value.get("elapsed").and_then(|v| v.as_f64()),
            extra: value.clone(),
        })
    }
}

// ── Compliance Report ───────────────────────────────────────────────────────

/// A single control result extracted from a compliance report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlResult {
    pub control_id: String,
    pub title: String,
    pub status: String,
    pub impact: f64,
    pub message: Option<String>,
    pub code_desc: Option<String>,
    pub source: Option<String>,
    pub resource_type: Option<String>,
    pub resource_name: Option<String>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub run_time: Option<f64>,
    pub tags: Value,
    pub extra: Value,
}

/// Parsed + normalized compliance report payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedComplianceReport {
    pub timestamp: DateTime<Utc>,
    pub node_name: String,
    pub run_id: Option<String>,
    pub node_id: Option<String>,
    pub platform_name: String,
    pub platform_version: String,
    pub chef_version: String,
    pub report_type: String,
    pub status: String,
    pub passed_count: i32,
    pub failed_count: i32,
    pub warning_count: i32,
    pub skipped_count: i32,
    pub controls: Vec<ControlResult>,
    pub started_at: Option<DateTime<Utc>>,
    pub extra: Value,
}

impl ParsedComplianceReport {
    /// Parse a raw JSON value into a typed ParsedComplianceReport.
    pub fn parse(value: &Value) -> Result<Self> {
        let get_str = |key: &str| -> Result<String> {
            value
                .get(key)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .ok_or_else(|| PipelineError::MissingField(key.to_string()))
        };

        let get_str_opt = |key: &str| -> Option<String> {
            value
                .get(key)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };

        let parse_ts = |key: &str| -> Result<DateTime<Utc>> {
            match value.get(key).and_then(|v| v.as_str()) {
                Some(s) => chrono::DateTime::parse_from_rfc3339(s)
                    .map(|dt| dt.with_timezone(&Utc))
                    .map_err(|_| PipelineError::ParseError(format!("bad timestamp: {}", key))),
                None => Err(PipelineError::MissingField(key.to_string())),
            }
        };

        let parse_ts_opt = |key: &str| -> Option<DateTime<Utc>> {
            match value.get(key).and_then(|v| v.as_str()) {
                Some(s) => chrono::DateTime::parse_from_rfc3339(s)
                    .ok()
                    .map(|dt| dt.with_timezone(&Utc)),
                None => None,
            }
        };

        // Controls from different payload structures:
        // - InSpec: `controls` array
        // - Audit: `resources[].controls`
        let controls: Vec<ControlResult> = if let Some(Value::Array(arr)) = value.get("controls") {
            arr.iter()
                .filter_map(|c| serde_json::to_value(c).ok())
                .map(ControlResult::from_value)
                .collect()
        } else if let Some(Value::Array(resources)) = value.get("resources") {
            resources
                .iter()
                .filter_map(|r| r.get("controls").and_then(|c| c.as_array()))
                .flatten()
                .filter_map(|c| serde_json::to_value(c).ok())
                .map(ControlResult::from_value)
                .collect()
        } else {
            Vec::new()
        };

        // Platform info
        let (platform_name, platform_version) = match value.get("platform") {
            Some(Value::Object(map)) => (
                map.get("name").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
                map.get("version").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
            ),
            Some(Value::String(s)) => (s.clone(), "unknown".to_string()),
            _ => ("unknown".to_string(), "unknown".to_string()),
        };

        Ok(ParsedComplianceReport {
            timestamp: parse_ts("timestamp")?,
            node_name: get_str("node_name")?,
            run_id: get_str_opt("run_id"),
            node_id: get_str_opt("node_id"),
            platform_name,
            platform_version,
            chef_version: get_str_opt("chef_version")
                .or_else(|| get_str_opt("chef_client_version"))
                .unwrap_or("unknown".to_string()),
            report_type: value.get("report_type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            status: value.get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            passed_count: value.get("passed").and_then(|v| v.as_i64()).unwrap_or(controls.iter().filter(|c| c.status == "passed").count() as i64) as i32,
            failed_count: value.get("failed").and_then(|v| v.as_i64()).unwrap_or(controls.iter().filter(|c| c.status == "failed").count() as i64) as i32,
            warning_count: value.get("warning").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
            skipped_count: value.get("skipped").and_then(|v| v.as_i64()).unwrap_or(controls.iter().filter(|c| c.status == "skipped").count() as i64) as i32,
            controls,
            started_at: parse_ts_opt("started_at"),
            extra: value.clone(),
        })
    }
}

impl ControlResult {
    fn from_value(value: Value) -> Self {
        let get_str = |key: &str| -> String {
            value.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
        };
        let get_f64 = |key: &str| -> Option<f64> {
            value.get(key).and_then(|v| v.as_f64())
        };
        let get_opt_str = |key: &str| -> Option<String> {
            value.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
        };

        ControlResult {
            control_id: value
                .get("id")
                .and_then(|v| v.as_str())
                .or_else(|| value.get("control_id").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string(),
            title: get_str("title"),
            status: get_str("status"),
            impact: value.get("impact").and_then(|v| v.as_f64()).unwrap_or(0.0),
            message: get_opt_str("message"),
            code_desc: get_opt_str("code_desc"),
            source: get_opt_str("source"),
            resource_type: get_opt_str("resource_type"),
            resource_name: get_opt_str("resource_name"),
            start_time: get_opt_str("start_time"),
            end_time: get_opt_str("end_time"),
            run_time: get_f64("run_time"),
            tags: value.get("tags").unwrap_or(&Value::Null).clone(),
            extra: value,
        }
    }
}

// ── ProcessedRun — pipeline output ──────────────────────────────────────────

/// Pipeline output: the fully parsed + normalized run representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessedRun {
    pub node_name: String,
    pub node_id: Option<String>,
    pub platform_name: String,
    pub platform_version: String,
    pub chef_version: String,
    pub run_list: Vec<String>,
    pub run_id: Option<String>,
    pub run_status: RunStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub timestamp: DateTime<Utc>,
    pub total_resource_count: i32,
    pub updated_count: i32,
    pub failed_count: i32,
    pub skipped_count: i32,
    pub resource_events: Vec<ResourceEvent>,
    pub control_results: Vec<ControlResult>,
    pub is_noop: bool,
}

impl ProcessedRun {
    fn from_converge(p: &ParsedRunConverge) -> Self {
        let updated = p.resources.iter().filter(|r| matches!(r.status, ResourceStatus::Updated)).count() as i32;
        let failed = p.resources.iter().filter(|r| matches!(r.status, ResourceStatus::Failed)).count() as i32;
        let skipped = p.resources.iter().filter(|r| matches!(r.status, ResourceStatus::Skipped)).count() as i32;

        ProcessedRun {
            node_name: p.node_name.clone(),
            node_id: p.node_id.clone(),
            platform_name: p.platform_name.clone(),
            platform_version: p.platform_version.clone(),
            chef_version: p.chef_version.clone(),
            run_list: p.run_list.clone(),
            run_id: p.run_id.clone(),
            run_status: p.run_status.clone(),
            started_at: p.started_at,
            timestamp: p.timestamp,
            total_resource_count: p.total_resource_count,
            updated_count: updated,
            failed_count: failed,
            skipped_count: skipped,
            resource_events: p.resources.clone(),
            control_results: Vec::new(),
            is_noop: p.resources.iter().all(|r| r.is_noop),
        }
    }

    fn from_compliance(p: &ParsedComplianceReport) -> Self {
        ProcessedRun {
            node_name: p.node_name.clone(),
            node_id: p.node_id.clone(),
            platform_name: p.platform_name.clone(),
            platform_version: p.platform_version.clone(),
            chef_version: p.chef_version.clone(),
            run_list: Vec::new(),
            run_id: p.run_id.clone(),
            run_status: RunStatus::from_str(&p.status),
            started_at: p.started_at,
            timestamp: p.timestamp,
            total_resource_count: 0,
            updated_count: 0,
            failed_count: p.failed_count,
            skipped_count: p.skipped_count,
            resource_events: Vec::new(),
            control_results: p.controls.clone(),
            is_noop: p.failed_count == 0,
        }
    }
}

// ── Pipeline trait ──────────────────────────────────────────────────────────

/// Main pipeline interface: parse payload → typed ProcessedRun.
pub trait Pipeline {
    /// Process a raw JSON payload into a ProcessedRun.
    ///
    /// Steps:
    /// 1. Detect payload type (run_start, run_converge, compliance_report)
    /// 2. Parse into typed struct
    /// 3. Normalize timestamps, statuses, actions
    /// 4. Extract resource events
    /// 5. Classify no-ops
    fn process(&self, payload: &Value) -> Result<ProcessedRun>;
}

/// The standard pipeline implementation.
pub struct StandardPipeline;

impl StandardPipeline {
    pub fn new() -> Self {
        Self
    }
}

impl Pipeline for StandardPipeline {
    fn process(&self, payload: &Value) -> Result<ProcessedRun> {
        let ptype = PayloadType::detect(payload)?;

        let result = match ptype {
            PayloadType::RunStart => {
                // Run start is typically followed by run converge;
                // just normalize node identity from start.
                let ps = ParsedRunStart::parse(payload)?;
                // Return a minimal ProcessedRun with node identity;
                // resource counts will come from the subsequent converge.
                ProcessedRun {
                    node_name: ps.node_name,
                    node_id: ps.node_id,
                    platform_name: ps.platform_name,
                    platform_version: ps.platform_version,
                    chef_version: ps.chef_version,
                    run_list: ps.run_list,
                    run_id: ps.run_id,
                    run_status: RunStatus::Running,
                    started_at: ps.started_at,
                    timestamp: ps.timestamp,
                    total_resource_count: 0,
                    updated_count: 0,
                    failed_count: 0,
                    skipped_count: 0,
                    resource_events: Vec::new(),
                    control_results: Vec::new(),
                    is_noop: false,
                }
            }
            PayloadType::RunConverge => {
                let pr = ParsedRunConverge::parse(payload)?;
                ProcessedRun::from_converge(&pr)
            }
            PayloadType::ComplianceReport => {
                let pr = ParsedComplianceReport::parse(payload)?;
                ProcessedRun::from_compliance(&pr)
            }
        };

        Ok(result)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_run_converge() -> Value {
        serde_json::json!({
            "timestamp": "2024-01-15T10:30:00Z",
            "node_name": "web-server-01",
            "run_id": "abc-123",
            "platform": {
                "name": "ubuntu",
                "version": "22.04",
                "family": "debian"
            },
            "chef_version": "18.1.0",
            "run_list": ["recipe[apache]", "recipe[php]"],
            "status": "success",
            "total_resources": 5,
            "elapsed": 42.5,
            "resources": [
                {
                    "resource_type": "package",
                    "resource_name": "apache2",
                    "action": "install",
                    "status": "updated",
                    "duration": 15000,
                    "cookbook_name": "apache",
                    "cookbook_version": "2.0",
                    "previous_properties": {},
                    "new_properties": {"version": "2.4.57"}
                },
                {
                    "resource_type": "file",
                    "resource_name": "/etc/apache2/apache2.conf",
                    "action": "create",
                    "status": "updated",
                    "duration": 500,
                    "cookbook_name": "apache",
                    "cookbook_version": "2.0",
                    "previous_properties": {},
                    "new_properties": {"content": "..."}
                },
                {
                    "resource_type": "service",
                    "resource_name": "apache2",
                    "action": "enable",
                    "status": "up-to-date",
                    "duration": 100,
                    "cookbook_name": "apache",
                    "cookbook_version": "2.0"
                },
                {
                    "resource_type": "package",
                    "resource_name": "php",
                    "action": "install",
                    "status": "failed",
                    "duration": 5000,
                    "cookbook_name": "php",
                    "cookbook_version": "1.0",
                    "error": "dependency not found"
                },
                {
                    "resource_type": "user",
                    "resource_name": "webuser",
                    "action": "create",
                    "status": "skipped",
                    "duration": 0,
                    "cookbook_name": "users",
                    "cookbook_version": "1.0",
                    "guard": true
                }
            ]
        })
    }

    fn sample_compliance_report() -> Value {
        serde_json::json!({
            "timestamp": "2024-01-15T10:35:00Z",
            "node_name": "web-server-01",
            "run_id": "abc-123",
            "platform": {
                "name": "ubuntu",
                "version": "22.04"
            },
            "chef_version": "18.1.0",
            "report_type": "inspec",
            "status": "passed",
            "passed": 3,
            "failed": 1,
            "skipped": 1,
            "controls": [
                {
                    "id": "sshd-port",
                    "title": "SSH port should not be 22",
                    "status": "passed",
                    "impact": 1.0,
                    "code_desc": "port is 2222",
                    "tags": {"severity": "high"}
                },
                {
                    "id": "firewall-enabled",
                    "title": "Firewall should be enabled",
                    "status": "failed",
                    "impact": 1.0,
                    "message": "ufw is not running",
                    "tags": {"severity": "critical"}
                },
                {
                    "id": "ntp-configured",
                    "title": "NTP should be configured",
                    "status": "passed",
                    "impact": 0.5,
                    "run_time": 0.023,
                    "tags": {}
                },
                {
                    "id": "logrotate-configured",
                    "title": "Logrotate should be configured",
                    "status": "skipped",
                    "impact": 0.3,
                    "message": "no log files found",
                    "tags": {}
                }
            ]
        })
    }

    fn sample_run_start() -> Value {
        serde_json::json!({
            "type": "run_start",
            "timestamp": "2024-01-15T10:29:00Z",
            "node_name": "web-server-01",
            "run_id": "abc-123",
            "node_id": "node-001",
            "platform": {
                "name": "ubuntu",
                "version": "22.04",
                "family": "debian"
            },
            "chef_version": "18.1.0",
            "run_list": ["recipe[apache]", "recipe[php]"],
            "started_at": "2024-01-15T10:29:00Z",
            "data_collector_endpoint": "http://collector.local:443",
            "data_collector_token": "secret"
        })
    }

    // ── Payload type detection ───────────────────────────────────────────

    #[test]
    fn test_detect_run_converge() {
        let json = sample_run_converge();
        assert_eq!(PayloadType::detect(&json).unwrap(), PayloadType::RunConverge);
    }

    #[test]
    fn test_detect_run_start() {
        let json = sample_run_start();
        assert_eq!(PayloadType::detect(&json).unwrap(), PayloadType::RunStart);
    }

    #[test]
    fn test_detect_compliance_report() {
        let json = sample_compliance_report();
        assert_eq!(PayloadType::detect(&json).unwrap(), PayloadType::ComplianceReport);
    }

    #[test]
    fn test_detect_explicit_type() {
        let json = serde_json::json!({"type": "run_start"});
        assert_eq!(PayloadType::detect(&json).unwrap(), PayloadType::RunStart);

        let json = serde_json::json!({"type": "run_converge"});
        assert_eq!(PayloadType::detect(&json).unwrap(), PayloadType::RunConverge);

        let json = serde_json::json!({"type": "compliance_report"});
        assert_eq!(PayloadType::detect(&json).unwrap(), PayloadType::ComplianceReport);
    }

    #[test]
    fn test_detect_unknown_type() {
        let json = serde_json::json!({"type": "unknown_type"});
        assert_eq!(PayloadType::detect(&json).unwrap_err(), PipelineError::UnknownPayloadType);
    }

    // ── Status normalization ─────────────────────────────────────────────

    #[test]
    fn test_status_from_str_updated() {
        assert_eq!(ResourceStatus::from_str("updated").unwrap(), ResourceStatus::Updated);
        assert_eq!(ResourceStatus::from_str("changed").unwrap(), ResourceStatus::Updated);
    }

    #[test]
    fn test_status_from_str_failed() {
        assert_eq!(ResourceStatus::from_str("failed").unwrap(), ResourceStatus::Failed);
        assert_eq!(ResourceStatus::from_str("error").unwrap(), ResourceStatus::Failed);
    }

    #[test]
    fn test_status_from_str_skipped() {
        assert_eq!(ResourceStatus::from_str("skipped").unwrap(), ResourceStatus::Skipped);
    }

    #[test]
    fn test_status_from_str_up_to_date() {
        assert_eq!(ResourceStatus::from_str("up-to-date").unwrap(), ResourceStatus::UpToDate);
        assert_eq!(ResourceStatus::from_str("uptodate").unwrap(), ResourceStatus::UpToDate);
    }

    #[test]
    fn test_status_from_str_invalid() {
        assert!(ResourceStatus::from_str("garbage").is_err());
    }

    #[test]
    fn test_noop_detection() {
        assert!(ResourceStatus::UpToDate.is_noop());
        assert!(!ResourceStatus::Updated.is_noop());
        assert!(!ResourceStatus::Failed.is_noop());
    }

    // ── Run start parsing ────────────────────────────────────────────────

    #[test]
    fn test_parse_run_start() {
        let parsed = ParsedRunStart::parse(&sample_run_start()).unwrap();
        assert_eq!(parsed.node_name, "web-server-01");
        assert_eq!(parsed.run_list, vec!["recipe[apache]", "recipe[php]"]);
        assert_eq!(parsed.chef_version, "18.1.0");
        assert_eq!(parsed.platform_name, "ubuntu");
        assert_eq!(parsed.platform_version, "22.04");
        assert_eq!(parsed.platform_family, Some("debian".to_string()));
        assert_eq!(parsed.run_id, Some("abc-123".to_string()));
        assert_eq!(parsed.node_id, Some("node-001".to_string()));
        assert!(parsed.started_at.is_some());
    }

    #[test]
    fn test_parse_run_start_missing_timestamp() {
        let json = sample_run_start();
        let mut json_obj = json.as_object().unwrap().clone();
        json_obj.remove("timestamp");
        let json = Value::Object(json_obj);
        assert!(ParsedRunStart::parse(&json).is_err());
    }

    #[test]
    fn test_parse_run_start_missing_node_name() {
        let json = sample_run_start();
        let mut json_obj = json.as_object().unwrap().clone();
        json_obj.remove("node_name");
        let json = Value::Object(json_obj);
        assert!(ParsedRunStart::parse(&json).is_err());
    }

    // ── Resource event extraction ────────────────────────────────────────

    #[test]
    fn test_extract_resource_events() {
        let converge = ParsedRunConverge::parse(&sample_run_converge()).unwrap();
        assert_eq!(converge.resources.len(), 5);

        let pkg = &converge.resources[0];
        assert_eq!(pkg.resource_type, "package");
        assert_eq!(pkg.resource_name, "apache2");
        assert_eq!(pkg.status, ResourceStatus::Updated);
        assert_eq!(pkg.duration_ms, 15000);
        assert_eq!(pkg.cookbook_name, "apache");

        let svc = &converge.resources[2];
        assert!(svc.is_noop);
        assert_eq!(svc.status, ResourceStatus::UpToDate);

        let php = &converge.resources[3];
        assert_eq!(php.status, ResourceStatus::Failed);
    }

    #[test]
    fn test_resource_event_noop() {
        let converge = ParsedRunConverge::parse(&sample_run_converge()).unwrap();
        // Only up-to-date resource should be a noop
        let noop_count = converge.resources.iter().filter(|r| r.is_noop).count();
        assert_eq!(noop_count, 1); // only the service up-to-date
    }

    #[test]
    fn test_resource_event_with_guard() {
        let converge = ParsedRunConverge::parse(&sample_run_converge()).unwrap();
        let user = &converge.resources[4];
        assert_eq!(user.status, ResourceStatus::Skipped);
    }

    // ── Run converge parsing ─────────────────────────────────────────────

    #[test]
    fn test_parse_run_converge() {
        let parsed = ParsedRunConverge::parse(&sample_run_converge()).unwrap();
        assert_eq!(parsed.node_name, "web-server-01");
        assert_eq!(parsed.run_list, vec!["recipe[apache]", "recipe[php]"]);
        assert_eq!(parsed.run_status, RunStatus::Succeeded);
        assert_eq!(parsed.total_resource_count, 5);
        assert_eq!(parsed.resources.len(), 5);
        assert_eq!(parsed.platform_name, "ubuntu");
        assert_eq!(parsed.chef_version, "18.1.0");
        assert!(parsed.started_at.is_none()); // not in converge sample
        assert_eq!(parsed.elapsed_seconds, Some(42.5));
    }

    #[test]
    fn test_parse_run_converge_counts() {
        let parsed = ParsedRunConverge::parse(&sample_run_converge()).unwrap();
        // 2 updated (apache pkg, file), 1 failed (php pkg), 1 skipped (user guard)
        assert_eq!(parsed.updated_count, 2);
        assert_eq!(parsed.failed_count, 1);
        assert_eq!(parsed.skipped_count, 1);
    }

    #[test]
    fn test_parse_run_converge_missing_timestamp() {
        let json = sample_run_converge();
        let mut json_obj = json.as_object().unwrap().clone();
        json_obj.remove("timestamp");
        let json = Value::Object(json_obj);
        assert!(ParsedRunConverge::parse(&json).is_err());
    }

    #[test]
    fn test_parse_run_converge_missing_node_name() {
        let json = sample_run_converge();
        let mut json_obj = json.as_object().unwrap().clone();
        json_obj.remove("node_name");
        let json = Value::Object(json_obj);
        assert!(ParsedRunConverge::parse(&json).is_err());
    }

    #[test]
    fn test_parse_run_converge_empty_resources() {
        let json = serde_json::json!({
            "timestamp": "2024-01-15T10:30:00Z",
            "node_name": "test-node",
            "status": "success",
            "resources": []
        });
        let parsed = ParsedRunConverge::parse(&json).unwrap();
        assert_eq!(parsed.resources.len(), 0);
        assert_eq!(parsed.total_resource_count, 0);
    }

    #[test]
    fn test_parse_run_converge_null_resources() {
        let json = serde_json::json!({
            "timestamp": "2024-01-15T10:30:00Z",
            "node_name": "test-node",
            "status": "success",
            "resources": null
        });
        let parsed = ParsedRunConverge::parse(&json).unwrap();
        assert_eq!(parsed.resources.len(), 0);
    }

    // ── Compliance report parsing ────────────────────────────────────────

    #[test]
    fn test_parse_compliance_report() {
        let parsed = ParsedComplianceReport::parse(&sample_compliance_report()).unwrap();
        assert_eq!(parsed.node_name, "web-server-01");
        assert_eq!(parsed.controls.len(), 4);
        assert_eq!(parsed.passed_count, 3);
        assert_eq!(parsed.failed_count, 1);
        assert_eq!(parsed.skipped_count, 1); // actually 1 skipped control
        assert_eq!(parsed.platform_name, "ubuntu");
        assert_eq!(parsed.report_type, "inspec");
    }

    #[test]
    fn test_compliance_control_result() {
        let parsed = ParsedComplianceReport::parse(&sample_compliance_report()).unwrap();

        let sshd = &parsed.controls[0];
        assert_eq!(sshd.control_id, "sshd-port");
        assert_eq!(sshd.status, "passed");
        assert_eq!(sshd.impact, 1.0);
        assert_eq!(sshd.code_desc, Some("port is 2222".to_string()));

        let fw = &parsed.controls[1];
        assert_eq!(fw.control_id, "firewall-enabled");
        assert_eq!(fw.status, "failed");
        assert_eq!(fw.message, Some("ufw is not running".to_string()));
    }

    #[test]
    fn test_parse_compliance_report_missing_timestamp() {
        let json = sample_compliance_report();
        let mut json_obj = json.as_object().unwrap().clone();
        json_obj.remove("timestamp");
        let json = Value::Object(json_obj);
        assert!(ParsedComplianceReport::parse(&json).is_err());
    }

    #[test]
    fn test_parse_compliance_report_missing_node_name() {
        let json = sample_compliance_report();
        let mut json_obj = json.as_object().unwrap().clone();
        json_obj.remove("node_name");
        let json = Value::Object(json_obj);
        assert!(ParsedComplianceReport::parse(&json).is_err());
    }

    #[test]
    fn test_parse_compliance_report_no_controls() {
        let json = serde_json::json!({
            "timestamp": "2024-01-15T10:35:00Z",
            "node_name": "test-node",
            "status": "passed"
        });
        let parsed = ParsedComplianceReport::parse(&json).unwrap();
        assert_eq!(parsed.controls.len(), 0);
    }

    // ── Pipeline trait ───────────────────────────────────────────────────

    #[test]
    fn test_pipeline_process_run_converge() {
        let pipeline = StandardPipeline::new();
        let result = pipeline.process(&sample_run_converge()).unwrap();

        assert_eq!(result.node_name, "web-server-01");
        assert_eq!(result.run_id, Some("abc-123".to_string()));
        assert_eq!(result.run_status, RunStatus::Succeeded);
        assert_eq!(result.total_resource_count, 5);
        assert_eq!(result.updated_count, 2);
        assert_eq!(result.failed_count, 1);
        assert_eq!(result.skipped_count, 1);
        assert_eq!(result.resource_events.len(), 5);

        // Not all events are noops — 2 updated, 1 failed, 1 skipped
        assert!(!result.is_noop);
    }

    #[test]
    fn test_pipeline_process_compliance_report() {
        let pipeline = StandardPipeline::new();
        let result = pipeline.process(&sample_compliance_report()).unwrap();

        assert_eq!(result.node_name, "web-server-01");
        assert_eq!(result.control_results.len(), 4);
        assert_eq!(result.resource_events.len(), 0);
    }

    #[test]
    fn test_pipeline_process_run_start() {
        let pipeline = StandardPipeline::new();
        let result = pipeline.process(&sample_run_start()).unwrap();

        assert_eq!(result.node_name, "web-server-01");
        assert_eq!(result.run_status, RunStatus::Running);
        assert_eq!(result.resource_events.len(), 0);
        assert_eq!(result.run_list, vec!["recipe[apache]", "recipe[php]"]);
    }

    #[test]
    fn test_pipeline_unknown_payload_type() {
        let pipeline = StandardPipeline::new();
        let json = serde_json::json!({"type": "garbage"});
        assert!(pipeline.process(&json).is_err());
    }

    #[test]
    fn test_pipeline_missing_timestamp() {
        let pipeline = StandardPipeline::new();
        let json = serde_json::json!({"type": "run_converge", "node_name": "test"});
        let err = pipeline.process(&json).unwrap_err();
        assert!(matches!(err, PipelineError::MissingField(_)));
    }

    // ── Run status normalization ─────────────────────────────────────────

    #[test]
    fn test_run_status_from_str() {
        assert_eq!(RunStatus::from_str("success"), RunStatus::Succeeded);
        assert_eq!(RunStatus::from_str("failed"), RunStatus::Failed);
        assert_eq!(RunStatus::from_str("partial_failure"), RunStatus::Partial);
        assert_eq!(RunStatus::from_str("running"), RunStatus::Running);
        assert!(matches!(RunStatus::from_str("custom"), RunStatus::Other(_)));
    }

    // ── Action classification ────────────────────────────────────────────

    #[test]
    fn test_resource_action_from_str() {
        assert_eq!(ResourceAction::from_str("create"), ResourceAction::Create);
        assert_eq!(ResourceAction::from_str("delete"), ResourceAction::Delete);
        assert_eq!(ResourceAction::from_str("modify"), ResourceAction::Modify);
        assert_eq!(ResourceAction::from_str("noop"), ResourceAction::Noop);
        assert!(matches!(ResourceAction::from_str("custom"), ResourceAction::Other(_)));
    }

    // ── Extra fields preservation ────────────────────────────────────────

    #[test]
    fn test_extra_fields_preserved() {
        let json = sample_run_converge();
        let parsed = ParsedRunConverge::parse(&json).unwrap();
        // extra should contain the full original JSON
        assert!(parsed.extra.is_object());
    }

    #[test]
    fn test_compliance_extra_fields_preserved() {
        let json = sample_compliance_report();
        let parsed = ParsedComplianceReport::parse(&json).unwrap();
        assert!(parsed.extra.is_object());
        // controls should also have their extra fields
        for control in &parsed.controls {
            assert!(control.extra.is_object());
        }
    }
}