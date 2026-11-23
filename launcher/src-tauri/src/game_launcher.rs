//! Desktop-side game binary spawning: locate + prevent duplicate launch.
//!
//! Old impl used `Command::new("template")` relying on `PATH`, result depended on what was installed:
//! could hit a same-named program elsewhere, or launch a second game while one already running.
//! Here we locate binary by deterministic layout and use child process handle to block duplicate launch.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

/// Env var directly specifying game binary; when set must point to a real existing file.
pub const GAME_BIN_ENV: &str = "PRISMA_GAME_BIN";

/// Game crate location relative to repo root.
const GAME_CRATE_DIR: &str = "projects/template";

/// Game binary filename (with platform suffix).
fn binary_name() -> String {
    format!("template{}", std::env::consts::EXE_SUFFIX)
}

/// Candidate binary paths, in priority order:
///
/// 1. Packaged layout — same directory as launcher executable;
/// 2. Dev layout — `projects/template/target/<profile>/template[.exe]` under repo root,
///    repo root found by walking up from `exe_dir` looking for `projects/template/Cargo.toml`.
pub fn candidates(exe_dir: &Path, profile: &str) -> Vec<PathBuf> {
    let mut out = vec![exe_dir.join(binary_name())];
    if let Some(root) = repo_root_from(exe_dir) {
        out.push(
            root.join(GAME_CRATE_DIR)
                .join("target")
                .join(profile)
                .join(binary_name()),
        );
    }
    out
}

pub(crate) fn repo_root_from(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(GAME_CRATE_DIR).join("Cargo.toml").is_file())
        .map(Path::to_path_buf)
}

/// Resolve game binary to launch.
///
/// When `env_override` present it takes precedence — explicit user choice, pointing to missing file is an error,
/// no further fallback (otherwise silently runs another `template`). Only without override walks
/// [`candidates`] order.
pub fn resolve(
    exe_dir: &Path,
    profile: &str,
    env_override: Option<&OsStr>,
) -> Result<PathBuf, String> {
    if let Some(raw) = env_override {
        let path = PathBuf::from(raw);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "{GAME_BIN_ENV} points to a nonexistent file: {}",
            path.display()
        ));
    }

    let tried = candidates(exe_dir, profile);
    tried
        .iter()
        .find(|path| path.is_file())
        .cloned()
        .ok_or_else(|| {
            let list = tried
                .iter()
                .map(|path| format!("  {}", path.display()))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "Cannot find game binary `{}`, tried:\n{list}\n\
                                  During development run `cargo build` in projects/template;\
                                  or use {GAME_BIN_ENV} to specify full path.",
                binary_name()
            )
        })
}

/// Already launched game child process; only used to block duplicate launch.
#[derive(Default)]
pub struct RunningGame(Mutex<Option<Child>>);

impl RunningGame {
    /// Launch game. Returns error when a live child process exists; already-exited child reaped.
    pub fn launch(&self, config: Option<String>) -> Result<(), String> {
        let mut slot = self
            .0
            .lock()
            .map_err(|_| "Game process state poisoned".to_string())?;
        if let Some(child) = slot.as_mut() {
            match child.try_wait() {
                // Still alive: do not launch second one.
                Ok(None) => return Err("Game is already running".to_string()),
                // Already exited or query failed: clear handle, allow relaunch.
                _ => *slot = None,
            }
        }

        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
            .unwrap_or_default();
        let profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        let binary = resolve(&exe_dir, profile, std::env::var_os(GAME_BIN_ENV).as_deref())?;

        let mut cmd = Command::new(&binary);
        if let Some(config) = config {
            cmd.env("PRISMA_LAUNCH_CONFIG", config);
        }
        // Do not swallow game output: only connect stdin to null, stdout/stderr inherit launcher console.
        cmd.stdin(Stdio::null());
        let child = cmd
            .spawn()
            .map_err(|err| format!("Failed to launch {}: {err}", binary.display()))?;
        *slot = Some(child);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each test uses independent temp dir, not cleaning after test fine (under system temp dir).
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("prisma-launcher-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("Create temp dir");
        dir
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("Create parent dir");
        }
        std::fs::write(path, b"fake").expect("Write file");
    }

    #[test]
    fn packaged_layout_uses_the_sibling_binary() {
        let root = scratch("packaged");
        let exe_dir = root.join("dist");
        touch(&exe_dir.join(binary_name()));

        let resolved = resolve(&exe_dir, "debug", None).expect("Should hit same-dir binary");
        assert_eq!(resolved, exe_dir.join(binary_name()));
    }

    #[test]
    fn dev_layout_finds_the_game_crate_target() {
        let root = scratch("dev");
        // Repo: <root>/repo/{Cargo.toml, projects/template/Cargo.toml}
        let repo = root.join("repo");
        touch(&repo.join("Cargo.toml"));
        touch(&repo.join(GAME_CRATE_DIR).join("Cargo.toml"));
        let game = repo
            .join(GAME_CRATE_DIR)
            .join("target")
            .join("debug")
            .join(binary_name());
        touch(&game);

        let exe_dir = repo
            .join("launcher")
            .join("src-tauri")
            .join("target")
            .join("debug");
        std::fs::create_dir_all(&exe_dir).expect("Create exe dir");

        let resolved =
            resolve(&exe_dir, "debug", None).expect("Should hit projects/template/target");
        assert_eq!(resolved, game);
    }

    #[test]
    fn env_override_wins_and_must_exist() {
        let root = scratch("env");
        let exe_dir = root.join("dist");
        touch(&exe_dir.join(binary_name()));
        let custom = root.join("custom").join("my-game");
        touch(&custom);

        let resolved = resolve(&exe_dir, "debug", Some(custom.as_os_str()))
            .expect("Override value should win");
        assert_eq!(resolved, custom);

        let missing = root.join("custom").join("nope");
        let err = resolve(&exe_dir, "debug", Some(missing.as_os_str()))
            .expect_err("Missing override should error not fall back");
        assert!(err.contains(GAME_BIN_ENV), "{err}");
    }

    #[test]
    fn missing_binary_lists_every_candidate() {
        let root = scratch("missing");
        let exe_dir = root.join("dist");
        std::fs::create_dir_all(&exe_dir).expect("Create exe dir");

        let err = resolve(&exe_dir, "release", None).expect_err("Missing binary should error");
        assert!(err.contains(&exe_dir.join(binary_name()).display().to_string()));
        assert!(err.contains("projects/template"), "{err}");
    }
}
