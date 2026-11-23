//! Render quality, mode, internal resolution, and post-processing settings.

use ash::vk;

use crate::capabilities::RayTracingCaps;

/// Render mode selector — full rasterized PBR vs real-time path tracing vs pure 2D sprites.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RenderMode {
    /// Standard rasterized PBR pipeline (ForwardPass + ShadowMap + GTAO + Post).
    #[default]
    Raster,
    /// Real-time path tracing (PathTracePass compute + Post).
    PathTrace,
    /// Pure 2D sprite pipeline (ForwardPass / SpritePass + Post, clips 3D ShadowMap / GTAO etc).
    Sprites2D,
}

/// Shadow rendering strategy.
///
/// Selected per-frame by [`RenderSettings::resolve_shadow`] using probed
/// ray-tracing capabilities, so the running path adapts to the GPU. Mirrors
/// `docs/DESIGN.md` §2.3: `VK_KHR_ray_query` present → RayQuery soft shadow;
/// otherwise fall back to a rasterized depth-only shadow map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ShadowMode {
    /// No shadows.
    None,
    /// Rasterized depth-only shadow map (always available; the fallback path).
    Raster,
    /// RayQuery inline soft shadow (requires `VK_KHR_ray_query` + a built TLAS).
    RayQuery,
    /// Automatic: RayQuery when available and RT is enabled, else Raster.
    #[default]
    Auto,
}

/// Global Illumination technique for the rasterized PBR pipeline (`RenderMode::Raster`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RasterGiMode {
    /// Static baked order-2 SH probe volume (or IBL cubemap fallback).
    #[default]
    BakedProbe,
    /// Screen-space ReSTIR GI on the raster path (`RestirGiPass`, requires `VK_KHR_ray_query`).
    Restir,
    /// Order-7 PRT64 + 64-way spatial tree (`PrtPass`).
    Prt,
    /// Real-time RTXGI v1.3 DDGI probe volume (`DdgiPass`, requires `VK_KHR_ray_query`).
    Ddgi,
}

/// Radiance cache / GI technique for the path-tracing pipeline (`RenderMode::PathTrace`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PtGiMode {
    /// Pure path-traced reference (no radiance cache).
    #[default]
    Off,
    /// NVIDIA RTXGI SHARC (Spatially Hashed Radiance Cache).
    Sharc,
    /// NVIDIA Neural Radiance Cache.
    Nrc,
}

/// DLSS Frame Generation mode (DLFG / `kFeatureDLSS_G`).
///
/// Lives here rather than in `crate::streamline` so render settings stay
/// constructible when the `dlss` feature is off: the enum is pure data, the
/// Streamline FFI around it is not.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DlssgMode {
    #[default]
    Off = 0,
    On = 1,
    Auto = 2,
}

/// DLSS upscaling mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DlssMode {
    #[default]
    Off,
    Quality,
    Balanced,
    Performance,
    UltraPerformance,
    Dlaa,
}

/// Pre-upscale filter applied before DLSS when `render_extent` is below DLSS's minimum input size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DlssPreUpscaleMode {
    #[default]
    Linear,
    Nearest,
    Off,
}

impl DlssPreUpscaleMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "linear" | "bilinear" | "1" => Some(Self::Linear),
            "nearest" | "point" | "2" => Some(Self::Nearest),
            "off" | "none" | "0" => Some(Self::Off),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Linear => "Linear",
            Self::Nearest => "Nearest",
            Self::Off => "Off",
        }
    }
}

/// Mobile and desktop upscaler mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MobileUpscaler {
    #[default]
    Off,
    /// Real nearest-neighbor spatial upscaling (`vk::Filter::NEAREST`).
    Nearest,
    /// Real bilinear spatial upscaling (`vk::Filter::LINEAR`).
    Linear,
    /// Qualcomm Snapdragon Game Super Resolution 2 (TAAU + Lanczos).
    Sgsr2,
    /// Arm Accuracy Super Resolution (FSR 2 mobile derivative).
    ArmAsr,
    /// PC-only placeholder for NVIDIA DLSS (equivalent to `Off`).
    Dlss,
    /// PC-only placeholder for AMD FSR (equivalent to `Off`).
    Fsr,
}

