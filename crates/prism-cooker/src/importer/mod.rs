//! # prism-cooker::importer
//!
//! Prisma asset pipeline importer framework
//!
//! Importer converts source files (`.png`, `.gltf`, `.obj`, `.wav`, etc.) into import-time data,
//! then cooker converts into runtime format.
//!
//! Import pipeline like below:
//!
//! ```text
//! Source file → [Importer] → ImportResult (import-time data)
//!   ↓
//! [AssetDatabase] records already created/updated
//! ```

mod skinned;

use crate::db::{AssetDatabase, AssetRecord, ImportCache};
use glam::{Mat3, Mat4, Vec3};
use prism_asset::core::{AssetId, AssetType};
use prism_asset::formats_skinned::{AnimClipSectionInfo, AnimClipWarpWindowInfo, AnimNotifyInfo};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

use serde_json::Value;

/// Author-side `*.skinning/montages.json` contains Montage data for one clip (#560 P4).
#[derive(Debug, Clone, Default)]
pub struct AuthoredMontage {
    pub sections: Vec<AnimClipSectionInfo>,
    pub warp_windows: Vec<AnimClipWarpWindowInfo>,
}

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("No importer found for: {0}")]
    NoImporter(PathBuf),

    #[error("Importer {0} rejected file: {1}")]
    ImporterRejected(String, PathBuf),

    #[error("Import failed: {0}")]
    ImportFailed(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Database error: {0}")]
    Database(#[from] crate::db::DatabaseError),

    #[error("Serde error: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Context provided to an importer during the import process.
pub struct ImportContext {
    /// Absolute path to the source file being imported.
    pub source_path: PathBuf,
    /// xxh3 hash of the source file contents.
    pub source_hash: u64,
    /// JSON settings passed to the importer.
    pub settings: Value,
    /// Reference to the asset database (for dependency lookups).
    pub db: Arc<AssetDatabase>,
}

impl std::fmt::Debug for ImportContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportContext")
            .field("source_path", &self.source_path)
            .field("source_hash", &format!("{:#x}", self.source_hash))
            .finish()
    }
}

/// The result of a successful import.
pub struct ImportResult {
    /// The type of asset produced.
    pub asset_type: AssetType,
    /// IDs of other assets this one depends on.
    pub dependencies: Vec<AssetId>,
    /// Intermediate binary data (input to the cooker).
    pub output_data: Vec<u8>,
    /// Optional JSON metadata stored alongside the asset.
    pub metadata: Option<Value>,
}

impl std::fmt::Debug for ImportResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportResult")
            .field("asset_type", &self.asset_type)
            .field("deps", &self.dependencies.len())
            .field("data_size", &self.output_data.len())
            .finish()
    }
}

/// Result of a single `import_file` call.
pub struct ImportFileResult {
    /// `true` when the file was actually imported; `false` when cache hit.
    pub was_imported: bool,
    /// When `was_imported == true`, the importer's intermediate output data.
    /// `None` when the file was cached.
    pub intermediate_data: Option<Vec<u8>>,
}

/// A pluggable importer that converts source files into intermediate data.
///
/// Implementations must be `Send + Sync` so they can be registered in a global
/// registry and run on any thread.
pub trait Importer: Send + Sync {
    /// Unique name for this importer (e.g. `"texture-importer"`).
    fn name(&self) -> &'static str;

    /// Version of this importer. Increment when the output format changes
    /// to force re-import.
    fn version(&self) -> u32;

    /// Return `true` if this impporter can handle the given source file.
    fn can_import(&self, path: &Path) -> bool;

    /// Perform the import.
    ///
    /// This may be called on a background thread / async task.
    fn import(&self, ctx: &ImportContext) -> Result<ImportResult, ImportError>;
}

/// Registry of all available importers, keyed by name.
pub struct ImporterRegistry {
    importers: Vec<Box<dyn Importer>>,
    by_name: HashMap<&'static str, usize>,
}

impl ImporterRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            importers: Vec::new(),
            by_name: HashMap::new(),
        }
    }

    /// Register an importer.
    pub fn register(&mut self, importer: Box<dyn Importer>) {
        let name = importer.name();
        let idx = self.importers.len();
        self.importers.push(importer);
        self.by_name.insert(name, idx);
        tracing::info!(
            "Registered importer: {name} v{}",
            self.importers[idx].version()
        );
    }

    /// Number of registered importers.
    pub fn len(&self) -> usize {
        self.importers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.importers.is_empty()
    }

    /// Find an importer by name.
    pub fn get(&self, name: &str) -> Option<&dyn Importer> {
        self.by_name
            .get(name)
            .map(|&idx| self.importers[idx].as_ref())
    }

    /// Find the first importer that can handle a given file.
    pub fn find_for_path(&self, path: &Path) -> Option<&dyn Importer> {
        self.importers
            .iter()
            .find(|imp| imp.can_import(path))
            .map(|b| b.as_ref())
    }

    /// Iterate all registered importers.
    pub fn iter(&self) -> impl Iterator<Item = &dyn Importer> {
        self.importers.iter().map(|b| b.as_ref())
    }
}

impl Default for ImporterRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ImporterRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ImporterRegistry({} importers)", self.importers.len())
    }
}

/// High-level import pipeline that coordinates importers, the database, and
/// the import cache.
pub struct ImportPipeline {
    registry: Arc<ImporterRegistry>,
}

impl ImportPipeline {
    /// Create a new pipeline using the given importer registry.
    pub fn new(registry: Arc<ImporterRegistry>) -> Self {
        Self { registry }
    }

    /// Reference to the underlying registry.
    pub fn registry(&self) -> &ImporterRegistry {
        &self.registry
    }

    /// Import a single file.
    ///
    /// If the file is unchanged (matching hash in import cache), the import
    /// is skipped. Returns [`ImportFileResult`] with `was_imported` indicating
    /// whether the file was actually processed, and `intermediate_data` carrying
    /// the importer's output for downstream cooking.
    pub fn import_file(
        &self,
        source_path: &Path,
        db: &mut AssetDatabase,
        cache: &mut ImportCache,
        settings: Option<Value>,
    ) -> Result<ImportFileResult, ImportError> {
        let normalized = normalize_relative_path(source_path);
        self.import_file_with_key(source_path, &normalized, db, cache, settings, false)
    }

    /// import  a single file under an explicit asset key.
    ///
    /// `relative_key` is the path relative to the project's `Assets/` root.
    /// CLI callers that already know the root should prefer this over
    /// [`Self::import_file`], whose key is derived from the (possibly
    /// absolute) source path and would otherwise duplicate the record.
    ///
    /// When `force` is `true` the import cache is ignored and the importer is
    /// run even if the source hash and settings are unchanged.
    pub fn import_file_with_key(
        &self,
        source_path: &Path,
        relative_key: &str,
        db: &mut AssetDatabase,
        cache: &mut ImportCache,
        settings: Option<Value>,
        force: bool,
    ) -> Result<ImportFileResult, ImportError> {
        let normalized = normalize_relative_path(Path::new(relative_key));
        let data = std::fs::read(source_path)?;
        let hash = xxhash_rust::xxh3::xxh3_64(&data);
        let settings = settings.unwrap_or(Value::Null);
        let settings_hash =
            xxhash_rust::xxh3::xxh3_64(serde_json::to_string(&settings)?.as_bytes());

        let importer = self
            .registry
            .find_for_path(source_path)
            .ok_or_else(|| ImportError::NoImporter(source_path.to_path_buf()))?;

        if !force && cache.is_up_to_date(&normalized, hash, settings_hash, importer.version()) {
            tracing::debug!("  ~ cached: {normalized}");
            return Ok(ImportFileResult {
                was_imported: false,
                intermediate_data: None,
            });
        }

        let ctx = ImportContext {
            source_path: source_path.to_path_buf(),
            source_hash: hash,
            settings,
            db: Arc::new(db.clone()),
        };

        let result = importer.import(&ctx)?;

        let id = db
            .id_by_path(&normalized)
            .unwrap_or_else(|| db.generate_id());
        let mut record =
            AssetRecord::new(id, normalized.clone(), result.asset_type, importer.name());
        record.source_hash = hash;
        record.import_settings_hash = settings_hash;
        record.dependencies = result.dependencies;
        record.version = importer.version();
        db.insert(record)?;

        cache.record(&normalized, hash, settings_hash, id, importer.version());

        Ok(ImportFileResult {
            was_imported: true,
            intermediate_data: Some(result.output_data),
        })
    }

    /// Import all files in a directory tree.
    pub fn import_directory(
        &self,
        dir: &Path,
        db: &mut AssetDatabase,
        cache: &mut ImportCache,
    ) -> ImportSummary {
        let mut summary = ImportSummary::default();
        walk_directory(
            dir,
            &mut |path| match self.import_file(&path, db, cache, None) {
                Ok(r) if r.was_imported => summary.imported += 1,
                Ok(_) => summary.cached += 1,
                Err(ImportError::NoImporter(_)) => summary.skipped += 1,
                Err(e) => {
                    tracing::warn!("  ! {}: {e}", path.display());
                    summary.errors += 1;
                }
            },
        );
        summary
    }
}

/// Summary of an import run.
#[derive(Debug, Default, Clone)]
pub struct ImportSummary {
    pub imported: u32,
    pub cached: u32,
    pub skipped: u32,
    pub errors: u32,
}

/// Imports any unrecognized file as a raw binary blob.
pub struct RawImporter;

impl Importer for RawImporter {
    fn name(&self) -> &'static str {
        "raw-importer"
    }

    fn version(&self) -> u32 {
        1
    }

    fn can_import(&self, _path: &Path) -> bool {
        true
    }

    fn import(&self, ctx: &ImportContext) -> Result<ImportResult, ImportError> {
        let data = std::fs::read(&ctx.source_path)?;
        Ok(ImportResult {
            asset_type: AssetType::Binary,
            dependencies: Vec::new(),
            output_data: data,
            metadata: None,
        })
    }
}

/// Import format tag stored in the intermediate binary data.
const TEXTURE_INTERMEDIATE_MAGIC: &[u8; 4] = b"RTXI";

/// Texture pixel format enum for intermediate storage.
#[repr(u8)]
#[derive(Debug, Clone, Copy)]
enum TexIntermediateFormat {
    Rgba8 = 0,
}

/// Imports image files by decoding them to RGBA8 and storing a standard
/// intermediate representation: `[magic:4][width:4][height:4][channels:1][format:1][pixels:N]`
pub struct TextureImporter;

