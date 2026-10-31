//! Evals for the whole clipping process, so changes are measured instead of eyeballed:
//!
//! - **Transcript** against a reference (human captions as WebVTT / SRT, or exact word times from a TTS fixture):
//!   word error rate, recall (words heard) and sync (our word start vs the reference's cue start).
//! - **Clips**: do they start and end on sentence boundaries, without cutting a word or opening mid-thought.
//! - **Framing** (from the render's plan): layouts used, how far crops are enlarged, how fast the camera moves, how
//!   often it zooms.
//! - **Exports**: right size, a whole face in the 9:16 frame compared with the source, its size, centring and
//!   steadiness (YuNet on the exported video), captions off the faces, inside the clip, readable and complete,
//!   loudness and peaks, black frames, pauses left after dead-air cuts.
//!
//! Each export gets a 0–100 score and the run their mean. `clip eval <run-id> [reference.vtt]` prints the report;
//! `tests/headless/run.py` holds a baseline so a change that makes clips worse fails.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::Word;

/// A caption cue: seconds on the source timeline and its text.
#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// "00:01:02.500" / "01:02,500" / "62.5" → seconds.
fn timestamp(s: &str) -> Option<f64> {
    let s = s.trim().replace(',', ".");
    let mut total = 0.0;
    for part in s.split(':') {
        total = total * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(total)
}

/// WebVTT or SRT cues (cue settings after the end time and numeric SRT ids are ignored).
pub fn parse_captions(text: &str) -> Vec<Cue> {
    let mut cues = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some((a, b)) = line.split_once("-->") else { continue };
        let (Some(start), Some(end)) = (timestamp(a), timestamp(b.split_whitespace().next().unwrap_or(""))) else {
            continue;
        };
        let mut body = Vec::new();
        while let Some(l) = lines.peek() {
            if l.trim().is_empty() {
                break;
            }
            body.push(lines.next().unwrap_or_default().trim().to_string());
        }
        cues.push(Cue { start, end, text: body.join(" ") });
    }
    cues
}

/// Words for comparison: lower case, letters / digits / inner apostrophes only. Speaker labels ("CA:",
/// "Alex Doe:"), sound tags ("(Laughter)", "[Music]") and inline markup ("<i>") are dropped.
pub fn normalize(text: &str) -> Vec<String> {
    let mut clean = String::new();
    let mut depth = 0;
    for c in text.chars() {
        match c {
            '(' | '[' | '<' => depth += 1,
            ')' | ']' | '>' => depth = (depth - 1).max(0),
            _ if depth == 0 => clean.push(c),
            _ => {}
        }
    }
    let tokens: Vec<&str> = clean.split_whitespace().collect();
    // A label is up to 3 capitalised tokens, the last ending in ':' (a lone "CA:" too).
    let label_len = (1..=3.min(tokens.len())).find(|&n| {
        tokens[n - 1].ends_with(':') && tokens[..n].iter().all(|t| t.chars().next().is_some_and(char::is_uppercase))
    });
    tokens[label_len.unwrap_or(0)..]
        .iter()
        .map(|t| {
            let t: String = t.chars().filter(|c| c.is_alphanumeric() || *c == '\'' || *c == '’').collect();
            t.replace('’', "'").trim_matches('\'').to_lowercase()
        })
        .filter(|t| !t.is_empty())
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Hit,
    Sub,
    Del,
    Ins,
}

/// Levenshtein alignment of `r` (reference) and `h` (hypothesis): the edit path, in order.
fn align(r: &[String], h: &[String]) -> Vec<Op> {
    let (n, m) = (r.len(), h.len());
    let mut d = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in 0..=n {
        d[at(i, 0)] = i as u32;
    }
    for j in 0..=m {
        d[at(0, j)] = j as u32;
    }
    for i in 1..=n {
        for j in 1..=m {
            let sub = d[at(i - 1, j - 1)] + u32::from(r[i - 1] != h[j - 1]);
            d[at(i, j)] = sub.min(d[at(i - 1, j)] + 1).min(d[at(i, j - 1)] + 1);
        }
    }
    let (mut i, mut j, mut ops) = (n, m, Vec::new());
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && d[at(i, j)] == d[at(i - 1, j - 1)] + u32::from(r[i - 1] != h[j - 1]) {
            ops.push(if r[i - 1] == h[j - 1] { Op::Hit } else { Op::Sub });
            (i, j) = (i - 1, j - 1);
        } else if i > 0 && d[at(i, j)] == d[at(i - 1, j)] + 1 {
            ops.push(Op::Del);
            i -= 1;
        } else {
            ops.push(Op::Ins);
            j -= 1;
        }
    }
    ops.reverse();
    ops
}

