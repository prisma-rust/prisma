//! Fixed-timestep 2D roguelite combat, enemy horde AI, weapons, pickups, and wave progression.

use glam::{Quat, Vec3};
use prism_game_api::physics2d::{resolve_crowd_separation, Circle2, CrowdAgent2D};
use prism_game_api::{keys, EntityHandle, GameWorld};
use prism_scene::components::{
    Camera, LocalTransform, ParticleEmitter2D, ParticleShape2D, SpriteRenderer,
};

use crate::config::{EnemyBehavior, WeaponDef, WeaponKind};
use crate::state::{
    ActiveEnemy, ActivePickup, ActiveProjectile, DamageNumber, EquippedWeapon, GamePhase,
    PickupKind, SpawnWarning, SurvivorsState, ARENA_HALF_H, ARENA_HALF_W, POOL_ENEMY,
    POOL_MELEE_FX, POOL_PICKUP, POOL_PROJECTILE, POOL_WARNING, POOL_WEAPON, POOL_WHITE_BAR,
};

const HIT_BURST_POOL_LEN: usize = 16;
const ARENA_MARGIN: f32 = 0.85;

/// Perform one-time arena, player, HP bar, and particle emitter setup.
pub fn ensure_world_initialized(state: &mut SurvivorsState, world: &mut GameWorld<'_>) {
    if state.world_built {
        return;
    }
    world.set_physics2d_settings([0.0, 0.0], false);

    if let Some(player) = world.find_by_name("player") {
        state.player_entity = player;
    } else {
        let player = world.spawn();
        world.set(
            player,
            LocalTransform {
                position: Vec3::new(0.0, 0.0, 0.5),
                rotation: Quat::IDENTITY,
                scale: Vec3::splat(1.15),
            },
        );
        state.player_entity = player;
    }

    world.enable_transform_interpolation2d(state.player_entity);
    world.snap_transform_interpolation2d(state.player_entity);
    state.config.apply_sprite(
        world,
        state.player_entity,
        "player_idle",
        (10, 0),
        [1.0, 1.0, 1.0, 1.0],
        false,
    );
    world.set(
        state.player_entity,
        LocalTransform {
            position: Vec3::new(state.player_pos[0], state.player_pos[1], 0.5),
            rotation: Quat::IDENTITY,
            scale: Vec3::splat(1.15),
        },
    );

    if let Some(camera) = world.find_by_name("camera") {
        if let Some(cam) = world.get::<Camera>(camera) {
            if cam.orthographic_size.is_finite() && cam.orthographic_size > 1.0 {
                state.ortho_half_height = cam.orthographic_size;
            }
        }
        if let Some(cam_tf) = world.get::<LocalTransform>(camera) {
            state.camera_pos = [cam_tf.position.x, cam_tf.position.y];
        }
    }

    build_arena_backdrop(state, world);

    // Player world-space HP bar using the built-in 1x1 white sprite.
    let hp_bg = world.spawn();
    world.set(
        hp_bg,
        SpriteRenderer {
            sorting_layer: 20,
            order_in_layer: 0,
            ..SpriteRenderer::white([0.06, 0.06, 0.09, 0.88])
        },
    );
    world.set(
        hp_bg,
        LocalTransform {
            position: Vec3::new(0.0, 0.9, 1.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::new(1.36, 0.16, 1.0),
        },
    );
    state.player_hp_bg = hp_bg;

    let hp_fill = world.spawn();
    world.set(
        hp_fill,
        SpriteRenderer {
            sorting_layer: 20,
            order_in_layer: 1,
            ..SpriteRenderer::white([0.20, 0.88, 0.34, 0.95])
        },
    );
    world.set(
        hp_fill,
        LocalTransform {
            position: Vec3::new(0.0, 0.9, 1.1),
            rotation: Quat::IDENTITY,
            scale: Vec3::new(1.28, 0.10, 1.0),
        },
    );
    state.player_hp_fill = hp_fill;

    state.hit_burst_entities = vec![EntityHandle::PLACEHOLDER; HIT_BURST_POOL_LEN];
    state.hit_burst_cursor = 0;
    state.world_built = true;
}

fn build_arena_backdrop(state: &SurvivorsState, world: &mut GameWorld<'_>) {
    // Tile the arena floor using data-configured sprite slicing.
    let step = 2.0_f32;
    let mut iy = 0_i32;
    let mut y = -ARENA_HALF_H + step * 0.5;
    while y < ARENA_HALF_H {
        let mut ix = 0_i32;
        let mut x = -ARENA_HALF_W + step * 0.5;
        while x < ARENA_HALF_W {
            let tile = world.spawn();
            world.set(
                tile,
                LocalTransform {
                    position: Vec3::new(x, y, -2.0),
                    rotation: Quat::IDENTITY,
                    scale: Vec3::splat(step),
                },
            );
            let tint = if (ix + iy) % 2 == 0 {
                [0.92, 0.95, 1.0, 1.0]
            } else {
                [0.80, 0.84, 0.90, 1.0]
            };
            state
                .config
                .apply_sprite(world, tile, "arena_tile", (-10, (ix + iy) & 1), tint, false);
            x += step;
            ix += 1;
        }
        y += step;
        iy += 1;
    }

    // Place decorative crates near outer quadrants so center combat lanes remain open.
    let crate_coords = [
        [-16.5, 10.5],
        [-15.3, 10.7],
        [16.0, 11.0],
        [17.2, -10.2],
        [-17.5, -11.0],
        [-16.2, -11.4],
        [15.4, -10.8],
        [0.0, 12.6],
        [0.0, -12.6],
    ];
    for [cx, cy] in crate_coords {
        let prop = world.spawn();
        world.set(
            prop,
            LocalTransform {
                position: Vec3::new(cx, cy, -1.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::splat(1.35),
            },
        );
        state.config.apply_sprite(
            world,
            prop,
            "arena_crate",
            (-5, 0),
            [0.88, 0.84, 0.80, 1.0],
            false,
        );
    }

    // Arena perimeter walls rendered via built-in 1x1 white sprites.
    let wall_thickness = 1.4;
    let total_w = ARENA_HALF_W * 2.0 + wall_thickness * 2.0;
    let total_h = ARENA_HALF_H * 2.0 + wall_thickness * 2.0;
    let walls = [
        (
            [0.0, ARENA_HALF_H + wall_thickness * 0.5],
            [total_w, wall_thickness],
        ),
        (
            [0.0, -ARENA_HALF_H - wall_thickness * 0.5],
            [total_w, wall_thickness],
        ),
        (
            [-ARENA_HALF_W - wall_thickness * 0.5, 0.0],
            [wall_thickness, total_h],
        ),
        (
            [ARENA_HALF_W + wall_thickness * 0.5, 0.0],
            [wall_thickness, total_h],
        ),
    ];
    for ([wx, wy], [sw, sh]) in walls {
        let wall = world.spawn();
        world.set(
            wall,
            SpriteRenderer {
                sorting_layer: -3,
                order_in_layer: 0,
                ..SpriteRenderer::white([0.24, 0.28, 0.36, 1.0])
            },
        );
        world.set(
            wall,
            LocalTransform {
                position: Vec3::new(wx, wy, -0.5),
                rotation: Quat::IDENTITY,
                scale: Vec3::new(sw, sh, 1.0),
            },
        );
    }
}

/// Spawn a pooled one-shot 2D particle burst at `pos`.
pub fn trigger_hit_burst(
    state: &mut SurvivorsState,
    world: &mut GameWorld<'_>,
    pos: [f32; 2],
    start_color: [f32; 4],
    end_color: [f32; 4],
    count: u32,
) {
    if state.hit_burst_entities.is_empty() {
        return;
    }
    let slot = state.hit_burst_cursor % state.hit_burst_entities.len();
    state.hit_burst_cursor = slot + 1;

    let prev = state.hit_burst_entities[slot];
    if prev != EntityHandle::PLACEHOLDER && world.is_alive(prev) {
        world.despawn(prev);
    }

    let burst_entity = world.spawn();
    world.set(
        burst_entity,
        LocalTransform {
            position: Vec3::new(pos[0], pos[1], 0.8),
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        },
    );
    world.set(
        burst_entity,
        ParticleEmitter2D {
            playing: true,
            emission_rate: 0.0,
            burst: count,
            duration: 0.35,
            looping: false,
            max_particles: count.max(16),
            lifetime: [0.14, 0.28],
            start_speed: [2.2, 6.2],
            start_size: [0.11, 0.23],
            start_color,
            end_color,
            gravity: [0.0, 0.0],
            drag: 0.72,
            shape: ParticleShape2D::Point,
            shape_size: [0.2, 0.2],
            shape_radius: 0.15,
            direction: [1.0, 0.0],
            spread_degrees: 180.0,
            sorting_layer: 16,
            order_in_layer: 0,
        },
    );
    state.hit_burst_entities[slot] = burst_entity;
}

/// Spawn an active enemy at a specific world coordinate (also used by integration tests).
pub fn spawn_enemy_at(
    state: &mut SurvivorsState,
    world: &mut GameWorld<'_>,
    enemy_id: &str,
    pos: [f32; 2],
) -> Option<usize> {
    let def_index = state.config.enemy_index_by_id(enemy_id)?;
    let def = &state.config.enemies[def_index];
    let wave_offset = state.wave_index.saturating_sub(1) as f32;
    let max_hp = (def.base_hp + def.hp_per_wave * wave_offset).max(1.0);
    let clamped = [
        pos[0].clamp(-ARENA_HALF_W + ARENA_MARGIN, ARENA_HALF_W - ARENA_MARGIN),
        pos[1].clamp(-ARENA_HALF_H + ARENA_MARGIN, ARENA_HALF_H - ARENA_MARGIN),
    ];

    let (entity, _) = state.pool.acquire(world, POOL_ENEMY);
    let sprite_scale = if def.behavior == EnemyBehavior::Boss {
        1.75
    } else if def.is_elite {
        1.30
    } else {
        1.05
    };
    world.set(
        entity,
        LocalTransform {
            position: Vec3::new(clamped[0], clamped[1], 0.3),
            rotation: Quat::IDENTITY,
            scale: Vec3::splat(sprite_scale),
        },
    );
    state.config.apply_sprite(
        world,
        entity,
        &def.sprite,
        (5, 0),
        [1.0, 1.0, 1.0, 1.0],
        clamped[0] > state.player_pos[0],
    );

    let (hp_bg_entity, hp_fill_entity) = if def.is_elite {
        let (bg, _) = state.pool.acquire(world, POOL_WHITE_BAR);
        world.set(
            bg,
            SpriteRenderer {
                sorting_layer: 20,
                order_in_layer: 0,
                ..SpriteRenderer::white([0.08, 0.06, 0.08, 0.88])
            },
        );
        let (fill, _) = state.pool.acquire(world, POOL_WHITE_BAR);
        world.set(
            fill,
            SpriteRenderer {
                sorting_layer: 20,
                order_in_layer: 1,
                ..SpriteRenderer::white([0.92, 0.22, 0.24, 0.95])
            },
        );
        (bg, fill)
    } else {
        (EntityHandle::PLACEHOLDER, EntityHandle::PLACEHOLDER)
    };

    let uid = state.next_enemy_uid;
    state.next_enemy_uid = state.next_enemy_uid.saturating_add(1);

    let initial_action = match def.behavior {
        EnemyBehavior::Charger => state.rng.range_f32(1.1, 2.2),
        EnemyBehavior::RangedShooter => state.rng.range_f32(1.0, 1.9),
        EnemyBehavior::Boss => 2.0,
        EnemyBehavior::Chaser => 0.0,
    };

    state.enemies.push(ActiveEnemy {
        uid,
        def_index,
        behavior: def.behavior,
        entity,
        hp_bg_entity,
        hp_fill_entity,
        pos: clamped,
        knockback_vel: [0.0, 0.0],
        hp: max_hp,
        max_hp,
        speed: def.speed,
        radius: def.radius,
        weight: def.weight,
        contact_damage: def.contact_damage,
        hit_flash: 0.0,
        action_timer: initial_action,
        telegraph_timer: 0.0,
        dash_dir: [1.0, 0.0],
        dash_timer: 0.0,
        is_elite: def.is_elite,
    });
    Some(state.enemies.len() - 1)
}

/// Spawn a pickup gem, gold coin, or heal orb at `pos`.
pub fn spawn_pickup_at(
    state: &mut SurvivorsState,
    world: &mut GameWorld<'_>,
    kind: PickupKind,
    pos: [f32; 2],
) {
    let clamped = [
        pos[0].clamp(-ARENA_HALF_W + 0.6, ARENA_HALF_W - 0.6),
        pos[1].clamp(-ARENA_HALF_H + 0.6, ARENA_HALF_H - 0.6),
    ];
    let (entity, _) = state.pool.acquire(world, POOL_PICKUP);
    let (sprite_key, scale) = match kind {
        PickupKind::XpSmall(_) => ("pickup_xp_small", 0.72),
        PickupKind::XpLarge(_) => ("pickup_xp_large", 0.90),
        PickupKind::Gold(_) => ("pickup_coin", 0.72),
        PickupKind::Heal(_) => ("pickup_heal", 0.85),
    };
    world.set(
        entity,
        LocalTransform {
            position: Vec3::new(clamped[0], clamped[1], 0.1),
            rotation: Quat::IDENTITY,
            scale: Vec3::splat(scale),
        },
    );
    state.config.apply_sprite(
        world,
        entity,
        sprite_key,
        (0, 0),
        [1.0, 1.0, 1.0, 1.0],
        false,
    );
    state.pickups.push(ActivePickup {
        entity,
        kind,
        pos: clamped,
        magnetized: matches!(state.phase, GamePhase::WaveClearSweep { .. }),
        speed: 0.0,
    });
}

/// Apply direct or critical damage plus knockback impulse to an active enemy.
pub fn apply_damage_to_enemy(
    state: &mut SurvivorsState,
    world: &mut GameWorld<'_>,
    enemy_idx: usize,
    base_damage: f32,
    crit_chance_pct: f32,
    crit_mult: f32,
    impulse: [f32; 2],
) {
    let Some(enemy) = state.enemies.get_mut(enemy_idx) else {
        return;
    };
    let is_crit = state.rng.next_f32() * 100.0 < crit_chance_pct;
    let final_dmg = if is_crit {
        (base_damage * crit_mult).max(1.0)
    } else {
        base_damage.max(1.0)
    };
    enemy.hp -= final_dmg;
    enemy.hit_flash = 0.11;
    let inv_weight = (1.0 / enemy.weight.max(0.25)).min(2.0);
    enemy.knockback_vel[0] += impulse[0] * inv_weight;
    enemy.knockback_vel[1] += impulse[1] * inv_weight;

    state.stats.total_damage = state
        .stats
        .total_damage
        .saturating_add(final_dmg.round() as u64);

    let hit_pos = enemy.pos;
    state.damage_numbers.push(DamageNumber {
        world_pos: [hit_pos[0], hit_pos[1] + enemy.radius * 0.9],
        vel_y: 1.85,
        age: 0.0,
        duration: 0.65,
        amount: final_dmg.round().max(1.0) as u32,
        is_crit,
        is_player_hit: false,
    });

    if is_crit || enemy.hp <= 0.0 {
        trigger_hit_burst(
            state,
            world,
            hit_pos,
            if is_crit {
                [1.0, 0.85, 0.25, 1.0]
            } else {
                [0.95, 0.35, 0.30, 1.0]
            },
            [1.0, 0.45, 0.1, 0.0],
            if is_crit { 14 } else { 9 },
        );
    }
}

/// Deterministic fixed-timestep update covering combat, horde separation, weapons, waves, and sweeps.
pub fn fixed_step(state: &mut SurvivorsState, world: &mut GameWorld<'_>, dt: f32) {
    ensure_world_initialized(state, world);
    let dt = dt.clamp(0.0, 0.05);
    if dt <= 0.0 {
        return;
    }

    if let Some((_, ttl)) = &mut state.status_banner {
        *ttl -= dt;
        if *ttl <= 0.0 {
            state.status_banner = None;
        }
    }

    match state.phase {
        GamePhase::WaveCombat => step_wave_combat(state, world, dt),
        GamePhase::WaveClearSweep { .. } => step_wave_clear_sweep(state, world, dt),
        GamePhase::LevelUpChoice { .. }
        | GamePhase::ShopPhase { .. }
        | GamePhase::Victory
        | GamePhase::GameOver => {
            sync_weapons_passive(state, world, dt);
        }
    }
}

fn step_wave_combat(state: &mut SurvivorsState, world: &mut GameWorld<'_>, dt: f32) {
    step_player_movement(state, world, dt);
    step_spawner_and_warnings(state, world, dt);
    rebuild_enemy_grid(state);
    step_weapons(state, world, dt);
    step_projectiles(state, world, dt);
    step_enemies(state, world, dt);
    cleanup_dead_enemies(state, world);
    step_pickups(state, world, dt, 18.0);

    if state.stats.hp <= 0.0 {
        state.stats.hp = 0.0;
        state.phase = GamePhase::GameOver;
        state.set_banner("DEFEATED!", 5.0);
        return;
    }

    if state.pending_level_ups > 0 {
        state.enter_level_up_if_pending(world);
        return;
    }

    state.wave_timer = (state.wave_timer - dt).max(0.0);
    if state.wave_timer <= 0.0 {
        begin_wave_clear_sweep(state, world);
    }
}

fn begin_wave_clear_sweep(state: &mut SurvivorsState, world: &mut GameWorld<'_>) {
    let cleared = std::mem::take(&mut state.enemies);
    for enemy in cleared {
        trigger_hit_burst(
            state,
            world,
            enemy.pos,
            [0.65, 0.85, 1.0, 0.9],
            [0.2, 0.4, 0.9, 0.0],
            6,
        );
        state.pool.release(world, POOL_ENEMY, enemy.entity);
        if enemy.hp_bg_entity != EntityHandle::PLACEHOLDER {
            state
                .pool
                .release(world, POOL_WHITE_BAR, enemy.hp_bg_entity);
        }
        if enemy.hp_fill_entity != EntityHandle::PLACEHOLDER {
            state
                .pool
                .release(world, POOL_WHITE_BAR, enemy.hp_fill_entity);
        }
    }
    for warn in state.spawn_warnings.drain(..) {
        state.pool.release(world, POOL_WARNING, warn.entity);
    }
    for proj in state.projectiles.drain(..) {
        state.pool.release(world, POOL_PROJECTILE, proj.entity);
    }
    for pickup in &mut state.pickups {
        pickup.magnetized = true;
    }

    let bonus = state
        .config
        .wave_for_number(state.wave_index)
        .clear_gold_bonus;
    state.stats.gold += bonus;
    state.wave_sweep_timer = 0.9;
    state.phase = GamePhase::WaveClearSweep { timer_tenths: 9 };
    state.set_banner(format!("WAVE {} CLEARED! +{bonus}G", state.wave_index), 1.8);
    world.play_sound("audio/wave_clear.wav", 0.5, false);
}

fn step_wave_clear_sweep(state: &mut SurvivorsState, world: &mut GameWorld<'_>, dt: f32) {
    step_player_movement(state, world, dt);
    sync_weapons_passive(state, world, dt);
    for pickup in &mut state.pickups {
        pickup.magnetized = true;
    }
    step_pickups(state, world, dt, 26.0);

    state.wave_sweep_timer = (state.wave_sweep_timer - dt).max(0.0);
    let tenths = (state.wave_sweep_timer * 10.0).ceil() as u32;
    state.phase = GamePhase::WaveClearSweep {
        timer_tenths: tenths,
    };

    if state.wave_sweep_timer <= 0.0 || (state.pickups.is_empty() && state.wave_sweep_timer <= 0.55)
    {
        let remaining = std::mem::take(&mut state.pickups);
        for pickup in remaining {
            collect_pickup(state, world, pickup.kind, false);
            state.pool.release(world, POOL_PICKUP, pickup.entity);
        }
        if !state.enter_level_up_if_pending(world) {
            state.open_shop(world);
        }
    }
}

fn step_player_movement(state: &mut SurvivorsState, world: &mut GameWorld<'_>, dt: f32) {
    let input = world.input();
    let mut move_x = 0.0_f32;
    let mut move_y = 0.0_f32;

    if input.key(keys::KEY_A) || input.key(keys::ARROW_LEFT) {
        move_x -= 1.0;
    }
    if input.key(keys::KEY_D) || input.key(keys::ARROW_RIGHT) {
        move_x += 1.0;
    }
    if input.key(keys::KEY_S) || input.key(keys::ARROW_DOWN) {
        move_y -= 1.0;
    }
    if input.key(keys::KEY_W) || input.key(keys::ARROW_UP) {
        move_y += 1.0;
    }

    let len_sq = move_x * move_x + move_y * move_y;
    let moving = len_sq > 0.0;
    if moving {
        let inv = len_sq.sqrt().recip();
        let speed = state.stats.move_speed();
        state.player_pos[0] = (state.player_pos[0] + move_x * inv * speed * dt)
            .clamp(-ARENA_HALF_W + ARENA_MARGIN, ARENA_HALF_W - ARENA_MARGIN);
        state.player_pos[1] = (state.player_pos[1] + move_y * inv * speed * dt)
            .clamp(-ARENA_HALF_H + ARENA_MARGIN, ARENA_HALF_H - ARENA_MARGIN);
    }

    if move_x < -0.01 {
        state.player_facing_left = true;
    } else if move_x > 0.01 {
        state.player_facing_left = false;
    }

    if state.player_i_frames > 0.0 {
        state.player_i_frames = (state.player_i_frames - dt).max(0.0);
    }
    if state.player_hit_flash > 0.0 {
        state.player_hit_flash = (state.player_hit_flash - dt).max(0.0);
    }

    // Continuous HP regeneration from player stats.
    if state.stats.hp_regen > 0.0 && state.stats.hp < state.stats.max_hp && state.stats.hp > 0.0 {
        state.player_regen_accum += state.stats.hp_regen * dt;
        if state.player_regen_accum >= 1.0 {
            let heal = state.player_regen_accum.floor();
            state.player_regen_accum -= heal;
            state.stats.hp = (state.stats.hp + heal).min(state.stats.max_hp);
        }
    }

    let color = if state.player_hit_flash > 0.0 {
        [3.2, 3.2, 3.2, 1.0]
    } else if state.player_i_frames > 0.0 && ((state.player_i_frames * 20.0) as i32) % 2 == 0 {
        [1.0, 0.75, 0.75, 0.45]
    } else {
        [1.0, 1.0, 1.0, 1.0]
    };

    if moving != state.player_is_moving {
        state.player_is_moving = moving;
        let key = if moving { "player_run" } else { "player_idle" };
        state.config.apply_sprite(
            world,
            state.player_entity,
            key,
            (10, 0),
            color,
            state.player_facing_left,
        );
    } else if let Some(mut sr) = world.get::<SpriteRenderer>(state.player_entity) {
        if sr.flip_x != state.player_facing_left || sr.color != color {
            sr.flip_x = state.player_facing_left;
            sr.color = color;
            world.set(state.player_entity, sr);
        }
    }

    world.set(
        state.player_entity,
        LocalTransform {
            position: Vec3::new(state.player_pos[0], state.player_pos[1], 0.5),
            rotation: Quat::IDENTITY,
            scale: Vec3::splat(1.15),
        },
    );
}

fn rebuild_enemy_grid(state: &mut SurvivorsState) {
    state.spatial_grid.clear();
    for (idx, enemy) in state.enemies.iter().enumerate() {
        state
            .spatial_grid
            .insert_circle(idx as u32, Circle2::new(enemy.pos, enemy.radius));
    }
}

fn sync_weapons_passive(state: &mut SurvivorsState, world: &mut GameWorld<'_>, dt: f32) {
    let count = state.weapons.len().max(1);
    for (slot, weapon) in state.weapons.iter_mut().enumerate() {
        ensure_weapon_entity(weapon, &state.config, &mut state.pool, world);
        if weapon.fx_timer > 0.0 {
            weapon.fx_timer = (weapon.fx_timer - dt).max(0.0);
            if weapon.fx_timer <= 0.0 && weapon.fx_entity != EntityHandle::PLACEHOLDER {
                state.pool.release(world, POOL_MELEE_FX, weapon.fx_entity);
                weapon.fx_entity = EntityHandle::PLACEHOLDER;
            }
        }
        let orbit = (slot as f32) * std::f32::consts::TAU / (count as f32);
        let anchor = [
            state.player_pos[0] + orbit.cos() * 1.15,
            state.player_pos[1] + orbit.sin() * 1.15,
        ];
        world.set(
            weapon.entity,
            LocalTransform {
                position: Vec3::new(anchor[0], anchor[1], 0.6),
                rotation: Quat::from_rotation_z(weapon.aim_angle),
                scale: Vec3::splat(0.85),
            },
        );
    }
}

fn ensure_weapon_entity(
    weapon: &mut EquippedWeapon,
    config: &crate::config::GameConfig,
    pool: &mut prism_game_api::EntityPool,
    world: &mut GameWorld<'_>,
) {
    if weapon.entity == EntityHandle::PLACEHOLDER || !world.is_alive(weapon.entity) {
        let (entity, _) = pool.acquire(world, POOL_WEAPON);
        weapon.entity = entity;
        weapon.sprite_applied = false;
    }
    if !weapon.sprite_applied {
        if let Some(def) = config.weapon_by_id(&weapon.weapon_id) {
            config.apply_sprite(
                world,
                weapon.entity,
                &def.sprite,
                (12, 0),
                tier_tint(weapon.tier),
                false,
            );
            weapon.sprite_applied = true;
        }
    }
}

fn tier_tint(tier: u8) -> [f32; 4] {
    match tier {
        2 => [0.78, 0.94, 1.0, 1.0],
        3 => [0.95, 0.78, 1.0, 1.0],
        4 => [1.0, 0.88, 0.55, 1.0],
        _ => [1.0, 1.0, 1.0, 1.0],
    }
}

fn step_weapons(state: &mut SurvivorsState, world: &mut GameWorld<'_>, dt: f32) {
    let weapon_count = state.weapons.len();
    if weapon_count == 0 {
        return;
    }

    let mut fired_ranged_sfx = false;
    let mut fired_melee_sfx = false;
    let mut hit_sfx = false;

    for slot in 0..weapon_count {
        let weapon_id = state.weapons[slot].weapon_id.clone();
        let Some(def) = state.config.weapon_by_id(&weapon_id).cloned() else {
            continue;
        };

        ensure_weapon_entity(
            &mut state.weapons[slot],
            &state.config,
            &mut state.pool,
            world,
        );
        if state.weapons[slot].fx_timer > 0.0 {
            state.weapons[slot].fx_timer = (state.weapons[slot].fx_timer - dt).max(0.0);
            if state.weapons[slot].fx_timer <= 0.0
                && state.weapons[slot].fx_entity != EntityHandle::PLACEHOLDER
            {
                state
                    .pool
                    .release(world, POOL_MELEE_FX, state.weapons[slot].fx_entity);
                state.weapons[slot].fx_entity = EntityHandle::PLACEHOLDER;
            }
        }
        if state.weapons[slot].swing_timer > 0.0 {
            state.weapons[slot].swing_timer = (state.weapons[slot].swing_timer - dt).max(0.0);
        }

        let orbit_angle = (slot as f32) * std::f32::consts::TAU / (weapon_count as f32);
        let anchor = [
            state.player_pos[0] + orbit_angle.cos() * 1.15,
            state.player_pos[1] + orbit_angle.sin() * 1.15,
        ];

        let range = state.weapons[slot].compute_range(&def, &state.stats);
        let cd = state.weapons[slot].compute_cooldown(&def, &state.stats);
        state.weapons[slot].total_cooldown = cd;
        state.weapons[slot].cooldown_remaining =
            (state.weapons[slot].cooldown_remaining - dt).max(0.0);

        // Locate nearest alive enemy around `anchor`.
        let enemies_ref = &state.enemies;
        let nearest = state.spatial_grid.find_nearest(anchor, range, |idx| {
            enemies_ref
                .get(idx as usize)
                .filter(|e| e.hp > 0.0)
                .map(|e| e.pos)
        });

        let mut can_fire = false;
        if let Some((_enemy_idx, target_pos, _dist_sq)) = nearest {
            let dx = target_pos[0] - anchor[0];
            let dy = target_pos[1] - anchor[1];
            state.weapons[slot].aim_angle = dy.atan2(dx);
            can_fire = state.weapons[slot].cooldown_remaining <= 0.0;
        } else {
            state.weapons[slot].aim_angle = if state.player_facing_left {
                std::f32::consts::PI
            } else {
                0.0
            };
        }

        if can_fire {
            state.weapons[slot].cooldown_remaining = cd;
            state.weapons[slot].swing_timer = state.weapons[slot].swing_duration;
            let aim = state.weapons[slot].aim_angle;
            let damage = state.weapons[slot].compute_damage(&def, &state.stats);
            let crit_chance = state.stats.crit_chance_pct + def.crit_chance_bonus;

            match def.kind {
                WeaponKind::Ranged => {
                    fire_ranged_weapon(state, world, &def, anchor, aim, damage, crit_chance);
                    if !fired_ranged_sfx {
                        world.play_sound("audio/shoot.wav", 0.30, false);
                        fired_ranged_sfx = true;
                    }
                }
                WeaponKind::MeleeSweep => {
                    let hit_any = perform_melee_sweep(
                        state,
                        world,
                        slot,
                        &def,
                        anchor,
                        aim,
                        range,
                        damage,
                        crit_chance,
                    );
                    if !fired_melee_sfx {
                        world.play_sound("audio/melee_swing.wav", 0.34, false);
                        fired_melee_sfx = true;
                    }
                    if hit_any && !hit_sfx {
                        world.play_sound("audio/hit_enemy.wav", 0.30, false);
                        hit_sfx = true;
                    }
                }
                WeaponKind::MeleeThrust => {
                    let hit_any = perform_melee_thrust(
                        state,
                        world,
                        slot,
                        &def,
                        anchor,
                        aim,
                        range,
                        damage,
                        crit_chance,
                    );
                    if !fired_melee_sfx {
                        world.play_sound("audio/melee_swing.wav", 0.34, false);
                        fired_melee_sfx = true;
                    }
                    if hit_any && !hit_sfx {
                        world.play_sound("audio/hit_enemy.wav", 0.30, false);
                        hit_sfx = true;
                    }
                }
            }
        }

        // Animate orbital weapon lunge/sweep transform.
        let weapon = &state.weapons[slot];
        let swing_t = if weapon.swing_duration > 0.0 {
            weapon.swing_timer / weapon.swing_duration
        } else {
            0.0
        };
        let (offset_dist, extra_rot) = match def.kind {
            WeaponKind::Ranged => (-0.18 * swing_t, 0.0),
            WeaponKind::MeleeSweep => (0.35 * swing_t, (swing_t - 0.5) * 1.6),
            WeaponKind::MeleeThrust => (0.95 * (1.0 - (swing_t - 0.5).abs() * 2.0).max(0.0), 0.0),
        };
        let render_pos = [
            anchor[0] + weapon.aim_angle.cos() * offset_dist,
            anchor[1] + weapon.aim_angle.sin() * offset_dist,
        ];
        let flip_y = weapon.aim_angle.cos() < 0.0;
        if let Some(mut sr) = world.get::<SpriteRenderer>(weapon.entity) {
            if sr.flip_y != flip_y {
                sr.flip_y = flip_y;
                world.set(weapon.entity, sr);
            }
        }
        world.set(
            weapon.entity,
            LocalTransform {
                position: Vec3::new(render_pos[0], render_pos[1], 0.6),
                rotation: Quat::from_rotation_z(weapon.aim_angle + extra_rot),
                scale: Vec3::splat(0.85),
            },
        );
    }
}

fn fire_ranged_weapon(
    state: &mut SurvivorsState,
    world: &mut GameWorld<'_>,
    def: &WeaponDef,
    origin: [f32; 2],
    aim_angle: f32,
    damage: f32,
    crit_chance: f32,
) {
    let count = def.projectiles_per_shot.max(1);
    let half_spread = def.spread_degrees.to_radians() * 0.5;
    for p in 0..count {
        let angle_offset = if count == 1 {
            state.rng.range_f32(-half_spread, half_spread)
        } else {
            let t = p as f32 / (count - 1) as f32;
            (t * 2.0 - 1.0) * half_spread + state.rng.range_f32(-0.04, 0.04)
        };
        let angle = aim_angle + angle_offset;
        let dir = [angle.cos(), angle.sin()];
        let spawn_pos = [origin[0] + dir[0] * 0.45, origin[1] + dir[1] * 0.45];
        let (entity, _) = state.pool.acquire(world, POOL_PROJECTILE);
        world.set(
            entity,
            LocalTransform {
                position: Vec3::new(spawn_pos[0], spawn_pos[1], 0.65),
                rotation: Quat::from_rotation_z(angle),
                scale: Vec3::splat(0.72),
            },
        );
        state.config.apply_sprite(
            world,
            entity,
            &def.projectile_sprite,
            (14, 0),
            [1.0, 1.0, 1.0, 1.0],
            false,
        );
        state.projectiles.push(ActiveProjectile {
            entity,
            pos: spawn_pos,
            vel: [dir[0] * def.projectile_speed, dir[1] * def.projectile_speed],
            radius: 0.24,
            damage,
            knockback: def.knockback,
            crit_chance,
            crit_mult: def.crit_multiplier,
            pierce_left: def.pierce,
            life: (def.range + 2.0) / def.projectile_speed.max(1.0),
            from_enemy: false,
            hit_enemy_uids: Vec::new(),
        });
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "melee sweep takes weapon slot, definition, origin, aim, range, damage, and crit"
)]
fn perform_melee_sweep(
    state: &mut SurvivorsState,
    world: &mut GameWorld<'_>,
    slot: usize,
    def: &WeaponDef,
    origin: [f32; 2],
    aim_angle: f32,
    range: f32,
    damage: f32,
    crit_chance: f32,
) -> bool {
    let dir = [aim_angle.cos(), aim_angle.sin()];
    let half_arc = def.arc_half_degrees.to_radians().max(0.25);
    state
        .spatial_grid
        .query_circle(Circle2::new(origin, range + 0.8), &mut state.query_scratch);
    let candidate_indices = state.query_scratch.clone();

    let mut hit_any = false;
    for idx_u32 in candidate_indices {
        let idx = idx_u32 as usize;
        let Some((pos, radius, uid)) = state.enemies.get(idx).map(|e| (e.pos, e.radius, e.uid))
        else {
            continue;
        };
        let _ = uid;
        let circle = Circle2::new(pos, radius);
        if circle.overlaps_sector(origin, dir, range, half_arc) {
            let dx = pos[0] - origin[0];
            let dy = pos[1] - origin[1];
            let len = (dx * dx + dy * dy).sqrt().max(0.001);
            let impulse = [dx / len * def.knockback, dy / len * def.knockback];
            apply_damage_to_enemy(
                state,
                world,
                idx,
                damage,
                crit_chance,
                def.crit_multiplier,
                impulse,
            );
            hit_any = true;
        }
    }

    spawn_melee_fx(
        state,
        world,
        slot,
        &def.fx_sprite,
        [
            origin[0] + dir[0] * (range * 0.48),
            origin[1] + dir[1] * (range * 0.48),
        ],
        aim_angle,
        range * 0.72,
    );
    hit_any
}

#[expect(
    clippy::too_many_arguments,
    reason = "melee thrust takes weapon slot, definition, origin, aim, range, damage, and crit"
)]
fn perform_melee_thrust(
    state: &mut SurvivorsState,
    world: &mut GameWorld<'_>,
    slot: usize,
    def: &WeaponDef,
    origin: [f32; 2],
    aim_angle: f32,
    range: f32,
    damage: f32,
    crit_chance: f32,
) -> bool {
    let dir = [aim_angle.cos(), aim_angle.sin()];
    let tip = [origin[0] + dir[0] * range, origin[1] + dir[1] * range];
    state
        .spatial_grid
        .query_circle(Circle2::new(origin, range + 0.8), &mut state.query_scratch);
    let candidate_indices = state.query_scratch.clone();

    let mut hit_any = false;
    for idx_u32 in candidate_indices {
        let idx = idx_u32 as usize;
        let Some((pos, radius)) = state.enemies.get(idx).map(|e| (e.pos, e.radius)) else {
            continue;
        };
        let circle = Circle2::new(pos, radius);
        if circle.overlaps_segment_radius(origin, tip, 0.58) {
            let impulse = [dir[0] * def.knockback, dir[1] * def.knockback];
            apply_damage_to_enemy(
                state,
                world,
                idx,
                damage,
                crit_chance,
                def.crit_multiplier,
                impulse,
            );
            hit_any = true;
        }
    }

    spawn_melee_fx(
        state,
        world,
        slot,
        &def.fx_sprite,
        [
            origin[0] + dir[0] * (range * 0.58),
            origin[1] + dir[1] * (range * 0.58),
        ],
        aim_angle,
        range * 0.65,
    );
    hit_any
}

