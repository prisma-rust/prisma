//! Viewport math: world <-> screen projection, rays, AABB picking, gizmo axis hit.
//!
//! Shares the same coordinate convention with engine (right-handed, camera looks -Z, Vulkan depth [0,1]).
//! Picking prefers entity real local AABB (mesh AABB / sprite size); falls back when no data to
//! unit cube (`[-0.5,0.5]^3` × scale).

use glam::{Mat3, Mat4, Quat, Vec2, Vec3};

use crate::doc::{EntityJson, SceneDoc, TransformJson};
use crate::workspace::OrbitCamera;

/// `[x, y, z, w]` -> glam quaternion (public interface still uses array, caller unchanged).
fn quat(q: [f32; 4]) -> Quat {
    Quat::from_xyzw(q[0], q[1], q[2], q[3])
}

fn quat_array(q: Quat) -> [f32; 4] {
    [q.x, q.y, q.z, q.w]
}

pub const GIZMO_SCREEN_PIXELS: f32 = 90.0;
pub const GIZMO_PICK_RADIUS: f32 = 8.0;
/// Hit bandwidth (pixels) of rotation gizmo ring.
pub const GIZMO_RING_TOLERANCE: f32 = 10.0;

pub const IDENTITY_BASIS: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
pub const IDENTITY_ROTATION: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// Entity local-space AABB, used for accurate picking (#600).
///
/// Mesh from cooked mesh local AABB; sprite derived from `frame_size / pixels_per_unit`
/// and pivot; falls back to existing unit-cube proxy when no data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickBounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl PickBounds {
    pub fn center(&self) -> [f32; 3] {
        [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[1] + self.max[1]) * 0.5,
            (self.min[2] + self.max[2]) * 0.5,
        ]
    }

    pub fn half(&self) -> [f32; 3] {
        [
            (self.max[0] - self.min[0]).abs() * 0.5,
            (self.max[1] - self.min[1]).abs() * 0.5,
            (self.max[2] - self.min[2]).abs() * 0.5,
        ]
    }
}

/// Gizmo coordinate space: world axes or entity own axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GizmoSpace {
    #[default]
    World,
    Local,
}

impl GizmoSpace {
    pub fn label(self) -> &'static str {
        match self {
            GizmoSpace::World => "World",
            GizmoSpace::Local => "Local",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            GizmoSpace::World => GizmoSpace::Local,
            GizmoSpace::Local => GizmoSpace::World,
        }
    }
}

/// Screen radius of rotation gizmo ring (one per axis).
pub fn ring_screen_radius(
    frame: &CameraFrame,
    origin: [f32; 3],
    axis: usize,
    world_radius: f32,
    width: f32,
    height: f32,
) -> Option<(f32, f32, f32)> {
    ring_screen_radius_oriented(
        frame,
        origin,
        IDENTITY_BASIS,
        axis,
        world_radius,
        width,
        height,
    )
}

/// Ring radius in local coordinates: three columns of `basis` are entity local axes in world space direction.
pub fn ring_screen_radius_oriented(
    frame: &CameraFrame,
    origin: [f32; 3],
    basis: [[f32; 3]; 3],
    axis: usize,
    world_radius: f32,
    width: f32,
    height: f32,
) -> Option<(f32, f32, f32)> {
    let (u, v) = match axis {
        0 => (basis[1], basis[2]),
        1 => (basis[0], basis[2]),
        _ => (basis[0], basis[1]),
    };
    let start = project(origin, frame, width, height)?;
    let a = project(add(origin, scale(u, world_radius)), frame, width, height)?;
    let b = project(add(origin, scale(v, world_radius)), frame, width, height)?;
    let ra = ((a.0 - start.0).powi(2) + (a.1 - start.1).powi(2)).sqrt();
    let rb = ((b.0 - start.0).powi(2) + (b.1 - start.1).powi(2)).sqrt();
    Some((start.0, start.1, (ra + rb) * 0.5))
}

/// Hit rotation ring: return axis. Hit when screen distance minus projected radius smaller than bandwidth.
pub fn pick_rotation_axis(
    point: (f32, f32),
    frame: &CameraFrame,
    origin: [f32; 3],
    world_radius: f32,
    width: f32,
    height: f32,
) -> Option<usize> {
    pick_rotation_axis_oriented(
        point,
        frame,
        origin,
        IDENTITY_BASIS,
        world_radius,
        width,
        height,
    )
}

/// Rotation ring segment count for 3D projected ellipses and screen ring.
pub const GIZMO_RING_SEGMENTS: usize = 64;

/// Interactive transform gizmo handle under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GizmoHandle {
    Axis(usize),
    Plane(usize, usize),
    Center,
    ScreenRing,
}

/// Ring hit in local coordinates.
pub fn pick_rotation_axis_oriented(
    point: (f32, f32),
    frame: &CameraFrame,
    origin: [f32; 3],
    basis: [[f32; 3]; 3],
    world_radius: f32,
    width: f32,
    height: f32,
) -> Option<usize> {
    match pick_rotation_handle_oriented(
        point,
        frame,
        origin,
        basis,
        world_radius,
        frame.ortho_half_height.is_some(),
        width,
        height,
    ) {
        Some(GizmoHandle::Axis(axis)) => Some(axis),
        _ => None,
    }
}

/// Project a 3D rotation ring around `basis[axis]` as 2D screen-space polyline segments.
#[expect(
    clippy::too_many_arguments,
    reason = "Explicit camera frame, basis, and viewport extent avoid bundled state"
)]
pub fn rotation_ring_segments_oriented(
    frame: &CameraFrame,
    origin: [f32; 3],
    basis: [[f32; 3]; 3],
    axis: usize,
    world_radius: f32,
    width: f32,
    height: f32,
    segments: usize,
) -> Vec<((f32, f32), (f32, f32))> {
    let (u, v) = match axis {
        0 => (basis[1], basis[2]),
        1 => (basis[0], basis[2]),
        _ => (basis[0], basis[1]),
    };
    ring_segments_from_basis(frame, origin, u, v, world_radius, width, height, segments)
}

/// Project an outer view-aligned screen-space rotation ring (`1.18 * world_radius`).
pub fn screen_ring_segments(
    frame: &CameraFrame,
    origin: [f32; 3],
    world_radius: f32,
    width: f32,
    height: f32,
    segments: usize,
) -> Vec<((f32, f32), (f32, f32))> {
    ring_segments_from_basis(
        frame,
        origin,
        frame.right,
        frame.up,
        world_radius * 1.18,
        width,
        height,
        segments,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "Shared ring tessellation helper across axis and screen-space rings"
)]
fn ring_segments_from_basis(
    frame: &CameraFrame,
    origin: [f32; 3],
    u: [f32; 3],
    v: [f32; 3],
    radius: f32,
    width: f32,
    height: f32,
    segments: usize,
) -> Vec<((f32, f32), (f32, f32))> {
    let segs = segments.max(8);
    let mut out = Vec::with_capacity(segs);
    for i in 0..segs {
        let a0 = (i as f32) * std::f32::consts::TAU / (segs as f32);
        let a1 = ((i + 1) as f32) * std::f32::consts::TAU / (segs as f32);
        let (s0, c0) = a0.sin_cos();
        let (s1, c1) = a1.sin_cos();
        let p0 = [
            origin[0] + (u[0] * c0 + v[0] * s0) * radius,
            origin[1] + (u[1] * c0 + v[1] * s0) * radius,
            origin[2] + (u[2] * c0 + v[2] * s0) * radius,
        ];
        let p1 = [
            origin[0] + (u[0] * c1 + v[0] * s1) * radius,
            origin[1] + (u[1] * c1 + v[1] * s1) * radius,
            origin[2] + (u[2] * c1 + v[2] * s1) * radius,
        ];
        if let Some(seg) = project_clipped_segment(p0, p1, frame, width, height) {
            out.push(seg);
        }
    }
    out
}

/// Pick the closest projected 3D rotation ring or outer screen-space ring under `point`.
#[expect(
    clippy::too_many_arguments,
    reason = "Explicit camera frame, basis, and viewport extent avoid bundled state"
)]
pub fn pick_rotation_handle_oriented(
    point: (f32, f32),
    frame: &CameraFrame,
    origin: [f32; 3],
    basis: [[f32; 3]; 3],
    world_radius: f32,
    ortho_2d: bool,
    width: f32,
    height: f32,
) -> Option<GizmoHandle> {
    let mut best: Option<(GizmoHandle, f32)> = None;
    let axes: &[usize] = if ortho_2d { &[2] } else { &[0, 1, 2] };
    for &axis in axes {
        let segs = rotation_ring_segments_oriented(
            frame,
            origin,
            basis,
            axis,
            world_radius,
            width,
            height,
            GIZMO_RING_SEGMENTS,
        );
        let min_dist = segs
            .iter()
            .map(|&(a, b)| distance_point_segment(point, a, b))
            .fold(f32::INFINITY, f32::min);
        if min_dist <= GIZMO_RING_TOLERANCE && best.map(|(_, d)| min_dist < d).unwrap_or(true) {
            best = Some((GizmoHandle::Axis(axis), min_dist));
        }
    }
    if best.is_none() {
        let outer = screen_ring_segments(
            frame,
            origin,
            world_radius,
            width,
            height,
            GIZMO_RING_SEGMENTS,
        );
        let min_dist = outer
            .iter()
            .map(|&(a, b)| distance_point_segment(point, a, b))
            .fold(f32::INFINITY, f32::min);
        if min_dist <= GIZMO_RING_TOLERANCE {
            best = Some((GizmoHandle::ScreenRing, min_dist));
        }
    }
    best.map(|(handle, _)| handle)
}

/// Angle of screen point relative to center (used for rotation drag).
pub fn screen_angle(point: (f32, f32), center: (f32, f32)) -> f32 {
    (point.1 - center.1).atan2(point.0 - center.0)
}

/// Normalize angle difference into `(-π, π]`.
pub fn wrap_angle(angle: f32) -> f32 {
    let two_pi = std::f32::consts::TAU;
    let mut wrapped = angle % two_pi;
    if wrapped > std::f32::consts::PI {
        wrapped -= two_pi;
    } else if wrapped <= -std::f32::consts::PI {
        wrapped += two_pi;
    }
    wrapped
}

/// Quaternion multiplication (`a * b`, same order as glam `Quat::mul_quat`).
pub fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    quat_array(quat(a) * quat(b))
}

/// Quaternion rotating `angle` radians around world axis.
pub fn quat_axis_angle(axis: usize, angle: f32) -> [f32; 4] {
    let axis = match axis {
        0 => Vec3::X,
        1 => Vec3::Y,
        _ => Vec3::Z,
    };
    quat_array(Quat::from_axis_angle(axis, angle))
}

/// Rotate vector by quaternion (test / rotated axis endpoint use).
pub fn quat_rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    (quat(q) * Vec3::from(v)).to_array()
}

pub fn quat_normalize(q: [f32; 4]) -> [f32; 4] {
    quat_array(quat(q) / quat(q).length().max(1e-6))
}

/// Conjugate of unit quaternion (= inverse).
pub fn quat_conjugate(q: [f32; 4]) -> [f32; 4] {
    quat_array(quat(q).conjugate())
}

