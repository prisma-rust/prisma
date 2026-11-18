//! Platform input event translation into engine input types.

use prism_engine::input::InputManager;
use prism_platform::Key;

/// `prism-platform` 的触点 phase → 引擎输入模型的 phase。
///
/// 两者当前一一对应，但分层上必须显式转换：`prism-app` 不把平台类型直接
/// 传进引擎，避免以后任一侧加减 phase 时静默错配。
pub(super) fn engine_touch_phase(
    phase: prism_platform::TouchPhase,
) -> prism_engine::input::TouchPhase {
    use prism_engine::input::TouchPhase;
    match phase {
        prism_platform::TouchPhase::Started => TouchPhase::Started,
        prism_platform::TouchPhase::Moved => TouchPhase::Moved,
        prism_platform::TouchPhase::Ended => TouchPhase::Ended,
        prism_platform::TouchPhase::Cancelled => TouchPhase::Cancelled,
    }
}

/// 把一个平台按键事件落到 [`InputManager`] 上，返回是否需要释放指针锁定。
///
/// 平台键码 -> 引擎键码 -> `key_held` 是三条独立的表（winit 物理键、VK、
/// `KeyCode`），任何一张表漏项都会让「按了没反应」，而这条链路以前只有
/// 各段自己的单测。把换算与写入收在这里，单测能一次覆盖整条路径；
/// 真正操作窗口的 `release_pointer` 仍由调用方执行。
pub(super) fn apply_key_event(input: &mut InputManager, key: Key, pressed: bool) -> bool {
    use prism_engine::input::ElementState;

    // `Other(u32::MAX)` = 平台没给出物理键（winit 的 `Unidentified`）。
    // 旧实现直接丢弃这类事件，这里保持一致。
    if key == Key::Other(u32::MAX) {
        return false;
    }
    let key = engine_key_from_native(key);
    let release = should_release_pointer_lock(key, pressed, input.pointer_locked);
    if release {
        input.set_locked(false);
    }
    input.handle_keyboard(
        key,
        if pressed {
            ElementState::Pressed
        } else {
            ElementState::Released
        },
    );
    release
}

/// Esc 是指针锁定的逃生口：锁定态下它先释放光标，再作为普通按键交给游戏。
pub(super) fn should_release_pointer_lock(
    key: prism_engine::input::KeyCode,
    pressed: bool,
    locked: bool,
) -> bool {
    locked && pressed && key == prism_engine::input::KeyCode::Escape
}

/// 锁定态下窗口光标位置是被 confinement 夹住的，不能参与累加；此时唯一的
/// 鼠标来源是原始设备增量 [`apply_device_mouse_motion`]。
///
/// 位置本身仍然同步（不产生 delta），解锁后第一帧才不会从旧位置跳一下。
pub(super) fn apply_window_cursor_move(input: &mut InputManager, x: f64, y: f64) {
    if input.pointer_locked {
        input.set_mouse_position([x, y]);
        return;
    }
    input.handle_mouse_move([x, y]);
}

/// 原始设备增量（`DeviceEvent::MouseMotion`）：锁定态下累加到一个虚拟绝对
/// 位置，使 `InputManager::mouse_delta()` 反映原始增量。
pub(super) fn apply_device_mouse_motion(input: &mut InputManager, dx: f64, dy: f64) {
    if !input.pointer_locked {
        return;
    }
    let cur = input.mouse_position();
    input.handle_mouse_move([cur[0] + dx, cur[1] + dy]);
}

/// 中立物理键 → 引擎键码。
///
/// 转换留在 `prism-app`（同时依赖 `prism-platform` 与 `prism-engine`），
/// 而不是 `prism-platform`：平台层保持 engine-free，只产出与窗口库无关的
/// [`prism_platform::PlatformEvent`]。未列出的键退化为 `Key::Other` 的原始码，
/// 避免静默丢弃。
pub(super) fn engine_key_from_native(key: Key) -> prism_engine::input::KeyCode {
    use prism_engine::input::KeyCode as Eng;
    match key {
        Key::KeyW => Eng::KeyW,
        Key::KeyA => Eng::KeyA,
        Key::KeyS => Eng::KeyS,
        Key::KeyD => Eng::KeyD,
        Key::KeyQ => Eng::KeyQ,
        Key::KeyE => Eng::KeyE,
        Key::Space => Eng::Space,
        Key::ShiftLeft => Eng::ShiftLeft,
        Key::ShiftRight => Eng::ShiftRight,
        Key::ControlLeft => Eng::ControlLeft,
        Key::ControlRight => Eng::ControlRight,
        Key::AltLeft => Eng::AltLeft,
        Key::AltRight => Eng::AltRight,
        Key::Escape => Eng::Escape,
        Key::Tab => Eng::Tab,
        Key::Enter => Eng::Enter,
        Key::ArrowUp => Eng::ArrowUp,
        Key::ArrowDown => Eng::ArrowDown,
        Key::ArrowLeft => Eng::ArrowLeft,
        Key::ArrowRight => Eng::ArrowRight,
        Key::Digit0 => Eng::Digit0,
        Key::Digit1 => Eng::Digit1,
        Key::Digit2 => Eng::Digit2,
        Key::Digit3 => Eng::Digit3,
        Key::Digit4 => Eng::Digit4,
        Key::Digit5 => Eng::Digit5,
        Key::Digit6 => Eng::Digit6,
        Key::Digit7 => Eng::Digit7,
        Key::Digit8 => Eng::Digit8,
        Key::Digit9 => Eng::Digit9,
        Key::KeyT => Eng::KeyT,
        Key::F1 => Eng::F1,
        Key::F2 => Eng::F2,
        Key::F3 => Eng::F3,
        Key::F4 => Eng::F4,
        Key::F5 => Eng::F5,
        Key::F6 => Eng::F6,
        Key::F7 => Eng::F7,
        Key::F8 => Eng::F8,
        Key::F9 => Eng::F9,
        Key::F10 => Eng::F10,
        Key::F11 => Eng::F11,
        Key::F12 => Eng::F12,
        // 字母键进引擎后仍是「原始键码」载荷，但取自 `Key::Letter` 的 ASCII 值，
        // 与后端无关（两个后端都给出同一个 KeyCode::Other）。
        Key::Letter(byte) => Eng::Other(u32::from(byte)),
        // 未建模的键保留原始虚拟键码，不丢键。
        Key::Other(_) => Eng::Other(key.to_vk()),
    }
}
