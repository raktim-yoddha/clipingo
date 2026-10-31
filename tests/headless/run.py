#!/usr/bin/env python3
"""Headless end-to-end run of the core on any OS: no window, no human, no third-party packages.

Starts its own clipd (separate pipe/socket, runs and exports in a scratch folder, so your Library is not touched),
builds labelled test videos, then drives the whole process over the same JSON-RPC every UI uses:

  create (transcribe, find clips) → framing rules → render 9:16 and 16:9 → evals → editing RPCs → cancel → errors

Each case says which layout its video must get, so the rules are tested against ground truth. `clip eval` scores
every export; scores below tests/headless/baseline.json (less a margin) fail the run.

    python tests/headless/run.py --faces <a video with real faces>        # everything
    python tests/headless/run.py                                          # no face video: the cases that need none
    python tests/headless/run.py --only solo,webcam --update-baseline

--faces must hold an interview: see FACE_SHOTS for the moments used (defaults fit TED's "The future we're building
— and boring"; pass --shots a,b,c for another video). --reference adds its human captions (WebVTT) as a transcript
reference. Needs ffmpeg, the speech models and `cargo build -p clip -p clipd`.
"""

import argparse
import json
import os
import platform
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXE = ".exe" if os.name == "nt" else ""
BIN = ROOT / "target" / "debug"
# (start, seconds): close-up of person A, close-up of person B, a stretch with camera cuts.
FACE_SHOTS = (2101.0, 13.0), (2084.0, 7.0), (1304.5, 60.0)
SCORE_MARGIN = 3.0
FIND = {"lengths": [[10, 30]], "count": 2}
RENDER = {"format": "vertical", "title": True, "cut_dead_air": True}


class Fail(Exception):
    pass


def ffmpeg(*args):
    subprocess.run(["ffmpeg", "-hide_banner", "-v", "error", "-y", *map(str, args)], check=True)


class Core:
    """JSON-RPC 2.0 client: one JSON object per line over the daemon's pipe or socket."""

    def __init__(self, endpoint):
        self.endpoint, self.next_id, self.calls = endpoint, 0, set()
        deadline = time.time() + 15
        while True:
            try:
                if os.name == "nt":
                    self.f = open(endpoint, "r+b", buffering=0)
                else:
                    self.s = socket.socket(socket.AF_UNIX)
                    self.s.connect(endpoint)
                    self.f = self.s.makefile("rwb", buffering=0)
                return
            except OSError:
                if time.time() > deadline:
                    raise
                time.sleep(0.2)

    def send(self, method, params):
        self.next_id += 1
        self.calls.add(method)
        line = json.dumps({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params}) + "\n"
        self.f.write(line.encode())
        self.f.flush()

    def read_line(self):
        buf = bytearray()
        while not buf.endswith(b"\n"):
            b = self.f.read(1)
            if not b:
                raise Fail("clipd closed the connection")
            buf += b
        return json.loads(buf)

    def call(self, method, **params):
        self.send(method, params)
        while True:
            reply = self.read_line()
            if "id" in reply and reply["id"] == self.next_id:
                if "error" in reply:
                    raise Fail(f"{method}: {reply['error']['message']}")
                return reply["result"]

    def error(self, method, **params):
        """The error message of a call that must fail."""
        try:
            self.call(method, **params)
        except Fail as e:
            return str(e)
        raise Fail(f"{method} {params} should have failed")

    def close(self):
        self.f.close()


def check(ok, what):
    if not ok:
        raise Fail(what)


