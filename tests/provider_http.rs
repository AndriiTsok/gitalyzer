//! HTTP-level adapter tests against a local wiremock server (RFC 0007 R11):
//! request shaping, error mapping, the retry budget, the `json_schema` →
//! `json_object` degradation, and the repair-retry path — all deterministic,
//! no real endpoints or keys.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use gitalyzer::provider::anthropic::AnthropicProvider;
use gitalyzer::provider::openai::OpenAiProvider;
use gitalyzer::provider::{AnyProvider, ProviderError, RetryPolicy, run_task};
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Retry policy that keeps tests fast.
fn fast_policy() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 3,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
    }
}

fn anthropic(server: &MockServer) -> AnyProvider {
    AnyProvider::Anthropic(
        AnthropicProvider::new(
            "test-key".into(),
            &server.uri(),
            "test-model".into(),
            Duration::from_secs(5),
            fast_policy(),
        )
        .expect("client builds"),
    )
}

fn openai(server: &MockServer) -> AnyProvider {
    AnyProvider::OpenAi(
        OpenAiProvider::new(
            Some("test-key".into()),
            &server.uri(),
            "test-model".into(),
            Duration::from_secs(5),
            fast_policy(),
        )
        .expect("client builds"),
    )
}

/// The typed payload used by all tasks in this suite.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Verdict {
    score: u8,
    summary: String,
}

/// Anthropic-shaped success body carrying `input` as the tool call result.
fn anthropic_tool_response(input: &serde_json::Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "id": "msg_test",
        "type": "message",
        "content": [{ "type": "tool_use", "id": "tu_1", "name": "judge", "input": input }],
        "stop_reason": "tool_use"
    }))
}

/// OpenAI-shaped success body carrying the JSON payload as message content.
fn openai_content_response(content: &serde_json::Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "id": "chatcmpl-test",
        "choices": [{ "index": 0, "message": { "role": "assistant",
            "content": content.to_string() }, "finish_reason": "stop" }]
    }))
}

/// Respond with a scripted sequence of templates (clamping to the last).
struct Sequence {
    hits: AtomicUsize,
    responses: Vec<ResponseTemplate>,
}

impl Sequence {
    fn new(responses: Vec<ResponseTemplate>) -> Self {
        Self {
            hits: AtomicUsize::new(0),
            responses,
        }
    }
}

impl Respond for Sequence {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let n = self.hits.fetch_add(1, Ordering::SeqCst);
        self.responses[n.min(self.responses.len() - 1)].clone()
    }
}

#[tokio::test]
async fn anthropic_forces_the_tool_call_and_returns_typed_output() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        // RFC 0003 R4: the tool call must be forced and carry our schema.
        .and(body_partial_json(json!({
            "model": "test-model",
            "tool_choice": { "type": "tool", "name": "judge" }
        })))
        .respond_with(anthropic_tool_response(
            &json!({"score": 9, "summary": "great"}),
        ))
        .expect(1)
        .mount(&server)
        .await;

    let verdict: Verdict = run_task(&anthropic(&server), "judge", "judge things", "sys", "user")
        .await
        .expect("typed result");
    assert_eq!(verdict.score, 9);
    assert_eq!(verdict.summary, "great");
}

#[tokio::test]
async fn anthropic_maps_auth_and_model_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "type": "error", "error": { "type": "authentication_error", "message": "bad key" }
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    let error = run_task::<Verdict>(&anthropic(&server), "judge", "d", "s", "u")
        .await
        .expect_err("401 must fail");
    assert!(matches!(error, ProviderError::Auth { .. }), "got: {error}");
    assert!(error.to_string().contains("bad key"));

    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "type": "error", "error": { "type": "not_found_error", "message": "model gone" }
        })))
        .mount(&server)
        .await;
    let error = run_task::<Verdict>(&anthropic(&server), "judge", "d", "s", "u")
        .await
        .expect_err("404 must fail");
    assert!(
        matches!(error, ProviderError::UnknownModel { .. }),
        "got: {error}"
    );
    assert!(error.to_string().contains("test-model"));
}

