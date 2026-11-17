//! Not-Enough-Items (NEI) palette, 9-slot hotbar, and aimed-at cell inspector.
//!
//! Uses `GameWorld::draw_quad` for pixel-aligned panels/swatches and
//! `prism_scene::ui::Text` ECS entities for crisp text overlays without
//! introducing any external UI framework.

use prism_game_api::{keys, EntityHandle, GameWorld, InputSnapshot, UiQuad};
use prism_scene::components::{Active, Name};
use prism_scene::ui::{
    Anchors, Canvas, CanvasScaleMode, Margin, Node, Pivot, Style, Text, TextAlign,
};

use crate::catalog::{format_cell_inspector, CatalogEntry, CatalogKind, CATALOG};
use crate::grid::CellGrid;
use crate::tags::PhysicalCell;

pub const HOTBAR_SLOTS: usize = 9;
const NEI_COLS: usize = 4;
const NEI_ROWS: usize = 5;
const NEI_PER_PAGE: usize = NEI_COLS * NEI_ROWS;

/// Category filter for the NEI palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogFilter {
    All,
    Items,
    Entities,
}

impl CatalogFilter {
    pub const fn next(self) -> Self {
        match self {
            Self::All => Self::Items,
            Self::Items => Self::Entities,
            Self::Entities => Self::All,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "ALL",
            Self::Items => "ITEM",
            Self::Entities => "ENTITY",
        }
    }

    pub fn matches(self, entry: &CatalogEntry) -> bool {
        match self {
            Self::All => true,
            Self::Items => entry.kind == CatalogKind::Item,
            Self::Entities => entry.kind == CatalogKind::Entity,
        }
    }
}

/// Actions emitted by NEI/hotbar input handling for the sandbox grid.
#[derive(Debug, Clone, Copy, Default)]
pub struct UiActions {
    pub place_at: Option<(usize, usize, PhysicalCell)>,
    pub erase_at: Option<(usize, usize)>,
}

/// Persistent NEI palette, hotbar, and aimed-at inspector state.
pub struct NeiHudState {
    pub palette_open: bool,
    pub filter: CatalogFilter,
    pub page: usize,
    pub hotbar: [usize; HOTBAR_SLOTS],
    pub selected_slot: usize,
    pub hovered_catalog: Option<usize>,
    prev_key_bits: u32,
    inspector_entity: Option<EntityHandle>,
    palette_header_entity: Option<EntityHandle>,
    hotbar_label_entity: Option<EntityHandle>,
    last_generation: Option<u64>,
}

impl Default for NeiHudState {
    fn default() -> Self {
        Self::new()
    }
}

impl NeiHudState {
    pub fn new() -> Self {
        let mut hotbar = [0usize; HOTBAR_SLOTS];
        for (slot, entry_idx) in hotbar.iter_mut().enumerate() {
            *entry_idx = slot % CATALOG.len();
        }
        Self {
            palette_open: true,
            filter: CatalogFilter::All,
            page: 0,
            hotbar,
            selected_slot: 0,
            hovered_catalog: None,
            prev_key_bits: 0,
            inspector_entity: None,
            palette_header_entity: None,
            hotbar_label_entity: None,
            last_generation: None,
        }
    }

