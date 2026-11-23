use super::*;

const EPS: f32 = 1e-4;

fn approx_eq(a: f32, b: f32) -> bool {
    (a - b).abs() < EPS
}
fn approx_eq3(a: [f32; 3], b: [f32; 3]) -> bool {
    approx_eq(a[0], b[0]) && approx_eq(a[1], b[1]) && approx_eq(a[2], b[2])
}

#[test]
fn bake_default_light_dir_matches_runtime_default() {
    let dir = bake_euler_xyz_deg_to_dir(BAKE_DEFAULT_LIGHT_EULER);
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    assert!(approx_eq(len, 1.0), "non-unit dir {dir:?}");
    let inv_sqrt2 = std::f32::consts::FRAC_1_SQRT_2;
    assert!(approx_eq(dir[1], inv_sqrt2), "y component {dir:?}");
    assert!(dir[1] > 0.0, "light must be above horizon: {dir:?}");
}

#[test]
fn bake_euler_is_unit_length() {
    for e in [[0.0, 0.0, 0.0], [45.0, -45.0, 0.0], [30.0, 60.0, 17.0]] {
        let d = bake_euler_xyz_deg_to_dir(e);
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        assert!(approx_eq(len, 1.0), "euler {e:?} -> len {len}");
    }
}

#[test]
fn probe_volume_info_size_is_48() {
    assert_eq!(std::mem::size_of::<ProbeVolumeInfo>(), 48);
}

#[test]
fn probe_volume_info_offsets() {
    assert_eq!(std::mem::offset_of!(ProbeVolumeInfo, origin), 0);
    assert_eq!(std::mem::offset_of!(ProbeVolumeInfo, spacing), 16);
    assert_eq!(std::mem::offset_of!(ProbeVolumeInfo, dims), 32);
}

#[test]
fn sh_basis_dc_is_constant() {
    for n in [
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.577, 0.577, 0.577],
    ] {
        assert!(approx_eq(sh_basis(n)[0], SH_C0));
    }
}

#[test]
fn eval_sh9_dc_only_is_direction_independent() {
    let mut sh = [[0.0f32; 3]; SH_COEFF_COUNT];
    sh[0] = [1.0, 2.0, 3.0];
    let expected = [SH_C0, 2.0 * SH_C0, 3.0 * SH_C0];
    for n in [
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [-0.577, 0.577, 0.577],
    ] {
        assert!(approx_eq3(eval_sh9(n, &sh), expected));
    }
}

#[test]
fn eval_sh9_linear_x_term_is_odd() {
    let mut sh = [[0.0f32; 3]; SH_COEFF_COUNT];
    sh[3] = [1.0, 1.0, 1.0];
    let px = eval_sh9([1.0, 0.0, 0.0], &sh)[0];
    let nx = eval_sh9([-1.0, 0.0, 0.0], &sh)[0];
    assert!(approx_eq(px, SH_C1));
    assert!(approx_eq(nx, -SH_C1));
    assert!(approx_eq(eval_sh9([0.0, 1.0, 0.0], &sh)[0], 0.0));
    assert!(approx_eq(eval_sh9([0.0, 0.0, 1.0], &sh)[0], 0.0));
}

#[test]
fn eval_sh9_irradiance_dc_only_scales_by_pi() {
    let pi = std::f32::consts::PI;
    let mut sh = [[0.0f32; 3]; SH_COEFF_COUNT];
    sh[0] = [1.0, 2.0, 3.0];
    for n in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
        let rad = eval_sh9(n, &sh);
        let irr = eval_sh9_irradiance(n, &sh);
        assert!(approx_eq3(irr, [rad[0] * pi, rad[1] * pi, rad[2] * pi]));
    }
}

#[test]
fn eval_sh9_irradiance_band1_scales_by_two_thirds_pi() {
    let mut sh = [[0.0f32; 3]; SH_COEFF_COUNT];
    sh[1] = [0.5, 1.0, 1.5];
    let factor = 2.0 * std::f32::consts::PI / 3.0;
    for n in [[0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.3, 0.95]] {
        let rad = eval_sh9(n, &sh);
        let irr = eval_sh9_irradiance(n, &sh);
        assert!(approx_eq3(
            irr,
            [rad[0] * factor, rad[1] * factor, rad[2] * factor]
        ));
    }
}

