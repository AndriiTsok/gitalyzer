# RFC 0003: LLM Providers

- **Status:** Accepted
- **Author(s):** Andrii Tsok
- **Created:** 2026-07-06
- **Supersedes:** —
- **Superseded by:** —

## Summary

Two native provider adapters — **Anthropic** and **OpenAI-compatible** — implemented
as thin `reqwest` clients behind a single internal provider abstraction. All LLM
calls return **schema-enforced JSON**; responses arrive complete (no streaming),
behind progress indication.

## Motivation

Multi-provider support is a headline product requirement (PRD §5). Pairing a native
Anthropic adapter with an OpenAI adapter whose `base_url` is configurable covers the
widest ecosystem for the least surface: Ollama, OpenRouter, Groq, Mistral and many
others all expose OpenAI-compatible APIs.

## Requirements

- **R1.** Provider ids: `anthropic` and `openai`. The active provider/model resolve
  via the RFC 0002 precedence chain (`provider`/`model` keys, `GITALYZER_PROVIDER`/
  `GITALYZER_MODEL`, `--provider`/`--model`).
- **R2.** Adapters are thin, hand-written HTTP clients (`reqwest` + `serde`) behind
  one internal trait. No per-provider SDK crates; no umbrella LLM crates.
- **R3.** The trait contract: given a task (system prompt, user content) and the
  expected result schema, return a **validated, typed result** or a typed error.
  Callers (RFCs 0005/0006) never see raw provider payloads.
- **R4.** Structured output uses each provider's strongest mechanism:
  - Anthropic: forced **tool-use** with the result schema as the tool's input schema;
  - OpenAI: **structured outputs** (`json_schema` response format), falling back to
    `json_object` mode for compatible servers that lack it.
  Results failing validation get exactly **one repair retry** (re-prompt with the
  validation error) before the call fails.
- **R5.** The `openai` adapter MUST honor `base_url` (default
  `https://api.openai.com/v1`) so any OpenAI-compatible endpoint works. The
  `anthropic` adapter SHOULD honor `base_url` too (default
  `https://api.anthropic.com`) for proxies/gateways.
- **R6.** Credentials per RFC 0002: `providers.<id>.api_key`, overridable via
  `GITALYZER_PROVIDERS__<ID>__API_KEY`, falling back to the standard variable —
  `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`. A missing key is an actionable error —
  except for `openai` with a non-default `base_url`, where the key is optional
  (local servers like Ollama need none).
- **R7.** Built-in defaults: provider `anthropic`, model `claude-sonnet-5`; for
  `openai`, model `gpt-5`. Defaults live in one constants table and are re-verified
  at implementation time.
- **R8.** No streaming in v1. Calls run to completion behind progress indication
  (RFC 0007); structured JSON must be buffered whole anyway.
- **R9.** Transient failures (HTTP 429/5xx, network errors) are retried up to 3
  attempts total with exponential backoff + jitter, honoring `Retry-After`. Request
  timeout defaults to 120 s (`request_timeout_secs`).
- **R10.** Provider errors map to actionable messages (missing key, 401 bad key,
  unknown model, retries exhausted) and exit code `1`.

## Config schema additions (extends RFC 0002)

```yaml
provider: anthropic
model: claude-sonnet-5
request_timeout_secs: 120

providers:
  anthropic:
    api_key: "..."                          # ← ANTHROPIC_API_KEY fallback
    base_url: "https://api.anthropic.com"   # default
  openai:
    api_key: "..."                          # ← OPENAI_API_KEY fallback
    base_url: "https://api.openai.com/v1"   # any OpenAI-compatible server
```

## Alternatives considered

- **Per-provider SDK crates** — uneven quality/maintenance; their abstractions leak
  into ours.
- **Umbrella LLM crates** (e.g. `genai`) — fastest start, but we would inherit their
  coverage, defaults, and release cadence in a core dependency.
- **Dedicated Ollama adapter** — unnecessary: the OpenAI-compatible path covers it;
  revisit only if native features (model pulls, listing) become product needs.
- **Streaming** — deferred; conflicts with schema-enforced JSON and adds per-provider
  code for cosmetic gain.

## Implementation notes

Implied dependencies: `reqwest` (rustls, JSON), `serde`/`serde_json`, and `schemars`
(derive JSON schemas for tool/structured-output definitions from the same Rust types
we deserialize into). Final versions locked in the implementation-bootstrap RFC.

## Deferred

- Task prompts and result schemas → RFC 0005 (analyze), RFC 0006 (write).
- Progress UI and retry observability → RFC 0007.
- Additional native providers (e.g. Gemini, Bedrock) → future RFCs.

## References

- PRD: [`../product.md`](../product.md), §5.
- RFC 0001 R7 (CLI overrides); RFC 0002 R5–R6 (env convention, key fallback).

## Changelog

- 2026-07-06 — Amended: tasks carry an output-token budget hint scaled by the
  caller (Anthropic `max_tokens`; deliberately not sent to OpenAI, where
  reasoning models spend output budget on hidden thinking). Truncated output
  (`stop_reason: max_tokens` / `finish_reason: length`) and context-window
  overflow (HTTP 400/413) map to dedicated actionable errors instead of
  surfacing as validation failures.