fn spawn_melee_fx(
    state: &mut SurvivorsState,
    world: &mut GameWorld<'_>,
    slot: usize,
    fx_sprite: &str,
    pos: [f32; 2],
    angle: f32,
    scale: f32,
) {
    if fx_sprite.is_empty() {
        return;
    }
    let Some(existing_fx) = state.weapons.get(slot).map(|w| w.fx_entity) else {
        return;
    };
    let fx_entity = if existing_fx == EntityHandle::PLACEHOLDER || !world.is_alive(existing_fx) {
        state.pool.acquire(world, POOL_MELEE_FX).0
    } else {
        existing_fx
    };
    if let Some(weapon) = state.weapons.get_mut(slot) {
        weapon.fx_entity = fx_entity;
        weapon.fx_timer = 0.14;
    }

    world.set(
        fx_entity,
        LocalTransform {
            position: Vec3::new(pos[0], pos[1], 0.72),
            rotation: Quat::from_rotation_z(angle),
            scale: Vec3::splat(scale.max(0.9)),
        },
    );
    state.config.apply_sprite(
        world,
        fx_entity,
        fx_sprite,
        (15, 0),
        [1.1, 1.1, 1.2, 0.92],
        false,
    );
}

fn step_spawner_and_warnings(state: &mut SurvivorsState, world: &mut GameWorld<'_>, dt: f32) {
    let wave = state.config.wave_for_number(state.wave_index).clone();

    // Trigger wave boss once the wave reaches its midpoint.
    if !state.boss_spawned {
        if let Some(boss_id) = &wave.boss_spawn {
            if state.wave_timer <= wave.duration_seconds * 0.65 {
                state.boss_spawned = true;
                if let Some(boss_idx) = state.config.enemy_index_by_id(boss_id) {
                    let boss_pos = pick_spawn_position(state);
                    spawn_warning(state, world, boss_idx, boss_pos);
                    state.set_banner("ELITE BOSS INCOMING!", 2.2);
                }
            }
        }
    }

    state.spawn_timer -= dt;
    let total_queued = state.enemies.len() + state.spawn_warnings.len();
    if state.spawn_timer <= 0.0 && total_queued < wave.max_active_enemies {
        state.spawn_timer = wave.spawn_interval.max(0.18);
        let available = wave.max_active_enemies.saturating_sub(total_queued);
        let count = (wave.batch_size as usize).min(available);
        for _ in 0..count {
            if let Some(def_idx) = pick_weighted_enemy(state, &wave.enemy_weights) {
                let pos = pick_spawn_position(state);
                spawn_warning(state, world, def_idx, pos);
            }
        }
    }

    let mut ready_spawns = Vec::new();
    let mut i = 0;
    while i < state.spawn_warnings.len() {
        state.spawn_warnings[i].timer -= dt;
        let pulse = 0.85 + 0.25 * (state.spawn_warnings[i].timer * 14.0).sin().abs();
        world.set(
            state.spawn_warnings[i].entity,
            LocalTransform {
                position: Vec3::new(
                    state.spawn_warnings[i].pos[0],
                    state.spawn_warnings[i].pos[1],
                    0.05,
                ),
                rotation: Quat::IDENTITY,
                scale: Vec3::splat(pulse),
            },
        );
        if state.spawn_warnings[i].timer <= 0.0 {
            let w = state.spawn_warnings.swap_remove(i);
            state.pool.release(world, POOL_WARNING, w.entity);
            ready_spawns.push((w.def_index, w.pos));
        } else {
            i += 1;
        }
    }

    for (def_idx, pos) in ready_spawns {
        if let Some(def) = state.config.enemies.get(def_idx) {
            let enemy_id = def.id.clone();
            spawn_enemy_at(state, world, &enemy_id, pos);
        }
    }
}

