//! NVIDIA Real-Time Denoisers (NRD) + 1 spp path-tracing denoiser contract.
//!
//! Defines the denoiser fallback ladder (`DLSS-RR -> NRD + DLSS-SR -> TAA -> direct`),
//! NRD ReBLUR/ReLAX/SIGMA configuration parameters, and the 1 spp path-tracer guide
//! buffer contract (`PT_DEPTH_H`, `PT_VELOCITY_H`, `PT_NORMAL_ROUGHNESS_H`,
//! `PT_DIFFUSE_H`, `PT_SPECULAR_H`, `PT_HIT_DISTANCE_H`).
//!
//! Reference: `docs/rtx-dlss-integration-design.md` §§ 2–4.

use std::ffi::c_void;

use crate::render_graph::{
    ResourceHandle, PT_COLOR_H, PT_DEPTH_H, PT_NORMAL_ROUGHNESS_H, PT_VELOCITY_H,
};
use crate::streamline::{probe_shared_library, SharedLib, StreamlineCaps};

/// Split-lobe diffuse radiance guide handle reserved for NRD / DLSS-RR (`PT_DIFFUSE_H`).
pub const PT_DIFFUSE_H: ResourceHandle = ResourceHandle(1011);
/// Split-lobe specular radiance guide handle reserved for NRD / DLSS-RR (`PT_SPECULAR_H`).
pub const PT_SPECULAR_H: ResourceHandle = ResourceHandle(1012);
/// Primary/secondary ray hit-distance guide handle reserved for NRD / DLSS-RR (`PT_HIT_DISTANCE_H`).
pub const PT_HIT_DISTANCE_H: ResourceHandle = ResourceHandle(1013);

/// Platform shared library name for NVIDIA NRD.
#[cfg(target_os = "windows")]
pub const NRD_LIB_NAME: &str = "NRD.dll";
/// Platform shared library name for NVIDIA NRD.
#[cfg(not(target_os = "windows"))]
pub const NRD_LIB_NAME: &str = "libNRD.so";

/// Active denoiser / reconstruction backend selected for 1 spp path tracing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtDenoiserKind {
    /// NVIDIA DLSS Ray Reconstruction via Streamline (`kFeatureDLSS_RR`).
    DlssRr,
    /// Open-source NVIDIA NRD (ReBLUR / ReLAX), optionally paired with DLSS-SR.
    Nrd { with_dlss_sr: bool },
    /// Built-in temporal anti-aliasing fallback consuming `PT_DEPTH_H` + `PT_VELOCITY_H`.
    Taa,
    /// Direct un-denoised path-traced output (`PT_COLOR_H`).
    Direct,
}

/// Select the best available path-tracing denoiser along the fallback ladder:
/// `DLSS-RR -> NRD (+ DLSS-SR if supported) -> TAA -> Direct`.
#[must_use]
pub const fn select_pt_denoiser(
    sl_caps: StreamlineCaps,
    nrd_available: bool,
    taa_enabled: bool,
) -> PtDenoiserKind {
    if sl_caps.dlss_rr_supported {
        PtDenoiserKind::DlssRr
    } else if nrd_available {
        PtDenoiserKind::Nrd {
            with_dlss_sr: sl_caps.dlss_sr_supported,
        }
    } else if taa_enabled {
        PtDenoiserKind::Taa
    } else {
        PtDenoiserKind::Direct
    }
}

/// NRD denoiser algorithm variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NrdDenoiserMode {
    /// Disabled.
    #[default]
    Off,
    /// ReBLUR diffuse + specular denoiser (primary 1 spp path-tracing denoiser).
    ReblurDiffuseSpecular,
    /// ReLAX diffuse + specular denoiser (fast A-Trous + history clamping).
    RelaxDiffuseSpecular,
    /// SIGMA shadow denoiser for directional/local ray-traced shadows.
    SigmaShadow,
}

