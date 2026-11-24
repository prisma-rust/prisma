//! Gameplay integration tests for `survivors` (host backend, headless ECS).

#![deny(warnings)]

use prism_game_api::{keys, GameWorld, InputSnapshot};
use prism_scene::components::{Camera, LocalTransform, Name, SpriteRenderer};
use prism_scene::BUILTIN_WHITE_SPRITE;
use survivors_core::combat;
use survivors_core::i18n::Language;
use survivors_core::state::{
    EquippedWeapon, GamePhase, MenuModal, PickupKind, ShopOffer, ShopOfferKind, StarterLoadout,
    SurvivorsState, ARENA_HALF_H, ARENA_HALF_W, MAX_WEAPONS, SPLASH_DURATION,
};
use survivors_core::{fixed_tick, frame_tick};

fn scene_world() -> prism_ecs::World {
    let mut world = prism_ecs::World::new();
    let cam = world.spawn();
    world.insert(cam, Name("camera".to_string()));
    world.insert(cam, LocalTransform::default());
    world.insert(
        cam,
        Camera {
            orthographic_size: 11.0,
            ..Camera::default()
        },
    );

    let player = world.spawn();
    world.insert(player, Name("player".to_string()));
    world.insert(player, LocalTransform::default());
    world
}

fn default_screen_input() -> InputSnapshot {
    InputSnapshot {
        screen: [1280.0, 720.0],
        ..InputSnapshot::default()
    }
}

fn key_held_input(held: &[usize]) -> InputSnapshot {
    let mut input = default_screen_input();
    for &k in held {
        input.set_key(k, true);
    }
    input
}

fn key_pressed_input(key: usize) -> InputSnapshot {
    let mut input = default_screen_input();
    input.set_key(key, true);
    input.set_key_edge(key, true);
    input
}

fn enter_combat_from_boot(state: &mut Option<SurvivorsState>, api: &mut GameWorld<'_>) {
    api.set_input(default_screen_input());
    fixed_tick(state, api, 1.0 / 60.0);
    api.set_input(key_pressed_input(keys::SPACE));
    frame_tick(state, api, 1.0 / 60.0);
    api.set_input(key_pressed_input(keys::ENTER));
    frame_tick(state, api, 1.0 / 60.0);
    api.set_input(default_screen_input());
    frame_tick(state, api, 1.0 / 60.0);
}

#[test]
fn config_and_world_init_build_arena_and_white_hp_bars() {
    let mut ecs = scene_world();
    let mut api = GameWorld::host(&mut ecs);
    let mut state: Option<SurvivorsState> = None;
    enter_combat_from_boot(&mut state, &mut api);

    let st = state.as_ref().expect("state initialized from data/*.json");
    assert!(st.world_built);
    assert_eq!(st.weapons.len(), 2);
    assert_eq!(st.weapons[0].weapon_id, "pistol");
    assert_eq!(st.weapons[1].weapon_id, "rusty_blade");

    let bg_sprite = api
        .get::<SpriteRenderer>(st.player_hp_bg)
        .expect("player HP background uses SpriteRenderer");
    let fill_sprite = api
        .get::<SpriteRenderer>(st.player_hp_fill)
        .expect("player HP fill uses SpriteRenderer");
    assert_eq!(bg_sprite.sprite_path, BUILTIN_WHITE_SPRITE);
    assert_eq!(fill_sprite.sprite_path, BUILTIN_WHITE_SPRITE);

    let player_sprite = api
        .get::<SpriteRenderer>(st.player_entity)
        .expect("player entity has SpriteRenderer");
    assert_eq!(player_sprite.sprite_path, "sprites/atlas.png");
    assert_eq!(player_sprite.region_px, [0, 0, 16, 16]);
}

#[test]
fn player_eight_way_movement_and_arena_clamping() {
    let mut ecs = scene_world();
    let mut api = GameWorld::host(&mut ecs);
    let mut state: Option<SurvivorsState> = None;
    enter_combat_from_boot(&mut state, &mut api);

    api.set_input(key_held_input(&[keys::KEY_D, keys::KEY_W]));
    for _ in 0..30 {
        fixed_tick(&mut state, &mut api, 1.0 / 60.0);
    }

    let st = state.as_ref().unwrap();
    assert!(
        st.player_pos[0] > 1.5,
        "expected +X travel, got {:?}",
        st.player_pos
    );
    assert!(
        st.player_pos[1] > 1.5,
        "expected +Y travel, got {:?}",
        st.player_pos
    );
    assert!(st.player_is_moving);

    let run_sprite = api.get::<SpriteRenderer>(st.player_entity).unwrap();
    assert_eq!(run_sprite.region_px, [0, 16, 16, 16]);

    for _ in 0..600 {
        fixed_tick(&mut state, &mut api, 1.0 / 60.0);
    }
    let st = state.as_ref().unwrap();
    assert!(st.player_pos[0] < ARENA_HALF_W);
    assert!(st.player_pos[1] < ARENA_HALF_H);
}

