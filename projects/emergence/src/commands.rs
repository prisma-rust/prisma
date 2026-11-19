//! Horizon-aligned chat & slash-command console (`T` / `Enter` / `/`) for `emergence`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

static UPSCALER_MODE: AtomicU8 = AtomicU8::new(0);
static UPSCALER_SCALE_BITS: AtomicU32 = AtomicU32::new(0.75_f32.to_bits());
static UPSCALER_SHARPNESS_BITS: AtomicU32 = AtomicU32::new(0.5_f32.to_bits());
static DLFG_MODE: AtomicU8 = AtomicU8::new(0);
static DDGI_ENABLED: AtomicBool = AtomicBool::new(false);

/// Returns `(dlfg_mode_id, ddgi_enabled)` for runtime RTXGI / Streamline controls.
pub fn current_rtx_toggles() -> (u8, bool) {
    (
        DLFG_MODE.load(Ordering::Relaxed),
        DDGI_ENABLED.load(Ordering::Relaxed),
    )
}

/// Toggle or set DLFG (`0=Off, 1=On, 2=Auto`) and DDGI from runtime UI or commands.
pub fn set_rtx_toggles(dlfg_mode: u8, ddgi_enabled: bool) {
    DLFG_MODE.store(dlfg_mode.min(2), Ordering::Relaxed);
    DDGI_ENABLED.store(ddgi_enabled, Ordering::Relaxed);
}

/// Returns `(mode_id, render_scale, sharpness)` where `mode_id` is:
/// `0 = Off`, `1 = Sgsr2`, `2 = ArmAsr`, `3 = Nearest`, `4 = Linear`,
/// `5 = Dlss (PC placeholder)`, `6 = Fsr (PC placeholder)`.
pub fn current_upscaler_config() -> (u8, f32, f32) {
    (
        UPSCALER_MODE.load(Ordering::Relaxed),
        f32::from_bits(UPSCALER_SCALE_BITS.load(Ordering::Relaxed)),
        f32::from_bits(UPSCALER_SHARPNESS_BITS.load(Ordering::Relaxed)),
    )
}

/// Update upscaler state from the in-game Render Settings UI.
pub fn set_upscaler_config(mode_id: u8, scale: f32, sharpness: f32) {
    UPSCALER_MODE.store(mode_id.min(6), Ordering::Relaxed);
    UPSCALER_SCALE_BITS.store(scale.clamp(0.35, 1.0).to_bits(), Ordering::Relaxed);
    UPSCALER_SHARPNESS_BITS.store(sharpness.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
}

/// Human-readable upscaler mode name for HUD and command feedback.
pub fn upscaler_mode_label(mode_id: u8) -> &'static str {
    match mode_id {
        1 => "SGSR 2",
        2 => "Arm ASR",
        3 => "Nearest",
        4 => "Linear",
        5 => "DLSS (Streamline)",
        6 => "FSR (Placeholder -> Off)",
        _ => "Off",
    }
}

/// Optional startup override via `EMERGENCE_UPSCALER=off|nearest|linear|sgsr2|asr|dlss|fsr`,
/// `EMERGENCE_UPSCALER_SCALE=0.75`, and `EMERGENCE_UPSCALER_SHARPNESS=0.5`.
pub fn init_upscaler_from_env() {
    if let Ok(mode) = std::env::var("EMERGENCE_UPSCALER") {
        let id = parse_upscaler_mode_id(&mode).unwrap_or(0);
        UPSCALER_MODE.store(id, Ordering::Relaxed);
    }
    if let Ok(scale) = std::env::var("EMERGENCE_UPSCALER_SCALE") {
        if let Ok(v) = scale.trim().parse::<f32>() {
            UPSCALER_SCALE_BITS.store(v.clamp(0.35, 1.0).to_bits(), Ordering::Relaxed);
        }
    }
    if let Ok(sharp) = std::env::var("EMERGENCE_UPSCALER_SHARPNESS") {
        if let Ok(v) = sharp.trim().parse::<f32>() {
            UPSCALER_SHARPNESS_BITS.store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
    }
}

fn parse_upscaler_mode_id(raw: &str) -> Option<u8> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" | "none" | "0" => Some(0),
        "sgsr2" | "sgsr" => Some(1),
        "asr" | "arm_asr" | "arm-asr" | "armasr" => Some(2),
        "nearest" | "point" => Some(3),
        "linear" | "bilinear" => Some(4),
        #[cfg(not(target_os = "android"))]
        "dlss" => Some(5),
        #[cfg(not(target_os = "android"))]
        "fsr" => Some(6),
        _ => None,
    }
}

use glam::Vec3;
use prism_game_api::{keys, EntityHandle, GameWorld, InputSnapshot};
use prism_scene::ui::{
    Anchors, Canvas, CanvasScaleMode, Margin, Node, Pivot, Style, Text, TextAlign,
};
use prism_scene::{Active, Camera, DirectionalLight, Name};

use crate::debug_hud::estimate_text_width;
use crate::materials::mat_id;
use crate::player::PlayerController;
use crate::sim::SimWorld;
use crate::tags::StimulusFlags;

/// Minecraft 24,000-tick day cycle.
pub const TICKS_PER_DAY: u64 = 24_000;

/// Active weather state affecting sky lighting and world-surface stimuli.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherKind {
    Clear,
    Rain,
    Thunder,
}

impl WeatherKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Clear => "clear",
            Self::Rain => "rain",
            Self::Thunder => "thunder",
        }
    }
}

/// Configurable Minecraft-style game rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameRules {
    pub do_daylight_cycle: bool,
    pub do_weather_cycle: bool,
    pub do_fire_tick: bool,
}

impl Default for GameRules {
    fn default() -> Self {
        Self {
            do_daylight_cycle: true,
            do_weather_cycle: true,
            do_fire_tick: true,
        }
    }
}

/// Persistent world time, weather, game rules, and in-game command console state.
/// Maximum messages retained in the Horizon-style chat scrollback.
const MAX_CHAT_HISTORY: usize = 100;
const CHAT_FADE_LIFETIME_SEC: f32 = 10.0;
const CHAT_FADE_TAIL_SEC: f32 = 1.0;
const MAX_VISIBLE_UNFOCUSED: usize = 10;
const MAX_VISIBLE_OPEN: usize = 14;
const CHAT_FONT_SIZE: f32 = 14.0;
const CHAT_ROW_HEIGHT: f32 = 19.0;
const INPUT_BAR_HEIGHT: f32 = 26.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatTag {
    Player,
    System,
    Command,
    Warning,
    Error,
}

impl ChatTag {
    const fn accent_color(self) -> Option<[f32; 4]> {
        match self {
            Self::Player => None,
            Self::System => Some([0.33, 1.0, 1.0, 1.0]),
            Self::Command => Some([0.33, 1.0, 0.33, 1.0]),
            Self::Warning => Some([1.0, 0.67, 0.0, 1.0]),
            Self::Error => Some([1.0, 0.33, 0.33, 1.0]),
        }
    }

