//! Transcription: Sarvam Saaras v3 for Hindi / Hinglish, whisper.cpp for English, cached per source.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::{Word, sarvam};

/// What the user said the stream is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// Detect with whisper when available; otherwise let Sarvam detect.
    #[default]
    Auto,
    English,
    /// Hindi in Devanagari.
    Hindi,
    /// Hindi-English code-mixed speech, transcribed as spoken.
    Hinglish,
}

/// Caption script for Hindi / Hinglish.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Script {
    /// As spoken: Devanagari for Hindi words, Latin for English words.
    #[default]
    Native,
    /// Everything in Roman letters (Sarvam `translit`).
    Roman,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TranscribeOptions {
    #[serde(default)]
    pub language: Language,
    #[serde(default)]
    pub script: Script,
    /// Ignore the cache and transcribe again (costs Sarvam credit).
    #[serde(default)]
    pub refresh: bool,
}

/// How trustworthy word start/end times are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WordTiming {
    /// From the model (whisper word segments).
    Model,
    /// Spread across Sarvam's phrase timestamps by word length.
    Estimated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    /// "whisper.cpp" or "sarvam:saaras:v3:<mode>".
    pub engine: String,
    /// Detected or requested language code, e.g. "en", "hi-IN".
    pub language: Option<String>,
    pub word_timing: WordTiming,
    pub words: Vec<Word>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Whisper,
    Sarvam,
}

/// Which engine handles a request, given what is installed / configured.
pub fn route(language: Language, detected: Option<&str>, whisper_ready: bool, sarvam_ready: bool) -> Result<Engine> {
    let want = match language {
        Language::English => Engine::Whisper,
        Language::Hindi | Language::Hinglish => Engine::Sarvam,
        Language::Auto => match detected {
            Some(code) if code.starts_with("en") => Engine::Whisper,
            Some(_) => Engine::Sarvam,
            None if sarvam_ready => Engine::Sarvam,
            None => Engine::Whisper,
        },
    };
    match (want, whisper_ready, sarvam_ready) {
        (Engine::Whisper, true, _) | (Engine::Sarvam, _, true) => Ok(want),
        // English still works through Sarvam (en-IN) when whisper is not set up.
        (Engine::Whisper, false, true) => Ok(Engine::Sarvam),
        (Engine::Sarvam, _, false) => bail!(
            "Hindi/Hinglish transcription uses Sarvam Saaras v3: set the SARVAM_API_KEY environment variable"
        ),
        (Engine::Whisper, false, false) => bail!(
            "English transcription needs whisper-cli on PATH and a model: put ggml-large-v3-turbo.bin in {} or set CLIPINGO_WHISPER_MODEL",
            crate::data_dir().join("models").display()
        ),
    }
}

/// Sarvam mode for a request: `verbatim` keeps fillers so pacing can cut them.
pub fn sarvam_mode(language: Language, script: Script) -> &'static str {
    match (language, script) {
        (_, Script::Roman) => "translit",
        (Language::Hinglish | Language::Auto, Script::Native) => "codemix",
        _ => "verbatim",
    }
}

// ---------------------------------------------------------------- whisper.cpp

/// The ggml model to use: CLIPINGO_WHISPER_MODEL, else large-v3-turbo, else any ggml model in the models folder.
pub fn whisper_model() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CLIPINGO_WHISPER_MODEL").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let dir = crate::data_dir().join("models");
    let preferred = dir.join("ggml-large-v3-turbo.bin");
    if preferred.is_file() {
        return Some(preferred);
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("ggml-") && n.ends_with(".bin")))
        .collect();
    found.sort();
    found.pop()
}

async fn whisper_cli_present() -> bool {
    Command::new("whisper-cli").arg("--help").kill_on_drop(true).output().await.is_ok()
}

async fn run_whisper(model: &Path, wav: &Path, extra: &[&str]) -> Result<std::process::Output> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8).to_string();
    let out = Command::new("whisper-cli")
        .arg("-m")
        .arg(model)
        .arg("-f")
        .arg(wav)
        .args(["-t", &threads, "-np"])
        .args(extra)
        .kill_on_drop(true)
        .output()
        .await
        .context("could not start whisper-cli")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("whisper-cli failed: {}", err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("unknown error"));
    }
    Ok(out)
}

/// Language code whisper hears at `at_ms` (sampled mid-stream; intros are often music or silence).
async fn whisper_detect(model: &Path, wav: &Path, at_ms: u64) -> Result<Option<String>> {
    let out = run_whisper(model, wav, &["-dl", "-l", "auto", "-ot", &at_ms.to_string()]).await?;
    // whisper prints e.g. "auto-detected language: hi (p = 0.912)" on stderr.
    let text = String::from_utf8_lossy(&out.stderr).into_owned() + &String::from_utf8_lossy(&out.stdout);
    Ok(text.lines().find_map(|l| l.split("auto-detected language:").nth(1)).and_then(|r| r.split_whitespace().next()).map(str::to_string))
}

