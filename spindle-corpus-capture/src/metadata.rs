/// Metadata extraction from Chef data collector requests.
///
/// Extracts client_version, platform info, run type classification, and node_name
/// from incoming HTTP requests to the corpus capture proxy.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MetadataError {
    #[error("failed to parse JSON body: {0}")]
    JsonParse(#[from] serde_json::Error),

    #[error("payload too large: {size} bytes (limit: {limit})")]
    PayloadTooLarge { size: u64, limit: u64 },
}

/// Platform information extracted from Chef client payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformInfo {
    pub name: String,
    pub version: String,
    pub architecture: String,
}

impl PlatformInfo {
    /// Create a human-readable platform identifier string (e.g., "ubuntu-22.04-x86_64").
    pub fn to_string(&self) -> String {
        format!("{}-{}-{}", self.name, self.version, self.architecture)
    }
}

/// Classification of the Chef run type based on path and payload content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RunType {
    /// Standard converge run that completed successfully
    ConvergeSuccess,
    /// Standard converge run that failed
    ConvergeFailure,
    /// Partial or compliance-only run
    Partial,
    /// Compliance phase only (InSpec profiles)
    ComplianceOnly,
    /// Unknown — couldn't classify from available data
    Unknown,
}

impl std::fmt::Display for RunType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunType::ConvergeSuccess => write!(f, "converge_success"),
            RunType::ConvergeFailure => write!(f, "converge_failure"),
            RunType::Partial => write!(f, "partial"),
            RunType::ComplianceOnly => write!(f, "compliance_only"),
            RunType::Unknown => write!(f, "unknown"),
        }
    }
}

/// Complete metadata extracted from a captured request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureMetadata {
    /// Chef client version (e.g., "18.4.23", "17.10.3")
    pub client_version: Option<String>,
    /// Platform information from the node attributes
    pub platform: PlatformInfo,
    /// Run type classification
    pub run_type: RunType,
    /// Node name extracted from path or payload
    pub node_name: String,
}

impl CaptureMetadata {
    /// Extract metadata from an HTTP request.
    ///
    /// This is the core function that parses incoming Chef data collector requests
    /// to extract rich metadata for corpus records. It uses multiple detection methods
    /// in priority order per the DESIGN.md specification.
    pub fn extract(path: &str, body: &[u8]) -> Result<Self, MetadataError> {
        // Parse JSON body (Chef data collector payloads are always JSON)
        let payload = parse_payload(body)?;

        // Extract client version from payload or headers
        let client_version = extract_client_version(&payload);

        // Classify run type from path + body content
        let run_type = classify_run(path, &payload);

        // Extract platform info
        let platform = extract_platform(&payload);

        // Extract node name
        let node_name = extract_node_name(path, &payload);

        Ok(CaptureMetadata {
            client_version,
            platform,
            run_type,
            node_name,
        })
    }

    /// Create a default metadata for requests where extraction fails.
    pub fn default_unknown() -> Self {
        CaptureMetadata {
            client_version: None,
            platform: PlatformInfo {
                name: "unknown".to_string(),
                version: "unknown".to_string(),
                architecture: "unknown".to_string(),
            },
            run_type: RunType::Unknown,
            node_name: "unknown".to_string(),
        }
    }
}

/// Parse the JSON request body. Returns None if not valid JSON (some Chef payloads may be malformed).
fn parse_payload(body: &[u8]) -> Result<serde_json::Value, MetadataError> {
    let payload = serde_json::from_slice::<serde_json::Value>(body)?;
    Ok(payload)
}

/// Extract client version using multiple methods in priority order.
///
/// Priority:
/// 1. `chef_implementation_version` field from JSON body (highest reliability)
/// 2. `chef_version` field as fallback
/// 3. Custom headers containing version info
fn extract_client_version(payload: &serde_json::Value) -> Option<String> {
    payload["chef_implementation_version"]
        .as_str()
        .map(String::from)
        .or_else(|| {
            // Fallback to chef_version field (older format)
            payload["chef_version"].as_str().map(String::from)
        })
}

/// Classify the run type based on request path and body content.
///
/// Classification rules per DESIGN.md:
/// - `/reports` → converge_success or converge_failure (status field in body)
/// - `/checkins` → partial or compliance_only (compliance_summary present = compliance_only)
/// - Generic POST to `/data_collector/v0/` → unknown
fn classify_run(path: &str, payload: &serde_json::Value) -> RunType {
    // Check path patterns first (higher priority than content analysis)
    if path.contains("/reports") {
        return classify_converge_status(payload);
    }

    if path.contains("/checkins") {
        return classify_checkin_type(path, payload);
    }

    // Fallback to content analysis for generic paths
    if path.contains("/data_collector/") {
        return classify_by_content(payload);
    }

    RunType::Unknown
}

