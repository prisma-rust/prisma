//! Forward PBR main render pass.
//!
//! [`ForwardPass`] renders entire game scene: environment skybox + all meshes, fragment shading
//! phase completes PBR lighting in one go (IBL + directional shadow + GTAO ambient occlusion +
//! global light probe sampling), outputs into HDR intermediate target of each swapchain image
//! (color / depth / view-space normal MRT), sampled by [`crate::post::PostPass`]
//! and tonemapped to swapchain.
//!
//! Owns one set of framebuffers per swapchain image (rebuilt only when swapchain view changes),
//! plus embedded [`SkyboxPass`] and [`crate::gizmo::Gizmo`].

use anyhow::Context as _;
use anyhow::Result;
use ash::vk;
use std::collections::HashMap;
use std::time::Instant;

use crate::context::VulkanContext;
use crate::gizmo::Gizmo;
use crate::ground_grid::GroundGrid;
use crate::mesh::Vertex;
use crate::pipeline::{blend_factor, blend_operation, cull_flags, GraphicsPipeline, PipelineDesc};
#[cfg(feature = "shadows")]
use crate::render_graph::SHADOW_MAP_H;
use crate::render_graph::{
    DrawState, GraphResources, PassInfo, PassKind, RenderContext, RenderGraphBuilder, RenderMode,
    RenderPassNode, RenderSettings, ResourceHandle, ResourceType, ResourceUsage, FORWARD_ALBEDO_H,
    FORWARD_COLOR_H, FORWARD_DEPTH_H, FORWARD_NORMAL_H, FORWARD_VELOCITY_H,
};
use crate::shader;
use crate::shader_bindings::{mesh_vert, scene_frag};
use crate::skybox_pass::SkyboxPass;

const _: () = assert!(
    std::mem::size_of::<scene_frag::PbrBindlessPush>() as u32 == scene_frag::PUSH_CONSTANT_SIZE
);

/// Grouping key for static draws that can share one instanced draw call (#491).
///
/// The model matrix is deliberately absent: it becomes one entry in the bone
/// SSBO. Skinned, transparent and missing-mesh draws never enter this path.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct StaticBatchKey {
    mesh: crate::managers::MeshHandle,
    material: Option<u32>,
    shader: Option<crate::ShaderHandle>,
    state: DrawState,
    index_start: u32,
    index_count: Option<u32>,
}

struct StaticBatch {
    items: Vec<usize>,
    /// First entry in the packed bone/instance SSBO; `None` = single draw.
    instance_offset: Option<u32>,
}

/// Group visible opaque static draws by mesh / material / pipeline state.
fn build_static_batches_with(
    draw_list: &[crate::render_graph::DrawItem],
    mesh_exists: impl Fn(crate::managers::MeshHandle) -> bool,
) -> Vec<StaticBatch> {
    let mut batches: Vec<StaticBatch> = Vec::new();
    let mut slots: HashMap<StaticBatchKey, usize> = HashMap::new();
    for (index, item) in draw_list.iter().enumerate() {
        if !item.visible_in_main_pass || item.index_count == Some(0) || !mesh_exists(item.mesh) {
            continue;
        }
        let groupable = item.bone_offset.is_none()
            && item.splat_tiling == 0.0
            && item.state.render_queue <= prism_gfx::RenderQueue::GEOMETRY_LAST;
        if !groupable {
            batches.push(StaticBatch {
                items: vec![index],
                instance_offset: None,
            });
            continue;
        }
        let key = StaticBatchKey {
            mesh: item.mesh,
            material: item.material,
            shader: item.shader,
            state: item.state,
            index_start: item.index_start,
            index_count: item.index_count,
        };
        let slot = *slots.entry(key).or_insert_with(|| {
            batches.push(StaticBatch {
                items: Vec::new(),
                instance_offset: None,
            });
            batches.len() - 1
        });
        batches[slot].items.push(index);
    }
    batches
}

/// Assign instance offsets and split groups that do not fit `capacity`.
fn prepare_static_batches(
    draw_list: &[crate::render_graph::DrawItem],
    mesh_manager: &crate::managers::RenderMeshManager,
    capacity: usize,
) -> (Vec<StaticBatch>, Vec<[[f32; 4]; 4]>) {
    prepare_static_batches_with(draw_list, |mesh| mesh_manager.get(mesh).is_some(), capacity)
}

fn prepare_static_batches_with(
    draw_list: &[crate::render_graph::DrawItem],
    mesh_exists: impl Fn(crate::managers::MeshHandle) -> bool,
    capacity: usize,
) -> (Vec<StaticBatch>, Vec<[[f32; 4]; 4]>) {
    let mut out = Vec::new();
    let mut matrices: Vec<[[f32; 4]; 4]> = Vec::new();
    for batch in build_static_batches_with(draw_list, mesh_exists) {
        if batch.items.len() > 1 && matrices.len() + batch.items.len() <= capacity {
            let mut batch = batch;
            batch.instance_offset = Some(matrices.len() as u32);
            matrices.extend(batch.items.iter().map(|&i| draw_list[i].model));
            out.push(batch);
        } else if batch.items.len() == 1 {
            out.push(batch);
        } else {
            out.extend(batch.items.into_iter().map(|i| StaticBatch {
                items: vec![i],
                instance_offset: None,
            }));
        }
    }
    (out, matrices)
}

/// Rust mirror of `assets/shaders/slang/outline.slang::OutlinePush`.
///
/// The reverse-facing shell only reads frame UBO (set 0 binding 0) and one push constant block; reflection report
/// block size 96 bytes (mat4 + vec4 + float + std140 tail padding).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct OutlinePush {
    model: [[f32; 4]; 4],
    color: [f32; 4],
    thickness: f32,
    _pad: [f32; 3],
}

/// Forward scene pass (bindless PBR + neutral ambient + shadow map) targeting
/// the swapchain.
///
/// Descriptor set layout (mirrors `scene_frag.slang`):
///   set 0 - per-frame UBO (binding 0) + material SSBO (binding 1)
///            one descriptor set per frame-in-flight (UBO buffer differs)
///   set 1 - bindless texture table (samplers + SRV array, owned by
///            `RenderTextureManager::bindless`)
///   set 2 - IBL resources (3 combined image samplers: env, irradiance, prefiltered)
///   set 3 - shadow map (SAMPLED_IMAGE + comparison SAMPLER)
///   set 4 - previous-frame GTAO R8 visibility texture (combined image sampler)
pub struct ForwardPass {
    /// High dynamic range intermediate color format (the ForwardPass no longer targets the
    /// swapchain directly; PostPass tonemaps HDR -> swapchain).
    color_format: vk::Format,
    /// Format of the view-space normal MRT attachment (SV_Target1). Written by
    /// the scene fragment shader and read by the GTAO pass.
    normal_format: vk::Format,
    /// #883 Phase 2: screen-space motion vectors (SV_Target2). RGBA16F keeps
    /// linear sampling universally available; only .xy are used.
    velocity_format: vk::Format,
    /// Format of the albedo + perceptual-roughness MRT attachment (SV_Target3).
    /// RGB = base color (post texture, pre lighting), A = perceptual roughness.
    /// Written by the scene fragment shader and read by RestirGiPass to
    /// demodulate / re-modulate indirect diffuse.
    albedo_format: vk::Format,
    /// Bindless handle for the BRDF LUT (registered in the bindless texture table).
    brdf_handle: u32,
    /// Byte stride between camera slots in the per-frame UBO (#461).
    camera_stride: u32,
    /// One framebuffer per swapchain image. With N swapchain images and N
    /// frames in flight, several command buffers can reference their
    /// respective framebuffers concurrently - so we can't keep just one
    /// rotating framebuffer (destroying it while a prior frame's command
    /// buffer still references it triggers
    /// VUID-vkDestroyFramebuffer-framebuffer-00892 and cascades into a
    /// device-lost). Indexed by `image_index` from `acquire_next_image`.
    framebuffers: Vec<Option<vk::Framebuffer>>,
    /// One high dynamic range color image per swapchain image (the ForwardPass render target
    /// replacing the old direct-to-swapchain path). Reused by PostPass as its
    /// sampled input.
    color_images: Vec<Option<crate::render_pass::NormalImage>>,
    /// One depth image per swapchain image (each framebuffer references its
    /// own depth view). Parallel to `framebuffers`.
    depth_images: Vec<Option<crate::render_pass::DepthImage>>,
    /// #1007: this frame copies main pass depth to this host-visible buffer (`None` = disabled).
    depth_readback: Option<vk::Buffer>,
    /// Most recent frame number that actually recorded a depth copy (keeps old value when main pass not rendered).
    depth_capture_frame: Option<u32>,
    /// One view-space normal image per swapchain image (MRT SV_Target1). Same
    /// per-slot lifetime as `depth_images`: rebuilt only when its swapchain
    /// view changes.
    normal_images: Vec<Option<crate::render_pass::NormalImage>>,
    /// One screen-space motion-vector image per swapchain image (#883 Phase 2).
    velocity_images: Vec<Option<crate::render_pass::NormalImage>>,
    /// One albedo + roughness image per swapchain image (MRT SV_Target3).
    /// Same per-slot lifetime as the other G-buffer attachments.
    albedo_images: Vec<Option<crate::render_pass::NormalImage>>,
    /// Cached image_index validity markers (one per slot). `set_target` uses
    /// `framebuffers[idx].is_some()` as the "current" check; this field is
    /// kept for parity with the old swapchain-view tracking pattern.
    target_views: Vec<vk::ImageView>,
    /// Number of swapchain images. Set by `set_image_count` (called from
    /// `GraphRenderer::recreate_swapchain` after the swapchain is recreated)
    /// and used by `ensure_target` so the per-image framebuffer vectors are
    /// sized correctly. Decouples framebuffer (re)creation from
    /// `GraphRenderer`'s per-frame call sequence.
    image_count: usize,
    /// Graph resource handles for this pass's outputs, created in `setup` and
    /// published (view registered) in `execute` so downstream passes
    /// (`GtaoPass`, `PostPass`) read them by handle instead of `GraphRenderer`
    /// poking into `ForwardPass` internals. The graph  does not allocate the
    /// underlying images (ForwardPass still owns its framebuffers in PR-1);
    /// only the handle->view mapping lives in `GraphResources`.
    out_color_h: ResourceHandle,
    out_depth_h: ResourceHandle,
    out_normal_h: ResourceHandle,
    out_velocity_h: ResourceHandle,
    out_albedo_h: ResourceHandle,
    extent: vk::Extent2D,
    render_pass: Option<vk::RenderPass>,
    /// Main pass pipeline cache: one per [`DrawState`] (cull / depth write / blend).
    ///
    /// All variants' descriptor set layouts / push constant ranges fully identical, only rasterization and color
    /// blend state differ, so descriptor binds and push constants can be reused across variants within one draw list.
    /// Key's first item is optional custom fragment shader (`None` = built-in PBR),
    /// third item is wireframe switch (#431): `POLYGON_MODE_LINE` needs separate pipeline.
    pipelines: std::collections::HashMap<
        (Option<crate::ShaderHandle>, DrawState, bool, bool),
        GraphicsPipeline,
    >,
    /// Registered custom fragment shader SPIR-V bytecodes, indexed by `ShaderHandle.0`.
    custom_shaders: Vec<Vec<u8>>,
    /// Content-hash deduplication map for `custom_shaders`.
    custom_shader_hashes: std::collections::HashMap<u64, crate::ShaderHandle>,
    /// Selected-outline reverse shell pipeline (#487): reads only set 0 frame UBO, fragment outputs solid color.
    /// Shares vertex format / render pass / depth attachment with scene meshes, only culls front face, no depth write.
    outline_pipeline: Option<GraphicsPipeline>,
    /// Fill + wireframe overlay pass (#431). Reuses outline shader (thickness 0, solid color),
    /// `POLYGON_MODE_LINE` + negative depth bias pushes lines onto filled surface.
    shaded_wireframe_pipeline: Option<GraphicsPipeline>,
    ibl_descriptor_set: vk::DescriptorSet,
    /// IBL descriptor set layout (borrowed from `IblResources`). Used by the
    /// skybox pass to build its pipeline layout.
    ibl_layout: vk::DescriptorSetLayout,
    shadow_ds_layout: Option<vk::DescriptorSetLayout>,
    shadow_descriptor_set: vk::DescriptorSet,
    shadow_ds_pool: Option<vk::DescriptorPool>,
    /// set 0 - per-frame-in-flight descriptor sets binding the frame UBO
    /// (binding 0) + the material SSBO (binding 1). Indexed by
    /// `frame_index` (frame-in-flight, 0..N), NOT swapchain image_index.
    frame_sets: Vec<vk::DescriptorSet>,
    /// set 0 layout (frame UBO + materials SSBO). Owned + destroyed on drop.
    frame_set_layout: Option<vk::DescriptorSetLayout>,
    /// Pool backing `frame_sets`. Owned + destroyed on drop.
    frame_set_pool: Option<vk::DescriptorPool>,
    /// set 1 - bindless texture table descriptor set (from
    /// `RenderTextureManager::bindless()`). Not owned by ForwardPass.
    bindless_set: vk::DescriptorSet,
    /// set 1 layout (from `BindlessTextureTable::layout`). Borrowed for
    /// pipeline-layout creation; not destroyed by ForwardPass.
    bindless_layout: vk::DescriptorSetLayout,
    /// light source  SSBO (set  0 bind  2): host-visible buffer  holding up to
    /// `LIGHT_MAX` hard-coded point lights. One per frame-in-flight to avoid
    /// tearing and race conditions between concurrent in-flight frames.
    light_buffers: Vec<vk::Buffer>,
    light_memories: Vec<vk::DeviceMemory>,
    /// Bone palette SSBO (set 0 binding 3, #560 P0-3): one per frame in
    /// flight, `BONE_CAPACITY` mat4 entries each.
    bone_buffers: Vec<vk::Buffer>,
    bone_memories: Vec<vk::DeviceMemory>,
    /// Previous-frame model matrices (#883 Phase 2), indexed by draw-list
    /// position. Separate from the bone SSBO so uploads cannot clobber skinned
    /// palettes; read by the vertex stage as set 0 binding 4.
    prev_model_buffers: Vec<vk::Buffer>,
    prev_model_memories: Vec<vk::DeviceMemory>,
    /// set 4 - previous-frame GTAO R8 visibility texture (combined image
    /// sampler). One descriptor set per frame-in-flight so updating the AO
    /// view for frame N doesn't disturb frame N-1's still-in-flight set.
    ao_ds_layout: Option<vk::DescriptorSetLayout>,
    /// One AO descriptor set per frame-in-flight (parallels `frame_sets`).
    ao_descriptor_sets: Vec<vk::DescriptorSet>,
    ao_ds_pool: Option<vk::DescriptorPool>,
    ao_sampler: vk::Sampler,
    /// The AO view currently bound to each frame-in-flight's AO descriptor
    /// set. Tracked so we skip redundant descriptor rewrites.
    ao_views: Vec<vk::ImageView>,
    /// set 4 binding 1 - previous-frame ReSTIR GI (RGBA16F, kept in
    /// `VK_IMAGE_LAYOUT_GENERAL` by the RestirGi pass). Tracked per
    /// frame-in-flight exactly like `ao_views`.
    gi_views: Vec<vk::ImageView>,
    /// Last time the AO_PROBE debug line in `set_ao` was logged; throttled to
    /// once per second so it doesn't flood the log at frame rate.
    last_probe_log: Instant,
    /// set 5 - probe volume GI (borrowed from `SceneScope`, scene-level).
    /// binding 0: 3D texture (SAMPLED_IMAGE), binding 1: ProbeVolumeInfo UBO.
    gi_descriptor_set: vk::DescriptorSet,
    /// GI descriptor set layout (borrowed from `SceneScope`). Used for
    /// pipeline-layout creation; NOT destroyed by ForwardPass.
    gi_layout: vk::DescriptorSetLayout,
    /// Skybox background pass (draws the IBL env cubemap). Owns its pipeline +
    /// set-2 (IBL env) layout; borrows the IBL descriptor set.
    skybox: SkyboxPass,
    /// 2D sprite pass (instanced quads, bindless textures). Drawn after the
    /// 3D mesh list and before the debug gizmo; owns its pipeline + a
    /// per-frame instance buffer.
    sprites: crate::sprite_pass::SpritePass,
    /// 3D particle pass (camera-facing billboards with a soft radial falloff).
    /// Drawn after the opaque mesh list (depth test on, write off) and before
    /// the 2D sprite pass; owns its pipeline + a per-frame instance buffer.
    particles: crate::particle_pass::ParticlePass,
    /// World-space XYZ orientation gizmo, drawn on top of the scene (depth
    /// test disabled). Built lazily once the render pass exists.
    gizmo: Option<Gizmo>,
    /// Ground reference mesh (Y = 0), depth test enabled to be naturally occluded by scene geometry
    ground_grid: Option<GroundGrid>,
    device: Option<ash::Device>,
}
impl ForwardPass {
    pub fn new(_swapchain_color_format: vk::Format) -> Self {
        Self {
            color_format: vk::Format::R16G16B16A16_SFLOAT,
            normal_format: vk::Format::R16G16B16A16_SFLOAT,
            velocity_format: vk::Format::R16G16B16A16_SFLOAT,
            albedo_format: vk::Format::R16G16B16A16_SFLOAT,
            brdf_handle: u32::MAX,
            camera_stride: 0,
            framebuffers: Vec::new(),
            color_images: Vec::new(),
            depth_images: Vec::new(),
            depth_readback: None,
            depth_capture_frame: None,
            normal_images: Vec::new(),
            velocity_images: Vec::new(),
            albedo_images: Vec::new(),
            target_views: Vec::new(),
            image_count: 0,
            out_color_h: ResourceHandle::INVALID,
            out_depth_h: ResourceHandle::INVALID,
            out_normal_h: ResourceHandle::INVALID,
            out_velocity_h: ResourceHandle::INVALID,
            out_albedo_h: ResourceHandle::INVALID,
            extent: vk::Extent2D {
                width: 0,
                height: 0,
            },
            render_pass: None,
            pipelines: std::collections::HashMap::new(),
            custom_shaders: Vec::new(),
            custom_shader_hashes: std::collections::HashMap::new(),
            outline_pipeline: None,
            shaded_wireframe_pipeline: None,
            ibl_descriptor_set: vk::DescriptorSet::null(),
            ibl_layout: vk::DescriptorSetLayout::null(),
            shadow_ds_layout: None,
            shadow_descriptor_set: vk::DescriptorSet::null(),
            shadow_ds_pool: None,
            frame_sets: Vec::new(),
            frame_set_layout: None,
            frame_set_pool: None,
            bindless_set: vk::DescriptorSet::null(),
            bindless_layout: vk::DescriptorSetLayout::null(),
            light_buffers: Vec::new(),
            light_memories: Vec::new(),
            bone_buffers: Vec::new(),
            bone_memories: Vec::new(),
            prev_model_buffers: Vec::new(),
            prev_model_memories: Vec::new(),
            ao_ds_layout: None,
            ao_descriptor_sets: Vec::new(),
            ao_ds_pool: None,
            ao_sampler: vk::Sampler::null(),
            ao_views: Vec::new(),
            last_probe_log: Instant::now(),
            gi_views: Vec::new(),
            gi_descriptor_set: vk::DescriptorSet::null(),
            gi_layout: vk::DescriptorSetLayout::null(),
            skybox: SkyboxPass::new(vk::DescriptorSet::null(), vk::DescriptorSetLayout::null()),
            sprites: crate::sprite_pass::SpritePass::new(),
            particles: crate::particle_pass::ParticlePass::new(),
            gizmo: None,
            ground_grid: None,
            device: None,
        }
    }

