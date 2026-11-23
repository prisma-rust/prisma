//! Vulkan context: instance, physical device, logical device, queues.
//!
//! Holds long-lived Vulkan handles needed before any rendering happens.
//! Swapchain and per-frame resources live in [`crate::swapchain`] and [`crate::renderer`].

use std::ffi::{c_char, CStr, CString};
use std::os::raw::c_void;

use ash::vk;

use crate::capabilities::{self, RayTracingCaps};

/// Validation layers requested in debug build / when loader has them.
const VALIDATION_LAYERS: [&str; 1] = ["VK_LAYER_KHRONOS_validation"];

/// When set, debug build with no validation layer fails directly, rather than silently degrading.
///
/// CI uses it to turn "validation truly enabled" into a hard gate; local without installed layer need not set it.
const REQUIRE_VALIDATION_ENV: &str = "PRISM_REQUIRE_VALIDATION";

/// Who owns the context-behind Vulkan objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextOwnership {
    /// Created by [`VulkanContext::new`], destroyed with context.
    Owned,
    /// Created by host (editor UI framework) and shared with renderer; **not destroyed** on context drop.
    External,
}

/// All the long-lived Vulkan state the renderer needs to do anything.
pub struct VulkanContext {
    pub entry: ash::Entry,
    pub instance: ash::Instance,
    pub physical_device: vk::PhysicalDevice,
    pub device: ash::Device,

    /// Queue family index that supports both graphics and presentation.
    pub graphics_queue_family: u32,
    pub graphics_queue: vk::Queue,

    /// Properties of the chosen physical device, kept for swapchain queries.
    pub physical_device_properties: vk::PhysicalDeviceProperties,
    pub physical_device_memory_properties: vk::PhysicalDeviceMemoryProperties,

    /// Probed ray-tracing capabilities of the chosen physical device.
    /// All fields are `false` on non-RT hardware; callers can branch freely.
    pub rt_caps: RayTracingCaps,

    /// Probed Qualcomm Adreno Neural Fusion capabilities (#1161).
    #[cfg(feature = "anf")]
    pub anf_caps: crate::anf::AnfCaps,

    /// Whether `fillModeNonSolid` available (needed for Wireframe pipeline).
    pub wireframe_supported: bool,

    /// Acceleration structure function pointers (loaded when the
    /// `VK_KHR_acceleration_structure` extension is enabled; `None` otherwise).
    pub acceleration_structure_fn: Option<ash::khr::acceleration_structure::Device>,

    /// Device extension names that were actually enabled (RT extensions are
    /// conditional; the rest are always-on). Stored for later RT modules.
    enabled_extensions: Vec<CString>,

    /// Ownership: `Owned` destroys device/instance on drop, `External` leaves to host.
    ownership: ContextOwnership,

    /// Whether `ash::Entry` was loaded through `sl.interposer` (Streamline Vulkan hooks active).
    ///
    /// Only the Streamline runtime reads it, so it exists only with the `dlss` brick.
    #[cfg(any(feature = "dlss", feature = "nrd"))]
    pub(crate) streamline_interposed: bool,

    _debug_messenger: Option<vk::DebugUtilsMessengerEXT>,
}

impl VulkanContext {
    /// Whether this context truly enabled Vulkan validation layer / debug messenger.
    ///
    /// Debug build tries to enable it; if system lacks layer, this returns `false`.
    /// CI under `PRISMA_REQUIRE_VALIDATION=1` asserts it is `true`.
    pub fn validation_enabled(&self) -> bool {
        self._debug_messenger.is_some()
    }

