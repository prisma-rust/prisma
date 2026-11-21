//! Prisma 的模块化渲染通道图。
//!
//! 取代旧版整体式渲染器。每个渲染阶段（GBuffer、RayQuery、SHARC GI、Lighting、Post）
//! 是一个 [`RenderPassNode`]，声明其输入/输出和一个执行方法。
//! 通道注册到 [`RenderGraph`] 中，该图管理临时资源分配和执行顺序。
//!
//! ## 设计
//!
//! - **通道是 trait 对象**——可在运行时添加/移除（特性开关：RT 开/关、GI 模式切换）。
//! - **资源句柄是类型化 ID**——图拥有实际的 Vulkan 资源；通道通过句柄引用它们，
//!   而非原始 `vk::Image`。
//! - **临时附件**使用 `LAZILY_ALLOCATED` 内存以实现 TBDR 效率（参见 `transient.rs`）。
//! - **子通道融合**——读取彼此 GBuffer 的通道可以融合到单个渲染通道中，
//!   以避免瓦片内存回写。

use std::collections::HashMap;
use std::time::Instant;

use anyhow::Result;
use ash::vk;

use crate::context::VulkanContext;
use crate::descriptor::{FrameUBO, GpuLight, PtAnalyticLight};
use crate::managers::RenderMeshManager;

pub mod draw_items;
pub mod settings;

/// Passes slower than this are logged with `target: "startup"`.
///
/// Lazy pipeline compilation makes the first frame of a pass far slower than
/// every frame after it, so a startup trace needs those numbers; logging every
/// pass at frame rate would be noise.
const SLOW_PASS_MS: f64 = 1.0;

pub use draw_items::*;
pub use settings::*;

/// A typed handle to a graph-managed resource (image, buffer).
/// The inner `u32` is an index into the graph's resource table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ResourceHandle(pub u32);

/// Well-known graph-edge 资源 handles published by `ForwardPass` and 读取
/// by downstream passes (`GtaoPass`, `PostPass`). Fixed (not counter-based)
/// so a pass added later can reference them without knowing the upstream
/// pass's internal handle field. The graph's `next_handle` counter is kept
/// below this range (see `create_resource_at`), so there is no collision.
pub const FORWARD_DEPTH_H: ResourceHandle = ResourceHandle(1000);
pub const FORWARD_NORMAL_H: ResourceHandle = ResourceHandle(1001);
pub const FORWARD_COLOR_H: ResourceHandle = ResourceHandle(1002);
/// PT (path tracing) 输出 颜色 — replaces FORWARD_COLOR_H when
/// `RenderSettings.render_mode == PathTrace`. Written by PathTracePass
/// as a storage+sampled image, read by PostPass for tonemapping.
pub const PT_COLOR_H: ResourceHandle = ResourceHandle(1003);
/// Directional light depth shadow map — written by `ShadowMapPass` and
/// sampled in `ForwardPass` fragment shader for shadowing.
pub const SHADOW_MAP_H: ResourceHandle = ResourceHandle(1004);
/// TAA-resolved HDR color (#883). Written by `TaaPass`, read by `PostPass`
/// instead of `FORWARD_COLOR_H` when `RenderSettings::taa` is enabled.
pub const TAA_COLOR_H: ResourceHandle = ResourceHandle(1005);
/// Per-pixel screen-space motion vectors (#883 Phase 2). Written by
/// `ForwardPass` (SV_Target2) and sampled by `TaaPass` for object motion.
pub const FORWARD_VELOCITY_H: ResourceHandle = ResourceHandle(1006);
/// ANF-upscaled full-resolution HDR color (#1161). Written by `AnfPass`,
/// read by `PostPass` instead of `FORWARD_COLOR_H` / `TAA_COLOR_H` when ANF is active.
pub const ANF_COLOR_H: ResourceHandle = ResourceHandle(1007);
/// Path-tracer NDC depth ([0,1], same convention as the raster depth buffer).
/// Written by `PathTracePass` as a storage+sampled image and consumed by
/// `TaaPass` as the reprojection guide while `render_mode == PathTrace`.
pub const PT_DEPTH_H: ResourceHandle = ResourceHandle(1008);
/// Path-tracer screen-space motion vectors in UV units (same convention as
/// `FORWARD_VELOCITY_H`: `current_uv - previous_uv`). Written by
/// `PathTracePass` and consumed by `TaaPass`.
pub const PT_VELOCITY_H: ResourceHandle = ResourceHandle(1009);
/// Path-tracer surface guides for the neural denoisers (DLSS-RR / NRD):
/// view-space unit normal in xyz and perceptual roughness in w, matching the
/// raster G-buffer's SV_Target1 convention but without the reactive mask.
/// Written by `PathTracePass` as an RGBA16F storage+sampled image.
pub const PT_NORMAL_ROUGHNESS_H: ResourceHandle = ResourceHandle(1010);
/// Raster G-buffer surface data for screen-space GI: RGB = base color
/// (albedo), A = perceptual roughness. Written by `ForwardPass` as an extra
/// SV_Target3 attachment and read by `RestirGiPass` to demodulate the
/// secondary bounce, so the GI signal is lighting only.
pub const FORWARD_ALBEDO_H: ResourceHandle = ResourceHandle(1011);
/// Screen-space indirect diffuse produced by `RestirGiPass`. Written as a
/// storage+sampled image at the internal resolution, read by `ForwardPass`
/// next frame (1-frame latency, same handshake as the GTAO visibility buffer).
pub const RESTIR_GI_COLOR_H: ResourceHandle = ResourceHandle(1012);
/// SHaRC hash-map entries (one `u64` per voxel slot). Written by the
/// `SharcTrain` node and read by `SharcResolve`.
///
/// The buffers are owned by `GraphRenderer` (they outlive several passes and
/// are recreated on a capacity change). The handle exists so the training and
/// resolve stages show up as declared graph edges instead of unrecorded work
/// recorded between passes (repair spec §4).
pub const SHARC_HASH_ENTRIES_H: ResourceHandle = ResourceHandle(1013);
/// SHaRC per-voxel accumulation counters (`uint4` per slot), written by
/// `SharcTrain` and normalised by `SharcResolve`.
pub const SHARC_ACCUMULATION_H: ResourceHandle = ResourceHandle(1014);
/// SHaRC resolved radiance cache (`float16_t4` radiance + sample data).
/// Written by `SharcResolve`, sampled by the path tracer and ReSTIR GI.
pub const SHARC_RESOLVED_H: ResourceHandle = ResourceHandle(1015);
/// Screen-space ambient occlusion visibility texture (R8_UNORM).
/// Written by `GtaoPass`, sampled by `ForwardPass` next frame.
pub const GTAO_AO_H: ResourceHandle = ResourceHandle(1016);
/// Post-processed tonemapped final color output image (R8G8B8A8_UNORM).
pub const FINAL_COLOR_H: ResourceHandle = ResourceHandle(1017);
/// PRT64 resolved order-2 SH GI volume (64^3 * 9 layers, RGBA32F), written by
/// `PrtPass` and sampled by `ForwardPass` next frame (1-frame latency).
pub const PRT_GI_VOLUME_H: ResourceHandle = ResourceHandle(1018);
/// HDR color composite produced by `VoxelDdaPass` when voxel chunks are active (R16G16B16A16_SFLOAT).
pub const VOXEL_COLOR_H: ResourceHandle = ResourceHandle(1019);
/// Linearized NDC depth composite produced by `VoxelDdaPass` (R32_SFLOAT).
pub const VOXEL_DEPTH_H: ResourceHandle = ResourceHandle(1020);
/// Full-resolution HDR color upscaled by `MobileUpscalerPass` (SGSR 2 / Arm ASR).
pub const UPSCALED_COLOR_H: ResourceHandle = ResourceHandle(1021);
/// Full-resolution HDR color upscaled/denoised by `DlssPass` (Streamline DLSS-SR / DLSS-RR).
pub const DLSS_COLOR_H: ResourceHandle = ResourceHandle(1022);

impl ResourceHandle {
    pub const INVALID: ResourceHandle = ResourceHandle(u32::MAX);
}

/// Resource type for graph-managed attachments.
#[derive(Clone, Debug)]
pub enum ResourceType {
    /// Color attachment (GBuffer layer, HDR output, etc.)
    ColorAttachment {
        format: vk::Format,
        extent: vk::Extent2D,
        sample_count: vk::SampleCountFlags,
    },
    /// Depth/stencil attachment
    DepthAttachment {
        extent: vk::Extent2D,
        sample_count: vk::SampleCountFlags,
    },
    /// Storage image (compute pass output, read by later passes)
    StorageImage {
        format: vk::Format,
        extent: vk::Extent3D,
    },
    /// Storage buffer (SHARC hash/accumulation/resolved buffers)
    StorageBuffer { size: u64 },
}

/// Description of a resource a pass needs — either reads from or writes to.
#[derive(Clone, Debug)]
pub struct ResourceUsage {
    pub handle: ResourceHandle,
    pub access: vk::AccessFlags,
    pub stage: vk::PipelineStageFlags,
    pub layout: vk::ImageLayout,
}

/// Direction of a declared resource edge. Read edges cause the graph to
/// transition the image into `usage.layout` (with src from the last writer);
/// write edges record the layout the pass leaves the image in (via its render
/// pass `final_layout`), so the next reader's barrier knows the true
/// `old_layout`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    Read,
    Write,
}

/// One declared resource access (a read or write edge) for dependency
/// resolution and automatic barrier insertion.
#[derive(Clone, Debug)]
pub struct ResourceEdge {
    pub pass_idx: usize,
    pub usage: ResourceUsage,
    pub kind: EdgeKind,
}

/// Per-resource lifecycle span `[first_write_pass, last_read_pass]`, computed
/// at build time. Currently only surfaced to the visualizer; reserved as input
/// for future TBDR 内存 aliasing (已实现).
#[derive(Clone, Debug, Default)]
pub struct ResourceLifecycle {
    pub first_write: Option<usize>,
    pub last_read: Option<usize>,
}

