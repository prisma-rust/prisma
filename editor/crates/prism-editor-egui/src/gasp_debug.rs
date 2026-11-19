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
    state: &mut GaspDebugState,
    runtime_graph: Option<&AnimGraphDebug>,
) {
    ui.heading("GASP-Godot Parity Debug");
    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        draw_runtime_section(ui, runtime_graph);
        ui.add_space(6.0);
        draw_traversal_section(ui, state);
        ui.add_space(6.0);
        draw_camera_rigs_section(ui, state);
        ui.add_space(6.0);
        draw_foley_section(ui, state);
    });
}

fn draw_runtime_section(ui: &mut Ui, runtime_graph: Option<&AnimGraphDebug>) {
    ui.collapsing("Live AnimGraph Runtime", |ui| match runtime_graph {
        Some(dbg) => {
            ui.label(format!("Entities: {}", dbg.entities.len()));
            for (entity, nodes) in &dbg.entities {
                ui.group(|ui| {
                    ui.label(
                        RichText::new(format!("Entity {entity:?} — {} node(s)", nodes.len()))
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
            ui.label(
                RichText::new("Enter Play Mode to inspect live AnimGraphDebug entities.").weak(),
            );
        }
    });
}

fn draw_traversal_section(ui: &mut Ui, state: &mut GaspDebugState) {
    ui.collapsing("P4 Traversal & MotionWarping Sandbox", |ui| {
        ui.add(
            egui::Slider::new(&mut state.obstacle_height, 0.1..=3.0).text("Obstacle Height (m)"),
        );
        ui.add(egui::Slider::new(&mut state.obstacle_depth, 0.2..=2.5).text("Obstacle Depth (m)"));
        ui.add(
            egui::Slider::new(&mut state.approach_speed, 0.0..=8.0).text("Approach Speed (m/s)"),
        );
        ui.checkbox(
            &mut state.has_clear_landing,
            "Clear Landing Beyond Obstacle",
        );

        let (action, targets) = state.evaluate_traversal();
        ui.label(RichText::new(format!("Selected Action: {action:?}")).strong());
        for (idx, t) in targets.iter().enumerate() {
            ui.label(format!(
                "  Warp[{idx}]: pos=[{:.2}, {:.2}, {:.2}] t=[{:.2}, {:.2}]",
                t.position[0], t.position[1], t.position[2], t.start_t, t.end_t
            ));
        }
    });
}

fn draw_camera_rigs_section(ui: &mut Ui, state: &mut GaspDebugState) {
    ui.collapsing("P2 GameplayCamera 9 Rigs", |ui| {
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
        ui.label(format!(
            "Target Preset: FOV {:.1} deg | Arm {:.2} m | Lag {:.1}",
            preset.fov_deg, preset.arm_length, preset.position_lag_speed
        ));
        ui.label(format!(
            "Blended Live: FOV {:.1} deg | Arm {:.2} m",
            state.rig_blender.current.fov_deg, state.rig_blender.current.arm_length
        ));
        ui.checkbox(&mut state.camera_hit_enabled, "Simulate Obstruction Hit");
        if state.camera_hit_enabled {
            ui.add(
                egui::Slider::new(&mut state.camera_hit_distance, 0.2..=5.0)
                    .text("Hit Distance (m)"),
            );
        }
        ui.label(
            RichText::new(format!("Clamped Arm Length: {:.2} m", state.clamped_arm())).strong(),
        );
    });
}

fn draw_foley_section(ui: &mut Ui, state: &mut GaspDebugState) {
    ui.collapsing("P6 Foley Notify Dispatcher", |ui| {
        ui.horizontal(|ui| {
            ui.label("Surface:");
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
            ui.label("Notify:");
            ui.text_edit_singleline(&mut state.foley_notify);
        });
        ui.add(egui::Slider::new(&mut state.foley_speed, 0.0..=8.0).text("Speed (m/s)"));
        if ui.button("Dispatch Notify").clicked() {
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
                ui.label(format!(
                    "Resolved: {:?} on {:?} (gain={:.2}, pitch={:.2})",
                    ev.action, ev.surface, ev.gain, ev.pitch
                ));
            }
            None => {
                ui.label(RichText::new("No event (unrecognized notify or in cooldown)").weak());
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
