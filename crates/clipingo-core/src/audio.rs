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
    ffmpeg(
        &["-i", &i, "-vn", "-ac", "1", "-ar", &rate, "-c:a", "pcm_s16le", "-fflags", "+bitexact", &o],
        "audio extraction",
    )
    .await?;
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

fn wav_header(data_len: u32) -> Vec<u8> {
    let mut h = b"RIFF".to_vec();
    h.extend_from_slice(&(36 + data_len).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0"); // PCM, mono
    h.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    h.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    h.extend_from_slice(b"\x02\0\x10\0data"); // 2-byte frames, 16 bit
    h.extend_from_slice(&data_len.to_le_bytes());
    h
}

/// Writes each chunk of a 16 kHz mono s16 WAV to `dir/NNNN.wav`: its `(start, end)` spans (seconds) joined by
/// `gap_s` of silence. Returns the file names.
pub fn write_chunks(wav: &Path, chunks: &[Vec<(f64, f64)>], gap_s: f64, dir: &Path) -> Result<Vec<PathBuf>> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut file = std::fs::File::open(wav).with_context(|| format!("cannot open {}", wav.display()))?;
    let mut header = vec![0u8; 4096];
    let n = file.read(&mut header)?;
    let (offset, len) = wav_data_range(&header[..n]).context("not a PCM WAV file")?;
    let byte = |t: f64| ((t.max(0.0) * SAMPLE_RATE as f64) as usize * 2).min(len);
    let gap = vec![0u8; byte(gap_s)];
    let mut names = Vec::with_capacity(chunks.len());
    for (i, spans) in chunks.iter().enumerate() {
        let mut pcm = Vec::new();
        for (k, &(s, e)) in spans.iter().enumerate() {
            if k > 0 {
                pcm.extend_from_slice(&gap);
            }
            let at = pcm.len();
            pcm.resize(at + byte(e).saturating_sub(byte(s)), 0);
            file.seek(SeekFrom::Start((offset + byte(s)) as u64))?;
            file.read_exact(&mut pcm[at..])?;
        }
        let name = PathBuf::from(format!("{i:04}.wav"));
        let mut out = std::io::BufWriter::new(std::fs::File::create(dir.join(&name))?);
        out.write_all(&wav_header(pcm.len() as u32))?;
        out.write_all(&pcm)?;
        out.flush()?;
        names.push(name);
    }
    Ok(names)
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
        let samples =
            buf[..filled - filled % 2].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f64 / 32768.0);
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

/// Pull word edges out of silences: word times run on until the next word (swallowing pauses) or start a little
/// early (DTW alignment). A word ending in punctuation is followed by its pause, so the pause ends it; otherwise
/// the word keeps the side of the silence that holds more of it. Words entirely inside a silence are left alone
/// (quiet speech is still speech).
pub fn refine_words(words: &mut [crate::Word], silences: &[(f64, f64)]) {
    const MIN_WORD_S: f64 = 0.08;
    let mut first = 0;
    for w in words.iter_mut() {
        while first < silences.len() && silences[first].1 <= w.start {
            first += 1;
        }
        let original_end = w.end;
        let ends_clause = w
            .text
            .trim_end_matches(['"', '\'', ')', '”', '’'])
            .ends_with(['.', ',', '?', '!', ':', ';', '…', '।', '॥']);
        for &(s, e) in silences[first..].iter().take_while(|(s, _)| *s < original_end) {
            let (before, after) = (s - w.start, w.end - e);
            if before.max(after) <= MIN_WORD_S {
                continue;
            }
            if before > MIN_WORD_S && (ends_clause || before >= after) {
                w.end = w.end.min(s);
            } else {
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

        // DTW-style early start: the word is really spoken after the pause.
        let mut early = vec![crate::Word { text: "pehle".into(), start: 0.9, end: 2.8 }];
        refine_words(&mut early, &s);
        assert!((early[0].start - 2.5).abs() < 0.02 && (early[0].end - 2.8).abs() < 1e-9, "{early:?}");

        // Same timing, but the word closes a sentence: the pause follows it.
        let mut last = vec![crate::Word { text: "faster.".into(), start: 0.9, end: 2.8 }];
        refine_words(&mut last, &s);
        assert!((last[0].start - 0.9).abs() < 1e-9 && (last[0].end - 1.0).abs() < 0.02, "{last:?}");
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
    fn writes_chunks_as_wavs() {
        let s: Vec<i16> = (1..=32_000).map(|i| i as i16).collect(); // 2 s, sample k holds k + 1
        let dir = std::env::temp_dir().join(format!("clipingo-chunks-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.wav");
        std::fs::write(&src, wav_bytes(&s, true)).unwrap();
        let names = write_chunks(&src, &[vec![(0.5, 1.0), (1.5, 9.0)]], 0.25, &dir).unwrap();
        let wav = std::fs::read(dir.join(&names[0])).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        let (off, len) = wav_data_range(&wav).unwrap();
        assert_eq!((off, len), (44, 2 * (8000 + 4000 + 8000)), "0.5 s + 0.25 s gap + 0.5 s clamped to the end");
        let sample = |k: usize| i16::from_le_bytes([wav[off + 2 * k], wav[off + 2 * k + 1]]);
        assert_eq!((sample(0), sample(7999)), (8001, 16_000), "first span");
        assert_eq!((sample(8000), sample(11_999)), (0, 0), "silent gap");
        assert_eq!(sample(12_000), 24_001, "second span starts at 1.5 s");
    }

    #[test]
    fn finds_data_after_list_chunk() {
        let bytes = wav_bytes(&[1, 2, 3], true);
        let (off, len) = wav_data_range(&bytes).unwrap();
        assert_eq!(len, 6);
        assert_eq!(&bytes[off..off + 2], &1i16.to_le_bytes());
    }
}