def make_fixtures(faces, shots, out):
    """Labelled videos built from two close-ups, or none without a face video."""
    out.mkdir(parents=True, exist_ok=True)
    made = {}
    a, b = out / "a.mp4", out / "b.mp4"
    if faces:
        for path, (start, length) in ((a, shots[0]), (b, shots[1])):
            if not path.exists():
                ffmpeg("-ss", start, "-t", length, "-i", faces, "-vf", "scale=1280:720", "-c:v", "libx264", "-preset", "veryfast", "-c:a", "aac", path)
        multi = out / "multicam.mp4"
        if not multi.exists():
            ffmpeg("-ss", shots[2][0], "-t", shots[2][1], "-i", faces, "-c:v", "libx264", "-preset", "veryfast", "-c:a", "aac", multi)
        made["multicam"] = multi
    loop = ["-stream_loop", "2"]
    enc = ["-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"]
    if faces:
        solo = out / "solo.mp4"
        if not solo.exists():
            ffmpeg(*loop, "-i", a, "-vf", "scale=1920:1080", "-t", 30, *enc, solo)
        # A game (something moving on the left) with a webcam fixed in the bottom-right corner.
        webcam = out / "webcam.mp4"
        if not webcam.exists():
            graph = (
                "color=c=0x1b2a4a:s=1920x1080:r=30,drawgrid=w=120:h=120:t=2:c=white@0.15[bg];"
                "color=c=0xff5533:s=220x220:r=30[hero];"
                "[bg][hero]overlay=x='260+170*sin(t*2)':y='380+220*cos(t*3)':shortest=1[game];"
                "[0:v]scale=400:225[cam];[game][cam]overlay=W-w-24:H-h-24:shortest=1[v]"
            )
            ffmpeg(*loop, "-i", a, "-filter_complex", graph, "-map", "[v]", "-map", "0:a", "-t", 30, *enc, webcam)
        # Two people side by side, as in a video call or a two-camera podcast.
        podcast = out / "podcast.mp4"
        if not podcast.exists():
            graph = "[0:v]scale=960:540,setsar=1[l];[1:v]scale=960:540,setsar=1[r];[l][r]hstack,pad=1920:1080:0:270[v]"
            ffmpeg(*loop, "-i", a, "-stream_loop", 4, "-i", b, "-filter_complex", graph, "-map", "[v]", "-map", "0:a", "-t", 30, *enc, podcast)
        group = out / "group.mp4"
        if not group.exists():
            graph = "[0:v]split=2[a1][a2];[a1]scale=640:360,setsar=1[l];[1:v]scale=640:360,setsar=1[m];[a2]hflip,scale=640:360,setsar=1[r];[l][m][r]hstack=3,pad=1920:1080:0:360[v]"
            ffmpeg(*loop, "-i", a, "-stream_loop", 4, "-i", b, "-filter_complex", graph, "-map", "[v]", "-map", "0:a", "-t", 30, *enc, group)
        made.update(solo=solo, webcam=webcam, podcast=podcast, group=group)
    # Nobody on screen, exact word times: the text-to-speech stream.
    stream = Path(tempfile.gettempdir()) / "clipingo-e2e" / "english-stream.mp4"
    if not stream.exists() and os.name == "nt":
        subprocess.run(["powershell", "-File", str(ROOT / "platforms" / "windows" / "tests" / "e2e" / "make-english-stream.ps1")], check=True, stdout=subprocess.DEVNULL)
    if stream.exists():
        made["screen"] = stream
    return made


# case → the layout that must fill most of the clip (None: only that several are used).
EXPECT = {"solo": "follow", "webcam": "split", "podcast": "stack", "group": "group", "screen": "whole", "multicam": None}


def layout_share(shots):
    total = sum(s["end"] - s["start"] for s in shots) or 1
    out = {}
    for s in shots:
        out[s["layout"]] = out.get(s["layout"], 0) + (s["end"] - s["start"]) / total
    return out


def clip(*args):
    run = subprocess.run([str(BIN / f"clip{EXE}"), *map(str, args)], capture_output=True, text=True, encoding="utf-8", env=os.environ)
    if run.returncode:
        raise Fail(f"clip {args[0]}: {run.stderr.strip()[-300:]}")
    return json.loads(run.stdout)


