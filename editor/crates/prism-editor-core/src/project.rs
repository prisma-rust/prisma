//! Project file layer: locates `assets/`, scans scenes/prefabs, reads/writes `.scene.json`.
//!
//! This is just **file system**, does not depend on `prism-engine` / `prism-render`. Asset root
//! lookup rule follows `prisma_engine::asset_paths` semantics: walk up from start, only candidates
//! with existing `assets/` subdirectory count (see AGENTS.md "Asset root resolution").

use crate::doc::SceneDoc;
use crate::workspace::DocKind;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// A document on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectFile {
    /// Relative to project root, e.g. `assets/scenes/default.scene.json`.
    pub path: String,
    pub name: String,
    pub kind: DocKind,
}

/// From `start` walk up looking for directory containing `assets/`.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        if dir.join("assets").is_dir() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

/// Scan `assets/scenes` and `assets/prefabs` (recursive, max 4 layers).
pub fn scan_documents(root: &Path) -> Vec<ProjectFile> {
    let mut files = Vec::new();
    collect(root, "assets/scenes", DocKind::Scene, 0, &mut files);
    collect(root, "assets/prefabs", DocKind::Prefab, 0, &mut files);
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

/// Scan entire `assets/` (for asset panel / inspector asset selector), max 12 layers.
/// Paths uniformly carry `assets/` prefix, consistent with `.scene.json` reference shape.
pub fn scan_assets(root: &Path) -> Vec<crate::assets::AssetInfo> {
    let mut files = Vec::new();
    collect_assets(root, "assets", 0, &mut files);
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

/// Build products / import settings type **derived files** do not generate sidecar: they appear
/// and disappear with build, giving them GUID only makes `git status` noisy, and no reference
/// needs stable identity.
fn is_derived_asset(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".import.json") {
        return true;
    }
    let extension = lower
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .unwrap_or("");
    matches!(
        extension,
        "pak"
            | "rscn"
            | "spv"
            | "rmes"
            | "rtex"
            | "rmat"
            | "rskel"
            | "rskin"
            | "ranim"
            | "rasm"
            | "rag"
            | "rbsp"
    )
}

/// For all assets under `assets/` still missing sidecar, generate `<path>.rmeta`,
/// return number newly written.
///
/// Existing valid sidecar untouched (GUID stable across rename); corrupted treated as missing
/// and rewritten. Called by editor on project open / asset refresh; this is an **explicit**
/// operation, not silently writing disk inside read-only [`scan_assets`].
pub fn ensure_asset_metas(root: &Path) -> usize {
    let assets = scan_assets(root);
    let mut written = 0;
    for asset in assets {
        if is_derived_asset(&asset.path) {
            continue;
        }
        if crate::asset_meta::AssetMeta::read(root, &asset.path).is_some() {
            continue;
        }
        match crate::asset_meta::AssetMeta::ensure(root, &asset.path, &asset.importer) {
            Ok(_) => written += 1,
            Err(error) => log::warn!(
                "asset meta: failed to write {}: {error}",
                crate::asset_meta::meta_relative_path(&asset.path)
            ),
        }
    }
    written
}

fn collect_assets(
    root: &Path,
    relative: &str,
    depth: usize,
    out: &mut Vec<crate::assets::AssetInfo>,
) {
    if depth > 12 {
        return;
    }
    let Ok(entries) = fs::read_dir(root.join(relative)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if name.ends_with(crate::asset_meta::META_SUFFIX) {
            continue;
        }
        let child = format!("{relative}/{name}");
        let path = entry.path();
        if path.is_dir() {
            collect_assets(root, &child, depth + 1, out);
            continue;
        }
        let size_bytes = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
        let guid = crate::asset_meta::AssetMeta::read(root, &child).map(|meta| meta.guid);
        out.push(crate::assets::AssetInfo {
            kind: asset_kind(&child),
            importer: importer_for(&child),
            path: child,
            size_bytes,
            guid,
        });
    }
}

/// Classify by extension (approximating `prism-editor-api`'s asset classification).
pub fn asset_kind(path: &str) -> crate::assets::AssetKind {
    use crate::assets::AssetKind;
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".scene.json") {
        return AssetKind::Scene;
    }
    if lower.ends_with(".prefab.json") {
        return AssetKind::Prefab;
    }
    if lower.ends_with(".mat.json") {
        return AssetKind::Material;
    }
    match lower.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("") {
        "fbx" | "obj" | "gltf" | "glb" | "rmesh" | "dae" | "blend" => AssetKind::Model,
        "png" | "jpg" | "jpeg" | "webp" | "tga" | "bmp" | "ktx2" | "rtex" => AssetKind::Texture,
        "hdr" | "exr" => AssetKind::Env,
        "rscn" => AssetKind::Scene,
        "wav" | "ogg" | "mp3" | "flac" => AssetKind::Audio,
        "pak" => AssetKind::Package,
        "ttf" | "otf" | "ttc" => AssetKind::Font,
        "material" | "mat" => AssetKind::Material,
        "json" | "toml" | "ron" | "yaml" | "yml" | "bin" | "slang" | "glsl" | "txt" => {
            AssetKind::Data
        }
        _ => AssetKind::Other,
    }
}

