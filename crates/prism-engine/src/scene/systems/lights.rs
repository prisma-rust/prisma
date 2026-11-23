//! Light source collector — queries active light source components in the ECS world.
//!
//! Two paths coexist:
//!
//! 1. **Unified shape** (`Light`): [`Light`] uses [`crate::scene::components::LightType`]
//!    to distinguish directional / point / spot; direction and position from entity transform (`transform.forward` /
//!    `LocalToWorld`).
//! 2. **Legacy components**: [`DirectionalLight`] / [`PointLight`] / [`SpotLight`], still used in old scenes.
//!
//! [`collect_light_samples`] normalizes both to [`LightSample`]; when same entity has both `Light`
//! and legacy components, **only `Light`** is taken (avoid double lighting one entity).
//!
//! These functions called each frame by `render_system::collect_scene_changes`.

use glam::{Quat, Vec3};
use prism_ecs::World;

use crate::euler_xyz_deg_to_dir;
use crate::scene::components::*;
use prism_scene::{LightShadows, LightType};

/// Normalized light source type (types unsupported by renderer already culled).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightKind {
    /// Directional light (infinite range).
    Directional,
    /// Point light source (omni-directional).
    Point,
    /// Spot light (cone).
    Spot,
}

/// One frame light source sample: position / direction / color / identity all parsed.
///
/// `color` is **linear color, intensity NOT multiplied**; intensity by type is lux (directional) or candela
/// (point / spot), consistent with [`Light`] / legacy components units (renderer multiplies
/// `LUX_TO_RADIANCE_SCALE`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightSample {
    /// Light source entity (to report ones squeezed out by LIGHT_MAX to editor).
    pub entity: prism_ecs::Entity,
    pub kind: LightKind,
    /// World position (meaningless for directional light, `ZERO`).
    pub position: Vec3,
    /// Light source direction = `transform.forward` (valid for directional / spot).
    pub direction: Vec3,
    /// Linear color (color temperature filter already applied).
    pub color: Vec3,
    /// Intensity: directional lux, point / spot candela.
    pub intensity: f32,
    /// Attenuation radius (ignored for directional).
    pub range: f32,
    /// Cosine of spot inner cone half-angle.
    pub cos_inner: f32,
    /// Cosine of spot outer cone half-angle.
    pub cos_outer: f32,
    /// IBL ambient factor of directional light (`Light` always 1.0).
    pub ambient: f32,
    /// Shadow mode. **Not yet wired**: current shadow still from global directional light map, here only provides data.
    pub shadows: LightShadows,
    /// Shadow intensity (same, data row first).
    pub shadow_strength: f32,
}

/// Component-level visibility. `World::query` already excludes entities set inactive via `World::set_active`;
/// this function handles scene `Active` component used by inspector and scene loader.
pub(crate) fn component_is_active(world: &World, entity: prism_ecs::Entity) -> bool {
    crate::scene::systems::hierarchy::is_active_in_hierarchy(world, entity)
}

/// Entity world position and rotation: prefers `LocalToWorld`, then `LocalTransform`.
fn entity_transform(world: &World, entity: prism_ecs::Entity) -> (Vec3, Quat) {
    if let Some(l2w) = world.get::<LocalToWorld>(entity) {
        let (_, rotation, translation) = l2w.value().to_scale_rotation_translation();
        (translation, rotation)
    } else if let Some(t) = world.get::<LocalTransform>(entity) {
        (t.position, t.rotation)
    } else {
        (Vec3::ZERO, Quat::IDENTITY)
    }
}

/// Light source direction here is `transform.forward` (+Z).
fn forward(rotation: Quat) -> Vec3 {
    (rotation * Vec3::Z).normalize_or_zero()
}

fn sample_from_light(
    entity: prism_ecs::Entity,
    light: &Light,
    position: Vec3,
    rotation: Quat,
) -> Option<LightSample> {
    let kind = match light.light_type {
        LightType::Directional => LightKind::Directional,
        LightType::Point => LightKind::Point,
        LightType::Spot => LightKind::Spot,
        _ => return None,
    };
    let (inner, outer) = light.cone_half_angles_radians();
    Some(LightSample {
        entity,
        kind,
        position: if kind == LightKind::Directional {
            Vec3::ZERO
        } else {
            position
        },
        direction: forward(rotation),
        color: light.effective_color(),
        intensity: light.intensity,
        range: light.range,
        cos_inner: inner.cos(),
        cos_outer: outer.cos(),
        ambient: 1.0,
        shadows: light.shadows,
        shadow_strength: light.shadow_strength,
    })
}

/// "Importance" of a local light: larger means more deserving to keep in [`LIGHT_MAX`] slots.
///
/// `intensity × range² / max(distance², ε)` — close, bright, far-reaching lights first.
/// Only used to pick lights when over budget, not in shading. Directional light not in sort (returns 0).
pub fn local_light_importance(sample: &LightSample, eye: Vec3) -> f32 {
    if sample.kind == LightKind::Directional {
        return 0.0;
    }
    let distance_sq = (sample.position - eye).length_squared().max(0.01);
    let range = sample.range.max(0.0);
    let score = sample.intensity.max(0.0) * range * range / distance_sq;
    if score.is_finite() {
        score
    } else {
        0.0
    }
}

