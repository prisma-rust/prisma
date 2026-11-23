//! Retained-mode `SlotMap` UI tree, input routing, and draw-list emission.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use glam::Vec2;
use slotmap::{SecondaryMap, SlotMap};

use crate::behavior::{
    DragPayload, DraggableValue, Focusable, Interactable, Scrollable, TextEditable, UiAction,
};
use crate::draw::{TextAlign, UiDrawCommand, UiDrawList, UiTextureId};
use crate::layout;
use crate::style::{Color, Edges, InteractionState, LayoutStyle, Rect, VisualBox};

slotmap::new_key_type! {
    /// Generational arena handle to a node in [`UiTree`].
    pub struct NodeId;
}

/// Stable semantic identifier for frame-to-frame state retention and MCP UI automation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct NodeKey(pub u64);

struct Fnv1a64(u64);

impl Default for Fnv1a64 {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for Fnv1a64 {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

impl NodeKey {
    /// Stable key for the single popup layer node.
    pub fn for_overlay_layer(self) -> Self {
        NodeKey(0x0f1a_c3d5_e7b9_2468)
    }
}

pub trait IntoNodeKey {
    fn into_node_key(self) -> NodeKey;
}

impl IntoNodeKey for NodeKey {
    fn into_node_key(self) -> NodeKey {
        self
    }
}

impl IntoNodeKey for u64 {
    fn into_node_key(self) -> NodeKey {
        NodeKey(self)
    }
}

impl<T: Hash + ?Sized> IntoNodeKey for &T {
    fn into_node_key(self) -> NodeKey {
        let mut h = Fnv1a64::default();
        self.hash(&mut h);
        NodeKey(h.finish())
    }
}

impl<A: Hash, B: Hash> IntoNodeKey for (A, B) {
    fn into_node_key(self) -> NodeKey {
        let mut h = Fnv1a64::default();
        self.hash(&mut h);
        NodeKey(h.finish())
    }
}

impl<A: Hash, B: Hash, C: Hash> IntoNodeKey for (A, B, C) {
    fn into_node_key(self) -> NodeKey {
        let mut h = Fnv1a64::default();
        self.hash(&mut h);
        NodeKey(h.finish())
    }
}

impl<A: Hash, B: Hash, C: Hash, D: Hash> IntoNodeKey for (A, B, C, D) {
    fn into_node_key(self) -> NodeKey {
        let mut h = Fnv1a64::default();
        self.hash(&mut h);
        NodeKey(h.finish())
    }
}

/// Visual payload carried by a node.
#[derive(Clone, Debug, Default)]
pub enum NodeContent {
    #[default]
    Empty,
    Text {
        text: String,
        font_size: f32,
        color: Color,
        align: TextAlign,
        wrap: bool,
    },
    Image {
        texture: UiTextureId,
        tint: Color,
    },
    Custom(Vec<UiDrawCommand>),
}

#[derive(Clone, Debug)]
pub struct UiNode {
    pub key: NodeKey,
    pub debug_label: Option<String>,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub layout: LayoutStyle,
    pub intrinsic_size: Vec2,
    pub computed_rect: Rect,
    pub state: InteractionState,
    pub visual: Option<VisualBox>,
    pub content: NodeContent,
    pub is_overlay: bool,
    /// Outer spacing that reserves room around the node in its parent's flow.
    pub margin: Edges,
}

impl UiNode {
    fn new(key: NodeKey) -> Self {
        Self {
            key,
            debug_label: None,
            parent: None,
            children: Vec::new(),
            layout: LayoutStyle::default(),
            intrinsic_size: Vec2::ZERO,
            computed_rect: Rect::default(),
            state: InteractionState::default(),
            visual: None,
            content: NodeContent::Empty,
            is_overlay: false,
            margin: Edges::default(),
        }
    }
}

/// Where a popup sits relative to its anchor node.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PopupAlign {
    /// Align the popup's left edge with the anchor's left edge.
    #[default]
    Start,
    /// Align the popup's right edge with the anchor's right edge.
    End,
}

/// Which edge of the anchor a popup hangs off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PopupSide {
    /// Below the anchor: menu bar triggers open their dropdown this way.
    #[default]
    Below,
    /// To the right of the anchor: nested menus open beside their parent row.
    Right,
}

/// Positions a popup (dropdown, tooltip, context menu) against another node.
///
/// Anchors are resolved after the normal layout pass, because the anchor's
/// rect is only known once its own siblings have been arranged.
#[derive(Clone, Copy, Debug)]
pub struct Anchor {
    pub target: NodeKey,
    /// Offset from the resolved corner of the target rect.
    pub offset: Vec2,
    pub align: PopupAlign,
    pub side: PopupSide,
}

