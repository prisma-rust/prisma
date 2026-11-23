//! Build / Cook: invoke `prism-build-pipeline` as **subprocess** (same path as Web editor).
//!
//! Editor does not link cooker/build pipeline (AGENTS.md hard rule), so here only spawn
//! `cargo run -p prism-build-pipeline -- …`, streaming stdout/stderr line by line to Console.
//! Compile / package may take tens of seconds, all on background thread, UI thread only `try_recv`.
//!
//! Output is **streaming**: stdout / stderr each has a read thread, each `cargo` line goes to Console,
//! no longer wait for process exit to appear all at once. `BuildJob::cancel()` kills subprocess and lets it finish normally.
//! One task may contain multi-step commands (player build first cooks scene), each step takes same streaming path.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::diagnostics::{parse_cargo_line, Diagnostic, MAX_DIAGNOSTICS};
use prism_editor_core::build::{copy_artifacts, plan_build, BuildPlan};
use prism_editor_core::settings::ProjectSettings;

/// One build task.
pub struct BuildJob {
    rx: Receiver<String>,
    label: String,
    running: bool,
    cancel: Arc<AtomicBool>,
    /// `cargo build` JSON diagnostics; Console still gets one summary line, Problems panel gets structured data.
    diagnostics: Arc<Mutex<Vec<Diagnostic>>>,
    /// Whether any step failed (non-0 exit / missing product / spawn failure).
    failed: Arc<AtomicBool>,
}

pub enum BuildKind {
    /// `prism-build-pipeline build-scenes` (all enabled scenes -> `scenes.pak`).
    ScenePack,
    /// `prism-build-pipeline cook <scene>` (single `.scene.json` -> `.rscn`).
    Cook(PathBuf),
    /// Build user project (`File → Build Player`): select profile per `[build]` settings,
    /// then cook scene package, place exe + `assets/` per `output_dir`.
    Player,
    /// Same as [`BuildKind::Player`], run `output_dir/<exe>` directly after build (`Build And Run`).
    PlayerRun,
    /// `cargo clean` player project (engine Clean Build).
    ///
    /// Player is **independent workspace** (`projects/<p>/target`), cleaning it is the meaning of Clean Build;
    /// cleaning repo root `target/` also deletes engine and all crate products, another matter.
    Clean,
    /// `cargo clean` repo root workspace (engine + all crates compile cache).
    CleanEngine,
    /// `prism-build-pipeline bake-gi`: offline GI probe baking (Offline Tools, #477).
    BakeGi {
        output: String,
        rays: u32,
        bounces: u32,
    },
    /// `prism-build-pipeline heightmap`: procedural terrain + erosion (Offline Tools, #477).
    Heightmap {
        output: String,
        width: u32,
        height: u32,
        seed: u64,
        iterations: u32,
        sea_level: f32,
    },
    /// `prism-build-pipeline build-scenes --output … --compression …`: custom packaging (#477).
    ScenePackCustom { output: String, compression: u32 },
}

pub(crate) fn game_manifest_path(root: &Path) -> PathBuf {
    let nested = root.join("projects/template/Cargo.toml");
    if nested.is_file() {
        nested
    } else {
        root.join("Cargo.toml")
    }
}

fn pipeline_manifest_path(root: &Path) -> Option<PathBuf> {
    // Only one implementation (data layer), player build and ScenePack both take from here.
    prism_editor_core::build::pipeline_manifest_path(root)
}

/// Read `[build]` settings and generate player build plan; use defaults (debug + `build/`) if settings file bad.
fn player_plan(root: &Path) -> BuildPlan {
    let settings = ProjectSettings::read(root).unwrap_or_default();
    plan_build(root, &settings)
}

