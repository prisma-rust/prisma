//! Day/night system (#983): advance [`TimeOfDay`], write result to ambient light on same entity.
//!
//! A project only needs one entity:
//!
//! ```json
//! "prism_engine::scene::TimeOfDay": { "hours": 8.0, "day_length_seconds": 600.0 },
//! "prism_engine::scene::Light2D": { "kind": "Ambient", "color": [1, 1, 1], "intensity": 1 }
//! ```
//!
//! System does two things each frame, fixed order:
//!
//! 1. Advance `hours` by `dt` (no change when `paused` / `day_length_seconds <= 0`);
//! 2. Use color from [`TimeOfDay::ambient`] to overwrite the `Light2DKind::Ambient`
//!    [`Light2D`] on the same entity.
//!
//! It only touches ambient light, point lights (lantern, campfire) keep author's intensity — at night point lights naturally stand out.
//! Within same frame `sprite_light_entries` reads post-advance value, so no one-frame delay on screen.

use crate::ecs::World;
use prism_scene::components::{Light2D, Light2DKind, TimeOfDay};

/// Advance day/night and refresh ambient light.
pub fn time_of_day_system(world: &mut World, dt: f32) {
    let mut ambient: Vec<(crate::ecs::Entity, [f32; 3], f32)> = Vec::new();
    for (entity, clock) in world.query_mut::<TimeOfDay>() {
        clock.advance(dt);
        let (color, intensity) = clock.ambient();
        ambient.push((entity, color, intensity));
    }

    for (entity, color, intensity) in ambient {
        let Some(light) = world.get_mut::<Light2D>(entity) else {
            continue;
        };
        if light.kind != Light2DKind::Ambient {
            continue;
        }
        light.color = color;
        light.intensity = intensity;
        light.enabled = true;
    }
}

#[cfg(test)]
#[path = "time_of_day_tests.rs"]
mod time_of_day_tests;
