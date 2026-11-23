//! Application config  loaded from `assets/settings.ini`.
//!
//! All fields have default values, so this file is entirely optional — missing sections / keys silently
//! use defaults; a malformed value logs one `log::warn!` and falls back to the default (see [`ini`]).
//!
//! Old projects' `assets/settings.toml` is still readable (`load()` falls back with a migration hint),
//! but the **write** side always emits `settings.ini`; one-time migration is completed by
//! `prism-build-pipeline build-scenes`.

pub mod ini;

use ini::Ini;

use crate::render_settings::{InternalScaleMode, RenderMode, RenderSettings};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AppConfig {
    pub app: AppInfo,
    pub window: WindowConfig,
    pub render: RenderConfig,
}

/// `[render]` section: render / debug / path-tracing defaults at startup.
///
/// All fields optional — falls back to [`RenderSettings::default`]. The editor (Tauri / egui)
/// reads and writes the same file, so field names here are the external contract for project-level render settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RenderConfig {
    /// `"raster"` or `"path_trace"`.
    pub render_mode: Option<String>,
    /// PBR isolate bit mask (0 = normal render).
    pub debug_flags: Option<u32>,
    /// 0 = Reinhard, 1 = ACES, 2 = passthrough (2D / pixel art).
    pub tonemap_mode: Option<u32>,
    /// Post-processing antialiasing: 0 = off (default), 1 = FXAA.
    pub aa_mode: Option<u32>,
    /// Occlusion culling (#1007): use previous-frame coarse depth to cull fully occluded draw items.
    pub occlusion_culling: Option<bool>,
    /// Display space exposure (`1.0` = unchanged, #502).
    pub contrast: Option<f32>,
    /// Main directional light shadow coverage distance (meters, see `RenderSettings::shadow_distance`).
    pub shadow_distance: Option<f32>,
    /// Shadow map edge length (texels, see `RenderSettings::shadow_map_size`, #526).
    pub shadow_map_size: Option<u32>,
    /// Directional light cascade count (1/2, see `RenderSettings::shadow_cascade_count`, #526).
    pub shadow_cascade_count: Option<u32>,
    /// Display space saturation (`1.0` = unchanged, `0.0` = grayscale).
    pub saturation: Option<f32>,
    /// Vignette strength (`0.0` = off, `1.0` = corners fully black).
    pub vignette: Option<f32>,
    /// HDR bloom switch (#502).
    pub bloom: Option<bool>,
    /// Bloom additive strength (`0.0` = off, `1.0` = strongest).
    pub bloom_intensity: Option<f32>,
    /// Bloom brightness threshold (HDR luminance).
    pub bloom_threshold: Option<f32>,
    /// Lens chromatic aberration strength (`0.0` = off, `1.0` = strongest, #502 Phase 2).
    pub chromatic_aberration: Option<f32>,
    /// Film grain strength (`0.0` = off, `1.0` = strongest, #502 Phase 2).
    pub film_grain: Option<f32>,
    /// White balance color temperature (`-1.0` = cool, `0.0` = neutral, `1.0` = warm, #502).
    pub temperature: Option<f32>,
    /// White balance hue (`-1.0` = greenish, `0.0` = neutral, `1.0` = magenta, #502).
    pub tint: Option<f32>,
    /// Depth of field focus distance (meters, #502 Phase 2).
    pub dof_focus_distance: Option<f32>,
    /// Depth of field strength (`0.0` = off, `1.0` = strongest, #502 Phase 2).
    pub dof_aperture: Option<f32>,
    /// Depth of field max blur radius (pixels, #502 Phase 2).
    pub dof_max_blur: Option<f32>,
    /// 3D color grading LUT `assets/`-relative path (`.cube`, #502); empty = disabled.
    pub lut: Option<String>,
    /// LUT blend strength (`0.0`..=`1.0`, #502).
    pub lut_strength: Option<f32>,
    /// Camera motion blur strength (`0.0` = off, `1.0` = strongest, #502 Phase 2).
    pub motion_blur: Option<f32>,
    /// Motion blur max trail length (pixels, #502 Phase 2).
    pub motion_blur_max_pixels: Option<f32>,
    /// Temporal anti-aliasing (#883).
    pub taa: Option<bool>,
    /// TAA history blend strength (0..=0.95).
    pub taa_strength: Option<f32>,
    /// Qualcomm Adreno Neural Fusion super-resolution (#1161).
    pub anf_sr: Option<bool>,
    /// ANF sharpening strength (0.0..=1.0).
    pub anf_sharpness: Option<f32>,
    /// Qualcomm Adreno Neural Fusion frame interpolation (#1161).
    pub anf_fg: Option<bool>,
    /// 0 = HDR tonemap, 1 = depth, 2 = view-space normal.
    pub debug_rt: Option<u32>,
    /// Forward pipeline wireframe mode (#431).
    pub wireframe: Option<bool>,
    /// Fill + wireframe overlay (#431).
    pub shaded_wireframe: Option<bool>,
    pub pt_max_bounces: Option<u32>,
    pub pt_ray_max_distance: Option<f32>,
    /// 0 = accumulate forever.
    pub pt_max_iterations: Option<u32>,
    /// Internal render resolution `"640x360"` (#613); `"off"` / empty = render at window size.
    pub internal_resolution: Option<String>,
    /// Upscale mode: `"stretch"` (default, fill window) or `"integer"` (integer scale + letterbox).
    pub internal_scale_mode: Option<String>,
    /// GI technique for `RenderMode::Raster` (`"baked_probe"`, `"restir"`, `"prt"`).
    pub raster_gi: Option<String>,
    /// Radiance cache / GI technique for `RenderMode::PathTrace` (`"off"`, `"sharc"`, `"nrc"`).
    pub pt_gi: Option<String>,
    /// PRT64 trilinear weight bias for thin-wall anti-leakage (default 0.05).
    pub prt_leak_bias: Option<f32>,
    /// DLSS Super Resolution mode (`"off"`, `"quality"`, `"balanced"`, `"performance"`, `"ultra_performance"`, `"dlaa"`).
    pub dlss_mode: Option<String>,
    /// DLSS pre-upscale filter (`"linear"`, `"nearest"`, `"off"`) when `render_extent < dlss_min`.
    pub dlss_pre_upscale: Option<String>,
    /// DLSS Frame Generation (DLFG) mode (`"off"`, `"on"`, `"auto"`).
    pub dlfg_mode: Option<String>,
    /// Number of generated frames per rendered frame (`1` = 2x, `2` = 3x, `3` = 4x).
    pub dlfg_frames: Option<u32>,
    /// Enable DLSS Ray Reconstruction (`true` / `false`).
    pub dlss_rr_enabled: Option<bool>,
}