fn spawn_warning(
    state: &mut SurvivorsState,
    world: &mut GameWorld<'_>,
    def_index: usize,
    pos: [f32; 2],
) {
    let (entity, _) = state.pool.acquire(world, POOL_WARNING);
    world.set(
        entity,
        LocalTransform {
            position: Vec3::new(pos[0], pos[1], 0.05),
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        },
    );
    state.config.apply_sprite(
        world,
        entity,
        "fx_spawn_ring",
        (-2, 0),
        [1.0, 0.45, 0.45, 0.9],
        false,
    );
    state.spawn_warnings.push(SpawnWarning {
        pos,
        timer: 0.55,
        def_index,
        entity,
    });
}

fn pick_spawn_position(state: &mut SurvivorsState) -> [f32; 2] {
    let angle = state.rng.range_f32(0.0, std::f32::consts::TAU);
    let dist = state.rng.range_f32(7.5, 13.5);
    [
        (state.player_pos[0] + angle.cos() * dist)
            .clamp(-ARENA_HALF_W + ARENA_MARGIN, ARENA_HALF_W - ARENA_MARGIN),
        (state.player_pos[1] + angle.sin() * dist)
            .clamp(-ARENA_HALF_H + ARENA_MARGIN, ARENA_HALF_H - ARENA_MARGIN),
    ]
}

