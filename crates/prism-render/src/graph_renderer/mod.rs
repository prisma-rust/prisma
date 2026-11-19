//! 基于 RenderGraph 的渲染器驱动程序。
//!
//! [`GraphRenderer`] 取代了旧版渲染器。它拥有 Vulkan 上下文、
//! 交换链、命令池 + 每帧命令缓冲区、帧 UBO、IBL 资源以及三个场景管理器
//!（网格、纹理、材质）。它构建包含 [`ShadowMapPass`] 和 [`ForwardPass`]
//! 的 [`RenderGraph`]，每帧执行并呈现到交换链。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use ash::vk;

use crate::context::VulkanContext;
use crate::descriptor::{FrameUBOData, GpuLight, PtAnalyticLight};
use crate::external_overlay::{OverlayMessage, SwapchainOverlay};
use crate::forward_pass::ForwardPass;
use crate::ibl::IblResources;

use crate::managers::{MeshHandle, RenderMaterialManager, RenderMeshManager, RenderTextureManager};
use crate::offscreen::OffscreenTarget;
#[cfg(feature = "pathtrace")]
use crate::pt_pass::PathTracePass;
/// `ShadowMapPass::setup` 是 trait 方法，只有注册阴影 pass 时才需要导入。
#[cfg(feature = "shadows")]
use crate::render_graph::RenderPassNode;
use crate::render_graph::{
    DrawItem, GraphFrame, RenderGraph, RenderGraphBuilder, RenderMode, RenderSettings, ShadowMode,
};
use crate::scene_scope::SceneScope;
use crate::swapchain::Swapchain;

/// One resolved draw for the bindless PBR path. The engine pre-resolves asset
/// handles into render-side mesh handles + material SSBO slots and hands the
/// renderer this flat list (so the renderer stays free of `prism_asset`
/// types). Previously lived in the deprecated monolithic renderer; kept here
/// as the engine<->renderer exchange type.
pub struct SceneDrawItem {
    pub mesh: MeshHandle,
    pub material_slot: u32,
    pub model: [[f32; 4]; 4],
}

/// Bundled per-frame input from the engine / app layer to [`GraphRenderer`].
///
/// Built each frame by [`render_system`] (ECS → flat data) and consumed by
/// [`GraphRenderer::execute`], which unpacks it into [`GraphFrame`] +
/// [`RenderContext`] and hands them to the render graph.
///
/// This struct is the **data boundary** between the CPU update (ECS queries,
/// camera math, light resolution, …) and the GPU render pipeline.  Future
/// phases (prepare / scene sync) may inject additional data here without
/// touching the [`GraphRenderer`] plumbing.
#[derive(Clone)]
pub struct FrameInput<'a> {
    pub draw_items: &'a [DrawItem],
    /// ECS-authored gizmos for this frame.
    pub gizmos: &'a [crate::gizmo::GizmoInstance],
    /// ECS-authored 2D sprites for this frame (sorted back-to-front by the
    /// engine). Empty for 3D-only scenes.
    pub sprite_items: &'a [crate::render_graph::SpriteDrawItem],
    /// `Light2D` entities for this frame, strongest first (#611).
    pub sprite_lights: &'a [crate::render_graph::SpriteLight],
    /// World-space particle billboards for this frame (sorted back-to-front
    /// by the engine). Empty when no `ParticleSystem` emits this frame.
    pub particle_items: &'a [crate::render_graph::ParticleDrawItem],
    /// Bone palette for skinned draws this frame (#560 P0-3).
    ///
    /// `DrawItem::bone_offset` indexes into this slice; empty when nothing is
    /// skinned. `ForwardPass` uploads it into the per-frame bone SSBO.
    pub bone_matrices: &'a [[[f32; 4]; 4]],
    /// ECS-authored skybox parameters; `None` means no sky draw.
    pub skybox: Option<crate::skybox_pass::SkyboxSettings>,
    pub frame_data: &'a FrameUBOData,
    /// Enabled cameras in `Camera.depth` order (#461). Empty keeps the legacy
    /// single-camera path (`frame_data` describes that one camera).
    pub cameras: &'a [crate::render_graph::CameraView],
    pub light_view_proj: [[f32; 4]; 4],
    /// Far-cascade light-space VP (#526 CSM).
    pub light_view_proj1: [[f32; 4]; 4],
    /// Camera-forward distance of the near/far cascade split.
    pub shadow_split: f32,
    /// 1 = single shadow map, 2 = two-cascade atlas.
    pub shadow_cascade_count: u32,
    pub inv_projection: [[f32; 4]; 4],
    pub debug_flags: u32,
    /// 前向管线线框模式（#431）；设备不支持 `fillModeNonSolid` 时被忽略。
    pub wireframe: bool,
    /// 正常填充 + 线框叠加（#431）。`wireframe` 为 true 时被纯线框覆盖。
    pub shaded_wireframe: bool,
    pub tonemap_mode: u32,
    /// Post-process anti-aliasing mode (0 = off, 1 = FXAA). Runs inside
    /// `PostPass`, so it costs no extra render targets and works identically for
    /// the swapchain and offscreen (editor viewport) paths.
    pub aa_mode: u32,
    pub debug_rt: u32,
    /// Display-referred color grading (#502). `PostPass` applies it after
    /// tonemapping; [`crate::PostGrade::default`] is an exact no-op.
    pub grade: crate::render_graph::PostGrade,
    /// Asset-resolved path to a `.cube` 3D LUT (#502). `None` leaves the
    /// currently bound LUT alone (the pass starts with an identity LUT), so
    /// clearing the setting cannot accidentally sample an unbound texture.
    pub lut_path: Option<&'a Path>,
    pub proj22: f32,
    pub proj32: f32,
    pub lights: &'a [GpuLight],
    pub render_mode: RenderMode,
    pub pt_max_bounces: u32,
    /// Max PT primary + shadow ray length (world units). Forwarded to the PT
    /// pass as a push constant so the inspector can tune it live.
    pub pt_ray_max_distance: f32,
    /// Exposure multiplier applied to the final HDR color before tonemapping.
    /// When the render mode is [`RenderMode::PathTrace`] this value is also
    /// forwarded to the PT compute shader as a push constant.
    pub exposure: f32,
    /// Maximum iterations (samples per pixel) for path tracing.
    /// 0 = accumulate forever (default).
    pub pt_max_iterations: u32,
    /// Analytic lights for path tracing (point/spot/area/directional).
    /// Passed via SSBO to the PT compute shader for multi-light NEE.
    pub pt_lights: &'a [PtAnalyticLight],
    /// When `true`, the path tracer should reset its accumulation next frame.
    /// Set when directional-light properties (intensity/color/direction) change.
    pub pt_accum_dirty: bool,
    /// Whether a usable Camera entity was found. When `false`, passes should
    /// skip camera-dependent work (skybox, PT rays) and leave the clear color.
    pub has_camera: bool,
    /// Whether the active camera projects orthographically. The path tracer
    /// uses it to cast parallel primary rays instead of a perspective fan
    /// (issue #10); the rasterizer already gets this from the projection
    /// matrix alone.
    pub camera_orthographic: bool,
    /// 主方向光是否投阴影（`Light.shadows != None`）。`false` 时整帧
    /// 跳过阴影贴图渲染（`ShadowMode::None`），着色器侧也会得到 `shadowStrength = 0`。
    pub shadows_enabled: bool,
    /// Clear color for the scene color attachment. Applied when the render pass
    /// begins; shows through where no skybox or geometry is drawn. Default gray
    /// `[0.5, 0.5, 0.5, 1.0]` lets the user distinguish "nothing drew" from
    /// black.
    pub clear_color: [f32; 4],
    /// UI overlay draw commands (filled by engine's ui_render_system).
    /// `None` in headless mode.
    pub ui_overlay: Option<&'a crate::ui_overlay::UiOverlayInput>,
    /// 是否在场景渲染中绘制地面参考网格。
    pub show_grid: bool,
    /// 统一脏分派计划（DESIGN §9 / #1157）。
    pub dispatch_plan: Option<&'a crate::dirty_dispatch::DirtyDispatchPlan>,
}

/// GPU session that owns the long-lived Vulkan runtime objects (device,
pub use crate::render_runtime::RenderRuntime;

/// 遮挡剔除粗深度的屏幕块边长（像素）。4×4 在 1080p 下是 480×270 个深度值。
const OCCLUSION_TILE_SIZE: u32 = 4;

mod assets;
mod occlusion;
mod telemetry;
mod temporal;
mod ui_host;

use assets::{env_bytes_fingerprint, make_ibl};
use occlusion::OcclusionState;
use telemetry::RenderTelemetry;
use temporal::TemporalState;
use ui_host::UiOverlayHost;

pub struct GraphRenderer {
    swapchain: Option<Swapchain>,
    /// Present-mode preference reused when the surface is recreated after
    /// a suspend/resume cycle (`Swapchain::new_with_vsync`).
    vsync: bool,
    mesh_manager: RenderMeshManager,
    texture_manager: RenderTextureManager,
    material_manager: RenderMaterialManager,
    // Owned for RAII; IBL cubemap + descriptor set are consumed via the
    // 描述符 集合 handle stored in `forward_pass`. Explicitly destroyed
    // in `destroy()` so the device handle is valid during cleanup.
    ibl: IblResources,
    /// Fingerprint of the active environment payload (`Some(None)` = empty IBL),
    /// used to skip redundant `device_wait_idle` + IBL rebuilds when switching
    /// between scenes that share the same environment map.
    env_fingerprint: Option<Option<u64>>,
    /// Retained environment payload when built in `EnvOnly` (PathTrace) mode,
    /// so switching to `Raster` later can lazily build the PBR IBL.
    env_source: Option<Vec<u8>>,
    /// Whether `self.ibl` currently holds full PBR IBL (irradiance/prefiltered/BRDF)
    /// rather than empty or `EnvOnly` (PathTrace sky).
    ibl_has_pbr: bool,
    /// Scene-level GI probe volume resources (set 5). Survives swapchain
    /// recreation; only rebuilt on scene/level change.
    scene_scope: SceneScope,
    /// 固定 bindless 槽位，持有当前 IBL 的 BRDF LUT 视图。`set_environment`
    /// 重建 IBL 时用 `register_with_handle` 覆盖同一槽，避免 bindless 表增长。
    brdf_bindless_slot: u32,
    graph: RenderGraph,
    /// All 渲染 passes (ShadowMapPass + ForwardPass + GtaoPass + PostPass)
    /// are owned by the graph and executed in registration order. The
    /// `GraphRenderer` no longer pokes individual passes; it drives them via
    /// `graph.execute` and reaches into them only for lifecycle ops
    /// (`recreate_swapchain`) via `graph.pass_mut`.
    settings: RenderSettings,
    /// Temporal anti-aliasing jitter and history state (#883).
    temporal: TemporalState,
    /// Path of the `.cube` LUT currently bound to `PostPass` (#502). `None`
    /// means the pass still holds its startup identity LUT.
    applied_lut: Option<PathBuf>,
    /// 当前需要画选中描边的 ECS 实体 id（#487）。编辑器每帧通过
    /// [`Self::set_outline_entities`] 更新；空 = 不画。
    outline_entities: Vec<u32>,
    shadow_sampler: vk::Sampler,
    // Captured from the graph's allocated shadow map; consumed via the
    // 描述符 集合 in `forward_pass`.
    #[allow(dead_code)]
    shadow_view: vk::ImageView,
    /// `ForwardPass` 在可选 pass 关闭/未就绪时的 1×1 占位纹理集合。
    placeholders: crate::placeholder::ForwardPlaceholders,
    #[allow(dead_code)]
    color_format: vk::Format,
    /// ECS UI 叠加（窗口/离屏）与外部宿主 UI 叠加层宿主（#583, #597）。
    ui_host: UiOverlayHost,
    /// Optional NVIDIA NRD runtime instance (`NRD.dll` / `libNRD.so`).
    nrd_runtime: Option<crate::nrd::NrdRuntime>,
    /// Real-time RTXGI v1.3 DDGI pass (created lazily when `RasterGiMode::Ddgi` is active).
    ddgi_pass: Option<crate::ddgi_pass::DdgiPass>,
    #[cfg(feature = "sharc")]
    sharc_targets: Option<crate::sharc::SharcTargets>,
    /// Reasons the world-space SHaRC cache no longer describes the scene.
    ///
    /// Filled in by the lifecycle hooks ([`Self::invalidate_pt_geometry`],
    /// [`Self::set_environment`], asset release/upload) and by the frame's
    /// [`DirtyDispatchPlan`]; consumed by `execute`, which clears the cache
    /// buffers and then resets this to the default.
    #[cfg(feature = "sharc")]
    sharc_invalidation: crate::sharc::SharcInvalidation,
    /// How many times the world-space SHaRC cache has been cleared since the
    /// renderer was created (initial allocation included). Lifecycle tests use
    /// it to observe that an invalidation actually reached the cache.
    #[cfg(feature = "sharc")]
    sharc_clear_count: u64,
    /// The voxel scale the cache was last trained against.
    ///
    /// The hash key is derived from world position through
    /// `sceneScale` (`sharc_hash_grid.slang`), so changing the scale redefines
    /// which voxel a position lands in without moving any world-space radiance:
    /// every stored entry becomes a key/value mismatch, which §8 calls a cache
    /// format change and wants cleared.
    #[cfg(feature = "sharc")]
    sharc_cache_scene_scale: Option<f32>,

    /// True when no surface is available (headless / CI / server mode).
    is_headless: bool,
    /// A minimized window has a valid surface but no drawable extent. Keep the
    /// existing swapchain and targets alive until the surface becomes usable.
    surface_suspended: bool,
    /// Offscreen target for headless mode — owned device-local image +
    /// host-visible staging buffer. `None` in windowed mode.
    offscreen: Option<OffscreenTarget>,
    /// Fence used to synchronize command buffer reuse in headless / offscreen mode.
    headless_fence: vk::Fence,
    /// GPU 时间戳剖析与截图状态机（#518）。
    telemetry: RenderTelemetry,
    /// 上一帧深度的粗粒度回读（#1007）。关闭时不分配缓冲、不回读。
    occlusion: OcclusionState,
    /// 暂存的资产上传请求队列（#1157），在 `prepare` 阶段统一刷新。
    staged_asset_requests: Vec<crate::asset_bridge::AssetResolveRequest>,
    /// 暂存的资产释放请求队列（#1157），在 `prepare` 阶段统一刷新。
    staged_asset_releases: Vec<crate::asset_bridge::AssetRelease>,
    /// 显存子分配器（#1030）。
    suballocator: crate::suballocator::MemorySuballocator,

    // ── last field ──────────────────────────────────────────────────
    /// Long-lived GPU session (device, command pool, descriptors, UBOs).
    ///
    /// **Must be the last field** — Rust drops struct fields in declaration
    /// order, so `runtime` (which owns the `Arc<VulkanContext>`) is destroyed
    /// *last*, after all other Vulkan-dependent fields have been cleaned up.
    /// This prevents the `ash::Device` from being freed while sibling-field
    /// drops still reference it.
    runtime: RenderRuntime,
}

/// Per-frame context returned by [`GraphRenderer::begin_frame`], consumed by
/// [`GraphRenderer::execute`] and [`GraphRenderer::present`].
pub struct FrameCtx {
    pub device: ash::Device,
    pub cmd: vk::CommandBuffer,
    pub image_index: u32,
    pub frame_index: u32,
    pub extent: vk::Extent2D,
    /// 场景实际渲染的 extent（内部渲染分辨率，#613）；未开启时等于 `extent`。
    pub render_extent: vk::Extent2D,
    fence: vk::Fence,
    image_available: vk::Semaphore,
    render_finished: vk::Semaphore,
}

impl GraphRenderer {
    pub fn new(
        window_extensions: Vec<&str>,
        window: &dyn raw_window_handle::HasDisplayHandle,
        window_handle: &dyn raw_window_handle::HasWindowHandle,
        // 窗口模式：IBL 由场景 `EnvironmentLighting` 在加载时经 `set_environment`
        // 驱动；此处忽略启动期 env_bytes（保持引擎「场景声明式光照」）。
        _env_bytes: Option<Vec<u8>>,
    ) -> anyhow::Result<Self> {
        Self::new_with_vsync(window_extensions, window, window_handle, _env_bytes, true)
    }

    /// Like [`GraphRenderer::new`], but with an explicit vsync preference
    /// forwarded to the swapchain present-mode selection.
    pub fn new_with_vsync(
        window_extensions: Vec<&str>,
        window: &dyn raw_window_handle::HasDisplayHandle,
        window_handle: &dyn raw_window_handle::HasWindowHandle,
        _env_bytes: Option<Vec<u8>>,
        vsync: bool,
    ) -> anyhow::Result<Self> {
        Self::new_with_vsync_and_shadow_size(
            window_extensions,
            window,
            window_handle,
            _env_bytes,
            vsync,
            crate::SHADOW_MAP_SIZE,
            1,
        )
    }

