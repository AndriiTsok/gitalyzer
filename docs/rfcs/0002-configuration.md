# RFC 0002: Configuration

- **Status:** Accepted
- **Author(s):** Andrii Tsok
- **Created:** 2026-07-06
- **Supersedes:** —
- **Superseded by:** —

## Summary

Defines Gitalyzer's layered configuration: built-in defaults, a user-level YAML file,
a project-level YAML file, convention-named environment variables, and CLI flags —
each layer overriding the previous one key-by-key.

## Motivation

Gitalyzer talks to multiple LLM providers (RFC 0003), so credentials and preferences
differ per user and per project. The PRD (§5) requires that every configuration value
can be supplied or overridden by a predictably named environment variable; RFC 0001
adds per-invocation CLI overrides on top.

## Requirements

- **R1.** Configuration files are **YAML**.
- **R2.** File discovery, both optional (a missing file is not an error):
  - user level: `$XDG_CONFIG_HOME/gitalyzer/config.yaml`
    (default `~/.config/gitalyzer/config.yaml`);
  - project level: `.gitalyzer.yaml` at the repository root.
- **R3.** Precedence, low → high, merged key-by-key (deep merge):
  1. built-in defaults
  2. user config file
  3. project config file
  4. environment variables
  5. CLI flags
- **R4.** `--config <path>` (RFC 0001 R8) MUST replace file discovery entirely: the
  given file becomes the only file layer (env vars and CLI flags still apply). An
  explicitly given file that cannot be read IS an error (exit `1`).
- **R5.** Every configuration value MUST be settable via an environment variable named
  `GITALYZER_` + the UPPER_SNAKE key path, with `__` (double underscore) as the
  nesting separator:

  ```text
  GITALYZER_PROVIDER=anthropic
  GITALYZER_MODEL=claude-sonnet-5
  GITALYZER_ANALYZE__COUNT=100
  GITALYZER_PROVIDERS__ANTHROPIC__API_KEY=sk-...
  ```

- **R6.** Standard provider credential variables (e.g. `ANTHROPIC_API_KEY`,
  `OPENAI_API_KEY`) MUST be honored as a fallback for that provider's `api_key` when
  the `GITALYZER_`-namespaced value is absent. The exact variable per provider is
  defined in RFC 0003.
- **R7.** API keys MAY be placed in config files, but documentation MUST discourage
  it and warn against committing secrets; environment variables are the recommended
  path.
- **R8.** Malformed YAML or invalid values (wrong type, unknown provider id) MUST
  fail with an actionable message and exit `1`. Unknown keys SHOULD produce a warning
  rather than an error (forward compatibility).

## Initial schema

The schema grows with later RFCs; this is the starting shape:

```yaml
provider: anthropic        # default LLM provider id        (default: RFC 0003)
model: claude-sonnet-5     # default model for the provider (default: RFC 0003)

analyze:
  count: 50                # commits analyzed by default
  batch_size: 10           # commits per LLM request (semantics: RFC 0005)

providers:
  anthropic:
    api_key: "..."         # discouraged here — prefer environment variables
  openai:
    api_key: "..."
    base_url: "..."        # OpenAI-compatible endpoints (details: RFC 0003)
```

## Alternatives considered

- **TOML** — the Rust-ecosystem default, declined in favor of YAML's familiarity.
- **JSON** — no comments; poor fit for a hand-edited file.
- **Flat env names** (`GITALYZER_ANALYZE_COUNT`) — reads slightly nicer but becomes
  ambiguous with snake_case keys; the `__` separator keeps the mapping mechanical.

## Implementation notes

Candidate crates: `figment` or `config` — both provide layered sources (files, env
with custom separators, defaults) and YAML support. The final pick is locked in the
implementation-bootstrap RFC; note that `serde_yaml` is deprecated, so the chosen
stack must rely on a maintained YAML path.

## Deferred

- Per-provider schema details (endpoints, models, credential variables) → RFC 0003.
- OS keychain / secret-manager integration → future RFC if needed.
- A `gitalyzer config show` debugging command → future RFC.

## References

- PRD: [`../product.md`](../product.md), §5.
- RFC 0001: R7 (override precedence), R8 (`--config`).
