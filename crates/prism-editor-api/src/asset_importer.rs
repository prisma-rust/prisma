//! Asset importer facade.
//!
//! Prisma assets are **handled directly by extension** (no `.meta`, no import database,
//! no per-asset import settings). This layer projects "which importer handles this file" into a class alias,
//! for editor display and tool scripts.
//!
//! | method  | here  |
//! |---|---|
//! | `get_at_path(path)` | [`get_at_path`] |
//! | `asset_path` | [`AssetImporter::asset_path`] |
//! | `user_data` | [`AssetImporter::user_data`] |
//! | `import_settings_missing` | [`AssetImporter::import_settings_missing`] |
//! | texture / model / audio / text … | [`ImporterClass`] |
//!
//! Differences (important, not to be treated as a bug): no import setting storage, so `user_data` always empty,
//! `import_settings_missing` always `true`; these fields exist to keep interface shape,
//! not to hint at a sidecar file. Real import parameters come from cooker `CookProfile`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::asset_ops::normalize_asset_path;
use crate::assets;

/// Importer responsible for a class of assets = engine concrete `AssetImporter` subclass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImporterClass {
    /// Texture → `ImageImporter`.
    Texture,
    /// Model → `ModelImporter`.
    Model,
    /// Audio → `AudioImporter`.
    Audio,
    /// HDR / EXR environment texture → `ImageImporter` (also texture import here).
    Environment,
    /// `.scene.json` → `DefaultImporter` (Prisma text scene format).
    Scene,
    /// `.prefab.json` → `DefaultImporter`.
    Prefab,
    /// `.pak` → `DefaultImporter` (packaged output, import = copy).
    Package,
    /// Text / shader source → `TextImporter`.
    Text,
    /// Others → `DefaultImporter`.
    Default,
}

impl ImporterClass {
    /// Concrete importer class name.
    pub const fn importer_name(self) -> &'static str {
        match self {
            Self::Texture | Self::Environment => "ImageImporter",
            Self::Model => "ModelImporter",
            Self::Audio => "AudioImporter",
            Self::Text => "TextImporter",
            Self::Scene | Self::Prefab | Self::Package | Self::Default => "DefaultImporter",
        }
    }

    /// Pick importer by extension (consistent with [`crate::assets::asset_kind`] classification,
    /// but further distinguishes scene / prefab / text source).
    pub fn from_path(path: &str) -> Self {
        let lower = path.to_ascii_lowercase();
        if lower.ends_with(".scene.json") {
            return Self::Scene;
        }
        if lower.ends_with(".prefab.json") {
            return Self::Prefab;
        }
        let ext = lower.rsplit('.').next().unwrap_or("");
        match ext {
            "gltf" | "glb" | "glb2" | "obj" | "fbx" | "mesh" => Self::Model,
            "png" | "jpg" | "jpeg" | "ktx2" | "dds" | "tga" | "webp" | "gif" | "bmp" | "ico"
            | "svg" | "avif" => Self::Texture,
            "hdr" | "exr" => Self::Environment,
            "wav" | "mp3" | "ogg" | "flac" | "m4a" | "aac" => Self::Audio,
            "pak" => Self::Package,
            "json" | "toml" | "ron" | "yaml" | "yml" | "txt" | "md" | "csv" | "ini" | "log"
            | "slang" | "shader" | "glsl" | "wgsl" | "hlsl" | "vert" | "frag" | "comp" => {
                Self::Text
            }
            _ => Self::Default,
        }
    }
}

/// One asset importer description = `AssetImporter.GetAtPath` return value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetImporter {
    /// Path relative to project root (`/` separated).
    pub asset_path: String,
    /// Importer class name (like `ImageImporter`).
    pub importer: &'static str,
    /// Main asset type (coarse classification, same as [`crate::assets::asset_kind`]).
    pub main_asset_type: &'static str,
    /// User custom data (no sidecar, always empty).
    pub user_data: String,
    /// Whether missing import settings (no `.meta`, always `true`).
    pub import_settings_missing: bool,
}

/// Query an asset importer = `AssetImporter.GetAtPath`.
///
/// Returns `Err` when path missing or is directory (correct directory returns `null`).
pub fn get_at_path(project_root: &Path, asset_path: &str) -> Result<AssetImporter, String> {
    let rel = normalize_asset_path(asset_path)?;
    let full = project_root.join(&rel);
    if full.is_dir() {
        return Err(format!("Directory has no importer: {rel}"));
    }
    if !full.is_file() {
        return Err(format!("Asset does not exist: {rel}"));
    }
    Ok(AssetImporter {
        importer: ImporterClass::from_path(&rel).importer_name(),
        main_asset_type: assets::asset_kind(&rel),
        asset_path: rel,
        user_data: String::new(),
        import_settings_missing: true,
    })
}

