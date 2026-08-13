//! Build the actual MCP `Tool` registries for each namespace.
//!
//! Each tool is a closure capturing an `Arc<SyncApi>` that maps its JSON
//! arguments onto a Spindle REST endpoint and returns the standard envelope
//! (via `crate::envelope::build_envelope`).

use std::sync::Arc;

use mcp_server::{McpError, Tool};
use serde_json::{json, Value};

use crate::client::SyncApi;
use crate::envelope::build_envelope;
use crate::namespace::{tool_count, Namespace};

/// Build the ordered tool registry for `namespace`.
pub fn build_registry(namespace: Namespace, api_url: &str, token: &str) -> Result<Tools, McpError> {
    let api = Arc::new(SyncApi::new(api_url, token.to_string())?);
    let tools = match namespace {
        Namespace::Query => query_tools(&api),
        Namespace::Admin => admin_tools(&api),
        Namespace::Ops => ops_tools(&api),
    };
    debug_assert_eq!(tools.len(), tool_count(namespace));
    Ok(Tools { namespace, tools })
}

/// A fully built, ordered tool set for one namespace.
pub struct Tools {
    pub namespace: Namespace,
    pub tools: Vec<Tool>,
}

// ── helpers ────────────────────────────────────────────────────────────────

fn obj(properties: Value) -> Value {
    json!({ "type": "object", "properties": properties, "additionalProperties": false })
}

fn strp(desc: &str) -> Value {
    json!({ "type": "string", "description": desc })
}

fn intp(desc: &str) -> Value {
    json!({ "type": "integer", "description": desc })
}

fn with_query(base: &str, parts: &[String]) -> String {
    if parts.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", parts.join("&"))
    }
}

/// Build a read-only GET tool. `path` is a closure from argument object → REST
/// path. `summary` describes what the endpoint does for the envelope.
fn get_tool(
    api: &Arc<SyncApi>,
    name: &'static str,
    description: &'static str,
    props: Value,
    path: impl Fn(&Value) -> String + Send + Sync + 'static,
    summary: &'static str,
) -> Tool {
    let api = api.clone();
    Tool::new(name, description, obj(props), move |args| {
        let p = path(&args);
        let raw = match api.get_json(&p) {
            Ok(v) => v,
            Err(e) => {
                let summary = format!("{summary} — {p} — ERROR: {e}");
                return Ok(build_envelope(
                    json!({}),
                    summary,
                ));
            }
        };
        let count = raw
            .get("data")
            .and_then(Value::as_array)
            .map(|a| a.len())
            .unwrap_or(0);
        Ok(build_envelope(
            raw,
            format!("{summary} — {p} ({count} items)"),
        ))
    })
}

/// Extract an optional string arg with the given key.
fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// Extract an optional integer arg with the given key.
fn opt_int(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(Value::as_i64)
}

// ── spindle-query (read-only, 11 tools) ────────────────────────────────────