    /// Like [`GraphRenderer::new_with_vsync`], but with an explicit shadow-map
    /// edge length (#526). Values outside
    /// [`crate::MIN_SHADOW_MAP_SIZE`]..=[`crate::MAX_SHADOW_MAP_SIZE`] are
    /// clamped; the resource is allocated when the renderer is constructed.
    pub fn new_with_vsync_and_shadow_size(
        window_extensions: Vec<&str>,
        window: &dyn raw_window_handle::HasDisplayHandle,
        window_handle: &dyn raw_window_handle::HasWindowHandle,
        _env_bytes: Option<Vec<u8>>,
        vsync: bool,
        shadow_map_size: u32,
        shadow_cascade_count: u32,
    ) -> anyhow::Result<Self> {
        let context = Arc::new(VulkanContext::new(&window_extensions)?);
        let swapchain = Swapchain::new_with_vsync(&context, window, window_handle, vsync)?;
        let color_format = swapchain.format.format;

        // Build the GPU runtime: descriptor infrastructure, command pool,
        // per-frame command buffers, and frame UBOs.  These survive swapchain
        // recreation; the runtime is independent of scene-level resources.
        let runtime = RenderRuntime::new(context.clone(), swapchain.views.len() as u32)?;

        // IBL：开屏/无光照场景以廉价「中性空 IBL」启动（不做 CPU 预卷积，≈0ms）。
        // 真正的 IBL 预卷积由 `set_environment` 在加载带光照的 3D 场景时按需重建。
        // 描述符 集合 布局 共享创建一次，确保场景切换重建 IBL 时管线 布局 稳定。
        let ibl_ds_layout = crate::ibl::ibl_descriptor_set_layout(&context.device)
            .context("create IBL ds layout")?;
        let ibl = IblResources::new_empty(
            context.clone(),
            runtime.command_pool,
            context.graphics_queue,
            ibl_ds_layout,
        )
        .context("create empty IBL resources")?;

        let mut texture_manager =
            RenderTextureManager::new(&context, runtime.command_pool, context.graphics_queue, 1024)
                .context("create RenderTextureManager")?;
        let material_manager =
            RenderMaterialManager::new(&context).context("create RenderMaterialManager")?;
        let mesh_manager = RenderMeshManager::new();

        let shadow_sampler = unsafe {
            context.device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .compare_enable(true)
                    .compare_op(vk::CompareOp::LESS)
                    .border_color(vk::BorderColor::FLOAT_OPAQUE_WHITE)
                    .unnormalized_coordinates(false),
                None,
            )
        }
        .context("create shadow comparison sampler")?;

        let placeholders = crate::placeholder::ForwardPlaceholders::new(
            &context,
            runtime.command_pool,
            context.graphics_queue,
        )?;

        let resolved = RenderSettings::default().resolve_shadow(&context.rt_caps);
        let settings = RenderSettings {
            shadow_mode: resolved,
            shadow_map_size: shadow_map_size
                .clamp(crate::MIN_SHADOW_MAP_SIZE, crate::MAX_SHADOW_MAP_SIZE),
            shadow_cascade_count: shadow_cascade_count.clamp(1, 2),
            ray_tracing_enabled: false,
            ..Default::default()
        };

        // Build graph with ShadowMapPass. Call setup() on the pass before
        // adding it so it registers its shadow-map resource, then allocate the
        // graph's Vulkan resources (the shadow map depth image) and fetch its
        // 图像 视图 for the ForwardPass to 样本
        //
        // `shadows` feature 关闭时整块不编译：图直接从空开始，ForwardPass 的
        // set 3 改绑一张 1×1 远深度 dummy（descriptor 布局不变，仍能过验证层）。
        #[cfg(feature = "shadows")]
        let (mut graph, shadow_view) = {
            let mut shadow_pass = crate::shadow_map_pass::ShadowMapPass::new();
            let mut builder = RenderGraphBuilder::new().settings(&settings);
            shadow_pass.setup(&mut builder, &settings);
            let shadow_handle = shadow_pass.shadow_map_handle();
            builder.add_pass(Box::new(shadow_pass));
            let mut graph = builder.build();
            graph
                .allocate_resources(&context.device, &context.physical_device_memory_properties)
                .context("allocate graph resources")?;
            let shadow_view = graph
                .image_view(shadow_handle)
                .context("shadow map view not found")?;
            (graph, shadow_view)
        };

        #[cfg(not(feature = "shadows"))]
        let (mut graph, shadow_view) = {
            let mut graph = RenderGraphBuilder::new().settings(&settings).build();
            graph
                .allocate_resources(&context.device, &context.physical_device_memory_properties)
                .context("allocate graph resources (shadows off)")?;
            log::warn!(
                "`shadows` feature 关闭：实时阴影不可用（set 3 绑定 1x1 远深度图）；\
                 需要阴影请在项目里启用 prism-render/shadows"
            );
            (graph, placeholders.shadow_view())
        };

        // 创建 forward_pass and wire its resources: IBL 集合 shadow 映射表 视图 +
        // comparison sampler, bindless texture table, material SSBO, and the
        // per-frame UBO buffers (one set0 descriptor set per frame-in-flight).
        // ForwardPass is executed directly by GraphRenderer (it targets the
        // swapchain, not a graph-managed resource).
        let frame_ubo_buffers: Vec<vk::Buffer> =
            runtime.frame_ubos.iter().map(|u| u.buffer).collect();
        let frame_ubo_stride = runtime
            .frame_ubos
            .first()
            .map(|ubo| ubo.stride as u32)
            .unwrap_or(0);
        let bindless = texture_manager.bindless_mut();
        let materials_buffer = material_manager.buffer();

        // Register the BRDF LUT in the bindless texture table.
        let brdf_handle = bindless
            .register(ibl.brdf_image_view())
            .context("register BRDF LUT into bindless table")?;
        log::info!(
            "IBL: BRDF LUT registered as bindless handle {}",
            brdf_handle.0
        );

        let mut forward_pass = ForwardPass::new(color_format);
        // Scene-level 全局光照 probe 音量 (SceneScope). Created before ForwardPass
        // wiring so its descriptor set + layout can be borrowed (set 5).
        let scene_scope = SceneScope::new(context.clone()).context("SceneScope::new")?;
        forward_pass
            .set_resources(
                &context,
                ibl.descriptor_set,
                ibl.descriptor_set_layout,
                shadow_view,
                shadow_sampler,
                bindless.set,
                bindless.layout,
                materials_buffer,
                &frame_ubo_buffers,
                brdf_handle.0,
                scene_scope.descriptor_set,
                scene_scope.descriptor_set_layout,
                frame_ubo_stride,
                placeholders.gi_view(),
            )
            .context("ForwardPass: set_resources")?;

        // ReSTIR GI pass (raster path, independent of the path tracer). It
        // builds its own BLAS/TLAS from the draw list and traces secondary
        // rays with ray queries, so it only exists when the device can do
        // that; otherwise `gi_mode = Restir` falls back to the baked probe
        // volume and the warning below says why.
        #[cfg(feature = "restir_gi")]
        let restir_gi_pass = if context.rt_caps.has_ray_query() {
            let mut pass = crate::restir_gi::RestirGiPass::new();
            pass.set_resources(
                bindless.set,
                bindless.layout,
                ibl.descriptor_set,
                ibl.descriptor_set_layout,
                materials_buffer,
            );
            log::info!("RestirGiPass registered in render graph");
            Some(pass)
        } else {
            log::warn!(
                "`restir_gi` 已编译但设备不支持 VK_KHR_ray_query：\
                 gi_mode = Restir 退回 baked probe volume"
            );
            None
        };

        // GTAO pass half-resolution screen-space 环境光遮蔽 Runs after ForwardPass
        // every frame and produces a double-buffered R8 AO texture the scene
        // samples (1-frame latency) to attenuate IBL diffuse + specular.
        //
        // `gtao` feature 关闭时整块不编译：set 4 改绑 1×1 白色（无遮蔽）dummy。
        #[cfg(feature = "gtao")]
        let swapchain_extent = swapchain.extent;
        #[cfg(feature = "gtao")]
        let gtao_pass =
            crate::gtao::GtaoPass::new(&context, runtime.command_pool, swapchain_extent)
                .context("GtaoPass::new")?;
        #[cfg(not(feature = "gtao"))]
        log::warn!(
            "`gtao` feature 关闭：屏幕空间环境光遮蔽不可用（set 4 绑定 1x1 白色图）；\
             需要 AO 请在项目里启用 prism-render/gtao"
        );

        // TAA resolve (#883). Always registered; `execute` is a no-op when
        // `RenderSettings::taa` is false.
        let taa_pass =
            crate::taa::TaaPass::new(&context, swapchain.extent).context("TaaPass::new")?;
        #[cfg(feature = "anf")]
        let anf_pass = crate::anf::AnfPass::new(&context, swapchain.extent, swapchain.extent)
            .context("AnfPass::new")?;

        // PostPass parameters: one descriptor set per swapchain image. The sets
        // are picked by `image_index`, so allocating fewer than the swapchain
        // has would make two images share a set and rewrite a descriptor the
        // GPU may still be reading.
        let frame_count = swapchain.views.len() as u32;
        let mut post_pass = crate::post::PostPass::new(
            &context,
            color_format,
            frame_count,
            vk::ImageLayout::PRESENT_SRC_KHR,
        )
        .context("PostPass::new")?;

        // AutoExposurePass meters the HDR image PostPass is about to tonemap and
        // publishes the adapted luminance PostPass divides AUTO_EXPOSURE_KEY by.
        // Registered ahead of PostPass so the ordering falls out of the graph.
        let auto_exposure = crate::auto_exposure::AutoExposurePass::new(&context, frame_count)
            .context("AutoExposurePass::new")?;
        post_pass.set_adapted_luminance(&context.device, auto_exposure.luminance_buffer());

        // PathTracePass — real-time path tracing compute pass.
        // Registered when hardware acceleration structures are supported;
        // checks RenderSettings.render_mode internally to decide whether to
        // dispatch. Scene geometry is uploaded later via set_geometry.
        // Omitted entirely when `pathtrace` feature is disabled (building blocks, see DESIGN §12).
        #[cfg(feature = "pathtrace")]
        #[cfg(feature = "pathtrace")]
        let pt_pass = if context.rt_caps.acceleration_structure {
            match PathTracePass::new(&context) {
                Ok(pt) => {
                    pt.set_material_resources(materials_buffer, bindless.set, bindless.layout);
                    pt.set_ibl_resources(ibl.descriptor_set, ibl.descriptor_set_layout);
                    log::info!("PathTracePass registered in render graph");
                    Some(pt)
                }
                Err(err) => {
                    log::warn!("Failed to initialize PathTracePass despite RT caps: {err:#}");
                    None
                }
            }
        } else {
            log::debug!("Acceleration structures not supported; PathTracePass not registered");
            None
        };

        // CRT 调度器：统一渲染所有注册的 RenderTexture（引擎式）。
        let rt_scheduler = crate::rt_scheduler::RenderTextureScheduler::new(&context);

        // Register all passes into the graph in execution order.
        // Shadow -> Scene -> GTAO -> RenderTextures -> PathTrace -> Resolve -> Post.
        graph.add_pass(Box::new(forward_pass));
        // 启动路径：同步 per-swapchain-image 槽位数。`ForwardPass::execute` 的
        // `ensure_target` 按 `image_count` 建 framebuffer，`recreate_swapchain`
        // 只覆盖下一步扩展重建——首次创建必须在这里初始化，否则 `set_target` 里
        // `idx >= image_count` 的守卫会静默跳过，永远建不出 framebuffer。
        if let Some(forward_pass) = graph.pass_mut::<ForwardPass>() {
            forward_pass.set_image_count(swapchain.views.len());
        }
        graph.add_pass(Box::new(crate::voxel_dda_pass::VoxelDdaPass::new(
            Arc::clone(&context),
        )?));
        if context.rt_caps.has_ray_query() {
            graph.add_pass(Box::new(crate::voxel_rt_gi_pass::VoxelRtGiPass::new(
                &context,
            )?));
        }
        #[cfg(feature = "gtao")]
        graph.add_pass(Box::new(gtao_pass));
        #[cfg(feature = "anf")]
        graph.add_pass(Box::new(anf_pass));
        // ReSTIR GI reads the forward G-buffer this frame and publishes the
        // signal the forward pass samples on the next one.
        #[cfg(feature = "restir_gi")]
        if let Some(pass) = restir_gi_pass {
            graph.add_pass(Box::new(pass));
        }
        graph.add_pass(Box::new(rt_scheduler));
        // SHaRC is a graph step, not a frame preamble: the trainer deposits
        // radiance, the resolve collapses it, and only then does the tracer
        // query it (repair spec §4 — no invisible training or resolve outside
        // the graph).
        #[cfg(all(feature = "pathtrace", feature = "sharc"))]
        if let Some(pt) = &pt_pass {
            graph.add_pass(Box::new(pt.trainer()));
        }
        #[cfg(feature = "sharc")]
        graph.add_pass(Box::new(crate::sharc::SharcResolveNode::new()));
        #[cfg(feature = "pathtrace")]
        if let Some(pt) = pt_pass {
            graph.add_pass(Box::new(pt));
        }
        #[cfg(feature = "prt")]
        graph.add_pass(Box::new(crate::prt_pass::PrtPass::new(context.clone())));
        // The temporal resolve runs after the path tracer so it can consume the
        // traced image and its guides in path-trace mode (#883 follow-up). In the
        // raster path it still resolves the forward pass output.
        graph.add_pass(Box::new(taa_pass));
        let mobile_upscaler_pass =
            crate::mobile_upscaler::MobileUpscalerPass::new(&context, swapchain.extent)
                .context("MobileUpscalerPass::new")?;
        graph.add_pass(Box::new(mobile_upscaler_pass));
        graph.add_pass(Box::new(auto_exposure));
        graph.add_pass(Box::new(post_pass));

        // per-pass GPU 计时（#518）：设备不支持时间戳时是 `None`（面板回退到 CPU 墙钟）。
        let telemetry = RenderTelemetry::new(&context, runtime.command_buffers.len())?;

