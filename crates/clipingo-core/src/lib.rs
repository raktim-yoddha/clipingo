//! Clipingo headless core: everything the UIs and the CLI share.

pub mod camera;
pub mod pacing;
pub mod scorer;
pub mod source;
pub mod system;

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
