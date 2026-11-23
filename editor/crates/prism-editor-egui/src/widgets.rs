//! Schema-driven field controls (used by inspector).
//!
//! Rules consistent with Web editor: control type entirely comes from
//! `prism-editor-core` `FieldSchema::widget()`, so new component fields do not require changing
//! here. Write-back uniformly via `Workspace::set_field` (reflection validation + undo merge).

use egui::{Response, Ui};
use egui_phosphor::regular as icons;
use prism_editor_core::viewport_math as vm;
use prism_editor_core::{FieldSchema, FieldWidget, Schema, TypeSchema};
use serde_json::Value;

/// Asset drag payload: drag from Project panel to hierarchy / inspector field / directory.
#[derive(Clone, Debug)]
pub struct AssetDrag {
    pub path: String,
}

/// Button actions on component card (collect first, applied by caller after render ends).
pub enum CardAction {
    Copy(String),
    /// Paste same-type component value from clipboard to selected entity (#418 component menu).
    Paste(String),
    Remove(String),
    /// Move component up / down in inspector order (#512).
    MoveUp(String),
    MoveDown(String),
    /// Save component current value as named preset (#418).
    SavePreset {
        component: String,
        name: String,
        value: Value,
    },
    /// Write named preset back to component (whole component = one undo step).
    ApplyPreset {
        component: String,
        name: String,
    },
}

/// One component card: header bar + field list + copy/delete buttons.
///
/// `set(field, value)` decided by caller which entity to write (single / multi-select batch).
/// Same as [`component_card`], but path field has asset picker.
/// Map a component type name to a semantic Phosphor icon glyph.
pub fn component_icon(name: &str) -> &'static str {
    if name.contains("Camera") {
        icons::CAMERA
    } else if name.contains("Light") {
        icons::LIGHTBULB
    } else if name.contains("MeshRenderer") || name.contains("SkinnedMesh") {
        icons::CUBE
    } else if name.contains("Sprite") || name.contains("Tilemap") {
        icons::IMAGE
    } else if name.contains("Audio") {
        icons::MUSIC_NOTES
    } else if name.contains("Animator") || name.contains("Animation") {
        icons::FILM_STRIP
    } else if name.contains("Collider") || name.contains("RigidBody") {
        icons::BOUNDING_BOX
    } else if name.contains("::ui::") {
        icons::SQUARES_FOUR
    } else if name.contains("Terrain") || name.contains("GiConfig") {
        icons::GLOBE
    } else {
        icons::GEAR
    }
}

pub fn component_card_with_assets(
    ui: &mut Ui,
    registry: &Schema,
    component: &TypeSchema,
    value: &Value,
    actions: &mut Vec<CardAction>,
    set: &mut dyn FnMut(&str, Value),
    assets: &[String],
    doc_dir: &str,
    presets: &[String],
    i18n: &prism_editor_core::I18n,
) {
    let collapse_id = ui.make_persistent_id(("comp-card-open", component.name.as_str()));
    let mut is_open = ui.data(|d| d.get_temp::<bool>(collapse_id)).unwrap_or(true);

    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(8, 5))
        .corner_radius(egui::CornerRadius::same(3))
        .show(ui, |ui| {
            egui::Frame::new()
                .fill(ui.visuals().faint_bg_color)
                .inner_margin(egui::Margin::symmetric(6, 3))
                .corner_radius(egui::CornerRadius::same(3))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let toggle_arrow = if is_open {
                            icons::CARET_DOWN
                        } else {
                            icons::CARET_RIGHT
                        };
                        if ui.small_button(toggle_arrow).clicked() {
                            is_open = !is_open;
                            ui.data_mut(|d| d.insert_temp(collapse_id, is_open));
                        }
                        ui.label(
                            egui::RichText::new(component_icon(&component.name))
                                .size(11.5)
                                .color(ui.visuals().text_color()),
                        );
                        ui.label(
                            egui::RichText::new(component.display_name())
                                .strong()
                                .size(12.0),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button(icons::X)
                                .on_hover_text(i18n.t1("inspector.removeComponent"))
                                .clicked()
                            {
                                actions.push(CardAction::Remove(component.name.clone()));
                            }
                            if ui
                                .small_button(icons::CARET_DOWN)
                                .on_hover_text(i18n.t1("inspector.moveDown"))
                                .clicked()
                            {
                                actions.push(CardAction::MoveDown(component.name.clone()));
                            }
                            if ui
                                .small_button(icons::CARET_UP)
                                .on_hover_text(i18n.t1("inspector.moveUp"))
                                .clicked()
                            {
                                actions.push(CardAction::MoveUp(component.name.clone()));
                            }
                            if ui
                                .small_button(icons::COPY)
                                .on_hover_text(i18n.t1("inspector.copyComponent"))
                                .clicked()
                            {
                                actions.push(CardAction::Copy(component.name.clone()));
                            }
                            ui.menu_button(icons::DOTS_THREE, |ui| {
                                if ui.button(i18n.t1("inspector.copyComponent")).clicked() {
                                    actions.push(CardAction::Copy(component.name.clone()));
                                    ui.close();
                                }
                                if ui.button(i18n.t1("inspector.pasteValues")).clicked() {
                                    actions.push(CardAction::Paste(component.name.clone()));
                                    ui.close();
                                }
                                ui.separator();
                                if ui.button(i18n.t1("inspector.moveUp")).clicked() {
                                    actions.push(CardAction::MoveUp(component.name.clone()));
                                    ui.close();
                                }
                                if ui.button(i18n.t1("inspector.moveDown")).clicked() {
                                    actions.push(CardAction::MoveDown(component.name.clone()));
                                    ui.close();
                                }
                                ui.separator();
                                let default = component.default_json.clone();
                                if ui
                                    .add_enabled(
                                        default.is_some(),
                                        egui::Button::new(i18n.t1("common.reset")),
                                    )
                                    .clicked()
                                {
                                    if let Some(default) = default {
                                        set("", default);
                                    }
                                    ui.close();
                                }
                                ui.separator();
                                ui.menu_button(i18n.t1("inspector.presets"), |ui| {
                                    ui.menu_button(i18n.t1("inspector.savePreset"), |ui| {
                                        let id =
                                            egui::Id::new(("preset-name", component.name.as_str()));
                                        let mut name = ui
                                            .data(|data| data.get_temp::<String>(id))
                                            .unwrap_or_default();
                                        if ui.text_edit_singleline(&mut name).changed() {
                                            ui.data_mut(|data| data.insert_temp(id, name.clone()));
                                        }
                                        let trimmed = name.trim().to_string();
                                        if ui
                                            .add_enabled(
                                                !trimmed.is_empty(),
                                                egui::Button::new(i18n.t1("common.save")),
                                            )
                                            .clicked()
                                        {
                                            actions.push(CardAction::SavePreset {
                                                component: component.name.clone(),
                                                name: trimmed,
                                                value: value.clone(),
                                            });
                                            ui.data_mut(|data| data.remove::<String>(id));
                                            ui.close();
                                        }
                                    });
                                    if presets.is_empty() {
                                        ui.label(
                                            egui::RichText::new(i18n.t1("inspector.noPresets"))
                                                .weak()
                                                .size(10.0),
                                        );
                                    } else {
                                        for preset in presets {
                                            if ui.button(preset).clicked() {
                                                actions.push(CardAction::ApplyPreset {
                                                    component: component.name.clone(),
                                                    name: preset.clone(),
                                                });
                                                ui.close();
                                            }
                                        }
                                    }
                                });
                                if ui.button(i18n.t1("inspector.removeComponent")).clicked() {
                                    actions.push(CardAction::Remove(component.name.clone()));
                                    ui.close();
                                }
                            });
                        });
                    });
                });
            if is_open {
                ui.add_space(4.0);
                let fields = component.fields.clone();
                for field in &fields {
                    let Some(field_value) = value.get(&field.name) else {
                        continue;
                    };
                    if field.name == "type_id" {
                        continue;
                    }
                    field_row_at(
                        ui,
                        registry,
                        field,
                        field_value,
                        "",
                        set,
                        assets,
                        doc_dir,
                        i18n,
                    );
                }
            }
            let _ = &component;
        });
}

