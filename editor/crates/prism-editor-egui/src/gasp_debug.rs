//! GASP-Godot parity interactive debug & sandbox panel (`#560`).
//!
//! Exposes live inspection and parameter testing for:
//! - Traversal Chooser & MotionWarping targets (`prism_anim::traversal`)
//! - 9 GameplayCamera rigs & collision arm clamping (`prism_anim::camera_rigs`)
//! - Foley audio notify dispatcher (`prism_anim::foley`)
//! - Live Play-mode `AnimGraphDebug` state when running in Game View.

use egui::{RichText, Ui};
use prism_anim::camera_rigs::{CameraRigBlender, CameraRigId, CameraRigParams};
use prism_anim::foley::{FoleyDispatcher, FoleyEvent, SurfaceMaterialKind};
use prism_anim::traversal::{
    choose_traversal, compute_warp_targets, MotionWarpTarget, TraversalAction, TraversalProbe,
};
use prism_engine::scene::systems::animator::AnimGraphDebug;

/// Persistent UI state for the GASP Debug panel.
#[derive(Clone, Debug)]
pub struct GaspDebugState {
    pub obstacle_height: f32,
    pub obstacle_depth: f32,
    pub approach_speed: f32,
    pub has_clear_landing: bool,
    pub selected_rig: CameraRigId,
    pub rig_blender: CameraRigBlender,
    pub camera_hit_enabled: bool,
    pub camera_hit_distance: f32,
    pub foley_surface: SurfaceMaterialKind,
    pub foley_notify: String,
    pub foley_speed: f32,
    pub foley_dispatcher: FoleyDispatcher,
    pub last_foley_event: Option<FoleyEvent>,
}

impl Default for GaspDebugState {
    fn default() -> Self {
        Self {
            obstacle_height: 0.95,
            obstacle_depth: 0.60,
            approach_speed: 3.5,
            has_clear_landing: true,
            selected_rig: CameraRigId::Walk,
            rig_blender: CameraRigBlender::default(),
            camera_hit_enabled: false,
            camera_hit_distance: 2.0,
            foley_surface: SurfaceMaterialKind::Concrete,
            foley_notify: "Footstep.L".to_owned(),
            foley_speed: 3.5,
            foley_dispatcher: FoleyDispatcher::default(),
            last_foley_event: None,
        }
    }
}

impl GaspDebugState {
    /// Evaluate the current traversal sandbox parameters.
    #[must_use]
    pub fn evaluate_traversal(&self) -> (TraversalAction, Vec<MotionWarpTarget>) {
        let probe = TraversalProbe {
            obstacle_height: self.obstacle_height,
            obstacle_depth: self.obstacle_depth,
            front_edge: [0.0, self.obstacle_height, -0.5],
            top_edge: [0.0, self.obstacle_height, -0.5 - self.obstacle_depth * 0.5],
            exit_edge: [0.0, 0.0, -0.5 - self.obstacle_depth - 0.6],
            has_clear_landing: self.has_clear_landing,
        };
        let action = choose_traversal(&probe, self.approach_speed);
        let targets = compute_warp_targets(&probe, action);
        (action, targets)
    }

    /// Compute the effective clamped camera arm length for the active rig.
    #[must_use]
    pub fn clamped_arm(&self) -> f32 {
        let hit = self.camera_hit_enabled.then_some(self.camera_hit_distance);
        self.rig_blender.clamped_arm_length(hit)
    }
}

/// Draw the GASP Debug panel.
pub fn gasp_debug_panel(
    ui: &mut Ui,
    workspace: &prism_editor_core::Workspace,
    state: &mut GaspDebugState,
    runtime_graph: Option<&AnimGraphDebug>,
) {
    ui.heading(workspace.t("desktop.gasp.title"));
    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        draw_runtime_section(ui, workspace, runtime_graph);
        ui.add_space(6.0);
        draw_traversal_section(ui, workspace, state);
        ui.add_space(6.0);
        draw_camera_rigs_section(ui, workspace, state);
        ui.add_space(6.0);
        draw_foley_section(ui, workspace, state);
    });
}

