//! Docking layout tree (Rust port of `editor/src/dock.ts`).
//!
//! Nodes are of two kinds:
//! - `leaf`: a panel container that can hold multiple tabs, only the active one shown at a time.
//! - `split`: horizontal / vertical bisection, `ratio` is the share of the first half.
//!
//! Adjusting a splitter only changes `ratio`, so all aligned panels on the same side resize
//! together; this is also the core behavior of ImGui docking.
//!
//! The serialization shape matches the frontend `prisma-editor.dock.v4` exactly (`{version, dock,
//! floating}`), so the same layout can be carried between the Web editor and the egui editor.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

pub const MIN_RATIO: f32 = 0.12;
pub const MAX_RATIO: f32 = 0.88;
pub const MIN_FLOAT_WIDTH: f32 = 220.0;
pub const MIN_FLOAT_HEIGHT: f32 = 140.0;
pub const SNAP_THRESHOLD: f32 = 10.0;
/// Total height of the top menu + toolbar; floating windows may not cover it.
pub const TOOLBAR_HEIGHT: f32 = 36.0;
/// Padding around the editor.
pub const VIEWPORT_MARGIN: f32 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DockDirection {
    Row,
    Column,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockZone {
    Center,
    Left,
    Right,
    Top,
    Bottom,
}

impl DockZone {
    pub fn as_str(self) -> &'static str {
        match self {
            DockZone::Center => "center",
            DockZone::Left => "left",
            DockZone::Right => "right",
            DockZone::Top => "top",
            DockZone::Bottom => "bottom",
        }
    }
}

/// Main window's three edges (top edge is the toolbar, not a dock target).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockEdge {
    Left,
    Right,
    Bottom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DockNode {
    #[serde(rename = "leaf")]
    Leaf {
        id: String,
        tabs: Vec<String>,
        active: String,
    },
    #[serde(rename = "split")]
    Split {
        id: String,
        dir: DockDirection,
        ratio: f32,
        a: Box<DockNode>,
        b: Box<DockNode>,
    },
}

impl DockNode {
    pub fn id(&self) -> &str {
        match self {
            DockNode::Leaf { id, .. } | DockNode::Split { id, .. } => id,
        }
    }

    pub fn is_leaf(&self) -> bool {
        matches!(self, DockNode::Leaf { .. })
    }

    pub fn active_tab(&self) -> Option<&str> {
        match self {
            DockNode::Leaf { tabs, active, .. } => {
                if tabs.is_empty() {
                    None
                } else {
                    Some(active.as_str())
                }
            }
            DockNode::Split { .. } => None,
        }
    }
}

/// Free-floating panel detached from the dock tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FloatPanel {
    pub tab: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub z: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FloatRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResizeDirection {
    N,
    S,
    E,
    W,
    Nw,
    Ne,
    Sw,
    Se,
}

impl ResizeDirection {
    pub fn has(self, axis: char) -> bool {
        matches!(
            (self, axis),
            (
                ResizeDirection::N | ResizeDirection::Nw | ResizeDirection::Ne,
                'n'
            ) | (
                ResizeDirection::S | ResizeDirection::Sw | ResizeDirection::Se,
                's'
            ) | (
                ResizeDirection::E | ResizeDirection::Ne | ResizeDirection::Se,
                'e'
            ) | (
                ResizeDirection::W | ResizeDirection::Nw | ResizeDirection::Sw,
                'w'
            )
        )
    }
}

static SEQ: AtomicU64 = AtomicU64::new(1);

fn uid(prefix: &str) -> String {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}{n}")
}

pub fn leaf(tabs: impl IntoIterator<Item = impl Into<String>>) -> DockNode {
    let tabs: Vec<String> = tabs.into_iter().map(Into::into).collect();
    let active = tabs.first().cloned().unwrap_or_default();
    DockNode::Leaf {
        id: uid("leaf-"),
        tabs,
        active,
    }
}

pub fn leaf_with_active(tabs: Vec<String>, active: impl Into<String>) -> DockNode {
    DockNode::Leaf {
        id: uid("leaf-"),
        tabs,
        active: active.into(),
    }
}

pub fn split(dir: DockDirection, a: DockNode, b: DockNode, ratio: f32) -> DockNode {
    DockNode::Split {
        id: uid("split-"),
        dir,
        ratio: clamp_ratio(ratio),
        a: Box::new(a),
        b: Box::new(b),
    }
}

/// Pixel length of the first half of a row / column split (#498).
///
/// `f32::clamp(min, max)` panics unconditionally when `min > max`, and nested splits easily
/// produce this state: dragging the outer splitter to the limit, or the window shrinking to
/// ~900px wide leaves the middle column ~95px, while its inner Row wants 60px on each side
/// (`clamp(60.0, 95 - 5 - 60 = 30.0)`) — the editor would abort outright.
///
/// Here we follow Unity `SplitView`'s approach of computing bounds first: when both minimums
/// don't fit, degrade to a halving split; no `usable` / `ratio` combination panics (negative
/// `usable`, NaN `ratio` included).
pub fn split_side_length(usable: f32, ratio: f32, min: f32) -> f32 {
    if !usable.is_finite() || usable <= 0.0 {
        return 0.0;
    }
    let min = if min.is_finite() { min.max(0.0) } else { 0.0 };
    let ratio = if ratio.is_finite() { ratio } else { 0.5 };
    if usable <= 2.0 * min {
        // Neither side fits the minimum: don't force it, split in half (at least no panic, and
        // the splitter isn't drawn outside the frame).
        usable * 0.5
    } else {
        (usable * ratio).clamp(min, usable - min)
    }
}

