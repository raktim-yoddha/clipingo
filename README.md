# Clipingo

Local-first stream clipper: a Rust core plus a native WinUI 3 app. See [PLAN.md](PLAN.md) for the full design.

## Layout

| Path | What |
|---|---|
| `crates/clipingo-core` | Pipeline library: system check, source probe, audio, local transcription, model manager, clip scorer, pacing, virtual camera |
| `crates/clipd` | Daemon: JSON-RPC 2.0 (one JSON object per line) on `\\.\pipe\clipingo` / Unix socket |
| `crates/clip` | CLI: `check`, `probe`, `faces`, `score`, `transcribe`, `run`, `render`, `models` |
| `app/Clipingo` | Windows app: WinUI 3, unpackaged Win32 .exe, Desktop Acrylic, MVVM + DI |

## Build and run (Windows)

```bash
cargo test --workspace
cargo build -p clipd
dotnet build app/Clipingo/Clipingo.csproj -p:Platform=x64
```

Run `app/Clipingo/bin/x64/Debug/net10.0-windows10.0.26100.0/win-x64/Clipingo.exe`. The app starts `clipd`
itself (from `CLIPINGO_CLIPD`, next to the app, or `target/debug`).

**Settings → Downloads** installs FFmpeg, yt-dlp, deno, the whisper.cpp runtime and the speech models into
`%LOCALAPPDATA%\Clipingo` on demand (tools already on `PATH` are used as they are). **Settings → System check** shows
what is missing.

### Installer and updates

```bash
powershell -File installer/build.ps1
```

builds `target\installer\Clipingo-Setup-<version>.exe` (~27 MB): a per-user Inno Setup wizard (no admin prompt) with
the trimmed, self-contained app and `clipd`. Nothing heavy is bundled; tools and models download in the app.
Pushing a tag `v<version>` (same as `<Version>` in `Clipingo.csproj`) runs `.github/workflows/release.yml`, which
publishes the installer as a GitHub release. **Settings → Updates** checks the latest release (automatically at
start-up unless turned off), verifies the installer's SHA-256 and installs it silently. The update check needs the
releases to be reachable without signing in (a public repository, or a public releases repository).

### Transcription (local only)

Everything runs on whisper.cpp; no audio leaves the machine.

| Language | Model | Output |
|---|---|---|
| English | whisper large-v3-turbo | English |
| Hinglish | Oriserve Whisper-Hindi2Hinglish-Apex | Roman-script Hinglish |
| Hindi | ARTPARK-IISc whisper-large-v3-vaani-hindi | Devanagari |

Install the runtime and models from **Settings → Speech models**, or:

```bash
cargo run -p clip -- models install all
```

