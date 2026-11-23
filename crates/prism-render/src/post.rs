//! Post-processing pass — HDR scene tone map (+ optional FXAA) → sRGB swapchain
//!
//! Full-screen triangle fragment pass, samples ForwardPass HDR intermediate color attachment,
//! applies Reinhard or ACES tone map (per `tonemap_mode`),
//! and writes result to swapchain image. Replaces inline tone map previously in `scene_frag.slang`,
//! keeping scene output linear HDR (usable by later post effects: bloom, temporal AA, etc.).
//!
//! When `aa_mode = 1`, performs FXAA after tone map (post antialiasing, see #494):
//! only samples already-tonemapped color several times within this pass, **no pipeline state changes**,
//! no extra render targets / resolve needed, so swapchain and offscreen (editor viewport) paths get
//! same result. Debug graph (`debug_rt != 0`) force-disables it.
//!
//! ## Resources
//! - One descriptor set binding HDR color as combined image sampler
//! - Own render pass + pipeline (1 color attachment = swapchain format, no depth)
//! - HDR input view updated each frame via `set_input` (ForwardPass rotates one HDR image per swapchain slot,
//!   matching its just-written frame buffer).
//!
//! ## Layout transitions recorded in `execute`
//! 1. barrier: HDR `COLOR_ATTACHMENT_OPTIMAL -> SHADER_READ_ONLY_OPTIMAL`.
//! 2. barrier: swapchain `UNDEFINED -> COLOR_ATTACHMENT_OPTIMAL` via the render
//!    pass load op.
//! 3. begin render pass (writes swapchain), draw fullscreen triangle, end.
//! 4. The caller (`GraphRenderer::render`) barriers the swapchain to
//!    `PRESENT_SRC_KHR`; the external overlay does the same through its own
//!    load and transition pass.

use anyhow::Context as _;
use ash::vk;

use crate::buffer::create_buffer;
use crate::context::VulkanContext;
use crate::lut::Lut3d;
use crate::pipeline::{GraphicsPipeline, PipelineDesc};
use crate::render_graph::{
    GraphResources, PassInfo, PassKind, RenderContext, RenderGraphBuilder, RenderMode,
    RenderPassNode, RenderSettings, ResourceHandle, ResourceType, ResourceUsage, ANF_COLOR_H,
    FINAL_COLOR_H, FORWARD_COLOR_H, FORWARD_DEPTH_H, FORWARD_NORMAL_H, PT_COLOR_H, TAA_COLOR_H,
};
use crate::render_pass::find_memory_type;
use crate::shader;
use crate::shader_bindings;

const _: () = assert!(
    std::mem::size_of::<shader_bindings::post::PostPush>() as u32
        == shader_bindings::post::PUSH_CONSTANT_SIZE
);

/// Camera matrices for the camera-motion-blur effect (#502 Phase 2).
///
/// `invViewProj` reconstructs world space from the current depth buffer;
/// `prevViewProj` projects that point into the previous frame. `resolution`
/// converts the normalized velocity into pixels for the trail clamp. Laid out
/// to match `MotionBlurData` in `post.slang`.
use crate::shader_bindings::post::MotionBlurData as MotionBlurUniform;

/// Fullscreen-triangle tonemap pass: HDR scene color -> sRGB swapchain.
pub struct PostPass {
    render_pass: Option<vk::RenderPass>,
    /// Swapchain color format (set in `new`, used to rebuild the render pass on
    /// `drop_target`/`set_target`). Stored so the visualizer can read it.
    color_format: vk::Format,
    /// Layout the render pass leaves the color attachment in.
    ///
    /// Window targets end in `PRESENT_SRC_KHR` so the swapchain image can be
    /// presented; offscreen targets stay in `COLOR_ATTACHMENT_OPTIMAL` — a
    /// headless `VulkanContext` never enables `VK_KHR_swapchain` (no surface),
    /// and `PRESENT_SRC_KHR` is not a valid layout without it.
    final_layout: vk::ImageLayout,
    /// One framebuffer per swapchain image (each wraps its swapchain view).
    framebuffers: Vec<Option<vk::Framebuffer>>,
    /// Cached swapchain views the framebuffers were built against (for
    /// rebuild detection, mirroring ForwardPass's mode
    target_views: Vec<vk::ImageView>,
    target_images: Vec<vk::Image>,
    extent: vk::Extent2D,
    pipeline: Option<GraphicsPipeline>,
    /// One descriptor set per frame-in-flight so `set_input` can update frame
    /// N's set without disturbing frame N-1's still-in-flight set
    /// (VUID-vkUpdateDescriptorSets-None-03047). Each binds the HDR color view
    /// as a combined image sampler.
    descriptor_sets: Vec<vk::DescriptorSet>,
    ds_layout: vk::DescriptorSetLayout,
    ds_pool: vk::DescriptorPool,
    sampler: vk::Sampler,
    /// Nearest sampler (`InternalScaleMode::Integer`, #613): pixel art upscale not blurred.
    sampler_nearest: vk::Sampler,
    /// Depth sampler (`ShaderMode::Depth` / depth of field): nearest, avoid interpolating across depth edges.
    sampler_depth: vk::Sampler,
    /// The HDR view currently bound to each frame-in-flight's descriptor set.
    /// Tracked so we skip redundant descriptor rewrites.
    bound_hdrs: Vec<vk::ImageView>,
    /// Whether each in-flight frame currently bound nearest sampler (cached with `bound_hdrs`).
    bound_nearest: Vec<bool>,
    /// Depth view currently bound per in-flight frame (#502 DOF).
    bound_depths: Vec<vk::ImageView>,
    /// 3D color-grading LUT (#502). Always bound (identity by default), so
    /// `lutStrength = 0` can skip the sample without an unbound descriptor.
    lut_image: vk::Image,
    lut_view: vk::ImageView,
    lut_memory: vk::DeviceMemory,
    /// Edge length of the bound LUT, forwarded to the shader for texel-centre
    /// sampling. A loaded `.cube` may have any size in `2..=128`.
    lut_size: u32,
    /// Motion-blur matrix UBOs (set 0 binding 3), one per frame in flight.
    motion_buffers: Vec<vk::Buffer>,
    motion_memories: Vec<vk::DeviceMemory>,
    /// Zeroed stand-in bound to set 0 binding 4 until
    /// [`Self::set_adapted_luminance`] supplies the metering pass's buffer.
    /// Owned by this pass, so it is freed in [`Self::destroy`].
    luminance_fallback: vk::Buffer,
    luminance_fallback_memory: vk::DeviceMemory,
    /// Previous frame's view-projection, used to reproject static pixels.
    /// `None` on the first frame (velocity is zero).
    prev_view_proj: Option<[[f32; 4]; 4]>,
    device: Option<ash::Device>,
}

