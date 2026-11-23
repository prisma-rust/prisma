//! Placeholder panel builders for additional editor tool windows.

use prism_ui::elements::text;
use prism_ui::prelude::*;

use crate::commands::LayoutPreset;
use crate::i18n::EditorI18n;
use crate::panels::PanelRegistry;

/// Tool windows that are not contributed by the panel registry.
pub const TOOL_WINDOWS: &[(&str, &str)] = &[
    ("console", "window.console"),
    ("game", "window.game"),
    ("profiler", "window.profiler"),
    ("lighting", "window.lighting"),
    ("audio_mixer", "window.audio_mixer"),
    ("animation", "window.animation"),
    ("package_manager", "window.package_manager"),
    ("project_settings", "window.project_settings"),
    ("build_settings", "window.build_settings"),
];

/// Register every tool window's metadata so menus and the dock can address it.
pub fn register_tool_windows(layout: &mut WindowLayout, i18n: &EditorI18n) {
    for (id, title_key) in TOOL_WINDOWS {
        layout.register_window(
            EditorWindowMeta::new(*id, i18n.t(title_key)).with_closable(true),
        );
    }
}

/// Refresh tool window titles after a locale change.
pub fn sync_tool_window_titles(layout: &mut WindowLayout, i18n: &EditorI18n) {
    for (id, title_key) in TOOL_WINDOWS {
        if let Some(meta) = layout.windows.get_mut(&WindowId::from(*id)) {
            meta.title = i18n.t(title_key).to_string();
        }
    }
}

/// Build the default panel layout plus one of the saved arrangement presets.
pub fn build_layout(
    preset: LayoutPreset,
    panels: &PanelRegistry,
    i18n: &EditorI18n,
    about_close_key: NodeKey,
) -> WindowLayout {
    let mut layout = panels.build_default_layout(i18n, about_close_key);
    register_tool_windows(&mut layout, i18n);
    match preset {
        LayoutPreset::Default => {}
        LayoutPreset::Wide => layout.main_root = wide_root(&mut layout),
        LayoutPreset::Tall => layout.main_root = tall_root(&mut layout),
    }
    layout
}

fn wide_root(layout: &mut WindowLayout) -> DockNode {
    let scene_area = layout.alloc_area_id();
    let side_area = layout.alloc_area_id();
    let bottom_area = layout.alloc_area_id();
    let v_split = layout.alloc_split_id();
    let h_split = layout.alloc_split_id();
    DockNode::Split(SplitView {
        id: h_split,
        axis: SplitAxis::Horizontal,
        ratio: 0.74,
        first: Box::new(DockNode::Split(SplitView {
            id: v_split,
            axis: SplitAxis::Vertical,
            ratio: 0.78,
            first: Box::new(DockNode::Area(DockArea::new(
                scene_area,
                vec!["scene".into(), "game".into()],
            ))),
            second: Box::new(DockNode::Area(DockArea::new(
                bottom_area,
                vec!["assets".into(), "console".into()],
            ))),
        })),
        second: Box::new(DockNode::Area(DockArea::new(
            side_area,
            vec!["inspector".into(), "hierarchy".into()],
        ))),
    })
}

fn tall_root(layout: &mut WindowLayout) -> DockNode {
    let scene_area = layout.alloc_area_id();
    let mid_area = layout.alloc_area_id();
    let inspector_area = layout.alloc_area_id();
    let h_split_1 = layout.alloc_split_id();
    let h_split_2 = layout.alloc_split_id();
    DockNode::Split(SplitView {
        id: h_split_1,
        axis: SplitAxis::Horizontal,
        ratio: 0.54,
        first: Box::new(DockNode::Area(DockArea::new(
            scene_area,
            vec!["scene".into(), "game".into()],
        ))),
        second: Box::new(DockNode::Split(SplitView {
            id: h_split_2,
            axis: SplitAxis::Horizontal,
            ratio: 0.45,
            first: Box::new(DockNode::Area(DockArea::new(
                mid_area,
                vec!["hierarchy".into(), "assets".into(), "console".into()],
            ))),
            second: Box::new(DockNode::Area(DockArea::new(
                inspector_area,
                vec!["inspector".into()],
            ))),
        })),
    })
}

/// Render a placeholder tool window body for `window_id` if it is one of the
/// secondary tool panels; returns `None` for core panels handled by the registry.
pub fn build_tool_window(window_id: &str, theme: &Theme, i18n: &EditorI18n) -> Option<ElementBox> {
    match window_id {
        "console" => Some(build_console(theme, i18n)),
        "game" => Some(build_game(theme, i18n)),
        "profiler" => Some(build_profiler(theme, i18n)),
        "lighting" => Some(build_lighting(theme, i18n)),
        "audio_mixer" => Some(build_audio_mixer(theme, i18n)),
        "animation" => Some(build_animation(theme, i18n)),
        "package_manager" => Some(build_package_manager(theme, i18n)),
        "project_settings" => Some(build_project_settings(theme, i18n)),
        "build_settings" => Some(build_build_settings(theme, i18n)),
        _ => None,
    }
}

fn panel_shell(theme: &Theme, title: &str) -> ElementBox {
    col()
        .size(Length::Percent(100.0), Length::Percent(100.0))
        .pad(10.0)
        .gap(8.0)
        .bg(theme.panel)
        .child(text(theme.muted, theme.font_size_small, title))
}

