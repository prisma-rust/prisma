//! Hardware ray tracing capability detection.
//!
//! Before creating the logical device, we probe physical device ray tracing support:
//! which extensions are already advertised, and whether feature chains actually report support.
//! Result ([`RayTracingCaps`]) drives conditional extension / feature enable in [`crate::context`].
//!
//! Detection is layered:
//!
//! ```text
//! Layer 4 VK_KHR_ray_query for inline rays in any shader stage
//! Layer 3 VK_KHR_ray_tracing_pipeline RT core pipeline (full SBT)
//! Layer 2 VK_KHR_acceleration_structure + deferred_host_operations
//! Layer 1 Vulkan 1.2 promoted features (buffer_device_address, descriptor_indexing, timeline_semaphore)
//! ```
//!
//! An extension is only considered *usable* when **both** the extension is
//! advertised by the driver **and** the corresponding feature struct reports
//! it as supported (`vkGetPhysicalDeviceFeatures2`).

use std::collections::HashSet;
use std::ffi::CStr;

use ash::vk;

/// Hardware execution tier for the unified virtual geometry pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VirtualGeometryTier {
    /// Desktop RTX 40/50 & RDNA4 class: Mesh Shader + 64-bit SW raster + CLAS/DMM RT.
    NextGenClasMeshSw,
    /// Flagship mobile (Adreno 750/850+, Immortalis-G925) and modern discrete GPUs.
    FlagshipMeshSw,
    /// Strong TBDR mobile devices preferring tile-local VisBuffer subpass resolve.
    TileLocalSubpass,
    /// Baseline Vulkan 1.2 fallback using indirect indexed draws and 32-bit VisBuffer.
    #[default]
    IndirectIndexedFallback,
}

/// Probed device capabilities controlling virtual geometry raster and RT paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VirtualGeometryCaps {
    pub mesh_shader: bool,
    pub cluster_acceleration_structure: bool,
    pub displacement_micromap: bool,
    pub opacity_micromap: bool,
    pub shader_tile_image: bool,
    pub int64_atomics: bool,
    pub subgroup_arithmetic: bool,
    pub tier: VirtualGeometryTier,
}

/// Resolves virtual geometry capabilities from advertised device extensions and RT/subgroup caps.
pub fn probe_virtual_geometry_caps(
    available_extensions: &HashSet<String>,
    rt_caps: &RayTracingCaps,
    subgroup_arithmetic: bool,
) -> VirtualGeometryCaps {
    let mesh_shader = has_extension(available_extensions, "VK_EXT_mesh_shader");
    let cluster_acceleration_structure =
        has_extension(available_extensions, "VK_NV_cluster_acceleration_structure");
    let displacement_micromap = has_extension(available_extensions, "VK_NV_displacement_micromap");
    let opacity_micromap = has_extension(available_extensions, "VK_EXT_opacity_micromap");
    let shader_tile_image = has_extension(available_extensions, "VK_EXT_shader_tile_image");
    let int64_atomics = rt_caps.shader_buffer_int64_atomics;

    let tier = if mesh_shader && int64_atomics && cluster_acceleration_structure {
        VirtualGeometryTier::NextGenClasMeshSw
    } else if mesh_shader && int64_atomics {
        VirtualGeometryTier::FlagshipMeshSw
    } else if shader_tile_image {
        VirtualGeometryTier::TileLocalSubpass
    } else {
        VirtualGeometryTier::IndirectIndexedFallback
    };

    VirtualGeometryCaps {
        mesh_shader,
        cluster_acceleration_structure,
        displacement_micromap,
        opacity_micromap,
        shader_tile_image,
        int64_atomics,
        subgroup_arithmetic,
        tier,
    }
}

