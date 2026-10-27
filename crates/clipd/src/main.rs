//! clipd: the Clipingo daemon. JSON-RPC 2.0, one JSON object per line, over a
//! named pipe (Windows) or Unix socket (macOS/Linux). Every UI talks to this.

use anyhow::Result;
use clipingo_core::transcribe::TranscribeOptions;
use clipingo_core::{Word, pipeline, scorer, source, system};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

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

async fn dispatch(method: &str, params: Value) -> Result<Value, RpcError> {
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
            Ok(json!(pipeline::transcribe_file(&p.input, &p.options).await?))
        }
        "clips.find" => {
            #[derive(Deserialize)]
            struct P {
                input: String,
                #[serde(default)]
                transcribe: TranscribeOptions,
                #[serde(default)]
                find: Option<scorer::FindOptions>,
            }
            let p: P = serde_json::from_value(params).map_err(invalid_params)?;
            Ok(json!(pipeline::find_clips(&p.input, &p.transcribe, &p.find.unwrap_or_default()).await?))
        }
        _ => Err(RpcError { code: -32601, message: format!("unknown method {method}") }),
    }
}

async fn respond(line: &str) -> Option<Value> {
    let req: Request = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => return Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": e.to_string() } })),
    };
    let result = dispatch(&req.method, req.params).await;
    let id = req.id?; // notifications get no reply
    Some(match result {
        Ok(v) => json!({ "jsonrpc": "2.0", "id": id, "result": v }),
        Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": e.code, "message": e.message } }),
    })
}

async fn serve<S: AsyncRead + AsyncWrite + Unpin>(stream: S) -> Result<()> {
    let (read, mut write) = tokio::io::split(stream);
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = respond(&line).await {
            let mut out = serde_json::to_vec(&reply)?;
            out.push(b'\n');
            write.write_all(&out).await?;
        }
    }
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
        let ok = respond(r#"{"jsonrpc":"2.0","id":1,"method":"version"}"#).await.unwrap();
        assert_eq!(ok["result"]["version"], env!("CARGO_PKG_VERSION"));
        let unknown = respond(r#"{"jsonrpc":"2.0","id":2,"method":"nope"}"#).await.unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
        let bad = respond(r#"{"jsonrpc":"2.0","id":3,"method":"source.probe","params":{}}"#).await.unwrap();
        assert_eq!(bad["error"]["code"], -32602);
        assert!(respond(r#"{"jsonrpc":"2.0","method":"version"}"#).await.is_none());
        assert_eq!(respond("not json").await.unwrap()["error"]["code"], -32700);
    }
}
