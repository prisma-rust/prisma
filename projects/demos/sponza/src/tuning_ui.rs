//! Sponza SHaRC & Path Tracing interactive tuning UI.
//!
//! Replicates the sections and controls of the NVIDIA RTXGI SDK
//! (`Samples/Pathtracer/UI.cpp`):
//! - **Generic**: Camera coordinates, VSync, Frame time / FPS.
//! - **Render**: Raster / Path Trace mode, Post (DLSS, Auto Exposure).
//! - **Path Tracing**: Mode (Reference, SHaRC, ReSTIR GI, NRC), Bounces, Ray Max Distance,
//!   Exposure.
//! - **SHaRC**: Enable Clear, Update, Resolve, Anti-firefly, Roughness threshold,
//!   Downscale factor, Scene scale, Accumulation frame count, Stale frame count.
//! - **ReSTIR GI**: raster-path real-time GI that reuses the same SHaRC cache; picking it
//!   also switches the renderer back to raster mode.
//! - **Lighting / Tonemapping**: Reinhard, ACES, Passthrough.
//! - **DLSS**: Off, Quality, Balanced, Performance, UltraPerformance, DLAA.
//!
//! Keyboard & Interaction:
//! - `F1` / `Tab`: Toggle Tuning Panel.
//! - `Up` / `Down` arrows or `W`/`S`: Navigate parameter rows.
//! - `Left` / `Right` arrows or `A`/`D`: Adjust value down/up.
//! - `Space` / `Enter`: Toggle boolean / trigger action.
//! - `1` / `2` / `3` / `4`: Quick select GI Mode (1: Reference, 2: SHaRC, 3: NRC, 4: ReSTIR GI).
//! - `T`: Cycle tonemapper (Reinhard -> ACES -> Passthrough).
//! - `R`: Reset accumulation cache.
//! - `V`: Toggle VSync.

use log::info;
use prism_app::FrameHook;
use prism_app::{RenderShared, RenderStats};
use prism_engine::ecs::{Entity, World};
use prism_engine::render_settings::{
    DlssMode, DlssPreUpscaleMode, DlssgMode, PtGiMode, RasterGiMode, RenderMode, RenderSettings,
};
use prism_platform::{Key, PlatformEvent, PlatformWindow};
use prism_scene::ui::{
    Anchors, Canvas, CanvasScaleMode, Margin, Node, Pivot, Style, Text, TextAlign,
};
use prism_scene::Active;

const NUM_ITEMS: usize = 22;
const SPONZA_CUSTOM_SHADER: &str = "shaders/sponza_custom.frag.spv";

#[derive(Debug)]
pub struct SponzaTuningHook {
    /// Pointer lock (camera takes over mouse/keyboard) snapshot, synced from window each frame by `on_tick`.
    ///
    /// Keyboard events carry no lock info, and panel key routing needs the split "locked → camera, unlocked → panel"
    /// stream, so store a copy here: event handlers read previous-frame value, faster than window query,
    /// and it will not change mid-frame.
    pub locked: bool,
    /// Previous-frame lock state: used to detect "just entered locked" frame.
    was_locked: bool,
    pub show: bool,
    pub selected_index: usize,

    // Parameters matching RTXGI Pathtracer UI
    pub render_mode: RenderMode,
    pub auto_exposure: bool,
    pub raster_gi: RasterGiMode,
    pub pt_gi: PtGiMode,
    pub bounces: u32,
    pub ray_max_distance: f32,
    pub tonemap_mode: u32,
    pub sharc_enable_clear: bool,
    pub sharc_enable_update: bool,
    pub sharc_enable_resolve: bool,
    pub sharc_enable_anti_firefly: bool,
    pub sharc_scene_scale: f32,
    pub sharc_capacity: u32,
    pub sharc_roughness_threshold: f32,
    pub sharc_downscale_factor: u32,
    pub sharc_accumulation_frame_num: u32,
    pub sharc_stale_frame_num: u32,
    pub dlss_mode: DlssMode,
    pub dlss_pre_upscale: DlssPreUpscaleMode,
    pub dlfg_mode: DlssgMode,
    pub dlfg_frames: u32,
    pub dlss_rr_enabled: bool,
    pub vsync: bool,
    pub custom_shader: bool,
    applied_custom_shader: Option<bool>,
    pub request_reset: bool,

    // ECS UI entities
    canvas_entity: Option<Entity>,
    panel_bg_entity: Option<Entity>,
    panel_text_entity: Option<Entity>,
    status_text_entity: Option<Entity>,
}

