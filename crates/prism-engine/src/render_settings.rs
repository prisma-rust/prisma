//! Render-settings bundle passed into [`render_system`] each frame.
//!
//! Consolidates all per-frame render-state knobs (debug, tonemap, path tracing)
//! into a single struct so [`render_system`] has a clean, extensible parameter
//! instead of a growing list of scalar arguments.

pub use prism_render::{
    DlssMode, DlssPreUpscaleMode, DlssgMode, InternalScaleMode, MobileUpscaler, PtGiMode,
    RasterGiMode, RenderMode,
};

/// Debug flag names, ordered by shader bit (`PBR_FLAG_*` in
/// `scene_frag.slang`). Bit `i` corresponds to `PBR_FLAG_NAMES[i]`.
///
/// Bits 0-14 isolate a single PBR term; bit 15 is *not* a component: it turns
/// on the skin-weight heatmap (#560), which the vertex stage resolves into a
/// colour before the material is sampled.
pub const PBR_FLAG_NAMES: [&str; 16] = [
    "Direct",
    "Shadow",
    "Specular",
    "Metallic",
    "Roughness",
    "DiffuseIBL",
    "SpecularIBL",
    "MultiLight",
    "AO",
    "Emissive",
    "Transmission",
    "Translucency",
    "Anisotropy",
    "ClearCoat",
    "GI",
    "SkinWeights",
];

/// Human-readable label for a PBR debug-flag mask (`0` = normal render).
pub fn pbr_flag_label(flags: u32) -> &'static str {
    if flags == 0 {
        return "(normal render)";
    }
    PBR_FLAG_NAMES
        .iter()
        .enumerate()
        .find(|(bit, _)| flags == (1u32 << bit))
        .map(|(_, name)| *name)
        .unwrap_or("(custom flags)")
}

/// Default shadow coverage distance (meters): 2048² shadow map per texel ≈ 2.9 cm (#526).
pub const DEFAULT_SHADOW_DISTANCE: f32 = 60.0;

