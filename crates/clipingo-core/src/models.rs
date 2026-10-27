//! Local speech models and the whisper.cpp runtime: what is installed, and installing it.
//!
//! All three models run on whisper.cpp:
//! - English:  whisper large-v3-turbo (official ggml).
//! - Hinglish: Oriserve Whisper-Hindi2Hinglish-Apex (large-v3-turbo fine-tune, Roman-script Hinglish).
//! - Hindi:    ARTPARK-IISc whisper-large-v3-vaani-hindi (large-v3 fine-tune, Devanagari).
//!
//! The two fine-tunes ship as Hugging Face safetensors, so they are converted once to ggml with
//! whisper.cpp's own converter (run through `uv`; Python is never needed at transcription time).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::transcribe::Language;

/// Pinned so a converter change upstream cannot silently change our models.
const WHISPER_CPP_TAG: &str = "v1.9.5";
const OPENAI_WHISPER_TAG: &str = "v20250625";
#[cfg(all(windows, target_arch = "x86_64"))]
const RUNTIME_ZIP: Option<&str> =
    Some("https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-win-cuda-12.4.0-x64.zip");
#[cfg(not(all(windows, target_arch = "x86_64")))]
const RUNTIME_ZIP: Option<&str> = None; // macOS / Linux: install whisper.cpp (e.g. `brew install whisper-cpp`) and put whisper-cli on PATH

/// Silero VAD for whisper.cpp: decoding only speech keeps word timestamps from drifting through
/// long pauses (measured: ~5 s drift without it, < 0.1 s with it).
const VAD_FILE: &str = "ggml-silero-v6.2.0.bin";
const VAD_URL: &str = "https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v6.2.0.bin";

pub fn vad_model() -> Option<PathBuf> {
    Some(models_dir().join(VAD_FILE)).filter(|p| p.is_file())
}

pub enum Source {
    /// A ready ggml file in a Hugging Face repo.
    Ggml { repo: &'static str, file: &'static str },
    /// Hugging Face transformers checkpoint, converted locally.
    Convert { repo: &'static str, files: &'static [&'static str] },
}

pub struct ModelSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub language: Language,
    /// Language token passed to whisper (`-l`). Apex is decoded with "en" to get Roman script, per its model card.
    pub whisper_lang: &'static str,
    /// File name inside the models folder.
    pub file: &'static str,
    pub source: Source,
    /// Approximate download size, for the UI.
    pub download_mb: u32,
}

const HF_FILES: &[&str] = &["config.json", "generation_config.json", "vocab.json", "added_tokens.json", "merges.txt"];

pub const MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "english",
        label: "English · whisper large-v3-turbo",
        language: Language::English,
        whisper_lang: "en",
        file: "ggml-large-v3-turbo.bin",
        source: Source::Ggml { repo: "ggerganov/whisper.cpp", file: "ggml-large-v3-turbo.bin" },
        download_mb: 1620,
    },
    ModelSpec {
        id: "hinglish",
        label: "Hinglish (Roman) · Oriserve Hindi2Hinglish-Apex",
        language: Language::Hinglish,
        whisper_lang: "en",
        file: "ggml-hindi2hinglish-apex.bin",
        source: Source::Convert { repo: "Oriserve/Whisper-Hindi2Hinglish-Apex", files: &["model.safetensors"] },
        download_mb: 1620,
    },
    ModelSpec {
        id: "hindi",
        label: "Hindi (Devanagari) · IISc Vaani large-v3",
        language: Language::Hindi,
        whisper_lang: "hi",
        file: "ggml-vaani-hindi-large-v3.bin",
        source: Source::Convert {
            repo: "ARTPARK-IISc/whisper-large-v3-vaani-hindi",
            files: &["model.safetensors.index.json", "model-00001-of-00002.safetensors", "model-00002-of-00002.safetensors"],
        },
        download_mb: 6170,
    },
];

pub fn models_dir() -> PathBuf {
    crate::data_dir().join("models")
}

pub fn spec(id: &str) -> Option<&'static ModelSpec> {
    MODELS.iter().find(|m| m.id == id)
}

pub fn for_language(language: Language) -> Option<&'static ModelSpec> {
    MODELS.iter().find(|m| m.language == language)
}

/// Installed ggml file for a model (CLIPINGO_WHISPER_MODEL overrides English, for custom models).
pub fn installed(spec: &ModelSpec) -> Option<PathBuf> {
    if spec.id == "english"
        && let Some(p) = std::env::var_os("CLIPINGO_WHISPER_MODEL").map(PathBuf::from).filter(|p| p.is_file())
    {
        return Some(p);
    }
    Some(models_dir().join(spec.file)).filter(|p| p.is_file())
}