impl TextureImporter {
    fn write_intermediate(width: u32, height: u32, channels: u8, rgba_pixels: &[u8]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(12 + rgba_pixels.len());
        buf.extend_from_slice(TEXTURE_INTERMEDIATE_MAGIC);
        buf.extend_from_slice(&width.to_le_bytes());
        buf.extend_from_slice(&height.to_le_bytes());
        buf.push(channels);
        buf.push(TexIntermediateFormat::Rgba8 as u8);
        buf.extend_from_slice(rgba_pixels);
        buf
    }

    fn read_raw_pixels(img: &image::DynamicImage) -> (u32, u32, u8, Vec<u8>) {
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        let pixels = rgba.into_raw();
        (w, h, 4, pixels)
    }
}

impl Importer for TextureImporter {
    fn name(&self) -> &'static str {
        "texture-importer"
    }

    fn version(&self) -> u32 {
        2
    }

    fn can_import(&self, path: &Path) -> bool {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        matches!(
            ext.as_str(),
            "png" | "jpg" | "jpeg" | "tga" | "bmp" | "hdr" | "exr"
        )
    }

    fn import(&self, ctx: &ImportContext) -> Result<ImportResult, ImportError> {
        let img = image::open(&ctx.source_path)
            .map_err(|e| ImportError::ImportFailed(format!("Failed to decode image: {e}")))?;

        let (w, h, channels, rgba) = Self::read_raw_pixels(&img);

        let output_data = Self::write_intermediate(w, h, channels, &rgba);

        let ext = ctx
            .source_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("unknown")
            .to_lowercase();

        let metadata = serde_json::json!({
            "original_name": ctx.source_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown"),
            "format": ext,
            "width": w,
            "height": h,
            "channels": channels,
            "decoded_size": output_data.len(),
        });

        Ok(ImportResult {
            asset_type: AssetType::Texture,
            dependencies: Vec::new(),
            output_data,
            metadata: Some(metadata),
        })
    }
}

/// Intermediate mesh format magic: "RMXI" (Resource Mesh Intermediate)
const MESH_INTERMEDIATE_MAGIC: &[u8; 4] = b"RMXI";

/// Imports .gltf / .glb files: all triangle primitives in **entire scene tree** cooked into one
/// world-space mesh asset (node TRS already multiplied into vertices, normals transformed by inverse-transpose matrix).
///
/// If one primitive lacks normals / UVs, entire mesh omits that property (not faking with zero values);
/// Non-indexed primitive will get an ordered index added. Each primitive's index range with source material index written into
/// RMXI v2 submesh table; at runtime drawn per submesh by `MeshRenderer.material_paths`.
/// Material / texture slices in source file can be output as RMAT / RTEX via [`GltfImporter::side_assets`]
/// sidecar assets; material parameters and texture pixels only saved in memory import result, not written into JSON metadata.
///
/// Intermediate format:
/// ```text
/// [magic:4][version:1][vert_count:4][idx_count:4][uv_count:4]
/// [positions: f32*3*vert_count][normals: f32*3*vert_count or empty]
/// [uv0: f32*2*vert_count or empty][indices: u32*idx_count]
/// v2 only: [submesh_count:4][index_start:4][index_count:4][material_index:4]...
/// ```
pub struct GltfImporter;

/// A sidecar asset generated by model importer, already cooked well (#520).
///
/// `path` is project-relative packaging path (also input to `AssetId::from_path`);
/// Material texture ID inside consistent with path written into package.
#[derive(Debug, Clone)]
pub struct ImportedSideAsset {
    /// Project-relative path inside the asset package.
    pub path: String,
    /// Cooked runtime bytes (RMAT for materials, RTEX for textures).
    pub cooked_data: Vec<u8>,
}

/// A glTF skin/animation product path (not encoded, just planned path).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GltfRigPaths {
    /// `.rskel` path: only when source file has skeleton.
    pub skeleton: Option<String>,
    /// `.rskin` path: only when source file has complete JOINTS_0/WEIGHTS_0.
    pub skinned_mesh: Option<String>,
    /// Each glTF animation one `.ranim`, order consistent with source file.
    pub clips: Vec<String>,
}

impl GltfRigPaths {
    /// No skinning info (normal static mesh).
    pub fn is_empty(&self) -> bool {
        self.skeleton.is_none() && self.skinned_mesh.is_none() && self.clips.is_empty()
    }
}

/// Material and texture assets included with glTF/GLB (#520).
#[derive(Debug, Clone, Default)]
pub struct GltfSideAssets {
    /// One material path per glTF material, in source order.
    pub material_paths: Vec<String>,
    /// One material path per mesh submesh, in draw order. Entries fall back to
    /// a generated default material when the submesh has no source material.
    pub submesh_material_paths: Vec<String>,
    /// Cooked side assets to stage at their declared paths.
    pub assets: Vec<ImportedSideAsset>,
    /// `.rskel` path when the source file carries a skeleton (#560).
    pub skeleton_path: Option<String>,
    /// `.rskin` path when the source file carries skin weights (#560).
    pub skinned_mesh_path: Option<String>,
    /// One `.ranim` path per glTF animation, in source order (#560).
    pub clip_paths: Vec<String>,
}

/// Write a set of already-split vertex properties into RMXI intermediate format.
///
/// Shared by glTF and OBJ importers; when `normals` / `uv0` is `None`, property omitted entirely
/// (cooker will fill default values), not writing zero values as fakes.
fn write_mesh_intermediate(
    positions: &[[f32; 3]],
    normals: Option<&[[f32; 3]]>,
    uv0: Option<&[[f32; 2]]>,
    indices: &[u32],
    submeshes: &[SubmeshInfo],
) -> Vec<u8> {
    let verts = positions.len() as u32;
    let idxs = indices.len() as u32;
    let uv_channels: u32 = if uv0.is_some() { 1 } else { 0 };

    let table_len = if submeshes.is_empty() {
        0
    } else {
        4 + submeshes.len() * 12
    };
    let cap = 4
        + 1
        + 4
        + 4
        + 4
        + verts as usize * 3 * 4
        + normals.map_or(0, |n| n.len() * 3 * 4)
        + uv0.map_or(0, |u| u.len() * 2 * 4)
        + idxs as usize * 4
        + table_len;
    let mut buf = Vec::with_capacity(cap);

    buf.extend_from_slice(MESH_INTERMEDIATE_MAGIC);
    buf.push(if submeshes.is_empty() { 1 } else { 2 });
    buf.extend_from_slice(&verts.to_le_bytes());
    buf.extend_from_slice(&idxs.to_le_bytes());
    buf.extend_from_slice(&uv_channels.to_le_bytes());

    for pos in positions {
        buf.extend_from_slice(&pos[0].to_le_bytes());
        buf.extend_from_slice(&pos[1].to_le_bytes());
        buf.extend_from_slice(&pos[2].to_le_bytes());
    }
    if let Some(n) = normals {
        for nrm in n {
            buf.extend_from_slice(&nrm[0].to_le_bytes());
            buf.extend_from_slice(&nrm[1].to_le_bytes());
            buf.extend_from_slice(&nrm[2].to_le_bytes());
        }
    }
    if let Some(uv) = uv0 {
        for t in uv {
            buf.extend_from_slice(&t[0].to_le_bytes());
            buf.extend_from_slice(&t[1].to_le_bytes());
        }
    }
    for idx in indices {
        buf.extend_from_slice(&idx.to_le_bytes());
    }
    if !submeshes.is_empty() {
        buf.extend_from_slice(&(submeshes.len() as u32).to_le_bytes());
        for submesh in submeshes {
            buf.extend_from_slice(&submesh.index_start.to_le_bytes());
            buf.extend_from_slice(&submesh.index_count.to_le_bytes());
            buf.extend_from_slice(
                &submesh
                    .material_index
                    .map_or(u32::MAX, |index| index as u32)
                    .to_le_bytes(),
            );
        }
    }
    buf
}

impl GltfImporter {
    fn read_gltf(path: &Path) -> Result<MeshData, ImportError> {
        let (document, buffers, images) = gltf::import(path)
            .map_err(|e| ImportError::ImportFailed(format!("glTF parse failed: {e}")))?;

        let mut mesh = MeshData {
            material_count: document.materials().count(),
            has_skins: document.skins().next().is_some(),
            ..Default::default()
        };

        let skin_result = skinned::read_skinning(&document, &buffers);
        if !skin_result.skinning.is_empty() {
            tracing::info!(
                "glTF: {} joint(s), {} clip(s)",
                skin_result.skinning.joint_count(),
                skin_result.clips.len()
            );
        }
        mesh.skinning = skin_result.skinning;
        mesh.clips = skin_result.clips;

        mesh.materials = document
            .materials()
            .map(|material| {
                let pbr = material.pbr_metallic_roughness();
                MaterialInfo {
                    name: material.name().map(str::to_string),
                    base_color: pbr.base_color_factor(),
                    metallic: pbr.metallic_factor(),
                    roughness: pbr.roughness_factor(),
                    emissive: material.emissive_factor(),
                    alpha_mode: format!("{:?}", material.alpha_mode()),
                    alpha_cutoff: material.alpha_cutoff(),
                    double_sided: material.double_sided(),
                    base_color_texture: pbr
                        .base_color_texture()
                        .map(|t| t.texture().source().index()),
                    metallic_roughness_texture: pbr
                        .metallic_roughness_texture()
                        .map(|t| t.texture().source().index()),
                    normal_texture: material
                        .normal_texture()
                        .map(|t| t.texture().source().index()),
                    normal_scale: material.normal_texture().map(|t| t.scale()).unwrap_or(1.0),
                    occlusion_texture: material
                        .occlusion_texture()
                        .map(|t| t.texture().source().index()),
                    occlusion_strength: material
                        .occlusion_texture()
                        .map(|t| t.strength())
                        .unwrap_or(1.0),
                    emissive_texture: material
                        .emissive_texture()
                        .map(|t| t.texture().source().index()),
                }
            })
            .collect();
        mesh.image_pixels = images.iter().map(rgba8_from_gltf_image).collect();
        mesh.images = document
            .images()
            .map(|image| {
                let (uri, mime_type, source_bytes) = match image.source() {
                    gltf::image::Source::Uri { uri, mime_type } => {
                        (Some(uri.to_string()), mime_type.map(str::to_string), 0)
                    }
                    gltf::image::Source::View { view, mime_type } => {
                        (None, Some(mime_type.to_string()), view.length())
                    }
                };
                let decoded = images.get(image.index());
                ImageInfo {
                    name: image.name().map(str::to_string),
                    mime_type,
                    uri,
                    width: decoded.map(|data| data.width).unwrap_or(0),
                    height: decoded.map(|data| data.height).unwrap_or(0),
                    source_bytes,
                    format: decoded
                        .map(|data| format!("{:?}", data.format))
                        .unwrap_or_else(|| "unknown".to_string()),
                }
            })
            .collect();

        let mut roots: Vec<gltf::Node> = Vec::new();
        match document
            .default_scene()
            .or_else(|| document.scenes().next())
        {
            Some(scene) => roots.extend(scene.nodes()),
            None => {
                let mut children = std::collections::HashSet::new();
                for node in document.nodes() {
                    for child in node.children() {
                        children.insert(child.index());
                    }
                }
                roots.extend(
                    document
                        .nodes()
                        .filter(|node| !children.contains(&node.index())),
                );
            }
        }

        let parent = Mat4::IDENTITY;
        for node in &roots {
            collect_node(node, &parent, &buffers, &mut mesh);
        }

        if mesh.primitive_count == 0 && !roots.is_empty() {
            return Err(ImportError::ImportFailed(
                "glTF has nodes but no triangle primitives".into(),
            ));
        }
        if mesh.primitive_count == 0 {
            for gltf_mesh in document.meshes() {
                for primitive in gltf_mesh.primitives() {
                    push_primitive(&primitive, &Mat4::IDENTITY, &buffers, None, &mut mesh);
                }
            }
        }
        if mesh.primitive_count == 0 {
            return Err(ImportError::ImportFailed(
                "No triangle primitives found in glTF".into(),
            ));
        }

        if mesh.positions.is_empty() {
            return Err(ImportError::ImportFailed(
                "glTF mesh has no vertices".into(),
            ));
        }
        if !mesh.all_skin_attrs || mesh.joints.len() != mesh.positions.len() {
            mesh.joints.clear();
            mesh.weights.clear();
        }

        Ok(mesh)
    }

