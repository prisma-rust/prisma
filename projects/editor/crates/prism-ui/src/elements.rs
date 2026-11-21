//! Modern declarative builder & composition API for `prism-ui`.
//!
//! Every high-level widget (`button`, `toggle_button`, `numeric_field`, `slider`,
//! `vec3_field`, `virtual_list`, `tree_view`) is composed from `ElementBox` plus
//! orthogonal behavior tables — zero inheritance and zero duplicated state machines.

use glam::Vec2;

use crate::behavior::{
    DragPayload, DraggableValue, Focusable, Interactable, Scrollable, TextEditable,
};
use crate::draw::{TextAlign, UiDrawCommand, UiTextureId};
use crate::style::{
    Align, Color, Edges, FlexDirection, Justify, LayoutStyle, Length, PositionType, VisualBox,
    VisualPatch,
};
use crate::tree::{
    Anchor, IntoNodeKey, NodeContent, NodeId, NodeKey, PopupAlign, UiTree,
};

/// Default dark-mode palette used by built-in composed widgets (overridable via modifiers).
pub struct UiPalette;

impl UiPalette {
    pub const SURFACE_0: Color = Color::rgb(0.11, 0.12, 0.14);
    pub const SURFACE_1: Color = Color::rgb(0.15, 0.16, 0.19);
    pub const SURFACE_2: Color = Color::rgb(0.20, 0.22, 0.26);
    pub const SURFACE_HOVER: Color = Color::rgb(0.25, 0.28, 0.33);
    pub const SURFACE_PRESS: Color = Color::rgb(0.17, 0.19, 0.23);
    pub const ACCENT: Color = Color::rgb(0.24, 0.52, 0.96);
    pub const BORDER: Color = Color::rgb(0.27, 0.29, 0.34);
    pub const TEXT: Color = Color::rgb(0.92, 0.93, 0.95);
    pub const TEXT_MUTED: Color = Color::rgb(0.62, 0.65, 0.70);
}

/// Mounting context passed to element builders.
pub struct UiContext<'a> {
    pub tree: &'a mut UiTree,
    parent: Option<NodeId>,
    auto_seq: u64,
}

impl<'a> UiContext<'a> {
    pub fn new(tree: &'a mut UiTree) -> Self {
        Self {
            tree,
            parent: None,
            auto_seq: 1,
        }
    }

    fn next_auto_key(&mut self) -> NodeKey {
        let parent_bits = self
            .parent
            .and_then(|id| self.tree.node(id))
            .map(|n| n.key.0)
            .unwrap_or(0x9e37_79b9_7f4a_7c15);
        let seq = self.auto_seq;
        self.auto_seq += 1;
        NodeKey(parent_bits.rotate_left(17) ^ seq.wrapping_mul(0x517c_c1b7_2722_0a95))
    }

    pub fn mount<E: UiElement>(&mut self, element: E) -> NodeId {
        element.build(self)
    }

