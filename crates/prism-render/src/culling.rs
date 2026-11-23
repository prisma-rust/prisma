//! Bounding box and frustum culling (pure math, no Vulkan handles, no glam dependency).
//!
//! During extraction phase (`prism-engine`'s `scene_render_system`), before submitting draw items do one
//! conservative cull: entity outside **main camera frustum** not submitted for whole frame. This is first step suggested in #491 —
//! previously each entity drew every frame, half of them behind camera.
//!
//! Conventions:
//!
//! - Coordinates and matrices follow `docs/DESIGN.md`: right-handed, camera looks −Z, column-major
//!   `mat[col][row]`, `clip = projection * view * model`, Vulkan depth range
//!   **[0, 1]**. Near plane therefore `z_clip >= 0` (row 2), far plane is
//!   `w - z >= 0` (row 3 − row 2), not OpenGL's `w + z`.
//! - Culling is **conservative**: keep if bounding box and frustum have any overlap. Rather draw one extra than
//!   cut half an object.
//! - No bounding box (procedural mesh, handle not yet parsed) or non-finite matrix: **do not cull**: keep
//!   old behavior when data unknown, better than objects disappearing into thin air.

/// Axis-aligned bounding box (local or world space, caller decides).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Aabb {
    pub fn new(min: [f32; 3], max: [f32; 3]) -> Self {
        Self { min, max }
    }

    /// Compute bounding box from vertex positions; empty set or non-finite coords returns `None`.
    pub fn from_positions<I: IntoIterator<Item = [f32; 3]>>(positions: I) -> Option<Self> {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        let mut any = false;
        for position in positions {
            for axis in 0..3 {
                let value = position[axis];
                if !value.is_finite() {
                    return None;
                }
                min[axis] = min[axis].min(value);
                max[axis] = max[axis].max(value);
            }
            any = true;
        }
        any.then_some(Self { min, max })
    }

    /// Union of two bounding boxes (component-wise min/max). If either invalid returns the other.
    ///
    /// Skinned mesh uses this to combine "bind bounding box after each bone transform" into one conservative box
    /// (#560 P0-3c): post-skin vertices are convex hull of these boxes, so union won't miss any.
    pub fn union(&self, other: &Self) -> Option<Self> {
        match (self.is_valid(), other.is_valid()) {
            (true, true) => {
                let mut bounds = *self;
                for axis in 0..3 {
                    bounds.min[axis] = self.min[axis].min(other.min[axis]);
                    bounds.max[axis] = self.max[axis].max(other.max[axis]);
                }
                Some(bounds)
            }
            (true, false) => Some(*self),
            (false, true) => Some(*other),
            (false, false) => None,
        }
    }

    /// `min <= max` and components finite. Empty / malformed boxes do not participate in culling.
    pub fn is_valid(&self) -> bool {
        (0..3).all(|axis| {
            let (min, max) = (self.min[axis], self.max[axis]);
            min.is_finite() && max.is_finite() && min <= max
        })
    }

    pub fn center(&self) -> [f32; 3] {
        [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[1] + self.max[1]) * 0.5,
            (self.min[2] + self.max[2]) * 0.5,
        ]
    }

    /// Bounding sphere radius (half diagonal length).
    pub fn radius(&self) -> f32 {
        let dx = (self.max[0] - self.min[0]) * 0.5;
        let dy = (self.max[1] - self.min[1]) * 0.5;
        let dz = (self.max[2] - self.min[2]) * 0.5;
        (dx * dx + dy * dy + dz * dz).sqrt()
    }

    /// 8 corner points (fixed order, tests and picking depend on this order).
    pub fn corners(&self) -> [[f32; 3]; 8] {
        let [x0, y0, z0] = self.min;
        let [x1, y1, z1] = self.max;
        [
            [x0, y0, z0],
            [x1, y0, z0],
            [x0, y1, z0],
            [x1, y1, z0],
            [x0, y0, z1],
            [x1, y0, z1],
            [x0, y1, z1],
            [x1, y1, z1],
        ]
    }

    /// Transform into world-space AABB with model matrix (column-major `mat[col][row]`).
    ///
    /// Non-finite transform or invalid box returns `None` — caller then gives up culling.
    pub fn transformed(&self, model: &[[f32; 4]; 4]) -> Option<Self> {
        if !self.is_valid() {
            return None;
        }
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for corner in self.corners() {
            let world = transform_point3(model, corner);
            for axis in 0..3 {
                let value = world[axis];
                if !value.is_finite() {
                    return None;
                }
                min[axis] = min[axis].min(value);
                max[axis] = max[axis].max(value);
            }
        }
        Some(Self { min, max })
    }
}