/// whisper-cli: CLIPINGO_WHISPER_CLI, the runtime we installed, or PATH.
pub fn whisper_cli() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CLIPINGO_WHISPER_CLI").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let exe = if cfg!(windows) { "whisper-cli.exe" } else { "whisper-cli" };
    let root = crate::data_dir().join("bin").join("whisper");
    for candidate in [root.join(exe), root.join("Release").join(exe)] {
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(exe)).find(|p| p.is_file())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelStatus {
    pub id: &'static str,
    pub label: &'static str,
    pub language: Language,
    pub installed: bool,
    pub path: Option<String>,
    pub download_mb: u32,
    /// Needs `uv` for the one-time conversion.
    pub needs_conversion: bool,
}

pub fn status() -> Vec<ModelStatus> {
    MODELS
        .iter()
        .map(|m| {
            let path = installed(m);
            ModelStatus {
                id: m.id,
                label: m.label,
                language: m.language,
                installed: path.is_some(),
                path: path.map(|p| p.to_string_lossy().into_owned()),
                download_mb: m.download_mb,
                needs_conversion: matches!(m.source, Source::Convert { .. }),
            }
        })
        .collect()
}

/// Progress of a long install step.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Progress {
    pub stage: String,
    pub done: u64,
    pub total: Option<u64>,
}

pub type ProgressFn<'a> = &'a (dyn Fn(Progress) + Send + Sync);

/// Download `url` to `dest`, resuming a previous `.part` file. Returns `dest`.
pub async fn download(url: &str, dest: &Path, stage: &str, progress: ProgressFn<'_>) -> Result<PathBuf> {
    if dest.is_file() {
        return Ok(dest.to_path_buf());
    }
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let part = dest.with_extension(format!("{}.part", dest.extension().and_then(|e| e.to_str()).unwrap_or("")));
    let have = tokio::fs::metadata(&part).await.map_or(0, |m| m.len());
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(120))
        .user_agent(concat!("clipingo/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let mut req = client.get(url);
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let mut resp = req.send().await.map_err(|e| anyhow::anyhow!("download failed: {}", e.without_url()))?;
    let status = resp.status();
    if !status.is_success() {
        bail!("download of {} failed: HTTP {status}", dest.file_name().unwrap_or_default().to_string_lossy());
    }
    let resumed = status == reqwest::StatusCode::PARTIAL_CONTENT;
    let mut done = if resumed { have } else { 0 };
    let total = resp.content_length().map(|n| n + done);
    // Exclusive OS lock: the app and the CLI must never write the same .part at once. Released if we die.
    // Open without truncating: lock first, then reset, so we never clobber a file another process holds.
    let mut std_file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&part)?;
    if std_file.try_lock().is_err() {
        bail!("{} is already being downloaded by another Clipingo process", dest.file_name().unwrap_or_default().to_string_lossy());
    }
    if !resumed {
        std_file.set_len(0)?;
    }
    std::io::Seek::seek(&mut std_file, std::io::SeekFrom::End(0))?;
    let mut file = tokio::fs::File::from_std(std_file);
    let mut last = std::time::Instant::now();
    while let Some(chunk) = resp.chunk().await.map_err(|e| anyhow::anyhow!("download interrupted (run install again to resume): {}", e.without_url()))? {
        file.write_all(&chunk).await?;
        done += chunk.len() as u64;
        if last.elapsed() > Duration::from_millis(500) {
            progress(Progress { stage: stage.to_string(), done, total });
            last = std::time::Instant::now();
        }
    }
    file.flush().await?;
    drop(file);
    if total.is_some_and(|t| t != done) {
        bail!("download of {} ended early ({done} bytes); run install again to resume", dest.display());
    }
    tokio::fs::rename(&part, dest).await?;
    progress(Progress { stage: stage.to_string(), done, total });
    Ok(dest.to_path_buf())
}