impl ResourceLifecycle {
    /// Fold a single edge into the span.
    pub fn update(&mut self, e: &ResourceEdge) {
        match e.kind {
            EdgeKind::Write => {
                self.first_write = Some(self.first_write.map_or(e.pass_idx, |w| w.min(e.pass_idx)));
            }
            EdgeKind::Read => {
                self.last_read = Some(self.last_read.map_or(e.pass_idx, |r| r.max(e.pass_idx)));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Read-only snapshots for render-graph visualizers / diagnostics.
//
// The engine-side viz must not borrow `RenderGraph` (its passes are private
// trait objects) nor 触摸 `vk::*` handles inside the UI 闭包 These
// plain-data structs are produced by `RenderGraph::snapshot` + each pass's
// `RenderPassNode::graph_info`, cloned once per frame, and consumed by the UI.
// ---------------------------------------------------------------------------

/// Coarse classification of a pass for visualization (coloring / iconography).
/// Kept in sync with the concrete pass structs that override `graph_info`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PassKind {
    /// Rasterized depth-only shadow map (`ShadowMapPass`).
    Shadow,
    /// 向前 PBR scene 渲染 (`ForwardPass`).
    Forward,
    /// Half-resolution screen-space ambient occlusion (`GtaoPass`).
    Gtao,
    /// Fullscreen tonemap / present (`PostPass`).
    Post,
    /// Real-time path tracing compute pass (`PathTracePass`).
    Pt,
    /// 离屏 RenderTexture 渲染 (`RenderToTexturePass`).
    Rt,
    /// Qualcomm Adreno Neural Fusion compute pass (`AnfPass`).
    Anf,
    /// Open-source mobile temporal upscaler compute pass (`MobileUpscalerPass`).
    MobileUpscaler,
    /// NVIDIA Streamline DLSS-SR / DLSS-RR upscaler pass (`DlssPass`).
    Dlss,
    /// Screen-space ReSTIR indirect diffuse (`RestirGiPass`).
    RestirGi,
    /// Standalone SHaRC trainer (`SharcTrainPass`): deposits radiance into the
    /// world-space cache at its own resolution and writes no screen output.
    SharcTrain,
    /// SHaRC accumulation resolve (`SharcResolveNode`).
    SharcResolve,
    /// Three-tier sparse voxel compute DDA raymarcher (`VoxelDdaPass`).
    Voxel,
    /// Unrecognized pass future / experimental).
    #[default]
    Unknown,
}

/// Static description of one graph-managed resource for the visualizer.
/// Mirrors the relevant subset of [`GraphResource`] without exposing Vulkan
/// handles.
#[derive(Clone, Debug)]
pub struct ResourceInfo {
    pub handle: ResourceHandle,
    pub res_type: ResourceType,
    /// `true` once `allocate_resources` has created the backing image.
    pub allocated: bool,
}

/// Static description of one pass for the visualizer: its declared resource
/// edges (`inputs` = handles it reads via `GraphResources::published_view`,
/// `outputs` = handles it publishes) plus a coarse kind for coloring.
///
/// Side-inputs that bypass the graph (shadow view, IBL set, previous-frame AO
/// bound via `set_ao`) are NOT listed here - they are surfaced as human-readable
/// notes by the viz instead, since they don't flow through `GraphResources`.
#[derive(Clone, Debug)]
pub struct PassInfo {
    /// Execution index (filled in by `RenderGraph::snapshot`).
    pub index: usize,
    pub name: String,
    pub kind: PassKind,
    /// Resource handles this pass reads from upstream passes.
    pub inputs: Vec<ResourceHandle>,
    /// Resource handles this pass publishes for downstream passes.
    pub outputs: Vec<ResourceHandle>,
}

/// A complete read-only snapshot of the render graph: passes in execution
/// order, the resource table, and the active settings. Produced per-frame by
/// [`RenderGraph::snapshot`].
#[derive(Clone, Debug)]
pub struct RenderGraphSnapshot {
    pub passes: Vec<PassInfo>,
    pub resources: Vec<ResourceInfo>,
    pub settings: RenderSettings,
}

/// Per-frame scene + lighting state shared with every pass via [`RenderContext`].
///
/// The `GraphRenderer` populates this once per frame (before driving the
/// graph) with the camera/light UBO, the draw list, and the light-space
/// view-projection used by both the shadow pass and the lighting pass
pub struct GraphFrame<'a> {
    /// Per-frame UBO (camera + light). Its descriptor set is bound at set 0.
    pub frame_ubo: &'a FrameUBO,
    /// Camera layers composited in `Camera.depth` order (#461). Empty means
    /// "one implicit camera from `FrameUBOData`", which keeps the legacy
    /// single-camera path bit-identical.
    pub cameras: &'a [CameraView],
    /// Draw list for the current frame.
    pub draw_list: &'a [DrawItem],
    /// ECS-authored gizmos for this frame.
    pub gizmos: &'a [crate::gizmo::GizmoInstance],
    /// ECS-authored 2D sprites for this frame (sorted back-to-front).
    pub sprite_items: &'a [SpriteDrawItem],
    /// `Light2D` entities for this frame, strongest first (#611).
    pub sprite_lights: &'a [SpriteLight],
    /// World-space particle billboards for this frame (sorted back-to-front).
    pub particle_items: &'a [ParticleDrawItem],
    /// Bone palette for skinned draws (#560 P0-3); indexed by
    /// [`DrawItem::bone_offset`]. Empty when nothing is skinned.
    pub bone_matrices: &'a [[[f32; 4]; 4]],
    /// ECS-authored skybox parameters (`None` = skip the sky draw).
    pub skybox: Option<crate::skybox_pass::SkyboxSettings>,
    /// Mesh manager — passes resolve [`DrawItem::mesh`] handles to GPU buffers.
    pub mesh_manager: &'a RenderMeshManager,
    /// Light-space view-projection (orthographic) used by the shadow pass and
    /// by the lighting pass to project world positions into the shadow map.
    pub light_view_proj: [[f32; 4]; 4],
    /// Far-cascade light-space VP (#526 CSM); ignored when
    /// `shadow_cascade_count <= 1`.
    pub light_view_proj1: [[f32; 4]; 4],
    /// Camera-forward distance of the near/far cascade split.
    pub shadow_split: f32,
    /// 1 = single shadow map, 2 = two-cascade atlas.
    pub shadow_cascade_count: u32,
    /// Effective shadow mode for this frame (after capability resolution).
    pub shadow_mode: ShadowMode,
    /// PBR component toggle bitmask (15 bits, see `scene_frag.slang`
    /// `PBR_FLAG_*`). 0 = all components neutral (raw baseColor). Forwarded
    /// to the bindless push constant `debug_flags` field.
    pub debug_flags: u32,
    /// 前向管线线框模式（#431）。设备不支持 `fillModeNonSolid` 时忽略。
    pub wireframe: bool,
    /// 正常填充 + 线框叠加（#431）。`wireframe` 为 true 时以纯线框为准。
    pub shaded_wireframe: bool,
    /// 需要画选中描边的 ECS 实体 id（#487）。
    ///
    /// 由编辑器通过 [`GraphRenderer::set_outline_entities`] 每帧设置；空切片
    /// 表示没有选中物体。`ForwardPass` 只对 `DrawItem::entity_id` 命中的绘制
    /// 项画反向外壳。
    pub outline_entities: &'a [u32],
    /// Inverse-view rotation (upper-left 3x3 of inverse(view)), packed as mat4.
    /// Used by the skybox pass to rotate view-space look directions into world
    /// space. Because the view matrix is a rigid transform, this is just the
    /// transpose of the upper-left 3x3 of `view` (the rotation basis), with w=0
    /// on the 4th row.
    pub inv_view_rot: [[f32; 4]; 4],
    /// Camera world-to-view matrix (`FrameUBOData::view`).
    pub view: [[f32; 4]; 4],
    /// Full world-space view-projection (clip = proj * view), including the
    /// surface rotation. Used by the world-space gizmo (drawn on top of the
    /// scene) so the axes track the camera.
    pub view_proj: [[f32; 4]; 4],
    /// Subpixel jitter applied to `view_proj` this frame, in NDC units
    /// (#883). Zero when TAA is off; `TaaPass` subtracts it to recover the
    /// unjittered camera matrix for history reprojection.
    pub view_proj_jitter: [f32; 2],
    /// Point lights collected from the ECS this frame, rewritten into the
    /// scene shader's 光源 SSBO. Forwarded to `ForwardPass::execute` so it can
    /// update its descriptor set without `GraphRenderer` poking it directly.
    pub lights: &'a [GpuLight],
    /// Previous-frame GTAO 可见性 视图 (1-frame 延迟 `ForwardPass`
    /// binds this as its AO input; it reads `ao[(frame + 1) % 2]` written by
    /// `GtaoPass` last frame. Forwarded via `GraphFrame` so the graph, not
    /// `GraphRenderer`, owns the cross-pass wiring.
    pub ao_view: vk::ImageView,
    /// Previous-frame ReSTIR GI indirect diffuse (1-frame latency: the forward
    /// pass reads the result `RestirGiPass` wrote last frame). `null` means GI
    /// is off, and the forward pass falls back to the baked probe volume.
    pub gi_view: vk::ImageView,
    /// Previous-frame PRT64 `set 5` descriptor set (`probeVolume` + `probeInfo`),
    /// bound by `ForwardPass` when `gi_mode == GiMode::Prt` (1-frame latency).
    pub prt_gi_descriptor_set: vk::DescriptorSet,
    /// Tonemap mode for `PostPass` (Reinhard / ACES / ...). Forwarded so
    /// `PostPass::execute` reads it from the graph context.
    pub tonemap_mode: u32,
    /// Post-process anti-aliasing mode for `PostPass` (0 = off, 1 = FXAA).
    /// Requires no extra targets: FXAA runs inside the tonemap pass, so the
    /// offscreen (editor viewport) and swapchain paths get the same result.
    pub aa_mode: u32,
    /// PostPass debug render-target viewer (Tab key). 0 = normal tonemapped
    /// HDR, 1 = linearized depth, 2 = view-space normal. PostPass picks which
    /// published view to sample based on this.
    pub debug_rt: u32,
    /// Display-referred color grading, applied in `PostPass` after tonemapping
    /// (#502). [`PostGrade::default`] is an exact no-op.
    pub grade: PostGrade,
    /// Projection matrix entries `[2][2]` / `[3][2]` (column-major
    /// `m[col][row]`) used by PostPass to linearize the depth buffer for the
    /// debug depth view (`view_z = proj22 * d + proj32`).
    pub proj22: f32,
    pub proj32: f32,
    /// Inverse projection (used by `GtaoPass` to reconstruct view-space
    /// radius from screen-space samples). Forwarded via `GraphFrame`.
    pub inv_projection: [[f32; 4]; 4],
    /// Swapchain image views for the current frame. Forwarded so `PostPass`
    /// can (re)build its per-swapchain-image framebuffers inside `execute`
    /// (mirroring `ForwardPass::ensure_target`), instead of relying on
    /// `GraphRenderer` to call `set_target` every frame.
    pub swapchain_views: &'a [vk::ImageView],
    /// Swapchain or offscreen target images for the current frame. Forwarded
    /// so passes like `PostPass` can publish `FINAL_COLOR_H`.
    pub swapchain_images: &'a [vk::Image],
    /// When true, freezes temporal accumulation, jitter, and history ping-ponging.
    pub freeze_temporal: bool,
    /// Active render mode (Raster vs PathTrace). Passes check this to decide
    /// whether to run (ForwardPass skips in PT 众数 PathTracePass skips in
    /// Raster mode).
    pub render_mode: RenderMode,
    /// Path tracing max bounces.
    pub pt_max_bounces: u32,
    /// Max world-space length of PT primary + shadow rays. Smaller values cut
    /// long-range bounces (and distant shadow casters), useful as an artistic
    /// "fog"/focus control and to limit cost on huge scenes.
    pub pt_ray_max_distance: f32,
    /// Maximum iterations (samples per pixel). 0 = accumulate forever.
    pub pt_max_iterations: u32,
    /// Camera world-space position [x, y, z, light_count] from the frame UBO.
    pub camera_pos: [f32; 4],
    /// Light direction [x, y, z, intensity] from the frame UBO.
    pub light_dir: [f32; 4],
    /// Light color [r, g, b, ambient] from the frame UBO. Forwarded to the
    /// path-trace pass so it can apply the scene's actual sun color instead
    /// of a hardcoded white (the rasterizer reads this via the FrameUBO).
    pub light_color: [f32; 4],
    /// Exposure multiplier applied to the final HDR color before tonemapping.
    /// Forwarded from [`FrameInput`](crate::graph_renderer::FrameInput) so both
    /// ForwardPass (via FrameUBOData) and PathTracePass (via 推送 常量 apply
    /// the same exposure value from the camera entity.
    pub exposure: f32,
    /// Analytic lights for path tracing (point/spot/area/directional).
    /// Written into the PT lights SSBO for multi-light NEE.
    pub pt_lights: &'a [PtAnalyticLight],
    /// When `true`, the path tracer should reset its accumulation next frame.
    /// Set when directional-light properties change.
    pub pt_accum_dirty: bool,
    /// Whether a usable Camera entity was found. When `false`, the skybox and
    /// PT pass should be skipped so the clear color (gray) shows through.
    pub has_camera: bool,
    /// Whether the active camera is orthographic; the PT pass casts parallel
    /// primary rays in that case (issue #10).
    pub camera_orthographic: bool,
    /// 清空 颜色 for the scene 颜色 附件 Applied by ForwardPass on
    /// render-pass begin; visible when the skybox and scene geometry are
    /// absent or transparent (e.g. no-camera fallback). Matches the app-level
    /// `clear_color` parameter passed to `render_system`.
    pub clear_color: [f32; 4],
    /// 是否在场景渲染中绘制地面参考网格。
    pub show_grid: bool,
    /// 统一脏分派计划（DESIGN §9 / #1157）。
    pub dispatch_plan: Option<&'a crate::dirty_dispatch::DirtyDispatchPlan>,
}

/// Context passed to each pass's `execute`.
pub struct RenderContext<'a> {
    pub device: &'a ash::Device,
    pub context: &'a VulkanContext,
    pub settings: &'a RenderSettings,
    pub cmd: vk::CommandBuffer,
    pub frame_index: u32,
    /// Swapchain image index returned by `acquire_next_image`. Distinct from
    /// `frame_index` (which is the frame-in-flight index): with N swapchain
    /// images and 2 frames in flight, `frame_index` cycles 0..2 while
    /// `image_index` cycles 0..N. Passes that own per-swapchain-image resources
    /// (e.g. `ForwardPass`'s framebuffers) 索引 by this, not `frame_index`.
    pub image_index: u32,
    /// Current swapchain extent.
    pub extent: vk::Extent2D,
    /// 场景实际渲染的 extent（内部渲染分辨率，#613）。
    ///
    /// 没有开启内部分辨率时与 [`Self::extent`] 相同。`ForwardPass` / 路径追踪
    /// 按这个尺寸分配目标；`PostPass` / UI 叠加 / 截图仍用 `extent`。
    pub render_extent: vk::Extent2D,
    /// Per-frame scene + lighting state (see [`GraphFrame`]).
    pub frame: &'a GraphFrame<'a>,
}

/// Trait for a modular render pass.
///
/// Each pass declares its resource needs via [`setup`] and records commands
/// in [`execute`]. The graph calls these in topological order.
pub trait RenderPassNode: std::any::Any {
    /// Human-readable name (for debugging / profiling).
    fn name(&self) -> &str;