#[test]
fn weapons_auto_target_and_kill_enemies_spawning_pickups() {
    let mut ecs = scene_world();
    let mut api = GameWorld::host(&mut ecs);
    let mut state: Option<SurvivorsState> = None;
    enter_combat_from_boot(&mut state, &mut api);

    {
        let st = state.as_mut().unwrap();
        st.spawn_timer = 99.0;
        let idx = combat::spawn_enemy_at(st, &mut api, "slime", [2.6, 0.2]).expect("slime def");
        st.enemies[idx].hp = 12.0;
    }

    let mut saw_damage_number = false;
    for _ in 0..90 {
        fixed_tick(&mut state, &mut api, 1.0 / 60.0);
        if !state.as_ref().unwrap().damage_numbers.is_empty() {
            saw_damage_number = true;
        }
        frame_tick(&mut state, &mut api, 1.0 / 60.0);
        if state.as_ref().unwrap().stats.kills > 0 {
            break;
        }
    }

    let st = state.as_ref().unwrap();
    assert!(
        saw_damage_number,
        "expected floating damage number on weapon hit"
    );
    assert!(
        st.stats.kills >= 1,
        "expected auto-targeted weapons to kill slime"
    );
    assert!(
        !st.pickups.is_empty() || st.stats.xp > 0,
        "expected enemy death to drop XP/gold pickup"
    );
}

#[test]
fn horde_250_enemies_crowd_separation_runs_cleanly() {
    let mut ecs = scene_world();
    let mut api = GameWorld::host(&mut ecs);
    let mut state: Option<SurvivorsState> = None;
    enter_combat_from_boot(&mut state, &mut api);

    {
        let st = state.as_mut().unwrap();
        st.spawn_timer = 999.0;
        st.player_pos = [-18.0, -12.0];
        for k in 0..256 {
            let angle = (k as f32) * 0.25;
            let r = 0.05 + (k as f32) * 0.004;
            combat::spawn_enemy_at(
                st,
                &mut api,
                "slime",
                [6.0 + angle.cos() * r, 4.0 + angle.sin() * r],
            );
        }
    }

    let initial_spread = {
        let st = state.as_ref().unwrap();
        st.enemies
            .iter()
            .map(|e| (e.pos[0] - 6.0).hypot(e.pos[1] - 4.0))
            .sum::<f32>()
            / st.enemies.len() as f32
    };

    for _ in 0..25 {
        fixed_tick(&mut state, &mut api, 1.0 / 60.0);
    }

    let st = state.as_ref().unwrap();
    assert!(st.enemies.len() >= 240);
    let mut mean_x = 0.0_f32;
    let mut mean_y = 0.0_f32;
    for e in &st.enemies {
        assert!(e.pos[0].is_finite() && e.pos[1].is_finite());
        assert!(e.pos[0].abs() <= ARENA_HALF_W && e.pos[1].abs() <= ARENA_HALF_H);
        mean_x += e.pos[0];
        mean_y += e.pos[1];
    }
    mean_x /= st.enemies.len() as f32;
    mean_y /= st.enemies.len() as f32;
    let post_spread = st
        .enemies
        .iter()
        .map(|e| (e.pos[0] - mean_x).hypot(e.pos[1] - mean_y))
        .sum::<f32>()
        / st.enemies.len() as f32;

    assert!(
        post_spread > initial_spread * 2.5,
        "crowd separation should push overlapping enemies apart (initial={initial_spread}, post={post_spread})"
    );
}

#[test]
fn xp_pickup_magnet_triggers_level_up_and_applies_upgrade() {
    let mut ecs = scene_world();
    let mut api = GameWorld::host(&mut ecs);
    let mut state: Option<SurvivorsState> = None;
    enter_combat_from_boot(&mut state, &mut api);

    {
        let st = state.as_mut().unwrap();
        st.spawn_timer = 99.0;
        let xp_needed = st.stats.xp_to_next + 10;
        combat::spawn_pickup_at(st, &mut api, PickupKind::XpLarge(xp_needed), [2.0, 0.0]);
    }

    for _ in 0..60 {
        fixed_tick(&mut state, &mut api, 1.0 / 60.0);
        if matches!(
            state.as_ref().unwrap().phase,
            GamePhase::LevelUpChoice { .. }
        ) {
            break;
        }
    }

    let choices_len = match &state.as_ref().unwrap().phase {
        GamePhase::LevelUpChoice { choices, .. } => choices.len(),
        other => panic!("expected LevelUpChoice, got {other:?}"),
    };
    assert_eq!(choices_len, 4);
    assert!(state.as_ref().unwrap().stats.level >= 2);

    api.set_input(key_pressed_input(keys::DIGIT_1));
    frame_tick(&mut state, &mut api, 1.0 / 60.0);

    assert_eq!(state.as_ref().unwrap().phase, GamePhase::WaveCombat);
}

