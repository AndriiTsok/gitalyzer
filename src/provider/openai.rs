//! OpenAI-compatible adapter (RFC 0003 R4–R5): structured outputs via the
//! `json_schema` response format, degrading once per process to `json_object`
//! mode (schema embedded in the system prompt) for compatible servers that
//! lack it — which is how Ollama, Groq, `OpenRouter` and friends stay covered
//! with a configurable `base_url`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::json;

use super::retry::{RetryPolicy, send_with_retry};
use super::{
    LlmProvider, OPENAI_ID, ProviderError, TaskSpec, api_error_message, is_context_overflow,
};

/// Thin client for the `OpenAI` Chat Completions API and compatible servers.
pub struct OpenAiProvider {
    client: reqwest::Client,
    api_key: Option<String>,
    base_url: String,
    model: String,
    policy: RetryPolicy,
    /// Set once a server rejects `json_schema`; later calls skip straight to
    /// `json_object` mode instead of failing the same way per batch.
    use_json_object: AtomicBool,
}

impl std::fmt::Debug for OpenAiProvider {
    /// Redacts the API key (RFC 0007 R7).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiProvider")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field(
                "use_json_object",
                &self.use_json_object.load(Ordering::Relaxed),
            )
            .finish_non_exhaustive()
    }
}

impl OpenAiProvider {
    /// Build a client for the given endpoint and model; `api_key` may be
    /// `None` for custom endpoints (RFC 0003 R6).
    pub fn new(
        api_key: Option<String>,
        base_url: &str,
        model: String,
        timeout: Duration,
        policy: RetryPolicy,
    ) -> Result<Self, ProviderError> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|source| ProviderError::Transport {
                provider: OPENAI_ID,
                source,
            })?;
        Ok(Self {
            client,
            api_key,
            base_url: base_url.trim_end_matches('/').to_owned(),
            model,
            policy,
            use_json_object: AtomicBool::new(false),
        })
    }

    /// Map a non-success response to a typed, actionable error (RFC 0003 R10).
    fn map_error(&self, status: reqwest::StatusCode, body: &str) -> ProviderError {
        if is_context_overflow(status.as_u16(), body) {
            return ProviderError::ContextTooLarge {
                provider: OPENAI_ID,
                message: api_error_message(body),
            };
        }
        let message = api_error_message(body);
        match status.as_u16() {
            401 | 403 => ProviderError::Auth {
                provider: OPENAI_ID,
                status: status.as_u16(),
                message,
            },
            404 => ProviderError::UnknownModel {
                provider: OPENAI_ID,
                model: self.model.clone(),
                message,
            },
            _ => ProviderError::Api {
                provider: OPENAI_ID,
                status: status.as_u16(),
                message,
            },
        }
    }

    /// Whether a 400 in schema mode means "this server has no `json_schema`
    /// support" and the call should degrade to `json_object` mode.
    fn is_schema_rejection(status: reqwest::StatusCode, body: &str) -> bool {
        status.as_u16() == 400 && (body.contains("response_format") || body.contains("json_schema"))
    }
}

/// The two structured-output modes of RFC 0003 R4.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    JsonSchema,
    JsonObject,
}

impl LlmProvider for OpenAiProvider {
    fn id(&self) -> &'static str {
        OPENAI_ID
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn complete_structured(
        &self,
        task: &TaskSpec,
    ) -> Result<serde_json::Value, ProviderError> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut mode = if self.use_json_object.load(Ordering::Relaxed) {
            Mode::JsonObject
        } else {
            Mode::JsonSchema
        };

        loop {
            let (system, response_format) = match mode {
                Mode::JsonSchema => (
                    task.system.clone(),
                    json!({
                        "type": "json_schema",
                        "json_schema": { "name": task.name, "schema": task.schema },
                    }),
                ),
                Mode::JsonObject => (
                    format!(
                        "{}\n\nRespond with a single JSON object that strictly matches this \
                         JSON Schema:\n{}",
                        task.system, task.schema
                    ),
                    json!({ "type": "json_object" }),
                ),
            };
            let body = json!({
                "model": self.model,
                "messages": [
                    { "role": "system", "content": system },
                    { "role": "user", "content": task.user },
                ],
                "response_format": response_format,
            });

            let (status, text) = send_with_retry(
                || {
                    let mut request = self.client.post(&url).json(&body);
                    if let Some(key) = &self.api_key {
                        request = request.bearer_auth(key);
                    }
                    request
                },
                &self.policy,
                OPENAI_ID,
            )
            .await?;

            if !status.is_success() {
                if mode == Mode::JsonSchema && Self::is_schema_rejection(status, &text) {
                    tracing::debug!(
                        "endpoint rejected json_schema response format; degrading to json_object"
                    );
                    self.use_json_object.store(true, Ordering::Relaxed);
                    mode = Mode::JsonObject;
                    continue;
                }
                return Err(self.map_error(status, &text));
            }

            let response: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| ProviderError::Malformed {
                    provider: OPENAI_ID,
                    detail: format!("response is not JSON: {e}"),
                })?;
            // finish_reason "length" means the JSON was cut mid-stream;
            // surface the real cause. No max token limit is sent for OpenAI:
            // reasoning models spend output budget on hidden thinking, so
            // capping it risks empty results.
            if response
                .pointer("/choices/0/finish_reason")
                .and_then(|r| r.as_str())
                == Some("length")
            {
                return Err(ProviderError::OutputTruncated {
                    provider: OPENAI_ID,
                });
            }
            let content = response
                .pointer("/choices/0/message/content")
                .and_then(|c| c.as_str())
                .ok_or_else(|| ProviderError::Malformed {
                    provider: OPENAI_ID,
                    detail: "no choices[0].message.content in the response".to_owned(),
                })?;

            return serde_json::from_str(strip_code_fences(content)).map_err(|e| {
                ProviderError::Malformed {
                    provider: OPENAI_ID,
                    detail: format!("message content is not a JSON object: {e}"),
                }
            });
        }
    }
}

/// Some OpenAI-compatible local models wrap JSON in Markdown fences despite
/// JSON mode; strip one outer fence pair defensively.
fn strip_code_fences(content: &str) -> &str {
    let trimmed = content.trim();
    let Some(inner) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let inner = inner.strip_prefix("json").unwrap_or(inner);
    inner.strip_suffix("```").unwrap_or(inner).trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_fences_are_stripped() {
        assert_eq!(strip_code_fences("{\"a\":1}"), "{\"a\":1}");
        assert_eq!(strip_code_fences("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_code_fences("```\n{\"a\":1}\n```"), "{\"a\":1}");
    }

    #[test]
    fn schema_rejection_detection_is_narrow() {
        let bad_request = reqwest::StatusCode::BAD_REQUEST;
        assert!(OpenAiProvider::is_schema_rejection(
            bad_request,
            r#"{"error":{"message":"response_format json_schema is not supported"}}"#
        ));
        assert!(!OpenAiProvider::is_schema_rejection(
            bad_request,
            "some other validation issue"
        ));
        assert!(!OpenAiProvider::is_schema_rejection(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "response_format"
        ));
    }
}
