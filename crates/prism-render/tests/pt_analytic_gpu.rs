//! Analytic micro-scenes for the shared-transport reference variant.
//!
//! Spec §7.2 requires first validating the pipeline with "micro-scenes whose answers can be written in closed form", then using it to judge
//! cache: otherwise reference and consumer share same error and authenticate each other. Each scene here
//! reads `PT_COLOR_H` (R32G32B32A32_SFLOAT, linear radiance before tonemap), not
//! `readback_pixels`: latter is 8-bit, already tonemapped, white furnace 0.25 and 0.5 compressed to
//! 0.19 and 0.31, making quantitative assertions meaningless.
//!
//! Scenes deliberately small. `pt_max_bounces = 1` removes indirect segment, so single-bounce error has nowhere
//! to hide; two-segment throughput test adds one indirect bounce and lands on convex diffuse box surface —
//! cosine sampling on convex outward surface can never hit itself, path length physically pinned to one segment.
//!
//! All scenes use `PtVariant::Reference`: no cache, no ReSTIR, no firefly clamp,
//! uniform light sampling. Bias terms of consumer variants are exactly what these tests must measure, cannot mix in.
#![cfg(all(feature = "pathtrace", feature = "ibl"))]

use prism_render::managers::{MaterialAlphaMode, MaterialUploadInput, MeshHandle};
use prism_render::render_graph::{DrawItem, DrawState, PostGrade, PtGiMode, RenderMode};
use prism_render::{FrameInput, FrameUBOData, GraphRenderer, HdrReadback, PtVariant};

const SIZE: usize = 64;

/// RGBE exponent byte. `hdr::rgbe_to_float` scale is `2^(e - 128 - 8)`, taking 128
/// gives scale exactly 1/256, so mantissa byte `b` decodes to radiance `b / 256` — 0.25,
/// 0.375, 0.5 are all exact f32 values, decode introduces no rounding.
const RGBE_EXPONENT: u8 = 128;

