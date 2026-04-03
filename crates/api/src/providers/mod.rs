use std::future::Future;
use std::pin::Pin;

use crate::error::ApiError;
use crate::types::{MessageRequest, MessageResponse};

pub mod anthropic;
pub mod openai_compat;

pub type ProviderFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ApiError>> + Send + 'a>>;

pub trait Provider {
    type Stream;

    fn send_message<'a>(
        &'a self,
        request: &'a MessageRequest,
    ) -> ProviderFuture<'a, MessageResponse>;

    fn stream_message<'a>(
        &'a self,
        request: &'a MessageRequest,
    ) -> ProviderFuture<'a, Self::Stream>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Anthropic,
    OpenAi,
    Xai,
    Gemini,
    Ollama,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderMetadata {
    pub provider: ProviderKind,
    pub auth_env: &'static str,
    pub base_url_env: &'static str,
    pub default_base_url: &'static str,
}

const MODEL_REGISTRY: &[(&str, ProviderMetadata)] = &[
    // ── Anthropic ───────────────────────────────────────────
    (
        "opus",
        ProviderMetadata {
            provider: ProviderKind::Anthropic,
            auth_env: "ANTHROPIC_API_KEY",
            base_url_env: "ANTHROPIC_BASE_URL",
            default_base_url: anthropic::DEFAULT_BASE_URL,
        },
    ),
    (
        "sonnet",
        ProviderMetadata {
            provider: ProviderKind::Anthropic,
            auth_env: "ANTHROPIC_API_KEY",
            base_url_env: "ANTHROPIC_BASE_URL",
            default_base_url: anthropic::DEFAULT_BASE_URL,
        },
    ),
    (
        "haiku",
        ProviderMetadata {
            provider: ProviderKind::Anthropic,
            auth_env: "ANTHROPIC_API_KEY",
            base_url_env: "ANTHROPIC_BASE_URL",
            default_base_url: anthropic::DEFAULT_BASE_URL,
        },
    ),
    // ── xAI / Grok ──────────────────────────────────────────
    (
        "grok",
        ProviderMetadata {
            provider: ProviderKind::Xai,
            auth_env: "XAI_API_KEY",
            base_url_env: "XAI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_XAI_BASE_URL,
        },
    ),
    (
        "grok-3",
        ProviderMetadata {
            provider: ProviderKind::Xai,
            auth_env: "XAI_API_KEY",
            base_url_env: "XAI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_XAI_BASE_URL,
        },
    ),
    (
        "grok-mini",
        ProviderMetadata {
            provider: ProviderKind::Xai,
            auth_env: "XAI_API_KEY",
            base_url_env: "XAI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_XAI_BASE_URL,
        },
    ),
    (
        "grok-3-mini",
        ProviderMetadata {
            provider: ProviderKind::Xai,
            auth_env: "XAI_API_KEY",
            base_url_env: "XAI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_XAI_BASE_URL,
        },
    ),
    (
        "grok-2",
        ProviderMetadata {
            provider: ProviderKind::Xai,
            auth_env: "XAI_API_KEY",
            base_url_env: "XAI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_XAI_BASE_URL,
        },
    ),
    // ── Google Gemini ───────────────────────────────────────
    (
        "gemini",
        ProviderMetadata {
            provider: ProviderKind::Gemini,
            auth_env: "GEMINI_API_KEY",
            base_url_env: "GEMINI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_GEMINI_BASE_URL,
        },
    ),
    (
        "gemini-pro",
        ProviderMetadata {
            provider: ProviderKind::Gemini,
            auth_env: "GEMINI_API_KEY",
            base_url_env: "GEMINI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_GEMINI_BASE_URL,
        },
    ),
    (
        "gemini-2.5-pro",
        ProviderMetadata {
            provider: ProviderKind::Gemini,
            auth_env: "GEMINI_API_KEY",
            base_url_env: "GEMINI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_GEMINI_BASE_URL,
        },
    ),
    (
        "gemini-2.5-flash",
        ProviderMetadata {
            provider: ProviderKind::Gemini,
            auth_env: "GEMINI_API_KEY",
            base_url_env: "GEMINI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_GEMINI_BASE_URL,
        },
    ),
    // ── Ollama (local) ──────────────────────────────────────
    (
        "ollama",
        ProviderMetadata {
            provider: ProviderKind::Ollama,
            auth_env: "OLLAMA_API_KEY",
            base_url_env: "OLLAMA_BASE_URL",
            default_base_url: openai_compat::DEFAULT_OLLAMA_BASE_URL,
        },
    ),
    (
        "llama3",
        ProviderMetadata {
            provider: ProviderKind::Ollama,
            auth_env: "OLLAMA_API_KEY",
            base_url_env: "OLLAMA_BASE_URL",
            default_base_url: openai_compat::DEFAULT_OLLAMA_BASE_URL,
        },
    ),
    (
        "codellama",
        ProviderMetadata {
            provider: ProviderKind::Ollama,
            auth_env: "OLLAMA_API_KEY",
            base_url_env: "OLLAMA_BASE_URL",
            default_base_url: openai_compat::DEFAULT_OLLAMA_BASE_URL,
        },
    ),
    (
        "deepseek-coder",
        ProviderMetadata {
            provider: ProviderKind::Ollama,
            auth_env: "OLLAMA_API_KEY",
            base_url_env: "OLLAMA_BASE_URL",
            default_base_url: openai_compat::DEFAULT_OLLAMA_BASE_URL,
        },
    ),
    (
        "qwen2.5-coder",
        ProviderMetadata {
            provider: ProviderKind::Ollama,
            auth_env: "OLLAMA_API_KEY",
            base_url_env: "OLLAMA_BASE_URL",
            default_base_url: openai_compat::DEFAULT_OLLAMA_BASE_URL,
        },
    ),
];

