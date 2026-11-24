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

use prism_scene::voxel::{VoxelHit, VoxelTier};

use crate::catalog::{
    collect_cell_debug_tags, format_cell_inspector, identify_cell, CatalogEntry, CatalogKind,
    CATALOG,
};
use crate::grid::CellGrid;
use crate::materials::mat_id;
use crate::player::{PlayerController, ToolAction, HOTBAR_LEN};
use crate::sim::SimWorld;
use crate::tags::{BioActivity, PhysicalCell, TransientFlags};

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
            [6.0, 5.0, 300.0, 6.0],
            Anchors::TOP_LEFT,
            Pivot::TOP_LEFT,
            TextAlign::Left,
        ));
        self.palette_header_entity = Some(spawn_hud_text(
            world,
            "emergence_nei_header",
            [-6.0, 6.0, 110.0, 5.5],
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
            [0.0, -22.0, 260.0, 5.5],
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

fn spawn_hud_text(
    world: &mut GameWorld<'_>,
    name: &str,
    rect: [f32; 4],
    anchors: Anchors,
    pivot: Pivot,
    alignment: TextAlign,
) -> EntityHandle {
    let [margin_x, margin_y, width, font_size] = rect;
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

/// One selectable entry in the 3D `EmergenceNeiHud` palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteItem {
    Catalog(usize),
    Stim(ToolAction),
}

impl PaletteItem {
    pub fn as_tool(self) -> ToolAction {
        match self {
            Self::Catalog(idx) => ToolAction::PlaceCatalog(idx),
            Self::Stim(tool) => tool,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Catalog(idx) => CATALOG.get(idx).map(|e| e.name).unwrap_or("Unknown"),
            Self::Stim(tool) => tool.label(),
        }
    }

    pub fn short_name(self) -> &'static str {
        match self {
            Self::Catalog(idx) => CATALOG
                .get(idx)
                .map(|e| compact_item_name(e.id, e.name))
                .unwrap_or("Item"),
            Self::Stim(tool) => tool.short_label(),
        }
    }

    pub fn kind_badge(self) -> &'static str {
        match self {
            Self::Catalog(idx) => CATALOG
                .get(idx)
                .map(|e| match e.kind {
                    CatalogKind::Item => "ITM",
                    CatalogKind::Entity => "ENT",
                })
                .unwrap_or("ITM"),
            Self::Stim(_) => "STM",
        }
    }

    pub fn color(self) -> [f32; 4] {
        match self {
            Self::Catalog(idx) => CATALOG
                .get(idx)
                .map(|e| e.color)
                .unwrap_or([0.7, 0.7, 0.7, 1.0]),
            Self::Stim(tool) => tool.swatch_color(),
        }
    }
}

fn compact_item_name(id: &str, fallback: &str) -> &'static str {
    match id {
        "conductive_wire" => "Wire",
        "superconductor" => "SupCoil",
        "water_cell" => "Water",
        "glass_wall" => "Glass",
        "insulated_wall" => "Alloy",
        "volatile_fuel" => "Fuel",
        "absorbent_sponge" => "Foam",
        "acid_pool" => "Acid",
        "cryo_spore" => "Spore",
        "hyper_neo_core" => "Mutant",
        "piezo_crystal" => "Piezo",
        "valve_conduit" => "Valve",
        "conveyor_east" => "Convey",
        "rad_isotope" => "Isotope",
        "stasis_emitter" => "Stasis",
        "plasma_arc" => "Plasma",
        "bedrock" => "Bedrock",
        "stone" => "Stone",
        "deepslate" => "Slate",
        "dirt" => "Dirt",
        "grass_block" => "Grass",
        "sand" => "Sand",
        "sandstone" => "Sandstn",
        "ice_block" => "Ice",
        "snow_block" => "Snow",
        "magma_lava" => "Magma",
        "coal_ore" => "Coal",
        "iron_ore" => "Iron",
        "copper_ore" => "Copper",
        "gold_ore" => "Gold",
        "redstone_ore" => "Redstn",
        "diamond_ore" => "Diamond",
        _ => {
            if fallback.len() <= 7 {
                "Item"
            } else {
                "Item"
            }
        }
    }
}

fn tool_compact_name(tool: ToolAction) -> &'static str {
    match tool {
        ToolAction::PlaceCatalog(idx) => PaletteItem::Catalog(idx).short_name(),
        other => other.short_label(),
    }
}

/// Top-center WAILA / Jade tooltip summary computed for either hovered NEI item or aimed world voxel.
#[derive(Debug, Clone, PartialEq)]
pub struct TopTooltipInfo {
    pub title: String,
    pub subtitle: String,
    pub tags: String,
    pub swatch: [f32; 4],
    pub accent: [f32; 4],
}

const HUD_NEI_COLS: usize = 4;
const HUD_NEI_ROWS: usize = 4;
const HUD_NEI_PER_PAGE: usize = HUD_NEI_COLS * HUD_NEI_ROWS;
const HUD_NEI_SLOT_W: f32 = 58.0;
const HUD_NEI_SLOT_H: f32 = 46.0;
const HUD_NEI_GAP: f32 = 5.0;
const HUD_NEI_PAD: f32 = 8.0;
const HUD_NEI_HEADER_H: f32 = 54.0;
const HUD_NEI_FOOTER_H: f32 = 24.0;

const HOTBAR_SLOT_SIZE: f32 = 46.0;
const HOTBAR_GAP: f32 = 4.0;
const HOTBAR_PAD: f32 = 6.0;

#[derive(Clone, Copy)]
struct BoxSlotUi {
    border: EntityHandle,
    bg: EntityHandle,
    swatch: EntityHandle,
    badge: EntityHandle,
    label: EntityHandle,
}

#[derive(Clone, Copy)]
struct TabButtonUi {
    bg: EntityHandle,
    text: EntityHandle,
}

