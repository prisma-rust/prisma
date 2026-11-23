//! Image-based lighting (IBL) resources for PBR rendering.
//!
//! User equirectangular HDR environment texture (or procedural fallback) converted at load time to
//! floating-point **cubemap** (6 faces + full mip chain). PBR shader samples cubemap by reflection direction,
//! which has no pole singularity or seam — so it remains stable when reflections view or object
//! rotates (old equirectangular sampling flickered near poles). This is "true" IBL from user asset.
//!
//! Additionally, three true IBL resources computed from equirectangular texture:
//! - diffuse irradiance cubemap (cosine-weighted hemisphere convolution)
//! - prefiltered environment texture (per mip level GGX importance sampling)
//! - BRDF integration LUT (2D, split-sum)

use anyhow::Context as _;
use ash::vk;
use prism_gfx::ibl::f32_to_f16;
use std::sync::Arc;

use crate::context::VulkanContext;

pub struct IblResources {
    device: ash::Device,
    pub descriptor_set_layout: vk::DescriptorSetLayout,
    pub descriptor_set: vk::DescriptorSet,
    // env cube
    image: vk::Image,
    image_view: vk::ImageView,
    sampler: vk::Sampler,
    memory: vk::DeviceMemory,
    staging: vk::Buffer,
    staging_memory: vk::DeviceMemory,
    // irradiance cube
    irradiance_image: vk::Image,
    irradiance_image_view: vk::ImageView,
    irradiance_sampler: vk::Sampler,
    irradiance_memory: vk::DeviceMemory,
    irradiance_staging: vk::Buffer,
    irradiance_staging_memory: vk::DeviceMemory,
    // prefiltered cube
    prefiltered_image: vk::Image,
    prefiltered_image_view: vk::ImageView,
    prefiltered_sampler: vk::Sampler,
    prefiltered_memory: vk::DeviceMemory,
    prefiltered_staging: vk::Buffer,
    prefiltered_staging_memory: vk::DeviceMemory,
    // brdf LUT
    brdf_image: vk::Image,
    brdf_image_view: vk::ImageView,
    brdf_sampler: vk::Sampler,
    brdf_memory: vk::DeviceMemory,
    brdf_staging: vk::Buffer,
    brdf_staging_memory: vk::DeviceMemory,
    // descriptor pool
    descriptor_pool: vk::DescriptorPool,
}

/// Shared IBL descriptor set layout (set 2): 3 COMBINED_IMAGE_SAMPLER
/// bindings (envCube / irradianceCube / prefilteredCube). Created once by `GraphRenderer`
/// and passed to empty / full IBL, ensuring set2 layout stays stable during "scene switch rebuild IBL",
/// forward pass / skybox pipeline layouts need no rebuild.
pub fn ibl_descriptor_set_layout(device: &ash::Device) -> anyhow::Result<vk::DescriptorSetLayout> {
    let layout = unsafe {
        device.create_descriptor_set_layout(
            &vk::DescriptorSetLayoutCreateInfo::default().bindings(&[
                vk::DescriptorSetLayoutBinding {
                    binding: 0,
                    descriptor_type: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                    descriptor_count: 1,
                    stage_flags: vk::ShaderStageFlags::FRAGMENT | vk::ShaderStageFlags::COMPUTE,
                    ..Default::default()
                },
                vk::DescriptorSetLayoutBinding {
                    binding: 1,
                    descriptor_type: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                    descriptor_count: 1,
                    stage_flags: vk::ShaderStageFlags::FRAGMENT | vk::ShaderStageFlags::COMPUTE,
                    ..Default::default()
                },
                vk::DescriptorSetLayoutBinding {
                    binding: 2,
                    descriptor_type: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                    descriptor_count: 1,
                    stage_flags: vk::ShaderStageFlags::FRAGMENT | vk::ShaderStageFlags::COMPUTE,
                    ..Default::default()
                },
            ]),
            None,
        )
    }?;
    Ok(layout)
}

