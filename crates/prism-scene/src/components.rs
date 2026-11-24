//! ECS components of the modern scene system
//!
//! | Category     | Component |
//! |------------|------------|
//! | Hierarchy | `Parent`, `Child` |
//! | Transform | `LocalTransform`, `LocalToWorld`, `TransformDirty` |
//! | Render | `MeshRenderer`, `LodGroup` (distance LOD), `SpriteRenderer`, `Tilemap`, `Active` |
//! | Animation | `SpriteAnimation` (2D frames), `KeyframeAnimation` (curves) |
//! | Particles | `ParticleSystem` (world-space 3D CPU particles), `ParticleEmitter2D` (world XY plane 2D particles), `UiParticleEmitter` (screen space, drawn on UI layer) |
//! | Light | `Light` (shape), `DirectionalLight`, `PointLight`, `SpotLight` (legacy), `Light2D` (2D sprite lighting) |
//! | Camera | `Camera`, `VirtualCamera2D`, `FlyCameraController` |
//! | Skybox | `Skybox` |
//! | Audio | `AudioSource` (play in scene), `AudioListener` (listener) |
//! | Terrain | `Terrain` (heightfield → runtime-generated mesh) |
//! | Skinning | `SkeletonRoot` (skeleton), `SkinnedMeshRenderer` (skinned mesh) |
//! | Physics | `Collider` (shape + query layer), `Rigidbody` (Static/Kinematic/Dynamic) |
//! | Debug | `Gizmo` |
//! | Scene | `SceneMember` |
//! | Identity | `Name` |

use prism_ecs::Entity;
use prism_reflect::{FieldType, Reflect, ReflectError, ReflectType, Value};
use serde::{Deserialize, Serialize};

use crate::render_enums::{
    correlated_color_temperature_to_rgb, BlendMode, BlendOp, CameraClearFlags, CullMode,
    LightRenderMode, LightShadows, LightType, RenderQueue, ShadowCastingMode,
};
use prism_physics2d::Aabb2;

// ---------------------------------------------------------------------------
// SceneAssetId
// ---------------------------------------------------------------------------

/// 64-bit asset identifier used by scene schema.
///
/// Consistent with `prism_asset::core::AssetId` binary representation, but kept as a local type,
/// so `prism-scene` does not depend on runtime `.pak` code; engine converts at asset resolution boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SceneAssetId(pub u64);

impl ReflectType for SceneAssetId {
    fn field_type() -> FieldType {
        FieldType::U64
    }

    fn to_value(&self) -> Value {
        Value::U64(self.0)
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        Ok(Self(value.as_u64()?))
    }
}

impl SceneAssetId {
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub fn generate() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

// ---------------------------------------------------------------------------
// Hierarchy
// ---------------------------------------------------------------------------

/// Reference to parent entity. Corresponds to data-oriented `.Transforms.Parent` (`.0` ≡ `.Value`).
///
/// An entity without `Parent` component is a **root node** in scene hierarchy. Use
/// [`HierarchyHelper`](super::helpers::HierarchyHelper) to modify parent-child relationships --- do not modify `Child` directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parent(pub Entity);

impl Parent {
    /// DOTS `Parent.Value`: parent entity.
    pub const fn value(&self) -> Entity {
        self.0
    }
}

/// Derived child entity list. Corresponds to data-oriented `.Transforms.Child` (buffer).
///
/// Kept in sync by [`HierarchyHelper::reparent`]; **do not** modify manually.
/// Entities without this component have no children.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Child(pub Vec<Entity>);

impl Child {
    /// Child entity slice.
    pub fn value(&self) -> &[Entity] {
        &self.0
    }

    /// Child entity list (mutable).
    pub fn value_mut(&mut self) -> &mut Vec<Entity> {
        &mut self.0
    }
}

// ---------------------------------------------------------------------------
// Transform
// ---------------------------------------------------------------------------

/// Local transform relative to parent entity (world space for root nodes).
///
/// Corresponds to data-oriented `.Transforms.LocalTransform`
/// (`position` / `rotation` / `scale` ≡ DOTS `Position` / `Rotation` / `Scale`).
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::LocalTransform")]
#[serde(default)]
pub struct LocalTransform {
    pub position: glam::Vec3,
    /// Quaternion.  Identity = `glam::Quat::IDENTITY`.
    pub rotation: glam::Quat,
    pub scale: glam::Vec3,
}

impl Default for LocalTransform {
    fn default() -> Self {
        Self {
            position: glam::Vec3::ZERO,
            rotation: glam::Quat::IDENTITY,
            scale: glam::Vec3::ONE,
        }
    }
}

impl LocalTransform {
    /// Build a model matrix: `T × R × S`.
    pub fn to_model_matrix(&self) -> glam::Mat4 {
        glam::Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.position)
    }
}

/// World-space transform matrix computed by `HierarchySystem`, updated every frame.
///
/// Corresponds to data-oriented `.Transforms.LocalToWorld` (`.0` ≡ DOTS `.Value`).
#[derive(Debug, Clone, Copy)]
pub struct LocalToWorld(pub glam::Mat4);

impl LocalToWorld {
    /// DOTS `LocalToWorld.Value`: world transform matrix.
    pub const fn value(&self) -> glam::Mat4 {
        self.0
    }
}

/// Optional dirty marker for subtree-optimised recompute future use).
#[derive(Debug, Clone, Copy, Default)]
pub struct TransformDirty(pub bool);

/// Runtime-only 2D transform history used to render fixed-step entities smoothly.
///
/// The simulation always reads and writes `LocalTransform` / `LocalToWorld`.
/// This component stores world-space XY samples for render interpolation and is
/// never authored in a scene document.
#[derive(Debug, Clone, Copy, PartialEq, Reflect)]
#[reflect(name = "prism_engine::scene::TransformInterpolation2D")]
pub struct TransformInterpolation2D {
    pub previous: [f32; 2],
    pub current: [f32; 2],
}

impl Default for TransformInterpolation2D {
    fn default() -> Self {
        Self {
            previous: [0.0; 2],
            current: [0.0; 2],
        }
    }
}

impl TransformInterpolation2D {
    pub fn sample(&self, alpha: f32) -> [f32; 2] {
        let alpha = if alpha.is_finite() {
            alpha.clamp(0.0, 1.0)
        } else {
            0.0
        };
        [
            self.previous[0] + (self.current[0] - self.previous[0]) * alpha,
            self.previous[1] + (self.current[1] - self.previous[1]) * alpha,
        ]
    }
}

// ---------------------------------------------------------------------------
// Render references
// ---------------------------------------------------------------------------

/// Active state — whether the entity participates in rendering.
///
/// Defaults to `true`.  Set to `false` to hide an entity without despawning it
/// (the entity and its components remain in the world).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::Active")]
pub struct Active(pub bool);

impl Default for Active {
    fn default() -> Self {
        Self(true)
    }
}

/// Cross-scene persistence marker (Unity `DontDestroyOnLoad` equivalent).
///
/// When switching scenes (`Engine::unload_scene` or `LoadSceneMode::Single` load),
/// entities with this marker **and their entire subtree** are not unloaded --- player manager, audio sources,
/// transition masks etc need to survive scene change. Added at runtime only, not a mountable component,
/// therefore does not appear in `.scene.json` (see `json::DERIVED_COMPONENTS`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Persistent;

/// Derived: AND of own [`Active`] and all ancestors `Active` (Unity
/// `activeInHierarchy`).
///
/// Overwritten every frame by `prism_engine::scene::systems::hierarchy::active_hierarchy_system`,
/// runtime derived data of the same kind as [`LocalToWorld`]: **not in `.scene.json`, not in component registry**.
/// Author/inspector always writes [`Active`] (Unity `activeSelf`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveInHierarchy(pub bool);

impl Default for ActiveInHierarchy {
    fn default() -> Self {
        Self(true)
    }
}

// ---------------------------------------------------------------------------
// Authoring-time helpers
// ---------------------------------------------------------------------------

/// Authoring-time bundle marker for a renderable entity.
///
/// Inserted by [`SceneLoader`](crate::scene::loader::SceneLoader) during
/// scene generation when an entity carries a mesh path; the material path is optional
/// (a mesh-only renderer still draws, using the renderer default material).
/// This component does **not** participate in render queries — the actual
/// rendering data lives on [`MeshRef`] and [`MaterialRef`] — but provides a
/// convenient handle for inspector editing and future hot-reload.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::MeshRenderer")]
#[serde(default)]
pub struct MeshRenderer {
    pub mesh_path: String,
    /// Single material path (compatible with old scenes; used when `material_paths` is empty).
    pub material_path: String,
    /// One material path per submesh (#520).
    ///
    /// Length usually equals number of submeshes; falls back to [`Self::material_path`] when empty.
    /// When length insufficient, takes source material index from submesh (glTF `material.index()`),
    /// then falls back to `material_path`. Empty string means submesh uses default material.
    pub material_paths: Vec<String>,
    /// Optional custom fragment shader asset path (`.frag.spv` / `.slang`).
    /// Empty uses the built-in PBR fragment shader (`scene_frag.frag.spv`).
    pub shader_path: String,
    /// Shadow casting mode = `Renderer.shadowCastingMode`.
    ///
    /// Determines whether object writes shadow map, whether it appears in camera main pass (`ShadowsOnly` only projects).
    pub shadow_casting_mode: ShadowCastingMode,
    /// Cull mode = material/renderer `CullMode` (shader property `_Cull`).
    ///
    /// Put it on `Material`; Prisma material currently only has PBR parameters,
    /// so lands on renderer for now, semantics still "which face this draw uses".
    pub cull_mode: CullMode,
    /// Render queue = `Material.renderQueue` (`int`, not enum).
    ///
    /// Determines draw order: opaque objects less than [`RenderQueue::GEOMETRY_LAST`] drawn first,
    /// then (transparent) queue, transparent queue internally sorted far-to-near by distance.
    pub render_queue: RenderQueue,
    /// Whether to write depth = `_ZWrite`. Transparent objects usually turned off.
    pub z_write: bool,
    /// Source blend factor = `_SrcBlend`.
    pub src_blend: BlendMode,
    /// Destination blend factor = `_DstBlend`.
    pub dst_blend: BlendMode,
    /// Blend operation = `_BlendOp`.
    pub blend_op: BlendOp,
}

impl Default for MeshRenderer {
    fn default() -> Self {
        Self {
            mesh_path: String::new(),
            material_path: String::new(),
            material_paths: Vec::new(),
            shader_path: String::new(),
            shadow_casting_mode: ShadowCastingMode::default(),
            cull_mode: CullMode::default(),
            render_queue: RenderQueue::default(),
            z_write: true,
            // Engine opaque default: One / Zero / Add (see `blends`).
            src_blend: BlendMode::One,
            dst_blend: BlendMode::Zero,
            blend_op: BlendOp::Add,
        }
    }
}

impl MeshRenderer {
    pub fn has_mesh(&self) -> bool {
        !self.mesh_path.is_empty()
    }
    pub fn has_material(&self) -> bool {
        !self.material_path.is_empty() || self.material_paths.iter().any(|path| !path.is_empty())
    }

    /// Which material path should submesh `submesh` use (#520).
    ///
    /// Resolution order:
    /// 1. When `material_paths` matches submesh count one-to-one, use same index directly;
    /// 2. Otherwise if source file material index known (glTF), take by material index;
    /// 3. Then fall back to same index (for short list, items beyond last decided by caller default);
    /// 4. Finally fall back to single field [`Self::material_path`].
    pub fn material_path_for_submesh(
        &self,
        submesh: usize,
        submesh_count: usize,
        material_index: Option<u32>,
    ) -> Option<&str> {
        if !self.material_paths.is_empty() {
            if self.material_paths.len() == submesh_count {
                if let Some(path) = self
                    .material_paths
                    .get(submesh)
                    .filter(|path| !path.is_empty())
                {
                    return Some(path.as_str());
                }
            }
            if let Some(index) = material_index {
                if let Some(path) = self
                    .material_paths
                    .get(index as usize)
                    .filter(|path| !path.is_empty())
                {
                    return Some(path.as_str());
                }
            }
            if let Some(path) = self
                .material_paths
                .get(submesh)
                .filter(|path| !path.is_empty())
            {
                return Some(path.as_str());
            }
        }

        (!self.material_path.is_empty()).then_some(self.material_path.as_str())
    }
    /// Whether blending is actually needed: `One` / `Zero` means "no blend",
    /// so default value means opaque.
    pub fn blends(&self) -> bool {
        self.src_blend != BlendMode::One
            || self.dst_blend != BlendMode::Zero
            || self.blend_op != BlendOp::Add
    }
}

// ---------------------------------------------------------------------------
// Distance LOD
// ---------------------------------------------------------------------------

/// Distance-based LOD (#491).
///
/// Attached to parent entity, parent entity [`Child`] list **order IS LOD order**: first child
/// is LOD0 (nearest, finest), second is LOD1, etc. Runtime system selects level by distance to nearest enabled
/// camera distance, and uses runtime `LodHidden` to hide other levels; author [`Active`] and
/// editor `Hidden` are not overwritten.
///
/// `distances[i]` is max distance at which level `i` is still shown (meters). When distance exceeds all thresholds,
/// show last child entity; when `distances` empty always show first one.
///
/// Example: `distances = [20.0, 60.0]` + three children = LOD0 within 20m, 20-60m
/// LOD1, LOD2 beyond 60m.
///
/// Can also use **screen ratio** level selection: when `screen_ratios` non-empty, use
/// `bounds_radius` (world-space bounding sphere radius) and camera projection to compute object height
/// ratio, then select level by threshold. `screen_ratios[i]` is minimum ratio at which level `i` is still shown,
/// arranged LOD0, LOD1... from large to small; beyond last threshold show last child entity.
/// Screen ratio mode takes priority over `distances`; when empty continues distance mode.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::LodGroup")]
#[serde(default)]
pub struct LodGroup {
    /// Max distance at which each level still shown (meters), arranged LOD0, LOD1...
    pub distances: Vec<f32>,
    /// Minimum screen height ratio at which each level still shown (0-1), arranged LOD0, LOD1... from large to small.
    ///
    /// When empty uses `distances` distance mode.
    pub screen_ratios: Vec<f32>,
    /// Bounding sphere radius (world units) used in screen ratio mode.
    ///
    /// When non-finite or non-positive falls back to distance mode, avoiding accidentally hiding whole group.
    pub bounds_radius: f32,
}

impl Default for LodGroup {
    fn default() -> Self {
        Self {
            distances: vec![20.0, 60.0],
            screen_ratios: Vec::new(),
            bounds_radius: 1.0,
        }
    }
}

impl LodGroup {
    /// Given distance and child count, return LOD index that should be shown.
    ///
    /// Degenerate input handled as "show most detailed level": no children returns `None`, non-finite distance
    /// treated as 0, negative thresholds treated as 0. Distance beyond all thresholds clamped to last child.
    pub fn level_for_distance(&self, distance: f32, child_count: usize) -> Option<usize> {
        if child_count == 0 {
            return None;
        }
        if self.distances.is_empty() {
            return Some(0);
        }
        let distance = if distance.is_finite() {
            distance.max(0.0)
        } else {
            0.0
        };
        let index = self
            .distances
            .iter()
            .position(|limit| {
                let limit = if limit.is_finite() {
                    limit.max(0.0)
                } else {
                    f32::INFINITY
                };
                distance <= limit
            })
            .unwrap_or(self.distances.len());
        Some(index.min(child_count - 1))
    }