/// Single field edit control. `set(field, value)` writes back (single goes via `set_field`,
/// multi via `set_field_multi`), so here does not care about entity.
/// Asset field extension filter (engine object field lists only acceptable types).
fn asset_filter(name: &str) -> Option<&'static [&'static str]> {
    if name == "font" || name.ends_with("_font") {
        Some(&["ttf", "otf", "ttc"])
    } else if name.contains("mesh") {
        Some(&["obj", "gltf", "glb", "fbx", "mesh"])
    } else if name.contains("sprite") || name.contains("texture") || name.ends_with("_tex") {
        Some(&["png", "jpg", "jpeg", "ktx2", "dds"])
    } else if name.contains("hdr") || name.contains("env") {
        Some(&["hdr", "exr"])
    } else if name.contains("volume") || name.contains("prefab") {
        Some(&["json"])
    } else if name.ends_with("_path") {
        Some(&[])
    } else {
        None
    }
}

fn asset_matches(name: &str, path: &str) -> bool {
    match asset_filter(name) {
        Some([]) | None => true,
        Some(extensions) => extensions
            .iter()
            .any(|extension| path.to_lowercase().ends_with(&format!(".{extension}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_fields_commit_on_enter_or_focus_loss_only() {
        assert_eq!(
            text_commit_decision(true, false, false, false, false),
            TextCommit::Keep
        );
        assert_eq!(
            text_commit_decision(true, false, true, false, false),
            TextCommit::Commit
        );
        assert_eq!(
            text_commit_decision(false, true, false, false, false),
            TextCommit::Commit
        );
        assert_eq!(
            text_commit_decision(true, false, true, true, false),
            TextCommit::Cancel
        );
        assert_eq!(
            text_commit_decision(true, false, true, false, true),
            TextCommit::Keep
        );
    }

    #[test]
    fn anchor_presets_cover_points_and_stretches() {
        let current = (0.2, 0.8, 0.2, 0.8);
        assert_eq!(
            AnchorPreset::Point { x: 0, y: 0 }.anchors(current),
            (0.0, 0.0, 0.0, 0.0)
        );
        assert_eq!(
            AnchorPreset::Point { x: 1, y: 1 }.anchors(current),
            (0.5, 0.5, 0.5, 0.5)
        );
        assert_eq!(
            AnchorPreset::Point { x: 2, y: 2 }.anchors(current),
            (1.0, 1.0, 1.0, 1.0)
        );

        assert_eq!(
            AnchorPreset::StretchHorizontal.anchors(current),
            (0.0, 0.8, 1.0, 0.8)
        );
        assert_eq!(
            AnchorPreset::StretchVertical.anchors(current),
            (0.2, 0.0, 0.2, 1.0)
        );
        assert_eq!(AnchorPreset::Stretch.anchors(current), (0.0, 0.0, 1.0, 1.0));

        let horizontal = AnchorPreset::StretchHorizontal.anchors(current);
        assert!(AnchorPreset::StretchHorizontal.is_active(horizontal));
        assert!(!AnchorPreset::Stretch.is_active(horizontal));
        assert!(AnchorPreset::Stretch.is_active((0.0, 0.0, 1.0, 1.0)));

        let value = anchors_value((0.0, 0.5, 1.0, 0.5));
        assert_eq!(value["min_x"], serde_json::json!(0.0));
        assert_eq!(value["min_y"], serde_json::json!(0.5));
        assert_eq!(value["max_x"], serde_json::json!(1.0));
        assert_eq!(value["max_y"], serde_json::json!(0.5));
        assert_eq!(anchors_tuple(&value), (0.0, 0.5, 1.0, 0.5));
    }

    #[test]
    fn asset_filters_match_expected_extensions() {
        assert!(asset_matches("mesh_path", "assets/ship.GLTF"));
        assert!(!asset_matches("mesh_path", "assets/ship.png"));
        assert!(asset_matches("sprite_path", "assets/hero.png"));
        assert!(asset_matches("font", "assets/fonts/hud.ttf"));
        assert!(!asset_matches("font", "assets/hero.png"));
        assert!(asset_matches("environment_path", "assets/sunset.exr"));
        assert!(asset_matches("custom_path", "assets/data.bin"));
        assert!(asset_matches("label", "anything.bin"));
    }

    /// A composite schema with two lists: `points: Vec<Vec3>` and `nodes: Vec<Point>`.
    fn list_schema() -> Schema {
        Schema::from_value(&serde_json::json!({
            "types": [
                {
                    "name": "test::Line",
                    "displayName": "Line",
                    "typeId": "test-line",
                    "version": 1,
                    "isComponent": true,
                    "defaultJson": {"points": [], "nodes": []},
                    "fields": [
                        {
                            "name": "points",
                            "kind": "list",
                            "structType": null,
                            "arrayLen": null,
                            "variants": null,
                            "readOnly": false,
                            "inner": {
                                "name": "",
                                "kind": "array",
                                "structType": null,
                                "arrayLen": 3,
                                "variants": null,
                                "readOnly": false,
                                "inner": {
                                    "name": "",
                                    "kind": "f32",
                                    "structType": null,
                                    "inner": null,
                                    "arrayLen": null,
                                    "variants": null,
                                    "readOnly": false
                                }
                            }
                        },
                        {
                            "name": "nodes",
                            "kind": "list",
                            "structType": null,
                            "arrayLen": null,
                            "variants": null,
                            "readOnly": false,
                            "inner": {
                                "name": "",
                                "kind": "struct",
                                "structType": "test::Point",
                                "inner": null,
                                "arrayLen": null,
                                "variants": null,
                                "readOnly": false
                            }
                        }
                    ]
                },
                {
                    "name": "test::Point",
                    "displayName": "Point",
                    "typeId": "test-point",
                    "version": 1,
                    "isComponent": false,
                    "defaultJson": null,
                    "fields": [
                        {"name": "x", "kind": "f32", "structType": null, "inner": null,
                         "arrayLen": null, "variants": null, "readOnly": false},
                        {"name": "y", "kind": "f32", "structType": null, "inner": null,
                         "arrayLen": null, "variants": null, "readOnly": false}
                    ]
                }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn field_paste_requires_the_same_json_shape() {
        assert!(values_compatible(
            &serde_json::json!([0.0, 0.0, 0.0]),
            &serde_json::json!([1.0, 2.0, 3.0])
        ));
        assert!(values_compatible(
            &serde_json::json!(1.0),
            &serde_json::json!(2.0)
        ));
        assert!(values_compatible(
            &serde_json::json!("a"),
            &serde_json::json!("b")
        ));
        assert!(values_compatible(
            &serde_json::json!(true),
            &serde_json::json!(false)
        ));
        assert!(values_compatible(
            &serde_json::json!({"x": 0}),
            &serde_json::json!({"x": 1})
        ));
        assert!(!values_compatible(
            &serde_json::json!([0.0]),
            &serde_json::json!("nope")
        ));
        assert!(!values_compatible(
            &serde_json::json!(1.0),
            &serde_json::json!(null)
        ));
    }

    #[test]
    fn list_elements_default_from_the_schema() {
        let schema = list_schema();
        let line = schema.component("test::Line").unwrap();
        let points = line
            .fields
            .iter()
            .find(|field| field.name == "points")
            .unwrap();
        assert_eq!(
            default_value_for(&schema, points.inner.as_deref().unwrap(), 0),
            serde_json::json!([0.0, 0.0, 0.0])
        );

        let nodes = line
            .fields
            .iter()
            .find(|field| field.name == "nodes")
            .unwrap();
        assert_eq!(
            default_value_for(&schema, nodes.inner.as_deref().unwrap(), 0),
            serde_json::json!({"x": 0.0, "y": 0.0})
        );

        assert_eq!(
            default_value_for(&schema, nodes.inner.as_deref().unwrap(), 99),
            Value::Null
        );
    }

    #[test]
    fn list_edits_keep_indices_in_range() {
        let mut items = vec![Value::from(1), Value::from(2), Value::from(3)];
        list_add(&mut items, Value::from(4));
        assert_eq!(items.len(), 4);

        assert!(list_move(&mut items, 0, 2));
        assert_eq!(items[0], Value::from(3));
        assert_eq!(items[2], Value::from(1));
        assert!(!list_move(&mut items, 3, 9));
        assert!(!list_move(&mut items, 1, 1));

        assert!(list_remove(&mut items, 0));
        assert!(!list_remove(&mut items, 99));
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn list_fields_render_without_writing_to_the_document() {
        let ctx = egui::Context::default();
        let schema = list_schema();
        let component = schema.component("test::Line").unwrap().clone();
        let value = serde_json::json!({
            "points": [[1.0, 2.0, 3.0]],
            "nodes": [{"x": 4.0, "y": 5.0}]
        });
        let mut writes: Vec<(String, Value)> = Vec::new();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut set = |path: &str, value: Value| writes.push((path.to_owned(), value));
                for field in &component.fields {
                    let field_value = value.get(&field.name).unwrap();
                    field_row_at(
                        ui,
                        &schema,
                        field,
                        field_value,
                        "",
                        &mut set,
                        &[],
                        "assets",
                        &prism_editor_core::I18n::default(),
                    );
                }
            });
        });
        assert!(
            writes.is_empty(),
            "only rendering, cannot write document: {writes:?}"
        );
    }

    #[test]
    fn card_actions_and_field_rows_can_render_without_gpu() {
        let ctx = egui::Context::default();
        let schema = Schema::from_value(&serde_json::json!({
            "types": [{
                "name": "Test",
                "displayName": "Test",
                "typeId": "test",
                "version": 1,
                "isComponent": true,
                "defaultJson": {"value": 0.0},
                "fields": [{
                    "name": "value",
                    "kind": "f32",
                    "structType": null,
                    "inner": null,
                    "arrayLen": null,
                    "variants": null,
                    "readOnly": false
                }]
            }]
        }))
        .unwrap();
        let component = schema.component("Test").unwrap().clone();
        let mut actions = Vec::new();
        let mut writes = Vec::new();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                component_card_with_assets(
                    ui,
                    &schema,
                    &component,
                    &serde_json::json!({"value": 1.0}),
                    &mut actions,
                    &mut |path, value| writes.push((path.to_owned(), value)),
                    &[],
                    "assets",
                    &["hero".to_string()],
                    &prism_editor_core::I18n::default(),
                );
            });
        });
        assert!(actions.is_empty());
        assert!(writes.is_empty());
    }
}

/// Expression parser connected to numeric input (`90/2`, `2*(3+4)`, `sin(30)`).
///
/// `DragValue::custom_parser` only called after user presses Enter, so normally shows number itself.
pub fn number_parser(text: &str) -> Option<f64> {
    prism_editor_core::expr::parse_number_or_expression(text)
}

fn drag_delta_from_response(ui: &Ui, response: &egui::Response, speed: f32) -> f32 {
    if !response.dragged() {
        return 0.0;
    }
    let mut delta = response.drag_delta().x;
    if delta == 0.0 {
        return 0.0;
    }
    let (shift, alt) = ui.input(|input| (input.modifiers.shift, input.modifiers.alt));
    if shift {
        delta *= 0.1;
    }
    if alt {
        delta *= 10.0;
    }
    delta * speed
}

/// Unity-inspired axis pill color (`X` #E05252, `Y` #62B850, `Z` #4A8EE8, `W` #D6AC52).
pub fn axis_badge_color(axis: usize) -> egui::Color32 {
    match axis {
        0 => egui::Color32::from_rgb(224, 82, 82),
        1 => egui::Color32::from_rgb(98, 184, 80),
        2 => egui::Color32::from_rgb(74, 142, 232),
        _ => egui::Color32::from_rgb(214, 172, 82),
    }
}

/// Draw a compact, color-coded scrubbable axis badge (`X` / `Y` / `Z` / `W`).
pub fn scrubbable_axis_badge(
    ui: &mut Ui,
    label: &str,
    axis: usize,
    value: &mut f32,
    speed: f32,
) -> bool {
    let accent = axis_badge_color(axis);
    let desired = egui::vec2(14.0, 17.0);
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::drag());
    if ui.is_rect_visible(rect) {
        let fill = if response.dragged() || response.hovered() {
            accent
        } else {
            accent.linear_multiply(0.75)
        };
        ui.painter().rect_filled(rect, 2.0_f32, fill);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::monospace(9.5),
            egui::Color32::from_rgb(18, 20, 24),
        );
    }
    let response = response.on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    let delta = drag_delta_from_response(ui, &response, speed);
    if delta == 0.0 {
        return false;
    }
    *value += delta;
    true
}

