//! Output formatting: JSON (stable) and human-readable (TTY).

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
        Value::Array(arr) => {
            arr.iter()
                .map(|v| format!("{}{}", pad, format_human_value(v, indent + 1)))
                .collect::<Vec<_>>()
                .join("\n")
        }
        Value::Object(map) => {
            map.iter()
                .map(|(k, v)| format!("{}{}: {}", pad, k, format_human_value(v, indent + 1)))
                .collect::<Vec<_>>()
                .join("\n")
        }
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

    let mut all_keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for obj in arr {
        if let Value::Object(map) = obj {
            for k in map.keys() {
                all_keys.insert(k.clone());
            }
        }
    }

    let keys: Vec<String> = all_keys.into_iter().collect();
    if keys.is_empty() {
        return "(empty)".to_string();
    }

    let mut lines = Vec::new();
    lines.push(keys.join("\t"));
    for obj in arr {
        let row: Vec<String> = keys
            .iter()
            .map(|k| {
                obj.get(k)
                    .map(|v| {
                        if matches!(v, Value::String(_)) {
                            v.as_str().unwrap_or("").to_string()
                        } else {
                            v.to_string()
                        }
                    })
                    .unwrap_or_default()
            })
            .collect();
        lines.push(row.join("\t"));
    }
    lines.join("\n")
}