    const fn text_color(self) -> [f32; 4] {
        match self {
            Self::Player => [0.878, 0.878, 0.878, 1.0],
            Self::System | Self::Command => [0.90, 0.96, 0.98, 1.0],
            Self::Warning => [1.0, 0.84, 0.38, 1.0],
            Self::Error => [1.0, 0.42, 0.42, 1.0],
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub text: String,
    pub tag: ChatTag,
    pub age_sec: f32,
}

#[derive(Debug, Clone, Copy)]
struct ChatRowEntities {
    bg: EntityHandle,
    accent: EntityHandle,
    text: EntityHandle,
}

#[derive(Debug, Default)]
struct ChatUiEntities {
    initialized: bool,
    rows: Vec<ChatRowEntities>,
    suggest_bg: Option<EntityHandle>,
    suggest_text: Option<EntityHandle>,
    input_bg: Option<EntityHandle>,
    input_text: Option<EntityHandle>,
}

const CHAT_CANVAS: Canvas = Canvas {
    reference_resolution: [1280.0, 720.0],
    scale_mode: CanvasScaleMode::ConstantPixelSize,
    match_width_or_height: 0.5,
};

pub struct CommandWorldState {
    pub time_ticks: u64,
    pub weather: WeatherKind,
    pub weather_timer_sec: f32,
    pub rules: GameRules,
    pub console_open: bool,
    pub input_line: String,
    pub cursor_char: usize,
    pub history: Vec<String>,
    pub history_cursor: Option<usize>,
    pub feedback: String,
    pub feedback_timer_sec: f32,
    pub chat_log: VecDeque<ChatMessage>,
    draft_line: String,
    scroll_offset: usize,
    caret_blink_sec: f32,
    chat_ui: ChatUiEntities,
    time_accum_sec: f32,
    weather_stim_accum_sec: f32,
}

impl Default for CommandWorldState {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandWorldState {
    pub fn new() -> Self {
        Self {
            time_ticks: 6_000, // Noon by default
            weather: WeatherKind::Clear,
            weather_timer_sec: 0.0,
            rules: GameRules::default(),
            console_open: false,
            input_line: String::new(),
            cursor_char: 0,
            history: Vec::new(),
            history_cursor: None,
            feedback: String::new(),
            feedback_timer_sec: 0.0,
            chat_log: VecDeque::new(),
            draft_line: String::new(),
            scroll_offset: 0,
            caret_blink_sec: 0.0,
            chat_ui: ChatUiEntities::default(),
            time_accum_sec: 0.0,
            weather_stim_accum_sec: 0.0,
        }
    }

    /// Returns a human-readable time-of-day phase label for the F3 overlay.
    pub fn time_phase_label(&self) -> &'static str {
        match self.time_ticks % TICKS_PER_DAY {
            0..=5_999 => "Morning",
            6_000..=11_999 => "Day",
            12_000..=13_499 => "Sunset",
            13_500..=22_499 => "Night",
            _ => "Dawn",
        }
    }

    /// Returns the active gamemode label aligned with PlayerController fly state.
    pub fn gamemode_label(&self, player: &PlayerController) -> &'static str {
        if player.fly_mode {
            "creative"
        } else {
            "survival"
        }
    }

    /// Appends one or more feedback/chat lines to the Horizon-style scrollback.
    pub fn push_chat(&mut self, text: &str, tag: ChatTag) {
        for raw_line in text.lines() {
            let line = raw_line.trim_end();
            if line.is_empty() {
                continue;
            }
            if self.chat_log.len() >= MAX_CHAT_HISTORY {
                self.chat_log.pop_back();
            }
            self.chat_log.push_front(ChatMessage {
                text: line.to_string(),
                tag,
                age_sec: 0.0,
            });
        }
    }

    fn open_console(&mut self, initial: &str) {
        self.console_open = true;
        self.input_line.clear();
        self.input_line.push_str(initial);
        self.cursor_char = self.input_line.chars().count();
        self.history_cursor = None;
        self.draft_line.clear();
        self.scroll_offset = 0;
        self.caret_blink_sec = 0.0;
    }

    fn close_console(&mut self) {
        self.console_open = false;
        self.input_line.clear();
        self.cursor_char = 0;
        self.history_cursor = None;
        self.draft_line.clear();
        self.scroll_offset = 0;
    }

    /// Advance daylight cycle, weather timers, and apply sky/sun changes to ECS.
    pub fn step(
        &mut self,
        dt: f32,
        world: &mut GameWorld<'_>,
        sim: &mut SimWorld,
        player_pos: Vec3,
    ) {
        if self.feedback_timer_sec > 0.0 {
            self.feedback_timer_sec = (self.feedback_timer_sec - dt).max(0.0);
        }

        if self.rules.do_daylight_cycle {
            // 20 ticks/sec = 20-minute full Minecraft day cycle.
            self.time_accum_sec += dt * 20.0;
            let whole = self.time_accum_sec.floor() as u64;
            if whole > 0 {
                self.time_accum_sec -= whole as f32;
                self.time_ticks = (self.time_ticks + whole) % TICKS_PER_DAY;
            }
        }

        if self.weather != WeatherKind::Clear
            && self.rules.do_weather_cycle
            && self.weather_timer_sec > 0.0
        {
            self.weather_timer_sec -= dt;
            if self.weather_timer_sec <= 0.0 {
                self.weather = WeatherKind::Clear;
                self.weather_timer_sec = 0.0;
            }
        }

        // Apply active weather stimuli (rain wets exposed blocks; thunder occasionally strikes).
        if self.weather != WeatherKind::Clear {
            self.weather_stim_accum_sec += dt;
            if self.weather_stim_accum_sec >= 0.25 {
                self.weather_stim_accum_sec = 0.0;
                let base_x = (player_pos.x * 4.0) as i32;
                let base_z = (player_pos.z * 4.0) as i32;
                let h = (self.time_ticks.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (base_x as u64))
                    .wrapping_add(base_z as u64);
                let dx = ((h % 33) as i32) - 16;
                let dz = (((h >> 8) % 33) as i32) - 16;
                let wx = (base_x + dx) as f32 * 0.25 + 0.125;
                let wz = (base_z + dz) as f32 * 0.25 + 0.125;
                if let Some(hit) = sim.raycast([wx, 48.0, wz], [0.0, -1.0, 0.0], 48.0, false) {
                    sim.inject_stimulus(hit.world_l2, StimulusFlags::NONE, true, 0);
                    if self.weather == WeatherKind::Thunder && (h >> 16).is_multiple_of(16) {
                        sim.inject_stimulus(hit.world_l2, StimulusFlags::SHOCK, true, 12);
                    }
                }
            }
        }

        self.sync_sky_and_sun(world);
    }

    fn sync_sky_and_sun(&self, world: &mut GameWorld<'_>) {
        // 0 = sunrise (06:00), 6000 = noon, 12000 = sunset, 18000 = midnight.
        let phase = (self.time_ticks % TICKS_PER_DAY) as f32 / TICKS_PER_DAY as f32;
        let sun_angle = phase * std::f32::consts::TAU; // 0 = east horizon, PI/2 = zenith, PI = west horizon
        let tilt = 22.0_f32.to_radians();
        let sun_vec = [
            sun_angle.cos(),
            sun_angle.sin() * tilt.cos(),
            sun_angle.sin() * tilt.sin(),
        ];
        let sun_len =
            (sun_vec[0] * sun_vec[0] + sun_vec[1] * sun_vec[1] + sun_vec[2] * sun_vec[2]).sqrt();
        let sun_dir = [
            sun_vec[0] / sun_len,
            sun_vec[1] / sun_len,
            sun_vec[2] / sun_len,
        ];
        let elevation = sun_dir[1];
        let is_night = elevation < -0.04;
        let day_factor = elevation.clamp(0.0, 1.0);
        let twilight = (1.0 - (elevation.abs() * 4.5).clamp(0.0, 1.0)).powf(1.5);

        let weather_dim = match self.weather {
            WeatherKind::Clear => 1.0,
            WeatherKind::Rain => 0.45,
            WeatherKind::Thunder => 0.22,
        };

        // Primary directional light follows the sun by day and switches to the
        // moon above the horizon at night so shadows + GI stay active 24h.
        let mut active_dir = if is_night {
            [-sun_dir[0], (-sun_dir[1]).max(0.10), -sun_dir[2]]
        } else {
            [sun_dir[0], sun_dir[1].max(0.08), sun_dir[2]]
        };
        let active_len = (active_dir[0] * active_dir[0]
            + active_dir[1] * active_dir[1]
            + active_dir[2] * active_dir[2])
            .sqrt();
        active_dir[0] /= active_len;
        active_dir[1] /= active_len;
        active_dir[2] /= active_len;

        // Exact inverse of `prism_engine::euler_xyz_deg_to_dir`:
        // +X pitch lifts light vector toward +Y (above horizon).
        let pitch_deg = active_dir[1].clamp(-0.99, 0.99).asin().to_degrees();
        let yaw_deg = active_dir[0].atan2(active_dir[2]).to_degrees();

        let (sun_color, intensity, ambient) = if is_night {
            let moon_elev = (-elevation).clamp(0.0, 1.0);
            let moon_col = [0.48 + 0.08 * moon_elev, 0.62 + 0.08 * moon_elev, 0.96];
            let moon_int = (9_000.0 + 7_000.0 * moon_elev) * weather_dim;
            let moon_amb = (0.12 + 0.05 * moon_elev) * (0.70 + 0.30 * weather_dim);
            (moon_col, moon_int, moon_amb)
        } else {
            let day_col = [1.0, 0.97 - twilight * 0.38, 0.92 - twilight * 0.62];
            let day_int = (18_000.0 + 76_000.0 * day_factor.powf(0.65)) * weather_dim;
            let day_amb = (0.28 + 0.30 * day_factor) * (0.65 + 0.35 * weather_dim);
            (day_col, day_int, day_amb)
        };

        for sun_ent in world.query::<DirectionalLight>() {
            if let Some(mut light) = world.get::<DirectionalLight>(sun_ent) {
                light.euler_xyz = [pitch_deg, yaw_deg, 0.0].into();
                light.color = sun_color.into();
                light.intensity = intensity;
                light.ambient = ambient;
                world.set(sun_ent, light);
            }
        }

        let sky = match self.weather {
            WeatherKind::Clear => [
                (0.02 + 0.16 * day_factor + 0.34 * twilight).min(1.0),
                (0.04 + 0.32 * day_factor + 0.16 * twilight).min(1.0),
                (0.09 + 0.52 * day_factor + 0.06 * twilight).min(1.0),
                1.0,
            ],
            WeatherKind::Rain => [
                0.11 * (0.18 + 0.82 * day_factor),
                0.15 * (0.18 + 0.82 * day_factor),
                0.21 * (0.18 + 0.82 * day_factor),
                1.0,
            ],
            WeatherKind::Thunder => [
                0.04 * (0.18 + 0.82 * day_factor),
                0.06 * (0.18 + 0.82 * day_factor),
                0.10 * (0.18 + 0.82 * day_factor),
                1.0,
            ],
        };
        for cam_ent in world.query::<Camera>() {
            if let Some(mut cam) = world.get::<Camera>(cam_ent) {
                cam.background_color = sky;
                world.set(cam_ent, cam);
            }
        }
    }

    /// Poll keyboard input to open/close/edit/submit the `/` command prompt.
    /// Returns `true` when the console owns keyboard focus this frame.
    pub fn handle_console_input(
        &mut self,
        input: &InputSnapshot,
        world: &mut GameWorld<'_>,
        sim: &mut SimWorld,
        player: &mut PlayerController,
    ) -> bool {
        if !self.console_open {
            if input.key_pressed(keys::KEY_T) || input.key_pressed(keys::ENTER) {
                self.open_console("");
                return true;
            }
            if input.key_pressed(keys::OTHER_BASE + 0xBF)
                || input.key_pressed(keys::OTHER_BASE + 0x6F)
            {
                self.open_console("/");
                return true;
            }
            return false;
        }

        if input.key_pressed(keys::ESCAPE) {
            self.close_console();
            return true;
        }

        if input.key_pressed(keys::ENTER) {
            let raw = self.input_line.trim().to_string();
            self.close_console();
            if !raw.is_empty() {
                if self.history.last() != Some(&raw) {
                    if self.history.len() >= MAX_CHAT_HISTORY {
                        self.history.remove(0);
                    }
                    self.history.push(raw.clone());
                }
                self.submit_line(&raw, world, sim, player);
            }
            return true;
        }

        if input.key_pressed(keys::ARROW_UP) && !self.history.is_empty() {
            if self.history_cursor.is_none() {
                self.draft_line = self.input_line.clone();
            }
            let next = match self.history_cursor {
                None => self.history.len() - 1,
                Some(i) => i.saturating_sub(1),
            };
            self.history_cursor = Some(next);
            self.input_line = self.history[next].clone();
            self.cursor_char = self.input_line.chars().count();
            return true;
        }
        if input.key_pressed(keys::ARROW_DOWN) {
            if let Some(i) = self.history_cursor {
                if i + 1 < self.history.len() {
                    self.history_cursor = Some(i + 1);
                    self.input_line = self.history[i + 1].clone();
                } else {
                    self.history_cursor = None;
                    self.input_line = std::mem::take(&mut self.draft_line);
                }
                self.cursor_char = self.input_line.chars().count();
            }
            return true;
        }

        let char_len = self.input_line.chars().count();
        self.cursor_char = self.cursor_char.min(char_len);

        if input.key_pressed(keys::ARROW_LEFT) {
            self.cursor_char = self.cursor_char.saturating_sub(1);
            self.caret_blink_sec = 0.0;
            return true;
        }
        if input.key_pressed(keys::ARROW_RIGHT) {
            self.cursor_char = (self.cursor_char + 1).min(char_len);
            self.caret_blink_sec = 0.0;
            return true;
        }
        if input.key_pressed(keys::OTHER_BASE + 0x24) {
            // Home
            self.cursor_char = 0;
            self.caret_blink_sec = 0.0;
            return true;
        }
        if input.key_pressed(keys::OTHER_BASE + 0x23) {
            // End
            self.cursor_char = char_len;
            self.caret_blink_sec = 0.0;
            return true;
        }
        if input.key_pressed(keys::OTHER_BASE + 0x21) {
            // PageUp
            let max_scroll = self.chat_log.len().saturating_sub(1);
            self.scroll_offset = (self.scroll_offset + 5).min(max_scroll);
            return true;
        }
        if input.key_pressed(keys::OTHER_BASE + 0x22) {
            // PageDown
            self.scroll_offset = self.scroll_offset.saturating_sub(5);
            return true;
        }
        if input.key_pressed(keys::TAB) {
            self.autocomplete();
            self.caret_blink_sec = 0.0;
            return true;
        }

        // Backspace (VK_BACK = 0x08) and Delete (VK_DELETE = 0x2E).
        if input.key_pressed(keys::OTHER_BASE + 0x08) {
            self.delete_before_cursor();
            self.caret_blink_sec = 0.0;
            return true;
        }
        if input.key_pressed(keys::OTHER_BASE + 0x2E) {
            self.delete_at_cursor();
            self.caret_blink_sec = 0.0;
            return true;
        }

        if let Some(ch) = poll_typed_char(input) {
            self.insert_char_at_cursor(ch);
            self.history_cursor = None;
            self.caret_blink_sec = 0.0;
        }
        true
    }

    fn submit_line(
        &mut self,
        raw: &str,
        world: &mut GameWorld<'_>,
        sim: &mut SimWorld,
        player: &mut PlayerController,
    ) {
        let is_slash = raw.starts_with('/');
        let first_word = raw
            .strip_prefix('/')
            .unwrap_or(raw)
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if is_slash || is_known_command(&first_word) {
            let msg = self.execute(raw, world, sim, player);
            let tag = classify_feedback_tag(&msg);
            self.push_chat(&msg, tag);
            self.feedback = msg;
            self.feedback_timer_sec = 5.0;
        } else {
            let line = format!("<Player> {raw}");
            self.push_chat(&line, ChatTag::Player);
            self.feedback = line;
            self.feedback_timer_sec = 5.0;
        }
    }

    /// Advances chat message fade timers and syncs the bottom-left Horizon chat UI.
    pub fn sync_chat_ui(&mut self, world: &mut GameWorld<'_>, dt: f32) {
        for msg in &mut self.chat_log {
            msg.age_sec += dt.clamp(0.0, 0.25);
        }
        if self.console_open {
            self.caret_blink_sec = (self.caret_blink_sec + dt.max(0.0)) % 1.0;
        }

        self.ensure_chat_ui(world);
        if !self.chat_ui.initialized {
            return;
        }

        let screen = world.screen_size();
        let screen_w = if screen[0] > 1.0 { screen[0] } else { 1280.0 };
        let screen_h = if screen[1] > 1.0 { screen[1] } else { 720.0 };
        let box_w = (screen_w * 0.45).clamp(360.0, 620.0);

        // Gather visible messages newest-at-bottom, oldest-above.
        let mut visible: Vec<(&ChatMessage, f32)> = Vec::new();
        if self.console_open {
            for msg in self
                .chat_log
                .iter()
                .skip(self.scroll_offset)
                .take(MAX_VISIBLE_OPEN)
            {
                visible.push((msg, 1.0));
            }
        } else {
            for msg in self.chat_log.iter().take(MAX_VISIBLE_UNFOCUSED) {
                if msg.age_sec >= CHAT_FADE_LIFETIME_SEC {
                    continue;
                }
                let remaining = CHAT_FADE_LIFETIME_SEC - msg.age_sec;
                let fade = (remaining / CHAT_FADE_TAIL_SEC).clamp(0.0, 1.0);
                visible.push((msg, fade * fade));
            }
        }

        let base_bottom_y = screen_h - 42.0;
        for (slot_idx, row) in self.chat_ui.rows.iter().enumerate() {
            let Some(&(msg, alpha)) = visible.get(slot_idx) else {
                world.set(row.bg, Active(false));
                world.set(row.accent, Active(false));
                world.set(row.text, Active(false));
                continue;
            };
            let y = (base_bottom_y - ((slot_idx + 1) as f32) * CHAT_ROW_HEIGHT).max(4.0);
            world.set(
                row.bg,
                Style {
                    width: Some(box_w),
                    height: Some(CHAT_ROW_HEIGHT),
                    anchors: Anchors::TOP_LEFT,
                    pivot: Pivot::TOP_LEFT,
                    margin: Margin {
                        left: 4.0,
                        top: y,
                        ..Margin::default()
                    },
                    background: [0.0, 0.0, 0.0, 0.50 * alpha],
                    ..Style::default()
                },
            );
            world.set(row.bg, Active(true));

            if let Some(mut accent_col) = msg.tag.accent_color() {
                accent_col[3] *= alpha;
                world.set(
                    row.accent,
                    Style {
                        width: Some(2.0),
                        height: Some(CHAT_ROW_HEIGHT),
                        anchors: Anchors::TOP_LEFT,
                        pivot: Pivot::TOP_LEFT,
                        margin: Margin {
                            left: 4.0,
                            top: y,
                            ..Margin::default()
                        },
                        background: accent_col,
                        ..Style::default()
                    },
                );
                world.set(row.accent, Active(true));
            } else {
                world.set(row.accent, Active(false));
            }

            let mut text_col = msg.tag.text_color();
            text_col[3] *= alpha;
            world.set(
                row.text,
                Style {
                    width: Some(box_w - 8.0),
                    height: Some(CHAT_ROW_HEIGHT),
                    anchors: Anchors::TOP_LEFT,
                    pivot: Pivot::TOP_LEFT,
                    margin: Margin {
                        left: 10.0,
                        top: y,
                        ..Margin::default()
                    },
                    ..Style::default()
                },
            );
            world.set(
                row.text,
                Text {
                    content: msg.text.clone(),
                    font_size: CHAT_FONT_SIZE,
                    color: text_col,
                    alignment: TextAlign::Left,
                    wrap: false,
                    ..Text::default()
                },
            );
            world.set(row.text, Active(true));
        }

        // Sync input bar and command suggestions when console is open.
        if let (Some(in_bg), Some(in_txt), Some(sug_bg), Some(sug_txt)) = (
            self.chat_ui.input_bg,
            self.chat_ui.input_text,
            self.chat_ui.suggest_bg,
            self.chat_ui.suggest_text,
        ) {
            if !self.console_open {
                world.set(in_bg, Active(false));
                world.set(in_txt, Active(false));
                world.set(sug_bg, Active(false));
                world.set(sug_txt, Active(false));
                return;
            }

            let bar_w = (screen_w - 8.0).max(200.0);
            let bar_y = (screen_h - INPUT_BAR_HEIGHT - 4.0).max(4.0);
            world.set(
                in_bg,
                Style {
                    width: Some(bar_w),
                    height: Some(INPUT_BAR_HEIGHT),
                    anchors: Anchors::TOP_LEFT,
                    pivot: Pivot::TOP_LEFT,
                    margin: Margin {
                        left: 4.0,
                        top: bar_y,
                        ..Margin::default()
                    },
                    background: [0.0, 0.0, 0.0, 0.70],
                    ..Style::default()
                },
            );
            world.set(in_bg, Active(true));

            let display_input = self.render_input_with_caret();
            world.set(
                in_txt,
                Style {
                    width: Some(bar_w - 12.0),
                    height: Some(INPUT_BAR_HEIGHT),
                    anchors: Anchors::TOP_LEFT,
                    pivot: Pivot::TOP_LEFT,
                    margin: Margin {
                        left: 10.0,
                        top: bar_y + 2.0,
                        ..Margin::default()
                    },
                    ..Style::default()
                },
            );
            world.set(
                in_txt,
                Text {
                    content: display_input,
                    font_size: 14.5,
                    color: [0.878, 0.878, 0.878, 1.0],
                    alignment: TextAlign::Left,
                    wrap: false,
                    ..Text::default()
                },
            );
            world.set(in_txt, Active(true));

            let suggestions = autocomplete_candidates(&self.input_line);
            if suggestions.is_empty() {
                world.set(sug_bg, Active(false));
                world.set(sug_txt, Active(false));
            } else {
                let joined = suggestions.join("  ");
                let sug_w = (estimate_text_width(&joined, CHAT_FONT_SIZE) + 12.0)
                    .min(bar_w)
                    .max(120.0);
                let sug_y = (bar_y - CHAT_ROW_HEIGHT - 2.0).max(4.0);
                world.set(
                    sug_bg,
                    Style {
                        width: Some(sug_w),
                        height: Some(CHAT_ROW_HEIGHT),
                        anchors: Anchors::TOP_LEFT,
                        pivot: Pivot::TOP_LEFT,
                        margin: Margin {
                            left: 4.0,
                            top: sug_y,
                            ..Margin::default()
                        },
                        background: [0.0, 0.0, 0.0, 0.82],
                        ..Style::default()
                    },
                );
                world.set(sug_bg, Active(true));
                world.set(
                    sug_txt,
                    Style {
                        width: Some(sug_w - 8.0),
                        height: Some(CHAT_ROW_HEIGHT),
                        anchors: Anchors::TOP_LEFT,
                        pivot: Pivot::TOP_LEFT,
                        margin: Margin {
                            left: 9.0,
                            top: sug_y,
                            ..Margin::default()
                        },
                        ..Style::default()
                    },
                );
                world.set(
                    sug_txt,
                    Text {
                        content: joined,
                        font_size: CHAT_FONT_SIZE,
                        color: [1.0, 1.0, 0.45, 1.0],
                        alignment: TextAlign::Left,
                        wrap: false,
                        ..Text::default()
                    },
                );
                world.set(sug_txt, Active(true));
            }
        }
    }

    fn ensure_chat_ui(&mut self, world: &mut GameWorld<'_>) {
        if self.chat_ui.initialized {
            return;
        }
        let spawn_node = |world: &mut GameWorld<'_>, name: String| -> EntityHandle {
            let ent = world.spawn();
            world.set(ent, Name(name));
            world.set(ent, Node::default());
            world.set(ent, CHAT_CANVAS);
            world.set(ent, Active(false));
            ent
        };

        for idx in 0..MAX_VISIBLE_OPEN {
            let bg = spawn_node(world, format!("chat_row_bg_{idx}"));
            let accent = spawn_node(world, format!("chat_row_accent_{idx}"));
            let text = spawn_node(world, format!("chat_row_text_{idx}"));
            self.chat_ui.rows.push(ChatRowEntities { bg, accent, text });
        }

        self.chat_ui.suggest_bg = Some(spawn_node(world, "chat_suggest_bg".to_string()));
        self.chat_ui.suggest_text = Some(spawn_node(world, "chat_suggest_text".to_string()));
        self.chat_ui.input_bg = Some(spawn_node(world, "chat_input_bg".to_string()));
        self.chat_ui.input_text = Some(spawn_node(world, "chat_input_text".to_string()));
        self.chat_ui.initialized = true;
    }