fn pick_weighted_enemy(
    state: &mut SurvivorsState,
    weights: &std::collections::BTreeMap<String, u32>,
) -> Option<usize> {
    let total: u32 = weights.values().copied().sum();
    if total == 0 {
        return (!state.config.enemies.is_empty()).then_some(0);
    }
    let roll = (state.rng.next_u64() % u64::from(total)) as u32;
    let mut accum = 0_u32;
    for (id, w) in weights {
        accum += *w;
        if roll < accum {
            if let Some(idx) = state.config.enemy_index_by_id(id) {
                return Some(idx);
            }
        }
    }
    (!state.config.enemies.is_empty()).then_some(0)
}

fn step_projectiles(state: &mut SurvivorsState, world: &mut GameWorld<'_>, dt: f32) {
    let mut player_damaged_this_tick = false;
    let mut hit_enemy_sfx = false;
    let mut i = 0;

    while i < state.projectiles.len() {
        state.projectiles[i].life -= dt;
        state.projectiles[i].pos[0] += state.projectiles[i].vel[0] * dt;
        state.projectiles[i].pos[1] += state.projectiles[i].vel[1] * dt;

        let pos = state.projectiles[i].pos;
        let out_of_bounds = pos[0].abs() > ARENA_HALF_W + 0.5 || pos[1].abs() > ARENA_HALF_H + 0.5;
        if state.projectiles[i].life <= 0.0 || out_of_bounds {
            let proj = state.projectiles.swap_remove(i);
            state.pool.release(world, POOL_PROJECTILE, proj.entity);
            continue;
        }

        world.set(
            state.projectiles[i].entity,
            LocalTransform {
                position: Vec3::new(pos[0], pos[1], 0.65),
                rotation: Quat::from_rotation_z(
                    state.projectiles[i].vel[1].atan2(state.projectiles[i].vel[0]),
                ),
                scale: Vec3::splat(if state.projectiles[i].from_enemy {
                    0.82
                } else {
                    0.72
                }),
            },
        );

        let mut remove_proj = false;
        if state.projectiles[i].from_enemy {
            if Circle2::new(pos, state.projectiles[i].radius)
                .overlaps_circle(Circle2::new(state.player_pos, 0.42))
            {
                if !player_damaged_this_tick {
                    apply_damage_to_player(state, world, state.projectiles[i].damage);
                    player_damaged_this_tick = true;
                }
                remove_proj = true;
            }
        } else {
            let radius = state.projectiles[i].radius;
            state
                .spatial_grid
                .query_circle(Circle2::new(pos, radius + 0.85), &mut state.query_scratch);
            let candidates = state.query_scratch.clone();
            for idx_u32 in candidates {
                let idx = idx_u32 as usize;
                let Some((e_pos, e_rad, e_uid, e_hp)) = state
                    .enemies
                    .get(idx)
                    .map(|e| (e.pos, e.radius, e.uid, e.hp))
                else {
                    continue;
                };
                if e_hp <= 0.0 || state.projectiles[i].hit_enemy_uids.contains(&e_uid) {
                    continue;
                }
                if Circle2::new(pos, radius).overlaps_circle(Circle2::new(e_pos, e_rad)) {
                    let vel = state.projectiles[i].vel;
                    let vlen = (vel[0] * vel[0] + vel[1] * vel[1]).sqrt().max(0.001);
                    let kb = state.projectiles[i].knockback;
                    let impulse = [vel[0] / vlen * kb, vel[1] / vlen * kb];
                    let dmg = state.projectiles[i].damage;
                    let crit_c = state.projectiles[i].crit_chance;
                    let crit_m = state.projectiles[i].crit_mult;

                    state.projectiles[i].hit_enemy_uids.push(e_uid);
                    apply_damage_to_enemy(state, world, idx, dmg, crit_c, crit_m, impulse);
                    if !hit_enemy_sfx {
                        world.play_sound("audio/hit_enemy.wav", 0.25, false);
                        hit_enemy_sfx = true;
                    }

                    if state.projectiles[i].pierce_left == 0 {
                        remove_proj = true;
                        break;
                    } else {
                        state.projectiles[i].pierce_left -= 1;
                    }
                }
            }
        }

        if remove_proj {
            let proj = state.projectiles.swap_remove(i);
            state.pool.release(world, POOL_PROJECTILE, proj.entity);
        } else {
            i += 1;
        }
    }
}

