# Contributing

Clipingo is a Rust core with one native app per OS. You can help without touching the other platforms.

## Where to start

| You want to | Go to |
|---|---|
| Build the macOS app | [platforms/macos](platforms/macos/README.md) |
| Build the Linux app | [platforms/linux](platforms/linux/README.md) |
| Improve the Windows app | [platforms/windows](platforms/windows/README.md) |
| Make clips better (tracking, captions, clip finding) | [docs/architecture.md](docs/architecture.md), [docs/evals.md](docs/evals.md) |
| Understand the protocol | [docs/rpc.md](docs/rpc.md) |

Each platform page has a list of open tasks, each sized for one pull request.

## Working with a coding agent

Agents are welcome, and the repository is set up for them. `AGENTS.md` (the same file as `CLAUDE.md`) holds every rule;
`GEMINI.md`, `.github/copilot-instructions.md` and `.cursor/rules/contract.mdc` point to it. A good first prompt:

> Read AGENTS.md, then platforms/linux/README.md, and take the first open task.

You remain responsible for what you submit: run it, read the diff, and say in the pull request what you tested.

## Before a pull request

```bash
cargo fmt --all
cargo clippy --workspace
cargo test --workspace
```

Changed how clips are made? Also run the evals and paste the result:

```bash
python tests/headless/run.py --faces <an interview>
```

`docs/evals.md` explains the numbers. Your OS's page says how to run its UI tests.

The pull request template asks what changed, how you tested it and on which OS. If you could not test on an OS you
touched, say so; someone else can.

## Commits

[Conventional Commits](https://www.conventionalcommits.org): `feat(core): …`, `fix(macos): …`, `docs: …`. Use the
scope `core`, `windows`, `macos`, `linux`, `evals` or `ci`.
