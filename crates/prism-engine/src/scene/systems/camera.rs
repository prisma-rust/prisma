//! camera system
//!
//! Split the old `crate::camera::Camera` enum (which mixed editor fields and runtime state)
//! into pure data components (`Camera` + [`FlyCameraController`]) and
//! free functions that derive runtime view / projection matrices from them each frame.
//!
//! - [`camera_controller_system`] applies input to `FlyCameraController` +
//!   same-level `LocalTransform` (writes yaw/pitch/translation, and syncs orientation into
//!   `LocalTransform.rotation` so child nodes and `LocalToWorld` follow the view).
//! - [`compute_camera_output`] reads `Camera` + `FlyCameraController` +
//!   `LocalToWorld` and derives the matrices the renderer needs.
//!
//! Coordinate convention: right-handed, +Y up, camera looks down −Z.
//! Vulkan y-flip projection, depth range [0,1].
//! See `README.md` §Coordinate Conventions and `DESIGN.md`.

use glam::{Mat4, Vec3};

use prism_ecs::World;

use prism_scene::CameraClearFlags;

use crate::input::InputManager;
use crate::scene::components::{Camera, FlyCameraController, LocalToWorld, LocalTransform, Parent};

/// Return the first [`Camera`] component found in the world.
///
/// If there are multiple cameras (e.g. editor + game view), the ordering is
/// determined by the ECS storage (typically insertion order). Returns `None`
/// when no camera is present.
pub fn collect_camera(world: &World) -> Option<Camera> {
    world.query::<Camera>().next().map(|(_, c)| c.clone())
}

/// All **enabled** cameras, sorted ascending by `Camera.depth` (higher value drawn later) = `Camera.allCameras`.
pub fn all_cameras(world: &World) -> Vec<prism_ecs::Entity> {
    let mut cameras: Vec<(f32, prism_ecs::Entity)> = world
        .query::<Camera>()
        .filter(|(_, cam)| cam.enabled)
        .map(|(entity, cam)| (cam.depth, entity))
        .collect();
    cameras.sort_by(|a, b| a.0.total_cmp(&b.0));
    cameras.into_iter().map(|(_, entity)| entity).collect()
}

/// Main camera = enabled camera with max `depth` = `Camera.main`.
pub fn main_camera(world: &World) -> Option<prism_ecs::Entity> {
    all_cameras(world).pop()
}

/// Enabled cameras other than main (diagnostic use).
///
/// Since #461 all enabled cameras participate in composition: forward pass draws once per camera in
/// ascending `Camera.depth`, `clear_flags` decides per-layer color / depth clear. This function kept for
/// tools needing "non-main camera" list (stats, debug UI), no longer means "ignored".
pub fn ignored_cameras(world: &World) -> Vec<prism_ecs::Entity> {
    let mut cameras = all_cameras(world);
    cameras.pop();
    cameras
}

/// True renderable camera = among enabled cameras with max `depth`, first one **with `LocalToWorld`**.
///
/// Framing, clearing, aspect writeback, fly input all go through this one place: as long as everyone uses same
/// selection function, there will be no "wrote A's aspect but rendered B" / "input drives A but screen is B" fork.
pub fn active_camera(world: &World) -> Option<prism_ecs::Entity> {
    all_cameras(world)
        .into_iter()
        .rev()
        .find(|entity| world.get::<LocalToWorld>(*entity).is_some())
}

/// Clear color when no camera (neutral gray: distinguishable from pure black / white, helps spot "nothing drawn").
pub const DEFAULT_CLEAR_COLOR: [f32; 4] = [0.5, 0.5, 0.5, 1.0];