impl BuildKind {
    pub fn label(&self, i18n: &prism_editor_core::I18n) -> String {
        match self {
            BuildKind::ScenePack => i18n.t1("desktop.buildKind.scenePack"),
            BuildKind::Cook(path) => i18n.t(
                "desktop.buildKind.cook",
                &[("path", &path.display().to_string())],
            ),
            BuildKind::Player => i18n.t1("desktop.buildKind.player"),
            BuildKind::PlayerRun => i18n.t1("desktop.buildKind.playerRun"),
            BuildKind::Clean => i18n.t1("desktop.buildKind.clean"),
            BuildKind::CleanEngine => i18n.t1("desktop.buildKind.cleanEngine"),
            BuildKind::BakeGi {
                output,
                rays,
                bounces,
            } => i18n.t(
                "desktop.buildKind.bakeGi",
                &[
                    ("rays", &rays.to_string()),
                    ("bounces", &bounces.to_string()),
                    ("output", output),
                ],
            ),
            BuildKind::Heightmap {
                output,
                width,
                height,
                ..
            } => i18n.t(
                "desktop.buildKind.heightmap",
                &[
                    ("width", &width.to_string()),
                    ("height", &height.to_string()),
                    ("output", output),
                ],
            ),
            BuildKind::ScenePackCustom { output, .. } => {
                i18n.t("desktop.buildKind.scenePackCustom", &[("output", output)])
            }
        }
    }

    /// Player build (needs cook + place products + optional run) rather than plain cargo step.
    pub fn is_player_build(&self) -> bool {
        matches!(self, BuildKind::Player | BuildKind::PlayerRun)
    }

    /// Whether this task launches product directly after build.
    pub fn runs_after_build(&self) -> bool {
        matches!(self, BuildKind::PlayerRun)
    }

    /// cargo commands this task executes sequentially. Player build first refreshes `scenes.pak`,
    /// otherwise runs scene from previous cook.
    fn commands(&self, root: &Path) -> Result<Vec<Vec<String>>, String> {
        let pipeline_with = |features: &[&str], extra: &[&str]| -> Result<Vec<String>, String> {
            let manifest = pipeline_manifest_path(root)
                .ok_or_else(|| "cannot find prism-build-pipeline workspace".to_string())?;
            let mut args = vec![
                "run".to_string(),
                "--manifest-path".to_string(),
                manifest.display().to_string(),
                "-p".to_string(),
                "prism-build-pipeline".to_string(),
            ];
            if !features.is_empty() {
                args.push("--features".to_string());
                args.push(features.join(","));
            }
            args.push("--".to_string());
            args.extend(extra.iter().map(|arg| arg.to_string()));
            Ok(args)
        };
        let pipeline = |extra: &[&str]| pipeline_with(&[], extra);
        // Player build reuses data layer `BuildPlan` (profile / output_dir / product layout all there),
        // here only translates its steps to cargo params.
        let player_commands = || -> Result<Vec<Vec<String>>, String> {
            Ok(player_plan(root)
                .steps
                .into_iter()
                .map(|step| step.args)
                .collect())
        };
        Ok(match self {
            BuildKind::ScenePack => vec![pipeline(&["build-scenes"])?],
            BuildKind::Cook(path) => vec![pipeline(&["cook", &path.display().to_string()])?],
            BuildKind::Player | BuildKind::PlayerRun => player_commands()?,
            BuildKind::Clean => vec![vec![
                "clean".to_string(),
                "--manifest-path".to_string(),
                game_manifest_path(root).display().to_string(),
            ]],
            BuildKind::CleanEngine => vec![vec![
                "clean".to_string(),
                "--manifest-path".to_string(),
                root.join("Cargo.toml").display().to_string(),
            ]],
            BuildKind::BakeGi {
                output,
                rays,
                bounces,
            } => {
                let rays = rays.to_string();
                let bounces = bounces.to_string();
                vec![pipeline_with(
                    &["gi-bake"],
                    &["bake-gi", output, "--rays", &rays, "--bounces", &bounces],
                )?]
            }
            BuildKind::Heightmap {
                output,
                width,
                height,
                seed,
                iterations,
                sea_level,
            } => {
                let (width, height) = (width.to_string(), height.to_string());
                let seed = format!("{seed}");
                let iterations = iterations.to_string();
                let sea_level = format!("{sea_level}");
                vec![pipeline(&[
                    "heightmap",
                    "--output",
                    output,
                    "--width",
                    &width,
                    "--height",
                    &height,
                    "--seed",
                    &seed,
                    "--iterations",
                    &iterations,
                    "--sea-level",
                    &sea_level,
                ])?]
            }
            BuildKind::ScenePackCustom {
                output,
                compression,
            } => {
                let compression = compression.to_string();
                vec![pipeline(&[
                    "build-scenes",
                    "--output",
                    output,
                    "--compression",
                    &compression,
                ])?]
            }
        })
    }
}

