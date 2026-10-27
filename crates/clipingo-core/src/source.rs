//! Source probing: a local file (ffprobe) or a YouTube link (yt-dlp metadata only).

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::process::Command;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    File,
    Youtube,
}

/// What the Create page's video card shows.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SourceInfo {
    pub kind: SourceKind,
    /// Canonical path or URL used for later steps.
    pub input: String,
    pub title: String,
    pub channel: Option<String>,
    pub duration_s: Option<f64>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub fps: Option<f64>,
    pub thumbnail: Option<String>,
    pub view_count: Option<u64>,
    /// `YYYYMMDD` as YouTube reports it.
    pub upload_date: Option<String>,
    pub was_live: bool,
}

/// Accepts youtube.com watch/live/shorts links and youtu.be links over https; returns the trimmed URL.
pub fn youtube_url(input: &str) -> Option<String> {
    let url = input.trim();
    let rest = url.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let ok = match host {
        "youtu.be" => !path.is_empty(),
        "youtube.com" | "www.youtube.com" | "m.youtube.com" => {
            path.starts_with("watch?") || path.starts_with("live/") || path.starts_with("shorts/")
        }
        _ => false,
    };
    (ok && !url.chars().any(char::is_whitespace)).then(|| url.to_string())
}

pub async fn probe(input: &str) -> Result<SourceInfo> {
    if let Some(url) = youtube_url(input) {
        return probe_youtube(&url).await;
    }
    let path = Path::new(input.trim());
    if !path.is_file() {
        bail!("Not a video file or a YouTube link: {input}");
    }
    probe_file(path).await
}

async fn run_json(cmd: &mut Command, what: &str) -> Result<Value> {
    let out = tokio::time::timeout(Duration::from_secs(60), cmd.kill_on_drop(true).output())
        .await
        .with_context(|| format!("{what} timed out"))?
        .with_context(|| format!("could not start {what}; run System check"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("{what} failed: {}", err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("unknown error"));
    }
    serde_json::from_slice(&out.stdout).with_context(|| format!("{what} returned invalid JSON"))
}

async fn probe_youtube(url: &str) -> Result<SourceInfo> {
    // `--` ends option parsing so the URL can never be read as a flag.
    let v = run_json(
        Command::new(crate::tools::resolve("yt-dlp")).args(crate::tools::yt_dlp_base_args()).args(["-J", "--no-playlist", "--no-warnings", "--", url]),
        "yt-dlp",
    )
    .await?;
    Ok(SourceInfo {
        kind: SourceKind::Youtube,
        input: v["webpage_url"].as_str().unwrap_or(url).to_string(),
        title: v["title"].as_str().unwrap_or("Untitled").to_string(),
        channel: v["channel"].as_str().or(v["uploader"].as_str()).map(str::to_string),
        duration_s: v["duration"].as_f64(),
        width: v["width"].as_u64(),
        height: v["height"].as_u64(),
        fps: v["fps"].as_f64(),
        thumbnail: v["thumbnail"].as_str().map(str::to_string),
        view_count: v["view_count"].as_u64(),
        upload_date: v["upload_date"].as_str().map(str::to_string),
        was_live: v["was_live"].as_bool().unwrap_or(false) || v["live_status"].as_str() == Some("was_live"),
    })
}

async fn probe_file(path: &Path) -> Result<SourceInfo> {
    let path = path.canonicalize().context("cannot resolve file path")?;
    let v = run_json(
        Command::new("ffprobe").args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"]).arg(&path),
        "ffprobe",
    )
    .await?;
    let video = v["streams"].as_array().and_then(|s| s.iter().find(|s| s["codec_type"] == "video"));
    let fps = video.and_then(|s| s["avg_frame_rate"].as_str()).and_then(parse_rate);
    let display = path.to_string_lossy().trim_start_matches(r"\\?\").to_string();
    Ok(SourceInfo {
        kind: SourceKind::File,
        title: v["format"]["tags"]["title"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()),
        input: display,
        channel: None,
        duration_s: v["format"]["duration"].as_str().and_then(|d| d.parse().ok()),
        width: video.and_then(|s| s["width"].as_u64()),
        height: video.and_then(|s| s["height"].as_u64()),
        fps,
        thumbnail: None,
        view_count: None,
        upload_date: None,
        was_live: false,
    })
}

/// Download a YouTube video (≤ 1080p, H.264/VP9, merged to MP4) to `dest`, reporting byte progress.
// ponytail: one full download; the plan's two-pass (audio + proxy first, HQ ranges later) comes with long VODs.
pub async fn download_youtube(url: &str, dest: &Path, progress: crate::models::ProgressFn<'_>) -> Result<()> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let url = youtube_url(url).context("not a YouTube video link")?;
    let mut child = Command::new(crate::tools::resolve("yt-dlp"))
        .args(crate::tools::yt_dlp_base_args())
        .args(["--no-playlist", "--no-warnings", "--newline", "-N", "8"])
        .args(["-f", "bv*[height<=1080][vcodec!^=av01]+ba/b[height<=1080]/b", "--merge-output-format", "mp4"])
        .args(["--progress-template", "download:%(progress.downloaded_bytes)s/%(progress.total_bytes,progress.total_bytes_estimate)s"])
        .arg("-o")
        .arg(dest)
        .args(["--", &url])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("could not start yt-dlp; install it from Settings")?;
    let mut lines = BufReader::new(child.stdout.take().expect("piped")).lines();
    while let Some(line) = lines.next_line().await? {
        if let Some((done, total)) = line.trim().split_once('/')
            && let Ok(done) = done.parse::<f64>()
        {
            let total = total.parse::<f64>().ok().map(|t| t as u64);
            progress(crate::models::Progress { stage: "Downloading video".into(), done: done as u64, total });
        }
    }
    let out = child.wait_with_output().await?;
    if !out.status.success() || !dest.is_file() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("download failed: {}", err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("yt-dlp produced no file"));
    }
    Ok(())
}

/// "30000/1001" → 29.97.
fn parse_rate(r: &str) -> Option<f64> {
    let (n, d) = r.split_once('/')?;
    let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
    (d > 0.0 && n > 0.0).then(|| n / d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_https_youtube_video_links() {
        assert!(youtube_url("https://www.youtube.com/watch?v=abc123").is_some());
        assert!(youtube_url("https://youtu.be/abc123").is_some());
        assert!(youtube_url("https://youtube.com/live/abc123").is_some());
        assert!(youtube_url("http://www.youtube.com/watch?v=abc").is_none());
        assert!(youtube_url("https://evil.com/watch?v=abc").is_none());
        assert!(youtube_url("https://www.youtube.com/@channel").is_none());
        assert!(youtube_url("--exec calc").is_none());
    }

    #[test]
    fn parses_frame_rates() {
        assert!((parse_rate("30000/1001").unwrap() - 29.97).abs() < 0.01);
        assert_eq!(parse_rate("0/0"), None);
    }
}
