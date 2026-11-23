//! Asset file operations: create / rename / move / delete directories and files inside project `assets/`,
//! and sync path references in `.scene.json` / `.prefab.json`.
//!
//! Design constraints:
//! - Only allow operations on relative paths under `assets/`, reject `..`, absolute paths and build output directories;
//! - rename / move first scans and plans reference rewrites, then performs filesystem ops, finally writes documents back;
//! - references preserve author's original writing style (scene-dir relative / project-root relative),
//!   so both `../valley.hdr` and `assets/valley.hdr` rewritten to equivalent new path;
//! - only fields **actually asset paths** in schema ([`prism_scene::references::ASSET_PATH_FIELDS`]) rewritten,
//!   entity names, UI text and other strings that happen to look like a path stay unchanged.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::assets::assets_dir;
use crate::scene::{list_docs, read_doc, write_doc, DocKind};
use prism_asset::{AssetMeta, GuidIndex};
use prism_scene::references::{guid_field, is_asset_path_field, ASSET_PATH_FIELDS};

/// Consistent with [`crate::assets`]: build output directories not involved in editing.
const SKIP_DIRS: [&str; 3] = ["cooked", "heightmaps", "target"];
/// Write-side path of scene manifest (new format; old `scenes.toml` read-only compatible).
const SCENE_MANIFEST: &str = "assets/scenes.json";

/// Result of one asset operation.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AssetMutation {
    /// Path before operation (relative to project root, `/` separated).
    pub from: String,
    /// Path after operation; `None` when deleted.
    pub to: Option<String>,
    /// Number of references rewritten (JSON fields + manifest entries).
    pub references_updated: usize,
    /// Documents whose content may change (relative to project root, already mapped to new path), for frontend to decide whether reload.
    pub affected_documents: Vec<String>,
}

/// Create new directory under `assets/`. Parent directory must already exist.
pub fn create_asset_dir(project_root: &Path, relative: &str) -> Result<String, String> {
    let rel = normalize_asset_path(relative)?;
    let dir = project_root.join(&rel);
    if dir.exists() {
        return Err(format!("Directory already exists: {rel}"));
    }
    std::fs::create_dir(&dir).map_err(|e| format!("Failed to create directory {rel}: {e}"))?;
    Ok(rel)
}

/// Rename / move asset (file or directory), syncing document references.
pub fn rename_asset(project_root: &Path, from: &str, to: &str) -> Result<AssetMutation, String> {
    let from_rel = normalize_asset_path(from)?;
    let to_rel = normalize_asset_path(to)?;
    ensure_document_placement(&from_rel, &to_rel)?;
    if from_rel == to_rel {
        return Ok(AssetMutation {
            from: from_rel.clone(),
            to: Some(to_rel),
            references_updated: 0,
            affected_documents: Vec::new(),
        });
    }

    let src = project_root.join(&from_rel);
    let dst = project_root.join(&to_rel);
    let is_dir = std::fs::metadata(&src)
        .map_err(|e| format!("Cannot find {}: {e}", from_rel))?
        .is_dir();
    if std::fs::metadata(&dst).is_ok() {
        return Err(format!("Target already exists: {to_rel}"));
    }
    if is_dir && to_rel.starts_with(&format!("{from_rel}/")) {
        return Err("Cannot move directory into itself".into());
    }
    if let Some(parent) = dst.parent() {
        if !parent.is_dir() {
            return Err(format!(
                "Target parent directory does not exist: {}",
                parent
                    .strip_prefix(project_root)
                    .unwrap_or(parent)
                    .display()
            ));
        }
    }

    let renamer = Renamer {
        old: lexical_abs(project_root, &from_rel),
        new: lexical_abs(project_root, &to_rel),
        is_dir,
    };
    let protected_guids = collect_asset_guids(project_root, &from_rel, is_dir);
    let plan = plan_rewrites(project_root, &renamer, &protected_guids)?;

    std::fs::rename(&src, &dst).map_err(|e| format!("Failed to rename {from_rel}: {e}"))?;
    if !is_dir {
        move_asset_sidecar(&src, &dst);
    }
    let affected_documents = plan.affected.clone();
    let references_updated = apply_plan(project_root, plan, &renamer)?;

    Ok(AssetMutation {
        from: from_rel,
        to: Some(to_rel),
        references_updated,
        affected_documents,
    })
}

/// Alias of [`rename_asset`], semantically for cross-directory moves.
pub fn move_asset(project_root: &Path, from: &str, to: &str) -> Result<AssetMutation, String> {
    rename_asset(project_root, from, to)
}

/// List documents referencing that asset (relative to project root).
///
/// For delete confirmation: frontend first shows impact scope, then decides whether force delete.
pub fn asset_reference_documents(
    project_root: &Path,
    relative: &str,
) -> Result<Vec<String>, String> {
    let rel = normalize_asset_path(relative)?;
    let path = project_root.join(&rel);
    let is_dir = path.is_dir();
    let target = lexical_abs(project_root, &rel);
    let target_guids = collect_asset_guids(project_root, &rel, is_dir);
    let mut out = BTreeSet::new();

    for entry in all_docs(project_root) {
        let path = PathBuf::from(&entry.path);
        if is_dir && lexical_abs(project_root, &entry.path).starts_with(&target) {
            out.insert(relative_path_string(project_root, &path));
            continue;
        }
        let Ok(value) = read_doc(&path) else {
            continue;
        };
        let scene_dir = path.parent().unwrap_or(project_root);
        if contains_reference(
            &value,
            scene_dir,
            project_root,
            &target,
            is_dir,
            &target_guids,
        ) {
            out.insert(relative_path_string(project_root, &path));
        }
    }

    if manifest_references(project_root, &target, is_dir, &target_guids)? {
        out.insert(SCENE_MANIFEST.to_string());
    }

    Ok(out.into_iter().collect())
}

