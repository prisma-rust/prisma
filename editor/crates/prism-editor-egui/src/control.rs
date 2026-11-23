//! Local control channel: MCP (`crates/prism-editor-mcp`) drives **running** editor.
//!
//! Why not env var / subprocess: window screenshot must happen inside editor process (`PrintWindow`
//! needs HWND, engine offscreen image needs `GraphRenderer`), egui is single-threaded event loop,
//! so external can only push requests to UI thread via one channel.
//!
//! Shape:
//!
//! - At startup `TcpListener` on `127.0.0.1:0` (kernel-assigned port), write port to
//!   `<state dir>/control.port`, delete on exit; listens only on loopback, no remote connections.
//! - One JSON per line, Q&A:
//!
//! ```text
//! → {"cmd":"ping"}
//! ← {"ok":true,"pid":1234,"frames":42,"scene":"default.scene.json"}
//! → {"cmd":"capture","target":"window","path":"shot.png","size":"1920x1080"}
//! ← {"ok":true,"path":"...","width":1440,"height":900}
//! ```
//!
//! Supported `cmd`: `ping` / `status` / `capture` / `quit` / `quit.confirm` /
//! `quit.cancel` (last two go through "unsaved changes" triage, not forced exit).
//!
//! Requests pushed from socket thread into `mpsc`, UI thread `drain()` once at frame start and
//! completes screenshot within same frame (window screenshot takes previous frame presented result), result sent back via reply channel.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use serde_json::{json, Value};

/// `--no-control`: process-level channel close. UI toggle in `Preferences::control_channel`.
static CLI_DISABLED: AtomicBool = AtomicBool::new(false);

pub fn set_cli_disabled(disabled: bool) {
    CLI_DISABLED.store(disabled, Ordering::Relaxed);
}

/// Whether command line requests closing control channel (`--no-control`).
pub fn cli_disabled() -> bool {
    CLI_DISABLED.load(Ordering::Relaxed)
}

/// Failure category: caller decides "retry with new params / wait / do not retry" based on this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Wrong params: retry with new params.
    BadRequest,
    /// Editor not ready yet (still loading, view not laid out, Play not running): wait and retry.
    NotReady,
    /// Explicitly rejected (platform unsupported, channel closed): do not retry.
    Denied,
    /// Editor internal error: retry usually useless, read logs.
    Internal,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BadRequest => "bad_request",
            Self::NotReady => "not_ready",
            Self::Denied => "denied",
            Self::Internal => "internal",
        }
    }
}

/// Failure reply: `kind` is new machine-readable category, `error` kept for old callers
/// (same as `message` content).
pub fn failed(kind: Kind, message: impl Into<String>) -> Value {
    let message = message.into();
    json!({
        "ok": false,
        "kind": kind.as_str(),
        "message": message.clone(),
        "error": message,
    })
}

/// Failure with category. `From<String>` / `From<&str>` default `bad_request`, so
/// existing `?` chain param validation does not need per-item change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlError {
    pub kind: Kind,
    pub message: String,
}

