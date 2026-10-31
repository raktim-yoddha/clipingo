//! clipd: the Clipingo daemon. JSON-RPC 2.0, one JSON object per line, over a
//! named pipe (Windows) or Unix socket (macOS/Linux). Every UI talks to this.
//! Long calls stream `{"method":"progress","params":{"request":<id>,…}}` notifications before their reply.

use anyhow::Result;
use clipingo_core::models::{self, Progress};
use clipingo_core::transcribe::TranscribeOptions;
use clipingo_core::{Word, pipeline, render, scorer, source, system, tools};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Deserialize)]
struct Request {
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

struct RpcError {
    code: i64,
    message: String,
}

impl From<anyhow::Error> for RpcError {
    fn from(e: anyhow::Error) -> Self {
        Self { code: -32000, message: format!("{e:#}") }
    }
}

fn invalid_params(e: serde_json::Error) -> RpcError {
    RpcError { code: -32602, message: e.to_string() }
}

/// Progress callback that streams `progress` notifications tagged with the request id.
fn notifier(out: &UnboundedSender<Value>, id: &Value) -> impl Fn(Progress) + Send + Sync + use<> {
    let (tx, request) = (out.clone(), id.clone());
    move |pr: Progress| {
        let _ = tx.send(json!({ "jsonrpc": "2.0", "method": "progress",
            "params": { "request": request, "stage": pr.stage, "done": pr.done, "total": pr.total } }));
    }
}

async fn dispatch(method: &str, params: Value, id: &Value, out: &UnboundedSender<Value>) -> Result<Value, RpcError> {
    match method {
        "version" => Ok(json!({ "version": env!("CARGO_PKG_VERSION") })),
        "system.check" => Ok(json!(system::check().await)),
        "source.probe" => {
            #[derive(Deserialize)]
            struct P {
                input: String,
            }
            let p: P = serde_json::from_value(params).map_err(invalid_params)?;
            Ok(json!(source::probe(&p.input).await?))
        }
        "scorer.find" => {
            #[derive(Deserialize)]
            struct P {
                words: Vec<Word>,
                #[serde(default)]
                options: Option<scorer::FindOptions>,
            }
            let p: P = serde_json::from_value(params).map_err(invalid_params)?;
            Ok(json!(scorer::find(&p.words, &p.options.unwrap_or_default(), None)))
        }
        "transcript.run" => {
            #[derive(Deserialize)]
            struct P {
                input: String,
                #[serde(default)]
                options: TranscribeOptions,
            }
            let p: P = serde_json::from_value(params).map_err(invalid_params)?;
            Ok(json!(pipeline::transcribe_file(&p.input, &p.options, &notifier(out, id)).await?))
        }
        "run.create" => {
            #[derive(Deserialize)]
            struct P {
                input: String,
                #[serde(default)]
                options: pipeline::RunOptions,
            }
            let p: P = serde_json::from_value(params).map_err(invalid_params)?;
            Ok(json!(pipeline::create_run(&p.input, &p.options, &notifier(out, id)).await?))
        }
        "run.get" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
            }
            let p: P = serde_json::from_value(params).map_err(invalid_params)?;
            Ok(json!(pipeline::load_run(&p.id)?.0))
        }
        "run.render" => {
            /// `ranks`: 1-based candidate ranks from `run.create`.
            #[derive(Deserialize)]
            struct P {
                id: String,
                ranks: Vec<usize>,
                #[serde(default)]
                options: render::RenderOptions,
            }
            let p: P = serde_json::from_value(params).map_err(invalid_params)?;
            Ok(json!(render::render_clips(&p.id, &p.ranks, &p.options, &notifier(out, id)).await?))
        }
        "run.list" => Ok(json!(pipeline::list_runs())),
        "run.open" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
            }
            let p: P = serde_json::from_value(params).map_err(invalid_params)?;
            let run = pipeline::load_run(&p.id)?.0;
            let options = render::last_options(&run);
            Ok(
                json!({ "clips": pipeline::exported(&run), "clips_dir": run.clips_dir(), "options": options, "run": run }),
            )
        }
        "run.words" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
                start: f64,
                end: f64,
            }
            let p: P = serde_json::from_value(params).map_err(invalid_params)?;
            Ok(json!(pipeline::clip_words(&p.id, p.start, p.end)?))
        }
        "run.edit" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
                #[serde(flatten)]
                edit: pipeline::ClipEdit,
            }
            let p: P = serde_json::from_value(params).map_err(invalid_params)?;
            Ok(json!(pipeline::edit_clip(&p.id, &p.edit)?))
        }
        "run.add_clip" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
                start: f64,
                end: f64,
                title: String,
            }
            let p: P = serde_json::from_value(params).map_err(invalid_params)?;
            Ok(json!(pipeline::add_clip(&p.id, p.start, p.end, &p.title)?))
        }
        "run.remove_clip" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
                rank: usize,
            }
            let p: P = serde_json::from_value(params).map_err(invalid_params)?;
            pipeline::remove_clip(&p.id, p.rank)?;
            Ok(json!({}))
        }
        "run.delete" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
            }
            let p: P = serde_json::from_value(params).map_err(invalid_params)?;
            pipeline::delete_run(&p.id)?;
            Ok(json!({}))
        }
        "run.retext" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
                start: f64,
                end: f64,
                text: String,
            }
            let p: P = serde_json::from_value(params).map_err(invalid_params)?;
            Ok(json!(pipeline::retext(&p.id, p.start, p.end, &p.text)?))
        }
        "run.framing" => {
            /// How a clip is framed in 9:16 on the editor's clock: shots, layouts and crops (source pixels).
            #[derive(Deserialize)]
            struct P {
                id: String,
                rank: usize,
                #[serde(default)]
                options: render::RenderOptions,
            }
            let p: P = serde_json::from_value(params).map_err(invalid_params)?;
            Ok(json!(render::preview_plan(&p.id, p.rank, &p.options, &notifier(out, id)).await?))
        }
        "run.set_layout" => {
            /// Your layout for the shot `shot` (source seconds) around source time `at`; no layout: back to the rules.
            #[derive(Deserialize)]
            struct P {
                id: String,
                rank: usize,
                at: f64,
                shot: (f64, f64),
                #[serde(default)]
                layout: Option<clipingo_core::framing::Layout>,
            }
            let p: P = serde_json::from_value(params).map_err(invalid_params)?;
            Ok(json!(pipeline::set_layout(&p.id, p.rank, p.at, p.shot, p.layout)?))
        }
        "run.thumbs" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
                times: Vec<f64>,
                #[serde(default = "thumb_height")]
                height: u32,
            }
            fn thumb_height() -> u32 {
                72
            }
            let p: P = serde_json::from_value(params).map_err(invalid_params)?;
            Ok(json!(pipeline::thumbs(&p.id, &p.times, p.height).await?))
        }
        "captions.presets" => Ok(json!(clipingo_core::captions::PRESETS)),
        "models.list" => Ok(json!({ "whisper_cli": models::whisper_cli(), "models": models::status() })),
        "models.install" => {
            /// A model id from `models.list`, "runtime" (whisper.cpp), "yt-dlp" or "deno".
            #[derive(Deserialize)]
            struct P {
                id: String,
            }
            let p: P = serde_json::from_value(params).map_err(invalid_params)?;
            Ok(json!({ "path": tools::install_component(&p.id, &notifier(out, id)).await? }))
        }
        _ => Err(RpcError { code: -32601, message: format!("unknown method {method}") }),
    }
}