/// Transform a point with column-major `mat[col][row]` (`w = 1`, no perspective divide).
fn transform_point3(model: &[[f32; 4]; 4], point: [f32; 3]) -> [f32; 3] {
    let [x, y, z] = point;
    [
        model[0][0] * x + model[1][0] * y + model[2][0] * z + model[3][0],
        model[0][1] * x + model[1][1] * y + model[2][1] * z + model[3][1],
        model[0][2] * x + model[1][2] * y + model[2][2] * z + model[3][2],
    ]
}

/// 6 frustum planes (`[nx, ny, nz, d]`: unit normal + distance).
///
/// Plane order doesn't matter much (only "whether entirely outside one side" used), but keep
/// left / right / bottom / top / near / far for easier debugging.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frustum {
    planes: [[f32; 4]; 6],
}

impl Frustum {
    /// Extract frustum planes from `view_proj` (column-major).
    ///
    /// Degenerate matrix (all zero / non-finite / normal length 0, e.g. unit matrix) returns `None`:
    /// any "culling" from such input is blind clipping, caller should skip culling.
    pub fn from_view_proj(view_proj: &[[f32; 4]; 4]) -> Option<Self> {
        let row = |index: usize| {
            [
                view_proj[0][index],
                view_proj[1][index],
                view_proj[2][index],
                view_proj[3][index],
            ]
        };
        let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
        let raw = [
            add4(r3, r0), // left:   w + x >= 0
            sub4(r3, r0), // right:  w - x >= 0
            add4(r3, r1), // bottom: w + y >= 0
            sub4(r3, r1), // top:    w - y >= 0
            r2,           // near:   z >= 0 (Vulkan depth range [0, 1])
            sub4(r3, r2), // far:    w - z >= 0
        ];
        let mut planes = [[0.0f32; 4]; 6];
        for (slot, plane) in planes.iter_mut().zip(raw) {
            let length = (plane[0] * plane[0] + plane[1] * plane[1] + plane[2] * plane[2]).sqrt();
            if !length.is_finite() || length < 1e-6 {
                return None;
            }
            for axis in 0..4 {
                slot[axis] = plane[axis] / length;
            }
        }
        Some(Self { planes })
    }

    /// Whether bounding box intersects frustum (conservative: touching, containing, partial overlap all `true`).
    ///
    /// Invalid box always returns `true` (no cull).
    pub fn intersects_aabb(&self, aabb: &Aabb) -> bool {
        if !aabb.is_valid() {
            return true;
        }
        for plane in self.planes {
            // Take box corner "farthest" along this plane's normal (p-vertex):
            // if it is on outside side, no visible part inside box.
            let positive = [
                if plane[0] > 0.0 {
                    aabb.max[0]
                } else {
                    aabb.min[0]
                },
                if plane[1] > 0.0 {
                    aabb.max[1]
                } else {
                    aabb.min[1]
                },
                if plane[2] > 0.0 {
                    aabb.max[2]
                } else {
                    aabb.min[2]
                },
            ];
            let distance =
                plane[0] * positive[0] + plane[1] * positive[1] + plane[2] * positive[2] + plane[3];
            if distance < 0.0 {
                return false;
            }
        }
        true
    }

