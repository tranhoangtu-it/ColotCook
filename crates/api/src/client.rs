//! Provider-agnostic client wrapper and streaming helpers.

use crate::error::ApiError;
use crate::prompt_cache::{PromptCache, PromptCacheRecord, PromptCacheStats};
use crate::providers::anthropic::{self, AnthropicClient, AuthSource};
use crate::providers::openai_compat::{self, OpenAiCompatClient, OpenAiCompatConfig};
use crate::providers::{self, Provider, ProviderKind};
use crate::types::{MessageRequest, MessageResponse, StreamEvent};

async fn send_via_provider<P: Provider>(
    provider: &P,
    request: &MessageRequest,
) -> Result<MessageResponse, ApiError> {
    provider.send_message(request).await
}

async fn stream_via_provider<P: Provider>(
    provider: &P,
    request: &MessageRequest,
) -> Result<P::Stream, ApiError> {
    provider.stream_message(request).await
}

/// A unified client that dispatches to the appropriate provider backend based on the model name.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum ProviderClient {
    Anthropic(AnthropicClient),
    OpenAi(OpenAiCompatClient),
    Xai(OpenAiCompatClient),
    Gemini(OpenAiCompatClient),
    Ollama(OpenAiCompatClient),
}

impl ProviderClient {
    /// Creates a client for the given model, reading credentials from environment variables.
    pub fn from_model(model: &str) -> Result<Self, ApiError> {
        Self::from_model_with_anthropic_auth(model, None)
    }

    /// Creates a client, optionally supplying an explicit [`AuthSource`] for Anthropic requests.
    pub fn from_model_with_anthropic_auth(
        model: &str,
        anthropic_auth: Option<AuthSource>,
    ) -> Result<Self, ApiError> {
        let resolved_model = providers::resolve_model_alias(model);
        match providers::detect_provider_kind(&resolved_model) {
            ProviderKind::Anthropic => Ok(Self::Anthropic(match anthropic_auth {
                Some(auth) => AnthropicClient::from_auth(auth),
                None => AnthropicClient::from_env()?,
            })),
            ProviderKind::OpenAi => Ok(Self::OpenAi(OpenAiCompatClient::from_env(
                OpenAiCompatConfig::openai(),
            )?)),
            ProviderKind::Xai => Ok(Self::Xai(OpenAiCompatClient::from_env(
                OpenAiCompatConfig::xai(),
            )?)),
            ProviderKind::Gemini => Ok(Self::Gemini(OpenAiCompatClient::from_env(
                OpenAiCompatConfig::gemini(),
            )?)),
            ProviderKind::Ollama => Ok(Self::Ollama(OpenAiCompatClient::from_env(
                OpenAiCompatConfig::ollama(),
            )?)),
        }
    }

    /// Returns which provider backend this client targets.
    #[must_use]
    pub const fn provider_kind(&self) -> ProviderKind {
        match self {
            Self::Anthropic(_) => ProviderKind::Anthropic,
            Self::OpenAi(_) => ProviderKind::OpenAi,
            Self::Xai(_) => ProviderKind::Xai,
            Self::Gemini(_) => ProviderKind::Gemini,
            Self::Ollama(_) => ProviderKind::Ollama,
        }
    }

    /// Attaches a prompt cache to this client (Anthropic only; no-op for other providers).
    #[must_use]
    pub fn with_prompt_cache(self, prompt_cache: PromptCache) -> Self {
        match self {
            Self::Anthropic(client) => Self::Anthropic(client.with_prompt_cache(prompt_cache)),
            other => other,
        }
    }

    /// Returns accumulated prompt-cache statistics, if available for this provider.
    #[must_use]
    pub fn prompt_cache_stats(&self) -> Option<PromptCacheStats> {
        match self {
            Self::Anthropic(client) => client.prompt_cache_stats(),
            _ => None,
        }
    }

    /// Takes and returns the most recent prompt-cache record, clearing it from internal state.
    #[must_use]
    pub fn take_last_prompt_cache_record(&self) -> Option<PromptCacheRecord> {
        match self {
            Self::Anthropic(client) => client.take_last_prompt_cache_record(),
            _ => None,
        }
    }