/// Determine if a converge run succeeded or failed based on status field.
fn classify_converge_status(payload: &serde_json::Value) -> RunType {
    // Chef Infra Client reports include a "status" field in the report JSON
    let status = payload["status"].as_str().unwrap_or("");

    match status.to_lowercase().as_str() {
        "success" | "succeeded" => RunType::ConvergeSuccess,
        "failure" | "failed" => RunType::ConvergeFailure,
        _ => RunType::Unknown, // Unknown for converge without clear status
    }
}

/// Classify a checkin as partial or compliance_only.
fn classify_checkin_type(path: &str, payload: &serde_json::Value) -> RunType {
    // Check if this was a compliance phase run (InSpec profiles present)
    let has_compliance = has_inspec_profiles(payload);

    if path.contains("compliance") || has_compliance {
        RunType::ComplianceOnly
    } else {
        RunType::Partial
    }
}

/// Classify by content when path doesn't match standard patterns.
fn classify_by_content(payload: &serde_json::Value) -> RunType {
    // Check for InSpec/compliance indicators in body
    if has_inspec_profiles(payload) {
        return RunType::ComplianceOnly;
    }

    // If we have a status field, use it
    let status = payload["status"].as_str().unwrap_or("");
    match status.to_lowercase().as_str() {
        "success" | "succeeded" => RunType::ConvergeSuccess,
        "failure" | "failed" => RunType::ConvergeFailure,
        _ => RunType::Unknown,
    }
}

/// Check if the payload contains InSpec profiles (compliance run indicator).
fn has_inspec_profiles(payload: &serde_json::Value) -> bool {
    // Look for InSpec-related fields in Chef payloads
    let has_run_list = payload["run_list"]
        .as_array()
        .map(|list| list.iter().any(|item| item.as_str().unwrap_or("").contains("inspec")))
        .unwrap_or(false);

    let has_compliance_summary = !payload["compliance_summary"].is_null();

    let has_inspec_profiles_field = payload["inspec"]
        .as_object()
        .map(|o| !o.is_empty())
        .unwrap_or(false)
        || payload["profile_results"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false);

    has_run_list || has_compliance_summary || has_inspec_profiles_field
}

/// Extract platform information from Chef client node attributes.
fn extract_platform(payload: &serde_json::Value) -> PlatformInfo {
    // Chef payloads have nested platform info under "node" → "platform" or directly in root
    let name = payload["platform"]["name"]
        .as_str()
        .or_else(|| payload["node"]["platform"]["name"].as_str())
        .unwrap_or("unknown")
        .to_string();

    let version = payload["platform"]["version"]
        .as_str()
        .or_else(|| payload["node"]["platform"]["version"].as_str())
        .unwrap_or("unknown")
        .to_string();

    let architecture = payload["platform"]["architecture"]
        .as_str()
        .or_else(|| payload["node"]["platform"]["architecture"].as_str())
        .unwrap_or("unknown")
        .to_string();

    PlatformInfo {
        name,
        version,
        architecture,
    }
}

