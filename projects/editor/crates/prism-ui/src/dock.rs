//! Docking tree, split views, and workspace layout manager.
//!
//! [`WindowLayout`] owns the main docked [`DockNode`] binary tree alongside
//! floating [`ContainerWindow`]s. Every frame it:
//! 1. Processes pointer input (`step_input`) for splitters, tab tear-off,
//!    floating window movement/resizing, and 5-zone dock compass targeting.
//! 2. Mounts the docked tree, floating glass windows, and dock preview
//!    overlays into [`UiTree`](crate::tree::UiTree) (`mount`).

use std::collections::BTreeMap;

use glam::Vec2;

use crate::behavior::UiAction;
use crate::elements::{col, row, text, ElementBox, UiContext};
use crate::icons::{hairline, macos_traffic_lights, tab_close_icon};
use crate::material::Material;
use crate::style::{Align, Color, Edges, Justify, Length, Rect};
use crate::theme::Theme;
use crate::tree::{IntoNodeKey, NodeId, NodeKey, UiInput};
use crate::window::{ContainerId, ContainerWindow, EditorWindowMeta, ResizeEdge, WindowId};

const SPLITTER_VISUAL_PX: f32 = 1.0;
const SPLITTER_HIT_PX: f32 = 6.0;
const TAB_BAR_HEIGHT: f32 = 28.0;
const FLOATING_TITLE_HEIGHT: f32 = 32.0;
const TAB_TEAR_THRESHOLD_PX: f32 = 6.0;
const RESIZE_BORDER_PX: f32 = 6.0;

/// Unique identifier of a [`SplitView`] node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SplitId(pub u64);

/// Unique identifier of a [`DockArea`] tab group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DockAreaId(pub u64);

/// Split orientation of a [`SplitView`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitAxis {
    /// Left (`first`) and Right (`second`).
    Horizontal,
    /// Top (`first`) and Bottom (`second`).
    Vertical,
}

/// Target zone when docking a window onto an existing [`DockArea`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockPosition {
    Left,
    Right,
    Top,
    Bottom,
    Tab,
}

/// Binary split container dividing space between two [`DockNode`] children.
#[derive(Clone, Debug, PartialEq)]
pub struct SplitView {
    pub id: SplitId,
    pub axis: SplitAxis,
    pub ratio: f32,
    pub first: Box<DockNode>,
    pub second: Box<DockNode>,
}

/// A tabbed container of docked [`WindowId`]s, mirroring Unity's `DockArea`.
#[derive(Clone, Debug, PartialEq)]
pub struct DockArea {
    pub id: DockAreaId,
    pub windows: Vec<WindowId>,
    pub active_tab: usize,
}

impl DockArea {
    pub fn new(id: DockAreaId, windows: Vec<WindowId>) -> Self {
        Self {
            id,
            windows,
            active_tab: 0,
        }
    }

    pub fn active_window(&self) -> Option<&WindowId> {
        self.windows.get(self.active_tab).or_else(|| self.windows.first())
    }

    pub fn set_active(&mut self, id: &WindowId) -> bool {
        if let Some(idx) = self.windows.iter().position(|w| w == id) {
            self.active_tab = idx;
            true
        } else {
            false
        }
    }
}

/// Node in a docking hierarchy.
#[derive(Clone, Debug, PartialEq)]
pub enum DockNode {
    Split(SplitView),
    Area(DockArea),
    Empty,
}

impl DockNode {
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Empty => true,
            Self::Area(area) => area.windows.is_empty(),
            Self::Split(split) => split.first.is_empty() && split.second.is_empty(),
        }
    }

    fn contains_window(&self, id: &WindowId) -> bool {
        match self {
            Self::Empty => false,
            Self::Area(area) => area.windows.iter().any(|w| w == id),
            Self::Split(split) => split.first.contains_window(id) || split.second.contains_window(id),
        }
    }

    fn focus_window(&mut self, id: &WindowId) -> bool {
        match self {
            Self::Empty => false,
            Self::Area(area) => area.set_active(id),
            Self::Split(split) => split.first.focus_window(id) || split.second.focus_window(id),
        }
    }

    fn remove_window(&mut self, id: &WindowId) -> bool {
        match self {
            Self::Empty => false,
            Self::Area(area) => {
                let Some(pos) = area.windows.iter().position(|w| w == id) else {
                    return false;
                };
                area.windows.remove(pos);
                if area.active_tab >= area.windows.len() && !area.windows.is_empty() {
                    area.active_tab = area.windows.len() - 1;
                }
                true
            }
            Self::Split(split) => split.first.remove_window(id) || split.second.remove_window(id),
        }
    }

    fn first_area_id(&self) -> Option<DockAreaId> {
        match self {
            Self::Empty => None,
            Self::Area(area) => Some(area.id),
            Self::Split(split) => split.first.first_area_id().or_else(|| split.second.first_area_id()),
        }
    }

    fn area_mut(&mut self, target: DockAreaId) -> Option<&mut DockArea> {
        match self {
            Self::Empty => None,
            Self::Area(area) if area.id == target => Some(area),
            Self::Area(_) => None,
            Self::Split(split) => split
                .first
                .area_mut(target)
                .or_else(|| split.second.area_mut(target)),
        }
    }

    fn collapse_empty(&mut self) {
        let replacement = match self {
            Self::Empty => None,
            Self::Area(area) if area.windows.is_empty() => Some(Self::Empty),
            Self::Area(_) => None,
            Self::Split(split) => {
                split.first.collapse_empty();
                split.second.collapse_empty();
                match (split.first.is_empty(), split.second.is_empty()) {
                    (true, true) => Some(Self::Empty),
                    (true, false) => Some(std::mem::replace(&mut *split.second, Self::Empty)),
                    (false, true) => Some(std::mem::replace(&mut *split.first, Self::Empty)),
                    (false, false) => None,
                }
            }
        };
        if let Some(node) = replacement {
            *self = node;
        }
    }
}

