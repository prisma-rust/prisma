//! RTXGI v1.3 DDGI Vulkan compute pass (`ddgi_clear`, `ddgi_update_irradiance`,
//! `ddgi_update_distance`, `ddgi_blend`, `ddgi_resolve`).
//!
//! Consumes [`DdgiVolumeParams`] and [`DdgiAtlasLayout`] from [`crate::ddgi`],
//! traces `VK_KHR_ray_query` rays against the active TLAS, and resolves the
//! octahedral irradiance atlas into the engine's producer-agnostic 3D SH9
//! `probeVolume` texture consumed by `scene_frag.slang`.

use std::ffi::CStr;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use ash::vk;

use crate::buffer::{create_buffer, find_memory_type};
use crate::compute::ComputePipeline;
use crate::context::VulkanContext;
use crate::ddgi::{
    DdgiAtlasLayout, DdgiScrollingState, DdgiVariabilityState, DdgiVolumeParams,
    DDGI_DISTANCE_TEXELS,
};
use crate::shader::load_shader_module;
use crate::shader_bindings::ddgi::{self as bindings, DdgiVolumeUbo};

const ENTRY_CLEAR: &CStr = c"ddgiClearMain";
const ENTRY_UPDATE_IRRADIANCE: &CStr = c"ddgiUpdateIrradianceMain";
const ENTRY_UPDATE_DISTANCE: &CStr = c"ddgiUpdateDistanceMain";
const ENTRY_BLEND: &CStr = c"ddgiBlendMain";
const ENTRY_RESOLVE: &CStr = c"ddgiResolveMain";

pub const DDGI_FLAG_RELOCATION: u32 = 1 << 0;
pub const DDGI_FLAG_CLASSIFICATION: u32 = 1 << 1;
pub const DDGI_FLAG_SCROLLING: u32 = 1 << 2;
pub const DDGI_FLAG_VARIABILITY: u32 = 1 << 3;
pub const DDGI_FLAG_CLEAR_ALL: u32 = 1 << 4;

struct ImageArrayGpu {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    layout: vk::ImageLayout,
}

impl ImageArrayGpu {
    fn create(
        context: &VulkanContext,
        width: u32,
        height: u32,
        layers: u32,
        format: vk::Format,
    ) -> Result<Self> {
        let device = &context.device;
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width: width.max(1),
                height: height.max(1),
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(layers.max(1))
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::SAMPLED)
            .initial_layout(vk::ImageLayout::UNDEFINED);

        let image = unsafe { device.create_image(&info, None) }
            .context("DdgiPass: create Texture2DArray image")?;
        let reqs = unsafe { device.get_image_memory_requirements(image) };
        let mem_type = find_memory_type(
            context,
            reqs.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .context("DdgiPass: no DEVICE_LOCAL memory for atlas")?;
        let alloc = vk::MemoryAllocateInfo::default()
            .allocation_size(reqs.size)
            .memory_type_index(mem_type);
        let memory = unsafe { device.allocate_memory(&alloc, None) }
            .context("DdgiPass: allocate atlas memory")?;
        unsafe { device.bind_image_memory(image, memory, 0) }
            .context("DdgiPass: bind atlas memory")?;

        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D_ARRAY)
            .format(format)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: layers.max(1),
            });
        let view = unsafe { device.create_image_view(&view_info, None) }
            .context("DdgiPass: create Texture2DArray view")?;

        Ok(Self {
            image,
            memory,
            view,
            layout: vk::ImageLayout::UNDEFINED,
        })
    }

    fn destroy(&mut self, device: &ash::Device) {
        unsafe {
            if self.view != vk::ImageView::null() {
                device.destroy_image_view(self.view, None);
                self.view = vk::ImageView::null();
            }
            if self.image != vk::Image::null() {
                device.destroy_image(self.image, None);
                self.image = vk::Image::null();
            }
            if self.memory != vk::DeviceMemory::null() {
                device.free_memory(self.memory, None);
                self.memory = vk::DeviceMemory::null();
            }
        }
    }
}