    /// Register a custom fragment shader SPIR-V module, deduplicating identical bytecodes.
    pub fn register_custom_shader(&mut self, spv: Vec<u8>) -> crate::ShaderHandle {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in &spv {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
        if let Some(&existing) = self.custom_shader_hashes.get(&hash) {
            if self.custom_shaders.get(existing.0 as usize) == Some(&spv) {
                return existing;
            }
        }
        let handle = crate::ShaderHandle(self.custom_shaders.len() as u32);
        self.custom_shaders.push(spv);
        self.custom_shader_hashes.insert(hash, handle);
        handle
    }

    /// Ensure the framebuffer for `image_index` exists and is built against the
    /// current extent. Returns the framebuffer handle via
    /// `self.framebuffers[image_index]` (read by `execute`).
    ///
    /// With N swapchain images and N frames in flight, several command buffers
    /// can be in flight at once - each referencing its own framebuffer. So we
    /// keep **one framebuffer per swapchain image** (plus its own HDR color +
    /// depth + normal image) and only rebuild an entry when the extent changed.
    /// This avoids destroying a framebuffer that a prior (still in-flight)
    /// command buffer references (VUID-vkDestroyFramebuffer-framebuffer-00892).
    ///
    /// `image_index` is the value returned by `acquire_next_image`;
    /// `image_count` is `swapchain.views.len()` (so we can size the per-slot
    /// vectors on the first call or after a recreate).
    pub fn set_image_count(&mut self, image_count: usize) {
        self.image_count = image_count;
    }

    /// Idempotent per-frame (re)creation of this swapchain image's
    /// frame buffer + HDR/depth/normal attachments. Called from `ForwardPass::`
    /// execute` (driven by the `RenderGraph`) so framebuffer lifecycle no
    /// longer depends on `GraphRenderer` calling `set_target` every frame.
    /// Rebuilds only the entry for `image_index` when it is missing or the
    /// swapchain changed; safe against in-flight framebuffers (mirrors the
    /// old `set_target` contract).
    /// Switch IBL descriptor set handle and BRDF LUT bindless slot on scene change.
    /// set2 layout shared (not rebuilt), only replace set handle and sync skybox pass's set2 bind.
    pub fn set_ibl(&mut self, descriptor_set: vk::DescriptorSet, brdf_handle: u32) {
        self.ibl_descriptor_set = descriptor_set;
        self.brdf_handle = brdf_handle;
        self.skybox.set_descriptor_set(descriptor_set);
    }

    pub fn ensure_target(
        &mut self,
        device: &ash::Device,
        context: &crate::context::VulkanContext,
        image_index: u32,
        extent: vk::Extent2D,
    ) -> Result<()> {
        self.set_target(device, context, self.image_count, image_index, extent)
    }

    pub fn set_target(
        &mut self,
        device: &ash::Device,
        context: &crate::context::VulkanContext,
        image_count: usize,
        image_index: u32,
        extent: vk::Extent2D,
    ) -> Result<()> {
        if extent.width == 0 || extent.height == 0 {
            return Ok(());
        }

        let idx = image_index as usize;
        if idx >= image_count {
            return Ok(());
        }

        self.ensure_render_pass(context)?;

        let swapchain_changed = self.target_views.len() != image_count || self.extent != extent;
        if swapchain_changed {
            self.drop_target(device);
            self.target_views = vec![vk::ImageView::null(); image_count];
            self.extent = extent;
            self.framebuffers = (0..image_count).map(|_| None).collect();
            self.color_images = (0..image_count).map(|_| None).collect();
            self.depth_images = (0..image_count).map(|_| None).collect();
            self.normal_images = (0..image_count).map(|_| None).collect();
            self.velocity_images = (0..image_count).map(|_| None).collect();
            self.albedo_images = (0..image_count).map(|_| None).collect();
        }

        let already_current = self.framebuffers[idx].is_some();
        if !already_current {
            let rp = self
                .render_pass
                .context("ForwardPass: render_pass missing in set_target")?;

            let color_image =
                crate::render_pass::NormalImage::new(context, extent, self.color_format)
                    .context("ForwardPass: create HDR color image")?;
            if let Some(mut old) = self.color_images[idx].take() {
                unsafe { old.destroy(device) };
            }
            self.color_images[idx] = Some(color_image);

            let depth_image = crate::render_pass::DepthImage::new(context, extent)
                .context("ForwardPass: create depth image")?;
            if let Some(mut old) = self.depth_images[idx].take() {
                unsafe { old.destroy(device) };
            }
            self.depth_images[idx] = Some(depth_image);

            let normal_image =
                crate::render_pass::NormalImage::new(context, extent, self.normal_format)
                    .context("ForwardPass: create normal image")?;
            if let Some(mut old) = self.normal_images[idx].take() {
                unsafe { old.destroy(device) };
            }
            self.normal_images[idx] = Some(normal_image);

            let velocity_image =
                crate::render_pass::NormalImage::new(context, extent, self.velocity_format)
                    .context("ForwardPass: create velocity image")?;
            if let Some(mut old) = self.velocity_images[idx].take() {
                unsafe { old.destroy(device) };
            }
            self.velocity_images[idx] = Some(velocity_image);

            let albedo_image =
                crate::render_pass::NormalImage::new(context, extent, self.albedo_format)
                    .context("ForwardPass: create albedo image")?;
            if let Some(mut old) = self.albedo_images[idx].take() {
                unsafe { old.destroy(device) };
            }
            self.albedo_images[idx] = Some(albedo_image);

            if let Some(old_fb) = self.framebuffers[idx].take() {
                unsafe { device.destroy_framebuffer(old_fb, None) };
            }

            let color = self.color_images[idx]
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("forward color image missing"))?;
            let depth = self.depth_images[idx]
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("forward depth image missing"))?;
            let normal = self.normal_images[idx]
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("forward normal image missing"))?;
            let velocity = self.velocity_images[idx]
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("forward velocity image missing"))?;
            let albedo = self.albedo_images[idx]
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("forward albedo image missing"))?;
            let attachments = [
                color.view,
                depth.view,
                normal.view,
                velocity.view,
                albedo.view,
            ];
            let fb = unsafe {
                device.create_framebuffer(
                    &vk::FramebufferCreateInfo::default()
                        .render_pass(rp)
                        .attachments(&attachments)
                        .width(extent.width)
                        .height(extent.height)
                        .layers(1),
                    None,
                )
            }
            .context("ForwardPass: create framebuffer")?;
            self.framebuffers[idx] = Some(fb);
        }
        Ok(())
    }

    /// Drop the swapchain-derived framebuffers + depth images.
    ///
    /// Must be called **before** the swapchain is recreated (and from
    /// `set_target` when the swapchain changes): each framebuffer wraps a
    /// swapchain image view + depth view, and `Swapchain::recreate` destroys
    /// the old views. Destroying the views while the framebuffers still
    /// reference them triggers `vkDestroyImageView` validation errors which
    /// cascade into a device-lost on the next submit.
    ///
    /// Framebuffers are destroyed before their depth images (each framebuffer
    /// references its depth view as an attachment). The render pass + pipeline
    /// are kept (they don't reference swapchain views); `set_target` rebuilds
    /// the framebuffers + depth on the next frame.
    pub fn drop_target(&mut self, device: &ash::Device) {
        for fb in self.framebuffers.drain(..).flatten() {
            unsafe { device.destroy_framebuffer(fb, None) };
        }
        for color in self.color_images.drain(..).flatten() {
            let mut c = color;
            unsafe { c.destroy(device) };
        }
        for depth in self.depth_images.drain(..).flatten() {
            let mut d = depth;
            unsafe { d.destroy(device) };
        }
        for normal in self.normal_images.drain(..).flatten() {
            let mut n = normal;
            unsafe { n.destroy(device) };
        }
        for velocity in self.velocity_images.drain(..).flatten() {
            let mut v = velocity;
            unsafe { v.destroy(device) };
        }
        for albedo in self.albedo_images.drain(..).flatten() {
            let mut a = albedo;
            unsafe { a.destroy(device) };
        }
        self.target_views.clear();
        self.extent = vk::Extent2D {
            width: 0,
            height: 0,
        };
    }

    /// Tear down ALL ForwardPass GPU resources (framebuffers, depth images,
    /// render pass, pipeline, shadow descriptor set layout + pool).
    ///
    /// Called from `GraphRenderer::destroy` on shutdown. After this the
    /// ForwardPass is empty  `device_wait_idle` must already have been called by
    /// the caller so no command buffers are in flight.
    pub fn destroy(&mut self, device: &ash::Device) {
        self.drop_target(device);

        if let Some(rp) = self.render_pass.take() {
            unsafe { device.destroy_render_pass(rp, None) };
        }
        self.pipelines.clear();
        self.shaded_wireframe_pipeline = None;

        if let Some(layout) = self.frame_set_layout.take() {
            unsafe { device.destroy_descriptor_set_layout(layout, None) };
        }
        if let Some(pool) = self.frame_set_pool.take() {
            unsafe { device.destroy_descriptor_pool(pool, None) };
        }
        self.frame_sets.clear();

        if let Some(layout) = self.shadow_ds_layout.take() {
            unsafe { device.destroy_descriptor_set_layout(layout, None) };
        }
        if let Some(pool) = self.shadow_ds_pool.take() {
            unsafe { device.destroy_descriptor_pool(pool, None) };
        }
        self.shadow_descriptor_set = vk::DescriptorSet::null();
        for buf in self.light_buffers.drain(..) {
            if buf != vk::Buffer::null() {
                unsafe { device.destroy_buffer(buf, None) };
            }
        }
        for mem in self.light_memories.drain(..) {
            if mem != vk::DeviceMemory::null() {
                unsafe { device.free_memory(mem, None) };
            }
        }
        for buf in self.bone_buffers.drain(..) {
            if buf != vk::Buffer::null() {
                unsafe { device.destroy_buffer(buf, None) };
            }
        }
        for mem in self.bone_memories.drain(..) {
            if mem != vk::DeviceMemory::null() {
                unsafe { device.free_memory(mem, None) };
            }
        }
        for buf in self.prev_model_buffers.drain(..) {
            if buf != vk::Buffer::null() {
                unsafe { device.destroy_buffer(buf, None) };
            }
        }
        for mem in self.prev_model_memories.drain(..) {
            if mem != vk::DeviceMemory::null() {
                unsafe { device.free_memory(mem, None) };
            }
        }
        self.skybox.destroy(device);
        self.gizmo = None;
        self.ground_grid = None;

        if let Some(layout) = self.ao_ds_layout.take() {
            unsafe { device.destroy_descriptor_set_layout(layout, None) };
        }
        if let Some(pool) = self.ao_ds_pool.take() {
            unsafe { device.destroy_descriptor_pool(pool, None) };
        }
        if self.ao_sampler != vk::Sampler::null() {
            unsafe { device.destroy_sampler(self.ao_sampler, None) };
            self.ao_sampler = vk::Sampler::null();
        }
        self.ao_descriptor_sets.clear();
        self.ao_views.clear();
        self.gi_views.clear();

        self.gi_descriptor_set = vk::DescriptorSet::null();
        self.gi_layout = vk::DescriptorSetLayout::null();

        self.device = None;
    }
    /// Wire all external resources the ForwardPass needs:
    /// - IBL cubemap descriptor set (set 2)
    /// - shadow map view + comparison sampler (set 3)
    /// - bindless texture table set + layout (set 1)
    /// - material SSBO buffer + per-frame UBO buffers (set 0, one set per
    ///   frame-in-flight so each frame's UBO buffer is bound without runtime
    ///   descriptor rewrites)
    /// - light SSBO buffer (set 0 binding 2, hard-coded point lights)
    /// - GI probe volume descriptor set + layout (set 5, borrowed from SceneScope)
    ///
    /// `frame_ubo_buffers` length determines the frame-in-flight count (== set0
    /// set count). `materials_buffer` is the `RenderMaterialManager` SSBO.
    #[expect(
        clippy::too_many_arguments,
        reason = "arity mirrors the float-parameter packing that the callers already flatten"
    )]
    pub fn set_resources(
        &mut self,
        context: &crate::context::VulkanContext,
        ibl_descriptor_set: vk::DescriptorSet,
        ibl_layout: vk::DescriptorSetLayout,
        shadow_view: vk::ImageView,
        shadow_sampler: vk::Sampler,
        bindless_set: vk::DescriptorSet,
        bindless_layout: vk::DescriptorSetLayout,
        materials_buffer: vk::Buffer,
        frame_ubo_buffers: &[vk::Buffer],
        brdf_handle: u32,
        gi_descriptor_set: vk::DescriptorSet,
        gi_layout: vk::DescriptorSetLayout,
        camera_stride: u32,
        gi_dummy_view: vk::ImageView,
    ) -> Result<()> {
        let device = &context.device;
        self.ibl_descriptor_set = ibl_descriptor_set;
        self.ibl_layout = ibl_layout;
        self.bindless_set = bindless_set;
        self.bindless_layout = bindless_layout;
        self.brdf_handle = brdf_handle;
        self.camera_stride = camera_stride;
        self.gi_descriptor_set = gi_descriptor_set;
        self.gi_layout = gi_layout;
        self.skybox = SkyboxPass::new(ibl_descriptor_set, ibl_layout);

        let frame_set_bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(scene_frag::FRAME_BINDING)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(scene_frag::MATERIALS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(scene_frag::POINT_LIGHTS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(3)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::VERTEX),
            vk::DescriptorSetLayoutBinding::default()
                .binding(4)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::VERTEX),
        ];
        let frame_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&frame_set_bindings),
                None,
            )
        }
        .context("ForwardPass: create set0 (frame+materials+lights) layout")?;

        if let Some(old) = self.frame_set_layout.take() {
            unsafe { device.destroy_descriptor_set_layout(old, None) };
        }
        if let Some(old) = self.frame_set_pool.take() {
            unsafe { device.destroy_descriptor_pool(old, None) };
        }
        self.frame_sets.clear();

        let fif_count = frame_ubo_buffers.len();
        let frame_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(fif_count as u32)
                    .pool_sizes(&[
                        vk::DescriptorPoolSize {
                            ty: vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC,
                            descriptor_count: fif_count as u32,
                        },
                        vk::DescriptorPoolSize {
                            ty: vk::DescriptorType::STORAGE_BUFFER,
                            descriptor_count: (fif_count * 4) as u32,
                        },
                    ]),
                None,
            )
        }
        .context("ForwardPass: create set0 pool")?;

        let layout_ptrs: Vec<vk::DescriptorSetLayout> =
            (0..fif_count).map(|_| frame_layout).collect();
        let sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(frame_pool)
                    .set_layouts(&layout_ptrs),
            )
        }
        .context("ForwardPass: allocate set0 sets")?;

        let light_ssbo_size = (crate::descriptor::LIGHT_MAX as vk::DeviceSize)
            * crate::descriptor::GPU_LIGHT_SIZE as vk::DeviceSize;
        let mut new_light_buffers = Vec::with_capacity(fif_count);
        let mut new_light_memories = Vec::with_capacity(fif_count);
        for _ in 0..fif_count {
            let (buf, mem) = crate::buffer::create_buffer(
                context,
                light_ssbo_size,
                crate::buffer::BufferUsage::STORAGE_BUFFER,
                crate::buffer::MemoryProperties::HOST_VISIBLE
                    | crate::buffer::MemoryProperties::HOST_COHERENT,
            )
            .context("ForwardPass: create light SSBO buffer")?;

            let light_ptr =
                unsafe { device.map_memory(mem, 0, light_ssbo_size, vk::MemoryMapFlags::empty()) }
                    .context("ForwardPass: map light SSBO memory")?;
            unsafe {
                std::ptr::write_bytes(light_ptr as *mut u8, 0, light_ssbo_size as usize);
            }
            unsafe { device.unmap_memory(mem) };

            new_light_buffers.push(buf);
            new_light_memories.push(mem);
        }

        let bone_ssbo_size =
            (Self::BONE_CAPACITY as vk::DeviceSize) * (std::mem::size_of::<[[f32; 4]; 4]>() as u64);
        let mut new_bone_buffers = Vec::with_capacity(fif_count);
        let mut new_bone_memories = Vec::with_capacity(fif_count);
        for _ in 0..fif_count {
            let (buf, mem) = crate::buffer::create_buffer(
                context,
                bone_ssbo_size,
                crate::buffer::BufferUsage::STORAGE_BUFFER,
                crate::buffer::MemoryProperties::HOST_VISIBLE
                    | crate::buffer::MemoryProperties::HOST_COHERENT,
            )
            .context("ForwardPass: create bone SSBO buffer")?;
            let ptr =
                unsafe { device.map_memory(mem, 0, bone_ssbo_size, vk::MemoryMapFlags::empty()) }
                    .context("ForwardPass: map bone SSBO memory")?;
            unsafe { std::ptr::write_bytes(ptr as *mut u8, 0, bone_ssbo_size as usize) };
            unsafe { device.unmap_memory(mem) };
            new_bone_buffers.push(buf);
            new_bone_memories.push(mem);
        }

        let prev_model_ssbo_size = (Self::PREV_MODEL_CAPACITY as vk::DeviceSize)
            * (std::mem::size_of::<[[f32; 4]; 4]>() as u64);
        let mut new_prev_model_buffers = Vec::with_capacity(fif_count);
        let mut new_prev_model_memories = Vec::with_capacity(fif_count);
        for _ in 0..fif_count {
            let (buf, mem) = crate::buffer::create_buffer(
                context,
                prev_model_ssbo_size,
                crate::buffer::BufferUsage::STORAGE_BUFFER,
                crate::buffer::MemoryProperties::HOST_VISIBLE
                    | crate::buffer::MemoryProperties::HOST_COHERENT,
            )
            .context("ForwardPass: create prev-model SSBO buffer")?;
            let ptr = unsafe {
                device.map_memory(mem, 0, prev_model_ssbo_size, vk::MemoryMapFlags::empty())
            }
            .context("ForwardPass: map prev-model SSBO memory")?;
            unsafe { std::ptr::write_bytes(ptr as *mut u8, 0, prev_model_ssbo_size as usize) };
            unsafe { device.unmap_memory(mem) };
            new_prev_model_buffers.push(buf);
            new_prev_model_memories.push(mem);
        }

        for buf in self.light_buffers.drain(..) {
            if buf != vk::Buffer::null() {
                unsafe { device.destroy_buffer(buf, None) };
            }
        }
        for mem in self.light_memories.drain(..) {
            if mem != vk::DeviceMemory::null() {
                unsafe { device.free_memory(mem, None) };
            }
        }
        for buf in self.bone_buffers.drain(..) {
            if buf != vk::Buffer::null() {
                unsafe { device.destroy_buffer(buf, None) };
            }
        }
        for mem in self.bone_memories.drain(..) {
            if mem != vk::DeviceMemory::null() {
                unsafe { device.free_memory(mem, None) };
            }
        }
        for buf in self.prev_model_buffers.drain(..) {
            if buf != vk::Buffer::null() {
                unsafe { device.destroy_buffer(buf, None) };
            }
        }
        for mem in self.prev_model_memories.drain(..) {
            if mem != vk::DeviceMemory::null() {
                unsafe { device.free_memory(mem, None) };
            }
        }
        self.light_buffers = new_light_buffers;
        self.light_memories = new_light_memories;
        self.bone_buffers = new_bone_buffers;
        self.bone_memories = new_bone_memories;
        self.prev_model_buffers = new_prev_model_buffers;
        self.prev_model_memories = new_prev_model_memories;

        let ubo_size = std::mem::size_of::<crate::descriptor::FrameUBOData>() as vk::DeviceSize;
        let mat_ssbo_size = vk::WHOLE_SIZE;
        let mat_info = vk::DescriptorBufferInfo::default()
            .buffer(materials_buffer)
            .offset(0)
            .range(mat_ssbo_size);
        let light_infos: Vec<vk::DescriptorBufferInfo> = self
            .light_buffers
            .iter()
            .map(|buf| {
                vk::DescriptorBufferInfo::default()
                    .buffer(*buf)
                    .offset(0)
                    .range(vk::WHOLE_SIZE)
            })
            .collect();
        let ubo_infos: Vec<vk::DescriptorBufferInfo> = frame_ubo_buffers
            .iter()
            .map(|buf| {
                vk::DescriptorBufferInfo::default()
                    .buffer(*buf)
                    .offset(0)
                    .range(ubo_size)
            })
            .collect();
        let bone_infos: Vec<vk::DescriptorBufferInfo> = self
            .bone_buffers
            .iter()
            .map(|buf| {
                vk::DescriptorBufferInfo::default()
                    .buffer(*buf)
                    .offset(0)
                    .range(vk::WHOLE_SIZE)
            })
            .collect();
        let prev_model_infos: Vec<vk::DescriptorBufferInfo> = self
            .prev_model_buffers
            .iter()
            .map(|buf| {
                vk::DescriptorBufferInfo::default()
                    .buffer(*buf)
                    .offset(0)
                    .range(vk::WHOLE_SIZE)
            })
            .collect();
        let mut writes = Vec::with_capacity(fif_count * 5);
        for (i, set) in sets.iter().enumerate() {
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(scene_frag::FRAME_BINDING)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC)
                    .buffer_info(std::slice::from_ref(&ubo_infos[i])),
            );
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(scene_frag::MATERIALS_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&mat_info)),
            );
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(scene_frag::POINT_LIGHTS_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&light_infos[i])),
            );
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(3)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&bone_infos[i])),
            );
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(4)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&prev_model_infos[i])),
            );
        }
        unsafe { device.update_descriptor_sets(&writes, &[]) };

        self.frame_set_layout = Some(frame_layout);
        self.frame_set_pool = Some(frame_pool);
        self.frame_sets = sets;

        let shadow_bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(scene_frag::SHADOW_MAP_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(scene_frag::SHADOW_SAMPLER_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let shadow_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&shadow_bindings),
                None,
            )
        }
        .context("ForwardPass: create shadow ds layout")?;

        if let Some(old) = self.shadow_ds_layout.take() {
            unsafe { device.destroy_descriptor_set_layout(old, None) };
        }
        if let Some(old) = self.shadow_ds_pool.take() {
            unsafe { device.destroy_descriptor_pool(old, None) };
        }
        self.shadow_descriptor_set = vk::DescriptorSet::null();

        let pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&[
                        vk::DescriptorPoolSize {
                            ty: vk::DescriptorType::SAMPLED_IMAGE,
                            descriptor_count: 1,
                        },
                        vk::DescriptorPoolSize {
                            ty: vk::DescriptorType::SAMPLER,
                            descriptor_count: 1,
                        },
                    ]),
                None,
            )
        }
        .context("ForwardPass: create shadow ds pool")?;

        let ds = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&[shadow_layout]),
            )
        }
        .context("ForwardPass: allocate shadow ds")?[0];

        let image_info = vk::DescriptorImageInfo::default()
            .image_view(shadow_view)
            .image_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL);
        let sampler_info = vk::DescriptorImageInfo::default()
            .sampler(shadow_sampler)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);

        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(ds)
                .dst_binding(scene_frag::SHADOW_MAP_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(std::slice::from_ref(&image_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(ds)
                .dst_binding(scene_frag::SHADOW_SAMPLER_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .image_info(std::slice::from_ref(&sampler_info)),
        ];
        unsafe { device.update_descriptor_sets(&writes, &[]) };

        self.shadow_ds_layout = Some(shadow_layout);
        self.shadow_ds_pool = Some(pool);
        self.shadow_descriptor_set = ds;

        let ao_bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(scene_frag::AO_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(scene_frag::GI_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let ao_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&ao_bindings),
                None,
            )
        }
        .context("ForwardPass: create set4 (AO) ds layout")?;

        if let Some(old) = self.ao_ds_layout.take() {
            unsafe { device.destroy_descriptor_set_layout(old, None) };
        }
        if let Some(old) = self.ao_ds_pool.take() {
            unsafe { device.destroy_descriptor_pool(old, None) };
        }
        if self.ao_sampler != vk::Sampler::null() {
            unsafe { device.destroy_sampler(self.ao_sampler, None) };
        }
        self.ao_descriptor_sets.clear();
        self.ao_views.clear();

        let ao_sampler = unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .min_lod(0.0)
                    .max_lod(vk::LOD_CLAMP_NONE),
                None,
            )
        }
        .context("ForwardPass: create AO sampler")?;

        let ao_fif = frame_ubo_buffers.len() as u32;
        let ao_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(ao_fif)
                    .pool_sizes(&[vk::DescriptorPoolSize {
                        ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                        descriptor_count: ao_fif * 2,
                    }]),
                None,
            )
        }
        .context("ForwardPass: create set4 (AO) ds pool")?;

        let ao_layouts = vec![ao_layout; ao_fif as usize];
        let ao_sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(ao_pool)
                    .set_layouts(&ao_layouts),
            )
        }
        .context("ForwardPass: allocate set4 (AO) ds")?;
        let ao_sets: Vec<vk::DescriptorSet> = ao_sets;

        self.ao_ds_layout = Some(ao_layout);
        self.ao_ds_pool = Some(ao_pool);
        self.ao_sampler = ao_sampler;
        self.ao_descriptor_sets = ao_sets;
        self.ao_views = vec![vk::ImageView::null(); ao_fif as usize];
        self.gi_views = vec![vk::ImageView::null(); ao_fif as usize];
        if gi_dummy_view != vk::ImageView::null() {
            self.gi_views.fill(gi_dummy_view);
            for set in &self.ao_descriptor_sets {
                let image_info = vk::DescriptorImageInfo::default()
                    .image_view(gi_dummy_view)
                    .sampler(ao_sampler)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
                let write = vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(scene_frag::GI_TEX_BINDING)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(std::slice::from_ref(&image_info));
                unsafe { device.update_descriptor_sets(&[write], &[]) };
            }
        }

        Ok(())
    }

    /// Update the set 4 AO descriptor for `frame_index` to point at `view`
    /// (the previous frame's GTAO output). Called every frame from
    /// `GraphRenderer::render` BEFORE `forward_pass.execute`. Skips the
    /// descriptor write when `view` matches the currently-bound view for this
    /// frame-in-flight.
    pub fn set_ao(&mut self, device: &ash::Device, frame_index: u32, view: vk::ImageView) {
        let i = (frame_index as usize) % self.ao_descriptor_sets.len();
        if self.last_probe_log.elapsed().as_secs_f32() >= 1.0 {
            self.last_probe_log = Instant::now();
            log::trace!(
                "AO_PROBE set_ao: frame={} slot={} view={:?} prev_bound={:?} ao_views={:?} will_write={}",
                frame_index,
                i,
                view,
                self.ao_views[i],
                self.ao_views,
                view != self.ao_views[i] && view != vk::ImageView::null()
            );
        }
        if view == self.ao_views[i] {
            return;
        }
        self.ao_views[i] = view;
        if view == vk::ImageView::null() {
            return;
        }
        let image_info = vk::DescriptorImageInfo::default()
            .image_view(view)
            .sampler(self.ao_sampler)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.ao_descriptor_sets[i])
            .dst_binding(scene_frag::AO_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(std::slice::from_ref(&image_info));
        unsafe { device.update_descriptor_sets(&[write], &[]) };
    }

    /// Update the set 4 binding 1 descriptor for `frame_index` to point at
    /// `view`, the previous frame's ReSTIR GI signal.
    ///
    /// The RestirGi pass keeps its ping-pong images in
    /// `VK_IMAGE_LAYOUT_GENERAL`, so the descriptor declares that layout
    /// rather than SHADER_READ_ONLY_OPTIMAL. Passing a null view leaves the
    /// binding unbound, which is safe because the shader only samples it when
    /// the push constant asks for the screen-space path.
    pub fn set_gi_view(&mut self, device: &ash::Device, frame_index: u32, view: vk::ImageView) {
        if self.gi_views.is_empty() {
            return;
        }
        let i = (frame_index as usize) % self.gi_views.len();
        if view == self.gi_views[i] {
            return;
        }
        self.gi_views[i] = view;
        if view == vk::ImageView::null() {
            return;
        }
        let image_info = vk::DescriptorImageInfo::default()
            .image_view(view)
            .sampler(self.ao_sampler)
            .image_layout(vk::ImageLayout::GENERAL);
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.ao_descriptor_sets[i])
            .dst_binding(scene_frag::GI_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(std::slice::from_ref(&image_info));
        unsafe { device.update_descriptor_sets(&[write], &[]) };
    }

    /// Borrow the HDR color image view for `image_index`. Consumed by the
    /// PostPass as its sampled input.
    pub fn color_view(&self, image_index: u32) -> Option<vk::ImageView> {
        self.color_images
            .get(image_index as usize)
            .and_then(|c| c.as_ref())
            .map(|c| c.view)
    }

    /// Borrow the HDR color image handle for `image_index`. The PostPass needs
    /// the image to record its SHADER_READ_ONLY_OPTIMAL layout barrier.
    pub fn color_image(&self, image_index: u32) -> Option<vk::Image> {
        self.color_images
            .get(image_index as usize)
            .and_then(|c| c.as_ref())
            .map(|c| c.image)
    }

    /// Borrow the depth image view for `image_index` (the slot ForwardPass just
    /// rendered into). The GTAO pass samples it after ForwardPass stores depth
    pub fn depth_view(&self, image_index: u32) -> Option<vk::ImageView> {
        self.depth_images
            .get(image_index as usize)
            .and_then(|d| d.as_ref())
            .map(|d| d.view)
    }

    /// #1007: set this frame's depth readback buffer. Pass `None` to skip copy this frame.
    pub fn set_depth_readback(&mut self, buffer: Option<vk::Buffer>) {
        self.depth_readback = buffer;
        self.depth_capture_frame = None;
    }

    /// Most recent frame number that recorded a depth copy (`None` = no copy this frame).
    pub fn depth_capture_frame(&self) -> Option<u32> {
        self.depth_capture_frame
    }

    /// Borrow the depth image handle for `image_index`. The GTAO pass needs the
    /// image (not just the view) to record its DEPTH_STENCIL_READ_ONLY_OPTIMAL
    /// layout barrier before sampling.
    pub fn depth_image(&self, image_index: u32) -> Option<vk::Image> {
        self.depth_images
            .get(image_index as usize)
            .and_then(|d| d.as_ref())
            .map(|d| d.image)
    }

    /// Borrow the view-space normal MRT view for `image_index`. Consumed by
    /// the GTAO pass when its `mode == 0` (normal MRT path).
    pub fn normal_view(&self, image_index: u32) -> Option<vk::ImageView> {
        self.normal_images
            .get(image_index as usize)
            .and_then(|n| n.as_ref())
            .map(|n| n.view)
    }

    /// Borrow the view-space normal MRT image handle for `image_index`. The
    /// GTAO pass needs the image to record its SHADER_READ_ONLY_OPTIMAL layout
    /// barrier before sampling.
    pub fn normal_image(&self, image_index: u32) -> Option<vk::Image> {
        self.normal_images
            .get(image_index as usize)
            .and_then(|n| n.as_ref())
            .map(|n| n.image)
    }

    /// Borrow the motion-vector image view for `image_index` (#883 Phase 2).
    pub fn velocity_view(&self, image_index: u32) -> Option<vk::ImageView> {
        self.velocity_images
            .get(image_index as usize)
            .and_then(|v| v.as_ref())
            .map(|v| v.view)
    }

    /// Borrow the motion-vector image handle for `image_index` (#883 Phase 2).
    pub fn velocity_image(&self, image_index: u32) -> Option<vk::Image> {
        self.velocity_images
            .get(image_index as usize)
            .and_then(|v| v.as_ref())
            .map(|v| v.image)
    }

    /// Borrow the albedo + roughness MRT view for image_index. Consumed by the
    /// ReSTIR GI pass (demodulation + temporal validation).
    pub fn albedo_view(&self, image_index: u32) -> Option<vk::ImageView> {
        self.albedo_images
            .get(image_index as usize)
            .and_then(|a| a.as_ref())
            .map(|a| a.view)
    }

    /// Borrow the albedo + roughness image handle for image_index.
    pub fn albedo_image(&self, image_index: u32) -> Option<vk::Image> {
        self.albedo_images
            .get(image_index as usize)
            .and_then(|a| a.as_ref())
            .map(|a| a.image)
    }

    /// The full-resolution extent the scene was rendered at. The GTAO pass
    /// uses this (halved) to size its own viewport + AO textures.
    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }

    /// HDR intermediate color format (the scene target PostPass tonemaps).
    /// Exposed for the render-graph visualizer.
    pub fn color_format(&self) -> vk::Format {
        self.color_format
    }

    /// View-space normal MRT format (read by GTAO). Exposed for the viz.
    pub fn normal_format(&self) -> vk::Format {
        self.normal_format
    }

    /// Number of swapchain images (framebuffers / HDR color / depth / normal
    /// slots are all sized to this). Exposed for the viz.
    pub fn image_count(&self) -> usize {
        self.image_count
    }

    /// The three well-known output handles (`[color, normal, depth]`), in the
    /// same order `setup` declares them. Exposed for the viz's edge labels.
    pub fn out_handles(&self) -> [ResourceHandle; 3] {
        [self.out_color_h, self.out_normal_h, self.out_depth_h]
    }

    /// Clamp light count to `LIGHT_MAX` to avoid buffer overflow.
    #[inline]
    pub const fn clamped_light_count(raw_count: usize) -> usize {
        let max = crate::descriptor::LIGHT_MAX as usize;
        if raw_count > max {
            max
        } else {
            raw_count
        }
    }

    /// Rewrite the point-light SSBO for this frame from a fresh `&[GpuLight]` slice.
    /// Called every frame from `ForwardPass::execute` with the lights collected by
    /// `render_system` from the ECS world. Unused slots (between `lights.len()`
    /// and `LIGHT_MAX`) are zeroed so the shader doesn't read stale data.
    ///
    /// Copies at most `LIGHT_MAX` lights to avoid buffer overflow if more lights are present.
    /// Uses the SSBO dedicated to `frame_index` to prevent data races across frames in flight.
    ///
    /// The buffer is `HOST_VISIBLE | HOST_COHERENT`, so this is a plain map +
    /// copy + unmap. Safe to call before the SSBO is first bound (the descriptor
    /// points at the same buffer regardless of its contents).
    /// How many bone matrices one frame can address (#560 P0-3).
    ///
    /// 4096 mat4 = 256 KiB per frame in flight; the engine packs palettes of
    /// every skinned draw into this window and refuses to draw beyond it.
    pub const BONE_CAPACITY: usize = 4096;

    /// How many previous-model matrices one frame can address (#883 Phase 2).
    pub const PREV_MODEL_CAPACITY: usize = 4096;

    /// Upload this frame's bone palette (skinning matrices, column-major).
    ///
    /// `matrices` longer than [`Self::BONE_CAPACITY`] is truncated: the draw
    /// loop uses the same clamp when it assigns `bone_offset`.
    pub fn update_bones(
        &mut self,
        device: &ash::Device,
        frame_index: u32,
        matrices: &[[[f32; 4]; 4]],
    ) -> Result<()> {
        if self.bone_memories.is_empty() {
            return Ok(());
        }
        let index = (frame_index as usize) % self.bone_memories.len();
        let mem = self.bone_memories[index];
        if mem == vk::DeviceMemory::null() {
            return Ok(());
        }
        let count = matrices.len().min(Self::BONE_CAPACITY);
        let matrix_size = std::mem::size_of::<[[f32; 4]; 4]>();
        let total_bytes = (Self::BONE_CAPACITY * matrix_size) as vk::DeviceSize;
        let ptr = unsafe { device.map_memory(mem, 0, total_bytes, vk::MemoryMapFlags::empty()) }
            .context("ForwardPass::update_bones: map")?;
        unsafe {
            let dst = ptr as *mut u8;
            std::ptr::write_bytes(
                dst.add(count * matrix_size),
                0,
                (Self::BONE_CAPACITY - count) * matrix_size,
            );
            if count > 0 {
                std::ptr::copy_nonoverlapping(
                    bytemuck::cast_slice(&matrices[..count]).as_ptr(),
                    dst,
                    count * matrix_size,
                );
            }
            device.unmap_memory(mem);
        }
        Ok(())
    }

    /// Upload the previous-frame model matrix for each draw-list item
    /// (#883 Phase 2). The shader indexes this by the draw-list position
    /// pushed in `_padding.y`; a missing entry falls back to the current
    /// model on the CPU side (camera-only motion).
    pub fn update_prev_models(
        &mut self,
        device: &ash::Device,
        frame_index: u32,
        models: &[[[f32; 4]; 4]],
    ) -> Result<()> {
        if self.prev_model_memories.is_empty() {
            return Ok(());
        }
        let index = (frame_index as usize) % self.prev_model_memories.len();
        let mem = self.prev_model_memories[index];
        if mem == vk::DeviceMemory::null() {
            return Ok(());
        }
        let count = models.len().min(Self::PREV_MODEL_CAPACITY);
        let matrix_size = std::mem::size_of::<[[f32; 4]; 4]>();
        let total_bytes = (Self::PREV_MODEL_CAPACITY * matrix_size) as vk::DeviceSize;
        let ptr = unsafe { device.map_memory(mem, 0, total_bytes, vk::MemoryMapFlags::empty()) }
            .context("ForwardPass::update_prev_models: map")?;
        unsafe {
            let dst = ptr as *mut u8;
            std::ptr::write_bytes(
                dst.add(count * matrix_size),
                0,
                (Self::PREV_MODEL_CAPACITY - count) * matrix_size,
            );
            if count > 0 {
                std::ptr::copy_nonoverlapping(
                    bytemuck::cast_slice(&models[..count]).as_ptr(),
                    dst,
                    count * matrix_size,
                );
            }
            device.unmap_memory(mem);
        }
        Ok(())
    }

    pub fn update_lights(
        &mut self,
        device: &ash::Device,
        frame_index: u32,
        lights: &[crate::descriptor::GpuLight],
    ) -> Result<()> {
        if self.light_memories.is_empty() {
            return Ok(());
        }
        let mem = self.light_memories[(frame_index as usize) % self.light_memories.len()];
        if mem == vk::DeviceMemory::null() {
            return Ok(());
        }
        let total_bytes =
            (crate::descriptor::LIGHT_MAX as usize) * crate::descriptor::GPU_LIGHT_SIZE;
        let ptr = unsafe {
            device.map_memory(
                mem,
                0,
                total_bytes as vk::DeviceSize,
                vk::MemoryMapFlags::empty(),
            )
        }
        .context("ForwardPass::update_lights: map")?;
        let count = Self::clamped_light_count(lights.len());
        let copy_bytes = count * crate::descriptor::GPU_LIGHT_SIZE;
        unsafe {
            std::ptr::write_bytes(ptr as *mut u8, 0, total_bytes);
            if copy_bytes > 0 {
                std::ptr::copy_nonoverlapping(
                    bytemuck::cast_slice(&lights[..count]).as_ptr(),
                    ptr as *mut u8,
                    copy_bytes,
                );
            }
        }
        unsafe { device.unmap_memory(mem) };
        Ok(())
    }
    fn ensure_render_pass(&mut self, context: &crate::context::VulkanContext) -> Result<()> {
        if self.render_pass.is_some() {
            return Ok(());
        }
        let device = &context.device;
        self.device = Some(device.clone());

        let color_attachment = vk::AttachmentDescription::default()
            .format(self.color_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::DONT_CARE)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

        let color_ref = vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

        let depth_attachment = vk::AttachmentDescription::default()
            .format(vk::Format::D32_SFLOAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::DONT_CARE)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);

        let depth_ref = vk::AttachmentReference::default()
            .attachment(1)
            .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);

        let normal_attachment = vk::AttachmentDescription::default()
            .format(self.normal_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::DONT_CARE)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

        let normal_ref = vk::AttachmentReference::default()
            .attachment(2)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

        let velocity_attachment = vk::AttachmentDescription::default()
            .format(self.velocity_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

        let velocity_ref = vk::AttachmentReference::default()
            .attachment(3)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

        let albedo_attachment = vk::AttachmentDescription::default()
            .format(self.albedo_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::DONT_CARE)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

        let albedo_ref = vk::AttachmentReference::default()
            .attachment(4)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

        let color_refs = [color_ref, normal_ref, velocity_ref, albedo_ref];
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_refs)
            .depth_stencil_attachment(&depth_ref);

        let dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .dst_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            );

        let attachments = [
            color_attachment,
            depth_attachment,
            normal_attachment,
            velocity_attachment,
            albedo_attachment,
        ];
        let rp_create_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(std::slice::from_ref(&subpass))
            .dependencies(std::slice::from_ref(&dependency));

        let rp = unsafe { device.create_render_pass(&rp_create_info, None) }
            .context("ForwardPass: create render pass")?;
        self.render_pass = Some(rp);

        if self.gizmo.is_none() {
            self.gizmo = Some(Gizmo::new(context, rp).context("ForwardPass: create gizmo")?);
        }
        if self.ground_grid.is_none() {
            self.ground_grid =
                Some(GroundGrid::new(context, rp).context("ForwardPass: create ground grid")?);
        }
        Ok(())
    }
    /// Build one pipeline for each [`DrawState`] appearing in draw list (idempotent).
    ///
    /// cull / depth write / blend are all Vulkan **pipeline state**, so split pipelines by state; all
    /// variants' descriptor layouts and push constants fully identical. Caller collects all states needed this frame
    /// before recording commands (see `execute`), avoid creating pipelines mid command buffer recording.
    fn ensure_pipelines(
        &mut self,
        device: &ash::Device,
        states: &[(Option<crate::ShaderHandle>, DrawState)],
        wireframe: bool,
    ) -> Result<()> {
        let missing: Vec<(Option<crate::ShaderHandle>, DrawState)> = states
            .iter()
            .copied()
            .filter(|(shader, state)| {
                !self
                    .pipelines
                    .contains_key(&(*shader, *state, wireframe, false))
                    || !self
                        .pipelines
                        .contains_key(&(*shader, *state, wireframe, true))
            })
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        let rp = self
            .render_pass
            .context("ForwardPass: render_pass not created before pipeline")?;

        const VERT_SPV: &[u8] = prism_render_shaders::MESH_VERT_VERT_SPV;
        const FRAG_SPV: &[u8] = prism_render_shaders::SCENE_FRAG_FRAG_SPV;
        let vert_module =
            shader::load_shader_module(device, VERT_SPV).context("ForwardPass: load vert")?;

        let vert_entry = std::ffi::CString::new(mesh_vert::ENTRY_VERTEX_MAIN).unwrap();
        let frag_entry = std::ffi::CString::new(scene_frag::ENTRY_FRAGMENT_MAIN).unwrap();

        const VERT_SKINNED_SPV: &[u8] = prism_render_shaders::MESH_VERT_SKINNED_VERT_SPV;
        let vert_skinned_module = shader::load_shader_module(device, VERT_SKINNED_SPV)
            .context("ForwardPass: load skinned vert")?;
        let skin_entry = std::ffi::CString::new("vertexSkinnedMain").unwrap();

        let mut frag_modules: std::collections::HashMap<
            Option<crate::ShaderHandle>,
            vk::ShaderModule,
        > = std::collections::HashMap::new();
        for &(shader_handle, _) in &missing {
            if frag_modules.contains_key(&shader_handle) {
                continue;
            }
            let bytes: &[u8] = match shader_handle {
                None => FRAG_SPV,
                Some(h) => self
                    .custom_shaders
                    .get(h.0 as usize)
                    .map(Vec::as_slice)
                    .with_context(|| format!("ForwardPass: unknown custom shader {:?}", h))?,
            };
            let module = shader::load_shader_module(device, bytes)
                .with_context(|| format!("ForwardPass: load frag ({shader_handle:?})"))?;
            frag_modules.insert(shader_handle, module);
        }

        let binding_desc = Vertex::binding_description();
        let attr_descs = Vertex::attribute_descriptions();
        let skin_binding_desc = crate::mesh::SkinVertex::binding_description();
        let skin_attr_descs = crate::mesh::SkinVertex::attribute_descriptions();

        let set0_layout = self
            .frame_set_layout
            .context("ForwardPass: set0 (frame+materials) layout not set (call set_resources)")?;
        let set1_layout = self.bindless_layout;
        let set2_layout = self.ibl_layout;
        let set3_layout = self
            .shadow_ds_layout
            .context("ForwardPass: shadow ds layout not set")?;
        let set4_layout = self
            .ao_ds_layout
            .context("ForwardPass: set4 (AO) layout not set (call set_resources)")?;
        let set5_layout = self.gi_layout;

        let set_layouts = [
            set0_layout,
            set1_layout,
            set2_layout,
            set3_layout,
            set4_layout,
            set5_layout,
        ];

        let push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(scene_frag::PUSH_CONSTANT_SIZE)];

        for (shader_handle, state) in missing {
            let frag_module = frag_modules[&shader_handle];
            let blend_attachments = color_blend_attachments(state);
            for skinned in [false, true] {
                let (variant_stages, variant_bindings) = if skinned {
                    let stages = [
                        shader::shader_stage(
                            vk::ShaderStageFlags::VERTEX,
                            vert_skinned_module,
                            skin_entry.as_c_str(),
                        ),
                        shader::shader_stage(
                            vk::ShaderStageFlags::FRAGMENT,
                            frag_module,
                            frag_entry.as_c_str(),
                        ),
                    ];
                    (stages, [binding_desc, skin_binding_desc])
                } else {
                    let stages = [
                        shader::shader_stage(
                            vk::ShaderStageFlags::VERTEX,
                            vert_module,
                            vert_entry.as_c_str(),
                        ),
                        shader::shader_stage(
                            vk::ShaderStageFlags::FRAGMENT,
                            frag_module,
                            frag_entry.as_c_str(),
                        ),
                    ];
                    (stages, [binding_desc, skin_binding_desc])
                };
                let binding_descs: &[vk::VertexInputBindingDescription] = if skinned {
                    &variant_bindings
                } else {
                    std::slice::from_ref(&variant_bindings[0])
                };
                let attr_descriptor_list: &[vk::VertexInputAttributeDescription] = if skinned {
                    &[
                        attr_descs[0],
                        attr_descs[1],
                        attr_descs[2],
                        attr_descs[3],
                        attr_descs[4],
                        skin_attr_descs[0],
                        skin_attr_descs[1],
                    ]
                } else {
                    &attr_descs
                };
                let pipeline = GraphicsPipeline::new(&PipelineDesc {
                device,
                shader_stages: &variant_stages,
                vertex_binding_desc: binding_descs,
                vertex_attr_descs: attr_descriptor_list,
                descriptor_set_layouts: &set_layouts,
                push_constant_ranges: &push,
                render_pass: rp,
                subpass: 0,
                cull_mode: Some(cull_flags(state.cull_mode)),
                depth_bias_enable: None,
                depth_bias_constant_factor: None,
                depth_bias_slope_factor: None,
                depth_write_enable: Some(state.z_write),
                depth_test_enable: None,
                color_attachment_count: None,
                color_blend_attachments: Some(&blend_attachments),
                front_face: None,
                polygon_mode: Some(if wireframe {
                    vk::PolygonMode::LINE
                } else {
                    vk::PolygonMode::FILL
                }),
            })
            .with_context(|| {
                format!(
                    "ForwardPass: create pipeline ({shader_handle:?}, {state:?}, wireframe={wireframe}, skinned={skinned})"
                )
            })?;
                self.pipelines
                    .insert((shader_handle, state, wireframe, skinned), pipeline);
            }
        }

        unsafe { device.destroy_shader_module(vert_module, None) };
        unsafe { device.destroy_shader_module(vert_skinned_module, None) };
        for (_, frag_module) in frag_modules {
            unsafe { device.destroy_shader_module(frag_module, None) };
        }

        Ok(())
    }

    /// Lazily create selected-outline pipeline (#487).
    ///
    /// Reverse shell only binds set 0 (frame UBO), so pipeline layout narrower than scene pipeline; culls front face,
    /// disables depth write, so expanded backfaces only pass depth test outside original mesh silhouette. Attachment 1 (view
    /// normal) not written, avoid polluting GTAO input.
    fn ensure_outline_pipeline(&mut self, device: &ash::Device) -> Result<()> {
        if self.outline_pipeline.is_some() {
            return Ok(());
        }
        let rp = self
            .render_pass
            .context("ForwardPass: render_pass not created before outline pipeline")?;
        let frame_set_layout = self
            .frame_set_layout
            .context("ForwardPass: set0 layout not set (call set_resources)")?;

        const VERT_SPV: &[u8] = prism_render_shaders::OUTLINE_VERT_SPV;
        const FRAG_SPV: &[u8] = prism_render_shaders::OUTLINE_FRAG_SPV;
        let vert_module =
            shader::load_shader_module(device, VERT_SPV).context("outline: load vert")?;
        let frag_module =
            shader::load_shader_module(device, FRAG_SPV).context("outline: load frag")?;
        let vert_entry = std::ffi::CString::new("vertexMain").unwrap();
        let frag_entry = std::ffi::CString::new("fragmentMain").unwrap();
        let shader_stages = [
            shader::shader_stage(
                vk::ShaderStageFlags::VERTEX,
                vert_module,
                vert_entry.as_c_str(),
            ),
            shader::shader_stage(
                vk::ShaderStageFlags::FRAGMENT,
                frag_module,
                frag_entry.as_c_str(),
            ),
        ];
        let binding_desc = Vertex::binding_description();
        let attr_descs = Vertex::attribute_descriptions();
        let set_layouts = [frame_set_layout];
        let push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(std::mem::size_of::<OutlinePush>() as u32)];
        let blend_attachments = [
            vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::RGBA)
                .blend_enable(false),
            vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::empty())
                .blend_enable(false),
            vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::empty())
                .blend_enable(false),
        ];

        let pipeline = GraphicsPipeline::new(&PipelineDesc {
            device,
            shader_stages: &shader_stages,
            vertex_binding_desc: std::slice::from_ref(&binding_desc),
            vertex_attr_descs: &attr_descs,
            descriptor_set_layouts: &set_layouts,
            push_constant_ranges: &push,
            render_pass: rp,
            subpass: 0,
            cull_mode: Some(vk::CullModeFlags::FRONT),
            depth_bias_enable: None,
            depth_bias_constant_factor: None,
            depth_bias_slope_factor: None,
            depth_write_enable: Some(false),
            depth_test_enable: None,
            color_attachment_count: None,
            color_blend_attachments: Some(&blend_attachments),
            front_face: None,
            polygon_mode: Some(vk::PolygonMode::FILL),
        })
        .context("ForwardPass: create outline pipeline")?;

        unsafe { device.destroy_shader_module(vert_module, None) };
        unsafe { device.destroy_shader_module(frag_module, None) };
        self.outline_pipeline = Some(pipeline);
        Ok(())
    }

    /// Lazily create "fill + wireframe overlay" pipeline (#431).
    ///
    /// Reuses outline shader: vertex only applies `model` transform (thickness 0, no expansion), fragment outputs
    /// dark lines. `POLYGON_MODE_LINE` + negative depth bias pushes lines onto filled surface, avoiding
    /// z-fighting; no depth write, so lines won't pollute subsequent depth test / selected outline.
    fn ensure_shaded_wireframe_pipeline(&mut self, device: &ash::Device) -> Result<()> {
        if self.shaded_wireframe_pipeline.is_some() {
            return Ok(());
        }
        let rp = self
            .render_pass
            .context("ForwardPass: render_pass not created before shaded wireframe")?;
        const VERT_SPV: &[u8] = prism_render_shaders::OUTLINE_VERT_SPV;
        const FRAG_SPV: &[u8] = prism_render_shaders::OUTLINE_FRAG_SPV;
        let vert_module =
            shader::load_shader_module(device, VERT_SPV).context("wireframe overlay: load vert")?;
        let frag_module =
            shader::load_shader_module(device, FRAG_SPV).context("wireframe overlay: load frag")?;

        let vert_entry = std::ffi::CString::new("vertexMain").unwrap();
        let frag_entry = std::ffi::CString::new("fragmentMain").unwrap();
        let vert_stage = shader::shader_stage(
            vk::ShaderStageFlags::VERTEX,
            vert_module,
            vert_entry.as_c_str(),
        );
        let frag_stage = shader::shader_stage(
            vk::ShaderStageFlags::FRAGMENT,
            frag_module,
            frag_entry.as_c_str(),
        );
        let shader_stages = [vert_stage, frag_stage];
        let binding_desc = Vertex::binding_description();
        let attr_descs = Vertex::attribute_descriptions();
        let frame_set_layout = self
            .frame_set_layout
            .context("ForwardPass: set0 layout not set (call set_resources)")?;
        let set_layouts = [frame_set_layout];
        let push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(std::mem::size_of::<OutlinePush>() as u32)];
        let blend_attachments = [
            vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::RGBA)
                .blend_enable(false),
            vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::empty())
                .blend_enable(false),
            vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::empty())
                .blend_enable(false),
        ];
        let pipeline = GraphicsPipeline::new(&PipelineDesc {
            device,
            shader_stages: &shader_stages,
            vertex_binding_desc: std::slice::from_ref(&binding_desc),
            vertex_attr_descs: &attr_descs,
            descriptor_set_layouts: &set_layouts,
            push_constant_ranges: &push,
            render_pass: rp,
            subpass: 0,
            cull_mode: Some(vk::CullModeFlags::BACK),
            depth_bias_enable: Some(true),
            depth_bias_constant_factor: Some(-2.0),
            depth_bias_slope_factor: Some(-1.0),
            depth_write_enable: Some(false),
            depth_test_enable: None,
            color_attachment_count: None,
            color_blend_attachments: Some(&blend_attachments),
            front_face: None,
            polygon_mode: Some(vk::PolygonMode::LINE),
        })
        .context("ForwardPass: create shaded wireframe pipeline")?;

        unsafe { device.destroy_shader_module(vert_module, None) };
        unsafe { device.destroy_shader_module(frag_module, None) };
        self.shaded_wireframe_pipeline = Some(pipeline);
        Ok(())
    }
}
impl RenderPassNode for ForwardPass {
    fn name(&self) -> &str {
        "ForwardPass"
    }

