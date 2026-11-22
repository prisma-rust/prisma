//! Asset browser data layer (the `AssetBrowser.vue` / `editor_asset_*` equivalent).
//!
//! Input is a flat path list under `assets/` (`AssetInfo`); this is responsible for:
//! - building the directory tree (two columns of the `Project` panel: tree + list);
//! - lookup by path / category / importer;
//! - category colors and volume formatting.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    Model,
    Texture,
    Env,
    Scene,
    Prefab,
    Package,
    Material,
    Data,
    Audio,
    /// Engine-bundled fonts (`.ttf` / `.otf` / `.ttc`, #583).
    Font,
    Other,
}

impl AssetKind {
    pub fn label(self) -> &'static str {
        match self {
            AssetKind::Model => "model",
            AssetKind::Texture => "texture",
            AssetKind::Env => "environment",
            AssetKind::Scene => "scene",
            AssetKind::Prefab => "prefab",
            AssetKind::Package => "package",
            AssetKind::Material => "material",
            AssetKind::Data => "data",
            AssetKind::Audio => "audio",
            AssetKind::Font => "font",
            AssetKind::Other => "other",
        }
    }

    pub fn parse(text: &str) -> AssetKind {
        match text {
            "model" => AssetKind::Model,
            "texture" => AssetKind::Texture,
            "env" => AssetKind::Env,
            "scene" => AssetKind::Scene,
            "prefab" => AssetKind::Prefab,
            "package" => AssetKind::Package,
            "material" => AssetKind::Material,
            "data" => AssetKind::Data,
            "audio" => AssetKind::Audio,
            "font" => AssetKind::Font,
            _ => AssetKind::Other,
        }
    }

    /// Consistent with `AssetBrowser.vue`'s category colors (used by `--accent-alt` etc.).
    pub fn is_text_like(self) -> bool {
        matches!(
            self,
            AssetKind::Scene | AssetKind::Prefab | AssetKind::Data | AssetKind::Material
        )
    }

    pub fn all() -> [AssetKind; 11] {
        [
            AssetKind::Model,
            AssetKind::Texture,
            AssetKind::Env,
            AssetKind::Scene,
            AssetKind::Prefab,
            AssetKind::Package,
            AssetKind::Material,
            AssetKind::Data,
            AssetKind::Audio,
            AssetKind::Font,
            AssetKind::Other,
        ]
    }
}

/// Whether a component field is an asset path (the inspector shows a "select asset" button).
///
/// Kept consistent with `prism-editor-api`'s `ASSET_PATH_FIELDS` judgment:
/// `*_path` suffix plus a few suffix-less historical fields (`env_map` / `volume` / `prefab`).
pub fn is_asset_path_field(component: &str, field: &str) -> bool {
    let leaf = field.rsplit('.').next().unwrap_or(field);
    leaf.ends_with("_path")
        || matches!(leaf, "env_map" | "volume" | "prefab" | "font")
        || (component.ends_with("PrefabInstance") && leaf == "prefab")
}

/// The sibling `*_guid` field for an asset-path field (#466).
///
/// Follows the same naming as `prism_scene::references`: `*_path` → `*_guid`,
/// `*_paths` → `*_guids`, and the non-`_path` references get a `_guid` suffix.
pub fn guid_field_for_path(component: &str, field: &str) -> Option<String> {
    let leaf = field.rsplit('.').next().unwrap_or(field);
    let leaf = leaf.split('[').next().unwrap_or(leaf);
    if !is_asset_path_field(component, leaf) && leaf != "material_paths" {
        return None;
    }
    if let Some(base) = leaf.strip_suffix("_paths") {
        Some(format!("{base}_guids"))
    } else if let Some(base) = leaf.strip_suffix("_path") {
        Some(format!("{base}_guid"))
    } else {
        Some(format!("{leaf}_guid"))
    }
}