impl RenderConfig {
    /// Apply config overrides to runtime settings (only explicitly provided fields).
    pub fn apply(&self, settings: &mut RenderSettings) {
        if let Some(mode) = &self.render_mode {
            settings.render_mode = match mode.as_str() {
                "path_trace" | "path-trace" | "pathtrace" => RenderMode::PathTrace,
                "sprites_2d" | "sprites2d" | "2d" => RenderMode::Sprites2D,
                "raster" => RenderMode::Raster,
                other => {
                    log::warn!("settings: unknown render_mode '{other}'; using raster");
                    RenderMode::Raster
                }
            };
        }
        if let Some(mode) = &self.raster_gi {
            settings.raster_gi = match mode.trim().to_ascii_lowercase().as_str() {
                "baked_probe" | "baked" | "probe" | "off" | "none" => {
                    crate::render_settings::RasterGiMode::BakedProbe
                }
                "restir" | "restir_gi" => crate::render_settings::RasterGiMode::Restir,
                "prt" | "prt64" => crate::render_settings::RasterGiMode::Prt,
                "ddgi" | "rtxgi" => crate::render_settings::RasterGiMode::Ddgi,
                other => {
                    log::warn!("settings: unknown raster_gi '{other}'; using baked_probe");
                    crate::render_settings::RasterGiMode::BakedProbe
                }
            };
        }
        if let Some(mode) = &self.pt_gi {
            settings.pt_gi = match mode.trim().to_ascii_lowercase().as_str() {
                "off" | "none" => crate::render_settings::PtGiMode::Off,
                "sharc" => crate::render_settings::PtGiMode::Sharc,
                "nrc" => crate::render_settings::PtGiMode::Nrc,
                other => {
                    log::warn!("settings: unknown pt_gi '{other}'; using off");
                    crate::render_settings::PtGiMode::Off
                }
            };
        }
        if let Some(bias) = self.prt_leak_bias {
            settings.prt_leak_bias = bias;
        }
        if let Some(mode) = &self.dlss_mode {
            settings.dlss_mode = match mode.trim().to_ascii_lowercase().as_str() {
                "off" | "none" => crate::render_settings::DlssMode::Off,
                "ultra_performance" | "ultraperformance" => {
                    crate::render_settings::DlssMode::UltraPerformance
                }
                "performance" => crate::render_settings::DlssMode::Performance,
                "balanced" => crate::render_settings::DlssMode::Balanced,
                "quality" => crate::render_settings::DlssMode::Quality,
                "dlaa" => crate::render_settings::DlssMode::Dlaa,
                other => {
                    log::warn!("settings: unknown dlss_mode '{other}'; using off");
                    crate::render_settings::DlssMode::Off
                }
            };
        }
        if let Some(mode) = &self.dlss_pre_upscale {
            if let Some(parsed) = crate::render_settings::DlssPreUpscaleMode::parse(mode) {
                settings.dlss_pre_upscale = parsed;
            } else {
                log::warn!("settings: unknown dlss_pre_upscale '{mode}'; using linear");
            }
        }
        if let Some(mode) = &self.dlfg_mode {
            settings.dlfg_mode = match mode.trim().to_ascii_lowercase().as_str() {
                "off" | "none" => crate::render_settings::DlssgMode::Off,
                "on" | "enabled" => crate::render_settings::DlssgMode::On,
                "auto" => crate::render_settings::DlssgMode::Auto,
                other => {
                    log::warn!("settings: unknown dlfg_mode '{other}'; using off");
                    crate::render_settings::DlssgMode::Off
                }
            };
        }
        if let Some(frames) = self.dlfg_frames {
            settings.dlfg_frames = frames.clamp(1, 3);
        }
        if let Some(rr) = self.dlss_rr_enabled {
            settings.dlss_rr_enabled = rr;
        }
        if let Some(flags) = self.debug_flags {
            settings.debug_flags = flags;
        }
        if let Some(wireframe) = self.wireframe {
            settings.wireframe = wireframe;
        }
        if let Some(shaded_wireframe) = self.shaded_wireframe {
            settings.shaded_wireframe = shaded_wireframe;
        }
        if let Some(mode) = self.tonemap_mode {
            settings.tonemap_mode = mode;
        }
        if let Some(mode) = self.aa_mode {
            settings.aa_mode = mode;
        }
        if let Some(enabled) = self.occlusion_culling {
            settings.occlusion_culling = enabled;
        }
        if let Some(distance) = self.shadow_distance {
            if distance.is_finite() && distance > 0.0 {
                settings.shadow_distance = distance;
            } else {
                log::warn!("settings: shadow_distance must be positive, got {distance}");
            }
        }
        if let Some(size) = self.shadow_map_size {
            if (prism_render::MIN_SHADOW_MAP_SIZE..=prism_render::MAX_SHADOW_MAP_SIZE)
                .contains(&size)
            {
                settings.shadow_map_size = size;
            } else {
                log::warn!(
                    "settings: shadow_map_size must be {}..={}, got {size}",
                    prism_render::MIN_SHADOW_MAP_SIZE,
                    prism_render::MAX_SHADOW_MAP_SIZE
                );
            }
        }
        if let Some(count) = self.shadow_cascade_count {
            if (1..=2).contains(&count) {
                settings.shadow_cascade_count = count;
            } else {
                log::warn!("settings: shadow_cascade_count must be 1 or 2, got {count}");
            }
        }
        if let Some(value) = self.contrast {
            settings.contrast = value.max(0.0);
        }
        if let Some(value) = self.saturation {
            settings.saturation = value.max(0.0);
        }
        if let Some(value) = self.vignette {
            settings.vignette = value.clamp(0.0, 1.0);
        }
        if let Some(bloom) = self.bloom {
            settings.bloom = bloom;
        }
        if let Some(value) = self.bloom_intensity {
            settings.bloom_intensity = value.clamp(0.0, 1.0);
        }
        if let Some(value) = self.bloom_threshold {
            settings.bloom_threshold = value.max(0.0);
        }
        if let Some(value) = self.chromatic_aberration {
            settings.chromatic_aberration = value.clamp(0.0, 1.0);
        }
        if let Some(value) = self.film_grain {
            settings.film_grain = value.clamp(0.0, 1.0);
        }
        if let Some(value) = self.temperature {
            settings.temperature = value.clamp(-1.0, 1.0);
        }
        if let Some(value) = self.tint {
            settings.tint = value.clamp(-1.0, 1.0);
        }
        if let Some(value) = self.dof_focus_distance {
            settings.dof_focus_distance = value.max(0.001);
        }
        if let Some(value) = self.dof_aperture {
            settings.dof_aperture = value.clamp(0.0, 1.0);
        }
        if let Some(value) = self.dof_max_blur {
            settings.dof_max_blur = value.clamp(0.0, 32.0);
        }
        if let Some(value) = &self.lut {
            settings.lut = value.trim().to_string();
        }
        if let Some(value) = self.lut_strength {
            settings.lut_strength = value.clamp(0.0, 1.0);
        }
        if let Some(value) = self.motion_blur {
            settings.motion_blur = value.clamp(0.0, 1.0);
        }
        if let Some(value) = self.motion_blur_max_pixels {
            settings.motion_blur_max_pixels = value.clamp(0.0, 64.0);
        }
        if let Some(value) = self.taa {
            settings.taa = value;
        }
        if let Some(value) = self.taa_strength {
            settings.taa_strength = value.clamp(0.0, 0.95);
        }
        if let Some(value) = self.anf_sr {
            settings.anf_sr = value;
        }
        if let Some(value) = self.anf_sharpness {
            settings.anf_sharpness = value.clamp(0.0, 1.0);
        }
        if let Some(value) = self.anf_fg {
            settings.anf_fg = value;
        }
        if let Some(rt) = self.debug_rt {
            settings.debug_rt = rt;
        }
        if let Some(bounces) = self.pt_max_bounces {
            settings.pt_max_bounces = bounces;
        }
        if let Some(distance) = self.pt_ray_max_distance {
            settings.pt_ray_max_distance = distance;
        }
        if let Some(iterations) = self.pt_max_iterations {
            settings.pt_max_iterations = iterations;
        }
        if let Some(value) = &self.internal_resolution {
            match parse_internal_resolution(value) {
                Some(resolution) => settings.internal_resolution = resolution,
                None => log::warn!(
                    "settings: internal_resolution '{value}' is not '<w>x<h>' or 'off'; \
                     rendering at window resolution"
                ),
            }
        }
        if let Some(value) = &self.internal_scale_mode {
            match InternalScaleMode::parse(value) {
                Some(mode) => settings.internal_scale_mode = mode,
                None => {
                    log::warn!("settings: unknown internal_scale_mode '{value}'; using 'stretch'")
                }
            }
        }
    }

