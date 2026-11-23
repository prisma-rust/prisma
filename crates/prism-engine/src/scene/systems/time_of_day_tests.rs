// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

use super::*;
use prism_scene::components::{Light2D, Light2DKind, TimeOfDay, TimeOfDayPhase};

fn clock(hours: f32) -> TimeOfDay {
    TimeOfDay {
        hours,
        ..TimeOfDay::default()
    }
}

#[test]
fn advancing_wraps_at_midnight() {
    let mut clock = TimeOfDay {
        hours: 23.0,
        day_length_seconds: 24.0,
        ..TimeOfDay::default()
    };
    clock.advance(2.0);
    assert!(
        (clock.hours - 1.0).abs() < 1e-4,
        "23:00 + 2h → 01:00, actual {}",
        clock.hours
    );
}

#[test]
fn paused_or_zero_length_clocks_do_not_move() {
    let mut paused = TimeOfDay {
        hours: 9.0,
        paused: true,
        ..TimeOfDay::default()
    };
    paused.advance(60.0);
    assert_eq!(paused.hours, 9.0);

    let mut frozen = TimeOfDay {
        hours: 9.0,
        day_length_seconds: 0.0,
        ..TimeOfDay::default()
    };
    frozen.advance(60.0);
    assert_eq!(frozen.hours, 9.0);
}

#[test]
fn clock_text_pads_hours_and_minutes() {
    assert_eq!(clock(6.5).clock_text(), "06:30");
    assert_eq!(clock(0.0).clock_text(), "00:00");
    // Author writing -3 / 30 also readable, normalized then reformatted.
    assert_eq!(clock(-3.0).clock_text(), "21:00");
    assert_eq!(clock(30.0).clock_text(), "06:00");
}

#[test]
fn phases_follow_sunrise_and_sunset() {
    assert_eq!(clock(12.0).phase(), TimeOfDayPhase::Day);
    assert_eq!(clock(2.0).phase(), TimeOfDayPhase::Night);
    // Sunrise / sunset themselves lie within transition interval.
    assert_eq!(clock(6.0).phase(), TimeOfDayPhase::Dawn);
    assert_eq!(clock(18.0).phase(), TimeOfDayPhase::Dusk);
}

#[test]
fn ambient_light_is_bright_at_day_and_dark_at_night() {
    let day = clock(12.0);
    let (color, intensity) = day.ambient();
    assert_eq!(color, day.day_color);
    assert_eq!(intensity, day.day_intensity);

    let night = clock(0.0);
    let (color, intensity) = night.ambient();
    assert_eq!(color, night.night_color);
    assert_eq!(intensity, night.night_intensity);

    // Sunrise midpoint should be exactly half day/night (smoothstep midpoint is 0.5).
    let dawn = clock(6.0).ambient();
    assert!(dawn.0[0] < day.day_color[0] && dawn.0[0] > night.night_color[0]);
    assert!((dawn.0[0] - (day.day_color[0] + night.night_color[0]) * 0.5).abs() < 1e-3);

    // Default intensity is 1.0 for both day and night; need "night darker" config to see interpolation.
    let mut dimmer = TimeOfDay {
        hours: 6.0,
        night_intensity: 0.35,
        ..TimeOfDay::default()
    };
    let (_, dawn_intensity) = dimmer.ambient();
    assert!(
        dawn_intensity < dimmer.day_intensity && dawn_intensity > dimmer.night_intensity,
        "Dawn/dusk intensity should be between day and night, actual {dawn_intensity}"
    );
    dimmer.hours = 12.0;
    assert!((dimmer.ambient().1 - dimmer.day_intensity).abs() < 1e-6);
}

#[test]
fn the_system_writes_the_ambient_light_and_leaves_point_lights_alone() {
    let mut world = World::new();
    let sun = world.spawn();
    world.insert(sun, clock(0.0));
    world.insert(
        sun,
        Light2D {
            kind: Light2DKind::Ambient,
            ..Light2D::default()
        },
    );

    let lamp = world.spawn();
    world.insert(lamp, clock(0.0));
    world.insert(
        lamp,
        Light2D {
            kind: Light2DKind::Point,
            color: [1.0, 0.6, 0.2],
            intensity: 3.0,
            radius: 8.0,
            ..Light2D::default()
        },
    );

    time_of_day_system(&mut world, 1.0);

    let sun_light = world.get::<Light2D>(sun).expect("sun light");
    // Midnight: ambient light takes night color.
    assert_eq!(sun_light.color, clock(0.0).night_color);
    assert_eq!(sun_light.intensity, clock(0.0).night_intensity);
    assert!(sun_light.enabled);

    // Point light unaffected (lantern bright at night, relies on ambient light darkening).
    let lamp_light = world.get::<Light2D>(lamp).expect("lamp light");
    assert_eq!(lamp_light.intensity, 3.0);
    assert_eq!(lamp_light.color, [1.0, 0.6, 0.2]);
}