/// Parse one reference in a document into a path in the asset table.
///
/// A project reference has three valid forms (see #465), all must be verifiable:
/// - `sprites/water.png` — relative to the `assets/` root (the **most common** form, no `assets/`
///   prefix);
/// - `assets/textures/wood.png` — prefixed project path;
/// - `../textures/wood.png` — relative to the **document's directory**;
/// - `wood.png` — bare file name, matched by "unique same-named file".
///
/// Returns `None` when the reference is broken: file deleted, renamed, or misspelled. The caller
/// uses this to surface a visible hint (inspector `Missing` marker / hierarchy tree warning icon,
/// see #467).
pub fn resolve_reference<'a>(
    assets: &'a [String],
    doc_dir: &str,
    reference: &str,
) -> Option<&'a str> {
    let reference = reference.trim();
    if reference.is_empty() {
        return None;
    }
    // Absolute path / drive letter: not a project asset, don't guess.
    if reference.starts_with('/') || reference.contains(':') {
        return None;
    }
    let exact = |path: &str| {
        assets
            .iter()
            .find(|asset| asset.as_str() == path)
            .map(String::as_str)
    };

    if reference.starts_with("assets/") {
        return exact(reference);
    }
    if reference.starts_with("./") || reference.starts_with("../") {
        let joined = if doc_dir.is_empty() {
            reference.to_string()
        } else {
            format!("{doc_dir}/{reference}")
        };
        return normalize_segments(&joined).and_then(|path| exact(&path));
    }
    // Relative to the `assets/` root: `sprites/water.png` / `textures/wood.png`.
    if let Some(hit) = exact(&format!("assets/{reference}")) {
        return Some(hit);
    }
    // Bare file name: same-named assets must be unique, otherwise don't guess.
    let mut hits = assets
        .iter()
        .filter(|asset| file_name(asset) == reference)
        .map(String::as_str);
    let first = hits.next()?;
    hits.next().is_none().then_some(first)
}

/// Drop `.` / `..` segments; `..` out of bounds (running out of `assets/`) returns `None`.
fn normalize_segments(path: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            other => out.push(other),
        }
    }
    (!out.is_empty()).then(|| out.join("/"))
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetInfo {
    pub path: String,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default = "other_kind")]
    pub kind: AssetKind,
    #[serde(default)]
    pub importer: String,
    /// Stable GUID from the asset sidecar (`<path>.rmeta`, #466). Callers without a sidecar /
    /// older callers get `None`; the details panel falls back to a path hash.
    #[serde(default)]
    pub guid: Option<String>,
}

fn other_kind() -> AssetKind {
    AssetKind::Other
}

impl AssetInfo {
    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    pub fn folder(&self) -> &str {
        self.path
            .rsplit_once('/')
            .map(|(folder, _)| folder)
            .unwrap_or("")
    }

    pub fn extension(&self) -> &str {
        self.file_name()
            .rsplit_once('.')
            .map(|(_, extension)| extension)
            .unwrap_or("")
    }
}

pub fn parse_assets(value: &serde_json::Value) -> Vec<AssetInfo> {
    serde_json::from_value(value.clone()).unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssetNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub kind: Option<AssetKind>,
    pub size_bytes: u64,
    pub children: Vec<AssetNode>,
}

impl AssetNode {
    pub fn child_names(&self) -> Vec<&str> {
        self.children
            .iter()
            .map(|child| child.name.as_str())
            .collect()
    }

