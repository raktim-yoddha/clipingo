# Clipingo for Linux

No app yet. This page is everything needed to start one. CI is set to compile and test the core on Ubuntu on every
push; nothing has been run with real media on Linux yet.

## Suggested stack

GTK 4 with libadwaita and GStreamer, in `platforms/linux/clipingo-gtk`, written in Rust with `gtk4-rs` (a workspace
member) so it shares the toolchain. It still reaches the core over the Unix socket, never by calling the library, so
the protocol stays the one way in. Another native toolkit is fine if the page says why.

## Set up

```bash
sudo apt install ffmpeg python3 libass9 fonts-dejavu fonts-noto-core pkg-config libssl-dev
cargo build -p clipd -p clip
cargo run -p clip -- check
```

`whisper-cli` must be on PATH: build [whisper.cpp](https://github.com/ggml-org/whisper.cpp) (tag in `models.rs`) with
Vulkan or CUDA. Then:

```bash
cargo run -p clip -- models install english
```

## First tasks

Each is one pull request. Tick it here in the same PR.

- [ ] **Prove the core.** Run `python3 tests/headless/run.py` and report what fails. Encoders try NVENC, QSV and AMF,
      but VAAPI is missing from `ENCODERS` in `system.rs`: add `h264_vaapi` with its arguments in `render.rs`.
- [ ] **Captions look right.** `captions.rs` maps Windows fonts to DejaVu (`system_font`) and `render.rs` points at
      `/usr/share/fonts`. Check all 13 styles and Devanagari; bundling open fonts may be the better answer.
- [ ] **Whisper runtime.** `models.rs` has no runtime download on Linux. Add a pinned build, or document the distro
      packages. Then `clip eval` the sync: the lag correction in `transcribe.rs` was measured on CUDA.
- [ ] **ffmpeg download.** `tools.rs` has none for Linux; decide between a pinned static build and the distro's.
- [ ] **The app shell.** Follow [Building a UI for another OS](../../docs/porting-a-ui.md), steps 1 to 6.
- [ ] **A UI test.** AT-SPI (dogtail) driving the same flows as `platforms/windows/tests/e2e/app-e2e.ps1`.
- [ ] **Package.** A Flatpak manifest, and a job in `.github/workflows/release.yml`.

## With an agent

Point it at this repository and say: "Read AGENTS.md, then platforms/linux/README.md, and take the first open task."
Everything it needs is written down; the contract tells it what it may not change.
