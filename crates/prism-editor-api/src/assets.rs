//! Asset directory browsing and reference analysis: provides data for the editor's "path fields" and "asset manager".
//!
//! Only does filename / size enumeration and string scanning in `.scene.json`, does not parse `.pak`,
//! so no need to link the runtime asset layer.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::asset_ops::normalize_asset_path;
use prism_scene::references::{guid_field, ASSET_PATH_FIELDS};

/// Max entries returned per call (avoid huge asset directories dragging down UI).
pub const MAX_ASSETS: usize = 4000;

/// Byte cap for editor inline preview (images and other binaries).
pub const MAX_PREVIEW_BYTES: u64 = 16 * 1024 * 1024;

/// Byte cap for text preview.
pub const MAX_TEXT_PREVIEW_BYTES: u64 = 512 * 1024;

/// Subdirectories skipped during enumeration: build outputs / huge intermediate data.
const SKIP_DIRS: [&str; 3] = ["cooked", "heightmaps", "target"];

/// `assets/` directory under project root.
pub fn assets_dir(project_root: &Path) -> PathBuf {
    project_root.join("assets")
}

/// Recursively list files under `assets/`, return paths **relative to project root** (`/` separated, already sorted).
///
/// Directory traversal has depth and count limits; hidden dirs and [`SKIP_DIRS`] are skipped.
pub fn list_assets(project_root: &Path) -> Result<Vec<String>, String> {
    let root = assets_dir(project_root);
    if !root.is_dir() {
        return Err(format!("Asset directory not found {}", root.display()));
    }

    let mut out = Vec::new();
    walk_files(&root, project_root, &mut out);
    out.sort();
    out.dedup();
    Ok(out)
}

/// Recursively list directories under `assets/` (including empty ones), relative to project root, `/` separated.
///
/// Asset manager uses this to render directory tree: file paths alone cannot represent a newly created empty folder.
pub fn list_asset_dirs(project_root: &Path) -> Result<Vec<String>, String> {
    let root = assets_dir(project_root);
    if !root.is_dir() {
        return Err(format!("Asset directory not found {}", root.display()));
    }

    let mut out = Vec::new();
    walk_dirs(&root, project_root, &mut out);
    out.sort();
    out.dedup();
    Ok(out)
}