    fn from_ini(ini: &Ini) -> Self {
        Self {
            render_mode: ini.get("render", "render_mode").map(str::to_string),
            debug_flags: ini.get_u32("render", "debug_flags"),
            tonemap_mode: ini.get_u32("render", "tonemap_mode"),
            aa_mode: ini.get_u32("render", "aa_mode"),
            occlusion_culling: ini.get_bool("render", "occlusion_culling"),
            contrast: ini.get_f32("render", "contrast"),
            shadow_distance: ini.get_f32("render", "shadow_distance"),
            shadow_map_size: ini.get_u32("render", "shadow_map_size"),
            shadow_cascade_count: ini.get_u32("render", "shadow_cascade_count"),
            saturation: ini.get_f32("render", "saturation"),
            vignette: ini.get_f32("render", "vignette"),
            bloom: ini.get_bool("render", "bloom"),
            bloom_intensity: ini.get_f32("render", "bloom_intensity"),
            bloom_threshold: ini.get_f32("render", "bloom_threshold"),
            chromatic_aberration: ini.get_f32("render", "chromatic_aberration"),
            film_grain: ini.get_f32("render", "film_grain"),
            temperature: ini.get_f32("render", "temperature"),
            tint: ini.get_f32("render", "tint"),
            dof_focus_distance: ini.get_f32("render", "dof_focus_distance"),
            dof_aperture: ini.get_f32("render", "dof_aperture"),
            dof_max_blur: ini.get_f32("render", "dof_max_blur"),
            lut: ini.get("render", "lut").map(str::to_string),
            lut_strength: ini.get_f32("render", "lut_strength"),
            motion_blur: ini.get_f32("render", "motion_blur"),
            motion_blur_max_pixels: ini.get_f32("render", "motion_blur_max_pixels"),
            taa: ini.get_bool("render", "taa"),
            taa_strength: ini.get_f32("render", "taa_strength"),
            anf_sr: ini.get_bool("render", "anf_sr"),
            anf_sharpness: ini.get_f32("render", "anf_sharpness"),
            anf_fg: ini.get_bool("render", "anf_fg"),
            debug_rt: ini.get_u32("render", "debug_rt"),
            wireframe: ini.get_bool("render", "wireframe"),
            shaded_wireframe: ini.get_bool("render", "shaded_wireframe"),
            pt_max_bounces: ini.get_u32("render", "pt_max_bounces"),
            pt_ray_max_distance: ini.get_f32("render", "pt_ray_max_distance"),
            pt_max_iterations: ini.get_u32("render", "pt_max_iterations"),
            internal_resolution: ini.get("render", "internal_resolution").map(str::to_string),
            internal_scale_mode: ini.get("render", "internal_scale_mode").map(str::to_string),
            raster_gi: ini.get("render", "raster_gi").map(str::to_string),
            pt_gi: ini.get("render", "pt_gi").map(str::to_string),
            prt_leak_bias: ini.get_f32("render", "prt_leak_bias"),
            dlss_mode: ini.get("render", "dlss_mode").map(str::to_string),
            dlss_pre_upscale: ini.get("render", "dlss_pre_upscale").map(str::to_string),
            dlfg_mode: ini.get("render", "dlfg_mode").map(str::to_string),
            dlfg_frames: ini.get_u32("render", "dlfg_frames"),
            dlss_rr_enabled: ini.get_bool("render", "dlss_rr_enabled"),
        }
    }
}

