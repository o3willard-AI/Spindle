//! Assemble a fully-configured `mcp_server::Server` for a namespace.

use mcp_server::Server;

use crate::namespace::Namespace;
use crate::registry::build_registry;

/// Build a ready-to-serve MCP `Server` for `namespace`, targeting `api_url`
/// with the given bearer `token`.
///
/// The server's reported identity is `spindle-mcp` with the namespace label so
/// an agent can see which server instance it is talking to.
pub fn build_server(
    namespace: Namespace,
    api_url: &str,
    token: &str,
) -> Result<Server, mcp_server::McpError> {
    let tools = build_registry(namespace, api_url, token)?;
    let mut server = Server::new(
        format!("spindle-mcp-{}", namespace.name()),
        env!("CARGO_PKG_VERSION"),
    );
    server.register_all(tools.tools);
    Ok(server)
}