/// Stable `NodeKey` constructors for dock tabs and close buttons.
pub struct DockNodeKey;

impl DockNodeKey {
    pub fn tab(area: DockAreaId, window: &WindowId) -> NodeKey {
        format!("dock:tab:{}:{}", area.0, window.0).into_node_key()
    }

    pub fn close(window: &WindowId) -> NodeKey {
        format!("dock:close:{}", window.0).into_node_key()
    }
}

#[derive(Clone, Debug)]
struct CachedSplitHit {
    id: SplitId,
    axis: SplitAxis,
    container_rect: Rect,
    hit_rect: Rect,
}

#[derive(Clone, Debug)]
struct CachedAreaHit {
    id: DockAreaId,
    rect: Rect,
    floating: bool,
}

#[derive(Clone, Debug)]
struct CachedTabHit {
    area_id: DockAreaId,
    window_id: WindowId,
    rect: Rect,
}

#[derive(Clone, Debug)]
struct ActiveSplitDrag {
    id: SplitId,
    axis: SplitAxis,
    container_rect: Rect,
}

#[derive(Clone, Debug)]
struct ActiveWindowDrag {
    container_id: ContainerId,
    grab_offset: Vec2,
}

#[derive(Clone, Debug)]
struct ActiveWindowResize {
    container_id: ContainerId,
    edge: ResizeEdge,
    start_pointer: Vec2,
    start_rect: Rect,
}

#[derive(Clone, Debug)]
struct PendingTabDrag {
    window_id: WindowId,
    origin: Vec2,
}

#[derive(Clone, Debug)]
struct DockDragState {
    container_id: ContainerId,
    window_id: WindowId,
    grab_offset: Vec2,
    hovered_target: Option<(DockAreaId, DockPosition, Rect)>,
}

/// Top-level window & docking layout state, akin to Unity's `WindowLayout`.
#[derive(Clone, Debug)]
pub struct WindowLayout {
    pub main_root: DockNode,
    pub floating: Vec<ContainerWindow>,
    pub windows: BTreeMap<WindowId, EditorWindowMeta>,
    next_id: u64,
    cached_splits: Vec<CachedSplitHit>,
    cached_areas: Vec<CachedAreaHit>,
    cached_tabs: Vec<CachedTabHit>,
    active_split: Option<ActiveSplitDrag>,
    active_window_drag: Option<ActiveWindowDrag>,
    active_window_resize: Option<ActiveWindowResize>,
    pending_tab_drag: Option<PendingTabDrag>,
    dock_drag: Option<DockDragState>,
    last_pointer: Vec2,
}

impl Default for WindowLayout {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowLayout {
    pub fn new() -> Self {
        Self {
            main_root: DockNode::Empty,
            floating: Vec::new(),
            windows: BTreeMap::new(),
            next_id: 1,
            cached_splits: Vec::new(),
            cached_areas: Vec::new(),
            cached_tabs: Vec::new(),
            active_split: None,
            active_window_drag: None,
            active_window_resize: None,
            pending_tab_drag: None,
            dock_drag: None,
            last_pointer: Vec2::ZERO,
        }
    }

    fn close_key_for(&self, id: &WindowId) -> NodeKey {
        self.windows
            .get(id)
            .and_then(|m| m.close_key)
            .unwrap_or_else(|| DockNodeKey::close(id))
    }

    pub fn alloc_split_id(&mut self) -> SplitId {
        let id = self.next_id;
        self.next_id += 1;
        SplitId(id)
    }

    pub fn alloc_area_id(&mut self) -> DockAreaId {
        let id = self.next_id;
        self.next_id += 1;
        DockAreaId(id)
    }

    pub fn alloc_container_id(&mut self) -> ContainerId {
        let id = self.next_id;
        self.next_id += 1;
        ContainerId(id)
    }

    /// Register metadata for a logical editor window.
    pub fn register_window(&mut self, meta: EditorWindowMeta) {
        self.windows.insert(meta.id.clone(), meta);
    }

    /// Whether `id` is currently open in either `main_root` or `floating`.
    pub fn is_window_open(&self, id: &WindowId) -> bool {
        self.main_root.contains_window(id) || self.floating.iter().any(|c| c.root.contains_window(id))
    }

    /// Focus an already-open window (selecting its tab and raising its container),
    /// or open it in the first available dock area / floating window if closed.
    /// Mirrors Unity's `EditorWindow.GetWindow` / `Focus`.
    pub fn get_window(&mut self, id: &WindowId) {
        if self.focus_window(id) {
            return;
        }
        if let Some(area_id) = self.main_root.first_area_id() {
            self.dock_window(id.clone(), area_id, DockPosition::Tab);
        } else {
            self.show_floating(id, Rect::new(160.0, 120.0, 420.0, 320.0), true);
        }
    }

