//! Theme: map data-layer tokens (same set as Web editor) to egui.
//!
//! Colors defined once in data layer, egui side does not hardcode color values (consistent with Web editor's
//! `style.css` consistent).
//!
//! Visual rules (aligned with `editor/src/style.css`):
//! - Surfaces are **neutral gray**: menu/status bar `chrome`, panel `panel`, tab bar `bg-sunken`,
//!   floating window `panel-raised`; blue appears only in **selected / pressed / focused** (`selection` /
//!   `accent`), do not lay blue background on ordinary controls.
//! - Right angles (`CornerRadius::ZERO`) as main, only popup menus 3px; 1px divider uses `border`.
//! - Controls do not hover-expand (`expansion = 0`), desktop / Web do not jitter.

use egui::{Color32, CornerRadius, FontId, Stroke};
use prism_editor_core::{ThemeColors, ThemeToken};

pub fn color32(colors: &ThemeColors, token: ThemeToken) -> Color32 {
    let rgba = colors.color(token);
    Color32::from_rgba_unmultiplied(rgba.r, rgba.g, rgba.b, rgba.a)
}

const RADIUS: CornerRadius = CornerRadius::same(3);
const POPUP_RADIUS: CornerRadius = CornerRadius::same(4);

/// Apply the compact, studio dark editor visual style and spacing metrics.
pub fn apply(ctx: &egui::Context, colors: &ThemeColors) {
    let panel = color32(colors, ThemeToken::Panel);
    let panel_alt = color32(colors, ThemeToken::PanelAlt);
    let panel_raised = color32(colors, ThemeToken::PanelRaised);
    let sunken = color32(colors, ThemeToken::BgSunken);
    let border = color32(colors, ThemeToken::Border);
    let border_strong = color32(colors, ThemeToken::BorderStrong);
    let border_soft = color32(colors, ThemeToken::BorderSubtle);
    let text = color32(colors, ThemeToken::Text);
    let text_strong = color32(colors, ThemeToken::TextStrong);
    let text_soft = color32(colors, ThemeToken::TextSoft);
    let muted = color32(colors, ThemeToken::Muted);
    let row_hover = color32(colors, ThemeToken::RowHover);
    let menu_hover = color32(colors, ThemeToken::MenuHover);
    let menu_active = color32(colors, ThemeToken::MenuActive);
    let accent = color32(colors, ThemeToken::Accent);
    let accent_solid = color32(colors, ThemeToken::AccentSolid);
    let selection = color32(colors, ThemeToken::Selection);
    let on_accent = color32(colors, ThemeToken::OnAccent);

    let mut visuals = egui::Visuals::dark();

    visuals.panel_fill = panel;
    visuals.window_fill = panel_raised;
    visuals.extreme_bg_color = sunken;
    visuals.faint_bg_color = panel_alt;
    visuals.code_bg_color = sunken;
    visuals.window_stroke = Stroke::new(1.0_f32, border_strong);
    visuals.window_corner_radius = POPUP_RADIUS;
    visuals.menu_corner_radius = POPUP_RADIUS;
    visuals.window_shadow = egui::Shadow::NONE;
    visuals.popup_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(140),
    };
    visuals.striped = false;
    visuals.override_text_color = Some(text);
    visuals.hyperlink_color = color32(colors, ThemeToken::AccentAlt);
    visuals.text_cursor = egui::style::TextCursorStyle {
        stroke: Stroke::new(1.0_f32, text_strong),
        ..Default::default()
    };
    visuals.selection.bg_fill = selection;
    visuals.selection.stroke = Stroke::NONE;
    visuals.slider_trailing_fill = true;

    let widgets = &mut visuals.widgets;

    widgets.noninteractive.bg_fill = panel;
    widgets.noninteractive.weak_bg_fill = panel;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, border);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, text_soft);
    widgets.noninteractive.corner_radius = RADIUS;
    widgets.noninteractive.expansion = 0.0;

    widgets.inactive.bg_fill = panel_raised;
    widgets.inactive.weak_bg_fill = panel_raised;
    widgets.inactive.bg_stroke = Stroke::new(1.0_f32, border_strong);
    widgets.inactive.fg_stroke = Stroke::new(1.0_f32, text);
    widgets.inactive.corner_radius = RADIUS;
    widgets.inactive.expansion = 0.0;

    widgets.hovered.bg_fill = menu_hover;
    widgets.hovered.weak_bg_fill = menu_hover;
    widgets.hovered.bg_stroke = Stroke::new(1.0_f32, border_strong);
    widgets.hovered.fg_stroke = Stroke::new(1.0_f32, text_strong);
    widgets.hovered.corner_radius = RADIUS;
    widgets.hovered.expansion = 0.0;

    widgets.active.bg_fill = accent_solid;
    widgets.active.weak_bg_fill = accent_solid;
    widgets.active.bg_stroke = Stroke::new(1.0_f32, accent_solid);
    widgets.active.fg_stroke = Stroke::new(1.0_f32, on_accent);
    widgets.active.corner_radius = RADIUS;
    widgets.active.expansion = 0.0;

    widgets.open.bg_fill = menu_active;
    widgets.open.weak_bg_fill = menu_active;
    widgets.open.bg_stroke = Stroke::new(1.0_f32, border_strong);
    widgets.open.fg_stroke = Stroke::new(1.0_f32, text_strong);
    widgets.open.corner_radius = RADIUS;
    widgets.open.expansion = 0.0;

    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (egui::TextStyle::Small, FontId::proportional(10.0)),
        (egui::TextStyle::Body, FontId::proportional(12.0)),
        (egui::TextStyle::Button, FontId::proportional(12.0)),
        (egui::TextStyle::Heading, FontId::proportional(13.0)),
        (egui::TextStyle::Monospace, FontId::monospace(11.0)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(8.0, 3.0);
    style.spacing.menu_margin = egui::Margin::same(4);
    style.spacing.indent = 12.0;
    style.spacing.interact_size.y = 22.0;
    style.spacing.combo_width = 160.0;
    style.spacing.scroll.floating = false;
    style.spacing.scroll.foreground_color = false;
    style.spacing.scroll.bar_width = 9.0;
    style.spacing.scroll.bar_inner_margin = 4.0;
    style.spacing.scroll.bar_outer_margin = 0.0;
    style.spacing.scroll.handle_min_length = 20.0;
    style.animation_time = 0.06;
    let _ = (muted, row_hover, accent, border_soft);
    ctx.set_style(style);
}

