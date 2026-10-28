//! Render clips from a run: dead-air cuts, 9:16 / 16:9 framing, burned word-highlight captions,
//! loudness normalisation, hardware encoding; plus an SRT and a Premiere (FCP 7 XML) timeline per clip.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::Word;
use crate::framing::{Face, Framing, Layout};
use crate::models::{Progress, ProgressFn};
use crate::pacing::{self, WindowWord};
use crate::pipeline::{self, Run};

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

#[derive(Debug, Clone, Deserialize)]
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

/// Source time → time in the cut clip (before speed), or `None` if that moment was cut.
fn to_clip_time(keeps: &[(f64, f64)], t: f64) -> Option<f64> {
    let mut offset = 0.0;
    for &(a, b) in keeps {
        if (a..b).contains(&t) {
            return Some(offset + t - a);
        }
        offset += b - a;
    }
    None
}

/// `[vcat] → [vf]`: 9:16 in `layout`, or 16:9 letterboxed.
fn framing_chain(format: Format, layout: Layout, faces: &[Face], duration: f64, src: (u32, u32)) -> String {
    match format {
        Format::Vertical => crate::framing::vertical_chain(layout, faces, duration, src, ("vcat", "vf")),
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
fn filter_graph(keeps: &[(f64, f64)], seek: f64, frame_chain: &str, speed: f64, ass_file: Option<&str>) -> String {
    let mut g = String::new();
    for (i, &(a, b)) in keeps.iter().enumerate() {
        let (a, b) = (a - seek, b - seek);
        let fade_out = (b - a - 0.012).max(0.0);
        g.push_str(&format!(
            "[0:v]trim=start={a:.3}:end={b:.3},setpts=PTS-STARTPTS[v{i}];\
             [0:a]atrim=start={a:.3}:end={b:.3},asetpts=PTS-STARTPTS,afade=t=in:d=0.012,afade=t=out:st={fade_out:.3}:d=0.012[a{i}];"
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
        Some(f) => g.push_str(&format!("[vl]ass=filename={f}:fontsdir={}:shaping=complex[vout];", fonts_dir())),
        None => g.push_str("[vl]null[vout];"),
    }
    g.push_str("[ac]loudnorm=I=-14:TP=-1.5:LRA=11,aresample=48000[aout]");
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
    let mut child = Command::new("ffmpeg")
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

/// Render the given candidates (1-based ranks) of a run into `<run>/clips`.
pub async fn render_clips(
    run_id: &str,
    ranks: &[usize],
    opts: &RenderOptions,
    progress: ProgressFn<'_>,
) -> Result<Vec<RenderedClip>> {
    let (run, words) = pipeline::load_run(run_id)?;
    let out_dir = Path::new(&run.dir).join("clips");
    std::fs::create_dir_all(&out_dir)?;
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
        let (keeps, mut clip_words) = plan_cuts(&words, c.start, c.end, opts.cut_dead_air);
        for w in &mut clip_words {
            (w.start, w.end) = (w.start / speed, w.end / speed);
        }
        let duration = keeps.iter().map(|(a, b)| b - a).sum::<f64>() / speed;
        let stem = format!("clip_{rank:02}_{}", if opts.format == Format::Vertical { "9x16" } else { "16x9" });

        let srt_path = out_dir.join(format!("{stem}.srt"));
        std::fs::write(&srt_path, crate::captions::srt(&clip_words))?;
        let ass_name = format!("{stem}.ass");
        let overlay = opts.captions || opts.title;
        if overlay {
            let shown = if opts.captions { clip_words.as_slice() } else { &[] };
            let title = opts.title.then_some((c.title.as_str(), duration));
            let style = crate::captions::preset(&opts.caption_style);
            std::fs::write(out_dir.join(&ass_name), crate::captions::ass(shown, w, h, style, title))?;
        }
        // ponytail: the Premiere timeline stays at 1× (no time remap yet); speed is in the MP4 only.
        let xml_path = out_dir.join(format!("{stem}.xml"));
        std::fs::write(&xml_path, premiere_xml(&run, &c.title, &keeps, opts.format))?;

        let src = (run.media_info.width.unwrap_or(1920) as u32, run.media_info.height.unwrap_or(1080) as u32);
        let mut faces: Vec<Face> = Vec::new();
        if opts.format == Format::Vertical && opts.framing != Framing::Classic {
            let model = crate::framing::model(progress).await?;
            progress(Progress { stage: format!("Finding the speaker in clip #{rank}"), done: 0, total: None });
            let (media, start, end) = (std::path::PathBuf::from(&run.media), c.start, c.end);
            let found = tokio::task::spawn_blocking(move || crate::framing::detect(&model, &media, start, end, src))
                .await
                .context("face detection stopped")??;
            // Onto the cut clip's timeline; faces in cut-out moments are dropped.
            faces = found.into_iter().filter_map(|f| Some(Face { t: to_clip_time(&keeps, f.t)?, ..f })).collect();
        }
        let (mut layout, unsure) = crate::framing::choose(opts.framing, &faces, duration * speed);
        if opts.format == Format::Vertical && opts.framing == Framing::Smart && opts.vision_check && unsure {
            progress(Progress { stage: format!("Checking clip #{rank} with AI vision"), done: 0, total: None });
            let frame = out_dir.join(format!("{stem}.vision.jpg"));
            let mid = format!("{:.3}", (c.start + c.end) / 2.0);
            let grabbed = Command::new("ffmpeg")
                .args(["-hide_banner", "-v", "error", "-y", "-ss", &mid, "-i", &run.media, "-frames:v", "1"])
                .args(["-vf", "scale=960:-2"])
                .arg(&frame)
                .status()
                .await
                .is_ok_and(|s| s.success());
            if grabbed && let Some(seen) = crate::framing::vision_layout(&frame).await {
                layout = seen;
            }
            std::fs::remove_file(&frame).ok();
        }
        let frame_chain = framing_chain(opts.format, layout, &faces, duration * speed, src);

        // Input seek (fast) to the clip start; the graph trims relative to it.
        let seek = c.start;
        let graph_name = format!("{stem}.graph.txt");
        std::fs::write(
            out_dir.join(&graph_name),
            filter_graph(&keeps, seek, &frame_chain, speed, overlay.then_some(ass_name.as_str())),
        )?;
        let video_name = format!("{stem}.mp4");
        let mut args: Vec<String> = vec![
            "-ss".into(),
            format!("{seek:.3}"),
            "-to".into(),
            format!("{:.3}", c.end + 0.5),
            "-i".into(),
            run.media.clone(),
        ];
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

        done.push(RenderedClip {
            rank,
            title: c.title.clone(),
            video: out_dir.join(&video_name).to_string_lossy().into_owned(),
            srt: srt_path.to_string_lossy().into_owned(),
            premiere_xml: xml_path.to_string_lossy().into_owned(),
            duration_s: duration,
            removed_s: (c.end - c.start) - duration * speed,
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
        let classic = framing_chain(Format::Vertical, Layout::Whole, &[], 5.0, (1920, 1080));
        let g = filter_graph(&[(10.0, 12.0), (13.0, 15.5)], 10.0, &classic, 1.0, Some("c.ass"));
        assert_eq!(g.matches("[0:v]trim").count(), 2);
        assert!(g.contains("trim=start=3.000:end=5.500") && g.contains("concat=n=2:v=1:a=1"));
        assert!(g.contains("[vcat]split=2") && g.contains("[vf];[vf]null[vl]"), "framing then speed: {g}");
        assert!(g.contains("shaping=complex") && g.ends_with("[aout]") && !g.contains("atempo"));
        let wide = framing_chain(Format::Horizontal, Layout::Follow, &[], 5.0, (1920, 1080));
        let fast = filter_graph(&[(0.0, 5.0)], 0.0, &wide, 1.5, None);
        assert!(fast.contains("setpts=PTS/1.5[vl]") && fast.contains("atempo=1.5[ac]"), "{fast}");
    }

    #[test]
    fn source_times_map_onto_the_cut_clip() {
        let keeps = [(10.0, 12.0), (15.0, 16.0)];
        assert_eq!(to_clip_time(&keeps, 11.0), Some(1.0));
        assert_eq!(to_clip_time(&keeps, 13.0), None, "cut out");
        assert_eq!(to_clip_time(&keeps, 15.5), Some(2.5));
    }

    #[test]
    fn file_urls_are_percent_encoded() {
        assert_eq!(file_url(Path::new(r"C:\a b\x#.mp4")), "file://localhost/C:/a%20b/x%23.mp4");
    }
}