/// Draw a draggable field label, returns **this frame** drag increment (already converted by `speed` and modifiers).
///
/// Unity `EditorGUIUtility.DragZoneRect`: turns text left of numeric field into drag hotzone,
/// drag left/right to continuously adjust, no need to reach right small handle (#479). Hold Shift for fine tune (×0.1),
/// Hold Alt for coarse adjust (×10).
pub fn drag_label(ui: &mut Ui, text: &str, speed: f32) -> f32 {
    let response = ui
        .add(egui::Label::new(egui::RichText::new(text).size(11.0)).sense(egui::Sense::drag()))
        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    drag_delta_from_response(ui, &response, speed)
}

/// Draw a fixed-width scrubbable label so inspector rows stay horizontally aligned.
pub fn aligned_scrubbable_label(
    ui: &mut Ui,
    text: &str,
    width: f32,
    value: &mut f32,
    speed: f32,
) -> bool {
    let mut changed = false;
    ui.allocate_ui_with_layout(
        egui::vec2(width, 18.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            changed = scrubbable_label(ui, text, value, speed);
        },
    );
    changed
}

/// Single-value version of [`drag_label`]: drag label directly changes `value`, returns whether changed.
pub fn scrubbable_label(ui: &mut Ui, text: &str, value: &mut f32, speed: f32) -> bool {
    let delta = drag_label(ui, text, speed);
    if delta == 0.0 {
        return false;
    }
    *value += delta;
    true
}

