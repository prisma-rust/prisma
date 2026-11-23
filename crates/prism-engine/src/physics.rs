//! Physics query (#522 Phase 1).
//!
//! This layer does pure geometry only: converts author's [`Collider`] into world-space parsed shape, then raycast intersection.
//! `prism-app`'s physics feature already uses rapier rigid bodies; this inside still does not depend
//! `rapier3d`, nor holds physics world state — every query reads ECS directly. rapier's
//! Started/Stopped events written by `prism-app::physics_runner` into [`PhysicsEvents`] resource.
//!
//! Supported shapes: box (OBB, intersected by entity rotation/scale), sphere, capsule (along local +Y).
//! Capsule uses "box + sphere" two-segment intersection similar (cylinder segment when box, ends when sphere), enough for character/bullet query.

use prism_ecs::{Entity, World};
use prism_scene::components::{Collider, ColliderShape, LocalToWorld};

/// Direction of one collision / trigger event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicsEventKind {
    Started,
    Stopped,
}

/// Collision events reported by rapier within fixed step, already mapped to ECS entity.
///
/// `is_sensor == true` means at least one side is `Collider.is_trigger`; normal rigid bodies between
/// contact also produces Started/Stopped, convenient for landing / impact logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicsEvent {
    pub kind: PhysicsEventKind,
    pub entity_a: Entity,
    pub entity_b: Entity,
    pub is_sensor: bool,
}

/// Main-thread visible physics event buffer.
///
/// Each fixed step covers once: Started/Stopped occurring after one `step` go inside here.
/// Consumer needing cross-step accumulation should drain into its own queue after each step.
#[derive(Debug, Clone, Default)]
pub struct PhysicsEvents {
    pub events: Vec<PhysicsEvent>,
}

impl PhysicsEvents {
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// Result of one hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    pub entity: Entity,
    /// Distance from raycast origin (world units).
    pub distance: f32,
    /// World coordinates of hit point.
    pub point: [f32; 3],
    /// World space normal of hit face (sphere/capsule is radial normal).
    pub normal: [f32; 3],
    /// Hit collision body is not trigger (trigger does not block movement, but query can see it).
    pub is_trigger: bool,
}

/// Casts a raycast, returns nearest hit.
///
/// `layer_mask` ANDed with collision body's `layer` non-zero to participate; `max_distance` outside does not count as hit.
/// Direction will be normalized, zero vector directly returns `None`.
pub fn raycast(
    world: &World,
    origin: [f32; 3],
    direction: [f32; 3],
    max_distance: f32,
    layer_mask: u32,
) -> Option<RayHit> {
    raycast_inner(world, origin, direction, max_distance, layer_mask, None)
}

/// [`raycast`] that skips one entity's colliders.
///
/// Character queries start inside the character's own capsule, so the nearest hit
/// would always be the caster itself. Excluding it here keeps the geometry loop in
/// one place instead of each caller re-casting past its own collider.
pub fn raycast_excluding(
    world: &World,
    origin: [f32; 3],
    direction: [f32; 3],
    max_distance: f32,
    layer_mask: u32,
    excluded: Entity,
) -> Option<RayHit> {
    raycast_inner(
        world,
        origin,
        direction,
        max_distance,
        layer_mask,
        Some(excluded),
    )
}

fn raycast_inner(
    world: &World,
    origin: [f32; 3],
    direction: [f32; 3],
    max_distance: f32,
    layer_mask: u32,
    excluded: Option<Entity>,
) -> Option<RayHit> {
    let dir = glam::Vec3::from_array(direction);
    let length = dir.length();
    if !length.is_finite() || length <= 1e-6 || max_distance <= 0.0 {
        return None;
    }
    let dir = dir / length;
    let origin = glam::Vec3::from_array(origin);

    let mut best: Option<RayHit> = None;
    for (entity, collider) in world.query::<Collider>() {
        if collider.layer & layer_mask == 0 || excluded == Some(entity) {
            continue;
        }
        let Some(transform) = world.get::<LocalToWorld>(entity) else {
            continue;
        };
        let shape = WorldShape::new(collider, &transform.0);
        let Some((distance, normal)) = shape.intersect(origin, dir, max_distance) else {
            continue;
        };
        if distance > max_distance {
            continue;
        }
        if best.is_some_and(|hit| hit.distance <= distance) {
            continue;
        }
        best = Some(RayHit {
            entity,
            distance,
            point: (origin + dir * distance).to_array(),
            normal: normal.to_array(),
            is_trigger: collider.is_trigger,
        });
    }
    best
}