/// Allocate a face×face×6 neutral cubemap (R16G16B16A16_SFLOAT, 1 mip),
/// return (image, memory, staging, staging_memory). Used for cheap empty IBL.
fn alloc_neutral_cube(
    device: &ash::Device,
    mem_props: &vk::PhysicalDeviceMemoryProperties,
    face: u32,
) -> anyhow::Result<(vk::Image, vk::DeviceMemory, vk::Buffer, vk::DeviceMemory)> {
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(vk::Format::R16G16B16A16_SFLOAT)
        .extent(vk::Extent3D {
            width: face,
            height: face,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(6)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
        .flags(vk::ImageCreateFlags::CUBE_COMPATIBLE)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let image = unsafe { device.create_image(&info, None) }?;
    let mem_req = unsafe { device.get_image_memory_requirements(image) };
    let memory = allocate_device_memory(device, mem_props, mem_req)?;
    unsafe { device.bind_image_memory(image, memory, 0) }?;
    let staging_size = (6 * face * face * 8) as u64;
    let (staging, staging_memory) = create_staging_buffer(device, mem_props, staging_size)?;
    Ok((image, memory, staging, staging_memory))
}

/// Scope of IBL textures to prepare on the GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IblScope {
    /// Only the environment cubemap (`envCube`, binding 0) for Path Tracing /
    /// skybox-only pipelines; irradiance, prefiltered, and BRDF LUT remain 1x1
    /// neutral dummies.
    EnvOnly,
    /// Full raster PBR IBL (`envCube` + irradiance + prefiltered + BRDF LUT).
    Full,
}

impl IblResources {
    #[cfg(feature = "ibl")]
    pub fn new(
        context: Arc<VulkanContext>,
        command_pool: vk::CommandPool,
        queue: vk::Queue,
        env_bytes: Option<Vec<u8>>,
        scope: IblScope,
        ds_layout: vk::DescriptorSetLayout,
    ) -> anyhow::Result<Self> {
        // Fast path: pre-baked `PIBL` payload from `.pak` — zero convolution.
        if let Some(bytes) = env_bytes.as_deref() {
            if prism_gfx::ibl::CookedIbl::is_cooked(bytes) {
                let t = std::time::Instant::now();
                let cooked = match scope {
                    IblScope::Full => prism_gfx::ibl::CookedIbl::decode(bytes)?,
                    IblScope::EnvOnly => prism_gfx::ibl::CookedIbl::decode_env_only(bytes)?,
                };
                log::info!(
                    "IBL: decoded cooked PIBL ({}x{}, {:?}, {}ms)",
                    cooked.env_face_size,
                    cooked.env_face_size,
                    scope,
                    t.elapsed().as_millis()
                );
                return Self::new_from_cooked(context, command_pool, queue, cooked, ds_layout);
            }
        }

        // Runtime `.hdr` or procedural sky: decode equirect on CPU, then run GPU
        // compute shaders (`ibl_bake.slang`) for cubemap & IBL convolutions.
        Self::new_from_equirect_gpu(
            context,
            command_pool,
            queue,
            env_bytes.as_deref(),
            scope,
            ds_layout,
        )
    }

    #[cfg(feature = "ibl")]
    fn new_from_cooked(
        context: Arc<VulkanContext>,
        command_pool: vk::CommandPool,
        queue: vk::Queue,
        cooked: prism_gfx::ibl::CookedIbl,
        ds_layout: vk::DescriptorSetLayout,
    ) -> anyhow::Result<Self> {
        let device = context.device.clone();
        let mem_props = &context.physical_device_memory_properties;

        let face_size = cooked.env_face_size;
        let irradiance_face_size = cooked.irradiance_face_size;
        let prefiltered_face_size = cooked.prefiltered_face_size;
        let prefiltered_mip_levels = cooked.prefiltered_mip_levels;
        let brdf_lut_size = cooked.brdf_size;
        let mip_levels = (face_size as f32).log2().floor() as u32 + 1;

        // 2. Create all images and fill staging buffers directly from `f16` slices.
        // 2a. Environment cubemap.
        let image_info = vk::ImageCreateInfo {
            image_type: vk::ImageType::TYPE_2D,
            format: vk::Format::R16G16B16A16_SFLOAT,
            extent: vk::Extent3D {
                width: face_size,
                height: face_size,
                depth: 1,
            },
            mip_levels,
            array_layers: 6,
            samples: vk::SampleCountFlags::TYPE_1,
            tiling: vk::ImageTiling::OPTIMAL,
            usage: vk::ImageUsageFlags::TRANSFER_DST
                | vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::SAMPLED,
            flags: vk::ImageCreateFlags::CUBE_COMPATIBLE,
            sharing_mode: vk::SharingMode::EXCLUSIVE,
            ..Default::default()
        };
        let t_img_create = std::time::Instant::now();
        let image = unsafe { device.create_image(&image_info, None) }?;
        let mem_req = unsafe { device.get_image_memory_requirements(image) };
        let memory = allocate_device_memory(&device, mem_props, mem_req)?;
        unsafe { device.bind_image_memory(image, memory, 0) }?;

        let staging_size = (cooked.env_cube.len() * 2) as u64;
        let (staging, staging_memory) = create_staging_buffer(&device, mem_props, staging_size)?;
        upload_u16_to_staging(&device, staging_memory, staging_size, &cooked.env_cube)?;

        // 2b. Irradiance cubemap.
        let irradiance_image_info = vk::ImageCreateInfo {
            image_type: vk::ImageType::TYPE_2D,
            format: vk::Format::R16G16B16A16_SFLOAT,
            extent: vk::Extent3D {
                width: irradiance_face_size,
                height: irradiance_face_size,
                depth: 1,
            },
            mip_levels: 1,
            array_layers: 6,
            samples: vk::SampleCountFlags::TYPE_1,
            tiling: vk::ImageTiling::OPTIMAL,
            usage: vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED,
            flags: vk::ImageCreateFlags::CUBE_COMPATIBLE,
            sharing_mode: vk::SharingMode::EXCLUSIVE,
            ..Default::default()
        };
        let irradiance_image = unsafe { device.create_image(&irradiance_image_info, None) }?;
        let irradiance_mem_req = unsafe { device.get_image_memory_requirements(irradiance_image) };
        let irradiance_memory = allocate_device_memory(&device, mem_props, irradiance_mem_req)?;
        unsafe { device.bind_image_memory(irradiance_image, irradiance_memory, 0) }?;

        let irradiance_staging_size = (cooked.irradiance_cube.len() * 2) as u64;
        let (irradiance_staging, irradiance_staging_memory) =
            create_staging_buffer(&device, mem_props, irradiance_staging_size)?;
        upload_u16_to_staging(
            &device,
            irradiance_staging_memory,
            irradiance_staging_size,
            &cooked.irradiance_cube,
        )?;

        // 2c. Prefiltered cubemap.
        let prefiltered_staging_size: u64 = cooked
            .prefiltered_mips
            .iter()
            .map(|m| (m.len() * 2) as u64)
            .sum();
        let (prefiltered_staging, prefiltered_staging_memory) =
            create_staging_buffer(&device, mem_props, prefiltered_staging_size)?;
        {
            let mapped = unsafe {
                device.map_memory(
                    prefiltered_staging_memory,
                    0,
                    prefiltered_staging_size,
                    vk::MemoryMapFlags::empty(),
                )?
            } as *mut u16;
            let mut offset_u16 = 0usize;
            for mip_data in &cooked.prefiltered_mips {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        mip_data.as_ptr(),
                        mapped.add(offset_u16),
                        mip_data.len(),
                    );
                }
                offset_u16 += mip_data.len();
            }
            unsafe {
                device.unmap_memory(prefiltered_staging_memory);
            }
        }

        let prefiltered_image_info = vk::ImageCreateInfo {
            image_type: vk::ImageType::TYPE_2D,
            format: vk::Format::R16G16B16A16_SFLOAT,
            extent: vk::Extent3D {
                width: prefiltered_face_size,
                height: prefiltered_face_size,
                depth: 1,
            },
            mip_levels: prefiltered_mip_levels,
            array_layers: 6,
            samples: vk::SampleCountFlags::TYPE_1,
            tiling: vk::ImageTiling::OPTIMAL,
            usage: vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED,
            flags: vk::ImageCreateFlags::CUBE_COMPATIBLE,
            sharing_mode: vk::SharingMode::EXCLUSIVE,
            ..Default::default()
        };
        let prefiltered_image = unsafe { device.create_image(&prefiltered_image_info, None) }?;
        let prefiltered_mem_req =
            unsafe { device.get_image_memory_requirements(prefiltered_image) };
        let prefiltered_memory = allocate_device_memory(&device, mem_props, prefiltered_mem_req)?;
        unsafe { device.bind_image_memory(prefiltered_image, prefiltered_memory, 0) }?;

        // 2d. BRDF LUT (RG16_SFLOAT, 1 layer).
        let brdf_staging_size = (cooked.brdf_lut.len() * 2) as u64;
        let (brdf_staging, brdf_staging_memory) =
            create_staging_buffer(&device, mem_props, brdf_staging_size)?;
        upload_u16_to_staging(
            &device,
            brdf_staging_memory,
            brdf_staging_size,
            &cooked.brdf_lut,
        )?;

        let brdf_image_info = vk::ImageCreateInfo {
            image_type: vk::ImageType::TYPE_2D,
            format: vk::Format::R16G16_SFLOAT,
            extent: vk::Extent3D {
                width: brdf_lut_size,
                height: brdf_lut_size,
                depth: 1,
            },
            mip_levels: 1,
            array_layers: 1,
            samples: vk::SampleCountFlags::TYPE_1,
            tiling: vk::ImageTiling::OPTIMAL,
            usage: vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED,
            flags: vk::ImageCreateFlags::empty(),
            sharing_mode: vk::SharingMode::EXCLUSIVE,
            ..Default::default()
        };
        let brdf_image = unsafe { device.create_image(&brdf_image_info, None) }?;
        let brdf_mem_req = unsafe { device.get_image_memory_requirements(brdf_image) };
        let brdf_memory = allocate_device_memory(&device, mem_props, brdf_mem_req)?;
        unsafe { device.bind_image_memory(brdf_image, brdf_memory, 0) }?;

        // 3. One-shot command buffer: upload all resources.
        let cmd = allocate_temp_command_buffer(&device, command_pool)?;
        unsafe { device.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default())? };

        // 3a. Env cube: transition all mips, copy mip0, generate mip chain.
        transition_image(
            &device,
            cmd,
            image,
            0,
            mip_levels,
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
        let copy = vk::BufferImageCopy {
            buffer_offset: 0,
            buffer_row_length: 0,
            buffer_image_height: 0,
            image_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 6,
            },
            image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
            image_extent: vk::Extent3D {
                width: face_size,
                height: face_size,
                depth: 1,
            },
        };
        unsafe {
            device.cmd_copy_buffer_to_image(
                cmd,
                staging,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[copy],
            )
        };

        // Generate mip chain for env cube.
        if mip_levels > 1 {
            transition_image(
                &device,
                cmd,
                image,
                0,
                1,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            );
            for mip in 1..mip_levels {
                let src_level = mip - 1;
                let src_ext = mip_extent(face_size, face_size, src_level);
                let dst_ext = mip_extent(face_size, face_size, mip);
                for layer in 0..6u32 {
                    let blit = vk::ImageBlit {
                        src_subresource: vk::ImageSubresourceLayers {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            mip_level: src_level,
                            base_array_layer: layer,
                            layer_count: 1,
                        },
                        src_offsets: [
                            vk::Offset3D { x: 0, y: 0, z: 0 },
                            vk::Offset3D {
                                x: src_ext.width as i32,
                                y: src_ext.height as i32,
                                z: 1,
                            },
                        ],
                        dst_subresource: vk::ImageSubresourceLayers {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            mip_level: mip,
                            base_array_layer: layer,
                            layer_count: 1,
                        },
                        dst_offsets: [
                            vk::Offset3D { x: 0, y: 0, z: 0 },
                            vk::Offset3D {
                                x: dst_ext.width as i32,
                                y: dst_ext.height as i32,
                                z: 1,
                            },
                        ],
                    };
                    unsafe {
                        device.cmd_blit_image(
                            cmd,
                            image,
                            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                            image,
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                            &[blit],
                            vk::Filter::LINEAR,
                        )
                    };
                }
                transition_image(
                    &device,
                    cmd,
                    image,
                    src_level,
                    1,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                );
                if mip + 1 < mip_levels {
                    transition_image(
                        &device,
                        cmd,
                        image,
                        mip,
                        1,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    );
                }
            }
        }
        transition_image(
            &device,
            cmd,
            image,
            mip_levels - 1,
            1,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );

        // 3b. Upload irradiance cube.
        transition_image_single(
            &device,
            cmd,
            irradiance_image,
            color_subresource(0, 1, 0, 6),
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
        let irradiance_copy = vk::BufferImageCopy {
            buffer_offset: 0,
            buffer_row_length: 0,
            buffer_image_height: 0,
            image_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 6,
            },
            image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
            image_extent: vk::Extent3D {
                width: irradiance_face_size,
                height: irradiance_face_size,
                depth: 1,
            },
        };
        unsafe {
            device.cmd_copy_buffer_to_image(
                cmd,
                irradiance_staging,
                irradiance_image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[irradiance_copy],
            )
        };
        transition_image_single(
            &device,
            cmd,
            irradiance_image,
            color_subresource(0, 1, 0, 6),
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );

        // 3c. Upload prefiltered cube (per-mip copies).
        transition_image_single(
            &device,
            cmd,
            prefiltered_image,
            color_subresource(0, prefiltered_mip_levels, 0, 6),
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
        let mut prefiltered_offset: u64 = 0;
        for mip in 0..prefiltered_mip_levels {
            let fs = (prefiltered_face_size >> mip).max(1);
            let mip_copy = vk::BufferImageCopy {
                buffer_offset: prefiltered_offset,
                buffer_row_length: 0,
                buffer_image_height: 0,
                image_subresource: vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: mip,
                    base_array_layer: 0,
                    layer_count: 6,
                },
                image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
                image_extent: vk::Extent3D {
                    width: fs,
                    height: fs,
                    depth: 1,
                },
            };
            unsafe {
                device.cmd_copy_buffer_to_image(
                    cmd,
                    prefiltered_staging,
                    prefiltered_image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[mip_copy],
                )
            };
            prefiltered_offset += (6 * fs * fs * 8) as u64;
        }
        transition_image_single(
            &device,
            cmd,
            prefiltered_image,
            color_subresource(0, prefiltered_mip_levels, 0, 6),
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );

        // 3d. Upload BRDF LUT.
        transition_image_single(
            &device,
            cmd,
            brdf_image,
            color_subresource(0, 1, 0, 1),
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
        let brdf_copy = vk::BufferImageCopy {
            buffer_offset: 0,
            buffer_row_length: 0,
            buffer_image_height: 0,
            image_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            },
            image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
            image_extent: vk::Extent3D {
                width: brdf_lut_size,
                height: brdf_lut_size,
                depth: 1,
            },
        };
        unsafe {
            device.cmd_copy_buffer_to_image(
                cmd,
                brdf_staging,
                brdf_image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[brdf_copy],
            )
        };
        transition_image_single(
            &device,
            cmd,
            brdf_image,
            color_subresource(0, 1, 0, 1),
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );

        unsafe { device.end_command_buffer(cmd) }?;
        let img_create_ms = t_img_create.elapsed().as_millis();
        log::info!(
            "  IBL phase: create images + alloc + fill staging: {}ms",
            img_create_ms
        );
        let t_upload = std::time::Instant::now();
        submit_and_wait(&device, queue, command_pool, cmd).context("IBL upload submit")?;
        let upload_ms = t_upload.elapsed().as_millis();
        log::info!("  IBL phase: upload + submit + wait: {}ms", upload_ms);

        let t_views = std::time::Instant::now();
        // 4. Create image views + samplers.

        // 4a. Env cube view + sampler.
        let image_view = unsafe {
            device.create_image_view(
                &vk::ImageViewCreateInfo {
                    image,
                    view_type: vk::ImageViewType::CUBE,
                    format: vk::Format::R16G16B16A16_SFLOAT,
                    subresource_range: vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: mip_levels,
                        base_array_layer: 0,
                        layer_count: 6,
                    },
                    ..Default::default()
                },
                None,
            )
        }?;

        let sampler = unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo {
                    mag_filter: vk::Filter::LINEAR,
                    min_filter: vk::Filter::LINEAR,
                    mipmap_mode: vk::SamplerMipmapMode::LINEAR,
                    address_mode_u: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    address_mode_v: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    address_mode_w: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    anisotropy_enable: vk::FALSE,
                    min_lod: 0.0,
                    max_lod: mip_levels as f32,
                    ..Default::default()
                },
                None,
            )
        }?;

        // 4b. Irradiance cube view + sampler.
        let irradiance_image_view = unsafe {
            device.create_image_view(
                &vk::ImageViewCreateInfo {
                    image: irradiance_image,
                    view_type: vk::ImageViewType::CUBE,
                    format: vk::Format::R16G16B16A16_SFLOAT,
                    subresource_range: vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 6,
                    },
                    ..Default::default()
                },
                None,
            )
        }?;

        let irradiance_sampler = unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo {
                    mag_filter: vk::Filter::LINEAR,
                    min_filter: vk::Filter::LINEAR,
                    mipmap_mode: vk::SamplerMipmapMode::LINEAR,
                    address_mode_u: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    address_mode_v: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    address_mode_w: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    anisotropy_enable: vk::FALSE,
                    min_lod: 0.0,
                    max_lod: 1.0,
                    ..Default::default()
                },
                None,
            )
        }?;

        // 4c. Prefiltered cube view + sampler.
        let prefiltered_image_view = unsafe {
            device.create_image_view(
                &vk::ImageViewCreateInfo {
                    image: prefiltered_image,
                    view_type: vk::ImageViewType::CUBE,
                    format: vk::Format::R16G16B16A16_SFLOAT,
                    subresource_range: vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: prefiltered_mip_levels,
                        base_array_layer: 0,
                        layer_count: 6,
                    },
                    ..Default::default()
                },
                None,
            )
        }?;

        let prefiltered_sampler = unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo {
                    mag_filter: vk::Filter::LINEAR,
                    min_filter: vk::Filter::LINEAR,
                    mipmap_mode: vk::SamplerMipmapMode::LINEAR,
                    address_mode_u: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    address_mode_v: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    address_mode_w: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    anisotropy_enable: vk::FALSE,
                    min_lod: 0.0,
                    max_lod: (prefiltered_mip_levels - 1) as f32,
                    ..Default::default()
                },
                None,
            )
        }?;

        // 4d. BRDF LUT view + sampler.
        let brdf_image_view = unsafe {
            device.create_image_view(
                &vk::ImageViewCreateInfo {
                    image: brdf_image,
                    view_type: vk::ImageViewType::TYPE_2D,
                    format: vk::Format::R16G16_SFLOAT,
                    subresource_range: vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
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

        let brdf_sampler = unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo {
                    mag_filter: vk::Filter::LINEAR,
                    min_filter: vk::Filter::LINEAR,
                    mipmap_mode: vk::SamplerMipmapMode::LINEAR,
                    address_mode_u: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    address_mode_v: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    address_mode_w: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    anisotropy_enable: vk::FALSE,
                    min_lod: 0.0,
                    max_lod: 1.0,
                    ..Default::default()
                },
                None,
            )
        }?;

        // 5. Descriptor set (set 2): 3 bindings envCube/irradianceCube/prefilteredCube.
        // Layout shared and passed by caller (ds_layout), ensuring forward pass / skybox pipeline
        // layouts stay stable when scene rebuilds IBL. brdfLUT registered separately to bindless table.
        let descriptor_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&[vk::DescriptorPoolSize {
                        ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                        descriptor_count: 3,
                    }]),
                None,
            )
        }?;

        let descriptor_set = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(&[ds_layout]),
            )
        }?[0];

        unsafe {
            device.update_descriptor_sets(
                &[
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(0)
                        .dst_array_element(0)
                        .descriptor_count(1)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&[vk::DescriptorImageInfo {
                            sampler,
                            image_view,
                            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        }]),
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(1)
                        .dst_array_element(0)
                        .descriptor_count(1)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&[vk::DescriptorImageInfo {
                            sampler: irradiance_sampler,
                            image_view: irradiance_image_view,
                            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        }]),
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(2)
                        .dst_array_element(0)
                        .descriptor_count(1)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&[vk::DescriptorImageInfo {
                            sampler: prefiltered_sampler,
                            image_view: prefiltered_image_view,
                            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        }]),
                ],
                &[],
            )
        };

        let views_ms = t_views.elapsed().as_millis();
        log::info!(
            "  IBL phase: views + samplers + descriptors: {}ms",
            views_ms
        );
        Ok(Self {
            device,
            descriptor_set_layout: ds_layout,
            descriptor_set,
            image,
            image_view,
            sampler,
            memory,
            staging,
            staging_memory,
            irradiance_image,
            irradiance_image_view,
            irradiance_sampler,
            irradiance_memory,
            irradiance_staging,
            irradiance_staging_memory,
            prefiltered_image,
            prefiltered_image_view,
            prefiltered_sampler,
            prefiltered_memory,
            prefiltered_staging,
            prefiltered_staging_memory,
            brdf_image,
            brdf_image_view,
            brdf_sampler,
            brdf_memory,
            brdf_staging,
            brdf_staging_memory,
            descriptor_pool,
        })
    }

    #[cfg(feature = "ibl")]
    fn new_from_equirect_gpu(
        context: Arc<VulkanContext>,
        command_pool: vk::CommandPool,
        queue: vk::Queue,
        env_bytes: Option<&[u8]>,
        scope: IblScope,
        ds_layout: vk::DescriptorSetLayout,
    ) -> anyhow::Result<Self> {
        use crate::shader_bindings::ibl_bake as bindings;
        use prism_gfx::ibl::{
            f32_slice_to_f16, f32_to_f16, procedural_equirect, BRDF_LUT_SAMPLES, BRDF_LUT_SIZE,
            ENV_FACE_SIZE, IRRADIANCE_FACE_SIZE, IRRADIANCE_SAMPLES, PREFILTERED_FACE_SIZE,
            PREFILTERED_MIP_LEVELS, PREFILTERED_SAMPLES,
        };
        use std::ffi::CString;

        let t_total = std::time::Instant::now();
        let device = context.device.clone();
        let mem_props = &context.physical_device_memory_properties;

        // 1. Decode equirect HDR on CPU (or synthesize procedural sky) -> f16.
        let (rgba_f32, eq_w, eq_h) = match env_bytes {
            Some(bytes) => {
                prism_gfx::hdr::load_rgbe(bytes).context("failed to decode environment .hdr")?
            }
            None => procedural_equirect(512, 256),
        };
        let eq_f16 = f32_slice_to_f16(&rgba_f32);
        drop(rgba_f32);

        let eq_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::R16G16B16A16_SFLOAT)
            .extent(vk::Extent3D {
                width: eq_w,
                height: eq_h,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let eq_image = unsafe { device.create_image(&eq_info, None) }?;
        let eq_mem_req = unsafe { device.get_image_memory_requirements(eq_image) };
        let eq_memory = allocate_device_memory(&device, mem_props, eq_mem_req)?;
        unsafe { device.bind_image_memory(eq_image, eq_memory, 0) }?;
        let eq_staging_size = (eq_f16.len() * 2) as u64;
        let (eq_staging, eq_staging_memory) =
            create_staging_buffer(&device, mem_props, eq_staging_size)?;
        upload_u16_to_staging(&device, eq_staging_memory, eq_staging_size, &eq_f16)?;
        drop(eq_f16);

        let eq_view = unsafe {
            device.create_image_view(
                &vk::ImageViewCreateInfo {
                    image: eq_image,
                    view_type: vk::ImageViewType::TYPE_2D,
                    format: vk::Format::R16G16B16A16_SFLOAT,
                    subresource_range: color_subresource(0, 1, 0, 1),
                    ..Default::default()
                },
                None,
            )
        }?;
        let eq_sampler = unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo {
                    mag_filter: vk::Filter::LINEAR,
                    min_filter: vk::Filter::LINEAR,
                    mipmap_mode: vk::SamplerMipmapMode::NEAREST,
                    address_mode_u: vk::SamplerAddressMode::REPEAT,
                    address_mode_v: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    address_mode_w: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    ..Default::default()
                },
                None,
            )
        }?;

        // 2. Create output GPU images.
        let face_size = ENV_FACE_SIZE;
        let mip_levels = (face_size as f32).log2().floor() as u32 + 1;
        let image_info = vk::ImageCreateInfo {
            image_type: vk::ImageType::TYPE_2D,
            format: vk::Format::R16G16B16A16_SFLOAT,
            extent: vk::Extent3D {
                width: face_size,
                height: face_size,
                depth: 1,
            },
            mip_levels,
            array_layers: 6,
            samples: vk::SampleCountFlags::TYPE_1,
            tiling: vk::ImageTiling::OPTIMAL,
            usage: vk::ImageUsageFlags::STORAGE
                | vk::ImageUsageFlags::TRANSFER_DST
                | vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::SAMPLED,
            flags: vk::ImageCreateFlags::CUBE_COMPATIBLE,
            sharing_mode: vk::SharingMode::EXCLUSIVE,
            ..Default::default()
        };
        let image = unsafe { device.create_image(&image_info, None) }?;
        let memory = allocate_device_memory(&device, mem_props, unsafe {
            device.get_image_memory_requirements(image)
        })?;
        unsafe { device.bind_image_memory(image, memory, 0) }?;

        let (irr_size, pre_size, pre_mips, brdf_size) = match scope {
            IblScope::Full => (
                IRRADIANCE_FACE_SIZE,
                PREFILTERED_FACE_SIZE,
                PREFILTERED_MIP_LEVELS,
                BRDF_LUT_SIZE,
            ),
            IblScope::EnvOnly => (1, 1, 1, 1),
        };

        let make_cube_image = |dim: u32,
                               mips: u32,
                               extra_usage: vk::ImageUsageFlags|
         -> anyhow::Result<(vk::Image, vk::DeviceMemory)> {
            let info = vk::ImageCreateInfo {
                image_type: vk::ImageType::TYPE_2D,
                format: vk::Format::R16G16B16A16_SFLOAT,
                extent: vk::Extent3D {
                    width: dim,
                    height: dim,
                    depth: 1,
                },
                mip_levels: mips,
                array_layers: 6,
                samples: vk::SampleCountFlags::TYPE_1,
                tiling: vk::ImageTiling::OPTIMAL,
                usage: vk::ImageUsageFlags::SAMPLED | extra_usage,
                flags: vk::ImageCreateFlags::CUBE_COMPATIBLE,
                sharing_mode: vk::SharingMode::EXCLUSIVE,
                ..Default::default()
            };
            let img = unsafe { device.create_image(&info, None) }?;
            let mem = allocate_device_memory(&device, mem_props, unsafe {
                device.get_image_memory_requirements(img)
            })?;
            unsafe { device.bind_image_memory(img, mem, 0) }?;
            Ok((img, mem))
        };

        let pbr_usage = match scope {
            IblScope::Full => vk::ImageUsageFlags::STORAGE,
            IblScope::EnvOnly => vk::ImageUsageFlags::TRANSFER_DST,
        };
        let (irradiance_image, irradiance_memory) = make_cube_image(irr_size, 1, pbr_usage)?;
        let (prefiltered_image, prefiltered_memory) =
            make_cube_image(pre_size, pre_mips, pbr_usage)?;

        let brdf_image_info = vk::ImageCreateInfo {
            image_type: vk::ImageType::TYPE_2D,
            format: vk::Format::R16G16_SFLOAT,
            extent: vk::Extent3D {
                width: brdf_size,
                height: brdf_size,
                depth: 1,
            },
            mip_levels: 1,
            array_layers: 1,
            samples: vk::SampleCountFlags::TYPE_1,
            tiling: vk::ImageTiling::OPTIMAL,
            usage: vk::ImageUsageFlags::SAMPLED | pbr_usage,
            flags: vk::ImageCreateFlags::empty(),
            sharing_mode: vk::SharingMode::EXCLUSIVE,
            ..Default::default()
        };
        let brdf_image = unsafe { device.create_image(&brdf_image_info, None) }?;
        let brdf_memory = allocate_device_memory(&device, mem_props, unsafe {
            device.get_image_memory_requirements(brdf_image)
        })?;
        unsafe { device.bind_image_memory(brdf_image, brdf_memory, 0) }?;

        // 3. Temporary storage views & compute pipelines.
        let make_array_storage_view = |img: vk::Image, mip: u32| -> anyhow::Result<vk::ImageView> {
            Ok(unsafe {
                device.create_image_view(
                    &vk::ImageViewCreateInfo {
                        image: img,
                        view_type: vk::ImageViewType::TYPE_2D_ARRAY,
                        format: vk::Format::R16G16B16A16_SFLOAT,
                        subresource_range: color_subresource(mip, 1, 0, 6),
                        ..Default::default()
                    },
                    None,
                )?
            })
        };

        let env_storage_view = make_array_storage_view(image, 0)?;
        let mut temp_views = vec![env_storage_view];

        let comp_ds_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&[
                    vk::DescriptorSetLayoutBinding {
                        binding: bindings::IN_EQUIRECT_BINDING,
                        descriptor_type: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                        descriptor_count: 1,
                        stage_flags: vk::ShaderStageFlags::COMPUTE,
                        ..Default::default()
                    },
                    vk::DescriptorSetLayoutBinding {
                        binding: bindings::OUT_CUBE_BINDING,
                        descriptor_type: vk::DescriptorType::STORAGE_IMAGE,
                        descriptor_count: 1,
                        stage_flags: vk::ShaderStageFlags::COMPUTE,
                        ..Default::default()
                    },
                    vk::DescriptorSetLayoutBinding {
                        binding: bindings::OUT_BRDF_BINDING,
                        descriptor_type: vk::DescriptorType::STORAGE_IMAGE,
                        descriptor_count: 1,
                        stage_flags: vk::ShaderStageFlags::COMPUTE,
                        ..Default::default()
                    },
                ]),
                None,
            )?
        };

        let comp_pipeline_layout = unsafe {
            device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&[comp_ds_layout])
                    .push_constant_ranges(&[vk::PushConstantRange {
                        stage_flags: vk::ShaderStageFlags::COMPUTE,
                        offset: 0,
                        size: bindings::PUSH_CONSTANT_SIZE,
                    }]),
                None,
            )?
        };

        let create_comp_pipe = |spv: &[u8], entry: &str| -> anyhow::Result<vk::Pipeline> {
            let module = crate::shader::load_shader_module(&device, spv)
                .context("load IBL compute shader module")?;
            let entry_c = CString::new(entry)?;
            let stage = vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::COMPUTE)
                .module(module)
                .name(&entry_c);
            let info = vk::ComputePipelineCreateInfo::default()
                .stage(stage)
                .layout(comp_pipeline_layout);
            let pipe = unsafe {
                device
                    .create_compute_pipelines(vk::PipelineCache::null(), &[info], None)
                    .map_err(|(_, e)| e)?
            }[0];
            unsafe { device.destroy_shader_module(module, None) };
            Ok(pipe)
        };

        const EQUIRECT_SPV: &[u8] = prism_render_shaders::IBL_EQUIRECT_TO_CUBE_COMP_SPV;
        let pipe_equirect = create_comp_pipe(EQUIRECT_SPV, bindings::ENTRY_EQUIRECT_TO_CUBE_MAIN)?;
        let mut temp_pipes = vec![pipe_equirect];

        let max_sets = if scope == IblScope::Full {
            3 + pre_mips
        } else {
            1
        };
        let comp_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(max_sets)
                    .pool_sizes(&[
                        vk::DescriptorPoolSize {
                            ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                            descriptor_count: max_sets,
                        },
                        vk::DescriptorPoolSize {
                            ty: vk::DescriptorType::STORAGE_IMAGE,
                            descriptor_count: max_sets * 2,
                        },
                    ]),
                None,
            )?
        };
        let comp_layouts = vec![comp_ds_layout; max_sets as usize];
        let comp_sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(comp_pool)
                    .set_layouts(&comp_layouts),
            )?
        };

        // Dummy 2D/array bindings for unused storage slot in shared set 0.
        let (dummy_cube_img, dummy_cube_mem) = if scope == IblScope::EnvOnly {
            make_cube_image(1, 1, vk::ImageUsageFlags::STORAGE)?
        } else {
            (vk::Image::null(), vk::DeviceMemory::null())
        };
        let (dummy_brdf_img, dummy_brdf_mem) = if scope == IblScope::EnvOnly {
            let info = vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(vk::Format::R16G16_SFLOAT)
                .extent(vk::Extent3D {
                    width: 1,
                    height: 1,
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(vk::ImageUsageFlags::STORAGE)
                .sharing_mode(vk::SharingMode::EXCLUSIVE);
            let img = unsafe { device.create_image(&info, None) }?;
            let mem = allocate_device_memory(&device, mem_props, unsafe {
                device.get_image_memory_requirements(img)
            })?;
            unsafe { device.bind_image_memory(img, mem, 0) }?;
            (img, mem)
        } else {
            (vk::Image::null(), vk::DeviceMemory::null())
        };

        let dummy_brdf_view = unsafe {
            let target = if scope == IblScope::Full {
                brdf_image
            } else {
                dummy_brdf_img
            };
            device.create_image_view(
                &vk::ImageViewCreateInfo {
                    image: target,
                    view_type: vk::ImageViewType::TYPE_2D,
                    format: vk::Format::R16G16_SFLOAT,
                    subresource_range: color_subresource(0, 1, 0, 1),
                    ..Default::default()
                },
                None,
            )?
        };
        temp_views.push(dummy_brdf_view);

        let write_comp_set =
            |ds: vk::DescriptorSet, cube_view: vk::ImageView, brdf_view: vk::ImageView| {
                let eq_info = [vk::DescriptorImageInfo {
                    sampler: eq_sampler,
                    image_view: eq_view,
                    image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                }];
                let cube_info = [vk::DescriptorImageInfo {
                    sampler: vk::Sampler::null(),
                    image_view: cube_view,
                    image_layout: vk::ImageLayout::GENERAL,
                }];
                let brdf_info = [vk::DescriptorImageInfo {
                    sampler: vk::Sampler::null(),
                    image_view: brdf_view,
                    image_layout: vk::ImageLayout::GENERAL,
                }];
                unsafe {
                    device.update_descriptor_sets(
                        &[
                            vk::WriteDescriptorSet::default()
                                .dst_set(ds)
                                .dst_binding(bindings::IN_EQUIRECT_BINDING)
                                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                                .image_info(&eq_info),
                            vk::WriteDescriptorSet::default()
                                .dst_set(ds)
                                .dst_binding(bindings::OUT_CUBE_BINDING)
                                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                                .image_info(&cube_info),
                            vk::WriteDescriptorSet::default()
                                .dst_set(ds)
                                .dst_binding(bindings::OUT_BRDF_BINDING)
                                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                                .image_info(&brdf_info),
                        ],
                        &[],
                    );
                }
            };

        write_comp_set(comp_sets[0], env_storage_view, dummy_brdf_view);

        // Prepare PBR pipelines & descriptor sets when Full.
        let pbr_pipes = if scope == IblScope::Full {
            const IRR_SPV: &[u8] = prism_render_shaders::IBL_IRRADIANCE_COMP_SPV;
            const PRE_SPV: &[u8] = prism_render_shaders::IBL_PREFILTER_COMP_SPV;
            const BRDF_SPV: &[u8] = prism_render_shaders::IBL_BRDF_LUT_COMP_SPV;
            let p_irr = create_comp_pipe(IRR_SPV, bindings::ENTRY_IRRADIANCE_MAIN)?;
            let p_pre = create_comp_pipe(PRE_SPV, bindings::ENTRY_PREFILTER_MAIN)?;
            let p_brdf = create_comp_pipe(BRDF_SPV, bindings::ENTRY_BRDF_LUT_MAIN)?;
            temp_pipes.extend([p_irr, p_pre, p_brdf]);

            let irr_view = make_array_storage_view(irradiance_image, 0)?;
            temp_views.push(irr_view);
            write_comp_set(comp_sets[1], irr_view, dummy_brdf_view);

            for mip in 0..pre_mips {
                let pv = make_array_storage_view(prefiltered_image, mip)?;
                temp_views.push(pv);
                write_comp_set(comp_sets[2 + mip as usize], pv, dummy_brdf_view);
            }
            write_comp_set(
                comp_sets[2 + pre_mips as usize],
                env_storage_view,
                dummy_brdf_view,
            );
            Some((p_irr, p_pre, p_brdf))
        } else {
            None
        };

        // For EnvOnly, upload 1x1 neutral dummies for the unused PBR slots.
        let mut neutral_staging: Vec<(vk::Buffer, vk::DeviceMemory)> = Vec::new();
        if scope == IblScope::EnvOnly {
            let cube_bytes = (6 * 4 * 2) as u64;
            let neutral_cube = vec![f32_to_f16(0.0); 6 * 4];
            let (s_irr, m_irr) = create_staging_buffer(&device, mem_props, cube_bytes)?;
            upload_u16_to_staging(&device, m_irr, cube_bytes, &neutral_cube)?;
            let (s_pre, m_pre) = create_staging_buffer(&device, mem_props, cube_bytes)?;
            upload_u16_to_staging(&device, m_pre, cube_bytes, &neutral_cube)?;
            let brdf_bytes = 4u64;
            let neutral_brdf = vec![f32_to_f16(1.0); 2];
            let (s_brdf, m_brdf) = create_staging_buffer(&device, mem_props, brdf_bytes)?;
            upload_u16_to_staging(&device, m_brdf, brdf_bytes, &neutral_brdf)?;
            neutral_staging.extend([(s_irr, m_irr), (s_pre, m_pre), (s_brdf, m_brdf)]);
        }

        // 4. Record command buffer: upload equirect -> dispatch compute shaders -> mipmap blit.
        let cmd = allocate_temp_command_buffer(&device, command_pool)?;
        unsafe { device.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default())? };

        // 4a. Upload equirect texture.
        transition_image_single(
            &device,
            cmd,
            eq_image,
            color_subresource(0, 1, 0, 1),
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
        let eq_copy = vk::BufferImageCopy {
            buffer_offset: 0,
            buffer_row_length: 0,
            buffer_image_height: 0,
            image_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            },
            image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
            image_extent: vk::Extent3D {
                width: eq_w,
                height: eq_h,
                depth: 1,
            },
        };
        unsafe {
            device.cmd_copy_buffer_to_image(
                cmd,
                eq_staging,
                eq_image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[eq_copy],
            );
        }
        transition_image_single(
            &device,
            cmd,
            eq_image,
            color_subresource(0, 1, 0, 1),
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );

        // 4b. Transition envCube to GENERAL for compute write, dispatch equirectToCubeMain.
        transition_image_compute(
            &device,
            cmd,
            image,
            color_subresource(0, mip_levels, 0, 6),
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::GENERAL,
        );
        let push_bytes = |push: &bindings::IblBakePush| unsafe {
            device.cmd_push_constants(
                cmd,
                comp_pipeline_layout,
                vk::ShaderStageFlags::COMPUTE,
                0,
                bytemuck::bytes_of(push),
            );
        };
        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pipe_equirect);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                comp_pipeline_layout,
                0,
                &[comp_sets[0]],
                &[],
            );
            push_bytes(&bindings::IblBakePush {
                face_size,
                roughness: 0.0,
                sample_count: 1,
                _pad: 0,
            });
            let g = face_size.div_ceil(8);
            device.cmd_dispatch(cmd, g, g, 6);
        }

        // Generate mip chain for envCube.
        if mip_levels > 1 {
            transition_image_single(
                &device,
                cmd,
                image,
                color_subresource(0, 1, 0, 6),
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            );
            transition_image_single(
                &device,
                cmd,
                image,
                color_subresource(1, mip_levels - 1, 0, 6),
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            );
            for mip in 1..mip_levels {
                let src_level = mip - 1;
                let src_ext = mip_extent(face_size, face_size, src_level);
                let dst_ext = mip_extent(face_size, face_size, mip);
                for layer in 0..6u32 {
                    let blit = vk::ImageBlit {
                        src_subresource: vk::ImageSubresourceLayers {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            mip_level: src_level,
                            base_array_layer: layer,
                            layer_count: 1,
                        },
                        src_offsets: [
                            vk::Offset3D { x: 0, y: 0, z: 0 },
                            vk::Offset3D {
                                x: src_ext.width as i32,
                                y: src_ext.height as i32,
                                z: 1,
                            },
                        ],
                        dst_subresource: vk::ImageSubresourceLayers {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            mip_level: mip,
                            base_array_layer: layer,
                            layer_count: 1,
                        },
                        dst_offsets: [
                            vk::Offset3D { x: 0, y: 0, z: 0 },
                            vk::Offset3D {
                                x: dst_ext.width as i32,
                                y: dst_ext.height as i32,
                                z: 1,
                            },
                        ],
                    };
                    unsafe {
                        device.cmd_blit_image(
                            cmd,
                            image,
                            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                            image,
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                            &[blit],
                            vk::Filter::LINEAR,
                        );
                    }
                }
                if mip < mip_levels - 1 {
                    transition_image(
                        &device,
                        cmd,
                        image,
                        mip,
                        1,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    );
                }
            }
            transition_image(
                &device,
                cmd,
                image,
                0,
                mip_levels - 1,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            );
            transition_image(
                &device,
                cmd,
                image,
                mip_levels - 1,
                1,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            );
        } else {
            transition_image_compute(
                &device,
                cmd,
                image,
                color_subresource(0, 1, 0, 6),
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            );
        }

        // 4c. Dispatch PBR compute shaders (Full) or upload 1x1 neutral dummies (EnvOnly).
        if let Some((p_irr, p_pre, p_brdf)) = pbr_pipes {
            transition_image_compute(
                &device,
                cmd,
                irradiance_image,
                color_subresource(0, 1, 0, 6),
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::GENERAL,
            );
            transition_image_compute(
                &device,
                cmd,
                prefiltered_image,
                color_subresource(0, pre_mips, 0, 6),
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::GENERAL,
            );
            transition_image_compute(
                &device,
                cmd,
                brdf_image,
                color_subresource(0, 1, 0, 1),
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::GENERAL,
            );

            // Irradiance
            unsafe {
                device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, p_irr);
                device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::COMPUTE,
                    comp_pipeline_layout,
                    0,
                    &[comp_sets[1]],
                    &[],
                );
                push_bytes(&bindings::IblBakePush {
                    face_size: irr_size,
                    roughness: 0.0,
                    sample_count: IRRADIANCE_SAMPLES,
                    _pad: 0,
                });
                let g = irr_size.div_ceil(8);
                device.cmd_dispatch(cmd, g, g, 6);
            }

            // Prefiltered mips
            unsafe {
                device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, p_pre);
            }
            for mip in 0..pre_mips {
                let fs = (pre_size >> mip).max(1);
                let roughness = if pre_mips > 1 {
                    let t = mip as f32 / (pre_mips - 1) as f32;
                    t * t
                } else {
                    0.0
                };
                unsafe {
                    device.cmd_bind_descriptor_sets(
                        cmd,
                        vk::PipelineBindPoint::COMPUTE,
                        comp_pipeline_layout,
                        0,
                        &[comp_sets[2 + mip as usize]],
                        &[],
                    );
                    push_bytes(&bindings::IblBakePush {
                        face_size: fs,
                        roughness,
                        sample_count: PREFILTERED_SAMPLES,
                        _pad: 0,
                    });
                    let g = fs.div_ceil(8);
                    device.cmd_dispatch(cmd, g, g, 6);
                }
            }

            // BRDF LUT
            unsafe {
                device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, p_brdf);
                device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::COMPUTE,
                    comp_pipeline_layout,
                    0,
                    &[comp_sets[2 + pre_mips as usize]],
                    &[],
                );
                push_bytes(&bindings::IblBakePush {
                    face_size: brdf_size,
                    roughness: 0.0,
                    sample_count: BRDF_LUT_SAMPLES,
                    _pad: 0,
                });
                let g = brdf_size.div_ceil(8);
                device.cmd_dispatch(cmd, g, g, 1);
            }

            transition_image_compute(
                &device,
                cmd,
                irradiance_image,
                color_subresource(0, 1, 0, 6),
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            );
            transition_image_compute(
                &device,
                cmd,
                prefiltered_image,
                color_subresource(0, pre_mips, 0, 6),
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            );
            transition_image_compute(
                &device,
                cmd,
                brdf_image,
                color_subresource(0, 1, 0, 1),
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            );
        } else {
            let copy_neutral = |img: vk::Image, buf: vk::Buffer, layers: u32| {
                transition_image_single(
                    &device,
                    cmd,
                    img,
                    color_subresource(0, 1, 0, layers),
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                );
                let copy = vk::BufferImageCopy {
                    buffer_offset: 0,
                    buffer_row_length: 0,
                    buffer_image_height: 0,
                    image_subresource: vk::ImageSubresourceLayers {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        mip_level: 0,
                        base_array_layer: 0,
                        layer_count: layers,
                    },
                    image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
                    image_extent: vk::Extent3D {
                        width: 1,
                        height: 1,
                        depth: 1,
                    },
                };
                unsafe {
                    device.cmd_copy_buffer_to_image(
                        cmd,
                        buf,
                        img,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        &[copy],
                    );
                }
                transition_image_single(
                    &device,
                    cmd,
                    img,
                    color_subresource(0, 1, 0, layers),
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                );
            };
            copy_neutral(irradiance_image, neutral_staging[0].0, 6);
            copy_neutral(prefiltered_image, neutral_staging[1].0, 6);
            copy_neutral(brdf_image, neutral_staging[2].0, 1);
        }

        unsafe { device.end_command_buffer(cmd) }?;
        submit_and_wait(&device, queue, command_pool, cmd).context("IBL GPU bake submit")?;

        // 5. Destroy all temporary equirect & compute objects.
        unsafe {
            for p in temp_pipes {
                device.destroy_pipeline(p, None);
            }
            device.destroy_pipeline_layout(comp_pipeline_layout, None);
            device.destroy_descriptor_set_layout(comp_ds_layout, None);
            device.destroy_descriptor_pool(comp_pool, None);
            for v in temp_views {
                device.destroy_image_view(v, None);
            }
            if dummy_cube_img != vk::Image::null() {
                device.destroy_image(dummy_cube_img, None);
                device.free_memory(dummy_cube_mem, None);
            }
            if dummy_brdf_img != vk::Image::null() {
                device.destroy_image(dummy_brdf_img, None);
                device.free_memory(dummy_brdf_mem, None);
            }
            for (b, m) in neutral_staging {
                device.destroy_buffer(b, None);
                device.free_memory(m, None);
            }
            device.destroy_sampler(eq_sampler, None);
            device.destroy_image_view(eq_view, None);
            device.destroy_image(eq_image, None);
            device.free_memory(eq_memory, None);
            device.destroy_buffer(eq_staging, None);
            device.free_memory(eq_staging_memory, None);
        }

        // 6. Final image views, samplers & set 2 descriptor set.
        let mk_cube_view = |img: vk::Image, mips: u32| -> anyhow::Result<vk::ImageView> {
            Ok(unsafe {
                device.create_image_view(
                    &vk::ImageViewCreateInfo {
                        image: img,
                        view_type: vk::ImageViewType::CUBE,
                        format: vk::Format::R16G16B16A16_SFLOAT,
                        subresource_range: color_subresource(0, mips, 0, 6),
                        ..Default::default()
                    },
                    None,
                )?
            })
        };
        let mk_sampler = |max_lod: f32| -> anyhow::Result<vk::Sampler> {
            Ok(unsafe {
                device.create_sampler(
                    &vk::SamplerCreateInfo {
                        mag_filter: vk::Filter::LINEAR,
                        min_filter: vk::Filter::LINEAR,
                        mipmap_mode: vk::SamplerMipmapMode::LINEAR,
                        address_mode_u: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                        address_mode_v: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                        address_mode_w: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                        min_lod: 0.0,
                        max_lod,
                        ..Default::default()
                    },
                    None,
                )?
            })
        };

        let image_view = mk_cube_view(image, mip_levels)?;
        let sampler = mk_sampler(mip_levels as f32)?;
        let irradiance_image_view = mk_cube_view(irradiance_image, 1)?;
        let irradiance_sampler = mk_sampler(1.0)?;
        let prefiltered_image_view = mk_cube_view(prefiltered_image, pre_mips)?;
        let prefiltered_sampler = mk_sampler((pre_mips - 1) as f32)?;
        let brdf_image_view = unsafe {
            device.create_image_view(
                &vk::ImageViewCreateInfo {
                    image: brdf_image,
                    view_type: vk::ImageViewType::TYPE_2D,
                    format: vk::Format::R16G16_SFLOAT,
                    subresource_range: color_subresource(0, 1, 0, 1),
                    ..Default::default()
                },
                None,
            )?
        };
        let brdf_sampler = mk_sampler(0.0)?;

        let descriptor_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&[vk::DescriptorPoolSize {
                        ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                        descriptor_count: 3,
                    }]),
                None,
            )?
        };
        let descriptor_set = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(&[ds_layout]),
            )?
        }[0];
        unsafe {
            device.update_descriptor_sets(
                &[
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&[vk::DescriptorImageInfo {
                            sampler,
                            image_view,
                            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        }]),
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(1)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&[vk::DescriptorImageInfo {
                            sampler: irradiance_sampler,
                            image_view: irradiance_image_view,
                            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        }]),
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(2)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&[vk::DescriptorImageInfo {
                            sampler: prefiltered_sampler,
                            image_view: prefiltered_image_view,
                            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        }]),
                ],
                &[],
            );
        }

        log::info!(
            "IBL: GPU compute baked {}x{} ({:?}, {}ms)",
            face_size,
            face_size,
            scope,
            t_total.elapsed().as_millis()
        );

        Ok(Self {
            device,
            descriptor_set_layout: ds_layout,
            descriptor_set,
            image,
            image_view,
            sampler,
            memory,
            staging: vk::Buffer::null(),
            staging_memory: vk::DeviceMemory::null(),
            irradiance_image,
            irradiance_image_view,
            irradiance_sampler,
            irradiance_memory,
            irradiance_staging: vk::Buffer::null(),
            irradiance_staging_memory: vk::DeviceMemory::null(),
            prefiltered_image,
            prefiltered_image_view,
            prefiltered_sampler,
            prefiltered_memory,
            prefiltered_staging: vk::Buffer::null(),
            prefiltered_staging_memory: vk::DeviceMemory::null(),
            brdf_image,
            brdf_image_view,
            brdf_sampler,
            brdf_memory,
            brdf_staging: vk::Buffer::null(),
            brdf_staging_memory: vk::DeviceMemory::null(),
            descriptor_pool,
        })
    }

    /// Cheap neutral IBL: create 1×1 legal cubemap / irradiance / prefiltered + BRDF LUT,
    /// **no CPU pre-convolution** (≈0ms). Used for "no lighting" scenes (splash screen UI), only making
    /// forward pass set2 descriptor legal; true IBL built on demand by `GraphRenderer::set_environment`
    /// when loading 3D scene with lighting (full `new` path).
    pub fn new_empty(
        context: Arc<VulkanContext>,
        command_pool: vk::CommandPool,
        queue: vk::Queue,
        ds_layout: vk::DescriptorSetLayout,
    ) -> anyhow::Result<Self> {
        let device = context.device.clone();
        let mem_props = &context.physical_device_memory_properties;
        const F: u32 = 1u32;

        // env / irradiance / prefiltered cube (1x1x6, 1 mip) — neutral gray (splash screen no geometry, not sampled)
        let (image, memory, staging, staging_memory) = alloc_neutral_cube(&device, mem_props, F)?;
        let (irradiance_image, irradiance_memory, irradiance_staging, irradiance_staging_memory) =
            alloc_neutral_cube(&device, mem_props, F)?;
        let (
            prefiltered_image,
            prefiltered_memory,
            prefiltered_staging,
            prefiltered_staging_memory,
        ) = alloc_neutral_cube(&device, mem_props, F)?;

        let texels = (6 * F * F) as usize;
        let neutral = vec![f32_to_f16(0.0f32); texels * 4];
        unsafe {
            let m = device.map_memory(
                staging_memory,
                0,
                (texels * 8) as u64,
                vk::MemoryMapFlags::empty(),
            )? as *mut u16;
            std::ptr::copy_nonoverlapping(neutral.as_ptr(), m, neutral.len());
            device.unmap_memory(staging_memory);
            let m = device.map_memory(
                irradiance_staging_memory,
                0,
                (texels * 8) as u64,
                vk::MemoryMapFlags::empty(),
            )? as *mut u16;
            std::ptr::copy_nonoverlapping(neutral.as_ptr(), m, neutral.len());
            device.unmap_memory(irradiance_staging_memory);
            let m = device.map_memory(
                prefiltered_staging_memory,
                0,
                (texels * 8) as u64,
                vk::MemoryMapFlags::empty(),
            )? as *mut u16;
            std::ptr::copy_nonoverlapping(neutral.as_ptr(), m, neutral.len());
            device.unmap_memory(prefiltered_staging_memory);
        }

        // BRDF LUT (1x1, RG16) — neutral (1,1) fallback.
        let brdf_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::R16G16_SFLOAT)
            .extent(vk::Extent3D {
                width: F,
                height: F,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let brdf_image = unsafe { device.create_image(&brdf_info, None) }?;
        let brdf_mem_req = unsafe { device.get_image_memory_requirements(brdf_image) };
        let brdf_memory = allocate_device_memory(&device, mem_props, brdf_mem_req)?;
        unsafe { device.bind_image_memory(brdf_image, brdf_memory, 0) }?;
        let brdf_staging_size = F as u64 * F as u64 * 4;
        let (brdf_staging, brdf_staging_memory) =
            create_staging_buffer(&device, mem_props, brdf_staging_size)?;
        {
            let mapped = unsafe {
                device.map_memory(
                    brdf_staging_memory,
                    0,
                    brdf_staging_size,
                    vk::MemoryMapFlags::empty(),
                )? as *mut u16
            };
            let buf = vec![f32_to_f16(1.0f32); (F * F * 2) as usize];
            unsafe {
                std::ptr::copy_nonoverlapping(buf.as_ptr(), mapped, buf.len());
                device.unmap_memory(brdf_staging_memory);
            }
        }

        // One-shot upload of 4 images.
        let cmd = allocate_temp_command_buffer(&device, command_pool)?;
        unsafe { device.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default())? };
        let upload_cube = |img: vk::Image, stg: vk::Buffer| {
            transition_image_single(
                &device,
                cmd,
                img,
                color_subresource(0, 1, 0, 6),
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            );
            let copy = vk::BufferImageCopy {
                buffer_offset: 0,
                buffer_row_length: 0,
                buffer_image_height: 0,
                image_subresource: vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 6,
                },
                image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
                image_extent: vk::Extent3D {
                    width: F,
                    height: F,
                    depth: 1,
                },
            };
            unsafe {
                device.cmd_copy_buffer_to_image(
                    cmd,
                    stg,
                    img,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[copy],
                )
            };
            transition_image_single(
                &device,
                cmd,
                img,
                color_subresource(0, 1, 0, 6),
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            );
        };
        upload_cube(image, staging);
        upload_cube(irradiance_image, irradiance_staging);
        upload_cube(prefiltered_image, prefiltered_staging);
        transition_image_single(
            &device,
            cmd,
            brdf_image,
            color_subresource(0, 1, 0, 1),
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
        {
            let copy = vk::BufferImageCopy {
                buffer_offset: 0,
                buffer_row_length: 0,
                buffer_image_height: 0,
                image_subresource: vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                },
                image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
                image_extent: vk::Extent3D {
                    width: F,
                    height: F,
                    depth: 1,
                },
            };
            unsafe {
                device.cmd_copy_buffer_to_image(
                    cmd,
                    brdf_staging,
                    brdf_image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[copy],
                )
            };
        }
        transition_image_single(
            &device,
            cmd,
            brdf_image,
            color_subresource(0, 1, 0, 1),
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );
        unsafe { device.end_command_buffer(cmd) }?;
        submit_and_wait(&device, queue, command_pool, cmd).context("empty IBL upload submit")?;

        // views + samplers (max_lod 1, since only 1 mip)
        let mk_sampler = || unsafe {
            device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::LINEAR)
                        .min_filter(vk::Filter::LINEAR)
                        .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .anisotropy_enable(false)
                        .min_lod(0.0)
                        .max_lod(1.0),
                    None,
                )
                .unwrap()
        };
        let image_view = unsafe {
            device
                .create_image_view(
                    &vk::ImageViewCreateInfo {
                        image,
                        view_type: vk::ImageViewType::CUBE,
                        format: vk::Format::R16G16B16A16_SFLOAT,
                        subresource_range: vk::ImageSubresourceRange {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            base_mip_level: 0,
                            level_count: 1,
                            base_array_layer: 0,
                            layer_count: 6,
                        },
                        ..Default::default()
                    },
                    None,
                )
                .unwrap()
        };
        let sampler = mk_sampler();
        let irradiance_image_view = unsafe {
            device
                .create_image_view(
                    &vk::ImageViewCreateInfo {
                        image: irradiance_image,
                        view_type: vk::ImageViewType::CUBE,
                        format: vk::Format::R16G16B16A16_SFLOAT,
                        subresource_range: vk::ImageSubresourceRange {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            base_mip_level: 0,
                            level_count: 1,
                            base_array_layer: 0,
                            layer_count: 6,
                        },
                        ..Default::default()
                    },
                    None,
                )
                .unwrap()
        };
        let irradiance_sampler = mk_sampler();
        let prefiltered_image_view = unsafe {
            device
                .create_image_view(
                    &vk::ImageViewCreateInfo {
                        image: prefiltered_image,
                        view_type: vk::ImageViewType::CUBE,
                        format: vk::Format::R16G16B16A16_SFLOAT,
                        subresource_range: vk::ImageSubresourceRange {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            base_mip_level: 0,
                            level_count: 1,
                            base_array_layer: 0,
                            layer_count: 6,
                        },
                        ..Default::default()
                    },
                    None,
                )
                .unwrap()
        };
        let prefiltered_sampler = mk_sampler();
        let brdf_image_view = unsafe {
            device
                .create_image_view(
                    &vk::ImageViewCreateInfo {
                        image: brdf_image,
                        view_type: vk::ImageViewType::TYPE_2D,
                        format: vk::Format::R16G16_SFLOAT,
                        subresource_range: vk::ImageSubresourceRange {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            base_mip_level: 0,
                            level_count: 1,
                            base_array_layer: 0,
                            layer_count: 1,
                        },
                        ..Default::default()
                    },
                    None,
                )
                .unwrap()
        };
        let brdf_sampler = mk_sampler();

        // Descriptor set (set2) — reuse shared layout ds_layout.
        let descriptor_pool = unsafe {
            device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&[vk::DescriptorPoolSize {
                            ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                            descriptor_count: 3,
                        }]),
                    None,
                )
                .unwrap()
        };
        let descriptor_set = unsafe {
            device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(descriptor_pool)
                        .set_layouts(&[ds_layout]),
                )
                .unwrap()[0]
        };
        unsafe {
            device.update_descriptor_sets(
                &[
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(0)
                        .dst_array_element(0)
                        .descriptor_count(1)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&[vk::DescriptorImageInfo {
                            sampler,
                            image_view,
                            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        }]),
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(1)
                        .dst_array_element(0)
                        .descriptor_count(1)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&[vk::DescriptorImageInfo {
                            sampler: irradiance_sampler,
                            image_view: irradiance_image_view,
                            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        }]),
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(2)
                        .dst_array_element(0)
                        .descriptor_count(1)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&[vk::DescriptorImageInfo {
                            sampler: prefiltered_sampler,
                            image_view: prefiltered_image_view,
                            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        }]),
                ],
                &[],
            )
        };

        Ok(Self {
            device,
            descriptor_set_layout: ds_layout,
            descriptor_set,
            image,
            image_view,
            sampler,
            memory,
            staging,
            staging_memory,
            irradiance_image,
            irradiance_image_view,
            irradiance_sampler,
            irradiance_memory,
            irradiance_staging,
            irradiance_staging_memory,
            prefiltered_image,
            prefiltered_image_view,
            prefiltered_sampler,
            prefiltered_memory,
            prefiltered_staging,
            prefiltered_staging_memory,
            brdf_image,
            brdf_image_view,
            brdf_sampler,
            brdf_memory,
            brdf_staging,
            brdf_staging_memory,
            descriptor_pool,
        })
    }
}