    /// Return the material paths a scene should assign to each submesh of a
    /// glTF/GLB mesh, in draw order (#520).
    ///
    /// This parses the source file but does not cook anything, so the scene
    /// packer can rewrite `MeshRenderer.material_paths` before cooking the
    /// scene. Use [`Self::side_assets`] to obtain the actual cooked RMAT/RTEX
    /// blobs.
    pub fn submesh_material_paths(
        path: &Path,
        mesh_pak_path: &str,
    ) -> Result<Vec<String>, ImportError> {
        let mesh = Self::read_gltf(path)?;
        Ok(gltf_material_plan(&mesh, mesh_pak_path).submesh_material_paths)
    }

    /// Read author-side clip event file (#560 P1 Notify).
    ///
    /// Position fixed **next to source file**: `assets/models/hero.glb` should be
    /// `assets/models/hero.glb.skinning/notifies.json`, content is
    ///
    /// ```json
    /// { "clips": { "walk": [{ "time": 0.4, "name": "Footstep.L" }] } }
    /// ```
    ///
    /// Key is glTF animation **original name** (not sanitized file name), because what author sees is this name.
    /// File not present = no events (positive common case); **present but parse failure = build failure**:
    /// Silently ignoring a wrongly written event table will only make people think notify function is bad.
    pub fn authored_notifies(
        source: &Path,
    ) -> Result<std::collections::HashMap<String, Vec<AnimNotifyInfo>>, ImportError> {
        let mut side = source.to_path_buf().into_os_string();
        side.push(".skinning/notifies.json");
        let path = std::path::PathBuf::from(side);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(std::collections::HashMap::new());
        };

        #[derive(serde::Deserialize)]
        struct Entry {
            time: f32,
            name: String,
        }
        #[derive(serde::Deserialize)]
        struct File {
            #[serde(default)]
            clips: std::collections::HashMap<String, Vec<Entry>>,
        }