/// Aggregate of all per-frame render knobs.
///
/// `App` owns one instance, mutates it in response to keyboard/UI input, and
/// passes `&self` to [`crate::render_system::render_system`] each frame.
///
/// Extend this struct when adding new renderer knobs — do **not** add more
/// scalar parameters to `render_system`.
#[derive(Clone, Debug)]
pub struct RenderSettings {
    /// Debug flag mask (16 bits, see `scene_frag.slang` `PBR_FLAG_*`).
    /// `0` = normal full-PBR render (all components on); `1 << bit` for
    /// bits 0-14 isolates that one component as a grayscale visualization;
    /// bit 15 (`1 << 15`) = skin-weight heatmap (#560).
    pub debug_flags: u32,
    /// hue map  operator: 0 = Reinhard, 1 = ACES (Narkowicz),
    /// 2 = passthrough (2D pixel art: preserve original palette, only saturate).
    pub tonemap_mode: u32,
    /// Post-processing antialiasing: 0 = off, 1 = FXAA (executed after tonemap in `PostPass`).
    ///
    /// Does not depend on MSAA / resolve target, swapchain and editor offscreen viewport share same path;
    /// Debug graph (`debug_rt != 0`) automatically ignores it.
    pub aa_mode: u32,
    /// PostPass debug render-target viewer (Tab cycles). 0 = normal tonemapped
    /// HDR, 1 = linearized depth, 2 = view-space normal.
    pub debug_rt: u32,
    /// Histogram auto-exposure metering (off by default).
    ///
    /// When off `AutoExposurePass` writes zero luminance, tonemap reads 0 = gain 1.0,
    /// image fully identical to before auto-exposure; when on converges to actual screen luminance
    /// `AUTO_EXPOSURE_KEY` (0.18 mid gray).
    pub auto_exposure: bool,
    /// Occlusion culling (#1007): use previous-frame coarse depth to cull fully occluded draw items.
    ///
    /// Off by default. When on image should stay consistent (conservative test + one-frame delay), only fewer
    /// draws; shadow pass unaffected, occluded objects still cast shadows.
    pub occlusion_culling: bool,
    /// Forward pipeline draws mesh with `POLYGON_MODE_LINE` (#431). No-op when device unsupported.
    pub wireframe: bool,
    /// Normal fill render + wireframe overlay (#431). No-op when device lacks `fillModeNonSolid`;
    /// overridden by pure wireframe when `wireframe == true`.
    pub shaded_wireframe: bool,
    /// Current render mode: Raster (PBR) or PathTrace (real-time PT).
    pub render_mode: RenderMode,
    /// Maximum path depth (bounces) for path tracing.
    pub pt_max_bounces: u32,
    /// Max world-space length of PT primary + shadow rays.
    pub pt_ray_max_distance: f32,
    /// Maximum iterations (samples per pixel). 0 = accumulate forever.
    pub pt_max_iterations: u32,
    /// GI technique for `RenderMode::Raster` (BakedProbe, Restir, Prt).
    pub raster_gi: RasterGiMode,
    /// Radiance cache / GI technique for `RenderMode::PathTrace` (Off, Sharc, Nrc).
    pub pt_gi: PtGiMode,
    /// PRT64 trilinear weight bias for thin-wall anti-leakage (default 0.05).
    pub prt_leak_bias: f32,
    /// SHaRC: Hash-map capacity (number of voxel slots). Changing it
    /// rebuilds the cache buffers, which clears all cached radiance.
    pub sharc_capacity: u32,
    /// SHaRC: World-space scale of the hash grid voxels.
    pub sharc_scene_scale: f32,
    /// SHaRC: Cache clear toggle.
    pub sharc_enable_clear: bool,
    /// SHaRC: Cache update toggle.
    pub sharc_enable_update: bool,
    /// SHaRC: Cache resolve toggle.
    pub sharc_enable_resolve: bool,
    /// SHaRC: Anti-firefly filter toggle.
    pub sharc_enable_anti_firefly: bool,
    /// SHaRC: Maximum stale frames before entry eviction.
    pub sharc_stale_frame_num: u32,
    /// SHaRC: Downscale factor for the sparse path update.
    pub sharc_downscale_factor: u32,
    /// SHaRC: Temporal accumulation frame window size.
    pub sharc_accumulation_frame_num: u32,
    /// SHaRC: Roughness threshold below which mirror rays query direct radiance instead of cache.
    pub sharc_roughness_threshold: f32,
    /// ReSTIR GI: resolution scale relative to the internal render extent
    /// (0.5 = half resolution). 0 disables the pass.
    pub restir_gi_scale: f32,
    /// ReSTIR GI: secondary rays traced per pixel each frame.
    pub restir_gi_rays_per_pixel: u32,
    /// ReSTIR GI: spatial reuse taps per pixel (0 disables spatial reuse).
    pub restir_gi_spatial_samples: u32,
    /// ReSTIR GI: maximum number of frames a reservoir may be reused.
    pub restir_gi_max_history: u32,
    /// ReSTIR GI: reuse the previous frame reservoirs (temporal reuse).
    pub restir_gi_temporal_reuse: bool,
    /// ReSTIR GI: reuse neighbouring pixels reservoirs (spatial reuse).
    pub restir_gi_spatial_reuse: bool,
    /// ReSTIR GI: maximum world-space length of a secondary ray.
    pub restir_gi_max_secondary_dist: f32,
    /// DLSS Mode (Off, Quality, Balanced, Performance, UltraPerformance, Dlaa).
    pub dlss_mode: DlssMode,
    /// Pre-upscale filter applied before DLSS when `render_extent < dlss_min`.
    pub dlss_pre_upscale: DlssPreUpscaleMode,
    /// DLSS Frame Generation (DLFG) mode via Streamline.
    pub dlfg_mode: DlssgMode,
    /// Number of generated frames per rendered frame for DLFG (`1` = 2x, `2` = 3x, `3` = 4x).
    pub dlfg_frames: u32,
    /// Enable DLSS Ray Reconstruction (DLSS-RR) when supported.
    pub dlss_rr_enabled: bool,
    /// Whether to render ground reference mesh in scene view (Y = 0).
    pub show_grid: bool,
    /// Display space exposure (`1.0` = unchanged, scaled around mid gray `0.5`). Applied after tonemap.
    ///
    /// These three fields are #502 post-processing color-grading parameters: like `tonemap_mode` / `aa_mode`
    /// they take effect in `PostPass`, swapchain and editor offscreen viewport share same path.
    pub contrast: f32,
    /// Display space saturation (`1.0` = unchanged, `0.0` = grayscale). Rec.709 luma interpolation.
    pub saturation: f32,
    /// Vignette strength (`0.0` = disabled, `1.0` = corners fully black).
    pub vignette: f32,
    /// HDR bloom switch (#502). When disabled `bloom_intensity` not used in render.
    pub bloom: bool,
    /// Bloom additive strength (`0.0` = disabled, `1.0` = strongest).
    pub bloom_intensity: f32,
    /// Bloom brightness threshold; only pixels above this HDR luminance will glow.
    pub bloom_threshold: f32,
    /// Lens chromatic aberration strength (#502 Phase 2): `0.0` = disabled, `1.0` = strongest.
    ///
    /// In HDR sampling phase offset red / blue channels radially from image center by distance,
    /// green unchanged; default 0 byte-identical to old image.
    pub chromatic_aberration: f32,
    /// Film grain strength (#502 Phase 2): `0.0` = disabled, `1.0` = strongest.
    ///
    /// Adds monochrome display-space noise after tonemap, grading and vignette; default 0 byte-identical
    /// to old image.
    pub film_grain: f32,
    /// White balance color temperature (#502 Phase 1): `-1.0` = cool, `0.0` = neutral, `1.0` = warm.
    ///
    /// In grading phase shift white point via RGB gains; at `0.0` gains exactly `(1, 1, 1)`,
    /// default path byte-identical to old image.
    pub temperature: f32,
    /// White balance hue (#502 Phase 1): `-1.0` = greenish, `0.0` = neutral, `1.0` = magenta.
    pub tint: f32,
    /// Depth of field focus distance (meters, #502 Phase 2).
    ///
    /// When `dof_aperture > 0`, greater distance from this focus means more blur; default 1 meter.
    pub dof_focus_distance: f32,
    /// Depth of field strength (`0.0` = disabled, `1.0` = strongest, #502 Phase 2).
    ///
    /// When off post-processing fully skips depth sampling, default path byte-identical.
    pub dof_aperture: f32,
    /// Depth of field max blur radius (pixels, 0–32).
    pub dof_max_blur: f32,
    /// 3D color grading LUT asset relative path (`.cube` under `assets/`, #502).
    ///
    /// Empty string = no LUT. File lazily loaded by renderer when path changes; on load failure
    /// only logs warning and keeps previous LUT, will not interrupt render.
    pub lut: String,
    /// LUT blend strength (`0.0` = disabled, `1.0` = fully use LUT).
    pub lut_strength: f32,
    /// Camera motion blur strength (`0.0` = disabled, `1.0` = strongest, #502 Phase 2).
    ///
    /// When off post-processing fully skips previous-frame matrix and depth rebuild, default path byte-identical.
    pub motion_blur: f32,
    /// Motion blur max trail length (pixels, 0–64).
    pub motion_blur_max_pixels: f32,
    /// Temporal anti-aliasing (#883). Default off; when on the renderer jitters
    /// the camera and resolves against the previous frame.
    pub taa: bool,
    /// TAA history weight (`0.0..=0.95`); ignored when `taa` is off.
    pub taa_strength: f32,
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
    /// Internal render resolution `[width, height]` (#613); `[0, 0]` = disabled.
    ///
    /// When on scene rendered at this size, `PostPass` upscales to window; camera aspect also becomes
    /// `width / height`, so window size changes no longer alter framing.
    pub internal_resolution: [u32; 2],
    /// How to upscale internal image to window (stretch / integer scale + letterbox).
    pub internal_scale_mode: InternalScaleMode,
    /// Main directional light shadow coverage distance (world units, meters).
    ///
    /// Shadow orthographic box centered at **`distance * 0.5` in front of camera**, edge length `distance`;
    /// larger value covers farther, each texel coarser (2048² at 60 m ≈ 2.9 cm/texel).
    /// No cascades (see #526), so it is both "how far shadows can cast" and "quality" knob.
    pub shadow_distance: f32,
    /// Shadow map edge length (texels, #526); takes effect when renderer constructed, changing at runtime requires rebuilding render graph.
    pub shadow_map_size: u32,
    /// Directional light cascade count (#526 CSM): `1` = single shadow map (default), `2` = 2×1 atlas
    /// near / far two cascades. Takes effect when renderer constructed.
    pub shadow_cascade_count: u32,
}

