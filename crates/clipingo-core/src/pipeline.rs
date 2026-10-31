//! Runs: source → (download) → audio → transcript → clip candidates, saved under `<data_dir>/runs/<id>`
//! so clips can be rendered later without transcribing again.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::Word;
use crate::models::{Progress, ProgressFn};
use crate::scorer::{self, Candidate, FindOptions, Loudness};
use crate::source::{self, SourceInfo, SourceKind};
use crate::transcribe::{self, TranscribeOptions, Transcript, Workspace};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSummary {
    pub model: String,
    pub language: Option<String>,
    pub words: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub dir: String,
    /// What the user picked (YouTube metadata for links).
    pub source: SourceInfo,
    /// Local video file the clips are cut from.
    pub media: String,
    pub media_info: SourceInfo,
    pub transcript: TranscriptSummary,
    pub candidates: Vec<Candidate>,
    /// Where this run's clips are exported (`output_dir()/<title> (<id>)`); empty for runs made before it existed.
    #[serde(default)]
    pub clips_dir: String,
}

impl Run {
    /// Exported clips folder (older runs kept theirs inside the run folder).
    pub fn clips_dir(&self) -> PathBuf {
        if self.clips_dir.is_empty() { Path::new(&self.dir).join("clips") } else { PathBuf::from(&self.clips_dir) }
    }
}

/// A folder name from a video title: no characters Windows rejects, at most 60 characters, plus the run id.
fn clips_folder_name(title: &str, id: &str) -> String {
    let clean: String = title
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let short: String = clean.chars().take(60).collect();
    let short = short.trim_end_matches(['.', ' ']);
    if short.is_empty() { id.to_string() } else { format!("{short} ({id})") }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RunOptions {
    #[serde(default)]
    pub transcribe: TranscribeOptions,
    #[serde(default)]
    pub find: Option<FindOptions>,
}

/// Saved runs. `CLIPINGO_RUNS_DIR` overrides (tests keep out of your Library).
pub fn runs_dir() -> PathBuf {
    std::env::var_os("CLIPINGO_RUNS_DIR")
        .filter(|d| !d.is_empty())
        .map_or_else(|| crate::data_dir().join("runs"), PathBuf::from)
}

fn new_run_id() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    format!("{}-{:04x}", now.as_secs(), (now.subsec_nanos() ^ std::process::id()) & 0xffff)
}

fn stage(progress: ProgressFn<'_>, s: &str) {
    progress(Progress { stage: s.into(), done: 0, total: None });
}

/// Transcript plus the loudness envelope, from one audio extraction.
async fn transcribe_with_loudness(
    media: &Path,
    duration: Option<f64>,
    opts: &TranscribeOptions,
    progress: ProgressFn<'_>,
) -> Result<(Transcript, Vec<f32>)> {
    let work = Workspace::new()?;
    stage(progress, "Extracting audio");
    let wav = crate::audio::extract_wav(media, &work.0.join("audio.wav")).await?;
    let envelope = crate::audio::loudness_envelope(&wav)?;
    let duration = duration.unwrap_or(envelope.len() as f64 / crate::audio::ENVELOPE_RATE);
    let transcript = transcribe::transcribe(media, &wav, duration, opts, &work.0, progress).await?;
    Ok((transcript, envelope))
}

async fn local_file(input: &str) -> Result<SourceInfo> {
    let info = source::probe(input).await?;
    if info.kind != SourceKind::File {
        bail!("expected a local file: {input}");
    }
    Ok(info)
}

pub async fn transcribe_file(input: &str, opts: &TranscribeOptions, progress: ProgressFn<'_>) -> Result<Transcript> {
    let info = local_file(input).await?;
    Ok(transcribe_with_loudness(Path::new(&info.input), info.duration_s, opts, progress).await?.0)
}