impl Default for SponzaTuningHook {
    fn default() -> Self {
        let (render_mode, raster_gi, pt_gi) = match std::env::var("SPONZA_RENDER_MODE") {
            Ok(mode) if mode.eq_ignore_ascii_case("raster") => {
                (RenderMode::Raster, RasterGiMode::BakedProbe, PtGiMode::Off)
            }
            Ok(mode) if mode.eq_ignore_ascii_case("prt") => {
                (RenderMode::Raster, RasterGiMode::Prt, PtGiMode::Off)
            }
            Ok(mode) if mode.eq_ignore_ascii_case("pt_off") => (
                RenderMode::PathTrace,
                RasterGiMode::BakedProbe,
                PtGiMode::Off,
            ),
            _ => (
                RenderMode::PathTrace,
                RasterGiMode::BakedProbe,
                PtGiMode::Sharc,
            ),
        };

        let auto_exposure = std::env::var("SPONZA_AUTO_EXPOSURE")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(!matches!(render_mode, RenderMode::Raster));
        let tonemap_mode = std::env::var("SPONZA_TONEMAP_MODE")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(if matches!(render_mode, RenderMode::Raster) {
                2
            } else {
                1
            });

        Self {
            locked: false,
            was_locked: false,
            show: true,
            selected_index: 0,
            render_mode,
            auto_exposure,
            raster_gi,
            pt_gi,
            bounces: 4,
            ray_max_distance: 1000.0,
            tonemap_mode,
            sharc_enable_clear: false,
            sharc_enable_update: true,
            sharc_enable_resolve: true,
            sharc_enable_anti_firefly: true,
            sharc_scene_scale: 50.0,
            sharc_capacity: 1 << 20,
            sharc_roughness_threshold: 0.4,
            sharc_downscale_factor: 5,
            sharc_accumulation_frame_num: 20,
            sharc_stale_frame_num: 60,
            dlss_mode: DlssMode::Off,
            dlss_pre_upscale: DlssPreUpscaleMode::Linear,
            dlfg_mode: DlssgMode::Off,
            dlfg_frames: 1,
            dlss_rr_enabled: false,
            vsync: false,
            custom_shader: true,
            applied_custom_shader: None,
            request_reset: false,
            canvas_entity: None,
            panel_bg_entity: None,
            panel_text_entity: None,
            status_text_entity: None,
        }
    }
}

impl SponzaTuningHook {
    fn cycle_render_mode(&mut self) {
        // Raster <-> Path Trace. Either direction invalidates the path tracing
        // accumulation.
        self.render_mode = match self.render_mode {
            RenderMode::PathTrace => RenderMode::Raster,
            _ => RenderMode::PathTrace,
        };
        if self.render_mode == RenderMode::Raster {
            self.auto_exposure = false;
            self.tonemap_mode = 2;
        } else {
            self.pt_gi = PtGiMode::Sharc;
            self.auto_exposure = true;
            self.tonemap_mode = 1;
        }
        self.request_reset = true;
    }

