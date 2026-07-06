# RFC 0008: Implementation Bootstrap

- **Status:** Accepted
- **Author(s):** Andrii Tsok
- **Created:** 2026-07-06
- **Supersedes:** —
- **Superseded by:** —

## Summary

Locks the project skeleton and the dependency set so implementation can start:
a single binary crate with a library core, Tokio-based async, edition 2024, the
vetted crate list below, and the slice-by-slice build order.

## Project shape

- **Single crate** `gitalyzer`: `src/lib.rs` (core, fully testable) + `src/main.rs`
  (thin entry: parse CLI → run → map errors to exit codes per RFC 0001 R10).
- **Edition 2024**, `rust-version = "1.85"` (raised only if a locked dependency
  demands it, with a changelog note).
- **Tokio** multi-thread runtime; async lives at the provider/orchestration layer
  (`buffer_unordered` implements `analyze.concurrency`, RFC 0005 R4). The git layer
  stays synchronous (`gix` is sync by design).

```text
src/
  main.rs        thin CLI entry, exit-code mapping
  lib.rs         module wiring, public API for tests
  cli.rs         RFC 0001 — clap definitions
  config.rs      RFC 0002 — layered loading, env mapping
  provider/      RFC 0003 — trait, anthropic.rs, openai.rs, mock.rs
  git/           RFC 0004 — repo.rs (local), remote.rs (clone), staged.rs
  analyze.rs     RFC 0005 — batching, scoring pipeline, stats
  write.rs       RFC 0006 — suggestion loop, commit handoff
  output/        RFC 0007 — human renderer, json renderer, progress
tests/
  fixtures/      scripted git repos (good/bad/mixed histories, staged states)
  e2e_analyze.rs / e2e_write.rs   CLI runs w/ mock provider
  snapshots/     insta snapshots (human + JSON)
```

## Dependencies (locked)

| Crate | Role | Why this one |
| --- | --- | --- |
| `clap` (derive) | CLI (RFC 0001) | de-facto standard; subcommands, env fallbacks, `--help` quality |
| `config` | layered config (RFC 0002) | native file+env layering with `__` separator; YAML via maintained `yaml-rust2` (honors 0002's maintained-YAML requirement) |
| `serde`, `serde_json` | (de)serialization everywhere | ecosystem baseline |
| `schemars` | JSON Schemas for structured output (RFC 0003 R4) | derives schemas from the same types we deserialize into |
| `reqwest` (rustls-tls, json) | provider HTTP (RFC 0003) | async-first, no OpenSSL linkage |
| `tokio` (rt-multi-thread, macros, signal), `futures` | runtime, concurrency, Ctrl-C | standard; `buffer_unordered` for batch concurrency |
| `gix` (blocking network, revision, diff features) | git reads + shallow clones (RFC 0004) | pure Rust, typed access; credential-helper + ssh transport support (R9) |
| `tempfile` | remote clone dirs (RFC 0004 R5) | RAII cleanup |
| `indicatif` + `console` | progress + decoration (RFC 0007 R1/R3) | pair designed together; TTY detection, `NO_COLOR` respected |
| `tracing`, `tracing-subscriber` (env-filter, json) | structured logging (RFC 0007 R5–R6) | standard structured logging; `GITALYZER_LOG` via EnvFilter |
| `thiserror` | typed domain errors | error taxonomy maps cleanly to exit codes |
| `anyhow` | binary-level error context | ergonomic top-level reporting |

Dev-dependencies: `insta` (snapshots), `assert_cmd` + `predicates` (CLI e2e),
`wiremock` (HTTP-level adapter tests: request shaping, 429 retry, repair retry),
`tempfile`.

Deliberately **no** dependency for: config paths (`$XDG_CONFIG_HOME` else
`~/.config`, uniform on all platforms, via `std::env`), interactive prompts (plain
stdin reads cover Enter/text/`r`, RFC 0006 R6).

## Tooling & conventions

- `cargo fmt` (default style) and `cargo clippy --all-targets -- -D warnings` must
  pass before any commit; lint tuning lives in `Cargo.toml` `[lints]`.
- Conventional Commits (`feat`, `fix`, `docs`, `refactor`, `test`, `chore`, …).
- CI (GitHub Actions: fmt + clippy + test) lands once a remote exists — deferred.

## Implementation order

Reviewable slices, each landing with its tests:

1. **Scaffold** — `cargo init`, CLI skeleton (all RFC 0001 flags parsing), config
   loading (RFC 0002), exit codes; e2e: `--help`, config precedence.
2. **Git layer** — local history walk + staged extraction + fixtures (RFC 0004).
3. **Provider layer** — trait, anthropic/openai adapters, mock, retries (RFC 0003).
4. **Analyze** — batching, scoring, stats, human/JSON reports (RFC 0005).
5. **Write** — suggestion loop, commit handoff, hook recovery (RFC 0006).
6. **Remote & polish** — `--url`/`--branch` clones, progress UI, decoration,
   logging depth (RFCs 0004/0007), README per PRD §7.

## Alternatives considered

- **Cargo workspace** — stronger boundaries, unnecessary ceremony at this size;
  the lib/bin split keeps the core testable and extractable later.
- **`figment`** for config — elegant providers, but its YAML path rides the archived
  `serde_yaml`; `config`+`yaml-rust2` satisfies RFC 0002's maintenance requirement.
- **Blocking `reqwest` + threads** — fewer deps, but concurrency, timeouts, and
  retry composition all get hand-threaded; Tokio is the ecosystem path.

## References

- All prior RFCs (0001–0007); PRD §7 (README duties).