fn step_enemies(state: &mut SurvivorsState, world: &mut GameWorld<'_>, dt: f32) {
    if state.enemies.is_empty() {
        return;
    }

    let player_pos = state.player_pos;
    let mut enemy_shots: Vec<([f32; 2], [f32; 2], f32)> = Vec::new();
    let decay = (-9.0 * dt).exp();

    for enemy in &mut state.enemies {
        if enemy.hit_flash > 0.0 {
            enemy.hit_flash = (enemy.hit_flash - dt).max(0.0);
        }

        let dx = player_pos[0] - enemy.pos[0];
        let dy = player_pos[1] - enemy.pos[1];
        let dist = (dx * dx + dy * dy).sqrt().max(0.001);
        let dir = [dx / dist, dy / dist];

        match enemy.behavior {
            EnemyBehavior::Chaser => {
                enemy.pos[0] += dir[0] * enemy.speed * dt;
                enemy.pos[1] += dir[1] * enemy.speed * dt;
            }
            EnemyBehavior::Charger => {
                if enemy.dash_timer > 0.0 {
                    enemy.dash_timer = (enemy.dash_timer - dt).max(0.0);
                    let dash_speed = enemy.speed * 3.1;
                    enemy.pos[0] += enemy.dash_dir[0] * dash_speed * dt;
                    enemy.pos[1] += enemy.dash_dir[1] * dash_speed * dt;
                } else if enemy.telegraph_timer > 0.0 {
                    enemy.telegraph_timer = (enemy.telegraph_timer - dt).max(0.0);
                    if enemy.telegraph_timer <= 0.0 {
                        enemy.dash_timer = 0.42;
                        enemy.action_timer = 2.1;
                    }
                } else {
                    enemy.pos[0] += dir[0] * enemy.speed * dt;
                    enemy.pos[1] += dir[1] * enemy.speed * dt;
                    enemy.action_timer -= dt;
                    if enemy.action_timer <= 0.0 && dist < 8.5 {
                        enemy.dash_dir = dir;
                        enemy.telegraph_timer = 0.42;
                    }
                }
            }
            EnemyBehavior::RangedShooter => {
                let desired = if dist > 7.5 {
                    1.0
                } else if dist < 4.8 {
                    -0.7
                } else {
                    0.0
                };
                enemy.pos[0] += dir[0] * enemy.speed * desired * dt;
                enemy.pos[1] += dir[1] * enemy.speed * desired * dt;
                enemy.action_timer -= dt;
                if enemy.action_timer <= 0.0 && dist < 11.5 {
                    enemy.action_timer = 2.2;
                    enemy_shots.push((
                        enemy.pos,
                        [dir[0] * 7.2, dir[1] * 7.2],
                        enemy.contact_damage,
                    ));
                }
            }
            EnemyBehavior::Boss => {
                enemy.pos[0] += dir[0] * enemy.speed * dt;
                enemy.pos[1] += dir[1] * enemy.speed * dt;
                enemy.action_timer -= dt;
                if enemy.action_timer <= 0.0 {
                    enemy.action_timer = 2.6;
                    for k in 0..8 {
                        let a = (k as f32) * std::f32::consts::TAU / 8.0;
                        enemy_shots.push((
                            enemy.pos,
                            [a.cos() * 6.4, a.sin() * 6.4],
                            enemy.contact_damage * 0.85,
                        ));
                    }
                }
            }
        }

        enemy.pos[0] += enemy.knockback_vel[0] * dt;
        enemy.pos[1] += enemy.knockback_vel[1] * dt;
        enemy.knockback_vel[0] *= decay;
        enemy.knockback_vel[1] *= decay;
    }

    for (pos, vel, dmg) in enemy_shots {
        spawn_enemy_projectile(state, world, pos, vel, dmg);
    }

    // Crowd separation via engine SpatialGrid2D + resolve_crowd_separation.
    state.crowd_buf.clear();
    for enemy in &state.enemies {
        state.crowd_buf.push(CrowdAgent2D {
            id: enemy.uid,
            position: enemy.pos,
            radius: enemy.radius,
            weight: enemy.weight,
        });
    }
    rebuild_enemy_grid(state);
    resolve_crowd_separation(&mut state.crowd_buf, &mut state.spatial_grid, 0.85, None);

    let mut contact_damage = None;
    for (enemy, separated) in state.enemies.iter_mut().zip(state.crowd_buf.iter()) {
        enemy.pos[0] =
            separated.position[0].clamp(-ARENA_HALF_W + enemy.radius, ARENA_HALF_W - enemy.radius);
        enemy.pos[1] =
            separated.position[1].clamp(-ARENA_HALF_H + enemy.radius, ARENA_HALF_H - enemy.radius);

        let sprite_scale = if enemy.behavior == EnemyBehavior::Boss {
            1.75
        } else if enemy.is_elite {
            1.30
        } else {
            1.05
        };
        world.set(
            enemy.entity,
            LocalTransform {
                position: Vec3::new(enemy.pos[0], enemy.pos[1], 0.3),
                rotation: Quat::IDENTITY,
                scale: Vec3::splat(sprite_scale),
            },
        );

        if let Some(mut sr) = world.get::<SpriteRenderer>(enemy.entity) {
            let desired_flip = enemy.pos[0] > state.player_pos[0];
            let desired_color = if enemy.hit_flash > 0.0 {
                [3.5, 3.5, 3.5, 1.0]
            } else if enemy.telegraph_timer > 0.0 {
                [1.8, 0.55, 0.45, 1.0]
            } else {
                [1.0, 1.0, 1.0, 1.0]
            };
            if sr.flip_x != desired_flip || sr.color != desired_color {
                sr.flip_x = desired_flip;
                sr.color = desired_color;
                world.set(enemy.entity, sr);
            }
        }

        if enemy.hp > 0.0
            && Circle2::new(enemy.pos, enemy.radius)
                .overlaps_circle(Circle2::new(state.player_pos, 0.40))
        {
            contact_damage = Some(contact_damage.map_or(enemy.contact_damage, |cur: f32| {
                cur.max(enemy.contact_damage)
            }));
        }
    }

    if let Some(raw_dmg) = contact_damage {
        apply_damage_to_player(state, world, raw_dmg);
    }
}

