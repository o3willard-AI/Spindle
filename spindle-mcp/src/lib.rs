//! # spindle-mcp
//!
//! An MCP (Model Context Protocol) server that exposes the Spindle REST API
//! as tools to AI agents, over stdio transport.
//!
//! ## Design
//!
//! This crate implements the **access-architecture.md §4** interface: a single
//! binary, `spindle-mcp`, that can run in three different *namespace* modes,
//! each exposing a focused set of tools that all talk to the same Spindle REST
//! API:
//!
//! | Namespace | Tools | Concern |
//! |-----------|-------|---------|
//! | `spindle-query` | 11 | Read-only fleet inspection (read scope). |
//! | `spindle-admin` | 5  | Mutating operator actions (admin scope). |
//! | `spindle-ops`   | 3  | Health / metrics / queue depth. |
//!
//! Every tool returns the standard envelope:
//! ```json
//! { "data": [...], "pagination": {...}, "summary": "…", "request_id": "…" }
//! ```
//!
//! HTTP is proxied through `spindle_cli::ApiClient` (so token transport,
//! URL joining and error handling are shared with the CLI). The wire protocol
//! itself (JSON-RPC over stdio) is delegated to the reusable `mcp-server`
//! crate.

pub mod client;
pub mod envelope;
pub mod namespace;
pub mod registry;
pub mod server_builder;

pub use namespace::Namespace;
pub use registry::Tools;
pub use server_builder::build_server;