/// Semantic cursor shape requested by a UI widget or layout handle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CursorHint {
    #[default]
    Default,
    Pointer,
    Text,
    ResizeHorizontal,
    ResizeVertical,
    ResizeNwSe,
    ResizeNeSw,
    Grab,
    Grabbing,
}

/// Per-frame platform input snapshot fed into [`UiTree::process_input`].
#[derive(Clone, Debug, Default)]
pub struct UiInput {
    pub pointer_pos: Vec2,
    pub primary_down: bool,
    pub primary_just_pressed: bool,
    pub primary_just_released: bool,
    pub secondary_just_pressed: bool,
    pub scroll_delta: Vec2,
    pub shift_held: bool,
    pub ctrl_held: bool,
    pub text_committed: String,
    pub backspace_pressed: bool,
    pub escape_pressed: bool,
    pub time_seconds: f64,
}

/// Retained-mode UI tree storing nodes in a `SlotMap` and orthogonal behaviors in `SecondaryMap`s.
pub struct UiTree {
    pub(crate) nodes: SlotMap<NodeId, UiNode>,
    pub(crate) root: Option<NodeId>,
    pub(crate) key_to_id: HashMap<NodeKey, NodeId>,
    // Orthogonal behavior tables (zero Rc<RefCell<T>>, zero inheritance)
    pub interactables: SecondaryMap<NodeId, Interactable>,
    pub focusables: SecondaryMap<NodeId, Focusable>,
    pub draggables: SecondaryMap<NodeId, DraggableValue>,
    pub scrollables: SecondaryMap<NodeId, Scrollable>,
    pub text_editables: SecondaryMap<NodeId, TextEditable>,
    pub drag_sources: SecondaryMap<NodeId, DragPayload>,
    pub drop_targets: SecondaryMap<NodeId, bool>,
    pub anchors: SecondaryMap<NodeId, Anchor>,
    /// Nodes that react to hover without accepting clicks (submenu rows,
    /// tooltip anchors). They must be hit-testable or the pointer never
    /// enters them.
    pub hover_targets: SecondaryMap<NodeId, ()>,
    /// Root of the popup layer: drawn last and hit-tested first, so dropdowns
    /// and tooltips are never covered by the panels they belong to.
    overlay_root: Option<NodeId>,
    // Retained cross-frame interaction state
    hovered_key: Option<NodeKey>,
    active_pointer_key: Option<NodeKey>,
    focused_key: Option<NodeKey>,
    active_drag: Option<DragPayload>,
    retained_scroll: HashMap<NodeKey, Vec2>,
    retained_text: HashMap<NodeKey, TextEditable>,
    retained_drag: HashMap<NodeKey, (Option<f64>, f32)>,
    actions: Vec<UiAction>,
    /// Fallback drop-shadow applied to text nodes without an explicit per-node shadow.
    pub default_text_shadow: Option<(Vec2, Color)>,
    #[cfg(feature = "ui-tween")]
    pub tweens: crate::tween::TweenStore,
}

impl Default for UiTree {
    fn default() -> Self {
        Self::new()
    }
}

impl UiTree {
    pub fn new() -> Self {
        Self {
            nodes: SlotMap::with_key(),
            root: None,
            key_to_id: HashMap::new(),
            interactables: SecondaryMap::new(),
            focusables: SecondaryMap::new(),
            draggables: SecondaryMap::new(),
            scrollables: SecondaryMap::new(),
            text_editables: SecondaryMap::new(),
            drag_sources: SecondaryMap::new(),
            drop_targets: SecondaryMap::new(),
            anchors: SecondaryMap::new(),
            hover_targets: SecondaryMap::new(),
            overlay_root: None,
            hovered_key: None,
            active_pointer_key: None,
            focused_key: None,
            active_drag: None,
            retained_scroll: HashMap::new(),
            retained_text: HashMap::new(),
            retained_drag: HashMap::new(),
            actions: Vec::new(),
            default_text_shadow: None,
            #[cfg(feature = "ui-tween")]
            tweens: crate::tween::TweenStore::default(),
        }
    }