/// Forward a pipeline line by line to channel; thread naturally ends when write end closes.
fn forward_lines(
    stream: impl Read + Send + 'static,
    tx: Sender<String>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else {
                break;
            };
            if tx.send(line).is_err() {
                break;
            }
        }
    })
}

/// stdout of `cargo --message-format=json`: parse into diagnostics,
/// while leaving one human-readable summary line in Console (JSON itself no one wants to see).
fn forward_cargo_json(
    stream: impl Read + Send + 'static,
    tx: Sender<String>,
    root: PathBuf,
    diagnostics: Arc<Mutex<Vec<Diagnostic>>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else {
                break;
            };
            let Some(diagnostic) = parse_cargo_line(&line, &root) else {
                // cargo occasionally mixes non-JSON lines; JSON events already covered by diagnostics / summary.
                if !line.trim_start().starts_with('{') && tx.send(line).is_err() {
                    break;
                }
                continue;
            };
            let text = diagnostic.console_line();
            let stored = {
                let mut diagnostics = diagnostics.lock().unwrap();
                if diagnostics.len() < MAX_DIAGNOSTICS {
                    diagnostics.push(diagnostic);
                    true
                } else {
                    false
                }
            };
            if stored && tx.send(text).is_err() {
                break;
            }
        }
    })
}

