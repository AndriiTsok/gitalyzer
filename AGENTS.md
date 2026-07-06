# AGENTS.md

Project instructions for AI coding agents. This file is loaded automatically at the start
of every session. Keep it current as the project evolves.

## Project

**Gitalyzer** — an AI-powered terminal tool that analyzes Git commit message quality
and helps developers write better commit messages.

Status: **v1 feature-complete.** All six implementation slices of RFC 0008 have
landed: both modes (analyze incl. remote `--url`/`--branch`, interactive write),
providers, configuration, progress/decoration, and the README. Requirements live in
RFCs 0001–0008 under `docs/rfcs/`; the PRD is `docs/product.md`. Changes must keep
the full gate green: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test`.

## Language & Toolchain

- **Language:** Rust (edition 2024, MSRV 1.85, toolchain 1.96+).
- Core libraries are chosen deliberately per feature and recorded in the relevant RFC
  before adoption — the locked set lives in `docs/rfcs/0008-implementation-bootstrap.md`.
  Do not add dependencies ad hoc — propose them in an RFC first.

## How We Work

We collaborate side by side. The flow is:

1. **Requirements** — capture what the system must do as RFCs in `docs/rfcs/`.
2. **Design** — record architecture and technical decisions as RFCs before coding.
3. **Implement** — build incrementally, one reviewable slice at a time.
4. **Document** — keep `docs/` in sync with the code as it lands.

Do not jump ahead to implementation while requirements for that slice are still open.

## Documentation

- `docs/` is the home for all project documentation.
- `docs/product.md` is the PRD: the *what* and *why* at product level, without deep
  technical detail.
- `docs/rfcs/` holds numbered RFCs (technical requirements + design). See
  `docs/rfcs/README.md` for the process and `docs/rfcs/0000-template.md` for the template.
- Every significant decision should be traceable to an RFC.

## Commit Conventions

- Use **Conventional Commits**: `type(scope): summary`.
  - Types: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `build`, `ci`, `perf`, `style`.
  - Example: `docs(rfc): add requirements RFC for repository analysis`
- One logical change per commit. Keep the summary imperative and under ~72 chars.
- Commit or push only when asked. Never commit secrets.

## Code Standards

- Write clean, idiomatic, well-documented Rust.
- Public items carry `///` doc comments explaining intent, not restating the signature.
- Prefer clarity over cleverness; match the style of surrounding code.
- `cargo fmt` and `cargo clippy` must pass before a change is considered done.
- Add tests alongside behavior; a change to product code has a runtime surface to verify.

## Conventions Recap

- Rust, Conventional Commits, RFC-driven requirements and design.
- Documentation lives in `docs/`; requirements precede implementation.