    /// Given screen height ratio and child count, return LOD index that should be shown.
    ///
    /// When `screen_ratios` empty or no finite thresholds returns `None`, caller should fall back to
    /// [`Self::level_for_distance`]. Non-finite ratio treated as 0; thresholds clamped to `0..=1`
    /// and matched in author order, so thresholds should be arranged LOD0, LOD1... from large to small.
    pub fn level_for_screen_ratio(&self, ratio: f32, child_count: usize) -> Option<usize> {
        if child_count == 0 {
            return None;
        }
        let thresholds: Vec<f32> = self
            .screen_ratios
            .iter()
            .copied()
            .filter(|value| value.is_finite())
            .map(|value| value.clamp(0.0, 1.0))
            .collect();
        if thresholds.is_empty() {
            return None;
        }
        let ratio = if ratio.is_finite() {
            ratio.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let index = thresholds
            .iter()
            .position(|threshold| ratio >= *threshold)
            .unwrap_or(thresholds.len());
        Some(index.min(child_count - 1))
    }
}

// ---------------------------------------------------------------------------
// 2D Sprite
// ---------------------------------------------------------------------------

/// Built-in 1x1 pure-white sprite texture identifier.
///
/// Resolved directly to the renderer's built-in 1x1 white texture without `.pak` lookup.
/// Combine with `SpriteRenderer::color` and `LocalTransform::scale` to draw solid-color
/// world-space rectangles through the standard 2D sprite batching pipeline.
pub const BUILTIN_WHITE_SPRITE: &str = "__builtin/white";

/// 2D sprite renderer = `SpriteRenderer` (rendering subset).
///
/// Only carries **authoring-time** fields: texture path, sprite sheet frames, sorting layer. GPU texture handle at runtime held by
/// `SpriteRef` (`prism-engine::scene::runtime_components`) held --- same relationship as
/// `MeshRenderer` → `MeshRef`.
///
/// 2D convention (see `projects/demos/fishing/DESIGN.md`):
/// Camera looks **+Z towards −Z**, larger `z` closer to camera; world size =
/// texture pixel size / [`SpriteRenderer::pixels_per_unit`].
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::SpriteRenderer")]
#[serde(default)]
pub struct SpriteRenderer {
    /// Texture resource path = `SpriteRenderer.sprite`. Empty string = do not draw.
    pub sprite_path: String,
    /// Sprite sheet single frame pixel size; `[0, 0]` means whole texture is one frame.
    pub frame_size: [u32; 2],
    /// Optional sub-texture pixel rectangle `[x, y, width, height]` inside a sprite atlas.
    ///
    /// When `width > 0 && height > 0`, UV coordinates and world size are derived from this
    /// sub-rectangle (advancing horizontally by `frame_index * width`), taking precedence over
    /// `frame_size` / `sheet_row`. `[0, 0, 0, 0]` disables custom region cropping.
    pub region_px: [u32; 4],
    /// Atlas columns per row; `0` = derived from "texture width / single frame width" (#584).
    ///
    /// In multi-row atlas, one row is usually one action / one character.
    pub sheet_columns: u32,
    /// Atlas row number (0 = first row); clamped to last row when out of range (#584).
    pub sheet_row: u32,
    /// Current frame index (left-to-right within row; continues to next row beyond column count).
    pub frame_index: u32,
    /// Pixels per world unit = `Sprite.pixelsPerUnit`.
    pub pixels_per_unit: f32,
    /// Pivot (`0..1`, **bottom-left** as origin) = normalized form of `Sprite.pivot`.
    /// `[0.5, 0.5]` = centered, `[0.5, 0.0]` = bottom-centered, `[0.5, 1.0]` = top-centered.
    pub pivot: [f32; 2],
    /// Vertex color multiply (linear RGBA) = `SpriteRenderer.color`.
    pub color: [f32; 4],
    /// Horizontal flip = `SpriteRenderer.flipX`.
    pub flip_x: bool,
    /// Vertical flip = `SpriteRenderer.flipY`.
    pub flip_y: bool,
    /// Sorting layer, larger value more front = numeric form of `sortingLayerName`.
    pub sorting_layer: i32,
    /// Order within layer = `sortingOrder`.
    pub order_in_layer: i32,
    /// Source blend factor (default `SrcAlpha`, standard alpha blend, #610).
    pub src_blend: BlendMode,
    /// Destination blend factor (default `OneMinusSrcAlpha`).
    pub dst_blend: BlendMode,
    /// Blend operation (default `Add`); additive glow uses `SrcAlpha` / `One`.
    pub blend_op: BlendOp,
    /// Whether to clip this sprite using [`SpriteRenderer::clip_rect`] (#582).
    ///
    /// Clipping judged per-pixel in fragment shader by **world coordinates**, so does not affect one draw call,
    /// same waterline rectangle can be shared by arbitrarily many sprites (reflections, whole group of underwater decorations).
    /// Multiple sprites using same rectangle = Unity `SpriteMask` rectangle mode.
    pub clip_enabled: bool,
    /// World-space clip rect `[x0, y0, x1, y1]` (`x0 <= x1`, `y0 <= y1` not required,
    /// engine sorts itself); only takes effect when `clip_enabled` is true.
    ///
    /// Enabling a zero-area rectangle clips the entire sprite---fill rect first then enable switch.
    pub clip_rect: [f32; 4],
    /// Whether illuminated by [`Light2D`] (#611).
    ///
    /// Default `false`: 2D sprite keeps appearance before lighting introduced (`texture × color`).
    /// When turned on, fragments accumulate scene `Light2D` by world position, then add ambient light ---
    /// when no `Light2D` still treated as unlit, so changing only this switch won't make screen black.
    pub receive_lights: bool,
}

impl Default for SpriteRenderer {
    fn default() -> Self {
        Self {
            sprite_path: String::new(),
            frame_size: [0, 0],
            region_px: [0, 0, 0, 0],
            sheet_columns: 0,
            sheet_row: 0,
            frame_index: 0,
            pixels_per_unit: 100.0,
            pivot: [0.5, 0.5],
            color: [1.0, 1.0, 1.0, 1.0],
            flip_x: false,
            flip_y: false,
            sorting_layer: 0,
            order_in_layer: 0,
            src_blend: BlendMode::SrcAlpha,
            dst_blend: BlendMode::OneMinusSrcAlpha,
            blend_op: BlendOp::Add,
            clip_enabled: false,
            clip_rect: [0.0; 4],
            receive_lights: false,
        }
    }
}

impl SpriteRenderer {
    /// Construct a solid-color sprite backed by [`BUILTIN_WHITE_SPRITE`] (`1x1` white texture).
    ///
    /// With `pixels_per_unit = 1.0`, `LocalTransform::scale` specifies the rectangle size directly
    /// in world units.
    pub fn white(color: [f32; 4]) -> Self {
        Self {
            sprite_path: BUILTIN_WHITE_SPRITE.to_string(),
            pixels_per_unit: 1.0,
            color,
            ..Default::default()
        }
    }

    /// Whether there is a drawable texture.
    pub fn has_sprite(&self) -> bool {
        !self.sprite_path.is_empty()
    }

    /// Single frame pixel size; when sprite sheet not configured caller falls back to texture size.
    pub fn frame_size_or(&self, texture_size: [u32; 2]) -> [u32; 2] {
        if self.region_px[2] > 0 && self.region_px[3] > 0 {
            [self.region_px[2], self.region_px[3]]
        } else if self.frame_size[0] > 0 && self.frame_size[1] > 0 {
            self.frame_size
        } else {
            texture_size
        }
    }

    /// Atlas columns: explicit `sheet_columns` takes priority, otherwise derived from texture width / single frame width.
    pub fn sheet_columns_or(&self, texture_size: [u32; 2]) -> u32 {
        if self.sheet_columns > 0 {
            return self.sheet_columns.max(1);
        }
        let frame = self.frame_size_or(texture_size);
        (texture_size[0] / frame[0].max(1)).max(1)
    }

    /// Atlas row count (at least 1).
    pub fn sheet_rows(&self, texture_size: [u32; 2]) -> u32 {
        let frame = self.frame_size_or(texture_size);
        (texture_size[1] / frame[1].max(1)).max(1)
    }

    /// Single frame size in world space (units/world). Falls back to 100 when `pixels_per_unit` invalid.
    pub fn world_size(&self, texture_size: [u32; 2]) -> [f32; 2] {
        let ppu = if self.pixels_per_unit.is_finite() && self.pixels_per_unit > 0.0 {
            self.pixels_per_unit
        } else {
            100.0
        };
        let frame = self.frame_size_or(texture_size);
        [frame[0] as f32 / ppu, frame[1] as f32 / ppu]
    }

    /// World-space AABB of the visible sprite frame, including pivot, scale and rotation.
    pub fn world_bounds(
        &self,
        transform: &LocalTransform,
        texture_size: [u32; 2],
    ) -> Option<Aabb2> {
        let frame = self.frame_size_or(texture_size);
        if frame[0] == 0 || frame[1] == 0 || !self.pixels_per_unit.is_finite() {
            return None;
        }
        let ppu = if self.pixels_per_unit > 0.0 {
            self.pixels_per_unit
        } else {
            100.0
        };
        let size = [frame[0] as f32 / ppu, frame[1] as f32 / ppu];
        transformed_aabb2(
            [-self.pivot[0] * size[0], -self.pivot[1] * size[1]],
            [
                (1.0 - self.pivot[0]) * size[0],
                (1.0 - self.pivot[1]) * size[1],
            ],
            transform.to_model_matrix(),
        )
    }
}

/// Axis-aligned 2D collider authored relative to an entity's transform.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::Collider2D")]
#[serde(default)]
pub struct Collider2D {
    pub offset: [f32; 2],
    pub half_extents: [f32; 2],
    pub is_trigger: bool,
    pub layer: u32,
}

impl Default for Collider2D {
    fn default() -> Self {
        Self {
            offset: [0.0; 2],
            half_extents: [0.5; 2],
            is_trigger: false,
            layer: 1,
        }
    }
}

impl Collider2D {
    /// Return this collider's world-space AABB using an entity-local transform.
    ///
    /// This is the root-entity convenience path. For parented entities, callers
    /// should prefer [`Self::world_bounds_with_matrix`] with the hierarchy's
    /// `LocalToWorld` matrix so physics and rendering use the same transform.
    pub fn world_bounds(&self, transform: &LocalTransform) -> Option<Aabb2> {
        self.world_bounds_with_matrix(transform.to_model_matrix())
    }

    /// Return this collider's world-space AABB from a complete world matrix.
    ///
    /// The authored offset and half extents are transformed as a rectangle and
    /// reduced to an axis-aligned bound, matching sprite bounds and the 2D
    /// physics query contract. Non-finite matrices or coordinates return `None`.
    pub fn world_bounds_with_matrix(&self, matrix: glam::Mat4) -> Option<Aabb2> {
        let half = [self.half_extents[0].abs(), self.half_extents[1].abs()];
        transformed_aabb2(
            [self.offset[0] - half[0], self.offset[1] - half[1]],
            [self.offset[0] + half[0], self.offset[1] + half[1]],
            matrix,
        )
    }
}

fn transformed_aabb2(
    local_min: [f32; 2],
    local_max: [f32; 2],
    matrix: glam::Mat4,
) -> Option<Aabb2> {
    let corners = [
        [local_min[0], local_min[1]],
        [local_max[0], local_min[1]],
        [local_min[0], local_max[1]],
        [local_max[0], local_max[1]],
    ];
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for [x, y] in corners {
        let point = matrix.transform_point3(glam::Vec3::new(x, y, 0.0));
        if !point.x.is_finite() || !point.y.is_finite() {
            return None;
        }
        min[0] = min[0].min(point.x);
        min[1] = min[1].min(point.y);
        max[0] = max[0].max(point.x);
        max[1] = max[1].max(point.y);
    }
    Some(Aabb2 { min, max })
}

/// 2D tilemap = `Tilemap` (#529).
///
/// One entity carries whole grid: engine expands **non-empty** cells each frame into `SpriteDrawItem`
/// (sharing `SpritePass` instance buffer and bindless texture table), so hundreds of cells still have only
/// one draw call, instead of hundreds of entities each with a `SpriteRenderer`.
///
/// Convention:
/// - `tiles` is row-major flat array, `0` = empty cell (consistent with Tiled GID 0),
///   `n > 0` = `n - 1`-th tile in tileset (left-to-right, top-to-bottom);
/// - `columns == 0` means all tiles arranged in one row;
/// - Row 0 at **top** (world `+Y` is screen top), so larger row number means smaller world `y`;
/// - Grid origin = entity world position (parent transform, rotation, scale all take effect).
///
/// Shares atlas with `SpriteRenderer`: `tile_size` is pixel size of single tile in tileset,
/// actual column count derived at runtime from "texture width / `tile_size.x`" (same as sprite sheet).
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::Tilemap")]
#[serde(default)]
pub struct Tilemap {
    /// tileset texture path; empty = do not draw.
    pub tileset_path: String,
    /// Pixel size of single tile in tileset; `[0, 0]` = whole image is one tile.
    pub tile_size: [u32; 2],
    /// World size per cell.
    pub cell_size: [f32; 2],
    /// Grid column count; `0` = all tiles arranged in one row.
    pub columns: u32,
    /// Row-major tile index; `0` = empty cell, `n > 0` = `n - 1`-th tile.
    pub tiles: Vec<u32>,
    /// Vertex color multiply (linear RGBA).
    pub color: [f32; 4],
    /// Sorting layer, larger value more front (same sorting key set as `SpriteRenderer`).
    pub sorting_layer: i32,
    /// Order within layer.
    pub order_in_layer: i32,
}

impl Default for Tilemap {
    fn default() -> Self {
        Self {
            tileset_path: String::new(),
            tile_size: [16, 16],
            cell_size: [1.0, 1.0],
            columns: 0,
            tiles: Vec::new(),
            color: [1.0, 1.0, 1.0, 1.0],
            sorting_layer: 0,
            order_in_layer: 0,
        }
    }
}

impl Tilemap {
    /// Whether there is drawable content.
    pub fn has_tiles(&self) -> bool {
        !self.tileset_path.is_empty()
            && self.tiles.iter().any(|tile| *tile != 0)
            && self.cell_size[0].is_finite()
            && self.cell_size[0] > 0.0
            && self.cell_size[1].is_finite()
            && self.cell_size[1] > 0.0
    }

    /// Grid scale `(columns, rows)`; returns `(1, 0)` when `tiles` empty.
    pub fn grid(&self) -> (u32, u32) {
        let columns = if self.columns > 0 {
            self.columns as usize
        } else {
            self.tiles.len().max(1)
        };
        let rows = self.tiles.len().div_ceil(columns.max(1));
        (columns.max(1) as u32, rows as u32)
    }

    /// Pixel size of single tile in tileset (degrades to whole texture when `tile_size` invalid).
    pub fn tile_size_or(&self, texture_size: [u32; 2]) -> [u32; 2] {
        if self.tile_size[0] > 0 && self.tile_size[1] > 0 {
            self.tile_size
        } else {
            [texture_size[0].max(1), texture_size[1].max(1)]
        }
    }

    /// Tileset column count / row count.
    pub fn tileset_grid(&self, texture_size: [u32; 2]) -> (u32, u32) {
        let tile = self.tile_size_or(texture_size);
        (
            (texture_size[0] / tile[0].max(1)).max(1),
            (texture_size[1] / tile[1].max(1)).max(1),
        )
    }

    /// UV rect `[u0, v0, u1, v1]` of tile `index` (0-based) in tileset.
    pub fn tile_uv(&self, index: u32, texture_size: [u32; 2]) -> [f32; 4] {
        let tile = self.tile_size_or(texture_size);
        let (columns, rows) = self.tileset_grid(texture_size);
        let slot = index.min(columns.saturating_mul(rows).saturating_sub(1));
        let column = slot % columns;
        let row = slot / columns;
        let (width, height) = (texture_size[0].max(1) as f32, texture_size[1].max(1) as f32);
        let (tile_w, tile_h) = (tile[0].max(1) as f32, tile[1].max(1) as f32);
        let u0 = column as f32 * tile_w / width;
        let v0 = row as f32 * tile_h / height;
        [
            u0,
            v0,
            ((column as f32 + 1.0) * tile_w / width).min(1.0),
            ((row as f32 + 1.0) * tile_h / height).min(1.0),
        ]
    }