    /// 6 planes (for debug / tests).
    pub fn planes(&self) -> &[[f32; 4]; 6] {
        &self.planes
    }
}

/// Screen-space coarse depth ("conservative occlusion culling" first-level depth cache).
///
/// Each `tile_size × tile_size` screen tile stores **max** depth inside tile (farthest fragment),
/// background keeps clear depth `1.0`. Then criterion "whole box occluded" can be written:
/// every tile covered by box projection rect satisfies `tile_max < box nearest point depth` — as long as tile
/// still has background (1.0) or farther surface, no false culling.
///
/// Depth comes from **previous frame**: renderer async reads back main channel depth map, downsamples then hands here.
/// One-frame delay only makes fast-moving objects occasionally culled a frame late, no correctness impact.
#[derive(Debug, Clone, PartialEq)]
pub struct DepthTiles {
    width: u32,
    height: u32,
    tile_size: u32,
    /// Row-major, length `width * height`.
    max_depth: Vec<f32>,
}

impl DepthTiles {
    /// Build coarse depth from row-major `D32_SFLOAT` depth map.
    ///
    /// `samples.len()` must be at least `extent[0] * extent[1]`; non-finite depths treated as
    /// `1.0` (farthest), prefer fewer culls. Returns `None` on invalid parameters.
    pub fn from_samples(extent: [u32; 2], tile_size: u32, samples: &[f32]) -> Option<Self> {
        let (width, height) = (extent[0], extent[1]);
        if width == 0 || height == 0 || tile_size == 0 {
            return None;
        }
        if (samples.len() as u64) < u64::from(width) * u64::from(height) {
            return None;
        }
        let tiles_x = width.div_ceil(tile_size);
        let tiles_y = height.div_ceil(tile_size);
        let mut max_depth = vec![f32::NEG_INFINITY; (tiles_x * tiles_y) as usize];
        for y in 0..height {
            let tile_row = (y / tile_size) * tiles_x;
            let row = (y * width) as usize;
            for x in 0..width {
                let depth = samples[row + x as usize];
                if !depth.is_finite() {
                    continue;
                }
                let slot = &mut max_depth[(tile_row + x / tile_size) as usize];
                if depth > *slot {
                    *slot = depth;
                }
            }
        }
        // Tiles with no finite sample keep clear depth (farthest), no false culling.
        for depth in &mut max_depth {
            if !depth.is_finite() {
                *depth = 1.0;
            }
        }
        Some(Self {
            width: tiles_x,
            height: tiles_y,
            tile_size,
            max_depth,
        })
    }