def run_case(core, name, video, reference, work):
    """Create → framing → render both formats → evals. Returns the result row."""
    t0 = time.time()
    run = core.call("run.create", input=str(video), options={"transcribe": {"language": "english"}, "find": FIND})
    created = time.time() - t0
    check(run["candidates"], f"{name}: no clips found")
    rank = run["candidates"][0]["rank"]
    shots = core.call("run.framing", id=run["id"], rank=rank, options=RENDER)
    share = layout_share(shots)
    want = EXPECT[name]
    if want:
        check(share.get(want, 0) >= 0.75, f"{name}: expected {want}, got {share}")
    else:
        check(len(share) >= 2, f"{name}: expected several layouts, got {share}")
    if name == "webcam":
        s = next(s for s in shots if s["layout"] == "split")
        top, bottom = s["panels"][0], s["panels"][1]
        check(top["dest_y"] == 0 and bottom["dest_y"] > 0, "webcam: face panel first")
        face_x = top["keys"][0][1] + top["w"] / 2
        action_x = sum(k[1] for k in bottom["keys"]) / len(bottom["keys"]) + bottom["w"] / 2
        check(face_x > 1400, f"webcam: face panel on the webcam, at {face_x}")
        check(action_x < 1000, f"webcam: content panel on the action, at {action_x}")
        swapped = core.call("run.framing", id=run["id"], rank=rank, options={**RENDER, "swap_stack": True})
        s2 = next(s for s in swapped if s["layout"] == "split")
        check(s2["panels"][0]["w"] == bottom["w"] and s2["panels"][1]["w"] == top["w"], "webcam: swap puts the content on top")
    # Tracking is found once and kept with the video: opening the clip again answers at once.
    t2 = time.time()
    again = core.call("run.framing", id=run["id"], rank=rank, options=RENDER)
    check(time.time() - t2 < 2 and again == shots, f"{name}: framing scanned again ({time.time() - t2:.1f} s)")
    t1 = time.time()
    rendered = core.call("run.render", id=run["id"], ranks=[rank], options=RENDER)
    if name in ("solo", "webcam"):
        rendered += core.call("run.render", id=run["id"], ranks=[rank], options={**RENDER, "format": "horizontal"})
    took = time.time() - t1
    for r in rendered:
        check(Path(r["video"]).stat().st_size > 10_000, f"{name}: empty export")
        check(Path(r["srt"]).exists() and Path(r["premiere_xml"]).exists(), f"{name}: missing sidecars")
    report = clip("eval", run["id"], *([reference] if reference else []))
    frames = work / "frames"
    frames.mkdir(exist_ok=True)
    shot = frames / f"{name}.png"
    ffmpeg("-ss", min(rendered[0]["duration_s"] / 2, 8), "-i", rendered[0]["video"], "-frames:v", 1, "-vf", "scale=270:-2", shot)
    return {"case": name, "run": run["id"], "layouts": {k: round(v, 2) for k, v in share.items()}, "score": round(report["score"], 1),
            "create_s": round(created, 1), "render_s": round(took, 1), "failures": report["failures"], "report": report, "frame": str(shot),
            "videos": [r["video"] for r in rendered]}


def api_contract(core, run_id):
    """Every editing call, and that bad input fails with a message instead of a crash."""
    opened = core.call("run.open", id=run_id)
    run = opened["run"]
    check(run["id"] == run_id and "options" in opened, "run.open")
    check(core.call("run.get", id=run_id)["id"] == run_id, "run.get")
    check(any(r["id"] == run_id for r in core.call("run.list")), "run.list")
    c = run["candidates"][0]
    words = core.call("run.words", id=run_id, start=c["start"], end=c["end"])
    check(words, "run.words")
    # Trim, then stitch two parts in reverse order, and read it back.
    mid = (c["start"] + c["end"]) / 2
    edited = core.call("run.edit", id=run_id, rank=c["rank"], title="Stitched", segments=[[mid, c["end"]], [c["start"], mid]])
    check(len(edited["segments"]) == 2 and edited["title"] == "Stitched", "run.edit segments")
    rendered = core.call("run.render", id=run_id, ranks=[c["rank"]], options={"format": "vertical", "cut_dead_air": False})[0]
    check(abs(rendered["duration_s"] - (c["end"] - c["start"])) < 0.2, "stitched length")
    fix = " ".join(["FIXED"] + [w["w"] for w in words[1:]])
    check(core.call("run.retext", id=run_id, start=c["start"], end=c["end"], text=fix)[0]["w"] == "FIXED", "run.retext")
    added = core.call("run.add_clip", id=run_id, start=1.0, end=6.0, title="By hand")
    check(added["reasons"] == ["Added by you"], "run.add_clip")
    check(core.call("run.thumbs", id=run_id, times=[1, 2.5], height=90)[0], "run.thumbs")
    # A shot's layout, chosen and cleared.
    shots = core.call("run.framing", id=run_id, rank=added["rank"], options=RENDER)
    pick = next((o for o in shots[0]["options"] if o != shots[0]["layout"]), None)
    if pick:
        core.call("run.set_layout", id=run_id, rank=added["rank"], at=2.0, shot=[1.0, 6.0], layout=pick)
        again = core.call("run.framing", id=run_id, rank=added["rank"], options=RENDER)
        check(again[0]["layout"] == pick and again[0]["why"] == "Your choice", "run.set_layout")
        core.call("run.set_layout", id=run_id, rank=added["rank"], at=2.0, shot=[1.0, 6.0], layout=None)
    core.call("run.remove_clip", id=run_id, rank=added["rank"])
    found = core.call("scorer.find", words=[{"w": w["w"], "s": w["s"], "e": w["e"]} for w in words], options={"min_len": 3, "max_len": 30})
    check(isinstance(found, list), "scorer.find")
    check(core.call("transcript.run", input=run["media"], options={"language": "english"})["words"], "transcript.run")
    check("unknown model" in core.error("models.install", id="klingon"), "unknown model")
    check("no clip" in core.error("run.remove_clip", id=run_id, rank=999), "remove a clip that is not there")
    check("no clip" in core.error("run.framing", id=run_id, rank=999), "framing of a clip that is not there")
    check("invalid run id" in core.error("run.get", id="../../etc"), "paths are not run ids")
    check("not found" in core.error("run.get", id="0-0"), "unknown run")
    check("1 second" in core.error("run.edit", id=run_id, rank=c["rank"], title="x", start=5, end=5.1), "too short")
    check("title" in core.error("run.edit", id=run_id, rank=c["rank"], title=" ", start=1, end=5), "empty title")