#[test]
fn wave_timer_expiry_sweeps_arena_and_runs_full_shop_loop() {
    let mut ecs = scene_world();
    let mut api = GameWorld::host(&mut ecs);
    let mut state: Option<SurvivorsState> = None;
    enter_combat_from_boot(&mut state, &mut api);

    {
        let st = state.as_mut().unwrap();
        st.spawn_timer = 99.0;
        combat::spawn_enemy_at(st, &mut api, "slime", [4.0, 4.0]);
        combat::spawn_pickup_at(st, &mut api, PickupKind::Gold(15), [10.0, 8.0]);
        st.wave_timer = 0.02;
    }

    for _ in 0..120 {
        fixed_tick(&mut state, &mut api, 1.0 / 60.0);
        if matches!(state.as_ref().unwrap().phase, GamePhase::ShopPhase { .. }) {
            break;
        }
    }

    let st = state.as_mut().unwrap();
    assert!(matches!(st.phase, GamePhase::ShopPhase { .. }));
    assert!(
        st.enemies.is_empty(),
        "wave end must clear remaining enemies"
    );
    assert!(
        st.pickups.is_empty(),
        "wave sweep must collect leftover pickups"
    );

    st.stats.gold = 500;
    if let GamePhase::ShopPhase { offers, .. } = &mut st.phase {
        *offers = vec![
            ShopOffer {
                kind: ShopOfferKind::Weapon {
                    weapon_id: "pistol".into(),
                    tier: 1,
                },
                price: 16,
                locked: false,
                purchased: false,
            },
            ShopOffer {
                kind: ShopOfferKind::Item {
                    item_id: "item_glasses".into(),
                },
                price: 22,
                locked: false,
                purchased: false,
            },
            ShopOffer {
                kind: ShopOfferKind::Weapon {
                    weapon_id: "smg".into(),
                    tier: 1,
                },
                price: 22,
                locked: false,
                purchased: false,
            },
            ShopOffer {
                kind: ShopOfferKind::Weapon {
                    weapon_id: "shotgun".into(),
                    tier: 2,
                },
                price: 48,
                locked: false,
                purchased: false,
            },
        ];
    }

    assert!(st.try_toggle_shop_lock(0));
    assert!(st.try_reroll_shop(&mut api));
    if let GamePhase::ShopPhase { offers, .. } = &st.phase {
        assert_eq!(
            offers[0].kind,
            ShopOfferKind::Weapon {
                weapon_id: "pistol".into(),
                tier: 1
            }
        );
    }

    let pre_weapons = st.weapons.len();
    assert!(st.try_buy_shop_offer(0, &mut api));
    assert_eq!(st.weapons.len(), pre_weapons + 1);

    assert!(st.has_combinable_weapons());
    assert!(st.try_combine_weapons(&mut api));
    assert_eq!(st.weapons.len(), pre_weapons);
    assert!(st
        .weapons
        .iter()
        .any(|w| w.weapon_id == "pistol" && w.tier == 2));

    while st.weapons.len() < MAX_WEAPONS {
        st.weapons.push(EquippedWeapon::new("smg", 1));
    }
    if let GamePhase::ShopPhase { offers, .. } = &mut st.phase {
        offers[1] = ShopOffer {
            kind: ShopOfferKind::Weapon {
                weapon_id: "pistol".into(),
                tier: 2,
            },
            price: 30,
            locked: false,
            purchased: false,
        };
    }
    assert!(st.try_buy_shop_offer(1, &mut api));
    assert_eq!(st.weapons.len(), MAX_WEAPONS);
    assert!(st
        .weapons
        .iter()
        .any(|w| w.weapon_id == "pistol" && w.tier == 3));

    let gold_before_sell = st.stats.gold;
    assert!(st.try_sell_weapon(MAX_WEAPONS - 1, &mut api));
    assert!(st.stats.gold > gold_before_sell);
    assert_eq!(st.weapons.len(), MAX_WEAPONS - 1);

    api.set_input(key_pressed_input(keys::SPACE));
    frame_tick(&mut state, &mut api, 1.0 / 60.0);

    let st = state.as_ref().unwrap();
    assert_eq!(st.phase, GamePhase::WaveCombat);
    assert_eq!(st.wave_index, 2);
}