/// World space collision shape (already baked entity rotation/scale/center offset into it).
#[derive(Debug, Clone, Copy)]
pub enum WorldShape {
    /// Oriented box: center + three half-axis vectors.
    Box {
        center: glam::Vec3,
        axes: [glam::Vec3; 3],
    },
    Sphere {
        center: glam::Vec3,
        radius: f32,
    },
}

impl WorldShape {
    /// Constructed from author's collision body + entity world matrix.
    ///
    /// Capsule degrades into "box centered at center, covering both end points", box itself retains rotation (OBB).
    /// Unlimited/degenerate size returns sphere (radius 0) — intersection will never hit.
    pub fn new(collider: &Collider, world: &glam::Mat4) -> Self {
        let scale = world.to_scale_rotation_translation().0;
        let rotation = world.to_scale_rotation_translation().1;
        let center = world.transform_point3(glam::Vec3::from_array(collider.center));
        let axis = |local: glam::Vec3| rotation * (local * scale);
        let abs_scale = glam::Vec3::new(scale.x.abs(), scale.y.abs(), scale.z.abs());

        match collider.shape {
            ColliderShape::Box => Self::Box {
                center,
                axes: [
                    axis(glam::Vec3::X) * collider.half_extents[0],
                    axis(glam::Vec3::Y) * collider.half_extents[1],
                    axis(glam::Vec3::Z) * collider.half_extents[2],
                ],
            },
            ColliderShape::Sphere => Self::Sphere {
                center,
                radius: (collider.radius * abs_scale.max_element()).max(0.0),
            },
            ColliderShape::Capsule => {
                // Cylinder segment + two end hemispheres: covered by "box + capsule radius" OBB, enough for query.
                let half = glam::Vec3::new(
                    collider.radius * abs_scale.x,
                    (collider.half_height + collider.radius) * abs_scale.y,
                    collider.radius * abs_scale.z,
                );
                Self::Box {
                    center,
                    axes: [
                        axis(glam::Vec3::X) * half.x,
                        axis(glam::Vec3::Y) * half.y,
                        axis(glam::Vec3::Z) * half.z,
                    ],
                }
            }
        }
    }

    /// Pushes a point of radius `radius` out of shape surface (#501 particle collision use).
    ///
    /// Returns `(corrected position, surface world normal)`; point outside shape (and beyond radius from surface) when
    /// returns `None`. Box internal pushed out along "most shallow face", sphere uses radial normal.
    pub fn push_out(&self, point: glam::Vec3, radius: f32) -> Option<(glam::Vec3, glam::Vec3)> {
        if !point.is_finite() || !radius.is_finite() || radius < 0.0 {
            return None;
        }
        match *self {
            Self::Sphere {
                center,
                radius: shape,
            } => {
                let delta = point - center;
                let distance = delta.length();
                let minimum = (shape + radius).max(0.0);
                if distance >= minimum {
                    return None;
                }
                let normal = if distance > 1e-6 {
                    delta / distance
                } else {
                    glam::Vec3::Y
                };
                Some((center + normal * minimum, normal))
            }
            Self::Box { center, axes } => {
                let lengths = [axes[0].length(), axes[1].length(), axes[2].length()];
                if lengths
                    .iter()
                    .any(|length| !length.is_finite() || *length <= 1e-6)
                {
                    return None;
                }
                let basis = [
                    axes[0] / lengths[0],
                    axes[1] / lengths[1],
                    axes[2] / lengths[2],
                ];
                let local = point - center;
                let mut coordinates = [0.0f32; 3];
                let mut closest = center;
                for i in 0..3 {
                    let coordinate = local.dot(basis[i]);
                    coordinates[i] = coordinate;
                    let clamped = coordinate.clamp(-lengths[i], lengths[i]);
                    closest += basis[i] * clamped;
                }
                let delta = point - closest;
                let distance = delta.length();
                if distance > 1e-6 {
                    if distance > radius {
                        return None;
                    }
                    let normal = delta / distance;
                    return Some((closest + normal * radius, normal));
                }

                // Inside box: pushed out along most shallow penetration axis.
                let mut best_penetration = f32::INFINITY;
                let mut normal = glam::Vec3::Y;
                let mut face = center;
                for i in 0..3 {
                    let penetration = lengths[i] - coordinates[i].abs();
                    if penetration < best_penetration {
                        best_penetration = penetration;
                        let sign = if coordinates[i] >= 0.0 { 1.0 } else { -1.0 };
                        normal = basis[i] * sign;
                        face = center;
                        for j in 0..3 {
                            let coordinate = if i == j {
                                lengths[j] * sign
                            } else {
                                coordinates[j].clamp(-lengths[j], lengths[j])
                            };
                            face += basis[j] * coordinate;
                        }
                    }
                }
                Some((face + normal * radius, normal))
            }
        }
    }