/// Extract node name from path or JSON body.
fn extract_node_name(path: &str, payload: &serde_json::Value) -> String {
    // Method 1: Try to parse node name from URL path
    if let Some(node_pos) = path.find("/nodes/") {
        let after_nodes = &path[node_pos + 7..]; // Skip "/nodes/"
        let node_name_end = after_nodes.find('/').unwrap_or(after_nodes.len());
        let candidate = &after_nodes[..node_name_end];
        if !candidate.is_empty() {
            return candidate.to_string();
        }
    }

    // Method 2: Extract from JSON body (node name field)
    payload["node"]["name"]
        .as_str()
        .or_else(|| payload["name"].as_str())
        .unwrap_or("unknown")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_client_version_from_implementation_field() {
        let body = r#"{ "chef_implementation_version": "18.4.23", "node": {} }"#;
        let payload = serde_json::from_str(body).unwrap();
        assert_eq!(extract_client_version(&payload), Some("18.4.23".to_string()));
    }

    #[test]
    fn test_extract_client_version_fallback_to_chef_version() {
        let body = r#"{ "chef_version": "17.10.3", "node": {} }"#;
        let payload = serde_json::from_str(body).unwrap();
        assert_eq!(extract_client_version(&payload), Some("17.10.3".to_string()));
    }

    #[test]
    fn test_extract_client_version_missing() {
        let body = r#"{ "node": {} }"#;
        let payload = serde_json::from_str(body).unwrap();
        assert_eq!(extract_client_version(&payload), None);
    }

    #[test]
    fn test_classify_converge_success() {
        let body = r#"{"status": "success", "node": {"platform": {"name": "ubuntu", "version": "22.04", "architecture": "x86_64"}}}"#;
        let payload = serde_json::from_str(body).unwrap();
        assert_eq!(classify_converge_status(&payload), RunType::ConvergeSuccess);
    }

    #[test]
    fn test_classify_converge_failure() {
        let body = r#"{"status": "failure", "node": {"platform": {"name": "rhel", "version": "8.8", "architecture": "x86_64"}}}"#;
        let payload = serde_json::from_str(body).unwrap();
        assert_eq!(classify_converge_status(&payload), RunType::ConvergeFailure);
    }

    #[test]
    fn test_classify_compliance_only() {
        let body = r#"{"compliance_summary": {"profiles": [{"name": "hardening"}]}, "node": {"platform": {"name": "ubuntu", "version": "22.04", "architecture": "x86_64"}}}"#;
        let payload = serde_json::from_str(body).unwrap();
        assert_eq!(classify_checkin_type("/data_collector/v0/nodes/test/checkins/compliance", &payload), RunType::ComplianceOnly);
    }

    #[test]
    fn test_classify_partial_run() {
        let body = r#"{"node": {"platform": {"name": "windows", "version": "2022", "architecture": "amd64"}}}"#;
        let payload = serde_json::from_str(body).unwrap();
        assert_eq!(classify_checkin_type("/data_collector/v0/nodes/test/checkins", &payload), RunType::Partial);
    }

    #[test]
    fn test_extract_platform_from_root() {
        let body = r#"{"platform": {"name": "ubuntu", "version": "22.04", "architecture": "x86_64"}, "node": {}}"#;
        let payload = serde_json::from_str(body).unwrap();
        let platform = extract_platform(&payload);
        assert_eq!(platform.name, "ubuntu");
        assert_eq!(platform.version, "22.04");
        assert_eq!(platform.architecture, "x86_64");
    }

    #[test]
    fn test_extract_platform_from_node() {
        let body = r#"{"node": {"platform": {"name": "rhel", "version": "8.8", "architecture": "x86_64"}}}"#;
        let payload = serde_json::from_str(body).unwrap();
        let platform = extract_platform(&payload);
        assert_eq!(platform.name, "rhel");
    }

    #[test]
    fn test_extract_node_name_from_path() {
        let path = "/data_collector/v0/nodes/webserver-01/reports";
        let body = r#"{"node": {"name": "other-node"}}"#;
        let payload = serde_json::from_str(body).unwrap();

        // Should extract from path, not body (path has higher priority)
        assert_eq!(extract_node_name(path, &payload), "webserver-01");
    }

    #[test]
    fn test_extract_node_name_from_body() {
        let path = "/data_collector/v0/";
        let body = r#"{"node": {"name": "my-node-42"}}"#;
        let payload = serde_json::from_str(body).unwrap();

        assert_eq!(extract_node_name(path, &payload), "my-node-42");
    }

    #[test]
    fn test_capture_metadata_full_extraction() {
        let path = "/data_collector/v0/nodes/webserver-01/reports";
        let body = r#"{
            "chef_implementation_version": "18.4.23",
            "status": "success",
            "node": {
                "name": "webserver-01",
                "platform": {
                    "name": "ubuntu",
                    "version": "22.04",
                    "architecture": "x86_64"
                }
            }
        }"#;

        let metadata = CaptureMetadata::extract(path, body.as_bytes()).unwrap();

        assert_eq!(metadata.client_version, Some("18.4.23".to_string()));
        assert_eq!(metadata.run_type, RunType::ConvergeSuccess);
        assert_eq!(metadata.node_name, "webserver-01");
        assert_eq!(metadata.platform.name, "ubuntu");
    }

    #[test]
    fn test_platform_info_to_string() {
        let platform = PlatformInfo {
            name: "ubuntu".to_string(),
            version: "22.04".to_string(),
            architecture: "x86_64".to_string(),
        };

        assert_eq!(platform.to_string(), "ubuntu-22.04-x86_64");
    }

    #[test]
    fn test_run_type_display() {
        assert_eq!(format!("{}", RunType::ConvergeSuccess), "converge_success");
        assert_eq!(format!("{}", RunType::ComplianceOnly), "compliance_only");
    }
}