    /// Returns the currently selected `CatalogEntry` from the active hotbar slot.
    pub fn selected_entry(&self) -> &'static CatalogEntry {
        &CATALOG[self.hotbar[self.selected_slot] % CATALOG.len()]
    }

    /// Collects catalog indices matching the current `CatalogFilter`.
    pub fn filtered_indices(&self) -> Vec<usize> {
        CATALOG
            .iter()
            .enumerate()
            .filter_map(|(idx, entry)| self.filter.matches(entry).then_some(idx))
            .collect()
    }

    pub fn page_count(&self, total: usize) -> usize {
        total.max(1).div_ceil(NEI_PER_PAGE)
    }

    /// Processes keyboard & cursor interactions for the NEI palette, hotbar, and world grid.
    pub fn update_input(
        &mut self,
        input: &InputSnapshot,
        screen: [f32; 2],
        grid_size: (usize, usize),
        cell_px: f32,
        probe_x: &mut usize,
        probe_y: &mut usize,
    ) -> UiActions {
        let (grid_width, grid_height) = grid_size;
        let mut actions = UiActions::default();

        let key_bits = collect_key_bits(input);
        let pressed = key_bits & !self.prev_key_bits;
        self.prev_key_bits = key_bits;

        if (pressed & (1 << 0)) != 0 {
            self.palette_open = !self.palette_open;
        }
        if (pressed & (1 << 1)) != 0 {
            self.filter = self.filter.next();
            self.page = 0;
        }
        if (pressed & (1 << 2)) != 0 {
            let pages = self.page_count(self.filtered_indices().len());
            self.page = (self.page + 1) % pages;
        }
        for slot in 0..HOTBAR_SLOTS {
            if (pressed & (1 << (3 + slot))) != 0 {
                self.selected_slot = slot;
            }
        }

        let layout = HudLayout::compute(screen);
        let cursor = input.cursor;
        let filtered = self.filtered_indices();
        let pages = self.page_count(filtered.len());
        if self.page >= pages {
            self.page = pages - 1;
        }

        self.hovered_catalog = None;
        let mut consumed_cursor = false;

        // Check hotbar slot hover/click.
        for slot in 0..HOTBAR_SLOTS {
            let rect = layout.hotbar_slot_rect(slot);
            if point_in_rect(cursor, rect) {
                consumed_cursor = true;
                self.hovered_catalog = Some(self.hotbar[slot]);
                if input.left_clicked {
                    self.selected_slot = slot;
                }
            }
        }

        // Check NEI palette hover/click.
        if self.palette_open && point_in_rect(cursor, layout.palette_panel) {
            consumed_cursor = true;
            if input.left_clicked && point_in_rect(cursor, layout.filter_btn) {
                self.filter = self.filter.next();
                self.page = 0;
            } else if input.left_clicked && point_in_rect(cursor, layout.prev_btn) {
                self.page = if self.page == 0 {
                    pages - 1
                } else {
                    self.page - 1
                };
            } else if input.left_clicked && point_in_rect(cursor, layout.next_btn) {
                self.page = (self.page + 1) % pages;
            } else {
                let start = self.page * NEI_PER_PAGE;
                for local_idx in 0..NEI_PER_PAGE {
                    let Some(&cat_idx) = filtered.get(start + local_idx) else {
                        break;
                    };
                    let rect = layout.nei_slot_rect(local_idx);
                    if point_in_rect(cursor, rect) {
                        self.hovered_catalog = Some(cat_idx);
                        if input.left_clicked {
                            self.hotbar[self.selected_slot] = cat_idx;
                        }
                        break;
                    }
                }
            }
        }

        // Map cursor to world grid cell if not hovering UI.
        if !consumed_cursor && cursor[0] >= 0.0 && cursor[1] >= 0.0 {
            let gx = (cursor[0] / cell_px) as usize;
            let gy = (cursor[1] / cell_px) as usize;
            if gx < grid_width && gy < grid_height {
                *probe_x = gx;
                *probe_y = gy;
                if input.left_held {
                    actions.place_at = Some((gx, gy, self.selected_entry().template));
                } else if input.right_held {
                    actions.erase_at = Some((gx, gy));
                }
            }
        }

        if (pressed & (1 << 12)) != 0 {
            actions.place_at = Some((*probe_x, *probe_y, self.selected_entry().template));
        }

        actions
    }

    /// Renders the NEI palette, hotbar, and aimed-at inspector overlay.
    pub fn render(
        &mut self,
        world: &mut GameWorld<'_>,
        grid: &CellGrid,
        probe_x: usize,
        probe_y: usize,
    ) {
        self.ensure_text_entities(world);
        let screen = valid_screen(world.screen_size());
        let layout = HudLayout::compute(screen);

        // 1. Aimed-at Inspector (top-left).
        draw_panel(world, layout.inspector_panel, [0.05, 0.07, 0.11, 0.84], 10);
        let inspector_str = if let Some(cat_idx) = self.hovered_catalog {
            let entry = &CATALOG[cat_idx];
            format_cell_inspector(Some((entry.name, entry.kind)), None, &entry.template)
        } else if let Some(cell) = grid.get(probe_x, probe_y) {
            format_cell_inspector(None, Some((probe_x, probe_y)), cell)
        } else {
            String::new()
        };
        set_text(world, self.inspector_entity, inspector_str, true);

        // 2. Bottom 9-slot Hotbar.
        draw_panel(world, layout.hotbar_panel, [0.06, 0.08, 0.12, 0.88], 10);
        for slot in 0..HOTBAR_SLOTS {
            let rect = layout.hotbar_slot_rect(slot);
            let entry = &CATALOG[self.hotbar[slot] % CATALOG.len()];
            let border_color = if slot == self.selected_slot {
                [1.0, 0.9, 0.35, 1.0]
            } else if entry.kind == CatalogKind::Entity {
                [0.3, 0.75, 0.65, 0.75]
            } else {
                [0.4, 0.45, 0.55, 0.7]
            };
            draw_panel(world, rect, border_color, 11);
            draw_panel(world, inset_rect(rect, 1.5), entry.color, 12);
        }
        let selected = self.selected_entry();
        let hotbar_caption = format!(
            "[1-9] {} ({}) | Tab:NEI | Spc/LMB:Place | RMB:Clear | F1-F5:Stim",
            selected.name,
            selected.kind.label()
        );
        set_text(world, self.hotbar_label_entity, hotbar_caption, true);

        // 3. Right-side NEI Palette.
        if self.palette_open {
            let filtered = self.filtered_indices();
            let pages = self.page_count(filtered.len());
            draw_panel(world, layout.palette_panel, [0.05, 0.07, 0.11, 0.88], 10);
            draw_panel(world, layout.filter_btn, [0.18, 0.24, 0.34, 0.95], 11);
            draw_panel(world, layout.prev_btn, [0.18, 0.24, 0.34, 0.95], 11);
            draw_panel(world, layout.next_btn, [0.18, 0.24, 0.34, 0.95], 11);

            let start = self.page * NEI_PER_PAGE;
            for local_idx in 0..NEI_PER_PAGE {
                let rect = layout.nei_slot_rect(local_idx);
                let Some(&cat_idx) = filtered.get(start + local_idx) else {
                    draw_panel(world, rect, [0.12, 0.14, 0.18, 0.45], 11);
                    continue;
                };
                let entry = &CATALOG[cat_idx];
                let is_hovered = self.hovered_catalog == Some(cat_idx);
                let border = if is_hovered {
                    [1.0, 1.0, 1.0, 1.0]
                } else if entry.kind == CatalogKind::Entity {
                    [0.3, 0.85, 0.65, 0.85]
                } else {
                    [0.45, 0.5, 0.6, 0.8]
                };
                draw_panel(world, rect, border, 11);
                draw_panel(world, inset_rect(rect, 1.5), entry.color, 12);
            }

            let header = format!(
                "NEI [{}] {}/{} (Q:Filter E:Page)",
                self.filter.label(),
                self.page + 1,
                pages
            );
            set_text(world, self.palette_header_entity, header, true);
        } else {
            set_text(world, self.palette_header_entity, String::new(), false);
        }
    }

    fn ensure_text_entities(&mut self, world: &mut GameWorld<'_>) {
        let gen = world.scene_generation();
        if self.last_generation == Some(gen)
            && self
                .inspector_entity
                .is_some_and(|entity| world.is_alive(entity))
        {
            return;
        }
        self.last_generation = Some(gen);
        self.inspector_entity = Some(spawn_hud_text(
            world,
            "emergence_inspector",
            6.0,
            5.0,
            300.0,
            6.0,
            Anchors::TOP_LEFT,
            Pivot::TOP_LEFT,
            TextAlign::Left,
        ));
        self.palette_header_entity = Some(spawn_hud_text(
            world,
            "emergence_nei_header",
            -6.0,
            6.0,
            110.0,
            5.5,
            Anchors {
                min_x: 1.0,
                min_y: 0.0,
                max_x: 1.0,
                max_y: 0.0,
            },
            Pivot { x: 1.0, y: 0.0 },
            TextAlign::Right,
        ));
        self.hotbar_label_entity = Some(spawn_hud_text(
            world,
            "emergence_hotbar_label",
            0.0,
            -22.0,
            260.0,
            5.5,
            Anchors::BOTTOM_CENTER,
            Pivot { x: 0.5, y: 1.0 },
            TextAlign::Center,
        ));
    }
}