/// Clear color for specified camera = `Camera.backgroundColor`.
///
/// **Framing and clearing must use same source**: caller passes [`CameraOutput::entity`] (the camera that
/// actually produced the view); here no re-selecting camera, so two places cannot fork on differing rules.
/// When entity does not exist or has no `Camera`, use [`DEFAULT_CLEAR_COLOR`].
pub fn clear_color(world: &World, camera: prism_ecs::Entity) -> [f32; 4] {
    world
        .get::<Camera>(camera)
        .map(|cam| cam.background_color)
        .unwrap_or(DEFAULT_CLEAR_COLOR)
}

/// Clear mode for specified camera = `Camera.clearFlags`.
///
/// Same source as [`clear_color`]: caller passes [`CameraOutput::entity`], fallback to
/// [`CameraClearFlags::Skybox`] (default for new cameras, also existing engine behavior).
pub fn clear_flags(world: &World, camera: prism_ecs::Entity) -> CameraClearFlags {
    world
        .get::<Camera>(camera)
        .map(|cam| cam.clear_flags)
        .unwrap_or_default()
}

/// World resource that suppresses [`camera_controller_system`] input while a
/// game UI or command console owns keyboard/mouse focus.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CameraInputGate {
    pub suppressed: bool,
}

/// Runtime camera output produced each frame by [`compute_camera_output`].
pub struct CameraOutput {
    pub view_proj: Mat4,
    pub view: Mat4,
    pub projection: Mat4,
    pub eye: Vec3,
    pub exposure: f32,
    /// The entity the camera was sourced from (for downstream look-ups).
    pub entity: prism_ecs::Entity,
}

/// Return a fallback `CameraOutput` when no usable Camera entity exists in the
/// ECS world. This avoids a fatal error — the engine renders a gray background
/// and the runtime HUD shows a "No camera  hint.
///
/// The fallback places the viewer at `(0, 0, 5)` looking toward the origin with
/// a 75° FOV, 16:9 aspect, and exposure 1.0.
pub fn fallback_camera_output(surface_rotation: &Mat4, aspect: f32) -> CameraOutput {
    let eye = Vec3::new(0.0, 0.0, 5.0);
    let view = Mat4::from_cols_array_2d(&[
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, -5.0, 1.0],
    ]);
    let fov_y = 75.0_f32.to_radians();
    let inv_tan = 1.0 / (fov_y * 0.5).tan();
    let near = 0.01;
    let far = 500.0;
    let mut proj = Mat4::ZERO;
    proj.col_mut(0).x = inv_tan / aspect;
    proj.col_mut(1).y = -inv_tan;
    proj.col_mut(2).z = far / (near - far);
    proj.col_mut(2).w = -1.0;
    proj.col_mut(3).z = near * far / (near - far);

    let vp = *surface_rotation * proj * view;

    CameraOutput {
        view_proj: vp,
        view,
        projection: proj,
        eye,
        exposure: 1.0,
        entity: prism_ecs::Entity::from_raw(u32::MAX, 0),
    }
}

/// Find [`active_camera`] (renderable enabled camera with max `depth`), derive its view/projection.
///
/// Framing must use same source as clearing: caller takes [`CameraOutput::entity`] to
/// [`clear_color`], neither side selects a camera separately.
///
/// When no `FlyCameraController` fall back to entity's own orientation (inverse of `LocalToWorld`),
/// i.e. the "camera driven by Transform" shape — so a scene with just a
/// `Camera` + transform can produce an image, no need to force authors to add a fly controller.
///
/// `surface_rotation` is the device-orientation matrix applied on top of the
/// view-projection (mirrors the old `mat_mul(&surface_rotation, &vp)` step in
/// `render_system`). Returns `None` if no usable camera exists.
pub fn compute_camera_output(world: &World, surface_rotation: &Mat4) -> Option<CameraOutput> {
    compute_camera_output_for(world, active_camera(world)?, surface_rotation)
}

