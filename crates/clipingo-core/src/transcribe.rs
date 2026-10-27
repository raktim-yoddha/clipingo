//! Transcription, fully local: whisper.cpp with one model per language (see `models`), cached per source.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::Word;
use crate::models::{self, ModelSpec, Progress, ProgressFn};

/// What the stream is in; picks the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// Detect mid-stream: English → English model, otherwise Hinglish (falls back to Hindi).
    #[default]
    Auto,
    English,
    /// Hindi in Devanagari.
    Hindi,
    /// Hindi-English code-mixed speech in Roman script.
    Hinglish,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TranscribeOptions {
    #[serde(default)]
    pub language: Language,
    /// Ignore the cache and transcribe again.
    #[serde(default)]
    pub refresh: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    /// Model id from `models::MODELS`, e.g. "hinglish".
    pub model: String,
    /// Language code whisper decoded with or detected.
    pub language: Option<String>,
    pub words: Vec<Word>,
}

/// Which model handles a request, given what is installed. `detected` is whisper's language code.
pub fn route(language: Language, detected: Option<&str>, installed: &dyn Fn(Language) -> bool) -> Result<Language> {
    let wanted = match (language, detected) {
        (Language::Auto, Some(code)) if code.starts_with("en") => vec![Language::English],
        (Language::Auto, _) => vec![Language::Hinglish, Language::Hindi, Language::English],
        (other, _) => vec![other],
    };
    wanted.iter().copied().find(|&l| installed(l)).ok_or_else(|| {
        let missing = models::for_language(wanted[0]).map_or("a speech model", |m| m.label);
        anyhow::anyhow!("{missing} is not installed: Settings → Speech models, or `clip models install {}`", models::for_language(wanted[0]).map_or("all", |m| m.id))
    })
}