pub fn clamp_ratio(ratio: f32) -> f32 {
    if !ratio.is_finite() {
        return 0.5;
    }
    ratio.clamp(MIN_RATIO, MAX_RATIO)
}

/// Default layout: docs / hierarchy / assets on the left, inspector on the right, viewport in
/// the middle. Consistent with `editor/src/dock.ts::defaultDock`.
pub fn default_dock() -> DockNode {
    let assets = leaf(["assets"]);
    let hierarchy = split(DockDirection::Column, leaf(["hierarchy"]), assets, 0.55);
    let left = split(DockDirection::Column, leaf(["docs"]), hierarchy, 0.3);
    let viewport = leaf(["sceneView", "gameView"]);
    let main = split(DockDirection::Row, viewport, leaf(["inspector"]), 0.76);
    split(DockDirection::Row, left, main, 0.19)
}

/// Classic 2 by 3 layout.
pub fn two_by_three_dock() -> DockNode {
    let left_viewports = split(
        DockDirection::Column,
        leaf(["sceneView"]),
        leaf(["gameView"]),
        0.5,
    );
    let mid_trees = split(
        DockDirection::Column,
        leaf(["hierarchy", "docs"]),
        leaf(["assets", "log"]),
        0.55,
    );
    let left_and_mid = split(DockDirection::Row, left_viewports, mid_trees, 0.7);
    split(DockDirection::Row, left_and_mid, leaf(["inspector"]), 0.78)
}

/// Classic Tall layout.
pub fn tall_dock() -> DockNode {
    let left_viewports = split(
        DockDirection::Column,
        leaf(["sceneView"]),
        leaf(["gameView"]),
        0.5,
    );
    let right_column = split(
        DockDirection::Column,
        leaf(["hierarchy", "assets"]),
        leaf(["inspector"]),
        0.45,
    );
    split(DockDirection::Row, left_viewports, right_column, 0.72)
}

/// Classic Wide layout.
pub fn wide_dock() -> DockNode {
    let top_viewports = split(
        DockDirection::Row,
        leaf(["sceneView"]),
        leaf(["gameView"]),
        0.5,
    );
    let bottom_panels = split(
        DockDirection::Row,
        leaf(["hierarchy", "docs"]),
        split(
            DockDirection::Row,
            leaf(["assets", "log"]),
            leaf(["inspector"]),
            0.55,
        ),
        0.28,
    );
    split(DockDirection::Column, top_viewports, bottom_panels, 0.65)
}

pub struct LayoutPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub factory: fn() -> DockNode,
}

pub const LAYOUT_PRESETS: &[LayoutPreset] = &[
    LayoutPreset {
        id: "default",
        label: "Default",
        factory: default_dock,
    },
    LayoutPreset {
        id: "2by3",
        label: "2 by 3",
        factory: two_by_three_dock,
    },
    LayoutPreset {
        id: "tall",
        label: "Tall",
        factory: tall_dock,
    },
    LayoutPreset {
        id: "wide",
        label: "Wide",
        factory: wide_dock,
    },
];

pub fn layout_preset(id: &str) -> Option<DockNode> {
    LAYOUT_PRESETS
        .iter()
        .find(|preset| preset.id == id)
        .map(|preset| (preset.factory)())
}

pub fn collect_tabs(node: &DockNode) -> Vec<String> {
    match node {
        DockNode::Leaf { tabs, .. } => tabs.clone(),
        DockNode::Split { a, b, .. } => {
            let mut all = collect_tabs(a);
            all.extend(collect_tabs(b));
            all
        }
    }
}

pub fn find_leaf<'a>(node: &'a DockNode, tab: &str) -> Option<&'a DockNode> {
    match node {
        DockNode::Leaf { tabs, .. } => tabs.iter().any(|t| t == tab).then_some(node),
        DockNode::Split { a, b, .. } => find_leaf(a, tab).or_else(|| find_leaf(b, tab)),
    }
}

pub fn find_leaf_by_id<'a>(node: &'a DockNode, leaf_id: &str) -> Option<&'a DockNode> {
    match node {
        DockNode::Leaf { id, .. } => (id == leaf_id).then_some(node),
        DockNode::Split { a, b, .. } => {
            find_leaf_by_id(a, leaf_id).or_else(|| find_leaf_by_id(b, leaf_id))
        }
    }
}

pub fn first_leaf(node: &DockNode) -> &DockNode {
    match node {
        DockNode::Leaf { .. } => node,
        DockNode::Split { a, .. } => {
            if a.is_leaf() {
                a
            } else {
                first_leaf(a)
            }
        }
    }
}

