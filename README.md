# Clipingo

Local-first clipper for streams, podcasts and interviews: it finds the best moments in a long video and turns them into
captioned vertical or horizontal clips, on your machine. A Rust core with a native app per OS.

[Contributing](CONTRIBUTING.md) · [Architecture](docs/architecture.md) · [Protocol](docs/rpc.md) ·
[Evals](docs/evals.md) · [Platforms](platforms/README.md)

## Layout

| Path | What |
|---|---|
| `crates/clipingo-core` | The pipeline: everything that makes a clip |
| `crates/clipd` | Daemon every app talks to: JSON-RPC over a named pipe or Unix socket |
| `crates/clip` | Command line: `check`, `probe`, `faces`, `score`, `transcribe`, `run`, `render`, `framing`, `eval`, `models` |
| `platforms/windows` | The Windows app (WinUI 3), installer and UI tests |
| `platforms/macos`, `platforms/linux` | Apps to build: start here if you want one |
| `tests/headless` | End-to-end run of the core with no UI, on any OS |
| `docs`, `adr` | How it works, and why |

## Build and run

```bash
cargo test --workspace
cargo build -p clipd -p clip
```

Then follow your OS's page: [Windows](platforms/windows/README.md), [macOS](platforms/macos/README.md),
[Linux](platforms/linux/README.md).

**Settings → Downloads** installs FFmpeg, yt-dlp, deno, the whisper.cpp runtime and the speech models into
`%LOCALAPPDATA%\Clipingo` on demand (tools already on `PATH` are used as they are). **Settings → System check** shows
what is missing.

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

Jobs run in the background (two at a time) on **Jobs** (Cancel stops the work in the core too); **Library** lists
every saved run with a thumbnail (Edit, Open folder, Remove: exported clips stay). **Editor** (sidebar, or Edit on a
clip in Jobs or Library) is a small non-linear editor:

- **Clips** list with thumbnails, filtered All / 9:16 / 16:9; each exported format has its own ▶ button that plays the
  file in the app. New clip (N, from marks or 30 s from the playhead), Delete clip.
- **Player** tabs: *Edit* plays the timeline as it will be exported (parts back to back, rough caption and title
  preview). In 9:16 it shows the source with the crop boxes that follow the people, and beside it the vertical frame
  they make, shot by shot, with the captions; 16:9 is one screen. *Source video* shows the whole video to mark a part (In / Out, I / O) and add it to the timeline or make a
  new clip of it, *9:16 export* / *16:9 export* play the exported files. Play / pause, ±1 s (Shift ±5 s), loop,
  0.5–2× preview speed.
- **Timeline**: filmstrip of the clip's parts and a caption track (click a word to jump there). Split at the playhead
  (S), drag a part's edges to trim, drag a part to reorder, trim to playhead (Q / W), Delete, Add clip (stitch another
  clip of the video in), zoom (Ctrl + wheel, + / −, Fit). Undo / redo for everything; changes save by themselves.
- **Inspector**: *Layout* (9:16 or 16:9; this shot's layouts, only the ones it can show; swap top and bottom; smart
  zoom; framing rules), *Captions* (on / off, 13 styles, auto / bottom / middle / top, size 60–160 %, per-part text
  fixes with word timing kept), *Clip* (title, speed 1–2×, pause cuts, fades).
- **Export 9:16 / 16:9** (Ctrl+E) renders the parts in order: MP4, SRT and a Premiere `.xml`.
- Exported files open in File Explorer (folder button); in Premiere Pro, File › Import the clip's `.xml`.

Same from the CLI:

```bash
cargo run -p clip -- run "https://www.youtube.com/watch?v=..." hinglish
cargo run -p clip -- render <run-id> 1 2 3 style=glow title speed=1.25 framing=smart vision
```

Runs live in `%LOCALAPPDATA%\Clipingo\runs\<id>` (`run.json`, `transcript.json`, downloaded `source.mp4`,
`render.json`). Exported clips go to `Videos\Clipingo\<video title> (<id>)\clip_NN_9x16.{mp4,srt,xml}`
(`CLIPINGO_OUTPUT_DIR` overrides). Rendering cuts dead air (pauses over 2 s, fillers), frames 9:16 shot by shot,
changes speed (pitch kept), burns captions and the title, normalizes loudness and encodes with the first working
encoder (NVENC → QSV → AMF → MF → x264).

### Vertical rules

Each camera shot gets a layout, from the faces, the motion on screen and who is talking:

| The shot shows | Layout |
|---|---|
| One person | **Follow**: a window on them, closer when the face is small |
| Two people apart | **Two people**: one above the other, captions on the seam |
| Two people close together | **Follow**: one window holding both |
| Three or more | **Everyone**: a wide band over a blurred fill |
| A webcam that stays in a corner (stream, gameplay) | **Face + content**: the face above the moving part of the screen; **Swap** turns it over |
| Nobody | **Whole frame** over a blurred fill |

The camera starts on the subject at every cut. **Smart zoom** pushes in on strong lines. In the editor each shot can be
set by hand; Face + content is offered on any shot with a face. `clip framing <run-id> <rank>` prints a clip's shots.

### Evals

`clip eval <run-id> [captions.vtt]` scores a run, from transcript to export, and `tests/headless/run.py` runs the whole
process on labelled videos. See [docs/evals.md](docs/evals.md).
