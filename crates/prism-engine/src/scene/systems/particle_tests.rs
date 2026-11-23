//! `particle_system` / `scene_particles_into` tests.

use super::*;
use crate::scene::components::LocalTransform;

fn spawn_emitter(world: &mut World, system: ParticleSystem, position: [f32; 3]) -> Entity {
    let entity = world.spawn();
    world.insert(
        entity,
        LocalTransform {
            position: glam::Vec3::from_array(position),
            ..LocalTransform::default()
        },
    );
    world.insert(
        entity,
        LocalToWorld(glam::Mat4::from_translation(glam::Vec3::from_array(
            position,
        ))),
    );
    world.insert(entity, system);
    entity
}

/// Advance `seconds` by `MAX_STEP` steps, simulating a real frame loop (`particle_system`
/// itself clamps larger `dt`).
fn run(world: &mut World, seconds: f32) {
    let steps = (seconds / MAX_STEP).round().max(1.0) as usize;
    for _ in 0..steps {
        particle_system(world, MAX_STEP);
    }
}

fn particles_of(world: &World, entity: Entity) -> Vec<Particle> {
    world
        .get::<ParticleEmitter>(entity)
        .map(|emitter| emitter.particles.clone())
        .unwrap_or_default()
}

fn fire() -> ParticleSystem {
    ParticleSystem {
        emission_rate: 10.0,
        lifetime: [1.0, 1.0],
        start_speed: [0.0, 0.0],
        start_size: [0.5, 0.5],
        gravity: [0.0, 0.0, 0.0],
        drag: 0.0,
        shape: ParticleShape::Point,
        ..ParticleSystem::default()
    }
}

#[test]
fn emission_follows_the_rate_and_is_deterministic() {
    let mut world = World::new();
    let entity = spawn_emitter(&mut world, fire(), [0.0, 0.0, 0.0]);

    // 10 / seconds × 0.5 seconds = 5 particles.
    run(&mut world, 0.5);
    assert_eq!(particles_of(&world, entity).len(), 5);

    // Same seed in another world gives same positions.
    let mut other = World::new();
    let other_entity = spawn_emitter(&mut other, fire(), [0.0, 0.0, 0.0]);
    run(&mut other, 0.5);
    let a = particles_of(&world, entity);
    let b = particles_of(&other, other_entity);
    assert_eq!(a.len(), b.len());
    for (left, right) in a.iter().zip(b.iter()) {
        assert_eq!(left.position, right.position);
    }
}

#[test]
fn particles_die_after_their_lifetime() {
    let mut world = World::new();
    let entity = spawn_emitter(
        &mut world,
        ParticleSystem {
            emission_rate: 4.0,
            ..fire()
        },
        [0.0; 3],
    );

    // Emit 0.5 seconds = 2 particles, then run to end of lifetime with no new emission.
    run(&mut world, 0.5);
    assert_eq!(particles_of(&world, entity).len(), 2);

    let mut system = fire();
    system.emission_rate = 0.0;
    world.insert(entity, system);
    run(&mut world, 1.5);
    assert!(particles_of(&world, entity).is_empty());
}

#[test]
fn a_short_emission_stops_after_its_duration() {
    let mut world = World::new();
    let entity = spawn_emitter(
        &mut world,
        ParticleSystem {
            emission_rate: 10.0,
            duration: 0.2,
            looping: false,
            lifetime: [10.0, 10.0],
            ..fire()
        },
        [0.0; 3],
    );

    run(&mut world, 0.1);
    let after_first = particles_of(&world, entity).len();
    run(&mut world, 0.1);
    let after_second = particles_of(&world, entity).len();
    run(&mut world, 0.5);
    let after_third = particles_of(&world, entity).len();

    assert_eq!(after_first, 1);
    assert!(
        after_second >= 2,
        "Should emit at least one more within duration"
    );
    assert_eq!(
        after_second, after_third,
        "Should not emit after duration ends, short-lived particles not yet dead"
    );
}

#[test]
fn gravity_and_drag_change_the_velocity() {
    let mut world = World::new();
    let entity = spawn_emitter(
        &mut world,
        ParticleSystem {
            emission_rate: 0.0,
            burst: 1,
            gravity: [0.0, -10.0, 0.0],
            drag: 0.0,
            start_speed: [0.0, 0.0],
            ..fire()
        },
        [0.0; 3],
    );

    // First 0.1 seconds only spawn (spawn after integration), second 0.1 seconds starts falling.
    run(&mut world, 0.2);
    let particles = particles_of(&world, entity);
    assert_eq!(particles.len(), 1);
    // Semi-implicit integration: first add v then move position → v = g * t = -1, displacement ≈ -0.1.
    assert!((particles[0].velocity.y + 1.0).abs() < 1e-4);
    assert!(particles[0].position.y < 0.0);
}