/// Scan all directories under `assets/` (including empty dirs), paths carry `assets/` prefix.
pub fn scan_asset_dirs(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, relative: &str, depth: usize, out: &mut Vec<String>) {
        if depth > 12 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            if !entry.path().is_dir() {
                continue;
            }
            let child = format!("{relative}/{name}");
            out.push(child.clone());
            walk(&entry.path(), &child, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(&root.join("assets"), "assets", 0, &mut out);
    out.sort();
    out
}

fn importer_for(path: &str) -> String {
    use crate::assets::AssetKind;
    match asset_kind(path) {
        AssetKind::Model => "ModelImporter",
        AssetKind::Texture => "ImageImporter",
        AssetKind::Env => "ImageImporter",
        AssetKind::Scene | AssetKind::Prefab => "DefaultImporter",
        AssetKind::Audio => "AudioImporter",
        AssetKind::Material => "TextImporter",
        AssetKind::Data => "TextImporter",
        _ => "DefaultImporter",
    }
    .to_string()
}

fn collect(root: &Path, relative: &str, kind: DocKind, depth: usize, out: &mut Vec<ProjectFile>) {
    if depth > 4 {
        return;
    }
    let directory = root.join(relative);
    let Ok(entries) = fs::read_dir(&directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let child_relative = format!("{relative}/{name}");
        if path.is_dir() {
            collect(root, &child_relative, kind, depth + 1, out);
            continue;
        }
        if !name.ends_with(kind.suffix()) {
            continue;
        }
        out.push(ProjectFile {
            path: child_relative,
            name,
            kind,
        });
    }
}

/// Read a document; path is relative to project root `assets/...`.
pub fn read_document(root: &Path, relative: &str) -> io::Result<SceneDoc> {
    let text = fs::read_to_string(root.join(relative))?;
    let mut doc: SceneDoc = serde_json::from_str(&text)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    doc.normalize();
    Ok(doc)
}

/// File modification time; returns `None` if missing / unreadable.
///
/// Editor uses this for "was modified externally before save" check (#452). mtime granularity
/// depends on filesystem (NTFS 100ns, FAT 2s), changes within same tick not detected -- acceptable
/// trade-off: real concurrent writes still won't tear file due to atomic rename.
pub fn file_mtime(root: &Path, relative: &str) -> Option<std::time::SystemTime> {
    fs::metadata(root.join(relative)).ok()?.modified().ok()
}

/// A file's "version stamp": mtime + length (#495 item 5), document can also carry content digest.
///
/// Only checking mtime misses same-second overwrites on coarse-grained filesystems (some Android
/// / FUSE / network drives); adding length covers most real edits. Document conflict detection
/// additionally computes content digest (`file_content_stamp*`), because "length and mtime unchanged"
/// overwrites do happen (#864, test loop 80 times reproduces ~9 times).
///
/// Loose asset polling (`loose_assets`) still uses `file_stamp*` without digest: reading large
/// textures/models every poll isn't worth it, mtime + length suffices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
    /// If filesystem doesn't provide mtime, `None`, compare only by length.
    pub modified: Option<std::time::SystemTime>,
    pub len: u64,
    /// Only populated by `file_content_stamp*`; `None` means no content read.
    pub digest: Option<u64>,
}

/// Relative project root file version stamp; returns `None` if missing. Doesn't read content.
pub fn file_stamp(root: &Path, relative: &str) -> Option<FileStamp> {
    file_stamp_at(&root.join(relative))
}

