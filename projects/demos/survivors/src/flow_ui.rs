//! Splash screen, main menu, pause menu, settings modal, and engine API verification UI.

use prism_game_api::{keys, EntityHandle, GameWorld, InputSnapshot, TextAlign};

use crate::i18n::tr;
use crate::state::{GamePhase, MenuModal, SurvivorsState, SPLASH_DURATION};
use crate::ui_hud::{
    draw_button, draw_label, draw_panel, draw_wrapped_label, inset, point_in_rect,
};

/// Toggle world-space player HP bar entities so they stay hidden on splash and main menu.
pub fn sync_player_hp_visibility(state: &SurvivorsState, world: &mut GameWorld<'_>, visible: bool) {
    for entity in [state.player_hp_bg, state.player_hp_fill] {
        if entity != EntityHandle::PLACEHOLDER
            && world.is_alive(entity)
            && world.is_entity_active(entity) != visible
        {
            world.set_entity_active(entity, visible);
        }
    }
}

/// Render and advance the animated splash screen; any confirm/cancel key or click skips it.
pub fn handle_and_draw_splash(
    state: &mut SurvivorsState,
    world: &mut GameWorld<'_>,
    input: &InputSnapshot,
    dt: f32,
) {
    if !state.splash_sound_played {
        state.splash_sound_played = true;
        state.splash_sound_id = state.play_sfx(world, "audio/wave_start.wav", 0.45, false);
    }

    state.splash_elapsed += dt;
    let skip_pressed = input.left_clicked
        || input.key_pressed(keys::SPACE)
        || input.key_pressed(keys::ENTER)
        || input.key_pressed(keys::ESCAPE);

    if skip_pressed || state.splash_elapsed >= SPLASH_DURATION {
        state.enter_main_menu(world);
        return;
    }

    let [sw, sh] = input.screen;
    if sw < 320.0 || sh < 240.0 {
        return;
    }

    let lang = state.settings.language;
    let progress = (state.splash_elapsed / SPLASH_DURATION).clamp(0.0, 1.0);
    let fade_in = (state.splash_elapsed / 0.55).clamp(0.0, 1.0);
    let fade_out = ((SPLASH_DURATION - state.splash_elapsed) / 0.45).clamp(0.0, 1.0);
    let alpha = fade_in.min(fade_out);

    draw_panel(world, [0.0, 0.0, sw, sh], [0.03, 0.04, 0.07, 0.96], 0.0, 10);

    let cx = sw * 0.5;
    let cy = sh * 0.5;
    let expand = 0.88 + 0.12 * fade_in;
    let card_w = 540.0 * expand;
    let card_h = 250.0 * expand;
    let card = [cx - card_w * 0.5, cy - card_h * 0.5, card_w, card_h];

    draw_panel(world, card, [0.24, 0.52, 0.92, 0.85 * alpha], 16.0, 11);
    draw_panel(
        world,
        inset(card, 3.0),
        [0.06, 0.08, 0.13, 0.97 * alpha],
        14.0,
        11,
    );

    let bar_w = 180.0 * fade_in;
    draw_panel(
        world,
        [cx - bar_w * 0.5, card[1] + 28.0, bar_w, 6.0],
        [1.0, 0.82, 0.28, alpha],
        3.0,
        12,
    );

    draw_label(
        world,
        [card[0] + 24.0, card[1] + 46.0, card_w - 48.0, 26.0],
        tr(lang, "由 PRISMA 引擎驱动", "POWERED BY PRISMA ENGINE"),
        15.0,
        [0.58, 0.80, 1.0, alpha],
        TextAlign::Center,
        12,
    );
    draw_label(
        world,
        [card[0] + 24.0, card[1] + 82.0, card_w - 48.0, 48.0],
        tr(lang, "土豆幸存者", "POTATO SURVIVORS"),
        34.0,
        [1.0, 0.90, 0.38, alpha],
        TextAlign::Center,
        12,
    );
    draw_label(
        world,
        [card[0] + 24.0, card[1] + 136.0, card_w - 48.0, 24.0],
        tr(
            lang,
            "2D 肉鸽竞技场 • 引擎 API 验证示例",
            "2D ROGUELITE ARENA & ENGINE API SHOWCASE",
        ),
        13.0,
        [0.78, 0.84, 0.92, 0.9 * alpha],
        TextAlign::Center,
        12,
    );

    let track = [
        card[0] + 64.0,
        card[1] + card_h - 56.0,
        card_w - 128.0,
        10.0,
    ];
    draw_panel(world, track, [0.12, 0.16, 0.24, alpha], 5.0, 12);
    draw_panel(
        world,
        [track[0], track[1], (track[2] * progress).max(4.0), track[3]],
        [0.28, 0.78, 0.98, alpha],
        5.0,
        13,
    );

    draw_label(
        world,
        [card[0] + 24.0, card[1] + card_h - 36.0, card_w - 48.0, 20.0],
        tr(
            lang,
            "按 空格 / 回车 或点击跳过",
            "PRESS SPACE / ENTER OR CLICK TO SKIP",
        ),
        11.0,
        [0.64, 0.72, 0.82, 0.85 * alpha],
        TextAlign::Center,
        12,
    );
}