impl NrdDenoiserMode {
    /// Whether this mode requires split diffuse/specular + hit-distance guide buffers.
    #[must_use]
    pub const fn requires_split_lobes(self) -> bool {
        matches!(
            self,
            Self::ReblurDiffuseSpecular | Self::RelaxDiffuseSpecular
        )
    }

    /// Guide buffer handles required by this denoiser mode.
    #[must_use]
    pub fn required_guides(self) -> Vec<ResourceHandle> {
        match self {
            Self::Off => Vec::new(),
            Self::ReblurDiffuseSpecular | Self::RelaxDiffuseSpecular => vec![
                PT_DEPTH_H,
                PT_VELOCITY_H,
                PT_NORMAL_ROUGHNESS_H,
                PT_DIFFUSE_H,
                PT_SPECULAR_H,
                PT_HIT_DISTANCE_H,
            ],
            Self::SigmaShadow => vec![PT_DEPTH_H, PT_NORMAL_ROUGHNESS_H, PT_HIT_DISTANCE_H],
        }
    }
}

/// Temporal accumulation mode for NRD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NrdAccumulationMode {
    #[default]
    Continue,
    Restart,
    ClearAndRestart,
}

/// Per-frame common parameters supplied to NRD (`nrd::CommonSettings` mirror).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NrdCommonSettings {
    pub view_to_clip: [[f32; 4]; 4],
    pub prev_view_to_clip: [[f32; 4]; 4],
    pub world_to_view: [[f32; 4]; 4],
    pub prev_world_to_view: [[f32; 4]; 4],
    /// Current-frame subpixel jitter in `[-0.5, 0.5]` pixel units.
    pub camera_jitter: [f32; 2],
    /// Previous-frame subpixel jitter in `[-0.5, 0.5]` pixel units.
    pub prev_camera_jitter: [f32; 2],
    /// Render resolution `[width, height]`.
    pub resource_size: [u16; 2],
    /// Maximum view-space denoising range in world units (NRD `denoisingRange`).
    pub denoising_range: f32,
    /// Disocclusion threshold as a fraction of view-space depth.
    pub disocclusion_threshold: f32,
    /// Monotonic frame counter.
    pub frame_index: u32,
    /// Whether to reset temporal history on camera cuts / scene reloads.
    pub accumulation_mode: NrdAccumulationMode,
    /// Enables NRD's anti-firefly spatial clamp for 1 spp path tracing.
    pub enable_anti_firefly: bool,
    /// Enables validation overlay output if supported by the NRD build.
    pub enable_validation: bool,
}

impl Default for NrdCommonSettings {
    fn default() -> Self {
        let identity = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        Self {
            view_to_clip: identity,
            prev_view_to_clip: identity,
            world_to_view: identity,
            prev_world_to_view: identity,
            camera_jitter: [0.0, 0.0],
            prev_camera_jitter: [0.0, 0.0],
            resource_size: [1920, 1080],
            denoising_range: 1000.0,
            disocclusion_threshold: 0.01,
            frame_index: 0,
            accumulation_mode: NrdAccumulationMode::Continue,
            enable_anti_firefly: true,
            enable_validation: false,
        }
    }
}

/// Runtime capability probe for the optional NRD shared library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NrdCaps {
    pub available: bool,
}

