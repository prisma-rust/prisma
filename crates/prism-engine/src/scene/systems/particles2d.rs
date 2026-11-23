//! 2D world-space CPU particles (#594).
//!
//! Author side only has [`ParticleEmitter2D`]; live particles stored in runtime component
//! [`ParticleEmitter2DState`], **not in** `.scene.json` / RSCN (same as [`SpriteRef`],
//! and 3D `ParticleEmitter`).
//!
//! Design tradeoffs (consistent with 3D `particles` module):
//!
//! - **CPU simulation + sprite pass draw**: particles integrate in world XY plane, each particle is a
//!   solid-color quad, drawn as [`SpriteDrawItem`](prism_render::SpriteDrawItem) sorted together with sprites —
//!   no new pass / shader needed, unaffected by screen resolution;
//! - Emitter movement does not drag already-flying particles;
//! - Initial velocity direction from emitter current Z rotation (2D only cares about Z), spawn offset uses same
//!   rotation;
//! - Random source is per-emitter independent `xorshift32` (seed = entity id), same scene each load has
//!   consistent particle distribution, unit-test reproducible;
//! - `dt` clamp reuses 3D [`MAX_STEP`], editor stutter will not teleport particles.
//!
//! System runs during `Engine::update` scheduling, before `hierarchy_system`; extract phase
//! (`scene_2d_system_into`) reads this-frame advanced particles.

use prism_ecs::{Entity, World};

use crate::scene::components::{LocalToWorld, ParticleEmitter2D, ParticleShape2D};
use crate::scene::systems::hierarchy::is_active_in_hierarchy;
use crate::scene::systems::particles::MAX_STEP;

/// Single live particle (world XY plane).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Particle2DInstance {
    /// World-space position (XY; Z from emitter).
    pub position: glam::Vec2,
    /// World-space velocity (units / sec).
    pub velocity: glam::Vec2,
    /// Time already alive (seconds).
    pub age: f32,
    /// Total lifetime (seconds); recycled when `age >= lifetime`.
    pub lifetime: f32,
    /// Quad edge length (world units).
    pub size: f32,
}

/// Runtime state of emitter (live particles + emission timer + random source).
///
/// Reset on every scene load; not in author format.
#[derive(Debug, Default, Clone)]
pub struct ParticleEmitter2DState {
    /// Live particles (capacity reused, not rebuilt each frame).
    pub particles: Vec<Particle2DInstance>,
    /// Emission accumulator (keeps fraction when not enough for one particle).
    pub accumulator: f32,
    /// Time within current emission loop (seconds).
    pub cycle_time: f32,
    /// Whether `burst` of this loop already emitted.
    pub emitted_burst: bool,
    /// `xorshift32` state; 0 means not yet seeded.
    pub rng: u32,
}

impl ParticleEmitter2DState {
    /// Seed with entity id: same scene each load has consistent particle distribution.
    pub fn seeded(entity: Entity) -> Self {
        Self {
            particles: Vec::new(),
            accumulator: 0.0,
            cycle_time: 0.0,
            emitted_burst: false,
            rng: seed_from_entity(entity),
        }
    }
}

fn seed_from_entity(entity: Entity) -> u32 {
    entity.id().wrapping_mul(2_654_435_761).max(1)
}

