# ADR 0001 — Agent contract, ADRs and CI

Decided by the product owner, 2026-10-30.

## Context

The repository had no written rules, so every contributor and coding agent would have had to guess them, and nothing
checked the core on macOS or Linux.

## Decision

`AGENTS.md` and `CLAUDE.md` are one identical contract, with pointers for Gemini, Copilot and Cursor. Decisions are
recorded in `adr/`. CI runs format, lint and tests on Windows, macOS and Linux and builds the Windows app. The checks
that keep this honest are a Rust test (`crates/clipingo-core/tests/contract.rs`), so they need no Node toolchain.

## Consequences

Releases stay manual and tag-driven, as written in the contract, rather than using a release script. Branch protection
is left to the product owner.
