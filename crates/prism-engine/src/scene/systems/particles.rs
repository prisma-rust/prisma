//! World-space CPU particle simulation (#501 Phase 1).
//!
//! Data side only has author component [`ParticleSystem`]; live particles stored in runtime component
//! [`ParticleEmitter`], **not in** `.scene.json` / RSCN (same convention as playhead, `SpriteRef`).
//! convention).
//!
//! Design tradeoffs:
//!
//! - **CPU simulation + instanced billboard**: first version consistent with ECS / scene format,
//!   GPU (compute + indirect draw) left for Phase 2;
//! - Particles integrate in **world space**, emitter movement does not drag already-flying particles; initial velocity
//!   direction computed from emitter current rotation;
//! - At spawn picks a random value from each `start_speed` / `start_size` / `lifetime` range;
//!   color / size / velocity offsets sampled from author curves by age (computed at extraction or integration, not stored
//!   as curve state in simulation, so editing author data takes effect immediately);
//! - Optional collision (#501): with `collision = true`, particles pushed out by own radius from entity surfaces with
//!   [`Collider`] (box / sphere / capsule) and [`TerrainData`] height field,
//!   normal reflected by `collision_bounce`, tangential damped by `collision_friction`;
//! - Random source is per-emitter independent `xorshift32` (seed = entity id), so same scene
//!   loads with consistent particle distribution — lighter dependency than `rand`, also unit-test reproducible;
//! - `dt` clamped to 0.1s: editor stutter will not teleport particles across whole scene.
//!
//! System runs during `Engine::update` scheduling, before `hierarchy_system` (alongside keyframe animation
//! system), so same-frame extraction sees already-advanced particles.

use prism_ecs::{Entity, World};

use crate::physics::WorldShape;
use crate::scene::components::{Collider, LocalToWorld, ParticleShape, ParticleSystem};
use crate::scene::systems::hierarchy::is_active_in_hierarchy;
use crate::scene::systems::terrain::{Heightfield, TerrainData};
use std::sync::Arc;

/// Max simulation step per frame (seconds). Larger `dt` clamped to avoid post-stutter teleport.
pub const MAX_STEP: f32 = 0.1;

/// Single live particle (world space).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Particle {
    /// World-space position.
    pub position: glam::Vec3,
    /// World-space velocity (units / sec).
    pub velocity: glam::Vec3,
    /// Time already alive (seconds).
    pub age: f32,
    /// Total lifetime (seconds); recycled when `age >= lifetime`.
    pub lifetime: f32,
    /// Radius in world units.
    pub size: f32,
}

/// Runtime state of an emitter (live particles + emission timer + random source).
///
/// Reset on every scene load; not in author format.
#[derive(Debug, Default, Clone)]
pub struct ParticleEmitter {
    /// Live particles (capacity reused, not rebuilt each frame).
    pub particles: Vec<Particle>,
    /// Emission accumulator (keeps fraction when not enough for one particle).
    pub accumulator: f32,
    /// Time within current emission loop (seconds).
    pub cycle_time: f32,
    /// Whether `burst` of this loop already emitted.
    pub emitted_burst: bool,
    /// `xorshift32` state; 0 means not yet seeded.
    pub rng: u32,
}

impl ParticleEmitter {
    /// Seed with entity id: same scene loads with consistent particle distribution.
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
    // 0 is illegal xorshift state, here force non-zero.
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
    ParticleSystem::random_range(range, t, fallback)
}

fn sanitize_vec3(v: [f32; 3]) -> glam::Vec3 {
    glam::Vec3::new(
        if v[0].is_finite() { v[0] } else { 0.0 },
        if v[1].is_finite() { v[1] } else { 0.0 },
        if v[2].is_finite() { v[2] } else { 0.0 },
    )
}

