//! Render clips from a run: dead-air cuts, 9:16 / 16:9 framing, burned word-highlight captions,
//! loudness normalisation, hardware encoding; plus an SRT and a Premiere (FCP 7 XML) timeline per clip.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::Word;
use crate::framing::{Face, Framing, Layout, Motion, Request, Shot};
use crate::models::{Progress, ProgressFn};
use crate::pacing::{self, WindowWord};
use crate::pipeline::{self, Run};
use crate::scorer::Candidate;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// 1080×1920 for Shorts / Reels.
    #[default]
    Vertical,
    /// 1920×1080.
    Horizontal,
}

impl Format {
    fn size(self) -> (u32, u32) {
        match self {
            Format::Vertical => (1080, 1920),
            Format::Horizontal => (1920, 1080),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderOptions {
    pub format: Format,
    pub cut_dead_air: bool,
    /// Burn word-by-word captions.
    pub captions: bool,
    /// Caption look: an id from `captions::PRESETS`.
    pub caption_style: String,
    /// The clip's title over the top of the video.
    pub title: bool,
    /// Playback speed of the exported clip, 1–2 (pitch kept).
    pub speed: f64,
    /// 9:16 only.
    pub framing: Framing,
    /// Smart framing: ask Antigravity CLI about clips the face detector is unsure of.
    pub vision_check: bool,
    pub caption_position: crate::captions::Position,
    /// Caption size multiplier, 0.6–1.6.
    pub caption_size: f64,
    /// Short fade in from and out to black (video and sound).
    pub fade: bool,
    /// 9:16: push in on the strong lines.
    pub smart_zoom: bool,
    /// 9:16: stacked panels the other way up (the action above the face, the right person on top).
    pub swap_stack: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            format: Format::Vertical,
            cut_dead_air: true,
            captions: true,
            caption_style: "pop".into(),
            title: false,
            speed: 1.0,
            framing: Framing::default(),
            vision_check: false,
            caption_position: crate::captions::Position::Auto,
            caption_size: 1.0,
            fade: false,
            smart_zoom: true,
            swap_stack: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderedClip {
    pub rank: usize,
    pub title: String,
    pub video: String,
    pub srt: String,
    pub premiere_xml: String,
    pub duration_s: f64,
    /// Dead air removed, seconds.
    pub removed_s: f64,
    pub encoder: String,
}

/// Source-time pieces kept, and the words moved onto the output timeline.
fn plan_cuts(words: &[Word], start: f64, end: f64, cut_dead_air: bool) -> (Vec<(f64, f64)>, Vec<Word>) {
    let window_ms = ((end - start) * 1000.0).round() as i64;
    let inside: Vec<&Word> = words.iter().filter(|w| w.end > start && w.start < end).collect();
    let keeps_ms = if cut_dead_air {
        let ww: Vec<WindowWord> = inside
            .iter()
            .map(|w| WindowWord {
                text: w.text.clone(),
                start_ms: (((w.start - start) * 1000.0).round() as i64).max(0),
                end_ms: (((w.end - start) * 1000.0).round() as i64).min(window_ms),
            })
            .collect();
        // Streams are webcam + screen: silence up to 2 s is usually something happening on screen.
        pacing::keep_intervals(&ww, window_ms, pacing::MAX_PAUSE_SCREEN_CAM_MS, &[], false)
    } else {
        vec![(0, window_ms)]
    };
    let keeps: Vec<(f64, f64)> =
        keeps_ms.iter().map(|&(a, b)| (start + a as f64 / 1000.0, start + b as f64 / 1000.0)).collect();

    // Map each kept word onto the output timeline; words whose middle was cut (fillers) disappear.
    let mut out = Vec::new();
    for w in inside {
        let mid = (w.start + w.end) / 2.0;
        let mut offset = 0.0;
        for &(a, b) in &keeps {
            if mid >= a && mid < b {
                let s = offset + (w.start.max(a) - a);
                let e = offset + (w.end.min(b) - a);
                out.push(Word { text: w.text.clone(), start: s, end: e.max(s + 0.05) });
                break;
            }
            offset += b - a;
        }
    }
    (keeps, out)
}

fn encoder_args(encoder: &str) -> Vec<&'static str> {
    match encoder {
        "h264_nvenc" => vec!["-c:v", "h264_nvenc", "-preset", "p5", "-rc", "vbr", "-cq", "21", "-b:v", "0"],
        "h264_qsv" => vec!["-c:v", "h264_qsv", "-global_quality", "22"],
        "h264_amf" => vec!["-c:v", "h264_amf", "-quality", "quality", "-rc", "qvbr", "-qvbr_quality_level", "22"],
        "h264_mf" => vec!["-c:v", "h264_mf", "-b:v", "10M"],
        "h264_videotoolbox" => vec!["-c:v", "h264_videotoolbox", "-allow_sw", "1", "-b:v", "10M"],
        _ => vec!["-c:v", "libx264", "-preset", "veryfast", "-crf", "20"],
    }
}

/// A kept part of the source: (piece index, start, end) in source seconds. Pieces are the clip's segments.
pub type Keep = (usize, f64, f64);

/// `plan_cuts` over each piece in turn: the kept parts in output order, and the words on the output timeline.
fn plan_pieces(words: &[Word], pieces: &[(f64, f64)], cut_dead_air: bool) -> (Vec<Keep>, Vec<Word>) {
    let (mut keeps, mut out, mut offset) = (Vec::new(), Vec::new(), 0.0);
    for (i, &(start, end)) in pieces.iter().enumerate() {
        let (k, w) = plan_cuts(words, start, end, cut_dead_air);
        out.extend(w.into_iter().map(|w| Word { start: w.start + offset, end: w.end + offset, ..w }));
        offset += k.iter().map(|(a, b)| b - a).sum::<f64>();
        keeps.extend(k.into_iter().map(|(a, b)| (i, a, b)));
    }
    (keeps, out)
}

/// Source time in `piece` → time in the cut clip (before speed), or `None` if that moment was cut.
fn to_clip_time(keeps: &[Keep], piece: usize, t: f64) -> Option<f64> {
    let mut offset = 0.0;
    for &(i, a, b) in keeps {
        if i == piece && (a..b).contains(&t) {
            return Some(offset + t - a);
        }
        offset += b - a;
    }
    None
}

/// Clip time of the first kept moment of `piece` after source time `t` (a moment that was cut out).
fn next_kept(keeps: &[Keep], piece: usize, t: f64) -> Option<f64> {
    let mut offset = 0.0;
    for &(i, a, b) in keeps {
        if i == piece && a >= t {
            return Some(offset);
        }
        offset += b - a;
    }
    None
}

/// `[vcat] → [vf]`: 9:16 framed by `shots`, or 16:9 letterboxed.
fn framing_chain(format: Format, shots: &[Shot]) -> String {
    match format {
        Format::Vertical => crate::framing::vertical_chain(shots, ("vcat", "vf")),
        Format::Horizontal => {
            let (w, h) = format.size();
            format!(
                "[vcat]scale={w}:{h}:force_original_aspect_ratio=decrease:flags=lanczos,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,format=yuv420p[vf];"
            )
        }
    }
}

/// The ffmpeg filter graph: cut pieces → concat → framing (`frame_chain`) → speed → captions; audio fades at
/// each join + loudness.
/// Fade length in seconds, when fades are on.
const FADE_S: f64 = 0.4;

/// Input `n` is piece `n`, seeked to `seeks[n]`. `fade_over`: the output length, to fade in and out over it.
fn filter_graph(
    keeps: &[Keep],
    seeks: &[f64],
    frame_chain: &str,
    speed: f64,
    ass_file: Option<&str>,
    fade_over: Option<f64>,
) -> String {
    let mut g = String::new();
    for (i, &(n, a, b)) in keeps.iter().enumerate() {
        let (a, b) = (a - seeks[n], b - seeks[n]);
        let fade_out = (b - a - 0.012).max(0.0);
        g.push_str(&format!(
            "[{n}:v]trim=start={a:.3}:end={b:.3},setpts=PTS-STARTPTS[v{i}];\
             [{n}:a]atrim=start={a:.3}:end={b:.3},asetpts=PTS-STARTPTS,afade=t=in:d=0.012,afade=t=out:st={fade_out:.3}:d=0.012[a{i}];"
        ));
    }
    for i in 0..keeps.len() {
        g.push_str(&format!("[v{i}][a{i}]"));
    }
    g.push_str(&format!("concat=n={}:v=1:a=1[vcat][acat];", keeps.len()));
    // Framing runs on the cut clip's timeline (its crop path is keyed to it), before the speed change.
    g.push_str(frame_chain);
    // atempo keeps the voice's pitch; 1–2× is inside its single-filter range.
    if (speed - 1.0).abs() > 1e-3 {
        g.push_str(&format!("[vf]setpts=PTS/{speed}[vl];[acat]atempo={speed}[ac];"));
    } else {
        g.push_str("[vf]null[vl];[acat]anull[ac];");
    }
    match ass_file {
        // Relative file name (ffmpeg runs in the clip folder) keeps drive colons out of the graph.
        // shaping=complex is required for Devanagari conjuncts (M0 spike).
        Some(f) => g.push_str(&format!("[vl]ass=filename={f}:fontsdir={}:shaping=complex[vo];", fonts_dir())),
        None => g.push_str("[vl]null[vo];"),
    }
    let (vfade, afade) = match fade_over {
        Some(len) if len > 3.0 * FADE_S => {
            let out = len - FADE_S;
            (
                format!(",fade=t=in:d={FADE_S},fade=t=out:st={out:.3}:d={FADE_S}"),
                format!(",afade=t=in:d={FADE_S},afade=t=out:st={out:.3}:d={FADE_S}"),
            )
        }
        _ => (String::new(), String::new()),
    };
    g.push_str(&format!("[vo]null{vfade}[vout];"));
    // loudnorm alone overshoots on short clips (`clip eval` measured peaks above 0 dBFS): a limiter holds them.
    g.push_str(&format!(
        "[ac]loudnorm=I=-14:TP=-1.5:LRA=11,aresample=48000,alimiter=limit=0.82:level=false{afade}[aout]"
    ));
    g
}

fn fonts_dir() -> &'static str {
    if cfg!(windows) {
        "C\\\\:/Windows/Fonts"
    } else if cfg!(target_os = "macos") {
        "/System/Library/Fonts"
    } else {
        "/usr/share/fonts"
    }
}

/// `file://localhost/C:/path%20with%20spaces/x.mp4` for FCP 7 XML.
fn file_url(path: &Path) -> String {
    let p = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://localhost/");
    for b in p.trim_start_matches('/').bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// FCP 7 XML (xmeml v5) sequence: the kept pieces as jump cuts on the original media, scaled to fit.
// ponytail: Motion/Crop effect names are the M0 Premiere test; until it runs, framing is a plain scale.
fn premiere_xml(run: &Run, title: &str, keeps: &[(f64, f64)], format: Format) -> String {
    let fps = run.media_info.fps.unwrap_or(30.0);
    let timebase = fps.round().max(1.0) as i64;
    let ntsc = if (fps - fps.round()).abs() > 0.01 { "TRUE" } else { "FALSE" };
    let rate = format!("<rate><timebase>{timebase}</timebase><ntsc>{ntsc}</ntsc></rate>");
    let frames = |t: f64| (t * fps).round() as i64;
    let media_frames = frames(run.media_info.duration_s.unwrap_or(0.0));
    let (src_w, src_h) = (run.media_info.width.unwrap_or(1920), run.media_info.height.unwrap_or(1080));
    let (w, h) = format.size();
    let scale = (w as f64 / src_w as f64).min(h as f64 / src_h as f64) * 100.0;
    let name = xml_escape(Path::new(&run.media).file_name().map(|n| n.to_string_lossy()).unwrap_or_default().as_ref());
    let file = format!(
        "<file id=\"file-1\"><name>{name}</name><pathurl>{}</pathurl>{rate}<duration>{media_frames}</duration>\
         <media><video><samplecharacteristics><width>{src_w}</width><height>{src_h}</height></samplecharacteristics></video><audio><channelcount>2</channelcount></audio></media></file>",
        xml_escape(&file_url(Path::new(&run.media)))
    );
    let motion = format!(
        "<filter><effect><name>Basic Motion</name><effectid>basic</effectid><effectcategory>motion</effectcategory><effecttype>motion</effecttype><mediatype>video</mediatype>\
         <parameter><parameterid>scale</parameterid><name>Scale</name><valuemin>0</valuemin><valuemax>1000</valuemax><value>{scale:.2}</value></parameter></effect></filter>"
    );
    let (mut video, mut audio, mut pos) = (String::new(), String::new(), 0i64);
    for (i, &(a, b)) in keeps.iter().enumerate() {
        let (inp, out) = (frames(a), frames(b));
        let len = out - inp;
        let f = if i == 0 { file.clone() } else { "<file id=\"file-1\"/>".to_string() };
        let item = |kind: &str, body: &str| {
            format!(
                "<clipitem id=\"{kind}-{i}\"><name>{name}</name><duration>{media_frames}</duration>{rate}<start>{pos}</start><end>{}</end><in>{inp}</in><out>{out}</out>{body}</clipitem>",
                pos + len
            )
        };
        video.push_str(&item("v", &format!("{f}{motion}")));
        audio.push_str(&item("a", "<file id=\"file-1\"/>"));
        pos += len;
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE xmeml>\n<xmeml version=\"5\"><sequence id=\"sequence-1\"><name>{}</name><duration>{pos}</duration>{rate}\
         <media><video><format><samplecharacteristics>{rate}<width>{w}</width><height>{h}</height><pixelaspectratio>square</pixelaspectratio><fielddominance>none</fielddominance></samplecharacteristics></format>\
         <track>{video}</track></video><audio><track>{audio}</track></audio></media></sequence></xmeml>\n",
        xml_escape(title)
    )
}

/// Run ffmpeg with `-progress`, reporting the share of `duration` encoded.
async fn run_ffmpeg(args: &[String], cwd: &Path, duration: f64, stage: &str, progress: ProgressFn<'_>) -> Result<()> {
    let mut child = Command::new(crate::tools::resolve("ffmpeg"))
        .args(["-hide_banner", "-v", "error", "-y", "-nostats", "-progress", "pipe:1"])
        .args(args)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("could not start ffmpeg")?;
    let mut lines = BufReader::new(child.stdout.take().expect("piped")).lines();
    while let Some(line) = lines.next_line().await? {
        if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.parse::<f64>().ok()) {
            let done = ((us / 1e6) / duration.max(0.1) * 100.0).clamp(0.0, 100.0) as u64;
            progress(Progress { stage: stage.to_string(), done, total: Some(100) });
        }
    }
    let out = child.wait_with_output().await?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("render failed: {}", err.lines().find(|l| !l.trim().is_empty()).unwrap_or("unknown ffmpeg error"));
    }
    Ok(())
}

/// What framing works from, on a clip's timeline.
#[derive(Debug, Clone, Default)]
pub struct Analysis {
    pub faces: Vec<Face>,
    pub motion: Vec<Motion>,
    pub cuts: Vec<f64>,
}

/// Bump when detection changes, so old scans are redone.
const SCAN_VERSION: u32 = 1;

/// Faces, motion and cuts of a stretch of the source, in source seconds. Saved with the run: a shot's tracking stays
/// fixed once found, so opening a clip or exporting it again never scans again.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Scanned {
    faces: Vec<Face>,
    motion: Vec<Motion>,
    cuts: Vec<f64>,
}