#[test]
fn splash_auto_advances_and_early_skip_enters_main_menu() {
    let mut ecs = scene_world();
    let mut api = GameWorld::host(&mut ecs);
    let mut state: Option<SurvivorsState> = None;

    api.set_input(default_screen_input());
    fixed_tick(&mut state, &mut api, 1.0 / 60.0);
    frame_tick(&mut state, &mut api, 1.0 / 60.0);

    let st = state.as_ref().unwrap();
    assert_eq!(st.phase, GamePhase::Splash);
    assert!(!api.is_entity_active(st.player_hp_bg));

    let steps = ((SPLASH_DURATION + 0.2) * 30.0) as usize;
    for _ in 0..steps {
        frame_tick(&mut state, &mut api, 1.0 / 30.0);
        if state.as_ref().unwrap().phase == GamePhase::MainMenu {
            break;
        }
    }
    assert_eq!(state.as_ref().unwrap().phase, GamePhase::MainMenu);

    let mut state2: Option<SurvivorsState> = None;
    api.set_input(default_screen_input());
    frame_tick(&mut state2, &mut api, 1.0 / 60.0);
    assert_eq!(state2.as_ref().unwrap().phase, GamePhase::Splash);

    api.set_input(key_pressed_input(keys::SPACE));
    frame_tick(&mut state2, &mut api, 1.0 / 60.0);
    assert_eq!(state2.as_ref().unwrap().phase, GamePhase::MainMenu);
}

#[test]
fn main_menu_loadout_start_pause_and_return_to_menu() {
    let mut ecs = scene_world();
    let mut api = GameWorld::host(&mut ecs);
    let mut state: Option<SurvivorsState> = None;

    api.set_input(default_screen_input());
    frame_tick(&mut state, &mut api, 1.0 / 60.0);
    api.set_input(key_pressed_input(keys::SPACE));
    frame_tick(&mut state, &mut api, 1.0 / 60.0);
    assert_eq!(state.as_ref().unwrap().phase, GamePhase::MainMenu);

    api.set_input(key_pressed_input(keys::KEY_D));
    frame_tick(&mut state, &mut api, 1.0 / 60.0);
    assert_eq!(
        state.as_ref().unwrap().selected_loadout,
        StarterLoadout::Gunslinger
    );

    api.set_input(key_pressed_input(keys::ENTER));
    frame_tick(&mut state, &mut api, 1.0 / 60.0);
    api.set_input(default_screen_input());
    frame_tick(&mut state, &mut api, 1.0 / 60.0);

    let st = state.as_ref().unwrap();
    assert_eq!(st.phase, GamePhase::WaveCombat);
    assert_eq!(st.weapons[0].weapon_id, "pistol");
    assert_eq!(st.weapons[1].weapon_id, "smg");
    assert!(api.is_entity_active(st.player_hp_bg));

    api.set_input(key_pressed_input(keys::ESCAPE));
    frame_tick(&mut state, &mut api, 1.0 / 60.0);
    assert_eq!(
        state.as_ref().unwrap().phase,
        GamePhase::Paused {
            from_sweep_tenths: None
        }
    );

    let timer_before_pause = state.as_ref().unwrap().wave_timer;
    api.set_input(default_screen_input());
    for _ in 0..10 {
        fixed_tick(&mut state, &mut api, 1.0 / 60.0);
    }
    assert_eq!(state.as_ref().unwrap().wave_timer, timer_before_pause);

    api.set_input(key_pressed_input(keys::ESCAPE));
    frame_tick(&mut state, &mut api, 1.0 / 60.0);
    assert_eq!(state.as_ref().unwrap().phase, GamePhase::WaveCombat);

    api.set_input(key_pressed_input(keys::ESCAPE));
    frame_tick(&mut state, &mut api, 1.0 / 60.0);
    api.set_input(key_pressed_input(keys::ARROW_UP));
    frame_tick(&mut state, &mut api, 1.0 / 60.0);
    api.set_input(key_pressed_input(keys::ENTER));
    frame_tick(&mut state, &mut api, 1.0 / 60.0);
    assert_eq!(state.as_ref().unwrap().phase, GamePhase::MainMenu);
}