/// Render and handle input for the main menu and its Settings / API Diagnostics modals.
pub fn handle_and_draw_main_menu(
    state: &mut SurvivorsState,
    world: &mut GameWorld<'_>,
    input: &InputSnapshot,
) {
    match state.modal {
        MenuModal::Settings => {
            draw_main_menu_backdrop(state, world, input);
            handle_and_draw_settings_modal(state, world, input);
            return;
        }
        MenuModal::ApiDiagnostics => {
            draw_main_menu_backdrop(state, world, input);
            handle_and_draw_api_modal(state, world, input);
            return;
        }
        MenuModal::None => {}
    }

    if input.key_pressed(keys::ARROW_UP) || input.key_pressed(keys::KEY_W) {
        state.menu_cursor = (state.menu_cursor + 2) % 3;
        state.play_sfx(world, "audio/shop_buy.wav", 0.22, false);
    }
    if input.key_pressed(keys::ARROW_DOWN) || input.key_pressed(keys::KEY_S) {
        state.menu_cursor = (state.menu_cursor + 1) % 3;
        state.play_sfx(world, "audio/shop_buy.wav", 0.22, false);
    }
    if input.key_pressed(keys::ARROW_LEFT) || input.key_pressed(keys::KEY_A) {
        state.selected_loadout = state.selected_loadout.prev();
        state.play_sfx(world, "audio/shop_reroll.wav", 0.28, false);
    }
    if input.key_pressed(keys::ARROW_RIGHT) || input.key_pressed(keys::KEY_D) {
        state.selected_loadout = state.selected_loadout.next();
        state.play_sfx(world, "audio/shop_reroll.wav", 0.28, false);
    }
    if input.key_pressed(keys::ENTER) || input.key_pressed(keys::SPACE) {
        match state.menu_cursor {
            0 => state.start_new_run(world),
            1 => {
                state.settings_cursor = 0;
                state.modal = MenuModal::Settings;
            }
            _ => {
                state.run_api_diagnostics(world);
                state.modal = MenuModal::ApiDiagnostics;
            }
        }
        return;
    }

    let [sw, sh] = input.screen;
    if sw < 400.0 || sh < 320.0 {
        return;
    }

    draw_main_menu_backdrop(state, world, input);

    let lang = state.settings.language;
    let cx = sw * 0.5;
    let margin = 28.0;
    let content_w = (sw - margin * 2.0).min(960.0);
    let left_x = cx - content_w * 0.5;
    let top_y = ((sh - 470.0) * 0.5).max(24.0);

    draw_panel(
        world,
        [left_x, top_y, content_w, 86.0],
        [0.07, 0.10, 0.16, 0.94],
        12.0,
        11,
    );
    draw_label(
        world,
        [left_x + 24.0, top_y + 12.0, content_w - 48.0, 38.0],
        tr(lang, "土豆幸存者", "POTATO SURVIVORS"),
        32.0,
        [1.0, 0.88, 0.30, 1.0],
        TextAlign::Center,
        12,
    );
    draw_label(
        world,
        [left_x + 24.0, top_y + 54.0, content_w - 48.0, 20.0],
        tr(
            lang,
            "生存 5 波 • 升级强化 • 构筑你的军械库",
            "SURVIVE 5 WAVES • LEVEL UP • BUILD YOUR ARSENAL",
        ),
        13.0,
        [0.72, 0.82, 0.94, 0.95],
        TextAlign::Center,
        12,
    );

    let lang_btn = [left_x + content_w - 154.0, top_y + 16.0, 136.0, 34.0];
    let lang_hover = point_in_rect(input.cursor, lang_btn);
    let lang_label = if lang.is_zh() {
        "语言: 简体中文"
    } else {
        "LANG: English"
    };
    draw_button(
        world,
        lang_btn,
        lang_label,
        lang_hover,
        true,
        [0.20, 0.34, 0.54, 1.0],
        13,
    );
    if lang_hover && input.left_clicked {
        state.toggle_language(world);
        state.play_sfx(world, "audio/shop_buy.wav", 0.25, false);
        return;
    }

    let col_gap = 20.0;
    let left_w = (content_w * 0.56).max(320.0);
    let right_w = content_w - left_w - col_gap;
    let body_y = top_y + 104.0;
    let body_h = 356.0;

    draw_panel(
        world,
        [left_x, body_y, left_w, body_h],
        [0.08, 0.11, 0.17, 0.94],
        12.0,
        11,
    );

    draw_label(
        world,
        [left_x + 20.0, body_y + 16.0, left_w - 40.0, 20.0],
        tr(lang, "初始流派  [A / D]", "STARTER LOADOUT  [A / D]"),
        13.0,
        [0.64, 0.78, 0.96, 1.0],
        TextAlign::Center,
        12,
    );

    let prev_btn = [left_x + 20.0, body_y + 42.0, 44.0, 56.0];
    let next_btn = [left_x + left_w - 64.0, body_y + 42.0, 44.0, 56.0];
    let loadout_box = [left_x + 72.0, body_y + 42.0, left_w - 144.0, 56.0];
    let prev_hover = point_in_rect(input.cursor, prev_btn);
    let next_hover = point_in_rect(input.cursor, next_btn);

    draw_button(
        world,
        prev_btn,
        "<",
        prev_hover,
        true,
        [0.20, 0.28, 0.42, 1.0],
        12,
    );
    draw_button(
        world,
        next_btn,
        ">",
        next_hover,
        true,
        [0.20, 0.28, 0.42, 1.0],
        12,
    );
    draw_panel(world, loadout_box, [0.11, 0.15, 0.23, 0.98], 8.0, 12);
    draw_label(
        world,
        [
            loadout_box[0] + 8.0,
            loadout_box[1] + 8.0,
            loadout_box[2] - 16.0,
            22.0,
        ],
        state.selected_loadout.label_for(lang),
        16.0,
        [1.0, 0.86, 0.34, 1.0],
        TextAlign::Center,
        13,
    );
    draw_label(
        world,
        [
            loadout_box[0] + 8.0,
            loadout_box[1] + 32.0,
            loadout_box[2] - 16.0,
            18.0,
        ],
        state.selected_loadout.subtitle_for(lang),
        12.0,
        [0.82, 0.88, 0.95, 1.0],
        TextAlign::Center,
        13,
    );

    if prev_hover && input.left_clicked {
        state.selected_loadout = state.selected_loadout.prev();
        state.play_sfx(world, "audio/shop_reroll.wav", 0.28, false);
    } else if next_hover && input.left_clicked {
        state.selected_loadout = state.selected_loadout.next();
        state.play_sfx(world, "audio/shop_reroll.wav", 0.28, false);
    }

    let menu_items = [
        (
            tr(lang, "开始游戏  [ENTER]", "START RUN  [ENTER]"),
            [0.20, 0.58, 0.34, 1.0],
        ),
        (tr(lang, "系统设置", "SETTINGS"), [0.22, 0.36, 0.58, 1.0]),
        (
            tr(lang, "引擎 API 验证", "ENGINE API VERIFY"),
            [0.42, 0.30, 0.60, 1.0],
        ),
    ];

    for (idx, (label, base_color)) in menu_items.iter().enumerate() {
        let btn = [
            left_x + 34.0,
            body_y + 124.0 + (idx as f32) * 64.0,
            left_w - 68.0,
            48.0,
        ];
        let hovered = point_in_rect(input.cursor, btn);
        if hovered {
            state.menu_cursor = idx;
        }
        let selected = state.menu_cursor == idx;
        if selected {
            draw_panel(
                world,
                [btn[0] - 3.0, btn[1] - 3.0, btn[2] + 6.0, btn[3] + 6.0],
                [1.0, 0.86, 0.32, 0.95],
                8.0,
                11,
            );
        }
        draw_button(
            world,
            btn,
            label,
            hovered || selected,
            true,
            *base_color,
            12,
        );
        if hovered && input.left_clicked {
            match idx {
                0 => state.start_new_run(world),
                1 => {
                    state.settings_cursor = 0;
                    state.modal = MenuModal::Settings;
                }
                _ => {
                    state.run_api_diagnostics(world);
                    state.modal = MenuModal::ApiDiagnostics;
                }
            }
            return;
        }
    }

    draw_label(
        world,
        [left_x + 20.0, body_y + body_h - 32.0, left_w - 40.0, 20.0],
        tr(
            lang,
            "WASD / 方向键导航 • 局内按 ESC 暂停",
            "WASD / ARROWS TO NAVIGATE • ESC TO PAUSE IN-GAME",
        ),
        11.0,
        [0.64, 0.72, 0.82, 0.9],
        TextAlign::Center,
        12,
    );

    let right_x = left_x + left_w + col_gap;
    draw_panel(
        world,
        [right_x, body_y, right_w, body_h],
        [0.08, 0.11, 0.17, 0.94],
        12.0,
        11,
    );
    draw_label(
        world,
        [right_x + 16.0, body_y + 16.0, right_w - 32.0, 24.0],
        tr(lang, "生涯战绩", "CAREER RECORDS"),
        16.0,
        [1.0, 0.86, 0.32, 1.0],
        TextAlign::Center,
        12,
    );

    let rec = &state.records;
    let rows = [
        (
            tr(lang, "游玩局数", "Runs Played"),
            format!("{}", rec.runs_played),
        ),
        (
            tr(lang, "通关次数", "Victories"),
            format!("{}", rec.victories),
        ),
        (
            tr(lang, "最高波次", "Best Wave"),
            format!("{}", rec.best_wave),
        ),
        (
            tr(lang, "最高等级", "Best Level"),
            format!("{}", rec.best_level),
        ),
        (
            tr(lang, "单局最高击杀", "Best Kills"),
            format!("{}", rec.best_kills),
        ),
        (
            tr(lang, "累计击杀", "Total Kills"),
            format!("{}", rec.total_kills),
        ),
    ];
    for (idx, (k, v)) in rows.iter().enumerate() {
        let ry = body_y + 56.0 + (idx as f32) * 34.0;
        draw_panel(
            world,
            [right_x + 18.0, ry, right_w - 36.0, 28.0],
            [0.11, 0.15, 0.22, 0.9],
            6.0,
            12,
        );
        draw_label(
            world,
            [right_x + 28.0, ry + 5.0, right_w * 0.55, 18.0],
            k,
            13.0,
            [0.78, 0.84, 0.92, 1.0],
            TextAlign::Left,
            13,
        );
        draw_label(
            world,
            [
                right_x + right_w * 0.5,
                ry + 5.0,
                right_w * 0.5 - 28.0,
                18.0,
            ],
            v,
            14.0,
            [1.0, 1.0, 1.0, 1.0],
            TextAlign::Right,
            13,
        );
    }

    let reset_btn = [right_x + 24.0, body_y + body_h - 52.0, right_w - 48.0, 34.0];
    let reset_hover = point_in_rect(input.cursor, reset_btn);
    draw_button(
        world,
        reset_btn,
        tr(lang, "重置战绩", "RESET RECORDS"),
        reset_hover,
        rec.runs_played > 0 || rec.total_kills > 0,
        [0.52, 0.22, 0.24, 1.0],
        12,
    );
    if reset_hover && input.left_clicked && (rec.runs_played > 0 || rec.total_kills > 0) {
        state.reset_records(world);
        state.play_sfx(world, "audio/shop_reroll.wav", 0.3, false);
    }
}