/// `xorshift32` → `[0, 1)`.
fn next_unit(rng: &mut u32) -> f32 {
    let mut x = *rng;
    if x == 0 {
        x = 1;
    }
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *rng = x;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// Range value `[min, max]` (falls back to `fallback` when non-finite / reversed).
fn range(range: [f32; 2], t: f32, fallback: f32) -> f32 {
    let (lo, hi) = if range[0].is_finite() && range[1].is_finite() {
        if range[0] <= range[1] {
            (range[0], range[1])
        } else {
            (range[1], range[0])
        }
    } else {
        (fallback, fallback)
    };
    lo + (hi - lo) * t
}

/// Spawn offset (emitter local XY) and initial velocity direction.
fn spawn_local(
    shape: ParticleShape2D,
    size: [f32; 2],
    radius: f32,
    direction: [f32; 2],
    spread_degrees: f32,
    rng: &mut u32,
) -> (glam::Vec2, glam::Vec2) {
    let half_x = if size[0].is_finite() {
        size[0].abs()
    } else {
        0.0
    };
    let half_y = if size[1].is_finite() {
        size[1].abs()
    } else {
        0.0
    };
    let radius = if radius.is_finite() && radius > 0.0 {
        radius
    } else {
        0.0
    };

    let offset = match shape {
        ParticleShape2D::Point => glam::Vec2::ZERO,
        ParticleShape2D::Line => glam::Vec2::new((next_unit(rng) * 2.0 - 1.0) * half_x, 0.0),
        ParticleShape2D::Box => glam::Vec2::new(
            (next_unit(rng) * 2.0 - 1.0) * half_x,
            (next_unit(rng) * 2.0 - 1.0) * half_y,
        ),
        ParticleShape2D::Circle => {
            let angle = next_unit(rng) * std::f32::consts::TAU;
            glam::Vec2::new(angle.cos(), angle.sin()) * radius * next_unit(rng).sqrt()
        }
    };

    let half_angle = if spread_degrees.is_finite() {
        spread_degrees.clamp(0.0, 180.0).to_radians()
    } else {
        0.0
    };
    let jitter = (next_unit(rng) * 2.0 - 1.0) * half_angle;
    let (sin, cos) = jitter.sin_cos();
    let dir = glam::Vec2::new(
        direction[0] * cos - direction[1] * sin,
        direction[0] * sin + direction[1] * cos,
    );
    (offset, dir)
}

fn spawn_particle(
    emitter_def: &ParticleEmitter2D,
    state: &mut ParticleEmitter2DState,
    origin: glam::Vec2,
    z: f32,
    rotation: f32,
) {
    let (offset, direction) = spawn_local(
        emitter_def.shape,
        emitter_def.shape_size,
        emitter_def.shape_radius,
        emitter_def.direction_or_down(),
        emitter_def.spread_degrees,
        &mut state.rng,
    );
    let (sin, cos) = rotation.sin_cos();
    let world_offset = glam::Vec2::new(
        offset.x * cos - offset.y * sin,
        offset.x * sin + offset.y * cos,
    );
    let world_dir = glam::Vec2::new(
        direction.x * cos - direction.y * sin,
        direction.x * sin + direction.y * cos,
    );

    let speed = range(emitter_def.start_speed, next_unit(&mut state.rng), 1.0).max(0.0);
    let size = range(emitter_def.start_size, next_unit(&mut state.rng), 0.1).max(0.0);
    let lifetime = range(emitter_def.lifetime, next_unit(&mut state.rng), 1.0).max(0.0);
    let _ = z;
    state.particles.push(Particle2DInstance {
        position: origin + world_offset,
        velocity: world_dir * speed,
        age: 0.0,
        lifetime,
        size,
    });
}

/// Advance all `ParticleEmitter2D` entities: integrate existing particles, emit new per rule.
///
/// Return total live particles at frame end (for editor / tests).
pub fn particle_emitter_2d_system(world: &mut World, dt: f32) -> usize {
    if !dt.is_finite() || dt < 0.0 {
        return 0;
    }
    let dt = dt.min(MAX_STEP);

    let entities: Vec<Entity> = world
        .query::<ParticleEmitter2D>()
        .filter(|(entity, _)| is_active_in_hierarchy(world, *entity))
        .map(|(entity, _)| entity)
        .collect();

    let mut alive = 0usize;
    for entity in entities {
        let Some(emitter_def) = world.get::<ParticleEmitter2D>(entity).cloned() else {
            continue;
        };
        let transform = world
            .get::<LocalToWorld>(entity)
            .map(|t| t.0)
            .unwrap_or(glam::Mat4::IDENTITY);
        let (scale, rotation, translation) = transform.to_scale_rotation_translation();
        let rotation = if rotation.is_finite() {
            rotation.to_euler(glam::EulerRot::XYZ).2
        } else {
            0.0
        };
        let origin = if translation.is_finite() {
            glam::Vec2::new(translation.x, translation.y)
        } else {
            glam::Vec2::ZERO
        };
        let z = if translation.is_finite() {
            translation.z
        } else {
            0.0
        };
        let _ = scale;

        if world.get::<ParticleEmitter2DState>(entity).is_none() {
            world.insert(entity, ParticleEmitter2DState::seeded(entity));
        }
        let Some(mut state) = world.remove::<ParticleEmitter2DState>(entity) else {
            continue;
        };

        step(&emitter_def, &mut state, origin, z, rotation, dt);
        alive += state.particles.len();
        world.insert(entity, state);
    }

    let orphans: Vec<Entity> = world
        .query::<ParticleEmitter2DState>()
        .filter(|(entity, _)| world.get::<ParticleEmitter2D>(*entity).is_none())
        .map(|(entity, _)| entity)
        .collect();
    for entity in orphans {
        world.remove::<ParticleEmitter2DState>(entity);
    }

    alive
}

fn step(
    emitter_def: &ParticleEmitter2D,
    state: &mut ParticleEmitter2DState,
    origin: glam::Vec2,
    z: f32,
    rotation: f32,
    dt: f32,
) {
    if !emitter_def.playing {
        return;
    }

    let drag = if emitter_def.drag.is_finite() {
        emitter_def.drag.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let damping = (1.0 - drag).max(0.0).powf(dt.max(0.0));
    let gravity = glam::Vec2::new(
        if emitter_def.gravity[0].is_finite() {
            emitter_def.gravity[0]
        } else {
            0.0
        },
        if emitter_def.gravity[1].is_finite() {
            emitter_def.gravity[1]
        } else {
            0.0
        },
    );
    for particle in &mut state.particles {
        particle.age += dt;
        particle.velocity += gravity * dt;
        particle.velocity *= damping;
        particle.position += particle.velocity * dt;
    }
    state.particles.retain(|p| p.age < p.lifetime);

    let duration = emitter_def.emission_duration();
    let emitting = match duration {
        None => true,
        Some(duration) => emitter_def.looping || state.cycle_time < duration,
    };

    let cap = emitter_def.particle_cap();

    if !state.emitted_burst {
        let burst = emitter_def.burst.min(cap as u32);
        for _ in 0..burst {
            if state.particles.len() >= cap {
                break;
            }
            spawn_particle(emitter_def, state, origin, z, rotation);
        }
        state.emitted_burst = true;
    }

    if emitting {
        state.accumulator += emitter_def.rate() * dt;
        while state.accumulator >= 1.0 {
            state.accumulator -= 1.0;
            if state.particles.len() >= cap {
                state.accumulator = 0.0;
                break;
            }
            spawn_particle(emitter_def, state, origin, z, rotation);
        }
    } else {
        state.accumulator = 0.0;
    }

    if state.particles.len() > cap {
        state.particles.truncate(cap);
    }

    if let Some(duration) = duration {
        state.cycle_time += dt;
        if state.cycle_time >= duration {
            if emitter_def.looping {
                state.cycle_time = state.cycle_time.rem_euclid(duration);
                state.emitted_burst = false;
            } else {
                state.cycle_time = duration;
            }
        }
    }
}

/// Particle current color: `start_color → end_color` lerped by age.
pub fn particle_2d_color(
    emitter_def: &ParticleEmitter2D,
    particle: &Particle2DInstance,
) -> [f32; 4] {
    let t = if particle.lifetime > 0.0 {
        (particle.age / particle.lifetime).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    [
        lerp(emitter_def.start_color[0], emitter_def.end_color[0]),
        lerp(emitter_def.start_color[1], emitter_def.end_color[1]),
        lerp(emitter_def.start_color[2], emitter_def.end_color[2]),
        lerp(emitter_def.start_color[3], emitter_def.end_color[3]),
    ]
}

/// Sort key of 2D particles; shares `SortKey` field order with sprites / polylines.
pub(crate) type Particle2DSortKey = (i32, i32, f32, u32);

/// Extract all visible 2D emitter particles as draw items.
///
/// Read-only; does not modify simulation state. Depth from emitter own Z, consistent with sprite sort rule
/// (`sorting_layer → order_in_layer → z → entity id`).
pub(crate) fn particle_entries_2d(
    world: &World,
) -> Vec<(Particle2DSortKey, prism_render::SpriteDrawItem)> {
    let mut out = Vec::new();
    for (entity, state) in world.query::<ParticleEmitter2DState>() {
        let Some(emitter_def) = world.get::<ParticleEmitter2D>(entity) else {
            continue;
        };
        if state.particles.is_empty() || !is_active_in_hierarchy(world, entity) {
            continue;
        }
        let z = world
            .get::<LocalToWorld>(entity)
            .map(|t| t.0.w_axis.z)
            .unwrap_or(0.0);
        let key: Particle2DSortKey = (
            emitter_def.sorting_layer,
            emitter_def.order_in_layer,
            z,
            entity.id(),
        );
        for particle in &state.particles {
            let model = glam::Mat4::from_scale_rotation_translation(
                glam::vec3(particle.size, particle.size, 1.0),
                glam::Quat::IDENTITY,
                glam::vec3(particle.position.x, particle.position.y, z),
            );
            out.push((
                key,
                prism_render::SpriteDrawItem {
                    model: model.to_cols_array_2d(),
                    corners: None,
                    uv_rect: [0.0, 0.0, 1.0, 1.0],
                    color: particle_2d_color(emitter_def, particle),
                    texture: crate::scene::systems::render::SOLID_TEXTURE,
                    src_blend: prism_scene::render_enums::BlendMode::SrcAlpha,
                    dst_blend: prism_scene::render_enums::BlendMode::OneMinusSrcAlpha,
                    blend_op: prism_scene::render_enums::BlendOp::Add,
                    clip: prism_render::NO_CLIP,
                    receive_lights: false,
                },
            ));
        }
    }
    out
}

#[cfg(test)]
#[path = "particles2d_tests.rs"]
mod tests;