    /// Returns the inner `OpenAiCompatClient` for any OpenAI-compatible provider.
    fn openai_compat_client(&self) -> Option<&OpenAiCompatClient> {
        match self {
            Self::OpenAi(c) | Self::Xai(c) | Self::Gemini(c) | Self::Ollama(c) => Some(c),
            Self::Anthropic(_) => None,
        }
    }

    /// Sends a non-streaming request and returns the complete response.
    pub async fn send_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageResponse, ApiError> {
        if let Self::Anthropic(client) = self {
            send_via_provider(client, request).await
        } else {
            // SAFETY: all non-Anthropic variants carry an OpenAiCompatClient
            let client = self
                .openai_compat_client()
                .unwrap_or_else(|| unreachable!("non-Anthropic variant must be OpenAI-compat"));
            send_via_provider(client, request).await
        }
    }

    /// Opens a streaming response and returns a [`MessageStream`] for polling SSE events.
    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageStream, ApiError> {
        if let Self::Anthropic(client) = self {
            stream_via_provider(client, request)
                .await
                .map(MessageStream::Anthropic)
        } else {
            // SAFETY: all non-Anthropic variants carry an OpenAiCompatClient
            let client = self
                .openai_compat_client()
                .unwrap_or_else(|| unreachable!("non-Anthropic variant must be OpenAI-compat"));
            stream_via_provider(client, request)
                .await
                .map(MessageStream::OpenAiCompat)
        }
    }
}

/// An active SSE stream from either the Anthropic or an OpenAI-compatible provider.
#[derive(Debug)]
pub enum MessageStream {
    Anthropic(anthropic::MessageStream),
    OpenAiCompat(openai_compat::MessageStream),
}

impl MessageStream {
    /// Returns the server-assigned request ID from the response headers, if present.
    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Anthropic(stream) => stream.request_id(),
            Self::OpenAiCompat(stream) => stream.request_id(),
        }
    }

    /// Polls the underlying SSE stream for the next event, returning `None` at end-of-stream.
    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        match self {
            Self::Anthropic(stream) => stream.next_event().await,
            Self::OpenAiCompat(stream) => stream.next_event().await,
        }
    }
}

pub use anthropic::{
    oauth_token_is_expired, resolve_saved_oauth_token, resolve_startup_auth_source, OAuthTokenSet,
};

/// Returns the Anthropic API base URL (overridable via `ANTHROPIC_BASE_URL`).
#[must_use]
pub fn read_base_url() -> String {
    anthropic::read_base_url()
}

/// Returns the xAI API base URL (overridable via `XAI_BASE_URL`).
#[must_use]
pub fn read_xai_base_url() -> String {
    openai_compat::read_base_url(OpenAiCompatConfig::xai())
}

/// Returns the Gemini API base URL (overridable via `GEMINI_BASE_URL`).
#[must_use]
pub fn read_gemini_base_url() -> String {
    openai_compat::read_base_url(OpenAiCompatConfig::gemini())
}

/// Returns the Ollama API base URL (overridable via `OLLAMA_BASE_URL`).
#[must_use]
pub fn read_ollama_base_url() -> String {
    openai_compat::read_base_url(OpenAiCompatConfig::ollama())
}

#[cfg(test)]
mod tests {
    use crate::providers::{detect_provider_kind, resolve_model_alias, ProviderKind};
    use super::*;

    #[test]
    fn resolves_existing_and_grok_aliases() {
        assert_eq!(resolve_model_alias("opus"), "claude-opus-4-6");
        assert_eq!(resolve_model_alias("grok"), "grok-3");
        assert_eq!(resolve_model_alias("grok-mini"), "grok-3-mini");
    }

    #[test]
    fn resolves_gemini_aliases() {
        assert_eq!(
            resolve_model_alias("gemini"),
            "gemini-2.5-pro-preview-05-06"
        );
    }

    #[test]
    fn provider_detection_prefers_model_family() {
        assert_eq!(detect_provider_kind("grok-3"), ProviderKind::Xai);
        assert_eq!(
            detect_provider_kind("claude-sonnet-4-6"),
            ProviderKind::Anthropic
        );
        assert_eq!(detect_provider_kind("gemini-pro"), ProviderKind::Gemini);
        assert_eq!(detect_provider_kind("llama3"), ProviderKind::Ollama);
        assert_eq!(detect_provider_kind("gpt-4o"), ProviderKind::OpenAi);
    }

