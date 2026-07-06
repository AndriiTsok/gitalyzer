# Gitalyzer

**AI-powered critique for Git commit messages — and help writing better ones.**

Gitalyzer is a terminal tool that reviews the quality of a repository's commit
history the way an experienced reviewer would, and suggests well-formed commit
messages for your staged changes at the moment you commit. It works with
multiple LLM providers, produces both rich terminal reports and stable JSON,
and never sends more of your code to a model than you allow.

```text
$ gitalyzer analyze

Analyzing last 50 commits...

━━━━━━━━━━━━━━━━━━━━━━━━━━━━
💩 COMMITS THAT NEED WORK
━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Commit: "fixed bug"
Score: 2/10
Issue: Too vague - which bug? What was the impact?
Better: fix(auth): resolve token expiration handling

━━━━━━━━━━━━━━━━━━━━━━━━━━━━
💎 WELL-WRITTEN COMMITS
━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Commit: "feat(api): add Redis caching layer
         - Implement cache for read endpoints"
Score: 9/10
Why it's good: Clear scope, specific changes, measurable impact

━━━━━━━━━━━━━━━━━━━━━━━━━━━━
📊 YOUR STATS
━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Average score: 4.2/10
Vague commits: 34 (68%)
One-word commits: 12 (24%)
```

## The two modes

- **`gitalyzer analyze`** — critique recent commit history: a 1–10 score per
  commit against an explicit rubric (specificity, rationale, conventional
  format, subject quality, message-vs-diff fidelity), concrete issues, a
  better rewrite for weak messages, and deterministic aggregate stats.
  Works on the current repository or any remote one.
- **`gitalyzer write`** — read your staged changes and suggest a well-formed
  commit message. Press **Enter** to accept (the commit is created through
  your own `git`, so hooks and signing apply), **type** your own message, or
  press **`r`** to regenerate a different suggestion.

## Installation

Requires Rust 1.85+ to build. The `git` binary is needed only when `write`
creates commits — analysis is pure Rust.

```sh
cargo build --release
# binary at target/release/gitalyzer
```

## Quick start

```sh
export ANTHROPIC_API_KEY=sk-ant-...   # or OPENAI_API_KEY with --provider openai

gitalyzer analyze                      # critique the last 50 commits here
gitalyzer analyze -n 100 --from v1.0   # 100 commits starting at a tag/sha/branch
gitalyzer analyze --url="git@github.com:example/project.git" --branch develop
gitalyzer analyze --format json --output report.json

git add -p
gitalyzer write                        # interactive commit writer
gitalyzer write --dry-run              # print the suggestion, never commit
```

Remote analysis accepts any Git transport (`https://`, `ssh://`,
`git@host:path`, `git://`, `file://`) and authenticates **with your existing
git credentials** — your SSH agent/config for SSH URLs, your configured git
credential helpers for HTTPS. Clones are bare, shallow, land in a temporary
directory, and are deleted when the run ends.

## Providers

| Provider id | Endpoint | Default model | Credential |
| --- | --- | --- | --- |
| `anthropic` | Anthropic API (default provider) | `claude-sonnet-5` | `ANTHROPIC_API_KEY` |
| `openai` | OpenAI API **or any OpenAI-compatible server** via `base_url` | `gpt-5` | `OPENAI_API_KEY` (optional for custom endpoints) |

Any OpenAI-compatible server works — Ollama, OpenRouter, Groq, and friends:

```yaml
# ~/.config/gitalyzer/config.yaml
provider: openai
model: qwen3:32b
providers:
  openai:
    base_url: "http://localhost:11434/v1"   # Ollama needs no API key
```

## Configuration

Layered, lowest to highest precedence — later layers override earlier ones
key-by-key:

1. built-in defaults
2. user file: `$XDG_CONFIG_HOME/gitalyzer/config.yaml` (default `~/.config/gitalyzer/config.yaml`)
3. project file: `.gitalyzer.yaml` at the repository root
4. environment variables
5. CLI flags (`--provider`, `--model`, `-n/--count`, `--batch-size`)

`--config <path>` replaces file discovery with exactly that file.

Every key maps to an environment variable: `GITALYZER_` + the upper-cased key
path with `__` (double underscore) separating nesting levels.

```yaml
# Full reference with defaults
provider: anthropic            # GITALYZER_PROVIDER
model: null                    # GITALYZER_MODEL (null = provider default)
request_timeout_secs: 120      # GITALYZER_REQUEST_TIMEOUT_SECS

analyze:
  count: 50                    # GITALYZER_ANALYZE__COUNT
  batch_size: 10               # GITALYZER_ANALYZE__BATCH_SIZE (0 = one request)
  concurrency: 1               # GITALYZER_ANALYZE__CONCURRENCY (batches in flight)
  max_patch_bytes: 4096        # GITALYZER_ANALYZE__MAX_PATCH_BYTES (0 = never send code)
  max_batch_bytes: 262144      # GITALYZER_ANALYZE__MAX_BATCH_BYTES (hard byte ceiling per request)
  system_prompt: null          # GITALYZER_ANALYZE__SYSTEM_PROMPT (see docs/prompts.md)
  thresholds:
    needs_work: 5              # GITALYZER_ANALYZE__THRESHOLDS__NEEDS_WORK
    well_written: 8            # GITALYZER_ANALYZE__THRESHOLDS__WELL_WRITTEN

write:
  style: auto                  # GITALYZER_WRITE__STYLE (auto | conventional)
  system_prompt: null          # GITALYZER_WRITE__SYSTEM_PROMPT (see docs/prompts.md)
  max_file_patch_bytes: 8192   # GITALYZER_WRITE__MAX_FILE_PATCH_BYTES
  max_diff_bytes: 65536        # GITALYZER_WRITE__MAX_DIFF_BYTES

providers:
  anthropic:
    api_key: null              # prefer ANTHROPIC_API_KEY; never commit keys
    base_url: "https://api.anthropic.com"
  openai:
    api_key: null              # prefer OPENAI_API_KEY
    base_url: "https://api.openai.com/v1"
```