    fn setup(&mut self, graph: &mut RenderGraphBuilder, _settings: &RenderSettings) {
        graph.create_resource_at(
            FORWARD_DEPTH_H,
            ResourceType::DepthAttachment {
                extent: vk::Extent2D {
                    width: 1,
                    height: 1,
                },
                sample_count: vk::SampleCountFlags::TYPE_1,
            },
        );
        graph.create_resource_at(
            FORWARD_NORMAL_H,
            ResourceType::ColorAttachment {
                format: self.normal_format,
                extent: vk::Extent2D {
                    width: 1,
                    height: 1,
                },
                sample_count: vk::SampleCountFlags::TYPE_1,
            },
        );
        graph.create_resource_at(
            FORWARD_COLOR_H,
            ResourceType::ColorAttachment {
                format: self.color_format,
                extent: vk::Extent2D {
                    width: 1,
                    height: 1,
                },
                sample_count: vk::SampleCountFlags::TYPE_1,
            },
        );
        graph.create_resource_at(
            FORWARD_VELOCITY_H,
            ResourceType::ColorAttachment {
                format: self.velocity_format,
                extent: vk::Extent2D {
                    width: 1,
                    height: 1,
                },
                sample_count: vk::SampleCountFlags::TYPE_1,
            },
        );
        graph.create_resource_at(
            FORWARD_ALBEDO_H,
            ResourceType::ColorAttachment {
                format: self.albedo_format,
                extent: vk::Extent2D {
                    width: 1,
                    height: 1,
                },
                sample_count: vk::SampleCountFlags::TYPE_1,
            },
        );
        self.out_depth_h = FORWARD_DEPTH_H;
        self.out_normal_h = FORWARD_NORMAL_H;
        self.out_color_h = FORWARD_COLOR_H;
        self.out_velocity_h = FORWARD_VELOCITY_H;
        self.out_albedo_h = FORWARD_ALBEDO_H;

        #[cfg(feature = "shadows")]
        graph.read_usage(ResourceUsage {
            handle: SHADOW_MAP_H,
            access: vk::AccessFlags::SHADER_READ,
            stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
            layout: vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
        });

        graph.write_usage(ResourceUsage {
            handle: FORWARD_DEPTH_H,
            access: vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            stage: vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
            layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
        });
        graph.write_usage(ResourceUsage {
            handle: FORWARD_NORMAL_H,
            access: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            stage: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        });
        graph.write_usage(ResourceUsage {
            handle: FORWARD_COLOR_H,
            access: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            stage: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        });
        graph.write_usage(ResourceUsage {
            handle: FORWARD_VELOCITY_H,
            access: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            stage: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        });
        graph.write_usage(ResourceUsage {
            handle: FORWARD_ALBEDO_H,
            access: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            stage: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        });
    }