#[derive(Clone, Copy)]
struct TooltipUi {
    border: EntityHandle,
    bg: EntityHandle,
    swatch_border: EntityHandle,
    swatch: EntityHandle,
    title: EntityHandle,
    subtitle: EntityHandle,
    tags: EntityHandle,
}

#[derive(Clone, Copy)]
struct NeiPanelUi {
    border: EntityHandle,
    bg: EntityHandle,
    title: EntityHandle,
    page_text: EntityHandle,
    tabs: [TabButtonUi; 3],
    prev_btn: TabButtonUi,
    next_btn: TabButtonUi,
    footer_hint: EntityHandle,
    slots: [BoxSlotUi; HUD_NEI_PER_PAGE],
}

#[derive(Clone, Copy)]
struct HotbarUi {
    caption_bg: EntityHandle,
    caption_text: EntityHandle,
    bar_border: EntityHandle,
    bar_bg: EntityHandle,
    slots: [BoxSlotUi; HOTBAR_LEN],
}

/// Integrated 3D WAILA top-center tooltip, bottom-center hotbar, and `[B]` toggleable NEI palette.
pub struct EmergenceNeiHud {
    pub open: bool,
    pub filter: CatalogFilter,
    pub page: usize,
    pub hovered_item: Option<PaletteItem>,
    pub hovered_hotbar: Option<usize>,
    attached: bool,
    tooltip: Option<TooltipUi>,
    hotbar: Option<HotbarUi>,
    panel: Option<NeiPanelUi>,
}

impl Default for EmergenceNeiHud {
    fn default() -> Self {
        Self::new()
    }
}

impl EmergenceNeiHud {
    pub fn new() -> Self {
        Self {
            open: true,
            filter: CatalogFilter::All,
            page: 0,
            hovered_item: None,
            hovered_hotbar: None,
            attached: false,
            tooltip: None,
            hotbar: None,
            panel: None,
        }
    }

    pub fn filtered_items(&self) -> Vec<PaletteItem> {
        let mut out = Vec::with_capacity(CATALOG.len() + 4);
        for (idx, entry) in CATALOG.iter().enumerate() {
            if self.filter.matches(entry) {
                out.push(PaletteItem::Catalog(idx));
            }
        }
        if self.filter == CatalogFilter::All {
            out.push(PaletteItem::Stim(ToolAction::StimHeat));
            out.push(PaletteItem::Stim(ToolAction::StimShock));
            out.push(PaletteItem::Stim(ToolAction::StimWet));
            out.push(PaletteItem::Stim(ToolAction::StimCold));
        }
        out
    }

    pub fn total_pages(&self, item_count: usize) -> usize {
        item_count.div_ceil(HUD_NEI_PER_PAGE).max(1)
    }

    /// Computes `[panel_x, panel_y, panel_w, panel_h]` for the bottom-right NEI palette.
    pub fn nei_panel_rect(screen: [f32; 2]) -> [f32; 4] {
        let sw = if screen[0] > 1.0 { screen[0] } else { 1280.0 };
        let sh = if screen[1] > 1.0 { screen[1] } else { 720.0 };
        let w = HUD_NEI_PAD * 2.0
            + HUD_NEI_COLS as f32 * HUD_NEI_SLOT_W
            + (HUD_NEI_COLS.saturating_sub(1)) as f32 * HUD_NEI_GAP;
        let grid_h = HUD_NEI_ROWS as f32 * HUD_NEI_SLOT_H
            + (HUD_NEI_ROWS.saturating_sub(1)) as f32 * HUD_NEI_GAP;
        let h = HUD_NEI_PAD * 2.0 + HUD_NEI_HEADER_H + grid_h + HUD_NEI_GAP + HUD_NEI_FOOTER_H;
        let x = (sw - w - 10.0).max(8.0);
        let y = (sh - h - 12.0).max(8.0);
        [x, y, w, h]
    }

    /// Computes `[bar_x, bar_y, bar_w, bar_h]` for the bottom-center hotbar.
    pub fn hotbar_rect(screen: [f32; 2]) -> [f32; 4] {
        let sw = if screen[0] > 1.0 { screen[0] } else { 1280.0 };
        let sh = if screen[1] > 1.0 { screen[1] } else { 720.0 };
        let w = HOTBAR_PAD * 2.0
            + HOTBAR_LEN as f32 * HOTBAR_SLOT_SIZE
            + (HOTBAR_LEN.saturating_sub(1)) as f32 * HOTBAR_GAP;
        let h = HOTBAR_PAD * 2.0 + HOTBAR_SLOT_SIZE;
        let x = ((sw - w) * 0.5).max(8.0);
        let y = (sh - h - 10.0).max(8.0);
        [x, y, w, h]
    }

    fn filter_tab_rect(panel: [f32; 4], tab_idx: usize) -> [f32; 4] {
        let inner_w = panel[2] - HUD_NEI_PAD * 2.0;
        let gap = 4.0;
        let tab_w = (inner_w - gap * 2.0) / 3.0;
        [
            panel[0] + HUD_NEI_PAD + tab_idx as f32 * (tab_w + gap),
            panel[1] + HUD_NEI_PAD + 22.0,
            tab_w,
            20.0,
        ]
    }

    fn nei_slot_rect(panel: [f32; 4], local_idx: usize) -> [f32; 4] {
        let col = local_idx % HUD_NEI_COLS;
        let row = local_idx / HUD_NEI_COLS;
        let x = panel[0] + HUD_NEI_PAD + col as f32 * (HUD_NEI_SLOT_W + HUD_NEI_GAP);
        let y =
            panel[1] + HUD_NEI_PAD + HUD_NEI_HEADER_H + row as f32 * (HUD_NEI_SLOT_H + HUD_NEI_GAP);
        [x, y, HUD_NEI_SLOT_W, HUD_NEI_SLOT_H]
    }

