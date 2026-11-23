//! UI Layout System — anchor-based layout computation
//!
//! Each frame queries all `Node + Style` entities, adds `ComputedLayout` output component as needed,
//! then computes final screen-space rect from anchors, fixed size, margins and parent container size.
//!
//! # layout rule
//! - Root element (no Parent) parent container = screen size (from `ScreenSize` resource)
//! - Child element parent container = parent's `ComputedLayout`
//! - Anchors define mapping of child edges relative to parent edges
//! - Fixed size overrides anchor stretching
//!
//! # Canvas scale (#463)
//! Node with [`Canvas`](crate::ui::components::Canvas) defines a scale factor for whole subtree:
//! layout computed in "virtual resolution" (`ScreenSize / scale`), then multiplied back by `scale` when
//! writing `ComputedLayout.rect`. So author writes `Style.width` / `margin` / anchors in design pixels,
//! 1080p and 720p get same composition.
//! `ComputedLayout.scale` left for render system to convert font size and corner radius.

use crate::ui::components::*;
use prism_ecs::World;

/// Screen size resource — updated each frame by engine.
#[derive(Clone, Copy, Debug)]
pub struct ScreenSize {
    pub width: f32,
    pub height: f32,
}

impl ScreenSize {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width: width as f32,
            height: height as f32,
        }
    }
}

/// Mapping between window pixels ↔ UI layout space (#988).
///
/// UI is now drawn together with scene into PostPass target rect (`internal_resolution` + integer scale
/// makes rect smaller than window). Layout still computed in window pixels, so pointer must first convert back to layout space,
/// hit test then matches drawn position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiScreenMapping {
    /// Layout space → window scale (target rect width / window width).
    pub scale: f32,
    /// Pixel offset of target rect top-left corner in window.
    pub offset: [f32; 2],
}

impl Default for UiScreenMapping {
    fn default() -> Self {
        Self {
            scale: 1.0,
            offset: [0.0, 0.0],
        }
    }
}

impl UiScreenMapping {
    /// Window pixels → layout space. Degenerates to identity map when `scale` non-finite or non-positive.
    pub fn window_to_layout(&self, point: [f32; 2]) -> [f32; 2] {
        if !(self.scale.is_finite() && self.scale > 0.0) {
            return point;
        }
        [
            (point[0] - self.offset[0]) / self.scale,
            (point[1] - self.offset[1]) / self.scale,
        ]
    }
}