/// Delete asset. When `force == false` reject if references still exist.
pub fn delete_asset(
    project_root: &Path,
    relative: &str,
    force: bool,
) -> Result<AssetMutation, String> {
    let rel = normalize_asset_path(relative)?;
    let path = project_root.join(&rel);
    let meta = std::fs::metadata(&path).map_err(|e| format!("Cannot find {}: {e}", rel))?;

    let mut refs = asset_reference_documents(project_root, &rel)?;
    if !force && !refs.is_empty() {
        return Err(format!(
            "{} is still referenced by {} documents; confirm before deleting",
            rel,
            refs.len()
        ));
    }

    if meta.is_dir() {
        std::fs::remove_dir_all(&path)
            .map_err(|e| format!("Failed to delete directory {rel}: {e}"))?;
    } else {
        std::fs::remove_file(&path).map_err(|e| format!("Failed to delete {rel}: {e}"))?;
        let sidecar = asset_sidecar_path(&path);
        if sidecar.exists() {
            std::fs::remove_file(&sidecar)
                .map_err(|e| format!("Failed to delete {}: {e}", sidecar.display()))?;
        }
    }

    let mut references_updated = 0;
    let target_stem = rel.strip_prefix("assets/").and_then(|asset_rel| {
        if meta.is_dir() {
            Some(asset_rel.to_string())
        } else {
            scene_stem(asset_rel)
        }
    });
    if let Some(stem) = target_stem {
        references_updated = remove_manifest_entries(project_root, &stem, meta.is_dir())?;
        if references_updated > 0 && !refs.iter().any(|doc| doc == SCENE_MANIFEST) {
            refs.push(SCENE_MANIFEST.to_string());
            refs.sort();
        }
    }

    Ok(AssetMutation {
        from: rel,
        to: None,
        references_updated,
        affected_documents: refs,
    })
}

/// Asset sidecar path: `wood.png` → `wood.png.rmeta` (#466).
fn asset_sidecar_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".rmeta");
    path.with_file_name(name)
}

/// Move sidecar along on rename; silently skip when sidecar missing.
fn move_asset_sidecar(src: &Path, dst: &Path) {
    let from = asset_sidecar_path(src);
    if !from.is_file() {
        return;
    }
    let to = asset_sidecar_path(dst);
    let _ = std::fs::remove_file(&to);
    if let Err(error) = std::fs::rename(&from, &to) {
        log::warn!(
            "asset meta: failed to move {} to {}: {error}",
            from.display(),
            to.display()
        );
    }
}

/// Collect stable GUIDs of moved assets before rename.
///
/// References corresponding to these GUIDs in documents can already resolve themselves, on rename **do not** rewrite their
/// `*_path` (#466 / #465). Directory collects whole subtree.
pub(crate) fn collect_asset_guids(
    project_root: &Path,
    relative: &str,
    is_dir: bool,
) -> BTreeSet<String> {
    let mut guids = BTreeSet::new();
    if is_dir {
        let index = GuidIndex::scan(&project_root.join(relative));
        guids.extend(index.guids().map(str::to_ascii_lowercase));
        return guids;
    }

    let mut candidates = vec![relative.to_string()];
    if let Some(stem) = relative.strip_suffix(".rscn") {
        candidates.push(format!("{stem}.scene.json"));
    }
    for candidate in candidates {
        if let Some(meta) = AssetMeta::read(project_root, &candidate) {
            guids.insert(meta.guid.to_ascii_lowercase());
        }
    }
    guids
}

/// Normalize to `assets/...` form, reject out-of-bounds / build directories / illegal Windows filenames.
pub fn normalize_asset_path(raw: &str) -> Result<String, String> {
    let normalized = raw.trim().replace('\\', "/");
    if normalized.is_empty() {
        return Err("Path is empty".into());
    }
    if normalized.starts_with('/') || normalized.contains(':') {
        return Err("Must be a relative path under assets/".into());
    }

    let mut parts: Vec<&str> = Vec::new();
    for part in normalized.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err("Path cannot contain ..".into()),
            other => parts.push(other),
        }
    }

    if parts.first() != Some(&"assets") {
        return Err("Path must start with assets/".into());
    }
    if parts.len() < 2 {
        return Err("Cannot operate on assets root directory".into());
    }

    for part in &parts[1..] {
        if SKIP_DIRS.contains(part) {
            return Err(format!(
                "{part}/ is a build output directory, cannot operate in editor"
            ));
        }
        validate_name(part)?;
    }

    Ok(parts.join("/"))
}

