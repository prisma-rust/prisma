//! Orthogonal interaction behaviors composed onto UI nodes via `SecondaryMap`.

use glam::Vec2;

use crate::tree::NodeKey;

/// Semantic action emitted by `UiTree::process_input` without holding mutable closures.
#[derive(Clone, Debug, PartialEq)]
pub enum UiAction {
    Clicked(NodeKey),
    DoubleClicked(NodeKey),
    Toggled {
        key: NodeKey,
        value: bool,
    },
    ValueChanged {
        key: NodeKey,
        value: f64,
    },
    ValueCommitted {
        key: NodeKey,
        old: f64,
        new: f64,
    },
    TextChanged {
        key: NodeKey,
        text: String,
    },
    TextCommitted {
        key: NodeKey,
        text: String,
    },
    Scrolled {
        key: NodeKey,
        offset: Vec2,
    },
    Dropped {
        target: NodeKey,
        payload: DragPayload,
    },
    FocusChanged(Option<NodeKey>),
}

/// Unified drag-and-drop payload shared by Hierarchy, Asset Browser, and Dock Tabs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DragPayload {
    Entity(u32),
    AssetPath(String),
    DockTab(String),
    Custom { kind: String, data: String },
}

/// Click, double-click, and optional toggle state behavior.
#[derive(Clone, Debug, Default)]
pub struct Interactable {
    pub toggle_state: Option<bool>,
    pub emit_double_click: bool,
    pub last_click_time: f64,
}

/// Keyboard focus and shortcut scope participation.
#[derive(Clone, Copy, Debug, Default)]
pub struct Focusable {
    pub tab_index: i32,
    pub captures_text_keys: bool,
}

/// Horizontal drag-to-scrub numeric behavior (shared by NumericField, Slider, Vec3, Splitter).
#[derive(Clone, Copy, Debug)]
pub struct DraggableValue {
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub drag_sensitivity: f64,
    pub drag_start_value: Option<f64>,
    pub drag_start_x: f32,
}

impl Default for DraggableValue {
    fn default() -> Self {
        Self {
            value: 0.0,
            min: f64::NEG_INFINITY,
            max: f64::INFINITY,
            step: 0.01,
            drag_sensitivity: 0.5,
            drag_start_value: None,
            drag_start_x: 0.0,
        }
    }
}

impl DraggableValue {
    pub fn apply_delta(&mut self, delta_px: f32, fine: bool, snap: bool) {
        let base = self.drag_start_value.unwrap_or(self.value);
        let scale = if fine { 0.1 } else { 1.0 };
        let mut next = base + (delta_px as f64) * self.step * self.drag_sensitivity * scale;
        if snap && self.step > 0.0 {
            let snap_step = self.step * 10.0;
            next = (next / snap_step).round() * snap_step;
        }
        self.value = next.clamp(self.min, self.max);
    }
}

/// Retained scroll offset and virtualized item range calculation.
#[derive(Clone, Copy, Debug, Default)]
pub struct Scrollable {
    pub offset: Vec2,
    pub content_size: Vec2,
    pub horizontal: bool,
    pub vertical: bool,
}

impl Scrollable {
    /// Compute the visible item range `[start, end)` for fixed-height virtualized lists/trees.
    pub fn visible_range(
        &self,
        item_count: usize,
        item_height: f32,
        viewport_h: f32,
    ) -> (usize, usize) {
        if item_count == 0 || item_height <= 0.0 || viewport_h <= 0.0 {
            return (0, 0);
        }
        let first = (self.offset.y / item_height).floor().max(0.0) as usize;
        let visible_count = (viewport_h / item_height).ceil() as usize + 2;
        let start = first.min(item_count);
        let end = (start + visible_count).min(item_count);
        (start, end)
    }

    pub fn clamp(&mut self, viewport_size: Vec2) {
        let max_x = (self.content_size.x - viewport_size.x).max(0.0);
        let max_y = (self.content_size.y - viewport_size.y).max(0.0);
        self.offset.x = if self.horizontal {
            self.offset.x.clamp(0.0, max_x)
        } else {
            0.0
        };
        self.offset.y = if self.vertical {
            self.offset.y.clamp(0.0, max_y)
        } else {
            0.0
        };
    }
}

/// Retained text editing state (single-line or multi-line).
#[derive(Clone, Debug, Default)]
pub struct TextEditable {
    pub text: String,
    pub placeholder: String,
    pub cursor: usize,
    pub selection_anchor: Option<usize>,
    pub multiline: bool,
    pub editing: bool,
}

impl TextEditable {
    pub fn insert_str(&mut self, s: &str) {
        self.delete_selection();
        let idx = self.cursor.min(self.text.len());
        self.text.insert_str(idx, s);
        self.cursor = idx + s.len();
    }

    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor == 0 || self.text.is_empty() {
            return;
        }
        let prev = self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.text.replace_range(prev..self.cursor, "");
        self.cursor = prev;
    }

    fn delete_selection(&mut self) -> bool {
        if let Some(anchor) = self.selection_anchor.take() {
            let (a, b) = if anchor <= self.cursor {
                (anchor, self.cursor)
            } else {
                (self.cursor, anchor)
            };
            if a < b && b <= self.text.len() {
                self.text.replace_range(a..b, "");
                self.cursor = a;
                return true;
            }
        }
        false
    }
}
