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

/// One H.264 encoder, probed by actually encoding a few frames (being compiled in proves nothing).
#[derive(Debug, Clone, serde::Serialize)]
pub struct EncoderStatus {
    pub name: &'static str,
    pub label: &'static str,
    pub works: bool,
    /// ffmpeg's own reason when it does not work.
    pub error: Option<String>,
    /// What the user can do about it, when there is something to do.
    pub hint: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SystemReport {
    pub os: &'static str,
    pub tools: Vec<ToolStatus>,
    /// "name, memory, driver" per GPU reported by `nvidia-smi`, empty when no NVIDIA GPU or driver.
    pub nvidia_gpus: Vec<String>,
    /// Encoders in fallback order; empty when ffmpeg is missing.
    pub encoders: Vec<EncoderStatus>,
    /// First working encoder: what renders will use.
    pub selected_encoder: Option<&'static str>,
}

/// Hardware first, CPU last.
#[cfg(not(target_os = "macos"))]
const ENCODERS: &[(&str, &str)] = &[
    ("h264_nvenc", "NVIDIA NVENC"),
    ("h264_qsv", "Intel Quick Sync"),
    ("h264_amf", "AMD AMF"),
    ("h264_mf", "Windows Media Foundation"),
    ("libx264", "CPU (x264)"),
];
#[cfg(target_os = "macos")]
const ENCODERS: &[(&str, &str)] = &[("h264_videotoolbox", "Apple VideoToolbox"), ("libx264", "CPU (x264)")];

/// The line of ffmpeg's stderr that explains the failure, without the `[h264_nvenc @ 0x…] ` prefix.
fn failure_reason(stderr: &str) -> Option<String> {
    let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    // Generic follow-up lines ("Nothing was written…", "Error while opening encoder…") hide the cause.
    let generic = |l: &&str| {
        let l = l.to_ascii_lowercase();
        l.contains("nothing was written") || l.contains("error while opening encoder") || l.contains("conversion failed")
    };
    let line = lines.iter().find(|l| !generic(l)).or(lines.first())?;
    let text = match (line.starts_with('['), line.find("] ")) {
        (true, Some(i)) => &line[i + 2..],
        _ => line,
    };
    Some(text.to_string())
}

/// User-facing advice for an encoder failure; `None` when the hardware simply isn't there.
fn hint_for(encoder: &str, reason: &str, has_nvidia_gpu: bool) -> Option<String> {
    let r = reason.to_ascii_lowercase();
    let driver_too_old = r.contains("driver does not support") || r.contains("api version") || r.contains("minimum required nvidia driver");
    if encoder.contains("nvenc") && (driver_too_old || has_nvidia_gpu) {
        return Some(if driver_too_old {
            format!("Update your NVIDIA GPU driver to enable NVENC hardware encoding ({reason}).")
        } else {
            format!("An NVIDIA GPU is present but NVENC did not start; updating the GPU driver usually fixes this ({reason}).")
        });
    }
    if r.contains("driver") {
        return Some(format!("Update your GPU driver to enable {encoder} ({reason})."));
    }
    None
}

async fn probe_encoder(name: &'static str, label: &'static str, has_nvidia_gpu: bool) -> EncoderStatus {
    let run = Command::new("ffmpeg")
        .args(["-hide_banner", "-v", "error", "-f", "lavfi", "-i", "testsrc2=s=1280x720:r=30:d=1", "-frames:v", "15"])
        .args(["-c:v", name, "-f", "null", "-"])
        .kill_on_drop(true)
        .output();
    let (works, error) = match tokio::time::timeout(Duration::from_secs(20), run).await {
        Ok(Ok(out)) if out.status.success() => (true, None),
        Ok(Ok(out)) => (false, failure_reason(&String::from_utf8_lossy(&out.stderr)).or(Some("encoder failed to open".into()))),
        Ok(Err(e)) => (false, Some(format!("could not run ffmpeg: {e}"))),
        Err(_) => (false, Some("timed out after 20 s".into())),
    };
    let hint = error.as_deref().and_then(|e| hint_for(name, e, has_nvidia_gpu));
    EncoderStatus { name, label, works, error, hint }
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
    let nvidia_gpus: Vec<String> = match Command::new("nvidia-smi")
        .args(["--query-gpu=name,memory.total,driver_version", "--format=csv,noheader"])
        .output()
        .await
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect(),
        _ => Vec::new(),
    };
    let has_ffmpeg = tools.iter().any(|t| t.name == "ffmpeg" && t.found);
    let encoders = if has_ffmpeg {
        let has_nvidia = !nvidia_gpus.is_empty();
        futures_join(ENCODERS.iter().map(move |&(name, label)| probe_encoder(name, label, has_nvidia))).await
    } else {
        Vec::new()
    };
    let selected_encoder = select_encoder(&encoders);
    SystemReport { os: std::env::consts::OS, tools, nvidia_gpus, encoders, selected_encoder }
}

/// First working encoder in fallback order.
pub fn select_encoder(encoders: &[EncoderStatus]) -> Option<&'static str> {
    encoders.iter().find(|e| e.works).map(|e| e.name)
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

#[cfg(test)]
mod tests {
    use super::*;

    const NVENC_OLD_DRIVER: &str = "[h264_nvenc @ 00000179dd5403c0] Driver does not support the required nvenc API version. Required: 13.1 Found: 13.0\n\
        [h264_nvenc @ 00000179dd5403c0] The minimum required Nvidia driver for nvenc is 590.00 or newer\n\
        [vost#0:0/h264_nvenc @ 0000017] Error while opening encoder - maybe incorrect parameters such as bit_rate, rate, width or height\n\
        [out#0/null @ 0000018826b05740] Nothing was written into output file, because at least one of its streams received no packets.";

    #[test]
    fn reason_skips_generic_lines_and_prefix() {
        assert_eq!(
            failure_reason(NVENC_OLD_DRIVER).as_deref(),
            Some("Driver does not support the required nvenc API version. Required: 13.1 Found: 13.0")
        );
        assert_eq!(failure_reason("[out#0/null @ 01] Nothing was written into output file").as_deref(), Some("Nothing was written into output file"));
        assert_eq!(failure_reason(""), None);
    }

    #[test]
    fn old_nvidia_driver_gets_update_hint() {
        let reason = failure_reason(NVENC_OLD_DRIVER).unwrap();
        let hint = hint_for("h264_nvenc", &reason, true).unwrap();
        assert!(hint.starts_with("Update your NVIDIA GPU driver"), "{hint}");
    }

    #[test]
    fn missing_hardware_gets_no_hint() {
        assert_eq!(hint_for("h264_amf", "DLL amfrt64.dll failed to open", false), None);
        assert_eq!(hint_for("h264_nvenc", "Cannot load nvcuda.dll", false), None);
        assert!(hint_for("h264_nvenc", "Cannot load nvcuda.dll", true).is_some(), "NVIDIA GPU present but NVENC dead");
    }

    #[test]
    fn selects_first_working_in_fallback_order() {
        let enc = |name, works| EncoderStatus { name, label: "", works, error: None, hint: None };
        let list = [enc("h264_nvenc", false), enc("h264_qsv", true), enc("libx264", true)];
        assert_eq!(select_encoder(&list), Some("h264_qsv"));
        assert_eq!(select_encoder(&[enc("h264_nvenc", false)]), None);
    }
}