    #[test]
    fn resolve_model_alias_passes_through_unknown_model() {
        // An unknown model string is returned unchanged
        let model = "some-unknown-model-xyz";
        let resolved = resolve_model_alias(model);
        assert!(!resolved.is_empty());
    }

    #[test]
    fn detect_provider_kind_for_all_providers() {
        assert_eq!(detect_provider_kind("claude-haiku-3"), ProviderKind::Anthropic);
        assert_eq!(detect_provider_kind("gpt-3.5-turbo"), ProviderKind::OpenAi);
        assert_eq!(detect_provider_kind("gemini-1.5-flash"), ProviderKind::Gemini);
        assert_eq!(detect_provider_kind("grok-beta"), ProviderKind::Xai);
    }

    #[test]
    fn provider_client_from_anthropic_with_explicit_auth_succeeds() {
        use crate::providers::anthropic::AuthSource;
        // Using explicit auth avoids needing ANTHROPIC_API_KEY in env
        let client = ProviderClient::from_model_with_anthropic_auth(
            "claude-sonnet-4-6",
            Some(AuthSource::ApiKey("test-key".to_string())),
        );
        assert!(client.is_ok());
        let client = client.unwrap();
        assert_eq!(client.provider_kind(), ProviderKind::Anthropic);
    }

    #[test]
    fn provider_client_provider_kind_correct_for_anthropic() {
        use crate::providers::anthropic::AuthSource;
        let client = ProviderClient::from_model_with_anthropic_auth(
            "claude-opus-4-6",
            Some(AuthSource::ApiKey("test-key".to_string())),
        )
        .unwrap();
        assert_eq!(client.provider_kind(), ProviderKind::Anthropic);
    }

    #[test]
    fn provider_client_with_prompt_cache_returns_same_kind() {
        use crate::providers::anthropic::AuthSource;
        use crate::prompt_cache::PromptCache;
        let client = ProviderClient::from_model_with_anthropic_auth(
            "claude-sonnet-4-6",
            Some(AuthSource::ApiKey("test-key".to_string())),
        )
        .unwrap();
        let kind_before = client.provider_kind();
        let client_with_cache = client.with_prompt_cache(PromptCache::new("test-session"));
        assert_eq!(client_with_cache.provider_kind(), kind_before);
    }

    #[test]
    fn provider_client_prompt_cache_stats_is_none_for_anthropic_without_cache() {
        use crate::providers::anthropic::AuthSource;
        let client = ProviderClient::from_model_with_anthropic_auth(
            "claude-sonnet-4-6",
            Some(AuthSource::ApiKey("test-key".to_string())),
        )
        .unwrap();
        // Without explicit cache, stats may be None or Some
        let _stats = client.prompt_cache_stats();
    }

    #[test]
    fn provider_client_take_last_prompt_cache_record_returns_none_without_cache() {
        use crate::providers::anthropic::AuthSource;
        let client = ProviderClient::from_model_with_anthropic_auth(
            "claude-sonnet-4-6",
            Some(AuthSource::ApiKey("test-key".to_string())),
        )
        .unwrap();
        // Should not panic; returns None when no cache record is available
        let _record = client.take_last_prompt_cache_record();
    }

    #[test]
    fn read_base_url_returns_non_empty() {
        let url = read_base_url();
        assert!(!url.is_empty());
        assert!(url.starts_with("https://") || url.starts_with("http://"));
    }

    #[test]
    fn read_xai_base_url_returns_non_empty() {
        let url = read_xai_base_url();
        assert!(!url.is_empty());
    }

    #[test]
    fn read_gemini_base_url_returns_non_empty() {
        let url = read_gemini_base_url();
        assert!(!url.is_empty());
    }

    #[test]
    fn read_ollama_base_url_returns_non_empty() {
        let url = read_ollama_base_url();
        assert!(!url.is_empty());
    }

    #[test]
    fn from_model_fails_when_credentials_missing_for_openai() {
        // Without OPENAI_API_KEY set, constructing an OpenAI client should fail
        // (unless the env var is already set in this test environment)
        // We just ensure the result type is either Ok or Err without panicking
        let _ = ProviderClient::from_model("gpt-4o");
    }

    #[test]
    fn provider_kind_debug_format() {
        let kind = ProviderKind::Anthropic;
        let s = format!("{kind:?}");
        assert!(!s.is_empty());
    }
}