/// Quaternion -> XYZ Euler angles (degrees).
///
/// Convention `R = Rz * Ry * Rx` (glam `EulerRot::XYZ`), inspector uses it to show
/// `rotation` as readable degrees; `euler_degrees_to_quat` is its inverse.
pub fn quat_to_euler_degrees(q: [f32; 4]) -> [f32; 3] {
    let q = quat(q);
    let length = q.length();
    if length < 1e-8 {
        return [0.0; 3];
    }
    let m = Mat3::from_quat(q / length);
    let (m00, m10, m20) = (m.x_axis.x, m.x_axis.y, m.x_axis.z);
    let (m21, m22) = (m.y_axis.z, m.z_axis.z);
    let (m11, m12) = (m.y_axis.y, m.z_axis.y);

    let sin_y = (-m20).clamp(-1.0, 1.0);
    let ey = sin_y.asin();
    let (ex, ez) = if sin_y.abs() < 0.999_99 {
        (m21.atan2(m22), m10.atan2(m00))
    } else {
        ((-m12).atan2(m11), 0.0)
    };
    [ex.to_degrees(), ey.to_degrees(), ez.to_degrees()]
}

/// XYZ Euler angles (degrees) -> Quaternion, inverse of [`quat_to_euler_degrees`].
pub fn euler_degrees_to_quat(euler: [f32; 3]) -> [f32; 4] {
    quat_array(
        Quat::from_rotation_z(euler[2].to_radians())
            * Quat::from_rotation_y(euler[1].to_radians())
            * Quat::from_rotation_x(euler[0].to_radians()),
    )
}

/// Local three-axis directions in world space (column vectors).
pub fn basis_from_quat(q: [f32; 4]) -> [[f32; 3]; 3] {
    let q = quat(q);
    [
        (q * Vec3::X).to_array(),
        (q * Vec3::Y).to_array(),
        (q * Vec3::Z).to_array(),
    ]
}

/// Entity world rotation accumulated along parent chain (including self; parent first, truncated on cycle).
pub fn world_rotation(doc: &SceneDoc, index: usize) -> [f32; 4] {
    let mut chain = Vec::new();
    let mut current = Some(index);
    while let Some(current_index) = current {
        if chain.len() >= 64 || chain.contains(&current_index) {
            break;
        }
        chain.push(current_index);
        current = doc.entities[current_index]
            .parent
            .and_then(|parent_id| doc.index_of_id(parent_id));
    }
    let mut rotation = IDENTITY_ROTATION;
    for entity_index in chain.iter().rev() {
        rotation = quat_mul(rotation, doc.entities[*entity_index].transform.rotation);
    }
    quat_normalize(rotation)
}

/// Parent world rotation (excluding self; root entity parent is identity rotation).
pub fn parent_world_rotation(doc: &SceneDoc, index: usize) -> [f32; 4] {
    doc.entities[index]
        .parent
        .and_then(|parent_id| doc.index_of_id(parent_id))
        .map(|parent_index| world_rotation(doc, parent_index))
        .unwrap_or(IDENTITY_ROTATION)
}

fn walk_world_matrix(doc: &SceneDoc, index: usize, depth: usize, clamp_zero_scale: bool) -> Mat4 {
    let Some(entity) = doc.entities.get(index) else {
        return Mat4::IDENTITY;
    };
    let mut s = entity.transform.scale;
    if clamp_zero_scale {
        for axis in &mut s {
            if axis.abs() < 1e-6 {
                *axis = if *axis < 0.0 { -1e-6 } else { 1e-6 };
            }
        }
    }
    let q = quat_normalize(entity.transform.rotation);
    let local = Mat4::from_scale_rotation_translation(
        Vec3::from(s),
        glam::Quat::from_xyzw(q[0], q[1], q[2], q[3]),
        Vec3::from(entity.transform.position),
    );
    if depth > 64 {
        return local;
    }
    let Some(parent_index) = entity.parent.and_then(|parent| doc.index_of_id(parent)) else {
        return local;
    };
    walk_world_matrix(doc, parent_index, depth + 1, clamp_zero_scale) * local
}

/// Compute the entity world transform matrix along the full parent chain (`Parent * T * R * S`).
pub fn world_matrix(doc: &SceneDoc, index: usize) -> Mat4 {
    walk_world_matrix(doc, index, 0, false)
}

/// Compute an invertible parent world matrix, preserving negative scale signs while clamping zero scales.
pub fn world_matrix_invertible(doc: &SceneDoc, index: usize) -> Mat4 {
    walk_world_matrix(doc, index, 0, true)
}

/// Entity world position accumulated along the full ancestor matrix chain.
pub fn world_position(doc: &SceneDoc, index: usize) -> [f32; 3] {
    world_matrix(doc, index).w_axis.truncate().to_array()
}

/// Entity world scale (multiply components along parent chain).
pub fn world_scale(doc: &SceneDoc, index: usize) -> [f32; 3] {
    let mut scale = doc
        .entities
        .get(index)
        .map(|entity| entity.transform.scale)
        .unwrap_or([1.0; 3]);
    let mut current = doc.entities.get(index).and_then(|entity| entity.parent);
    let mut depth = 0;
    while let Some(id) = current {
        if depth > 64 {
            break;
        }
        let Some(parent_index) = doc.index_of_id(id) else {
            break;
        };
        let parent_scale = doc.entities[parent_index].transform.scale;
        for axis in 0..3 {
            scale[axis] *= parent_scale[axis];
        }
        current = doc.entities[parent_index].parent;
        depth += 1;
    }
    scale
}

/// **Bounding sphere** `(center, radius)` of a set of entities in world space (#505).
///
/// Editor document has no mesh data, so each entity uses "unit cube × world scale" as proxy
/// (consistent with selection outline / gizmo proxy): 8 corners transformed to world space by world rotation,
/// center and half-diagonal of union AABB as radius. Parent chain translation / rotation / scale all via
/// [`world_position`] / [`world_rotation`] / [`world_scale`] accumulated, so child bounding box is its position in **world**
/// (not local `transform.position`).
///
/// Out-of-bounds index, non-finite transform, empty set all skipped; returns `None` when all empty. Radius floor `0.25`,
/// so zero-scale / single-point entity framing distance does not collapse to camera minimum distance.
pub fn world_bounds(doc: &SceneDoc, indices: &[usize]) -> Option<([f32; 3], f32)> {
    world_bounds_with(doc, indices, |index| authored_local_bounds(doc, index))
}

/// Local AABB derivable from **authoring document** (sprite / collider).
///
/// Sprite only computable when `frame_size` written (`[0, 0]` = whole texture, size only engine
/// knows); collider shape is authoring data, always available. If cannot derive returns `None`, caller falls back to
/// unit cube.
pub fn authored_local_bounds(doc: &SceneDoc, index: usize) -> Option<PickBounds> {
    let entity = doc.entities.get(index)?;
    collider_local_bounds(entity).or_else(|| sprite_local_bounds(entity))
}

fn number(value: Option<&serde_json::Value>, default: f32) -> f32 {
    value
        .and_then(serde_json::Value::as_f64)
        .map(|value| value as f32)
        .unwrap_or(default)
}

fn collider_local_bounds(entity: &EntityJson) -> Option<PickBounds> {
    let collider = entity.component("prism_engine::scene::Collider")?;
    let center = [0usize, 1, 2].map(|axis| {
        number(
            collider.get("center").and_then(|center| center.get(axis)),
            0.0,
        )
    });
    let shape = collider
        .get("shape")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Box");
    let (half_x, half_y, half_z) = match shape {
        "Sphere" => {
            let radius = number(collider.get("radius"), 0.5).max(1.0e-4);
            (radius, radius, radius)
        }
        "Capsule" => {
            let radius = number(collider.get("radius"), 0.5).max(1.0e-4);
            let half_height = number(collider.get("half_height"), 0.5).max(0.0);
            (radius, half_height + radius, radius)
        }
        _ => {
            let half = collider.get("half_extents");
            (
                number(half.and_then(|value| value.get(0)), 0.5).max(1.0e-4),
                number(half.and_then(|value| value.get(1)), 0.5).max(1.0e-4),
                number(half.and_then(|value| value.get(2)), 0.5).max(1.0e-4),
            )
        }
    };
    Some(PickBounds {
        min: [center[0] - half_x, center[1] - half_y, center[2] - half_z],
        max: [center[0] + half_x, center[1] + half_y, center[2] + half_z],
    })
}

fn sprite_local_bounds(entity: &EntityJson) -> Option<PickBounds> {
    let sprite = entity.component("prism_engine::scene::SpriteRenderer")?;
    let size = sprite.get("frame_size");
    let frame_w = number(size.and_then(|value| value.get(0)), 0.0);
    let frame_h = number(size.and_then(|value| value.get(1)), 0.0);
    if frame_w <= 0.0 || frame_h <= 0.0 {
        return None;
    }
    let ppu = number(sprite.get("pixels_per_unit"), 100.0);
    let ppu = if ppu > 0.0 { ppu } else { 100.0 };
    let pivot = sprite.get("pivot");
    let pivot_x = number(pivot.and_then(|value| value.get(0)), 0.5);
    let pivot_y = number(pivot.and_then(|value| value.get(1)), 0.5);
    let width = frame_w / ppu;
    let height = frame_h / ppu;
    Some(PickBounds {
        min: [-pivot_x * width, -pivot_y * height, -0.05],
        max: [(1.0 - pivot_x) * width, (1.0 - pivot_y) * height, 0.05],
    })
}

/// Inject-able version of [`world_bounds`]: `bounds_for` returns entity **local** AABB
/// (`None` = fall back to unit cube).
///
/// In editor process engine knows texture size (computable even when `frame_size` not written), data layer only reads document
/// does not know, so caller injects "bounds with texture size" when framing.
pub fn world_bounds_with(
    doc: &SceneDoc,
    indices: &[usize],
    bounds_for: impl Fn(usize) -> Option<PickBounds>,
) -> Option<([f32; 3], f32)> {
    let mut points: Vec<[f32; 3]> = Vec::new();
    for index in indices {
        if *index >= doc.entities.len() {
            continue;
        }
        let center = world_position(doc, *index);
        let scale = world_scale(doc, *index);
        let rotation = world_rotation(doc, *index);
        if !center.iter().all(|value| value.is_finite())
            || !scale.iter().all(|value| value.is_finite())
            || !rotation.iter().all(|value| value.is_finite())
        {
            continue;
        }
        let local = bounds_for(*index);
        let half = match &local {
            Some(bounds) => {
                let half = bounds.half();
                [
                    half[0] * scale[0].abs(),
                    half[1] * scale[1].abs(),
                    half[2] * scale[2].abs(),
                ]
            }
            None => [
                scale[0].abs() * 0.5,
                scale[1].abs() * 0.5,
                scale[2].abs() * 0.5,
            ],
        };
        let offset = local
            .as_ref()
            .map(|bounds| {
                let center = bounds.center();
                quat_rotate(
                    rotation,
                    [
                        center[0] * scale[0],
                        center[1] * scale[1],
                        center[2] * scale[2],
                    ],
                )
            })
            .unwrap_or([0.0; 3]);
        let origin = add(center, offset);
        for sx in [-1.0f32, 1.0] {
            for sy in [-1.0f32, 1.0] {
                for sz in [-1.0f32, 1.0] {
                    let corner = quat_rotate(rotation, [half[0] * sx, half[1] * sy, half[2] * sz]);
                    points.push(add(origin, corner));
                }
            }
        }
    }
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for point in &points {
        for axis in 0..3 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }
    if points.is_empty() {
        return None;
    }
    let center = [
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ];
    let radius = points
        .iter()
        .map(|point| {
            ((point[0] - center[0]).powi(2)
                + (point[1] - center[1]).powi(2)
                + (point[2] - center[2]).powi(2))
            .sqrt()
        })
        .fold(0.0f32, f32::max);
    let radius = if radius.is_finite() {
        radius.max(0.25)
    } else {
        0.25
    };
    Some((center, radius))
}

