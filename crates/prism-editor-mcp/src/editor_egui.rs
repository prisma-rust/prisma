//! `editor_*` tools that drive a **running** egui editor (`editor/`).
//!
//! The editor opens a local control channel on `127.0.0.1:<port>` and writes
//! the port to `<project root>/target/editor-egui/control.port`. This module only
//! does TCP + JSON; it does not link engine / render and does not open Vulkan.
//!
//! | Tool | Purpose |
//! |---|---|
//! | `editor_launch` | Starts `prism-editor-egui` (debug build) and waits for the control channel |
//! | `editor_status` | Frame count, current scene, selection, Play state |
//! | `editor_capture` | Captures window/viewport PNG (`target` = `window`/`scene`/`game`) |
//! | `editor_quit` | Asks the editor to exit |
//! | `editor_ui` | UI node-tree snapshot, click/set by node (like DOM operations in browser MCP) |
//! | `editor_play` | Play / Stop / Pause (Play compiles and loads the project module) |
//! | `editor_logs` | Reads the Console buffer (project-module logs and panic backtraces) |
//! | `editor_input` | Injects keyboard/mouse into Play (one event per frame) for automation |

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use prism_editor_mcp::ToolDef;

/// The process started by `editor_launch`.
#[derive(Default)]
pub struct EditorHost {
    child: Mutex<Option<Child>>,
}

impl EditorHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn call(&self, root: &Path, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "editor_launch" => self.launch(root, args),
            "editor_status" => status(root),
            "editor_capture" => capture(root, args),
            "editor_quit" => quit(self),
            "editor_ui" => ui(root, args),
            "editor_play" => play(root, args),
            "editor_logs" => logs(root, args),
            "editor_input" => input(root, args),
            other => Err(format!("Unknown editor tool '{other}'")),
        }
    }

    fn launch(&self, root: &Path, args: &Value) -> Result<String, String> {
        let mut guard = self
            .child
            .lock()
            .map_err(|_| "editor child lock poisoned")?;
        if let Some(child) = guard.as_mut() {
            match child.try_wait() {
                Ok(None) => return Err("Editor already running (call editor_quit first)".into()),
                _ => *guard = None,
            }
        }
        let port_file = port_file(root);
        let _ = std::fs::remove_file(&port_file);

        let command = args
            .get("command")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| default_command(root));
        let mut parts = command.split_whitespace();
        let program = parts.next().ok_or("command is empty")?;
        let mut child = Command::new(program)
            .args(parts)
            .current_dir(root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| format!("Failed to launch '{program}': {err}"))?;

        let wait = args
            .get("wait_ms")
            .and_then(Value::as_u64)
            .unwrap_or(30_000);
        let deadline = Instant::now() + Duration::from_millis(wait);
        while Instant::now() < deadline {
            if port_file.exists() && control_port(root).is_ok() {
                let ready = request(root, json!({ "cmd": "status" }))
                    .ok()
                    .and_then(|status| status.get("frames").and_then(Value::as_u64))
                    .map(|frames| frames >= 3)
                    .unwrap_or(false);
                if ready {
                    let pid = child.id();
                    *guard = Some(child);
                    return Ok(json!({
                        "ok": true,
                        "pid": pid,
                        "control": port_file.display().to_string(),
                        "command": command,
                    })
                    .to_string());
                }
            }
            if let Ok(Some(code)) = child.try_wait() {
                return Err(format!(
                    "Editor exited (code {code}), control channel did not come up"
                ));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Err(format!("Timed out waiting for control channel ({wait} ms)"))
    }
}

pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "editor_launch",
                        description: "Launch egui editor (default target/debug/prism-editor-egui), return pid after local control channel ready.",
            input_schema: json!({
                "type": "object",
                "properties": {
                                        "command": { "type": "string", "description": "Override launch command (relative to project root)" },
                    "wait_ms": { "type": "integer", "default": 30000 }
                }
            }),
        },
        ToolDef {
            name: "editor_status",
                        description: "Status of running egui editor: frame count, current scene, selected entity, Play / paused, whether unsaved changes.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "editor_capture",
                        description: "Take screenshot from running editor: target=window captures whole window (UI + viewport), target=scene|game captures that viewport engine offscreen image. Returns absolute PNG path.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "enum": ["window", "scene", "game"],
                                                "description": "What to capture: window / Scene view / Game view, default window"
                    },
                                        "path": { "type": "string", "description": "Output PNG (relative to project root or absolute); if omitted writes to target/editor-egui/captures/" },
                                        "size": { "type": "string", "description": "e.g. 1920x1080 (nearest-neighbor scaling done in editor)" }
                }
            }),
        },
        ToolDef {
            name: "editor_ui",
                        description: "Editor UI node tree and pointer: action=snapshot returns whole UI tree (panels / docs / hierarchy entities / assets / inspector fields / viewport objects / modal dialogs / command list); action=click clicks by node id (command:Save / panel:hierarchy / doc:0 / entity:3 / asset:assets/x.prefab.json / view:scene:entity:2 / dialog:close:discard); action=move reports target under a viewport point; action=wheel scrolls in viewport (Scene zoom / Game wheel); action=drag drags in viewport (gizmo / box select / orbit / gameplay pointer); action=set assigns inspector field or dialog input.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["snapshot", "click", "move", "wheel", "drag", "set"], "default": "snapshot" },
                                        "node": { "type": "string", "description": "Node id for click / set / move / wheel (see snapshot return)" },
                                        "offset": { "description": "Pixel offset relative to node anchor for move / wheel [dx,dy]" },
                                        "tab": { "type": "string", "description": "When no node use tab + x/y (sceneView / gameView)" },
                                        "x": { "type": "number", "description": "Viewport local X (when no node)" },
                                        "y": { "type": "number", "description": "Viewport local Y (when no node)" },
                                        "delta": { "type": "number", "description": "Wheel delta (default 120)" },
                                        "from": { "description": "Drag start: {node,offset} or {x,y}" },
                                        "to": { "description": "Drag end: {node,offset} or {x,y}" },
                                        "steps": { "type": "integer", "description": "Drag sampling steps (default 8)" },
                                        "mode": { "type": "string", "enum": ["orbit"], "description": "Force orbit camera during drag" },
                                        "ctrl": { "type": "boolean", "description": "Hold Ctrl during drag (gizmo snap invert)" },
                                        "value": { "description": "Value for set (JSON: number / string / bool / array)" }
                }
            }),
        },
        ToolDef {
            name: "editor_play",
                        description: "Control editor Play: action=play starts (compiles and loads project module in background, project code runs in Game view), stop stops, pause pauses / resumes.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["play", "stop", "pause"],
                        "default": "play"
                    }
                }
            }),
        },
        ToolDef {
            name: "editor_logs",
                        description: "Read editor Console buffer (engine logs + project module project: logs + panic backtraces). since is previous next, for incremental pull.",
            input_schema: json!({
                "type": "object",
                "properties": {
                                        "since": { "type": "integer", "default": 0, "description": "Only lines with seq >= since (use next from last return)" },
                    "limit": { "type": "integer", "default": 200 }
                }
            }),
        },
        ToolDef {
            name: "editor_input",
                        description: "Inject input into Game View currently in Play: key=space/w/a/s/d/... or mouse=left/right/middle, state=press|release; click=true equals left press+release. Events queued per frame, press / release not swallowed by same frame.",
            input_schema: json!({
                "type": "object",
                "properties": {
                                        "key": { "type": "string", "description": "Key name: w/a/s/d/q/e/space/shift/ctrl/alt/escape/tab/enter/up/down/left/right/0-9" },
                    "mouse": { "type": "string", "enum": ["left", "right", "middle"] },
                    "state": { "type": "string", "enum": ["press", "release"], "default": "press" },
                                        "click": { "type": "boolean", "description": "Left click once (press + release)" }
                }
            }),
        },
        ToolDef {
            name: "editor_quit",
                        description: "Make editor exit through normal path (will write state.json).",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
    ]
}

