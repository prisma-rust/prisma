//! Build plan: translates "build project" into executable subprocess steps and artifact copies.
//!
//! Same semantics as the Web editor's `editor_build_project`: build the game crate first, then
//! cook `assets/scenes.json`, finally copy exe / `scenes.pak` / `settings.ini` into
//! `[build].output_dir`. Only plain data is produced here; actual spawning is done by the UI layer.

use crate::settings::ProjectSettings;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildStep {
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
    /// Working directory (usually the project root; `cargo run -p ...` needs the root workspace).
    pub cwd: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactCopy {
    pub from: PathBuf,
    pub to: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    pub steps: Vec<BuildStep>,
    pub copies: Vec<ArtifactCopy>,
    pub output_dir: PathBuf,
    /// Missing it counts as a build failure (the executable); missing `scenes.pak` etc. in
    /// `copies` are just skipped.
    pub required: Vec<PathBuf>,
}

/// Game crate manifest: the repo layout prefers `projects/template`, otherwise the project root itself.
pub fn game_manifest_path(root: &Path) -> PathBuf {
    let nested = root.join("projects/template/Cargo.toml");
    if nested.is_file() {
        nested
    } else {
        root.join("Cargo.toml")
    }
}

fn target_profile(settings: &ProjectSettings) -> &'static str {
    if settings.build.profile.eq_ignore_ascii_case("release") {
        "release"
    } else {
        "debug"
    }
}

/// Executable file name: `.exe` on Windows.
pub fn exe_name() -> &'static str {
    if cfg!(windows) {
        "template.exe"
    } else {
        "template"
    }
}

/// Read `[package] name` from the user project's `Cargo.toml` (fall back to `template` if not found).
///
/// The player crate isn't necessarily named `template` (`projects/demos/fishing` isn't), so artifact
/// paths follow it.
pub fn game_binary_name(manifest: &Path) -> String {
    let stem = std::fs::read_to_string(manifest)
        .ok()
        .and_then(|text| {
            let mut in_package = false;
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    in_package = line == "[package]";
                    continue;
                }
                if !in_package {
                    continue;
                }
                let Some(rest) = line.strip_prefix("name") else {
                    continue;
                };
                let rest = rest.trim_start();
                let Some(value) = rest.strip_prefix('=') else {
                    continue;
                };
                let value = value.trim().trim_matches(['"', '\'']);
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
            None
        })
        .unwrap_or_else(|| "template".to_string());
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem
    }
}

/// `assets/` artifacts needed at runtime: `.pak` package, path list, scene list, `settings.ini`,
/// plus top-level data files the project reads at runtime (`*.json`).
///
/// Project data in subdirectories (`assets/data/`) is packed into `.pak` by `build-scenes` (#1049);
/// here we only handle top-level loose files.
///
/// Only copy **artifacts**, not authoring documents (`*.scene.json` / `*.prefab.json`) or loose
/// assets: scene content is already in `.pak`, loose assets are decided by cook, copying the
/// directory would carry off a bunch of source files.
///
/// `.pak.meta.json` is the path list the runtime uses to parse `AssetId`; without it all textures
/// are judged as `broken asset reference`; `scenes.json` is the startup scene list, without it
/// no scene loads (both are "silent failures", see #989).
fn asset_artifacts(root: &Path, output_dir: &Path) -> Vec<ArtifactCopy> {
    let assets = root.join("assets");
    let Ok(entries) = std::fs::read_dir(&assets) else {
        return Vec::new();
    };
    let mut copies: Vec<ArtifactCopy> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let name = name.to_ascii_lowercase();
        let wanted = name.ends_with(".pak")
            || name.ends_with(".pak.meta.json")
            || name == "settings.ini"
            || (name.ends_with(".json")
                && !name.ends_with(".scene.json")
                && !name.ends_with(".prefab.json"));
        if !wanted {
            continue;
        }
        let to = match path.file_name() {
            Some(file) => output_dir.join("assets").join(file),
            None => continue,
        };
        copies.push(ArtifactCopy { from: path, to });
    }
    // The plan must be reproducible: sort by file name so `read_dir` order doesn't affect the
    // order of build artifacts.
    copies.sort_by(|a, b| a.from.cmp(&b.from));
    copies
}

