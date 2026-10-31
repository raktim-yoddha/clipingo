//! clip: command-line front end to the Clipingo core. Prints JSON; progress goes to stderr.

use anyhow::{Context, Result, bail};
use clipingo_core::models::{self, Progress};
use clipingo_core::pipeline::{self, RunOptions};
use clipingo_core::render::{self, Format, RenderOptions};
use clipingo_core::transcribe::{Language, TranscribeOptions};
use clipingo_core::{Word, scorer, source, system, tools};

const USAGE: &str = "usage:
  clip check                          tools, GPU, video encoders, speech models
  clip probe <file|url>               read a video file or YouTube link
  clip faces <file> <start> <end>     the speaker's face per sampled frame (framing input)
  clip transcribe <file> [lang] [refresh]
  clip run <file|url> [lang] [refresh]
                                      download (links), transcribe, find clips; prints the saved run
  clip render <run-id> <rank>... [horizontal] [keep-pauses] [no-captions] [title] [speed=1.5] [style=glow] [framing=smart|full|classic] [vision] [no-zoom] [swap] [position=auto|top|middle|bottom]
                                      render candidates (9:16 by default) + SRT + Premiere XML
  clip framing <run-id> <rank> [smart|full|classic] [swap] [no-zoom]
                                      how a clip is framed in 9:16: shots, layouts, zooms and crops (editor clock)
  clip eval <run-id> [captions.vtt|srt]
                                      evals: transcript vs reference captions (WER, recall, sync), clip
                                      boundaries, exports (size, face in frame, pauses, caption timing)
  clip score <words.json>             find clip candidates in a transcript ([{\"w\",\"s\",\"e\"}, …])
  clip models                         list local speech models
  clip models install <id|all>        english, hinglish, hindi, runtime (whisper.cpp), yt-dlp, deno
  lang: auto (default) | en | hi | hinglish";

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

fn render_options(flags: &[&str]) -> Result<(Vec<usize>, RenderOptions)> {
    let mut opts = RenderOptions::default();
    let mut ranks = Vec::new();
    for &f in flags {
        match f {
            "horizontal" => opts.format = Format::Horizontal,
            "vertical" => opts.format = Format::Vertical,
            "keep-pauses" => opts.cut_dead_air = false,
            "no-captions" => opts.captions = false,
            "title" => opts.title = true,
            "vision" => opts.vision_check = true,
            "no-zoom" => opts.smart_zoom = false,
            "swap" => opts.swap_stack = true,
            f if f.starts_with("position=") => {
                opts.caption_position =
                    serde_json::from_value(serde_json::json!(&f[9..])).with_context(|| format!("bad position: {f}"))?
            }
            f if f.starts_with("speed=") => opts.speed = f[6..].parse().with_context(|| format!("bad speed: {f}"))?,
            f if f.starts_with("style=") => opts.caption_style = f[6..].to_string(),
            f if f.starts_with("framing=") => {
                opts.framing =
                    serde_json::from_value(serde_json::json!(&f[8..])).with_context(|| format!("bad framing: {f}"))?
            }
            n => ranks.push(n.parse().with_context(|| format!("not a rank or option: {n}\n{USAGE}"))?),
        }
    }
    if ranks.is_empty() {
        bail!("give at least one candidate rank\n{USAGE}");
    }
    Ok((ranks, opts))
}

fn print_progress(p: Progress) {
    match p.total {
        Some(100) => eprint!("\r{}: {}%        ", p.stage, p.done),
        Some(t) if t > 0 => {
            eprint!("\r{}: {:.0}% ({} / {} MB)   ", p.stage, p.done as f64 * 100.0 / t as f64, p.done >> 20, t >> 20)
        }
        _ => eprint!("\r{}…                    ", p.stage),
    }
}

async fn install(id: &str) -> Result<serde_json::Value> {
    let progress = |p: Progress| print_progress(p);
    let mut done = serde_json::Map::new();
    let ids: Vec<&str> = if id == "all" {
        ["ffmpeg", "runtime", "yt-dlp", "deno"].into_iter().chain(models::MODELS.iter().map(|m| m.id)).collect()
    } else {
        vec![id]
    };
    for id in ids {
        let path = tools::install_component(id, &progress).await?;
        eprintln!();
        done.insert(id.to_string(), serde_json::json!(path));
    }
    Ok(done.into())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let progress = |p: Progress| print_progress(p);
    let out = match args.as_slice() {
        ["check"] => serde_json::to_value(system::check().await)?,
        ["probe", input] => serde_json::to_value(source::probe(input).await?)?,
        ["faces", input, start, end] => {
            let info = source::probe(input).await?;
            let src = (info.width.unwrap_or(1920) as u32, info.height.unwrap_or(1080) as u32);
            let model = clipingo_core::framing::model(&progress).await?;
            let faces =
                clipingo_core::framing::detect(&model, std::path::Path::new(input), start.parse()?, end.parse()?, src)?;
            serde_json::json!(faces.iter().map(|f| [f.t, f.x, f.y, f.h]).collect::<Vec<_>>())
        }
        ["score", path] => {
            let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {path}"))?;
            let words: Vec<Word> = serde_json::from_str(&text).context("transcript must be [{\"w\",\"s\",\"e\"}]")?;
            serde_json::to_value(scorer::find(&words, &scorer::FindOptions::default(), None))?
        }
        ["transcribe", input, flags @ ..] => {
            serde_json::to_value(pipeline::transcribe_file(input, &transcribe_options(flags)?, &progress).await?)?
        }
        ["run", input, flags @ ..] => {
            let opts = RunOptions { transcribe: transcribe_options(flags)?, find: None };
            serde_json::to_value(pipeline::create_run(input, &opts, &progress).await?)?
        }
        ["render", run_id, flags @ ..] => {
            let (ranks, opts) = render_options(flags)?;
            serde_json::to_value(render::render_clips(run_id, &ranks, &opts, &progress).await?)?
        }
        ["framing", run_id, rank, rest @ ..] => {
            let mut opts = RenderOptions::default();
            for &f in rest {
                match f {
                    "swap" => opts.swap_stack = true,
                    "no-zoom" => opts.smart_zoom = false,
                    other => {
                        opts.framing = serde_json::from_value(serde_json::json!(other))
                            .with_context(|| format!("unknown option {other}\n{USAGE}"))?
                    }
                }
            }
            serde_json::to_value(render::preview_plan(run_id, rank.parse()?, &opts, &progress).await?)?
        }
        ["eval", run_id, rest @ ..] => {
            let reference = rest.first().map(std::path::Path::new);
            let report = clipingo_core::eval::run(run_id, reference, &progress).await?;
            eprintln!();
            for f in &report.failures {
                eprintln!("FAIL {f}");
            }
            if report.failures.is_empty() {
                eprintln!("all checks pass");
            }
            serde_json::to_value(report)?
        }
        ["models"] => serde_json::json!({ "whisper_cli": models::whisper_cli(), "models": models::status() }),
        ["models", "install", id] => install(id).await?,
        _ => bail!("{USAGE}"),
    };
    eprintln!();
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