/// Whether text field should write back to document (#479: during editing only local buffer changed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextCommit {
    /// Continue editing, do not touch document.
    Keep,
    /// Enter (single line) or blur: write buffer back.
    Commit,
    /// Esc: discard buffer, return to document value.
    Cancel,
}

/// Pure functional part of input state, convenient for unit test (egui interaction cannot be simulated in test).
pub(crate) fn text_commit_decision(
    focused: bool,
    lost_focus: bool,
    enter: bool,
    escape: bool,
    multiline: bool,
) -> TextCommit {
    if focused && escape {
        return TextCommit::Cancel;
    }
    if (focused && enter && !multiline) || lost_focus {
        return TextCommit::Commit;
    }
    TextCommit::Keep
}

/// Delayed-submit text input: during editing only changes egui temp buffer, returns new value at submit.
///
/// Previously every keystroke called `set_field`: undo stack flooded, `dirty` kept flashing,
/// also triggered whole document sync reload (#435 / #473).
fn delayed_text_edit(
    ui: &mut Ui,
    id: egui::Id,
    value: &str,
    multiline: bool,
) -> (Response, String, Option<String>) {
    let mut buffer = ui
        .data_mut(|data| data.get_temp::<String>(id))
        .unwrap_or_else(|| value.to_string());
    let response = if multiline {
        ui.add(egui::TextEdit::multiline(&mut buffer))
    } else {
        ui.add(egui::TextEdit::singleline(&mut buffer))
    };
    let focused = response.has_focus();
    let (enter, escape) = ui.input(|input| {
        (
            input.key_pressed(egui::Key::Enter),
            input.key_pressed(egui::Key::Escape),
        )
    });
    let mut committed = None;
    match text_commit_decision(focused, response.lost_focus(), enter, escape, multiline) {
        TextCommit::Cancel => {
            buffer = value.to_string();
            response.surrender_focus();
        }
        TextCommit::Commit => {
            if buffer != value {
                committed = Some(buffer.clone());
            }
        }
        TextCommit::Keep => {
            if !focused {
                buffer = value.to_string();
            }
        }
    }
    ui.data_mut(|data| data.insert_temp(id, buffer.clone()));
    (response, buffer, committed)
}

fn property_label_width(ui: &Ui) -> f32 {
    (ui.available_width() * 0.36).clamp(72.0, 140.0)
}

fn property_label_slot(
    ui: &mut Ui,
    width: f32,
    add_contents: impl FnOnce(&mut Ui),
) -> egui::Response {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 18.0),
        egui::Layout::left_to_right(egui::Align::Center),
        add_contents,
    )
    .response
}