fn play(root: &Path, args: &Value) -> Result<String, String> {
    let action = args.get("action").and_then(Value::as_str).unwrap_or("play");
    let cmd = match action {
        "play" => "play",
        "stop" => "stop",
        "pause" => "pause",
        other => {
            return Err(format!(
                "action can only be play / stop / pause, got '{other}'"
            ))
        }
    };
    let reply = request(root, json!({ "cmd": cmd }))?;
    Ok(reply.to_string())
}

fn logs(root: &Path, args: &Value) -> Result<String, String> {
    let since = args.get("since").and_then(Value::as_u64).unwrap_or(0);
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(200);
    let reply = request(
        root,
        json!({ "cmd": "logs", "since": since, "limit": limit }),
    )?;
    Ok(reply.to_string())
}

fn input(root: &Path, args: &Value) -> Result<String, String> {
    let reply = request(
        root,
        json!({
            "cmd": "input",
            "key": args.get("key").cloned().unwrap_or(Value::Null),
            "mouse": args.get("mouse").cloned().unwrap_or(Value::Null),
            "state": args.get("state").cloned().unwrap_or(Value::Null),
            "click": args.get("click").cloned().unwrap_or(Value::Null),
        }),
    )?;
    Ok(reply.to_string())
}

/// Pointer arguments for `editor_ui`: forward only the keys the caller actually
/// provided, so `null` is not sent to the control channel.
fn pointer_args(args: &Value) -> Value {
    let mut body = serde_json::Map::new();
    for key in [
        "node", "offset", "tab", "x", "y", "delta", "from", "to", "steps", "mode", "ctrl",
    ] {
        if let Some(value) = args.get(key) {
            if !value.is_null() {
                body.insert(key.to_string(), value.clone());
            }
        }
    }
    Value::Object(body)
}

fn ui(root: &Path, args: &Value) -> Result<String, String> {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("snapshot");
    let (cmd, mut body) = match action {
        "snapshot" => ("ui.snapshot", json!({})),
        "click" => {
            let node = args
                .get("node")
                .and_then(Value::as_str)
                .ok_or("click needs node")?;
            ("ui.click", json!({ "node": node }))
        }
        "move" => ("ui.move", pointer_args(args)),
        "wheel" => ("ui.wheel", pointer_args(args)),
        "drag" => {
            if args.get("from").is_none() || args.get("to").is_none() {
                return Err("drag needs from / to".to_string());
            }
            ("ui.drag", pointer_args(args))
        }
        "set" => {
            let node = args
                .get("node")
                .and_then(Value::as_str)
                .ok_or("set needs node")?;
            (
                "ui.set",
                json!({ "node": node, "value": args.get("value").cloned().unwrap_or(Value::Null) }),
            )
        }
        other => {
            return Err(format!(
                "Unknown action '{other}' (snapshot / click / move / wheel / drag / set)"
            ))
        }
    };
    body["cmd"] = json!(cmd);
    let response = request(root, body)?;
    Ok(response.to_string())
}

fn status(root: &Path) -> Result<String, String> {
    let response = request(root, json!({ "cmd": "status" }))?;
    Ok(response.to_string())
}

fn capture(root: &Path, args: &Value) -> Result<String, String> {
    let target = args
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or("window");
    if !matches!(target, "window" | "scene" | "game") {
        return Err(format!("Unknown target '{target}' (window / scene / game)"));
    }
    let path = match args.get("path").and_then(Value::as_str) {
        Some(value) => {
            let value = PathBuf::from(value);
            if value.is_absolute() {
                value
            } else {
                root.join(value)
            }
        }
        None => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis())
                .unwrap_or_default();
            root.join("target/editor-egui/captures")
                .join(format!("{target}-{stamp}.png"))
        }
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create directory: {err}"))?;
    }
    let mut request_body =
        json!({ "cmd": "capture", "target": target, "path": path.to_string_lossy() });
    if let Some(size) = args.get("size").and_then(Value::as_str) {
        request_body["size"] = json!(size);
    }
    let response = request(root, request_body)?;
    Ok(response.to_string())
}

