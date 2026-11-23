//! Unit tests for `App` input translation, asset path normalization, and hot reload.

#[cfg(test)]
mod cases {
    use super::super::input::{
        apply_device_mouse_motion, apply_key_event, apply_window_cursor_move,
        engine_key_from_native, engine_touch_phase, should_release_pointer_lock,
    };
    use super::super::*;
    use prism_platform::PlatformEvent;

    /// Environment texture path written in scene has three common forms, all land on pak key.
    #[test]
    fn environment_paths_normalize_to_package_keys() {
        assert_eq!(
            normalize_asset_path("../valley_of_desolation_1k.hdr"),
            "valley_of_desolation_1k.hdr"
        );
        assert_eq!(normalize_asset_path("assets/env/sky.hdr"), "env/sky.hdr");
        assert_eq!(normalize_asset_path("assets\\env\\sky.hdr"), "env/sky.hdr");
        assert_eq!(normalize_asset_path("./env/./sky.hdr"), "env/sky.hdr");
        assert_eq!(
            normalize_asset_path("assets/env/../env/sky.hdr"),
            "env/sky.hdr"
        );
    }

    /// #1054: Gameplay state must survive across `remove_resource` / `insert_resource` between ticks,
    /// and first tick gets `None` (plugin form same semantics).
    #[test]
    fn gameplay_state_survives_between_ticks() {
        let mut world = prism_engine::ecs::World::new();
        let mut first_tick_state = None;
        let mut tick =
            |state: &mut Option<u32>, _world: &mut prism_game_api::GameWorld<'_>, _dt: f32| {
                first_tick_state = first_tick_state.or(Some(*state));
                *state = Some(state.unwrap_or(41) + 1);
            };

        tick_gameplay_state::<u32, _>(&mut world, &mut tick, 0.0);
        tick_gameplay_state::<u32, _>(&mut world, &mut tick, 0.0);