    /// Open `id` as a floating [`ContainerWindow`], or focus it if already open.
    pub fn show_floating(&mut self, id: &WindowId, rect: Rect, glass: bool) -> ContainerId {
        if let Some(existing) = self.floating.iter().find(|c| c.root.contains_window(id)) {
            let cid = existing.id;
            self.focus_window(id);
            return cid;
        }
        self.main_root.remove_window(id);
        self.main_root.collapse_empty();

        let area_id = self.alloc_area_id();
        let cid = self.alloc_container_id();
        let min_size = self
            .windows
            .get(id)
            .map_or(Vec2::new(220.0, 160.0), |m| m.min_size);
        let container = ContainerWindow::new(
            cid,
            rect,
            DockNode::Area(DockArea::new(area_id, vec![id.clone()])),
        )
        .with_glass(glass)
        .with_min_size(min_size);
        self.floating.push(container);
        cid
    }

    /// Focus `id` (activating its tab and raising any containing floating window).
    pub fn focus_window(&mut self, id: &WindowId) -> bool {
        if self.main_root.focus_window(id) {
            return true;
        }
        if let Some(idx) = self.floating.iter().position(|c| c.root.contains_window(id)) {
            self.floating[idx].root.focus_window(id);
            let win = self.floating.remove(idx);
            self.floating.push(win);
            return true;
        }
        false
    }

    /// Close `id` wherever it is docked or floating, collapsing empty splits.
    pub fn close_window(&mut self, id: &WindowId) {
        if self.main_root.remove_window(id) {
            self.main_root.collapse_empty();
        }
        for c in &mut self.floating {
            if c.root.remove_window(id) {
                c.root.collapse_empty();
            }
        }
        self.floating.retain(|c| !c.root.is_empty());
    }

    /// Dock `id` into `target_area` at `pos` (`Tab` or splitting one of the 4 sides).
    pub fn dock_window(&mut self, id: WindowId, target_area: DockAreaId, pos: DockPosition) {
        self.close_window(&id);

        if pos == DockPosition::Tab {
            if let Some(area) = self.find_area_mut(target_area) {
                area.windows.push(id);
                area.active_tab = area.windows.len() - 1;
                return;
            }
        }

        let new_area_id = self.alloc_area_id();
        let new_split_id = self.alloc_split_id();
        let new_node = DockNode::Area(DockArea::new(new_area_id, vec![id]));

        if !Self::split_area_in_tree(
            &mut self.main_root,
            target_area,
            pos,
            new_split_id,
            new_node.clone(),
        ) {
            // Fallback: replace empty root
            if self.main_root.is_empty() {
                self.main_root = new_node;
            }
        }
    }

    /// Tear off `id` from its current dock area into a new floating [`ContainerWindow`].
    pub fn undock_window(&mut self, id: &WindowId, rect: Rect, glass: bool) -> Option<ContainerId> {
        if !self.is_window_open(id) {
            return None;
        }
        Some(self.show_floating(id, rect, glass))
    }