fn draw_runtime_section(
    ui: &mut Ui,
    workspace: &prism_editor_core::Workspace,
    runtime_graph: Option<&AnimGraphDebug>,
) {
    ui.collapsing(
        workspace.t("desktop.gasp.runtime"),
        |ui| match runtime_graph {
            Some(dbg) => {
                ui.label(workspace.tf(
                    "desktop.gasp.entities",
                    &[("count", &dbg.entities.len().to_string())],
                ));
                for (entity, nodes) in &dbg.entities {
                    ui.group(|ui| {
                        ui.label(
                            RichText::new(workspace.tf(
                                "desktop.gasp.entityNodes",
                                &[
                                    ("entity", &format!("{entity:?}")),
                                    ("count", &nodes.len().to_string()),
                                ],
                            ))
                            .strong(),
                        );
                        for node in nodes {
                            ui.label(format!(
                                "  '{}' ({:?}): state={}, t={:.2}s, blend={:.2}",
                                node.name, node.kind, node.state, node.time, node.blend_weight
                            ));
                        }
                    });
                }
            }
            None => {
                ui.label(RichText::new(workspace.t("desktop.gasp.enterPlay")).weak());
            }
        },
    );
}

fn draw_traversal_section(
    ui: &mut Ui,
    workspace: &prism_editor_core::Workspace,
    state: &mut GaspDebugState,
) {
    ui.collapsing(workspace.t("desktop.gasp.traversal"), |ui| {
        ui.add(
            egui::Slider::new(&mut state.obstacle_height, 0.1..=3.0)
                .text(workspace.t("desktop.gasp.obstacleHeight")),
        );
        ui.add(
            egui::Slider::new(&mut state.obstacle_depth, 0.2..=2.5)
                .text(workspace.t("desktop.gasp.obstacleDepth")),
        );
        ui.add(
            egui::Slider::new(&mut state.approach_speed, 0.0..=8.0)
                .text(workspace.t("desktop.gasp.approachSpeed")),
        );
        ui.checkbox(
            &mut state.has_clear_landing,
            workspace.t("desktop.gasp.clearLanding"),
        );

        let (action, targets) = state.evaluate_traversal();
        ui.label(
            RichText::new(
                workspace.tf("desktop.gasp.action", &[("action", &format!("{action:?}"))]),
            )
            .strong(),
        );
        for (idx, t) in targets.iter().enumerate() {
            ui.label(format!(
                "  Warp[{idx}]: pos=[{:.2}, {:.2}, {:.2}] t=[{:.2}, {:.2}]",
                t.position[0], t.position[1], t.position[2], t.start_t, t.end_t
            ));
        }
    });
}

fn draw_camera_rigs_section(
    ui: &mut Ui,
    workspace: &prism_editor_core::Workspace,
    state: &mut GaspDebugState,
) {
    ui.collapsing(workspace.t("desktop.gasp.cameraRigs"), |ui| {
        ui.horizontal_wrapped(|ui| {
            for rig in CameraRigId::ALL {
                let selected = state.selected_rig == rig;
                if ui.selectable_label(selected, format!("{rig:?}")).clicked() {
                    state.selected_rig = rig;
                }
            }
        });
        let dt = ui.input(|i| i.stable_dt.clamp(0.0, 0.1));
        state.rig_blender.step(state.selected_rig, dt);
        let preset = CameraRigParams::preset(state.selected_rig);
        ui.label(workspace.tf(
            "desktop.gasp.targetPreset",
            &[
                ("fov", &format!("{:.1}", preset.fov_deg)),
                ("arm", &format!("{:.2}", preset.arm_length)),
                ("lag", &format!("{:.1}", preset.position_lag_speed)),
            ],
        ));
        ui.label(workspace.tf(
            "desktop.gasp.blendedLive",
            &[
                ("fov", &format!("{:.1}", state.rig_blender.current.fov_deg)),
                (
                    "arm",
                    &format!("{:.2}", state.rig_blender.current.arm_length),
                ),
            ],
        ));
        ui.checkbox(
            &mut state.camera_hit_enabled,
            workspace.t("desktop.gasp.simulateHit"),
        );
        if state.camera_hit_enabled {
            ui.add(
                egui::Slider::new(&mut state.camera_hit_distance, 0.2..=5.0)
                    .text(workspace.t("desktop.gasp.hitDistance")),
            );
        }
        ui.label(
            RichText::new(workspace.tf(
                "desktop.gasp.clampedArm",
                &[("arm", &format!("{:.2}", state.clamped_arm()))],
            ))
            .strong(),
        );
    });
}