`write.style: auto` infers the repository's dominant message convention from
recent history and falls back to Conventional Commits; `conventional` always
suggests `type(scope): summary` messages.

The system prompts behind both modes are documented verbatim — and fully
overridable per user or per project — in [`docs/prompts.md`](docs/prompts.md).
Result parsing never depends on prompt wording: output shape is schema-enforced
at the API level. Context windows are protected by construction: prompts are
packed under a hard per-request byte ceiling (`analyze.max_batch_bytes`),
pathological commit messages are capped, output-token budgets scale with batch
size, and overflow/truncation surface as specific, actionable errors.

## JSON output & scripting

`--format json` prints **exactly one JSON document on stdout and nothing
else** — no progress, no warnings — so pipelines can parse it directly.
`--output <path>` writes the report to a file instead. The envelopes are
versioned (`schema_version: 1`) and stable; `write --format json` is always
non-interactive and never commits.

```sh
gitalyzer analyze --format json | jq '.stats.average_score'
gitalyzer write --format json | jq -r '.suggestion.subject'
```

## What gets sent to the LLM

Only ever to the provider you configured, and only this:

- **analyze** — commit messages, diff statistics (file paths, +/- counts),
  and a per-commit patch excerpt capped at `analyze.max_patch_bytes`
  (default 4 KiB). Set it to `0` and **no code content leaves your machine**
  in analysis — critique then rests on messages and stats alone.
- **write** — the staged summary, file list, and staged patches under
  `write.*` budgets. Generated and lock files (`Cargo.lock`,
  `package-lock.json`, `node_modules/`, minified assets, …) are always
  listed but their **content is never sent**.

API keys are redacted from all diagnostics. Nothing is stored server-side by
Gitalyzer itself.

## GitHub Actions

The repository ships a manually triggered workflow
([`.github/workflows/analyze.yml`](.github/workflows/analyze.yml)): open the
**Actions** tab → *Commit Message Analysis* → **Run workflow**, optionally
providing a Git `url` (empty analyzes this repository), `branch`, `count`,
`from`, `provider`, `model`, and `batch_size`. The report is published on the
run's summary page and attached as an artifact.

Setup: add an `ANTHROPIC_API_KEY` repository secret (or `OPENAI_API_KEY` and
run with `provider: openai`) under *Settings → Secrets and variables →
Actions*.

## Diagnostics

- `-v` (debug) / `-vv` (trace) — structured logs on stderr; trace includes
  full prompts/responses with keys redacted.
- `GITALYZER_LOG` — fine-grained filter directives (overrides `-v`).
- `GITALYZER_LOG_FORMAT=json` — machine-readable JSON log lines.
- `--no-color` or a non-empty `NO_COLOR` — plain output (also automatic when
  piping).

Exit codes: `0` success · `1` runtime failure · `2` CLI usage error.

## Dependencies

Runtime: [`clap`](https://crates.io/crates/clap) (CLI) ·
[`config`](https://crates.io/crates/config) (layered YAML/env configuration) ·
[`serde`](https://crates.io/crates/serde)/`serde_json` ·
[`schemars`](https://crates.io/crates/schemars) (JSON Schemas for structured LLM output) ·
[`reqwest`](https://crates.io/crates/reqwest) (rustls; no OpenSSL) ·
[`tokio`](https://crates.io/crates/tokio)/`futures` (async runtime, batch concurrency) ·
[`gix`](https://crates.io/crates/gix) (pure-Rust Git: history, diffs, staged
changes, shallow clones) · [`tempfile`](https://crates.io/crates/tempfile) ·
[`indicatif`](https://crates.io/crates/indicatif)/`console` (progress) ·
[`tracing`](https://crates.io/crates/tracing) (structured logging) ·
[`thiserror`](https://crates.io/crates/thiserror)/`anyhow` (errors).

Development: `insta` (snapshot tests), `assert_cmd`/`predicates` (CLI
end-to-end), `wiremock` (HTTP-level provider tests), `tempfile`.

The one deliberate exception to pure-Rust Git: creating a commit shells out
to your `git` binary so `pre-commit`/`commit-msg` hooks, `commit.gpgsign`,
and identity resolution behave exactly like a hand-typed `git commit`.

## Development

Requirements, design decisions, and their rationale live in
[`docs/`](docs/README.md) — the product definition in
[`docs/product.md`](docs/product.md) and the technical RFCs in
[`docs/rfcs/`](docs/rfcs/README.md).

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test          # deterministic: mock provider, fixture repos, no keys needed
```
