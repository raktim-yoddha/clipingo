# ADR 0003 — Vertical clips are framed by rules per shot

Decided by the product owner, 2026-10-30.

## Context

9:16 clips followed one face, which fits a talking head and little else: interviews lost a person, streams lost the
game. The product owner pointed to Clippo's reframing and asked for several rules working together, including a face
stacked with the action on screen, either way up.

## Decision

Each camera shot gets a layout: **Follow**, **Two people** (stacked), **Face + content** (a webcam or face stacked with
the moving part of the frame), **Everyone** (a wide band) or **Whole frame**, chosen by rules from the faces, a motion
map and mouth movement. A shot reports which layouts it can show; the user can pick one per shot and swap the stacked
order. Captions sit on the seam of a stack or below a band (`auto` position). Smart zoom pushes in on strong lines,
spaced apart and clear of cuts.

## Consequences

A plan is computed per render and per preview. Face + content is automatic only for a small face that stays in a corner
for most of the clip, and otherwise a choice in the editor, because a small face alone cannot tell a stage from a
stream.

## Amendment (2026-10-31): tracking is saved with the video

Faces, motion and cuts of a stretch of video are saved in `runs/<id>/scans/` the first time they are found. Opening a clip
or exporting it again reuses them, so the boxes stay fixed once a clip is made. A trim inside a scanned part reuses that
scan; a stretch outside it is scanned once. `SCAN_VERSION` retires old scans when detection changes.