/// Render and handle input for the in-game pause menu.
pub fn handle_and_draw_paused(
    state: &mut SurvivorsState,
    world: &mut GameWorld<'_>,
    input: &InputSnapshot,
    from_sweep_tenths: Option<u32>,
) {
    if state.modal == MenuModal::Settings {
        handle_and_draw_settings_modal(state, world, input);
        return;
    }

    if input.key_pressed(keys::ESCAPE) {
        state.phase = resume_phase(from_sweep_tenths);
        return;
    }
    if input.key_pressed(keys::ARROW_UP) || input.key_pressed(keys::KEY_W) {
        state.pause_cursor = (state.pause_cursor + 3) % 4;
    }
    if input.key_pressed(keys::ARROW_DOWN) || input.key_pressed(keys::KEY_S) {
        state.pause_cursor = (state.pause_cursor + 1) % 4;
    }
    if input.key_pressed(keys::ENTER) || input.key_pressed(keys::SPACE) {
        match state.pause_cursor {
            0 => state.phase = resume_phase(from_sweep_tenths),
            1 => state.start_new_run(world),
            2 => {
                state.settings_cursor = 0;
                state.modal = MenuModal::Settings;
            }
            _ => state.enter_main_menu(world),
        }
        return;
    }

    let [sw, sh] = input.screen;
    if sw < 320.0 || sh < 240.0 {
        return;
    }

    let lang = state.settings.language;
    draw_panel(world, [0.0, 0.0, sw, sh], [0.02, 0.03, 0.06, 0.78], 0.0, 10);
    let box_w = 380.0;
    let box_h = 324.0;
    let rect = [sw * 0.5 - box_w * 0.5, sh * 0.5 - box_h * 0.5, box_w, box_h];
    draw_panel(world, rect, [0.34, 0.52, 0.82, 0.95], 12.0, 11);
    draw_panel(world, inset(rect, 3.0), [0.07, 0.10, 0.16, 0.97], 10.0, 11);

    draw_label(
        world,
        [rect[0] + 20.0, rect[1] + 20.0, box_w - 40.0, 32.0],
        tr(lang, "游戏已暂停", "GAME PAUSED"),
        24.0,
        [1.0, 0.88, 0.34, 1.0],
        TextAlign::Center,
        12,
    );

    let items = [
        (
            tr(lang, "继续游戏  [ESC]", "RESUME  [ESC]"),
            [0.20, 0.58, 0.34, 1.0],
        ),
        (tr(lang, "重新开始", "RESTART RUN"), [0.24, 0.42, 0.64, 1.0]),
        (tr(lang, "系统设置", "SETTINGS"), [0.28, 0.36, 0.54, 1.0]),
        (tr(lang, "返回主菜单", "MAIN MENU"), [0.54, 0.24, 0.26, 1.0]),
    ];

    for (idx, (label, base_color)) in items.iter().enumerate() {
        let btn = [
            rect[0] + 38.0,
            rect[1] + 74.0 + (idx as f32) * 58.0,
            box_w - 76.0,
            44.0,
        ];
        let hovered = point_in_rect(input.cursor, btn);
        if hovered {
            state.pause_cursor = idx;
        }
        let selected = state.pause_cursor == idx;
        if selected {
            draw_panel(
                world,
                [btn[0] - 2.5, btn[1] - 2.5, btn[2] + 5.0, btn[3] + 5.0],
                [1.0, 0.86, 0.32, 0.95],
                8.0,
                11,
            );
        }
        draw_button(
            world,
            btn,
            label,
            hovered || selected,
            true,
            *base_color,
            12,
        );
        if hovered && input.left_clicked {
            match idx {
                0 => state.phase = resume_phase(from_sweep_tenths),
                1 => state.start_new_run(world),
                2 => {
                    state.settings_cursor = 0;
                    state.modal = MenuModal::Settings;
                }
                _ => state.enter_main_menu(world),
            }
            return;
        }
    }
}

