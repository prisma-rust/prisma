//! Keyframe animation: playhead and writing back `Transform`.
//!
//! Data side (tracks / sampling / endpoint hold) in [`prism_scene::animation`], component is
//! [`KeyframeAnimation`]. This file only handles "where playback is this frame":
//!
//! - Playhead is runtime component [`ClipPlayhead`], **not in** `.scene.json`: same scene
//!   plays from start every load, serialized state never carries over previous run progress.
//! - Sampling only writes channels **with keyframes**; channels without keyframes preserve author value in `transform`
//!   (animating only position will not zero out rotation / scale).
//! - System runs during `Engine::update` scheduling, before `hierarchy_system`, so same frame
//!   `LocalToWorld` / render extraction / camera follow all see post-animation pose.
//! - Clip events (#490): when playhead crosses `AnimationEvent.time`, write to
//!   [`AnimationEventQueue`]; queue rewritten each frame, gameplay systems read in same schedule.
//!   Only forward playback handled; looping clips fire once per loop.
//!
//! Note: `save_scene_state` reads `LocalToWorld`, saving mid-play stores **current
//! pose** not author pose. Consistent with other systems rewriting `LocalTransform` each frame.

use prism_ecs::{Entity, World};

use crate::scene::components::{
    KeyframeAnimation, LocalTransform, SpriteAnimation, SpriteRef, SpriteRenderer,
};

/// Single sprite playhead (seconds). Runtime component, **not written into** `.scene.json`.
///
/// 1. Each entity has own start, no longer using "global time since process start";
/// 2. When `playing` changes false→true resets to 0, so one-shot can replay;
/// 3. `was_playing` only detects false→true edge, pausing keeps current frame.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct SpriteAnimationState {
    /// Time already played (not including `phase`).
    pub time: f32,
    /// Previous frame `SpriteAnimation.playing`, for detecting replay edge.
    pub was_playing: bool,
    /// Whether already initialized: on first seeing entity do not misjudge as "replay".
    pub initialized: bool,
}

/// Advance all sprite animations each frame.
///
/// Frame count priority: `SpriteAnimation.frames` (>0) → row columns − `frame_start`
/// → 1 (when undetermined stay on first frame).
pub fn sprite_animation_system(world: &mut World, dt: f32) {
    let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
    let entities: Vec<prism_ecs::Entity> = world
        .query3::<SpriteRenderer, SpriteAnimation, SpriteRef>()
        .filter(|(_, _, animation, _)| animation.fps > 0.0)
        .map(|(entity, _, _, _)| entity)
        .collect();

    for entity in entities {
        let Some(animation) = world.get::<SpriteAnimation>(entity).cloned() else {
            continue;
        };
        let (total, columns) = {
            let Some(sprite) = world.get::<SpriteRenderer>(entity) else {
                continue;
            };
            let Some(sprite_ref) = world.get::<SpriteRef>(entity) else {
                continue;
            };
            (
                animation_frame_count(&animation, sprite, sprite_ref.size),
                sprite.sheet_columns_or(sprite_ref.size),
            )
        };
        if total <= 1 {
            continue;
        }

        if world.get::<SpriteAnimationState>(entity).is_none() {
            world.insert(entity, SpriteAnimationState::default());
        }
        let Some(index) = world.get_mut::<SpriteAnimationState>(entity).map(|state| {
            if !state.initialized {
                state.initialized = true;
                state.was_playing = animation.playing;
            }
            if animation.playing {
                if !state.was_playing {
                    state.time = 0.0;
                }
                state.time += dt;
                state.was_playing = true;
            } else {
                state.was_playing = false;
            }

            let duration = total as f32 / animation.fps;
            let raw_time = if animation.looped {
                state.time
            } else {
                state.time.min(duration)
            };
            let raw = (raw_time + animation.phase).max(0.0) * animation.fps;
            if animation.looped {
                raw as u32 % total
            } else {
                raw.min((total - 1) as f32) as u32
            }
        }) else {
            continue;
        };

        if let Some(sprite) = world.get_mut::<SpriteRenderer>(entity) {
            sprite.frame_index = animation
                .frame_start
                .saturating_add(index)
                .min(columns.saturating_sub(1));
        }
    }
}

/// Number of frames for this animation (at least 1).
///
/// When `frames > 0` author-specified length; otherwise from `frame_start` to end of row (#584).
fn animation_frame_count(
    animation: &SpriteAnimation,
    sprite: &SpriteRenderer,
    texture: [u32; 2],
) -> u32 {
    let available = sprite
        .sheet_columns_or(texture)
        .saturating_sub(animation.frame_start)
        .max(1);
    if animation.frames > 0 {
        return animation.frames.min(available);
    }
    available
}

/// One entity keyframe playhead (seconds). Runtime component, not written into scene.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct ClipPlayhead {
    /// Time already played (not including [`KeyframeAnimation::phase`]).
    pub time: f32,
    /// For one-shot (`looped == false`) whether already reached end.
    ///
    /// Recomputed each frame, not sticky flag: increasing `duration` or playing another loop naturally returns
    /// to `false`, no extra reset needed.
    pub finished: bool,
}