impl MobileUpscaler {
    /// Canonical config name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Nearest => "nearest",
            Self::Linear => "linear",
            Self::Sgsr2 => "sgsr2",
            Self::ArmAsr => "asr",
            Self::Dlss => "dlss",
            Self::Fsr => "fsr",
        }
    }

    /// Human-readable label for debug overlays and settings UI.
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Nearest => "Nearest",
            Self::Linear => "Linear",
            Self::Sgsr2 => "SGSR 2",
            Self::ArmAsr => "Arm ASR",
            Self::Dlss => "DLSS (Placeholder)",
            Self::Fsr => "FSR (Placeholder)",
        }
    }

    /// Whether this mode runs the temporal compute upscaler pass (`MobileUpscalerPass`)
    /// and requires camera subpixel jitter.
    pub fn is_temporal_compute(self) -> bool {
        matches!(self, Self::Sgsr2 | Self::ArmAsr)
    }

    /// Whether this mode renders the scene at scaled `internal_resolution`
    /// and upscales to the output swapchain extent.
    ///
    /// Note: `Dlss` and `Fsr` are PC placeholders equivalent to `Off`.
    pub fn uses_internal_resolution(self) -> bool {
        matches!(
            self,
            Self::Nearest | Self::Linear | Self::Sgsr2 | Self::ArmAsr | Self::Dlss
        )
    }

    /// Parse upscaler mode name (case-insensitive).
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "0" => Some(Self::Off),
            "nearest" | "point" => Some(Self::Nearest),
            "linear" | "bilinear" => Some(Self::Linear),
            "sgsr2" | "sgsr" => Some(Self::Sgsr2),
            "asr" | "arm_asr" | "arm-asr" | "armasr" => Some(Self::ArmAsr),
            "dlss" => Some(Self::Dlss),
            "fsr" => Some(Self::Fsr),
            _ => None,
        }
    }
}

/// Quality / feature settings that passes consult at execution time.
/// These are the runtime-switchable knobs described in
/// `docs/mobile-raytracing-gi-design.md`.
#[derive(Clone, Debug)]
pub struct RenderSettings {
    /// GBuffer color format toggle.
    /// `true` = RGBA32F (quality), `false` = R10G10B10A2 (bandwidth, default).
    pub gbuffer_high_precision: bool,

    /// Ray tracing master switch.
    pub ray_tracing_enabled: bool,

    /// Ray query resolution scale: 1.0 = full res, 0.5 = half res (default).
    /// Setting to 1.0 disables half-resolution (user wants full quality).
    pub ray_query_resolution_scale: f32,

    /// GI technique for `RenderMode::Raster` (BakedProbe / ReSTIR / PRT).
    pub raster_gi: RasterGiMode,

    /// Radiance cache / GI technique for `RenderMode::PathTrace` (Off / SHARC / NRC).
    pub pt_gi: PtGiMode,

    /// PRT64 trilinear weight bias to suppress thin-wall light bleeding (default 0.05).
    pub prt_leak_bias: f32,

    /// SHARC hash-map capacity (number of voxel slots).
    /// Desktop GPUs can afford 2^23 (8M); the allocation is rebuilt, and the
    /// cache cleared, whenever this changes.
    pub sharc_capacity: u32,

    /// SHARC scene scale — controls voxel physical size.
    pub sharc_scene_scale: f32,

    /// SHARC cache clear toggle.
    pub sharc_enable_clear: bool,
    /// SHARC cache update toggle.
    pub sharc_enable_update: bool,
    /// SHARC cache resolve toggle.
    pub sharc_enable_resolve: bool,
    /// SHARC anti-firefly filter toggle.
    pub sharc_enable_anti_firefly: bool,
    /// SHARC roughness threshold below which rays keep tracing.
    pub sharc_roughness_threshold: f32,
    /// SHARC downscale factor for sparse updates.
    pub sharc_downscale_factor: u32,
    /// SHARC temporal accumulation frame window size.
    pub sharc_accumulation_frame_num: u32,
    /// SHARC stale frames before eviction.
    pub sharc_stale_frame_num: u32,