/// Draw a compact live engine telemetry strip in the bottom-right corner when enabled.
pub fn draw_api_telemetry_strip(
    state: &SurvivorsState,
    world: &mut GameWorld<'_>,
    input: &InputSnapshot,
) {
    if !state.settings.show_api_telemetry {
        return;
    }
    let [sw, sh] = input.screen;
    if sw < 420.0 || sh < 240.0 {
        return;
    }
    let w = 360.0;
    let h = 28.0;
    let x = sw - w - 16.0;
    let y = sh - h - 12.0;
    draw_panel(world, [x, y, w, h], [0.04, 0.06, 0.10, 0.86], 6.0, 9);
    let fps = if state.last_dt > 0.0001 {
        1.0 / state.last_dt
    } else {
        0.0
    };
    let text = format!(
        "API  {:.0}x{:.0}  {:.0}fps  gen:{}  vol:{:.0}%  enemies:{}",
        sw,
        sh,
        fps,
        world.scene_generation(),
        state.settings.effective_sfx_volume() * 100.0,
        state.enemies.len()
    );
    draw_label(
        world,
        [x + 8.0, y + 5.0, w - 16.0, 18.0],
        &text,
        11.0,
        [0.58, 0.88, 0.98, 0.96],
        TextAlign::Right,
        9,
    );
}

