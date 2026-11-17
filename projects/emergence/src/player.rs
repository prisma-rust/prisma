//! First-person 3D player controller, swept L1/L2/L3 voxel collision, aiming tools, and HUD.

use glam::{EulerRot, Quat, Vec3};
use prism_game_api::{keys, EntityHandle, GameWorld, InputSnapshot};
use prism_scene::ui::{Anchors, Canvas, CanvasScaleMode, Node, Pivot, Style, Text, TextAlign};
use prism_scene::{
    Active, Camera, FlyCameraController, LocalTransform, Name, VoxelCursorHighlight, VoxelHit,
    VoxelTier, VOXEL_L1_SIZE_M, VOXEL_L2_SIZE_M, VOXEL_L3_SIZE_M,
};

use crate::catalog::CATALOG;
use crate::materials::mat_id;
use crate::sim::SimWorld;
use crate::tags::StimulusFlags;

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

pub const HOTBAR_LEN: usize = 10;

/// Map a 32-entry catalog slug onto the closest ROM visual material ID while preserving its `PhysicalCell` template.
pub fn catalog_mat_id(entry_id: &str) -> u8 {
    match entry_id {
        "conductive_wire" | "superconductor" | "copper_ore" | "gold_ore" => mat_id::WIRE,
        "piezo_crystal" => mat_id::CONDUIT,
        "water_cell" => mat_id::WATER,
        "glass_wall" | "diamond_ore" => mat_id::GLASS,
        "insulated_wall" | "iron_ore" => mat_id::ALLOY,
        "volatile_fuel" | "absorbent_sponge" => mat_id::TIMBER,
        "acid_pool" => mat_id::ACID,
        "cryo_spore" | "hyper_neo_core" => mat_id::BIOMASS,
        "valve_conduit" | "redstone_ore" => mat_id::VALVE,
        "conveyor_east" => mat_id::CONDUIT,
        "rad_isotope" | "plasma_arc" | "magma_lava" => mat_id::ISOTOPE,
        "stasis_emitter" | "ice_block" | "snow_block" => mat_id::CRYO,
        "bedrock" | "deepslate" => mat_id::BEDROCK,
        "stone" | "sandstone" | "coal_ore" => mat_id::LIMESTONE,
        "dirt" | "sand" => mat_id::SOIL,
        "grass_block" => mat_id::GRASS,
        _ => mat_id::LIMESTONE,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolAction {
    PlaceMaterial(u8),
    PlaceCatalog(usize),
    StimHeat,
    StimShock,
    StimWet,
    StimCold,
}

impl ToolAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::PlaceMaterial(id) => match id {
                mat_id::BEDROCK => "Place Bedrock",
                mat_id::SOIL => "Place Soil",
                mat_id::GRASS => "Place Grass",
                mat_id::ALLOY => "Place Alloy",
                mat_id::GLASS => "Place Glass",
                mat_id::TIMBER => "Place Timber",
                mat_id::WATER => "Pour Water",
                mat_id::ACID => "Pour Acid",
                mat_id::CRYO => "Pour Cryo",
                mat_id::BIOMASS => "Seed Biomass",
                mat_id::WIRE => "Lay Wire",
                mat_id::CONDUIT => "Place Conduit",
                mat_id::VALVE => "Place Valve",
                mat_id::ISOTOPE => "Place Isotope",
                mat_id::TANK => "Place Tank",
                _ => "Place Stone",
            },
            Self::PlaceCatalog(idx) => CATALOG.get(idx).map(|e| e.name).unwrap_or("Place Catalog"),
            Self::StimHeat => "Stim: Heat/Ignite",
            Self::StimShock => "Stim: Shock [E]",
            Self::StimWet => "Stim: Splash Wet",
            Self::StimCold => "Stim: Cryo Cold",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::PlaceMaterial(id) => match id {
                mat_id::BEDROCK => "Bedrock",
                mat_id::SOIL => "Soil",
                mat_id::GRASS => "Grass",
                mat_id::ALLOY => "Alloy",
                mat_id::GLASS => "Glass",
                mat_id::TIMBER => "Timber",
                mat_id::WATER => "Water",
                mat_id::ACID => "Acid",
                mat_id::CRYO => "Cryo",
                mat_id::BIOMASS => "Bio",
                mat_id::WIRE => "Wire",
                mat_id::CONDUIT => "Conduit",
                mat_id::VALVE => "Valve",
                mat_id::ISOTOPE => "Isotope",
                mat_id::TANK => "Tank",
                _ => "Stone",
            },
            Self::PlaceCatalog(idx) => CATALOG.get(idx).map(|e| e.name).unwrap_or("Catalog"),
            Self::StimHeat => "Heat",
            Self::StimShock => "Shock",
            Self::StimWet => "Wet",
            Self::StimCold => "Cold",
        }
    }

    pub fn swatch_color(self) -> [f32; 4] {
        match self {
            Self::PlaceMaterial(id) => match id {
                mat_id::BEDROCK => [0.22, 0.23, 0.27, 1.0],
                mat_id::SOIL => [0.46, 0.31, 0.20, 1.0],
                mat_id::GRASS => [0.28, 0.62, 0.25, 1.0],
                mat_id::ALLOY => [0.58, 0.63, 0.70, 1.0],
                mat_id::GLASS => [0.62, 0.86, 0.96, 1.0],
                mat_id::TIMBER => [0.62, 0.41, 0.24, 1.0],
                mat_id::WATER => [0.22, 0.56, 0.94, 1.0],
                mat_id::ACID => [0.45, 0.95, 0.22, 1.0],
                mat_id::CRYO => [0.46, 0.88, 0.98, 1.0],
                mat_id::BIOMASS => [0.62, 0.28, 0.84, 1.0],
                mat_id::WIRE => [0.85, 0.54, 0.24, 1.0],
                mat_id::CONDUIT => [0.48, 0.56, 0.64, 1.0],
                mat_id::VALVE => [0.85, 0.26, 0.24, 1.0],
                mat_id::ISOTOPE => [0.35, 0.98, 0.42, 1.0],
                mat_id::TANK => [0.34, 0.68, 0.72, 1.0],
                _ => [0.56, 0.57, 0.60, 1.0],
            },
            Self::PlaceCatalog(idx) => CATALOG
                .get(idx)
                .map(|e| e.color)
                .unwrap_or([0.75, 0.75, 0.75, 1.0]),
            Self::StimHeat => [0.98, 0.44, 0.14, 1.0],
            Self::StimShock => [0.35, 0.84, 1.0, 1.0],
            Self::StimWet => [0.24, 0.58, 0.96, 1.0],
            Self::StimCold => [0.58, 0.92, 1.0, 1.0],
        }
    }
}