/// Layout System — runs each frame, computes screen-space rects for all UI elements.
pub fn ui_layout_system(world: &mut World) {
    let screen = world
        .get_resource::<ScreenSize>()
        .copied()
        .unwrap_or(ScreenSize {
            width: 1920.0,
            height: 1080.0,
        });

    // First collect all `Node + Style` entities and Parent.
    // Layer level only recognizes scene's `Parent` (the one maintained by `HierarchyHelper`), no second
    // `UiParent` state. `ComputedLayout` is this system's **output**: author does not write it,
    // missing ones get defaults filled here, otherwise layout / hit test / draw (all query on it)
    // would silently skip the entity.
    // Two passes: first root nodes with no Parent, then child nodes
    let layout_queue: Vec<(prism_ecs::Entity, Option<prism_ecs::Entity>)> = world
        .query2::<Node, Style>()
        .map(|(entity, _node, _style)| {
            let parent = world.get::<prism_scene::components::Parent>(entity);
            (entity, parent.map(|p| p.0))
        })
        .collect();

    for (entity, _parent) in &layout_queue {
        if world.get::<ComputedLayout>(*entity).is_none() {
            world.insert(*entity, ComputedLayout::default());
        }
    }

    // Scale of Canvas each node is in (#463). Nearest Canvas ancestor decides;
    // without Canvas it is 1.0, behavior same as before Canvas introduced.
    let canvases: std::collections::HashMap<prism_ecs::Entity, Canvas> = world
        .query::<Canvas>()
        .map(|(entity, canvas)| (entity, *canvas))
        .collect();
    let parents: std::collections::HashMap<prism_ecs::Entity, prism_ecs::Entity> = layout_queue
        .iter()
        .filter_map(|(entity, parent)| parent.map(|parent| (*entity, parent)))
        .collect();
    let mut scales: std::collections::HashMap<prism_ecs::Entity, f32> =
        std::collections::HashMap::new();
    for (entity, _) in &layout_queue {
        let scale = node_scale(*entity, &parents, &canvases, &screen, &mut scales, 0);
        scales.insert(*entity, scale);
    }

    // Parent-child relationship of layout group (#527). Sibling order must match draw order: by scene `id`
    // (runtime `spawn`ed entities come after), so "first added on left/top" is intuitive,
    // and fully same source as `assign_draw_order`.
    let layout_entities: std::collections::HashSet<prism_ecs::Entity> =
        layout_queue.iter().map(|(entity, _)| *entity).collect();
    let mut children: std::collections::HashMap<prism_ecs::Entity, Vec<prism_ecs::Entity>> =
        std::collections::HashMap::new();
    let mut roots: Vec<prism_ecs::Entity> = Vec::new();
    for (entity, parent) in &layout_queue {
        match parent.filter(|parent| layout_entities.contains(parent)) {
            Some(parent) => children.entry(parent).or_default().push(*entity),
            None => roots.push(*entity),
        }
    }
    for list in children.values_mut() {
        list.sort_by_key(|entity| sibling_key(world, *entity));
    }
    roots.sort_by_key(|entity| sibling_key(world, *entity));

    // Parent-first child-after traversal order. `seen` also guards against Parent cycles.
    let mut order: Vec<prism_ecs::Entity> = Vec::new();
    let mut seen: std::collections::HashSet<prism_ecs::Entity> = std::collections::HashSet::new();
    let mut stack: Vec<prism_ecs::Entity> = roots.iter().rev().copied().collect();
    while let Some(entity) = stack.pop() {
        if !seen.insert(entity) {
            continue;
        }
        order.push(entity);
        if let Some(kids) = children.get(&entity) {
            for kid in kids.iter().rev() {
                stack.push(*kid);
            }
        }
    }

    let styles: std::collections::HashMap<prism_ecs::Entity, Style> = layout_queue
        .iter()
        .filter_map(|(entity, _)| {
            world
                .get::<Style>(*entity)
                .map(|style| (*entity, style.clone()))
        })
        .collect();

    // First pass measure: children first, parents after. Layout group measures own content size from children's sizes,
    // shrink accordingly when `fit_content` (#527).
    let measured = measure_sizes(&order, &children, &styles);

    // Second pass arrange: parents first, children after. Layout group children positioned by arrangement order, other nodes continue
    // with original "anchor × parent rect" algorithm.
    let mut resolved: std::collections::HashMap<prism_ecs::Entity, [f32; 4]> =
        std::collections::HashMap::new();
    // Main-axis cursor of each layout group (virtual coords, relative to content area start).
    let mut cursors: std::collections::HashMap<prism_ecs::Entity, f32> =
        std::collections::HashMap::new();

    for entity in &order {
        let Some(style) = styles.get(entity) else {
            continue;
        };
        let own = measured.get(entity).copied();
        let scale = scales.get(entity).copied().unwrap_or(1.0);
        let root_rect = [0.0, 0.0, screen.width / scale, screen.height / scale];

        let parent = parents.get(entity).copied();
        let rect = match parent {
            // Parent is not a UI entity (no Node + Style): reuse its existing output rect.
            Some(parent) if !layout_entities.contains(&parent) => {
                let parent_rect = world
                    .get::<ComputedLayout>(parent)
                    .map(|layout| {
                        let parent_scale = if layout.scale.is_finite() && layout.scale > 0.0 {
                            layout.scale
                        } else {
                            1.0
                        };
                        [
                            layout.rect[0] / parent_scale,
                            layout.rect[1] / parent_scale,
                            layout.rect[2] / parent_scale,
                            layout.rect[3] / parent_scale,
                        ]
                    })
                    .unwrap_or(root_rect);
                anchored_rect(style, parent_rect, own)
            }
            Some(parent) => {
                let parent_rect = resolved.get(&parent).copied().unwrap_or(root_rect);
                match styles.get(&parent) {
                    Some(parent_style)
                        if parent_style.layout.direction != LayoutDirection::None =>
                    {
                        place_in_layout(parent, parent_style.layout, parent_rect, own, &mut cursors)
                    }
                    _ => anchored_rect(style, parent_rect, own),
                }
            }
            None => anchored_rect(style, root_rect, own),
        };

        let screen_rect = [
            rect[0] * scale,
            rect[1] * scale,
            rect[2] * scale,
            rect[3] * scale,
        ];
        if let Some(layout) = world.get_mut::<ComputedLayout>(*entity) {
            layout.rect = screen_rect;
            layout.scale = scale;
        } else {
            world.insert(
                *entity,
                ComputedLayout {
                    rect: screen_rect,
                    order: 0,
                    scale,
                },
            );
        }
        resolved.insert(*entity, rect);
    }
    assign_draw_order(world);
}

