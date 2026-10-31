# Clipingo for Windows

WinUI 3 on .NET 10: an unpackaged Win32 app with MVVM and dependency injection, compiled ahead of time in release.
The window is opaque gray Mica with acrylic panels (ADR 0005). The app runs on the shared Windows App Runtime (ADR 0006).

| Path | What |
|---|---|
| `Clipingo/` | The app. `Views/` are pages, `ViewModels/` hold state, `Services/CoreClient.cs` is the protocol client, `Models/CoreModels.cs` mirrors the core's JSON |
| `installer/` | Per-user Inno Setup installer, about 10 MB; tools and models download in the app |
| `tests/e2e/` | Drives the real app through UI Automation |

## Set up

Windows 10 19041 or later, Rust stable, .NET 10 SDK, the Windows App Runtime the app asks for, ffmpeg on PATH (or
Settings → Downloads). Release builds also need Visual Studio's C++ tools.

```bash
cargo build -p clipd -p clip
dotnet build platforms/windows/Clipingo/Clipingo.csproj -p:Platform=x64
```

Run `platforms/windows/Clipingo/bin/x64/Debug/net10.0-windows10.0.26100.0/win-x64/Clipingo.exe`. The app starts `clipd`
itself, from `CLIPINGO_CLIPD`, beside the app, or `target/`.

## Test

```bash
powershell -File platforms/windows/tests/e2e/make-english-stream.ps1
powershell -File platforms/windows/tests/e2e/app-e2e.ps1 -Source "%TEMP%\clipingo-e2e\english-stream.mp4" -Language English
powershell -File platforms/windows/tests/e2e/auto-e2e.ps1 -Source "%TEMP%\clipingo-e2e\english-stream.mp4"
```

A debug build hides Native AOT failures. Before finishing, run `installer/build.ps1` and pass
`-Exe target\installer\app\Clipingo.exe` to both suites.

These need a signed-in desktop: UI Automation drives the real window. `app-e2e` covers Review & edit, Jobs and the
editor, then runs the evals; `auto-e2e` covers Automatic, 16:9 and the Library. A control's `AutomationProperties.Name`
is what these tests find, so renaming one means updating them.

## Release

```bash
powershell -File platforms/windows/installer/build.ps1
```

Pushing a tag `v<version>` equal to `<Version>` in `Clipingo.csproj` publishes the installer as a GitHub release.
