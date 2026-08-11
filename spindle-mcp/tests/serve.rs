//! Integration test: spawn the real `spindle-mcp` binary and drive it over
//! stdio exactly like an MCP client would.
//!
//! Uses the live Spindle API when `SPINDLE_MCP_TEST_API` is set (e.g.
//! `http://198.51.100.101:8080`); otherwise it only checks the handshake and
//! tool registry against an unreachable URL (no network dependency).

use std::io::Write;
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_spindle-mcp");

fn api_url() -> String {
    std::env::var("SPINDLE_MCP_TEST_API").unwrap_or_else(|_| "http://127.0.0.1:9".to_string())
}

fn token() -> String {
    std::env::var("SPINDLE_MCP_TEST_TOKEN").unwrap_or_else(|_| "spindle-dev-token".to_string())
}

/// Spawn the binary, send a sequence of newline-delimited JSON-RPC messages,
/// close stdin, and return every response line written to stdout.
fn run_client(namespace: &str, messages: &[String]) -> Vec<String> {
    let mut child = Command::new(BIN)
        .args(["serve", "--namespace", namespace, "--api-url", &api_url()])
        .env("SPINDLE_TOKEN", token())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn spindle-mcp");

    let mut stdin = child.stdin.take().unwrap();
    for msg in messages {
        writeln!(stdin, "{msg}").unwrap();
    }
    drop(stdin);

    let output = child.wait_with_output().expect("failed to wait");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

fn initialize() -> &'static str {
    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#
}

fn tools_list() -> &'static str {
    r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#
}

fn calls_tool(name: &str, args: &str) -> String {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{{\"name\":\"{name}\",\"arguments\":{args}}}}}"
    )
}

#[test]
fn query_namespace_handshake_and_tool_list() {
    let responses = run_client("spindle-query", &[initialize().to_string(), tools_list().to_string()]);
    assert_eq!(responses.len(), 2, "expected two responses, got: {responses:?}");

    let init: serde_json::Value = serde_json::from_str(&responses[0]).unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "spindle-mcp-spindle-query");
    assert!(init["result"]["capabilities"]["tools"].is_object());

    let list: serde_json::Value = serde_json::from_str(&responses[1]).unwrap();
    let tools = list["result"]["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 11);
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"list_nodes"));
    assert!(names.contains(&"detect_drift"));
}

#[test]
fn query_list_nodes_returns_envelope() {
    // The tool always wraps its result in the standard envelope (data /
    // pagination / summary / request_id), whether the upstream call succeeded
    // (live API) or failed. We validate the envelope shape either way.
    let responses = run_client("spindle-query", &[calls_tool("list_nodes", "{}")]);
    let call: serde_json::Value = serde_json::from_str(&responses[0]).unwrap();
    let content = &call["result"]["structuredContent"];
    assert!(content.get("summary").is_some());
    assert!(content.get("pagination").is_some());
    assert!(content.get("request_id").is_some());
}

#[test]
fn unknown_namespace_rejected() {
    let mut child = Command::new(BIN)
        .args(["serve", "--namespace", "bogus", "--api-url", "http://x"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("unknown namespace"), "stderr: {err}");
}

#[test]
fn unsupported_namespace_lengths() {
    // Sanity: query=11, admin=5, ops=3 via the running binary's tools/list.
    for (ns, count) in [("spindle-query", 11), ("spindle-admin", 5), ("spindle-ops", 3)] {
        let responses = run_client(ns, &[tools_list().to_string()]);
        let list: serde_json::Value = serde_json::from_str(&responses[0]).unwrap();
        let tools = list["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), count, "namespace {ns} tool count");
    }
}