    fn pager_rect(panel: [f32; 4], is_next: bool) -> [f32; 4] {
        let y = panel[1] + panel[3] - HUD_NEI_PAD - HUD_NEI_FOOTER_H;
        let btn_w = 36.0;
        let x = if is_next {
            panel[0] + panel[2] - HUD_NEI_PAD - btn_w
        } else {
            panel[0] + HUD_NEI_PAD
        };
        [x, y, btn_w, 20.0]
    }

    fn hotbar_slot_rect(bar: [f32; 4], slot_idx: usize) -> [f32; 4] {
        let x = bar[0] + HOTBAR_PAD + slot_idx as f32 * (HOTBAR_SLOT_SIZE + HOTBAR_GAP);
        let y = bar[1] + HOTBAR_PAD;
        [x, y, HOTBAR_SLOT_SIZE, HOTBAR_SLOT_SIZE]
    }

    fn point_in(rect: [f32; 4], p: [f32; 2]) -> bool {
        p[0] >= rect[0] && p[0] <= rect[0] + rect[2] && p[1] >= rect[1] && p[1] <= rect[1] + rect[3]
    }

    /// Handles `[B]` NEI toggle and pointer interactions with hotbar & NEI panel.
    /// Returns `true` when pointer input falls inside HUD panels and should not pass to world drag/carve/place.
    pub fn handle_input(
        &mut self,
        input: &InputSnapshot,
        screen: [f32; 2],
        console_active: bool,
        player: &mut PlayerController,
    ) -> bool {
        self.hovered_item = None;
        self.hovered_hotbar = None;

        if console_active {
            return false;
        }

        if input.key_pressed(keys::OTHER_BASE + b'B' as usize) {
            self.open = !self.open;
        }
        if self.open && input.key_pressed(keys::OTHER_BASE + b'[' as usize) {
            self.page = self.page.saturating_sub(1);
        }
        if self.open && input.key_pressed(keys::OTHER_BASE + b']' as usize) {
            let pages = self.total_pages(self.filtered_items().len());
            self.page = (self.page + 1).min(pages - 1);
        }

        if input.pointer_locked {
            return false;
        }

        let items = self.filtered_items();
        let pages = self.total_pages(items.len());
        self.page = self.page.min(pages - 1);

        let cur = input.cursor;
        let bar = Self::hotbar_rect(screen);
        if Self::point_in(bar, cur) {
            for idx in 0..HOTBAR_LEN {
                let r = Self::hotbar_slot_rect(bar, idx);
                if Self::point_in(r, cur) {
                    self.hovered_hotbar = Some(idx);
                    if input.left_clicked {
                        player.select_slot(idx);
                    }
                    break;
                }
            }
            return true;
        }

        if !self.open {
            return false;
        }

        let panel = Self::nei_panel_rect(screen);
        if !Self::point_in(panel, cur) {
            return false;
        }

        const FILTERS: [CatalogFilter; 3] = [
            CatalogFilter::All,
            CatalogFilter::Items,
            CatalogFilter::Entities,
        ];
        for (tab_idx, filter) in FILTERS.into_iter().enumerate() {
            if Self::point_in(Self::filter_tab_rect(panel, tab_idx), cur) {
                if input.left_clicked {
                    self.filter = filter;
                    self.page = 0;
                }
                return true;
            }
        }

        if Self::point_in(Self::pager_rect(panel, false), cur) {
            if input.left_clicked {
                self.page = self.page.saturating_sub(1);
            }
            return true;
        }
        if Self::point_in(Self::pager_rect(panel, true), cur) {
            if input.left_clicked {
                self.page = (self.page + 1).min(pages - 1);
            }
            return true;
        }

        let start = self.page * HUD_NEI_PER_PAGE;
        for local_idx in 0..HUD_NEI_PER_PAGE {
            let Some(&item) = items.get(start + local_idx) else {
                break;
            };
            if Self::point_in(Self::nei_slot_rect(panel, local_idx), cur) {
                self.hovered_item = Some(item);
                if input.left_clicked {
                    player.equip_active_slot(item.as_tool());
                } else if input.right_clicked {
                    player.equip_active_slot(item.as_tool());
                }
                return true;
            }
        }

        true
    }

    /// Resolves the WAILA / Jade top-center tooltip from hover state or aimed 3D voxel.
    pub fn build_top_tooltip(
        &self,
        sim: &SimWorld,
        player: &PlayerController,
    ) -> Option<TopTooltipInfo> {
        if let Some(item) = self.hovered_item {
            return Some(Self::tooltip_for_palette_item(item, player));
        }
        if let Some(slot_idx) = self.hovered_hotbar {
            let tool = player.hotbar[slot_idx.min(HOTBAR_LEN - 1)];
            return Some(Self::tooltip_for_tool(tool, slot_idx));
        }
        let hit = player.last_hit()?;
        Self::tooltip_for_hit(sim, player, hit)
    }

    fn tooltip_for_palette_item(item: PaletteItem, player: &PlayerController) -> TopTooltipInfo {
        match item {
            PaletteItem::Catalog(idx) => {
                let entry = &CATALOG[idx];
                let mut tags = collect_cell_debug_tags(&entry.template);
                let tag_line = if tags.is_empty() {
                    format!("Click to equip slot [{}]", player.tool_index + 1)
                } else {
                    tags.truncate(4);
                    format!(
                        "{}  | Click: Slot {}",
                        tags.join(" "),
                        player.tool_index + 1
                    )
                };
                TopTooltipInfo {
                    title: format!("{}  [{}]", entry.name, entry.kind.label()),
                    subtitle: format!(
                        "emergence:{} · {:?} · {:?} · HP {}/255",
                        entry.id,
                        entry.template.env.matter(),
                        entry.template.env.temperature(),
                        entry.template.durability,
                    ),
                    tags: tag_line,
                    swatch: entry.color,
                    accent: match entry.kind {
                        CatalogKind::Item => [0.35, 0.72, 0.98, 0.96],
                        CatalogKind::Entity => [0.95, 0.56, 0.25, 0.96],
                    },
                }
            }
            PaletteItem::Stim(tool) => TopTooltipInfo {
                title: format!("{}  [Stimulus]", tool.label()),
                subtitle: "Applies v3.0 environmental stimulus on aimed voxel".to_string(),
                tags: format!("Click to equip slot [{}]", player.tool_index + 1),
                swatch: tool.swatch_color(),
                accent: tool.swatch_color(),
            },
        }
    }