struct BufferGpu {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
}

impl BufferGpu {
    fn create(
        context: &VulkanContext,
        size: vk::DeviceSize,
        usage: vk::BufferUsageFlags,
        props: vk::MemoryPropertyFlags,
    ) -> Result<Self> {
        let size = size.max(16);
        let (buffer, memory) =
            create_buffer(context, size, usage, props).context("DdgiPass: create buffer")?;
        Ok(Self { buffer, memory })
    }

    fn write(&self, device: &ash::Device, bytes: &[u8]) -> Result<()> {
        unsafe {
            let ptr = device
                .map_memory(
                    self.memory,
                    0,
                    bytes.len() as vk::DeviceSize,
                    vk::MemoryMapFlags::empty(),
                )
                .context("DdgiPass: map buffer")?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
            device.unmap_memory(self.memory);
        }
        Ok(())
    }

    fn read_u32(&self, device: &ash::Device) -> Result<u32> {
        let mut val = 0u32;
        unsafe {
            let ptr = device
                .map_memory(self.memory, 0, 4, vk::MemoryMapFlags::empty())
                .context("DdgiPass: map variability buffer")?;
            std::ptr::copy_nonoverlapping(ptr as *const u8, &mut val as *mut u32 as *mut u8, 4);
            device.unmap_memory(self.memory);
        }
        Ok(val)
    }

    fn destroy(&mut self, device: &ash::Device) {
        unsafe {
            if self.buffer != vk::Buffer::null() {
                device.destroy_buffer(self.buffer, None);
                self.buffer = vk::Buffer::null();
            }
            if self.memory != vk::DeviceMemory::null() {
                device.free_memory(self.memory, None);
                self.memory = vk::DeviceMemory::null();
            }
        }
    }
}

/// Per-frame scene inputs required to execute [`DdgiPass::record_dispatch`].
pub struct DdgiDispatchInputs {
    pub tlas: vk::AccelerationStructureKHR,
    pub vertex_buffer: vk::Buffer,
    pub index_buffer: vk::Buffer,
    pub instance_meta_buffer: vk::Buffer,
    pub materials_buffer: vk::Buffer,
    pub lights_buffer: vk::Buffer,
    pub emissive_buffer: vk::Buffer,
    pub probe_volume_out_view: vk::ImageView,
    pub bindless_set: vk::DescriptorSet,
    pub ibl_set: vk::DescriptorSet,
    pub target_world_pos: [f32; 3],
    pub light_dir: [f32; 3],
    pub light_color: [f32; 3],
    pub num_lights: u32,
    pub num_emissive: u32,
    pub frame_index: u32,
}

/// Compute dispatch dimensions for each DDGI stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DdgiDispatchGrid {
    pub clear_groups: [u32; 3],
    pub irradiance_groups: [u32; 3],
    pub distance_groups: [u32; 3],
    pub blend_groups: [u32; 3],
    pub resolve_groups: [u32; 3],
}

impl DdgiDispatchGrid {
    /// Compute workgroup counts matching `ddgi.slang` `[numthreads]` declarations.
    #[must_use]
    pub const fn from_params(params: &DdgiVolumeParams) -> Self {
        let [nx, ny, nz] = params.probe_counts;
        let total_probes = nx * ny * nz;
        Self {
            clear_groups: [nx.div_ceil(4), ny.div_ceil(4), nz],
            irradiance_groups: [total_probes, 1, 1],
            distance_groups: [
                (nx * DDGI_DISTANCE_TEXELS).div_ceil(8),
                (nz * DDGI_DISTANCE_TEXELS).div_ceil(8),
                ny,
            ],
            blend_groups: [nx, nz, ny],
            resolve_groups: [nx.div_ceil(4), ny.div_ceil(4), nz],
        }
    }
}

