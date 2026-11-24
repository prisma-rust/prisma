//! Deterministic 2D collision geometry for platform games.
//!
//! This crate is deliberately independent of ECS, rendering, and a rigid-body
//! backend. The XY plane uses +X right and +Y up; callers choose the world unit.

#![deny(warnings)]

const EPSILON: f32 = 1.0e-6;
const MAX_CONTACTS: usize = 4;
const MAX_DEPENETRATION_STEPS: usize = 8;

/// An axis-aligned rectangle in world space.
///
/// Touching edges do not overlap. `min` and `max` are expected to be ordered;
/// constructors normalize them so authoring data cannot silently invert a box.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Aabb2 {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

impl Aabb2 {
    pub const fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self {
            min: [if x0 < x1 { x0 } else { x1 }, if y0 < y1 { y0 } else { y1 }],
            max: [if x0 > x1 { x0 } else { x1 }, if y0 > y1 { y0 } else { y1 }],
        }
    }

    pub fn from_center_half_extents(center: [f32; 2], half_extents: [f32; 2]) -> Self {
        Self::new(
            center[0] - half_extents[0].abs(),
            center[1] - half_extents[1].abs(),
            center[0] + half_extents[0].abs(),
            center[1] + half_extents[1].abs(),
        )
    }

    pub fn overlaps(self, other: Self) -> bool {
        self.min[0] < other.max[0]
            && self.max[0] > other.min[0]
            && self.min[1] < other.max[1]
            && self.max[1] > other.min[1]
    }

    pub fn contains_point(self, point: [f32; 2]) -> bool {
        point[0] > self.min[0]
            && point[0] < self.max[0]
            && point[1] > self.min[1]
            && point[1] < self.max[1]
    }
}

/// The first hit of a 2D ray against an AABB.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RayHit2 {
    /// Distance from the origin in the caller's world units.
    pub distance: f32,
    pub point: [f32; 2],
    /// Outward face normal; zero when the ray starts inside the box.
    pub normal: [f32; 2],
}

/// Cast a normalized 2D ray against one AABB and return its first hit.
///
/// A ray starting inside the box hits at distance zero. Zero directions,
/// non-finite inputs, and non-positive maximum distances return `None`.
pub fn raycast_aabb(
    origin: [f32; 2],
    direction: [f32; 2],
    max_distance: f32,
    bounds: Aabb2,
) -> Option<RayHit2> {
    if !finite2(origin)
        || !finite2(direction)
        || !finite_aabb(bounds)
        || !max_distance.is_finite()
        || max_distance <= 0.0
    {
        return None;
    }
    let length = (direction[0] * direction[0] + direction[1] * direction[1]).sqrt();
    if !length.is_finite() || length <= EPSILON {
        return None;
    }
    let normalized = [direction[0] / length, direction[1] / length];
    if bounds.contains_point(origin) {
        return Some(RayHit2 {
            distance: 0.0,
            point: origin,
            normal: [0.0, 0.0],
        });
    }
    let delta = [normalized[0] * max_distance, normalized[1] * max_distance];
    let (toi, normal) = sweep_point(origin, delta, bounds)?;
    let distance = toi * max_distance;
    Some(RayHit2 {
        distance,
        point: [
            origin[0] + normalized[0] * distance,
            origin[1] + normalized[1] * distance,
        ],
        normal,
    })
}

/// A contact discovered while sliding an AABB.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Contact2 {
    /// Outward normal of the obstacle at the contact point.
    pub normal: [f32; 2],
    /// Fraction of the remaining move at which this contact occurred.
    pub toi: f32,
}

/// Result of [`move_and_slide`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveResult2 {
    /// Center position after the complete sweep and slide.
    pub position: [f32; 2],
    /// Difference between the returned position and the start position.
    pub applied_delta: [f32; 2],
    /// Contacts in sweep order. At most four are retained; flags remain valid
    /// even when a corner produces more contacts than the fixed storage.
    pub contacts: [Contact2; MAX_CONTACTS],
    pub contact_count: usize,
    pub hit_x: bool,
    pub hit_y: bool,
    pub grounded: bool,
    pub ceilinged: bool,
}