    /// Local coordinates of cell center `row / column` (bottom-left as origin, `+Y` up).
    pub fn cell_center(&self, column: u32, row: u32, rows: u32) -> [f32; 2] {
        let (cell_w, cell_h) = (self.cell_size[0], self.cell_size[1]);
        [
            (column as f32 + 0.5) * cell_w,
            (rows as f32 - row as f32 - 0.5) * cell_h,
        ]
    }
}

/// 2D Sprite frame animation = minimal substitute for `Animator` + `AnimationClip`.
///
/// Only describes how to play; current frame written back to [`SpriteRenderer::frame_index`], advanced by engine
/// `sprite_animation_system` every frame (play progress does not enter scene, see that system docs).
///
/// Sprite sheet convention: single frame pixel size given by [`SpriteRenderer::frame_size`]; within a row
/// played left-to-right, row selected by [`SpriteRenderer::sheet_row`] (multi-row atlas, #584).
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::SpriteAnimation")]
#[serde(default)]
pub struct SpriteAnimation {
    /// Total frame count; `0` = derived from row column count (#584).
    pub frames: u32,
    /// Start frame within row (`0` = start from first column); used to switch multiple actions in same row.
    pub frame_start: u32,
    /// Frames per second.
    pub fps: f32,
    /// Whether looped; `false` stops at last frame.
    pub looped: bool,
    /// Whether playing (paused keeps current frame).
    pub playing: bool,
    /// Phase offset (seconds), used to desync multiple entities.
    pub phase: f32,
}

impl Default for SpriteAnimation {
    fn default() -> Self {
        Self {
            frames: 0,
            frame_start: 0,
            fps: 12.0,
            looped: true,
            playing: true,
            phase: 0.0,
        }
    }
}

/// Keyframe animation clip: drives **own** `Transform` position / euler / scale.
///
/// Alongside [`SpriteAnimation`] (2D sprite frames, using `frame_index`), this is curve animation.
/// Playhead not placed in component (would serialize with `.scene.json`), but runtime component on engine side
/// component `ClipPlayhead`, see
/// `prism_engine::scene::systems::animation::keyframe_animation_system`.
///
/// Three tracks independent: channel without keyframes keeps author-written `transform` value,
/// therefore "only animate position" will not zero rotation / scale.
///
/// Rotation keyframes are XYZ Euler angles (**degrees**), same convention as [`DirectionalLight::euler_xyz`]
/// same convention; endpoints held (no extrapolation), so single playback ends at last frame.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::KeyframeAnimation")]
#[serde(default)]
pub struct KeyframeAnimation {
    /// Clip name (for log / editor display, does not affect playback).
    pub name: String,
    /// Clip duration (seconds); `0` = derived from last keyframe of three tracks.
    ///
    /// When longer than last frame, keeps last frame value after last frame.
    pub duration: f32,
    /// Playback multiplier; `0` equals pause, negative reverses.
    pub speed: f32,
    /// Whether looped; `false` plays to `duration` then stops at end frame.
    pub looped: bool,
    /// Whether playing (paused keeps current pose).
    pub playing: bool,
    /// Start time offset (seconds).
    pub phase: f32,
    /// Position track.
    pub position: Vec<crate::animation::Vec3Key>,
    /// Rotation track (XYZ Euler angles, degrees).
    pub euler_xyz: Vec<crate::animation::Vec3Key>,
    /// Scale track.
    pub scale: Vec<crate::animation::Vec3Key>,
    /// Clip events (#490): when playhead crosses event time, written to `AnimationEventQueue`.
    pub events: Vec<crate::animation::AnimationEvent>,
}

impl Default for KeyframeAnimation {
    fn default() -> Self {
        Self {
            name: "clip".into(),
            duration: 0.0,
            speed: 1.0,
            looped: true,
            playing: true,
            phase: 0.0,
            position: Vec::new(),
            euler_xyz: Vec::new(),
            scale: Vec::new(),
            events: Vec::new(),
        }
    }
}

impl KeyframeAnimation {
    /// Effective duration: use `duration > 0`, otherwise take latest time from three tracks / events.
    ///
    /// Clip with only events must also have length, otherwise playhead will not advance, events never trigger.
    pub fn clip_duration(&self) -> f32 {
        if self.duration > 0.0 && self.duration.is_finite() {
            return self.duration;
        }
        let last_event = self
            .events
            .iter()
            .map(|event| event.time)
            .filter(|time| time.is_finite())
            .fold(0.0f32, f32::max);
        crate::animation::max_key_time(&self.position)
            .max(crate::animation::max_key_time(&self.euler_xyz))
            .max(crate::animation::max_key_time(&self.scale))
            .max(last_event)
    }

    /// Whether all three tracks and events are empty (empty clip drives no channels, occupies no playhead).
    pub fn is_empty(&self) -> bool {
        self.position.is_empty()
            && self.euler_xyz.is_empty()
            && self.scale.is_empty()
            && self.events.is_empty()
    }

    /// Sample `local_time` (time already processed by loop / endpoints).
    pub fn sample(&self, local_time: f32) -> crate::animation::ClipSample {
        crate::animation::ClipSample {
            position: crate::animation::sample_vec3(&self.position, local_time),
            euler_xyz: crate::animation::sample_vec3(&self.euler_xyz, local_time),
            scale: crate::animation::sample_vec3(&self.scale, local_time),
        }
    }
}

/// Polyline corner join style (#604).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LineJoin {
    /// Miter: outer corner extends to intersection of two edges; degrades to
    /// [`Bevel`](Self::Bevel), avoiding sharp corners pulling out long spikes.
    #[default]
    Miter,
    /// Bevel: each segment ends at corner, triangle fills the gap.
    Bevel,
}

impl ReflectType for LineJoin {
    fn field_type() -> FieldType {
        FieldType::U8
    }

    fn enum_variants() -> Option<&'static [&'static str]> {
        Some(&["Miter", "Bevel"])
    }

    fn to_value(&self) -> Value {
        Value::U64(match self {
            Self::Miter => 0,
            Self::Bevel => 1,
        })
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        match value.as_u64()? {
            0 => Ok(Self::Miter),
            1 => Ok(Self::Bevel),
            other => Err(ReflectError::BadData(format!(
                "invalid LineJoin discriminant {other}"
            ))),
        }
    }
}

/// Polyline endpoint cap style (#604).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LineCap {
    /// Butt: line segment ends exactly at endpoint.
    #[default]
    Butt,
    /// Square: extends half line width beyond endpoint along segment direction.
    Square,
    /// Round: semicircle centered at endpoint.
    Round,
}

impl ReflectType for LineCap {
    fn field_type() -> FieldType {
        FieldType::U8
    }

    fn enum_variants() -> Option<&'static [&'static str]> {
        Some(&["Butt", "Square", "Round"])
    }

    fn to_value(&self) -> Value {
        Value::U64(match self {
            Self::Butt => 0,
            Self::Square => 1,
            Self::Round => 2,
        })
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        match value.as_u64()? {
            0 => Ok(Self::Butt),
            1 => Ok(Self::Square),
            2 => Ok(Self::Round),
            other => Err(ReflectError::BadData(format!(
                "invalid LineCap discriminant {other}"
            ))),
        }
    }
}

/// 2D polyline renderer = `LineRenderer` (2D subset).
///
/// Points are in **world space** (equivalent to `useWorldSpace = true`); engine triangulates polyline into
/// one **non-overlapping** thick stroke (corner join + endpoint cap) merged into sprite channel, so sorting /
/// blending consistent with sprite; translucent polyline will not double-overlap at corners (#604).
///
/// Line color goes through sprite channel `texture == u32::MAX` (solid color) branch, does not sample texture.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::LineRenderer")]
#[serde(default)]
pub struct LineRenderer {
    /// World-space polyline points; does not draw when fewer than 2 points.
    pub points: Vec<[f32; 3]>,
    /// Linear RGBA color.
    pub color: [f32; 4],
    /// Line width (world units).
    ///
    /// **Draws as 1 pixel when less than 1 render pixel** (#978): subpixel-wide geometry causes pixel
    /// centers to fall outside triangles, screen breaks into dots. Rendering converts 1 pixel to world
    /// length via main camera, width takes larger of the two.
    pub width: f32,
    /// Sorting layer, same numeric set as [`SpriteRenderer::sorting_layer`].
    pub sorting_layer: i32,
    /// Order within layer.
    pub order_in_layer: i32,
    /// Corner join style (default miter).
    pub join: LineJoin,
    /// Endpoint cap style (default butt).
    pub cap: LineCap,
    /// Miter length limit (multiple of half width); `Miter` degrades to bevel when exceeded.
    pub miter_limit: f32,
}

impl Default for LineRenderer {
    fn default() -> Self {
        Self {
            points: Vec::new(),
            color: [1.0, 1.0, 1.0, 1.0],
            width: 0.1,
            sorting_layer: 0,
            order_in_layer: 0,
            join: LineJoin::Miter,
            cap: LineCap::Butt,
            miter_limit: 4.0,
        }
    }
}

impl LineRenderer {
    /// Whether there is a drawable line segment.
    pub fn has_line(&self) -> bool {
        self.points.len() >= 2
    }
}

// ---------------------------------------------------------------------------
// Particles
// ---------------------------------------------------------------------------

/// Emitter shape = `ParticleSystemShape`.
///
/// Only affects particle birth offset and initial velocity direction; Phase 1 has no collision/trail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ParticleShape {
    /// All emitted from emitter origin, random direction.
    #[default]
    Point,
    /// Born inside sphere shell, direction along radial outward.
    Sphere,
    /// Born inside cube, random direction.
    Box,
    /// Cone (along emitter local **+Y**), `cone_angle_degrees` is half angle.
    Cone,
}

impl ReflectType for ParticleShape {
    fn field_type() -> FieldType {
        FieldType::U8
    }

    fn enum_variants() -> Option<&'static [&'static str]> {
        // Order consistent with declaration; these strings are values in `.scene.json`.
        Some(&["Point", "Sphere", "Box", "Cone"])
    }

    fn to_value(&self) -> Value {
        Value::U64(match self {
            Self::Point => 0,
            Self::Sphere => 1,
            Self::Box => 2,
            Self::Cone => 3,
        })
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        match value.as_u64()? {
            0 => Ok(Self::Point),
            1 => Ok(Self::Sphere),
            2 => Ok(Self::Box),
            3 => Ok(Self::Cone),
            other => Err(ReflectError::BadData(format!(
                "invalid ParticleShape discriminant {other}"
            ))),
        }
    }
}

/// World-space CPU particle emitter = `ParticleSystem` (Phase 1 subset).
///
/// Only describes authoring parameters; living particles exist in runtime component
/// `prism_engine::scene::systems::particles::ParticleEmitter`, does not enter
/// `.scene.json` / RSCN. Each particle takes once at birth
/// random value of `start_speed` / `start_size` / `lifetime`.
///
/// Color has two forms: when `color_stops` non-empty, samples multi-segment gradient uniformly by lifetime (one stop =
/// constant color); when empty falls back to two-color linear interpolation `start_color → end_color`. Size similarly:
/// `size_over_lifetime` is multiplier at birth / death (`[1, 0]` shrinks, `[0.5, 2]` grows),
/// multiplied by particle birth size.
///
/// Particles simulated in **world space** (emitter movement does not drag existing particles), initial velocity direction per emitter
/// current rotation conversion.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::ParticleSystem")]
#[serde(default)]
pub struct ParticleSystem {
    /// Whether emitting/advancing; `false` existing particles stop in place (not cleared).
    pub playing: bool,
    /// Emission count per second.
    pub emission_rate: f32,
    /// Extra one-shot emission count at start of each loop.
    pub burst: u32,
    /// Emission duration (seconds); `0` = emit continuously (ignores `looping`).
    pub duration: f32,
    /// Whether looping emission when `duration > 0`.
    pub looping: bool,
    /// Max particles alive simultaneously; `0` = built-in default (256).
    pub max_particles: u32,
    /// Lifetime random interval `[min, max]` (seconds).
    pub lifetime: [f32; 2],
    /// Initial velocity random interval (world units / second).
    pub start_speed: [f32; 2],
    /// Initial size random interval (world units).
    pub start_size: [f32; 2],
    /// Linear RGBA color at birth (including alpha).
    pub start_color: [f32; 4],
    /// Linear RGBA color at death; alpha usually 0 for fade out.
    pub end_color: [f32; 4],
    /// Multi-segment color gradient (linear RGBA); when non-empty overrides `start_color → end_color`.
    ///
    /// Stops distributed uniformly over lifetime (0th = birth, last = death), between adjacent points
    /// linearly interpolated; writing one point equals constant color. Empty = two-color interpolation (old behavior).
    /// Inspector list editor can directly add/remove/edit these `[f32; 4]`.
    pub color_stops: Vec<[f32; 4]>,
    /// Size multiplier at birth / death, multiplied by particle birth size (`[1, 1]` = unchanged).
    ///
    /// `[1, 0]` shrinks to disappear, `[0.5, 2]` gradually grows; non-finite treated as 1, result will not be negative.
    pub size_over_lifetime: [f32; 2],
    /// World-space velocity offset at birth / death (units / second), after linear interpolation over lifetime
    /// added to displacement (not written to particle velocity, so will not affect gravity / drag in reverse).
    ///
    /// `[[0, 0, 0], [0, 0, 0]]` = unchanged. Can be used for wind, updraft, lateral drift;
    /// Non-finite components treated as 0.
    pub velocity_over_lifetime: [[f32; 3]; 2],
    /// Constant acceleration (world units / second²), `[0, -9.81, 0]` is gravity.
    pub gravity: [f32; 3],
    /// Velocity drag (per-second decay ratio, `0` = no drag, `1` = velocity to zero within one second).
    pub drag: f32,
    /// Whether to push particles out of entity surfaces with `Collider` (#501). Default off, keeps old behavior.
    pub collision: bool,
    /// Collision restitution: `0` = stick to surface, `1` = full bounce. Out-of-range clamped to `[0, 1]`.
    pub collision_bounce: f32,
    /// Collision tangential friction: `0` = keep all tangential velocity, `1` = zero tangential velocity.
    pub collision_friction: f32,
    /// Emitter shape.
    pub shape: ParticleShape,
    /// `Sphere` / `Box` radius (world units).
    pub shape_radius: f32,
    /// `Cone` half angle (degrees, 0..90).
    pub cone_angle_degrees: f32,
}

impl Default for ParticleSystem {
    fn default() -> Self {
        Self {
            playing: true,
            emission_rate: 20.0,
            burst: 0,
            duration: 0.0,
            looping: true,
            max_particles: 0,
            lifetime: [0.8, 1.4],
            start_speed: [0.6, 1.6],
            start_size: [0.12, 0.28],
            start_color: [1.0, 0.85, 0.35, 1.0],
            end_color: [1.0, 0.25, 0.05, 0.0],
            color_stops: Vec::new(),
            size_over_lifetime: [1.0, 1.0],
            velocity_over_lifetime: [[0.0; 3]; 2],
            gravity: [0.0, 1.2, 0.0],
            drag: 0.6,
            collision: false,
            collision_bounce: 0.3,
            collision_friction: 0.2,
            shape: ParticleShape::Cone,
            shape_radius: 0.15,
            cone_angle_degrees: 20.0,
        }
    }
}

impl ParticleSystem {
    /// Max particles alive simultaneously (falls back to 256 when `max_particles == 0`).
    pub fn particle_cap(&self) -> usize {
        if self.max_particles == 0 {
            256
        } else {
            self.max_particles as usize
        }
    }

    /// Emission duration (seconds); `0` / non-finite means infinite.
    pub fn emission_duration(&self) -> Option<f32> {
        if self.duration.is_finite() && self.duration > 0.0 {
            Some(self.duration)
        } else {
            None
        }
    }

    /// Emission per second (invalid treated as 0, negative does not emit).
    pub fn rate(&self) -> f32 {
        if self.emission_rate.is_finite() && self.emission_rate > 0.0 {
            self.emission_rate
        } else {
            0.0
        }
    }

    /// Interval value (uses `fallback` when invalid / reversed, and sorts both ends).
    pub fn random_range(range: [f32; 2], t: f32, fallback: f32) -> f32 {
        let (lo, hi) = if range[0].is_finite() && range[1].is_finite() {
            if range[0] <= range[1] {
                (range[0], range[1])
            } else {
                (range[1], range[0])
            }
        } else {
            (fallback, fallback)
        };
        lo + (hi - lo) * t
    }
}

/// 2D emitter shape (`ParticleEmitter2D`).
///
/// Only affects particle birth offset in emitter local XY plane; initial velocity direction by
/// `direction` + `spread_degrees` determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ParticleShape2D {
    /// All emitted from emitter origin.
    #[default]
    Point,
    /// A line segment along local X axis (`shape_size[0]` is half length).
    Line,
    /// Local rectangle (`shape_size` is half width / half height).
    Box,
    /// Local disk (`shape_radius`).
    Circle,
}