        let parsed: File = serde_json::from_str(&text).map_err(|error| {
            ImportError::ImportFailed(format!("{}: invalid notify file: {error}", path.display()))
        })?;
        let mut out = std::collections::HashMap::new();
        for (clip, entries) in parsed.clips {
            let notifies: Vec<AnimNotifyInfo> = entries
                .into_iter()
                .filter(|entry| {
                    entry.time.is_finite() && entry.time >= 0.0 && !entry.name.trim().is_empty()
                })
                .map(|entry| AnimNotifyInfo {
                    time: entry.time,
                    name: entry.name,
                })
                .collect();
            out.insert(clip, notifies);
        }
        Ok(out)
    }

    /// Read author-side sync flag file (#560 Sync marker assetization).
    ///
    /// Position: `assets/models/hero.glb.skinning/sync_markers.json`
    ///
    /// ```json
    /// { "clips": { "walk": [0.0, 0.5], "run": [0.0, 0.5] } }
    /// ```
    ///
    /// Bitwise normalized to `[0, 1)`, limited values sorted deduplicated. File not present = no flag;
    /// Present but parse failure = build failure, avoiding wrong flag silently ignored.
    pub fn authored_sync_markers(
        source: &Path,
    ) -> Result<std::collections::HashMap<String, Vec<f32>>, ImportError> {
        let mut side = source.to_path_buf().into_os_string();
        side.push(".skinning/sync_markers.json");
        let path = std::path::PathBuf::from(side);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(std::collections::HashMap::new());
        };

        #[derive(serde::Deserialize)]
        struct File {
            #[serde(default)]
            clips: std::collections::HashMap<String, Vec<f32>>,
        }

        let parsed: File = serde_json::from_str(&text).map_err(|error| {
            ImportError::ImportFailed(format!(
                "{}: invalid sync marker file: {error}",
                path.display()
            ))
        })?;
        let mut out = std::collections::HashMap::new();
        for (clip, phases) in parsed.clips {
            let mut phases: Vec<f32> = phases
                .into_iter()
                .filter(|phase| phase.is_finite())
                .map(|phase| phase.rem_euclid(1.0))
                .collect();
            phases.sort_by(f32::total_cmp);
            phases.dedup_by(|a, b| (*a - *b).abs() <= 1e-6);
            out.insert(clip, phases);
        }
        Ok(out)
    }

    /// Read author-side Montage segment table (#560 P4).
    ///
    /// Position: `assets/models/hero.glb.skinning/montages.json`
    ///
    /// ```json
    /// {
    ///   "clips": {
    ///     "vault": {
    ///       "sections": [
    ///         { "name": "Start", "start": 0.0, "end": 0.3 },
    ///         { "name": "Vault", "start": 0.3, "end": 0.8 }
    ///       ]
    ///     }
    ///   }
    /// }
    /// ```
    ///
    /// Key is glTF animation **original name**. Segment name must be non-empty and unique within same clip, time must be limited,
    /// `start >= 0`, `end >= start`. File not present = no segment; present but parse failure = build
    /// Failure, avoiding wrong segment silently ignored.
    pub fn authored_montages(
        source: &Path,
    ) -> Result<std::collections::HashMap<String, AuthoredMontage>, ImportError> {
        let mut side = source.to_path_buf().into_os_string();
        side.push(".skinning/montages.json");
        let path = std::path::PathBuf::from(side);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(std::collections::HashMap::new());
        };

        #[derive(serde::Deserialize)]
        struct Entry {
            #[serde(default)]
            sections: Vec<Range>,
            #[serde(default)]
            warp_windows: Vec<Range>,
        }
        #[derive(serde::Deserialize)]
        struct Range {
            name: String,
            start: f32,
            end: f32,
        }
        #[derive(serde::Deserialize)]
        struct File {
            #[serde(default)]
            clips: std::collections::HashMap<String, Entry>,
        }

        let parsed: File = serde_json::from_str(&text).map_err(|error| {
            ImportError::ImportFailed(format!("{}: invalid montage file: {error}", path.display()))
        })?;

        let parse_ranges = |clip: &str, kind: &str, ranges: Vec<Range>| {
            let mut out = Vec::with_capacity(ranges.len());
            for range in ranges {
                let name = range.name.trim();
                if name.is_empty() {
                    return Err(ImportError::ImportFailed(format!(
                        "{}: clip '{clip}' has a montage {kind} without a name",
                        path.display()
                    )));
                }
                if !range.start.is_finite()
                    || !range.end.is_finite()
                    || range.start < 0.0
                    || range.end < range.start
                {
                    return Err(ImportError::ImportFailed(format!(
                        "{}: clip '{clip}' {kind} '{name}' needs finite 0 <= start <= end",
                        path.display()
                    )));
                }
                if out
                    .iter()
                    .any(|existing: &(String, f32, f32)| existing.0 == name)
                {
                    return Err(ImportError::ImportFailed(format!(
                        "{}: clip '{clip}' has duplicate montage {kind} '{name}'",
                        path.display()
                    )));
                }
                out.push((name.to_string(), range.start, range.end));
            }
            out.sort_by(|a: &(String, f32, f32), b: &(String, f32, f32)| {
                a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0))
            });
            Ok(out)
        };

        let mut out = std::collections::HashMap::new();
        for (clip, entry) in parsed.clips {
            let sections = parse_ranges(&clip, "section", entry.sections)?
                .into_iter()
                .map(|(name, start, end)| AnimClipSectionInfo { name, start, end })
                .collect();
            let warp_windows = parse_ranges(&clip, "warp window", entry.warp_windows)?
                .into_iter()
                .map(|(name, start, end)| AnimClipWarpWindowInfo { name, start, end })
                .collect();
            out.insert(
                clip,
                AuthoredMontage {
                    sections,
                    warp_windows,
                },
            );
        }
        Ok(out)
    }

    /// Read-only glTF, **not cook** skin/animation path planning (#560 P0-4).
    ///
    /// Path rule fully consistent with [`Self::side_assets`] (same source file → same path),
    /// So packer can before cooking scene fill `SkinnedMeshRenderer` / `SkeletonRoot` /
    /// `Animator` automatically into entity, without needing to encode RSCL/RANI once.
    pub fn rig_paths(path: &Path, mesh_pak_path: &str) -> Result<GltfRigPaths, ImportError> {
        let mesh = Self::read_gltf(path)?;
        let mut rig = GltfRigPaths::default();
        if mesh.skinning.is_empty() {
            return Ok(rig);
        }
        rig.skeleton = Some(format!("{mesh_pak_path}.skinning/skeleton.rskel"));
        if mesh.joints.len() == mesh.positions.len() && !mesh.joints.is_empty() {
            rig.skinned_mesh = Some(format!("{mesh_pak_path}.skinning/mesh.rskin"));
        }
        for (index, clip) in mesh.clips.iter().enumerate() {
            let name = sanitize_asset_name(Some(&clip.name), "clip");
            rig.clips.push(format!(
                "{mesh_pak_path}.skinning/clips/{index}_{name}.ranim"
            ));
        }
        Ok(rig)
    }

    /// Cook all materials and textures referenced by a glTF/GLB file (#520).
    ///
    /// Paths are derived deterministically from `mesh_pak_path`:
    /// `{mesh}.materials/{index}_{name}.mat.json` and
    /// `{mesh}.textures/{index}_{name}.png`. The latter contain RTEX bytes;
    /// keeping the source-style extension matches the rest of the package
    /// layout, where a `.png` path is a cooked RTEX asset at runtime.
    pub fn side_assets(path: &Path, mesh_pak_path: &str) -> Result<GltfSideAssets, ImportError> {
        use crate::cooker::profile::CookSettings;
        use crate::cooker::{CookContext, Cooker as _, TextureCooker};

        let mesh = Self::read_gltf(path)?;
        let plan = gltf_material_plan(&mesh, mesh_pak_path);
        let mut assets = Vec::new();

        let mut image_paths: Vec<Option<String>> = vec![None; mesh.images.len()];
        for (index, info) in mesh.images.iter().enumerate() {
            let Some(pixels) = mesh.image_pixels.get(index) else {
                continue;
            };
            if pixels.is_empty() || info.width == 0 || info.height == 0 {
                tracing::warn!(
                    "glTF image {index} has no decodable RGBA8 pixels; skipping texture asset"
                );
                continue;
            }
            let tex_path = gltf_texture_path(mesh_pak_path, index, info.name.as_deref());
            let intermediate =
                TextureImporter::write_intermediate(info.width, info.height, 4, pixels);
            let record = AssetRecord::new(
                AssetId::from_path(&tex_path),
                tex_path.clone(),
                AssetType::Texture,
                "texture-importer",
            );
            let cooked = TextureCooker
                .cook(&CookContext {
                    record: &record,
                    imported_data: &intermediate,
                    settings: &CookSettings::default(),
                })
                .map_err(|error| {
                    ImportError::ImportFailed(format!(
                        "cook glTF texture '{tex_path}' failed: {error}"
                    ))
                })?;
            assets.push(ImportedSideAsset {
                path: tex_path.clone(),
                cooked_data: cooked.cooked_data,
            });
            image_paths[index] = Some(tex_path);
        }

        let tex_path = |slot: Option<usize>| -> Option<String> {
            slot.and_then(|index| image_paths.get(index).cloned().flatten())
        };

        for (index, info) in mesh.materials.iter().enumerate() {
            let mat_path = plan.material_paths.get(index).cloned().ok_or_else(|| {
                ImportError::ImportFailed(format!(
                    "glTF material {index} has no generated path for '{mesh_pak_path}'"
                ))
            })?;
            let material = MaterialJson {
                name: info.name.clone(),
                base_color: info.base_color,
                metallic: info.metallic,
                roughness: info.roughness,
                emissive: info.emissive,
                emissive_strength: 1.0,
                normal_scale: info.normal_scale,
                occlusion_strength: info.occlusion_strength,
                transmission: 0.0,
                ior: 1.5,
                translucency: 0.0,
                anisotropy: 0.0,
                clearcoat: 0.0,
                clearcoat_roughness: 0.0,
                albedo_tex: tex_path(info.base_color_texture),
                normal_tex: tex_path(info.normal_texture),
                metallic_roughness_tex: tex_path(info.metallic_roughness_texture),
                emissive_tex: tex_path(info.emissive_texture),
                occlusion_tex: tex_path(info.occlusion_texture),
                alpha_mode: Some(info.alpha_mode.clone()),
                alpha_cutoff: info.alpha_cutoff,
            };
            assets.push(cook_gltf_material(&material, &mat_path)?);
        }

        if plan.needs_default {
            let material = MaterialJson {
                name: Some("glTF default".to_string()),
                base_color: [0.8, 0.8, 0.8, 1.0],
                metallic: 0.0,
                roughness: 0.5,
                emissive: [0.0; 3],
                emissive_strength: 1.0,
                normal_scale: 1.0,
                occlusion_strength: 1.0,
                transmission: 0.0,
                ior: 1.5,
                translucency: 0.0,
                anisotropy: 0.0,
                clearcoat: 0.0,
                clearcoat_roughness: 0.0,
                albedo_tex: None,
                normal_tex: None,
                metallic_roughness_tex: None,
                emissive_tex: None,
                occlusion_tex: None,
                alpha_mode: Some("Opaque".to_string()),
                alpha_cutoff: None,
            };
            assets.push(cook_gltf_material(&material, &plan.default_path)?);
        }

        let mut skeleton_path = None;
        let mut skinned_mesh_path = None;
        let mut clip_paths = Vec::new();
        if !mesh.skinning.is_empty() {
            let joint_count = mesh.skinning.joint_count() as u32;
            let skel_path = format!("{mesh_pak_path}.skinning/skeleton.rskel");
            match crate::cooker::skinned::encode_rskel(&mesh.skinning.skeleton) {
                Some(bytes) => {
                    assets.push(ImportedSideAsset {
                        path: skel_path.clone(),
                        cooked_data: bytes,
                    });
                    skeleton_path = Some(skel_path);
                }
                None => tracing::warn!(
                    "glTF skeleton for '{mesh_pak_path}' could not be encoded; skipping .rskel"
                ),
            }

            if mesh.joints.len() == mesh.positions.len() && !mesh.joints.is_empty() {
                let rskin_path = format!("{mesh_pak_path}.skinning/mesh.rskin");
                match skinned::skinned_mesh_info(&mesh, joint_count) {
                    Some(info) => match crate::cooker::skinned::encode_rskin(&info) {
                        Some(bytes) => {
                            assets.push(ImportedSideAsset {
                                path: rskin_path.clone(),
                                cooked_data: bytes,
                            });
                            skinned_mesh_path = Some(rskin_path);
                        }
                        None => tracing::warn!(
                            "glTF skinned mesh for '{mesh_pak_path}' could not be encoded; skipping .rskin"
                        ),
                    },
                    None => tracing::warn!(
                        "glTF skinned mesh for '{mesh_pak_path}' has no usable joint/weight data; skipping .rskin"
                    ),
                }
            } else if !mesh.skinning.is_empty() {
                tracing::warn!(
                    "glTF '{mesh_pak_path}' has a skeleton but no complete JOINTS_0/WEIGHTS_0 stream; no .rskin"
                );
            }

            let authored = Self::authored_notifies(path)?;
            let mut used_notifies: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            let authored_markers = Self::authored_sync_markers(path)?;
            let mut used_markers: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            let authored_montages = Self::authored_montages(path)?;
            let mut used_montages: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for (index, clip) in mesh.clips.iter().enumerate() {
                let mut clip = clip.clone();
                if let Some(notifies) = authored.get(&clip.name) {
                    used_notifies.insert(clip.name.clone());
                    clip.notifies = notifies.clone();
                }
                if let Some(markers) = authored_markers.get(&clip.name) {
                    used_markers.insert(clip.name.clone());
                    clip.sync_markers = markers.clone();
                }
                if let Some(montage) = authored_montages.get(&clip.name) {
                    let duration = clip.clip_duration();
                    for section in &montage.sections {
                        if section.end > duration {
                            return Err(ImportError::ImportFailed(format!(
                                "{}: clip '{}' section '{}' ends at {:.3}s, beyond the {:.3}s clip",
                                path.display(),
                                clip.name,
                                section.name,
                                section.end,
                                duration
                            )));
                        }
                    }
                    for window in &montage.warp_windows {
                        if window.end > duration {
                            return Err(ImportError::ImportFailed(format!(
                                "{}: clip '{}' warp window '{}' ends at {:.3}s, beyond the {:.3}s clip",
                                path.display(),
                                clip.name,
                                window.name,
                                window.end,
                                duration
                            )));
                        }
                    }
                    used_montages.insert(clip.name.clone());
                    clip.sections = montage.sections.clone();
                    clip.warp_windows = montage.warp_windows.clone();
                }
                let clip = &clip;
                let name = sanitize_asset_name(Some(&clip.name), "clip");
                let clip_path = format!("{mesh_pak_path}.skinning/clips/{index}_{name}.ranim");
                match crate::cooker::skinned::encode_ranim(clip) {
                    Some(bytes) => {
                        assets.push(ImportedSideAsset {
                            path: clip_path.clone(),
                            cooked_data: bytes,
                        });
                        clip_paths.push(clip_path);
                    }
                    None => {
                        tracing::warn!("glTF clip '{}' could not be encoded; skipped", clip.name)
                    }
                }
            }
            for name in authored.keys() {
                if !used_notifies.contains(name) {
                    tracing::warn!(
                        "'{mesh_pak_path}': notify file names clip '{name}', but the glTF has                          no animation with that name"
                    );
                }
            }
            for name in authored_markers.keys() {
                if !used_markers.contains(name) {
                    tracing::warn!(
                        "'{mesh_pak_path}': sync marker file names clip '{name}', but the glTF has no animation with that name"
                    );
                }
            }
            for name in authored_montages.keys() {
                if !used_montages.contains(name) {
                    tracing::warn!(
                        "'{mesh_pak_path}': montage file names clip '{name}', but the glTF has no animation with that name"
                    );
                }
            }
        }

        Ok(GltfSideAssets {
            material_paths: plan.material_paths,
            submesh_material_paths: plan.submesh_material_paths,
            assets,
            skeleton_path,
            skinned_mesh_path,
            clip_paths,
        })
    }
}

/// Deterministic material/texture plan for one glTF mesh.
struct GltfMaterialPlan {
    /// One path per source material, in glTF material order.
    material_paths: Vec<String>,
    /// One path per submesh, in draw order.
    submesh_material_paths: Vec<String>,
    default_path: String,
    /// True when at least one submesh has no source material (or the file has
    /// no materials at all) and therefore needs the generated default.
    needs_default: bool,
}

fn gltf_material_plan(mesh: &MeshData, mesh_pak_path: &str) -> GltfMaterialPlan {
    let material_paths: Vec<String> = mesh
        .materials
        .iter()
        .enumerate()
        .map(|(index, info)| {
            let name = sanitize_asset_name(info.name.as_deref(), "material");
            format!("{mesh_pak_path}.materials/{index}_{name}.mat.json")
        })
        .collect();
    let default_path = format!("{mesh_pak_path}.materials/default.mat.json");

    let mut needs_default = mesh.materials.is_empty() && !mesh.submeshes.is_empty();
    let submesh_material_paths = mesh
        .submeshes
        .iter()
        .map(|submesh| {
            submesh
                .material_index
                .and_then(|index| material_paths.get(index).cloned())
                .unwrap_or_else(|| {
                    needs_default = true;
                    default_path.clone()
                })
        })
        .collect();

    GltfMaterialPlan {
        material_paths,
        submesh_material_paths,
        default_path,
        needs_default,
    }
}

