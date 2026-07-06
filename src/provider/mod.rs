//! LLM provider layer (RFC 0003).
//!
//! Two thin HTTP adapters — native Anthropic and OpenAI-compatible — behind
//! one trait (R2), returning **schema-enforced JSON** (R4) with a single
//! repair retry, plus a scriptable mock used by the test pyramid (RFC 0007
//! R11). Runtime selection is a closed enum ([`AnyProvider`]), so dispatch is
//! static and futures stay `Send` without extra machinery.

pub mod anthropic;
pub mod mock;
pub mod openai;
mod retry;

use serde::de::DeserializeOwned;

pub use retry::RetryPolicy;

use crate::config::Settings;

/// Provider id of the native Anthropic adapter (RFC 0003 R1).
pub const ANTHROPIC_ID: &str = "anthropic";
/// Provider id of the OpenAI-compatible adapter (RFC 0003 R1).
pub const OPENAI_ID: &str = "openai";
/// Internal provider id used by the deterministic test mock (RFC 0007 R11).
pub const MOCK_ID: &str = "mock";

/// Default endpoint of the Anthropic API (RFC 0003 R5).
pub const ANTHROPIC_DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
/// Default endpoint of the `OpenAI` API (RFC 0003 R5).
pub const OPENAI_DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// Built-in default models per provider (RFC 0003 R7). Re-verify these ids
/// when bumping provider integrations.
pub const DEFAULT_MODELS: &[(&str, &str)] =
    &[(ANTHROPIC_ID, "claude-sonnet-5"), (OPENAI_ID, "gpt-5")];

/// Resolve the built-in default model for a provider id (RFC 0003 R7).
pub fn default_model(provider: &str) -> Option<&'static str> {
    DEFAULT_MODELS
        .iter()
        .find(|(id, _)| *id == provider)
        .map(|(_, model)| *model)
}

/// One structured task for a provider: prompts plus the JSON Schema the
/// result must satisfy (RFC 0003 R3).
#[derive(Debug, Clone)]
pub struct TaskSpec {
    /// Tool/schema name shown to the model (e.g. `critique_commits`).
    pub name: &'static str,
    /// Tool description shown to the model.
    pub description: &'static str,
    /// System prompt.
    pub system: String,
    /// User content.
    pub user: String,
    /// JSON Schema of the expected result.
    pub schema: serde_json::Value,
    /// Output-token budget hint; callers scale it with the amount of content
    /// they expect back so large batches cannot be truncated mid-JSON.
    /// `None` uses the adapter default.
    pub max_output_tokens: Option<u32>,
}