/// Parse `"640x360"` / `"640,360"` / `"640*360"`; `"off"` / `"native"` / empty → `[0, 0]`.
fn parse_internal_resolution(value: &str) -> Option<[u32; 2]> {
    let lower = value.trim().to_ascii_lowercase();
    if lower.is_empty() || lower == "off" || lower == "native" || lower == "none" {
        return Some([0, 0]);
    }
    let mut parts = lower.split(['x', ',', '*']);
    let width: u32 = parts.next()?.trim().parse().ok()?;
    let height: u32 = parts.next()?.trim().parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some([width, height])
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppInfo {
    pub company: String,
    pub name: String,
}

impl Default for AppInfo {
    fn default() -> Self {
        Self {
            company: default_company(),
            name: default_app_name(),
        }
    }
}

impl AppInfo {
    fn from_ini(ini: &Ini) -> Self {
        let mut info = Self::default();
        if let Some(value) = ini.get("app", "company") {
            info.company = value.to_string();
        }
        if let Some(value) = ini.get("app", "name") {
            info.name = value.to_string();
        }
        info
    }
}

fn default_company() -> String {
    "Excurs1ons".to_string()
}

fn default_app_name() -> String {
    "Prisma".to_string()
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowConfig {
    pub title: String,

    pub width: u32,
    pub height: u32,

    pub min_width: Option<u32>,
    pub min_height: Option<u32>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,

    pub position_x: Option<i32>,
    pub position_y: Option<i32>,

    pub resizable: bool,

    pub fullscreen: bool,

    pub maximized: bool,

    pub visible: bool,

    pub decorations: bool,

    pub vsync: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: default_title(),
            width: default_width(),
            height: default_height(),
            min_width: None,
            min_height: None,
            max_width: None,
            max_height: None,
            position_x: None,
            position_y: None,
            resizable: default_resizable(),
            fullscreen: false,
            maximized: false,
            visible: default_visible(),
            decorations: default_decorations(),
            vsync: default_vsync(),
        }
    }
}

