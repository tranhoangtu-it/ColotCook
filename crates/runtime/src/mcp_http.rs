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
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::Client;
use tokio::sync::RwLock;

use crate::config::McpOAuthConfig;
use crate::mcp_client::{McpClientAuth, McpRemoteTransport, DEFAULT_MCP_TOOL_CALL_TIMEOUT_MS};
use crate::mcp_stdio::{
    JsonRpcId, JsonRpcRequest, JsonRpcResponse, McpInitializeClientInfo, McpInitializeParams,
    McpInitializeResult, McpListToolsParams, McpListToolsResult, McpServerManagerError,
    McpToolCallParams, McpToolCallResult,
};
use crate::oauth::{
    load_mcp_oauth_credentials, save_mcp_oauth_credentials, clear_mcp_oauth_credentials,
    OAuthRefreshRequest, OAuthTokenSet,
};

const DEFAULT_HTTP_TIMEOUT_MS: u64 = 30_000;
const TOKEN_EXPIRY_BUFFER_SECS: u64 = 30;

/// Manages OAuth tokens for MCP servers, including caching, expiry detection, and refresh.
#[derive(Debug, Clone)]
struct McpOAuthTokenManager {
    server_name: String,
    oauth_config: McpOAuthConfig,
    http_client: Client,
    token: Arc<RwLock<Option<OAuthTokenSet>>>,
}

impl McpOAuthTokenManager {
    /// Create a new token manager and load any saved credentials for the server.
    async fn new(
        server_name: &str,
        oauth_config: McpOAuthConfig,
        http_client: Client,
    ) -> Result<Self, McpServerManagerError> {
        let manager = Self {
            server_name: server_name.to_string(),
            oauth_config,
            http_client,
            token: Arc::new(RwLock::new(None)),
        };

        // Try to load saved credentials for this server
        match load_mcp_oauth_credentials(server_name) {
            Ok(Some(token_set)) => {
                *manager.token.write().await = Some(token_set);
            }
            Ok(None) => {
                // No saved credentials, will require authentication on first use
            }
            Err(e) => {
                // Log but don't fail initialization; auth might be configured differently
                #[cfg(debug_assertions)]
                eprintln!("[MCP] warning: failed to load saved OAuth credentials for {server_name}: {e}");
            }
        }

        Ok(manager)
    }