/// Install whisper.cpp (Windows x64: official CUDA 12.4 build, which also runs on CPU).
pub async fn install_runtime(progress: ProgressFn<'_>) -> Result<PathBuf> {
    download(VAD_URL, &models_dir().join(VAD_FILE), "voice activity model", progress).await?;
    if let Some(cli) = whisper_cli() {
        return Ok(cli);
    }
    let Some(url) = RUNTIME_ZIP else {
        bail!("install whisper.cpp for this OS and put whisper-cli on PATH (macOS: brew install whisper-cpp)");
    };
    let root = crate::data_dir().join("bin").join("whisper");
    let zip = download(url, &crate::data_dir().join("downloads").join("whisper-bin-win-cuda-x64.zip"), "whisper.cpp runtime", progress).await?;
    tokio::fs::create_dir_all(&root).await?;
    progress(Progress { stage: "Unpacking whisper.cpp".into(), done: 0, total: None });
    // Windows 10+ ships bsdtar, which reads zip files.
    let out = Command::new("tar").arg("-xf").arg(&zip).arg("-C").arg(&root).output().await.context("could not run tar")?;
    if !out.status.success() {
        bail!("unpacking whisper.cpp failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    tokio::fs::remove_file(&zip).await.ok();
    whisper_cli().context("whisper-cli.exe not found after unpacking the runtime")
}

/// Install one model; returns its ggml path.
pub async fn install(id: &str, progress: ProgressFn<'_>) -> Result<PathBuf> {
    let spec = spec(id).with_context(|| format!("unknown model {id}"))?;
    if let Some(p) = installed(spec) {
        return Ok(p);
    }
    let dest = models_dir().join(spec.file);
    match &spec.source {
        Source::Ggml { repo, file } => {
            download(&format!("https://huggingface.co/{repo}/resolve/main/{file}"), &dest, spec.label, progress).await
        }
        Source::Convert { repo, files } => {
            let src = crate::data_dir().join("downloads").join("hf").join(repo.replace('/', "--"));
            for f in HF_FILES.iter().chain(files.iter()) {
                download(&format!("https://huggingface.co/{repo}/resolve/main/{f}"), &src.join(f), &format!("{} · {f}", spec.label), progress)
                    .await?;
            }
            convert(&src, &dest, spec.label, progress).await?;
            // The ggml file holds everything; the multi-GB checkpoint is no longer needed.
            tokio::fs::remove_dir_all(&src).await.ok();
            Ok(dest)
        }
    }
}

/// HF checkpoint folder → ggml f16 file, using whisper.cpp's converter in a throwaway `uv` environment.
async fn convert(src: &Path, dest: &Path, label: &str, progress: ProgressFn<'_>) -> Result<()> {
    let tools = crate::data_dir().join("downloads").join("converter");
    let script = download(
        &format!("https://raw.githubusercontent.com/ggml-org/whisper.cpp/{WHISPER_CPP_TAG}/models/convert-h5-to-ggml.py"),
        &tools.join("convert-h5-to-ggml.py"),
        "converter",
        progress,
    )
    .await?;
    // The converter expects <dir>/whisper/assets/mel_filters.npz (openai/whisper repo layout).
    download(
        &format!("https://raw.githubusercontent.com/openai/whisper/{OPENAI_WHISPER_TAG}/whisper/assets/mel_filters.npz"),
        &tools.join("whisper").join("assets").join("mel_filters.npz"),
        "mel filters",
        progress,
    )
    .await?;

    force_float32_load(&src.join("config.json"))?;
    let out_dir = src.join("ggml-out");
    tokio::fs::create_dir_all(&out_dir).await?;
    progress(Progress { stage: format!("Converting {label} (one-time, a few minutes)"), done: 0, total: None });
    let out = Command::new("uv")
        .args(["run", "--no-project", "--python", "3.12", "--with", "torch", "--with", "transformers", "--with", "numpy"])
        .arg(&script)
        .arg(src)
        .arg(&tools)
        .arg(&out_dir)
        .kill_on_drop(true)
        .output()
        .await
        .context("converting needs uv (https://docs.astral.sh/uv/): install it and try again")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("model conversion failed: {}", err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("unknown error"));
    }
    tokio::fs::create_dir_all(dest.parent().expect("models dir")).await?;
    tokio::fs::rename(out_dir.join("ggml-model.bin"), dest).await.context("converter produced no ggml-model.bin")?;
    Ok(())
}

/// transformers loads checkpoints in their stored dtype ("auto"); bf16 weights then break the converter's
/// `.numpy()` ("unsupported ScalarType BFloat16"). Declaring float32 makes it upcast; the converter still writes f16.
fn force_float32_load(config: &Path) -> Result<()> {
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(config)?)?;
    let obj = v.as_object_mut().context("config.json is not an object")?;
    obj.insert("torch_dtype".into(), "float32".into());
    obj.insert("dtype".into(), "float32".into());
    std::fs::write(config, serde_json::to_vec_pretty(&v)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bf16_config_is_forced_to_float32() {
        let p = std::env::temp_dir().join(format!("clipingo-cfg-{}.json", std::process::id()));
        std::fs::write(&p, r#"{"d_model":1280,"torch_dtype":"bfloat16"}"#).unwrap();
        force_float32_load(&p).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        std::fs::remove_file(&p).ok();
        assert_eq!(v["torch_dtype"], "float32");
        assert_eq!(v["dtype"], "float32");
        assert_eq!(v["d_model"], 1280);
    }

    #[test]
    fn one_model_per_language_with_expected_decoding() {
        for lang in [Language::English, Language::Hinglish, Language::Hindi] {
            assert!(for_language(lang).is_some(), "{lang:?}");
        }
        assert_eq!(for_language(Language::Hinglish).unwrap().whisper_lang, "en", "Apex decodes Roman Hinglish with the en token");
        assert_eq!(for_language(Language::Hindi).unwrap().whisper_lang, "hi");
        let ids: std::collections::HashSet<_> = MODELS.iter().map(|m| m.file).collect();
        assert_eq!(ids.len(), MODELS.len(), "distinct files");
    }
}