impl ReflectType for ParticleShape2D {
    fn field_type() -> FieldType {
        FieldType::U8
    }

    fn enum_variants() -> Option<&'static [&'static str]> {
        Some(&["Point", "Line", "Box", "Circle"])
    }

    fn to_value(&self) -> Value {
        Value::U64(match self {
            Self::Point => 0,
            Self::Line => 1,
            Self::Box => 2,
            Self::Circle => 3,
        })
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        match value.as_u64()? {
            0 => Ok(Self::Point),
            1 => Ok(Self::Line),
            2 => Ok(Self::Box),
            3 => Ok(Self::Circle),
            other => Err(ReflectError::BadData(format!(
                "invalid ParticleShape2D discriminant {other}"
            ))),
        }
    }
}

/// 2D world-space particle emitter (#594).
///
/// Same idea as 3D [`ParticleSystem`]: only describes **authoring parameters**, living
/// particles exist in runtime component
/// `prism_engine::scene::systems::particles2d::ParticleEmitter2DState`,
/// not in `.scene.json` / RSCN. Particles integrated in **world XY plane** (emitter movement will not
/// drag already-flown particles), initial velocity direction per emitter current Z axis rotation.
///
/// Drawing via 2D sprite channel (`SpriteDrawItem`): each particle is a solid color quad,
/// sorted together with sprites by `sorting_layer → order_in_layer → z`, so splashes can be interleaved
/// between ship and water surface, without separate render pass.
///
/// Common rain / snow / splash do not need new shaders, just different parameters:
///
/// ```json
/// "prism_engine::scene::ParticleEmitter2D": {
///   "emission_rate": 240, "lifetime": [1.2, 1.8],
///   "start_speed": [9.0, 12.0], "start_size": [0.04, 0.06],
///   "direction": [0, -1], "spread_degrees": 6,
///   "shape": "Box", "shape_size": [12, 0.5],
///   "gravity": [0, -2.0], "drag": 0.0,
///   "start_color": [0.75, 0.85, 1.0, 0.65],
///   "end_color": [0.75, 0.85, 1.0, 0.25]
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::ParticleEmitter2D")]
#[serde(default)]
pub struct ParticleEmitter2D {
    /// Whether emitting/advancing; `false` existing particles stop in place (not cleared).
    pub playing: bool,
    /// Emission count per second.
    pub emission_rate: f32,
    /// Extra one-shot emission count at start of each emission loop.
    pub burst: u32,
    /// Emission duration (seconds); `0` = emit continuously (ignores `looping`).
    pub duration: f32,
    /// Whether looping emission when `duration > 0`.
    pub looping: bool,
    /// Max particles alive simultaneously; `0` = built-in default (256).
    pub max_particles: u32,
    /// Lifetime random interval `[min, max]` (seconds).
    pub lifetime: [f32; 2],
    /// Initial velocity random interval (world units / second).
    pub start_speed: [f32; 2],
    /// Initial side length random interval (world units; quad is square).
    pub start_size: [f32; 2],
    /// RGBA color at birth (including alpha).
    pub start_color: [f32; 4],
    /// RGBA color at death; alpha usually 0 for fade out.
    pub end_color: [f32; 4],
    /// Constant acceleration (world units / second²), `[0, -9.8]` is gravity.
    pub gravity: [f32; 2],
    /// Velocity drag (per-second decay ratio, `0` = no drag, `1` = velocity to zero within one second).
    pub drag: f32,
    /// Birth point shape.
    pub shape: ParticleShape2D,
    /// `Line` / `Box` half size (world units); `Line` only uses [0].
    pub shape_size: [f32; 2],
    /// `Circle` radius (world units).
    pub shape_radius: f32,
    /// Initial velocity base direction (local space, need not normalize); use `[0, -1]` for zero vector.
    pub direction: [f32; 2],
    /// Random half angle relative to `direction` (degrees, 0 = all along same direction).
    pub spread_degrees: f32,
    /// Layer sorted together with other 2D drawing items.
    pub sorting_layer: i32,
    /// Order within layer.
    pub order_in_layer: i32,
}

impl Default for ParticleEmitter2D {
    fn default() -> Self {
        Self {
            playing: true,
            emission_rate: 20.0,
            burst: 0,
            duration: 0.0,
            looping: true,
            max_particles: 0,
            lifetime: [0.6, 1.2],
            start_speed: [1.0, 2.5],
            start_size: [0.05, 0.12],
            start_color: [1.0, 0.92, 0.7, 1.0],
            end_color: [1.0, 0.55, 0.2, 0.0],
            gravity: [0.0, -3.0],
            drag: 0.4,
            shape: ParticleShape2D::Point,
            shape_size: [0.5, 0.5],
            shape_radius: 0.25,
            direction: [0.0, -1.0],
            spread_degrees: 15.0,
            sorting_layer: 0,
            order_in_layer: 0,
        }
    }
}

impl ParticleEmitter2D {
    /// Max particles alive simultaneously (falls back to 256 when `max_particles == 0`).
    pub fn particle_cap(&self) -> usize {
        if self.max_particles == 0 {
            256
        } else {
            self.max_particles as usize
        }
    }

    /// Emission duration (seconds); `0` / non-finite means infinite.
    pub fn emission_duration(&self) -> Option<f32> {
        if self.duration.is_finite() && self.duration > 0.0 {
            Some(self.duration)
        } else {
            None
        }
    }

    /// Emission per second (invalid treated as 0, negative does not emit).
    pub fn rate(&self) -> f32 {
        if self.emission_rate.is_finite() && self.emission_rate > 0.0 {
            self.emission_rate
        } else {
            0.0
        }
    }

    /// Normalized base direction; falls back to "down" when zero / non-finite.
    pub fn direction_or_down(&self) -> [f32; 2] {
        let length =
            (self.direction[0] * self.direction[0] + self.direction[1] * self.direction[1]).sqrt();
        if length.is_finite() && length > 1e-6 {
            [self.direction[0] / length, self.direction[1] / length]
        } else {
            [0.0, -1.0]
        }
    }
}

/// Motion preset for [`UiParticleEmitter`] (#1057).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum UiParticleKind {
    /// Fast falling streaks with a faint tail; wind tilts them.
    #[default]
    Rain,
    /// Slow drifting dots that sway sideways; no tail.
    Snow,
    /// Short-lived dots sprayed in every direction.
    Spark,
}

impl ReflectType for UiParticleKind {
    fn field_type() -> FieldType {
        FieldType::U8
    }

    fn enum_variants() -> Option<&'static [&'static str]> {
        Some(&["Rain", "Snow", "Spark"])
    }

    fn to_value(&self) -> Value {
        Value::U64(match self {
            Self::Rain => 0,
            Self::Snow => 1,
            Self::Spark => 2,
        })
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        match value.as_u64()? {
            0 => Ok(Self::Rain),
            1 => Ok(Self::Snow),
            2 => Ok(Self::Spark),
            other => Err(ReflectError::BadData(format!(
                "invalid UiParticleKind discriminant {other}"
            ))),
        }
    }
}

/// Screen-space particle emitter, drawn through the UI layer (#1057).
///
/// [`ParticleEmitter2D`] integrates in world XY and draws through the sprite
/// channel, so a full-screen UI panel (a fade-in black screen, a menu backdrop)
/// covers it. This emitter integrates in **pixel screen space** — origin at the
/// top-left, y down, the same space as `ComputedLayout::rect` — and emits plain
/// quads into the UI draw list, so particles can sit above UI panels.
///
/// Only the author parameters live here. The engine keeps the live particles in
/// a runtime state component and advances them every tick; a scene therefore
/// needs nothing but this component:
///
/// ```json
/// "prism_engine::scene::UiParticleEmitter": {
///   "kind": "Rain", "emission_rate": 420, "intensity": 0.8
/// }
/// ```
///
/// Distances and speeds are screen pixels, so the same values mean different
/// things on different resolutions; the defaults are tuned for 1080p.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::UiParticleEmitter")]
#[serde(default)]
pub struct UiParticleEmitter {
    /// `false` stops spawning new particles; live ones finish their life.
    pub playing: bool,
    /// Motion preset (see [`UiParticleKind`]).
    pub kind: UiParticleKind,
    /// Particles spawned per second at `intensity = 1`.
    pub emission_rate: f32,
    /// Downward acceleration (px/s²).
    pub gravity: f32,
    /// Rightward acceleration (px/s²); tilts rain.
    pub wind: f32,
    /// Initial vertical speed range (px/s).
    pub speed: [f32; 2],
    /// Trail length range (px); `0` gives round dots.
    pub length: [f32; 2],
    /// Quad thickness range (px); the trailing edge is rounded.
    pub width: [f32; 2],
    /// RGB color 0..=1 (sRGB authoring value).
    pub color: [f32; 3],
    /// Base alpha range 0..=1, further scaled by `intensity`.
    pub alpha: [f32; 2],
    /// Global 0..=1 multiplier for spawn rate and alpha; `0` fades out.
    pub intensity: f32,
    /// UI layer of the emitted quads (`UiQuad::layer`). Must be above the panel
    /// the particles should cover.
    pub layer: i32,
    /// Soft cap on live particles; `0` keeps the preset limit.
    pub max_particles: u32,
}

impl Default for UiParticleEmitter {
    fn default() -> Self {
        Self {
            playing: true,
            kind: UiParticleKind::Rain,
            emission_rate: 420.0,
            gravity: 240.0,
            wind: 90.0,
            speed: [900.0, 1500.0],
            length: [16.0, 42.0],
            width: [1.0, 2.4],
            color: [0.72, 0.82, 0.96],
            alpha: [0.16, 0.42],
            intensity: 1.0,
            layer: 1,
            max_particles: 0,
        }
    }
}

impl UiParticleEmitter {
    /// Spawn rate with non-finite / negative input treated as zero.
    pub fn rate(&self) -> f32 {
        if self.emission_rate.is_finite() && self.emission_rate > 0.0 {
            self.emission_rate
        } else {
            0.0
        }
    }

    /// Global intensity clamped to `0..=1`; non-finite input becomes zero.
    pub fn clamped_intensity(&self) -> f32 {
        if self.intensity.is_finite() {
            self.intensity.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

// ---------------------------------------------------------------------------
// Lighting
// ---------------------------------------------------------------------------

/// 2D light type (#611).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Light2DKind {
    /// Uniformly illuminates entire screen (night background color). `intensity` directly multiplies color.
    Ambient,
    /// Circular attenuation: attenuates by `(1 - d / radius)^2` within radius.
    #[default]
    Point,
    /// Directional light: no position / attenuation, no shadow on direction---2D sprites have no normals,
    /// can only multiply an overall color, used for "sunset dyes screen orange".
    Directional,
}

impl ReflectType for Light2DKind {
    fn field_type() -> FieldType {
        FieldType::U8
    }

    fn enum_variants() -> Option<&'static [&'static str]> {
        Some(&["Ambient", "Point", "Directional"])
    }

    fn to_value(&self) -> Value {
        Value::U64(match self {
            Self::Ambient => 0,
            Self::Point => 1,
            Self::Directional => 2,
        })
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        match value.as_u64()? {
            0 => Ok(Self::Ambient),
            1 => Ok(Self::Point),
            2 => Ok(Self::Directional),
            other => Err(ReflectError::BadData(format!(
                "invalid Light2DKind discriminant {other}"
            ))),
        }
    }
}

/// 2D light (#611): illuminates sprites that opened [`SpriteRenderer::receive_lights`].
///
/// Different unit system from 3D [`PointLight`] / [`DirectionalLight`]: 3D uses candela
/// / lux; here `intensity = 1.0` means "original color", convenient for authors to adjust by screen.
///
/// 2D Sprite has no normal, so point light only does radial attenuation (no Lambert), directional light multiplies
/// a color on entire sprite. Soft shadow / light cookie left for later (#611 Phase 2).
///
/// Usage (night lantern):
///
/// ```json
/// "prism_engine::scene::Light2D": {
///   "kind": "Point", "color": [1.0, 0.75, 0.45], "intensity": 1.2, "radius": 6.0
/// }
/// ```
///
/// Add another `kind = "Ambient"`, `color = [0.18, 0.2, 0.32]`, `intensity = 1.0`
/// as night background; without ambient light, sprites outside radius turn black (normal result of 2D lighting).
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::Light2D")]
#[serde(default)]
pub struct Light2D {
    /// Light type (default point light).
    pub kind: Light2DKind,
    /// Linear RGB color (proportion multiplied onto sprite when `intensity = 1`).
    pub color: [f32; 3],
    /// Intensity multiplier; `0` = light off, negative treated as 0.
    pub intensity: f32,
    /// Point light radius (world units); when `<= 0` / non-finite point light degenerates to "full screen",
    /// convenient to see effect during debugging first.
    pub radius: f32,
    /// Directional light irradiation direction (world XY, before normalization). 2D has no normal, this value only used for
    /// subsequent directional attenuation (Phase 2), currently does not affect result.
    pub direction: [f32; 2],
    /// Whether participates in lighting.
    pub enabled: bool,
}

impl Default for Light2D {
    fn default() -> Self {
        Self {
            kind: Light2DKind::Point,
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            radius: 5.0,
            direction: [0.0, -1.0],
            enabled: true,
        }
    }
}

impl Light2D {
    /// Whether to add brightness to `receive_lights` sprites (off / non-positive intensity → `false`).
    pub fn contributes(&self) -> bool {
        self.enabled
            && self.intensity.is_finite()
            && self.intensity > 0.0
            && self.color.iter().any(|channel| *channel > 0.0)
    }

    /// Effective intensity (non-finite / negative treated as 0).
    pub fn strength(&self) -> f32 {
        if self.intensity.is_finite() && self.intensity > 0.0 {
            self.intensity
        } else {
            0.0
        }
    }

    /// Effective radius; `0` means "no attenuation / full screen".
    pub fn falloff_radius(&self) -> f32 {
        if self.radius.is_finite() && self.radius > 0.0 {
            self.radius
        } else {
            0.0
        }
    }
}

/// Day-night clock (#983): one component equals one day, no need for each project to write timer code.
///
/// System advances `hours` every frame and converts to **ambient light** written back to same entity
/// [`Light2D`] (`kind = Ambient`). Therefore "night lantern" only needs two things:
///
/// ```json
/// "prism_engine::scene::TimeOfDay": { "hours": 8.0, "day_length_seconds": 600.0 },
/// "prism_engine::scene::Light2D": { "kind": "Ambient" }
/// ```
///
/// Point light (lantern, campfire) does not move itself, relies on ambient light dimming to naturally show contrast.
/// Read current time via [`TimeOfDay::clock_text`], judge phase via [`TimeOfDay::phase`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::TimeOfDay")]
#[serde(default)]
pub struct TimeOfDay {
    /// Current time, `0.0..24.0` (fractional part is minutes).
    pub hours: f32,
    /// How many seconds per game day; `<= 0` means time frozen (read-only `hours`).
    pub day_length_seconds: f32,
    /// Pause advancement (`hours` can still be directly written by script).
    pub paused: bool,
    /// Sunrise hour, dawn/dusk transition centered on it.
    pub sunrise_hour: f32,
    /// Sunset hour, dawn/dusk transition centered on it.
    pub sunset_hour: f32,
    /// Half-width of dawn/dusk transition (hours): gradient between `sunrise ± twilight_hours`.
    pub twilight_hours: f32,
    /// Daytime ambient light color (linear RGB).
    pub day_color: [f32; 3],
    /// Daytime ambient light intensity.
    pub day_intensity: f32,
    /// Nighttime ambient light color (linear RGB).
    pub night_color: [f32; 3],
    /// Nighttime ambient light intensity.
    pub night_intensity: f32,
}

/// Four phases of a day (for game logic / HUD, does not affect ambient light calculation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeOfDayPhase {
    /// Around sunrise (including dawn/dusk transition).
    Dawn,
    /// Daytime.
    Day,
    /// Around sunset (including dawn/dusk transition).
    Dusk,
    /// Nighttime.
    Night,
}

impl Default for TimeOfDay {
    fn default() -> Self {
        Self {
            hours: 8.0,
            day_length_seconds: 600.0,
            paused: false,
            sunrise_hour: 6.0,
            sunset_hour: 18.0,
            twilight_hours: 1.5,
            day_color: [1.0, 1.0, 1.0],
            day_intensity: 1.0,
            night_color: [0.18, 0.2, 0.34],
            night_intensity: 1.0,
        }
    }
}