#[test]
fn settings_and_career_records_persist_via_save_data() {
    let temp_path = std::env::temp_dir().join("prisma_survivors_test_save.json");
    let _ = std::fs::remove_file(&temp_path);

    {
        let mut ecs = scene_world();
        ecs.insert_resource(prism_engine::user_data::UserStore::open(&temp_path));
        let mut api = GameWorld::host(&mut ecs);
        let mut state: Option<SurvivorsState> = None;
        enter_combat_from_boot(&mut state, &mut api);

        let st = state.as_mut().unwrap();
        st.settings.sfx_volume = 0.4;
        st.settings.show_api_telemetry = true;
        assert!(st.settings.save(&mut api));

        st.stats.kills = 42;
        st.stats.level = 5;
        st.wave_index = 3;
        st.phase = GamePhase::Victory;
        st.record_run_end(true, &mut api);
    }

    {
        let mut ecs = scene_world();
        ecs.insert_resource(prism_engine::user_data::UserStore::open(&temp_path));
        let mut api = GameWorld::host(&mut ecs);
        let mut state: Option<SurvivorsState> = None;

        api.set_input(default_screen_input());
        fixed_tick(&mut state, &mut api, 1.0 / 60.0);

        let st = state.as_ref().unwrap();
        assert!((st.settings.sfx_volume - 0.4).abs() < 0.001);
        assert!(st.settings.show_api_telemetry);
        assert_eq!(st.records.runs_played, 1);
        assert_eq!(st.records.victories, 1);
        assert_eq!(st.records.best_wave, 3);
        assert_eq!(st.records.best_kills, 42);
        assert_eq!(st.records.best_level, 5);
        assert_eq!(st.records.total_kills, 42);
    }

    let _ = std::fs::remove_file(&temp_path);
}

#[test]
fn api_diagnostics_verifies_engine_apis() {
    let temp_path = std::env::temp_dir().join("prisma_survivors_api_probe.json");
    let _ = std::fs::remove_file(&temp_path);

    let mut ecs = scene_world();
    ecs.insert_resource(prism_engine::user_data::UserStore::open(&temp_path));
    let mut api = GameWorld::host(&mut ecs);
    let mut state: Option<SurvivorsState> = None;

    api.set_input(default_screen_input());
    frame_tick(&mut state, &mut api, 1.0 / 60.0);
    api.set_input(key_pressed_input(keys::SPACE));
    frame_tick(&mut state, &mut api, 1.0 / 60.0);

    api.set_input(key_pressed_input(keys::ARROW_UP));
    frame_tick(&mut state, &mut api, 1.0 / 60.0);
    api.set_input(key_pressed_input(keys::ENTER));
    frame_tick(&mut state, &mut api, 1.0 / 60.0);

    let st = state.as_ref().unwrap();
    assert_eq!(st.modal, MenuModal::ApiDiagnostics);
    let report = st.api_report.as_ref().expect("diagnostic report generated");
    assert_eq!(report.total_count, 7);
    assert_eq!(
        report.passed_count, report.total_count,
        "all engine API checks should pass: {:?}",
        report.entries
    );

    let _ = std::fs::remove_file(&temp_path);
}

#[test]
fn language_defaults_to_chinese_and_toggles_with_persistence() {
    let temp_path = std::env::temp_dir().join("prisma_survivors_lang_test.json");
    let _ = std::fs::remove_file(&temp_path);

    {
        let mut ecs = scene_world();
        ecs.insert_resource(prism_engine::user_data::UserStore::open(&temp_path));
        let mut api = GameWorld::host(&mut ecs);
        let mut state: Option<SurvivorsState> = None;

        api.set_input(default_screen_input());
        fixed_tick(&mut state, &mut api, 1.0 / 60.0);

        let st = state.as_mut().unwrap();
        assert_eq!(st.settings.language, Language::ZhCn);
        assert_eq!(st.settings.language.display_name(), "简体中文");

        st.toggle_language(&mut api);
        assert_eq!(st.settings.language, Language::EnUs);
        assert_eq!(st.settings.language.display_name(), "English");
    }

    {
        let mut ecs = scene_world();
        ecs.insert_resource(prism_engine::user_data::UserStore::open(&temp_path));
        let mut api = GameWorld::host(&mut ecs);
        let mut state: Option<SurvivorsState> = None;

        api.set_input(default_screen_input());
        fixed_tick(&mut state, &mut api, 1.0 / 60.0);

        let st = state.as_mut().unwrap();
        assert_eq!(st.settings.language, Language::EnUs);
        st.toggle_language(&mut api);
        assert_eq!(st.settings.language, Language::ZhCn);
    }

    let _ = std::fs::remove_file(&temp_path);
}