    /// Create the instance and device.
    ///
    /// `window_extensions` are the instance extensions the surface needs
    /// (obtained via [`ash_window::enumerate_required_extensions`]).
    pub fn new(window_extensions: &[&str]) -> anyhow::Result<Self> {
        use anyhow::Context as _;
        #[cfg(not(any(feature = "dlss", feature = "nrd")))]
        let entry = unsafe { ash::Entry::load() }.context("failed to load Vulkan loader")?;
        #[cfg(all(feature = "nrd", not(feature = "dlss")))]
        let (entry, streamline_interposed) = (
            unsafe { ash::Entry::load() }.context("failed to load Vulkan loader")?,
            false,
        );
        #[cfg(feature = "dlss")]
        let (entry, streamline_interposed) = {
            let interposer_path = crate::streamline::StreamlineRuntime::pre_init_interposer();
            if let Some(path) = interposer_path.as_deref() {
                match unsafe { ash::Entry::load_from(path) } {
                    Ok(entry) => (entry, true),
                    Err(err) => {
                        log::warn!("ash::Entry::load_from({path}) failed ({err}), falling back to system Vulkan loader");
                        (
                            unsafe { ash::Entry::load() }
                                .context("failed to load Vulkan loader")?,
                            false,
                        )
                    }
                }
            } else {
                (
                    unsafe { ash::Entry::load() }.context("failed to load Vulkan loader")?,
                    false,
                )
            }
        };

        let enable_debug = cfg!(debug_assertions);
        let instance = create_instance(&entry, window_extensions, enable_debug)?;
        let debug_messenger = if enable_debug {
            setup_debug_messenger(&entry, &instance)
        } else {
            None
        };

        let physical_device = pick_physical_device(&instance)?;
        let physical_device_properties =
            unsafe { instance.get_physical_device_properties(physical_device) };
        let physical_device_memory_properties =
            unsafe { instance.get_physical_device_memory_properties(physical_device) };

        let rt_caps = unsafe { capabilities::probe(&instance, physical_device) };
        log::info!("RT capabilities: {rt_caps}");
        #[cfg(feature = "anf")]
        #[cfg(feature = "anf")]
        let anf_caps = crate::anf::AnfCaps::probe(&physical_device_properties);
        let wireframe_supported = unsafe { instance.get_physical_device_features(physical_device) }
            .fill_mode_non_solid
            != 0;

        let graphics_queue_family = pick_graphics_queue_family(&instance, physical_device)
            .context("no graphics-capable queue family found")?;

        let (device, enabled_extensions) = create_device(
            &instance,
            physical_device,
            graphics_queue_family,
            &rt_caps,
            !window_extensions.is_empty(),
            &[],
        )?;

        let graphics_queue = unsafe { device.get_device_queue(graphics_queue_family, 0) };

        let acceleration_structure_fn = if rt_caps.acceleration_structure {
            Some(ash::khr::acceleration_structure::Device::new(
                &instance, &device,
            ))
        } else {
            None
        };

        Ok(Self {
            entry,
            instance,
            physical_device,
            device,
            graphics_queue_family,
            graphics_queue,
            physical_device_properties,
            physical_device_memory_properties,
            rt_caps,
            #[cfg(feature = "anf")]
            anf_caps,
            wireframe_supported,
            acceleration_structure_fn,
            enabled_extensions,
            ownership: ContextOwnership::Owned,
            #[cfg(any(feature = "dlss", feature = "nrd"))]
            streamline_interposed,
            _debug_messenger: debug_messenger,
        })
    }

    /// For host (editor / UI framework) that already built instance and physical device, but wants renderer and
    /// UI to **share same `VkDevice`**.
    ///
    /// Device still created by this function with same feature set as [`VulkanContext::new`] (bindless,
    /// timeline semaphores, synchronization2 prerequisites all here), host only needs to additionally
    /// declare its device extensions (e.g. `VK_KHR_swapchain`).
    ///
    /// Ownership: `instance` and `device` belong to host, context drop **will not** destroy them;
    /// host must destroy device/instance only after renderer fully destroyed.
    pub fn from_host(
        entry: ash::Entry,
        instance: ash::Instance,
        physical_device: vk::PhysicalDevice,
        graphics_queue_family: u32,
        extra_device_extensions: &[&str],
    ) -> anyhow::Result<Self> {
        let physical_device_properties =
            unsafe { instance.get_physical_device_properties(physical_device) };
        let physical_device_memory_properties =
            unsafe { instance.get_physical_device_memory_properties(physical_device) };

        let rt_caps = unsafe { capabilities::probe(&instance, physical_device) };
        log::info!("RT capabilities (host device): {rt_caps}");
        #[cfg(feature = "anf")]
        #[cfg(feature = "anf")]
        let anf_caps = crate::anf::AnfCaps::probe(&physical_device_properties);
        let wireframe_supported = unsafe { instance.get_physical_device_features(physical_device) }
            .fill_mode_non_solid
            != 0;

        let (device, enabled_extensions) = create_device(
            &instance,
            physical_device,
            graphics_queue_family,
            &rt_caps,
            true,
            extra_device_extensions,
        )?;
        let graphics_queue = unsafe { device.get_device_queue(graphics_queue_family, 0) };

        let acceleration_structure_fn = if rt_caps.acceleration_structure {
            Some(ash::khr::acceleration_structure::Device::new(
                &instance, &device,
            ))
        } else {
            None
        };

        Ok(Self {
            entry,
            instance,
            physical_device,
            device,
            graphics_queue_family,
            graphics_queue,
            physical_device_properties,
            physical_device_memory_properties,
            rt_caps,
            #[cfg(feature = "anf")]
            anf_caps,
            wireframe_supported,
            acceleration_structure_fn,
            enabled_extensions,
            ownership: ContextOwnership::External,
            #[cfg(any(feature = "dlss", feature = "nrd"))]
            streamline_interposed: false,
            _debug_messenger: None,
        })
    }

