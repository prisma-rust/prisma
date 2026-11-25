//! Screen-space HUD, floating damage numbers, world-space health bars, and interactive shop/upgrade UI.

use glam::{Quat, Vec3};
use prism_game_api::{
    keys, world_to_screen_2d, EntityHandle, GameWorld, InputSnapshot, TextAlign, UiQuad, UiText,
};
use prism_scene::components::{LocalTransform, SpriteRenderer};

use crate::config::{StatModifiers, WeaponKind};
use crate::flow_ui;
use crate::i18n::tr;
use crate::state::{GamePhase, ShopOfferKind, SurvivorsState};

const HP_BAR_SCALE_X: f32 = 1.28;

/// Variable-timestep presentation update: camera sync, HP bars, damage numbers, HUD, and modals.
pub fn frame_step(state: &mut SurvivorsState, world: &mut GameWorld<'_>, dt: f32) {
    let clamped_dt = dt.clamp(0.0, 0.1);
    if clamped_dt > 0.0 {
        state.last_dt = clamped_dt;
    }
    sync_camera_pos(state, world);

    let in_front_end = matches!(state.phase, GamePhase::Splash | GamePhase::MainMenu);
    flow_ui::sync_player_hp_visibility(state, world, !in_front_end);
    world.set_camera_input_suppressed(!matches!(
        state.phase,
        GamePhase::WaveCombat | GamePhase::WaveClearSweep { .. }
    ));

    if !in_front_end {
        sync_world_health_bars(state, world);
        step_and_draw_damage_numbers(state, world, clamped_dt);
    }

    let input = world.input();
    match state.phase.clone() {
        GamePhase::Splash => {
            flow_ui::handle_and_draw_splash(state, world, &input, clamped_dt);
        }
        GamePhase::MainMenu => {
            flow_ui::handle_and_draw_main_menu(state, world, &input);
        }
        GamePhase::Paused { from_sweep_tenths } => {
            draw_combat_hud(state, world, &input);
            flow_ui::handle_and_draw_paused(state, world, &input, from_sweep_tenths);
        }
        GamePhase::LevelUpChoice {
            choices,
            pending_levels,
        } => {
            draw_combat_hud(state, world, &input);
            handle_and_draw_level_up(state, world, &input, &choices, pending_levels);
        }
        GamePhase::ShopPhase {
            offers,
            reroll_cost,
            reroll_count,
        } => {
            draw_combat_hud(state, world, &input);
            handle_and_draw_shop(state, world, &input, &offers, reroll_cost, reroll_count);
        }
        GamePhase::Victory => {
            state.record_run_end(true, world);
            draw_combat_hud(state, world, &input);
            handle_and_draw_end_screen(state, world, &input, true);
        }
        GamePhase::GameOver => {
            state.record_run_end(false, world);
            draw_combat_hud(state, world, &input);
            handle_and_draw_end_screen(state, world, &input, false);
        }
        GamePhase::WaveCombat => {
            if input.key_pressed(keys::ESCAPE) {
                state.pause_cursor = 0;
                state.phase = GamePhase::Paused {
                    from_sweep_tenths: None,
                };
                draw_combat_hud(state, world, &input);
                let idle_input = InputSnapshot {
                    screen: input.screen,
                    ..InputSnapshot::default()
                };
                flow_ui::handle_and_draw_paused(state, world, &idle_input, None);
            } else {
                draw_combat_hud(state, world, &input);
            }
        }
        GamePhase::WaveClearSweep { timer_tenths } => {
            if input.key_pressed(keys::ESCAPE) {
                state.pause_cursor = 0;
                state.phase = GamePhase::Paused {
                    from_sweep_tenths: Some(timer_tenths),
                };
                draw_combat_hud(state, world, &input);
                let idle_input = InputSnapshot {
                    screen: input.screen,
                    ..InputSnapshot::default()
                };
                flow_ui::handle_and_draw_paused(state, world, &idle_input, Some(timer_tenths));
            } else {
                draw_combat_hud(state, world, &input);
            }
        }
    }

    flow_ui::draw_api_telemetry_strip(state, world, &input);
}

fn sync_camera_pos(state: &mut SurvivorsState, world: &GameWorld<'_>) {
    if let Some(camera) = world.find_by_name("camera") {
        if let Some(tf) = world.get::<LocalTransform>(camera) {
            state.camera_pos = [tf.position.x, tf.position.y];
        }
    }
}