impl WindowConfig {
    fn from_ini(ini: &Ini) -> Self {
        let mut window = Self::default();
        if let Some(value) = ini.get("window", "title") {
            window.title = value.to_string();
        }
        window.width = ini.get_u32("window", "width").unwrap_or(window.width);
        window.height = ini.get_u32("window", "height").unwrap_or(window.height);
        window.min_width = ini.get_u32("window", "min_width");
        window.min_height = ini.get_u32("window", "min_height");
        window.max_width = ini.get_u32("window", "max_width");
        window.max_height = ini.get_u32("window", "max_height");
        window.position_x = ini.get_i32("window", "position_x");
        window.position_y = ini.get_i32("window", "position_y");
        window.resizable = ini
            .get_bool("window", "resizable")
            .unwrap_or(window.resizable);
        window.fullscreen = ini
            .get_bool("window", "fullscreen")
            .unwrap_or(window.fullscreen);
        window.maximized = ini
            .get_bool("window", "maximized")
            .unwrap_or(window.maximized);
        window.visible = ini.get_bool("window", "visible").unwrap_or(window.visible);
        window.decorations = ini
            .get_bool("window", "decorations")
            .unwrap_or(window.decorations);
        window.vsync = ini.get_bool("window", "vsync").unwrap_or(window.vsync);
        window
    }
}