    pub fn tiles(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn tile_size(&self) -> u32 {
        self.tile_size
    }

    /// Whether bounding box is **fully** occluded.
    ///
    /// Criterion conservative: only when all 8 corners inside screen, depths in `[0, 1]`, and every depth tile
    /// covered by projection rect strictly nearer than box's nearest point return `true`. Any corner
    /// outside screen, box crossing near plane, or tile with background stays visible.
    ///
    /// `view_proj` must be matrix **from the frame that generated this depth map**; `margin_tiles` expands
    /// outward one ring to offset edge error from one-frame delay.
    pub fn is_box_occluded(
        &self,
        view_proj: &[[f32; 4]; 4],
        aabb: &Aabb,
        margin_tiles: u32,
    ) -> bool {
        if !aabb.is_valid() || self.max_depth.is_empty() {
            return false;
        }
        const BIAS: f32 = 1e-4;
        let mut nearest = f32::INFINITY;
        let (mut min_uv, mut max_uv) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        for corner in aabb.corners() {
            let [x, y, z] = corner;
            let clip = [
                view_proj[0][0] * x + view_proj[1][0] * y + view_proj[2][0] * z + view_proj[3][0],
                view_proj[0][1] * x + view_proj[1][1] * y + view_proj[2][1] * z + view_proj[3][1],
                view_proj[0][2] * x + view_proj[1][2] * y + view_proj[2][2] * z + view_proj[3][2],
                view_proj[0][3] * x + view_proj[1][3] * y + view_proj[2][3] * z + view_proj[3][3],
            ];
            // Corner on or behind camera plane: no reliable screen projection, conservatively keep.
            if !clip[3].is_finite() || clip[3] <= 1e-6 {
                return false;
            }
            let inv_w = 1.0 / clip[3];
            let ndc = [clip[0] * inv_w, clip[1] * inv_w, clip[2] * inv_w];
            if !ndc.iter().all(|value| value.is_finite()) {
                return false;
            }
            if !(0.0..=1.0).contains(&ndc[2]) {
                return false;
            }
            let uv = [ndc[0] * 0.5 + 0.5, ndc[1] * 0.5 + 0.5];
            if uv.iter().any(|value| !(-0.001..=1.001).contains(value)) {
                return false;
            }
            let uv = [uv[0].clamp(0.0, 1.0), uv[1].clamp(0.0, 1.0)];
            min_uv = [min_uv[0].min(uv[0]), min_uv[1].min(uv[1])];
            max_uv = [max_uv[0].max(uv[0]), max_uv[1].max(uv[1])];
            nearest = nearest.min(ndc[2]);
        }
        if !nearest.is_finite() {
            return false;
        }
        let tile = |uv: f32, tiles: u32| -> u32 {
            ((uv * tiles as f32).floor() as u32).min(tiles.saturating_sub(1))
        };
        let x0 = tile(min_uv[0], self.width).saturating_sub(margin_tiles);
        let y0 = tile(min_uv[1], self.height).saturating_sub(margin_tiles);
        let x1 = tile(max_uv[0], self.width)
            .saturating_add(margin_tiles)
            .min(self.width - 1);
        let y1 = tile(max_uv[1], self.height)
            .saturating_add(margin_tiles)
            .min(self.height - 1);
        let mut farthest = 0.0f32;
        for y in y0..=y1 {
            for x in x0..=x1 {
                farthest = farthest.max(self.max_depth[(y * self.width + x) as usize]);
            }
        }
        farthest < nearest - BIAS
    }
}

/// Occlusion culling result of one frame (for report / tests).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct OcclusionCounts {
    /// Number of draw items tested (those without world bounding box not counted).
    pub tested: usize,
    /// Count judged fully occluded and no longer submitted in main channel.
    pub culled: usize,
}

/// Flag draw items judged fully occluded by previous-frame coarse depth as "not visible in main channel".
///
/// Only changes `visible_in_main_pass`: shadow channel does not read this field, so occluded objects still
/// cast visible shadows (correct — occlusion relation only holds for current camera).
pub fn mark_occluded_draws(
    items: &mut [crate::render_graph::DrawItem],
    tiles: &DepthTiles,
    view_proj: &[[f32; 4]; 4],
    margin_tiles: u32,
) -> OcclusionCounts {
    let mut counts = OcclusionCounts::default();
    for item in items {
        if !item.visible_in_main_pass {
            continue;
        }
        let Some(bounds) = item.world_bounds.as_ref() else {
            continue;
        };
        counts.tested += 1;
        if tiles.is_box_occluded(view_proj, bounds, margin_tiles) {
            item.visible_in_main_pass = false;
            counts.culled += 1;
        }
    }
    counts
}

fn add4(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]]
}