impl PostPass {
    /// Create the pass + persistent resources (sampler, ds layout/pool/sets,
    /// render pass). `color_format` is the swapchain format. `frames_in_flight`
    /// is the number of descriptor sets to allocate (one per concurrent frame
    /// so `set_input` doesn't disturb an in-flight set). The pipeline +
    /// framebuffers are created lazily once a render pass + target exist.
    ///
    /// `final_layout` is where the render pass leaves the color attachment:
    /// `PRESENT_SRC_KHR` for a swapchain image, `COLOR_ATTACHMENT_OPTIMAL` for
    /// an offscreen target (see [`Self::final_layout`]).
    pub fn new(
        context: &VulkanContext,
        color_format: vk::Format,
        frames_in_flight: u32,
        final_layout: vk::ImageLayout,
    ) -> anyhow::Result<Self> {
        let device = &context.device;

        let sampler = unsafe {
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
        .context("PostPass: create sampler")?;

        let sampler_nearest = unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::NEAREST)
                    .min_filter(vk::Filter::NEAREST)
                    .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .min_lod(0.0)
                    .max_lod(vk::LOD_CLAMP_NONE),
                None,
            )
        }
        .context("PostPass: create nearest sampler")?;

        let sampler_depth = unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::NEAREST)
                    .min_filter(vk::Filter::NEAREST)
                    .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .min_lod(0.0)
                    .max_lod(vk::LOD_CLAMP_NONE),
                None,
            )
        }
        .context("PostPass: create depth sampler")?;

        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(shader_bindings::post::HDR_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(shader_bindings::post::DEPTH_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(shader_bindings::post::LUT_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(shader_bindings::post::MOTION_BINDING)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(shader_bindings::post::ADAPTED_LUMINANCE_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let ds_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )
        }
        .context("PostPass: create ds layout")?;

        let fif = frames_in_flight.max(1);
        let ds_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(fif)
                    .pool_sizes(&[
                        vk::DescriptorPoolSize {
                            ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                            descriptor_count: fif * 3,
                        },
                        vk::DescriptorPoolSize {
                            ty: vk::DescriptorType::UNIFORM_BUFFER,
                            descriptor_count: fif,
                        },
                        vk::DescriptorPoolSize {
                            ty: vk::DescriptorType::STORAGE_BUFFER,
                            descriptor_count: fif,
                        },
                    ]),
                None,
            )
        }
        .context("PostPass: create ds pool")?;

        let layouts = vec![ds_layout; fif as usize];
        let descriptor_sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(ds_pool)
                    .set_layouts(&layouts),
            )
        }
        .context("PostPass: allocate ds")?;
        let descriptor_sets: Vec<vk::DescriptorSet> = descriptor_sets;

        let identity = Lut3d::identity(32);
        let (lut_image, lut_memory, lut_view) =
            create_lut_image(context, &identity).context("PostPass: create default LUT")?;
        for &set in &descriptor_sets {
            let info = vk::DescriptorImageInfo::default()
                .image_view(lut_view)
                .sampler(sampler)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
            let write = vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(shader_bindings::post::LUT_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(std::slice::from_ref(&info));
            unsafe { device.update_descriptor_sets(&[write], &[]) };
        }
        let motion_size = std::mem::size_of::<MotionBlurUniform>() as vk::DeviceSize;
        let mut motion_buffers = Vec::with_capacity(fif as usize);
        let mut motion_memories = Vec::with_capacity(fif as usize);
        for _ in 0..fif {
            let (buffer, memory) = crate::buffer::create_buffer(
                context,
                motion_size,
                crate::buffer::BufferUsage::UNIFORM_BUFFER,
                crate::buffer::MemoryProperties::HOST_VISIBLE
                    | crate::buffer::MemoryProperties::HOST_COHERENT,
            )
            .context("PostPass: create motion-blur UBO")?;
            let ptr =
                unsafe { device.map_memory(memory, 0, motion_size, vk::MemoryMapFlags::empty()) }
                    .context("PostPass: map motion-blur UBO")?;
            unsafe { std::ptr::write_bytes(ptr as *mut u8, 0, motion_size as usize) };
            unsafe { device.unmap_memory(memory) };
            motion_buffers.push(buffer);
            motion_memories.push(memory);
        }
        let motion_infos: Vec<vk::DescriptorBufferInfo> = motion_buffers
            .iter()
            .map(|buffer| {
                vk::DescriptorBufferInfo::default()
                    .buffer(*buffer)
                    .offset(0)
                    .range(motion_size)
            })
            .collect();
        let motion_writes: Vec<vk::WriteDescriptorSet> = descriptor_sets
            .iter()
            .zip(motion_infos.iter())
            .map(|(set, info)| {
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(shader_bindings::post::MOTION_BINDING)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(std::slice::from_ref(info))
            })
            .collect();
        unsafe { device.update_descriptor_sets(&motion_writes, &[]) };

        let (luminance_fallback, luminance_fallback_memory) = crate::buffer::create_buffer(
            context,
            4,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .context("PostPass: create fallback luminance buffer")?;
        let fallback_info = vk::DescriptorBufferInfo::default()
            .buffer(luminance_fallback)
            .offset(0)
            .range(4);
        let fallback_writes: Vec<vk::WriteDescriptorSet> = descriptor_sets
            .iter()
            .map(|set| {
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(shader_bindings::post::ADAPTED_LUMINANCE_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&fallback_info))
            })
            .collect();
        unsafe { device.update_descriptor_sets(&fallback_writes, &[]) };

        let render_pass = create_render_pass(device, color_format, final_layout)?;

        Ok(Self {
            render_pass: Some(render_pass),
            color_format,
            final_layout,
            framebuffers: Vec::new(),
            target_views: Vec::new(),
            target_images: Vec::new(),
            extent: vk::Extent2D {
                width: 0,
                height: 0,
            },
            pipeline: None,
            descriptor_sets,
            ds_layout,
            ds_pool,
            sampler,
            sampler_nearest,
            sampler_depth,
            bound_hdrs: vec![vk::ImageView::null(); fif as usize],
            bound_nearest: vec![false; fif as usize],
            bound_depths: vec![vk::ImageView::null(); fif as usize],
            lut_image,
            lut_view,
            lut_memory,
            lut_size: identity.size,
            motion_buffers,
            motion_memories,
            luminance_fallback,
            luminance_fallback_memory,
            prev_view_proj: None,
            device: Some(device.clone()),
        })
    }

    /// Swapchain extent PostPass tonemaps into. Exposed for the visualizer.
    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }

    /// Swapchain color format PostPass targets. Exposed for the visualizer.
    pub fn color_format(&self) -> vk::Format {
        self.color_format
    }

    /// Layout the render pass leaves the target image in.
    ///
    /// `PRESENT_SRC_KHR` for swapchain targets, `COLOR_ATTACHMENT_OPTIMAL` for
    /// offscreen targets. Exposed so tests can assert a headless renderer never
    /// asks for a swapchain-only layout.
    pub fn final_layout(&self) -> vk::ImageLayout {
        self.final_layout
    }

    /// Bind the metering pass's adapted-luminance buffer to set 0 binding 4.
    ///
    /// `GraphRenderer` calls this once after constructing both passes. The
    /// buffer holds a single `uint` (a float bit pattern) and is rewritten by
    /// `AutoExposurePass` every frame, so the descriptor only has to be updated
    /// here — the contents change underneath it. Passing the fallback back in
    /// restores the no-op behaviour.
    pub fn set_adapted_luminance(&mut self, device: &ash::Device, buffer: vk::Buffer) {
        let info = vk::DescriptorBufferInfo::default()
            .buffer(buffer)
            .offset(0)
            .range(4);
        let writes: Vec<vk::WriteDescriptorSet> = self
            .descriptor_sets
            .iter()
            .map(|set| {
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(shader_bindings::post::ADAPTED_LUMINANCE_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&info))
            })
            .collect();
        if !writes.is_empty() {
            unsafe { device.update_descriptor_sets(&writes, &[]) };
        }
    }

    /// Ensure the framebuffer for `image_index` exists and is built against the
    /// Current swapchain views + extent. Mirrors ForwardPass::set_target's
    /// per-slot rebuild logic (only rebuild an entry when its view changes, so
    /// in-flight framebuffers are never touched).
    pub fn set_target(
        &mut self,
        device: &ash::Device,
        swapchain_views: &[vk::ImageView],
        image_index: u32,
        extent: vk::Extent2D,
    ) -> anyhow::Result<()> {
        if extent.width == 0 || extent.height == 0 {
            return Ok(());
        }
        let idx = image_index as usize;
        if idx >= swapchain_views.len() {
            return Ok(());
        }
        let view = swapchain_views[idx];

        let swapchain_changed = self.target_views.len() != swapchain_views.len()
            || self.extent != extent
            || self
                .target_views
                .iter()
                .zip(swapchain_views.iter())
                .any(|(a, b)| a != b);
        if swapchain_changed {
            self.drop_target(device);
            self.target_views = swapchain_views.to_vec();
            self.extent = extent;
            self.framebuffers = (0..swapchain_views.len()).map(|_| None).collect();
        }

        let already_current = idx < self.target_views.len()
            && self.target_views[idx] == view
            && self.framebuffers[idx].is_some();
        if !already_current {
            let rp = self
                .render_pass
                .context("PostPass: render_pass missing in set_target")?;
            if let Some(old_fb) = self.framebuffers[idx].take() {
                unsafe { device.destroy_framebuffer(old_fb, None) };
            }
            let attachments = [view];
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
            .context("PostPass: create framebuffer")?;
            self.framebuffers[idx] = Some(fb);
            self.target_views[idx] = view;
        }
        Ok(())
    }

    /// Update the target image handles so `PostPass` can publish `FINAL_COLOR_H`.
    pub fn set_target_images(&mut self, images: &[vk::Image]) {
        self.target_images = images.to_vec();
    }

    /// Replace the 3D color-grading LUT (#502).
    ///
    /// Uploads `lut` as an `R32G32B32A32_SFLOAT` 3D image and rebinds it on
    /// every frame-in-flight descriptor set. The old image is destroyed only
    /// after `vkDeviceWaitIdle`, because a previously submitted frame may still
    /// be sampling it. LUT changes are user-driven and rare, so the stall is
    /// preferable to tracking per-frame retirement.
    pub fn set_lut(&mut self, context: &VulkanContext, lut: &Lut3d) -> anyhow::Result<()> {
        let device = &context.device;
        let (image, memory, view) =
            create_lut_image(context, lut).context("PostPass: upload LUT")?;

        unsafe { device.device_wait_idle() }.context("PostPass: wait before LUT swap")?;

        for &set in &self.descriptor_sets {
            let info = vk::DescriptorImageInfo::default()
                .image_view(view)
                .sampler(self.sampler)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
            let write = vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(shader_bindings::post::LUT_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(std::slice::from_ref(&info));
            unsafe { device.update_descriptor_sets(&[write], &[]) };
        }

        unsafe {
            device.destroy_image_view(self.lut_view, None);
            device.destroy_image(self.lut_image, None);
            device.free_memory(self.lut_memory, None);
        }
        self.lut_image = image;
        self.lut_view = view;
        self.lut_memory = memory;
        self.lut_size = lut.size;
        Ok(())
    }

    /// Drop the swapchain-derived framebuffers (called before swapchain
    /// recreate, mirroring ForwardPass::drop_target).
    pub fn drop_target(&mut self, device: &ash::Device) {
        for fb in self.framebuffers.drain(..).flatten() {
            unsafe { device.destroy_framebuffer(fb, None) };
        }
        self.target_views.clear();
        self.target_images.clear();
        self.extent = vk::Extent2D {
            width: 0,
            height: 0,
        };
    }

    /// Update the HDR input view bound to the frame-in-flight's descriptor set.
    /// Called every frame from `GraphRenderer::render` before `execute`.
    /// Bind `view` (sampled with `image_layout`) as the input texture for this
    /// frame-in-flight's descriptor set. Skips the write when `view` matches
    /// the currently-bound one. `image_layout` must match the image's actual
    /// layout at draw time (depth uses `DEPTH_STENCIL_READ_ONLY_OPTIMAL`,
    /// color/normal use `SHADER_READ_ONLY_OPTIMAL`).
    pub fn set_input(
        &mut self,
        device: &ash::Device,
        frame_index: u32,
        view: vk::ImageView,
        image_layout: vk::ImageLayout,
        nearest: bool,
    ) {
        let i = (frame_index as usize) % self.descriptor_sets.len();
        if view == self.bound_hdrs[i] && nearest == self.bound_nearest[i] {
            return;
        }
        self.bound_hdrs[i] = view;
        self.bound_nearest[i] = nearest;
        let image_info = vk::DescriptorImageInfo::default()
            .image_view(view)
            .sampler(if nearest {
                self.sampler_nearest
            } else {
                self.sampler
            })
            .image_layout(image_layout);
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.descriptor_sets[i])
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(std::slice::from_ref(&image_info));
        unsafe { device.update_descriptor_sets(&[write], &[]) };
    }

    /// Bind the scene depth view used by depth of field (#502 Phase 2).
    ///
    /// Called every frame from `execute` when the graph published
    /// `FORWARD_DEPTH_H`; the view is always in
    /// `DEPTH_STENCIL_READ_ONLY_OPTIMAL` (declared in `setup`).
    pub fn set_depth_input(&mut self, device: &ash::Device, frame_index: u32, view: vk::ImageView) {
        let i = (frame_index as usize) % self.descriptor_sets.len();
        if view == self.bound_depths[i] {
            return;
        }
        self.bound_depths[i] = view;
        let image_info = vk::DescriptorImageInfo::default()
            .image_view(view)
            .sampler(self.sampler_depth)
            .image_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL);
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.descriptor_sets[i])
            .dst_binding(shader_bindings::post::DEPTH_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(std::slice::from_ref(&image_info));
        unsafe { device.update_descriptor_sets(&[write], &[]) };
    }

    /// Upload the camera matrices used by camera motion blur (#502 Phase 2).
    fn set_motion_blur(
        &mut self,
        device: &ash::Device,
        frame_index: u32,
        inv_view_proj: [[f32; 4]; 4],
        prev_view_proj: [[f32; 4]; 4],
        resolution: [f32; 2],
    ) -> anyhow::Result<()> {
        if self.motion_memories.is_empty() {
            return Ok(());
        }
        let index = (frame_index as usize) % self.motion_memories.len();
        let memory = self.motion_memories[index];
        if memory == vk::DeviceMemory::null() {
            return Ok(());
        }
        let uniform = MotionBlurUniform {
            inv_view_proj,
            prev_view_proj,
            resolution,
            ..Default::default()
        };
        let size = std::mem::size_of::<MotionBlurUniform>() as vk::DeviceSize;
        let ptr = unsafe { device.map_memory(memory, 0, size, vk::MemoryMapFlags::empty()) }
            .context("PostPass::set_motion_blur: map")?;
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytemuck::bytes_of(&uniform).as_ptr(),
                ptr as *mut u8,
                size as usize,
            );
            device.unmap_memory(memory);
        }
        Ok(())
    }

    /// Record the PostPass into `cmd`. Must run AFTER ForwardPass (which leaves
    /// the HDR color in COLOR_ATTACHMENT_OPTIMAL). The caller barriers the
    /// Swapchain to PRESENT_SRC_KHR (or external overlay handles it) after this.
    /// `frame_index` selects the per-frame-in-flight descriptor set; `image_index`
    /// selects the per-swapchain-image framebuffer.
    #[expect(
        clippy::too_many_arguments,
        reason = "the pass already owns the rest of its state; these are per-frame inputs"
    )]
    pub fn execute(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        frame_index: u32,
        image_index: u32,
        hdr_image: vk::Image,
        dest: vk::Rect2D,
        push: &shader_bindings::post::PostPush,
    ) -> anyhow::Result<()> {
        self.ensure_pipeline(device)?;
        let rp = self.render_pass.unwrap();
        let pipeline = self.pipeline.as_ref().unwrap();
        let fb = self
            .framebuffers
            .get(image_index as usize)
            .copied()
            .flatten()
            .context("PostPass: no framebuffer for image_index (call set_target first)")?;
        let ds = self
            .descriptor_sets
            .get(frame_index as usize)
            .copied()
            .context("PostPass: no descriptor set for frame_index")?;

        let _ = hdr_image;

        let clear_values = [vk::ClearValue {
            color: vk::ClearColorValue {
                float32: [0.0, 0.0, 0.0, 1.0],
            },
        }];
        let begin_info = vk::RenderPassBeginInfo::default()
            .render_pass(rp)
            .framebuffer(fb)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.extent,
            })
            .clear_values(&clear_values);
        unsafe { device.cmd_begin_render_pass(cmd, &begin_info, vk::SubpassContents::INLINE) };

        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.layout,
                0,
                std::slice::from_ref(&ds),
                &[],
            );
            device.cmd_set_viewport(
                cmd,
                0,
                &[vk::Viewport {
                    x: dest.offset.x as f32,
                    y: dest.offset.y as f32,
                    width: dest.extent.width as f32,
                    height: dest.extent.height as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            device.cmd_set_scissor(cmd, 0, &[dest]);
            device.cmd_push_constants(
                cmd,
                pipeline.layout,
                vk::ShaderStageFlags::FRAGMENT,
                0,
                bytemuck::bytes_of(push),
            );
            device.cmd_draw(cmd, 3, 1, 0, 0);
        }

        unsafe { device.cmd_end_render_pass(cmd) };

        log::trace!(
            "PostPass: tonemapped HDR -> swapchain image {} ({}x{})",
            image_index,
            self.extent.width,
            self.extent.height
        );
        Ok(())
    }

    fn ensure_pipeline(&mut self, device: &ash::Device) -> anyhow::Result<()> {
        if self.pipeline.is_some() {
            return Ok(());
        }
        let rp = self
            .render_pass
            .context("PostPass: render_pass not created before pipeline")?;

        const VERT_SPV: &[u8] = prism_render_shaders::POST_VERT_SPV;
        const FRAG_SPV: &[u8] = prism_render_shaders::POST_FRAG_SPV;
        let vert_module =
            shader::load_shader_module(device, VERT_SPV).context("PostPass: load vert")?;
        let frag_module =
            shader::load_shader_module(device, FRAG_SPV).context("PostPass: load frag")?;

        let vert_entry = std::ffi::CString::new(shader_bindings::post::ENTRY_VERTEX_MAIN).unwrap();
        let frag_entry =
            std::ffi::CString::new(shader_bindings::post::ENTRY_FRAGMENT_MAIN).unwrap();
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

        let binding_descs: [vk::VertexInputBindingDescription; 0] = [];
        let attr_descs: [vk::VertexInputAttributeDescription; 0] = [];
        let set_layouts = [self.ds_layout];

        let push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(shader_bindings::post::PUSH_CONSTANT_SIZE)];

        let blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(false);

        let pipeline = GraphicsPipeline::new(&PipelineDesc {
            device,
            shader_stages: &shader_stages,
            vertex_binding_desc: &binding_descs,
            vertex_attr_descs: &attr_descs,
            descriptor_set_layouts: &set_layouts,
            push_constant_ranges: &push,
            render_pass: rp,
            subpass: 0,
            cull_mode: Some(vk::CullModeFlags::NONE),
            depth_bias_enable: None,
            depth_bias_constant_factor: None,
            depth_bias_slope_factor: None,
            depth_write_enable: Some(false),
            depth_test_enable: None,
            color_attachment_count: None,
            color_blend_attachments: Some(std::slice::from_ref(&blend_attachment)),
            front_face: None,
            polygon_mode: None,
        })
        .context("PostPass: create pipeline")?;

        unsafe {
            device.destroy_shader_module(vert_module, None);
            device.destroy_shader_module(frag_module, None);
        }
        self.pipeline = Some(pipeline);
        Ok(())
    }

    /// Destroy all GPU resources. Called from GraphRenderer::destroy on
    /// shutdown. `device_wait_idle` must already have been called by the caller.
    pub fn destroy(&mut self, device: &ash::Device) {
        self.drop_target(device);
        if let Some(rp) = self.render_pass.take() {
            unsafe { device.destroy_render_pass(rp, None) };
        }
        self.pipeline = None;
        unsafe { device.destroy_descriptor_set_layout(self.ds_layout, None) };
        unsafe { device.destroy_descriptor_pool(self.ds_pool, None) };
        unsafe { device.destroy_sampler(self.sampler, None) };
        unsafe { device.destroy_sampler(self.sampler_nearest, None) };
        unsafe { device.destroy_sampler(self.sampler_depth, None) };
        unsafe {
            device.destroy_image_view(self.lut_view, None);
            device.destroy_image(self.lut_image, None);
            device.free_memory(self.lut_memory, None);
        }
        for buffer in self.motion_buffers.drain(..) {
            if buffer != vk::Buffer::null() {
                unsafe { device.destroy_buffer(buffer, None) };
            }
        }
        for memory in self.motion_memories.drain(..) {
            if memory != vk::DeviceMemory::null() {
                unsafe { device.free_memory(memory, None) };
            }
        }
        if self.luminance_fallback != vk::Buffer::null() {
            unsafe { device.destroy_buffer(self.luminance_fallback, None) };
            self.luminance_fallback = vk::Buffer::null();
        }
        if self.luminance_fallback_memory != vk::DeviceMemory::null() {
            unsafe { device.free_memory(self.luminance_fallback_memory, None) };
            self.luminance_fallback_memory = vk::DeviceMemory::null();
        }
        self.device = None;
    }
}