/// Property row for one schema field with two-column label/editor alignment.
fn field_row_at(
    ui: &mut Ui,
    registry: &Schema,
    field: &FieldSchema,
    value: &Value,
    prefix: &str,
    set: &mut dyn FnMut(&str, Value),
    assets: &[String],
    doc_dir: &str,
    i18n: &prism_editor_core::I18n,
) {
    let widget = field.widget();
    let label = field.label();
    let name = format!("{prefix}{}", field.name);
    let label_w = property_label_width(ui);

    match widget {
        FieldWidget::ReadOnly => {
            ui.horizontal(|ui| {
                let response = property_label_slot(ui, label_w, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(label).size(11.0))
                            .sense(egui::Sense::click()),
                    );
                });
                field_context_menu(&response, &name, value, set, i18n);
                ui.label(
                    egui::RichText::new(format_json(value))
                        .monospace()
                        .weak()
                        .size(11.0),
                );
            });
        }
        FieldWidget::Bool => {
            let mut checked = value.as_bool().unwrap_or(false);
            ui.horizontal(|ui| {
                let label_res = property_label_slot(ui, label_w, |ui| {
                    ui.label(egui::RichText::new(label).size(11.0));
                });
                let response = ui.checkbox(&mut checked, "");
                field_context_menu(&label_res, &name, value, set, i18n);
                field_context_menu(&response, &name, value, set, i18n);
                if response.changed() {
                    set(&name, Value::Bool(checked));
                }
            });
        }
        FieldWidget::Integer => {
            let mut current = value.as_i64().unwrap_or(0);
            ui.horizontal(|ui| {
                let mut scrubbed = current as f32;
                let dragged = aligned_scrubbable_label(ui, label, label_w, &mut scrubbed, 1.0);
                if dragged {
                    current = scrubbed.round() as i64;
                }
                let drag = ui
                    .add_sized(
                        [ui.available_width().max(52.0), 18.0],
                        egui::DragValue::new(&mut current)
                            .speed(1.0)
                            .custom_parser(number_parser),
                    )
                    .on_hover_text(i18n.t1("inspector.expressionHint"));
                field_context_menu(&drag, &name, value, set, i18n);
                if dragged || drag.changed() {
                    set(&name, Value::from(current));
                }
            });
        }
        FieldWidget::Float => {
            let mut current = value.as_f64().unwrap_or(0.0) as f32;
            ui.horizontal(|ui| {
                let dragged = aligned_scrubbable_label(ui, label, label_w, &mut current, 0.01);
                let drawer = field.drawer();
                let mut drag = egui::DragValue::new(&mut current).speed(0.01);
                if let Some((min, max)) = drawer.and_then(|drawer| drawer.range()) {
                    drag = drag.range(min..=max);
                } else {
                    if let Some(min) = drawer.and_then(|drawer| drawer.min()) {
                        drag = drag.range(min..=f32::MAX);
                    }
                    if let Some(max) = drawer.and_then(|drawer| drawer.max()) {
                        drag = drag.range(f32::MIN..=max);
                    }
                }
                let response = ui
                    .add_sized(
                        [ui.available_width().max(52.0), 18.0],
                        drag.custom_parser(number_parser),
                    )
                    .on_hover_text(i18n.t1("inspector.expressionHintTrig"));
                field_context_menu(&response, &name, value, set, i18n);
                if dragged || response.changed() {
                    set(&name, Value::from(current as f64));
                }
            });
        }
        FieldWidget::Text => {
            let mut current = value.as_str().unwrap_or_default().to_string();
            let edit_id = ui.make_persistent_id(("field-text", name.as_str()));
            ui.horizontal(|ui| {
                property_label_slot(ui, label_w, |ui| {
                    ui.label(egui::RichText::new(label).size(11.0));
                });
                let (response, buffer, committed) = delayed_text_edit(ui, edit_id, &current, false);
                field_context_menu(&response, &name, value, set, i18n);
                current = buffer;
                if let Some(text) = committed {
                    set(&name, Value::String(text));
                }
                let looks_like_asset = name.ends_with("_path")
                    || matches!(name.as_str(), "prefab" | "env_map" | "volume" | "mesh");
                if looks_like_asset
                    && !current.is_empty()
                    && prism_editor_core::assets::resolve_reference(assets, doc_dir, &current)
                        .is_none()
                {
                    ui.label(
                        egui::RichText::new("⚠")
                            .size(10.5)
                            .color(egui::Color32::from_rgb(232, 96, 96)),
                    );
                }
                if looks_like_asset {
                    if let Some(payload) = response.dnd_release_payload::<AssetDrag>() {
                        set(&name, Value::String(payload.path.clone()));
                    }
                    let mut picked: Option<String> = None;
                    ui.menu_button("…", |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt((name.clone(), "asset-picker"))
                            .max_height(260.0)
                            .show(ui, |ui| {
                                if assets.is_empty() {
                                    ui.label(
                                        egui::RichText::new(i18n.t1("inspector.noAssets"))
                                            .weak()
                                            .size(10.0),
                                    );
                                }
                                for path in assets {
                                    if !asset_matches(&name, path) {
                                        continue;
                                    }
                                    if ui.selectable_label(current == *path, path).clicked() {
                                        picked = Some(path.clone());
                                        ui.close();
                                    }
                                }
                            });
                    });
                    if let Some(path) = picked {
                        set(&name, Value::String(path));
                    }
                }
            });
        }
        FieldWidget::Multiline => {
            let current = value.as_str().unwrap_or_default().to_string();
            let edit_id = ui.make_persistent_id(("field-multiline", name.as_str()));
            ui.label(egui::RichText::new(label).size(11.0));
            let (response, _buffer, committed) = delayed_text_edit(ui, edit_id, &current, true);
            field_context_menu(&response, &name, value, set, i18n);
            if let Some(text) = committed {
                set(&name, Value::String(text));
            }
        }
        FieldWidget::Vector { len } => {
            let mut values: Vec<f32> = (0..len)
                .map(|axis| {
                    value
                        .get(axis)
                        .and_then(|item| item.as_f64())
                        .unwrap_or(0.0) as f32
                })
                .collect();
            ui.horizontal(|ui| {
                let mut row_delta = 0.0f32;
                if aligned_scrubbable_label(ui, label, label_w, &mut row_delta, 0.01) {
                    for item in values.iter_mut() {
                        *item += row_delta;
                    }
                    let mut next = value.clone();
                    if let Some(array) = next.as_array_mut() {
                        for (axis, item) in values.iter().enumerate() {
                            if let Some(slot) = array.get_mut(axis) {
                                *slot = Value::from(*item as f64);
                            }
                        }
                        set(&name, next);
                    }
                }
                for (axis, item) in values.iter_mut().enumerate() {
                    let axis_name = ["X", "Y", "Z", "W"].get(axis).copied().unwrap_or("");
                    let badge_changed = if !axis_name.is_empty() && len >= 2 && len <= 4 {
                        scrubbable_axis_badge(ui, axis_name, axis, item, 0.01)
                    } else {
                        false
                    };
                    let response = ui.add(
                        egui::DragValue::new(item)
                            .speed(0.01)
                            .custom_parser(number_parser),
                    );
                    field_context_menu(&response, &name, value, set, i18n);
                    if badge_changed || response.changed() {
                        let mut next = value.clone();
                        if let Some(array) = next.as_array_mut() {
                            if let Some(slot) = array.get_mut(axis) {
                                *slot = Value::from(*item as f64);
                            }
                            set(&name, next);
                        }
                    }
                }
            });
        }
        FieldWidget::Quat => {
            let current = [0usize, 1, 2, 3].map(|axis| {
                value
                    .get(axis)
                    .and_then(|item| item.as_f64())
                    .unwrap_or(if axis == 3 { 1.0 } else { 0.0 }) as f32
            });
            let mut degrees = vm::quat_to_euler_degrees(current);
            ui.horizontal(|ui| {
                property_label_slot(ui, label_w, |ui| {
                    ui.label(egui::RichText::new(label).size(11.0));
                });
                for axis in 0..3 {
                    let badge_changed = scrubbable_axis_badge(
                        ui,
                        ["X", "Y", "Z"][axis],
                        axis,
                        &mut degrees[axis],
                        0.5,
                    );
                    let response = ui
                        .add(
                            egui::DragValue::new(&mut degrees[axis])
                                .speed(0.5)
                                .max_decimals(2)
                                .suffix("°")
                                .custom_parser(number_parser),
                        )
                        .on_hover_text(i18n.t1("inspector.expressionHintAngle"));
                    field_context_menu(&response, &name, value, set, i18n);
                    if badge_changed || response.changed() {
                        let quat = vm::euler_degrees_to_quat(degrees);
                        set(
                            &name,
                            Value::Array(
                                quat.iter()
                                    .map(|component| Value::from(*component as f64))
                                    .collect(),
                            ),
                        );
                        break;
                    }
                }
            });
        }
        FieldWidget::Color { alpha } => {
            let mut rgba = read_color(value, alpha);
            ui.horizontal(|ui| {
                property_label_slot(ui, label_w, |ui| {
                    ui.label(egui::RichText::new(label).size(11.0));
                });
                let response = ui.color_edit_button_rgba_unmultiplied(&mut rgba);
                field_context_menu(&response, &name, value, set, i18n);
                if response.changed() {
                    set(&name, color_value(rgba, alpha));
                }
            });
        }
        FieldWidget::Enum { variants } => {
            let current = value.as_str().unwrap_or_default().to_string();
            ui.horizontal(|ui| {
                property_label_slot(ui, label_w, |ui| {
                    ui.label(egui::RichText::new(label).size(11.0));
                });
                let combo = egui::ComboBox::from_id_salt((name.clone(), field.name.clone()))
                    .selected_text(current.clone())
                    .show_ui(ui, |ui| {
                        for variant in &variants {
                            if ui.selectable_label(*variant == current, variant).clicked() {
                                set(&name, Value::String(variant.clone()));
                            }
                        }
                    });
                field_context_menu(&combo.response, &name, value, set, i18n);
            });
        }
        FieldWidget::List => {
            list_row(
                ui, registry, field, value, prefix, set, assets, doc_dir, i18n,
            );
        }
        FieldWidget::Option => {
            let inner = field.inner.as_deref();
            let mut enabled = !value.is_null();
            let response = ui.checkbox(&mut enabled, label);
            field_context_menu(&response, &name, value, set, i18n);
            if response.changed() {
                set(
                    &name,
                    if enabled {
                        inner
                            .map(|inner| default_value_for(registry, inner, 0))
                            .unwrap_or(Value::Null)
                    } else {
                        Value::Null
                    },
                );
            }
            if enabled {
                if let Some(inner) = inner {
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        optional_scalar(ui, &name, inner, value, set);
                    });
                }
            }
        }
        FieldWidget::Struct => {
            if field.struct_type.as_deref() == Some("prism_engine::ui::Anchors") {
                anchor_preset_grid(ui, &name, value, set);
            }
            let fields = field
                .struct_type
                .as_deref()
                .and_then(|name| registry.get(name))
                .map(|schema| schema.fields.clone())
                .unwrap_or_default();
            if fields.is_empty() {
                json_row(ui, label, value);
            } else {
                let section = egui::CollapsingHeader::new(label)
                    .id_salt(("struct", name.clone()))
                    .default_open(true)
                    .show(ui, |ui| {
                        let child_prefix = format!("{name}.");
                        for child in &fields {
                            let child_value =
                                value.get(&child.name).cloned().unwrap_or(Value::Null);
                            field_row_at(
                                ui,
                                registry,
                                child,
                                &child_value,
                                &child_prefix,
                                set,
                                assets,
                                doc_dir,
                                i18n,
                            );
                        }
                    });
                field_context_menu(&section.header_response, &name, value, set, i18n);
            }
        }
    }
}