    fn adjust_parameter(&mut self, increase: bool) {
        match self.selected_index {
            0 => self.cycle_render_mode(),
            1 => {
                if self.render_mode == RenderMode::Raster {
                    self.raster_gi = match self.raster_gi {
                        RasterGiMode::BakedProbe => {
                            if increase {
                                RasterGiMode::Restir
                            } else {
                                RasterGiMode::Ddgi
                            }
                        }
                        RasterGiMode::Restir => {
                            if increase {
                                RasterGiMode::Prt
                            } else {
                                RasterGiMode::BakedProbe
                            }
                        }
                        RasterGiMode::Prt => {
                            if increase {
                                RasterGiMode::Ddgi
                            } else {
                                RasterGiMode::Restir
                            }
                        }
                        RasterGiMode::Ddgi => {
                            if increase {
                                RasterGiMode::BakedProbe
                            } else {
                                RasterGiMode::Prt
                            }
                        }
                    };
                } else {
                    self.pt_gi = match self.pt_gi {
                        PtGiMode::Off => {
                            if increase {
                                PtGiMode::Sharc
                            } else {
                                PtGiMode::Nrc
                            }
                        }
                        PtGiMode::Sharc => {
                            if increase {
                                PtGiMode::Nrc
                            } else {
                                PtGiMode::Off
                            }
                        }
                        PtGiMode::Nrc => {
                            if increase {
                                PtGiMode::Off
                            } else {
                                PtGiMode::Sharc
                            }
                        }
                    };
                }
                self.request_reset = true;
            }
            2 => {
                // Bounces: 1 .. 24
                if increase {
                    self.bounces = (self.bounces + 1).min(24);
                } else {
                    self.bounces = self.bounces.saturating_sub(1).max(1);
                }
                self.request_reset = true;
            }
            3 => {
                // Ray max distance: 100.0 .. 5000.0
                if increase {
                    self.ray_max_distance = (self.ray_max_distance + 100.0).min(5000.0);
                } else {
                    self.ray_max_distance = (self.ray_max_distance - 100.0).max(100.0);
                }
                self.request_reset = true;
            }
            4 => {
                // Tonemapper: 0=Reinhard, 1=ACES, 2=Passthrough
                if increase {
                    self.tonemap_mode = (self.tonemap_mode + 1) % 3;
                } else {
                    self.tonemap_mode = if self.tonemap_mode == 0 {
                        2
                    } else {
                        self.tonemap_mode - 1
                    };
                }
            }
            5 => self.sharc_enable_clear = !self.sharc_enable_clear,
            6 => self.sharc_enable_update = !self.sharc_enable_update,
            7 => self.sharc_enable_resolve = !self.sharc_enable_resolve,
            8 => self.sharc_enable_anti_firefly = !self.sharc_enable_anti_firefly,
            9 => {
                // Scene scale: 5.0 .. 100.0
                if increase {
                    self.sharc_scene_scale = (self.sharc_scene_scale + 5.0).min(100.0);
                } else {
                    self.sharc_scene_scale = (self.sharc_scene_scale - 5.0).max(5.0);
                }
                self.request_reset = true;
            }
            10 => {
                // Roughness threshold: 0.0 .. 1.0
                if increase {
                    self.sharc_roughness_threshold =
                        (self.sharc_roughness_threshold + 0.05).min(1.0);
                } else {
                    self.sharc_roughness_threshold =
                        (self.sharc_roughness_threshold - 0.05).max(0.0);
                }
                self.request_reset = true;
            }
            11 => {
                // Downscale factor: 1 .. 10
                if increase {
                    self.sharc_downscale_factor = (self.sharc_downscale_factor + 1).min(10);
                } else {
                    self.sharc_downscale_factor =
                        self.sharc_downscale_factor.saturating_sub(1).max(1);
                }
            }
            12 => {
                // Accumulation frames: 1 .. 120
                if increase {
                    self.sharc_accumulation_frame_num =
                        (self.sharc_accumulation_frame_num + 5).min(120);
                } else {
                    self.sharc_accumulation_frame_num =
                        self.sharc_accumulation_frame_num.saturating_sub(5).max(1);
                }
            }
            13 => {
                // Stale frames: 1 .. 120
                if increase {
                    self.sharc_stale_frame_num = (self.sharc_stale_frame_num + 5).min(120);
                } else {
                    self.sharc_stale_frame_num =
                        self.sharc_stale_frame_num.saturating_sub(5).max(1);
                }
            }
            14 => {
                // Cache capacity: 2^16 .. 2^23 entries (doubles per step).
                // Resizing rebuilds the buffers, so the cache restarts empty.
                if increase {
                    self.sharc_capacity = (self.sharc_capacity * 2).min(1 << 23);
                } else {
                    self.sharc_capacity = (self.sharc_capacity / 2).max(1 << 16);
                }
                self.request_reset = true;
            }
            15 => {
                // DLSS Mode
                self.dlss_mode = match self.dlss_mode {
                    DlssMode::Off => {
                        if increase {
                            DlssMode::Quality
                        } else {
                            DlssMode::Dlaa
                        }
                    }
                    DlssMode::Quality => {
                        if increase {
                            DlssMode::Balanced
                        } else {
                            DlssMode::Off
                        }
                    }
                    DlssMode::Balanced => {
                        if increase {
                            DlssMode::Performance
                        } else {
                            DlssMode::Quality
                        }
                    }
                    DlssMode::Performance => {
                        if increase {
                            DlssMode::UltraPerformance
                        } else {
                            DlssMode::Balanced
                        }
                    }
                    DlssMode::UltraPerformance => {
                        if increase {
                            DlssMode::Dlaa
                        } else {
                            DlssMode::Performance
                        }
                    }
                    DlssMode::Dlaa => {
                        if increase {
                            DlssMode::Off
                        } else {
                            DlssMode::UltraPerformance
                        }
                    }
                };
                self.request_reset = true;
            }
            16 => self.auto_exposure = !self.auto_exposure,
            17 => self.custom_shader = !self.custom_shader,
            18 => {
                self.dlfg_mode = match self.dlfg_mode {
                    DlssgMode::Off => {
                        if increase {
                            DlssgMode::On
                        } else {
                            DlssgMode::Auto
                        }
                    }
                    DlssgMode::On => {
                        if increase {
                            DlssgMode::Auto
                        } else {
                            DlssgMode::Off
                        }
                    }
                    DlssgMode::Auto => {
                        if increase {
                            DlssgMode::Off
                        } else {
                            DlssgMode::On
                        }
                    }
                };
            }
            19 => {
                self.dlss_rr_enabled = !self.dlss_rr_enabled;
            }
            20 => {
                self.dlss_pre_upscale = match self.dlss_pre_upscale {
                    DlssPreUpscaleMode::Linear => {
                        if increase {
                            DlssPreUpscaleMode::Nearest
                        } else {
                            DlssPreUpscaleMode::Off
                        }
                    }
                    DlssPreUpscaleMode::Nearest => {
                        if increase {
                            DlssPreUpscaleMode::Off
                        } else {
                            DlssPreUpscaleMode::Linear
                        }
                    }
                    DlssPreUpscaleMode::Off => {
                        if increase {
                            DlssPreUpscaleMode::Linear
                        } else {
                            DlssPreUpscaleMode::Nearest
                        }
                    }
                };
            }
            21 => {
                self.dlfg_frames = if increase {
                    (self.dlfg_frames % 3) + 1
                } else if self.dlfg_frames <= 1 {
                    3
                } else {
                    self.dlfg_frames - 1
                };
            }
            _ => {}
        }
    }