impl MoveResult2 {
    fn new(start: [f32; 2]) -> Self {
        Self {
            position: start,
            applied_delta: [0.0, 0.0],
            contacts: [Contact2::default(); MAX_CONTACTS],
            contact_count: 0,
            hit_x: false,
            hit_y: false,
            grounded: false,
            ceilinged: false,
        }
    }

    fn push_contact(&mut self, normal: [f32; 2], toi: f32) {
        if self.contact_count < MAX_CONTACTS {
            self.contacts[self.contact_count] = Contact2 { normal, toi };
            self.contact_count += 1;
        }
        if normal[0].abs() > 0.5 {
            self.hit_x = true;
        }
        if normal[1].abs() > 0.5 {
            self.hit_y = true;
        }
        if normal[1] > 0.5 {
            self.grounded = true;
        } else if normal[1] < -0.5 {
            self.ceilinged = true;
        }
    }
}

/// Continuously sweep an AABB against static AABB obstacles and slide along
/// their surfaces.
///
/// `start` is the center position, `half_extents` is the moving box's half size,
/// and `delta` is the requested displacement for one simulation step. The
/// For a non-zero displacement the returned position never enters an obstacle.
/// A zero displacement is a pure query and does not depenetrate an
/// already-overlapping box.
pub fn move_and_slide(
    start: [f32; 2],
    half_extents: [f32; 2],
    delta: [f32; 2],
    obstacles: &[Aabb2],
) -> MoveResult2 {
    let half_extents = [half_extents[0].abs(), half_extents[1].abs()];
    let mut result = MoveResult2::new(start);
    if !finite2(start) || !finite2(half_extents) || !finite2(delta) {
        return result;
    }
    if delta[0].abs() <= EPSILON && delta[1].abs() <= EPSILON {
        return result;
    }

    depenetrate(&mut result, half_extents, obstacles);
    let mut remaining = delta;
    for _ in 0..MAX_CONTACTS {
        if remaining[0].abs() <= EPSILON && remaining[1].abs() <= EPSILON {
            break;
        }

        let mut best: Option<(f32, [f32; 2])> = None;
        for obstacle in obstacles.iter().copied() {
            if !finite_aabb(obstacle) {
                continue;
            }
            let expanded = Aabb2::new(
                obstacle.min[0] - half_extents[0],
                obstacle.min[1] - half_extents[1],
                obstacle.max[0] + half_extents[0],
                obstacle.max[1] + half_extents[1],
            );
            if let Some((toi, normal)) = sweep_point(result.position, remaining, expanded) {
                if best.is_none_or(|(best_toi, _)| toi < best_toi) {
                    best = Some((toi, normal));
                }
            }
        }

        let Some((toi, normal)) = best else {
            result.position[0] += remaining[0];
            result.position[1] += remaining[1];
            break;
        };

        let toi = toi.clamp(0.0, 1.0);
        result.position[0] += remaining[0] * toi;
        result.position[1] += remaining[1] * toi;
        result.push_contact(normal, toi);

        let remainder_scale = 1.0 - toi;
        remaining[0] *= remainder_scale;
        remaining[1] *= remainder_scale;
        let into_surface = remaining[0] * normal[0] + remaining[1] * normal[1];
        if into_surface < 0.0 {
            remaining[0] -= normal[0] * into_surface;
            remaining[1] -= normal[1] * into_surface;
        }

        if toi <= EPSILON && remaining[0].abs() <= EPSILON && remaining[1].abs() <= EPSILON {
            break;
        }
    }

    result.applied_delta = [result.position[0] - start[0], result.position[1] - start[1]];
    result
}

