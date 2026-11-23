use crate::commands::DocCommand;
use crate::inspector::field_node_key;
use crate::*;

#[test]
fn schema_inspector_edit_and_undo_redo_roundtrip() {
    let mut editor = EditorUiState::default();
    editor.build_and_step(Vec2::new(1280.0, 720.0), &UiInput::default(), 0.016);

    let key = field_node_key(
        2,
        "prism_engine::scene::LocalTransform",
        "position",
        Some(0),
    );
    let rect = editor
        .tree
        .node(editor.tree.node_by_key(key).expect("field node exists"))
        .unwrap()
        .computed_rect;
    let center = Vec2::new(rect.x + rect.w * 0.5, rect.y + rect.h * 0.5);

    editor.build_and_step(
        Vec2::new(1280.0, 720.0),
        &UiInput {
            pointer_pos: center,
            primary_down: true,
            primary_just_pressed: true,
            ..Default::default()
        },
        0.016,
    );
    editor.build_and_step(
        Vec2::new(1280.0, 720.0),
        &UiInput {
            pointer_pos: center + Vec2::new(40.0, 0.0),
            primary_down: true,
            ..Default::default()
        },
        0.016,
    );
    editor.build_and_step(
        Vec2::new(1280.0, 720.0),
        &UiInput {
            pointer_pos: center + Vec2::new(40.0, 0.0),
            primary_just_released: true,
            ..Default::default()
        },
        0.016,
    );

    let edited_x = editor.doc.entities[1].components["prism_engine::scene::LocalTransform"]
        ["position"][0]
        .as_f64()
        .unwrap();
    assert!((edited_x - 1.0).abs() < 1e-4);
    assert!(editor.undo.can_undo());

    let undo_rect = editor
        .tree
        .node(
            editor
                .tree
                .node_by_key("toolbar:undo".into_node_key())
                .unwrap(),
        )
        .unwrap()
        .computed_rect;
    let u_center = Vec2::new(
        undo_rect.x + undo_rect.w * 0.5,
        undo_rect.y + undo_rect.h * 0.5,
    );
    editor.build_and_step(
        Vec2::new(1280.0, 720.0),
        &UiInput {
            pointer_pos: u_center,
            primary_down: true,
            primary_just_pressed: true,
            ..Default::default()
        },
        0.016,
    );
    editor.build_and_step(
        Vec2::new(1280.0, 720.0),
        &UiInput {
            pointer_pos: u_center,
            primary_just_released: true,
            ..Default::default()
        },
        0.016,
    );

    let undone_x = editor.doc.entities[1].components["prism_engine::scene::LocalTransform"]
        ["position"][0]
        .as_f64()
        .unwrap();
    assert!((undone_x - 0.0).abs() < 1e-4);
}

#[test]
fn doc_command_create_and_rename_undo_redo_roundtrip() {
    let mut editor = EditorUiState::default();
    let new_id = editor.doc.next_entity_id();
    editor.apply_commands(vec![
        EditorCommand::Doc(DocCommand::CreateEntity {
            id: new_id,
            name: "Player".to_string(),
            parent: Some(1),
        }),
        EditorCommand::Doc(DocCommand::RenameEntity {
            id: new_id,
            old_name: "Player".to_string(),
            new_name: "Hero".to_string(),
        }),
    ]);
    assert_eq!(editor.doc.entities.len(), 4);
    assert_eq!(editor.doc.entities[3].name, "Hero");

    editor.apply_commands(vec![EditorCommand::Undo]);
    assert_eq!(editor.doc.entities[3].name, "Player");

    editor.apply_commands(vec![EditorCommand::Undo]);
    assert_eq!(editor.doc.entities.len(), 3);

    editor.apply_commands(vec![EditorCommand::Redo, EditorCommand::Redo]);
    assert_eq!(editor.doc.entities.len(), 4);
    assert_eq!(editor.doc.entities[3].name, "Hero");
}

#[test]
fn dynamic_panel_registration_updates_menus_and_toggle_command() {
    struct ConsolePanel;
    impl EditorPanel for ConsolePanel {
        fn id(&self) -> &'static str {
            "console"
        }
        fn title_key(&self) -> &'static str {
            "Console"
        }
        fn view(&self, _ctx: &PanelViewCtx, ui: &mut UiContext) {
            ui.mount(label("Console"));
        }
    }

    let mut editor = EditorUiState::default();
    editor.register_panel(Box::new(ConsolePanel));
    let menus = editor.menus();
    let window_menu = menus.iter().find(|m| m.id == "window").unwrap();
    assert!(window_menu
        .entries
        .iter()
        .any(|e| matches!(e, MenuEntry::Checkbox { command, .. } if command == "window.console")));

    editor.apply_menu_actions(vec![MenuAction::Command("window.console".to_string())]);
    assert!(editor.window_layout.is_window_open(&"console".into()));
}

