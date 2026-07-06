# RFC 0005: Analysis Mode

- **Status:** Accepted
- **Author(s):** Andrii Tsok
- **Created:** 2026-07-06
- **Supersedes:** —
- **Superseded by:** —

## Summary

Specifies `gitalyzer analyze` end-to-end: the per-commit critique schema and scoring
rubric, fully configurable batching, report composition with score thresholds,
locally computed statistics, and the stable JSON payload (printable or written to a
file via `--output`).

## Pipeline

1. Collect commits and their context (RFC 0004 R1–R3, R5).
2. Chunk into batches per the batching configuration.
3. One schema-enforced LLM task per batch (RFC 0003 R3–R4), with progress feedback.
4. Merge per-commit critiques; compute stats locally.
5. Render the report — human or JSON, to stdout or `--output <path>`.

## Requirements

- **R1.** Per commit, the LLM returns (schema-enforced):

  ```json
  {
    "sha": "1a2b3c4",
    "score": 2,
    "issue": "Too vague - which bug? What was the impact?",
    "better": "fix(auth): resolve token expiration handling",
    "why_good": null,
    "tags": { "vague": true, "misleading": false, "no_why": true }
  }
  ```

  `score` is an integer 1–10. `issue` + `better` are required for weak messages;
  `why_good` for strong ones. Tags are LLM judgments only — `one_word` is computed
  locally (word count of the subject line), never asked of the model.
- **R2.** The scoring rubric is embedded in the prompt and anchored: 1–3 contentless
  (`wip`, `fixed bug`), 4–5 vague or unscoped, 6–7 adequate, 8–10 specific, scoped,
  explains *why*, and matches the actual change. Dimensions: specificity, rationale,
  conventional format (`type(scope): summary`), subject quality (length, imperative
  mood), and message-vs-diff fidelity. Exact prompt wording is an implementation
  detail; the rubric semantics are fixed here.
- **R3.** Per-commit context sent to the model: short SHA, full message, diffstat,
  file list (capped at 20 paths), and the patch excerpt capped by
  `analyze.max_patch_bytes` (default `4096`; `0` disables patch content entirely —
  the cheap/private mode). Truncations carry an explicit marker.
- **R4.** Batching is fully configuration-driven (file, env, CLI per RFC 0002):
  - `analyze.batch_size` (default `10`; `0` = no batching, one request for all
    commits) — CLI `--batch-size`, env `GITALYZER_ANALYZE__BATCH_SIZE`;
  - `analyze.concurrency` (default `1` = sequential; `N` batches in flight
    otherwise) — env `GITALYZER_ANALYZE__CONCURRENCY`.
- **R5.** Report thresholds are configurable and validated
  (`needs_work < well_written` required):
  - `analyze.thresholds.needs_work` (default `5`): score ≤ threshold → 💩 section,
    worst first;
  - `analyze.thresholds.well_written` (default `8`): score ≥ threshold → 💎 section,
    best first;
  - the middle band appears in stats only.
- **R6.** Stats are computed locally, deterministically: average score (one decimal),
  vague count/percent (LLM tag), one-word count/percent (local check) — over all
  analyzed commits. Additional tag counts (e.g. misleading) MAY be shown when
  non-zero.
- **R7.** Human report layout follows PRD §4.1 (sections, separators, emoji);
  rendering details in RFC 0007.
- **R8.** JSON payload (`--format json`) is a stable, versioned envelope containing
  **every** analyzed commit regardless of bucket:

  ```json
  {
    "schema_version": 1,
    "mode": "analyze",
    "repository": { "source": "local", "url": null },
    "range": { "from": "HEAD", "requested": 50, "analyzed": 48 },
    "commits": [
      {
        "sha": "...", "short_sha": "1a2b3c4",
        "author": "...", "date": "2026-07-06T12:00:00Z",
        "message": "fixed bug",
        "files_changed": 3, "insertions": 12, "deletions": 4,
        "score": 2, "issue": "...", "better": "...", "why_good": null,
        "tags": { "vague": true, "misleading": false, "no_why": true },
        "one_word": false
      }
    ],
    "stats": {
      "average_score": 4.2,
      "vague": { "count": 34, "percent": 68 },
      "one_word": { "count": 12, "percent": 24 }
    },
    "meta": { "provider": "anthropic", "model": "...", "batches": 5 }
  }
  ```

- **R9.** `--output <path>` (RFC 0001 R11) writes the rendered report to the file;
  progress and errors stay on the terminal. In human format, stdout confirms
  `Report written to <path>`.
- **R10.** If a batch fails after the RFC 0003 retry policy, the run aborts with an
  actionable error (exit `1`) — no partial report in v1.

## Alternatives considered

- **Multi-dimension subscores** — richer diagnostics, but more tokens/schema for a
  UI that displays one number.
- **LLM-composed stats** — numbers can contradict the scores just given; local
  arithmetic is free and always consistent.
- **Fixed top/bottom-K report** — hides how widespread problems are; thresholds keep
  the report proportional to actual quality.

## Deferred

- Partial-result tolerance on batch failure.
- Score distribution/histogram in stats.
- Per-commit result caching across runs.

## References

- PRD §4.1; RFC 0001 R4/R6/R11; RFC 0002 (config); RFC 0003 R3–R4/R9; RFC 0004 R1–R3/R5.

## Changelog

- 2026-07-06 — Amended: the R2 system prompt is overridable via
  `analyze.system_prompt` (structured output remains schema-enforced); R4
  batching additionally packs under a hard per-request byte ceiling
  (`analyze.max_batch_bytes`, default 256 KiB) so `batch_size: 0` over large
  ranges cannot overflow a model's context window; commit messages inside
  prompts are capped; output-token budgets scale with batch size.
