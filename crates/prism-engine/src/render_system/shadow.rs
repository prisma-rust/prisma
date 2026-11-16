//! Directional light and cascaded shadow map matrix math helpers.

use glam::{Mat4, Vec3, Vec4};

use super::DEFAULT_SHADOW_DISTANCE;

// ---------------------------------------------------------------------------
// Euler angle ↔ direction helpers
// ---------------------------------------------------------------------------

/// Convert XYZ Euler angles (degrees) to a unit direction vector (direction
/// TO the light), in world space.
pub fn euler_xyz_deg_to_dir(e: Vec3) -> Vec3 {
    let p = e.x.to_radians();
    let y = e.y.to_radians();
    let r = e.z.to_radians();
    let (sp, cp) = p.sin_cos();
    let (sy, cy) = y.sin_cos();
    let _ = r;
    Vec3::new(cp * sy, sp, cp * cy).normalize_or_zero()
}

/// Inverse of [`euler_xyz_deg_to_dir`]: derive XYZ Euler angles (degrees) from
/// a direction vector.
pub fn dir_to_euler_xyz_deg(d: Vec3) -> Vec3 {
    let n = d.normalize_or_zero();
    Vec3::new(n.y.asin().to_degrees(), n.x.atan2(n.z).to_degrees(), 0.0)
}

// ---------------------------------------------------------------------------
// Matrix helpers
// ---------------------------------------------------------------------------

/// 方向光从「光传播方向」（`transform.forward`）转换为
/// `FrameUBO.lightDirection` 的「指向光源」约定（见 `common.slang`）。
///
/// `LightSample.direction` 与光学的 `transform.forward` 一致；着色器和阴影相机都要求
/// 指向光源的方向，所以方向光在写入 UBO 时取反。点光 / 聚光的 `direction` 是锥轴传播
/// 方向，不走这条通路。
pub(super) fn directional_light_ubo_direction(direction: Vec3, radiance: f32) -> Vec4 {
    Vec4::new(-direction.x, -direction.y, -direction.z, radiance)
}

/// The eight world-space corners of the main camera's view frustum, but with the
/// far plane clamped to `max_depth` (camera-forward distance).
///
/// Unprojecting `z_ndc` for a given view depth `d` uses the projection matrix
/// directly: with `clip.w = -z_view = d` and `clip.z = p22 * z_view + p32`,
/// `z_ndc = (p32 - p22 * d) / d`. Orthographic cameras have `clip.w = 1`, so
/// `z_ndc = p22 * d + p32` there instead.
pub(super) fn camera_frustum_corners(
    inv_view_proj: &Mat4,
    proj22: f32,
    proj32: f32,
    orthographic: bool,
    near: f32,
    max_depth: f32,
) -> [Vec3; 8] {
    let depth_of = |d: f32| {
        if orthographic {
            proj22 * d + proj32
        } else {
            (proj32 - proj22 * d) / d.max(1e-6)
        }
    };
    let far = max_depth.max(near + 1e-3);
    let mut corners = [Vec3::ZERO; 8];
    let mut slot = 0;
    for depth in [near, far] {
        let z = depth_of(depth);
        for (x, y) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let clip = glam::Vec4::new(x, y, z, 1.0);
            let world = *inv_view_proj * clip;
            corners[slot] = world.truncate() / world.w;
            slot += 1;
        }
    }
    corners
}