#[test]
fn engine_draw_list_conversion_preserves_viewport_and_text() {
    let mut editor = EditorUiState::default();
    editor.build_and_step(Vec2::new(1280.0, 720.0), &UiInput::default(), 0.016);
    let packet = bridge::to_ui_overlay(&editor.draw_list);
    assert!(packet.quads.iter().any(|q| q.texture == Some(0)));
    assert!(packet.texts.iter().any(|t| t.content == "Prisma Editor"));
}

#[test]
fn submitting_the_editor_draw_list_does_not_erase_engine_ui() {
    use prism_engine::ui::{UiDrawList as EngineUiDrawList, UiQuad};

    let mut world = prism_engine::ecs::World::new();
    let mut engine_list = EngineUiDrawList::default();
    engine_list.push_quad(UiQuad {
        rect: [0.0, 0.0, 10.0, 10.0],
        color: [1.0, 0.0, 0.0, 1.0],
        ..Default::default()
    });
    world.insert_resource(engine_list);

    let mut editor = EditorUiState::default();
    editor.build_and_step(Vec2::new(1280.0, 720.0), &UiInput::default(), 0.016);
    bridge::submit_draw_list(&mut world, &editor.draw_list);

    let list = world
        .get_resource::<EngineUiDrawList>()
        .expect("draw list resource exists");
    assert!(list.quads().count() > 1);
    assert!(list.texts().any(|t| t.content == "Prisma Editor"));
    assert!(list.quads().any(|q| q.color == [1.0, 0.0, 0.0, 1.0]));
}

#[test]
fn editor_pointer_input_comes_from_the_engine_ui_state() {
    let mut world = prism_engine::ecs::World::new();
    let empty = bridge::read_engine_input(&world, 0.0);
    assert_eq!(empty.pointer_pos, Vec2::ZERO);
    assert!(!empty.primary_down);

    world.insert_resource(prism_engine::ui::UiInputState {
        cursor_pos: [321.0, 123.0],
        left_clicked: true,
        left_held: true,
        scroll_delta: [0.0, 2.0],
        shift_held: true,
        pressed_keys: vec![prism_engine::input::KeyCode::Escape],
        ..Default::default()
    });
    let input = bridge::read_engine_input(&world, 0.5);
    assert_eq!(input.pointer_pos, Vec2::new(321.0, 123.0));
    assert!(input.primary_down);
    assert!(input.primary_just_pressed);
    assert!(!input.primary_just_released);
    assert_eq!(input.scroll_delta, Vec2::new(0.0, 2.0));
    assert!(input.shift_held);
    assert!(!input.ctrl_held);
    assert!(input.escape_pressed);
    assert_eq!(input.time_seconds, 0.5);
}

#[test]
fn engine_pointer_state_reaches_the_ui_tree() {
    let mut editor = EditorUiState::default();
    editor.build_and_step(Vec2::new(1280.0, 720.0), &UiInput::default(), 0.016);
    let rect = editor
        .tree
        .node(
            editor
                .tree
                .node_by_key("toolbar:play".into_node_key())
                .expect("play button exists"),
        )
        .unwrap()
        .computed_rect;
    let center = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];

    let mut world = prism_engine::ecs::World::new();
    world.insert_resource(prism_engine::ui::UiInputState {
        cursor_pos: center,
        left_clicked: true,
        left_held: true,
        ..Default::default()
    });
    let press = bridge::read_engine_input(&world, 0.0);
    editor.build_and_step(Vec2::new(1280.0, 720.0), &press, 0.016);

    world.insert_resource(prism_engine::ui::UiInputState {
        cursor_pos: center,
        left_released: true,
        ..Default::default()
    });
    let release = bridge::read_engine_input(&world, 0.1);
    let actions = editor.build_and_step(Vec2::new(1280.0, 720.0), &release, 0.016);

    assert!(actions
        .iter()
        .any(|a| matches!(a, UiAction::Toggled { .. } | UiAction::Clicked(_))));
    assert!(editor.session.playing);
}

#[test]
fn menu_actions_switch_theme_language_and_open_tool_windows() {
    let mut editor = EditorUiState::default();
    assert_eq!(editor.theme.preset(), ThemePreset::Dark);

    editor.apply_menu_actions(vec![
        MenuAction::Command("view.theme.light".into()),
        MenuAction::Command("view.lang.zh-CN".into()),
        MenuAction::Command("view.grid".into()),
        MenuAction::Command("window.console".into()),
        MenuAction::Command("window.profiler".into()),
    ]);

    assert_eq!(editor.theme.preset(), ThemePreset::Light);
    assert_eq!(editor.i18n.locale, Locale::ZhCn);
    assert!(editor.session.show_grid);
    assert!(editor.window_layout.is_window_open(&"console".into()));
    assert!(editor.window_layout.is_window_open(&"profiler".into()));

    editor.build_and_step(Vec2::new(1280.0, 720.0), &UiInput::default(), 0.016);
    assert!(!editor.draw_list.commands.is_empty());

    editor.apply_menu_actions(vec![MenuAction::Command("file.exit".into())]);
    assert!(editor.exit_requested);
}
