//! Audio extraction (ffmpeg) and the loudness envelope the scorer uses for vocal energy.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tokio::process::Command;

/// Speech models want 16 kHz mono.
pub const SAMPLE_RATE: u32 = 16_000;
/// Loudness envelope resolution (values per second): 10 ms steps, fine enough to find pauses between words.
pub const ENVELOPE_RATE: f64 = 100.0;

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

/// Stretches of silence (seconds) at least `min_len_s` long. The threshold sits between the room's
/// floor (10th percentile) and the talking level (90th percentile), so it adapts to each recording.
/// Returns nothing when speech and floor are too close to tell apart (music under everything).
pub fn silences(envelope: &[f32], rate: f64, min_len_s: f64) -> Vec<(f64, f64)> {
    let db: Vec<f64> = envelope.iter().map(|&r| 20.0 * (r as f64).max(1e-5).log10()).collect();
    if db.len() < 10 {
        return Vec::new();
    }
    let mut sorted = db.clone();
    sorted.sort_by(f64::total_cmp);
    let (floor, talk) = (sorted[sorted.len() / 10], sorted[sorted.len() * 9 / 10]);
    if talk - floor < 6.0 {
        return Vec::new();
    }
    let threshold = floor + 0.3 * (talk - floor);
    let min_len = (min_len_s * rate).ceil() as usize;
    let mut out = Vec::new();
    let mut run_start = None;
    for (i, &v) in db.iter().chain(std::iter::once(&f64::INFINITY)).enumerate() {
        match (v < threshold, run_start) {
            (true, None) => run_start = Some(i),
            (false, Some(s)) => {
                if i - s >= min_len {
                    out.push((s as f64 / rate, i as f64 / rate));
                }
                run_start = None;
            }
            _ => {}
        }
    }
    out
}

/// Pull word edges out of silences: whisper's word segments run on until the next word, swallowing pauses.
/// A silence in a word's tail ends the word there; one at its head starts it later. Words entirely inside
/// a silence are left alone (quiet speech is still speech).
pub fn refine_words(words: &mut [crate::Word], silences: &[(f64, f64)]) {
    const MIN_WORD_S: f64 = 0.08;
    let mut first = 0;
    for w in words.iter_mut() {
        while first < silences.len() && silences[first].1 <= w.start {
            first += 1;
        }
        let original_end = w.end;
        for &(s, e) in silences[first..].iter().take_while(|(s, _)| *s < original_end) {
            if s > w.start + MIN_WORD_S {
                w.end = w.end.min(s);
            } else if e < w.end - MIN_WORD_S {
                w.start = w.start.max(e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_pauses_and_trims_swallowing_words() {
        // 100 Hz: 1 s speech, 1.5 s silence, 1 s speech.
        let mut env = vec![0.3f32; 100];
        env.extend(vec![0.001f32; 150]);
        env.extend(vec![0.3f32; 100]);
        let s = silences(&env, 100.0, 0.25);
        assert_eq!(s.len(), 1);
        assert!((s[0].0 - 1.0).abs() < 0.02 && (s[0].1 - 2.5).abs() < 0.02, "{s:?}");

        // whisper-style words with no gap: "tested," runs through the pause.
        let mut words = vec![
            crate::Word { text: "never".into(), start: 0.2, end: 0.6 },
            crate::Word { text: "tested,".into(), start: 0.6, end: 2.5 },
            crate::Word { text: "again".into(), start: 2.5, end: 3.0 },
        ];
        refine_words(&mut words, &s);
        assert!((words[1].end - 1.0).abs() < 0.02, "pause moved out of the word: {words:?}");
        assert!((words[2].start - 2.5).abs() < 0.02);
    }

    #[test]
    fn music_bed_gives_no_silences() {
        let env: Vec<f32> = (0..500).map(|i| 0.2 + 0.01 * (i % 7) as f32).collect();
        assert!(silences(&env, 100.0, 0.25).is_empty());
    }

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
        // 10 ms silence, 10 ms full-scale square wave, 5 ms tail (one envelope value per 10 ms).
        let mut s = vec![0i16; 160];
        s.extend((0..160).map(|i| if i % 2 == 0 { 32767 } else { -32767 }));
        s.extend(vec![0i16; 80]);
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