fn sub4(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── hand-written small matrix utils (this crate no glam dependency) ─────────────────────────

    fn mul4(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
        let mut out = [[0.0f32; 4]; 4];
        for col in 0..4 {
            for row in 0..4 {
                out[col][row] = (0..4).map(|k| a[k][row] * b[col][k]).sum();
            }
        }
        out
    }

    fn translation(x: f32, y: f32, z: f32) -> [[f32; 4]; 4] {
        let mut m = identity();
        m[3] = [x, y, z, 1.0];
        m
    }

    fn scale(s: f32) -> [[f32; 4]; 4] {
        let mut m = identity();
        m[0][0] = s;
        m[1][1] = s;
        m[2][2] = s;
        m
    }

    fn identity() -> [[f32; 4]; 4] {
        let mut m = [[0.0f32; 4]; 4];
        for (axis, column) in m.iter_mut().enumerate() {
            column[axis] = 1.0;
        }
        m
    }

    /// Vulkan-style perspective (depth 0..1, y flipped downward), fov 45°, near 0.1 / far 100.
    fn perspective() -> [[f32; 4]; 4] {
        let fov = 45f32.to_radians();
        let (near, far) = (0.1f32, 100.0f32);
        let f = 1.0 / (fov * 0.5).tan();
        let mut m = [[0.0f32; 4]; 4];
        m[0][0] = f;
        m[1][1] = -f; // Vulkan y-flip
        m[2][2] = far / (near - far);
        m[2][3] = -1.0;
        m[3][2] = (near * far) / (near - far);
        m
    }

    /// `view_proj` of camera at origin looking down −Z.
    fn camera() -> Frustum {
        Frustum::from_view_proj(&perspective()).expect("camera frustum must be valid")
    }

    fn unit_box_at(x: f32, y: f32, z: f32) -> Aabb {
        Aabb::new([x - 0.5, y - 0.5, z - 0.5], [x + 0.5, y + 0.5, z + 0.5])
    }

    #[test]
    fn a_box_in_front_of_the_camera_is_kept() {
        assert!(camera().intersects_aabb(&unit_box_at(0.0, 0.0, -5.0)));
    }

    #[test]
    fn a_box_behind_the_camera_is_culled() {
        assert!(!camera().intersects_aabb(&unit_box_at(0.0, 0.0, 5.0)));
    }

    #[test]
    fn a_box_far_to_the_side_is_culled() {
        assert!(!camera().intersects_aabb(&unit_box_at(100.0, 0.0, -5.0)));
    }

    #[test]
    fn a_box_past_the_far_plane_is_culled() {
        assert!(!camera().intersects_aabb(&unit_box_at(0.0, 0.0, -200.0)));
    }

    #[test]
    fn a_box_containing_the_camera_is_kept() {
        assert!(camera().intersects_aabb(&unit_box_at(0.0, 0.0, 0.0)));
    }

    #[test]
    fn a_box_straddling_the_left_plane_is_kept() {
        // At z = −5 half image width ≈ 2.07 (45° fov), this box half outside.
        assert!(camera().intersects_aabb(&Aabb::new([-2.5, -0.2, -5.5], [-2.0, 0.2, -4.5],)));
    }

    #[test]
    fn an_invalid_box_is_never_culled() {
        let inverted = Aabb::new([1.0; 3], [0.0; 3]);
        assert!(!inverted.is_valid());
        assert!(camera().intersects_aabb(&inverted));

        let nan = Aabb::new([f32::NAN; 3], [1.0; 3]);
        assert!(camera().intersects_aabb(&nan));
    }

    #[test]
    fn a_degenerate_matrix_produces_no_frustum() {
        // All-zero / zero-length-after-normalize matrix: no usable planes.
        assert!(Frustum::from_view_proj(&[[0.0; 4]; 4]).is_none());
        assert!(Frustum::from_view_proj(&scale(0.0)).is_none());
    }

    #[test]
    fn the_planes_are_normalised() {
        for plane in camera().planes() {
            let length = (plane[0] * plane[0] + plane[1] * plane[1] + plane[2] * plane[2]).sqrt();
            assert!(
                (length - 1.0).abs() < 1e-5,
                "plane {plane:?} is not unit length"
            );
        }
    }

    #[test]
    fn a_box_spanning_the_camera_plane_is_kept() {
        // z from +1 to −1: has part behind camera and part outside near plane.
        let straddling = Aabb::new([-0.1, -0.1, -1.0], [0.1, 0.1, 1.0]);
        assert!(camera().intersects_aabb(&straddling));
        // Same box pushed entirely behind camera (z 1..2) becomes invisible.
        let behind = Aabb::new([-0.1, -0.1, 1.0], [0.1, 0.1, 2.0]);
        assert!(!camera().intersects_aabb(&behind));
    }

    #[test]
    fn transforming_a_box_keeps_its_world_extent() {
        let local = Aabb::new([0.0, 0.0, 0.0], [1.0, 2.0, 3.0]);
        let world = local.transformed(&translation(4.0, 5.0, 6.0)).unwrap();
        assert_eq!(world.min, [4.0, 5.0, 6.0]);
        assert_eq!(world.max, [5.0, 7.0, 9.0]);
    }

    #[test]
    fn a_non_finite_transform_has_no_world_box() {
        let local = Aabb::new([0.0; 3], [1.0; 3]);
        assert!(local
            .transformed(&translation(f32::NAN, 0.0, 0.0))
            .is_none());
    }

    #[test]
    fn a_moved_camera_keeps_its_own_box_visible() {
        // After camera translated to (0, 0, -10) looking −Z: (0,0,-15) inside frustum, (0,0,-5) behind.
        let view_proj = mul4(&perspective(), &translation(0.0, 0.0, 10.0));
        let frustum = Frustum::from_view_proj(&view_proj).unwrap();
        assert!(frustum.intersects_aabb(&unit_box_at(0.0, 0.0, -15.0)));
        assert!(!frustum.intersects_aabb(&unit_box_at(0.0, 0.0, -5.0)));
    }

    #[test]
    fn union_wraps_both_boxes_and_ignores_broken_ones() {
        let left = Aabb::new([-1.0, 0.0, -1.0], [0.0, 1.0, 1.0]);
        let right = Aabb::new([0.5, -2.0, -1.0], [2.0, 0.0, 1.0]);
        let merged = left.union(&right).expect("both valid");
        assert_eq!(merged.min, [-1.0, -2.0, -1.0]);
        assert_eq!(merged.max, [2.0, 1.0, 1.0]);
        // Invalid one ignored; both invalid then no result.
        let broken = Aabb::new([f32::NAN, 0.0, 0.0], [1.0, 1.0, 1.0]);
        assert_eq!(left.union(&broken), Some(left));
        assert_eq!(broken.union(&broken), None);
    }

    fn uniform_tiles(depth: f32) -> DepthTiles {
        DepthTiles::from_samples([128, 128], 4, &vec![depth; 128 * 128]).expect("valid samples")
    }

    #[test]
    fn a_box_behind_a_full_screen_surface_is_occluded() {
        // Wall depth 0.99 (≈ z = −10), box at z = −50 (depth ≈ 0.999).
        let tiles = uniform_tiles(0.99);
        let camera = perspective();
        assert!(tiles.is_box_occluded(&camera, &unit_box_at(0.0, 0.0, -50.0), 1));
    }

    #[test]
    fn a_box_in_front_of_the_surface_stays_visible() {
        // Depth at z = −5 ≈ 0.981 < 0.99: box nearer than wall, must not cull.
        let tiles = uniform_tiles(0.99);
        let camera = perspective();
        assert!(!tiles.is_box_occluded(&camera, &unit_box_at(0.0, 0.0, -5.0), 1));
    }

    #[test]
    fn a_background_tile_inside_the_rect_keeps_the_box_visible() {
        // Right half is background (1.0): if box projection covers it there's no occluder there.
        let mut samples = vec![0.99f32; 128 * 128];
        for y in 0..128 {
            for x in 64..128 {
                samples[y * 128 + x] = 1.0;
            }
        }
        let tiles = DepthTiles::from_samples([128, 128], 4, &samples).unwrap();
        let camera = perspective();
        // Box at z = −50, x = 6: projection lands in right half.
        assert!(!tiles.is_box_occluded(&camera, &unit_box_at(6.0, 0.0, -50.0), 1));
    }

    #[test]
    fn boxes_touching_the_screen_edge_or_camera_plane_stay_visible() {
        let tiles = uniform_tiles(0.99);
        let camera = perspective();
        // Outside screen (x = 100) and crossing near plane (z from −0.05 to +0.05) both conservatively kept.
        assert!(!tiles.is_box_occluded(&camera, &unit_box_at(100.0, 0.0, -50.0), 1));
        assert!(!tiles.is_box_occluded(
            &camera,
            &Aabb::new([-0.1, -0.1, -0.05], [0.1, 0.1, 0.05]),
            1
        ));
        // Invalid box likewise not culled.
        assert!(!tiles.is_box_occluded(&camera, &Aabb::new([f32::NAN; 3], [1.0; 3]), 1));
    }

    #[test]
    fn marking_draws_only_touches_the_main_pass_flag() {
        use crate::render_graph::DrawItem;
        fn item(bounds: Option<Aabb>) -> DrawItem {
            DrawItem {
                mesh: Default::default(),
                entity_id: u32::MAX,
                model: identity(),
                material: None,
                cast_shadow: true,
                visible_in_main_pass: true,
                cast_shadow_two_sided: false,
                bone_offset: None,
                splat_slots: [u32::MAX; 4],
                splat_tiling: 0.0,
                shader: None,
                state: Default::default(),
                world_bounds: bounds,
                index_start: 0,
                index_count: None,
            }
        }
        let tiles = uniform_tiles(0.99);
        let hidden = item(Some(unit_box_at(0.0, 0.0, -50.0)));
        let visible = item(Some(unit_box_at(0.0, 0.0, -5.0)));
        let unknown = item(None);
        let mut items = vec![hidden, visible, unknown];
        let counts = mark_occluded_draws(&mut items, &tiles, &perspective(), 1);
        assert_eq!(
            counts,
            OcclusionCounts {
                tested: 2,
                culled: 1
            }
        );
        assert!(!items[0].visible_in_main_pass);
        assert!(items[1].visible_in_main_pass);
        assert!(items[2].visible_in_main_pass);
        // Shadow still decided by `cast_shadow`, culling doesn't touch it.
        assert!(items[0].cast_shadow);
    }

    #[test]
    fn a_depth_grid_rejects_bad_input() {
        assert!(DepthTiles::from_samples([0, 4], 4, &[]).is_none());
        assert!(DepthTiles::from_samples([4, 4], 0, &[0.0; 16]).is_none());
        assert!(DepthTiles::from_samples([4, 4], 4, &[0.0; 15]).is_none());
        let tiles = DepthTiles::from_samples([5, 3], 4, &[f32::NAN; 15]).unwrap();
        assert_eq!(tiles.tiles(), (2, 1));
        // Non-finite depth treated as farthest, never cull.
        assert!(!tiles.is_box_occluded(&perspective(), &unit_box_at(0.0, 0.0, -50.0), 0));
    }

    #[test]
    fn box_bounds_come_from_positions() {
        let bounds = Aabb::from_positions([[1.0, -2.0, 3.0], [-1.0, 2.0, 0.0], [0.5, 0.0, 6.0]])
            .expect("finite positions give bounds");
        assert_eq!(bounds.min, [-1.0, -2.0, 0.0]);
        assert_eq!(bounds.max, [1.0, 2.0, 6.0]);
        assert!(Aabb::from_positions([]).is_none());
        assert!(Aabb::from_positions([[f32::NAN, 0.0, 0.0]]).is_none());
    }
}
