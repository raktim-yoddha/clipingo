# Evals

Quality here is measured, not eyeballed. Two tools share the same checks:

- `clip eval <run-id> [captions.vtt|srt]` scores one run: the targets it misses, and 0–100 per export and overall.
- `tests/headless/run.py` runs the whole process on labelled videos and fails on a wrong layout or a score below
  `tests/headless/baseline.json` (less a margin of 3).

## What is measured

| Stage | Check | Target |
|---|---|---|
| Transcript (with a reference) | Word error rate | ≤ 15 % |
| | Words heard | ≥ 88 % |
| | Sync of our words with the reference | median ≤ 0.25 s, ≥ 70 % within 0.3 s |
| Clips | Start after a sentence end or pause, end on one | ≥ 80 % |
| | No cut inside a word | every clip |
| | Do not open mid-thought ("and", "so", "because"…) | ≥ 80 % |
| Framing (ground truth) | The layout the video calls for | ≥ 75 % of the clip |
| Framing (from the plan) | Enlargement of a crop | ≤ 3.6× (a crop as tall as the source does not count) |
| | Zooms | ≤ 10 a minute |
| Export | Size for the format | exact |
| | Whole face, against how often the source shows one | ≥ 80 % |
| | Face steadiness inside a shot | ≤ 0.04 of the width per sample |
| | Captions over a face | ≤ 10 % |
| | Words in the captions, against the words kept | ≥ 95 % |
| | Reading speed (fastest 5 %) | ≤ 30 characters a second |
| | Captions inside the clip | all |
| | Loudness | −14 ± 2 LUFS |
| | True peak | ≤ −0.5 dBFS |
| | Black picture | ≤ 0.5 s |

Scores weigh the checks that apply: face 30, steadiness, captions on faces, caption words, loudness and size 10 each,
reading speed, face size, black and enlargement 5 each.

## Test videos

`tests/headless/run.py --faces <video> [--reference captions.vtt]` builds them from an interview, so each has a known
answer:

| Case | Built from | Must get |
|---|---|---|
| `solo` | One person's close-up | Follow |
| `webcam` | A moving "game" with a small webcam in a corner | Face + content, the face panel on the webcam and the content panel on the action; the swap turns it over |
| `podcast` | Two people side by side | Two people |
| `group` | Three people side by side | Everyone |
| `screen` | A screen with synthetic speech, nobody on camera | Whole frame; transcript against the synthesizer's exact word times (Windows) |
| `multicam` | A stretch with real camera cuts | Several layouts; transcript against human captions |

The reference video is not in the repository. The defaults of `FACE_SHOTS` fit TED's "The future we're building — and
boring"; for any other interview pass `--shots <close-up A>,<close-up B>,<a stretch with cuts>`.

After the cases it runs every editing call, bad input (which must fail with a message, not a crash), all 13 caption
styles, 3 framings × 4 caption positions, delete and cancel. It uses its own daemon, pipe and folders, so your Library
is not touched.

## Changing the numbers

Raising a target or the baseline needs nothing but the better result: `--update-baseline`. Lowering either needs an ADR
that says why (see `AGENTS.md`).
