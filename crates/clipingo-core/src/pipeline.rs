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

pub fn runs_dir() -> PathBuf {
    crate::data_dir().join("runs")
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

/// The run's exported `.mp4` files, by name.
pub fn exported(run: &Run) -> Vec<String> {
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
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        bail!("invalid run id");
    }
    let dir = runs_dir().join(id);
    let run: Run =
        serde_json::from_slice(&std::fs::read(dir.join("run.json")).with_context(|| format!("run {id} not found"))?)?;
    let words: Vec<Word> = serde_json::from_slice(&std::fs::read(dir.join("transcript.json"))?)?;
    Ok((run, words))
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

/// An editor change to one clip: new in / out points (source seconds) and title.
#[derive(Debug, Clone, Deserialize)]
pub struct ClipEdit {
    pub rank: usize,
    pub start: f64,
    pub end: f64,
    pub title: String,
}

/// Saves a clip's trim and title into the run; the next render uses them.
pub fn edit_clip(id: &str, edit: &ClipEdit) -> Result<Candidate> {
    let (mut run, _) = load_run(id)?;
    let duration = run.media_info.duration_s.unwrap_or(f64::MAX);
    if !(edit.start >= 0.0 && edit.end <= duration + 0.5 && edit.end - edit.start >= 1.0) {
        bail!("a clip must be at least 1 second long and inside the video");
    }
    let title = edit.title.trim();
    if title.is_empty() {
        bail!("a clip needs a title");
    }
    let c = run
        .candidates
        .iter_mut()
        .find(|c| c.rank == edit.rank)
        .with_context(|| format!("no clip #{} in this run", edit.rank))?;
    (c.start, c.end, c.title) = (edit.start, edit.end.min(duration), title.to_string());
    let out = c.clone();
    save_run(&run)?;
    Ok(out)
}

/// Adds a clip the user marked by hand (no score); it gets the next free rank.
pub fn add_clip(id: &str, start: f64, end: f64, title: &str) -> Result<Candidate> {
    let (mut run, _) = load_run(id)?;
    let rank = run.candidates.iter().map(|c| c.rank).max().unwrap_or(0) + 1;
    run.candidates.push(Candidate {
        start: 0.0,
        end: 0.0,
        score: 0,
        rank,
        title: String::new(),
        reasons: vec!["Added by you".into()],
        keywords: Vec::new(),
    });
    save_run(&run)?;
    edit_clip(id, &ClipEdit { rank, start, end, title: title.to_string() }).inspect_err(|_| {
        // Invalid range or title: take the placeholder out again.
        run.candidates.retain(|c| c.rank != rank);
        save_run(&run).ok();
    })
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