/// Find the workspace root containing `crates/prism-build-pipeline` (walking up from project root).
///
/// User projects are usually **independent workspaces** (`projects/demos/fishing/Cargo.toml` has no
/// pipeline crate), so running `cargo run -p prism-build-pipeline` directly in the project root
/// errors with `package(s) prism-build-pipeline not found in workspace`. Cook must run in the
/// project root (it finds `assets/` by cwd), so we use `--manifest-path` to point back at the
/// engine repo.
pub fn pipeline_manifest_path(root: &Path) -> Option<PathBuf> {
    let mut current = Some(root);
    while let Some(dir) = current {
        if dir.join("crates/prism-build-pipeline/Cargo.toml").is_file() {
            return Some(dir.join("Cargo.toml"));
        }
        current = dir.parent();
    }
    None
}

fn cook_step(root: &Path) -> BuildStep {
    let mut args: Vec<String> = vec!["run".into()];
    if let Some(manifest) = pipeline_manifest_path(root) {
        args.push("--manifest-path".into());
        args.push(manifest.to_string_lossy().into_owned());
    }
    args.extend(
        ["-p", "prism-build-pipeline", "--", "build-scenes"]
            .into_iter()
            .map(str::to_string),
    );
    BuildStep {
        label: "build-scenes (cook scene pak)".into(),
        program: "cargo".into(),
        args,
        // cwd must be the project root: `build-scenes` finds `assets/scenes.json` by cwd.
        cwd: root.to_path_buf(),
    }
}

/// Generate the build plan from project root + `[build]` settings.
pub fn plan_build(root: &Path, settings: &ProjectSettings) -> BuildPlan {
    let manifest = game_manifest_path(root);
    let release = target_profile(settings) == "release";
    let mut build_args = vec![
        "build".to_string(),
        "--manifest-path".to_string(),
        manifest.to_string_lossy().into_owned(),
        // The editor parses these two outputs into Problems panel entries; `cargo run` without
        // them would pollute game stdout.
        "--message-format".to_string(),
        "json".to_string(),
    ];
    if release {
        build_args.push("--release".to_string());
    }

    let output_dir = {
        let configured = PathBuf::from(settings.build.output_dir.trim());
        // Leaving it empty must not degenerate into "project root" — that would copy assets/
        // into itself.
        let configured = if configured.as_os_str().is_empty() {
            PathBuf::from("build/player")
        } else {
            configured
        };
        if configured.is_absolute() {
            configured
        } else {
            root.join(configured)
        }
    };

    let game_target = manifest
        .parent()
        .unwrap_or(root)
        .join("target")
        .join(target_profile(settings));
    // The artifact layout must let `asset_paths` recognize `assets/`: the executable goes in
    // `output_dir/`, assets go in `output_dir/assets/` (consistent with the Web editor and the
    // game directory packing layout).
    let binary = game_binary_name(&manifest);
    let exe = output_dir.join(&binary);
    let mut copies = vec![ArtifactCopy {
        from: game_target.join(&binary),
        to: exe.clone(),
    }];
    copies.extend(asset_artifacts(root, &output_dir));

    BuildPlan {
        steps: vec![
            BuildStep {
                label: format!("cargo build ({})", target_profile(settings)),
                program: "cargo".into(),
                args: build_args,
                cwd: root.to_path_buf(),
            },
            cook_step(root),
        ],
        copies,
        output_dir,
        required: vec![exe],
    }
}

/// Whether two paths point to the same file (compared via canonicalize; if either side doesn't
/// exist, it doesn't count).
fn same_file(from: &Path, to: &Path) -> bool {
    match (from.canonicalize(), to.canonicalize()) {
        (Ok(from), Ok(to)) => from == to,
        _ => from == to,
    }
}