impl Drop for PostPass {
    fn drop(&mut self) {
        if let Some(device) = self.device.take() {
            self.destroy(&device);
        }
    }
}

impl RenderPassNode for PostPass {
    fn name(&self) -> &str {
        "PostPass"
    }

    fn setup(&mut self, graph: &mut RenderGraphBuilder, _settings: &RenderSettings) {
        graph.read_usage(ResourceUsage {
            handle: FORWARD_COLOR_H,
            access: vk::AccessFlags::SHADER_READ,
            stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        });
        graph.read_usage(ResourceUsage {
            handle: TAA_COLOR_H,
            access: vk::AccessFlags::SHADER_READ,
            stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        });
        graph.read_usage(ResourceUsage {
            handle: PT_COLOR_H,
            access: vk::AccessFlags::SHADER_READ,
            stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        });
        graph.read_usage(ResourceUsage {
            handle: FORWARD_DEPTH_H,
            access: vk::AccessFlags::SHADER_READ,
            stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
            layout: vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
        });
        graph.read_usage(ResourceUsage {
            handle: FORWARD_NORMAL_H,
            access: vk::AccessFlags::SHADER_READ,
            stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        });
        graph.read_usage(ResourceUsage {
            handle: ANF_COLOR_H,
            access: vk::AccessFlags::SHADER_READ,
            stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        });
        graph.read_usage(ResourceUsage {
            handle: crate::render_graph::UPSCALED_COLOR_H,
            access: vk::AccessFlags::SHADER_READ,
            stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        });
        graph.read_usage(ResourceUsage {
            handle: crate::render_graph::DLSS_COLOR_H,
            access: vk::AccessFlags::SHADER_READ,
            stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        });
        graph.read_usage(ResourceUsage {
            handle: crate::render_graph::VOXEL_COLOR_H,
            access: vk::AccessFlags::SHADER_READ,
            stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        });
        graph.create_resource_at(
            FINAL_COLOR_H,
            ResourceType::ColorAttachment {
                format: self.color_format,
                extent: self.extent,
                sample_count: vk::SampleCountFlags::TYPE_1,
            },
        );
        graph.write_usage(ResourceUsage {
            handle: FINAL_COLOR_H,
            access: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            stage: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            layout: self.final_layout,
        });
    }

