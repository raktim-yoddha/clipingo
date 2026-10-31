//! 9:16 framing: a set of rules applied shot by shot (scene cuts from ffmpeg), the same plan driving the export and
//! the editor's preview.
//!
//! **Smart** picks per shot: one person → a window following them, a little closer on a small face; two people apart
//! → one above the other; two close together → one window holding both; three or more → a wide band over a blurred
//! fill; a webcam that stays put for most of the clip → **Face + content**, the webcam and the action on screen
//! stacked (either way round); nobody → the whole frame. **Follow** always follows the speaker, **Classic** is the
//! whole frame over a blur. Each shot can be overridden, and it reports which layouts it can support.
//!
//! The camera starts on the subject at every cut. Faces come from YuNet (OpenCV Zoo, 2023mar) on ONNX Runtime at
//! `camera::ANALYSIS_FPS`: the whole frame first, then overlapping tiles when it found no big face, so people in a
//! wide shot are not missed. Landmarks give a mouth-movement score (who is talking); frame differences give a motion
//! map (where the action is). Rules follow Clippo's reframing.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::camera;
use crate::models::{self, ProgressFn};
use crate::zoom::{self, Zoom};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Framing {
    /// The rules: follow, two people, face + content, wide band or whole frame, shot by shot.
    #[default]
    Smart,
    /// Always follow the speaker.
    Full,
    /// Whole frame, blurred fill.
    Classic,
}

const MODEL_FILE: &str = "face_detection_yunet_2023mar.onnx";
const MODEL_URL: &str = "https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models/face_detection_yunet/face_detection_yunet_2023mar.onnx";
/// YuNet 2023mar's fixed input size.
const SIDE: usize = 640;
const MIN_SCORE: f32 = 0.6;
/// A frame with a face this tall needs no tile scan.
const BIG_FACE_H: f32 = 0.2;
/// Output frame.
const OUT: (u32, u32) = (1080, 1920);
/// Motion map cells.
pub const GRID: (usize, usize) = (32, 18);

/// A face in one sampled frame: time, centre and height as shares of the source frame, and how much its mouth moved
/// against the rest of the face since the previous sample (0–255, noisy for one frame, telling over a shot).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Face {
    pub t: f64,
    pub x: f64,
    pub y: f64,
    pub h: f64,
    pub talk: f64,
}

/// Mean frame difference per `GRID` cell (row-major) at time `t`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Motion {
    pub t: f64,
    pub cells: Vec<f32>,
}

/// What `scan` found in a stretch of video.
#[derive(Debug, Clone, Default)]
pub struct Scan {
    pub faces: Vec<Face>,
    pub motion: Vec<Motion>,
}

/// The face model, downloaded on first use (230 KB).
pub async fn model(progress: ProgressFn<'_>) -> Result<PathBuf> {
    models::download(MODEL_URL, &models::models_dir().join(MODEL_FILE), "face detector", progress).await
}

/// A face box with its mouth and eye-line, in the pixels of whatever image it was found in.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Detection {
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    score: f32,
    /// Mouth centre and half width; eye-line centre.
    mouth: (f32, f32, f32),
    eyes: (f32, f32),
}

impl Detection {
    /// The same face after scaling by `k` and moving by (`dx`, `dy`).
    fn mapped(self, k: f32, dx: f32, dy: f32) -> Self {
        let (m, e) = (self.mouth, self.eyes);
        Self {
            cx: self.cx * k + dx,
            cy: self.cy * k + dy,
            w: self.w * k,
            h: self.h * k,
            score: self.score,
            mouth: (m.0 * k + dx, m.1 * k + dy, m.2 * k),
            eyes: (e.0 * k + dx, e.1 * k + dy),
        }
    }
}

/// The same face twice: boxes overlapping much, or one mostly inside the other (a tile edge cuts a face short).
fn same_face(a: &Detection, b: &Detection) -> bool {
    let ix = ((a.cx + a.w / 2.0).min(b.cx + b.w / 2.0) - (a.cx - a.w / 2.0).max(b.cx - b.w / 2.0)).max(0.0);
    let iy = ((a.cy + a.h / 2.0).min(b.cy + b.h / 2.0) - (a.cy - a.h / 2.0).max(b.cy - b.h / 2.0)).max(0.0);
    let (inter, a_area, b_area) = (ix * iy, a.w * a.h, b.w * b.h);
    inter / (a_area + b_area - inter).max(1e-6) >= 0.3 || inter / a_area.min(b_area).max(1e-6) >= 0.6
}

/// Best first, overlapping boxes merged, at most 4 faces.
fn nms(mut all: Vec<Detection>) -> Vec<Detection> {
    all.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Detection> = Vec::new();
    for d in all {
        if kept.len() < 4 && !kept.iter().any(|k| same_face(k, &d)) {
            kept.push(d);
        }
    }
    kept
}

