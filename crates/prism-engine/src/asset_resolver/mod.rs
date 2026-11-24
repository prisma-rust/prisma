//! GPU asset parser---load + cache mesh/material/texture resources from `.pak` package on demand.
//!
//! Holds [`ResourceManager`] (runtime resource database) and three type-specific caches (ensuring same mesh/material/texture
//! not heavily duplicated during transmission), and provides [`GpuAssetResolver::prepare_requests`] method to query ECS world in
//! pending [`MeshRenderer`] entities, generate GPU upload requests.
//!
//! ## async model (decoupled from render thread)
//!
//! Resource parsing split into two segments, because `&mut World` (main thread) and `&mut GraphRenderer` (render thread)
//! cannot be on same thread:
//! - **CPU segment (main thread)**: [`prepare_requests`] queries pending-parse entities, loads `.pak`, deinterleaves vertex/
//!   texture pixels, produces pure-data [`AssetResolveRequest`]. Does not touch renderer.
//! - **GPU segment (render thread)**: `GraphRenderer::apply_asset_requests` executes upload and returns handle.
//! - Main thread again consumes results, writes handle back to `MeshRef`/`MaterialRef`.
//!
//! This way, when renderer builds asynchronously on render thread at startup, main thread can still handle window events (disable/move/scale).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use prism_asset::runtime::ResourceManager;
use prism_ecs::Entity;
use prism_ecs::World;
use prism_render::asset_bridge::{AssetRelease, AssetResolveRequest, MaterialResolveData};
use prism_render::managers::{MeshUploadInput, SkinAttributes};

use crate::scene::components::{
    MaterialRef, MeshRef, MeshRenderer, SpriteRef, SpriteRenderer, Tilemap,
};
use crate::scene::runtime_components::{
    MaterialSlots, MeshSubmesh, MeshSubmeshes, ShaderRef, TilemapRef, UiImageRef,
};
use crate::scene::systems::render::MeshBounds;

mod decoders;
#[cfg(test)]
mod tests;

/// Asset slot bit, used as key for "failure cache".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum AssetSlot {
    Mesh,
    Material,
    Shader,
    Sprite,
    /// Tile map graph tileset (#529).
    Tileset,
    /// UI `Style::image` background graph (#591).
    UiImage,
}

/// A mesh's CPU upload data + submesh table (#520).
struct LoadedMesh {
    input: MeshUploadInput,
    submeshes: Vec<MeshSubmesh>,
}

/// Skinned mesh CPU segment product: vertex data + skin property uploaded together (#560 P0-3b).
struct LoadedSkinnedMesh {
    input: MeshUploadInput,
    submeshes: Vec<MeshSubmesh>,
    skin: SkinAttributes,
}

/// Resources an entity needs to parse this frame (only contains still-missing halves).
struct PendingRequest {
    entity: Entity,
    mesh_path: Option<String>,
    /// Authoring component fast reference: many material paths need to wait for mesh load then expand by submesh count.
    renderer: MeshRenderer,
    /// Whether this frame needs to parse material (`MaterialRef.generation == 0`).
    wants_material: bool,
    /// Optional custom fragment shader path (`MeshRenderer.shader_path`).
    shader_path: Option<String>,
    /// Sprite graph / tile map graph tileset / UI background graph: all three only need one graph,
    /// One entity in same frame will only submit one of them.
    sprite_path: Option<String>,
    tileset: bool,
    /// `sprite_path` is UI `Style::image` (#591).
    ui_image: bool,
    /// Skinned mesh path (#560 P0-3b): `.rskin` takes a different decode path,
    /// produces `SkinAttributes` when same.
    skinned_path: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct AtlasSpriteMeta {
    uv_rect: [f32; 4],
    trimmed_offset: [i32; 2],
    original_size: [u32; 2],
    size: [u32; 2],
}

#[derive(Debug, Clone, Copy)]
struct CachedTextureEntry {
    texture_slot: u32,
    size: [u32; 2],
}

#[derive(Debug, Clone)]
struct CachedAtlasEntry {
    texture_path: String,
    meta: AtlasSpriteMeta,
}

#[derive(Debug, Clone)]
struct TextureWaiter {
    entity: Entity,
    slot: AssetSlot,
    atlas_meta: Option<AtlasSpriteMeta>,
}

/// On-demand asset loader + GPU uploader + cache.
///
/// Call [`prepare_requests`] every frame to collect pending entities and produce upload requests (CPU segment);
/// GPU upload completed asynchronously by render thread.
pub struct GpuAssetResolver {
    /// Already successfully mounted `.pak` (build stamp uniformly verified at first `prepare_requests`, #976).
    mounted_paks: Vec<PathBuf>,
    /// Build stamp reported only once: `App::build` phase logger not yet installed, logs will be dropped.
    stamps_checked: bool,
    pub resource_manager: ResourceManager,
    /// Entities already submitted upload requests (dedup: avoid heavy re-enqueue every frame until result written back to World).
    enqueued: HashSet<Entity>,
    /// Slot bits already attempted and failed: `(entity, slot bit) -> path at failure time`.
    ///
    /// Path not in manifest, decode failure class error will not "try once and be good", heavy retry every frame only will incorrectly duplicate.
    /// reads `.pak` and flushes logs. Cache keyed by path: when path is rewritten (change material, hot reload swap package),
    /// key no longer matches, will automatically heavy retry.
    failed: HashMap<(Entity, AssetSlot), String>,
    /// Most recently parsed sprite path. Project only changes `sprite_path`, if this finds inconsistency then heavy re-parse,
    /// not again requiring caller to manually clear `SpriteRef::generation` to zero.
    sprite_paths: HashMap<Entity, String>,
    /// Most recently parsed tileset path (#529), semantics same as [`Self::sprite_paths`].
    tileset_paths: HashMap<Entity, String>,
    /// Most recently parsed UI background graph path (`Style::image`, #591).
    ui_image_paths: HashMap<Entity, String>,
    /// Last resolved custom fragment shader path per entity.
    shader_paths: HashMap<Entity, String>,
    /// Staged custom shader paths/ids waiting for upload completion.
    pending_shader_meta: HashMap<Entity, (String, prism_scene::SceneAssetId)>,
    /// Project font paths already handed to renderer (#583).
    ///
    /// Font registers entire file into UI atlas, not like graph per entity; same path only read once.
    fonts_loaded: HashSet<String>,
    /// Font path read but failed: not heavy retry every frame (same principle as [`Self::failed`]).
    fonts_failed: HashSet<String>,
    /// GPU slot bit/handle already written into component inside, accounted by `(entity, slot bit)` (#495).
    ///
    /// When component destroyed, path rewritten, `generation` zeroed (hot reload), VRAM reclaimed accordingly;
    /// Before these resources are overwritten by new upload, old graph/image/mesh forever stay in GPU.
    resolved: HashMap<(Entity, AssetSlot), AssetRelease>,
    /// Which slot bit this entity's graph request belongs to (`Sprite` / `Tileset` / `UiImage`).
    ///
    /// When result returned, only has one `texture_slot`, must know which component it correctly belongs to in order to account.
    pending_texture_slot: HashMap<Entity, AssetSlot>,
    /// Release requests pending host forwarding to renderer.
    releases: Vec<AssetRelease>,
    /// Mesh local bounding box already submitted but handle not yet written back (`entity → AABB`).
    ///
    /// CPU segment already has vertex position in hand, so bounding box calculated when upload request sent; handle comes back
    /// (after [`Self::apply_results`]) together with same handle written into [`MeshBounds`], render extraction
    /// can cull by main camera (#491).
    pending_bounds: HashMap<Entity, prism_render::culling::Aabb>,
    /// Submesh table already submitted but handle not yet written back (#520).
    pending_submeshes: HashMap<Entity, Vec<MeshSubmesh>>,
    /// Each submesh uses which unique material index in this request (`None` = default material).
    ///
    /// Submeshes with same path share one material upload, returned slot bit expanded using this table.
    pending_material_map: HashMap<Entity, Vec<Option<usize>>>,
    /// Staged atlas sub-sprite metadata waiting for texture upload completion (#1158).
    pending_atlas_sprites: HashMap<Entity, AtlasSpriteMeta>,
    /// Shared GPU texture slot cache keyed by backing texture path.
    texture_cache: HashMap<String, CachedTextureEntry>,
    /// Decoded `.ratlas#sprite` metadata cache.
    atlas_cache: HashMap<String, CachedAtlasEntry>,
    /// Reference counts per bindless texture slot across `resolved` entries.
    slot_refcounts: HashMap<u32, usize>,
    /// Backing texture path for each entity that initiated an async texture upload.
    pending_texture_path: HashMap<Entity, String>,
    /// Backing texture paths currently in flight between `prepare_requests` and `apply_results`.
    inflight_texture_paths: HashSet<String>,
    /// Additional entities waiting on an already-in-flight backing texture upload.
    texture_waiters: HashMap<String, Vec<TextureWaiter>>,
    /// `optional_*.pak` already mounted on demand (#509).
    ///
    /// Heavy re-mounting same package only makes `ResourceManager` warn each asset ID as heavy duplicate
    /// once, so record per path, `load_optional_packages` can safely call every close.
    mounted_optional: HashSet<PathBuf>,
}

/// Reference breakage reported only once (each path), and is `error` not `warn`.
///
/// Path not in resource package means document inside reference already broken (file deleted / renamed / misspelled).
/// After result object entirely not appear or graph turns white, should not be quietly downgraded by log level; but same broken reference each
/// entity needs to parse once, every flushes one line will drown Console, so heavy dedup by path (#467).
fn report_missing_reference(path: &str, kind: &str) {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};