    fn render_input_with_caret(&self) -> String {
        let caret = if self.caret_blink_sec < 0.5 { "|" } else { " " };
        let byte_idx = self.char_to_byte(self.cursor_char);
        let mut out = String::with_capacity(self.input_line.len() + 2);
        out.push_str(&self.input_line[..byte_idx]);
        out.push_str(caret);
        out.push_str(&self.input_line[byte_idx..]);
        out
    }

    fn char_to_byte(&self, char_idx: usize) -> usize {
        self.input_line
            .char_indices()
            .nth(char_idx)
            .map(|(byte, _)| byte)
            .unwrap_or(self.input_line.len())
    }

    fn insert_char_at_cursor(&mut self, ch: char) {
        let byte_idx = self.char_to_byte(self.cursor_char);
        self.input_line.insert(byte_idx, ch);
        self.cursor_char += 1;
    }

    fn delete_before_cursor(&mut self) {
        if self.cursor_char == 0 {
            return;
        }
        let target_char = self.cursor_char - 1;
        let byte_idx = self.char_to_byte(target_char);
        if byte_idx < self.input_line.len() {
            self.input_line.remove(byte_idx);
            self.cursor_char = target_char;
        }
    }

    fn delete_at_cursor(&mut self) {
        let byte_idx = self.char_to_byte(self.cursor_char);
        if byte_idx < self.input_line.len() {
            self.input_line.remove(byte_idx);
        }
    }

