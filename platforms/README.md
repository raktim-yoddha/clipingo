# Platforms

One folder per OS. Each holds its native app, its packaging, its UI tests and a README saying what is proven and what
to pick up next. All of them drive the same core through [the protocol](../docs/rpc.md).

| | Windows | macOS | Linux |
|---|---|---|---|
| Native app | WinUI 3, working | not started | not started |
| Installer, updates | Inno Setup, GitHub releases | not started | not started |
| Core builds and passes tests | proven | set up in CI, first run pending | set up in CI, first run pending |
| Headless run with real media | proven | needs a contributor | needs a contributor |

Start with [Building a UI for another OS](../docs/porting-a-ui.md) and your OS's page:
[Windows](windows/README.md), [macOS](macos/README.md), [Linux](linux/README.md). A change only to `platforms/<os>/`
never needs the other platforms to be touched.