#[test]
fn world_to_probe_coord_maps_origin_and_spacing() {
    let info = ProbeVolumeInfo::new([10.0, 0.0, -5.0], [2.0, 2.0, 2.0], [4, 4, 4]);
    assert!(approx_eq3(
        world_to_probe_coord([10.0, 0.0, -5.0], &info),
        [0.0, 0.0, 0.0]
    ));
    assert!(approx_eq3(
        world_to_probe_coord([12.0, 0.0, -5.0], &info),
        [1.0, 0.0, 0.0]
    ));
    assert!(approx_eq3(
        world_to_probe_coord([13.0, 4.0, -5.0], &info),
        [1.5, 2.0, 0.0]
    ));
}

#[test]
fn trilinear_weights_sum_to_one() {
    let dims = [5u32, 4, 3];
    for coord in [
        [0.0, 0.0, 0.0],
        [1.5, 2.25, 0.75],
        [4.0, 3.0, 2.0],
        [2.9, 1.1, 1.9],
    ] {
        let (_, w) = trilinear_weights(coord, dims);
        let sum: f32 = w.iter().sum();
        assert!(approx_eq(sum, 1.0), "weights sum {sum} for coord {coord:?}");
    }
}

#[test]
fn trilinear_weights_at_grid_origin() {
    let (base, w) = trilinear_weights([0.0, 0.0, 0.0], [4, 4, 4]);
    assert_eq!(base, [0, 0, 0]);
    assert!(approx_eq(w[0], 1.0));
    for weight in &w[1..8] {
        assert!(approx_eq(*weight, 0.0));
    }
}

#[test]
fn trilinear_weights_at_far_corner() {
    let (base, w) = trilinear_weights([3.0, 3.0, 3.0], [4, 4, 4]);
    assert_eq!(base, [2, 2, 2]);
    assert!(approx_eq(w[7], 1.0));
    for weight in &w[..7] {
        assert!(approx_eq(*weight, 0.0));
    }
}

#[test]
fn trilinear_weights_at_cell_midpoint() {
    let (base, w) = trilinear_weights([0.5, 0.5, 0.5], [4, 4, 4]);
    assert_eq!(base, [0, 0, 0]);
    for weight in &w {
        assert!(approx_eq(*weight, 0.125));
    }
}

#[test]
fn trilinear_weights_clamp_outside_volume() {
    let (base, w) = trilinear_weights([100.0, -50.0, 999.0], [4, 4, 4]);
    assert_eq!(base, [2, 0, 2]);
    assert!(approx_eq(w[5], 1.0));
    let sum: f32 = w.iter().sum();
    assert!(approx_eq(sum, 1.0));
}

#[test]
fn trilinear_weights_single_probe_axis() {
    let (base, w) = trilinear_weights([3.7, -2.0, 0.5], [1, 1, 1]);
    assert_eq!(base, [0, 0, 0]);
    assert!(approx_eq(w[0], 1.0));
}

#[test]
fn sample_uniform_field_is_position_and_normal_independent() {
    let info = ProbeVolumeInfo::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [4, 4, 4]);
    let dc = [0.5, 0.25, 1.0];
    let pi = std::f32::consts::PI;
    let fetch = |_i: i32, _j: i32, _k: i32, c: usize| -> [f32; 3] {
        if c == 0 {
            dc
        } else {
            [0.0, 0.0, 0.0]
        }
    };
    let expected = [dc[0] * SH_C0 * pi, dc[1] * SH_C0 * pi, dc[2] * SH_C0 * pi];
    for world in [[0.0, 0.0, 0.0], [1.5, 2.5, 0.5], [3.0, 3.0, 3.0]] {
        for n in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
            assert!(approx_eq3(
                sample_probe_irradiance(world, n, &info, fetch),
                expected
            ));
        }
    }
}

#[test]
fn sample_linear_field_is_exact() {
    let pi = std::f32::consts::PI;
    let info = ProbeVolumeInfo::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [4, 4, 4]);
    let fetch = |i: i32, _j: i32, _k: i32, c: usize| -> [f32; 3] {
        if c == 0 {
            [i as f32, 0.0, 0.0]
        } else {
            [0.0, 0.0, 0.0]
        }
    };
    let got = sample_probe_irradiance([1.5, 0.7, 2.3], [0.0, 1.0, 0.0], &info, fetch);
    assert!(approx_eq(got[0], 1.5 * SH_C0 * pi), "got {:?}", got);
    assert!(approx_eq(got[1], 0.0));
    assert!(approx_eq(got[2], 0.0));
}
