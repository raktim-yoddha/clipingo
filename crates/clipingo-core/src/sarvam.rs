//! Sarvam Saaras v3 batch speech-to-text (Hindi / Hinglish).
//!
//! Flow: create job → presigned upload URLs → PUT audio → start → poll status →
//! presigned download URLs → one JSON per input. Each input may be up to 2 h, up to 20 per job.
//! Saaras returns phrase-level timestamps only; word times are spread inside each phrase.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::transcribe::{Transcript, WordTiming};
use crate::{Word, audio};

const API: &str = "https://api.sarvam.ai/speech-to-text/job/v1";
const MODEL: &str = "saaras:v3";
/// Stay under the 2 h per-file limit.
const PIECE_S: f64 = 6900.0;
const MAX_FILES: usize = 20;
const POLL_EVERY: Duration = Duration::from_secs(5);
const MAX_WAIT: Duration = Duration::from_secs(4 * 3600);

/// The key from `SARVAM_API_KEY` (the app will pass it from OS secure storage the same way).
pub fn api_key() -> Option<String> {
    std::env::var("SARVAM_API_KEY").ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty())
}

/// reqwest errors carry the URL; presigned URLs embed access tokens, so never show it.
fn net(e: reqwest::Error) -> anyhow::Error {
    anyhow::anyhow!("Sarvam request failed: {}", e.without_url())
}

async fn call(key: &str, req: reqwest::RequestBuilder, what: &str) -> Result<Value> {
    let resp = req.header("api-subscription-key", key).send().await.map_err(net)?;
    let status = resp.status();
    let body = resp.text().await.map_err(net)?;
    if !status.is_success() {
        bail!("Sarvam {what} failed ({status}): {}", body.chars().take(400).collect::<String>());
    }
    serde_json::from_str(&body).with_context(|| format!("Sarvam {what} returned invalid JSON"))
}

pub async fn transcribe(key: &str, wav: &Path, duration_s: f64, language_code: &str, mode: &str, work: &Path) -> Result<Transcript> {
    let pieces = (duration_s / PIECE_S).ceil().max(1.0) as usize;
    if pieces > MAX_FILES {
        bail!("source is too long for one Sarvam job ({pieces} pieces of 1h55m, max {MAX_FILES})");
    }
    let mut names = Vec::new();
    for i in 0..pieces {
        let name = format!("part-{i:03}.mp3");
        audio::extract_mp3_slice(wav, i as f64 * PIECE_S, PIECE_S, &work.join(&name)).await?;
        names.push(name);
    }

    let client = reqwest::Client::builder().connect_timeout(Duration::from_secs(30)).build().map_err(net)?;
    let job = call(
        key,
        client.post(API).json(&json!({ "job_parameters": {
            "model": MODEL, "mode": mode, "language_code": language_code, "with_timestamps": true
        }})),
        "create job",
    )
    .await?;
    let job_id = job["job_id"].as_str().context("Sarvam returned no job_id")?.to_string();

    let up = call(key, client.post(format!("{API}/upload-files")).json(&json!({ "job_id": job_id, "files": names })), "upload URLs").await?;
    for name in &names {
        let url = up["upload_urls"][name]["file_url"].as_str().with_context(|| format!("no upload URL for {name}"))?;
        let bytes = tokio::fs::read(work.join(name)).await?;
        let resp = client
            .put(url)
            .header("x-ms-blob-type", "BlockBlob")
            .header("Content-Type", "audio/mpeg")
            .timeout(Duration::from_secs(1800))
            .body(bytes)
            .send()
            .await
            .map_err(net)?;
        if !resp.status().is_success() {
            bail!("uploading {name} to Sarvam failed ({})", resp.status());
        }
    }

    call(key, client.post(format!("{API}/{job_id}/start")), "start job").await?;

    let started = std::time::Instant::now();
    let status = loop {
        let s = call(key, client.get(format!("{API}/{job_id}/status")), "job status").await?;
        match s["job_state"].as_str() {
            Some("Completed") => break s,
            Some("Failed") => bail!("Sarvam job failed: {}", s["error_message"].as_str().unwrap_or("no reason given")),
            _ if started.elapsed() > MAX_WAIT => bail!("Sarvam job {job_id} did not finish within 4 hours"),
            _ => tokio::time::sleep(POLL_EVERY).await,
        }
    };

    // Map each output JSON back to the input piece (and so its time offset).
    let mut outputs: Vec<(String, f64)> = Vec::new();
    for detail in status["job_details"].as_array().into_iter().flatten() {
        let input = detail["inputs"][0]["file_name"].as_str().unwrap_or_default();
        let Some(index) = names.iter().position(|n| n == input) else { continue };
        if let Some(out) = detail["outputs"][0]["file_name"].as_str() {
            outputs.push((out.to_string(), index as f64 * PIECE_S));
        } else {
            bail!("Sarvam could not transcribe {input}: {}", detail["error_message"].as_str().unwrap_or("no reason given"));
        }
    }
    outputs.sort_by(|a, b| a.1.total_cmp(&b.1));
    let files: Vec<&str> = outputs.iter().map(|o| o.0.as_str()).collect();
    let down = call(key, client.post(format!("{API}/download-files")).json(&json!({ "job_id": job_id, "files": files })), "download URLs").await?;

    let mut words = Vec::new();
    let mut language = None;
    for (file, offset) in &outputs {
        let url = down["download_urls"][file]["file_url"].as_str().with_context(|| format!("no download URL for {file}"))?;
        let body = client.get(url).send().await.map_err(net)?.text().await.map_err(net)?;
        let piece_len = (duration_s - offset).min(PIECE_S);
        let (w, lang) = parse_output(&body, *offset, piece_len)?;
        words.extend(w);
        language = language.or(lang);
    }
    Ok(Transcript { engine: format!("sarvam:{MODEL}:{mode}"), language, word_timing: WordTiming::Estimated, words })
}