    /// ReSTIR GI internal resolution scale: 0.5 = half res (default),
    /// 1.0 = full res. The pass traces and resamples at this resolution and
    /// the forward pass bilinearly upsamples the result.
    pub restir_gi_scale: f32,
    /// Secondary rays traced per GI pixel per frame (1 = one ray, cheap;
    /// 2-4 raises quality linearly in cost).
    pub restir_gi_rays_per_pixel: u32,
    /// Spatial resampling taps per pixel (0 disables the spatial reuse).
    pub restir_gi_spatial_samples: u32,
    /// Frames a reservoir may survive before it is dropped (0 = reuse
    /// forever). Guards against stale history after disocclusion.
    pub restir_gi_max_history: u32,
    /// Merge the previous frame's reservoir into this frame's samples.
    pub restir_gi_temporal_reuse: bool,
    /// Merge neighbour reservoirs inside the current frame.
    pub restir_gi_spatial_reuse: bool,
    /// Maximum secondary ray distance in world units; longer hits fall back to
    /// the environment, which keeps the trace bounded for the cache lookup.
    pub restir_gi_max_secondary_dist: f32,

    /// DLSS upscaling mode.
    pub dlss_mode: DlssMode,
    /// Pre-upscale filter when `render_extent < dlss_min`.
    pub dlss_pre_upscale: DlssPreUpscaleMode,
    /// DLSS Frame Generation (DLFG) mode via Streamline.
    pub dlfg_mode: DlssgMode,
    /// Number of generated frames per rendered frame for DLFG (`1` = 2x, `2` = 3x, `3` = 4x).
    pub dlfg_frames: u32,
    /// Enable DLSS Ray Reconstruction (DLSS-RR) when supported.
    pub dlss_rr_enabled: bool,

    /// Shadow strategy. `Auto` (default) picks RayQuery when RT is enabled and
    /// `VK_KHR_ray_query` is supported, otherwise falls back to the rasterized
    /// shadow map. See [`ShadowMode`].
    pub shadow_mode: ShadowMode,

    /// Shadow map edge length (texels, #526).
    ///
    /// This is **render graph resource** size, active at create / rebuild shadow pass; modifying at
    /// runtime needs render graph rebuild (current `GraphRenderer` only reads at construction). Clamped by
    /// [`crate::MIN_SHADOW_MAP_SIZE`] / [`crate::MAX_SHADOW_MAP_SIZE`].
    pub shadow_map_size: u32,

    /// Directional light cascade count (#526): `1` = single square shadow map, `2` = 2×1 atlas.
    /// Like `shadow_map_size`, active when render graph is constructed.
    pub shadow_cascade_count: u32,

    /// Render mode: Raster (PBR) or PathTrace (real-time PT).
    pub render_mode: RenderMode,

    /// Maximum path depth (bounces) for path tracing.
    pub pt_max_bounces: u32,
    /// Max world-space length of PT primary + shadow rays. Smaller values cut
    /// long-range bounces (and distant shadow casters) — an artistic focus/
    /// fog control and a cost cap on huge scenes.
    pub pt_ray_max_distance: f32,
    /// Maximum iterations (samples per pixel) for path tracing.
    /// 0 = accumulate forever (default).
    pub pt_max_iterations: u32,

    /// Temporal anti-aliasing (#883). `false` keeps the pre-TAA path.
    pub taa: bool,
    /// History blend weight for TAA, clamped to `0..=0.95`; 0 = current frame.
    pub taa_strength: f32,

    /// Histogram auto-exposure metering (default off).
    ///
    /// Off keeps the pre-metering image: `AutoExposurePass` writes a zero
    /// adapted luminance, which the tonemapper reads as a no-op gain of 1.0.
    pub auto_exposure: bool,
    /// Internal render resolution `[width, height]` (#613); `[0, 0]` = disabled.
    ///
    /// After enabling, scene (mesh / 2D sprite / tile / particle / PT) renders at this size,
    /// then `PostPass` upscales to window. UI still drawn at window resolution.
    pub internal_resolution: [u32; 2],

    /// How internal image is upscaled to window (see [`InternalScaleMode`]).
    pub internal_scale_mode: InternalScaleMode,

    /// Qualcomm Adreno Neural Fusion Super Resolution (#1161).
    pub anf_sr: bool,
    /// ANF Super Resolution sharpness parameter [0.0..=1.0].
    pub anf_sharpness: f32,
    /// Qualcomm Adreno Neural Fusion Frame Generation (#1161).
    pub anf_fg: bool,

    /// Open-source mobile temporal upscaler (`Off` / `Sgsr2` / `ArmAsr`).
    pub mobile_upscaler: MobileUpscaler,
    /// Sharpening strength for `mobile_upscaler`, clamped to `0.0..=1.0`.
    pub upscaler_sharpness: f32,

