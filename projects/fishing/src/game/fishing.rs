//! Fishing state machine (M0 vertical slice).
//!
//! This is the **user project** gameplay layer: engine only provides generic `LineRenderer` /
//! `SpriteRenderer` and input snapshot; rod, bobber, bite timing all here.
//!
//! State transitions:
//!
//! ```text
//! Idle ──click──> Casting ──splash──> Waiting ──timer──> Biting
//!   ^                                                  │
//!   └────── Caught <──reel success ── Reeling <──click────────┘
//!                                                      └──timeout ──> Waiting
//! ```
//!
//! State and read / write go through [`prism_game_api::GameWorld`]: same structure as `boat` / `world`,
//! static Player and editor dynamic module share same gameplay code.

use prism_game_api::{keys, EntityHandle, GameWorld};
use prism_scene::components::{LineRenderer, LocalTransform, Name, SpriteRenderer, TimeOfDay};
use prism_scene::ui::Text;

/// Rod tip offset relative to boat anchor (boat-local pixels).
const ROD_OFFSET: [f32; 2] = [44.0, 24.0];
/// Horizontal distance of cast landing point relative to boat.
const CAST_DISTANCE: f32 = 150.0;
/// Bobber flight time.
const CAST_SECONDS: f32 = 0.55;
/// Bite hint duration (timeout unhooks).
const BITE_WINDOW: f32 = 1.1;
/// Reel animation time.
const REEL_SECONDS: f32 = 0.9;
/// Water surface height (world y).
const WATER_LINE: f32 = 80.0;
/// Fishing line sort layer.
const LINE_LAYER: i32 = 45;

/// Fishing phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Casting,
    Waiting,
    Biting,
    Reeling,
    Caught,
}

impl Default for Phase {
    fn default() -> Self {
        Self::Idle
    }
}

/// Fishing state — **project-private**, held by caller (Player side in ECS resource).
#[derive(Default)]
pub struct FishingState {
    phase: Phase,
    /// Time elapsed in current phase (seconds).
    elapsed: f32,
    /// Target duration of this phase; `0` means decided by logic.
    duration: f32,
    /// Bobber current world position.
    bobber_pos: [f32; 2],
    /// Cast start point (boat anchor).
    cast_origin: [f32; 2],
    /// Bite time of this waiting (pseudo-random).
    bite_at: f32,
    /// Bobber position when the hook was set; the reel interpolates from here.
    reel_origin: [f32; 2],
    /// Fish caught count (M0 only counts).
    caught: u32,
    /// HUD top-bar text entity (`title`), writes fish count and time each frame.
    hud: Option<EntityHandle>,
    boat: Option<EntityHandle>,
    bobber: Option<EntityHandle>,
    line: Option<EntityHandle>,
}

impl FishingState {
    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn caught(&self) -> u32 {
        self.caught
    }
}

/// HUD top-bar text entity (called `title` in scene, placed by project own scene).
fn find_hud_label(world: &mut GameWorld<'_>) -> Option<EntityHandle> {
    world.find_by_name("title")
}

