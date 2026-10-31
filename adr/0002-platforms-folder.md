# ADR 0002 — One folder per operating system

Decided by the product owner, 2026-10-30.

## Context

Other people are to build the macOS and Linux apps, with their own agents, but the only app sat in `app/Clipingo` and
nothing said where another would go or what it may rely on.

## Decision

Each OS has `platforms/<os>/` holding its native app, packaging and UI tests, and a page with what is proven and a list
of tasks of one pull request each. The Windows app, installer and UI tests moved there. Apps are native on each OS and
reach the core only through the `clipd` protocol in `docs/rpc.md`. No cross-platform UI toolkit.

## Consequences

Three UIs to maintain, each thin; the protocol must stay additive. CI builds and tests the core on all three systems, but only
Windows has been proven with real media so far.

## Amendment (2026-10-31): the frontend stays with each platform

Rust holds the logic, the backend and the core: finding, framing, captioning and rendering clips. Frontend work is built
independently on each platform, including the editing session (segments, split, trim, undo), the clock mapping for the
timeline, the live preview, and wizard presets. Something moves to the core only when it is real shared logic, not to
save writing a UI twice. So the three apps are not as thin as first written above.