/// Anchor presets (Unity `LayoutDropdownWindow` 9 points + 3 stretch types).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AnchorPreset {
    Point { x: u8, y: u8 },
    StretchHorizontal,
    StretchVertical,
    Stretch,
}

impl AnchorPreset {
    /// 3×3 point anchor grid, arranged "top -> bottom, left -> right".
    const GRID: [[Self; 3]; 3] = [
        [
            Self::Point { x: 0, y: 0 },
            Self::Point { x: 1, y: 0 },
            Self::Point { x: 2, y: 0 },
        ],
        [
            Self::Point { x: 0, y: 1 },
            Self::Point { x: 1, y: 1 },
            Self::Point { x: 2, y: 1 },
        ],
        [
            Self::Point { x: 0, y: 2 },
            Self::Point { x: 1, y: 2 },
            Self::Point { x: 2, y: 2 },
        ],
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Point { x: 0, y: 0 } => "TL",
            Self::Point { x: 1, y: 0 } => "T",
            Self::Point { x: 2, y: 0 } => "TR",
            Self::Point { x: 0, y: 1 } => "L",
            Self::Point { x: 1, y: 1 } => "C",
            Self::Point { x: 2, y: 1 } => "R",
            Self::Point { x: 0, y: 2 } => "BL",
            Self::Point { x: 1, y: 2 } => "B",
            Self::Point { x: 2, y: 2 } => "BR",
            Self::Point { .. } => "?",
            Self::StretchHorizontal => "H",
            Self::StretchVertical => "V",
            Self::Stretch => "Full",
        }
    }

    fn tooltip(self) -> &'static str {
        match self {
            Self::Point { x: 0, y: 0 } => "Anchor: top left",
            Self::Point { x: 1, y: 0 } => "Anchor: top center",
            Self::Point { x: 2, y: 0 } => "Anchor: top right",
            Self::Point { x: 0, y: 1 } => "Anchor: middle left",
            Self::Point { x: 1, y: 1 } => "Anchor: center",
            Self::Point { x: 2, y: 1 } => "Anchor: middle right",
            Self::Point { x: 0, y: 2 } => "Anchor: bottom left",
            Self::Point { x: 1, y: 2 } => "Anchor: bottom center",
            Self::Point { x: 2, y: 2 } => "Anchor: bottom right",
            Self::Point { .. } => "Anchor",
            Self::StretchHorizontal => "Stretch horizontally (keep vertical anchor)",
            Self::StretchVertical => "Stretch vertically (keep horizontal anchor)",
            Self::Stretch => "Stretch to fill the parent",
        }
    }

    /// Apply preset to current anchor. Stretch preset keeps other axis center, consistent with Unity's
    /// `anchorMin/anchorMax` panel.
    fn anchors(self, current: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
        let (min_x, min_y, max_x, max_y) = current;
        let center_x = (min_x + max_x) * 0.5;
        let center_y = (min_y + max_y) * 0.5;
        match self {
            Self::Point { x, y } => {
                let point_x = f32::from(x) * 0.5;
                let point_y = f32::from(y) * 0.5;
                (point_x, point_y, point_x, point_y)
            }
            Self::StretchHorizontal => (0.0, center_y, 1.0, center_y),
            Self::StretchVertical => (center_x, 0.0, center_x, 1.0),
            Self::Stretch => (0.0, 0.0, 1.0, 1.0),
        }
    }

    fn is_active(self, current: (f32, f32, f32, f32)) -> bool {
        let (min_x, min_y, max_x, max_y) = self.anchors(current);
        let close = |a: f32, b: f32| (a - b).abs() <= 1e-4;
        match self {
            Self::Point { .. } | Self::Stretch => {
                close(current.0, min_x)
                    && close(current.1, min_y)
                    && close(current.2, max_x)
                    && close(current.3, max_y)
            }
            Self::StretchHorizontal => {
                close(current.0, 0.0) && close(current.2, 1.0) && close(current.1, current.3)
            }
            Self::StretchVertical => {
                close(current.1, 0.0) && close(current.3, 1.0) && close(current.0, current.2)
            }
        }
    }
}