/// Scene documents enumerated as flat directory (only scan `assets/scenes/`), must stay there;
/// prefabs enumerated recursively under `assets/`, can be in any subdirectory as long as suffix kept.
fn ensure_document_placement(from: &str, to: &str) -> Result<(), String> {
    if matches!(from, "assets/scenes" | "assets/prefabs") {
        return Err("Cannot rename or move editor document root directory".into());
    }
    if from.ends_with(".scene.json") {
        let scenes_dir = crate::scene::DocKind::Scene.subdir();
        let parent = to.rsplit_once('/').map(|(parent, _)| parent).unwrap_or("");
        if parent != scenes_dir || !to.ends_with(".scene.json") {
            return Err(format!(
                "Scene documents must stay in {scenes_dir}/ root (editor enumerates scenes as flat directory)"
            ));
        }
    }
    if from.ends_with(".prefab.json") && !to.ends_with(".prefab.json") {
        return Err(
            "Prefab must keep .prefab.json suffix (otherwise asset tree stops enumerating it)"
                .into(),
        );
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), String> {
    if name
        .chars()
        .any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
    {
        return Err(format!("Filename contains illegal characters: {name}"));
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err(format!("Filename cannot end with dot or space: {name}"));
    }
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    const RESERVED: [&str; 6] = ["CON", "PRN", "AUX", "NUL", "COM1", "LPT1"];
    if RESERVED.contains(&stem.as_str()) {
        return Err(format!("Windows reserved filename: {name}"));
    }
    Ok(())
}

struct Renamer {
    old: PathBuf,
    new: PathBuf,
    is_dir: bool,
}

impl Renamer {
    fn map(&self, abs: &Path) -> Option<PathBuf> {
        if self.is_dir {
            abs.strip_prefix(&self.old)
                .ok()
                .map(|rest| self.new.join(rest))
        } else if abs == self.old {
            Some(self.new.clone())
        } else {
            None
        }
    }

    /// `scenes/foo.rscn` in manifest maps by "same-name `.scene.json`".
    fn map_manifest(&self, abs: &Path) -> Option<PathBuf> {
        if let Some(mapped) = self.map(abs) {
            return Some(mapped);
        }
        let name = abs.file_name()?.to_str()?;
        let stem = name.strip_suffix(".rscn")?;
        let json = abs.with_file_name(format!("{stem}.scene.json"));
        let mapped = self.map(&json)?;
        let mapped_name = mapped.file_name()?.to_str()?;
        let mapped_stem = mapped_name.strip_suffix(".scene.json")?;
        Some(mapped.with_file_name(format!("{mapped_stem}.rscn")))
    }
}

/// Field names in components holding **asset paths** (full list from `prism-scene` component schema).
///
/// Only string values of these fields participate in reference rewriting: other strings in `.scene.json` (`Name`, UI
/// `Text.content`, material / tag text that happens to look like a path) are not references, changing them is data corruption.
///
/// True source in `prism-scene::references` (shared by cooker / engine / editor),
/// when adding new path-carrying component field change only there; `asset_path_fields_cover_string_paths_in_schema`
/// guards against omissions using `prism-scene` registry (does not cover `env_map` / `volume` / `prefab`
/// names not ending in `_path`, so don't rely only on test).
struct RewritePlan {
    /// `(old document absolute path, rewritten JSON)`.
    documents: Vec<(PathBuf, serde_json::Value)>,
    /// Rewritten `assets/scenes.json` text.
    manifest: Option<String>,
    /// Documents whose content changed or which were themselves moved (relative to project root, new path).
    affected: Vec<String>,
    references: usize,
}

fn plan_rewrites(
    project_root: &Path,
    renamer: &Renamer,
    protected_guids: &BTreeSet<String>,
) -> Result<RewritePlan, String> {
    let mut documents = Vec::new();
    let mut affected = BTreeSet::new();
    let mut references = 0;

    for entry in all_docs(project_root) {
        let path = PathBuf::from(&entry.path);
        let Ok(mut value) = read_doc(&path) else {
            continue;
        };
        let old_abs = lexical_abs(project_root, &entry.path);
        let mapped_abs = renamer.map(&old_abs).unwrap_or_else(|| old_abs.clone());
        let old_dir = old_abs.parent().unwrap_or(project_root);
        let write_dir = mapped_abs.parent().unwrap_or(old_dir);
        let changed = rewrite_json(
            &mut value,
            old_dir,
            write_dir,
            project_root,
            None,
            &|abs| renamer.map(abs),
            protected_guids,
        );
        let old_rel = relative_path_string(project_root, &old_abs);
        let mapped_rel = relative_path_string(project_root, &mapped_abs);
        if changed > 0 || mapped_rel != old_rel {
            affected.insert(mapped_rel);
        }
        if changed > 0 {
            references += changed;
            documents.push((path, value));
        }
    }

    let manifest = rewrite_manifest(project_root, renamer, protected_guids, &mut references)?;
    Ok(RewritePlan {
        documents,
        manifest,
        affected: affected.into_iter().collect(),
        references,
    })
}

/// Write planned document changes back (paths mapped to new locations after rename).
fn apply_plan(project_root: &Path, plan: RewritePlan, renamer: &Renamer) -> Result<usize, String> {
    for (old_path, value) in plan.documents {
        let target = renamer.map(&old_path).unwrap_or(old_path);
        write_doc(&target, &value)?;
    }
    if let Some(text) = plan.manifest {
        let path = crate::scene_manifest::manifest_path(project_root);
        std::fs::write(&path, text)
            .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
    }
    Ok(plan.references)
}

/// Recursively rewrite asset path references in documents.
///
/// `field` is name of field current string is in (array elements use outer field name). Only
/// fields recognized by [`is_asset_path_field`] treated as references; `None` (document root or unnamed
/// nested value) never changed.
fn rewrite_json(
    value: &mut serde_json::Value,
    resolve_dir: &Path,
    write_dir: &Path,
    project_root: &Path,
    field: Option<&str>,
    map: &dyn Fn(&Path) -> Option<PathBuf>,
    protected_guids: &BTreeSet<String>,
) -> usize {
    match value {
        serde_json::Value::String(text) => {
            if !field.is_some_and(is_asset_path_field) {
                return 0;
            }
            let abs_scene = lexical_abs(resolve_dir, text);
            let abs_root = lexical_abs(project_root, text);
            let (new_abs, root_style) = if let Some(mapped) = map(&abs_scene) {
                (mapped, false)
            } else if let Some(mapped) = map(&abs_root) {
                (mapped, true)
            } else {
                return 0;
            };
            let next = if Path::new(text).is_absolute() {
                slash_path(&new_abs)
            } else if root_style {
                slash_path(&relative_to(project_root, &new_abs))
            } else {
                relative_from(write_dir, &new_abs)
            };
            if next == *text {
                0
            } else {
                *text = next;
                1
            }
        }
        serde_json::Value::Array(items) => items
            .iter_mut()
            .map(|item| {
                rewrite_json(
                    item,
                    resolve_dir,
                    write_dir,
                    project_root,
                    field,
                    map,
                    protected_guids,
                )
            })
            .sum(),
        serde_json::Value::Object(fields) => rewrite_object_fields(
            fields,
            resolve_dir,
            write_dir,
            project_root,
            map,
            protected_guids,
        ),
        _ => 0,
    }
}

/// Rewrite one component object's fields, leaving GUID-protected slots alone.
///
/// A path slot is protected when its sibling `*_guid` belongs to the asset
/// being renamed: readers will resolve the GUID to the new location, so the
/// document does not need to change (#466). Legacy path-only documents still
/// take the rewrite path.
fn rewrite_object_fields(
    fields: &mut serde_json::Map<String, serde_json::Value>,
    resolve_dir: &Path,
    write_dir: &Path,
    project_root: &Path,
    map: &dyn Fn(&Path) -> Option<PathBuf>,
    protected_guids: &BTreeSet<String>,
) -> usize {
    let mut protected_single: BTreeSet<&str> = BTreeSet::new();
    let mut protected_slots: BTreeMap<&str, BTreeSet<usize>> = BTreeMap::new();

    if !protected_guids.is_empty() {
        for &path_field in ASSET_PATH_FIELDS {
            let Some(guid_field) = guid_field(path_field) else {
                continue;
            };
            match (fields.get(path_field), fields.get(guid_field)) {
                (Some(serde_json::Value::String(_)), Some(serde_json::Value::String(guid)))
                    if guid_matches(guid, protected_guids) =>
                {
                    protected_single.insert(path_field);
                }
                (Some(serde_json::Value::Array(_)), Some(serde_json::Value::Array(guids))) => {
                    let slots: BTreeSet<usize> = guids
                        .iter()
                        .enumerate()
                        .filter_map(|(index, guid)| {
                            guid.as_str()
                                .filter(|guid| guid_matches(guid, protected_guids))
                                .map(|_| index)
                        })
                        .collect();
                    if !slots.is_empty() {
                        protected_slots.insert(path_field, slots);
                    }
                }
                _ => {}
            }
        }
    }

    let mut changed = 0;
    for (name, item) in fields.iter_mut() {
        if protected_single.contains(name.as_str()) {
            continue;
        }
        if let Some(slots) = protected_slots.get(name.as_str()) {
            if let serde_json::Value::Array(items) = item {
                for (index, slot) in items.iter_mut().enumerate() {
                    if slots.contains(&index) {
                        continue;
                    }
                    changed += rewrite_json(
                        slot,
                        resolve_dir,
                        write_dir,
                        project_root,
                        Some(name.as_str()),
                        map,
                        protected_guids,
                    );
                }
                continue;
            }
        }
        changed += rewrite_json(
            item,
            resolve_dir,
            write_dir,
            project_root,
            Some(name.as_str()),
            map,
            protected_guids,
        );
    }
    changed
}

fn guid_matches(guid: &str, protected_guids: &BTreeSet<String>) -> bool {
    protected_guids.contains(&guid.to_ascii_lowercase())
}

fn contains_reference(
    value: &serde_json::Value,
    document_dir: &Path,
    project_root: &Path,
    target: &Path,
    is_dir: bool,
    target_guids: &BTreeSet<String>,
) -> bool {
    let matches = |path: &Path| path == target || (is_dir && path.starts_with(target));
    if let serde_json::Value::Object(fields) = value {
        for &path_field in ASSET_PATH_FIELDS {
            let Some(guid_field) = guid_field(path_field) else {
                continue;
            };
            match (fields.get(path_field), fields.get(guid_field)) {
                (Some(serde_json::Value::String(_)), Some(serde_json::Value::String(guid)))
                    if guid_matches(guid, target_guids) =>
                {
                    return true;
                }
                (Some(serde_json::Value::Array(_)), Some(serde_json::Value::Array(guids)))
                    if guids
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .any(|guid| guid_matches(guid, target_guids)) =>
                {
                    return true;
                }
                _ => {}
            }
        }
    }
    match value {
        serde_json::Value::String(text) => {
            matches(&lexical_abs(document_dir, text)) || matches(&lexical_abs(project_root, text))
        }
        serde_json::Value::Array(items) => items.iter().any(|item| {
            contains_reference(
                item,
                document_dir,
                project_root,
                target,
                is_dir,
                target_guids,
            )
        }),
        serde_json::Value::Object(fields) => fields.values().any(|item| {
            contains_reference(
                item,
                document_dir,
                project_root,
                target,
                is_dir,
                target_guids,
            )
        }),
        _ => false,
    }
}

fn rewrite_manifest(
    project_root: &Path,
    renamer: &Renamer,
    protected_guids: &BTreeSet<String>,
    references: &mut usize,
) -> Result<Option<String>, String> {
    if crate::scene_manifest::existing_manifest(project_root).is_none() {
        return Ok(None);
    }
    let mut scenes = crate::scene_manifest::read(project_root)?;

    let base = assets_dir(project_root);
    let mut changed = 0;
    for scene in &mut scenes {
        if scene
            .guid
            .as_deref()
            .is_some_and(|guid| protected_guids.contains(&guid.to_ascii_lowercase()))
        {
            continue;
        }
        let abs = lexical_abs(&base, &scene.path);
        let Some(mapped) = renamer.map_manifest(&abs) else {
            continue;
        };
        let new_value = slash_path(&relative_to(&base, &mapped));
        if new_value == scene.path {
            continue;
        }
        scene.path = new_value;
        changed += 1;
    }
    if changed == 0 {
        Ok(None)
    } else {
        *references += changed;
        crate::scene_manifest::render(&scenes).map(Some)
    }
}

fn manifest_references(
    project_root: &Path,
    target: &Path,
    is_dir: bool,
    target_guids: &BTreeSet<String>,
) -> Result<bool, String> {
    if crate::scene_manifest::existing_manifest(project_root).is_none() {
        return Ok(false);
    }
    let Ok(scenes) = crate::scene_manifest::read(project_root) else {
        return Ok(false);
    };
    let base = assets_dir(project_root);
    Ok(scenes.iter().any(|scene| {
        if scene
            .guid
            .as_deref()
            .is_some_and(|guid| target_guids.contains(&guid.to_ascii_lowercase()))
        {
            return true;
        }
        let abs = lexical_abs(&base, &scene.path);
        abs == *target || (is_dir && abs.starts_with(target))
    }))
}

/// Manifest path (relative to `assets/`) scene stem after removing `.scene.json` / `.rscn`.
///
/// Consistent with `build-scenes`: both suffixes point to same scene, deleting either form clears entry.
fn scene_stem(entry_path: &str) -> Option<String> {
    let trimmed = entry_path.trim_start_matches("./");
    trimmed
        .strip_suffix(".scene.json")
        .or_else(|| trimmed.strip_suffix(".rscn"))
        .map(str::to_string)
}

fn stem_matches(target: &str, stem: &str, is_dir: bool) -> bool {
    stem == target
        || (is_dir
            && stem
                .strip_prefix(target)
                .is_some_and(|rest| rest.starts_with('/')))
}

/// Remove entries in `assets/scenes.json` pointing to deleted scene.
///
/// JSON has no comments to preserve, parse → filter → write back directly (order and enabled unchanged).
fn remove_manifest_entries(
    project_root: &Path,
    target_stem: &str,
    is_dir: bool,
) -> Result<usize, String> {
    if crate::scene_manifest::existing_manifest(project_root).is_none() {
        return Ok(0);
    }
    let mut scenes = crate::scene_manifest::read(project_root)?;
    let before = scenes.len();
    scenes.retain(|scene| {
        !scene_stem(&scene.path).is_some_and(|stem| stem_matches(target_stem, &stem, is_dir))
    });
    let removed = before - scenes.len();
    if removed == 0 {
        return Ok(0);
    }
    crate::scene_manifest::write(project_root, &scenes)?;
    Ok(removed)
}

fn all_docs(project_root: &Path) -> Vec<crate::scene::SceneEntry> {
    let mut docs = Vec::new();
    for kind in [DocKind::Scene, DocKind::Prefab] {
        if let Ok(entries) = list_docs(project_root, kind) {
            docs.extend(entries);
        }
    }
    docs
}

/// Combine `base` and `value`, fold `.` / `..`, no requirement path already exists.
fn lexical_abs(base: &Path, value: &str) -> PathBuf {
    let joined = if Path::new(value).is_absolute() {
        PathBuf::from(value)
    } else {
        base.join(value)
    };

    let mut prefix = String::new();
    let mut rooted = false;
    let mut parts: Vec<String> = Vec::new();
    for component in joined.components() {
        match component {
            std::path::Component::Prefix(prefix_component) => {
                prefix = prefix_component.as_os_str().to_string_lossy().into_owned();
            }
            std::path::Component::RootDir => rooted = true,
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                parts.pop();
            }
            std::path::Component::Normal(part) => {
                parts.push(part.to_string_lossy().into_owned());
            }
        }
    }

    let mut text = prefix;
    if rooted {
        text.push(std::path::MAIN_SEPARATOR);
    }
    text.push_str(&parts.join(std::path::MAIN_SEPARATOR_STR));
    PathBuf::from(text)
}