    /// Declare resource reads/writes. Called once during graph compilation.
    /// The pass should register its needs via `graph.create_resource(...)` /
    /// `graph.read(...)` / `graph.write(...)`. `settings` is the runtime
    /// render configuration (e.g. `gbuffer_high_precision`) so the pass
    /// can pick the right format for its attachments.
    fn setup(&mut self, graph: &mut RenderGraphBuilder, settings: &RenderSettings);

    /// Record Vulkan commands into `ctx.cmd`. `resources` is mutable so the
    /// pass can publish its output views (depth / normal / HDR) for downstream
    /// passes to read by handle.
    fn execute(&mut self, ctx: &RenderContext, resources: &mut GraphResources) -> Result<()>;

    /// Pre‑create pipelines / shader modules at load time so the first frame
    /// doesn't pay pipeline‑compilation cost. Default is a no‑op; passes with
    /// lazy pipelines override this.
    fn warmup(
        &mut self,
        _device: &ash::Device,
        _context: &crate::context::VulkanContext,
    ) -> Result<()> {
        Ok(())
    }

    /// Read-only snapshot of this pass's declared resource edges + coarse kind,
    /// for the render-graph visualizer. Default returns an "unknown" pass with
    /// no edges; concrete passes override to populate `kind`/`inputs`/`outputs`.
    ///
    /// The execution `index` is filled in by [`RenderGraph::snapshot`] (the pass
    /// does not know its own position); implementations should leave it as
    /// `usize::MAX` or `0`.
    fn graph_info(&self) -> PassInfo {
        PassInfo {
            index: usize::MAX,
            name: self.name().to_string(),
            kind: PassKind::Unknown,
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Resource state tracker — automatic barrier derivation
// ---------------------------------------------------------------------------

/// Per-resource GPU layout + access + stage snapshot for auto-barrier logic.
#[derive(Clone, Copy, Debug)]
pub struct ResourceState {
    pub layout: vk::ImageLayout,
    pub access: vk::AccessFlags,
    pub stage: vk::PipelineStageFlags,
}

/// Tracks per-`(handle, image_index)` resource state across passes and frames.
/// Used by `RenderGraph::execute` to derive automatic barriers between passes.
/// `reset()` clears all state (called after swapchain recreation).
#[derive(Clone, Debug)]
pub struct ResourceStateTracker {
    states: HashMap<(ResourceHandle, u32), ResourceState>,
}

impl ResourceStateTracker {
    pub fn new() -> Self {
        Self {
            states: HashMap::new(),
        }
    }

    /// Look up current state; returns UNDEFINED / empty / TOP_OF_PIPE when
    /// unknown (initial-transition semantics).
    pub fn get(&self, handle: ResourceHandle, image_index: u32) -> ResourceState {
        self.states
            .get(&(handle, image_index))
            .copied()
            .unwrap_or(ResourceState {
                layout: vk::ImageLayout::UNDEFINED,
                access: vk::AccessFlags::empty(),
                stage: vk::PipelineStageFlags::TOP_OF_PIPE,
            })
    }

    /// Record that a resource is now in the given state (called after a pass's
    /// render pass transitions it via `final_layout`).
    pub fn set(
        &mut self,
        handle: ResourceHandle,
        image_index: u32,
        layout: vk::ImageLayout,
        access: vk::AccessFlags,
        stage: vk::PipelineStageFlags,
    ) {
        self.states.insert(
            (handle, image_index),
            ResourceState {
                layout,
                access,
                stage,
            },
        );
    }

    /// Batch-apply write edges from a pass: each write edge records the layout
    /// the pass leaves the resource in.
    pub fn apply_writes(&mut self, edges: &[ResourceEdge], image_index: u32) {
        for e in edges {
            if e.kind == EdgeKind::Write {
                self.set(
                    e.usage.handle,
                    image_index,
                    e.usage.layout,
                    e.usage.access,
                    e.usage.stage,
                );
            }
        }
    }

    /// Build barriers for all read edges whose tracked layout differs from the
    /// declared usage. Returns `(barriers, src_stage, dst_stage)`; callers
    /// should emit a single `vkCmdPipelineBarrier` with the union of stages.
    /// Skips resources already in the desired 布局 or 待发布.
    ///
    /// Each emitted barrier also updates the tracked layout to the reader's
    /// `usage.layout`, so a second reader of the same resource in the same
    /// frame sees the post-transition layout as its `old_layout` instead of a
    /// stale pre-transition value (which would trip VUID-oldLayout-01197).
    pub fn build_read_barriers(
        &mut self,
        edges: &[ResourceEdge],
        last_writers: &HashMap<ResourceHandle, ResourceUsage>,
        image_index: u32,
        resources: &GraphResources,
    ) -> (
        Vec<vk::ImageMemoryBarrier<'_>>,
        vk::PipelineStageFlags,
        vk::PipelineStageFlags,
    ) {
        let mut barriers: Vec<vk::ImageMemoryBarrier> = Vec::new();
        let mut max_src_stage = vk::PipelineStageFlags::empty();
        let mut max_dst_stage = vk::PipelineStageFlags::empty();

        for re in edges {
            if re.kind != EdgeKind::Read {
                continue;
            }
            let handle = re.usage.handle;
            let current = self.get(handle, image_index);
            if current.layout == re.usage.layout {
                continue;
            }

            let image = resources
                .published_image(handle)
                .or_else(|| resources.image(handle));
            let image = match image {
                Some(img) => img,
                None => continue,
            };

            let (src_access, src_stage) = match last_writers.get(&handle) {
                Some(w) => (w.access, w.stage),
                None => (
                    vk::AccessFlags::empty(),
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                ),
            };

            let aspect = aspect_mask_for_layout(re.usage.layout);
            barriers.push(
                vk::ImageMemoryBarrier::default()
                    .old_layout(current.layout)
                    .new_layout(re.usage.layout)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(image)
                    .src_access_mask(src_access)
                    .dst_access_mask(re.usage.access)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: aspect,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    }),
            );
            max_src_stage |= src_stage;
            max_dst_stage |= re.usage.stage;

            // Advance the tracked state to the post-barrier layout, matching
            // the GPU transition we just recorded. This must happen per-read,
            // not per-write: a layout change is driven by the read edge here,
            // and `apply_writes` only runs after the pass executes.
            self.set(
                handle,
                image_index,
                re.usage.layout,
                re.usage.access,
                re.usage.stage,
            );
        }

        (barriers, max_src_stage, max_dst_stage)
    }

    /// Clear all tracked state (swapchain recreation).
    pub fn reset(&mut self) {
        self.states.clear();
    }
}

impl Default for ResourceStateTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// A resource entry in the graph's resource table.
#[derive(Clone)]
pub struct GraphResource {
    pub handle: ResourceHandle,
    pub res_type: ResourceType,
    /// Owning Vulkan image (None until allocated).
    pub image: Option<vk::Image>,
    pub image_view: Option<vk::ImageView>,
    pub memory: Option<vk::DeviceMemory>,
}

/// Resource table passed to passes at execute time.
/// Besides the graph-owned images (allocated in `allocate_resources`), it
/// carries **pass-exported views + images** (e.g. `ForwardPass` publishes its
/// depth / normal / HDR views AND images here so downstream passes like
/// `GtaoPass` / `PostPass` can read them by handle). This is the minimal
/// graph-edge resource handoff for PR-1: the graph does not own the
/// underlying images (passes still create their own framebuffers), but it is
/// the channel through which passes exchange resource handles instead of
/// `GraphRenderer` poking each pass
pub struct GraphResources {
    pub resources: HashMap<ResourceHandle, GraphResource>,
    /// Pass-published image views, keyed by `ResourceHandle`.
    pub image_views: HashMap<ResourceHandle, vk::ImageView>,
    /// Pass-published images (handles), keyed by `ResourceHandle`. Needed by
    /// downstream passes that emit layout barriers (which reference the image,
    /// not the view).
    pub images: HashMap<ResourceHandle, vk::Image>,
    /// Pass-published scalar values (e.g. bindless slots), keyed by
    /// `ResourceHandle`. Generic handle handoff for producers → consumers
    /// that don't map to a Vulkan handle (see `RenderToTexturePass` → the
    /// `TextureHandle` it publishes under `RT_OUTPUT_H`).
    pub params: HashMap<ResourceHandle, u32>,
}

impl GraphResources {
    pub fn new() -> Self {
        Self {
            resources: HashMap::new(),
            image_views: HashMap::new(),
            images: HashMap::new(),
            params: HashMap::new(),
        }
    }