fn walk_dirs(dir: &Path, project_root: &Path, out: &mut Vec<String>) {
    if out.len() >= MAX_ASSETS {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();

    for path in paths {
        if out.len() >= MAX_ASSETS {
            return;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with('.') || !path.is_dir() || SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        if let Ok(relative) = path.strip_prefix(project_root) {
            out.push(relative.to_string_lossy().replace('\\', "/"));
        }
        walk_dirs(&path, project_root, out);
    }
}

fn walk_files(dir: &Path, project_root: &Path, out: &mut Vec<String>) {
    if out.len() >= MAX_ASSETS {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    // Stable order: sort by filename then recurse.
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();

    for path in paths {
        if out.len() >= MAX_ASSETS {
            return;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk_files(&path, project_root, out);
        } else if let Ok(relative) = path.strip_prefix(project_root) {
            out.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

// ---------------------------------------------------------------------------
// Asset metadata
// ---------------------------------------------------------------------------

/// Metadata of one asset (for editor "asset manager").
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AssetInfo {
    /// Path relative to project root (`/` separated).
    pub path: String,
    /// File size (bytes; 0 on read failure).
    pub size_bytes: u64,
    /// Coarse class: `model` / `texture` / `env` / `scene` / `package` / `data` / `other`.
    pub kind: &'static str,
    /// Name of importer class responsible for importing the file (`ImageImporter` / `ModelImporter` ...).
    pub importer: &'static str,
}

/// Coarse-classify asset by extension.
pub fn asset_kind(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".scene.json") {
        return "scene";
    }
    let ext = lower.rsplit('.').next().unwrap_or("");
    match ext {
        "gltf" | "glb" | "obj" | "fbx" | "mesh" | "glb2" => "model",
        "png" | "jpg" | "jpeg" | "ktx2" | "dds" | "tga" | "webp" => "texture",
        "hdr" | "exr" => "env",
        "rscn" | "scene" => "scene",
        "pak" => "package",
        "json" | "toml" | "txt" | "md" | "bin" => "data",
        "ttf" | "otf" | "ttc" => "font",
        _ => "other",
    }
}

/// Enumerate assets with size and class attached.
pub fn list_assets_info(project_root: &Path) -> Result<Vec<AssetInfo>, String> {
    let paths = list_assets(project_root)?;
    Ok(paths
        .into_iter()
        .map(|path| {
            // `path` relative to project root (not relative to `assets/`).
            let size_bytes = std::fs::metadata(project_root.join(&path))
                .map(|meta| meta.len())
                .unwrap_or(0);
            AssetInfo {
                importer: crate::asset_importer::ImporterClass::from_path(&path).importer_name(),
                kind: asset_kind(&path),
                path,
                size_bytes,
            }
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Asset preview
// ---------------------------------------------------------------------------

/// Preview content of a text asset.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AssetTextPreview {
    /// Path relative to project root (`/` separated).
    pub path: String,
    /// File content (at most `max_bytes` bytes, truncated at UTF-8 char boundary).
    pub text: String,
    /// Whether truncated due to limit.
    pub truncated: bool,
    /// Total file size (bytes).
    pub size_bytes: u64,
}

fn resolve_asset_file(project_root: &Path, raw: &str) -> Result<PathBuf, String> {
    let normalized = normalize_asset_path(raw)?;
    let path = project_root.join(&normalized);
    if !path.is_file() {
        return Err(format!("Asset missing or not a file: {normalized}"));
    }
    Ok(path)
}

/// Read first `max_bytes` bytes of asset (for image preview).
///
/// Only allow paths under `assets/`, and reject oversized files — avoid
/// pushing tens of MB of HDR / model into WebView in one IPC call.
pub fn read_asset_bytes(project_root: &Path, raw: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
    let path = resolve_asset_file(project_root, raw)?;
    let size = std::fs::metadata(&path)
        .map(|meta| meta.len())
        .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    if size > max_bytes {
        return Err(format!(
            "File too large: {:.1} MB (preview limit {:.1} MB)",
            size as f64 / (1024.0 * 1024.0),
            max_bytes as f64 / (1024.0 * 1024.0)
        ));
    }
    std::fs::read(&path).map_err(|e| format!("Cannot read {}: {e}", path.display()))
}

/// Read text asset (decode as UTF-8, truncate when over limit).
pub fn read_asset_text(
    project_root: &Path,
    raw: &str,
    max_bytes: u64,
) -> Result<AssetTextPreview, String> {
    let path = resolve_asset_file(project_root, raw)?;
    let size = std::fs::metadata(&path)
        .map(|meta| meta.len())
        .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;

    // Read one extra byte to determine whether truncated.
    let mut file =
        std::fs::File::open(&path).map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;

    let truncated = bytes.len() as u64 > max_bytes;
    if truncated {
        bytes.truncate(max_bytes as usize);
    }
    if bytes.contains(&0) {
        return Err("File is binary content, cannot preview as text".into());
    }

    // Truncation may land mid multi-byte char; only roll back to nearest char boundary;
    // if the rest of file is not UTF-8 still report error, do not silently swallow content.
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(err) => {
            let valid = err.utf8_error().valid_up_to();
            let bytes = err.into_bytes();
            if truncated && bytes.len() - valid <= 3 {
                String::from_utf8(bytes[..valid].to_vec())
                    .map_err(|_| "File is not UTF-8 text, cannot preview".to_string())?
            } else {
                return Err("File is not UTF-8 text, cannot preview".into());
            }
        }
    };

    Ok(AssetTextPreview {
        path: raw.replace('\\', "/"),
        text,
        truncated,
        size_bytes: size,
    })
}

// ---------------------------------------------------------------------------
// Reference analysis
// ---------------------------------------------------------------------------

/// One record of "a component field of some scene references this asset".
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AssetRef {
    /// Reference source: `"field"` = component field in document; `"manifest"` = `assets/scenes.json` entry.
    pub kind: String,
    /// Scene logical name (manifest entry fixed to `scenes.json`).
    pub scene: String,
    /// Absolute path of scene file.
    pub scene_path: String,
    /// Entity name (`#<index>` when unnamed; manifest entry uses entry name).
    pub entity: String,
    pub entity_index: usize,
    /// Canonical component name (manifest entry uses `assets/scenes.json`).
    pub component: String,
    /// Field path inside component (dot-connected, array index `[i]`; manifest entry uses `path`).
    pub field: String,
    /// Actual value written in that field (may be relative path).
    pub value: String,
}

/// Scan all `.scene.json` / `.prefab.json`, find fields referencing `target`,
/// then add build manifest entries in `assets/scenes.json` pointing to that asset.
///
/// `target` is path **relative to project root**. References in fields are resolved against two bases:
/// 1. relative to scene file's directory (engine convention, e.g. `../valley.hdr` in `default.scene.json`);
/// 2. relative to project root (authors sometimes write `assets/valley.hdr`).
///
/// Both normalized to absolute path then compared, so no false positives from same filename.
pub fn asset_usage(project_root: &Path, target: &str) -> Result<Vec<AssetRef>, String> {
    let target_abs = resolve(project_root, target);
    if target_abs.is_empty() {
        return Err("Target asset path is empty".into());
    }
    let target_guids = normalize_asset_path(target)
        .ok()
        .map(|relative| {
            let is_dir = project_root.join(&relative).is_dir();
            crate::asset_ops::collect_asset_guids(project_root, &relative, is_dir)
        })
        .unwrap_or_default();
    let mut refs = Vec::new();

    // Scenes and prefabs share the same document shape, both need scanning; prefabs under `assets/` are enumerated recursively.
    let mut documents = crate::scene::list_scenes(project_root)?;
    documents.extend(crate::scene::list_docs(
        project_root,
        crate::scene::DocKind::Prefab,
    )?);

    for entry in documents {
        let doc = match crate::scene::read_scene_doc(Path::new(&entry.path)) {
            Ok(doc) => doc,
            Err(_) => continue,
        };
        let Some(entities) = doc.get("entities").and_then(|v| v.as_array()) else {
            continue;
        };

        for (index, entity) in entities.iter().enumerate() {
            let entity_name = entity
                .get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("#{index}"));
            let Some(components) = entity.get("components").and_then(|v| v.as_object()) else {
                continue;
            };

            let scene_dir = Path::new(&entry.path)
                .parent()
                .unwrap_or(project_root)
                .to_path_buf();

            for (component, payload) in components {
                let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
                let mut found = Vec::new();
                collect_strings(payload, "", &mut found);
                for (field, value) in found {
                    if value.trim().is_empty() {
                        continue;
                    }
                    let referenced = resolve(&scene_dir, &value) == target_abs
                        || resolve(project_root, &value) == target_abs;
                    if referenced && seen.insert((field.clone(), value.clone())) {
                        refs.push(AssetRef {
                            kind: "field".into(),
                            scene: entry.name.clone(),
                            scene_path: entry.path.clone(),
                            entity: entity_name.clone(),
                            entity_index: index,
                            component: component.clone(),
                            field,
                            value,
                        });
                    }
                }

                for (field, value) in collect_guid_refs(payload, &target_guids, target) {
                    if !seen.insert((field.clone(), value.clone())) {
                        continue;
                    }
                    refs.push(AssetRef {
                        kind: "field".into(),
                        scene: entry.name.clone(),
                        scene_path: entry.path.clone(),
                        entity: entity_name.clone(),
                        entity_index: index,
                        component: component.clone(),
                        field,
                        value,
                    });
                }
            }
        }
    }

    refs.extend(manifest_usage(project_root, &target_abs)?);
    Ok(refs)
}

/// Entries in `assets/scenes.json` pointing to that asset.
///
/// Manifest path can be `.rscn` (cooked artifact) or `.scene.json` (authored document), both compared by same
/// stem — consistent with `build-scenes` convention; so deleting / renaming either form is detected.
fn manifest_usage(project_root: &Path, target_abs: &str) -> Result<Vec<AssetRef>, String> {
    let Some(path) = crate::scene_manifest::existing_manifest(project_root) else {
        return Ok(Vec::new());
    };
    let Ok(manifest) = crate::scene_manifest::read(project_root) else {
        return Ok(Vec::new());
    };

    let base = assets_dir(project_root);
    let mut refs = Vec::new();
    for (index, entry) in manifest.iter().enumerate() {
        if entry.path.trim().is_empty() {
            continue;
        }
        let trimmed = entry.path.trim_start_matches("./");
        let stem = trimmed
            .strip_suffix(".scene.json")
            .or_else(|| trimmed.strip_suffix(".rscn"));
        let mut candidates = vec![resolve(&base, trimmed)];
        if let Some(stem) = stem {
            candidates.push(resolve(&base, &format!("{stem}.scene.json")));
            candidates.push(resolve(&base, &format!("{stem}.rscn")));
        }
        if candidates.iter().any(|candidate| candidate == target_abs) {
            refs.push(AssetRef {
                kind: "manifest".into(),
                scene: "scenes.json".into(),
                scene_path: path.to_string_lossy().into_owned(),
                entity: entry.name.clone(),
                entity_index: index,
                component: "assets/scenes.json".into(),
                field: "path".into(),
                value: entry.path.clone(),
            });
        }
    }
    Ok(refs)
}

/// Resolve `value` with `base`, fold `.` / `..`, return `/`-separated normalized path.
fn resolve(base: &Path, value: &str) -> String {
    use std::path::Component;

    let joined = if Path::new(value).is_absolute() {
        PathBuf::from(value)
    } else {
        base.join(value)
    };

    let mut stack: Vec<String> = Vec::new();
    for component in joined.components() {
        match component {
            Component::RootDir => stack.clear(),
            Component::CurDir => {}
            Component::ParentDir => {
                stack.pop();
            }
            Component::Prefix(prefix) => {
                stack.push(prefix.as_os_str().to_string_lossy().into_owned());
            }
            Component::Normal(part) => stack.push(part.to_string_lossy().into_owned()),
        }
    }
    stack.join("/")
}

/// Recursively collect strings in JSON, field path shaped like `a.b[0].c`.
/// GUID-backed path slots that point at `target_guids` (the stored path may be
/// stale after a zero-rewrite rename).
fn collect_guid_refs(
    payload: &serde_json::Value,
    target_guids: &BTreeSet<String>,
    fallback: &str,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if target_guids.is_empty() {
        return out;
    }
    let Some(fields) = payload.as_object() else {
        return out;
    };
    let matches = |guid: &str| target_guids.contains(&guid.to_ascii_lowercase());
    for &path_field in ASSET_PATH_FIELDS {
        let Some(guid_field) = guid_field(path_field) else {
            continue;
        };
        match (fields.get(path_field), fields.get(guid_field)) {
            (Some(serde_json::Value::String(path)), Some(serde_json::Value::String(guid)))
                if matches(guid) =>
            {
                let value = if path.is_empty() {
                    fallback
                } else {
                    path.as_str()
                };
                out.push((path_field.to_string(), value.to_string()));
            }
            (Some(serde_json::Value::Array(paths)), Some(serde_json::Value::Array(guids))) => {
                for (index, guid) in guids.iter().enumerate() {
                    let Some(guid) = guid.as_str() else {
                        continue;
                    };
                    if !matches(guid) {
                        continue;
                    }
                    let path = paths.get(index).and_then(serde_json::Value::as_str);
                    let value = path.filter(|path| !path.is_empty()).unwrap_or(fallback);
                    out.push((format!("{path_field}[{index}]"), value.to_string()));
                }
            }
            _ => {}
        }
    }
    out
}

fn collect_strings(value: &serde_json::Value, prefix: &str, out: &mut Vec<(String, String)>) {
    match value {
        serde_json::Value::String(text) => out.push((prefix.to_string(), text.clone())),
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_strings(item, &format!("{prefix}[{index}]"), out);
            }
        }
        serde_json::Value::Object(map) => {
            for (key, item) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                collect_strings(item, &path, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "prism_editor_assets_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        dir
    }

    #[test]
    fn lists_relative_paths_and_skips_build_dirs() {
        let root = temp_root("list");
        let assets = assets_dir(&root);
        std::fs::create_dir_all(assets.join("external/models")).unwrap();
        std::fs::create_dir_all(assets.join("cooked/scenes")).unwrap();
        std::fs::create_dir_all(assets.join("heightmaps")).unwrap();
        std::fs::write(assets.join("valley.hdr"), b"x").unwrap();
        std::fs::write(assets.join("external/models/a.gltf"), b"x").unwrap();
        std::fs::write(assets.join("cooked/scenes/secret.rscn"), b"x").unwrap();
        std::fs::write(assets.join("heightmaps/h.png"), b"x").unwrap();

        let listed = list_assets(&root).unwrap();
        assert_eq!(
            listed,
            vec![
                "assets/external/models/a.gltf".to_string(),
                "assets/valley.hdr".to_string(),
            ]
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn lists_directories_including_empty_ones() {
        let root = temp_root("dirs");
        let assets = assets_dir(&root);
        std::fs::create_dir_all(assets.join("models/empty")).unwrap();
        std::fs::create_dir_all(assets.join("cooked/scenes")).unwrap();
        std::fs::write(assets.join("models/a.gltf"), b"x").unwrap();

        let dirs = list_asset_dirs(&root).unwrap();
        assert!(dirs.contains(&"assets/models".to_string()));
        assert!(dirs.contains(&"assets/models/empty".to_string()));
        assert!(!dirs.iter().any(|dir| dir.contains("cooked")));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_assets_dir_is_an_error() {
        let root = temp_root("missing");
        assert!(list_assets(&root).is_err());
    }

    #[test]
    fn classifies_assets_by_extension() {
        assert_eq!(asset_kind("assets/m/a.gltf"), "model");
        assert_eq!(asset_kind("assets/t/a.PNG"), "texture");
        assert_eq!(asset_kind("assets/e/valley.hdr"), "env");
        assert_eq!(asset_kind("assets/scenes/intro.scene.json"), "scene");
        assert_eq!(asset_kind("assets/scenes.pak"), "package");
        assert_eq!(asset_kind("assets/data/x.toml"), "data");
        assert_eq!(asset_kind("assets/gi/probe_volume.bin"), "data");
        assert_eq!(asset_kind("assets/weird.xyz"), "other");
    }

    #[test]
    fn asset_info_reports_size_and_kind() {
        let root = temp_root("info");
        let assets = assets_dir(&root);
        std::fs::create_dir_all(assets.join("scenes")).unwrap();
        std::fs::write(assets.join("valley.hdr"), b"abcd").unwrap();
        std::fs::write(assets.join("scenes/demo.scene.json"), b"{}").unwrap();

        let info = list_assets_info(&root).unwrap();
        assert_eq!(info.len(), 2);
        assert_eq!(info[0].path, "assets/scenes/demo.scene.json");
        assert_eq!(info[0].kind, "scene");
        assert_eq!(info[0].importer, "DefaultImporter");
        assert_eq!(info[0].size_bytes, 2);
        assert_eq!(info[1].path, "assets/valley.hdr");
        assert_eq!(info[1].kind, "env");
        assert_eq!(info[1].importer, "ImageImporter");
        assert_eq!(info[1].size_bytes, 4);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn finds_asset_references_including_relative_ones() {
        let root = temp_root("usage");
        let scenes = crate::scene::scenes_dir(&root);
        std::fs::create_dir_all(&scenes).unwrap();

        let doc = serde_json::json!({
            "version": 1,
            "entities": [
                {
                    "name": "Sky",
                    "parent": null,
                    "transform": { "translation": [0, 0, 0], "rotation": [0, 0, 0, 1], "scale": [1, 1, 1] },
                    "components": {
                        "prism_engine::scene::Skybox": { "hdr_path": "../valley.hdr", "enabled": true },
                        "prism_engine::scene::EnvironmentLighting": { "env_map": "../valley.hdr" },
                        "prism_engine::ui::Text": { "content": "not an asset", "font_size": 12.0, "color": [1, 1, 1, 1], "alignment": "Center" }
                    }
                },
                {
                    "name": "Mesh",
                    "parent": 0,
                    "transform": { "translation": [0, 0, 0], "rotation": [0, 0, 0, 1], "scale": [1, 1, 1] },
                    "components": {
                        "prism_engine::scene::MeshRenderer": { "mesh_path": "assets/valley.hdr" }
                    }
                }
            ]
        });
        std::fs::write(
            scenes.join("demo.scene.json"),
            serde_json::to_string(&doc).unwrap(),
        )
        .unwrap();

        let refs = asset_usage(&root, "assets/valley.hdr").unwrap();
        assert_eq!(refs.len(), 3, "got {refs:#?}");

        let fields: Vec<&str> = refs.iter().map(|r| r.field.as_str()).collect();
        assert!(fields.contains(&"hdr_path")); // Scene-dir relative: ../valley.hdr
        assert!(fields.contains(&"env_map"));
        assert!(fields.contains(&"mesh_path")); // Project-root relative: assets/valley.hdr

        let sky = refs.iter().find(|r| r.field == "hdr_path").unwrap();
        assert_eq!(sky.entity, "Sky");
        assert_eq!(sky.entity_index, 0);
        assert_eq!(sky.component, "prism_engine::scene::Skybox");
        assert_eq!(sky.scene, "demo");

        // Unrelated strings do not produce references.
        assert!(asset_usage(&root, "not_an_asset").unwrap().is_empty());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn finds_references_in_prefabs() {
        let root = temp_root("usage_prefab");
        std::fs::create_dir_all(root.join("assets/models")).unwrap();
        std::fs::create_dir_all(root.join("assets/prefabs")).unwrap();
        std::fs::write(root.join("assets/models/a.gltf"), b"x").unwrap();
        let doc = serde_json::json!({
            "version": 1,
            "entities": [{
                "name": "Chair",
                "parent": null,
                "transform": { "translation": [0, 0, 0], "rotation": [0, 0, 0, 1], "scale": [1, 1, 1] },
                "components": {
                    "prism_engine::scene::MeshRenderer": { "mesh_path": "../models/a.gltf" }
                }
            }]
        });
        std::fs::write(
            root.join("assets/prefabs/chair.prefab.json"),
            serde_json::to_string(&doc).unwrap(),
        )
        .unwrap();

        let refs = asset_usage(&root, "assets/models/a.gltf").unwrap();
        assert_eq!(refs.len(), 1, "got {refs:#?}");
        assert_eq!(refs[0].kind, "field");
        assert_eq!(refs[0].scene, "chair");
        assert_eq!(refs[0].entity, "Chair");
        assert!(refs[0].scene_path.ends_with("chair.prefab.json"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn finds_references_in_scene_manifest() {
        let root = temp_root("usage_manifest");
        std::fs::create_dir_all(root.join("assets/scenes")).unwrap();
        crate::scene_manifest::write(
            &root,
            &[crate::scene_manifest::SceneListEntry {
                name: "intro".into(),
                path: "scenes/intro.rscn".into(),
                enabled: true,
                guid: None,
            }],
        )
        .unwrap();

        // `.rscn` and its authored document `.scene.json` treated as same scene.
        for target in ["assets/scenes/intro.rscn", "assets/scenes/intro.scene.json"] {
            let refs = asset_usage(&root, target).unwrap();
            assert_eq!(refs.len(), 1, "{target}: {refs:#?}");
            assert_eq!(refs[0].kind, "manifest");
            assert_eq!(refs[0].scene, "scenes.json");
            assert_eq!(refs[0].entity, "intro");
            assert_eq!(refs[0].field, "path");
            assert!(refs[0].scene_path.ends_with("scenes.json"));
        }

        // Unregistered asset does not produce manifest reference.
        assert!(asset_usage(&root, "assets/scenes/other.rscn")
            .unwrap()
            .is_empty());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reads_asset_bytes_and_rejects_oversized_files() {
        let root = temp_root("preview_bytes");
        let assets = assets_dir(&root);
        std::fs::create_dir_all(assets.join("textures")).unwrap();
        std::fs::write(assets.join("textures/a.png"), b"PNGDATA").unwrap();

        let bytes = read_asset_bytes(&root, "assets/textures/a.png", 64).unwrap();
        assert_eq!(bytes, b"PNGDATA");
        assert!(read_asset_bytes(&root, "assets/textures/a.png", 4).is_err());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reads_text_with_truncation_on_char_boundary() {
        let root = temp_root("preview_text");
        let assets = assets_dir(&root);
        std::fs::create_dir_all(assets.join("data")).unwrap();
        std::fs::write(assets.join("data/a.txt"), "你好，世界").unwrap();

        let full = read_asset_text(&root, "assets/data/a.txt", 1024).unwrap();
        assert_eq!(full.text, "你好，世界");
        assert!(!full.truncated);
        assert_eq!(full.size_bytes, "你好，世界".len() as u64);

        // Truncating to 7 bytes lands inside a multi-byte char; should fall back to 6 bytes (two CJK chars).
        let cut = read_asset_text(&root, "assets/data/a.txt", 7).unwrap();
        assert_eq!(cut.text, "你好");
        assert!(cut.truncated);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn preview_rejects_paths_outside_assets() {
        let root = temp_root("preview_safe");
        let assets = assets_dir(&root);
        std::fs::create_dir_all(&assets).unwrap();
        std::fs::write(assets.join("a.txt"), b"x").unwrap();

        assert!(read_asset_text(&root, "../a.txt", 64).is_err());
        assert!(read_asset_bytes(&root, "assets/../a.txt", 64).is_err());
        assert!(read_asset_text(&root, "assets/missing.txt", 64).is_err());

        std::fs::remove_dir_all(&root).ok();
    }
}