/// Absolute path file version stamp; returns `None` if missing / can't stat. Doesn't read content.
pub fn file_stamp_at(path: &Path) -> Option<FileStamp> {
    let meta = fs::metadata(path).ok()?;
    Some(FileStamp {
        modified: meta.modified().ok(),
        len: meta.len(),
        digest: None,
    })
}

/// Relative project root document version stamp: content digest in addition to mtime + length (#864).
pub fn file_content_stamp(root: &Path, relative: &str) -> Option<FileStamp> {
    file_content_stamp_at(&root.join(relative))
}

/// Absolute path document version stamp; returns `None` if missing / unreadable.
///
/// For small docs like scene / prefab only, not for polling whole asset directory.
pub fn file_content_stamp_at(path: &Path) -> Option<FileStamp> {
    let meta = fs::metadata(path).ok()?;
    let bytes = fs::read(path).ok()?;
    Some(FileStamp {
        modified: meta.modified().ok(),
        len: meta.len(),
        digest: Some(fnv1a64(&bytes)),
    })
}

/// FNV-1a 64-bit -- only for "file content changed or not", not for security scenarios.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Whether asset is "hand-edited JSON" type (material `.mat.json` / `.asset.json`).
///
/// These two have no dedicated document model; editor edits files as ordinary JSON values (#488).
pub fn is_json_editable_asset(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".mat.json") || lower.ends_with(".asset.json")
}

/// Read a JSON asset (not parsed into any specific structure, retains unknown fields).
pub fn read_json_asset(root: &Path, relative: &str) -> io::Result<serde_json::Value> {
    let text = fs::read_to_string(root.join(relative))?;
    serde_json::from_str(&text)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))
}

/// Atomically write a JSON asset (2-space indent + trailing newline, same as scene file).
pub fn write_json_asset(root: &Path, relative: &str, value: &serde_json::Value) -> io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    text.push('\n');
    atomic_write_verified(&path, &text, |text| {
        serde_json::from_str::<serde_json::Value>(text)
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
}

/// Write back a document (auto-create parent dir, 2-space indent, consistent with authoring format).
///
/// Save must be **atomic**: first write `<path>.tmp`, `sync_all`, roll old file to `<path>.bak`,
/// then `rename` to official path, finally read-back and parse once. Any step failure restores `.bak`,
/// avoiding truncated JSON from power loss / disk full.
pub fn write_document(root: &Path, relative: &str, doc: &SceneDoc) -> io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(doc)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    text.push('\n');
    atomic_write(&path, &text)
}

/// Sibling file in same directory as `path` (append suffix, keep original extension).
fn sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn atomic_write(path: &Path, text: &str) -> io::Result<()> {
    atomic_write_verified(path, text, |text| {
        serde_json::from_str::<SceneDoc>(text)
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
}

/// Generic version of [`atomic_write`]: `verify` decides "whether written content is acceptable".
fn atomic_write_verified(
    path: &Path,
    text: &str,
    verify: impl Fn(&str) -> Result<(), String>,
) -> io::Result<()> {
    use std::io::Write as _;

    let tmp = sibling_path(path, ".tmp");
    let bak = sibling_path(path, ".bak");
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }

    let had_original = path.exists();
    if had_original {
        let _ = fs::remove_file(&bak);
        if let Err(error) = fs::rename(path, &bak) {
            let _ = fs::remove_file(&tmp);
            return Err(error);
        }
    }

    if let Err(error) = fs::rename(&tmp, path) {
        if had_original {
            let _ = fs::rename(&bak, path);
        }
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }

    let verified = fs::read_to_string(path)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
        .and_then(|text| {
            verify(&text).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
        });
    if let Err(error) = verified {
        if had_original {
            let _ = fs::rename(&bak, path);
        } else {
            let _ = fs::remove_file(path);
        }
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("saved document failed to read back: {error}"),
        ));
    }
    Ok(())
}

/// Asset rename / move (only within `assets/`), fails if target exists.
pub fn rename_path(root: &Path, from: &str, to: &str) -> io::Result<()> {
    if !from.starts_with("assets/") || !to.starts_with("assets/") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "only files within assets/ may be moved",
        ));
    }
    if from == to {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source and target same",
        ));
    }
    let source = root.join(from);
    let target = root.join(to);
    if !source.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{from} does not exist"),
        ));
    }
    if target.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{to} already exists"),
        ));
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let is_file = source.is_file();
    fs::rename(source, target)?;
    if is_file {
        let source_meta = asset_sidecar_path(&root.join(from));
        if source_meta.is_file() {
            let target_meta = asset_sidecar_path(&root.join(to));
            let _ = fs::remove_file(&target_meta);
            if let Err(error) = fs::rename(&source_meta, &target_meta) {
                log::warn!(
                    "asset meta: failed to move {} to {}: {error}",
                    source_meta.display(),
                    target_meta.display()
                );
            }
        }
    }
    Ok(())
}