    /// Clear per-frame published views, images, and params for the next frame.
    /// Retains the underlying capacity of the HashMaps to avoid per-frame allocations.
    pub fn clear_frame(&mut self) {
        self.image_views.clear();
        self.images.clear();
        self.params.clear();
    }

    pub fn image(&self, h: ResourceHandle) -> Option<vk::Image> {
        self.resources.get(&h).and_then(|r| r.image)
    }

    pub fn image_view(&self, h: ResourceHandle) -> Option<vk::ImageView> {
        self.resources.get(&h).and_then(|r| r.image_view)
    }

    /// Publish an image view under a handle so downstream passes can read it.
    pub fn set_image_view(&mut self, h: ResourceHandle, view: vk::ImageView) {
        self.image_views.insert(h, view);
    }

    /// Publish an image under a handle (for downstream layout barriers).
    pub fn set_image(&mut self, h: ResourceHandle, image: vk::Image) {
        self.images.insert(h, image);
    }

    /// Read a view published by an upstream pass.
    pub fn published_view(&self, h: ResourceHandle) -> Option<vk::ImageView> {
        self.image_views.get(&h).copied()
    }

    /// Read an image published by an upstream pass.
    pub fn published_image(&self, h: ResourceHandle) -> Option<vk::Image> {
        self.images.get(&h).copied()
    }

    /// 发布 a scalar value under a handle (e.g. a bindless slot) so downstream
    /// passes can 读取 it without knowing the producer's internals.
    pub fn set_param(&mut self, h: ResourceHandle, value: u32) {
        self.params.insert(h, value);
    }

    /// 读取 a scalar published by an upstream pass.
    pub fn param(&self, h: ResourceHandle) -> Option<u32> {
        self.params.get(&h).copied()
    }
}

impl Default for GraphResources {
    fn default() -> Self {
        Self::new()
    }
}

/// Groups edges by their execution `pass_idx` into preallocated buckets.
fn build_pass_edges(passes_count: usize, edges: &[ResourceEdge]) -> Vec<Vec<ResourceEdge>> {
    let mut buckets: Vec<Vec<ResourceEdge>> = vec![Vec::new(); passes_count];
    for e in edges {
        if e.pass_idx < buckets.len() {
            buckets[e.pass_idx].push(e.clone());
        }
    }
    buckets
}

// ---------------------------------------------------------------------------
// Graph builder — collects passes and resource declarations, then compiles.
// ---------------------------------------------------------------------------

pub struct RenderGraphBuilder {
    passes: Vec<Box<dyn RenderPassNode + 'static>>,
    resources: HashMap<ResourceHandle, GraphResource>,
    /// Declared read/write edges collected from passes' `setup`. Indexed by
    /// `pass_idx` (= `pass_idx_offset + passes.len()` at the moment
    /// `read_usage`/`write_usage` is called during setup, before `add_pass`
    /// pushes the pass). The offset is non-zero when this builder is a
    /// temporary created by `RenderGraph::add_pass` to set up a pass that will
    /// be appended to an already-populated graph.
    edges: Vec<ResourceEdge>,
    next_handle: u32,
    settings: RenderSettings,
    /// Index of the first pass this builder will register, in the final
    /// graph's pass list. Zero for a fresh builder; set to
    /// `RenderGraph::passes.len()` by `RenderGraph::add_pass` so edges declared
    /// during a pass's `setup` get the correct absolute `pass_idx`.
    pass_idx_offset: usize,
}

impl RenderGraphBuilder {
    pub fn new() -> Self {
        Self {
            passes: Vec::new(),
            resources: HashMap::new(),
            edges: Vec::new(),
            next_handle: 0,
            settings: RenderSettings::default(),
            pass_idx_offset: 0,
        }
    }

    /// Set the base pass index for edges declared via this builder. Used by
    /// [`RenderGraph::add_pass`] so a pass appended to an already-built graph
    /// records edges with its true absolute `pass_idx` rather than 0.
    pub fn pass_idx_offset(mut self, offset: usize) -> Self {
        self.pass_idx_offset = offset;
        self
    }

    /// Override the render settings used when `setup` is called on passes.
    pub fn settings(mut self, settings: &RenderSettings) -> Self {
        self.settings = settings.clone();
        self
    }

    /// Register a pass. Order of insertion = execution order (simple
    /// linear pipeline for now; topological sort can be added later).
    pub fn add_pass(&mut self, pass: Box<dyn RenderPassNode + 'static>) {
        self.passes.push(pass);
    }

    /// Create a transient resource managed by the graph.
    pub fn create_resource(&mut self, res_type: ResourceType) -> ResourceHandle {
        let handle = ResourceHandle(self.next_handle);
        self.next_handle += 1;
        self.resources.insert(
            handle,
            GraphResource {
                handle,
                res_type,
                image: None,
                image_view: None,
                memory: None,
            },
        );
        handle
    }

    /// Create a transient resource at a specific handle (e.g. a well-known
    /// graph-edge handle like `FORWARD_DEPTH_H`). Used so downstream passes can
    /// reference a publisher's output without knowing its internal field.
    pub fn create_resource_at(&mut self, handle: ResourceHandle, res_type: ResourceType) {
        self.resources.insert(
            handle,
            GraphResource {
                handle,
                res_type,
                image: None,
                image_view: None,
                memory: None,
            },
        );
    }

    /// Mark the pass currently being set up (i.e. the next one `add_pass`
    /// will push, at index `self.passes.len()`) as reading a resource, with
    /// the access/stage/layout it reads with. The graph uses this to insert a
    /// `vkCmdPipelineBarrier` before the pass when the image's current layout
    /// differs from `usage.layout`.
    ///
    /// Must be called from within `RenderPassNode::setup` (before `add_pass`
    /// pushes the pass calling it elsewhere panics.
    pub fn read_usage(&mut self, usage: ResourceUsage) {
        self.push_edge(usage, EdgeKind::Read);
    }

    /// Mark the pass currently being set up as writing a resource, with the
    /// access/stage/layout it leaves the image in (typically the render pass
    /// `final_layout`). The graph records this as the resource's current
    /// layout after the pass executes, so the next reader's barrier knows the
    /// true `old_layout`. No barrier is emitted for the write itself (the
    /// pass's render pass performs the layout transition implicitly).
    pub fn write_usage(&mut self, usage: ResourceUsage) {
        self.push_edge(usage, EdgeKind::Write);
    }

    fn push_edge(&mut self, usage: ResourceUsage, kind: EdgeKind) {
        self.edges.push(ResourceEdge {
            pass_idx: self.pass_idx_offset + self.passes.len(),
            usage,
            kind,
        });
    }