struct HudLayout {
    inspector_panel: [f32; 4],
    hotbar_panel: [f32; 4],
    palette_panel: [f32; 4],
    filter_btn: [f32; 4],
    prev_btn: [f32; 4],
    next_btn: [f32; 4],
    hotbar_origin: [f32; 2],
    nei_origin: [f32; 2],
}

impl HudLayout {
    fn compute(screen: [f32; 2]) -> Self {
        let sw = screen[0];
        let sh = screen[1];

        let hotbar_w = HOTBAR_SLOTS as f32 * 16.0 + 4.0;
        let hotbar_x = (sw - hotbar_w) * 0.5;
        let hotbar_y = sh - 20.0;

        let palette_w = NEI_COLS as f32 * 18.0 + 8.0;
        let palette_h = NEI_ROWS as f32 * 18.0 + 22.0;
        let palette_x = sw - palette_w - 4.0;
        let palette_y = 4.0;

        Self {
            inspector_panel: [4.0, 4.0, 210.0, 62.0],
            hotbar_panel: [hotbar_x, hotbar_y, hotbar_w, 18.0],
            palette_panel: [palette_x, palette_y, palette_w, palette_h],
            filter_btn: [palette_x + 4.0, palette_y + 10.0, 32.0, 8.0],
            prev_btn: [palette_x + palette_w - 28.0, palette_y + 10.0, 11.0, 8.0],
            next_btn: [palette_x + palette_w - 15.0, palette_y + 10.0, 11.0, 8.0],
            hotbar_origin: [hotbar_x + 3.0, hotbar_y + 2.0],
            nei_origin: [palette_x + 5.0, palette_y + 20.0],
        }
    }