fn sync_world_health_bars(state: &SurvivorsState, world: &mut GameWorld<'_>) {
    if state.player_hp_bg != EntityHandle::PLACEHOLDER && world.is_alive(state.player_hp_bg) {
        let bx = state.player_pos[0];
        let by = state.player_pos[1] + 0.92;
        world.set(
            state.player_hp_bg,
            LocalTransform {
                position: Vec3::new(bx, by, 1.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::new(1.36, 0.16, 1.0),
            },
        );
        if world.is_alive(state.player_hp_fill) {
            let ratio = (state.stats.hp / state.stats.max_hp.max(1.0)).clamp(0.0, 1.0);
            let fill_w = (HP_BAR_SCALE_X * ratio).max(0.001);
            let fill_x = bx - (HP_BAR_SCALE_X - fill_w) * 0.5;
            world.set(
                state.player_hp_fill,
                LocalTransform {
                    position: Vec3::new(fill_x, by, 1.1),
                    rotation: Quat::IDENTITY,
                    scale: Vec3::new(fill_w, 0.10, 1.0),
                },
            );
            let bar_color = if ratio > 0.5 {
                [0.20, 0.88, 0.34, 0.95]
            } else if ratio > 0.25 {
                [0.95, 0.72, 0.18, 0.95]
            } else {
                [0.95, 0.24, 0.24, 0.95]
            };
            if world
                .get::<SpriteRenderer>(state.player_hp_fill)
                .is_none_or(|sr| sr.color != bar_color)
            {
                world.set(
                    state.player_hp_fill,
                    SpriteRenderer {
                        sorting_layer: 20,
                        order_in_layer: 1,
                        ..SpriteRenderer::white(bar_color)
                    },
                );
            }
        }
    }

    for enemy in &state.enemies {
        if enemy.hp_bg_entity == EntityHandle::PLACEHOLDER
            || enemy.hp_fill_entity == EntityHandle::PLACEHOLDER
        {
            continue;
        }
        let bar_w = (enemy.radius * 2.2).clamp(1.2, 2.8);
        let ratio = (enemy.hp / enemy.max_hp.max(1.0)).clamp(0.0, 1.0);
        let ex = enemy.pos[0];
        let ey = enemy.pos[1] + enemy.radius + 0.45;
        world.set(
            enemy.hp_bg_entity,
            LocalTransform {
                position: Vec3::new(ex, ey, 1.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::new(bar_w + 0.08, 0.16, 1.0),
            },
        );
        let fill_w = (bar_w * ratio).max(0.001);
        world.set(
            enemy.hp_fill_entity,
            LocalTransform {
                position: Vec3::new(ex - (bar_w - fill_w) * 0.5, ey, 1.1),
                rotation: Quat::IDENTITY,
                scale: Vec3::new(fill_w, 0.10, 1.0),
            },
        );
    }

    for weapon in &state.weapons {
        if weapon.entity != EntityHandle::PLACEHOLDER && world.is_alive(weapon.entity) {
            if let Some(mut sr) = world.get::<SpriteRenderer>(weapon.entity) {
                let desired = tier_world_tint(weapon.tier);
                if sr.color != desired {
                    sr.color = desired;
                    world.set(weapon.entity, sr);
                }
            }
        }
    }
}

fn step_and_draw_damage_numbers(state: &mut SurvivorsState, world: &mut GameWorld<'_>, dt: f32) {
    let screen = world.input().screen;
    let can_draw = screen[0] > 64.0 && screen[1] > 64.0;
    let mut i = 0;
    while i < state.damage_numbers.len() {
        state.damage_numbers[i].age += dt;
        state.damage_numbers[i].world_pos[1] += state.damage_numbers[i].vel_y * dt;
        state.damage_numbers[i].vel_y *= (1.0 - 2.2 * dt).max(0.0);

        if state.damage_numbers[i].age >= state.damage_numbers[i].duration {
            let recycled = state.damage_numbers.swap_remove(i);
            state.damage_number_pool.push(recycled);
            continue;
        }

        if can_draw && state.settings.show_damage_numbers {
            let dn = &state.damage_numbers[i];
            let px = world_to_screen_2d(
                dn.world_pos,
                state.camera_pos,
                state.ortho_half_height,
                screen,
            );
            let alpha = ((1.0 - dn.age / dn.duration) * 1.4).clamp(0.0, 1.0);
            let (color, size) = if dn.is_player_hit && dn.amount == 0 {
                ([0.35, 0.92, 1.0, alpha], 18.0)
            } else if dn.is_player_hit {
                ([1.0, 0.28, 0.28, alpha], 19.0)
            } else if dn.is_crit {
                ([1.0, 0.88, 0.20, alpha], 22.0)
            } else {
                ([0.97, 0.97, 0.97, alpha], 16.0)
            };
            world.draw_text(UiText {
                rect: [px[0] - 45.0, px[1] - 14.0, 90.0, 28.0],
                content: dn.label.clone(),
                font_size: size,
                color,
                alignment: TextAlign::Center,
                outline_width: 2.0,
                outline_color: [0.04, 0.04, 0.06, alpha],
                layer: 5,
                ..UiText::default()
            });
        }
        i += 1;
    }
}

fn draw_combat_hud(state: &SurvivorsState, world: &mut GameWorld<'_>, input: &InputSnapshot) {
    let [sw, sh] = input.screen;
    if sw < 200.0 || sh < 200.0 {
        return;
    }

    draw_panel(
        world,
        [16.0, 16.0, 296.0, 118.0],
        [0.06, 0.08, 0.12, 0.84],
        8.0,
        1,
    );

    let hp_ratio = (state.stats.hp / state.stats.max_hp.max(1.0)).clamp(0.0, 1.0);
    draw_panel(
        world,
        [28.0, 28.0, 272.0, 22.0],
        [0.14, 0.08, 0.10, 0.95],
        4.0,
        2,
    );
    if hp_ratio > 0.0 {
        draw_panel(
            world,
            [28.0, 28.0, 272.0 * hp_ratio, 22.0],
            [0.84, 0.20, 0.24, 0.96],
            4.0,
            2,
        );
    }
    let lang = state.settings.language;
    let hp_text = if lang.is_zh() {
        format!(
            "生命  {} / {}  (+{:.1}/秒)",
            state.stats.hp.ceil() as u32,
            state.stats.max_hp.round() as u32,
            state.stats.hp_regen
        )
    } else {
        format!(
            "HP  {} / {}  (+{:.1}/s)",
            state.stats.hp.ceil() as u32,
            state.stats.max_hp.round() as u32,
            state.stats.hp_regen
        )
    };
    draw_label(
        world,
        [28.0, 29.0, 272.0, 20.0],
        &hp_text,
        14.0,
        [1.0, 1.0, 1.0, 1.0],
        TextAlign::Center,
        3,
    );

    let xp_ratio = (state.stats.xp as f32 / state.stats.xp_to_next.max(1) as f32).clamp(0.0, 1.0);
    draw_panel(
        world,
        [28.0, 56.0, 272.0, 18.0],
        [0.08, 0.12, 0.18, 0.95],
        4.0,
        2,
    );
    if xp_ratio > 0.0 {
        draw_panel(
            world,
            [28.0, 56.0, 272.0 * xp_ratio, 18.0],
            [0.24, 0.68, 0.96, 0.96],
            4.0,
            2,
        );
    }
    let xp_text = if lang.is_zh() {
        format!(
            "等级 {}   经验 {} / {}",
            state.stats.level, state.stats.xp, state.stats.xp_to_next
        )
    } else {
        format!(
            "LV {}   XP {} / {}",
            state.stats.level, state.stats.xp, state.stats.xp_to_next
        )
    };
    draw_label(
        world,
        [28.0, 56.0, 272.0, 18.0],
        &xp_text,
        13.0,
        [0.95, 0.98, 1.0, 1.0],
        TextAlign::Center,
        3,
    );

    let gold_text = if lang.is_zh() {
        format!("金币: {}G", state.stats.gold)
    } else {
        format!("GOLD: {}G", state.stats.gold)
    };
    draw_label(
        world,
        [28.0, 82.0, 136.0, 22.0],
        &gold_text,
        16.0,
        [1.0, 0.84, 0.26, 1.0],
        TextAlign::Left,
        3,
    );
    let kills_text = if lang.is_zh() {
        format!("击杀: {}", state.stats.kills)
    } else {
        format!("KILLS: {}", state.stats.kills)
    };
    draw_label(
        world,
        [164.0, 82.0, 136.0, 22.0],
        &kills_text,
        16.0,
        [0.92, 0.94, 0.98, 1.0],
        TextAlign::Right,
        3,
    );
    let stats_line = if lang.is_zh() {
        format!(
            "伤害 {:+.0}%  攻速 {:+.0}%  暴击 {:.0}%  护甲 {:.0}",
            state.stats.damage_pct,
            state.stats.attack_speed_pct,
            state.stats.crit_chance_pct,
            state.stats.armor
        )
    } else {
        format!(
            "DMG {:+.0}%  SPD {:+.0}%  CRIT {:.0}%  ARM {:.0}",
            state.stats.damage_pct,
            state.stats.attack_speed_pct,
            state.stats.crit_chance_pct,
            state.stats.armor
        )
    };
    draw_label(
        world,
        [28.0, 106.0, 272.0, 20.0],
        &stats_line,
        12.0,
        [0.74, 0.80, 0.88, 0.95],
        TextAlign::Left,
        3,
    );

    let cx = sw * 0.5;
    draw_panel(
        world,
        [cx - 110.0, 16.0, 220.0, 60.0],
        [0.06, 0.08, 0.12, 0.84],
        10.0,
        1,
    );
    let wave_text = if lang.is_zh() {
        format!(
            "第 {} / {} 波",
            state.wave_index,
            state.config.total_waves()
        )
    } else {
        format!("WAVE {} / {}", state.wave_index, state.config.total_waves())
    };
    draw_label(
        world,
        [cx - 105.0, 20.0, 210.0, 20.0],
        &wave_text,
        14.0,
        [0.82, 0.88, 0.96, 1.0],
        TextAlign::Center,
        3,
    );
    let (timer_label, timer_color) = match state.phase {
        GamePhase::WaveClearSweep { .. } => (
            tr(lang, "清场吸取!", "VACUUM!").to_string(),
            [0.42, 0.92, 1.0, 1.0],
        ),
        GamePhase::ShopPhase { .. } => (
            tr(lang, "商店开放", "SHOP OPEN").to_string(),
            [1.0, 0.84, 0.26, 1.0],
        ),
        GamePhase::LevelUpChoice { .. } => (
            tr(lang, "升级!", "LEVEL UP!").to_string(),
            [0.42, 0.96, 0.52, 1.0],
        ),
        GamePhase::Victory => (
            tr(lang, "胜利!", "VICTORY!").to_string(),
            [1.0, 0.86, 0.24, 1.0],
        ),
        GamePhase::GameOver => (
            tr(lang, "失败", "DEFEATED").to_string(),
            [0.96, 0.28, 0.28, 1.0],
        ),
        GamePhase::WaveCombat => {
            let secs = state.wave_timer.ceil() as u32;
            let color = if secs <= 5 {
                [1.0, 0.36, 0.32, 1.0]
            } else {
                [1.0, 1.0, 1.0, 1.0]
            };
            (format!("{secs:02}s"), color)
        }
        GamePhase::Splash => ("SPLASH".to_string(), [1.0, 1.0, 1.0, 1.0]),
        GamePhase::MainMenu => ("READY".to_string(), [1.0, 1.0, 1.0, 1.0]),
        GamePhase::Paused { .. } => ("PAUSED".to_string(), [1.0, 0.84, 0.26, 1.0]),
    };
    draw_label(
        world,
        [cx - 105.0, 41.0, 210.0, 32.0],
        &timer_label,
        25.0,
        timer_color,
        TextAlign::Center,
        3,
    );

    if let Some((banner, _)) = &state.status_banner {
        draw_panel(
            world,
            [cx - 190.0, 84.0, 380.0, 30.0],
            [0.08, 0.10, 0.16, 0.82],
            6.0,
            2,
        );
        draw_label(
            world,
            [cx - 186.0, 88.0, 372.0, 24.0],
            banner,
            15.0,
            [1.0, 0.90, 0.38, 1.0],
            TextAlign::Center,
            3,
        );
    }

    let slot_w = 112.0;
    let slot_h = 62.0;
    let gap = 8.0;
    let base_y = sh - slot_h - 16.0;
    for slot in 0..6 {
        let sx = 16.0 + (slot as f32) * (slot_w + gap);
        if let Some(w) = state.weapons.get(slot) {
            let border = tier_ui_color(w.tier);
            draw_panel(world, [sx, base_y, slot_w, slot_h], border, 6.0, 1);
            draw_panel(
                world,
                [sx + 2.0, base_y + 2.0, slot_w - 4.0, slot_h - 4.0],
                [0.08, 0.10, 0.15, 0.92],
                5.0,
                1,
            );
            let title = state
                .config
                .weapon_by_id(&w.weapon_id)
                .map_or(w.weapon_id.as_str(), |d| d.name.as_str());
            draw_label(
                world,
                [sx + 6.0, base_y + 6.0, slot_w - 12.0, 18.0],
                title,
                12.0,
                [0.96, 0.96, 0.98, 1.0],
                TextAlign::Left,
                3,
            );
            draw_label(
                world,
                [sx + 6.0, base_y + 24.0, slot_w - 12.0, 16.0],
                &format!("Tier {}", tier_numeral(w.tier)),
                12.0,
                border,
                TextAlign::Left,
                3,
            );
            let ready = 1.0 - (w.cooldown_remaining / w.total_cooldown.max(0.01)).clamp(0.0, 1.0);
            draw_panel(
                world,
                [sx + 6.0, base_y + slot_h - 14.0, slot_w - 12.0, 6.0],
                [0.14, 0.16, 0.22, 0.95],
                2.0,
                2,
            );
            draw_panel(
                world,
                [
                    sx + 6.0,
                    base_y + slot_h - 14.0,
                    (slot_w - 12.0) * ready,
                    6.0,
                ],
                [0.36, 0.86, 0.56, 0.95],
                2.0,
                2,
            );
        } else {
            draw_panel(
                world,
                [sx, base_y, slot_w, slot_h],
                [0.06, 0.07, 0.10, 0.62],
                6.0,
                1,
            );
            draw_label(
                world,
                [sx, base_y + 20.0, slot_w, 20.0],
                &format!("SLOT {}", slot + 1),
                12.0,
                [0.45, 0.48, 0.56, 0.7],
                TextAlign::Center,
                2,
            );
        }
    }
}

fn handle_and_draw_level_up(
    state: &mut SurvivorsState,
    world: &mut GameWorld<'_>,
    input: &InputSnapshot,
    choices: &[String],
    pending_levels: u32,
) {
    let key_choice = [keys::DIGIT_1, keys::DIGIT_2, keys::DIGIT_3, keys::DIGIT_4]
        .iter()
        .position(|k| input.key_pressed(*k));
    if let Some(idx) = key_choice {
        if idx < choices.len() {
            state.select_level_up_choice(idx, world);
            return;
        }
    }

    let [sw, sh] = input.screen;
    if sw < 320.0 || sh < 240.0 {
        return;
    }

    draw_panel(world, [0.0, 0.0, sw, sh], [0.03, 0.04, 0.07, 0.78], 0.0, 10);

    let cx = sw * 0.5;
    let top_y = (sh * 0.5 - 175.0).max(36.0);
    let extra = if pending_levels > 0 {
        format!(" (+{pending_levels} MORE)")
    } else {
        String::new()
    };
    draw_label(
        world,
        [cx - 340.0, top_y, 680.0, 38.0],
        &format!("LEVEL UP!  LV {}{}", state.stats.level, extra),
        30.0,
        [1.0, 0.88, 0.28, 1.0],
        TextAlign::Center,
        11,
    );
    draw_label(
        world,
        [cx - 340.0, top_y + 38.0, 680.0, 22.0],
        "CLICK A CARD OR PRESS [1] - [4] TO CHOOSE AN UPGRADE",
        14.0,
        [0.80, 0.86, 0.94, 0.95],
        TextAlign::Center,
        11,
    );

    let count = choices.len().max(1);
    let card_w = ((sw - 120.0) / count as f32 - 18.0).clamp(170.0, 235.0);
    let card_h = 225.0;
    let gap = 18.0;
    let total_w = card_w * count as f32 + gap * (count.saturating_sub(1)) as f32;
    let start_x = cx - total_w * 0.5;
    let card_y = top_y + 78.0;

    let mut clicked_idx = None;
    for (idx, upg_id) in choices.iter().enumerate() {
        let rx = start_x + idx as f32 * (card_w + gap);
        let rect = [rx, card_y, card_w, card_h];
        let hovered = point_in_rect(input.cursor, rect);
        if hovered && input.left_clicked {
            clicked_idx = Some(idx);
        }

        let border = if hovered {
            [1.0, 0.86, 0.30, 1.0]
        } else {
            [0.32, 0.44, 0.62, 0.95]
        };
        let fill = if hovered {
            [0.14, 0.18, 0.26, 0.96]
        } else {
            [0.09, 0.12, 0.18, 0.94]
        };
        draw_panel(world, rect, border, 10.0, 11);
        draw_panel(world, inset(rect, 2.5), fill, 8.0, 11);

        draw_label(
            world,
            [rx + 12.0, card_y + 12.0, card_w - 24.0, 18.0],
            &format!("OPTION [{}]", idx + 1),
            12.0,
            [0.64, 0.78, 0.96, 1.0],
            TextAlign::Center,
            12,
        );

        let (name, desc, mod_text) = state
            .config
            .upgrade_by_id(upg_id)
            .map(|u| {
                (
                    u.name.clone(),
                    u.description.clone(),
                    summarize_modifiers(&u.modifiers),
                )
            })
            .unwrap_or_else(|| (upg_id.clone(), String::new(), String::new()));

        draw_label(
            world,
            [rx + 12.0, card_y + 38.0, card_w - 24.0, 28.0],
            &name,
            18.0,
            [1.0, 1.0, 1.0, 1.0],
            TextAlign::Center,
            12,
        );
        draw_wrapped_label(
            world,
            [rx + 14.0, card_y + 76.0, card_w - 28.0, 62.0],
            &desc,
            13.0,
            [0.78, 0.84, 0.90, 0.95],
            12,
        );
        draw_wrapped_label(
            world,
            [rx + 14.0, card_y + 136.0, card_w - 28.0, 42.0],
            &mod_text,
            13.0,
            [0.42, 0.94, 0.56, 1.0],
            12,
        );
        draw_button(
            world,
            [rx + 24.0, card_y + card_h - 42.0, card_w - 48.0, 28.0],
            "CHOOSE",
            hovered,
            true,
            [0.22, 0.56, 0.32, 1.0],
            12,
        );
    }

    if let Some(idx) = clicked_idx {
        state.select_level_up_choice(idx, world);
    }
}

fn handle_and_draw_shop(
    state: &mut SurvivorsState,
    world: &mut GameWorld<'_>,
    input: &InputSnapshot,
    offers: &[crate::state::ShopOffer],
    reroll_cost: u32,
    reroll_count: u32,
) {
    for (idx, k) in [keys::DIGIT_1, keys::DIGIT_2, keys::DIGIT_3, keys::DIGIT_4]
        .iter()
        .enumerate()
    {
        if input.key_pressed(*k) {
            state.try_buy_shop_offer(idx, world);
            return;
        }
    }
    if input.key_pressed(keys::KEY_Q) {
        state.try_reroll_shop(world);
        return;
    }
    if input.key_pressed(keys::KEY_E) {
        state.try_combine_weapons(world);
        return;
    }
    if input.key_pressed(keys::SPACE) || input.key_pressed(keys::ENTER) {
        state.start_next_wave(world);
        return;
    }

    let [sw, sh] = input.screen;
    if sw < 400.0 || sh < 320.0 {
        return;
    }

    draw_panel(world, [0.0, 0.0, sw, sh], [0.03, 0.04, 0.07, 0.86], 0.0, 10);

    let margin = 24.0;
    let sidebar_w = 248.0;
    let main_w = (sw - margin * 3.0 - sidebar_w).max(520.0);
    let top_y = 22.0;

    draw_panel(
        world,
        [margin, top_y, sw - margin * 2.0, 56.0],
        [0.09, 0.12, 0.18, 0.96],
        8.0,
        11,
    );
    let lang = state.settings.language;
    let shop_title = if lang.is_zh() {
        format!("波次商店  -  第 {} 波完成", state.wave_index)
    } else {
        format!("SHOP  -  AFTER WAVE {}", state.wave_index)
    };
    draw_label(
        world,
        [margin + 18.0, top_y + 14.0, 380.0, 28.0],
        &shop_title,
        22.0,
        [1.0, 1.0, 1.0, 1.0],
        TextAlign::Left,
        12,
    );
    let shop_gold = if lang.is_zh() {
        format!(
            "金币: {}G   |   等级 {}",
            state.stats.gold, state.stats.level
        )
    } else {
        format!("GOLD: {}G   |   LV {}", state.stats.gold, state.stats.level)
    };
    draw_label(
        world,
        [margin + 380.0, top_y + 14.0, 240.0, 28.0],
        &shop_gold,
        20.0,
        [1.0, 0.84, 0.26, 1.0],
        TextAlign::Center,
        12,
    );

    let next_btn = [sw - margin - 210.0, top_y + 10.0, 196.0, 36.0];
    let next_hover = point_in_rect(input.cursor, next_btn);
    draw_button(
        world,
        next_btn,
        if state.wave_index >= state.config.total_waves() {
            tr(lang, "完成挑战 [SPACE]", "FINISH RUN [SPACE]")
        } else {
            tr(lang, "下一波 [SPACE]", "NEXT WAVE [SPACE]")
        },
        next_hover,
        true,
        [0.18, 0.62, 0.34, 1.0],
        12,
    );
    if next_hover && input.left_clicked {
        state.start_next_wave(world);
        return;
    }

    let cards_y = top_y + 72.0;
    let card_gap = 12.0;
    let card_w = (main_w - card_gap * 3.0) / 4.0;
    let card_h = (sh - cards_y - 210.0).clamp(185.0, 250.0);

    let mut buy_slot = None;
    let mut lock_slot = None;

    for (slot, offer) in offers.iter().enumerate() {
        let rx = margin + (slot as f32) * (card_w + card_gap);
        let card_rect = [rx, cards_y, card_w, card_h];
        let (title, badge, desc, accent) = describe_shop_offer(state, offer);
        draw_panel(world, card_rect, accent, 8.0, 11);
        draw_panel(
            world,
            inset(card_rect, 2.0),
            [0.09, 0.11, 0.17, 0.96],
            6.0,
            11,
        );

        draw_label(
            world,
            [rx + 10.0, cards_y + 10.0, card_w - 20.0, 16.0],
            &format!("[{}] {}", slot + 1, badge),
            12.0,
            accent,
            TextAlign::Center,
            12,
        );
        draw_label(
            world,
            [rx + 10.0, cards_y + 32.0, card_w - 20.0, 24.0],
            &title,
            16.0,
            [1.0, 1.0, 1.0, 1.0],
            TextAlign::Center,
            12,
        );
        draw_wrapped_label(
            world,
            [rx + 12.0, cards_y + 62.0, card_w - 24.0, card_h - 142.0],
            &desc,
            12.0,
            [0.80, 0.86, 0.92, 0.95],
            12,
        );

        let buy_rect = [rx + 10.0, cards_y + card_h - 70.0, card_w - 20.0, 28.0];
        let can_afford = !offer.purchased && state.stats.gold >= offer.price;
        let buy_hover = point_in_rect(input.cursor, buy_rect);
        let buy_label = if offer.purchased {
            "SOLD OUT".to_string()
        } else {
            format!("BUY  {}G", offer.price)
        };
        draw_button(
            world,
            buy_rect,
            &buy_label,
            buy_hover,
            can_afford,
            [0.20, 0.52, 0.78, 1.0],
            12,
        );
        if buy_hover && input.left_clicked && !offer.purchased {
            buy_slot = Some(slot);
        }

        let lock_rect = [rx + 10.0, cards_y + card_h - 36.0, card_w - 20.0, 24.0];
        let lock_hover = point_in_rect(input.cursor, lock_rect);
        draw_button(
            world,
            lock_rect,
            if offer.locked { "LOCKED" } else { "LOCK" },
            lock_hover,
            !offer.purchased,
            if offer.locked {
                [0.72, 0.54, 0.14, 1.0]
            } else {
                [0.24, 0.28, 0.36, 1.0]
            },
            12,
        );
        if lock_hover && input.left_clicked && !offer.purchased {
            lock_slot = Some(slot);
        }
    }

    if let Some(slot) = buy_slot {
        state.try_buy_shop_offer(slot, world);
        return;
    }
    if let Some(slot) = lock_slot {
        state.try_toggle_shop_lock(slot);
        return;
    }

    let actions_y = cards_y + card_h + 12.0;
    let reroll_btn = [margin, actions_y, 240.0, 34.0];
    let reroll_hover = point_in_rect(input.cursor, reroll_btn);
    draw_button(
        world,
        reroll_btn,
        &format!("REROLL ({reroll_cost}G)  [Q] (#{reroll_count})"),
        reroll_hover,
        state.stats.gold >= reroll_cost,
        [0.44, 0.36, 0.72, 1.0],
        12,
    );
    if reroll_hover && input.left_clicked {
        state.try_reroll_shop(world);
        return;
    }

    let can_merge = state.has_combinable_weapons();
    let merge_btn = [margin + 254.0, actions_y, 280.0, 34.0];
    let merge_hover = point_in_rect(input.cursor, merge_btn);
    draw_button(
        world,
        merge_btn,
        if can_merge {
            "SYNTHESIZE IDENTICAL [E]"
        } else {
            "NEED 2 IDENTICAL WEAPONS [E]"
        },
        merge_hover,
        can_merge,
        [0.74, 0.52, 0.14, 1.0],
        12,
    );
    if merge_hover && input.left_clicked && can_merge {
        state.try_combine_weapons(world);
        return;
    }

    let eq_y = actions_y + 48.0;
    let eq_w = (main_w - card_gap * 5.0) / 6.0;
    let eq_h = 84.0;
    draw_label(
        world,
        [margin, eq_y - 18.0, 340.0, 16.0],
        &format!("EQUIPPED WEAPONS ({} / 6)", state.weapons.len()),
        13.0,
        [0.84, 0.90, 0.96, 1.0],
        TextAlign::Left,
        12,
    );

    let mut sell_slot = None;
    for slot in 0..6 {
        let sx = margin + (slot as f32) * (eq_w + card_gap);
        let rect = [sx, eq_y, eq_w, eq_h];
        if let Some(w) = state.weapons.get(slot) {
            let color = tier_ui_color(w.tier);
            draw_panel(world, rect, color, 6.0, 11);
            draw_panel(world, inset(rect, 2.0), [0.08, 0.10, 0.16, 0.95], 5.0, 11);
            let (name, sell_val) = state
                .config
                .weapon_by_id(&w.weapon_id)
                .map_or((w.weapon_id.as_str(), 5), |d| {
                    (d.name.as_str(), d.sell_value_for_tier(w.tier))
                });
            draw_label(
                world,
                [sx + 6.0, eq_y + 8.0, eq_w - 12.0, 18.0],
                name,
                12.0,
                [1.0, 1.0, 1.0, 1.0],
                TextAlign::Center,
                12,
            );
            draw_label(
                world,
                [sx + 6.0, eq_y + 28.0, eq_w - 12.0, 16.0],
                &format!("Tier {}", tier_numeral(w.tier)),
                12.0,
                color,
                TextAlign::Center,
                12,
            );
            let sell_rect = [sx + 8.0, eq_y + eq_h - 30.0, eq_w - 16.0, 22.0];
            let sell_hover = point_in_rect(input.cursor, sell_rect);
            let can_sell = state.weapons.len() > 1;
            draw_button(
                world,
                sell_rect,
                &format!("SELL +{sell_val}G"),
                sell_hover,
                can_sell,
                [0.64, 0.24, 0.24, 1.0],
                12,
            );
            if sell_hover && input.left_clicked && can_sell {
                sell_slot = Some(slot);
            }
        } else {
            draw_panel(world, rect, [0.14, 0.16, 0.22, 0.55], 6.0, 11);
            draw_label(
                world,
                [sx, eq_y + 30.0, eq_w, 20.0],
                "EMPTY",
                12.0,
                [0.45, 0.48, 0.56, 0.8],
                TextAlign::Center,
                12,
            );
        }
    }
    if let Some(slot) = sell_slot {
        state.try_sell_weapon(slot, world);
        return;
    }

    let sb_x = sw - margin - sidebar_w;
    let sb_h = (sh - cards_y - margin).max(260.0);
    draw_panel(
        world,
        [sb_x, cards_y, sidebar_w, sb_h],
        [0.09, 0.12, 0.18, 0.96],
        8.0,
        11,
    );
    draw_label(
        world,
        [sb_x + 12.0, cards_y + 10.0, sidebar_w - 24.0, 22.0],
        "POTATO STATS",
        16.0,
        [1.0, 0.86, 0.34, 1.0],
        TextAlign::Center,
        12,
    );

    let stat_rows = [
        ("Max HP", format!("{:.0}", state.stats.max_hp)),
        ("HP Regen", format!("{:.1}/s", state.stats.hp_regen)),
        ("Melee Dmg", format!("{:+.0}", state.stats.melee_damage)),
        ("Ranged Dmg", format!("{:+.0}", state.stats.ranged_damage)),
        ("% Damage", format!("{:+.0}%", state.stats.damage_pct)),
        (
            "Atk Speed",
            format!("{:+.0}%", state.stats.attack_speed_pct),
        ),
        (
            "Crit Chance",
            format!("{:.0}%", state.stats.crit_chance_pct),
        ),
        ("Move Speed", format!("{:+.0}%", state.stats.move_speed_pct)),
        ("Dodge", format!("{:.0}%", state.stats.dodge_pct)),
        ("Armor", format!("{:.0}", state.stats.armor)),
        ("Pickup Rad", format!("{:.1}m", state.stats.pickup_radius)),
        ("Luck", format!("{:+.0}", state.stats.luck)),
        ("Range", format!("{:+.1}m", state.stats.range_bonus)),
    ];
    for (idx, (label, val)) in stat_rows.iter().enumerate() {
        let ry = cards_y + 40.0 + (idx as f32) * 22.0;
        if ry + 18.0 > cards_y + sb_h {
            break;
        }
        draw_label(
            world,
            [sb_x + 14.0, ry, 130.0, 18.0],
            label,
            13.0,
            [0.74, 0.80, 0.88, 0.95],
            TextAlign::Left,
            12,
        );
        draw_label(
            world,
            [sb_x + 134.0, ry, sidebar_w - 148.0, 18.0],
            val,
            13.0,
            [0.95, 0.98, 1.0, 1.0],
            TextAlign::Right,
            12,
        );
    }
}

fn handle_and_draw_end_screen(
    state: &mut SurvivorsState,
    world: &mut GameWorld<'_>,
    input: &InputSnapshot,
    victory: bool,
) {
    if input.key_pressed(keys::SPACE) || input.key_pressed(keys::ENTER) {
        state.start_new_run(world);
        return;
    }
    if input.key_pressed(keys::ESCAPE) {
        state.enter_main_menu(world);
        return;
    }

    let [sw, sh] = input.screen;
    if sw < 320.0 || sh < 240.0 {
        return;
    }

    draw_panel(world, [0.0, 0.0, sw, sh], [0.03, 0.04, 0.07, 0.84], 0.0, 10);
    let box_w = 480.0;
    let box_h = 312.0;
    let rect = [sw * 0.5 - box_w * 0.5, sh * 0.5 - box_h * 0.5, box_w, box_h];
    let accent = if victory {
        [1.0, 0.84, 0.24, 1.0]
    } else {
        [0.92, 0.26, 0.28, 1.0]
    };
    draw_panel(world, rect, accent, 12.0, 11);
    draw_panel(world, inset(rect, 3.0), [0.08, 0.10, 0.16, 0.96], 10.0, 11);

    let lang = state.settings.language;
    draw_label(
        world,
        [rect[0] + 20.0, rect[1] + 24.0, box_w - 40.0, 34.0],
        if victory {
            tr(
                lang,
                "胜利！全部波次已通关！",
                "VICTORY! ALL WAVES CLEARED!",
            )
        } else {
            tr(lang, "失败 - 土豆倒下了！", "RUN OVER - POTATO DOWN!")
        },
        24.0,
        accent,
        TextAlign::Center,
        12,
    );

    let summary = if lang.is_zh() {
        [
            format!(
                "到达波次: {} / {}",
                state.wave_index,
                state.config.total_waves()
            ),
            format!("最终等级: {}", state.stats.level),
            format!("击败敌人: {}", state.stats.kills),
            format!("累计伤害: {}", state.stats.total_damage),
            format!("剩余金币: {}G", state.stats.gold),
        ]
    } else {
        [
            format!(
                "Wave Reached: {} / {}",
                state.wave_index,
                state.config.total_waves()
            ),
            format!("Level Achieved: {}", state.stats.level),
            format!("Enemies Defeated: {}", state.stats.kills),
            format!("Total Damage Dealt: {}", state.stats.total_damage),
            format!("Remaining Gold: {}G", state.stats.gold),
        ]
    };
    for (idx, line) in summary.iter().enumerate() {
        draw_label(
            world,
            [
                rect[0] + 36.0,
                rect[1] + 78.0 + (idx as f32) * 26.0,
                box_w - 72.0,
                22.0,
            ],
            line,
            16.0,
            [0.90, 0.94, 0.98, 1.0],
            TextAlign::Center,
            12,
        );
    }

    let lang = state.settings.language;
    let retry_btn = [rect[0] + 28.0, rect[1] + box_h - 62.0, 214.0, 38.0];
    let menu_btn = [rect[0] + box_w - 242.0, rect[1] + box_h - 62.0, 214.0, 38.0];
    let hover = point_in_rect(input.cursor, retry_btn);
    let menu_hover = point_in_rect(input.cursor, menu_btn);
    draw_button(
        world,
        retry_btn,
        tr(lang, "再来一局  [SPACE]", "PLAY AGAIN  [SPACE]"),
        hover,
        true,
        [0.20, 0.58, 0.34, 1.0],
        12,
    );
    draw_button(
        world,
        menu_btn,
        tr(lang, "返回主菜单  [ESC]", "MAIN MENU  [ESC]"),
        menu_hover,
        true,
        [0.26, 0.38, 0.58, 1.0],
        12,
    );
    if hover && input.left_clicked {
        state.start_new_run(world);
    } else if menu_hover && input.left_clicked {
        state.enter_main_menu(world);
    }
}

fn describe_shop_offer(
    state: &SurvivorsState,
    offer: &crate::state::ShopOffer,
) -> (String, String, String, [f32; 4]) {
    match &offer.kind {
        ShopOfferKind::Weapon { weapon_id, tier } => {
            let accent = tier_ui_color(*tier);
            if let Some(def) = state.config.weapon_by_id(weapon_id) {
                let kind_tag = match def.kind {
                    WeaponKind::Ranged => "Ranged",
                    WeaponKind::MeleeSweep => "Sweep Melee",
                    WeaponKind::MeleeThrust => "Thrust Melee",
                };
                let tier_idx = crate::config::WeaponDef::tier_index(*tier);
                let dmg = def.base_damage * def.tier_damage_mult[tier_idx];
                let cd = def.cooldown * def.tier_cooldown_mult[tier_idx];
                let desc = format!(
                    "{kind_tag}\nBase Dmg: {:.0}\nCooldown: {:.2}s\nRange: {:.1}m",
                    dmg, cd, def.range
                );
                (
                    def.name.clone(),
                    format!("WEAPON T{}", tier_numeral(*tier)),
                    desc,
                    accent,
                )
            } else {
                (weapon_id.clone(), "WEAPON".into(), String::new(), accent)
            }
        }
        ShopOfferKind::Item { item_id } => {
            let accent = [0.34, 0.84, 0.72, 1.0];
            if let Some(item) = state.config.shop_item_by_id(item_id) {
                let mods = summarize_modifiers(&item.modifiers);
                let desc = format!("{}\n{}", item.description, mods);
                (item.name.clone(), "PASSIVE ITEM".into(), desc, accent)
            } else {
                (item_id.clone(), "ITEM".into(), String::new(), accent)
            }
        }
    }
}

fn summarize_modifiers(mods: &StatModifiers) -> String {
    let mut parts = Vec::new();
    if mods.max_hp != 0.0 {
        parts.push(format!("{:+.0} Max HP", mods.max_hp));
    }
    if mods.heal_now != 0.0 {
        parts.push(format!("+{:.0} Heal", mods.heal_now));
    }
    if mods.hp_regen != 0.0 {
        parts.push(format!("{:+.1} Regen/s", mods.hp_regen));
    }
    if mods.melee_damage != 0.0 {
        parts.push(format!("{:+.0} Melee Dmg", mods.melee_damage));
    }
    if mods.ranged_damage != 0.0 {
        parts.push(format!("{:+.0} Ranged Dmg", mods.ranged_damage));
    }
    if mods.damage_pct != 0.0 {
        parts.push(format!("{:+.0}% Dmg", mods.damage_pct));
    }
    if mods.attack_speed_pct != 0.0 {
        parts.push(format!("{:+.0}% Atk Spd", mods.attack_speed_pct));
    }
    if mods.crit_chance_pct != 0.0 {
        parts.push(format!("{:+.0}% Crit", mods.crit_chance_pct));
    }
    if mods.move_speed_pct != 0.0 {
        parts.push(format!("{:+.0}% Speed", mods.move_speed_pct));
    }
    if mods.dodge_pct != 0.0 {
        parts.push(format!("{:+.0}% Dodge", mods.dodge_pct));
    }
    if mods.armor != 0.0 {
        parts.push(format!("{:+.0} Armor", mods.armor));
    }
    if mods.pickup_radius != 0.0 {
        parts.push(format!("{:+.1}m Pickup", mods.pickup_radius));
    }
    if mods.luck != 0.0 {
        parts.push(format!("{:+.0} Luck", mods.luck));
    }
    if mods.range_bonus != 0.0 {
        parts.push(format!("{:+.1}m Range", mods.range_bonus));
    }
    parts.join(" | ")
}

fn tier_world_tint(tier: u8) -> [f32; 4] {
    match tier {
        2 => [0.78, 0.94, 1.0, 1.0],
        3 => [0.95, 0.78, 1.0, 1.0],
        4 => [1.0, 0.88, 0.55, 1.0],
        _ => [1.0, 1.0, 1.0, 1.0],
    }
}

fn tier_ui_color(tier: u8) -> [f32; 4] {
    match tier {
        2 => [0.32, 0.74, 1.0, 1.0],
        3 => [0.76, 0.44, 1.0, 1.0],
        4 => [1.0, 0.78, 0.22, 1.0],
        _ => [0.62, 0.68, 0.76, 1.0],
    }
}

fn tier_numeral(tier: u8) -> &'static str {
    match tier {
        2 => "II",
        3 => "III",
        4 => "IV",
        _ => "I",
    }
}

pub(crate) fn point_in_rect(pos: [f32; 2], rect: [f32; 4]) -> bool {
    pos[0] >= rect[0]
        && pos[0] <= rect[0] + rect[2]
        && pos[1] >= rect[1]
        && pos[1] <= rect[1] + rect[3]
}

pub(crate) fn inset(rect: [f32; 4], amount: f32) -> [f32; 4] {
    [
        rect[0] + amount,
        rect[1] + amount,
        (rect[2] - amount * 2.0).max(0.0),
        (rect[3] - amount * 2.0).max(0.0),
    ]
}

pub(crate) fn draw_panel(
    world: &mut GameWorld<'_>,
    rect: [f32; 4],
    color: [f32; 4],
    border_radius: f32,
    layer: i32,
) {
    world.draw_quad(UiQuad {
        rect,
        color,
        border_radius,
        layer,
    });
}

pub(crate) fn draw_label(
    world: &mut GameWorld<'_>,
    rect: [f32; 4],
    content: &str,
    font_size: f32,
    color: [f32; 4],
    alignment: TextAlign,
    layer: i32,
) {
    world.draw_text(UiText {
        rect,
        content: content.to_string(),
        font_size,
        color,
        alignment,
        outline_width: 1.4,
        outline_color: [0.04, 0.05, 0.08, color[3]],
        layer,
        ..UiText::default()
    });
}

pub(crate) fn draw_wrapped_label(
    world: &mut GameWorld<'_>,
    rect: [f32; 4],
    content: &str,
    font_size: f32,
    color: [f32; 4],
    layer: i32,
) {
    world.draw_text(UiText {
        rect,
        content: content.to_string(),
        font_size,
        color,
        alignment: TextAlign::Center,
        wrap: true,
        outline_width: 1.2,
        outline_color: [0.04, 0.05, 0.08, color[3]],
        layer,
        ..UiText::default()
    });
}

pub(crate) fn draw_button(
    world: &mut GameWorld<'_>,
    rect: [f32; 4],
    label: &str,
    hovered: bool,
    enabled: bool,
    base_color: [f32; 4],
    layer: i32,
) {
    let color = if !enabled {
        [0.18, 0.20, 0.24, 0.72]
    } else if hovered {
        [
            (base_color[0] + 0.14).min(1.0),
            (base_color[1] + 0.14).min(1.0),
            (base_color[2] + 0.14).min(1.0),
            1.0,
        ]
    } else {
        base_color
    };
    draw_panel(world, rect, color, 6.0, layer);
    let text_color = if enabled {
        [1.0, 1.0, 1.0, 1.0]
    } else {
        [0.58, 0.62, 0.68, 0.85]
    };
    draw_label(
        world,
        [
            rect[0] + 4.0,
            rect[1] + (rect[3] - 18.0) * 0.5,
            rect[2] - 8.0,
            18.0,
        ],
        label,
        13.0,
        text_color,
        TextAlign::Center,
        layer + 1,
    );
}
