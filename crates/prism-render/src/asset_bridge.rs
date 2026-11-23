//! Main-thread (CPU) ↔ render-thread (GPU) asset parse request/result bridge types.
//!
//! These types are pure data (`Send`), passed via [`prism_app::RenderShared`] channel:
//! - Main thread prepares upload input (CPU: load `.pak` + deinterleave vertex / texture), does not touch `GraphRenderer`;
//! - Render thread (`GraphRenderer::apply_asset_requests`) executes GPU upload and returns handles.
//!
//! So asset parsing can run async, without requiring `&mut World` and `&mut GraphRenderer`
//! simultaneously on same thread — during startup renderer is built on render thread while main thread still handles window events.

use crate::managers::{MeshHandle, MeshUploadInput, TextureUploadInput};

/// CPU-side data needed for material parsing: 18 PBR scalars + 5 texture pixels
/// (fixed order: albedo, normal, metallic_roughness, emissive, occlusion).
/// Render thread uploads textures to get bindless slots, then assembles `MaterialUploadInput`.
pub struct MaterialResolveData {
    pub scalars: [f32; 18],
    /// Alpha semantics from the cooked RMAT. Geometry whose material is
    /// `Opaque` can use the AS `OPAQUE` flag and skip per-candidate alpha tests.
    pub alpha_mode: crate::managers::MaterialAlphaMode,
    pub alpha_cutoff: f32,
    pub textures: [Option<TextureUploadInput>; 5],
}

/// One entity GPU upload request (pure data, Send).
pub struct AssetResolveRequest {
    pub mesh: Option<MeshUploadInput>,
    /// Skinning attributes (when `.rskin` vertices carry joint/weight; #560 P0-3).
    pub skin: Option<crate::managers::SkinAttributes>,
    /// Deduplicated material list; `materials[i]` corresponds to `AssetResolveResult::material_slots[i]`.
    /// Multiple submeshes of same path share one entry.
    pub materials: Vec<Option<MaterialResolveData>>,
    /// 2D sprite texture (`SpriteRenderer.sprite_path` parse result).
    pub texture: Option<TextureUploadInput>,
    /// Optional custom fragment shader SPIR-V bytecode (`ShaderAsset`).
    pub shader: Option<Vec<u8>>,
}

/// One decided-to-release GPU resource (pure data, Send).
///
/// Main thread does not know renderer internal handles, only slot bits / handle values stored in components, so returned
/// release request uses same values; render thread looks up and destroys by value (#495).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AssetRelease {
    /// Bindless SRV slot bit.
    pub texture_slot: Option<u32>,
    /// Mesh handle.
    pub mesh_handle: Option<MeshHandle>,
    /// Material SSBO slot bit (first, keeps old field semantics).
    pub material_slot: Option<u32>,
    /// All material slot bits occupied by this entity (multi-submesh).
    pub material_slots: Vec<u32>,
}

impl AssetRelease {
    /// Whether contains any resource to release.
    pub fn is_empty(&self) -> bool {
        self.texture_slot.is_none()
            && self.mesh_handle.is_none()
            && self.material_slot.is_none()
            && self.material_slots.is_empty()
    }
}

/// Handles returned after render thread completes upload (pure data, Send).
#[derive(Default)]
pub struct AssetResolveResult {
    pub mesh_handle: Option<MeshHandle>,
    /// First material slot (for legacy code reading only that field).
    pub material_slot: Option<u32>,
    /// Slot bits one-to-one with request `materials`; `None` = that entry load/upload failed.
    pub material_slots: Vec<Option<u32>>,
    /// Sprite texture bindless SRV slot bit.
    pub texture_slot: Option<u32>,
    /// Sprite texture pixel size `[width, height]` (valid only when `texture_slot` valid).
    pub texture_size: Option<[u32; 2]>,
    /// Registered custom fragment shader handle.
    pub shader_handle: Option<crate::ShaderHandle>,
}