    /// Get a valid access token, refreshing if necessary.
    async fn get_valid_token(&self) -> Result<String, McpServerManagerError> {
        let token = self.token.read().await;

        if let Some(ref token_set) = *token {
            if !Self::is_token_expired(token_set) {
                return Ok(token_set.access_token.clone());
            }
        }
        drop(token);

        // Token is expired or missing; try to refresh if we have a refresh_token
        let refresh_token = {
            let token = self.token.read().await;
            token.as_ref().and_then(|t| t.refresh_token.clone())
        };

        if let Some(refresh_token) = refresh_token {
            self.refresh_token(&refresh_token).await?;
            let token = self.token.read().await;
            if let Some(ref token_set) = *token {
                return Ok(token_set.access_token.clone());
            }
        }

        // No valid token and no way to refresh; prompt user to authenticate
        Err(McpServerManagerError::Transport {
            server_name: self.server_name.clone(),
            method: "oauth_token_refresh",
            source: io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "no valid OAuth credentials for MCP server '{}'. \
                    Please run: colotcook login --server {}",
                    self.server_name, self.server_name
                ),
            ),
        })
    }

    /// Refresh the OAuth token using the refresh_token.
    async fn refresh_token(&self, refresh_token: &str) -> Result<(), McpServerManagerError> {
        // Construct the token endpoint URL from auth_server_metadata_url
        let token_url = match &self.oauth_config.auth_server_metadata_url {
            Some(base_url) => {
                let base = base_url.trim_end_matches('/');
                format!("{base}/token")
            }
            None => {
                return Err(McpServerManagerError::Transport {
                    server_name: self.server_name.clone(),
                    method: "oauth_refresh",
                    source: io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "auth_server_metadata_url not configured; cannot refresh token",
                    ),
                });
            }
        };

        // Build the refresh request
        let refresh_request = OAuthRefreshRequest {
            grant_type: "refresh_token",
            refresh_token: refresh_token.to_string(),
            client_id: self
                .oauth_config
                .client_id
                .as_ref()
                .ok_or_else(|| McpServerManagerError::Transport {
                    server_name: self.server_name.clone(),
                    method: "oauth_refresh",
                    source: io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "client_id not configured",
                    ),
                })?
                .clone(),
            scopes: vec![], // Keep existing scopes from saved token
        };

        let response = self
            .http_client
            .post(&token_url)
            .header("content-type", "application/x-www-form-urlencoded")
            .form(&refresh_request.form_params())
            .send()
            .await
            .map_err(|e| McpServerManagerError::Transport {
                server_name: self.server_name.clone(),
                method: "oauth_refresh",
                source: io::Error::new(io::ErrorKind::Other, e.to_string()),
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(McpServerManagerError::Transport {
                server_name: self.server_name.clone(),
                method: "oauth_refresh",
                source: io::Error::new(
                    io::ErrorKind::Other,
                    format!("token refresh failed: HTTP {status}: {body}"),
                ),
            });
        }

        let new_token_set: OAuthTokenSet = response.json().await.map_err(|e| {
            McpServerManagerError::Transport {
                server_name: self.server_name.clone(),
                method: "oauth_refresh",
                source: io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("failed to parse token response: {e}"),
                ),
            }
        })?;

        // Save the refreshed token and update the in-memory cache
        save_mcp_oauth_credentials(&self.server_name, &new_token_set).map_err(|e| {
            McpServerManagerError::Transport {
                server_name: self.server_name.clone(),
                method: "oauth_refresh",
                source: e,
            }
        })?;

        *self.token.write().await = Some(new_token_set);
        Ok(())
    }

    /// Check if a token is expired, with a 30-second buffer before actual expiry.
    fn is_token_expired(token: &OAuthTokenSet) -> bool {
        if let Some(expires_at) = token.expires_at {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            expires_at <= now + TOKEN_EXPIRY_BUFFER_SECS
        } else {
            false
        }
    }

    /// Clear the token from both cache and persistent storage.
    async fn clear(&self) -> Result<(), McpServerManagerError> {
        clear_mcp_oauth_credentials(&self.server_name).map_err(|e| {
            McpServerManagerError::Transport {
                server_name: self.server_name.clone(),
                method: "oauth_clear",
                source: e,
            }
        })?;
        *self.token.write().await = None;
        Ok(())
    }
}

/// HTTP-based MCP client that communicates with remote MCP servers via JSON-RPC over HTTP POST.
#[derive(Debug)]
pub struct McpHttpClient {
    client: Client,
    url: String,
    headers: BTreeMap<String, String>,
    auth: McpClientAuth,
    server_name: String,
    token_manager: Option<McpOAuthTokenManager>,
}

impl McpHttpClient {
    /// Create a new HTTP MCP client from a remote transport configuration.
    pub async fn new(server_name: &str, transport: &McpRemoteTransport) -> Result<Self, io::Error> {
        let client = Client::builder()
            .user_agent("colotcook/0.1")
            .build()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        let token_manager = if let McpClientAuth::OAuth(ref oauth_config) = transport.auth {
            if oauth_config.client_id.is_some() {
                // Try to initialize the token manager, but don't fail if we can't load saved credentials
                let manager = McpOAuthTokenManager::new(server_name, oauth_config.clone(), client.clone())
                    .await
                    .ok();
                Some(manager)
            } else {
                None
            }
        } else {
            None
        };

        Ok(Self {
            client,
            url: transport.url.clone(),
            headers: transport.headers.clone(),
            auth: transport.auth.clone(),
            server_name: server_name.to_string(),
            token_manager: token_manager.flatten(),
        })
    }

