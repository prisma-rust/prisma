//! Per-frame draw item DTOs and camera view definitions.

use crate::managers::MeshHandle;

/// Per-draw render state (`Material` / `Renderer` fields determining pipeline state).
///
/// It is also `ForwardPass` pipeline cache key: draws mixing different states each get correct
/// cull / depth write / blend config, while descriptor set layout and push constants same across variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DrawState {
    /// Cull mode (`CullMode`).
    pub cull_mode: prism_gfx::CullMode,
    /// Render queue (`Material.renderQueue`), determines draw order.
    pub render_queue: i32,
    /// Whether to write depth (`_ZWrite`).
    pub z_write: bool,
    /// Source blend factor (`_SrcBlend`).
    pub src_blend: prism_gfx::BlendMode,
    /// Destination blend factor (`_DstBlend`).
    pub dst_blend: prism_gfx::BlendMode,
    /// Blend operation (`_BlendOp`).
    pub blend_op: prism_gfx::BlendOp,
}

impl Default for DrawState {
    fn default() -> Self {
        Self {
            cull_mode: prism_gfx::CullMode::Back,
            render_queue: prism_gfx::RenderQueue::GEOMETRY,
            z_write: true,
            src_blend: prism_gfx::BlendMode::One,
            dst_blend: prism_gfx::BlendMode::Zero,
            blend_op: prism_gfx::BlendOp::Add,
        }
    }
}

impl DrawState {
    /// Whether to enable blend; uses `One` / `Zero` to mean non-transparent.
    pub fn blends(&self) -> bool {
        self.src_blend != prism_gfx::BlendMode::One
            || self.dst_blend != prism_gfx::BlendMode::Zero
            || self.blend_op != prism_gfx::BlendOp::Add
    }
}

/// One draw call's static data, supplied by the engine each frame.
/// Graph passes read these to record geometry draws into their attachments.
#[derive(Clone)]
pub struct DrawItem {
    /// GPU mesh handle (from [`crate::managers::RenderMeshManager`]).
    pub mesh: MeshHandle,
    /// ECS entity id that produced this draw item (`prism_ecs::Entity::id()`).
    ///
    /// Renderer itself does not depend on ECS; editor passes selected entity id to
    /// [`GraphRenderer::set_outline_entities`], `ForwardPass` draws selection outline for
    /// corresponding mesh (#487). Procedural / test draws use `u32::MAX` for no entity.
    pub entity_id: u32,
    /// Model matrix (world transform) for this instance.
    pub model: [[f32; 4]; 4],
    /// Material SSBO slot (index into `RenderMaterialManager`'s
    /// `GpuMaterial[]` buffer) for the bindless PBR path. `None` -> slot 0
    /// (the fallback material). `app.rs` resolves `MaterialHandle` -> slot
    /// via `mat_map` when building the draw list, so passes can push the
    /// slot directly without a per-draw `slot_of()` lookup.
    pub material: Option<u32>,
    /// Whether to write shadow map (`ShadowCastingMode::casts_shadows`).
    pub cast_shadow: bool,
    /// Whether visible in camera main pass (`ShadowsOnly` is `false`).
    pub visible_in_main_pass: bool,
    /// Whether shadow pass disables back-face cull (`ShadowCastingMode::TwoSided`).
    /// `false` = back-face cull (default), `true` = double-sided casting. Read only by `ShadowMapPass`.
    pub cast_shadow_two_sided: bool,
    /// Pipeline state (cull / queue / depth write / blend), from `MeshRenderer`.
    /// Shadow map pipeline ignores cull and blend (fixed double-sided, no blend write depth).
    pub state: DrawState,
    /// World-space AABB (computed once in mesh lifetime); without bounds conservatively not culled.
    ///
    /// Main camera frustum cull used in extract phase; light frustum cull (#526) after frame packing uses it
    /// to decide whether still needs to write shadow map.
    pub world_bounds: Option<crate::culling::Aabb>,
    /// Starting index in index buffer (#520 submesh). Non-indexed mesh ignores.
    pub index_start: u32,
    /// Index count for this draw; `None` = entire mesh (legacy).
    ///
    /// Draw item with `Some(0)` skipped by renderer, lets caller express empty submesh.
    pub index_count: Option<u32>,
    /// Skinned draw (#560 P0-3): index of this draw's first bone-palette
    /// matrix inside the frame's bone SSBO (`ForwardPass::BONE_CAPACITY`).
    ///
    /// `None` = static mesh; the vertex stage then skips the bone buffer
    /// entirely, so existing draws cost exactly what they did before.
    pub bone_offset: Option<u32>,
    /// 4 material slots for texture-layer splat (#517); `u32::MAX` = that layer missing.
    pub splat_slots: [u32; 4],
    /// UV tiling factor for texture-layer splat; `0` = not a splat draw.
    pub splat_tiling: f32,
    /// Optional custom fragment shader handle (`None` = built-in `scene_frag.frag.spv`).
    pub shader: Option<crate::ShaderHandle>,
}