impl IblResources {
    /// Cubemap image view (for registering into the bindless texture table).
    pub fn image_view(&self) -> vk::ImageView {
        self.image_view
    }

    /// Cubemap sampler (for registering into the bindless texture table).
    pub fn sampler(&self) -> vk::Sampler {
        self.sampler
    }

    /// BRDF LUT image view (for registering into the bindless texture table).
    pub fn brdf_image_view(&self) -> vk::ImageView {
        self.brdf_image_view
    }
}

impl IblResources {
    /// Explicitly destroy all GPU resources. Matches the RAII pattern used by
    /// the rest of the renderer — `GraphRenderer::destroy` calls this so the
    /// device handle is valid regardless of struct field-drop ordering.
    pub fn destroy(&mut self) {
        // Guard: zero out handles so re-entrant Drop (if any) is safe.
        if self.descriptor_set_layout == vk::DescriptorSetLayout::null() {
            return;
        }
        unsafe {
            let free_buf = |b: vk::Buffer, m: vk::DeviceMemory| {
                if b != vk::Buffer::null() {
                    self.device.destroy_buffer(b, None);
                }
                if m != vk::DeviceMemory::null() {
                    self.device.free_memory(m, None);
                }
            };
            // env cube
            self.device.destroy_image_view(self.image_view, None);
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.memory, None);
            free_buf(self.staging, self.staging_memory);
            self.device.destroy_sampler(self.sampler, None);
            // irradiance
            self.device
                .destroy_image_view(self.irradiance_image_view, None);
            self.device.destroy_image(self.irradiance_image, None);
            self.device.free_memory(self.irradiance_memory, None);
            free_buf(self.irradiance_staging, self.irradiance_staging_memory);
            self.device.destroy_sampler(self.irradiance_sampler, None);
            // prefiltered
            self.device
                .destroy_image_view(self.prefiltered_image_view, None);
            self.device.destroy_image(self.prefiltered_image, None);
            self.device.free_memory(self.prefiltered_memory, None);
            free_buf(self.prefiltered_staging, self.prefiltered_staging_memory);
            self.device.destroy_sampler(self.prefiltered_sampler, None);
            // brdf LUT
            self.device.destroy_image_view(self.brdf_image_view, None);
            self.device.destroy_image(self.brdf_image, None);
            self.device.free_memory(self.brdf_memory, None);
            free_buf(self.brdf_staging, self.brdf_staging_memory);
            self.device.destroy_sampler(self.brdf_sampler, None);
            // Descriptor pool (set layout shared and held by GraphRenderer, not destroyed here)
            self.device
                .destroy_descriptor_pool(self.descriptor_pool, None);
        }
        // Null out handles so re-entrant Drop is safe.
        self.descriptor_set_layout = vk::DescriptorSetLayout::null();
        self.image = vk::Image::null();
        self.image_view = vk::ImageView::null();
        self.irradiance_image = vk::Image::null();
        self.irradiance_image_view = vk::ImageView::null();
        self.prefiltered_image = vk::Image::null();
        self.prefiltered_image_view = vk::ImageView::null();
        self.brdf_image = vk::Image::null();
        self.brdf_image_view = vk::ImageView::null();
    }
}