/// Default PBR mode: normal full-PBR render, Reinhard tonemap, Raster.
impl RenderSettings {
    pub fn post_grade(&self) -> prism_render::PostGrade {
        prism_render::PostGrade {
            contrast: self.contrast,
            saturation: self.saturation,
            vignette: self.vignette,
            bloom_intensity: if self.bloom {
                self.bloom_intensity
            } else {
                0.0
            },
            bloom_threshold: self.bloom_threshold,
            chromatic_aberration: self.chromatic_aberration,
            film_grain: self.film_grain,
            temperature: self.temperature,
            tint: self.tint,
            dof_focus_distance: self.dof_focus_distance,
            dof_aperture: self.dof_aperture,
            dof_max_blur: self.dof_max_blur,
            lut_strength: self.lut_strength,
            motion_blur: self.motion_blur,
            motion_blur_max_pixels: self.motion_blur_max_pixels,
        }
    }

    /// Export as pure data for render graph report (`prism_render::render_graph_report`).
    ///
    /// Report DTO lives in `prism-render` (no engine dependency), mapping stays here, editor and
    /// player process share one copy, avoid writing field table twice.
    pub fn to_report_settings(&self) -> prism_render::render_graph_report::ReportSettings {
        prism_render::render_graph_report::ReportSettings::new(
            self.render_mode,
            self.debug_flags,
            self.tonemap_mode,
            self.debug_rt,
            self.pt_max_bounces,
            self.pt_ray_max_distance,
            self.pt_max_iterations,
        )
    }
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            debug_flags: 0,
            tonemap_mode: 0,
            aa_mode: 0,
            debug_rt: 0,
            auto_exposure: false,
            occlusion_culling: false,
            wireframe: false,
            shaded_wireframe: false,
            render_mode: RenderMode::Raster,
            pt_max_bounces: 3,
            pt_ray_max_distance: 1000.0,
            pt_max_iterations: 0,
            raster_gi: RasterGiMode::BakedProbe,
            pt_gi: PtGiMode::Off,
            prt_leak_bias: prism_render::prt_runtime::DEFAULT_LEAK_BIAS,
            sharc_capacity: 1 << 20,
            sharc_scene_scale: 50.0,
            sharc_enable_clear: false,
            sharc_enable_update: true,
            sharc_enable_resolve: true,
            sharc_enable_anti_firefly: true,
            sharc_stale_frame_num: 60,
            sharc_downscale_factor: 5,
            sharc_accumulation_frame_num: 20,
            sharc_roughness_threshold: 0.4,
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
            show_grid: false,
            contrast: 1.0,
            saturation: 1.0,
            vignette: 0.0,
            bloom: false,
            bloom_intensity: 0.15,
            bloom_threshold: 1.0,
            chromatic_aberration: 0.0,
            film_grain: 0.0,
            temperature: 0.0,
            tint: 0.0,
            dof_focus_distance: 1.0,
            dof_aperture: 0.0,
            dof_max_blur: 8.0,
            lut: String::new(),
            lut_strength: 0.0,
            motion_blur: 0.0,
            motion_blur_max_pixels: 16.0,
            taa: false,
            taa_strength: 0.9,
            anf_sr: false,
            anf_sharpness: 0.5,
            anf_fg: false,
            mobile_upscaler: MobileUpscaler::Off,
            upscaler_sharpness: 0.5,
            shadow_distance: DEFAULT_SHADOW_DISTANCE,
            shadow_map_size: prism_render::SHADOW_MAP_SIZE,
            shadow_cascade_count: 1,
            internal_resolution: [0, 0],
            internal_scale_mode: InternalScaleMode::Stretch,
        }
    }
}