    fn autocomplete(&mut self) {
        let candidates = autocomplete_candidates(&self.input_line);
        let Some(first) = candidates.first() else {
            return;
        };
        if self.input_line.ends_with(' ') {
            self.input_line.push_str(first);
        } else if let Some(last_space) = self.input_line.rfind(' ') {
            self.input_line.truncate(last_space + 1);
            self.input_line.push_str(first);
        } else {
            let has_slash = self.input_line.starts_with('/');
            self.input_line.clear();
            if has_slash && !first.starts_with('/') {
                self.input_line.push('/');
            }
            self.input_line.push_str(first);
        }
        self.cursor_char = self.input_line.chars().count();
    }

    /// Parse and execute one Minecraft-style slash command.
    pub fn execute(
        &mut self,
        raw: &str,
        world: &mut GameWorld<'_>,
        sim: &mut SimWorld,
        player: &mut PlayerController,
    ) -> String {
        let trimmed = raw.trim().strip_prefix('/').unwrap_or(raw.trim());
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            return String::new();
        }
        match parts[0].to_ascii_lowercase().as_str() {
            "help" | "?" => "Commands: /time <set|add|query>, /weather <clear|rain|thunder>, /gamemode <survival|creative|walk|fly>, /tp <x> <y> <z>, /setblock <x> <y> <z> <mat>, /gamerule <rule> <bool>, /upscaler <off|sgsr2|asr|query> [scale] [sharpness], /seed, /clear".to_string(),
            "seed" => format!("Seed: {}", sim.seed),
            "clear" => {
                self.chat_log.clear();
                "Cleared chat history.".to_string()
            }
            "time" => self.exec_time(&parts[1..], world),
            "weather" => self.exec_weather(&parts[1..], world),
            "gamemode" | "gm" => self.exec_gamemode(&parts[1..], player),
            "tp" | "teleport" => self.exec_tp(&parts[1..], world, sim, player),
            "setblock" => self.exec_setblock(&parts[1..], sim, player),
            "gamerule" => self.exec_gamerule(&parts[1..]),
            "upscaler" => self.exec_upscaler(&parts[1..]),
            "dlfg" => {
                let next = match parts.get(1).copied().map(str::to_ascii_lowercase).as_deref() {
                    Some("on") => 1,
                    Some("auto") => 2,
                    Some("off") => 0,
                    _ => (DLFG_MODE.load(Ordering::Relaxed) + 1) % 3,
                };
                DLFG_MODE.store(next, Ordering::Relaxed);
                let label = match next {
                    1 => "On",
                    2 => "Auto",
                    _ => "Off",
                };
                format!("DLFG (Frame Gen) set to {label}")
            }
            "ddgi" => {
                let next = match parts.get(1).copied().map(str::to_ascii_lowercase).as_deref() {
                    Some("on" | "true") => true,
                    Some("off" | "false") => false,
                    _ => !DDGI_ENABLED.load(Ordering::Relaxed),
                };
                DDGI_ENABLED.store(next, Ordering::Relaxed);
                format!("DDGI set to {}", if next { "ON" } else { "OFF" })
            }
            other => format!("Unknown command '{other}'. Type /help."),
        }
    }

    fn exec_time(&mut self, args: &[&str], world: &mut GameWorld<'_>) -> String {
        match args {
            ["query"] => format!("Time is {} ticks", self.time_ticks),
            ["set", val] => {
                let ticks = match val.to_ascii_lowercase().as_str() {
                    "day" => Some(1_000),
                    "noon" => Some(6_000),
                    "sunset" => Some(12_000),
                    "night" => Some(13_000),
                    "midnight" => Some(18_000),
                    _ => val.parse::<u64>().ok(),
                };
                if let Some(t) = ticks {
                    self.time_ticks = t % TICKS_PER_DAY;
                    self.sync_sky_and_sun(world);
                    format!("Set time to {}", self.time_ticks)
                } else {
                    format!("Invalid time '{val}'")
                }
            }
            ["add", val] => {
                if let Ok(delta) = val.parse::<u64>() {
                    self.time_ticks = (self.time_ticks + delta) % TICKS_PER_DAY;
                    self.sync_sky_and_sun(world);
                    format!("Added {delta} ticks (now {})", self.time_ticks)
                } else {
                    format!("Invalid tick delta '{val}'")
                }
            }
            _ => "Usage: /time <set|add|query> <value>".to_string(),
        }
    }

    fn exec_weather(&mut self, args: &[&str], world: &mut GameWorld<'_>) -> String {
        let Some(&kind_str) = args.first() else {
            return format!("Current weather: {}", self.weather.label());
        };
        let duration = args
            .get(1)
            .and_then(|s| s.parse::<f32>().ok())
            .unwrap_or(180.0)
            .clamp(5.0, 3600.0);
        self.weather = match kind_str.to_ascii_lowercase().as_str() {
            "clear" => WeatherKind::Clear,
            "rain" => WeatherKind::Rain,
            "thunder" => WeatherKind::Thunder,
            _ => return "Usage: /weather <clear|rain|thunder> [seconds]".to_string(),
        };
        self.weather_timer_sec = if self.weather == WeatherKind::Clear {
            0.0
        } else {
            duration
        };
        self.sync_sky_and_sun(world);
        format!("Set weather to {}", self.weather.label())
    }

    fn exec_gamemode(&self, args: &[&str], player: &mut PlayerController) -> String {
        let Some(&mode) = args.first() else {
            return format!(
                "Current gamemode: {}",
                if player.fly_mode {
                    "creative"
                } else {
                    "survival"
                }
            );
        };
        match mode.to_ascii_lowercase().as_str() {
            "survival" | "walk" | "0" => {
                player.fly_mode = false;
                player.velocity = Vec3::ZERO;
                "Set gamemode to survival (walk)".to_string()
            }
            "creative" | "fly" | "spectator" | "1" | "3" => {
                player.fly_mode = true;
                player.velocity = Vec3::ZERO;
                "Set gamemode to creative (fly)".to_string()
            }
            _ => "Usage: /gamemode <survival|creative|walk|fly>".to_string(),
        }
    }

    fn parse_rel(arg: &str, base: f32) -> Option<f32> {
        if let Some(rest) = arg.strip_prefix('~') {
            if rest.is_empty() {
                Some(base)
            } else {
                rest.parse::<f32>().ok().map(|d| base + d)
            }
        } else {
            arg.parse::<f32>().ok()
        }
    }

    fn exec_tp(
        &self,
        args: &[&str],
        world: &mut GameWorld<'_>,
        sim: &mut SimWorld,
        player: &mut PlayerController,
    ) -> String {
        if args.len() != 3 {
            return "Usage: /tp <x> <y> <z>".to_string();
        }
        let (Some(x), Some(y), Some(z)) = (
            Self::parse_rel(args[0], player.feet_pos.x),
            Self::parse_rel(args[1], player.feet_pos.y),
            Self::parse_rel(args[2], player.feet_pos.z),
        ) else {
            return "Invalid coordinates for /tp".to_string();
        };
        player.feet_pos = Vec3::new(x, y.clamp(1.0, 62.0), z);
        player.velocity = Vec3::ZERO;
        sim.stream_around(player.feet_pos, Some(world), 16);
        format!("Teleported to ({x:.1}, {y:.1}, {z:.1})")
    }

    fn parse_mat_name(name: &str) -> Option<u8> {
        match name.to_ascii_lowercase().as_str() {
            "air" => Some(mat_id::AIR),
            "bedrock" => Some(mat_id::BEDROCK),
            "dirt" | "soil" => Some(mat_id::SOIL),
            "grass" => Some(mat_id::GRASS),
            "stone" | "limestone" => Some(mat_id::LIMESTONE),
            "alloy" | "iron" => Some(mat_id::ALLOY),
            "glass" => Some(mat_id::GLASS),
            "timber" | "wood" | "planks" => Some(mat_id::TIMBER),
            "water" => Some(mat_id::WATER),
            "acid" => Some(mat_id::ACID),
            "cryo" | "ice" => Some(mat_id::CRYO),
            "biomass" => Some(mat_id::BIOMASS),
            "wire" | "copper" => Some(mat_id::WIRE),
            "conduit" => Some(mat_id::CONDUIT),
            "valve" => Some(mat_id::VALVE),
            "isotope" => Some(mat_id::ISOTOPE),
            "tank" => Some(mat_id::TANK),
            _ => None,
        }
    }

    fn exec_setblock(
        &self,
        args: &[&str],
        sim: &mut SimWorld,
        player: &PlayerController,
    ) -> String {
        if args.len() != 4 {
            return "Usage: /setblock <x> <y> <z> <material>".to_string();
        }
        let (Some(x), Some(y), Some(z)) = (
            Self::parse_rel(args[0], player.feet_pos.x),
            Self::parse_rel(args[1], player.feet_pos.y),
            Self::parse_rel(args[2], player.feet_pos.z),
        ) else {
            return "Invalid coordinates for /setblock".to_string();
        };
        let Some(mat) = Self::parse_mat_name(args[3]) else {
            return format!("Unknown material '{}'", args[3]);
        };
        let l2 = [
            (x * 4.0).floor() as i32,
            (y * 4.0).floor() as i32,
            (z * 4.0).floor() as i32,
        ];
        if sim.set_l2_material(l2, mat) {
            format!("Placed {} at ({x:.1}, {y:.1}, {z:.1})", args[3])
        } else {
            "Target chunk not loaded".to_string()
        }
    }

    fn exec_gamerule(&mut self, args: &[&str]) -> String {
        if args.len() != 2 {
            return "Usage: /gamerule <doDaylightCycle|doWeatherCycle|doFireTick> <true|false>"
                .to_string();
        }
        let Ok(val) = args[1].parse::<bool>() else {
            return format!("Invalid boolean '{}'", args[1]);
        };
        match args[0] {
            "doDaylightCycle" => self.rules.do_daylight_cycle = val,
            "doWeatherCycle" => self.rules.do_weather_cycle = val,
            "doFireTick" => self.rules.do_fire_tick = val,
            other => return format!("Unknown gamerule '{other}'"),
        }
        format!("Gamerule {} = {val}", args[0])
    }

    fn exec_upscaler(&self, args: &[&str]) -> String {
        if args.is_empty() || args[0].eq_ignore_ascii_case("query") {
            let (id, scale, sharp) = current_upscaler_config();
            return format!(
                "Upscaler: {} (scale={scale:.2}, sharpness={sharp:.2})",
                upscaler_mode_label(id)
            );
        }
        let Some(id) = parse_upscaler_mode_id(args[0]) else {
            return "Usage: /upscaler <off|nearest|linear|sgsr2|asr|dlss|fsr|query> [scale 0.35..1.0] [sharpness 0..1]".to_string();
        };
        UPSCALER_MODE.store(id, Ordering::Relaxed);
        if let Some(raw_scale) = args.get(1) {
            let Ok(v) = raw_scale.parse::<f32>() else {
                return format!("Invalid upscaler scale '{raw_scale}'");
            };
            UPSCALER_SCALE_BITS.store(v.clamp(0.35, 1.0).to_bits(), Ordering::Relaxed);
        }
        if let Some(raw_sharp) = args.get(2) {
            let Ok(v) = raw_sharp.parse::<f32>() else {
                return format!("Invalid upscaler sharpness '{raw_sharp}'");
            };
            UPSCALER_SHARPNESS_BITS.store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
        let (_, scale, sharp) = current_upscaler_config();
        format!(
            "Upscaler set to {} (scale={scale:.2}, sharpness={sharp:.2})",
            upscaler_mode_label(id)
        )
    }
}

