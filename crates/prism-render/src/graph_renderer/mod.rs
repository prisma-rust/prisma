//! RenderGraph-based render driver.
//!
//! [`GraphRenderer`] replaces old version renderer. It holds Vulkan below files,
//! swap chain, command pool + per-frame command buffer, frame UBO, IBL resources and three scene managers
//! (mesh, texture, material). It builds [`ShadowMapPass`] and [`ForwardPass`]
//! [`RenderGraph`], executes every frame and presents to swap chain.

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
/// `ShadowMapPass::setup` is trait method, only needs import when registering shadow pass.
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
    /// Before switching to wireframe mode (#431); ignored when device does not support `fillModeNonSolid`.
    pub wireframe: bool,
    /// Positive fill + wireframe overlay (#431). When `wireframe` is true, overridden by pure wireframe.
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
    /// Main directional light whether casts shadow (`Light.shadows != None`). `false` skips entire frame
    /// skips shadow texture render (`ShadowMode::None`), shader side will also get `shadowStrength = 0`.
    pub shadows_enabled: bool,
    /// Clear color for the scene color attachment. Applied when the render pass
    /// begins; shows through where no skybox or geometry is drawn. Default gray
    /// `[0.5, 0.5, 0.5, 1.0]` lets the user distinguish "nothing drew" from
    /// black.
    pub clear_color: [f32; 4],
    /// UI overlay draw commands (filled by engine's ui_render_system).
    /// `None` in headless mode.
    pub ui_overlay: Option<&'a crate::ui_overlay::UiOverlayInput>,
    /// Whether to draw ground reference mesh in scene render.
    pub show_grid: bool,
    /// Unified dirty dispatch schedule (DESIGN §9 / #1157).
    pub dispatch_plan: Option<&'a crate::dirty_dispatch::DirtyDispatchPlan>,
}

/// GPU session that owns the long-lived Vulkan runtime objects (device,
pub use crate::render_runtime::RenderRuntime;

