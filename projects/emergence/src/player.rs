//! First-person 3D player controller, swept L1/L2/L3 voxel collision, aiming tools, and HUD.

use glam::{EulerRot, Quat, Vec3};
use prism_game_api::{keys, EntityHandle, GameWorld, InputSnapshot};
use prism_scene::ui::{
    Anchors, Canvas, CanvasScaleMode, Margin, Node, Pivot, Style, Text, TextAlign,
};
use prism_scene::{
    Active, Camera, FlyCameraController, LocalTransform, Name, VoxelCursorHighlight, VoxelHit,
    VoxelTier, VOXEL_L1_SIZE_M, VOXEL_L2_SIZE_M, VOXEL_L3_SIZE_M,
};

use crate::materials::mat_id;
use crate::sim::SimWorld;
use crate::tags::{StimulusFlags, TransientFlags};

const PLAYER_RADIUS_M: f32 = 0.26;
const PLAYER_HEIGHT_M: f32 = 1.68;
const PLAYER_EYE_HEIGHT_M: f32 = 1.50;
const STEP_HEIGHT_M: f32 = 0.28;
const WALK_SPEED_MPS: f32 = 4.4;
const SPRINT_SPEED_MPS: f32 = 7.4;
const FLY_SPEED_MPS: f32 = 9.0;
const GRAVITY_MPS2: f32 = 18.0;
const JUMP_VEL_MPS: f32 = 6.8;
const MAX_REACH_M: f32 = 14.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolAction {
    PlaceMaterial(u8),
    StimHeat,
    StimShock,
    StimWet,
    StimCold,
}

impl ToolAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::PlaceMaterial(id) => match id {
                mat_id::ALLOY => "Place Alloy",
                mat_id::GLASS => "Place Glass",
                mat_id::TIMBER => "Place Timber",
                mat_id::WATER => "Pour Water",
                mat_id::ACID => "Pour Acid",
                mat_id::CRYO => "Pour Cryo",
                mat_id::BIOMASS => "Seed Biomass",
                mat_id::WIRE => "Lay Wire",
                mat_id::ISOTOPE => "Place Isotope",
                _ => "Place Stone",
            },
            Self::StimHeat => "Stim: Heat/Ignite",
            Self::StimShock => "Stim: Shock [E]",
            Self::StimWet => "Stim: Splash Wet",
            Self::StimCold => "Stim: Cryo Cold",
        }
    }
}

const HOTBAR_TOOLS: [ToolAction; 10] = [
    ToolAction::PlaceMaterial(mat_id::ALLOY),
    ToolAction::PlaceMaterial(mat_id::GLASS),
    ToolAction::PlaceMaterial(mat_id::TIMBER),
    ToolAction::PlaceMaterial(mat_id::WATER),
    ToolAction::PlaceMaterial(mat_id::ACID),
    ToolAction::PlaceMaterial(mat_id::CRYO),
    ToolAction::PlaceMaterial(mat_id::BIOMASS),
    ToolAction::PlaceMaterial(mat_id::WIRE),
    ToolAction::StimHeat,
    ToolAction::StimShock,
];

/// Active voxel interaction granularity mode: `Auto` adapts to hit tier, or locked `L1`/`L2`/`L3`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierMode {
    Auto,
    Fixed(VoxelTier),
}

/// First-person player kinematics, aiming probe, and HUD entity bindings.
pub struct PlayerController {
    pub feet_pos: Vec3,
    pub velocity: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub grounded: bool,
    pub fly_mode: bool,
    pub tier: VoxelTier,
    pub tier_mode: TierMode,
    pub tool_index: usize,
    coyote_timer: f32,
    jump_buffer: f32,
    camera_step_offset_y: f32,
    was_pointer_locked: bool,
    was_unlocked_right_held: bool,
    unlocked_right_drag_px: f32,
    last_synced_camera_rot: Option<Quat>,
    camera_entity: Option<EntityHandle>,
    cursor_entity: Option<EntityHandle>,
    hud_panel: Option<EntityHandle>,
    hud_status: Option<EntityHandle>,
    hud_probe: Option<EntityHandle>,
    crosshair: Option<EntityHandle>,
    last_hit: Option<VoxelHit>,
}

impl Default for PlayerController {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayerController {
    pub fn new() -> Self {
        Self {
            feet_pos: Vec3::new(6.5, 4.05, 6.5),
            velocity: Vec3::ZERO,
            yaw: -2.356, // Look northeast from (6.5, 6.5) across water pond toward Research Outpost.
            pitch: -0.14,
            grounded: true,
            fly_mode: false,
            tier: VoxelTier::L2,
            tier_mode: TierMode::Auto,
            tool_index: 0,
            coyote_timer: 0.0,
            jump_buffer: 0.0,
            camera_step_offset_y: 0.0,
            was_pointer_locked: false,
            was_unlocked_right_held: false,
            unlocked_right_drag_px: 0.0,
            last_synced_camera_rot: None,
            camera_entity: None,
            cursor_entity: None,
            hud_panel: None,
            hud_status: None,
            hud_probe: None,
            crosshair: None,
            last_hit: None,
        }
    }

    pub fn eye_pos(&self) -> Vec3 {
        self.feet_pos + Vec3::new(0.0, PLAYER_EYE_HEIGHT_M + self.camera_step_offset_y, 0.0)
    }

    pub fn active_tier(&self, hit: Option<&VoxelHit>) -> VoxelTier {
        match self.tier_mode {
            TierMode::Auto => hit.map(|h| h.hit_tier).unwrap_or(self.tier),
            TierMode::Fixed(t) => t,
        }
    }

