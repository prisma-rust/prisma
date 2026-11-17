//! Minecraft-style slash-command system (`time`, `weather`, `gamemode`, `tp`,
//! `setblock`, `gamerule`, `seed`, `help`) for `emergence`.

use glam::Vec3;
use prism_game_api::{keys, GameWorld, InputSnapshot};
use prism_scene::{Camera, DirectionalLight};

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
pub struct CommandWorldState {
    pub time_ticks: u64,
    pub weather: WeatherKind,
    pub weather_timer_sec: f32,
    pub rules: GameRules,
    pub console_open: bool,
    pub input_line: String,
    pub history: Vec<String>,
    pub history_cursor: Option<usize>,
    pub feedback: String,
    pub feedback_timer_sec: f32,
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
            history: Vec::new(),
            history_cursor: None,
            feedback: String::new(),
            feedback_timer_sec: 0.0,
            time_accum_sec: 0.0,
            weather_stim_accum_sec: 0.0,
        }
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
            if input.key_pressed(keys::OTHER_BASE + b'/' as usize) || input.key_pressed(keys::F1) {
                self.console_open = true;
                self.input_line.clear();
                self.input_line.push('/');
                self.history_cursor = None;
                return true;
            }
            return false;
        }

        if input.key_pressed(keys::ESCAPE) {
            self.console_open = false;
            self.input_line.clear();
            return true;
        }

        if input.key_pressed(keys::ENTER) {
            let cmd = self.input_line.trim().to_string();
            self.console_open = false;
            self.input_line.clear();
            if !cmd.is_empty() {
                if self.history.last() != Some(&cmd) {
                    self.history.push(cmd.clone());
                }
                let msg = self.execute(&cmd, world, sim, player);
                self.feedback = msg;
                self.feedback_timer_sec = 5.0;
            }
            return true;
        }

        if input.key_pressed(keys::ARROW_UP) && !self.history.is_empty() {
            let next = match self.history_cursor {
                None => self.history.len() - 1,
                Some(i) => i.saturating_sub(1),
            };
            self.history_cursor = Some(next);
            self.input_line = self.history[next].clone();
            return true;
        }
        if input.key_pressed(keys::ARROW_DOWN) {
            if let Some(i) = self.history_cursor {
                if i + 1 < self.history.len() {
                    self.history_cursor = Some(i + 1);
                    self.input_line = self.history[i + 1].clone();
                } else {
                    self.history_cursor = None;
                    self.input_line.clear();
                }
            }
            return true;
        }

        // Backspace (ASCII 8 or 127).
        if input.key_pressed(keys::OTHER_BASE + 8) || input.key_pressed(keys::OTHER_BASE + 127) {
            self.input_line.pop();
            return true;
        }

        // Text entry mapping from `keys` bitset.
        let shift = input.key(keys::SHIFT_LEFT) || input.key(keys::SHIFT_RIGHT);
        if input.key_pressed(keys::SPACE) {
            self.input_line.push(' ');
        }
        for (k, ch) in [
            (keys::KEY_W, 'w'),
            (keys::KEY_A, 'a'),
            (keys::KEY_S, 's'),
            (keys::KEY_D, 'd'),
            (keys::KEY_Q, 'q'),
            (keys::KEY_E, 'e'),
            (keys::KEY_T, 't'),
            (keys::DIGIT_0, '0'),
            (keys::DIGIT_1, '1'),
            (keys::DIGIT_2, '2'),
            (keys::DIGIT_3, '3'),
            (keys::DIGIT_4, '4'),
            (keys::DIGIT_5, '5'),
            (keys::DIGIT_6, '6'),
            (keys::DIGIT_7, '7'),
            (keys::DIGIT_8, '8'),
            (keys::DIGIT_9, '9'),
        ] {
            if input.key_pressed(k) {
                self.input_line.push(ch);
            }
        }
        for b in b'A'..=b'Z' {
            if matches!(b, b'W' | b'A' | b'S' | b'D' | b'Q' | b'E' | b'T') {
                continue;
            }
            if input.key_pressed(keys::OTHER_BASE + b as usize) {
                self.input_line.push((b as char).to_ascii_lowercase());
            }
        }
        for (raw, normal, shifted) in [
            (b'/' as usize, '/', '?'),
            (b'-' as usize, '-', '_'),
            (b'.' as usize, '.', '>'),
            (b'`' as usize, '~', '~'),
        ] {
            if input.key_pressed(keys::OTHER_BASE + raw) {
                self.input_line.push(if shift { shifted } else { normal });
            }
        }
        true
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
            "help" => "Commands: /time <set|add|query>, /weather <clear|rain|thunder>, /gamemode <survival|creative|walk|fly>, /tp <x> <y> <z>, /setblock <x> <y> <z> <mat>, /gamerule <rule> <bool>, /seed".to_string(),
            "seed" => format!("Seed: {}", sim.seed),
            "time" => self.exec_time(&parts[1..], world),
            "weather" => self.exec_weather(&parts[1..], world),
            "gamemode" | "gm" => self.exec_gamemode(&parts[1..], player),
            "tp" | "teleport" => self.exec_tp(&parts[1..], world, sim, player),
            "setblock" => self.exec_setblock(&parts[1..], sim, player),
            "gamerule" => self.exec_gamerule(&parts[1..]),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::materials::MaterialTable;
    use crate::world_gen::generate_world;

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
}
