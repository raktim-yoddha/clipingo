//! clip: command-line front end to the Clipingo core. Prints JSON.

use anyhow::{Context, Result, bail};
use clipingo_core::models::{self, Progress};
use clipingo_core::transcribe::{Language, TranscribeOptions};
use clipingo_core::{Word, pipeline, scorer, source, system};

const USAGE: &str = "usage:
  clip check                          tools, GPU, video encoders, speech models
  clip probe <file|url>               read a video file or YouTube link
  clip score <words.json>             find clip candidates in a transcript ([{\"w\",\"s\",\"e\"}, …])
  clip transcribe <file> [lang] [refresh]
  clip find <file> [lang] [refresh]   lang: auto (default) | en | hi | hinglish
  clip models                         list local speech models
  clip models install <id|runtime|all>
                                      ids: english, hinglish, hindi; runtime = whisper.cpp";

fn transcribe_options(flags: &[&str]) -> Result<TranscribeOptions> {
    let mut opts = TranscribeOptions::default();
    for &f in flags {
        match f {
            "auto" => opts.language = Language::Auto,
            "en" | "english" => opts.language = Language::English,
            "hi" | "hindi" => opts.language = Language::Hindi,
            "hinglish" => opts.language = Language::Hinglish,
            "refresh" => opts.refresh = true,
            other => bail!("unknown option {other}\n{USAGE}"),
        }
    }
    Ok(opts)
}

fn print_progress(p: Progress) {
    match p.total {
        Some(t) if t > 0 => eprint!("\r{}: {:.0}% ({} / {} MB)   ", p.stage, p.done as f64 * 100.0 / t as f64, p.done >> 20, t >> 20),
        _ => eprint!("\r{}…   ", p.stage),
    }
}

async fn install(id: &str) -> Result<serde_json::Value> {
    let progress = |p: Progress| print_progress(p);
    let mut done = serde_json::Map::new();
    let ids: Vec<&str> = if id == "all" { std::iter::once("runtime").chain(models::MODELS.iter().map(|m| m.id)).collect() } else { vec![id] };
    for id in ids {
        let path = if id == "runtime" { models::install_runtime(&progress).await? } else { models::install(id, &progress).await? };
        eprintln!();
        done.insert(id.to_string(), serde_json::json!(path));
    }
    Ok(done.into())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = match args.as_slice() {
        ["check"] => serde_json::to_value(system::check().await)?,
        ["probe", input] => serde_json::to_value(source::probe(input).await?)?,
        ["score", path] => {
            let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {path}"))?;
            let words: Vec<Word> = serde_json::from_str(&text).context("transcript must be [{\"w\",\"s\",\"e\"}]")?;
            serde_json::to_value(scorer::find(&words, &scorer::FindOptions::default(), None))?
        }
        ["transcribe", input, flags @ ..] => serde_json::to_value(pipeline::transcribe_file(input, &transcribe_options(flags)?).await?)?,
        ["find", input, flags @ ..] => {
            serde_json::to_value(pipeline::find_clips(input, &transcribe_options(flags)?, &scorer::FindOptions::default()).await?)?
        }
        ["models"] => serde_json::json!({ "whisper_cli": models::whisper_cli(), "models": models::status() }),
        ["models", "install", id] => install(id).await?,
        _ => bail!("{USAGE}"),
    };
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