/// `v1_12000_45500.json` → (12.0, 45.5), for this `SCAN_VERSION`.
fn scan_range(file: &str) -> Option<(f64, f64)> {
    let mut parts = file.strip_suffix(".json")?.split('_');
    (parts.next()? == format!("v{SCAN_VERSION}")).then_some(())?;
    Some((parts.next()?.parse::<f64>().ok()? / 1000.0, parts.next()?.parse::<f64>().ok()? / 1000.0))
}

/// The saved scan holding `start..end`, cut down to it. A trim inside a scanned part finds it here.
fn saved_scan(dir: &Path, start: f64, end: f64) -> Option<Scanned> {
    let file = std::fs::read_dir(dir).ok()?.flatten().find(|f| {
        scan_range(&f.file_name().to_string_lossy()).is_some_and(|(a, b)| a <= start + 0.01 && b >= end - 0.01)
    })?;
    let mut s: Scanned = serde_json::from_slice(&std::fs::read(file.path()).ok()?).ok()?;
    s.faces.retain(|f| (start..end).contains(&f.t));
    s.motion.retain(|m| (start..end).contains(&m.t));
    s.cuts.retain(|t| (start..end).contains(t));
    Some(s)
}

/// Faces, motion and camera cuts on the timeline of `keeps` (the kept parts of `pieces`, in order): what falls in
/// cut-out moments is dropped, and each join of two pieces is a cut.
pub async fn analyse(run: &Run, pieces: &[(f64, f64)], keeps: &[Keep], progress: ProgressFn<'_>) -> Result<Analysis> {
    let src = (run.media_info.width.unwrap_or(1920) as u32, run.media_info.height.unwrap_or(1080) as u32);
    let dir = Path::new(&run.dir).join("scans");
    let mut out = Analysis::default();
    for (i, &(start, end)) in pieces.iter().enumerate() {
        let found = match saved_scan(&dir, start, end) {
            Some(s) => s,
            None => {
                let model = crate::framing::model(progress).await?;
                let path = std::path::PathBuf::from(&run.media);
                let found = tokio::task::spawn_blocking(move || {
                    let scan = crate::framing::scan(&model, &path, start, end, src)?;
                    let cuts = crate::framing::scene_cuts(&path, start, end);
                    anyhow::Ok(Scanned { faces: scan.faces, motion: scan.motion, cuts })
                })
                .await
                .context("face detection stopped")??;
                std::fs::create_dir_all(&dir)?;
                let name = format!("v{SCAN_VERSION}_{}_{}.json", (start * 1000.0).floor(), (end * 1000.0).ceil());
                std::fs::write(dir.join(name), serde_json::to_vec(&found)?)?;
                found
            }
        };
        out.faces.extend(found.faces.into_iter().filter_map(|f| Some(Face { t: to_clip_time(keeps, i, f.t)?, ..f })));
        out.motion
            .extend(found.motion.into_iter().filter_map(|m| Some(Motion { t: to_clip_time(keeps, i, m.t)?, ..m })));
        // A cut inside a removed pause still changes the picture: at the next kept frame.
        out.cuts.extend(
            found.cuts.into_iter().filter_map(|t| to_clip_time(keeps, i, t).or_else(|| next_kept(keeps, i, t))),
        );
        if i > 0
            && let Some(&(_, a, _)) = keeps.iter().find(|k| k.0 == i)
        {
            out.cuts.extend(to_clip_time(keeps, i, a));
        }
    }
    out.cuts.sort_by(f64::total_cmp);
    Ok(out)
}