    /// Clear node storage for the new frame while preserving cross-frame interaction/scroll/text/tween state.
    pub fn begin_frame(&mut self) {
        for (id, scroll) in &self.scrollables {
            if let Some(node) = self.nodes.get(id) {
                self.retained_scroll.insert(node.key, scroll.offset);
            }
        }
        for (id, text) in &self.text_editables {
            if let Some(node) = self.nodes.get(id) {
                self.retained_text.insert(node.key, text.clone());
            }
        }
        self.retained_drag.clear();
        for (id, drag) in &self.draggables {
            if drag.drag_start_value.is_some() {
                if let Some(node) = self.nodes.get(id) {
                    self.retained_drag
                        .insert(node.key, (drag.drag_start_value, drag.drag_start_x));
                }
            }
        }
        self.nodes.clear();
        self.root = None;
        self.overlay_root = None;
        self.key_to_id.clear();
        self.interactables.clear();
        self.focusables.clear();
        self.draggables.clear();
        self.scrollables.clear();
        self.text_editables.clear();
        self.drag_sources.clear();
        self.drop_targets.clear();
        self.anchors.clear();
        self.hover_targets.clear();
        self.actions.clear();
    }

    /// Root of the popup layer, created on first use.
    ///
    /// The popup layer is a full-viewport, non-interactive container that
    /// normally sits last in draw order and first in hit-test order.
    pub fn overlay_root(&mut self, viewport: Vec2) -> NodeId {
        if let Some(id) = self.overlay_root {
            return id;
        }
        let id = self.spawn_node(NodeKey::default().for_overlay_layer(), None);
        if let Some(node) = self.nodes.get_mut(id) {
            node.layout.position = crate::style::PositionType::Absolute;
            node.layout.left = crate::style::Length::Px(0.0);
            node.layout.top = crate::style::Length::Px(0.0);
            node.layout.width = crate::style::Length::Px(viewport.x);
            node.layout.height = crate::style::Length::Px(viewport.y);
            // Popups size to their content. The default cross-axis alignment
            // stretches children to the container, which would make every
            // dropdown as tall as the window.
            node.layout.align_items = crate::style::Align::Start;
            node.layout.justify_content = crate::style::Justify::Start;
            node.is_overlay = true;
        }
        self.overlay_root = Some(id);
        id
    }

    /// The popup layer node, if anything requested one this frame.
    pub fn overlay_layer(&self) -> Option<NodeId> {
        self.overlay_root
    }

    /// Whether `key` is under the pointer.
    ///
    /// Builders call this while mounting, before layout runs, so it reflects the
    /// previous frame's hit test. That is the correct input for hover-open
    /// behaviour: the pointer has not moved since the hit test that produced the
    /// current hover, and the current frame's rects do not exist yet.
    pub fn is_hovered(&self, key: NodeKey) -> bool {
        self.hovered_key == Some(key)
    }

    /// The key currently under the pointer, if any.
    pub fn hovered(&self) -> Option<NodeKey> {
        self.hovered_key
    }

    /// Keys from `key` up to the root, innermost first.
    ///
    /// Popups are mounted as siblings on the popup layer, so this walks the node
    /// tree rather than the popup chain; callers that need the popup nesting
    /// derive it from the row keys they find on the path.
    pub fn ancestor_keys(&self, key: NodeKey) -> Option<Vec<NodeKey>> {
        let mut out = Vec::new();
        let mut cursor = self.key_to_id.get(&key).copied();
        while let Some(id) = cursor {
            let node = self.nodes.get(id)?;
            out.push(node.key);
            cursor = node.parent;
        }
        Some(out)
    }

    /// Whether the pointer is on `key` or anywhere inside the subtree it roots.
    ///
    /// A menu row that spawns a panel stays "entered" while the pointer is on any
    /// item of that panel, which is what keeps an open branch open.
    pub fn is_hovered_within(&self, key: NodeKey) -> bool {
        let Some(hovered) = self.hovered_key else {
            return false;
        };
        let Some(root) = self.key_to_id.get(&key).copied() else {
            return false;
        };
        let Some(target) = self.key_to_id.get(&hovered).copied() else {
            return false;
        };
        let mut cursor = Some(target);
        while let Some(id) = cursor {
            if id == root {
                return true;
            }
            cursor = self.nodes.get(id).and_then(|node| node.parent);
        }
        false
    }