impl TimeOfDay {
    /// Advance time; `dt` is seconds. Does not move when paused / `day_length_seconds <= 0`.
    pub fn advance(&mut self, dt: f32) {
        if self.paused || !(dt.is_finite() && dt > 0.0) {
            return;
        }
        let length = self.day_length_seconds;
        if !(length.is_finite() && length > 0.0) {
            return;
        }
        let hours = self.normalized_hours() + dt / length * 24.0;
        self.hours = hours.rem_euclid(24.0);
    }

    /// `hours` normalized to `0..24` (author wrote `-3` or `30` also usable).
    pub fn normalized_hours(&self) -> f32 {
        let hours = if self.hours.is_finite() {
            self.hours
        } else {
            0.0
        };
        hours.rem_euclid(24.0)
    }

    /// `HH:MM`, for HUD direct display.
    pub fn clock_text(&self) -> String {
        let hours = self.normalized_hours();
        let whole = hours.floor();
        let minutes = ((hours - whole) * 60.0).floor();
        format!("{:02}:{:02}", whole as u32 % 24, (minutes as u32).min(59))
    }

    /// Current phase. Dawn/dusk transition interval counts as `Dawn` / `Dusk**.
    pub fn phase(&self) -> TimeOfDayPhase {
        let h = self.normalized_hours();
        let twilight = self.twilight_hours.max(0.0);
        let sunrise = self.sunrise_hour.rem_euclid(24.0);
        let sunset = self.sunset_hour.rem_euclid(24.0);
        if within(h, sunrise, twilight) {
            TimeOfDayPhase::Dawn
        } else if within(h, sunset, twilight) {
            TimeOfDayPhase::Dusk
        } else if day_side(h, sunrise, sunset) {
            TimeOfDayPhase::Day
        } else {
            TimeOfDayPhase::Night
        }
    }

    /// Current ambient light: `(color, intensity)`. Linearly interpolated between dawn/dusk, otherwise takes day/night values.
    pub fn ambient(&self) -> ([f32; 3], f32) {
        let h = self.normalized_hours();
        let twilight = self.twilight_hours.max(0.0);
        let sunrise = self.sunrise_hour.rem_euclid(24.0);
        let sunset = self.sunset_hour.rem_euclid(24.0);
        // 0 = night, 1 = day; sunrise / sunset each have half transition.
        let daylight = if twilight <= 0.0 {
            if day_side(h, sunrise, sunset) {
                1.0
            } else {
                0.0
            }
        } else if within(h, sunrise, twilight) {
            smoothstep(0.0, 1.0, (h - (sunrise - twilight)) / (2.0 * twilight))
        } else if within(h, sunset, twilight) {
            smoothstep(0.0, 1.0, ((sunset + twilight) - h) / (2.0 * twilight))
        } else if day_side(h, sunrise, sunset) {
            1.0
        } else {
            0.0
        };
        let mix = |day: f32, night: f32| night + (day - night) * daylight;
        (
            [
                mix(self.day_color[0], self.night_color[0]),
                mix(self.day_color[1], self.night_color[1]),
                mix(self.day_color[2], self.night_color[2]),
            ],
            mix(self.day_intensity, self.night_intensity),
        )
    }
}

/// Whether `hour` falls within `center ± width` (counts across zero point).
fn within(hour: f32, center: f32, width: f32) -> bool {
    let delta = (hour - center).rem_euclid(24.0);
    delta <= width || delta >= 24.0 - width
}

/// Whether `hour` is on daytime side between sunrise and sunset (supports sunrise-later-than-sunset multi-night-day configuration).
fn day_side(hour: f32, sunrise: f32, sunset: f32) -> bool {
    if sunrise <= sunset {
        hour >= sunrise && hour < sunset
    } else {
        hour >= sunrise || hour < sunset
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if (edge1 - edge0).abs() <= f32::EPSILON {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Directional (infinite) light.
///
/// Orientation is stored as XYZ Euler angles (degrees) so it round-trips
/// cleanly through `scene_state.json`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::DirectionalLight")]
#[serde(default)]
pub struct DirectionalLight {
    /// XYZ Euler angles (degrees): pitch (X), yaw (Y), roll (Z).
    pub euler_xyz: glam::Vec3,
    /// Linear RGB colour, typically `[0, 1]`.
    pub color: glam::Vec3,
    /// Illuminance in **lux** (physical unit).
    pub intensity: f32,
    /// IBL ambient factor.
    pub ambient: f32,
}

impl Default for DirectionalLight {
    fn default() -> Self {
        Self {
            euler_xyz: glam::Vec3::new(45.0, -45.0, 0.0),
            color: glam::Vec3::ONE,
            intensity: 100_000.0,
            ambient: 1.0,
        }
    }
}

/// Point light.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::PointLight")]
#[serde(default)]
pub struct PointLight {
    /// Linear RGB colour.
    pub color: glam::Vec3,
    /// Luminous intensity in **candela**.
    pub intensity: f32,
    /// Attenuation radius (world units).
    pub range: f32,
}

impl Default for PointLight {
    fn default() -> Self {
        Self {
            color: glam::Vec3::ONE,
            intensity: 100.0,
            range: 12.0,
        }
    }
}

/// Spot light.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::SpotLight")]
#[serde(default)]
pub struct SpotLight {
    pub color: glam::Vec3,
    pub intensity: f32,
    pub range: f32,
    /// Inner cone half-angle (radians).
    pub inner_cone_angle: f32,
    /// Outer cone half-angle (radians).
    pub outer_cone_angle: f32,
}

impl Default for SpotLight {
    fn default() -> Self {
        Self {
            color: glam::Vec3::ONE,
            intensity: 100.0,
            range: 20.0,
            inner_cone_angle: 0.436, // ~25°
            outer_cone_angle: 0.873, // ~50°
        }
    }
}

/// Unified light component.
///
/// Only one `Light` component, distinguished by [`LightType`] for directional / point / spot light; light source
/// **direction determined by entity transform** (is `light.transform.forward`), position determined by
/// `LocalToWorld**. This component is that shape.
///
/// Old [`DirectionalLight`] / [`PointLight`] / [`SpotLight`] still usable (renderer receives both,
/// both collected, see `prism-engine::scene::systems::lights`), so old scenes do not need migration;
/// new scenes recommended to use only this component.
///
/// Units consistent with old components, use **physical quantities** (directional light lux / point/spot candela), not
/// default normalized `intensity = 1`: renderer internally converts by `LUX_TO_RADIANCE_SCALE`, changing
/// `intensity` feel same as old component.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::Light")]
#[serde(default)]
pub struct Light {
    /// `Behaviour.enabled`: renderer skips this light when `false`.
    pub enabled: bool,
    /// Light type = `Light.type**.
    pub light_type: LightType,
    /// Linear RGB color = `Light.color**.
    pub color: glam::Vec3,
    /// Intensity (directional light lux / point & spot candela) = `Light.intensity**.
    pub intensity: f32,
    /// Attenuation radius (world units) = `Light.range** (directional light ignores).
    pub range: f32,
    /// Spot **full cone angle** (degrees) = `Light.spotAngle**.
    pub spot_angle: f32,
    /// Inner cone **full cone angle** (degrees) = `Light.innerSpotAngle**; less than `spot_angle**.
    pub inner_spot_angle: f32,
    /// Shadow mode = `Light.shadows**.
    pub shadows: LightShadows,
    /// Shadow strength = `Light.shadowStrength` (`0..=1**).
    pub shadow_strength: f32,
    /// Color temperature (kelvin) = `Light.colorTemperature**.
    pub color_temperature: f32,
    /// Whether color temperature participates in light color = `Light.useColorTemperature**.
    pub use_color_temperature: bool,
    /// Render mode = `Light.renderMode**.
    pub render_mode: LightRenderMode,
}

impl Default for Light {
    fn default() -> Self {
        Self {
            enabled: true,
            // Engine `new Light()` defaults to point light source.
            light_type: LightType::Point,
            color: glam::Vec3::ONE,
            intensity: 100.0,
            range: 10.0,
            spot_angle: 30.0,
            inner_spot_angle: 21.8,
            shadows: LightShadows::None,
            shadow_strength: 1.0,
            color_temperature: 6570.0,
            use_color_temperature: false,
            render_mode: LightRenderMode::Auto,
        }
    }
}

impl Light {
    /// Final light color = `color** (× color temperature filter).
    ///
    /// Corresponds to engine `ExtractColorTemperature` + `ApplyColorTemperature**:
    /// Color temperature only multiplied when `use_color_temperature` is true, and not normalized.
    pub fn effective_color(&self) -> glam::Vec3 {
        if self.use_color_temperature {
            self.color * correlated_color_temperature_to_rgb(self.color_temperature)
        } else {
            self.color
        }
    }

    /// Spot inner / outer **half cone angle** (radians), for GPU and shader use.
    ///
    /// Engine `spotAngle` / `innerSpotAngle` are both full cone angles (degrees); here divide by 2 to convert to
    /// half angle, and clamp inner angle to `[0, outer angle]`, avoiding invalid config producing negative cosine interval.
    pub fn cone_half_angles_radians(&self) -> (f32, f32) {
        let outer = (self.spot_angle.max(0.0) * 0.5).to_radians();
        let inner = (self.inner_spot_angle.max(0.0) * 0.5).to_radians();
        (inner.min(outer), outer)
    }

    /// Whether casts shadow (enabled + `shadows != None**).
    pub fn is_shadow_caster(&self) -> bool {
        self.enabled && self.shadows.casts_shadows()
    }
}

/// Scene environment lighting (IBL) declarative component.
///
/// Engine uses "scene declarative lighting": renderer builds IBL pre-convolution at scene load only when scene explicitly carries this component and `env_map` is `Some**,
/// (CPU convolution, approx ms). Splash screens etc with no 3D lighting needs do not write this component (or `env_map = None`) → renderer starts
/// with cheap neutral empty IBL, starts in seconds, does not load 3D environment map.

#[derive(Debug, Clone, Default, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::EnvironmentLighting")]
#[serde(default)]
pub struct EnvironmentLighting {
    /// Environment map (equirectangular HDR) path relative to scene file. `None** or missing = does not build IBL.**
    pub env_map: Option<String>,
}

/// Scene global illumination (GI probe volume) declarative component.
///
/// Only when `enabled = true` and baked volume data exists does renderer upload real GI probe
/// volume; otherwise keeps free synthetic sky placeholder (only makes set5 descriptor valid). No 3D lighting
/// demand scenes (like splash screen) do not write this component.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::GiConfig")]
#[serde(default)]
pub struct GiConfig {
    /// Whether to enable real GI (replacing synthetic sky placeholder).
    pub enabled: bool,
    /// Baked GI volume data path relative to scene file (optional).
    pub volume: Option<String>,
}

/// Operating mode of a scene [`ProbeVolume`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ProbeVolumeMode {
    #[default]
    Off,
    Baked,
    Dynamic,
}

impl ReflectType for ProbeVolumeMode {
    fn field_type() -> FieldType {
        FieldType::U8
    }

    fn enum_variants() -> Option<&'static [&'static str]> {
        Some(&["Off", "Baked", "Dynamic"])
    }

    fn to_value(&self) -> Value {
        Value::U64(match self {
            Self::Off => 0,
            Self::Baked => 1,
            Self::Dynamic => 2,
        })
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        match value.as_u64()? {
            0 => Ok(Self::Off),
            1 => Ok(Self::Baked),
            2 => Ok(Self::Dynamic),
            other => Err(ReflectError::BadData(format!(
                "invalid ProbeVolumeMode discriminant {other}"
            ))),
        }
    }
}

/// Scene GI probe-volume component editable in `.scene.json` and serialized via RSCN v4 (#1164).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::ProbeVolume")]
#[serde(default)]
pub struct ProbeVolume {
    pub mode: ProbeVolumeMode,
    pub origin: [f32; 3],
    pub spacing: [f32; 3],
    pub probe_counts: [u32; 3],
    pub rays_per_probe: u32,
    pub hysteresis: f32,
    pub relocation: bool,
    pub classification: bool,
    pub scrolling: bool,
    pub variability: bool,
    pub baked_volume: Option<String>,
}

impl Default for ProbeVolume {
    fn default() -> Self {
        Self {
            mode: ProbeVolumeMode::Off,
            origin: [0.0, 0.0, 0.0],
            spacing: [2.0, 2.0, 2.0],
            probe_counts: [16, 8, 16],
            rays_per_probe: 128,
            hysteresis: 0.97,
            relocation: true,
            classification: true,
            scrolling: false,
            variability: false,
            baked_volume: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Camera
// ---------------------------------------------------------------------------

/// Perspective camera parameters (data component).
///
/// Holds the editor-editable projection + exposure fields. The runtime view
/// matrix is computed each frame by [`super::systems::camera`] from this
/// component plus a sibling [`FlyCameraController`] (or future controller)
/// and the entity's [`LocalToWorld`].
///
/// `aspect` is a runtime cache written by the app on resize; it is exposed in
/// Inspector treats as read-only. Enable flag controls whether renderer picks up
/// this camera.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::Camera")]
#[serde(default)]
pub struct Camera {
    pub fov_y_degrees: f32,
    pub near: f32,
    pub far: f32,
    /// Exposure multiplier applied to the final HDR color before tonemapping.
    /// Default 1.0 = no scaling; range [0, 5] via inspector slider.
    pub exposure: f32,
    /// Current aspect ratio (width / height). Written by the app on resize /
    /// orientation change; the projection matrix is derived from it each frame.
    #[reflect(read_only)]
    pub aspect: f32,
    /// When `false` the camera is skipped during scene collection (the
    /// renderer falls back to the next available camera).
    pub enabled: bool,
    /// Clear mode = `Camera.clearFlags` ([`CameraClearFlags`]**).
    pub clear_flags: CameraClearFlags,
    /// Solid color background = `Camera.backgroundColor` (linear RGBA, takes effect when `SolidColor**)**).
    pub background_color: [f32; 4],
    /// Orthographic projection = `Camera.orthographic**.
    pub orthographic: bool,
    /// Orthographic viewport half height (world units) = `Camera.orthographicSize**.
    pub orthographic_size: f32,
    /// Render order = `Camera.depth`; larger value drawn later, `main_camera` takes maximum.
    pub depth: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 1000.0,
            exposure: 1.0,
            aspect: 16.0 / 9.0,
            enabled: true,
            clear_flags: CameraClearFlags::Skybox,
            background_color: [0.5, 0.5, 0.5, 1.0],
            orthographic: false,
            orthographic_size: 5.0,
            depth: 0.0,
        }
    }
}

impl Camera {
    /// Projection matrix (column-major) = `Camera.projectionMatrix**.
    ///
    /// Right-handed, camera looks toward −Z, Vulkan y-flip (`p[1][1] = -inv_tan**),
    /// depth range `[0, 1]`. `aspect` provided by caller (runtime from framebuffer,**
    /// editor from panel), degenerates to 1.0 when non-finite or non-positive.
    ///
    /// When `orthographic` true uses [`Camera::orthographic_size`] (half height, world units**)**).
    pub fn projection_matrix(&self, aspect: f32) -> glam::Mat4 {
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            1.0
        };
        let near = self.near;
        let far = self.far;
        if self.orthographic {
            let half_h = self.orthographic_size.max(1e-4);
            let half_w = half_h * aspect;
            glam::Mat4::from_cols(
                glam::vec4(1.0 / half_w, 0.0, 0.0, 0.0),
                glam::vec4(0.0, -1.0 / half_h, 0.0, 0.0),
                glam::vec4(0.0, 0.0, 1.0 / (near - far), 0.0),
                glam::vec4(0.0, 0.0, near / (near - far), 1.0),
            )
        } else {
            let inv_tan = 1.0 / (self.fov_y_degrees.to_radians() * 0.5).tan();
            glam::Mat4::from_cols(
                glam::vec4(inv_tan / aspect, 0.0, 0.0, 0.0),
                glam::vec4(0.0, -inv_tan, 0.0, 0.0),
                glam::vec4(0.0, 0.0, far / (near - far), -1.0),
                glam::vec4(0.0, 0.0, near * far / (near - far), 0.0),
            )
        }
    }

    /// World → camera space = `Camera.worldToCameraMatrix` (i.e. view matrix**).
    pub fn world_to_camera_matrix(camera_to_world: glam::Mat4) -> glam::Mat4 {
        camera_to_world.inverse()
    }

    /// Camera → world space = `Camera.cameraToWorldMatrix**.
    pub fn camera_to_world_matrix(world_to_camera: glam::Mat4) -> glam::Mat4 {
        world_to_camera.inverse()
    }
}

/// 2D virtual camera controller component (#1160).
///
/// Used with [`Camera`] for 2D game scenes, provides:
/// 1. Target follow and smooth damping
/// 2. Deadzone rectangular window
/// 3. Level boundary confiner
/// 4. Pixel grid snapping anti-jitter
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::Camera2D")]
#[serde(default)]
pub struct Camera2D {
    /// Toggle state.
    pub enabled: bool,
    /// Follow target entity ID (u64::MAX or 0 means no target follow).
    pub target_entity: u64,
    /// Target relative offset [x, y] (world coordinate units).
    pub offset: [f32; 2],
    /// Follow smooth damping factor (0.0 means teleport tight follow, larger means smoother, e.g. 5.0 ~ 10.0).
    pub damping: f32,
    /// Deadzone half width/height [half_width, half_height]. Camera stays still when target moves within this rectangular window.
    pub deadzone: [f32; 2],
    /// Whether to enable level boundary constraint (Confiner).
    pub confiner_enabled: bool,
    /// Level world boundary rectangle [min_x, min_y, max_x, max_y].
    pub confiner_bounds: [f32; 4],
    /// Pixel snap switch (anti-jitter for pixel art games, rounds final world coordinates to pixel grid).
    pub pixel_snapping: bool,
    /// Pixels per world unit (Pixels Per Unit, default 16.0).
    pub pixels_per_unit: f32,
}

impl Default for Camera2D {
    fn default() -> Self {
        Self {
            enabled: true,
            target_entity: u64::MAX,
            offset: [0.0, 0.0],
            damping: 5.0,
            deadzone: [0.0, 0.0],
            confiner_enabled: false,
            confiner_bounds: [-1000.0, -1000.0, 1000.0, 1000.0],
            pixel_snapping: false,
            pixels_per_unit: 16.0,
        }
    }
}

/// 2D virtual camera follow settings.
///
/// The camera keeps its current world-space Z and follows the entity whose
/// [`Name`] matches [`Self::target_name`]. Follow and confiner coordinates are
/// world-space, including when the camera is parented. `damping` is a per-axis
/// convergence rate in inverse seconds; zero snaps immediately. `deadzone`
/// is the half-size around the camera that the target may occupy before the
/// camera starts moving. `confiner` is an optional world-space rectangle
/// `[min_x, min_y, max_x, max_y]`; it keeps the orthographic view inside the
/// rectangle and centers the camera when the room is smaller than the view.
/// Confiner bounds take priority over pixel snapping when no grid point fits.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::VirtualCamera2D")]
#[serde(default)]
pub struct VirtualCamera2D {
    /// Stable scene name used to resolve the follow target at runtime.
    pub target_name: String,
    /// Exponential follow rates for X and Y. Zero means immediate follow.
    pub damping: [f32; 2],
    /// Target deadzone half-size in world units.
    pub deadzone: [f32; 2],
    /// Optional world-space confiner rectangle.
    pub confiner: Option<[f32; 4]>,
    /// Offset applied to the resolved target before deadzone and damping.
    pub follow_offset: [f32; 2],
    /// Snap the camera position to the pixel grid after damping and confining.
    pub pixel_snap: bool,
    /// Pixels per world unit used by pixel snapping.
    pub pixels_per_unit: f32,
}

impl Default for VirtualCamera2D {
    fn default() -> Self {
        Self {
            target_name: String::new(),
            damping: [0.0; 2],
            deadzone: [0.0; 2],
            confiner: None,
            follow_offset: [0.0; 2],
            pixel_snap: false,
            pixels_per_unit: 1.0,
        }
    }
}

/// Local post-processing volume (#502 Phase 2).
///
/// Attached to scene entity, uses axis-aligned box (`size` = half size, meters) to outline an area; main camera enters
/// box, box parameters mixed from low to high `priority` onto this frame's [`PostGrade`].
/// When `global = true`, ignores box and distance, always participates in mixing.
///
/// Only reads entity world transform **translation + axis length** (scale), rotation does not participate --- consistent with terrain heightfield
/// convention. `blend_distance` is fade-out distance outside box edge (meters); 0 means only effective inside
/// box, hard cut at boundary.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::PostProcessVolume")]
#[serde(default)]
pub struct PostProcessVolume {
    /// When turned off, entire volume does not participate in mixing (authoring-time switch).
    pub enabled: bool,
    /// Priority: larger value applied last, usually overrides smaller value.
    pub priority: f32,
    /// Ignores `size` / `blend_distance`, always effective.
    pub global: bool,
    /// Box half size (meters), scaled by entity scale; rotation ignored.
    pub size: [f32; 3],
    /// Fade-out distance outside box edge (meters); `0` = hard cut inside box.
    pub blend_distance: f32,
    /// Display-space contrast (`1.0` = unchanged).
    pub contrast: f32,
    /// Display-space saturation (`0.0` = grayscale).
    pub saturation: f32,
    /// Vignette strength (`0.0` = off).
    pub vignette: f32,
    /// Whether to enable HDR bloom.
    pub bloom: bool,
    /// Bloom additive strength.
    pub bloom_intensity: f32,
    /// Bloom brightness threshold.
    pub bloom_threshold: f32,
    /// Lens chromatic aberration strength.
    pub chromatic_aberration: f32,
    /// Film grain strength.
    pub film_grain: f32,
    /// White balance: warm / cool (`-1..=1`, `0` = neutral).
    pub temperature: f32,
    /// White balance: magenta / green (`-1..=1`, `0` = neutral).
    pub tint: f32,
    /// Depth of field focus distance (meters, #502 Phase 2).
    pub dof_focus_distance: f32,
    /// Depth of field strength (`0.0` = off, `1.0` = strongest).
    pub dof_aperture: f32,
    /// Depth of field max blur radius (pixels, 0-32).
    pub dof_max_blur: f32,
    /// Camera motion blur strength (`0.0` = off, `1.0` = strongest).
    pub motion_blur: f32,
    /// Motion blur max ghosting length (pixels, 0-64).
    pub motion_blur_max_pixels: f32,
}

impl Default for PostProcessVolume {
    fn default() -> Self {
        Self {
            enabled: true,
            priority: 0.0,
            global: false,
            size: [10.0, 5.0, 10.0],
            blend_distance: 5.0,
            contrast: 1.0,
            saturation: 1.0,
            vignette: 0.0,
            bloom: false,
            bloom_intensity: 0.15,
            bloom_threshold: 1.0,
            chromatic_aberration: 0.0,
            film_grain: 0.0,
            temperature: 0.0,
            tint: 0.0,
            dof_focus_distance: 1.0,
            dof_aperture: 0.0,
            dof_max_blur: 8.0,
            motion_blur: 0.0,
            motion_blur_max_pixels: 16.0,
        }
    }
}

/// Free-fly camera input controller (data component).
///
/// Splits the runtime/input-owned fields off the old `FlyCamera` enum variant:
/// `yaw`/`pitch` are written each frame by [`super::systems::camera`] from
/// input, while `move_speed`/`look_sensitivity` are editor-editable. The
/// camera position lives on the sibling [`LocalTransform`] (and its derived
/// [`LocalToWorld`]), so this component carries no position of its own.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::FlyCameraController")]
#[serde(default)]
pub struct FlyCameraController {
    /// Yaw around +Y (rad). 0 = looking down -Z (matches `FlyCamera`).
    pub yaw: f32,
    /// Pitch above/below the horizon (rad). 0 = horizontal.
    pub pitch: f32,
    /// Base translation speed (world units / second) at boost = 1.
    pub move_speed: f32,
    /// Mouse look sensitivity (rad per pixel).
    pub look_sensitivity: f32,
}

impl Default for FlyCameraController {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: 0.0,
            move_speed: 5.0,
            look_sensitivity: 0.005,
        }
    }
}

