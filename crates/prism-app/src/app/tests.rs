//! Unit tests for `App` input translation, asset path normalization, and hot reload.

#[cfg(test)]
mod tests {
    use super::super::input::{
        apply_device_mouse_motion, apply_key_event, apply_window_cursor_move,
        engine_key_from_native, engine_touch_phase, should_release_pointer_lock,
    };
    use super::super::*;
    use prism_platform::PlatformEvent;

    /// 场景里写的环境贴图路径常见三种写法，都要落到 pak 的键上。
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

    /// #1054：玩法状态在 tick 之间必须活过 `remove_resource` / `insert_resource`，
    /// 且第一次 tick 拿到 `None`（插件形态同一语义）。
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

    /// `touch_event` 走 winit 的 `WindowEvent`，只有 winit 后端才编得出来。
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

    /// 触摸必须经平台快照落到 `InputManager::touches()`，并保留触点 id 与 phase
    /// （回归防护：`handle_platform_event` 的 `_ => {}` 曾把整条链路丢掉）。
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
                other => panic!("touch 被转换成了 {other:?}"),
            }
        }

        let touches = input.touches();
        assert_eq!(touches.len(), 2);
        assert_eq!(touches[0].id, 3);
        assert_eq!(touches[0].phase, prism_engine::input::TouchPhase::Started);
        assert_eq!(touches[0].position, [40.0, 50.0]);
        assert_eq!(touches[1].phase, prism_engine::input::TouchPhase::Moved);
    }

    /// #988：UI 与 PostPass 的目标矩形重合，指针换算用同一组参数。
    #[test]
    fn ui_layout_mapping_follows_the_internal_resolution_letterbox() {
        let mapping = ui_layout_mapping(
            1600,
            900,
            [640, 360],
            prism_render::InternalScaleMode::Integer,
        );
        // 整数倍向下取整：2 倍 → 1280×720，居中留 160 / 90 的黑边。
        assert!(
            (mapping.scale - 0.8).abs() < 1e-6,
            "scale {}",
            mapping.scale
        );
        assert_eq!(mapping.offset, [160.0, 90.0]);
        // 窗口中心就是布局中心（画面中心），点哪里选哪里。
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

    /// 锁定态下窗口光标位置不再参与累加（confinement 会把它夹在窗口内）。
    #[test]
    fn locked_pointer_ignores_window_cursor_positions() {
        let mut input = InputManager::new();
        input.set_locked(true);
        apply_window_cursor_move(&mut input, 120.0, 80.0);
        assert_eq!(input.mouse_delta(), [0.0, 0.0], "窗口光标不该产生增量");

        input.set_locked(false);
        apply_window_cursor_move(&mut input, 132.0, 80.0);
        assert_eq!(input.mouse_delta(), [12.0, 0.0], "解锁后光标位置恢复正常");
    }

    /// 回归防护：锁定态下原始设备增量是唯一来源，不能与窗口光标叠加。
    #[test]
    fn raw_device_motion_is_the_only_delta_while_locked() {
        let mut input = InputManager::new();
        input.set_locked(true);
        // 夹住后的光标位置先到（以前会先累加一次 delta）。
        apply_window_cursor_move(&mut input, 640.0, 360.0);
        apply_device_mouse_motion(&mut input, 3.0, -4.0);
        assert_eq!(input.mouse_delta(), [3.0, -4.0]);
    }

    /// 未锁定时不接受原始增量（否则窗口光标与设备增量会同时生效）。
    #[test]
    fn raw_device_motion_is_ignored_when_unlocked() {
        let mut input = InputManager::new();
        apply_device_mouse_motion(&mut input, 5.0, 5.0);
        assert_eq!(input.mouse_delta(), [0.0, 0.0]);
    }

    /// Esc 是锁定态下的释放入口；其它按键和按键释放都不影响锁定。
    #[test]
    fn escape_releases_the_pointer_lock() {
        use prism_engine::input::KeyCode;
        assert!(should_release_pointer_lock(KeyCode::Escape, true, true));
        assert!(!should_release_pointer_lock(KeyCode::Escape, false, true));
        assert!(!should_release_pointer_lock(KeyCode::KeyW, true, true));
        assert!(!should_release_pointer_lock(KeyCode::Escape, true, false));
    }

    /// 未按 Shift 的 1..9、0 覆盖分量 0..9。
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

    /// 回归防护：Shift 曾经被硬编码成 `false`，分量 10..14 完全不可达。
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
            // Shift+6 是蒙皮权重热图（#560），不是 PBR 分量，但同样是 15 号 debug 位。
            (K::Digit6, 15),
        ] {
            assert_eq!(
                bindings.action_for(prism_engine::key_code_from_vk(key.to_vk())),
                Some(EngineAction::IsolatePbrComponent(bit)),
                "{key:?} with Shift"
            );
        }
        // 15 个 debug 位已经用完；Shift+7..0 与普通字母都不该抢快捷键。
        for key in [K::Digit7, K::Digit8, K::Digit9, K::Digit0, K::KeyA] {
            assert_eq!(
                bindings.action_for(prism_engine::key_code_from_vk(key.to_vk())),
                None,
                "{key:?}"
            );
        }
    }

    /// 键盘映射是手工表：抽查移动/修饰/功能/数字键，防止平台层改动后
    /// 只有部分键还能到达 `InputManager`。
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
        // 未列出的键不能丢：退化为原始码（winit 判别值 / Win32 VK）。
        assert_eq!(engine_key_from_native(K::Other(0x5A)), Eng::Other(0x5A));
    }

    /// 端到端：平台按键事件走完换算后必须能在 `InputManager` 里查到，
    /// 否则相机读 `key_held(KeyW)` 永远是 false（「WASD 不动相机」的根因）。
    #[test]
    fn platform_key_events_reach_the_engine_input_state() {
        use prism_engine::input::KeyCode;
        use prism_platform::Key as K;
        let mut input = InputManager::new();
        for key in [K::KeyW, K::KeyA, K::KeyS, K::KeyD, K::KeyQ, K::KeyE] {
            assert!(!apply_key_event(&mut input, key, true), "{key:?} 不该解锁");
            assert!(
                input.key_held(engine_key_from_native(key)),
                "{key:?} 按下后不在 key_held 里"
            );
            assert!(!apply_key_event(&mut input, key, false), "{key:?} 不该解锁");
            assert!(
                !input.key_held(engine_key_from_native(key)),
                "{key:?} 抬起后仍在 key_held 里"
            );
        }
        // 只走 `LETTER_KEYS` 兜底的那些键同样不能丢。
        assert!(!apply_key_event(&mut input, K::Letter(b'R'), true));
        assert!(input.key_held(KeyCode::Other(u32::from(b'R'))));
    }

    /// Esc 在锁定态下先释放锁定，然后仍作为普通按键交给游戏。
    #[test]
    fn escape_unlocks_and_still_reaches_the_game() {
        use prism_engine::input::KeyCode;
        use prism_platform::Key as K;
        let mut input = InputManager::new();
        input.set_locked(true);
        assert!(
            apply_key_event(&mut input, K::Escape, true),
            "锁定态 Esc 要解锁"
        );
        assert!(!input.pointer_locked, "Esc 之后必须已经解锁");
        assert!(input.key_held(KeyCode::Escape), "解锁不影响 Esc 本身的分发");
        // 已解锁时再按 Esc 不重复触发。
        assert!(!apply_key_event(&mut input, K::Escape, true));
    }

    /// winit 的 `Unidentified`（`Other(u32::MAX)`）必须被丢弃，不能塞进输入表。
    #[test]
    fn unidentified_keys_are_dropped() {
        use prism_engine::input::KeyCode;
        use prism_platform::Key as K;
        let mut input = InputManager::new();
        input.set_locked(true);
        assert!(!apply_key_event(&mut input, K::Other(u32::MAX), true));
        assert!(!input.key_held(KeyCode::Other(u32::MAX)));
        assert!(input.pointer_locked, "丢弃事件不该动锁定状态");
    }
}

#[cfg(test)]
mod hot_reload_tests {
    use super::super::hot_reload::hot_reload_enabled;

    /// #495：只有显式的「开」才建轮询线程，发布版默认完全不做额外工作。
    #[test]
    fn only_explicit_truthy_values_enable_hot_reload() {
        for value in ["1", "true", "TRUE", " yes ", "On"] {
            assert!(hot_reload_enabled(Some(value)), "{value}");
        }
        for value in ["0", "false", "", "  ", "off", "no", "2", "enabled"] {
            assert!(!hot_reload_enabled(Some(value)), "{value}");
        }
        assert!(!hot_reload_enabled(None), "未设置 = 关");
    }
}