fn quit(host: &EditorHost) -> Result<String, String> {
    let root = prism_editor_mcp::resolve_root().map_err(|err| err.to_string())?;
    let response = request(&root, json!({ "cmd": "quit" }))?;
    let mut guard = host
        .child
        .lock()
        .map_err(|_| "editor child lock poisoned")?;
    if let Some(mut child) = guard.take() {
        let _ = child.wait();
    }
    Ok(response.to_string())
}

/// File the editor writes its port to (`PRISMA_EDITOR_CONTROL` can override it
/// with a port number).
fn port_file(root: &Path) -> PathBuf {
    match std::env::var_os("PRISMA_EDITOR_CONTROL") {
        Some(value) => PathBuf::from(value),
        None => root.join("target/editor-egui/control.port"),
    }
}

fn control_port(root: &Path) -> Result<u16, String> {
    if let Ok(value) = std::env::var("PRISMA_EDITOR_CONTROL") {
        if let Ok(port) = value.trim().parse::<u16>() {
            return Ok(port);
        }
    }
    let file = port_file(root);
    let text = std::fs::read_to_string(&file).map_err(|err| {
        format!(
                        "Cannot read control port {}: {err} (editor not open, used --no-control, or call editor_launch first)",
            file.display()
        )
    })?;
    text.trim()
        .parse::<u16>()
        .map_err(|err| format!("Control port file invalid: {err}"))
}

fn request(root: &Path, body: Value) -> Result<Value, String> {
    let port = control_port(root)?;
    let stream = TcpStream::connect(("127.0.0.1", port)).map_err(|err| {
        format!("Cannot connect to editor control channel 127.0.0.1:{port}: {err}")
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let mut writer = stream
        .try_clone()
        .map_err(|err| format!("Failed to copy socket: {err}"))?;
    writeln!(writer, "{body}").map_err(|err| format!("Failed to write control channel: {err}"))?;
    writer.flush().ok();
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|err| format!("Failed to read control channel: {err}"))?;
    let reply: Value = serde_json::from_str(line.trim())
        .map_err(|err| format!("Control channel reply not JSON: {err}"))?;
    if reply.get("ok").and_then(Value::as_bool) == Some(false) {
        return Err(reply.to_string());
    }
    Ok(reply)
}

fn default_command(root: &Path) -> String {
    let editor_root = find_editor_root(root).unwrap_or_else(|| root.to_path_buf());
    let debug = editor_root.join("editor/target/debug/prism-editor-egui");
    let release = editor_root.join("editor/target/release/prism-editor-egui");
    for candidate in [release, debug] {
        let with_exe = if cfg!(windows) {
            candidate.with_extension("exe")
        } else {
            candidate.clone()
        };
        if with_exe.exists() {
            return with_exe.display().to_string();
        }
    }
    format!(
        "cargo run -p prism-editor-egui --manifest-path {}",
        editor_root.join("editor/Cargo.toml").display()
    )
}

/// Walk up from `root` to the repository root that owns the egui editor.
///
/// `editor/Cargo.toml` alone is ambiguous: a user project may legitimately live at
/// `projects/editor/` and carry its own workspace manifest. The crate directory is
/// what identifies this repository's editor, so anchor on that instead.
fn find_editor_root(root: &Path) -> Option<PathBuf> {
    let mut current = Some(root);
    while let Some(dir) = current {
        if dir
            .join("editor/crates/prism-editor-egui/Cargo.toml")
            .is_file()
        {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "test discovers repository root from cargo test cwd"
    )]
    fn finds_editor_from_nested_user_project() {
        let mut repo = std::env::current_dir().unwrap();
        while !repo
            .join("editor/crates/prism-editor-egui/Cargo.toml")
            .is_file()
        {
            repo = repo.parent().expect("repository root").to_path_buf();
        }
        let fishing = repo.join("projects/demos/fishing");
        assert_eq!(find_editor_root(&fishing), Some(repo));
    }
}