#[test]
fn the_cap_limits_the_number_of_live_particles() {
    let mut world = World::new();
    let entity = spawn_emitter(
        &mut world,
        ParticleSystem {
            emission_rate: 1000.0,
            max_particles: 5,
            lifetime: [10.0, 10.0],
            ..fire()
        },
        [0.0; 3],
    );

    run(&mut world, 0.5);
    assert_eq!(particles_of(&world, entity).len(), 5);
}

#[test]
fn a_paused_emitter_keeps_its_particles_in_place() {
    let mut world = World::new();
    let entity = spawn_emitter(
        &mut world,
        ParticleSystem {
            emission_rate: 10.0,
            start_speed: [3.0, 3.0],
            ..fire()
        },
        [0.0; 3],
    );

    run(&mut world, 0.2);
    let before = particles_of(&world, entity);
    assert!(!before.is_empty());

    let mut paused = fire();
    paused.playing = false;
    paused.start_speed = [3.0, 3.0];
    world.insert(entity, paused);
    run(&mut world, 0.5);
    assert_eq!(before, particles_of(&world, entity));
}

#[test]
fn the_emitter_transform_offsets_the_spawn_position() {
    let mut world = World::new();
    let entity = spawn_emitter(
        &mut world,
        ParticleSystem {
            emission_rate: 0.0,
            burst: 1,
            start_speed: [0.0, 0.0],
            shape_radius: 0.0,
            shape: ParticleShape::Sphere,
            ..fire()
        },
        [10.0, 2.0, -3.0],
    );

    particle_system(&mut world, 0.1);
    let particles = particles_of(&world, entity);
    assert_eq!(particles.len(), 1);
    assert_eq!(particles[0].position, glam::Vec3::new(10.0, 2.0, -3.0));
}

#[test]
fn a_cone_keeps_its_particles_inside_the_half_angle() {
    let mut world = World::new();
    let entity = spawn_emitter(
        &mut world,
        ParticleSystem {
            emission_rate: 200.0,
            lifetime: [10.0, 10.0],
            cone_angle_degrees: 15.0,
            shape: ParticleShape::Cone,
            shape_radius: 0.0,
            start_speed: [1.0, 1.0],
            ..fire()
        },
        [0.0; 3],
    );

    run(&mut world, 0.5);
    let particles = particles_of(&world, entity);
    assert!(!particles.is_empty());
    let limit = 15.0_f32.to_radians().cos();
    for particle in particles {
        let dir = particle.velocity.normalize();
        assert!(
            dir.y >= limit - 1e-4,
            "Cone only allows directions within local +Y half-angle, got {dir:?}"
        );
    }
}

#[test]
fn colors_fade_from_start_to_end() {
    let system = ParticleSystem {
        start_color: [1.0, 0.5, 0.0, 1.0],
        end_color: [0.0, 0.0, 1.0, 0.0],
        ..ParticleSystem::default()
    };
    let particle = Particle {
        position: glam::Vec3::ZERO,
        velocity: glam::Vec3::ZERO,
        age: 0.5,
        lifetime: 1.0,
        size: 1.0,
    };
    let color = particle_color(&system, &particle);
    assert!((color[0] - 0.5).abs() < 1e-6);
    assert!((color[1] - 0.25).abs() < 1e-6);
    assert!((color[2] - 0.5).abs() < 1e-6);
    assert!((color[3] - 0.5).abs() < 1e-6);
}

#[test]
fn extraction_sorts_particles_back_to_front() {
    let mut world = World::new();
    let entity = spawn_emitter(
        &mut world,
        ParticleSystem {
            emission_rate: 0.0,
            burst: 0,
            ..fire()
        },
        [0.0; 3],
    );
    world.insert(
        entity,
        ParticleEmitter {
            particles: vec![
                Particle {
                    position: glam::Vec3::new(0.0, 0.0, 1.0),
                    velocity: glam::Vec3::ZERO,
                    age: 0.0,
                    lifetime: 1.0,
                    size: 1.0,
                },
                Particle {
                    position: glam::Vec3::new(0.0, 0.0, -9.0),
                    velocity: glam::Vec3::ZERO,
                    age: 0.0,
                    lifetime: 1.0,
                    size: 1.0,
                },
            ],
            ..ParticleEmitter::seeded(entity)
        },
    );

    let mut out = Vec::new();
    scene_particles_into(&world, glam::Vec3::ZERO, &mut out);
    assert_eq!(out.len(), 2);
    // Camera at origin looking −Z: z = −9 farther, drawn first.
    assert_eq!(out[0].center[2], -9.0);
    assert_eq!(out[1].center[2], 1.0);
}