/// The clip's chosen shot layouts on the timeline of `keeps`.
fn shot_choices(c: &Candidate, keeps: &[Keep]) -> Vec<(f64, Layout)> {
    let pieces = c.pieces();
    c.layouts
        .iter()
        .filter_map(|&(t, l)| {
            let i = pieces.iter().position(|p| (p.0..p.1).contains(&t))?;
            Some((to_clip_time(keeps, i, t).or_else(|| next_kept(keeps, i, t))?, l))
        })
        .collect()
}

/// AI vision's layout for a frame from the middle of the clip's first part.
async fn vision_check(media: &str, dir: &Path, rank: usize, pieces: &[(f64, f64)]) -> Option<Layout> {
    let frame = dir.join(format!("clip_{rank:02}.vision.jpg"));
    let mid = format!("{:.3}", (pieces[0].0 + pieces[0].1) / 2.0);
    let grabbed = Command::new(crate::tools::resolve("ffmpeg"))
        .args(["-hide_banner", "-v", "error", "-y", "-ss", &mid, "-i", media, "-frames:v", "1"])
        .args(["-vf", "scale=960:-2"])
        .arg(&frame)
        .status()
        .await
        .is_ok_and(|s| s.success());
    let seen = if grabbed { crate::framing::vision_layout(&frame).await } else { None };
    std::fs::remove_file(&frame).ok();
    seen
}