fn depenetrate(result: &mut MoveResult2, half_extents: [f32; 2], obstacles: &[Aabb2]) {
    for _ in 0..MAX_DEPENETRATION_STEPS {
        let moving = Aabb2::from_center_half_extents(result.position, half_extents);
        let mut best: Option<(f32, [f32; 2])> = None;
        for obstacle in obstacles.iter().copied() {
            if !finite_aabb(obstacle) || !moving.overlaps(obstacle) {
                continue;
            }
            let candidates = [
                (moving.max[0] - obstacle.min[0], [-1.0, 0.0]),
                (obstacle.max[0] - moving.min[0], [1.0, 0.0]),
                (moving.max[1] - obstacle.min[1], [0.0, -1.0]),
                (obstacle.max[1] - moving.min[1], [0.0, 1.0]),
            ];
            let candidate = candidates
                .into_iter()
                .filter(|(depth, _)| *depth >= 0.0)
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some(candidate) = candidate {
                if best.is_none_or(|current| candidate.0 < current.0) {
                    best = Some(candidate);
                }
            }
        }

        let Some((depth, normal)) = best else {
            break;
        };
        let correction = depth + EPSILON * 2.0;
        result.position[0] += normal[0] * correction;
        result.position[1] += normal[1] * correction;
        result.push_contact(normal, 0.0);
    }
}

/// Return the first contact between a moving point and an AABB.
fn sweep_point(origin: [f32; 2], delta: [f32; 2], bounds: Aabb2) -> Option<(f32, [f32; 2])> {
    if bounds.contains_point(origin) {
        let distances = [
            (origin[0] - bounds.min[0], [-1.0, 0.0]),
            (bounds.max[0] - origin[0], [1.0, 0.0]),
            (origin[1] - bounds.min[1], [0.0, -1.0]),
            (bounds.max[1] - origin[1], [0.0, 1.0]),
        ];
        return distances
            .into_iter()
            .filter(|(distance, _)| *distance >= 0.0)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, normal)| (0.0, normal));
    }

    let mut enter: f32 = 0.0;
    let mut exit: f32 = 1.0;
    let mut normal = [0.0, 0.0];
    for axis in 0..2 {
        let origin_axis = origin[axis];
        let delta_axis = delta[axis];
        if delta_axis.abs() <= EPSILON {
            if origin_axis <= bounds.min[axis] || origin_axis >= bounds.max[axis] {
                return None;
            }
            continue;
        }

        let inverse = 1.0 / delta_axis;
        let mut near = (bounds.min[axis] - origin_axis) * inverse;
        let mut far = (bounds.max[axis] - origin_axis) * inverse;
        let near_normal = if delta_axis > 0.0 {
            let mut value = [0.0, 0.0];
            value[axis] = -1.0;
            value
        } else {
            let mut value = [0.0, 0.0];
            value[axis] = 1.0;
            value
        };
        if near > far {
            std::mem::swap(&mut near, &mut far);
        }
        if near > enter || (normal == [0.0, 0.0] && near >= enter) {
            enter = near;
            normal = near_normal;
        }
        exit = exit.min(far);
        if enter > exit {
            return None;
        }
    }

    if !(0.0..=1.0).contains(&enter) {
        return None;
    }
    if enter <= EPSILON && (delta[0] * normal[0] + delta[1] * normal[1]) >= 0.0 {
        return None;
    }
    Some((enter, normal))
}

fn finite2(value: [f32; 2]) -> bool {
    value[0].is_finite() && value[1].is_finite()
}

fn finite_aabb(value: Aabb2) -> bool {
    finite2(value.min)
        && finite2(value.max)
        && value.min[0] <= value.max[0]
        && value.min[1] <= value.max[1]
}

/// A 2D circle in world space (`+X` right, `+Y` up).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Circle2 {
    pub center: [f32; 2],
    pub radius: f32,
}

impl Circle2 {
    pub const fn new(center: [f32; 2], radius: f32) -> Self {
        Self { center, radius }
    }

    fn effective_radius(self) -> f32 {
        if self.radius.is_finite() {
            self.radius.max(0.0)
        } else {
            0.0
        }
    }

    pub fn bounds(self) -> Aabb2 {
        let r = self.effective_radius();
        Aabb2::from_center_half_extents(self.center, [r, r])
    }