/// Remove a tab from the tree; empty leaves are pruned, a split with only one child collapses.
pub fn remove_panel(node: &DockNode, tab: &str) -> Option<DockNode> {
    match node {
        DockNode::Leaf { id, tabs, active } => {
            if !tabs.iter().any(|t| t == tab) {
                return Some(node.clone());
            }
            let tabs: Vec<String> = tabs.iter().filter(|t| *t != tab).cloned().collect();
            if tabs.is_empty() {
                return None;
            }
            let active = if tabs.iter().any(|t| t == active) {
                active.clone()
            } else {
                tabs[0].clone()
            };
            Some(DockNode::Leaf {
                id: id.clone(),
                tabs,
                active,
            })
        }
        DockNode::Split {
            id,
            dir,
            ratio,
            a,
            b,
        } => {
            let a = remove_panel(a, tab);
            let b = remove_panel(b, tab);
            match (a, b) {
                (Some(a), Some(b)) => Some(DockNode::Split {
                    id: id.clone(),
                    dir: *dir,
                    ratio: *ratio,
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (Some(node), None) | (None, Some(node)) => Some(node),
                (None, None) => None,
            }
        }
    }
}

pub fn activate_tab(node: &DockNode, leaf_id: &str, tab: &str) -> DockNode {
    match node {
        DockNode::Leaf {
            id,
            tabs,
            active: _,
        } => {
            if id != leaf_id || !tabs.iter().any(|t| t == tab) {
                return node.clone();
            }
            DockNode::Leaf {
                id: id.clone(),
                tabs: tabs.clone(),
                active: tab.to_string(),
            }
        }
        DockNode::Split {
            id,
            dir,
            ratio,
            a,
            b,
        } => DockNode::Split {
            id: id.clone(),
            dir: *dir,
            ratio: *ratio,
            a: Box::new(activate_tab(a, leaf_id, tab)),
            b: Box::new(activate_tab(b, leaf_id, tab)),
        },
    }
}

/// Update a split's ratio (located by split id).
pub fn set_ratio(node: &DockNode, split_id: &str, ratio: f32, min: f32, max: f32) -> DockNode {
    match node {
        DockNode::Leaf { .. } => node.clone(),
        DockNode::Split {
            id,
            dir,
            ratio: r,
            a,
            b,
        } => {
            if id == split_id {
                return DockNode::Split {
                    id: id.clone(),
                    dir: *dir,
                    ratio: clamp_ratio_between(ratio, min, max),
                    a: a.clone(),
                    b: b.clone(),
                };
            }
            DockNode::Split {
                id: id.clone(),
                dir: *dir,
                ratio: *r,
                a: Box::new(set_ratio(a, split_id, ratio, min, max)),
                b: Box::new(set_ratio(b, split_id, ratio, min, max)),
            }
        }
    }
}

pub fn clamp_ratio_between(ratio: f32, min: f32, max: f32) -> f32 {
    if !ratio.is_finite() {
        return 0.5;
    }
    let lo = min.max(0.02);
    let hi = max.min(0.98);
    ratio.clamp(lo, hi.max(lo))
}

fn replace_leaf(node: &DockNode, leaf_id: &str, replacement: DockNode) -> DockNode {
    match node {
        DockNode::Leaf { id, .. } => {
            if id == leaf_id {
                replacement
            } else {
                node.clone()
            }
        }
        DockNode::Split {
            id,
            dir,
            ratio,
            a,
            b,
        } => DockNode::Split {
            id: id.clone(),
            dir: *dir,
            ratio: *ratio,
            a: Box::new(replace_leaf(a, leaf_id, replacement.clone())),
            b: Box::new(replace_leaf(b, leaf_id, replacement)),
        },
    }
}

/// Place a panel into a region of the target leaf.
/// - center: becomes a tab of the target leaf (supports reordering tabs within the same leaf);
// - left / right / top / bottom: open a new leaf next to the target.
pub fn insert_panel(
    root: &DockNode,
    target_leaf_id: &str,
    tab: &str,
    zone: DockZone,
    ratio: f32,
    target_index: Option<usize>,
) -> DockNode {
    let Some(target) = find_leaf_by_id(root, target_leaf_id) else {
        return root.clone();
    };

    if zone == DockZone::Center {
        let (id, tabs) = match target {
            DockNode::Leaf { id, tabs, .. } => (id.clone(), tabs.clone()),
            _ => unreachable!("find_leaf_by_id returns leaf only"),
        };

        if let Some(current_index) = tabs.iter().position(|t| t == tab) {
            // Reorder tabs within the same leaf.
            let filtered: Vec<String> = tabs.iter().filter(|t| *t != tab).cloned().collect();
            let insert_at = match target_index {
                Some(index) => {
                    let adjusted = if index > current_index {
                        index - 1
                    } else {
                        index
                    };
                    adjusted.min(filtered.len())
                }
                None => filtered.len(),
            };
            let mut next = filtered;
            next.insert(insert_at, tab.to_string());
            return replace_leaf(
                root,
                target_leaf_id,
                DockNode::Leaf {
                    id,
                    tabs: next,
                    active: tab.to_string(),
                },
            );
        }

        let detached = remove_panel(root, tab).unwrap_or_else(|| root.clone());
        let Some(next_target) = find_leaf_by_id(&detached, target_leaf_id) else {
            // The target leaf disappeared because we dragged away its only panel; put it back in place.
            return detached;
        };
        let (id, mut next_tabs) = match next_target {
            DockNode::Leaf { id, tabs, .. } => (id.clone(), tabs.clone()),
            _ => unreachable!(),
        };
        let insert_at = target_index.unwrap_or(next_tabs.len()).min(next_tabs.len());
        next_tabs.insert(insert_at, tab.to_string());
        return replace_leaf(
            &detached,
            target_leaf_id,
            DockNode::Leaf {
                id,
                tabs: next_tabs,
                active: tab.to_string(),
            },
        );
    }

    // First remove the tab from its old position; if the target leaf disappears as a result,
    // we're dragging the target leaf's own only tab — edge-splitting onto itself would just
    // duplicate it, so do nothing.
    let Some(next) = remove_panel(root, tab) else {
        return root.clone();
    };
    let Some(next_target) = find_leaf_by_id(&next, target_leaf_id) else {
        return root.clone();
    };

    let placed = leaf([tab]);
    let dir = if matches!(zone, DockZone::Left | DockZone::Right) {
        DockDirection::Row
    } else {
        DockDirection::Column
    };
    let first_on_top = matches!(zone, DockZone::Left | DockZone::Top);
    let (first, second) = if first_on_top {
        (placed, next_target.clone())
    } else {
        (next_target.clone(), placed)
    };
    let node = split(
        dir,
        first,
        second,
        if first_on_top { ratio } else { 1.0 - ratio },
    );
    replace_leaf(&next, target_leaf_id, node)
}

/// Dock a panel to the **main window's** left / right / bottom edge (opens a whole column or row).
pub fn dock_panel_at_edge(
    root: Option<&DockNode>,
    tab: &str,
    edge: DockEdge,
    ratio: f32,
) -> DockNode {
    let next = root.and_then(|root| remove_panel(root, tab));
    let Some(next) = next else {
        return root.cloned().unwrap_or_else(|| leaf([tab]));
    };

    let placed = leaf([tab]);
    match edge {
        DockEdge::Left => split(DockDirection::Row, placed, next, ratio),
        DockEdge::Right => split(DockDirection::Row, next, placed, 1.0 - ratio),
        DockEdge::Bottom => split(DockDirection::Column, next, placed, 1.0 - ratio),
    }
}

/// Utility / settings panels (default to joining the inspector leaf as a tab, not opening a new column).
pub const UTILITY_TABS: &[&str] = &[
    "renderGraph",
    "projectSettings",
    "preferences",
    "build",
    "problems",
    "prefabOverrides",
    "material",
    "tools",
    "shortcuts",
    "about",
    "history",
];

/// For the "show panel" menu: place the panel back beside a reasonable anchor.
pub fn show_panel(root: &DockNode, tab: &str) -> DockNode {
    if collect_tabs(root).iter().any(|t| t == tab) {
        return match find_leaf(root, tab) {
            Some(leaf) => activate_tab(root, leaf.id(), tab),
            None => root.clone(),
        };
    }

    if tab == "log" {
        return split(DockDirection::Column, root.clone(), leaf([tab]), 0.76);
    }

    if UTILITY_TABS.contains(&tab) {
        let mut hosts: Vec<&str> = vec!["inspector"];
        hosts.extend(UTILITY_TABS.iter().copied());
        for host in hosts {
            if let Some(leaf) = find_leaf(root, host) {
                return insert_panel(root, leaf.id(), tab, DockZone::Center, 0.5, None);
            }
        }
    }

    let anchors: &[(&str, &[(&str, DockZone)])] = &[
        (
            "docs",
            &[
                ("hierarchy", DockZone::Top),
                ("assets", DockZone::Top),
                ("inspector", DockZone::Left),
            ],
        ),
        (
            "hierarchy",
            &[("docs", DockZone::Bottom), ("assets", DockZone::Top)],
        ),
        (
            "assets",
            &[("hierarchy", DockZone::Bottom), ("docs", DockZone::Bottom)],
        ),
        (
            "inspector",
            &[
                ("docs", DockZone::Right),
                ("hierarchy", DockZone::Right),
                ("assets", DockZone::Right),
            ],
        ),
    ];

    for (anchor, zones) in anchors {
        if !zones.iter().any(|_| true) {
            continue;
        }
        if let Some(leaf) = find_leaf(root, anchor) {
            let zone = zones[0].1;
            return insert_panel(root, leaf.id(), tab, zone, 0.35, None);
        }
    }

    let fallback = first_leaf(root);
    insert_panel(root, fallback.id(), tab, DockZone::Center, 0.5, None)
}

/// Sanitize persisted data: keep only known panels, drop empty leaves and single-child splits.
pub fn sanitize_dock(value: &serde_json::Value, known_tabs: &[String]) -> Option<DockNode> {
    fn clean(value: &serde_json::Value, known: &[String]) -> Option<DockNode> {
        let object = value.as_object()?;
        match object.get("type").and_then(|v| v.as_str()) {
            Some("leaf") => {
                let tabs: Vec<String> = object
                    .get("tabs")
                    .and_then(|v| v.as_array())
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.as_str())
                            .filter(|tab| known.iter().any(|k| k == tab))
                            .map(|tab| tab.to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                if tabs.is_empty() {
                    return None;
                }
                let active = object
                    .get("active")
                    .and_then(|v| v.as_str())
                    .filter(|active| tabs.iter().any(|t| t == *active))
                    .unwrap_or(&tabs[0])
                    .to_string();
                let id = object
                    .get("id")
                    .and_then(|v| v.as_str())
                    .filter(|id| !id.is_empty())
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| uid("leaf-"));
                Some(DockNode::Leaf { id, tabs, active })
            }
            Some("split") => {
                let a = clean(object.get("a")?, known);
                let b = clean(object.get("b")?, known);
                let (a, b) = match (a, b) {
                    (Some(a), Some(b)) => (a, b),
                    (Some(node), None) | (None, Some(node)) => return Some(node),
                    (None, None) => return None,
                };
                let dir = match object.get("dir").and_then(|v| v.as_str()) {
                    Some("column") => DockDirection::Column,
                    _ => DockDirection::Row,
                };
                let ratio = object
                    .get("ratio")
                    .and_then(|v| v.as_f64())
                    .map(|value| clamp_ratio(value as f32))
                    .unwrap_or(0.5);
                let id = object
                    .get("id")
                    .and_then(|v| v.as_str())
                    .filter(|id| !id.is_empty())
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| uid("split-"));
                Some(DockNode::Split {
                    id,
                    dir,
                    ratio,
                    a: Box::new(a),
                    b: Box::new(b),
                })
            }
            _ => None,
        }
    }

    clean(value, known_tabs)
}