/// Result of probing one physical device for ray-tracing capabilities.
///
/// Every field is `false` / zero when the feature is absent, so callers can
/// unconditionally branch on these flags without risking a panic on non-RT
/// hardware.
#[derive(Debug, Clone, Default)]
pub struct RayTracingCaps {
    /// Physical device supports Vulkan 1.2 API (`api_version >= 1.2`).
    pub vulkan_1_2: bool,
    /// `bufferDeviceAddress` available (required by acceleration structures).
    pub buffer_device_address: bool,
    /// `descriptorIndexing` available (used by RT descriptor layouts).
    pub descriptor_indexing: bool,
    /// `timelineSemaphore` available (useful for long-running AS builds).
    pub timeline_semaphore: bool,
    pub synchronization2: bool,
    pub copy_commands2: bool,
    /// `shaderBufferInt64Atomics` available (64-bit atomics, used by RTXGI SHARC).
    pub shader_buffer_int64_atomics: bool,
    /// `shaderInt64` available. RTXGI's hash-grid key packing (`HashGridKey`)
    /// is a `uint64_t`, so a shader module that declares the SPIR-V `Int64`
    /// capability cannot be created without this feature.
    pub shader_int64: bool,
    /// `shaderFloat16` available. SHARC packs cached radiance as `float16_t4`.
    pub shader_float16: bool,
    /// `storageBuffer16BitAccess` available. The packed `SharcPackedData`
    /// storage buffer holds 16-bit components, which needs this feature in
    /// addition to `shaderFloat16`.
    pub storage_buffer_16bit_access: bool,
    /// `uniformAndStorageBuffer16BitAccess` available. Slang emits this
    /// capability (alongside `StorageBuffer16BitAccess`) for the SHARC cache
    /// buffers, and Vulkan 1.1 wants both feature bits enabled to match.
    pub uniform_and_storage_buffer_16bit_access: bool,

    /// `VK_KHR_acceleration_structure` extension + feature available.
    pub acceleration_structure: bool,
    /// `VK_KHR_deferred_host_operations` available (AS build dependency).
    pub deferred_host_operations: bool,

    /// `VK_KHR_ray_tracing_pipeline` extension + feature available.
    pub ray_tracing_pipeline: bool,

    /// `VK_KHR_ray_query` extension + feature available.
    pub ray_query: bool,

    pub max_recursion_depth: u32,
    pub shader_group_handle_size: u32,
    pub max_shader_group_stride: u32,
    pub shader_group_base_alignment: u32,
    pub max_ray_dispatch_invocation_count: u32,
    pub shader_group_handle_alignment: u32,
    pub max_ray_hit_attribute_size: u32,
    /// Probed virtual geometry hardware execution tier and optional extensions.
    pub virtual_geometry: VirtualGeometryCaps,
}

impl RayTracingCaps {
    /// Convenience: is *any* ray-tracing path available?
    /// True when acceleration structures are present and at least one of
    /// the ray-tracing-pipeline or ray-query layers is usable.
    pub fn any_ray_tracing(&self) -> bool {
        self.acceleration_structure && (self.ray_tracing_pipeline || self.ray_query)
    }

    /// The full RT-core pipeline path (BLAS/TLAS + SBT + rgen/rmiss/rchit).
    pub fn has_rt_pipeline(&self) -> bool {
        self.acceleration_structure && self.ray_tracing_pipeline
    }

    /// The lighter ray-query path (inline `traceRayEXT` in fragment shaders).
    pub fn has_ray_query(&self) -> bool {
        self.acceleration_structure && self.ray_query
    }

    /// Capabilities required by texture upload path. When missing, device creation should fail,
    /// to avoid triggering invalid Vulkan commands later in upload phase.
    pub fn has_transfer_commands(&self) -> bool {
        self.synchronization2 && self.copy_commands2
    }
}

impl std::fmt::Display for RayTracingCaps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "vulkan_1_2={} buffer_device_address={} descriptor_indexing={} \
             timeline_semaphore={} synchronization2={} copy_commands2={} accel_struct={} deferred_host={} \
             rt_pipeline={} ray_query={}",
            self.vulkan_1_2,
            self.buffer_device_address,
            self.descriptor_indexing,
            self.timeline_semaphore,
            self.synchronization2,
            self.copy_commands2,
            self.acceleration_structure,
            self.deferred_host_operations,
            self.ray_tracing_pipeline,
            self.ray_query,
        )?;
        if self.ray_tracing_pipeline {
            write!(
                f,
                " [max_recursion={} handle_size={} stride={} base_align={}]",
                self.max_recursion_depth,
                self.shader_group_handle_size,
                self.max_shader_group_stride,
                self.shader_group_base_alignment,
            )?;
        }
        Ok(())
    }
}