fn random_unit(rng: &mut u32) -> glam::Vec3 {
    // Rejection sample: normalize random point in sphere; hit rate ~52%, a few retries enough.
    for _ in 0..4 {
        let x = next_unit(rng) * 2.0 - 1.0;
        let y = next_unit(rng) * 2.0 - 1.0;
        let z = next_unit(rng) * 2.0 - 1.0;
        let v = glam::Vec3::new(x, y, z);
        let len_sq = v.length_squared();
        if len_sq > 1e-4 && len_sq <= 1.0 {
            return v / len_sq.sqrt();
        }
    }
    glam::Vec3::Y
}

/// Spawn offset and initial velocity direction in emitter local space.
fn spawn_local(
    shape: ParticleShape,
    radius: f32,
    cone_degrees: f32,
    rng: &mut u32,
) -> (glam::Vec3, glam::Vec3) {
    let radius = if radius.is_finite() && radius > 0.0 {
        radius
    } else {
        0.0
    };
    match shape {
        ParticleShape::Point => (glam::Vec3::ZERO, random_unit(rng)),
        ParticleShape::Sphere => {
            let dir = random_unit(rng);
            (dir * radius * next_unit(rng).sqrt(), dir)
        }
        ParticleShape::Box => {
            let offset = glam::Vec3::new(
                (next_unit(rng) * 2.0 - 1.0) * radius,
                (next_unit(rng) * 2.0 - 1.0) * radius,
                (next_unit(rng) * 2.0 - 1.0) * radius,
            );
            (offset, random_unit(rng))
        }
        ParticleShape::Cone => {
            // Cone around emitter local +Y: first pick point on sphere surface, then compress
            // polar angle to [0, cone] by half-angle.
            let cos_max = cone_degrees.clamp(0.0, 180.0).to_radians().cos();
            let z = 1.0 - next_unit(rng) * (1.0 - cos_max);
            let phi = next_unit(rng) * std::f32::consts::TAU;
            let sin_theta = (1.0 - z * z).max(0.0).sqrt();
            let dir = glam::Vec3::new(sin_theta * phi.cos(), z, sin_theta * phi.sin());
            let offset = dir * radius * next_unit(rng).sqrt();
            (offset, dir)
        }
    }
}

fn spawn_particle(
    system: &ParticleSystem,
    emitter: &mut ParticleEmitter,
    origin: glam::Vec3,
    rotation: glam::Quat,
) {
    let (offset, direction) = spawn_local(
        system.shape,
        system.shape_radius,
        system.cone_angle_degrees,
        &mut emitter.rng,
    );
    let speed = range(system.start_speed, next_unit(&mut emitter.rng), 1.0).max(0.0);
    let size = range(system.start_size, next_unit(&mut emitter.rng), 0.2).max(0.0);
    let lifetime = range(system.lifetime, next_unit(&mut emitter.rng), 1.0).max(0.0);
    emitter.particles.push(Particle {
        position: origin + rotation * offset,
        velocity: rotation * direction * speed,
        age: 0.0,
        lifetime,
        size,
    });
}

/// Advance all `ParticleSystem` entities: integrate existing particles, emit new per emission rule.
///
/// Return total live particles at frame end (for editor / tests).
pub fn particle_system(world: &mut World, dt: f32) -> usize {
    if !dt.is_finite() || dt < 0.0 {
        return 0;
    }
    let dt = dt.min(MAX_STEP);

    let entities: Vec<Entity> = world
        .query::<ParticleSystem>()
        .filter(|(entity, _)| is_active_in_hierarchy(world, *entity))
        .map(|(entity, _)| entity)
        .collect();

    // #501: collision shapes snapshot once per frame; can still accept this overhead even with no emitter collision
    // (usually scene has only a few colliders).
    let colliders: Vec<WorldShape> = world
        .query::<Collider>()
        .filter_map(|(entity, collider)| {
            world
                .get::<LocalToWorld>(entity)
                .map(|transform| WorldShape::new(collider, &transform.0))
        })
        .collect();
    // #501: terrain height field uses same per-frame snapshot; `TerrainData` written by terrain runtime system.
    let terrains = TerrainShape::from_world(world);

    let mut alive = 0usize;
    for entity in entities {
        // Author component may have just been deleted; copy first to avoid borrow conflict.
        let Some(system) = world.get::<ParticleSystem>(entity).cloned() else {
            continue;
        };
        let transform = world
            .get::<LocalToWorld>(entity)
            .map(|t| t.0)
            .unwrap_or(glam::Mat4::IDENTITY);
        let (scale, rotation, translation) = transform.to_scale_rotation_translation();
        // Invalid matrix (NaN / zero scale) treated as identity transform, don't shoot particles to infinity.
        let rotation = if rotation.is_finite() {
            rotation.normalize()
        } else {
            glam::Quat::IDENTITY
        };
        let origin = if translation.is_finite() {
            translation
        } else {
            glam::Vec3::ZERO
        };
        let _ = scale;

        if world.get::<ParticleEmitter>(entity).is_none() {
            world.insert(entity, ParticleEmitter::seeded(entity));
        }
        let Some(mut emitter) = world.remove::<ParticleEmitter>(entity) else {
            continue;
        };

        step(
            &system,
            &mut emitter,
            origin,
            rotation,
            dt,
            &colliders,
            &terrains,
        );
        alive += emitter.particles.len();
        world.insert(entity, emitter);
    }
    alive
}

