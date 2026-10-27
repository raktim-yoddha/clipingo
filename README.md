# Clipingo

Local-first stream clipper: a Rust core plus a native WinUI 3 app. See [PLAN.md](PLAN.md) for the full design.

## Layout

| Path | What |
|---|---|
| `crates/clipingo-core` | Pipeline library: system check, source probe, clip scorer, pacing, virtual camera |
| `crates/clipd` | Daemon: JSON-RPC 2.0 (one JSON object per line) on `\\.\pipe\clipingo` / Unix socket |
| `crates/clip` | CLI: `clip check`, `clip probe <file\|url>`, `clip score <words.json>` |
| `app/Clipingo` | Windows app: WinUI 3, unpackaged Win32 .exe, Desktop Acrylic, MVVM + DI |

## Build and run (Windows)

```bash
cargo test --workspace
cargo build -p clipd
dotnet build app/Clipingo/Clipingo.csproj -p:Platform=x64
```

Run `app/Clipingo/bin/x64/Debug/net10.0-windows10.0.26100.0/win-x64/Clipingo.exe`. The app starts `clipd`
itself (from `CLIPINGO_CLIPD`, next to the app, or `target/debug`).

Needs on `PATH`: `ffmpeg`/`ffprobe`, and for YouTube links `yt-dlp` + `deno`. **Settings → System check** shows what is missing.

### Transcription

- **Hindi / Hinglish**: Sarvam Saaras v3 batch API (cloud: audio is uploaded to Sarvam). Set `SARVAM_API_KEY`.
  Saaras gives phrase timestamps only, so word times are estimated inside each phrase (`word_timing: estimated`).
- **English / offline**: `whisper-cli` (whisper.cpp) on `PATH` plus a ggml model in `%LOCALAPPDATA%\Clipingo\models`
  (`ggml-large-v3-turbo.bin` preferred) or `CLIPINGO_WHISPER_MODEL`.
- `auto` asks whisper which language is spoken mid-stream: English → whisper, anything else → Sarvam.
- Results are cached in `%LOCALAPPDATA%\Clipingo\cache\transcripts` (keyed by file, size, mtime, engine), so a
  stream is never sent to Sarvam twice; pass `refresh` to redo it.

```bash
cargo run -p clip -- find "D:\streams\vod.mp4" hinglish
```

## clipd methods

| Method | Params | Result |
|---|---|---|
| `version` | — | `{ version }` |
| `system.check` | — | tools found, NVIDIA GPUs, H.264 encoders actually probed (NVENC → QSV → AMF → MF → x264) with fix hints, selected encoder |
| `source.probe` | `{ input }` file path or https YouTube link | title, channel, duration, size, fps, views, upload date |
| `transcript.run` | `{ input, options?: {language: auto\|english\|hindi\|hinglish, script: native\|roman, refresh} }` | words with times, engine, language, `word_timing` (`model` / `estimated`) |
| `clips.find` | `{ input, transcribe?, find? }` | source card, transcript summary, ranked candidates (transcript + loudness) |
| `scorer.find` | `{ words: [{w,s,e}], options?: {min_len,max_len,count} }` | ranked clip candidates |