    fn execute(&mut self, ctx: &RenderContext, resources: &mut GraphResources) -> Result<()> {
        self.ensure_target(ctx.device, ctx.context, ctx.image_index, ctx.render_extent)?;
        self.set_ao(ctx.device, ctx.frame_index, ctx.frame.ao_view);
        self.set_gi_view(ctx.device, ctx.frame_index, ctx.frame.gi_view);
        self.update_lights(ctx.device, ctx.frame_index, ctx.frame.lights)?;

        self.ensure_render_pass(ctx.context)?;
        let wireframe = ctx.frame.wireframe && ctx.context.wireframe_supported;
        let shaded_wireframe =
            !wireframe && ctx.frame.shaded_wireframe && ctx.context.wireframe_supported;
        let mut states: Vec<(Option<crate::ShaderHandle>, DrawState)> =
            vec![(None, DrawState::default())];
        let mut seen = std::collections::HashSet::new();
        seen.insert((None, DrawState::default()));
        for item in ctx.frame.draw_list {
            let key = (item.shader, item.state);
            if item.visible_in_main_pass && seen.insert(key) {
                states.push(key);
            }
        }
        self.ensure_pipelines(ctx.device, &states, wireframe)?;
        if shaded_wireframe {
            self.ensure_shaded_wireframe_pipeline(ctx.device)?;
        }

        let rp = self.render_pass.unwrap();
        let idx = ctx.image_index as usize;
        let fb = self
            .framebuffers
            .get(idx)
            .copied()
            .flatten()
            .context("ForwardPass: no framebuffer for image_index (call set_target first)")?;
        let (scene_pipeline, scene_layout) = {
            let pipeline = self
                .pipelines
                .get(&(None, DrawState::default(), wireframe, false))
                .context("ForwardPass: default pipeline missing")?;
            (pipeline.pipeline, pipeline.layout)
        };

        let frame_set = self
            .frame_sets
            .get(ctx.frame_index as usize)
            .copied()
            .context("ForwardPass: no set0 descriptor set for frame_index (call set_resources)")?;

        let cc = ctx.frame.clear_color;
        let clear_values = [
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [cc[0], cc[1], cc[2], cc[3]],
                },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [0.0, 0.0, 0.0, 0.0],
                },
            },
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [0.0, 0.0, 0.0, 0.0],
                },
            },
        ];

        let begin_info = vk::RenderPassBeginInfo::default()
            .render_pass(rp)
            .framebuffer(fb)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.extent,
            })
            .clear_values(&clear_values);

        unsafe {
            ctx.device
                .cmd_begin_render_pass(ctx.cmd, &begin_info, vk::SubpassContents::INLINE)
        };

        let cameras: Vec<crate::render_graph::CameraView> = if ctx.frame.cameras.is_empty() {
            vec![crate::render_graph::CameraView::legacy(
                ctx.frame.view_proj,
                ctx.frame.inv_view_rot,
                ctx.frame.clear_color,
                ctx.frame.has_camera,
            )]
        } else {
            ctx.frame
                .cameras
                .iter()
                .take(crate::descriptor::MAX_CAMERAS)
                .copied()
                .collect()
        };

        let mut drawn = 0usize;
        for (camera_index, camera) in cameras.iter().enumerate() {
            let mut clears: Vec<vk::ClearAttachment> = Vec::new();
            if camera.clear_color_enabled {
                clears.push(vk::ClearAttachment {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    color_attachment: 0,
                    clear_value: vk::ClearValue {
                        color: vk::ClearColorValue {
                            float32: camera.clear_color,
                        },
                    },
                });
                clears.push(vk::ClearAttachment {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    color_attachment: 2,
                    clear_value: vk::ClearValue {
                        color: vk::ClearColorValue {
                            float32: [0.0, 0.0, 0.0, 0.0],
                        },
                    },
                });
            }
            if camera.clear_depth {
                clears.push(vk::ClearAttachment {
                    aspect_mask: vk::ImageAspectFlags::DEPTH,
                    color_attachment: 1,
                    clear_value: vk::ClearValue {
                        depth_stencil: vk::ClearDepthStencilValue {
                            depth: 1.0,
                            stencil: 0,
                        },
                    },
                });
            }
            if !clears.is_empty() {
                let rect = vk::ClearRect {
                    rect: vk::Rect2D {
                        offset: vk::Offset2D { x: 0, y: 0 },
                        extent: self.extent,
                    },
                    base_array_layer: 0,
                    layer_count: 1,
                };
                unsafe {
                    ctx.device
                        .cmd_clear_attachments(ctx.cmd, &clears, std::slice::from_ref(&rect));
                }
            }

            unsafe {
                ctx.device.cmd_set_viewport(
                    ctx.cmd,
                    0,
                    &[vk::Viewport {
                        x: 0.0,
                        y: 0.0,
                        width: self.extent.width as f32,
                        height: self.extent.height as f32,
                        min_depth: 0.0,
                        max_depth: 1.0,
                    }],
                );
                ctx.device.cmd_set_scissor(
                    ctx.cmd,
                    0,
                    &[vk::Rect2D {
                        offset: vk::Offset2D { x: 0, y: 0 },
                        extent: self.extent,
                    }],
                );
            }

            if camera.draw_skybox && camera.has_camera {
                let mut inv_view_proj_rot = [[0.0_f32; 4]; 4];
                for (c, column) in inv_view_proj_rot.iter_mut().enumerate() {
                    for (r, value) in column.iter_mut().enumerate() {
                        *value = (0..4)
                            .map(|k| ctx.frame.inv_view_rot[k][r] * ctx.frame.inv_projection[c][k])
                            .sum();
                    }
                }
                if let Some(skybox) = ctx.frame.skybox {
                    let rp = self
                        .render_pass
                        .ok_or_else(|| anyhow::anyhow!("forward render pass missing"))?;
                    if let Err(e) = self.skybox.draw(
                        ctx.device,
                        ctx.cmd,
                        rp,
                        self.extent,
                        &inv_view_proj_rot,
                        skybox,
                    ) {
                        log::warn!("SkyboxPass draw failed (skybox skipped): {e:#}");
                    }
                }
            }

            let dynamic_offset = camera_index as u32 * self.camera_stride;
            unsafe {
                ctx.device.cmd_bind_pipeline(
                    ctx.cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    scene_pipeline,
                );
                ctx.device.cmd_bind_descriptor_sets(
                    ctx.cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    scene_layout,
                    0,
                    std::slice::from_ref(&frame_set),
                    std::slice::from_ref(&dynamic_offset),
                );
                ctx.device.cmd_bind_descriptor_sets(
                    ctx.cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    scene_layout,
                    1,
                    std::slice::from_ref(&self.bindless_set),
                    &[],
                );
                ctx.device.cmd_bind_descriptor_sets(
                    ctx.cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    scene_layout,
                    2,
                    std::slice::from_ref(&self.ibl_descriptor_set),
                    &[],
                );
                ctx.device.cmd_bind_descriptor_sets(
                    ctx.cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    scene_layout,
                    3,
                    std::slice::from_ref(&self.shadow_descriptor_set),
                    &[],
                );
                let ao_set = self
                    .ao_descriptor_sets
                    .get(ctx.frame_index as usize)
                    .copied()
                    .unwrap_or(vk::DescriptorSet::null());
                ctx.device.cmd_bind_descriptor_sets(
                    ctx.cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    scene_layout,
                    4,
                    std::slice::from_ref(&ao_set),
                    &[],
                );
                let set5 = if ctx.frame.render_mode == crate::render_graph::RenderMode::Raster
                    && ctx.settings.raster_gi == crate::render_graph::RasterGiMode::Prt
                    && ctx.frame.prt_gi_descriptor_set != vk::DescriptorSet::null()
                {
                    ctx.frame.prt_gi_descriptor_set
                } else {
                    self.gi_descriptor_set
                };
                ctx.device.cmd_bind_descriptor_sets(
                    ctx.cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    scene_layout,
                    5,
                    std::slice::from_ref(&set5),
                    &[],
                );
            }

            unsafe {
                if ctx.frame.render_mode != crate::RenderMode::Sprites2D {
                    let bone_count = ctx
                        .frame
                        .bone_matrices
                        .len()
                        .min(ForwardPass::BONE_CAPACITY);
                    let capacity = ForwardPass::BONE_CAPACITY.saturating_sub(bone_count);
                    let (batches, instance_matrices) = prepare_static_batches(
                        ctx.frame.draw_list,
                        ctx.frame.mesh_manager,
                        capacity,
                    );
                    if !instance_matrices.is_empty() {
                        let mut packed = Vec::with_capacity(bone_count + instance_matrices.len());
                        packed.extend_from_slice(&ctx.frame.bone_matrices[..bone_count]);
                        packed.extend_from_slice(&instance_matrices);
                        self.update_bones(ctx.device, ctx.frame_index, &packed)?;
                    }

                    let mut bound_state = (None, DrawState::default(), wireframe, false);
                    for batch in &batches {
                        let item = &ctx.frame.draw_list[batch.items[0]];
                        let uploaded_mesh = match ctx.frame.mesh_manager.get(item.mesh) {
                            Some(m) => m,
                            None => continue,
                        };
                        let uploaded = &uploaded_mesh.mesh;
                        let skinned = item.bone_offset.is_some();
                        let skin = if skinned {
                            uploaded_mesh.skin.as_ref()
                        } else {
                            None
                        };
                        if skinned && skin.is_none() {
                            continue;
                        }
                        let key = (item.shader, item.state, wireframe, skinned);
                        if key != bound_state {
                            let variant = self.pipelines.get(&key).with_context(|| {
                                format!(
                                    "ForwardPass: no pipeline for ({:?}, {:?})",
                                    item.shader, item.state
                                )
                            })?;
                            ctx.device.cmd_bind_pipeline(
                                ctx.cmd,
                                vk::PipelineBindPoint::GRAPHICS,
                                variant.pipeline,
                            );
                            bound_state = key;
                        }
                        drawn += 1;

                        if let Some(skin) = skin {
                            let vertex_buffers = [uploaded.vertex_buffer, skin.buffer];
                            let offsets = [0u64, 0u64];
                            ctx.device.cmd_bind_vertex_buffers(
                                ctx.cmd,
                                0,
                                &vertex_buffers,
                                &offsets,
                            );
                        } else {
                            let vertex_buffers = [uploaded.vertex_buffer];
                            let offsets = [0u64];
                            ctx.device.cmd_bind_vertex_buffers(
                                ctx.cmd,
                                0,
                                &vertex_buffers,
                                &offsets,
                            );
                        }

                        let instance_count = batch
                            .instance_offset
                            .map(|_| batch.items.len() as u32)
                            .unwrap_or(0);
                        let instance_offset = batch
                            .instance_offset
                            .map(|offset| bone_count as u32 + offset)
                            .unwrap_or(0);
                        let pc = crate::shader_bindings::scene_frag::PbrBindlessPush {
                            model: item.model,
                            material_slot: item.material.unwrap_or(0),
                            env_handle: self.brdf_handle,
                            debug_flags: ctx.frame.debug_flags,
                            bone_offset: item.bone_offset.unwrap_or(0),
                            skin_mode: u32::from(item.bone_offset.is_some()),
                            instance_offset,
                            instance_count,
                            prev_draw_index: batch.items[0] as u32,
                            reactive_mask: u32::from(item.state.blends()),
                            splat_slot0: item.splat_slots[0],
                            splat_slot1: item.splat_slots[1],
                            splat_slot2: item.splat_slots[2],
                            splat_slot3: item.splat_slots[3],
                            splat_tiling: item.splat_tiling,
                            gi_mode: if ctx.frame.render_mode
                                == crate::render_graph::RenderMode::Raster
                                && ctx.settings.raster_gi == crate::render_graph::RasterGiMode::Prt
                                && ctx.frame.prt_gi_descriptor_set != vk::DescriptorSet::null()
                            {
                                2
                            } else if ctx.frame.render_mode
                                == crate::render_graph::RenderMode::Raster
                                && ctx.settings.raster_gi
                                    == crate::render_graph::RasterGiMode::Restir
                                && ctx.frame.gi_view != vk::ImageView::null()
                            {
                                1
                            } else {
                                0
                            },
                            prt_leak_bias: ctx
                                .settings
                                .prt_leak_bias
                                .clamp(0.0, crate::prt_runtime::MAX_LEAK_BIAS),
                        };
                        ctx.device.cmd_push_constants(
                            ctx.cmd,
                            scene_layout,
                            vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                            0,
                            bytemuck::bytes_of(&pc),
                        );

                        let draw_instances = instance_count.max(1);
                        if let Some(ib) = uploaded.index_buffer {
                            let index_count = item.index_count.unwrap_or(uploaded.index_count);
                            if index_count == 0 {
                                continue;
                            }
                            ctx.device
                                .cmd_bind_index_buffer(ctx.cmd, ib, 0, vk::IndexType::UINT32);
                            ctx.device.cmd_draw_indexed(
                                ctx.cmd,
                                index_count,
                                draw_instances,
                                item.index_start,
                                0,
                                0,
                            );
                        } else {
                            ctx.device.cmd_draw(
                                ctx.cmd,
                                uploaded.vertex_count,
                                draw_instances,
                                0,
                                0,
                            );
                        }
                    }
                }

                if shaded_wireframe {
                    let overlay = self
                        .shaded_wireframe_pipeline
                        .as_ref()
                        .context("ForwardPass: shaded wireframe pipeline missing")?;
                    ctx.device.cmd_bind_pipeline(
                        ctx.cmd,
                        vk::PipelineBindPoint::GRAPHICS,
                        overlay.pipeline,
                    );
                    ctx.device.cmd_bind_descriptor_sets(
                        ctx.cmd,
                        vk::PipelineBindPoint::GRAPHICS,
                        overlay.layout,
                        0,
                        std::slice::from_ref(&frame_set),
                        std::slice::from_ref(&dynamic_offset),
                    );
                    for item in ctx.frame.draw_list {
                        if !item.visible_in_main_pass {
                            continue;
                        }
                        let Some(uploaded) = ctx.frame.mesh_manager.get(item.mesh) else {
                            continue;
                        };
                        let pc = OutlinePush {
                            model: item.model,
                            color: [0.02, 0.02, 0.02, 1.0],
                            thickness: 0.0,
                            _pad: [0.0; 3],
                        };
                        let vertex_buffers = [uploaded.mesh.vertex_buffer];
                        let offsets = [0u64];
                        ctx.device
                            .cmd_bind_vertex_buffers(ctx.cmd, 0, &vertex_buffers, &offsets);
                        ctx.device.cmd_push_constants(
                            ctx.cmd,
                            overlay.layout,
                            vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                            0,
                            bytemuck::bytes_of(&pc),
                        );
                        if let Some(ib) = uploaded.mesh.index_buffer {
                            let index_count = item.index_count.unwrap_or(uploaded.mesh.index_count);
                            if index_count == 0 {
                                continue;
                            }
                            ctx.device
                                .cmd_bind_index_buffer(ctx.cmd, ib, 0, vk::IndexType::UINT32);
                            ctx.device.cmd_draw_indexed(
                                ctx.cmd,
                                index_count,
                                1,
                                item.index_start,
                                0,
                                0,
                            );
                        } else {
                            ctx.device
                                .cmd_draw(ctx.cmd, uploaded.mesh.vertex_count, 1, 0, 0);
                        }
                    }
                }

                let outline_requested = ctx.frame.draw_list.iter().any(|item| {
                    item.visible_in_main_pass
                        && ctx.frame.outline_entities.contains(&item.entity_id)
                });
                if outline_requested {
                    self.ensure_outline_pipeline(ctx.device)?;
                    let outline = self
                        .outline_pipeline
                        .as_ref()
                        .context("ForwardPass: outline pipeline missing")?;
                    ctx.device.cmd_bind_pipeline(
                        ctx.cmd,
                        vk::PipelineBindPoint::GRAPHICS,
                        outline.pipeline,
                    );
                    ctx.device.cmd_bind_descriptor_sets(
                        ctx.cmd,
                        vk::PipelineBindPoint::GRAPHICS,
                        outline.layout,
                        0,
                        std::slice::from_ref(&frame_set),
                        std::slice::from_ref(&dynamic_offset),
                    );

                    for item in ctx.frame.draw_list {
                        if !item.visible_in_main_pass
                            || !ctx.frame.outline_entities.contains(&item.entity_id)
                        {
                            continue;
                        }
                        let Some(uploaded) = ctx.frame.mesh_manager.get(item.mesh) else {
                            continue;
                        };
                        let scale = (0..3)
                            .map(|column| {
                                let c = &item.model[column];
                                (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt()
                            })
                            .fold(0.0f32, f32::max);
                        let pc = OutlinePush {
                            model: item.model,
                            color: [4.0, 0.25, 0.02, 1.0],
                            thickness: 0.02 * scale.max(1e-4),
                            _pad: [0.0; 3],
                        };
                        let vertex_buffers = [uploaded.mesh.vertex_buffer];
                        let offsets = [0u64];
                        ctx.device
                            .cmd_bind_vertex_buffers(ctx.cmd, 0, &vertex_buffers, &offsets);
                        ctx.device.cmd_push_constants(
                            ctx.cmd,
                            outline.layout,
                            vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                            0,
                            bytemuck::bytes_of(&pc),
                        );
                        if let Some(ib) = uploaded.mesh.index_buffer {
                            let index_count = item.index_count.unwrap_or(uploaded.index_count());
                            if index_count == 0 {
                                continue;
                            }
                            ctx.device
                                .cmd_bind_index_buffer(ctx.cmd, ib, 0, vk::IndexType::UINT32);
                            ctx.device.cmd_draw_indexed(
                                ctx.cmd,
                                index_count,
                                1,
                                item.index_start,
                                0,
                                0,
                            );
                        } else {
                            ctx.device
                                .cmd_draw(ctx.cmd, uploaded.vertex_count(), 1, 0, 0);
                        }
                    }
                }
            }

            if !ctx.frame.particle_items.is_empty() {
                if let Err(e) = self.particles.draw(
                    ctx.device,
                    ctx.context,
                    ctx.cmd,
                    self.render_pass.unwrap(),
                    &camera.view_proj,
                    &camera.inv_view,
                    ctx.frame.particle_items,
                    ctx.frame_index,
                ) {
                    log::warn!("ParticlePass draw failed (particles skipped): {e:#}");
                }
            }

            if !ctx.frame.sprite_items.is_empty() {
                if let Err(e) = self.sprites.draw(
                    ctx.device,
                    ctx.context,
                    ctx.cmd,
                    self.render_pass.unwrap(),
                    self.bindless_set,
                    self.bindless_layout,
                    &camera.view_proj,
                    ctx.frame.sprite_items,
                    ctx.frame.sprite_lights,
                    ctx.frame_index,
                ) {
                    log::warn!("SpritePass draw failed (sprites skipped): {e:#}");
                }
            }

            if ctx.frame.show_grid && ctx.frame.render_mode != RenderMode::Sprites2D {
                if let Some(grid) = &self.ground_grid {
                    grid.draw(ctx.cmd, &camera.view_proj, ctx.frame.camera_orthographic);
                }
            }

            if let Some(gizmo) = &self.gizmo {
                gizmo.draw(ctx.cmd, &camera.view_proj, ctx.frame.gizmos);
            }
        }

        unsafe { ctx.device.cmd_end_render_pass(ctx.cmd) };

        if let Some(buffer) = self.depth_readback {
            if let Some(depth_image) = self.depth_image(ctx.image_index) {
                let device = ctx.device;
                let range = vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::DEPTH)
                    .level_count(1)
                    .layer_count(1);
                let to_transfer = vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                    .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(depth_image)
                    .subresource_range(range)
                    .src_access_mask(vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_READ);
                unsafe {
                    device.cmd_pipeline_barrier(
                        ctx.cmd,
                        vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
                            | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
                        vk::PipelineStageFlags::TRANSFER,
                        vk::DependencyFlags::empty(),
                        &[],
                        &[],
                        std::slice::from_ref(&to_transfer),
                    );
                }
                let region = vk::BufferImageCopy::default()
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::DEPTH)
                            .mip_level(0)
                            .base_array_layer(0)
                            .layer_count(1),
                    )
                    .image_extent(vk::Extent3D {
                        width: self.extent.width,
                        height: self.extent.height,
                        depth: 1,
                    });
                unsafe {
                    device.cmd_copy_image_to_buffer(
                        ctx.cmd,
                        depth_image,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        buffer,
                        &[region],
                    );
                }
                let to_attachment = vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .new_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(depth_image)
                    .subresource_range(range)
                    .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                    .dst_access_mask(
                        vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_READ
                            | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
                    );
                unsafe {
                    device.cmd_pipeline_barrier(
                        ctx.cmd,
                        vk::PipelineStageFlags::TRANSFER,
                        vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
                            | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
                        vk::DependencyFlags::empty(),
                        &[],
                        &[],
                        std::slice::from_ref(&to_attachment),
                    );
                }
                self.depth_capture_frame = Some(ctx.frame_index);
            }
        }

        let idx = ctx.image_index;
        if let (Some(v), Some(i)) = (self.color_view(idx), self.color_image(idx)) {
            resources.set_image_view(self.out_color_h, v);
            resources.set_image(self.out_color_h, i);
        }
        if let (Some(v), Some(i)) = (self.depth_view(idx), self.depth_image(idx)) {
            resources.set_image_view(self.out_depth_h, v);
            resources.set_image(self.out_depth_h, i);
        }
        if let (Some(v), Some(i)) = (self.normal_view(idx), self.normal_image(idx)) {
            resources.set_image_view(self.out_normal_h, v);
            resources.set_image(self.out_normal_h, i);
        }
        if let (Some(v), Some(i)) = (self.velocity_view(idx), self.velocity_image(idx)) {
            resources.set_image_view(self.out_velocity_h, v);
            resources.set_image(self.out_velocity_h, i);
        }
        if let (Some(v), Some(i)) = (self.albedo_view(idx), self.albedo_image(idx)) {
            resources.set_image_view(self.out_albedo_h, v);
            resources.set_image(self.out_albedo_h, i);
        }

        log::trace!(
            "ForwardPass: rendered {} draws into {}x{} ({} in list)",
            drawn,
            self.extent.width,
            self.extent.height,
            ctx.frame.draw_list.len()
        );
        Ok(())
    }

    fn graph_info(&self) -> PassInfo {
        PassInfo {
            index: usize::MAX,
            name: self.name().to_string(),
            kind: PassKind::Forward,
            #[cfg(feature = "shadows")]
            inputs: vec![SHADOW_MAP_H],
            #[cfg(not(feature = "shadows"))]
            inputs: Vec::new(),
            outputs: vec![
                self.out_depth_h,
                self.out_normal_h,
                self.out_color_h,
                self.out_velocity_h,
                self.out_albedo_h,
            ],
        }
    }

    fn warmup(&mut self, device: &ash::Device, context: &VulkanContext) -> Result<()> {
        if !self.pipelines.is_empty() {
            return Ok(());
        }
        self.ensure_render_pass(context)?;
        self.ensure_pipelines(device, &[(None, DrawState::default())], false)
    }
}

