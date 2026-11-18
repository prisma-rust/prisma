//! NVIDIA Streamline 2.x dynamic loader and ABI contract (DLSS-SR, DLSS-RR, DLSS-G).
//!
//! Mirrors the official Streamline 2.14.16 C++ headers (`sl.h`, `sl_core_api.h`,
//! `sl_core_types.h`, `sl_dlss.h`, `sl_dlss_d.h`, `sl_version.h`) and loads
//! `sl.interposer` dynamically at runtime so hosts without the SDK degrade
//! gracefully without link-time dependencies.

use std::ffi::{c_char, c_void};

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

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlResult {
    Ok = 0,
    ErrorIO = 1,
    ErrorDriverOutOfDate = 2,
    ErrorOsOutOfDate = 3,
    ErrorOsDisabledHws = 4,
    ErrorDeviceNotCreated = 5,
    ErrorNoSupportedAdapterFound = 6,
    ErrorAdapterNotSupported = 7,
    ErrorNoPlugins = 8,
    ErrorVulkanApi = 9,
    ErrorNotInitialized = 21,
    ErrorFeatureMissing = 29,
    ErrorFeatureNotSupported = 30,
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
    pub sl_init: PFunSlInit,
    pub sl_shutdown: PFunSlShutdown,
    pub sl_is_feature_supported: PFunSlIsFeatureSupported,
    pub sl_get_new_frame_token: PFunSlGetNewFrameToken,
    pub sl_set_tag_for_frame: PFunSlSetTagForFrame,
    pub sl_evaluate_feature: PFunSlEvaluateFeature,
    pub sl_get_feature_function: PFunSlGetFeatureFunction,
}

// SAFETY: Function pointers inside `StreamlineBinding` point to immutable code pages.
unsafe impl Send for StreamlineBinding {}
// SAFETY: Function pointers inside `StreamlineBinding` point to immutable code pages.
unsafe impl Sync for StreamlineBinding {}

impl StreamlineBinding {
    /// Attempt to load `sl.interposer` from standard library search paths.
    /// Returns `None` when the Streamline SDK runtime is absent.
    pub fn try_load() -> Option<Self> {
        let candidates = ["sl.interposer.dll", "libsl.interposer.so"];
        for candidate in candidates {
            if let Some(binding) = Self::try_load_from(candidate) {
                return Some(binding);
            }
        }
        None
    }

    pub fn try_load_from(path: &str) -> Option<Self> {
        let library = SharedLib::open(path)?;
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
            Some(Self {
                _library: library,
                sl_init,
                sl_shutdown,
                sl_is_feature_supported,
                sl_get_new_frame_token,
                sl_set_tag_for_frame,
                sl_evaluate_feature,
                sl_get_feature_function,
            })
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
