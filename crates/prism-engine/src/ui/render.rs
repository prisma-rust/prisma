//! UI Render System — collect UI draw commands
//!
//! Each frame queries all entities with `Node + ComputedLayout`, generates ordered `UiDrawList`.
//! `UiDrawList` is set into `World` Resource for `GraphRenderer` UI Pass consumption.

use crate::ui::components::*;
use crate::ui::{is_ui_active, ScreenSize};
use prism_ecs::World;

// ── draw commands ─────────────────────────────────────────────────

/// A rectangle with background color (corner radius handled by shader).
#[derive(Clone, Copy, Debug)]
pub struct UiQuad {
    /// Screen-space rect `[left, top, width, height]`
    pub rect: [f32; 4],
    /// RGBA color 0..=1, interpreted as **sRGB** author value (decoded to linear before render, #592).
    pub color: [f32; 4],
    /// Corner radius (px)
    pub border_radius: f32,
    /// Background texture bindless slot (#591); `None` = solid fill.
    pub texture: Option<u32>,
    /// Backdrop blur radius in pixels; `0.0` means the background is not sampled.
    ///
    /// When a quad needs a blur, the host filters the composited scene once and the
    /// UI shader samples the result. The surface declares this through its material
    /// rather than through a per-widget toggle bit.
    pub backdrop_sigma: f32,
    /// Draw layer (z-index). Within layer sorted by [`UiQuad::order`].
    ///
    /// Normal UI nodes are 0; things needing "always above some layer" (rain particles, debug
    /// overlay) explicitly write larger value.
    pub layer: i32,
    /// Draw order within layer, from [`ComputedLayout::order`].
    pub order: u32,
}

impl Default for UiQuad {
    fn default() -> Self {
        Self {
            rect: [0.0; 4],
            color: [0.0; 4],
            border_radius: 0.0,
            texture: None,
            backdrop_sigma: 0.0,
            layer: 0,
            order: EXTERNAL_UI_ORDER,
        }
    }
}

/// One text draw command.
#[derive(Clone, Debug, Default)]
pub struct UiTextCmd {
    pub rect: [f32; 4],
    pub content: String,
    pub font_size: f32,
    /// RGBA sRGB author value (same as [`UiQuad::color`]).
    pub color: [f32; 4],
    pub alignment: TextAlign,
    /// Project-bundled font asset path; empty = system default font (#583).
    pub font: String,
    /// Wrap by rect width (#527); `false` = single line.
    pub wrap: bool,
    /// Optional SDF outline width in pixels (`0.0` = no outline).
    pub outline_width: f32,
    /// SDF outline RGBA color (sRGB).
    pub outline_color: [f32; 4],
    /// Optional drop-shadow offset in pixels.
    pub shadow_offset: [f32; 2],
    /// Drop-shadow RGBA color (sRGB, alpha `0.0` = no shadow).
    pub shadow_color: [f32; 4],
    /// See [`UiQuad::layer`].
    pub layer: i32,
    /// See [`UiQuad::order`].
    pub order: u32,
}

/// Draw commands without layer source (externally submitted quad / particles) sorted after all UI entities.
pub const EXTERNAL_UI_ORDER: u32 = u32::MAX;

/// Layer for externally submitted commands: above all ECS UI entities (#462).
///
/// Hosts omit layer; merging stamps this value so quads and text share one layer
/// and submission order decides draw order.
pub const EXTERNAL_LAYER: i32 = 9_500;

/// One draw command.
///
/// Background rect and text in **same** list, sorted by `(layer, order, type)` then submitted
/// — this is what UGUI does in Canvas (#462): text not unconditionally above all backgrounds,
/// later full-screen mask can cover earlier title.
#[derive(Clone, Debug)]
pub enum UiDrawItem {
    Quad(UiQuad),
    Text(UiTextCmd),
}

/// UI draw command list — already sorted by draw order, for render Pass ordered consumption.
#[derive(Clone, Debug, Default)]
pub struct UiDrawList {
    pub items: Vec<UiDrawItem>,
}

impl UiDrawList {
    pub fn push_quad(&mut self, quad: UiQuad) {
        self.items.push(UiDrawItem::Quad(quad));
    }