/// Encode [`DdgiVolumeParams`] feature flags into the `ddgi.slang` bitmask.
#[must_use]
pub const fn encode_ddgi_flags(params: &DdgiVolumeParams, clear_all: bool) -> u32 {
    let mut flags = 0u32;
    if params.relocation_enabled {
        flags |= DDGI_FLAG_RELOCATION;
    }
    if params.classification_enabled {
        flags |= DDGI_FLAG_CLASSIFICATION;
    }
    if params.scrolling_enabled {
        flags |= DDGI_FLAG_SCROLLING;
    }
    if params.variability_enabled {
        flags |= DDGI_FLAG_VARIABILITY;
    }
    if clear_all {
        flags |= DDGI_FLAG_CLEAR_ALL;
    }
    flags
}

/// Deterministic per-frame unit quaternion for rotating Fibonacci sphere rays.
#[must_use]
pub fn frame_ray_rotation_quat(frame_index: u32) -> [f32; 4] {
    let a = (frame_index as f32) * 0.618_034;
    let b = (frame_index as f32) * 0.381_966;
    let axis = [
        (a * std::f32::consts::TAU).cos() * (b * std::f32::consts::PI).sin(),
        (b * std::f32::consts::PI).cos(),
        (a * std::f32::consts::TAU).sin() * (b * std::f32::consts::PI).sin(),
    ];
    let len = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2])
        .sqrt()
        .max(1e-6);
    let half = (frame_index as f32 * 0.754_877_7) * 0.5;
    let s = half.sin() / len;
    [axis[0] * s, axis[1] * s, axis[2] * s, half.cos()]
}

/// Manages GPU resources and the 5 compute pipelines for RTXGI v1.3 DDGI.
pub struct DdgiPass {
    context: Arc<VulkanContext>,
    set0_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    pipeline_clear: ComputePipeline,
    pipeline_update_irradiance: ComputePipeline,
    pipeline_update_distance: ComputePipeline,
    pipeline_blend: ComputePipeline,
    pipeline_resolve: ComputePipeline,
    params: DdgiVolumeParams,
    irradiance_atlas: ImageArrayGpu,
    distance_atlas: ImageArrayGpu,
    probe_metadata: ImageArrayGpu,
    ray_buffer: BufferGpu,
    variability_buffer: BufferGpu,
    ubo_buffer: BufferGpu,
    dummy_lights: BufferGpu,
    dummy_emissive: BufferGpu,
    scene: Option<crate::bake_common::PtScene>,
    scrolling: DdgiScrollingState,
    variability: DdgiVariabilityState,
    needs_clear: bool,
}