fn spawn_enemy_projectile(
    state: &mut SurvivorsState,
    world: &mut GameWorld<'_>,
    pos: [f32; 2],
    vel: [f32; 2],
    damage: f32,
) {
    let (entity, _) = state.pool.acquire(world, POOL_PROJECTILE);
    world.set(
        entity,
        LocalTransform {
            position: Vec3::new(pos[0], pos[1], 0.65),
            rotation: Quat::from_rotation_z(vel[1].atan2(vel[0])),
            scale: Vec3::splat(0.82),
        },
    );
    state.config.apply_sprite(
        world,
        entity,
        "proj_enemy_fire",
        (14, 0),
        [1.0, 1.0, 1.0, 1.0],
        false,
    );
    state.projectiles.push(ActiveProjectile {
        entity,
        pos,
        vel,
        radius: 0.25,
        damage,
        knockback: 0.0,
        crit_chance: 0.0,
        crit_mult: 1.0,
        pierce_left: 0,
        life: 2.6,
        from_enemy: true,
        hit_enemy_uids: Vec::new(),
    });
}

fn apply_damage_to_player(state: &mut SurvivorsState, world: &mut GameWorld<'_>, raw_damage: f32) {
    if state.player_i_frames > 0.0 || state.stats.hp <= 0.0 {
        return;
    }
    if state.rng.next_f32() * 100.0 < state.stats.dodge_pct {
        state.player_i_frames = 0.28;
        state.damage_numbers.push(DamageNumber {
            world_pos: [state.player_pos[0], state.player_pos[1] + 0.9],
            vel_y: 1.4,
            age: 0.0,
            duration: 0.55,
            amount: 0,
            is_crit: false,
            is_player_hit: true,
        });
        return;
    }

    let reduced = (raw_damage * (1.0 - state.stats.armor_reduction())).max(1.0);
    state.stats.hp = (state.stats.hp - reduced).max(0.0);
    state.player_i_frames = 0.50;
    state.player_hit_flash = 0.14;
    state.damage_numbers.push(DamageNumber {
        world_pos: [state.player_pos[0], state.player_pos[1] + 0.95],
        vel_y: 1.7,
        age: 0.0,
        duration: 0.70,
        amount: reduced.round().max(1.0) as u32,
        is_crit: false,
        is_player_hit: true,
    });
    trigger_hit_burst(
        state,
        world,
        state.player_pos,
        [1.0, 0.25, 0.25, 1.0],
        [0.8, 0.1, 0.1, 0.0],
        10,
    );
    world.play_sound("audio/player_hurt.wav", 0.50, false);
}