    fn tooltip_for_tool(tool: ToolAction, slot_idx: usize) -> TopTooltipInfo {
        match tool {
            ToolAction::PlaceCatalog(idx) => {
                let entry = &CATALOG[idx];
                TopTooltipInfo {
                    title: format!("{}  [Slot {}]", entry.name, slot_idx + 1),
                    subtitle: format!(
                        "emergence:{} · {:?} · {:?} · HP {}/255",
                        entry.id,
                        entry.template.env.matter(),
                        entry.template.env.temperature(),
                        entry.template.durability,
                    ),
                    tags: format!("Tool: {} | Right-Click to place", entry.kind.label()),
                    swatch: entry.color,
                    accent: [0.98, 0.84, 0.32, 0.96],
                }
            }
            _ => TopTooltipInfo {
                title: format!("{}  [Slot {}]", tool.label(), slot_idx + 1),
                subtitle: "Hotbar tool ready — Right-Click in viewport to use".to_string(),
                tags: "Keys [4-0], [Q], or Mouse Wheel switch slots".to_string(),
                swatch: tool.swatch_color(),
                accent: [0.98, 0.84, 0.32, 0.96],
            },
        }
    }

    fn tooltip_for_hit(
        sim: &SimWorld,
        player: &PlayerController,
        hit: &VoxelHit,
    ) -> Option<TopTooltipInfo> {
        let (mat, cell, l3_mask) = sim.inspect_l2(hit.world_l2)?;
        let entry = sim.materials.get(mat);
        let mat_name = &entry.name;
        let slug = mat_name.to_ascii_lowercase().replace(' ', "_");
        let (mut ident_name, mut kind) = identify_cell(&cell);
        if ident_name == "Bio-Specimen"
            && matches!(mat, mat_id::GRASS | mat_id::SOIL | mat_id::TIMBER)
            && cell.env.bio_activity() < BioActivity::Vital
            && !cell.transient.contains(TransientFlags::INFECTED)
        {
            ident_name = mat_name;
            kind = CatalogKind::Item;
        }
        let tier_label = match player.tier {
            VoxelTier::L1 => "L1 1.0m",
            VoxelTier::L2 => "L2 0.25m",
            VoxelTier::L3 => "L3 0.06m",
        };
        let display_name = if ident_name == "Solid Matter"
            || ident_name == "Fluid Cell"
            || ident_name.eq_ignore_ascii_case(mat_name)
        {
            mat_name.to_string()
        } else {
            format!("{ident_name} ({mat_name})")
        };

        let mut tags = collect_cell_debug_tags(&cell);
        let tags_line = if tags.is_empty() {
            format!(
                "LMB Carve · RMB {} · [B] NEI",
                tool_compact_name(player.active_tool())
            )
        } else {
            tags.truncate(4);
            tags.join("  ")
        };

        let [r, g, b, a] = entry.rgba;
        let swatch = [
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
            (a as f32 / 255.0).max(0.75),
        ];
        let accent = match kind {
            CatalogKind::Entity => [0.95, 0.56, 0.25, 0.95],
            CatalogKind::Item => match hit.hit_tier {
                VoxelTier::L1 => [0.36, 0.72, 0.98, 0.95],
                VoxelTier::L2 => [0.38, 0.88, 0.62, 0.95],
                VoxelTier::L3 => [0.42, 0.94, 0.98, 0.98],
            },
        };

        Some(TopTooltipInfo {
            title: format!("{display_name}  [{tier_label} · {}]", kind.label()),
            subtitle: format!(
                "emergence:{slug} · {:?} · {:?} · L3 {}/64 · HP {}/255 ({:.1}m)",
                cell.env.matter(),
                cell.env.temperature(),
                l3_mask.count_ones(),
                cell.durability,
                hit.distance,
            ),
            tags: tags_line,
            swatch,
            accent,
        })
    }

