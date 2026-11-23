//! Runs the game process and captures screenshots, closing the "edit then
//! look" loop for MCP.
//!
//! The game is a **child process** (`cargo run --release` or a direct exe).
//! Reader threads collect stdout/stderr into a ring buffer, and `game_logs`
//! returns the last N lines. Screenshots reuse the engine's
//! `PRISMA_SCREENSHOT*` environment variables: spawn the process, wait for
//! the PNG to land, then stop it.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use prism_editor_mcp::ToolDef;

/// Log buffer capacity: enough to diagnose problems without exhausting memory.
const LOG_CAPACITY: usize = 2000;

pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "game_run",
            description: "Launch the game from project root (default `cargo run --release`), logs go to buffer.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Override launch command, default `cargo run --release`" },
                    "env": { "type": "object", "description": "Extra env vars, values are strings" }
                }
            }),
        },
        ToolDef {
            name: "game_stop",
            description: "Terminate process started by game_run / screenshot.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "game_logs",
            description: "Get the last N lines of game log (stdout + stderr mixed).",
            input_schema: json!({
                "type": "object",
                "properties": { "lines": { "type": "integer", "default": 200 } }
            }),
        },
        ToolDef {
            name: "screenshot",
            description: "Launch game, capture at specified frame, then terminate; returns PNG path and resolution.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Output PNG (relative to project root or absolute path)" },
                    "size": { "type": "string", "description": "e.g. 1920x1080; if omitted use window resolution" },
                    "scale": { "type": "number", "description": "Window resolution multiplier (size takes priority)" },
                    "frame": { "type": "integer", "description": "Which frame to capture, default 120" },
                    "timeout_ms": { "type": "integer", "description": "Wait limit, default 60000" },
                    "command": { "type": "string", "description": "Override launch command" },
                    "env": { "type": "object" }
                },
                "required": ["path"]
            }),
        },
    ]
}

/// The process started by `game_run` plus its log buffer.
#[derive(Default)]
pub struct GameHost {
    child: Mutex<Option<Child>>,
    logs: Arc<Mutex<VecDeque<String>>>,
}

impl GameHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn call(&self, root: &Path, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "game_run" => self.run(root, args),
            "game_stop" => self.stop(),
            "game_logs" => self.logs(args),
            "screenshot" => self.screenshot(root, args),
            other => Err(format!("Unknown tool '{other}'")),
        }
    }

    fn spawn(
        &self,
        root: &Path,
        args: &Value,
        extra_env: &[(String, String)],
    ) -> Result<(), String> {
        let mut guard = self
            .child
            .lock()
            .map_err(|_| "Process lock poisoned".to_string())?;
        if guard.is_some() {
            return Err("A game is already running, call game_stop first".into());
        }
        let command = args
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("cargo run --release");
        let mut parts = command.split_whitespace();
        let program = parts
            .next()
            .ok_or_else(|| "Launch command is empty".to_string())?;

        let mut cmd = Command::new(program);
        cmd.args(parts)
            .current_dir(root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(env) = args.get("env").and_then(Value::as_object) {
            for (key, value) in env {
                if let Some(value) = value.as_str() {
                    cmd.env(key, value);
                }
            }
        }
        for (key, value) in extra_env {
            cmd.env(key, value);
        }

        let mut child = cmd
            .spawn()
            .map_err(|err| format!("Failed to launch '{command}': {err}"))?;
        self.pipe_logs(&mut child);
        *guard = Some(child);
        Ok(())
    }

    fn pipe_logs(&self, child: &mut Child) {
        let logs = Arc::clone(&self.logs);
        if let Some(stdout) = child.stdout.take() {
            let logs = Arc::clone(&logs);
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    push(&logs, line);
                }
            });
        }
        if let Some(stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    push(&logs, line);
                }
            });
        }
    }

    fn run(&self, root: &Path, args: &Value) -> Result<String, String> {
        self.spawn(root, args, &[])?;
        Ok("Started (get logs with game_logs)".into())
    }

    fn stop(&self) -> Result<String, String> {
        let mut guard = self
            .child
            .lock()
            .map_err(|_| "Process lock poisoned".to_string())?;
        match guard.take() {
            Some(mut child) => {
                let _ = child.kill();
                let _ = child.wait();
                Ok("Terminated".into())
            }
            None => Ok("No running process".into()),
        }
    }

    fn logs(&self, args: &Value) -> Result<String, String> {
        let lines = args.get("lines").and_then(Value::as_u64).unwrap_or(200) as usize;
        let logs = self
            .logs
            .lock()
            .map_err(|_| "Log lock poisoned".to_string())?;
        let skip = logs.len().saturating_sub(lines);
        Ok(logs
            .iter()
            .skip(skip)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n"))
    }

    fn screenshot(&self, root: &Path, args: &Value) -> Result<String, String> {
        let raw = args
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| "Missing 'path'".to_string())?;
        let target = {
            let path = PathBuf::from(raw);
            if path.is_absolute() {
                path
            } else {
                root.join(path)
            }
        };
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("Failed to create directory: {err}"))?;
        }
        let _ = std::fs::remove_file(&target);

        let frame = args.get("frame").and_then(Value::as_u64).unwrap_or(120);
        let timeout = Duration::from_millis(
            args.get("timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(60_000),
        );

        let mut env = vec![
            (
                "PRISMA_SCREENSHOT".to_string(),
                target.display().to_string(),
            ),
            ("PRISMA_SCREENSHOT_FRAME".to_string(), frame.to_string()),
        ];
        if let Some(size) = args.get("size").and_then(Value::as_str) {
            env.push(("PRISMA_SCREENSHOT_SIZE".to_string(), size.to_string()));
        } else if let Some(scale) = args.get("scale").and_then(Value::as_f64) {
            env.push(("PRISMA_SCREENSHOT_SCALE".to_string(), scale.to_string()));
        }

        self.spawn(root, args, &env)?;
        let started = Instant::now();
        let mut written = false;
        while started.elapsed() < timeout {
            if target.exists() {
                let first = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
                std::thread::sleep(Duration::from_millis(200));
                let second = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
                if first == second && second > 0 {
                    written = true;
                    break;
                }
            }
            if !self.is_running() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.stop();

        if !written {
            let tail = self.logs(&json!({ "lines": 30 })).unwrap_or_default();
            return Err(format!(
                "Screenshot wait timed out ({} ms)\nRecent logs:\n{tail}",
                timeout.as_millis()
            ));
        }
        let bytes = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
        Ok(serde_json::to_string_pretty(&json!({
            "path": target.display().to_string(),
            "bytes": bytes,
        }))
        .unwrap_or_default())
    }

    fn is_running(&self) -> bool {
        let Ok(mut guard) = self.child.lock() else {
            return false;
        };
        let Some(child) = guard.as_mut() else {
            return false;
        };
        matches!(child.try_wait(), Ok(None))
    }
}

fn push(logs: &Arc<Mutex<VecDeque<String>>>, line: String) {
    if let Ok(mut logs) = logs.lock() {
        if logs.len() == LOG_CAPACITY {
            logs.pop_front();
        }
        logs.push_back(line);
    }
}