/// Download (for links), transcribe and score; the run is saved before returning.
pub async fn create_run(input: &str, opts: &RunOptions, progress: ProgressFn<'_>) -> Result<Run> {
    stage(progress, "Reading source");
    let source = source::probe(input).await?;
    let id = new_run_id();
    let dir = runs_dir().join(&id);
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;

    let media_info = match source.kind {
        SourceKind::File => source.clone(),
        SourceKind::Youtube => {
            let dest = dir.join("source.mp4");
            source::download_youtube(&source.input, &dest, progress).await?;
            local_file(&dest.to_string_lossy()).await?
        }
    };
    let (mut transcript, envelope) =
        transcribe_with_loudness(Path::new(&media_info.input), media_info.duration_s, &opts.transcribe, progress)
            .await?;
    // Word segments swallow pauses; trim them to the audio so pacing and captions see the silences.
    let quiet = crate::audio::silences(&envelope, crate::audio::ENVELOPE_RATE, 0.25);
    crate::audio::refine_words(&mut transcript.words, &quiet);
    if transcript.words.is_empty() {
        bail!("no speech found in {}", source.title);
    }
    stage(progress, "Finding moments");
    let loud = Loudness { rms: &envelope, rate: crate::audio::ENVELOPE_RATE };
    let candidates = scorer::find(&transcript.words, &opts.find.clone().unwrap_or_default(), Some(&loud));

    std::fs::write(dir.join("transcript.json"), serde_json::to_vec(&transcript.words)?)?;
    let run = Run {
        id: id.clone(),
        dir: dir.to_string_lossy().into_owned(),
        media: media_info.input.clone(),
        source: source.clone(),
        media_info,
        transcript: TranscriptSummary {
            model: transcript.model,
            language: transcript.language,
            words: transcript.words.len(),
        },
        candidates,
        clips_dir: crate::output_dir().join(clips_folder_name(&source.title, &id)).to_string_lossy().into_owned(),
    };
    std::fs::write(dir.join("run.json"), serde_json::to_vec_pretty(&run)?)?;
    Ok(run)
}

/// A saved run with the clips rendered from it so far (Library).
#[derive(Debug, Clone, Serialize)]
pub struct RunListing {
    #[serde(flatten)]
    pub run: Run,
    /// Rendered `.mp4` files.
    pub clips: Vec<String>,
}

/// The exported `.mp4` files of the run's clips, by name (files of removed clips stay on disk but are not listed).
pub fn exported(run: &Run) -> Vec<String> {
    let mut clips = clips_folder_videos(run);
    clips.retain(|v| rank_of(v).is_some_and(|r| run.candidates.iter().any(|c| c.rank == r)));
    clips
}

fn clips_folder_videos(run: &Run) -> Vec<String> {
    let mut clips: Vec<String> = std::fs::read_dir(run.clips_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|c| c.path())
        .filter(|p| p.extension().is_some_and(|x| x == "mp4"))
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    clips.sort();
    clips
}

/// `…/clip_07_9x16.mp4` → 7.
pub fn rank_of(video: &str) -> Option<usize> {
    Path::new(video).file_name()?.to_str()?.strip_prefix("clip_")?.split('_').next()?.parse().ok()
}

/// Every saved run, newest first. Unreadable runs are skipped.
pub fn list_runs() -> Vec<RunListing> {
    let mut out: Vec<RunListing> = std::fs::read_dir(runs_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let run: Run = serde_json::from_slice(&std::fs::read(e.path().join("run.json")).ok()?).ok()?;
            Some(RunListing { clips: exported(&run), run })
        })
        .collect();
    // Ids start with the creation time in seconds.
    out.sort_by(|a, b| b.run.id.cmp(&a.run.id));
    out
}

/// A saved run and its words. `id` must be a plain run id (no paths).
pub fn load_run(id: &str) -> Result<(Run, Vec<Word>)> {
    let run = read_run(id)?;
    let words: Vec<Word> = serde_json::from_slice(&std::fs::read(Path::new(&run.dir).join("transcript.json"))?)?;
    Ok((run, words))
}

