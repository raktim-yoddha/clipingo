# Clippingo — Build Plan

Local-first stream clipper. Input: YouTube stream VOD (URL) or local file. Output: short clips and long-form
cuts, rendered to MP4 **and** pushed to Adobe Premiere Pro as editable sequences with native captions.
No uploading or publishing.

Scope now: **Rust core + Windows (WinUI 3) app**. Mac/Linux UIs later on the same core contract.
Feature set = everything in **BridgeClip** (minus publishing) + everything in **Clippo** + stream-specific additions.

---

## 1. Architecture

```
WinUI 3 app (C#) ──JSON-RPC over named pipe──┐
clip CLI ─────────────────────────────────────┤
Premiere panel (UXP / CEP) ──WebSocket────────┤
                                              ▼
                                   clipd  (Rust daemon)
   ingest ─ signals ─ candidates ─ judge ─ edit plan ─ render / Premiere export
     │         │                      │
  yt-dlp    ort (ONNX) + sherpa-onnx  llama-server (local LLM / VLM)
  ffmpeg    whisper.cpp, libDF        agy / claude / codex CLIs
```

- **Rust workspace**: `core` (pipeline lib), `clipd` (daemon), `clip` (CLI), `ffi` (C ABI).
- **Mac/Linux contract**: JSON-RPC over named pipe / Unix socket (OpenRPC spec) for any UI; plus UniFFI for
  in-process Swift/Kotlin. UniFFI officially supports Kotlin, Swift, Python (Ruby partly); C# is third-party only,
  so the Windows app uses JSON-RPC, not UniFFI.
- Separate daemon process: a crashing model or FFmpeg never takes the UI down. BridgeClip uses the same split
  (Electron main → Python worker over JSON lines, one job at a time); we allow 2 parallel jobs like its Jobs page.
- **No Python at runtime**: every local model is ONNX, GGUF or a native lib; Hindi/Hinglish ASR calls the Sarvam API (§6). (BridgeClip needs Python + OpenCV; Clippo
  needs a browser.)

### Hardware (any GPU; Windows + macOS)
Same idea as Clippo's runtime chain (`webgpu` fp16 encoder / q4 decoder → `wasm` q8 fallback), native backends:

| Workload | Library | NVIDIA | AMD / Intel (Win) | Apple silicon | Notes |
|---|---|---|---|---|---|
| ASR | whisper.cpp via `whisper-rs` | `cuda` | `vulkan` | `metal` + `coreml` | |
| ONNX models | `ort` 2.0 | TensorRT → CUDA | DirectML | CoreML | EP list, silent CPU fallback, `error_on_failure()`, `load-dynamic` |
| Speech toolkit | sherpa-onnx (`sherpa-rs`) | via ORT provider | via ORT | via ORT | |
| Denoise | DeepFilterNet `libDF` (tract, pure Rust) | CPU | CPU | CPU | |
| LLM / VLM | `llama-server` | CUDA | Vulkan | Metal | multimodal via `--mmproj` |
| Encode | ffmpeg | `h264_nvenc` | AMF / QSV | `h264_videotoolbox -allow_sw 1` | fallback `libx264` / `libopenh264` |

Startup probe picks backends; **System check** page shows the result. Stages run one after another and
unload models, so small GPUs work.

---

## 2. Pipeline (verified algorithms)

### 2.1 Ingest
- **Local file** or **YouTube link** with video card (title, channel, thumbnail, duration, views, upload date).
- **Prefer a source range** (start/end).
- Downloader: `yt-dlp` binary managed by the `yt-dlp` Rust crate (installs/updates yt-dlp + ffmpeg, progress
  callbacks, byte-range partial download of a time range, falls back to full download).
  `deno` shipped next to yt-dlp; needed for YouTube JS challenges.