impl RenderSettings {
    /// Camera aspect when internal resolution active; otherwise window aspect (#613).
    ///
    /// When only one dimension non-zero the other derived from window ratio, aspect same as window, return directly
    /// `window_aspect`.
    pub fn render_aspect(&self, window_aspect: f32) -> f32 {
        let [width, height] = self.internal_resolution;
        if width > 0 && height > 0 {
            width as f32 / height as f32
        } else if window_aspect.is_finite() && window_aspect > 0.0 {
            window_aspect
        } else {
            16.0 / 9.0
        }
    }

    /// Scene actual render pixel height (#978).
    ///
    /// When `internal_resolution` active it is internal height; when only one dimension given derive by window ratio
    /// (consistent with `prism_render::render_graph::internal_extent`). When neither given
    /// it is window height.
    pub fn render_height(&self, window_width: u32, window_height: u32) -> f32 {
        let window_height = window_height.max(1);
        let [width, height] = self.internal_resolution;
        if width == 0 && height == 0 {
            return window_height as f32;
        }
        if height > 0 {
            return height as f32;
        }
        let window_width = window_width.max(1);
        let derived = (width as f32 * window_height as f32 / window_width as f32).round();
        derived.max(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{pbr_flag_label, InternalScaleMode, RenderSettings, PBR_FLAG_NAMES};

    #[test]
    fn every_flag_bit_has_a_label_and_the_heatmap_bit_is_not_a_pbr_component() {
        for bit in 0..PBR_FLAG_NAMES.len() {
            let label = pbr_flag_label(1u32 << bit);
            assert_ne!(label, "(custom flags)", "bit {bit} has no name");
        }
        // Bit 16 is skinning weight stream (#560), not a PBR component; combined value still counts as custom.
        assert_eq!(pbr_flag_label(1 << 15), "SkinWeights");
        assert_eq!(pbr_flag_label((1 << 0) | (1 << 15)), "(custom flags)");
        assert_eq!(pbr_flag_label(0), "(normal render)");
        assert_eq!(pbr_flag_label(1 << 16), "(custom flags)");
    }

    #[test]
    fn render_height_follows_the_internal_resolution() {
        let mut settings = RenderSettings::default();
        assert_eq!(
            settings.render_height(1920, 1080),
            1080.0,
            "Internal resolution disabled"
        );

        settings.internal_resolution = [640, 360];
        assert_eq!(settings.render_height(1920, 1080), 360.0);

        // Only width given: derive height by window ratio (consistent with internal_extent).
        settings.internal_resolution = [640, 0];
        assert_eq!(settings.render_height(1920, 1080), 360.0);

        // Only height given: use directly.
        settings.internal_resolution = [0, 240];
        assert_eq!(settings.render_height(1920, 1080), 240.0);

        // Degenerate input does not produce 0 / NaN.
        settings.internal_resolution = [0, 0];
        assert_eq!(settings.render_height(0, 0), 1.0);
    }

    #[test]
    fn post_grade_only_passes_bloom_when_enabled() {
        let mut settings = RenderSettings {
            bloom_intensity: 0.7,
            ..RenderSettings::default()
        };
        assert_eq!(settings.post_grade().bloom_intensity, 0.0);

        settings.bloom = true;
        let grade = settings.post_grade();
        assert_eq!(grade.bloom_intensity, 0.7);
        assert_eq!(grade.bloom_threshold, settings.bloom_threshold);

        settings.chromatic_aberration = 0.35;
        assert_eq!(settings.post_grade().chromatic_aberration, 0.35);

        settings.film_grain = 0.5;
        assert_eq!(settings.post_grade().film_grain, 0.5);

        settings.temperature = -0.4;
        settings.tint = 0.25;
        settings.dof_focus_distance = 3.5;
        settings.dof_aperture = 0.6;
        settings.dof_max_blur = 12.0;
        settings.motion_blur = 0.7;
        settings.motion_blur_max_pixels = 24.0;
        let grade = settings.post_grade();
        assert_eq!(grade.temperature, -0.4);
        assert_eq!(grade.tint, 0.25);
        assert_eq!(grade.dof_focus_distance, 3.5);
        assert_eq!(grade.dof_aperture, 0.6);
        assert_eq!(grade.dof_max_blur, 12.0);
        assert_eq!(grade.motion_blur, 0.7);
        assert_eq!(grade.motion_blur_max_pixels, 24.0);
    }

    #[test]
    fn render_aspect_prefers_the_internal_resolution() {
        // #613: when internal resolution active camera must project by internal ratio, not window ratio.
        let mut settings = RenderSettings::default();
        assert_eq!(settings.internal_resolution, [0, 0]);
        assert_eq!(settings.internal_scale_mode, InternalScaleMode::Stretch);
        // Without internal resolution use window ratio.
        assert!((settings.render_aspect(21.0 / 9.0) - 21.0 / 9.0).abs() < 1e-6);

        settings.internal_resolution = [640, 360];
        assert!((settings.render_aspect(21.0 / 9.0) - 16.0 / 9.0).abs() < 1e-6);

        // When only one dimension given aspect same as window, fall back to window ratio.
        settings.internal_resolution = [640, 0];
        assert!((settings.render_aspect(4.0 / 3.0) - 4.0 / 3.0).abs() < 1e-6);

        // Bad window ratio also has deterministic result, won't pass NaN to projection matrix.
        settings.internal_resolution = [0, 0];
        assert_eq!(settings.render_aspect(f32::NAN), 16.0 / 9.0);
        assert_eq!(settings.render_aspect(0.0), 16.0 / 9.0);
    }
}