/// Raw YuNet outputs, per stride 8 / 16 / 32, laid out row-major over the stride grid.
struct Raw<'a> {
    cls: [&'a [f32]; 3],
    obj: [&'a [f32]; 3],
    bbox: [&'a [f32]; 3],
    kps: [&'a [f32]; 3],
}

/// Faces in YuNet's raw outputs scoring at least `min_score`, in input pixels.
fn faces_in(raw: &Raw<'_>, min_score: f32) -> Vec<Detection> {
    let mut all = Vec::new();
    for (k, stride) in [8usize, 16, 32].into_iter().enumerate() {
        let cols = SIDE / stride;
        for idx in 0..cols * cols {
            let score = (raw.cls[k][idx].clamp(0.0, 1.0) * raw.obj[k][idx].clamp(0.0, 1.0)).sqrt();
            if score >= min_score {
                let (r, c, b) = ((idx / cols) as f32, (idx % cols) as f32, &raw.bbox[k][idx * 4..idx * 4 + 4]);
                let s = stride as f32;
                // Landmarks: right eye, left eye, nose, right and left mouth corner.
                let kp = &raw.kps[k][idx * 10..idx * 10 + 10];
                let at = |n: usize| ((kp[2 * n] + c) * s, (kp[2 * n + 1] + r) * s);
                let (re, le, mr, ml) = (at(0), at(1), at(3), at(4));
                all.push(Detection {
                    cx: (c + b[0]) * s,
                    cy: (r + b[1]) * s,
                    w: b[2].exp() * s,
                    h: b[3].exp() * s,
                    score,
                    mouth: ((mr.0 + ml.0) / 2.0, (mr.1 + ml.1) / 2.0, ((ml.0 - mr.0).abs() / 2.0).max(1.0)),
                    eyes: ((re.0 + le.0) / 2.0, (re.1 + le.1) / 2.0),
                });
            }
        }
    }
    nms(all)
}

/// The size frames are decoded at: the short side is the model's input side, so tiles see full detail.
fn analysis_size(src: (u32, u32)) -> (usize, usize) {
    let (w, h) = (src.0.max(2) as f64, src.1.max(2) as f64);
    let s = SIDE as f64 / w.min(h);
    let even = |v: f64| ((v / 2.0).round() as usize * 2).max(SIDE);
    (even(w * s), even(h * s))
}

/// Top-left corners of overlapping `SIDE` squares along the long side (none for a nearly square frame).
fn tiles(fw: usize, fh: usize) -> Vec<(usize, usize)> {
    let (long, wide) = (fw.max(fh), fw >= fh);
    if (long as f64) < SIDE as f64 * 1.25 {
        return Vec::new();
    }
    (0..3).map(|i| i * (long - SIDE) / 2).map(|o| if wide { (o, 0) } else { (0, o) }).collect()
}

/// The whole frame (interleaved BGR) fitted into the model's square, padded at the right / bottom, as planar floats
/// (YuNet's own preprocessing). Returns the input and the frame pixels per input pixel.
fn whole_input(frame: &[u8], fw: usize, fh: usize) -> (Vec<f32>, f32) {
    let k = fw.max(fh) as f32 / SIDE as f32;
    let plane = SIDE * SIDE;
    let mut input = vec![0f32; plane * 3];
    for y in 0..((fh as f32 / k) as usize).min(SIDE) {
        for x in 0..((fw as f32 / k) as usize).min(SIDE) {
            // A 2×2 box, enough for the factor of about 1.8 of a 16:9 frame.
            let (sx, sy) = ((x as f32 * k) as usize, (y as f32 * k) as usize);
            let (x2, y2) = ((sx + 1).min(fw - 1), (sy + 1).min(fh - 1));
            for ch in 0..3 {
                let px = |a: usize, b: usize| frame[(b * fw + a) * 3 + ch] as f32;
                input[ch * plane + y * SIDE + x] = (px(sx, sy) + px(x2, sy) + px(sx, y2) + px(x2, y2)) / 4.0;
            }
        }
    }
    (input, k)
}

/// A `SIDE` square of the frame as planar floats.
fn tile_input(frame: &[u8], fw: usize, (ox, oy): (usize, usize)) -> Vec<f32> {
    let plane = SIDE * SIDE;
    let mut input = vec![0f32; plane * 3];
    for y in 0..SIDE {
        for x in 0..SIDE {
            let at = ((y + oy) * fw + x + ox) * 3;
            for ch in 0..3 {
                input[ch * plane + y * SIDE + x] = frame[at + ch] as f32;
            }
        }
    }
    input
}

/// Mean absolute difference of the green channel over a rectangle of two frames, every second pixel.
fn patch_diff(a: &[u8], b: &[u8], fw: usize, fh: usize, (x0, y0, x1, y1): (i64, i64, i64, i64)) -> f64 {
    let (x0, x1) = (x0.clamp(0, fw as i64 - 1) as usize, x1.clamp(1, fw as i64) as usize);
    let (y0, y1) = (y0.clamp(0, fh as i64 - 1) as usize, y1.clamp(1, fh as i64) as usize);
    let (mut sum, mut n) = (0u64, 0u64);
    for y in (y0..y1).step_by(2) {
        for x in (x0..x1).step_by(2) {
            let i = (y * fw + x) * 3 + 1;
            sum += (a[i] as i64 - b[i] as i64).unsigned_abs();
            n += 1;
        }
    }
    if n == 0 { 0.0 } else { sum as f64 / n as f64 }
}

/// Mouth movement above the head's own movement (the eye band), so nodding does not read as talking.
fn talk_score(d: &Detection, cur: &[u8], prev: &[u8], fw: usize, fh: usize) -> f64 {
    let (mx, my, mw) = d.mouth;
    let rect =
        |x: f32, y: f32, hw: f32, hh: f32| ((x - hw) as i64, (y - hh) as i64, (x + hw) as i64 + 2, (y + hh) as i64 + 2);
    let mouth = patch_diff(cur, prev, fw, fh, rect(mx, my, mw * 1.3, mw * 0.8));
    let eyes = patch_diff(cur, prev, fw, fh, rect(d.eyes.0, d.eyes.1, mw * 1.6, mw * 0.5));
    (mouth - eyes).max(0.0)
}

/// Mean frame difference per `GRID` cell, every fourth pixel.
fn motion_cells(a: &[u8], b: &[u8], fw: usize, fh: usize) -> Vec<f32> {
    let (gw, gh) = GRID;
    let (mut sum, mut n) = (vec![0f32; gw * gh], vec![0f32; gw * gh]);
    for y in (0..fh).step_by(4) {
        for x in (0..fw).step_by(4) {
            let i = (y * fw + x) * 3 + 1;
            let c = (y * gh / fh) * gw + x * gw / fw;
            sum[c] += (a[i] as f32 - b[i] as f32).abs();
            n[c] += 1.0;
        }
    }
    sum.iter().zip(&n).map(|(s, k)| s / k.max(1.0)).collect()
}

/// One inference of a planar square.
fn infer(session: &mut ort::session::Session, input: Vec<f32>) -> Result<Vec<Detection>> {
    let tensor = ort::value::Tensor::from_array(([1usize, 3, SIDE, SIDE], input))?;
    let out = session.run(ort::inputs!["input" => tensor])?;
    let get = |name: &str| out[name].try_extract_tensor::<f32>().map(|(_, d)| d);
    let raw = Raw {
        cls: [get("cls_8")?, get("cls_16")?, get("cls_32")?],
        obj: [get("obj_8")?, get("obj_16")?, get("obj_32")?],
        bbox: [get("bbox_8")?, get("bbox_16")?, get("bbox_32")?],
        kps: [get("kps_8")?, get("kps_16")?, get("kps_32")?],
    };
    Ok(faces_in(&raw, MIN_SCORE))
}

/// Faces (up to 4 per sampled frame, best first, so several share a `t`) and motion in `start..end` (seconds) of
/// `media`, whose frames are `src` pixels. Blocking: runs ffmpeg and the model.
pub fn scan(model: &Path, media: &Path, start: f64, end: f64, src: (u32, u32)) -> Result<Scan> {
    // ort's builder errors carry the builder (not Send), so they are flattened to text.
    let ort_err = |e: &dyn std::fmt::Display| anyhow::anyhow!("face detector: {e}");
    // ONNX Runtime allows one environment per process, ever (even after it is dropped): build it once, share it.
    static ENV: std::sync::OnceLock<Result<ort::environment::Environment, String>> = std::sync::OnceLock::new();
    let env = ENV.get_or_init(|| ort::init().build().map_err(|e| e.to_string())).as_ref().map_err(|e| ort_err(e))?;
    let builder = ort::session::Session::builder(env).map_err(|e| ort_err(&e))?;
    let mut builder = builder.with_intra_threads(4).map_err(|e| ort_err(&e))?;
    let mut session = builder.commit_from_file(model).map_err(|e| ort_err(&e))?;
    let (fw, fh) = analysis_size(src);
    let vf = format!("fps={},scale={fw}:{fh}:flags=bilinear", camera::ANALYSIS_FPS);
    let mut child = Command::new(crate::tools::resolve("ffmpeg"))
        .args(["-hide_banner", "-v", "error", "-ss", &format!("{start:.3}"), "-to", &format!("{end:.3}"), "-i"])
        .arg(media)
        .args(["-an", "-vf", &vf, "-pix_fmt", "bgr24", "-f", "rawvideo", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("could not start ffmpeg")?;
    let mut stdout = child.stdout.take().expect("piped");
    let squares = tiles(fw, fh);
    let mut frame = vec![0u8; fw * fh * 3];
    let mut prev: Option<Vec<u8>> = None;
    let mut out = Scan::default();
    let mut n = 0;
    while stdout.read_exact(&mut frame).is_ok() {
        let t = start + n as f64 / camera::ANALYSIS_FPS;
        let (input, k) = whole_input(&frame, fw, fh);
        let mut found: Vec<Detection> =
            infer(&mut session, input)?.into_iter().map(|d| d.mapped(k, 0.0, 0.0)).collect();
        if !found.iter().any(|d| d.h / fh as f32 >= BIG_FACE_H) {
            for &(ox, oy) in &squares {
                let hits = infer(&mut session, tile_input(&frame, fw, (ox, oy)))?;
                found.extend(hits.into_iter().map(|d| d.mapped(1.0, ox as f32, oy as f32)));
            }
            found = nms(found);
        }
        out.faces.extend(found.iter().map(|d| Face {
            t,
            x: d.cx as f64 / fw as f64,
            y: d.cy as f64 / fh as f64,
            h: d.h as f64 / fh as f64,
            talk: prev.as_deref().map_or(0.0, |p| talk_score(d, &frame, p, fw, fh)),
        }));
        if let Some(p) = &prev {
            out.motion.push(Motion { t, cells: motion_cells(&frame, p, fw, fh) });
        }
        prev = Some(frame.clone());
        n += 1;
    }
    child.wait()?;
    Ok(out)
}

/// Just the faces of `scan`.
pub fn detect(model: &Path, media: &Path, start: f64, end: f64, src: (u32, u32)) -> Result<Vec<Face>> {
    Ok(scan(model, media, start, end, src)?.faces)
}

/// Piecewise-linear ffmpeg expression of `t` through `(t, value)` keyframes.
fn lerp_expr(keys: &[(f64, f64)]) -> String {
    let Some(&(_, last)) = keys.last() else { return "0".into() };
    let mut expr = format!("{last:.1}");
    for pair in keys.windows(2).rev() {
        let ((t0, v0), (t1, v1)) = (pair[0], pair[1]);
        let span = (t1 - t0).max(1e-3);
        expr = format!("if(lt(t,{t1:.3}),{v0:.1}+({:.1})*(t-{t0:.3})/{span:.3},{expr})", v1 - v0);
    }
    expr
}

/// Whole frame over a blurred, darkened fill.
pub fn classic_chain(input: &str, output: &str) -> String {
    let (w, h) = OUT;
    format!(
        "[{input}]split=2[{output}bg][{output}fg];[{output}bg]scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},gblur=sigma=24,eq=brightness=-0.12[{output}bl];\
         [{output}fg]scale={w}:-2:flags=lanczos[{output}fs];[{output}bl][{output}fs]overlay=(W-w)/2:(H-h)/2,setsar=1,format=yuv420p[{output}];"
    )
}

/// What a 9:16 shot shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    /// A 9:16 window following one person (or two standing close).
    Follow,
    /// Face + content: a face and what is happening on screen, stacked.
    Split,
    /// Two people, one above the other (interviews, podcasts).
    Stack,
    /// Three or more people: a wide band over a blurred fill.
    Group,
    /// Whole frame over a blurred fill.
    Whole,
}

/// Seconds of a shot: shorter shots join the one before (flash cuts, cut-detector noise).
const MIN_SHOT_S: f64 = 1.2;
/// A face this small, in a corner, at the same spot for most of the clip is a webcam overlay. A small face that
/// comes and goes with the camera is a person in a wide shot (stage, podcast studio).
const WEBCAM_H: f64 = 0.17;
const WEBCAM_PRESENCE: f64 = 0.6;
/// Most a crop is enlarged: a small face in a low-resolution wide shot gets a wider crop instead of a blurry one.
const MAX_UPSCALE: f64 = 2.6;
/// Two faces are stacked rather than cut smaller, so a little softer is worth it there.
const MAX_UPSCALE_PANEL: f64 = 3.5;
/// Two people this far apart (share of the frame width) cannot share one 9:16 window.
const APART: f64 = 0.2;
/// Face + content: heights of the two panels, the content taking more.
const FACE_PANEL_H: u32 = 768;
/// Panels are 1080 wide; the seam between them gets a line this tall.
const DIVIDER_H: u32 = 6;
/// Lower third, clear of the platforms' buttons at the bottom of a 9:16 frame.
pub const CAPTION_Y: u32 = 1380;
/// Lowest a caption may sit.
const CAPTION_Y_MAX: u32 = 1560;

fn in_corner(x: f64, y: f64) -> bool {
    !(0.3..=0.7).contains(&x) && !(0.4..=0.6).contains(&y)
}

/// Scene cuts (seconds of `media`) in `start..end`: ffmpeg's scene score on a small copy. Blocking.
pub fn scene_cuts(media: &Path, start: f64, end: f64) -> Vec<f64> {
    let out = Command::new(crate::tools::resolve("ffmpeg"))
        .args(["-hide_banner", "-nostats", "-ss", &format!("{start:.3}"), "-to", &format!("{end:.3}"), "-i"])
        .arg(media)
        // Cuts between two dark stage cameras (TED) score only ~0.2; a false cut just re-aims the camera.
        .args(["-an", "-vf", "scale=192:-2,select='gt(scene\\,0.18)',showinfo", "-f", "null", "-"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    let Ok(out) = out else { return Vec::new() };
    parse_showinfo(&String::from_utf8_lossy(&out.stderr)).into_iter().map(|t| start + t).collect()
}

/// `pts_time:12.345` of each showinfo line.
fn parse_showinfo(log: &str) -> Vec<f64> {
    log.lines()
        .filter(|l| l.contains("Parsed_showinfo"))
        .filter_map(|l| l.split("pts_time:").nth(1)?.split_whitespace().next()?.parse().ok())
        .collect()
}

/// A crop of the source scaled into part of the 1080×1920 frame. `keys` are (clip seconds, left, top) in source
/// pixels; one key is a still crop.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Panel {
    pub w: u32,
    pub h: u32,
    pub keys: Vec<(f64, f64, f64)>,
    /// Where it goes in the output: top and height (it is always 1080 wide).
    pub dest_y: u32,
    pub dest_h: u32,
}

/// One camera shot of a clip and how it is framed. Times are on the clip's timeline.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Shot {
    pub start: f64,
    pub end: f64,
    pub layout: Layout,
    /// Empty for `Whole`.
    pub panels: Vec<Panel>,
    /// Why this layout, in words for the editor.
    pub why: &'static str,
    /// People seen in the shot.
    pub people: usize,
    /// The layouts this shot can really show (the editor offers only these).
    pub options: Vec<Layout>,
    /// Punch-ins inside this shot (following shots only).
    pub zooms: Vec<Zoom>,
    /// Centre height of the captions in the output frame, clear of faces.
    pub caption_y: u32,
}

/// All a plan needs. Times are on the clip's timeline (seconds from its first frame).
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    pub framing: Framing,
    pub faces: &'a [Face],
    pub motion: &'a [Motion],
    pub cuts: &'a [f64],
    pub words: &'a [crate::Word],
    pub duration: f64,
    pub src: (u32, u32),
    /// Applies to every shot that can show it (the AI vision check).
    pub force: Option<Layout>,
    /// Your choice per shot: a moment of the shot and its layout.
    pub overrides: &'a [(f64, Layout)],
    /// Face + content and two people: the other way up.
    pub swap: bool,
    /// Punch in on the strong lines.
    pub smart_zoom: bool,
}

/// Faces of one person through a shot.
#[derive(Debug, Clone)]
struct Track {
    faces: Vec<Face>,
}

impl Track {
    fn median(&self, pick: fn(&Face) -> f64) -> f64 {
        let mut v: Vec<f64> = self.faces.iter().map(pick).collect();
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    }
    /// How much of the shot this person fills: frames seen × size.
    fn weight(&self) -> f64 {
        self.faces.len() as f64 * self.median(|f| f.h)
    }
    fn talk(&self) -> f64 {
        self.faces.iter().map(|f| f.talk).sum::<f64>() / self.faces.len().max(1) as f64
    }
    /// The face's path as camera samples, shot-local, moved by `dx`.
    fn samples(&self, start: f64, dx: f64) -> Vec<(i64, f64, f64)> {
        self.faces.iter().map(|f| (((f.t - start) * 1000.0) as i64, f.x + dx, f.y)).collect()
    }
}

/// Groups a shot's faces into people. Within a frame the biggest face chooses first and a person takes one face; a
/// face joins the person whose last face is near (the reach grows with face size and with frames missed), else it
/// starts a new person.
fn tracks(faces: &[Face]) -> Vec<Track> {
    let mut sorted = faces.to_vec();
    sorted.sort_by(|a, b| a.t.total_cmp(&b.t).then(b.h.total_cmp(&a.h)));
    let step = 1.0 / camera::ANALYSIS_FPS;
    let mut out: Vec<Track> = Vec::new();
    for f in sorted {
        let best = out
            .iter_mut()
            .filter_map(|t| {
                let l = *t.faces.last().expect("non-empty");
                let missed = ((f.t - l.t) / step - 1.0).max(0.0);
                let dist = (l.x - f.x).hypot((l.y - f.y) * 0.6);
                (l.t < f.t && dist < 0.08f64.max(l.h * 1.1) + missed * 0.02).then_some((dist, t))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        match best {
            Some((_, t)) => t.faces.push(f),
            None => out.push(Track { faces: vec![f] }),
        }
    }
    // Two people are on screen at the same time. Two tracks that hardly ever are, however far apart, are one person
    // who moved, or came back (a stack of the same face twice is the worst this can get wrong).
    let mut merged: Vec<Track> = Vec::new();
    out.sort_by_key(|t| std::cmp::Reverse(t.faces.len()));
    for t in out {
        let same = merged.iter_mut().find(|m| {
            let both = t.faces.iter().filter(|f| m.faces.iter().any(|g| (g.t - f.t).abs() < 1e-6)).count();
            both <= (m.faces.len().min(t.faces.len()) / 10).max(2)
        });
        match same {
            Some(m) => {
                m.faces.extend(t.faces);
                m.faces.sort_by(|a, b| a.t.total_cmp(&b.t));
            }
            None => merged.push(t),
        }
    }
    merged.sort_by(|a, b| b.weight().total_cmp(&a.weight()));
    merged
}

/// Even size, at least 2 (yuv420p).
fn even(v: f64) -> u32 {
    ((v / 2.0).floor() * 2.0).max(2.0) as u32
}

/// A panel following `track` (moved by `dx`), `face_share` of its height being the face, the face `face_at` from the
/// top, cropped at `aspect` (w / h) for a `dest_h` tall place in the frame.
struct Follow {
    aspect: f64,
    face_h: f64,
    face_share: f64,
    face_at: f64,
    dest_y: u32,
    dest_h: u32,
    max_upscale: f64,
    /// Smallest crop height as a share of the tallest possible (a cap on the zoom).
    min_h: f64,
}

fn follow_panel(track: &Track, dx: f64, f: &Follow, start: f64, end: f64, src: (f64, f64)) -> Panel {
    let tallest = src.1.min(src.0 / f.aspect);
    let h = (f.face_h * src.1 / f.face_share).max(f.dest_h as f64 / f.max_upscale).max(tallest * f.min_h).min(tallest);
    let (cw, ch) = (even(h * f.aspect), even(h));
    let path = camera::smooth_focus_path(&track.samples(start, dx), ((end - start) * 1000.0) as i64, cw as f64 / src.0);
    let keys = path
        .iter()
        .map(|k| {
            let x = (k.x * src.0 - cw as f64 / 2.0).clamp(0.0, src.0 - cw as f64);
            let y = (k.y * src.1 - ch as f64 * f.face_at).clamp(0.0, src.1 - ch as f64);
            (start + k.t_ms as f64 / 1000.0, x.floor(), y.floor())
        })
        .collect();
    Panel { w: cw, h: ch, keys, dest_y: f.dest_y, dest_h: f.dest_h }
}

/// Median left edge of a panel.
fn median_x(p: &Panel) -> f64 {
    let mut v: Vec<f64> = p.keys.iter().map(|k| k.1).collect();
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// The two people's panels, left person first, pushed apart when their crops would mostly overlap.
fn stack_panels(a: &Track, b: &Track, start: f64, end: f64, src: (f64, f64), swap: bool) -> Vec<Panel> {
    let half = OUT.1 / 2;
    let (left, right) = if a.median(|f| f.x) <= b.median(|f| f.x) { (a, b) } else { (b, a) };
    let spec = |i: u32| Follow {
        aspect: OUT.0 as f64 / half as f64,
        // One scale for both, from the larger face, so neither looks zoomed.
        face_h: a.median(|f| f.h).max(b.median(|f| f.h)),
        face_share: 0.3,
        face_at: 0.42,
        dest_y: i * half,
        dest_h: half,
        max_upscale: MAX_UPSCALE_PANEL,
        min_h: 0.4,
    };
    let (first, second) = if swap { (right, left) } else { (left, right) };
    let mut p =
        vec![follow_panel(first, 0.0, &spec(0), start, end, src), follow_panel(second, 0.0, &spec(1), start, end, src)];
    let overlap =
        (median_x(&p[if swap { 1 } else { 0 }]) + p[0].w as f64 - median_x(&p[if swap { 0 } else { 1 }])).max(0.0);
    if overlap > 0.0 {
        // Half each, keeping both faces well inside (a sixth of the crop's width at most).
        let shift = (overlap / 2.0).min(p[0].w as f64 / 6.0);
        let limit = src.0 - p[0].w as f64;
        for (i, panel) in p.iter_mut().enumerate() {
            let sign = if (i == 0) != swap { -1.0 } else { 1.0 };
            for k in &mut panel.keys {
                k.1 = (k.1 + sign * shift).clamp(0.0, limit).floor();
            }
        }
    }
    p
}

/// Where the action is in a shot: per second the motion-weighted centre (x) of the frame outside `avoid`
/// (x, y, half width, half height as shares of the frame), pulled towards the free side's centre so a lone flicker
/// at an edge does not drag the crop. Shot-local camera samples.
fn action_samples(motion: &[Motion], start: f64, end: f64, avoid: (f64, f64, f64, f64)) -> Vec<(i64, f64, f64)> {
    let (gw, gh) = GRID;
    let masked = |cx: usize, cy: usize| {
        let (x, y) = ((cx as f64 + 0.5) / gw as f64, (cy as f64 + 0.5) / gh as f64);
        (x - avoid.0).abs() < avoid.2 && (y - avoid.1).abs() < avoid.3
    };
    let free_centre = 0.5 + (0.5 - avoid.0).signum() * avoid.2.min(0.25) * (avoid.0 - 0.5).abs().min(0.5) * 2.0;
    let mut out = Vec::new();
    let mut t = start;
    while t < end {
        let (mut sum, mut mass) = (0.0, 0.0);
        for m in motion.iter().filter(|m| m.t >= t && m.t < t + 1.0) {
            for cy in 0..gh {
                for cx in 0..gw {
                    if !masked(cx, cy) {
                        let w = (m.cells[cy * gw + cx] as f64).powi(2);
                        sum += w * (cx as f64 + 0.5) / gw as f64;
                        mass += w;
                    }
                }
            }
        }
        let x = if mass > 1.0 { 0.5 * sum / mass + 0.5 * free_centre } else { free_centre };
        out.push((((t - start) * 1000.0) as i64, x, 0.5));
        t += 1.0;
    }
    out
}

/// Face + content panels: the face and the action, each in its own place; the face on top unless `swap`.
fn split_panels(face: &Track, r: &Request<'_>, start: f64, end: f64, src: (f64, f64)) -> Vec<Panel> {
    let (w, h) = OUT;
    let content_h = h - FACE_PANEL_H;
    let (face_y, content_y) = if r.swap { (content_h, 0) } else { (0, FACE_PANEL_H) };
    let (fx, fy, fh) = (face.median(|f| f.x), face.median(|f| f.y), face.median(|f| f.h));
    let spec = Follow {
        aspect: w as f64 / FACE_PANEL_H as f64,
        face_h: fh,
        // Tight: a webcam's own frame ends close around the head.
        face_share: 0.5,
        face_at: 0.46,
        dest_y: face_y,
        dest_h: FACE_PANEL_H,
        max_upscale: MAX_UPSCALE_PANEL,
        min_h: 0.15,
    };
    let face_panel = follow_panel(face, 0.0, &spec, start, end, src);
    // The action: as tall as the source, keeping away from the face's own corner of the frame.
    let aspect = w as f64 / content_h as f64;
    let ch = src.1.min(src.0 / aspect);
    let (cw, ch) = (even(ch * aspect), even(ch));
    let avoid = (fx, fy, fh * 1.6, fh * 1.8);
    let samples = action_samples(r.motion, start, end, avoid);
    let path = camera::smooth_focus_path(&samples, ((end - start) * 1000.0) as i64, cw as f64 / src.0);
    let keys = path
        .iter()
        .map(|k| {
            let x = (k.x * src.0 - cw as f64 / 2.0).clamp(0.0, src.0 - cw as f64);
            (start + k.t_ms as f64 / 1000.0, x.floor(), ((src.1 - ch as f64) / 2.0).floor())
        })
        .collect();
    let content = Panel { w: cw, h: ch, keys, dest_y: content_y, dest_h: content_h };
    if r.swap { vec![content, face_panel] } else { vec![face_panel, content] }
}

/// A wide band holding everyone, over a blurred fill.
fn group_panel(people: &[Track], start: f64, src: (f64, f64)) -> Panel {
    let xs: Vec<f64> = people.iter().map(|p| p.median(|f| f.x)).collect();
    let ys: Vec<f64> = people.iter().map(|p| p.median(|f| f.y)).collect();
    let (lo, hi) = (xs.iter().copied().fold(1.0, f64::min), xs.iter().copied().fold(0.0, f64::max));
    let my = ys.iter().sum::<f64>() / ys.len() as f64;
    let cw = ((hi - lo + 0.3) * src.0).clamp(src.0 * 0.32, src.0);
    let ch = src.1.min(cw * OUT.1 as f64 / OUT.0 as f64);
    let (cw, ch) = (even(cw), even(ch));
    let x = ((lo + hi) / 2.0 * src.0 - cw as f64 / 2.0).clamp(0.0, src.0 - cw as f64).floor();
    let y = (my * src.1 - ch as f64 * 0.45).clamp(0.0, src.1 - ch as f64).floor();
    let dest_h = even(OUT.0 as f64 * ch as f64 / cw as f64).min(OUT.1);
    Panel { w: cw, h: ch, keys: vec![(start, x, y)], dest_y: (OUT.1 - dest_h) / 2, dest_h }
}

/// How each shot of a clip is framed.
pub fn plan(r: &Request<'_>) -> Vec<Shot> {
    let mut bounds = vec![0.0];
    for &c in r.cuts.iter().filter(|&&c| c > 0.0 && c < r.duration) {
        if c - bounds[bounds.len() - 1] >= MIN_SHOT_S && r.duration - c >= MIN_SHOT_S {
            bounds.push(c);
        }
    }
    bounds.push(r.duration);
    let frames = (r.duration * camera::ANALYSIS_FPS).max(1.0);
    // Two people who stay for the whole clip are a call or a podcast: a webcam is alone on its screen.
    let everyone = tracks(r.faces);
    let lasting = everyone.iter().filter(|t| t.faces.len() as f64 >= WEBCAM_PRESENCE * frames).count();
    let webcam = everyone
        .iter()
        .find(|t| {
            lasting == 1
                && t.faces.len() as f64 >= WEBCAM_PRESENCE * frames
                && t.median(|f| f.h) < WEBCAM_H
                && in_corner(t.median(|f| f.x), t.median(|f| f.y))
        })
        .map(|t| (t.median(|f| f.x), t.median(|f| f.y), t.median(|f| f.h)));
    let zooms = if r.smart_zoom { zoom::plan(r.words, &bounds[1..bounds.len() - 1], r.duration) } else { Vec::new() };
    bounds
        .windows(2)
        .map(|b| {
            let mut s = shot(r, b[0], b[1], webcam);
            if s.layout == Layout::Follow {
                s.zooms = zooms.iter().filter(|z| z.t >= s.start && z.end <= s.end).cloned().collect();
            }
            s
        })
        .collect()
}

fn shot(r: &Request<'_>, start: f64, end: f64, webcam: Option<(f64, f64, f64)>) -> Shot {
    let src = (r.src.0 as f64, r.src.1 as f64);
    let faces: Vec<Face> = r.faces.iter().filter(|f| f.t >= start && f.t < end).copied().collect();
    let frames = ((end - start) * camera::ANALYSIS_FPS).max(1.0);
    // People seen in at least 30 % of the shot; someone small in the background is not a second speaker.
    let mut people: Vec<Track> = tracks(&faces).into_iter().filter(|t| t.faces.len() as f64 >= frames * 0.3).collect();
    let top = people.first().map_or(0.0, Track::weight);
    people.retain(|p| p.weight() >= top * 0.35);
    let is_webcam = |t: &Track| {
        webcam.is_some_and(|(x, y, _)| (t.median(|f| f.x) - x).abs() < 0.08 && (t.median(|f| f.y) - y).abs() < 0.08)
    };
    let cam_track = people
        .iter()
        .find(|t| is_webcam(t))
        .cloned()
        .or_else(|| webcam.map(|(x, y, h)| Track { faces: vec![Face { t: start, x, y, h, talk: 0.0 }] }));
    let apart = people.len() >= 2 && (people[0].median(|f| f.x) - people[1].median(|f| f.x)).abs() >= APART;

    let mut options = vec![Layout::Whole];
    if !people.is_empty() {
        options.extend([Layout::Follow, Layout::Split]);
    } else if webcam.is_some() {
        options.push(Layout::Split);
    }
    if people.len() >= 2 {
        options.extend([Layout::Stack, Layout::Group]);
    }

    let (auto, auto_why) = match (r.framing, people.len()) {
        (Framing::Classic, _) => (Layout::Whole, "Classic: whole frame"),
        (Framing::Full, 0) => (Layout::Whole, "Nobody on screen"),
        (Framing::Full, _) => (Layout::Follow, "Follows the speaker"),
        (Framing::Smart, _) if webcam.is_some() && (people.is_empty() || people.iter().any(is_webcam)) => {
            (Layout::Split, "Webcam stays put: face and action together")
        }
        (Framing::Smart, 0) => (Layout::Whole, "Nobody on screen"),
        (Framing::Smart, 1) => (Layout::Follow, "One person"),
        (Framing::Smart, 2) if apart => (Layout::Stack, "Two people apart"),
        (Framing::Smart, 2) => (Layout::Follow, "Two people close together"),
        (Framing::Smart, _) => (Layout::Group, "Three or more people"),
    };
    let chosen = r.overrides.iter().find(|o| o.0 >= start && o.0 < end).map(|o| o.1);
    let wanted = chosen.or(r.force.filter(|_| r.framing == Framing::Smart));
    let (layout, why) = match wanted.filter(|l| options.contains(l)) {
        Some(l) if chosen == Some(l) => (l, "Your choice"),
        Some(l) => (l, "AI vision check"),
        None => (auto, auto_why),
    };

    let (w, h) = OUT;
    let main_face = people.first().map_or(0.0, |p| p.median(|f| f.h));
    let panels = match layout {
        Layout::Whole => Vec::new(),
        Layout::Follow => {
            // The one talking leads when the mouths clearly say so; close together, a window holds both.
            let mut lead = 0;
            if people.len() >= 2 && people[1].talk() > people[0].talk() * 1.4 {
                lead = 1;
            }
            let dx = if people.len() >= 2 && !apart {
                (people[0].median(|f| f.x) + people[1].median(|f| f.x)) / 2.0 - people[lead].median(|f| f.x)
            } else {
                0.0
            };
            let spec = Follow {
                aspect: w as f64 / h as f64,
                face_h: people[lead].median(|f| f.h),
                face_share: 0.24,
                face_at: 0.4,
                dest_y: 0,
                dest_h: h,
                max_upscale: MAX_UPSCALE,
                min_h: 0.55,
            };
            vec![follow_panel(&people[lead], dx, &spec, start, end, src)]
        }
        Layout::Stack => stack_panels(&people[0], &people[1], start, end, src, r.swap),
        Layout::Split => {
            let face = cam_track.as_ref().or(people.first()).expect("Split needs a face");
            split_panels(face, r, start, end, src)
        }
        Layout::Group => vec![group_panel(&people, start, src)],
    };

    // Captions: on the seam of stacked panels, under a band, else the lower third, moved down off a low face.
    let caption_y = match layout {
        Layout::Stack | Layout::Split => panels.get(1).map_or(CAPTION_Y, |p| p.dest_y),
        Layout::Whole => {
            let band = (w as f64 * src.1 / src.0) as u32;
            ((h + band) / 2 + 120).clamp(CAPTION_Y, CAPTION_Y_MAX)
        }
        Layout::Group => (panels[0].dest_y + panels[0].dest_h + 120).clamp(CAPTION_Y, CAPTION_Y_MAX),
        Layout::Follow => {
            let p = &panels[0];
            let mut ys: Vec<f64> = p.keys.iter().map(|k| k.2).collect();
            ys.sort_by(f64::total_cmp);
            let top = ys[ys.len() / 2];
            let lead_y = people.first().map_or(0.5, |t| t.median(|f| f.y));
            let chin = (lead_y * src.1 + 0.55 * main_face * src.1 - top) / p.h as f64 * h as f64;
            if chin + 150.0 > CAPTION_Y as f64 { (chin as u32 + 150).min(CAPTION_Y_MAX) } else { CAPTION_Y }
        }
    };
    Shot { start, end, layout, panels, why, people: people.len(), options, zooms: Vec::new(), caption_y }
}

/// Whether Smart should ask AI vision about a clip: few faces, or a face size near the webcam / person boundary.
pub fn unsure(framing: Framing, faces: &[Face], duration: f64) -> bool {
    if framing != Framing::Smart {
        return false;
    }
    let frames = (duration * camera::ANALYSIS_FPS).max(1.0);
    let seen = faces.iter().map(|f| (f.t * 1000.0) as i64).collect::<std::collections::BTreeSet<_>>().len();
    let mut hs: Vec<f64> = faces.iter().map(|f| f.h).collect();
    hs.sort_by(f64::total_cmp);
    seen as f64 / frames < 0.5 || hs.get(hs.len() / 2).is_some_and(|h| (0.10..0.24).contains(h))
}

/// Asks Antigravity CLI which layout one frame (`frame`, an image) shows. `None` when agy is missing,
/// signed out, refused, or answered off-schema; the caller keeps its own choice then.
pub async fn vision_layout(frame: &Path) -> Option<Layout> {
    let dir = frame.parent()?;
    let schema = dir.join("layout.schema.json");
    let schema_json = r#"{"type":"object","properties":{"layout":{"type":"string","enum":["talking_head","screen_with_webcam","no_person"]}},"required":["layout"]}"#;
    std::fs::write(&schema, schema_json).ok()?;
    let prompt = format!(
        "Use your file viewing tool (not a shell command) to open the image {} and look at it. It is one frame of a \
         livestream. Answer talking_head if a person's face and upper body fill much of the frame, screen_with_webcam \
         if it is mostly a screen, game or desktop with a small webcam of the streamer, or no_person if nobody is visible.",
        frame.display()
    );
    let out = tokio::process::Command::new(crate::tools::resolve("agy"))
        .args(["-p", &prompt, "--output-format", "json", "--json-schema"])
        .arg(&schema)
        .arg("--add-dir")
        .arg(dir)
        .args(["--print-timeout", "2m"])
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    std::fs::remove_file(&schema).ok();
    parse_vision(&String::from_utf8_lossy(&out.stdout))
}

/// agy's JSON envelope → layout. Not SUCCESS, any denied action, or no structured answer → `None`.
fn parse_vision(stdout: &str) -> Option<Layout> {
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    let denied = v["denied_actions"].as_array().is_some_and(|a| !a.is_empty());
    if v["status"] != "SUCCESS" || denied {
        return None;
    }
    match v["structured_output"]["layout"].as_str()? {
        "talking_head" => Some(Layout::Follow),
        "screen_with_webcam" => Some(Layout::Split),
        "no_person" => Some(Layout::Whole),
        _ => None,
    }
}

/// The `[input] → [output]` chain for a 1080×1920 clip framed by `shots` (from `plan`). Several shots are cut
/// apart with `trim`, framed one by one and joined again.
pub fn vertical_chain(shots: &[Shot], io: (&str, &str)) -> String {
    let (input, output) = io;
    match shots {
        [] => classic_chain(input, output),
        [one] => shot_chain(one, input, output),
        _ => {
            let n = shots.len();
            let mut g = format!("[{input}]split={n}");
            for i in 0..n {
                g.push_str(&format!("[sh{i}]"));
            }
            g.push(';');
            for (i, s) in shots.iter().enumerate() {
                let end = if i + 1 == n { String::new() } else { format!(":end={:.3}", s.end) };
                g.push_str(&format!("[sh{i}]trim=start={:.3}{end},setpts=PTS-STARTPTS[st{i}];", s.start));
                g.push_str(&shot_chain(s, &format!("st{i}"), &format!("so{i}")));
            }
            for i in 0..n {
                g.push_str(&format!("[so{i}]"));
            }
            g.push_str(&format!("concat=n={n}:v=1:a=0[{output}];"));
            g
        }
    }
}

/// One shot, its `t` starting at 0.
fn shot_chain(s: &Shot, input: &str, output: &str) -> String {
    let (w, h) = OUT;
    let crop = |p: &Panel| {
        let local: Vec<(f64, f64, f64)> = p.keys.iter().map(|&(t, x, y)| (t - s.start, x, y)).collect();
        let x = lerp_expr(&local.iter().map(|k| (k.0, k.1)).collect::<Vec<_>>());
        let y = lerp_expr(&local.iter().map(|k| (k.0, k.2)).collect::<Vec<_>>());
        format!("crop=w={}:h={}:x='{x}':y='{y}',scale={w}:{}:flags=lanczos", p.w, p.h, p.dest_h)
    };
    match s.panels.as_slice() {
        [] => classic_chain(input, output),
        // A band over the blurred frame.
        [p] if p.dest_h < h => format!(
            "[{input}]split=2[{output}bg][{output}fg];[{output}bg]scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},gblur=sigma=24,eq=brightness=-0.12[{output}bl];\
             [{output}fg]{}[{output}fs];[{output}bl][{output}fs]overlay=0:{},setsar=1,format=yuv420p[{output}];",
            crop(p),
            p.dest_y
        ),
        [p] => {
            // Smart zoom: enlarge frame by frame and cut back to size, keeping the face (40 % down) where it is.
            let zoom = if s.zooms.is_empty() {
                String::new()
            } else {
                let z = zoom::expr(&s.zooms, s.start);
                format!(
                    ",scale=w='2*trunc({}*({z}))':h='2*trunc({}*({z}))':eval=frame:flags=bicubic,crop={w}:{h}:(iw-{w})/2:(ih-{h})*0.4",
                    w / 2,
                    h / 2
                )
            };
            format!("[{input}]{}{zoom},setsar=1,format=yuv420p[{output}];", crop(p))
        }
        [a, b] => format!(
            "[{input}]split=2[{output}a][{output}b];[{output}a]{}[{output}t];[{output}b]{}[{output}u];\
             [{output}t][{output}u]vstack=inputs=2,drawbox=x=0:y={}:w={w}:h={DIVIDER_H}:color=white@0.9:t=fill,setsar=1,format=yuv420p[{output}];",
            crop(a),
            crop(b),
            b.dest_y - DIVIDER_H / 2
        ),
        _ => classic_chain(input, output),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: (u32, u32) = (1920, 1080);

    fn req<'a>(framing: Framing, faces: &'a [Face], cuts: &'a [f64]) -> Request<'a> {
        Request {
            framing,
            faces,
            motion: &[],
            cuts,
            words: &[],
            duration: 10.0,
            src: SRC,
            force: None,
            overrides: &[],
            swap: false,
            smart_zoom: false,
        }
    }

    fn one(framing: Framing, f: &[Face]) -> Shot {
        plan(&req(framing, f, &[])).remove(0)
    }

    fn faces(h: f64, x: f64) -> Vec<Face> {
        (0..40).map(|i| Face { t: i as f64 * 0.25, x, y: 0.4, h, talk: 1.0 }).collect()
    }

    fn with(mut a: Vec<Face>, b: Vec<Face>) -> Vec<Face> {
        a.extend(b);
        a
    }

    #[test]
    fn expressions_interpolate_between_keyframes() {
        assert_eq!(lerp_expr(&[(0.0, 100.0)]), "100.0");
        let e = lerp_expr(&[(0.0, 0.0), (2.0, 100.0), (4.0, 100.0)]);
        assert_eq!(e, "if(lt(t,2.000),0.0+(100.0)*(t-0.000)/2.000,if(lt(t,4.000),100.0+(0.0)*(t-2.000)/2.000,100.0))");
    }

    #[test]
    fn decodes_faces_and_merges_duplicates() {
        let n8 = (SIDE / 8) * (SIDE / 8);
        let (mut cls8, mut obj8, mut bbox8, kps8) =
            (vec![0.0f32; n8], vec![0.0f32; n8], vec![0.0f32; n8 * 4], vec![0.0f32; n8 * 10]);
        let idx = 10 * (SIDE / 8) + 20; // row 10, column 20
        (cls8[idx], obj8[idx]) = (0.9, 0.9);
        bbox8[idx * 4..idx * 4 + 4].copy_from_slice(&[0.5, 0.5, 0.0, 2f32.ln()]);
        let small = |s: usize| vec![0.0f32; (SIDE / s) * (SIDE / s)];
        let (c16, c32) = (small(16), small(32));
        let (b16, b32) = (vec![0.0f32; c16.len() * 4], vec![0.0f32; c32.len() * 4]);
        let (k16, k32) = (vec![0.0f32; c16.len() * 10], vec![0.0f32; c32.len() * 10]);
        // A second, weaker face elsewhere, and a near-duplicate of the first.
        let far = 30 * (SIDE / 8) + 60;
        (cls8[far], obj8[far]) = (0.7, 0.7);
        bbox8[far * 4..far * 4 + 4].copy_from_slice(&[0.0, 0.0, 2f32.ln(), 2f32.ln()]);
        let dup = idx + 1;
        (cls8[dup], obj8[dup]) = (0.8, 0.8);
        bbox8[dup * 4..dup * 4 + 4].copy_from_slice(&[-0.5, 0.5, 0.0, 2f32.ln()]);
        let raw = Raw {
            cls: [&cls8, &c16, &c32],
            obj: [&obj8, &c16, &c32],
            bbox: [&bbox8, &b16, &b32],
            kps: [&kps8, &k16, &k32],
        };
        let found = faces_in(&raw, 0.6);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!((found[0].cx, found[0].cy), (20.5 * 8.0, 10.5 * 8.0));
        assert!((found[0].h - 16.0).abs() < 1e-4 && (found[0].score - 0.9).abs() < 1e-6);
        assert_eq!((found[1].cx, found[1].cy), (60.0 * 8.0, 30.0 * 8.0));
        assert_eq!(found[0].mouth.0, 20.0 * 8.0, "landmarks are decoded from the anchor");
    }

    #[test]
    fn frames_are_tiled_along_the_long_side() {
        assert_eq!(analysis_size((1920, 1080)), (1138, 640));
        assert_eq!(tiles(1138, 640), [(0, 0), (249, 0), (498, 0)]);
        assert!(tiles(640, 640).is_empty());
        assert_eq!(tiles(640, 1138), [(0, 0), (0, 249), (0, 498)]);
    }

    #[test]
    fn rules_pick_a_layout_per_shot() {
        let follow = one(Framing::Smart, &faces(0.3, 0.7));
        assert_eq!(
            (follow.layout, follow.panels[0].w, follow.panels[0].h, follow.why),
            (Layout::Follow, 606, 1080, "One person")
        );
        let wide = one(Framing::Smart, &faces(0.06, 0.5));
        assert_eq!(wide.layout, Layout::Follow, "a small face mid-frame is a person in a wide shot, not a webcam");
        assert!(wide.panels[0].h < 1080, "and gets closer: {:?}", wide.panels[0].h);

        let two = with(faces(0.2, 0.3), faces(0.18, 0.72));
        let stack = one(Framing::Smart, &two);
        assert_eq!(stack.layout, Layout::Stack, "two people: one above the other");
        assert!(stack.panels[0].keys[0].1 < stack.panels[1].keys[0].1 && stack.panels[1].dest_y == 960);
        assert_eq!(stack.caption_y, 960, "captions on the seam, off both faces");
        let close = one(Framing::Smart, &with(faces(0.2, 0.45), faces(0.2, 0.55)));
        assert_eq!(close.layout, Layout::Follow, "close together: one window");
        let mid = close.panels[0].keys[0].1 + close.panels[0].w as f64 / 2.0;
        assert!((mid - 960.0).abs() < 4.0, "centred between them: {mid}");

        let group = one(Framing::Smart, &with(with(faces(0.15, 0.2), faces(0.15, 0.5)), faces(0.15, 0.8)));
        assert_eq!(group.layout, Layout::Group, "three people: a band");
        assert!(group.panels[0].dest_h < 1920 && group.panels[0].dest_y > 0);

        assert_eq!(one(Framing::Full, &two).layout, Layout::Follow, "Follow mode always follows");
        assert_eq!(one(Framing::Smart, &[]).layout, Layout::Whole, "nobody: whole frame");
        assert_eq!(one(Framing::Classic, &faces(0.3, 0.5)).layout, Layout::Whole);
        assert_eq!(
            one(Framing::Smart, &faces(0.3, 0.5)[..10]).layout,
            Layout::Whole,
            "barely there is not the subject"
        );
    }

    #[test]
    fn webcam_and_game_stack_either_way_round() {
        let cam: Vec<Face> = faces(0.1, 0.88).into_iter().map(|f| Face { y: 0.8, ..f }).collect();
        // The action happens on the left of the frame.
        let (gw, gh) = GRID;
        let mut cells = vec![0f32; gw * gh];
        for y in 0..gh {
            cells[y * gw + 6] = 60.0;
        }
        let motion: Vec<Motion> = (0..40).map(|i| Motion { t: i as f64 * 0.25, cells: cells.clone() }).collect();
        let r = Request { motion: &motion, ..req(Framing::Smart, &cam, &[]) };
        let s = plan(&r).remove(0);
        assert_eq!((s.layout, s.why), (Layout::Split, "Webcam stays put: face and action together"));
        assert_eq!((s.panels[0].dest_y, s.panels[1].dest_y), (0, FACE_PANEL_H), "face on top");
        let face_mid = s.panels[0].keys[0].1 + s.panels[0].w as f64 / 2.0;
        assert!(face_mid > 1300.0, "the face panel is on the webcam: {face_mid}");
        let action_mid = s.panels[1].keys[0].1 + s.panels[1].w as f64 / 2.0;
        assert!(action_mid < 960.0, "the content follows the action: {action_mid}");
        assert_eq!(s.caption_y, FACE_PANEL_H, "captions on the seam");
        let g = vertical_chain(std::slice::from_ref(&s), ("v", "o"));
        assert!(g.contains("vstack=inputs=2,drawbox=x=0:y=765"), "a divider on the seam: {g}");

        let up = Request { swap: true, ..r };
        let s = plan(&up).remove(0);
        assert_eq!(s.panels[0].dest_y, 0);
        assert_eq!(s.panels[1].dest_y, 1920 - FACE_PANEL_H, "swapped: the action on top, the face below");
        assert_eq!(s.panels[1].dest_h, FACE_PANEL_H);
        assert_eq!(s.caption_y, 1920 - FACE_PANEL_H);

        let mut nobody = cam.clone();
        nobody.retain(|f| f.t < 7.5);
        let r = Request { motion: &motion, ..req(Framing::Smart, &nobody, &[7.5]) };
        let shots = plan(&r);
        assert_eq!(shots[1].layout, Layout::Split, "a webcam stays put through a moment it was not seen");
    }

    #[test]
    fn two_lasting_faces_are_a_call_not_a_webcam() {
        let at = |x: f64| faces(0.12, x).into_iter().map(|f| Face { y: 0.3, ..f }).collect::<Vec<_>>();
        assert_eq!(one(Framing::Smart, &with(at(0.22), at(0.75))).layout, Layout::Stack);
    }

    #[test]
    fn stage_faces_are_not_a_webcam() {
        // A close-up for 6 s, then a wide stage shot with a small face in the corner for 4 s.
        let mut stage: Vec<Face> = faces(0.35, 0.5)[..24].to_vec();
        stage.extend(faces(0.06, 0.85)[24..].iter().map(|f| Face { y: 0.8, ..*f }));
        let shots = plan(&req(Framing::Smart, &stage, &[6.0]));
        assert_eq!(shots.iter().map(|s| s.layout).collect::<Vec<_>>(), [Layout::Follow, Layout::Follow]);
    }

    #[test]
    fn shots_offer_what_they_can_show_and_your_choice_wins() {
        let two = with(faces(0.2, 0.3), faces(0.18, 0.72));
        let lone = faces(0.3, 0.5);
        assert_eq!(one(Framing::Smart, &[]).options, [Layout::Whole]);
        assert_eq!(one(Framing::Smart, &lone).options, [Layout::Whole, Layout::Follow, Layout::Split]);
        assert!(one(Framing::Smart, &two).options.contains(&Layout::Stack));

        let r = Request { overrides: &[(2.0, Layout::Whole)], ..req(Framing::Smart, &two, &[]) };
        let s = plan(&r).remove(0);
        assert_eq!((s.layout, s.why), (Layout::Whole, "Your choice"));
        let r = Request { overrides: &[(2.0, Layout::Stack)], ..req(Framing::Smart, &lone, &[]) };
        assert_eq!(plan(&r)[0].layout, Layout::Follow, "a choice the shot cannot show falls back to the rules");
    }

    #[test]
    fn the_one_talking_leads() {
        let quiet = faces(0.2, 0.45);
        let talking: Vec<Face> = faces(0.18, 0.75).into_iter().map(|f| Face { talk: 9.0, ..f }).collect();
        let s = one(Framing::Full, &with(quiet, talking));
        let mid = s.panels[0].keys[0].1 + s.panels[0].w as f64 / 2.0;
        assert!(mid > 1250.0, "follows the mouth that moves: {mid}");
    }

    #[test]
    fn someone_who_jumps_or_returns_stays_one_person() {
        let mut f = faces(0.3, 0.4)[..18].to_vec();
        f.extend(faces(0.3, 0.5)[22..].iter().copied());
        assert_eq!(tracks(&f).len(), 1);
        // Far across the frame, but never at the same time as themselves.
        let mut walked = faces(0.3, 0.2)[..20].to_vec();
        walked.extend(faces(0.3, 0.8)[20..].iter().copied());
        assert_eq!(tracks(&walked).len(), 1);
        assert_eq!(one(Framing::Smart, &walked).layout, Layout::Follow, "never the same face stacked twice");
        // A face counted twice for a moment does not make a second person.
        let mut twice = faces(0.3, 0.4);
        twice.extend(faces(0.3, 0.43)[5..7].iter().copied());
        assert_eq!(tracks(&twice).len(), 1);
    }

    #[test]
    fn people_keep_their_tracks_when_they_cross_paths_of_detection() {
        // Two people, one drops out for a moment; a far-away face never joins the wrong person.
        let mut f = faces(0.2, 0.3);
        f.retain(|x| !(2.0..3.0).contains(&x.t));
        f.extend(faces(0.2, 0.7));
        let t = tracks(&f);
        assert_eq!(t.len(), 2, "{}", t.len());
        assert!(t.iter().all(|p| (p.median(|f| f.x) - p.faces[0].x).abs() < 1e-9));
    }

    #[test]
    fn camera_cuts_with_the_shot() {
        // Close-up of A on the left for 5 s, then a cut to B on the right.
        let mut f: Vec<Face> =
            (0..20).map(|i| Face { t: i as f64 * 0.25, x: 0.25, y: 0.4, h: 0.35, talk: 0.0 }).collect();
        f.extend((20..40).map(|i| Face { t: i as f64 * 0.25, x: 0.75, y: 0.4, h: 0.35, talk: 0.0 }));
        let shots = plan(&req(Framing::Smart, &f, &[5.0, 5.4]));
        assert_eq!(shots.len(), 2, "a cut 0.4 s after another is dropped: {shots:?}");
        let x_at_start = |s: &Shot| s.panels[0].keys[0].1 + s.panels[0].w as f64 / 2.0;
        assert!(
            (x_at_start(&shots[0]) - 480.0).abs() < 2.0 && (x_at_start(&shots[1]) - 1440.0).abs() < 2.0,
            "{shots:?}"
        );
        let g = vertical_chain(&shots, ("v", "o"));
        assert!(
            g.contains("split=2[sh0][sh1]")
                && g.contains("trim=start=0.000:end=5.000")
                && g.contains("concat=n=2:v=1:a=0[o]"),
            "{g}"
        );
        assert!(g.contains("[sh1]trim=start=5.000,setpts=PTS-STARTPTS"), "last shot runs to the end: {g}");
        assert_eq!(parse_showinfo("[Parsed_showinfo_2 @ 0x1] n:   0 pts:  51200 pts_time:4.0     duration"), [4.0]);
    }

    #[test]
    fn two_whole_shots_keep_their_filter_labels_apart() {
        let shots = plan(&req(Framing::Smart, &[], &[5.0]));
        let g = vertical_chain(&shots, ("v", "o"));
        assert!(g.contains("[so0bg]") && g.contains("[so1bg]"), "{g}");
    }

    #[test]
    fn smart_zoom_stays_inside_following_shots() {
        let words: Vec<crate::Word> = "Why did nobody tell me this about the million dollars?"
            .split_whitespace()
            .enumerate()
            .map(|(i, w)| crate::Word { text: w.into(), start: 0.5 + i as f64 * 0.4, end: 0.8 + i as f64 * 0.4 })
            .collect();
        let f = faces(0.3, 0.5);
        let r = Request { words: &words, smart_zoom: true, ..req(Framing::Smart, &f, &[]) };
        let s = plan(&r).remove(0);
        assert!(!s.zooms.is_empty());
        let g = vertical_chain(std::slice::from_ref(&s), ("v", "o"));
        assert!(g.contains("eval=frame") && g.contains("crop=1080:1920:(iw-1080)/2:(ih-1920)*0.4"), "{g}");
        let off = plan(&Request { smart_zoom: false, ..r }).remove(0);
        assert!(off.zooms.is_empty());
    }

    #[test]
    fn smart_flags_unsure_clips_for_a_vision_check() {
        assert!(!unsure(Framing::Smart, &faces(0.3, 0.5), 10.0));
        assert!(unsure(Framing::Smart, &faces(0.15, 0.5), 10.0), "near the size boundary");
        assert!(unsure(Framing::Smart, &faces(0.3, 0.5)[..10], 10.0), "face in a quarter of the frames");
        assert!(!unsure(Framing::Full, &faces(0.15, 0.5), 10.0));
    }

    #[test]
    fn reads_agy_envelopes() {
        let ok = r#"{"status":"SUCCESS","structured_output":{"layout":"screen_with_webcam"},"denied_actions":[]}"#;
        assert_eq!(parse_vision(ok), Some(Layout::Split));
        assert_eq!(parse_vision(r#"{"status":"ERROR","structured_output":null}"#), None);
        let denied = r#"{"status":"SUCCESS","structured_output":{"layout":"no_person"},"denied_actions":["shell"]}"#;
        assert_eq!(parse_vision(denied), None, "a denied action means it did not just look at the file");
        assert_eq!(parse_vision("not json"), None);
    }
}
