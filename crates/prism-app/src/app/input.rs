//! Platform input event translation into engine input types.

use prism_engine::input::InputManager;
use prism_platform::Key;

/// `prism-platform` touch phase -> engine input model phase.
///
/// Both currently one-to-one, but layering must explicitly convert: `prism-app` does not pass platform type directly
/// into engine, to avoid silent mismatch when either side adds/removes phase later.
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

/// Drop a platform key event onto [`InputManager`], return whether pointer lock should be released.
///
/// Platform keycode -> engine keycode -> `key_held` are three independent tables (winit physical key, VK,
/// `KeyCode`), any table missing entry will cause "pressed but no response", and this chain previously only had
/// each segment's own unit tests. Put conversion and write here, unit test can cover whole path at once;
/// Real window `release_pointer` operation still performed by caller.
pub(super) fn apply_key_event(input: &mut InputManager, key: Key, pressed: bool) -> bool {
    use prism_engine::input::ElementState;

    // `Other(u32::MAX)` = platform gave no physical key (winit's `Unidentified`).
    // Old implementation directly discarded such events, keep consistent here.
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

/// Esc is pointer lock escape hatch: under lock it first releases cursor, then acts as normal key to game.
pub(super) fn should_release_pointer_lock(
    key: prism_engine::input::KeyCode,
    pressed: bool,
    locked: bool,
) -> bool {
    locked && pressed && key == prism_engine::input::KeyCode::Escape
}

/// mouse source is raw device delta [`apply_device_mouse_motion`].
///
/// Position itself still synced (no delta produced); after unlock first frame won\'t jump from old position.
pub(super) fn apply_window_cursor_move(input: &mut InputManager, x: f64, y: f64) {
    if input.pointer_locked {
        input.set_mouse_position([x, y]);
        return;
    }
    input.handle_mouse_move([x, y]);
}

/// Raw device delta (`DeviceEvent::MouseMotion`): under lock accumulate into a virtual absolute
/// position, so `InputManager::mouse_delta()` reflects raw delta.
pub(super) fn apply_device_mouse_motion(input: &mut InputManager, dx: f64, dy: f64) {
    if !input.pointer_locked {
        return;
    }
    let cur = input.mouse_position();
    input.handle_mouse_move([cur[0] + dx, cur[1] + dy]);
}

/// Neutral physical key -> engine keycode.
///
/// Conversion stays in `prism-app` (depends on both `prism-platform` and `prism-engine`),
/// instead of `prism-platform`: platform layer stays engine-free, only produces window-lib-independent
/// [`prism_platform::PlatformEvent`]. Unlisted keys degrade to `Key::Other` raw code,
/// avoid silent drop.
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
        // Letter key enters engine still as "raw keycode" payload, but takes ASCII value from `Key::Letter`,
        // backend-independent (both backends give same KeyCode::Other).
        Key::Letter(byte) => Eng::Other(u32::from(byte)),
        // Unmapped keys keep raw virtual keycode, do not drop key.
        Key::Other(_) => Eng::Other(key.to_vk()),
    }
}
