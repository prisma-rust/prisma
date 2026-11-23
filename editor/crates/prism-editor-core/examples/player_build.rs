//! Headless recreation of the editor's "build project": `cargo build` → `build-scenes` → copy artifacts.
//!
//! Usage (project root is the directory containing `assets/settings.ini`):
//!
//! ```text
//! cargo run -p prism-editor-core --example player_build -- projects/demos/fishing
//! ```
//!
//! Reason for existence: #989-style "artifact manifest missing files" problems are only discovered by actually running a full release layout,
//! and clicking buttons in the UI can't get into CI. This example walks the **editor's same** [`plan_build`] /
//! [`copy_artifacts`], so it can serve as a release smoke script.

use prism_editor_core::{build, settings::SettingsCache};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let Some(root) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: player_build <project-root>");
        return ExitCode::from(2);
    };
    let root = match root.canonicalize() {
        Ok(root) => root,
        Err(error) => {
            eprintln!("project root {}: {error}", root.display());
            return ExitCode::FAILURE;
        }
    };

    let cache = SettingsCache::load(&root);
    if let Some(error) = &cache.error {
        eprintln!("settings.ini: {error}");
    }
    let plan = build::plan_build(&root, &cache.settings);

    for step in &plan.steps {
        println!("$ {} {}", step.program, step.args.join(" "));
        let status = match Command::new(&step.program)
            .args(&step.args)
            .current_dir(&step.cwd)
            .status()
        {
            Ok(status) => status,
            Err(error) => {
                eprintln!("{}: {error}", step.label);
                return ExitCode::FAILURE;
            }
        };
        if !status.success() {
            eprintln!("{} failed with {status}", step.label);
            return ExitCode::FAILURE;
        }
    }

    match build::copy_artifacts(&plan.copies) {
        Ok(copied) => {
            for line in &copied {
                println!("copied {line}");
            }
        }
        Err(error) => {
            eprintln!("copy artifacts: {error}");
            return ExitCode::FAILURE;
        }
    }

    // `required` is the **executable in the release directory**, only present after copy.
    for required in &plan.required {
        if !required.is_file() {
            eprintln!("missing build output: {}", required.display());
            return ExitCode::FAILURE;
        }
    }

    println!("player: {}", plan.output_dir.display());
    ExitCode::SUCCESS
}
