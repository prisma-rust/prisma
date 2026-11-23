//! One-time migration of old project config: `settings.toml` → the INI settings file,
//!
//! `scenes.toml` → `scenes.json`.
//!
//! Only run when **new file doesn't exist**, so repeating is safe (idempotent) and won't overwrite
//! user-edited new format files. Old files remain on disk (engine prefers new file),
//! migration only keeps old project runnable; after confirming no issues, manually delete.
//!
//! `settings.toml` content fully overlaps INI dialect (`[section]` + `key = value` +
//! quoted strings + bare booleans/numbers), so directly reuse engine INI parser to re-serialize;
//! scene manifest `[[scenes]]` array table then reuse
//! [`prism_scene::SceneManifest::parse`] old format branch.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use prism_ini::Ini;
use prism_scene::SceneManifest;

/// Which files this migration wrote (`None` = file doesn't need migration).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationSummary {
    pub settings: Option<PathBuf>,
    pub scenes: Option<PathBuf>,
}

impl MigrationSummary {
    pub fn is_empty(&self) -> bool {
        self.settings.is_none() && self.scenes.is_none()
    }
}

/// Run migration under `assets/` directory, return actually written new files.
///
/// `assets_dir` is **directory containing** `settings.toml` / `scenes.toml` (usually
/// `<project>/assets`).
pub fn migrate_legacy_config(assets_dir: &Path) -> Result<MigrationSummary> {
    let mut summary = MigrationSummary::default();

    if let Some(path) = migrate_settings(assets_dir)? {
        log::info!(
            "migrated {} -> settings.ini",
            assets_dir.join("settings.toml").display()
        );
        summary.settings = Some(path);
    }
    if let Some(path) = migrate_scenes(assets_dir)? {
        log::info!(
            "migrated {} -> scenes.json",
            assets_dir.join("scenes.toml").display()
        );
        summary.scenes = Some(path);
    }

    Ok(summary)
}

fn migrate_settings(assets_dir: &Path) -> Result<Option<PathBuf>> {
    let legacy = assets_dir.join("settings.toml");
    let target = assets_dir.join("settings.ini");
    if target.is_file() || !legacy.is_file() {
        return Ok(None);
    }
    let text =
        std::fs::read_to_string(&legacy).with_context(|| format!("read {}", legacy.display()))?;
    let document = Ini::parse(&text);
    if document.sections().is_empty() {
        log::warn!("{} has no [section]; leaving it alone", legacy.display());
        return Ok(None);
    }
    std::fs::write(&target, document.render())
        .with_context(|| format!("write {}", target.display()))?;
    Ok(Some(target))
}

fn migrate_scenes(assets_dir: &Path) -> Result<Option<PathBuf>> {
    let legacy = assets_dir.join("scenes.toml");
    let target = assets_dir.join("scenes.json");
    if target.is_file() || !legacy.is_file() {
        return Ok(None);
    }
    let text =
        std::fs::read_to_string(&legacy).with_context(|| format!("read {}", legacy.display()))?;
    let manifest = SceneManifest::parse(&text)
        .map_err(|error| anyhow::anyhow!("parse {}: {error}", legacy.display()))?;
    let json =
        serde_json::to_string_pretty(&manifest).context("serialize the migrated scene manifest")?;
    std::fs::write(&target, format!("{json}\n"))
        .with_context(|| format!("write {}", target.display()))?;
    Ok(Some(target))
}

/// Parse `build-scenes` manifest path: first migrate old config, then return **actually need to read** file.
///
/// Default parameter is `assets/scenes.json`; old project only has `scenes.toml` will first migrate to
/// same directory `scenes.json`, so here only do "existence + same directory fallback" step.
pub fn resolve_scene_manifest(manifest: &Path) -> Result<PathBuf> {
    let assets_dir = manifest.parent().unwrap_or_else(|| Path::new("."));
    migrate_legacy_config(assets_dir)?;

    if manifest.is_file() {
        return Ok(manifest.to_path_buf());
    }
    let sibling = assets_dir.join("scenes.json");
    if sibling.is_file() {
        return Ok(sibling);
    }
    anyhow::bail!("scene manifest not found: {}", manifest.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_assets(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "prism-migrate-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn migrates_settings_keeping_every_section_and_key() {
        let assets = temp_assets("settings");
        std::fs::write(
            assets.join("settings.toml"),
            "[app]\ncompany = \"Acme\"\nname = \"Game\"\n\n[window]\nwidth = 1280\nvsync = false\n",
        )
        .unwrap();

        let summary = migrate_legacy_config(&assets).unwrap();
        assert!(summary.settings.is_some());
        assert!(summary.scenes.is_none());

        let text = std::fs::read_to_string(assets.join("settings.ini")).unwrap();
        let document = Ini::parse(&text);
        assert_eq!(document.get("app", "company"), Some("Acme"));
        assert_eq!(document.get_u32("window", "width"), Some(1280));
        assert_eq!(document.get_bool("window", "vsync"), Some(false));
        assert!(assets.join("settings.toml").is_file());
        std::fs::remove_dir_all(&assets).ok();
    }

    #[test]
    fn migrates_scenes_and_is_idempotent() {
        let assets = temp_assets("scenes");
        std::fs::write(
            assets.join("scenes.toml"),
            "# hand written\n[[scenes]]\nname = \"intro\"\npath = \"scenes/intro.rscn\"\n\
             [[scenes]]\nname = \"draft\"\npath = \"scenes/draft.rscn\"\nenabled = false\n",
        )
        .unwrap();

        let summary = migrate_legacy_config(&assets).unwrap();
        assert!(summary.scenes.is_some());
        let text = std::fs::read_to_string(assets.join("scenes.json")).unwrap();
        assert!(text.contains("\"intro\""), "{text}");
        assert!(text.contains("\"enabled\": false"), "{text}");

        let again = migrate_legacy_config(&assets).unwrap();
        assert!(again.is_empty(), "{again:?}");
        std::fs::remove_dir_all(&assets).ok();
    }

    #[test]
    fn resolve_manifest_prefers_the_new_file_and_migrates_first() {
        let assets = temp_assets("resolve");
        std::fs::write(
            assets.join("scenes.toml"),
            "[[scenes]]\nname = \"a\"\npath = \"scenes/a.rscn\"\n",
        )
        .unwrap();

        let resolved = resolve_scene_manifest(&assets.join("scenes.json")).unwrap();
        assert_eq!(resolved, assets.join("scenes.json"));
        assert!(resolved.is_file());
        std::fs::remove_dir_all(&assets).ok();
    }

    #[test]
    fn missing_manifest_is_an_error() {
        let assets = temp_assets("missing");
        let error = resolve_scene_manifest(&assets.join("scenes.json")).unwrap_err();
        assert!(error.to_string().contains("not found"));
        std::fs::remove_dir_all(&assets).ok();
    }
}