impl Drop for IblResources {
    fn drop(&mut self) {
        self.destroy();
    }
}

#[cfg(feature = "ibl")]
fn upload_u16_to_staging(
    device: &ash::Device,
    staging_memory: vk::DeviceMemory,
    byte_size: u64,
    data: &[u16],
) -> anyhow::Result<()> {
    let mapped =
        unsafe { device.map_memory(staging_memory, 0, byte_size, vk::MemoryMapFlags::empty())? }
            as *mut u16;
    unsafe {
        std::ptr::copy_nonoverlapping(data.as_ptr(), mapped, data.len());
        device.unmap_memory(staging_memory);
    }
    Ok(())
}
#[cfg(feature = "ibl")]
fn transition_image_compute(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    subresource: vk::ImageSubresourceRange,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
) {
    let (src_stage, src_access, dst_stage, dst_access) = match (old_layout, new_layout) {
        (vk::ImageLayout::UNDEFINED, vk::ImageLayout::GENERAL) => (
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::AccessFlags::empty(),
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::AccessFlags::SHADER_WRITE,
        ),
        (vk::ImageLayout::GENERAL, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL) => (
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::AccessFlags::SHADER_WRITE,
            vk::PipelineStageFlags::FRAGMENT_SHADER | vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::AccessFlags::SHADER_READ,
        ),
        _ => (
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::AccessFlags::MEMORY_WRITE,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
        ),
    };
    let barrier = vk::ImageMemoryBarrier {
        old_layout,
        new_layout,
        src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
        dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
        image,
        subresource_range: subresource,
        src_access_mask: src_access,
        dst_access_mask: dst_access,
        ..Default::default()
    };
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            src_stage,
            dst_stage,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
    }
}