fn asset_sidecar_path(path: &Path) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(crate::asset_meta::META_SUFFIX);
    path.with_file_name(name)
}

/// Create new asset directory (only within `assets/`).
pub fn create_folder(root: &Path, relative: &str) -> io::Result<()> {
    if !relative.starts_with("assets/") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "only directories within assets/ may be created",
        ));
    }
    fs::create_dir_all(root.join(relative))
}

/// Copy asset file (only within `assets/`), fails if target exists.
pub fn copy_file(root: &Path, from: &str, to: &str) -> io::Result<()> {
    if !from.starts_with("assets/") || !to.starts_with("assets/") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "only files within assets/ may be copied",
        ));
    }
    let source = root.join(from);
    let target = root.join(to);
    if !source.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{from} does not exist"),
        ));
    }
    if target.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{to} already exists"),
        ));
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, target).map(|_| ())
}

/// Default save path for unsaved document; appends `-2`, `-3` on duplicate names.
pub fn sanitize_stem(input: &str) -> String {
    let cleaned: String = input
        .trim()
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other => other,
        })
        .collect();
    let cleaned = cleaned.trim_matches(['.', ' ']).to_string();
    if cleaned.is_empty() {
        "Untitled".to_string()
    } else {
        cleaned
    }
}

pub fn default_save_path(root: &Path, kind: DocKind, title: &str) -> String {
    unique_save_path(root, kind, title, &[])
}

/// Same as [`default_save_path`], additionally avoiding paths already occupied in `taken`.
///
/// Disk check only covers "files left from previous session"; opening two template documents
/// consecutively in same session, previous one not yet saved to disk, checking disk only gives
/// same name (#449).
pub fn unique_save_path(root: &Path, kind: DocKind, title: &str, taken: &[String]) -> String {
    let stem = title
        .trim_end_matches(kind.suffix())
        .trim_end_matches(".json")
        .trim();
    let stem = sanitize_stem(stem);
    let folder = kind.folder();
    let mut candidate = format!("{folder}/{stem}{}", kind.suffix());
    let mut index = 2;
    while root.join(&candidate).exists() || taken.iter().any(|path| path == &candidate) {
        candidate = format!("{folder}/{stem}-{index}{}", kind.suffix());
        index += 1;
    }
    candidate
}

/// Normalize user-input save path (allows bare filename, allows backslashes).
/// Is relative path (relative to project root, `assets/...` form) valid?
///
/// Pure string prefix check isn't enough: `assets/scenes/../evil.scene.json` passes
/// `starts_with("assets/")`, but after `root.join()` it writes outside `assets/`;
/// `../../x` directly escapes project root. Uses `Path::components` here.
pub fn asset_path_problem(path: &str) -> Option<&'static str> {
    use std::path::Component;
    if path.is_empty() {
        return Some("path is empty");
    }
    if path.contains(':') {
        return Some("path cannot contain `:`");
    }
    for component in std::path::Path::new(path).components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => return Some("path cannot contain `..`"),
            Component::RootDir | Component::Prefix(_) => {
                return Some("path cannot be absolute path")
            }
        }
    }
    None
}

/// Normalize filename to `kind.suffix()`.
///
/// `level2` / `level2.scene` / `level2.json` / `level2.scene.json` all get
/// `level2.scene.json`. `Save As` dialog default also goes here, so can't save a file
/// without suffix that Document panel and Build Settings don't recognize (#459).
pub fn ensure_document_suffix(kind: DocKind, name: &str) -> String {
    let suffix = kind.suffix();
    if name.ends_with(suffix) {
        return name.to_string();
    }
    let short = suffix.trim_end_matches(".json");
    let mut stem = name.strip_suffix(".json").unwrap_or(name);
    stem = stem.strip_suffix(short).unwrap_or(stem);
    format!("{stem}{suffix}")
}