fn gltf_texture_path(mesh_pak_path: &str, index: usize, name: Option<&str>) -> String {
    let name = sanitize_asset_name(name, "image");
    format!("{mesh_pak_path}.textures/{index}_{name}.png")
}

fn sanitize_asset_name(name: Option<&str>, fallback: &str) -> String {
    let mut out = String::new();
    for character in name.unwrap_or(fallback).chars() {
        if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
            out.push(character);
        } else {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_');
    out = if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    };
    out.truncate(48);
    out
}

/// Cook a generated material. `tex_paths` are project-relative pak paths in
/// RMAT slot order; dependencies are derived with the same `AssetId::from_path`
/// the packer uses for the staged texture files.
fn cook_gltf_material(
    material: &MaterialJson,
    material_path: &str,
) -> Result<ImportedSideAsset, ImportError> {
    use crate::cooker::profile::CookSettings;
    use crate::cooker::{CookContext, Cooker as _, MaterialCooker};

    let raw_paths: [Option<String>; 5] = [
        material.albedo_tex.clone(),
        material.normal_tex.clone(),
        material.metallic_roughness_tex.clone(),
        material.emissive_tex.clone(),
        material.occlusion_tex.clone(),
    ];
    let mut resolved: [Option<String>; 5] = [None, None, None, None, None];
    let mut dependencies = Vec::new();
    for (index, path) in raw_paths.iter().enumerate() {
        if let Some(path) = path {
            resolved[index] = Some(path.clone());
            dependencies.push(AssetId::from_path(path));
        }
    }
    let intermediate = MaterialImporter::write_intermediate(material, &resolved);
    let mut record = AssetRecord::new(
        AssetId::from_path(material_path),
        material_path.to_string(),
        AssetType::Material,
        "material-importer",
    );
    record.dependencies = dependencies;
    let cooked = MaterialCooker
        .cook(&CookContext {
            record: &record,
            imported_data: &intermediate,
            settings: &CookSettings::default(),
        })
        .map_err(|error| {
            ImportError::ImportFailed(format!(
                "cook glTF material '{material_path}' failed: {error}"
            ))
        })?;
    Ok(ImportedSideAsset {
        path: material_path.to_string(),
        cooked_data: cooked.cooked_data,
    })
}

/// Convert a decoded `gltf` image to tightly packed RGBA8.
fn rgba8_from_gltf_image(data: &gltf::image::Data) -> Vec<u8> {
    use gltf::image::Format;

    let byte = |value: f32| -> u8 { (value.clamp(0.0, 1.0) * 255.0).round() as u8 };
    let wide = |value: u16| -> u8 { (value as f32 / 65535.0 * 255.0).round() as u8 };
    let float = |bytes: &[u8]| -> f32 { f32::from_le_bytes(bytes.try_into().unwrap()) };

    match data.format {
        Format::R8 => data.pixels.iter().flat_map(|&r| [r, r, r, 255]).collect(),
        Format::R8G8 => data
            .pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], 0, 255])
            .collect(),
        Format::R8G8B8 => data
            .pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        Format::R8G8B8A8 => data.pixels.clone(),
        Format::R16 => data
            .pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| {
                let r = wide(u16::from_le_bytes([p[0], p[1]]));
                [r, r, r, 255]
            })
            .collect(),
        Format::R16G16 => data
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| {
                let r = wide(u16::from_le_bytes([p[0], p[1]]));
                let g = wide(u16::from_le_bytes([p[2], p[3]]));
                [r, g, 0, 255]
            })
            .collect(),
        Format::R16G16B16 => data
            .pixels
            .as_chunks::<6>()
            .0
            .iter()
            .flat_map(|p| {
                let r = wide(u16::from_le_bytes([p[0], p[1]]));
                let g = wide(u16::from_le_bytes([p[2], p[3]]));
                let b = wide(u16::from_le_bytes([p[4], p[5]]));
                [r, g, b, 255]
            })
            .collect(),
        Format::R16G16B16A16 => data
            .pixels
            .as_chunks::<8>()
            .0
            .iter()
            .flat_map(|p| {
                [
                    wide(u16::from_le_bytes([p[0], p[1]])),
                    wide(u16::from_le_bytes([p[2], p[3]])),
                    wide(u16::from_le_bytes([p[4], p[5]])),
                    wide(u16::from_le_bytes([p[6], p[7]])),
                ]
            })
            .collect(),
        Format::R32G32B32FLOAT => data
            .pixels
            .as_chunks::<12>()
            .0
            .iter()
            .flat_map(|p| {
                [
                    byte(float(&p[0..4])),
                    byte(float(&p[4..8])),
                    byte(float(&p[8..12])),
                    255,
                ]
            })
            .collect(),
        Format::R32G32B32A32FLOAT => data
            .pixels
            .as_chunks::<16>()
            .0
            .iter()
            .flat_map(|p| {
                [
                    byte(float(&p[0..4])),
                    byte(float(&p[4..8])),
                    byte(float(&p[8..12])),
                    byte(float(&p[12..16])),
                ]
            })
            .collect(),
    }
}

/// Accumulated result: entire scene tree baked into one world-space triangle mesh.
#[derive(Debug)]
struct MeshData {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uv0: Vec<[f32; 2]>,
    indices: Vec<u32>,
    /// If any primitive lacks normals, entire mesh omits normals (handled by #515 bitmask normal stream).
    all_normals: bool,
    all_uv0: bool,
    primitive_count: usize,
    node_count: usize,
    material_count: usize,
    has_skins: bool,
    /// Each primitive after merge: index buffer range and material (#520).
    submeshes: Vec<SubmeshInfo>,
    /// glTF declared material parameters (#520); this version only goes into metadata, not generating assets.
    materials: Vec<MaterialInfo>,
    /// glTF texture slice size / source (#520); same only into metadata.
    images: Vec<ImageInfo>,
    /// Decoded RGBA8 pixels for each image, in glTF image order. Empty entries
    /// mean the decoder could not produce RGBA8 (unsupported format).
    image_pixels: Vec<Vec<u8>>,
    /// 4 bone indices per vertex (already mapped to global skeleton), #560.
    joints: Vec<[u16; 4]>,
    /// 4 weights per vertex (original value, cooker responsible for normalization), #560.
    weights: Vec<[f32; 4]>,
    /// True only when each primitive carries JOINTS_0/WEIGHTS_0.
    all_skin_attrs: bool,
    /// Skeleton + each skin joint heavy map (empty when no skin), #560.
    skinning: skinned::GltfSkinning,
    /// glTF animation clips (already mapped to global skeleton), #560.
    clips: Vec<prism_asset::formats_skinned::AnimClipInfo>,
}

/// Merge one submesh inside mesh: index range + material used.
#[derive(Debug, Clone, serde::Serialize)]
struct SubmeshInfo {
    index_start: u32,
    index_count: u32,
    material_index: Option<usize>,
    material: Option<String>,
}

/// glTF material PBR parameters (only keep fields RMAT can express / map in future).
#[derive(Debug, Clone, serde::Serialize)]
struct MaterialInfo {
    name: Option<String>,
    base_color: [f32; 4],
    metallic: f32,
    roughness: f32,
    emissive: [f32; 3],
    alpha_mode: String,
    /// glTF `alphaCutoff`; `None` means the glTF default (0.5).
    alpha_cutoff: Option<f32>,
    double_sided: bool,
    base_color_texture: Option<usize>,
    metallic_roughness_texture: Option<usize>,
    normal_texture: Option<usize>,
    normal_scale: f32,
    occlusion_texture: Option<usize>,
    occlusion_strength: f32,
    emissive_texture: Option<usize>,
}

/// glTF texture slice metadata. Pixels themselves not written into mesh metadata (avoid JSON bloat).
#[derive(Debug, Clone, serde::Serialize)]
struct ImageInfo {
    name: Option<String>,
    mime_type: Option<String>,
    uri: Option<String>,
    width: u32,
    height: u32,
    source_bytes: usize,
    format: String,
}

impl Default for MeshData {
    fn default() -> Self {
        Self {
            positions: Vec::new(),
            normals: Vec::new(),
            uv0: Vec::new(),
            indices: Vec::new(),
            all_normals: true,
            all_uv0: true,
            primitive_count: 0,
            node_count: 0,
            material_count: 0,
            has_skins: false,
            submeshes: Vec::new(),
            materials: Vec::new(),
            images: Vec::new(),
            image_pixels: Vec::new(),
            joints: Vec::new(),
            weights: Vec::new(),
            all_skin_attrs: true,
            skinning: skinned::GltfSkinning::default(),
            clips: Vec::new(),
        }
    }
}

/// Normal matrix = inverse-transpose of left 3×3 (still correct with non-uniform scale).
fn normal_matrix(m: &Mat4) -> Mat3 {
    let linear = Mat3::from_mat4(*m);
    if linear.determinant().abs() < 1e-12 {
        return Mat3::IDENTITY;
    }
    linear.inverse().transpose()
}

fn apply_normal(m: &Mat3, n: [f32; 3]) -> [f32; 3] {
    let transformed = *m * Vec3::from(n);
    let length = transformed.length();
    if length > 1e-6 {
        (transformed / length).to_array()
    } else {
        [0.0, 1.0, 0.0]
    }
}

fn collect_node(
    node: &gltf::Node,
    parent: &Mat4,
    buffers: &[gltf::buffer::Data],
    out: &mut MeshData,
) {
    out.node_count += 1;
    let local = Mat4::from_cols_array_2d(&node.transform().matrix());
    let world = *parent * local;
    if let Some(mesh) = node.mesh() {
        let skin = node.skin();
        for primitive in mesh.primitives() {
            push_primitive(&primitive, &world, buffers, skin.as_ref(), out);
        }
    }
    for child in node.children() {
        collect_node(&child, &world, buffers, out);
    }
}