/// Scale of Canvas entity is in (#463): nearest Canvas ancestor decides, otherwise 1.0.
///
/// Recursive + memoized; `depth` limit stops bad hierarchy (A's parent is B, B's parent is A) infinite recursion.
fn node_scale(
    entity: prism_ecs::Entity,
    parents: &std::collections::HashMap<prism_ecs::Entity, prism_ecs::Entity>,
    canvases: &std::collections::HashMap<prism_ecs::Entity, Canvas>,
    screen: &ScreenSize,
    cache: &mut std::collections::HashMap<prism_ecs::Entity, f32>,
    depth: u32,
) -> f32 {
    if let Some(scale) = cache.get(&entity) {
        return *scale;
    }
    if depth > 64 {
        return 1.0;
    }
    let scale = if let Some(canvas) = canvases.get(&entity) {
        canvas.scale_factor((screen.width, screen.height))
    } else if let Some(parent) = parents.get(&entity) {
        node_scale(*parent, parents, canvases, screen, cache, depth + 1)
    } else {
        1.0
    };
    cache.insert(entity, scale);
    scale
}

/// Compute `ComputedLayout::order` for each UI entity (draw order).
///
/// Semantics aligned with UGUI: **depth-first traversal by sibling order within same parent**, parent before children, whole subtree of a node
/// comes before its next sibling. So "later added full-screen mask" can cover earlier title
/// text (#462), instead of always drawing in two batches by layer 0/1.
///
/// Sibling order from `.scene.json` `id` (`SceneEntityId`); runtime `spawn`ed
/// entities lack it, sorted after all scene entities (by entity allocation order), consistent with UGUI where new
/// `Instantiate` siblings default append to end.
pub fn assign_draw_order(world: &mut World) {
    use std::collections::HashMap;

    let mut children: HashMap<prism_ecs::Entity, Vec<prism_ecs::Entity>> = HashMap::new();
    let mut roots: Vec<prism_ecs::Entity> = Vec::new();
    let mut all: Vec<prism_ecs::Entity> = Vec::new();

    for (entity, _node, _style) in world.query2::<Node, Style>() {
        all.push(entity);
    }

    let is_ui: std::collections::HashSet<prism_ecs::Entity> = all.iter().copied().collect();

    for &entity in &all {
        match world
            .get::<prism_scene::components::Parent>(entity)
            .map(|parent| parent.0)
            .filter(|parent| is_ui.contains(parent))
        {
            Some(parent) => children.entry(parent).or_default().push(entity),
            None => roots.push(entity),
        }
    }

    roots.sort_by_key(|entity| sibling_key(world, *entity));
    for list in children.values_mut() {
        list.sort_by_key(|entity| sibling_key(world, *entity));
    }

    // Depth-first pre-order; `depth` only guards Parent cycles (nodes in cycle appear again within own subtree
    // but no infinite recursion).
    let mut order: u32 = 0;
    let mut stack: Vec<(prism_ecs::Entity, u32)> =
        roots.into_iter().rev().map(|entity| (entity, 0)).collect();
    let mut seen: std::collections::HashSet<prism_ecs::Entity> = std::collections::HashSet::new();

    while let Some((entity, depth)) = stack.pop() {
        if !seen.insert(entity) {
            continue;
        }
        if let Some(layout) = world.get_mut::<ComputedLayout>(entity) {
            layout.order = order;
            order = order.saturating_add(1);
        }
        if depth < 64 {
            if let Some(kids) = children.get(&entity) {
                for &kid in kids.iter().rev() {
                    stack.push((kid, depth + 1));
                }
            }
        }
    }
}

/// Sibling sort key: scene `id` first, runtime entities by allocation order.
fn sibling_key(world: &World, entity: prism_ecs::Entity) -> (u32, u32) {
    let scene_id = world
        .get::<crate::scene::components::SceneEntityId>(entity)
        .map(|id| id.0)
        .unwrap_or(u32::MAX);
    (scene_id, entity.id())
}

