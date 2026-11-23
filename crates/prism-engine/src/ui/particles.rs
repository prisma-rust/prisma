//! 2D screen-space particle system — generic simulator for rain, snow, sparks and other 2D effects.
//!
//! > World-space 3D particles (billboards, depth test, `ParticleSystem` scene component) are not here,
//! > see `crate::scene::systems::particles` (#501 Phase 1).
//!
//! Particles are simulated in **pixel screen space** (origin top-left, y downward, consistent with `ComputedLayout::rect`)
//! and output as `UiQuad` (plain colored quads). The engine's existing
//! `UiOverlay` overlay pass draws these quads directly — **no new / modified shader needed**.
//!
//! Raindrops are essentially many thin, low-`alpha` vertical quads; this module packs them into
//! `UiDrawList::quads`, which `convert_ui_draw_list_to_overlay` automatically
//! converts to NDC during frame extraction for the overlay pass. Overlay pass submission order: first all quads (sorted by
//! `layer`) then all text, so setting rain's `layer` to 1 puts rain above the black backdrop
//! (layer 0) and below title text.
//!
//! In scenes just attach `prism_engine::scene::UiParticleEmitter` component: engine holds one
//! `ParticleSystem2D` per emitter entity, advances each frame and merges quads into UI draw list (#1057),
//! project need not write simulation code.
//!
//! Below is how the host drives the resource itself, only needed for full control over emission params (e.g. changing
//! `intensity` along an animation curve):
//!
//! ```ignore
//! // Insert resource at construction (rain preset, size updated each frame from ScreenSize).
//! app.insert_resource(ParticleSystem2D::rain_preset(1920.0, 1080.0));
//!
//! // Each frame: update simulation and use intensity to control overall opacity, append particles into UiDrawList.
//! let quads = {
//!     let rain = world.get_resource_mut::<ParticleSystem2D>().unwrap();
//!     rain.set_bounds(w, h);
//!     rain.intensity = title_alpha; // fade with title
//!     rain.update(dt);
//!     world.get_resource::<ParticleSystem2D>().unwrap().emit_quads()
//! };
//! world.get_resource_mut::<UiDrawList>().unwrap().quads.extend(quads);
//! ```

use crate::scene::components::{UiParticleEmitter, UiParticleKind};

/// One screen-space particle (pixel coords, y downward).
#[derive(Clone, Copy, Debug)]
pub struct Particle2D {
    /// Current position (px).
    pub x: f32,
    /// Current position (px, y downward, so vy > 0 when falling).
    pub y: f32,
    /// Horizontal velocity (px/s).
    pub vx: f32,
    /// Vertical velocity (px/s, positive when falling).
    pub vy: f32,
    /// Time already alive (s).
    pub age: f32,
    /// Lifetime (s), recycled when exceeded.
    pub life: f32,
    /// Trail length (px) — raindrops are thin bars, snow / sparks use 0.
    pub len: f32,
    /// Thickness (px).
    pub width: f32,
    /// Base opacity 0..=1 (final alpha also modulated by system `intensity` and fade).
    pub alpha: f32,
}

/// Particle emission shape — affects initial velocity and trail orientation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpawnKind {
    /// Rain: fast downward fall + slight slanted wind, with vertical trail.
    #[default]
    Rain,
    /// Snow: slow drifting fall + sine sway, no trail.
    Snow,
    /// Sparks: random scatter + gravity, no trail.
    Spark,
}

