# Engineering Contract (AGENTS.md = CLAUDE.md)

This file is mandatory for every coding agent working in this repository.

**`AGENTS.md` and `CLAUDE.md` are one document in two files** (Claude reads `CLAUDE.md`, other agents
read `AGENTS.md`). Whenever you change either one, make the identical change to the other in the
same commit; a test fails when they differ. Never let them drift.

**This contract is the hub.** Every rule and process an agent must follow lives here. Other docs
(design, architecture, product spec, ADRs, READMEs) explain and record; they never add a rule an
agent must follow. A new rule goes here first, in both files, with a check when it can be checked.

## Source of Truth

Read in this order:

1. `AGENTS.md` (this file)
2. `README.md` (what the product does)
3. `docs/architecture.md`, `docs/rpc.md`, `docs/evals.md`, `docs/porting-a-ui.md`
4. `platforms/<os>/README.md` for the OS you work on
5. `PLAN.md` (the original design; where it disagrees with the above, it is the older one)
- relevant `adr/*.md` (how and when to write one: "ADRs" below)

If implementation and documentation disagree, do not silently choose one. Find out which is stale
and fix that one (and record the decision as an ADR when the rules below require one).

## Project Rules

1. **The core owns behavior; apps own presentation.** How clips are found, framed, captioned and rendered lives in
   `crates/clipingo-core`. A native app never runs ffmpeg or whisper, reads a run's files or decides a layout: it
   calls `clipd`. Layout rules are never duplicated in a UI.
2. **`docs/rpc.md` is the shared contract.** Change a method there and in `crates/clipd` in the same commit (a test
   fails on an undocumented method). Changes are additive: new fields are optional with a default, because several
   apps depend on them. Removing or renaming anything needs an ADR.
3. **Stay on your platform.** Work on one OS stays under `platforms/<os>/`. Touch the core only when the task needs
   it, and then keep every OS building: OS-specific code goes behind `cfg`, and every Windows default (fonts,
   encoders, paths) has a path on macOS and Linux.
4. **Clip quality is measured.** A change to how clips are made runs `tests/headless/run.py`, and its score may not
   fall below `tests/headless/baseline.json`. Lowering a target in `eval.rs` or the baseline needs an ADR. A bug found
   by hand gets a check in `eval.rs`, a test, or both, so it cannot come back.
5. **Local only.** Audio and video never leave the machine. No hosted speech or vision API unless the user opts in,
   and an ADR says so.
6. **Tests never touch a real Library.** They set `CLIPINGO_RUNS_DIR`, `CLIPINGO_OUTPUT_DIR` and `CLIPINGO_ENDPOINT`.
7. **Plain words in the UI.** "Two people", not "Stack". Offer only what a shot can really show.
8. **No reference media in the repository.** Test videos are built from what the contributor passes in.

## Keeping the Code Lazy

Prefer the standard library, the platform and what the repo already has over new code or dependencies. Mark a
deliberate shortcut with a `ponytail:` comment that names its limit and how to lift it.

## Core Rules

1. **Reuse before creating.** Search the repository before adding a component, hook, service,
   utility or adapter; extend what exists. Never duplicate functionality.
2. **Keep layers apart.** Views show state and raise actions; they don't persist, spawn processes or
   touch the filesystem. View models talk to the core, nothing else.
3. **Narrow, typed boundaries** around privileged work (filesystem, processes, secrets, network).
4. **Native look.** Use the platform's own controls and theme resources before custom ones.
5. **Accessibility.** Keyboard access, labels on icon buttons, visible focus, meaningful semantics.
6. **No silent product decisions.** If a requirement is ambiguous and affects architecture or UX:
   check the docs, the code and the ADRs; if still unclear, ask the user and record the answer as an ADR.
7. **Keep files focused.** A file that mixes screen layout, domain logic, persistence and I/O is a
   design failure; split by responsibility.