    /// Process pointer input and [`UiAction`]s for splitters, tabs, floating
    /// windows, tear-off, and 5-zone dock targeting.
    pub fn step_input(&mut self, workspace: Rect, input: &UiInput, actions: &[UiAction]) {
        self.last_pointer = input.pointer_pos;
        for action in actions {
            if let UiAction::Clicked(key) = action {
                for hit in &self.cached_tabs.clone() {
                    if *key == DockNodeKey::tab(hit.area_id, &hit.window_id) {
                        self.focus_window(&hit.window_id);
                    } else if *key == self.close_key_for(&hit.window_id) {
                        self.close_window(&hit.window_id);
                    }
                }
            }
        }

        if input.primary_just_pressed {
            // 1. Check floating windows from top to bottom (Z-order).
            let mut handled = false;
            for idx in (0..self.floating.len()).rev() {
                let win = &self.floating[idx];
                let outer = Rect::new(
                    win.rect.x - RESIZE_BORDER_PX,
                    win.rect.y - RESIZE_BORDER_PX,
                    win.rect.w + RESIZE_BORDER_PX * 2.0,
                    win.rect.h + RESIZE_BORDER_PX * 2.0,
                );
                if !outer.contains(input.pointer_pos) {
                    continue;
                }
                let cid = win.id;
                let rect = win.rect;
                if let Some(edge) = hit_resize_edge(rect, input.pointer_pos) {
                    self.bring_container_to_front(cid);
                    self.active_window_resize = Some(ActiveWindowResize {
                        container_id: cid,
                        edge,
                        start_pointer: input.pointer_pos,
                        start_rect: rect,
                    });
                    handled = true;
                    break;
                }
                let title_rect = Rect::new(rect.x, rect.y, rect.w, FLOATING_TITLE_HEIGHT);
                if title_rect.contains(input.pointer_pos) {
                    // Check if clicking a tab for tear-off/docking
                    if let Some(tab) = self
                        .cached_tabs
                        .iter()
                        .find(|t| t.rect.contains(input.pointer_pos))
                        .cloned()
                    {
                        self.focus_window(&tab.window_id);
                        self.dock_drag = Some(DockDragState {
                            container_id: cid,
                            window_id: tab.window_id,
                            grab_offset: input.pointer_pos - Vec2::new(rect.x, rect.y),
                            hovered_target: None,
                        });
                    } else {
                        self.bring_container_to_front(cid);
                        self.active_window_drag = Some(ActiveWindowDrag {
                            container_id: cid,
                            grab_offset: input.pointer_pos - Vec2::new(rect.x, rect.y),
                        });
                    }
                    handled = true;
                    break;
                }
                self.bring_container_to_front(cid);
                handled = true;
                break;
            }

            // 2. Check docked splitters and tabs.
            if !handled {
                if let Some(split) = self
                    .cached_splits
                    .iter()
                    .find(|s| s.hit_rect.contains(input.pointer_pos))
                    .cloned()
                {
                    self.active_split = Some(ActiveSplitDrag {
                        id: split.id,
                        axis: split.axis,
                        container_rect: split.container_rect,
                    });
                } else if let Some(tab) = self
                    .cached_tabs
                    .iter()
                    .find(|t| t.rect.contains(input.pointer_pos))
                    .cloned()
                {
                    self.focus_window(&tab.window_id);
                    self.pending_tab_drag = Some(PendingTabDrag {
                        window_id: tab.window_id,
                        origin: input.pointer_pos,
                    });
                }
            }
        }

        if input.primary_down {
            if let Some(drag) = &self.active_split {
                let ratio = match drag.axis {
                    SplitAxis::Horizontal => {
                        (input.pointer_pos.x - drag.container_rect.x)
                            / drag.container_rect.w.max(1.0)
                    }
                    SplitAxis::Vertical => {
                        (input.pointer_pos.y - drag.container_rect.y)
                            / drag.container_rect.h.max(1.0)
                    }
                };
                Self::set_split_ratio(&mut self.main_root, drag.id, ratio.clamp(0.08, 0.92));
            }

            if let Some(drag) = &self.active_window_drag {
                if let Some(win) = self.floating.iter_mut().find(|w| w.id == drag.container_id) {
                    win.rect.x = input.pointer_pos.x - drag.grab_offset.x;
                    win.rect.y = input.pointer_pos.y - drag.grab_offset.y;
                    win.clamp_to_bounds(workspace);
                }
            }

            if let Some(resize) = &self.active_window_resize {
                if let Some(win) = self.floating.iter_mut().find(|w| w.id == resize.container_id) {
                    apply_resize(win, resize, input.pointer_pos);
                    win.clamp_to_bounds(workspace);
                }
            }

            if let Some(pending) = &self.pending_tab_drag {
                if input.pointer_pos.distance(pending.origin) >= TAB_TEAR_THRESHOLD_PX {
                    let window_id = pending.window_id.clone();
                    self.pending_tab_drag = None;
                    let rect = Rect::new(
                        (input.pointer_pos.x - 120.0).max(workspace.x),
                        (input.pointer_pos.y - 12.0).max(workspace.y),
                        340.0,
                        260.0,
                    );
                    let cid = self.show_floating(&window_id, rect, true);
                    self.dock_drag = Some(DockDragState {
                        container_id: cid,
                        window_id,
                        grab_offset: Vec2::new(120.0, 12.0),
                        hovered_target: None,
                    });
                }
            }

            if let Some(mut drag) = self.dock_drag.take() {
                if let Some(win) = self.floating.iter_mut().find(|w| w.id == drag.container_id) {
                    win.rect.x = input.pointer_pos.x - drag.grab_offset.x;
                    win.rect.y = input.pointer_pos.y - drag.grab_offset.y;
                    win.clamp_to_bounds(workspace);
                }
                drag.hovered_target = self.hit_dock_target(drag.container_id, input.pointer_pos);
                self.dock_drag = Some(drag);
            }
        }

        if input.primary_just_released {
            if let Some(drag) = self.dock_drag.take() {
                if let Some((target_area, pos, _)) = drag.hovered_target {
                    self.dock_window(drag.window_id, target_area, pos);
                }
            }
            self.active_split = None;
            self.active_window_drag = None;
            self.active_window_resize = None;
            self.pending_tab_drag = None;
        }
    }

    /// Mount the docked tree, floating windows, and active dock compass/preview
    /// into `UiTree`.
    pub fn mount(
        &mut self,
        ctx: &mut UiContext,
        overlay_root: NodeId,
        theme: &Theme,
        workspace: Rect,
        draw_content: &mut dyn FnMut(&WindowId, &mut UiContext),
    ) {
        self.cached_splits.clear();
        self.cached_areas.clear();
        self.cached_tabs.clear();

        let root_node = self.main_root.clone();
        self.mount_node(ctx, theme, &root_node, workspace, false, draw_content);

        // Floating windows mount on the overlay layer in Z-order.
        let floating = self.floating.clone();
        ctx.with_parent(overlay_root, |ctx| {
            for win in &floating {
                let mut container = col()
                    .absolute(win.rect.x, win.rect.y)
                    .size(Length::Px(win.rect.w), Length::Px(win.rect.h))
                    .radius(8.0)
                    .border(1.0, theme.border);
                if win.glass {
                    let sigma = win.blur_sigma.unwrap_or(theme.glass_blur_sigma);
                    let tint = win
                        .tint_alpha
                        .map_or(theme.glass_tint, |a| theme.glass_tint.with_alpha(a));
                    container = container
                        .bg(tint)
                        .material(Material::frosted_glass(sigma).with_radius(8.0));
                } else {
                    container = container.bg(theme.panel_raised);
                }
                let cid = ctx.mount(container);
                ctx.with_parent(cid, |ctx| {
                    self.mount_node(
                        ctx,
                        theme,
                        &win.root,
                        Rect::new(0.0, 0.0, win.rect.w, win.rect.h),
                        true,
                        draw_content,
                    );
                });
            }

            if let Some(drag) = &self.dock_drag {
                if let Some((area_id, pos, preview)) = drag.hovered_target {
                    ctx.mount(
                        ElementBox::default()
                            .absolute(preview.x, preview.y)
                            .size(Length::Px(preview.w), Length::Px(preview.h))
                            .radius(4.0)
                            .border(1.5, theme.accent)
                            .bg(theme.accent.with_alpha(0.18)),
                    );
                    if let Some(area) = self.cached_areas.iter().find(|a| a.id == area_id) {
                        mount_dock_compass(ctx, theme, area.rect, pos);
                    }
                }
            }
        });
    }