/// Occlusion cull coarse depth screen block edge length (pixels). 4×4 in 1080p below is 480×270 depth values.
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
    /// Fixed bindless slot bit, holds current IBL BRDF LUT view. `set_environment`
    /// When heavy rebuilding IBL, uses `register_with_handle` to overwrite same slot, avoiding bindless table growth.
    brdf_bindless_slot: u32,
    graph: RenderGraph,
    /// All render  passes (ShadowMapPass + ForwardPass + GtaoPass + PostPass)
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
    /// Current need to draw outline ECS entity id (#487). Editor updates every frame via
    /// [`Self::set_outline_entities`] update; empty = not draw.
    outline_entities: Vec<u32>,
    shadow_sampler: vk::Sampler,
    /// `ForwardPass` optional pass disabled/not ready when 1×1 placeholder texture set.
    placeholders: crate::placeholder::ForwardPlaceholders,
    color_format: vk::Format,
    /// ECS UI overlay (window / offscreen) and outside host UI overlay layer host (#583, #597).
    ui_host: UiOverlayHost,
    /// Optional NVIDIA NRD runtime instance (`NRD.dll` / `libNRD.so`).
    #[cfg(feature = "nrd")]
    nrd_runtime: Option<crate::nrd::NrdRuntime>,
    /// NVIDIA Streamline runtime (DLSS-SR, DLSS-RR, DLFG), `None` when absent.
    #[cfg(feature = "dlss")]
    streamline: Option<std::sync::Arc<crate::streamline::StreamlineRuntime>>,
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
    /// GPU timestamp profiling and screenshot state machine (#518).
    telemetry: RenderTelemetry,
    /// Per-frame coarse depth readback (#1007). Disabled: no buffer allocated, no readback.
    occlusion: OcclusionState,
    /// Staged asset upload request queue (#1157), uniformly flushed in `prepare` phase.
    staged_asset_requests: Vec<crate::asset_bridge::AssetResolveRequest>,
    /// Staged asset release request queue (#1157), uniformly flushed in `prepare` phase.
    staged_asset_releases: Vec<crate::asset_bridge::AssetRelease>,
    /// VRAM sub-allocator (#1030).
    suballocator: crate::suballocator::MemorySuballocator,

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
    /// Scene actual render extent (internal render resolution, #613); not equal to `extent` when not opened at start.
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

        let runtime = RenderRuntime::new(context.clone(), swapchain.views.len() as u32)?;

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
                "`shadows` feature disabled: real-time shadows unavailable (set 3 binds 1x1 far depth map);\
                 need shadows please enable prism-render/shadows in project"
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
            .context("register BRDF LUT into bindless table")?;
        log::info!(
            "IBL: BRDF LUT registered as bindless handle {}",
            brdf_handle.0
        );

        let mut forward_pass = ForwardPass::new(color_format);
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
                "`restir_gi` compiled but device does not support VK_KHR_ray_query:\
                 gi_mode = Restir falls back to baked probe volume"
            );
            None
        };

        #[cfg(feature = "gtao")]
        let swapchain_extent = swapchain.extent;
        #[cfg(feature = "gtao")]
        let gtao_pass =
            crate::gtao::GtaoPass::new(&context, runtime.command_pool, swapchain_extent)
                .context("GtaoPass::new")?;
        #[cfg(not(feature = "gtao"))]
        log::warn!(
            "`gtao` feature disabled: screen-space ambient occlusion unavailable (set 4 binds 1x1 white image);\
             need AO please enable prism-render/gtao in project"
        );

        let taa_pass =
            crate::taa::TaaPass::new(&context, swapchain.extent).context("TaaPass::new")?;
        #[cfg(feature = "anf")]
        let anf_pass = crate::anf::AnfPass::new(&context, swapchain.extent, swapchain.extent)
            .context("AnfPass::new")?;

        let frame_count = swapchain.views.len() as u32;
        let mut post_pass = crate::post::PostPass::new(
            &context,
            color_format,
            frame_count,
            vk::ImageLayout::PRESENT_SRC_KHR,
        )
        .context("PostPass::new")?;

        let auto_exposure = crate::auto_exposure::AutoExposurePass::new(&context, frame_count)
            .context("AutoExposurePass::new")?;
        post_pass.set_adapted_luminance(&context.device, auto_exposure.luminance_buffer());

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

        let rt_scheduler = crate::rt_scheduler::RenderTextureScheduler::new(&context);

        graph.add_pass(Box::new(forward_pass));
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
        #[cfg(feature = "restir_gi")]
        if let Some(pass) = restir_gi_pass {
            graph.add_pass(Box::new(pass));
        }
        graph.add_pass(Box::new(rt_scheduler));
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
        #[cfg(feature = "dlss")]
        let streamline =
            crate::streamline::StreamlineRuntime::try_init(&context).map(std::sync::Arc::new);
        #[cfg(feature = "dlss")]
        {
            let dlss_pass =
                crate::dlss_pass::DlssPass::new(&context, swapchain.extent, streamline.clone())
                    .context("DlssPass::new")?;
            graph.add_pass(Box::new(dlss_pass));
        }
        graph.add_pass(Box::new(taa_pass));
        let mobile_upscaler_pass =
            crate::mobile_upscaler::MobileUpscalerPass::new(&context, swapchain.extent)
                .context("MobileUpscalerPass::new")?;
        graph.add_pass(Box::new(mobile_upscaler_pass));
        graph.add_pass(Box::new(auto_exposure));
        graph.add_pass(Box::new(post_pass));

        let telemetry = RenderTelemetry::new(&context, runtime.command_buffers.len())?;

        Ok(Self {
            swapchain: Some(swapchain),
            vsync,
            runtime,
            mesh_manager,
            texture_manager,
            material_manager,
            ibl,
            env_fingerprint: None,
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
            placeholders,
            color_format,
            ui_host: UiOverlayHost::default(),
            #[cfg(feature = "nrd")]
            nrd_runtime: crate::nrd::NrdRuntime::try_init(),
            #[cfg(feature = "dlss")]
            streamline,
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

    /// Runtime Streamline capabilities (`DLSS-SR`, `DLSS-RR`, `DLFG`).
    #[cfg(feature = "dlss")]
    pub fn streamline_caps(&self) -> crate::streamline::StreamlineCaps {
        self.streamline
            .as_ref()
            .map(|sl| sl.caps())
            .unwrap_or_default()
    }

    /// Access the live Streamline runtime (if initialized).
    #[cfg(feature = "dlss")]
    pub fn streamline_runtime(&self) -> Option<&crate::streamline::StreamlineRuntime> {
        self.streamline.as_deref()
    }

    /// Scene pass samples "per-frame ambient occlusion" view.
    ///
    /// `gtao` enabled at startup takes GTAO pass double buffering `(frame+1)%2`; disabled returns 1x1 white
    /// dummy (visible 1 = no occlusion), descriptor layout and bind count exactly consistent.
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

    /// Scene pass samples "per-frame ReSTIR GI indirect incorrect radiance" view.
    ///
    /// Same structure as AO: `RestirGiPass` double buffering, this frame writes A, next frame samples A, so take
    /// `gi_view()` returns "most recently written" view. Pass not registered or this frame not dispatched
    /// returns null, before toward pass will fall back to baked probe volume.
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

    /// Offscreen render target size (no window host mode); returns `None` when swap chain exists.
    pub fn offscreen_extent(&self) -> Option<ash::vk::Extent2D> {
        self.offscreen.as_ref().map(|target| target.extent())
    }

    /// Offscreen color graph image view (`R8G8B8A8_UNORM`). After first call at frame end
    /// [`Self::offscreen_present_for_sampling`], then register it as UI texture.
    pub fn offscreen_image_view(&self) -> Option<ash::vk::ImageView> {
        self.offscreen.as_ref().map(|target| target.image_view())
    }

    /// Offscreen color graph image format.
    pub fn offscreen_image_format(&self) -> Option<ash::vk::Format> {
        self.offscreen.as_ref().map(|target| target.format())
    }

    /// Convert offscreen graph image after frame end to `SHADER_READ_ONLY_OPTIMAL` and wait for GPU
    /// complete, for UI framework direct sampling (zero-copy viewport).
    ///
    /// After frame end offscreen graph image stays in `COLOR_ATTACHMENT_OPTIMAL` (headless below file doesn't have
    /// `VK_KHR_swapchain`, cannot use `PRESENT_SRC_KHR`); next frame's PostPass uses
    /// `initial_layout = UNDEFINED` (load op clear), therefore coming from any layout is legal.
    pub fn offscreen_present_for_sampling(&mut self) -> anyhow::Result<()> {
        let context = self.runtime.context.clone();
        let target = self
            .offscreen
            .as_mut()
            .context("offscreen_present_for_sampling: no OffscreenTarget")?;
        target.transition_to_shader_read(&context, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
    }

    /// Create a headless `GraphRenderer` — no window surface or swapchain.
    /// Useful for CI tests, dedicated servers, and offline asset baking.
    ///
    /// `env_bytes` is optional IBL environment map data (`None` = default sky).
    pub fn headless_new(env_bytes: Option<Vec<u8>>) -> anyhow::Result<Self> {
        Self::offscreen_new(256, 256, env_bytes)
    }

    /// Create a no-window `GraphRenderer` rendering into offscreen graph image of
    /// `width × height` (editor host mode / offscreen preview).
    ///
    /// Same render path as [`Self::headless_new`]: complete graph (Shadow → Scene →
    /// GTAO → Post) after complete, PostPass writes **post-tonemap result** into offscreen graph image,
    /// Then [`Self::copy_offscreen_to_readback`] + [`Self::readback_pixels`]
    /// reads that frame back to CPU.
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

        let runtime = RenderRuntime::new(context.clone(), 2)?;

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
                "`shadows` feature disabled: real-time shadows unavailable (set 3 binds 1x1 far depth map);\
                 need shadows please enable prism-render/shadows in project"
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
        let mut post_pass = crate::post::PostPass::new(
            &context,
            color_format,
            frame_count,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        )
        .context("PostPass::new (headless)")?;

        let auto_exposure = crate::auto_exposure::AutoExposurePass::new(&context, frame_count)
            .context("AutoExposurePass::new (headless)")?;
        post_pass.set_adapted_luminance(&context.device, auto_exposure.luminance_buffer());

        #[cfg(feature = "gtao")]
        let gtao_pass =
            crate::gtao::GtaoPass::new(&context, runtime.command_pool, offscreen.extent())
                .context("GtaoPass::new (headless)")?;
        #[cfg(not(feature = "gtao"))]
        log::warn!(
            "`gtao` feature disabled: screen-space ambient occlusion unavailable (set 4 binds 1x1 white image);\
             need AO please enable prism-render/gtao in project"
        );

        let taa_pass = crate::taa::TaaPass::new(&context, offscreen.extent())
            .context("TaaPass::new (headless)")?;
        #[cfg(feature = "anf")]
        let anf_pass = crate::anf::AnfPass::new(&context, offscreen.extent(), offscreen.extent())
            .context("AnfPass::new (headless)")?;

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
        #[cfg(feature = "dlss")]
        let streamline =
            crate::streamline::StreamlineRuntime::try_init(&context).map(std::sync::Arc::new);
        #[cfg(feature = "dlss")]
        {
            let dlss_pass =
                crate::dlss_pass::DlssPass::new(&context, offscreen.extent(), streamline.clone())
                    .context("DlssPass::new (headless)")?;
            graph.add_pass(Box::new(dlss_pass));
        }
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
            env_fingerprint: initial_env_fp.map(Some),
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
            placeholders,
            color_format,
            ui_host: UiOverlayHost::default(),
            #[cfg(feature = "nrd")]
            nrd_runtime: crate::nrd::NrdRuntime::try_init(),
            #[cfg(feature = "dlss")]
            streamline,
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
    /// Only takes effect after [`clear_offscreen`](Self::clear_offscreen) or
    /// [`copy_offscreen_to_readback`](Self::copy_offscreen_to_readback).
    pub fn readback_pixels(&self) -> anyhow::Result<Vec<u8>> {
        let target = self
            .offscreen
            .as_ref()
            .context("readback_pixels called but no OffscreenTarget")?;
        target.readback(&self.runtime.context)
    }

    /// Poll async offscreen readback; returns `None` when GPU not complete.
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

    /// Async submit offscreen graph image readback; skips this submission when readback not complete yet.
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

    /// Setting ECS entity id to draw outline this frame (#487).
    ///
    /// Editor maps `workspace.selection` to `prism_ecs::Entity::id()` after calling;
    /// Empty slice disables outline. Waiting draw during list will be copied; caller does not need to keep borrow.
    pub fn set_outline_entities(&mut self, entities: &[u32]) {
        self.outline_entities.clear();
        self.outline_entities.extend_from_slice(entities);
    }

    /// Current selected outline entity list (test / debug use).
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

    /// Inject outside overlay layer (host UI etc). Main thread calls before starting render thread;
    /// Overlay GPU resources lazily created by implementation at `record` time.
    pub fn set_external_overlay(&mut self, overlay: Box<dyn SwapchainOverlay>) {
        self.ui_host.set_external_overlay(overlay);
    }

    /// Deliver one type-erased message (like "new host UI frame") to outside overlay layer.
    /// Render thread takes message from shared channel after calling; silently drops when no overlay layer.
    pub fn apply_overlay_message(&mut self, msg: OverlayMessage) {
        self.ui_host.apply_overlay_message(msg);
    }

    /// Apply this frame's TAA subpixel jitter to `frame_data` and remember the
    /// NDC offset for `TaaPass`. Call once per frame before `prepare` (#883).
    pub fn apply_taa_jitter(&mut self, frame_data: &mut FrameUBOData) {
        let active = self.settings.taa
            || self.settings.anf_sr
            || self.settings.mobile_upscaler.is_temporal_compute()
            || cfg!(feature = "dlss")
                && self.settings.dlss_mode != crate::render_graph::DlssMode::Off;
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
        if self.settings.internal_resolution != resolution {
            unsafe { self.runtime.context.device.device_wait_idle() }.ok();
        }
        self.settings.internal_resolution = resolution;
        self.settings.internal_scale_mode = mode;
    }

    /// Scene actual render extent (internal render resolution when effective smaller than [`Self::extent`]).
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
        self.graph.warmup_passes(&device, &self.runtime.context)?;

        #[cfg(feature = "ui")]
        {
            let swapchain_format = self.swapchain.as_ref().map(|s| s.format.format);
            let offscreen_format = self.offscreen.as_ref().map(|target| target.format());
            if swapchain_format.is_some() || offscreen_format.is_some() {
                let bindless_layout = self.texture_manager.bindless().layout;
                let ui_start = std::time::Instant::now();
                self.ui_host.warmup(
                    &self.runtime.context,
                    self.runtime.command_pool,
                    swapchain_format,
                    offscreen_format,
                    bindless_layout,
                )?;
                log::debug!(
                    target: "startup",
                    "[startup] warmup: ui overlay {:.3}ms",
                    ui_start.elapsed().as_secs_f64() * 1000.0
                );
            }
        }
        Ok(())
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
        self.surface_suspended = true;
        unsafe { self.runtime.context.device.device_wait_idle() }
            .context("recreate_swapchain: device_wait_idle")?;

        if let Some(forward_pass) = self.graph.pass_mut::<ForwardPass>() {
            forward_pass.drop_target(&self.runtime.context.device);
        }
        self.temporal.reset_history();
        #[cfg(feature = "sharc")]
        if let Some(resolve) = self.graph.pass_mut::<crate::sharc::SharcResolveNode>() {
            resolve.reset_history();
        }
        if let Some(post) = self.graph.pass_mut::<crate::post::PostPass>() {
            post.drop_target(&self.runtime.context.device);
        }
        self.ui_host.on_swapchain_pre_recreate();

        if let Some(sw) = self.swapchain.as_mut() {
            if !sw.recreate_if_drawable(&self.runtime.context)? {
                self.surface_suspended = true;
                return Ok(());
            }
        }

        if let Some(sw) = self.swapchain.as_ref() {
            self.ui_host
                .on_swapchain_post_recreate(&sw.views, sw.extent);
        }

        if let Some((extent, image_count)) = self
            .swapchain
            .as_ref()
            .map(|sw| (sw.extent, sw.views.len()))
        {
            if let Some(forward_pass) = self.graph.pass_mut::<ForwardPass>() {
                forward_pass.set_image_count(image_count);
            }
            #[cfg(feature = "gtao")]
            if let Some(gtao) = self.graph.pass_mut::<crate::gtao::GtaoPass>() {
                if let Err(e) =
                    gtao.recreate_target(&self.runtime.context, self.runtime.command_pool, extent)
                {
                    log::warn!("GtaoPass recreate_target failed: {e:#}");
                }
            }
            #[cfg(feature = "dlss")]
            if let Some(dlss) = self.graph.pass_mut::<crate::dlss_pass::DlssPass>() {
                if let Err(e) = dlss.recreate_target(&self.runtime.context, extent) {
                    log::warn!("DlssPass recreate_target failed: {e:#}");
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

        self.graph.reset_layouts();
        self.surface_suspended = false;
        Ok(())
    }

    /// Adjust **offscreen** render target size (taken when editor viewport panel resizes).
    ///
    /// Order consistent with [`Self::recreate_swapchain`]: first `device_wait_idle`, then tear down
    /// framebuffer referencing old view, then change target, heavy build dependent passes by new size, then clear most
    /// graph cache inside layout (new graph image transitions from UNDEFINED heavy).
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
        self.temporal.reset_history();
        #[cfg(feature = "sharc")]
        if let Some(resolve) = self.graph.pass_mut::<crate::sharc::SharcResolveNode>() {
            resolve.reset_history();
        }
        if let Some(post) = self.graph.pass_mut::<crate::post::PostPass>() {
            post.drop_target(&self.runtime.context.device);
        }
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
        #[cfg(feature = "dlss")]
        if let Some(dlss) = self.graph.pass_mut::<crate::dlss_pass::DlssPass>() {
            if let Err(e) = dlss.recreate_target(&context, extent) {
                log::warn!("DlssPass recreate_target failed (offscreen resize): {e:#}");
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

        self.telemetry.begin_frame_timings(&device, frame as u32);
        self.collect_occlusion_depth(image_index, render_extent);

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

    /// Per-pass GPU time of most recent frame.
    ///
    /// `supported = false` (or `passes` empty) means device does not support timestamp, caller should fall back to
    /// CPU wall clock (`frame_time_ms`). Result **one frame behind** current frame: Vulkan timestamp only in
    /// readable after submission complete; delayed readback avoids stalling pipeline.
    pub fn gpu_timings(&self) -> &crate::gpu_timer::GpuTimings {
        self.telemetry.gpu_timings()
    }

    /// GPU total time of most recent N frames (old → new, #518 Phase 2 history curve).
    pub fn gpu_history(&self) -> &crate::gpu_timer::GpuHistory {
        self.telemetry.gpu_history()
    }

    /// Draw call record of this frame (#518 Phase 2 data layer, not including replay).
    ///
    /// Mesh records one by one by `draw_items` order (even if culled, still recorded, `visible=false`
    /// lets frontend explain "why not drawn"); sprite and particles merged by actual batch (consistent with `SpritePass`
    /// consistent batching by same blend state). Parts exceeding [`MAX_REPORTED_DRAW_CALLS`]
    /// directly discarded, avoiding large scene bloating report beyond readability.
    /// Pipeline state summary of one draw call (#518 Frame Debugger).
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

    /// Texture / mesh / material and VRAM sub-allocator residency statistics (#518 Phase 2, #1030).
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

    /// Get VRAM sub-allocator reference (#1030).
    pub fn suballocator(&self) -> &crate::suballocator::MemorySuballocator {
        &self.suballocator
    }

    /// Get mutable VRAM sub-allocator reference (#1030).
    pub fn suballocator_mut(&mut self) -> &mut crate::suballocator::MemorySuballocator {
        &mut self.suballocator
    }

    /// Get VRAM sub-allocator state statistics (#1030).
    pub fn suballocator_stats(&self) -> crate::suballocator::SuballocatorStats {
        self.suballocator.stats()
    }

    /// Prepare phase: before command recording, batch sync CPU scene state and GPU resources (DESIGN §9 / #1157).
    /// Uniformly consume `DirtyDispatchPlan` and converge execute staged batch uploads and releases.
    pub fn prepare(&mut self, _ctx: &FrameCtx, input: &FrameInput<'_>) -> anyhow::Result<()> {
        self.ensure_lut(input.lut_path)?;

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
    /// render  graph  (ShadowMap → Scene → GTAO → Post), records the external
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
        let mut frame_data = **frame_data;
        let prev_vp = if let Some(camera) = cameras.first() {
            camera.view_proj
        } else {
            let mut vp = frame_data.view_proj;
            vp[3][0] -= self.temporal.taa_jitter_ndc[0];
            vp[3][1] -= self.temporal.taa_jitter_ndc[1];
            vp
        };
        frame_data.prev_view_proj = self.temporal.prev_camera_view_proj.unwrap_or(prev_vp);
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

        let mut record: anyhow::Result<()> = Ok(());

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
                    let light_count = data.camera_position[3];
                    data.view_proj = camera.view_proj;
                    data.view = camera.view;
                    data.camera_position =
                        [camera.eye[0], camera.eye[1], camera.eye[2], light_count];
                    data.exposure = camera.exposure;
                    if let Err(error) = ubo.update(device, &data, slot) {
                        record = Err(error).context("update frame UBO");
                        break;
                    }
                }
            }
        }

        if record.is_ok() {
            let capture = if self.occlusion.enabled && cameras.is_empty() {
                self.ensure_occlusion_target(image_index, render_extent)
            } else {
                None
            };
            if let Some(forward) = self.graph.pass_mut::<ForwardPass>() {
                forward.set_depth_readback(capture);
            }
            let ao_view = self.ao_view(frame as u32);
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
            if let Some(forward) = self.graph.pass_mut::<ForwardPass>() {
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
                view: frame_data.view,
                view_proj: frame_data.view_proj,
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
                camera_pos: frame_data.camera_position,
                light_dir: frame_data.light_direction,
                light_color: frame_data.light_color,
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

            #[cfg(feature = "sharc")]
            if crate::sharc::sharc_active(
                self.settings.pt_gi,
                self.settings.raster_gi,
                *render_mode,
            ) {
                let capacity = self.settings.sharc_capacity.max(1);
                let capacity_changed = self
                    .sharc_targets
                    .as_ref()
                    .is_some_and(|targets| targets.capacity != capacity);
                let mut needs_init_clear = false;
                if capacity_changed {
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
                            let rebound = self
                                .graph
                                .pass_mut::<crate::sharc::SharcResolveNode>()
                                .map(|resolve| resolve.set_targets(&self.runtime.context, &targets))
                                .unwrap_or(Ok(()));
                            match rebound {
                                Ok(()) => {
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

                let invalidation = std::mem::take(&mut self.sharc_invalidation);
                if invalidation.any() {
                    log::debug!(
                        "SHaRC cache invalidated: geometry={} lights={} sky={}",
                        invalidation.geometry,
                        invalidation.lights,
                        invalidation.sky
                    );
                }
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
            #[cfg(all(feature = "pathtrace", not(feature = "sharc")))]
            if crate::render_graph::PtGiMode::Sharc == self.settings.pt_gi {
                static SHARC_MISSING_WARNED: std::sync::Once = std::sync::Once::new();
                SHARC_MISSING_WARNED.call_once(|| {
                    log::warn!(
                        "gi_mode = Sharc requires prism-render/sharc, this build did not compile SHaRC;\
                         degraded to pure path tracing"
                    );
                });
                if let Some(pt) = self.graph.pass_mut::<PathTracePass>() {
                    pt.set_sharc_buffers(None, None, None, 0);
                }
            }
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
                                frame_data.camera_position[0],
                                frame_data.camera_position[1],
                                frame_data.camera_position[2],
                            ],
                            [
                                frame_data.light_direction[0],
                                frame_data.light_direction[1],
                                frame_data.light_direction[2],
                            ],
                            [
                                frame_data.light_color[0],
                                frame_data.light_color[1],
                                frame_data.light_color[2],
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

            #[cfg(feature = "dlss")]
            if let Some(sl) = &self.streamline {
                let sl_dlss_mode = match self.settings.dlss_mode {
                    crate::render_graph::DlssMode::Off => crate::streamline::DlssMode::Off,
                    crate::render_graph::DlssMode::UltraPerformance => {
                        crate::streamline::DlssMode::UltraPerformance
                    }
                    crate::render_graph::DlssMode::Performance => {
                        crate::streamline::DlssMode::MaxPerformance
                    }
                    crate::render_graph::DlssMode::Balanced => {
                        crate::streamline::DlssMode::Balanced
                    }
                    crate::render_graph::DlssMode::Quality => {
                        crate::streamline::DlssMode::MaxQuality
                    }
                    crate::render_graph::DlssMode::Dlaa => crate::streamline::DlssMode::Dlaa,
                };
                if sl_dlss_mode != crate::streamline::DlssMode::Off
                    || self.settings.dlfg_mode != crate::render_graph::DlssgMode::Off
                {
                    let constants = crate::streamline::SlConstants {
                        camera_view_to_clip: frame_data.view_proj,
                        clip_to_camera_view: inv_projection,
                        jitter_offset: self.temporal.taa_jitter_ndc,
                        mvec_scale: [-1.0, -1.0],
                        camera_pos: [
                            frame_data.camera_position[0],
                            frame_data.camera_position[1],
                            frame_data.camera_position[2],
                        ],
                        camera_aspect_ratio: extent.width as f32 / extent.height.max(1) as f32,
                        ..Default::default()
                    };
                    let _ = sl.set_frame_constants(ctx.frame_index, &constants);
                    if self.settings.dlfg_mode != crate::render_graph::DlssgMode::Off
                        && sl.caps().dlss_fg_supported
                    {
                        let dlfg_opts = crate::streamline::DlssgOptions {
                            mode: self.settings.dlfg_mode,
                            num_frames_to_generate: self.settings.dlfg_frames.clamp(1, 3),
                            color_width: extent.width,
                            color_height: extent.height,
                            mvec_depth_width: render_extent.width,
                            mvec_depth_height: render_extent.height,
                            ..Default::default()
                        };
                        let _ = sl.configure_dlfg(&dlfg_opts);
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
                        *slot = Some(frame_data.view_proj);
                    }
                }
            }
        }

        /// Log a slow UI-overlay record, which is where lazy glyph-atlas and pipeline
        /// creation land on the first frame.
        #[cfg(feature = "ui")]
        fn log_slow_ui(path: &str, elapsed: std::time::Duration) {
            let ms = elapsed.as_secs_f64() * 1000.0;
            if ms >= 1.0 {
                log::debug!(target: "startup", "[startup] ui_overlay record ({path}) {ms:.3}ms");
            }
        }

        #[cfg(feature = "ui")]
        if record.is_ok() {
            let has_ui = ui_overlay.is_some_and(|o| !o.quads.is_empty() || !o.texts.is_empty());
            if has_ui {
                let bindless_set = self.texture_manager.bindless().set;
                let bindless_layout = self.texture_manager.bindless().layout;

                if let Some(sw) = self.swapchain.as_ref() {
                    let dest = crate::render_graph::post_destination(
                        extent,
                        render_extent,
                        self.settings.internal_scale_mode,
                    );
                    let ui_start = std::time::Instant::now();
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
                        sw.transfer_src,
                    );
                    log_slow_ui("swapchain", ui_start.elapsed());
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
                    let dest = vk::Rect2D {
                        offset: vk::Offset2D { x: 0, y: 0 },
                        extent: target_extent,
                    };
                    let ui_start = std::time::Instant::now();
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
                    log_slow_ui("offscreen", ui_start.elapsed());
                }
            }
        }

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

    /// Frame end phase: advance frame-level state and execute deferred recycling.
    pub fn end_frame(&mut self, _ctx: &FrameCtx, _presented: bool) -> anyhow::Result<()> {
        self.graph.flush_pass_dump(&self.runtime.context.device)?;
        Ok(())
    }

    /// Render a frame: one-shot convenience that calls [`begin_frame`],
    /// [`execute`], and [`present`] in order.
    ///
    /// This is a compatibility wrapper; new code should prefer the explicit
    /// phase API for finer error handling and future prepare-stage insertion.
    #[expect(
        clippy::too_many_arguments,
        reason = "arity mirrors the float-parameter packing that the callers already flatten"
    )]
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
        let mut frame_data = *frame_data;
        frame_data.shadow_cascade_count = frame_data
            .shadow_cascade_count
            .clamp(1, self.settings.shadow_cascade_count.clamp(1, 2));
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
            light_view_proj1: frame_data.light_view_proj1,
            shadow_split: frame_data.shadow_split,
            shadow_cascade_count: frame_data.shadow_cascade_count,
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
        exec_result?;
        self.end_frame(&ctx, out_of_date)?;
        Ok(out_of_date)
    }

    /// Release all GPU resources.
    /// Schedule one screenshot: next frame `execute` copies swap chain graph image to readback buffer,
    /// `finish_screenshot` then writes PNG.
    ///
    /// When `size` is `None`, consistent with window resolution; when given size, nearest-neighbor scale
    /// (pixels enlarged won't be blurry).
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

    /// Called after frame submission: waits for submission fence, reads back pixels, writes PNG.
    ///
    /// Returns `Ok(None)` when no screenshot in progress.
    pub fn finish_screenshot(
        &mut self,
        ctx: &FrameCtx,
    ) -> anyhow::Result<Option<std::path::PathBuf>> {
        self.telemetry
            .finish_screenshot(&self.runtime.context, ctx.fence, self.color_format)
    }

    /// Active NVIDIA NRD library version `(major, minor, build)` if loaded.
    #[must_use]
    #[cfg(feature = "nrd")]
    pub fn nrd_version(&self) -> Option<(u8, u8, u8)> {
        self.nrd_runtime
            .as_ref()
            .map(crate::nrd::NrdRuntime::version)
    }

    pub fn destroy(&mut self) {
        self.destroy_occlusion_targets();
        let device = &self.runtime.context.device;
        unsafe { device.device_wait_idle() }.ok();

        #[cfg(feature = "dlss")]
        {
            if let Some(dlss) = self.graph.pass_mut::<crate::dlss_pass::DlssPass>() {
                dlss.release_streamline();
            }
            self.streamline = None;
        }

        self.telemetry.destroy(device);
        self.graph.discard_pass_dump(device);

        let ibl_ds_layout = self.ibl.descriptor_set_layout;
        self.ibl.destroy();
        if ibl_ds_layout != vk::DescriptorSetLayout::null() {
            unsafe { device.destroy_descriptor_set_layout(ibl_ds_layout, None) };
        }

        self.material_manager.destroy(device);
        self.texture_manager.destroy();
        self.mesh_manager.destroy(device);
        self.suballocator.destroy(device);

        if let Some(forward_pass) = self.graph.pass_mut::<ForwardPass>() {
            forward_pass.destroy(device);
        }

        self.ddgi_pass = None;
        self.scene_scope.destroy();

        #[cfg(feature = "gtao")]
        if let Some(gtao) = self.graph.pass_mut::<crate::gtao::GtaoPass>() {
            gtao.destroy(device);
        }
        self.placeholders.destroy(device);

        if let Some(post) = self.graph.pass_mut::<crate::post::PostPass>() {
            post.destroy(device);
        }

        if let Some(auto_exposure) = self
            .graph
            .pass_mut::<crate::auto_exposure::AutoExposurePass>()
        {
            auto_exposure.destroy(device);
        }

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

        #[cfg(feature = "shadows")]
        if let Some(shadow) = self
            .graph
            .pass_mut::<crate::shadow_map_pass::ShadowMapPass>()
        {
            shadow.destroy(device);
        }

        self.ui_host.destroy();

        unsafe { device.destroy_sampler(self.shadow_sampler, None) };

        self.graph.destroy(device);

        unsafe { device.destroy_command_pool(self.runtime.command_pool, None) };

        if let Some(mut sw) = self.swapchain.take() {
            unsafe { sw.destroy(device) };
        }

        if let Some(mut target) = self.offscreen.take() {
            unsafe { target.destroy(device) };
        }

        if self.headless_fence != vk::Fence::null() {
            unsafe { device.destroy_fence(self.headless_fence, None) };
            self.headless_fence = vk::Fence::null();
        }
    }
}

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