fn cleanup_dead_enemies(state: &mut SurvivorsState, world: &mut GameWorld<'_>) {
    let mut any_died = false;
    let mut i = 0;
    while i < state.enemies.len() {
        if state.enemies[i].hp > 0.0 {
            i += 1;
            continue;
        }
        any_died = true;
        let dead = state.enemies.swap_remove(i);
        state.pool.release(world, POOL_ENEMY, dead.entity);
        if dead.hp_bg_entity != EntityHandle::PLACEHOLDER {
            state.pool.release(world, POOL_WHITE_BAR, dead.hp_bg_entity);
        }
        if dead.hp_fill_entity != EntityHandle::PLACEHOLDER {
            state
                .pool
                .release(world, POOL_WHITE_BAR, dead.hp_fill_entity);
        }
        state.stats.kills = state.stats.kills.saturating_add(1);

        if let Some(def) = state.config.enemies.get(dead.def_index).cloned() {
            let xp_kind = if def.xp_value >= 8 {
                PickupKind::XpLarge(def.xp_value)
            } else {
                PickupKind::XpSmall(def.xp_value)
            };
            spawn_pickup_at(state, world, xp_kind, dead.pos);

            let luck_bonus = (state.stats.luck / 250.0).clamp(0.0, 0.25);
            if state.rng.next_f32() < def.gold_chance + luck_bonus {
                spawn_pickup_at(
                    state,
                    world,
                    PickupKind::Gold(def.gold_value),
                    [dead.pos[0] + 0.28, dead.pos[1] - 0.18],
                );
            }
            if state.rng.next_f32() < def.heal_chance + luck_bonus * 0.25 {
                spawn_pickup_at(
                    state,
                    world,
                    PickupKind::Heal(12.0),
                    [dead.pos[0] - 0.25, dead.pos[1] + 0.2],
                );
            }
        }
    }

    if any_died {
        world.play_sound("audio/enemy_die.wav", 0.32, false);
    }
}

