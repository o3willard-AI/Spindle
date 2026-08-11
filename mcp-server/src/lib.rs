//! # mcp-server
//!
//! A minimal, dependency-light implementation of the
//! [Model Context Protocol](https://modelcontextprotocol.io/) **server** over
//! **stdio** transport.
//!
//! This crate is intentionally small and focused on the *protocol* layer only:
//! it understands how to speak JSON-RPC 2.0 over stdin/stdout, how to answer
//! `initialize` / `tools/list` / `tools/call` / `ping`, and how to present a
//! collection of `Tool`s to an MCP client. It knows nothing about Spindle —
//! the tool *handlers* are supplied by the caller (see the `spindle-mcp`
//! crate), which is what keeps this reusable.
//!
//! ## Protocol surface
//!
//! The following methods are implemented, which is everything a read-mostly
//! tools-only MCP client needs:
//!
//! | Method | Handler |
//! |--------|---------|
//! | `initialize` | Returns protocol version, server info, and `tools` capability. |
//! | `notifications/initialized` | Acknowledged (no response — it's a notification). |
//! | `tools/list` | Returns every registered tool with its `inputSchema`. |
//! | `tools/call` | Dispatches to the matching tool handler. |
//! | `ping` | Empty result. |
//! | `shutdown` | Empty result (server exits after a short grace). |
//!
//! Unknown methods and unknown tools return the standard JSON-RPC error
//! envelope (`code`, `message`, `data`).
//!
//! ## Transport
//!
//! `stdio::serve(server)` reads newline-delimited JSON-RPC messages from
//! stdin, dispatches each one, and writes responses to stdout (flushed
//! per-message, as the MCP stdio transport requires).
//!
//! The core `Server::dispatch()` is pure (takes a JSON-RPC `Value`, returns an
//! optional response `Value`) so the protocol can be unit-tested without any
//! real process I/O.

mod error;
mod tool;

pub mod server;
pub mod stdio;

pub use error::{McpError, error_response, error_value};
pub use server::Server;
pub use stdio::serve_stdio;
pub use tool::{Tool, ToolHandler};