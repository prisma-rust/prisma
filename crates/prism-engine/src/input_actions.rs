//! Engine-layer **key binding**: separates "which action" from "which key pressed".
//!
//! Two rules:
//!
//! 1. **No default bindings**. Engine itself has no "global shortcuts" — HUD / debug graph / tone map / PBR isolation
//!    are just fields on `RenderSettings`, **whoever wants binds them**. Host (prism-app debug layer, editor)
//!    declares via [`KeyBindings::default_engine_debug`]; project declaring nothing occupies no keys.
//! 2. **Engine-bound keys ask project first**. Before host dispatch it first calls `FrameHook::on_platform_event`,
//!    hook returns true then engine bindings apply — project always has final say.

use crate::input::KeyCode;

/// Engine built-in debug actions.
///
/// These actions only change debug fields of [`crate::render_settings::RenderSettings`],
/// by default **no key bindings** (see module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EngineAction {
    /// Toggle runtime diagnostics HUD.
    ToggleHud,
    /// Cycle PostPass debug graph (0 = normal / 1 = depth / 2 = view-space normal).
    CycleDebugTarget,
    /// Cycle tone map operator (0 = Reinhard / 1 = ACES / 2 = passthrough).
    CycleTonemap,
    /// PBR component isolation: show only component `bit` (0..=14), bit 15 is skinning weight heatmap.
    IsolatePbrComponent(u32),
}

/// Number keys → PBR component. Two semantics differ only by Shift, so one table.
///
/// Without Shift: 1..=9 → 0..=8, 0 → 9.
/// With Shift: 1..=5 → 10..=14, 6 → 15 (skinning weight heatmap), other number keys unbound.
fn digit_bindings(shift: bool) -> Vec<(KeyCode, u32)> {
    let digits = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
        KeyCode::Digit0,
    ];
    digits
        .iter()
        .enumerate()
        .filter_map(|(index, key)| {
            let index = index as u32;
            let bit = if shift {
                if index < 6 {
                    10 + index
                } else {
                    return None;
                }
            } else {
                index
            };
            Some((*key, bit))
        })
        .collect()
}

/// One binding: a key triggers an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyBinding {
    pub key: KeyCode,
    pub action: EngineAction,
}

/// A binding table. Empty table is default value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyBindings {
    bindings: Vec<KeyBinding>,
}

impl KeyBindings {
    /// Empty table: engine occupies no keys.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Default bindings for host debug layer (prism-app `engine-debug` feature calls this).
    ///
    /// F3 = HUD, Tab = debug graph, T = tone map, 1..=9 = PBR components 0..=8,
    /// Shift+1..=Shift+6 = PBR components 9..=14. Letter movement keys all unbound.
    ///
    /// This table does not auto-apply: if project does not enable debug layer, or consumes event in hook, it can fully take over these keys.
    pub fn default_engine_debug() -> Self {
        let mut bindings = Vec::new();
        bindings.push(KeyBinding {
            key: KeyCode::F3,
            action: EngineAction::ToggleHud,
        });
        bindings.push(KeyBinding {
            key: KeyCode::Tab,
            action: EngineAction::CycleDebugTarget,
        });
        bindings.push(KeyBinding {
            key: KeyCode::KeyT,
            action: EngineAction::CycleTonemap,
        });
        for (key, bit) in digit_bindings(false) {
            bindings.push(KeyBinding {
                key,
                action: EngineAction::IsolatePbrComponent(bit),
            });
        }
        Self { bindings }
    }

    /// Host debug layer bindings, two sets of number-key semantics by Shift state.
    ///
    /// Without Shift `1..=9, 0` are PBR components 0..=9; with Shift `1..=5` are 10..=14,
    /// `6` is skinning weight heatmap (bit 15). Letter keys and other function keys unaffected by Shift.
    ///
    /// Passing Shift as parameter rather than a second table is because it only changes number key **meaning**, not binding set.
    pub fn engine_debug(shift: bool) -> Self {
        let mut bindings = Self::default_engine_debug();
        bindings
            .bindings
            .retain(|binding| !matches!(binding.action, EngineAction::IsolatePbrComponent(_)));
        for (key, bit) in digit_bindings(shift) {
            bindings.bind(key, EngineAction::IsolatePbrComponent(bit));
        }
        bindings
    }

    /// Bind a key; when same key bound repeatedly **last added** wins.
    pub fn bind(&mut self, key: KeyCode, action: EngineAction) -> &mut Self {
        self.bindings.push(KeyBinding { key, action });
        self
    }

    /// Whether any binding exists.
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// All bindings (order is insertion order; lookup uses later overriding earlier).
    pub fn iter(&self) -> impl Iterator<Item = &KeyBinding> {
        self.bindings.iter()
    }

    /// Look up action triggered by a key; with duplicate bindings takes latest added.
    pub fn action_for(&self, key: KeyCode) -> Option<EngineAction> {
        self.bindings
            .iter()
            .rev()
            .find(|binding| binding.key == key)
            .map(|binding| binding.action)
    }
}