/// Size of `(main axis, cross axis)` according to direction.
fn main_cross(direction: LayoutDirection, size: [f32; 2]) -> (f32, f32) {
    match direction {
        LayoutDirection::Row => (size[0], size[1]),
        LayoutDirection::Column => (size[1], size[0]),
        LayoutDirection::None => (0.0, 0.0),
    }
}

/// Measure pass: "content size" of each node.
///
/// Non-layout-group node is explicit width/height (0 if not written); layout group in `fit_content` shrinks itself
/// to "sum of children main axis + spacing + padding" and "cross axis max + padding".
fn measure_sizes(
    order: &[prism_ecs::Entity],
    children: &std::collections::HashMap<prism_ecs::Entity, Vec<prism_ecs::Entity>>,
    styles: &std::collections::HashMap<prism_ecs::Entity, Style>,
) -> std::collections::HashMap<prism_ecs::Entity, [f32; 2]> {
    /// Finite and non-negative; bad values as 0.
    fn extent(value: f32) -> f32 {
        if value.is_finite() && value > 0.0 {
            value
        } else {
            0.0
        }
    }

    let mut measured: std::collections::HashMap<prism_ecs::Entity, [f32; 2]> =
        std::collections::HashMap::new();
    for entity in order.iter().rev() {
        let Some(style) = styles.get(entity) else {
            continue;
        };
        let mut size = [
            style.width.map(extent).unwrap_or(0.0),
            style.height.map(extent).unwrap_or(0.0),
        ];
        let layout = style.layout;
        if layout.direction != LayoutDirection::None {
            let padding = layout.padding;
            let spacing = extent(layout.spacing);
            let mut main = 0.0;
            let mut cross: f32 = 0.0;
            let mut count = 0usize;
            if let Some(kids) = children.get(entity) {
                for kid in kids {
                    let Some(kid_size) = measured.get(kid).copied() else {
                        continue;
                    };
                    let (kid_main, kid_cross) = main_cross(layout.direction, kid_size);
                    if count > 0 {
                        main += spacing;
                    }
                    count += 1;
                    main += kid_main;
                    cross = cross.max(kid_cross);
                }
            }
            if layout.fit_content {
                let (content_main, content_cross) = match layout.direction {
                    LayoutDirection::Row => (
                        extent(padding.left) + extent(padding.right),
                        extent(padding.top) + extent(padding.bottom),
                    ),
                    LayoutDirection::Column => (
                        extent(padding.top) + extent(padding.bottom),
                        extent(padding.left) + extent(padding.right),
                    ),
                    LayoutDirection::None => (0.0, 0.0),
                };
                let (content_w, content_h) = match layout.direction {
                    LayoutDirection::Row => (main + content_main, cross + content_cross),
                    LayoutDirection::Column => (cross + content_cross, main + content_main),
                    LayoutDirection::None => (0.0, 0.0),
                };
                if style.width.is_none() {
                    size[0] = content_w.max(0.0);
                }
                if style.height.is_none() {
                    size[1] = content_h.max(0.0);
                }
            }
        }
        measured.insert(*entity, size);
    }
    measured
}

/// Place one child into parent's layout group: main axis accumulates by cursor, cross axis by `child_align`.
fn place_in_layout(
    parent: prism_ecs::Entity,
    layout: Layout,
    parent_rect: [f32; 4],
    own: Option<[f32; 2]>,
    cursors: &mut std::collections::HashMap<prism_ecs::Entity, f32>,
) -> [f32; 4] {
    let padding = layout.padding;
    let content = [
        parent_rect[0] + padding.left,
        parent_rect[1] + padding.top,
        (parent_rect[2] - padding.left - padding.right).max(0.0),
        (parent_rect[3] - padding.top - padding.bottom).max(0.0),
    ];
    let own = own.unwrap_or([0.0, 0.0]);
    let (main, cross_default) = main_cross(layout.direction, own);
    let cross_usable = match layout.direction {
        LayoutDirection::Row => content[3],
        LayoutDirection::Column => content[2],
        LayoutDirection::None => 0.0,
    };
    let cross = if layout.child_align == ChildAlign::Stretch {
        cross_usable
    } else {
        cross_default
    };
    let cross_offset = match layout.child_align {
        ChildAlign::Start | ChildAlign::Stretch => 0.0,
        ChildAlign::Center => ((cross_usable - cross) * 0.5).max(0.0),
        ChildAlign::End => (cross_usable - cross).max(0.0),
    };
    let cursor = cursors.entry(parent).or_insert(0.0);
    let rect = match layout.direction {
        LayoutDirection::Row => [content[0] + *cursor, content[1] + cross_offset, main, cross],
        LayoutDirection::Column => [content[0] + cross_offset, content[1] + *cursor, cross, main],
        LayoutDirection::None => [content[0], content[1], main, cross],
    };
    *cursor += main
        + if layout.spacing.is_finite() && layout.spacing > 0.0 {
            layout.spacing
        } else {
            0.0
        };
    rect
}