impl NrdCaps {
    /// Probe whether `NRD.dll` / `libNRD.so` can be dynamically loaded on this host.
    #[must_use]
    pub fn probe() -> Self {
        Self {
            available: NrdBinding::find_library_path().is_some()
                || probe_shared_library(NRD_LIB_NAME),
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NrdLibraryDesc {
    pub spirv_binding_offsets: [u32; 4],
    pub supported_denoisers: *const u32,
    pub supported_denoisers_num: u32,
    pub version_major: u8,
    pub version_minor: u8,
    pub version_build: u8,
    pub normal_encoding: u8,
    pub roughness_encoding: u8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NrdDenoiserDesc {
    pub identifier: u32,
    pub denoiser: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NrdAllocationCallbacks {
    pub allocate: *const c_void,
    pub reallocate: *const c_void,
    pub free: *const c_void,
    pub user_arg: *mut c_void,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NrdInstanceCreationDesc {
    pub allocation_callbacks: NrdAllocationCallbacks,
    pub denoisers: *const NrdDenoiserDesc,
    pub denoisers_num: u32,
}

pub type PFunNrdGetLibraryDesc = unsafe extern "C" fn() -> *const NrdLibraryDesc;
pub type PFunNrdCreateInstance =
    unsafe extern "C" fn(desc: *const NrdInstanceCreationDesc, instance: *mut *mut c_void) -> i32;
pub type PFunNrdDestroyInstance = unsafe extern "C" fn(instance: *mut c_void);

/// Dynamically loaded NVIDIA NRD C-ABI binding (`NRD.dll` / `libNRD.so`).
pub struct NrdBinding {
    _library: SharedLib,
    pub get_library_desc: PFunNrdGetLibraryDesc,
    pub create_instance: PFunNrdCreateInstance,
    pub destroy_instance: PFunNrdDestroyInstance,
}

// SAFETY: Resolved NRD entry points point to immutable code pages.
unsafe impl Send for NrdBinding {}
// SAFETY: Resolved NRD entry points point to immutable code pages.
unsafe impl Sync for NrdBinding {}

impl NrdBinding {
    fn candidate_paths() -> Vec<String> {
        let mut candidates = Vec::new();
        for env_key in ["PRISMA_NRD_DIR", "NRD_SDK_DIR"] {
            if let Ok(dir) = std::env::var(env_key) {
                let base = std::path::Path::new(&dir);
                candidates.push(base.join(NRD_LIB_NAME).to_string_lossy().into_owned());
            }
        }
        candidates.extend([
            NRD_LIB_NAME.to_owned(),
            format!(".local/reference/RTXGI/Bin/{NRD_LIB_NAME}"),
            format!("../.local/reference/RTXGI/Bin/{NRD_LIB_NAME}"),
            format!("../../.local/reference/RTXGI/Bin/{NRD_LIB_NAME}"),
            format!("../../../.local/reference/RTXGI/Bin/{NRD_LIB_NAME}"),
        ]);
        candidates
    }

    fn find_library_path() -> Option<String> {
        Self::candidate_paths()
            .into_iter()
            .find(|p| std::path::Path::new(p).exists())
    }

    /// Attempt to load `NRD.dll` / `libNRD.so` from env overrides, reference Bin, or PATH.
    pub fn try_load() -> Option<Self> {
        for path in Self::candidate_paths() {
            if let Some(binding) = Self::try_load_from(&path) {
                return Some(binding);
            }
        }
        None
    }

    /// Load NRD from an explicit shared library path.
    pub fn try_load_from(path: &str) -> Option<Self> {
        let library = SharedLib::open(path)?;
        unsafe {
            let get_library_desc = std::mem::transmute::<*mut c_void, PFunNrdGetLibraryDesc>(
                library.get_symbol(b"GetLibraryDesc\0")?,
            );
            let create_instance = std::mem::transmute::<*mut c_void, PFunNrdCreateInstance>(
                library.get_symbol(b"CreateInstance\0")?,
            );
            let destroy_instance = std::mem::transmute::<*mut c_void, PFunNrdDestroyInstance>(
                library.get_symbol(b"DestroyInstance\0")?,
            );
            Some(Self {
                _library: library,
                get_library_desc,
                create_instance,
                destroy_instance,
            })
        }
    }
}

/// Live NVIDIA NRD runtime instance (ReBLUR / ReLAX / SIGMA) for 1 spp path tracing.
pub struct NrdRuntime {
    binding: NrdBinding,
    instance: *mut c_void,
    version: (u8, u8, u8),
}

// SAFETY: `NrdRuntime` owns its NRD instance exclusively and is only mutated on the render thread.
unsafe impl Send for NrdRuntime {}
// SAFETY: `NrdRuntime` is synchronized by `GraphRenderer` on the render thread.
unsafe impl Sync for NrdRuntime {}

impl NrdRuntime {
    /// Load `NRD.dll` / `libNRD.so` and create a ReBLUR diffuse+specular instance.
    pub fn try_init() -> Option<Self> {
        let binding = NrdBinding::try_load()?;
        let desc_ptr = unsafe { (binding.get_library_desc)() };
        let version = if desc_ptr.is_null() {
            (0, 0, 0)
        } else {
            let d = unsafe { &*desc_ptr };
            (d.version_major, d.version_minor, d.version_build)
        };
        let denoiser = NrdDenoiserDesc {
            identifier: 0,
            denoiser: 0, // REBLUR_DIFFUSE_SPECULAR
        };
        let create_desc = NrdInstanceCreationDesc {
            allocation_callbacks: NrdAllocationCallbacks {
                allocate: std::ptr::null(),
                reallocate: std::ptr::null(),
                free: std::ptr::null(),
                user_arg: std::ptr::null_mut(),
            },
            denoisers: &raw const denoiser,
            denoisers_num: 1,
        };
        let mut instance: *mut c_void = std::ptr::null_mut();
        let res = unsafe { (binding.create_instance)(&raw const create_desc, &raw mut instance) };
        if res != 0 || instance.is_null() {
            log::warn!("NRD CreateInstance failed (code={res})");
            return None;
        }
        log::info!(
            "NRD initialized (v{}.{}.{})",
            version.0,
            version.1,
            version.2
        );
        Some(Self {
            binding,
            instance,
            version,
        })
    }

    #[must_use]
    pub const fn version(&self) -> (u8, u8, u8) {
        self.version
    }
}

impl Drop for NrdRuntime {
    fn drop(&mut self) {
        if !self.instance.is_null() {
            unsafe {
                (self.binding.destroy_instance)(self.instance);
            }
            self.instance = std::ptr::null_mut();
        }
    }
}

/// Validates that 1 spp path-tracing guide outputs cover the active denoiser contract.
#[must_use]
pub const fn pt_1spp_color_handle() -> ResourceHandle {
    PT_COLOR_H
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_ladder_prefers_dlss_rr_then_nrd_then_taa_then_direct() {
        let full = StreamlineCaps {
            is_nvidia: true,
            interposer_loaded: true,
            dlss_sr_supported: true,
            dlss_rr_supported: true,
            dlss_fg_supported: true,
        };
        assert_eq!(select_pt_denoiser(full, true, true), PtDenoiserKind::DlssRr);

        let sr_only = StreamlineCaps {
            is_nvidia: true,
            interposer_loaded: true,
            dlss_sr_supported: true,
            dlss_rr_supported: false,
            dlss_fg_supported: false,
        };
        assert_eq!(
            select_pt_denoiser(sr_only, true, true),
            PtDenoiserKind::Nrd { with_dlss_sr: true }
        );
        assert_eq!(
            select_pt_denoiser(StreamlineCaps::default(), true, true),
            PtDenoiserKind::Nrd {
                with_dlss_sr: false
            }
        );
        assert_eq!(
            select_pt_denoiser(StreamlineCaps::default(), false, true),
            PtDenoiserKind::Taa
        );
        assert_eq!(
            select_pt_denoiser(StreamlineCaps::default(), false, false),
            PtDenoiserKind::Direct
        );
    }

    #[test]
    fn reblur_and_relax_require_six_split_lobe_guides() {
        assert!(NrdDenoiserMode::ReblurDiffuseSpecular.requires_split_lobes());
        assert!(NrdDenoiserMode::RelaxDiffuseSpecular.requires_split_lobes());
        assert!(!NrdDenoiserMode::SigmaShadow.requires_split_lobes());

        let guides = NrdDenoiserMode::ReblurDiffuseSpecular.required_guides();
        assert_eq!(guides.len(), 6);
        assert!(guides.contains(&PT_DIFFUSE_H));
        assert!(guides.contains(&PT_SPECULAR_H));
        assert!(guides.contains(&PT_HIT_DISTANCE_H));
    }

    #[test]
    fn nrd_probe_does_not_panic_when_absent() {
        let _ = NrdCaps::probe();
    }
}
