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
        anyhow::anyhow!(
            "{missing} is not installed: Settings → Speech models, or `clip models install {}`",
            models::for_language(wanted[0]).map_or("all", |m| m.id)
        )
    })
}

/// Runs whisper-cli on `inputs` (relative to `cwd`) from one model load, passing each stderr line to `on_line`.
/// Returns all stderr text (language detection reads it).
async fn run_whisper(
    cli: &Path,
    model: &Path,
    cwd: &Path,
    inputs: &[PathBuf],
    extra: &[&str],
    on_line: &mut (dyn FnMut(&str) + Send),
) -> Result<String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8).to_string();
    let mut cmd = Command::new(cli);
    cmd.current_dir(cwd).arg("-m").arg(model).args(["-t", &threads]).args(extra);
    for input in inputs {
        cmd.arg("-f").arg(input);
    }
    let mut child = cmd
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("could not start whisper-cli")?;
    let mut lines = BufReader::new(child.stderr.take().expect("piped")).lines();
    let mut log = String::new();
    while let Some(line) = lines.next_line().await? {
        on_line(&line);
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
    let cwd = wav.parent()?;
    let args = ["-np", "-dl", "-l", "auto", "-ot", &at_ms.to_string()];
    let log = run_whisper(cli, model, cwd, &[wav.to_path_buf()], &args, &mut |_| {}).await.ok()?;
    // whisper prints e.g. "auto-detected language: hi (p = 0.912)".
    log.lines()
        .find_map(|l| l.split("auto-detected language:").nth(1))
        .and_then(|r| r.split_whitespace().next())
        .map(str::to_string)
}

/// Speech spans (seconds) from Silero VAD via whisper.cpp's segment tool, split to at most `max_s`.
async fn speech_spans(cli: &Path, wav: &Path, max_s: f64) -> Option<Vec<(f64, f64)>> {
    let tool = cli.with_file_name(format!("whisper-vad-speech-segments{}", std::env::consts::EXE_SUFFIX));
    let out = Command::new(tool)
        .arg("-vm")
        .arg(models::vad_model()?)
        .arg("-f")
        .arg(wav)
        .args(["-vmsd", &max_s.to_string(), "-np"])
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    out.status.success().then(|| parse_vad_segments(&String::from_utf8_lossy(&out.stdout)))
}

/// "Speech segment 0: start = 311.00, end = 361.00" (centiseconds) → seconds.
fn parse_vad_segments(text: &str) -> Vec<(f64, f64)> {
    let num = |l: &str, key: &str| l.split(key).nth(1)?.split(',').next()?.trim().parse::<f64>().ok();
    text.lines()
        .filter(|l| l.contains("Speech segment"))
        .filter_map(|l| Some((num(l, "start =")? / 100.0, num(l, "end =")? / 100.0)))
        .collect()
}

/// Silence between a chunk's speech spans: enough to separate words, too little for DTW to drift across.
const GAP_S: f64 = 0.2;

/// Packs speech spans (padded so word edges survive) into decode chunks: each chunk is a list of source spans
/// whose audio, joined by `GAP_S` of silence, lasts at most `max_s`. Pauses are never decoded: whisper
/// hallucinates on them and DTW pulls words across them.
fn pack_chunks(spans: &[(f64, f64)], max_s: f64, duration_s: f64) -> Vec<Vec<(f64, f64)>> {
    const PAD_S: f64 = 0.1;
    let mut chunks: Vec<Vec<(f64, f64)>> = Vec::new();
    let (mut len, mut prev_end) = (0.0, 0.0f64);
    for (k, &(s, e)) in spans.iter().enumerate() {
        let next = spans.get(k + 1).map_or(duration_s, |n| n.0);
        let (s, e) = ((s - PAD_S).max(prev_end), (e + PAD_S).min(next).min(duration_s));
        prev_end = e;
        match chunks.last_mut() {
            // A pause shorter than the gap stays as it is.
            Some(c) if c.last().is_some_and(|l| s - l.1 < GAP_S && len + e - l.1 <= max_s) => {
                let last = c.last_mut().expect("non-empty");
                len += e - last.1;
                last.1 = e;
            }
            Some(c) if len + GAP_S + e - s <= max_s => {
                c.push((s, e));
                len += GAP_S + e - s;
            }
            _ => {
                chunks.push(vec![(s, e)]);
                len = e - s;
            }
        }
    }
    chunks
}

/// Time in a chunk's joined audio → source time. Times inside an inserted gap snap to the next span.
fn to_source(pieces: &[(f64, f64)], t: f64) -> f64 {
    let mut off = 0.0;
    for &(s, e) in pieces {
        if t < off + (e - s) {
            return s + (t - off).max(0.0);
        }
        off += e - s + GAP_S;
    }
    pieces.last().map_or(t, |p| p.1)
}

