//! Output formatting: JSON (stable) and human-readable (TTY).
//!
//! Human-readable output uses `comfy-table` for nice table rendering.
//! JSON output is stable and machine-readable.

use comfy_table::{presets::UTF8_FULL, Table};
use serde_json::Value;

pub fn format_output_human(data: &Value) -> String {
    match data {
        Value::Array(arr) => {
            if arr.is_empty() {
                "(empty)".to_string()
            } else if let Some(first) = arr.first() {
                if let Value::Object(_) = first {
                    format_table(arr)
                } else {
                    arr.iter()
                        .map(|v| format_human_value(v, 0))
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            } else {
                "(empty)".to_string()
            }
        }
        Value::Object(map) => {
            // If the object has a "data" key with an array, render that as a table
            if let Some(inner) = map.get("data") {
                if let Value::Array(arr) = inner {
                    if !arr.is_empty() && arr.first().map(|v| v.is_object()).unwrap_or(false) {
                        let mut result = format_table(arr);
                        // Append any pagination info
                        if let Some(pagination) = map.get("pagination") {
                            result.push_str(&format!("\n\n{}", format_human_value(pagination, 0)));
                        }
                        if let Some(filters) = map.get("filters") {
                            result.push_str(&format!("\n{}", format_human_value(filters, 0)));
                        }
                        return result;
                    }
                }
                if let Value::Object(_) = inner {
                    let mut result = format_human_value(inner, 0);
                    if let Some(pagination) = map.get("pagination") {
                        result.push_str(&format!("\n{}", format_human_value(pagination, 0)));
                    }
                    return result;
                }
            }
            // Plain object — render as key-value pairs
            map.iter()
                .map(|(k, v)| format!("{}: {}", k, format_human_value(v, 0)))
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => format_human_value(data, 0),
    }
}

pub fn format_human_value(val: &Value, indent: usize) -> String {
    let pad = "  ".repeat(indent);
    match val {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "(null)".to_string(),
        Value::Array(arr) => arr
            .iter()
            .map(|v| format!("{}{}", pad, format_human_value(v, indent + 1)))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| format!("{}{}: {}", pad, k, format_human_value(v, indent + 1)))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

pub fn format_table(arr: &[Value]) -> String {
    if arr.is_empty() || !arr.iter().all(|v| v.is_object()) {
        return arr
            .iter()
            .map(|v| format_human_value(v, 0))
            .collect::<Vec<_>>()
            .join("\n");
    }

    // Collect all unique keys across all objects, preserving first-seen order
    let mut all_keys: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for obj in arr {
        if let Value::Object(map) = obj {
            for k in map.keys() {
                if seen.insert(k.clone()) {
                    all_keys.push(k.clone());
                }
            }
        }
    }

    if all_keys.is_empty() {
        return "(empty)".to_string();
    }

    let mut table = Table::new();
    table.load_style(UTF8_FULL);

    // Header row
    let header: Vec<&str> = all_keys.iter().map(|k| k.as_str()).collect();
    table.set_header(header);

    // Data rows
    for obj in arr {
        if let Value::Object(map) = obj {
            let row: Vec<String> = all_keys
                .iter()
                .map(|k| map.get(k).map(format_value_cell).unwrap_or_default())
                .collect();
            table.add_row(row);
        }
    }

    table.to_string()
}

/// Format a single value as a table cell string.
fn format_value_cell(val: &Value) -> String {
    match val {
        Value::String(s) => {
            if s.len() > 50 {
                format!("{}...", &s[..47])
            } else {
                s.clone()
            }
        }
        Value::Null => String::new(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Array(arr) => {
            if arr.is_empty() {
                "[]".to_string()
            } else {
                format!("[{} items]", arr.len())
            }
        }
        Value::Object(map) => {
            if map.is_empty() {
                "{}".to_string()
            } else {
                format!(
                    "{{{}}}",
                    map.keys().next().map(|k| k.as_str()).unwrap_or("")
                )
            }
        }
    }
}