/// Collect all renderable light sources (unified `Light` first, legacy components fallback).
///
/// **No truncation**: local light limit [`LIGHT_MAX`] picked by `render_system` after knowing camera position
/// by "intensity × range² / distance²" (#468), here returns all enabled light sources.
/// Directional light does not occupy [`LIGHT_MAX`] (separate path via frame UBO).
pub fn collect_light_samples(world: &World) -> Vec<LightSample> {
    let mut out: Vec<LightSample> = Vec::new();
    collect_light_samples_into(world, &mut out);
    out
}

/// [`collect_light_samples`] into a caller-owned buffer.
///
/// The frame packet keeps the sample buffer alive across frames, so the
/// per-frame collection only refills an existing allocation (#66).
pub fn collect_light_samples_into(world: &World, out: &mut Vec<LightSample>) {
    out.clear();

    for (entity, light) in world.query::<Light>() {
        if !light.enabled || !component_is_active(world, entity) {
            continue;
        }
        let (position, rotation) = entity_transform(world, entity);
        if let Some(sample) = sample_from_light(entity, light, position, rotation) {
            out.push(sample);
        }
    }

    for (entity, light) in world.query::<DirectionalLight>() {
        if world.get::<Light>(entity).is_some() || !component_is_active(world, entity) {
            continue;
        }
        out.push(LightSample {
            entity,
            kind: LightKind::Directional,
            position: Vec3::ZERO,
            direction: -euler_xyz_deg_to_dir(light.euler_xyz),
            color: light.color,
            intensity: light.intensity,
            range: 0.0,
            cos_inner: 1.0,
            cos_outer: -1.0,
            ambient: light.ambient,
            shadows: LightShadows::Hard,
            shadow_strength: 1.0,
        });
    }
    for (entity, light) in world.query::<PointLight>() {
        if world.get::<Light>(entity).is_some() || !component_is_active(world, entity) {
            continue;
        }
        let (position, _) = entity_transform(world, entity);
        out.push(LightSample {
            entity,
            kind: LightKind::Point,
            position,
            direction: Vec3::ZERO,
            color: light.color,
            intensity: light.intensity,
            range: light.range,
            cos_inner: 1.0,
            cos_outer: -1.0,
            ambient: 1.0,
            shadows: LightShadows::None,
            shadow_strength: 1.0,
        });
    }
    for (entity, light) in world.query::<SpotLight>() {
        if world.get::<Light>(entity).is_some() || !component_is_active(world, entity) {
            continue;
        }
        let (position, rotation) = entity_transform(world, entity);
        let inner = light.inner_cone_angle.clamp(0.0, light.outer_cone_angle);
        out.push(LightSample {
            entity,
            kind: LightKind::Spot,
            position,
            direction: forward(rotation),
            color: light.color,
            intensity: light.intensity,
            range: light.range,
            cos_inner: inner.cos(),
            cos_outer: light.outer_cone_angle.cos(),
            ambient: 1.0,
            shadows: LightShadows::None,
            shadow_strength: 1.0,
        });
    }
}

/// Directional light shadow state `(enabled, strength)`.
///
/// `enabled` = `Light.shadows != None`; `strength` = `Light.shadowStrength`
/// (clamped to `0..=1`). Both "disabled" when no directional light.
pub fn directional_shadow_state(samples: &[LightSample]) -> (bool, f32) {
    match samples
        .iter()
        .find(|sample| sample.kind == LightKind::Directional)
    {
        Some(light) => (
            light.shadows.casts_shadows(),
            light.shadow_strength.clamp(0.0, 1.0),
        ),
        None => (false, 0.0),
    }
}

/// Primary directional light = first active directional light (unified `Light` or legacy).
pub fn primary_directional_light(world: &World) -> Option<LightSample> {
    collect_light_samples(world)
        .into_iter()
        .find(|sample| sample.kind == LightKind::Directional)
}

/// Return the first [`DirectionalLight`] in the world, if any.
///
/// Legacy API, kept for callers still fetching lights by legacy components; new code uses
/// [`primary_directional_light`] (both recognized).
pub fn collect_directional_light(world: &World) -> Option<DirectionalLight> {
    world
        .query::<DirectionalLight>()
        .find(|(entity, _)| component_is_active(world, *entity))
        .map(|(_, light)| *light)
}

/// Collect point lights (all of them).
///
/// Convenience query for legacy component path; **no truncation** — [`LIGHT_MAX`] picked by `render_system` after
/// knowing camera position (#468), cutting again here would make point and spot lights squeeze each other out.
pub fn collect_point_lights(world: &World) -> Vec<PointLight> {
    world
        .query::<PointLight>()
        .filter(|(entity, _)| component_is_active(world, *entity))
        .map(|(_, l)| *l)
        .collect()
}

/// Collect spot lights (all of them).
pub fn collect_spot_lights(world: &World) -> Vec<SpotLight> {
    world
        .query::<SpotLight>()
        .filter(|(entity, _)| component_is_active(world, *entity))
        .map(|(_, l)| *l)
        .collect()
}

#[cfg(test)]
#[path = "lights_tests.rs"]
mod tests;