fn anchors_tuple(value: &Value) -> (f32, f32, f32, f32) {
    let read = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_f64)
            .map(|value| value as f32)
            .unwrap_or(0.0)
    };
    (read("min_x"), read("min_y"), read("max_x"), read("max_y"))
}

fn anchors_value(anchors: (f32, f32, f32, f32)) -> Value {
    let (min_x, min_y, max_x, max_y) = anchors;
    serde_json::json!({
        "min_x": min_x,
        "min_y": min_y,
        "max_x": max_x,
        "max_y": max_y,
    })
}

/// Draw preset buttons above `Anchors` struct; click writes back whole anchors (one undo step).
fn anchor_preset_grid(ui: &mut Ui, name: &str, value: &Value, set: &mut dyn FnMut(&str, Value)) {
    let current = anchors_tuple(value);
    egui::Grid::new(("anchor-presets", name))
        .spacing([2.0, 2.0])
        .show(ui, |ui| {
            for row in AnchorPreset::GRID {
                for preset in row {
                    let response = ui.add_sized(
                        [26.0, 18.0],
                        egui::Button::new(preset.label()).selected(preset.is_active(current)),
                    );
                    if response.clicked() {
                        set(name, anchors_value(preset.anchors(current)));
                    }
                    response.on_hover_text(preset.tooltip());
                }
                ui.end_row();
            }
        });
    ui.horizontal(|ui| {
        for preset in [
            AnchorPreset::StretchHorizontal,
            AnchorPreset::StretchVertical,
            AnchorPreset::Stretch,
        ] {
            let response = ui.add_sized(
                [40.0, 18.0],
                egui::Button::new(preset.label()).selected(preset.is_active(current)),
            );
            if response.clicked() {
                set(name, anchors_value(preset.anchors(current)));
            }
            response.on_hover_text(preset.tooltip());
        }
    });
    ui.add_space(2.0);
}

/// Field right-click menu (#418): copy / paste value, copy property path.
///
/// Clipboard in `egui` temp data (`serde_json::Value` storable), so
/// [`field_row_at`] signature no longer needs an extra `UiState` param, component panel can reuse.
/// Validate shape with [`values_compatible`] before paste, avoid writing string into `Vec3`.
fn field_context_menu(
    response: &Response,
    name: &str,
    value: &Value,
    set: &mut dyn FnMut(&str, Value),
    i18n: &prism_editor_core::I18n,
) {
    const CLIPBOARD: &str = "prism-inspector-field-clipboard";
    let id = egui::Id::new(CLIPBOARD);
    response.context_menu(|ui| {
        if ui.button(i18n.t1("inspector.copyValue")).clicked() {
            ui.ctx().copy_text(format_json(value));
            ui.data_mut(|data| data.insert_temp(id, value.clone()));
            ui.close();
        }
        let clipboard = ui.data(|data| data.get_temp::<Value>(id));
        let compatible = clipboard
            .as_ref()
            .map(|clipboard| values_compatible(value, clipboard))
            .unwrap_or(false);
        if ui
            .add_enabled(
                compatible,
                egui::Button::new(i18n.t1("inspector.pasteValue")),
            )
            .on_hover_text(i18n.t1("inspector.pasteValueHint"))
            .clicked()
        {
            if let Some(clipboard) = clipboard {
                set(name, clipboard);
            }
            ui.close();
        }
        ui.separator();
        if ui.button(i18n.t1("inspector.copyPath")).clicked() {
            ui.ctx().copy_text(name.to_string());
            ui.close();
        }
    });
}

/// Whether two JSON values have same shape (validation before paste; no type cast).
fn values_compatible(current: &Value, candidate: &Value) -> bool {
    matches!(
        (current, candidate),
        (Value::Array(_), Value::Array(_))
            | (Value::Object(_), Value::Object(_))
            | (Value::String(_), Value::String(_))
            | (Value::Bool(_), Value::Bool(_))
            | (Value::Number(_), Value::Number(_))
            | (Value::Null, Value::Null)
    )
}

/// Gray JSON fallback for nested fields (lists, unparseable structs).
fn json_row(ui: &mut Ui, label: &str, value: &Value) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format_json(value))
                    .monospace()
                    .weak()
                    .size(10.0),
            );
        });
    });
}

/// Default value for field "add element"; decided entirely by schema, does not guess JSON type.
///
/// `Option<T>` has value / list append element all go through it: `Struct` recursively asks registry,
/// scalar uses neutral value, `Vec3` / `Quat` uses unit value, `Enum` takes first variant.
fn default_value_for(registry: &Schema, field: &FieldSchema, depth: usize) -> Value {
    if depth > 8 {
        return Value::Null;
    }
    match field.widget() {
        FieldWidget::Struct => {
            let Some(schema) = field
                .struct_type
                .as_deref()
                .and_then(|name| registry.get(name))
            else {
                return Value::Null;
            };
            let mut map = serde_json::Map::new();
            for child in &schema.fields {
                map.insert(
                    child.name.clone(),
                    default_value_for(registry, child, depth + 1),
                );
            }
            Value::Object(map)
        }
        FieldWidget::List => Value::Array(Vec::new()),
        FieldWidget::Option => Value::Null,
        FieldWidget::Enum { variants } => variants
            .first()
            .cloned()
            .map(Value::String)
            .unwrap_or(Value::Null),
        FieldWidget::Bool => Value::Bool(false),
        FieldWidget::Integer => Value::from(0),
        FieldWidget::Float => Value::from(0.0),
        FieldWidget::Text | FieldWidget::Multiline => Value::String(String::new()),
        FieldWidget::Vector { len } => Value::Array(vec![Value::from(0.0); len]),
        FieldWidget::Color { alpha } => {
            let mut channels = vec![Value::from(1.0); 3];
            if alpha {
                channels.push(Value::from(1.0));
            }
            Value::Array(channels)
        }
        FieldWidget::Quat => Value::Array(vec![
            Value::from(0.0),
            Value::from(0.0),
            Value::from(0.0),
            Value::from(1.0),
        ]),
        FieldWidget::ReadOnly => Value::Null,
    }
}

