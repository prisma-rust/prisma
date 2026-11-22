//! Pure data conversion for animation clip timeline and root motion (#560 P0 editor).
//!
//! `.ranim` itself is cooked binary; the editor only does **read-only checks**: timeline
//! coordinates, Notify / sync marker positions, root joint start-to-end displacement. Real
//! editing still goes through the authoring side `notifies.json` / `sync_markers.json` sidecars.

/// Map x coordinate on the timeline to seconds, clamped to `0..=duration` and quantized to 0.01 s.
pub fn timeline_time_at(x: f32, left: f32, width: f32, duration: f32) -> f32 {
    if !x.is_finite()
        || !left.is_finite()
        || !width.is_finite()
        || width <= 0.0
        || !duration.is_finite()
        || duration <= 0.0
    {
        return 0.0;
    }
    let fraction = ((x - left) / width).clamp(0.0, 1.0);
    ((fraction * duration) * 100.0).round() / 100.0
}

/// Map seconds back to timeline x coordinate; out of clip range clamps to both ends.
pub fn timeline_x(time: f32, duration: f32, left: f32, width: f32) -> f32 {
    if !time.is_finite()
        || !duration.is_finite()
        || duration <= 0.0
        || !left.is_finite()
        || !width.is_finite()
        || width <= 0.0
    {
        return left;
    }
    left + (time / duration).clamp(0.0, 1.0) * width
}

/// Root joint start-to-end displacement and yaw around Y (degrees).
///
/// Prisma's forward is `-Z`, so `atan2(dx, -dz)`: walking -Z is 0°, walking +X is +90°.
/// Vertical displacement doesn't participate in yaw.
pub fn root_motion_delta(start: [f32; 3], end: [f32; 3]) -> ([f32; 3], f32) {
    let delta = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
    let yaw = delta[0].atan2(-delta[2]).to_degrees();
    (delta, yaw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_coordinates_map_both_ways_and_clamp() {
        assert_eq!(timeline_time_at(50.0, 0.0, 100.0, 2.0), 1.0);
        assert_eq!(timeline_time_at(-40.0, 0.0, 100.0, 2.0), 0.0);
        assert_eq!(timeline_time_at(400.0, 0.0, 100.0, 2.0), 2.0);
        assert_eq!(timeline_time_at(33.0, 0.0, 100.0, 2.0), 0.66);
        assert_eq!(timeline_time_at(f32::NAN, 0.0, 100.0, 2.0), 0.0);
        assert_eq!(timeline_time_at(10.0, 0.0, 0.0, 2.0), 0.0);
        assert_eq!(timeline_time_at(10.0, 0.0, 100.0, 0.0), 0.0);

        assert_eq!(timeline_x(1.0, 2.0, 0.0, 100.0), 50.0);
        assert_eq!(timeline_x(-1.0, 2.0, 0.0, 100.0), 0.0);
        assert_eq!(timeline_x(9.0, 2.0, 0.0, 100.0), 100.0);
        assert_eq!(timeline_x(f32::NAN, 2.0, 0.0, 100.0), 0.0);
        assert_eq!(timeline_x(1.0, 0.0, 0.0, 100.0), 0.0);
    }

    #[test]
    fn root_motion_reports_delta_and_yaw_with_minus_z_forward() {
        let (delta, yaw) = root_motion_delta([0.0, 0.0, 0.0], [0.0, 0.0, -2.0]);
        assert_eq!(delta, [0.0, 0.0, -2.0]);
        assert!(yaw.abs() < 1e-4, "walking -Z is 0°");

        let (_, yaw) = root_motion_delta([0.0, 0.0, 0.0], [2.0, 0.0, 0.0]);
        assert!((yaw - 90.0).abs() < 1e-4, "walking +X is +90°");

        let (delta, yaw) = root_motion_delta([1.0, 2.0, 3.0], [4.0, 6.0, 5.0]);
        assert_eq!(delta, [3.0, 4.0, 2.0]);
        assert!(
            yaw.is_finite(),
            "vertical displacement should not produce NaN"
        );
    }
}