    fn hotbar_slot_rect(&self, slot: usize) -> [f32; 4] {
        [
            self.hotbar_origin[0] + slot as f32 * 16.0,
            self.hotbar_origin[1],
            14.0,
            14.0,
        ]
    }

    fn nei_slot_rect(&self, local_idx: usize) -> [f32; 4] {
        let col = local_idx % NEI_COLS;
        let row = local_idx / NEI_COLS;
        [
            self.nei_origin[0] + col as f32 * 18.0,
            self.nei_origin[1] + row as f32 * 18.0,
            16.0,
            16.0,
        ]
    }
}

fn collect_key_bits(input: &InputSnapshot) -> u32 {
    let mut bits = 0u32;
    if input.key_pressed(keys::TAB) {
        bits |= 1 << 0;
    }
    if input.key_pressed(keys::KEY_Q) {
        bits |= 1 << 1;
    }
    if input.key_pressed(keys::KEY_E) {
        bits |= 1 << 2;
    }
    let digits = [
        keys::DIGIT_1,
        keys::DIGIT_2,
        keys::DIGIT_3,
        keys::DIGIT_4,
        keys::DIGIT_5,
        keys::DIGIT_6,
        keys::DIGIT_7,
        keys::DIGIT_8,
        keys::DIGIT_9,
    ];
    for (idx, key) in digits.into_iter().enumerate() {
        if input.key_pressed(key) {
            bits |= 1 << (3 + idx);
        }
    }
    if input.key_pressed(keys::SPACE) {
        bits |= 1 << 12;
    }
    bits
}

fn valid_screen(screen: [f32; 2]) -> [f32; 2] {
    if screen[0] > 1.0 && screen[1] > 1.0 {
        screen
    } else {
        [320.0, 180.0]
    }
}

fn point_in_rect(p: [f32; 2], r: [f32; 4]) -> bool {
    p[0] >= r[0] && p[0] <= r[0] + r[2] && p[1] >= r[1] && p[1] <= r[1] + r[3]
}

