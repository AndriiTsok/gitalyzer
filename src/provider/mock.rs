//! Deterministic scripted provider for the test pyramid (RFC 0007 R11).
//!
//! Selected via `provider: mock` in configuration; end-to-end tests point
//! `GITALYZER_MOCK_SCRIPT` at a JSON file containing an array of responses,
//! consumed one per call. A response that doesn't match the task's schema
//! exercises the repair-retry path exactly like a misbehaving real model.

use std::collections::VecDeque;
use std::sync::Mutex;

use super::{LlmProvider, MOCK_ID, ProviderError, TaskSpec};

/// Environment variable pointing at the mock's response script.
pub const MOCK_SCRIPT_ENV: &str = "GITALYZER_MOCK_SCRIPT";

/// Scripted provider: returns canned responses in order.
#[derive(Debug)]
pub struct MockProvider {
    responses: Mutex<VecDeque<serde_json::Value>>,
    model: String,
}

impl MockProvider {
    /// Build directly from canned responses (unit tests).
    pub fn new(responses: Vec<serde_json::Value>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            model: "mock-model".to_owned(),
        }
    }

    /// Build from the `GITALYZER_MOCK_SCRIPT` file (end-to-end tests).
    pub fn from_env() -> Result<Self, ProviderError> {
        let path = std::env::var(MOCK_SCRIPT_ENV).map_err(|_| ProviderError::Malformed {
            provider: MOCK_ID,
            detail: format!(
                "the mock provider requires {MOCK_SCRIPT_ENV} to point at a JSON script"
            ),
        })?;
        let raw = std::fs::read_to_string(&path).map_err(|e| ProviderError::Malformed {
            provider: MOCK_ID,
            detail: format!("cannot read mock script `{path}`: {e}"),
        })?;
        let responses: Vec<serde_json::Value> =
            serde_json::from_str(&raw).map_err(|e| ProviderError::Malformed {
                provider: MOCK_ID,
                detail: format!("mock script `{path}` is not a JSON array: {e}"),
            })?;
        Ok(Self::new(responses))
    }
}

impl LlmProvider for MockProvider {
    fn id(&self) -> &'static str {
        MOCK_ID
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn complete_structured(
        &self,
        _task: &TaskSpec,
    ) -> Result<serde_json::Value, ProviderError> {
        self.responses
            .lock()
            .expect("mock script mutex")
            .pop_front()
            .ok_or_else(|| ProviderError::Malformed {
                provider: MOCK_ID,
                detail: "mock script exhausted: more calls were made than scripted responses"
                    .to_owned(),
            })
    }
}