/// Screen direction of world axes in current camera (x right, y down).
pub fn orientation_axis_direction(frame: &CameraFrame, axis: usize, sign: f32) -> [f32; 2] {
    let world = match axis {
        0 => [sign, 0.0, 0.0],
        1 => [0.0, sign, 0.0],
        _ => [0.0, 0.0, sign],
    };
    [dot(world, frame.right), -dot(world, frame.up)]
}

/// Endpoint of a direction gizmo axis (screen coordinates).
pub fn orientation_axis_end(
    frame: &CameraFrame,
    axis: usize,
    sign: f32,
    center: (f32, f32),
    radius: f32,
) -> (f32, f32) {
    let direction = orientation_axis_direction(frame, axis, sign);
    (
        center.0 + direction[0] * radius,
        center.1 + direction[1] * radius,
    )
}

/// Which direction gizmo axis is clicked (returns `(axis, sign)`), tolerance 9px.
pub fn pick_orientation_axis(
    position: (f32, f32),
    center: (f32, f32),
    radius: f32,
    frame: &CameraFrame,
) -> Option<(usize, f32)> {
    let mut best: Option<(usize, f32, f32)> = None;
    for axis in 0..3 {
        for sign in [1.0_f32, -1.0] {
            let direction = orientation_axis_direction(frame, axis, sign);
            if (direction[0] * direction[0] + direction[1] * direction[1]).sqrt() < 0.2 {
                continue;
            }
            let end = orientation_axis_end(frame, axis, sign, center, radius);
            let dx = end.0 - position.0;
            let dy = end.1 - position.1;
            let distance = (dx * dx + dy * dy).sqrt();
            if distance <= 9.0 && best.is_none_or(|(_, _, best)| distance < best) {
                best = Some((axis, sign, distance));
            }
        }
    }
    best.map(|(axis, sign, _)| (axis, sign))
}

#[derive(Debug, Clone, Copy)]
pub struct CameraFrame {
    pub eye: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub forward: [f32; 3],
    /// Orthographic half-height (world units); `None` = perspective.
    ///
    /// [#641] Scene view 2D mode is orthographic camera, projection / picking rays need formula change,
    /// otherwise screen position does not match engine drawing.
    pub ortho_half_height: Option<f32>,
    /// Vertical field-of-view (degrees); projection / picking rays derive half-angle from it (#416).
    pub fov_y_degrees: f32,
}

pub fn camera_frame(camera: &OrbitCamera) -> CameraFrame {
    let eye = Vec3::from(camera.eye());
    let rotation = Quat::from_rotation_y(camera.yaw) * Quat::from_rotation_x(-camera.pitch);
    let right = (rotation * Vec3::X).normalize();
    let up = (rotation * Vec3::Y).normalize();
    let forward = (rotation * Vec3::NEG_Z).normalize();
    CameraFrame {
        eye: eye.to_array(),
        right: right.to_array(),
        up: up.to_array(),
        forward: forward.to_array(),
        ortho_half_height: camera.ortho_half_height(),
        fov_y_degrees: if camera.fov_y_degrees.is_finite() {
            camera.fov_y_degrees.clamp(10.0, 170.0)
        } else {
            60.0
        },
    }
}

/// Perspective camera half-angle tangent; `CameraFrame::fov_y_degrees` already clamped at construction.
fn half_fov_tan(frame: &CameraFrame) -> f32 {
    (frame.fov_y_degrees * 0.5).to_radians().tan().max(1e-4)
}

pub fn normalize(v: [f32; 3]) -> [f32; 3] {
    (Vec3::from(v) / Vec3::from(v).length().max(1e-6)).to_array()
}

pub fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    Vec3::from(a).cross(Vec3::from(b)).to_array()
}

pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    Vec3::from(a).dot(Vec3::from(b))
}

pub fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    (Vec3::from(a) - Vec3::from(b)).to_array()
}

pub fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    (Vec3::from(a) + Vec3::from(b)).to_array()
}

pub fn scale(a: [f32; 3], factor: f32) -> [f32; 3] {
    (Vec3::from(a) * factor).to_array()
}

/// World coordinates -> screen coordinates (pixels relative to viewport top-left), returns `(x, y, depth)`.
pub fn project(
    point: [f32; 3],
    frame: &CameraFrame,
    width: f32,
    height: f32,
) -> Option<(f32, f32, f32)> {
    let origin = Vec3::from(frame.eye);
    let delta = Vec3::from(point) - origin;
    let depth = delta.dot(Vec3::from(frame.forward));
    if depth <= 0.05 {
        return None;
    }
    let aspect = (width / height.max(1.0)).max(0.2);
    let (ndc_x, ndc_y) = match frame.ortho_half_height {
        Some(half_h) => {
            let half_h = half_h.max(1e-4);
            (
                delta.dot(Vec3::from(frame.right)) / (half_h * aspect),
                delta.dot(Vec3::from(frame.up)) / half_h,
            )
        }
        None => {
            let tan_half = half_fov_tan(frame);
            (
                delta.dot(Vec3::from(frame.right)) / depth / tan_half / aspect,
                delta.dot(Vec3::from(frame.up)) / depth / tan_half,
            )
        }
    };
    Some((
        (ndc_x * 0.5 + 0.5) * width,
        (1.0 - (ndc_y * 0.5 + 0.5)) * height,
        depth,
    ))
}

/// Screen pixels -> world ray `(origin, direction)`.
pub fn ray_from_screen(
    x: f32,
    y: f32,
    frame: &CameraFrame,
    width: f32,
    height: f32,
) -> ([f32; 3], [f32; 3]) {
    let aspect = (width / height.max(1.0)).max(0.2);
    let (ndc_x, ndc_y) = (
        2.0 * x / width.max(1.0) - 1.0,
        1.0 - 2.0 * y / height.max(1.0),
    );
    let eye = Vec3::from(frame.eye);
    let right = Vec3::from(frame.right);
    let up = Vec3::from(frame.up);
    let forward = Vec3::from(frame.forward);
    if let Some(half_h) = frame.ortho_half_height {
        let half_h = half_h.max(1e-4);
        let offset = right * (ndc_x * half_h * aspect) + up * (ndc_y * half_h);
        return ((eye + offset).to_array(), forward.to_array());
    }
    let tan_half = half_fov_tan(frame);
    let ndc_x = ndc_x * aspect * tan_half;
    let ndc_y = ndc_y * tan_half;
    let direction = (forward + right * ndc_x + up * ndc_y).normalize();
    (eye.to_array(), direction.to_array())
}

/// Ray-axis-aligned-box intersection, returns nearest hit distance.
pub fn ray_box(
    origin: [f32; 3],
    direction: [f32; 3],
    center: [f32; 3],
    half: [f32; 3],
) -> Option<f32> {
    let mut t_min = f32::NEG_INFINITY;
    let mut t_max = f32::INFINITY;
    for axis in 0..3 {
        let origin_axis = origin[axis];
        let dir = direction[axis];
        let min = center[axis] - half[axis];
        let max = center[axis] + half[axis];
        if dir.abs() < 1e-6 {
            if origin_axis < min || origin_axis > max {
                return None;
            }
            continue;
        }
        let inv = 1.0 / dir;
        let mut t1 = (min - origin_axis) * inv;
        let mut t2 = (max - origin_axis) * inv;
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        t_min = t_min.max(t1);
        t_max = t_max.min(t2);
        if t_min > t_max {
            return None;
        }
    }
    if t_max < 0.0 {
        None
    } else {
        Some(t_min.max(0.0))
    }
}

/// Screen-space point to segment distance.
pub fn distance_point_segment(point: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let ab = (b.0 - a.0, b.1 - a.1);
    let ap = (point.0 - a.0, point.1 - a.1);
    let length_sq = ab.0 * ab.0 + ab.1 * ab.1;
    let t = if length_sq <= 1e-6 {
        0.0
    } else {
        ((ap.0 * ab.0 + ap.1 * ab.1) / length_sq).clamp(0.0, 1.0)
    };
    let projection = (a.0 + ab.0 * t, a.1 + ab.1 * t);
    ((point.0 - projection.0).powi(2) + (point.1 - projection.1).powi(2)).sqrt()
}

/// Clip a world-space segment against the camera near plane in perspective mode.
pub fn clip_segment_to_near_plane(
    a: [f32; 3],
    b: [f32; 3],
    frame: &CameraFrame,
    near: f32,
) -> Option<([f32; 3], [f32; 3])> {
    if frame.ortho_half_height.is_some() {
        return Some((a, b));
    }
    let eye = Vec3::from(frame.eye);
    let forward = Vec3::from(frame.forward);
    let da = (Vec3::from(a) - eye).dot(forward) - near;
    let db = (Vec3::from(b) - eye).dot(forward) - near;
    if da < 0.0 && db < 0.0 {
        return None;
    }
    if da >= 0.0 && db >= 0.0 {
        return Some((a, b));
    }
    let denom = da - db;
    let t = if denom.abs() > 1e-6 {
        (da / denom).clamp(0.0, 1.0)
    } else {
        0.5
    };
    let clipped = [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ];
    if da < 0.0 {
        Some((clipped, b))
    } else {
        Some((a, clipped))
    }
}

/// Clip a world-space segment against the camera near plane and project to viewport pixels.
pub fn project_clipped_segment(
    a: [f32; 3],
    b: [f32; 3],
    frame: &CameraFrame,
    width: f32,
    height: f32,
) -> Option<((f32, f32), (f32, f32))> {
    let (ca, cb) = clip_segment_to_near_plane(a, b, frame, 0.06)?;
    let pa = project(ca, frame, width, height)?;
    let pb = project(cb, frame, width, height)?;
    Some(((pa.0, pa.1), (pb.0, pb.1)))
}

/// Intersect a ray with a plane; returns `None` if parallel or behind the origin (when `!allow_behind`).
pub fn ray_intersect_plane(
    ray_origin: [f32; 3],
    ray_dir: [f32; 3],
    plane_origin: [f32; 3],
    plane_normal: [f32; 3],
    allow_behind: bool,
) -> Option<[f32; 3]> {
    let n = Vec3::from(plane_normal);
    let d = Vec3::from(ray_dir);
    let denom = d.dot(n);
    if denom.abs() < 1e-4 {
        return None;
    }
    let o = Vec3::from(ray_origin);
    let p0 = Vec3::from(plane_origin);
    let t = (p0 - o).dot(n) / denom;
    if !t.is_finite() || (!allow_behind && t < 0.0) {
        return None;
    }
    Some((o + d * t).to_array())
}

/// Compute the closest parameter `t` along an infinite world axis `(axis_origin + axis_dir * t)` to a screen ray.
pub fn ray_closest_point_on_axis(
    ray_origin: [f32; 3],
    ray_dir: [f32; 3],
    axis_origin: [f32; 3],
    axis_dir: [f32; 3],
) -> Option<f32> {
    let u = Vec3::from(axis_dir).normalize_or_zero();
    let v = Vec3::from(ray_dir).normalize_or_zero();
    if u.length_squared() < 1e-6 || v.length_squared() < 1e-6 {
        return None;
    }
    let w0 = Vec3::from(axis_origin) - Vec3::from(ray_origin);
    let b = u.dot(v);
    let denom = 1.0 - b * b;
    if denom.abs() < 1e-4 {
        return None;
    }
    let d = u.dot(w0);
    let e = v.dot(w0);
    let t = (b * e - d) / denom;
    t.is_finite().then_some(t)
}