fn relative_from(from_dir: &Path, to: &Path) -> String {
    let from = components(from_dir);
    let target = components(to);
    if from.first() != target.first() || from.get(1) != target.get(1) {
        return slash_path(to);
    }

    let common = from
        .iter()
        .zip(target.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<String> = Vec::new();
    for _ in common..from.len() {
        parts.push("..".into());
    }
    for component in &target[common..] {
        parts.push(component.to_string_lossy().into_owned());
    }
    if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    }
}

fn components(path: &Path) -> Vec<OsString> {
    path.components()
        .map(|component| component.as_os_str().to_os_string())
        .collect()
}

fn relative_to(base: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(base)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf())
}

fn relative_path_string(project_root: &Path, path: &Path) -> String {
    slash_path(&relative_to(project_root, path))
}

fn slash_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "prism_asset_ops_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("assets/scenes")).unwrap();
        std::fs::create_dir_all(dir.join("assets/prefabs")).unwrap();
        dir
    }

    fn scene_with(paths: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "version": 1,
            "entities": [{
                "name": "Entity",
                "parent": null,
                "transform": {
                    "translation": [0, 0, 0],
                    "rotation": [0, 0, 0, 1],
                    "scale": [1, 1, 1]
                },
                "components": {
                    "prism_engine::scene::MeshRenderer": { "mesh_path": paths[0] },
                    "prism_engine::scene::Skybox": { "hdr_path": paths[1] }
                }
            }]
        })
    }

    /// Write an `assets/scenes.json` (new format).
    fn write_manifest(root: &Path, entries: &[(&str, &str)]) {
        let scenes: Vec<crate::scene_manifest::SceneListEntry> = entries
            .iter()
            .map(|(name, path)| crate::scene_manifest::SceneListEntry {
                name: (*name).to_string(),
                path: (*path).to_string(),
                enabled: true,
                guid: None,
            })
            .collect();
        crate::scene_manifest::write(root, &scenes).unwrap();
    }

    #[test]
    fn normalizes_and_rejects_unsafe_paths() {
        assert_eq!(
            normalize_asset_path("assets/models/../models/a.gltf").unwrap_err(),
            "Path cannot contain .."
        );
        assert!(normalize_asset_path("D:/x").is_err());
        assert!(normalize_asset_path("assets/cooked/x.rscn").is_err());
        assert_eq!(
            normalize_asset_path("assets\\models\\a.gltf").unwrap(),
            "assets/models/a.gltf"
        );
    }

    #[test]
    fn creates_directory() {
        let root = temp_root("mkdir");
        let rel = create_asset_dir(&root, "assets/models").unwrap();
        assert_eq!(rel, "assets/models");
        assert!(root.join("assets/models").is_dir());
        assert!(create_asset_dir(&root, "assets/models").is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rename_file_rewrites_scene_and_root_style_paths() {
        let root = temp_root("rename_file");
        std::fs::create_dir_all(root.join("assets/textures")).unwrap();
        std::fs::write(root.join("assets/valley.hdr"), b"x").unwrap();
        let doc = scene_with(&["assets/valley.hdr", "../valley.hdr"]);
        write_doc(&root.join("assets/scenes/demo.scene.json"), &doc).unwrap();

        let result = rename_asset(&root, "assets/valley.hdr", "assets/textures/sky.hdr").unwrap();
        assert_eq!(result.references_updated, 2);
        assert!(result
            .affected_documents
            .contains(&"assets/scenes/demo.scene.json".to_string()));

        let text = std::fs::read_to_string(root.join("assets/scenes/demo.scene.json")).unwrap();
        assert!(text.contains("../textures/sky.hdr"));
        assert!(text.contains("assets/textures/sky.hdr"));
        std::fs::remove_dir_all(&root).ok();
    }

    /// #466: a GUID-backed reference needs no document rewrite; the reader
    /// resolves it to the asset's new path.
    #[test]
    fn rename_skips_documents_whose_reference_has_the_asset_guid() {
        let root = temp_root("rename_guid_zero_write");
        fs::create_dir_all(root.join("assets/models")).unwrap();
        fs::write(root.join("assets/models/old.gltf"), b"x").unwrap();
        let meta = AssetMeta::ensure(&root, "assets/models/old.gltf", "GltfImporter").unwrap();
        let doc_path = root.join("assets/scenes/demo.scene.json");
        let doc = serde_json::json!({
            "version": 3,
            "entities": [{
                "id": 0,
                "name": "Entity",
                "parent": null,
                "transform": {},
                "components": {
                    "prism_engine::scene::MeshRenderer": {
                        "mesh_path": "assets/models/old.gltf",
                        "mesh_guid": meta.guid
                    }
                }
            }]
        });
        write_doc(&doc_path, &doc).unwrap();
        let before = fs::read_to_string(&doc_path).unwrap();

        let result =
            rename_asset(&root, "assets/models/old.gltf", "assets/models/new.gltf").unwrap();
        assert_eq!(
            result.references_updated, 0,
            "GUID reference is not rewritten"
        );
        assert!(result.affected_documents.is_empty());
        assert_eq!(
            fs::read_to_string(&doc_path).unwrap(),
            before,
            "the document must not be touched"
        );
        assert!(root.join("assets/models/new.gltf.rmeta").is_file());

        let resolved = crate::scene::read_doc_in_project(&root, &doc_path).unwrap();
        assert_eq!(
            resolved["entities"][0]["components"]["prism_engine::scene::MeshRenderer"]["mesh_path"],
            "assets/models/new.gltf"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A GUID-only reference must still be visible to deletion / usage tools
    /// even though the stored path is stale after a zero-rewrite rename.
    #[test]
    fn guid_reference_is_listed_and_blocks_delete() {
        let root = temp_root("guid_reference_usage");
        fs::create_dir_all(root.join("assets/models")).unwrap();
        fs::write(root.join("assets/models/old.gltf"), b"x").unwrap();
        let meta = AssetMeta::ensure(&root, "assets/models/old.gltf", "GltfImporter").unwrap();
        let doc_path = root.join("assets/scenes/demo.scene.json");
        let doc = serde_json::json!({
            "version": 3,
            "entities": [{
                "id": 0,
                "name": "Entity",
                "parent": null,
                "transform": {},
                "components": {
                    "prism_engine::scene::MeshRenderer": {
                        "mesh_path": "assets/models/old.gltf",
                        "mesh_guid": meta.guid
                    }
                }
            }]
        });
        write_doc(&doc_path, &doc).unwrap();
        rename_asset(&root, "assets/models/old.gltf", "assets/models/new.gltf").unwrap();

        let documents = asset_reference_documents(&root, "assets/models/new.gltf").unwrap();
        assert!(
            documents.contains(&"assets/scenes/demo.scene.json".to_string()),
            "GUID reference must be discovered: {documents:?}"
        );
        let usage = crate::assets::asset_usage(&root, "assets/models/new.gltf").unwrap();
        assert!(
            usage.iter().any(|reference| reference.field == "mesh_path"),
            "GUID reference must show up in asset usage: {usage:?}"
        );

        let error = delete_asset(&root, "assets/models/new.gltf", false).unwrap_err();
        assert!(error.contains("still referenced"), "{error}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// Legacy path-only slots in the same array are still rewritten.
    #[test]
    fn rename_rewrites_only_unprotected_array_slots() {
        let root = temp_root("rename_guid_array");
        fs::create_dir_all(root.join("assets/materials")).unwrap();
        fs::write(root.join("assets/materials/a.mat"), b"x").unwrap();
        let meta = AssetMeta::ensure(&root, "assets/materials/a.mat", "MaterialImporter").unwrap();
        let doc_path = root.join("assets/scenes/demo.scene.json");
        let doc = serde_json::json!({
            "version": 3,
            "entities": [{
                "id": 0,
                "name": "Entity",
                "parent": null,
                "transform": {},
                "components": {
                    "prism_engine::scene::MeshRenderer": {
                        "mesh_path": "assets/models/box.gltf",
                        "material_paths": [
                            "assets/materials/a.mat",
                            "assets/materials/a.mat"
                        ],
                        "material_guids": [meta.guid, ""]
                    }
                }
            }]
        });
        write_doc(&doc_path, &doc).unwrap();

        let result =
            rename_asset(&root, "assets/materials/a.mat", "assets/materials/b.mat").unwrap();
        assert_eq!(
            result.references_updated, 1,
            "only the legacy slot is rewritten"
        );

        let written = crate::scene::read_doc(&doc_path).unwrap();
        let renderer = &written["entities"][0]["components"]["prism_engine::scene::MeshRenderer"];
        assert_eq!(renderer["material_paths"][0], "assets/materials/a.mat");
        assert_eq!(renderer["material_paths"][1], "assets/materials/b.mat");
        std::fs::remove_dir_all(&root).ok();
    }

    /// The manifest is also left alone when its scene entry has the GUID.
    #[test]
    fn rename_skips_manifest_entry_with_scene_guid() {
        let root = temp_root("rename_guid_manifest");
        let old_scene = root.join("assets/scenes/old.scene.json");
        write_doc(&old_scene, &crate::scene::empty_scene_doc()).unwrap();
        let meta =
            AssetMeta::ensure(&root, "assets/scenes/old.scene.json", "SceneImporter").unwrap();
        crate::scene_manifest::write(
            &root,
            &[crate::scene_manifest::SceneListEntry {
                name: "old".into(),
                path: "scenes/old.rscn".into(),
                enabled: true,
                guid: Some(meta.guid.clone()),
            }],
        )
        .unwrap();
        let manifest_before = fs::read_to_string(root.join("assets/scenes.json")).unwrap();

        rename_asset(
            &root,
            "assets/scenes/old.scene.json",
            "assets/scenes/new.scene.json",
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(root.join("assets/scenes.json")).unwrap(),
            manifest_before,
            "manifest path is resolved from the GUID, not rewritten"
        );
        let scenes = crate::scene_manifest::read(&root).unwrap();
        assert_eq!(scenes[0].path, "scenes/new.rscn");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rename_leaves_non_path_strings_alone() {
        let root = temp_root("rename_non_path");
        std::fs::write(root.join("assets/valley.hdr"), b"x").unwrap();
        let doc = serde_json::json!({
            "version": 3,
            "entities": [{
                "name": "assets/valley.hdr",
                "parent": null,
                "transform": {
                    "translation": [0, 0, 0],
                    "rotation": [0, 0, 0, 1],
                    "scale": [1, 1, 1]
                },
                "components": {
                    "prism_engine::scene::MeshRenderer": { "mesh_path": "assets/valley.hdr" },
                    "prism_engine::ui::Text": {
                        "content": "assets/valley.hdr",
                        "font": "assets/valley.hdr"
                    }
                }
            }]
        });
        write_doc(&root.join("assets/scenes/demo.scene.json"), &doc).unwrap();

        let result = rename_asset(&root, "assets/valley.hdr", "assets/sky.hdr").unwrap();
        assert_eq!(
            result.references_updated, 2,
            "mesh_path and Text.font are references, Text.content is not"
        );

        let text = std::fs::read_to_string(root.join("assets/scenes/demo.scene.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let components = &value["entities"][0]["components"];
        assert_eq!(
            components["prism_engine::scene::MeshRenderer"]["mesh_path"],
            "assets/sky.hdr"
        );
        assert_eq!(
            components["prism_engine::ui::Text"]["content"],
            "assets/valley.hdr"
        );
        assert_eq!(
            components["prism_engine::ui::Text"]["font"], "assets/sky.hdr",
            "Font is asset reference, rename must rewrite"
        );
        assert_eq!(value["entities"][0]["name"], "assets/valley.hdr");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn asset_path_fields_cover_string_paths_in_schema() {
        let schema = crate::schema::build_schema();
        for ty in &schema.types {
            for field in &ty.fields {
                if field.kind == "string" && field.name.ends_with("_path") {
                    assert!(
                        is_asset_path_field(&field.name),
                        "{}::{} is a path field but not registered in ASSET_PATH_FIELDS",
                        ty.name,
                        field.name
                    );
                }
            }
        }

        for name in ["env_map", "volume", "prefab", "font"] {
            assert!(is_asset_path_field(name), "{name} must stay registered");
        }
        assert!(!is_asset_path_field("content"));
        assert!(!is_asset_path_field("name"));
    }

    #[test]
    fn rename_material_updates_every_material_paths_entry() {
        let root = temp_root("rename_material_paths");
        fs::create_dir_all(root.join("assets/materials")).unwrap();
        fs::write(root.join("assets/materials/a.mat"), b"x").unwrap();
        fs::write(root.join("assets/materials/b.mat"), b"x").unwrap();
        let doc = serde_json::json!({
            "version": 3,
            "entities": [{
                "id": 0,
                "name": "Entity",
                "parent": null,
                "transform": {"position": [0, 0, 0], "rotation": [0, 0, 0, 1], "scale": [1, 1, 1]},
                "components": {
                    "prism_engine::scene::MeshRenderer": {
                        "mesh_path": "../materials/a.mat",
                        "material_paths": ["../materials/a.mat", "../materials/b.mat"]
                    }
                }
            }]
        });
        write_doc(&root.join("assets/scenes/demo.scene.json"), &doc).unwrap();

        let result = rename_asset(&root, "assets/materials/a.mat", "assets/materials/c.mat")
            .expect("rename material");
        assert!(result.references_updated >= 2, "{result:?}");
        let text = fs::read_to_string(root.join("assets/scenes/demo.scene.json")).unwrap();
        assert!(!text.contains("materials/a.mat"));
        assert_eq!(text.matches("../materials/c.mat").count(), 2);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rename_directory_updates_nested_references() {
        let root = temp_root("rename_dir");
        std::fs::create_dir_all(root.join("assets/models")).unwrap();
        std::fs::write(root.join("assets/models/a.gltf"), b"x").unwrap();
        let doc = scene_with(&["../models/a.gltf", "../models/a.gltf"]);
        write_doc(&root.join("assets/scenes/demo.scene.json"), &doc).unwrap();

        let result = rename_asset(&root, "assets/models", "assets/models2").unwrap();
        assert_eq!(result.references_updated, 2);
        assert!(root.join("assets/models2/a.gltf").is_file());

        let text = std::fs::read_to_string(root.join("assets/scenes/demo.scene.json")).unwrap();
        assert!(text.contains("../models2/a.gltf"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn delete_requires_force_when_referenced() {
        let root = temp_root("delete");
        std::fs::write(root.join("assets/valley.hdr"), b"x").unwrap();
        let doc = scene_with(&["assets/valley.hdr", "assets/valley.hdr"]);
        write_doc(&root.join("assets/scenes/demo.scene.json"), &doc).unwrap();

        assert!(delete_asset(&root, "assets/valley.hdr", false).is_err());
        let result = delete_asset(&root, "assets/valley.hdr", true).unwrap();
        assert_eq!(result.affected_documents.len(), 1);
        assert!(!root.join("assets/valley.hdr").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn delete_directory_lists_contained_documents() {
        let root = temp_root("delete_dir");
        let doc = scene_with(&["assets/a.png", "assets/a.png"]);
        write_doc(&root.join("assets/scenes/demo.scene.json"), &doc).unwrap();

        let refs = asset_reference_documents(&root, "assets/scenes").unwrap();
        assert!(refs.contains(&"assets/scenes/demo.scene.json".to_string()));
        assert!(delete_asset(&root, "assets/scenes", false).is_err());
        delete_asset(&root, "assets/scenes", true).unwrap();
        assert!(!root.join("assets/scenes").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn prefab_outside_prefabs_root_can_be_renamed_and_moved() {
        let root = temp_root("prefab_move");
        std::fs::create_dir_all(root.join("assets/models/chair")).unwrap();
        std::fs::create_dir_all(root.join("assets/actors")).unwrap();
        let doc = scene_with(&["assets/a.png", "assets/a.png"]);
        write_doc(&root.join("assets/models/chair/seat.prefab.json"), &doc).unwrap();

        rename_asset(
            &root,
            "assets/models/chair/seat.prefab.json",
            "assets/actors/seat.prefab.json",
        )
        .unwrap();
        assert!(root.join("assets/actors/seat.prefab.json").is_file());
        assert!(!root.join("assets/models/chair/seat.prefab.json").exists());

        rename_asset(
            &root,
            "assets/actors/seat.prefab.json",
            "assets/actors/stool.prefab.json",
        )
        .unwrap();
        assert!(root.join("assets/actors/stool.prefab.json").is_file());

        rename_asset(
            &root,
            "assets/actors/stool.prefab.json",
            "assets/prefabs/stool.prefab.json",
        )
        .unwrap();
        assert!(root.join("assets/prefabs/stool.prefab.json").is_file());

        rename_asset(
            &root,
            "assets/prefabs/stool.prefab.json",
            "assets/models/stool.prefab.json",
        )
        .unwrap();
        assert!(root.join("assets/models/stool.prefab.json").is_file());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn prefab_rename_must_keep_the_prefab_extension() {
        let root = temp_root("prefab_ext");
        std::fs::create_dir_all(root.join("assets/models")).unwrap();
        let doc = scene_with(&["assets/a.png", "assets/a.png"]);
        write_doc(&root.join("assets/models/chair.prefab.json"), &doc).unwrap();

        assert!(rename_asset(
            &root,
            "assets/models/chair.prefab.json",
            "assets/models/chair.json"
        )
        .is_err());
        assert!(root.join("assets/models/chair.prefab.json").is_file());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scene_documents_stay_in_flat_directory() {
        let root = temp_root("doc_placement");
        std::fs::create_dir_all(root.join("assets/external")).unwrap();
        let doc = scene_with(&["assets/a.png", "assets/a.png"]);
        write_doc(&root.join("assets/scenes/demo.scene.json"), &doc).unwrap();

        assert!(rename_asset(
            &root,
            "assets/scenes/demo.scene.json",
            "assets/external/demo.scene.json"
        )
        .is_err());
        assert!(root.join("assets/scenes/demo.scene.json").is_file());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rename_scene_updates_manifest_entry() {
        let root = temp_root("manifest");
        write_manifest(&root, &[("demo", "scenes/demo.rscn")]);
        let doc = scene_with(&["assets/a.png", "assets/a.png"]);
        write_doc(&root.join("assets/scenes/demo.scene.json"), &doc).unwrap();

        rename_asset(
            &root,
            "assets/scenes/demo.scene.json",
            "assets/scenes/level.scene.json",
        )
        .unwrap();
        let manifest = std::fs::read_to_string(root.join("assets/scenes.json")).unwrap();
        assert!(manifest.contains("scenes/level.rscn"), "{manifest}");
        assert!(!manifest.contains("scenes/demo.rscn"), "{manifest}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn delete_scene_drops_its_manifest_entry() {
        let root = temp_root("delete_scene_manifest");
        write_manifest(
            &root,
            &[
                ("intro", "scenes/intro.rscn"),
                ("default", "scenes/default.rscn"),
            ],
        );
        for name in ["intro", "default"] {
            let doc = scene_with(&["assets/a.png", "assets/a.png"]);
            write_doc(&root.join(format!("assets/scenes/{name}.scene.json")), &doc).unwrap();
        }

        let result = delete_asset(&root, "assets/scenes/intro.scene.json", true).unwrap();
        assert_eq!(result.references_updated, 1);
        assert!(result
            .affected_documents
            .contains(&"assets/scenes.json".to_string()));

        let manifest = std::fs::read_to_string(root.join("assets/scenes.json")).unwrap();
        assert!(!manifest.contains("scenes/intro.rscn"), "{manifest}");
        assert!(manifest.contains("scenes/default.rscn"), "{manifest}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn delete_scene_directory_drops_nested_entries() {
        let root = temp_root("delete_scene_dir_manifest");
        write_manifest(
            &root,
            &[
                ("chapter", "scenes/chapter/intro.rscn"),
                ("chaptered", "scenes/chaptered.rscn"),
                ("other", "scenes/other.rscn"),
            ],
        );
        std::fs::create_dir_all(root.join("assets/scenes/chapter")).unwrap();
        let doc = scene_with(&["assets/a.png", "assets/a.png"]);
        write_doc(&root.join("assets/scenes/chapter/intro.scene.json"), &doc).unwrap();

        let result = delete_asset(&root, "assets/scenes/chapter", true).unwrap();
        assert_eq!(result.references_updated, 1);

        let manifest = std::fs::read_to_string(root.join("assets/scenes.json")).unwrap();
        assert!(
            !manifest.contains("scenes/chapter/intro.rscn"),
            "{manifest}"
        );
        assert!(manifest.contains("scenes/chaptered.rscn"), "{manifest}");
        assert!(manifest.contains("scenes/other.rscn"), "{manifest}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn renaming_an_asset_moves_its_meta_sidecar() {
        let root = temp_root("rename_moves_meta");
        std::fs::create_dir_all(root.join("assets/textures")).unwrap();
        std::fs::write(root.join("assets/textures/a.png"), b"x").unwrap();
        std::fs::write(
            root.join("assets/textures/a.png.rmeta"),
            r#"{"guid":"0123456789abcdef0123456789abcdef","importer":"TextureImporter"}"#,
        )
        .unwrap();

        rename_asset(&root, "assets/textures/a.png", "assets/textures/b.png").unwrap();
        assert!(!root.join("assets/textures/a.png.rmeta").exists());
        let moved = std::fs::read_to_string(root.join("assets/textures/b.png.rmeta"))
            .expect("sidecar must follow the rename");
        assert!(moved.contains("0123456789abcdef0123456789abcdef"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn deleting_an_asset_removes_its_meta_sidecar() {
        let root = temp_root("delete_removes_meta");
        std::fs::create_dir_all(root.join("assets/textures")).unwrap();
        std::fs::write(root.join("assets/textures/a.png"), b"x").unwrap();
        let meta = root.join("assets/textures/a.png.rmeta");
        std::fs::write(&meta, b"{}").unwrap();

        delete_asset(&root, "assets/textures/a.png", true).unwrap();
        assert!(!root.join("assets/textures/a.png").exists());
        assert!(!meta.exists(), "sidecar must be deleted with the asset");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn delete_unrelated_asset_keeps_manifest() {
        let root = temp_root("delete_unrelated_manifest");
        std::fs::create_dir_all(root.join("assets/textures")).unwrap();
        std::fs::write(root.join("assets/textures/a.png"), b"x").unwrap();
        write_manifest(&root, &[("intro", "scenes/intro.rscn")]);
        let before = std::fs::read_to_string(root.join("assets/scenes.json")).unwrap();

        let result = delete_asset(&root, "assets/textures/a.png", true).unwrap();
        assert_eq!(result.references_updated, 0);
        assert_eq!(
            std::fs::read_to_string(root.join("assets/scenes.json")).unwrap(),
            before
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