fn default_title() -> String {
    "Prisma".to_string()
}

const fn default_width() -> u32 {
    1600
}

const fn default_height() -> u32 {
    900
}

const fn default_resizable() -> bool {
    true
}

const fn default_visible() -> bool {
    true
}

const fn default_decorations() -> bool {
    true
}

const fn default_vsync() -> bool {
    true
}

const CONFIG_PATH: &str = "settings.ini";
const LEGACY_CONFIG_PATH: &str = "settings.toml";

/// Sections / keys recognized by the engine. Everything else ignored (one `log::debug!` entry),
/// so when the editor writes `[build]` and other own sections into the same file, engine does not complain.
const KNOWN_KEYS: &[(&str, &[&str])] = &[
    ("app", &["company", "name"]),
    ("build", &[]),
    (
        "window",
        &[
            "title",
            "width",
            "height",
            "min_width",
            "min_height",
            "max_width",
            "max_height",
            "position_x",
            "position_y",
            "resizable",
            "fullscreen",
            "maximized",
            "visible",
            "decorations",
            "vsync",
        ],
    ),
    (
        "render",
        &[
            "render_mode",
            "debug_flags",
            "tonemap_mode",
            "aa_mode",
            "occlusion_culling",
            "contrast",
            "shadow_distance",
            "shadow_map_size",
            "shadow_cascade_count",
            "saturation",
            "vignette",
            "bloom",
            "bloom_intensity",
            "bloom_threshold",
            "chromatic_aberration",
            "film_grain",
            "temperature",
            "tint",
            "dof_focus_distance",
            "dof_aperture",
            "dof_max_blur",
            "lut",
            "lut_strength",
            "motion_blur",
            "motion_blur_max_pixels",
            "taa",
            "taa_strength",
            "anf_sr",
            "anf_sharpness",
            "anf_fg",
            "debug_rt",
            "wireframe",
            "shaded_wireframe",
            "pt_max_bounces",
            "pt_ray_max_distance",
            "pt_max_iterations",
            "internal_resolution",
            "internal_scale_mode",
            "raster_gi",
            "pt_gi",
            "prt_leak_bias",
        ],
    ),
];

fn log_unknown_entries(ini: &Ini) {
    for section in ini.sections() {
        let known = KNOWN_KEYS
            .iter()
            .find(|(name, _)| *name == section.name.as_str())
            .map(|(_, keys)| *keys);
        let Some(keys) = known else {
            log::debug!("settings: ignoring unknown section [{}]", section.name);
            continue;
        };
        for (key, _) in &section.entries {
            if !keys.contains(&key.as_str()) {
                log::debug!("settings: ignoring unknown key [{}.{}]", section.name, key);
            }
        }
    }
}

impl AppConfig {
    /// Parse config from caller-provided text; no filesystem access.
    pub fn from_ini(text: &str) -> Self {
        let ini = Ini::parse(text);
        log_unknown_entries(&ini);
        Self {
            app: AppInfo::from_ini(&ini),
            window: WindowConfig::from_ini(&ini),
            render: RenderConfig::from_ini(&ini),
        }
    }

    /// Load `assets/settings.ini`; if missing fall back to old `assets/settings.toml`
    /// and hint migration. When neither exists return defaults.
    ///
    /// Path resolved via [`crate::asset_paths`], does not depend on process cwd.
    #[deprecated(note = "use prism_app::load_config or AppConfig::from_ini")]
    pub fn load() -> Self {
        for (name, legacy) in [(CONFIG_PATH, false), (LEGACY_CONFIG_PATH, true)] {
            let Some(path) = crate::asset_paths::asset_path(name) else {
                return Self::default();
            };
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    log::warn!("{name} ({}): {error} — using defaults", path.display());
                    return Self::default();
                }
            };
            if legacy {
                log::warn!(
                    "{} is deprecated: run `prism-build-pipeline build-scenes` to migrate it \
                     to {CONFIG_PATH}",
                    path.display()
                );
            }
            return Self::from_ini(&text);
        }
        Self::default()
    }
}