/// Fit an orthographic light-space box to `corners` and return its matrix.
///
/// Coverage follows the **main camera frustum**, not the whole world: the box is
/// the frustum's AABB in light space, so scenery the camera cannot see never
/// widens the map. The light-space X/Y extent is squared off (equal half-extent
/// on both axes) and the centre is snapped to the texel grid, so camera motion
/// slides the map by whole texels instead of resampling every frame.
///
/// The depth axis covers `[s_near - extent, s_far]`, where `s = fwd·p` measured
/// away from the light: `s_far` stops at the farthest receiver in view, and the
/// near margin is one box extent so casters up to that far upstream still make
/// it into the map. Anything past that cannot reach the visible region.
pub(super) fn shadow_matrix_fitting(light_dir: &Vec4, corners: &[Vec3], texels: u32) -> Mat4 {
    let l = Vec3::new(light_dir.x, light_dir.y, light_dir.z).normalize_or_zero();
    let fwd = (-l).normalize_or_zero();
    let up = if l.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let right = fwd.cross(up).normalize_or_zero();
    let true_up = right.cross(fwd);

    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut min_s = f32::INFINITY;
    let mut max_s = f32::NEG_INFINITY;
    for corner in corners {
        let x = right.dot(*corner);
        let y = true_up.dot(*corner);
        let s = fwd.dot(*corner);
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
        min_s = min_s.min(s);
        max_s = max_s.max(s);
    }
    if !min_x.is_finite() || !max_x.is_finite() || (max_s - min_s) <= 1e-6 {
        return Mat4::IDENTITY;
    }

    // Square off the X/Y extent so texels stay square, then snap the centre to
    // the texel grid (depth stays continuous so casters are not quantised).
    let mut half = (0.5 * (max_x - min_x)).max(0.5 * (max_y - min_y)).max(1e-3);
    let mut center_x = 0.5 * (min_x + max_x);
    let mut center_y = 0.5 * (min_y + max_y);
    if texels > 1 {
        // Snapping can move the centre by up to half a texel, so grow the box by
        // one texel afterwards: coverage must stay conservative.
        let texel = (2.0 * half) / texels as f32;
        center_x = (center_x / texel).round() * texel;
        center_y = (center_y / texel).round() * texel;
        half += texel;
    }

    // Depth: near margin of one box extent covers upstream casters that can
    // still throw a shadow into view; far stops at the farthest receiver.
    let near_s = min_s - (max_s - min_s);
    let span = (max_s - near_s).max(1e-3);
    let center = right * center_x + true_up * center_y + fwd * (0.5 * (near_s + max_s));
    // `fwd·eye = near_s` puts view-space z = 0 on the near plane.
    let eye = center + l * (fwd.dot(center) - near_s);

    let view = Mat4::from_cols(
        glam::vec4(right.x, true_up.x, -fwd.x, 0.0),
        glam::vec4(right.y, true_up.y, -fwd.y, 0.0),
        glam::vec4(right.z, true_up.z, -fwd.z, 0.0),
        glam::vec4(-right.dot(eye), -true_up.dot(eye), fwd.dot(eye), 1.0),
    );
    let projection = Mat4::from_cols(
        glam::vec4(half.recip(), 0.0, 0.0, 0.0),
        glam::vec4(0.0, half.recip(), 0.0, 0.0),
        glam::vec4(0.0, 0.0, -span.recip(), 0.0),
        glam::vec4(0.0, 0.0, 0.0, 1.0),
    );
    // clip = projection * view * world (column-vector convention).
    projection * view
}

/// Build the light-space matrices for one or two cascades.
///
/// `camera_front` is the near plane depth and `distance` caps how far shadows
/// reach; the corners come from the main camera's frustum, so the map covers
/// exactly what the player can see rather than a fixed box in front of the eye.
#[allow(clippy::too_many_arguments)]
pub(super) fn shadow_cascade_matrices(
    light_dir: &Vec4,
    inv_view_proj: &Mat4,
    view: &Mat4,
    proj22: f32,
    proj32: f32,
    orthographic: bool,
    camera_near: f32,
    distance: f32,
    count: u32,
) -> (Mat4, Mat4, f32) {
    let texels = prism_render::SHADOW_MAP_SIZE;
    let distance = if distance.is_finite() && distance > 0.0 {
        distance
    } else {
        DEFAULT_SHADOW_DISTANCE
    };
    let near = camera_near.clamp(0.001, distance * 0.5);

    if count <= 1 {
        let corners =
            camera_frustum_corners(inv_view_proj, proj22, proj32, orthographic, near, distance);
        let matrix = shadow_matrix_fitting(light_dir, &corners, texels);
        return (matrix, matrix, distance * 0.5);
    }

    // Two equal slabs along the camera axis: [near, d/2] and [d/2, d]. Each
    // cascade fits the frustum corners inside its own slab.
    let split = distance * 0.5;
    let near_corners =
        camera_frustum_corners(inv_view_proj, proj22, proj32, orthographic, near, split);
    let far_corners =
        camera_frustum_corners(inv_view_proj, proj22, proj32, orthographic, split, distance);
    // `view` is only read through the callers' depth split; keep it explicit so a
    // future change to the split metric cannot silently disagree with the shader.
    let _ = view;
    (
        shadow_matrix_fitting(light_dir, &near_corners, texels),
        shadow_matrix_fitting(light_dir, &far_corners, texels),
        split,
    )
}

#[allow(dead_code)]
fn norm3(a: [f32; 3]) -> [f32; 3] {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).max(1e-8).sqrt();
    [a[0] / l, a[1] / l, a[2] / l]
}