    pub fn with_parent<F>(&mut self, parent: NodeId, f: F)
    where
        F: FnOnce(&mut UiContext<'_>),
    {
        let prev = self.parent;
        let prev_seq = self.auto_seq;
        self.parent = Some(parent);
        self.auto_seq = 1;
        f(self);
        self.parent = prev;
        self.auto_seq = prev_seq;
    }
}

pub trait UiElement {
    fn build(self, ctx: &mut UiContext<'_>) -> NodeId;
}

type ChildBuilder = Box<dyn FnOnce(&mut UiContext<'_>)>;

/// Fluent, composable UI node builder with orthogonal modifier chain.
pub struct ElementBox {
    key: Option<NodeKey>,
    debug_label: Option<String>,
    layout: LayoutStyle,
    visual: Option<VisualBox>,
    content: NodeContent,
    is_overlay: bool,
    interactable: Option<Interactable>,
    focusable: Option<Focusable>,
    draggable_value: Option<DraggableValue>,
    scrollable: Option<Scrollable>,
    text_editable: Option<TextEditable>,
    drag_source: Option<DragPayload>,
    drop_target: bool,
    margin: Edges,
    anchor: Option<Anchor>,
    children: Vec<ChildBuilder>,
}

impl Default for ElementBox {
    fn default() -> Self {
        Self {
            key: None,
            debug_label: None,
            layout: LayoutStyle::default(),
            visual: None,
            content: NodeContent::Empty,
            is_overlay: false,
            interactable: None,
            focusable: None,
            draggable_value: None,
            scrollable: None,
            text_editable: None,
            drag_source: None,
            drop_target: false,
            margin: Edges::default(),
            anchor: None,
            children: Vec::new(),
        }
    }
}

impl ElementBox {
    pub fn key(mut self, key: impl IntoNodeKey) -> Self {
        self.key = Some(key.into_node_key());
        self
    }

    pub fn debug_label(mut self, label: impl Into<String>) -> Self {
        self.debug_label = Some(label.into());
        self
    }

    pub fn direction(mut self, dir: FlexDirection) -> Self {
        self.layout.direction = dir;
        self
    }

    pub fn absolute(mut self, left: f32, top: f32) -> Self {
        self.layout.position = PositionType::Absolute;
        self.layout.left = Length::Px(left);
        self.layout.top = Length::Px(top);
        self
    }

    pub fn overlay(mut self) -> Self {
        self.is_overlay = true;
        self
    }

    pub fn width(mut self, w: Length) -> Self {
        self.layout.width = w;
        self
    }

    pub fn height(mut self, h: Length) -> Self {
        self.layout.height = h;
        self
    }

    pub fn size(mut self, w: Length, h: Length) -> Self {
        self.layout.width = w;
        self.layout.height = h;
        self
    }

    pub fn grow(mut self, factor: f32) -> Self {
        self.layout.width = Length::Grow(factor);
        self.layout.height = Length::Grow(factor);
        self
    }

    pub fn padding(mut self, p: Edges) -> Self {
        self.layout.padding = p;
        self
    }

    pub fn pad(mut self, all: f32) -> Self {
        self.layout.padding = Edges::all(all);
        self
    }

    pub fn gap(mut self, gap: f32) -> Self {
        self.layout.gap = gap;
        self
    }

    pub fn align(mut self, align: Align) -> Self {
        self.layout.align_items = align;
        self
    }

    pub fn justify(mut self, justify: Justify) -> Self {
        self.layout.justify_content = justify;
        self
    }

    pub fn clip(mut self) -> Self {
        self.layout.clip_children = true;
        self
    }

    fn ensure_visual(&mut self) -> &mut VisualBox {
        self.visual.get_or_insert_with(VisualBox::default)
    }

    pub fn bg(mut self, color: Color) -> Self {
        self.ensure_visual().base.bg = Some(color);
        self
    }

    pub fn hover_bg(mut self, color: Color) -> Self {
        let v = self.ensure_visual();
        v.hover.get_or_insert_with(VisualPatch::default).bg = Some(color);
        self
    }

    pub fn pressed_bg(mut self, color: Color) -> Self {
        let v = self.ensure_visual();
        v.pressed.get_or_insert_with(VisualPatch::default).bg = Some(color);
        self
    }

    pub fn border(mut self, width: f32, color: Color) -> Self {
        let v = self.ensure_visual();
        v.base.border_width = width;
        v.base.border_color = Some(color);
        self
    }

    pub fn focus_border(mut self, color: Color) -> Self {
        let v = self.ensure_visual();
        let f = v.focused.get_or_insert_with(VisualPatch::default);
        f.border_color = Some(color);
        f.border_width = Some(1.5);
        self
    }

    pub fn radius(mut self, radius: f32) -> Self {
        self.ensure_visual().base.radius = radius;
        self
    }

    #[cfg(feature = "ui-blur")]
    pub fn backdrop_blur(mut self, sigma: f32) -> Self {
        self.ensure_visual().base.backdrop_blur = sigma;
        self
    }

    pub fn clickable(mut self) -> Self {
        self.interactable.get_or_insert_with(Interactable::default);
        self
    }

    pub fn toggleable(mut self, active: bool) -> Self {
        let inter = self.interactable.get_or_insert_with(Interactable::default);
        inter.toggle_state = Some(active);
        self
    }

    pub fn double_clickable(mut self) -> Self {
        let inter = self.interactable.get_or_insert_with(Interactable::default);
        inter.emit_double_click = true;
        self
    }

    pub fn focusable(mut self, tab_index: i32) -> Self {
        self.focusable = Some(Focusable {
            tab_index,
            captures_text_keys: false,
        });
        self
    }

    pub fn draggable_value(mut self, dv: DraggableValue) -> Self {
        self.draggable_value = Some(dv);
        self
    }

    pub fn scrollable(mut self, vertical: bool, horizontal: bool, content_size: Vec2) -> Self {
        self.layout.clip_children = true;
        self.scrollable = Some(Scrollable {
            offset: Vec2::ZERO,
            content_size,
            horizontal,
            vertical,
        });
        self
    }

    pub fn text_editable(
        mut self,
        text: impl Into<String>,
        placeholder: impl Into<String>,
    ) -> Self {
        self.focusable = Some(Focusable {
            tab_index: 0,
            captures_text_keys: true,
        });
        self.text_editable = Some(TextEditable {
            text: text.into(),
            placeholder: placeholder.into(),
            ..Default::default()
        });
        self
    }

    pub fn drag_source(mut self, payload: DragPayload) -> Self {
        self.drag_source = Some(payload);
        self
    }

    pub fn drop_target(mut self) -> Self {
        self.drop_target = true;
        self
    }

    /// Minimum width, letting the node grow past it with its content.
    pub fn min_width(mut self, min_w: f32) -> Self {
        self.layout.min_width = min_w;
        self
    }

    /// Vertical margin rendered as extra space above and below the node.
    pub fn margin_vertical(mut self, margin: f32) -> Self {
        self.margin = Edges::symmetric(0.0, margin);
        self
    }

    /// Right-align this node's text content.
    pub fn align_text_right(mut self) -> Self {
        if let NodeContent::Text { align, .. } = &mut self.content {
            *align = TextAlign::Right;
        }
        self
    }

    /// Anchor this node to another node's rect instead of its layout position.
    pub fn anchored(mut self, target: NodeKey, offset: Vec2, align: PopupAlign) -> Self {
        self.anchor = Some(Anchor {
            target,
            offset,
            align,
        });
        self
    }

    pub fn child<E: UiElement + 'static>(mut self, child: E) -> Self {
        self.children.push(Box::new(move |ctx| {
            ctx.mount(child);
        }));
        self
    }

    pub fn children_fn<F: FnOnce(&mut UiContext<'_>) + 'static>(mut self, f: F) -> Self {
        self.children.push(Box::new(f));
        self
    }
}

impl UiElement for ElementBox {
    fn build(self, ctx: &mut UiContext<'_>) -> NodeId {
        let key = self.key.unwrap_or_else(|| ctx.next_auto_key());
        let id = ctx.tree.spawn_node(key, ctx.parent);
        if let Some(node) = ctx.tree.node_mut(id) {
            node.debug_label = self.debug_label;
            node.layout = self.layout;
            node.margin = self.margin;
            node.visual = self.visual;
            node.content = self.content;
            node.is_overlay = self.is_overlay;
        }
        if let Some(inter) = self.interactable {
            ctx.tree.interactables.insert(id, inter);
        }
        if let Some(foc) = self.focusable {
            ctx.tree.focusables.insert(id, foc);
        }
        if let Some(dv) = self.draggable_value {
            ctx.tree.restore_draggable(id, dv);
        }
        if let Some(sc) = self.scrollable {
            ctx.tree.restore_scroll(id, sc);
        }
        if let Some(te) = self.text_editable {
            ctx.tree.restore_text(id, te);
        }
        if let Some(ds) = self.drag_source {
            ctx.tree.drag_sources.insert(id, ds);
        }
        if self.drop_target {
            ctx.tree.drop_targets.insert(id, true);
        }
        if let Some(anchor) = self.anchor {
            ctx.tree.anchors.insert(id, anchor);
        }
        if !self.children.is_empty() {
            ctx.with_parent(id, |child_ctx| {
                for builder in self.children {
                    builder(child_ctx);
                }
            });
        }
        id
    }
}

pub fn row() -> ElementBox {
    ElementBox::default().direction(FlexDirection::Row)
}

pub fn col() -> ElementBox {
    ElementBox::default().direction(FlexDirection::Column)
}

pub fn spacer() -> ElementBox {
    ElementBox::default().grow(1.0)
}

pub fn divider(vertical: bool) -> ElementBox {
    if vertical {
        ElementBox::default()
            .width(Length::Px(1.0))
            .height(Length::Percent(100.0))
            .bg(UiPalette::BORDER)
    } else {
        ElementBox::default()
            .width(Length::Percent(100.0))
            .height(Length::Px(1.0))
            .bg(UiPalette::BORDER)
    }
}

/// A text run with explicit size and colour, for widgets that style their own
/// typography instead of using the built-in label presets.
pub fn text(color: Color, font_size: f32, content: impl Into<String>) -> ElementBox {
    ElementBox {
        content: NodeContent::Text {
            text: content.into(),
            font_size,
            color,
            align: TextAlign::Left,
            wrap: false,
        },
        ..Default::default()
    }
}

pub fn label(text: impl Into<String>) -> ElementBox {
    ElementBox {
        content: NodeContent::Text {
            text: text.into(),
            font_size: 13.0,
            color: UiPalette::TEXT,
            align: TextAlign::Left,
            wrap: false,
        },
        ..Default::default()
    }
}

pub fn muted_label(text: impl Into<String>) -> ElementBox {
    ElementBox {
        content: NodeContent::Text {
            text: text.into(),
            font_size: 12.0,
            color: UiPalette::TEXT_MUTED,
            align: TextAlign::Left,
            wrap: false,
        },
        ..Default::default()
    }
}

pub fn viewport(texture: UiTextureId) -> ElementBox {
    let mut b = ElementBox::default().grow(1.0);
    b.content = NodeContent::Image {
        texture,
        tint: Color::WHITE,
    };
    b
}

pub fn canvas(commands: Vec<UiDrawCommand>) -> ElementBox {
    ElementBox {
        content: NodeContent::Custom(commands),
        ..Default::default()
    }
}

fn button_core(key: impl IntoNodeKey, text: impl Into<String>, toggle: Option<bool>) -> ElementBox {
    let active = toggle.unwrap_or(false);
    let base_bg = if active {
        UiPalette::ACCENT
    } else {
        UiPalette::SURFACE_2
    };
    let mut b = row()
        .key(key)
        .padding(Edges::symmetric(10.0, 5.0))
        .align(Align::Center)
        .justify(Justify::Center)
        .bg(base_bg)
        .hover_bg(if active {
            UiPalette::ACCENT
        } else {
            UiPalette::SURFACE_HOVER
        })
        .pressed_bg(UiPalette::SURFACE_PRESS)
        .border(1.0, UiPalette::BORDER)
        .focus_border(UiPalette::ACCENT)
        .radius(4.0)
        .focusable(0)
        .child(label(text));
    b = match toggle {
        Some(v) => b.toggleable(v),
        None => b.clickable(),
    };
    b
}

pub fn button(key: impl IntoNodeKey, text: impl Into<String>) -> ElementBox {
    button_core(key, text, None)
}

pub fn toggle_button(key: impl IntoNodeKey, text: impl Into<String>, active: bool) -> ElementBox {
    button_core(key, text, Some(active))
}

pub fn text_field(
    key: impl IntoNodeKey,
    text: impl Into<String>,
    placeholder: impl Into<String>,
) -> ElementBox {
    let text_str = text.into();
    let ph_str = placeholder.into();
    let display = if text_str.is_empty() {
        ph_str.clone()
    } else {
        text_str.clone()
    };
    row()
        .key(key)
        .height(Length::Px(26.0))
        .padding(Edges::symmetric(8.0, 4.0))
        .align(Align::Center)
        .bg(UiPalette::SURFACE_0)
        .border(1.0, UiPalette::BORDER)
        .focus_border(UiPalette::ACCENT)
        .radius(4.0)
        .text_editable(text_str, ph_str)
        .child(label(display))
}

pub fn numeric_field(
    key: impl IntoNodeKey,
    value: f64,
    min: f64,
    max: f64,
    step: f64,
) -> ElementBox {
    row()
        .key(key)
        .min_width_px(64.0)
        .height(Length::Px(24.0))
        .padding(Edges::symmetric(6.0, 3.0))
        .align(Align::Center)
        .bg(UiPalette::SURFACE_0)
        .hover_bg(UiPalette::SURFACE_1)
        .border(1.0, UiPalette::BORDER)
        .focus_border(UiPalette::ACCENT)
        .radius(4.0)
        .focusable(0)
        .draggable_value(DraggableValue {
            value,
            min,
            max,
            step,
            ..Default::default()
        })
        .child(label(format!("{value:.2}")))
}

/// Composed `Vec3` editor built from 3 `numeric_field`s + axis badges.
pub fn vec3_field(key_prefix: &str, v: [f32; 3], step: f64) -> ElementBox {
    let axes = [
        ("X", Color::rgb(0.85, 0.30, 0.30), v[0]),
        ("Y", Color::rgb(0.32, 0.78, 0.36), v[1]),
        ("Z", Color::rgb(0.30, 0.56, 0.92), v[2]),
    ];
    let mut container = row().gap(4.0).align(Align::Center);
    for (idx, (axis, color, val)) in axes.into_iter().enumerate() {
        let mut badge = label(axis)
            .padding(Edges::symmetric(4.0, 2.0))
            .bg(color)
            .radius(3.0);
        badge.layout.height = Length::Px(24.0);
        container = container.child(badge).child(numeric_field(
            (key_prefix, idx as u32),
            val as f64,
            f64::NEG_INFINITY,
            f64::INFINITY,
            step,
        ));
    }
    container
}

/// Fixed-height virtualized list: only mounts visible items `[start..end)`.
///
/// The mounted range is derived from the list's **retained** scroll offset, so a
/// wheel gesture over the list scrolls it. `scroll_offset_y` is only the
/// first-frame seed (scroll-to-item requests, and tests that mount without a wheel);
/// once the list has scrolled, the retained offset wins.
pub fn virtual_list<F>(
    key: impl IntoNodeKey,
    item_count: usize,
    item_height: f32,
    viewport_height: f32,
    scroll_offset_y: f32,
    mut render_item: F,
) -> ElementBox
where
    F: FnMut(usize) -> ElementBox + 'static,
{
    let total_h = (item_count as f32) * item_height;
    let list_key = key.into_node_key();

    col()
        .key(list_key)
        .grow(1.0)
        .scrollable(true, false, Vec2::new(0.0, total_h))
        .children_fn(move |ctx| {
            let offset = ctx.tree.scroll_offset(list_key).y.max(scroll_offset_y);
            let first = (offset / item_height).floor().max(0.0) as usize;
            let visible = (viewport_height / item_height).ceil() as usize + 2;
            let start = first.min(item_count);
            let end = (start + visible).min(item_count);
            let top_pad = (start as f32) * item_height;
            if top_pad > 0.0 {
                ctx.mount(ElementBox::default().height(Length::Px(top_pad)));
            }
            for idx in start..end {
                ctx.mount(render_item(idx).height(Length::Px(item_height)));
            }
        })
}

/// Tree view composed directly on top of `virtual_list` + indent + expand toggle.
pub fn tree_view<F>(
    key: impl IntoNodeKey,
    visible_depths: Vec<(u32, bool, bool)>, // (id, has_children, expanded) with depth encoded via render
    depths: Vec<u16>,
    item_height: f32,
    viewport_height: f32,
    scroll_offset_y: f32,
    mut render_row_body: F,
) -> ElementBox
where
    F: FnMut(usize) -> ElementBox + 'static,
{
    let count = visible_depths.len();
    virtual_list(
        key,
        count,
        item_height,
        viewport_height,
        scroll_offset_y,
        move |idx| {
            let (node_id, has_children, expanded) = visible_depths[idx];
            let depth = depths.get(idx).copied().unwrap_or(0);
            let indent = (depth as f32) * 14.0;
            let mut r = row()
                .gap(4.0)
                .align(Align::Center)
                .child(ElementBox::default().width(Length::Px(indent)));
            if has_children {
                let glyph = if expanded { "v" } else { ">" };
                r = r.child(toggle_button(("tree_exp", node_id), glyph, expanded));
            } else {
                r = r.child(ElementBox::default().width(Length::Px(18.0)));
            }
            r.child(render_row_body(idx))
        },
    )
}

impl ElementBox {
    fn min_width_px(mut self, min_w: f32) -> Self {
        self.layout.min_width = min_w;
        self
    }
}