    /// Mark a pass (by index) as reading a resource.
    /// Tracked for future barrier generation and topological sort.
    pub fn read(&mut self, pass_idx: usize, handle: ResourceHandle) {
        // Future: push to a dependency list for barrier generation
        let _ = (pass_idx, handle);
    }

    /// Mark a pass (by index) as writing a resource.
    /// Tracked for future barrier generation and topological sort.
    pub fn write(&mut self, pass_idx: usize, handle: ResourceHandle) {
        let _ = (pass_idx, handle);
    }

    /// Compile into an executable graph.
    pub fn build(self) -> RenderGraph {
        let lifecycles = compute_lifecycles(&self.edges);
        let pass_edges = build_pass_edges(self.passes.len(), &self.edges);
        let last_writers = build_last_writers(&self.edges);
        let g = RenderGraph {
            passes: self.passes,
            graph_resources: GraphResources {
                resources: self.resources,
                image_views: HashMap::new(),
                images: HashMap::new(),
                params: HashMap::new(),
            },
            settings: self.settings,
            edges: self.edges,
            pass_edges,
            last_writers,
            state_tracker: ResourceStateTracker::new(),
            lifecycles,
            last_barrier_probe: Instant::now(),
            pass_dump: crate::pass_dump::PassDump::from_env(),
            debug_control: crate::render_debug::RenderDebugControl::from_env(),
            hdr_readback_requested: false,
            pending_hdr_readback: None,
        };
        g.validate_edges();
        g
    }
}

/// Compute the `[first_write, last_read]` span per resource from the declared
/// edges. Used by the visualizer and reserved as input for future TBDR memory
/// aliasing (no aliasing is performed today).
fn compute_lifecycles(edges: &[ResourceEdge]) -> HashMap<ResourceHandle, ResourceLifecycle> {
    let mut map: HashMap<ResourceHandle, ResourceLifecycle> = HashMap::new();
    for e in edges {
        map.entry(e.usage.handle).or_default().update(e);
    }
    map
}

impl Default for RenderGraphBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Executable graph
// ---------------------------------------------------------------------------

pub struct RenderGraph {
    passes: Vec<Box<dyn RenderPassNode + 'static>>,
    graph_resources: GraphResources,
    settings: RenderSettings,
    /// Declared read/write edges, collected from each pass's `setup`.
    edges: Vec<ResourceEdge>,
    /// Precomputed pass_idx -> edges bucket list for fast traversal without per-frame cloning.
    pass_edges: Vec<Vec<ResourceEdge>>,
    /// Precomputed handle -> last writer's usage for barrier src stage/access.
    last_writers: HashMap<ResourceHandle, ResourceUsage>,
    /// Per-`(handle, image_index)` resource state tracker, persisted across
    /// frames so cross-frame reads (e.g. GTAO's double-buffered AO) keep their
    /// 布局 Keyed by `image_index` because `ForwardPass`/`PostPass` own
    /// per-swapchain-image attachments under the same handle.
    state_tracker: ResourceStateTracker,
    /// `[first_write, last_read]` span per resource, for the visualizer and
    /// future aliasing.
    lifecycles: HashMap<ResourceHandle, ResourceLifecycle>,
    /// Last time the `BARRIER_PROBE` trace lines were emitted; throttled to
    /// once per second so the log isn't flooded at frame rate.
    last_barrier_probe: Instant,
    /// `PRISMA_PASS_DUMP` state; a no-op unless the env var is set.
    pass_dump: crate::pass_dump::PassDump,
    /// Diagnostic controls for pass bypass, pessimistic barrier, and temporal freeze.
    debug_control: crate::render_debug::RenderDebugControl,
    /// Set by [`Self::request_hdr_readback`]; consumed at the end of
    /// [`Self::execute_with_timer`], which records a copy of the path tracer's
    /// linear output into a host-visible staging buffer.
    hdr_readback_requested: bool,
    /// The recorded copy, waiting for the frame fence before it can be mapped.
    pending_hdr_readback: Option<crate::hdr_readback::PendingHdrReadback>,
}

impl RenderGraph {
    /// Borrow a registered pass by concrete type (for lifecycle operations
    /// like `recreate_swapchain`, which must call into a specific pass).
    /// Returns `None` if no pass of that type was registered.
    pub fn pass_mut<T: RenderPassNode + 'static>(&mut self) -> Option<&mut T> {
        self.passes
            .iter_mut()
            .find_map(|p| (&mut **p as &mut dyn std::any::Any).downcast_mut::<T>())
    }

    /// Immutable borrow of a registered pass by concrete type. The read-only
    /// counterpart to [`pass_mut`](Self::pass_mut); used by the render-graph
    /// visualizer to pull live per-pass state (extent / format / image_count)
    /// without mutating the graph.
    pub fn pass_ref<T: RenderPassNode + 'static>(&self) -> Option<&T> {
        self.passes
            .iter()
            .find_map(|p| (&**p as &dyn std::any::Any).downcast_ref::<T>())
    }

    /// Active render settings (feature knobs consulted by passes at execute
    /// time). Exposed read-only for the visualizer's header summary.
    pub fn settings(&self) -> &RenderSettings {
        &self.settings
    }

    /// Call [`warmup`](RenderPassNode::warmup) on every registered pass.
    /// Designed to be called once after graph construction so that lazy
    /// pipelines are compiled ahead of the first frame.
    pub fn warmup_passes(
        &mut self,
        device: &ash::Device,
        context: &crate::context::VulkanContext,
    ) -> Result<()> {
        for pass in &mut self.passes {
            pass.warmup(device, context)?;
        }
        Ok(())
    }

    /// Iterator over all declared graph resources (depth/color attachments,
    /// storage images). Exposed read-only for the visualizer.
    pub fn resources(&self) -> impl Iterator<Item = &GraphResource> {
        self.graph_resources.resources.values()
    }

    /// Look up a graph-managed resource by handle (immutable).
    pub fn resource(&self, h: ResourceHandle) -> Option<&GraphResource> {
        self.graph_resources.resources.get(&h)
    }