    fn ensure_attached(&mut self, world: &mut GameWorld<'_>) {
        if self.attached {
            return;
        }
        self.attached = true;

        self.tooltip = Some(TooltipUi {
            border: spawn_ui_entity(world, "nei_tip_border"),
            bg: spawn_ui_entity(world, "nei_tip_bg"),
            swatch_border: spawn_ui_entity(world, "nei_tip_swatch_border"),
            swatch: spawn_ui_entity(world, "nei_tip_swatch"),
            title: spawn_ui_entity(world, "nei_tip_title"),
            subtitle: spawn_ui_entity(world, "nei_tip_subtitle"),
            tags: spawn_ui_entity(world, "nei_tip_tags"),
        });

        let caption_bg = spawn_ui_entity(world, "hotbar_caption_bg");
        let caption_text = spawn_ui_entity(world, "hotbar_caption_text");
        let bar_border = spawn_ui_entity(world, "hotbar_border");
        let bar_bg = spawn_ui_entity(world, "hotbar_bg");
        let h_slots = std::array::from_fn(|i| BoxSlotUi {
            border: spawn_ui_entity(world, &format!("hotbar_slot_border_{i}")),
            bg: spawn_ui_entity(world, &format!("hotbar_slot_bg_{i}")),
            swatch: spawn_ui_entity(world, &format!("hotbar_slot_swatch_{i}")),
            badge: spawn_ui_entity(world, &format!("hotbar_slot_key_{i}")),
            label: spawn_ui_entity(world, &format!("hotbar_slot_label_{i}")),
        });
        self.hotbar = Some(HotbarUi {
            caption_bg,
            caption_text,
            bar_border,
            bar_bg,
            slots: h_slots,
        });

        let border = spawn_ui_entity(world, "nei_panel_border");
        let bg = spawn_ui_entity(world, "nei_panel_bg");
        let title = spawn_ui_entity(world, "nei_panel_title");
        let page_text = spawn_ui_entity(world, "nei_panel_page");
        let tabs = [
            TabButtonUi {
                bg: spawn_ui_entity(world, "nei_tab_bg_0"),
                text: spawn_ui_entity(world, "nei_tab_text_0"),
            },
            TabButtonUi {
                bg: spawn_ui_entity(world, "nei_tab_bg_1"),
                text: spawn_ui_entity(world, "nei_tab_text_1"),
            },
            TabButtonUi {
                bg: spawn_ui_entity(world, "nei_tab_bg_2"),
                text: spawn_ui_entity(world, "nei_tab_text_2"),
            },
        ];
        let prev_btn = TabButtonUi {
            bg: spawn_ui_entity(world, "nei_prev_bg"),
            text: spawn_ui_entity(world, "nei_prev_text"),
        };
        let next_btn = TabButtonUi {
            bg: spawn_ui_entity(world, "nei_next_bg"),
            text: spawn_ui_entity(world, "nei_next_text"),
        };
        let footer_hint = spawn_ui_entity(world, "nei_footer_hint");
        let p_slots = std::array::from_fn(|i| BoxSlotUi {
            border: spawn_ui_entity(world, &format!("nei_slot_border_{i}")),
            bg: spawn_ui_entity(world, &format!("nei_slot_bg_{i}")),
            swatch: spawn_ui_entity(world, &format!("nei_slot_swatch_{i}")),
            badge: spawn_ui_entity(world, &format!("nei_slot_badge_{i}")),
            label: spawn_ui_entity(world, &format!("nei_slot_label_{i}")),
        });
        self.panel = Some(NeiPanelUi {
            border,
            bg,
            title,
            page_text,
            tabs,
            prev_btn,
            next_btn,
            footer_hint,
            slots: p_slots,
        });
    }

