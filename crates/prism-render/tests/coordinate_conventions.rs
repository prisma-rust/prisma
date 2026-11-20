//! Golden regression tests pinning Prisma's coordinate & matrix conventions (#1210).
//!
//! Conventions (`GEMINI.md` §4):
//! - Right-handed: camera looks down `-Z`, `+Y` up, `+Z` toward the viewer.
//! - Column-major `[[f32; 4]; 4]` indexed `[col][row]`; `clip = projection * view * model`.
//! - Vulkan y-flip `p[1][1] = -inv_tan(fovy / 2)` with depth `[0, 1]`.
//! - NDC `y = -1` at top, `+1` at bottom.
//! - `GraphFrame::inv_view_rot` is the transpose of `view`'s upper-left 3x3.

use prism_render::graph_renderer::inv_view_rot_from_view;

fn mul_mat4(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for c in 0..4 {
        for r in 0..4 {
            out[c][r] = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    out
}

fn mul_vec4(m: &[[f32; 4]; 4], v: [f32; 4]) -> [f32; 4] {
    let mut out = [0.0f32; 4];
    for r in 0..4 {
        out[r] = (0..4).map(|c| m[c][r] * v[c]).sum();
    }
    out
}

fn vulkan_perspective_rh(fovy_rad: f32, aspect: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
    let inv_tan = 1.0 / (fovy_rad * 0.5).tan();
    let mut p = [[0.0f32; 4]; 4];
    p[0][0] = inv_tan / aspect;
    p[1][1] = -inv_tan;
    p[2][2] = far / (near - far);
    p[2][3] = -1.0;
    p[3][2] = (near * far) / (near - far);
    p
}

#[test]
fn column_major_translation_and_clip_order() {
    let model = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [3.0, 4.0, -10.0, 1.0],
    ];
    let world = mul_vec4(&model, [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(world, [3.0, 4.0, -10.0, 1.0]);

    let proj = vulkan_perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, 1.0, 100.0);
    let view = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let mvp = mul_mat4(&proj, &mul_mat4(&view, &model));
    let clip = mul_vec4(&mvp, [0.0, 0.0, 0.0, 1.0]);
    assert!((clip[3] - 10.0).abs() < 1e-5);
}

#[test]
fn vulkan_y_flip_maps_plus_y_to_top_ndc_and_depth_zero_to_one() {
    let near = 1.0_f32;
    let far = 101.0_f32;
    let proj = vulkan_perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, near, far);
    assert!(proj[1][1] < 0.0);

    let top_clip = mul_vec4(&proj, [0.0, 2.0, -5.0, 1.0]);
    let top_ndc_y = top_clip[1] / top_clip[3];
    assert!(top_ndc_y < 0.0, "+Y world must map to negative NDC y (top)");

    let near_clip = mul_vec4(&proj, [0.0, 0.0, -near, 1.0]);
    let far_clip = mul_vec4(&proj, [0.0, 0.0, -far, 1.0]);
    assert!((near_clip[2] / near_clip[3] - 0.0).abs() < 1e-5);
    assert!((far_clip[2] / far_clip[3] - 1.0).abs() < 1e-5);
}

#[test]
fn inv_view_rot_is_upper_left_transpose_without_translation() {
    let theta = 0.6_f32;
    let (s, c) = theta.sin_cos();
    let view = [
        [c, 0.0, -s, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [s, 0.0, c, 0.0],
        [12.0, -5.0, 30.0, 1.0],
    ];
    let inv = inv_view_rot_from_view(&view);
    assert_eq!(inv[3], [0.0, 0.0, 0.0, 1.0]);

    let mut view_rot = view;
    view_rot[3] = [0.0, 0.0, 0.0, 1.0];
    let prod = mul_mat4(&inv, &view_rot);
    for (col, col_vals) in prod.iter().enumerate() {
        for (row, &val) in col_vals.iter().enumerate() {
            let expected = if col == row { 1.0 } else { 0.0 };
            assert!((val - expected).abs() < 1e-5);
        }
    }
}