fn step_pickups(state: &mut SurvivorsState, world: &mut GameWorld<'_>, dt: f32, magnet_speed: f32) {
    let radius_sq = state.stats.pickup_radius * state.stats.pickup_radius;
    let mut play_sfx = true;
    let mut i = 0;

    while i < state.pickups.len() {
        let dx = state.player_pos[0] - state.pickups[i].pos[0];
        let dy = state.player_pos[1] - state.pickups[i].pos[1];
        let dist_sq = dx * dx + dy * dy;

        if dist_sq <= radius_sq {
            state.pickups[i].magnetized = true;
        }

        if state.pickups[i].magnetized && dist_sq > 0.0001 {
            let dist = dist_sq.sqrt();
            state.pickups[i].speed = (state.pickups[i].speed + 36.0 * dt).min(magnet_speed);
            let step_len = (state.pickups[i].speed * dt).min(dist);
            state.pickups[i].pos[0] += (dx / dist) * step_len;
            state.pickups[i].pos[1] += (dy / dist) * step_len;
            let p = state.pickups[i].pos;
            let scale = match state.pickups[i].kind {
                PickupKind::XpLarge(_) | PickupKind::Heal(_) => 0.88,
                _ => 0.72,
            };
            world.set(
                state.pickups[i].entity,
                LocalTransform {
                    position: Vec3::new(p[0], p[1], 0.1),
                    rotation: Quat::IDENTITY,
                    scale: Vec3::splat(scale),
                },
            );
        }

        let ndx = state.player_pos[0] - state.pickups[i].pos[0];
        let ndy = state.player_pos[1] - state.pickups[i].pos[1];
        if ndx * ndx + ndy * ndy <= 0.65 * 0.65 {
            let pickup = state.pickups.swap_remove(i);
            collect_pickup(state, world, pickup.kind, play_sfx);
            play_sfx = false;
            state.pool.release(world, POOL_PICKUP, pickup.entity);
        } else {
            i += 1;
        }
    }
}

fn collect_pickup(
    state: &mut SurvivorsState,
    world: &mut GameWorld<'_>,
    kind: PickupKind,
    play_sfx: bool,
) {
    match kind {
        PickupKind::XpSmall(v) | PickupKind::XpLarge(v) => {
            let gained = state.stats.grant_xp(v);
            state.pending_level_ups = state.pending_level_ups.saturating_add(gained);
            if play_sfx {
                world.play_sound("audio/pickup_gem.wav", 0.28, false);
            }
        }
        PickupKind::Gold(v) => {
            state.stats.gold = state.stats.gold.saturating_add(v);
            if play_sfx {
                world.play_sound("audio/pickup_coin.wav", 0.34, false);
            }
        }
        PickupKind::Heal(amount) => {
            state.stats.hp = (state.stats.hp + amount).min(state.stats.max_hp);
            if play_sfx {
                world.play_sound("audio/pickup_gem.wav", 0.32, false);
            }
        }
    }
}
