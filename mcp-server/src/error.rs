//! Error types for the MCP server.

#![allow(warnings)]
use serde_json::{json, Value};
use thiserror::Error;

/// An error that can occur while handling an MCP message or running the server.
#[derive(Debug, Error)]
pub enum McpError {
    /// The wire message was not valid JSON-RPC.
    #[error("invalid JSON-RPC message: {0}")]
    InvalidMessage(String),

    /// I/O failure reading stdin or writing stdout.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Failure serializing/deserializing JSON.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// A tool was not found.
    #[error("unknown tool: {0}")]
    UnknownTool(String),

    /// A tool handler returned an error.
    #[error("tool error: {0}")]
    Tool(String),
}

/// Standard JSON-RPC error codes.
pub mod code {
    /// Invalid JSON was received by the server.
    pub const PARSE_ERROR: i64 = -32700;
    /// The JSON sent is not a valid Request object.
    pub const INVALID_REQUEST: i64 = -32600;
    /// The method does not exist / is not recognized.
    pub const METHOD_NOT_FOUND: i64 = -32601;
    /// Invalid method parameter(s).
    pub const INVALID_PARAMS: i64 = -32602;
    /// Internal JSON-RPC error.
    pub const INTERNAL_ERROR: i64 = -32603;
}

/// Build a JSON-RPC error `Value` object (`code` + `message`).
pub fn error_value(code: i64, message: &str) -> Value {
    json!({ "code": code, "message": message })
}

/// Build a complete JSON-RPC error response `Value`.
pub fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": error_value(code, message),
    })
}