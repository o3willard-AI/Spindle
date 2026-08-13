//! Core server: holds tools and dispatches JSON-RPC messages.
//!
//! [`Server::dispatch`] is the pure protocol core — it takes a complete
//! JSON-RPC message `Value` and returns `Some(response)` for a request or
//! `None` for a notification. The stdio loop (see `stdio.rs`) just feeds lines
//! into it and writes responses out.

#![allow(warnings)]
use serde_json::{json, Value};

use crate::error::{code, error_response, McpError};
use crate::tool::Tool;

const PROTOCOL_VERSION: &str = "2024-11-05";

/// An MCP protocol server exposing a set of tools.
pub struct Server {
    name: String,
    version: String,
    tools: Vec<Tool>,
}

impl Server {
    /// Build a new server with the given identity (name + version reported in
    /// the `initialize` response).
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            tools: Vec::new(),
        }
    }

    /// Register a tool (by value) with the server. The order registered is the
    /// order reported in `tools/list`.
    pub fn register(&mut self, tool: Tool) -> &mut Self {
        self.tools.push(tool);
        self
    }

    /// Register many tools at once.
    pub fn register_all(&mut self, tools: Vec<Tool>) -> &mut Self {
        self.tools.extend(tools);
        self
    }

    /// Access the registered tools.
    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    /// Dispatch a single JSON-RPC message.
    ///
    /// Returns `Some(response_value)` when a response is required (the message
    /// carried an `id`), and `None` for notifications (no `id`), which get no
    /// reply.
    pub fn dispatch(&self, message: &Value) -> Option<Value> {
        let id = match message.get("id") {
            Some(v) => v.clone(),
            None => {
                // Notification: still process idempotent methods, but never reply.
                self.handle_known_notification(message);
                return None;
            }
        };

        let method = match message.get("method").and_then(Value::as_str) {
            Some(m) => m,
            None => return Some(error_response(id, code::INVALID_REQUEST, "missing method")),
        };

        let result = match method {
            "initialize" => self.handle_initialize(),
            "ping" => json!({}),
            "tools/list" => self.handle_tools_list(),
            "tools/call" => self.handle_tools_call(message),
            "shutdown" => json!({}),
            other => {
                return Some(error_response(
                    id,
                    code::METHOD_NOT_FOUND,
                    &format!("method not found: {other}"),
                ))
            }
        };

        Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }))
    }

    /// Handle notification-only methods (no reply required). Idempotent and
    /// currently a no-op except for logging-friendly bookkeeping.
    fn handle_known_notification(&self, message: &Value) {
        // `notifications/initialized` and `notifications/cancelled` are
        // acknowledged implicitly by not responding.
        let _ = message.get("method");
    }
}

impl Server {
    /// `initialize` result: protocol version, server info, capabilities.
    fn handle_initialize(&self) -> Value {
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {
                "tools": { "listChanged": false },
            },
            "serverInfo": {
                "name": self.name,
                "version": self.version,
            },
        })
    }

    /// `tools/list` result: every registered tool's description + schema.
    fn handle_tools_list(&self) -> Value {
        let tools: Vec<Value> = self
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": t.input_schema,
                })
            })
            .collect();
        json!({ "tools": tools })
    }

    /// `tools/call` result: dispatch to the named tool and return its output.
    fn handle_tools_call(&self, message: &Value) -> Value {
        let name = message
            .pointer("/params/name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let args = message
            .pointer("/params/arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        match self.tools.iter().find(|t| t.name == name) {
            Some(tool) => match tool.run(args) {
                Ok(out) => json!({
                    "content": [
                        { "type": "text", "text": out.to_string() }
                    ],
                    "isError": false,
                    "structuredContent": out,
                }),
                Err(McpError::Tool(msg)) => json!({
                    "content": [
                        { "type": "text", "text": format!("tool error: {msg}") }
                    ],
                    "isError": true,
                }),
                Err(other) => json!({
                    "content": [
                        { "type": "text", "text": format!("error: {other}") }
                    ],
                    "isError": true,
                }),
            },
            None => json!({
                "content": [
                    { "type": "text", "text": format!("unknown tool: {name}") }
                ],
                "isError": true,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Tool;

    fn server() -> Server {
        let mut s = Server::new("test-mcp", "0.1.0");
        s.register(Tool::new(
            "echo",
            "echo the argument",
            json!({ "type": "object", "properties": { "msg": { "type": "string" } } }),
            |args| Ok(args),
        ));
        s
    }

    fn call(s: &Server, method: &str, params: Value) -> Value {
        let msg = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });
        s.dispatch(&msg).expect("expected a response")
    }

    #[test]
    fn initialize_reports_capabilities() {
        let resp = call(&server(), "initialize", json!({}));
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(resp["result"]["serverInfo"]["name"], "test-mcp");
        assert!(resp["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_list_returns_registered_tools() {
        let resp = call(&server(), "tools/list", json!({}));
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "echo");
        assert!(tools[0]["inputSchema"].is_object());
    }

    #[test]
    fn tools_call_invokes_handler() {
        let resp = call(
            &server(),
            "tools/call",
            json!({
                "name": "echo",
                "arguments": { "msg": "hi" },
            }),
        );
        assert_eq!(resp["result"]["isError"], false);
        assert_eq!(resp["result"]["structuredContent"]["msg"], "hi");
    }

    #[test]
    fn tools_call_unknown_tool_is_error() {
        let resp = call(
            &server(),
            "tools/call",
            json!({
                "name": "nope",
                "arguments": {},
            }),
        );
        assert_eq!(resp["result"]["isError"], true);
    }

    #[test]
    fn notification_gets_no_response() {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        });
        assert!(server().dispatch(&msg).is_none());
    }

    #[test]
    fn unknown_method_returns_error_envelope() {
        let resp = call(&server(), "bogus/method", json!({}));
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[test]
    fn shutdown_returns_empty_result() {
        let resp = call(&server(), "shutdown", json!({}));
        assert!(resp["result"].is_object());
    }
}
