use glam::Vec3;
use prism_anim_ik::solve_two_bone;
use proptest::prelude::*;

fn coord() -> impl Strategy<Value = f32> {
    -20.0f32..20.0f32
}

fn vec3_strategy() -> impl Strategy<Value = Vec3> {
    (coord(), coord(), coord()).prop_map(|(x, y, z)| Vec3::new(x, y, z))
}

proptest! {
    #[test]
    fn solve_two_bone_preserves_bone_lengths_and_unit_quats(
        upper_len in 0.2f32..5.0f32,
        lower_len in 0.2f32..5.0f32,
        target in vec3_strategy(),
        pole in vec3_strategy(),
        weight in 0.01f32..=1.0f32,
    ) {
        let upper = Vec3::ZERO;
        let middle = Vec3::new(0.0, upper_len, 0.0);
        let end = Vec3::new(0.1, upper_len + lower_len, 0.0);
        let actual_upper = (middle - upper).length();
        let actual_lower = (end - middle).length();

        if let Some(sol) = solve_two_bone(upper, middle, end, target, pole, weight) {
            prop_assert!(sol.upper_rotation.is_finite());
            prop_assert!(sol.middle_rotation.is_finite());
            prop_assert!(sol.middle_position.is_finite());
            prop_assert!(sol.end_position.is_finite());
            prop_assert!((sol.upper_rotation.length() - 1.0).abs() < 1.0e-4);
            prop_assert!((sol.middle_rotation.length() - 1.0).abs() < 1.0e-4);
            if (weight - 1.0).abs() < 1.0e-5 {
                let d1 = (sol.middle_position - upper).length();
                let d2 = (sol.end_position - sol.middle_position).length();
                prop_assert!((d1 - actual_upper).abs() < 1.0e-3, "upper len {d1} != {actual_upper}");
                prop_assert!((d2 - actual_lower).abs() < 1.0e-3, "lower len {d2} != {actual_lower}");
            }
        }
    }
}