/// One 2D screen-space particle system (stored as ECS resource).
///
/// Config fields all `pub`, caller (e.g. opening animation) can adjust `intensity`,
/// `color`, `spawn_rate` etc. to control appearance. `update` advances simulation, `emit_quads`
/// outputs the `UiQuad` list to draw.
#[derive(Debug)]
pub struct ParticleSystem2D {
    /// Live particle pool (unbounded but `MAX_PARTICLES` soft cap prevents runaway).
    pub particles: Vec<Particle2D>,
    /// Emission rate (particles/sec). Actual count multiplied by `intensity`.
    pub spawn_rate: f32,
    /// Gravity acceleration (px/s², positive downward).
    pub gravity: f32,
    /// Horizontal wind offset (px/s², positive rightward), decides rain slant.
    pub wind: f32,
    /// Initial vertical velocity range (px/s).
    pub speed_min: f32,
    pub speed_max: f32,
    /// Trail length range (px).
    pub len_min: f32,
    pub len_max: f32,
    /// Thickness range (px).
    pub width_min: f32,
    pub width_max: f32,
    /// Color (RGB, 0..=1).
    pub color: [f32; 3],
    /// Base opacity range 0..=1.
    pub alpha_min: f32,
    pub alpha_max: f32,
    /// Spawn y range `[-spawn_top, 0]` (a point above screen, smooth entry).
    pub spawn_top: f32,
    /// Overall strength 0..=1: modulates emission rate and final alpha. Set 0 to fully stop rain.
    pub intensity: f32,
    /// Emission shape.
    pub kind: SpawnKind,
    /// Current screen width (px), updated each frame via `set_bounds`.
    pub width: f32,
    /// Current screen height (px), updated each frame via `set_bounds`.
    pub height: f32,
    /// Soft cap: when particle count exceeds this, pause emission (protect vertex buffer and performance).
    pub max_particles: usize,
    /// UI layer level of output quads (`UiQuad::layer`).
    pub layer: i32,
    /// Snow's horizontal sway amplitude (px/s), only for `Snow`.
    pub sway: f32,
    rng: u64,
    spawn_accum: f32,
}

impl Default for ParticleSystem2D {
    fn default() -> Self {
        Self::new()
    }
}

impl ParticleSystem2D {
    /// Construct a generic particle system (default rain shape). Appearance configured by caller.
    pub fn new() -> Self {
        Self::rain_preset(1920.0, 1080.0)
    }

    /// Rain preset — uniformly dense slanted rain covering screen, cool blue-white.
    ///
    /// `width`/`height` are only initial screen size; at runtime call `set_bounds` with
    /// `ScreenSize` to update in real time.
    pub fn rain_preset(width: f32, height: f32) -> Self {
        let area_scale = (width / 1920.0).clamp(0.5, 2.0);
        Self {
            particles: Vec::with_capacity(1024),
            spawn_rate: 420.0 * area_scale,
            gravity: 240.0,
            wind: 90.0,
            speed_min: 900.0,
            speed_max: 1500.0,
            len_min: 16.0,
            len_max: 42.0,
            width_min: 1.0,
            width_max: 2.4,
            color: [0.72, 0.82, 0.96],
            alpha_min: 0.16,
            alpha_max: 0.42,
            spawn_top: 70.0,
            intensity: 1.0,
            kind: SpawnKind::Rain,
            width,
            height,
            max_particles: (900.0 * area_scale) as usize,
            layer: 1,
            sway: 0.0,
            rng: 0x9E37_79B9_7F4A_7C15,
            spawn_accum: 0.0,
        }
    }

    /// Snow preset — slow drifting fall, swaying, no trail.
    pub fn snow_preset(width: f32, height: f32) -> Self {
        let area_scale = (width / 1920.0).clamp(0.5, 2.0);
        Self {
            particles: Vec::with_capacity(1024),
            spawn_rate: 120.0 * area_scale,
            gravity: 60.0,
            wind: 0.0,
            speed_min: 40.0,
            speed_max: 110.0,
            len_min: 0.0,
            len_max: 0.0,
            width_min: 1.5,
            width_max: 3.5,
            color: [0.95, 0.97, 1.0],
            alpha_min: 0.5,
            alpha_max: 0.9,
            spawn_top: 40.0,
            intensity: 1.0,
            kind: SpawnKind::Snow,
            width,
            height,
            max_particles: (400.0 * area_scale) as usize,
            layer: 1,
            sway: 40.0,
            rng: 0x1B87_3D5E_9C2A_4F11,
            spawn_accum: 0.0,
        }
    }

    /// Update current screen size (usually called each frame after reading `ScreenSize` resource).
    pub fn set_bounds(&mut self, width: f32, height: f32) {
        self.width = width;
        self.height = height;
    }

