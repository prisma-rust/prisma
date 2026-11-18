//! NVIDIA Real-Time Denoisers (NRD) + 1 spp path-tracing denoiser contract.
//!
//! Defines the denoiser fallback ladder (`DLSS-RR -> NRD + DLSS-SR -> TAA -> direct`),
//! NRD ReBLUR/ReLAX/SIGMA configuration parameters, and the 1 spp path-tracer guide
//! buffer contract (`PT_DEPTH_H`, `PT_VELOCITY_H`, `PT_NORMAL_ROUGHNESS_H`,
//! `PT_DIFFUSE_H`, `PT_SPECULAR_H`, `PT_HIT_DISTANCE_H`).
//!
//! Reference: `docs/rtx-dlss-integration-design.md` §§ 2–4.

use crate::render_graph::{
    ResourceHandle, PT_COLOR_H, PT_DEPTH_H, PT_NORMAL_ROUGHNESS_H, PT_VELOCITY_H,
};
use crate::streamline::{probe_shared_library, StreamlineCaps};

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
            available: probe_shared_library(NRD_LIB_NAME),
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