def all_styles(core, run_id, rank):
    """Every caption style and framing mode renders."""
    styles = [p["id"] for p in core.call("captions.presets")]
    check(len(styles) == 13, f"{len(styles)} styles")
    for style in styles:
        out = core.call("run.render", id=run_id, ranks=[rank], options={"format": "vertical", "caption_style": style, "framing": "classic", "speed": 2})
        check(out[0]["duration_s"] > 1, f"style {style}")
    for framing in ("smart", "full", "classic"):
        for position in ("auto", "top", "middle", "bottom"):
            core.call("run.render", id=run_id, ranks=[rank], options={"format": "vertical", "framing": framing, "caption_position": position, "fade": True, "speed": 1.5})
    return len(styles)


def cancel(endpoint, video):
    """Hanging up stops the work: no whisper or ffmpeg left behind by this daemon."""
    core = Core(endpoint)
    core.send("run.create", {"input": str(video), "options": {"transcribe": {"language": "english", "refresh": True}}})
    time.sleep(2.5)
    core.close()
    time.sleep(2)
    if os.name == "nt":
        tasks = subprocess.run(["tasklist"], capture_output=True, text=True).stdout.lower()
        check("whisper-cli" not in tasks, "cancel: whisper still running")
    else:
        check(subprocess.run(["pgrep", "whisper-cli"], capture_output=True).returncode != 0, "cancel: whisper still running")


