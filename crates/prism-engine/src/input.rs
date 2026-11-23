/// Abstract key code (platform independent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyCode {
    KeyW,
    KeyA,
    KeyS,
    KeyD,
    KeyQ,
    KeyE,
    /// `T`: no move semantics, but host / project shortcuts need it to have independent identity
    /// (previously degraded to `Other(0x54)`, mixed with Win32 `VK_T` char).
    KeyT,
    Space,
    ShiftLeft,
    ShiftRight,
    ControlLeft,
    ControlRight,
    AltLeft,
    AltRight,
    Escape,
    Tab,
    Enter,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    /// Function key. Host debug layer / editor use; project may or may not use.
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    Other(u32),
}

impl KeyCode {
    /// Stable key bit index for `InputSnapshot` bit graph host sends to project module.
    ///
    /// This is **part of cross-module ABI**: once published numbers cannot be reordered, otherwise already compiled
    /// project modules will recognize wrong keys. New keys append to end.
    /// `None` = this key code not placed into `InputSnapshot` bit graph (see [`Self::Other`] branch).
    pub fn bit_index(self) -> Option<usize> {
        Some(match self {
            Self::KeyW => 0,
            Self::KeyA => 1,
            Self::KeyS => 2,
            Self::KeyD => 3,
            Self::KeyQ => 4,
            Self::KeyE => 5,
            Self::Space => 6,
            Self::ShiftLeft => 7,
            Self::ShiftRight => 8,
            Self::ControlLeft => 9,
            Self::ControlRight => 10,
            Self::AltLeft => 11,
            Self::AltRight => 12,
            Self::Escape => 13,
            Self::Tab => 14,
            Self::Enter => 15,
            Self::ArrowUp => 16,
            Self::ArrowDown => 17,
            Self::ArrowLeft => 18,
            Self::ArrowRight => 19,
            Self::Digit0 => 20,
            Self::Digit1 => 21,
            Self::Digit2 => 22,
            Self::Digit3 => 23,
            Self::Digit4 => 24,
            Self::Digit5 => 25,
            Self::Digit6 => 26,
            Self::Digit7 => 27,
            Self::Digit8 => 28,
            Self::Digit9 => 29,
            Self::KeyT => 30,
            Self::F1 => 31,
            Self::F2 => 32,
            Self::F3 => 33,
            Self::F4 => 34,
            Self::F5 => 35,
            Self::F6 => 36,
            Self::F7 => 37,
            Self::F8 => 38,
            Self::F9 => 39,
            Self::F10 => 40,
            Self::F11 => 41,
            Self::F12 => 42,
            Self::Other(code) if code <= 212 => 43 + code as usize,
            Self::Other(_) => return None,
        })
    }
}

/// Key or button state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementState {
    Pressed,
    Released,
}

/// Mouse button abstraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

/// Touch phase (platform-independent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

/// A single touch event.
#[derive(Clone, Copy, Debug)]
pub struct TouchEvent {
    pub id: u64,
    pub phase: TouchPhase,
    pub position: [f64; 2],
}

/// Per-frame input state.
///
/// Owns the raw input state (held keys, mouse position, transient just-pressed
/// events) and the pointer-lock state machine.  Keyboard shortcuts that trigger
/// app- or editor-level actions are handled by the caller — this struct only
/// tracks which keys are pressed.
///
/// ## Usage
/// 1. Call individual handlers (`handle_keyboard`, `handle_mouse_button`,
///    `handle_mouse_move`, `handle_scroll`, `handle_touch`) from the app's
///    event handler to route window-system events into the state machine.
/// 2. Call [`begin_frame`](Self::begin_frame) at the end of each frame to
///    clear transient state.
/// 3. Query helpers (`key_held`, `mouse_delta`, etc.) during the frame's
///    update logic.
#[derive(Default, Clone)]
pub struct InputManager {
    keys_held: rustc_hash::FxHashSet<KeyCode>,
    mouse_buttons_held: rustc_hash::FxHashSet<MouseButton>,
    mouse_position: [f64; 2],

    keys_just_pressed: Vec<KeyCode>,
    keys_just_released: Vec<KeyCode>,
    mouse_just_pressed: Vec<MouseButton>,
    mouse_just_released: Vec<MouseButton>,
    mouse_delta: [f64; 2],
    scroll_delta: f64,
    touches: Vec<TouchEvent>,