8. **Preserve existing work.** Understand why code exists, find its consumers and tests before
   changing it; prefer incremental refactors over rewrites.
9. **Privacy.** No real person's data (names, emails, usernames, home paths, accounts, keys) in
   source, tests, fixtures or prompts. Use neutral placeholders (`Alex`, `/home/me`).
10. **Tests** for every non-trivial logic change. Typecheck, lint and tests pass before a commit.
11. **Conventional Commits.** `feat:`, `fix:`, `docs:`, `test:`, `ci:`, `build:`, `chore:`,
    `refactor:`, `perf:`, `style:`; a breaking change adds `!` or a `BREAKING CHANGE:` body.
    Releases are computed from these, so the type must be honest.
12. **Never bypass checks.** No `--no-verify`, no skipped hooks, no edited generated files, no
    force-push to `main`.

## Releases & Semantic Versioning

This project follows [Semantic Versioning 2.0.0](https://semver.org): `MAJOR.MINOR.PATCH`. Before 1.0 a breaking
change is a MINOR step.

**The command is `release`.** When the user says "release":

1. Read the commits since the last `v*` tag and pick the version: breaking → MINOR (before 1.0), `feat` → MINOR,
   other user-facing commits → PATCH; docs, tests, CI and chores alone → nothing to release (tell the user and stop).
2. Write the notes at the top of `CHANGELOG.md`: a title of up to ten words, then two to six `- ` highlights in the
   product's words, present tense. A fix is written as the problem that is gone; a breaking change comes first.
3. Set `<Version>` in `platforms/windows/Clipingo/Clipingo.csproj` and `version` in `Cargo.toml` to it.
4. On `main`, commit `chore(release): vX.Y.Z`, tag `vX.Y.Z` and push the tag.
5. The tag starts `.github/workflows/release.yml`, which refuses a tag that differs from the app's version and
   publishes the Windows installer. Watch the run; report the release URL, or the failing step verbatim.

Never reuse a tag and never skip a number.

## ADRs (Architecture Decision Records)

`adr/` is the project's memory of *why*. Every agent writes ADRs; this section is the only place
the process is described.

**An ADR is required when a change:**

- carries a product decision the user made (what the product does, or never does)
- adds or changes behavior users see (a feature, a removed option, a new default)
- adds or widens a permission, an API or IPC surface, or a server endpoint
- changes stored data, a file format, a protocol or a shared contract
- adds a dependency, a tool, a build or release step, or a CI workflow
- changes or adds a rule in this contract

**Not needed for:** bug fixes that restore intended behavior, refactors with no behavior change,
styling within the design tokens, tests, and typo-level docs.

**How:**

1. Read the related ADRs first. If one already covers the subject, add an
   `## Amendment (YYYY-MM-DD): …` section to it instead of writing a new one.
2. Otherwise create `adr/NNNN-short-kebab-name.md` with the next free number (highest + 1),
   from `adr/TEMPLATE.md`: title, "Decided by …, YYYY-MM-DD.", Context, Decision, Consequences.
3. Never rewrite the decision of an existing ADR: amend it, or write a new ADR that says
   "Supersedes ADR XXXX".
4. Commit the ADR with the change it explains, and mention its number where the rule is enforced
   in code (`// ADR 0007`).

`crates/clipingo-core/tests/contract.rs` fails on a bad file name, a duplicate or skipped number, a
title that doesn't match, or a missing section.

## Definition of Done

```text
[ ] Behavior matches the product spec; UI matches the design docs
[ ] Reused existing code; no duplicate logic
[ ] `cargo fmt`, `cargo clippy` and `cargo test --workspace` pass; UI tests pass on the OS you changed, against the published build on Windows
[ ] `tests/headless/run.py` is at or above the baseline, if clip making changed
[ ] Docs updated; ADR written or amended when required
[ ] AGENTS.md and CLAUDE.md are still identical
```
