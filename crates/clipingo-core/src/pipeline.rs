//! Source → audio → transcript → clip candidates.

use std::path::Path;

use anyhow::{Result, bail};

use crate::scorer::{self, Candidate, FindOptions, Loudness};
use crate::source::{self, SourceInfo, SourceKind};
use crate::transcribe::{self, TranscribeOptions, Transcript, WordTiming, Workspace};

#[derive(Debug, serde::Serialize)]
pub struct TranscriptSummary {
    pub engine: String,
    pub language: Option<String>,
    pub word_timing: WordTiming,
    pub words: usize,
}

#[derive(Debug, serde::Serialize)]
pub struct FindResult {
    pub source: SourceInfo,
    pub transcript: TranscriptSummary,
    pub candidates: Vec<Candidate>,
}

async fn local_source(input: &str) -> Result<SourceInfo> {
    let info = source::probe(input).await?;
    if info.kind != SourceKind::File {
        bail!("YouTube links need the download step, which is not built yet; download the VOD and pick the file");
    }
    Ok(info)
}

/// Transcript plus the loudness envelope, from one audio extraction.
async fn transcribe_with_loudness(info: &SourceInfo, opts: &TranscribeOptions) -> Result<(Transcript, Vec<f32>)> {
    let work = Workspace::new()?;
    let wav = crate::audio::extract_wav(Path::new(&info.input), &work.0.join("audio.wav")).await?;
    let envelope = crate::audio::loudness_envelope(&wav)?;
    let duration = info.duration_s.unwrap_or(envelope.len() as f64 / crate::audio::ENVELOPE_RATE);
    let transcript = transcribe::transcribe(Path::new(&info.input), &wav, duration, opts, &work.0).await?;
    Ok((transcript, envelope))
}

pub async fn transcribe_file(input: &str, opts: &TranscribeOptions) -> Result<Transcript> {
    let info = local_source(input).await?;
    Ok(transcribe_with_loudness(&info, opts).await?.0)
}

pub async fn find_clips(input: &str, topts: &TranscribeOptions, fopts: &FindOptions) -> Result<FindResult> {
    let info = local_source(input).await?;
    let (transcript, envelope) = transcribe_with_loudness(&info, topts).await?;
    if transcript.words.is_empty() {
        bail!("no speech found in {}", info.title);
    }
    let loud = Loudness { rms: &envelope, rate: crate::audio::ENVELOPE_RATE };
    let candidates = scorer::find(&transcript.words, fopts, Some(&loud));
    Ok(FindResult {
        transcript: TranscriptSummary {
            engine: transcript.engine,
            language: transcript.language,
            word_timing: transcript.word_timing,
            words: transcript.words.len(),
        },
        source: info,
        candidates,
    })
}