    fn toggle_parameter(&mut self) {
        match self.selected_index {
            0 | 1 | 15 | 18 | 20 | 21 => self.adjust_parameter(true),
            5 => self.sharc_enable_clear = !self.sharc_enable_clear,
            6 => self.sharc_enable_update = !self.sharc_enable_update,
            7 => self.sharc_enable_resolve = !self.sharc_enable_resolve,
            8 => self.sharc_enable_anti_firefly = !self.sharc_enable_anti_firefly,
            16 => self.auto_exposure = !self.auto_exposure,
            17 => self.custom_shader = !self.custom_shader,
            19 => self.dlss_rr_enabled = !self.dlss_rr_enabled,
            _ => {}
        }
    }
}

/// One key routing result.
///
/// Hook sees events before App: returning `false` lets App run its own shortcuts (F3 / Tab / T /
/// number keys PBR isolation) **and** pass key to InputManager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyRoute {
    /// Panel does not handle: pass through to App (including engine movement keys).
    Forward,
    /// Panel consumed the key (App shortcuts and engine key state no longer see it).
    Consume,
}

/// Panel visibility in locked state.
///
/// Two modes never overlap: **locked** = mouse hidden, panel hidden, WASD goes to bit; **unlocked** =
/// mouse shown, panel shown, WASD flips menu. So lock state **changing frame** directly aligns panel
/// to target state (lock hides, unlock expands), intermediate frames preserve player F1 / Tab
/// switches — unconditionally writing `show && !locked` each frame would erase hook just-set `show = true`
/// immediately, F1 / Tab would appear broken.
fn panel_visible_after_lock(show: bool, locked: bool, was_locked: bool) -> bool {
    if locked == was_locked {
        show
    } else {
        !locked
    }
}

impl SponzaTuningHook {
    /// Panel toggle swallowed by hook, otherwise App Tab (debug target switch) would fire at same time.
    fn on_key_pressed(&mut self, key: Key) -> KeyRoute {
        match key {
            // Panel toggle: no occlusion key may steal it, else panel opens then cannot close.
            Key::F1 | Key::Tab => {
                self.show = !self.show;
                return KeyRoute::Consume;
            }
            // GI mode is Sponza signature operation, panel should allow it even when hidden (consistent with
            // top status bar "[1] Ref [2] SHaRC [3] NRC").
            Key::Digit1 => {
                self.pt_gi = PtGiMode::Off;
                self.render_mode = RenderMode::PathTrace;
                self.request_reset = true;
                return KeyRoute::Consume;
            }
            Key::Digit2 => {
                self.pt_gi = PtGiMode::Sharc;
                self.render_mode = RenderMode::PathTrace;
                self.request_reset = true;
                return KeyRoute::Consume;
            }
            Key::Digit3 => {
                self.pt_gi = PtGiMode::Nrc;
                self.render_mode = RenderMode::PathTrace;
                self.request_reset = true;
                return KeyRoute::Consume;
            }
            Key::Digit4 => {
                self.raster_gi = RasterGiMode::Restir;
                self.render_mode = RenderMode::Raster;
                self.request_reset = true;
                return KeyRoute::Consume;
            }
            Key::Digit5 => {
                self.raster_gi = RasterGiMode::Prt;
                self.render_mode = RenderMode::Raster;
                self.request_reset = true;
                return KeyRoute::Consume;
            }
            // M / R / V are panel own function keys, camera never reads them
            // (`camera_controller_system` only looks at WASD / QE / Space / Ctrl / Shift),
            // so lock state does not affect key semantics, both states receive.
            // Previously added `!self.locked` guard for "locked → camera", but window auto-locks on focus,
            // effectively making these keys unreachable in game — M not responding came from this.
            Key::Letter(b'M') => {
                self.cycle_render_mode();
                return KeyRoute::Consume;
            }
            Key::Letter(b'R') => {
                self.request_reset = true;
                return KeyRoute::Consume;
            }
            Key::Letter(b'V') => {
                self.vsync = !self.vsync;
                return KeyRoute::Consume;
            }
            // Tone map also panel own setting: available even when panel hidden (status bar always shows it).
            Key::KeyT => {
                self.tonemap_mode = (self.tonemap_mode + 1) % 3;
                return KeyRoute::Consume;
            }
            // Next panel full is **menu operation**, only belongs to unlocked state. Locked = camera takes
            // mouse/keyboard, menu should receive no keys (previously only blocked WASD, arrows / Space / Enter still eaten,
            // locked pressing empty slot neither fires nor flips menu, pure key swallow).
            _ if self.locked => KeyRoute::Forward,
            // Unlocked: WASD is menu navigation. If panel manually hidden by F1, re-expand then navigate,
            // else keys act on invisible list.
            Key::KeyW | Key::KeyA | Key::KeyS | Key::KeyD => {
                self.show = true;
                self.navigate_with_wasd(key);
                KeyRoute::Consume
            }
            // Panel closed then next panel meaningless: pass back to App as-is.
            _ if !self.show => KeyRoute::Forward,
            Key::ArrowUp => {
                self.selected_index = if self.selected_index == 0 {
                    NUM_ITEMS - 1
                } else {
                    self.selected_index - 1
                };
                return KeyRoute::Consume;
            }
            Key::ArrowDown => {
                self.selected_index = (self.selected_index + 1) % NUM_ITEMS;
                return KeyRoute::Consume;
            }
            Key::ArrowLeft => {
                self.adjust_parameter(false);
                return KeyRoute::Consume;
            }
            Key::ArrowRight => {
                self.adjust_parameter(true);
                return KeyRoute::Consume;
            }
            Key::Space | Key::Enter => {
                self.toggle_parameter();
                return KeyRoute::Consume;
            }
            _ => KeyRoute::Forward,
        }
    }