    /// Build a complete read-only snapshot of the graph for the visualizer:
    /// passes in execution order (with `index` filled in), the resource table,
    /// and the active settings. Cheap to call per-frame - clones only the
    /// small declarative metadata, never Vulkan handles.
    pub fn snapshot(&self) -> RenderGraphSnapshot {
        let passes = self
            .passes
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut info = p.graph_info();
                info.index = i;
                info
            })
            .collect();
        let resources = self
            .graph_resources
            .resources
            .values()
            .map(|r| ResourceInfo {
                handle: r.handle,
                res_type: r.res_type.clone(),
                allocated: r.image.is_some(),
            })
            .collect();
        RenderGraphSnapshot {
            passes,
            resources,
            settings: self.settings.clone(),
        }
    }

    /// Recomputes the pass edge bucket cache and last writer mapping when the graph
    /// topology changes (e.g. at build time or after `add_pass`).
    fn rebuild_topology_cache(&mut self) {
        self.pass_edges = build_pass_edges(self.passes.len(), &self.edges);
        self.last_writers = build_last_writers(&self.edges);
    }

    /// 追加 a pass to an already-built 图 (e.g. ForwardPass / GtaoPass /
    /// PostPass, registered after the shadow map's resources are allocated so
    /// the scene can bind the shadow view). Runs `setup` on the new pass
    /// (merging its declared resources into the graph) and appends it to the
    /// execution order.
    pub fn add_pass(&mut self, mut pass: Box<dyn RenderPassNode + 'static>) {
        let pass_idx = self.passes.len();
        let mut b = RenderGraphBuilder::new()
            .settings(&self.settings)
            .pass_idx_offset(pass_idx);
        pass.setup(&mut b, &self.settings);
        for (h, r) in b.resources {
            self.graph_resources.resources.insert(h, r);
        }
        // Edges were recorded with pass_idx = offset + b.passes.len() == pass_idx
        // (b is a fresh builder with no passes pushed); sanity-check before merging.
        for mut e in b.edges {
            debug_assert_eq!(e.pass_idx, pass_idx);
            e.pass_idx = pass_idx;
            self.lifecycles
                .entry(e.usage.handle)
                .or_default()
                .update(&e);
            self.edges.push(e);
        }
        self.passes.push(pass);
        self.rebuild_topology_cache();
    }

    /// Drop all cached image layouts. Called after `recreate_swapchain` (where
    /// every per-swapchain-image attachment is rebuilt) so stale layout state
    /// doesn't suppress the first-frame barriers.
    pub fn reset_layouts(&mut self) {
        self.state_tracker.reset();
    }

    /// Validate declared edges: warn on reads before writes (potential
    /// cross-frame / ordering issue) and log an error on dependency cycles.
    /// Execution order is never reordered (the registration order in
    /// `GraphRenderer::new` reflects physical dependencies).
    fn validate_edges(&self) {
        use std::collections::HashSet;
        // Per-handle write-before-read check.
        let mut last_write: HashMap<ResourceHandle, usize> = HashMap::new();
        for e in &self.edges {
            match e.kind {
                EdgeKind::Write => {
                    last_write.insert(e.usage.handle, e.pass_idx);
                }
                EdgeKind::Read => match last_write.get(&e.usage.handle) {
                    Some(w) if *w > e.pass_idx => {
                        log::warn!(
                            "render-graph: pass {} reads {:?} before pass {} writes it \
                             (cross-frame dependency? ensure manual barriers cover this)",
                            e.pass_idx,
                            e.usage.handle,
                            w
                        );
                    }
                    _ => {}
                },
            }
        }
        // Cycle detection: pass A -> pass B if A writes a handle B reads.
        let n = self.passes.len();
        let mut adj: Vec<HashSet<usize>> = vec![HashSet::new(); n];
        for e in &self.edges {
            if e.kind == EdgeKind::Read {
                if let Some(w) = last_write.get(&e.usage.handle) {
                    if *w != e.pass_idx {
                        adj[*w].insert(e.pass_idx);
                    }
                }
            }
        }
        let mut color = vec![0u8; n]; // 0=white,1=gray,2=black
        let mut has_cycle = false;
        fn dfs(u: usize, adj: &[HashSet<usize>], color: &mut [u8], has_cycle: &mut bool) {
            color[u] = 1;
            for &v in &adj[u] {
                match color[v] {
                    1 => {
                        *has_cycle = true;
                    }
                    0 => dfs(v, adj, color, has_cycle),
                    _ => {}
                }
            }
            color[u] = 2;
        }
        for s in 0..n {
            if color[s] == 0 {
                dfs(s, &adj, &mut color, &mut has_cycle);
            }
        }
        if has_cycle {
            log::error!("render-graph: dependency cycle detected among passes");
        }
    }

    /// Run all registered passes in order, recording into `ctx.cmd`.
    ///
    /// Before each pass, the graph inspects that pass's declared **read** edges
    /// and emits a `vkCmdPipelineBarrier` per resource whose cached layout
    /// differs from the read's `usage.layout`. The barrier's `src` stage/access
    /// come from the last **write** edge on that handle (the pass that left the
    /// image in its current layout); if no writer is known, `TOP_OF_PIPE` /
    /// empty access is used (initial-transition semantics). After each pass,
    /// its **write** edges update the cached layout (no barrier emitted - the
    /// pass's own render pass performs that transition via `final_layout`).
    ///
    /// Note: cross-frame reads (e.g. GTAO's double-buffered AO fed back to
    /// `ForwardPass`) and the 交换链 `-> PRESENT_SRC_KHR` 过渡 are NOT
    /// graph edges and remain manual (see the pass-level comments). The layout
    /// cache only tracks the four graph-flow handles (shadow / scene depth /
    /// normal / HDR color).
    pub fn execute(&mut self, ctx: &RenderContext) -> Result<()> {
        self.execute_with_timer(ctx, None)
    }

    /// 同上，但记录每个 pass 的 GPU 时间戳（#518）。
    ///
    /// `timer` 为 `Some` 时，pass 首尾各写一个时间戳；时间戳池由调用方拥有，
    /// 结果也要等下一帧再由调用方读回（避免 stall）。
    pub fn execute_with_timer(
        &mut self,
        ctx: &RenderContext,
        mut timer: Option<&mut crate::gpu_timer::GpuTimer>,
    ) -> Result<()> {
        self.graph_resources.clear_frame();

        // `PRISMA_PASS_DUMP`: decide once per frame whether to snapshot the
        // published attachments at the end. A no-op unless the env var is set.
        let capture = self.pass_dump.should_capture();

        // Throttle the BARRIER_PROBE trace lines to once per second so the log
        // isn't flooded at frame rate. The probe is a debugging aid for the
        // automatic barrier pipeline; gating it here avoids passing `Instant`
        // state through the free functions.
        let probe = if self.last_barrier_probe.elapsed().as_secs_f32() >= 1.0 {
            self.last_barrier_probe = Instant::now();
            true
        } else {
            false
        };

        for (pass_idx, pass) in self.passes.iter_mut().enumerate() {
            if self.debug_control.is_pass_bypassed(pass.name()) {
                log::debug!("RenderGraph: bypassing pass '{}'", pass.name());
                continue;
            }

            if self.debug_control.pessimistic_barrier {
                let mem_barrier = vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
                    .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE);
                unsafe {
                    ctx.device.cmd_pipeline_barrier(
                        ctx.cmd,
                        vk::PipelineStageFlags::ALL_COMMANDS,
                        vk::PipelineStageFlags::ALL_COMMANDS,
                        vk::DependencyFlags::empty(),
                        &[mem_barrier],
                        &[],
                        &[],
                    );
                }
            }

            // Build and emit barriers for this pass's read edges, then
            // update the tracker with write edges after the pass executes.
            let (barriers, src_stage, dst_stage) = self.state_tracker.build_read_barriers(
                &self.pass_edges[pass_idx],
                &self.last_writers,
                ctx.image_index,
                &self.graph_resources,
            );
            if !barriers.is_empty() {
                let src_stage = if src_stage.is_empty() {
                    vk::PipelineStageFlags::TOP_OF_PIPE
                } else {
                    src_stage
                };
                let dst_stage = if dst_stage.is_empty() {
                    vk::PipelineStageFlags::BOTTOM_OF_PIPE
                } else {
                    dst_stage
                };
                unsafe {
                    ctx.device.cmd_pipeline_barrier(
                        ctx.cmd,
                        src_stage,
                        dst_stage,
                        vk::DependencyFlags::empty(),
                        &[],
                        &[],
                        &barriers,
                    );
                }
            }

            if probe {
                for e in &self.pass_edges[pass_idx] {
                    if e.kind == EdgeKind::Read {
                        let st = self.state_tracker.get(e.usage.handle, ctx.image_index);
                        log::trace!(
                            "BARRIER_PROBE pass {} {} {:?}: tracked={:?} desired={:?} image_index={}",
                            pass_idx,
                            if e.kind == EdgeKind::Read { "read" } else { "write" },
                            e.usage.handle,
                            st.layout,
                            e.usage.layout,
                            ctx.image_index
                        );
                    }
                }
            }

            let timed = timer
                .as_deref_mut()
                .map(|timer| timer.begin_pass(ctx.device, ctx.cmd, ctx.frame_index, pass.name()))
                .unwrap_or(false);
            let pass_start = Instant::now();
            pass.execute(ctx, &mut self.graph_resources)?;
            let pass_ms = pass_start.elapsed().as_secs_f64() * 1000.0;
            if pass_ms >= SLOW_PASS_MS {
                log::debug!(
                    target: "startup",
                    "[startup] render_graph pass '{}' {pass_ms:.3}ms",
                    pass.name()
                );
            }
            if timed {
                if let Some(timer) = timer.as_deref_mut() {
                    timer.end_pass(ctx.device, ctx.cmd, ctx.frame_index);
                }
            }

            if self.debug_control.pessimistic_barrier {
                let mem_barrier = vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
                    .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE);
                unsafe {
                    ctx.device.cmd_pipeline_barrier(
                        ctx.cmd,
                        vk::PipelineStageFlags::ALL_COMMANDS,
                        vk::PipelineStageFlags::ALL_COMMANDS,
                        vk::DependencyFlags::empty(),
                        &[mem_barrier],
                        &[],
                        &[],
                    );
                }
            }

            self.state_tracker
                .apply_writes(&self.pass_edges[pass_idx], ctx.image_index);
        }

        if capture {
            self.dump_published_images(ctx);
            self.pass_dump.mark_captured();
        }

        // Linear HDR readback (repair spec §7.2): same slot as the pass dump, so
        // the copy sees the frame's final image after every pass ran. A frame
        // without a published `PT_COLOR_H` is not an error - the path tracer is
        // skipped for raster frames, and the caller can simply ask again.
        if self.hdr_readback_requested {
            self.hdr_readback_requested = false;
            if let Err(err) = self.record_hdr_readback(ctx) {
                log::warn!("hdr readback: {err}");
            }
        }

        Ok(())
    }

    /// Copy the graph's published color attachments into host-visible buffers
    /// for `PRISMA_PASS_DUMP` (see [`crate::pass_dump`]).
    ///
    /// Runs after every pass has executed, so each image is in the layout the
    /// state tracker recorded for its last write.
    fn dump_published_images(&mut self, ctx: &RenderContext) {
        // (handle, name, format, kind)
        let targets = [
            (
                FORWARD_COLOR_H,
                "forward_color",
                vk::Format::R16G16B16A16_SFLOAT,
                crate::pass_dump::DumpKind::Radiance,
            ),
            (
                FORWARD_NORMAL_H,
                "forward_normal",
                vk::Format::R16G16B16A16_SFLOAT,
                crate::pass_dump::DumpKind::Normal,
            ),
            (
                FORWARD_DEPTH_H,
                "forward_depth",
                vk::Format::D32_SFLOAT,
                crate::pass_dump::DumpKind::Depth,
            ),
            (
                FORWARD_ALBEDO_H,
                "forward_albedo",
                vk::Format::R16G16B16A16_SFLOAT,
                crate::pass_dump::DumpKind::Albedo,
            ),
            (
                FORWARD_VELOCITY_H,
                "forward_velocity",
                vk::Format::R16G16B16A16_SFLOAT,
                crate::pass_dump::DumpKind::Velocity,
            ),
            (
                GTAO_AO_H,
                "gtao_ao",
                vk::Format::R8_UNORM,
                crate::pass_dump::DumpKind::Grayscale,
            ),
            (
                RESTIR_GI_COLOR_H,
                "restir_gi_color",
                vk::Format::R16G16B16A16_SFLOAT,
                crate::pass_dump::DumpKind::Radiance,
            ),
            (
                TAA_COLOR_H,
                "taa_color",
                vk::Format::R16G16B16A16_SFLOAT,
                crate::pass_dump::DumpKind::Radiance,
            ),
            (
                ANF_COLOR_H,
                "anf_color",
                vk::Format::R16G16B16A16_SFLOAT,
                crate::pass_dump::DumpKind::Radiance,
            ),
            (
                UPSCALED_COLOR_H,
                "upscaled_color",
                vk::Format::R16G16B16A16_SFLOAT,
                crate::pass_dump::DumpKind::Radiance,
            ),
            (
                PT_COLOR_H,
                "pt_color",
                vk::Format::R32G32B32A32_SFLOAT,
                crate::pass_dump::DumpKind::Radiance,
            ),
            (
                PT_DEPTH_H,
                "pt_depth",
                vk::Format::R32_SFLOAT,
                crate::pass_dump::DumpKind::Depth,
            ),
            (
                PT_VELOCITY_H,
                "pt_velocity",
                vk::Format::R32G32_SFLOAT,
                crate::pass_dump::DumpKind::Velocity,
            ),
            (
                PT_NORMAL_ROUGHNESS_H,
                "pt_normal_roughness",
                vk::Format::R16G16B16A16_SFLOAT,
                crate::pass_dump::DumpKind::Normal,
            ),
            (
                FINAL_COLOR_H,
                "final_color",
                vk::Format::R8G8B8A8_UNORM,
                crate::pass_dump::DumpKind::RawRgba8,
            ),
        ];

        for (handle, name, format, kind) in targets {
            let Some(path) = self.pass_dump.path_for(name) else {
                continue;
            };
            let Some(image) = self.graph_resources.published_image(handle) else {
                log::trace!("pass dump: {name} was not published this frame");
                continue;
            };
            let default_extent = if handle == FINAL_COLOR_H {
                ctx.extent
            } else {
                ctx.render_extent
            };
            let (format, extent) = self
                .graph_resources
                .resources
                .get(&handle)
                .map(|r| match r.res_type {
                    ResourceType::ColorAttachment {
                        format: fmt,
                        extent,
                        ..
                    } => {
                        // ForwardPass (and other passes that manage their own textures)
                        // registers a 1x1 placeholder extent during setup. Fall back
                        // to the frame render extent when the declared extent is <= 1.
                        let ext = if extent.width > 1 && extent.height > 1 {
                            extent
                        } else {
                            default_extent
                        };
                        (fmt, ext)
                    }
                    ResourceType::DepthAttachment { extent, .. } => {
                        let ext = if extent.width > 1 && extent.height > 1 {
                            extent
                        } else {
                            default_extent
                        };
                        (format, ext)
                    }
                    _ => (format, default_extent),
                })
                .unwrap_or((format, default_extent));

            if extent.width == 0 || extent.height == 0 {
                log::warn!("pass dump: {name} has zero extent ({extent:?}), skipping");
                continue;
            }

            let state = self.state_tracker.get(handle, ctx.image_index);
            let layout = if state.layout == vk::ImageLayout::UNDEFINED {
                if format == vk::Format::D32_SFLOAT {
                    vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL
                } else {
                    vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
                }
            } else {
                state.layout
            };
            let access = if state.access.is_empty() {
                if format == vk::Format::D32_SFLOAT {
                    vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE
                } else {
                    vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                }
            } else {
                state.access
            };

            if let Err(err) =
                self.record_dump(ctx, name, image, extent, format, kind, layout, access, path)
            {
                log::warn!("pass dump: {name} failed: {err}");
            }
        }

        // The shadow atlas is graph-owned (ShadowMapPass does not publish it)
        // and its extent is the atlas size, not the render extent.
        if let Some(image) = self.graph_resources.image(SHADOW_MAP_H) {
            let atlas_extent = self
                .graph_resources
                .resources
                .get(&SHADOW_MAP_H)
                .and_then(|r| match r.res_type {
                    ResourceType::DepthAttachment { extent, .. } => Some(extent),
                    _ => None,
                });
            if let (Some(extent), Some(path)) =
                (atlas_extent, self.pass_dump.path_for("shadow_map"))
            {
                let name = "shadow_map";
                let layout = vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL;
                let access = vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE;
                if let Err(err) = self.record_dump(
                    ctx,
                    name,
                    image,
                    extent,
                    vk::Format::D32_SFLOAT,
                    crate::pass_dump::DumpKind::Depth,
                    layout,
                    access,
                    path,
                ) {
                    log::warn!("pass dump: {name} failed: {err}");
                }
            } else {
                log::warn!("pass dump: shadow_map has no atlas extent");
            }
        } else {
            log::warn!("pass dump: shadow_map was not allocated this frame");
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record_dump(
        &mut self,
        ctx: &RenderContext,
        name: &str,
        image: vk::Image,
        extent: vk::Extent2D,
        format: vk::Format,
        kind: crate::pass_dump::DumpKind,
        old_layout: vk::ImageLayout,
        old_access: vk::AccessFlags,
        path: std::path::PathBuf,
    ) -> anyhow::Result<()> {
        use crate::pass_dump::image_bytes;

        let (buffer, memory) = crate::buffer::create_buffer(
            ctx.context,
            image_bytes(extent, format),
            vk::BufferUsageFlags::TRANSFER_DST,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;

        let aspect = match format {
            vk::Format::D32_SFLOAT
            | vk::Format::D16_UNORM
            | vk::Format::D32_SFLOAT_S8_UINT
            | vk::Format::D24_UNORM_S8_UINT => vk::ImageAspectFlags::DEPTH,
            _ => vk::ImageAspectFlags::COLOR,
        };
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(aspect)
            .base_mip_level(0)
            .level_count(1)
            .base_array_layer(0)
            .layer_count(1);

        let to_transfer = vk::ImageMemoryBarrier::default()
            .old_layout(old_layout)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(range)
            .src_access_mask(old_access)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ);

        // Put the image back so the recorded layout still matches reality.
        let restore = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .new_layout(old_layout)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(range)
            .src_access_mask(vk::AccessFlags::TRANSFER_READ)
            .dst_access_mask(old_access);

        let region = vk::BufferImageCopy::default()
            .buffer_offset(0)
            .buffer_row_length(0)
            .buffer_image_height(0)
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(aspect)
                    .mip_level(0)
                    .base_array_layer(0)
                    .layer_count(1),
            )
            .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
            .image_extent(vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            });

        unsafe {
            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_transfer],
            );
            ctx.device.cmd_copy_image_to_buffer(
                ctx.cmd,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer,
                &[region],
            );
            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[restore],
            );
        }

        self.pass_dump.push(crate::pass_dump::PendingDump {
            buffer,
            memory,
            extent,
            format,
            kind,
            path,
        });
        log::info!(
            "pass dump: captured {name} ({}x{})",
            extent.width,
            extent.height
        );
        Ok(())
    }

    /// Decode pending dumps into PNGs. Call after the frame fence signalled.
    pub fn flush_pass_dump(&mut self, device: &ash::Device) -> anyhow::Result<()> {
        self.pass_dump.flush(device)
    }

    /// Release pending dump buffers without writing them.
    pub fn discard_pass_dump(&mut self, device: &ash::Device) {
        self.pass_dump.discard(device);
    }

    /// Programmatically configure pass dump for automated tests or headless runs.
    pub fn configure_pass_dump(
        &mut self,
        dir: std::path::PathBuf,
        target_frame: u64,
        filter: Option<Vec<String>>,
    ) {
        self.pass_dump = crate::pass_dump::PassDump::new(dir, target_frame, filter);
    }

    /// Access the diagnostic debug control settings (immutable).
    pub fn debug_control(&self) -> &crate::render_debug::RenderDebugControl {
        &self.debug_control
    }

    /// Access the diagnostic debug control settings (mutable).
    pub fn debug_control_mut(&mut self) -> &mut crate::render_debug::RenderDebugControl {
        &mut self.debug_control
    }

    /// Replace the diagnostic debug control settings.
    pub fn set_debug_control(&mut self, control: crate::render_debug::RenderDebugControl) {
        self.debug_control = control;
    }

    /// Ask the next [`Self::execute_with_timer`] to copy the path tracer's linear
    /// output (`PT_COLOR_H`, `R32G32B32A32_SFLOAT`) into a host-visible buffer.
    ///
    /// The copy happens after every pass ran, so it captures the current frame's
    /// converged radiance rather than a partially-written accum image. The result
    /// is retrieved with [`Self::take_hdr_readback`] once the frame fence has
    /// signalled. Requesting twice before taking replaces the earlier request.
    pub fn request_hdr_readback(&mut self) {
        self.hdr_readback_requested = true;
    }

    /// Whether a linear readback was requested but the buffer has not been
    /// recorded yet (i.e. the frame has not executed since the request).
    pub fn hdr_readback_requested(&self) -> bool {
        self.hdr_readback_requested
    }

    /// Map and return the pending linear readback, if any.
    ///
    /// Call this **after** the frame fence signalled - `GraphRenderer::present`
    /// already waits for it in headless mode, and windowed frames wait inside
    /// `end_frame`. Returns `None` when no frame executed since the request, or
    /// when the path tracer did not publish `PT_COLOR_H` that frame.
    pub fn take_hdr_readback(
        &mut self,
        device: &ash::Device,
    ) -> anyhow::Result<Option<crate::hdr_readback::HdrReadback>> {
        match self.pending_hdr_readback.take() {
            Some(pending) => Ok(Some(pending.take(device)?)),
            None => Ok(None),
        }
    }

    /// Drop a pending linear readback without mapping it.
    pub fn discard_hdr_readback(&mut self, device: &ash::Device) {
        if let Some(pending) = self.pending_hdr_readback.take() {
            pending.destroy(device);
        }
        self.hdr_readback_requested = false;
    }

    /// Record the copy for a requested linear readback.
    ///
    /// Same shape as [`Self::record_dump`], minus the PNG encode: the graph hands
    /// the raw f32 texels back to the host. The image is put back into the layout
    /// the state tracker recorded, so a following frame's barriers stay valid.
    fn record_hdr_readback(&mut self, ctx: &RenderContext) -> anyhow::Result<()> {
        use crate::pass_dump::image_bytes;

        let Some(image) = self.graph_resources.published_image(PT_COLOR_H) else {
            anyhow::bail!("hdr readback: PT_COLOR_H was not published this frame");
        };
        if self.pending_hdr_readback.is_some() {
            // Previous frame's buffer was never taken; drop it so the map does
            // not leak and the newest frame wins.
            self.discard_hdr_readback(ctx.device);
        }

        let format = vk::Format::R32G32B32A32_SFLOAT;
        let extent = ctx.render_extent;
        let (buffer, memory) = crate::buffer::create_buffer(
            ctx.context,
            image_bytes(extent, format),
            vk::BufferUsageFlags::TRANSFER_DST,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;

        let state = self.state_tracker.get(PT_COLOR_H, ctx.image_index);
        let old_layout = if state.layout == vk::ImageLayout::UNDEFINED {
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
        } else {
            state.layout
        };
        let old_access = if state.access.is_empty() {
            vk::AccessFlags::SHADER_WRITE
        } else {
            state.access
        };

        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .base_mip_level(0)
            .level_count(1)
            .base_array_layer(0)
            .layer_count(1);

        let to_transfer = vk::ImageMemoryBarrier::default()
            .old_layout(old_layout)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(range)
            .src_access_mask(old_access)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ);
        // Put the image back so the recorded layout still matches reality.
        let restore = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .new_layout(old_layout)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(range)
            .src_access_mask(vk::AccessFlags::TRANSFER_READ)
            .dst_access_mask(old_access);

        let region = vk::BufferImageCopy::default()
            .buffer_offset(0)
            .buffer_row_length(0)
            .buffer_image_height(0)
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .mip_level(0)
                    .base_array_layer(0)
                    .layer_count(1),
            )
            .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
            .image_extent(vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            });

        unsafe {
            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_transfer],
            );
            ctx.device.cmd_copy_image_to_buffer(
                ctx.cmd,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer,
                &[region],
            );
            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[restore],
            );
        }

        self.pending_hdr_readback = Some(crate::hdr_readback::PendingHdrReadback {
            buffer,
            memory,
            extent,
        });
        log::trace!("hdr readback: recorded {}x{}", extent.width, extent.height);
        Ok(())
    }

    /// Allocate (or re-use) Vulkan resources for all declared graph resources.
    /// Called once at startup or when the graph topology changes.
    pub fn allocate_resources(
        &mut self,
        device: &ash::Device,
        mem_props: &vk::PhysicalDeviceMemoryProperties,
    ) -> Result<()> {
        for res in self.graph_resources.resources.values_mut() {
            if res.image.is_some() {
                continue; // already allocated
            }
            match &res.res_type {
                ResourceType::ColorAttachment {
                    format,
                    extent,
                    sample_count,
                } => {
                    let (image, view, memory) = create_transient_image(
                        device,
                        mem_props,
                        *format,
                        *extent,
                        *sample_count,
                        vk::ImageUsageFlags::COLOR_ATTACHMENT
                            | vk::ImageUsageFlags::INPUT_ATTACHMENT
                            | vk::ImageUsageFlags::STORAGE,
                    )?;
                    res.image = Some(image);
                    res.image_view = Some(view);
                    res.memory = Some(memory);
                }
                ResourceType::DepthAttachment {
                    extent,
                    sample_count,
                } => {
                    let (image, view, memory) = create_transient_image(
                        device,
                        mem_props,
                        vk::Format::D32_SFLOAT,
                        *extent,
                        *sample_count,
                        vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
                            | vk::ImageUsageFlags::INPUT_ATTACHMENT
                            | vk::ImageUsageFlags::SAMPLED
                            | vk::ImageUsageFlags::TRANSFER_SRC,
                    )?;
                    res.image = Some(image);
                    res.image_view = Some(view);
                    res.memory = Some(memory);
                }
                ResourceType::StorageImage { format, extent } => {
                    let (image, view, memory) = create_transient_image(
                        device,
                        mem_props,
                        *format,
                        vk::Extent2D {
                            width: extent.width,
                            height: extent.height,
                        },
                        vk::SampleCountFlags::TYPE_1,
                        vk::ImageUsageFlags::STORAGE
                            | vk::ImageUsageFlags::TRANSFER_DST
                            | vk::ImageUsageFlags::SAMPLED,
                    )?;
                    res.image = Some(image);
                    res.image_view = Some(view);
                    res.memory = Some(memory);
                }
                ResourceType::StorageBuffer { size: _ } => {
                    // Buffers allocated on demand by the pass that owns them
                    // (e.g. SHARC buffers are created in SharcPass::setup).
                }
            }
        }
        Ok(())
    }

    /// Look up a graph-managed image view by resource handle.
    /// Returns `None` if the handle does not exist or the resource has no view.
    pub fn image_view(&self, h: ResourceHandle) -> Option<vk::ImageView> {
        self.graph_resources.image_view(h)
    }

    /// Destroy all owned Vulkan resources.
    pub fn destroy(&mut self, device: &ash::Device) {
        for res in self.graph_resources.resources.values() {
            unsafe {
                if let Some(view) = res.image_view {
                    device.destroy_image_view(view, None);
                }
                if let Some(image) = res.image {
                    device.destroy_image(image, None);
                }
                if let Some(mem) = res.memory {
                    device.free_memory(mem, None);
                }
            }
        }
        self.graph_resources.resources.clear();

        self.discard_hdr_readback(device);
    }
}