/// Signed right-handed angle (radians) from `prev_vec` to `next_vec` around `axis_dir`.
pub fn signed_angle_around_axis(prev_vec: [f32; 3], next_vec: [f32; 3], axis_dir: [f32; 3]) -> f32 {
    let a = Vec3::from(prev_vec).normalize_or_zero();
    let b = Vec3::from(next_vec).normalize_or_zero();
    let n = Vec3::from(axis_dir).normalize_or_zero();
    if a.length_squared() < 1e-6 || b.length_squared() < 1e-6 || n.length_squared() < 1e-6 {
        return 0.0;
    }
    let sin_term = n.dot(a.cross(b));
    let cos_term = a.dot(b);
    sin_term.atan2(cos_term)
}

/// World length of gizmo axis: keeps screen length approximately constant.
pub fn gizmo_world_length(camera: &OrbitCamera, viewport_height: f32) -> f32 {
    let pixels = GIZMO_SCREEN_PIXELS / viewport_height.max(1.0);
    if camera.orthographic {
        return camera.distance.max(0.05) * pixels * 2.0;
    }
    let tan_half = (camera.fov_y_degrees.clamp(10.0, 170.0) * 0.5)
        .to_radians()
        .tan();
    camera.distance.max(0.05) * pixels * 2.0 * tan_half
}

/// World-space gizmo size keeping constant screen pixel height at `gizmo_center`.
pub fn gizmo_world_length_at(
    camera: &OrbitCamera,
    gizmo_center: [f32; 3],
    viewport_height: f32,
) -> f32 {
    let pixels = GIZMO_SCREEN_PIXELS / viewport_height.max(1.0);
    if camera.orthographic {
        return camera.distance.max(0.05) * pixels * 2.0;
    }
    let frame = camera_frame(camera);
    let depth = (Vec3::from(gizmo_center) - Vec3::from(frame.eye))
        .dot(Vec3::from(frame.forward))
        .abs()
        .max(0.1);
    let tan_half = half_fov_tan(&frame);
    depth * pixels * 2.0 * tan_half
}

/// Screen endpoints `(start, end, pixels per world unit)` of one gizmo axis.
pub type ScreenSegment = ((f32, f32), (f32, f32), f32);

/// Screen endpoints and "pixels per world unit" of one gizmo axis.
pub fn axis_screen_segment(
    frame: &CameraFrame,
    origin: [f32; 3],
    axis: usize,
    world_length: f32,
    width: f32,
    height: f32,
) -> Option<ScreenSegment> {
    let mut direction = [0.0; 3];
    direction[axis] = 1.0;
    axis_screen_segment_oriented(frame, origin, direction, world_length, width, height)
}

/// Axis line segment in given direction (for local gizmo).
pub fn axis_screen_segment_oriented(
    frame: &CameraFrame,
    origin: [f32; 3],
    direction: [f32; 3],
    world_length: f32,
    width: f32,
    height: f32,
) -> Option<ScreenSegment> {
    let end = add(origin, scale(direction, world_length));
    let start = project(origin, frame, width, height)?;
    let end = project(end, frame, width, height)?;
    let px = ((end.0 - start.0).powi(2) + (end.1 - start.1).powi(2)).sqrt();
    Some((
        (start.0, start.1),
        (end.0, end.1),
        px / world_length.max(1e-4),
    ))
}

/// Hit test: returns (axis, screen direction unit vector, pixels per world unit).
pub fn pick_gizmo_axis(
    point: (f32, f32),
    frame: &CameraFrame,
    origin: [f32; 3],
    world_length: f32,
    width: f32,
    height: f32,
) -> Option<(usize, [f32; 2], f32)> {
    pick_gizmo_axis_oriented(
        point,
        frame,
        origin,
        IDENTITY_BASIS,
        world_length,
        width,
        height,
    )
}

/// Axis hit in local coordinates.
pub fn pick_gizmo_axis_oriented(
    point: (f32, f32),
    frame: &CameraFrame,
    origin: [f32; 3],
    basis: [[f32; 3]; 3],
    world_length: f32,
    width: f32,
    height: f32,
) -> Option<(usize, [f32; 2], f32)> {
    let mut best: Option<(usize, [f32; 2], f32, f32)> = None;
    for (axis, direction) in basis.iter().enumerate() {
        let Some((start, end, px_per_unit)) =
            axis_screen_segment_oriented(frame, origin, *direction, world_length, width, height)
        else {
            continue;
        };
        let distance = distance_point_segment(point, start, end);
        if distance > GIZMO_PICK_RADIUS {
            continue;
        }
        let dir = normalize2((end.0 - start.0, end.1 - start.1));
        if best
            .map(|(_, _, _, best_distance)| distance < best_distance)
            .unwrap_or(true)
        {
            best = Some((axis, dir, px_per_unit, distance));
        }
    }
    best.map(|(axis, dir, px, _)| (axis, dir, px))
}

/// Compute world-space corners of a planar gizmo quad spanning `basis[u]` and `basis[v]`.
pub fn plane_quad_corners(
    origin: [f32; 3],
    basis: [[f32; 3]; 3],
    u: usize,
    v: usize,
    world_length: f32,
) -> [[f32; 3]; 4] {
    let min_off = world_length * 0.22;
    let max_off = world_length * 0.44;
    let bu = basis[u];
    let bv = basis[v];
    [
        add(origin, add(scale(bu, min_off), scale(bv, min_off))),
        add(origin, add(scale(bu, max_off), scale(bv, min_off))),
        add(origin, add(scale(bu, max_off), scale(bv, max_off))),
        add(origin, add(scale(bu, min_off), scale(bv, max_off))),
    ]
}

fn point_in_quad(point: (f32, f32), quad: [(f32, f32); 4]) -> bool {
    let mut pos = false;
    let mut neg = false;
    for i in 0..4 {
        let a = quad[i];
        let b = quad[(i + 1) % 4];
        let cross = (b.0 - a.0) * (point.1 - a.1) - (b.1 - a.1) * (point.0 - a.0);
        if cross > 1e-3 {
            pos = true;
        } else if cross < -1e-3 {
            neg = true;
        }
        if pos && neg {
            return false;
        }
    }
    pos || neg
}

/// Pick center, axis arrows, or planar quads of a translate/scale gizmo under `point`.
#[expect(
    clippy::too_many_arguments,
    reason = "Explicit camera frame, basis, and viewport dimensions avoid bundled UI state"
)]
pub fn pick_gizmo_handle_oriented(
    point: (f32, f32),
    frame: &CameraFrame,
    origin: [f32; 3],
    basis: [[f32; 3]; 3],
    world_length: f32,
    ortho_2d: bool,
    width: f32,
    height: f32,
) -> Option<(GizmoHandle, [f32; 2], f32)> {
    if let Some((cx, cy, _)) = project(origin, frame, width, height) {
        let dist_sq = (point.0 - cx).powi(2) + (point.1 - cy).powi(2);
        if dist_sq <= 10.0_f32.powi(2) {
            let world_per_px = if let Some(half_h) = frame.ortho_half_height {
                (half_h * 2.0) / height.max(1.0)
            } else {
                let depth = (Vec3::from(origin) - Vec3::from(frame.eye))
                    .dot(Vec3::from(frame.forward))
                    .abs()
                    .max(0.1);
                (2.0 * depth * half_fov_tan(frame)) / height.max(1.0)
            };
            return Some((
                GizmoHandle::Center,
                [1.0, 0.0],
                1.0 / world_per_px.max(1e-4),
            ));
        }
    }

    let axes: &[usize] = if ortho_2d { &[0, 1] } else { &[0, 1, 2] };
    let mut best_axis: Option<(usize, [f32; 2], f32, f32)> = None;
    for &axis in axes {
        let Some((start, end, px_per_unit)) =
            axis_screen_segment_oriented(frame, origin, basis[axis], world_length, width, height)
        else {
            continue;
        };
        let distance = distance_point_segment(point, start, end);
        if distance > GIZMO_PICK_RADIUS {
            continue;
        }
        let dir = normalize2((end.0 - start.0, end.1 - start.1));
        if best_axis.map(|(_, _, _, d)| distance < d).unwrap_or(true) {
            best_axis = Some((axis, dir, px_per_unit, distance));
        }
    }
    if let Some((axis, dir, px_per_unit, _)) = best_axis {
        return Some((GizmoHandle::Axis(axis), dir, px_per_unit));
    }

    let planes: &[(usize, usize)] = if ortho_2d {
        &[(0, 1)]
    } else {
        &[(1, 2), (0, 2), (0, 1)]
    };
    for &(u, v) in planes {
        let corners = plane_quad_corners(origin, basis, u, v, world_length);
        let mut screen = [(0.0f32, 0.0f32); 4];
        let mut visible = true;
        for (slot, corner) in corners.iter().enumerate() {
            if let Some((x, y, _)) = project(*corner, frame, width, height) {
                screen[slot] = (x, y);
            } else {
                visible = false;
                break;
            }
        }
        if visible && point_in_quad(point, screen) {
            return Some((GizmoHandle::Plane(u, v), [1.0, 0.0], 1.0));
        }
    }
    None
}

pub fn normalize2(v: (f32, f32)) -> [f32; 2] {
    let v = Vec2::new(v.0, v.1);
    (v / v.length().max(1e-6)).to_array()
}

/// 8 corners of entity local unit cube (world coordinates, including rotation and scale).
///
/// Proxy `transform` is already world transform (`world_position` / `world_rotation` /
/// `world_scale`), so no longer derive along parent chain here.
fn transformed_corners(transform: &TransformJson) -> [[f32; 3]; 8] {
    let rotation = quat_normalize(transform.rotation);
    let half = [
        transform.scale[0].abs().max(0.05) * 0.5,
        transform.scale[1].abs().max(0.05) * 0.5,
        transform.scale[2].abs().max(0.05) * 0.5,
    ];
    let mut corners = [[0.0f32; 3]; 8];
    let mut next = 0;
    for dx in [-1.0f32, 1.0] {
        for dy in [-1.0f32, 1.0] {
            for dz in [-1.0f32, 1.0] {
                let local = [half[0] * dx, half[1] * dy, half[2] * dz];
                let rotated = quat_rotate(rotation, local);
                corners[next] = [
                    transform.position[0] + rotated[0],
                    transform.position[1] + rotated[1],
                    transform.position[2] + rotated[2],
                ];
                next += 1;
            }
        }
    }
    corners
}

/// 12 edges of world proxy cube projected to screen space (#487 selection outline).
///
/// Returns `[((x0, y0), (x1, y1)); 12]`, coordinates are **viewport pixels** (top-left origin, same
/// [`box_select`] / `pick_entity` coordinate system). When any corner is behind camera, returns
/// `None`: per-point clipping in that case only draws penetrating junk lines.
pub fn projected_box_edges(
    transform: &TransformJson,
    frame: &CameraFrame,
    width: f32,
    height: f32,
) -> Option<[((f32, f32), (f32, f32)); 12]> {
    let corners = transformed_corners(transform);
    let mut screen = [(0.0f32, 0.0f32); 8];
    for (slot, corner) in corners.iter().enumerate() {
        let (x, y, _) = project(*corner, frame, width, height)?;
        screen[slot] = (x, y);
    }
    const EDGES: [(usize, usize); 12] = [
        (0, 1),
        (2, 3),
        (4, 5),
        (6, 7), 
        (0, 2),
        (1, 3),
        (4, 6),
        (5, 7), 
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7), 
    ];
    let mut out = [((0.0f32, 0.0f32), (0.0f32, 0.0f32)); 12];
    for (slot, (a, b)) in EDGES.iter().enumerate() {
        out[slot] = (screen[*a], screen[*b]);
    }
    Some(out)
}

