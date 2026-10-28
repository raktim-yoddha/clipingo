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
        id,
        dir: dir.to_string_lossy().into_owned(),
        media: media_info.input.clone(),
        source,
        media_info,
        transcript: TranscriptSummary {
            model: transcript.model,
            language: transcript.language,
            words: transcript.words.len(),
        },
        candidates,
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

/// Every saved run, newest first. Unreadable runs are skipped.
pub fn list_runs() -> Vec<RunListing> {
    let mut out: Vec<RunListing> = std::fs::read_dir(runs_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let run: Run = serde_json::from_slice(&std::fs::read(e.path().join("run.json")).ok()?).ok()?;
            let mut clips: Vec<String> = std::fs::read_dir(e.path().join("clips"))
                .into_iter()
                .flatten()
                .flatten()
                .map(|c| c.path())
                .filter(|p| p.extension().is_some_and(|x| x == "mp4"))
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            clips.sort();
            Some(RunListing { run, clips })
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
