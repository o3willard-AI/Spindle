//! Tool definition types for the MCP server.

#![allow(warnings)]
use serde_json::Value;
use std::sync::Arc;

use crate::error::McpError;

/// A single MCP tool: its identity, its JSON-Schema input contract, and the
/// function that runs it.
#[derive(Clone)]
pub struct Tool {
    /// Tool name, as exposed to the MCP client (e.g. `list_nodes`).
    pub name: &'static str,
    /// Human/agent-readable description shown in `tools/list`.
    pub description: &'static str,
    /// JSON Schema (`type: "object"` + `properties` + `required`) describing
    /// the tool's arguments.
    pub input_schema: Value,
    /// The executable body of the tool.
    pub call: Arc<dyn Fn(Value) -> Result<Value, McpError> + Send + Sync + 'static>,
}

impl Tool {
    /// Build a tool from static metadata and a boxed handler.
    pub fn new(
        name: &'static str,
        description: &'static str,
        input_schema: Value,
        call: impl Fn(Value) -> Result<Value, McpError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            name,
            description,
            input_schema,
            call: Arc::new(call),
        }
    }

    /// Thin wrapper over the callable for callers that need to invoke it.
    pub fn run(&self, args: Value) -> Result<Value, McpError> {
        (self.call)(args)
    }
}

/// Trait that any stateful executor (e.g. one holding an `ApiClient`) can
/// implement. Register instances via `Server::register` (see `server.rs`),
/// which boxes them into a `Tool`.
pub trait ToolHandler: Send + Sync {
    /// Execute with the raw arguments object.
    fn execute(&self, args: Value) -> Result<Value, McpError>;
}

/// A default, permissive object schema (`properties: {}`, no required fields).
pub fn json_object_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false,
    })
}
