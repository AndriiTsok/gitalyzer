# RFC 0006: Write Mode

- **Status:** Accepted
- **Author(s):** Andrii Tsok
- **Created:** 2026-07-06
- **Supersedes:** —
- **Superseded by:** —

## Summary

Specifies `gitalyzer write` end-to-end: staged-change ingestion with context-window
budgeting that never fails on large commits, style-aware suggestions (configurable —
inferred from the repository by default, Conventional Commits as fallback), an
accept / type / regenerate loop with hook-failure recovery, commit creation through
the system `git` (RFC 0004 R6), and the JSON payload.

## Requirements

- **R1.** Preconditions: inside a Git repository with staged changes — otherwise an
  actionable error (exit `1`). The interactive flow requires a TTY; without one, the
  error points at `--format json` or `--dry-run`.
- **R2.** Context sent to the model: staged summary (files changed, insertions,
  deletions), the full file list with per-file stats, per-file patch content subject
  to budgeting (R3), and recent commit subjects when style inference is active (R4).
- **R3.** Context budgeting — the command MUST NOT fail because of diff size.
  Managing the context window is core business logic:
  - per-file patch cap: `write.max_file_patch_bytes` (default `8192`);
  - total patch budget: `write.max_diff_bytes` (default `65536`);
  - generated/vendored/lock content (e.g. `Cargo.lock`, `package-lock.json`, build
    and vendor directories, minified assets) is always listed but its patch content
    excluded;
  - on overflow, degrade gracefully: drop patch content for the largest files first,
    down to file list + stats only if necessary — always with explicit truncation
    markers so the model knows what it isn't seeing;
  - all knobs follow RFC 0002 (file, `GITALYZER_WRITE__*` env vars). Model-aware
    token budgets are deferred.
- **R4.** Message style is configurable via `write.style`:
  - `auto` (default): include up to 15 recent non-merge subjects from the repository;
    the model matches the dominant discernible convention, falling back to
    Conventional Commits when history is absent or inconsistent;
  - `conventional`: always `type(scope): summary` plus explanatory bullets;
  - custom templates are deferred.
- **R5.** The LLM task returns (schema-enforced, RFC 0003 R4):

  ```json
  {
    "changes_detected": ["Modified authentication logic", "..."],
    "subject": "refactor(auth): improve error handling",
    "body": "- Add specific error types for auth failures\n- ..."
  }
  ```

- **R6.** Interactive loop — after showing the staged summary, detected changes, and
  the suggestion:

  ```text
  Press Enter to accept, type your own message, or 'r' to regenerate:
  ```

  - **Enter** → commit the suggestion;
  - **typed text** → commit that text as the message;
  - **`r`** → a new LLM call, instructed to produce a distinct alternative (the
    previous suggestion is passed as context); the loop repeats, unbounded.
- **R7.** Committing goes through the system `git` (RFC 0004 R6); success prints a
  confirmation with the short SHA and subject.
- **R8.** Hook rejection (`pre-commit`/`commit-msg`): show the hook output verbatim,
  keep the staged state untouched, and return to the R6 prompt — no lost work, no
  wasted re-run. Ctrl-C exits cleanly with staged changes intact.
- **R9.** `--dry-run` (RFC 0001, amended): the full flow, but the final message is
  printed (or written via `--output`) instead of committed. In `--format json` the
  command is always non-interactive and never commits (RFC 0001 R6).
- **R10.** JSON envelope:

  ```json
  {
    "schema_version": 1,
    "mode": "write",
    "staged": { "files_changed": 12, "insertions": 247, "deletions": 89,
                "files": ["src/auth.rs", "..."] },
    "changes_detected": ["Modified authentication logic", "..."],
    "suggestion": { "subject": "refactor(auth): improve error handling",
                    "body": "- ...", "style": "conventional" },
    "meta": { "provider": "anthropic", "model": "..." }
  }
  ```

## Alternatives considered

- **Plain accept/type** — no recovery when a suggestion is *almost* right; one `r`
  fixes that cheaply.
- **Abort on hook rejection** — wastes the LLM call and forces a full re-run against
  strict `commit-msg` hooks.
- **Two-pass map-reduce for big diffs** — handles any size faithfully but doubles
  calls and latency; graceful capping chosen instead (revisit if quality suffers).

## Deferred

- `$EDITOR` editing of the suggestion before committing.
- Custom style templates (`write.style: template`).
- Model-aware token budgeting (per-model context sizes).
- Critiquing a user-typed message before committing (score-your-own flow).

## References

- PRD §4.2; RFC 0001 R5–R6/R11; RFC 0003 R3–R4; RFC 0004 R4/R6.

## Changelog

- 2026-07-06 — Amended: the R5 base system prompt is overridable via
  `write.system_prompt`; the R4 style clause is still appended after it.