impl Drop for RenderGraph {
    fn drop(&mut self) {
        // Resources should be destroyed explicitly via destroy().
        // If not, they leak — we can't call device destroy in Drop without
        // holding a device reference. This is intentional: the owner of the
        // graph (Renderer/Engine) must call destroy() before dropping.
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Pick the image aspect mask for a layout transition barrier: depth/stencil
/// layouts use the DEPTH aspect (this project uses D32_SFLOAT, no separate
/// stencil), all color/sample/storage layouts use COLOR.
fn aspect_mask_for_layout(layout: vk::ImageLayout) -> vk::ImageAspectFlags {
    match layout {
        vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL
        | vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL
        | vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL
        | vk::ImageLayout::DEPTH_READ_ONLY_OPTIMAL
        | vk::ImageLayout::STENCIL_ATTACHMENT_OPTIMAL
        | vk::ImageLayout::STENCIL_READ_ONLY_OPTIMAL => vk::ImageAspectFlags::DEPTH,
        _ => vk::ImageAspectFlags::COLOR,
    }
}

/// Build a `handle -> &ResourceUsage` map of the last writer (highest pass_idx
/// that writes the handle). Readers use this to fill a barrier's src
/// stage/access; a handle with no writer uses `TOP_OF_PIPE` / empty access
/// (initial-transition semantics).
fn build_last_writers(edges: &[ResourceEdge]) -> HashMap<ResourceHandle, ResourceUsage> {
    let mut map: HashMap<ResourceHandle, (usize, &ResourceUsage)> = HashMap::new();
    for e in edges {
        if e.kind == EdgeKind::Write {
            match map.get(&e.usage.handle) {
                Some((prev_idx, _)) if *prev_idx >= e.pass_idx => {}
                _ => {
                    map.insert(e.usage.handle, (e.pass_idx, &e.usage));
                }
            }
        }
    }
    map.into_iter().map(|(h, (_, u))| (h, u.clone())).collect()
}

/// Create a graph-owned 2D image and bind memory for it.
///
/// Lazy (`LAZILY_ALLOCATED`) memory is only legal for images created with
/// `TRANSIENT_ATTACHMENT`, and such an image must not also be `SAMPLED` or
/// `STORAGE`. Every resource this graph allocates today is consumed by other
/// passes (color -> post, depth/shadow -> sampled), so they all end up in
/// `DEVICE_LOCAL`: a sampled depth image in a TBDR lazy heap produces
/// validation errors and undefined reads.
fn create_transient_image(
    device: &ash::Device,
    mem_props: &vk::PhysicalDeviceMemoryProperties,
    format: vk::Format,
    extent: vk::Extent2D,
    sample_count: vk::SampleCountFlags,
    usage: vk::ImageUsageFlags,
) -> anyhow::Result<(vk::Image, vk::ImageView, vk::DeviceMemory)> {
    let flags = vk::ImageCreateFlags::empty();
    let _ = flags; // suppress unused warning

    let image_create_info = vk::ImageCreateInfo {
        image_type: vk::ImageType::TYPE_2D,
        format,
        extent: vk::Extent3D {
            width: extent.width,
            height: extent.height,
            depth: 1,
        },
        mip_levels: 1,
        array_layers: 1,
        samples: sample_count,
        tiling: vk::ImageTiling::OPTIMAL,
        usage,
        sharing_mode: vk::SharingMode::EXCLUSIVE,
        initial_layout: vk::ImageLayout::UNDEFINED,
        ..Default::default()
    };

    let image = unsafe { device.create_image(&image_create_info, None) }?;
    let req = unsafe { device.get_image_memory_requirements(image) };

    // For real transient attachments, prefer LAZILY_ALLOCATED 内存 类型.
    let lazy = lazy_allocation_allowed(usage);
    debug_assert!(
        !lazy
            || !(usage.contains(vk::ImageUsageFlags::SAMPLED)
                || usage.contains(vk::ImageUsageFlags::STORAGE)),
        "LAZILY_ALLOCATED requires TRANSIENT_ATTACHMENT without SAMPLED/STORAGE"
    );
    let mem_type = if lazy {
        find_memory_type(
            mem_props,
            req.memory_type_bits,
            vk::MemoryPropertyFlags::LAZILY_ALLOCATED,
        )
        .or_else(|| {
            // Fallback to device-local if no lazy type available (non-TBDR GPU)
            find_memory_type(
                mem_props,
                req.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
        })
        .ok_or_else(|| anyhow::anyhow!("no suitable memory type for transient image"))?
    } else {
        find_memory_type(
            mem_props,
            req.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .ok_or_else(|| anyhow::anyhow!("no suitable memory type for storage image"))?
    };

    let memory = unsafe {
        device.allocate_memory(
            &vk::MemoryAllocateInfo {
                allocation_size: req.size,
                memory_type_index: mem_type,
                ..Default::default()
            },
            None,
        )
    }?;
    unsafe { device.bind_image_memory(image, memory, 0) }?;

    let aspect = if format == vk::Format::D32_SFLOAT {
        vk::ImageAspectFlags::DEPTH
    } else {
        vk::ImageAspectFlags::COLOR
    };

    let view = unsafe {
        device.create_image_view(
            &vk::ImageViewCreateInfo {
                image,
                view_type: vk::ImageViewType::TYPE_2D,
                format,
                subresource_range: vk::ImageSubresourceRange {
                    aspect_mask: aspect,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                },
                ..Default::default()
            },
            None,
        )
    }?;

    Ok((image, view, memory))
}

/// Whether `usage` permits backing the image with `LAZILY_ALLOCATED` memory.
///
/// Vulkan only allows that memory for `TRANSIENT_ATTACHMENT` images, and the
/// same image may not be `SAMPLED`/`STORAGE` (so its contents need never be
/// written back to the heap).
fn lazy_allocation_allowed(usage: vk::ImageUsageFlags) -> bool {
    usage.contains(vk::ImageUsageFlags::TRANSIENT_ATTACHMENT)
        && !usage.contains(vk::ImageUsageFlags::SAMPLED)
        && !usage.contains(vk::ImageUsageFlags::STORAGE)
}

fn find_memory_type(
    mem_props: &vk::PhysicalDeviceMemoryProperties,
    type_filter: u32,
    flags: vk::MemoryPropertyFlags,
) -> Option<u32> {
    (0..mem_props.memory_type_count).find(|&i| {
        (type_filter & (1 << i)) != 0
            && mem_props.memory_types[i as usize]
                .property_flags
                .contains(flags)
    })
}

#[cfg(test)]
#[path = "../render_graph_tests.rs"]
mod tests;
