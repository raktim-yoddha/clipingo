# ADR 0004 — Clip quality is measured against targets

Decided by the product owner, 2026-10-30.

## Context

Late captions and bad framing were found by watching clips. Nothing stopped them from coming back.

## Decision

`clip eval` scores the whole process: transcript, clip boundaries, framing, captions, audio. `tests/headless/run.py`
runs it on videos with known answers and fails below a baseline. Targets live in `eval.rs`.

## Consequences

The reference video is not in the repository, so the full run needs a contributor's own interview and models, and CI
cannot run it yet; CI runs the unit tests only.