    pub fn camera_rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0)
    }

    pub fn forward_dir(&self) -> Vec3 {
        (self.camera_rotation() * -Vec3::Z).normalize()
    }

    fn resolve_active_camera(world: &GameWorld<'_>) -> Option<EntityHandle> {
        let mut best: Option<(f32, EntityHandle)> = None;
        for entity in world.query::<Camera>() {
            let Some(cam) = world.get::<Camera>(entity) else {
                continue;
            };
            if !cam.enabled {
                continue;
            }
            if best.is_none_or(|(d, _)| cam.depth >= d) {
                best = Some((cam.depth, entity));
            }
        }
        best.map(|(_, entity)| entity)
            .or_else(|| world.find_by_name("camera"))
    }

    /// Extract gimbal-safe `(yaw, pitch)` matching an arbitrary camera orientation quaternion.
    fn apply_camera_rotation(&mut self, rot: Quat) {
        let norm = rot.normalize();
        if !norm.is_finite() {
            return;
        }
        let fwd = (norm * -Vec3::Z).normalize_or(Vec3::NEG_Z);
        self.pitch = fwd.y.clamp(-1.0, 1.0).asin().clamp(-1.52, 1.52);
        if fwd.x * fwd.x + fwd.z * fwd.z > 1e-6 {
            self.yaw = (-fwd.x).atan2(-fwd.z);
        }
    }

    /// Pull external camera rotation (`FlyCameraController` or `LocalTransform`) into player heading.
    ///
    /// Returns `true` if a `FlyCameraController` is attached and owns pointer-locked mouse look.
    fn sync_from_camera_if_modified(&mut self, world: &GameWorld<'_>) -> bool {
        if let Some(active) = Self::resolve_active_camera(world) {
            if self.camera_entity != Some(active) {
                self.camera_entity = Some(active);
                self.last_synced_camera_rot = None;
            }
        }
        let Some(cam) = self.camera_entity else {
            return false;
        };
        if let Some(ctrl) = world.get::<FlyCameraController>(cam) {
            self.yaw = ctrl.yaw;
            self.pitch = ctrl.pitch.clamp(-1.52, 1.52);
            self.last_synced_camera_rot = Some(self.camera_rotation());
            return true;
        }
        let Some(lt) = world.get::<LocalTransform>(cam) else {
            return false;
        };
        let cam_rot = lt.rotation.normalize();
        if !cam_rot.is_finite() {
            return false;
        }
        let externally_modified = match self.last_synced_camera_rot {
            Some(prev) => 1.0 - cam_rot.dot(prev).abs() > 1e-5,
            None => true,
        };
        if externally_modified {
            self.apply_camera_rotation(cam_rot);
            self.last_synced_camera_rot = Some(cam_rot);
        }
        false
    }

    fn live_camera_rotation(&self, world: &GameWorld<'_>) -> Quat {
        if let Some(cam) = self.camera_entity {
            if let Some(ctrl) = world.get::<FlyCameraController>(cam) {
                return Quat::from_euler(
                    EulerRot::YXZ,
                    ctrl.yaw,
                    ctrl.pitch.clamp(-1.52, 1.52),
                    0.0,
                );
            }
            if let Some(lt) = world.get::<LocalTransform>(cam) {
                let norm = lt.rotation.normalize();
                if norm.is_finite() {
                    return norm;
                }
            }
        }
        self.camera_rotation()
    }

    /// Ensure camera, 3D wireframe cursor entity, and HUD overlay entities exist in `GameWorld`.
    pub fn attach_ecs(&mut self, world: &mut GameWorld<'_>, sim: &SimWorld) {
        if self.camera_entity.is_none() {
            if let Some(cam) = Self::resolve_active_camera(world) {
                self.camera_entity = Some(cam);
                if let Some(lt) = world.get::<LocalTransform>(cam) {
                    // Use authored courtyard overlook on initial frame while snapping cleanly above terrain.
                    self.feet_pos = Vec3::new(
                        lt.position[0],
                        (lt.position[1] - PLAYER_EYE_HEIGHT_M).max(4.02),
                        lt.position[2],
                    );
                    self.apply_camera_rotation(lt.rotation);
                    self.last_synced_camera_rot = Some(self.camera_rotation());
                }
            } else {
                let cam = world.spawn();
                world.set(cam, Name("camera".to_string()));
                world.set(cam, Active(true));
                world.set(
                    cam,
                    prism_scene::Camera {
                        fov_y_degrees: 65.0,
                        near: 0.08,
                        far: 256.0,
                        orthographic: false,
                        clear_flags: prism_scene::CameraClearFlags::SolidColor,
                        background_color: [0.22, 0.36, 0.58, 1.0],
                        ..prism_scene::Camera::default()
                    },
                );
                self.camera_entity = Some(cam);
            }
            // Push feet above ground if spawn overlapped solid terrain.
            while sim
                .aabb_intersects_solid(self.aabb_min(self.feet_pos), self.aabb_max(self.feet_pos))
                && self.feet_pos.y < 58.0
            {
                self.feet_pos.y += 0.25;
            }
            self.sync_camera_transform(world);
        }

        if self.cursor_entity.is_none() {
            let cur = world.spawn();
            world.set(cur, Name("voxel_cursor".to_string()));
            world.set(cur, Active(true));
            world.set(cur, VoxelCursorHighlight::default());
            self.cursor_entity = Some(cur);
        }

        if self.crosshair.is_none() {
            self.spawn_hud(world);
        }
    }

    fn spawn_hud(&mut self, world: &mut GameWorld<'_>) {
        // Translucent dark backdrop panel in top-left corner for legible readout.
        let panel = world.spawn();
        world.set(panel, Name("hud_panel".to_string()));
        world.set(panel, Node::default());
        world.set(panel, hud_canvas());
        world.set(
            panel,
            Style {
                width: Some(820.0),
                height: Some(82.0),
                anchors: Anchors::TOP_LEFT,
                pivot: Pivot::TOP_LEFT,
                margin: Margin {
                    left: 12.0,
                    top: 12.0,
                    ..Margin::default()
                },
                background: [0.04, 0.07, 0.12, 0.72],
                border_radius: 6.0,
                ..Style::default()
            },
        );
        world.set(panel, Active(true));
        self.hud_panel = Some(panel);

        self.hud_status = Some(spawn_hud_text(
            world,
            "hud_status",
            "Emergence v3.0 MVP",
            17.0,
            22.0,
            20.0,
            790.0,
            [0.92, 0.96, 1.0, 1.0],
        ));
        self.hud_probe = Some(spawn_hud_text(
            world,
            "hud_probe",
            "Aim at voxel to inspect...",
            15.0,
            22.0,
            46.0,
            790.0,
            [0.72, 0.92, 0.85, 1.0],
        ));

        let ch = world.spawn();
        world.set(ch, Name("hud_crosshair".to_string()));
        world.set(ch, Node::default());
        world.set(ch, hud_canvas());
        world.set(
            ch,
            Style {
                width: Some(24.0),
                height: Some(24.0),
                anchors: Anchors::CENTER,
                pivot: Pivot::CENTER,
                ..Style::default()
            },
        );
        world.set(
            ch,
            Text {
                content: "+".to_string(),
                font_size: 22.0,
                color: [1.0, 0.96, 0.65, 0.92],
                alignment: TextAlign::Center,
                ..Text::default()
            },
        );
        world.set(ch, Active(true));
        self.crosshair = Some(ch);
    }

    /// Process frame-responsive mouse look and hotbar selection (`update`).
    pub fn update_look_and_tools(
        &mut self,
        world: &mut GameWorld<'_>,
        sim: &mut SimWorld,
        dt: f32,
    ) {
        let has_fly_controller = self.sync_from_camera_if_modified(world);
        let input = world.input();

        // Look via mouse delta while locked, plus drag-look and arrow-key fallback when unlocked.
        let unlocked_drag = !input.pointer_locked
            && (input.right_held
                || input.middle_held
                || (!self.was_pointer_locked && input.left_held));
        let allow_mouse_look = (!has_fly_controller && input.pointer_locked) || unlocked_drag;
        let prev_yaw = self.yaw;
        let prev_pitch = self.pitch;
        if allow_mouse_look {
            self.yaw -= input.mouse_delta[0] * 0.0024;
            self.pitch = (self.pitch - input.mouse_delta[1] * 0.0024).clamp(-1.52, 1.52);
        }
        let key_turn = 1.8 * dt;
        if input.key(keys::ARROW_LEFT) {
            self.yaw += key_turn;
        }
        if input.key(keys::ARROW_RIGHT) {
            self.yaw -= key_turn;
        }
        if input.key(keys::ARROW_UP) {
            self.pitch = (self.pitch + key_turn * 0.7).clamp(-1.52, 1.52);
        }
        if input.key(keys::ARROW_DOWN) {
            self.pitch = (self.pitch - key_turn * 0.7).clamp(-1.52, 1.52);
        }
        if has_fly_controller
            && ((self.yaw - prev_yaw).abs() > 1e-6 || (self.pitch - prev_pitch).abs() > 1e-6)
        {
            if let Some(cam) = self.camera_entity {
                if let Some(mut ctrl) = world.get::<FlyCameraController>(cam) {
                    ctrl.yaw = self.yaw;
                    ctrl.pitch = self.pitch;
                    world.set(cam, ctrl);
                }
            }
        }

        // Track unlocked RMB click vs drag-look so dragging does not spuriously trigger placement.
        let mouse_move_px = (input.mouse_delta[0].powi(2) + input.mouse_delta[1].powi(2)).sqrt();
        let unlocked_right_click = if input.pointer_locked {
            self.was_unlocked_right_held = false;
            self.unlocked_right_drag_px = 0.0;
            false
        } else if input.right_held {
            if !self.was_unlocked_right_held {
                self.unlocked_right_drag_px = 0.0;
            }
            self.unlocked_right_drag_px += mouse_move_px;
            self.was_unlocked_right_held = true;
            false
        } else {
            let released_click = self.was_unlocked_right_held && self.unlocked_right_drag_px < 6.0;
            self.was_unlocked_right_held = false;
            self.unlocked_right_drag_px = 0.0;
            released_click || input.right_clicked
        };
        let use_tool = (input.pointer_locked && input.right_clicked) || unlocked_right_click;

        // Toggle Walk / Fly with Tab or V.
        if input.key_pressed(keys::TAB) || input.key_pressed(keys::OTHER_BASE + b'V' as usize) {
            self.fly_mode = !self.fly_mode;
            self.velocity = Vec3::ZERO;
        }

        // Switch 3-tier voxel editing granularity: 1 = L1 (1m), 2 = L2 (0.25m), 3 = L3 (0.0625m), press again or T = Auto.
        for (key, target_tier) in [
            (keys::DIGIT_1, VoxelTier::L1),
            (keys::DIGIT_2, VoxelTier::L2),
            (keys::DIGIT_3, VoxelTier::L3),
        ] {
            if input.key_pressed(key) {
                self.tier_mode = if self.tier_mode == TierMode::Fixed(target_tier) {
                    TierMode::Auto
                } else {
                    TierMode::Fixed(target_tier)
                };
            }
        }
        if input.key_pressed(keys::KEY_T) {
            self.tier_mode = match self.tier_mode {
                TierMode::Auto => TierMode::Fixed(self.tier),
                TierMode::Fixed(_) => TierMode::Auto,
            };
        }

        // Smoothly glide camera eye height over stepped ledges without abrupt pops.
        if self.camera_step_offset_y.abs() > 1e-4 {
            self.camera_step_offset_y *= (-dt.clamp(0.0, 0.05) * 18.0).exp();
            if self.camera_step_offset_y.abs() <= 1e-4 {
                self.camera_step_offset_y = 0.0;
            }
        }

        // Hotbar slots via digits 4..=9, 0 or Q / scroll wheel.
        for (slot_idx, key) in [
            (0, keys::DIGIT_4),
            (1, keys::DIGIT_5),
            (2, keys::DIGIT_6),
            (3, keys::DIGIT_7),
            (4, keys::DIGIT_8),
            (5, keys::DIGIT_9),
            (6, keys::DIGIT_0),
        ] {
            if input.key_pressed(key) {
                self.tool_index = slot_idx;
            }
        }
        if input.key_pressed(keys::KEY_Q) {
            self.tool_index = (self.tool_index + 1) % HOTBAR_TOOLS.len();
        }
        if input.scroll.abs() > 0.1 {
            let delta = if input.scroll > 0.0 { 1isize } else { -1isize };
            let len = HOTBAR_TOOLS.len() as isize;
            self.tool_index = ((self.tool_index as isize + delta).rem_euclid(len)) as usize;
        }

        if input.key_pressed(keys::SPACE) {
            self.jump_buffer = 0.12;
        }

        self.sync_camera_transform(world);

        // Aiming raycast across L1/L2/L3 chunks & cursor wireframe update from live camera basis.
        let eye = self.eye_pos();
        let cam_rot = self.live_camera_rotation(world);
        let fwd = (cam_rot * -Vec3::Z).normalize_or(self.forward_dir());
        let tool = HOTBAR_TOOLS[self.tool_index];
        let hit_liquids = matches!(
            tool,
            ToolAction::StimCold
                | ToolAction::StimHeat
                | ToolAction::StimShock
                | ToolAction::StimWet
        );
        self.last_hit = sim.raycast(eye.to_array(), fwd.to_array(), MAX_REACH_M, hit_liquids);
        if self.last_hit.is_none() && hit_liquids {
            self.last_hit = sim.raycast(eye.to_array(), fwd.to_array(), MAX_REACH_M, true);
        }
        self.tier = self.active_tier(self.last_hit.as_ref());

        if let Some(cur_ent) = self.cursor_entity {
            if let Some(hit) = self.last_hit {
                let (bmin, bmax) = SimWorld::cursor_bounds_for_hit(self.tier, &hit);
                let color = match self.tier {
                    VoxelTier::L1 => [1.0, 0.55, 0.20, 0.95],
                    VoxelTier::L2 => [1.0, 0.92, 0.25, 0.95],
                    VoxelTier::L3 => [0.25, 0.95, 1.0, 0.98],
                };
                world.set(
                    cur_ent,
                    VoxelCursorHighlight {
                        active: true,
                        min: bmin,
                        max: bmax,
                        color,
                    },
                );
            } else {
                world.set(cur_ent, VoxelCursorHighlight::default());
            }
        }

        // Interaction: Carve (Left Click), Place / Tool (Right Click), Quick Stimuli (F/E/R/G).
        if let Some(hit) = self.last_hit {
            if self.was_pointer_locked && input.left_clicked {
                sim.carve_at_hit(self.tier, &hit);
            }
            if use_tool {
                match tool {
                    ToolAction::PlaceMaterial(mat) => {
                        let place_pos = self.placement_world_center(&hit);
                        if !self.overlaps_player_voxel(place_pos, self.tier) {
                            sim.place_adjacent(self.tier, &hit, mat);
                        }
                    }
                    ToolAction::StimHeat => {
                        sim.inject_stimulus(hit.world_l2, StimulusFlags::HEAT, false, 1)
                    }
                    ToolAction::StimShock => {
                        sim.inject_stimulus(hit.world_l2, StimulusFlags::SHOCK, false, 1)
                    }
                    ToolAction::StimWet => {
                        sim.inject_stimulus(hit.world_l2, StimulusFlags::NONE, true, 1)
                    }
                    ToolAction::StimCold => {
                        sim.inject_stimulus(hit.world_l2, StimulusFlags::COLD, false, 1)
                    }
                }
            }
            if input.key_pressed(keys::OTHER_BASE + b'F' as usize) {
                sim.inject_stimulus(hit.world_l2, StimulusFlags::HEAT, false, 1);
            }
            if input.key_pressed(keys::KEY_E) {
                sim.inject_stimulus(hit.world_l2, StimulusFlags::SHOCK, false, 1);
            }
            if input.key_pressed(keys::OTHER_BASE + b'R' as usize) {
                sim.inject_stimulus(hit.world_l2, StimulusFlags::NONE, true, 1);
            }
            if input.key_pressed(keys::OTHER_BASE + b'G' as usize) {
                sim.inject_stimulus(hit.world_l2, StimulusFlags::COLD, false, 1);
            }
        }

        self.was_pointer_locked = input.pointer_locked;
        self.update_hud_text(world, sim);
    }

    /// Fixed-timestep kinematics & swept three-tier voxel AABB collision (`fixed_update`).
    pub fn step_physics(&mut self, world: &mut GameWorld<'_>, sim: &SimWorld, dt: f32) {
        self.sync_from_camera_if_modified(world);
        let input = world.input();
        let cam_rot = self.live_camera_rotation(world);
        let wish_dir = horizontal_wish_dir_for_rot(&input, cam_rot, self.yaw);
        let speed = if input.key(keys::SHIFT_LEFT) || input.key(keys::SHIFT_RIGHT) {
            SPRINT_SPEED_MPS
        } else if self.fly_mode {
            FLY_SPEED_MPS
        } else {
            WALK_SPEED_MPS
        };

        if self.fly_mode {
            let mut move_vec = wish_dir;
            if input.key(keys::SPACE) {
                move_vec.y += 1.0;
            }
            if input.key(keys::CONTROL_LEFT) || input.key(keys::OTHER_BASE + b'C' as usize) {
                move_vec.y -= 1.0;
            }
            if move_vec.length_squared() > 1e-4 {
                move_vec = move_vec.normalize() * speed * dt;
                self.move_axis_with_collision(sim, 0, move_vec.x, false);
                self.move_axis_with_collision(sim, 1, move_vec.y, false);
                self.move_axis_with_collision(sim, 2, move_vec.z, false);
            }
            self.sync_camera_transform(world);
            return;
        }

        // Walk mode kinematics with gravity, coyote time, jump buffer, and L2 step-up.
        self.velocity.x = wish_dir.x * speed;
        self.velocity.z = wish_dir.z * speed;

        if self.grounded {
            self.coyote_timer = 0.12;
        } else {
            self.coyote_timer = (self.coyote_timer - dt).max(0.0);
        }
        self.jump_buffer = (self.jump_buffer - dt).max(0.0);

        if self.jump_buffer > 0.0 && self.coyote_timer > 0.0 {
            self.velocity.y = JUMP_VEL_MPS;
            self.grounded = false;
            self.coyote_timer = 0.0;
            self.jump_buffer = 0.0;
        }

        self.velocity.y = (self.velocity.y - GRAVITY_MPS2 * dt).max(-28.0);

        let pre_step_y = self.feet_pos.y;
        self.move_axis_with_collision(sim, 0, self.velocity.x * dt, true);
        self.move_axis_with_collision(sim, 2, self.velocity.z * dt, true);
        let step_lift = self.feet_pos.y - pre_step_y;
        if step_lift > 1e-4 {
            // Compensate camera eye height so stepping up glides smoothly across render frames.
            self.camera_step_offset_y = (self.camera_step_offset_y - step_lift).clamp(-0.6, 0.0);
            self.grounded = true;
            if self.velocity.y < 0.0 {
                self.velocity.y = 0.0;
            }
        }

        let dy = self.velocity.y * dt;
        let hit_y = self.move_axis_with_collision(sim, 1, dy, false);
        if hit_y {
            if dy < 0.0 {
                self.grounded = true;
            }
            self.velocity.y = 0.0;
        } else if step_lift <= 1e-4 && dy.abs() > 1e-4 {
            self.grounded = false;
        }

        // Clamp within sandbox world safety envelope.
        self.feet_pos.x = self.feet_pos.x.clamp(0.4, 31.6);
        self.feet_pos.z = self.feet_pos.z.clamp(0.4, 31.6);
        if self.feet_pos.y < 1.0 {
            self.feet_pos.y = 6.0;
            self.velocity.y = 0.0;
            self.camera_step_offset_y = 0.0;
        }

        self.sync_camera_transform(world);
    }

    fn aabb_min(&self, feet: Vec3) -> [f32; 3] {
        [feet.x - PLAYER_RADIUS_M, feet.y, feet.z - PLAYER_RADIUS_M]
    }

    fn aabb_max(&self, feet: Vec3) -> [f32; 3] {
        [
            feet.x + PLAYER_RADIUS_M,
            feet.y + PLAYER_HEIGHT_M,
            feet.z + PLAYER_RADIUS_M,
        ]
    }

    fn move_axis_with_collision(
        &mut self,
        sim: &SimWorld,
        axis: usize,
        delta: f32,
        allow_step: bool,
    ) -> bool {
        if delta.abs() < 1e-6 {
            return false;
        }
        let mut candidate = self.feet_pos;
        candidate[axis] += delta;
        if !sim.aabb_intersects_solid(self.aabb_min(candidate), self.aabb_max(candidate)) {
            self.feet_pos = candidate;
            return false;
        }
        let can_step = allow_step
            && axis != 1
            && (self.grounded || self.coyote_timer > 0.0 || self.velocity.y <= 0.5);
        if can_step {
            let mut max_stepped = candidate;
            max_stepped.y += STEP_HEIGHT_M;
            if !sim.aabb_intersects_solid(self.aabb_min(max_stepped), self.aabb_max(max_stepped)) {
                // Find the minimal vertical clearance so L2/L3 steps never overshoot or bounce.
                let mut lo = 0.0f32;
                let mut hi = STEP_HEIGHT_M;
                for _ in 0..8 {
                    let mid = (lo + hi) * 0.5;
                    let mut p = candidate;
                    p.y += mid;
                    if sim.aabb_intersects_solid(self.aabb_min(p), self.aabb_max(p)) {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                let mut exact = candidate;
                exact.y += (hi + 0.0005).min(STEP_HEIGHT_M);
                self.feet_pos =
                    if !sim.aabb_intersects_solid(self.aabb_min(exact), self.aabb_max(exact)) {
                        exact
                    } else {
                        max_stepped
                    };
                return false;
            }
        }
        // Binary refine remaining gap cleanly up to surface face.
        let mut lo = 0.0f32;
        let mut hi = delta;
        for _ in 0..5 {
            let mid = (lo + hi) * 0.5;
            let mut p = self.feet_pos;
            p[axis] += mid;
            if sim.aabb_intersects_solid(self.aabb_min(p), self.aabb_max(p)) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        self.feet_pos[axis] += lo;
        true
    }

    fn placement_world_center(&self, hit: &VoxelHit) -> [f32; 3] {
        match self.tier {
            VoxelTier::L1 => [
                (hit.world_l1[0] + hit.normal[0]) as f32 + 0.5,
                (hit.world_l1[1] + hit.normal[1]) as f32 + 0.5,
                (hit.world_l1[2] + hit.normal[2]) as f32 + 0.5,
            ],
            VoxelTier::L2 => [
                ((hit.world_l2[0] + hit.normal[0]) as f32 + 0.5) * VOXEL_L2_SIZE_M,
                ((hit.world_l2[1] + hit.normal[1]) as f32 + 0.5) * VOXEL_L2_SIZE_M,
                ((hit.world_l2[2] + hit.normal[2]) as f32 + 0.5) * VOXEL_L2_SIZE_M,
            ],
            VoxelTier::L3 => [
                ((hit.world_l3[0] + hit.normal[0]) as f32 + 0.5) * VOXEL_L3_SIZE_M,
                ((hit.world_l3[1] + hit.normal[1]) as f32 + 0.5) * VOXEL_L3_SIZE_M,
                ((hit.world_l3[2] + hit.normal[2]) as f32 + 0.5) * VOXEL_L3_SIZE_M,
            ],
        }
    }

    fn overlaps_player_voxel(&self, center: [f32; 3], tier: VoxelTier) -> bool {
        let half = match tier {
            VoxelTier::L1 => VOXEL_L1_SIZE_M * 0.5,
            VoxelTier::L2 => VOXEL_L2_SIZE_M * 0.5,
            VoxelTier::L3 => VOXEL_L3_SIZE_M * 0.5,
        };
        let pmin = self.aabb_min(self.feet_pos);
        let pmax = self.aabb_max(self.feet_pos);
        (center[0] + half > pmin[0] && center[0] - half < pmax[0])
            && (center[1] + half > pmin[1] && center[1] - half < pmax[1])
            && (center[2] + half > pmin[2] && center[2] - half < pmax[2])
    }

    fn sync_camera_transform(&mut self, world: &mut GameWorld<'_>) {
        let Some(cam) = self.camera_entity else {
            return;
        };
        let eye = self.eye_pos();
        let rot = self.camera_rotation();
        let mut lt = world.get::<LocalTransform>(cam).unwrap_or_default();
        lt.position = eye;
        lt.rotation = rot;
        world.set(cam, lt);
        self.last_synced_camera_rot = Some(rot.normalize());
    }

    fn update_hud_text(&self, world: &mut GameWorld<'_>, sim: &SimWorld) {
        let size_label = match self.tier {
            VoxelTier::L1 => "L1 (1.0m)",
            VoxelTier::L2 => "L2 (0.25m)",
            VoxelTier::L3 => "L3 (0.0625m)",
        };
        let tier_label = match self.tier_mode {
            TierMode::Auto => format!("AUTO->{size_label}"),
            TierMode::Fixed(_) => format!("LOCK {size_label}"),
        };
        let mode_label = if self.fly_mode { "FLY" } else { "WALK" };
        let tool_label = HOTBAR_TOOLS[self.tool_index].label();

        if let Some(ent) = self.hud_status {
            if let Some(mut text) = world.get::<Text>(ent) {
                text.content = format!(
                    "[{mode_label}] Tier(1/2/3/T): {tier_label} | Tool(Q/4-0): {tool_label} | Active: {}",
                    sim.active_count()
                );
                world.set(ent, text);
            }
        }

        if let Some(ent) = self.hud_probe {
            if let Some(mut text) = world.get::<Text>(ent) {
                if let Some(hit) = self.last_hit {
                    if let Some((mat, cell, l3_mask)) = sim.inspect_l2(hit.world_l2) {
                        let mat_name = &sim.materials.get(mat).name;
                        let tags = format_transient_tags(cell.transient);
                        let hit_tag = match hit.hit_tier {
                            VoxelTier::L1 => "L1 1m",
                            VoxelTier::L2 => "L2 0.25m",
                            VoxelTier::L3 => "L3 0.06m",
                        };
                        text.content = format!(
                            "Aim[{hit_tag}]: {mat_name} ({:?}/{:?}/{:?}) L3:{}/64 HP:{} [{tags}] | F:Heat E:Shock R:Wet G:Cryo Tab:Fly",
                            cell.env.matter(),
                            cell.env.temperature(),
                            cell.env.bio_activity(),
                            l3_mask.count_ones(),
                            cell.durability,
                        );
                    }
                } else {
                    text.content =
                        "LMB:Carve  RMB:Place/Stim  1/2/3:Tier(re-press=Auto)  F:Heat  E:Shock  R:Wet  G:Cryo  Tab:Fly"
                            .to_string();
                }
                world.set(ent, text);
            }
        }
    }
}

fn horizontal_wish_dir_for_rot(input: &InputSnapshot, cam_rot: Quat, fallback_yaw: f32) -> Vec3 {
    let norm = cam_rot.normalize();
    let cam_fwd = norm * -Vec3::Z;
    let cam_right = norm * Vec3::X;
    let mut forward = Vec3::new(cam_fwd.x, 0.0, cam_fwd.z);
    if forward.length_squared() > 1e-6 {
        forward = forward.normalize();
    } else {
        forward = Vec3::new(-fallback_yaw.sin(), 0.0, -fallback_yaw.cos());
    }
    let mut right = Vec3::new(cam_right.x, 0.0, cam_right.z);
    if right.length_squared() > 1e-6 {
        right = right.normalize();
    } else {
        right = Vec3::new(fallback_yaw.cos(), 0.0, -fallback_yaw.sin());
    }
    let mut wish = Vec3::ZERO;
    if input.key(keys::KEY_W) {
        wish += forward;
    }
    if input.key(keys::KEY_S) {
        wish -= forward;
    }
    if input.key(keys::KEY_D) {
        wish += right;
    }
    if input.key(keys::KEY_A) {
        wish -= right;
    }
    if wish.length_squared() > 1e-4 {
        wish.normalize()
    } else {
        Vec3::ZERO
    }
}

fn format_transient_tags(t: TransientFlags) -> String {
    let mut parts = Vec::new();
    if t.contains(TransientFlags::WET) {
        parts.push("WET");
    }
    if t.contains(TransientFlags::BURNING) {
        parts.push("BURNING");
    }
    if t.contains(TransientFlags::FROZEN) {
        parts.push("FROZEN");
    }
    if t.contains(TransientFlags::CHARGED) {
        parts.push("CHARGED");
    }
    if t.contains(TransientFlags::CORRODED) || t.contains(TransientFlags::CORROSIVE) {
        parts.push("CORRODED");
    }
    if t.contains(TransientFlags::INFECTED) {
        parts.push("INFECTED");
    }
    if t.contains(TransientFlags::BRITTLE) {
        parts.push("BRITTLE");
    }
    if t.contains(TransientFlags::WARNING) {
        parts.push("WARNING");
    }
    if parts.is_empty() {
        "NORMAL".to_string()
    } else {
        parts.join("+")
    }
}

fn hud_canvas() -> Canvas {
    Canvas {
        reference_resolution: [1280.0, 720.0],
        scale_mode: CanvasScaleMode::ScaleWithScreenSize,
        match_width_or_height: 0.5,
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_hud_text(
    world: &mut GameWorld<'_>,
    name: &str,
    content: &str,
    font_size: f32,
    left: f32,
    top: f32,
    width: f32,
    color: [f32; 4],
) -> EntityHandle {
    let entity = world.spawn();
    world.set(entity, Name(name.to_string()));
    world.set(entity, Node::default());
    world.set(entity, hud_canvas());
    world.set(
        entity,
        Style {
            width: Some(width),
            height: Some(font_size + 10.0),
            anchors: Anchors::TOP_LEFT,
            pivot: Pivot::TOP_LEFT,
            margin: Margin {
                left,
                top,
                ..Margin::default()
            },
            ..Style::default()
        },
    );
    world.set(
        entity,
        Text {
            content: content.to_string(),
            font_size,
            color,
            alignment: TextAlign::Left,
            ..Text::default()
        },
    );
    world.set(entity, Active(true));
    entity
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::materials::MaterialTable;
    use crate::world_gen::generate_world;

    #[test]
    fn swept_collision_lands_on_floor_and_blocks_wall() {
        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 2026);
        let sim = SimWorld::new(mats, gen.chunks, gen.cells, gen.active);
        let mut player = PlayerController::new();
        player.feet_pos = Vec3::new(17.5, 4.4, 14.0);
        player.fly_mode = false;
        player.grounded = false;

        // Simulate falling onto courtyard alloy apron (top at y = 4.0m) using standalone world.
        let mut ecs = prism_ecs::World::new();
        let mut world = GameWorld::host(&mut ecs);
        for _ in 0..30 {
            player.step_physics(&mut world, &sim, 1.0 / 60.0);
        }
        assert!(player.grounded, "player should land on y=4.0m floor");
        assert!((player.feet_pos.y - 4.0).abs() < 0.05);

        // Moving forward into solid outpost wall at x=13.0..13.25 must stop before intersecting wall.
        player.feet_pos = Vec3::new(12.2, 4.0, 17.5);
        player.yaw = -std::f32::consts::FRAC_PI_2; // facing +X
        let mut input = InputSnapshot::default();
        input.keys[keys::KEY_W / 64] |= 1u64 << (keys::KEY_W % 64);
        world.set_input(input);
        for _ in 0..30 {
            player.step_physics(&mut world, &sim, 1.0 / 60.0);
        }
        assert!(
            player.feet_pos.x <= 13.0 - PLAYER_RADIUS_M + 0.02,
            "player x={} penetrated solid outpost wall at x=13.0",
            player.feet_pos.x
        );
    }

    fn press_key(input: &mut InputSnapshot, code: usize) {
        input.keys[code / 64] |= 1u64 << (code % 64);
    }

    #[test]
    fn mouse_look_updates_movement_and_aiming_direction() {
        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 2026);
        let mut sim = SimWorld::new(mats, gen.chunks, gen.cells, gen.active);
        let mut ecs = prism_ecs::World::new();
        let mut world = GameWorld::host(&mut ecs);

        let cam = world.spawn();
        world.set(cam, Name("camera".to_string()));
        world.set(cam, Active(true));
        world.set(cam, prism_scene::Camera::default());
        world.set(
            cam,
            LocalTransform {
                position: Vec3::new(17.5, 5.5, 17.5),
                rotation: Quat::IDENTITY, // facing -Z
                scale: Vec3::ONE,
            },
        );

        let mut player = PlayerController::new();
        player.attach_ecs(&mut world, &sim);
        player.feet_pos = Vec3::new(17.5, 4.0, 17.5);
        player.grounded = true;
        player.sync_camera_transform(&mut world);
        assert!((player.forward_dir().z - (-1.0)).abs() < 1e-4);

        // Step W while facing -Z.
        let mut input = InputSnapshot::default();
        press_key(&mut input, keys::KEY_W);
        world.set_input(input);
        let before = player.feet_pos;
        player.step_physics(&mut world, &sim, 1.0 / 60.0);
        let delta = player.feet_pos - before;
        assert!(
            delta.z < -0.05 && delta.x.abs() < 1e-4,
            "expected -Z movement, got {delta:?}"
        );

        // Rotate right 90 degrees via unlocked RMB drag (e.g. Editor Game View) to face +X.
        let mut look_input = InputSnapshot {
            pointer_locked: false,
            right_held: true,
            mouse_delta: [std::f32::consts::FRAC_PI_2 / 0.0024, 0.0],
            ..InputSnapshot::default()
        };
        press_key(&mut look_input, keys::KEY_W);
        world.set_input(look_input);
        player.update_look_and_tools(&mut world, &mut sim, 1.0 / 60.0);

        let cam_rot = world.get::<LocalTransform>(cam).unwrap().rotation;
        let cam_fwd = (cam_rot * -Vec3::Z).normalize();
        assert!(
            cam_fwd.x > 0.99,
            "camera forward should turn toward +X, got {cam_fwd:?}"
        );
        assert!(
            player.forward_dir().x > 0.99,
            "aim forward should turn toward +X"
        );
        let hit = player
            .last_hit
            .expect("aim ray should hit east outpost wall when facing +X");
        assert!(
            hit.world_pos[0] > player.feet_pos.x,
            "hit should be in +X direction: {hit:?}"
        );

        // Stepping W must now move in +X matching rotated camera angle.
        let before2 = player.feet_pos;
        player.step_physics(&mut world, &sim, 1.0 / 60.0);
        let delta2 = player.feet_pos - before2;
        assert!(
            delta2.x > 0.05 && delta2.z.abs() < 1e-3,
            "expected +X movement after camera turn, got {delta2:?}"
        );
    }

    #[test]
    fn external_camera_and_fly_controller_update_player_heading() {
        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 2026);
        let mut sim = SimWorld::new(mats, gen.chunks, gen.cells, gen.active);
        let mut ecs = prism_ecs::World::new();
        let mut world = GameWorld::host(&mut ecs);

        let cam = world.spawn();
        world.set(cam, Name("camera".to_string()));
        world.set(cam, Active(true));
        world.set(cam, prism_scene::Camera::default());
        world.set(
            cam,
            LocalTransform {
                position: Vec3::new(17.5, 5.5, 17.5),
                rotation: Quat::from_euler(EulerRot::YXZ, std::f32::consts::FRAC_PI_2, -0.1, 0.0), // -X
                scale: Vec3::ONE,
            },
        );

        let mut player = PlayerController::new();
        player.attach_ecs(&mut world, &sim);
        player.feet_pos = Vec3::new(17.5, 4.0, 17.5);
        assert!(
            player.forward_dir().x < -0.95,
            "initial facing should inherit -X camera rotation"
        );

        // Externally rotate camera entity to look +Z (yaw = PI).
        let mut lt = world.get::<LocalTransform>(cam).unwrap();
        lt.rotation = Quat::from_euler(EulerRot::YXZ, std::f32::consts::PI, -0.05, 0.0);
        world.set(cam, lt);

        let mut input = InputSnapshot::default();
        press_key(&mut input, keys::KEY_W);
        world.set_input(input);
        player.update_look_and_tools(&mut world, &mut sim, 1.0 / 60.0);
        assert!(
            player.forward_dir().z > 0.95,
            "aim forward should sync to +Z camera rotation"
        );
        let before = player.feet_pos;
        player.step_physics(&mut world, &sim, 1.0 / 60.0);
        let delta = player.feet_pos - before;
        assert!(
            delta.z > 0.05 && delta.x.abs() < 1e-3,
            "expected +Z movement after external camera turn, got {delta:?}"
        );

        // Attach FlyCameraController facing +X (yaw = -PI/2) and verify physics & aiming follow it.
        world.set(
            cam,
            FlyCameraController {
                yaw: -std::f32::consts::FRAC_PI_2,
                pitch: 0.0,
                ..FlyCameraController::default()
            },
        );
        player.update_look_and_tools(&mut world, &mut sim, 1.0 / 60.0);
        assert!(
            player.forward_dir().x > 0.99,
            "aim forward should sync to FlyCameraController +X"
        );
        let before_fly = player.feet_pos;
        player.step_physics(&mut world, &sim, 1.0 / 60.0);
        let delta_fly = player.feet_pos - before_fly;
        assert!(
            delta_fly.x > 0.05 && delta_fly.z.abs() < 1e-3,
            "expected +X movement under FlyCameraController, got {delta_fly:?}"
        );
    }

    #[test]
    fn auto_tier_highlights_l1_l2_and_l3_and_allows_manual_override() {
        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 2026);
        let mut sim = SimWorld::new(mats, gen.chunks, gen.cells, gen.active);
        let mut ecs = prism_ecs::World::new();
        let mut world = GameWorld::host(&mut ecs);

        let mut player = PlayerController::new();
        player.attach_ecs(&mut world, &sim);
        // Stand at (8.5, 4.0, 8.5) aiming +X straight at visible L1 COW monolith [10, 4, 8] (`x in [10,11)`).
        player.feet_pos = Vec3::new(8.5, 4.0, 8.5);
        player.yaw = -std::f32::consts::FRAC_PI_2;
        player.pitch = -0.35;
        player.sync_camera_transform(&mut world);

        player.update_look_and_tools(&mut world, &mut sim, 1.0 / 60.0);
        let cur = world
            .get::<VoxelCursorHighlight>(player.cursor_entity.unwrap())
            .expect("cursor highlight");
        assert!(cur.active);
        assert!(
            (cur.max[0] - cur.min[0] - VOXEL_L1_SIZE_M).abs() < 1e-4,
            "Auto mode aiming at L1 COW block must draw 1.0m L1 wireframe, got {:?}",
            cur
        );
        assert_eq!(player.tier, VoxelTier::L1);

        // Press `2` to override cursor to L2 (`0.25m`) sub-cell granularity on the same L1 block.
        let mut key_input = InputSnapshot::default();
        let idx = keys::DIGIT_2;
        key_input.keys[idx / 64] |= 1u64 << (idx % 64);
        key_input.key_edges[idx / 64] |= 1u64 << (idx % 64);
        world.set_input(key_input);
        player.update_look_and_tools(&mut world, &mut sim, 1.0 / 60.0);
        let cur_l2 = world
            .get::<VoxelCursorHighlight>(player.cursor_entity.unwrap())
            .expect("cursor highlight");
        assert!(
            (cur_l2.max[0] - cur_l2.min[0] - VOXEL_L2_SIZE_M).abs() < 1e-4,
            "Fixed L2 mode must highlight 0.25m L2 sub-cell"
        );

        // Press `2` again to return to Auto and verify L3 window grate (`[59, 20, 60]`) draws 0.0625m L3 outline.
        let mut back_auto = InputSnapshot::default();
        back_auto.keys[idx / 64] |= 1u64 << (idx % 64);
        back_auto.key_edges[idx / 64] |= 1u64 << (idx % 64);
        world.set_input(back_auto);
        // Stand at (14.875, 4.0, 14.0) aiming +Z directly at L3 window mullion bar at `x=14.78, y=5.03, z=15.09`.
        player.feet_pos = Vec3::new(14.78, 3.6, 14.2);
        player.yaw = std::f32::consts::PI;
        player.pitch = 0.0;
        player.sync_camera_transform(&mut world);
        player.update_look_and_tools(&mut world, &mut sim, 1.0 / 60.0);
        assert_eq!(
            player.tier,
            VoxelTier::L3,
            "Auto mode aiming at L3 mullion must highlight L3 0.0625m"
        );
    }

    #[test]
    fn step_up_lifts_exact_height_and_smooths_camera_eye() {
        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 2026);
        let mut sim = SimWorld::new(mats, gen.chunks, gen.cells, gen.active);
        let mut ecs = prism_ecs::World::new();
        let mut world = GameWorld::host(&mut ecs);

        let mut player = PlayerController::new();
        player.attach_ecs(&mut world, &sim);
        // Build a wide 0.25m L2 step platform ahead (`x in 17.5..20.5, y in 4.0..4.25, z in 13.5..14.5`)
        // on the clean open apron (`y = 4.0`).
        for lz in 54..58 {
            for lx in 70..82 {
                sim.place_adjacent(
                    VoxelTier::L2,
                    &VoxelHit {
                        distance: 1.0,
                        world_pos: [lx as f32 * 0.25 + 0.125, 4.0, lz as f32 * 0.25 + 0.125],
                        normal: [0, 1, 0],
                        world_l1: [lx >> 2, 3, lz >> 2],
                        world_l2: [lx, 15, lz],
                        world_l3: [lx * 4, 63, lz * 4],
                        rgba: 0,
                        visual: 0,
                        hit_tier: VoxelTier::L2,
                    },
                    mat_id::ALLOY,
                );
            }
        }
        player.feet_pos = Vec3::new(17.10, 4.0, 14.0);
        player.yaw = -std::f32::consts::FRAC_PI_2; // +X
        player.pitch = 0.0;
        player.grounded = true;
        player.sync_camera_transform(&mut world);

        let mut input = InputSnapshot::default();
        press_key(&mut input, keys::KEY_W);
        world.set_input(input);

        let initial_eye_y = player.eye_pos().y;
        let mut max_single_frame_eye_jump = 0.0f32;
        let mut prev_eye_y = initial_eye_y;
        let mut prev_delta_y = 0.0f32;
        let mut sign_reversals = 0;

        for _ in 0..25 {
            player.step_physics(&mut world, &sim, 1.0 / 60.0);
            player.update_look_and_tools(&mut world, &mut sim, 1.0 / 60.0);
            let eye_y = player.eye_pos().y;
            let dy = eye_y - prev_eye_y;
            max_single_frame_eye_jump = max_single_frame_eye_jump.max(dy.abs());
            if prev_delta_y > 1e-3 && dy < -1e-3 || prev_delta_y < -1e-3 && dy > 1e-3 {
                sign_reversals += 1;
            }
            if dy.abs() > 1e-3 {
                prev_delta_y = dy;
            }
            prev_eye_y = eye_y;
        }

        assert!(
            player.feet_pos.y >= 4.24 && player.feet_pos.y <= 4.26,
            "feet should rest smoothly atop 0.25m step without overshoot, got {}",
            player.feet_pos.y
        );
        assert!(
            max_single_frame_eye_jump < 0.09,
            "camera eye jumped {}m in one frame on step-up",
            max_single_frame_eye_jump
        );
        assert_eq!(
            sign_reversals, 0,
            "camera step-up must be monotonic with no bounce jitter"
        );
    }
}