Files go to `%LOCALAPPDATA%\Clipingo` (`bin\whisper`, `models`); downloads resume if interrupted. The Hinglish and
Hindi models are converted once to ggml with whisper.cpp's converter, which needs [uv](https://docs.astral.sh/uv/).
`auto` detects the language mid-stream: English → English model, otherwise Hinglish (then Hindi). Speech is cut
into short chunks with Silero VAD and decoded in one whisper.cpp call; word times come from DTW alignment
(the fine-tunes' own timestamps are unusable). Transcripts are cached in
`%LOCALAPPDATA%\Clipingo\cache\transcripts`; pass `refresh` to redo one.

### Workflow

**Create** is a five-step wizard (BridgeClip's options, minus publishing):

| Step | Options |
|---|---|
| Video | Workflow **Automatic** (find + export) or **Review & edit** (pick clips on Jobs), file or YouTube link, preferred part of the video |
| Format | Vertical 9:16 / Horizontal 16:9; framing **Smart** / **Full frame** / **Classic**; AI vision check for unsure shots (Antigravity CLI); cut dead air; speed 1 / 1.1 / 1.25 / 1.5 / 1.75 / 2× |
| Clips | What to clip (optional), spoken language, lengths 10–30s … 15–30m (any number), clip count or let Clipingo decide |
| Captions | Title at the top, burn-in on/off, 13 caption styles (Pop, Spotlight, Impact, Glow, Boxed, Sweep, Editorial, Hype, Punch, Neon, Headline, Paper, Subtle) |
| Review | Summary with Edit links → **Generate clips** / **Find candidates** (Ctrl+Enter from any step) |

Jobs run in the background (two at a time) on **Jobs**; **Library** lists every saved run. **Editor** (sidebar, or
Edit on a clip in Jobs or Library):

- Clip list with **New clip** (from the playhead) and **Delete**; source / exported-clip preview at 0.5–2×.
- Trim on a timeline: ±1 s, set start / end to the playhead, undo / redo.
- Transcript: click a word to jump there; fix the title and caption text (word timing kept).
- Export look: format, framing, caption style, caption position (bottom / middle / top) and size (60–160 %),
  speed 1–2×, fade in / out, captions, title, dead-air cuts.
- Shortcuts: Space, I / O, ← / →, N, Ctrl+Z / Ctrl+Y, Ctrl+S, Ctrl+E.
- Exported files open in File Explorer; in Premiere Pro, File › Import the clip's `.xml`.

Same from the CLI:

```bash
cargo run -p clip -- run "https://www.youtube.com/watch?v=..." hinglish
cargo run -p clip -- render <run-id> 1 2 3 style=glow title speed=1.25 framing=smart vision
```

Runs live in `%LOCALAPPDATA%\Clipingo\runs\<id>` (`run.json`, `transcript.json`, downloaded `source.mp4`,
`render.json`). Exported clips go to `Videos\Clipingo\<video title> (<id>)\clip_NN_9x16.{mp4,srt,xml}`
(`CLIPINGO_OUTPUT_DIR` overrides). Rendering cuts dead air (pauses over 2 s, fillers), frames 9:16 (faces from
YuNet on ONNX Runtime: Full frame follows the speaker; Smart follows a talking head or stacks screen over webcam when
the face is small; Classic = whole frame over a blur), changes speed (pitch kept), burns captions and the title,
normalizes loudness and encodes with the first working encoder (NVENC → QSV → AMF → MF → x264).

### End-to-end tests

```bash
powershell -File tests/e2e/make-english-stream.ps1
powershell -File tests/e2e/app-e2e.ps1 -Source "%TEMP%\clipingo-e2e\english-stream.mp4" -Language English
```

## clipd methods

| Method | Params | Result |
|---|---|---|
| `version` | — | `{ version }` |
| `system.check` | — | tools, NVIDIA GPUs, probed H.264 encoders (NVENC → QSV → AMF → MF → x264) with fix hints, selected encoder, whisper-cli, speech models |
| `source.probe` | `{ input }` file path or https YouTube link | title, channel, duration, size, fps, views, upload date |
| `models.list` | — | whisper-cli path, models with installed state |
| `models.install` | `{ id: english\|hinglish\|hindi\|runtime }` | `{ path }`; streams `progress` notifications `{request, stage, done, total}` first |
| `transcript.run` | `{ input, options?: {language: auto\|english\|hindi\|hinglish, refresh} }` | words with times, model, language |
| `run.create` | `{ input, options?: {transcribe, find} }` (`find` as in `scorer.find`) file or YouTube link | saved run: source, media, transcript summary, ranked candidates; streams progress |
| `run.get` | `{ id }` | a saved run |
| `run.render` | `{ id, ranks: [1,2], options?: {format: vertical\|horizontal, framing: smart\|full\|classic, vision_check, cut_dead_air, captions, caption_style, caption_position: bottom\|middle\|top, caption_size, fade, title, speed} }` | per clip: `.mp4`, `.srt`, Premiere `.xml`, duration, dead air removed, encoder; streams progress |
| `run.list` | — | saved runs, newest first, with their exported clips |
| `run.open` | `{ id }` | the run, its clips folder, exported clips and last export options |
| `run.words` | `{ id, start, end }` | transcript words in that part of the source |
| `run.edit` | `{ id, rank, start, end, title }` | saves a clip's trim and title; the next export uses them |
| `run.add_clip` | `{ id, start, end, title }` | a clip marked by hand (next free rank) |
| `run.remove_clip` | `{ id, rank }` | removes a clip from the run (exported files stay) |
| `run.retext` | `{ id, start, end, text }` | corrected caption text for that span (word times kept or shared) |
| `captions.presets` | — | the 13 caption styles: id, name, description, font, colours |
| `scorer.find` | `{ words: [{w,s,e}], options?: {min_len,max_len,lengths: [[min,max]],count,within: [start,end],request} }` | ranked clip candidates |