    /// Deterministic seed for the spawn stream.
    ///
    /// Emitters without an explicit seed all share the preset constant, so two
    /// instances would look identical; the engine seeds each entity from its id.
    pub fn set_seed(&mut self, seed: u64) {
        self.rng = if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        };
    }

    /// Copy the author knobs from a [`UiParticleEmitter`] component.
    ///
    /// Live particles are kept, so this is safe to call every tick: editing
    /// color / rate / layer in the editor takes effect immediately instead of
    /// restarting the emitter. `max_particles == 0` keeps the preset limit.
    pub fn apply_emitter(&mut self, emitter: &UiParticleEmitter) {
        self.kind = match emitter.kind {
            UiParticleKind::Rain => SpawnKind::Rain,
            UiParticleKind::Snow => SpawnKind::Snow,
            UiParticleKind::Spark => SpawnKind::Spark,
        };
        self.spawn_rate = emitter.rate();
        self.gravity = finite_or(emitter.gravity, 0.0);
        self.wind = finite_or(emitter.wind, 0.0);
        let (speed_min, speed_max) = sorted_range(emitter.speed);
        self.speed_min = speed_min.max(0.0);
        self.speed_max = speed_max.max(0.0);
        let (len_min, len_max) = sorted_range(emitter.length);
        self.len_min = len_min.max(0.0);
        self.len_max = len_max.max(0.0);
        let (width_min, width_max) = sorted_range(emitter.width);
        self.width_min = width_min.max(0.0);
        self.width_max = width_max.max(0.0);
        let (alpha_min, alpha_max) = sorted_range(emitter.alpha);
        self.alpha_min = alpha_min.clamp(0.0, 1.0);
        self.alpha_max = alpha_max.clamp(0.0, 1.0);
        self.color = [
            finite_or(emitter.color[0], 0.0).clamp(0.0, 1.0),
            finite_or(emitter.color[1], 0.0).clamp(0.0, 1.0),
            finite_or(emitter.color[2], 0.0).clamp(0.0, 1.0),
        ];
        self.layer = emitter.layer;
        self.sway = if self.kind == SpawnKind::Snow {
            40.0
        } else {
            0.0
        };
        if emitter.max_particles > 0 {
            self.max_particles = emitter.max_particles as usize;
        }
    }

    /// Advance simulation `dt` seconds each frame: emit new particles by `intensity`, integrate motion and recycle.
    pub fn update(&mut self, dt: f32) {
        self.spawn_accum += self.spawn_rate * self.intensity * dt;
        let mut to_spawn = self.spawn_accum.floor() as i32;
        self.spawn_accum -= to_spawn as f32;
        while to_spawn > 0 && self.particles.len() < self.max_particles {
            self.spawn_one();
            to_spawn -= 1;
        }

        let h = self.height;
        let w = self.width;
        let gravity = self.gravity;
        let kind = self.kind;
        let sway = self.sway;
        self.particles.retain_mut(|p| {
            p.vy += gravity * dt;
            if kind == SpawnKind::Snow {
                p.vx = (p.age * 2.2).sin() * sway;
            }
            p.x += p.vx * dt;
            p.y += p.vy * dt;
            p.age += dt;
            p.age < p.life && p.y < h + 60.0 && p.x > -60.0 && p.x < w + 60.0
        });
    }

    /// Emit one particle matching current shape config.
    fn spawn_one(&mut self) {
        let w = self.width;
        let x = Self::rand(&mut self.rng) * w;
        let y = -self.spawn_top * Self::rand(&mut self.rng);
        let life = match self.kind {
            SpawnKind::Rain => 6.0,
            SpawnKind::Snow => 12.0,
            SpawnKind::Spark => 1.2,
        };
        let speed = lerp(self.speed_min, self.speed_max, Self::rand(&mut self.rng));
        let (vx, len) = match self.kind {
            SpawnKind::Rain => (
                self.wind * (0.4 + 0.6 * Self::rand(&mut self.rng)),
                lerp(self.len_min, self.len_max, Self::rand(&mut self.rng)),
            ),
            SpawnKind::Snow => (0.0, 0.0),
            SpawnKind::Spark => {
                let ang = Self::rand(&mut self.rng) * std::f32::consts::TAU;
                let sp = lerp(self.speed_min, self.speed_max, Self::rand(&mut self.rng));
                (ang.cos() * sp, 0.0)
            }
        };
        let width = lerp(self.width_min, self.width_max, Self::rand(&mut self.rng));
        let alpha = lerp(self.alpha_min, self.alpha_max, Self::rand(&mut self.rng));
        self.particles.push(Particle2D {
            x,
            y,
            vx,
            vy: speed,
            age: 0.0,
            life,
            len,
            width,
            alpha,
        });
    }

    /// Emit draw-packet quads for all live particles (pixel-space rects merged
    /// into `UiDrawList` via `ExternalUiQuads`), drawn above backdrop and below text.
    pub fn emit_quads(&self) -> Vec<prism_ui_draw::UiQuad> {
        let h = self.height;
        let mut quads = Vec::with_capacity(self.particles.len());
        for p in &self.particles {
            let fade_in = (p.age / 0.18).min(1.0);
            let bottom_fade = if p.y > h - 50.0 {
                ((h + 50.0 - p.y) / 100.0).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let a = p.alpha * self.intensity * fade_in * bottom_fade;
            if a <= 0.002 {
                continue;
            }
            let rect = [
                p.x - p.width * 0.5,
                p.y - p.len,
                p.width,
                p.len.max(p.width),
            ];
            quads.push(prism_ui_draw::UiQuad {
                rect,
                color: [self.color[0], self.color[1], self.color[2], a],
                border_radius: p.width * 0.5,
                texture: None,
                backdrop_sigma: 0.0,
            });
        }
        quads
    }
}

