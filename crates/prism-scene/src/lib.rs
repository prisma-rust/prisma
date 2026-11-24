//! Scene schema layer: ECS scene components, the component registry, the
//! authoring `.scene.json` shape, and the compact RSCN v4 cooked format.
//!
//! This crate is **runtime-agnostic**: it does not depend on Vulkan, audio,
//! windowing, or the `.pak` runtime. The cooker uses it to turn authoring JSON
//! into RSCN; the engine uses it to turn RSCN back into ECS components. Both
//! sides therefore share one schema and one registry.

pub mod animation;
pub mod components;
pub mod cook_stamp;
pub mod helpers;
pub mod json;
pub mod manifest;
pub mod migrate;
pub mod references;
pub mod registry;
pub mod render_enums;
pub mod rscn;
pub mod sprite_sheet;
pub mod terrain;
pub mod ui;
pub mod voxel;

pub use components::*;
pub use helpers::{is_active_in_hierarchy, world_matrix, HierarchyHelper};
pub use json::{EntityJson, SceneJson, TransformJson};
pub use manifest::{SceneManifest, SceneManifestEntry};
pub use migrate::{doc_version, migrate_scene_doc, normalize_scene_doc, SCENE_DOC_VERSION};
pub use registry::{
    builtin_registry, builtin_schema_fingerprint, register_builtin_components, ComponentRegistry,
};
pub use render_enums::{
    correlated_color_temperature_to_rgb, BlendMode, CameraClearFlags, CompareFunction, CullMode,
    LightRenderMode, LightShadows, LightType, RenderQueue, ShadowCastingMode,
};
pub use rscn::{decode_scene, encode_scene, DecodedEntity, DecodedScene, RscnError};
pub use sprite_sheet::{SpriteAtlasConfig, SpriteSliceDef};
pub use terrain::SculptMode;
pub use voxel::{
    aabb_intersects_solid_indexed, pack_rgba, pack_visual, raycast_chunks, raycast_chunks_indexed,
    unpack_rgba, visual_emissive, visual_flags, visual_roughness, VoxelBrick, VoxelChunk,
    VoxelChunkIndex, VoxelCursorHighlight, VoxelHit, VoxelTier, VoxelVisualFlags,
    DEFAULT_CHUNK_DIMS_L1, L1_UNIFORM_REF, VOXEL_BRICK_CELLS, VOXEL_L1_SIZE_M, VOXEL_L2_SIZE_M,
    VOXEL_L3_SIZE_M, VOXEL_TIER_SUBDIV,
};

/// Authoring prefix for procedural builtin meshes (`__builtin/box`, …).
///
/// The renderer generates these by name instead of cooking them, so the
/// cooker and the asset resolver both skip packaging them.
pub const BUILTIN_MESH_PREFIX: &str = "__builtin/";