#[cfg(feature = "ibl")]
fn mip_extent(width: u32, height: u32, level: u32) -> vk::Extent3D {
    vk::Extent3D {
        width: (width >> level).max(1),
        height: (height >> level).max(1),
        depth: 1,
    }
}

#[cfg(feature = "ibl")]
fn transition_image(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    base_mip: u32,
    level_count: u32,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
) {
    let (src_stage, src_access, dst_stage, dst_access) = match (old_layout, new_layout) {
        (vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL) => (
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::AccessFlags::empty(),
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::TRANSFER_WRITE,
        ),
        (
            vk::ImageLayout::GENERAL,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL | vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        ) => (
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::AccessFlags::SHADER_WRITE,
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::TRANSFER_READ | vk::AccessFlags::TRANSFER_WRITE,
        ),
        (vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::TRANSFER_SRC_OPTIMAL) => (
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::TRANSFER_WRITE,
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::TRANSFER_READ,
        ),
        (_, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL) => (
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::TRANSFER_WRITE | vk::AccessFlags::TRANSFER_READ,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::AccessFlags::SHADER_READ,
        ),
        _ => (
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::AccessFlags::empty(),
            vk::PipelineStageFlags::BOTTOM_OF_PIPE,
            vk::AccessFlags::empty(),
        ),
    };

    let barrier = vk::ImageMemoryBarrier {
        old_layout,
        new_layout,
        src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
        dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
        image,
        subresource_range: vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: base_mip,
            level_count,
            base_array_layer: 0,
            layer_count: 6,
        },
        src_access_mask: src_access,
        dst_access_mask: dst_access,
        ..Default::default()
    };
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            src_stage,
            dst_stage,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        )
    };
}