/// Enumerate importers for all assets under `assets/` (same-source version of `AssetInfo.importer`).
pub fn list_importers(project_root: &Path) -> Result<Vec<AssetImporter>, String> {
    let importers = assets::list_assets(project_root)?
        .into_iter()
        .map(|path| AssetImporter {
            importer: ImporterClass::from_path(&path).importer_name(),
            main_asset_type: assets::asset_kind(&path),
            asset_path: path,
            user_data: String::new(),
            import_settings_missing: true,
        })
        .collect();
    Ok(importers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "prism_editor_importer_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        dir
    }

    #[test]
    fn maps_extensions_to_importer_classes() {
        assert_eq!(
            ImporterClass::from_path("assets/tex/a.PNG").importer_name(),
            "ImageImporter"
        );
        assert_eq!(
            ImporterClass::from_path("assets/models/a.gltf").importer_name(),
            "ModelImporter"
        );
        assert_eq!(
            ImporterClass::from_path("assets/audio/step.ogg").importer_name(),
            "AudioImporter"
        );
        assert_eq!(
            ImporterClass::from_path("assets/env/valley.hdr").importer_name(),
            "ImageImporter"
        );
        assert_eq!(
            ImporterClass::from_path("assets/shaders/x.slang").importer_name(),
            "TextImporter"
        );
        assert_eq!(
            ImporterClass::from_path("assets/weird.xyz").importer_name(),
            "DefaultImporter"
        );
        assert_eq!(
            ImporterClass::from_path("assets/gi/probe_volume.bin").importer_name(),
            "DefaultImporter"
        );
    }

    #[test]
    fn scene_and_prefab_suffixes_win_over_the_last_extension() {
        assert_eq!(
            ImporterClass::from_path("assets/scenes/intro.scene.json"),
            ImporterClass::Scene
        );
        assert_eq!(
            ImporterClass::from_path("assets/prefabs/lamp.prefab.json"),
            ImporterClass::Prefab
        );
        assert_eq!(
            ImporterClass::from_path("assets/data/x.json"),
            ImporterClass::Text
        );
    }

    #[test]
    fn get_at_path_reads_main_asset_type() {
        let root = temp_root("get");
        let assets = crate::assets::assets_dir(&root);
        std::fs::create_dir_all(assets.join("tex")).unwrap();
        std::fs::write(assets.join("tex/a.png"), b"x").unwrap();

        let importer = get_at_path(&root, "assets/tex/a.png").expect("importer");
        assert_eq!(importer.importer, "ImageImporter");
        assert_eq!(importer.main_asset_type, "texture");
        assert_eq!(importer.asset_path, "assets/tex/a.png");
        assert!(importer.import_settings_missing);
        assert!(importer.user_data.is_empty());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn get_at_path_rejects_missing_files_and_directories() {
        let root = temp_root("reject");
        std::fs::create_dir_all(crate::assets::assets_dir(&root).join("tex")).unwrap();

        assert!(get_at_path(&root, "assets/tex/missing.png").is_err());
        assert!(get_at_path(&root, "assets/tex").is_err());
    }

    #[test]
    fn list_importers_covers_every_asset() {
        let root = temp_root("list");
        let assets = crate::assets::assets_dir(&root);
        std::fs::create_dir_all(assets.join("scenes")).unwrap();
        std::fs::create_dir_all(assets.join("models")).unwrap();
        std::fs::write(assets.join("models/a.obj"), b"x").unwrap();
        std::fs::write(assets.join("scenes/demo.scene.json"), b"{}").unwrap();

        let importers = list_importers(&root).unwrap();
        assert_eq!(importers.len(), 2);
        assert_eq!(importers[0].asset_path, "assets/models/a.obj");
        assert_eq!(importers[0].importer, "ModelImporter");
        assert_eq!(importers[1].asset_path, "assets/scenes/demo.scene.json");
        assert_eq!(importers[1].importer, "DefaultImporter");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn serializes_with_camel_case_keys() {
        let importer = AssetImporter {
            asset_path: "assets/tex/a.png".into(),
            importer: "ImageImporter",
            main_asset_type: "texture",
            user_data: String::new(),
            import_settings_missing: true,
        };
        let json = serde_json::to_value(&importer).unwrap();
        assert_eq!(json["assetPath"], "assets/tex/a.png");
        assert_eq!(json["mainAssetType"], "texture");
        assert_eq!(json["importSettingsMissing"], true);
        assert_eq!(json["userData"], "");
    }
}