    /// In unlocked state WASD same as arrow keys (panel is only place using keyboard).
    fn navigate_with_wasd(&mut self, key: Key) {
        match key {
            Key::KeyW => self.on_key_pressed(Key::ArrowUp),
            Key::KeyS => self.on_key_pressed(Key::ArrowDown),
            Key::KeyA => self.on_key_pressed(Key::ArrowLeft),
            Key::KeyD => self.on_key_pressed(Key::ArrowRight),
            _ => KeyRoute::Forward,
        };
    }
}

impl FrameHook for SponzaTuningHook {
    fn on_platform_event(&mut self, window: &PlatformWindow, event: PlatformEvent) -> bool {
        let PlatformEvent::Key { key, pressed } = event else {
            return false;
        };
        // Lock state ask window directly: `on_tick` cache is previous-frame, while "click screen to lock"
        // and "Esc unlock" happen between frames, using old value would misroute this key.
        self.locked = window.is_pointer_locked();
        if pressed && self.on_key_pressed(key) == KeyRoute::Consume {
            return true;
        }
        // false = not consumed: App runs shortcuts as usual and routes key into InputManager (under lock
        // WASD goes this path: panel ignores, camera moves normally). Note consumption does not block
        // InputManager — so "unlocked WASD does not move camera" enforced by engine-side pointer lock check.
        false
    }