    /// Whether the cursor is hidden + grabbed for FPS-style look.
    pub pointer_locked: bool,
}

impl InputManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Call at the START of each frame to reset transient state.
    pub fn begin_frame(&mut self) {
        self.keys_just_pressed.clear();
        self.keys_just_released.clear();
        self.mouse_just_pressed.clear();
        self.mouse_just_released.clear();
        self.mouse_delta = [0.0; 2];
        self.scroll_delta = 0.0;
        self.touches.clear();
    }

    pub fn key_held(&self, key: KeyCode) -> bool {
        self.keys_held.contains(&key)
    }
    pub fn key_just_pressed(&self, key: KeyCode) -> bool {
        self.keys_just_pressed.contains(&key)
    }

    /// All keys newly pressed this frame (deduplicated, already-held-and-pressed will not repeat).
    pub fn pressed_keys(&self) -> &[KeyCode] {
        &self.keys_just_pressed
    }

    /// All currently held keys (including just pressed this frame).
    ///
    /// `InputSnapshot` host sends to project module uses held semantics: gameplay code
    /// `input.key(...)` checks continuous held, not only true on press frame.
    pub fn held_keys(&self) -> impl Iterator<Item = KeyCode> + '_ {
        self.keys_held.iter().copied()
    }
    pub fn key_just_released(&self, key: KeyCode) -> bool {
        self.keys_just_released.contains(&key)
    }

    /// All keys released this frame (host fills `InputSnapshot::key_edges`).
    pub fn released_keys(&self) -> &[KeyCode] {
        &self.keys_just_released
    }
    pub fn mouse_held(&self, button: MouseButton) -> bool {
        self.mouse_buttons_held.contains(&button)
    }
    pub fn mouse_just_pressed(&self, button: MouseButton) -> bool {
        self.mouse_just_pressed.contains(&button)
    }

    /// Mouse button just released this frame (`Input.GetMouseButtonUp`).
    pub fn mouse_just_released(&self, button: MouseButton) -> bool {
        self.mouse_just_released.contains(&button)
    }

    pub fn mouse_delta(&self) -> [f64; 2] {
        self.mouse_delta
    }
    /// Accumulated vertical scroll this frame, **unit is "scroll line"** (one traditional wheel notch = 1.0,
    /// trackpad fractional lines accumulate smoothly).
    ///
    /// Platform layer already converted winit pixel delta to lines (see
    /// `prism_platform::PIXELS_PER_SCROLL_LINE`), so scale / scroll curve can follow
    /// "1.0 ≈ one notch" without distinguishing devices.
    pub fn scroll_delta(&self) -> f64 {
        self.scroll_delta
    }
    pub fn mouse_position(&self) -> [f64; 2] {
        self.mouse_position
    }
    pub fn touches(&self) -> &[TouchEvent] {
        &self.touches
    }

    pub fn handle_keyboard(&mut self, key: KeyCode, state: ElementState) {
        match state {
            ElementState::Pressed => {
                if self.keys_held.insert(key) {
                    self.keys_just_pressed.push(key);
                }
            }
            ElementState::Released => {
                if self.keys_held.remove(&key) {
                    self.keys_just_released.push(key);
                }
            }
        }
    }

    pub fn handle_mouse_move(&mut self, position: [f64; 2]) {
        self.mouse_delta[0] += position[0] - self.mouse_position[0];
        self.mouse_delta[1] += position[1] - self.mouse_position[1];
        self.mouse_position = position;
    }

    /// Set the pointer position without accumulating delta.
    pub fn set_mouse_position(&mut self, position: [f64; 2]) {
        self.mouse_position = position;
    }

    pub fn handle_mouse_button(&mut self, button: MouseButton, state: ElementState) {
        match state {
            ElementState::Pressed => {
                if self.mouse_buttons_held.insert(button) {
                    self.mouse_just_pressed.push(button);
                }
            }
            ElementState::Released => {
                if self.mouse_buttons_held.remove(&button) {
                    self.mouse_just_released.push(button);
                }
            }
        }
    }

    /// Handle a scroll event. `delta_y` is positive for scroll up (zoom in),
    /// Negative for scrolling down. Unit is scroll line (one notch = 1.0).
    pub fn handle_scroll(&mut self, delta_y: f64) {
        self.scroll_delta += delta_y;
    }

    pub fn handle_touch(&mut self, id: u64, phase: TouchPhase, position: [f64; 2]) {
        self.touches.push(TouchEvent {
            id,
            phase,
            position,
        });
    }

    /// Update the pointer-lock state without performing any window-system
    /// operations (cursor grab / visibility).  The caller is responsible for
    /// adjusting the actual cursor state via the window system.
    ///
    /// Resets accumulated mouse delta so the view doesn't jump after the
    /// transition.
    pub fn set_locked(&mut self, locked: bool) {
        self.pointer_locked = locked;
        self.begin_frame();
    }

    /// Mirror the platform pointer-lock truth into the input state.
    ///
    /// Lock state has only **one** true value: platform window (result of `grab_pointer` / `release_pointer`).
    /// Host copies it each frame, engine (camera) and project hooks (panels) read same
    /// boolean value, no conflicting "cursor already unlocked but camera still eating WASD" case.
    ///
    /// Difference from [`Self::set_locked`]: does not touch transient input when state unchanged, so can call every frame.
    pub fn sync_pointer_lock(&mut self, locked: bool) {
        if self.pointer_locked == locked {
            return;
        }
        self.set_locked(locked);
    }
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;