    fn mount_node(
        &mut self,
        ctx: &mut UiContext,
        theme: &Theme,
        node: &DockNode,
        rect: Rect,
        in_floating: bool,
        draw_content: &mut dyn FnMut(&WindowId, &mut UiContext),
    ) {
        match node {
            DockNode::Empty => {}
            DockNode::Split(split) => match split.axis {
                SplitAxis::Horizontal => {
                    let first_w = ((rect.w - SPLITTER_VISUAL_PX) * split.ratio).max(0.0);
                    let second_w = (rect.w - SPLITTER_VISUAL_PX - first_w).max(0.0);
                    let first_rect = Rect::new(rect.x, rect.y, first_w, rect.h);
                    let second_rect = Rect::new(
                        rect.x + first_w + SPLITTER_VISUAL_PX,
                        rect.y,
                        second_w,
                        rect.h,
                    );
                    let hit_rect = Rect::new(
                        rect.x + first_w - SPLITTER_HIT_PX * 0.5,
                        rect.y,
                        SPLITTER_HIT_PX,
                        rect.h,
                    );
                    self.cached_splits.push(CachedSplitHit {
                        id: split.id,
                        axis: split.axis,
                        container_rect: rect,
                        hit_rect,
                    });
                    self.mount_node(ctx, theme, &split.first, first_rect, in_floating, draw_content);
                    ctx.mount(
                        ElementBox::default()
                            .absolute(rect.x + first_w, rect.y)
                            .size(Length::Px(SPLITTER_VISUAL_PX), Length::Px(rect.h))
                            .bg(theme.border_subtle),
                    );
                    self.mount_node(ctx, theme, &split.second, second_rect, in_floating, draw_content);
                }
                SplitAxis::Vertical => {
                    let first_h = ((rect.h - SPLITTER_VISUAL_PX) * split.ratio).max(0.0);
                    let second_h = (rect.h - SPLITTER_VISUAL_PX - first_h).max(0.0);
                    let first_rect = Rect::new(rect.x, rect.y, rect.w, first_h);
                    let second_rect = Rect::new(
                        rect.x,
                        rect.y + first_h + SPLITTER_VISUAL_PX,
                        rect.w,
                        second_h,
                    );
                    let hit_rect = Rect::new(
                        rect.x,
                        rect.y + first_h - SPLITTER_HIT_PX * 0.5,
                        rect.w,
                        SPLITTER_HIT_PX,
                    );
                    self.cached_splits.push(CachedSplitHit {
                        id: split.id,
                        axis: split.axis,
                        container_rect: rect,
                        hit_rect,
                    });
                    self.mount_node(ctx, theme, &split.first, first_rect, in_floating, draw_content);
                    ctx.mount(
                        ElementBox::default()
                            .absolute(rect.x, rect.y + first_h)
                            .size(Length::Px(rect.w), Length::Px(SPLITTER_VISUAL_PX))
                            .bg(theme.border_subtle),
                    );
                    self.mount_node(ctx, theme, &split.second, second_rect, in_floating, draw_content);
                }
            },
            DockNode::Area(area) => {
                self.cached_areas.push(CachedAreaHit {
                    id: area.id,
                    rect,
                    floating: in_floating,
                });

                let panel = if in_floating {
                    col()
                        .size(Length::Percent(100.0), Length::Percent(100.0))
                } else {
                    col()
                        .absolute(rect.x, rect.y)
                        .size(Length::Px(rect.w), Length::Px(rect.h))
                        .bg(theme.panel)
                };
                let area_node = ctx.mount(panel);
                ctx.with_parent(area_node, |ctx| {
                    if in_floating {
                        // macOS-style titlebar: traffic lights on the left, centered title
                        // (or compact pill tabs when multiple panels are grouped), 1px hairline.
                        let active_win = area.active_window().cloned();
                        let close_key = active_win
                            .as_ref()
                            .map_or_else(|| DockNodeKey::close(&"".into()), |w| self.close_key_for(w));
                        let lights_rect = Rect::new(rect.x + 8.0, rect.y + 4.0, 62.0, 24.0);
                        let show_glyphs = lights_rect.contains(self.last_pointer);

                        let titlebar = ctx.mount(
                            row()
                                .width(Length::Percent(100.0))
                                .height(Length::Px(FLOATING_TITLE_HEIGHT))
                                .padding(Edges::symmetric(12.0, 0.0))
                                .align(Align::Center)
                                .justify(Justify::SpaceBetween)
                                .child(macos_traffic_lights(close_key, None, None, show_glyphs)),
                        );
                        ctx.with_parent(titlebar, |ctx| {
                            if area.windows.len() <= 1 {
                                let title = active_win
                                    .as_ref()
                                    .and_then(|w| self.windows.get(w))
                                    .map_or_else(
                                        || active_win.as_ref().map_or("", |w| w.as_str()),
                                        |m| m.title.as_str(),
                                    );
                                if let Some(w) = &active_win {
                                    self.cached_tabs.push(CachedTabHit {
                                        area_id: area.id,
                                        window_id: w.clone(),
                                        rect: Rect::new(
                                            rect.x + 70.0,
                                            rect.y,
                                            (rect.w - 140.0).max(0.0),
                                            FLOATING_TITLE_HEIGHT,
                                        ),
                                    });
                                }
                                ctx.mount(text(theme.text_strong, theme.font_size, title));
                            } else {
                                let pills = ctx.mount(
                                    row()
                                        .gap(4.0)
                                        .align(Align::Center)
                                        .padding(Edges::symmetric(3.0, 2.0))
                                        .radius(6.0)
                                        .bg(theme.chrome.with_alpha(0.6)),
                                );
                                let mut tab_x = rect.x + rect.w * 0.25;
                                ctx.with_parent(pills, |ctx| {
                                    for (idx, win_id) in area.windows.iter().enumerate() {
                                        let is_active = idx == area.active_tab;
                                        let title = self
                                            .windows
                                            .get(win_id)
                                            .map_or(win_id.as_str(), |m| m.title.as_str());
                                        let tab_w = (title.len() as f32 * 7.0 + 18.0).max(52.0);
                                        self.cached_tabs.push(CachedTabHit {
                                            area_id: area.id,
                                            window_id: win_id.clone(),
                                            rect: Rect::new(
                                                tab_x,
                                                rect.y + 4.0,
                                                tab_w,
                                                FLOATING_TITLE_HEIGHT - 8.0,
                                            ),
                                        });
                                        tab_x += tab_w + 4.0;
                                        let bg = if is_active {
                                            theme.panel_raised
                                        } else {
                                            Color::TRANSPARENT
                                        };
                                        let fg = if is_active {
                                            theme.text_strong
                                        } else {
                                            theme.muted
                                        };
                                        ctx.mount(
                                            row()
                                                .key(DockNodeKey::tab(area.id, win_id))
                                                .padding(Edges::symmetric(8.0, 2.0))
                                                .radius(4.0)
                                                .align(Align::Center)
                                                .bg(bg)
                                                .child(text(fg, theme.font_size_small, title)),
                                        );
                                    }
                                });
                            }
                            // Right balance spacer matching the 52px traffic-light cluster
                            // so the center title/pills sit at the true window midpoint.
                            ctx.mount(ElementBox::default().size(Length::Px(52.0), Length::Px(1.0)));
                        });
                        ctx.mount(hairline(theme.border_subtle.with_alpha(0.45)));
                    } else {
                        // Docked Tab bar
                        let bar = ctx.mount(
                            row()
                                .width(Length::Percent(100.0))
                                .height(Length::Px(TAB_BAR_HEIGHT))
                                .padding(Edges::symmetric(6.0, 3.0))
                                .gap(4.0)
                                .align(Align::Center)
                                .bg(theme.chrome),
                        );
                        let mut tab_x = rect.x + 6.0;
                        ctx.with_parent(bar, |ctx| {
                            for (idx, win_id) in area.windows.iter().enumerate() {
                                let is_active = idx == area.active_tab;
                                let meta = self.windows.get(win_id);
                                let title = meta.map_or(win_id.as_str(), |m| m.title.as_str());
                                let closable = meta.map_or(true, |m| m.closable);
                                let tab_w = (title.len() as f32 * 7.5
                                    + if closable { 30.0 } else { 18.0 })
                                .max(64.0);

                                self.cached_tabs.push(CachedTabHit {
                                    area_id: area.id,
                                    window_id: win_id.clone(),
                                    rect: Rect::new(tab_x, rect.y + 3.0, tab_w, TAB_BAR_HEIGHT - 6.0),
                                });
                                tab_x += tab_w + 4.0;

                                let tab_bg = if is_active {
                                    theme.panel_alt
                                } else {
                                    Color::TRANSPARENT
                                };
                                let text_color = if is_active {
                                    theme.text_strong
                                } else {
                                    theme.muted
                                };
                                let tab_row = ctx.mount(
                                    row()
                                        .key(DockNodeKey::tab(area.id, win_id))
                                        .height(Length::Px(TAB_BAR_HEIGHT - 6.0))
                                        .padding(Edges::symmetric(8.0, 2.0))
                                        .gap(6.0)
                                        .radius(5.0)
                                        .align(Align::Center)
                                        .justify(Justify::SpaceBetween)
                                        .bg(tab_bg)
                                        .child(text(text_color, theme.font_size_small, title)),
                                );
                                if closable {
                                    let close_key = self.close_key_for(win_id);
                                    ctx.with_parent(tab_row, |ctx| {
                                        ctx.mount(tab_close_icon(close_key, theme));
                                    });
                                }
                            }
                        });
                        ctx.mount(hairline(theme.border_subtle));
                    }

                    // Active window body
                    if let Some(active) = area.active_window().cloned() {
                        let body = ctx.mount(
                            col()
                                .width(Length::Percent(100.0))
                                .grow(1.0),
                        );
                        ctx.with_parent(body, |ctx| {
                            draw_content(&active, ctx);
                        });
                    }
                });
            }
        }
    }