    fn on_tick(
        &mut self,
        window: &PlatformWindow,
        world: &mut World,
        settings: &mut RenderSettings,
        stats: &RenderStats,
        _shared: &RenderShared,
    ) {
        // Pointer lock = camera takes over keyboard/mouse. Platform layer is only true source (App copy
        // `InputManager::pointer_locked` not reachable by hook), synced each frame for key routing.
        self.locked = window.is_pointer_locked();

        // Sync runtime settings to engine
        settings.render_mode = self.render_mode;
        settings.auto_exposure = self.auto_exposure;
        settings.raster_gi = self.raster_gi;
        settings.pt_gi = self.pt_gi;
        settings.pt_max_bounces = self.bounces;
        settings.pt_ray_max_distance = self.ray_max_distance;
        settings.tonemap_mode = self.tonemap_mode;
        // After lock cursor hidden, UI should not handle hover / click: otherwise when cursor clamped at
        // window center, pointer over a button would be "held", border glowing pressed during lock.
        if self.locked {
            world.remove_resource::<prism_engine::ui::UiInputState>();
        }
        // Pointer lock = full-screen view, panel hidden (F1 / Tab can bring back anytime).
        // Only hide on **just locked** frame: writing `show && !locked` every frame makes F1 useless
        // — on press hook sets show true, next frame on_tick wipes false, panel never opens.
        self.show = panel_visible_after_lock(self.show, self.locked, self.was_locked);
        self.was_locked = self.locked;
        settings.sharc_enable_clear = self.sharc_enable_clear;
        settings.sharc_enable_update = self.sharc_enable_update;
        settings.sharc_enable_resolve = self.sharc_enable_resolve;
        settings.sharc_enable_anti_firefly = self.sharc_enable_anti_firefly;
        settings.sharc_scene_scale = self.sharc_scene_scale;
        settings.sharc_capacity = self.sharc_capacity;
        settings.sharc_roughness_threshold = self.sharc_roughness_threshold;
        settings.sharc_downscale_factor = self.sharc_downscale_factor;
        settings.sharc_accumulation_frame_num = self.sharc_accumulation_frame_num;
        settings.sharc_stale_frame_num = self.sharc_stale_frame_num;
        settings.dlss_mode = self.dlss_mode;
        settings.dlss_pre_upscale = self.dlss_pre_upscale;
        settings.dlfg_mode = self.dlfg_mode;
        settings.dlfg_frames = self.dlfg_frames.clamp(1, 3);
        settings.dlss_rr_enabled = self.dlss_rr_enabled;

        if self.request_reset {
            self.request_reset = false;
            settings.pt_max_iterations = 0;
            info!("Sponza: path tracing accumulation reset requested");
        }

        if self.applied_custom_shader != Some(self.custom_shader) {
            let target = if self.custom_shader {
                SPONZA_CUSTOM_SHADER
            } else {
                ""
            };
            let entities: Vec<Entity> = world
                .query::<prism_scene::components::MeshRenderer>()
                .map(|(entity, _)| entity)
                .collect();
            for entity in entities {
                if let Some(mr) = world.get_mut::<prism_scene::components::MeshRenderer>(entity) {
                    if mr.shader_path != target {
                        mr.shader_path = target.to_string();
                    }
                }
            }
            self.applied_custom_shader = Some(self.custom_shader);
        }

        // Initialize UI entities once
        if self.canvas_entity.is_none() {
            let canvas = world.spawn();
            world.insert(canvas, Node::default());
            world.insert(
                canvas,
                Canvas {
                    reference_resolution: [1280.0, 720.0],
                    scale_mode: CanvasScaleMode::ScaleWithScreenSize,
                    match_width_or_height: 0.5,
                },
            );
            world.insert(canvas, Active(true));
            self.canvas_entity = Some(canvas);

            // Panel background quad
            let bg = world.spawn();
            world.insert(bg, Node::default());
            world.insert(
                bg,
                Style {
                    width: Some(450.0),
                    height: Some(520.0),
                    anchors: Anchors::TOP_LEFT,
                    pivot: Pivot::TOP_LEFT,
                    margin: Margin {
                        left: 12.0,
                        top: 12.0,
                        ..Margin::default()
                    },
                    background: [0.06, 0.08, 0.11, 0.92],
                    border_radius: 8.0,
                    visible: true,
                    ..Style::default()
                },
            );
            world.insert(bg, Active(true));
            self.panel_bg_entity = Some(bg);

            // Panel multiline text
            let ptext = world.spawn();
            world.insert(ptext, Node::default());
            world.insert(
                ptext,
                Style {
                    width: Some(430.0),
                    height: Some(500.0),
                    anchors: Anchors::TOP_LEFT,
                    pivot: Pivot::TOP_LEFT,
                    margin: Margin {
                        left: 22.0,
                        top: 20.0,
                        ..Margin::default()
                    },
                    visible: true,
                    ..Style::default()
                },
            );
            world.insert(
                ptext,
                Text {
                    content: String::new(),
                    font_size: 13.0,
                    color: [0.94, 0.96, 1.0, 1.0],
                    alignment: TextAlign::Left,
                    wrap: true,
                    ..Text::default()
                },
            );
            world.insert(ptext, Active(true));
            self.panel_text_entity = Some(ptext);

            // Top status banner (visible when panel is closed)
            let stext = world.spawn();
            world.insert(stext, Node::default());
            world.insert(
                stext,
                Style {
                    width: Some(800.0),
                    height: Some(28.0),
                    anchors: Anchors::TOP_LEFT,
                    pivot: Pivot::TOP_LEFT,
                    margin: Margin {
                        left: 14.0,
                        top: 10.0,
                        ..Margin::default()
                    },
                    visible: true,
                    ..Style::default()
                },
            );
            world.insert(
                stext,
                Text {
                    content: String::new(),
                    font_size: 13.0,
                    color: [0.72, 0.88, 1.0, 0.95],
                    alignment: TextAlign::Left,
                    wrap: false,
                    ..Text::default()
                },
            );
            world.insert(stext, Active(true));
            self.status_text_entity = Some(stext);
        }

        // Format and update text
        let fps = stats.fps;
        let ms = stats.frame_time_ms;
        let samples = stats.pt_frame_count.unwrap_or(0);
        let gpu_str = match &stats.gpu_timings {
            Some(g) if g.supported => format!("{:.2} ms", g.total_ms),
            _ => "n/a".to_string(),
        };

        if self.show {
            if let Some(bg) = self.panel_bg_entity {
                world.set_active(bg, true);
            }
            if let Some(stext) = self.status_text_entity {
                world.set_active(stext, false);
            }
            if let Some(ptext) = self.panel_text_entity {
                world.set_active(ptext, true);

                let marker = |idx: usize| {
                    if idx == self.selected_index {
                        "> "
                    } else {
                        "  "
                    }
                };

                let gi_str = match self.render_mode {
                    RenderMode::Raster => match self.raster_gi {
                        RasterGiMode::BakedProbe => "Baked Probe (raster)",
                        RasterGiMode::Restir => "ReSTIR GI (raster)",
                        RasterGiMode::Prt => "PRT64 (raster)",
                        RasterGiMode::Ddgi => "DDGI (raster RTXGI)",
                    },
                    RenderMode::PathTrace => match self.pt_gi {
                        PtGiMode::Off => "Reference (PT)",
                        PtGiMode::Sharc => "SHaRC (PT cache)",
                        PtGiMode::Nrc => "NRC (Neural)",
                    },
                    RenderMode::Sprites2D => "Off (2D)",
                };

                let tonemap_str = match self.tonemap_mode {
                    0 => "Reinhard",
                    1 => "ACES",
                    _ => "Passthrough",
                };

                let dlss_str = match self.dlss_mode {
                    DlssMode::Off => "Off",
                    DlssMode::Quality => "Quality",
                    DlssMode::Balanced => "Balanced",
                    DlssMode::Performance => "Performance",
                    DlssMode::UltraPerformance => "Ultra Performance",
                    DlssMode::Dlaa => "DLAA",
                };

                let dlfg_str = match self.dlfg_mode {
                    DlssgMode::Off => "Off",
                    DlssgMode::On => "On",
                    DlssgMode::Auto => "Auto",
                };

                let mode_str = match self.render_mode {
                    RenderMode::PathTrace => "Path Trace",
                    RenderMode::Raster => "Raster",
                    _ => "Sprites 2D",
                };

                let on_off = |v: bool| if v { "ON" } else { "OFF" };

                let content = format!(
                    "=== Sponza SHaRC / PT Tuning ===\n\
                     Frame: {:.2} ms ({:.0} FPS) | GPU: {}\n\
                     Accum Samples: {} | [F1] Hide | [R] Reset\n\
                     ----------------------------------------\n\
                     {}Render Mode:        [ {} ] (Key: M)\n\
                     {}GI Mode:            [ {} ] (Keys: 1, 2, 3)\n\
                     {}Max Bounces:        [ {} ]\n\
                     {}Ray Max Dist:       [ {:.0} ]\n\
                     {}Tonemapper:         [ {} ] (Key: T)\n\
                     --- SHaRC Options ----------------------\n\
                     {}Enable Clear:       [ {} ]\n\
                     {}Enable Update:      [ {} ]\n\
                     {}Enable Resolve:     [ {} ]\n\
                     {}Anti-Firefly:       [ {} ]\n\
                     {}Scene Scale:        [ {:.1} ]\n\
                     {}Roughness Thresh:   [ {:.2} ]\n\
                     {}Downscale Factor:   [ {} ]\n\
                     {}Accum Frame Num:    [ {} ]\n\
                     {}Stale Frame Num:    [ {} ]\n\
                     {}Cache Capacity:     [ {}K ]\n\
                     --- Post / Upscaling -------------------\n\
                     {}DLSS Mode:          [ {} ]\n\
                     {}Auto Exposure:      [ {} ]\n\
                     {}Custom Shader:      [ {} ] (Raster)\n\
                     {}DLFG (Frame Gen):   [ {} ]\n\
                     {}DLSS Ray Recon:     [ {} ]\n\
                     {}DLSS Pre-Upscale:   [ {} ]\n\
                     {}DLFG Multiplier:    [ {}x ({}) ]\n\
                     ----------------------------------------\n\
                     [Up/Down] Select   [Left/Right] Adjust\n\
                     [Space/Enter] Toggle   [V] VSync: {}",
                    ms,
                    fps,
                    gpu_str,
                    samples,
                    marker(0),
                    mode_str,
                    marker(1),
                    gi_str,
                    marker(2),
                    self.bounces,
                    marker(3),
                    self.ray_max_distance,
                    marker(4),
                    tonemap_str,
                    marker(5),
                    on_off(self.sharc_enable_clear),
                    marker(6),
                    on_off(self.sharc_enable_update),
                    marker(7),
                    on_off(self.sharc_enable_resolve),
                    marker(8),
                    on_off(self.sharc_enable_anti_firefly),
                    marker(9),
                    self.sharc_scene_scale,
                    marker(10),
                    self.sharc_roughness_threshold,
                    marker(11),
                    self.sharc_downscale_factor,
                    marker(12),
                    self.sharc_accumulation_frame_num,
                    marker(13),
                    self.sharc_stale_frame_num,
                    marker(14),
                    self.sharc_capacity / 1024,
                    marker(15),
                    dlss_str,
                    marker(16),
                    on_off(self.auto_exposure),
                    marker(17),
                    on_off(self.custom_shader),
                    marker(18),
                    dlfg_str,
                    marker(19),
                    on_off(self.dlss_rr_enabled),
                    marker(20),
                    self.dlss_pre_upscale.label(),
                    marker(21),
                    self.dlfg_frames.clamp(1, 3) + 1,
                    self.dlfg_frames.clamp(1, 3),
                    on_off(self.vsync),
                );

                if let Some(t) = world.get_mut::<Text>(ptext) {
                    t.content = content;
                }
            }
        } else {
            if let Some(bg) = self.panel_bg_entity {
                world.set_active(bg, false);
            }
            if let Some(ptext) = self.panel_text_entity {
                world.set_active(ptext, false);
            }
            if let Some(stext) = self.status_text_entity {
                world.set_active(stext, true);

                let gi_short = match self.render_mode {
                    RenderMode::Raster => match self.raster_gi {
                        RasterGiMode::BakedProbe => "BakedProbe",
                        RasterGiMode::Restir => "ReSTIR",
                        RasterGiMode::Prt => "PRT64",
                        RasterGiMode::Ddgi => "DDGI",
                    },
                    RenderMode::PathTrace => match self.pt_gi {
                        PtGiMode::Off => "Reference",
                        PtGiMode::Sharc => "SHaRC",
                        PtGiMode::Nrc => "NRC",
                    },
                    RenderMode::Sprites2D => "Off",
                };
                let mode_short = match self.render_mode {
                    RenderMode::PathTrace => "PT",
                    RenderMode::Raster => "Raster",
                    _ => "Sprites2D",
                };
                let status = format!(
                    "[F1/Tab] Tuning Panel | Mode: {mode_short} [M] | GI: {gi_short} | Bounces: {} | {:.0} FPS ({:.1} ms) | Samples: {} | [1] Ref [2] SHaRC [3] NRC [4] ReSTIR",
                    self.bounces, fps, ms, samples
                );
                if let Some(t) = world.get_mut::<Text>(stext) {
                    t.content = status;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{panel_visible_after_lock, KeyRoute, SponzaTuningHook};
    use prism_platform::Key;

    /// Panel toggle must be swallowed by hook, otherwise App Tab (debug target switch) fires too.
    #[test]
    fn f1_and_tab_toggle_the_panel_and_are_consumed() {
        let mut hook = SponzaTuningHook::default();
        let before = hook.show;
        assert_eq!(hook.on_key_pressed(Key::F1), KeyRoute::Consume);
        assert_ne!(hook.show, before, "F1 must really toggle panel");
        assert_eq!(hook.on_key_pressed(Key::Tab), KeyRoute::Consume);
        assert_eq!(hook.show, before, "Tab equals F1, press again toggles back");
    }

    /// Locked state: WASD passes to camera as-is, panel must not touch anything.
    #[test]
    fn wasd_goes_to_the_camera_while_locked() {
        let mut hook = SponzaTuningHook {
            locked: true,
            ..SponzaTuningHook::default()
        };
        let before = hook.selected_index;
        for key in [Key::KeyW, Key::KeyA, Key::KeyS, Key::KeyD] {
            assert_eq!(hook.on_key_pressed(key), KeyRoute::Forward, "{key:?}");
        }
        assert_eq!(
            hook.selected_index, before,
            "WASD must not flip menu in locked state"
        );
    }

    /// Locked state: no menu key may be swallowed — panel hidden, arrows / Space must pass to game as-is.
    #[test]
    fn menu_keys_are_forwarded_while_locked() {
        let mut hook = SponzaTuningHook {
            locked: true,
            show: true,
            ..SponzaTuningHook::default()
        };
        let before = hook.selected_index;
        for key in [
            Key::ArrowUp,
            Key::ArrowDown,
            Key::ArrowLeft,
            Key::ArrowRight,
            Key::Space,
            Key::Enter,
        ] {
            assert_eq!(hook.on_key_pressed(key), KeyRoute::Forward, "{key:?}");
        }
        assert_eq!(
            hook.selected_index, before,
            "Arrow keys must not flip menu in locked state"
        );
    }

    /// Unlocked: WASD only flips menu and must be consumed — else camera moves too.
    #[test]
    fn wasd_drives_the_menu_and_is_consumed_while_unlocked() {
        let mut hook = SponzaTuningHook {
            locked: false,
            ..SponzaTuningHook::default()
        };
        let before = hook.selected_index;
        assert_eq!(hook.on_key_pressed(Key::KeyS), KeyRoute::Consume);
        assert_ne!(
            hook.selected_index, before,
            "S should move selection down in unlocked state"
        );
    }

    /// Lock state change: panel immediately aligns to target state; intermediate frames preserve F1 / Tab toggles.
    #[test]
    fn panel_follows_the_lock_state() {
        assert!(
            !panel_visible_after_lock(true, true, false),
            "Hide immediately on lock"
        );
        assert!(
            panel_visible_after_lock(false, false, true),
            "Expand immediately on unlock"
        );
        assert!(
            panel_visible_after_lock(true, true, true),
            "After F1 opens in locked state must stay open"
        );
        assert!(
            !panel_visible_after_lock(false, true, true),
            "Preserve hidden in locked state"
        );
        assert!(
            panel_visible_after_lock(true, false, false),
            "Preserve expanded in unlocked state"
        );
    }
}