        assert_eq!(
            first_tick_state,
            Some(None),
            "the first tick starts unbound"
        );
        assert_eq!(
            world
                .get_resource::<GameplayState<u32>>()
                .map(|held| held.0),
            Some(Some(43)),
            "the state must be put back after every tick"
        );
    }

    /// `touch_event` goes through winit's `WindowEvent`, only winit backend compiles it.
    #[cfg(feature = "winit-backend")]
    fn touch_event(phase: winit::event::TouchPhase, id: u64) -> WindowEvent {
        WindowEvent::Touch(winit::event::Touch {
            device_id: winit::event::DeviceId::dummy(),
            phase,
            location: winit::dpi::PhysicalPosition::new(40.0, 50.0),
            force: Some(winit::event::Force::Normalized(1.0)),
            id,
        })
    }

    /// Touch must reach `InputManager::touches()` via platform snapshot, keep touch id and phase
    /// (regression guard: `handle_platform_event` `_ => {}` once dropped whole chain).
    #[cfg(feature = "winit-backend")]
    #[test]
    fn touch_snapshot_reaches_input_manager() {
        let mut input = InputManager::new();
        for (phase, id) in [
            (winit::event::TouchPhase::Started, 3u64),
            (winit::event::TouchPhase::Moved, 3u64),
        ] {
            match prism_platform::to_platform_event(&touch_event(phase, id)) {
                PlatformEvent::Touch { id, x, y, phase } => {
                    input.handle_touch(id, engine_touch_phase(phase), [x, y])
                }
                other => panic!("touch converted to {other:?}"),
            }
        }

        let touches = input.touches();
        assert_eq!(touches.len(), 2);
        assert_eq!(touches[0].id, 3);
        assert_eq!(touches[0].phase, prism_engine::input::TouchPhase::Started);
        assert_eq!(touches[0].position, [40.0, 50.0]);
        assert_eq!(touches[1].phase, prism_engine::input::TouchPhase::Moved);
    }

    /// #988: UI and PostPass target rect overlap, pointer conversion uses same set of params.
    #[test]
    fn ui_layout_mapping_follows_the_internal_resolution_letterbox() {
        let mapping = ui_layout_mapping(
            1600,
            900,
            [640, 360],
            prism_render::InternalScaleMode::Integer,
        );
        // Integer floor down: 2x -> 1280x720, centered leaves 160/90 black borders.
        assert!(
            (mapping.scale - 0.8).abs() < 1e-6,
            "scale {}",
            mapping.scale
        );
        assert_eq!(mapping.offset, [160.0, 90.0]);
        // Window center is layout center (screen center), click where you select.
        assert_eq!(mapping.window_to_layout([800.0, 450.0]), [800.0, 450.0]);
    }

    #[test]
    fn ui_layout_mapping_is_identity_without_an_internal_resolution() {
        for internal in [[0, 0], [0, 360], [640, 0]] {
            let mapping = ui_layout_mapping(
                1600,
                900,
                internal,
                prism_render::InternalScaleMode::Stretch,
            );
            assert_eq!(mapping, prism_engine::ui::UiScreenMapping::default());
        }
    }

    /// Locked state window cursor position no longer participates in accumulation (confinement clamps it inside window).
    #[test]
    fn locked_pointer_ignores_window_cursor_positions() {
        let mut input = InputManager::new();
        input.set_locked(true);
        apply_window_cursor_move(&mut input, 120.0, 80.0);
        assert_eq!(
            input.mouse_delta(),
            [0.0, 0.0],
            "window cursor should not produce delta"
        );

        input.set_locked(false);
        apply_window_cursor_move(&mut input, 132.0, 80.0);
        assert_eq!(
            input.mouse_delta(),
            [12.0, 0.0],
            "cursor position restores normal after unlock"
        );
    }

    /// Regression guard: locked state raw device delta is only source, cannot stack with window cursor.
    #[test]
    fn raw_device_motion_is_the_only_delta_while_locked() {
        let mut input = InputManager::new();
        input.set_locked(true);
        // Clamped cursor position arrives first (previously delta was accumulated first).
        apply_window_cursor_move(&mut input, 640.0, 360.0);
        apply_device_mouse_motion(&mut input, 3.0, -4.0);
        assert_eq!(input.mouse_delta(), [3.0, -4.0]);
    }

    /// Unlocked state does not accept raw delta (otherwise window cursor and device delta both take effect).
    #[test]
    fn raw_device_motion_is_ignored_when_unlocked() {
        let mut input = InputManager::new();
        apply_device_mouse_motion(&mut input, 5.0, 5.0);
        assert_eq!(input.mouse_delta(), [0.0, 0.0]);
    }

    /// Esc is release entry under locked state; other keys and key releases do not affect lock.
    #[test]
    fn escape_releases_the_pointer_lock() {
        use prism_engine::input::KeyCode;
        assert!(should_release_pointer_lock(KeyCode::Escape, true, true));
        assert!(!should_release_pointer_lock(KeyCode::Escape, false, true));
        assert!(!should_release_pointer_lock(KeyCode::KeyW, true, true));
        assert!(!should_release_pointer_lock(KeyCode::Escape, true, false));
    }

    /// Unshifted 1..9, 0 cover components 0..9.
    #[test]
    fn plain_digits_select_the_lower_pbr_components() {
        use prism_engine::EngineAction;
        use prism_platform::Key as K;
        let bindings = prism_engine::KeyBindings::engine_debug(false);
        for (key, bit) in [
            (K::Digit1, 0),
            (K::Digit2, 1),
            (K::Digit3, 2),
            (K::Digit4, 3),
            (K::Digit5, 4),
            (K::Digit6, 5),
            (K::Digit7, 6),
            (K::Digit8, 7),
            (K::Digit9, 8),
            (K::Digit0, 9),
        ] {
            assert_eq!(
                bindings.action_for(prism_engine::key_code_from_vk(key.to_vk())),
                Some(EngineAction::IsolatePbrComponent(bit)),
                "{key:?} without Shift"
            );
        }
    }

    /// Regression guard: Shift was once hardcoded `false`, components 10..14 completely unreachable.
    #[test]
    fn shift_digits_select_the_upper_pbr_components() {
        use prism_engine::EngineAction;
        use prism_platform::Key as K;
        let bindings = prism_engine::KeyBindings::engine_debug(true);
        for (key, bit) in [
            (K::Digit1, 10),
            (K::Digit2, 11),
            (K::Digit3, 12),
            (K::Digit4, 13),
            (K::Digit5, 14),
            // Shift+6 is skin weight heatmap (#560), not PBR component, but also debug bit 15.
            (K::Digit6, 15),
        ] {
            assert_eq!(
                bindings.action_for(prism_engine::key_code_from_vk(key.to_vk())),
                Some(EngineAction::IsolatePbrComponent(bit)),
                "{key:?} with Shift"
            );
        }
        // 15 debug bits already used; Shift+7..0 with normal letters should not grab hotkeys.
        for key in [K::Digit7, K::Digit8, K::Digit9, K::Digit0, K::KeyA] {
            assert_eq!(
                bindings.action_for(prism_engine::key_code_from_vk(key.to_vk())),
                None,
                "{key:?}"
            );
        }
    }

    /// Keyboard mapping is hand table: spot-check movement/modifier/function/digit keys, prevent platform layer change after
    /// only some keys still reach `InputManager`.
    #[test]
    fn native_keys_map_to_engine_keys() {
        use prism_engine::input::KeyCode as Eng;
        use prism_platform::Key as K;
        for (key, expected) in [
            (K::KeyW, Eng::KeyW),
            (K::KeyA, Eng::KeyA),
            (K::KeyS, Eng::KeyS),
            (K::KeyD, Eng::KeyD),
            (K::KeyQ, Eng::KeyQ),
            (K::KeyE, Eng::KeyE),
            (K::Space, Eng::Space),
            (K::ShiftLeft, Eng::ShiftLeft),
            (K::ShiftRight, Eng::ShiftRight),
            (K::ControlLeft, Eng::ControlLeft),
            (K::ControlRight, Eng::ControlRight),
            (K::AltLeft, Eng::AltLeft),
            (K::AltRight, Eng::AltRight),
            (K::Escape, Eng::Escape),
            (K::Tab, Eng::Tab),
            (K::Enter, Eng::Enter),
            (K::ArrowUp, Eng::ArrowUp),
            (K::ArrowDown, Eng::ArrowDown),
            (K::ArrowLeft, Eng::ArrowLeft),
            (K::ArrowRight, Eng::ArrowRight),
            (K::Digit0, Eng::Digit0),
            (K::Digit5, Eng::Digit5),
            (K::Digit9, Eng::Digit9),
            (K::KeyT, Eng::KeyT),
            (K::F1, Eng::F1),
            (K::F3, Eng::F3),
            (K::F12, Eng::F12),
        ] {
            assert_eq!(engine_key_from_native(key), expected, "{key:?}");
        }
        // Unlisted keys cannot be dropped: degrade to raw code (winit discriminant / Win32 VK).
        assert_eq!(engine_key_from_native(K::Other(0x5A)), Eng::Other(0x5A));
    }

    /// End-to-end: platform key event after conversion must be findable in `InputManager`,
    /// otherwise camera reading `key_held(KeyW)` forever false ("WASD doesn't move camera" root cause).
    #[test]
    fn platform_key_events_reach_the_engine_input_state() {
        use prism_engine::input::KeyCode;
        use prism_platform::Key as K;
        let mut input = InputManager::new();
        for key in [K::KeyW, K::KeyA, K::KeyS, K::KeyD, K::KeyQ, K::KeyE] {
            assert!(
                !apply_key_event(&mut input, key, true),
                "{key:?} should not unlock"
            );
            assert!(
                input.key_held(engine_key_from_native(key)),
                "{key:?} not in key_held after press"
            );
            assert!(
                !apply_key_event(&mut input, key, false),
                "{key:?} should not unlock"
            );
            assert!(
                !input.key_held(engine_key_from_native(key)),
                "{key:?} still in key_held after release"
            );
        }
        // Keys only covered by `LETTER_KEYS` fallback also cannot be dropped.
        assert!(!apply_key_event(&mut input, K::Letter(b'R'), true));
        assert!(input.key_held(KeyCode::Other(u32::from(b'R'))));
    }

    /// Esc under locked state first releases lock, then still acts as normal key to game.
    #[test]
    fn escape_unlocks_and_still_reaches_the_game() {
        use prism_engine::input::KeyCode;
        use prism_platform::Key as K;
        let mut input = InputManager::new();
        input.set_locked(true);
        assert!(
            apply_key_event(&mut input, K::Escape, true),
            "locked state Esc must unlock"
        );
        assert!(!input.pointer_locked, "after Esc must already be unlocked");
        assert!(
            input.key_held(KeyCode::Escape),
            "unlock does not affect Esc itself being dispatched"
        );
        // Pressing Esc again when already unlocked does not re-trigger.
        assert!(!apply_key_event(&mut input, K::Escape, true));
    }

    /// winit's `Unidentified` (`Other(u32::MAX)`) must be dropped, not stuffed into input table.
    #[test]
    fn unidentified_keys_are_dropped() {
        use prism_engine::input::KeyCode;
        use prism_platform::Key as K;
        let mut input = InputManager::new();
        input.set_locked(true);
        assert!(!apply_key_event(&mut input, K::Other(u32::MAX), true));
        assert!(!input.key_held(KeyCode::Other(u32::MAX)));
        assert!(
            input.pointer_locked,
            "dropping event should not change lock state"
        );
    }
}

#[cfg(test)]
mod hot_reload_tests {
    use super::super::hot_reload::hot_reload_enabled;

    /// #495: Only explicit "on" builds poller thread, release build completely does no extra work.
    #[test]
    fn only_explicit_truthy_values_enable_hot_reload() {
        for value in ["1", "true", "TRUE", " yes ", "On"] {
            assert!(hot_reload_enabled(Some(value)), "{value}");
        }
        for value in ["0", "false", "", "  ", "off", "no", "2", "enabled"] {
            assert!(!hot_reload_enabled(Some(value)), "{value}");
        }
        assert!(!hot_reload_enabled(None), "unset = off");
    }
}