/// A saved run without its transcript.
fn read_run(id: &str) -> Result<Run> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        bail!("invalid run id");
    }
    let file = runs_dir().join(id).join("run.json");
    Ok(serde_json::from_slice(&std::fs::read(file).with_context(|| format!("run {id} not found"))?)?)
}

/// JPEG frames of the run's video at `times` (seconds), `height` pixels tall: the editor's filmstrip and clip
/// cards. Cached in the run folder; a frame that cannot be read comes back as "".
pub async fn thumbs(id: &str, times: &[f64], height: u32) -> Result<Vec<String>> {
    let run = read_run(id)?;
    let dir = Path::new(&run.dir).join("thumbs");
    std::fs::create_dir_all(&dir)?;
    let height = height.clamp(24, 360);
    let paths: Vec<PathBuf> =
        times.iter().map(|t| dir.join(format!("{height}_{}.jpg", (t.max(0.0) * 10.0).round() as u64))).collect();
    let mut missing: Vec<(f64, PathBuf)> =
        times.iter().zip(&paths).filter(|(_, p)| !p.exists()).map(|(t, p)| (*t, p.clone())).collect();
    missing.dedup_by(|a, b| a.1 == b.1);
    // ponytail: 6 ffmpeg seeks at a time; one decode pass per range if long filmstrips get slow.
    for chunk in missing.chunks(6) {
        let mut set = tokio::task::JoinSet::new();
        for (t, out) in chunk {
            let (t, out, media) = (*t, out.clone(), run.media.clone());
            set.spawn(async move {
                tokio::process::Command::new(crate::tools::resolve("ffmpeg"))
                    .args(["-hide_banner", "-v", "error", "-y", "-ss", &format!("{t:.2}"), "-i", &media])
                    .args(["-frames:v", "1", "-vf", &format!("scale=-2:{height}"), "-q:v", "5"])
                    .arg(&out)
                    .kill_on_drop(true)
                    .output()
                    .await
            });
        }
        while set.join_next().await.is_some() {}
    }
    Ok(paths.iter().map(|p| if p.exists() { p.to_string_lossy().into_owned() } else { String::new() }).collect())
}

fn save_run(run: &Run) -> Result<()> {
    std::fs::write(Path::new(&run.dir).join("run.json"), serde_json::to_vec_pretty(run)?)?;
    Ok(())
}

/// Words overlapping `start..end` (the same test the renderer uses).
fn inside(w: &Word, start: f64, end: f64) -> bool {
    w.end > start && w.start < end
}

/// A run's words in `start..end`: the editor's caption text for a clip.
pub fn clip_words(id: &str, start: f64, end: f64) -> Result<Vec<Word>> {
    Ok(load_run(id)?.1.into_iter().filter(|w| inside(w, start, end)).collect())
}

/// An editor change to one clip: new in / out points (source seconds) and title, or the source ranges it plays one
/// after another (`segments`, cut and stitched on the editor's timeline; then `start` / `end` are ignored).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ClipEdit {
    pub rank: usize,
    #[serde(default)]
    pub start: f64,
    #[serde(default)]
    pub end: f64,
    pub title: String,
    #[serde(default)]
    pub segments: Vec<(f64, f64)>,
}

/// Shortest piece the timeline keeps.
const MIN_SEGMENT_S: f64 = 0.2;

/// Every part inside the video and at least `MIN_SEGMENT_S`, the clip at least 1 s. The slack absorbs the editor's
/// rounding (a part trimmed to exactly the minimum).
fn check_pieces(pieces: &[(f64, f64)], duration: f64) -> Result<()> {
    let inside = |&(a, b): &(f64, f64)| a >= 0.0 && b <= duration + 0.5 && b - a >= MIN_SEGMENT_S - 1e-3;
    let total: f64 = pieces.iter().map(|(a, b)| b - a).sum();
    if !(pieces.iter().all(inside) && total >= 1.0 - 1e-3) {
        bail!("a clip must be at least 1 second long and inside the video, and each part at least {MIN_SEGMENT_S} s");
    }
    Ok(())
}