/// How a clip would be framed in 9:16, on the editor's clock (its parts back to back, no dead-air cuts): the
/// editor draws the crop boxes on the source and previews the vertical frame from it. Uncut parts are what the
/// editor plays, so its boxes line up with what it shows; the export plans again on the cut timeline.
pub async fn preview_plan(
    run_id: &str,
    rank: usize,
    opts: &RenderOptions,
    progress: ProgressFn<'_>,
) -> Result<Vec<Shot>> {
    let (run, words) = pipeline::load_run(run_id)?;
    let c = run.candidates.iter().find(|c| c.rank == rank).with_context(|| format!("no clip #{rank} in this run"))?;
    let pieces = c.pieces();
    let keeps: Vec<Keep> = pieces.iter().enumerate().map(|(i, &(a, b))| (i, a, b)).collect();
    let length = pieces.iter().map(|(a, b)| b - a).sum::<f64>();
    let src = (run.media_info.width.unwrap_or(1920) as u32, run.media_info.height.unwrap_or(1080) as u32);
    let scan = analyse(&run, &pieces, &keeps, progress).await?;
    let (_, clip_words) = plan_pieces(&words, &pieces, false);
    let overrides = shot_choices(c, &keeps);
    Ok(crate::framing::plan(&Request {
        framing: opts.framing,
        faces: &scan.faces,
        motion: &scan.motion,
        cuts: &scan.cuts,
        words: &clip_words,
        duration: length,
        src,
        force: None,
        overrides: &overrides,
        swap: opts.swap_stack,
        smart_zoom: opts.smart_zoom,
    }))
}