    /// Enables multi-platform virtual geometry Cluster-DAG rendering when assets provide `RCLD`.
    pub virtual_geometry_enabled: bool,
    /// Screen-space geometric error budget in output pixels (lower = finer LOD cut).
    pub vg_error_threshold_px: f32,
    /// Virtual geometry 64 KiB streaming page pool budget in MiB (`0` = automatic tier default).
    pub vg_page_pool_mb: u32,

    /// When `mobile_upscaler == MobileUpscaler::Sgsr2`, run the 3-pass pipeline
    /// (`Convert -> Activate -> Upscale`, default `true`) vs 2-pass (`Convert -> Upscale`).
    pub sgsr2_three_pass: bool,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            gbuffer_high_precision: true,
            ray_tracing_enabled: false,
            ray_query_resolution_scale: 0.5,
            raster_gi: RasterGiMode::BakedProbe,
            pt_gi: PtGiMode::Off,
            prt_leak_bias: crate::prt_runtime::DEFAULT_LEAK_BIAS,
            #[cfg(feature = "sharc")]
            sharc_capacity: crate::sharc::DEFAULT_SHARC_CAPACITY,
            #[cfg(not(feature = "sharc"))]
            sharc_capacity: 0,
            sharc_scene_scale: 50.0,
            sharc_enable_clear: false,
            sharc_enable_update: true,
            sharc_enable_resolve: true,
            sharc_enable_anti_firefly: true,
            sharc_roughness_threshold: 0.4,
            sharc_downscale_factor: 5,
            sharc_accumulation_frame_num: 20,
            sharc_stale_frame_num: 60,
            restir_gi_scale: 0.5,
            restir_gi_rays_per_pixel: 1,
            restir_gi_spatial_samples: 5,
            restir_gi_max_history: 20,
            restir_gi_temporal_reuse: true,
            restir_gi_spatial_reuse: true,
            restir_gi_max_secondary_dist: 30.0,
            dlss_mode: DlssMode::Off,
            dlss_pre_upscale: DlssPreUpscaleMode::Linear,
            dlfg_mode: DlssgMode::Off,
            dlfg_frames: 1,
            dlss_rr_enabled: false,
            shadow_mode: ShadowMode::Auto,
            shadow_map_size: crate::SHADOW_MAP_SIZE,
            shadow_cascade_count: 1,
            render_mode: RenderMode::Raster,
            pt_max_bounces: 3,
            pt_ray_max_distance: 1000.0,
            pt_max_iterations: 0,
            taa: false,
            taa_strength: 0.9,
            auto_exposure: false,
            internal_resolution: [0, 0],
            internal_scale_mode: InternalScaleMode::Stretch,
            anf_sr: false,
            anf_sharpness: 0.5,
            anf_fg: false,
            mobile_upscaler: MobileUpscaler::Off,
            upscaler_sharpness: 0.5,
            virtual_geometry_enabled: false,
            vg_error_threshold_px: 1.0,
            vg_page_pool_mb: 0,
            sgsr2_three_pass: true,
        }
    }
}

impl RenderSettings {
    /// Resolve the effective shadow mode given probed capabilities.
    ///
    /// `Auto` selects RayQuery when ray tracing is enabled and
    /// `VK_KHR_ray_query` is supported, otherwise falls back to the
    /// rasterized shadow map. Explicit modes pass through unchanged.
    pub fn resolve_shadow(&self, caps: &RayTracingCaps) -> ShadowMode {
        match self.shadow_mode {
            ShadowMode::Auto => {
                if self.ray_tracing_enabled && caps.has_ray_query() {
                    ShadowMode::RayQuery
                } else {
                    ShadowMode::Raster
                }
            }
            other => other,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal render resolution (#613)
// ---------------------------------------------------------------------------

/// How internal render resolution is upscaled to window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InternalScaleMode {
    /// Fill whole window: allow non-integer multiple, use linear sampling to upscale.
    #[default]
    Stretch,
    /// Integer multiple upscale centered, black bars around; nearest-neighbor sampling, pixel blocks equal width (pixel art).
    Integer,
}

impl InternalScaleMode {
    /// Writable name (used in the engine settings file).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stretch => "stretch",
            Self::Integer => "integer",
        }
    }

    /// Parse `"stretch"` / `"integer"` (case-insensitive); unknown value returns `None`.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "stretch" | "fill" => Some(Self::Stretch),
            "integer" | "pixel" | "nearest" => Some(Self::Integer),
            _ => None,
        }
    }
}