impl DdgiPass {
    /// Allocate DDGI atlases, ray/metadata buffers, and all 5 compute pipelines.
    pub fn new(
        context: Arc<VulkanContext>,
        bindless_layout: vk::DescriptorSetLayout,
        ibl_layout: vk::DescriptorSetLayout,
        params: DdgiVolumeParams,
    ) -> Result<Self> {
        let device = &context.device;
        let layout_bindings = [
            vk::DescriptorSetLayoutBinding {
                binding: bindings::DDGI_UBO_BINDING,
                descriptor_type: vk::DescriptorType::UNIFORM_BUFFER,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::COMPUTE,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: bindings::IRRADIANCE_ATLAS_BINDING,
                descriptor_type: vk::DescriptorType::STORAGE_IMAGE,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::COMPUTE,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: bindings::TLAS_BINDING,
                descriptor_type: vk::DescriptorType::ACCELERATION_STRUCTURE_KHR,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::COMPUTE,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: bindings::VERTEX_DATA_BINDING,
                descriptor_type: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::COMPUTE,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: bindings::INDICES_BINDING,
                descriptor_type: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::COMPUTE,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: bindings::DISTANCE_ATLAS_BINDING,
                descriptor_type: vk::DescriptorType::STORAGE_IMAGE,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::COMPUTE,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: bindings::INSTANCE_META_BINDING,
                descriptor_type: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::COMPUTE,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: bindings::MATERIALS_BINDING,
                descriptor_type: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::COMPUTE,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: bindings::PT_LIGHTS_BINDING,
                descriptor_type: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::COMPUTE,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: bindings::PT_EMISSIVE_BINDING,
                descriptor_type: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::COMPUTE,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: bindings::PROBE_METADATA_BINDING,
                descriptor_type: vk::DescriptorType::STORAGE_IMAGE,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::COMPUTE,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: bindings::RAY_BUFFER_BINDING,
                descriptor_type: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::COMPUTE,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: bindings::PROBE_VOLUME_OUT_BINDING,
                descriptor_type: vk::DescriptorType::STORAGE_IMAGE,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::COMPUTE,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: bindings::VARIABILITY_ACCUM_BINDING,
                descriptor_type: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::COMPUTE,
                ..Default::default()
            },
        ];

        let set0_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&layout_bindings),
                None,
            )
        }
        .context("DdgiPass: create descriptor layout")?;

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UNIFORM_BUFFER,
                descriptor_count: 1,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_IMAGE,
                descriptor_count: 4,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::ACCELERATION_STRUCTURE_KHR,
                descriptor_count: 1,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: 8,
            },
        ];
        let descriptor_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&pool_sizes),
                None,
            )
        }
        .context("DdgiPass: create descriptor pool")?;

        let descriptor_set = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(std::slice::from_ref(&set0_layout)),
            )
        }
        .context("DdgiPass: allocate descriptor set")?[0];

        let set_layouts = [set0_layout, bindless_layout, ibl_layout];
        let build_pipe = |spv: &[u8], entry: &CStr| -> Result<ComputePipeline> {
            let module = load_shader_module(device, spv)?;
            let res = ComputePipeline::new(device, module, entry, &set_layouts, &[]);
            unsafe { device.destroy_shader_module(module, None) };
            res
        };

        let pipeline_clear = build_pipe(prism_render_shaders::DDGI_CLEAR_COMP_SPV, ENTRY_CLEAR)?;
        let pipeline_update_irradiance = build_pipe(
            prism_render_shaders::DDGI_UPDATE_IRRADIANCE_COMP_SPV,
            ENTRY_UPDATE_IRRADIANCE,
        )?;
        let pipeline_update_distance = build_pipe(
            prism_render_shaders::DDGI_UPDATE_DISTANCE_COMP_SPV,
            ENTRY_UPDATE_DISTANCE,
        )?;
        let pipeline_blend = build_pipe(prism_render_shaders::DDGI_BLEND_COMP_SPV, ENTRY_BLEND)?;
        let pipeline_resolve =
            build_pipe(prism_render_shaders::DDGI_RESOLVE_COMP_SPV, ENTRY_RESOLVE)?;

        let atlas_layout = DdgiAtlasLayout::from_probe_counts(params.probe_counts);
        let irradiance_atlas = ImageArrayGpu::create(
            &context,
            atlas_layout.irradiance_extent[0],
            atlas_layout.irradiance_extent[1],
            atlas_layout.array_layers,
            vk::Format::R16G16B16A16_SFLOAT,
        )?;
        let distance_atlas = ImageArrayGpu::create(
            &context,
            atlas_layout.distance_extent[0],
            atlas_layout.distance_extent[1],
            atlas_layout.array_layers,
            vk::Format::R16G16_SFLOAT,
        )?;
        let probe_metadata = ImageArrayGpu::create(
            &context,
            params.probe_counts[0],
            params.probe_counts[2],
            params.probe_counts[1],
            vk::Format::R16G16B16A16_SFLOAT,
        )?;

        let ray_bytes =
            (params.total_probes() as u64) * (params.aligned_rays_per_probe() as u64) * 16;
        let ray_buffer = BufferGpu::create(
            &context,
            ray_bytes,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )?;
        let variability_buffer = BufferGpu::create(
            &context,
            4,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;
        let ubo_buffer = BufferGpu::create(
            &context,
            std::mem::size_of::<DdgiVolumeUbo>() as vk::DeviceSize,
            vk::BufferUsageFlags::UNIFORM_BUFFER,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;
        let dummy_lights = BufferGpu::create(
            &context,
            64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )?;
        let dummy_emissive = BufferGpu::create(
            &context,
            64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )?;

        let scrolling = DdgiScrollingState::new(params.origin, params.probe_spacing);
        Ok(Self {
            context,
            set0_layout,
            descriptor_pool,
            descriptor_set,
            pipeline_clear,
            pipeline_update_irradiance,
            pipeline_update_distance,
            pipeline_blend,
            pipeline_resolve,
            params,
            irradiance_atlas,
            distance_atlas,
            probe_metadata,
            ray_buffer,
            variability_buffer,
            ubo_buffer,
            dummy_lights,
            dummy_emissive,
            scene: None,
            scrolling,
            variability: DdgiVariabilityState::default(),
            needs_clear: true,
        })
    }

    /// Force a full atlas clear on the next dispatch (e.g. after scene reload).
    pub fn invalidate(&mut self) {
        self.needs_clear = true;
        self.variability.wake();
    }

    /// Whether a TLAS + geometry buffers are currently built for DDGI tracing.
    #[must_use]
    pub fn has_geometry(&self) -> bool {
        self.scene.as_ref().is_some_and(|s| s.tlas.is_some())
    }

    /// Drop cached acceleration structures so the next frame rebuilds them.
    pub fn invalidate_geometry(&mut self) {
        if let Some(mut scene) = self.scene.take() {
            if let Some(device) = scene.device.take() {
                scene.destroy(&device);
            }
        }
        self.invalidate();
    }

    /// Build or rebuild the DDGI ray-tracing scene from world-space instances.
    pub fn set_geometry(
        &mut self,
        command_pool: vk::CommandPool,
        instances: &[crate::bake_common::PtGeometryInstance],
    ) -> Result<()> {
        self.invalidate_geometry();
        if instances.is_empty() {
            return Ok(());
        }
        let materials = vec![0u8; std::mem::size_of::<crate::managers::GpuMaterial>()];
        let scene =
            crate::bake_common::build_pt_scene(&self.context, command_pool, instances, &materials)
                .context("DdgiPass: build ray-tracing scene")?;
        self.scene = Some(scene);
        Ok(())
    }

    /// Volume parameters and active origin (accounts for infinite scrolling).
    #[must_use]
    pub const fn params(&self) -> &DdgiVolumeParams {
        &self.params
    }

    /// Current origin (snapped if scrolling is enabled).
    #[must_use]
    pub const fn active_origin(&self) -> [f32; 3] {
        if self.params.scrolling_enabled {
            self.scrolling.snapped_origin
        } else {
            self.params.origin
        }
    }

    /// Dispatch DDGI using the internally cached [`PtScene`] and shared material/bindless sets.
    #[expect(
        clippy::too_many_arguments,
        reason = "dispatches DDGI with scene, material, bindless, IBL, and lighting uniforms"
    )]
    pub fn dispatch_scene(
        &mut self,
        cmd: vk::CommandBuffer,
        probe_volume_out_view: vk::ImageView,
        materials_buffer: vk::Buffer,
        bindless_set: vk::DescriptorSet,
        ibl_set: vk::DescriptorSet,
        target_world_pos: [f32; 3],
        light_dir: [f32; 3],
        light_color: [f32; 3],
        frame_index: u32,
    ) -> Result<bool> {
        let Some(scene) = self.scene.as_ref() else {
            return Ok(false);
        };
        let Some(tlas) = scene.tlas.as_ref() else {
            return Ok(false);
        };
        let inputs = DdgiDispatchInputs {
            tlas: tlas.handle,
            vertex_buffer: scene.vertex_buffer,
            index_buffer: scene.index_buffer,
            instance_meta_buffer: scene.instance_meta_buffer,
            materials_buffer,
            lights_buffer: self.dummy_lights.buffer,
            emissive_buffer: self.dummy_emissive.buffer,
            probe_volume_out_view,
            bindless_set,
            ibl_set,
            target_world_pos,
            light_dir,
            light_color,
            num_lights: 0,
            num_emissive: 0,
            frame_index,
        };
        self.record_dispatch(cmd, &inputs)?;
        Ok(true)
    }

    /// Current DDGI variability sleep state.
    #[must_use]
    pub const fn is_asleep(&self) -> bool {
        self.variability.asleep
    }

    /// Record all 5 DDGI compute passes into `cmd` and resolve into `probe_volume_out_view`.
    pub fn record_dispatch(
        &mut self,
        cmd: vk::CommandBuffer,
        inputs: &DdgiDispatchInputs,
    ) -> Result<()> {
        let device = &self.context.device;

        if self.params.variability_enabled && !self.needs_clear {
            let max_var_fixed = self.variability_buffer.read_u32(device)?;
            self.variability
                .record_frame(max_var_fixed as f32 / 65535.0);
            if self.variability.asleep {
                return Ok(());
            }
        }
        self.variability_buffer.write(device, &0u32.to_le_bytes())?;

        let scroll_delta = if self.params.scrolling_enabled {
            self.scrolling.update(
                inputs.target_world_pos,
                self.params.probe_spacing,
                self.params.probe_counts,
            )
        } else {
            crate::ddgi::DdgiScrollDelta {
                step: [0, 0, 0],
                clear_all: false,
            }
        };

        let clear_all = self.needs_clear || scroll_delta.clear_all;
        let run_clear = clear_all || scroll_delta.step != [0, 0, 0];
        let origin = if self.params.scrolling_enabled {
            self.scrolling.snapped_origin
        } else {
            self.params.origin
        };

        let ubo = DdgiVolumeUbo {
            origin: [
                origin[0],
                origin[1],
                origin[2],
                self.params.probe_max_ray_distance,
            ],
            rotation_xyzw: self.params.rotation_xyzw,
            spacing: [
                self.params.probe_spacing[0],
                self.params.probe_spacing[1],
                self.params.probe_spacing[2],
                self.params.hysteresis,
            ],
            counts: [
                self.params.probe_counts[0],
                self.params.probe_counts[1],
                self.params.probe_counts[2],
                self.params.aligned_rays_per_probe(),
            ],
            scroll_offsets: [
                self.scrolling.scroll_offsets[0],
                self.scrolling.scroll_offsets[1],
                self.scrolling.scroll_offsets[2],
                inputs.frame_index as i32,
            ],
            scroll_delta: [
                scroll_delta.step[0],
                scroll_delta.step[1],
                scroll_delta.step[2],
                encode_ddgi_flags(&self.params, clear_all) as i32,
            ],
            ray_rot_xyzw: frame_ray_rotation_quat(inputs.frame_index),
            light_dir: [
                inputs.light_dir[0],
                inputs.light_dir[1],
                inputs.light_dir[2],
                self.params.normal_bias,
            ],
            light_color: [
                inputs.light_color[0],
                inputs.light_color[1],
                inputs.light_color[2],
                self.params.view_bias,
            ],
            thresholds: [
                self.params.irradiance_threshold,
                self.params.brightness_threshold,
                inputs.num_lights as f32,
                inputs.num_emissive as f32,
            ],
        };
        self.ubo_buffer.write(device, bytemuck::bytes_of(&ubo))?;

        self.write_descriptor_set(inputs);
        self.transition_atlases_to_general(cmd);

        let grid = DdgiDispatchGrid::from_params(&self.params);
        let sets = [self.descriptor_set, inputs.bindless_set, inputs.ibl_set];

        if run_clear {
            self.dispatch_stage(cmd, &self.pipeline_clear, &sets, grid.clear_groups);
            self.compute_barrier(cmd);
        }
        self.dispatch_stage(
            cmd,
            &self.pipeline_update_irradiance,
            &sets,
            grid.irradiance_groups,
        );
        self.compute_barrier(cmd);
        self.dispatch_stage(
            cmd,
            &self.pipeline_update_distance,
            &sets,
            grid.distance_groups,
        );
        self.compute_barrier(cmd);
        self.dispatch_stage(cmd, &self.pipeline_blend, &sets, grid.blend_groups);
        self.compute_barrier(cmd);
        self.dispatch_stage(cmd, &self.pipeline_resolve, &sets, grid.resolve_groups);

        self.needs_clear = false;
        Ok(())
    }

    fn dispatch_stage(
        &self,
        cmd: vk::CommandBuffer,
        pipeline: &ComputePipeline,
        sets: &[vk::DescriptorSet; 3],
        groups: [u32; 3],
    ) {
        unsafe {
            self.context.device.cmd_bind_pipeline(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                pipeline.pipeline,
            );
            self.context.device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                pipeline.layout,
                0,
                sets,
                &[],
            );
            self.context
                .device
                .cmd_dispatch(cmd, groups[0], groups[1], groups[2]);
        }
    }

    fn compute_barrier(&self, cmd: vk::CommandBuffer) {
        let mem = vk::MemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::SHADER_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE);
        unsafe {
            self.context.device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[mem],
                &[],
                &[],
            );
        }
    }

    fn transition_atlases_to_general(&mut self, cmd: vk::CommandBuffer) {
        let mut barriers = Vec::with_capacity(3);
        for atlas in [
            &mut self.irradiance_atlas,
            &mut self.distance_atlas,
            &mut self.probe_metadata,
        ] {
            if atlas.layout == vk::ImageLayout::UNDEFINED {
                barriers.push(
                    vk::ImageMemoryBarrier::default()
                        .old_layout(vk::ImageLayout::UNDEFINED)
                        .new_layout(vk::ImageLayout::GENERAL)
                        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .image(atlas.image)
                        .subresource_range(vk::ImageSubresourceRange {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            base_mip_level: 0,
                            level_count: 1,
                            base_array_layer: 0,
                            layer_count: vk::REMAINING_ARRAY_LAYERS,
                        })
                        .src_access_mask(vk::AccessFlags::empty())
                        .dst_access_mask(
                            vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE,
                        ),
                );
                atlas.layout = vk::ImageLayout::GENERAL;
            }
        }
        if !barriers.is_empty() {
            unsafe {
                self.context.device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &barriers,
                );
            }
        }
    }

    fn write_descriptor_set(&self, inputs: &DdgiDispatchInputs) {
        let ubo_info = vk::DescriptorBufferInfo::default()
            .buffer(self.ubo_buffer.buffer)
            .range(vk::WHOLE_SIZE);
        let irr_info = vk::DescriptorImageInfo::default()
            .image_view(self.irradiance_atlas.view)
            .image_layout(vk::ImageLayout::GENERAL);
        let mut tlas_write = vk::WriteDescriptorSetAccelerationStructureKHR::default()
            .acceleration_structures(std::slice::from_ref(&inputs.tlas));
        let vtx_info = vk::DescriptorBufferInfo::default()
            .buffer(inputs.vertex_buffer)
            .range(vk::WHOLE_SIZE);
        let idx_info = vk::DescriptorBufferInfo::default()
            .buffer(inputs.index_buffer)
            .range(vk::WHOLE_SIZE);
        let dist_info = vk::DescriptorImageInfo::default()
            .image_view(self.distance_atlas.view)
            .image_layout(vk::ImageLayout::GENERAL);
        let meta_info = vk::DescriptorBufferInfo::default()
            .buffer(inputs.instance_meta_buffer)
            .range(vk::WHOLE_SIZE);
        let mat_info = vk::DescriptorBufferInfo::default()
            .buffer(inputs.materials_buffer)
            .range(vk::WHOLE_SIZE);
        let lights_info = vk::DescriptorBufferInfo::default()
            .buffer(inputs.lights_buffer)
            .range(vk::WHOLE_SIZE);
        let emissive_info = vk::DescriptorBufferInfo::default()
            .buffer(inputs.emissive_buffer)
            .range(vk::WHOLE_SIZE);
        let probe_meta_info = vk::DescriptorImageInfo::default()
            .image_view(self.probe_metadata.view)
            .image_layout(vk::ImageLayout::GENERAL);
        let ray_info = vk::DescriptorBufferInfo::default()
            .buffer(self.ray_buffer.buffer)
            .range(vk::WHOLE_SIZE);
        let out_info = vk::DescriptorImageInfo::default()
            .image_view(inputs.probe_volume_out_view)
            .image_layout(vk::ImageLayout::GENERAL);
        let var_info = vk::DescriptorBufferInfo::default()
            .buffer(self.variability_buffer.buffer)
            .range(vk::WHOLE_SIZE);

        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::DDGI_UBO_BINDING)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .buffer_info(std::slice::from_ref(&ubo_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::IRRADIANCE_ATLAS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .image_info(std::slice::from_ref(&irr_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::TLAS_BINDING)
                .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
                .descriptor_count(1)
                .push_next(&mut tlas_write),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::VERTEX_DATA_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(&vtx_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::INDICES_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(&idx_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::DISTANCE_ATLAS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .image_info(std::slice::from_ref(&dist_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::INSTANCE_META_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(&meta_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::MATERIALS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(&mat_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::PT_LIGHTS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(&lights_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::PT_EMISSIVE_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(&emissive_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::PROBE_METADATA_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .image_info(std::slice::from_ref(&probe_meta_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::RAY_BUFFER_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(&ray_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::PROBE_VOLUME_OUT_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .image_info(std::slice::from_ref(&out_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::VARIABILITY_ACCUM_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(&var_info)),
        ];

        unsafe {
            self.context.device.update_descriptor_sets(&writes, &[]);
        }
    }
}

impl Drop for DdgiPass {
    fn drop(&mut self) {
        self.invalidate_geometry();
        let device = &self.context.device;
        self.irradiance_atlas.destroy(device);
        self.distance_atlas.destroy(device);
        self.probe_metadata.destroy(device);
        self.ray_buffer.destroy(device);
        self.variability_buffer.destroy(device);
        self.ubo_buffer.destroy(device);
        self.dummy_lights.destroy(device);
        self.dummy_emissive.destroy(device);
        unsafe {
            if self.descriptor_pool != vk::DescriptorPool::null() {
                device.destroy_descriptor_pool(self.descriptor_pool, None);
            }
            if self.set0_layout != vk::DescriptorSetLayout::null() {
                device.destroy_descriptor_set_layout(self.set0_layout, None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_grid_matches_shader_workgroup_sizes() {
        let params = DdgiVolumeParams {
            probe_counts: [16, 8, 12],
            ..DdgiVolumeParams::default()
        };
        let grid = DdgiDispatchGrid::from_params(&params);
        assert_eq!(grid.clear_groups, [4, 2, 12]);
        assert_eq!(grid.irradiance_groups, [16 * 8 * 12, 1, 1]);
        assert_eq!(
            grid.distance_groups,
            [(16 * 14u32).div_ceil(8), (12 * 14u32).div_ceil(8), 8]
        );
        assert_eq!(grid.blend_groups, [16, 12, 8]);
        assert_eq!(grid.resolve_groups, [4, 2, 12]);
    }

    #[test]
    fn flags_encode_all_ddgi_features_cleanly() {
        let params = DdgiVolumeParams::default();
        let flags = encode_ddgi_flags(&params, true);
        assert_ne!(flags & DDGI_FLAG_RELOCATION, 0);
        assert_ne!(flags & DDGI_FLAG_CLASSIFICATION, 0);
        assert_eq!(flags & DDGI_FLAG_SCROLLING, 0);
        assert_ne!(flags & DDGI_FLAG_VARIABILITY, 0);
        assert_ne!(flags & DDGI_FLAG_CLEAR_ALL, 0);
    }

    #[test]
    fn frame_ray_rotation_quat_is_unit_length() {
        for f in [0, 1, 7, 1024] {
            let q = frame_ray_rotation_quat(f);
            let norm = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
            assert!((norm - 1.0).abs() < 1e-5, "frame {f} norm={norm}");
        }
    }
}
