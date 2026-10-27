//! System check: which external tools and GPU backends this machine has.

use std::time::Duration;

use tokio::process::Command;

/// One external tool the pipeline can use.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolStatus {
    pub name: &'static str,
    pub purpose: &'static str,
    pub required: bool,
    pub found: bool,
    pub version: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SystemReport {
    pub os: &'static str,
    pub tools: Vec<ToolStatus>,
    /// GPU names reported by `nvidia-smi`, empty when no NVIDIA GPU or driver.
    pub nvidia_gpus: Vec<String>,
}

const TOOLS: &[(&str, &str, &str, bool)] = &[
    ("ffmpeg", "-version", "Decode, render and encode video", true),
    ("ffprobe", "-version", "Read media duration and streams", true),
    ("yt-dlp", "--version", "Download YouTube streams", true),
    ("deno", "--version", "YouTube challenge solver for yt-dlp", true),
    ("whisper-cli", "--help", "Local speech recognition (English, offline)", false),
    ("llama-server", "--version", "Local LLM / vision judge", false),
    ("agy", "--version", "Antigravity CLI visual checks", false),
];

/// First non-empty output line of `program arg`, or `None` if it cannot run.
async fn first_line(program: &str, arg: &str) -> Option<String> {
    let run = Command::new(program).arg(arg).kill_on_drop(true).output();
    let out = tokio::time::timeout(Duration::from_secs(10), run).await.ok()?.ok()?;
    let text = if out.stdout.is_empty() { out.stderr } else { out.stdout };
    String::from_utf8_lossy(&text).lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)
}

pub async fn check() -> SystemReport {
    let probes = TOOLS.iter().map(|&(name, arg, purpose, required)| async move {
        let line = first_line(name, arg).await;
        ToolStatus { name, purpose, required, found: line.is_some(), version: line }
    });
    let tools = futures_join(probes).await;
    let nvidia_gpus = match Command::new("nvidia-smi").args(["--query-gpu=name,memory.total", "--format=csv,noheader"]).output().await {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect(),
        _ => Vec::new(),
    };
    SystemReport { os: std::env::consts::OS, tools, nvidia_gpus }
}

/// Run futures concurrently and keep their order (avoids a `futures` dependency for one call).
async fn futures_join<F: std::future::Future<Output = T> + Send + 'static, T: Send + 'static>(
    futs: impl Iterator<Item = F>,
) -> Vec<T> {
    let handles: Vec<_> = futs.map(tokio::spawn).collect();
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        out.push(h.await.expect("probe task panicked"));
    }
    out
}