/// Window extent → internal render extent; `resolution == [0, 0]` returns `None` (disabled).
///
/// When only one dimension is 0, derive the other from window aspect, to avoid author writing half size
/// and getting a 0-pixel target.
pub fn internal_extent(swapchain: vk::Extent2D, resolution: [u32; 2]) -> Option<vk::Extent2D> {
    let (mut width, mut height) = (resolution[0], resolution[1]);
    if width == 0 && height == 0 {
        return None;
    }
    if width == 0 {
        let aspect = if swapchain.height > 0 {
            swapchain.width as f32 / swapchain.height as f32
        } else {
            1.0
        };
        width = ((height as f32 * aspect).round() as u32).max(1);
    }
    if height == 0 {
        let aspect = if swapchain.width > 0 {
            swapchain.height as f32 / swapchain.width as f32
        } else {
            1.0
        };
        height = ((width as f32 * aspect).round() as u32).max(1);
    }
    Some(vk::Extent2D {
        width: width.max(1),
        height: height.max(1),
    })
}

/// Target rectangle where `PostPass` places internal image onto window.
///
/// `Stretch` fills whole window; `Integer` takes `floor(min(scale))` (at least 1) centered,
/// remaining edges filled black by render pass clear color. When internal size larger than window, scales down.
pub fn post_destination(
    swapchain: vk::Extent2D,
    internal: vk::Extent2D,
    mode: InternalScaleMode,
) -> vk::Rect2D {
    let full = vk::Rect2D {
        offset: vk::Offset2D { x: 0, y: 0 },
        extent: swapchain,
    };
    if internal.width == 0 || internal.height == 0 {
        return full;
    }
    let scale = match mode {
        InternalScaleMode::Stretch => return full,
        InternalScaleMode::Integer => {
            let by_width = swapchain.width as f32 / internal.width as f32;
            let by_height = swapchain.height as f32 / internal.height as f32;
            let factor = by_width.min(by_height);
            if factor >= 1.0 {
                factor.floor()
            } else {
                factor
            }
        }
    };
    let width = ((internal.width as f32 * scale).round() as u32).clamp(1, swapchain.width.max(1));
    let height =
        ((internal.height as f32 * scale).round() as u32).clamp(1, swapchain.height.max(1));
    vk::Rect2D {
        offset: vk::Offset2D {
            x: ((swapchain.width as i32 - width as i32) / 2).max(0),
            y: ((swapchain.height as i32 - height as i32) / 2).max(0),
        },
        extent: vk::Extent2D { width, height },
    }
}

/// Pure data version of [`post_destination`]: `(offset, extent)`, no `ash` dependency.
///
/// Host (`prism-app`) uses it to derive "window pixels → UI layout space" mapping (#988),
/// ensuring HUD overlaid on screen fully overlaps rectangle after PostPass upscale.
pub fn post_destination_px(
    swapchain: [u32; 2],
    internal: [u32; 2],
    mode: InternalScaleMode,
) -> ([u32; 2], [u32; 2]) {
    let rect = post_destination(
        vk::Extent2D {
            width: swapchain[0],
            height: swapchain[1],
        },
        vk::Extent2D {
            width: internal[0],
            height: internal[1],
        },
        mode,
    );
    (
        [rect.offset.x.max(0) as u32, rect.offset.y.max(0) as u32],
        [rect.extent.width, rect.extent.height],
    )
}

