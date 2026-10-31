# Architecture

Clipingo is one Rust core with a native app per operating system. The core does everything that makes a clip; an app
only shows it. That is what lets each OS have a truly native UI without rewriting the product.

```text
 platforms/windows   WinUI 3 (C#)          ┐
 platforms/macos     native app (to build) ├─ JSON-RPC, one object per line ─► clipd ─► clipingo-core
 platforms/linux     native app (to build) ┘   named pipe / Unix socket         ▲
 clip (CLI), tests/headless ────────────────────────────────────────────────────┘
```

| Path | What |
|---|---|
| `crates/clipingo-core` | The pipeline, as a library |
| `crates/clipd` | The daemon: the protocol in [rpc.md](rpc.md) |
| `crates/clip` | The command line: same core, prints JSON |
| `platforms/<os>` | A native app, its packaging and its UI tests |
| `tests/headless` | End-to-end run of the core with no UI, on any OS |

## Core modules

| Module | Does |
|---|---|
| `source` | Probes files and YouTube links, downloads with yt-dlp |
| `audio` | 16 kHz audio, loudness envelope, silences |
| `transcribe`, `models` | whisper.cpp with one model per language, voice activity detection, word times, model downloads |
| `scorer`, `emphasis` | Finds the moments worth clipping; how strongly a word lands |
| `pipeline` | Runs: what is saved in `runs/<id>` and every edit to it |
| `framing`, `camera`, `zoom` | The vertical rules, shot by shot: faces, tracking, layouts, the camera path, smart zoom |
| `captions` | ASS captions (burned in by libass) and SRT |
| `pacing` | Dead-air and filler cuts |
| `render` | ffmpeg graph, encoders, the Premiere timeline |
| `eval` | Measures all of the above: see [evals.md](evals.md) |

## The clip's two timelines

The editor plays a clip's parts back to back, so `run.framing` plans on that clock. An export cuts pauses first, so
`render` plans again on the cut timeline with the same rules. Choices you make (a shot's layout) are stored in source
seconds and mapped onto whichever timeline is planning.

## Portability today

The core is written for Windows, macOS and Linux; OS differences stay behind `cfg` in a few places: data folders
(`lib.rs`), the endpoint, encoders (`system.rs`), fonts (`captions.rs`, `render.rs`), tool downloads (`tools.rs`,
`models.rs`). [The platform pages](../platforms/README.md) say what is proven on each OS.
