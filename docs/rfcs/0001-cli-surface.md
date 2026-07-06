# RFC 0001: CLI Surface

- **Status:** Accepted
- **Author(s):** Andrii Tsok
- **Created:** 2026-07-06
- **Supersedes:** —
- **Superseded by:** —

## Summary

Defines Gitalyzer's command-line interface: a subcommand-based structure, flexible
history selection, human and JSON output formats, and per-invocation provider/model
overrides. This is the contract the mode RFCs (0005, 0006) and the implementation
build against.

## Motivation

The CLI is Gitalyzer's entire user surface. It must serve two audiences at once:
humans (rich formatting, progress indication — responses are never instantaneous) and
programs (stable JSON for scripting, CI, and editor integrations). Locking the shape
early keeps later RFCs and code aligned.

## Requirements

- **R1.** The binary MUST be named `gitalyzer` and expose the subcommands `analyze`
  and `write`.
- **R2.** Bare `gitalyzer` (no arguments) MUST print help to stdout and exit `0`.
  `--help`/`-h` and `--version`/`-V` MUST be supported.
- **R3.** `analyze` MUST default to the current repository and the last 50 commits,
  walking backwards from `HEAD`.
- **R4.** `analyze` MUST support flexible history selection:
  - `--url <git-url>` — analyze a remote repository instead of the current one;
  - `-n, --count <N>` — number of commits to analyze (default: 50);
  - `--from <revision>` — start walking from this commit instead of `HEAD`; accepts
    anything Git resolves (SHA, branch, tag, `HEAD~20`);
  - `--batch-size <N>` — commits per LLM request (default and semantics: RFC 0005).
- **R5.** `write` MUST operate on the staged changes of the current repository and be
  interactive by default.
- **R6.** A global `--format <human|json>` flag MUST select the output format
  (default: `human`). In JSON mode: output MUST be stable, documented, and
  machine-parseable; progress indication and decoration MUST be suppressed; `write`
  MUST run non-interactively (emit the suggestion and exit). Exact JSON shapes are
  specified per mode in RFCs 0005 and 0006.
- **R7.** Global `--provider <id>` and `--model <name>` flags MUST override the
  configured values. Precedence: CLI flag > environment variable > config file >
  built-in default (full configuration model: RFC 0002).
- **R8.** A global `--config <path>` flag MUST select an explicit configuration file
  (discovery rules: RFC 0002).
- **R9.** In human format, long-running steps (LLM calls, remote cloning) SHOULD show
  progress indication (spinner/step feedback — specified in RFC 0007).
- **R10.** Exit codes: `0` success, `1` runtime failure, `2` CLI usage error.

## Canonical shape

```text
gitalyzer [GLOBAL FLAGS] <COMMAND>

Commands:
  analyze   Critique recent commit messages of a repository
  write     Suggest a commit message for the staged changes

Global flags:
  --provider <id>        Override the configured LLM provider
  --model <name>         Override the configured model
  --format <human|json>  Output format (default: human)
  --config <path>        Use an explicit configuration file

Examples:
  gitalyzer analyze
  gitalyzer analyze --url="https://github.com/example/project"
  gitalyzer analyze -n 100 --from 1a2b3c4
  gitalyzer analyze --format json
  gitalyzer write
```

## Alternatives considered

- **Mode flags (`--analyze`/`--write`)** as in the early PRD examples — rejected:
  scales poorly and permits contradictory combinations; PRD updated to subcommands.
- **Hidden flag aliases alongside subcommands** — rejected: one canonical surface.
- **Interactive mode picker on bare invocation** — rejected in favor of help output:
  predictable, and no accidental LLM spend.

## Deferred

- Batch-size default and batching semantics → RFC 0005.
- JSON payload schemas → RFCs 0005 (analyze) and 0006 (write).
- Progress/formatting details, verbosity flags, shell completions → RFC 0007.

## References

- PRD: [`../product.md`](../product.md), §4.