/// Advance fishing state machine each frame.
pub fn tick(state: &mut Option<FishingState>, world: &mut GameWorld<'_>, dt: f32) {
    if state.is_none() {
        let mut fresh = FishingState::default();
        fresh.boat = world.find_by_name("boat");
        fresh.bobber = world.find_by_name("bobber");
        // #984: progress restored from player save (key is `caught`, project-defined schema).
        fresh.caught = world
            .load_data("caught")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        if fresh.caught > 0 {
            log::info!(
                "fishing: restored {} caught fish from the save",
                fresh.caught
            );
        }
        fresh.hud = find_hud_label(world);
        fresh.line = spawn_line(world);
        *state = Some(fresh);
    }

    let Some(current) = state.as_mut() else {
        return;
    };

    // --- input (input snapshot sent down by host each frame) ---
    let input = world.input();
    let clicked = input.left_clicked;
    // Cast and hook-set are edge-triggered: holding the key counts once, like `left_clicked`.
    let pressed_space = input.key_pressed(keys::SPACE);
    // `FISHING_AUTOPLAY=1` makes state machine advance itself: for demos and automated screenshots,
    // normal play does not set this var, fully input-driven.
    let autoplay = std::env::var_os("FISHING_AUTOPLAY").is_some();
    let auto_fire =
        autoplay && matches!(current.phase, Phase::Idle | Phase::Biting) && current.elapsed > 1.0;
    let action = clicked || pressed_space || auto_fire;

    // --- boat anchor ---
    let boat_pos = current
        .boat
        .and_then(|boat| world.get::<LocalTransform>(boat))
        .map(|transform| [transform.position.x, transform.position.y])
        .unwrap_or([0.0, WATER_LINE]);

    current.elapsed += dt;

    match current.phase {
        Phase::Idle => {
            if action {
                current.phase = Phase::Casting;
                current.elapsed = 0.0;
                current.duration = CAST_SECONDS;
                current.cast_origin = [boat_pos[0] + ROD_OFFSET[0], boat_pos[1] + ROD_OFFSET[1]];
                current.bobber_pos = current.cast_origin;
                log::info!("fishing: cast");
            }
        }
        Phase::Casting => {
            let t = (current.elapsed / CAST_SECONDS).clamp(0.0, 1.0);
            let target = [boat_pos[0] + CAST_DISTANCE, WATER_LINE];
            // Parabola: horizontal linear interpolation, vertical adds an arc.
            let arc = (t * std::f32::consts::PI).sin() * 60.0;
            current.bobber_pos = [
                current.cast_origin[0] + (target[0] - current.cast_origin[0]) * t,
                current.cast_origin[1] + (target[1] - current.cast_origin[1]) * t + arc,
            ];
            if t >= 1.0 {
                current.phase = Phase::Waiting;
                current.elapsed = 0.0;
                current.bite_at = 1.4 + pseudo_random(current.caught) * 2.2;
                log::info!("fishing: bobber landed, waiting {:.2}s", current.bite_at);
            }
        }
        Phase::Waiting => {
            current.bobber_pos = [
                boat_pos[0] + CAST_DISTANCE,
                WATER_LINE + (current.elapsed * 2.2).sin() * 1.5,
            ];
            if current.elapsed >= current.bite_at {
                current.phase = Phase::Biting;
                current.elapsed = 0.0;
                log::info!("fishing: bite!");
            }
        }
        Phase::Biting => {
            // Bobber sinks and shakes on bite.
            current.bobber_pos = [
                boat_pos[0] + CAST_DISTANCE,
                WATER_LINE - 4.0 + (current.elapsed * 18.0).sin() * 2.5,
            ];
            if action {
                current.phase = Phase::Reeling;
                current.reel_origin = current.bobber_pos;
                current.elapsed = 0.0;
                log::info!("fishing: hook set");
            } else if current.elapsed >= BITE_WINDOW {
                current.phase = Phase::Waiting;
                current.elapsed = 0.0;
                current.bite_at = 1.2 + pseudo_random(current.caught + 7) * 2.0;
                log::info!("fishing: missed the bite");
            }
        }
        Phase::Reeling => {
            let t = (current.elapsed / REEL_SECONDS).clamp(0.0, 1.0);
            let target = [boat_pos[0] + ROD_OFFSET[0], boat_pos[1] + ROD_OFFSET[1]];
            // Pure function of `t` (smoothstep), so a given elapsed time yields the
            // same point whatever the frame rate is (see #1059).
            let ease = t * t * (3.0 - 2.0 * t);
            current.bobber_pos = [
                current.reel_origin[0] + (target[0] - current.reel_origin[0]) * ease,
                current.reel_origin[1] + (target[1] - current.reel_origin[1]) * ease,
            ];
            if t >= 1.0 {
                current.caught += 1;
                current.phase = Phase::Caught;
                current.elapsed = 0.0;
                log::info!("fishing: landed fish #{}", current.caught);
                // Splash sound on hook (#987): both player and editor Play can hear.
                match world.play_sound("audio/splash.wav", 0.8, false) {
                    Some(id) => log::info!("fishing: splash sound id {id}"),
                    None => log::warn!("fishing: splash sound unavailable"),
                }
                // #984: save immediately on catch, quitting game will not lose progress.
                let saved = world.save_data("caught", &current.caught.to_string());
                log::info!("fishing: landed fish #{} (saved: {saved})", current.caught);
            }
        }
        Phase::Caught => {
            if current.elapsed >= 0.8 {
                current.phase = Phase::Idle;
                current.elapsed = 0.0;
                current.bobber_pos = [boat_pos[0] + ROD_OFFSET[0], boat_pos[1] + ROD_OFFSET[1]];
            }
        }
    }

    // --- HUD: fish count + current time -------------------------------------------------
    //
    // Time from engine `TimeOfDay` (#983), project no longer maintains own timer.
    if let Some(hud) = current.hud {
        if let Some(mut text) = world.get::<Text>(hud) {
            let clock = world
                .query::<TimeOfDay>()
                .into_iter()
                .find_map(|entity| world.get::<TimeOfDay>(entity))
                .map(|clock| clock.clock_text())
                .unwrap_or_else(|| "--:--".to_string());
            let content = format!("CATCH LOG   x{}   {}", current.caught, clock);
            if text.content != content {
                text.content = content;
                world.set(hud, text);
            }
        }
    }

    // --- Write back bobber position and appearance ---
    if let Some(bobber) = current.bobber {
        if let Some(mut transform) = world.get::<LocalTransform>(bobber) {
            transform.position = glam::Vec3::new(current.bobber_pos[0], current.bobber_pos[1], 0.0);
            world.set(bobber, transform);
        }
        if let Some(mut sprite) = world.get::<SpriteRenderer>(bobber) {
            // Flash warm color on bite so player sees window.
            sprite.color = if current.phase == Phase::Biting {
                [1.0, 0.55, 0.35, 1.0]
            } else {
                [1.0, 1.0, 1.0, 1.0]
            };
            sprite.sorting_layer = if current.phase == Phase::Idle {
                -100
            } else {
                60
            };
            world.set(bobber, sprite);
        }
    }

    // --- Fishing line: rod tip → bobber (sags slightly in middle) ---
    if let Some(line) = current.line {
        if let Some(mut renderer) = world.get::<LineRenderer>(line) {
            let tip = [boat_pos[0] + ROD_OFFSET[0], boat_pos[1] + ROD_OFFSET[1]];
            let visible = current.phase != Phase::Idle && current.phase != Phase::Caught;
            renderer.points = if visible {
                let mid = [
                    (tip[0] + current.bobber_pos[0]) * 0.5,
                    (tip[1] + current.bobber_pos[1]) * 0.5 - 6.0,
                    0.0,
                ];
                vec![
                    [tip[0], tip[1], 0.0],
                    mid,
                    [current.bobber_pos[0], current.bobber_pos[1], 0.0],
                ]
            } else {
                Vec::new()
            };
            world.set(line, renderer);
        }
    }
}

fn spawn_line(world: &mut GameWorld<'_>) -> Option<EntityHandle> {
    let entity = world.spawn();
    world.set(entity, Name("fishing-line".to_string()));
    world.set(entity, LocalTransform::default());
    world.set(
        entity,
        LineRenderer {
            points: Vec::new(),
            color: [0.92, 0.95, 1.0, 0.85],
            width: 0.6,
            sorting_layer: LINE_LAYER,
            order_in_layer: 0,
            ..LineRenderer::default()
        },
    );
    Some(entity)
}

/// Stable pseudo-random (0..1), avoid pulling in rand dependency for a demo.
fn pseudo_random(seed: u32) -> f32 {
    let mut x = seed.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    x ^= x >> 15;
    x = x.wrapping_mul(2_246_822_519);
    x ^= x >> 13;
    (x & 0xFFFF) as f32 / 65535.0
}