#[must_use]
pub fn resolve_model_alias(model: &str) -> String {
    let trimmed = model.trim();
    // Strip "ollama:" prefix — Ollama models use the tag after the colon
    if let Some(ollama_model) = trimmed
        .strip_prefix("ollama:")
        .or_else(|| trimmed.strip_prefix("OLLAMA:"))
    {
        return ollama_model.to_string();
    }
    let lower = trimmed.to_ascii_lowercase();
    MODEL_REGISTRY
        .iter()
        .find_map(|(alias, metadata)| {
            (*alias == lower).then_some(match metadata.provider {
                ProviderKind::Anthropic => match *alias {
                    "opus" => "claude-opus-4-6",
                    "sonnet" => "claude-sonnet-4-6",
                    "haiku" => "claude-haiku-4-5-20251213",
                    _ => trimmed,
                },
                ProviderKind::Xai => match *alias {
                    "grok" | "grok-3" => "grok-3",
                    "grok-mini" | "grok-3-mini" => "grok-3-mini",
                    "grok-2" => "grok-2",
                    _ => trimmed,
                },
                ProviderKind::Gemini => match *alias {
                    "gemini" | "gemini-pro" | "gemini-2.5-pro" => "gemini-2.5-pro-preview-05-06",
                    "gemini-2.5-flash" => "gemini-2.5-flash-preview-04-17",
                    _ => trimmed,
                },
                ProviderKind::Ollama => {
                    // Ollama models pass through as-is (user specifies the exact tag)
                    trimmed
                }
                ProviderKind::OpenAi => trimmed,
            })
        })
        .map_or_else(|| trimmed.to_string(), ToOwned::to_owned)
}

#[must_use]
pub fn metadata_for_model(model: &str) -> Option<ProviderMetadata> {
    let canonical = resolve_model_alias(model);

    // Check explicit registry first
    if let Some((_, meta)) = MODEL_REGISTRY
        .iter()
        .find(|(alias, _)| *alias == canonical || *alias == model.trim().to_ascii_lowercase())
    {
        return Some(*meta);
    }

    // Fallback: detect by model name prefix
    if canonical.starts_with("claude") {
        return Some(ProviderMetadata {
            provider: ProviderKind::Anthropic,
            auth_env: "ANTHROPIC_API_KEY",
            base_url_env: "ANTHROPIC_BASE_URL",
            default_base_url: anthropic::DEFAULT_BASE_URL,
        });
    }
    if canonical.starts_with("grok") {
        return Some(ProviderMetadata {
            provider: ProviderKind::Xai,
            auth_env: "XAI_API_KEY",
            base_url_env: "XAI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_XAI_BASE_URL,
        });
    }
    if canonical.starts_with("gemini") {
        return Some(ProviderMetadata {
            provider: ProviderKind::Gemini,
            auth_env: "GEMINI_API_KEY",
            base_url_env: "GEMINI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_GEMINI_BASE_URL,
        });
    }
    if canonical.starts_with("gpt") || canonical.starts_with("o1") || canonical.starts_with("o3") {
        return Some(ProviderMetadata {
            provider: ProviderKind::OpenAi,
            auth_env: "OPENAI_API_KEY",
            base_url_env: "OPENAI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_OPENAI_BASE_URL,
        });
    }
    // Detect "ollama:" prefix for explicit Ollama model selection (e.g. "ollama:llama3")
    if canonical.starts_with("ollama:") || model.trim().to_ascii_lowercase().starts_with("ollama:")
    {
        return Some(ProviderMetadata {
            provider: ProviderKind::Ollama,
            auth_env: "OLLAMA_API_KEY",
            base_url_env: "OLLAMA_BASE_URL",
            default_base_url: openai_compat::DEFAULT_OLLAMA_BASE_URL,
        });
    }
    None
}