impl DrawItem {
    /// World-space position (taken from `model` translation column, column-major `model[3][0..3]`).
    pub fn world_position(&self) -> [f32; 3] {
        [self.model[3][0], self.model[3][1], self.model[3][2]]
    }
}

/// Sort draw list by engine `renderQueue` rule.
///
/// - Queue values ascending; ties keep original order (`sort_by` is stable).
/// - Queue greater than [`prism_gfx::RenderQueue::GEOMETRY_LAST`] (transparent segment) internal sorted by
///   distance to camera **far to near**, so alpha blend stacking order correct.
/// - Camera position from `eye` (world space); opaque segment not distance-sorted.
pub fn sort_draw_items(items: &mut [DrawItem], eye: [f32; 3]) {
    let distance_sq = |item: &DrawItem| {
        let p = item.world_position();
        let d = [p[0] - eye[0], p[1] - eye[1], p[2] - eye[2]];
        d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
    };
    items.sort_by(|a, b| {
        a.state
            .render_queue
            .cmp(&b.state.render_queue)
            .then_with(|| {
                if b.state.render_queue > prism_gfx::RenderQueue::GEOMETRY_LAST {
                    distance_sq(b)
                        .partial_cmp(&distance_sq(a))
                        .unwrap_or(std::cmp::Ordering::Equal)
                } else {
                    std::cmp::Ordering::Equal
                }
            })
    });
}

/// One world-space particle billboard for
/// [`crate::particle_pass::ParticlePass`].
///
/// The engine simulates the particles on the CPU and hands the renderer a flat
/// list. The camera basis is **not** part of the record: the pass expands the
/// quad along `GraphFrame::inv_view_rot`'s right/up columns, so the billboard
/// faces whatever camera rendered the frame.
#[derive(Clone, Copy, Debug)]
pub struct ParticleDrawItem {
    /// World-space center (xyz) and radius in world units (w).
    pub center: [f32; 3],
    /// Billboard half-size / radius in world units.
    pub size: f32,
    /// Linear RGBA color (already faded by the particle's age).
    pub color: [f32; 4],
}

/// One 2D sprite instance for [`crate::sprite_pass::SpritePass`].
#[derive(Clone, Copy, Debug)]
pub struct SpriteDrawItem {
    /// World matrix applied to the unit quad (includes size + pivot).
    ///
    /// Ignored when [`Self::corners`] is `Some`: line triangles provide their
    /// world-space vertices explicitly (#604).
    pub model: [[f32; 4]; 4],
    /// Explicit quad corners in world space, in the order
    /// `[bottom-left, bottom-right, top-right, top-left]`.
    ///
    /// `None` for sprites (the vertex shader expands [`Self::model`]); lines
    /// use it to draw non-overlapping triangles. A triangle is a quad whose
    /// last two corners coincide.
    pub corners: Option<[[f32; 3]; 4]>,
    /// UV rect `(u0, v0, u1, v1)` inside the sprite sheet.
    pub uv_rect: [f32; 4],
    /// Linear RGBA tint multiplied with the sampled texel.
    pub color: [f32; 4],
    /// Bindless texture slot.
    pub texture: u32,
    /// Blend state for this sprite (#610).
    pub src_blend: prism_gfx::BlendMode,
    pub dst_blend: prism_gfx::BlendMode,
    pub blend_op: prism_gfx::BlendOp,
    /// World-space clipping rect `[min_x, min_y, max_x, max_y]` (#582).
    ///
    /// [`NO_CLIP`] means "draw everywhere"; anything with `max_x >= min_x`
    /// clips per-fragment in the shader.
    pub clip: [f32; 4],
    /// Whether [`SpriteLight`]s modulate this sprite (#611).
    ///
    /// `false` keeps the unlit `texture × color` result, so 2D scenes that have
    /// no lights look exactly as they did before lighting existed.
    pub receive_lights: bool,
}