/// Returns the platform-specific data directory for the application
/// (e.g., `%APPDATA%\Excurs1ons\Prisma\` on Windows
///
/// Reads company/app name from the default [`AppConfig`] (it does **not** read
/// `settings.ini`; callers with a loaded config should use
/// [`app_data_dir_for`]).  Creates the directory if it doesn't exist.  Returns
/// `None` if the platform data dir cannot be determined (unlikely on desktop,
/// possible on unusual platforms).
pub fn app_data_dir() -> Option<std::path::PathBuf> {
    let cfg = AppConfig::default();
    app_data_dir_for(&cfg)
}

/// Compute the persistent data directory from a caller-provided config.
///
/// Unlike [`app_data_dir`], this never reads `settings.ini` from disk.
pub fn app_data_dir_for(cfg: &AppConfig) -> Option<std::path::PathBuf> {
    let base = dirs::data_dir()?;
    let dir = base.join(&cfg.app.company).join(&cfg.app.name);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::{AppConfig, Ini};

    #[test]
    fn parses_in_memory_config_without_filesystem() {
        let cfg = AppConfig::from_ini("[window]\nwidth = 1024\nheight = 576\n");
        assert_eq!(cfg.window.width, 1024);
        assert_eq!(cfg.window.height, 576);
        assert_eq!(cfg.window.title, "Prisma");
    }

    #[test]
    fn malformed_in_memory_config_uses_defaults() {
        let cfg = AppConfig::from_ini("[window\nwidth = nope");
        assert_eq!(cfg.window.width, 1600);
        assert_eq!(cfg.window.height, 900);
    }

    #[test]
    fn quotes_bools_and_negative_numbers() {
        let cfg = AppConfig::from_ini(
            "[app]\ncompany = \"Acme\"\nname = \"Game\"\n\
             [window]\ntitle = \"My Game\"\nwidth = 1280\nvsync = false\nposition_x = -40\n",
        );
        assert_eq!(cfg.app.company, "Acme");
        assert_eq!(cfg.app.name, "Game");
        assert_eq!(cfg.window.title, "My Game");
        assert_eq!(cfg.window.width, 1280);
        assert!(!cfg.window.vsync);
        assert_eq!(cfg.window.position_x, Some(-40));
    }

    #[test]
    fn render_section_overrides_runtime_settings() {
        use crate::render_settings::{RenderMode, RenderSettings};

        let cfg = AppConfig::from_ini(
            "[render]\nrender_mode = \"path_trace\"\ntonemap_mode = 1\naa_mode = 1\nocclusion_culling = true\ncontrast = 1.4\nsaturation = 0.6\nvignette = 0.3\nbloom = true\nbloom_intensity = 0.4\nbloom_threshold = 2.5\nchromatic_aberration = 0.25\nfilm_grain = 0.35\ntemperature = -0.4\ntint = 0.2\ndof_focus_distance = 4.5\ndof_aperture = 0.6\ndof_max_blur = 12\nlut = \"luts/grade.cube\"\nlut_strength = 0.8\nmotion_blur = 0.7\nmotion_blur_max_pixels = 24\ntaa = true\ntaa_strength = 0.75\nshadow_distance = 120.0\nshadow_map_size = 1024\nshadow_cascade_count = 2\nwireframe = true\nshaded_wireframe = true\npt_max_bounces = 7\n",
        );
        let mut settings = RenderSettings::default();
        cfg.render.apply(&mut settings);
        assert_eq!(settings.render_mode, RenderMode::PathTrace);
        assert_eq!(settings.tonemap_mode, 1);
        assert_eq!(settings.aa_mode, 1);
        assert!(settings.occlusion_culling);
        assert_eq!(settings.contrast, 1.4);
        assert_eq!(settings.saturation, 0.6);
        assert_eq!(settings.vignette, 0.3);
        assert!(settings.bloom);
        assert_eq!(settings.bloom_intensity, 0.4);
        assert_eq!(settings.bloom_threshold, 2.5);
        assert_eq!(settings.chromatic_aberration, 0.25);
        assert_eq!(settings.film_grain, 0.35);
        assert_eq!(settings.temperature, -0.4);
        assert_eq!(settings.tint, 0.2);
        assert_eq!(settings.dof_focus_distance, 4.5);
        assert_eq!(settings.dof_aperture, 0.6);
        assert_eq!(settings.dof_max_blur, 12.0);
        assert_eq!(settings.lut, "luts/grade.cube");
        assert_eq!(settings.lut_strength, 0.8);
        assert_eq!(settings.motion_blur, 0.7);
        assert_eq!(settings.motion_blur_max_pixels, 24.0);
        assert!(settings.taa);
        assert_eq!(settings.taa_strength, 0.75);
        assert_eq!(settings.pt_max_bounces, 7);
        assert_eq!(settings.shadow_distance, 120.0);
        assert_eq!(settings.shadow_map_size, 1024);
        assert_eq!(settings.shadow_cascade_count, 2);
        assert!(settings.wireframe);
        assert!(settings.shaded_wireframe);
        assert_eq!(settings.pt_max_iterations, 0);

        let mut clamped = RenderSettings::default();
        cfg.render.apply(&mut clamped);
        let mut extreme = cfg.render.clone();
        extreme.temperature = Some(5.0);
        extreme.tint = Some(-5.0);
        extreme.dof_focus_distance = Some(-1.0);
        extreme.dof_aperture = Some(3.0);
        extreme.dof_max_blur = Some(100.0);
        extreme.lut_strength = Some(5.0);
        extreme.motion_blur = Some(5.0);
        extreme.motion_blur_max_pixels = Some(100.0);
        extreme.taa_strength = Some(5.0);
        extreme.apply(&mut clamped);
        assert_eq!(clamped.temperature, 1.0);
        assert_eq!(clamped.tint, -1.0);
        assert_eq!(clamped.dof_focus_distance, 0.001);
        assert_eq!(clamped.dof_aperture, 1.0);
        assert_eq!(clamped.dof_max_blur, 32.0);
        assert_eq!(clamped.lut_strength, 1.0);
        assert_eq!(clamped.motion_blur, 1.0);
        assert_eq!(clamped.motion_blur_max_pixels, 64.0);
        assert_eq!(clamped.taa_strength, 0.95);
    }

    #[test]
    fn shadow_cascade_count_outside_one_or_two_is_ignored() {
        use crate::render_settings::RenderSettings;

        let cfg = AppConfig::from_ini("[render]\nshadow_cascade_count = 3\n");
        let mut settings = RenderSettings::default();
        cfg.render.apply(&mut settings);
        assert_eq!(settings.shadow_cascade_count, 1);
    }

    #[test]
    fn shadow_map_size_outside_the_supported_range_is_ignored() {
        use crate::render_settings::RenderSettings;

        let cfg = AppConfig::from_ini("[render]\nshadow_map_size = 64\n");
        let mut settings = RenderSettings::default();
        cfg.render.apply(&mut settings);
        assert_eq!(
            settings.shadow_map_size,
            prism_render::SHADOW_MAP_SIZE,
            "out-of-range values must not reach the renderer"
        );
    }

    #[test]
    fn internal_resolution_and_scale_mode_come_from_the_render_section() {
        use crate::render_settings::{InternalScaleMode, RenderSettings};

        let cfg = AppConfig::from_ini(
            "[render]\ninternal_resolution = 640x360\ninternal_scale_mode = integer\n",
        );
        let mut settings = RenderSettings::default();
        cfg.render.apply(&mut settings);
        assert_eq!(settings.internal_resolution, [640, 360]);
        assert_eq!(settings.internal_scale_mode, InternalScaleMode::Integer);

        for (value, expected) in [
            ("640,360", [640, 360]),
            ("640*360", [640, 360]),
            ("off", [0, 0]),
            ("native", [0, 0]),
            ("", [0, 0]),
        ] {
            let key = format!("[render]\ninternal_resolution = {value}\n");
            let mut settings = RenderSettings::default();
            AppConfig::from_ini(&key).render.apply(&mut settings);
            assert_eq!(settings.internal_resolution, expected, "{value}");
        }

        let mut settings = RenderSettings::default();
        AppConfig::from_ini(
            "[render]\ninternal_resolution = 640x360x2\ninternal_scale_mode = nope\n",
        )
        .render
        .apply(&mut settings);
        assert_eq!(settings.internal_resolution, [0, 0]);
        assert_eq!(settings.internal_scale_mode, InternalScaleMode::Stretch);
    }

    #[test]
    fn unknown_sections_and_keys_are_ignored() {
        let ini =
            Ini::parse("[build]\noutput_dir = \"out\"\n[app]\nnickname = \"x\"\nname = \"ok\"\n");
        let cfg = AppConfig::from_ini(&ini.render());
        assert_eq!(cfg.app.name, "ok");
    }
}