    fn bring_container_to_front(&mut self, id: ContainerId) {
        if let Some(idx) = self.floating.iter().position(|c| c.id == id) {
            if idx + 1 < self.floating.len() {
                let win = self.floating.remove(idx);
                self.floating.push(win);
            }
        }
    }

    fn find_area_mut(&mut self, target: DockAreaId) -> Option<&mut DockArea> {
        if let Some(area) = self.main_root.area_mut(target) {
            return Some(area);
        }
        for c in &mut self.floating {
            if let Some(area) = c.root.area_mut(target) {
                return Some(area);
            }
        }
        None
    }

    fn split_area_in_tree(
        node: &mut DockNode,
        target: DockAreaId,
        pos: DockPosition,
        split_id: SplitId,
        new_node: DockNode,
    ) -> bool {
        match node {
            DockNode::Empty => false,
            DockNode::Area(area) if area.id == target => {
                let existing = std::mem::replace(node, DockNode::Empty);
                let (axis, first, second) = match pos {
                    DockPosition::Left => (SplitAxis::Horizontal, new_node, existing),
                    DockPosition::Right => (SplitAxis::Horizontal, existing, new_node),
                    DockPosition::Top => (SplitAxis::Vertical, new_node, existing),
                    DockPosition::Bottom => (SplitAxis::Vertical, existing, new_node),
                    DockPosition::Tab => return false,
                };
                *node = DockNode::Split(SplitView {
                    id: split_id,
                    axis,
                    ratio: 0.5,
                    first: Box::new(first),
                    second: Box::new(second),
                });
                true
            }
            DockNode::Area(_) => false,
            DockNode::Split(split) => {
                Self::split_area_in_tree(&mut split.first, target, pos, split_id, new_node.clone())
                    || Self::split_area_in_tree(&mut split.second, target, pos, split_id, new_node)
            }
        }
    }

