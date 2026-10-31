# The core protocol

Every UI talks to the same daemon, `clipd`, in JSON-RPC 2.0: one JSON object per line, UTF-8.

| OS | Endpoint | Override |
|---|---|---|
| Windows | named pipe `\\.\pipe\clipingo` | `CLIPINGO_ENDPOINT` |
| macOS, Linux | Unix socket `$TMPDIR/clipingo.sock` | `CLIPINGO_ENDPOINT` |

Run `clipd` yourself or start it from the app (the Windows app looks for `CLIPINGO_CLIPD`, then beside the app, then
`target/`). One connection handles one call at a time. Long work (`run.create`, `run.render`, `run.framing`,
`models.install`) belongs on its own connection: the call is cancelled when that connection closes, which also stops
its ffmpeg, whisper and yt-dlp processes.

Long calls stream notifications before their reply:

```json
{"jsonrpc":"2.0","method":"progress","params":{"request":7,"stage":"Rendering clip #1 (h264_nvenc)","done":42,"total":100}}
```

`total` is null when unknown. Errors are `{"code":-32000,"message":"…"}` with a message fit to show the user;
`-32602` is bad parameters, `-32601` an unknown method.

All times are seconds and all sizes are pixels. The source of truth for the shapes is the Rust code in
`crates/clipd/src/main.rs` and `crates/clipingo-core`; the Windows app's mirrors are in
`platforms/windows/Clipingo/Models/CoreModels.cs`.

## Methods

| Method | Params | Result |
|---|---|---|
| `version` | — | `{ version }` |
| `system.check` | — | tools, NVIDIA GPUs, probed H.264 encoders (with fix hints), selected encoder, whisper-cli, speech models |
| `source.probe` | `{ input }` file path or https YouTube link | title, channel, duration, size, fps, views, upload date |
| `models.list` | — | whisper-cli path, models with installed state |
| `models.install` | `{ id: english\|hinglish\|hindi\|runtime\|yt-dlp\|deno }` | `{ path }`; streams progress |
| `transcript.run` | `{ input, options?: {language: auto\|english\|hindi\|hinglish, refresh} }` | words with times, model, language |
| `scorer.find` | `{ words: [{w,s,e}], options?: {min_len,max_len,lengths: [[min,max]],count,within: [start,end],request} }` | ranked clip candidates |
| `run.create` | `{ input, options?: {transcribe, find} }` | saved run: source, media, transcript summary, ranked candidates; streams progress |
| `run.get` | `{ id }` | a saved run |
| `run.list` | — | saved runs, newest first, with their exported clips |
| `run.open` | `{ id }` | the run, its clips folder, exported clips and last export options |
| `run.delete` | `{ id }` | removes a saved run (exported clips stay) |
| `run.words` | `{ id, start, end }` | transcript words in that part of the source |
| `run.edit` | `{ id, rank, start, end, title, segments?: [[start, end], …] }` | saves a clip's trim (or the source ranges it plays in order) and title |
| `run.add_clip` | `{ id, start, end, title }` | a clip marked by hand (next free rank) |
| `run.remove_clip` | `{ id, rank }` | removes a clip from the run (exported files stay) |
| `run.retext` | `{ id, start, end, text }` | corrected caption text for that span (word times kept or shared) |
| `run.thumbs` | `{ id, times: [s], height? }` | JPEG frames of the video at those times (cached in the run folder) |
| `run.framing` | `{ id, rank, options? }` (render options) | how a clip is framed in 9:16 on the editor's clock, see below; streams progress |
| `run.set_layout` | `{ id, rank, at, shot: [start, end], layout? }` source seconds; no layout: back to the rules | the clip |
| `run.render` | `{ id, ranks: [1,2], options? }` | per clip: `.mp4`, `.srt`, Premiere `.xml`, duration, dead air removed, encoder; streams progress |
| `captions.presets` | — | the 13 caption styles: id, name, description, font, colours |

### Render options

`format: vertical|horizontal`, `framing: smart|full|classic`, `vision_check`, `cut_dead_air`, `captions`,
`caption_style`, `caption_position: auto|top|middle|bottom`, `caption_size` (0.6–1.6), `fade`, `title`, `speed` (1–2),
`smart_zoom`, `swap_stack`. All optional.

### A framing plan

`run.framing` returns the shots of a clip on the editor's clock (its parts back to back, no dead-air cuts), so a UI can
draw them over what it plays and preview the vertical frame without rendering:

```json
{
  "start": 12.6, "end": 17.4,
  "layout": "stack",
  "why": "Two people apart",
  "people": 2,
  "options": ["whole", "follow", "split", "stack", "group"],
  "panels": [{ "w": 414, "h": 368, "dest_y": 0, "dest_h": 960, "keys": [[12.6, 0, 0]] }],
  "zooms": [{ "t": 14.1, "end": 16.0, "amount": 0.14, "slow": false, "word": "million" }],
  "caption_y": 960
}
```

- `layout`: `follow`, `split` (face + content), `stack` (two people), `group`, `whole`.
- `options`: the layouts this shot can really show; offer only these.
- `panels`: crops of the source, top to bottom. `keys` are `[t, left, top]`: straight lines between them, the last one
  held. Each is scaled to 1080 wide and `dest_h` tall at `dest_y` in the 1080×1920 frame. `whole` has none: fit the
  frame to the width over a blurred fill.
- `zooms`: enlarge the panel around its centre line, 40 % down: sine ease in over `0.3 + 2 × amount` s (a `slow` one
  over all of `end − t`), sine ease out over `0.45 + 2 × amount` s after `end`.
- `caption_y`: centre height of the captions in the frame when the position is `auto`.

Faces, motion and cuts are saved with the run the first time a stretch of video is scanned, so opening a clip again,
trying a layout, the swap or the zoom, and exporting all answer without scanning. A trim inside a scanned part reuses
that scan.
