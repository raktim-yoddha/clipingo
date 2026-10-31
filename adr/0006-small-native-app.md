# ADR 0006 — A small native app on the shared Windows App Runtime

Decided by the product owner, 2026-10-31.

## Context

The installer was 27.6 MB and the installed app 120 MB in 309 files: a trimmed .NET runtime, a private copy of the
Windows App Runtime, 86 language folders and a TLS library. The product owner asked for a very small, efficient app, and
their app Pluto already ships this way.

## Decision

Release builds use Native AOT: one native `Clipingo.exe`, no JIT and no .NET runtime. RPC JSON comes from a source
generator (`CoreJson`), and call parameters are plain `JsonObject`s, because there is no reflection to read anonymous
objects.

The app runs on Windows' shared Windows App Runtime (`WindowsAppSDKSelfContained=false`). `Program.cs` starts it through
the Bootstrapper, and `WindowsAppRuntime` offers Microsoft's installer once if it is missing. The core uses the operating
system's TLS (`native-tls`) instead of bundling one. `build.ps1` removes the SDK parts the app never loads.

## Consequences

A PC without the runtime needs a one-time download of about 110 MB from Microsoft, after the user says yes. The Windows
App SDK packages are pinned to the set `Microsoft.WindowsAppSDK.Runtime` expects. Building a release needs Visual
Studio's C++ tools. Anything crossing into XAML must be a concrete, `partial` type: a list bound as an interface type
crashes only in the published build, so UI tests must run against it. Linux builds now need OpenSSL headers. Crash logs
carry no stack traces and framework messages are resource keys. ONNX Runtime stays linked into `clipd`, about 20 MB, so
that face tracking needs no extra download.