/// Saves a clip's trim (or segments) and title into the run; the next render uses them.
pub fn edit_clip(id: &str, edit: &ClipEdit) -> Result<Candidate> {
    let (mut run, _) = load_run(id)?;
    let duration = run.media_info.duration_s.unwrap_or(f64::MAX);
    let pieces = if edit.segments.is_empty() { vec![(edit.start, edit.end)] } else { edit.segments.clone() };
    check_pieces(&pieces, duration)?;
    let title = edit.title.trim();
    if title.is_empty() {
        bail!("a clip needs a title");
    }
    let c = run
        .candidates
        .iter_mut()
        .find(|c| c.rank == edit.rank)
        .with_context(|| format!("no clip #{} in this run", edit.rank))?;
    let pieces: Vec<(f64, f64)> = pieces.into_iter().map(|(a, b)| (a, b.min(duration))).collect();
    c.start = pieces.iter().map(|p| p.0).fold(f64::MAX, f64::min);
    c.end = pieces.iter().map(|p| p.1).fold(0.0, f64::max);
    c.segments = if pieces.len() > 1 { pieces } else { Vec::new() };
    c.title = title.to_string();
    let out = c.clone();
    save_run(&run)?;
    Ok(out)
}

/// Adds a clip the user marked by hand (no score); it gets the next free rank.
pub fn add_clip(id: &str, start: f64, end: f64, title: &str) -> Result<Candidate> {
    let (mut run, _) = load_run(id)?;
    // Past the ranks of files still in the clips folder too: a removed clip's export must not look like this one's.
    let rank = run
        .candidates
        .iter()
        .map(|c| c.rank)
        .chain(clips_folder_videos(&run).iter().filter_map(|v| rank_of(v)))
        .max()
        .unwrap_or(0)
        + 1;
    run.candidates.push(Candidate {
        start: 0.0,
        end: 0.0,
        score: 0,
        rank,
        title: String::new(),
        reasons: vec!["Added by you".into()],
        keywords: Vec::new(),
        segments: Vec::new(),
        layouts: Vec::new(),
    });
    save_run(&run)?;
    edit_clip(id, &ClipEdit { rank, start, end, title: title.to_string(), ..Default::default() }).inspect_err(|_| {
        // Invalid range or title: take the placeholder out again.
        run.candidates.retain(|c| c.rank != rank);
        save_run(&run).ok();
    })
}

/// Deletes a saved run: its transcript, thumbnails and downloaded video. Exported clips stay in their folder.
pub fn delete_run(id: &str) -> Result<()> {
    read_run(id)?; // a valid id of an existing run
    // The folder from the id, never the `dir` written in run.json.
    let dir = runs_dir().join(id);
    std::fs::remove_dir_all(&dir).with_context(|| format!("cannot delete {}", dir.display()))
}

/// Sets (or with `None` clears) the layout you chose for the shot around source time `at`; `shot` is that shot's
/// source range, and an earlier choice inside it is replaced.
pub fn set_layout(
    id: &str,
    rank: usize,
    at: f64,
    shot: (f64, f64),
    layout: Option<crate::framing::Layout>,
) -> Result<Candidate> {
    let (mut run, _) = load_run(id)?;
    let c =
        run.candidates.iter_mut().find(|c| c.rank == rank).with_context(|| format!("no clip #{rank} in this run"))?;
    c.layouts.retain(|l| !(shot.0..shot.1).contains(&l.0));
    if let Some(l) = layout {
        c.layouts.push((at, l));
    }
    let out = c.clone();
    save_run(&run)?;
    Ok(out)
}