    /// Name an object in the debug layer (no-op outside debug builds / no layer).
    pub fn name_object(&self, ty: vk::ObjectType, handle: u64, name: &str) {
        if self._debug_messenger.is_some() {
            let ext = ash::ext::debug_utils::Device::new(&self.instance, &self.device);
            let name_c = CString::new(name).unwrap();
            let info = vk::DebugUtilsObjectNameInfoEXT {
                s_type: vk::StructureType::DEBUG_UTILS_OBJECT_NAME_INFO_EXT,
                p_next: std::ptr::null(),
                object_type: ty,
                object_handle: handle,
                p_object_name: name_c.as_ptr(),
                _marker: std::marker::PhantomData,
            };
            unsafe {
                let _ = ext.set_debug_utils_object_name(&info);
            }
        }
    }

    /// Names of the device extensions that were enabled at device creation.
    /// `VK_KHR_swapchain` is included only for windowed contexts (a headless
    /// context like the GI baker enables no surface, so swapchain is omitted);
    /// RT extensions are included when the hardware supports them. Used by RT
    /// modules to decide which code path to take.
    pub fn enabled_extension_names(&self) -> &[CString] {
        &self.enabled_extensions
    }

    /// Convenience: was a specific device extension enabled?
    pub fn has_extension(&self, name: &CStr) -> bool {
        self.enabled_extensions.iter().any(|c| c.as_c_str() == name)
    }

    /// `deviceName` of the adapter this context runs on, or `None` when the
    /// driver leaves it empty. Used to label benchmark reports (#1002).
    pub fn adapter_name(&self) -> Option<String> {
        let raw = &self.physical_device_properties.device_name;
        let name = unsafe { CStr::from_ptr(raw.as_ptr()) }.to_string_lossy();
        let name = name.trim();
        (!name.is_empty()).then(|| name.to_owned())
    }
}