/// Build a COLOR `ImageSubresourceRange` for [`transition_image_single`].
fn color_subresource(
    base_mip: u32,
    level_count: u32,
    base_layer: u32,
    layer_count: u32,
) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .base_mip_level(base_mip)
        .level_count(level_count)
        .base_array_layer(base_layer)
        .layer_count(layer_count)
}

/// Transition a single image with an explicit subresource range (for
/// non-cube images).
fn transition_image_single(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    subresource: vk::ImageSubresourceRange,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
) {
    let (src_stage, src_access, dst_stage, dst_access) = match (old_layout, new_layout) {
        (vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL) => (
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::AccessFlags::empty(),
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::TRANSFER_WRITE,
        ),
        (
            vk::ImageLayout::GENERAL,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL | vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        ) => (
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::AccessFlags::SHADER_WRITE,
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::TRANSFER_READ | vk::AccessFlags::TRANSFER_WRITE,
        ),
        (_, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL) => (
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::TRANSFER_WRITE,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::AccessFlags::SHADER_READ,
        ),
        _ => (
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::AccessFlags::empty(),
            vk::PipelineStageFlags::BOTTOM_OF_PIPE,
            vk::AccessFlags::empty(),
        ),
    };

    let barrier = vk::ImageMemoryBarrier {
        old_layout,
        new_layout,
        src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
        dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
        image,
        subresource_range: subresource,
        src_access_mask: src_access,
        dst_access_mask: dst_access,
        ..Default::default()
    };
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            src_stage,
            dst_stage,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        )
    };
}

