# M0 spike results

Machine: Windows 11, RTX 4060 Laptop (8 GB, driver 581.86), Intel iGPU, ffmpeg 9.0 (gyan.dev full build), agy 1.4.7.
Re-run any check with the script in its folder.

## 1. ffmpeg: HarfBuzz + encoders — `ffmpeg/check.ps1`

| Check | Result |
|---|---|
| libass / libharfbuzz / libfribidi compiled in | yes / yes / yes |
| Devanagari with `ass=…:fontsdir=…:shaping=complex` | **Correct**: conjuncts and matras render (श्रद्धा, क्षत्रिय, क्या) |
| Same with `shaping=simple` | **Broken**: visible halants, wrong matra order — never use for Hindi |
| `h264_qsv`, `h264_mf`, `libx264` | work |
| `h264_amf` | unavailable (no AMD GPU — expected) |
| `h264_nvenc` / `hevc_nvenc` / `av1_nvenc` | driver 581.86: **fail** ("Driver does not support the required nvenc API version. Required: 13.1 Found: 13.0"). After updating to driver **621.01: all three work**; core selects `h264_nvenc` |

Decisions:
- Caption burn-in always passes `shaping=complex`; `fontsdir` works without fontconfig (Windows font provider).
- NVENC needs an NVIDIA driver that supports NVENC API 13.1 (581.86 does not, 621.01 does), **or** an ffmpeg build
  against older nv-codec-headers. Done: the core probes encoders by actually opening them and falls back
  NVENC → QSV → AMF → Media Foundation → libx264; System check shows "update your NVIDIA GPU driver" when NVENC fails this way.

## 2. Antigravity CLI headless visual check — `agy/check.ps1`

| Check | Result |
|---|---|
| Piped stdout empty (reported Windows bug) | **Not reproduced**: JSON envelope always arrived (also on auth failure) |
| Headless auth | Uses the interactive `agy` login; without one it exits after 60 s with `status: ERROR` (no hang) |
| `--json-schema` | `structured_output` returned and schema-valid |
| Reads a workspace image | **Yes** — random code and all 4 tile colours exact |
| Time | ~32 s agent time, ~56 s wall per call (≈27k input tokens for one 960×540 sheet) |

Gotchas found:
- With a vague prompt the agent tried a **shell command** to inspect the file; headless soft-denies it
  (`denied_actions: [command]`), exits 0 with an empty `response`. Fix: prompt says *"use your file viewing tool,
  not a shell command"* and gives the absolute path. Never pass `--dangerously-skip-permissions`.
- Windows PowerShell 5.1 strips quotes from JSON passed as an argument → pass `--json-schema` as a **file path**.
  (The Rust core spawns agy with an argv array, so it is unaffected, but keep using the file path.)
- Treat `status != SUCCESS`, empty `structured_output`, or non-empty `denied_actions` as "check unavailable".

## 3. Premiere import + captions — `premiere/`

**Not run: Premiere Pro is not installed on this machine** (`C:\Program Files\Adobe\Adobe Premiere Pro 2025`
holds only a leftover log). Ready-to-run bundle:

1. `pwsh spikes/m0/premiere/make-bundle.ps1` → `bundle/test.mp4`, `bundle/test.srt` (Hindi/Hinglish/English),
   `bundle/clipingo-m0.xml` (xmeml v5: 1080×1920, V1 screen crop + V2 webcam crop with keyed Motion, jump cut, markers).
2. **ExtendScript** (all 25.x/26.x): open any project, run `test-extendscript.jsx` with VS Code's ExtendScript
   Debugger → `bundle/results-extendscript.txt`. Answers: XML into the *open* project? 1080×1920? Crop/Motion
   keyframes honoured? `createCaptionTrack(srt)` makes a native caption track?
3. **UXP** (25.6+): load `uxp/` in UXP Developer Tool, paste the bundle path, Run → `bundle/results-uxp.txt`.
   Answers: `importFiles` of XML, any create-caption call in UXP, and saves `transcript-format.json` from a clip
   Premiere transcribed (the Transcript import format we need).
4. In Premiere, File → Export → Final Cut Pro XML of the imported sequence → compare its Motion/Crop
   `effectid`/parameter names with ours.
