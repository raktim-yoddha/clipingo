//! Audio extraction (ffmpeg) and the loudness envelope the scorer uses for vocal energy.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tokio::process::Command;

/// Speech models want 16 kHz mono.
pub const SAMPLE_RATE: u32 = 16_000;
/// Loudness envelope resolution (values per second), as the scorer expects.
pub const ENVELOPE_RATE: f64 = 10.0;

async fn ffmpeg(args: &[&str], what: &str) -> Result<()> {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-v", "error", "-y"])
        .args(args)
        .kill_on_drop(true)
        .output()
        .await
        .context("could not start ffmpeg; run System check")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("{what} failed: {}", err.lines().find(|l| !l.trim().is_empty()).unwrap_or("unknown ffmpeg error"));
    }
    Ok(())
}

/// 16 kHz mono 16-bit WAV of the whole source (whisper input and loudness source).
pub async fn extract_wav(input: &Path, out: &Path) -> Result<PathBuf> {
    let (i, o) = (input.to_string_lossy(), out.to_string_lossy());
    let rate = SAMPLE_RATE.to_string();
    ffmpeg(&["-i", &i, "-vn", "-ac", "1", "-ar", &rate, "-c:a", "pcm_s16le", "-fflags", "+bitexact", &o], "audio extraction").await?;
    Ok(out.to_path_buf())
}

/// 64 kbps mono MP3 slice `[start_s, start_s + dur_s)` for upload (a 2 h slice is ~58 MB instead of ~230 MB WAV).
pub async fn extract_mp3_slice(wav: &Path, start_s: f64, dur_s: f64, out: &Path) -> Result<PathBuf> {
    let (i, o) = (wav.to_string_lossy(), out.to_string_lossy());
    let (ss, t) = (format!("{start_s:.3}"), format!("{dur_s:.3}"));
    ffmpeg(&["-ss", &ss, "-t", &t, "-i", &i, "-c:a", "libmp3lame", "-b:a", "64k", &o], "audio slicing").await?;
    Ok(out.to_path_buf())
}

/// Byte range of the PCM samples in a RIFF/WAVE file (walks chunks; ffmpeg may add LIST before data).
fn wav_data_range(header: &[u8]) -> Option<(usize, usize)> {
    if header.len() < 12 || &header[..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12;
    while pos + 8 <= header.len() {
        let id = &header[pos..pos + 4];
        let size = u32::from_le_bytes(header[pos + 4..pos + 8].try_into().ok()?) as usize;
        if id == b"data" {
            return Some((pos + 8, size));
        }
        pos += 8 + size + (size & 1);
    }
    None
}

/// RMS loudness of a 16 kHz mono s16 WAV at `ENVELOPE_RATE` values per second, streamed in blocks.
pub fn loudness_envelope(wav: &Path) -> Result<Vec<f32>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(wav).with_context(|| format!("cannot open {}", wav.display()))?;
    let mut header = vec![0u8; 4096];
    let n = file.read(&mut header)?;
    let (offset, len) = wav_data_range(&header[..n]).context("not a PCM WAV file")?;
    file.seek(SeekFrom::Start(offset as u64))?;

    let window = (SAMPLE_RATE as f64 / ENVELOPE_RATE) as usize; // samples per envelope value
    let mut reader = std::io::BufReader::new(file.take(len as u64));
    let mut buf = vec![0u8; window * 2];
    let mut env = Vec::new();
    loop {
        let mut filled = 0;
        while filled < buf.len() {
            match reader.read(&mut buf[filled..])? {
                0 => break,
                k => filled += k,
            }
        }
        if filled < 2 {
            break;
        }
        let samples = buf[..filled - filled % 2].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f64 / 32768.0);
        let (sum, count) = samples.fold((0.0, 0usize), |(s, c), x| (s + x * x, c + 1));
        env.push((sum / count as f64).sqrt() as f32);
        if filled < buf.len() {
            break;
        }
    }
    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav_bytes(samples: &[i16], extra_chunk: bool) -> Vec<u8> {
        let mut data = Vec::new();
        for s in samples {
            data.extend_from_slice(&s.to_le_bytes());
        }
        let mut out = b"RIFF\0\0\0\0WAVE".to_vec();
        out.extend_from_slice(b"fmt \x10\0\0\0\x01\0\x01\0\x80\x3e\0\0\0\x7d\0\0\x02\0\x10\0");
        if extra_chunk {
            out.extend_from_slice(b"LIST\x03\0\0\0abc\0"); // odd size + pad byte
        }
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend(data);
        out
    }

    #[test]
    fn envelope_tracks_loud_and_quiet_tenths() {
        // 0.1 s silence, 0.1 s full-scale square wave, 0.05 s tail.
        let mut s = vec![0i16; 1600];
        s.extend((0..1600).map(|i| if i % 2 == 0 { 32767 } else { -32767 }));
        s.extend(vec![0i16; 800]);
        let path = std::env::temp_dir().join(format!("clipingo-env-test-{}.wav", std::process::id()));
        std::fs::write(&path, wav_bytes(&s, true)).unwrap();
        let env = loudness_envelope(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(env.len(), 3);
        assert!(env[0] < 0.01 && env[1] > 0.99 && env[2] < 0.01, "{env:?}");
    }

    #[test]
    fn finds_data_after_list_chunk() {
        let bytes = wav_bytes(&[1, 2, 3], true);
        let (off, len) = wav_data_range(&bytes).unwrap();
        assert_eq!(len, 6);
        assert_eq!(&bytes[off..off + 2], &1i16.to_le_bytes());
    }
}