#[test]
fn extraction_skips_emitters_without_an_authoring_component() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(
        entity,
        ParticleEmitter {
            particles: vec![Particle {
                position: glam::Vec3::ZERO,
                velocity: glam::Vec3::ZERO,
                age: 0.0,
                lifetime: 1.0,
                size: 1.0,
            }],
            ..ParticleEmitter::seeded(entity)
        },
    );

    let mut out = Vec::new();
    scene_particles_into(&world, glam::Vec3::ZERO, &mut out);
    assert!(
        out.is_empty(),
        "Emitter without author component should not output image"
    );
}

#[test]
fn a_huge_dt_is_clamped_to_the_maximum_step() {
    let mut world = World::new();
    let entity = spawn_emitter(
        &mut world,
        ParticleSystem {
            emission_rate: 10.0,
            lifetime: [10.0, 10.0],
            ..fire()
        },
        [0.0; 3],
    );

    // dt of 10 seconds clamped to 0.1 seconds → only 1 particle emitted (not 100).
    particle_system(&mut world, 10.0);
    let particles = particles_of(&world, entity);
    assert_eq!(particles.len(), 1);
    // New particle spawned after integration, so age this frame still 0.
    assert_eq!(particles[0].age, 0.0);
}

fn particle_at(age: f32, lifetime: f32, size: f32) -> Particle {
    Particle {
        position: glam::Vec3::ZERO,
        velocity: glam::Vec3::ZERO,
        age,
        lifetime,
        size,
    }
}

#[test]
fn gradient_samples_every_stop_uniformly() {
    let stops = [
        [1.0, 0.0, 0.0, 1.0],
        [0.0, 1.0, 0.0, 1.0],
        [0.0, 0.0, 1.0, 1.0],
    ];
    assert_eq!(sample_gradient(&stops, 0.0), Some([1.0, 0.0, 0.0, 1.0]));
    assert_eq!(sample_gradient(&stops, 0.5), Some([0.0, 1.0, 0.0, 1.0]));
    assert_eq!(sample_gradient(&stops, 1.0), Some([0.0, 0.0, 1.0, 1.0]));

    // Linear interpolation between two stops: t = 0.25 → scaled 0.5 → half red half green.
    let quarter = sample_gradient(&stops, 0.25).unwrap();
    for (actual, expected) in quarter.iter().zip([0.5, 0.5, 0.0, 1.0]) {
        assert!((actual - expected).abs() < 1e-5, "{quarter:?}");
    }

    // Beyond range clamps to endpoint; non-finite value treated as dead.
    assert_eq!(sample_gradient(&stops, -2.0), Some(stops[0]));
    assert_eq!(sample_gradient(&stops, 9.0), Some(stops[2]));
    assert_eq!(sample_gradient(&stops, f32::NAN), Some(stops[2]));
}

#[test]
fn a_single_stop_is_a_constant_and_no_stops_means_legacy_lerp() {
    let single = [[0.2, 0.4, 0.6, 0.8]];
    assert_eq!(sample_gradient(&single, 0.0), Some(single[0]));
    assert_eq!(sample_gradient(&single, 1.0), Some(single[0]));
    assert_eq!(sample_gradient(&[], 0.5), None);

    let system = ParticleSystem {
        start_color: [1.0, 0.0, 0.0, 1.0],
        end_color: [0.0, 0.0, 1.0, 1.0],
        color_stops: Vec::new(),
        ..fire()
    };
    let color = particle_color(&system, &particle_at(0.5, 1.0, 0.5));
    for (actual, expected) in color.iter().zip([0.5, 0.0, 0.5, 1.0]) {
        assert!((actual - expected).abs() < 1e-5, "{color:?}");
    }
}

#[test]
fn color_stops_override_the_two_colour_lerp() {
    let system = ParticleSystem {
        start_color: [1.0, 1.0, 1.0, 1.0],
        end_color: [0.0, 0.0, 0.0, 1.0],
        color_stops: vec![
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 1.0],
        ],
        ..fire()
    };
    let color = particle_color(&system, &particle_at(0.5, 1.0, 0.5));
    assert_eq!(color, [0.0, 1.0, 0.0, 1.0], "t=0.5 lands at middle stop");
}