fn push_primitive(
    primitive: &gltf::Primitive,
    world: &Mat4,
    buffers: &[gltf::buffer::Data],
    skin: Option<&gltf::Skin>,
    out: &mut MeshData,
) {
    if primitive.mode() != gltf::mesh::Mode::Triangles {
        tracing::warn!(
            "gltf: skipping a {:?} primitive (only triangles are supported)",
            primitive.mode()
        );
        return;
    }

    let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));
    let Some(positions) = reader.read_positions() else {
        tracing::warn!("gltf: skipping a primitive without POSITION");
        return;
    };
    let positions: Vec<[f32; 3]> = positions.collect();
    if positions.is_empty() {
        return;
    }

    let base = out.positions.len() as u32;
    let index_start = out.indices.len() as u32;
    let normals = normal_matrix(world);
    out.positions.extend(
        positions
            .iter()
            .map(|p| world.transform_point3(Vec3::from(*p)).to_array()),
    );

    match reader.read_normals() {
        Some(normals_iter) => {
            let collected: Vec<[f32; 3]> = normals_iter.collect();
            if collected.len() == positions.len() {
                out.normals
                    .extend(collected.iter().map(|n| apply_normal(&normals, *n)));
            } else {
                out.all_normals = false;
                out.normals
                    .resize(out.normals.len() + positions.len(), [0.0, 0.0, 0.0]);
            }
        }
        None => {
            out.all_normals = false;
            out.normals
                .resize(out.normals.len() + positions.len(), [0.0, 0.0, 0.0]);
        }
    }

    match reader.read_tex_coords(0) {
        Some(uvs) => {
            let collected: Vec<[f32; 2]> = uvs.into_f32().collect();
            if collected.len() == positions.len() {
                out.uv0.extend(collected);
            } else {
                out.all_uv0 = false;
                out.uv0.resize(out.uv0.len() + positions.len(), [0.0, 0.0]);
            }
        }
        None => {
            out.all_uv0 = false;
            out.uv0.resize(out.uv0.len() + positions.len(), [0.0, 0.0]);
        }
    }

    if out.all_skin_attrs {
        let remap =
            skin.and_then(|skin| out.skinning.remap(skin.index()).map(|remap| (skin, remap)));
        match (remap, reader.read_joints(0), reader.read_weights(0)) {
            (Some((_skin, remap)), Some(joints), Some(weights)) => {
                let joints: Vec<[u16; 4]> = joints.into_u16().collect();
                let weights: Vec<[f32; 4]> = weights.into_f32().collect();
                if joints.len() == positions.len() && weights.len() == positions.len() {
                    out.joints.extend(joints.iter().map(|joint| {
                        let mut mapped = [0u16; 4];
                        for (slot, index) in joint.iter().enumerate() {
                            mapped[slot] = remap.get(*index as usize).copied().unwrap_or(0);
                        }
                        mapped
                    }));
                    out.weights.extend(weights);
                } else {
                    out.all_skin_attrs = false;
                    out.joints
                        .resize(out.joints.len() + positions.len(), [0; 4]);
                    out.weights
                        .resize(out.weights.len() + positions.len(), [0.0; 4]);
                }
            }
            _ => {
                out.all_skin_attrs = false;
                out.joints
                    .resize(out.joints.len() + positions.len(), [0; 4]);
                out.weights
                    .resize(out.weights.len() + positions.len(), [0.0; 4]);
            }
        }
    }

    match reader.read_indices() {
        Some(indices) => out.indices.extend(
            indices
                .into_u32()
                .filter(|index| (*index as usize) < positions.len())
                .map(|index| index + base),
        ),
        None => out
            .indices
            .extend((0..positions.len() as u32).map(|index| index + base)),
    }

    let index_count = out.indices.len() as u32 - index_start;
    let material = primitive.material();
    out.submeshes.push(SubmeshInfo {
        index_start,
        index_count,
        material_index: material.index(),
        material: material.name().map(str::to_string),
    });

    out.primitive_count += 1;
}

impl Importer for GltfImporter {
    fn name(&self) -> &'static str {
        "gltf-importer"
    }

    fn version(&self) -> u32 {
        2
    }

    fn can_import(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("gltf") || e.eq_ignore_ascii_case("glb"))
    }

    fn import(&self, ctx: &ImportContext) -> Result<ImportResult, ImportError> {
        let src = &ctx.source_path;

        let mesh = Self::read_gltf(src)?;

        let normals = mesh.all_normals.then_some(mesh.normals.as_slice());
        let texcoords = mesh.all_uv0.then_some(mesh.uv0.as_slice());
        let output_data = write_mesh_intermediate(
            &mesh.positions,
            normals,
            texcoords,
            &mesh.indices,
            &mesh.submeshes,
        );

        let metadata = serde_json::json!({
            "original_name": src
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown"),
            "vertex_count": mesh.positions.len(),
            "index_count": mesh.indices.len(),
            "has_normals": normals.is_some(),
            "has_texcoords": texcoords.is_some(),
            "primitive_count": mesh.primitive_count,
            "node_count": mesh.node_count,
            "material_count": mesh.material_count,
            "submesh_count": mesh.submeshes.len(),
            "submeshes": mesh.submeshes,
            "materials": mesh.materials,
            "images": mesh.images,
            "has_skins": mesh.has_skins,
            "intermediate_size": output_data.len(),
        });

        Ok(ImportResult {
            asset_type: AssetType::Mesh,
            dependencies: Vec::new(),
            output_data,
            metadata: Some(metadata),
        })
    }
}

/// Import Wavefront `.obj`.
///
/// Supports `v` / `vt` / `vn` / `f`, three face index writing methods (`v`, `v/vt`, `v//vn`,
/// `v/vt/vn`) and **negative index**; many polygons triangulated by fan. Vertices by
/// `(position, uv, normal)` tuple deduplicated, therefore triangles sharing edges store only one vertex.
///
/// Consistent with glTF importer: if any face lacks UVs / normals, entire mesh omits that property
/// (not faking with zero values); `usemtl` / `o` / `g` / `s` only used for statistics or ignored before.
pub struct ObjImporter;

impl ObjImporter {
    fn read_obj(path: &Path) -> Result<MeshData, ImportError> {
        let text = std::fs::read_to_string(path)?;
        let mut positions: Vec<[f32; 3]> = Vec::new();
        let mut texcoords: Vec<[f32; 2]> = Vec::new();
        let mut normals: Vec<[f32; 3]> = Vec::new();

        let mut vertex_positions: Vec<[f32; 3]> = Vec::new();
        let mut vertex_uvs: Vec<Option<[f32; 2]>> = Vec::new();
        let mut vertex_normals: Vec<Option<[f32; 3]>> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut dedup: HashMap<(usize, Option<usize>, Option<usize>), u32> = HashMap::new();

        let mut all_normals = true;
        let mut all_uv0 = true;
        let mut face_count = 0usize;
        let mut materials = std::collections::HashSet::new();

        for (line_no, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let mut fields = line.split_whitespace();
            let Some(kind) = fields.next() else {
                continue;
            };
            match kind {
                "v" => {
                    let x = parse_obj_float(fields.next(), line_no, "vertex x")?;
                    let y = parse_obj_float(fields.next(), line_no, "vertex y")?;
                    let z = parse_obj_float(fields.next(), line_no, "vertex z")?;
                    positions.push([x, y, z]);
                }
                "vt" => {
                    let u = parse_obj_float(fields.next(), line_no, "texcoord u")?;
                    let v = fields
                        .next()
                        .map(|value| parse_obj_float(Some(value), line_no, "texcoord v"))
                        .transpose()?
                        .unwrap_or(0.0);
                    texcoords.push([u, v]);
                }
                "vn" => {
                    let x = parse_obj_float(fields.next(), line_no, "normal x")?;
                    let y = parse_obj_float(fields.next(), line_no, "normal y")?;
                    let z = parse_obj_float(fields.next(), line_no, "normal z")?;
                    normals.push([x, y, z]);
                }
                "f" => {
                    let face: Vec<(usize, Option<usize>, Option<usize>)> = fields
                        .map(|token| {
                            parse_obj_face_ref(
                                token,
                                positions.len(),
                                texcoords.len(),
                                normals.len(),
                                line_no,
                            )
                        })
                        .collect::<Result<_, _>>()?;
                    if face.len() < 3 {
                        return Err(ImportError::ImportFailed(format!(
                            "OBJ line {}: face needs at least 3 vertices",
                            line_no + 1
                        )));
                    }
                    for triangle in 1..face.len() - 1 {
                        for &(position, uv, normal) in
                            [&face[0], &face[triangle], &face[triangle + 1]]
                        {
                            if uv.is_none() {
                                all_uv0 = false;
                            }
                            if normal.is_none() {
                                all_normals = false;
                            }
                            let key = (position, uv, normal);
                            let index = match dedup.get(&key) {
                                Some(&index) => index,
                                None => {
                                    let index = vertex_positions.len() as u32;
                                    dedup.insert(key, index);
                                    vertex_positions.push(positions[position]);
                                    vertex_uvs.push(uv.map(|i| texcoords[i]));
                                    vertex_normals.push(normal.map(|i| normals[i]));
                                    index
                                }
                            };
                            indices.push(index);
                        }
                    }
                    face_count += 1;
                }
                "usemtl" => {
                    if let Some(name) = fields.next() {
                        materials.insert(name.to_string());
                    }
                }
                _ => {}
            }
        }

        if face_count == 0 {
            return Err(ImportError::ImportFailed(
                "OBJ contains no faces (`f` lines)".into(),
            ));
        }

        let out_normals = if all_normals {
            vertex_normals
                .iter()
                .map(|normal| normal.unwrap_or([0.0, 0.0, 1.0]))
                .collect()
        } else {
            Vec::new()
        };
        let out_uvs = if all_uv0 {
            vertex_uvs
                .iter()
                .map(|uv| uv.unwrap_or([0.0, 0.0]))
                .collect()
        } else {
            Vec::new()
        };

        Ok(MeshData {
            positions: vertex_positions,
            normals: out_normals,
            uv0: out_uvs,
            indices,
            all_normals,
            all_uv0,
            primitive_count: 1,
            node_count: 1,
            material_count: materials.len(),
            has_skins: false,
            ..Default::default()
        })
    }
}

fn parse_obj_float(value: Option<&str>, line: usize, what: &str) -> Result<f32, ImportError> {
    let Some(value) = value else {
        return Err(ImportError::ImportFailed(format!(
            "OBJ line {}: missing {what}",
            line + 1
        )));
    };
    value
        .parse::<f32>()
        .map_err(|_| {
            ImportError::ImportFailed(format!("OBJ line {}: invalid {what} `{value}`", line + 1))
        })
        .and_then(|parsed| {
            if parsed.is_finite() {
                Ok(parsed)
            } else {
                Err(ImportError::ImportFailed(format!(
                    "OBJ line {}: {what} `{value}` is not finite",
                    line + 1
                )))
            }
        })
}

