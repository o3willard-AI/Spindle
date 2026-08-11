//! The standard MCP tool result envelope.
//!
//! Per docs/access-architecture.md §4.3, every tool returns:
//! ```json
//! { "data": [...], "pagination": {...}, "summary": "…", "request_id": "…" }
//! ```
//! This module builds that envelope from a Spindle REST API response (which
//! itself already carries `api_version`, `request_id`, `data`, `pagination`)
//! and a caller-supplied human summary.

use serde_json::{Value, json};

/// Build the standard MCP tool result envelope from a raw Spindle API body.
///
/// Preserves `data` and `pagination` if present in the upstream response,
/// synthesizes an empty pagination otherwise, always emits a fresh `summary`
/// string, and re-uses the upstream `request_id` when available (generating a
/// UUIDv4 fallback).
pub fn build_envelope(raw: Value, summary: impl Into<String>) -> Value {
    let data = raw.get("data").cloned().unwrap_or_else(|| raw.clone());
    let pagination = raw
        .get("pagination")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let request_id = raw
        .get("request_id")
        .map(|v| v.clone())
        .unwrap_or_else(|| json!(uuid::Uuid::new_v4().to_string()));

    json!({
        "data": data,
        "pagination": pagination,
        "summary": summary.into(),
        "request_id": request_id,
    })
}

/// Build a friendly "N item(s)" summary from an array element inside the
/// envelope's `data` field, falling back to a generic message.
pub fn summary_for(data: &Value) -> String {
    match data {
        Value::Array(items) => {
            let n = items.len();
            format!("Returned {n} item(s).")
        }
        Value::Object(_) => "Returned an object.".to_string(),
        other => format!("Returned {other}."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_upstream_fields() {
        let raw = json!({
            "api_version": "v1",
            "request_id": "abc123",
            "data": [1, 2, 3],
            "pagination": { "total_count": 3 },
        });
        let env = build_envelope(raw, "three items");
        assert_eq!(env["data"].as_array().unwrap().len(), 3);
        assert_eq!(env["pagination"]["total_count"], 3);
        assert_eq!(env["request_id"], "abc123");
        assert_eq!(env["summary"], "three items");
    }

    #[test]
    fn synthesizes_when_missing() {
        let env = build_envelope(json!({"nodes": []}), "ok");
        assert!(env["pagination"].is_object());
        assert!(env["request_id"].is_string()); // fallback UUID
    }

    #[test]
    fn wraps_non_data_objects() {
        let env = build_envelope(json!({"health": "up"}), "healthy");
        // no upstream `data`, so the whole object becomes `data`
        assert_eq!(env["data"]["health"], "up");
    }

    #[test]
    fn summary_counts_arrays() {
        assert_eq!(summary_for(&json!([1, 2])), "Returned 2 item(s).");
    }
}