/// Keep floating panels always within the viewport.
pub fn clamp_float_panel(
    panel: FloatPanel,
    viewport_width: f32,
    viewport_height: f32,
) -> FloatPanel {
    let margin = VIEWPORT_MARGIN;
    let max_width = (viewport_width - margin * 2.0).max(MIN_FLOAT_WIDTH);
    let max_height = (viewport_height - 48.0).max(MIN_FLOAT_HEIGHT);
    let width = panel.width.clamp(MIN_FLOAT_WIDTH, max_width);
    let height = panel.height.clamp(MIN_FLOAT_HEIGHT, max_height);
    FloatPanel {
        x: panel
            .x
            .clamp(margin, (viewport_width - width - margin).max(margin)),
        y: panel.y.clamp(
            TOOLBAR_HEIGHT,
            (viewport_height - height - margin).max(TOOLBAR_HEIGHT),
        ),
        width,
        height,
        ..panel
    }
}

/// Sanitize persisted floating panels (unknown tabs / duplicate tabs / non-numeric coordinates).
pub fn sanitize_floating(
    value: &serde_json::Value,
    known_tabs: &[String],
    viewport_width: f32,
    viewport_height: f32,
) -> Vec<FloatPanel> {
    let Some(items) = value.as_array() else {
        return Vec::new();
    };
    let mut seen: Vec<String> = Vec::new();
    let mut result = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let Some(object) = item.as_object() else {
            continue;
        };
        let Some(tab) = object.get("tab").and_then(|v| v.as_str()) else {
            continue;
        };
        if !known_tabs.iter().any(|k| k == tab) || seen.iter().any(|t| t == tab) {
            continue;
        }
        seen.push(tab.to_string());
        let number = |key: &str, fallback: f32| -> f32 {
            object
                .get(key)
                .and_then(|v| v.as_f64())
                .map(|v| v as f32)
                .filter(|v| v.is_finite())
                .unwrap_or(fallback)
        };
        result.push(clamp_float_panel(
            FloatPanel {
                tab: tab.to_string(),
                x: number("x", 40.0 + index as f32 * 24.0),
                y: number("y", 60.0 + index as f32 * 24.0),
                width: number("width", 300.0),
                height: number("height", 340.0),
                z: object
                    .get("z")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(index as i64 + 1) as i32,
            },
            viewport_width,
            viewport_height,
        ));
    }
    result
}