/// Edit control for `Vec<T>` / fixed array (#418).
///
/// - Dynamic list (`kind = "list"`): one row per element, with `▲` / `▼` / `✕`; bottom `+ Add`
///   generates default value per element schema (`Vec3` -> `[0,0,0]`, `Struct` -> each subfield default);
/// - Fixed array (`array_len = Some(n)`): no add/remove, edit per item only;
/// - Element itself reuses [`field_row_at`], so fields inside `Vec<Vec3>` / `Vec<Struct>` still use
///   real controls, path is `field[i]` / `field[i].child` (`prism_editor_core::schema`
///   `parse_path` supports indexing).
///
/// Add/remove / reorder all write back **whole array**: `set_field` one undo step, consistent with Unity array editing.
#[allow(clippy::too_many_arguments)]
fn list_row(
    ui: &mut Ui,
    registry: &Schema,
    field: &FieldSchema,
    value: &Value,
    prefix: &str,
    set: &mut dyn FnMut(&str, Value),
    assets: &[String],
    doc_dir: &str,
    i18n: &prism_editor_core::I18n,
) {
    let name = format!("{prefix}{}", field.name);
    let items = value.as_array().cloned().unwrap_or_default();
    let fixed = field.array_len.is_some();
    let inner = field.inner.clone().map(|inner| *inner);
    let mut pending: Option<Value> = None;

    let section = egui::CollapsingHeader::new(format!("{}  ({})", field.label(), items.len()))
        .id_salt(("list", name.as_str()))
        .default_open(true)
        .show(ui, |ui| {
            let mut working = items.clone();
            for (index, item) in items.iter().enumerate() {
                let mut element = inner.clone();
                if let Some(element) = element.as_mut() {
                    element.name = format!("[{index}]");
                }
                ui.horizontal(|ui| {
                    if !fixed {
                        if ui
                            .small_button(icons::CARET_UP)
                            .on_hover_text(i18n.t1("inspector.moveUp"))
                            .clicked()
                            && list_move(&mut working, index, index.saturating_sub(1))
                        {
                            pending = Some(Value::Array(working.clone()));
                        }
                        if ui
                            .small_button(icons::CARET_DOWN)
                            .on_hover_text(i18n.t1("inspector.moveDown"))
                            .clicked()
                            && list_move(&mut working, index, index + 1)
                        {
                            pending = Some(Value::Array(working.clone()));
                        }
                        if ui
                            .small_button(icons::X)
                            .on_hover_text(i18n.t1("inspector.removeElement"))
                            .clicked()
                            && list_remove(&mut working, index)
                        {
                            pending = Some(Value::Array(working.clone()));
                        }
                    }
                    match &element {
                        Some(element) => field_row_at(
                            ui, registry, element, item, &name, set, assets, doc_dir, i18n,
                        ),
                        None => json_row(ui, &format!("[{index}]"), item),
                    }
                });
            }
            if !fixed
                && ui
                    .small_button(format!("{} Add", icons::PLUS))
                    .on_hover_text(i18n.t1("inspector.appendElement"))
                    .clicked()
            {
                let element = inner
                    .as_ref()
                    .map(|inner| default_value_for(registry, inner, 0))
                    .unwrap_or(Value::Null);
                list_add(&mut working, element);
                pending = Some(Value::Array(working.clone()));
            }
            if items.is_empty() {
                ui.label(
                    egui::RichText::new(i18n.t1("inspector.emptyList"))
                        .weak()
                        .size(10.0),
                );
            }
        });
    field_context_menu(&section.header_response, &name, value, set, i18n);

    if let Some(value) = pending {
        set(&name, value);
    }
}

/// Append an element (only write entry for list editing, convenient for unit test).
fn list_add(items: &mut Vec<Value>, element: Value) {
    items.push(element);
}

/// Delete index `index`; returns `False` out-of-bounds (does not write back document).
fn list_remove(items: &mut Vec<Value>, index: usize) -> bool {
    if index < items.len() {
        items.remove(index);
        true
    } else {
        false
    }
}

/// Swap two elements; returns `False` if either end out-of-bounds.
fn list_move(items: &mut [Value], from: usize, to: usize) -> bool {
    if from < items.len() && to < items.len() && from != to {
        items.swap(from, to);
        true
    } else {
        false
    }
}

fn optional_scalar(
    ui: &mut Ui,
    path: &str,
    inner: &FieldSchema,
    value: &Value,
    set: &mut dyn FnMut(&str, Value),
) {
    match inner.widget() {
        FieldWidget::Bool => {
            let mut checked = value.as_bool().unwrap_or(false);
            if ui.checkbox(&mut checked, "").changed() {
                set(path, Value::Bool(checked));
            }
        }
        FieldWidget::Integer => {
            let mut current = value.as_i64().unwrap_or(0);
            if ui
                .add(egui::DragValue::new(&mut current).speed(1.0))
                .changed()
            {
                set(path, Value::from(current));
            }
        }
        FieldWidget::Float => {
            let mut current = value.as_f64().unwrap_or(0.0) as f32;
            if ui
                .add(egui::DragValue::new(&mut current).speed(0.01))
                .changed()
            {
                set(path, Value::from(current as f64));
            }
        }
        FieldWidget::Text => {
            let mut current = value.as_str().unwrap_or_default().to_string();
            if ui.text_edit_singleline(&mut current).changed() {
                set(path, Value::String(current));
            }
        }
        _ => {
            ui.label(
                egui::RichText::new(format_json(value))
                    .monospace()
                    .weak()
                    .size(10.0),
            );
        }
    }
}

fn format_json(value: &Value) -> String {
    let text = value.to_string();
    if text.len() > 80 {
        format!("{}…", &text[..80])
    } else {
        text
    }
}

fn read_color(value: &Value, alpha: bool) -> [f32; 4] {
    let channel = |index: usize, fallback: f32| {
        value
            .get(index)
            .and_then(|item| item.as_f64())
            .map(|item| item as f32)
            .unwrap_or(fallback)
    };
    [
        channel(0, 1.0),
        channel(1, 1.0),
        channel(2, 1.0),
        if alpha { channel(3, 1.0) } else { 1.0 },
    ]
}

fn color_value(color: [f32; 4], alpha: bool) -> Value {
    let mut array = vec![
        Value::from(color[0] as f64),
        Value::from(color[1] as f64),
        Value::from(color[2] as f64),
    ];
    if alpha {
        array.push(Value::from(color[3] as f64));
    }
    Value::Array(array)
}
