# RFC 0007: Output, Logging & Testing

- **Status:** Accepted
- **Author(s):** Andrii Tsok
- **Created:** 2026-07-06
- **Supersedes:** —
- **Superseded by:** —

## Summary

Cross-cutting behavior: terminal rendering and progress indication, strict JSON-mode
cleanliness, decoration controls, structured logging, the privacy posture, and the
testing strategy the implementation must follow.

## Requirements

### Output & progress

- **R1.** Human mode shows rich progress on **stderr** — spinner, step labels,
  per-batch counters ("batch 2/5, commits 11–20") — degrading automatically to plain
  text lines when stderr is not a TTY (CI, pipes). The final report renders to stdout
  or the `--output` file.
- **R2.** JSON mode is **absolutely clean**: stdout carries exactly one JSON document
  and nothing else; no progress, spinners, or decoration on any stream. stderr is
  silent except for fatal errors — or logging the user explicitly requested via
  `-v`/`GITALYZER_LOG`.
- **R3.** Decoration (color, emoji) auto-detects: on for TTYs, off for pipes/CI; the
  `NO_COLOR` convention is honored; a global `--no-color` flag forces plain output.
  Undecorated reports use plain-text headers and ASCII separators.
- **R4.** Human report sections follow the PRD mockups (💩 / 💎 / 📊 with heavy-line
  separators) when decorated.

### Logging

- **R5.** Structured logging (`tracing`) to stderr. Default: errors/warnings only;
  `-v` = debug (batch sizes, request timing, git operation stats); `-vv` = trace
  (full prompts/responses). `GITALYZER_LOG` accepts fine-grained filter directives
  and overrides the flags. `-v`/`-vv` are global flags (RFC 0001, amended).
- **R6.** Log format is human-readable by default; `GITALYZER_LOG_FORMAT=json`
  switches to JSON lines with structured fields preserved.
- **R7.** Secrets are never logged: API keys are redacted at every level, including
  trace-level request dumps.

### Privacy & safety

- **R8.** Data leaves the machine only toward the configured provider endpoint.
  Patch content in analysis can be disabled entirely (`analyze.max_patch_bytes: 0`,
  RFC 0005 R3). The README MUST disclose what each mode sends where.
- **R9.** Ctrl-C exits cleanly everywhere: temporary clones removed (RFC 0004 R5),
  staged state untouched (RFC 0006 R8), terminal state restored.

### Performance

- **R10.** Runtime is provider-dominated by nature; Gitalyzer's own overhead
  (startup + git reads) SHOULD stay under ~1 s for a 50-commit local analysis, and
  memory stays bounded by the patch caps (RFCs 0005 R3, 0006 R3).

### Testing

- **R11.** A deterministic pyramid with **no network or API keys in CI**:
  - unit tests per module;
  - a **mock provider** implementing the RFC 0003 trait with canned schema-valid
    responses *and* fault modes (invalid JSON → repair path, 429 → retry path);
  - **git fixture repositories** scripted into temp dirs (good/bad/mixed histories,
    staged states, hooks);
  - **snapshot tests** (`insta`) for human and JSON output of both modes;
  - end-to-end CLI runs against fixtures with the mock provider selected via config.
- **R12.** Live-API smoke tests are deferred; when added they stay opt-in and
  env-gated, never default CI.

## Alternatives considered

- **Plain-only progress** — works everywhere but wastes the interactive terminal
  experience; auto-degradation gives both.
- **Config-driven color (`ui.color`)** — re-implements what TTY detection +
  `NO_COLOR` + `--no-color` already solve.
- **Live provider tests in CI** — flaky and key-dependent; the mock provider
  exercises every contract deterministically.

## Deferred

- `ui.progress` style override in config.
- Log-to-file option.
- Shell completions (re-deferred from RFC 0001) — trivial with clap once the surface
  stabilizes; post-v1.
- Live smoke-test suite design.

## References

- PRD §6; RFC 0001 R6/R9 (+ amendments); RFC 0003 R9; RFC 0004 R5; RFC 0005 R3/R7;
  RFC 0006 R3/R8.
