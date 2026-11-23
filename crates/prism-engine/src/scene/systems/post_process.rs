//! Local post-processing volumes (#502 Phase 2).
//!
//! [`PostProcessVolume`] in scene outlines a region with axis-aligned box; when main camera enters, box
//! parameters blend by `priority` from low to high onto this frame post-processing parameters ([`PostGrade`]).
//!
//! - With no volume, return caller `base` (usually `RenderSettings::post_grade()`),
//!   so default path bit-identical to old image;
//! - Outside box within `blend_distance` linear fade-out, beyond that not involved;
//! - `global = true` volume ignores box, always involved;
//! - Only reads entity world transform translation + axis lengths (scale), rotation not involved — consistent with terrain height field;
//! - Volume under inactive parent does not take effect, consistent with render entity `Active` rule.
//!
//! Multi-camera compositing (#461) shares one post-processing, so this only parses by **main camera**
//! position (i.e. `active_camera`, `SceneChanges::eye`); per-camera post-processing needs multiple targets, left for later.

use glam::Vec3;
use prism_ecs::World;
use prism_render::PostGrade;

use crate::scene::components::{LocalToWorld, PostProcessVolume};

/// Blend `base` with all hit volumes by priority, get this frame post-processing parameters.
pub fn resolve_post_grade(world: &World, eye: Vec3, base: PostGrade) -> PostGrade {
    if !eye.is_finite() {
        return base.sanitized();
    }
    let mut volumes: Vec<(u32, f32, PostProcessVolume, Vec3, Vec3)> = Vec::new();
    for (entity, volume, transform) in world.query2::<PostProcessVolume, LocalToWorld>() {
        if !volume.enabled {
            continue;
        }
        if !crate::scene::systems::hierarchy::is_active_in_hierarchy(world, entity) {
            continue;
        }
        let matrix = transform.0;
        let center = matrix.w_axis.truncate();
        let scale = Vec3::new(
            matrix.x_axis.truncate().length(),
            matrix.y_axis.truncate().length(),
            matrix.z_axis.truncate().length(),
        );
        let priority = if volume.priority.is_finite() {
            volume.priority
        } else {
            0.0
        };
        volumes.push((entity.id(), priority, volume.clone(), center, scale));
    }
    if volumes.is_empty() {
        return base.sanitized();
    }
    volumes.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));

    let mut grade = base.sanitized();
    for (_, _, volume, center, scale) in volumes {
        let weight = volume_weight(&volume, center, scale, eye);
        if weight <= 0.0 {
            continue;
        }
        grade = lerp_grade(grade, volume_grade(&volume), weight);
    }
    grade
}

/// Volume weight relative camera position: 1 inside box, linear fade within `blend_distance`, 0 outside.
fn volume_weight(volume: &PostProcessVolume, center: Vec3, scale: Vec3, point: Vec3) -> f32 {
    if volume.global {
        return 1.0;
    }
    let half = Vec3::from_array(volume.size);
    let half = Vec3::new(
        finite_abs(half.x * scale.x),
        finite_abs(half.y * scale.y),
        finite_abs(half.z * scale.z),
    );
    let offset = (point - center).abs() - half;
    let outside = offset.max_element();
    if outside <= 0.0 {
        return 1.0;
    }
    let blend = if volume.blend_distance.is_finite() {
        volume.blend_distance.max(0.0)
    } else {
        0.0
    };
    if blend <= 0.0 {
        0.0
    } else {
        (1.0 - outside / blend).clamp(0.0, 1.0)
    }
}

fn finite_abs(value: f32) -> f32 {
    if value.is_finite() {
        value.abs()
    } else {
        0.0
    }
}

/// Map component parameters to post-processing parameters (intensity goes to zero when `bloom = false`).
fn volume_grade(volume: &PostProcessVolume) -> PostGrade {
    PostGrade {
        contrast: volume.contrast,
        saturation: volume.saturation,
        vignette: volume.vignette,
        bloom_intensity: if volume.bloom {
            volume.bloom_intensity
        } else {
            0.0
        },
        bloom_threshold: volume.bloom_threshold,
        chromatic_aberration: volume.chromatic_aberration,
        film_grain: volume.film_grain,
        temperature: volume.temperature,
        tint: volume.tint,
        dof_focus_distance: volume.dof_focus_distance,
        dof_aperture: volume.dof_aperture,
        dof_max_blur: volume.dof_max_blur,
        lut_strength: 0.0,
        motion_blur: volume.motion_blur,
        motion_blur_max_pixels: volume.motion_blur_max_pixels,
    }
    .sanitized()
}

/// Per-field linear blend `a → b`, then clamp to legal range.
fn lerp_grade(a: PostGrade, b: PostGrade, t: f32) -> PostGrade {
    let t = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mix = |from: f32, to: f32| from + (to - from) * t;
    PostGrade {
        contrast: mix(a.contrast, b.contrast),
        saturation: mix(a.saturation, b.saturation),
        vignette: mix(a.vignette, b.vignette),
        bloom_intensity: mix(a.bloom_intensity, b.bloom_intensity),
        bloom_threshold: mix(a.bloom_threshold, b.bloom_threshold),
        chromatic_aberration: mix(a.chromatic_aberration, b.chromatic_aberration),
        film_grain: mix(a.film_grain, b.film_grain),
        temperature: mix(a.temperature, b.temperature),
        tint: mix(a.tint, b.tint),
        dof_focus_distance: mix(a.dof_focus_distance, b.dof_focus_distance),
        dof_aperture: mix(a.dof_aperture, b.dof_aperture),
        dof_max_blur: mix(a.dof_max_blur, b.dof_max_blur),
        lut_strength: 0.0,
        motion_blur: mix(a.motion_blur, b.motion_blur),
        motion_blur_max_pixels: mix(a.motion_blur_max_pixels, b.motion_blur_max_pixels),
    }
    .sanitized()
}

#[cfg(test)]
#[path = "post_process_tests.rs"]
mod tests;