impl Drop for VulkanContext {
    fn drop(&mut self) {
        if self.ownership == ContextOwnership::External {
            return;
        }
        unsafe {
            if let Some(messenger) = self._debug_messenger.take() {
                let ext = ash::ext::debug_utils::Instance::new(&self.entry, &self.instance);
                ext.destroy_debug_utils_messenger(messenger, None);
            }
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

/// Create Vulkan instance for **host** (editor UI framework needs own instance to create
/// surface / swapchain, while also sharing device with renderer).
///
/// Extension list given by host (usually `ash_window::enumerate_required_extensions`
/// result); debug/validation policy consistent with [`VulkanContext::new`].
pub fn create_host_instance(
    entry: &ash::Entry,
    extensions: &[&str],
) -> anyhow::Result<(ash::Instance, Option<vk::DebugUtilsMessengerEXT>)> {
    let enable_debug = cfg!(debug_assertions);
    let instance = create_instance(entry, extensions, enable_debug)?;
    let messenger = if enable_debug {
        setup_debug_messenger(entry, &instance)
    } else {
        None
    };
    Ok((instance, messenger))
}

/// Select a physical device that can present to `surface` and has a graphics queue.
///
/// Renderer requires `maxPushConstantsSize >= 176` (PathTracePass), checked here too,
/// to avoid host building device then renderer init failing.
pub fn pick_host_physical_device(
    instance: &ash::Instance,
    surface_loader: &ash::khr::surface::Instance,
    surface: vk::SurfaceKHR,
) -> anyhow::Result<vk::PhysicalDevice> {
    pick_physical_device_impl(instance, |device| {
        pick_present_queue_family(instance, device, surface_loader, surface).is_some()
    })
}

/// Queue family supporting both graphics and presentation (host sharing device can only use same queue family).
pub fn pick_present_queue_family(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    surface_loader: &ash::khr::surface::Instance,
    surface: vk::SurfaceKHR,
) -> Option<u32> {
    let families = unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
    for (index, family) in families.iter().enumerate() {
        if !family.queue_flags.contains(vk::QueueFlags::GRAPHICS) {
            continue;
        }
        let supported = unsafe {
            surface_loader.get_physical_device_surface_support(
                physical_device,
                index as u32,
                surface,
            )
        }
        .unwrap_or(false);
        if supported {
            return Some(index as u32);
        }
    }
    None
}

fn validation_required() -> bool {
    match std::env::var(REQUIRE_VALIDATION_ENV) {
        Ok(value) => !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false"),
        Err(_) => false,
    }
}

fn create_instance(
    entry: &ash::Entry,
    window_extensions: &[&str],
    enable_debug: bool,
) -> anyhow::Result<ash::Instance> {
    use anyhow::Context as _;

    let instance_version = unsafe { entry.try_enumerate_instance_version() }
        .ok()
        .flatten()
        .unwrap_or(vk::API_VERSION_1_1);
    let api_version = if instance_version >= vk::API_VERSION_1_2 {
        vk::API_VERSION_1_2
    } else {
        vk::API_VERSION_1_1
    };

    let app_info = vk::ApplicationInfo::default()
        .application_name(c"Prisma")
        .application_version(vk::make_api_version(0, 0, 1, 0))
        .engine_name(c"Prisma")
        .engine_version(vk::make_api_version(0, 0, 1, 0))
        .api_version(api_version);

    let mut extension_names: Vec<CString> = window_extensions
        .iter()
        .map(|s| CString::new(*s).unwrap())
        .collect();
    if enable_debug {
        extension_names.push(vk::EXT_DEBUG_UTILS_NAME.into());
    }
    let extension_ptrs: Vec<*const c_char> = extension_names.iter().map(|c| c.as_ptr()).collect();

    let mut enabled_layers: Vec<CString> = Vec::new();
    if enable_debug {
        let available = unsafe { entry.enumerate_instance_layer_properties() }
            .map(|props| {
                props.iter().any(|p| {
                    let name = unsafe { CStr::from_ptr(p.layer_name.as_ptr()) };
                    VALIDATION_LAYERS.contains(&name.to_str().unwrap_or(""))
                })
            })
            .unwrap_or(false);
        if available {
            enabled_layers = VALIDATION_LAYERS
                .iter()
                .map(|s| CString::new(*s).unwrap())
                .collect();
        } else if validation_required() {
            anyhow::bail!(
                "{REQUIRE_VALIDATION_ENV} is set but {} is not installed",
                VALIDATION_LAYERS[0]
            );
        } else {
            log::warn!("validation layers requested but not available; continuing without");
        }
    }
    let layer_ptrs: Vec<*const c_char> = enabled_layers.iter().map(|c| c.as_ptr()).collect();

    let mut create_info = vk::InstanceCreateInfo::default()
        .application_info(&app_info)
        .enabled_extension_names(&extension_ptrs);
    if enable_debug {
        create_info = create_info.enabled_layer_names(&layer_ptrs);
    }

    let instance = unsafe { entry.create_instance(&create_info, None) }
        .context("failed to create Vulkan instance")?;

    Ok(instance)
}

fn pick_physical_device(instance: &ash::Instance) -> anyhow::Result<vk::PhysicalDevice> {
    pick_physical_device_impl(instance, |_| true)
}

fn pick_physical_device_impl(
    instance: &ash::Instance,
    accept: impl Fn(vk::PhysicalDevice) -> bool,
) -> anyhow::Result<vk::PhysicalDevice> {
    use anyhow::Context as _;
    let devices = unsafe { instance.enumerate_physical_devices() }
        .context("failed to enumerate physical devices")?;

    let mut best = None;
    let mut best_score = -1i32;
    for device in devices {
        if !accept(device) {
            continue;
        }
        let props = unsafe { instance.get_physical_device_properties(device) };
        let score = match props.device_type {
            vk::PhysicalDeviceType::DISCRETE_GPU => 3,
            vk::PhysicalDeviceType::INTEGRATED_GPU => 2,
            vk::PhysicalDeviceType::VIRTUAL_GPU => 1,
            _ => 0,
        };
        if pick_graphics_queue_family(instance, device).is_some() && score > best_score {
            best_score = score;
            best = Some(device);
        }
    }
    let device = best.ok_or_else(|| anyhow::anyhow!("no GPU with a graphics queue found"))?;
    let props = unsafe { instance.get_physical_device_properties(device) };
    let max_pc = props.limits.max_push_constants_size;
    anyhow::ensure!(
        max_pc >= 176,
        "selected GPU only supports {} bytes of push constants; PT pass needs 176",
        max_pc
    );
    Ok(device)
}

fn pick_graphics_queue_family(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
) -> Option<u32> {
    let families = unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
    for (i, family) in families.iter().enumerate() {
        if family.queue_flags.contains(vk::QueueFlags::GRAPHICS) {
            return Some(i as u32);
        }
    }
    None
}

fn create_device(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    graphics_queue_family: u32,
    rt_caps: &RayTracingCaps,
    has_surface: bool,
    extra_device_extensions: &[&str],
) -> anyhow::Result<(ash::Device, Vec<CString>)> {
    use anyhow::Context as _;
    let priorities = [1.0f32];
    let queue_create_infos = [vk::DeviceQueueCreateInfo::default()
        .queue_family_index(graphics_queue_family)
        .queue_priorities(&priorities)];

    let available_features = unsafe { instance.get_physical_device_features(physical_device) };
    let mut legacy_features = vk::PhysicalDeviceFeatures {
        shader_clip_distance: available_features.shader_clip_distance,
        independent_blend: available_features.independent_blend,
        fill_mode_non_solid: available_features.fill_mode_non_solid,
        shader_storage_image_write_without_format: available_features
            .shader_storage_image_write_without_format,
        ..vk::PhysicalDeviceFeatures::default()
    };

    let mut enabled_extensions: Vec<CString> = Vec::new();
    if has_surface {
        enabled_extensions.push(ash::khr::swapchain::NAME.into());
    }
    if rt_caps.synchronization2 {
        enabled_extensions.push(ash::khr::synchronization2::NAME.into());
    }
    if rt_caps.copy_commands2 {
        enabled_extensions.push(ash::khr::copy_commands2::NAME.into());
    }
    if let Ok(ext_props) =
        unsafe { instance.enumerate_device_extension_properties(physical_device) }
    {
        for optional_ext in [
            ash::khr::push_descriptor::NAME,
            c"VK_NVX_binary_import",
            c"VK_NVX_image_view_handle",
        ] {
            let has_ext = ext_props.iter().any(|p| {
                let name = unsafe { std::ffi::CStr::from_ptr(p.extension_name.as_ptr()) };
                name == optional_ext
            });
            if has_ext {
                enabled_extensions.push(optional_ext.into());
            }
        }
    }
    for rt_ext in capabilities::rt_extension_names(rt_caps) {
        enabled_extensions.push(rt_ext.into());
    }
    for name in extra_device_extensions {
        let already = enabled_extensions
            .iter()
            .any(|existing| existing.to_str().map(|s| s == *name).unwrap_or(false));
        if !already {
            enabled_extensions.push(CString::new(*name).expect("extension name"));
        }
    }
    let extension_ptrs: Vec<*const c_char> =
        enabled_extensions.iter().map(|c| c.as_ptr()).collect();

    let mut vk11 = vk::PhysicalDeviceVulkan11Features::default();
    let mut vk12 = vk::PhysicalDeviceVulkan12Features::default();
    let mut accel_features = vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default();
    let mut rt_pipeline_features = vk::PhysicalDeviceRayTracingPipelineFeaturesKHR::default();
    let mut ray_query_features = vk::PhysicalDeviceRayQueryFeaturesKHR::default();
    let mut sync2_features = vk::PhysicalDeviceSynchronization2FeaturesKHR::default();

    vk11.shader_draw_parameters = vk::TRUE;

    if rt_caps.buffer_device_address {
        vk12.buffer_device_address = vk::TRUE;
    }
    if rt_caps.descriptor_indexing {
        vk12.descriptor_indexing = vk::TRUE;
        vk12.runtime_descriptor_array = vk::TRUE;
        vk12.descriptor_binding_partially_bound = vk::TRUE;
        vk12.descriptor_binding_sampled_image_update_after_bind = vk::TRUE;
        vk12.descriptor_binding_variable_descriptor_count = vk::TRUE;
        vk12.shader_sampled_image_array_non_uniform_indexing = vk::TRUE;
        vk12.descriptor_binding_storage_image_update_after_bind = vk::TRUE;
        vk12.descriptor_binding_storage_buffer_update_after_bind = vk::TRUE;
    }
    if rt_caps.timeline_semaphore {
        vk12.timeline_semaphore = vk::TRUE;
    }
    if rt_caps.shader_buffer_int64_atomics {
        vk12.shader_buffer_int64_atomics = vk::TRUE;
    }
    if rt_caps.shader_int64 {
        legacy_features.shader_int64 = vk::TRUE;
    }
    if rt_caps.shader_float16 {
        vk12.shader_float16 = vk::TRUE;
    }
    if rt_caps.uniform_and_storage_buffer_16bit_access {
        vk11.uniform_and_storage_buffer16_bit_access = vk::TRUE;
    }
    if rt_caps.storage_buffer_16bit_access {
        vk11.storage_buffer16_bit_access = vk::TRUE;
    }

    if rt_caps.synchronization2 {
        sync2_features.synchronization2 = vk::TRUE;
    }
    let mut features2 = vk::PhysicalDeviceFeatures2::default()
        .features(legacy_features)
        .push_next(&mut vk11)
        .push_next(&mut vk12)
        .push_next(&mut sync2_features);

    if rt_caps.acceleration_structure {
        accel_features.acceleration_structure = vk::TRUE;
        accel_features.descriptor_binding_acceleration_structure_update_after_bind = vk::TRUE;
        features2 = features2.push_next(&mut accel_features);
    }
    if rt_caps.ray_tracing_pipeline {
        rt_pipeline_features.ray_tracing_pipeline = vk::TRUE;
        features2 = features2.push_next(&mut rt_pipeline_features);
    }
    if rt_caps.ray_query {
        ray_query_features.ray_query = vk::TRUE;
        features2 = features2.push_next(&mut ray_query_features);
    }

    let create_info = vk::DeviceCreateInfo::default()
        .queue_create_infos(&queue_create_infos)
        .enabled_extension_names(&extension_ptrs)
        .push_next(&mut features2);

    let device = unsafe { instance.create_device(physical_device, &create_info, None) }
        .context("failed to create logical device")?;

    if rt_caps.any_ray_tracing() {
        log::info!(
            "device created with ray tracing: pipeline={} query={}",
            rt_caps.has_rt_pipeline(),
            rt_caps.has_ray_query()
        );
    } else {
        log::info!("device created (no ray tracing support)");
    }

    Ok((device, enabled_extensions))
}

fn setup_debug_messenger(
    entry: &ash::Entry,
    instance: &ash::Instance,
) -> Option<vk::DebugUtilsMessengerEXT> {
    let available = unsafe { entry.enumerate_instance_layer_properties() }
        .ok()?
        .iter()
        .any(|p| {
            let name = unsafe { CStr::from_ptr(p.layer_name.as_ptr()) };
            name == c"VK_LAYER_KHRONOS_validation"
        });
    if !available {
        log::warn!("validation layers requested but not available");
        return None;
    }

    let ext = ash::ext::debug_utils::Instance::new(entry, instance);
    let create_info = vk::DebugUtilsMessengerCreateInfoEXT::default()
        .message_severity(
            vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
                | vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
        )
        .message_type(
            vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
        )
        .pfn_user_callback(Some(debug_callback));

    Some(
        unsafe { ext.create_debug_utils_messenger(&create_info, None) }
            .expect("failed to create debug messenger despite layer being available"),
    )
}

unsafe extern "system" fn debug_callback(
    message_severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _message_type: vk::DebugUtilsMessageTypeFlagsEXT,
    p_callback_data: *const vk::DebugUtilsMessengerCallbackDataEXT,
    _p_user_data: *mut c_void,
) -> vk::Bool32 {
    let message = if p_callback_data.is_null() {
        String::from("(no message)")
    } else {
        let data = unsafe { &*p_callback_data };
        unsafe { CStr::from_ptr(data.p_message) }
            .to_string_lossy()
            .into_owned()
    };

    if message_severity >= vk::DebugUtilsMessageSeverityFlagsEXT::ERROR {
        log::error!("[validation] {message}");
    } else if message_severity >= vk::DebugUtilsMessageSeverityFlagsEXT::WARNING {
        log::warn!("[validation] {message}");
    }
    vk::FALSE
}
