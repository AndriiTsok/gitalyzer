# Gitalyzer — Product Requirements Document (PRD)

- **Status:** Living document (v0.1)
- **Owner:** Andrii Tsok
- **Last updated:** 2026-07-06

## 1. Overview

Gitalyzer is an AI-powered terminal tool that helps developers write better commit
messages. It reviews the quality of existing Git commit history with LLM-generated
critique, and it helps author new, well-formed commit messages directly from staged
changes.

This document defines **what** Gitalyzer must do, from a product perspective. It
deliberately avoids deep technical detail — architecture, library choices, and other
implementation decisions are captured separately as RFCs under [`rfcs/`](rfcs/README.md).

## 2. Problem

Commit messages are the primary record of *why* a codebase changed, yet real-world
histories are full of entries like `fixed bug`, `wip`, or `update`. Poor messages slow
down code review, debugging, release preparation, and onboarding. Conventional linters
can check formatting at best — they cannot judge whether a message actually explains
the change.

Gitalyzer closes that gap: it scores and critiques messages the way an experienced
reviewer would, and coaches developers toward better ones at the moment of committing.

## 3. Target users

- Individual developers who want honest feedback on their commit hygiene.
- Team leads who want a quick quality read on a repository's history.
- Anyone preparing a commit who wants a well-structured message derived from their
  actual staged changes.

## 4. Core functionality

Gitalyzer is a terminal application with two modes.

### 4.1 Analysis mode

- Analyzes commit messages from **any Git repository**.
- Defaults to the **current repository** and the **last 50 commits**; the commit
  count and the starting commit are selectable.
- Can analyze a **remote repository** instead, given its URL.
- For each reviewed commit, produces: a quality **score out of 10**, a concrete
  **critique** (what is wrong or what makes it good), and — for weak messages — a
  **better alternative**.
- Groups results into "needs work" and "well-written" sections, and closes with
  **aggregate statistics** (average score, share of vague commits, share of one-word
  commits, and similar).

Expected shape of an analysis session:

```text
$ gitalyzer analyze

Analyzing last 50 commits...

━━━━━━━━━━━━━━━━━━━━━━━━━━━━
💩 COMMITS THAT NEED WORK
━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Commit: "fixed bug"
Score: 2/10
Issue: Too vague - which bug? What was the impact?
Better: "fix(auth): resolve token expiration handling"

Commit: "wip"
Score: 1/10
Issue: No information about what's in progress
Better: Describe what you're working on

━━━━━━━━━━━━━━━━━━━━━━━━━━━━
💎 WELL-WRITTEN COMMITS
━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Commit: "feat(api): add Redis caching layer
         - Implement cache for read endpoints
         - Add TTL configuration
         - Improves response time by 200ms"
Score: 9/10
Why it's good: Clear scope, specific changes, measurable impact

━━━━━━━━━━━━━━━━━━━━━━━━━━━━
📊 YOUR STATS
━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Average score: 4.2/10
Vague commits: 34 (68%)
One-word commits: 12 (24%)
```

### 4.2 Interactive mode (commit writer)

- Reads the **staged changes** of the current repository (`git diff --staged`).
- Summarizes what changed: file/line counts plus a short thematic breakdown.
- Suggests a **well-formatted commit message** (conventional style: typed, scoped
  summary line plus explanatory bullets).
- Lets the user **accept the suggestion with Enter or type their own message**.

Expected shape of an interactive session:

```text
$ gitalyzer write

Analyzing staged changes... (12 files changed, +247 -89 lines)

Changes detected:
- Modified authentication logic
- Added error handling
- Updated unit tests

Suggested commit message:
━━━━━━━━━━━━━━━━━━━━━━━━━━━━
refactor(auth): improve error handling

- Add specific error types for auth failures
- Extract validation into separate methods
- Update tests to cover edge cases
━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Press Enter to accept, or type your own message:
>
```

### 4.3 Invocation

```text
# Analyze the last 50 commits of the current repository
gitalyzer analyze

# Analyze the last 50 commits of a remote repository
gitalyzer analyze --url="https://github.com/example/project"

# Analyze 100 commits starting from a specific commit
gitalyzer analyze -n 100 --from 1a2b3c4

# Machine-readable results for scripting and CI
gitalyzer analyze --format json

# Interactively write a commit message for staged changes
gitalyzer write
```

The canonical CLI surface is specified in [RFC 0001](rfcs/0001-cli-surface.md).

## 5. LLM providers & configuration

- Gitalyzer works with **multiple LLM providers**; the user chooses the provider and
  model.
- All settings come from a **configuration file and/or environment variables**:
  - the configuration file provides defaults;
  - **every** configuration value can be overridden — or supplied outright — by an
    environment variable following a predictable naming convention.
- API credentials are provided via configuration/environment only and are never stored
  in the repository.
- Sensible defaults: once credentials are configured, the tool works with minimal
  further setup.

## 6. Output & UX expectations

- Clear, structured, scannable terminal output: sections, separators, and emoji
  accents as shown in the examples above.
- A machine-readable **JSON output mode** so results can be consumed programmatically
  (scripting, CI, editor integrations).
- Progress feedback for long-running steps (e.g. "Analyzing last 50 commits...") —
  responses involve LLM calls and are never instantaneous.
- Friendly, actionable error messages for the common failure cases: missing
  credentials, not inside a Git repository, nothing staged, unreachable remote URL.

## 7. Documentation

- The repository `README.md` must contain a detailed explanation of the project,
  setup instructions, how to run each mode, and the dependency list.

## 8. Out of scope (initial)

- No GUI or web interface — Gitalyzer is terminal-only.
- No automatic rewriting of existing commit history; analysis mode is read-only.

## 9. Open questions

To be resolved together in the requirements RFC before implementation:

- Should accepting a suggestion in interactive mode create the commit directly, or
  only emit the message?
- How should very large histories or very large staged diffs be handled?

## 10. Success criteria

- Running analysis mode against a real repository produces critique matching the
  documented output shape.
- Interactive mode produces a ready-to-use, well-formed message from real staged
  changes.
- Switching LLM providers requires only configuration changes — never code changes.