#[derive(Debug, Clone, Serialize)]
pub struct Sync {
    /// Cues whose first word we heard.
    pub cues: usize,
    /// Our first-word start minus the cue start: positive = our captions are late.
    pub median_offset_s: f64,
    pub median_abs_s: f64,
    pub p90_abs_s: f64,
    /// Share of cues starting within 0.3 s of the reference.
    pub within_300ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TranscriptEval {
    pub reference_words: usize,
    pub words: usize,
    pub word_error_rate: f64,
    /// Reference words we heard (exact match after normalising).
    pub recall: f64,
    pub substitutions: usize,
    pub deletions: usize,
    pub insertions: usize,
    pub sync: Sync,
    /// The longest stretches of reference speech we missed: (start s, words).
    pub worst_gaps: Vec<(f64, usize)>,
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

/// Compares our transcript with reference cues inside `range` (seconds; the whole video when None).
/// Alignment runs in 60 s blocks (a whole stream in one table would take gigabytes); a word near a block edge
/// may count as one deletion plus one insertion.
pub fn transcript(reference: &[Cue], words: &[Word], range: Option<(f64, f64)>) -> TranscriptEval {
    const BLOCK_S: f64 = 60.0;
    let (lo, hi) = range.unwrap_or((f64::MIN, f64::MAX));
    // Reference words get times spread over their cue; only a cue's first word time is exact.
    let mut refs: Vec<(String, f64, bool)> = Vec::new();
    for cue in reference.iter().filter(|c| c.start >= lo && c.start < hi) {
        let ws = normalize(&cue.text);
        let n = ws.len().max(1) as f64;
        for (k, w) in ws.into_iter().enumerate() {
            refs.push((w, cue.start + (cue.end - cue.start) * k as f64 / n, k == 0));
        }
    }
    let hyp: Vec<(String, f64)> = words
        .iter()
        .filter(|w| w.start >= lo && w.start < hi)
        .flat_map(|w| normalize(&w.text).into_iter().map(move |t| (t, w.start)))
        .collect();

    let (mut hits, mut subs, mut dels, mut ins) = (0, 0, 0, 0);
    let mut offsets = Vec::new();
    let mut gaps: Vec<(f64, usize)> = Vec::new();
    let end = refs.iter().map(|r| r.1).chain(hyp.iter().map(|h| h.1)).fold(0.0, f64::max);
    let start = refs.iter().map(|r| r.1).chain(hyp.iter().map(|h| h.1)).fold(f64::MAX, f64::min);
    let mut block = start;
    while block <= end {
        let next = block + BLOCK_S;
        let r: Vec<&(String, f64, bool)> = refs.iter().filter(|x| x.1 >= block && x.1 < next).collect();
        let h: Vec<&(String, f64)> = hyp.iter().filter(|x| x.1 >= block && x.1 < next).collect();
        let rt: Vec<String> = r.iter().map(|x| x.0.clone()).collect();
        let ht: Vec<String> = h.iter().map(|x| x.0.clone()).collect();
        let (mut i, mut j, mut run): (usize, usize, Option<(f64, usize)>) = (0, 0, None);
        for op in align(&rt, &ht) {
            if op == Op::Del {
                let e = run.get_or_insert((r[i].1, 0));
                e.1 += 1;
            } else if let Some(g) = run.take() {
                gaps.push(g);
            }
            match op {
                Op::Hit => {
                    hits += 1;
                    if r[i].2 {
                        offsets.push(h[j].1 - r[i].1);
                    }
                    (i, j) = (i + 1, j + 1);
                }
                Op::Sub => {
                    subs += 1;
                    (i, j) = (i + 1, j + 1);
                }
                Op::Del => {
                    dels += 1;
                    i += 1;
                }
                Op::Ins => {
                    ins += 1;
                    j += 1;
                }
            }
        }
        gaps.extend(run);
        block = next;
    }
    gaps.sort_by_key(|g| std::cmp::Reverse(g.1));
    gaps.truncate(5);
    let n = refs.len().max(1) as f64;
    let mut abs: Vec<f64> = offsets.iter().map(|o: &f64| o.abs()).collect();
    abs.sort_by(f64::total_cmp);
    let mut signed = offsets.clone();
    signed.sort_by(f64::total_cmp);
    TranscriptEval {
        reference_words: refs.len(),
        words: hyp.len(),
        word_error_rate: (subs + dels + ins) as f64 / n,
        recall: hits as f64 / n,
        substitutions: subs,
        deletions: dels,
        insertions: ins,
        sync: Sync {
            cues: offsets.len(),
            median_offset_s: quantile(&signed, 0.5),
            median_abs_s: quantile(&abs, 0.5),
            p90_abs_s: quantile(&abs, 0.9),
            within_300ms: abs.iter().filter(|a| **a <= 0.3).count() as f64 / abs.len().max(1) as f64,
        },
        worst_gaps: gaps,
    }
}

/// How one clip candidate is cut.
#[derive(Debug, Clone, Serialize)]
pub struct ClipEval {
    pub rank: usize,
    pub length_s: f64,
    pub words: usize,
    /// The word before the clip ends a sentence (or a pause ≥ 0.6 s comes first).
    pub clean_start: bool,
    /// The clip's last word ends a sentence.
    pub clean_end: bool,
    /// A boundary falls inside a word.
    pub cuts_a_word: bool,
    /// The clip opens on a continuation ("and", "so", "but", "because"…), mid-thought.
    pub opens_mid_thought: bool,
}

/// Opening words that lean on what came before.
const CONTINUATIONS: &[&str] =
    &["and", "but", "so", "because", "or", "which", "then", "also", "aur", "lekin", "toh", "kyunki"];

fn ends_sentence(w: &Word) -> bool {
    w.text.trim_end_matches(['"', '\'', ')', '”', '’']).ends_with(['.', '?', '!', '…', '।'])
}

/// Boundary checks for each piece of each clip, against the run's words.
pub fn clips(candidates: &[crate::scorer::Candidate], words: &[Word]) -> Vec<ClipEval> {
    candidates
        .iter()
        .map(|c| {
            let pieces = c.pieces();
            // As the renderer decides: a word belongs to a part if its middle does.
            let overlaps = |w: &Word, &(a, b): &(f64, f64)| (a..b).contains(&((w.start + w.end) / 2.0));
            let inside: Vec<&Word> = words.iter().filter(|w| pieces.iter().any(|p| overlaps(w, p))).collect();
            // The word before the first piece's first word.
            let first = words.iter().position(|w| overlaps(w, &pieces[0]));
            let clean_start = match first {
                Some(i) if i > 0 => {
                    // Leading "And so…" trimmed off the sentence still counts as its start.
                    let mut k = i - 1;
                    while k > 0
                        && i - k < 3
                        && normalize(&words[k].text).first().is_some_and(|w| CONTINUATIONS.contains(&w.as_str()))
                    {
                        k -= 1;
                    }
                    ends_sentence(&words[k]) || ends_sentence(&words[i - 1]) || words[i].start - words[i - 1].end >= 0.6
                }
                _ => true,
            };
            let last = words.iter().rposition(|w| overlaps(w, &pieces[pieces.len() - 1]));
            let clean_end = last.is_some_and(|i| ends_sentence(&words[i]));
            let cuts_a_word = pieces.iter().any(|&(a, b)| {
                words
                    .iter()
                    .any(|w| (w.start + 0.05 < a && a < w.end - 0.05) || (w.start + 0.05 < b && b < w.end - 0.05))
            });
            ClipEval {
                rank: c.rank,
                length_s: pieces.iter().map(|(a, b)| b - a).sum(),
                words: inside.len(),
                clean_start,
                clean_end,
                cuts_a_word,
                opens_mid_thought: inside
                    .first()
                    .and_then(|w| normalize(&w.text).into_iter().next())
                    .is_some_and(|w| CONTINUATIONS.contains(&w.as_str())),
            }
        })
        .collect()
}

/// One exported video.
#[derive(Debug, Clone, Serialize)]
pub struct ExportEval {
    pub file: String,
    pub size: (u32, u32),
    /// Matches the format in its name (9x16 → 1080×1920, 16x9 → 1920×1080).
    pub right_size: bool,
    pub duration_s: f64,
    /// 9:16 only: share of sampled frames with a face, and with the face fully inside the frame.
    pub face_seen: Option<f64>,
    pub face_inside: Option<f64>,
    /// 9:16 only: share of the clip's source frames with a face (B-roll and slides have none), and how much of
    /// that the export keeps whole: `face_inside / source_face_seen`, at most 1.
    pub source_face_seen: Option<f64>,
    pub face_kept: Option<f64>,
    /// 9:16 only: median height of the face (share of the frame), its median distance from the frame's centre line,
    /// and how much it shakes between samples inside a shot (share of the frame's width).
    pub face_size: Option<f64>,
    pub face_off_centre: Option<f64>,
    pub face_shake: Option<f64>,
    /// 9:16 only: share of face samples that a caption covers.
    pub caption_on_face: Option<f64>,
    /// How the render framed it.
    pub framing: Option<FramingEval>,
    /// Pauses over 1 s left in the audio.
    pub long_pauses: usize,
    /// SRT cues ending after the video does.
    pub captions_past_end: usize,
    /// Words in the SRT against the words the clip kept.
    pub caption_coverage: Option<f64>,
    /// Fastest 5 % of the cues, characters per second.
    pub reading_speed: f64,
    /// Integrated loudness (LUFS) and true peak (dBFS).
    pub loudness: Option<f64>,
    pub peak: Option<f64>,
    /// Seconds of black picture.
    pub black_s: f64,
    /// 0–100, see `score`.
    pub score: f64,
}

/// What the render's plan did with a clip.
#[derive(Debug, Clone, Serialize, Default)]
pub struct FramingEval {
    /// Shots per layout, e.g. `{"follow": 4, "stack": 2}`.
    pub layouts: std::collections::BTreeMap<String, usize>,
    pub shots: usize,
    pub zooms: usize,
    /// Largest enlargement of a crop smaller than the source's height.
    pub max_upscale: f64,
    /// Fastest camera move, crop widths per second.
    pub max_camera_speed: f64,
}

/// Reads a render's saved plan.
fn framing_eval(plan: &serde_json::Value) -> FramingEval {
    let mut out = FramingEval::default();
    let src_h = plan["src"][1].as_f64().unwrap_or(f64::MAX);
    for s in plan["shots"].as_array().into_iter().flatten() {
        out.shots += 1;
        *out.layouts.entry(s["layout"].as_str().unwrap_or("?").to_string()).or_default() += 1;
        out.zooms += s["zooms"].as_array().map_or(0, Vec::len);
        for p in s["panels"].as_array().into_iter().flatten() {
            let (w, h, dest) =
                (p["w"].as_f64().unwrap_or(1.0), p["h"].as_f64().unwrap_or(1.0), p["dest_h"].as_f64().unwrap_or(0.0));
            // A crop as tall as the source is as sharp as this video gets.
            if h < src_h {
                out.max_upscale = out.max_upscale.max(dest / h.max(1.0));
            }
            let keys: Vec<(f64, f64)> = p["keys"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|k| (k[0].as_f64().unwrap_or(0.0), k[1].as_f64().unwrap_or(0.0)))
                .collect();
            for k in keys.windows(2) {
                out.max_camera_speed =
                    out.max_camera_speed.max((k[1].1 - k[0].1).abs() / (k[1].0 - k[0].0).max(0.05) / w.max(1.0));
            }
        }
    }
    out
}

/// Share of face samples under a caption: the caption is a band about 180 px tall at the shot's `caption_y`.
fn captions_on_faces(faces: &[crate::framing::Face], plan: &serde_json::Value) -> Option<f64> {
    let speed = plan["speed"].as_f64().unwrap_or(1.0);
    let shots = plan["shots"].as_array()?;
    let (mut hit, mut n) = (0, 0);
    for f in faces {
        let t = f.t * speed;
        let Some(s) =
            shots.iter().find(|s| t >= s["start"].as_f64().unwrap_or(0.0) && t < s["end"].as_f64().unwrap_or(0.0))
        else {
            continue;
        };
        let y = s["caption_y"].as_f64().unwrap_or(1380.0);
        let (top, bottom) = ((f.y - f.h * 0.45) * 1920.0, (f.y + f.h * 0.45) * 1920.0);
        n += 1;
        if y + 90.0 > top && y - 90.0 < bottom {
            hit += 1;
        }
    }
    Some(if n == 0 { 0.0 } else { hit as f64 / n as f64 })
}

/// Loudness, true peak and seconds of black from one ffmpeg pass.
async fn signal_checks(video: &Path) -> (Option<f64>, Option<f64>, f64) {
    let out = tokio::process::Command::new(crate::tools::resolve("ffmpeg"))
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(video)
        .args(["-af", "ebur128=peak=true", "-vf", "blackdetect=d=0.2:pix_th=0.08", "-f", "null", "-"])
        .kill_on_drop(true)
        .output()
        .await;
    let Ok(out) = out else { return (None, None, 0.0) };
    let log = String::from_utf8_lossy(&out.stderr);
    let summary = log.rsplit("Summary:").next().unwrap_or("");
    let after = |s: &str, key: &str| s.split(key).nth(1)?.split_whitespace().next()?.parse::<f64>().ok();
    let black = log.lines().filter_map(|l| after(l, "black_duration:")).sum();
    (after(summary, "I:"), after(summary, "Peak:"), black)
}

/// 0–100 from the checks that apply: each is worth its weight at target and fades to nothing.
pub fn score(e: &ExportEval) -> f64 {
    let (mut got, mut total) = (0.0, 0.0);
    let mut add = |weight: f64, value: f64| {
        got += weight * value.clamp(0.0, 1.0);
        total += weight;
    };
    add(10.0, f64::from(u8::from(e.right_size)));
    if let (Some(kept), Some(seen)) = (e.face_kept, e.source_face_seen)
        && seen > 0.2
    {
        add(30.0, kept / target::FACE_KEPT);
    }
    if let Some(h) = e.face_size {
        add(5.0, if h < 0.1 { h / 0.1 } else { 1.0 });
    }
    if let Some(v) = e.face_shake {
        add(10.0, 1.0 - (v - target::FACE_SHAKE).max(0.0) / target::FACE_SHAKE);
    }
    if let Some(v) = e.caption_on_face {
        add(10.0, 1.0 - (v - target::CAPTION_ON_FACE).max(0.0) * 4.0);
    }
    if let Some(v) = e.caption_coverage {
        add(10.0, v / target::CAPTION_COVERAGE);
    }
    add(5.0, 1.0 - (e.reading_speed - target::READING_SPEED).max(0.0) / target::READING_SPEED);
    if let Some(l) = e.loudness {
        add(10.0, 1.0 - ((l - target::LOUDNESS).abs() - target::LOUDNESS_RANGE).max(0.0) / 6.0);
    }
    add(5.0, 1.0 - e.black_s / 2.0);
    if let Some(u) = e.framing.as_ref().map(|f| f.max_upscale) {
        add(5.0, 1.0 - (u - target::MAX_UPSCALE).max(0.0) / target::MAX_UPSCALE);
    }
    if total > 0.0 { got / total * 100.0 } else { 0.0 }
}

/// Where an export came from: the source video, the clip's pieces (source seconds) and the source's size.
pub type ClipSource<'a> = (&'a Path, Vec<(f64, f64)>, (u32, u32));

/// Share of sampled frames with at least one face in `media`'s `pieces`, whose frames are `src` pixels. Blocking.
fn face_share(model: &Path, media: &Path, pieces: &[(f64, f64)], src: (u32, u32)) -> Result<f64> {
    let (mut seen, mut frames) = (0usize, 0.0);
    for &(a, b) in pieces {
        // Faces big enough to be the subject: a tiny face in a wide stage shot over a slide is not one.
        let mut faces: Vec<_> =
            crate::framing::detect(model, media, a, b, src)?.into_iter().filter(|f| f.h >= 0.06).collect();
        faces.dedup_by(|y, x| (x.t - y.t).abs() < 1e-6);
        seen += faces.len();
        frames += (b - a) * crate::camera::ANALYSIS_FPS;
    }
    Ok((seen as f64 / frames.max(1.0)).min(1.0))
}

/// Probes an exported clip: size, length, faces (9:16, against `source`: the video and the clip's pieces),
/// pauses and SRT timing.
pub async fn export(
    video: &Path,
    face_model: Option<&Path>,
    source: Option<ClipSource<'_>>,
    plan: Option<&serde_json::Value>,
) -> Result<ExportEval> {
    let info = crate::source::probe(&video.to_string_lossy()).await?;
    let size = (info.width.unwrap_or(0) as u32, info.height.unwrap_or(0) as u32);
    let name = video.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let vertical = name.contains("9x16");
    let right_size = if vertical { size == (1080, 1920) } else { size == (1920, 1080) };
    let duration = info.duration_s.unwrap_or(0.0);

    let (mut face_seen, mut face_inside, mut source_face_seen, mut face_kept) = (None, None, None, None);
    let (mut face_size, mut face_off_centre, mut face_shake, mut caption_on_face) = (None, None, None, None);
    if vertical && let Some(model) = face_model {
        let (model_file, path) = (model.to_path_buf(), video.to_path_buf());
        let faces =
            tokio::task::spawn_blocking(move || crate::framing::detect(&model_file, &path, 0.0, duration, size))
                .await
                .context("face check stopped")??;
        // The best face of each sampled frame (detect lists them best first).
        let mut faces = faces;
        faces.dedup_by(|b, a| (a.t - b.t).abs() < 1e-6);
        let frames = (duration * crate::camera::ANALYSIS_FPS).max(1.0);
        // A face's box is about 0.8 of its height wide; in a 9:16 frame that is 0.8 × h × 16/9 of the width.
        let inside = faces.iter().filter(|f| (f.x - 0.5).abs() + 0.4 * f.h * 16.0 / 9.0 <= 0.5).count();
        face_seen = Some((faces.len() as f64 / frames).min(1.0));
        face_inside = Some((inside as f64 / frames).min(1.0));
        if !faces.is_empty() {
            let median = |mut v: Vec<f64>| {
                v.sort_by(f64::total_cmp);
                v[v.len() / 2]
            };
            face_size = Some(median(faces.iter().map(|f| f.h).collect()));
            face_off_centre = Some(median(faces.iter().map(|f| (f.x - 0.5).abs()).collect()));
            // Consecutive samples of the same face; a bigger jump is a cut or another person.
            let steps: Vec<f64> = faces
                .windows(2)
                .filter(|p| p[1].t - p[0].t < 0.3)
                .map(|p| (p[1].x - p[0].x).abs())
                .filter(|d| *d < 0.12)
                .collect();
            if !steps.is_empty() {
                face_shake = Some(steps.iter().sum::<f64>() / steps.len() as f64);
            }
        }
        caption_on_face = plan.and_then(|p| captions_on_faces(&faces, p));
        if let Some((media, pieces, src)) = source {
            let (model, media) = (model.to_path_buf(), media.to_path_buf());
            let share = tokio::task::spawn_blocking(move || face_share(&model, &media, &pieces, src))
                .await
                .context("face check stopped")??;
            source_face_seen = Some(share);
            face_kept = Some(if share > 0.0 { (inside as f64 / frames / share).min(1.0) } else { 1.0 });
        }
    }

    let work = crate::transcribe::Workspace::new()?;
    let wav = crate::audio::extract_wav(video, &work.0.join("a.wav")).await?;
    let env = crate::audio::loudness_envelope(&wav)?;
    let long_pauses = crate::audio::silences(&env, crate::audio::ENVELOPE_RATE, 1.0).len();
    let srt = std::fs::read_to_string(video.with_extension("srt")).unwrap_or_default();
    let cues = parse_captions(&srt);
    let captions_past_end = cues.iter().filter(|c| c.end > duration + 0.1).count();
    let mut speeds: Vec<f64> =
        cues.iter().map(|c| c.text.chars().count() as f64 / (c.end - c.start).max(0.2)).collect();
    speeds.sort_by(f64::total_cmp);
    let spoken: usize = cues.iter().map(|c| normalize(&c.text).len()).sum();
    let caption_coverage =
        plan.and_then(|p| p["words"].as_f64()).filter(|n| *n > 0.0).map(|n| (spoken as f64 / n).min(1.0));
    let (loudness, peak, black_s) = signal_checks(video).await;
    let mut out = ExportEval {
        file: name,
        size,
        right_size,
        duration_s: duration,
        face_seen,
        face_inside,
        source_face_seen,
        face_kept,
        face_size,
        face_off_centre,
        face_shake,
        caption_on_face,
        framing: plan.map(framing_eval),
        long_pauses,
        captions_past_end,
        caption_coverage,
        reading_speed: quantile(&speeds, 0.95).max(0.0),
        loudness,
        peak,
        black_s,
        score: 0.0,
    };
    out.score = score(&out);
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub run: String,
    pub transcript: Option<TranscriptEval>,
    pub clips: Vec<ClipEval>,
    pub exports: Vec<ExportEval>,
    /// Mean export score, 0–100.
    pub score: f64,
    /// One line per check that misses its target.
    pub failures: Vec<String>,
}

/// Targets a good run meets; misses are listed in `Report::failures`.
pub mod target {
    pub const WER: f64 = 0.15;
    pub const RECALL: f64 = 0.88;
    pub const SYNC_MEDIAN_ABS_S: f64 = 0.25;
    pub const SYNC_WITHIN_300MS: f64 = 0.7;
    pub const CLEAN_BOUNDARIES: f64 = 0.8;
    /// Of the moments the source shows a face, the share the 9:16 export shows it whole.
    pub const FACE_KEPT: f64 = 0.8;
    /// Mean move of the face between samples inside a shot, share of the frame's width.
    pub const FACE_SHAKE: f64 = 0.04;
    pub const CAPTION_ON_FACE: f64 = 0.1;
    pub const CAPTION_COVERAGE: f64 = 0.95;
    /// Characters per second a viewer can read.
    pub const READING_SPEED: f64 = 30.0;
    pub const LOUDNESS: f64 = -14.0;
    pub const LOUDNESS_RANGE: f64 = 2.0;
    pub const PEAK: f64 = -0.5;
    pub const MAX_UPSCALE: f64 = 3.6;
    /// Most punch-ins a minute: more is restless.
    pub const ZOOMS_PER_MINUTE: f64 = 10.0;
}

/// Evaluates a saved run: its transcript against `reference` captions (when given), its clips and its exports.
pub async fn run(id: &str, reference: Option<&Path>, progress: crate::models::ProgressFn<'_>) -> Result<Report> {
    let (run, words) = crate::pipeline::load_run(id)?;
    let mut failures = Vec::new();
    let transcript = match reference {
        Some(path) => {
            let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
            let t = transcript(&parse_captions(&text), &words, None);
            if t.word_error_rate > target::WER {
                failures.push(format!(
                    "transcript: word error rate {:.1}% > {:.0}%",
                    t.word_error_rate * 100.0,
                    target::WER * 100.0
                ));
            }
            if t.recall < target::RECALL {
                failures.push(format!(
                    "transcript: heard {:.1}% of the words < {:.0}%",
                    t.recall * 100.0,
                    target::RECALL * 100.0
                ));
            }
            if t.sync.median_abs_s > target::SYNC_MEDIAN_ABS_S || t.sync.within_300ms < target::SYNC_WITHIN_300MS {
                failures.push(format!(
                    "transcript: sync median {:.2} s (offset {:+.2} s), {:.0}% within 0.3 s",
                    t.sync.median_abs_s,
                    t.sync.median_offset_s,
                    t.sync.within_300ms * 100.0
                ));
            }
            Some(t)
        }
        None => None,
    };

    let clips = clips(&run.candidates, &words);
    let share =
        |f: &dyn Fn(&ClipEval) -> bool| clips.iter().filter(|c| f(c)).count() as f64 / clips.len().max(1) as f64;
    let (starts, ends) = (share(&|c| c.clean_start), share(&|c| c.clean_end));
    if starts < target::CLEAN_BOUNDARIES || ends < target::CLEAN_BOUNDARIES {
        failures.push(format!(
            "clips: {:.0}% start and {:.0}% end on a sentence boundary",
            starts * 100.0,
            ends * 100.0
        ));
    }
    for c in clips.iter().filter(|c| c.cuts_a_word) {
        failures.push(format!("clip #{}: a cut falls inside a word", c.rank));
    }
    if share(&|c| c.opens_mid_thought) > 1.0 - target::CLEAN_BOUNDARIES {
        failures.push(format!("clips: {:.0}% open mid-thought", share(&|c| c.opens_mid_thought) * 100.0));
    }

    let model = crate::framing::model(progress).await.ok();
    let mut exports = Vec::new();
    for video in crate::pipeline::exported(&run) {
        progress(crate::models::Progress {
            stage: format!("Checking {}", Path::new(&video).display()),
            done: 0,
            total: None,
        });
        let src = (run.media_info.width.unwrap_or(1920) as u32, run.media_info.height.unwrap_or(1080) as u32);
        let pieces = crate::pipeline::rank_of(&video)
            .and_then(|r| run.candidates.iter().find(|c| c.rank == r))
            .map(|c| c.pieces());
        let source = pieces.map(|p| (Path::new(&run.media), p, src));
        let stem = Path::new(&video).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let plan: Option<serde_json::Value> =
            std::fs::read(crate::render::plan_file(&run, &stem)).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let e = export(Path::new(&video), model.as_deref(), source, plan.as_ref()).await?;
        if !e.right_size {
            failures.push(format!("{}: {}×{} is the wrong size", e.file, e.size.0, e.size.1));
        }
        if let Some(kept) = e.face_kept.filter(|&v| v < target::FACE_KEPT && e.source_face_seen.unwrap_or(0.0) > 0.2) {
            failures.push(format!(
                "{}: shows a whole face {:.0}% of the time the source has one ({:.0}% → {:.0}%)",
                e.file,
                kept * 100.0,
                e.source_face_seen.unwrap_or(0.0) * 100.0,
                e.face_inside.unwrap_or(0.0) * 100.0
            ));
        }
        if e.captions_past_end > 0 {
            failures.push(format!("{}: {} captions run past the end", e.file, e.captions_past_end));
        }
        let mut miss = |bad: bool, what: String| {
            if bad {
                failures.push(format!("{}: {what}", e.file));
            }
        };
        if let Some(v) = e.face_shake {
            miss(v > target::FACE_SHAKE, format!("the face shakes ({:.3} of the width per sample)", v));
        }
        if let Some(v) = e.caption_on_face {
            miss(v > target::CAPTION_ON_FACE, format!("captions cover the face {:.0}% of the time", v * 100.0));
        }
        if let Some(v) = e.caption_coverage {
            miss(v < target::CAPTION_COVERAGE, format!("captions hold {:.0}% of the words", v * 100.0));
        }
        miss(
            e.reading_speed > target::READING_SPEED,
            format!("captions read at {:.0} characters a second", e.reading_speed),
        );
        if let Some(l) = e.loudness {
            miss((l - target::LOUDNESS).abs() > target::LOUDNESS_RANGE, format!("loudness {l:.1} LUFS"));
        }
        if let Some(p) = e.peak {
            miss(p > target::PEAK, format!("peaks at {p:.1} dBFS"));
        }
        miss(e.black_s > 0.5, format!("{:.1} s of black", e.black_s));
        if let Some(f) = &e.framing {
            miss(f.max_upscale > target::MAX_UPSCALE, format!("a crop is enlarged {:.1}×", f.max_upscale));
            let per_minute = f.zooms as f64 / (e.duration_s / 60.0).max(0.1);
            miss(per_minute > target::ZOOMS_PER_MINUTE, format!("{per_minute:.0} zooms a minute"));
        }
        exports.push(e);
    }
    let score = exports.iter().map(|e| e.score).sum::<f64>() / exports.len().max(1) as f64;
    Ok(Report { run: id.to_string(), transcript, clips, exports, score, failures })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(t: &str, s: f64) -> Word {
        Word { text: t.into(), start: s, end: s + 0.3 }
    }

    #[test]
    fn reads_vtt_and_srt() {
        let vtt = "WEBVTT\nKind: captions\n\n00:00:12.520 --> 00:00:15.536 align:start\nSam Roe:\nAlex, hey, welcome back.\n\n00:00:16.960 --> 00:00:18.536\n(Laughter)\n";
        let cues = parse_captions(vtt);
        assert_eq!(cues.len(), 2);
        assert_eq!((cues[0].start, cues[0].end), (12.52, 15.536));
        assert_eq!(normalize(&cues[0].text), ["alex", "hey", "welcome", "back"]);
        assert!(normalize(&cues[1].text).is_empty(), "sound tags are not speech");
        let srt = "1\n00:01:02,500 --> 00:01:03,000\nHi there\n";
        assert_eq!(parse_captions(srt)[0].start, 62.5);
        assert_eq!(normalize("CA: It's “great” — isn't it?"), ["it's", "great", "isn't", "it"]);
        assert_eq!(normalize("I said: Go"), ["i", "said", "go"], "a colon mid-sentence is not a label");
    }

    #[test]
    fn scores_errors_and_sync() {
        let reference = parse_captions(
            "00:00:01.000 --> 00:00:03.000\nthe quick brown fox\n\n00:00:05.000 --> 00:00:07.000\njumps over the lazy dog\n",
        );
        // "brown" missed, "lazy" misheard as "crazy", an extra "um"; captions 0.2 s late.
        let hyp = [
            w("The", 1.2),
            w("quick", 1.6),
            w("fox", 2.2),
            w("jumps", 5.2),
            w("over", 5.5),
            w("um", 5.7),
            w("the", 5.9),
            w("crazy", 6.2),
            w("dog.", 6.6),
        ];
        let t = transcript(&reference, &hyp, None);
        assert_eq!((t.reference_words, t.substitutions, t.deletions, t.insertions), (9, 1, 1, 1));
        assert!((t.word_error_rate - 3.0 / 9.0).abs() < 1e-9 && (t.recall - 7.0 / 9.0).abs() < 1e-9);
        assert_eq!(t.sync.cues, 2);
        assert!((t.sync.median_offset_s - 0.2).abs() < 1e-9 && t.sync.within_300ms == 1.0, "{:?}", t.sync);
        assert_eq!(t.worst_gaps, [(1.0 + 2.0 * 2.0 / 4.0, 1)], "the missed word and where");
    }

    #[test]
    fn checks_clip_boundaries() {
        let words = [
            w("Intro.", 0.0),
            w("Why", 1.0),
            w("not?", 1.4),
            w("Truly", 2.0),
            w("it", 2.4),
            w("works.", 2.8),
            w("And", 3.4),
        ];
        let cand = |start: f64, end: f64| crate::scorer::Candidate {
            start,
            end,
            score: 0,
            rank: 1,
            title: String::new(),
            reasons: vec![],
            keywords: vec![],
            segments: vec![],
            layouts: vec![],
        };
        let good = &clips(&[cand(0.95, 3.2)], &words)[0];
        assert!(good.clean_start && good.clean_end && !good.cuts_a_word, "{good:?}");
        let bad = &clips(&[cand(2.6, 3.6)], &words)[0];
        assert!(!bad.clean_start && !bad.clean_end && bad.cuts_a_word, "{bad:?}");
    }
}
