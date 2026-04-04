//! OAuth login/logout flow and CLI auth-source resolution.

use std::env;
use std::io::{self, Read, Write};
use std::net::TcpListener;

use colotcook_api as api;
use colotcook_api::{resolve_startup_auth_source, AnthropicClient, AuthSource};
use colotcook_runtime::{
    clear_oauth_credentials, generate_pkce_pair, generate_state,
    parse_oauth_callback_request_target, save_oauth_credentials, ConfigLoader,
    OAuthAuthorizationRequest, OAuthConfig, OAuthTokenExchangeRequest,
};

use crate::util::open_browser;

/// Default callback port for OAuth flow.
pub(crate) const DEFAULT_OAUTH_CALLBACK_PORT: u16 = 4545;

/// Build the default OAuth configuration for Claude platform.
pub(crate) fn default_oauth_config() -> OAuthConfig {
    OAuthConfig {
        client_id: String::from("9d1c250a-e61b-44d9-88ed-5944d1962f5e"),
        authorize_url: String::from("https://platform.claude.com/oauth/authorize"),
        token_url: String::from("https://platform.claude.com/v1/oauth/token"),
        callback_port: None,
        manual_redirect_url: None,
        scopes: vec![
            String::from("user:profile"),
            String::from("user:inference"),
            String::from("user:sessions:colotcook"),
        ],
    }
}

/// Run the OAuth login flow: authorize, exchange code, save credentials.
pub(crate) fn run_login() -> Result<(), Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let config = ConfigLoader::default_for(&cwd).load()?;
    let default_oauth = default_oauth_config();
    let oauth = config.oauth().unwrap_or(&default_oauth);
    let callback_port = oauth.callback_port.unwrap_or(DEFAULT_OAUTH_CALLBACK_PORT);
    let redirect_uri = colotcook_runtime::loopback_redirect_uri(callback_port);
    let pkce = generate_pkce_pair()?;
    let state = generate_state()?;
    let authorize_url =
        OAuthAuthorizationRequest::from_config(oauth, redirect_uri.clone(), state.clone(), &pkce)
            .build_url();

    println!("Starting Claude OAuth login...");
    println!("Listening for callback on {redirect_uri}");
    if let Err(error) = open_browser(&authorize_url) {
        eprintln!("warning: failed to open browser automatically: {error}");
        println!("Open this URL manually:\n{authorize_url}");
    }

    let callback = wait_for_oauth_callback(callback_port)?;
    if let Some(error) = callback.error {
        let description = callback
            .error_description
            .unwrap_or_else(|| "authorization failed".to_string());
        return Err(io::Error::other(format!("{error}: {description}")).into());
    }
    let code = callback.code.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "callback did not include code")
    })?;
    let returned_state = callback.state.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "callback did not include state")
    })?;
    if returned_state != state {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "oauth state mismatch").into());
    }

    let client = AnthropicClient::from_auth(AuthSource::None).with_base_url(api::read_base_url());
    let exchange_request =
        OAuthTokenExchangeRequest::from_config(oauth, code, state, pkce.verifier, redirect_uri);
    let runtime = tokio::runtime::Runtime::new()?;
    let token_set = runtime.block_on(client.exchange_oauth_code(oauth, &exchange_request))?;
    save_oauth_credentials(&colotcook_runtime::OAuthTokenSet {
        access_token: token_set.access_token,
        refresh_token: token_set.refresh_token,
        expires_at: token_set.expires_at,
        scopes: token_set.scopes,
    })?;
    println!("Claude OAuth login complete.");
    Ok(())
}

/// Clear stored OAuth credentials.
pub(crate) fn run_logout() -> Result<(), Box<dyn std::error::Error>> {
    clear_oauth_credentials()?;
    println!("Claude OAuth credentials cleared.");
    Ok(())
}

/// Listen for the OAuth redirect on localhost and parse the callback params.
pub(crate) fn wait_for_oauth_callback(
    port: u16,
) -> Result<colotcook_runtime::OAuthCallbackParams, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let (mut stream, _) = listener.accept()?;
    let mut buffer = [0_u8; 4096];
    let bytes_read = stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
    let request_line = request.lines().next().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "missing callback request line")
    })?;
    let target = request_line.split_whitespace().nth(1).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "missing callback request target",
        )
    })?;
    let callback = parse_oauth_callback_request_target(target)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let body = if callback.error.is_some() {
        "Claude OAuth login failed. You can close this window."
    } else {
        "Claude OAuth login succeeded. You can close this window."
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes())?;
    Ok(callback)
}

/// Resolve the auth source for the CLI from environment or config.
pub(crate) fn resolve_cli_auth_source() -> Result<AuthSource, Box<dyn std::error::Error>> {
    Ok(resolve_startup_auth_source(|| {
        let cwd = env::current_dir().map_err(api::ApiError::from)?;
        let config = ConfigLoader::default_for(&cwd).load().map_err(|error| {
            api::ApiError::Auth(format!("failed to load runtime OAuth config: {error}"))
        })?;
        Ok(config.oauth().cloned())
    })?)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- default_oauth_config ---

    #[test]
    fn default_oauth_config_has_client_id() {
        let config = default_oauth_config();
        assert!(!config.client_id.is_empty());
    }

    #[test]
    fn default_oauth_config_has_authorize_url() {
        let config = default_oauth_config();
        assert!(config.authorize_url.starts_with("https://"));
    }

    #[test]
    fn default_oauth_config_has_token_url() {
        let config = default_oauth_config();
        assert!(config.token_url.starts_with("https://"));
    }

    #[test]
    fn default_oauth_config_scopes_not_empty() {
        let config = default_oauth_config();
        assert!(!config.scopes.is_empty());
    }

    #[test]
    fn default_oauth_config_scopes_contain_inference() {
        let config = default_oauth_config();
        assert!(config.scopes.iter().any(|s| s.contains("inference")));
    }

    #[test]
    fn default_oauth_config_callback_port_is_none() {
        let config = default_oauth_config();
        assert!(config.callback_port.is_none());
    }

    #[test]
    fn default_oauth_config_manual_redirect_is_none() {
        let config = default_oauth_config();
        assert!(config.manual_redirect_url.is_none());
    }

    #[test]
    fn default_oauth_callback_port_value() {
        assert_eq!(DEFAULT_OAUTH_CALLBACK_PORT, 4545);
    }
}