/// Drag-drop landing point: ray intersection with "authoring plane" (#646).
///
/// 3D view drops asset to ground `y = 0`; 2D view (#641) is front-facing XY, ground there is
/// a sideways line, so switch to `z = 0` (UI / sprite plane). Ray parallel to plane,
/// intersection behind camera (`t <= 0`) or non-finite returns `None`.
pub fn drop_point(camera: &OrbitCamera, ray: ([f32; 3], [f32; 3])) -> Option<[f32; 3]> {
    let (origin, direction) = ray;
    let axis = if camera.orthographic { 2 } else { 1 };
    let mut point = [0.0f32; 3];
    point[axis] = 0.0;
    if direction[axis].abs() <= 1e-4 {
        return None;
    }
    let t = -origin[axis] / direction[axis];
    if !t.is_finite() || t <= 0.0 {
        return None;
    }
    for slot in 0..3 {
        point[slot] = if slot == axis {
            0.0
        } else {
            origin[slot] + direction[slot] * t
        };
    }
    if point.iter().all(|value| value.is_finite()) {
        Some(point)
    } else {
        None
    }
}

/// 4 edges of 2D rectangle (sprite / rect) projected to screen space (#643).
///
/// `size` is width/height in world units, `pivot` is `0..1` pivot (bottom-left as origin,
/// consistent with `SpriteRenderer::pivot`). Rectangle on entity local XY plane,
/// following `transform` rotation / scale; returns 4 connected edges,
/// Coordinates also **viewport pixels** (top-left origin).
pub fn projected_rect_edges(
    transform: &TransformJson,
    size: [f32; 2],
    pivot: [f32; 2],
    frame: &CameraFrame,
    width: f32,
    height: f32,
) -> Option<[((f32, f32), (f32, f32)); 4]> {
    if !(size[0].is_finite() && size[1].is_finite()) || size[0] <= 0.0 || size[1] <= 0.0 {
        return None;
    }
    let (x0, x1) = (-pivot[0] * size[0], (1.0 - pivot[0]) * size[0]);
    let (y0, y1) = (-pivot[1] * size[1], (1.0 - pivot[1]) * size[1]);
    let local = [
        [x0 * transform.scale[0], y0 * transform.scale[1], 0.0],
        [x1 * transform.scale[0], y0 * transform.scale[1], 0.0],
        [x1 * transform.scale[0], y1 * transform.scale[1], 0.0],
        [x0 * transform.scale[0], y1 * transform.scale[1], 0.0],
    ];
    let mut screen = [(0.0f32, 0.0f32); 4];
    for (slot, corner) in local.iter().enumerate() {
        let rotated = quat_rotate(transform.rotation, *corner);
        let world = add(transform.position, rotated);
        let (x, y, _) = project(world, frame, width, height)?;
        screen[slot] = (x, y);
    }
    Some([
        (screen[0], screen[1]),
        (screen[1], screen[2]),
        (screen[2], screen[3]),
        (screen[3], screen[0]),
    ])
}

/// Convert screen drag box to viewport pixel coordinates `(min_x, min_y, max_x, max_y)`.
///
/// [`box_select`] and `pick_entity` use **viewport top-left** as origin, while egui gives screen coordinates:
/// When hierarchy panel on viewport left origin is non-zero. Forgot to subtract origin, box select always starts from viewport top-left,
/// selects all objects above cursor together (#422). Reverse drag also normalized to min/max here.
pub fn local_box(start: (f32, f32), end: (f32, f32), origin: (f32, f32)) -> (f32, f32, f32, f32) {
    let start = (start.0 - origin.0, start.1 - origin.1);
    let end = (end.0 - origin.0, end.1 - origin.1);
    (
        start.0.min(end.0),
        start.1.min(end.1),
        start.0.max(end.0),
        start.1.max(end.1),
    )
}

/// Box select: entity rotated AABB projected to screen AABB, selected when **intersects** rectangle.
///
/// Rectangle is `(min_x, min_y, max_x, max_y)` (viewport pixel coordinates); indices returned in document order.
/// Uses intersection not "corners inside rectangle", so large objects filling viewport can be selected by small box.
/// Project all 12 world proxy box edges clipped against the camera near plane.
pub fn projected_box_clipped_edges(
    transform: &TransformJson,
    frame: &CameraFrame,
    width: f32,
    height: f32,
) -> Vec<((f32, f32), (f32, f32))> {
    let corners = transformed_corners(transform);
    const EDGES: [(usize, usize); 12] = [
        (0, 1),
        (2, 3),
        (4, 5),
        (6, 7),
        (0, 2),
        (1, 3),
        (4, 6),
        (5, 7),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];
    let mut out = Vec::with_capacity(12);
    for (a, b) in EDGES {
        if let Some(seg) = project_clipped_segment(corners[a], corners[b], frame, width, height) {
            out.push(seg);
        }
    }
    out
}

pub fn box_select(
    rect: (f32, f32, f32, f32),
    frame: &CameraFrame,
    width: f32,
    height: f32,
    entities: &[(usize, TransformJson)],
) -> Vec<usize> {
    let (min_x, min_y, max_x, max_y) = rect;
    let mut selected = Vec::new();
    for (index, transform) in entities {
        let mut bounds: Option<(f32, f32, f32, f32)> = None;
        for (a, b) in projected_box_clipped_edges(transform, frame, width, height) {
            for (x, y) in [a, b] {
                bounds = Some(match bounds {
                    None => (x, y, x, y),
                    Some((min_px, min_py, max_px, max_py)) => {
                        (min_px.min(x), min_py.min(y), max_px.max(x), max_py.max(y))
                    }
                });
            }
        }
        let Some((min_px, min_py, max_px, max_py)) = bounds else {
            continue;
        };
        if max_px >= min_x && min_px <= max_x && max_py >= min_y && min_py <= max_y {
            selected.push(*index);
        }
    }
    selected
}

/// Ray intersection with entity **oriented** AABB, returns world-space ray parameter.
///
/// Transform ray into entity local space (first inverse-rotate, then divide scale) then intersect unit cube
/// `[-0.5, 0.5]^3`; `ray_box` returned parameter same scale as world ray, so
/// entities with different scales can still be directly compared by distance.
fn ray_obb(origin: [f32; 3], direction: [f32; 3], transform: &TransformJson) -> Option<f32> {
    let inverse = quat_conjugate(quat_normalize(transform.rotation));
    let scale = [
        transform.scale[0].abs().max(0.05),
        transform.scale[1].abs().max(0.05),
        transform.scale[2].abs().max(0.05),
    ];
    let offset = [
        origin[0] - transform.position[0],
        origin[1] - transform.position[1],
        origin[2] - transform.position[2],
    ];
    let local_offset = quat_rotate(inverse, offset);
    let local_direction = quat_rotate(inverse, direction);
    ray_box(
        [
            local_offset[0] / scale[0],
            local_offset[1] / scale[1],
            local_offset[2] / scale[2],
        ],
        [
            local_direction[0] / scale[0],
            local_direction[1] / scale[1],
            local_direction[2] / scale[2],
        ],
        [0.0, 0.0, 0.0],
        [0.5, 0.5, 0.5],
    )
}

/// Use entity real local AABB for OBB hit; falls back to unit cube when `None`.
fn ray_obb_with_bounds(
    origin: [f32; 3],
    direction: [f32; 3],
    transform: &TransformJson,
    bounds: Option<PickBounds>,
) -> Option<f32> {
    let Some(bounds) = bounds else {
        return ray_obb(origin, direction, transform);
    };
    let inverse = quat_conjugate(quat_normalize(transform.rotation));
    let scale = [
        transform.scale[0].abs().max(0.05),
        transform.scale[1].abs().max(0.05),
        transform.scale[2].abs().max(0.05),
    ];
    let offset = [
        origin[0] - transform.position[0],
        origin[1] - transform.position[1],
        origin[2] - transform.position[2],
    ];
    let local_offset = quat_rotate(inverse, offset);
    let local_direction = quat_rotate(inverse, direction);
    ray_box(
        [
            local_offset[0] / scale[0],
            local_offset[1] / scale[1],
            local_offset[2] / scale[2],
        ],
        [
            local_direction[0] / scale[0],
            local_direction[1] / scale[1],
            local_direction[2] / scale[2],
        ],
        bounds.center(),
        bounds.half(),
    )
}

/// Pick entity nearest to camera (index, hit distance).
///
/// Compatible with old calls: use unit cube when no bounds data.
pub fn pick_entity(
    point: (f32, f32),
    frame: &CameraFrame,
    width: f32,
    height: f32,
    entities: &[(usize, TransformJson)],
) -> Option<usize> {
    let (origin, direction) = ray_from_screen(point.0, point.1, frame, width, height);
    let mut best: Option<(usize, f32)> = None;
    for (index, transform) in entities {
        if let Some(distance) = ray_obb(origin, direction, transform) {
            if best.map(|(_, best)| distance < best).unwrap_or(true) {
                best = Some((*index, distance));
            }
        }
    }
    best.map(|(index, _)| index)
}

/// **All** hits under the same screen point, sorted by distance near to far (#416 click cycle penetration).
///
/// Returns entity indices; repeated clicks at same position select next covered layer in turn.
pub fn pick_entities_with_bounds(
    point: (f32, f32),
    frame: &CameraFrame,
    width: f32,
    height: f32,
    entities: &[(usize, TransformJson, Option<PickBounds>)],
) -> Vec<usize> {
    let (origin, direction) = ray_from_screen(point.0, point.1, frame, width, height);
    let mut hits: Vec<(usize, f32)> = entities
        .iter()
        .filter_map(|(index, transform, bounds)| {
            ray_obb_with_bounds(origin, direction, transform, *bounds)
                .map(|distance| (*index, distance))
        })
        .collect();
    hits.sort_by(|a, b| a.1.total_cmp(&b.1));
    hits.into_iter().map(|(index, _)| index).collect()
}