fn inset_rect(r: [f32; 4], pad: f32) -> [f32; 4] {
    [
        r[0] + pad,
        r[1] + pad,
        (r[2] - 2.0 * pad).max(1.0),
        (r[3] - 2.0 * pad).max(1.0),
    ]
}

fn draw_panel(world: &mut GameWorld<'_>, rect: [f32; 4], color: [f32; 4], layer: i32) {
    world.draw_quad(UiQuad {
        rect,
        color,
        border_radius: 0.0,
        layer,
    });
}

#[allow(clippy::too_many_arguments)]
fn spawn_hud_text(
    world: &mut GameWorld<'_>,
    name: &str,
    margin_x: f32,
    margin_y: f32,
    width: f32,
    font_size: f32,
    anchors: Anchors,
    pivot: Pivot,
    alignment: TextAlign,
) -> EntityHandle {
    let entity = world.spawn();
    world.set(entity, Name(name.to_string()));
    world.set(entity, Node::default());
    world.set(
        entity,
        Canvas {
            reference_resolution: [320.0, 180.0],
            scale_mode: CanvasScaleMode::ScaleWithScreenSize,
            match_width_or_height: 0.5,
        },
    );
    world.set(
        entity,
        Style {
            width: Some(width),
            height: Some(80.0),
            anchors,
            pivot,
            margin: Margin {
                left: if margin_x >= 0.0 { margin_x } else { 0.0 },
                right: if margin_x < 0.0 { -margin_x } else { 0.0 },
                top: if margin_y >= 0.0 { margin_y } else { 0.0 },
                bottom: if margin_y < 0.0 { -margin_y } else { 0.0 },
            },
            ..Style::default()
        },
    );
    world.set(
        entity,
        Text {
            content: String::new(),
            font_size,
            color: [0.92, 0.96, 1.0, 1.0],
            alignment,
            ..Text::default()
        },
    );
    world.set(entity, Active(true));
    entity
}

fn set_text(
    world: &mut GameWorld<'_>,
    handle: Option<EntityHandle>,
    content: String,
    visible: bool,
) {
    let Some(entity) = handle else {
        return;
    };
    if let Some(mut text) = world.get::<Text>(entity) {
        if text.content != content {
            text.content = content;
            world.set(entity, text);
        }
    }
    world.set(entity, Active(visible));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nei_filter_separates_items_and_entities() {
        let mut hud = NeiHudState::new();
        let all_count = hud.filtered_indices().len();
        assert_eq!(all_count, CATALOG.len());

        hud.filter = CatalogFilter::Items;
        let items = hud.filtered_indices();
        assert!(items
            .iter()
            .all(|&idx| CATALOG[idx].kind == CatalogKind::Item));

        hud.filter = CatalogFilter::Entities;
        let entities = hud.filtered_indices();
        assert!(entities
            .iter()
            .all(|&idx| CATALOG[idx].kind == CatalogKind::Entity));
        assert_eq!(items.len() + entities.len(), all_count);
    }

    #[test]
    fn clicking_nei_slot_equips_hotbar_and_clicking_grid_places_cell() {
        let mut hud = NeiHudState::new();
        let mut px = 0usize;
        let mut py = 0usize;
        let layout = HudLayout::compute([320.0, 180.0]);
        let slot_rect = layout.nei_slot_rect(9);

        let mut click_nei = InputSnapshot {
            cursor: [slot_rect[0] + 4.0, slot_rect[1] + 4.0],
            screen: [320.0, 180.0],
            left_clicked: true,
            ..InputSnapshot::default()
        };
        hud.update_input(&click_nei, [320.0, 180.0], (32, 18), 10.0, &mut px, &mut py);
        assert_eq!(hud.hotbar[hud.selected_slot], 9);
        assert_eq!(hud.hovered_catalog, Some(9));

        click_nei.cursor = [55.0, 45.0];
        click_nei.left_clicked = false;
        click_nei.left_held = true;
        let actions =
            hud.update_input(&click_nei, [320.0, 180.0], (32, 18), 10.0, &mut px, &mut py);
        assert_eq!((px, py), (5, 4));
        assert!(actions.place_at.is_some());
    }
}
