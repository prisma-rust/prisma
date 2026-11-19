//! NVIDIA Streamline 2.x dynamic loader and ABI contract (DLSS-SR, DLSS-RR, DLSS-G).
//!
//! Mirrors the official Streamline 2.14.16 C++ headers (`sl.h`, `sl_core_api.h`,
//! `sl_core_types.h`, `sl_dlss.h`, `sl_dlss_d.h`, `sl_version.h`) and loads
//! `sl.interposer` dynamically at runtime so hosts without the SDK degrade
//! gracefully without link-time dependencies.

use std::ffi::{c_char, c_void, CStr};

use ash::vk;

pub const NVIDIA_VENDOR_ID: u32 = 0x10DE;

pub const SL_VERSION_MAJOR: u64 = 2;
pub const SL_VERSION_MINOR: u64 = 14;
pub const SL_VERSION_PATCH: u64 = 16;
pub const SL_SDK_VERSION_MAGIC: u64 = 0xFEDC;
pub const SL_SDK_VERSION: u64 = (SL_VERSION_MAJOR << 48)
    | (SL_VERSION_MINOR << 32)
    | (SL_VERSION_PATCH << 16)
    | SL_SDK_VERSION_MAGIC;

pub const INVALID_UINT: u32 = 0xFFFF_FFFF;
pub const INVALID_FLOAT: f32 = f32::MAX;

pub type Feature = u32;
pub const FEATURE_DLSS: Feature = 0;
pub const FEATURE_REFLEX: Feature = 3;
pub const FEATURE_DLSS_G: Feature = 1000;
pub const FEATURE_DLSS_RR: Feature = 1001;
pub const FEATURE_DLSS_NR: Feature = 1004;

pub type BufferType = u32;
pub const BUFFER_TYPE_DEPTH: BufferType = 0;
pub const BUFFER_TYPE_MOTION_VECTORS: BufferType = 1;
pub const BUFFER_TYPE_HUD_LESS_COLOR: BufferType = 2;
pub const BUFFER_TYPE_SCALING_INPUT_COLOR: BufferType = 3;
pub const BUFFER_TYPE_SCALING_OUTPUT_COLOR: BufferType = 4;
pub const BUFFER_TYPE_NORMALS: BufferType = 5;
pub const BUFFER_TYPE_ROUGHNESS: BufferType = 6;
pub const BUFFER_TYPE_ALBEDO: BufferType = 7;
pub const BUFFER_TYPE_SPECULAR_ALBEDO: BufferType = 8;
pub const BUFFER_TYPE_INDIRECT_ALBEDO: BufferType = 9;
pub const BUFFER_TYPE_NORMAL_ROUGHNESS: BufferType = 14;
pub const BUFFER_TYPE_DIFFUSE_HIT_NOISY: BufferType = 15;
pub const BUFFER_TYPE_SPECULAR_HIT_NOISY: BufferType = 17;
pub const BUFFER_TYPE_SPECULAR_HIT_DISTANCE: BufferType = 42;

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlResult(pub i32);