impl ControlError {
    pub fn new(kind: Kind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn not_ready(message: impl Into<String>) -> Self {
        Self::new(Kind::NotReady, message)
    }

    pub fn into_json(self) -> Value {
        failed(self.kind, self.message)
    }
}

impl From<String> for ControlError {
    fn from(message: String) -> Self {
        Self::new(Kind::BadRequest, message)
    }
}

impl From<&str> for ControlError {
    fn from(message: &str) -> Self {
        Self::new(Kind::BadRequest, message)
    }
}

/// One pending request + reply channel.
pub struct ControlRequest {
    pub command: String,
    pub args: Value,
    pub reply: Sender<Value>,
}

/// Running control channel. `Drop` stops listening and deletes port file.
pub struct ControlServer {
    port_file: PathBuf,
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    requests: Receiver<ControlRequest>,
}

impl ControlServer {
    /// When `enabled` false, no listener, no port file, returns `None`.
    pub fn start_if(enabled: bool, state_dir: &Path) -> anyhow::Result<Option<Self>> {
        if enabled {
            Self::start(state_dir).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Open channel under `state_dir` and write `control.port`.
    pub fn start(state_dir: &Path) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|error| anyhow::anyhow!("control: bind loopback failed: {error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| anyhow::anyhow!("control: local_addr failed: {error}"))?;
        let port = address.port();

        let (tx, rx) = mpsc::channel::<ControlRequest>();
        let tx = Arc::new(tx);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("editor-control".to_owned())
            .spawn(move || {
                for stream in listener.incoming() {
                    // `Drop` sets flag then connects to this port, to wake up blocked accept here.
                    if thread_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let tx = Arc::clone(&tx);
                    std::thread::spawn(move || serve(stream, &tx));
                }
            })
            .map_err(|error| anyhow::anyhow!("control: spawn accept thread failed: {error}"))?;

        std::fs::create_dir_all(state_dir).ok();
        let port_file = state_dir.join("control.port");
        std::fs::write(&port_file, format!("{port}\n"))
            .map_err(|error| anyhow::anyhow!("control: write {}: {error}", port_file.display()))?;
        log::info!("control: listening on 127.0.0.1:{port}");

        Ok(Self {
            port_file,
            address,
            stop,
            requests: rx,
        })
    }

    /// UI thread calls once per frame: take this frame's requests to process (non-blocking).
    pub fn drain(&self) -> Vec<ControlRequest> {
        self.requests.try_iter().collect()
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wake up accept thread so listener truly closes (only deleting port file leaves port bound,
        // bound, reopening old connections queue into unprocessed queue).
        let _ = TcpStream::connect(self.address);
        std::fs::remove_file(&self.port_file).ok();
    }
}

/// Single connection: read one JSON line -> queue -> reply one JSON line.
fn serve(stream: TcpStream, tx: &Sender<ControlRequest>) {
    let peer_ok = stream
        .peer_addr()
        .map(|addr| addr.ip().is_loopback())
        .unwrap_or(false);
    if !peer_ok {
        return;
    }
    let reader_stream = match stream.try_clone() {
        Ok(clone) => clone,
        Err(_) => return,
    };
    let mut writer = stream;
    let mut reader = BufReader::new(reader_stream);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(error) => {
                let _ = writeln!(
                    writer,
                    "{}",
                    failed(Kind::BadRequest, format!("bad json: {error}"))
                );
                continue;
            }
        };
        let command = value
            .get("cmd")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        if command.is_empty() {
            let _ = writeln!(writer, "{}", failed(Kind::BadRequest, "missing cmd"));
            continue;
        }
        let (reply_tx, reply_rx) = mpsc::channel();
        let request = ControlRequest {
            command,
            args: value,
            reply: reply_tx,
        };
        if tx.send(request).is_err() {
            return;
        }
        // UI thread replies within 5s max; timeout means editor stuck / not running UI loop.
        let response = reply_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or_else(|_| failed(Kind::NotReady, "editor did not answer in 5s"));
        if writeln!(writer, "{response}").is_err() {
            return;
        }
        let _ = writer.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};

    #[test]
    fn server_publishes_loopback_port_and_removes_it_on_drop() {
        let dir = std::env::temp_dir().join(format!(
            "prisma-editor-control-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let server = ControlServer::start(&dir).unwrap();
        let port_file = dir.join("control.port");
        let port: u16 = std::fs::read_to_string(&port_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        writeln!(stream, "{{\"cmd\":\"PiNg\"}}").unwrap();

        let request = loop {
            match server.requests.try_recv() {
                Ok(request) => break request,
                Err(mpsc::TryRecvError::Empty) => std::thread::yield_now(),
                Err(mpsc::TryRecvError::Disconnected) => panic!("control queue disconnected"),
            }
        };
        assert_eq!(request.command, "ping");
        request.reply.send(json!({ "ok": true })).unwrap();
        let mut response = String::new();
        BufReader::new(stream).read_line(&mut response).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&response).unwrap()["ok"],
            true
        );

        drop(server);
        assert!(!port_file.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_and_missing_commands_return_errors_without_queueing() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        let client = TcpStream::connect(address).unwrap();
        let (server_stream, _) = listener.accept().unwrap();
        std::thread::spawn(move || serve(server_stream, &tx));
        let mut writer = client;
        writeln!(writer, "{{bad").unwrap();
        writeln!(writer, "{{\"value\":1}}").unwrap();
        let mut reader = BufReader::new(writer.try_clone().unwrap());
        let mut first = String::new();
        let mut second = String::new();
        reader.read_line(&mut first).unwrap();
        reader.read_line(&mut second).unwrap();
        let first: Value = serde_json::from_str(&first).unwrap();
        let second: Value = serde_json::from_str(&second).unwrap();
        assert_eq!(first["kind"], "bad_request");
        assert_eq!(first["message"], first["error"]);
        assert!(first["error"].as_str().unwrap().starts_with("bad json:"));
        assert_eq!(second["kind"], "bad_request");
        assert_eq!(second["error"], "missing cmd");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn failure_replies_carry_kind_and_keep_error_for_old_callers() {
        let value = failed(Kind::NotReady, "wait");
        assert_eq!(value["ok"], false);
        assert_eq!(value["kind"], "not_ready");
        assert_eq!(value["message"], "wait");
        assert_eq!(value["error"], "wait", "old callers still read `error`");
        assert_eq!(
            ControlError::from("bad arg".to_string()).kind,
            Kind::BadRequest
        );
        assert_eq!(ControlError::from("bad arg").kind, Kind::BadRequest);
        assert_eq!(
            ControlError::not_ready("later").into_json()["kind"],
            "not_ready"
        );
        assert_eq!(Kind::Denied.as_str(), "denied");
        assert_eq!(Kind::Internal.as_str(), "internal");
    }

    #[test]
    fn disabled_channel_opens_no_listener_and_writes_no_port_file() {
        let dir = std::env::temp_dir().join(format!(
            "prisma-editor-control-off-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(ControlServer::start_if(false, &dir).unwrap().is_none());
        assert!(!dir.join("control.port").exists());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn dropping_the_server_closes_the_listener() {
        let dir = std::env::temp_dir().join(format!(
            "prisma-editor-control-drop-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let server = ControlServer::start(&dir).unwrap();
        let address = server.address;
        drop(server);
        // accept thread exits only after receiving wake connection; give it a short time.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if TcpStream::connect(address).is_err() {
                std::fs::remove_dir_all(dir).ok();
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("control listener still accepts connections after Drop");
    }
}