impl Drop for ForwardPass {
    fn drop(&mut self) {
        if let Some(device) = self.device.take() {
            for fb in self.framebuffers.drain(..).flatten() {
                unsafe { device.destroy_framebuffer(fb, None) };
            }
            for depth in self.depth_images.drain(..).flatten() {
                let mut d = depth;
                unsafe { d.destroy(&device) };
            }
            if let Some(rp) = self.render_pass.take() {
                unsafe { device.destroy_render_pass(rp, None) };
            }
            self.pipelines.clear();
            if let Some(layout) = self.frame_set_layout.take() {
                unsafe { device.destroy_descriptor_set_layout(layout, None) };
            }
            if let Some(pool) = self.frame_set_pool.take() {
                unsafe { device.destroy_descriptor_pool(pool, None) };
            }
            if let Some(layout) = self.shadow_ds_layout.take() {
                unsafe { device.destroy_descriptor_set_layout(layout, None) };
            }
            if let Some(pool) = self.shadow_ds_pool.take() {
                unsafe { device.destroy_descriptor_pool(pool, None) };
            }
            for buf in self.light_buffers.drain(..) {
                if buf != vk::Buffer::null() {
                    unsafe { device.destroy_buffer(buf, None) };
                }
            }
            for mem in self.light_memories.drain(..) {
                if mem != vk::DeviceMemory::null() {
                    unsafe { device.free_memory(mem, None) };
                }
            }
            self.skybox.destroy(&device);
            self.gizmo = None;
            self.ground_grid = None;
        }
    }
}