    pub fn contains_point(self, point: [f32; 2]) -> bool {
        if !finite2(self.center) || !finite2(point) {
            return false;
        }
        let r = self.effective_radius();
        let dx = point[0] - self.center[0];
        let dy = point[1] - self.center[1];
        dx * dx + dy * dy <= r * r
    }

    pub fn overlaps_circle(self, other: Self) -> bool {
        if !finite2(self.center) || !finite2(other.center) {
            return false;
        }
        let r = self.effective_radius() + other.effective_radius();
        let dx = other.center[0] - self.center[0];
        let dy = other.center[1] - self.center[1];
        dx * dx + dy * dy < r * r
    }

    pub fn overlaps_aabb(self, bounds: Aabb2) -> bool {
        if !finite2(self.center) || !finite_aabb(bounds) {
            return false;
        }
        let cx = self.center[0].clamp(bounds.min[0], bounds.max[0]);
        let cy = self.center[1].clamp(bounds.min[1], bounds.max[1]);
        let dx = self.center[0] - cx;
        let dy = self.center[1] - cy;
        let r = self.effective_radius();
        dx * dx + dy * dy < r * r
    }

    /// Returns `(normal_from_other_to_self, penetration_depth)` when overlapping.
    pub fn penetration_circle(self, other: Self) -> Option<([f32; 2], f32)> {
        if !finite2(self.center) || !finite2(other.center) {
            return None;
        }
        let sum_r = self.effective_radius() + other.effective_radius();
        let dx = self.center[0] - other.center[0];
        let dy = self.center[1] - other.center[1];
        let dist_sq = dx * dx + dy * dy;
        if dist_sq >= sum_r * sum_r {
            return None;
        }
        let dist = dist_sq.sqrt();
        if dist > EPSILON {
            Some(([dx / dist, dy / dist], sum_r - dist))
        } else {
            Some(([1.0, 0.0], sum_r))
        }
    }

    /// Returns `true` if this circle overlaps a 2D circular sector (`origin`, `dir`, `range`, `half_angle_rad`).
    pub fn overlaps_sector(
        self,
        origin: [f32; 2],
        dir: [f32; 2],
        range: f32,
        half_angle_rad: f32,
    ) -> bool {
        if !finite2(self.center)
            || !finite2(origin)
            || !finite2(dir)
            || !range.is_finite()
            || !half_angle_rad.is_finite()
            || range <= 0.0
            || half_angle_rad < 0.0
        {
            return false;
        }
        let r = self.effective_radius();
        let dx = self.center[0] - origin[0];
        let dy = self.center[1] - origin[1];
        let dist_sq = dx * dx + dy * dy;
        let max_dist = range + r;
        if dist_sq > max_dist * max_dist {
            return false;
        }
        if dist_sq <= r * r || half_angle_rad >= std::f32::consts::PI {
            return true;
        }

        let dir_len = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
        if dir_len <= EPSILON {
            return false;
        }
        let ndx = dir[0] / dir_len;
        let ndy = dir[1] / dir_len;
        let dist = dist_sq.sqrt();
        let cos_angle = (dx * ndx + dy * ndy) / dist;
        if cos_angle >= half_angle_rad.cos() {
            return true;
        }

        let (sin_h, cos_h) = half_angle_rad.sin_cos();
        let edge_a = [
            origin[0] + (ndx * cos_h - ndy * sin_h) * range,
            origin[1] + (ndx * sin_h + ndy * cos_h) * range,
        ];
        let edge_b = [
            origin[0] + (ndx * cos_h + ndy * sin_h) * range,
            origin[1] + (-ndx * sin_h + ndy * cos_h) * range,
        ];
        point_to_segment_dist_sq(self.center, origin, edge_a) <= r * r
            || point_to_segment_dist_sq(self.center, origin, edge_b) <= r * r
    }