async fn whisper_transcribe(
    cli: &Path,
    spec: &ModelSpec,
    model: &Path,
    wav: &Path,
    duration_s: f64,
    work: &Path,
    progress: ProgressFn<'_>,
) -> Result<Transcript> {
    let stage = format!("Transcribing ({})", spec.label);
    progress(Progress { stage: stage.clone(), done: 0, total: None });
    // ponytail: without the VAD model, fixed windows (silence gets decoded and words can be cut at edges).
    let chunks = match speech_spans(cli, wav, spec.chunk_s - 1.0).await {
        Some(spans) => pack_chunks(&spans, spec.chunk_s, duration_s),
        None => (0..(duration_s / spec.chunk_s).ceil() as usize)
            .map(|i| vec![(i as f64 * spec.chunk_s, ((i + 1) as f64 * spec.chunk_s).min(duration_s))])
            .collect(),
    };
    let names = crate::audio::write_chunks(wav, &chunks, GAP_S, work)?;

    // -nt: one segment per chunk (fine-tunes' timestamp tokens are garbage); word times come from DTW alignment.
    // DTW needs flash attention off. -mc 0: whisper-cli would prompt each file with the previous file's text,
    // which shifts DTW times and makes the model repeat the previous chunk's last words.
    let args = ["-l", spec.whisper_lang, "-nt", "-mc", "0", "-sns", "-ojf", "-dtw", spec.dtw, "-nfa"];
    let total = names.len() as u64;
    let mut started = 0u64;
    // ponytail: batches keep the command line short; each batch reloads the model (~4 s).
    for batch in names.chunks(400) {
        run_whisper(cli, model, work, batch, &args, &mut |line| {
            if line.contains("processing '") {
                // Percent: the app shows other totals as bytes.
                progress(Progress { stage: stage.clone(), done: started * 100 / total, total: Some(100) });
                started += 1;
            }
        })
        .await?;
    }

    let mut words = Vec::new();
    let mut language = None;
    for (name, pieces) in names.iter().zip(&chunks) {
        let json_path = work.join(format!("{}.json", name.display()));
        let bytes =
            std::fs::read(&json_path).with_context(|| format!("whisper-cli wrote no {}", json_path.display()))?;
        // Token text can split a Devanagari character's bytes; only whole segment text is guaranteed UTF-8.
        let json = String::from_utf8_lossy(&bytes);
        let (chunk_words, lang) = parse_whisper_json(&json, &|t| to_source(pieces, t))?;
        words.extend(chunk_words);
        language = language.or(lang);
    }
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
    text: String,
    #[serde(default)]
    tokens: Vec<WhisperToken>,
}
#[derive(Deserialize)]
struct WhisperToken {
    text: String,
    offsets: WhisperOffsets,
    /// DTW time in 10 ms units; -1 when not computed.
    #[serde(default = "no_dtw")]
    t_dtw: i64,
}
fn no_dtw() -> i64 {
    -1
}
#[derive(Deserialize)]
struct WhisperOffsets {
    from: i64,
}

/// DTW token times trail the sound: `clip eval` measured words starting 0.19 s late against a TTS fixture's exact
/// word times and 0.22 s late against TED's human captions. Word starts are moved back by this much.
const DTW_LAG_S: f64 = 0.2;

/// whisper-cli `-ojf` output for one chunk → words on the source timeline (`to_source` maps chunk seconds).
/// A word starts at its first token's DTW time (less `DTW_LAG_S`) and runs to the next word (`audio::refine_words`
/// trims pauses); punctuation-only words join the previous word.
pub fn parse_whisper_json(json: &str, to_source: &dyn Fn(f64) -> f64) -> Result<(Vec<Word>, Option<String>)> {
    const LAST_WORD_S: f64 = 0.6;
    let parsed: WhisperJson = serde_json::from_str(json).context("unexpected whisper JSON")?;
    let mut words: Vec<Word> = Vec::new();
    for seg in parsed.transcription {
        let time = |t: &WhisperToken| {
            if t.t_dtw >= 0 { (t.t_dtw as f64 / 100.0 - DTW_LAG_S).max(0.0) } else { t.offsets.from as f64 / 1000.0 }
        };
        let tokens: Vec<&WhisperToken> =
            seg.tokens.iter().filter(|t| !(t.text.starts_with("[_") && t.text.ends_with(']'))).collect();
        let starts: Vec<f64> = tokens
            .iter()
            .enumerate()
            .filter(|(i, t)| *i == 0 || t.text.starts_with(' '))
            .map(|(_, t)| time(t))
            .collect();
        let texts: Vec<&str> = seg.text.split_whitespace().collect();
        if texts.len() != starts.len() {
            continue; // ponytail: never seen; would need token-level text assembly
        }
        let last_end = tokens.last().map_or(0.0, |t| time(t) + LAST_WORD_S);
        let ends = starts.iter().skip(1).copied().chain([last_end]);
        for ((text, &start), end) in texts.into_iter().zip(&starts).zip(ends) {
            let start = to_source(start);
            let end = to_source(end).max(start + 0.05);
            if text.starts_with('[') || text.starts_with('(') {
                continue; // [BLANK_AUDIO], (music) …
            }
            match words.last_mut() {
                Some(last) if text.chars().all(|c| !c.is_alphanumeric()) => {
                    last.text.push_str(text);
                    last.end = last.end.max(end);
                }
                _ => words.push(Word { text: text.to_string(), start, end }),
            }
        }
    }
    Ok((words, parsed.result.and_then(|r| r.language)))
}

