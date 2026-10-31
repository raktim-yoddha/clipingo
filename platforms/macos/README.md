# Clipingo for macOS

No app yet. This page is everything needed to start one. CI is set to compile and test the core on Apple silicon
on every push; nothing has been run with real media on a Mac yet.

## Suggested stack

SwiftUI with AVFoundation, in `platforms/macos/Clipingo`. It reaches the core over the Unix socket, like every other
client, so it needs no Rust bindings. Another native toolkit is fine if the page says why.

## Set up

```bash
brew install ffmpeg whisper-cpp
cargo build -p clipd -p clip
cargo run -p clip -- check
cargo run -p clip -- models install english
```

## First tasks

Each is one pull request. Tick it here in the same PR.

- [ ] **Prove the core.** Run `python3 tests/headless/run.py` and report what fails. Encoders should pick
      `h264_videotoolbox`. Fix what you find in the core, behind `cfg` where it is Mac-only.
- [ ] **Captions look right.** `captions.rs` maps Windows fonts to Mac stand-ins (`system_font`) and
      `render.rs` sets the fonts folder. Check all 13 styles and Devanagari.
- [ ] **Whisper on Metal.** `models.rs` has no runtime download on macOS: it finds `whisper-cli` on PATH. Confirm
      Homebrew's build gives word times with `--dtw`, or add a pinned download. Then `clip eval` the sync: the lag
      correction in `transcribe.rs` was measured on CUDA.
- [ ] **Face detection speed.** `ort` runs on the CPU here; try the CoreML provider if a scan is slow.
- [ ] **The app shell.** Follow [Building a UI for another OS](../../docs/porting-a-ui.md), steps 1 to 6.
- [ ] **A UI test.** XCUITest driving the same flows as `platforms/windows/tests/e2e/app-e2e.ps1`.
- [ ] **Notarized build.** A signed, notarized `.dmg` in `.github/workflows/release.yml`.

## With an agent

Point it at this repository and say: "Read AGENTS.md, then platforms/macos/README.md, and take the first open task."
Everything it needs is written down; the contract tells it what it may not change.