- Formats (BridgeClip's, proven against YouTube):
  - Sort `hdr:SDR, res:2160, fps`; select `bv*[vcodec!^=av01]+ba/b[vcodec!^=av01]`,
    fallback `bv*[vcodec^=avc1]+ba[acodec^=mp4a]/b[vcodec!^=av01]`.
  - 403 / dropped connection → re-extract and retry 3× (2 s, 5 s); other 4xx fail at once.
  - Guards: ≤ 20 GB source, 4 h download deadline, keep ≥ 1 GB free.
- **Two-pass download**, using verified flags:
  1. `-J` metadata.
  2. Analysis set: audio only (`-f ba`), 360p proxy, **live chat** (yt-dlp treats live chat as a subtitle track:
     `--write-subs --sub-langs live_chat --skip-download`), `-N 8` concurrent fragments.
  3. Full quality only for chosen ranges (`--download-sections "*a-b"` or the crate's byte-range path);
     full file when long-form needs most of the stream. This is BridgeClip's **Replace source video** done automatically.
- `--cookies-from-browser` for unlisted/private own streams; daily yt-dlp update.

### 2.2 Layout profile + smart framing fallback
- Webcam + screen layout drawn once and saved (named profiles for different OBS scenes).
- Fallback for non-matching footage = BridgeClip's analyzer:
  - Analysis at **4 fps, 640 px wide**, keyframes 1 fps / 768 px; one decode per window, detection on up to
    4 threads (their 65-min → seconds framing fix).
  - Shot cuts: histogram Bhattacharyya distance **> 0.42**, shots shorter than **1.2 s** merged.
    Frame-accurate camera scan: ffmpeg `scale=320:-2,settb=1/1000000,select='gte(scene,0)'`, markers at score ≥ 0.025.
  - Layout classes: `talking_head`, `two_shot`, `screen_cam`, `screen`. Webcam overlay = face height ≤ 0.17 of frame
    and centre in a corner (x < 0.3 or > 0.7, y < 0.4 or > 0.6). Two-shot = two faces of similar height (≥ 0.5 ratio)
    and ≥ 0.22 apart. Webcam box refined to image edges (needs 2 supported edges); padded insets detected on a 160 px copy.
  - Layout changes confirmed by 3 samples over 500 ms, backdated to the first one.

### 2.3 Faces and the virtual camera
- Detector: **YuNet `face_detection_yunet_2023mar.onnx`** (BridgeClip ships it; OpenCV Zoo: ~0.7 ms at 160×120 on
  desktop CPU). Run through `ort`, not OpenCV DNN (BridgeClip pins OpenCV < 5: v5 DNN runs YuNet ~5× slower).
- Thresholds: detection ≥ 0.72; a face ≥ 0.90 drops competing faces < 0.85; min face height 0.035;
  track must be present ≥ 35% of the shot; gaps ≤ 1 s bridged.
- Small faces: Clippo's tiling: scan 256 px; if the long side is > 1.25× the square, also scan overlapping square tiles.
- Camera path (BridgeClip's, export-friendly): resample to grid → median filter (k = 2) → **dead zone
  0.12 × crop width**, pan speed capped at **0.9 crop widths/s**, eases halfway → vertical eases at 0.2 →
  simplified to **≤ 40 keyframes** within 0.2% error. Rendered as an ffmpeg piecewise `crop` expression;
  the same keyframes become Premiere Motion keyframes.
  Alternative tight mode for steady webcam faces: Clippo's dead zone 0.02 + zero-phase EMA (τ 0.4 s).
- Split layout geometry: screen+cam top panel 0.6 of height (max 0.7), two-shot 0.5, upscale cap 2.6×,
  person crop = 3.2 × face size with face at 40% height.

### 2.4 Signals for hype
| Signal | Method |
|---|---|
| Transcript HI / Hinglish | Sarvam Saaras v3 batch API (`codemix` / `translit` / `verbatim`), chunk timestamps + local forced alignment for word times (§6) |
| Transcript EN / offline fallback | whisper.cpp; word times via `-ml 1` (marked experimental) or `--dtw` token timestamps; `--prompt` + `--carry-initial-prompt` for custom vocabulary; `-sns` suppress non-speech |
| Chunking | 300 s chunks, 1 s overlap, overlap words assigned by midpoint |
| Language ID | detect on loudest 30 s |
| Hallucination filter | drop "thank you for watching / subscribe" style junk |
| VAD | Silero v6.2.0 built into whisper.cpp (`--vad -vm`), or sherpa-onnx Silero/Ten VAD |
| Reactions (laugh, cheer, scream, explosion) | sherpa-onnx **audio tagging** (CED or Zipformer AudioSet models) |
| Loudness / energy | per-sentence RMS vs global mean/std, sigmoid |
| Strong words | power words, numbers, ₹/$/%, `!`, ALL CAPS, ≥ 8 letters, louder than usual (EN + HI + Hinglish lists) |
| Chat hype | msgs/sec vs rolling median, emotes, keywords, shifted back 5–10 s |
| Screen activity | motion map inside the screen box |
| Screen text | PP-OCRv5 ONNX (RapidOCR model set) via `ort` |

### 2.5 Candidates (offline scorer, Clippo's, + stream signals)
Sentence-aligned windows, weights verbatim:
`0.30 hook + 0.12 emotion + 0.10 energy + 0.05 dynamics + 0.08 pace + 0.15 completeness + 0.08 topic focus
+ 0.07 max(story, insight) + 0.05 debate + 0.03 laughter − 0.06 × long gaps − start/end penalties`.
- Hook: question +0.34, story +0.22, insight +0.16, hook words +0.12 each (max 0.3), "you" +0.12, numbers +0.08,
  ≤ 12 words +0.10; fillers, dangling references ("that…") and continuation starts ("and so…") penalised.
- Lexicons in English, Hindi (Devanagari) and Hinglish (hook, question, emotion, story, insight, debate, filler…).
- Topic focus = TF-IDF of top 5 terms. Pace target = 1.12 × median words/sec.
- **Let AI decide** count: at most ⌈duration/50 s⌉+1 (max 20), keep only scores ≥ max(0.36, 0.72 × best),
  overlap ≤ 30%, starts ≥ 3 s apart, topic Jaccard ≤ 0.45.
- Score shown 35–99: `62 + 37 × (raw − p50)/(max − p50)`. Reasons ("Question hook", "Hot take", "Funny"…),
  title from the best opening line, description, keywords.
- adds chat-spike and reaction-tag terms to `raw`; weights tuned on your streams.

### 2.6 Planner and judge (local LLM)
- **Planner** (BridgeClip's prompts and schema):
  - Moment schema: `topic`, `topic_start_segment`, `topic_end_segment`, `setup_segment`, `payoff_segment`,
    `requires_visual_context`.
  - Rubric `hook, standalone, arc, quotability, ending` (0–10), virality = mean / 10.
  - Clips overlap ≤ 5 s; free-text **What to clip** ≤ 1000 chars.
  - Long-form: classify the source first (stream, tutorial…), one coherent topic per episode, strong first 30 s,
    internal **skips ≥ 8 s** removing ≤ **40%** of the episode, chapters ≥ 20 s apart.
- **Editorial judge** (BridgeClip's "Jev" rules): per-check thresholds — source faithfulness 0.65;
  clarity, title support 0.70; opening context, complete ending, logical flow 0.75; not-sponsored 0.80;
  sufficient evidence 0.50. Up to 2 repairs (60 s, then 180 s context), one rediscovery pass, duplicate shortlist.
- **How it runs locally**: `llama-server` `/completion` with `n_probs` returns top-token probabilities →
  P(yes) for each yes/no check; `json_schema` is compiled to a GBNF grammar for strict JSON output.
- **Visual checks**: Antigravity CLI `agy -p --output-format json --json-schema`. Headless accepts text
  blocks only, so contact sheets are written to the job workspace (`--add-dir`) and named in the prompt
  (M0 test: image reading + reported empty-stdout bug on Windows). Local fallback: `llama-server` with a
  vision model via `--mmproj` (image, audio and video input supported).
- **Source research** (optional): agy web search before planning idea, agy as the tool.
- **Presets** (replace BridgeClip's Quality / Economy / Advanced): Fast, Balanced (default), Best (adds agy vision
  + research), Advanced (any Hugging Face GGUF/ONNX per role, any CLI backend).

### 2.7 Cleanup (pacing)
BridgeClip's tight-pacing rules:
- Longest pause kept as-is: **talking head 700 ms, screen + cam 2000 ms, pure screen never cut**
  (silence on a screen recording is usually something happening — matters for vibe coding).
- Keep 260 ms of each removed pause (split across the cut), lead-in 120 ms, tail 380 ms, ignore cuts < 200 ms,
  drop pieces < 400 ms, snap to frames. Long-form: cut pauses ≥ 1300 ms, keep 450 ms.
- Reaction gaps (after laughter/applause) are protected, never cut.
- Fillers: BridgeClip's set (um, uh, erm, hmm, mhm…) + Clippo's (you know, I mean, kind of, matlab, basically…).
- Clippo's voice-band detector for pause edges: 120–1000 Hz band, presets **Natural** (gap ≥ 0.5 s, keep 0.15 s) and
  **Tight** (≥ 0.3 s, keep 0.1 s), breath/click rejection.
-: restart/bad-take removal (judge-checked), AFK/BRB removal or speed-ramp, secret redaction (OCR → blur),
  optional profanity mute, optional denoise (DeepFilterNet3, 48 kHz, 480-sample hop).

### 2.8 Render
- Every edit reads one **edit plan** JSON per clip; preview, MP4 and Premiere export all come from it.
- Formats 9:16, 16:9 + 1:1, 4:5, 4:3, source, custom. Framing Smart / Full frame / Classic (blurred fit:
  `gblur=sigma=10`, darkened), Split / Group.
- Audio: two-pass `loudnorm` **I = −14, TP = −1.5, LRA = 11** (`print_format=json` to measure, then `measured_*`)
 ; fades 12 ms at every cut, 30 ms start, 80 ms end; 48 kHz stereo.
- Speed 1–2×: `setpts=(PTS-STARTPTS)/s` + `atempo=s` with `apad/atrim` to the exact sample count;
  atempo valid 0.5–100, above 2 it skips samples.
- Bitrates (landscape) 12 / 20 / 45 Mbps for 1080 / 1440 / 2160.

### 2.9 Captions
- ASS generator (BridgeClip's design): each word group drawn as 5 aligned layers — plate, shadow, glow,
  active-word pill, face — so every effect is a real blurred bitmap; pop-in tags
  `\fscx82\fscy82\t(0,90,\fscx106\fscy106)\t(90,170,\fscx100\fscy100)`; captions linger ≤ 700 ms.
- Burn-in through libass `subtitles`/`ass` filter with `fontsdir` for any TTF/OTF.
  **Devanagari needs `shaping=complex` and a libass built with HarfBuzz** — the ffmpeg build must have it.
- SRT rules (for Premiere captions): ≤ 42 chars/line, cue ≤ 6 s, break at pauses ≥ 700 ms, linger 400 ms.
- Styles: BridgeClip's 13 (Pop, Spotlight, Impact, Glow, Boxed, Sweep, Editorial, Hype, Punch, Neon, Headline, Paper,
  Subtle) + Clippo's 29 (Hormozi, Karaoke, Typewriter, Glitch, 3D, Cinematic, Desi bold, Emoji pop, Highlighter,
  Bubbles, Word box, Subtitle bar, One word…), deduped. Bundled fonts: Anton, Archivo Black, Montserrat,
  Poppins, Instrument Serif, Plus Jakarta Sans, Baloo 2, Bangers, DM Serif Display, Space Grotesk (all OFL).
- Hinglish: Devanagari → Roman transliteration with schwa deletion; caption translation via local LLM.

### 2.10 Visual extras
- Auto zoom: levels subtle 1.075×, medium 1.14×, strong 1.21× (strength scales 0.7 / 1 / 1.2); strong ≈ once per 20 s;
  never right after or across a camera cut; slow push-in after 12 s with no zoom or cut; add (Z) / remove by hand.
- Hook title, title at the top, watermark, progress bar, sound effects, background music (beat-aligned
  start, loops, ducks under voice, built-in generated tracks), stock B-roll (Pexels/Pixabay key, optional).

---

## 3. Push to Premiere Pro

**Versions**: Premiere Pro 2025 and 2026 (25.x, 26.x).

| Need | UXP (25.6+) | ExtendScript / CEP |
|---|---|---|
| Import files / XML | `project.importFiles(paths, suppressUI, bin)` | `app.project.importFiles(...)` |
| Import sequences | `importSequences(projectPath, ids)` | `importSequences(path, ids)` |
| Build sequence | `SequenceEditor.createInsertProjectItemAction`, in/out actions | `sequence.insertClip(...)` |
| Markers | `createAddMarkerAction` | yes |
| **Native caption track** | read only (`getCaptionTrack`) — no create call found | **`sequence.createCaptionTrack(srtItem, startAt, CAPTION_FORMAT_SUBTITLE)`** |
| Our transcript in Premiere | `Transcript.importFromJSON` + `createImportTextSegmentsAction` | — |
| Talk to `clipd` | WebSocket client only (no server), domain in manifest | CEP panel |

Plan:
1. Core writes **FCP 7 XML (xmeml)** per sequence (Premiere reads FCP 7 XML, not FCPXML): original source
   with in/out (no re-encode), V1 screen + V2 webcam with crop/motion, face-follow keyframes (≤ 40 per shot),
   cuts as clip items, speed, markers. Exact `effectid`/parameter names for Motion and Crop are not documented
   publicly → learn them from an FCP XML **exported by Premiere itself** (M0 test).
2. Captions in Premiere:
   - **SRT → native caption track** via ExtendScript `createCaptionTrack` (verified API).
   - **Transcript import** (25.6+) so Premiere's Text-Based Editing and Create Captions use our Hindi/Hinglish text.
   - Optional styled overlay `.mov` (ProRes 4444 alpha) for animated looks.
3. Panels: **UXP** panel (25.6+) for import/sequence/markers/transcript; **CEP** panel for caption tracks and for
   25.0–25.5. Same WebSocket protocol. Whether CEP still loads in 26.x is an M0 test; if not, the SRT is placed with
   one drag (Premiere's "New caption track" prompt).
4. **OTIO** export too: write `.otio` JSON directly (Rust bindings are work-in-progress; FCP XML adapter is a Python
   plugin). Premiere 25.6+ imports OTIO.

---

## 4. Feature list

### Create (wizard: Video → Format → Clips → Captions → Review)
- **Automatic** or **Review & edit** workflow.
- Video: file or link, video card, source range.
- Format: aspect, framing (Smart / Full frame / Classic), AI vision for framing, **Cut dead air and filler words**,
  **Video speed** 1 / 1.1 / 1.25 / 1.5 / 1.75 / 2×.
- Clips: What to clip; lengths Extra short 10–30 s, Short 30–60 s, Medium 1–2 m, Long 2–5 m, Extra long 5–10 m,
  Extended 10–15 m, Feature 15–30 m; count 1–100 or Let AI decide; title at the top; model preset.
- Captions: burn-in toggle, style picker with animated sample.
- Review: summary with edit links; queue another video; draft kept across navigation.

### Editor (Review & edit)
- Candidates **Refining → Ready → Baked**, Discard / Restore, Editing badge with count left.
- Judge panel: every check, probability, threshold; Review again (old review marked stale).
- Source + output previews, **Play cuts only**, review speed 1 / 1.5 / 2 / 3× (keys 1/2/3).
- Timeline: trim handles, timecodes, I/O, split (S), delete (X), Extend to playhead, snapping, timeline zoom
  (Full source / Clip / Playhead), frame step, Shift+arrow 1 s, undo/redo, autosave; lanes for hype curve,
  chat spikes, silence.
- Layouts: Full frame / Split / Fit, drag/resize crops, New layout here (diamonds), Use layout for whole clip,
  Smooth movement 0.1–5 s; Find camera changes with sensitivity → Insert layout at cut / Align nearest / Remove /
  Restore markers; focus dots per shot or all similar shots.
- Captions: placement Top / Middle / Bottom / slider / Automatic with guide; pencil correction keeping word times;
  Suppress captions sections; import SRT/VTT/TXT, export SRT/VTT.
- Zoom (Z), sound effects, music, B-roll lanes.
- Mark ready → **Bake** / **Bake all ready**; **Push to Premiere** / **Push all ready**; Replace source video.

### Jobs
Active/queued (2 parallel), previous, filter/search, Run again, Open folder, Details; per-stage progress with
elapsed time; **Processing time by stage**; framing diagnostics; model usage and GPU backend; real error messages.

### Library
Runs grid, Bookmarked / Recent, search; clips with score, reasons, timecodes, tags; **Not exported / Exported to
Premiere**; open folder, push, delete, bulk delete, delete run; **Inspect transcript & edits** (transcript, planner
requests, topic/setup/payoff, judge Q&A, research sources, final intervals); export ZIP; optional title/description/
tags drafts as text.

### Assistant
Chat that drives the app's own tools (start jobs, tidy Library, bake, push to Premiere, settings) through
Antigravity CLI, Claude Code, Codex CLI or a local LLM; approval card before deletes or long GPU work.

### Settings
Model manager (Hugging Face downloads, sizes, backend per model), Sarvam API key (OS secure storage), CLI paths, output folder, storage size, custom
vocabulary, layout profiles, caption presets, **System check**, About/changelog, auto-update (app + yt-dlp).

### Not in scope
Uploading, scheduling, social accounts, posting queues (BridgeClip's Zernio features).

---

## 5. Windows UI (WinUI 3)
- Sidebar: Create · Jobs · Library · Editor · Assistant · Settings.
- BridgeClip's "Flat Glass" rules mapped to native materials: solid dark canvas (#07080C), flat translucent
  panels, blur only on floating bars/dialogs (Acrylic), one blue accent (#1A5FDF) for primary/selection,
  red/amber/green only for status, gold for virality score, errors show the real message + fix,
  reduced-motion and reduced-transparency respected.
- Editor = render + timeline in one frame: candidate list | source + output previews | timeline lanes | right tabs
  (Layout · Captions · Transcript · Judge).
- Preview: core renders frames (crop + libass captions) into a D3D swap chain shown in a **SwapChainPanel**
  (composed with XAML, presents without a XAML render walk). Same renderer as export → preview = output.

---

## 6. Hindi / Hinglish ASR
**Hindi / Hinglish default: Sarvam Saaras v3** (`saaras:v3`). English and offline fallback: Whisper large-v3-turbo.
- Saaras is **not local**: it runs on Sarvam's API (audio leaves the machine, needs a Sarvam API key, billed per use).
  Self-hosting is only offered through AWS SageMaker. Whisper stays the offline path.
- Batch API (`POST /speech-to-text/job/v1`): create job → upload → start → poll/webhook; ≤ 2 h per file,
  ≤ 20 files per job; speaker diarization; auto language detection (`hi-IN`, …).
- Modes: `codemix` for Hinglish as spoken, `transcribe` (Devanagari Hindi), `translit` (Roman script — feeds
  Hinglish captions directly, replacing local transliteration for this path), `verbatim` (keeps fillers and
  repetitions — use this for filler-word and bad-take removal), `translate` (English subtitles).
- **Timestamps are chunk-level only (sentence/phrase), no word-level.** Word-by-word captions, filler cuts and
  dead-air trimming need word times, so a local **forced-alignment** step aligns Saaras text to the audio
  inside each chunk (CTC aligner, ONNX via `ort`). Which aligner holds up on Hinglish is an M0 test.
- Long streams: split into ≤ 2 h files (one job, up to 20 files), shift chunk times back onto the source timeline.
- Custom vocabulary: Saaras v3 has no keyterm list (that is v4-only), so names/tech terms are fixed in the
  local LLM correction pass.
- Bake-off still runs on your audio (M0 test), with Saaras v3 as the candidate to beat: Whisper large-v3-turbo,
  Collabora Whisper-Hindi, Shunya Labs zero-stt-hinglish, AI4Bharat IndicConformer (ONNX).
  Test set 3 × 10 min of your streams, hand-corrected once. Metric: WER + word-timing error after alignment.
- Note: whisper.cpp marks `-ml 1` word timestamps experimental; `--dtw` token timestamps are the fallback.

---

## 7. Milestones

**M0: spikes** (only things docs could not settle)
1. `agy -p` on Windows: reads workspace images headless; empty piped stdout bug.
2. Premiere 25.6+ / 26.x: `importFiles` with xmeml into the open project; CEP panel loads; `createCaptionTrack`
   from SRT; Transcript JSON format (export one from a Premiere-transcribed clip).
3. Export an FCP XML from Premiere with Motion + Crop keyframes → copy its exact xmeml structure.
4. ASR bake-off with Saaras v3 as default candidate, plus forced-alignment accuracy on Hinglish (§6).
5. ffmpeg build check: libass with HarfBuzz (`shaping=complex` renders Devanagari), NVENC/AMF/QSV/VideoToolbox.
6. yt-dlp crate byte-range partial download vs `--download-sections` on a 4 h VOD.

**M1: core + CLI, shorts end-to-end** — ingest, transcript, signals, scorer, planner, render with split layout + captions.
**M2: cleanup + long-form + Premiere** — pacing, AFK, redaction, long-form, xmeml/OTIO/SRT/transcript, UXP + CEP panels.
**M3: WinUI 3 app** — Create, Jobs, Library, Editor, Settings, model manager.
**M4: judges + extras** — logprob judge, agy + local VLM vision, research, zoom, SFX, music, B-roll, translit, Assistant.
**M5: portability** — macOS (Metal/CoreML) and Linux CI builds of the core; OpenRPC spec + UniFFI Swift package.

---

## 8. What changed after verification
- PANNs export dropped → **sherpa-onnx audio tagging** (CED / Zipformer AudioSet) does laughter/cheer/scream directly.
- Separate Silero ONNX dropped → whisper.cpp has Silero v6.2.0 VAD built in.
- Crop motion uses BridgeClip's piecewise ffmpeg `crop` expression (≤ 40 keyframes), not `sendcmd`.
- Judge probabilities come from `llama-server` `/completion` `n_probs` (verified), not the chat endpoint.
- Premiere captions: ExtendScript `createCaptionTrack` is the only verified way to create a caption track by script;
  UXP can only read caption tracks → CEP panel kept for captions.
- Devanagari burn-in requires `shaping=complex` + HarfBuzz-enabled libass.
- Denoise via DeepFilterNet `libDF` (pure Rust, tract), no ORT needed.
- OCR via PP-OCRv5 ONNX models (RapidOCR set) on `ort`.
- C# talks to the core over JSON-RPC (UniFFI has no official C#).
- Hindi/Hinglish ASR moved to Sarvam Saaras v3 (cloud API, chunk-level timestamps) + local forced alignment;
  Whisper kept for English and offline use.