/// Resize a floating window in 8 directions and adjust its position.
///
/// Guarantees:
/// - width not less than `MIN_FLOAT_WIDTH`, height not less than `MIN_FLOAT_HEIGHT`;
/// - dragging the top / left edge keeps the relative anchor (bottom / right edge) fixed;
/// - dragging the bottom / right edge keeps the relative anchor (top / left edge) fixed;
/// - viewport boundary constraint: does not exceed viewport edges (left / right / bottom keep
///   margin, top not below the toolbar).
pub fn resize_float_rect(
    initial: FloatRect,
    dir: ResizeDirection,
    dx: f32,
    dy: f32,
    viewport_width: f32,
    viewport_height: f32,
) -> FloatRect {
    let margin = VIEWPORT_MARGIN;
    let top_limit = TOOLBAR_HEIGHT;
    let bottom_limit = (viewport_height - margin).max(top_limit + MIN_FLOAT_HEIGHT);
    let right_limit = (viewport_width - margin).max(margin + MIN_FLOAT_WIDTH);

    let mut x = initial.x;
    let mut y = initial.y;
    let mut width = initial.width;
    let mut height = initial.height;

    if dir.has('e') {
        let max_width = (right_limit - initial.x).max(MIN_FLOAT_WIDTH);
        width = (initial.width + dx).clamp(MIN_FLOAT_WIDTH, max_width);
    } else if dir.has('w') {
        let max_x = initial.x + initial.width - MIN_FLOAT_WIDTH;
        let requested_x = initial.x + dx;
        x = requested_x.clamp(margin, max_x.max(margin));
        width = initial.x + initial.width - x;
    }

    if dir.has('s') {
        let max_height = (bottom_limit - initial.y).max(MIN_FLOAT_HEIGHT);
        height = (initial.height + dy).clamp(MIN_FLOAT_HEIGHT, max_height);
    } else if dir.has('n') {
        let max_y = initial.y + initial.height - MIN_FLOAT_HEIGHT;
        let requested_y = initial.y + dy;
        y = requested_y.clamp(top_limit, max_y.max(top_limit));
        height = initial.y + initial.height - y;
    }

    FloatRect {
        x,
        y,
        width,
        height,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SnapAxis {
    Vertical,
    Horizontal,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SnapLine {
    pub axis: SnapAxis,
    pub coord: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SnapResult {
    pub x: f32,
    pub y: f32,
    pub snap_lines: Vec<SnapLine>,
}

/// Snap a floating window: align or stick to viewport edges (left, right, top, bottom) and
/// other floating panels' four edges, center lines, and cascading offsets.
pub fn snap_float_position(
    panel: FloatRect,
    others: &[FloatRect],
    viewport_width: f32,
    viewport_height: f32,
    threshold: f32,
) -> SnapResult {
    let margin = VIEWPORT_MARGIN;
    let top_limit = TOOLBAR_HEIGHT;
    let bottom_limit = (viewport_height - panel.height - 24.0).max(top_limit);
    let right_limit = (viewport_width - panel.width - margin).max(margin);

    let mut snapped_x = panel.x;
    let mut snapped_y = panel.y;
    let mut best_x = threshold + 1.0;
    let mut best_y = threshold + 1.0;
    let mut lines = Vec::new();

    let mut x_candidates: Vec<(f32, f32)> =
        vec![(margin, margin), (right_limit, viewport_width - margin)];
    let mut y_candidates: Vec<(f32, f32)> = vec![
        (top_limit, top_limit),
        (bottom_limit, viewport_height - 24.0),
    ];

    for other in others {
        x_candidates.push((other.x, other.x));
        x_candidates.push((other.x + other.width - panel.width, other.x + other.width));
        x_candidates.push((
            other.x + (other.width - panel.width) * 0.5,
            other.x + other.width * 0.5,
        ));
        x_candidates.push((other.x + 28.0, other.x + 28.0));
        x_candidates.push((other.x + other.width, other.x + other.width));
        x_candidates.push((other.x - panel.width, other.x));

        y_candidates.push((other.y, other.y));
        y_candidates.push((
            other.y + other.height - panel.height,
            other.y + other.height,
        ));
        y_candidates.push((
            other.y + (other.height - panel.height) * 0.5,
            other.y + other.height * 0.5,
        ));
        y_candidates.push((other.y + 28.0, other.y + 28.0));
        y_candidates.push((other.y + other.height, other.y + other.height));
        y_candidates.push((other.y - panel.height, other.y));
    }

    let mut chosen_x = None;
    for (target, line) in &x_candidates {
        let distance = (panel.x - target).abs();
        if distance <= threshold && distance < best_x {
            best_x = distance;
            snapped_x = *target;
            chosen_x = Some(*line);
        }
    }
    if let Some(coord) = chosen_x {
        lines.push(SnapLine {
            axis: SnapAxis::Vertical,
            coord,
        });
    }

    let mut chosen_y = None;
    for (target, line) in &y_candidates {
        let distance = (panel.y - target).abs();
        if distance <= threshold && distance < best_y {
            best_y = distance;
            snapped_y = *target;
            chosen_y = Some(*line);
        }
    }
    if let Some(coord) = chosen_y {
        lines.push(SnapLine {
            axis: SnapAxis::Horizontal,
            coord,
        });
    }

    SnapResult {
        x: snapped_x,
        y: snapped_y,
        snap_lines: lines,
    }
}

/// Persisted layout (`prisma-editor.dock.v4`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DockLayout {
    pub version: u32,
    pub dock: DockNode,
    pub floating: Vec<FloatPanel>,
}

impl DockLayout {
    pub const VERSION: u32 = 4;

    pub fn from_defaults() -> Self {
        Self {
            version: Self::VERSION,
            dock: default_dock(),
            floating: Vec::new(),
        }
    }

    /// Restore from v4 JSON; old or corrupt data falls back to the default layout.
    pub fn from_json(
        value: &serde_json::Value,
        known_tabs: &[String],
        viewport_width: f32,
        viewport_height: f32,
    ) -> Self {
        let version = value.get("version").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let dock = value
            .get("dock")
            .and_then(|dock| sanitize_dock(dock, known_tabs))
            .or_else(|| match version {
                0..=3 => value
                    .get("dock")
                    .and_then(|dock| sanitize_dock(dock, known_tabs)),
                _ => None,
            })
            .unwrap_or_else(default_dock);
        let floating = value
            .get("floating")
            .map(|floating| {
                sanitize_floating(floating, known_tabs, viewport_width, viewport_height)
            })
            .unwrap_or_default();
        Self {
            version: Self::VERSION,
            dock,
            floating,
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "version": Self::VERSION,
            "dock": self.dock,
            "floating": self.floating,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_contains_core_panels() {
        let dock = default_dock();
        let tabs = collect_tabs(&dock);
        for expected in [
            "docs",
            "hierarchy",
            "assets",
            "sceneView",
            "gameView",
            "inspector",
        ] {
            assert!(tabs.iter().any(|tab| tab == expected), "missing {expected}");
        }
        assert!(find_leaf(&dock, "inspector").is_some());
        assert!(find_leaf(&dock, "log").is_none());
    }

    #[test]
    fn remove_panel_prunes_empty_leaf_and_collapses_split() {
        let dock = split(DockDirection::Row, leaf(["a"]), leaf(["b"]), 0.5);
        let after = remove_panel(&dock, "a").expect("b still there");
        assert!(
            after.is_leaf(),
            "should collapse split when only one leaf remains"
        );
        assert_eq!(collect_tabs(&after), vec!["b".to_string()]);

        let empty = remove_panel(&leaf(["a"]), "a");
        assert!(empty.is_none());
    }

    #[test]
    fn activate_tab_only_touches_own_leaf() {
        let dock = split(DockDirection::Column, leaf(["a", "b"]), leaf(["c"]), 0.4);
        let DockNode::Split { a, .. } = &dock else {
            panic!("expected split")
        };
        let leaf_id = a.id().to_string();
        let after = activate_tab(&dock, &leaf_id, "b");
        assert_eq!(first_leaf(&after).active_tab(), Some("b"));

        // Unknown tab / unknown leaf doesn't change anything.
        assert_eq!(activate_tab(&dock, &leaf_id, "zzz"), dock);
        assert_eq!(activate_tab(&dock, "nope", "b"), dock);
    }

    #[test]
    fn insert_center_moves_tab_and_keeps_order() {
        let dock = split(DockDirection::Row, leaf(["a", "b", "c"]), leaf(["d"]), 0.5);
        let DockNode::Split { a, .. } = &dock else {
            panic!()
        };
        let left_id = a.id().to_string();

        // Drag c to the far left (index 0).
        let after = insert_panel(&dock, &left_id, "c", DockZone::Center, 0.5, Some(0));
        let DockNode::Split { a, .. } = &after else {
            panic!()
        };
        let DockNode::Leaf { tabs, active, .. } = a.as_ref() else {
            panic!()
        };
        assert_eq!(tabs, &vec!["c".to_string(), "a".into(), "b".into()]);
        assert_eq!(active, "c");

        // Drag from right leaf to left leaf: right leaf becomes empty, pruned, split collapses.
        let after = insert_panel(&dock, &left_id, "d", DockZone::Center, 0.5, None);
        assert!(
            after.is_leaf(),
            "right leaf empty should collapse into a single leaf"
        );
        assert_eq!(collect_tabs(&after), vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn insert_edge_creates_new_split() {
        let dock = leaf(["a", "b"]);
        let id = dock.id().to_string();
        let after = insert_panel(&dock, &id, "c", DockZone::Right, 0.5, None);
        let DockNode::Split { dir, a, b, .. } = &after else {
            panic!("expected split")
        };
        assert_eq!(*dir, DockDirection::Row);
        assert_eq!(collect_tabs(a), vec!["a", "b"]);
        assert_eq!(collect_tabs(b), vec!["c"]);
    }

    #[test]
    fn insert_edge_for_own_only_tab_is_noop() {
        let dock = leaf(["a"]);
        let id = dock.id().to_string();
        let after = insert_panel(&dock, &id, "a", DockZone::Right, 0.5, None);
        assert_eq!(after, dock);
    }

    #[test]
    fn dock_at_edge_wraps_whole_tree() {
        let dock = split(DockDirection::Column, leaf(["a"]), leaf(["b"]), 0.6);
        let after = dock_panel_at_edge(Some(&dock), "b", DockEdge::Left, 0.25);
        let DockNode::Split { dir, ratio, a, .. } = &after else {
            panic!()
        };
        assert_eq!(*dir, DockDirection::Row);
        assert!((ratio - 0.25).abs() < 1e-6);
        assert_eq!(collect_tabs(a), vec!["b"]);
        assert_eq!(after.id().is_empty(), false);
        // The remaining subtree still keeps a.
        assert!(collect_tabs(&after).iter().any(|t| t == "a"));
    }

    #[test]
    fn split_side_length_keeps_the_ratio_inside_both_minimums() {
        // Normal width: a ratio split with a floor.
        assert!((split_side_length(1000.0, 0.3, 60.0) - 300.0).abs() < 1e-4);
        // Ratio at the extreme: stops at the minimum, not squeezed to 0.
        assert!((split_side_length(1000.0, 0.0, 60.0) - 60.0).abs() < 1e-4);
        assert!((split_side_length(1000.0, 1.0, 60.0) - 940.0).abs() < 1e-4);
    }

    #[test]
    fn a_squeezed_nested_split_falls_back_to_half_instead_of_panicking() {
        // #498: `2by3` dragging the middle column to ~95px wide, inner Row wants `clamp(60.0, 30.0)`.
        assert!((split_side_length(95.0, 0.88, 60.0) - 47.5).abs() < 1e-4);
        assert!((split_side_length(107.0, 0.5, 50.0) - 53.5).abs() < 1e-4);
    }

    #[test]
    fn no_usable_width_or_ratio_can_panic_the_layout() {
        for usable in [0.0, 2.0, 42.0, 60.0, 95.0, 107.0, 500.0] {
            for ratio in [0.0, 0.12, 0.5, 0.88, 1.0, f32::NAN, f32::INFINITY] {
                for min in [0.0, 50.0, 60.0] {
                    let a = split_side_length(usable, ratio, min);
                    assert!(a.is_finite(), "usable={usable} ratio={ratio} min={min}");
                    assert!(
                        a >= 0.0 && a <= usable,
                        "usable={usable} ratio={ratio} min={min}"
                    );
                }
            }
        }
        // Negative / non-finite available space (parent split already flattened) must not
        // produce weird values.
        assert_eq!(split_side_length(-20.0, 0.5, 60.0), 0.0);
        assert_eq!(split_side_length(f32::NAN, 0.5, 60.0), 0.0);
    }

    #[test]
    fn set_ratio_clamps() {
        let dock = split(DockDirection::Row, leaf(["a"]), leaf(["b"]), 0.5);
        let split_id = dock.id().to_string();
        let after = set_ratio(&dock, &split_id, 5.0, MIN_RATIO, MAX_RATIO);
        let DockNode::Split { ratio, .. } = after else {
            panic!()
        };
        assert!((ratio - MAX_RATIO).abs() < 1e-6);
    }

    #[test]
    fn show_panel_reuses_anchor_and_adds_once() {
        let dock = default_dock();
        let shown = show_panel(&dock, "log");
        assert_eq!(
            collect_tabs(&shown).iter().filter(|t| *t == "log").count(),
            1
        );

        let shown_again = show_panel(&shown, "log");
        assert_eq!(
            collect_tabs(&shown_again)
                .iter()
                .filter(|t| *t == "log")
                .count(),
            1
        );

        // Utility windows default to joining the inspector leaf, not opening a new column.
        let with_settings = show_panel(&dock, "preferences");
        let inspector_leaf = find_leaf(&with_settings, "inspector").expect("inspector");
        let DockNode::Leaf { tabs, .. } = inspector_leaf else {
            panic!()
        };
        assert!(tabs.iter().any(|t| t == "preferences"));
    }

    #[test]
    fn sanitize_drops_unknown_tabs_and_bad_nodes() {
        let known = vec!["a".to_string(), "b".to_string()];
        let value = serde_json::json!({
            "type": "split",
            "id": "s",
            "dir": "column",
            "ratio": 0.3,
            "a": { "type": "leaf", "id": "l1", "tabs": ["a", "gone"], "active": "gone" },
            "b": { "type": "leaf", "id": "l2", "tabs": ["b"], "active": "b" }
        });
        let node = sanitize_dock(&value, &known).expect("valid");
        assert_eq!(collect_tabs(&node), vec!["a", "b"]);
        assert_eq!(
            first_leaf(&node).active_tab(),
            Some("a"),
            "Illegal active falls back to the first tab"
        );

        let only_unknown = serde_json::json!({ "type": "leaf", "tabs": ["zzz"] });
        assert!(sanitize_dock(&only_unknown, &known).is_none());
    }

    #[test]
    fn layout_roundtrip_through_json() {
        let layout = DockLayout::from_defaults();
        let json = layout.to_json();
        let known: Vec<String> = collect_tabs(&layout.dock);
        let restored = DockLayout::from_json(&json, &known, 1600.0, 900.0);
        assert_eq!(restored.dock, layout.dock);
        assert_eq!(json["version"], 4);
    }

    #[test]
    fn float_panels_clamp_and_sanitize() {
        let panel = clamp_float_panel(
            FloatPanel {
                tab: "log".into(),
                x: -50.0,
                y: -100.0,
                width: 10.0,
                height: 10.0,
                z: 1,
            },
            1280.0,
            800.0,
        );
        assert_eq!(panel.x, VIEWPORT_MARGIN);
        assert_eq!(panel.y, TOOLBAR_HEIGHT);
        assert_eq!(panel.width, MIN_FLOAT_WIDTH);
        assert_eq!(panel.height, MIN_FLOAT_HEIGHT);

        let known = vec!["log".to_string()];
        let value = serde_json::json!([
            { "tab": "log", "x": 100, "y": 120, "width": 400, "height": 300, "z": 2 },
            { "tab": "log", "x": 0, "y": 0 },
            { "tab": "unknown" }
        ]);
        let panels = sanitize_floating(&value, &known, 1280.0, 800.0);
        assert_eq!(panels.len(), 1);
        assert_eq!(panels[0].width, 400.0);
    }

    #[test]
    fn resize_float_rect_keeps_min_size_and_anchors() {
        let rect = FloatRect {
            x: 200.0,
            y: 200.0,
            width: 400.0,
            height: 300.0,
        };
        let wider = resize_float_rect(rect, ResizeDirection::E, 100.0, 0.0, 1280.0, 800.0);
        assert_eq!(wider.width, 500.0);
        assert_eq!(wider.x, 200.0);

        // Dragging left beyond the minimum width clamps width to the minimum, and x follows the anchor.
        let squeezed = resize_float_rect(rect, ResizeDirection::W, 500.0, 0.0, 1280.0, 800.0);
        assert_eq!(squeezed.width, MIN_FLOAT_WIDTH);
        assert_eq!(squeezed.x, 600.0 - MIN_FLOAT_WIDTH);

        // The top edge cannot cross the toolbar.
        let up = resize_float_rect(rect, ResizeDirection::N, 0.0, -500.0, 1280.0, 800.0);
        assert_eq!(up.y, TOOLBAR_HEIGHT);
        assert_eq!(up.height, 500.0 - TOOLBAR_HEIGHT);
    }

    #[test]
    fn snap_float_position_magnetizes_edges_and_other_panels() {
        let panel = FloatRect {
            x: 1300.0,
            y: 100.0,
            width: 300.0,
            height: 200.0,
        };
        let other = FloatRect {
            x: 100.0,
            y: 200.0,
            width: 200.0,
            height: 100.0,
        };
        let result = snap_float_position(panel, &[other], 1600.0, 900.0, SNAP_THRESHOLD);
        // Right edge sticks to the viewport edge: x + width = 1600 - 8.
        assert!((result.x + panel.width - (1600.0 - VIEWPORT_MARGIN)).abs() < 1e-3);
        assert!(result
            .snap_lines
            .iter()
            .any(|l| l.axis == SnapAxis::Vertical));

        // Aligns with the other panel's top edge.
        let aligned = FloatRect {
            x: 300.0,
            y: 203.0,
            width: 100.0,
            height: 100.0,
        };
        let snapped = snap_float_position(aligned, &[other], 1600.0, 900.0, SNAP_THRESHOLD);
        assert!((snapped.y - 200.0).abs() < 1e-3);
    }
}