fn is_known_command(word: &str) -> bool {
    matches!(
        word,
        "help"
            | "?"
            | "time"
            | "weather"
            | "gamemode"
            | "gm"
            | "tp"
            | "teleport"
            | "setblock"
            | "gamerule"
            | "upscaler"
            | "seed"
            | "clear"
    )
}

fn classify_feedback_tag(msg: &str) -> ChatTag {
    if msg.starts_with("Unknown") || msg.starts_with("Invalid") || msg.starts_with("Usage") {
        ChatTag::Error
    } else if msg.starts_with("Target chunk") {
        ChatTag::Warning
    } else {
        ChatTag::Command
    }
}

fn poll_typed_char(input: &InputSnapshot) -> Option<char> {
    let shift = input.key(keys::SHIFT_LEFT) || input.key(keys::SHIFT_RIGHT);
    if input.key_pressed(keys::SPACE) {
        return Some(' ');
    }
    for (k, lower, upper) in [
        (keys::KEY_W, 'w', 'W'),
        (keys::KEY_A, 'a', 'A'),
        (keys::KEY_S, 's', 'S'),
        (keys::KEY_D, 'd', 'D'),
        (keys::KEY_Q, 'q', 'Q'),
        (keys::KEY_E, 'e', 'E'),
        (keys::KEY_T, 't', 'T'),
        (keys::DIGIT_0, '0', ')'),
        (keys::DIGIT_1, '1', '!'),
        (keys::DIGIT_2, '2', '@'),
        (keys::DIGIT_3, '3', '#'),
        (keys::DIGIT_4, '4', '$'),
        (keys::DIGIT_5, '5', '%'),
        (keys::DIGIT_6, '6', '^'),
        (keys::DIGIT_7, '7', '&'),
        (keys::DIGIT_8, '8', '*'),
        (keys::DIGIT_9, '9', '('),
    ] {
        if input.key_pressed(k) {
            return Some(if shift { upper } else { lower });
        }
    }
    for b in b'A'..=b'Z' {
        if matches!(b, b'W' | b'A' | b'S' | b'D' | b'Q' | b'E' | b'T') {
            continue;
        }
        if input.key_pressed(keys::OTHER_BASE + b as usize) {
            let ch = b as char;
            return Some(if shift { ch } else { ch.to_ascii_lowercase() });
        }
    }
    for (vk, normal, shifted) in [
        (0xBAusize, ';', ':'),
        (0xBBusize, '=', '+'),
        (0xBCusize, ',', '<'),
        (0xBDusize, '-', '_'),
        (0xBEusize, '.', '>'),
        (0xBFusize, '/', '?'),
        (0xC0usize, '`', '~'),
    ] {
        if input.key_pressed(keys::OTHER_BASE + vk) {
            return Some(if shift { shifted } else { normal });
        }
    }
    for (vk, ch) in [
        (0x60usize, '0'),
        (0x61usize, '1'),
        (0x62usize, '2'),
        (0x63usize, '3'),
        (0x64usize, '4'),
        (0x65usize, '5'),
        (0x66usize, '6'),
        (0x67usize, '7'),
        (0x68usize, '8'),
        (0x69usize, '9'),
        (0x6Ausize, '*'),
        (0x6Busize, '+'),
        (0x6Dusize, '-'),
        (0x6Eusize, '.'),
        (0x6Fusize, '/'),
    ] {
        if input.key_pressed(keys::OTHER_BASE + vk) {
            return Some(ch);
        }
    }
    None
}