impl BuildJob {
    /// Start task. `root` is project root (contains `assets/` and root `Cargo.toml`).
    pub fn spawn(kind: BuildKind, root: &Path, label: String) -> Self {
        let (tx, rx) = channel();
        let root = root.to_path_buf();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let diagnostics = Arc::new(Mutex::new(Vec::new()));
        let worker_diagnostics = Arc::clone(&diagnostics);
        let failed = Arc::new(AtomicBool::new(false));
        let worker_failed = Arc::clone(&failed);
        std::thread::spawn(move || {
            let commands = match kind.commands(&root) {
                Ok(commands) => commands,
                Err(error) => {
                    let _ = tx.send(error);
                    worker_failed.store(true, Ordering::Relaxed);
                    return;
                }
            };
            for args in commands {
                if worker_cancel.load(Ordering::Relaxed) {
                    let _ = tx.send("── cancelled ──".to_string());
                    return;
                }
                let _ = tx.send(format!("$ cargo {}", args.join(" ")));
                let mut command = std::process::Command::new("cargo");
                command
                    .args(&args)
                    .current_dir(&root)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                let mut child: Child = match command.spawn() {
                    Ok(child) => child,
                    Err(error) => {
                        let _ = tx.send(format!("failed to spawn cargo: {error}"));
                        worker_failed.store(true, Ordering::Relaxed);
                        return;
                    }
                };

                // whether this step is structured diagnostic mode (only `cargo build` has this param).
                let json = args.iter().any(|arg| arg == "json");
                if json {
                    worker_diagnostics.lock().unwrap().clear();
                }
                // diagnostic span relative to workspace root (parent dir of `--manifest-path`).
                let span_root = args
                    .iter()
                    .position(|arg| arg == "--manifest-path")
                    .and_then(|index| args.get(index + 1))
                    .map(PathBuf::from)
                    .and_then(|manifest| manifest.parent().map(Path::to_path_buf))
                    .unwrap_or_else(|| root.clone());

                // one read thread each for stdout / stderr, guarantee both outputs do not starve each other.
                let mut readers = Vec::new();
                if let Some(stdout) = child.stdout.take() {
                    readers.push(if json {
                        forward_cargo_json(
                            stdout,
                            tx.clone(),
                            span_root,
                            Arc::clone(&worker_diagnostics),
                        )
                    } else {
                        forward_lines(stdout, tx.clone())
                    });
                }
                if let Some(stderr) = child.stderr.take() {
                    readers.push(forward_lines(stderr, tx.clone()));
                }

                let mut cancelled = false;
                loop {
                    if worker_cancel.load(Ordering::Relaxed) {
                        // kill only sends signal; below wait() reaps, read thread also waits.
                        let _ = child.kill();
                        cancelled = true;
                        break;
                    }
                    match child.try_wait() {
                        Ok(Some(_)) => break,
                        Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                        Err(error) => {
                            let _ = tx.send(format!("cannot wait for cargo: {error}"));
                            worker_failed.store(true, Ordering::Relaxed);
                            cancelled = true;
                            break;
                        }
                    }
                }

                let status = child.wait().ok();
                for reader in readers {
                    let _ = reader.join();
                }
                if cancelled {
                    let _ = tx.send("── cancelled ──".to_string());
                    return;
                }
                let code = status.and_then(|status| status.code()).unwrap_or(-1);
                let _ = tx.send(format!("── exit {code} ──"));
                // if previous step fails, do not run subsequent steps (e.g. cook failure should not package old scene).
                if !status.map(|status| status.success()).unwrap_or(false) {
                    let _ = tx.send("build stopped: step failed".to_string());
                    worker_failed.store(true, Ordering::Relaxed);
                    return;
                }
            }

            // Player build: place exe / `assets/` into `[build].output_dir`, run product directly when needed.
            if kind.is_player_build() {
                let plan = player_plan(&root);
                let _ = tx.send(format!(
                    "staging artifacts into {}",
                    plan.output_dir.display()
                ));
                match copy_artifacts(&plan.copies) {
                    Ok(copied) => {
                        for line in copied {
                            let _ = tx.send(format!("copied {line}"));
                        }
                    }
                    Err(error) => {
                        let _ = tx.send(format!("cannot stage artifacts: {error}"));
                        worker_failed.store(true, Ordering::Relaxed);
                        return;
                    }
                }
                let missing: Vec<String> = plan
                    .required
                    .iter()
                    .filter(|path| !path.is_file())
                    .map(|path| path.display().to_string())
                    .collect();
                if !missing.is_empty() {
                    let _ = tx.send(format!(
                        "build stopped: missing artifact {}",
                        missing.join(", ")
                    ));
                    worker_failed.store(true, Ordering::Relaxed);
                    return;
                }
                if kind.runs_after_build() {
                    let exe = plan
                        .required
                        .first()
                        .cloned()
                        .unwrap_or_else(|| plan.output_dir.clone());
                    match std::process::Command::new(&exe)
                        .current_dir(&plan.output_dir)
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                    {
                        Ok(child) => {
                            let _ =
                                tx.send(format!("launched {} (pid {})", exe.display(), child.id()));
                        }
                        Err(error) => {
                            let _ = tx.send(format!("cannot launch {}: {error}", exe.display()));
                            worker_failed.store(true, Ordering::Relaxed);
                        }
                    }
                }
            }
        });
        Self {
            rx,
            label,
            running: true,
            cancel,
            diagnostics,
            failed,
        }
    }

