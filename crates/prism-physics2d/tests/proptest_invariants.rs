use prism_physics2d::{move_and_slide, raycast_aabb, Aabb2};
use proptest::prelude::*;

fn coord() -> impl Strategy<Value = f32> {
    -1000.0f32..1000.0f32
}

fn positive_extent() -> impl Strategy<Value = f32> {
    0.05f32..50.0f32
}

fn aabb_strategy() -> impl Strategy<Value = Aabb2> {
    (coord(), coord(), coord(), coord()).prop_map(|(x0, y0, x1, y1)| Aabb2::new(x0, y0, x1, y1))
}

proptest! {
    #[test]
    fn aabb_new_orders_bounds(x0 in coord(), y0 in coord(), x1 in coord(), y1 in coord()) {
        let box2 = Aabb2::new(x0, y0, x1, y1);
        prop_assert!(box2.min[0] <= box2.max[0]);
        prop_assert!(box2.min[1] <= box2.max[1]);
    }

    #[test]
    fn aabb_overlaps_is_symmetric(a in aabb_strategy(), b in aabb_strategy()) {
        prop_assert_eq!(a.overlaps(b), b.overlaps(a));
    }

    #[test]
    fn shared_interior_point_implies_overlap(
        a in aabb_strategy(),
        b in aabb_strategy(),
        px in coord(),
        py in coord(),
    ) {
        let p = [px, py];
        if a.contains_point(p) && b.contains_point(p) {
            prop_assert!(a.overlaps(b));
        }
    }

    #[test]
    fn raycast_hit_stays_within_max_distance(
        origin_x in coord(),
        origin_y in coord(),
        dir_x in -10.0f32..10.0f32,
        dir_y in -10.0f32..10.0f32,
        max_dist in 0.01f32..500.0f32,
        bounds in aabb_strategy(),
    ) {
        if let Some(hit) = raycast_aabb([origin_x, origin_y], [dir_x, dir_y], max_dist, bounds) {
            prop_assert!(hit.distance >= 0.0);
            prop_assert!(hit.distance <= max_dist + 1.0e-4);
            let n_len = (hit.normal[0] * hit.normal[0] + hit.normal[1] * hit.normal[1]).sqrt();
            prop_assert!(n_len <= 1.0 + 1.0e-4);
        }
    }

    #[test]
    fn move_and_slide_produces_finite_position(
        sx in -100.0f32..100.0f32,
        sy in -100.0f32..100.0f32,
        hx in positive_extent(),
        hy in positive_extent(),
        dx in -20.0f32..20.0f32,
        dy in -20.0f32..20.0f32,
        obstacles in prop::collection::vec(aabb_strategy(), 0..6),
    ) {
        let result = move_and_slide([sx, sy], [hx, hy], [dx, dy], &obstacles);
        prop_assert!(result.position[0].is_finite());
        prop_assert!(result.position[1].is_finite());
    }
}