fn info_row(theme: &Theme, label_text: &str, value_text: &str) -> ElementBox {
    row()
        .width(Length::Percent(100.0))
        .padding(Edges::symmetric(8.0, 4.0))
        .radius(4.0)
        .bg(theme.panel_alt)
        .justify(Justify::SpaceBetween)
        .align(Align::Center)
        .child(text(theme.text_soft, theme.font_size_small, label_text))
        .child(text(theme.text, theme.font_size_small, value_text))
}

fn build_console(theme: &Theme, i18n: &EditorI18n) -> ElementBox {
    panel_shell(theme, i18n.t("window.console"))
        .child(
            row()
                .gap(6.0)
                .child(button("console:clear", "Clear"))
                .child(toggle_button("console:info", "Info (2)", true))
                .child(toggle_button("console:warn", "Warn (1)", true))
                .child(toggle_button("console:error", "Error (0)", true)),
        )
        .child(info_row(
            theme,
            "[Info] Renderer initialized (Vulkan)",
            "00:00.12",
        ))
        .child(info_row(
            theme,
            "[Info] Loaded scene scenes/main.scene.json",
            "00:00.28",
        ))
        .child(info_row(
            theme,
            "[Warn] Fallback texture active on slot 0",
            "00:00.31",
        ))
}

fn build_game(theme: &Theme, i18n: &EditorI18n) -> ElementBox {
    panel_shell(theme, i18n.t("window.game"))
        .child(
            row()
                .gap(8.0)
                .align(Align::Center)
                .child(text(
                    theme.text_soft,
                    theme.font_size_small,
                    "16:9 (1280x720)",
                ))
                .child(text(theme.muted, theme.font_size_small, "Scale 1.0x")),
        )
        .child(
            col()
                .width(Length::Percent(100.0))
                .grow(1.0)
                .align(Align::Center)
                .justify(Justify::Center)
                .radius(6.0)
                .border(1.0, theme.border_subtle)
                .bg(theme.bg)
                .child(text(theme.muted, theme.font_size, "MainCamera Preview")),
        )
}

fn build_profiler(theme: &Theme, i18n: &EditorI18n) -> ElementBox {
    panel_shell(theme, i18n.t("window.profiler"))
        .child(info_row(theme, "CPU Frame", "2.41 ms"))
        .child(info_row(theme, "GPU Frame", "4.18 ms"))
        .child(info_row(theme, "UI Overlay", "0.38 ms"))
        .child(info_row(theme, "Draw Calls", "142"))
        .child(info_row(theme, "VRAM Allocated", "312 MB"))
}

fn build_lighting(theme: &Theme, i18n: &EditorI18n) -> ElementBox {
    panel_shell(theme, i18n.t("window.lighting"))
        .child(info_row(theme, "Skybox Material", "Procedural Sky"))
        .child(info_row(theme, "Sun Source", "SunLight"))
        .child(info_row(theme, "GI Mode", "SHaRC + ReSTIR"))
        .child(info_row(theme, "Ambient Intensity", "1.00"))
        .child(info_row(theme, "Fog", "Disabled"))
}

fn build_audio_mixer(theme: &Theme, i18n: &EditorI18n) -> ElementBox {
    panel_shell(theme, i18n.t("window.audio_mixer"))
        .child(info_row(theme, "Master", "0.0 dB"))
        .child(info_row(theme, "Music", "-3.5 dB"))
        .child(info_row(theme, "SFX", "-1.2 dB"))
        .child(info_row(theme, "Ambience", "-6.0 dB"))
}

fn build_animation(theme: &Theme, i18n: &EditorI18n) -> ElementBox {
    panel_shell(theme, i18n.t("window.animation"))
        .child(
            row()
                .gap(6.0)
                .child(button("anim:play", "Preview"))
                .child(text(
                    theme.text_soft,
                    theme.font_size_small,
                    "00:00.00 / 00:02.50",
                )),
        )
        .child(info_row(theme, "LocalTransform.position", "0 -> 60 frames"))
        .child(info_row(theme, "LocalTransform.rotation", "0 -> 60 frames"))
}

fn build_package_manager(theme: &Theme, i18n: &EditorI18n) -> ElementBox {
    panel_shell(theme, i18n.t("window.package_manager"))
        .child(info_row(theme, "prism-render", "0.1.0 (Installed)"))
        .child(info_row(theme, "prism-physics", "0.1.0 (Installed)"))
        .child(info_row(theme, "prism-audio", "0.1.0 (Installed)"))
        .child(info_row(theme, "prism-ui", "0.1.0 (Installed)"))
}

fn build_project_settings(theme: &Theme, i18n: &EditorI18n) -> ElementBox {
    panel_shell(theme, i18n.t("window.project_settings"))
        .child(info_row(theme, "Graphics Backend", "Vulkan 1.3"))
        .child(info_row(theme, "Fixed Timestep", "0.01667 s (60 Hz)"))
        .child(info_row(theme, "Color Space", "Linear"))
        .child(info_row(theme, "Input System", "Raw + Action Map"))
}

fn build_build_settings(theme: &Theme, i18n: &EditorI18n) -> ElementBox {
    panel_shell(theme, i18n.t("window.build_settings"))
        .child(info_row(theme, "Target Platform", "Windows x86_64"))
        .child(info_row(theme, "Entry Scene", "scenes/main.scene.json"))
        .child(info_row(theme, "Compression", "LZ4"))
        .child(button("build:run", "Build"))
}
