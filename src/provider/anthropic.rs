//! Native Anthropic adapter (RFC 0003 R4–R5): structured output via a
//! **forced tool call** whose input schema is the expected result schema.

use std::time::Duration;

use serde_json::json;

use super::retry::{RetryPolicy, send_with_retry};
use super::{
    ANTHROPIC_ID, LlmProvider, ProviderError, TaskSpec, api_error_message, is_context_overflow,
};

/// Anthropic Messages API version header value.
const API_VERSION: &str = "2023-06-01";
/// Default generation budget when the task carries no hint.
const DEFAULT_MAX_TOKENS: u32 = 8192;

/// Thin client for the Anthropic Messages API.
pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
    policy: RetryPolicy,
}

impl std::fmt::Debug for AnthropicProvider {
    /// Redacts the API key (RFC 0007 R7).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicProvider")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl AnthropicProvider {
    /// Build a client for the given endpoint and model.
    pub fn new(
        api_key: String,
        base_url: &str,
        model: String,
        timeout: Duration,
        policy: RetryPolicy,
    ) -> Result<Self, ProviderError> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|source| ProviderError::Transport {
                provider: ANTHROPIC_ID,
                source,
            })?;
        Ok(Self {
            client,
            api_key,
            base_url: base_url.trim_end_matches('/').to_owned(),
            model,
            policy,
        })
    }

    /// Map a non-success response to a typed, actionable error (RFC 0003 R10).
    fn map_error(&self, status: reqwest::StatusCode, body: &str) -> ProviderError {
        if is_context_overflow(status.as_u16(), body) {
            return ProviderError::ContextTooLarge {
                provider: ANTHROPIC_ID,
                message: api_error_message(body),
            };
        }
        let message = api_error_message(body);
        match status.as_u16() {
            401 | 403 => ProviderError::Auth {
                provider: ANTHROPIC_ID,
                status: status.as_u16(),
                message,
            },
            404 => ProviderError::UnknownModel {
                provider: ANTHROPIC_ID,
                model: self.model.clone(),
                message,
            },
            _ => ProviderError::Api {
                provider: ANTHROPIC_ID,
                status: status.as_u16(),
                message,
            },
        }
    }
}

impl LlmProvider for AnthropicProvider {
    fn id(&self) -> &'static str {
        ANTHROPIC_ID
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn complete_structured(
        &self,
        task: &TaskSpec,
    ) -> Result<serde_json::Value, ProviderError> {
        let url = format!("{}/v1/messages", self.base_url);
        let body = json!({
            "model": self.model,
            // Scaled by the caller with expected output volume so large
            // batches cannot be truncated mid-JSON (RFC 0003, amended).
            "max_tokens": task.max_output_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            "system": task.system,
            "messages": [{ "role": "user", "content": task.user }],
            "tools": [{
                "name": task.name,
                "description": task.description,
                "input_schema": task.schema,
            }],
            // Forcing the tool call is what makes the output schema-enforced
            // (RFC 0003 R4).
            "tool_choice": { "type": "tool", "name": task.name },
        });

        let (status, text) = send_with_retry(
            || {
                self.client
                    .post(&url)
                    .header("x-api-key", &self.api_key)
                    .header("anthropic-version", API_VERSION)
                    .json(&body)
            },
            &self.policy,
            ANTHROPIC_ID,
        )
        .await?;

        if !status.is_success() {
            return Err(self.map_error(status, &text));
        }

        let response: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| ProviderError::Malformed {
                provider: ANTHROPIC_ID,
                detail: format!("response is not JSON: {e}"),
            })?;
        // A max_tokens stop means the tool input was cut mid-JSON: surface
        // the real cause instead of a confusing validation failure.
        if response.pointer("/stop_reason").and_then(|r| r.as_str()) == Some("max_tokens") {
            return Err(ProviderError::OutputTruncated {
                provider: ANTHROPIC_ID,
            });
        }
        response
            .pointer("/content")
            .and_then(|content| content.as_array())
            .and_then(|blocks| {
                blocks
                    .iter()
                    .find(|block| block.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
            })
            .and_then(|block| block.get("input").cloned())
            .ok_or_else(|| ProviderError::Malformed {
                provider: ANTHROPIC_ID,
                detail: "no tool_use block in the response content".to_owned(),
            })
    }
}