    /// Raycast intersection: returns `(distance, world normal)`.
    pub fn intersect(
        &self,
        origin: glam::Vec3,
        dir: glam::Vec3,
        max_distance: f32,
    ) -> Option<(f32, glam::Vec3)> {
        match *self {
            Self::Box { center, axes } => {
                // Slab method: transform raycast into box's local coordinate system (axis vectors can carry scale, first normalize to take lengths).
                let lengths = [axes[0].length(), axes[1].length(), axes[2].length()];
                if lengths.iter().any(|l| !l.is_finite() || *l <= 1e-6) {
                    return None;
                }
                let basis = [
                    axes[0] / lengths[0],
                    axes[1] / lengths[1],
                    axes[2] / lengths[2],
                ];
                let local_origin = origin - center;
                let origin_local = glam::Vec3::new(
                    local_origin.dot(basis[0]) / lengths[0],
                    local_origin.dot(basis[1]) / lengths[1],
                    local_origin.dot(basis[2]) / lengths[2],
                );
                let dir_local = glam::Vec3::new(
                    dir.dot(basis[0]) / lengths[0],
                    dir.dot(basis[1]) / lengths[1],
                    dir.dot(basis[2]) / lengths[2],
                );

                let mut t_min = 0.0f32;
                let mut t_max = max_distance;
                let mut axis_hit = 0usize;
                let mut sign = 1.0f32;
                for i in 0..3 {
                    let o = origin_local[i];
                    let d = dir_local[i];
                    if d.abs() < 1e-9 {
                        if o.abs() > 1.0 {
                            return None;
                        }
                        continue;
                    }
                    let inv = 1.0 / d;
                    let mut t1 = (-1.0 - o) * inv;
                    let mut t2 = (1.0 - o) * inv;
                    // Record entering face: first plane entered is the hit face.
                    let mut near_sign = -1.0;
                    if t1 > t2 {
                        std::mem::swap(&mut t1, &mut t2);
                        near_sign = 1.0;
                    }
                    if t1 > t_min {
                        t_min = t1;
                        axis_hit = i;
                        sign = near_sign;
                    }
                    t_max = t_max.min(t2);
                    if t_min > t_max {
                        return None;
                    }
                }
                if t_min > max_distance || t_max < 0.0 {
                    return None;
                }
                let normal = basis[axis_hit] * sign;
                Some((t_min, normal))
            }
            Self::Sphere { center, radius } => {
                let oc = origin - center;
                let b = oc.dot(dir);
                let c = oc.dot(oc) - radius * radius;
                let discriminant = b * b - c;
                if discriminant < 0.0 {
                    return None;
                }
                let sqrt = discriminant.sqrt();
                let mut t = -b - sqrt;
                if t < 0.0 {
                    // Origin inside sphere: shoot from inside, use far intersection point.
                    t = -b + sqrt;
                }
                if t < 0.0 || t > max_distance {
                    return None;
                }
                let point = origin + dir * t;
                let normal = (point - center).normalize_or_zero();
                Some((t, normal))
            }
        }
    }
}

#[cfg(test)]
#[path = "physics_tests.rs"]
mod tests;