/// Typed provider failures; messages are actionable (RFC 0003 R10).
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// No API key available for a provider that needs one (RFC 0003 R6).
    #[error(
        "no API key configured for provider `{provider}`; set the {var} environment variable \
         (or GITALYZER_PROVIDERS__{provider_upper}__API_KEY, or providers.{provider}.api_key \
         in the config file)"
    )]
    MissingApiKey {
        /// Provider id.
        provider: &'static str,
        /// The standard credential variable for this provider.
        var: &'static str,
        /// Provider id in env-var casing.
        provider_upper: &'static str,
    },
    /// The configured provider id has no adapter (guarded by config
    /// validation; kept for defense in depth).
    #[error("provider `{0}` is not supported (expected `anthropic` or `openai`)")]
    Unsupported(String),
    /// The endpoint rejected the credentials.
    #[error("provider `{provider}` rejected the API key (HTTP {status}): {message}")]
    Auth {
        /// Provider id.
        provider: &'static str,
        /// HTTP status code.
        status: u16,
        /// Message extracted from the error body.
        message: String,
    },
    /// The endpoint does not know the requested model.
    #[error("provider `{provider}` does not know model `{model}`: {message}")]
    UnknownModel {
        /// Provider id.
        provider: &'static str,
        /// The model that was requested.
        model: String,
        /// Message extracted from the error body.
        message: String,
    },
    /// Any other non-success API response (after retries where applicable).
    #[error("provider `{provider}` request failed (HTTP {status}): {message}")]
    Api {
        /// Provider id.
        provider: &'static str,
        /// HTTP status code.
        status: u16,
        /// Message extracted from the error body.
        message: String,
    },
    /// Transient failures persisted through the whole retry budget (RFC 0003 R9).
    #[error(
        "provider `{provider}` is rate-limiting or unavailable; giving up after {attempts} \
         attempts (last HTTP {status})"
    )]
    RetriesExhausted {
        /// Provider id.
        provider: &'static str,
        /// Total attempts made.
        attempts: u32,
        /// Status of the last response.
        status: u16,
    },
    /// The endpoint could not be reached at all.
    #[error("could not reach provider `{provider}`: {source}")]
    Transport {
        /// Provider id.
        provider: &'static str,
        /// Underlying HTTP client error.
        #[source]
        source: reqwest::Error,
    },
    /// The response arrived but did not have the documented shape.
    #[error("provider `{provider}` returned an unexpected response shape: {detail}")]
    Malformed {
        /// Provider id.
        provider: &'static str,
        /// What was wrong.
        detail: String,
    },
    /// The result kept failing schema validation after the repair retry (RFC 0003 R4).
    #[error("provider `{provider}` result failed validation even after a repair retry: {detail}")]
    InvalidResult {
        /// Provider id.
        provider: &'static str,
        /// Final validation error.
        detail: String,
    },
    /// The request exceeded the model's context window.
    #[error(
        "the request exceeded the model's context window ({message}); lower \
         analyze.batch_size / --batch-size or analyze.max_patch_bytes (for write: the \
         write.* budgets), or pick a model with a larger context"
    )]
    ContextTooLarge {
        /// Provider id.
        provider: &'static str,
        /// Message extracted from the error body.
        message: String,
    },
    /// The model ran out of output tokens before completing the result.
    #[error(
        "provider `{provider}` ran out of output tokens before completing the result; \
         lower analyze.batch_size / --batch-size so each request returns fewer items"
    )]
    OutputTruncated {
        /// Provider id.
        provider: &'static str,
    },
}

/// Whether an HTTP 400/413 body describes a context-window overflow.
pub(crate) fn is_context_overflow(status: u16, body: &str) -> bool {
    if status != 400 && status != 413 {
        return false;
    }
    let lowered = body.to_lowercase();
    [
        "context length",
        "maximum context",
        "prompt is too long",
        "too many tokens",
        "context window",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
}

/// The provider contract (RFC 0003 R2–R3): execute one structured task and
/// return schema-valid JSON. Callers never see raw provider payloads.
// `async fn` in a public trait is fine here: the impl set is closed (this
// crate only) and call sites use the concrete `AnyProvider`, so `Send` is
// inferred structurally.
#[allow(async_fn_in_trait)]
pub trait LlmProvider {
    /// Stable provider id (`anthropic`, `openai`, `mock`).
    fn id(&self) -> &'static str;
    /// The model this instance talks to.
    fn model(&self) -> &str;
    /// Execute the task, returning JSON produced under schema enforcement.
    async fn complete_structured(
        &self,
        task: &TaskSpec,
    ) -> Result<serde_json::Value, ProviderError>;
}

/// Runtime-selected provider (closed set per RFC 0003 R1).
#[derive(Debug)]
pub enum AnyProvider {
    /// Native Anthropic adapter.
    Anthropic(anthropic::AnthropicProvider),
    /// `OpenAI` or any OpenAI-compatible endpoint.
    OpenAi(openai::OpenAiProvider),
    /// Deterministic scripted mock (tests only; RFC 0007 R11).
    Mock(mock::MockProvider),
}

impl LlmProvider for AnyProvider {
    fn id(&self) -> &'static str {
        match self {
            Self::Anthropic(p) => p.id(),
            Self::OpenAi(p) => p.id(),
            Self::Mock(p) => p.id(),
        }
    }

    fn model(&self) -> &str {
        match self {
            Self::Anthropic(p) => p.model(),
            Self::OpenAi(p) => p.model(),
            Self::Mock(p) => p.model(),
        }
    }

    async fn complete_structured(
        &self,
        task: &TaskSpec,
    ) -> Result<serde_json::Value, ProviderError> {
        match self {
            Self::Anthropic(p) => p.complete_structured(task).await,
            Self::OpenAi(p) => p.complete_structured(task).await,
            Self::Mock(p) => p.complete_structured(task).await,
        }
    }
}

