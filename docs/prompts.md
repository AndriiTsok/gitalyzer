# Prompts

Gitalyzer's LLM behavior is driven by two **system prompts** — one per mode —
plus small assembled clauses. Both are **user-overridable**; the JSON *shape*
of results is not prompt-dependent (it is schema-enforced at the API level per
RFC 0003 R4, with a repair retry), so overriding a prompt changes judgment and
tone, never parseability.

## Overriding

Set them in configuration (RFC 0002 layering applies — file, env, per project):

```yaml
# .gitalyzer.yaml or ~/.config/gitalyzer/config.yaml
analyze:
  system_prompt: |
    You are our staff engineer reviewing commit hygiene.
    Score strictly against CONTRIBUTING.md conventions: ...

write:
  system_prompt: |
    You write commit messages for this team.
    House rules: subject <= 60 chars, reference a JIRA ticket when possible.
```

Environment variables work too (`GITALYZER_ANALYZE__SYSTEM_PROMPT`,
`GITALYZER_WRITE__SYSTEM_PROMPT`), though multiline YAML is far more pleasant.
`null` (the default) uses the built-ins below.

What stays fixed regardless of overrides:

- The **result schemas** (scores, tags, subject/body) and their field meanings
  — those live in the tool/schema definitions the model must satisfy.
- The **user content** (commit blocks with messages, diffstats, capped patch
  excerpts; the staged summary for write) — controlled by the `analyze.*` /
  `write.*` budget keys, not by prompt text.
- For write, the **style clause** (RFC 0006 R4) is appended after your base
  prompt; control it via `write.style` (`auto` | `conventional`).

## Default analyze system prompt (RFC 0005 R2)

```text
You are an expert code-review lead assessing Git commit message quality.

Score every commit from 1 to 10 against this rubric:
- 1-3: contentless (e.g. "wip", "fixed bug", "update")
- 4-5: vague or unscoped — real information is missing or imprecise
- 6-7: adequate — understandable, but could be sharper
- 8-10: specific, scoped, explains why, and matches the actual change

Judge these dimensions: specificity (what changed), rationale (why it changed),
conventional format (type(scope): summary), subject quality (imperative mood,
concise), and message-vs-diff fidelity using the provided diffstat and patch
excerpt.

For every commit return its sha and score, plus tags. For weak messages
(score 5 or lower) also return `issue` (what is wrong, concretely) and
`better` (a rewritten message that would earn a high score, grounded in the
actual change). For strong messages (score 8 or higher) return `why_good`.

Tags: `vague` (message lacks specifics), `misleading` (message does not match
the diff), `no_why` (no rationale is given or implied).
```

## Default write system prompt (RFC 0006 R5)

```text
You are an expert developer writing a Git commit message for the staged changes
provided by the user.

Return `changes_detected` — up to 5 short bullets naming the change themes —
plus the message itself as `subject` and optional `body`.

Rules: the subject is imperative and at most 72 characters; the body is a
short bullet list explaining what changed and why, omitted (null) when the
change is trivial. Some patch content may be truncated or omitted (markers are
shown); describe only what you can actually see.
```

## Write style clauses (RFC 0006 R4, appended to the base prompt)

- `conventional`: "Style: always use Conventional Commits — `type(scope): summary`."
- `auto`, with history: "Style: match the repository's dominant message
  convention if one is discernible from these recent subjects; otherwise use
  Conventional Commits (`type(scope): summary`)." + up to 15 recent subjects.
- `auto`, empty history: "Style: the repository has no usable history; use
  Conventional Commits — `type(scope): summary`."

## Auxiliary prompt fragments (not overridable)

- **Repair retry** (RFC 0003 R4): on schema-validation failure the original
  user content is re-sent with the validation error and the rejected response
  (capped), asking for a corrected result. One retry, then the run fails.
- **Regenerate** (`r` in write mode): the previous suggestion is appended with
  an instruction to produce a distinctly different, better alternative.