/// Blend state of color attachment (MRT: attachment 0 = color, attachment 1 = view normal).
///
/// Attachment 0 configured by material state (`_SrcBlend` / `_DstBlend` / `_BlendOp`);
/// default `One`/`Zero`/`Add` means opaque, then blend disabled. Attachment 1 never blends
/// (view-space normal must be written as-is).
fn color_blend_attachments(state: DrawState) -> [vk::PipelineColorBlendAttachmentState; 4] {
    let mut color = vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::RGBA);
    if state.blends() {
        color = color
            .blend_enable(true)
            .src_color_blend_factor(blend_factor(state.src_blend))
            .dst_color_blend_factor(blend_factor(state.dst_blend))
            .color_blend_op(blend_operation(state.blend_op))
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD);
    }
    [
        color,
        vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(false),
        vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::R | vk::ColorComponentFlags::G)
            .blend_enable(false),
        vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(false),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamped_light_count_clamps_to_light_max() {
        assert_eq!(ForwardPass::clamped_light_count(0), 0);
        assert_eq!(ForwardPass::clamped_light_count(1), 1);
        assert_eq!(
            ForwardPass::clamped_light_count(crate::descriptor::LIGHT_MAX as usize),
            crate::descriptor::LIGHT_MAX as usize
        );
        assert_eq!(
            ForwardPass::clamped_light_count(crate::descriptor::LIGHT_MAX as usize + 1),
            crate::descriptor::LIGHT_MAX as usize
        );
        assert_eq!(
            ForwardPass::clamped_light_count(100),
            crate::descriptor::LIGHT_MAX as usize
        );
    }

    #[test]
    fn forward_pass_initializes_empty_light_resources() {
        let pass = ForwardPass::new(vk::Format::B8G8R8A8_UNORM);
        assert!(pass.light_buffers.is_empty());
        assert!(pass.light_memories.is_empty());
    }

    fn static_draw(mesh: crate::managers::MeshHandle, material: Option<u32>) -> crate::DrawItem {
        crate::DrawItem {
            mesh,
            entity_id: 1,
            model: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            material,
            cast_shadow: false,
            visible_in_main_pass: true,
            cast_shadow_two_sided: false,
            bone_offset: None,
            splat_slots: [u32::MAX; 4],
            splat_tiling: 0.0,
            shader: None,
            state: DrawState::default(),
            world_bounds: None,
            index_start: 0,
            index_count: None,
        }
    }

    #[test]
    fn static_batches_group_identical_mesh_and_material() {
        let mesh = crate::managers::MeshHandle::default();
        let draws = [
            static_draw(mesh, Some(3)),
            static_draw(mesh, Some(3)),
            static_draw(mesh, Some(4)),
        ];
        let batches = build_static_batches_with(&draws, |_| true);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].items, vec![0, 1]);
        assert_eq!(batches[1].items, vec![2]);
    }

    #[test]
    fn static_batches_keep_skinned_and_transparent_as_singletons() {
        let mesh = crate::managers::MeshHandle::default();
        let mut skinned = static_draw(mesh, None);
        skinned.bone_offset = Some(0);
        let mut transparent = static_draw(mesh, None);
        transparent.state.render_queue = prism_gfx::RenderQueue::TRANSPARENT;
        let mut invisible = static_draw(mesh, None);
        invisible.visible_in_main_pass = false;
        let mut empty = static_draw(mesh, None);
        empty.index_count = Some(0);
        let draws = [skinned, transparent, invisible, empty];
        let batches = build_static_batches_with(&draws, |_| true);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].items, vec![0]);
        assert_eq!(batches[0].instance_offset, None);
        assert_eq!(batches[1].items, vec![1]);
        assert_eq!(batches[1].instance_offset, None);
        let draws = [static_draw(mesh, None), static_draw(mesh, None)];
        assert!(build_static_batches_with(&draws, |_| false).is_empty());
    }

    #[test]
    fn static_batches_do_not_overflow_the_instance_capacity() {
        let mesh = crate::managers::MeshHandle::default();
        let capacity = 4;
        let draws: Vec<_> = (0..=capacity).map(|_| static_draw(mesh, None)).collect();
        let (batches, matrices) = prepare_static_batches_with(&draws, |_| true, capacity);
        assert!(matrices.is_empty());
        assert_eq!(batches.len(), draws.len());
        assert!(batches.iter().all(|batch| batch.instance_offset.is_none()));
    }

    #[test]
    fn static_batches_assign_offsets_to_the_first_group() {
        let mesh = crate::managers::MeshHandle::default();
        let mut draws = vec![static_draw(mesh, None), static_draw(mesh, None)];
        draws.push(static_draw(mesh, Some(1)));
        let (batches, matrices) = prepare_static_batches_with(&draws, |_| true, 8);
        assert_eq!(matrices.len(), 2);
        assert_eq!(batches[0].instance_offset, Some(0));
        assert_eq!(batches[0].items.len(), 2);
        assert_eq!(batches[1].instance_offset, None);
        assert_eq!(batches[1].items.len(), 1);
    }
}