#[test]
fn size_over_lifetime_scales_the_birth_size() {
    let shrink = ParticleSystem {
        size_over_lifetime: [1.0, 0.0],
        ..fire()
    };
    assert!((particle_size(&shrink, &particle_at(0.5, 1.0, 0.5)) - 0.25).abs() < 1e-5);
    assert!((particle_size(&shrink, &particle_at(1.0, 1.0, 0.5))).abs() < 1e-5);

    let grow = ParticleSystem {
        size_over_lifetime: [0.5, 2.0],
        ..fire()
    };
    assert!((particle_size(&grow, &particle_at(0.0, 1.0, 0.5)) - 0.25).abs() < 1e-5);
    assert!((particle_size(&grow, &particle_at(1.0, 1.0, 0.5)) - 1.0).abs() < 1e-5);

    // Non-finite multiplier as 1; negative multiplier clamped to 0, never produces negative size / NaN.
    let broken = ParticleSystem {
        size_over_lifetime: [f32::NAN, f32::INFINITY],
        ..fire()
    };
    assert!((particle_size(&broken, &particle_at(0.5, 1.0, 0.5)) - 0.5).abs() < 1e-5);
    let negative = ParticleSystem {
        size_over_lifetime: [-1.0, -1.0],
        ..fire()
    };
    assert_eq!(particle_size(&negative, &particle_at(0.5, 1.0, 0.5)), 0.0);
}

#[test]
fn velocity_over_lifetime_adds_a_linear_drift_without_touching_velocity() {
    let mut world = World::new();
    let system = ParticleSystem {
        emission_rate: 0.0,
        burst: 0,
        velocity_over_lifetime: [[1.0, 0.0, 0.0], [3.0, 0.0, 0.0]],
        ..fire()
    };
    let entity = spawn_emitter(&mut world, system, [0.0; 3]);
    world.insert(
        entity,
        ParticleEmitter {
            particles: vec![Particle {
                position: glam::Vec3::ZERO,
                velocity: glam::Vec3::ZERO,
                age: 0.0,
                lifetime: 1.0,
                size: 1.0,
            }],
            ..ParticleEmitter::seeded(entity)
        },
    );

    // t=0.1: offset = 1 + (3-1)*0.1 = 1.2 → x += 0.12
    particle_system(&mut world, 0.1);
    let first = particles_of(&world, entity)[0].position.x;
    assert!((first - 0.12).abs() < 1e-5, "{first}");
    assert_eq!(
        particles_of(&world, entity)[0].velocity,
        glam::Vec3::ZERO,
        "Speed curve only adds displacement, does not write back speed"
    );

    // t=0.2: offset = 1.4 → x += 0.14
    particle_system(&mut world, 0.1);
    let second = particles_of(&world, entity)[0].position.x;
    assert!((second - 0.26).abs() < 1e-5, "{second}");
}

#[test]
fn velocity_offset_lerps_and_sanitizes_non_finite_components() {
    let system = ParticleSystem {
        velocity_over_lifetime: [[0.0, 1.0, 0.0], [f32::NAN, 5.0, f32::INFINITY]],
        ..fire()
    };
    let offset = particle_velocity_offset(&system, &particle_at(0.5, 1.0, 0.5));
    assert_eq!(offset, glam::Vec3::new(0.0, 3.0, 0.0));

    // Default curve = no change.
    let default = ParticleSystem::default().velocity_over_lifetime;
    assert_eq!(default, [[0.0; 3]; 2]);
    assert_eq!(
        particle_velocity_offset(&ParticleSystem::default(), &particle_at(0.5, 1.0, 0.5)),
        glam::Vec3::ZERO
    );
}

#[test]
fn scene_particles_use_the_size_curve() {
    let mut world = World::new();
    let system = ParticleSystem {
        size_over_lifetime: [1.0, 0.0],
        ..fire()
    };
    let entity = spawn_emitter(&mut world, system, [0.0, 0.0, 0.0]);
    run(&mut world, 0.02);
    let mut out = Vec::new();
    scene_particles_into(&world, glam::Vec3::ZERO, &mut out);
    assert!(!out.is_empty(), "Should be at least one particle");
    assert!(
        out.iter().all(|item| item.size <= 0.5 && item.size >= 0.0),
        "Size must be scaled by curve to [0, spawn size]"
    );
    let _ = entity;
}