// ---------------------------------------------------------------------------
// Prefab instances (editor metadata)
// ---------------------------------------------------------------------------

/// **Editor metadata**: which prefab this entity (subtree root) was instantiated from.
///
/// Engine runtime has no behavior for it (no system queries it), but it is a normal scene component,
/// therefore cooks/loads with scene --- so editor can do "reverse link / restore / apply to prefab /
/// sync same-origin instances", without needing extra sidecar file.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::PrefabInstance")]
#[serde(default)]
pub struct PrefabInstance {
    /// Prefab path (**relative to project root**, `/` separated), e.g. `assets/prefabs/door.prefab.json`.
    pub prefab: String,
}

// ---------------------------------------------------------------------------
// Identification
// ---------------------------------------------------------------------------

/// Human-readable entity name (data component).
///
/// Optional: entities without a `Name` are displayed in the inspector by their
/// raw id. Populated by the scene loader from the cooked `.rscn` name field;
/// editable from the inspector at runtime (not persisted to the scene file).
#[derive(Debug, Clone, Default, Reflect)]
#[reflect(name = "prism_engine::scene::Name")]
pub struct Name(pub String);

// ---------------------------------------------------------------------------
// Skybox
// ---------------------------------------------------------------------------

/// Skybox / environment map component.
///
/// The entity with this component owns the environment map for the scene:
/// its HDR drives both image-based lighting (IBL) and the background sky.
/// There should be at most one enabled skybox entity per scene; the first
/// enabled one wins.
///
/// The map is rendered as an infinitely distant background; the component's
/// rotation/intensity are artist-facing controls rather than a world
/// transform.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::Skybox")]
#[serde(default)]
pub struct Skybox {
    /// Optional stable resource id resolved by the cooker (legacy path).
    pub env_asset: SceneAssetId,
    /// Authoring HDR path, relative to the scene file (or `assets/...`).
    pub hdr_path: String,
    /// When `false` the environment is ignored: no IBL and no sky draw.
    pub enabled: bool,
    /// Background yaw rotation in degrees.
    pub rotation_degrees: f32,
    /// Background brightness multiplier (1 = HDR as-is).
    pub intensity: f32,
}

impl Default for Skybox {
    fn default() -> Self {
        Self {
            env_asset: SceneAssetId::from_raw(0),
            hdr_path: String::new(),
            enabled: true,
            rotation_degrees: 0.0,
            intensity: 1.0,
        }
    }
}

/// World-space XYZ axis gizmo owned by an entity.
///
/// Handle display mode (translate / rotate / scale).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GizmoMode {
    #[default]
    Translate,
    Rotate,
    Scale,
}

impl prism_reflect::ReflectType for GizmoMode {
    fn field_type() -> prism_reflect::FieldType {
        prism_reflect::FieldType::U8
    }

    fn enum_variants() -> Option<&'static [&'static str]> {
        Some(&["Translate", "Rotate", "Scale"])
    }

    fn to_value(&self) -> prism_reflect::Value {
        prism_reflect::Value::U64(match self {
            Self::Translate => 0,
            Self::Rotate => 1,
            Self::Scale => 2,
        })
    }

    fn from_value(value: prism_reflect::Value) -> Result<Self, prism_reflect::ReflectError> {
        match value.as_u64()? {
            0 => Ok(Self::Translate),
            1 => Ok(Self::Rotate),
            2 => Ok(Self::Scale),
            other => Err(prism_reflect::ReflectError::BadData(format!(
                "invalid GizmoMode discriminant {other}"
            ))),
        }
    }
}

/// The gizmo uses the entity's `LocalToWorld` (position/rotation/scale)
/// multiplied by [`Gizmo::scale`], so it can be placed and oriented like any
/// other scene object. No `Gizmo` component means no gizmo is drawn.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::Gizmo")]
#[serde(default)]
pub struct Gizmo {
    pub enabled: bool,
    /// Additional uniform scale applied on top of the entity transform.
    pub scale: f32,
    /// Show handle mode (translate arrow, rotate ring, scale handle).
    pub mode: GizmoMode,
}

impl Default for Gizmo {
    fn default() -> Self {
        Self {
            enabled: true,
            scale: 1.0,
            mode: GizmoMode::Translate,
        }
    }
}

// ---------------------------------------------------------------------------
// Physics
// ---------------------------------------------------------------------------

/// Collider shape = `ColliderShape`.
///
/// Dimensions are all in **local space** (multiplied by entity scale); Phase 1 only does shape/raycast query,
/// no real rigidbody integration yet (see #522 follow-up steps).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ColliderShape {
    /// Axis-aligned box: `half_extents` is half size along three axes.
    #[default]
    Box,
    /// Sphere: `radius` is radius.
    Sphere,
    /// Capsule: along local **+Y**, `radius` + `half_height` (cylinder segment half height).
    Capsule,
}

impl ReflectType for ColliderShape {
    fn field_type() -> FieldType {
        FieldType::U8
    }

    fn enum_variants() -> Option<&'static [&'static str]> {
        Some(&["Box", "Sphere", "Capsule"])
    }

    fn to_value(&self) -> Value {
        Value::U64(match self {
            Self::Box => 0,
            Self::Sphere => 1,
            Self::Capsule => 2,
        })
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        match value.as_u64()? {
            0 => Ok(Self::Box),
            1 => Ok(Self::Sphere),
            2 => Ok(Self::Capsule),
            other => Err(ReflectError::BadData(format!(
                "invalid ColliderShape discriminant {other}"
            ))),
        }
    }
}

/// Rigidbody type = `BodyType**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BodyType {
    /// Stationary, only participates in collision/query (floor, wall).
    #[default]
    Static,
    /// Moved by animation/code, pushes dynamic objects, not pushed itself.
    Kinematic,
    /// Affected by gravity and collision.
    Dynamic,
}

impl ReflectType for BodyType {
    fn field_type() -> FieldType {
        FieldType::U8
    }

    fn enum_variants() -> Option<&'static [&'static str]> {
        Some(&["Static", "Kinematic", "Dynamic"])
    }

    fn to_value(&self) -> Value {
        Value::U64(match self {
            Self::Static => 0,
            Self::Kinematic => 1,
            Self::Dynamic => 2,
        })
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        match value.as_u64()? {
            0 => Ok(Self::Static),
            1 => Ok(Self::Kinematic),
            2 => Ok(Self::Dynamic),
            other => Err(ReflectError::BadData(format!(
                "invalid BodyType discriminant {other}"
            ))),
        }
    }
}

/// Collider = `Collider`.
///
/// Shape determined by [`ColliderShape`], specific size uses the following three fields (independent, used per shape):
/// Box uses `half_extents`, sphere uses `radius`, capsule uses `radius` + `half_height`.
/// All sizes are in **local space**, world space size multiplied by entity scale.
///
/// `#522` implemented "authorable + raycast + rapier rigidbody integration": scene/editor can add and modify,
/// engine `physics::raycast` intersects by true shape; trigger callback and collision mesh still pending wiring.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::Collider")]
#[serde(default)]
pub struct Collider {
    pub shape: ColliderShape,
    /// Offset of shape center relative to entity origin (local space).
    pub center: [f32; 3],
    /// Box three half sizes.
    pub half_extents: [f32; 3],
    /// Sphere / capsule radius.
    pub radius: f32,
    /// Capsule cylinder segment half height (excluding both end hemispheres).
    pub half_height: f32,
    /// Trigger: participates in query but **does not block** rigidbody (rapier sensor event not yet wired to callback).
    pub is_trigger: bool,
    pub friction: f32,
    pub restitution: f32,
    /// Query layer mask bits; `raycast` only hits colliders with mask AND non-zero.
    pub layer: u32,
}

impl Default for Collider {
    fn default() -> Self {
        Self {
            shape: ColliderShape::Box,
            center: [0.0, 0.0, 0.0],
            half_extents: [0.5, 0.5, 0.5],
            radius: 0.5,
            half_height: 0.5,
            is_trigger: false,
            friction: 0.5,
            restitution: 0.0,
            layer: 1,
        }
    }
}