/// Fold engine [`InputManager`] into one [`InputSnapshot`].
///
/// `screen` is drawing area size (pixels, same coordinate system as `cursor`). Static Player
/// `GameWorld::input()` and editor plugin form share this mapping, avoiding field semantic drift.
pub fn input_snapshot(input: &InputManager, screen: [f32; 2]) -> prism_project_api::InputSnapshot {
    use prism_project_api::{InputSnapshot, TouchSnapshot};

    let mut snapshot = InputSnapshot::default();
    let cursor = input.mouse_position();
    snapshot.cursor = [cursor[0] as f32, cursor[1] as f32];
    snapshot.screen = screen;
    snapshot.left_clicked = input.mouse_just_pressed(MouseButton::Left);
    snapshot.left_held = input.mouse_held(MouseButton::Left);
    snapshot.right_clicked = input.mouse_just_pressed(MouseButton::Right);
    snapshot.right_held = input.mouse_held(MouseButton::Right);
    snapshot.middle_clicked = input.mouse_just_pressed(MouseButton::Middle);
    snapshot.middle_held = input.mouse_held(MouseButton::Middle);
    let delta = input.mouse_delta();
    snapshot.mouse_delta = [delta[0] as f32, delta[1] as f32];
    snapshot.scroll = input.scroll_delta() as f32;
    snapshot.pointer_locked = input.pointer_locked;

    let mut dropped: Option<u32> = None;
    for key in input.held_keys() {
        match key.bit_index() {
            Some(index) => snapshot.set_key(index, true),
            None => {
                if let KeyCode::Other(code) = key {
                    dropped = dropped.or(Some(code));
                }
            }
        }
    }
    for key in input.pressed_keys() {
        if let Some(index) = key.bit_index() {
            snapshot.set_key_edge(index, true);
        }
    }
    for key in input.released_keys() {
        if let Some(index) = key.bit_index() {
            snapshot.set_key_edge(index, false);
        }
    }
    if let Some(code) = dropped {
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| {
            log::warn!(
                "input: key code {code} does not fit the InputSnapshot bitmap                  (30 + code must stay below 256); keys above 225 are ignored,                  later codes are not logged"
            )
        });
    }

    let touches = input.touches();
    let count = touches.len().min(InputSnapshot::MAX_TOUCHES);
    snapshot.touch_count = count as u32;
    for (slot, touch) in snapshot.touches.iter_mut().zip(touches.iter()).take(count) {
        *slot = TouchSnapshot {
            id: touch.id,
            position: [touch.position[0] as f32, touch.position[1] as f32],
            phase: touch_phase_code(touch.phase),
            _reserved: 0,
        };
    }
    snapshot
}

/// Engine touch phase → ABI constant.
fn touch_phase_code(phase: TouchPhase) -> u32 {
    use prism_project_api::{
        TOUCH_PHASE_CANCELLED, TOUCH_PHASE_ENDED, TOUCH_PHASE_MOVED, TOUCH_PHASE_STARTED,
    };
    match phase {
        TouchPhase::Started => TOUCH_PHASE_STARTED,
        TouchPhase::Moved => TOUCH_PHASE_MOVED,
        TouchPhase::Ended => TOUCH_PHASE_ENDED,
        TouchPhase::Cancelled => TOUCH_PHASE_CANCELLED,
    }
}