    pub fn push_text(&mut self, text: UiTextCmd) {
        self.items.push(UiDrawItem::Text(text));
    }

    pub fn extend_quads(&mut self, quads: impl IntoIterator<Item = UiQuad>) {
        self.items.extend(quads.into_iter().map(UiDrawItem::Quad));
    }

    pub fn quads(&self) -> impl Iterator<Item = &UiQuad> {
        self.items.iter().filter_map(|item| match item {
            UiDrawItem::Quad(quad) => Some(quad),
            UiDrawItem::Text(_) => None,
        })
    }

    pub fn texts(&self) -> impl Iterator<Item = &UiTextCmd> {
        self.items.iter().filter_map(|item| match item {
            UiDrawItem::Quad(_) => None,
            UiDrawItem::Text(text) => Some(text),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }
}

/// Externally (host / project module) submitted UI quad.
///
/// Items are [`prism_ui_draw::UiQuad`] so hosts submit the same struct used on the
/// render side without placeholder layer/order values. `ui_render_system` rebuilds
/// `UiDrawList` every frame, so callers must insert this before UI rendering; the
/// system merges and drains it each frame.
#[derive(Clone, Debug, Default)]
pub struct ExternalUiQuads(pub Vec<prism_ui_draw::UiQuad>);

/// Externally submitted UI quads that specify an explicit non-zero layer offset above `EXTERNAL_LAYER`.
#[derive(Clone, Debug, Default)]
pub struct ExternalUiLayeredQuads(pub Vec<UiQuad>);

/// Externally (host / project module) submitted immediate UI text commands.
#[derive(Clone, Debug, Default)]
pub struct ExternalUiTexts(pub Vec<UiTextCmd>);

/// Canvas scale of layout node (#463); bad value treated as 1, avoid NaN spreading to vertices.
fn layout_scale(layout: &ComputedLayout) -> f32 {
    if layout.scale.is_finite() && layout.scale > 0.0 {
        layout.scale
    } else {
        1.0
    }
}

/// Update `UiDrawList` resource each frame — query all UI entities generate draw commands.
pub fn ui_render_system(world: &mut World) {
    // (layer, order, type: 0 = rect / 1 = text, sequence) — sequence makes sort stable.
    let mut pending: Vec<((i32, u32, u8, u32), UiDrawItem)> = Vec::new();

    // Merge externally submitted quads and texts first; sorting places them in EXTERNAL_LAYER
    // (plus any relative layer offset) at EXTERNAL_UI_ORDER after all ECS UI entities.
    if let Some(external) = world.remove_resource::<ExternalUiQuads>() {
        for (seq, quad) in external.0.into_iter().enumerate() {
            pending.push((
                (EXTERNAL_LAYER, EXTERNAL_UI_ORDER, 0, seq as u32),
                UiDrawItem::Quad(UiQuad {
                    rect: quad.rect,
                    color: quad.color,
                    border_radius: quad.border_radius,
                    texture: quad.texture,
                    backdrop_sigma: quad.backdrop_sigma,
                    layer: EXTERNAL_LAYER,
                    order: EXTERNAL_UI_ORDER,
                }),
            ));
        }
    }
    if let Some(layered) = world.remove_resource::<ExternalUiLayeredQuads>() {
        for (seq, mut quad) in layered.0.into_iter().enumerate() {
            quad.order = EXTERNAL_UI_ORDER;
            pending.push((
                (quad.layer, quad.order, 0, seq as u32),
                UiDrawItem::Quad(quad),
            ));
        }
    }
    if let Some(texts) = world.remove_resource::<ExternalUiTexts>() {
        for (seq, mut text) in texts.0.into_iter().enumerate() {
            text.order = EXTERNAL_UI_ORDER;
            pending.push((
                (text.layer, text.order, 1, seq as u32),
                UiDrawItem::Text(text),
            ));
        }
    }

    // Collect background rects
    for (entity, layout, style) in world.query2::<ComputedLayout, Style>() {
        if !is_ui_active(world, entity) {
            continue;
        }
        if !style.visible {
            continue;
        }
        // #528: `Button` changes color by interaction state, without `Button` continue using
        // `Style::background` (both can coexist, button first).
        let texture = world
            .get::<crate::scene::runtime_components::UiImageRef>(entity)
            .filter(|image| image.generation > 0)
            .map(|image| image.texture_slot);
        let mut bg = match world.get::<Button>(entity) {
            Some(button) => button.color(world.get::<Interaction>(entity)),
            None => style.background,
        };
        // #591: with background texture `background` only as tint; if alpha 0 means author did not write
        // tint, display texture original colors as opaque white.
        if texture.is_some() && bg[3] <= 0.0 {
            bg = [1.0, 1.0, 1.0, 1.0];
        }
        if bg[3] <= 0.0 && texture.is_none() {
            continue;
        } // fully transparent skip

        let seq = pending.len() as u32;
        // #463: corner radius is "design pixels", convert to screen pixels by containing Canvas scale.
        let scale = layout_scale(layout);
        pending.push((
            (0, layout.order, 0, seq),
            UiDrawItem::Quad(UiQuad {
                rect: layout.rect,
                color: bg,
                border_radius: style.border_radius * scale,
                texture,
                backdrop_sigma: 0.0,
                layer: 0,
                order: layout.order,
            }),
        ));
    }

    // Collect text
    for (entity, layout, text) in world.query2::<ComputedLayout, Text>() {
        if !is_ui_active(world, entity) {
            continue;
        }
        // Consistent with quad above: `Style::visible == false` entities produce no draw commands.
        // Text entities usually also carry `Style` (`Node + Style + Text`), without `Style` treated visible.
        if matches!(world.get::<Style>(entity), Some(style) if !style.visible) {
            continue;
        }
        if text.content.is_empty() {
            continue;
        }

        let seq = pending.len() as u32;
        let scale = layout_scale(layout);
        pending.push((
            (0, layout.order, 1, seq),
            UiDrawItem::Text(UiTextCmd {
                rect: layout.rect,
                content: text.content.clone(),
                // #463: font size likewise by Canvas scale, otherwise text on 4K screen half size.
                font_size: text.font_size * scale,
                color: text.color,
                alignment: text.alignment,
                font: text.font.clone(),
                wrap: text.wrap,
                outline_width: text.outline_width * scale,
                outline_color: text.outline_color,
                shadow_offset: [text.shadow_offset[0] * scale, text.shadow_offset[1] * scale],
                shadow_color: text.shadow_color,
                layer: 0,
                order: layout.order,
            }),
        ));
    }

    // First by layer, then layer order, finally "background before text within same node".
    pending.sort_by_key(|(key, _)| *key);

    let mut draw_list = UiDrawList {
        items: Vec::with_capacity(pending.len()),
    };
    draw_list
        .items
        .extend(pending.into_iter().map(|(_, item)| item));

    world.insert_resource(draw_list);
}

/// Merge externally submitted quads during scheduling into already-built [`UiDrawList`].
///
/// [`ui_render_system`] builds list during scheduling (`ui::render`), project systems usually
/// register after it; `Engine::update` merges once at schedule end, so static Player
/// `GameWorld::draw_quad` submissions are not one frame late (#1048). Editor writes before
/// `engine.update()`, goes through that `ui_render_system` merge.
pub fn merge_external_quads(world: &mut World) {
    let external = world.remove_resource::<ExternalUiQuads>();
    let layered = world.remove_resource::<ExternalUiLayeredQuads>();
    let texts = world.remove_resource::<ExternalUiTexts>();
    let has_quads = external.as_ref().is_some_and(|e| !e.0.is_empty());
    let has_layered = layered.as_ref().is_some_and(|l| !l.0.is_empty());
    let has_texts = texts.as_ref().is_some_and(|t| !t.0.is_empty());
    if !has_quads && !has_layered && !has_texts {
        return;
    }
    let mut draw_list = world.remove_resource::<UiDrawList>().unwrap_or_default();
    if let Some(external) = external {
        draw_list.extend_quads(external.0.into_iter().map(|quad| UiQuad {
            rect: quad.rect,
            color: quad.color,
            border_radius: quad.border_radius,
            texture: quad.texture,
            backdrop_sigma: quad.backdrop_sigma,
            layer: EXTERNAL_LAYER,
            order: EXTERNAL_UI_ORDER,
        }));
    }
    if let Some(layered) = layered {
        draw_list.extend_quads(layered.0.into_iter().map(|mut quad| {
            quad.order = EXTERNAL_UI_ORDER;
            quad
        }));
    }
    if let Some(texts) = texts {
        for mut text in texts.0 {
            text.order = EXTERNAL_UI_ORDER;
            draw_list.push_text(text);
        }
    }
    // List already ordered, `sort_by_key` stable, so just insert later elements at correct position.
    draw_list.items.sort_by_key(|item| match item {
        UiDrawItem::Quad(quad) => (quad.layer, quad.order, 0u8),
        UiDrawItem::Text(text) => (text.layer, text.order, 1u8),
    });
    world.insert_resource(draw_list);
}

/// Convert engine `UiDrawList` (pixel coords) to renderer `UiOverlayInput` (NDC).
///
/// Read screen size from world `ScreenSize` resource.
pub fn convert_ui_draw_list_to_overlay(world: &World) -> prism_ui_draw::UiOverlayInput {
    let mut out = prism_ui_draw::UiOverlayInput::default();
    convert_ui_draw_list_to_overlay_into(world, &mut out);
    out
}

/// [`convert_ui_draw_list_to_overlay`] into a caller-owned buffer.
///
/// The frame packet owns the `UiOverlayInput` across frames, so this refills
/// the existing `quads` / `texts` allocations instead of building new ones (#66).
pub fn convert_ui_draw_list_to_overlay_into(
    world: &World,
    out: &mut prism_ui_draw::UiOverlayInput,
) {
    out.clear();

    let screen = world
        .get_resource::<ScreenSize>()
        .copied()
        .unwrap_or(ScreenSize {
            width: 1920.0,
            height: 1080.0,
        });
    let w = screen.width;
    let h = screen.height;

    let Some(draw_list) = world.get_resource::<UiDrawList>() else {
        return;
    };

    for item in &draw_list.items {
        let q = match item {
            UiDrawItem::Quad(quad) => quad,
            UiDrawItem::Text(text) => {
                // Text commands pass through pixel coords as-is (expanded per-glyph quad when recording).
                out.push_text(prism_ui_draw::UiTextInput {
                    rect: text.rect,
                    content: text.content.clone(),
                    font_size: text.font_size,
                    color: text.color,
                    alignment: match text.alignment {
                        TextAlign::Left => prism_ui_draw::UiTextAlign::Left,
                        TextAlign::Center => prism_ui_draw::UiTextAlign::Center,
                        TextAlign::Right => prism_ui_draw::UiTextAlign::Right,
                    },
                    font: (!text.font.is_empty()).then(|| text.font.clone()),
                    wrap: text.wrap,
                    outline_width: text.outline_width,
                    outline_color: text.outline_color,
                    shadow_offset: text.shadow_offset,
                    shadow_color: text.shadow_color,
                });
                continue;
            }
        };
        let [px, py, pw, ph] = q.rect; // pixel left, top, width, height
                                       // NDC: [-1,1] x [-1,1], origin center. **Vulkan Y axis points downward**:
                                       // `y = -1` is screen top, `+1` bottom (camera projection
                                       // `p[1][1] = -inv_tan` is flipped for this convention).
                                       // Pixel y also grows downward, so here **no flip**:
                                       // flipping again would turn whole UI layer upside down.
        let x0 = (px / w) * 2.0 - 1.0;
        let y0 = (py / h) * 2.0 - 1.0;
        let x1 = ((px + pw) / w) * 2.0 - 1.0;
        let y1 = ((py + ph) / h) * 2.0 - 1.0;
        // NDC border radius: normalize by quad own size, render side multiplies back by pixel size
        // to get corner consistent with author writing (#634).
        let br_ndc = if pw.max(ph) > 0.0 {
            q.border_radius / pw.max(ph) * 2.0
        } else {
            0.0
        };
        out.push_quad(prism_ui_draw::UiQuad {
            rect: [x0, y0, x1, y1], // NDC [xmin, ymin, xmax, ymax]
            color: q.color,
            border_radius: br_ndc,
            texture: q.texture,
            backdrop_sigma: q.backdrop_sigma,
        });
    }
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