/// Kinematic character movement parameters (#560 P2).
///
/// Entity needs both `Rigidbody { body_type: Kinematic }` and capsule `Collider`;
/// `prism-app/physics` character system converts input direction, gravity and jump by these parameters into
/// `CharacterMoveRequests`, collision solved by rapier `KinematicCharacterController**.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::CharacterController")]
#[serde(default)]
pub struct CharacterController {
    /// Walk speed (meters/second).
    pub walk_speed: f32,
    /// Sprint speed (meters/second).
    pub run_speed: f32,
    /// Horizontal speed approaches target velocity acceleration (meters/second²).
    pub acceleration: f32,
    /// Jump initial velocity (meters/second).
    pub jump_speed: f32,
    /// Gravity acceleration (meters/second²).
    pub gravity: f32,
    /// Max climbable slope (degrees); beyond which character slides down.
    pub max_slope_degrees: f32,
    /// Snap back to ground when closer than this distance (meters).
    pub snap_distance: f32,
    /// Gap with environment (meters); too small causes jitter, cannot be 0.
    pub skin_offset: f32,
}

impl Default for CharacterController {
    fn default() -> Self {
        Self {
            walk_speed: 2.0,
            run_speed: 5.5,
            acceleration: 20.0,
            jump_speed: 5.0,
            gravity: 20.0,
            max_slope_degrees: 45.0,
            snap_distance: 0.3,
            skin_offset: 0.01,
        }
    }
}

/// Rigidbody = `Rigidbody**.
///
/// Real integration/solving done by `prism-app` rapier thread (#522), supports parent-child hierarchy;
/// Entities with `Collider` but no `Rigidbody` treated as static obstacle.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::Rigidbody")]
#[serde(default)]
pub struct Rigidbody {
    pub body_type: BodyType,
    /// Mass (kilograms); only meaningful for `Dynamic`.
    pub mass: f32,
    pub linear_damping: f32,
    pub angular_damping: f32,
    pub use_gravity: bool,
    /// Continuous collision detection (CCD): fast-moving rigidbody won't tunnel through thin colliders.
    /// Only meaningful for `Dynamic` rigidbody; default off.
    pub ccd: bool,
}

impl Default for Rigidbody {
    fn default() -> Self {
        Self {
            body_type: BodyType::Dynamic,
            mass: 1.0,
            linear_damping: 0.0,
            angular_damping: 0.05,
            use_gravity: true,
            ccd: false,
        }
    }
}

// ---------------------------------------------------------------------------
// 2D Physics
// ---------------------------------------------------------------------------

/// 2D rigidbody type = `RigidBody2DType**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RigidBody2DType {
    /// Static object: stationary, only participates in collision and raycast (ground, terrain boundary, indestructible wall).
    #[default]
    Static,
    /// Kinematic object: moved directly by code/animation, pushes dynamic objects, not affected by gravity or external impulse (moving platform, elevator).
    Kinematic,
    /// Dynamic object: fully simulated by 2D physics system (gravity, collision impulse, bounce, drag).
    Dynamic,
}

impl ReflectType for RigidBody2DType {
    fn field_type() -> FieldType {
        FieldType::U8
    }

    fn enum_variants() -> Option<&'static [&'static str]> {
        Some(&["Static", "Kinematic", "Dynamic"])
    }

    fn to_value(&self) -> Value {
        Value::U64(match self {
            Self::Static => 0,
            Self::Kinematic => 1,
            Self::Dynamic => 2,
        })
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        match value.as_u64()? {
            0 => Ok(Self::Static),
            1 => Ok(Self::Kinematic),
            2 => Ok(Self::Dynamic),
            other => Err(ReflectError::BadData(format!(
                "invalid RigidBody2DType discriminant {other}"
            ))),
        }
    }
}

/// 2D rigidbody = `RigidBody2D** (#1160).
///
/// Physics entity with 2D mass, linear velocity, angular velocity and gravity factor.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::RigidBody2D")]
#[serde(default)]
pub struct RigidBody2D {
    pub body_type: RigidBody2DType,
    /// Mass (kilograms), effective for Dynamic objects.
    pub mass: f32,
    /// 2D linear velocity [vx, vy] (meters/second).
    pub linear_velocity: [f32; 2],
    /// 2D angular velocity (radians/second).
    pub angular_velocity: f32,
    /// Gravity acceleration multiplier (default 1.0).
    pub gravity_scale: f32,
    /// Linear drag coefficient (slows translation speed).
    pub linear_damping: f32,
    /// Angular drag coefficient (slows rotation speed).
    pub angular_damping: f32,
    /// Whether to fix orientation (prohibits rotation, commonly used for 2D character controller).
    pub fixed_rotation: bool,
    /// Continuous collision detection switch (prevents extremely fast objects like bullets from penetrating walls).
    pub is_bullet: bool,
}

impl Default for RigidBody2D {
    fn default() -> Self {
        Self {
            body_type: RigidBody2DType::Dynamic,
            mass: 1.0,
            linear_velocity: [0.0, 0.0],
            angular_velocity: 0.0,
            gravity_scale: 1.0,
            linear_damping: 0.0,
            angular_damping: 0.05,
            fixed_rotation: false,
            is_bullet: false,
        }
    }
}

/// 2D collider shape = `Collider2DShape**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Collider2DShape {
    /// 2D rectangle box: `half_extents` [half_w, half_h].
    #[default]
    Box,
    /// 2D circle: `radius` radius.
    Circle,
    /// 2D capsule: `radius` radius + `half_height` half height.
    Capsule,
    /// 2D polygon: `vertices` vertex list.
    Polygon,
    /// 2D line segment: `offset` start point to `segment_b` end point.
    Segment,
}

impl ReflectType for Collider2DShape {
    fn field_type() -> FieldType {
        FieldType::U8
    }

    fn enum_variants() -> Option<&'static [&'static str]> {
        Some(&["Box", "Circle", "Capsule", "Polygon", "Segment"])
    }

    fn to_value(&self) -> Value {
        Value::U64(match self {
            Self::Box => 0,
            Self::Circle => 1,
            Self::Capsule => 2,
            Self::Polygon => 3,
            Self::Segment => 4,
        })
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        match value.as_u64()? {
            0 => Ok(Self::Box),
            1 => Ok(Self::Circle),
            2 => Ok(Self::Capsule),
            3 => Ok(Self::Polygon),
            4 => Ok(Self::Segment),
            other => Err(ReflectError::BadData(format!(
                "invalid Collider2DShape discriminant {other}"
            ))),
        }
    }
}

/// 2D collider = `PhysicsCollider2D** (#1160).
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::PhysicsCollider2D")]
#[serde(default)]
pub struct PhysicsCollider2D {
    pub shape: Collider2DShape,
    /// Shape center local offset relative to entity origin.
    pub offset: [f32; 2],
    /// Box half size [half_w, half_h].
    pub half_extents: [f32; 2],
    /// Circle / capsule radius.
    pub radius: f32,
    /// Capsule half height.
    pub half_height: f32,
    /// Polygon convex hull local vertex list.
    pub vertices: Vec<[f32; 2]>,
    /// Line segment end point.
    pub segment_b: [f32; 2],
    /// Sensor trigger (only produces enter/leave events, does not block object movement).
    pub is_sensor: bool,
    /// Friction coefficient [0, 1].
    pub friction: f32,
    /// Restitution coefficient [0, 1] (0 = fully inelastic, 1 = fully elastic).
    pub restitution: f32,
    /// Density (kg/m²), used to calculate collider volume mass.
    pub density: f32,
    /// Collision group layer mask bits.
    pub collision_groups: u32,
    /// Collision filter layer mask bits.
    pub collision_mask: u32,
}

impl Default for PhysicsCollider2D {
    fn default() -> Self {
        Self {
            shape: Collider2DShape::Box,
            offset: [0.0, 0.0],
            half_extents: [0.5, 0.5],
            radius: 0.5,
            half_height: 0.5,
            vertices: Vec::new(),
            segment_b: [1.0, 0.0],
            is_sensor: false,
            friction: 0.2,
            restitution: 0.0,
            density: 1.0,
            collision_groups: 1,
            collision_mask: 0xFFFFFFFF,
        }
    }
}

impl PhysicsCollider2D {
    pub fn new_box(half_w: f32, half_h: f32) -> Self {
        Self {
            shape: Collider2DShape::Box,
            half_extents: [half_w, half_h],
            ..Default::default()
        }
    }

    pub fn new_circle(radius: f32) -> Self {
        Self {
            shape: Collider2DShape::Circle,
            radius,
            ..Default::default()
        }
    }

    pub fn new_capsule(radius: f32, half_height: f32) -> Self {
        Self {
            shape: Collider2DShape::Capsule,
            radius,
            half_height,
            ..Default::default()
        }
    }

    pub fn new_polygon(vertices: Vec<[f32; 2]>) -> Self {
        Self {
            shape: Collider2DShape::Polygon,
            vertices,
            ..Default::default()
        }
    }

    pub fn new_segment(a: [f32; 2], b: [f32; 2]) -> Self {
        Self {
            shape: Collider2DShape::Segment,
            offset: a,
            segment_b: b,
            ..Default::default()
        }
    }
}

pub type BoxCollider2D = PhysicsCollider2D;
pub type CircleCollider2D = PhysicsCollider2D;
pub type PolygonCollider2D = PhysicsCollider2D;

// ---------------------------------------------------------------------------
// Skinning
// ---------------------------------------------------------------------------

/// Skeleton root (#560): points to a `.rskel`, skeleton shared by all
/// [`SkinnedMeshRenderer`] bones + reference pose.
///
/// Bone world matrices computed by `prism-anim` at runtime from reference pose + animation clip,
/// scene file only stores paths, no per-bone data.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::SkeletonRoot")]
#[serde(default)]
#[derive(Default)]
pub struct SkeletonRoot {
    /// `.rskel` path (relative to `assets/`).
    pub skeleton_path: String,
    /// Default animation clip path (`.ranim`, optional: code/editor decides which to play).
    pub default_clip_path: String,
}

impl SkeletonRoot {
    pub fn has_skeleton(&self) -> bool {
        !self.skeleton_path.trim().is_empty()
    }
}

/// Skinned mesh renderer (#560).
///
/// Alongside [`MeshRenderer`] not extending it: `.rskin` vertices carry joints/weights,
/// upload path different from normal `.rmes`, GPU skinning (P0-3) also takes separate pipeline branch.
/// Material, shadow, culling "how to draw" settings copy [`MeshRenderer`] semantics.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::SkinnedMeshRenderer")]
#[serde(default)]
#[derive(Default)]
pub struct SkinnedMeshRenderer {
    /// `.rskin` path (relative to `assets/`).
    pub skinned_mesh_path: String,
    /// One material path per submesh; uses `material_path` when empty.
    pub material_paths: Vec<String>,
    /// Single material fallback path.
    pub material_path: String,
    /// Optional custom fragment shader asset path (`.frag.spv` / `.slang`).
    pub shader_path: String,
    /// Shadow casting mode (same as [`MeshRenderer`]).
    pub shadow_casting_mode: ShadowCastingMode,
}

impl SkinnedMeshRenderer {
    pub fn has_mesh(&self) -> bool {
        !self.skinned_mesh_path.trim().is_empty()
    }