    fn execute(
        &mut self,
        ctx: &RenderContext,
        resources: &mut GraphResources,
    ) -> anyhow::Result<()> {
        let is_pt = ctx.frame.render_mode == RenderMode::PathTrace;
        let has_camera = ctx.frame.has_camera;
        let has_pt_view = resources.published_view(PT_COLOR_H).is_some();
        let anf_view = if ctx.settings.anf_sr && !is_pt && ctx.frame.debug_rt == 0 {
            resources.published_view(ANF_COLOR_H)
        } else {
            None
        };
        let dlss_view = if ctx.settings.dlss_mode != crate::render_graph::DlssMode::Off
            && ctx.frame.debug_rt == 0
            && anf_view.is_none()
        {
            resources.published_view(crate::render_graph::DLSS_COLOR_H)
        } else {
            None
        };
        let upscaled_view = if ctx.settings.mobile_upscaler.is_temporal_compute()
            && !is_pt
            && ctx.frame.debug_rt == 0
            && anf_view.is_none()
            && dlss_view.is_none()
        {
            resources.published_view(crate::render_graph::UPSCALED_COLOR_H)
        } else {
            None
        };
        let taa_view = if ctx.settings.taa
            && ctx.frame.debug_rt == 0
            && anf_view.is_none()
            && dlss_view.is_none()
            && upscaled_view.is_none()
        {
            resources.published_view(TAA_COLOR_H)
        } else {
            None
        };
        let (handle, image_layout) = if anf_view.is_some() {
            (ANF_COLOR_H, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        } else if dlss_view.is_some() {
            (
                crate::render_graph::DLSS_COLOR_H,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            )
        } else if upscaled_view.is_some() {
            (
                crate::render_graph::UPSCALED_COLOR_H,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            )
        } else if taa_view.is_some() {
            (TAA_COLOR_H, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        } else if !is_pt
            && ctx.frame.debug_rt == 0
            && resources
                .published_view(crate::render_graph::VOXEL_COLOR_H)
                .is_some()
        {
            (
                crate::render_graph::VOXEL_COLOR_H,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            )
        } else {
            select_input_target(ctx.frame.debug_rt, is_pt, has_camera, has_pt_view)
        };
        let input_view = match resources.published_view(handle) {
            Some(v) => v,
            None => {
                log::warn!("PostPass: no {:?} view published; skipping", handle);
                return Ok(());
            }
        };
        let input_image = resources
            .published_image(handle)
            .unwrap_or(vk::Image::null());

        self.set_target(
            ctx.device,
            ctx.frame.swapchain_views,
            ctx.image_index,
            ctx.extent,
        )
        .context("PostPass: set_target")?;

        let already_upscaled = anf_view.is_some() || upscaled_view.is_some();
        let spatial_upscale = matches!(
            ctx.settings.mobile_upscaler,
            crate::render_graph::MobileUpscaler::Nearest
                | crate::render_graph::MobileUpscaler::Linear
        );
        let nearest = !already_upscaled
            && ctx.render_extent != ctx.extent
            && (ctx.settings.mobile_upscaler == crate::render_graph::MobileUpscaler::Nearest
                || ctx.settings.internal_scale_mode
                    == crate::render_graph::InternalScaleMode::Integer);
        let dest = if already_upscaled || spatial_upscale {
            vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: ctx.extent,
            }
        } else {
            crate::render_graph::post_destination(
                ctx.extent,
                ctx.render_extent,
                ctx.settings.internal_scale_mode,
            )
        };
        self.set_input(
            ctx.device,
            ctx.frame_index,
            input_view,
            image_layout,
            nearest,
        );

        let depth_view = resources.published_view(FORWARD_DEPTH_H);
        if let Some(depth_view) = depth_view {
            self.set_depth_input(ctx.device, ctx.frame_index, depth_view);
        }

        let current_view_proj = ctx.frame.view_proj;
        let prev_view_proj = self
            .prev_view_proj
            .replace(current_view_proj)
            .unwrap_or(current_view_proj);
        let inv_view_proj = crate::math::mat_inverse(&current_view_proj);
        let motion_extent = if ctx.render_extent.width > 0 && ctx.render_extent.height > 0 {
            ctx.render_extent
        } else {
            ctx.extent
        };
        self.set_motion_blur(
            ctx.device,
            ctx.frame_index,
            inv_view_proj,
            prev_view_proj,
            [motion_extent.width as f32, motion_extent.height as f32],
        )?;
        let motion_blur = if ctx.frame.debug_rt == 0
            && ctx.frame.render_mode == RenderMode::Raster
            && ctx.frame.cameras.len() <= 1
            && depth_view.is_some()
        {
            ctx.frame.grade.motion_blur
        } else {
            0.0
        };

        let proj22 = ctx.frame.proj22;
        let proj32 = ctx.frame.proj32;
        let near = if (proj22 - 1.0).abs() > 1e-6 {
            proj32 / (proj22 + 1.0)
        } else {
            0.1
        };
        let far = if (proj22 - 1.0).abs() > 1e-6 {
            proj32 / (proj22 - 1.0)
        } else {
            100.0
        };
        let dof_aperture = if ctx.frame.debug_rt == 0 && depth_view.is_some() {
            ctx.frame.grade.dof_aperture
        } else {
            0.0
        };
        let push = shader_bindings::post::PostPush {
            tonemap_mode: ctx.frame.tonemap_mode,
            aa_mode: effective_aa_mode(ctx.frame.debug_rt, ctx.frame.aa_mode),
            contrast: ctx.frame.grade.contrast,
            saturation: ctx.frame.grade.saturation,
            vignette: ctx.frame.grade.vignette,
            bloom_intensity: ctx.frame.grade.bloom_intensity,
            bloom_threshold: ctx.frame.grade.bloom_threshold,
            chromatic_aberration: ctx.frame.grade.chromatic_aberration,
            film_grain: ctx.frame.grade.film_grain,
            temperature: ctx.frame.grade.temperature,
            tint: ctx.frame.grade.tint,
            near_plane: near,
            far_plane: far,
            dof_focus_distance: ctx.frame.grade.dof_focus_distance,
            dof_aperture,
            dof_max_blur: ctx.frame.grade.dof_max_blur,
            lut_strength: if ctx.frame.debug_rt == 0 {
                ctx.frame.grade.lut_strength
            } else {
                0.0
            },
            lut_size: self.lut_size as f32,
            motion_blur,
            motion_blur_max_pixels: ctx.frame.grade.motion_blur_max_pixels,
            inv_texel_size: [
                if ctx.render_extent.width > 0 {
                    1.0 / ctx.render_extent.width as f32
                } else {
                    0.0
                },
                if ctx.render_extent.height > 0 {
                    1.0 / ctx.render_extent.height as f32
                } else {
                    0.0
                },
            ],
        };
        let exec_result = self.execute(
            ctx.device,
            ctx.cmd,
            ctx.frame_index,
            ctx.image_index,
            input_image,
            dest,
            &push,
        );
        if let Some(&img) = ctx.frame.swapchain_images.get(ctx.image_index as usize) {
            resources.set_image(FINAL_COLOR_H, img);
            if let Some(&view) = ctx.frame.swapchain_views.get(ctx.image_index as usize) {
                resources.set_image_view(FINAL_COLOR_H, view);
            }
        }
        exec_result
    }

    fn graph_info(&self) -> PassInfo {
        PassInfo {
            index: usize::MAX,
            name: self.name().to_string(),
            kind: PassKind::Post,
            inputs: vec![FORWARD_COLOR_H],
            outputs: vec![FINAL_COLOR_H],
        }
    }

    fn warmup(&mut self, device: &ash::Device, _context: &VulkanContext) -> anyhow::Result<()> {
        if self.pipeline.is_some() {
            return Ok(());
        }
        if self.render_pass.is_none() {
            self.render_pass = Some(create_render_pass(
                device,
                self.color_format,
                self.final_layout,
            )?);
        }
        self.ensure_pipeline(device)
    }
}

/// Create PostPass render pass with 1 color-format color attachment
/// (CLEAR -> STORE), no depth. `initial_layout = UNDEFINED` so the GPU
/// transitions target image from whatever layout it was in (typically
/// `PRESENT_SRC_KHR` from last  frame  into COLOR_ATTACHMENT_OPTIMAL as part of
/// the load  op.
///
/// `final_layout` is supplied by the caller: `PRESENT_SRC_KHR` for a swapchain
/// image, `COLOR_ATTACHMENT_OPTIMAL` for an offscreen image. Headless contexts
/// do not enable `VK_KHR_swapchain`, so the swapchain layout would be an
/// invalid enum value there (`VUID-VkAttachmentDescription-finalLayout-parameter`).
fn create_render_pass(
    device: &ash::Device,
    format: vk::Format,
    final_layout: vk::ImageLayout,
) -> anyhow::Result<vk::RenderPass> {
    let color_attachment = vk::AttachmentDescription::default()
        .format(format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(final_layout);

    let color_ref = vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

    let subpass = vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(std::slice::from_ref(&color_ref));

    let dependency = vk::SubpassDependency::default()
        .src_subpass(vk::SUBPASS_EXTERNAL)
        .dst_subpass(0)
        .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .src_access_mask(vk::AccessFlags::empty())
        .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);

    let rp_create_info = vk::RenderPassCreateInfo::default()
        .attachments(std::slice::from_ref(&color_attachment))
        .subpasses(std::slice::from_ref(&subpass))
        .dependencies(std::slice::from_ref(&dependency));

    let rp = unsafe { device.create_render_pass(&rp_create_info, None) }
        .context("PostPass: create render pass")?;
    Ok(rp)
}

fn _memory_type_for_hdr(context: &VulkanContext, mem_type_bits: u32) -> anyhow::Result<u32> {
    find_memory_type(
        &context.physical_device_memory_properties,
        mem_type_bits,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    )
    .context("PostPass: no suitable memory type for HDR image")
}

/// Anti-aliasing mode actually handed to the shader.
///
/// The debug render-target viewer (`debug_rt` 1 = depth, 2 = view-space normal)
/// shows a diagnostic image: FXAA would blend neighbouring texels and hide
/// exactly the stair-stepping those views exist to expose, so it is forced off
/// there.
pub(crate) fn effective_aa_mode(debug_rt: u32, aa_mode: u32) -> u32 {
    if debug_rt == 0 {
        aa_mode
    } else {
        0
    }
}

/// Helper to select the input resource handle and expected image layout for PostPass.
///
/// In path-trace mode we read PT_COLOR_H instead of FORWARD_COLOR_H, unless no usable camera
/// exists or PT_COLOR_H was not published (e.g. acceleration structures unsupported, or PT pass
/// skipped). In that case, falls back to FORWARD_COLOR_H so PostPass can tonemap and transition
/// the presentation image properly.
pub(crate) fn select_input_target(
    debug_rt: u32,
    is_pt: bool,
    has_camera: bool,
    has_pt_view: bool,
) -> (ResourceHandle, vk::ImageLayout) {
    match debug_rt {
        1 => (
            FORWARD_DEPTH_H,
            vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
        ),
        2 => (FORWARD_NORMAL_H, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
        _ if is_pt && has_camera && has_pt_view => {
            (PT_COLOR_H, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        }
        _ if is_pt && has_camera => {
            log::warn!(
                "PostPass: PT_COLOR_H requested but not published; falling back to FORWARD_COLOR_H"
            );
            (FORWARD_COLOR_H, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        }
        _ => (FORWARD_COLOR_H, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
    }
}

/// Create a sampled 3D image and upload `lut` into it.
///
/// The image is left in `SHADER_READ_ONLY_OPTIMAL`. `R32G32B32A32_SFLOAT`
/// keeps the interpolation exact enough that an identity LUT is a no-op up to
/// 8-bit display quantization; a 32³ LUT costs 512 KiB, which is negligible
/// next to the HDR targets.
fn create_lut_image(
    context: &VulkanContext,
    lut: &Lut3d,
) -> anyhow::Result<(vk::Image, vk::DeviceMemory, vk::ImageView)> {
    let device = &context.device;
    let n = lut.size.max(2);
    debug_assert_eq!(lut.data.len(), (n * n * n) as usize);

    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_3D)
        .format(vk::Format::R32G32B32A32_SFLOAT)
        .extent(vk::Extent3D {
            width: n,
            height: n,
            depth: n,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    let image = unsafe { device.create_image(&image_info, None) }.context("create LUT image")?;

    let requirements = unsafe { device.get_image_memory_requirements(image) };
    let memory_type = find_memory_type(
        &context.physical_device_memory_properties,
        requirements.memory_type_bits,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    )
    .context("find LUT image memory type")?;
    let memory = match unsafe {
        device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(memory_type),
            None,
        )
    } {
        Ok(memory) => memory,
        Err(error) => {
            unsafe { device.destroy_image(image, None) };
            return Err(error).context("allocate LUT image memory");
        }
    };
    if let Err(error) = unsafe { device.bind_image_memory(image, memory, 0) } {
        unsafe {
            device.destroy_image(image, None);
            device.free_memory(memory, None);
        }
        return Err(error).context("bind LUT image memory");
    }

    let view = match unsafe {
        device.create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_3D)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .base_mip_level(0)
                        .level_count(1)
                        .base_array_layer(0)
                        .layer_count(1),
                ),
            None,
        )
    } {
        Ok(view) => view,
        Err(error) => {
            unsafe {
                device.destroy_image(image, None);
                device.free_memory(memory, None);
            }
            return Err(error).context("create LUT image view");
        }
    };

    let pixels = lut.to_rgba_f32();
    let byte_len = (pixels.len() * std::mem::size_of::<f32>()) as vk::DeviceSize;
    let (staging, staging_memory) = create_buffer(
        context,
        byte_len,
        vk::BufferUsageFlags::TRANSFER_SRC,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    )
    .context("create LUT staging buffer")?;
    let ptr =
        unsafe { device.map_memory(staging_memory, 0, byte_len, vk::MemoryMapFlags::empty()) }
            .context("map LUT staging buffer")?;
    unsafe {
        std::ptr::copy_nonoverlapping(
            pixels.as_ptr() as *const u8,
            ptr as *mut u8,
            byte_len as usize,
        );
        device.unmap_memory(staging_memory);
    }

    let pool = unsafe {
        device.create_command_pool(
            &vk::CommandPoolCreateInfo::default()
                .flags(vk::CommandPoolCreateFlags::TRANSIENT)
                .queue_family_index(context.graphics_queue_family),
            None,
        )
    }
    .context("create LUT upload command pool")?;
    let cmd = unsafe {
        device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1),
        )
    }
    .context("allocate LUT upload command buffer")?[0];