/// Parse a 1-based (or negative = relative to current end) OBJ index.
fn parse_obj_index(value: &str, len: usize, what: &str, line: usize) -> Result<usize, ImportError> {
    let raw: i64 = value.parse().map_err(|_| {
        ImportError::ImportFailed(format!(
            "OBJ line {}: invalid {what} index `{value}`",
            line + 1
        ))
    })?;
    let index = match raw {
        0 => {
            return Err(ImportError::ImportFailed(format!(
                "OBJ line {}: {what} index 0 is not valid (OBJ is 1-based)",
                line + 1
            )))
        }
        value if value > 0 => value - 1,
        value => len as i64 + value,
    };
    if index < 0 || index as usize >= len {
        return Err(ImportError::ImportFailed(format!(
            "OBJ line {}: {what} index `{value}` is out of range (have {len})",
            line + 1
        )));
    }
    Ok(index as usize)
}

fn parse_obj_face_ref(
    token: &str,
    positions: usize,
    texcoords: usize,
    normals: usize,
    line: usize,
) -> Result<(usize, Option<usize>, Option<usize>), ImportError> {
    let mut parts = token.split('/');
    let position = parse_obj_index(
        parts.next().unwrap_or_default(),
        positions,
        "position",
        line,
    )?;
    let uv = match parts.next() {
        None | Some("") => None,
        Some(value) => Some(parse_obj_index(value, texcoords, "texcoord", line)?),
    };
    let normal = match parts.next() {
        None | Some("") => None,
        Some(value) => Some(parse_obj_index(value, normals, "normal", line)?),
    };
    if parts.next().is_some() {
        return Err(ImportError::ImportFailed(format!(
            "OBJ line {}: face reference `{token}` has too many components",
            line + 1
        )));
    }
    Ok((position, uv, normal))
}

impl Importer for ObjImporter {
    fn name(&self) -> &'static str {
        "obj-importer"
    }

    fn version(&self) -> u32 {
        1
    }

    fn can_import(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("obj"))
    }

    fn import(&self, ctx: &ImportContext) -> Result<ImportResult, ImportError> {
        let src = &ctx.source_path;
        let mesh = Self::read_obj(src)?;

        let normals = mesh.all_normals.then_some(mesh.normals.as_slice());
        let texcoords = mesh.all_uv0.then_some(mesh.uv0.as_slice());
        let output_data =
            write_mesh_intermediate(&mesh.positions, normals, texcoords, &mesh.indices, &[]);

        let metadata = serde_json::json!({
            "original_name": src
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown"),
            "source_format": "obj",
            "vertex_count": mesh.positions.len(),
            "index_count": mesh.indices.len(),
            "has_normals": normals.is_some(),
            "has_texcoords": texcoords.is_some(),
            "primitive_count": mesh.primitive_count,
            "node_count": mesh.node_count,
            "material_count": mesh.material_count,
            "has_skins": mesh.has_skins,
            "intermediate_size": output_data.len(),
        });

        Ok(ImportResult {
            asset_type: AssetType::Mesh,
            dependencies: Vec::new(),
            output_data,
            metadata: Some(metadata),
        })
    }
}

/// Parse author's `*.animgraph.json` into RAGP binary, and record clip / state machine dependencies.
///
/// Graph format itself in `prism_asset::formats_anim_graph`; validation and encode in
/// `crate::cooker::anim_graph`. This importer only responsible for "file → import-time data +
/// dependencies", cooker then packs RAGP as-is into `.pak`.
pub struct AnimGraphImporter;

impl Importer for AnimGraphImporter {
    fn name(&self) -> &'static str {
        "anim-graph-importer"
    }

    fn version(&self) -> u32 {
        1
    }

    fn can_import(&self, path: &Path) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.to_ascii_lowercase().ends_with(".animgraph.json"))
    }

    fn import(&self, ctx: &ImportContext) -> Result<ImportResult, ImportError> {
        let text = std::fs::read_to_string(&ctx.source_path)?;
        let document: Value = serde_json::from_str(&text).map_err(|error| {
            ImportError::ImportFailed(format!(
                "{}: invalid animation graph JSON: {error}",
                ctx.source_path.display()
            ))
        })?;
        let graph = crate::cooker::anim_graph::parse_anim_graph_json(&document)
            .map_err(ImportError::ImportFailed)?;
        let data = crate::cooker::anim_graph::encode_rag(&graph).ok_or_else(|| {
            ImportError::ImportFailed(format!(
                "{}: animation graph could not be encoded",
                ctx.source_path.display()
            ))
        })?;

        let mut dependencies = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for path in graph.nodes.iter().flat_map(|node| node.clips.iter()).chain(
            graph
                .nodes
                .iter()
                .map(|node| &node.state_machine_path)
                .filter(|path| !path.is_empty()),
        ) {
            let normalized = path.replace('\\', "/");
            if let Some(id) = ctx.db.id_by_path(&normalized) {
                if seen.insert(id) {
                    dependencies.push(id);
                }
            }
        }

        let metadata = serde_json::json!({
            "parameter_count": graph.parameters.len(),
            "node_count": graph.nodes.len(),
            "output": graph.output,
            "dependency_count": dependencies.len(),
        });

        Ok(ImportResult {
            asset_type: AssetType::AnimGraph,
            dependencies,
            output_data: data,
            metadata: Some(metadata),
        })
    }
}

/// Imports JSON files, validating syntax, and registers them as Binary assets
/// with structured metadata.
pub struct JsonImporter;

impl Importer for JsonImporter {
    fn name(&self) -> &'static str {
        "json-importer"
    }

    fn version(&self) -> u32 {
        1
    }

    fn can_import(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
    }

    fn import(&self, ctx: &ImportContext) -> Result<ImportResult, ImportError> {
        let text = std::fs::read_to_string(&ctx.source_path)?;
        let parsed: Value = serde_json::from_str(&text)?;

        let metadata = serde_json::json!({
            "is_object": parsed.is_object(),
            "is_array": parsed.is_array(),
            "size_bytes": text.len(),
        });

        Ok(ImportResult {
            asset_type: AssetType::Binary,
            dependencies: Vec::new(),
            output_data: text.into_bytes(),
            metadata: Some(metadata),
        })
    }
}

/// Intermediate material format magic: "RMATI" (Resource Material Intermediate).
/// 5 bytes so it is distinct from the 4-byte RMAT runtime magic.
const MATERIAL_INTERMEDIATE_MAGIC: &[u8; 5] = b"RMATI";

/// Texture slots referenced by a material. The order is fixed and matches the
/// RMATI/RMAT binary layout (5 slots). Names match the `MaterialJson` fields.
const MATERIAL_TEX_SLOTS: [&str; 5] = [
    "albedo_tex",
    "normal_tex",
    "metallic_roughness_tex",
    "emissive_tex",
    "occlusion_tex",
];

/// Authoring schema for `.mat.json` files.
///
/// Texture fields are relative asset paths (resolved to `AssetId` dependencies
/// at import time). All scalar fields are optional with sensible defaults.
#[derive(Debug, Clone, serde::Deserialize)]
struct MaterialJson {
    #[serde(default)]
    name: Option<String>,
    #[serde(default = "default_base_color")]
    base_color: [f32; 4],
    #[serde(default)]
    metallic: f32,
    #[serde(default = "default_roughness")]
    roughness: f32,
    #[serde(default)]
    emissive: [f32; 3],
    #[serde(default)]
    emissive_strength: f32,
    #[serde(default = "default_one")]
    normal_scale: f32,
    #[serde(default = "default_one")]
    occlusion_strength: f32,
    #[serde(default)]
    transmission: f32,
    #[serde(default = "default_ior")]
    ior: f32,
    #[serde(default)]
    translucency: f32,
    #[serde(default)]
    anisotropy: f32,
    #[serde(default)]
    clearcoat: f32,
    #[serde(default)]
    clearcoat_roughness: f32,
    #[serde(default)]
    albedo_tex: Option<String>,
    #[serde(default)]
    normal_tex: Option<String>,
    #[serde(default)]
    metallic_roughness_tex: Option<String>,
    #[serde(default)]
    emissive_tex: Option<String>,
    #[serde(default)]
    occlusion_tex: Option<String>,
    /// glTF-style alpha mode name (`Opaque` / `Mask` / `Blend`). Absent means
    /// [`MaterialAlphaMode::Mask`], which preserves the pre-v2 behaviour of
    /// testing every surface's alpha at the default cutoff.
    #[serde(default)]
    alpha_mode: Option<String>,
    /// Alpha comparison threshold for `Mask`. Absent means 0.5.
    #[serde(default)]
    alpha_cutoff: Option<f32>,
}

fn default_base_color() -> [f32; 4] {
    [0.8, 0.8, 0.8, 1.0]
}
fn default_roughness() -> f32 {
    0.5
}
fn default_one() -> f32 {
    1.0
}
fn default_ior() -> f32 {
    1.5
}

/// Imports `.mat.json` material definition files.
///
/// Produces an RMATI intermediate blob (magic + version + scalars + 5 texture
/// path records) and resolves the texture paths to `AssetId` dependencies via
/// `ctx.db.id_by_path`. Textures that don't resolve are dropped with a warning
/// (the material still imports; the slot is left empty at runtime).
///
/// Intermediate format (all little-endian):
/// ```text
/// [magic:5]   b"RMATI"
/// [version:1] 1
/// [scalars]   base_color[4] + metallic + roughness + emissive[3]
///             + emissive_strength + normal_scale + occlusion_strength
///             + transmission + ior + translucency + anisotropy
///             + clearcoat + clearcoat_roughness   (each f32 LE, 18 floats total)
/// per slot (5x):
///   [present:1]   0 or 1
///   [if present]  [path_len:u16][path bytes UTF-8]
/// ```
pub struct MaterialImporter;