#[cfg(test)]
mod abi_key_tests {
    use super::KeyCode;
    use prism_project_api::keys;

    /// Bit indices are a host/project ABI: changing `bit_index` must update
    /// `prism_project_api::keys`, or dynamic modules read the wrong inputs.
    #[test]
    fn abi_key_table_matches_bit_index() {
        assert_eq!(KeyCode::KeyW.bit_index(), Some(keys::KEY_W));
        assert_eq!(KeyCode::KeyA.bit_index(), Some(keys::KEY_A));
        assert_eq!(KeyCode::KeyS.bit_index(), Some(keys::KEY_S));
        assert_eq!(KeyCode::KeyD.bit_index(), Some(keys::KEY_D));
        assert_eq!(KeyCode::KeyQ.bit_index(), Some(keys::KEY_Q));
        assert_eq!(KeyCode::KeyE.bit_index(), Some(keys::KEY_E));
        assert_eq!(KeyCode::Space.bit_index(), Some(keys::SPACE));
        assert_eq!(KeyCode::ShiftLeft.bit_index(), Some(keys::SHIFT_LEFT));
        assert_eq!(KeyCode::ShiftRight.bit_index(), Some(keys::SHIFT_RIGHT));
        assert_eq!(KeyCode::ControlLeft.bit_index(), Some(keys::CONTROL_LEFT));
        assert_eq!(KeyCode::ControlRight.bit_index(), Some(keys::CONTROL_RIGHT));
        assert_eq!(KeyCode::AltLeft.bit_index(), Some(keys::ALT_LEFT));
        assert_eq!(KeyCode::AltRight.bit_index(), Some(keys::ALT_RIGHT));
        assert_eq!(KeyCode::Escape.bit_index(), Some(keys::ESCAPE));
        assert_eq!(KeyCode::Tab.bit_index(), Some(keys::TAB));
        assert_eq!(KeyCode::Enter.bit_index(), Some(keys::ENTER));
        assert_eq!(KeyCode::ArrowUp.bit_index(), Some(keys::ARROW_UP));
        assert_eq!(KeyCode::ArrowDown.bit_index(), Some(keys::ARROW_DOWN));
        assert_eq!(KeyCode::ArrowLeft.bit_index(), Some(keys::ARROW_LEFT));
        assert_eq!(KeyCode::ArrowRight.bit_index(), Some(keys::ARROW_RIGHT));
        assert_eq!(KeyCode::Digit0.bit_index(), Some(keys::DIGIT_0));
        assert_eq!(KeyCode::KeyT.bit_index(), Some(keys::KEY_T));
        assert_eq!(KeyCode::F1.bit_index(), Some(keys::F1));
        assert_eq!(KeyCode::F12.bit_index(), Some(keys::F12));
        assert_eq!(KeyCode::Other(5).bit_index(), Some(keys::OTHER_BASE + 5));
    }

    /// Distinct key codes must not share a bit: before #986 `Other(code)` was
    /// taken modulo the table, so two vk codes collided and pressing one rare
    /// key reported another.
    #[test]
    fn other_key_codes_never_alias_each_other() {
        let mut seen = [None; 256];
        for code in 0u32..=212 {
            let index = KeyCode::Other(code)
                .bit_index()
                .expect("code <= 212 must fit in bit graph");
            assert!(index < 256, "code {code} → {index} out of bounds");
            assert!(
                seen[index].is_none(),
                "code {code} and {:?} compete for same bit {index}",
                seen[index]
            );
            seen[index] = Some(code);
        }
    }

    /// Only 213 bitmap slots remain (`KeyT` plus `F1..=F12` take 13): codes past
    /// the table must be dropped (`None`), never folded onto another key.
    #[test]
    fn other_key_codes_above_the_table_are_dropped() {
        for code in [213u32, 226, 255, 452, 1000, u32::MAX] {
            assert_eq!(
                KeyCode::Other(code).bit_index(),
                None,
                "code {code} should not be folded into another key"
            );
        }
    }

    /// Bitmap edge: the largest representable key code lands on the last bit.
    #[test]
    fn the_last_representable_key_fills_the_last_bit() {
        assert_eq!(KeyCode::Other(212).bit_index(), Some(255));
        let mut snapshot = prism_project_api::InputSnapshot::default();
        snapshot.set_key(255, true);
        assert_eq!(
            snapshot.keys[3] >> 63,
            1,
            "Bit 255 should be highest bit of last word"
        );
    }
}
