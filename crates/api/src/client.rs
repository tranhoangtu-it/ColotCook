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
    pub fn from_model(model: &str) -> Result<Self, ApiError> {
        Self::from_model_with_anthropic_auth(model, None)
    }

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

    #[must_use]
    pub fn with_prompt_cache(self, prompt_cache: PromptCache) -> Self {
        match self {
            Self::Anthropic(client) => Self::Anthropic(client.with_prompt_cache(prompt_cache)),
            other => other,
        }
    }

    #[must_use]
    pub fn prompt_cache_stats(&self) -> Option<PromptCacheStats> {
        match self {
            Self::Anthropic(client) => client.prompt_cache_stats(),
            _ => None,
        }
    }

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

    pub async fn send_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageResponse, ApiError> {
        match self {
            Self::Anthropic(client) => send_via_provider(client, request).await,
            _ => {
                let client = self.openai_compat_client().expect("non-Anthropic provider");
                send_via_provider(client, request).await
            }
        }
    }

    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageStream, ApiError> {
        match self {
            Self::Anthropic(client) => stream_via_provider(client, request)
                .await
                .map(MessageStream::Anthropic),
            _ => {
                let client = self.openai_compat_client().expect("non-Anthropic provider");
                stream_via_provider(client, request)
                    .await
                    .map(MessageStream::OpenAiCompat)
            }
        }
    }
}

#[derive(Debug)]
pub enum MessageStream {
    Anthropic(anthropic::MessageStream),
    OpenAiCompat(openai_compat::MessageStream),
}

impl MessageStream {
    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Anthropic(stream) => stream.request_id(),
            Self::OpenAiCompat(stream) => stream.request_id(),
        }
    }

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

#[must_use]
pub fn read_base_url() -> String {
    anthropic::read_base_url()
}

#[must_use]
pub fn read_xai_base_url() -> String {
    openai_compat::read_base_url(OpenAiCompatConfig::xai())
}

#[must_use]
pub fn read_gemini_base_url() -> String {
    openai_compat::read_base_url(OpenAiCompatConfig::gemini())
}

#[must_use]
pub fn read_ollama_base_url() -> String {
    openai_compat::read_base_url(OpenAiCompatConfig::ollama())
}

#[cfg(test)]
mod tests {
    use crate::providers::{detect_provider_kind, resolve_model_alias, ProviderKind};

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
}
