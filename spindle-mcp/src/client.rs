//! A synchronous wrapper around `spindle_cli::ApiClient` so MCP tool handlers
//! (which are synchronous closures) can call the async REST API.

use mcp_server::McpError;
use serde_json::Value;
use spindle_cli::ApiClient;

/// Owns an `ApiClient` plus a current-thread tokio runtime used to `block_on`
/// each HTTP call. Handlers capture an `Arc<SyncApi>` and call its sync methods.
pub struct SyncApi {
    client: ApiClient,
    rt: tokio::runtime::Runtime,
}

impl SyncApi {
    /// Build a sync client for the given API base URL and bearer token.
    pub fn new(base_url: &str, token: String) -> Result<Self, McpError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| McpError::Tool(format!("failed to start runtime: {e}")))?;
        Ok(Self {
            client: ApiClient::new(base_url, &token),
            rt,
        })
    }

    /// GET a path and return the parsed JSON body.
    pub fn get_json(&self, path: &str) -> Result<Value, McpError> {
        self.rt
            .block_on(self.client.get_json(path))
            .map_err(|e| McpError::Tool(format!("GET {path} failed: {e}")))
    }

    /// POST a JSON body to a path and return the parsed JSON response.
    pub fn post_json(&self, path: &str, body: &Value) -> Result<Value, McpError> {
        self.rt
            .block_on(self.client.post_json(path, body))
            .map_err(|e| McpError::Tool(format!("POST {path} failed: {e}")))
    }

    /// DELETE a path, returning the HTTP status code.
    pub fn delete(&self, path: &str) -> Result<u16, McpError> {
        self.rt
            .block_on(self.client.delete(path))
            .map_err(|e| McpError::Tool(format!("DELETE {path} failed: {e}")))
    }
}