    /// Resolve every anchored node's rect against its target.
    ///
    /// Runs after layout so the target rect is final; a missing target leaves
    /// the node at its arranged position rather than teleporting it.
    pub fn resolve_anchors(&mut self, viewport: Vec2) {
        let entries: Vec<(NodeId, Anchor)> = self
            .anchors
            .iter()
            .map(|(id, anchor)| (id, *anchor))
            .collect();
        // Resolve in dependency order. Popup panels are siblings on the popup
        // layer, so tree depth cannot order them; what matters is that a panel
        // anchored to a row *inside another panel* is resolved after that other
        // panel. Sorting by the anchor target's depth is not enough — measure the
        // dependency chain explicitly.
        let mut entries = entries;
        let mut ordered: Vec<(NodeId, Anchor, usize)> = entries
            .drain(..)
            .map(|(id, anchor)| {
                let level = self.anchor_chain_len(&anchor);
                (id, anchor, level)
            })
            .collect();
        ordered.sort_by_key(|(_, _, level)| *level);
        let entries: Vec<(NodeId, Anchor)> = ordered
            .into_iter()
            .map(|(id, anchor, _)| (id, anchor))
            .collect();
        for (id, anchor) in entries {
            let Some(target) = self.key_to_id.get(&anchor.target).copied() else {
                continue;
            };
            let Some(target_rect) = self.nodes.get(target).map(|n| n.computed_rect) else {
                continue;
            };
            let Some(own) = self.nodes.get(id).map(|n| n.computed_rect) else {
                continue;
            };
            let (mut x, mut y) = match anchor.side {
                PopupSide::Below => {
                    let x = match anchor.align {
                        PopupAlign::Start => target_rect.x,
                        PopupAlign::End => target_rect.x + target_rect.w - own.w,
                    };
                    (x, target_rect.y + target_rect.h)
                }
                PopupSide::Right => (
                    target_rect.x + target_rect.w,
                    // Align the panel's first row with the row that opened it,
                    // which is what makes a chain of menus readable.
                    match anchor.align {
                        PopupAlign::Start => target_rect.y,
                        PopupAlign::End => target_rect.y + target_rect.h - own.h,
                    },
                ),
            };
            x += anchor.offset.x;
            y += anchor.offset.y;
            // Flip a side panel that would leave the viewport: a nested menu
            // opening off the right edge is unreachable, so open it leftwards
            // instead of clamping it over its parent.
            if anchor.side == PopupSide::Right && x + own.w > viewport.x {
                x = target_rect.x - own.w + anchor.offset.x;
            }
            // Keep the popup on screen; a dropdown that runs off an edge is
            // unusable, so clamp rather than clip it.
            let x = x.clamp(0.0, (viewport.x - own.w).max(0.0));
            let y = y.clamp(0.0, (viewport.y - own.h).max(0.0));
            // Translate the whole subtree: children were arranged relative to the
            // node's provisional position, so moving only the parent would leave
            // the items behind.
            self.translate_subtree(id, x - own.x, y - own.y);
        }
    }

    /// How many other anchored nodes `anchor` depends on, transitively.
    ///
    /// A panel anchored to a row that is itself inside an anchored panel depends
    /// on that outer panel; resolving the outer one first is what makes the
    /// inner panel measure a row that has already been moved.
    fn anchor_chain_len(&self, anchor: &Anchor) -> usize {
        let mut level = 0;
        let mut target = self.key_to_id.get(&anchor.target).copied();
        let mut guard = 0;
        while let Some(id) = target {
            guard += 1;
            if guard > 64 {
                break;
            }
            // Walk up until we find an ancestor that is itself anchored.
            let mut cursor = self.nodes.get(id).and_then(|node| node.parent);
            let mut found = None;
            while let Some(parent) = cursor {
                if self.anchors.contains_key(parent) {
                    found = Some(parent);
                    break;
                }
                cursor = self.nodes.get(parent).and_then(|node| node.parent);
            }
            let Some(outer) = found else {
                break;
            };
            level += 1;
            // Continue from the outer panel's own anchor target.
            target = self
                .anchors
                .get(outer)
                .and_then(|a| self.key_to_id.get(&a.target).copied());
        }
        level
    }