#[must_use]
pub fn detect_provider_kind(model: &str) -> ProviderKind {
    if let Some(metadata) = metadata_for_model(model) {
        return metadata.provider;
    }
    // Fallback: check available credentials in priority order
    if anthropic::has_auth_from_env_or_saved().unwrap_or(false) {
        return ProviderKind::Anthropic;
    }
    if openai_compat::has_api_key("OPENAI_API_KEY") {
        return ProviderKind::OpenAi;
    }
    if openai_compat::has_api_key("GEMINI_API_KEY") || openai_compat::has_api_key("GOOGLE_API_KEY")
    {
        return ProviderKind::Gemini;
    }
    if openai_compat::has_api_key("XAI_API_KEY") {
        return ProviderKind::Xai;
    }
    // Default: Anthropic
    ProviderKind::Anthropic
}

#[must_use]
pub fn max_tokens_for_model(model: &str) -> u32 {
    let canonical = resolve_model_alias(model);
    if canonical.contains("opus") {
        32_000
    } else if canonical.starts_with("gemini") {
        65_536
    } else {
        64_000
    }
}

#[cfg(test)]
mod tests {
    use super::{detect_provider_kind, max_tokens_for_model, resolve_model_alias, ProviderKind};

    #[test]
    fn resolves_grok_aliases() {
        assert_eq!(resolve_model_alias("grok"), "grok-3");
        assert_eq!(resolve_model_alias("grok-mini"), "grok-3-mini");
        assert_eq!(resolve_model_alias("grok-2"), "grok-2");
    }

    #[test]
    fn resolves_gemini_aliases() {
        assert_eq!(
            resolve_model_alias("gemini"),
            "gemini-2.5-pro-preview-05-06"
        );
        assert_eq!(
            resolve_model_alias("gemini-pro"),
            "gemini-2.5-pro-preview-05-06"
        );
        assert_eq!(
            resolve_model_alias("gemini-2.5-flash"),
            "gemini-2.5-flash-preview-04-17"
        );
    }

    #[test]
    fn resolves_ollama_aliases() {
        // Ollama models pass through as-is
        assert_eq!(resolve_model_alias("llama3"), "llama3");
        assert_eq!(resolve_model_alias("codellama"), "codellama");
        assert_eq!(resolve_model_alias("deepseek-coder"), "deepseek-coder");
    }

    #[test]
    fn detects_provider_from_model_name_first() {
        assert_eq!(detect_provider_kind("grok"), ProviderKind::Xai);
        assert_eq!(
            detect_provider_kind("claude-sonnet-4-6"),
            ProviderKind::Anthropic
        );
        assert_eq!(detect_provider_kind("gemini-pro"), ProviderKind::Gemini);
        assert_eq!(
            detect_provider_kind("gemini-2.5-flash"),
            ProviderKind::Gemini
        );
        assert_eq!(detect_provider_kind("llama3"), ProviderKind::Ollama);
    }

    #[test]
    fn detects_openai_by_prefix() {
        assert_eq!(detect_provider_kind("gpt-4o"), ProviderKind::OpenAi);
        assert_eq!(detect_provider_kind("o1-preview"), ProviderKind::OpenAi);
        assert_eq!(detect_provider_kind("o3-mini"), ProviderKind::OpenAi);
    }

    #[test]
    fn keeps_existing_max_token_heuristic() {
        assert_eq!(max_tokens_for_model("opus"), 32_000);
        assert_eq!(max_tokens_for_model("grok-3"), 64_000);
        assert_eq!(max_tokens_for_model("gemini-pro"), 65_536);
    }
}
