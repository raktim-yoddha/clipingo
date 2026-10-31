# Building a UI for another OS

A native Clipingo app is a client of `clipd`. Finding, framing, captioning and rendering clips are all in the core, and
[rpc.md](rpc.md) is how to reach them. Everything else is the app's own: see [what each app builds](#what-each-app-builds).
Build screens in this order; each is usable before the next exists. The Windows app in `platforms/windows/Clipingo` is
the reference for behaviour and wording.

## 1. Connect

Start `clipd` if it is not running, connect to the socket, call `version`. Ship the binary inside the app; let
`CLIPINGO_CLIPD` point elsewhere for development. Open a second connection for each long call.

**Done when:** the app shows the core's version, and restarts `clipd` if it dies.

## 2. Create

A video (file or link), then options, then Start: `source.probe`, `run.create`, `run.render`. Keep the same order as
Windows: Video, Format, Clips, Captions, Review. Options come from the render options in rpc.md.

**Done when:** a file becomes exported clips with progress shown, and Cancel closes the connection.

## 3. Jobs and Library

Jobs are this session's calls, two at a time. The Library is `run.list`; Edit opens the editor, Remove is `run.delete`
after asking.

## 4. Editor

Three columns and a timeline: clips, player, inspector.

| Part | Calls |
|---|---|
| Clips, with a play button per exported format | `run.open`, `run.thumbs` |
| Timeline: split, trim, reorder, delete, stitch, undo | `run.edit` with `segments`, a moment after each change |
| Marking a part of the source | `run.add_clip`, or a new segment |
| 9:16 Edit view: the source with crop boxes, and the vertical frame | `run.framing` |
| This shot's layout, swap, smart zoom | `run.set_layout`, then `run.framing` again |
| Caption text | `run.retext` |
| Export | `run.render` |

Preview the vertical frame from the plan's panels, not by rendering. Windows plays muted copies of the source inside
clipped views; on macOS `AVPlayerLayer` with a transform does it, on Linux a GStreamer `videocrop` per panel.

**Done when:** every flow in `platforms/windows/tests/e2e/app-e2e.ps1` has an equivalent automated test on your OS.

## 5. Settings

`system.check` and `models.list`, `models.install` with progress. A missing whisper or ffmpeg must say what to do.

## 6. Package

A signed, per-user install with `clipd` inside; updates if the platform allows. Add the job to
`.github/workflows/release.yml`.

## What each app builds

The frontend is native on each OS, so these stay in the app and are written once per platform, in that platform's own
idiom (ADR 0002). Take the behaviour from the file named.

| Logic | Windows reference |
|---|---|
| The editing session: the list of segments, split, trim, reorder, delete and undo. The core only receives the result through `run.edit` | `ViewModels/EditorViewModel.cs` |
| Mapping between the edit clock and source time, and the words shown on the timeline | `ViewModels/EditorViewModel.cs` |
| Live preview: following a panel's crop keys and easing its zooms, using the formulas in rpc.md | `CropAt`, `ZoomAt` in `EditorViewModel.cs` |
| Live preview of captions and the title over the player | `Views/EditorPage.xaml.cs` |
| Wizard presets and defaults: clip lengths, speeds, the time range parser | `ViewModels/CreateViewModel.cs` |
| Updates, which differ per OS anyway | `Services/Updater.cs` |

## UI rules

Plain words ("Two people", not "Stack"), one primary action per screen, nothing shown that a shot cannot do, every
control reachable by keyboard and named for screen readers. Native controls over custom ones.