/// One frame snapshot of a terrain height field (#501). `TerrainData` already parsed `.raw`,
/// here only keeps `Arc` and world transform needed for sampling, avoiding re-reading ECS inside particle loop.
#[derive(Clone)]
struct TerrainShape {
    origin: glam::Vec3,
    half: glam::Vec2,
    field: Arc<Heightfield>,
    height_scale: f32,
}

impl TerrainShape {
    fn from_world(world: &World) -> Vec<Self> {
        world
            .query::<TerrainData>()
            .filter_map(|(entity, data)| {
                let half = glam::Vec2::new(data.size[0] * 0.5, data.size[1] * 0.5);
                if !half.is_finite() || half.x <= 0.0 || half.y <= 0.0 {
                    return None;
                }
                let origin = world
                    .get::<LocalToWorld>(entity)
                    .map(|transform| transform.0.w_axis.truncate())
                    .unwrap_or(glam::Vec3::ZERO);
                Some(Self {
                    origin,
                    half,
                    field: data.field.clone(),
                    height_scale: data.height_scale,
                })
            })
            .collect()
    }

    /// When particle within terrain coverage and below surface, push out along height field normal to above surface.
    /// Only handles horizontal coverage (consistent with `terrain_sample`, no rotated terrain).
    fn push_out(&self, point: glam::Vec3, radius: f32) -> Option<(glam::Vec3, glam::Vec3)> {
        let local = glam::Vec2::new(point.x - self.origin.x, point.z - self.origin.z);
        if local.x.abs() > self.half.x || local.y.abs() > self.half.y {
            return None;
        }
        let u = local.x / (self.half.x * 2.0) + 0.5;
        let v = local.y / (self.half.y * 2.0) + 0.5;
        let height = self.field.sample_height(u, v) * self.height_scale + self.origin.y;
        if point.y - radius >= height {
            return None;
        }
        let normal = glam::Vec3::from_array(self.field.sample_normal(u, v, self.height_scale));
        Some((glam::Vec3::new(point.x, height + radius, point.z), normal))
    }
}

fn apply_contact(
    particle: &mut Particle,
    position: glam::Vec3,
    normal: glam::Vec3,
    bounce: f32,
    friction: f32,
) {
    particle.position = position;
    let normal_speed = particle.velocity.dot(normal);
    if normal_speed < 0.0 {
        let tangential = particle.velocity - normal * normal_speed;
        particle.velocity = tangential * (1.0 - friction) - normal * normal_speed * bounce;
    }
}