pub const DEFAULT_HOTBAR_TOOLS: [ToolAction; HOTBAR_LEN] = [
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
    was_fly_mode: bool,
    pub tier: VoxelTier,
    pub tier_mode: TierMode,
    pub tool_index: usize,
    pub hotbar: [ToolAction; HOTBAR_LEN],
    coyote_timer: f32,
    jump_buffer: f32,
    camera_step_offset_y: f32,
    was_pointer_locked: bool,
    was_unlocked_right_held: bool,
    unlocked_right_drag_px: f32,
    last_synced_camera_rot: Option<Quat>,
    camera_entity: Option<EntityHandle>,
    cursor_entity: Option<EntityHandle>,
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
            was_fly_mode: false,
            tier: VoxelTier::L2,
            tier_mode: TierMode::Auto,
            tool_index: 0,
            hotbar: DEFAULT_HOTBAR_TOOLS,
            coyote_timer: 0.0,
            jump_buffer: 0.0,
            camera_step_offset_y: 0.0,
            was_pointer_locked: false,
            was_unlocked_right_held: false,
            unlocked_right_drag_px: 0.0,
            last_synced_camera_rot: None,
            camera_entity: None,
            cursor_entity: None,
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
        // Only the centered crosshair stays permanently visible; status/probe readouts live in F3.
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
        self.update_look_and_tools_with_ui(world, sim, dt, false);
    }

    /// Process frame-responsive mouse look and hotbar selection, gating click/drag actions when HUD UI consumes input.
    pub fn update_look_and_tools_with_ui(
        &mut self,
        world: &mut GameWorld<'_>,
        sim: &mut SimWorld,
        dt: f32,
        ui_consumed: bool,
    ) {
        let has_fly_controller = self.sync_from_camera_if_modified(world);
        let input = world.input();

        // Look via mouse delta while locked, plus drag-look and arrow-key fallback when unlocked.
        let unlocked_drag = !ui_consumed
            && !input.pointer_locked
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
        let unlocked_right_click = if input.pointer_locked || ui_consumed {
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
        let use_tool = !ui_consumed && ((input.pointer_locked && input.right_clicked) || unlocked_right_click);

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
                self.select_slot(slot_idx);
            }
        }
        if input.key_pressed(keys::KEY_Q) {
            self.tool_index = (self.tool_index + 1) % self.hotbar.len();
        }
        if !ui_consumed && input.scroll.abs() > 0.1 {
            let delta = if input.scroll > 0.0 { 1isize } else { -1isize };
            let len = self.hotbar.len() as isize;
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
        let tool = self.active_tool();
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
            if !ui_consumed && self.was_pointer_locked && input.left_clicked {
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
                    ToolAction::PlaceCatalog(idx) => {
                        if let Some(entry) = CATALOG.get(idx) {
                            let place_pos = self.placement_world_center(&hit);
                            if !self.overlaps_player_voxel(place_pos, self.tier) {
                                sim.place_adjacent_cell(
                                    self.tier,
                                    &hit,
                                    catalog_mat_id(entry.id),
                                    entry.template,
                                );
                            }
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
            self.was_fly_mode = true;
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

        // Depenetrate ONLY on the transition from Fly -> Walk so walking into walls
        // never triggers vertical climbing or floor jitter.
        if self.was_fly_mode {
            self.was_fly_mode = false;
            while sim
                .aabb_intersects_solid(self.aabb_min(self.feet_pos), self.aabb_max(self.feet_pos))
                && self.feet_pos.y < 62.0
            {
                self.feet_pos.y += 0.25;
                self.velocity.y = 0.0;
            }
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

        // Respawn above ground only if player fell below bedrock floor.
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

    pub(crate) fn sync_camera_transform(&mut self, world: &mut GameWorld<'_>) {
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

    /// Returns the most recent aimed voxel hit for F3 targeted-block inspection.
    pub fn last_hit(&self) -> Option<&VoxelHit> {
        self.last_hit.as_ref()
    }

    /// Returns the active tool action for the currently selected hotbar slot.
    pub fn active_tool(&self) -> ToolAction {
        self.hotbar[self.tool_index.min(HOTBAR_LEN - 1)]
    }

    /// Selects a hotbar slot (`0..HOTBAR_LEN`).
    pub fn select_slot(&mut self, slot: usize) {
        self.tool_index = slot.min(HOTBAR_LEN - 1);
    }

    /// Replaces the currently selected hotbar slot with `tool`.
    pub fn equip_active_slot(&mut self, tool: ToolAction) {
        let idx = self.tool_index.min(HOTBAR_LEN - 1);
        self.hotbar[idx] = tool;
    }

    /// Returns the active tool label for F3 status readout.
    pub fn active_tool_label(&self) -> &'static str {
        self.active_tool().label()
    }

    /// Returns a human-readable summary of the current voxel editing tier mode.
    pub fn tier_summary(&self) -> String {
        let size_label = match self.tier {
            VoxelTier::L1 => "L1 (1.0m)",
            VoxelTier::L2 => "L2 (0.25m)",
            VoxelTier::L3 => "L3 (0.0625m)",
        };
        match self.tier_mode {
            TierMode::Auto => format!("Auto -> {size_label}"),
            TierMode::Fixed(_) => format!("Fixed ({size_label})"),
        }
    }

    /// Returns the current locomotion mode label.
    pub fn motion_mode_label(&self) -> &'static str {
        if self.fly_mode {
            "Fly"
        } else {
            "Walk"
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

fn hud_canvas() -> Canvas {
    Canvas {
        reference_resolution: [1280.0, 720.0],
        scale_mode: CanvasScaleMode::ScaleWithScreenSize,
        match_width_or_height: 0.5,
    }
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
        player.feet_pos = Vec3::new(10.0, 4.4, 12.0);
        player.fly_mode = false;
        player.grounded = false;

        // Simulate falling onto the flat spawn courtyard (top at y = 4.0m) using a standalone world.
        let mut ecs = prism_ecs::World::new();
        let mut world = GameWorld::host(&mut ecs);
        for _ in 0..30 {
            player.step_physics(&mut world, &sim, 1.0 / 60.0);
        }
        assert!(player.grounded, "player should land on y=4.0m floor");
        assert!((player.feet_pos.y - 4.0).abs() < 0.05);

        // Walking into the hand-placed research outpost must stop at its west wall (x = 12.0m).
        player.feet_pos = Vec3::new(10.2, 4.0, 15.0);
        player.yaw = -std::f32::consts::FRAC_PI_2; // facing +X
        let mut input = InputSnapshot::default();
        input.keys[keys::KEY_W / 64] |= 1u64 << (keys::KEY_W % 64);
        world.set_input(input);
        for _ in 0..30 {
            player.step_physics(&mut world, &sim, 1.0 / 60.0);
        }
        assert!(
            player.feet_pos.x <= 12.0 - PLAYER_RADIUS_M + 0.02,
            "player x={} penetrated solid outpost wall at x=12.0",
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
                position: Vec3::new(9.5, 5.5, 15.0),
                rotation: Quat::IDENTITY, // facing -Z
                scale: Vec3::ONE,
            },
        );

        let mut player = PlayerController::new();
        player.attach_ecs(&mut world, &sim);
        player.feet_pos = Vec3::new(9.5, 4.0, 15.0);
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
            .expect("aim ray should hit the east outpost wall when facing +X");
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
                position: Vec3::new(9.5, 5.5, 11.5),
                rotation: Quat::from_euler(EulerRot::YXZ, std::f32::consts::FRAC_PI_2, -0.1, 0.0), // -X
                scale: Vec3::ONE,
            },
        );

        let mut player = PlayerController::new();
        player.attach_ecs(&mut world, &sim);
        player.feet_pos = Vec3::new(9.5, 4.0, 11.5);
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
        // Stand on the spawn courtyard aiming straight down at natural L1 COW terrain.
        player.feet_pos = Vec3::new(9.5, 4.0, 15.5);
        player.yaw = -std::f32::consts::FRAC_PI_2;
        player.pitch = -1.5;
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

        // Press `2` again to return to Auto and verify the outpost window mullion draws a 0.0625m L3 outline.
        let mut back_auto = InputSnapshot::default();
        back_auto.keys[idx / 64] |= 1u64 << (idx % 64);
        back_auto.key_edges[idx / 64] |= 1u64 << (idx % 64);
        world.set_input(back_auto);
        // Stand in front of the hand-placed outpost window grate (L2 `x = 55`, `y = 19..24`, `z = 52`).
        player.feet_pos = Vec3::new(13.79, 4.0, 12.0);
        player.yaw = std::f32::consts::PI;
        player.pitch = -0.5826;
        player.sync_camera_transform(&mut world);
        player.update_look_and_tools(&mut world, &mut sim, 1.0 / 60.0);
        assert_eq!(
            player.tier,
            VoxelTier::L3,
            "Auto mode aiming at the outpost L3 mullion must highlight L3 0.0625m"
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
        // Build a wide 0.25m L2 step platform ahead (`x in 9.5..12.5, y in 4.0..4.25, z in 10.5..11.5`)
        // on the clean open courtyard (`y = 4.0`).
        for lz in 42..46 {
            for lx in 38..50 {
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
        player.feet_pos = Vec3::new(9.10, 4.0, 11.0);
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

    #[test]
    fn fly_beyond_legacy_bounds_and_switch_back_to_walk_keeps_position() {
        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 2026);
        let mut sim = SimWorld::new(mats, gen.chunks, gen.cells, gen.active);
        let mut ecs = prism_ecs::World::new();
        let mut world = GameWorld::host(&mut ecs);

        let mut player = PlayerController::new();
        player.attach_ecs(&mut world, &sim);
        player.fly_mode = true;
        player.feet_pos = Vec3::new(90.0, 24.0, 90.0);
        sim.stream_around(player.feet_pos, Some(&mut world), 32);

        // Switch back to Walk mode and step physics: position must NOT be clamped to 31.6m.
        player.fly_mode = false;
        player.step_physics(&mut world, &sim, 1.0 / 60.0);
        assert!(
            player.feet_pos.x > 85.0 && player.feet_pos.z > 85.0,
            "switching from Fly to Walk must keep distant position, got {:?}",
            player.feet_pos
        );
    }
}