impl ParticleSystem2D {
    /// xorshift64 PRNG (avoid adding rand dependency). Returns unsigned 64-bit integer.
    fn xorshift(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }

    /// Uniform pseudo-random in `[0,1)` (xorshift64, avoids rand dependency).
    fn rand(state: &mut u64) -> f32 {
        (Self::xorshift(state) >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Linear interpolation.
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

/// Sort a `[min, max]` range and replace non-finite ends with zero.
fn sorted_range(range: [f32; 2]) -> (f32, f32) {
    let a = finite_or(range[0], 0.0);
    let b = finite_or(range[1], 0.0);
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_emitter_copies_the_author_knobs() {
        let mut system = ParticleSystem2D::new();
        let emitter = UiParticleEmitter {
            kind: UiParticleKind::Snow,
            emission_rate: 12.5,
            gravity: 5.0,
            wind: -1.0,
            speed: [40.0, 10.0],
            length: [0.0, 0.0],
            width: [3.0, 1.5],
            color: [0.1, 0.2, 0.3],
            alpha: [0.9, 0.4],
            layer: 42,
            max_particles: 8,
            ..UiParticleEmitter::default()
        };

        system.apply_emitter(&emitter);

        assert_eq!(system.kind, SpawnKind::Snow);
        assert_eq!(system.spawn_rate, 12.5);
        assert_eq!(system.speed_min, 10.0, "reversed ranges are sorted");
        assert_eq!(system.speed_max, 40.0);
        assert_eq!(system.width_min, 1.5);
        assert_eq!(system.width_max, 3.0);
        assert_eq!(system.color, [0.1, 0.2, 0.3]);
        assert_eq!(system.layer, 42);
        assert_eq!(system.max_particles, 8);
        assert!(system.sway > 0.0, "snow sways");
    }

    #[test]
    fn apply_emitter_keeps_live_particles() {
        let mut system = ParticleSystem2D::rain_preset(800.0, 600.0);
        system.update(0.5);
        let before = system.particles.len();
        assert!(before > 0);

        system.apply_emitter(&UiParticleEmitter::default());

        assert_eq!(
            system.particles.len(),
            before,
            "re-applying the config must not restart the emitter"
        );
    }

    #[test]
    fn the_seed_decides_the_spawn_stream() {
        let mut first = ParticleSystem2D::rain_preset(800.0, 600.0);
        first.set_seed(7);
        let mut second = ParticleSystem2D::rain_preset(800.0, 600.0);
        second.set_seed(7);
        let mut other = ParticleSystem2D::rain_preset(800.0, 600.0);
        other.set_seed(8);

        first.update(0.3);
        second.update(0.3);
        other.update(0.3);

        assert_eq!(first.particles.len(), second.particles.len());
        assert_eq!(first.particles[0].x, second.particles[0].x);
        assert_ne!(first.particles[0].x, other.particles[0].x);
    }
}
