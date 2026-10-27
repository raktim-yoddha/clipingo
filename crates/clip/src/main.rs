//! clip: command-line front end to the Clipingo core. Prints JSON.

use anyhow::{Context, Result, bail};
use clipingo_core::{Word, scorer, source, system};

const USAGE: &str = "usage:
  clip check                 tools and GPU this machine has
  clip probe <file|url>      read a video file or YouTube link
  clip score <words.json>    find clip candidates in a transcript ([{\"w\",\"s\",\"e\"}, …])";

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["check"] => serde_json::to_value(system::check().await)?,
        ["probe", input] => serde_json::to_value(source::probe(input).await?)?,
        ["score", path] => {
            let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {path}"))?;
            let words: Vec<Word> = serde_json::from_str(&text).context("transcript must be [{\"w\",\"s\",\"e\"}]")?;
            serde_json::to_value(scorer::find(&words, &scorer::FindOptions::default(), None))?
        }
        _ => bail!("{USAGE}"),
    };
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
