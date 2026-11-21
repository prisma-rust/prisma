//! Composable, retained-mode UI kernel and modern declarative API for Prisma.

pub mod behavior;
pub mod draw;
pub mod elements;
mod layout;
pub mod style;
pub mod tree;
#[cfg(feature = "ui-tween")]
pub mod tween;

pub mod prelude {
    pub use crate::behavior::{DragPayload, DraggableValue, UiAction};
    pub use crate::draw::{TextAlign, UiDrawCommand, UiDrawList, UiTextureId};
    pub use crate::elements::{
        button, canvas, col, divider, label, muted_label, numeric_field, row, spacer, text_field,
        toggle_button, tree_view, vec3_field, viewport, virtual_list, ElementBox, UiContext,
        UiElement, UiPalette,
    };
    pub use crate::style::{
        Align, Color, Edges, FlexDirection, InteractionState, Justify, LayoutStyle, Length,
        PositionType, Rect, VisualBox, VisualStyle,
    };
    pub use crate::tree::{IntoNodeKey, NodeId, NodeKey, UiInput, UiTree};
    #[cfg(feature = "ui-tween")]
    pub use crate::tween::{Ease, TweenProperty, TweenStore};
}

#[cfg(test)]
mod tests {
    use glam::Vec2;

    use crate::prelude::*;

    #[test]
    fn button_click_and_toggle_emit_actions() {
        let mut tree = UiTree::new();
        let build = |t: &mut UiTree, active: bool| {
            t.begin_frame();
            let mut ctx = UiContext::new(t);
            ctx.mount(
                row()
                    .size(Length::Px(400.0), Length::Px(100.0))
                    .pad(8.0)
                    .gap(8.0)
                    .child(button("save_btn", "Save"))
                    .child(toggle_button("play_btn", "Play", active)),
            );
            t.compute_layout(Vec2::new(400.0, 100.0));
        };

        build(&mut tree, false);
        let save_rect = tree
            .node(tree.node_by_key("save_btn".into_node_key()).unwrap())
            .unwrap()
            .computed_rect;
        let center = Vec2::new(
            save_rect.x + save_rect.w * 0.5,
            save_rect.y + save_rect.h * 0.5,
        );

        let press = UiInput {
            pointer_pos: center,
            primary_down: true,
            primary_just_pressed: true,
            ..Default::default()
        };
        let press_actions = tree.process_input(&press);
        assert_eq!(
            press_actions,
            vec![UiAction::FocusChanged(Some("save_btn".into_node_key()))]
        );

        let release = UiInput {
            pointer_pos: center,
            primary_just_released: true,
            ..Default::default()
        };
        let actions = tree.process_input(&release);
        assert_eq!(actions, vec![UiAction::Clicked("save_btn".into_node_key())]);

        // Test toggle_button
        let play_rect = tree
            .node(tree.node_by_key("play_btn".into_node_key()).unwrap())
            .unwrap()
            .computed_rect;
        let p_center = Vec2::new(
            play_rect.x + play_rect.w * 0.5,
            play_rect.y + play_rect.h * 0.5,
        );
        tree.process_input(&UiInput {
            pointer_pos: p_center,
            primary_down: true,
            primary_just_pressed: true,
            ..Default::default()
        });
        let toggle_actions = tree.process_input(&UiInput {
            pointer_pos: p_center,
            primary_just_released: true,
            ..Default::default()
        });
        assert!(toggle_actions.contains(&UiAction::Toggled {
            key: "play_btn".into_node_key(),
            value: true
        }));
    }

    #[test]
    fn numeric_field_drag_scrub_and_commit() {
        let mut tree = UiTree::new();
        tree.begin_frame();
        {
            let mut ctx = UiContext::new(&mut tree);
            ctx.mount(
                row()
                    .size(Length::Px(300.0), Length::Px(60.0))
                    .child(vec3_field("pos", [1.0, 2.0, 3.0], 0.1)),
            );
        }
        tree.compute_layout(Vec2::new(300.0, 60.0));

        let x_key = ("pos", 0u32).into_node_key();
        let x_rect = tree
            .node(tree.node_by_key(x_key).unwrap())
            .unwrap()
            .computed_rect;
        let start = Vec2::new(x_rect.x + x_rect.w * 0.5, x_rect.y + x_rect.h * 0.5);

        tree.process_input(&UiInput {
            pointer_pos: start,
            primary_down: true,
            primary_just_pressed: true,
            ..Default::default()
        });
        let drag_actions = tree.process_input(&UiInput {
            pointer_pos: start + Vec2::new(20.0, 0.0),
            primary_down: true,
            ..Default::default()
        });
        assert!(matches!(
            drag_actions.as_slice(),
            [UiAction::ValueChanged { key, value }] if *key == x_key && (*value - 2.0).abs() < 1e-4
        ));

        let commit_actions = tree.process_input(&UiInput {
            pointer_pos: start + Vec2::new(20.0, 0.0),
            primary_just_released: true,
            ..Default::default()
        });
        assert!(matches!(
            commit_actions.as_slice(),
            [UiAction::ValueCommitted { key, old, new }]
                if *key == x_key && (*old - 1.0).abs() < 1e-4 && (*new - 2.0).abs() < 1e-4
        ));
    }

    #[test]
    fn text_field_retains_focus_and_edits() {
        let mut tree = UiTree::new();
        tree.begin_frame();
        {
            let mut ctx = UiContext::new(&mut tree);
            ctx.mount(
                row()
                    .size(Length::Px(300.0), Length::Px(40.0))
                    .child(text_field("search", "", "Filter...")),
            );
        }
        tree.compute_layout(Vec2::new(300.0, 40.0));
        let key = "search".into_node_key();
        let rect = tree
            .node(tree.node_by_key(key).unwrap())
            .unwrap()
            .computed_rect;
        let center = Vec2::new(rect.x + 10.0, rect.y + 10.0);

        tree.process_input(&UiInput {
            pointer_pos: center,
            primary_down: true,
            primary_just_pressed: true,
            ..Default::default()
        });
        let typed = tree.process_input(&UiInput {
            text_committed: "Light".to_string(),
            ..Default::default()
        });
        assert_eq!(
            typed,
            vec![UiAction::TextChanged {
                key,
                text: "Light".to_string()
            }]
        );
    }

    #[test]
    fn draw_list_emits_quads_text_and_viewport() {
        let mut tree = UiTree::new();
        tree.begin_frame();
        {
            let mut ctx = UiContext::new(&mut tree);
            ctx.mount(
                row()
                    .size(Length::Px(800.0), Length::Px(600.0))
                    .child(
                        col()
                            .width(Length::Px(200.0))
                            .bg(UiPalette::SURFACE_1)
                            .child(label("Hierarchy")),
                    )
                    .child(viewport(UiTextureId(42))),
            );
        }
        tree.compute_layout(Vec2::new(800.0, 600.0));
        let mut draw = UiDrawList::default();
        tree.build_draw_list(&mut draw);

        assert!(draw.commands.iter().any(|c| matches!(
            c,
            UiDrawCommand::Quad { texture: Some(UiTextureId(42)), rect, .. }
            if (rect.w - 600.0).abs() < 1e-3 && (rect.h - 600.0).abs() < 1e-3
        )));
        assert!(draw.commands.iter().any(|c| matches!(
            c,
            UiDrawCommand::Text { content, .. } if content == "Hierarchy"
        )));
    }
}