/// Copy existing artifacts, returning each actual `from -> to` copied; missing artifacts are
/// skipped (not a failure).
pub fn copy_artifacts(copies: &[ArtifactCopy]) -> io::Result<Vec<String>> {
    let mut copied = Vec::new();
    for copy in copies {
        if !copy.from.is_file() {
            continue;
        }
        // Copying onto itself (output_dir points back at project root) errors, just skip it.
        if same_file(&copy.from, &copy.to) {
            continue;
        }
        if let Some(parent) = copy.to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&copy.from, &copy.to)?;
        copied.push(format!("{} -> {}", copy.from.display(), copy.to.display()));
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(profile: &str, output: &str) -> ProjectSettings {
        let mut settings = ProjectSettings::default();
        settings.build.profile = profile.into();
        settings.build.output_dir = output.into();
        settings
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("prism-editor-build-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn manifest_prefers_nested_game_crate() {
        let root = temp_dir("manifest-nested");
        std::fs::create_dir_all(root.join("projects/template")).unwrap();
        std::fs::write(root.join("projects/template/Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(
            game_manifest_path(&root),
            root.join("projects/template/Cargo.toml")
        );

        let root = temp_dir("manifest-root");
        std::fs::write(root.join("Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(game_manifest_path(&root), root.join("Cargo.toml"));
    }

    /// When the user project is an independent workspace, the cook step must use `--manifest-path`
    /// to point back at the engine workspace;
    /// otherwise `cargo run -p prism-build-pipeline` directly in the project root reports
    /// "package(s) `prism-build-pipeline` not found in workspace".
    #[test]
    fn cook_step_points_back_at_the_engine_workspace() {
        let root = temp_dir("cook-standalone");
        let project = root.join("projects/demos/fishing");
        std::fs::create_dir_all(&project).unwrap();
        let plan = plan_build(&project, &settings("debug", "build/windows"));
        assert_eq!(plan.steps[1].cwd, project);
        assert!(
            !plan.steps[1]
                .args
                .iter()
                .any(|arg| arg == "--manifest-path"),
            "no engine workspace should not fabricate a manifest: {:?}",
            plan.steps[1].args
        );

        std::fs::create_dir_all(root.join("crates/prism-build-pipeline")).unwrap();
        std::fs::write(root.join("crates/prism-build-pipeline/Cargo.toml"), "").unwrap();
        let plan = plan_build(&project, &settings("debug", "build/windows"));
        let args = &plan.steps[1].args;
        let index = args
            .iter()
            .position(|arg| arg == "--manifest-path")
            .expect("when the engine repo is above, --manifest-path should be present");
        assert_eq!(args[index + 1], root.join("Cargo.toml").to_string_lossy());
        // Cook still runs in the project root, otherwise it would cook the engine repo's own assets/.
        assert_eq!(plan.steps[1].cwd, project);
        assert_eq!(args.last().map(String::as_str), Some("build-scenes"));
    }

    #[test]
    fn debug_plan_has_build_and_cook_steps() {
        let root = temp_dir("plan-debug");
        let plan = plan_build(&root, &settings("debug", "build/windows"));
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].program, "cargo");
        assert_eq!(plan.steps[0].args[0], "build");
        assert!(!plan.steps[0].args.iter().any(|arg| arg == "--release"));
        assert_eq!(
            plan.steps[1].args.last().map(String::as_str),
            Some("build-scenes")
        );
        assert_eq!(plan.steps[0].cwd, root);
        assert_eq!(plan.output_dir, root.join("build/windows"));
        // The temp dir has no `assets/`, so only one copy item (the exe).
        assert_eq!(plan.copies.len(), 1);
        assert_eq!(
            plan.copies[0].from,
            root.join("target/debug").join(exe_name())
        );
        assert_eq!(plan.required, vec![plan.copies[0].to.clone()]);
    }

    /// Artifacts must land in `output_dir/assets/`: the engine only recognizes `<root>/assets/`;
    /// flattening into `output_dir/scenes.pak` makes double-clicking the exe report
    /// "no assets directory".
    #[test]
    fn assets_are_staged_into_an_assets_subdirectory() {
        let root = temp_dir("layout");
        let assets = root.join("assets");
        std::fs::create_dir_all(assets.join("scenes")).unwrap();
        for name in [
            "scenes.pak",
            "scenes.pak.meta.json",
            "settings.ini",
            "scenes.json",
            "project_data.json",
        ] {
            std::fs::write(assets.join(name), b"x").unwrap();
        }
        // Authoring documents and loose assets must not be copied into the release directory.
        std::fs::write(assets.join("scenes/lake.scene.json"), b"{}").unwrap();
        std::fs::write(assets.join("boat.png"), b"png").unwrap();

        let plan = plan_build(&root, &settings("debug", "build/windows"));
        let out = root.join("build/windows");
        assert_eq!(plan.copies[0].to, out.join(exe_name()));
        let staged: Vec<String> = plan.copies[1..]
            .iter()
            .map(|copy| copy.to.display().to_string())
            .collect();
        for name in [
            "scenes.pak",
            "scenes.pak.meta.json",
            "settings.ini",
            "scenes.json",
            "project_data.json",
        ] {
            assert!(
                staged.contains(&out.join("assets").join(name).display().to_string()),
                "{name} did not enter the release directory: {staged:?}"
            );
        }
        assert!(!staged.iter().any(|path| path.ends_with(".scene.json")));
        assert!(!staged.iter().any(|path| path.ends_with(".png")));
    }

    #[test]
    fn the_binary_name_follows_the_game_crate() {
        let root = temp_dir("binary-name");
        std::fs::create_dir_all(root.join("projects")).unwrap();
        let manifest = root.join("projects/Cargo.toml");
        std::fs::write(
            &manifest,
            "[package]
name = \"fishing\"
version = \"0.1.0\"
",
        )
        .unwrap();
        let expected = if cfg!(windows) {
            "fishing.exe"
        } else {
            "fishing"
        };
        assert_eq!(game_binary_name(&manifest), expected);
        assert_eq!(
            game_binary_name(&root.join("nope/Cargo.toml")),
            exe_name(),
            "fall back to template when the manifest can't be read"
        );
    }

    #[test]
    fn release_plan_uses_release_target_and_absolute_output() {
        let root = temp_dir("plan-release");
        std::fs::create_dir_all(root.join("projects/template")).unwrap();
        std::fs::write(root.join("projects/template/Cargo.toml"), "[package]\n").unwrap();
        let out = std::env::temp_dir().join("prism-editor-build-abs-out");
        let plan = plan_build(&root, &settings("release", out.to_str().unwrap()));
        assert!(plan.steps[0].args.iter().any(|arg| arg == "--release"));
        assert_eq!(
            plan.copies[0].from,
            root.join("projects/template/target/release")
                .join(exe_name())
        );
        assert_eq!(plan.output_dir, out);
    }

    #[test]
    fn a_blank_output_dir_does_not_stage_into_the_project_root() {
        let root = temp_dir("blank-output");
        let plan = plan_build(&root, &settings("debug", "   "));
        assert_eq!(plan.output_dir, root.join("build/player"));
    }

    #[test]
    fn copying_a_file_onto_itself_is_skipped() {
        let root = temp_dir("same-file");
        let from = root.join("assets/scenes.pak");
        std::fs::create_dir_all(from.parent().unwrap()).unwrap();
        std::fs::write(&from, b"pak").unwrap();
        let copies = vec![ArtifactCopy {
            from: from.clone(),
            to: from.clone(),
        }];
        assert!(copy_artifacts(&copies).unwrap().is_empty());
        assert_eq!(std::fs::read(&from).unwrap(), b"pak");
    }

    #[test]
    fn copy_artifacts_skips_missing_and_copies_existing() {
        let root = temp_dir("copy");
        let from = root.join("assets/scenes.pak");
        std::fs::create_dir_all(from.parent().unwrap()).unwrap();
        std::fs::write(&from, b"pak").unwrap();
        let out = root.join("build");
        let copies = vec![
            ArtifactCopy {
                from: from.clone(),
                to: out.join("scenes.pak"),
            },
            ArtifactCopy {
                from: root.join("missing.exe"),
                to: out.join("missing.exe"),
            },
        ];
        let copied = copy_artifacts(&copies).unwrap();
        assert_eq!(copied.len(), 1);
        assert!(copied[0].contains("scenes.pak"));
        assert_eq!(std::fs::read(out.join("scenes.pak")).unwrap(), b"pak");
        assert!(!out.join("missing.exe").exists());
    }
}