#[allow(non_upper_case_globals)]
impl SlResult {
    pub const Ok: Self = Self(0);
    pub const ErrorIO: Self = Self(1);
    pub const ErrorDriverOutOfDate: Self = Self(2);
    pub const ErrorOsOutOfDate: Self = Self(3);
    pub const ErrorOsDisabledHws: Self = Self(4);
    pub const ErrorDeviceNotCreated: Self = Self(5);
    pub const ErrorNoSupportedAdapterFound: Self = Self(6);
    pub const ErrorAdapterNotSupported: Self = Self(7);
    pub const ErrorNoPlugins: Self = Self(8);
    pub const ErrorVulkanApi: Self = Self(9);
    pub const ErrorNotInitialized: Self = Self(21);
    pub const ErrorFeatureMissing: Self = Self(29);
    pub const ErrorFeatureNotSupported: Self = Self(30);
    pub const ErrorFeatureMissingHooks: Self = Self(31);
    pub const ErrorFeatureFailedToLoad: Self = Self(32);
    pub const ErrorFeatureWrongPriority: Self = Self(33);
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SlBoolean {
    #[default]
    False = 0,
    True = 1,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StructType {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct BaseStructure {
    pub next: *mut BaseStructure,
    pub struct_type: StructType,
    pub struct_version: usize,
}

impl BaseStructure {
    pub const fn new(struct_type: StructType, struct_version: usize) -> Self {
        Self {
            next: std::ptr::null_mut(),
            struct_type,
            struct_version,
        }
    }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DlssMode {
    #[default]
    Off = 0,
    MaxPerformance = 1,
    Balanced = 2,
    MaxQuality = 3,
    UltraPerformance = 4,
    UltraQuality = 5,
    Dlaa = 6,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DlssPreset {
    #[default]
    Default = 0,
    PresetJ = 10,
    PresetK = 11,
    PresetL = 12,
    PresetM = 13,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DlssdPreset {
    #[default]
    Default = 0,
    PresetD = 4,
    PresetE = 5,
    PresetF = 6,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DlssdNormalRoughnessMode {
    #[default]
    Unpacked = 0,
    Packed = 1,
}

pub const DLSS_OPTIONS_GUID: StructType = StructType {
    data1: 0x6ac8_26e4,
    data2: 0x4c61,
    data3: 0x4101,
    data4: [0xa9, 0x2d, 0x63, 0x8d, 0x42, 0x10, 0x57, 0xb8],
};

pub const DLSSD_OPTIONS_GUID: StructType = StructType {
    data1: 0x0ad8_7504,
    data2: 0x774e,
    data3: 0x4bf3,
    data4: [0x96, 0x33, 0xa4, 0x4d, 0x1f, 0x7f, 0x9c, 0xb8],
};

pub const DLSSG_OPTIONS_GUID: StructType = StructType {
    data1: 0xfac5_f1cb,
    data2: 0x2dfd,
    data3: 0x4f36,
    data4: [0xa1, 0xe6, 0x3a, 0x9e, 0x86, 0x52, 0x56, 0xc5],
};

pub const PREFERENCES_GUID: StructType = StructType {
    data1: 0x1ca1_0965,
    data2: 0xbf8e,
    data3: 0x432b,
    data4: [0x8d, 0xa1, 0x67, 0x16, 0xd8, 0x79, 0xfb, 0x14],
};

pub const ADAPTER_INFO_GUID: StructType = StructType {
    data1: 0x0677_315f,
    data2: 0xa746,
    data3: 0x4492,
    data4: [0x9f, 0x42, 0xcb, 0x61, 0x42, 0xc9, 0xc3, 0xd4],
};

pub const VULKAN_INFO_GUID: StructType = StructType {
    data1: 0x0eed_6fd5,
    data2: 0x82cd,
    data3: 0x43a9,
    data4: [0xbd, 0xb5, 0x47, 0xa5, 0xba, 0x2f, 0x45, 0xd6],
};

pub const VIEWPORT_HANDLE_GUID: StructType = StructType {
    data1: 0x171b_6435,
    data2: 0x9b3c,
    data3: 0x4fc8,
    data4: [0x99, 0x94, 0xfb, 0xe5, 0x25, 0x69, 0xaa, 0xa4],
};

pub const RESOURCE_GUID: StructType = StructType {
    data1: 0x3a9d_70cf,
    data2: 0x2418,
    data3: 0x4b72,
    data4: [0x83, 0x91, 0x13, 0xf8, 0x72, 0x1c, 0x72, 0x61],
};

pub const RESOURCE_TAG_GUID: StructType = StructType {
    data1: 0x4c6a_5aad,
    data2: 0xb445,
    data3: 0x496c,
    data4: [0x87, 0xff, 0x1a, 0xf3, 0x84, 0x5b, 0xe6, 0x53],
};

pub const CONSTANTS_GUID: StructType = StructType {
    data1: 0xdcd3_5ad7,
    data2: 0x4e4a,
    data3: 0x4bad,
    data4: [0xa9, 0x0c, 0xe0, 0xc4, 0x9e, 0xb2, 0x3a, 0xfe],
};

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DlssgMode {
    #[default]
    Off = 0,
    On = 1,
    Auto = 2,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DlssgOptions {
    pub base: BaseStructure,
    pub mode: DlssgMode,
    pub num_frames_to_generate: u32,
    pub flags: u32,
    pub dynamic_res_width: u32,
    pub dynamic_res_height: u32,
    pub num_back_buffers: u32,
    pub mvec_depth_width: u32,
    pub mvec_depth_height: u32,
    pub color_width: u32,
    pub color_height: u32,
    pub color_buffer_format: u32,
    pub mvec_buffer_format: u32,
    pub depth_buffer_format: u32,
    pub hud_less_buffer_format: u32,
    pub ui_buffer_format: u32,
    pub on_error_callback: *const c_void,
    pub b_reserved15: u8,
    pub _pad_reserved: [u8; 3],
    pub queue_parallelism_mode: u32,
    pub enable_ui_recomposition: u8,
    pub _pad_ui: [u8; 3],
    pub dynamic_target_frame_rate: f32,
}

impl Default for DlssgOptions {
    fn default() -> Self {
        Self {
            base: BaseStructure::new(DLSSG_OPTIONS_GUID, 5),
            mode: DlssgMode::Off,
            num_frames_to_generate: 1,
            flags: 0,
            dynamic_res_width: 0,
            dynamic_res_height: 0,
            num_back_buffers: 0,
            mvec_depth_width: 0,
            mvec_depth_height: 0,
            color_width: 0,
            color_height: 0,
            color_buffer_format: 0,
            mvec_buffer_format: 0,
            depth_buffer_format: 0,
            hud_less_buffer_format: 0,
            ui_buffer_format: 0,
            on_error_callback: std::ptr::null(),
            b_reserved15: 2, // eInvalid
            _pad_reserved: [0; 3],
            queue_parallelism_mode: 0,
            enable_ui_recomposition: 0,
            _pad_ui: [0; 3],
            dynamic_target_frame_rate: 0.0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SlPreferences {
    pub base: BaseStructure,
    pub show_console: bool,
    pub log_level: u32,
    pub paths_to_plugins: *const *const u16,
    pub num_paths_to_plugins: u32,
    pub path_to_logs_and_data: *const u16,
    pub allocate_callback: *const c_void,
    pub release_callback: *const c_void,
    pub log_message_callback: *const c_void,
    pub flags: u64,
    pub features_to_load: *const Feature,
    pub num_features_to_load: u32,
    pub application_id: u32,
    pub engine: u32,
    pub engine_version: *const c_char,
    pub project_id: *const c_char,
    pub render_api: u32, // 0 = D3D11, 1 = D3D12, 2 = Vulkan
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SlAdapterInfo {
    pub base: BaseStructure,
    pub device_luid: *mut u8,
    pub device_luid_size_in_bytes: u32,
    pub vk_physical_device: *mut c_void,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SlVulkanInfo {
    pub base: BaseStructure,
    pub device: vk::Device,
    pub instance: vk::Instance,
    pub physical_device: vk::PhysicalDevice,
    pub compute_queue_index: u32,
    pub compute_queue_family: u32,
    pub graphics_queue_index: u32,
    pub graphics_queue_family: u32,
    pub optical_flow_queue_index: u32,
    pub optical_flow_queue_family: u32,
    pub use_native_optical_flow_mode: bool,
    pub compute_queue_create_flags: u32,
    pub graphics_queue_create_flags: u32,
    pub optical_flow_queue_create_flags: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SlViewportHandle {
    pub base: BaseStructure,
    pub value: u32,
}

impl SlViewportHandle {
    pub const fn new(value: u32) -> Self {
        Self {
            base: BaseStructure::new(VIEWPORT_HANDLE_GUID, 1),
            value,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SlExtent {
    pub top: u32,
    pub left: u32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SlResource {
    pub base: BaseStructure,
    pub res_type: u32, // 0 = Tex2D
    pub native: *mut c_void,
    pub memory: *mut c_void,
    pub view: *mut c_void,
    pub state: u32, // VkImageLayout
    pub width: u32,
    pub height: u32,
    pub native_format: u32,
    pub mip_levels: u32,
    pub array_layers: u32,
    pub gpu_virtual_address: u64,
    pub flags: u32,
    pub usage: u32,
    pub internal_flags: u16,
    pub reserved: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SlResourceTag {
    pub base: BaseStructure,
    pub resource: *mut SlResource,
    pub buffer_type: BufferType,
    pub lifecycle: u32, // 0 = OnlyValidNow, 1 = ValidUntilPresent, 2 = ValidUntilEvaluate
    pub extent: SlExtent,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SlConstants {
    pub base: BaseStructure,
    pub camera_view_to_clip: [[f32; 4]; 4],
    pub clip_to_camera_view: [[f32; 4]; 4],
    pub clip_to_lens_clip: [[f32; 4]; 4],
    pub clip_to_prev_clip: [[f32; 4]; 4],
    pub prev_clip_to_clip: [[f32; 4]; 4],
    pub jitter_offset: [f32; 2],
    pub mvec_scale: [f32; 2],
    pub camera_pinhole_offset: [f32; 2],
    pub camera_pos: [f32; 3],
    pub camera_up: [f32; 3],
    pub camera_right: [f32; 3],
    pub camera_fwd: [f32; 3],
    pub camera_near: f32,
    pub camera_far: f32,
    pub camera_fov: f32,
    pub camera_aspect_ratio: f32,
    pub motion_vectors_invalid_value: f32,
    pub depth_inverted: u8,
    pub camera_motion_included: u8,
    pub motion_vectors_3d: u8,
    pub reset: u8,
    pub orthographic_projection: u8,
    pub motion_vectors_dilated: u8,
    pub motion_vectors_jittered: u8,
    pub min_relative_linear_depth_object_separation: f32,
}

impl Default for SlConstants {
    fn default() -> Self {
        let id = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        Self {
            base: BaseStructure::new(CONSTANTS_GUID, 2),
            camera_view_to_clip: id,
            clip_to_camera_view: id,
            clip_to_lens_clip: id,
            clip_to_prev_clip: id,
            prev_clip_to_clip: id,
            jitter_offset: [0.0, 0.0],
            mvec_scale: [1.0, 1.0],
            camera_pinhole_offset: [0.0, 0.0],
            camera_pos: [0.0; 3],
            camera_up: [0.0, 1.0, 0.0],
            camera_right: [1.0, 0.0, 0.0],
            camera_fwd: [0.0, 0.0, -1.0],
            camera_near: 0.1,
            camera_far: 1000.0,
            camera_fov: 1.0,
            camera_aspect_ratio: 16.0 / 9.0,
            motion_vectors_invalid_value: INVALID_FLOAT,
            depth_inverted: 0,
            camera_motion_included: 1,
            motion_vectors_3d: 0,
            reset: 0,
            orthographic_projection: 0,
            motion_vectors_dilated: 0,
            motion_vectors_jittered: 0,
            min_relative_linear_depth_object_separation: 40.0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DlssOptions {
    pub base: BaseStructure,
    pub mode: DlssMode,
    pub output_width: u32,
    pub output_height: u32,
    pub sharpness: f32,
    pub pre_exposure: f32,
    pub exposure_scale: f32,
    pub color_buffers_hdr: SlBoolean,
    pub indicator_invert_axis_x: SlBoolean,
    pub indicator_invert_axis_y: SlBoolean,
    pub dlaa_preset: DlssPreset,
    pub quality_preset: DlssPreset,
    pub balanced_preset: DlssPreset,
    pub performance_preset: DlssPreset,
    pub ultra_performance_preset: DlssPreset,
    pub ultra_quality_preset: DlssPreset,
    pub use_auto_exposure: SlBoolean,
}

impl Default for DlssOptions {
    fn default() -> Self {
        Self {
            base: BaseStructure::new(DLSS_OPTIONS_GUID, 3),
            mode: DlssMode::Off,
            output_width: INVALID_UINT,
            output_height: INVALID_UINT,
            sharpness: 0.0,
            pre_exposure: 1.0,
            exposure_scale: 1.0,
            color_buffers_hdr: SlBoolean::True,
            indicator_invert_axis_x: SlBoolean::False,
            indicator_invert_axis_y: SlBoolean::False,
            dlaa_preset: DlssPreset::Default,
            quality_preset: DlssPreset::Default,
            balanced_preset: DlssPreset::Default,
            performance_preset: DlssPreset::Default,
            ultra_performance_preset: DlssPreset::Default,
            ultra_quality_preset: DlssPreset::Default,
            use_auto_exposure: SlBoolean::False,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DlssdOptions {
    pub base: BaseStructure,
    pub mode: DlssMode,
    pub output_width: u32,
    pub output_height: u32,
    pub sharpness: f32,
    pub pre_exposure: f32,
    pub exposure_scale: f32,
    pub color_buffers_hdr: SlBoolean,
    pub indicator_invert_axis_x: SlBoolean,
    pub indicator_invert_axis_y: SlBoolean,
    pub normal_roughness_mode: DlssdNormalRoughnessMode,
    pub world_to_camera_view: [[f32; 4]; 4],
    pub camera_view_to_world: [[f32; 4]; 4],
    pub alpha_upscaling_enabled: SlBoolean,
    pub dlaa_preset: DlssdPreset,
    pub quality_preset: DlssdPreset,
    pub balanced_preset: DlssdPreset,
    pub performance_preset: DlssdPreset,
    pub ultra_performance_preset: DlssdPreset,
    pub ultra_quality_preset: DlssdPreset,
}

impl Default for DlssdOptions {
    fn default() -> Self {
        let identity = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        Self {
            base: BaseStructure::new(DLSSD_OPTIONS_GUID, 3),
            mode: DlssMode::Off,
            output_width: INVALID_UINT,
            output_height: INVALID_UINT,
            sharpness: 0.0,
            pre_exposure: 1.0,
            exposure_scale: 1.0,
            color_buffers_hdr: SlBoolean::True,
            indicator_invert_axis_x: SlBoolean::False,
            indicator_invert_axis_y: SlBoolean::False,
            normal_roughness_mode: DlssdNormalRoughnessMode::Unpacked,
            world_to_camera_view: identity,
            camera_view_to_world: identity,
            alpha_upscaling_enabled: SlBoolean::False,
            dlaa_preset: DlssdPreset::Default,
            quality_preset: DlssdPreset::Default,
            balanced_preset: DlssdPreset::Default,
            performance_preset: DlssdPreset::Default,
            ultra_performance_preset: DlssdPreset::Default,
            ultra_quality_preset: DlssdPreset::Default,
        }
    }
}

/// Runtime capability summary for NVIDIA Streamline neural rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StreamlineCaps {
    pub is_nvidia: bool,
    pub interposer_loaded: bool,
    pub dlss_sr_supported: bool,
    pub dlss_rr_supported: bool,
    pub dlss_fg_supported: bool,
}

impl StreamlineCaps {
    pub fn probe(properties: &vk::PhysicalDeviceProperties) -> Self {
        let is_nvidia = properties.vendor_id == NVIDIA_VENDOR_ID;
        Self {
            is_nvidia,
            interposer_loaded: false,
            dlss_sr_supported: false,
            dlss_rr_supported: false,
            dlss_fg_supported: false,
        }
    }
}

pub type PFunSlInit = unsafe extern "C" fn(pref: *const c_void, sdk_version: u64) -> SlResult;
pub type PFunSlShutdown = unsafe extern "C" fn() -> SlResult;
pub type PFunSlIsFeatureSupported =
    unsafe extern "C" fn(feature: Feature, adapter_info: *const c_void) -> SlResult;
pub type PFunSlGetNewFrameToken =
    unsafe extern "C" fn(token: *mut *mut c_void, frame_index: *const u32) -> SlResult;
pub type PFunSlSetTagForFrame = unsafe extern "C" fn(
    frame: *const c_void,
    viewport: *const u32,
    tags: *const c_void,
    num_tags: u32,
    cmd_buffer: *mut c_void,
) -> SlResult;
pub type PFunSlEvaluateFeature = unsafe extern "C" fn(
    feature: Feature,
    frame: *const c_void,
    inputs: *const *const BaseStructure,
    num_inputs: u32,
    cmd_buffer: *mut c_void,
) -> SlResult;
pub type PFunSlSetVulkanInfo = unsafe extern "C" fn(info: *const SlVulkanInfo) -> SlResult;
pub type PFunSlSetConstants = unsafe extern "C" fn(
    values: *const SlConstants,
    frame: *const c_void,
    viewport: *const SlViewportHandle,
) -> SlResult;
pub type PFunSlDlssSetOptions = unsafe extern "C" fn(
    viewport: *const SlViewportHandle,
    options: *const DlssOptions,
) -> SlResult;
pub type PFunSlDlssdSetOptions = unsafe extern "C" fn(
    viewport: *const SlViewportHandle,
    options: *const DlssdOptions,
) -> SlResult;
pub type PFunSlDlssgSetOptions = unsafe extern "C" fn(
    viewport: *const SlViewportHandle,
    options: *const DlssgOptions,
) -> SlResult;
pub type PFunSlGetFeatureFunction = unsafe extern "C" fn(
    feature: Feature,
    function_name: *const c_char,
    function: *mut *mut c_void,
) -> SlResult;

struct SharedLib {
    handle: *mut c_void,
}

// SAFETY: Dynamic library handles and resolved entry-point pointers are process-global.
unsafe impl Send for SharedLib {}
// SAFETY: Dynamic library handles and resolved entry-point pointers are process-global.
unsafe impl Sync for SharedLib {}

impl Drop for SharedLib {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            #[cfg(unix)]
            unsafe {
                dlclose(self.handle);
            }
            #[cfg(windows)]
            unsafe {
                FreeLibrary(self.handle);
            }
        }
    }
}

impl SharedLib {
    fn open(path: &str) -> Option<Self> {
        let c_path = std::ffi::CString::new(path).ok()?;
        #[cfg(unix)]
        let handle = unsafe { dlopen(c_path.as_ptr(), 1) }; // RTLD_LAZY
        #[cfg(windows)]
        let handle = unsafe { LoadLibraryA(c_path.as_ptr()) };
        #[cfg(not(any(unix, windows)))]
        let handle: *mut c_void = std::ptr::null_mut();

        if handle.is_null() {
            None
        } else {
            Some(Self { handle })
        }
    }

    unsafe fn get_symbol(&self, symbol: &[u8]) -> Option<*mut c_void> {
        #[cfg(unix)]
        let ptr = dlsym(self.handle, symbol.as_ptr().cast());
        #[cfg(windows)]
        let ptr = GetProcAddress(self.handle, symbol.as_ptr().cast());
        #[cfg(not(any(unix, windows)))]
        let ptr: *mut c_void = std::ptr::null_mut();

        if ptr.is_null() {
            None
        } else {
            Some(ptr)
        }
    }
}

/// Probe whether a shared library can be loaded on the current host.
#[must_use]
pub(crate) fn probe_shared_library(name: &str) -> bool {
    SharedLib::open(name).is_some()
}

#[cfg(unix)]
extern "C" {
    fn dlopen(filename: *const c_char, flag: std::os::raw::c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> std::os::raw::c_int;
}

#[cfg(windows)]
extern "system" {
    fn LoadLibraryA(lp_lib_file_name: *const c_char) -> *mut c_void;
    fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const c_char) -> *mut c_void;
    fn FreeLibrary(h_lib_module: *mut c_void) -> i32;
}

/// Dynamically loaded Streamline 2.x interposer function table.
pub struct StreamlineBinding {
    _library: SharedLib,
    loaded_dir: Option<std::path::PathBuf>,
    pub sl_init: PFunSlInit,
    pub sl_shutdown: PFunSlShutdown,
    pub sl_is_feature_supported: PFunSlIsFeatureSupported,
    pub sl_get_new_frame_token: PFunSlGetNewFrameToken,
    pub sl_set_tag_for_frame: PFunSlSetTagForFrame,
    pub sl_evaluate_feature: PFunSlEvaluateFeature,
    pub sl_get_feature_function: PFunSlGetFeatureFunction,
    pub sl_set_vulkan_info: Option<PFunSlSetVulkanInfo>,
    pub sl_set_constants: Option<PFunSlSetConstants>,
}

// SAFETY: Function pointers inside `StreamlineBinding` point to immutable code pages.
unsafe impl Send for StreamlineBinding {}
// SAFETY: Function pointers inside `StreamlineBinding` point to immutable code pages.
unsafe impl Sync for StreamlineBinding {}

impl StreamlineBinding {
    /// Attempt to load `sl.interposer` from env overrides, `vendor/streamline`, or system paths.
    /// Returns `None` when the Streamline SDK runtime is absent.
    #[expect(
        clippy::disallowed_methods,
        reason = "Streamline SDK DLLs live in workspace vendor/streamline outside asset packs"
    )]
    pub fn try_load() -> Option<Self> {
        let mut candidates: Vec<String> = Vec::new();
        for env_key in ["PRISMA_STREAMLINE_DIR", "STREAMLINE_SDK_DIR"] {
            if let Ok(dir) = std::env::var(env_key) {
                let base = std::path::Path::new(&dir);
                candidates.push(
                    base.join("sl.interposer.dll")
                        .to_string_lossy()
                        .into_owned(),
                );
                candidates.push(
                    base.join("bin/x64/sl.interposer.dll")
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
        candidates.extend([
            "sl.interposer.dll".to_owned(),
            "libsl.interposer.so".to_owned(),
        ]);
        for start in [
            std::env::current_dir().ok(),
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.to_path_buf())),
        ]
        .into_iter()
        .flatten()
        {
            for ancestor in start.ancestors().take(7) {
                let dll = ancestor.join("vendor/streamline/bin/x64/sl.interposer.dll");
                if dll.is_file() {
                    candidates.push(dll.to_string_lossy().into_owned());
                }
            }
        }
        for candidate in &candidates {
            if let Some(binding) = Self::try_load_from(candidate) {
                return Some(binding);
            }
        }
        None
    }

    pub fn try_load_from(path: &str) -> Option<Self> {
        let library = SharedLib::open(path)?;
        let loaded_dir = std::path::Path::new(path)
            .canonicalize()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .or_else(|| {
                std::path::Path::new(path)
                    .parent()
                    .filter(|d| !d.as_os_str().is_empty())
                    .map(|d| d.to_path_buf())
            });
        unsafe {
            let sl_init =
                std::mem::transmute::<*mut c_void, PFunSlInit>(library.get_symbol(b"slInit\0")?);
            let sl_shutdown = std::mem::transmute::<*mut c_void, PFunSlShutdown>(
                library.get_symbol(b"slShutdown\0")?,
            );
            let sl_is_feature_supported =
                std::mem::transmute::<*mut c_void, PFunSlIsFeatureSupported>(
                    library.get_symbol(b"slIsFeatureSupported\0")?,
                );
            let sl_get_new_frame_token = std::mem::transmute::<*mut c_void, PFunSlGetNewFrameToken>(
                library.get_symbol(b"slGetNewFrameToken\0")?,
            );
            let sl_set_tag_for_frame = std::mem::transmute::<*mut c_void, PFunSlSetTagForFrame>(
                library.get_symbol(b"slSetTagForFrame\0")?,
            );
            let sl_evaluate_feature = std::mem::transmute::<*mut c_void, PFunSlEvaluateFeature>(
                library.get_symbol(b"slEvaluateFeature\0")?,
            );
            let sl_get_feature_function =
                std::mem::transmute::<*mut c_void, PFunSlGetFeatureFunction>(
                    library.get_symbol(b"slGetFeatureFunction\0")?,
                );
            let sl_set_vulkan_info = library
                .get_symbol(b"slSetVulkanInfo\0")
                .map(|p| std::mem::transmute::<*mut c_void, PFunSlSetVulkanInfo>(p));
            let sl_set_constants = library
                .get_symbol(b"slSetConstants\0")
                .map(|p| std::mem::transmute::<*mut c_void, PFunSlSetConstants>(p));
            Some(Self {
                _library: library,
                loaded_dir,
                sl_init,
                sl_shutdown,
                sl_is_feature_supported,
                sl_get_new_frame_token,
                sl_set_tag_for_frame,
                sl_evaluate_feature,
                sl_get_feature_function,
                sl_set_vulkan_info,
                sl_set_constants,
            })
        }
    }
}

unsafe extern "C" fn sl_log_callback(log_type: u32, msg: *const c_char) {
    if msg.is_null() {
        return;
    }
    let text = unsafe { CStr::from_ptr(msg) }.to_string_lossy();
    let trimmed = text.trim_end();
    match log_type {
        0 => log::info!("[streamline] {trimmed}"),
        1 => log::warn!("[streamline] {trimmed}"),
        _ => log::error!("[streamline] {trimmed}"),
    }
}

/// Live NVIDIA Streamline runtime bound to a Vulkan device (DLSS-SR, DLSS-RR, DLFG).
pub struct StreamlineRuntime {
    binding: StreamlineBinding,
    caps: StreamlineCaps,
    dlss_set_options: Option<PFunSlDlssSetOptions>,
    dlssd_set_options: Option<PFunSlDlssdSetOptions>,
    dlssg_set_options: Option<PFunSlDlssgSetOptions>,
    viewport: SlViewportHandle,
}

impl StreamlineRuntime {
    /// Probe and initialize Streamline for `context`. Returns `None` on non-NVIDIA
    /// hosts or when `sl.interposer` / required plugins fail to initialize.
    pub fn try_init(context: &crate::context::VulkanContext) -> Option<Self> {
        if context.physical_device_properties.vendor_id != NVIDIA_VENDOR_ID {
            return None;
        }
        let binding = StreamlineBinding::try_load()?;
        let features = [
            FEATURE_DLSS,
            FEATURE_DLSS_RR,
            FEATURE_DLSS_G,
            FEATURE_REFLEX,
        ];
        let plugin_dir_utf16: Option<Vec<u16>> = binding.loaded_dir.as_ref().map(|dir| {
            let mut s = dir.to_string_lossy().into_owned();
            if let Some(stripped) = s.strip_prefix(r"\\?\") {
                s = stripped.to_owned();
            }
            s.encode_utf16().chain(std::iter::once(0)).collect()
        });
        let plugin_dir_ptrs: [*const u16; 1] = [plugin_dir_utf16
            .as_ref()
            .map_or(std::ptr::null(), |v| v.as_ptr())];
        let prefs = SlPreferences {
            base: BaseStructure::new(PREFERENCES_GUID, 1),
            show_console: false,
            log_level: if log::log_enabled!(log::Level::Info) {
                2
            } else {
                0
            },
            paths_to_plugins: if plugin_dir_utf16.is_some() {
                plugin_dir_ptrs.as_ptr()
            } else {
                std::ptr::null()
            },
            num_paths_to_plugins: u32::from(plugin_dir_utf16.is_some()),
            path_to_logs_and_data: std::ptr::null(),
            allocate_callback: std::ptr::null(),
            release_callback: std::ptr::null(),
            log_message_callback: sl_log_callback as *const c_void,
            flags: (1 << 0) | (1 << 7), // eDisableCLStateTracking | eUseFrameBasedResourceTagging
            features_to_load: features.as_ptr(),
            num_features_to_load: features.len() as u32,
            application_id: 231_313_132,
            engine: 0,
            engine_version: c"0.1.0".as_ptr(),
            project_id: std::ptr::null(),
            render_api: 2, // Vulkan
        };
        let init_res = unsafe { (binding.sl_init)((&raw const prefs).cast(), SL_SDK_VERSION) };
        if init_res != SlResult::Ok {
            log::warn!("Streamline slInit failed: {init_res:?}");
            return None;
        }

        if let Some(set_vk) = binding.sl_set_vulkan_info {
            let vk_info = SlVulkanInfo {
                base: BaseStructure::new(VULKAN_INFO_GUID, 3),
                device: context.device.handle(),
                instance: context.instance.handle(),
                physical_device: context.physical_device,
                compute_queue_index: 0,
                compute_queue_family: context.graphics_queue_family,
                graphics_queue_index: 0,
                graphics_queue_family: context.graphics_queue_family,
                optical_flow_queue_index: 0,
                optical_flow_queue_family: context.graphics_queue_family,
                use_native_optical_flow_mode: false,
                compute_queue_create_flags: 0,
                graphics_queue_create_flags: 0,
                optical_flow_queue_create_flags: 0,
            };
            let vk_res = unsafe { set_vk(&raw const vk_info) };
            if vk_res != SlResult::Ok {
                log::warn!("Streamline slSetVulkanInfo failed: {vk_res:?}");
                unsafe { (binding.sl_shutdown)() };
                return None;
            }
        }

        let adapter = SlAdapterInfo {
            base: BaseStructure::new(ADAPTER_INFO_GUID, 1),
            device_luid: std::ptr::null_mut(),
            device_luid_size_in_bytes: 0,
            vk_physical_device: ash::vk::Handle::as_raw(context.physical_device) as *mut c_void,
        };
        let supported = |feat: Feature| -> bool {
            let res =
                unsafe { (binding.sl_is_feature_supported)(feat, (&raw const adapter).cast()) };
            if res != SlResult::Ok {
                log::info!("Streamline feature {feat} not supported: {res:?}");
            }
            res == SlResult::Ok
        };
        let caps = StreamlineCaps {
            is_nvidia: true,
            interposer_loaded: true,
            dlss_sr_supported: supported(FEATURE_DLSS),
            dlss_rr_supported: supported(FEATURE_DLSS_RR),
            dlss_fg_supported: supported(FEATURE_DLSS_G),
        };
        log::info!(
            "Streamline initialized (sr={}, rr={}, fg={})",
            caps.dlss_sr_supported,
            caps.dlss_rr_supported,
            caps.dlss_fg_supported
        );

        let load_fn = |feat: Feature, name: &CStr| -> Option<*mut c_void> {
            let mut ptr: *mut c_void = std::ptr::null_mut();
            let res =
                unsafe { (binding.sl_get_feature_function)(feat, name.as_ptr(), &raw mut ptr) };
            if res == SlResult::Ok && !ptr.is_null() {
                Some(ptr)
            } else {
                None
            }
        };
        let dlss_set_options = load_fn(FEATURE_DLSS, c"slDLSSSetOptions")
            .map(|p| unsafe { std::mem::transmute::<*mut c_void, PFunSlDlssSetOptions>(p) });
        let dlssd_set_options = load_fn(FEATURE_DLSS_RR, c"slDLSSDSetOptions")
            .map(|p| unsafe { std::mem::transmute::<*mut c_void, PFunSlDlssdSetOptions>(p) });
        let dlssg_set_options = load_fn(FEATURE_DLSS_G, c"slDLSSGSetOptions")
            .map(|p| unsafe { std::mem::transmute::<*mut c_void, PFunSlDlssgSetOptions>(p) });

        log::info!(
            "Streamline initialized: DLSS-SR={}, DLSS-RR={}, DLFG={}",
            caps.dlss_sr_supported,
            caps.dlss_rr_supported,
            caps.dlss_fg_supported
        );

        Some(Self {
            binding,
            caps,
            dlss_set_options,
            dlssd_set_options,
            dlssg_set_options,
            viewport: SlViewportHandle::new(0),
        })
    }

    #[must_use]
    pub const fn caps(&self) -> StreamlineCaps {
        self.caps
    }

    /// Push per-frame camera matrices and jitter constants to Streamline.
    pub fn set_frame_constants(&self, frame_index: u32, constants: &SlConstants) -> SlResult {
        let Some(set_consts) = self.binding.sl_set_constants else {
            return SlResult::ErrorNotInitialized;
        };
        let mut token: *mut c_void = std::ptr::null_mut();
        let tok_res = unsafe {
            (self.binding.sl_get_new_frame_token)(&raw mut token, &raw const frame_index)
        };
        if tok_res != SlResult::Ok || token.is_null() {
            return tok_res;
        }
        unsafe { set_consts(&raw const *constants, token, &raw const self.viewport) }
    }

    /// Tag a Vulkan image resource for the current frame (`DLSS-SR`, `DLSS-RR`, `DLSS-G`).
    pub fn tag_resource(
        &self,
        frame_index: u32,
        tags: &[SlResourceTag],
        cmd: vk::CommandBuffer,
    ) -> SlResult {
        let mut token: *mut c_void = std::ptr::null_mut();
        let tok_res = unsafe {
            (self.binding.sl_get_new_frame_token)(&raw mut token, &raw const frame_index)
        };
        if tok_res != SlResult::Ok || token.is_null() {
            return tok_res;
        }
        unsafe {
            (self.binding.sl_set_tag_for_frame)(
                token,
                (&raw const self.viewport).cast(),
                tags.as_ptr().cast(),
                tags.len() as u32,
                vk::Handle::as_raw(cmd) as *mut c_void,
            )
        }
    }

    /// Configure and evaluate DLSS Super Resolution on `cmd`.
    pub fn evaluate_dlss_sr(
        &self,
        cmd: vk::CommandBuffer,
        frame_index: u32,
        options: &DlssOptions,
    ) -> SlResult {
        let Some(set_opts) = self.dlss_set_options else {
            return SlResult::ErrorFeatureNotSupported;
        };
        let res = unsafe { set_opts(&raw const self.viewport, &raw const *options) };
        if res != SlResult::Ok {
            return res;
        }
        let mut token: *mut c_void = std::ptr::null_mut();
        let tok_res = unsafe {
            (self.binding.sl_get_new_frame_token)(&raw mut token, &raw const frame_index)
        };
        if tok_res != SlResult::Ok || token.is_null() {
            return tok_res;
        }
        let inputs = [&raw const self.viewport.base];
        unsafe {
            (self.binding.sl_evaluate_feature)(
                FEATURE_DLSS,
                token,
                inputs.as_ptr(),
                inputs.len() as u32,
                vk::Handle::as_raw(cmd) as *mut c_void,
            )
        }
    }

    /// Configure and evaluate DLSS Ray Reconstruction on `cmd`.
    pub fn evaluate_dlss_rr(
        &self,
        cmd: vk::CommandBuffer,
        frame_index: u32,
        options: &DlssdOptions,
    ) -> SlResult {
        let Some(set_opts) = self.dlssd_set_options else {
            return SlResult::ErrorFeatureNotSupported;
        };
        let res = unsafe { set_opts(&raw const self.viewport, &raw const *options) };
        if res != SlResult::Ok {
            return res;
        }
        let mut token: *mut c_void = std::ptr::null_mut();
        let tok_res = unsafe {
            (self.binding.sl_get_new_frame_token)(&raw mut token, &raw const frame_index)
        };
        if tok_res != SlResult::Ok || token.is_null() {
            return tok_res;
        }
        let inputs = [&raw const self.viewport.base];
        unsafe {
            (self.binding.sl_evaluate_feature)(
                FEATURE_DLSS_RR,
                token,
                inputs.as_ptr(),
                inputs.len() as u32,
                vk::Handle::as_raw(cmd) as *mut c_void,
            )
        }
    }

    /// Configure DLSS Frame Generation (DLFG / `FEATURE_DLSS_G`).
    pub fn configure_dlfg(&self, options: &DlssgOptions) -> SlResult {
        let Some(set_opts) = self.dlssg_set_options else {
            return SlResult::ErrorFeatureNotSupported;
        };
        unsafe { set_opts(&raw const self.viewport, &raw const *options) }
    }
}

impl Drop for StreamlineRuntime {
    fn drop(&mut self) {
        unsafe {
            (self.binding.sl_shutdown)();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_version_matches_2_14_16_magic() {
        assert_eq!(SL_SDK_VERSION, 0x0002_000E_0010_FEDC);
    }

    #[test]
    fn base_structure_and_options_headers_initialize() {
        let sr = DlssOptions::default();
        assert_eq!(sr.base.struct_type, DLSS_OPTIONS_GUID);
        assert_eq!(sr.base.struct_version, 3);

        let rr = DlssdOptions::default();
        assert_eq!(rr.base.struct_type, DLSSD_OPTIONS_GUID);
        assert_eq!(rr.base.struct_version, 3);
    }

    #[test]
    fn missing_interposer_returns_none_without_panicking() {
        assert!(StreamlineBinding::try_load_from("nonexistent_sl_interposer.so").is_none());
    }

    #[test]
    fn probe_distinguishes_nvidia_vendor_id() {
        let props = vk::PhysicalDeviceProperties {
            vendor_id: NVIDIA_VENDOR_ID,
            ..Default::default()
        };
        assert!(StreamlineCaps::probe(&props).is_nvidia);

        let non_nvidia = vk::PhysicalDeviceProperties {
            vendor_id: 0x5143,
            ..Default::default()
        };
        assert!(!StreamlineCaps::probe(&non_nvidia).is_nvidia);
    }
}
