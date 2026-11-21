//! Two-pass Flexbox layout solver over `SlotMap<NodeId, UiNode>`.

use glam::Vec2;
use slotmap::SlotMap;

use crate::style::{Align, FlexDirection, Justify, Length, PositionType, Rect};
use crate::tree::{NodeContent, NodeId, UiNode};

/// Compute `computed_rect` for every node in `nodes` starting from `root`.
pub(crate) fn compute_layout(
    nodes: &mut SlotMap<NodeId, UiNode>,
    root: NodeId,
    viewport_size: Vec2,
) {
    let intrinsic = measure_node(nodes, root, viewport_size);
    let root_rect = Rect::new(
        0.0,
        0.0,
        viewport_size.x.max(intrinsic.x),
        viewport_size.y.max(intrinsic.y),
    );
    arrange_node(nodes, root, root_rect);
}

fn resolve_length(len: Length, available: f32, intrinsic: f32) -> f32 {
    match len {
        Length::Px(px) => px,
        Length::Percent(p) => available * (p / 100.0),
        Length::Auto | Length::Grow(_) => intrinsic,
    }
}

fn measure_text_approx(text: &str, font_size: f32) -> Vec2 {
    let width = (text.chars().count() as f32) * font_size * 0.56;
    Vec2::new(width.ceil(), (font_size * 1.35).ceil())
}

fn measure_node(nodes: &mut SlotMap<NodeId, UiNode>, id: NodeId, available: Vec2) -> Vec2 {
    let Some(node) = nodes.get(id).cloned() else {
        return Vec2::ZERO;
    };

    let content_size = match &node.content {
        NodeContent::Text {
            text, font_size, ..
        } => measure_text_approx(text, *font_size),
        _ => Vec2::ZERO,
    };

    let mut total_main = 0.0f32;
    let mut max_cross = 0.0f32;
    let mut rel_count = 0usize;

    for &child_id in &node.children {
        let child_pos = nodes
            .get(child_id)
            .map(|c| c.layout.position)
            .unwrap_or_default();
        let child_size = measure_node(nodes, child_id, available);
        if child_pos == PositionType::Absolute {
            continue;
        }
        rel_count += 1;
        match node.layout.direction {
            FlexDirection::Row => {
                total_main += child_size.x;
                max_cross = max_cross.max(child_size.y);
            }
            FlexDirection::Column => {
                total_main += child_size.y;
                max_cross = max_cross.max(child_size.x);
            }
        }
    }

    let gap_total = if rel_count > 1 {
        node.layout.gap * (rel_count - 1) as f32
    } else {
        0.0
    };

    let inner = match node.layout.direction {
        FlexDirection::Row => Vec2::new(
            (total_main + gap_total).max(content_size.x),
            max_cross.max(content_size.y),
        ),
        FlexDirection::Column => Vec2::new(
            max_cross.max(content_size.x),
            (total_main + gap_total).max(content_size.y),
        ),
    };

    let pad = node.layout.padding;
    let raw_w = resolve_length(node.layout.width, available.x, inner.x + pad.horizontal());
    let raw_h = resolve_length(node.layout.height, available.y, inner.y + pad.vertical());
    let clamped = Vec2::new(
        raw_w.clamp(node.layout.min_width, node.layout.max_width),
        raw_h.clamp(node.layout.min_height, node.layout.max_height),
    );
    if let Some(n) = nodes.get_mut(id) {
        n.intrinsic_size = clamped;
    }
    clamped
}