fn spawn_box_collider(world: &mut World, half: f32, translation: [f32; 3]) -> Entity {
    let entity = world.spawn();
    world.insert(
        entity,
        crate::scene::components::Collider {
            shape: crate::scene::components::ColliderShape::Box,
            half_extents: [half; 3],
            ..Default::default()
        },
    );
    world.insert(
        entity,
        LocalToWorld(glam::Mat4::from_translation(glam::Vec3::from_array(
            translation,
        ))),
    );
    entity
}

fn spawn_flat_terrain(world: &mut World, height: f32, size: f32) -> Entity {
    let entity = world.spawn();
    world.insert(
        entity,
        TerrainData {
            key: "test-terrain".to_string(),
            field: Arc::new(Heightfield::flat(4)),
            mesh_path: String::new(),
            size: [size, size],
            height_scale: 1.0,
        },
    );
    world.insert(
        entity,
        LocalToWorld(glam::Mat4::from_translation(glam::Vec3::new(
            0.0, height, 0.0,
        ))),
    );
    entity
}

/// #501: after enabling collision particles blocked by static box, stop on top face; with collision off pass through.
#[test]
fn collision_stops_particles_on_a_static_box() {
    let falling = || ParticleSystem {
        collision: true,
        collision_bounce: 0.0,
        collision_friction: 1.0,
        emission_rate: 0.0,
        burst: 1,
        lifetime: [10.0, 10.0],
        start_speed: [0.0, 0.0],
        start_size: [0.25, 0.25],
        gravity: [0.0, -9.81, 0.0],
        drag: 0.0,
        shape: ParticleShape::Point,
        ..fire()
    };

    let mut world = World::new();
    spawn_box_collider(&mut world, 1.0, [0.0, 0.0, 0.0]);
    let emitter = spawn_emitter(&mut world, falling(), [0.0, 5.0, 0.0]);
    run(&mut world, 1.5);

    let particles = particles_of(&world, emitter);
    assert_eq!(particles.len(), 1);
    let y = particles[0].position.y;
    assert!(
        (1.2..1.4).contains(&y),
        "particle should rest above the box top (y≈1.25), got {y}"
    );
    assert!(
        particles[0].velocity.y.abs() < 0.1,
        "normal velocity should be absorbed, got {:?}",
        particles[0].velocity
    );
}

/// #501: terrain height field also participates in collision (#517 parsed height field).
#[test]
fn collision_stops_particles_on_a_terrain_heightfield() {
    let mut world = World::new();
    spawn_flat_terrain(&mut world, 1.0, 20.0);
    let emitter = spawn_emitter(
        &mut world,
        ParticleSystem {
            collision: true,
            collision_bounce: 0.0,
            collision_friction: 1.0,
            emission_rate: 0.0,
            burst: 1,
            lifetime: [10.0, 10.0],
            start_speed: [0.0, 0.0],
            start_size: [0.25, 0.25],
            gravity: [0.0, -9.81, 0.0],
            drag: 0.0,
            shape: ParticleShape::Point,
            ..fire()
        },
        [0.0, 5.0, 0.0],
    );
    run(&mut world, 1.5);

    let particles = particles_of(&world, emitter);
    assert_eq!(particles.len(), 1);
    let y = particles[0].position.y;
    assert!(
        (1.2..1.4).contains(&y),
        "particle should rest above the terrain surface (y≈1.25), got {y}"
    );
    assert!(
        particles[0].velocity.y.abs() < 0.1,
        "normal velocity should be absorbed, got {:?}",
        particles[0].velocity
    );
}

#[test]
fn particles_fall_through_when_collision_is_disabled() {
    let mut world = World::new();
    spawn_box_collider(&mut world, 1.0, [0.0, 0.0, 0.0]);
    let emitter = spawn_emitter(
        &mut world,
        ParticleSystem {
            collision: false,
            emission_rate: 0.0,
            burst: 1,
            lifetime: [10.0, 10.0],
            start_speed: [0.0, 0.0],
            start_size: [0.25, 0.25],
            gravity: [0.0, -9.81, 0.0],
            drag: 0.0,
            shape: ParticleShape::Point,
            ..fire()
        },
        [0.0, 5.0, 0.0],
    );
    run(&mut world, 1.5);

    let particles = particles_of(&world, emitter);
    assert_eq!(particles.len(), 1);
    assert!(
        particles[0].position.y < 0.0,
        "without collision the particle should fall through, got y={}",
        particles[0].position.y
    );
}