/// Post-processing knobs consumed by `PostPass` (#502).
///
/// `contrast` / `saturation` / `vignette` are display-referred and run **after**
/// tonemapping. `bloom_intensity` / `bloom_threshold` run in HDR **before**
/// tonemapping, so bright pixels bloom without clipping the highlight first.
///
/// All knobs are no-ops at their defaults: `PostGrade::default()` produces
/// exactly the pre-#502 image.
///
/// - `contrast`: scale around mid-grey `0.5` (`1.0` = unchanged);
/// - `saturation`: Rec.709 luma interpolation (`1.0` = unchanged, `0.0` = grey);
/// - `vignette`: corner falloff strength (`0.0` = off, `1.0` = corners black);
/// - `bloom_intensity`: additive bloom strength (`0.0` = off);
/// - `bloom_threshold`: HDR luminance above which pixels start blooming;
/// - `chromatic_aberration`: radial red/blue split strength (`0.0` = off);
/// - `film_grain`: display-referred monochrome noise (`0.0` = off);
/// - `temperature`: white-balance warm/cool shift in `-1..=1` (`0.0` = neutral);
/// - `tint`: white-balance magenta/green shift in `-1..=1` (`0.0` = neutral);
/// - `dof_focus_distance`: depth-of-field focus plane in metres;
/// - `dof_aperture`: depth-of-field strength in `0..=1` (`0.0` = off);
/// - `dof_max_blur`: largest depth-of-field blur radius in pixels;
/// - `lut_strength`: blend toward the sampled 3D LUT in `0..=1` (`0.0` = off);
/// - `motion_blur`: camera motion-blur strength in `0..=1` (`0.0` = off);
/// - `motion_blur_max_pixels`: largest motion-blur trail in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PostGrade {
    pub contrast: f32,
    pub saturation: f32,
    pub vignette: f32,
    pub bloom_intensity: f32,
    pub bloom_threshold: f32,
    pub chromatic_aberration: f32,
    pub film_grain: f32,
    pub temperature: f32,
    pub tint: f32,
    pub dof_focus_distance: f32,
    pub dof_aperture: f32,
    pub dof_max_blur: f32,
    pub lut_strength: f32,
    pub motion_blur: f32,
    pub motion_blur_max_pixels: f32,
}

impl Default for PostGrade {
    fn default() -> Self {
        Self {
            contrast: 1.0,
            saturation: 1.0,
            vignette: 0.0,
            bloom_intensity: 0.0,
            bloom_threshold: 1.0,
            chromatic_aberration: 0.0,
            film_grain: 0.0,
            temperature: 0.0,
            tint: 0.0,
            dof_focus_distance: 1.0,
            dof_aperture: 0.0,
            dof_max_blur: 8.0,
            lut_strength: 0.0,
            motion_blur: 0.0,
            motion_blur_max_pixels: 16.0,
        }
    }
}

impl PostGrade {
    /// `true` when every knob is at its default (the pass can skip the math).
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }

    /// Clamp to the ranges the shader expects (`contrast`/`saturation` never
    /// negative, `vignette`/`bloom_intensity` in `0..=1`, threshold non-negative).
    pub fn sanitized(self) -> Self {
        Self {
            contrast: if self.contrast.is_finite() {
                self.contrast.max(0.0)
            } else {
                1.0
            },
            saturation: if self.saturation.is_finite() {
                self.saturation.max(0.0)
            } else {
                1.0
            },
            vignette: if self.vignette.is_finite() {
                self.vignette.clamp(0.0, 1.0)
            } else {
                0.0
            },
            bloom_intensity: if self.bloom_intensity.is_finite() {
                self.bloom_intensity.clamp(0.0, 1.0)
            } else {
                0.0
            },
            bloom_threshold: if self.bloom_threshold.is_finite() {
                self.bloom_threshold.max(0.0)
            } else {
                1.0
            },
            chromatic_aberration: if self.chromatic_aberration.is_finite() {
                self.chromatic_aberration.clamp(0.0, 1.0)
            } else {
                0.0
            },
            film_grain: if self.film_grain.is_finite() {
                self.film_grain.clamp(0.0, 1.0)
            } else {
                0.0
            },
            temperature: if self.temperature.is_finite() {
                self.temperature.clamp(-1.0, 1.0)
            } else {
                0.0
            },
            tint: if self.tint.is_finite() {
                self.tint.clamp(-1.0, 1.0)
            } else {
                0.0
            },
            dof_focus_distance: if self.dof_focus_distance.is_finite() {
                self.dof_focus_distance.max(0.001)
            } else {
                1.0
            },
            dof_aperture: if self.dof_aperture.is_finite() {
                self.dof_aperture.clamp(0.0, 1.0)
            } else {
                0.0
            },
            dof_max_blur: if self.dof_max_blur.is_finite() {
                self.dof_max_blur.clamp(0.0, 32.0)
            } else {
                8.0
            },
            lut_strength: if self.lut_strength.is_finite() {
                self.lut_strength.clamp(0.0, 1.0)
            } else {
                0.0
            },
            motion_blur: if self.motion_blur.is_finite() {
                self.motion_blur.clamp(0.0, 1.0)
            } else {
                0.0
            },
            motion_blur_max_pixels: if self.motion_blur_max_pixels.is_finite() {
                self.motion_blur_max_pixels.clamp(0.0, 64.0)
            } else {
                16.0
            },
        }
    }
}
