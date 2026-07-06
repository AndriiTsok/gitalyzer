# RFCs

RFCs (Requests for Comments) are how we record **requirements** and **design decisions**
for Gitalyzer. Nothing significant gets built until the relevant RFC is agreed on.

## Why RFCs

- They make requirements explicit and reviewable before code exists.
- They give every architectural decision a durable, referenceable rationale.
- They let us collaborate asynchronously and keep a decision history.

## Process

1. **Draft** — copy [`0000-template.md`](0000-template.md) to the next number, e.g.
   `0001-repository-analysis-requirements.md`. Set status to `Draft`.
2. **Discuss** — iterate together until the scope and approach are agreed.
3. **Accept** — set status to `Accepted`. It now guides implementation.
4. **Supersede** — if a later RFC replaces it, mark it `Superseded by NNNN` rather than
   deleting it. History stays intact.

## Statuses

`Draft` → `Accepted` → (`Superseded` | `Rejected`)

## Numbering

- Four-digit, zero-padded, monotonically increasing (`0001`, `0002`, …).
- `0000` is reserved for the template.

## Index

| RFC  | Title    | Status |
| ---- | -------- | ------ |
| [0001](0001-cli-surface.md) | CLI Surface | Accepted |
| [0002](0002-configuration.md) | Configuration | Accepted |
| [0003](0003-llm-providers.md) | LLM Providers | Accepted |
| [0004](0004-git-access.md) | Git Access | Accepted |
| [0005](0005-analysis-mode.md) | Analysis Mode | Accepted |
| [0006](0006-write-mode.md) | Write Mode | Accepted |
| [0007](0007-output-logging-testing.md) | Output, Logging & Testing | Accepted |
| [0008](0008-implementation-bootstrap.md) | Implementation Bootstrap | Accepted |