    /// Returns `true` if this circle overlaps a capsule (line segment `a..=b` expanded by `segment_radius`).
    pub fn overlaps_segment_radius(self, a: [f32; 2], b: [f32; 2], segment_radius: f32) -> bool {
        if !finite2(self.center) || !finite2(a) || !finite2(b) || !segment_radius.is_finite() {
            return false;
        }
        let total_r = self.effective_radius() + segment_radius.max(0.0);
        point_to_segment_dist_sq(self.center, a, b) <= total_r * total_r
    }
}

fn point_to_segment_dist_sq(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let abx = b[0] - a[0];
    let aby = b[1] - a[1];
    let len_sq = abx * abx + aby * aby;
    if len_sq <= EPSILON * EPSILON {
        let dx = p[0] - a[0];
        let dy = p[1] - a[1];
        return dx * dx + dy * dy;
    }
    let t = (((p[0] - a[0]) * abx + (p[1] - a[1]) * aby) / len_sq).clamp(0.0, 1.0);
    let cx = a[0] + abx * t;
    let cy = a[1] + aby * t;
    let dx = p[0] - cx;
    let dy = p[1] - cy;
    dx * dx + dy * dy
}

/// Deterministic uniform spatial hash grid for fast 2D broadphase and nearest-neighbor queries.
#[derive(Debug, Clone)]
pub struct SpatialGrid2D {
    inv_cell_size: f32,
    cell_size: f32,
    buckets: std::collections::HashMap<(i32, i32), Vec<u32>>,
}

impl SpatialGrid2D {
    pub fn new(cell_size: f32) -> Self {
        let cell_size = if cell_size.is_finite() && cell_size > EPSILON {
            cell_size
        } else {
            1.0
        };
        Self {
            inv_cell_size: 1.0 / cell_size,
            cell_size,
            buckets: std::collections::HashMap::new(),
        }
    }

    pub fn cell_size(&self) -> f32 {
        self.cell_size
    }

    /// Clear all entries while retaining allocated bucket vectors for zero-churn reuse.
    pub fn clear(&mut self) {
        for bucket in self.buckets.values_mut() {
            bucket.clear();
        }
    }

    fn cell_coords(&self, pos: [f32; 2]) -> (i32, i32) {
        (
            (pos[0] * self.inv_cell_size).floor() as i32,
            (pos[1] * self.inv_cell_size).floor() as i32,
        )
    }

    pub fn insert_point(&mut self, id: u32, pos: [f32; 2]) {
        if !finite2(pos) {
            return;
        }
        let cell = self.cell_coords(pos);
        self.buckets.entry(cell).or_default().push(id);
    }

    pub fn insert_circle(&mut self, id: u32, circle: Circle2) {
        if !finite2(circle.center) {
            return;
        }
        self.insert_aabb(id, circle.bounds());
    }

    pub fn insert_aabb(&mut self, id: u32, bounds: Aabb2) {
        if !finite_aabb(bounds) {
            return;
        }
        let (x0, y0) = self.cell_coords(bounds.min);
        let (x1, y1) = self.cell_coords(bounds.max);
        for cy in y0..=y1 {
            for cx in x0..=x1 {
                self.buckets.entry((cx, cy)).or_default().push(id);
            }
        }
    }