def contact_sheet(rows, out):
    frames = [r["frame"] for r in rows if Path(r.get("frame", "")).exists()]
    if frames:
        inputs = [x for f in frames for x in ("-i", f)]
        ffmpeg(*inputs, "-filter_complex", f"hstack=inputs={len(frames)}" if len(frames) > 1 else "null", "-frames:v", 1, out)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--faces", help="a video with real faces (an interview)")
    ap.add_argument("--reference", help="its captions (WebVTT), to score the multi-camera transcript")
    ap.add_argument("--shots", help="start seconds of close-up A, close-up B and a stretch with cuts")
    ap.add_argument("--only", help="comma-separated cases")
    ap.add_argument("--work", default=str(ROOT / "target" / "e2e"), help="scratch folder")
    ap.add_argument("--update-baseline", action="store_true")
    a = ap.parse_args()
    work = Path(a.work)
    shots = FACE_SHOTS
    if a.shots:
        s = [float(x) for x in a.shots.split(",")]
        shots = (s[0], 13.0), (s[1], 7.0), (s[2], 60.0)
    videos = make_fixtures(a.faces, shots, work / "fixtures")
    if a.only:
        videos = {k: v for k, v in videos.items() if k in a.only.split(",")}
    check(videos, "no test videos")

    data = work / "data"
    shutil.rmtree(data, ignore_errors=True)
    (data / "out").mkdir(parents=True)
    env = {"CLIPINGO_RUNS_DIR": str(data / "runs"), "CLIPINGO_OUTPUT_DIR": str(data / "out"), "CLIPINGO_ENDPOINT": r"\\.\pipe\clipingo-test" if os.name == "nt" else str(data / "clipd.sock")}
    os.environ.update(env)
    daemon = subprocess.Popen([str(BIN / f"clipd{EXE}")], env=os.environ, stderr=subprocess.DEVNULL)
    rows, problems = [], []
    t0 = time.time()
    try:
        core = Core(env["CLIPINGO_ENDPOINT"])
        version = core.call("version")["version"]
        system = core.call("system.check")
        check(system["selected_encoder"], "no working video encoder")
        check(core.call("source.probe", input=str(next(iter(videos.values()))))["duration_s"] > 1, "source.probe")
        check(any(m["installed"] for m in core.call("models.list")["models"]), "no speech model installed")
        print(f"clipd {version} on {platform.system()}, encoder {system['selected_encoder']}")
        for name, video in videos.items():
            speech = video.with_name("english-speech.vtt") if name == "screen" else None
            reference = speech if speech and speech.exists() else None
            if name == "multicam" and a.reference:
                reference = work / "multicam.vtt"
                trim_reference(Path(a.reference), shots[2], reference)
            try:
                row = run_case(core, name, video, reference, work)
                rows.append(row)
                print(f"  {name:9s} {row['score']:5.1f}  {row['layouts']}  create {row['create_s']} s, render {row['render_s']} s")
                for f in row["failures"]:
                    print(f"            miss: {f}")
            except Fail as e:
                problems.append(str(e))
                print(f"  {name:9s} FAILED {e}")
        if rows:
            r = rows[0]
            api_contract(core, r["run"])
            print("  editing calls and bad input: ok")
            rank = core.call("run.get", id=r["run"])["candidates"][0]["rank"]
            print(f"  {all_styles(core, r['run'], rank)} caption styles, 3 framings × 4 caption positions: ok")
            core.call("run.delete", id=r["run"])
            check("not found" in core.error("run.get", id=r["run"]), "run.delete")
            print("  delete: ok")
        cancel(env["CLIPINGO_ENDPOINT"], next(iter(videos.values())))
        print("  cancel stops the work: ok")
    except (Fail, subprocess.CalledProcessError, OSError) as e:
        problems.append(f"{type(e).__name__}: {e}")
    finally:
        daemon.kill()

    baseline_file = ROOT / "tests" / "headless" / "baseline.json"
    baseline = json.loads(baseline_file.read_text(encoding="utf-8")) if baseline_file.exists() else {}
    for row in rows:
        floor = baseline.get(row["case"])
        if floor is not None and row["score"] < floor - SCORE_MARGIN:
            problems.append(f"{row['case']}: score {row['score']} fell below baseline {floor}")
    if a.update_baseline:
        baseline.update({r["case"]: r["score"] for r in rows})
        baseline_file.write_text(json.dumps(baseline, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    contact_sheet(rows, work / "contact-sheet.png")
    result = {"seconds": round(time.time() - t0), "cases": rows, "problems": problems, "calls": sorted(core.calls)}
    (work / "report.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(f"mean score {sum(r['score'] for r in rows) / max(1, len(rows)):.1f}, {len(rows)} cases, {result['seconds']} s, report: {work / 'report.json'}")
    for p in problems:
        print(f"PROBLEM: {p}")
    sys.exit(1 if problems else 0)


def trim_reference(src, shot, out):
    """The cues of a stretch of the video, moved to start at 0."""
    start, length = shot
    lines, cue = ["WEBVTT", ""], None

    def secs(s):
        h, m, r = s.strip().split(":")
        return int(h) * 3600 + int(m) * 60 + float(r)

    def stamp(t):
        return f"{int(t // 3600):02}:{int(t // 60 % 60):02}:{t % 60:06.3f}"

    for line in src.read_text(encoding="utf-8").splitlines():
        if "-->" in line:
            a, b = (secs(x.split()[0]) for x in line.split("-->"))
            cue = (a, b) if start <= a < start + length else None
            if cue:
                lines.append(f"{stamp(a - start)} --> {stamp(b - start)}")
        elif cue:
            lines.append(line)
    out.write_text("\n".join(lines) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