impl AnyProvider {
    /// Build the configured provider from merged settings (RFC 0003 R1,
    /// R5–R7): resolves the model default, base URL, credentials — including
    /// the "no key needed for custom OpenAI-compatible endpoints" rule (R6).
    pub fn from_settings(settings: &Settings) -> Result<Self, ProviderError> {
        let timeout = std::time::Duration::from_secs(settings.request_timeout_secs);
        let policy = RetryPolicy::default();
        let model = |fallback: &str| {
            settings
                .model
                .clone()
                .unwrap_or_else(|| fallback.to_owned())
        };

        match settings.provider.as_str() {
            ANTHROPIC_ID => {
                let key = settings.providers.anthropic.api_key.clone().ok_or(
                    ProviderError::MissingApiKey {
                        provider: ANTHROPIC_ID,
                        var: "ANTHROPIC_API_KEY",
                        provider_upper: "ANTHROPIC",
                    },
                )?;
                let base_url = settings
                    .providers
                    .anthropic
                    .base_url
                    .clone()
                    .unwrap_or_else(|| ANTHROPIC_DEFAULT_BASE_URL.to_owned());
                let default = default_model(ANTHROPIC_ID).expect("in table");
                Ok(Self::Anthropic(anthropic::AnthropicProvider::new(
                    key,
                    &base_url,
                    model(default),
                    timeout,
                    policy,
                )?))
            }
            OPENAI_ID => {
                let base_url = settings
                    .providers
                    .openai
                    .base_url
                    .clone()
                    .unwrap_or_else(|| OPENAI_DEFAULT_BASE_URL.to_owned());
                let key = settings.providers.openai.api_key.clone();
                // RFC 0003 R6: a key is optional only for non-default
                // endpoints (local servers like Ollama need none).
                if key.is_none() && base_url.trim_end_matches('/') == OPENAI_DEFAULT_BASE_URL {
                    return Err(ProviderError::MissingApiKey {
                        provider: OPENAI_ID,
                        var: "OPENAI_API_KEY",
                        provider_upper: "OPENAI",
                    });
                }
                let default = default_model(OPENAI_ID).expect("in table");
                Ok(Self::OpenAi(openai::OpenAiProvider::new(
                    key,
                    &base_url,
                    model(default),
                    timeout,
                    policy,
                )?))
            }
            MOCK_ID => Ok(Self::Mock(mock::MockProvider::from_env()?)),
            other => Err(ProviderError::Unsupported(other.to_owned())),
        }
    }
}

/// Run one typed task end-to-end (RFC 0003 R3–R4): derive the JSON Schema
/// from `T`, execute, validate by typed deserialization, and perform exactly
/// one repair retry (re-prompting with the validation error) before failing.
pub async fn run_task<T>(
    provider: &AnyProvider,
    name: &'static str,
    description: &'static str,
    system: &str,
    user: &str,
    max_output_tokens: Option<u32>,
) -> Result<T, ProviderError>
where
    T: DeserializeOwned + schemars::JsonSchema,
{
    let schema = schemars::SchemaGenerator::default()
        .into_root_schema_for::<T>()
        .to_value();
    let mut task = TaskSpec {
        name,
        description,
        system: system.to_owned(),
        user: user.to_owned(),
        schema,
        max_output_tokens,
    };

    let mut last_error = None;
    for attempt in 0..2 {
        let value = provider.complete_structured(&task).await?;
        match serde_json::from_value::<T>(value.clone()) {
            Ok(result) => return Ok(result),
            Err(error) => {
                tracing::debug!(
                    provider = provider.id(),
                    %error,
                    attempt,
                    "structured result failed validation"
                );
                if attempt == 0 {
                    let snippet = truncate_for_prompt(&value.to_string(), 2000);
                    task.user = format!(
                        "{user}\n\nYour previous response was rejected because it did not \
                         match the required schema: {error}\nPrevious response: {snippet}\n\
                         Respond again with ONLY a valid result matching the schema exactly."
                    );
                }
                last_error = Some(error);
            }
        }
    }

    Err(ProviderError::InvalidResult {
        provider: provider.id(),
        detail: last_error.map_or_else(|| "unknown".to_owned(), |e| e.to_string()),
    })
}

