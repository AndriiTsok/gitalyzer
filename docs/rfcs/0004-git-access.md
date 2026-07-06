# RFC 0004: Git Access

- **Status:** Accepted
- **Author(s):** Andrii Tsok
- **Created:** 2026-07-06
- **Supersedes:** —
- **Superseded by:** —

## Summary

Gitalyzer uses **gitoxide (`gix`)** — pure Rust — for all read-side Git work:
repository discovery, revision resolution, history walking, diffs, staged-change
extraction, and shallow-cloning remote repositories. The single write-side operation,
creating a commit in write mode, shells out to the **system `git` binary** so hooks,
signing, and user configuration behave exactly as a hand-typed `git commit`.

## Motivation

Pure-Rust Git access keeps the binary self-contained — no subprocess text parsing, no
native C dependency, typed object access for fast analysis. Committing, however, must
be indistinguishable from the user's own workflow: `commit-msg`/`pre-commit` hooks,
`commit.gpgsign`, conditional identity includes. gitoxide does not replicate those
today, so that one operation delegates to `git` for full fidelity at trivial cost.

## Requirements

- **R1.** All read operations go through `gix`: repository discovery (upward from the
  working directory), resolution of `--from` revisions, history walking in `git log`
  order, commit metadata and messages, diffstats and patches, and staged-change
  detection.
- **R2.** History walking collects the requested number of **non-merge commits**;
  merge commits are skipped by default (their messages are largely auto-generated and
  would distort critique and stats).
- **R3.** Analysis extraction per commit: short + full SHA, author, authored date,
  full message (subject and body), diffstat (files changed, insertions, deletions,
  file paths), and a **patch excerpt capped per commit** with an explicit truncation
  marker — the critic must be able to judge whether the message matches the actual
  change. Cap sizes and prompt budgeting live in RFC 0005.
- **R4.** Write mode reads staged changes via `gix`: summary stats plus the full
  staged patch (budgeting in RFC 0006). No staged changes is an actionable error.
- **R5.** Remote analysis (`--url`) performs a **bare shallow clone** into a
  temporary directory: depth = requested count + a small buffer, single branch
  (the remote's default). If `--from`/`--count` exceed the shallow boundary, the
  clone is deepened (or re-cloned with sufficient depth) automatically. The temporary
  directory is removed on normal exit, error, and Ctrl-C (best effort on hard kill).
- **R6.** Commit creation (accepting in write mode) invokes the system `git commit`
  with the message on stdin, in the repository root — hooks, signing, and identity
  resolution all apply. Hook output is shown to the user; a missing `git` binary is
  an actionable error (only commit creation requires it). The resulting short SHA is
  reported. Hook-rejection flow is specified in RFC 0006.
- **R7.** Analyze mode never mutates the local repository; remote clones are
  read-only throwaways.
- **R8.** Failures are actionable, exit `1`: not inside a repository, unresolvable
  `--from`, empty history, unreachable/invalid remote URL.

## Alternatives considered

- **System `git` for everything** — maximum fidelity but means parsing text output
  everywhere and requiring the binary for all modes; declined in favor of typed
  in-process reads (git is needed only when actually committing).
- **`git2` (libgit2)** — mature, but adds a native C dependency and has the same
  hook/signing gap for commit creation.
- **Pure-`gix` commit creation** — bypasses hooks and signing today; rejected for
  fidelity. Revisit in a future RFC once gitoxide supports them.
- **Hosting-provider APIs for remotes** — host lock-in and rate limits; the PRD says
  *any* Git repository.

## Implementation notes

`gix` with blocking network features for clone/fetch; `tempfile` for the clone
directory (RAII cleanup) plus a Ctrl-C handler to make interrupt cleanup explicit.
Final dependency set is locked in the implementation-bootstrap RFC.

## Deferred

- Patch-excerpt and token budgets → RFC 0005 (analyze) / RFC 0006 (write).
- An opt-in flag to include merge commits → future RFC.
- Native `gix` commit creation (when hooks/signing land upstream) → future RFC.

## References

- RFC 0001 R3–R5 (history selection, `--url`); RFC 0005; RFC 0006.