fn draw_foley_section(
    ui: &mut Ui,
    workspace: &prism_editor_core::Workspace,
    state: &mut GaspDebugState,
) {
    ui.collapsing(workspace.t("desktop.gasp.foley"), |ui| {
        ui.horizontal(|ui| {
            ui.label(workspace.t("desktop.gasp.surface"));
            for surf in [
                SurfaceMaterialKind::Default,
                SurfaceMaterialKind::Concrete,
                SurfaceMaterialKind::Wood,
                SurfaceMaterialKind::Metal,
                SurfaceMaterialKind::Grass,
                SurfaceMaterialKind::Water,
            ] {
                if ui
                    .selectable_label(state.foley_surface == surf, format!("{surf:?}"))
                    .clicked()
                {
                    state.foley_surface = surf;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label(workspace.t("desktop.gasp.notify"));
            ui.text_edit_singleline(&mut state.foley_notify);
        });
        ui.add(
            egui::Slider::new(&mut state.foley_speed, 0.0..=8.0)
                .text(workspace.t("desktop.gasp.speed")),
        );
        if ui.button(workspace.t("desktop.gasp.dispatch")).clicked() {
            let now = ui.input(|i| i.time as f32);
            state.last_foley_event = state.foley_dispatcher.try_dispatch(
                &state.foley_notify,
                state.foley_surface,
                state.foley_speed,
                now,
            );
        }
        match state.last_foley_event {
            Some(ev) => {
                ui.label(workspace.tf(
                    "desktop.gasp.resolved",
                    &[
                        ("action", &format!("{:?}", ev.action)),
                        ("surface", &format!("{:?}", ev.surface)),
                        ("gain", &format!("{:.2}", ev.gain)),
                        ("pitch", &format!("{:.2}", ev.pitch)),
                    ],
                ));
            }
            None => {
                ui.label(RichText::new(workspace.t("desktop.gasp.noEvent")).weak());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_sandbox_selects_expected_actions() {
        let mut state = GaspDebugState::default();
        let (action, targets) = state.evaluate_traversal();
        assert_eq!(action, TraversalAction::Vault);
        assert!(!targets.is_empty());

        state.obstacle_height = 1.8;
        let (action_high, _) = state.evaluate_traversal();
        assert_eq!(action_high, TraversalAction::MantleHigh);
    }

    #[test]
    fn camera_rig_clamping_respects_obstruction() {
        let mut state = GaspDebugState {
            camera_hit_enabled: true,
            camera_hit_distance: 1.0,
            ..Default::default()
        };
        assert!(state.clamped_arm() < 1.0);
        state.camera_hit_enabled = false;
        assert!(
            (state.clamped_arm() - CameraRigParams::preset(CameraRigId::Walk).arm_length).abs()
                < 1e-4
        );
    }

    #[test]
    fn foley_dispatcher_resolves_and_deduplicates() {
        let mut state = GaspDebugState::default();
        let first =
            state
                .foley_dispatcher
                .try_dispatch("Footstep.L", SurfaceMaterialKind::Metal, 4.0, 1.0);
        assert!(first.is_some());
        let dup = state.foley_dispatcher.try_dispatch(
            "Footstep.L",
            SurfaceMaterialKind::Metal,
            4.0,
            1.02,
        );
        assert!(dup.is_none());
    }
}