fn query_tools(api: &Arc<SyncApi>) -> Vec<Tool> {
    let list_nodes = get_tool(
        api,
        "list_nodes",
        "List fleet nodes. Supports limit/platform/status/search filters.",
        json!({
            "limit": intp("Max nodes to return (default 50)."),
            "platform": strp("Filter by platform (e.g. ubuntu)."),
            "status": strp("Filter by status (e.g. compliant)."),
            "search": strp("Free-text search on node name."),
        }),
        |a| {
            let mut q = Vec::new();
            if let Some(l) = opt_int(a, "limit") {
                q.push(format!("limit={l}"));
            }
            if let Some(p) = opt_str(a, "platform") {
                q.push(format!("platform={p}"));
            }
            if let Some(s) = opt_str(a, "status") {
                q.push(format!("status={s}"));
            }
            if let Some(s) = opt_str(a, "search") {
                q.push(format!("search={s}"));
            }
            with_query("v1/nodes", &q)
        },
        "nodes list",
    );

    let get_node = get_tool(
        api,
        "get_node",
        "Get details for a single node by id or name.",
        json!({ "id": strp("Node id or name.") }),
        |a| format!("v1/nodes/{}", opt_str(a, "id").unwrap_or("")),
        "node detail",
    );

    let list_runs = get_tool(
        api,
        "list_runs",
        "List converge runs, optionally filtered by node.",
        json!({
            "node_id": strp("Filter runs by node id."),
            "limit": intp("Max runs to return (default 50)."),
        }),
        |a| {
            let mut q = Vec::new();
            if let Some(n) = opt_str(a, "node_id") {
                q.push(format!("node_id={n}"));
            }
            if let Some(l) = opt_int(a, "limit") {
                q.push(format!("limit={l}"));
            }
            with_query("v1/runs", &q)
        },
        "runs list",
    );

    let get_run = get_tool(
        api,
        "get_run",
        "Get a single run's details by id.",
        json!({ "id": strp("Run id.") }),
        |a| format!("v1/runs/{}", opt_str(a, "id").unwrap_or("")),
        "run detail",
    );

    let list_resource_events = get_tool(
        api,
        "list_resource_events",
        "List resource events for a run.",
        json!({ "run_id": strp("Run id whose resource events to list.") }),
        |a| format!("v1/runs/{}/resource-events", opt_str(a, "run_id").unwrap_or("")),
        "resource events",
    );

    let list_compliance_reports = get_tool(
        api,
        "list_compliance_reports",
        "List compliance (InSpec) reports.",
        json!({
            "node_id": strp("Filter reports by node id."),
            "limit": intp("Max reports to return."),
        }),
        |a| {
            let mut q = Vec::new();
            if let Some(n) = opt_str(a, "node_id") {
                q.push(format!("node_id={n}"));
            }
            if let Some(l) = opt_int(a, "limit") {
                q.push(format!("limit={l}"));
            }
            with_query("v1/compliance/reports", &q)
        },
        "compliance reports list",
    );

    let get_compliance_report = get_tool(
        api,
        "get_compliance_report",
        "Get a single compliance report by id.",
        json!({ "id": strp("Compliance report id.") }),
        |a| format!("v1/compliance/reports/{}", opt_str(a, "id").unwrap_or("")),
        "compliance report detail",
    );

    let list_cookbooks = get_tool(
        api,
        "list_cookbooks",
        "List cookbook inventory.",
        json!({ "limit": intp("Max cookbooks to return.") }),
        |a| {
            if let Some(l) = opt_int(a, "limit") {
                format!("v1/cookbooks?limit={l}")
            } else {
                "v1/cookbooks".to_string()
            }
        },
        "cookbooks list",
    );

    let get_cookbook = get_tool(
        api,
        "get_cookbook",
        "Get a single cookbook by name.",
        json!({ "name": strp("Cookbook name.") }),
        |a| format!("v1/cookbooks/{}", opt_str(a, "name").unwrap_or("")),
        "cookbook detail",
    );

    let aggregate_resources = get_tool(
        api,
        "aggregate_resources",
        "Aggregate resource events by cookbook/type/platform.",
        json!({
            "group_by": strp("Group by: cookbook, resource_type, platform."),
            "window": strp("Time window (e.g. 1h, 24h)."),
        }),
        |a| {
            let mut q = Vec::new();
            if let Some(g) = opt_str(a, "group_by") {
                q.push(format!("group_by={g}"));
            }
            if let Some(w) = opt_str(a, "window") {
                q.push(format!("window={w}"));
            }
            with_query("v1/resource-events/aggregates", &q)
        },
        "aggregates",
    );

    let detect_drift = get_tool(
        api,
        "detect_drift",
        "Detect frequently-changing resources (drift).",
        json!({
            "window": strp("Time window (e.g. 24h)."),
            "threshold": strp("Update-rate threshold."),
            "node_id": strp("Filter by node id."),
        }),
        |a| {
            let mut q = Vec::new();
            if let Some(w) = opt_str(a, "window") {
                q.push(format!("window={w}"));
            }
            if let Some(t) = opt_str(a, "threshold") {
                q.push(format!("threshold={t}"));
            }
            if let Some(n) = opt_str(a, "node_id") {
                q.push(format!("node_id={n}"));
            }
            with_query("v1/resource-events/drift", &q)
        },
        "drift",
    );

    vec![
        list_nodes,
        get_node,
        list_runs,
        get_run,
        list_resource_events,
        list_compliance_reports,
        get_compliance_report,
        list_cookbooks,
        get_cookbook,
        aggregate_resources,
        detect_drift,
    ]
}

// ── spindle-admin (mutating, 5 tools) ──────────────────────────────────────