    /// Synchronizes all pooled ECS UI entities for top-center tooltips, hotbar, and NEI panel.
    pub fn sync_ui(
        &mut self,
        world: &mut GameWorld<'_>,
        sim: &SimWorld,
        player: &PlayerController,
        console_open: bool,
    ) {
        self.ensure_attached(world);
        let screen = world.screen_size();
        let sw = if screen[0] > 1.0 { screen[0] } else { 1280.0 };

        if let Some(tip_ui) = self.tooltip {
            if let Some(info) = self.build_top_tooltip(sim, player) {
                let title_w = crate::debug_hud::estimate_text_width(&info.title, 14.5);
                let sub_w = crate::debug_hud::estimate_text_width(&info.subtitle, 12.0);
                let tag_w = crate::debug_hud::estimate_text_width(&info.tags, 11.5);
                let text_w = (title_w.max(sub_w).max(tag_w) * 1.12).clamp(200.0, 560.0);
                let card_w = (56.0 + text_w + 18.0).min(sw - 24.0);
                let card_h = 68.0;
                let card_x = (sw - card_w) * 0.5;
                let card_y = 10.0;

                set_rect_style(
                    world,
                    tip_ui.border,
                    [card_x - 1.5, card_y - 1.5, card_w + 3.0, card_h + 3.0],
                    info.accent,
                    6.0,
                    true,
                );
                set_rect_style(
                    world,
                    tip_ui.bg,
                    [card_x, card_y, card_w, card_h],
                    [0.06, 0.08, 0.12, 0.90],
                    5.0,
                    true,
                );
                set_rect_style(
                    world,
                    tip_ui.swatch_border,
                    [card_x + 10.0, card_y + 15.0, 36.0, 36.0],
                    [0.24, 0.29, 0.37, 0.95],
                    4.0,
                    true,
                );
                set_rect_style(
                    world,
                    tip_ui.swatch,
                    [card_x + 12.0, card_y + 17.0, 32.0, 32.0],
                    info.swatch,
                    3.0,
                    true,
                );

                let tx = card_x + 56.0;
                let tw = (card_w - 64.0).max(80.0);
                set_text_style(
                    world,
                    tip_ui.title,
                    [tx, card_y + 7.0, tw, 22.0],
                    info.title,
                    14.5,
                    [0.96, 0.98, 1.0, 1.0],
                    TextAlign::Left,
                );
                set_text_style(
                    world,
                    tip_ui.subtitle,
                    [tx, card_y + 27.0, tw, 18.0],
                    info.subtitle,
                    12.0,
                    [0.72, 0.80, 0.90, 1.0],
                    TextAlign::Left,
                );
                set_text_style(
                    world,
                    tip_ui.tags,
                    [tx, card_y + 45.0, tw, 18.0],
                    info.tags,
                    11.5,
                    [0.54, 0.88, 0.84, 1.0],
                    TextAlign::Left,
                );
            } else {
                for h in [
                    tip_ui.border,
                    tip_ui.bg,
                    tip_ui.swatch_border,
                    tip_ui.swatch,
                    tip_ui.title,
                    tip_ui.subtitle,
                    tip_ui.tags,
                ] {
                    world.set(h, Active(false));
                }
            }
        }

        if let Some(hb_ui) = self.hotbar {
            if console_open {
                for h in [
                    hb_ui.caption_bg,
                    hb_ui.caption_text,
                    hb_ui.bar_border,
                    hb_ui.bar_bg,
                ] {
                    world.set(h, Active(false));
                }
                for s in hb_ui.slots {
                    for h in [s.border, s.bg, s.swatch, s.badge, s.label] {
                        world.set(h, Active(false));
                    }
                }
            } else {
                let bar = Self::hotbar_rect(screen);
                let caption = format!(
                    "{}  |  {}  |  [B] NEI {}",
                    player.active_tool_label(),
                    player.tier_summary(),
                    if self.open { "Open" } else { "Closed" }
                );
                let cap_w = (crate::debug_hud::estimate_text_width(&caption, 11.5) + 18.0)
                    .clamp(220.0, sw - 24.0);
                let cap_x = (sw - cap_w) * 0.5;
                let cap_y = bar[1] - 24.0;
                set_rect_style(
                    world,
                    hb_ui.caption_bg,
                    [cap_x, cap_y, cap_w, 20.0],
                    [0.06, 0.08, 0.12, 0.84],
                    4.0,
                    true,
                );
                set_text_style(
                    world,
                    hb_ui.caption_text,
                    [cap_x + 4.0, cap_y + 2.0, cap_w - 8.0, 20.0],
                    caption,
                    12.0,
                    [0.92, 0.96, 1.0, 0.96],
                    TextAlign::Center,
                );

                set_rect_style(
                    world,
                    hb_ui.bar_border,
                    [bar[0] - 1.5, bar[1] - 1.5, bar[2] + 3.0, bar[3] + 3.0],
                    [0.24, 0.30, 0.38, 0.92],
                    6.0,
                    true,
                );
                set_rect_style(
                    world,
                    hb_ui.bar_bg,
                    bar,
                    [0.06, 0.08, 0.12, 0.90],
                    5.0,
                    true,
                );

                for (i, slot_ui) in hb_ui.slots.iter().enumerate() {
                    let r = Self::hotbar_slot_rect(bar, i);
                    let tool = player.hotbar[i];
                    let selected = player.tool_index == i;
                    let hovered = self.hovered_hotbar == Some(i);
                    let border_col = if selected {
                        [1.0, 0.86, 0.28, 1.0]
                    } else if hovered {
                        [0.52, 0.84, 1.0, 0.95]
                    } else {
                        [0.20, 0.25, 0.32, 0.90]
                    };
                    let bg_col = if selected {
                        [0.16, 0.20, 0.28, 0.96]
                    } else if hovered {
                        [0.14, 0.17, 0.24, 0.94]
                    } else {
                        [0.09, 0.11, 0.16, 0.92]
                    };
                    set_rect_style(world, slot_ui.border, r, border_col, 4.0, true);
                    set_rect_style(
                        world,
                        slot_ui.bg,
                        [r[0] + 1.5, r[1] + 1.5, r[2] - 3.0, r[3] - 3.0],
                        bg_col,
                        3.0,
                        true,
                    );
                    set_rect_style(
                        world,
                        slot_ui.swatch,
                        [r[0] + 14.0, r[1] + 6.0, 18.0, 17.0],
                        tool.swatch_color(),
                        3.0,
                        true,
                    );
                    if i <= 6 {
                        let key_badge = if i < 6 {
                            format!("{}", i + 4)
                        } else {
                            "0".to_string()
                        };
                        set_text_style(
                            world,
                            slot_ui.badge,
                            [r[0] + 3.0, r[1] + 2.0, 16.0, 16.0],
                            key_badge,
                            11.0,
                            if selected {
                                [1.0, 0.90, 0.45, 1.0]
                            } else {
                                [0.64, 0.72, 0.82, 0.92]
                            },
                            TextAlign::Left,
                        );
                    } else {
                        world.set(slot_ui.badge, Active(false));
                    }
                    set_text_style(
                        world,
                        slot_ui.label,
                        [r[0] + 1.0, r[1] + 26.0, r[2] - 2.0, 18.0],
                        tool_compact_name(tool).to_string(),
                        11.0,
                        [0.92, 0.95, 0.99, 0.96],
                        TextAlign::Center,
                    );
                }
            }
        }

        if let Some(panel_ui) = self.panel {
            if !self.open {
                for h in [
                    panel_ui.border,
                    panel_ui.bg,
                    panel_ui.title,
                    panel_ui.page_text,
                    panel_ui.footer_hint,
                    panel_ui.prev_btn.bg,
                    panel_ui.prev_btn.text,
                    panel_ui.next_btn.bg,
                    panel_ui.next_btn.text,
                ] {
                    world.set(h, Active(false));
                }
                for t in panel_ui.tabs {
                    world.set(t.bg, Active(false));
                    world.set(t.text, Active(false));
                }
                for s in panel_ui.slots {
                    world.set(s.border, Active(false));
                    world.set(s.bg, Active(false));
                    world.set(s.swatch, Active(false));
                    world.set(s.badge, Active(false));
                    world.set(s.label, Active(false));
                }
                return;
            }

            let items = self.filtered_items();
            let pages = self.total_pages(items.len());
            self.page = self.page.min(pages - 1);
            let panel = Self::nei_panel_rect(screen);

            set_rect_style(
                world,
                panel_ui.border,
                [
                    panel[0] - 1.5,
                    panel[1] - 1.5,
                    panel[2] + 3.0,
                    panel[3] + 3.0,
                ],
                [0.28, 0.38, 0.50, 0.92],
                7.0,
                true,
            );
            set_rect_style(
                world,
                panel_ui.bg,
                panel,
                [0.06, 0.08, 0.12, 0.90],
                6.0,
                true,
            );
            set_text_style(
                world,
                panel_ui.title,
                [panel[0] + HUD_NEI_PAD, panel[1] + 7.0, 150.0, 20.0],
                "NEI Catalog [B]".to_string(),
                13.0,
                [0.95, 0.97, 1.0, 1.0],
                TextAlign::Left,
            );
            set_text_style(
                world,
                panel_ui.page_text,
                [panel[0] + panel[2] - 96.0, panel[1] + 7.0, 88.0, 20.0],
                format!("{}/{} ({})", self.page + 1, pages, items.len()),
                11.5,
                [0.72, 0.82, 0.94, 0.95],
                TextAlign::Right,
            );

            const FILTERS: [CatalogFilter; 3] = [
                CatalogFilter::All,
                CatalogFilter::Items,
                CatalogFilter::Entities,
            ];
            for (idx, filter) in FILTERS.into_iter().enumerate() {
                let tr = Self::filter_tab_rect(panel, idx);
                let active = self.filter == filter;
                set_rect_style(
                    world,
                    panel_ui.tabs[idx].bg,
                    tr,
                    if active {
                        [0.24, 0.44, 0.68, 0.96]
                    } else {
                        [0.12, 0.15, 0.21, 0.92]
                    },
                    4.0,
                    true,
                );
                set_text_style(
                    world,
                    panel_ui.tabs[idx].text,
                    [tr[0], tr[1] + 2.0, tr[2], 20.0],
                    filter.label().to_string(),
                    11.5,
                    if active {
                        [1.0, 0.98, 0.86, 1.0]
                    } else {
                        [0.74, 0.80, 0.88, 0.92]
                    },
                    TextAlign::Center,
                );
            }

            let prev_r = Self::pager_rect(panel, false);
            let next_r = Self::pager_rect(panel, true);
            set_rect_style(
                world,
                panel_ui.prev_btn.bg,
                prev_r,
                [0.13, 0.17, 0.24, 0.94],
                4.0,
                true,
            );
            set_text_style(
                world,
                panel_ui.prev_btn.text,
                [prev_r[0], prev_r[1] + 2.0, prev_r[2], 20.0],
                "< [".to_string(),
                11.5,
                [0.88, 0.94, 1.0, 1.0],
                TextAlign::Center,
            );
            set_rect_style(
                world,
                panel_ui.next_btn.bg,
                next_r,
                [0.13, 0.17, 0.24, 0.94],
                4.0,
                true,
            );
            set_text_style(
                world,
                panel_ui.next_btn.text,
                [next_r[0], next_r[1] + 2.0, next_r[2], 20.0],
                "] >".to_string(),
                11.5,
                [0.88, 0.94, 1.0, 1.0],
                TextAlign::Center,
            );
            set_text_style(
                world,
                panel_ui.footer_hint,
                [
                    prev_r[0] + prev_r[2] + 4.0,
                    prev_r[1] + 2.0,
                    (next_r[0] - (prev_r[0] + prev_r[2]) - 8.0).max(40.0),
                    20.0,
                ],
                format!("Equip -> Slot {}", player.tool_index + 1),
                11.5,
                [0.74, 0.82, 0.92, 0.94],
                TextAlign::Center,
            );

            let active_tool = player.active_tool();
            let start = self.page * HUD_NEI_PER_PAGE;
            for (local_idx, slot_ui) in panel_ui.slots.iter().enumerate() {
                let Some(&item) = items.get(start + local_idx) else {
                    for h in [
                        slot_ui.border,
                        slot_ui.bg,
                        slot_ui.swatch,
                        slot_ui.badge,
                        slot_ui.label,
                    ] {
                        world.set(h, Active(false));
                    }
                    continue;
                };
                let r = Self::nei_slot_rect(panel, local_idx);
                let hovered = self.hovered_item == Some(item);
                let equipped = active_tool == item.as_tool();
                let border_col = if hovered {
                    [1.0, 0.86, 0.34, 1.0]
                } else if equipped {
                    [0.36, 0.88, 0.62, 0.98]
                } else {
                    [0.20, 0.25, 0.33, 0.90]
                };
                let bg_col = if hovered {
                    [0.16, 0.21, 0.29, 0.96]
                } else if equipped {
                    [0.12, 0.20, 0.19, 0.95]
                } else {
                    [0.09, 0.12, 0.17, 0.92]
                };
                set_rect_style(world, slot_ui.border, r, border_col, 4.0, true);
                set_rect_style(
                    world,
                    slot_ui.bg,
                    [r[0] + 1.5, r[1] + 1.5, r[2] - 3.0, r[3] - 3.0],
                    bg_col,
                    3.0,
                    true,
                );
                set_rect_style(
                    world,
                    slot_ui.swatch,
                    [r[0] + 5.0, r[1] + 5.0, 16.0, 16.0],
                    item.color(),
                    3.0,
                    true,
                );
                set_text_style(
                    world,
                    slot_ui.badge,
                    [r[0] + 25.0, r[1] + 5.0, r[2] - 28.0, 18.0],
                    item.kind_badge().to_string(),
                    11.0,
                    if item.kind_badge() == "ENT" {
                        [0.98, 0.66, 0.36, 0.95]
                    } else if item.kind_badge() == "STM" {
                        [0.45, 0.88, 1.0, 0.95]
                    } else {
                        [0.64, 0.74, 0.86, 0.90]
                    },
                    TextAlign::Left,
                );
                set_text_style(
                    world,
                    slot_ui.label,
                    [r[0] + 2.0, r[1] + 25.0, r[2] - 4.0, 18.0],
                    item.short_name().to_string(),
                    11.5,
                    [0.93, 0.96, 1.0, 0.98],
                    TextAlign::Center,
                );
            }
        }
    }
}