/// [`compute_camera_output`] for one explicit camera entity (#461).
///
/// Returns `None` when the entity lost its `Camera` / `LocalToWorld` since the
/// caller enumerated it.
pub fn compute_camera_output_for(
    world: &World,
    entity: prism_ecs::Entity,
    surface_rotation: &Mat4,
) -> Option<CameraOutput> {
    let cam = world.get::<Camera>(entity)?;
    let world_tf = world.get::<LocalToWorld>(entity)?;

    let col3 = world_tf.0.col(3);
    let eye = Vec3::new(col3.x, col3.y, col3.z);

    let view = match world.get::<FlyCameraController>(entity) {
        Some(ctrl) => fly_view(eye, ctrl.yaw, ctrl.pitch),
        None => world_tf.0.inverse(),
    };
    let proj = perspective(cam);
    let vp = *surface_rotation * proj * view;

    Some(CameraOutput {
        view_proj: vp,
        view,
        projection: proj,
        eye,
        exposure: cam.exposure,
        entity,
    })
}

/// Apply free-fly input for one frame to the first `FlyCameraController` +
/// sibling `LocalTransform` found on a `Camera` entity.
///
/// `look_active` is the pointer-lock state and it owns **all** camera input:
/// while it is false the camera neither rotates from the mouse delta (a held
/// right mouse button still does) nor moves from WASD/QE. The project UI owns
/// the keyboard in that state, so a shared key can never drive both at once.
///
/// Returns `true` if a controller was updated (so callers can skip the legacy
/// demo-spin animation for that entity).
pub fn camera_controller_system(
    world: &mut World,
    input: &InputManager,
    dt: f32,
    look_active: bool,
) -> Option<prism_ecs::Entity> {
    use crate::input::KeyCode;

    let cam_entity = active_camera(world)?;

    let ctrl = world.get_mut::<FlyCameraController>(cam_entity)?;
    let move_speed;
    {
        if look_active {
            let d = input.mouse_delta();
            ctrl.yaw -= d[0] as f32 * ctrl.look_sensitivity;
            ctrl.pitch -= d[1] as f32 * ctrl.look_sensitivity;
            let lim = std::f32::consts::FRAC_PI_2 - 0.02;
            ctrl.pitch = ctrl.pitch.clamp(-lim, lim);

            let scroll = input.scroll_delta() as f32;
            if scroll.abs() > 0.0 {
                ctrl.move_speed *= 1.0 - scroll * 0.1;
                ctrl.move_speed = ctrl.move_speed.clamp(0.5, 200.0);
            }
        }
        move_speed = ctrl.move_speed;
    }

    let boost = if input.key_held(KeyCode::ShiftLeft) || input.key_held(KeyCode::ShiftRight) {
        4.0
    } else {
        1.0
    };
    let speed = move_speed * boost * dt;

    let (yaw, pitch) = {
        let c = world.get::<FlyCameraController>(cam_entity)?;
        (c.yaw, c.pitch)
    };
    let f = forward(yaw, pitch);
    let r = right(yaw);
    let up = Vec3::Y;
    let mut movev = Vec3::ZERO;
    if look_active && input.key_held(KeyCode::KeyW) {
        movev += f;
    }
    if look_active && input.key_held(KeyCode::KeyS) {
        movev -= f;
    }
    if look_active && input.key_held(KeyCode::KeyD) {
        movev += r;
    }
    if look_active && input.key_held(KeyCode::KeyA) {
        movev -= r;
    }
    if look_active && (input.key_held(KeyCode::Space) || input.key_held(KeyCode::KeyE)) {
        movev += up;
    }
    if look_active && (input.key_held(KeyCode::ControlLeft) || input.key_held(KeyCode::KeyQ)) {
        movev -= up;
    }

    let world_rot = fly_rotation(yaw, pitch);
    let parent_rot = parent_world_rotation(world, cam_entity);
    let local_rot = match parent_rot {
        Some(p) => p.inverse() * world_rot,
        None => world_rot,
    };

    let ml = movev.length();
    let delta_world = if ml > 1e-6 {
        movev * (speed / ml)
    } else {
        Vec3::ZERO
    };
    let delta_local = match parent_rot {
        Some(p) => p.inverse() * delta_world,
        None => delta_world,
    };

    if let Some(t) = world.get_mut::<LocalTransform>(cam_entity) {
        t.rotation = local_rot;
        t.position += delta_local;
    }

    Some(cam_entity)
}