impl MaterialImporter {
    /// Serialize scalars + alpha semantics + 5 texture-path records into the
    /// RMATI blob.
    fn write_intermediate(mat: &MaterialJson, tex_paths: &[Option<String>; 5]) -> Vec<u8> {
        use prism_asset::formats::{MaterialAlphaMode, DEFAULT_ALPHA_CUTOFF};
        let mut buf = Vec::with_capacity(64);
        buf.extend_from_slice(MATERIAL_INTERMEDIATE_MAGIC);
        buf.push(crate::cooker::MATERIAL_INTERMEDIATE_VERSION);

        buf.extend_from_slice(&mat.base_color[0].to_le_bytes());
        buf.extend_from_slice(&mat.base_color[1].to_le_bytes());
        buf.extend_from_slice(&mat.base_color[2].to_le_bytes());
        buf.extend_from_slice(&mat.base_color[3].to_le_bytes());
        buf.extend_from_slice(&mat.metallic.to_le_bytes());
        buf.extend_from_slice(&mat.roughness.to_le_bytes());
        buf.extend_from_slice(&mat.emissive[0].to_le_bytes());
        buf.extend_from_slice(&mat.emissive[1].to_le_bytes());
        buf.extend_from_slice(&mat.emissive[2].to_le_bytes());
        buf.extend_from_slice(&mat.emissive_strength.to_le_bytes());
        buf.extend_from_slice(&mat.normal_scale.to_le_bytes());
        buf.extend_from_slice(&mat.occlusion_strength.to_le_bytes());
        buf.extend_from_slice(&mat.transmission.to_le_bytes());
        buf.extend_from_slice(&mat.ior.to_le_bytes());
        buf.extend_from_slice(&mat.translucency.to_le_bytes());
        buf.extend_from_slice(&mat.anisotropy.to_le_bytes());
        buf.extend_from_slice(&mat.clearcoat.to_le_bytes());
        buf.extend_from_slice(&mat.clearcoat_roughness.to_le_bytes());

        let alpha_mode = mat
            .alpha_mode
            .as_deref()
            .map(MaterialAlphaMode::from_name)
            .unwrap_or(MaterialAlphaMode::Mask);
        buf.push(alpha_mode.as_u8());
        buf.extend_from_slice(
            &mat.alpha_cutoff
                .unwrap_or(DEFAULT_ALPHA_CUTOFF)
                .to_le_bytes(),
        );

        for slot in tex_paths {
            match slot {
                Some(path) => {
                    let bytes = path.as_bytes();
                    buf.push(1);
                    let len = bytes.len().min(u16::MAX as usize) as u16;
                    buf.extend_from_slice(&len.to_le_bytes());
                    buf.extend_from_slice(&bytes[..len as usize]);
                }
                None => buf.push(0),
            }
        }

        buf
    }
}

impl Importer for MaterialImporter {
    fn name(&self) -> &'static str {
        "material-importer"
    }

    fn version(&self) -> u32 {
        1
    }

    fn can_import(&self, path: &Path) -> bool {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        if name.ends_with(".mat.json") {
            return true;
        }
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("mat"))
    }

    fn import(&self, ctx: &ImportContext) -> Result<ImportResult, ImportError> {
        let text = std::fs::read_to_string(&ctx.source_path)?;
        let mat: MaterialJson = serde_json::from_str(&text)
            .map_err(|e| ImportError::ImportFailed(format!("material JSON parse failed: {e}")))?;

        let raw_paths: [Option<String>; 5] = [
            mat.albedo_tex.clone(),
            mat.normal_tex.clone(),
            mat.metallic_roughness_tex.clone(),
            mat.emissive_tex.clone(),
            mat.occlusion_tex.clone(),
        ];

        let mut dependencies: Vec<AssetId> = Vec::new();
        let mut resolved_paths: [Option<String>; 5] = [None, None, None, None, None];
        for (i, path_opt) in raw_paths.iter().enumerate() {
            if let Some(path) = path_opt {
                let normalized = path.replace('\\', "/");
                match ctx.db.id_by_path(&normalized) {
                    Some(id) => {
                        dependencies.push(id);
                        resolved_paths[i] = Some(normalized);
                    }
                    None => {
                        tracing::warn!(
                            "material importer: texture '{}' not in DB (slot '{}'); leaving empty",
                            normalized,
                            MATERIAL_TEX_SLOTS[i]
                        );
                    }
                }
            }
        }

        let output_data = Self::write_intermediate(&mat, &resolved_paths);

        let metadata = serde_json::json!({
            "name": mat.name.clone().unwrap_or_default(),
            "texture_slots": resolved_paths.iter().map(|p| p.is_some()).collect::<Vec<_>>(),
            "dependency_count": dependencies.len(),
        });

        Ok(ImportResult {
            asset_type: AssetType::Material,
            dependencies,
            output_data,
            metadata: Some(metadata),
        })
    }
}

/// Intermediate shader format magic: "RSLI" (Resource Slang Intermediate).
const SHADER_INTERMEDIATE_MAGIC: &[u8; 4] = b"RSLI";

/// Infer the Slang entry-point name and stage from a source filename.
///
/// Convention (matches `shaders/compile.sh`):
/// - `*_vert.slang`  -> `vertexMain`   / `vertex`
/// - `*_frag.slang`  -> `fragmentMain` / `fragment`
/// - `*_comp.slang`  -> `computeMain`  / `compute`
/// - `*_geom.slang`  -> `geometryMain` / `geometry`
/// - `*_hull.slang`  -> `hullMain`     / `hull`
/// - `*_domain.slang`-> `domainMain`   / `domain`
/// - `pt_*.slang` / `gi_*.slang` (compute) -> `ptMain` / `compute`
///
/// Returns `None` for unrecognized names; the caller falls back to defaults.
fn infer_entry_stage_from_name(file_stem: &str) -> Option<(&'static str, &'static str)> {
    let stem = file_stem.to_lowercase();
    if stem.ends_with("_vert") {
        Some(("vertexMain", "vertex"))
    } else if stem.ends_with("_frag") {
        Some(("fragmentMain", "fragment"))
    } else if stem.ends_with("_comp") {
        Some(("computeMain", "compute"))
    } else if stem.ends_with("_geom") {
        Some(("geometryMain", "geometry"))
    } else if stem.ends_with("_hull") {
        Some(("hullMain", "hull"))
    } else if stem.ends_with("_domain") {
        Some(("domainMain", "domain"))
    } else if stem.starts_with("pt_") {
        Some(("ptMain", "compute"))
    } else {
        None
    }
}

/// Look up `key` in the importer settings JSON (an object), returning its
/// string value if present.
fn setting_str(settings: &Value, key: &str) -> Option<String> {
    settings
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned())
}

/// Imports `.slang` shader source files.
///
/// Produces an RSLI intermediate blob that carries the entry-point name,
/// stage, profile, and raw source bytes. The cooker later feeds these to
/// `slangc` to produce SPIR-V
///
/// Entry-point / stage / profile are resolved in this priority order:
/// 1. `settings["slang_entry"]` / `["slang_stage"]` / `["slang_profile"]`
///    (per-asset import overrides passed by the editor or CLI).
/// 2. Filename convention (see [`infer_entry_stage_from_name`]).
/// 3. Defaults: entry = `vertexMain`, stage = `vertex`, profile = `spirv_1_5`.
///
/// Intermediate format (all little-endian):
/// ```text
/// [magic:4]        b"RSLI"
/// [version:1]      1
/// [entry_len:u16]  + entry bytes (UTF-8)
/// [stage_len:u16]  + stage bytes (UTF-8)
/// [profile_len:u16]+ profile bytes (UTF-8)
/// [source_len:u32] + source bytes (raw .slang content)
/// ```
pub struct ShaderImporter;

impl ShaderImporter {
    fn write_intermediate(entry: &str, stage: &str, profile: &str, source: &[u8]) -> Vec<u8> {
        let entry_b = entry.as_bytes();
        let stage_b = stage.as_bytes();
        let profile_b = profile.as_bytes();
        let cap =
            4 + 1 + 2 + entry_b.len() + 2 + stage_b.len() + 2 + profile_b.len() + 4 + source.len();
        let mut buf = Vec::with_capacity(cap);
        buf.extend_from_slice(SHADER_INTERMEDIATE_MAGIC);
        buf.push(1);

        let entry_len = entry_b.len().min(u16::MAX as usize) as u16;
        buf.extend_from_slice(&entry_len.to_le_bytes());
        buf.extend_from_slice(&entry_b[..entry_len as usize]);

        let stage_len = stage_b.len().min(u16::MAX as usize) as u16;
        buf.extend_from_slice(&stage_len.to_le_bytes());
        buf.extend_from_slice(&stage_b[..stage_len as usize]);

        let profile_len = profile_b.len().min(u16::MAX as usize) as u16;
        buf.extend_from_slice(&profile_len.to_le_bytes());
        buf.extend_from_slice(&profile_b[..profile_len as usize]);

        buf.extend_from_slice(&(source.len() as u32).to_le_bytes());
        buf.extend_from_slice(source);
        buf
    }
}

impl Importer for ShaderImporter {
    fn name(&self) -> &'static str {
        "shader-importer"
    }

    fn version(&self) -> u32 {
        1
    }

    fn can_import(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("slang"))
    }

    fn import(&self, ctx: &ImportContext) -> Result<ImportResult, ImportError> {
        let source = std::fs::read(&ctx.source_path)?;

        let file_stem = ctx
            .source_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        let (default_entry, default_stage) =
            infer_entry_stage_from_name(file_stem).unwrap_or(("vertexMain", "vertex"));
        let entry =
            setting_str(&ctx.settings, "slang_entry").unwrap_or_else(|| default_entry.to_owned());
        let stage =
            setting_str(&ctx.settings, "slang_stage").unwrap_or_else(|| default_stage.to_owned());
        let profile =
            setting_str(&ctx.settings, "slang_profile").unwrap_or_else(|| "spirv_1_5".to_owned());

        let output_data = Self::write_intermediate(&entry, &stage, &profile, &source);

        let metadata = serde_json::json!({
            "entry": entry,
            "stage": stage,
            "profile": profile,
            "source_size": source.len(),
        });

        Ok(ImportResult {
            asset_type: AssetType::Shader,
            dependencies: Vec::new(),
            output_data,
            metadata: Some(metadata),
        })
    }
}

/// Build the default importer registry with all built-in importers.
pub fn default_importer_registry() -> ImporterRegistry {
    let mut reg = ImporterRegistry::new();
    reg.register(Box::new(TextureImporter));
    reg.register(Box::new(GltfImporter));
    reg.register(Box::new(ObjImporter));
    reg.register(Box::new(MaterialImporter));
    reg.register(Box::new(ShaderImporter));
    reg.register(Box::new(AnimGraphImporter));
    reg.register(Box::new(JsonImporter));
    reg.register(Box::new(RawImporter));
    reg
}

fn normalize_relative_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn walk_directory(dir: &Path, cb: &mut impl FnMut(PathBuf)) {
    if !dir.is_dir() {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk_directory(&path, cb);
            } else if path.is_file() {
                cb(path);
            }
        }
    }
}

#[cfg(test)]
mod tests;