/// Anchor rect override: layout group with `fit_content` and no explicit width/height uses measured content size,
/// then repositions by anchor/pivot.
fn anchored_rect(style: &Style, parent: [f32; 4], measured: Option<[f32; 2]>) -> [f32; 4] {
    if style.layout.direction != LayoutDirection::None && style.layout.fit_content {
        let measured = measured.unwrap_or([0.0, 0.0]);
        let size = [
            style.width.unwrap_or(measured[0]).max(0.0),
            style.height.unwrap_or(measured[1]).max(0.0),
        ];
        return compute_rect_sized(style, parent, Some(size));
    }
    compute_rect(style, parent)
}

/// Compute screen-space rect from Style and parent container rect.
///
/// Semantics aligned with engine's `anchorMin` / `anchorMax` + `pivot`:
/// - **Stretch axis** (`min != max`): position from anchors and margins, `pivot` **not involved**
///   (otherwise full-screen root element would get negative origin);
/// - **point anchor axis** (`min == max`): size from `width`/`height`, position aligned by `pivot`.
fn compute_rect(style: &Style, parent: [f32; 4]) -> [f32; 4] {
    compute_rect_sized(style, parent, None)
}

/// Optional size override for [`compute_rect`] (used by layout group `fit_content`).
fn compute_rect_sized(style: &Style, parent: [f32; 4], size: Option<[f32; 2]>) -> [f32; 4] {
    let (px, py, pw, ph) = (parent[0], parent[1], parent[2], parent[3]);

    // Anchors → four rect edges
    let anchor_left = px + style.anchors.min_x * pw;
    let anchor_right = px + style.anchors.max_x * pw;
    let anchor_top = py + style.anchors.min_y * ph;
    let anchor_bottom = py + style.anchors.max_y * ph;

    let margin = &style.margin;

    let stretched_x = style.anchors.min_x != style.anchors.max_x;
    let stretched_y = style.anchors.min_y != style.anchors.max_y;

    // Fixed size vs anchor stretching; layout group `fit_content` uses caller-provided content size.
    let w = size
        .map(|size| size[0])
        .or(style.width)
        .unwrap_or((anchor_right - anchor_left) - margin.left - margin.right);
    let h = size
        .map(|size| size[1])
        .or(style.height)
        .unwrap_or((anchor_bottom - anchor_top) - margin.top - margin.bottom);
    let w = w.max(0.0);
    let h = h.max(0.0);

    // Stretch axis: anchors + margins decide origin; point anchor axis: pivot participates.
    // Point anchors must apply right/bottom margin only when aligned accordingly, otherwise element pinned to bottom-right corner
    // would hug screen edge (#593).
    let left = if stretched_x {
        anchor_left + margin.left
    } else {
        anchor_left + margin.left - margin.right - style.pivot.x * w
    };
    let top = if stretched_y {
        anchor_top + margin.top
    } else {
        anchor_top + margin.top - margin.bottom - style.pivot.y * h
    };

    [left, top, w, h]
}

/// If anchor min == max (point anchor), use this + fixed size for positioning.
fn _anchor_point_rect(style: &Style, parent: [f32; 4]) -> [f32; 4] {
    let (px, py, pw, ph) = (parent[0], parent[1], parent[2], parent[3]);

    let anchor_x = px + style.anchors.min_x * pw;
    let anchor_y = py + style.anchors.min_y * ph;

    let w = style.width.unwrap_or(0.0);
    let h = style.height.unwrap_or(0.0);

    let left = anchor_x - style.pivot.x * w + style.margin.left - style.margin.right;
    let top = anchor_y - style.pivot.y * h + style.margin.top - style.margin.bottom;

    [left, top, w.max(0.0), h.max(0.0)]
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