        Ok(Self {
            swapchain: Some(swapchain),
            vsync,
            runtime,
            mesh_manager,
            texture_manager,
            material_manager,
            ibl,
            env_fingerprint: Some(None),
            env_source: None,
            ibl_has_pbr: false,
            scene_scope,
            brdf_bindless_slot: brdf_handle.0,
            graph,
            settings,
            temporal: TemporalState::default(),
            applied_lut: None,
            outline_entities: Vec::new(),
            shadow_sampler,
            shadow_view,
            placeholders,
            color_format,
            ui_host: UiOverlayHost::default(),
            nrd_runtime: crate::nrd::NrdRuntime::try_init(),
            ddgi_pass: None,
            #[cfg(feature = "sharc")]
            sharc_targets: None,
            #[cfg(feature = "sharc")]
            sharc_invalidation: crate::sharc::SharcInvalidation::default(),
            #[cfg(feature = "sharc")]
            sharc_clear_count: 0,
            #[cfg(feature = "sharc")]
            sharc_cache_scene_scale: None,

            is_headless: false,
            surface_suspended: false,
            offscreen: None,
            headless_fence: vk::Fence::null(),
            telemetry,
            occlusion: OcclusionState {
                margin_tiles: 1,
                ..Default::default()
            },
            staged_asset_requests: Vec::new(),
            staged_asset_releases: Vec::new(),
            suballocator: crate::suballocator::MemorySuballocator::new(
                crate::suballocator::MemorySuballocator::DEFAULT_CHUNK_SIZE,
            ),
        })
    }
    // -------------------------------------------------------------------
    // Public API
    // -------------------------------------------------------------------

    pub fn context(&self) -> &VulkanContext {
        &self.runtime.context
    }
    pub fn context_arc(&self) -> Arc<VulkanContext> {
        self.runtime.context.clone()
    }
    pub fn command_pool(&self) -> vk::CommandPool {
        self.runtime.command_pool
    }
    pub fn graphics_queue(&self) -> vk::Queue {
        self.runtime.context.graphics_queue
    }

    /// Whether this renderer was created in headless mode (no window surface).
    pub fn is_headless(&self) -> bool {
        self.is_headless
    }

    /// 场景 pass 要采样的「上一帧环境光遮蔽」视图。
    ///
    /// `gtao` 开启时取 GTAO pass 的双缓冲 `(frame+1)%2`；关闭时返回 1×1 白色
    /// dummy（可见性 1 = 无遮蔽），descriptor 布局与绑定次数完全一致。
    #[cfg(feature = "gtao")]
    fn ao_view(&mut self, frame: u32) -> vk::ImageView {
        self.graph
            .pass_mut::<crate::gtao::GtaoPass>()
            .map(|g| g.ao_view((frame + 1) % 2))
            .unwrap_or_else(vk::ImageView::null)
    }

    #[cfg(not(feature = "gtao"))]
    fn ao_view(&mut self, _frame: u32) -> vk::ImageView {
        self.placeholders.ao_view()
    }

    /// 场景 pass 要采样的「上一帧 ReSTIR GI 间接漫反射」视图。
    ///
    /// 与 AO 同构：`RestirGiPass` 双缓冲，本帧写 A、下一帧采样 A，所以取
    /// `gi_view()` 返回的「最近一次已写完」的视图。pass 未注册或本帧未分派
    /// 时返回 null，前向 pass 会退回 baked probe volume。
    #[cfg(feature = "restir_gi")]
    fn gi_view(&mut self) -> vk::ImageView {
        self.graph
            .pass_mut::<crate::restir_gi::RestirGiPass>()
            .map(|pass| pass.gi_view())
            .unwrap_or_else(vk::ImageView::null)
    }

    #[cfg(not(feature = "restir_gi"))]
    fn gi_view(&mut self) -> vk::ImageView {
        vk::ImageView::null()
    }

    #[cfg(feature = "prt")]
    fn prt_gi_descriptor_set(&mut self, frame: u32) -> vk::DescriptorSet {
        self.graph
            .pass_ref::<crate::prt_pass::PrtPass>()
            .and_then(|p| p.gi_volume_descriptor_set(frame))
            .unwrap_or_else(vk::DescriptorSet::null)
    }

    #[cfg(not(feature = "prt"))]
    fn prt_gi_descriptor_set(&mut self, _frame: u32) -> vk::DescriptorSet {
        vk::DescriptorSet::null()
    }

    /// 离屏渲染目标的尺寸（无窗口宿主模式）；有交换链时返回 `None`。
    pub fn offscreen_extent(&self) -> Option<ash::vk::Extent2D> {
        self.offscreen.as_ref().map(|target| target.extent())
    }

    /// 离屏颜色图像视图（`R8G8B8A8_UNORM`）。帧结束后先调用
    /// [`Self::offscreen_present_for_sampling`]，再把它注册成 UI 纹理。
    pub fn offscreen_image_view(&self) -> Option<ash::vk::ImageView> {
        self.offscreen.as_ref().map(|target| target.image_view())
    }

    /// 离屏颜色图像的格式。
    pub fn offscreen_image_format(&self) -> Option<ash::vk::Format> {
        self.offscreen.as_ref().map(|target| target.format())
    }

    /// 把帧结束后的离屏图像转到 `SHADER_READ_ONLY_OPTIMAL` 并等待 GPU
    /// 完成，供 UI 框架直接采样（零拷贝视口）。
    ///
    /// 帧结束后离屏图像停在 `COLOR_ATTACHMENT_OPTIMAL`（headless 上下文没有
    /// `VK_KHR_swapchain`，不能用 `PRESENT_SRC_KHR`）；下一次帧的 PostPass 用
    /// `initial_layout = UNDEFINED`（load op clear），因此从任何布局回来都合法。
    pub fn offscreen_present_for_sampling(&mut self) -> anyhow::Result<()> {
        let context = self.runtime.context.clone();
        let target = self
            .offscreen
            .as_mut()
            .context("offscreen_present_for_sampling: no OffscreenTarget")?;
        target.transition_to_shader_read(&context, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
    }

    // ── headless mode ──────────────────────────────────────────────

    /// Create a headless `GraphRenderer` — no window surface or swapchain.
    /// Useful for CI tests, dedicated servers, and offline asset baking.
    ///
    /// `env_bytes` is optional IBL environment map data (`None` = default sky).
    pub fn headless_new(env_bytes: Option<Vec<u8>>) -> anyhow::Result<Self> {
        Self::offscreen_new(256, 256, env_bytes)
    }

    /// 创建 a 无窗口 `GraphRenderer` rendering into an offscreen 图像 of
    /// `width × height` (编辑器 host 模式 / 离屏预览).
    ///
    /// 与 [`Self::headless_new`] 同一条渲染路径：完整图（Shadow → Scene →
    /// GTAO → Post）跑完后，PostPass 把 **tonemap 后的结果** 写进离屏图像，
    /// 随后 [`Self::copy_offscreen_to_readback`] + [`Self::readback_pixels`]
    /// 把那一帧读回 CPU。
    pub fn offscreen_new(
        width: u32,
        height: u32,
        env_bytes: Option<Vec<u8>>,
    ) -> anyhow::Result<Self> {
        Self::offscreen_new_with_shadow_size(width, height, env_bytes, crate::SHADOW_MAP_SIZE, 1)
    }

    /// Like [`Self::offscreen_new`], but with an explicit shadow-map edge length
    /// (#526), clamped to the supported range.
    pub fn offscreen_new_with_shadow_size(
        width: u32,
        height: u32,
        env_bytes: Option<Vec<u8>>,
        shadow_map_size: u32,
        shadow_cascade_count: u32,
    ) -> anyhow::Result<Self> {
        let context = Arc::new(VulkanContext::new(&[])?);
        Self::offscreen_new_with_context_and_shadow_size(
            context,
            width,
            height,
            env_bytes,
            shadow_map_size,
            shadow_cascade_count,
        )
    }

    /// Like [`Self::offscreen_new`], but on a **host-provided** context.
    ///
    /// This is the zero-copy host path: the editor creates the `VkDevice`
    /// (shared with its UI framework via [`VulkanContext::from_host`]) and the
    /// renderer draws into its own offscreen image. After each frame
    /// [`Self::offscreen_present_for_sampling`] turns that image into a
    /// `SHADER_READ_ONLY_OPTIMAL` texture and
    /// [`Self::offscreen_image_view`] hands it to the UI framework —
    /// no `vkCmdCopyImageToBuffer` / CPU round-trip.
    pub fn offscreen_new_with_context(
        context: Arc<VulkanContext>,
        width: u32,
        height: u32,
        env_bytes: Option<Vec<u8>>,
    ) -> anyhow::Result<Self> {
        Self::offscreen_new_with_context_and_shadow_size(
            context,
            width,
            height,
            env_bytes,
            crate::SHADOW_MAP_SIZE,
            1,
        )
    }

    /// Like [`Self::offscreen_new_with_context`], but with an explicit shadow-map
    /// edge length (#526), clamped to the supported range.
    pub fn offscreen_new_with_context_and_shadow_size(
        context: Arc<VulkanContext>,
        width: u32,
        height: u32,
        env_bytes: Option<Vec<u8>>,
        shadow_map_size: u32,
        shadow_cascade_count: u32,
    ) -> anyhow::Result<Self> {
        let offscreen = OffscreenTarget::new(&context, width, height)?;
        let color_format = offscreen.format();

        let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
        let headless_fence = unsafe { context.device.create_fence(&fence_info, None) }
            .context("create headless frame fence")?;

        // Runtime with 2 command buffers (minimal for headless ops).
        let runtime = RenderRuntime::new(context.clone(), 2)?;

        // Headless mode still builds the full render graph so the
        // graph's topology is validated even without a display.
        let resolver = RenderSettings::default().resolve_shadow(&context.rt_caps);
        let settings = RenderSettings {
            shadow_mode: resolver,
            shadow_map_size: shadow_map_size
                .clamp(crate::MIN_SHADOW_MAP_SIZE, crate::MAX_SHADOW_MAP_SIZE),
            shadow_cascade_count: shadow_cascade_count.clamp(1, 2),
            ray_tracing_enabled: false,
            ..Default::default()
        };

        let ibl_ds_layout = crate::ibl::ibl_descriptor_set_layout(&context.device)
            .context("create IBL ds layout (headless)")?;
        let initial_env_fp = env_bytes.as_deref().map(env_bytes_fingerprint);
        let initial_scope = if settings.render_mode == RenderMode::PathTrace {
            crate::ibl::IblScope::EnvOnly
        } else {
            crate::ibl::IblScope::Full
        };
        let initial_has_pbr = env_bytes.is_some() && initial_scope == crate::ibl::IblScope::Full;
        let ibl = make_ibl(
            context.clone(),
            runtime.command_pool,
            context.graphics_queue,
            env_bytes.as_deref(),
            initial_scope,
            ibl_ds_layout,
        )
        .context("create IBL resources (headless)")?;

        let mut texture_manager =
            RenderTextureManager::new(&context, runtime.command_pool, context.graphics_queue, 1024)
                .context("create RenderTextureManager (headless)")?;
        let material_manager = RenderMaterialManager::new(&context)
            .context("create RenderMaterialManager (headless)")?;
        let mesh_manager = RenderMeshManager::new();

        let shadow_sampler = unsafe {
            context.device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .compare_enable(true)
                    .compare_op(vk::CompareOp::LESS)
                    .border_color(vk::BorderColor::FLOAT_OPAQUE_WHITE)
                    .unnormalized_coordinates(false),
                None,
            )
        }
        .context("create shadow comparison sampler (headless)")?;

        let placeholders = crate::placeholder::ForwardPlaceholders::new(
            &context,
            runtime.command_pool,
            context.graphics_queue,
        )?;

        // Graph: Shadow -> Scene -> GTAO -> PathTrace -> Post.
        #[cfg(feature = "shadows")]
        let (mut graph, shadow_view) = {
            let mut shadow_pass = crate::shadow_map_pass::ShadowMapPass::new();
            let mut builder = RenderGraphBuilder::new().settings(&settings);
            shadow_pass.setup(&mut builder, &settings);
            let shadow_handle = shadow_pass.shadow_map_handle();
            builder.add_pass(Box::new(shadow_pass));
            let mut graph = builder.build();
            graph
                .allocate_resources(&context.device, &context.physical_device_memory_properties)
                .context("allocate graph resources (headless)")?;
            let shadow_view = graph
                .image_view(shadow_handle)
                .context("shadow map view not found (headless)")?;
            (graph, shadow_view)
        };

        #[cfg(not(feature = "shadows"))]
        let (mut graph, shadow_view) = {
            let mut graph = RenderGraphBuilder::new().settings(&settings).build();
            graph
                .allocate_resources(&context.device, &context.physical_device_memory_properties)
                .context("allocate graph resources (headless, shadows off)")?;
            log::warn!(
                "`shadows` feature 关闭：实时阴影不可用（set 3 绑定 1x1 远深度图）；\
                 需要阴影请在项目里启用 prism-render/shadows"
            );
            (graph, placeholders.shadow_view())
        };

        let frame_ubo_buffers: Vec<vk::Buffer> =
            runtime.frame_ubos.iter().map(|u| u.buffer).collect();
        let frame_ubo_stride = runtime
            .frame_ubos
            .first()
            .map(|ubo| ubo.stride as u32)
            .unwrap_or(0);
        let bindless = texture_manager.bindless_mut();
        let materials_buffer = material_manager.buffer();

        let brdf_handle = bindless
            .register(ibl.brdf_image_view())
            .context("register BRDF LUT into bindless table (headless)")?;

        let mut forward_pass = ForwardPass::new(color_format);
        let scene_scope = SceneScope::new(context.clone()).context("SceneScope::new (headless)")?;
        forward_pass
            .set_resources(
                &context,
                ibl.descriptor_set,
                ibl.descriptor_set_layout,
                shadow_view,
                shadow_sampler,
                bindless.set,
                bindless.layout,
                materials_buffer,
                &frame_ubo_buffers,
                brdf_handle.0,
                scene_scope.descriptor_set,
                scene_scope.descriptor_set_layout,
                frame_ubo_stride,
                placeholders.gi_view(),
            )
            .context("ForwardPass::set_resources (headless)")?;

        // Same ReSTIR GI pass as the windowed path; the offscreen extent is
        // resolved lazily from `ctx.render_extent` on the first dispatch.
        #[cfg(feature = "restir_gi")]
        let restir_gi_pass = if context.rt_caps.has_ray_query() {
            let mut pass = crate::restir_gi::RestirGiPass::new();
            pass.set_resources(
                bindless.set,
                bindless.layout,
                ibl.descriptor_set,
                ibl.descriptor_set_layout,
                materials_buffer,
            );
            log::info!("RestirGiPass registered in render graph (headless)");
            Some(pass)
        } else {
            None
        };

        let frame_count = 2u32;
        // Headless contexts do not enable `VK_KHR_swapchain`, so the offscreen
        // image must never end a render pass in `PRESENT_SRC_KHR`; it stays a
        // color attachment until something explicitly transitions it.
        let mut post_pass = crate::post::PostPass::new(
            &context,
            color_format,
            frame_count,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        )
        .context("PostPass::new (headless)")?;

        // AutoExposurePass meters the HDR image PostPass is about to tonemap and
        // publishes the adapted luminance PostPass divides AUTO_EXPOSURE_KEY by.
        // Registered ahead of PostPass so the ordering falls out of the graph.
        let auto_exposure = crate::auto_exposure::AutoExposurePass::new(&context, frame_count)
            .context("AutoExposurePass::new (headless)")?;
        post_pass.set_adapted_luminance(&context.device, auto_exposure.luminance_buffer());

        // `gtao` feature 关闭时 set 4 退化为 1×1 白色（无遮蔽）。
        #[cfg(feature = "gtao")]
        let gtao_pass =
            crate::gtao::GtaoPass::new(&context, runtime.command_pool, offscreen.extent())
                .context("GtaoPass::new (headless)")?;
        #[cfg(not(feature = "gtao"))]
        log::warn!(
            "`gtao` feature 关闭：屏幕空间环境光遮蔽不可用（set 4 绑定 1x1 白色图）；\
             需要 AO 请在项目里启用 prism-render/gtao"
        );

        let taa_pass = crate::taa::TaaPass::new(&context, offscreen.extent())
            .context("TaaPass::new (headless)")?;
        #[cfg(feature = "anf")]
        let anf_pass = crate::anf::AnfPass::new(&context, offscreen.extent(), offscreen.extent())
            .context("AnfPass::new (headless)")?;

        // CRT 调度器（headless 下同样接入图，支持无窗口离屏 RT 调度）。
        let rt_scheduler = crate::rt_scheduler::RenderTextureScheduler::new(&context);

        #[cfg(feature = "pathtrace")]
        let pt_pass = if context.rt_caps.acceleration_structure {
            match PathTracePass::new(&context) {
                Ok(pt) => {
                    pt.set_material_resources(materials_buffer, bindless.set, bindless.layout);
                    pt.set_ibl_resources(ibl.descriptor_set, ibl.descriptor_set_layout);
                    log::info!("PathTracePass registered in render graph (headless)");
                    Some(pt)
                }
                Err(err) => {
                    log::warn!(
                        "Failed to initialize PathTracePass (headless) despite RT caps: {err:#}"
                    );
                    None
                }
            }
        } else {
            None
        };

        graph.add_pass(Box::new(forward_pass));
        if let Some(forward_pass) = graph.pass_mut::<ForwardPass>() {
            forward_pass.set_image_count(1);
            forward_pass.set_target(&context.device, &context, 1, 0, offscreen.extent())?;
        }
        graph.add_pass(Box::new(crate::voxel_dda_pass::VoxelDdaPass::new(
            Arc::clone(&context),
        )?));
        if context.rt_caps.has_ray_query() {
            graph.add_pass(Box::new(crate::voxel_rt_gi_pass::VoxelRtGiPass::new(
                &context,
            )?));
        }
        #[cfg(feature = "gtao")]
        graph.add_pass(Box::new(gtao_pass));
        #[cfg(feature = "anf")]
        graph.add_pass(Box::new(anf_pass));
        #[cfg(feature = "restir_gi")]
        if let Some(pass) = restir_gi_pass {
            graph.add_pass(Box::new(pass));
        }
        graph.add_pass(Box::new(rt_scheduler));
        // Same order as the windowed path: train, resolve, then consume.
        #[cfg(all(feature = "pathtrace", feature = "sharc"))]
        if let Some(pt) = &pt_pass {
            graph.add_pass(Box::new(pt.trainer()));
        }
        #[cfg(feature = "sharc")]
        graph.add_pass(Box::new(crate::sharc::SharcResolveNode::new()));
        #[cfg(feature = "pathtrace")]
        if let Some(pt) = pt_pass {
            graph.add_pass(Box::new(pt));
        }
        // Same ordering as the windowed path: resolve after the tracer.
        #[cfg(feature = "prt")]
        graph.add_pass(Box::new(crate::prt_pass::PrtPass::new(context.clone())));
        graph.add_pass(Box::new(taa_pass));
        let mobile_upscaler_pass =
            crate::mobile_upscaler::MobileUpscalerPass::new(&context, offscreen.extent())
                .context("MobileUpscalerPass::new (headless)")?;
        graph.add_pass(Box::new(mobile_upscaler_pass));
        graph.add_pass(Box::new(auto_exposure));
        graph.add_pass(Box::new(post_pass));
        if let Some(post_pass) = graph.pass_mut::<crate::post::PostPass>() {
            post_pass.set_target(
                &context.device,
                std::slice::from_ref(&offscreen.image_view()),
                0,
                offscreen.extent(),
            )?;
        }

        let telemetry = RenderTelemetry::new(&context, runtime.command_buffers.len())?;

        Ok(Self {
            swapchain: None,
            vsync: false,
            runtime,
            mesh_manager,
            texture_manager,
            material_manager,
            ibl,
            env_fingerprint: Some(initial_env_fp),
            env_source: env_bytes,
            ibl_has_pbr: initial_has_pbr,
            scene_scope,
            brdf_bindless_slot: brdf_handle.0,
            graph,
            settings,
            temporal: TemporalState::default(),
            applied_lut: None,
            outline_entities: Vec::new(),
            shadow_sampler,
            shadow_view,
            placeholders,
            color_format,
            ui_host: UiOverlayHost::default(),
            nrd_runtime: crate::nrd::NrdRuntime::try_init(),
            ddgi_pass: None,
            #[cfg(feature = "sharc")]
            sharc_targets: None,
            #[cfg(feature = "sharc")]
            sharc_invalidation: crate::sharc::SharcInvalidation::default(),
            #[cfg(feature = "sharc")]
            sharc_clear_count: 0,
            #[cfg(feature = "sharc")]
            sharc_cache_scene_scale: None,

            is_headless: true,
            surface_suspended: false,
            telemetry,
            occlusion: OcclusionState {
                margin_tiles: 1,
                ..Default::default()
            },
            staged_asset_requests: Vec::new(),
            staged_asset_releases: Vec::new(),
            offscreen: Some(offscreen),
            headless_fence,
            suballocator: crate::suballocator::MemorySuballocator::new(
                crate::suballocator::MemorySuballocator::DEFAULT_CHUNK_SIZE,
            ),
        })
    }

    /// Clear the offscreen image to `color`, copy to the host-visible buffer,
    /// and wait for the GPU.  Only valid in headless mode.
    pub fn clear_offscreen(&mut self, color: [f32; 4]) -> anyhow::Result<()> {
        let target = self
            .offscreen
            .as_mut()
            .context("clear_offscreen called but no OffscreenTarget (not headless?)")?;
        target.clear_and_copy(&self.runtime.context, color)
    }

    /// Read back pixel data from the offscreen target.
    /// Returns RGBA bytes, 256 × 256 = 262144 bytes.
    /// Only 有效 after [`clear_offscreen`](Self::clear_offscreen) or
    /// [`copy_offscreen_to_readback`](Self::copy_offscreen_to_readback).
    pub fn readback_pixels(&self) -> anyhow::Result<Vec<u8>> {
        let target = self
            .offscreen
            .as_ref()
            .context("readback_pixels called but no OffscreenTarget")?;
        target.readback(&self.runtime.context)
    }

    /// 轮询异步离屏读回；GPU 未完成时返回 `None`。
    pub fn poll_offscreen_readback(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
        self.offscreen
            .as_mut()
            .context("poll_offscreen_readback called but no OffscreenTarget")?
            .poll_readback(&self.runtime.context)
    }

    /// Copy the rendered offscreen image to the host-visible readback buffer
    /// and wait for completion. Only valid in headless mode. Afterwards call
    /// [`readback_pixels`](Self::readback_pixels).
    pub fn copy_offscreen_to_readback(&mut self) -> anyhow::Result<()> {
        let target = self
            .offscreen
            .as_mut()
            .context("copy_offscreen_to_readback called but no OffscreenTarget (not headless?)")?;
        target.copy_to_readback(
            &self.runtime.context,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        )
    }

    /// 异步提交离屏图像读回；已有读回未完成时跳过本次提交。
    pub fn copy_offscreen_to_readback_async(&mut self) -> anyhow::Result<bool> {
        self.offscreen
            .as_mut()
            .context("copy_offscreen_to_readback_async called but no OffscreenTarget")?
            .copy_to_readback_async(
                &self.runtime.context,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            )
    }

    /// Ask the next frame to copy the path tracer's **linear** output
    /// (`PT_COLOR_H`, before tonemapping) into a host-visible buffer.
    ///
    /// `readback_pixels` returns tonemapped 8-bit values, which clips every
    /// radiance above 1.0 into the same white - useless for checking that a
    /// path-traced frame is linear in its inputs (repair spec §7.2).
    pub fn request_hdr_readback(&mut self) {
        self.graph.request_hdr_readback();
    }

    /// Map the linear readback recorded by the last executed frame, if any.
    ///
    /// Call after `present` (headless) or `end_frame` (windowed); both wait for
    /// the frame fence, so the staging buffer is already safe to map.
    pub fn take_hdr_readback(
        &mut self,
    ) -> anyhow::Result<Option<crate::hdr_readback::HdrReadback>> {
        self.graph.take_hdr_readback(&self.runtime.context.device)
    }

    /// Immutable borrow of the render graph (passes + declared resources +
    /// settings). Exposed for the render-graph visualizer (F2): the viz takes a
    /// per-frame `snapshot()` from this and reads live per-pass state via
    /// `pass_ref::<T>()`. Read-only - no mutation path is exposed.
    pub fn graph(&self) -> &RenderGraph {
        &self.graph
    }

    /// Mutable borrow of the render graph. Used internally for lifecycle ops
    /// and by the app layer to reach passes (e.g. PathTracePass::set_geometry)
    /// after construction.
    pub fn graph_mut(&mut self) -> &mut RenderGraph {
        &mut self.graph
    }

    /// Current render settings.
    pub fn settings(&self) -> &RenderSettings {
        &self.settings
    }

    /// Mutable reference to current render settings.
    pub fn settings_mut(&mut self) -> &mut RenderSettings {
        &mut self.settings
    }

    /// 设置本帧要画选中描边的 ECS 实体 id（#487）。
    ///
    /// 编辑器把 `workspace.selection` 映射成 `prism_ecs::Entity::id()` 后调用；
    /// 空切片关闭描边。等待绘制期间列表会被复制，调用方不需要保持借用。
    pub fn set_outline_entities(&mut self, entities: &[u32]) {
        self.outline_entities.clear();
        self.outline_entities.extend_from_slice(entities);
    }

    /// 当前选中描边实体列表（测试 / 调试用）。
    pub fn outline_entities(&self) -> &[u32] {
        &self.outline_entities
    }

    /// Request a path-tracer accumulation reset on the next frame. Call this
    /// when a render parameter that affects traced radiance changes (max
    /// bounces, exposure, light color/direction/intensity, scene reload, ...).
    /// No-op if PathTracePass isn't in the graph.
    pub fn request_pt_reset(&mut self) {
        #[cfg(feature = "pathtrace")]
        if let Some(pt) = self.graph.pass_mut::<PathTracePass>() {
            pt.request_reset();
        }
    }

    /// Select the path tracer's transport variant (consumer, cache-free
    /// reference or SHaRC trainer). Resets accumulation because the variants
    /// produce different radiance. No-op if PathTracePass isn't in the graph.
    #[cfg(feature = "pathtrace")]
    pub fn set_pt_variant(&mut self, variant: crate::pt_pass::PtVariant) {
        if let Some(pt) = self.graph.pass_mut::<PathTracePass>() {
            pt.set_variant(variant);
            pt.request_reset();
        }
    }

    /// Active path-tracer variant, or `None` when the pass isn't registered.
    #[cfg(feature = "pathtrace")]
    pub fn pt_variant(&self) -> Option<crate::pt_pass::PtVariant> {
        self.graph
            .pass_ref::<PathTracePass>()
            .map(|pt| pt.variant())
    }

    /// Current PT frame counter (number of accumulated samples per pixel).
    /// Capped at `pt_max_iterations` when freeze is active (> 0), so the UI
    /// doesn't keep counting after the shader stops accumulating.
    /// Returns `None` if the path-trace pass is not in the graph.
    pub fn pt_frame_count(&self) -> Option<u32> {
        // 不用 `return`：关掉 `pathtrace` 时函数体只剩这一句，`return None;`
        // 会让 clippy 的 `needless_return` 直接报错（-D warnings 下编译失败）。
        #[cfg(not(feature = "pathtrace"))]
        {
            None
        }
        #[cfg(feature = "pathtrace")]
        self.graph.pass_ref::<PathTracePass>().map(|pt| {
            let fc = pt.frame_count();
            let max = self.settings.pt_max_iterations;
            if max > 0 && fc > max {
                max
            } else {
                fc
            }
        })
    }

    /// 注入外部叠加层（宿主 UI 等）。主线程在启动渲染线程前调用；
    /// overlay 的 GPU 资源由实现方在 `record` 时懒创建。
    pub fn set_external_overlay(&mut self, overlay: Box<dyn SwapchainOverlay>) {
        self.ui_host.set_external_overlay(overlay);
    }

    /// 把一条类型擦除的消息（如"新的宿主 UI 帧"）投递给外部叠加层。
    /// 渲染线程从 shared 通路取出消息后调用；无叠加层时静默丢弃。
    pub fn apply_overlay_message(&mut self, msg: OverlayMessage) {
        self.ui_host.apply_overlay_message(msg);
    }

    /// Apply this frame's TAA subpixel jitter to `frame_data` and remember the
    /// NDC offset for `TaaPass`. Call once per frame before `prepare` (#883).
    pub fn apply_taa_jitter(&mut self, frame_data: &mut FrameUBOData) {
        let active = self.settings.taa
            || self.settings.anf_sr
            || self.settings.mobile_upscaler.is_temporal_compute();
        self.temporal.apply_jitter(frame_data, active);
    }

    /// Enable/disable TAA and set its history weight (#883).
    pub fn set_taa(&mut self, enabled: bool, strength: f32) {
        if !enabled && self.settings.taa {
            self.temporal.taa_frame = 0;
        }
        self.settings.taa = enabled;
        self.settings.taa_strength = strength.clamp(0.0, 0.95);
    }

    pub fn set_anf_sr(&mut self, enabled: bool, sharpness: f32) {
        self.settings.anf_sr = enabled;
        self.settings.anf_sharpness = sharpness.clamp(0.0, 1.0);
    }

    pub fn set_mobile_upscaler(
        &mut self,
        mode: crate::render_graph::MobileUpscaler,
        sharpness: f32,
    ) {
        if !mode.is_temporal_compute()
            && self.settings.mobile_upscaler.is_temporal_compute()
            && !self.settings.taa
            && !self.settings.anf_sr
        {
            self.temporal.taa_frame = 0;
        }
        self.settings.mobile_upscaler = mode;
        self.settings.upscaler_sharpness = sharpness.clamp(0.0, 1.0);
    }

    pub fn set_internal_resolution(
        &mut self,
        resolution: [u32; 2],
        mode: crate::render_graph::InternalScaleMode,
    ) {
        if self.settings.internal_resolution == resolution
            && self.settings.internal_scale_mode == mode
        {
            return;
        }
        self.settings.internal_resolution = resolution;
        self.settings.internal_scale_mode = mode;
    }

    /// 场景实际渲染 extent（内部渲染分辨率生效时小于 [`Self::extent`]）。
    pub fn render_extent(&self) -> vk::Extent2D {
        let swapchain = self.extent();
        crate::render_graph::internal_extent(swapchain, self.settings.internal_resolution)
            .unwrap_or(swapchain)
    }

    pub fn extent(&self) -> vk::Extent2D {
        self.swapchain
            .as_ref()
            .map(|s| s.extent)
            .or_else(|| self.offscreen.as_ref().map(|o| o.extent()))
            .unwrap_or_default()
    }

    pub fn orientation(&self) -> (f32, [[f32; 4]; 4]) {
        use vk::SurfaceTransformFlagsKHR as T;
        let extent = self.extent();
        let transform = self
            .swapchain
            .as_ref()
            .map(|s| s.pre_transform())
            .unwrap_or(T::IDENTITY);
        let portrait_buffer = extent.width < extent.height;
        let (dw, dh) = if portrait_buffer {
            (extent.height, extent.width)
        } else {
            (extent.width, extent.height)
        };
        let angle = match transform {
            T::ROTATE_90 => std::f32::consts::FRAC_PI_2,
            T::ROTATE_270 => -std::f32::consts::FRAC_PI_2,
            T::ROTATE_180 => std::f32::consts::PI,
            _ => 0.0,
        };
        let aspect = if dh == 0 { 1.0 } else { dw as f32 / dh as f32 };
        let (s, c) = angle.sin_cos();
        let rotation = [
            [c, s, 0.0, 0.0],
            [-s, c, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        (aspect, rotation)
    }

    pub fn has_swapchain(&self) -> bool {
        self.swapchain.is_some()
    }

    /// Pre‑compile all lazy‑created pipelines so the first frame does not
    /// stall on pipeline creation. Call once after construction, before any
    /// [`execute`](Self::execute).
    pub fn warmup_pipelines(&mut self) -> anyhow::Result<()> {
        let device = self.runtime.context.device.clone();
        self.graph.warmup_passes(&device, &self.runtime.context)
    }

    pub fn suspend_surface(&mut self) {
        if self.is_headless {
            return;
        }
        let device = &self.runtime.context.device;
        unsafe { device.device_wait_idle() }.ok();
        if let Some(mut sw) = self.swapchain.take() {
            unsafe { sw.destroy(device) };
        }
        log::info!("GraphRenderer suspended");
    }

    pub fn resume_surface(
        &mut self,
        window: &dyn raw_window_handle::HasDisplayHandle,
        window_handle: &dyn raw_window_handle::HasWindowHandle,
    ) -> anyhow::Result<()> {
        if self.is_headless || self.swapchain.is_some() {
            return Ok(());
        }
        let swapchain =
            Swapchain::new_with_vsync(&self.runtime.context, window, window_handle, self.vsync)?;
        // 恢复路径与启动路径同样需要同步 ForwardPass 的 per-image 槽位数
        // （见 `GraphRenderer::new` 里的注释）。
        let image_count = swapchain.views.len();
        self.swapchain = Some(swapchain);
        if let Some(forward_pass) = self.graph.pass_mut::<ForwardPass>() {
            forward_pass.set_image_count(image_count);
        }
        log::info!("GraphRenderer resumed");
        Ok(())
    }

    pub fn recreate_swapchain(&mut self) -> anyhow::Result<()> {
        if self.is_headless {
            return Ok(());
        }
        let Some(swapchain) = self.swapchain.as_ref() else {
            return Ok(());
        };
        let desired_extent = swapchain.desired_extent(self.runtime.context.physical_device)?;
        if desired_extent.width == 0 || desired_extent.height == 0 {
            self.surface_suspended = true;
            log::debug!(
                "swapchain recreate deferred while surface is suspended ({}x{})",
                desired_extent.width,
                desired_extent.height
            );
            return Ok(());
        }
        // From this point on, any early return must keep frame acquisition
        // suspended until the swapchain and lazy pass targets are rebuilt.
        self.surface_suspended = true;
        // Wait for the GPU to finish all in-flight work BEFORE destroying any
        // framebuffers. The previous frame's command buffer references both
        // the ForwardPass framebuffers and the external overlay framebuffers; without
        // this wait, vkDestroyFramebuffer fires while a command buffer is still
        // executing (VUID-vkDestroyFramebuffer-framebuffer-00892).
        unsafe { self.runtime.context.device.device_wait_idle() }
            .context("recreate_swapchain: device_wait_idle")?;

        // 放置 the ForwardPass 帧缓冲 + 深度 图像 BEFORE the 交换链 is
        // recreated: the framebuffer wraps a swapchain image view, and
        // `Swapchain::recreate` destroys the old views. Destroying the views
        // while the framebuffer still references them triggers a validation
        // error (VUID-vkDestroyImageView-imageView-01026) which cascades into a
        // device-lost on the next queue submit.
        //
        // This is the single entry point for swapchain recreation - the
        // acquire/present out-of-date paths in `render` also route through
        // here so the framebuffer is always torn down first.
        if let Some(forward_pass) = self.graph.pass_mut::<ForwardPass>() {
            forward_pass.drop_target(&self.runtime.context.device);
        }
        // #883 Phase 2: the previous camera/model history refers to the old
        // extent; clear it so the first frame after a resize does not emit
        // bogus motion vectors.
        self.temporal.reset_history();
        #[cfg(feature = "sharc")]
        if let Some(resolve) = self.graph.pass_mut::<crate::sharc::SharcResolveNode>() {
            resolve.reset_history();
        }
        // PostPass wraps swapchain views too (its framebuffers target the
        // swapchain directly). Drop them on the same lifecycle.
        if let Some(post) = self.graph.pass_mut::<crate::post::PostPass>() {
            post.drop_target(&self.runtime.context.device);
        }
        // ECS UI 叠加按 image view 缓存 framebuffer；必须在 `sw.recreate()`
        // 销毁旧 image view **之前**释放，否则 framebuffer 会引用已销毁的
        // view（VUID-vkDestroyImageView-imageView-01026），可导致验证错误/
        // device lost。此处 device 已 idle。
        self.ui_host.on_swapchain_pre_recreate();

        if let Some(sw) = self.swapchain.as_mut() {
            if !sw.recreate_if_drawable(&self.runtime.context)? {
                self.surface_suspended = true;
                return Ok(());
            }
        }

        // 通知外部叠加层：per-swapchain framebuffers 已按新 views 重建。
        if let Some(sw) = self.swapchain.as_ref() {
            self.ui_host
                .on_swapchain_post_recreate(&sw.views, sw.extent);
        }

        // 依赖交换链尺寸/图像数的 pass 必须在 `sw.recreate()` **之后**按新值
        // resize：recreate 之前 `sw.extent` 与 `sw.views.len()` 还是旧交换链的，
        // `GtaoPass::recreate_target` 拿旧的半分辨率做早退（old/2 == 旧 extent），
        // 而 `set_image_count` 会让 `ForwardPass::ensure_target` 缺少新图像对应的
        // 槽位（`image_index >= image_count` 直接 return）。
        if let Some((extent, image_count)) = self
            .swapchain
            .as_ref()
            .map(|sw| (sw.extent, sw.views.len()))
        {
            if let Some(forward_pass) = self.graph.pass_mut::<ForwardPass>() {
                forward_pass.set_image_count(image_count);
            }
            // GTAO 拥有自己的 AO 图像（不来自交换链），但尺寸是交换链的一半。
            #[cfg(feature = "gtao")]
            if let Some(gtao) = self.graph.pass_mut::<crate::gtao::GtaoPass>() {
                if let Err(e) =
                    gtao.recreate_target(&self.runtime.context, self.runtime.command_pool, extent)
                {
                    log::warn!("GtaoPass recreate_target failed: {e:#}");
                }
            }
            if let Some(taa) = self.graph.pass_mut::<crate::taa::TaaPass>() {
                if let Err(e) = taa.recreate_target(&self.runtime.context, extent) {
                    log::warn!("TaaPass recreate_target failed: {e:#}");
                }
            }
            if let Some(upscaler) = self
                .graph
                .pass_mut::<crate::mobile_upscaler::MobileUpscalerPass>()
            {
                if let Err(e) = upscaler.recreate_target(&self.runtime.context, extent) {
                    log::warn!("MobileUpscalerPass recreate_target failed: {e:#}");
                }
            }
            if let Some(voxel) = self.graph.pass_mut::<crate::voxel_dda_pass::VoxelDdaPass>() {
                voxel.drop_targets();
            }
            #[cfg(feature = "anf")]
            {
                let render_extent = self.render_extent();
                if let Some(anf) = self.graph.pass_mut::<crate::anf::AnfPass>() {
                    if let Err(e) =
                        anf.recreate_target(&self.runtime.context, render_extent, extent)
                    {
                        log::warn!("AnfPass recreate_target failed: {e:#}");
                    }
                }
            }
            // CRT 调度器的 RenderTextures 跟随 swapchain 尺寸（演示 RT 全屏）。
            if let Some(sched) = self
                .graph
                .pass_mut::<crate::rt_scheduler::RenderTextureScheduler>()
            {
                if let Err(e) = sched.resize_all(
                    &self.runtime.context,
                    self.texture_manager.bindless_mut(),
                    extent,
                ) {
                    log::warn!("RenderTextureScheduler resize_all failed: {e:#}");
                }
            }
        }

        // All per-swapchain-image attachments (ForwardPass HDR/depth/normal,
        // PostPass framebuffer) were just rebuilt, so the render graph's cached
        // image layouts are stale. Clear them so the first frame after
        // recreate re-transitions from UNDEFINED instead of trusting a layout
        // that no longer matches the fresh images.
        self.graph.reset_layouts();
        self.surface_suspended = false;
        Ok(())
    }

    /// 调整**离屏**渲染目标尺寸（编辑器视口面板改大小时走这里）。
    ///
    /// 顺序与 [`Self::recreate_swapchain`] 一致：先 `device_wait_idle`，再拆掉
    /// 引用旧视图的 framebuffer，然后换目标、按新尺寸重建依赖 pass，最后清掉
    /// 图缓存里的 layout（新图像从 UNDEFINED 重新过渡）。
    pub fn resize_offscreen(&mut self, width: u32, height: u32) -> anyhow::Result<()> {
        let width = width.max(1);
        let height = height.max(1);
        let Some(target) = self.offscreen.as_ref() else {
            return Ok(());
        };
        if target.extent().width == width && target.extent().height == height {
            return Ok(());
        }

        unsafe { self.runtime.context.device.device_wait_idle() }
            .context("resize_offscreen: device_wait_idle")?;

        if let Some(forward_pass) = self.graph.pass_mut::<ForwardPass>() {
            forward_pass.drop_target(&self.runtime.context.device);
        }
        // #883 Phase 2: the previous camera/model history refers to the old
        // extent; clear it so the first frame after a resize does not emit
        // bogus motion vectors.
        self.temporal.reset_history();
        #[cfg(feature = "sharc")]
        if let Some(resolve) = self.graph.pass_mut::<crate::sharc::SharcResolveNode>() {
            resolve.reset_history();
        }
        if let Some(post) = self.graph.pass_mut::<crate::post::PostPass>() {
            post.drop_target(&self.runtime.context.device);
        }
        // UI 叠加按 image view 缓存 framebuffer，必须先于旧视图销毁释放。
        self.ui_host.on_offscreen_pre_recreate();

        let context = self.runtime.context.clone();
        let fresh = OffscreenTarget::new(&context, width, height)?;
        let extent = fresh.extent();
        if let Some(mut old) = self.offscreen.replace(fresh) {
            unsafe { old.destroy(&context.device) };
        }
        let view = self
            .offscreen
            .as_ref()
            .map(|target| target.image_view())
            .unwrap_or(vk::ImageView::null());

        if let Some(forward_pass) = self.graph.pass_mut::<ForwardPass>() {
            forward_pass.set_image_count(1);
            forward_pass.set_target(&context.device, &context, 1, 0, extent)?;
        }
        if let Some(post) = self.graph.pass_mut::<crate::post::PostPass>() {
            post.set_target(&context.device, std::slice::from_ref(&view), 0, extent)?;
        }
        #[cfg(feature = "gtao")]
        if let Some(gtao) = self.graph.pass_mut::<crate::gtao::GtaoPass>() {
            if let Err(e) = gtao.recreate_target(&context, self.runtime.command_pool, extent) {
                log::warn!("GtaoPass recreate_target failed (offscreen resize): {e:#}");
            }
        }
        if let Some(taa) = self.graph.pass_mut::<crate::taa::TaaPass>() {
            if let Err(e) = taa.recreate_target(&context, extent) {
                log::warn!("TaaPass recreate_target failed (offscreen resize): {e:#}");
            }
        }
        if let Some(upscaler) = self
            .graph
            .pass_mut::<crate::mobile_upscaler::MobileUpscalerPass>()
        {
            if let Err(e) = upscaler.recreate_target(&context, extent) {
                log::warn!("MobileUpscalerPass recreate_target failed (offscreen resize): {e:#}");
            }
        }
        if let Some(voxel) = self.graph.pass_mut::<crate::voxel_dda_pass::VoxelDdaPass>() {
            voxel.drop_targets();
        }
        #[cfg(feature = "anf")]
        {
            let render_extent = self.render_extent();
            if let Some(anf) = self.graph.pass_mut::<crate::anf::AnfPass>() {
                if let Err(e) = anf.recreate_target(&context, render_extent, extent) {
                    log::warn!("AnfPass recreate_target failed (offscreen resize): {e:#}");
                }
            }
        }
        if let Some(sched) = self
            .graph
            .pass_mut::<crate::rt_scheduler::RenderTextureScheduler>()
        {
            if let Err(e) = sched.resize_all(&context, self.texture_manager.bindless_mut(), extent)
            {
                log::warn!("RenderTextureScheduler resize_all failed (offscreen resize): {e:#}");
            }
        }

        self.graph.reset_layouts();
        Ok(())
    }

    /// Supply the three-tier sparse voxel scene payload for subsequent frames.
    pub fn set_voxel_scene(&mut self, scene: Option<Arc<crate::voxel_dda_pass::VoxelSceneUpload>>) {
        if let Some(voxel) = self.graph.pass_mut::<crate::voxel_dda_pass::VoxelDdaPass>() {
            voxel.set_scene(scene);
        }
    }
    // -------------------------------------------------------------------
    // Frame lifecycle — phase API
    // -------------------------------------------------------------------

    /// Phase 1/3: acquire swapchain image, reset & begin the command buffer.
    ///
    /// Returns a [`FrameCtx`] carrying the per-frame Vulkan handles. On
    /// swapchain out-of-date returns `Ok(None)` — the caller should return
    /// early (the swapchain was recreated internally). On real error returns
    /// `Err`.
    ///
    /// In headless mode always returns the command buffer from the runtime
    /// pool (no acquire needed).
    pub fn begin_frame(&mut self) -> anyhow::Result<Option<FrameCtx>> {
        let device = self.runtime.context.device.clone();

        // Headless: no acquire, just grab cmd buf[0].
        if self.is_headless {
            if self.headless_fence != vk::Fence::null() {
                unsafe {
                    device
                        .wait_for_fences(&[self.headless_fence], true, u64::MAX)
                        .context("wait for headless frame fence")?;
                    device
                        .reset_fences(&[self.headless_fence])
                        .context("reset headless frame fence")?;
                }
            }

            self.telemetry.begin_frame_timings(&device, 0);

            let cmd = self.runtime.command_buffers[0];
            unsafe { device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty()) }
                .context("reset command buffer (headless)")?;
            let begin_info = vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            unsafe { device.begin_command_buffer(cmd, &begin_info) }
                .context("begin command buffer (headless)")?;
            self.telemetry.begin_frame_span(&device, cmd, 0);

            // Re-use the offscreen target extent as a stand-in.
            let extent = self
                .offscreen
                .as_ref()
                .map(|o| o.extent())
                .unwrap_or(vk::Extent2D {
                    width: 256,
                    height: 256,
                });

            let render_extent =
                crate::render_graph::internal_extent(extent, self.settings.internal_resolution)
                    .unwrap_or(extent);
            // 上一帧的深度拷贝已经随 headless fence 完成，可以安全读回。
            self.collect_occlusion_depth(0, render_extent);
            return Ok(Some(FrameCtx {
                device,
                cmd,
                image_index: 0,
                frame_index: 0,
                extent,
                render_extent,
                fence: self.headless_fence,
                image_available: vk::Semaphore::null(),
                render_finished: vk::Semaphore::null(),
            }));
        }
        if self.surface_suspended {
            let desired_extent = self
                .swapchain
                .as_ref()
                .context("begin_frame called with no swapchain")?
                .desired_extent(self.runtime.context.physical_device)?;
            if desired_extent.width == 0 || desired_extent.height == 0 {
                return Ok(None);
            }
            self.recreate_swapchain()?;
            if self.surface_suspended {
                return Ok(None);
            }
        }

        let device = self.runtime.context.device.clone();

        // --- Acquire next image ---
        let (image_index, frame, image_available, render_finished, fence) = match self
            .swapchain
            .as_mut()
            .context("begin_frame called with no swapchain")?
            .acquire_next_image(&device)
        {
            Ok(v) => v,
            Err(e) => {
                let msg = format!("{e}");
                if msg.contains("out of date") {
                    log::debug!("acquire out of date, recreating");
                    self.recreate_swapchain()?;
                    return Ok(None);
                }
                return Err(e);
            }
        };

        let cmd = self.runtime.command_buffers[frame];
        let extent = self.extent();
        let render_extent =
            crate::render_graph::internal_extent(extent, self.settings.internal_resolution)
                .unwrap_or(extent);

        // 上一帧（同一帧槽位）的时间戳结果现在一定写好了（acquire 已等过栅栏）。
        self.telemetry.begin_frame_timings(&device, frame as u32);
        // 同一张图像的深度回读缓冲也已经写完，可以映射。
        self.collect_occlusion_depth(image_index, render_extent);

        // --- Reset & begin command buffer ---
        unsafe { device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty()) }
            .context("reset command buffer")?;
        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        unsafe { device.begin_command_buffer(cmd, &begin_info) }.context("begin command buffer")?;
        self.telemetry.begin_frame_span(&device, cmd, frame as u32);

        Ok(Some(FrameCtx {
            device,
            cmd,
            image_index,
            frame_index: frame as u32,
            extent,
            render_extent,
            fence,
            image_available,
            render_finished,
        }))
    }

    /// 最近一帧的 per-pass GPU 时间。
    ///
    /// `supported = false`（或 `passes` 为空）表示设备不支持时间戳，调用方应回退到
    /// CPU 墙钟（`frame_time_ms`）。结果比当前帧**晚一帧**：Vulkan 的时间戳只在
    /// 提交完成后才可读，延迟读回才不会 stall 管线。
    pub fn gpu_timings(&self) -> &crate::gpu_timer::GpuTimings {
        self.telemetry.gpu_timings()
    }

    /// 最近 N 帧的 GPU 总时间（旧 → 新，#518 Phase 2 历史曲线）。
    pub fn gpu_history(&self) -> &crate::gpu_timer::GpuHistory {
        self.telemetry.gpu_history()
    }

    /// 本帧的 draw call 记录（#518 Phase 2 的数据层，不含回放）。
    ///
    /// 网格按 `draw_items` 顺序逐条记录（即使被剔除也记下来，`visible=false`
    /// 让面板能解释“为什么没画”）；精灵与粒子按实际批次合并（与 `SpritePass`
    /// 的连续相同混合状态分批一致）。超过 [`MAX_REPORTED_DRAW_CALLS`] 的部分
    /// 直接丢弃，避免大场景把报告撑到无法阅读。
    /// 一条 draw call 的管线状态摘要（#518 Frame Debugger）。
    fn draw_state_summary(state: &crate::render_graph::DrawState) -> String {
        format!(
            "queue {} · cull {:?} · z-write {} · {:?}/{:?} {:?}",
            state.render_queue,
            state.cull_mode,
            state.z_write,
            state.src_blend,
            state.dst_blend,
            state.blend_op,
        )
    }

    pub fn draw_call_report(
        &self,
        draw_items: &[crate::render_graph::DrawItem],
        sprite_items: &[crate::render_graph::SpriteDrawItem],
        particle_items: &[crate::render_graph::ParticleDrawItem],
    ) -> Vec<crate::render_graph_report::ReportDrawCall> {
        use crate::render_graph_report::{ReportDrawCall, MAX_REPORTED_DRAW_CALLS};
        let mut calls: Vec<ReportDrawCall> = Vec::new();

        for (index, item) in draw_items.iter().enumerate() {
            if calls.len() >= MAX_REPORTED_DRAW_CALLS {
                return calls;
            }
            let uploaded = self.mesh_manager.get(item.mesh);
            let triangles = uploaded
                .map(|uploaded| {
                    let indices = item.index_count.unwrap_or(uploaded.mesh.index_count);
                    (indices as u64 / 3).max(1)
                })
                .unwrap_or(0);
            // 与 `ForwardPass::pipeline_for` 的缓存键一致：混合 / 不写深度是
            // 独立的 pipeline 变体，回放时能看出为什么换了管线。
            let pipeline = if item.state.blends() {
                "forward/transparent"
            } else if item.state.z_write {
                "forward/opaque"
            } else {
                "forward/opaque-no-zwrite"
            };
            calls.push(ReportDrawCall {
                pass: "forward".to_string(),
                label: format!("mesh #{index} · material {}", item.material.unwrap_or(0)),
                instances: 1,
                triangles,
                visible: item.visible_in_main_pass,
                state: Some(Self::draw_state_summary(&item.state)),
                pipeline: Some(pipeline.to_string()),
                descriptor_sets: [
                    "frame+materials",
                    "bindless-textures",
                    "ibl",
                    "shadow",
                    "gtao",
                    "gi-probes",
                ]
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
                vertices: uploaded.map(|uploaded| uploaded.vertex_count() as u64),
            });
        }

        let mut cursor = 0;
        while cursor < sprite_items.len() && calls.len() < MAX_REPORTED_DRAW_CALLS {
            let blend = |item: &crate::render_graph::SpriteDrawItem| {
                (item.src_blend, item.dst_blend, item.blend_op)
            };
            let key = blend(&sprite_items[cursor]);
            let start = cursor;
            while cursor < sprite_items.len() && blend(&sprite_items[cursor]) == key {
                cursor += 1;
            }
            let count = cursor - start;
            calls.push(ReportDrawCall {
                pass: "sprite".to_string(),
                label: format!("{:?} / {:?}", key.0, key.1),
                instances: count,
                triangles: count as u64 * 2,
                visible: true,
                state: Some(format!(
                    "cull off · z-write false · {:?}/{:?} {:?}",
                    key.0, key.1, key.2
                )),
                pipeline: Some("sprite/batch".to_string()),
                descriptor_sets: ["bindless-textures", "sprite-lights"]
                    .iter()
                    .map(|name| (*name).to_string())
                    .collect(),
                vertices: Some(count as u64 * 4),
            });
        }

        if !particle_items.is_empty() && calls.len() < MAX_REPORTED_DRAW_CALLS {
            calls.push(ReportDrawCall {
                pass: "particle".to_string(),
                label: "billboards".to_string(),
                instances: particle_items.len(),
                triangles: particle_items.len() as u64 * 2,
                visible: true,
                state: Some("billboard · depth test on · z-write false".to_string()),
                pipeline: Some("particle/billboard".to_string()),
                descriptor_sets: Vec::new(),
                vertices: Some(particle_items.len() as u64 * 4),
            });
        }
        calls
    }

    /// 纹理 / 网格 / 材质与显存子分配的实时驻留统计（#518 Phase 2, #1030）。
    pub fn render_stats(&self) -> crate::render_graph_report::ReportStats {
        let (textures, texture_bytes) = self.texture_manager.stats();
        let (meshes, mesh_bytes) = self.mesh_manager.stats();
        let (materials, material_bytes) = self.material_manager.stats();
        let suballoc = self.suballocator.stats();
        crate::render_graph_report::ReportStats {
            textures,
            texture_bytes,
            meshes,
            mesh_bytes,
            materials,
            material_bytes,
            occlusion_tested: self.occlusion.counts.tested,
            occlusion_culled: self.occlusion.counts.culled,
            suballoc_reserved_bytes: suballoc.total_reserved_bytes,
            suballoc_active_bytes: suballoc.active_used_bytes,
        }
    }

    /// 获取显存子分配器引用（#1030）。
    pub fn suballocator(&self) -> &crate::suballocator::MemorySuballocator {
        &self.suballocator
    }

    /// 获取显存子分配器可变引用（#1030）。
    pub fn suballocator_mut(&mut self) -> &mut crate::suballocator::MemorySuballocator {
        &mut self.suballocator
    }

    /// 获取显存子分配器状态统计（#1030）。
    pub fn suballocator_stats(&self) -> crate::suballocator::SuballocatorStats {
        self.suballocator.stats()
    }

    /// 准备阶段：在命令录制前批量同步 CPU 场景状态和 GPU 资源（DESIGN §9 / #1157）。
    /// 统一消费 `DirtyDispatchPlan` 并收敛执行暂存的批量上传与释放。
    pub fn prepare(&mut self, _ctx: &FrameCtx, input: &FrameInput<'_>) -> anyhow::Result<()> {
        // #502：LUT 文件只在路径变化时重新读取/上传，避免每帧 I/O。
        self.ensure_lut(input.lut_path)?;

        // #1157 消费统一 DirtyDispatchPlan 并记录分派
        if let Some(plan) = input.dispatch_plan {
            if plan.pt_accum_dirty {
                log::trace!("prepare: pt accum dirty (from dispatch plan)");
            }
            if plan.has_work() {
                log::trace!(
                    "prepare: dispatch plan tex_up={} mesh_up={} mat_up={} lights={} inst={}",
                    plan.texture_uploads,
                    plan.mesh_uploads,
                    plan.material_updates,
                    plan.lights_dirty,
                    plan.instances_dirty,
                );
            }
        } else if input.pt_accum_dirty {
            log::trace!("prepare: pt accum dirty — will reset accumulation");
        }

        if input.draw_items.is_empty() && !input.has_camera {
            log::trace!("prepare: no draw_items and no camera — frame will clear only");
        }

        // 统一在 prepare 阶段收敛并批量执行暂存的 GPU 资产操作
        self.flush_staged_assets()?;

        if input.render_mode == crate::render_graph::RenderMode::Raster && !self.ibl_has_pbr {
            if let Some(bytes) = self.env_source.take() {
                let res = self.rebuild_ibl_with_scope(Some(&bytes), crate::ibl::IblScope::Full);
                self.env_source = Some(bytes);
                res?;
            }
        }

        #[cfg(feature = "pathtrace")]
        if input.render_mode == crate::render_graph::RenderMode::PathTrace {
            let needs_geom = self
                .graph
                .pass_ref::<PathTracePass>()
                .map(|pt| !pt.has_geometry())
                .unwrap_or(false);
            if needs_geom && !input.draw_items.is_empty() {
                self.sync_pt_geometry(input.draw_items)?;
            }
        }
        // The ReSTIR GI pass traces the same world-space scene, but it runs in
        // raster mode, where nothing else uploads geometry. Build its own copy
        // of the acceleration structures from the same draw list.
        #[cfg(feature = "restir_gi")]
        if input.render_mode == crate::render_graph::RenderMode::Raster
            && self.settings.raster_gi == crate::render_graph::RasterGiMode::Restir
        {
            let needs_geom = self
                .graph
                .pass_ref::<crate::restir_gi::RestirGiPass>()
                .map(|pass| !pass.has_geometry())
                .unwrap_or(false);
            if needs_geom && !input.draw_items.is_empty() {
                self.sync_restir_gi_geometry(input.draw_items)?;
            }
        }
        if input.render_mode == crate::render_graph::RenderMode::Raster
            && self.settings.raster_gi == crate::render_graph::RasterGiMode::Ddgi
        {
            if !self.runtime.context.rt_caps.has_ray_query() {
                static DDGI_NO_RQ_WARNED: std::sync::Once = std::sync::Once::new();
                DDGI_NO_RQ_WARNED.call_once(|| {
                    log::warn!(
                        "raster_gi = Ddgi requested, but VK_KHR_ray_query is unavailable; falling back to BakedProbe"
                    );
                });
            } else {
                if self.ddgi_pass.is_none() {
                    let params = crate::ddgi::DdgiVolumeParams::default();
                    match crate::ddgi_pass::DdgiPass::new(
                        self.runtime.context.clone(),
                        self.texture_manager.bindless().layout,
                        self.ibl.descriptor_set_layout,
                        params,
                    ) {
                        Ok(pass) => {
                            let info = crate::gi::ProbeVolumeInfo::new(
                                pass.active_origin(),
                                params.probe_spacing,
                                params.probe_counts,
                            );
                            if let Err(e) = self
                                .scene_scope
                                .ensure_probe_volume_for_ddgi(params.probe_counts, &info)
                            {
                                log::warn!("DDGI probe volume setup failed: {e:#}");
                            } else {
                                self.ddgi_pass = Some(pass);
                            }
                        }
                        Err(e) => log::warn!("Failed to initialize DdgiPass: {e:#}"),
                    }
                }
                let needs_geom = self
                    .ddgi_pass
                    .as_ref()
                    .is_some_and(|pass| !pass.has_geometry());
                if needs_geom && !input.draw_items.is_empty() {
                    let instances = self.collect_rt_instances(input.draw_items);
                    if !instances.is_empty() {
                        if let Some(ddgi) = &mut self.ddgi_pass {
                            ddgi.set_geometry(self.runtime.command_pool, &instances)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Build the ReSTIR GI ray-tracing scene from the draw list.
    ///
    /// Mirrors [`Self::sync_pt_geometry`] but targets [`crate::restir_gi::RestirGiPass`]:
    /// the GI pass owns its acceleration structures so it stays independent of
    /// the path tracer (it must work with `pathtrace` compiled out).
    #[cfg(feature = "restir_gi")]
    pub fn sync_restir_gi_geometry(
        &mut self,
        draw_items: &[crate::render_graph::DrawItem],
    ) -> anyhow::Result<()> {
        let instances = self.collect_rt_instances(draw_items);
        if instances.is_empty() {
            log::debug!("sync_restir_gi_geometry: no CPU geometry found in draw_items");
            return Ok(());
        }
        if let Some(pass) = self.graph.pass_mut::<crate::restir_gi::RestirGiPass>() {
            pass.set_geometry(&self.runtime.context, self.runtime.command_pool, &instances)?;
        }
        Ok(())
    }

    /// Build world-space ray-tracing instances from the draw list.
    ///
    /// Mirrors the instance construction inside [`Self::sync_pt_geometry`] so the
    /// ReSTIR GI / DDGI passes see exactly the same geometry (same vertex transform,
    /// same index slice, same material slot and alpha semantics) as the path tracer.
    fn collect_rt_instances(
        &self,
        draw_items: &[crate::render_graph::DrawItem],
    ) -> Vec<crate::bake_common::PtGeometryInstance> {
        let mut instances = Vec::new();
        for item in draw_items {
            let Some((verts, indices)) = self.mesh_manager.get_cpu_geometry(item.mesh) else {
                continue;
            };
            let model = &item.model;
            let world_vertices: Vec<crate::mesh::Vertex> = verts
                .iter()
                .map(|v| {
                    let p = crate::math::transform_point(model, v.position);
                    let n = crate::math::transform_vector_normalized(model, v.normal);
                    let t = crate::math::transform_vector_normalized(
                        model,
                        [v.tangent[0], v.tangent[1], v.tangent[2]],
                    );
                    crate::mesh::Vertex {
                        position: p,
                        normal: n,
                        color: v.color,
                        uv: v.uv,
                        tangent: [t[0], t[1], t[2], v.tangent[3]],
                    }
                })
                .collect();
            let sub_indices: Vec<u32> = match item.index_count {
                Some(count) => {
                    let start = item.index_start as usize;
                    let count = count as usize;
                    if start + count <= indices.len() {
                        indices[start..start + count].to_vec()
                    } else {
                        indices.to_vec()
                    }
                }
                None => indices.to_vec(),
            };
            let material_slot = item.material.unwrap_or(0);
            let opaque = item
                .material
                .is_some_and(|_| self.material_manager.is_opaque_slot(material_slot));
            instances.push(crate::bake_common::PtGeometryInstance {
                vertices: world_vertices,
                indices: sub_indices,
                material_slot,
                opaque,
            });
        }
        instances
    }

    /// Upload world-space geometry from draw_items into PathTracePass.
    #[cfg(feature = "pathtrace")]
    pub fn sync_pt_geometry(
        &mut self,
        draw_items: &[crate::render_graph::DrawItem],
    ) -> anyhow::Result<()> {
        let mut instances = Vec::new();
        for item in draw_items {
            if let Some((verts, indices)) = self.mesh_manager.get_cpu_geometry(item.mesh) {
                let model = &item.model;
                let world_vertices: Vec<crate::mesh::Vertex> = verts
                    .iter()
                    .map(|v| {
                        let p = crate::math::transform_point(model, v.position);
                        let n = crate::math::transform_vector_normalized(model, v.normal);
                        let t = crate::math::transform_vector_normalized(
                            model,
                            [v.tangent[0], v.tangent[1], v.tangent[2]],
                        );
                        crate::mesh::Vertex {
                            position: p,
                            normal: n,
                            color: v.color,
                            uv: v.uv,
                            tangent: [t[0], t[1], t[2], v.tangent[3]],
                        }
                    })
                    .collect();

                let sub_indices: Vec<u32> = if let Some(count) = item.index_count {
                    let start = item.index_start as usize;
                    let count = count as usize;
                    if start + count <= indices.len() {
                        indices[start..start + count].to_vec()
                    } else {
                        indices.to_vec()
                    }
                } else {
                    indices.to_vec()
                };

                let material_slot = item.material.unwrap_or(0);
                // Only trust the slot's alpha semantics when the draw item
                // actually names a material: the `0` fallback is just a shading
                // default and may belong to an unrelated opaque material.
                let opaque = item
                    .material
                    .is_some_and(|_| self.material_manager.is_opaque_slot(material_slot));
                instances.push(crate::bake_common::PtGeometryInstance {
                    vertices: world_vertices,
                    indices: sub_indices,
                    material_slot,
                    opaque,
                });
            }
        }

        if instances.is_empty() {
            log::debug!("sync_pt_geometry: no CPU geometry found in draw_items");
            return Ok(());
        }

        {
            let mut lo = [f32::INFINITY; 3];
            let mut hi = [f32::NEG_INFINITY; 3];
            let mut slots = Vec::new();
            for inst in &instances {
                slots.push(inst.material_slot);
                for v in &inst.vertices {
                    for axis in 0..3 {
                        lo[axis] = lo[axis].min(v.position[axis]);
                        hi[axis] = hi[axis].max(v.position[axis]);
                    }
                }
            }
            slots.sort_unstable();
            slots.dedup();
            log::debug!(
                "sync_pt_geometry: {} instances, world bounds {:?}..{:?}, material slots {:?} ({} unique)",
                instances.len(),
                lo,
                hi,
                slots,
                slots.len()
            );
            log::debug!(
                "sync_pt_geometry: material_manager len={} slot0={:?}",
                self.material_manager.len(),
                self.material_manager.debug_slot(0)
            );
        }

        if let Some(pt) = self.graph.pass_mut::<PathTracePass>() {
            pt.set_geometry(&self.runtime.context, self.runtime.command_pool, &instances)?;
        }
        Ok(())
    }

    /// Number of times the world-space SHaRC cache has been cleared since the
    /// renderer was created, including the initial allocation.
    ///
    /// The cache itself is fully device-local, so this counter is how the
    /// lifecycle tests (and the editor's diagnostics) observe that an
    /// invalidation reason reached the cache.
    #[cfg(feature = "sharc")]
    pub fn sharc_clear_count(&self) -> u64 {
        self.sharc_clear_count
    }

    /// How many frames recorded a real SHaRC training dispatch.
    ///
    /// Distinct from "the `SharcTrain` GPU interval was opened": the dispatch is
    /// skipped whenever the scene, materials or cache buffers are not ready, and
    /// the interval is recorded either way. Tests asserting that training ran
    /// must use this, not the GPU timings.
    #[cfg(feature = "sharc")]
    pub fn sharc_train_dispatches(&self) -> u64 {
        // No trainer node exists without the path tracer, so nothing trained.
        #[cfg(feature = "pathtrace")]
        {
            self.graph
                .pass_ref::<crate::pt_pass::SharcTrainPass>()
                .map(|trainer| trainer.dispatches())
                .unwrap_or(0)
        }
        #[cfg(not(feature = "pathtrace"))]
        {
            0
        }
    }

    /// How many frames recorded a real SHaRC resolve dispatch.
    #[cfg(feature = "sharc")]
    pub fn sharc_resolve_dispatches(&self) -> u64 {
        self.graph
            .pass_ref::<crate::sharc::SharcResolveNode>()
            .map(|resolve| resolve.dispatches())
            .unwrap_or(0)
    }

    /// Whether a SHaRC invalidation reason is queued for the next frame the
    /// cache is used. Reasons raised while the cache is idle (raster mode, or
    /// `gi_mode != Sharc`) stay pending until then.
    #[cfg(feature = "sharc")]
    pub fn sharc_invalidation_pending(&self) -> bool {
        self.sharc_invalidation.any()
    }

    /// Invalidate path-tracing geometry, forcing a rebuild on next frame.
    ///
    /// Also drops the world-space SHaRC cache: it stores radiance keyed by
    /// world position, so replacing the geometry leaves radiance that no
    /// longer belongs to any surface (repair spec §8).
    pub fn invalidate_pt_geometry(&mut self) {
        #[cfg(feature = "pathtrace")]
        if let Some(pt) = self.graph.pass_mut::<PathTracePass>() {
            pt.invalidate_geometry();
        }
        #[cfg(feature = "sharc")]
        {
            self.sharc_invalidation.geometry = true;
        }
    }

    /// Phase 2/3: record all render commands into the frame's command buffer.
    ///
    /// Updates the per-frame UBO, builds the [`GraphFrame`], executes the
    /// 渲染 图 (ShadowMap → Scene → GTAO → Post), records the external
    /// overlay if present (or inserts the swapchain-layout barrier), and ends
    /// the command buffer.
    ///
    /// Recording errors are captured and returned, but the command buffer is
    /// **always ended** — even on failure — so that [`present`] can submit a
    /// partial buffer and keep the in-flight fence signaled.
    pub fn execute(&mut self, ctx: &FrameCtx, input: &FrameInput<'_>) -> anyhow::Result<()> {
        let device = &ctx.device;
        let cmd = ctx.cmd;
        let frame = ctx.frame_index as usize;
        let image_index = ctx.image_index;
        let extent = ctx.extent;
        let render_extent = ctx.render_extent;

        let FrameInput {
            draw_items,
            gizmos,
            sprite_items,
            sprite_lights,
            particle_items,
            bone_matrices,
            skybox,
            frame_data,
            cameras,
            light_view_proj,
            light_view_proj1,
            shadow_split,
            shadow_cascade_count,
            inv_projection,
            debug_flags,
            wireframe,
            shaded_wireframe,
            tonemap_mode,
            aa_mode,
            debug_rt,
            grade,
            lut_path: _,
            proj22,
            proj32,
            lights,
            render_mode,
            pt_max_bounces,
            pt_ray_max_distance,
            pt_max_iterations,
            exposure,
            pt_lights,
            pt_accum_dirty,
            has_camera,
            camera_orthographic,
            clear_color,
            shadows_enabled,
            ui_overlay,
            show_grid,
            dispatch_plan,
        } = input;
        #[cfg(not(feature = "ui"))]
        let _ = ui_overlay;
        // Fold this frame's dirty plan into the SHaRC world-cache invalidation.
        //
        // `instances_dirty` is deliberately *not* consulted: the draw list is
        // rebuilt every frame after frustum and shadow-caster culling, so its
        // content (and therefore its signature) changes whenever the camera
        // moves. The cache keys on world position and only needs reprojection
        // for that. Genuine geometry changes reach us through
        // `mesh_releases`/`material_releases` here and through
        // [`Self::invalidate_pt_geometry`] on scene unload.
        #[cfg(feature = "sharc")]
        if let Some(plan) = dispatch_plan {
            if plan.lights_dirty {
                self.sharc_invalidation.lights = true;
            }
            if plan.sky_dirty {
                self.sharc_invalidation.sky = true;
            }
            if plan.mesh_releases > 0 || plan.material_releases > 0 {
                self.sharc_invalidation.geometry = true;
            }
        }
        #[cfg(not(feature = "sharc"))]
        let _ = dispatch_plan;
        // #883 Phase 2: frame_data is copied so we can inject the previous
        // unjittered view-projection without mutating the caller's value.
        let mut frame_data = **frame_data;
        let prev_vp = if let Some(camera) = cameras.first() {
            // Explicit camera slots are not TAA-jittered (TAA currently
            // resolves the implicit single-camera path).
            camera.view_proj
        } else {
            let mut vp = frame_data.viewProj;
            vp[3][0] -= self.temporal.taa_jitter_ndc[0];
            vp[3][1] -= self.temporal.taa_jitter_ndc[1];
            vp
        };
        frame_data.prevViewProj = self.temporal.prev_camera_view_proj.unwrap_or(prev_vp);
        let light_view_proj = *light_view_proj;
        let light_view_proj1 = *light_view_proj1;
        let shadow_split = *shadow_split;
        let shadow_cascade_count = *shadow_cascade_count;
        let inv_projection = *inv_projection;
        let debug_flags = *debug_flags;
        let tonemap_mode = *tonemap_mode;
        let aa_mode = *aa_mode;
        let debug_rt = *debug_rt;
        let proj22 = *proj22;
        let proj32 = *proj32;

        // Record into a `Result` rather than `?`-propagating: if any step
        // fails we still must `end_command_buffer` below so the in-flight
        // fence gets signaled in `present`. Otherwise the next frame's
        // `wait_for_fences` would hang forever.
        let mut record: anyhow::Result<()> = Ok(());

        // --- 更新 帧 UBO (#461: one aligned slot per composited camera) ---
        if record.is_ok() {
            let ubo = &self.runtime.frame_ubos[frame];
            if cameras.is_empty() {
                record = ubo
                    .update(device, &frame_data, 0)
                    .context("update frame UBO");
            } else {
                for (slot, camera) in cameras
                    .iter()
                    .take(crate::descriptor::MAX_CAMERAS)
                    .enumerate()
                {
                    let mut data = frame_data;
                    let light_count = data.cameraPosition[3];
                    data.viewProj = camera.view_proj;
                    data.view = camera.view;
                    data.cameraPosition =
                        [camera.eye[0], camera.eye[1], camera.eye[2], light_count];
                    data.exposure = camera.exposure;
                    if let Err(error) = ubo.update(device, &data, slot) {
                        record = Err(error).context("update frame UBO");
                        break;
                    }
                }
            }
        }

        // --- Execute render graph (Shadow -> Scene -> GTAO -> Post) ---
        if record.is_ok() {
            // #1007：本帧结束时把主通道深度拷到回读缓冲。多相机层用同一份
            // 深度图合成，粗深度只能对应其中一台，先关闭保证不误剔。
            // 放在借用 `swapchain` / `forward` 之前，避免和它们的借用冲突。
            let capture = if self.occlusion.enabled && cameras.is_empty() {
                self.ensure_occlusion_target(image_index, render_extent)
            } else {
                None
            };
            if let Some(forward) = self.graph.pass_mut::<ForwardPass>() {
                forward.set_depth_readback(capture);
            }
            let ao_view = self.ao_view(frame as u32);
            // 上一帧 ReSTIR GI 输出（1 帧延迟）。pass 未注册 / 未分派时为 null，
            // 前向 pass 的 `gi_mode` push constant 随之为 0，退回 probe volume。
            let gi_view = self.gi_view();
            let prt_gi_descriptor_set = self.prt_gi_descriptor_set(ctx.frame_index);
            let offscreen_views = self.offscreen.as_ref().map(|o| [o.image_view()]);
            let swapchain_views: &[vk::ImageView] = match (&self.swapchain, &offscreen_views) {
                (Some(sw), _) => sw.views.as_slice(),
                (None, Some(ov)) => ov.as_slice(),
                (None, None) => &[],
            };
            let offscreen_images = self.offscreen.as_ref().map(|o| [o.image()]);
            let swapchain_images: &[vk::Image] = match (&self.swapchain, &offscreen_images) {
                (Some(sw), _) => sw.images.as_slice(),
                (None, Some(oi)) => oi.as_slice(),
                (None, None) => &[],
            };
            // Skinned draws (#560 P0-3): push this frame's palette into the
            // per-frame bone SSBO before the graph runs.
            if let Some(forward) = self.graph.pass_mut::<ForwardPass>() {
                // #883 Phase 2: match this frame's draws to last frame's
                // models by entity id. First-seen entities fall back to their
                // current model (camera-only motion).
                let prev_models: Vec<[[f32; 4]; 4]> = draw_items
                    .iter()
                    .map(|item| {
                        self.temporal
                            .prev_models
                            .get(&item.entity_id)
                            .copied()
                            .unwrap_or(item.model)
                    })
                    .collect();
                forward.update_prev_models(device, ctx.frame_index, &prev_models)?;
                forward.update_bones(device, ctx.frame_index, bone_matrices)?;
            }
            let graph_frame = GraphFrame {
                frame_ubo: &self.runtime.frame_ubos[frame],
                cameras,
                draw_list: draw_items,
                gizmos,
                sprite_items,
                sprite_lights,
                particle_items,
                bone_matrices,
                skybox: *skybox,
                mesh_manager: &self.mesh_manager,
                light_view_proj,
                light_view_proj1,
                shadow_split,
                shadow_cascade_count,
                // 场景里没有投阴影的方向光时整帧跳过阴影 pass。
                shadow_mode: if *shadows_enabled {
                    self.settings.shadow_mode
                } else {
                    ShadowMode::None
                },
                debug_flags,
                wireframe: *wireframe,
                shaded_wireframe: *shaded_wireframe,
                outline_entities: &self.outline_entities,
                grade: grade.sanitized(),
                inv_view_rot: inv_view_rot_from_view(&frame_data.view),
                view_proj: frame_data.viewProj,
                view_proj_jitter: if self.graph.debug_control().freeze_temporal {
                    [0.0, 0.0]
                } else {
                    self.temporal.taa_jitter_ndc
                },
                lights,
                ao_view,
                gi_view,
                prt_gi_descriptor_set,
                tonemap_mode,
                aa_mode,
                debug_rt,
                proj22,
                proj32,
                inv_projection,
                swapchain_views,
                swapchain_images,
                freeze_temporal: self.graph.debug_control().freeze_temporal,
                render_mode: *render_mode,
                pt_max_bounces: *pt_max_bounces,
                pt_ray_max_distance: *pt_ray_max_distance,
                pt_max_iterations: *pt_max_iterations,
                camera_pos: frame_data.cameraPosition,
                light_dir: frame_data.lightDirection,
                light_color: frame_data.lightColor,
                exposure: *exposure,
                pt_lights,
                pt_accum_dirty: *pt_accum_dirty,
                has_camera: *has_camera,
                camera_orthographic: *camera_orthographic,
                clear_color: *clear_color,
                show_grid: *show_grid,
                dispatch_plan: input.dispatch_plan,
            };
            self.temporal.prev_camera_view_proj = Some(prev_vp);
            self.temporal.prev_models = draw_items
                .iter()
                .filter(|item| item.entity_id != u32::MAX)
                .map(|item| (item.entity_id, item.model))
                .collect();

            // SHaRC has two independent consumers now: the path tracer (trains
            // the cache, then queries it) and the ReSTIR GI pass (deposits and
            // queries while tracing its secondary rays, no path tracer involved).
            // Both need the same clear / buffer-handover bookkeeping, so the gate
            // below covers either mode instead of hard-coding the path-traced one.
            //
            // The training and resolve dispatches themselves are graph nodes
            // (`SharcTrainPass` / `SharcResolveNode`); what stays here is the
            // cache ownership: allocation, invalidation and the clear.
            #[cfg(feature = "sharc")]
            if crate::sharc::sharc_active(
                self.settings.pt_gi,
                self.settings.raster_gi,
                *render_mode,
            ) {
                // The hash map is sized when it is allocated, so a capacity
                // change has to rebuild the buffers. Voxel keys are meaningless
                // across a resize, so the rebuilt cache starts cleared.
                let capacity = self.settings.sharc_capacity.max(1);
                let capacity_changed = self
                    .sharc_targets
                    .as_ref()
                    .is_some_and(|targets| targets.capacity != capacity);
                let mut needs_init_clear = false;
                if capacity_changed {
                    // Command buffers from the frames still in flight may
                    // reference the old buffers. Capacity changes are rare, so
                    // stalling the device once is the cheap way to be safe.
                    unsafe { device.device_wait_idle() }.ok();
                    if let Some(resolve) = self.graph.pass_mut::<crate::sharc::SharcResolveNode>() {
                        resolve.clear(device);
                    }
                    if let Some(mut targets) = self.sharc_targets.take() {
                        targets.destroy(&self.runtime.context);
                    }
                    log::info!("SHaRC capacity changed to {capacity} entries");
                }
                if self.sharc_targets.is_none() {
                    match crate::sharc::SharcTargets::new(&self.runtime.context, capacity) {
                        Ok(targets) => {
                            // The resolve node owns the descriptor set that binds
                            // the cache buffers, so rebind it whenever the
                            // buffers are (re)allocated.
                            let rebound = self
                                .graph
                                .pass_mut::<crate::sharc::SharcResolveNode>()
                                .map(|resolve| resolve.set_targets(&self.runtime.context, &targets))
                                .unwrap_or(Ok(()));
                            match rebound {
                                Ok(()) => {
                                    // Hand the buffers to the trainer/consumer.
                                    #[cfg(feature = "pathtrace")]
                                    if let Some(pt) = self.graph.pass_mut::<PathTracePass>() {
                                        pt.set_sharc_buffers(
                                            Some(targets.hash_entries_buffer),
                                            Some(targets.accumulation_buffer),
                                            Some(targets.resolved_buffer),
                                            targets.capacity,
                                        );
                                    }
                                    #[cfg(feature = "restir_gi")]
                                    if let Some(gi) =
                                        self.graph.pass_mut::<crate::restir_gi::RestirGiPass>()
                                    {
                                        gi.set_sharc_buffers(
                                            Some(targets.hash_entries_buffer),
                                            Some(targets.accumulation_buffer),
                                            Some(targets.resolved_buffer),
                                            targets.capacity,
                                        );
                                    }
                                    self.sharc_targets = Some(targets);
                                    needs_init_clear = true;
                                    log::info!("SHaRC initialized ({capacity} entries)");
                                }
                                Err(e) => log::warn!("Failed to create SharcResolvePass: {e:#}"),
                            }
                        }
                        Err(e) => log::warn!("Failed to create SharcTargets: {e:#}"),
                    }
                }

                // Consume the recorded lifecycle reasons. A reason that could
                // not be serviced because SHaRC is idle this frame stays
                // pending until the cache is used again, which is exactly when
                // the stale voxels would become visible.
                let invalidation = std::mem::take(&mut self.sharc_invalidation);
                if invalidation.any() {
                    log::debug!(
                        "SHaRC cache invalidated: geometry={} lights={} sky={}",
                        invalidation.geometry,
                        invalidation.lights,
                        invalidation.sky
                    );
                }
                // A voxel-scale change redefines the hash key for every world
                // position, so the surviving entries describe the wrong voxels
                // (repair spec §8: cache format changes clear).
                let scale = self.settings.sharc_scene_scale;
                let scale_changed = self
                    .sharc_cache_scene_scale
                    .is_some_and(|previous| previous != scale);
                self.sharc_cache_scene_scale = Some(scale);
                if scale_changed {
                    log::info!("SHaRC voxel scale changed to {scale}; clearing the cache");
                }
                if needs_init_clear
                    || invalidation.any()
                    || scale_changed
                    || self.settings.sharc_enable_clear
                {
                    if let Some(targets) = &self.sharc_targets {
                        targets.clear(device, cmd);
                        self.sharc_clear_count += 1;
                        // Pipeline barrier: TRANSFER_WRITE -> SHADER_READ | SHADER_WRITE
                        let barrier = vk::MemoryBarrier::default()
                            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                            .dst_access_mask(
                                vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE,
                            );
                        unsafe {
                            device.cmd_pipeline_barrier(
                                cmd,
                                vk::PipelineStageFlags::TRANSFER,
                                vk::PipelineStageFlags::COMPUTE_SHADER,
                                vk::DependencyFlags::empty(),
                                &[barrier],
                                &[],
                                &[],
                            );
                        }
                    }
                    self.settings.sharc_enable_clear = false;
                }
            }
            // 设了 `gi_mode = Sharc` 却没编 `sharc`：退化为纯路径追踪，但必须说清楚
            // 缺哪个 feature（DESIGN §12.3 规则 3：关掉要可诊断，不要静默降级）。
            // 这个分支每帧都会走到，所以只报一次。
            #[cfg(all(feature = "pathtrace", not(feature = "sharc")))]
            if crate::render_graph::PtGiMode::Sharc == self.settings.pt_gi {
                static SHARC_MISSING_WARNED: std::sync::Once = std::sync::Once::new();
                SHARC_MISSING_WARNED.call_once(|| {
                    log::warn!(
                        "gi_mode = Sharc 需要 prism-render/sharc，本次构建没有编译 SHaRC；\
                         已退化为纯路径追踪"
                    );
                });
                if let Some(pt) = self.graph.pass_mut::<PathTracePass>() {
                    pt.set_sharc_buffers(None, None, None, 0);
                }
            }
            // Resolve is the only stage that normalises the accumulation
            // counters (see `crate::sharc::training_enabled`). Training with
            // resolve disabled would keep adding to counters that nothing
            // resets, so the trainer node skips and the reason is logged once.
            #[cfg(feature = "sharc")]
            if self.settings.sharc_enable_update && !self.settings.sharc_enable_resolve {
                static TRAIN_WITHOUT_RESOLVE_WARNED: std::sync::Once = std::sync::Once::new();
                TRAIN_WITHOUT_RESOLVE_WARNED.call_once(|| {
                    log::warn!(
                        "sharc_enable_update is set but sharc_enable_resolve is not; \
                         training is skipped because resolve owns the accumulation \
                         counters. Enable sharc_enable_resolve to train the cache."
                    );
                });
            }

            if *render_mode == crate::render_graph::RenderMode::Raster
                && self.settings.raster_gi == crate::render_graph::RasterGiMode::Ddgi
            {
                if let Some(ddgi) = &mut self.ddgi_pass {
                    if ddgi.has_geometry() {
                        let pv_image = self.scene_scope.probe_volume_image();
                        let to_general = vk::ImageMemoryBarrier::default()
                            .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                            .new_layout(vk::ImageLayout::GENERAL)
                            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                            .image(pv_image)
                            .subresource_range(vk::ImageSubresourceRange {
                                aspect_mask: vk::ImageAspectFlags::COLOR,
                                base_mip_level: 0,
                                level_count: 1,
                                base_array_layer: 0,
                                layer_count: 1,
                            })
                            .src_access_mask(vk::AccessFlags::SHADER_READ)
                            .dst_access_mask(vk::AccessFlags::SHADER_WRITE);
                        unsafe {
                            device.cmd_pipeline_barrier(
                                cmd,
                                vk::PipelineStageFlags::FRAGMENT_SHADER,
                                vk::PipelineStageFlags::COMPUTE_SHADER,
                                vk::DependencyFlags::empty(),
                                &[],
                                &[],
                                std::slice::from_ref(&to_general),
                            );
                        }

                        ddgi.dispatch_scene(
                            cmd,
                            self.scene_scope.probe_volume_view(),
                            self.material_manager.buffer(),
                            self.texture_manager.bindless().set,
                            self.ibl.descriptor_set,
                            [
                                frame_data.cameraPosition[0],
                                frame_data.cameraPosition[1],
                                frame_data.cameraPosition[2],
                            ],
                            [
                                frame_data.lightDirection[0],
                                frame_data.lightDirection[1],
                                frame_data.lightDirection[2],
                            ],
                            [
                                frame_data.lightColor[0],
                                frame_data.lightColor[1],
                                frame_data.lightColor[2],
                            ],
                            ctx.frame_index,
                        )?;

                        let to_read = vk::ImageMemoryBarrier::default()
                            .old_layout(vk::ImageLayout::GENERAL)
                            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                            .image(pv_image)
                            .subresource_range(vk::ImageSubresourceRange {
                                aspect_mask: vk::ImageAspectFlags::COLOR,
                                base_mip_level: 0,
                                level_count: 1,
                                base_array_layer: 0,
                                layer_count: 1,
                            })
                            .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                            .dst_access_mask(vk::AccessFlags::SHADER_READ);
                        unsafe {
                            device.cmd_pipeline_barrier(
                                cmd,
                                vk::PipelineStageFlags::COMPUTE_SHADER,
                                vk::PipelineStageFlags::FRAGMENT_SHADER,
                                vk::DependencyFlags::empty(),
                                &[],
                                &[],
                                std::slice::from_ref(&to_read),
                            );
                        }
                    }
                }
            }

            let render_ctx = crate::render_graph::RenderContext {
                device,
                context: &self.runtime.context,
                settings: &self.settings,
                cmd,
                frame_index: frame as u32,
                image_index,
                extent,
                render_extent,
                frame: &graph_frame,
            };

            record = self
                .graph
                .execute_with_timer(&render_ctx, self.telemetry.gpu_timer_mut())
                .context("graph execute");
            // 深度拷贝确实记进命令缓冲区后，才把矩阵留给下一帧配对。
            if record.is_ok() && capture.is_some() {
                let captured = self
                    .graph
                    .pass_mut::<ForwardPass>()
                    .and_then(|forward| forward.depth_capture_frame());
                if captured == Some(ctx.frame_index) {
                    if let Some(slot) = self
                        .occlusion
                        .pending_view_proj
                        .get_mut(image_index as usize)
                    {
                        *slot = Some(frame_data.viewProj);
                    }
                }
            }
        }

        // --- ECS UI 叠加 (quads + fontdue glyph text) ---
        //
        // Recorded between the 渲染 图 output and the external overlay
        // (or the explicit PRESENT 屏障). For the swapchain path its render
        // pass owns COLOR_ATTACHMENT_OPTIMAL → PRESENT_SRC_KHR; for the
        // offscreen path it stays in COLOR_ATTACHMENT_OPTIMAL so the host can
        // sample the image after `offscreen_present_for_sampling` (#597).
        //
        // 图 末尾 pass（RtPreview）final = PRESENT_SRC_KHR，所以 swapchain
        // image 在 graph 后已是展示布局；UI/overlay render pass 期望
        // COLOR_ATTACHMENT_OPTIMAL 初始布局 → 各自 record 前都要显式转回。
        #[cfg(feature = "ui")]
        if record.is_ok() {
            let has_ui = ui_overlay.is_some_and(|o| !o.quads.is_empty() || !o.texts.is_empty());
            if has_ui {
                // UI 背景图（#591）从场景共享的 bindless 表取样。
                let bindless_set = self.texture_manager.bindless().set;
                let bindless_layout = self.texture_manager.bindless().layout;
                if let Some(sw) = self.swapchain.as_ref() {
                    let dest = crate::render_graph::post_destination(
                        extent,
                        render_extent,
                        self.settings.internal_scale_mode,
                    );
                    record = self.ui_host.record_ecs_ui_swapchain(
                        &self.runtime.context,
                        self.runtime.command_pool,
                        cmd,
                        sw.format.format,
                        sw.images[image_index as usize],
                        sw.views[image_index as usize],
                        extent,
                        dest,
                        ui_overlay.unwrap(),
                        bindless_layout,
                        bindless_set,
                        ctx.frame_index,
                    );
                } else if let Some((image, view, target_extent, color_format)) =
                    self.offscreen.as_ref().map(|target| {
                        (
                            target.image(),
                            target.image_view(),
                            target.extent(),
                            target.format(),
                        )
                    })
                {
                    // 离屏目标没有交换链缩放，UI 铺满整张图。
                    let dest = vk::Rect2D {
                        offset: vk::Offset2D { x: 0, y: 0 },
                        extent: target_extent,
                    };
                    record = self.ui_host.record_ecs_ui_offscreen(
                        &self.runtime.context,
                        self.runtime.command_pool,
                        cmd,
                        color_format,
                        image,
                        view,
                        target_extent,
                        dest,
                        ui_overlay.unwrap(),
                        bindless_layout,
                        bindless_set,
                        ctx.frame_index,
                    );
                }
            }
        }

        // --- Transition swapchain image to PRESENT_SRC_KHR ---
        //
        // 外部叠加层（宿主 UI）在 ECS UI 之后 record：image 此时
        // 已是 PRESENT_SRC_KHR（graph 末尾 RtPreview 或 ECS UI 的 final
        // layout），叠加 render pass 期望 COLOR_ATTACHMENT_OPTIMAL 初始
        // 布局 → record 前显式转回。无叠加时无需任何转换。
        if record.is_ok() && self.ui_host.has_external_overlay_content() {
            if let Some(sw) = self.swapchain.as_ref() {
                let image = sw.images[image_index as usize];
                let barrier = vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                    .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(image)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    })
                    .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                    .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);
                unsafe {
                    device.cmd_pipeline_barrier(
                        cmd,
                        vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                        vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                        vk::DependencyFlags::empty(),
                        &[],
                        &[],
                        std::slice::from_ref(&barrier),
                    );
                }
                let oc = crate::external_overlay::OverlayRecordCtx {
                    device,
                    context: &self.runtime.context,
                    command_pool: self.runtime.command_pool,
                    graphics_queue: self.runtime.context.graphics_queue,
                    cmd,
                    swapchain_views: &sw.views,
                    image_index,
                    extent,
                    color_format: self.color_format,
                };
                record = self.ui_host.record_external_overlay(&oc);
            } else {
                record = Err(anyhow::anyhow!("external overlay: swapchain missing"));
            }
        }

        // --- 截图：交换链图像 → host-visible 缓冲区 ---
        //
        // 必须放在图像回到 PRESENT_SRC_KHR 之后、`end_command_buffer` 之前：
        // 拷贝本身需要 TRANSFER_SRC_OPTIMAL，记录完再转回展示布局。
        if record.is_ok() {
            let target = if let Some(ref sw) = self.swapchain {
                sw.images
                    .get(image_index as usize)
                    .copied()
                    .map(|img| (img, vk::ImageLayout::PRESENT_SRC_KHR))
            } else {
                self.offscreen
                    .as_ref()
                    .map(|offscreen| (offscreen.image(), vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL))
            };
            if let Err(err) = self.telemetry.record_screenshot_if_requested(
                &self.runtime.context,
                cmd,
                extent,
                target,
            ) {
                record = Err(err);
            }
        }

        // --- End command buffer (always attempted) ---
        if record.is_ok() {
            self.telemetry.end_frame(device, cmd, ctx.frame_index);
        }
        if let Err(end_err) = unsafe { device.end_command_buffer(cmd) } {
            if record.is_ok() {
                record = Err(anyhow::anyhow!("end command buffer: {end_err:?}"));
            }
        }

        record
    }

    /// Phase 3/3: submit the recorded command buffer and present to the
    /// swapchain.
    ///
    /// Runs **regardless** of whether [`execute`] returned an error — the
    /// in-flight fence (reset during [`begin_frame`]) must be signaled so the
    /// next frame does not hang. Returns `true` when the swapchain was
    /// recreated (out-of-date on present).
    /// Phase 3/3: submit and present.  In headless mode this is a no-op
    /// (the offscreen target owns its own command-submission path).
    pub fn present(&mut self, ctx: &FrameCtx) -> anyhow::Result<bool> {
        // Headless: just submit with a fence-out but skip present.
        if self.is_headless {
            let cmd_bufs = [ctx.cmd];
            let submit = vk::SubmitInfo::default().command_buffers(&cmd_bufs);
            unsafe {
                self.runtime.context.device.queue_submit(
                    self.runtime.context.graphics_queue,
                    &[submit],
                    ctx.fence,
                )
            }
            .context("headless queue submit")?;
            if ctx.fence != vk::Fence::null() {
                unsafe {
                    self.runtime
                        .context
                        .device
                        .wait_for_fences(&[ctx.fence], true, u64::MAX)
                }
                .context("headless wait for fence")?;
            }
            return Ok(false);
        }
        let wait_semaphores = [ctx.image_available];
        let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let signal_semaphores = [ctx.render_finished];
        let cmd_bufs = [ctx.cmd];
        let submit = vk::SubmitInfo::default()
            .wait_semaphores(&wait_semaphores)
            .wait_dst_stage_mask(&wait_stages)
            .command_buffers(&cmd_bufs)
            .signal_semaphores(&signal_semaphores);
        unsafe {
            ctx.device
                .queue_submit(self.runtime.context.graphics_queue, &[submit], ctx.fence)
        }
        .context("queue submit")?;

        let out_of_date = self
            .swapchain
            .as_mut()
            .context("present: no swapchain")?
            .present(
                self.runtime.context.graphics_queue,
                ctx.image_index,
                ctx.render_finished,
            )?;

        if out_of_date {
            log::debug!("present out of date, recreating");
            self.recreate_swapchain()?;
        }

        Ok(out_of_date)
    }

    /// 帧结束阶段：推进帧级状态并执行延迟回收。
    pub fn end_frame(&mut self, _ctx: &FrameCtx, _presented: bool) -> anyhow::Result<()> {
        // `PRISMA_PASS_DUMP`: the previous `present` waited on the frame
        // fence, so the readback buffers are safe to map and encode here.
        self.graph.flush_pass_dump(&self.runtime.context.device)?;
        Ok(())
    }

    /// Render a frame: one-shot convenience that calls [`begin_frame`],
    /// [`execute`], and [`present`] in order.
    ///
    /// This is a compatibility wrapper; new code should prefer the explicit
    /// phase API for finer error handling and future prepare-stage insertion.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        draw_items: &[DrawItem],
        frame_data: &FrameUBOData,
        light_view_proj: [[f32; 4]; 4],
        inv_projection: [[f32; 4]; 4],
        debug_flags: u32,
        tonemap_mode: u32,
        aa_mode: u32,
        debug_rt: u32,
        proj22: f32,
        proj32: f32,
        lights: &[GpuLight],
        render_mode: RenderMode,
        pt_max_bounces: u32,
        pt_ray_max_distance: f32,
        pt_max_iterations: u32,
        exposure: f32,
        pt_lights: &[PtAnalyticLight],
        grade: crate::render_graph::PostGrade,
        wireframe: bool,
        shaded_wireframe: bool,
        bone_matrices: &[[[f32; 4]; 4]],
    ) -> anyhow::Result<bool> {
        let ctx = match self.begin_frame()? {
            Some(c) => c,
            None => return Ok(false),
        };
        // The shadow atlas was allocated at construction time; clamp the frame
        // request so a runtime settings edit cannot make the shader sample a
        // cascade that was never allocated.
        let mut frame_data = *frame_data;
        frame_data.shadowCascadeCount = frame_data
            .shadowCascadeCount
            .clamp(1, self.settings.shadow_cascade_count.clamp(1, 2));
        // #883: subpixel jitter for TAA. The graph frame carries the applied
        // NDC offset so `TaaPass` can recover the unjittered camera matrix.
        self.apply_taa_jitter(&mut frame_data);
        let input = FrameInput {
            draw_items,
            gizmos: &[],
            sprite_items: &[],
            sprite_lights: &[],
            particle_items: &[],
            bone_matrices,
            skybox: None,
            frame_data: &frame_data,
            cameras: &[],
            light_view_proj,
            light_view_proj1: frame_data.lightViewProj1,
            shadow_split: frame_data.shadowSplit,
            shadow_cascade_count: frame_data.shadowCascadeCount,
            inv_projection,
            debug_flags,
            wireframe,
            shaded_wireframe,
            tonemap_mode,
            aa_mode,
            debug_rt,
            grade,
            lut_path: None,
            proj22,
            proj32,
            lights,
            render_mode,
            pt_max_bounces,
            pt_ray_max_distance,
            pt_max_iterations,
            exposure,
            pt_lights,
            pt_accum_dirty: false,
            has_camera: true,
            // The convenience wrapper has no camera entity to inspect and
            // always drove a perspective projection before, so keep that.
            camera_orthographic: false,
            clear_color: [0.5, 0.5, 0.5, 1.0],
            shadows_enabled: true,
            ui_overlay: None,
            show_grid: false,
            dispatch_plan: None,
        };
        self.prepare(&ctx, &input)?;
        let exec_result = self.execute(&ctx, &input);
        let out_of_date = self.present(&ctx)?;
        exec_result?; // propagate recording error after fence is safe
        self.end_frame(&ctx, out_of_date)?;
        Ok(out_of_date)
    }

    /// Release all GPU resources.
    /// 预约一次截图：下一帧 `execute` 把交换链图像拷贝到读回缓冲区，
    /// `finish_screenshot` 再写成 PNG。
    ///
    /// `size` 为 `None` 时与窗口分辨率一致；给了尺寸就最近邻缩放
    /// （像素画放大不会糊）。
    pub fn request_screenshot(
        &mut self,
        path: impl Into<std::path::PathBuf>,
        size: Option<[u32; 2]>,
        scale: Option<f32>,
    ) {
        self.telemetry.request_screenshot(path.into(), size, scale);
    }

    /// Programmatically configure pass dump for automated tests or headless runs.
    pub fn configure_pass_dump(
        &mut self,
        dir: impl Into<std::path::PathBuf>,
        target_frame: u64,
        filter: Option<Vec<String>>,
    ) {
        self.graph
            .configure_pass_dump(dir.into(), target_frame, filter);
    }

    /// Access the diagnostic debug control settings (immutable).
    pub fn debug_control(&self) -> &crate::render_debug::RenderDebugControl {
        self.graph.debug_control()
    }

    /// Access the diagnostic debug control settings (mutable).
    pub fn debug_control_mut(&mut self) -> &mut crate::render_debug::RenderDebugControl {
        self.graph.debug_control_mut()
    }

    /// Replace the diagnostic debug control settings.
    pub fn set_debug_control(&mut self, control: crate::render_debug::RenderDebugControl) {
        self.graph.set_debug_control(control);
    }

    /// 帧提交之后调用：等待本次提交的围栏、读回像素、写 PNG。
    ///
    /// 没有进行中的截图时返回 `Ok(None)`。
    pub fn finish_screenshot(
        &mut self,
        ctx: &FrameCtx,
    ) -> anyhow::Result<Option<std::path::PathBuf>> {
        self.telemetry
            .finish_screenshot(&self.runtime.context, ctx.fence, self.color_format)
    }

    /// Active NVIDIA NRD library version `(major, minor, build)` if loaded.
    #[must_use]
    pub fn nrd_version(&self) -> Option<(u8, u8, u8)> {
        self.nrd_runtime
            .as_ref()
            .map(crate::nrd::NrdRuntime::version)
    }

    pub fn destroy(&mut self) {
        self.destroy_occlusion_targets();
        let device = &self.runtime.context.device;
        unsafe { device.device_wait_idle() }.ok();

        self.telemetry.destroy(device);
        self.graph.discard_pass_dump(device);

        // Destroy IBL resources (env/irradiance/prefiltered cubes, BRDF LUT).
        // 共享的 set2 描述符 布局 由 GraphRenderer 持有，此处单独销毁
        // （在 ibl.destroy() 把字段置空前取出句柄）。
        let ibl_ds_layout = self.ibl.descriptor_set_layout;
        self.ibl.destroy();
        if ibl_ds_layout != vk::DescriptorSetLayout::null() {
            unsafe { device.destroy_descriptor_set_layout(ibl_ds_layout, None) };
        }

        // Destroy scene managers.
        self.material_manager.destroy(device);
        self.texture_manager.destroy();
        self.mesh_manager.destroy(device);
        self.suballocator.destroy(device);

        // 销毁 ForwardPass (framebuffers, 深度 images, 渲染 pass
        // pipeline, shadow descriptor set). Without this, vkDestroyDevice
        // reports leaked VkImage/VkDeviceMemory/VkImageView/VkRenderPass.
        if let Some(forward_pass) = self.graph.pass_mut::<ForwardPass>() {
            forward_pass.destroy(device);
        }

        // Destroy scene-level GI probe volume (SceneScope). Must happen AFTER
        // ForwardPass::destroy (ForwardPass borrows the 描述符 集合
        self.ddgi_pass = None;
        self.scene_scope.destroy();

        // Destroy GTAO pass (AO images, render pass, pipeline, descriptor
        // sets, sampler).
        #[cfg(feature = "gtao")]
        if let Some(gtao) = self.graph.pass_mut::<crate::gtao::GtaoPass>() {
            gtao.destroy(device);
        }
        self.placeholders.destroy(device);

        // Destroy PostPass (framebuffers, render pass, pipeline, descriptor
        // set, sampler).
        if let Some(post) = self.graph.pass_mut::<crate::post::PostPass>() {
            post.destroy(device);
        }

        // Destroy AutoExposurePass (histogram/luminance buffers, descriptor
        // pool, sampler). Must run before `self.graph.destroy`, which drops the
        // pass objects themselves.
        if let Some(auto_exposure) = self
            .graph
            .pass_mut::<crate::auto_exposure::AutoExposurePass>()
        {
            auto_exposure.destroy(device);
        }

        // Destroy PathTracePass (accumulation images, geometry buffers,
        // BLAS/TLAS, descriptor set, layout, pool).
        #[cfg(feature = "pathtrace")]
        if let Some(pt) = self.graph.pass_mut::<PathTracePass>() {
            pt.destroy(device);
        }

        #[cfg(feature = "sharc")]
        {
            if let Some(resolve) = self.graph.pass_mut::<crate::sharc::SharcResolveNode>() {
                resolve.clear(device);
            }
            if let Some(mut targets) = self.sharc_targets.take() {
                targets.destroy(&self.runtime.context);
            }
        }

        // Destroy ShadowMapPass (framebuffer, render pass, pipeline/layout).
        // This MUST happen BEFORE scene_scope/graph destruction (which owns
        // the Arc<VulkanContext>) because Rust field-drop order drops the
        // context-holders (runtime/ibl/scene_scope) *before* the graph — if
        // ShadowMapPass relied on its Drop alone, the device handle would be
        // stale by the time it ran, causing leaked resources + access violation.
        #[cfg(feature = "shadows")]
        if let Some(shadow) = self
            .graph
            .pass_mut::<crate::shadow_map_pass::ShadowMapPass>()
        {
            shadow.destroy(device);
        }

        // 销毁外部叠加层（render pass/framebuffers/渲染器）。
        self.ui_host.destroy();

        // Destroy shadow sampler.
        unsafe { device.destroy_sampler(self.shadow_sampler, None) };

        // Destroy graph resources (shadow map images, etc.).
        self.graph.destroy(device);

        // Destroy command pool.
        unsafe { device.destroy_command_pool(self.runtime.command_pool, None) };

        // Destroy swapchain.
        if let Some(mut sw) = self.swapchain.take() {
            unsafe { sw.destroy(device) };
        }

        // Destroy offscreen target (headless mode).
        if let Some(mut target) = self.offscreen.take() {
            unsafe { target.destroy(device) };
        }

        if self.headless_fence != vk::Fence::null() {
            unsafe { device.destroy_fence(self.headless_fence, None) };
            self.headless_fence = vk::Fence::null();
        }
    }
}

// 安全性 UI 状态已移出渲染线程（主线程持有），
// 渲染线程上剩余字段只持有 ash/Vulkan handles
// (vk::*, ash::Device → u64 wrappers Send+Sync), Arc<VulkanContext> (Send+Sync
// because all inner fields are vulkan handles), or container types whose
// elements are Send.  These fields are accessed exclusively from the render
// thread, so there is no concurrent &mut mutation.
unsafe impl Send for GraphRenderer {}

/// Transpose of `view`'s upper-left 3x3 (for `GraphFrame::inv_view_rot`).
#[must_use]
pub fn inv_view_rot_from_view(view: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut m = [[0.0f32; 4]; 4];
    for c in 0..3 {
        for r in 0..3 {
            m[c][r] = view[r][c];
        }
    }
    m[3][3] = 1.0;
    m
}

impl Drop for GraphRenderer {
    fn drop(&mut self) {
        self.destroy();
    }
}
