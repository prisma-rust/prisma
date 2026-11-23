//! The About dialog: editor identity, shown on a frosted-glass card.
//!
//! This is editor policy, not a UI-kernel widget: it names the product, lists
//! what is running and decides its own layout. The kernel only supplies the
//! generic pieces it is built from — a [`Material`] for the glass surface and
//! the usual primitives for the rows.

use glam::Vec2;
use prism_ui::elements::text;
use prism_ui::prelude::*;

use crate::i18n::EditorI18n;

/// Node key of the dialog's outer panel.
pub const ABOUT_PANEL_KEY: &str = "about:panel";
/// Node key of the dismiss button.
pub const ABOUT_CLOSE_KEY: &str = "about:close";

/// Blur radius of the dialog's glass surface, in pixels (mirrors [`Theme::glass_blur_sigma`]).
pub const ABOUT_GLASS_SIGMA: f32 = Theme::dark().glass_blur_sigma;
/// Width of the card; its content decides the height.
pub const ABOUT_WIDTH: f32 = 420.0;
/// Corner radius of the card.
pub const ABOUT_RADIUS: f32 = 14.0;

/// Rows of the dialog body, in draw order.
pub const ABOUT_FACTS: [(&str, &str); 4] = [
    ("Version", env!("CARGO_PKG_VERSION")),
    ("Renderer", "Vulkan"),
    ("UI", "prism-ui"),
    ("License", "MIT OR Apache-2.0"),
];

/// Backdrop tint of the glass card for `theme`.
///
/// The editor decides this because "how opaque may glass be before body text
/// loses contrast" is a product decision; the kernel only knows how to blur.
fn glass_tint(theme: &Theme) -> Color {
    theme.glass_tint
}

/// Default floating window bounds for About, centred in `viewport`.
pub fn about_default_rect(theme: &Theme, viewport: Vec2) -> Rect {
    let width = ABOUT_WIDTH.min((viewport.x - 48.0).max(260.0));
    let height = 276.0;
    let left = ((viewport.x - width) * 0.5).max(0.0);
    let top = (viewport.y * 0.5 - 160.0).max(theme.menu_bar_height + 24.0);
    Rect::new(left, top, width, height)
}

/// Build the interior content of the About window using `i18n`.
pub fn build_about_body_localized(theme: &Theme, i18n: &EditorI18n) -> ElementBox {
    let fact_labels = [
        (i18n.t("about.version"), env!("CARGO_PKG_VERSION")),
        (i18n.t("about.renderer"), "Vulkan (SDF + Dual-Kawase)"),
        (i18n.t("about.ui"), "prism-ui"),
        (i18n.t("about.license"), "MIT OR Apache-2.0"),
    ];
    let mut facts = col().width(Length::Percent(100.0)).gap(6.0);
    for (name, value) in fact_labels {
        facts = facts.child(
            row()
                .width(Length::Percent(100.0))
                .gap(12.0)
                .align(Align::Center)
                .child(text(theme.muted, theme.font_size_small, name).width(Length::Px(88.0)))
                .child(text(theme.text, theme.font_size, value)),
        );
    }

    col()
        .key(ABOUT_PANEL_KEY)
        .width(Length::Percent(100.0))
        .padding(Edges::symmetric(24.0, 20.0))
        .gap(14.0)
        .child(
            text(theme.text_strong, theme.font_size + 6.0, "Prisma Editor")
                .text_shadow(theme.text_shadow_offset, theme.text_shadow_color),
        )
        .child(text(
            theme.text_soft,
            theme.font_size,
            i18n.t("about.subtitle"),
        ))
        .child(
            ElementBox::default()
                .width(Length::Percent(100.0))
                .height(Length::Px(1.0))
                .bg(theme.border_subtle),
        )
        .child(facts)
}

/// Build the interior content of the About window (for mounting inside a
/// [`ContainerWindow`](prism_ui::window::ContainerWindow) or [`build_about_dialog`]).
pub fn build_about_body(theme: &Theme) -> ElementBox {
    build_about_body_localized(theme, &EditorI18n::default())
}

/// Build the standalone About dialog card, centred in `viewport`.
pub fn build_about_dialog(theme: &Theme, viewport: Vec2) -> ElementBox {
    let rect = about_default_rect(theme, viewport);
    let glass = Material::frosted_glass(ABOUT_GLASS_SIGMA).with_radius(ABOUT_RADIUS);

    col()
        .absolute(rect.x, rect.y)
        .width(Length::Px(rect.w))
        .radius(ABOUT_RADIUS)
        .border(1.0, theme.border)
        // The fill is the glass tint: the shader mixes the blurred backdrop
        // toward it, so the same colour reads as "how visible is the scene
        // through this card".
        .bg(glass_tint(theme))
        .material(glass)
        .child(build_about_body(theme))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(theme: &Theme, viewport: Vec2) -> UiDrawList {
        let mut tree = UiTree::new();
        tree.begin_frame();
        {
            let mut ctx = UiContext::new(&mut tree);
            ctx.mount(build_about_dialog(theme, viewport));
        }
        tree.compute_layout(viewport);
        let mut list = UiDrawList::default();
        tree.build_draw_list(&mut list);
        list
    }

    fn glass_quad(list: &UiDrawList) -> Option<(Rect, Material)> {
        list.commands.iter().find_map(|cmd| match cmd {
            UiDrawCommand::Quad {
                rect,
                material: Some(material),
                ..
            } if material.needs_backdrop() => Some((*rect, material.clone())),
            _ => None,
        })
    }

    #[test]
    fn the_dialog_paints_a_frosted_surface() {
        let theme = Theme::dark();
        let list = render(&theme, Vec2::new(1280.0, 720.0));
        let (_, material) = glass_quad(&list).expect("the dialog paints a glass surface");
        assert_eq!(material.shader, ShaderRef::FrostedGlass);
        assert_eq!(material.backdrop_sigma, ABOUT_GLASS_SIGMA);
    }

    #[test]
    fn the_card_stays_inside_a_narrow_viewport() {
        let viewport = Vec2::new(360.0, 640.0);
        let list = render(&Theme::dark(), viewport);
        let (rect, _) = glass_quad(&list).expect("glass surface");
        assert!(rect.x >= 0.0, "card starts off-screen: {rect:?}");
        assert!(
            rect.x + rect.w <= viewport.x + 0.5,
            "card overflows the viewport: {rect:?}"
        );
    }

    #[test]
    fn light_and_dark_glass_share_their_geometry() {
        let viewport = Vec2::new(1280.0, 720.0);
        let dark = glass_quad(&render(&Theme::dark(), viewport)).unwrap().0;
        let light = glass_quad(&render(&Theme::light(), viewport)).unwrap().0;
        assert_eq!(dark, light, "a theme switch must not relayout");
    }
}