/// 2D light kinds — must match `SPRITE_LIGHT_*` in `sprites.slang`.
pub const SPRITE_LIGHT_AMBIENT: u32 = 0;
pub const SPRITE_LIGHT_POINT: u32 = 1;
pub const SPRITE_LIGHT_DIRECTIONAL: u32 = 2;

/// Maximum number of 2D lights uploaded per frame (#611).
///
/// The sprite shader loops over this many entries; extra lights are dropped by
/// the engine (sorted by contribution so the strongest survive).
pub const SPRITE_LIGHT_MAX: usize = 16;

/// One 2D light for [`crate::sprite_pass::SpritePass`] (#611).
///
/// 64-byte, `std430`-friendly record (three `float4`s) mirroring
/// `SpriteLight` in `sprites.slang`. Kept separate from [`GpuLight`], whose
/// photometric units (candela / lux) are meaningless for 2D sprites.
#[derive(Clone, Copy, Debug, Default)]
pub struct SpriteLight {
    /// `x, y` = world position; `z` = falloff radius (`0` = no falloff);
    /// `w` = intensity.
    pub position_range: [f32; 4],
    /// Linear RGB colour; `w` = kind ([`SPRITE_LIGHT_AMBIENT`] / …).
    pub color_kind: [f32; 4],
    /// `x, y` = direction (world space); `zw` reserved.
    pub direction: [f32; 4],
}

impl SpriteLight {
    /// A point light at `position` with `radius` and `intensity`.
    pub fn point(position: [f32; 2], color: [f32; 3], intensity: f32, radius: f32) -> Self {
        Self {
            position_range: [position[0], position[1], radius, intensity.max(0.0)],
            color_kind: [color[0], color[1], color[2], SPRITE_LIGHT_POINT as f32],
            direction: [0.0; 4],
        }
    }

    /// A flat ambient light (no position / falloff).
    pub fn ambient(color: [f32; 3], intensity: f32) -> Self {
        Self {
            position_range: [0.0, 0.0, 0.0, intensity.max(0.0)],
            color_kind: [color[0], color[1], color[2], SPRITE_LIGHT_AMBIENT as f32],
            direction: [0.0; 4],
        }
    }

    /// A directional light (whole sprite multiplied by `color × intensity`).
    ///
    /// Direction will be normalized; zero vector / non-finite values fall back to "downward", so shader always reads
    /// a legal identity vector.
    pub fn directional(direction: [f32; 2], color: [f32; 3], intensity: f32) -> Self {
        let direction = normalize_light_direction(direction);
        Self {
            position_range: [0.0, 0.0, 0.0, intensity.max(0.0)],
            color_kind: [
                color[0],
                color[1],
                color[2],
                SPRITE_LIGHT_DIRECTIONAL as f32,
            ],
            direction: [direction[0], direction[1], 0.0, 0.0],
        }
    }

    /// Strongest-first ordering key used when trimming to [`SPRITE_LIGHT_MAX`].
    fn weight(&self) -> f32 {
        let intensity = self.position_range[3].max(0.0);
        let luma =
            0.2126 * self.color_kind[0] + 0.7152 * self.color_kind[1] + 0.0722 * self.color_kind[2];
        intensity * luma.max(0.0)
    }
}