#[tokio::test]
async fn transient_429_is_retried_until_success() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(Sequence::new(vec![
            ResponseTemplate::new(429).insert_header("retry-after", "0"),
            anthropic_tool_response(&json!({"score": 5, "summary": "ok"})),
        ]))
        .expect(2)
        .mount(&server)
        .await;

    let verdict: Verdict = run_task(&anthropic(&server), "judge", "d", "s", "u")
        .await
        .expect("second try wins");
    assert_eq!(verdict.score, 5);
}

#[tokio::test]
async fn persistent_500s_exhaust_the_retry_budget() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .expect(3) // RFC 0003 R9: exactly 3 attempts total
        .mount(&server)
        .await;

    let error = run_task::<Verdict>(&anthropic(&server), "judge", "d", "s", "u")
        .await
        .expect_err("must give up");
    assert!(
        matches!(error, ProviderError::RetriesExhausted { attempts: 3, .. }),
        "got: {error}"
    );
}

#[tokio::test]
async fn openai_uses_json_schema_response_format() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(json!({
            "model": "test-model",
            "response_format": { "type": "json_schema" }
        })))
        .respond_with(openai_content_response(
            &json!({"score": 8, "summary": "solid"}),
        ))
        .expect(1)
        .mount(&server)
        .await;

    let verdict: Verdict = run_task(&openai(&server), "judge", "d", "s", "u")
        .await
        .expect("typed result");
    assert_eq!(verdict.score, 8);
}

#[tokio::test]
async fn openai_degrades_to_json_object_once_and_caches_it() {
    /// Reject `json_schema` bodies; accept `json_object` ones.
    struct SchemaRejector {
        hits: AtomicUsize,
    }
    impl Respond for SchemaRejector {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            self.hits.fetch_add(1, Ordering::SeqCst);
            let body = String::from_utf8_lossy(&request.body);
            if body.contains("json_schema") {
                ResponseTemplate::new(400).set_body_json(json!({
                    "error": { "message": "response_format `json_schema` is not supported" }
                }))
            } else {
                openai_content_response(&json!({"score": 4, "summary": "fallback"}))
            }
        }
    }

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(SchemaRejector {
            hits: AtomicUsize::new(0),
        })
        .mount(&server)
        .await;

    let provider = openai(&server);
    let first: Verdict = run_task(&provider, "judge", "d", "s", "u")
        .await
        .expect("degrades");
    assert_eq!(first.summary, "fallback");
    // Second task must skip straight to json_object: 1 rejection + 1 success
    // for the first call, then exactly 1 request for the second.
    let second: Verdict = run_task(&provider, "judge", "d", "s", "u")
        .await
        .expect("cached mode");
    assert_eq!(second.score, 4);
    let requests = server.received_requests().await.expect("recording enabled");
    assert_eq!(
        requests.len(),
        3,
        "degradation must be cached after the first rejection"
    );
}

#[tokio::test]
async fn schema_invalid_result_triggers_one_repair_retry() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(Sequence::new(vec![
            anthropic_tool_response(&json!({"score": "not a number", "summary": 1})),
            anthropic_tool_response(&json!({"score": 6, "summary": "repaired"})),
        ]))
        .expect(2)
        .mount(&server)
        .await;

    let verdict: Verdict = run_task(&anthropic(&server), "judge", "d", "s", "u")
        .await
        .expect("repaired");
    assert_eq!(verdict.summary, "repaired");
}

#[tokio::test]
async fn openai_strips_markdown_fences_from_local_models() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "role": "assistant",
                "content": "```json\n{\"score\": 3, \"summary\": \"fenced\"}\n```" } }]
        })))
        .mount(&server)
        .await;

    let verdict: Verdict = run_task(&openai(&server), "judge", "d", "s", "u")
        .await
        .expect("fences stripped");
    assert_eq!(verdict.summary, "fenced");
}