fn admin_tools(api: &Arc<SyncApi>) -> Vec<Tool> {
    let create_waiver_props = json!({
        "control_id": strp("Control id to waive."),
        "profile_id": strp("Profile id."),
        "justification": strp("Why the waiver is granted."),
        "approver": strp("Approver identity."),
        "days": intp("Waiver expiry in days."),
    });

    let create_waiver = {
        let api = api.clone();
        Tool::new(
            "create_waiver",
            "Create a compliance waiver.",
            obj(create_waiver_props),
            move |args| {
                let body = json!({
                    "control_id": opt_str(&args, "control_id").unwrap_or(""),
                    "profile_id": opt_str(&args, "profile_id").unwrap_or(""),
                    "justification": opt_str(&args, "justification").unwrap_or(""),
                    "approver": opt_str(&args, "approver").unwrap_or(""),
                    "expiry_days": opt_int(&args, "days").unwrap_or(30),
                });
                match api.post_json("v1/waivers", &body) {
                    Ok(raw) => Ok(build_envelope(raw, "create_waiver")),
                    Err(e) => Ok(build_envelope(json!({}), format!("create_waiver — ERROR: {e}"))),
                }
            },
        )
    };

    let revoke_waiver = {
        let api = api.clone();
        Tool::new(
            "revoke_waiver",
            "Revoke (delete) a waiver by id.",
            obj(json!({ "id": strp("Waiver id.") })),
            move |args| {
                let id = opt_str(&args, "id").unwrap_or("");
                match api.delete(&format!("v1/waivers/{id}")) {
                    Ok(status) => {
                        let raw = json!({ "status": status, "deleted": id });
                        Ok(build_envelope(raw, format!("revoke_waiver -> HTTP {status}")))
                    }
                    Err(e) => Ok(build_envelope(json!({}), format!("revoke_waiver — ERROR: {e}"))),
                }
            },
        )
    };

    let run_backup = {
        let api = api.clone();
        Tool::new(
            "run_backup",
            "Trigger a full backup (DB + raw archive).",
            obj(json!({ "dest": strp("Optional destination path.") })),
            move |args| {
                let body = json!({ "dest": opt_str(&args, "dest").unwrap_or("") });
                match api.post_json("v1/backup", &body) {
                    Ok(raw) => Ok(build_envelope(raw, "run_backup")),
                    Err(e) => Ok(build_envelope(json!({}), format!("run_backup — ERROR: {e}"))),
                }
            },
        )
    };

    let restore_backup = {
        let api = api.clone();
        Tool::new(
            "restore_backup",
            "Restore from a backup artifact.",
            obj(json!({ "path": strp("Backup artifact path or id.") })),
            move |args| {
                let body = json!({ "path": opt_str(&args, "path").unwrap_or("") });
                match api.post_json("v1/backup/restore", &body) {
                    Ok(raw) => Ok(build_envelope(raw, "restore_backup")),
                    Err(e) => Ok(build_envelope(json!({}), format!("restore_backup — ERROR: {e}"))),
                }
            },
        )
    };

    let config_validate = {
        let api = api.clone();
        Tool::new(
            "config_validate",
            "Validate the current Spindle configuration.",
            obj(json!({})),
            move |_args| {
                match api.get_json("v1/config/validate") {
                    Ok(raw) => Ok(build_envelope(raw, "config_validate")),
                    Err(e) => Ok(build_envelope(json!({}), format!("config_validate — ERROR: {e}"))),
                }
            },
        )
    };

    vec![
        create_waiver,
        revoke_waiver,
        run_backup,
        restore_backup,
        config_validate,
    ]
}

// ── spindle-ops (health/metrics, 3 tools) ──────────────────────────────────

fn ops_tools(api: &Arc<SyncApi>) -> Vec<Tool> {
    let health_check = get_tool(
        api,
        "health_check",
        "Check Spindle API health.",
        json!({}),
        |_| "v1/health".to_string(),
        "health check",
    );

    let get_metrics = get_tool(
        api,
        "get_metrics",
        "Get Spindle metrics dump.",
        json!({}),
        |_| "v1/health/metrics".to_string(),
        "metrics",
    );

    let queue_depth = {
        let api = api.clone();
        Tool::new(
            "queue_depth",
            "Get the current ingest queue depth.",
            obj(json!({})),
            move |_args| {
                match api.get_json("v1/health") {
                    Ok(raw) => {
                        let depth = raw.pointer("/ingest_lag/queue_depth").cloned();
                        let out = json!({ "queue_depth": depth.unwrap_or(json!(null)) });
                        Ok(build_envelope(out, "queue_depth"))
                    }
                    Err(e) => Ok(build_envelope(json!({}), format!("queue_depth — ERROR: {e}"))),
                }
            },
        )
    };

    vec![health_check, get_metrics, queue_depth]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::namespace::tool_count;

    #[test]
    fn query_namespace_has_11_tools_in_order() {
        // Requires an API URL; uses a throwaway token. Building does not hit the
        // network (clients are lazy).
        let Tools { namespace, tools } =
            build_registry(Namespace::Query, "http://127.0.0.1:1", "").unwrap();
        assert_eq!(namespace, Namespace::Query);
        assert_eq!(tools.len(), tool_count(Namespace::Query));
        let names: Vec<_> = tools.iter().map(|t| t.name).collect();
        let expected = [
            "list_nodes",
            "get_node",
            "list_runs",
            "get_run",
            "list_resource_events",
            "list_compliance_reports",
            "get_compliance_report",
            "list_cookbooks",
            "get_cookbook",
            "aggregate_resources",
            "detect_drift",
        ];
        assert_eq!(names, expected);
    }

    #[test]
    fn admin_namespace_has_5_tools() {
        let Tools { namespace, tools } =
            build_registry(Namespace::Admin, "http://127.0.0.1:1", "").unwrap();
        assert_eq!(namespace, Namespace::Admin);
        assert_eq!(tools.len(), 5);
        let names: Vec<_> = tools.iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            vec![
                "create_waiver",
                "revoke_waiver",
                "run_backup",
                "restore_backup",
                "config_validate"
            ]
        );
    }

    #[test]
    fn ops_namespace_has_3_tools() {
        let Tools { namespace, tools } =
            build_registry(Namespace::Ops, "http://127.0.0.1:1", "").unwrap();
        assert_eq!(namespace, Namespace::Ops);
        assert_eq!(tools.len(), 3);
        let names: Vec<_> = tools.iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["health_check", "get_metrics", "queue_depth"]);
    }
}