/// Push one particle out of all intersecting colliders and handle bounce / friction by normal / tangent.
///
/// Multiple pushes are necessary: corners / grooves can touch two shapes at once; each push uses new
/// position to continue testing until all shapes satisfied (few shapes, no spatial index).
fn collide_particle(
    system: &ParticleSystem,
    particle: &mut Particle,
    colliders: &[WorldShape],
    terrains: &[TerrainShape],
) {
    let bounce = if system.collision_bounce.is_finite() {
        system.collision_bounce.clamp(0.0, 1.0)
    } else {
        0.3
    };
    let friction = if system.collision_friction.is_finite() {
        system.collision_friction.clamp(0.0, 1.0)
    } else {
        0.2
    };
    let radius = particle.size.max(0.0);
    for shape in colliders {
        let Some((position, normal)) = shape.push_out(particle.position, radius) else {
            continue;
        };
        apply_contact(particle, position, normal, bounce, friction);
    }
    for terrain in terrains {
        let Some((position, normal)) = terrain.push_out(particle.position, radius) else {
            continue;
        };
        apply_contact(particle, position, normal, bounce, friction);
    }
}

fn step(
    system: &ParticleSystem,
    emitter: &mut ParticleEmitter,
    origin: glam::Vec3,
    rotation: glam::Quat,
    dt: f32,
    colliders: &[WorldShape],
    terrains: &[TerrainShape],
) {
    if !system.playing {
        // Paused: particles stay in place, emission count not accumulated.
        return;
    }

    // 1. Integrate + recycle.
    //    Drag handled as "decay drag fraction per second": `(1 - drag)^dt`, frame-rate independent.
    let drag = if system.drag.is_finite() {
        system.drag.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let damping = (1.0 - drag).max(0.0).powf(dt.max(0.0));
    let gravity = sanitize_vec3(system.gravity);
    for particle in &mut emitter.particles {
        particle.age += dt;
        particle.velocity += gravity * dt;
        particle.velocity *= damping;
        // Speed curve only changes **displacement**, not writes back `velocity`: otherwise it would stack with gravity / drag,
        // turning "wind speed" into permanent acceleration.
        let drift = particle_velocity_offset(system, particle);
        particle.position += (particle.velocity + drift) * dt;
        if system.collision && (!colliders.is_empty() || !terrains.is_empty()) {
            collide_particle(system, particle, colliders, terrains);
        }
    }
    emitter.particles.retain(|p| p.age < p.lifetime);

    // 2. Emission window (duration / looping). `duration == 0` = always emit.
    //
    // First emit based on window state at "step start", then advance `cycle_time`: this way emitter with
    // `duration = 0.2` can fill the final small segment, rather than stopping early on the frame
    // that happens to hit the endpoint.
    let duration = system.emission_duration();
    let emitting = match duration {
        None => true,
        Some(duration) => system.looping || emitter.cycle_time < duration,
    };

    let cap = system.particle_cap();

    // 3. Burst: emit all at once at start of each loop.
    if !emitter.emitted_burst {
        let burst = system.burst.min(cap as u32);
        for _ in 0..burst {
            if emitter.particles.len() >= cap {
                break;
            }
            spawn_particle(system, emitter, origin, rotation);
        }
        emitter.emitted_burst = true;
    }

    // 4. Continuous emission.
    if emitting {
        emitter.accumulator += system.rate() * dt;
        while emitter.accumulator >= 1.0 {
            emitter.accumulator -= 1.0;
            if emitter.particles.len() >= cap {
                emitter.accumulator = 0.0;
                break;
            }
            spawn_particle(system, emitter, origin, rotation);
        }
    } else {
        emitter.accumulator = 0.0;
    }

    if emitter.particles.len() > cap {
        emitter.particles.truncate(cap);
    }

    // 5. Advance emission window: loop back to 0 at endpoint (re-count burst) or stop at endpoint.
    if let Some(duration) = duration {
        emitter.cycle_time += dt;
        if emitter.cycle_time >= duration {
            if system.looping {
                emitter.cycle_time = emitter.cycle_time.rem_euclid(duration);
                emitter.emitted_burst = false;
            } else {
                emitter.cycle_time = duration;
            }
        }
    }
}

/// Particle progress in lifetime (`0` = spawn, `1` = dead).
///
/// Zero / non-finite lifetime treated as `1` (already dead), avoid division by zero NaN.
fn age_fraction(particle: &Particle) -> f32 {
    if particle.lifetime.is_finite() && particle.lifetime > 0.0 {
        (particle.age / particle.lifetime).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// Sample from multi-stop color gradient; returns `None` when `stops` empty (caller falls back to two-color lerp).
///
/// Stops uniformly distributed on `[0, 1]`, linear interpolation between adjacent; `t` out of range clamped to endpoint.
/// With one stop entire lifetime is that color.
pub fn sample_gradient(stops: &[[f32; 4]], t: f32) -> Option<[f32; 4]> {
    match stops.len() {
        0 => None,
        1 => Some(stops[0]),
        len => {
            let t = if t.is_finite() {
                t.clamp(0.0, 1.0)
            } else {
                1.0
            };
            let scaled = t * (len - 1) as f32;
            let index = (scaled.floor() as usize).min(len - 2);
            let local = scaled - index as f32;
            let a = stops[index];
            let b = stops[index + 1];
            let lerp = |a: f32, b: f32| a + (b - a) * local;
            Some([
                lerp(a[0], b[0]),
                lerp(a[1], b[1]),
                lerp(a[2], b[2]),
                lerp(a[3], b[3]),
            ])
        }
    }
}

/// Particle current color.
///
/// When `color_stops` non-empty samples from multi-stop gradient over lifetime; otherwise falls back to
/// `start_color → end_color` two-color lerp (legacy).
pub fn particle_color(system: &ParticleSystem, particle: &Particle) -> [f32; 4] {
    let t = age_fraction(particle);
    if let Some(color) = sample_gradient(&system.color_stops, t) {
        return color;
    }
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    [
        lerp(system.start_color[0], system.end_color[0]),
        lerp(system.start_color[1], system.end_color[1]),
        lerp(system.start_color[2], system.end_color[2]),
        lerp(system.start_color[3], system.end_color[3]),
    ]
}

/// Particle current size: spawn size × `size_over_lifetime` interpolated over lifetime.
///
/// Non-finite multiplier treated as `1`; result clamped non-negative, avoid billboard size negative / NaN.
pub fn particle_size(system: &ParticleSystem, particle: &Particle) -> f32 {
    let t = age_fraction(particle);
    let start = system.size_over_lifetime[0];
    let end = system.size_over_lifetime[1];
    let start = if start.is_finite() { start } else { 1.0 };
    let end = if end.is_finite() { end } else { 1.0 };
    let scale = start + (end - start) * t;
    (particle.size * scale).max(0.0)
}

/// Particle current world-space velocity offset (`velocity_over_lifetime` lerp over lifetime).
///
/// Non-finite value as 0; result only used for displacement, not written back [`Particle::velocity`].
pub fn particle_velocity_offset(system: &ParticleSystem, particle: &Particle) -> glam::Vec3 {
    let start = sanitize_vec3(system.velocity_over_lifetime[0]);
    let end = sanitize_vec3(system.velocity_over_lifetime[1]);
    let t = age_fraction(particle);
    start + (end - start) * t
}

/// Extract all visible emitter particles as billboard draw items, sorted **far to near** by camera distance.
///
/// Read-only; does not modify simulation state. Orphan `ParticleEmitter` without `ParticleSystem`
/// (author deleted component) skipped.
pub fn scene_particles_into(
    world: &World,
    eye: glam::Vec3,
    out: &mut Vec<prism_render::ParticleDrawItem>,
) {
    out.clear();
    for (entity, emitter) in world.query::<ParticleEmitter>() {
        let Some(system) = world.get::<ParticleSystem>(entity) else {
            continue;
        };
        if !is_active_in_hierarchy(world, entity) {
            continue;
        }
        out.extend(
            emitter
                .particles
                .iter()
                .map(|particle| prism_render::ParticleDrawItem {
                    center: particle.position.to_array(),
                    size: particle_size(system, particle),
                    color: particle_color(system, particle),
                }),
        );
    }

    out.sort_by(|a, b| {
        let da = glam::Vec3::from_array(a.center).distance_squared(eye);
        let db = glam::Vec3::from_array(b.center).distance_squared(eye);
        db.total_cmp(&da)
    });
}

#[cfg(test)]
#[path = "particle_tests.rs"]
mod particle_tests;