    /// Collect sorted, deduplicated candidate IDs overlapping `bounds`.
    pub fn query_aabb(&self, bounds: Aabb2, out: &mut Vec<u32>) {
        out.clear();
        if !finite_aabb(bounds) {
            return;
        }
        let (x0, y0) = self.cell_coords(bounds.min);
        let (x1, y1) = self.cell_coords(bounds.max);
        for cy in y0..=y1 {
            for cx in x0..=x1 {
                if let Some(bucket) = self.buckets.get(&(cx, cy)) {
                    out.extend_from_slice(bucket);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
    }

    /// Collect sorted, deduplicated candidate IDs overlapping `circle`'s bounding box.
    pub fn query_circle(&self, circle: Circle2, out: &mut Vec<u32>) {
        if !finite2(circle.center) {
            out.clear();
            return;
        }
        self.query_aabb(circle.bounds(), out);
    }

    /// Find the closest entity within `max_radius` whose position is supplied by `pos_of`.
    pub fn find_nearest<F>(
        &self,
        origin: [f32; 2],
        max_radius: f32,
        pos_of: F,
    ) -> Option<(u32, [f32; 2], f32)>
    where
        F: Fn(u32) -> Option<[f32; 2]>,
    {
        if !finite2(origin) || !max_radius.is_finite() || max_radius <= 0.0 {
            return None;
        }
        let bounds = Aabb2::from_center_half_extents(origin, [max_radius, max_radius]);
        let (x0, y0) = self.cell_coords(bounds.min);
        let (x1, y1) = self.cell_coords(bounds.max);
        let max_sq = max_radius * max_radius;
        let mut best: Option<(u32, [f32; 2], f32)> = None;
        for cy in y0..=y1 {
            for cx in x0..=x1 {
                let Some(bucket) = self.buckets.get(&(cx, cy)) else {
                    continue;
                };
                for &id in bucket {
                    let Some(pos) = pos_of(id) else {
                        continue;
                    };
                    if !finite2(pos) {
                        continue;
                    }
                    let dx = pos[0] - origin[0];
                    let dy = pos[1] - origin[1];
                    let dist_sq = dx * dx + dy * dy;
                    if dist_sq > max_sq {
                        continue;
                    }
                    let is_better = match best {
                        None => true,
                        Some((best_id, _, best_sq)) => {
                            dist_sq < best_sq || (dist_sq == best_sq && id < best_id)
                        }
                    };
                    if is_better {
                        best = Some((id, pos, dist_sq));
                    }
                }
            }
        }
        best.map(|(id, pos, dist_sq)| (id, pos, dist_sq.sqrt()))
    }
}

/// Agent descriptor for [`resolve_crowd_separation`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrowdAgent2D {
    pub id: u32,
    pub position: [f32; 2],
    pub radius: f32,
    pub weight: f32,
}

/// Apply soft pairwise crowd separation across 2D circular agents using `grid`,
/// keeping dense swarms from collapsing onto identical coordinates.
pub fn resolve_crowd_separation(
    agents: &mut [CrowdAgent2D],
    grid: &mut SpatialGrid2D,
    stiffness: f32,
    arena_bounds: Option<Aabb2>,
) {
    if agents.is_empty() || !stiffness.is_finite() || stiffness <= 0.0 {
        return;
    }
    grid.clear();
    for (index, agent) in agents.iter().enumerate() {
        grid.insert_circle(index as u32, Circle2::new(agent.position, agent.radius));
    }

    let mut deltas = vec![[0.0f32; 2]; agents.len()];
    let mut neighbors = Vec::with_capacity(32);
    for (i, a) in agents.iter().enumerate() {
        let ra = a.radius.max(0.0);
        grid.query_circle(Circle2::new(a.position, ra), &mut neighbors);
        let wa = a.weight.max(0.01);
        for &j_u32 in &neighbors {
            let j = j_u32 as usize;
            if j <= i {
                continue;
            }
            let b = &agents[j];
            let rb = b.radius.max(0.0);
            let sum_r = ra + rb;
            let dx = b.position[0] - a.position[0];
            let dy = b.position[1] - a.position[1];
            let dist_sq = dx * dx + dy * dy;
            if dist_sq >= sum_r * sum_r {
                continue;
            }
            let dist = dist_sq.sqrt();
            let (nx, ny) = if dist > EPSILON {
                (dx / dist, dy / dist)
            } else {
                let angle = (i as f32) * 2.399_963_2;
                (angle.cos(), angle.sin())
            };
            let depth = (sum_r - dist) * stiffness;
            let wb = b.weight.max(0.01);
            let total_w = wa + wb;
            let push_a = depth * (wb / total_w);
            let push_b = depth * (wa / total_w);
            deltas[i][0] -= nx * push_a;
            deltas[i][1] -= ny * push_a;
            deltas[j][0] += nx * push_b;
            deltas[j][1] += ny * push_b;
        }
    }

    for (agent, delta) in agents.iter_mut().zip(deltas) {
        let max_step = agent.radius.max(0.25);
        let mag_sq = delta[0] * delta[0] + delta[1] * delta[1];
        let (dx, dy) = if mag_sq > max_step * max_step {
            let scale = max_step / mag_sq.sqrt();
            (delta[0] * scale, delta[1] * scale)
        } else {
            (delta[0], delta[1])
        };
        agent.position[0] += dx;
        agent.position[1] += dy;
        if let Some(bounds) = arena_bounds {
            let r = agent.radius.max(0.0);
            let min_x = (bounds.min[0] + r).min(bounds.max[0]);
            let max_x = (bounds.max[0] - r).max(bounds.min[0]);
            let min_y = (bounds.min[1] + r).min(bounds.max[1]);
            let max_y = (bounds.max[1] - r).max(bounds.min[1]);
            agent.position[0] = agent.position[0].clamp(min_x, max_x);
            agent.position[1] = agent.position[1].clamp(min_y, max_y);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_touch_without_overlapping() {
        let left = Aabb2::new(0.0, 0.0, 10.0, 10.0);
        let right = Aabb2::new(10.0, 0.0, 20.0, 10.0);
        assert!(!left.overlaps(right));
    }

    #[test]
    fn high_speed_horizontal_move_stops_at_a_thin_wall() {
        let result = move_and_slide(
            [0.0, 5.0],
            [1.0, 1.0],
            [100.0, 0.0],
            &[Aabb2::new(10.0, 0.0, 11.0, 10.0)],
        );
        assert!((result.position[0] - 9.0).abs() < 1e-5);
        assert!(result.hit_x);
        assert_eq!(result.applied_delta, [9.0, 0.0]);
    }

    #[test]
    fn high_speed_leftward_move_stops_at_a_thin_wall() {
        let result = move_and_slide(
            [20.0, 5.0],
            [1.0, 1.0],
            [-100.0, 0.0],
            &[Aabb2::new(10.0, 0.0, 11.0, 10.0)],
        );
        assert!((result.position[0] - 12.0).abs() < 1e-5);
        assert!(result.hit_x);
        assert_eq!(result.applied_delta, [-8.0, 0.0]);
    }

    #[test]
    fn falling_move_reports_grounded_and_slides_horizontally() {
        let result = move_and_slide(
            [5.0, 10.0],
            [1.0, 1.0],
            [20.0, -20.0],
            &[Aabb2::new(0.0, 0.0, 100.0, 2.0)],
        );
        assert!((result.position[0] - 25.0).abs() < 1e-5);
        assert!((result.position[1] - 3.0).abs() < 1e-5);
        assert!(result.grounded);
    }

    #[test]
    fn corner_contact_slides_along_both_surfaces() {
        let result = move_and_slide(
            [5.0, 5.0],
            [1.0, 1.0],
            [20.0, 20.0],
            &[
                Aabb2::new(10.0, 0.0, 11.0, 20.0),
                Aabb2::new(0.0, 10.0, 20.0, 11.0),
            ],
        );
        assert!((result.position[0] - 9.0).abs() < 1e-5);
        assert!((result.position[1] - 9.0).abs() < 1e-5);
        assert!(result.hit_x && result.ceilinged);
    }

    #[test]
    fn zero_delta_does_not_push_an_embedded_box() {
        let start = [5.0, 5.0];
        let result = move_and_slide(
            start,
            [2.0, 2.0],
            [0.0, 0.0],
            &[Aabb2::new(0.0, 0.0, 10.0, 10.0)],
        );
        assert_eq!(result.position, start);
        assert_eq!(result.contact_count, 0);
    }

    #[test]
    fn nonzero_move_depenetrates_an_embedded_box() {
        let result = move_and_slide(
            [5.0, 5.0],
            [1.0, 1.0],
            [0.0, -1.0],
            &[Aabb2::new(0.0, 0.0, 10.0, 10.0)],
        );
        let body = Aabb2::from_center_half_extents(result.position, [1.0, 1.0]);
        assert!(!body.overlaps(Aabb2::new(0.0, 0.0, 10.0, 10.0)));
    }

    #[test]
    fn raycast_reports_distance_and_outward_normal() {
        let hit = raycast_aabb(
            [0.0, 5.0],
            [2.0, 0.0],
            20.0,
            Aabb2::new(10.0, 0.0, 12.0, 10.0),
        )
        .expect("ray hits the box");
        assert!((hit.distance - 10.0).abs() < 1e-5);
        assert_eq!(hit.point, [10.0, 5.0]);
        assert_eq!(hit.normal, [-1.0, 0.0]);
    }

    #[test]
    fn raycast_starting_inside_reports_zero_distance() {
        let hit = raycast_aabb(
            [5.0, 5.0],
            [1.0, 0.0],
            20.0,
            Aabb2::new(0.0, 0.0, 10.0, 10.0),
        )
        .expect("inside origin is a hit");
        assert_eq!(hit.distance, 0.0);
        assert_eq!(hit.point, [5.0, 5.0]);
        assert_eq!(hit.normal, [0.0, 0.0]);
    }

    #[test]
    fn circle_overlaps_sector_and_capsule_shapes() {
        let target = Circle2::new([2.0, 1.0], 0.5);
        assert!(target.overlaps_sector([0.0, 0.0], [1.0, 0.0], 3.0, 0.6));
        assert!(!target.overlaps_sector([0.0, 0.0], [-1.0, 0.0], 3.0, 0.6));
        assert!(target.overlaps_segment_radius([0.0, 0.0], [3.0, 1.0], 0.3));
        assert!(!target.overlaps_segment_radius([0.0, 0.0], [0.0, 5.0], 0.3));
        assert!(target.overlaps_aabb(Aabb2::new(1.6, 0.6, 4.0, 4.0)));
    }

    #[test]
    fn spatial_grid_finds_nearest_and_queries_bounds() {
        let mut grid = SpatialGrid2D::new(2.0);
        let positions = vec![
            (10u32, [5.0, 0.0]),
            (20u32, [1.5, 0.5]),
            (30u32, [-10.0, 0.0]),
        ];
        for &(id, pos) in &positions {
            grid.insert_circle(id, Circle2::new(pos, 0.4));
        }

        let mut hits = Vec::new();
        grid.query_circle(Circle2::new([1.0, 0.0], 1.5), &mut hits);
        assert_eq!(hits, vec![20]);

        let nearest = grid
            .find_nearest([0.0, 0.0], 6.0, |id| {
                positions
                    .iter()
                    .find(|(pid, _)| *pid == id)
                    .map(|(_, pos)| *pos)
            })
            .expect("nearest enemy within range");
        assert_eq!(nearest.0, 20);
        assert!((nearest.2 - (1.5f32.hypot(0.5))).abs() < 1e-5);
    }

    #[test]
    fn crowd_separation_spreads_overlapping_swarms_cleanly() {
        let mut agents: Vec<CrowdAgent2D> = (0..200)
            .map(|i| CrowdAgent2D {
                id: i,
                position: [0.0, 0.0],
                radius: 0.4,
                weight: 1.0,
            })
            .collect();
        let mut grid = SpatialGrid2D::new(1.0);
        let bounds = Aabb2::new(-20.0, -20.0, 20.0, 20.0);
        for _ in 0..24 {
            resolve_crowd_separation(&mut agents, &mut grid, 0.5, Some(bounds));
        }
        assert!(agents
            .iter()
            .all(|a| a.position[0].is_finite() && a.position[1].is_finite()));
        let unique_positions: std::collections::HashSet<(i32, i32)> = agents
            .iter()
            .map(|a| {
                (
                    (a.position[0] * 100.0).round() as i32,
                    (a.position[1] * 100.0).round() as i32,
                )
            })
            .collect();
        assert!(
            unique_positions.len() > 150,
            "agents should separate instead of staying stacked: {}",
            unique_positions.len()
        );
    }
}