    static REPORTED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let reported = REPORTED.get_or_init(|| Mutex::new(HashSet::new()));
    let Ok(mut reported) = reported.lock() else {
        return;
    };
    if !reported.insert(path.to_string()) {
        return;
    }
    log::error!(
        "broken asset reference: {kind} '{path}' is not in the resource package          (deleted, renamed, or misspelt?)"
    );
}

impl GpuAssetResolver {
    pub fn new() -> Self {
        Self {
            mounted_paks: Vec::new(),
            stamps_checked: false,
            resource_manager: ResourceManager::new(),
            enqueued: HashSet::new(),
            failed: HashMap::new(),
            sprite_paths: HashMap::new(),
            tileset_paths: HashMap::new(),
            ui_image_paths: HashMap::new(),
            shader_paths: HashMap::new(),
            pending_shader_meta: HashMap::new(),
            fonts_loaded: HashSet::new(),
            fonts_failed: HashSet::new(),
            resolved: HashMap::new(),
            pending_texture_slot: HashMap::new(),
            releases: Vec::new(),
            pending_bounds: HashMap::new(),
            pending_submeshes: HashMap::new(),
            pending_material_map: HashMap::new(),
            pending_atlas_sprites: HashMap::new(),
            texture_cache: HashMap::new(),
            atlas_cache: HashMap::new(),
            slot_refcounts: HashMap::new(),
            pending_texture_path: HashMap::new(),
            inflight_texture_paths: HashSet::new(),
            texture_waiters: HashMap::new(),
            mounted_optional: HashSet::new(),
        }
    }

    /// Collect project bundled fonts used in scene and read bytes (#583).
    ///
    /// Return **(newly read this time)** `(asset path, font bytes)`; caller hands it to
    /// `GraphRenderer::register_ui_font`. Already submitted paths will not be heavy-duplicated, read failure
    /// paths will not heavy retry every frame (same strategy as mesh/graph failure cache).
    ///
    /// Fonts go through `.pak` path list (`AssetType::Font`), so packaged product and editor inside
    /// see the same file.
    pub fn prepare_ui_fonts(&mut self, world: &World) -> Vec<(String, Vec<u8>)> {
        let _span = crate::util::timing::span("asset::prepare_ui_fonts");
        let mut paths: Vec<String> = Vec::new();
        for (_, text) in world.query::<crate::ui::components::Text>() {
            let path = text.font.trim();
            if path.is_empty()
                || self.fonts_loaded.contains(path)
                || self.fonts_failed.contains(path)
                || paths.iter().any(|known| known == path)
            {
                continue;
            }
            paths.push(path.to_string());
        }

        let mut out = Vec::new();
        for path in paths {
            let Some(id) = self.resource_manager.id_by_path(&path) else {
                report_missing_reference(&path, "font");
                self.fonts_failed.insert(path);
                continue;
            };
            match self.resource_manager.load_with_deps_raw(id) {
                Ok(bytes) if !bytes.is_empty() => {
                    log::info!("ui fonts: loaded {} ({} bytes)", path, bytes.len());
                    self.fonts_loaded.insert(path.clone());
                    out.push((path, bytes));
                }
                Ok(_) => {
                    log::warn!("ui fonts: '{path}' is empty — ignoring it");
                    self.fonts_failed.insert(path);
                }
                Err(error) => {
                    log::warn!("ui fonts: load '{path}' failed: {error}");
                    self.fonts_failed.insert(path);
                }
            }
        }
        out
    }

    /// Record below "which GPU resource this entity's slot bit is currently using".
    ///
    /// When overwriting old value if resource changed (heavy re-upload to new slot bit), old one immediately queued for release
    /// --- component rewritten in same frame will not have "still sampling already released slot bit" inter-frame issue.
    fn record_resolved(&mut self, entity: Entity, slot: AssetSlot, value: AssetRelease) {
        if value.is_empty() {
            return;
        }
        let white_slot = prism_render::bindless::TextureHandle::WHITE.0;
        if let Some(new_tex) = value.texture_slot {
            if new_tex != white_slot {
                *self.slot_refcounts.entry(new_tex).or_insert(0) += 1;
            }
        }
        if let Some(previous) = self.resolved.insert((entity, slot), value.clone()) {
            if previous != value {
                self.release_entry(previous);
            } else if let Some(tex_slot) = value.texture_slot {
                if tex_slot != white_slot {
                    if let Some(count) = self.slot_refcounts.get_mut(&tex_slot) {
                        *count = count.saturating_sub(1);
                    }
                }
            }
        }
    }