/// Top / bottom chrome bar (menu bar, toolbar, status bar) bottom: `chrome` fill.
pub fn chrome_fill(colors: &ThemeColors) -> Color32 {
    color32(colors, ThemeToken::Chrome)
}

/// Ordinary panel bottom (consistent with egui `panel_fill`).
pub fn panel_fill(colors: &ThemeColors) -> Color32 {
    color32(colors, ThemeToken::Panel)
}

/// 1px divider / border color.
pub fn border(colors: &ThemeColors) -> Color32 {
    color32(colors, ThemeToken::Border)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_installs_editor_visuals_and_metrics() {
        let ctx = egui::Context::default();
        let colors = ThemeColors::preset("studio");
        apply(&ctx, &colors);
        let visuals = ctx.style().visuals.clone();
        assert_eq!(visuals.window_corner_radius, CornerRadius::same(4));
        assert_eq!(
            visuals.widgets.inactive.corner_radius,
            CornerRadius::same(3)
        );
        assert_eq!(ctx.style().spacing.interact_size.y, 22.0);
        assert_eq!(panel_fill(&colors), color32(&colors, ThemeToken::Panel));
        assert_eq!(chrome_fill(&colors), color32(&colors, ThemeToken::Chrome));
    }

    #[test]
    fn font_installation_is_safe_without_system_font_files() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        assert!(ctx.fonts(|fonts| !fonts.families().is_empty()));
    }
}

/// Font: prefer system UI font (close to Web editor system sans-serif),
/// then add CJK fallback (egui built-in font has no Chinese).
///
/// When none found keep egui built-in font, does not affect function.
pub fn install_fonts(ctx: &egui::Context) {
    if std::env::var("PRISMA_EDITOR_NO_CJK").is_ok() {
        return;
    }

    const UI_SANS: &[&str] = &[
        "C:/Windows/Fonts/segoeui.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/System/Library/Fonts/Helvetica.ttc",
    ];
    const MONO: &[&str] = &[
        "C:/Windows/Fonts/consola.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
        "/System/Library/Fonts/Menlo.ttc",
    ];
    const CJK: &[&str] = &[
        "/system/fonts/NotoSansCJK-Regular.ttc",
        "/system/fonts/NotoSansCJKsc-Regular.otf",
        "/system/fonts/DroidSansFallback.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/simhei.ttf",
        "/System/Library/Fonts/PingFang.ttc",
    ];

    fn first_existing(paths: &'static [&'static str]) -> Option<(&'static str, Vec<u8>)> {
        paths
            .iter()
            .find_map(|path| std::fs::read(path).ok().map(|bytes| (*path, bytes)))
    }

    let mut fonts = egui::FontDefinitions::default();
    let mut loaded = 0;
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);

    if let Some((path, bytes)) = first_existing(UI_SANS) {
        fonts.font_data.insert(
            "ui-sans".to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "ui-sans".to_owned());
        loaded += 1;
        log::info!("UI sans font: {path}");
    }
    if let Some((path, bytes)) = first_existing(MONO) {
        fonts.font_data.insert(
            "ui-mono".to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .insert(0, "ui-mono".to_owned());
        loaded += 1;
        log::info!("UI mono font: {path}");
    }
    if let Some((path, bytes)) = first_existing(CJK) {
        fonts.font_data.insert(
            "cjk".to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push("cjk".to_owned());
        }
        loaded += 1;
        log::info!("CJK fallback font: {path}");
    }

    if loaded == 0 {
        log::debug!("no system fonts found; using egui built-in fonts");
    }
    ctx.set_fonts(fonts);
}