fn resume_phase(from_sweep_tenths: Option<u32>) -> GamePhase {
    match from_sweep_tenths {
        Some(timer_tenths) => GamePhase::WaveClearSweep { timer_tenths },
        None => GamePhase::WaveCombat,
    }
}

fn draw_main_menu_backdrop(
    _state: &SurvivorsState,
    world: &mut GameWorld<'_>,
    input: &InputSnapshot,
) {
    let [sw, sh] = input.screen;
    draw_panel(world, [0.0, 0.0, sw, sh], [0.03, 0.04, 0.08, 0.72], 0.0, 10);
}

fn handle_and_draw_settings_modal(
    state: &mut SurvivorsState,
    world: &mut GameWorld<'_>,
    input: &InputSnapshot,
) {
    if input.key_pressed(keys::ESCAPE) {
        state.modal = MenuModal::None;
        return;
    }

    let [sw, sh] = input.screen;
    if sw < 360.0 || sh < 280.0 {
        return;
    }

    let lang = state.settings.language;
    let box_w = 490.0;
    let box_h = 424.0;
    let rect = [sw * 0.5 - box_w * 0.5, sh * 0.5 - box_h * 0.5, box_w, box_h];
    draw_panel(world, rect, [0.34, 0.56, 0.90, 0.95], 12.0, 13);
    draw_panel(world, inset(rect, 3.0), [0.07, 0.10, 0.16, 0.98], 10.0, 13);

    draw_label(
        world,
        [rect[0] + 20.0, rect[1] + 18.0, box_w - 40.0, 28.0],
        tr(
            lang,
            "系统设置  (通过 save_data 持久化)",
            "SETTINGS  (PERSISTED VIA save_data)",
        ),
        18.0,
        [1.0, 0.88, 0.34, 1.0],
        TextAlign::Center,
        14,
    );

    let mut changed = false;

    let lang_row_y = rect[1] + 62.0;
    draw_panel(
        world,
        [rect[0] + 24.0, lang_row_y, box_w - 48.0, 44.0],
        [0.11, 0.15, 0.22, 0.95],
        8.0,
        14,
    );
    draw_label(
        world,
        [rect[0] + 36.0, lang_row_y + 12.0, 220.0, 20.0],
        tr(lang, "界面语言", "Language"),
        14.0,
        [0.92, 0.95, 1.0, 1.0],
        TextAlign::Left,
        15,
    );
    let lang_btn = [rect[0] + box_w - 156.0, lang_row_y + 7.0, 120.0, 30.0];
    let lang_hover = point_in_rect(input.cursor, lang_btn);
    draw_button(
        world,
        lang_btn,
        lang.display_name(),
        lang_hover,
        true,
        [0.22, 0.42, 0.66, 1.0],
        15,
    );
    if lang_hover && input.left_clicked {
        state.toggle_language(world);
        return;
    }

    let row0_y = rect[1] + 116.0;
    draw_panel(
        world,
        [rect[0] + 24.0, row0_y, box_w - 48.0, 44.0],
        [0.11, 0.15, 0.22, 0.95],
        8.0,
        14,
    );
    let vol_text = if lang.is_zh() {
        format!("音效音量: {:.0}%", state.settings.sfx_volume * 100.0)
    } else {
        format!("SFX Volume: {:.0}%", state.settings.sfx_volume * 100.0)
    };
    draw_label(
        world,
        [rect[0] + 36.0, row0_y + 12.0, 200.0, 20.0],
        &vol_text,
        14.0,
        [0.92, 0.95, 1.0, 1.0],
        TextAlign::Left,
        15,
    );
    let vol_minus = [rect[0] + box_w - 132.0, row0_y + 7.0, 44.0, 30.0];
    let vol_plus = [rect[0] + box_w - 80.0, row0_y + 7.0, 44.0, 30.0];
    let minus_hover = point_in_rect(input.cursor, vol_minus);
    let plus_hover = point_in_rect(input.cursor, vol_plus);
    draw_button(
        world,
        vol_minus,
        "-",
        minus_hover,
        true,
        [0.24, 0.34, 0.50, 1.0],
        15,
    );
    draw_button(
        world,
        vol_plus,
        "+",
        plus_hover,
        true,
        [0.24, 0.34, 0.50, 1.0],
        15,
    );
    if minus_hover && input.left_clicked {
        state.settings.sfx_volume = (state.settings.sfx_volume - 0.1).clamp(0.0, 1.0);
        changed = true;
    } else if plus_hover && input.left_clicked {
        state.settings.sfx_volume = (state.settings.sfx_volume + 0.1).clamp(0.0, 1.0);
        changed = true;
    }

    let toggles = [
        (tr(lang, "静音", "Mute Audio"), state.settings.muted),
        (
            tr(lang, "伤害数字", "Damage Numbers"),
            state.settings.show_damage_numbers,
        ),
        (
            tr(lang, "API 实时遥测", "Live API Telemetry"),
            state.settings.show_api_telemetry,
        ),
    ];
    for (idx, (label, active)) in toggles.iter().enumerate() {
        let ry = rect[1] + 172.0 + (idx as f32) * 52.0;
        draw_panel(
            world,
            [rect[0] + 24.0, ry, box_w - 48.0, 42.0],
            [0.11, 0.15, 0.22, 0.95],
            8.0,
            14,
        );
        draw_label(
            world,
            [rect[0] + 36.0, ry + 11.0, 230.0, 20.0],
            label,
            14.0,
            [0.92, 0.95, 1.0, 1.0],
            TextAlign::Left,
            15,
        );
        let btn = [rect[0] + box_w - 132.0, ry + 6.0, 96.0, 30.0];
        let hovered = point_in_rect(input.cursor, btn);
        let btn_color = if *active {
            [0.22, 0.58, 0.34, 1.0]
        } else {
            [0.34, 0.24, 0.28, 1.0]
        };
        let state_label = if *active {
            tr(lang, "开启", "ON")
        } else {
            tr(lang, "关闭", "OFF")
        };
        draw_button(world, btn, state_label, hovered, true, btn_color, 15);
        if hovered && input.left_clicked {
            match idx {
                0 => state.settings.muted = !state.settings.muted,
                1 => state.settings.show_damage_numbers = !state.settings.show_damage_numbers,
                _ => state.settings.show_api_telemetry = !state.settings.show_api_telemetry,
            }
            changed = true;
        }
    }

    if changed {
        state.settings.save(world);
    }

    let test_btn = [rect[0] + 24.0, rect[1] + box_h - 62.0, 212.0, 38.0];
    let close_btn = [rect[0] + box_w - 236.0, rect[1] + box_h - 62.0, 212.0, 38.0];
    let test_hover = point_in_rect(input.cursor, test_btn);
    let close_hover = point_in_rect(input.cursor, close_btn);

    draw_button(
        world,
        test_btn,
        tr(lang, "测试音效 (PLAY/STOP)", "TEST SFX (PLAY/STOP)"),
        test_hover,
        true,
        [0.26, 0.44, 0.68, 1.0],
        15,
    );
    draw_button(
        world,
        close_btn,
        tr(lang, "返回  [ESC]", "BACK  [ESC]"),
        close_hover,
        true,
        [0.24, 0.52, 0.34, 1.0],
        15,
    );

    if test_hover && input.left_clicked {
        if let Some(prev_id) = state.test_sound_id.take() {
            world.stop_sound(prev_id);
        }
        state.test_sound_id = state.play_sfx(world, "audio/level_up.wav", 0.55, false);
    } else if close_hover && input.left_clicked {
        state.modal = MenuModal::None;
    }
}