fn allocate_temp_command_buffer(
    device: &ash::Device,
    pool: vk::CommandPool,
) -> anyhow::Result<vk::CommandBuffer> {
    let cmd = unsafe {
        device.allocate_command_buffers(&vk::CommandBufferAllocateInfo {
            command_pool: pool,
            level: vk::CommandBufferLevel::PRIMARY,
            command_buffer_count: 1,
            ..Default::default()
        })
    }?[0];
    Ok(cmd)
}

fn submit_and_wait(
    device: &ash::Device,
    queue: vk::Queue,
    pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
) -> anyhow::Result<()> {
    let cmd_arr = [cmd];
    let submit = vk::SubmitInfo::default().command_buffers(&cmd_arr);
    let fence = unsafe {
        device
            .create_fence(&vk::FenceCreateInfo::default(), None)
            .context("IBL submit_and_wait: create fence")?
    };
    unsafe {
        device
            .queue_submit(queue, &[submit], fence)
            .context("IBL submit_and_wait: queue_submit")?;
        device
            .wait_for_fences(&[fence], true, u64::MAX)
            .context("IBL submit_and_wait: wait_for_fences")?;
        device.destroy_fence(fence, None);
        device.free_command_buffers(pool, &cmd_arr);
    }
    Ok(())
}