const HUD_CANVAS: Canvas = Canvas {
    reference_resolution: [1280.0, 720.0],
    scale_mode: CanvasScaleMode::ConstantPixelSize,
    match_width_or_height: 0.5,
};

fn spawn_ui_entity(world: &mut GameWorld<'_>, name: &str) -> EntityHandle {
    let e = world.spawn();
    world.set(e, Name(name.to_string()));
    world.set(e, Node::default());
    world.set(e, HUD_CANVAS);
    world.set(e, Active(false));
    e
}

fn set_rect_style(
    world: &mut GameWorld<'_>,
    entity: EntityHandle,
    rect: [f32; 4],
    background: [f32; 4],
    border_radius: f32,
    visible: bool,
) {
    world.set(entity, Active(visible));
    if !visible {
        return;
    }
    world.set(
        entity,
        Style {
            width: Some(rect[2].max(1.0)),
            height: Some(rect[3].max(1.0)),
            anchors: Anchors::TOP_LEFT,
            pivot: Pivot::TOP_LEFT,
            margin: Margin {
                left: rect[0],
                top: rect[1],
                ..Margin::default()
            },
            background,
            border_radius,
            ..Style::default()
        },
    );
}

fn set_text_style(
    world: &mut GameWorld<'_>,
    entity: EntityHandle,
    rect: [f32; 4],
    content: String,
    font_size: f32,
    color: [f32; 4],
    alignment: TextAlign,
) {
    world.set(entity, Active(true));
    world.set(
        entity,
        Style {
            width: Some(rect[2].max(4.0)),
            height: Some(rect[3].max(4.0)),
            anchors: Anchors::TOP_LEFT,
            pivot: Pivot::TOP_LEFT,
            margin: Margin {
                left: rect[0],
                top: rect[1],
                ..Margin::default()
            },
            ..Style::default()
        },
    );
    world.set(
        entity,
        Text {
            content,
            font_size,
            color,
            alignment,
            wrap: false,
            ..Text::default()
        },
    );
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

    #[test]
    fn emergence_nei_hud_equips_hotbar_filters_pages_and_builds_top_tooltip() {
        use crate::materials::MaterialTable;
        use crate::world_gen::generate_world;
        use prism_game_api::InputSnapshot;

        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 2026);
        let sim = SimWorld::new(mats, gen.chunks, gen.cells, gen.active);
        let mut ecs = prism_ecs::World::new();
        let mut world = GameWorld::host(&mut ecs);

        let mut player = PlayerController::new();
        player.attach_ecs(&mut world, &sim);
        let mut nei = EmergenceNeiHud::new();
        assert!(nei.open);

        let mut sim_mut = sim;
        player.update_look_and_tools(&mut world, &mut sim_mut, 1.0 / 60.0);
        let tip = nei
            .build_top_tooltip(&sim_mut, &player)
            .expect("initial player gaze hits world terrain/outpost for top tooltip");
        assert!(tip.subtitle.contains("emergence:"));
        assert!(tip.subtitle.contains("L3 "));

        let panel = EmergenceNeiHud::nei_panel_rect([1280.0, 720.0]);
        let slot3 = EmergenceNeiHud::nei_slot_rect(panel, 3);
        let click_slot = InputSnapshot {
            cursor: [slot3[0] + 10.0, slot3[1] + 10.0],
            screen: [1280.0, 720.0],
            left_clicked: true,
            ..InputSnapshot::default()
        };
        let consumed = nei.handle_input(&click_slot, [1280.0, 720.0], false, &mut player);
        assert!(consumed);
        assert_eq!(player.active_tool(), ToolAction::PlaceCatalog(3));
        assert_eq!(nei.hovered_item, Some(PaletteItem::Catalog(3)));

        let hover_tip = nei
            .build_top_tooltip(&sim_mut, &player)
            .expect("hovered NEI item populates top-center tooltip");
        assert!(hover_tip.title.contains(CATALOG[3].name));

        let ent_tab = EmergenceNeiHud::filter_tab_rect(panel, 2);
        let click_ent = InputSnapshot {
            cursor: [ent_tab[0] + 8.0, ent_tab[1] + 6.0],
            screen: [1280.0, 720.0],
            left_clicked: true,
            ..InputSnapshot::default()
        };
        assert!(nei.handle_input(&click_ent, [1280.0, 720.0], false, &mut player));
        assert_eq!(nei.filter, CatalogFilter::Entities);

        let all_tab = EmergenceNeiHud::filter_tab_rect(panel, 0);
        let click_all = InputSnapshot {
            cursor: [all_tab[0] + 8.0, all_tab[1] + 6.0],
            screen: [1280.0, 720.0],
            left_clicked: true,
            ..InputSnapshot::default()
        };
        nei.handle_input(&click_all, [1280.0, 720.0], false, &mut player);
        let next_btn = EmergenceNeiHud::pager_rect(panel, true);
        let click_next = InputSnapshot {
            cursor: [next_btn[0] + 6.0, next_btn[1] + 6.0],
            screen: [1280.0, 720.0],
            left_clicked: true,
            ..InputSnapshot::default()
        };
        nei.handle_input(&click_next, [1280.0, 720.0], false, &mut player);
        assert_eq!(nei.page, 1);

        nei.sync_ui(&mut world, &sim_mut, &player, false);
        assert_eq!(
            world.get::<Active>(nei.panel.unwrap().bg),
            Some(Active(true))
        );
        assert_eq!(
            world.get::<Active>(nei.tooltip.unwrap().bg),
            Some(Active(true))
        );

        let mut press_b = InputSnapshot::default();
        let b_idx = keys::OTHER_BASE + b'B' as usize;
        press_b.keys[b_idx / 64] |= 1u64 << (b_idx % 64);
        press_b.key_edges[b_idx / 64] |= 1u64 << (b_idx % 64);
        nei.handle_input(&press_b, [1280.0, 720.0], false, &mut player);
        assert!(!nei.open);
        nei.sync_ui(&mut world, &sim_mut, &player, false);
        assert_eq!(
            world.get::<Active>(nei.panel.unwrap().bg),
            Some(Active(false))
        );
    }
}