fn autocomplete_candidates(input_line: &str) -> Vec<&'static str> {
    const COMMANDS: &[&str] = &[
        "time", "weather", "gamemode", "tp", "setblock", "gamerule", "upscaler", "seed", "help",
        "clear",
    ];
    let trimmed = input_line.trim_start();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let body = trimmed.strip_prefix('/').unwrap_or(trimmed);
    let has_slash = trimmed.starts_with('/');
    let ends_space = input_line.ends_with(' ');
    let tokens: Vec<&str> = body.split_whitespace().collect();

    if tokens.is_empty() {
        return if has_slash {
            COMMANDS.to_vec()
        } else {
            Vec::new()
        };
    }

    if tokens.len() == 1 && !ends_space {
        let prefix = tokens[0].to_ascii_lowercase();
        if !has_slash && !COMMANDS.iter().any(|c| c.starts_with(&prefix)) {
            return Vec::new();
        }
        return COMMANDS
            .iter()
            .copied()
            .filter(|cmd| cmd.starts_with(&prefix) && *cmd != prefix)
            .collect();
    }

    let cmd = tokens[0].to_ascii_lowercase();
    let arg_idx = if ends_space {
        tokens.len() - 1
    } else {
        tokens.len() - 2
    };
    let arg_prefix = if ends_space {
        ""
    } else {
        tokens.last().copied().unwrap_or("")
    };
    let options: &[&str] = match (cmd.as_str(), arg_idx) {
        ("time", 0) => &["set", "add", "query"],
        ("time", 1) if tokens.get(1).copied() == Some("set") => {
            &["day", "noon", "sunset", "night", "midnight"]
        }
        ("weather", 0) => &["clear", "rain", "thunder"],
        ("gamemode" | "gm", 0) => &["survival", "creative", "spectator", "walk", "fly"],
        ("gamerule", 0) => &["doDaylightCycle", "doWeatherCycle", "doFireTick"],
        ("gamerule", 1) => &["true", "false"],
        ("upscaler", 0) => &[
            "off", "nearest", "linear", "sgsr2", "asr", "dlss", "fsr", "query",
        ],
        ("setblock", 3) => &[
            "stone", "alloy", "glass", "timber", "water", "acid", "cryo", "biomass", "wire",
            "isotope", "air",
        ],
        _ => &[],
    };
    let prefix_lower = arg_prefix.to_ascii_lowercase();
    options
        .iter()
        .copied()
        .filter(|opt| opt.to_ascii_lowercase().starts_with(&prefix_lower) && *opt != arg_prefix)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::materials::MaterialTable;
    use crate::world_gen::generate_world;

    #[test]
    fn upscaler_command_switches_modes_and_placeholders() {
        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 2026);
        let mut sim = SimWorld::new(mats, gen.chunks, gen.cells, gen.active);
        let mut player = PlayerController::new();
        let mut ecs = prism_ecs::World::new();
        let mut world = GameWorld::host(&mut ecs);
        let mut cmd = CommandWorldState::new();

        let msg = cmd.execute(
            "/upscaler sgsr2 0.75 0.6",
            &mut world,
            &mut sim,
            &mut player,
        );
        assert!(msg.contains("SGSR 2"), "msg={msg}");
        assert_eq!(current_upscaler_config(), (1, 0.75, 0.6));

        let msg = cmd.execute("/upscaler asr 0.67 0.5", &mut world, &mut sim, &mut player);
        assert!(msg.contains("Arm ASR"), "msg={msg}");
        assert_eq!(current_upscaler_config(), (2, 0.67, 0.5));

        let msg = cmd.execute("/upscaler nearest 0.5", &mut world, &mut sim, &mut player);
        assert!(msg.contains("Nearest"), "msg={msg}");
        assert_eq!(current_upscaler_config(), (3, 0.5, 0.5));

        let msg = cmd.execute("/upscaler linear 0.85", &mut world, &mut sim, &mut player);
        assert!(msg.contains("Linear"), "msg={msg}");
        assert_eq!(current_upscaler_config(), (4, 0.85, 0.5));

        #[cfg(not(target_os = "android"))]
        {
            let msg = cmd.execute("/upscaler dlss", &mut world, &mut sim, &mut player);
            assert!(msg.contains("DLSS"), "msg={msg}");
            assert_eq!(current_upscaler_config().0, 5);

            let msg = cmd.execute("/upscaler fsr", &mut world, &mut sim, &mut player);
            assert!(msg.contains("FSR"), "msg={msg}");
            assert_eq!(current_upscaler_config().0, 6);
        }

        let _ = cmd.execute("/upscaler off", &mut world, &mut sim, &mut player);
        assert_eq!(current_upscaler_config().0, 0);
    }

    #[test]
    fn commands_update_time_weather_gamemode_tp_and_gamerules() {
        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 2026);
        let mut sim = SimWorld::new(mats, gen.chunks, gen.cells, gen.active);
        let mut ecs = prism_ecs::World::new();
        let mut world = GameWorld::host(&mut ecs);
        let sun = world.spawn();
        world.set(sun, DirectionalLight::default());

        let mut player = PlayerController::new();
        player.attach_ecs(&mut world, &sim);
        let mut cmd = CommandWorldState::new();

        assert!(cmd
            .execute("/time set midnight", &mut world, &mut sim, &mut player)
            .contains("18000"));
        assert_eq!(cmd.time_ticks, 18_000);

        cmd.execute("/weather thunder 120", &mut world, &mut sim, &mut player);
        assert_eq!(cmd.weather, WeatherKind::Thunder);

        cmd.execute("/gamemode creative", &mut world, &mut sim, &mut player);
        assert!(player.fly_mode);

        cmd.execute("/tp ~10 20 ~-5", &mut world, &mut sim, &mut player);
        assert!((player.feet_pos.x - 16.5).abs() < 0.01);
        assert!((player.feet_pos.y - 20.0).abs() < 0.01);

        cmd.execute(
            "/gamerule doDaylightCycle false",
            &mut world,
            &mut sim,
            &mut player,
        );
        assert!(!cmd.rules.do_daylight_cycle);

        let placed = cmd.execute("/setblock 1 2 1 stone", &mut world, &mut sim, &mut player);
        assert!(placed.starts_with("Placed stone"), "unexpected: {placed}");

        // Verify full time-of-day lighting: noon -> high warm sun, midnight -> cool moonlight above horizon.
        cmd.execute("/weather clear", &mut world, &mut sim, &mut player);
        cmd.execute("/time set noon", &mut world, &mut sim, &mut player);
        let noon_light = world.get::<DirectionalLight>(sun).unwrap();
        assert!(
            noon_light.euler_xyz.x > 50.0,
            "noon sun should be high: {:?}",
            noon_light.euler_xyz
        );
        assert!(noon_light.intensity > 60_000.0);

        cmd.execute("/time set midnight", &mut world, &mut sim, &mut player);
        let night_light = world.get::<DirectionalLight>(sun).unwrap();
        assert!(
            night_light.euler_xyz.x > 40.0,
            "midnight moon should be above horizon: {:?}",
            night_light.euler_xyz
        );
        assert!(
            night_light.color.z > night_light.color.x,
            "moonlight should be cool blue"
        );
        assert!(night_light.intensity < 20_000.0);
    }

    fn press(key_idx: usize) -> InputSnapshot {
        let mut input = InputSnapshot::default();
        input.keys[key_idx / 64] |= 1u64 << (key_idx % 64);
        input.key_edges[key_idx / 64] |= 1u64 << (key_idx % 64);
        input
    }

    fn press_shifted(key_idx: usize) -> InputSnapshot {
        let mut input = press(key_idx);
        input.keys[keys::SHIFT_LEFT / 64] |= 1u64 << (keys::SHIFT_LEFT % 64);
        input
    }

    #[test]
    fn t_enter_and_slash_triggers_align_with_horizon_and_replace_f1() {
        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 2026);
        let mut sim = SimWorld::new(mats, gen.chunks, gen.cells, gen.active);
        let mut ecs = prism_ecs::World::new();
        let mut world = GameWorld::host(&mut ecs);
        let mut player = PlayerController::new();
        player.attach_ecs(&mut world, &sim);
        let mut cmd = CommandWorldState::new();

        // F1 no longer opens the command console.
        assert!(!cmd.handle_console_input(&press(keys::F1), &mut world, &mut sim, &mut player));
        assert!(!cmd.console_open);

        // `T` opens console with an empty buffer and does not inject 't'.
        assert!(cmd.handle_console_input(&press(keys::KEY_T), &mut world, &mut sim, &mut player));
        assert!(cmd.console_open);
        assert_eq!(cmd.input_line, "");

        // `Escape` cancels and closes console.
        assert!(cmd.handle_console_input(&press(keys::ESCAPE), &mut world, &mut sim, &mut player));
        assert!(!cmd.console_open);

        // `Enter` when closed opens console empty and does not execute immediately.
        assert!(cmd.handle_console_input(&press(keys::ENTER), &mut world, &mut sim, &mut player));
        assert!(cmd.console_open);
        assert_eq!(cmd.input_line, "");
        assert!(cmd.chat_log.is_empty());
        assert!(cmd.handle_console_input(&press(keys::ESCAPE), &mut world, &mut sim, &mut player));

        // `/` opens console prefilled with `/`.
        assert!(cmd.handle_console_input(
            &press(keys::OTHER_BASE + 0xBF),
            &mut world,
            &mut sim,
            &mut player,
        ));
        assert!(cmd.console_open);
        assert_eq!(cmd.input_line, "/");
        assert_eq!(cmd.cursor_char, 1);
    }

    #[test]
    fn console_supports_vk_punctuation_editing_tab_history_and_chat_feed() {
        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 2026);
        let mut sim = SimWorld::new(mats, gen.chunks, gen.cells, gen.active);
        let mut ecs = prism_ecs::World::new();
        let mut world = GameWorld::host(&mut ecs);
        let mut player = PlayerController::new();
        player.attach_ecs(&mut world, &sim);
        let mut cmd = CommandWorldState::new();

        // Open with `T` and type `/ti` + Tab -> `/time`, then ` set day` -> Enter.
        cmd.handle_console_input(&press(keys::KEY_T), &mut world, &mut sim, &mut player);
        for k in [
            press(keys::OTHER_BASE + 0xBF),
            press(keys::KEY_T),
            press(keys::OTHER_BASE + b'I' as usize),
        ] {
            cmd.handle_console_input(&k, &mut world, &mut sim, &mut player);
        }
        assert_eq!(cmd.input_line, "/ti");
        cmd.handle_console_input(&press(keys::TAB), &mut world, &mut sim, &mut player);
        assert_eq!(cmd.input_line, "/time");

        // Cursor navigation + editing: add `x` at start via Home, then Delete it.
        cmd.handle_console_input(
            &press(keys::OTHER_BASE + 0x24),
            &mut world,
            &mut sim,
            &mut player,
        );
        assert_eq!(cmd.cursor_char, 0);
        cmd.handle_console_input(
            &press(keys::OTHER_BASE + b'X' as usize),
            &mut world,
            &mut sim,
            &mut player,
        );
        assert_eq!(cmd.input_line, "x/time");
        cmd.handle_console_input(
            &press(keys::OTHER_BASE + 0x08),
            &mut world,
            &mut sim,
            &mut player,
        );
        assert_eq!(cmd.input_line, "/time");
        cmd.handle_console_input(
            &press(keys::OTHER_BASE + 0x23),
            &mut world,
            &mut sim,
            &mut player,
        );

        for k in [
            press(keys::SPACE),
            press(keys::KEY_S),
            press(keys::KEY_E),
            press(keys::KEY_T),
            press(keys::SPACE),
            press(keys::KEY_D),
            press(keys::KEY_A),
            press(keys::OTHER_BASE + b'Y' as usize),
        ] {
            cmd.handle_console_input(&k, &mut world, &mut sim, &mut player);
        }
        assert_eq!(cmd.input_line, "/time set day");
        cmd.handle_console_input(&press(keys::ENTER), &mut world, &mut sim, &mut player);
        assert!(!cmd.console_open);
        assert_eq!(cmd.time_ticks, 1_000);
        assert_eq!(cmd.history, vec!["/time set day".to_string()]);
        assert_eq!(cmd.chat_log[0].tag, ChatTag::Command);

        // Open with `Enter`, test relative tilde (`Shift + backquote`) + minus + dot, then chat msg.
        cmd.handle_console_input(&press(keys::ENTER), &mut world, &mut sim, &mut player);
        for k in [
            press_shifted(keys::OTHER_BASE + 0xC0),
            press(keys::OTHER_BASE + 0xBD),
            press(keys::DIGIT_5),
            press(keys::OTHER_BASE + 0xBE),
            press(keys::DIGIT_2),
        ] {
            cmd.handle_console_input(&k, &mut world, &mut sim, &mut player);
        }
        assert_eq!(cmd.input_line, "~-5.2");
        cmd.handle_console_input(&press(keys::ARROW_UP), &mut world, &mut sim, &mut player);
        assert_eq!(cmd.input_line, "/time set day");
        cmd.handle_console_input(&press(keys::ARROW_DOWN), &mut world, &mut sim, &mut player);
        assert_eq!(cmd.input_line, "~-5.2");
        cmd.handle_console_input(&press(keys::ENTER), &mut world, &mut sim, &mut player);
        assert_eq!(cmd.chat_log[0].tag, ChatTag::Player);
        assert_eq!(cmd.chat_log[0].text, "<Player> ~-5.2");

        // Sync chat UI spawns pooled bottom-left entities and ages out unfocused lines after 10s.
        cmd.sync_chat_ui(&mut world, 0.2);
        assert!(cmd.chat_ui.initialized);
        assert_eq!(
            world.get::<Active>(cmd.chat_ui.rows[0].bg),
            Some(Active(true))
        );
        for _ in 0..45 {
            cmd.sync_chat_ui(&mut world, 0.25);
        }
        assert_eq!(
            world.get::<Active>(cmd.chat_ui.rows[0].bg),
            Some(Active(false))
        );
        assert_eq!(
            world.get::<Active>(cmd.chat_ui.rows[0].text),
            Some(Active(false))
        );

        // `/clear` clears the chat log.
        cmd.execute("/clear", &mut world, &mut sim, &mut player);
        assert!(cmd.chat_log.is_empty());
    }
}