fn arrange_node(nodes: &mut SlotMap<NodeId, UiNode>, id: NodeId, rect: Rect) {
    let Some(node) = nodes.get_mut(id) else {
        return;
    };
    node.computed_rect = rect;
    let layout = node.layout;
    let children = node.children.clone();

    let inner_x = rect.x + layout.padding.left;
    let inner_y = rect.y + layout.padding.top;
    let inner_w = (rect.w - layout.padding.horizontal()).max(0.0);
    let inner_h = (rect.h - layout.padding.vertical()).max(0.0);

    let mut rel_children = Vec::new();
    let mut abs_children = Vec::new();
    for child_id in children {
        if let Some(c) = nodes.get(child_id) {
            if c.layout.position == PositionType::Absolute {
                abs_children.push(child_id);
            } else {
                rel_children.push(child_id);
            }
        }
    }

    // 1. Arrange absolute children directly inside parent rect.
    for child_id in abs_children {
        let Some(c) = nodes.get(child_id) else {
            continue;
        };
        let cw = resolve_length(c.layout.width, rect.w, c.intrinsic_size.x)
            .clamp(c.layout.min_width, c.layout.max_width);
        let ch = resolve_length(c.layout.height, rect.h, c.intrinsic_size.y)
            .clamp(c.layout.min_height, c.layout.max_height);
        let cx = rect.x + resolve_length(c.layout.left, rect.w, 0.0);
        let cy = rect.y + resolve_length(c.layout.top, rect.h, 0.0);
        arrange_node(nodes, child_id, Rect::new(cx, cy, cw, ch));
    }

    if rel_children.is_empty() {
        return;
    }

    let is_row = layout.direction == FlexDirection::Row;
    let avail_main = if is_row { inner_w } else { inner_h };
    let avail_cross = if is_row { inner_h } else { inner_w };
    let total_gap = layout.gap * (rel_children.len().saturating_sub(1)) as f32;

    let mut fixed_main = 0.0f32;
    let mut total_grow = 0.0f32;

    for &cid in &rel_children {
        let c = &nodes[cid];
        let main_len = if is_row {
            c.layout.width
        } else {
            c.layout.height
        };
        match main_len {
            Length::Grow(g) => total_grow += g.max(0.0),
            Length::Px(px) => fixed_main += px,
            Length::Percent(p) => fixed_main += avail_main * (p / 100.0),
            Length::Auto => {
                fixed_main += if is_row {
                    c.intrinsic_size.x
                } else {
                    c.intrinsic_size.y
                };
            }
        }
    }

    let free_main = (avail_main - fixed_main - total_gap).max(0.0);
    let mut cursor = match layout.justify_content {
        Justify::Start | Justify::SpaceBetween => 0.0,
        Justify::Center if total_grow == 0.0 => free_main * 0.5,
        Justify::End if total_grow == 0.0 => free_main,
        _ => 0.0,
    };
    let step_gap = if layout.justify_content == Justify::SpaceBetween
        && total_grow == 0.0
        && rel_children.len() > 1
    {
        layout.gap + free_main / (rel_children.len() - 1) as f32
    } else {
        layout.gap
    };

    for &cid in &rel_children {
        let (c_layout, c_intrinsic) = {
            let c = &nodes[cid];
            (c.layout, c.intrinsic_size)
        };
        let main_len = if is_row {
            c_layout.width
        } else {
            c_layout.height
        };
        let cross_len = if is_row {
            c_layout.height
        } else {
            c_layout.width
        };

        let main_size = match main_len {
            Length::Grow(g) if total_grow > 0.0 => free_main * (g.max(0.0) / total_grow),
            Length::Grow(_) => 0.0,
            Length::Px(px) => px,
            Length::Percent(p) => avail_main * (p / 100.0),
            Length::Auto => {
                if is_row {
                    c_intrinsic.x
                } else {
                    c_intrinsic.y
                }
            }
        };

        let cross_intrinsic = if is_row { c_intrinsic.y } else { c_intrinsic.x };
        let cross_size = match cross_len {
            Length::Grow(_) => avail_cross,
            Length::Px(px) => px,
            Length::Percent(p) => avail_cross * (p / 100.0),
            Length::Auto => {
                if layout.align_items == Align::Stretch {
                    avail_cross
                } else {
                    cross_intrinsic
                }
            }
        };

        let cross_offset = match layout.align_items {
            Align::Start | Align::Stretch => 0.0,
            Align::Center => (avail_cross - cross_size) * 0.5,
            Align::End => avail_cross - cross_size,
        };

        let child_rect = if is_row {
            Rect::new(
                inner_x + cursor,
                inner_y + cross_offset,
                main_size,
                cross_size,
            )
        } else {
            Rect::new(
                inner_x + cross_offset,
                inner_y + cursor,
                cross_size,
                main_size,
            )
        };
        arrange_node(nodes, cid, child_rect);
        cursor += main_size + step_gap;
    }
}