    /// Equivalent [`MeshRenderer`] needed for draw/material parse (mesh path empty: mesh comes from `.rskin`).
    ///
    /// Let skinned entities reuse static mesh resource parsing and draw state code, instead of copying again.
    pub fn as_mesh_renderer(&self) -> MeshRenderer {
        MeshRenderer {
            mesh_path: String::new(),
            material_path: self.material_path.clone(),
            material_paths: self.material_paths.clone(),
            shader_path: self.shader_path.clone(),
            shadow_casting_mode: self.shadow_casting_mode,
            ..MeshRenderer::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Skinning Animation
// ---------------------------------------------------------------------------

/// One animation layer over base animation (#560 P1 Layer / Avatar Mask).
///
/// `clip_path` samples on same timeline as base clip; `weight` of 0 skips whole layer.
/// `bone_mask` empty means full body; otherwise only listed bones affected (names consistent with
/// `SkeletonRoot` skeleton joint names consistent, missing names ignored).
#[derive(Debug, Clone, Serialize, Deserialize, Reflect, PartialEq)]
#[reflect(name = "prism_engine::scene::AnimLayer")]
#[serde(default)]
pub struct AnimLayer {
    /// Clip path for this layer (`.ranim`, relative to `assets/`).
    pub clip_path: String,
    /// Blend weight (`0..=1`, NaN/out-of-range cleaned by engine).
    pub weight: f32,
    /// `true` = additive layer relative to reference (bind) pose; `false` = normal override layer.
    pub additive: bool,
    /// Only these bones participate in this layer; empty = full body.
    pub bone_mask: Vec<String>,
}

impl Default for AnimLayer {
    fn default() -> Self {
        Self {
            clip_path: String::new(),
            weight: 1.0,
            additive: false,
            bone_mask: Vec::new(),
        }
    }
}

/// Foot IK settings for one skeleton (#560 P4).
///
/// Bone names are resolved against the skeleton loaded through [`SkeletonRoot`].
/// If any of them is missing the pose is left exactly as the clip, state machine
/// or graph produced it, instead of guessing. Without this component (or with
/// `enabled = false`) the animator output does not change.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::FootIk")]
#[serde(default)]
pub struct FootIk {
    /// Master switch; the component stays authored but inert when `false`.
    pub enabled: bool,
    /// Pelvis joint. Both leg chains must be children of it.
    pub hips_joint: String,
    pub left_thigh: String,
    pub left_shin: String,
    pub left_foot: String,
    pub right_thigh: String,
    pub right_shin: String,
    pub right_foot: String,
    /// Collider layer mask the ground probe considers (`0` = nothing).
    pub ground_layer: u32,
    /// How far above the ankle the downward probe starts.
    pub probe_height: f32,
    /// Ground higher than this above the ankle counts as a wall, not a step.
    pub max_step_up: f32,
    /// Ground farther than this below the ankle counts as no ground.
    pub max_step_down: f32,
    /// Ankle height above the ground when planted.
    pub foot_offset: f32,
    /// How far the pelvis may be lowered (negative) when a planted foot would
    /// pull a leg past its reach.
    pub pelvis_min: f32,
    /// Smoothing time constant for plant targets and weights; `0` snaps.
    pub blend_seconds: f32,
    /// Global weight, `0` disables the solve without removing the component.
    pub weight: f32,
}

impl Default for FootIk {
    fn default() -> Self {
        Self {
            enabled: true,
            hips_joint: String::new(),
            left_thigh: String::new(),
            left_shin: String::new(),
            left_foot: String::new(),
            right_thigh: String::new(),
            right_shin: String::new(),
            right_foot: String::new(),
            ground_layer: 1,
            probe_height: 0.15,
            max_step_up: 0.35,
            max_step_down: 0.45,
            foot_offset: 0.05,
            pelvis_min: -0.25,
            blend_seconds: 0.08,
            weight: 1.0,
        }
    }
}

/// Skeleton playback control (#560 P0-2).
///
/// Event source is cooked `.ranim` clip; skeleton specified by [`SkeletonRoot`] on same entity.
/// When `clip_path` empty, uses `SkeletonRoot::default_clip_path`.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::Animator")]
#[serde(default)]
pub struct Animator {
    /// Animation clip path (`.ranim`, relative to `assets/`). Empty = uses skeleton default clip.
    pub clip_path: String,
    /// Start time (seconds); editor scrub writes directly here.
    pub time: f32,
    /// Playback speed multiplier (negative reverses).
    pub speed: f32,
    /// Loop playback.
    pub looped: bool,
    /// Whether to advance time.
    pub playing: bool,
    /// State machine (`.rasm`) path (#560 P1). When non-empty and loaded, **state machine decides which clip to play**:
    /// `clip_path` / blend space only falls back when state's own clip is empty.
    pub state_machine_path: String,
    /// Animation graph (`.rag` / `*.animgraph.json`) path (#560 P1). When non-empty and loaded,
    /// **takes priority over** state machine / blend space / single clip: evaluated by `AnimGraphEvaluator`.
    pub anim_graph_path: String,
    /// Current state name (engine writes back every frame; author can pre-set start state). Empty / not found = first state.
    pub state: String,
    /// 1D blend space clip paths (#560 P1). ≥2 items and clip found **takes priority over**
    /// `clip_path`: pose interpolated by `blend_parameter` on `blend_positions`.
    pub blend_clip_paths: Vec<String>,
    /// Parameter axis positions corresponding one-to-one with [`Self::blend_clip_paths`] (need not be sorted).
    pub blend_positions: Vec<f32>,
    /// Blend parameter (game writes every frame, usually velocity). Clamped to ends when out of range.
    pub blend_parameter: f32,
    /// Blend parameter smooth time constant (seconds, #560 P1). `0` = use written value directly;
    /// `> 0` exponentially approaches target (per frame `1 - exp(-dt / tau)`), avoids game-side velocity
    /// jump causing pose pop. Only applies to blend space parameter axis, does not change component value.
    pub blend_smoothing: f32,
    /// 2D blend space clip paths (#560 P1). ≥3 items and `blend_2d_positions`
    /// length consistent **takes priority over** 1D blend space and `clip_path`: pose from parameter plane
    /// positions interpolated by triangulation between sample points.
    pub blend_2d_clip_paths: Vec<String>,
    /// Parameter plane coordinates corresponding one-to-one with [`Self::blend_2d_clip_paths`]
    /// (`[x, y]`, usually x = velocity, y = direction). Need not be sorted.
    pub blend_2d_positions: Vec<[f32; 2]>,
    /// 2D blend space second parameter axis (y). Clamped to sample convex hull boundary when out of range.
    pub blend_parameter_y: f32,
    /// Additive clip path (#560 P1). Empty = no additive layer.
    ///
    /// Additive clip local transform relative to reference (bind) pose difference then multiplied by
    /// [`Self::additive_weight`] onto final pose --- breathing, aim offset, hit shake etc
    /// clips that only modify some bones don't need a separate base clip.
    pub additive_clip_path: String,
    /// Additive layer weight (`0` = off, `1` = full increment). Negative / NaN treated as 0.
    pub additive_weight: f32,
    /// Cross-fade duration when changing clip (seconds).
    ///
    /// When `clip_path` changed at runtime, transitions from **pose where old clip stopped** to new clip,
    /// instead of hard cut (hard cut makes character "jump"). `0` = direct switch; negative / NaN also treated as 0
    /// handle. Transition itself only affects pose, does not change component `time`.
    pub blend_duration: f32,
    /// Additional animation layer on top of base pose (#560 P1). Stacked in array order, later layers
    /// cover previous results; `additive` layer applied as relative to bind pose increment.
    ///
    /// These layers evaluated after cross-fade, do not participate in base clip transitions.
    pub layers: Vec<AnimLayer>,
    /// Animation LOD (#560 P6): when enabled, entity farther than [`Self::lod_far`] meters from camera
    /// **stops advancing** (frozen at current pose), continues playing when back in range.
    ///
    /// Default off, to avoid behavior change when no camera / editor preview. Frequency reduction and bone subset are later tiers.
    pub lod_enabled: bool,
    /// Animation LOD freeze distance (meters). Non-finite or `<= 0` treated as 50m.
    pub lod_far: f32,
    /// Animation LOD frequency reduction start (meters): between `lod_near..lod_far` evaluated every other frame,
    /// each update advances `2 * dt` to maintain playback speed. Non-finite / `<= 0` / larger than `lod_far`
    /// equals `lod_far` (no frequency reduction tier).
    pub lod_near: f32,
    /// Animation LOD "bone subset" tier (#560 P6): beyond `lod_far` only updates root joint and bones
    /// listed here, other bones keep last frame pose. Empty list = not enabled (same as `lod_root_only`
    /// same, only single-clip entities supported); names parsed by skeleton, missing names ignored.
    /// Root joint always participates in update, no need to write it in.
    pub lod_bones: Vec<String>,
    /// Animation LOD "root-only" tier (#560 P6): beyond `lod_far` not frozen as whole, but
    /// only advances time and samples root joint (other bones keep last frame pose), root motion consumption still effective.
    /// Only supports single clip / regular clip path; state machine, blend space, animation graph entities still frozen as whole
    /// (they need full evaluator advance). First time entering this tier, evaluates one full frame first.
    pub lod_root_only: bool,
    /// Root motion consumption (#560 P2): `true` adds each frame root displacement increment to entity's
    /// [`LocalTransform`], and clears root translation in pose, avoiding "entity moved, skeleton root
    /// root also moves" double displacement. Only consumes **translation**; root rotation increment still only recorded in
    /// `AnimatorPlayhead.root_motion`, waiting for CharacterMovement to take over.
    pub root_motion_consume: bool,
}

impl Default for Animator {
    fn default() -> Self {
        Self {
            clip_path: String::new(),
            time: 0.0,
            speed: 1.0,
            looped: true,
            playing: true,
            state_machine_path: String::new(),
            anim_graph_path: String::new(),
            state: String::new(),
            blend_clip_paths: Vec::new(),
            blend_positions: Vec::new(),
            blend_parameter: 0.0,
            blend_smoothing: 0.0,
            blend_2d_clip_paths: Vec::new(),
            blend_2d_positions: Vec::new(),
            blend_parameter_y: 0.0,
            additive_clip_path: String::new(),
            additive_weight: 1.0,
            // 0.2s ≈ UE default blend time: short enough not to drag, covers most pose differences.
            blend_duration: 0.2,
            layers: Vec::new(),
            lod_enabled: false,
            lod_far: 50.0,
            lod_near: 25.0,
            root_motion_consume: false,
            lod_root_only: false,
            lod_bones: Vec::new(),
        }
    }
}

impl Animator {
    pub fn has_clip(&self) -> bool {
        !self.clip_path.trim().is_empty()
    }

    /// Whether to use 1D blend space (at least two clips + at least two positions).
    pub fn uses_blend_space(&self) -> bool {
        self.blend_clip_paths.len() >= 2 && self.blend_positions.len() >= 2
    }

    /// Whether to use 2D blend space (at least three clips + at least three positions, and can form triangle).
    ///
    /// Only two points / all collinear means triangulation has no area, runtime falls back to 1D blend space.
    pub fn uses_blend_space_2d(&self) -> bool {
        self.blend_2d_clip_paths.len() >= 3 && self.blend_2d_positions.len() >= 3
    }
}

// ---------------------------------------------------------------------------
// Terrain
// ---------------------------------------------------------------------------

/// Heightfield terrain (#517 Phase 1).
///
/// Just a data component: runtime generates terrain mesh from `heightmap` (f32 little-endian height array) + mesh parameters,
/// Terrain mesh. `chunks == 1` attaches to same entity [`MeshRenderer`]; `chunks > 1` when
/// generates a runtime child entity per chunk, convenient for per-chunk frustum culling. `lod_distances` then generates
/// 1-3 level terrain LOD; `vegetation_*` scatters vegetation instances sharing mesh by height/slope;
/// `splat` writes height/slope color bands to vertex colors. Material still determined by MeshRenderer.
///
/// Height array is row-major `resolution × resolution` f32 (`resolution = 0` treated as 32),
/// world size is `size` meters (x/z), `height_scale` scales `[0, 1]` height to meters.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::Terrain")]
#[serde(default)]
pub struct Terrain {
    /// `.raw` heightmap path (relative to `assets/`, parsed same as other asset paths).
    pub heightmap: String,
    /// Horizontal size (meters): `[x, z]`.
    pub size: [f32; 2],
    /// Height multiplier: mesh height = heightmap value × `height_scale` (meters).
    ///
    /// heightmap CLI output `.raw` already in meters, so default here is `1.0`;
    /// normalized `[0, 1]` heightfield can use it (e.g. `100.0`) to convert to meters.
    pub height_scale: f32,
    /// Cells per side. `0` = follows heightmap size (sample count must be perfect square).
    ///
    /// When non-zero, height array length must exactly be `resolution * resolution`, otherwise falls back to flat terrain and warns.
    pub resolution: u32,
    /// How many chunks per side (#517 Phase 2). `1` = one mesh for whole terrain;
    /// greater than `1` each chunk is independent entity / independent mesh, rendering and shadows can be culled per chunk.
    ///
    /// Clamped at runtime to `1..=16`, and won't exceed `cells - 1` (at least one quad per chunk).
    pub chunks: u32,
    /// Terrain LOD distance thresholds (meters), arranged LOD0, LOD1... (#517 Phase 2).
    ///
    /// Empty array = no LOD generated; default two thresholds generates 3 levels (step 1 / 2 / 4).
    /// Distance takes nearest enabled camera to terrain origin distance, **entire terrain uses single level**, avoids adjacent
    /// chunks using different LODs causing seam cracks.
    pub lod_distances: Vec<f32>,
    /// Vegetation density: instances per 100 m² (0 = not generated). Clamped to max 4096 at runtime.
    pub vegetation_density: f32,
    /// Vegetation mesh path; empty = uses built-in double-sided cross-quad (height 1 m).
    pub vegetation_mesh: String,
    /// Vegetation material path; empty = default material (author can fill grass / leaf material).
    pub vegetation_material: String,
    /// Single plant height (meters): instances scaled on Y axis by this value.
    pub vegetation_size: f32,
    /// Procedural color band splat (#517): writes sand/grass/rock/snow to vertex colors by height/slope,
    /// forward PBR uses vertex color to modulate albedo. Default off, existing scenes unaffected.
    pub splat: bool,
    /// Texture layer splat (#517): up to 4 material paths, mixed by height/slope.
    ///
    /// When non-empty vertex color becomes 4 layer weights (R/G/B first three layers, fourth = 1 - R - G - B),
    /// forward PBR samples albedo/normal/metallic-roughness of each material. Empty array = off.
    pub splat_materials: Vec<String>,
    /// Texture layer splat UV tiling multiplier (default 16; only effective when `splat_materials` non-empty).
    pub splat_tiling: f32,
}

impl Default for Terrain {
    fn default() -> Self {
        Self {
            heightmap: String::new(),
            size: [100.0, 100.0],
            height_scale: 1.0,
            resolution: 0,
            chunks: 4,
            lod_distances: vec![80.0, 200.0],
            vegetation_density: 0.0,
            vegetation_mesh: String::new(),
            vegetation_material: String::new(),
            vegetation_size: 1.0,
            splat: false,
            splat_materials: Vec::new(),
            splat_tiling: 16.0,
        }
    }
}

impl Terrain {
    /// Cells per side: `0` uses default, then clamped to safe range.
    pub fn cells(&self) -> usize {
        let cells = if self.resolution == 0 {
            32
        } else {
            self.resolution
        };
        cells.clamp(2, 1024) as usize
    }

    pub fn has_heightmap(&self) -> bool {
        !self.heightmap.trim().is_empty()
    }

    /// Requested blocks per side, clamped to `1..=16`; actual block count also limited by `cells - 1`.
    pub fn chunk_count(&self) -> usize {
        if self.chunks == 0 {
            1
        } else {
            self.chunks.clamp(1, 16) as usize
        }
    }

    /// LOD level count: 1 level without thresholds, otherwise `1 + threshold count`, max 3 levels (step 1/2/4).
    pub fn lod_level_count(&self) -> usize {
        (1 + self.lod_distances.len()).min(3)
    }
}

// ---------------------------------------------------------------------------
// Audio
// ---------------------------------------------------------------------------

/// Audio bus = grouped volume (#508).
///
/// `Master` multiplies **all** buses; remaining buses each multiply another layer. Project code can call
/// `prism_app::App::set_audio_bus_volume` in settings menu, no need to rebuild any voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum AudioBus {
    Master,
    Music,
    #[default]
    Sfx,
    Ambient,
    Voice,
}

impl AudioBus {
    pub const ALL: [Self; 5] = [
        Self::Master,
        Self::Music,
        Self::Sfx,
        Self::Ambient,
        Self::Voice,
    ];

    pub fn index(self) -> usize {
        match self {
            Self::Master => 0,
            Self::Music => 1,
            Self::Sfx => 2,
            Self::Ambient => 3,
            Self::Voice => 4,
        }
    }
}

impl ReflectType for AudioBus {
    fn field_type() -> FieldType {
        FieldType::U8
    }

    fn enum_variants() -> Option<&'static [&'static str]> {
        Some(&["Master", "Music", "Sfx", "Ambient", "Voice"])
    }

    fn to_value(&self) -> Value {
        Value::U64(self.index() as u64)
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        match value.as_u64()? {
            0 => Ok(Self::Master),
            1 => Ok(Self::Music),
            2 => Ok(Self::Sfx),
            3 => Ok(Self::Ambient),
            4 => Ok(Self::Voice),
            other => Err(ReflectError::BadData(format!(
                "invalid AudioBus discriminant {other}"
            ))),
        }
    }
}

/// Audio player = `AudioSource`.
///
/// Only carries **authoring-time** parameters: clip path + playback/attenuation settings. Real decoding and mixing done at host layer
/// (`prism-app::scene_audio`): takes file from `assets/` by path, background thread decodes,
/// `AudioEngine::play`, consistent with `MeshRenderer` → `MeshRef` layering --- engine/scene layer
/// does not hold any audio handle.
///
/// Spatialization does **distance attenuation + equal-power panning**: `spatial_blend = 0` plays 2D (no attenuation,
/// centered), `= 1` fully attenuates per [`AudioSource::volume_at`], and by sound source horizontal angle in listener space
/// horizontal angle panning. Panning updated in real-time by mixer spatialization node, not baked into clip (#508, #1034).
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::AudioSource")]
#[serde(default)]
pub struct AudioSource {
    /// Audio asset path, relative to `assets/` (`.ogg` / `.wav` / `.mp3` / `.flac`).
    /// Empty string = not playing.
    pub sound_path: String,
    /// Volume, `0.0..=1.0` (multiplied by distance attenuation and listener gain).
    pub volume: f32,
    /// Loop playback: automatically restarts after finishing.
    pub looped: bool,
    /// Auto-play after scene load; when off component is just "has a clip loaded", needs code/editor manual trigger.
    pub play_on_awake: bool,
    /// Volume group (#508); `Master` bus multiplies over all groups.
    pub bus: AudioBus,
    /// 3D spatialization degree: `0.0` = pure 2D (no attenuation), `1.0` = fully distance attenuated.
    pub spatial_blend: f32,
    /// Near distance where attenuation starts (world units).
    pub min_distance: f32,
    /// Far distance where attenuation reaches silence (world units).
    pub max_distance: f32,
}

impl Default for AudioSource {
    fn default() -> Self {
        Self {
            sound_path: String::new(),
            volume: 1.0,
            looped: false,
            play_on_awake: true,
            bus: AudioBus::Sfx,
            spatial_blend: 0.0,
            min_distance: 1.0,
            max_distance: 50.0,
        }
    }
}

impl AudioSource {
    /// Whether clip configured (empty path = silent placeholder).
    pub fn has_sound(&self) -> bool {
        !self.sound_path.is_empty()
    }

    /// **Actual volume** at given distance to listener (already includes `volume` and `spatial_blend`).
    ///
    /// Curve: 1.0 within `min_distance`, then linear decay, 0 at `max_distance`,
    /// stays 0 beyond. `spatial_blend` linearly mixes this curve with "2D 1.0".
    pub fn volume_at(&self, distance: f32) -> f32 {
        let near = self.min_distance.max(0.0);
        let far = self.max_distance.max(near);
        let attenuation = if !distance.is_finite() || distance >= far {
            0.0
        } else if distance <= near || far <= near {
            1.0
        } else {
            1.0 - (distance - near) / (far - near)
        };
        let blend = self.spatial_blend.clamp(0.0, 1.0);
        self.volume.clamp(0.0, 1.0) * (1.0 - blend * (1.0 - attenuation))
    }
}

/// Listener = `AudioListener**.
///
/// Position taken from entity `LocalToWorld`; when no listener in scene host falls back to "first camera entity",
/// then uses world origin. With multiple listeners only first is recognized (same convention as `Skybox`).
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::AudioListener")]
#[serde(default)]
pub struct AudioListener {
    /// Global gain, `0.0..=1.0` (multiplied onto each `AudioSource` volume).
    pub gain: f32,
}

impl Default for AudioListener {
    fn default() -> Self {
        Self { gain: 1.0 }
    }
}

// ---------------------------------------------------------------------------
// Scene management
// ---------------------------------------------------------------------------

/// Marks an entity as belonging to a specific scene.
///
/// Used for batch unload and multi-scene bookkeeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SceneMember(pub SceneAssetId);

#[cfg(test)]
#[path = "components_tests.rs"]
mod tests;