    /// Apply custom headers and authentication to a request builder.
    async fn apply_headers_and_auth(
        &self,
        mut builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, McpServerManagerError> {
        for (key, value) in &self.headers {
            builder = builder.header(key.as_str(), value.as_str());
        }

        // Apply OAuth token if a token manager is available
        if let Some(ref token_manager) = self.token_manager {
            let token = token_manager.get_valid_token().await?;
            builder = builder.bearer_auth(token);
        }

        Ok(builder)
    }

    /// Send a JSON-RPC request and return the parsed response.
    /// Retries once on 401 (Unauthorized) by clearing cached OAuth tokens.
    async fn send_request<P: serde::Serialize + Send + Sync + 'static, R: serde::de::DeserializeOwned + Send + 'static>(
        &self,
        method: &'static str,
        request: JsonRpcRequest<P>,
        timeout_ms: u64,
    ) -> Result<JsonRpcResponse<R>, McpServerManagerError> {
        self.send_request_internal(method, request, timeout_ms, false)
            .await
    }

    /// Internal implementation of send_request with retry logic.
    fn send_request_internal<'a, P: serde::Serialize + Send + Sync + 'a, R: serde::de::DeserializeOwned + Send + 'a>(
        &'a self,
        method: &'static str,
        request: JsonRpcRequest<P>,
        timeout_ms: u64,
        is_retry: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<JsonRpcResponse<R>, McpServerManagerError>> + Send + 'a>> {
        Box::pin(async move {
        let builder = self
            .client
            .post(&self.url)
            .timeout(Duration::from_millis(timeout_ms))
            .header("Content-Type", "application/json");

        let builder = self.apply_headers_and_auth(builder).await?;

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

        // Handle 401 Unauthorized: clear the cached token and retry once
        if response.status() == 401 && !is_retry {
            if let Some(ref token_manager) = self.token_manager {
                let _ = token_manager.clear().await;
            }
            return self
                .send_request_internal(method, request, timeout_ms, true)
                .await;
        }

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
        }) // end Box::pin(async move { ... })
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

        let builder = self.apply_headers_and_auth(builder).await;

        // Per MCP spec, the initialized notification is fire-and-forget.
        // Log the error for debugging but don't fail the handshake.
        match builder {
            Ok(builder) => {
                if let Err(_e) = builder.json(&notification).send().await {
                    #[cfg(debug_assertions)]
                    eprintln!(
                        "[MCP] warning: failed to send initialized notification to {}: {_e}",
                        self.server_name
                    );
                }
            }
            Err(_e) => {
                #[cfg(debug_assertions)]
                eprintln!(
                    "[MCP] warning: failed to prepare initialized notification to {}: {_e}",
                    self.server_name
                );
            }
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
    use crate::config::McpOAuthConfig;
    use crate::mcp_client::McpRemoteTransport;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn creates_http_client_from_transport() {
        let transport = McpRemoteTransport {
            url: "http://localhost:8080/mcp".to_string(),
            headers: BTreeMap::from([("X-Api-Key".to_string(), "test-key".to_string())]),
            headers_helper: None,
            auth: McpClientAuth::None,
        };
        let client = tokio::runtime::Runtime::new().unwrap().block_on(
            McpHttpClient::new("test-server", &transport),
        );
        assert!(client.is_ok());
        let client = client.unwrap();
        assert_eq!(client.url, "http://localhost:8080/mcp");
        assert_eq!(client.server_name, "test-server");
    }

    #[test]
    fn creates_http_client_with_oauth_auth() {
        let oauth_config = McpOAuthConfig {
            client_id: Some("test-client-id".to_string()),
            callback_port: Some(4545),
            auth_server_metadata_url: Some("https://auth.example.com".to_string()),
            xaa: None,
        };
        let transport = McpRemoteTransport {
            url: "http://localhost:8080/mcp".to_string(),
            headers: BTreeMap::new(),
            headers_helper: None,
            auth: McpClientAuth::OAuth(oauth_config),
        };
        let client = tokio::runtime::Runtime::new().unwrap().block_on(
            McpHttpClient::new("test-server", &transport),
        );
        assert!(client.is_ok());
        let client = client.unwrap();
        assert!(client.token_manager.is_some());
    }

    #[test]
    fn detects_expired_tokens() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let expired_token = OAuthTokenSet {
            access_token: "expired".to_string(),
            refresh_token: None,
            expires_at: Some(now - 60), // Expired 1 minute ago
            scopes: vec![],
        };
        assert!(McpOAuthTokenManager::is_token_expired(&expired_token));

        let valid_token = OAuthTokenSet {
            access_token: "valid".to_string(),
            refresh_token: None,
            expires_at: Some(now + 3600), // Expires in 1 hour
            scopes: vec![],
        };
        assert!(!McpOAuthTokenManager::is_token_expired(&valid_token));

        // Token expiring within buffer (30s)
        let almost_expired = OAuthTokenSet {
            access_token: "almost".to_string(),
            refresh_token: None,
            expires_at: Some(now + 10), // Expires in 10s
            scopes: vec![],
        };
        assert!(McpOAuthTokenManager::is_token_expired(&almost_expired));

        // Token with no expiry
        let no_expiry = OAuthTokenSet {
            access_token: "no_expiry".to_string(),
            refresh_token: None,
            expires_at: None,
            scopes: vec![],
        };
        assert!(!McpOAuthTokenManager::is_token_expired(&no_expiry));
    }
}