    /// Offset a node and every descendant by `(dx, dy)`.
    fn translate_subtree(&mut self, id: NodeId, dx: f32, dy: f32) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        let children: Vec<NodeId> = match self.nodes.get(id) {
            Some(node) => node.children.clone(),
            None => return,
        };
        if let Some(node) = self.nodes.get_mut(id) {
            node.computed_rect.x += dx;
            node.computed_rect.y += dy;
        }
        for child in children {
            self.translate_subtree(child, dx, dy);
        }
    }

    pub fn spawn_node(&mut self, key: NodeKey, parent: Option<NodeId>) -> NodeId {
        let mut node = UiNode::new(key);
        node.parent = parent;
        node.state.hovered = self.hovered_key == Some(key);
        node.state.pressed = self.active_pointer_key == Some(key);
        node.state.focused = self.focused_key == Some(key);
        let id = self.nodes.insert(node);
        self.key_to_id.insert(key, id);
        if let Some(pid) = parent {
            if let Some(p) = self.nodes.get_mut(pid) {
                p.children.push(id);
            }
        } else if self.root.is_none() {
            self.root = Some(id);
        }
        id
    }

    pub fn node(&self, id: NodeId) -> Option<&UiNode> {
        self.nodes.get(id)
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut UiNode> {
        self.nodes.get_mut(id)
    }

    pub fn node_by_key(&self, key: NodeKey) -> Option<NodeId> {
        self.key_to_id.get(&key).copied()
    }

    /// Retained scroll offset for `key` (zero when it never scrolled).
    ///
    /// Virtualized builders need it *before* mounting: the visible item range is
    /// derived from the offset, while the offset itself is only updated by the
    /// wheel during `process_input`. Reading it back here is what makes a wheel
    /// scroll actually move the list instead of only moving a hidden counter.
    pub fn scroll_offset(&self, key: NodeKey) -> Vec2 {
        self.retained_scroll
            .get(&key)
            .copied()
            .unwrap_or(Vec2::ZERO)
    }

    pub fn restore_scroll(&mut self, id: NodeId, mut scroll: Scrollable) {
        if let Some(node) = self.nodes.get(id) {
            if let Some(&saved) = self.retained_scroll.get(&node.key) {
                scroll.offset = saved;
            }
        }
        self.scrollables.insert(id, scroll);
    }

    pub fn restore_text(&mut self, id: NodeId, fallback: TextEditable) {
        let state = if let Some(node) = self.nodes.get(id) {
            self.retained_text
                .get(&node.key)
                .cloned()
                .unwrap_or(fallback)
        } else {
            fallback
        };
        self.text_editables.insert(id, state);
    }

    pub fn restore_draggable(&mut self, id: NodeId, mut drag: DraggableValue) {
        if let Some(node) = self.nodes.get(id) {
            if let Some(&(start_val, start_x)) = self.retained_drag.get(&node.key) {
                drag.drag_start_value = start_val;
                drag.drag_start_x = start_x;
            }
        }
        self.draggables.insert(id, drag);
    }

    pub fn compute_layout(&mut self, viewport_size: Vec2) {
        if let Some(root) = self.root {
            layout::compute_layout(&mut self.nodes, root, viewport_size);
        }
        if let Some(overlay) = self.overlay_root {
            layout::compute_layout(&mut self.nodes, overlay, viewport_size);
        }
        self.resolve_anchors(viewport_size);
        for (id, scroll) in &mut self.scrollables {
            if let Some(node) = self.nodes.get(id) {
                scroll.clamp(Vec2::new(node.computed_rect.w, node.computed_rect.h));
            }
        }
    }

    pub fn step_animations(&mut self, dt: f32) -> bool {
        #[cfg(feature = "ui-tween")]
        {
            self.tweens.step(dt)
        }
        #[cfg(not(feature = "ui-tween"))]
        {
            let _ = dt;
            false
        }
    }

    /// Hit-test from front to back (overlays first, reverse child order).
    pub fn hit_test(&self, pos: Vec2) -> Option<NodeId> {
        // The popup layer wins over everything below it, mirroring draw order.
        if let Some(overlay) = self.overlay_root {
            if let Some(hit) = self.hit_test_node(overlay, pos, None) {
                return Some(hit);
            }
        }
        let root = self.root?;
        self.hit_test_node(root, pos, None)
    }

    fn hit_test_node(&self, id: NodeId, pos: Vec2, clip: Option<Rect>) -> Option<NodeId> {
        let node = self.nodes.get(id)?;
        let effective_clip = match (clip, node.layout.clip_children) {
            (Some(c), true) => Some(c.intersect(node.computed_rect)),
            (Some(c), false) => Some(c),
            (None, true) => Some(node.computed_rect),
            (None, false) => None,
        };
        if let Some(c) = effective_clip {
            if !c.contains(pos) {
                return None;
            }
        }
        // Overlays first, then normal children in reverse draw order.
        for &cid in node.children.iter().rev() {
            if self.nodes.get(cid).is_some_and(|n| n.is_overlay) {
                if let Some(hit) = self.hit_test_node(cid, pos, effective_clip) {
                    return Some(hit);
                }
            }
        }
        for &cid in node.children.iter().rev() {
            if self.nodes.get(cid).is_some_and(|n| !n.is_overlay) {
                if let Some(hit) = self.hit_test_node(cid, pos, effective_clip) {
                    return Some(hit);
                }
            }
        }
        if node.computed_rect.contains(pos) && (self.is_interactive(id) || self.blocks_pointer(id))
        {
            Some(id)
        } else {
            None
        }
    }

    fn blocks_pointer(&self, id: NodeId) -> bool {
        if self.root == Some(id) || self.overlay_root == Some(id) {
            return false;
        }
        let Some(node) = self.nodes.get(id) else {
            return false;
        };
        node.visual.as_ref().is_some_and(|v| {
            let s = v.resolve(&node.state);
            s.bg.is_some_and(|c| c.a > 0.0) || s.material.is_some()
        })
    }

    /// Cursor shape requested by the currently hovered or actively dragged node.
    pub fn desired_cursor(&self) -> CursorHint {
        if let Some(active_key) = self.active_pointer_key {
            if let Some(id) = self.key_to_id.get(&active_key).copied() {
                if self.draggables.contains_key(id) {
                    return CursorHint::ResizeHorizontal;
                }
            }
        }
        if let Some(hovered_key) = self.hovered_key {
            if let Some(id) = self.key_to_id.get(&hovered_key).copied() {
                if self.draggables.contains_key(id) {
                    return CursorHint::ResizeHorizontal;
                }
                if self.text_editables.contains_key(id) {
                    return CursorHint::Text;
                }
                if self.interactables.contains_key(id) {
                    return CursorHint::Pointer;
                }
            }
        }
        CursorHint::Default
    }

    fn is_interactive(&self, id: NodeId) -> bool {
        self.interactables.contains_key(id)
            || self.draggables.contains_key(id)
            || self.scrollables.contains_key(id)
            || self.text_editables.contains_key(id)
            || self.focusables.contains_key(id)
            || self.drop_targets.contains_key(id)
            || self.hover_targets.contains_key(id)
    }

    pub fn process_input(&mut self, input: &UiInput) -> Vec<UiAction> {
        self.actions.clear();
        let hit = self.hit_test(input.pointer_pos);
        self.hovered_key = hit.and_then(|id| self.nodes.get(id)).map(|n| n.key);

        if input.primary_just_pressed {
            self.active_pointer_key = self.hovered_key;
            if let Some(id) = hit {
                let key = self.nodes[id].key;
                if self.focusables.contains_key(id) || self.text_editables.contains_key(id) {
                    if self.focused_key != Some(key) {
                        self.focused_key = Some(key);
                        self.actions.push(UiAction::FocusChanged(Some(key)));
                    }
                } else if self.focused_key.is_some() {
                    self.focused_key = None;
                    self.actions.push(UiAction::FocusChanged(None));
                }
                if let Some(drag) = self.draggables.get_mut(id) {
                    drag.drag_start_value = Some(drag.value);
                    drag.drag_start_x = input.pointer_pos.x;
                }
                if let Some(payload) = self.drag_sources.get(id) {
                    self.active_drag = Some(payload.clone());
                }
            } else if self.focused_key.is_some() {
                self.focused_key = None;
                self.actions.push(UiAction::FocusChanged(None));
            }
        }

        // Active pointer drag (e.g. DraggableValue scrub).
        if input.primary_down {
            if let Some(active_key) = self.active_pointer_key {
                if let Some(id) = self.key_to_id.get(&active_key).copied() {
                    if let Some(drag) = self.draggables.get_mut(id) {
                        let dx = input.pointer_pos.x - drag.drag_start_x;
                        let prev = drag.value;
                        drag.apply_delta(dx, input.shift_held, input.ctrl_held);
                        if (drag.value - prev).abs() > f64::EPSILON {
                            self.actions.push(UiAction::ValueChanged {
                                key: active_key,
                                value: drag.value,
                            });
                        }
                    }
                }
            }
        }

        if input.primary_just_released {
            if let Some(active_key) = self.active_pointer_key.take() {
                if let Some(id) = self.key_to_id.get(&active_key).copied() {
                    if let Some(drag) = self.draggables.get_mut(id) {
                        if let Some(old) = drag.drag_start_value.take() {
                            if (drag.value - old).abs() > f64::EPSILON {
                                self.actions.push(UiAction::ValueCommitted {
                                    key: active_key,
                                    old,
                                    new: drag.value,
                                });
                            }
                        }
                    }
                    if self.hovered_key == Some(active_key) {
                        if let Some(inter) = self.interactables.get_mut(id) {
                            let is_double = inter.emit_double_click
                                && (input.time_seconds - inter.last_click_time) < 0.35;
                            inter.last_click_time = input.time_seconds;
                            if is_double {
                                self.actions.push(UiAction::DoubleClicked(active_key));
                            } else {
                                if let Some(ref mut toggled) = inter.toggle_state {
                                    *toggled = !*toggled;
                                    self.actions.push(UiAction::Toggled {
                                        key: active_key,
                                        value: *toggled,
                                    });
                                }
                                self.actions.push(UiAction::Clicked(active_key));
                            }
                        }
                    }
                }
            }
            if let Some(payload) = self.active_drag.take() {
                if let Some(target_id) = hit {
                    if self.drop_targets.contains_key(target_id) {
                        let target = self.nodes[target_id].key;
                        self.actions.push(UiAction::Dropped { target, payload });
                    }
                }
            }
        }

        // Scroll routing: bubble from hit node up to nearest Scrollable ancestor.
        if input.scroll_delta.length_squared() > 0.0 {
            let mut cursor = hit;
            while let Some(cid) = cursor {
                if let Some(scroll) = self.scrollables.get_mut(cid) {
                    let node = &self.nodes[cid];
                    scroll.offset -= input.scroll_delta * 24.0;
                    scroll.clamp(Vec2::new(node.computed_rect.w, node.computed_rect.h));
                    self.actions.push(UiAction::Scrolled {
                        key: node.key,
                        offset: scroll.offset,
                    });
                    break;
                }
                cursor = self.nodes.get(cid).and_then(|n| n.parent);
            }
        }

        // Keyboard / text routing to focused node.
        if let Some(focused_key) = self.focused_key {
            if let Some(fid) = self.key_to_id.get(&focused_key).copied() {
                if let Some(edit) = self.text_editables.get_mut(fid) {
                    let mut changed = false;
                    if !input.text_committed.is_empty() {
                        edit.insert_str(&input.text_committed);
                        changed = true;
                    }
                    if input.backspace_pressed {
                        edit.backspace();
                        changed = true;
                    }
                    if changed {
                        self.actions.push(UiAction::TextChanged {
                            key: focused_key,
                            text: edit.text.clone(),
                        });
                    }
                    if input.escape_pressed {
                        self.focused_key = None;
                        self.actions.push(UiAction::FocusChanged(None));
                    }
                }
            }
        }

        // Refresh node interaction flags so draw list reflects latest state.
        for (id, node) in &mut self.nodes {
            node.state.hovered = self.hovered_key == Some(node.key);
            node.state.pressed = self.active_pointer_key == Some(node.key);
            node.state.focused = self.focused_key == Some(node.key);
            let _ = id;
        }

        std::mem::take(&mut self.actions)
    }

    pub fn build_draw_list(&self, out: &mut UiDrawList) {
        out.clear();
        if let Some(root) = self.root {
            self.emit_node_draw(root, out);
        }
        // Popups draw last so they sit above every panel.
        if let Some(overlay) = self.overlay_root {
            for &cid in &self.nodes[overlay].children {
                self.emit_node_draw(cid, out);
            }
        }
    }

    fn emit_node_draw(&self, id: NodeId, out: &mut UiDrawList) {
        let Some(node) = self.nodes.get(id) else {
            return;
        };
        let rect = node.computed_rect;
        if rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }

        if let Some(vbox) = &node.visual {
            let style = vbox.resolve(&node.state);
            #[cfg(feature = "ui-effects")]
            if let Some(shadow) = style.shadow {
                out.push(UiDrawCommand::DropShadow {
                    rect,
                    radius: style.radius,
                    color: shadow.color.with_alpha(shadow.color.a * style.opacity),
                    offset: shadow.offset,
                    blur: shadow.blur,
                });
            }
            let bg = style.bg.unwrap_or(Color::TRANSPARENT);
            let bc = style.border_color.unwrap_or(Color::TRANSPARENT);
            // A material owns the surface, so it draws even when the fill is
            // clear: a frosted panel with no flat colour still has to composite.
            if bg.a > 0.0 || (bc.a > 0.0 && style.border_width > 0.0) || style.material.is_some() {
                out.push(UiDrawCommand::Quad {
                    rect,
                    color: bg.with_alpha(bg.a * style.opacity),
                    border_color: bc.with_alpha(bc.a * style.opacity),
                    border_width: style.border_width,
                    radius: style.radius,
                    texture: None,
                    material: style.material.clone(),
                });
            }
        }

        match &node.content {
            NodeContent::Empty => {}
            NodeContent::Text {
                text,
                font_size,
                color,
                align,
                wrap,
            } => {
                let text_effect = node
                    .visual
                    .as_ref()
                    .map(|v| v.resolve(&node.state).text_effect)
                    .unwrap_or_default();
                let (shadow_offset, shadow_color) = if text_effect.shadow_color.a > 0.0 {
                    (text_effect.shadow_offset, text_effect.shadow_color)
                } else if let Some((offset, color)) = self.default_text_shadow {
                    (offset, color)
                } else {
                    (text_effect.shadow_offset, text_effect.shadow_color)
                };
                out.push(UiDrawCommand::Text {
                    rect,
                    content: text.clone(),
                    font_size: *font_size,
                    color: *color,
                    align: *align,
                    wrap: *wrap,
                    outline_width: text_effect.outline_width,
                    outline_color: text_effect.outline_color,
                    shadow_offset,
                    shadow_color,
                });
            }
            NodeContent::Image { texture, tint } => {
                out.push(UiDrawCommand::Quad {
                    rect,
                    color: *tint,
                    border_color: Color::TRANSPARENT,
                    border_width: 0.0,
                    radius: 0.0,
                    texture: Some(*texture),
                    material: node
                        .visual
                        .as_ref()
                        .and_then(|vbox| vbox.resolve(&node.state).material),
                });
            }
            NodeContent::Custom(cmds) => {
                for cmd in cmds {
                    out.push(cmd.clone());
                }
            }
        }

        if node.layout.clip_children {
            out.push(UiDrawCommand::PushClip(rect));
        }

        // Normal children first, overlays last.
        for &cid in &node.children {
            if self.nodes.get(cid).is_some_and(|n| !n.is_overlay) {
                self.emit_node_draw(cid, out);
            }
        }
        for &cid in &node.children {
            if self.nodes.get(cid).is_some_and(|n| n.is_overlay) {
                self.emit_node_draw(cid, out);
            }
        }

        if node.layout.clip_children {
            out.push(UiDrawCommand::PopClip);
        }
    }

    /// Expose semantic keys + computed rects for automated UI testing & `prism-editor-mcp`.
    pub fn snapshot_debug_nodes(&self) -> Vec<(NodeKey, Option<&str>, Rect, InteractionState)> {
        self.nodes
            .values()
            .map(|n| (n.key, n.debug_label.as_deref(), n.computed_rect, n.state))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elements::{button, col, numeric_field, ElementBox, UiContext};
    use crate::style::{Color, Length};

    #[test]
    fn overlay_surface_blocks_clicks_from_penetrating_underneath() {
        let mut tree = UiTree::new();
        tree.begin_frame();
        let root =
            {
                let mut ctx = UiContext::new(&mut tree);
                ctx.mount(col().size(Length::Px(400.0), Length::Px(300.0)).child(
                    button("under:btn", "Click Me").size(Length::Px(200.0), Length::Px(80.0)),
                ))
            };
        let overlay = tree.overlay_root(Vec2::new(400.0, 300.0));
        {
            let mut ctx = UiContext::new(&mut tree);
            ctx.with_parent(overlay, |ctx| {
                ctx.mount(
                    ElementBox::default()
                        .key("overlay:card")
                        .absolute(0.0, 0.0)
                        .size(Length::Px(250.0), Length::Px(120.0))
                        .bg(Color::rgba(0.1, 0.1, 0.1, 0.8)),
                );
            });
        }
        let _ = root;
        tree.compute_layout(Vec2::new(400.0, 300.0));

        let hit = tree.hit_test(Vec2::new(50.0, 30.0));
        assert_eq!(
            hit.and_then(|id| tree.nodes.get(id)).map(|n| n.key),
            Some("overlay:card".into_node_key()),
            "non-interactive overlay surface must block hit-testing underneath"
        );
    }

    #[test]
    fn desired_cursor_reflects_hovered_draggable_and_button() {
        let mut tree = UiTree::new();
        tree.begin_frame();
        {
            let mut ctx = UiContext::new(&mut tree);
            ctx.mount(
                col()
                    .size(Length::Px(400.0), Length::Px(300.0))
                    .child(numeric_field("num:x", 1.0, -10.0, 10.0, 0.1))
                    .child(button("btn:ok", "OK").size(Length::Px(100.0), Length::Px(30.0))),
            );
        }
        tree.compute_layout(Vec2::new(400.0, 300.0));

        tree.process_input(&UiInput {
            pointer_pos: Vec2::new(20.0, 12.0),
            ..Default::default()
        });
        assert_eq!(tree.desired_cursor(), CursorHint::ResizeHorizontal);

        tree.process_input(&UiInput {
            pointer_pos: Vec2::new(20.0, 35.0),
            ..Default::default()
        });
        assert_eq!(tree.desired_cursor(), CursorHint::Pointer);
    }
}
