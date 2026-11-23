//! Runtime-only render handles attached to scene entities.
//!
//! These are intentionally **not** part of the scene schema: cooked scenes
//! carry authoring components (`MeshRenderer`, transforms, lights, …), while
//! the engine resolves those into GPU-side handles at load time.

use prism_render::managers::MeshHandle;
use prism_scene::SceneAssetId;

/// GPU mesh reference — resolved from an asset at spawn time.
#[derive(Debug, Clone, Copy)]
pub struct MeshRef {
    /// Stable asset id (hot-reload / debugging).
    pub asset_id: SceneAssetId,
    /// GPU-side handle.
    pub render_handle: MeshHandle,
    /// Asset generation when resolved; bumped on hot-reload.
    pub generation: u32,
}

/// GPU material slot reference.
#[derive(Debug, Clone, Copy)]
pub struct MaterialRef {
    /// Stable asset id.
    pub asset_id: SceneAssetId,
    /// SSBO slot from the render material manager.
    pub material_slot: u32,
    /// Generation at resolve time.
    pub generation: u32,
}

/// GPU custom fragment shader reference resolved from [`ShaderAsset`](prism_asset::runtime::ShaderAsset).
#[derive(Debug, Clone, Copy)]
pub struct ShaderRef {
    /// Stable asset id.
    pub asset_id: SceneAssetId,
    /// Registered shader handle inside `ForwardPass`.
    pub shader_handle: prism_render::ShaderHandle,
    /// Generation at resolve time (`0` = pending resolve).
    pub generation: u32,
}

/// Submesh index range within one mesh (#520).
///
/// Restored from RMES v2 submesh table in `.pak`. At runtime render extraction system uses these ranges
/// to split one entity into multiple [`DrawItem`](prism_render::DrawItem)s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshSubmesh {
    pub index_start: u32,
    pub index_count: u32,
    /// Source file (glTF) material index; only used to map `material_paths` to submeshes.
    pub material_index: Option<u32>,
}

/// Runtime: mesh submesh table. Author component `MeshRenderer` does not save it,
/// because ranges belong to cooked mesh asset, not an instance in scene.
#[derive(Debug, Clone, Default)]
pub struct MeshSubmeshes {
    pub submeshes: Vec<MeshSubmesh>,
}

/// Runtime: material SSBO slot after parsing each submesh (`None` = default material).
///
/// `MaterialRef::material_slot` still saves first material for legacy code reading only that field;
/// multi-material draw reads per-submesh slots here.
#[derive(Debug, Clone, Default)]
pub struct MaterialSlots {
    pub slots: Vec<Option<u32>>,
}

/// GPU texture reference for a 2D [`SpriteRenderer`](prism_scene::components::SpriteRenderer).
///
/// Same role as [`MeshRef`] for meshes: the scene only stores the authoring
/// texture path, while the resolved bindless texture handle lives here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpriteRef {
    /// Stable asset id (hot-reload / debugging).
    pub asset_id: SceneAssetId,
    /// Bindless SRV slot of the resolved texture (what the sprite pass reads).
    pub texture_slot: u32,
    /// Pixel size of the uploaded texture or atlas sub-sprite, filled in after upload.
    pub size: [u32; 2],
    /// `0` = not resolved yet; bumped on hot-reload.
    pub generation: u32,
    /// Atlas UV rect `[u0, v0, u1, v1]` (defaults to `[0.0, 0.0, 1.0, 1.0]` for standalone textures).
    pub uv_rect: [f32; 4],
    /// Trimmed offset `[x, y]` in pixels relative to original sprite top-left.
    pub trimmed_offset: [i32; 2],
    /// Original sprite size before alpha trimming `[width, height]`.
    pub original_size: [u32; 2],
}

impl SpriteRef {
    /// Helper to construct a standard standalone sprite reference.
    pub fn new(asset_id: SceneAssetId, texture_slot: u32, size: [u32; 2], generation: u32) -> Self {
        Self {
            asset_id,
            texture_slot,
            size,
            generation,
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            trimmed_offset: [0, 0],
            original_size: size,
        }
    }
}

impl Default for SpriteRef {
    fn default() -> Self {
        Self {
            asset_id: SceneAssetId::generate(),
            texture_slot: 0,
            size: [0, 0],
            generation: 0,
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            trimmed_offset: [0, 0],
            original_size: [0, 0],
        }
    }
}

/// GPU texture reference for a 2D [`Tilemap`](prism_scene::components::Tilemap) tileset.
///
/// Same role as [`SpriteRef`]: the scene stores `tileset_path`, the resolver uploads
/// it and writes the bindless slot here (#529).
#[derive(Debug, Clone, Copy)]
pub struct TilemapRef {
    /// Stable asset id (hot-reload / debugging).
    pub asset_id: SceneAssetId,
    /// Bindless SRV slot of the tileset texture.
    pub texture_slot: u32,
    /// Pixel size of the uploaded tileset.
    pub size: [u32; 2],
    /// `0` = not resolved yet; bumped on hot-reload.
    pub generation: u32,
}

/// GPU texture reference for a UI [`Style::image`](prism_scene::ui::Style) background.
///
/// Same role as [`SpriteRef`]: the scene stores the authoring path, the resolver
/// uploads it and writes the bindless slot here (#591).
#[derive(Debug, Clone, Copy)]
pub struct UiImageRef {
    /// Stable asset id (hot-reload / debugging).
    pub asset_id: SceneAssetId,
    /// Bindless SRV slot of the uploaded image.
    pub texture_slot: u32,
    /// Pixel size of the uploaded image.
    pub size: [u32; 2],
    /// `0` = not resolved yet; bumped on hot-reload.
    pub generation: u32,
}

/// Stable id of each entity in `.scene.json` v3+.
///
/// ECS [`Entity`](prism_ecs::Entity) handle reallocated every load; cross-session (save restore,
/// editor sync) matching same entity only possible via author document id. Loader writes this flag at spawn;
/// it does not participate in rendering, nor enter `.scene.json` (author document already has `id`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SceneEntityId(pub u32);

/// Runtime hidden flag for distance LOD (#491).
///
/// Written by `lod_system`: when parent entity has [`LodGroup`](prism_scene::components::LodGroup)
/// only child entities matching current distance level get `LodHidden(false)`, others `true`.
/// `is_active_in_hierarchy` skips `true` entities, but **does not rewrite** author
/// `Active`, nor affect editor [`Hidden`]. Not serialized.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LodHidden(pub bool);

/// Editor session-level hidden flag (Unity `SceneVisibilityManager`).
///
/// `Active` is author data (goes into `.scene.json`, no more simulation when disabled); this flag only in
/// current editor process set by Scene view, render systems skip entity accordingly, but physics, scripts and
/// document content unaffected. It **is not** an attachable component, nor serialized.
///
/// Hidden-flagged ancestor and its child tree all get flagged, so render filter need not walk parent chain
/// again in hot path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hidden(pub bool);