fn handle_and_draw_api_modal(
    state: &mut SurvivorsState,
    world: &mut GameWorld<'_>,
    input: &InputSnapshot,
) {
    if input.key_pressed(keys::ESCAPE) {
        state.modal = MenuModal::None;
        return;
    }

    if state.api_report.is_none() {
        state.run_api_diagnostics(world);
    }

    let [sw, sh] = input.screen;
    if sw < 420.0 || sh < 320.0 {
        return;
    }

    let lang = state.settings.language;
    let box_w = (sw - 64.0).min(660.0);
    let box_h = (sh - 48.0).min(470.0);
    let rect = [sw * 0.5 - box_w * 0.5, sh * 0.5 - box_h * 0.5, box_w, box_h];
    draw_panel(world, rect, [0.54, 0.40, 0.88, 0.96], 12.0, 13);
    draw_panel(world, inset(rect, 3.0), [0.06, 0.08, 0.14, 0.98], 10.0, 13);

    let header = if let Some(rep) = &state.api_report {
        if lang.is_zh() {
            format!(
                "引擎 API 验证  ({}/{} 通过)",
                rep.passed_count, rep.total_count
            )
        } else {
            format!(
                "ENGINE API VERIFICATION  ({}/{} PASSED)",
                rep.passed_count, rep.total_count
            )
        }
    } else {
        tr(lang, "引擎 API 验证", "ENGINE API VERIFICATION").to_string()
    };
    draw_label(
        world,
        [rect[0] + 20.0, rect[1] + 18.0, box_w - 40.0, 26.0],
        &header,
        18.0,
        [0.82, 0.72, 1.0, 1.0],
        TextAlign::Center,
        14,
    );

    if let Some(rep) = &state.api_report {
        for (idx, entry) in rep.entries.iter().enumerate() {
            let ry = rect[1] + 58.0 + (idx as f32) * 44.0;
            if ry + 40.0 > rect[1] + box_h - 72.0 {
                break;
            }
            draw_panel(
                world,
                [rect[0] + 22.0, ry, box_w - 44.0, 38.0],
                [0.10, 0.14, 0.21, 0.96],
                7.0,
                14,
            );
            let badge_color = if entry.passed {
                [0.24, 0.78, 0.42, 1.0]
            } else {
                [0.88, 0.28, 0.28, 1.0]
            };
            draw_panel(
                world,
                [rect[0] + 30.0, ry + 8.0, 56.0, 22.0],
                badge_color,
                5.0,
                15,
            );
            draw_label(
                world,
                [rect[0] + 30.0, ry + 10.0, 56.0, 18.0],
                if entry.passed {
                    tr(lang, "通过", "PASS")
                } else {
                    tr(lang, "失败", "FAIL")
                },
                11.0,
                [1.0, 1.0, 1.0, 1.0],
                TextAlign::Center,
                16,
            );
            draw_label(
                world,
                [rect[0] + 96.0, ry + 9.0, box_w * 0.46, 20.0],
                entry.name,
                13.0,
                [0.94, 0.96, 1.0, 1.0],
                TextAlign::Left,
                15,
            );
            draw_wrapped_label(
                world,
                [rect[0] + box_w * 0.56, ry + 9.0, box_w * 0.44 - 32.0, 22.0],
                &entry.detail,
                12.0,
                [0.72, 0.84, 0.94, 1.0],
                15,
            );
        }
    }

    let rerun_btn = [rect[0] + 24.0, rect[1] + box_h - 58.0, 210.0, 38.0];
    let close_btn = [rect[0] + box_w - 234.0, rect[1] + box_h - 58.0, 210.0, 38.0];
    let rerun_hover = point_in_rect(input.cursor, rerun_btn);
    let close_hover = point_in_rect(input.cursor, close_btn);

    draw_button(
        world,
        rerun_btn,
        tr(lang, "重新检测", "RE-RUN CHECKS"),
        rerun_hover,
        true,
        [0.36, 0.28, 0.62, 1.0],
        15,
    );
    draw_button(
        world,
        close_btn,
        tr(lang, "关闭  [ESC]", "CLOSE  [ESC]"),
        close_hover,
        true,
        [0.24, 0.52, 0.34, 1.0],
        15,
    );

    if rerun_hover && input.left_clicked {
        state.run_api_diagnostics(world);
    } else if close_hover && input.left_clicked {
        state.modal = MenuModal::None;
    }
}