    pub fn find(&self, path: &str) -> Option<&AssetNode> {
        if self.path == path {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(path))
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AssetTree {
    pub roots: Vec<AssetNode>,
    pub count: usize,
}

impl AssetTree {
    pub fn build(assets: &[AssetInfo]) -> Self {
        // path -> child nodes (directories use BTreeMap for stable order: dirs first, then
        // same category by name).
        #[derive(Default)]
        struct Dir {
            name: String,
            path: String,
            dirs: BTreeMap<String, Dir>,
            files: Vec<AssetInfo>,
        }

        let mut root = Dir::default();
        for asset in assets {
            let mut parts: Vec<&str> = asset
                .path
                .split('/')
                .filter(|part| !part.is_empty())
                .collect();
            // `assets/` is the project asset root, not shown as a collapsible directory.
            if parts.first() == Some(&"assets") {
                parts.remove(0);
            }
            if parts.is_empty() {
                continue;
            }
            let mut current = &mut root;
            for (depth, part) in parts.iter().enumerate() {
                let is_file = depth + 1 == parts.len();
                if is_file {
                    current.files.push(asset.clone());
                } else {
                    let path = format!("assets/{}", parts[..=depth].join("/"));
                    current = current.dirs.entry(part.to_string()).or_insert_with(|| Dir {
                        name: part.to_string(),
                        path,
                        ..Default::default()
                    });
                }
            }
        }

        fn convert(dir: Dir) -> Vec<AssetNode> {
            let mut nodes: Vec<AssetNode> = dir
                .dirs
                .into_values()
                .map(|dir| AssetNode {
                    name: dir.name.clone(),
                    path: dir.path.clone(),
                    is_dir: true,
                    kind: None,
                    size_bytes: 0,
                    children: convert(dir),
                })
                .collect();
            let mut files: Vec<AssetNode> = dir
                .files
                .into_iter()
                .map(|asset| AssetNode {
                    name: asset.file_name().to_string(),
                    path: asset.path.clone(),
                    is_dir: false,
                    kind: Some(asset.kind),
                    size_bytes: asset.size_bytes,
                    children: Vec::new(),
                })
                .collect();
            files.sort_by_key(|a| a.name.to_lowercase());
            nodes.extend(files);
            nodes
        }

        Self {
            roots: convert(root),
            count: assets.len(),
        }
    }

    pub fn find(&self, path: &str) -> Option<&AssetNode> {
        self.roots.iter().find_map(|root| root.find(path))
    }

    /// Directory list (directories directly under `assets/`).
    pub fn top_level(&self) -> Vec<&AssetNode> {
        self.roots.iter().filter(|node| node.is_dir).collect()
    }

    /// Loose files under the asset root (`assets/settings.ini` etc.).
    pub fn root_files(&self) -> Vec<&AssetNode> {
        self.roots.iter().filter(|node| !node.is_dir).collect()
    }

    /// Direct children of a directory (`None` = root).
    pub fn children_of(&self, folder: Option<&str>) -> Vec<&AssetNode> {
        match folder {
            None | Some("assets") | Some("") => self.roots.iter().collect(),
            Some(folder) => self
                .find(folder)
                .map(|node| node.children.iter().collect())
                .unwrap_or_default(),
        }
    }

    /// Depth-first rows (directory tree panel).
    pub fn visible_rows<'a>(
        &'a self,
        expanded: &std::collections::HashSet<String>,
    ) -> Vec<(&'a AssetNode, u32)> {
        fn walk<'a>(
            nodes: &'a [AssetNode],
            depth: u32,
            expanded: &std::collections::HashSet<String>,
            out: &mut Vec<(&'a AssetNode, u32)>,
        ) {
            for node in nodes {
                out.push((node, depth));
                if node.is_dir && expanded.contains(&node.path) {
                    walk(&node.children, depth + 1, expanded, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.roots, 0, expanded, &mut out);
        out
    }

    /// Insert an empty directory node (`scan_assets` only returns files, empty directories need
    /// to be added separately).
    pub fn insert_dir(&mut self, path: &str) {
        let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        let Some(start) = parts.iter().position(|part| *part != "assets") else {
            return;
        };
        let mut nodes = &mut self.roots;
        let mut current = String::from("assets");
        for part in &parts[start..] {
            current.push('/');
            current.push_str(part);
            let position = nodes
                .iter()
                .position(|node| node.is_dir && node.path == current);
            let index = if let Some(position) = position {
                position
            } else {
                let node = AssetNode {
                    name: (*part).to_string(),
                    path: current.clone(),
                    is_dir: true,
                    kind: None,
                    size_bytes: 0,
                    children: Vec::new(),
                };
                let first_file = nodes
                    .iter()
                    .position(|node| !node.is_dir)
                    .unwrap_or(nodes.len());
                let key = part.to_lowercase();
                let insert_at = nodes[..first_file]
                    .iter()
                    .position(|node| node.name.to_lowercase() > key)
                    .unwrap_or(first_file);
                nodes.insert(insert_at, node);
                insert_at
            };
            nodes = &mut nodes[index].children;
        }
    }

    /// All files under a directory (recursive).
    pub fn files_under<'a>(&'a self, folder: Option<&str>) -> Vec<&'a AssetNode> {
        let mut out = Vec::new();
        let mut stack: Vec<&AssetNode> = match folder {
            None | Some("assets") | Some("") => self.roots.iter().collect(),
            Some(folder) => self
                .find(folder)
                .map(|node| node.children.iter().collect())
                .unwrap_or_default(),
        };
        while let Some(node) = stack.pop() {
            if node.is_dir {
                stack.extend(node.children.iter());
            } else {
                out.push(node);
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }
}

/// Stable asset GUID (32-bit hex, same shape as the guid in `.meta`).
///
/// FNV-1a 64-bit hash of the path: no uuid dependency, stable across processes for the same path.
pub fn asset_guid(path: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in path.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}{hash:016x}")
}

/// Generate a non-conflicting path when copying an asset: `rock.png` → `rock-2.png`.
///
/// The double suffix of `.scene.json` / `.prefab.json` is kept as a whole (`a-2.scene.json`),
/// not producing `a.scene-2.json`.
pub fn unique_asset_path(assets: &[AssetInfo], path: &str) -> String {
    let taken: std::collections::HashSet<&str> =
        assets.iter().map(|asset| asset.path.as_str()).collect();
    if !taken.contains(path) {
        return path.to_string();
    }
    let (stem, suffix) = split_asset_suffix(path);
    for index in 2..1000 {
        let candidate = format!("{stem}-{index}{suffix}");
        if !taken.contains(candidate.as_str()) {
            return candidate;
        }
    }
    format!("{stem}-copy{suffix}")
}

fn split_asset_suffix(path: &str) -> (&str, &str) {
    for suffix in [".scene.json", ".prefab.json"] {
        if let Some(stem) = path.strip_suffix(suffix) {
            return (stem, suffix);
        }
    }
    match path.rsplit_once('.') {
        Some((stem, _extension)) if !stem.is_empty() => (stem, &path[stem.len()..]),
        _ => (path, ""),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AssetPreview {
    Text {
        text: String,
        truncated: bool,
    },
    Image {
        width: Option<u32>,
        height: Option<u32>,
        bytes: u64,
    },
    Document,
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssetReference {
    pub document: String,
    pub entity: u32,
    pub entity_name: String,
    pub field: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssetDetails {
    pub asset: AssetInfo,
    pub guid: String,
    pub preview: AssetPreview,
    pub references: Vec<AssetReference>,
}

/// Read a text preview (at most `max_bytes`, truncated at UTF-8 boundary).
pub fn text_preview(
    root: &std::path::Path,
    relative: &str,
    max_bytes: usize,
) -> Option<(String, bool)> {
    let bytes = std::fs::read(root.join(relative)).ok()?;
    let truncated = bytes.len() > max_bytes;
    let slice = &bytes[..bytes.len().min(max_bytes)];
    let mut text = String::from_utf8_lossy(slice).to_string();
    while !text.is_char_boundary(text.len()) {
        text.pop();
    }
    Some((text, truncated))
}

/// Image dimensions: supports PNG / GIF / BMP / JPEG (reads only the file header, no decode
/// library introduced).
pub fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.len() >= 24 {
        let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
        let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
        return Some((width, height));
    }
    if (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) && bytes.len() >= 10 {
        let width = u16::from_le_bytes(bytes[6..8].try_into().ok()?) as u32;
        let height = u16::from_le_bytes(bytes[8..10].try_into().ok()?) as u32;
        return Some((width, height));
    }
    if bytes.starts_with(b"BM") && bytes.len() >= 26 {
        let width = i32::from_le_bytes(bytes[18..22].try_into().ok()?) as u32;
        let height = i32::from_le_bytes(bytes[22..26].try_into().ok()?).unsigned_abs();
        return Some((width, height));
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        let mut offset = 2;
        while offset + 9 < bytes.len() {
            if bytes[offset] != 0xff {
                offset += 1;
                continue;
            }
            let marker = bytes[offset + 1];
            let length = u16::from_be_bytes([bytes[offset + 2], bytes[offset + 3]]) as usize;
            let is_sof = matches!(
                marker,
                0xc0 | 0xc1
                    | 0xc2
                    | 0xc3
                    | 0xc5
                    | 0xc6
                    | 0xc7
                    | 0xc9
                    | 0xca
                    | 0xcb
                    | 0xcd
                    | 0xce
                    | 0xcf
            );
            if is_sof && offset + 9 < bytes.len() {
                let height = u16::from_be_bytes([bytes[offset + 5], bytes[offset + 6]]) as u32;
                let width = u16::from_be_bytes([bytes[offset + 7], bytes[offset + 8]]) as u32;
                return Some((width, height));
            }
            offset += 2 + length;
        }
    }
    None
}

pub fn asset_preview(
    root: Option<&std::path::Path>,
    asset: &AssetInfo,
    max_text_bytes: usize,
) -> AssetPreview {
    if asset.kind == AssetKind::Scene || asset.kind == AssetKind::Prefab {
        return AssetPreview::Document;
    }
    let Some(root) = root else {
        return AssetPreview::None;
    };
    if asset.kind == AssetKind::Texture || asset.kind == AssetKind::Env {
        let bytes = std::fs::read(root.join(&asset.path)).unwrap_or_default();
        let dimensions = image_dimensions(&bytes);
        return AssetPreview::Image {
            width: dimensions.map(|(width, _)| width),
            height: dimensions.map(|(_, height)| height),
            bytes: bytes.len() as u64,
        };
    }
    if asset.kind.is_text_like() || asset.kind == AssetKind::Audio || asset.kind == AssetKind::Other
    {
        if let Some((text, truncated)) = text_preview(root, &asset.path, max_text_bytes) {
            if !text.contains('\u{0}') {
                return AssetPreview::Text { text, truncated };
            }
        }
    }
    AssetPreview::None
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AssetQuery {
    pub search: String,
    pub kinds: Vec<AssetKind>,
}

impl AssetQuery {
    pub fn matches(&self, asset: &AssetInfo) -> bool {
        if !self.kinds.is_empty() && !self.kinds.contains(&asset.kind) {
            return false;
        }
        if self.search.is_empty() {
            return true;
        }
        asset
            .path
            .to_ascii_lowercase()
            .contains(&self.search.to_ascii_lowercase())
    }
}

pub fn filter_assets<'a>(assets: &'a [AssetInfo], query: &AssetQuery) -> Vec<&'a AssetInfo> {
    assets.iter().filter(|asset| query.matches(asset)).collect()
}

/// Human-readable size (consistent with `format.ts::formatBytes`: 1024-based, 1 decimal).
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<AssetInfo> {
        vec![
            AssetInfo {
                path: "assets/scenes/default.scene.json".into(),
                size_bytes: 2048,
                kind: AssetKind::Scene,
                importer: "DefaultImporter".into(),
                guid: None,
            },
            AssetInfo {
                path: "assets/scenes/rocket.scene.json".into(),
                size_bytes: 1024,
                kind: AssetKind::Scene,
                importer: "DefaultImporter".into(),
                guid: None,
            },
            AssetInfo {
                path: "assets/prefabs/camera.prefab.json".into(),
                size_bytes: 512,
                kind: AssetKind::Prefab,
                importer: "DefaultImporter".into(),
                guid: None,
            },
            AssetInfo {
                path: "assets/textures/brick.png".into(),
                size_bytes: 5_242_880,
                kind: AssetKind::Texture,
                importer: "ImageImporter".into(),
                guid: None,
            },
            AssetInfo {
                path: "assets/settings.ini".into(),
                size_bytes: 64,
                kind: AssetKind::Data,
                importer: "TextImporter".into(),
                guid: None,
            },
        ]
    }

    #[test]
    fn builds_directory_tree_with_dirs_first() {
        let tree = AssetTree::build(&sample());
        assert_eq!(tree.count, 5);
        let top: Vec<&str> = tree
            .top_level()
            .iter()
            .map(|node| node.name.as_str())
            .collect();
        assert_eq!(top, vec!["prefabs", "scenes", "textures"]);
        let root_files: Vec<&str> = tree
            .root_files()
            .iter()
            .map(|node| node.name.as_str())
            .collect();
        assert_eq!(root_files, vec!["settings.ini"]);

        let scenes = tree.find("assets/scenes").expect("scenes dir");
        let names: Vec<&str> = scenes.child_names();
        assert_eq!(names, vec!["default.scene.json", "rocket.scene.json"]);

        // Root directory files are also in the root node.
        assert!(tree.find("assets/settings.ini").is_some());
    }

    #[test]
    fn visible_rows_respect_expansion() {
        let tree = AssetTree::build(&sample());
        let mut expanded = std::collections::HashSet::new();
        assert_eq!(
            tree.visible_rows(&expanded).len(),
            4,
            "3 directories + 1 root file"
        );
        assert_eq!(tree.children_of(Some("assets")).len(), 4);

        expanded.insert("assets/scenes".to_string());
        let rows = tree.visible_rows(&expanded);
        assert_eq!(rows.len(), 6);
        assert!(rows
            .iter()
            .any(|(node, depth)| node.name == "rocket.scene.json" && *depth == 1));
    }

    #[test]
    fn files_under_is_recursive_and_sorted() {
        let tree = AssetTree::build(&sample());
        let assets = tree.files_under(Some("assets"));
        assert_eq!(assets.len(), 5);
        assert_eq!(assets[0].path, "assets/prefabs/camera.prefab.json");

        let scenes = tree.files_under(Some("assets/scenes"));
        assert_eq!(scenes.len(), 2);
    }

    #[test]
    fn query_filters_by_kind_and_search() {
        let assets = sample();
        let mut query = AssetQuery::default();
        assert_eq!(filter_assets(&assets, &query).len(), 5);

        query.search = "SCENE".into();
        let hits = filter_assets(&assets, &query);
        assert_eq!(hits.len(), 2, "search is case-insensitive");

        query.search.clear();
        query.kinds.push(AssetKind::Texture);
        let hits = filter_assets(&assets, &query);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].file_name(), "brick.png");
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(5_242_880), "5.0 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn asset_metadata_helpers() {
        let asset = &sample()[0];
        assert_eq!(asset.file_name(), "default.scene.json");
        assert_eq!(asset.folder(), "assets/scenes");
        assert_eq!(asset.extension(), "json");
        assert!(AssetKind::Scene.is_text_like());
        assert!(!AssetKind::Texture.is_text_like());
        assert_eq!(AssetKind::parse("texture"), AssetKind::Texture);
        assert_eq!(AssetKind::parse("weird"), AssetKind::Other);
    }

    #[test]
    fn guid_is_stable_and_path_specific() {
        let first = asset_guid("assets/rock.png");
        assert_eq!(first.len(), 32);
        assert_eq!(first, asset_guid("assets/rock.png"));
        assert_ne!(first, asset_guid("assets/rock2.png"));
    }

    #[test]
    fn unique_asset_path_keeps_document_suffix() {
        let assets = sample();
        assert_eq!(
            unique_asset_path(&assets, "assets/scenes/new.scene.json"),
            "assets/scenes/new.scene.json"
        );
        assert_eq!(
            unique_asset_path(&assets, "assets/scenes/default.scene.json"),
            "assets/scenes/default-2.scene.json"
        );
        assert_eq!(
            unique_asset_path(&assets, "assets/textures/brick.png"),
            "assets/textures/brick-2.png"
        );
    }

    #[test]
    fn reads_image_dimensions_from_headers() {
        let mut png = vec![0_u8; 24];
        png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        png[16..20].copy_from_slice(&640_u32.to_be_bytes());
        png[20..24].copy_from_slice(&480_u32.to_be_bytes());
        assert_eq!(image_dimensions(&png), Some((640, 480)));

        let mut gif = vec![0_u8; 10];
        gif[..6].copy_from_slice(b"GIF89a");
        gif[6..8].copy_from_slice(&32_u16.to_le_bytes());
        gif[8..10].copy_from_slice(&16_u16.to_le_bytes());
        assert_eq!(image_dimensions(&gif), Some((32, 16)));
        assert_eq!(image_dimensions(b"not an image"), None);
    }

    #[test]
    fn text_preview_truncates_on_char_boundary() {
        let root = std::env::temp_dir().join(format!(
            "prism-asset-preview-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/note.txt"), "hello world").unwrap();
        let (text, truncated) = text_preview(&root, "assets/note.txt", 5).unwrap();
        assert_eq!(text, "hello");
        assert!(truncated);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn paths() -> Vec<String> {
        vec![
            "assets/textures/wood.png".to_string(),
            "assets/scenes/level.scene.json".to_string(),
            "assets/prefabs/box.prefab.json".to_string(),
            "assets/a/shared.png".to_string(),
            "assets/b/shared.png".to_string(),
        ]
    }

    #[test]
    fn references_resolve_in_all_three_writing_styles() {
        let assets = paths();
        // Relative to project root
        assert_eq!(
            resolve_reference(&assets, "assets/scenes", "assets/textures/wood.png"),
            Some("assets/textures/wood.png")
        );
        // Relative to document directory (a form allowed by #465)
        assert_eq!(
            resolve_reference(&assets, "assets/scenes/deep", "../../textures/wood.png"),
            Some("assets/textures/wood.png")
        );
        // Relative to assets/ root (the most common form in the project)
        assert_eq!(
            resolve_reference(&assets, "assets/scenes", "textures/wood.png"),
            Some("assets/textures/wood.png")
        );
        // Bare file name (when unique)
        assert_eq!(
            resolve_reference(&assets, "assets/scenes", "wood.png"),
            Some("assets/textures/wood.png")
        );
    }

    #[test]
    fn a_broken_reference_resolves_to_nothing() {
        let assets = paths();
        assert_eq!(
            resolve_reference(&assets, "assets/scenes", "assets/textures/stone.png"),
            None
        );
        // Can't guess when same-named files aren't unique.
        assert_eq!(resolve_reference(&assets, "assets", "shared.png"), None);
        // Empty value = not set, not a broken reference.
        assert_eq!(resolve_reference(&assets, "assets", "   "), None);
        // Relative path runs out of assets/.
        assert_eq!(resolve_reference(&assets, "assets", "../outside.png"), None);
    }
}
