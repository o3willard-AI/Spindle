//! Namespace definitions and tool metadata for `spindle-mcp`.
//!
//! Each namespace exposes a focused set of MCP tools (access-architecture.md
//! §4.2). Tools are defined here with their name, description and JSON-Schema
//! input contract; the actual REST dispatch is in `registry.rs`.

/// The three supported MCP namespaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Namespace {
    /// Read-only fleet inspection (11 tools) — `read` scope.
    Query,
    /// Mutating operator actions (5 tools) — `admin` scope.
    Admin,
    /// Health / metrics / queue depth (3 tools).
    Ops,
}

impl Namespace {
    /// Parse a namespace from its CLI string (`spindle-query`, ...).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "spindle-query" => Some(Namespace::Query),
            "spindle-admin" => Some(Namespace::Admin),
            "spindle-ops" => Some(Namespace::Ops),
            _ => None,
        }
    }

    /// The CLI identifier for this namespace.
    pub fn name(&self) -> &'static str {
        match self {
            Namespace::Query => "spindle-query",
            Namespace::Admin => "spindle-admin",
            Namespace::Ops => "spindle-ops",
        }
    }

    /// Human label for logging / initialize response.
    pub fn label(&self) -> &'static str {
        match self {
            Namespace::Query => "Spindle query (read-only)",
            Namespace::Admin => "Spindle admin (mutating)",
            Namespace::Ops => "Spindle ops (health/metrics)",
        }
    }
}

/// The count of tools each namespace exposes.
pub fn tool_count(namespace: Namespace) -> usize {
    match namespace {
        Namespace::Query => 11,
        Namespace::Admin => 5,
        Namespace::Ops => 3,
    }
}