/// Turn off sun: `light_dir.w` is pre-multiplied radiance intensity, taking 0 makes
/// `sun_radiance = light_color.rgb * light_dir.w` zero, NEE's
/// `dot(L_rad, L_rad) > 1e-10` check skips directly.
const SUN_OFF: [f32; 4] = [0.0, 0.0, 1.0, 0.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// Sun propagating only along +Z, intensity written in `w`.
fn sun(intensity: f32) -> [f32; 4] {
    [0.0, 0.0, 1.0, intensity]
}

/// Build a Radiance RGBE image where every pixel equals `radiance`.
///
/// `radiance` must be integer multiple of 1/256 (above exponent convention), and converted mantissa byte
/// cannot equal 2 — when scanner sees two bytes `0x02 0x02` decoder treats as RLE, while we
/// need an uncompressed literal scanline.
fn constant_environment(radiance: f32) -> Vec<u8> {
    let mantissa = (radiance * 256.0).round() as u8;
    assert!(
        (f32::from(mantissa) / 256.0 - radiance).abs() < 1e-6,
        "radiance {radiance} is not a multiple of 1/256"
    );
    assert_ne!(mantissa, 2, "byte 2 would be decoded as an RLE scanline");

    let mut bytes = Vec::with_capacity(64 + SIZE * SIZE * 4);
    bytes.extend_from_slice(b"#?RADIANCE\n");
    bytes.extend_from_slice(b"FORMAT=32-bit_rle_rgbe\n");
    bytes.push(b'\n');
    bytes.extend_from_slice(format!("-Y {SIZE} +X {SIZE}\n").as_bytes());
    for _ in 0..SIZE * SIZE {
        bytes.extend_from_slice(&[mantissa, mantissa, mantissa, RGBE_EXPONENT]);
    }
    bytes
}

fn try_create(env: Option<Vec<u8>>) -> Option<GraphRenderer> {
    let mut renderer = match GraphRenderer::offscreen_new(SIZE as u32, SIZE as u32, env) {
        Ok(renderer) => renderer,
        Err(err) => {
            let message = format!("{err:#}");
            if message.contains("VK_ERROR_INCOMPATIBLE_DRIVER")
                || message.contains("Unable to find a Vulkan driver")
                || message.contains("VK_ERROR_EXTENSION_NOT_PRESENT")
                || message.contains("no devices")
                || message.contains("No valid")
                || message.contains("PhysicalDevice")
            {
                eprintln!("[SKIP] no usable Vulkan device: {message}");
                return None;
            }
            panic!("failed to create the offscreen renderer: {message}");
        }
    };
    // Without ray query there's no `PathTracePass`, whole test suite has nowhere to land.
    if !renderer.context().rt_caps.has_ray_query() {
        eprintln!("[SKIP] no VK_KHR_ray_query support");
        return None;
    }
    renderer.set_pt_variant(PtVariant::Reference);
    Some(renderer)
}

fn register_box(renderer: &mut GraphRenderer) -> MeshHandle {
    renderer
        .register_mesh(&prism_render::primitives::builtin("box").expect("builtin box"))
        .expect("register box")
}

fn draw_item(mesh: MeshHandle, model: [[f32; 4]; 4], material: u32, entity_id: u32) -> DrawItem {
    DrawItem {
        mesh,
        entity_id,
        model,
        material: Some(material),
        cast_shadow: false,
        visible_in_main_pass: true,
        cast_shadow_two_sided: false,
        state: DrawState::default(),
        world_bounds: None,
        index_start: 0,
        index_count: None,
        bone_offset: None,
        splat_slots: [u32::MAX; 4],
        splat_tiling: 0.0,
        shader: None,
    }
}

/// Register a material and return its SSBO slot. SSBO uploaded wholesale, so must
/// `flush_materials` before path tracing can read it.
fn upload_material(renderer: &mut GraphRenderer, input: MaterialUploadInput) -> u32 {
    let handle = renderer
        .register_material(input)
        .expect("register material");
    let slot = renderer.material_slot(handle).expect("material slot");
    renderer.flush_materials().expect("flush materials");
    slot
}

/// Box behind camera. Main raycast can never hit it, only reason it exists is to give
/// `PathTracePass` geometry: with no instances `pt_scene` empty, entire PT dispatch
/// skipped, `PT_COLOR_H` never published.
fn behind_camera_box(renderer: &mut GraphRenderer) -> Vec<DrawItem> {
    let mesh = register_box(renderer);
    let slot = upload_material(
        renderer,
        MaterialUploadInput {
            base_color: [0.0, 0.0, 0.0, 1.0],
            metallic: 0.0,
            roughness: 1.0,
            alpha_mode: MaterialAlphaMode::Opaque,
            ..Default::default()
        },
    );
    vec![draw_item(
        mesh,
        translation_scale(0.0, 0.0, 4.0, 2.0),
        slot,
        1,
    )]
}

/// Diffuse box 5 meters in front of camera (half extent 1, ~48% of image height, center pixel
/// certainly hits front face). `roughness = 1.0` makes bounce 0 scatter always take cosine branch: source has
/// `prob_diffuse = surf.roughness`, only this path's throughput and sample direction matter.
fn diffuse_box(renderer: &mut GraphRenderer, albedo: f32) -> Vec<DrawItem> {
    let mesh = register_box(renderer);
    let slot = upload_material(
        renderer,
        MaterialUploadInput {
            base_color: [albedo, albedo, albedo, 1.0],
            metallic: 0.0,
            roughness: 1.0,
            alpha_mode: MaterialAlphaMode::Opaque,
            ..Default::default()
        },
    );
    vec![draw_item(
        mesh,
        translation_scale(0.0, 0.0, -6.0, 2.0),
        slot,
        1,
    )]
}

/// Emissive-only box. `base_color` fully black, so reflection term zero, screen should not show
/// any non-emissive source. Note not calling `set_emissive`: that would register triangles into glow triangle list,
/// opening extra NEE pass, while we test material's own emissive term.
fn emissive_box(renderer: &mut GraphRenderer, emission: f32) -> Vec<DrawItem> {
    let mesh = register_box(renderer);
    let slot = upload_material(
        renderer,
        MaterialUploadInput {
            base_color: [0.0, 0.0, 0.0, 1.0],
            metallic: 0.0,
            roughness: 1.0,
            // `to_gpu` only sends emissive[0] into SSBO, and in shader
            // `s.emissive = ei * es`, so this value is expected radiance.
            emissive: [emission, 0.0, 0.0],
            emissive_strength: 1.0,
            alpha_mode: MaterialAlphaMode::Opaque,
            ..Default::default()
        },
    );
    vec![draw_item(
        mesh,
        translation_scale(0.0, 0.0, -6.0, 2.0),
        slot,
        1,
    )]
}

/// Two front-back swapped diffuse boxes. Scattered raycast may hit other box, so
/// bounce 1+ truly hits surface — the only geometry shape able to make SHaRC hash lookup actually run
/// (single convex box means indirect segment always misses).
fn two_box_scene(renderer: &mut GraphRenderer, albedo: f32) -> Vec<DrawItem> {
    let mesh = register_box(renderer);
    let slot = upload_material(
        renderer,
        MaterialUploadInput {
            base_color: [albedo, albedo, albedo, 1.0],
            metallic: 0.0,
            roughness: 1.0,
            alpha_mode: MaterialAlphaMode::Opaque,
            ..Default::default()
        },
    );
    vec![
        draw_item(mesh, translation_scale(0.0, 0.0, -4.0, 2.0), slot, 1),
        draw_item(mesh, translation_scale(0.0, -2.4, -6.0, 2.0), slot, 2),
    ]
}

fn perspective() -> [[f32; 4]; 4] {
    let fov = 45.0f32.to_radians();
    let near = 0.1f32;
    let far = 100.0f32;
    let f = 1.0 / (fov * 0.5).tan();
    let mut m = [[0.0f32; 4]; 4];
    m[0][0] = f;
    m[1][1] = -f;
    m[2][2] = far / (near - far);
    m[2][3] = -1.0;
    m[3][2] = (near * far) / (near - far);
    m
}

fn translation_scale(x: f32, y: f32, z: f32, scale: f32) -> [[f32; 4]; 4] {
    [
        [scale, 0.0, 0.0, 0.0],
        [0.0, scale, 0.0, 0.0],
        [0.0, 0.0, scale, 0.0],
        [x, y, z, 1.0],
    ]
}

fn frame_data(
    view_proj: [[f32; 4]; 4],
    sun_direction: [f32; 4],
    sun_color: [f32; 4],
) -> FrameUBOData {
    FrameUBOData {
        view_proj,
        camera_position: [0.0, 0.0, 0.0, 0.0],
        light_direction: sun_direction,
        light_color: sun_color,
        view: view_proj,
        light_view_proj: view_proj,
        light_view_proj1: view_proj,
        shadow_split: 0.0,
        shadow_cascade_count: 1,
        tonemap_mode: 2,
        viewport_w: SIZE as f32,
        viewport_h: SIZE as f32,
        exposure: 1.0,
        shadow_strength: 0.0,
        prev_view_proj: view_proj,
        ..Default::default()
    }
}

/// Run one frame path tracing and read `PT_COLOR_H` back as linear f32.
///
/// `request_hdr_readback` must be before `execute` (copy recorded into that frame's command
/// buffer), `take_hdr_readback` must be after `present` (headless present
/// waits frame fence, only then mapping VRAM is safe).
fn render_pt_frame(
    renderer: &mut GraphRenderer,
    items: &[DrawItem],
    sun_direction: [f32; 4],
    sun_color: [f32; 4],
    pt_max_bounces: u32,
    reset: bool,
    readback: bool,
) -> Option<HdrReadback> {
    let view_proj = perspective();
    let ctx = renderer.begin_frame().expect("begin_frame").expect("frame");
    let data = frame_data(view_proj, sun_direction, sun_color);
    let input = FrameInput {
        draw_items: items,
        gizmos: &[],
        sprite_items: &[],
        sprite_lights: &[],
        particle_items: &[],
        bone_matrices: &[],
        skybox: None,
        frame_data: &data,
        cameras: &[],
        light_view_proj: view_proj,
        light_view_proj1: view_proj,
        shadow_split: 0.0,
        shadow_cascade_count: 1,
        inv_projection: view_proj,
        debug_flags: 0,
        wireframe: false,
        shaded_wireframe: false,
        tonemap_mode: 2,
        aa_mode: 0,
        debug_rt: 0,
        grade: PostGrade::default(),
        lut_path: None,
        proj22: view_proj[2][2],
        proj32: view_proj[3][2],
        lights: &[],
        render_mode: RenderMode::PathTrace,
        pt_max_bounces,
        pt_ray_max_distance: 100.0,
        // Don't freeze: one sample per frame enough — every scene's estimator below is zero variance
        // (constant environment, constant emissive, single NEE), no convergence noise to store.
        pt_max_iterations: 0,
        // `result *= push.camera_pos.w`: exposure multiplied at output, parsed value only equals radiance itself when
        // exposure == 1.
        exposure: 1.0,
        pt_lights: &[],
        // `reset = true` accumulates from zero, so "one frame == one Monte Carlo sample"; false
        // lets accumulation converge across frames, for low-variance quantitative comparisons.
        pt_accum_dirty: reset,
        has_camera: true,
        camera_orthographic: false,
        clear_color: [0.0, 0.0, 0.0, 1.0],
        shadows_enabled: false,
        ui_overlay: None,
        show_grid: false,
        dispatch_plan: None,
    };
    if readback {
        renderer.request_hdr_readback();
    }
    renderer.prepare(&ctx, &input).expect("prepare");
    renderer.execute(&ctx, &input).expect("execute");
    let out_of_date = renderer.present(&ctx).expect("present");
    renderer.end_frame(&ctx, out_of_date).expect("end_frame");
    if !readback {
        return None;
    }
    Some(
        renderer
            .take_hdr_readback()
            .expect("take_hdr_readback")
            .expect("the path tracer did not publish PT_COLOR_H this frame"),
    )
}

/// Single frame, zero accumulation, read PT_COLOR_H back.
fn trace_frame(
    renderer: &mut GraphRenderer,
    items: &[DrawItem],
    sun_direction: [f32; 4],
    sun_color: [f32; 4],
    pt_max_bounces: u32,
) -> HdrReadback {
    render_pt_frame(
        renderer,
        items,
        sun_direction,
        sun_color,
        pt_max_bounces,
        true,
        true,
    )
    .expect("frame published PT_COLOR_H")
}

/// Accumulate `samples` frames then read back: only first frame zeros accumulation, so read-back value is mean of
/// `samples` samples (`PT_COLOR_H` stores running mean).
fn accumulate(
    renderer: &mut GraphRenderer,
    items: &[DrawItem],
    sun_direction: [f32; 4],
    sun_color: [f32; 4],
    pt_max_bounces: u32,
    samples: usize,
) -> HdrReadback {
    let mut hdr = None;
    for sample in 0..samples {
        let last = sample + 1 == samples;
        hdr = render_pt_frame(
            renderer,
            items,
            sun_direction,
            sun_color,
            pt_max_bounces,
            sample == 0,
            last,
        );
    }
    hdr.expect("frame published PT_COLOR_H")
}

/// Min and max of entire image (red channel). Constant scene should degrade to min == max.
fn value_range(hdr: &HdrReadback) -> (f32, f32) {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for y in 0..hdr.height {
        for x in 0..hdr.width {
            let value = hdr.pixel(x, y).expect("in-range pixel")[0];
            lo = lo.min(value);
            hi = hi.max(value);
        }
    }
    (lo, hi)
}

fn assert_uniform(hdr: &HdrReadback, expected: f32, what: &str) {
    assert!(
        !hdr.has_non_finite(),
        "{what}: readback contains {} non-finite texels",
        hdr.non_finite_count()
    );
    let (lo, hi) = value_range(hdr);
    assert!(
        (lo - expected).abs() < 1e-4 && (hi - expected).abs() < 1e-4,
        "{what}: radiance range [{lo}, {hi}] != {expected}"
    );
}

#[test]
fn constant_environment_reads_back_linearly() {
    // White furnace: constant environment 0.25, nothing in scene blocks raycast, every pixel should only see environment.
    let Some(mut renderer) = try_create(Some(constant_environment(0.25))) else {
        return;
    };
    let items = behind_camera_box(&mut renderer);

    let quarter = trace_frame(&mut renderer, &items, SUN_OFF, WHITE, 1);
    assert_uniform(&quarter, 0.25, "furnace at 0.25");

    // Same scene with doubled radiance.
    renderer
        .set_environment(Some(constant_environment(0.5)))
        .expect("set_environment");
    let half = trace_frame(&mut renderer, &items, SUN_OFF, WHITE, 1);
    assert_uniform(&half, 0.5, "furnace at 0.5");

    // Linearity: doubled radiance, read-back must double. This assertion proves "read back before tonemap" —
    // post-tonemap 8-bit path compresses 0.25 and 0.5 to about 0.19 and 0.31,
    // ratio only 1.6x.
    let ratio = half.mean_luminance() / quarter.mean_luminance();
    assert!(
        (ratio - 2.0).abs() < 1e-3,
        "hdr readback is not linear: 0.5 / 0.25 read back as {ratio}"
    );
}

#[test]
fn two_segment_throughput_matches_the_analytic_value() {
    // Constant environment 0.375 (mantissa byte 96). Use radiance value different from above, so IBL disk
    // cache hash differs — two tests running in parallel won't fight for same cache file.
    let Some(mut renderer) = try_create(Some(constant_environment(0.375))) else {
        return;
    };
    let albedo = 0.5;
    let items = diffuse_box(&mut renderer, albedo);

    // Two segments: main raycast → box surface (one scatter) → miss → environment.
    let hdr = trace_frame(&mut renderer, &items, SUN_OFF, WHITE, 2);
    assert!(!hdr.has_non_finite(), "two-segment path produced NaN/Inf");

    // Background: zero-segment scatter, result is environment radiance itself (miss propagation).
    let corner = hdr.pixel(0, 0).expect("corner pixel")[0];
    assert!(
        (corner - 0.375).abs() < 1e-4,
        "background {corner} != 0.375"
    );

    // Front face: one diffuse scatter then miss. Per-segment throughput is
    // (brdf·cos) / pdf; with roughness = 1 pdf always cosine pdf (cos/π), so
    //   throughput = (albedo/π + specular)·cos / (cos/π) = albedo + π·specular.
    // With normal incidence D = 1/π, V = 0.25, F = 0.1024, therefore π·specular = 0.1024·0.25
    // = 0.0256. Expected value of whole path is (albedo + 0.0256) · 0.375.
    let expected = (albedo + 0.0256) * 0.375;
    let face = hdr
        .pixel(SIZE as u32 / 2, SIZE as u32 / 2)
        .expect("centre pixel")[0];
    let relative = (face - expected).abs() / expected;
    assert!(
        relative < 0.05,
        "diffuse face read {face}, analytic value {expected} ({relative:.1}% off)"
    );

    // Convex outward cosine sampling can never hit itself, so raising bounce count shouldn't
    // change any pixel: extra segments are all empty.
    let deep = trace_frame(&mut renderer, &items, SUN_OFF, WHITE, 4);
    let deep_face = deep
        .pixel(SIZE as u32 / 2, SIZE as u32 / 2)
        .expect("centre pixel")[0];
    assert!(
        (deep_face - face).abs() < 1e-4,
        "a convex scene must terminate after one indirect segment: {face} -> {deep_face}"
    );
    let (lo, hi) = value_range(&deep);
    assert!(lo >= 0.0, "negative radiance {lo}");
    assert!(hi < 1e3, "runaway radiance {hi}");
}

#[test]
fn pure_emission_matches_the_material() {
    // Black environment (`new_empty` is 1x1x6 all-zero cube), only light source in whole scene is material's own
    // emissive term.
    let Some(mut renderer) = try_create(None) else {
        return;
    };
    let emission = 0.6f32;
    let items = emissive_box(&mut renderer, emission);

    // pt_max_bounces = 1: bounce 0 only accumulates emissive, then NEE (sun off,
    // no glow triangles), loop never entered. Expected value is emissive itself.
    let hdr = trace_frame(&mut renderer, &items, SUN_OFF, WHITE, 1);
    assert!(!hdr.has_non_finite(), "emissive scene produced NaN/Inf");

    let centre = hdr
        .pixel(SIZE as u32 / 2, SIZE as u32 / 2)
        .expect("centre pixel");
    assert!(
        (centre[0] - emission).abs() < 1e-3,
        "emissive face read {} instead of {emission}",
        centre[0]
    );
    // `GpuMaterial` only packs `emissive[0]` into SSBO (`metallic_roughness_emissive.z`),
    // then multiplies `emissive_strength` so entire RGB takes effect — glow body is gray body, all three channels
    // equal same scalar. This assertion pins "gray body" fact onto interface.
    for (channel, value) in [("green", centre[1]), ("blue", centre[2])] {
        assert!(
            (value - emission).abs() < 1e-3,
            "{channel} channel read {value} instead of {emission}: the emissive term is scalar",
        );
    }

    // Box ~48% of image height, corner raycast hits black environment, must be strictly 0.
    let corner = hdr.pixel(0, 0).expect("corner pixel")[0];
    assert!(corner.abs() < 1e-6, "background {corner} is not black");
}

#[test]
fn directional_light_is_linear_in_radiance() {
    let Some(mut renderer) = try_create(None) else {
        return;
    };
    let albedo = 0.5;
    let items = diffuse_box(&mut renderer, albedo);

    let one = trace_frame(&mut renderer, &items, sun(1.0), WHITE, 1);
    let two = trace_frame(&mut renderer, &items, sun(2.0), WHITE, 1);

    // Single directional light, pt_max_bounces = 1: only light transport term is bounce 0 NEE.
    // L_dir same direction as +Z face normal (n_dot_l = 1), L_rad = light_color.rgb · 1,
    // occlusion raycast cast to outside side of box, necessarily unoccluded, so
    //   L = (albedo/π + F·D·V) · intensity
    // where F = 0.1024 (metallic 0), D = 1/π, V = 0.25.
    let brdf = albedo / std::f32::consts::PI + 0.1024 * 0.25 / std::f32::consts::PI;
    let centre = one
        .pixel(SIZE as u32 / 2, SIZE as u32 / 2)
        .expect("centre pixel")[0];
    let relative = (centre - brdf).abs() / brdf;
    assert!(
        relative < 0.05,
        "single directional light read {centre}, analytic value {brdf} ({relative:.1}% off)"
    );

    // Linearity: doubled intensity, radiance doubles. This assertion guards "NEE missing or extra 1/pdf
    // factor" class of errors — uniform sampling one light's 1/pdf is exactly 1.
    let centre_two = two
        .pixel(SIZE as u32 / 2, SIZE as u32 / 2)
        .expect("centre pixel")[0];
    let ratio = centre_two / centre;
    assert!(
        (ratio - 2.0).abs() < 1e-2,
        "direct lighting is not linear in radiance: {ratio}"
    );
    assert!(!one.has_non_finite() && !two.has_non_finite());
}

#[test]
fn a_degenerate_sharc_hash_does_not_poison_the_image() {
    // Last item of §7.2: "NaN/Inf and empty hash". When hash capacity 0 cache table can't be
    // built, but query bit in push constants still gets set, so shader truly queries a
    // degenerate table. Image must still be finite and non-negative — one out-of-bounds read or NaN propagation would make
    // PT_COLOR_H dirty, which downstream tonemap can't hide.
    let Some(mut renderer) = try_create(Some(constant_environment(0.25))) else {
        return;
    };
    renderer.settings_mut().pt_gi = PtGiMode::Sharc;
    renderer.settings_mut().sharc_capacity = 0;
    let items = two_box_scene(&mut renderer, 0.5);

    let hdr = trace_frame(&mut renderer, &items, SUN_OFF, WHITE, 4);
    assert!(
        !hdr.has_non_finite(),
        "degenerate hash produced {} non-finite texels",
        hdr.non_finite_count()
    );
    let (lo, hi) = value_range(&hdr);
    assert!(lo >= 0.0, "degenerate hash produced negative radiance {lo}");
    // Only light source is constant environment 0.25; diffuse box only attenuates it, so no pixel should
    // exceed environment itself (plus one GGX highlight point outside).
    assert!(hi < 0.5, "degenerate hash amplified radiance to {hi}");
}

#[test]
fn cache_on_preserves_the_analytic_energy() {
    // Remaining three items of §7.2 measured together here: on cache hit must copy `cached_radiance` then
    // directly `break` (`pt_render.slang:771-810`), cannot add another local NEE; training side
    // judging "this voxel is credible enough" (`SharcUpdateHit` returns false) ends path,
    // cannot double deposited measurable amount; while main raycast miss sky term completely unrelated to cache. Three share same,
    // observable consequence: same scene with cache on/off, frame-level amount must match.
    //
    // SHaRC is biased (voxel resolution, material demodulation, firefly suppression), so §7.2 only requires
    // conventional diffuse ROI relative average luminance error ≤5%, and "no stable double counting" — hit
    // then adding NEE again typical result is doubled indirect term, in this scene showing >30% overall
    // offset, far from 5% threshold.
    let Some(mut renderer) = try_create(Some(constant_environment(0.625))) else {
        return;
    };
    renderer.set_pt_variant(PtVariant::Consume);
    let items = two_box_scene(&mut renderer, 0.5);
    let samples = 16;
    let bounces = 4;

    renderer.settings_mut().pt_gi = PtGiMode::Off;
    let uncached = accumulate(&mut renderer, &items, SUN_OFF, WHITE, bounces, samples);

    // Reverse check: turning off gi_mode these two counters must not move. Without this, below
    // "counter > 0" assertion can't exclude "counter always 1" bad case.
    #[cfg(feature = "sharc")]
    assert_eq!(
        (
            renderer.sharc_train_dispatches(),
            renderer.sharc_resolve_dispatches()
        ),
        (0, 0),
        "gi_mode = Off must not train or resolve the SHaRC cache"
    );

    // Voxel edge length is `2^level / sharc_scene_scale`, and `level` determined by distance from hit point to camera
    // (in this scene 2–6 m → level 1–2). With sceneScale = 5 voxels are 0.4–0.8 m,
    // same order as line segment length between two boxes, so `valid_hit`'s two geometry thresholds pass;
    // factory value 50 gives 4 cm voxels, hits sparse to only a few pixels.
    renderer.settings_mut().pt_gi = PtGiMode::Sharc;
    renderer.settings_mut().sharc_capacity = 1 << 14;
    renderer.settings_mut().sharc_scene_scale = 5.0;
    let cached = accumulate(&mut renderer, &items, SUN_OFF, WHITE, bounces, samples);

    // Cache truly ran, otherwise below quantitative comparison is empty talk. Timers showing
    // "SharcTrain" / "SharcResolve" only means those two GPU ranges were opened
    // (`GraphRenderer::execute` unconditional `begin_pass`), inside range
    // `dispatch_training` may still silently early-return via guard; renderer's own recorded real dispatch
    // count is evidence.
    #[cfg(feature = "sharc")]
    {
        let trained = renderer.sharc_train_dispatches();
        let resolved = renderer.sharc_resolve_dispatches();
        assert!(
            trained > 0,
            "SHaRC never recorded a training dispatch in {samples} frames"
        );
        assert!(
            resolved > 0,
            "SHaRC never recorded a resolve dispatch in {samples} frames"
        );
    }

    for (name, hdr) in [("uncached", &uncached), ("cached", &cached)] {
        assert!(
            !hdr.has_non_finite(),
            "{name}: {} non-finite texels",
            hdr.non_finite_count()
        );
        let (lo, _) = value_range(hdr);
        assert!(lo >= 0.0, "{name}: negative radiance {lo}");

        // Miss propagation: main raycast from image top-left corner grazes past two boxes, only sees constant sky.
        // Cache must not participate here — no matter how long it trained.
        let corner = hdr.pixel(0, 0).expect("corner pixel")[0];
        assert!(
            (corner - 0.625).abs() < 1e-4,
            "{name}: corner sees {corner}, expected the 0.625 sky"
        );
    }

    // Training and parsing truly dispatched is only necessary condition — training runs but no voxel
    // falls into queried position, image still won't change. Real evidence is image: cache hit copies
    // `cached_radiance` and ends path directly, same pixel's estimate must differ from pure path tracing.
    let mut changed = 0usize;
    for y in 0..SIZE as u32 {
        for x in 0..SIZE as u32 {
            let plain = uncached.pixel(x, y).expect("uncached pixel")[0];
            let accelerated = cached.pixel(x, y).expect("cached pixel")[0];
            if (plain - accelerated).abs() > 1e-3 {
                changed += 1;
            }
        }
    }
    assert!(
        changed > 0,
        "the consumer never read the cache: every one of the {SIZE}x{SIZE} pixels is \
         identical with and without SHaRC"
    );

    let uncached_mean = uncached.mean_luminance();
    let cached_mean = cached.mean_luminance();
    let gap = (cached_mean - uncached_mean).abs() / uncached_mean;
    assert!(
        gap < 0.05,
        "the cache moved the frame energy by {:.2}% ({uncached_mean} -> {cached_mean}); \
         a hit that kept paying for the local NEE would show up here as a ~2x indirect term",
        gap * 100.0
    );
}