fn cache_path(input: &Path, model_file: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    /// Bump when word timing changes, so old transcripts are redone.
    const TIMING_VERSION: u32 = 4; // 3: speech-only chunks + DTW word times; 4: DTW lag corrected
    let meta = std::fs::metadata(input).ok()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    // ponytail: SipHash key is only stable per toolchain; a toolchain change just means a cache miss.
    // VAD changes chunking, so it is part of the key.
    let key = (input.canonicalize().ok()?, meta.len(), meta.modified().ok()?, model_file);
    (key, models::vad_model().is_some(), TIMING_VERSION).hash(&mut h);
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
pub async fn transcribe(
    input: &Path,
    wav: &Path,
    duration_s: f64,
    opts: &TranscribeOptions,
    work: &Path,
    progress: ProgressFn<'_>,
) -> Result<Transcript> {
    let cli = models::whisper_cli()
        .context("whisper.cpp is not installed: Settings → Speech models, or `clip models install runtime`")?;
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
        && let Some(t) =
            cache.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok())
    {
        return Ok(t);
    }
    let transcript = whisper_transcribe(&cli, spec, &model, wav, duration_s, work, progress).await?;
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
    fn builds_words_from_dtw_tokens() {
        // Shape of whisper-cli -ojf: "भाई आज" split across byte tokens, special tokens, a lone " -" token.
        let json = r#"{"result":{"language":"hi"},"transcription":[{"text":"भाई आज - ठीक","tokens":[
            {"text":"[_BEG_]","offsets":{"from":0},"t_dtw":-1},
            {"text":"भ","offsets":{"from":80},"t_dtw":8},
            {"text":"ाई","offsets":{"from":80},"t_dtw":30},
            {"text":" आ","offsets":{"from":80},"t_dtw":46},
            {"text":"ज","offsets":{"from":80},"t_dtw":94},
            {"text":" -","offsets":{"from":80},"t_dtw":100},
            {"text":" ठ","offsets":{"from":80},"t_dtw":120},
            {"text":"ीक","offsets":{"from":80},"t_dtw":130},
            {"text":"[_EOT_]","offsets":{"from":0},"t_dtw":-1}]}]}"#;
        let (words, lang) = parse_whisper_json(json, &|t| 10.0 + t).unwrap();
        assert_eq!(lang.as_deref(), Some("hi"));
        let texts: Vec<_> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, ["भाई", "आज-", "ठीक"]);
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        // DTW times less the 0.2 s lag (clamped at the chunk start).
        assert!(close(words[0].start, 10.0) && close(words[0].end, 10.26), "{words:?}");
        assert!(close(words[1].end, 11.00), "punctuation extends the previous word: {words:?}");
        assert!(close(words[2].start, 11.00) && close(words[2].end, 11.70), "last word: last token + 0.6 s");
    }

    #[test]
    fn packs_speech_into_padded_chunks() {
        let spans = parse_vad_segments(
            "Detected 4 speech segments:\nSpeech segment 0: start = 311.00, end = 361.00\n\
             Speech segment 1: start = 387.00, end = 624.00\nSpeech segment 2: start = 1500.00, end = 1900.00\n",
        );
        assert_eq!(spans, [(3.11, 3.61), (3.87, 6.24), (15.0, 19.0)]);
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        let flat = |c: &Vec<Vec<(f64, f64)>>| c.iter().flatten().flat_map(|p| [p.0, p.1]).collect::<Vec<_>>();

        // 0.06 s pause after padding: joined as is. The 8.8 s pause is dropped, the last span gets its own chunk.
        let chunks = pack_chunks(&spans, 6.0, 19.1);
        assert_eq!(chunks.len(), 2, "{chunks:?}");
        let want = [3.01, 6.34, 14.9, 19.1];
        assert!(flat(&chunks).iter().zip(want).all(|(a, b)| close(*a, b)), "{chunks:?}");

        // Room for all: spans joined by GAP_S of silence, mapped back to source time.
        let chunks = pack_chunks(&spans, 30.0, 19.1);
        assert_eq!(chunks.len(), 1);
        let pieces = &chunks[0];
        assert!(close(to_source(pieces, 1.0), 4.01), "inside the first piece");
        assert!(close(to_source(pieces, 3.4), 14.9), "in the gap: snaps to the next piece");
        assert!(close(to_source(pieces, 3.33 + GAP_S + 1.0), 15.9), "inside the second piece");
        assert!(close(to_source(pieces, 99.0), 19.1), "past the end");
    }
}