/// Picking with real local AABB (#600).
pub fn pick_entity_with_bounds(
    point: (f32, f32),
    frame: &CameraFrame,
    width: f32,
    height: f32,
    entities: &[(usize, TransformJson, Option<PickBounds>)],
) -> Option<usize> {
    pick_entities_with_bounds(point, frame, width, height, entities)
        .first()
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> OrbitCamera {
        OrbitCamera {
            target: [0.0, 0.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
            distance: 10.0,
            orthographic: false,
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 1000.0,
        }
    }

    fn camera_2d() -> OrbitCamera {
        let mut camera = camera();
        camera.set_2d(true);
        camera
    }

    /// #641: 2D view is orthographic projection, screen position independent of depth.
    #[test]
    fn ortho_projection_ignores_depth() {
        let camera = camera_2d();
        let frame = camera_frame(&camera);
        let near = project([1.0, 0.5, 0.0], &frame, 800.0, 600.0).unwrap();
        let far = project([1.0, 0.5, -6.0], &frame, 800.0, 600.0).unwrap();
        assert!((near.0 - far.0).abs() < 1e-4);
        assert!((near.1 - far.1).abs() < 1e-4);
        assert!((near.0 - 430.0).abs() < 1e-3);
        assert!((near.1 - 285.0).abs() < 1e-3);
    }

    /// Orthographic rays parallel to each other, origins spread by pixels; center ray starts from eye.
    #[test]
    fn ortho_rays_are_parallel() {
        let camera = camera_2d();
        let frame = camera_frame(&camera);
        let (o_center, d_center) = ray_from_screen(400.0, 300.0, &frame, 800.0, 600.0);
        let (o_side, d_side) = ray_from_screen(700.0, 100.0, &frame, 800.0, 600.0);
        for axis in 0..3 {
            assert!((d_center[axis] - d_side[axis]).abs() < 1e-5);
        }
        assert!((o_center[0] - frame.eye[0]).abs() < 1e-4);
        assert!((o_center[1] - frame.eye[1]).abs() < 1e-4);
        assert!(
            o_side[0] > o_center[0],
            "screen right -> origin translates +X"
        );
        assert!(o_side[1] > o_center[1], "screen up -> origin translates +Y");
        assert!(ray_box(o_center, d_center, [0.0, 0.0, 0.0], [0.5, 0.5, 0.5]).is_some());
        assert!(ray_box(o_side, d_side, [0.0, 0.0, 0.0], [0.5, 0.5, 0.5]).is_none());
    }

    fn rect_transform(position: [f32; 3], scale: [f32; 3]) -> TransformJson {
        TransformJson {
            position,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale,
        }
    }

    /// #643: sprite rect lands on correct screen pixels by `size` + `pivot`.
    #[test]
    fn sprite_rect_uses_size_and_pivot() {
        let camera = camera_2d();
        let frame = camera_frame(&camera);
        let centered = rect_transform([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let edges =
            projected_rect_edges(&centered, [2.0, 1.0], [0.5, 0.5], &frame, 800.0, 600.0).unwrap();
        let xs: Vec<f32> = edges.iter().map(|(a, _)| a.0).collect();
        let ys: Vec<f32> = edges.iter().map(|(a, _)| a.1).collect();
        assert!((xs.iter().cloned().fold(f32::MIN, f32::max) - 430.0).abs() < 1e-3);
        assert!((xs.iter().cloned().fold(f32::MAX, f32::min) - 370.0).abs() < 1e-3);
        assert!((ys.iter().cloned().fold(f32::MIN, f32::max) - 315.0).abs() < 1e-3);
        assert!((ys.iter().cloned().fold(f32::MAX, f32::min) - 285.0).abs() < 1e-3);

        let bottom =
            projected_rect_edges(&centered, [2.0, 1.0], [0.5, 0.0], &frame, 800.0, 600.0).unwrap();
        let min_y = bottom.iter().map(|(a, _)| a.1).fold(f32::MAX, f32::min);
        let max_y = bottom.iter().map(|(a, _)| a.1).fold(f32::MIN, f32::max);
        assert!((max_y - 300.0).abs() < 1e-3);
        assert!((min_y - 270.0).abs() < 1e-3);
    }

    /// Degenerate size (no `frame_size` filled) not drawn, avoids zero-area rect.
    #[test]
    fn sprite_rect_rejects_degenerate_sizes() {
        let camera = camera_2d();
        let frame = camera_frame(&camera);
        let transform = rect_transform([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        assert!(
            projected_rect_edges(&transform, [0.0, 1.0], [0.5, 0.5], &frame, 800.0, 600.0)
                .is_none()
        );
        assert!(projected_rect_edges(
            &transform,
            [1.0, f32::NAN],
            [0.5, 0.5],
            &frame,
            800.0,
            600.0
        )
        .is_none());
    }

    /// #646: Drag-drop landing point --- 3D hits ground y = 0, 2D hits z = 0.
    #[test]
    fn drop_point_uses_the_authoring_plane_per_view() {
        let mut camera = camera_2d();
        let frame = camera_frame(&camera);
        let ray = ray_from_screen(430.0, 285.0, &frame, 800.0, 600.0);
        let point = drop_point(&camera, ray).unwrap();
        assert!(
            point[2].abs() < 1e-4,
            "2D landing point must be on z = 0 plane"
        );
        assert!((point[0] - 1.0).abs() < 1e-3);
        assert!((point[1] - 0.5).abs() < 1e-3);

        camera.set_2d(false);
        camera.pitch = 0.6;
        let frame = camera_frame(&camera);
        let ray = ray_from_screen(400.0, 300.0, &frame, 800.0, 600.0);
        let point = drop_point(&camera, ray).unwrap();
        assert!(point[1].abs() < 1e-4, "3D landing point must be on ground");
        assert!(point[0].abs() < 1e-3 && point[2].abs() < 1e-3);
    }

    #[test]
    fn drop_point_rejects_rays_parallel_to_the_plane() {
        let camera = camera();
        let frame = camera_frame(&camera);
        let ray = ray_from_screen(400.0, 300.0, &frame, 800.0, 600.0);
        assert!(drop_point(&camera, ray).is_none());
        assert!(drop_point(&camera, ([0.0, 5.0, 0.0], [1.0, 0.0, 0.0])).is_none());
        assert!(drop_point(&camera, ([0.0, 5.0, 0.0], [0.0, 1.0, 0.0])).is_none());
        let point = drop_point(&camera, ([1.0, 5.0, 2.0], [0.0, -1.0, 0.0])).unwrap();
        assert_eq!(point, [1.0, 0.0, 2.0]);
    }

    #[test]
    fn ortho_gizmo_length_follows_the_ortho_size() {
        let camera = camera_2d();
        let length = gizmo_world_length(&camera, 600.0);
        let expected = camera.distance * GIZMO_SCREEN_PIXELS / 600.0 * 2.0;
        assert!((length - expected).abs() < 1e-4);
        assert!(length.is_finite());
    }

    #[test]
    fn projects_the_orbit_target_to_screen_center() {
        let camera = camera();
        let frame = camera_frame(&camera);
        let (x, y, depth) = project([0.0, 0.0, 0.0], &frame, 800.0, 600.0).unwrap();
        assert!((x - 400.0).abs() < 1e-3);
        assert!((y - 300.0).abs() < 1e-3);
        assert!((depth - 10.0).abs() < 1e-3);
    }

    #[test]
    fn points_behind_the_camera_are_rejected() {
        let camera = camera();
        let frame = camera_frame(&camera);
        assert!(project([0.0, 0.0, 20.0], &frame, 800.0, 600.0).is_none());
    }

    #[test]
    fn ray_through_center_hits_box_at_origin() {
        let camera = camera();
        let frame = camera_frame(&camera);
        let (origin, direction) = ray_from_screen(400.0, 300.0, &frame, 800.0, 600.0);
        let distance = ray_box(origin, direction, [0.0, 0.0, 0.0], [0.5, 0.5, 0.5]).unwrap();
        assert!((distance - 9.5).abs() < 0.2, "distance = {distance}");

        let (origin, direction) = ray_from_screen(10.0, 10.0, &frame, 800.0, 600.0);
        assert!(ray_box(origin, direction, [0.0, 0.0, 0.0], [0.5, 0.5, 0.5]).is_none());
    }

    #[test]
    fn picks_nearest_entity_under_cursor() {
        let camera = camera();
        let frame = camera_frame(&camera);
        let entities = vec![
            (
                0,
                TransformJson {
                    position: [0.0, 0.0, 0.0],
                    ..Default::default()
                },
            ),
            (
                1,
                TransformJson {
                    position: [3.0, 0.0, 0.0],
                    ..Default::default()
                },
            ),
        ];
        let picked = pick_entity((400.0, 300.0), &frame, 800.0, 600.0, &entities);
        assert_eq!(picked, Some(0));

        let (x, _, _) = project([3.0, 0.0, 0.0], &frame, 800.0, 600.0).unwrap();
        let picked = pick_entity((x, 300.0), &frame, 800.0, 600.0, &entities);
        assert_eq!(picked, Some(1));
    }

    #[test]
    fn projection_follows_the_configured_fov() {
        let mut camera = camera();
        let frame = camera_frame(&camera);
        let (wide_x, _, _) = project([1.0, 0.0, 0.0], &frame, 800.0, 600.0).unwrap();
        camera.set_fov_degrees(30.0);
        let frame = camera_frame(&camera);
        let (narrow_x, _, _) = project([1.0, 0.0, 0.0], &frame, 800.0, 600.0).unwrap();
        let wide_offset = (wide_x - 400.0).abs();
        let narrow_offset = (narrow_x - 400.0).abs();
        assert!(
            narrow_offset > wide_offset * 1.5,
            "30° offset {narrow_offset} should be clearly larger than 60° offset {wide_offset}"
        );
        let frame = camera_frame(&camera);
        let (origin, direction) = ray_from_screen(narrow_x, 300.0, &frame, 800.0, 600.0);
        assert!(origin.iter().all(|value| value.is_finite()));
        assert!(direction.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn overlapping_hits_are_returned_near_to_far() {
        let camera = camera();
        let frame = camera_frame(&camera);
        let entities = vec![
            (
                0,
                TransformJson {
                    position: [0.0, 0.0, -5.0],
                    ..TransformJson::default()
                },
                None,
            ),
            (
                1,
                TransformJson {
                    position: [0.0, 0.0, 2.0],
                    ..TransformJson::default()
                },
                None,
            ),
        ];
        let hits = pick_entities_with_bounds((400.0, 300.0), &frame, 800.0, 600.0, &entities);
        assert_eq!(hits, vec![1, 0], "near z = 2 in front, far z = -5 behind");
        assert_eq!(
            pick_entity_with_bounds((400.0, 300.0), &frame, 800.0, 600.0, &entities),
            Some(1)
        );
    }

    #[test]
    fn picking_uses_the_local_bounds_when_provided() {
        let camera = camera_2d();
        let frame = camera_frame(&camera);
        let entities = vec![(
            0,
            TransformJson::default(),
            Some(PickBounds {
                min: [2.0, -1.0, -0.1],
                max: [8.0, 1.0, 0.1],
            }),
        )];
        let (x, y, _) = project([5.0, 0.0, 0.0], &frame, 800.0, 600.0).unwrap();
        assert_eq!(
            pick_entity_with_bounds((x, y), &frame, 800.0, 600.0, &entities),
            Some(0),
            "should hit inside real AABB"
        );
        assert_eq!(
            pick_entity_with_bounds((400.0, 300.0), &frame, 800.0, 600.0, &entities),
            None,
            "unit cube at origin should no longer impersonate whole sprite"
        );
    }

    #[test]
    fn picking_uses_the_rotated_box_not_its_axis_aligned_bounds() {
        let plate = TransformJson {
            position: [0.0, 0.0, 0.0],
            rotation: quat_axis_angle(2, std::f32::consts::FRAC_PI_4),
            scale: [2.0, 0.1, 2.0],
        };
        let (origin, direction) = ([0.9f32, 0.0, 5.0], [0.0f32, 0.0, -1.0]);
        assert!(
            ray_obb(origin, direction, &plate).is_none(),
            "empty AABB corner should not hit rotated thin plate"
        );
        assert!(
            ray_obb([0.636, 0.636, 5.0], direction, &plate).is_some(),
            "rotated thin plate body should be clickable"
        );
    }

    #[test]
    fn box_select_matches_a_wall_that_fills_the_viewport() {
        let camera = camera();
        let frame = camera_frame(&camera);
        let wall = TransformJson {
            position: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [40.0, 40.0, 0.2],
        };
        let entities = vec![(7, wall)];
        let hits = box_select(
            (100.0, 100.0, 150.0, 150.0),
            &frame,
            800.0,
            600.0,
            &entities,
        );
        assert_eq!(
            hits,
            vec![7],
            "large object intersecting screen rect should be box-selected"
        );

        let misses = box_select(
            (4000.0, 4000.0, 4100.0, 4100.0),
            &frame,
            800.0,
            600.0,
            &entities,
        );
        assert!(misses.is_empty());
    }

    #[test]
    fn euler_conversion_composes_glam_axis_rotations() {
        let euler = [20.0f32, -35.0, 50.0];
        let expected = glam::Quat::from_rotation_z(euler[2].to_radians())
            * glam::Quat::from_rotation_y(euler[1].to_radians())
            * glam::Quat::from_rotation_x(euler[0].to_radians());
        let q = euler_degrees_to_quat(euler);
        for (got, want) in q
            .iter()
            .zip([expected.x, expected.y, expected.z, expected.w])
        {
            assert!((got - want).abs() < 1e-6, "{q:?} != {expected:?}");
        }
    }

    #[test]
    fn euler_round_trips_through_quaternions() {
        for euler in [
            [0.0f32, 0.0, 0.0],
            [0.0, 90.0, 0.0],
            [30.0, 45.0, -60.0],
            [-120.0, 10.0, 170.0],
        ] {
            let quat = euler_degrees_to_quat(euler);
            let back = quat_to_euler_degrees(quat);
            for axis in 0..3 {
                assert!(
                    (back[axis] - euler[axis]).abs() < 1e-4,
                    "euler {euler:?} -> {back:?}"
                );
            }
        }
    }

    #[test]
    fn euler_conversion_handles_the_gimbal_pole_and_degenerate_quat() {
        let quat = euler_degrees_to_quat([20.0, 90.0, 0.0]);
        let back = quat_to_euler_degrees(quat);
        assert!((back[1] - 90.0).abs() < 1e-3, "{back:?}");
        assert_eq!(back[2], 0.0);
        assert_eq!(quat_to_euler_degrees([0.0; 4]), [0.0; 3]);
        let long = euler_degrees_to_quat([0.0, 90.0, 0.0]).map(|value| value * 3.0);
        let back = quat_to_euler_degrees(long);
        assert!((back[1] - 90.0).abs() < 1e-4, "{back:?}");
    }

    #[test]
    fn gizmo_world_length_follows_the_viewport_height() {
        let camera = camera();
        let short = gizmo_world_length(&camera, 600.0);
        let tall = gizmo_world_length(&camera, 1200.0);
        assert!((short - tall * 2.0).abs() < 1e-4);
        assert!(
            gizmo_world_length(&camera, 0.0).is_finite(),
            "degenerate height cannot divide by zero"
        );
    }

    #[test]
    fn gizmo_axis_pick_and_pixel_scale() {
        let camera = camera();
        let frame = camera_frame(&camera);
        let length = gizmo_world_length(&camera, 600.0);
        let (start, end, px_per_unit) =
            axis_screen_segment(&frame, [0.0, 0.0, 0.0], 0, length, 800.0, 600.0).unwrap();
        assert!(px_per_unit > 1.0);
        let middle = ((start.0 + end.0) * 0.5, (start.1 + end.1) * 0.5);
        let hit = pick_gizmo_axis(middle, &frame, [0.0, 0.0, 0.0], length, 800.0, 600.0);
        let (axis, _, _) = hit.expect("X axis hit");
        assert_eq!(axis, 0, "horizontal direction closest to X axis");

        assert!(
            pick_gizmo_axis((10.0, 590.0), &frame, [0.0, 0.0, 0.0], length, 800.0, 600.0).is_none()
        );
    }

    #[test]
    fn quaternion_rotation_about_world_axes() {
        let q = quat_axis_angle(0, std::f32::consts::FRAC_PI_2);
        let rotated = quat_rotate(q, [0.0, 1.0, 0.0]);
        assert!((rotated[0]).abs() < 1e-4);
        assert!((rotated[1]).abs() < 1e-4);
        assert!((rotated[2] - 1.0).abs() < 1e-4, "+Y → +Z");

        let qx = quat_axis_angle(0, std::f32::consts::FRAC_PI_2);
        let qy = quat_axis_angle(1, std::f32::consts::FRAC_PI_2);
        let combined = quat_normalize(quat_mul(qy, qx));
        let v = quat_rotate(combined, [0.0, 1.0, 0.0]);
        assert!((v[0] - 1.0).abs() < 1e-4, "+Y → +Z → +X: {v:?}");
    }

    #[test]
    fn ring_pick_and_angle_helpers() {
        let camera = camera();
        let frame = camera_frame(&camera);
        let radius = gizmo_world_length_at(&camera, [0.0, 0.0, 0.0], 600.0);
        let y_segs = rotation_ring_segments_oriented(
            &frame,
            [0.0, 0.0, 0.0],
            IDENTITY_BASIS,
            1,
            radius,
            800.0,
            600.0,
            GIZMO_RING_SEGMENTS,
        );
        assert!(!y_segs.is_empty());
        let sample = y_segs[0].0;
        let hit = pick_rotation_handle_oriented(
            sample,
            &frame,
            [0.0, 0.0, 0.0],
            IDENTITY_BASIS,
            radius,
            false,
            800.0,
            600.0,
        );
        assert_eq!(hit, Some(GizmoHandle::Axis(1)));

        assert!((wrap_angle(3.0 * std::f32::consts::PI) - std::f32::consts::PI).abs() < 1e-4);
        assert!((wrap_angle(-3.0 * std::f32::consts::PI) + std::f32::consts::PI).abs() < 1e-4);
        let a = screen_angle((10.0, 0.0), (0.0, 0.0));
        assert!(a.abs() < 1e-5);
    }

    #[test]
    fn quaternion_conjugate_inverts_rotation() {
        let q = quat_normalize(quat_axis_angle(1, 0.7));
        let v = [0.3, -0.5, 0.8];
        let round_trip = quat_rotate(quat_conjugate(q), quat_rotate(q, v));
        for axis in 0..3 {
            assert!((round_trip[axis] - v[axis]).abs() < 1e-4);
        }
    }

    #[test]
    fn basis_from_quat_rotates_axes() {
        let basis = basis_from_quat(quat_axis_angle(1, std::f32::consts::FRAC_PI_2));
        assert!((basis[0][2] + 1.0).abs() < 1e-4);
        assert!((basis[1][1] - 1.0).abs() < 1e-4);
        assert!((basis[2][0] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn world_rotation_composes_parent_chain() {
        let mut doc = SceneDoc::default();
        let root = doc.add_named_child(None, "Root");
        doc.entities[root].transform.rotation = quat_axis_angle(1, std::f32::consts::FRAC_PI_2);
        let child = doc.add_named_child(Some(root), "Child");
        doc.entities[child].transform.rotation = quat_axis_angle(1, std::f32::consts::FRAC_PI_2);

        let x = quat_rotate(world_rotation(&doc, child), [1.0, 0.0, 0.0]);
        assert!((x[0] + 1.0).abs() < 1e-4, "two 90° = 180°, X -> −X");

        let parent_x = quat_rotate(parent_world_rotation(&doc, child), [1.0, 0.0, 0.0]);
        assert!(
            (parent_x[2] + 1.0).abs() < 1e-4,
            "parent only has Root own rotation"
        );
    }

    #[test]
    fn oriented_gizmo_pick_uses_local_axis() {
        let camera = camera();
        let frame = camera_frame(&camera);
        let basis = basis_from_quat(quat_axis_angle(2, 0.6));
        let length = gizmo_world_length(&camera, 600.0);
        let (start, end, _) =
            axis_screen_segment_oriented(&frame, [0.0, 0.0, 0.0], basis[0], length, 800.0, 600.0)
                .unwrap();
        let middle = ((start.0 + end.0) * 0.5, (start.1 + end.1) * 0.5);
        let hit =
            pick_gizmo_axis_oriented(middle, &frame, [0.0, 0.0, 0.0], basis, length, 800.0, 600.0);
        assert_eq!(hit.map(|(axis, _, _)| axis), Some(0));
    }

    #[test]
    fn box_select_uses_projected_bounds() {
        let camera = camera();
        let frame = camera_frame(&camera);
        let entities = vec![
            (0, TransformJson::default()),
            (
                1,
                TransformJson {
                    position: [3.0, 0.0, 0.0],
                    ..Default::default()
                },
            ),
        ];
        let (center_x, center_y, _) = project([0.0, 0.0, 0.0], &frame, 800.0, 600.0).unwrap();
        let small = (
            center_x - 20.0,
            center_y - 20.0,
            center_x + 20.0,
            center_y + 20.0,
        );
        assert_eq!(box_select(small, &frame, 800.0, 600.0, &entities), vec![0]);
        assert_eq!(
            box_select((0.0, 0.0, 800.0, 600.0), &frame, 800.0, 600.0, &entities),
            vec![0, 1]
        );
        assert!(box_select((0.0, 0.0, 4.0, 4.0), &frame, 800.0, 600.0, &entities).is_empty());
    }

    #[test]
    fn box_select_starts_where_the_drag_started_not_at_the_viewport_origin() {
        let origin = (300.0, 120.0);
        assert_eq!(
            local_box((700.0, 500.0), (720.0, 520.0), origin),
            (400.0, 380.0, 420.0, 400.0)
        );
    }

    #[test]
    fn a_world_box_projects_twelve_edges() {
        let camera = camera();
        let frame = camera_frame(&camera);
        let transform = TransformJson::default();
        let edges = projected_box_edges(&transform, &frame, 800.0, 600.0).expect("in front");
        assert_eq!(edges.len(), 12);
        let center_x: f32 = edges.iter().map(|(a, b)| (a.0 + b.0) * 0.5).sum::<f32>() / 12.0;
        let center_y: f32 = edges.iter().map(|(a, b)| (a.1 + b.1) * 0.5).sum::<f32>() / 12.0;
        assert!((center_x - 400.0).abs() < 1.0, "{center_x}");
        assert!((center_y - 300.0).abs() < 1.0, "{center_y}");

        let behind = TransformJson {
            position: [0.0, 0.0, 50.0],
            ..TransformJson::default()
        };
        assert!(projected_box_edges(&behind, &frame, 800.0, 600.0).is_none());
    }

    #[test]
    fn local_box_normalizes_a_reverse_drag() {
        assert_eq!(
            local_box((720.0, 520.0), (700.0, 500.0), (300.0, 120.0)),
            (400.0, 380.0, 420.0, 400.0)
        );
    }

    #[test]
    fn world_position_and_scale_follow_parent_chain() {
        let mut doc = SceneDoc::default();
        let parent = doc.add_named_child(None, "Parent");
        doc.entities[parent].transform.position = [1.0, 2.0, 3.0];
        doc.entities[parent].transform.scale = [2.0, 2.0, 2.0];
        let child = doc.add_named_child(Some(parent), "Child");
        doc.entities[child].transform.position = [1.0, 0.0, 0.0];
        let world = world_position(&doc, child);
        assert!(
            (world[0] - 3.0).abs() < 1e-4,
            "parent scale applies to child local position: {world:?}"
        );
        assert!((world[1] - 2.0).abs() < 1e-4);
        assert!((world[2] - 3.0).abs() < 1e-4);
        let scale = world_scale(&doc, child);
        assert_eq!(scale, [2.0, 2.0, 2.0]);
    }

    #[test]
    fn world_bounds_wraps_the_unit_cube_and_follows_the_parent_chain() {
        let mut doc = SceneDoc::default();
        let parent = doc.add_named_child(None, "Parent");
        doc.entities[parent].transform.position = [100.0, 0.0, 0.0];
        let child = doc.add_named_child(Some(parent), "Child");

        let (center, radius) = world_bounds(&doc, &[child]).expect("bounds");
        assert!(
            (center[0] - 100.0).abs() < 1e-4,
            "child should be at world x=100: {center:?}"
        );
        assert!((radius - 0.866_025).abs() < 1e-3, "radius {radius}");

        doc.entities[parent].transform.scale = [10.0, 1.0, 1.0];
        let (_, scaled_radius) = world_bounds(&doc, &[child]).expect("bounds");
        assert!(
            scaled_radius > radius * 5.0,
            "radius after scale should grow: {scaled_radius}"
        );

        doc.entities[parent].transform.scale = [1.0; 3];
        doc.entities[parent].transform.rotation = euler_degrees_to_quat([30.0, 45.0, 10.0]);
        let (_, rotated_radius) = world_bounds(&doc, &[child]).expect("bounds");
        assert!(
            (rotated_radius - radius).abs() < 1e-3,
            "rotation should not change radius"
        );
    }

    /// #991: 2D sprite framing must use texture size, cannot substitute unit cube.
    #[test]
    fn world_bounds_uses_the_sprite_extent_from_the_document() {
        let mut doc = SceneDoc::default();
        let entity = doc.add_named_child(None, "Boat");
        doc.entities[entity].components.insert(
            "prism_engine::scene::SpriteRenderer".to_string(),
            serde_json::json!({
                "sprite_path": "sprites/boat.png",
                "frame_size": [64, 16],
                "pixels_per_unit": 1.0,
                "pivot": [0.5, 0.5],
            }),
        );
        let (_, radius) = world_bounds(&doc, &[entity]).expect("bounds");
        let expected = (32.0f32 * 32.0 + 8.0 * 8.0 + 0.05 * 0.05).sqrt();
        assert!(
            (radius - expected).abs() < 1e-2,
            "radius should come from texture size {expected}, actual {radius}"
        );

        doc.entities[entity].components.insert(
            "prism_engine::scene::SpriteRenderer".to_string(),
            serde_json::json!({
                "sprite_path": "sprites/boat.png",
                "frame_size": [0, 0],
                "pixels_per_unit": 1.0,
            }),
        );
        let (_, unit) = world_bounds(&doc, &[entity]).expect("bounds");
        assert!(
            (unit - 0.866_025).abs() < 1e-3,
            "fell back to unit cube: {unit}"
        );
    }

    /// Collider fits shape better than sprite, preferred for framing.
    #[test]
    fn world_bounds_prefers_the_collider_shape() {
        let mut doc = SceneDoc::default();
        let entity = doc.add_named_child(None, "Crate");
        doc.entities[entity].components.insert(
            "prism_engine::scene::SpriteRenderer".to_string(),
            serde_json::json!({ "frame_size": [4, 4], "pixels_per_unit": 1.0 }),
        );
        doc.entities[entity].components.insert(
            "prism_engine::scene::Collider".to_string(),
            serde_json::json!({ "shape": "Box", "half_extents": [2.0, 1.0, 0.5] }),
        );
        let (_, radius) = world_bounds(&doc, &[entity]).expect("bounds");
        let expected = (4.0f32 + 1.0 + 0.25).sqrt();
        assert!(
            (radius - expected).abs() < 1e-3,
            "collider half-size 2×1×0.5 -> radius {expected}, actual {radius}"
        );
    }

    /// Editor-injected bounds take precedence over document-computable part (texture size only engine knows).
    #[test]
    fn world_bounds_with_uses_the_injected_bounds() {
        let mut doc = SceneDoc::default();
        let entity = doc.add_named_child(None, "Boat");
        let injected = PickBounds {
            min: [-32.0, -8.0, -0.05],
            max: [32.0, 8.0, 0.05],
        };
        let (_, radius) = world_bounds_with(&doc, &[entity], |_| Some(injected)).expect("bounds");
        assert!(
            radius > 32.0,
            "after injecting 64×16 radius should exceed 32, actual {radius}"
        );
        let (_, unit) = world_bounds(&doc, &[entity]).expect("bounds");
        assert!(
            unit < 1.0,
            "same entity without injection still unit cube: {unit}"
        );
    }

    #[test]
    fn world_bounds_handles_empty_and_broken_input() {
        let doc = SceneDoc::default();
        assert!(world_bounds(&doc, &[]).is_none());
        assert!(
            world_bounds(&doc, &[7]).is_none(),
            "out-of-bounds index skipped"
        );

        let mut doc = SceneDoc::default();
        let entity = doc.add_named_child(None, "Tiny");
        doc.entities[entity].transform.scale = [0.0; 3];
        let (_, radius) = world_bounds(&doc, &[entity]).expect("zero scale still needs a radius");
        assert_eq!(
            radius, 0.25,
            "radius floor prevents framing distance collapsing to 0"
        );
    }

    #[test]
    fn orientation_axes_project_and_pick() {
        let camera = camera();
        let frame = camera_frame(&camera);
        let center = (100.0, 100.0);
        let x_end = orientation_axis_end(&frame, 0, 1.0, center, 40.0);
        assert!(x_end.0 > center.0);
        assert_eq!(
            pick_orientation_axis(x_end, center, 40.0, &frame),
            Some((0, 1.0))
        );
        assert_eq!(
            pick_orientation_axis((center.0, center.1), center, 40.0, &frame),
            None
        );
    }

    #[test]
    fn scaled_boxes_report_world_space_distance() {
        let origin = [0.0f32, 0.0, 5.0];
        let direction = [0.0f32, 0.0, -1.0];
        for scale in [1.0f32, 2.0, 4.0] {
            let entity = TransformJson {
                position: [0.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [scale; 3],
            };
            let distance = ray_obb(origin, direction, &entity).expect("center ray must hit");
            let expected = 5.0 - 0.5 * scale;
            assert!(
                (distance - expected).abs() < 1e-4,
                "scale {scale}: distance {distance} != {expected}"
            );
        }
    }

    #[test]
    fn pick_prefers_a_near_scaled_box_over_a_far_unit_box() {
        let camera = camera();
        let frame = camera_frame(&camera);
        let entities = vec![
            (
                0,
                TransformJson {
                    position: [0.0, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [4.0, 4.0, 4.0],
                },
            ),
            (
                1,
                TransformJson {
                    position: [0.0, 0.0, -10.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0],
                },
            ),
        ];
        assert_eq!(
            pick_entity((400.0, 300.0), &frame, 800.0, 600.0, &entities),
            Some(0),
            "near scaled entity should win, not be pushed back by enlarged distance"
        );
    }

    #[test]
    fn point_segment_distance_is_clamped() {
        assert!((distance_point_segment((0.0, 5.0), (0.0, 0.0), (10.0, 0.0)) - 5.0).abs() < 1e-4);
        assert!((distance_point_segment((-5.0, 0.0), (0.0, 0.0), (10.0, 0.0)) - 5.0).abs() < 1e-4);
        assert!(distance_point_segment((5.0, 0.0), (0.0, 0.0), (10.0, 0.0)) < 1e-4);
    }

    #[test]
    fn camera_frame_stays_orthonormal_at_poles() {
        for sign in [1.0f32, -1.0f32] {
            let mut camera = OrbitCamera::default();
            camera.align_to_axis(1, sign);
            let frame = camera_frame(&camera);
            let right = Vec3::from(frame.right);
            let up = Vec3::from(frame.up);
            let forward = Vec3::from(frame.forward);
            assert!(right.is_finite() && (right.length() - 1.0).abs() < 1e-4);
            assert!(up.is_finite() && (up.length() - 1.0).abs() < 1e-4);
            assert!(forward.is_finite() && (forward.length() - 1.0).abs() < 1e-4);
            assert!(right.dot(up).abs() < 1e-4);
            assert!(right.dot(forward).abs() < 1e-4);
            assert!(up.dot(forward).abs() < 1e-4);
            assert!((forward.y + sign).abs() < 1e-4);
        }
    }

    #[test]
    fn gizmo_world_length_at_tracks_distance_and_fov() {
        let mut cam = camera();
        cam.fov_y_degrees = 90.0;
        let len_90 = gizmo_world_length_at(&cam, [0.0, 0.0, 0.0], 600.0);
        cam.fov_y_degrees = 60.0;
        let len_60 = gizmo_world_length_at(&cam, [0.0, 0.0, 0.0], 600.0);
        assert!(len_90 > len_60 * 1.5);
        let closer = gizmo_world_length_at(&cam, [0.0, 0.0, 5.0], 600.0);
        assert!((closer - len_60 * 0.5).abs() < 1e-3);
    }

    #[test]
    fn near_plane_clipping_keeps_partial_segments() {
        let cam = camera();
        let frame = camera_frame(&cam);
        let seg = project_clipped_segment([0.0, 0.0, 12.0], [2.0, 0.0, 0.0], &frame, 800.0, 600.0);
        assert!(seg.is_some());
        let behind =
            project_clipped_segment([0.0, 0.0, 12.0], [1.0, 0.0, 13.0], &frame, 800.0, 600.0);
        assert!(behind.is_none());
    }

    #[test]
    fn planar_and_center_gizmo_pick_handles_3d_and_2d() {
        let cam = camera();
        let frame = camera_frame(&cam);
        let origin = [0.0, 0.0, 0.0];
        let world_length = gizmo_world_length_at(&cam, origin, 600.0);
        let center_px = project(origin, &frame, 800.0, 600.0).unwrap();
        assert_eq!(
            pick_gizmo_handle_oriented(
                (center_px.0, center_px.1),
                &frame,
                origin,
                IDENTITY_BASIS,
                world_length,
                false,
                800.0,
                600.0,
            )
            .map(|(h, _, _)| h),
            Some(GizmoHandle::Center)
        );
        let corners = plane_quad_corners(origin, IDENTITY_BASIS, 0, 1, world_length);
        let mid = [
            (corners[0][0] + corners[2][0]) * 0.5,
            (corners[0][1] + corners[2][1]) * 0.5,
            (corners[0][2] + corners[2][2]) * 0.5,
        ];
        let p = project(mid, &frame, 800.0, 600.0).unwrap();
        assert_eq!(
            pick_gizmo_handle_oriented(
                (p.0, p.1),
                &frame,
                origin,
                IDENTITY_BASIS,
                world_length,
                true,
                800.0,
                600.0,
            )
            .map(|(h, _, _)| h),
            Some(GizmoHandle::Plane(0, 1))
        );
    }

    #[test]
    fn ray_axis_and_plane_intersections_and_signed_angle() {
        let cam = camera();
        let frame = camera_frame(&cam);
        let ray = ray_from_screen(400.0, 300.0, &frame, 800.0, 600.0);
        let hit =
            ray_intersect_plane(ray.0, ray.1, [0.0, 0.0, 0.0], [0.0, 0.0, 1.0], false).unwrap();
        assert!(hit[0].abs() < 1e-3 && hit[1].abs() < 1e-3 && hit[2].abs() < 1e-3);
        let right_ray = ray_from_screen(500.0, 300.0, &frame, 800.0, 600.0);
        let t_x =
            ray_closest_point_on_axis(right_ray.0, right_ray.1, [0.0, 0.0, 0.0], [1.0, 0.0, 0.0])
                .unwrap();
        assert!(t_x > 0.0);
        let angle = signed_angle_around_axis([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
        assert!((angle - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
    }
}