/// Normalize 2D light direction; zero vector / non-finite values fall back to "downward".
fn normalize_light_direction(direction: [f32; 2]) -> [f32; 2] {
    let length = (direction[0] * direction[0] + direction[1] * direction[1]).sqrt();
    if length.is_finite() && length > 1e-6 {
        [direction[0] / length, direction[1] / length]
    } else {
        [0.0, -1.0]
    }
}

/// Prune `lights` to [`SPRITE_LIGHT_MAX`], higher intensity first (#611).
pub fn trim_sprite_lights(lights: &mut Vec<SpriteLight>) {
    if lights.len() <= SPRITE_LIGHT_MAX {
        return;
    }
    lights.sort_by(|a, b| b.weight().total_cmp(&a.weight()));
    lights.truncate(SPRITE_LIGHT_MAX);
}

/// Disabled clip rect: `max < min`, which the sprite shader treats as "no clip".
pub const NO_CLIP: [f32; 4] = [0.0, 0.0, -1.0, -1.0];

/// Normalize author clip rect to `[min_x, min_y, max_x, max_y]`.
///
/// Disabled, non-finite, or any axis collapsing to zero area returns [`NO_CLIP`] — author just checked
/// `clip_enabled` but not yet filled rect, sprite will not disappear from empty.
pub fn clip_bounds(enabled: bool, rect: [f32; 4]) -> [f32; 4] {
    if !enabled || rect.iter().any(|value| !value.is_finite()) {
        return NO_CLIP;
    }
    let bounds = [
        rect[0].min(rect[2]),
        rect[1].min(rect[3]),
        rect[0].max(rect[2]),
        rect[1].max(rect[3]),
    ];
    if bounds[0] == bounds[2] || bounds[1] == bounds[3] {
        return NO_CLIP;
    }
    bounds
}

/// One camera composited into the frame (#461).
///
/// Every enabled `Camera` produces one of these, sorted by `Camera.depth`
/// ascending. The forward pass draws the scene once per entry, clearing the
/// colour / depth buffers according to the camera's `clearFlags`, so a
/// depth-ordered stack of opaque views composites into the single HDR target.
///
/// The first version covers opaque full-screen layers only: no per-camera
/// viewport rect and no per-camera post stack.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraView {
    /// `surface_rotation * projection * view`.
    pub view_proj: [[f32; 4]; 4],
    /// `world -> view`.
    pub view: [[f32; 4]; 4],
    /// Inverse of `view`, used by the skybox pass to orient the environment.
    pub inv_view: [[f32; 4]; 4],
    /// Camera world position (`.w` is filled with the light count at upload).
    pub eye: [f32; 3],
    /// Clear colour (`Camera.backgroundColor`).
    pub clear_color: [f32; 4],
    /// `clearFlags` clears the colour buffer (`Skybox` / `SolidColor`).
    pub clear_color_enabled: bool,
    /// `clearFlags` clears the depth buffer (everything except `Nothing`).
    pub clear_depth: bool,
    /// Draw the skybox for this camera (`clearFlags == Skybox`).
    pub draw_skybox: bool,
    /// Exposure multiplier for this camera.
    pub exposure: f32,
    /// `false` for the synthetic fallback when the scene has no camera.
    pub has_camera: bool,
}

impl CameraView {
    /// Build the implicit single-camera view used by the legacy call sites
    /// (`GraphRenderer::render`, tests) whose `FrameUBOData` describes the only
    /// camera.
    pub fn legacy(
        view_proj: [[f32; 4]; 4],
        inv_view: [[f32; 4]; 4],
        clear_color: [f32; 4],
        has_camera: bool,
    ) -> Self {
        Self {
            view_proj,
            view: inv_view,
            inv_view,
            eye: [0.0; 3],
            clear_color,
            clear_color_enabled: true,
            clear_depth: true,
            draw_skybox: true,
            exposure: 1.0,
            has_camera,
        }
    }
}