    let range = vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .base_mip_level(0)
        .level_count(1)
        .base_array_layer(0)
        .layer_count(1);
    let to_transfer = vk::ImageMemoryBarrier::default()
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .src_access_mask(vk::AccessFlags::empty())
        .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
        .image(image)
        .subresource_range(range);
    let to_shader = vk::ImageMemoryBarrier::default()
        .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
        .dst_access_mask(vk::AccessFlags::SHADER_READ)
        .image(image)
        .subresource_range(range);
    let copy = vk::BufferImageCopy::default()
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
            width: n,
            height: n,
            depth: n,
        });

    let record = unsafe {
        device.begin_command_buffer(
            cmd,
            &vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
        )?;
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_transfer],
        );
        device.cmd_copy_buffer_to_image(
            cmd,
            staging,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &[copy],
        );
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_shader],
        );
        device.end_command_buffer(cmd)
    };
    if let Err(error) = record {
        unsafe {
            device.free_command_buffers(pool, &[cmd]);
            device.destroy_command_pool(pool, None);
            device.destroy_buffer(staging, None);
            device.free_memory(staging_memory, None);
            device.destroy_image_view(view, None);
            device.destroy_image(image, None);
            device.free_memory(memory, None);
        }
        return Err(error).context("record LUT upload");
    }

    let fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }
        .context("create LUT upload fence")?;
    let submit = vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&cmd));
    let submitted = unsafe { device.queue_submit(context.graphics_queue, &[submit], fence) };
    if let Err(error) = submitted {
        unsafe {
            device.destroy_fence(fence, None);
            device.free_command_buffers(pool, &[cmd]);
            device.destroy_command_pool(pool, None);
            device.destroy_buffer(staging, None);
            device.free_memory(staging_memory, None);
            device.destroy_image_view(view, None);
            device.destroy_image(image, None);
            device.free_memory(memory, None);
        }
        return Err(error).context("submit LUT upload");
    }
    let waited = unsafe { device.wait_for_fences(&[fence], true, u64::MAX) };
    unsafe {
        device.destroy_fence(fence, None);
        device.free_command_buffers(pool, &[cmd]);
        device.destroy_command_pool(pool, None);
        device.destroy_buffer(staging, None);
        device.free_memory(staging_memory, None);
    }
    waited.context("wait for LUT upload")?;

    Ok((image, memory, view))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_input_target_raster() {
        let (handle, layout) = select_input_target(0, false, true, false);
        assert_eq!(handle, FORWARD_COLOR_H);
        assert_eq!(layout, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
    }

    #[test]
    fn test_select_input_target_path_trace_with_pt_view() {
        let (handle, layout) = select_input_target(0, true, true, true);
        assert_eq!(handle, PT_COLOR_H);
        assert_eq!(layout, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
    }

    #[test]
    fn test_select_input_target_path_trace_fallback_when_view_missing() {
        let (handle, layout) = select_input_target(0, true, true, false);
        assert_eq!(handle, FORWARD_COLOR_H);
        assert_eq!(layout, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
    }

    #[test]
    fn test_select_input_target_path_trace_no_camera() {
        let (handle, layout) = select_input_target(0, true, false, false);
        assert_eq!(handle, FORWARD_COLOR_H);
        assert_eq!(layout, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
    }

    #[test]
    fn test_effective_aa_mode_is_off_for_debug_views() {
        assert_eq!(effective_aa_mode(0, 0), 0);
        assert_eq!(effective_aa_mode(0, 1), 1);
        assert_eq!(effective_aa_mode(1, 1), 0);
        assert_eq!(effective_aa_mode(2, 1), 0);
    }

    #[test]
    fn test_post_push_layout_matches_the_shader() {
        use shader_bindings::post::{PostPush, PUSH_CONSTANT_SIZE};
        assert_eq!(std::mem::offset_of!(PostPush, tonemap_mode), 0);
        assert_eq!(std::mem::offset_of!(PostPush, aa_mode), 4);
        assert_eq!(std::mem::offset_of!(PostPush, inv_texel_size), 8);
        assert_eq!(std::mem::offset_of!(PostPush, contrast), 16);
        assert_eq!(std::mem::offset_of!(PostPush, saturation), 20);
        assert_eq!(std::mem::offset_of!(PostPush, vignette), 24);
        assert_eq!(std::mem::offset_of!(PostPush, bloom_intensity), 28);
        assert_eq!(std::mem::offset_of!(PostPush, bloom_threshold), 32);
        assert_eq!(std::mem::offset_of!(PostPush, chromatic_aberration), 36);
        assert_eq!(std::mem::offset_of!(PostPush, film_grain), 40);
        assert_eq!(std::mem::offset_of!(PostPush, temperature), 44);
        assert_eq!(std::mem::offset_of!(PostPush, tint), 48);
        assert_eq!(std::mem::offset_of!(PostPush, near_plane), 52);
        assert_eq!(std::mem::offset_of!(PostPush, far_plane), 56);
        assert_eq!(std::mem::offset_of!(PostPush, dof_focus_distance), 60);
        assert_eq!(std::mem::offset_of!(PostPush, dof_aperture), 64);
        assert_eq!(std::mem::offset_of!(PostPush, dof_max_blur), 68);
        assert_eq!(std::mem::offset_of!(PostPush, lut_strength), 72);
        assert_eq!(std::mem::offset_of!(PostPush, lut_size), 76);
        assert_eq!(std::mem::offset_of!(PostPush, motion_blur), 80);
        assert_eq!(std::mem::offset_of!(PostPush, motion_blur_max_pixels), 84);
        assert_eq!(std::mem::size_of::<PostPush>(), 88);
        assert_eq!(PUSH_CONSTANT_SIZE, 88);
    }

    #[test]
    fn test_motion_blur_is_clamped_and_off_by_default() {
        use crate::render_graph::PostGrade;
        assert!(PostGrade::default().is_neutral());
        assert_eq!(PostGrade::default().motion_blur, 0.0);
        assert_eq!(std::mem::size_of::<MotionBlurUniform>(), 144);

        let clamped = PostGrade {
            motion_blur: 2.0,
            motion_blur_max_pixels: 100.0,
            ..PostGrade::default()
        }
        .sanitized();
        assert_eq!(clamped.motion_blur, 1.0);
        assert_eq!(clamped.motion_blur_max_pixels, 64.0);

        let broken = PostGrade {
            motion_blur: f32::NAN,
            motion_blur_max_pixels: f32::NEG_INFINITY,
            ..PostGrade::default()
        }
        .sanitized();
        assert_eq!(broken.motion_blur, 0.0);
        assert_eq!(broken.motion_blur_max_pixels, 16.0);
    }

    #[test]
    fn test_depth_of_field_is_clamped_and_off_by_default() {
        use crate::render_graph::PostGrade;
        assert!(PostGrade::default().is_neutral());
        assert_eq!(PostGrade::default().dof_aperture, 0.0);

        let clamped = PostGrade {
            dof_focus_distance: -4.0,
            dof_aperture: 2.0,
            dof_max_blur: 100.0,
            ..PostGrade::default()
        }
        .sanitized();
        assert_eq!(clamped.dof_focus_distance, 0.001);
        assert_eq!(clamped.dof_aperture, 1.0);
        assert_eq!(clamped.dof_max_blur, 32.0);

        let broken = PostGrade {
            dof_focus_distance: f32::NAN,
            dof_aperture: f32::NAN,
            dof_max_blur: f32::INFINITY,
            ..PostGrade::default()
        }
        .sanitized();
        assert_eq!(broken.dof_focus_distance, 1.0);
        assert_eq!(broken.dof_aperture, 0.0);
        assert_eq!(broken.dof_max_blur, 8.0);
    }

    #[test]
    fn test_white_balance_is_clamped_and_neutral_at_zero() {
        use crate::render_graph::PostGrade;
        assert!(PostGrade::default().is_neutral());
        let clamped = PostGrade {
            temperature: 2.0,
            tint: -3.0,
            ..PostGrade::default()
        }
        .sanitized();
        assert_eq!(clamped.temperature, 1.0);
        assert_eq!(clamped.tint, -1.0);

        let broken = PostGrade {
            temperature: f32::NAN,
            tint: f32::INFINITY,
            ..PostGrade::default()
        }
        .sanitized();
        assert_eq!(broken.temperature, 0.0);
        assert_eq!(broken.tint, 0.0);
    }

    #[test]
    fn test_chromatic_aberration_is_clamped_and_neutral_at_zero() {
        use crate::render_graph::PostGrade;
        assert!(PostGrade::default().is_neutral());
        assert_eq!(
            PostGrade {
                chromatic_aberration: 2.0,
                ..PostGrade::default()
            }
            .sanitized()
            .chromatic_aberration,
            1.0
        );
        assert_eq!(
            PostGrade {
                chromatic_aberration: -1.0,
                ..PostGrade::default()
            }
            .sanitized()
            .chromatic_aberration,
            0.0
        );
        assert_eq!(
            PostGrade {
                chromatic_aberration: f32::NAN,
                ..PostGrade::default()
            }
            .sanitized()
            .chromatic_aberration,
            0.0
        );
    }

    #[test]
    fn test_film_grain_is_clamped_and_neutral_at_zero() {
        use crate::render_graph::PostGrade;
        assert!(PostGrade::default().is_neutral());
        assert_eq!(
            PostGrade {
                film_grain: 2.0,
                ..PostGrade::default()
            }
            .sanitized()
            .film_grain,
            1.0
        );
        assert_eq!(
            PostGrade {
                film_grain: -1.0,
                ..PostGrade::default()
            }
            .sanitized()
            .film_grain,
            0.0
        );
        assert_eq!(
            PostGrade {
                film_grain: f32::NAN,
                ..PostGrade::default()
            }
            .sanitized()
            .film_grain,
            0.0
        );
    }

    #[test]
    fn test_select_input_target_debug_modes() {
        let (h1, l1) = select_input_target(1, true, true, true);
        assert_eq!(h1, FORWARD_DEPTH_H);
        assert_eq!(l1, vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL);

        let (h2, l2) = select_input_target(2, true, true, true);
        assert_eq!(h2, FORWARD_NORMAL_H);
        assert_eq!(l2, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
    }
}