fn digit_key(digit: u32) -> KeyCode {
    match digit {
        0 => KeyCode::Digit0,
        1 => KeyCode::Digit1,
        2 => KeyCode::Digit2,
        3 => KeyCode::Digit3,
        4 => KeyCode::Digit4,
        5 => KeyCode::Digit5,
        6 => KeyCode::Digit6,
        7 => KeyCode::Digit7,
        8 => KeyCode::Digit8,
        _ => KeyCode::Digit9,
    }
}

fn f_key(index: u32) -> KeyCode {
    match index {
        0 => KeyCode::F1,
        1 => KeyCode::F2,
        2 => KeyCode::F3,
        3 => KeyCode::F4,
        4 => KeyCode::F5,
        5 => KeyCode::F6,
        6 => KeyCode::F7,
        7 => KeyCode::F8,
        8 => KeyCode::F9,
        9 => KeyCode::F10,
        10 => KeyCode::F11,
        _ => KeyCode::F12,
    }
}

/// Platform-layer virtual key code (`prism_platform::Key::to_vk()`, Win32 `VK_*` contract) → engine key code.
///
/// Conversion takes only a `u32`, so placed in engine: both window backends share same table, and testable.
/// Keys not in table keep raw code (`KeyCode::Other(vk)`), project recognizes by literal.
pub fn key_code_from_vk(vk: u32) -> KeyCode {
    match vk {
        0x57 => KeyCode::KeyW,
        0x41 => KeyCode::KeyA,
        0x53 => KeyCode::KeyS,
        0x44 => KeyCode::KeyD,
        0x51 => KeyCode::KeyQ,
        0x45 => KeyCode::KeyE,
        0x54 => KeyCode::KeyT,
        0x20 => KeyCode::Space,
        0xA0 => KeyCode::ShiftLeft,
        0xA1 => KeyCode::ShiftRight,
        0xA2 => KeyCode::ControlLeft,
        0xA3 => KeyCode::ControlRight,
        0xA4 => KeyCode::AltLeft,
        0xA5 => KeyCode::AltRight,
        0x1B => KeyCode::Escape,
        0x09 => KeyCode::Tab,
        0x0D => KeyCode::Enter,
        0x26 => KeyCode::ArrowUp,
        0x28 => KeyCode::ArrowDown,
        0x25 => KeyCode::ArrowLeft,
        0x27 => KeyCode::ArrowRight,
        0x30..=0x39 => digit_key(vk - 0x30),
        0x70..=0x7B => f_key(vk - 0x70),
        other => KeyCode::Other(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default binding table is **empty**: engine may not occupy any key when not requested.
    #[test]
    fn engine_binds_no_keys_by_default() {
        let bindings = KeyBindings::empty();
        assert!(bindings.is_empty());
        for vk in [0x09u32, 0x54, 0x70, 0x31, 0x52] {
            assert_eq!(bindings.action_for(key_code_from_vk(vk)), None);
        }
    }

    /// Host debug layer bindings cover F3 / Tab / T / number keys, and do not touch letter movement keys.
    #[test]
    fn host_debug_bindings_cover_the_documented_keys() {
        let bindings = KeyBindings::default_engine_debug();
        assert_eq!(
            bindings.action_for(KeyCode::F3),
            Some(EngineAction::ToggleHud)
        );
        assert_eq!(
            bindings.action_for(KeyCode::Tab),
            Some(EngineAction::CycleDebugTarget)
        );
        assert_eq!(
            bindings.action_for(KeyCode::KeyT),
            Some(EngineAction::CycleTonemap)
        );
        assert_eq!(
            bindings.action_for(KeyCode::Digit1),
            Some(EngineAction::IsolatePbrComponent(0))
        );
        assert_eq!(
            bindings.action_for(KeyCode::Digit0),
            Some(EngineAction::IsolatePbrComponent(9))
        );
        assert_eq!(
            bindings.action_for(KeyCode::Digit9),
            Some(EngineAction::IsolatePbrComponent(8))
        );
        for key in [
            KeyCode::KeyW,
            KeyCode::KeyA,
            KeyCode::KeyS,
            KeyCode::KeyD,
            KeyCode::KeyQ,
            KeyCode::KeyE,
            KeyCode::Space,
        ] {
            assert_eq!(
                bindings.action_for(key),
                None,
                "{key:?} should not be occupied by debug layer"
            );
        }
    }

    /// `Key::to_vk` contract (Win32 virtual key code) → engine key code.
    #[test]
    fn virtual_key_codes_map_to_engine_keys() {
        assert_eq!(key_code_from_vk(0x57), KeyCode::KeyW);
        assert_eq!(key_code_from_vk(0x51), KeyCode::KeyQ);
        assert_eq!(key_code_from_vk(0x45), KeyCode::KeyE);
        assert_eq!(key_code_from_vk(0x54), KeyCode::KeyT);
        assert_eq!(key_code_from_vk(0x09), KeyCode::Tab);
        assert_eq!(key_code_from_vk(0x70), KeyCode::F1);
        assert_eq!(key_code_from_vk(0x72), KeyCode::F3);
        assert_eq!(key_code_from_vk(0x7B), KeyCode::F12);
        assert_eq!(key_code_from_vk(0x31), KeyCode::Digit1);
        assert_eq!(key_code_from_vk(0x52), KeyCode::Other(0x52));
    }
}
