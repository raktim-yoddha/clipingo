//! Clipingo headless core: everything the UIs and the CLI share.

pub mod audio;
pub mod camera;
pub mod captions;
pub mod framing;
pub mod models;
pub mod pacing;
pub mod pipeline;
pub mod render;
pub mod scorer;
pub mod source;
pub mod system;
pub mod tools;
pub mod transcribe;

/// Per-user data folder (models, cache): `%LOCALAPPDATA%\Clipingo`, `~/Library/Application Support/Clipingo`,
/// or `$XDG_DATA_HOME/clipingo`. `CLIPINGO_DATA_DIR` overrides.
pub fn data_dir() -> std::path::PathBuf {
    use std::path::PathBuf;
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(dir) = env("CLIPINGO_DATA_DIR") {
        return dir;
    }
    if cfg!(windows) {
        env("LOCALAPPDATA").unwrap_or_else(std::env::temp_dir).join("Clipingo")
    } else if cfg!(target_os = "macos") {
        env("HOME").unwrap_or_else(std::env::temp_dir).join("Library/Application Support/Clipingo")
    } else {
        env("XDG_DATA_HOME")
            .or_else(|| env("HOME").map(|h| h.join(".local/share")))
            .unwrap_or_else(std::env::temp_dir)
            .join("clipingo")
    }
}

/// Where exported clips go, one folder per video: `~/Videos/Clipingo` (Windows, macOS), `~/videos/clipingo`
/// elsewhere. `CLIPINGO_OUTPUT_DIR` overrides. Outside the app-data folder on purpose: it is easy to find, and
/// packaged parents (MSIX) redirect `AppData` writes of their child processes, never the Videos folder.
// ponytail: `%USERPROFILE%\Videos`, not the Known Folder API; a Videos folder moved to OneDrive is not followed.
pub fn output_dir() -> std::path::PathBuf {
    use std::path::PathBuf;
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(dir) = env("CLIPINGO_OUTPUT_DIR") {
        return dir;
    }
    let home = env(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_else(std::env::temp_dir);
    if cfg!(any(windows, target_os = "macos")) {
        home.join("Videos").join("Clipingo")
    } else {
        home.join("videos").join("clipingo")
    }
}

/// One transcribed word, times in seconds on the source timeline.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Word {
    #[serde(rename = "w")]
    pub text: String,
    #[serde(rename = "s")]
    pub start: f64,
    #[serde(rename = "e")]
    pub end: f64,
}

/// Path of the daemon's local endpoint (named pipe on Windows, Unix socket elsewhere).
pub fn endpoint() -> String {
    if cfg!(windows) {
        r"\\.\pipe\clipingo".to_string()
    } else {
        std::env::temp_dir().join("clipingo.sock").to_string_lossy().into_owned()
    }
}
