//! External command-line tools: where to find them, and installing the ones Clipingo manages
//! (yt-dlp and its JavaScript runtime deno) into `<data_dir>/bin`.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use tokio::process::Command;

use crate::models::{ProgressFn, download};

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

pub fn bin_dir() -> PathBuf {
    crate::data_dir().join("bin")
}

/// The tool we installed, else whatever is on PATH (spawned by bare name).
pub fn resolve(name: &str) -> PathBuf {
    let ours = bin_dir().join(exe(name));
    if ours.is_file() { ours } else { PathBuf::from(name) }
}

/// Arguments every yt-dlp call needs: point it at our deno for YouTube's JavaScript challenges.
pub fn yt_dlp_base_args() -> Vec<String> {
    let deno = bin_dir().join(exe("deno"));
    if deno.is_file() { vec!["--js-runtimes".into(), format!("deno:{}", deno.display())] } else { Vec::new() }
}

/// Official release URLs. yt-dlp is a single binary; deno ships as a zip.
fn release_url(name: &str) -> Option<&'static str> {
    match (name, std::env::consts::OS, std::env::consts::ARCH) {
        ("yt-dlp", "windows", _) => Some("https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"),
        ("yt-dlp", "macos", _) => Some("https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos"),
        ("yt-dlp", "linux", "x86_64") => Some("https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux"),
        ("deno", "windows", "x86_64") => {
            Some("https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip")
        }
        ("deno", "macos", "aarch64") => {
            Some("https://github.com/denoland/deno/releases/latest/download/deno-aarch64-apple-darwin.zip")
        }
        ("deno", "macos", "x86_64") => {
            Some("https://github.com/denoland/deno/releases/latest/download/deno-x86_64-apple-darwin.zip")
        }
        ("deno", "linux", "x86_64") => {
            Some("https://github.com/denoland/deno/releases/latest/download/deno-x86_64-unknown-linux-gnu.zip")
        }
        _ => None,
    }
}

/// Install anything the app manages: "runtime" (whisper.cpp), "yt-dlp", "deno", or a speech model id.
pub async fn install_component(id: &str, progress: ProgressFn<'_>) -> Result<PathBuf> {
    match id {
        "runtime" => crate::models::install_runtime(progress).await,
        "yt-dlp" | "deno" => install(id, progress).await,
        model => crate::models::install(model, progress).await,
    }
}

/// Install `yt-dlp` or `deno` into the bin folder; returns the executable path.
pub async fn install(name: &str, progress: ProgressFn<'_>) -> Result<PathBuf> {
    let target = bin_dir().join(exe(name));
    if target.is_file() {
        return Ok(target);
    }
    let url = release_url(name)
        .with_context(|| format!("no {name} build known for this OS; install it and put it on PATH"))?;
    if url.ends_with(".zip") {
        let zip =
            download(url, &crate::data_dir().join("downloads").join(format!("{name}.zip")), name, progress).await?;
        let out = Command::new("tar")
            .arg("-xf")
            .arg(&zip)
            .arg("-C")
            .arg(bin_dir())
            .output()
            .await
            .context("could not run tar")?;
        if !out.status.success() {
            bail!("unpacking {name} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        tokio::fs::remove_file(&zip).await.ok();
    } else {
        download(url, &target, name, progress).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    target.is_file().then_some(target).with_context(|| format!("{name} not found after install"))
}