async fn respond(line: &str, out: &UnboundedSender<Value>) -> Option<Value> {
    let req: Request = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => {
            return Some(
                json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": e.to_string() } }),
            );
        }
    };
    let id = req.id.clone().unwrap_or(Value::Null);
    let result = dispatch(&req.method, req.params, &id, out).await;
    req.id?; // notifications get no reply
    Some(match result {
        Ok(v) => json!({ "jsonrpc": "2.0", "id": id, "result": v }),
        Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": e.code, "message": e.message } }),
    })
}

async fn serve<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(stream: S) -> Result<()> {
    let (read, mut write) = tokio::io::split(stream);
    // One writer keeps progress notifications and replies in order on the wire.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let mut bytes = serde_json::to_vec(&msg)?;
            bytes.push(b'\n');
            write.write_all(&bytes).await?;
        }
        anyhow::Ok(())
    });
    let mut lines = BufReader::new(read).lines();
    let mut pending: Option<String> = None;
    loop {
        let line = match pending.take() {
            Some(l) => l,
            None => match lines.next_line().await? {
                Some(l) => l,
                None => break,
            },
        };
        if line.trim().is_empty() {
            continue;
        }
        // While a call runs, watch the connection: a client that hangs up (Cancel in Jobs) drops the call, which
        // kills its ffmpeg / whisper / yt-dlp children. A line sent meanwhile waits its turn.
        let work = respond(&line, &tx);
        tokio::pin!(work);
        let reply = tokio::select! {
            r = &mut work => r,
            next = lines.next_line() => match next? {
                None => break,
                Some(l) => {
                    pending = Some(l);
                    work.await
                }
            },
        };
        if let Some(reply) = reply {
            let _ = tx.send(reply);
        }
    }
    drop(tx);
    writer.await??;
    Ok(())
}

fn spawn_client<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(stream: S) {
    tokio::spawn(async move {
        if let Err(e) = serve(stream).await {
            eprintln!("client error: {e:#}");
        }
    });
}

#[cfg(windows)]
async fn listen(endpoint: &str) -> Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;
    // first_pipe_instance fails if another process already owns the name (single instance, no squatting).
    let mut server = ServerOptions::new().first_pipe_instance(true).create(endpoint)?;
    loop {
        server.connect().await?;
        let client = std::mem::replace(&mut server, ServerOptions::new().create(endpoint)?);
        spawn_client(client);
    }
}

#[cfg(unix)]
async fn listen(endpoint: &str) -> Result<()> {
    let _ = std::fs::remove_file(endpoint);
    let listener = tokio::net::UnixListener::bind(endpoint)?;
    loop {
        let (stream, _) = listener.accept().await?;
        spawn_client(stream);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let endpoint = clipingo_core::endpoint();
    eprintln!("clipd {} listening on {endpoint}", env!("CARGO_PKG_VERSION"));
    listen(&endpoint).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn answers_requests_and_errors_in_jsonrpc_shape() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let ok = respond(r#"{"jsonrpc":"2.0","id":1,"method":"version"}"#, &tx).await.unwrap();
        assert_eq!(ok["result"]["version"], env!("CARGO_PKG_VERSION"));
        let unknown = respond(r#"{"jsonrpc":"2.0","id":2,"method":"nope"}"#, &tx).await.unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
        let bad = respond(r#"{"jsonrpc":"2.0","id":3,"method":"source.probe","params":{}}"#, &tx).await.unwrap();
        assert_eq!(bad["error"]["code"], -32602);
        assert!(respond(r#"{"jsonrpc":"2.0","method":"version"}"#, &tx).await.is_none());
        assert_eq!(respond("not json", &tx).await.unwrap()["error"]["code"], -32700);
        let unknown_model =
            respond(r#"{"jsonrpc":"2.0","id":4,"method":"models.install","params":{"id":"klingon"}}"#, &tx)
                .await
                .unwrap();
        assert!(unknown_model["error"]["message"].as_str().unwrap().contains("unknown model"));
    }
}