    /// Take compiler diagnostics parsed by background thread (for Problems panel).
    pub fn take_diagnostics(&self) -> Vec<Diagnostic> {
        std::mem::take(&mut *self.diagnostics.lock().unwrap())
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn running(&self) -> bool {
        self.running
    }

    /// Request interrupt: background thread will `kill()` subprocess and send `── cancelled ──`.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Whether this task was cancelled by user (true after `cancel()`).
    pub fn was_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Whether any step really failed (non-0 exit / missing product / spawn failure).
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    /// Take new output (`None` = task ended).
    pub fn poll(&mut self) -> (Vec<String>, bool) {
        let mut lines = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(line) => lines.push(line),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.running = false;
                    break;
                }
            }
        }
        (lines, self.running)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_root() -> PathBuf {
        let mut repo = std::env::current_dir().unwrap();
        while !repo
            .join("crates/prism-build-pipeline/Cargo.toml")
            .is_file()
        {
            repo = repo.parent().expect("repository root").to_path_buf();
        }
        repo
    }

    fn mentions(args: &[String], needle: &str) -> bool {
        args.iter().any(|arg| arg.contains(needle))
    }

    /// #477: Offline Tools panel only assembles args into `prism-build-pipeline` command line.
    #[test]
    fn offline_tool_commands_target_the_pipeline() {
        let root = repo_root();

        let bake = BuildKind::BakeGi {
            output: "assets/gi/out.bin".to_string(),
            rays: 128,
            bounces: 4,
        }
        .commands(&root)
        .unwrap();
        assert_eq!(bake.len(), 1);
        assert!(mentions(&bake[0], "bake-gi"));
        assert!(mentions(&bake[0], "assets/gi/out.bin"));
        assert!(bake[0]
            .windows(2)
            .any(|pair| pair[0] == "--features" && pair[1] == "gi-bake"));
        assert!(bake[0]
            .windows(2)
            .any(|pair| pair[0] == "--rays" && pair[1] == "128"));
        assert!(bake[0]
            .windows(2)
            .any(|pair| pair[0] == "--bounces" && pair[1] == "4"));

        let heightmap = BuildKind::Heightmap {
            output: "assets/heightmaps/t.raw".to_string(),
            width: 256,
            height: 128,
            seed: 7,
            iterations: 9,
            sea_level: 0.4,
        }
        .commands(&root)
        .unwrap();
        assert_eq!(heightmap.len(), 1);
        let args = &heightmap[0];
        assert!(mentions(args, "heightmap"));
        for (flag, value) in [
            ("--output", "assets/heightmaps/t.raw"),
            ("--width", "256"),
            ("--height", "128"),
            ("--seed", "7"),
            ("--iterations", "9"),
            ("--sea-level", "0.4"),
        ] {
            assert!(
                args.windows(2)
                    .any(|pair| pair[0] == flag && pair[1] == value),
                "{flag} {value} missing from {args:?}"
            );
        }

        let pack = BuildKind::ScenePackCustom {
            output: "assets/custom.pak".to_string(),
            compression: 7,
        }
        .commands(&root)
        .unwrap();
        assert!(mentions(&pack[0], "build-scenes"));
        assert!(pack[0]
            .windows(2)
            .any(|pair| pair[0] == "--output" && pair[1] == "assets/custom.pak"));
        assert!(pack[0]
            .windows(2)
            .any(|pair| pair[0] == "--compression" && pair[1] == "7"));
    }

    /// Build a temp project root with `assets/settings.ini` (build plan reads profile / output_dir from it).
    fn project_root(name: &str, build_section: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("prism-egui-build-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(
            root.join("assets/settings.ini"),
            format!(
                "[build]
{build_section}
"
            ),
        )
        .unwrap();
        // Real project cooked once has these products; product list expands by existence; test also needs this premise.
        for name in ["scenes.pak", "scenes.pak.meta.json", "scenes.json"] {
            std::fs::write(root.join("assets").join(name), b"{}").unwrap();
        }
        root
    }

    #[test]
    fn player_builds_refresh_the_scene_pack_first() {
        let root = project_root("player-steps", "profile = debug");
        for kind in [BuildKind::Player, BuildKind::PlayerRun] {
            let commands = kind.commands(&root).unwrap();
            assert_eq!(commands.len(), 2, "player builds need two steps");
            assert_eq!(commands[0][0], "build", "first step builds the player");
            assert!(
                commands[0].iter().any(|arg| arg.contains("Cargo.toml")),
                "the build step needs a manifest"
            );
            assert!(
                mentions(&commands[1], "build-scenes"),
                "the second step refreshes the scene pack"
            );
        }
    }

    /// `[build].profile` decides `--release`: previously Build Player always built debug.
    #[test]
    fn the_release_profile_survives_the_trip_from_settings() {
        let debug_root = project_root("profile-debug", "profile = debug");
        let debug = BuildKind::Player.commands(&debug_root).unwrap();
        assert!(!mentions(&debug[0], "--release"));

        let release_root = project_root("profile-release", "profile = release");
        let release = BuildKind::Player.commands(&release_root).unwrap();
        assert!(
            mentions(&release[0], "--release"),
            "release profile must reach cargo: {:?}",
            release[0]
        );
    }

    #[test]
    fn player_build_targets_project_root_manifest() {
        let repo = repo_root();
        let root = project_root("manifest", "profile = debug");
        // User project without `projects/template` falls back to its own manifest.
        assert_eq!(
            BuildKind::Player.commands(&root).unwrap()[0],
            vec![
                "build".to_string(),
                "--manifest-path".to_string(),
                root.join("Cargo.toml").display().to_string(),
                "--message-format".to_string(),
                "json".to_string(),
            ]
        );
        assert_eq!(
            game_manifest_path(&repo),
            repo.join("projects/template/Cargo.toml")
        );
    }

    /// Player build wants JSON diagnostics (Problems panel); package/cook steps cannot add.
    #[test]
    fn only_the_player_build_asks_for_json_diagnostics() {
        let root = project_root("diagnostics", "profile = debug");
        for kind in [BuildKind::Player, BuildKind::PlayerRun] {
            let commands = kind.commands(&root).unwrap();
            assert!(mentions(&commands[0], "json"));
        }
        // Package step uses real repo root (temp root has no pipeline workspace).
        let pack = BuildKind::ScenePack.commands(&repo_root()).unwrap();
        assert!(!mentions(&pack[0], "json"));
    }

    /// `Build And Run` runs **placed product**, not `cargo run` debug build.
    #[test]
    fn build_and_run_launches_the_staged_binary() {
        assert!(BuildKind::PlayerRun.is_player_build());
        assert!(BuildKind::PlayerRun.runs_after_build());
        assert!(BuildKind::Player.is_player_build());
        assert!(!BuildKind::Player.runs_after_build());
        let root = project_root(
            "staged",
            "profile = release
output_dir = build/win",
        );
        let plan = player_plan(&root);
        assert_eq!(plan.output_dir, root.join("build/win"));
        assert_eq!(
            plan.required,
            vec![plan.output_dir.join(prism_editor_core::build::exe_name())]
        );
        assert!(plan
            .copies
            .iter()
            .any(|copy| copy.to.ends_with("assets/scenes.pak")));
        let local = if cfg!(windows) {
            "template.exe"
        } else {
            "template"
        };
        assert_eq!(plan.required[0].file_name().unwrap().to_str(), Some(local));
    }

    #[test]
    fn scene_pack_cooks_in_one_step() {
        let root = repo_root();
        let commands = BuildKind::ScenePack.commands(&root).unwrap();
        assert_eq!(commands.len(), 1);
        assert!(mentions(&commands[0], "build-scenes"));
    }

    /// Clean only cleans player workspace, CleanEngine cleans repo root.
    #[test]
    fn clean_targets_differ_between_player_and_engine() {
        let repo = repo_root();
        assert_eq!(
            BuildKind::Clean.commands(&repo).unwrap(),
            vec![vec![
                "clean".to_string(),
                "--manifest-path".to_string(),
                repo.join("projects/template/Cargo.toml")
                    .display()
                    .to_string()
            ]]
        );
        assert_eq!(
            BuildKind::CleanEngine.commands(&repo).unwrap(),
            vec![vec![
                "clean".to_string(),
                "--manifest-path".to_string(),
                repo.join("Cargo.toml").display().to_string()
            ]]
        );
    }

    #[test]
    fn pipeline_manifest_search_walks_to_repository_root() {
        let repo = repo_root();
        let root = repo.join("projects/fishing");
        assert_eq!(pipeline_manifest_path(&root), Some(repo.join("Cargo.toml")));
    }
}