/// Cap text embedded into prompts or error messages.
pub(crate) fn truncate_for_prompt(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut cut = max_bytes;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &text[..cut])
}

/// Extract a human-readable message from a provider error body; both
/// Anthropic and `OpenAI` nest it under `error.message`.
pub(crate) fn api_error_message(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(|m| m.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| truncate_for_prompt(body.trim(), 300))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Settings;

    fn settings(provider: &str) -> Settings {
        Settings {
            provider: provider.to_owned(),
            ..Settings::default()
        }
    }

    #[test]
    fn anthropic_requires_an_api_key() {
        let error =
            AnyProvider::from_settings(&settings(ANTHROPIC_ID)).expect_err("must fail without key");
        assert!(matches!(error, ProviderError::MissingApiKey { .. }));
        assert!(
            error.to_string().contains("ANTHROPIC_API_KEY"),
            "got: {error}"
        );
    }

    #[test]
    fn openai_key_is_optional_only_for_custom_endpoints() {
        let mut s = settings(OPENAI_ID);
        let error = AnyProvider::from_settings(&s).expect_err("default endpoint requires a key");
        assert!(error.to_string().contains("OPENAI_API_KEY"), "got: {error}");

        s.providers.openai.base_url = Some("http://localhost:11434/v1".to_owned());
        let provider = AnyProvider::from_settings(&s).expect("local endpoint needs no key");
        assert_eq!(provider.id(), OPENAI_ID);
    }

    #[test]
    fn model_defaults_resolve_per_provider_and_overrides_win() {
        let mut s = settings(ANTHROPIC_ID);
        s.providers.anthropic.api_key = Some("sk-test".into());
        let provider = AnyProvider::from_settings(&s).expect("provider");
        assert_eq!(provider.model(), "claude-sonnet-5");

        s.model = Some("claude-opus-4-8".into());
        let provider = AnyProvider::from_settings(&s).expect("provider");
        assert_eq!(provider.model(), "claude-opus-4-8");
    }

    #[test]
    fn unsupported_provider_id_is_reported() {
        let error = AnyProvider::from_settings(&settings("nonsense")).expect_err("must fail");
        assert!(matches!(error, ProviderError::Unsupported(_)));
    }

    #[test]
    fn api_error_message_prefers_the_nested_field() {
        let body = r#"{"type":"error","error":{"type":"x","message":"boom"}}"#;
        assert_eq!(api_error_message(body), "boom");
        assert_eq!(api_error_message("plain text"), "plain text");
    }

    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    struct Payload {
        score: u8,
    }

    #[tokio::test]
    async fn run_task_repairs_one_invalid_result() {
        let provider = AnyProvider::Mock(mock::MockProvider::new(vec![
            serde_json::json!({"score": "not a number"}),
            serde_json::json!({"score": 7}),
        ]));
        let payload: Payload = run_task(&provider, "t", "d", "system", "user", None)
            .await
            .expect("repaired");
        assert_eq!(payload.score, 7);
    }

    #[tokio::test]
    async fn run_task_gives_up_after_one_repair() {
        let provider = AnyProvider::Mock(mock::MockProvider::new(vec![
            serde_json::json!({"score": "bad"}),
            serde_json::json!({"score": "still bad"}),
        ]));
        let error = run_task::<Payload>(&provider, "t", "d", "system", "user", None)
            .await
            .expect_err("must fail");
        assert!(
            matches!(error, ProviderError::InvalidResult { .. }),
            "got: {error}"
        );
    }
}