    /// Decrement texture slot refcounts and queue GPU release only when no active owner remains.
    fn release_entry(&mut self, mut release: AssetRelease) {
        let white_slot = prism_render::bindless::TextureHandle::WHITE.0;
        if let Some(tex_slot) = release.texture_slot {
            let should_release = if tex_slot == white_slot {
                false
            } else if let Some(count) = self.slot_refcounts.get_mut(&tex_slot) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.slot_refcounts.remove(&tex_slot);
                    true
                } else {
                    false
                }
            } else {
                true
            };
            if should_release {
                self.texture_cache
                    .retain(|_, cached| cached.texture_slot != tex_slot);
            } else {
                release.texture_slot = None;
            }
        }
        if !release.is_empty() {
            self.releases.push(release);
        }
    }

    /// Whether this frame this entity's slot bit already no longer needs GPU resource.
    ///
    /// Judgment standard only two conditions, both are "certainly will not draw again": component carrying it is gone, or
    /// `generation == 0` (`invalidate_*` just marked dirty, immediately will heavy re-upload).
    fn slot_is_stale(&self, world: &World, entity: Entity, slot: AssetSlot) -> bool {
        use crate::scene::runtime_components::{TilemapRef, UiImageRef};
        match slot {
            AssetSlot::Mesh => {
                world.get::<MeshRenderer>(entity).is_none()
                    || world
                        .get::<MeshRef>(entity)
                        .map(|r| r.generation == 0)
                        .unwrap_or(true)
            }
            AssetSlot::Material => {
                world.get::<MeshRenderer>(entity).is_none()
                    || world
                        .get::<MaterialRef>(entity)
                        .map(|r| r.generation == 0)
                        .unwrap_or(true)
            }
            AssetSlot::Shader => {
                (world
                    .get::<MeshRenderer>(entity)
                    .is_none_or(|r| r.shader_path.is_empty())
                    && world
                        .get::<prism_scene::components::SkinnedMeshRenderer>(entity)
                        .is_none_or(|r| r.shader_path.is_empty()))
                    || world
                        .get::<ShaderRef>(entity)
                        .is_none_or(|r| r.generation == 0)
            }
            AssetSlot::Sprite => {
                world.get::<SpriteRenderer>(entity).is_none()
                    || world
                        .get::<SpriteRef>(entity)
                        .map(|r| r.generation == 0)
                        .unwrap_or(true)
            }
            AssetSlot::Tileset => {
                world.get::<Tilemap>(entity).is_none()
                    || world
                        .get::<TilemapRef>(entity)
                        .map(|r| r.generation == 0)
                        .unwrap_or(true)
            }
            AssetSlot::UiImage => {
                world
                    .get::<crate::ui::components::Style>(entity)
                    .map(|style| style.image.is_empty())
                    .unwrap_or(true)
                    || world
                        .get::<UiImageRef>(entity)
                        .map(|r| r.generation == 0)
                        .unwrap_or(true)
            }
        }
    }

    /// Queue already invalidated GPU resources for release (handed to host by [`Self::take_releases`]).
    fn release_stale_assets(&mut self, world: &World) {
        let stale: Vec<(Entity, AssetSlot)> = self
            .resolved
            .keys()
            .filter(|(entity, slot)| self.slot_is_stale(world, *entity, *slot))
            .copied()
            .collect();
        for key in stale {
            if let Some(release) = self.resolved.remove(&key) {
                self.release_entry(release);
            }
        }
        self.pending_texture_slot.retain(|entity, _| {
            self.resolved.keys().any(|(e, _)| e == entity)
                || world.get::<SpriteRenderer>(*entity).is_some()
                || world.get::<Tilemap>(*entity).is_some()
                || world.get::<crate::ui::components::Style>(*entity).is_some()
        });
        self.pending_atlas_sprites
            .retain(|entity, _| world.get::<SpriteRenderer>(*entity).is_some());
    }

    /// Scene switch / unload after immediately discard cache of entities no longer present (#482).
    ///
    /// Equivalent to cleanup at head of `prepare_requests`, but can be explicitly called, so scene switch when frame just puts
    /// old world slot bits returned to renderer, and clears `enqueued` / failure cache keys incorrectly not on.
    pub fn forget_missing_entities(&mut self, world: &World) {
        self.release_stale_assets(world);
        self.enqueued.retain(|entity| {
            world.get::<MeshRenderer>(*entity).is_some()
                || world.get::<SpriteRenderer>(*entity).is_some()
                || world.get::<Tilemap>(*entity).is_some()
                || world
                    .get::<crate::ui::components::Style>(*entity)
                    .is_some_and(|style| !style.image.is_empty())
        });
        self.failed.retain(|(entity, _), _| {
            world.get::<MeshRenderer>(*entity).is_some()
                || world.get::<SpriteRenderer>(*entity).is_some()
                || world.get::<Tilemap>(*entity).is_some()
                || world
                    .get::<crate::ui::components::Style>(*entity)
                    .is_some_and(|style| !style.image.is_empty())
        });
    }

    /// Take GPU resources pending release; host forwards them to renderer (`GraphRenderer::release_assets`).
    ///
    /// Called after `prepare_requests`: invalidated slot bits recycled within this frame, heavy re-upload
    /// requests same time enqueued, so hot reload will not occupy VRAM multiple times each time.
    pub fn take_releases(&mut self) -> Vec<AssetRelease> {
        std::mem::take(&mut self.releases)
    }

    /// Whether this entity's slot bit already failed due to same path before.
    fn failed_before(&self, entity: Entity, slot: AssetSlot, path: &str) -> bool {
        self.failed
            .get(&(entity, slot))
            .is_some_and(|failed_path| failed_path == path)
    }

    /// Remember a load failure: path not in manifest / decode failure will not become good because below one frame retries,
    /// cache tries same path only once (automatically heavy retries when path rewritten).
    fn record_failure(&mut self, entity: Entity, slot: AssetSlot, path: &str) {
        log::warn!(
            "asset_resolver: {slot:?} '{path}' failed to load; \
             not retrying until the path changes"
        );
        self.failed.insert((entity, slot), path.to_string());
    }

    fn fatal_pak_error(msg: impl std::fmt::Display) -> ! {
        log::error!("{msg}");
        eprintln!("FATAL: {msg}");
        std::process::exit(1);
    }

    /// Attempt to load the `.pak` resource package and its path manifest.
    ///
    /// In dev/hosted builds, missing packages warn and continue with procedural geometry.
    /// Under [`crate::asset_paths::strict_pak_only`] (release / dist player builds),
    /// missing or invalid `.pak` files immediately report an error and exit without
    /// touching `.pak.meta.json` or loose files.
    pub fn load_resource_package(&mut self) {
        let mut span = crate::util::timing::span("asset::load_resource_package");
        let strict = crate::asset_paths::strict_pak_only();
        let Some(asset_dir) = crate::asset_paths::asset_dir() else {
            if strict {
                Self::fatal_pak_error("resource package unavailable: no `assets/` directory found");
            }
            log::warn!("resource package unavailable: no `assets/` directory found");
            return;
        };
        let paks = crate::asset_paths::pak_files_in(&asset_dir);
        if paks.is_empty() {
            if strict {
                Self::fatal_pak_error(format!(
                    "resource package unavailable: no `.pak` found in {}",
                    asset_dir.display()
                ));
            }
            log::warn!(
                "resource package unavailable: no `.pak` in {}; scene assets cannot load. \
                 Build the package, then restart: {}",
                asset_dir.display(),
                crate::asset_paths::build_scenes_command(&asset_dir)
            );
            return;
        }

        let loaded = self.mount_packages(&paks);
        if strict && loaded.is_empty() {
            Self::fatal_pak_error(format!(
                "no resource packages could be mounted from {}",
                asset_dir.display()
            ));
        }
        span.detail(format!(
            "paks={} mounted={} assets={}",
            paks.len(),
            loaded.len(),
            self.resource_manager.asset_count()
        ));

        log::info!(
            "resource packages loaded from {}: {} package(s), {} assets registered",
            asset_dir.display(),
            loaded.len(),
            self.resource_manager.asset_count(),
        );
    }

    /// #509: Mount optional packages under `assets/optional/` on demand (`optional_*.pak`).
    ///
    /// Auto-mount only scans `assets/` top layer, so optional package bytes will not reside at startup. Enter
    /// need these resources' close card / gameplay explicitly call; already mounted packages will be skipped, heavy duplicate calls safe.
    ///
    /// Returns **newly mounted this time** package count (directory does not exist, empty, or all already mounted = 0).
    pub fn load_optional_packages(&mut self) -> usize {
        let Some(dir) = crate::asset_paths::optional_pak_dir() else {
            return 0;
        };
        let paks: Vec<PathBuf> = crate::asset_paths::pak_files_in(&dir)
            .into_iter()
            .filter(|pak| !self.mounted_optional.contains(pak))
            .collect();
        if paks.is_empty() {
            log::debug!("no un-mounted optional packages in {}", dir.display());
            return 0;
        }

        let loaded = self.mount_packages(&paks);
        let count = loaded.len();
        self.mounted_optional.extend(loaded);
        log::info!(
            "optional resource packages loaded from {}: {count} package(s), {} assets registered",
            dir.display(),
            self.resource_manager.asset_count(),
        );
        count
    }

    /// #509: After newly mounting optional package, let entities failing because "path not in package inside" heavy retry once.
    ///
    /// Failure cache originally "same bad path reported only once", but optional package is late-mounted: before mounting
    /// parsed entities already recorded failure. This inside only cleans up entities for paths **currently parseable**,
    /// not touching other failure items. Returns number of entities heavy re-queued.
    pub fn retry_failed_after_package_change(&mut self, world: &mut World) -> usize {
        let retry: Vec<Entity> = self
            .failed
            .iter()
            .filter(|(_, path)| self.resource_manager.id_by_path(path).is_some())
            .map(|((entity, _), _)| *entity)
            .collect();
        let mut retried = 0usize;
        for entity in retry {
            self.reset_entity_refs(world, entity);
            retried += 1;
        }
        if retried > 0 {
            log::info!(
                "resource packages: {retried} entity(ies) will retry assets from a newly mounted package"
            );
        }
        retried
    }

    /// Load a list of `.pak` files plus their `<name>.pak.meta.json` manifests (when not strict pak-only).
    /// Returns the packages that were registered (in dev mode failed ones are skipped;
    /// under [`crate::asset_paths::strict_pak_only`] any failure exits immediately).
    fn mount_packages(&mut self, paks: &[PathBuf]) -> Vec<PathBuf> {
        let strict = crate::asset_paths::strict_pak_only();
        let mut loaded = Vec::new();
        for pak_path in paks {
            let mut pak_span = crate::util::timing::span("asset::mount_package");
            pak_span.detail(pak_path.display().to_string());
            if let Err(e) = self.resource_manager.load_package(pak_path) {
                if strict {
                    Self::fatal_pak_error(format!(
                        "resource package {} failed to load: {e}",
                        pak_path.display()
                    ));
                }
                log::warn!(
                    "resource package {} could not be loaded: {e}; \
                     assets from it will be missing",
                    pak_path.display()
                );
                continue;
            }
            loaded.push(pak_path.clone());
            if !self.mounted_paks.iter().any(|mounted| mounted == pak_path) {
                self.mounted_paks.push(pak_path.clone());
            }

            if strict {
                continue;
            }

            let manifest_path = pak_path.with_extension("pak.meta.json");
            if manifest_path.is_file() {
                let mut manifest_span = crate::util::timing::span("asset::load_path_manifest");
                manifest_span.detail(manifest_path.display().to_string());
                if let Err(e) = self.resource_manager.load_path_manifest(&manifest_path) {
                    log::warn!(
                        "failed to load path manifest {}: {e} \
                         (asset resolution by path won't work for this package)",
                        manifest_path.display()
                    );
                }
            } else {
                log::warn!(
                    "resource package {} has no path manifest ({})",
                    pak_path.display(),
                    manifest_path.display()
                );
            }
        }
        loaded
    }

    /// Validate build stamp inside package (#976): give clear hint when package expires, not silently draw wrong content.
    ///
    /// Three cases:
    /// - No stamp → package built by older pipeline, cannot judge freshness (warn, not block);
    /// - Schema fingerprint not same → engine changed component layout, old package can parse error data (error);
    /// - Project document newer than package → changed scene but forgot to repackage (error, with rebuild command).
    ///
    /// Uniformly validate already mounted packages once when first accessing resources (logger now ready).
    fn check_cook_stamps_once(&mut self) {
        if self.stamps_checked {
            return;
        }
        self.stamps_checked = true;
        let _span = crate::util::timing::span("asset::check_cook_stamps_once");
        let paks = std::mem::take(&mut self.mounted_paks);
        for pak in &paks {
            self.check_cook_stamp(pak);
        }
        self.mounted_paks = paks;
    }

    /// Pub version for hot reload: heavy repackage after still needs to validate once (#976).
    ///
    /// Three cases:
    /// - No stamp / stamp not readable → package built by older pipeline (warn, not block);
    /// - Schema fingerprint not same → engine changed component layout, old package can parse error data (error);
    /// - Project document newer than package → changed scene but forgot to repackage (error, with rebuild command).
    ///
    /// Note `App::build` phase also mounts packages, but logger not yet installed, so true positive validation
    /// deferred to first `prepare_requests` (see [`Self::check_cook_stamps_once`]).
    pub fn check_cook_stamp(&mut self, pak_path: &Path) {
        use prism_scene::cook_stamp as stamp_mod;
        let stamp = self
            .resource_manager
            .id_by_path(stamp_mod::COOK_STAMP_NAME)
            .and_then(|id| self.resource_manager.load_with_deps_raw(id).ok())
            .and_then(|bytes| stamp_mod::CookStamp::parse(&String::from_utf8_lossy(&bytes)));
        let strict = crate::asset_paths::strict_pak_only();
        let newest = if strict {
            0
        } else {
            let mut walk_span = crate::util::timing::span("asset::newest_source_mtime");
            let newest = pak_path
                .parent()
                .map(stamp_mod::newest_source_mtime)
                .unwrap_or(0);
            walk_span.detail(format!("newest={newest}"));
            newest
        };
        match stamp_mod::check(
            stamp,
            prism_scene::registry::builtin_schema_fingerprint(),
            newest,
        ) {
            stamp_mod::StampCheck::Fresh => {
                log::debug!("resource package {} is up to date", pak_path.display());
            }
            stamp_mod::StampCheck::Missing => {
                log::warn!(
                    "resource package {} has no cook stamp: it was built by an older pipeline, \
                     so its freshness cannot be verified. If the scene looks wrong, rebuild it: \
                     `prism-build-pipeline build-scenes`",
                    pak_path.display()
                );
            }
            stamp_mod::StampCheck::SchemaChanged { cooked, current } => {
                let msg = format!(
                    "resource package {} was cooked with component schema {cooked:#x}, this \
                     engine has {current:#x}: scene data may decode incorrectly. \
                     Rebuild with `prism-build-pipeline build-scenes`",
                    pak_path.display()
                );
                if strict {
                    Self::fatal_pak_error(msg);
                }
                log::error!("{msg}");
            }
            stamp_mod::StampCheck::SourcesNewer { cooked, newest } => {
                log::error!(
                    "resource package {} is older than the project documents (pak {cooked}, \
                     newest source {newest}): the scene will render stale content. \
                     Rebuild with `prism-build-pipeline build-scenes`",
                    pak_path.display()
                );
            }
        }
    }

    /// Load `.pak` and its path manifest from memory resources.
    pub fn load_resource_package_bytes(
        &mut self,
        pak_bytes: &[u8],
        path_manifest_json: Option<&str>,
    ) -> anyhow::Result<()> {
        self.resource_manager.load_package_bytes(pak_bytes)?;
        if let Some(manifest) = path_manifest_json {
            self.resource_manager
                .load_path_manifest_from_str(manifest)?;
        }
        Ok(())
    }

    /// Query ECS world pending [`MeshRenderer`] entities, load `.pak` + deinterleave,
    /// Produce World-fully-decoupled [`AssetResolveRequest`] (pure data, Send).
    ///
    /// Do not touch `GraphRenderer` --- GPU upload completed by render thread's `apply_asset_requests`.
    /// Returned requests handed to render thread by caller via [`prism_app::RenderShared`] channel.
    ///
    /// Returns quantity = number of entities newly submitted this frame. Already enqueued but results not yet written back entities will be heavy-dedup skipped.
    ///
    /// Each entity only carries the halves that are still missing: when the mesh
    /// resolved but the material is pending, the mesh is neither loaded nor
    /// uploaded again (the old implementation re-sent the whole request, which
    /// registered fresh GPU resources every frame). A slot that fails to load is
    /// recorded in the failure cache ([`GpuAssetResolver::failed`]), so one bad
    /// path is tried exactly once until someone rewrites it.
    /// Immediately resolve built-in white sprites and already-cached textures on the main thread
    /// so newly spawned entities render on the same frame without duplicate GPU uploads.
    pub fn resolve_cached_sprites(&mut self, world: &mut World) -> usize {
        enum CacheAction {
            White {
                entity: Entity,
                slot: AssetSlot,
                path: String,
            },
            Cached {
                entity: Entity,
                slot: AssetSlot,
                path: String,
                tex_slot: u32,
                size: [u32; 2],
                atlas_meta: Option<AtlasSpriteMeta>,
            },
        }

        let mut pending: Vec<(Entity, AssetSlot, String)> = Vec::new();
        for (entity, sprite) in world.query::<SpriteRenderer>() {
            if !sprite.has_sprite() {
                continue;
            }
            let unresolved = world
                .get::<SpriteRef>(entity)
                .map(|r| r.generation == 0)
                .unwrap_or(true);
            let path_changed = self
                .sprite_paths
                .get(&entity)
                .is_some_and(|last| last != &sprite.sprite_path);
            if unresolved || path_changed {
                pending.push((entity, AssetSlot::Sprite, sprite.sprite_path.clone()));
            }
        }
        for (entity, tilemap) in world.query::<Tilemap>() {
            if tilemap.tileset_path.is_empty() {
                continue;
            }
            let unresolved = world
                .get::<TilemapRef>(entity)
                .map(|r| r.generation == 0)
                .unwrap_or(true);
            let path_changed = self
                .tileset_paths
                .get(&entity)
                .is_some_and(|last| last != &tilemap.tileset_path);
            if unresolved || path_changed {
                pending.push((entity, AssetSlot::Tileset, tilemap.tileset_path.clone()));
            }
        }
        for (entity, style) in world.query::<crate::ui::components::Style>() {
            if style.image.is_empty() {
                continue;
            }
            let unresolved = world
                .get::<UiImageRef>(entity)
                .map(|r| r.generation == 0)
                .unwrap_or(true);
            let path_changed = self
                .ui_image_paths
                .get(&entity)
                .is_some_and(|last| last != &style.image);
            if unresolved || path_changed {
                pending.push((entity, AssetSlot::UiImage, style.image.clone()));
            }
        }

        let mut actions: Vec<CacheAction> = Vec::new();
        for (entity, slot, path) in pending {
            if path == prism_scene::BUILTIN_WHITE_SPRITE {
                actions.push(CacheAction::White { entity, slot, path });
                continue;
            }
            let Some((backing_path, atlas_meta)) = self.resolve_texture_target(&path) else {
                continue;
            };
            if let Some(&cached) = self.texture_cache.get(&backing_path) {
                actions.push(CacheAction::Cached {
                    entity,
                    slot,
                    path,
                    tex_slot: cached.texture_slot,
                    size: cached.size,
                    atlas_meta,
                });
            }
        }

        let count = actions.len();
        let white_slot = prism_render::bindless::TextureHandle::WHITE.0;
        for action in actions {
            match action {
                CacheAction::White { entity, slot, path } => {
                    if let Some(prev) = self.resolved.remove(&(entity, slot)) {
                        self.release_entry(prev);
                    }
                    self.write_texture_refs(world, entity, slot, white_slot, [1, 1], None);
                    self.record_slot_path(entity, slot, path);
                    self.enqueued.remove(&entity);
                    self.failed.remove(&(entity, slot));
                }
                CacheAction::Cached {
                    entity,
                    slot,
                    path,
                    tex_slot,
                    size,
                    atlas_meta,
                } => {
                    self.record_resolved(
                        entity,
                        slot,
                        AssetRelease {
                            texture_slot: Some(tex_slot),
                            ..AssetRelease::default()
                        },
                    );
                    self.write_texture_refs(world, entity, slot, tex_slot, size, atlas_meta);
                    self.record_slot_path(entity, slot, path);
                    self.enqueued.remove(&entity);
                    self.failed.remove(&(entity, slot));
                }
            }
        }
        count
    }

    fn record_slot_path(&mut self, entity: Entity, slot: AssetSlot, path: String) {
        match slot {
            AssetSlot::Sprite => {
                self.sprite_paths.insert(entity, path);
            }
            AssetSlot::Tileset => {
                self.tileset_paths.insert(entity, path);
            }
            AssetSlot::UiImage => {
                self.ui_image_paths.insert(entity, path);
            }
            AssetSlot::Mesh | AssetSlot::Material | AssetSlot::Shader => {}
        }
    }

    fn write_texture_refs(
        &self,
        world: &mut World,
        entity: Entity,
        slot: AssetSlot,
        tex_slot: u32,
        size: [u32; 2],
        atlas_meta: Option<AtlasSpriteMeta>,
    ) {
        if slot == AssetSlot::Sprite || world.get::<SpriteRenderer>(entity).is_some() {
            let mut sprite = world
                .get::<SpriteRef>(entity)
                .cloned()
                .unwrap_or_else(|| SpriteRef {
                    asset_id: crate::scene::components::SceneAssetId::generate(),
                    texture_slot: 0,
                    size: [0, 0],
                    generation: 0,
                    uv_rect: [0.0, 0.0, 1.0, 1.0],
                    trimmed_offset: [0, 0],
                    original_size: [0, 0],
                });
            sprite.texture_slot = tex_slot;
            sprite.generation = 1;
            if let Some(meta) = atlas_meta {
                sprite.uv_rect = meta.uv_rect;
                sprite.trimmed_offset = meta.trimmed_offset;
                sprite.original_size = meta.original_size;
                sprite.size = meta.size;
            } else {
                sprite.size = size;
                sprite.uv_rect = [0.0, 0.0, 1.0, 1.0];
                sprite.trimmed_offset = [0, 0];
                sprite.original_size = size;
            }
            world.insert(entity, sprite);
        }

        if world.get::<Tilemap>(entity).is_some() {
            let tilemap = world
                .get::<TilemapRef>(entity)
                .cloned()
                .unwrap_or_else(|| TilemapRef {
                    asset_id: crate::scene::components::SceneAssetId::generate(),
                    texture_slot: 0,
                    size: [0, 0],
                    generation: 0,
                });
            world.insert(
                entity,
                TilemapRef {
                    texture_slot: tex_slot,
                    size,
                    generation: 1,
                    ..tilemap
                },
            );
        }

        if world
            .get::<crate::ui::components::Style>(entity)
            .is_some_and(|style| !style.image.is_empty())
        {
            let image = world
                .get::<UiImageRef>(entity)
                .cloned()
                .unwrap_or_else(|| UiImageRef {
                    asset_id: crate::scene::components::SceneAssetId::generate(),
                    texture_slot: 0,
                    size: [0, 0],
                    generation: 0,
                });
            world.insert(
                entity,
                UiImageRef {
                    texture_slot: tex_slot,
                    size,
                    generation: 1,
                    ..image
                },
            );
        }
    }

    pub fn prepare_requests(&mut self, world: &World) -> Vec<(Entity, AssetResolveRequest)> {
        let mut span = crate::util::timing::span("asset::prepare_requests");
        self.check_cook_stamps_once();
        self.release_stale_assets(world);
        let procedural = world.get_resource::<crate::scene::systems::terrain::ProceduralMeshes>();
        let pending: Vec<PendingRequest> = {
            self.sprite_paths
                .retain(|entity, _| world.get::<SpriteRenderer>(*entity).is_some());
            self.pending_bounds
                .retain(|entity, _| world.get::<MeshRef>(*entity).is_some());
            self.failed.retain(|(entity, _), _| {
                world.get::<MeshRenderer>(*entity).is_some()
                    || world.get::<SpriteRenderer>(*entity).is_some()
                    || world.get::<Tilemap>(*entity).is_some()
                    || world
                        .get::<crate::ui::components::Style>(*entity)
                        .is_some_and(|style| !style.image.is_empty())
            });
            self.tileset_paths
                .retain(|entity, _| world.get::<Tilemap>(*entity).is_some());
            self.ui_image_paths.retain(|entity, _| {
                world
                    .get::<crate::ui::components::Style>(*entity)
                    .is_some_and(|style| !style.image.is_empty())
            });
            self.shader_paths.retain(|entity, _| {
                world.get::<MeshRenderer>(*entity).is_some()
                    || world
                        .get::<prism_scene::components::SkinnedMeshRenderer>(*entity)
                        .is_some()
            });

            let mut out = Vec::new();
            for (entity, mr) in world.query::<MeshRenderer>() {
                let skinned = world
                    .get::<crate::scene::components::SkinnedMeshRenderer>(entity)
                    .is_some();
                let wants_mesh = !skinned
                    && !mr.mesh_path.is_empty()
                    && world
                        .get::<MeshRef>(entity)
                        .is_some_and(|r| r.generation == 0)
                    && !self.failed_before(entity, AssetSlot::Mesh, &mr.mesh_path);
                let material_unresolved = world
                    .get::<MaterialRef>(entity)
                    .is_some_and(|r| r.generation == 0);
                let wants_material = material_unresolved
                    && ((!mr.material_path.is_empty()
                        && !self.failed_before(entity, AssetSlot::Material, &mr.material_path))
                        || mr.material_paths.iter().any(|path| {
                            !path.is_empty()
                                && !self.failed_before(entity, AssetSlot::Material, path)
                        }));
                let shader_unresolved = world
                    .get::<ShaderRef>(entity)
                    .is_none_or(|r| r.generation == 0);
                let shader_changed = self
                    .shader_paths
                    .get(&entity)
                    .is_some_and(|prev| prev != &mr.shader_path);
                let wants_shader = !mr.shader_path.is_empty()
                    && (shader_unresolved || shader_changed)
                    && !self.failed_before(entity, AssetSlot::Shader, &mr.shader_path);
                if (wants_mesh || wants_material || wants_shader)
                    && !self.enqueued.contains(&entity)
                {
                    out.push(PendingRequest {
                        entity,
                        mesh_path: wants_mesh.then(|| mr.mesh_path.clone()),
                        renderer: mr.clone(),
                        wants_material,
                        shader_path: wants_shader.then(|| mr.shader_path.clone()),
                        sprite_path: None,
                        tileset: false,
                        ui_image: false,
                        skinned_path: None,
                    });
                    self.enqueued.insert(entity);
                }
            }
            for (entity, skinned) in world.query::<prism_scene::components::SkinnedMeshRenderer>() {
                let path = &skinned.skinned_mesh_path;
                let wants_mesh = !path.is_empty()
                    && world
                        .get::<MeshRef>(entity)
                        .is_some_and(|r| r.generation == 0)
                    && !self.failed_before(entity, AssetSlot::Mesh, path);
                let wants_material = world
                    .get::<MaterialRef>(entity)
                    .is_some_and(|r| r.generation == 0)
                    && ((!skinned.material_path.is_empty()
                        && !self.failed_before(
                            entity,
                            AssetSlot::Material,
                            &skinned.material_path,
                        ))
                        || skinned.material_paths.iter().any(|path| {
                            !path.is_empty()
                                && !self.failed_before(entity, AssetSlot::Material, path)
                        }));
                let shader_unresolved = world
                    .get::<ShaderRef>(entity)
                    .is_none_or(|r| r.generation == 0);
                let shader_changed = self
                    .shader_paths
                    .get(&entity)
                    .is_some_and(|prev| prev != &skinned.shader_path);
                let wants_shader = !skinned.shader_path.is_empty()
                    && (shader_unresolved || shader_changed)
                    && !self.failed_before(entity, AssetSlot::Shader, &skinned.shader_path);
                if (wants_mesh || wants_material || wants_shader)
                    && !self.enqueued.contains(&entity)
                {
                    out.push(PendingRequest {
                        entity,
                        mesh_path: None,
                        renderer: skinned.as_mesh_renderer(),
                        wants_material,
                        shader_path: wants_shader.then(|| skinned.shader_path.clone()),
                        sprite_path: None,
                        tileset: false,
                        ui_image: false,
                        skinned_path: wants_mesh.then(|| path.clone()),
                    });
                    self.enqueued.insert(entity);
                }
            }
            for (entity, sprite) in world.query::<SpriteRenderer>() {
                if !sprite.has_sprite() {
                    continue;
                }
                let unresolved = world
                    .get::<SpriteRef>(entity)
                    .map(|r| r.generation == 0)
                    .unwrap_or(true);
                let path_changed = self
                    .sprite_paths
                    .get(&entity)
                    .is_some_and(|last| last != &sprite.sprite_path);
                if (unresolved || path_changed)
                    && !self.failed_before(entity, AssetSlot::Sprite, &sprite.sprite_path)
                    && !self.enqueued.contains(&entity)
                {
                    out.push(PendingRequest {
                        entity,
                        mesh_path: None,
                        renderer: MeshRenderer::default(),
                        wants_material: false,
                        shader_path: None,
                        sprite_path: Some(sprite.sprite_path.clone()),
                        tileset: false,
                        ui_image: false,
                        skinned_path: None,
                    });
                    self.enqueued.insert(entity);
                    self.sprite_paths.insert(entity, sprite.sprite_path.clone());
                }
            }
            for (entity, tilemap) in world.query::<Tilemap>() {
                if tilemap.tileset_path.is_empty() {
                    continue;
                }
                let unresolved = world
                    .get::<TilemapRef>(entity)
                    .map(|r| r.generation == 0)
                    .unwrap_or(true);
                let path_changed = self
                    .tileset_paths
                    .get(&entity)
                    .is_some_and(|last| last != &tilemap.tileset_path);
                if (unresolved || path_changed)
                    && !self.failed_before(entity, AssetSlot::Tileset, &tilemap.tileset_path)
                    && !self.enqueued.contains(&entity)
                {
                    out.push(PendingRequest {
                        entity,
                        mesh_path: None,
                        renderer: MeshRenderer::default(),
                        wants_material: false,
                        shader_path: None,
                        sprite_path: Some(tilemap.tileset_path.clone()),
                        tileset: true,
                        ui_image: false,
                        skinned_path: None,
                    });
                    self.enqueued.insert(entity);
                    self.tileset_paths
                        .insert(entity, tilemap.tileset_path.clone());
                }
            }
            for (entity, style) in world.query::<crate::ui::components::Style>() {
                if style.image.is_empty() {
                    continue;
                }
                let unresolved = world
                    .get::<UiImageRef>(entity)
                    .map(|r| r.generation == 0)
                    .unwrap_or(true);
                let path_changed = self
                    .ui_image_paths
                    .get(&entity)
                    .is_some_and(|last| last != &style.image);
                if (unresolved || path_changed)
                    && !self.failed_before(entity, AssetSlot::UiImage, &style.image)
                    && !self.enqueued.contains(&entity)
                {
                    out.push(PendingRequest {
                        entity,
                        mesh_path: None,
                        renderer: MeshRenderer::default(),
                        wants_material: false,
                        shader_path: None,
                        sprite_path: Some(style.image.clone()),
                        tileset: false,
                        ui_image: true,
                        skinned_path: None,
                    });
                    self.enqueued.insert(entity);
                    self.ui_image_paths.insert(entity, style.image.clone());
                }
            }
            out
        };

        if pending.is_empty() {
            return Vec::new();
        }

        let mut reqs = Vec::with_capacity(pending.len());
        for item in pending {
            let loaded = match item.mesh_path.as_deref() {
                Some(path) => {
                    let loaded = procedural
                        .and_then(|meshes| meshes.get(path))
                        .map(|input| LoadedMesh {
                            input,
                            submeshes: Vec::new(),
                        })
                        .or_else(|| self.load_mesh_inputs(path));
                    if loaded.is_none() {
                        self.record_failure(item.entity, AssetSlot::Mesh, path);
                    }
                    loaded
                }
                None => None,
            };
            let mut skin = None;
            let loaded = match item.skinned_path.as_deref() {
                Some(path) => {
                    let loaded = self.load_skinned_mesh_inputs(path);
                    if loaded.is_none() {
                        self.record_failure(item.entity, AssetSlot::Mesh, path);
                    }
                    if let Some(skinned) = loaded.as_ref() {
                        skin = Some(skinned.skin.clone());
                    }
                    loaded.map(|skinned| LoadedMesh {
                        input: skinned.input,
                        submeshes: skinned.submeshes,
                    })
                }
                None => loaded,
            };
            let (mesh, submeshes) = match loaded {
                Some(loaded) => (Some(loaded.input), loaded.submeshes),
                None => (
                    None,
                    world
                        .get::<MeshSubmeshes>(item.entity)
                        .map(|cached| cached.submeshes.clone())
                        .unwrap_or_default(),
                ),
            };
            if !submeshes.is_empty() {
                self.pending_submeshes
                    .insert(item.entity, submeshes.clone());
            }

            let mut materials: Vec<Option<MaterialResolveData>> = Vec::new();
            if item.wants_material {
                let submesh_count = if submeshes.is_empty() {
                    1
                } else {
                    submeshes.len()
                };
                let mut unique_paths: Vec<String> = Vec::new();
                let mut material_map: Vec<Option<usize>> = Vec::with_capacity(submesh_count);
                for submesh in 0..submesh_count {
                    let material_index = submeshes
                        .get(submesh)
                        .and_then(|submesh| submesh.material_index);
                    let path = item.renderer.material_path_for_submesh(
                        submesh,
                        submesh_count,
                        material_index,
                    );
                    match path {
                        Some(path)
                            if !self.failed_before(item.entity, AssetSlot::Material, path) =>
                        {
                            let index = unique_paths
                                .iter()
                                .position(|known| known == path)
                                .unwrap_or_else(|| {
                                    unique_paths.push(path.to_string());
                                    unique_paths.len() - 1
                                });
                            material_map.push(Some(index));
                        }
                        _ => material_map.push(None),
                    }
                }
                for path in &unique_paths {
                    let loaded = self.load_material_data(path);
                    if loaded.is_none() {
                        self.record_failure(item.entity, AssetSlot::Material, path);
                    }
                    materials.push(loaded);
                }
                self.pending_material_map.insert(item.entity, material_map);
            }

            let mut queued_texture_waiter = false;
            let texture = match item.sprite_path.as_deref() {
                Some(path) => {
                    let slot = if item.tileset {
                        AssetSlot::Tileset
                    } else if item.ui_image {
                        AssetSlot::UiImage
                    } else {
                        AssetSlot::Sprite
                    };
                    if path == prism_scene::BUILTIN_WHITE_SPRITE {
                        let loaded = self.load_texture_by_resolved_path(path);
                        if loaded.is_some() {
                            self.pending_texture_slot.insert(item.entity, slot);
                        }
                        loaded
                    } else if let Some((backing_path, atlas_meta)) =
                        self.resolve_texture_target(path)
                    {
                        if self.inflight_texture_paths.contains(&backing_path) {
                            queued_texture_waiter = true;
                            self.texture_waiters.entry(backing_path).or_default().push(
                                TextureWaiter {
                                    entity: item.entity,
                                    slot,
                                    atlas_meta,
                                },
                            );
                            None
                        } else {
                            let loaded = self.load_texture_by_resolved_path(&backing_path);
                            if loaded.is_some() {
                                if let Some(meta) = atlas_meta {
                                    self.pending_atlas_sprites.insert(item.entity, meta);
                                }
                                self.pending_texture_slot.insert(item.entity, slot);
                                self.pending_texture_path
                                    .insert(item.entity, backing_path.clone());
                                self.inflight_texture_paths.insert(backing_path);
                            } else {
                                self.record_failure(item.entity, slot, path);
                            }
                            loaded
                        }
                    } else {
                        self.record_failure(item.entity, slot, path);
                        None
                    }
                }
                None => None,
            };
            if texture.is_some() {
                let slot = if item.tileset {
                    AssetSlot::Tileset
                } else if item.ui_image {
                    AssetSlot::UiImage
                } else {
                    AssetSlot::Sprite
                };
                self.pending_texture_slot.insert(item.entity, slot);
            }
            if let Some(bounds) = mesh.as_ref().and_then(MeshUploadInput::bounds) {
                self.pending_bounds.insert(item.entity, bounds);
            }
            let shader = match item.shader_path {
                Some(path) => {
                    let loaded =
                        self.resource_manager.id_by_path(&path).and_then(|id| {
                            match self
                                .resource_manager
                                .load::<prism_asset::runtime::ShaderAsset>(id)
                            {
                                Ok(handle) => self
                                    .resource_manager
                                    .get(handle)
                                    .ok()
                                    .map(|asset| (id, asset.spirv.clone())),
                                Err(err) => {
                                    log::warn!(
                                        "asset_resolver: ShaderAsset '{path}' decode failed: {err}"
                                    );
                                    None
                                }
                            }
                        });
                    match loaded {
                        Some((id, spirv)) => {
                            self.pending_shader_meta.insert(
                                item.entity,
                                (path, prism_scene::SceneAssetId::from_raw(id.0)),
                            );
                            Some(spirv)
                        }
                        None => {
                            report_missing_reference(&path, "shader");
                            self.record_failure(item.entity, AssetSlot::Shader, &path);
                            None
                        }
                    }
                }
                None => None,
            };
            if queued_texture_waiter
                && mesh.is_none()
                && materials.is_empty()
                && texture.is_none()
                && shader.is_none()
            {
                continue;
            }
            reqs.push((
                item.entity,
                AssetResolveRequest {
                    skin,
                    mesh,
                    materials,
                    texture,
                    shader,
                },
            ));
        }

        span.detail(format!("staged={}", reqs.len()));
        if !reqs.is_empty() {
            log::info!(
                "prepare_requests: staged {} entit(y/ies) for async GPU upload",
                reqs.len()
            );
        }
        reqs
    }

    /// Result write-back after call: remove entity's enqueue heavy flag, and write GPU handle.
    ///
    /// Must be called in main thread (holds `&mut World`).
    pub fn apply_results(
        &mut self,
        world: &mut World,
        results: &[(Entity, prism_render::asset_bridge::AssetResolveResult)],
    ) {
        for (entity, result) in results {
            if let Some(mr) = world.get_mut::<MeshRef>(*entity) {
                if let Some(h) = result.mesh_handle {
                    mr.render_handle = h;
                    mr.generation = 1;
                }
            }
            if let Some(handle) = result.mesh_handle {
                self.record_resolved(
                    *entity,
                    AssetSlot::Mesh,
                    AssetRelease {
                        mesh_handle: Some(handle),
                        ..AssetRelease::default()
                    },
                );
            }
            if let Some(bounds) = self.pending_bounds.remove(entity) {
                if let Some(handle) = result.mesh_handle {
                    if world.get_resource::<MeshBounds>().is_none() {
                        world.insert_resource(MeshBounds::default());
                    }
                    if let Some(registry) = world.get_resource_mut::<MeshBounds>() {
                        registry.insert(handle, bounds);
                    }
                }
            }
            if let Some(submeshes) = self.pending_submeshes.remove(entity) {
                if result.mesh_handle.is_some() {
                    if submeshes.is_empty() {
                        world.remove::<MeshSubmeshes>(*entity);
                    } else {
                        world.insert(*entity, MeshSubmeshes { submeshes });
                    }
                }
            }
            match self.pending_material_map.remove(entity) {
                Some(material_map) => {
                    let uploaded = if result.material_slots.is_empty() {
                        result
                            .material_slot
                            .map(|slot| vec![Some(slot)])
                            .unwrap_or_default()
                    } else {
                        result.material_slots.clone()
                    };
                    let slots: Vec<Option<u32>> = material_map
                        .iter()
                        .map(|index| index.and_then(|index| uploaded.get(index).copied().flatten()))
                        .collect();
                    let first = slots.iter().find_map(|slot| *slot);
                    let mut unique: Vec<u32> = slots.iter().flatten().copied().collect();
                    unique.sort_unstable();
                    unique.dedup();

                    if let Some(mr) = world.get_mut::<MaterialRef>(*entity) {
                        if let Some(slot) = first {
                            mr.material_slot = slot;
                            mr.generation = 1;
                        }
                    }
                    if !slots.is_empty() {
                        world.insert(*entity, MaterialSlots { slots });
                    }
                    if first.is_some() {
                        self.record_resolved(
                            *entity,
                            AssetSlot::Material,
                            AssetRelease {
                                material_slot: first,
                                material_slots: unique,
                                ..AssetRelease::default()
                            },
                        );
                    }
                }
                None => {
                    if let Some(slot) = result.material_slot {
                        if let Some(mr) = world.get_mut::<MaterialRef>(*entity) {
                            mr.material_slot = slot;
                            mr.generation = 1;
                        }
                        self.record_resolved(
                            *entity,
                            AssetSlot::Material,
                            AssetRelease {
                                material_slot: Some(slot),
                                material_slots: vec![slot],
                                ..AssetRelease::default()
                            },
                        );
                    }
                }
            }
            if let Some((shader_path, asset_id)) = self.pending_shader_meta.remove(entity) {
                if let Some(shader_handle) = result.shader_handle {
                    self.shader_paths.insert(*entity, shader_path);
                    world.insert(
                        *entity,
                        ShaderRef {
                            asset_id,
                            shader_handle,
                            generation: 1,
                        },
                    );
                }
            }
            let pending_path = self.pending_texture_path.remove(entity);
            if let Some(ref backing_path) = pending_path {
                self.inflight_texture_paths.remove(backing_path);
            }
            if let (Some(slot), Some(size)) = (result.texture_slot, result.texture_size) {
                let texture_slot = self
                    .pending_texture_slot
                    .remove(entity)
                    .unwrap_or(AssetSlot::Sprite);
                self.record_resolved(
                    *entity,
                    texture_slot,
                    AssetRelease {
                        texture_slot: Some(slot),
                        ..AssetRelease::default()
                    },
                );
                let atlas_meta = self.pending_atlas_sprites.remove(entity);
                self.write_texture_refs(world, *entity, texture_slot, slot, size, atlas_meta);

                if let Some(backing_path) = pending_path {
                    if slot != prism_render::bindless::TextureHandle::WHITE.0 {
                        self.texture_cache.insert(
                            backing_path.clone(),
                            CachedTextureEntry {
                                texture_slot: slot,
                                size,
                            },
                        );
                    }
                    if let Some(waiters) = self.texture_waiters.remove(&backing_path) {
                        for waiter in waiters {
                            if world.is_alive(waiter.entity) {
                                self.record_resolved(
                                    waiter.entity,
                                    waiter.slot,
                                    AssetRelease {
                                        texture_slot: Some(slot),
                                        ..AssetRelease::default()
                                    },
                                );
                                self.write_texture_refs(
                                    world,
                                    waiter.entity,
                                    waiter.slot,
                                    slot,
                                    size,
                                    waiter.atlas_meta,
                                );
                            }
                            self.enqueued.remove(&waiter.entity);
                        }
                    }
                }
            } else if let Some(backing_path) = pending_path {
                self.pending_texture_slot.remove(entity);
                self.pending_atlas_sprites.remove(entity);
                if let Some(waiters) = self.texture_waiters.remove(&backing_path) {
                    for waiter in waiters {
                        self.enqueued.remove(&waiter.entity);
                    }
                }
            }
            self.enqueued.remove(entity);
        }
    }

    /// Invalidate parse results of **all** `MeshRenderer` / `SpriteRenderer` (`.pak` heavy rebuild after call).
    ///
    /// Only clears state on entity side (`MeshRef` / `MaterialRef` / `SpriteRef`'s
    /// `generation`, failure cache, queue flag), **does not touch** `ResourceManager`: bytes inside package by
    /// [`ResourceManager::on_pak_changed`] swapped, new bytes in below one frame
    /// [`Self::prepare_requests`] inside heavy re-reads them, render thread gives them new handles.
    ///
    /// Returns **heavy re-queued** entity count. Old GPU handles temporarily remain in renderer inside (no interface),
    /// frequent hot reload will cause texture/material slots to grow --- already known, see #495 follow-up entry.
    pub fn invalidate_assets(&mut self, world: &mut World) -> usize {
        self.texture_cache.clear();
        self.atlas_cache.clear();
        self.inflight_texture_paths.clear();
        self.texture_waiters.clear();
        self.pending_texture_path.clear();
        let mut entities: Vec<Entity> = Vec::new();
        for (entity, _) in world.query::<MeshRenderer>() {
            entities.push(entity);
        }
        for (entity, _) in world.query::<SpriteRenderer>() {
            if !entities.contains(&entity) {
                entities.push(entity);
            }
        }
        for (entity, _) in world.query::<Tilemap>() {
            if !entities.contains(&entity) {
                entities.push(entity);
            }
        }
        for (entity, _) in world.query::<crate::scene::runtime_components::UiImageRef>() {
            if !entities.contains(&entity) {
                entities.push(entity);
            }
        }
        let count = entities.len();
        for entity in entities {
            self.reset_entity_refs(world, entity);
        }
        count
    }

    /// Only lets entities referencing `path` heavy re-parse (changed one graph / one mesh).
    ///
    /// `path` consistent with authoring document inside writing (`mesh_path` / `material_path` /
    /// `sprite_path`). Returns number of entities referencing that path; returns 0 if no entity references it.
    pub fn invalidate_path(&mut self, world: &mut World, path: &str) -> usize {
        self.texture_cache.remove(path);
        self.atlas_cache.retain(|key, entry| {
            entry.texture_path != path
                && key
                    .split_once('#')
                    .is_none_or(|(atlas_path, _)| atlas_path != path)
        });
        self.inflight_texture_paths.remove(path);
        self.texture_waiters.remove(path);
        let mut entities: Vec<Entity> = Vec::new();
        for (entity, renderer) in world.query::<MeshRenderer>() {
            if renderer.mesh_path == path
                || renderer.material_path == path
                || renderer
                    .material_paths
                    .iter()
                    .any(|material| material == path)
            {
                entities.push(entity);
            }
        }
        for (entity, sprite) in world.query::<SpriteRenderer>() {
            if sprite.sprite_path == path && !entities.contains(&entity) {
                entities.push(entity);
            }
        }
        for (entity, tilemap) in world.query::<Tilemap>() {
            if tilemap.tileset_path == path && !entities.contains(&entity) {
                entities.push(entity);
            }
        }
        for (entity, style) in world.query::<crate::ui::components::Style>() {
            if style.image == path && !entities.contains(&entity) {
                entities.push(entity);
            }
        }
        let count = entities.len();
        for entity in entities {
            self.reset_entity_refs(world, entity);
        }
        count
    }

    /// Reset entity's already parsed handle back to `generation == 0`, and clear cache blocking heavy re-parse.
    ///
    /// Returns `true` indicating real modification was made (entities already `0` do not count).
    fn reset_entity_refs(&mut self, world: &mut World, entity: Entity) -> bool {
        let mut changed = false;
        if let Some(mesh) = world.get_mut::<MeshRef>(entity) {
            if mesh.generation != 0 {
                mesh.generation = 0;
                changed = true;
            }
        }
        if let Some(material) = world.get_mut::<MaterialRef>(entity) {
            if material.generation != 0 {
                material.generation = 0;
                changed = true;
            }
        }
        if let Some(sprite) = world.get_mut::<SpriteRef>(entity) {
            if sprite.generation != 0 {
                sprite.generation = 0;
                changed = true;
            }
        }
        if let Some(tileset) = world.get_mut::<TilemapRef>(entity) {
            if tileset.generation != 0 {
                tileset.generation = 0;
                changed = true;
            }
        }
        if let Some(image) = world.get_mut::<UiImageRef>(entity) {
            if image.generation != 0 {
                image.generation = 0;
                changed = true;
            }
        }
        self.enqueued.remove(&entity);
        self.pending_bounds.remove(&entity);
        self.pending_submeshes.remove(&entity);
        self.pending_material_map.remove(&entity);
        if let Some(path) = self.pending_texture_path.remove(&entity) {
            self.inflight_texture_paths.remove(&path);
        }
        for waiters in self.texture_waiters.values_mut() {
            waiters.retain(|waiter| waiter.entity != entity);
        }
        self.tileset_paths.remove(&entity);
        self.ui_image_paths.remove(&entity);
        self.failed.retain(|(owner, _), _| *owner != entity);
        changed
    }

    /// Read the `.rskel` / `.ranim` files a scene references into [`SkeletonLibrary`].
    ///
    /// Takes `&mut World` because it writes resources, so it is not a scheduled
    /// system: callers run it once per frame next to `prepare_requests` (repeat
    /// calls are idempotent).
    pub fn load_animation_assets(&mut self, world: &mut World) -> usize {
        let _span = crate::util::timing::span("asset::load_animation_assets");
        use crate::scene::systems::animator::SkeletonLibrary;

        let mut missing_refs = Vec::new();
        for (entity, renderer) in world.query::<prism_scene::components::SkinnedMeshRenderer>() {
            if renderer.skinned_mesh_path.trim().is_empty() {
                continue;
            }
            if world.get::<MeshRef>(entity).is_none() {
                missing_refs.push((entity, true));
            } else if world.get::<MaterialRef>(entity).is_none() {
                missing_refs.push((entity, false));
            }
        }
        for (entity, needs_mesh) in missing_refs {
            if needs_mesh {
                world.insert(
                    entity,
                    MeshRef {
                        asset_id: crate::scene::components::SceneAssetId::generate(),
                        render_handle: prism_render::managers::MeshHandle::default(),
                        generation: 0,
                    },
                );
            }
            if world.get::<MaterialRef>(entity).is_none() {
                world.insert(
                    entity,
                    MaterialRef {
                        asset_id: crate::scene::components::SceneAssetId::generate(),
                        material_slot: 0,
                        generation: 0,
                    },
                );
            }
        }

        let mut requested: Vec<String> = Vec::new();
        for (entity, root) in world.query::<prism_scene::components::SkeletonRoot>() {
            if !root.skeleton_path.trim().is_empty() {
                requested.push(root.skeleton_path.clone());
            }
            if let Some(animator) = world.get::<prism_scene::components::Animator>(entity) {
                let clip = if animator.clip_path.trim().is_empty() {
                    root.default_clip_path.clone()
                } else {
                    animator.clip_path.clone()
                };
                if !clip.trim().is_empty() {
                    requested.push(clip);
                }
                if !animator.state_machine_path.trim().is_empty() {
                    requested.push(animator.state_machine_path.clone());
                }
                if !animator.anim_graph_path.trim().is_empty() {
                    requested.push(animator.anim_graph_path.clone());
                }
            }
        }
        if requested.is_empty() {
            return 0;
        }
        requested.sort();
        requested.dedup();

        if world.get_resource::<SkeletonLibrary>().is_none() {
            world.insert_resource(SkeletonLibrary::new());
        }
        if world
            .get_resource::<crate::scene::systems::animator::StateMachineLibrary>()
            .is_none()
        {
            world.insert_resource(crate::scene::systems::animator::StateMachineLibrary::new());
        }
        if world
            .get_resource::<crate::scene::systems::animator::AnimGraphLibrary>()
            .is_none()
        {
            world.insert_resource(crate::scene::systems::animator::AnimGraphLibrary::new());
        }
        if world.get_resource::<prism_anim::AnimParameters>().is_none() {
            world.insert_resource(prism_anim::AnimParameters::default());
        }

        let mut loaded = 0usize;
        for path in requested {
            let already = world
                .get_resource::<SkeletonLibrary>()
                .is_some_and(|library| library.has_skeleton(&path) || library.has_clip(&path))
                || world
                    .get_resource::<crate::scene::systems::animator::StateMachineLibrary>()
                    .is_some_and(|library| library.has_state_machine(&path))
                || world
                    .get_resource::<crate::scene::systems::animator::AnimGraphLibrary>()
                    .is_some_and(|library| library.has_graph(&path));
            if already {
                continue;
            }
            let Some(id) = self.resource_manager.id_by_path(&path) else {
                report_missing_reference(&path, "animation");
                continue;
            };
            if path.ends_with(".rskel") {
                let Ok(handle) = self
                    .resource_manager
                    .load_with_deps::<prism_asset::runtime::SkeletonAsset>(id)
                    .map_err(|error| log::warn!("load_animation_assets: '{path}' failed: {error}"))
                else {
                    continue;
                };
                let Ok(asset) = self.resource_manager.get(handle) else {
                    continue;
                };
                let Some(skeleton) = prism_anim::Skeleton::from_info(&asset.info) else {
                    log::warn!("load_animation_assets: '{path}' is not a valid skeleton");
                    continue;
                };
                if let Some(library) = world.get_resource_mut::<SkeletonLibrary>() {
                    library.insert_skeleton(path, skeleton);
                    loaded += 1;
                }
            } else if path.ends_with(".ranim") {
                let Ok(handle) = self
                    .resource_manager
                    .load_with_deps::<prism_asset::runtime::AnimClipAsset>(id)
                    .map_err(|error| log::warn!("load_animation_assets: '{path}' failed: {error}"))
                else {
                    continue;
                };
                let Ok(asset) = self.resource_manager.get(handle) else {
                    continue;
                };
                let clip = prism_anim::Clip::from_info(&asset.info);
                if let Some(library) = world.get_resource_mut::<SkeletonLibrary>() {
                    library.insert_clip(path, clip);
                    loaded += 1;
                }
            } else if path.ends_with(".rasm") {
                let Ok(handle) = self
                    .resource_manager
                    .load_with_deps::<prism_asset::runtime::StateMachineAsset>(id)
                    .map_err(|error| log::warn!("load_animation_assets: '{path}' failed: {error}"))
                else {
                    continue;
                };
                let Ok(asset) = self.resource_manager.get(handle) else {
                    continue;
                };
                let machine = prism_anim::StateMachine::from_info(&asset.info);
                if world
                    .get_resource_mut::<crate::scene::systems::animator::StateMachineLibrary>()
                    .is_some_and(|library| library.insert(path, machine))
                {
                    loaded += 1;
                }
            } else if path.ends_with(".rag") || path.ends_with(".animgraph.json") {
                let Ok(handle) = self
                    .resource_manager
                    .load_with_deps::<prism_asset::runtime::AnimGraphAsset>(id)
                    .map_err(|error| log::warn!("load_animation_assets: '{path}' failed: {error}"))
                else {
                    continue;
                };
                let Ok(asset) = self.resource_manager.get(handle) else {
                    continue;
                };
                if world
                    .get_resource_mut::<crate::scene::systems::animator::AnimGraphLibrary>()
                    .is_some_and(|library| library.insert(path, asset.info.clone()))
                {
                    loaded += 1;
                }
            }
        }

        let mut graph_machines: Vec<String> = Vec::new();
        if let Some(library) =
            world.get_resource::<crate::scene::systems::animator::AnimGraphLibrary>()
        {
            for (_, graph) in library.all() {
                for node in &graph.nodes {
                    if !node.state_machine_path.trim().is_empty() {
                        graph_machines.push(node.state_machine_path.clone());
                    }
                }
            }
        }
        graph_machines.sort();
        graph_machines.dedup();
        for path in graph_machines {
            let already = world
                .get_resource::<crate::scene::systems::animator::StateMachineLibrary>()
                .is_some_and(|library| library.has_state_machine(&path));
            if already {
                continue;
            }
            let Some(id) = self.resource_manager.id_by_path(&path) else {
                report_missing_reference(&path, "animation");
                continue;
            };
            let Ok(handle) = self
                .resource_manager
                .load_with_deps::<prism_asset::runtime::StateMachineAsset>(id)
                .map_err(|error| log::warn!("load_animation_assets: '{path}' failed: {error}"))
            else {
                continue;
            };
            let Ok(asset) = self.resource_manager.get(handle) else {
                continue;
            };
            let machine = prism_anim::StateMachine::from_info(&asset.info);
            if world
                .get_resource_mut::<crate::scene::systems::animator::StateMachineLibrary>()
                .is_some_and(|library| library.insert(path, machine))
            {
                loaded += 1;
            }
        }

        let mut state_clips: Vec<String> = Vec::new();
        if let Some(library) =
            world.get_resource::<crate::scene::systems::animator::StateMachineLibrary>()
        {
            for machine in library.all() {
                for state in &machine.states {
                    if !state.clip_path.trim().is_empty() {
                        state_clips.push(state.clip_path.clone());
                    }
                }
            }
        }
        if let Some(library) =
            world.get_resource::<crate::scene::systems::animator::AnimGraphLibrary>()
        {
            for (_, graph) in library.all() {
                for node in &graph.nodes {
                    state_clips.extend(node.clips.iter().cloned());
                }
            }
        }
        state_clips.sort();
        state_clips.dedup();
        for path in state_clips {
            let already = world
                .get_resource::<SkeletonLibrary>()
                .is_some_and(|library| library.has_clip(&path));
            if already {
                continue;
            }
            let Some(id) = self.resource_manager.id_by_path(&path) else {
                report_missing_reference(&path, "animation");
                continue;
            };
            let Ok(handle) = self
                .resource_manager
                .load_with_deps::<prism_asset::runtime::AnimClipAsset>(id)
                .map_err(|error| log::warn!("load_animation_assets: '{path}' failed: {error}"))
            else {
                continue;
            };
            let Ok(asset) = self.resource_manager.get(handle) else {
                continue;
            };
            let clip = prism_anim::Clip::from_info(&asset.info);
            if let Some(library) = world.get_resource_mut::<SkeletonLibrary>() {
                library.insert_clip(path, clip);
                loaded += 1;
            }
        }
        loaded
    }
}

impl Default for GpuAssetResolver {
    fn default() -> Self {
        Self::new()
    }
}