fn find_memory_type(
    props: &vk::PhysicalDeviceMemoryProperties,
    type_filter: u32,
    flags: vk::MemoryPropertyFlags,
) -> u32 {
    for i in 0..props.memory_type_count {
        if (type_filter & (1 << i)) != 0
            && props.memory_types[i as usize]
                .property_flags
                .contains(flags)
        {
            return i;
        }
    }
    for i in 0..props.memory_type_count {
        if (type_filter & (1 << i)) != 0 {
            return i;
        }
    }
    panic!("no suitable memory type found");
}

fn allocate_device_memory(
    device: &ash::Device,
    mem_props: &vk::PhysicalDeviceMemoryProperties,
    req: vk::MemoryRequirements,
) -> anyhow::Result<vk::DeviceMemory> {
    let mem_type = find_memory_type(
        mem_props,
        req.memory_type_bits,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    );
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
    Ok(memory)
}

fn create_staging_buffer(
    device: &ash::Device,
    mem_props: &vk::PhysicalDeviceMemoryProperties,
    size: u64,
) -> anyhow::Result<(vk::Buffer, vk::DeviceMemory)> {
    let buffer = unsafe {
        device.create_buffer(
            &vk::BufferCreateInfo {
                size,
                usage: vk::BufferUsageFlags::TRANSFER_SRC,
                ..Default::default()
            },
            None,
        )
    }?;
    let req = unsafe { device.get_buffer_memory_requirements(buffer) };
    let mem_type = find_memory_type(
        mem_props,
        req.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    );
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
    unsafe { device.bind_buffer_memory(buffer, memory, 0) }?;
    Ok((buffer, memory))
}