/// Unit forward vector from yaw/pitch.
///
/// Forward = `[-sin(yaw)·cos(pitch), sin(pitch), -cos(yaw)·cos(pitch)]`.
/// `yaw = 0` looks down **−Z** (consistent with identity transform in `.scene.json` and
/// `README.md` coordinate convention); increasing `yaw` turns left.
fn forward(yaw: f32, pitch: f32) -> Vec3 {
    let (s_y, c_y) = yaw.sin_cos();
    let (s_p, c_p) = pitch.sin_cos();
    Vec3::new(-s_y * c_p, s_p, -c_y * c_p)
}

/// Unit right vector, derived from yaw only (not forward × worldUp).
///
/// Right = `[cos(yaw), 0, -sin(yaw)]`; at `yaw = 0` it is **+X**. Constructed from yaw alone,
/// so any pitch is well-defined (including pitch = ±π/2, where forward × worldUp degenerates).
fn right(yaw: f32) -> Vec3 {
    let (s_y, c_y) = yaw.sin_cos();
    Vec3::new(c_y, 0.0, -s_y)
}

/// Camera world rotation corresponding to `yaw`/`pitch` (camera looks down its own −Z).
///
/// Uses same basis vectors as [`fly_view`] (columns = right / up / −forward), so after writing back
/// `LocalTransform.rotation`, camera child nodes and `LocalToWorld` get orientation
/// fully consistent with rendered image. `yaw = 0, pitch = 0` → identity quaternion.
fn fly_rotation(yaw: f32, pitch: f32) -> glam::Quat {
    let f = forward(yaw, pitch).normalize();
    let r = right(yaw);
    let u = r.cross(f);
    glam::Quat::from_mat3(&glam::Mat3::from_cols(r, u, -f)).normalize()
}

/// Parent's world rotation (`None` when no parent / parent hasn't computed `LocalToWorld` yet).
///
/// `hierarchy_system` also multiplies parent scale into `LocalToWorld`; here only rotation needed,
/// so take orthogonal part of T×R×S decomposition (and normalize, avoid non-uniform scale skewing quaternion).
fn parent_world_rotation(world: &World, entity: prism_ecs::Entity) -> Option<glam::Quat> {
    let parent = world.get::<Parent>(entity)?.0;
    let parent_world = world.get::<LocalToWorld>(parent)?.0;
    let (_, rotation, _) = parent_world.to_scale_rotation_translation();
    Some(rotation.normalize())
}

/// Column-major view matrix for a free-fly camera at `eye` with `yaw`/`pitch`.
fn fly_view(eye: Vec3, yaw: f32, pitch: f32) -> Mat4 {
    let f = forward(yaw, pitch).normalize();
    let r = right(yaw);
    let u = r.cross(f);
    Mat4::from_cols(
        glam::vec4(r.x, u.x, -f.x, 0.0),
        glam::vec4(r.y, u.y, -f.y, 0.0),
        glam::vec4(r.z, u.z, -f.z, 0.0),
        glam::vec4(-r.dot(eye), -u.dot(eye), f.dot(eye), 1.0),
    )
}

/// Column-major projection matrix (Vulkan y-flip, depth range [0,1]).
///
/// Perspective / orthographic choice in [`Camera::projection_matrix`] (`prism-scene`'s
/// `components_tests.rs` covers y-flip and `near → 0` / `far → 1` mapping).
fn perspective(cam: &Camera) -> Mat4 {
    cam.projection_matrix(cam.aspect)
}

#[cfg(test)]
#[path = "camera_tests.rs"]
mod tests;