/// One animation event fired this frame (#490).
#[derive(Debug, Clone, PartialEq)]
pub struct FiredAnimationEvent {
    /// Entity playing this clip.
    pub entity: Entity,
    /// Author-written event name.
    pub name: String,
    /// Event time within clip (seconds).
    pub time: f32,
}

/// Animation event queue: `keyframe_animation_system` **rewrites each frame**.
///
/// Gameplay systems read this resource during `Engine::update` scheduling (it runs before `hierarchy_system`).
/// Empty when no events / no animation entities, never keeps previous frame old events.
#[derive(Debug, Clone, Default)]
pub struct AnimationEventQueue {
    pub events: Vec<FiredAnimationEvent>,
}

impl AnimationEventQueue {
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &FiredAnimationEvent> {
        self.events.iter()
    }
}

/// Fold original playback time into clip local time within `[0, duration]`.
fn local_time_at(raw: f32, duration: f32, looped: bool) -> f32 {
    if duration <= 0.0 || !duration.is_finite() || !raw.is_finite() {
        0.0
    } else if looped {
        raw.rem_euclid(duration)
    } else {
        raw.clamp(0.0, duration)
    }
}

/// Collect events in `(from, to]`; when `include_from` also counts `from` itself.
fn collect_events(
    clip: &KeyframeAnimation,
    from: f32,
    to: f32,
    include_from: bool,
    out: &mut Vec<(String, f32)>,
) {
    if !from.is_finite() || !to.is_finite() || to < from {
        return;
    }
    for event in &clip.events {
        if event.name.is_empty() || !event.time.is_finite() {
            continue;
        }
        let in_range = if include_from {
            event.time >= from && event.time <= to
        } else {
            event.time > from && event.time <= to
        };
        if in_range {
            out.push((event.name.clone(), event.time));
        }
    }
}

/// Advance all [`KeyframeAnimation`] each frame, write sampling result to entity `LocalTransform`.
///
/// Empty clips (no keyframes in all three tracks) get no playhead, avoid garbage component on pure static entity.
pub fn keyframe_animation_system(world: &mut World, dt: f32) {
    let entities: Vec<Entity> = world
        .query::<KeyframeAnimation>()
        .map(|(entity, _)| entity)
        .collect();

    let mut fired: Vec<FiredAnimationEvent> = Vec::new();

    for entity in entities {
        let computed = world.get::<KeyframeAnimation>(entity).and_then(|clip| {
            if clip.is_empty() {
                return None;
            }
            let previous_head = world
                .get::<ClipPlayhead>(entity)
                .copied()
                .unwrap_or_default();
            let mut head = previous_head;
            let duration = clip.clip_duration();
            let speed = if clip.speed.is_finite() {
                clip.speed
            } else {
                1.0
            };
            if clip.playing && dt.is_finite() {
                head.time += dt * speed;
                let last = duration - clip.phase;
                if !clip.looped && last > 0.0 {
                    head.time = head.time.min(last);
                }
            }
            let raw = head.time + clip.phase;
            head.finished = duration > 0.0 && !clip.looped && raw >= duration;
            let local = local_time_at(raw, duration, clip.looped);

            let mut events = Vec::new();
            if clip.playing && duration > 0.0 && dt.is_finite() {
                let previous_raw = previous_head.time + clip.phase;
                let previous_local = local_time_at(previous_raw, duration, clip.looped);
                if speed >= 0.0 {
                    let cycles = (raw / duration).floor() - (previous_raw / duration).floor();
                    if clip.looped && cycles >= 1.0 {
                        collect_events(clip, previous_local, duration, false, &mut events);
                        collect_events(clip, 0.0, local, true, &mut events);
                    } else {
                        let include_start =
                            previous_head.time == 0.0 && previous_raw <= 0.0 && clip.phase == 0.0;
                        collect_events(clip, previous_local, local, include_start, &mut events);
                    }
                } else {
                    let cycles = (previous_raw / duration).floor() - (raw / duration).floor();
                    if clip.looped && cycles >= 1.0 {
                        collect_events(clip, 0.0, previous_local, true, &mut events);
                        collect_events(clip, local, duration, false, &mut events);
                    } else {
                        collect_events(clip, local, previous_local, false, &mut events);
                    }
                }
            }

            Some((head, clip.sample(local), events))
        });

        let Some((head, sample, events)) = computed else {
            continue;
        };
        for event in events {
            fired.push(FiredAnimationEvent {
                entity,
                name: event.0,
                time: event.1,
            });
        }
        let Some(transform) = world.get_mut::<LocalTransform>(entity) else {
            continue;
        };
        if let Some(position) = sample.position {
            transform.position = glam::Vec3::from_array(position);
        }
        if let Some(euler) = sample.euler_xyz {
            let [x, y, z] = euler.map(f32::to_radians);
            transform.rotation = glam::Quat::from_euler(glam::EulerRot::XYZ, x, y, z);
        }
        if let Some(scale) = sample.scale {
            transform.scale = glam::Vec3::from_array(scale);
        }
        world.insert(entity, head);
    }

    world.insert_resource(AnimationEventQueue { events: fired });
}

#[cfg(test)]
#[path = "animation_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "keyframe_animation_tests.rs"]
mod keyframe_tests;