/// Collect all device extension names advertised by the physical device.
///
/// Pure function over the enumerated properties; kept separate from
/// [`probe`] so it can be unit-tested with synthetic data.
pub fn collect_extension_names(props: &[vk::ExtensionProperties]) -> HashSet<String> {
    props
        .iter()
        .map(|p| {
            unsafe { CStr::from_ptr(p.extension_name.as_ptr()) }
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

/// Does the advertised extension set contain `name`?
pub fn has_extension(available: &HashSet<String>, name: &str) -> bool {
    available.contains(name)
}

/// The set of device extensions that should be enabled for ray tracing,
/// given the probed capabilities. Always includes `VK_KHR_swapchain`
/// (the caller already adds it). Returns the RT-specific extension names
/// as `&'static CStr` for direct use in the extension pointer array.
pub fn rt_extension_names(caps: &RayTracingCaps) -> Vec<&'static CStr> {
    let mut names = Vec::new();
    if caps.acceleration_structure {
        names.push(vk::EXT_DESCRIPTOR_INDEXING_NAME);
        names.push(vk::KHR_BUFFER_DEVICE_ADDRESS_NAME);
        names.push(vk::KHR_ACCELERATION_STRUCTURE_NAME);
        names.push(vk::KHR_DEFERRED_HOST_OPERATIONS_NAME);
    }
    if caps.ray_tracing_pipeline {
        names.push(vk::KHR_SPIRV_1_4_NAME);
        names.push(vk::KHR_SHADER_FLOAT_CONTROLS_NAME);
        names.push(vk::KHR_RAY_TRACING_PIPELINE_NAME);
        names.push(vk::KHR_PIPELINE_LIBRARY_NAME);
    }
    if caps.ray_query {
        names.push(vk::KHR_RAY_QUERY_NAME);
    }
    names
}

/// Probe a physical device for ray-tracing capabilities.
///
/// # Safety
///
/// `instance` and `physical_device` must be valid Vulkan handles obtained
/// from a loaded `ash::Entry`.
pub unsafe fn probe(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
) -> RayTracingCaps {
    let props = unsafe { instance.get_physical_device_properties(physical_device) };
    let vulkan_1_2 = props.api_version >= vk::API_VERSION_1_2;
    let vulkan_1_3 = props.api_version >= vk::API_VERSION_1_3;

    let ext_props = unsafe {
        instance
            .enumerate_device_extension_properties(physical_device)
            .unwrap_or_default()
    };
    let available = collect_extension_names(&ext_props);

    let has_accel_ext = has_extension(
        &available,
        vk::KHR_ACCELERATION_STRUCTURE_NAME.to_str().unwrap(),
    );
    let has_rt_pipeline_ext = has_extension(
        &available,
        vk::KHR_RAY_TRACING_PIPELINE_NAME.to_str().unwrap(),
    );
    let has_ray_query_ext = has_extension(&available, vk::KHR_RAY_QUERY_NAME.to_str().unwrap());
    let has_deferred_ext = has_extension(
        &available,
        vk::KHR_DEFERRED_HOST_OPERATIONS_NAME.to_str().unwrap(),
    );
    let has_descriptor_indexing_ext = has_extension(
        &available,
        vk::EXT_DESCRIPTOR_INDEXING_NAME.to_str().unwrap(),
    );
    let has_buffer_device_address_ext = has_extension(
        &available,
        vk::KHR_BUFFER_DEVICE_ADDRESS_NAME.to_str().unwrap(),
    );
    let has_spirv_1_4_ext = has_extension(&available, vk::KHR_SPIRV_1_4_NAME.to_str().unwrap());
    let has_shader_float_controls_ext = has_extension(
        &available,
        vk::KHR_SHADER_FLOAT_CONTROLS_NAME.to_str().unwrap(),
    );

    let mut vk11 = vk::PhysicalDeviceVulkan11Features::default();
    let mut vk12 = vk::PhysicalDeviceVulkan12Features::default();
    let mut accel_features = vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default();
    let mut rt_pipeline_features = vk::PhysicalDeviceRayTracingPipelineFeaturesKHR::default();
    let mut ray_query_features = vk::PhysicalDeviceRayQueryFeaturesKHR::default();

    let mut features2 = vk::PhysicalDeviceFeatures2::default()
        .push_next(&mut vk11)
        .push_next(&mut vk12);

    if has_accel_ext {
        features2 = features2.push_next(&mut accel_features);
    }
    if has_rt_pipeline_ext {
        features2 = features2.push_next(&mut rt_pipeline_features);
    }
    if has_ray_query_ext {
        features2 = features2.push_next(&mut ray_query_features);
    }

    unsafe { instance.get_physical_device_features2(physical_device, &mut features2) };

    let buffer_device_address = vk12.buffer_device_address == vk::TRUE;
    let descriptor_indexing = vk12.descriptor_indexing == vk::TRUE;
    let timeline_semaphore = vk12.timeline_semaphore == vk::TRUE;
    let shader_buffer_int64_atomics = vk12.shader_buffer_int64_atomics == vk::TRUE;
    let shader_float16 = vk12.shader_float16 == vk::TRUE;
    let storage_buffer_16bit_access = vk11.storage_buffer16_bit_access == vk::TRUE;
    let uniform_and_storage_buffer_16bit_access =
        vk11.uniform_and_storage_buffer16_bit_access == vk::TRUE;
    let legacy_features = unsafe { instance.get_physical_device_features(physical_device) };
    let shader_int64 = legacy_features.shader_int64 == vk::TRUE;

    let acceleration_structure = has_accel_ext
        && has_deferred_ext
        && has_descriptor_indexing_ext
        && has_buffer_device_address_ext
        && accel_features.acceleration_structure == vk::TRUE;
    let deferred_host_operations = has_deferred_ext;

    let ray_tracing_pipeline = has_rt_pipeline_ext
        && has_spirv_1_4_ext
        && has_shader_float_controls_ext
        && rt_pipeline_features.ray_tracing_pipeline == vk::TRUE;
    let ray_query = has_ray_query_ext
        && has_spirv_1_4_ext
        && has_shader_float_controls_ext
        && ray_query_features.ray_query == vk::TRUE;

    let mut subgroup_props = vk::PhysicalDeviceSubgroupProperties::default();
    let mut rt_props = vk::PhysicalDeviceRayTracingPipelinePropertiesKHR::default();
    let mut props2 = vk::PhysicalDeviceProperties2::default();
    if props.api_version >= vk::API_VERSION_1_1 {
        props2 = props2.push_next(&mut subgroup_props);
    }
    if ray_tracing_pipeline {
        props2 = props2.push_next(&mut rt_props);
    }
    if props.api_version >= vk::API_VERSION_1_1 || ray_tracing_pipeline {
        unsafe { instance.get_physical_device_properties2(physical_device, &mut props2) };
    }
    let subgroup_arithmetic = props.api_version >= vk::API_VERSION_1_1
        && subgroup_props
            .supported_operations
            .contains(vk::SubgroupFeatureFlags::BALLOT | vk::SubgroupFeatureFlags::ARITHMETIC);

    let mut caps = RayTracingCaps {
        vulkan_1_2,
        buffer_device_address,
        descriptor_indexing,
        timeline_semaphore,
        synchronization2: vulkan_1_3
            || has_extension(&available, vk::KHR_SYNCHRONIZATION2_NAME.to_str().unwrap()),
        copy_commands2: vulkan_1_3
            || has_extension(&available, vk::KHR_COPY_COMMANDS2_NAME.to_str().unwrap()),
        shader_buffer_int64_atomics,
        shader_int64,
        shader_float16,
        storage_buffer_16bit_access,
        uniform_and_storage_buffer_16bit_access,
        acceleration_structure,
        deferred_host_operations,
        ray_tracing_pipeline,
        ray_query,
        max_recursion_depth: rt_props.max_ray_recursion_depth,
        shader_group_handle_size: rt_props.shader_group_handle_size,
        max_shader_group_stride: rt_props.max_shader_group_stride,
        shader_group_base_alignment: rt_props.shader_group_base_alignment,
        max_ray_dispatch_invocation_count: rt_props.max_ray_dispatch_invocation_count,
        shader_group_handle_alignment: rt_props.shader_group_handle_alignment,
        max_ray_hit_attribute_size: rt_props.max_ray_hit_attribute_size,
        virtual_geometry: VirtualGeometryCaps::default(),
    };
    caps.virtual_geometry = probe_virtual_geometry_caps(&available, &caps, subgroup_arithmetic);
    caps
}

#[cfg(test)]
#[path = "capabilities_tests.rs"]
mod tests;