/// Removes a clip from the run (its exported files stay where they are).
pub fn remove_clip(id: &str, rank: usize) -> Result<()> {
    let (mut run, _) = load_run(id)?;
    let before = run.candidates.len();
    run.candidates.retain(|c| c.rank != rank);
    if run.candidates.len() == before {
        bail!("no clip #{rank} in this run");
    }
    save_run(&run)
}

/// Corrected caption text for the words in `start..end`. Same word count: each word keeps its times. Otherwise the
/// new words share the old words' span in proportion to their length (captions stay in sync at the edges).
fn retime(old: &[Word], text: &str) -> Vec<Word> {
    let new: Vec<&str> = text.split_whitespace().collect();
    if new.len() == old.len() {
        return old.iter().zip(&new).map(|(w, t)| Word { text: t.to_string(), ..w.clone() }).collect();
    }
    let (Some(first), Some(last)) = (old.first(), old.last()) else { return Vec::new() };
    let (span_start, span) = (first.start, last.end - first.start);
    let total: usize = new.iter().map(|t| t.chars().count()).sum();
    let mut at = 0usize;
    new.iter()
        .map(|t| {
            let len = t.chars().count();
            let s = span_start + span * at as f64 / total as f64;
            at += len;
            Word { text: t.to_string(), start: s, end: span_start + span * at as f64 / total as f64 }
        })
        .collect()
}

/// Replaces the transcript's words in `start..end` with corrected text (captions, SRT and titles read them).
pub fn retext(id: &str, start: f64, end: f64, text: &str) -> Result<Vec<Word>> {
    let (run, words) = load_run(id)?;
    let old: Vec<Word> = words.iter().filter(|w| inside(w, start, end)).cloned().collect();
    if old.is_empty() {
        bail!("no speech in this part of the video to correct");
    }
    let new = retime(&old, text);
    let mut all: Vec<Word> = words.into_iter().filter(|w| !inside(w, start, end)).chain(new.iter().cloned()).collect();
    all.sort_by(|a, b| a.start.total_cmp(&b.start));
    std::fs::write(Path::new(&run.dir).join("transcript.json"), serde_json::to_vec(&all)?)?;
    Ok(new)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(t: &str, s: f64, e: f64) -> Word {
        Word { text: t.into(), start: s, end: e }
    }

    #[test]
    fn parts_are_checked_with_rounding_slack() {
        assert!(check_pieces(&[(20.98, 21.18), (30.0, 31.0)], 100.0).is_ok(), "0.2 s after rounding");
        assert!(check_pieces(&[(1.0, 1.1), (2.0, 4.0)], 100.0).is_err(), "part too short");
        assert!(check_pieces(&[(1.0, 1.5)], 100.0).is_err(), "clip too short");
        assert!(check_pieces(&[(99.0, 102.0)], 100.0).is_err(), "past the end");
    }

    #[test]
    fn corrected_text_keeps_or_shares_word_times() {
        let old = [w("hello", 1.0, 1.5), w("wrld", 1.6, 2.0)];
        let same = retime(&old, "Hello world");
        assert_eq!((same[1].text.as_str(), same[1].start, same[1].end), ("world", 1.6, 2.0));
        let more = retime(&old, "hi there world");
        assert_eq!(more.len(), 3);
        assert_eq!((more[0].start, more[2].end), (1.0, 2.0), "same span");
        assert!(more.windows(2).all(|p| p[0].end <= p[1].start + 1e-9), "{more:?}");
        assert!(retime(&old, "").is_empty());
    }

    #[test]
    fn folder_names_are_safe_and_short() {
        assert_eq!(clips_folder_name("a: b/c?", "1-x"), "a b c (1-x)");
        assert_eq!(clips_folder_name("  ", "1-x"), "1-x");
        assert_eq!(clips_folder_name(&"x".repeat(80), "1").len(), 64);
        assert_eq!(clips_folder_name("ends with dots...", "1"), "ends with dots (1)");
    }
}