async fn whisper_transcribe(model: &Path, wav: &Path, language: &str, work: &Path) -> Result<Transcript> {
    let base = work.join("whisper");
    let base_s = base.to_string_lossy();
    // -ml 1 -sow: one segment per word with its own start/end (whisper.cpp's word-level mode).
    run_whisper(model, wav, &["-l", language, "-ml", "1", "-sow", "-sns", "-oj", "-of", &base_s]).await?;
    let json = std::fs::read_to_string(base.with_extension("json")).context("whisper-cli wrote no JSON")?;
    parse_whisper_json(&json)
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
pub fn parse_whisper_json(json: &str) -> Result<Transcript> {
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
    Ok(Transcript {
        engine: "whisper.cpp".into(),
        language: parsed.result.and_then(|r| r.language),
        word_timing: WordTiming::Model,
        words,
    })
}

// ---------------------------------------------------------------- cache + entry point

fn cache_path(input: &Path, engine_key: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let meta = std::fs::metadata(input).ok()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    // ponytail: SipHash key is only stable per toolchain; a toolchain change just means a cache miss.
    (input.canonicalize().ok()?, meta.len(), meta.modified().ok()?, engine_key).hash(&mut h);
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

/// Transcribe `input` (already-extracted 16 kHz `wav`), routing by language and reusing cached results.
pub async fn transcribe(input: &Path, wav: &Path, duration_s: f64, opts: &TranscribeOptions, work: &Path) -> Result<Transcript> {
    let model = whisper_model();
    let whisper_ready = model.is_some() && whisper_cli_present().await;
    let sarvam_key = sarvam::api_key();

    let detected = match (opts.language, &model, whisper_ready) {
        (Language::Auto, Some(m), true) => whisper_detect(m, wav, (duration_s * 500.0) as u64).await.ok().flatten(),
        _ => None,
    };
    let engine = route(opts.language, detected.as_deref(), whisper_ready, sarvam_key.is_some())?;
    let mode = sarvam_mode(opts.language, opts.script);
    let engine_key = match engine {
        Engine::Whisper => format!("whisper:{}", model.as_ref().and_then(|m| m.file_name()).map(|n| n.to_string_lossy()).unwrap_or_default()),
        Engine::Sarvam => format!("sarvam:saaras:v3:{mode}"),
    };

    let cache = cache_path(input, &engine_key);
    if !opts.refresh
        && let Some(t) = cache.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok())
    {
        return Ok(t);
    }

    let transcript = match engine {
        Engine::Whisper => {
            let lang = detected.as_deref().filter(|_| opts.language == Language::Auto).unwrap_or("en");
            whisper_transcribe(model.as_ref().expect("routed to whisper"), wav, lang, work).await?
        }
        Engine::Sarvam => {
            let code = match opts.language {
                Language::Auto => "unknown",
                Language::English => "en-IN",
                Language::Hindi | Language::Hinglish => "hi-IN",
            };
            let key = sarvam_key.expect("routed to sarvam");
            sarvam::transcribe(&key, wav, duration_s, code, mode, work).await?
        }
    };
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
    fn routes_by_language_and_availability() {
        assert_eq!(route(Language::English, None, true, true).unwrap(), Engine::Whisper);
        assert_eq!(route(Language::Hinglish, None, true, true).unwrap(), Engine::Sarvam);
        assert_eq!(route(Language::Auto, Some("en"), true, true).unwrap(), Engine::Whisper);
        assert_eq!(route(Language::Auto, Some("hi"), true, true).unwrap(), Engine::Sarvam);
        assert_eq!(route(Language::Auto, None, false, true).unwrap(), Engine::Sarvam);
        assert_eq!(route(Language::English, None, false, true).unwrap(), Engine::Sarvam, "English falls back to Sarvam en-IN");
        assert!(route(Language::Hindi, None, true, false).unwrap_err().to_string().contains("SARVAM_API_KEY"));
        assert!(route(Language::Auto, None, false, false).is_err());
    }

    #[test]
    fn sarvam_modes() {
        assert_eq!(sarvam_mode(Language::Hinglish, Script::Native), "codemix");
        assert_eq!(sarvam_mode(Language::Hinglish, Script::Roman), "translit");
        assert_eq!(sarvam_mode(Language::Hindi, Script::Native), "verbatim");
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
        let t = parse_whisper_json(json).unwrap();
        assert_eq!(t.language.as_deref(), Some("en"));
        let texts: Vec<_> = t.words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, ["And", "so,", "yes"]);
        assert!((t.words[1].end - 0.7).abs() < 1e-9 && (t.words[0].start - 0.32).abs() < 1e-9);
    }
}