/// Words with times spread across `[start, end)` by length (longer words take longer to say).
// ponytail: proportional estimate; swap for CTC forced alignment (plan §6) when caption timing needs it.
pub fn spread_words(text: &str, start: f64, end: f64) -> Vec<Word> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let weight = |t: &str| t.chars().count() as f64 + 1.0;
    let total: f64 = tokens.iter().map(|t| weight(t)).sum();
    let span = (end - start).max(0.0);
    let mut t = start;
    tokens
        .into_iter()
        .map(|tok| {
            let d = span * weight(tok) / total;
            let w = Word { text: tok.to_string(), start: t, end: t + d * 0.92 };
            t += d;
            w
        })
        .collect()
}

/// One output JSON → words on the source timeline (`offset` = piece start).
pub fn parse_output(body: &str, offset: f64, piece_len: f64) -> Result<(Vec<Word>, Option<String>)> {
    let v: Value = serde_json::from_str(body).context("Sarvam output is not JSON")?;
    let language = v["language_code"].as_str().map(str::to_string);
    let mut words = Vec::new();
    let ts = &v["timestamps"];
    if let (Some(texts), Some(starts), Some(ends)) =
        (ts["words"].as_array(), ts["start_time_seconds"].as_array(), ts["end_time_seconds"].as_array())
    {
        for ((text, s), e) in texts.iter().zip(starts).zip(ends) {
            if let (Some(text), Some(s), Some(e)) = (text.as_str(), s.as_f64(), e.as_f64()) {
                words.extend(spread_words(text, offset + s, offset + e));
            }
        }
    } else if let Some(entries) = v["diarized_transcript"]["entries"].as_array() {
        for e in entries {
            if let (Some(text), Some(s), Some(end)) = (e["transcript"].as_str(), e["start_time_seconds"].as_f64(), e["end_time_seconds"].as_f64()) {
                words.extend(spread_words(text, offset + s, offset + end));
            }
        }
    } else if let Some(text) = v["transcript"].as_str() {
        words.extend(spread_words(text, offset, offset + piece_len));
    }
    Ok((words, language))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spreads_words_inside_phrase_in_order() {
        let w = spread_words("bhai kya scene hai", 10.0, 12.0);
        assert_eq!(w.len(), 4);
        assert!((w[0].start - 10.0).abs() < 1e-9 && w[3].end <= 12.0);
        assert!(w.windows(2).all(|p| p[0].end <= p[1].start));
        assert!(w[2].end - w[2].start > w[1].end - w[1].start, "longer word gets more time");
    }

    #[test]
    fn parses_phrase_timestamps_with_piece_offset() {
        let body = r#"{"transcript":"क्या आप तैयार हैं? चलो शुरू करते हैं","language_code":"hi-IN",
            "timestamps":{"words":["क्या आप तैयार हैं?","चलो शुरू करते हैं"],"start_time_seconds":[0.5,3.0],"end_time_seconds":[2.5,5.0]}}"#;
        let (words, lang) = parse_output(body, 6900.0, 6900.0).unwrap();
        assert_eq!(lang.as_deref(), Some("hi-IN"));
        assert_eq!(words.len(), 8);
        assert!((words[0].start - 6900.5).abs() < 1e-9);
        assert_eq!(words[4].text, "चलो");
        assert!((words[4].start - 6903.0).abs() < 1e-9);
    }

    #[test]
    fn falls_back_to_diarized_entries_then_plain_text() {
        let body = r#"{"diarized_transcript":{"entries":[{"transcript":"hello bhai","start_time_seconds":1.0,"end_time_seconds":2.0,"speaker_id":"0"}]}}"#;
        assert_eq!(parse_output(body, 0.0, 10.0).unwrap().0.len(), 2);
        let plain = parse_output(r#"{"transcript":"ek do teen"}"#, 0.0, 3.0).unwrap().0;
        assert_eq!(plain.len(), 3);
        assert!(plain[2].end <= 3.0);
    }
}
