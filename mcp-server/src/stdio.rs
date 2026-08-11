//! Stdio transport for the MCP server.
//!
//! The MCP stdio transport is newline-delimited JSON-RPC: the client writes
//! one JSON message per line to stdin, and the server writes one response per
//! line to stdout. Responses must be flushed after every message so the client
//! can correlate them by `id`.

use crate::server::Server;
use crate::McpError;
use std::io::{BufRead, Write};

/// Run the server over stdin/stdout until EOF on stdin.
///
/// Reads newline-delimited JSON, dispatches each message through
/// [`Server::dispatch`], and writes any response back to stdout (flushed per
/// message). Exits when stdin reaches EOF.
pub fn serve_stdio(server: Server) -> Result<(), McpError> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut reader = stdin.lock();

    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            // EOF — client closed the pipe.
            return Ok(());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let message: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(m) => m,
            Err(e) => {
                let id = serde_json::json!(null);
                let resp = crate::error_response(id, crate::error::code::PARSE_ERROR, &e.to_string());
                writeln!(stdout, "{resp}")?;
                stdout.flush()?;
                continue;
            }
        };

        if let Some(response) = server.dispatch(&message) {
            writeln!(stdout, "{response}")?;
            stdout.flush()?;
        }
    }
}