    fn set_split_ratio(node: &mut DockNode, target: SplitId, ratio: f32) -> bool {
        match node {
            DockNode::Empty | DockNode::Area(_) => false,
            DockNode::Split(split) => {
                if split.id == target {
                    split.ratio = ratio;
                    return true;
                }
                Self::set_split_ratio(&mut split.first, target, ratio)
                    || Self::set_split_ratio(&mut split.second, target, ratio)
            }
        }
    }

    fn hit_dock_target(
        &self,
        dragged_container: ContainerId,
        pointer: Vec2,
    ) -> Option<(DockAreaId, DockPosition, Rect)> {
        let dragged_area = self
            .floating
            .iter()
            .find(|c| c.id == dragged_container)
            .and_then(|c| c.root.first_area_id());

        for area in &self.cached_areas {
            if area.floating || Some(area.id) == dragged_area || !area.rect.contains(pointer) {
                continue;
            }
            let r = area.rect;
            let cx = r.x + r.w * 0.5;
            let cy = r.y + r.h * 0.5;
            let dx = (pointer.x - cx) / (r.w * 0.5).max(1.0);
            let dy = (pointer.y - cy) / (r.h * 0.5).max(1.0);
            let (pos, preview) = if dx.abs() < 0.3 && dy.abs() < 0.3 {
                (DockPosition::Tab, r)
            } else if dx.abs() > dy.abs() {
                if dx < 0.0 {
                    (DockPosition::Left, Rect::new(r.x, r.y, r.w * 0.5, r.h))
                } else {
                    (
                        DockPosition::Right,
                        Rect::new(r.x + r.w * 0.5, r.y, r.w * 0.5, r.h),
                    )
                }
            } else if dy < 0.0 {
                (DockPosition::Top, Rect::new(r.x, r.y, r.w, r.h * 0.5))
            } else {
                (
                    DockPosition::Bottom,
                    Rect::new(r.x, r.y + r.h * 0.5, r.w, r.h * 0.5),
                )
            };
            return Some((area.id, pos, preview));
        }
        None
    }
}

fn mount_dock_compass(ctx: &mut UiContext, theme: &Theme, target: Rect, active: DockPosition) {
    let cx = target.x + target.w * 0.5;
    let cy = target.y + target.h * 0.5;
    let box_sz = 26.0;
    let gap = 30.0;
    let zones = [
        (DockPosition::Tab, cx - box_sz * 0.5, cy - box_sz * 0.5),
        (DockPosition::Left, cx - box_sz * 0.5 - gap, cy - box_sz * 0.5),
        (DockPosition::Right, cx - box_sz * 0.5 + gap, cy - box_sz * 0.5),
        (DockPosition::Top, cx - box_sz * 0.5, cy - box_sz * 0.5 - gap),
        (DockPosition::Bottom, cx - box_sz * 0.5, cy - box_sz * 0.5 + gap),
    ];
    for (zone, x, y) in zones {
        let is_active = zone == active;
        let bg = if is_active {
            theme.accent
        } else {
            theme.panel_raised.with_alpha(0.85)
        };
        ctx.mount(
            ElementBox::default()
                .absolute(x, y)
                .size(Length::Px(box_sz), Length::Px(box_sz))
                .radius(4.0)
                .border(1.0, if is_active { theme.accent } else { theme.border_strong })
                .bg(bg),
        );
    }
}

fn hit_resize_edge(rect: Rect, p: Vec2) -> Option<ResizeEdge> {
    let left = (p.x - rect.x).abs() <= RESIZE_BORDER_PX;
    let right = (p.x - (rect.x + rect.w)).abs() <= RESIZE_BORDER_PX;
    let top = (p.y - rect.y).abs() <= RESIZE_BORDER_PX;
    let bottom = (p.y - (rect.y + rect.h)).abs() <= RESIZE_BORDER_PX;
    match (left, right, top, bottom) {
        (true, _, true, _) => Some(ResizeEdge::TopLeft),
        (_, true, true, _) => Some(ResizeEdge::TopRight),
        (true, _, _, true) => Some(ResizeEdge::BottomLeft),
        (_, true, _, true) => Some(ResizeEdge::BottomRight),
        (true, _, _, _) => Some(ResizeEdge::Left),
        (_, true, _, _) => Some(ResizeEdge::Right),
        (_, _, true, _) => Some(ResizeEdge::Top),
        (_, _, _, true) => Some(ResizeEdge::Bottom),
        _ => None,
    }
}

fn apply_resize(win: &mut ContainerWindow, resize: &ActiveWindowResize, pointer: Vec2) {
    let delta = pointer - resize.start_pointer;
    let mut r = resize.start_rect;
    let min = win.min_size;
    match resize.edge {
        ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft => {
            let new_w = (r.w - delta.x).max(min.x);
            r.x += r.w - new_w;
            r.w = new_w;
        }
        ResizeEdge::Right | ResizeEdge::TopRight | ResizeEdge::BottomRight => {
            r.w = (r.w + delta.x).max(min.x);
        }
        _ => {}
    }
    match resize.edge {
        ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight => {
            let new_h = (r.h - delta.y).max(min.y);
            r.y += r.h - new_h;
            r.h = new_h;
        }
        ResizeEdge::Bottom | ResizeEdge::BottomLeft | ResizeEdge::BottomRight => {
            r.h = (r.h + delta.y).max(min.y);
        }
        _ => {}
    }
    win.rect = r;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::UiTree;

    fn sample_layout() -> (WindowLayout, DockAreaId, DockAreaId, DockAreaId) {
        let mut layout = WindowLayout::new();
        layout.register_window(EditorWindowMeta::new("hierarchy", "Hierarchy"));
        layout.register_window(EditorWindowMeta::new("scene", "Scene"));
        layout.register_window(EditorWindowMeta::new("inspector", "Inspector"));

        let left = layout.alloc_area_id();
        let center = layout.alloc_area_id();
        let right = layout.alloc_area_id();
        let split_r = layout.alloc_split_id();
        let split_l = layout.alloc_split_id();

        layout.main_root = DockNode::Split(SplitView {
            id: split_l,
            axis: SplitAxis::Horizontal,
            ratio: 0.2,
            first: Box::new(DockNode::Area(DockArea::new(left, vec!["hierarchy".into()]))),
            second: Box::new(DockNode::Split(SplitView {
                id: split_r,
                axis: SplitAxis::Horizontal,
                ratio: 0.7,
                first: Box::new(DockNode::Area(DockArea::new(center, vec!["scene".into()]))),
                second: Box::new(DockNode::Area(DockArea::new(right, vec!["inspector".into()]))),
            })),
        });
        (layout, left, center, right)
    }

    #[test]
    fn docking_into_tab_and_split_updates_tree() {
        let (mut layout, _left, center, _right) = sample_layout();
        layout.register_window(EditorWindowMeta::new("console", "Console"));

        layout.dock_window("console".into(), center, DockPosition::Bottom);
        assert!(layout.is_window_open(&"console".into()));
        assert!(layout.is_window_open(&"scene".into()));

        layout.close_window(&"console".into());
        assert!(!layout.is_window_open(&"console".into()));
        assert!(layout.is_window_open(&"scene".into()));
    }

    #[test]
    fn undocking_to_floating_and_redocking_collapses_empty_splits() {
        let (mut layout, left, center, _right) = sample_layout();
        let cid = layout
            .undock_window(&"hierarchy".into(), Rect::new(80.0, 80.0, 300.0, 240.0), true)
            .expect("hierarchy undocked");
        assert_eq!(layout.floating.len(), 1);
        assert_eq!(layout.floating[0].id, cid);
        assert!(!layout.main_root.contains_window(&"hierarchy".into()));

        layout.dock_window("hierarchy".into(), center, DockPosition::Tab);
        assert!(layout.floating.is_empty());
        assert!(layout.main_root.contains_window(&"hierarchy".into()));
        let _ = left;
    }

    #[test]
    fn splitter_drag_adjusts_ratio() {
        let (mut layout, _left, _center, _right) = sample_layout();
        let mut tree = UiTree::new();
        let theme = Theme::dark();
        let workspace = Rect::new(0.0, 0.0, 1000.0, 600.0);

        tree.begin_frame();
        let mut ctx = UiContext::new(&mut tree);
        let overlay = ctx.tree.overlay_root(Vec2::new(1000.0, 600.0));
        layout.mount(&mut ctx, overlay, &theme, workspace, &mut |_, _| {});

        let splitter_x = layout.cached_splits[0].hit_rect.x + 3.0;
        let press = UiInput {
            pointer_pos: Vec2::new(splitter_x, 300.0),
            primary_down: true,
            primary_just_pressed: true,
            ..Default::default()
        };
        layout.step_input(workspace, &press, &[]);
        let drag = UiInput {
            pointer_pos: Vec2::new(400.0, 300.0),
            primary_down: true,
            ..Default::default()
        };
        layout.step_input(workspace, &drag, &[]);

        if let DockNode::Split(split) = &layout.main_root {
            assert!((split.ratio - 0.4).abs() < 0.02, "got ratio {}", split.ratio);
        } else {
            panic!("expected root split");
        }
    }
}
