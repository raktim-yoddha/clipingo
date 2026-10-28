//! 9:16 framing. **Full frame** follows the speaker with a smoothed crop (see `camera`); **Smart** looks at
//! the face size: a talking head is followed, a small face (webcam over a screen share) becomes screen on top,
//! webcam below; **Classic** is the whole frame over a blurred fill. Faces come from YuNet (OpenCV Zoo,
//! 2023mar) on ONNX Runtime, sampled at `camera::ANALYSIS_FPS`.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::camera;
use crate::models::{self, ProgressFn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Framing {
    /// Follow a talking head; split screen + webcam when the face is small.
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
/// A face taller than this share of the frame is a talking head; smaller is a webcam overlay.
const TALKING_HEAD_H: f64 = 0.16;
/// Output frame and the two halves of the split layout.
const OUT: (u32, u32) = (1080, 1920);
const HALF_H: u32 = 960;

/// The best face in one sampled frame: time, centre and height as shares of the source frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Face {
    pub t: f64,
    pub x: f64,
    pub y: f64,
    pub h: f64,
}

/// The face model, downloaded on first use (230 KB).
pub async fn model(progress: ProgressFn<'_>) -> Result<PathBuf> {
    models::download(MODEL_URL, &models::models_dir().join(MODEL_FILE), "face detector", progress).await
}

/// Highest-scoring face in YuNet's raw outputs, in input pixels: (centre x, centre y, height, score).
/// `cls`, `obj`, `bbox` are per stride 8 / 16 / 32, laid out row-major over the stride grid.
fn best_face(cls: [&[f32]; 3], obj: [&[f32]; 3], bbox: [&[f32]; 3]) -> Option<(f32, f32, f32, f32)> {
    let mut best: Option<(f32, f32, f32, f32)> = None;
    for (k, stride) in [8usize, 16, 32].into_iter().enumerate() {
        let cols = SIDE / stride;
        for idx in 0..cols * cols {
            let score = (cls[k][idx].clamp(0.0, 1.0) * obj[k][idx].clamp(0.0, 1.0)).sqrt();
            if best.is_none_or(|b| score > b.3) {
                let (r, c, b) = ((idx / cols) as f32, (idx % cols) as f32, &bbox[k][idx * 4..idx * 4 + 4]);
                let s = stride as f32;
                best = Some(((c + b[0]) * s, (r + b[1]) * s, b[3].exp() * s, score));
            }
        }
    }
    best
}

/// Faces in `start..end` (seconds) of `media`, whose frames are `src` pixels. Blocking: runs ffmpeg and the model.
pub fn detect(model: &Path, media: &Path, start: f64, end: f64, src: (u32, u32)) -> Result<Vec<Face>> {
    // ort's builder errors carry the builder (not Send), so they are flattened to text.
    let ort_err = |e: &dyn std::fmt::Display| anyhow::anyhow!("face detector: {e}");
    // ONNX Runtime allows one environment per process, ever (even after it is dropped): build it once, share it.
    static ENV: std::sync::OnceLock<Result<ort::environment::Environment, String>> = std::sync::OnceLock::new();
    let env = ENV.get_or_init(|| ort::init().build().map_err(|e| e.to_string())).as_ref().map_err(|e| ort_err(e))?;
    let builder = ort::session::Session::builder(env).map_err(|e| ort_err(&e))?;
    let mut builder = builder.with_intra_threads(4).map_err(|e| ort_err(&e))?;
    let mut session = builder.commit_from_file(model).map_err(|e| ort_err(&e))?;
    let vf = format!(
        "fps={},scale={SIDE}:{SIDE}:force_original_aspect_ratio=decrease,pad={SIDE}:{SIDE}:0:0",
        camera::ANALYSIS_FPS
    );
    let mut child = Command::new(crate::tools::resolve("ffmpeg"))
        .args(["-hide_banner", "-v", "error", "-ss", &format!("{start:.3}"), "-to", &format!("{end:.3}"), "-i"])
        .arg(media)
        .args(["-an", "-vf", &vf, "-pix_fmt", "bgr24", "-f", "rawvideo", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("could not start ffmpeg")?;
    let mut stdout = child.stdout.take().expect("piped");
    // Input pixels per source pixel: the frame is scaled to fit and padded at the right / bottom.
    let scale = (SIDE as f64 / src.0 as f64).min(SIDE as f64 / src.1 as f64);
    let plane = SIDE * SIDE;
    let mut frame = vec![0u8; plane * 3];
    let mut faces = Vec::new();
    let mut n = 0;
    while stdout.read_exact(&mut frame).is_ok() {
        // Interleaved BGR bytes → planar BGR floats, 0–255 (YuNet's own preprocessing).
        let mut input = vec![0f32; plane * 3];
        for (p, px) in frame.chunks_exact(3).enumerate() {
            for ch in 0..3 {
                input[ch * plane + p] = px[ch] as f32;
            }
        }
        let tensor = ort::value::Tensor::from_array(([1usize, 3, SIDE, SIDE], input))?;
        let out = session.run(ort::inputs!["input" => tensor])?;
        let get = |name: &str| out[name].try_extract_tensor::<f32>().map(|(_, d)| d);
        let found = best_face(
            [get("cls_8")?, get("cls_16")?, get("cls_32")?],
            [get("obj_8")?, get("obj_16")?, get("obj_32")?],
            [get("bbox_8")?, get("bbox_16")?, get("bbox_32")?],
        );
        if let Some((cx, cy, fh, score)) = found
            && score >= MIN_SCORE
        {
            let px = |v: f32| v as f64 / scale;
            faces.push(Face {
                t: start + n as f64 / camera::ANALYSIS_FPS,
                x: px(cx) / src.0 as f64,
                y: px(cy) / src.1 as f64,
                h: px(fh) / src.1 as f64,
            });
        }
        n += 1;
    }
    child.wait()?;
    Ok(faces)
}

/// Even size, at least 2 (yuv420p).
fn even(v: f64) -> u32 {
    ((v / 2.0).floor() * 2.0).max(2.0) as u32
}

/// Even offset, at least 0.
fn even_at(v: f64) -> u32 {
    ((v / 2.0).floor() * 2.0).max(0.0) as u32
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
        "[{input}]split=2[bg][fg];[bg]scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},gblur=sigma=24,eq=brightness=-0.12[bgb];\
         [fg]scale={w}:-2:flags=lanczos[fgs];[bgb][fgs]overlay=(W-w)/2:(H-h)/2,setsar=1,format=yuv420p[{output}];"
    )
}

/// What a 9:16 clip shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// A 9:16 window following the speaker.
    Follow,
    /// Screen on top, webcam below.
    Split,
    /// Whole frame over a blurred fill.
    Whole,
}

fn median_height(faces: &[Face]) -> Option<f64> {
    let mut heights: Vec<f64> = faces.iter().map(|f| f.h).collect();
    heights.sort_by(f64::total_cmp);
    heights.get(heights.len() / 2).copied()
}

/// The layout for `framing`, and whether Smart is unsure (few faces, or a face size near the
/// talking-head / webcam boundary): those clips can be checked with AI vision.
pub fn choose(framing: Framing, faces: &[Face], duration: f64) -> (Layout, bool) {
    let coverage = faces.len() as f64 / (duration * camera::ANALYSIS_FPS).max(1.0);
    match (framing, median_height(faces)) {
        (Framing::Classic, _) => (Layout::Whole, false),
        (_, None) => (Layout::Whole, framing == Framing::Smart),
        (Framing::Full, Some(_)) => (Layout::Follow, false),
        (Framing::Smart, Some(h)) => {
            let layout = if h < TALKING_HEAD_H { Layout::Split } else { Layout::Follow };
            (layout, coverage < 0.5 || (0.10..0.24).contains(&h))
        }
    }
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

/// The `[input] → [output]` chain for a 1080×1920 clip of `duration` seconds in `layout`. `faces` are on the
/// clip's own timeline (seconds from its first frame); without faces only `Whole` is possible.
pub fn vertical_chain(layout: Layout, faces: &[Face], duration: f64, src: (u32, u32), io: (&str, &str)) -> String {
    let (input, output) = io;
    let (sw, sh) = (src.0 as f64, src.1 as f64);
    let (w, h) = OUT;
    let Some(median_h) = median_height(faces).filter(|_| layout != Layout::Whole) else {
        return classic_chain(input, output);
    };

    if layout == Layout::Split {
        // Webcam over a screen share: screen on top (9:8, centred), the webcam's head and shoulders below.
        // ponytail: one layout per clip from the median face; per-shot layouts need scene-cut detection.
        let mut xs: Vec<f64> = faces.iter().map(|f| f.x).collect();
        let mut ys: Vec<f64> = faces.iter().map(|f| f.y).collect();
        xs.sort_by(f64::total_cmp);
        ys.sort_by(f64::total_cmp);
        let (fx, fy) = (xs[xs.len() / 2] * sw, ys[ys.len() / 2] * sh);
        let aspect = w as f64 / HALF_H as f64;
        let cam_h = (median_h * sh * 3.2).min(sh).min(sw / aspect);
        let (cam_w, cam_h) = (even(cam_h * aspect), even(cam_h));
        let cam_x = even_at((fx - cam_w as f64 / 2.0).clamp(0.0, sw - cam_w as f64));
        let cam_y = even_at((fy - cam_h as f64 * 0.45).clamp(0.0, sh - cam_h as f64));
        let scr_h = sh.min(sw / aspect);
        let (scr_w, scr_h) = (even(scr_h * aspect), even(scr_h));
        let scr_x = even_at((sw - scr_w as f64) / 2.0);
        let scr_y = even_at((sh - scr_h as f64) / 2.0);
        return format!(
            "[{input}]split=2[scr][cam];[scr]crop={scr_w}:{scr_h}:{scr_x}:{scr_y},scale={w}:{HALF_H}:flags=lanczos[top];\
             [cam]crop={cam_w}:{cam_h}:{cam_x}:{cam_y},scale={w}:{HALF_H}:flags=lanczos[bot];\
             [top][bot]vstack=inputs=2,setsar=1,format=yuv420p[{output}];"
        );
    }

    // Follow the speaker: a 9:16 window as tall as the source, moved along the smoothed face path.
    let crop_h = sh.min(sw * h as f64 / w as f64);
    let (crop_w, crop_h) = (even(crop_h * w as f64 / h as f64), even(crop_h));
    let samples: Vec<(i64, f64, f64)> = faces.iter().map(|f| ((f.t * 1000.0) as i64, f.x, f.y)).collect();
    let path = camera::smooth_focus_path(&samples, (duration * 1000.0) as i64, crop_w as f64 / sw);
    let axis = |pos: fn(&camera::Keyframe) -> f64, size: f64, span: f64| {
        let keys: Vec<(f64, f64)> = path
            .iter()
            .map(|k| (k.t_ms as f64 / 1000.0, (pos(k) * span - size / 2.0).clamp(0.0, (span - size).max(0.0))))
            .collect();
        lerp_expr(&keys)
    };
    let x = axis(|k| k.x, crop_w as f64, sw);
    let y = axis(|k| k.y, crop_h as f64, sh);
    format!(
        "[{input}]crop=w={crop_w}:h={crop_h}:x='{x}':y='{y}',scale={w}:{h}:flags=lanczos,setsar=1,format=yuv420p[{output}];"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_strongest_anchor() {
        let n8 = (SIDE / 8) * (SIDE / 8);
        let (mut cls8, mut obj8, mut bbox8) = (vec![0.0f32; n8], vec![0.0f32; n8], vec![0.0f32; n8 * 4]);
        let idx = 10 * (SIDE / 8) + 20; // row 10, column 20
        (cls8[idx], obj8[idx]) = (0.9, 0.9);
        bbox8[idx * 4..idx * 4 + 4].copy_from_slice(&[0.5, 0.5, 0.0, 2f32.ln()]);
        let small = |s: usize| vec![0.0f32; (SIDE / s) * (SIDE / s)];
        let (c16, c32) = (small(16), small(32));
        let (b16, b32) = (vec![0.0f32; c16.len() * 4], vec![0.0f32; c32.len() * 4]);
        let (cx, cy, h, score) = best_face([&cls8, &c16, &c32], [&obj8, &c16, &c32], [&bbox8, &b16, &b32]).unwrap();
        assert_eq!((cx, cy), (20.5 * 8.0, 10.5 * 8.0));
        assert!((h - 16.0).abs() < 1e-4 && (score - 0.9).abs() < 1e-6);
    }

    #[test]
    fn expressions_interpolate_between_keyframes() {
        assert_eq!(lerp_expr(&[(0.0, 100.0)]), "100.0");
        let e = lerp_expr(&[(0.0, 0.0), (2.0, 100.0), (4.0, 100.0)]);
        assert_eq!(e, "if(lt(t,2.000),0.0+(100.0)*(t-0.000)/2.000,if(lt(t,4.000),100.0+(0.0)*(t-2.000)/2.000,100.0))");
    }

    fn faces(h: f64, x: f64) -> Vec<Face> {
        (0..40).map(|i| Face { t: i as f64 * 0.25, x, y: 0.4, h }).collect()
    }

    #[test]
    fn smart_picks_follow_or_split_by_face_size() {
        let src = (1920, 1080);
        let chain = |framing, f: &[Face]| vertical_chain(choose(framing, f, 10.0).0, f, 10.0, src, ("v", "o"));
        let follow = chain(Framing::Smart, &faces(0.3, 0.7));
        assert!(follow.contains("crop=w=606:h=1080:x='") && follow.contains("scale=1080:1920"), "{follow}");
        let split = chain(Framing::Smart, &faces(0.06, 0.85));
        assert!(split.contains("vstack") && split.contains("crop=1214:1080:352:0"), "screen 9:8 centred: {split}");
        assert!(!chain(Framing::Full, &faces(0.06, 0.85)).contains("vstack"), "Full frame always follows");
        assert!(chain(Framing::Smart, &[]).contains("gblur"), "no face: classic");
    }

    #[test]
    fn smart_flags_unsure_clips_for_a_vision_check() {
        assert_eq!(choose(Framing::Smart, &faces(0.3, 0.5), 10.0), (Layout::Follow, false));
        assert_eq!(choose(Framing::Smart, &faces(0.06, 0.5), 10.0), (Layout::Split, false));
        assert!(choose(Framing::Smart, &faces(0.15, 0.5), 10.0).1, "near the size boundary");
        assert!(choose(Framing::Smart, &faces(0.3, 0.5)[..10], 10.0).1, "face in a quarter of the frames");
        assert_eq!(choose(Framing::Full, &faces(0.15, 0.5), 10.0), (Layout::Follow, false));
        assert_eq!(choose(Framing::Classic, &[], 10.0), (Layout::Whole, false));
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