/// Runs whisper-cli, streaming its stderr; with `-pp` in `extra`, "progress = N%" lines are reported.
/// Returns all stderr text (language detection reads it).
async fn run_whisper(cli: &Path, model: &Path, wav: &Path, extra: &[&str], progress: Option<(ProgressFn<'_>, &str)>) -> Result<String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8).to_string();
    let mut child = Command::new(cli)
        .arg("-m")
        .arg(model)
        .arg("-f")
        .arg(wav)
        .args(["-t", &threads, "-np"])
        .args(extra)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("could not start whisper-cli")?;
    let mut lines = BufReader::new(child.stderr.take().expect("piped")).lines();
    let mut log = String::new();
    while let Some(line) = lines.next_line().await? {
        if let (Some((report, stage)), Some(pct)) = (progress, line.split("progress =").nth(1)) {
            if let Ok(p) = pct.trim().trim_end_matches('%').trim().parse::<u64>() {
                report(Progress { stage: stage.to_string(), done: p, total: Some(100) });
            }
            continue;
        }
        log.push_str(&line);
        log.push('\n');
    }
    let status = child.wait().await?;
    if !status.success() {
        bail!("whisper-cli failed: {}", log.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("unknown error"));
    }
    Ok(log)
}

/// Language whisper hears at `at_ms` (sampled mid-stream; intros are often music or silence).
async fn detect_language(cli: &Path, model: &Path, wav: &Path, at_ms: u64) -> Option<String> {
    let log = run_whisper(cli, model, wav, &["-dl", "-l", "auto", "-ot", &at_ms.to_string()], None).await.ok()?;
    // whisper prints e.g. "auto-detected language: hi (p = 0.912)".
    log.lines().find_map(|l| l.split("auto-detected language:").nth(1)).and_then(|r| r.split_whitespace().next()).map(str::to_string)
}

async fn whisper_transcribe(cli: &Path, spec: &ModelSpec, model: &Path, wav: &Path, work: &Path, progress: ProgressFn<'_>) -> Result<Transcript> {
    let base = work.join("whisper");
    let base_s = base.to_string_lossy();
    let stage = format!("Transcribing ({})", spec.label);
    progress(Progress { stage: stage.clone(), done: 0, total: Some(100) });
    // -ml 1 -sow: one segment per word with its own start/end (whisper.cpp word-level mode).
    let mut args: Vec<String> = ["-l", spec.whisper_lang, "-ml", "1", "-sow", "-sns", "-pp", "-oj", "-of"].map(String::from).to_vec();
    args.push(base_s.into_owned());
    // VAD: only speech is decoded, so word times do not drift through pauses.
    if let Some(vad) = models::vad_model() {
        args.extend(["--vad".into(), "-vm".into(), vad.to_string_lossy().into_owned()]);
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run_whisper(cli, model, wav, &args, Some((progress, &stage))).await?;
    let json = std::fs::read_to_string(base.with_extension("json")).context("whisper-cli wrote no JSON")?;
    let (words, language) = parse_whisper_json(&json)?;
    Ok(Transcript { model: spec.id.to_string(), language: language.or(Some(spec.whisper_lang.to_string())), words })
}

#[derive(Deserialize)]
struct WhisperJson {
    #[serde(default)]
    result: Option<WhisperResult>,
    transcription: Vec<WhisperSegment>,
}
#[derive(Deserialize)]
struct WhisperResult {
    language: Option<String>,
}
#[derive(Deserialize)]
struct WhisperSegment {
    offsets: WhisperOffsets,
    text: String,
}
#[derive(Deserialize)]
struct WhisperOffsets {
    from: i64,
    to: i64,
}

/// whisper-cli `-oj` output (offsets in ms) → words; punctuation-only segments join the previous word.
pub fn parse_whisper_json(json: &str) -> Result<(Vec<Word>, Option<String>)> {
    let parsed: WhisperJson = serde_json::from_str(json).context("unexpected whisper JSON")?;
    let mut words: Vec<Word> = Vec::new();
    for seg in parsed.transcription {
        let text = seg.text.trim();
        if text.is_empty() || text.starts_with('[') || text.starts_with('(') {
            continue; // [BLANK_AUDIO], (music) …
        }
        let (start, end) = (seg.offsets.from as f64 / 1000.0, seg.offsets.to as f64 / 1000.0);
        let is_punct = text.chars().all(|c| !c.is_alphanumeric());
        match words.last_mut() {
            Some(last) if is_punct => {
                last.text.push_str(text);
                last.end = last.end.max(end);
            }
            _ => words.push(Word { text: text.to_string(), start, end }),
        }
    }
    Ok((words, parsed.result.and_then(|r| r.language)))
}

fn cache_path(input: &Path, model_file: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let meta = std::fs::metadata(input).ok()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    // ponytail: SipHash key is only stable per toolchain; a toolchain change just means a cache miss.
    // VAD changes word timing, so it is part of the key.
    (input.canonicalize().ok()?, meta.len(), meta.modified().ok()?, model_file, models::vad_model().is_some()).hash(&mut h);
    Some(crate::data_dir().join("cache").join("transcripts").join(format!("{:016x}.json", h.finish())))
}

/// A scratch folder removed when dropped.
pub struct Workspace(pub PathBuf);
impl Workspace {
    pub fn new() -> Result<Self> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("clipingo-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Transcribe `input` (already-extracted 16 kHz `wav`), picking the model by language and reusing cached results.
pub async fn transcribe(input: &Path, wav: &Path, duration_s: f64, opts: &TranscribeOptions, work: &Path, progress: ProgressFn<'_>) -> Result<Transcript> {
    let cli = models::whisper_cli().context("whisper.cpp is not installed: Settings → Speech models, or `clip models install runtime`")?;
    let installed = |l: Language| models::for_language(l).and_then(models::installed).is_some();

    // Detection uses the stock multilingual English model (fine-tunes lose language ID).
    let detected = match (opts.language, models::for_language(Language::English).and_then(models::installed)) {
        (Language::Auto, Some(detector)) => detect_language(&cli, &detector, wav, (duration_s * 500.0) as u64).await,
        _ => None,
    };
    let language = route(opts.language, detected.as_deref(), &installed)?;
    let spec = models::for_language(language).expect("every language has a model");
    let model = models::installed(spec).expect("routed to an installed model");

    let cache = cache_path(input, spec.file);
    if !opts.refresh
        && let Some(t) = cache.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok())
    {
        return Ok(t);
    }
    let transcript = whisper_transcribe(&cli, spec, &model, wav, work, progress).await?;
    if let Some(p) = cache {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        std::fs::write(&p, serde_json::to_vec(&transcript)?).ok();
    }
    Ok(transcript)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_by_language_and_what_is_installed() {
        let all = |_: Language| true;
        let only = |x: Language| move |l: Language| l == x;
        assert_eq!(route(Language::English, None, &all).unwrap(), Language::English);
        assert_eq!(route(Language::Hindi, None, &all).unwrap(), Language::Hindi);
        assert_eq!(route(Language::Auto, Some("en"), &all).unwrap(), Language::English);
        assert_eq!(route(Language::Auto, Some("hi"), &all).unwrap(), Language::Hinglish);
        assert_eq!(route(Language::Auto, Some("hi"), &only(Language::Hindi)).unwrap(), Language::Hindi);
        assert_eq!(route(Language::Auto, None, &only(Language::English)).unwrap(), Language::English);
        let err = route(Language::Hinglish, None, &only(Language::English)).unwrap_err().to_string();
        assert!(err.contains("Hinglish") && err.contains("install hinglish"), "{err}");
    }

    #[test]
    fn parses_whisper_word_segments() {
        let json = r#"{"result":{"language":"en"},"transcription":[
            {"timestamps":{"from":"00:00:00,000","to":"00:00:00,320"},"offsets":{"from":0,"to":320},"text":""},
            {"offsets":{"from":320,"to":370},"text":" And"},
            {"offsets":{"from":370,"to":690},"text":" so"},
            {"offsets":{"from":690,"to":700},"text":","},
            {"offsets":{"from":800,"to":1200},"text":" [BLANK_AUDIO]"},
            {"offsets":{"from":1200,"to":1500},"text":" yes"}]}"#;
        let (words, lang) = parse_whisper_json(json).unwrap();
        assert_eq!(lang.as_deref(), Some("en"));
        let texts: Vec<_> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, ["And", "so,", "yes"]);
        assert!((words[1].end - 0.7).abs() < 1e-9 && (words[0].start - 0.32).abs() < 1e-9);
    }
}