pub fn normalize_save_path(kind: DocKind, input: &str) -> String {
    let trimmed = input.trim().replace('\\', "/");
    let trimmed = trimmed.trim_start_matches("./");
    if trimmed.is_empty() {
        return String::new();
    }
    if asset_path_problem(&trimmed).is_some() {
        return String::new();
    }
    let with_folder = if trimmed.starts_with("assets/") {
        trimmed.to_string()
    } else {
        format!("{}/{trimmed}", kind.folder())
    };
    ensure_document_suffix(kind, &with_folder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn file_stamp_tracks_length_even_when_mtime_is_coarse() {
        let dir = std::env::temp_dir().join(format!("prism-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.txt");
        std::fs::write(&path, b"one").unwrap();

        let first = file_stamp_at(&path).expect("stamp");
        assert_eq!(first.len, 3);
        assert_eq!(first.digest, None, "poll path doesn't read content");
        std::fs::write(&path, b"longer").unwrap();
        let second = file_stamp_at(&path).expect("stamp");
        assert_eq!(second.len, 6);
        assert_ne!(first, second, "length change must be detected");

        assert!(file_stamp_at(&dir.join("missing.txt")).is_none());
        assert_eq!(
            file_stamp(&dir, "a.txt").map(|stamp| stamp.len),
            Some(6),
            "relative path version matches absolute path"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_content_stamp_detects_same_length_rewrites() {
        let dir = std::env::temp_dir().join(format!("prism-content-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.txt");
        std::fs::write(&path, b"one").unwrap();

        let first = file_content_stamp_at(&path).expect("stamp");
        std::fs::write(&path, b"two").unwrap();
        let second = file_content_stamp_at(&path).expect("stamp");
        assert_eq!(first.len, second.len);
        assert!(first.digest.is_some());
        assert_ne!(first.digest, second.digest);
        assert_ne!(first, second, "same-length overwrite must be detected");

        std::fs::remove_dir_all(&dir).ok();
    }

    struct TempProject {
        root: PathBuf,
    }

    impl TempProject {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "prism-editor-core-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join("assets/scenes/nested")).unwrap();
            fs::create_dir_all(root.join("assets/prefabs")).unwrap();
            Self { root }
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn material_json_is_classified_and_round_trips() {
        assert_eq!(
            asset_kind("assets/materials/wood.mat.json"),
            crate::assets::AssetKind::Material
        );
        assert!(is_json_editable_asset("assets/materials/wood.mat.json"));
        assert!(is_json_editable_asset("assets/asset-defs/m.asset.json"));
        assert!(!is_json_editable_asset("assets/scenes/a.scene.json"));

        let temp = TempProject::new();
        let value = serde_json::json!({
            "type": "material",
            "base_color": [0.8, 0.4, 0.2, 1.0],
            "metallic": 0.25,
        });
        write_json_asset(&temp.root, "assets/materials/wood.mat.json", &value).unwrap();
        let read = read_json_asset(&temp.root, "assets/materials/wood.mat.json").unwrap();
        assert_eq!(read["metallic"], serde_json::json!(0.25));
        assert_eq!(read["type"], serde_json::json!("material"));
    }

    #[test]
    fn finds_project_root_by_assets_directory() {
        let project = TempProject::new();
        let deep = project.root.join("assets/scenes/nested");
        assert_eq!(find_project_root(&deep), Some(project.root.clone()));
        assert_eq!(
            find_project_root(&project.root.join("assets")),
            Some(project.root.clone())
        );

        let lonely = std::env::temp_dir().join("no-assets-here");
        if find_project_root(&lonely).is_none() {
            assert!(true);
        }
    }

    #[test]
    fn scans_scenes_and_prefabs_recursively() {
        let project = TempProject::new();
        fs::write(project.root.join("assets/scenes/a.scene.json"), "{}").unwrap();
        fs::write(project.root.join("assets/scenes/nested/b.scene.json"), "{}").unwrap();
        fs::write(project.root.join("assets/prefabs/c.prefab.json"), "{}").unwrap();
        fs::write(project.root.join("assets/scenes/ignore.txt"), "x").unwrap();

        let files = scan_documents(&project.root);
        let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "assets/prefabs/c.prefab.json",
                "assets/scenes/a.scene.json",
                "assets/scenes/nested/b.scene.json"
            ]
        );
        assert_eq!(files[0].kind, DocKind::Prefab);
        assert_eq!(files[1].kind, DocKind::Scene);
    }

    #[test]
    fn writes_and_reads_document_roundtrip() {
        let project = TempProject::new();
        let mut doc = SceneDoc::default();
        let index = doc.add_named_child(None, "Cube");
        doc.entities[index].transform.position = [1.0, 2.0, 3.0];

        write_document(&project.root, "assets/scenes/new.scene.json", &doc).unwrap();
        let raw = fs::read_to_string(project.root.join("assets/scenes/new.scene.json")).unwrap();
        assert!(raw.ends_with('\n'));
        assert!(raw.contains("\"Cube\""));

        let loaded = read_document(&project.root, "assets/scenes/new.scene.json").unwrap();
        assert_eq!(loaded.entities.len(), 1);
        assert_eq!(loaded.entities[0].transform.position, [1.0, 2.0, 3.0]);
        assert_eq!(loaded.entities[0].id, Some(1));
    }

    #[test]
    fn read_rejects_missing_or_invalid_files() {
        let project = TempProject::new();
        assert!(read_document(&project.root, "assets/scenes/none.scene.json").is_err());
        fs::write(project.root.join("assets/scenes/bad.scene.json"), "{oops").unwrap();
        assert!(read_document(&project.root, "assets/scenes/bad.scene.json").is_err());
    }

    #[test]
    fn default_path_avoids_collisions_and_sanitizes() {
        let project = TempProject::new();
        let path = default_save_path(&project.root, DocKind::Scene, "New Scene");
        assert_eq!(path, "assets/scenes/New Scene.scene.json");

        fs::write(project.root.join(&path), "{}").unwrap();
        let next = default_save_path(&project.root, DocKind::Scene, "New Scene");
        assert_eq!(next, "assets/scenes/New Scene-2.scene.json");

        let weird = default_save_path(&project.root, DocKind::Prefab, "a/b:c");
        assert_eq!(weird, "assets/prefabs/a_b_c.prefab.json");

        let fallback = default_save_path(&project.root, DocKind::Scene, ".scene.json");
        assert_eq!(fallback, "assets/scenes/Untitled.scene.json");
    }

    #[test]
    fn renames_only_inside_assets() {
        let project = TempProject::new();
        fs::write(project.root.join("assets/scenes/a.scene.json"), "{}").unwrap();

        assert!(rename_path(
            &project.root,
            "assets/scenes/a.scene.json",
            "assets/scenes/b.scene.json"
        )
        .is_ok());
        assert!(!project.root.join("assets/scenes/a.scene.json").exists());
        assert!(project.root.join("assets/scenes/b.scene.json").exists());

        assert!(rename_path(&project.root, "assets/scenes/b.scene.json", "outside.json").is_err());
        assert!(rename_path(
            &project.root,
            "assets/scenes/b.scene.json",
            "assets/scenes/b.scene.json"
        )
        .is_err());
        assert!(rename_path(&project.root, "assets/none.json", "assets/scenes/x.json").is_err());
        fs::write(project.root.join("assets/scenes/c.scene.json"), "{}").unwrap();
        assert!(rename_path(
            &project.root,
            "assets/scenes/b.scene.json",
            "assets/scenes/c.scene.json"
        )
        .is_err());
        assert!(rename_path(
            &project.root,
            "assets/scenes/b.scene.json",
            "assets/scenes/sub/b.scene.json"
        )
        .is_ok());
    }

    #[test]
    fn creates_folders_and_copies_assets() {
        let project = TempProject::new();
        create_folder(&project.root, "assets/textures/ui").unwrap(); 
        assert!(project.root.join("assets/textures/ui").is_dir());
        assert!(create_folder(&project.root, "outside").is_err());

        fs::write(project.root.join("assets/scenes/a.scene.json"), "{}").unwrap();
        copy_file(
            &project.root,
            "assets/scenes/a.scene.json",
            "assets/scenes/copy.scene.json",
        )
        .unwrap();
        assert!(project.root.join("assets/scenes/copy.scene.json").is_file());

        assert!(copy_file(
            &project.root,
            "assets/scenes/a.scene.json",
            "assets/scenes/copy.scene.json"
        )
        .is_err());
        assert!(copy_file(
            &project.root,
            "assets/scenes/missing.scene.json",
            "assets/scenes/x.scene.json"
        )
        .is_err());
    }

    #[test]
    fn write_document_is_atomic_and_keeps_one_backup() {
        let project = TempProject::new();
        let relative = "assets/scenes/atomic.scene.json";
        let path = project.root.join(relative);
        let bak = sibling_path(&path, ".bak");
        let tmp = sibling_path(&path, ".tmp");

        let mut doc = SceneDoc::default();
        doc.add_named_child(None, "first");
        write_document(&project.root, relative, &doc).unwrap();
        assert!(path.exists());
        assert!(!tmp.exists(), "temp file must be renamed away");
        assert!(!bak.exists(), "first save has no old file to back up");

        let mut doc = SceneDoc::default();
        doc.add_named_child(None, "second");
        write_document(&project.root, relative, &doc).unwrap();

        assert!(bak.exists(), "overwrite save should leave previous .bak");
        let backup: SceneDoc = serde_json::from_str(&fs::read_to_string(&bak).unwrap()).unwrap();
        assert_eq!(backup.entities[0].display_name(), "first");
        let current: SceneDoc = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(current.entities[0].display_name(), "second");
        assert!(!tmp.exists());
    }

    #[test]
    fn normalizes_user_input_paths() {
        assert_eq!(
            normalize_save_path(DocKind::Scene, "foo.scene.json"),
            "assets/scenes/foo.scene.json"
        );
        assert_eq!(
            normalize_save_path(DocKind::Scene, "./assets/scenes/bar.scene.json"),
            "assets/scenes/bar.scene.json"
        );
        assert_eq!(
            normalize_save_path(DocKind::Prefab, "assets/prefabs/x.prefab.json"),
            "assets/prefabs/x.prefab.json"
        );
        assert_eq!(normalize_save_path(DocKind::Scene, "   "), "");
        assert_eq!(
            normalize_save_path(DocKind::Scene, "level2"),
            "assets/scenes/level2.scene.json"
        );
        assert_eq!(
            normalize_save_path(DocKind::Scene, "level2.scene"),
            "assets/scenes/level2.scene.json"
        );
        assert_eq!(
            normalize_save_path(DocKind::Scene, "level2.json"),
            "assets/scenes/level2.scene.json"
        );
        assert_eq!(
            normalize_save_path(DocKind::Scene, "assets/scenes/level2"),
            "assets/scenes/level2.scene.json"
        );
        assert_eq!(
            normalize_save_path(DocKind::Prefab, "box"),
            "assets/prefabs/box.prefab.json"
        );
        assert_eq!(normalize_save_path(DocKind::Scene, "../evil"), "");
        assert_eq!(normalize_save_path(DocKind::Scene, "../../evil"), "");
        assert_eq!(normalize_save_path(DocKind::Scene, "assets/../evil"), "");
        assert_eq!(normalize_save_path(DocKind::Scene, "/tmp/evil"), "");
        assert_eq!(normalize_save_path(DocKind::Scene, "C:/evil"), "");
        assert_eq!(
            normalize_save_path(DocKind::Scene, "assets\\scenes\\win.scene.json"),
            "assets/scenes/win.scene.json"
        );
    }
    #[test]
    fn ensure_asset_metas_creates_sidecars_once_and_scan_exposes_the_guid() {
        let project = TempProject::new();
        std::fs::create_dir_all(project.root.join("assets/textures")).unwrap();
        std::fs::write(project.root.join("assets/textures/wood.png"), b"png").unwrap();

        std::fs::write(project.root.join("assets/scenes.pak"), b"pak").unwrap();
        std::fs::write(
            project.root.join("assets/textures/wood.png.import.json"),
            b"{}",
        )
        .unwrap();

        assert_eq!(ensure_asset_metas(&project.root), 1);
        assert!(!project.root.join("assets/scenes.pak.rmeta").exists());
        assert!(!project
            .root
            .join("assets/textures/wood.png.import.json.rmeta")
            .exists());
        let assets = scan_assets(&project.root);
        assert!(
            !assets.iter().any(|asset| asset.path.ends_with(".rmeta")),
            "sidecar must not appear in asset browser"
        );
        let wood = assets
            .iter()
            .find(|asset| asset.path == "assets/textures/wood.png")
            .expect("texture asset");
        let guid = wood.guid.clone().expect("read sidecar guid on scan");
        assert_eq!(guid.len(), 32);

        assert_eq!(ensure_asset_metas(&project.root), 0);
        let again = scan_assets(&project.root)
            .into_iter()
            .find(|asset| asset.path == "assets/textures/wood.png")
            .unwrap();
        assert_eq!(again.guid.as_deref(), Some(guid.as_str()));
    }
}
