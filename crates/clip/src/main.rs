//! clip: command-line front end to the Clipingo core. Prints JSON.

use anyhow::{Context, Result, bail};
use clipingo_core::transcribe::{Language, Script, TranscribeOptions};
use clipingo_core::{Word, pipeline, scorer, source, system};

const USAGE: &str = "usage:
  clip check                          tools, GPU and video encoders on this machine
  clip probe <file|url>               read a video file or YouTube link
  clip score <words.json>             find clip candidates in a transcript ([{\"w\",\"s\",\"e\"}, …])
  clip transcribe <file> [lang] [roman] [refresh]
  clip find <file> [lang] [roman] [refresh]
                                      lang: auto (default) | en | hi | hinglish
                                      Hindi/Hinglish use Sarvam Saaras v3 (SARVAM_API_KEY),
                                      English uses whisper-cli + a ggml model";

fn transcribe_options(flags: &[&str]) -> Result<TranscribeOptions> {
    let mut opts = TranscribeOptions::default();
    for &f in flags {
        match f {
            "auto" => opts.language = Language::Auto,
            "en" | "english" => opts.language = Language::English,
            "hi" | "hindi" => opts.language = Language::Hindi,
            "hinglish" => opts.language = Language::Hinglish,
            "roman" => opts.script = Script::Roman,
            "refresh" => opts.refresh = true,
            other => bail!("unknown option {other}\n{USAGE}"),
        }
    }
    Ok(opts)
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
        _ => bail!("{USAGE}"),
    };
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