/// Where a render keeps its plan, in the run's folder.
pub fn plan_file(run: &Run, stem: &str) -> std::path::PathBuf {
    Path::new(&run.dir).join("plans").join(format!("{stem}.json"))
}

/// Options of the run's last export (defaults before the first).
pub fn last_options(run: &Run) -> RenderOptions {
    std::fs::read(Path::new(&run.dir).join("render.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Render the given candidates (1-based ranks) of a run into its clips folder.
pub async fn render_clips(
    run_id: &str,
    ranks: &[usize],
    opts: &RenderOptions,
    progress: ProgressFn<'_>,
) -> Result<Vec<RenderedClip>> {
    let (run, words) = pipeline::load_run(run_id)?;
    let out_dir = run.clips_dir();
    std::fs::create_dir_all(&out_dir).with_context(|| format!("cannot create {}", out_dir.display()))?;
    // The editor re-exports a clip with the options the job used.
    std::fs::write(Path::new(&run.dir).join("render.json"), serde_json::to_vec_pretty(opts)?)?;
    progress(Progress { stage: "Choosing video encoder".into(), done: 0, total: None });
    let encoder = crate::system::render_encoder().await;
    let (w, h) = opts.format.size();
    let speed = opts.speed.clamp(1.0, 2.0);

    let mut done = Vec::new();
    for &rank in ranks {
        let c = run
            .candidates
            .iter()
            .find(|c| c.rank == rank)
            .with_context(|| format!("no candidate #{rank} in this run"))?;
        let pieces = c.pieces();
        let (keeps, mut clip_words) = plan_pieces(&words, &pieces, opts.cut_dead_air);
        let flat: Vec<(f64, f64)> = keeps.iter().map(|&(_, a, b)| (a, b)).collect();
        let duration = flat.iter().map(|(a, b)| b - a).sum::<f64>() / speed;
        let unsped_words = clip_words.clone();
        let src = (run.media_info.width.unwrap_or(1920) as u32, run.media_info.height.unwrap_or(1080) as u32);
        let shots = if opts.format == Format::Vertical {
            progress(Progress { stage: format!("Framing clip #{rank}"), done: 0, total: None });
            let scan = if opts.framing != Framing::Classic || !c.layouts.is_empty() {
                analyse(&run, &pieces, &keeps, progress).await?
            } else {
                Analysis::default()
            };
            let mut force = None;
            if opts.vision_check && crate::framing::unsure(opts.framing, &scan.faces, duration * speed) {
                progress(Progress { stage: format!("Checking clip #{rank} with AI vision"), done: 0, total: None });
                force = vision_check(&run.media, &out_dir, rank, &pieces).await;
            }
            let overrides = shot_choices(c, &keeps);
            crate::framing::plan(&Request {
                framing: opts.framing,
                faces: &scan.faces,
                motion: &scan.motion,
                cuts: &scan.cuts,
                words: &unsped_words,
                duration: duration * speed,
                src,
                force,
                overrides: &overrides,
                swap: opts.swap_stack,
                smart_zoom: opts.smart_zoom,
            })
        } else {
            Vec::new()
        };
        for w in &mut clip_words {
            // The last word's estimated end can run past the clip (`clip eval` caught SRT cues after the end).
            (w.start, w.end) = (w.start / speed, (w.end / speed).min(duration));
        }
        let stem = format!("clip_{rank:02}_{}", if opts.format == Format::Vertical { "9x16" } else { "16x9" });

        // For `clip eval`: the framing and how many words the captions should hold.
        let saved = serde_json::json!({
            "speed": speed, "duration": duration, "words": clip_words.len(), "src": src, "shots": shots
        });
        let plan_path = plan_file(&run, &stem);
        std::fs::create_dir_all(plan_path.parent().expect("has a folder"))?;
        std::fs::write(&plan_path, serde_json::to_vec(&saved)?)?;

        let srt_path = out_dir.join(format!("{stem}.srt"));
        std::fs::write(&srt_path, crate::captions::srt(&clip_words))?;
        let ass_name = format!("{stem}.ass");
        let overlay = opts.captions || opts.title;
        if overlay {
            let shown = if opts.captions { clip_words.as_slice() } else { &[] };
            let title = opts.title.then_some((c.title.as_str(), duration));
            let style = crate::captions::preset(&opts.caption_style);
            let place = crate::captions::Placement { position: opts.caption_position, scale: opts.caption_size };
            let anchors: Vec<(f64, f64, u32)> =
                shots.iter().map(|s| (s.start / speed, s.end / speed, s.caption_y)).collect();
            std::fs::write(out_dir.join(&ass_name), crate::captions::ass(shown, w, h, style, place, title, &anchors))?;
        }
        // ponytail: the Premiere timeline stays at 1× (no time remap yet); speed is in the MP4 only.
        let xml_path = out_dir.join(format!("{stem}.xml"));
        std::fs::write(&xml_path, premiere_xml(&run, &c.title, &flat, opts.format))?;

        let frame_chain = framing_chain(opts.format, &shots);

        // One input per piece, seeked (fast) to its start; the graph trims relative to it.
        let seeks: Vec<f64> = pieces.iter().map(|p| p.0).collect();
        let graph_name = format!("{stem}.graph.txt");
        std::fs::write(
            out_dir.join(&graph_name),
            filter_graph(
                &keeps,
                &seeks,
                &frame_chain,
                speed,
                overlay.then_some(ass_name.as_str()),
                opts.fade.then_some(duration),
            ),
        )?;
        let video_name = format!("{stem}.mp4");
        let mut args: Vec<String> = Vec::new();
        for &(a, b) in &pieces {
            args.extend(["-ss".into(), format!("{a:.3}"), "-to".into(), format!("{:.3}", b + 0.5), "-i".into()]);
            args.push(run.media.clone());
        }
        args.extend([
            "-/filter_complex".into(),
            graph_name.clone(),
            "-map".into(),
            "[vout]".into(),
            "-map".into(),
            "[aout]".into(),
        ]);
        args.extend(encoder_args(encoder).into_iter().map(String::from));
        args.extend(["-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart"].map(String::from));
        args.push(video_name.clone());
        let stage = format!("Rendering clip #{rank} ({encoder})");
        run_ffmpeg(&args, &out_dir, duration, &stage, progress).await?;
        std::fs::remove_file(out_dir.join(&graph_name)).ok();
        std::fs::remove_file(out_dir.join(&ass_name)).ok();

        done.push(RenderedClip {
            rank,
            title: c.title.clone(),
            video: out_dir.join(&video_name).to_string_lossy().into_owned(),
            srt: srt_path.to_string_lossy().into_owned(),
            premiere_xml: xml_path.to_string_lossy().into_owned(),
            duration_s: duration,
            removed_s: pieces.iter().map(|(a, b)| b - a).sum::<f64>() - duration * speed,
            encoder: encoder.to_string(),
        });
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(t: &str, s: f64, e: f64) -> Word {
        Word { text: t.into(), start: s, end: e }
    }

    #[test]
    fn cuts_long_pause_and_shifts_words() {
        let words = [
            w("before", 99.0, 99.5),
            w("hello", 100.0, 100.5),
            w("um", 100.7, 101.0),
            w("there", 104.0, 104.5),
            w("after", 110.0, 110.5),
        ];
        let (keeps, out) = plan_cuts(&words, 100.0, 105.0, true);
        assert!(keeps.len() >= 2, "3 s gap must be cut: {keeps:?}");
        let texts: Vec<_> = out.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(texts, ["hello", "there"], "filler gone, outside words ignored");
        let kept: f64 = keeps.iter().map(|(a, b)| b - a).sum();
        assert!(out[1].end <= kept + 1e-6 && out[1].start < 3.0, "{out:?} in {kept}");
    }

    #[test]
    fn no_cuts_when_disabled() {
        let (keeps, out) = plan_cuts(&[w("hi", 1.0, 1.2)], 0.0, 10.0, false);
        assert_eq!(keeps, vec![(0.0, 10.0)]);
        assert!((out[0].start - 1.0).abs() < 1e-9);
    }

    #[test]
    fn graph_has_one_trim_pair_per_piece_and_escaped_fonts() {
        let classic = framing_chain(Format::Vertical, &[]);
        let g = filter_graph(&[(0, 10.0, 12.0), (0, 13.0, 15.5)], &[10.0], &classic, 1.0, Some("c.ass"), None);
        assert_eq!(g.matches("[0:v]trim").count(), 2);
        assert!(g.contains("trim=start=3.000:end=5.500") && g.contains("concat=n=2:v=1:a=1"));
        assert!(g.contains("[vcat]split=2") && g.contains("[vf];[vf]null[vl]"), "framing then speed: {g}");
        assert!(g.contains("shaping=complex") && g.ends_with("[aout]") && !g.contains("atempo"));
        let wide = framing_chain(Format::Horizontal, &[]);
        let fast = filter_graph(&[(0, 0.0, 5.0)], &[0.0], &wide, 1.5, None, None);
        assert!(fast.contains("setpts=PTS/1.5[vl]") && fast.contains("atempo=1.5[ac]"), "{fast}");
        assert!(!fast.contains("d=0.4"), "no fades unless asked (the 12 ms de-click fades stay)");
        let faded = filter_graph(&[(0, 0.0, 5.0)], &[0.0], &wide, 1.0, None, Some(5.0));
        let stitched = filter_graph(&[(0, 50.0, 52.0), (1, 10.0, 12.0)], &[50.0, 10.0], &wide, 1.0, None, None);
        assert!(stitched.contains("[1:v]trim=start=0.000:end=2.000") && stitched.contains("[0:a]atrim=start=0.000"));
        assert!(faded.contains("fade=t=out:st=4.600:d=0.4[vout]") && faded.contains("afade=t=in:d=0.4"), "{faded}");
    }

    #[test]
    fn a_saved_scan_answers_for_any_stretch_inside_it() {
        assert_eq!(scan_range(&format!("v{SCAN_VERSION}_12000_45500.json")), Some((12.0, 45.5)));
        assert_eq!(scan_range("v0_12000_45500.json"), None, "older detection");
        let dir = crate::transcribe::Workspace::new().unwrap();
        let scan = Scanned {
            faces: vec![Face { t: 13.0, ..Default::default() }, Face { t: 30.0, ..Default::default() }],
            motion: Vec::new(),
            cuts: vec![14.0, 31.0],
        };
        std::fs::write(dir.0.join(format!("v{SCAN_VERSION}_12000_45500.json")), serde_json::to_vec(&scan).unwrap())
            .unwrap();
        let part = saved_scan(&dir.0, 12.5, 20.0).expect("inside");
        assert_eq!((part.faces.len(), part.cuts), (1, vec![14.0]));
        assert!(saved_scan(&dir.0, 40.0, 50.0).is_none(), "reaches outside what was scanned");
    }

    #[test]
    fn source_times_map_onto_the_cut_clip() {
        let keeps = [(0, 10.0, 12.0), (0, 15.0, 16.0), (1, 2.0, 4.0)];
        assert_eq!(to_clip_time(&keeps, 0, 11.0), Some(1.0));
        assert_eq!(to_clip_time(&keeps, 0, 13.0), None, "cut out");
        assert_eq!(to_clip_time(&keeps, 0, 15.5), Some(2.5));
        assert_eq!(to_clip_time(&keeps, 1, 3.0), Some(4.0), "second piece follows the first");
        assert_eq!(to_clip_time(&keeps, 0, 3.0), None, "only its own piece");
        assert_eq!(next_kept(&keeps, 0, 13.0), Some(2.0), "a cut in a removed pause lands where the picture resumes");
        assert_eq!(next_kept(&keeps, 1, 5.0), None);
    }

    #[test]
    fn stitched_pieces_play_in_order() {
        let words = [w("late", 50.5, 51.0), w("early", 10.5, 11.0)];
        let (keeps, out) = plan_pieces(&words, &[(50.0, 52.0), (10.0, 12.0)], false);
        assert_eq!(keeps, vec![(0, 50.0, 52.0), (1, 10.0, 12.0)]);
        let got: Vec<_> = out.iter().map(|x| (x.text.as_str(), x.start)).collect();
        assert_eq!(got, [("late", 0.5), ("early", 2.5)]);
    }

    #[test]
    fn file_urls_are_percent_encoded() {
        assert_eq!(file_url(Path::new(r"C:\a b\x#.mp4")), "file://localhost/C:/a%20b/x%23.mp4");
    }
}
