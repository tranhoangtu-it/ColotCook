//! HTTP/SSE transport for remote MCP servers.
//!
//! Implements the MCP Streamable HTTP transport (2025-03-26 spec):
//! - `POST /` with JSON-RPC request body
//! - Response is JSON-RPC response (no streaming for now)
//! - Headers and optional bearer token authentication
//!
//! **SSE limitation:** SSE servers are currently treated as plain HTTP
//! (request-response polling). True server-sent event streaming is not yet
//! implemented. This works for most MCP servers but may cause issues with
//! servers that require persistent SSE connections.

use std::collections::BTreeMap;
use std::io;
use std::time::Duration;

use reqwest::Client;
use crate::mcp_client::{McpClientAuth, McpRemoteTransport, DEFAULT_MCP_TOOL_CALL_TIMEOUT_MS};
use crate::mcp_stdio::{
    JsonRpcId, JsonRpcRequest, JsonRpcResponse, McpInitializeClientInfo, McpInitializeParams,
    McpInitializeResult, McpListToolsParams, McpListToolsResult, McpServerManagerError,
    McpToolCallParams, McpToolCallResult,
};

const DEFAULT_HTTP_TIMEOUT_MS: u64 = 30_000;

/// HTTP-based MCP client that communicates with remote MCP servers via JSON-RPC over HTTP POST.
#[derive(Debug)]
pub struct McpHttpClient {
    client: Client,
    url: String,
    headers: BTreeMap<String, String>,
    auth: McpClientAuth,
    server_name: String,
}

impl McpHttpClient {
    /// Create a new HTTP MCP client from a remote transport configuration.
    pub fn new(server_name: &str, transport: &McpRemoteTransport) -> Result<Self, io::Error> {
        let client = Client::builder()
            .user_agent("colotcook/0.1")
            .build()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        Ok(Self {
            client,
            url: transport.url.clone(),
            headers: transport.headers.clone(),
            auth: transport.auth.clone(),
            server_name: server_name.to_string(),
        })
    }

    /// Apply custom headers and authentication to a request builder.
    fn apply_headers_and_auth(
        &self,
        mut builder: reqwest::RequestBuilder,
    ) -> reqwest::RequestBuilder {
        for (key, value) in &self.headers {
            builder = builder.header(key.as_str(), value.as_str());
        }

        // NOTE: OAuth support is currently limited to using `client_id` as a
        // bearer token placeholder. Full OAuth 2.0 token exchange (authorize →
        // token → refresh) is not yet implemented. For static API keys, use the
        // `headers` field instead (e.g., `"Authorization": "Bearer <key>"`).
        if let McpClientAuth::OAuth(ref oauth) = self.auth {
            if let Some(ref client_id) = oauth.client_id {
                builder = builder.bearer_auth(client_id);
            }
        }

        builder
    }

    /// Send a JSON-RPC request and return the parsed response.
    async fn send_request<P: serde::Serialize, R: serde::de::DeserializeOwned>(
        &self,
        method: &'static str,
        request: JsonRpcRequest<P>,
        timeout_ms: u64,
    ) -> Result<JsonRpcResponse<R>, McpServerManagerError> {
        let builder = self
            .client
            .post(&self.url)
            .timeout(Duration::from_millis(timeout_ms))
            .header("Content-Type", "application/json");

        let builder = self.apply_headers_and_auth(builder);

        let response = builder
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    McpServerManagerError::Timeout {
                        server_name: self.server_name.clone(),
                        method,
                        timeout_ms,
                    }
                } else if e.is_connect() {
                    McpServerManagerError::Transport {
                        server_name: self.server_name.clone(),
                        method,
                        source: io::Error::new(
                            io::ErrorKind::ConnectionRefused,
                            e.to_string(),
                        ),
                    }
                } else {
                    McpServerManagerError::Transport {
                        server_name: self.server_name.clone(),
                        method,
                        source: io::Error::new(io::ErrorKind::Other, e.to_string()),
                    }
                }
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(McpServerManagerError::InvalidResponse {
                server_name: self.server_name.clone(),
                method,
                details: format!("HTTP {status}: {body}"),
            });
        }

        let parsed: JsonRpcResponse<R> =
            response.json().await.map_err(|e| {
                McpServerManagerError::InvalidResponse {
                    server_name: self.server_name.clone(),
                    method,
                    details: format!("failed to parse JSON-RPC response: {e}"),
                }
            })?;

        Ok(parsed)
    }

    /// Initialize the MCP connection.
    pub async fn initialize(
        &self,
        request_id: JsonRpcId,
    ) -> Result<JsonRpcResponse<McpInitializeResult>, McpServerManagerError> {
        let request = JsonRpcRequest::new(
            request_id,
            "initialize",
            Some(McpInitializeParams {
                protocol_version: "2025-03-26".to_string(),
                capabilities: serde_json::json!({}),
                client_info: McpInitializeClientInfo {
                    name: "ColotCook".to_string(),
                    version: "0.1.0".to_string(),
                },
            }),
        );
        self.send_request("initialize", request, DEFAULT_HTTP_TIMEOUT_MS)
            .await
    }

    /// Send the initialized notification (fire-and-forget per MCP spec).
    pub async fn send_initialized(&self) -> Result<(), McpServerManagerError> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        });

        let builder = self
            .client
            .post(&self.url)
            .timeout(Duration::from_millis(5_000))
            .header("Content-Type", "application/json");

        let builder = self.apply_headers_and_auth(builder);

        // Per MCP spec, the initialized notification is fire-and-forget.
        // Log the error for debugging but don't fail the handshake.
        if let Err(_e) = builder.json(&notification).send().await {
            #[cfg(debug_assertions)]
            eprintln!(
                "[MCP] warning: failed to send initialized notification to {}: {_e}",
                self.server_name
            );
        }
        Ok(())
    }

    /// List tools from the remote MCP server.
    pub async fn list_tools(
        &self,
        request_id: JsonRpcId,
        params: McpListToolsParams,
    ) -> Result<JsonRpcResponse<McpListToolsResult>, McpServerManagerError> {
        let request = JsonRpcRequest::new(request_id, "tools/list", Some(params));
        self.send_request("tools/list", request, DEFAULT_HTTP_TIMEOUT_MS)
            .await
    }

    /// Call a tool on the remote MCP server.
    pub async fn call_tool(
        &self,
        request_id: JsonRpcId,
        params: McpToolCallParams,
    ) -> Result<JsonRpcResponse<McpToolCallResult>, McpServerManagerError> {
        let request = JsonRpcRequest::new(request_id, "tools/call", Some(params));
        self.send_request("tools/call", request, DEFAULT_MCP_TOOL_CALL_TIMEOUT_MS)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_client::McpRemoteTransport;

    #[test]
    fn creates_http_client_from_transport() {
        let transport = McpRemoteTransport {
            url: "http://localhost:8080/mcp".to_string(),
            headers: BTreeMap::from([("X-Api-Key".to_string(), "test-key".to_string())]),
            headers_helper: None,
            auth: McpClientAuth::None,
        };
        let client = McpHttpClient::new("test-server", &transport);
        assert!(client.is_ok());
        let client = client.unwrap();
        assert_eq!(client.url, "http://localhost:8080/mcp");
        assert_eq!(client.server_name, "test-server");
    }
}
