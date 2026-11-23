//! Per-machine golden-frame regression scenarios for the composed render graph.
//!
//! The scene, camera, extent and settings are fixed in `golden_support`; each
//! machine records its own baseline under `target/golden-baselines/` on first
//! run and compares subsequent runs against it. Use `PRISM_UPDATE_GOLDENS=1`
//! to refresh baselines after an intentional rendering change.

mod golden_support;

use golden_support::{
    assert_golden, compare, format_version, write_mismatch_artifacts, FrameHarness, FrameOptions,
};
use prism_render::render_graph::PostGrade;

const SIZE: usize = 256;

fn harness() -> Option<FrameHarness> {
    FrameHarness::new(SIZE)
}

fn golden_harness() -> Option<FrameHarness> {
    let harness = harness()?;
    if harness.is_software_device()
        && std::env::var_os("PRISM_FORCE_GOLDEN").is_none()
        && std::env::var_os("PRISM_UPDATE_GOLDENS").is_none()
    {
        eprintln!(
            "[SKIP] software Vulkan adapter ({}) — no hardware GPU",
            harness.adapter_info()
        );
        return None;
    }
    Some(harness)
}

#[test]
fn baseline_frame_matches_golden() {
    let Some(mut harness) = golden_harness() else {
        return;
    };
    let pixels = harness.render(FrameOptions::default());
    assert_golden("baseline", &pixels, SIZE, &harness.adapter_info());
}

#[test]
fn shaded_wireframe_frame_matches_golden() {
    let Some(mut harness) = golden_harness() else {
        return;
    };
    if !harness.supports_wireframe() {
        eprintln!("[SKIP] fillModeNonSolid not supported by this driver");
        return;
    }
    let pixels = harness.render(FrameOptions {
        shaded_wireframe: true,
        ..FrameOptions::default()
    });
    assert_golden("shaded_wireframe", &pixels, SIZE, &harness.adapter_info());
}

#[test]
fn bloom_frame_matches_golden() {
    let Some(mut harness) = golden_harness() else {
        return;
    };
    let pixels = harness.render(FrameOptions {
        grade: PostGrade {
            bloom_intensity: 0.35,
            bloom_threshold: 0.0,
            ..PostGrade::default()
        },
        ..FrameOptions::default()
    });
    assert_golden("bloom", &pixels, SIZE, &harness.adapter_info());
}

#[test]
fn version_numbers_decode_as_vk_make_version() {
    assert_eq!(
        format_version((1 << 22) | (3 << 12) | 280),
        "1.3.280 (0x00403118)"
    );
}

#[test]
fn adapter_info_names_the_device_and_driver() {
    let Some(harness) = harness() else {
        return;
    };
    let info = harness.adapter_info();
    assert!(info.contains(" | Vulkan API "), "{info}");
    assert!(info.contains(" | driver "), "{info}");
}

/// The failure path must leave actionable artifacts for CI triage.
#[test]
fn mismatch_artifacts_include_actual_and_diff() {
    let expected = vec![0u8; 8 * 8 * 4];
    let mut actual = expected.clone();
    actual[0] = 255;
    actual[1] = 128;

    let diff = compare(&expected, &actual);
    assert_eq!(diff.changed_pixels, 1);
    assert_eq!(diff.max_diff, 255);
    assert!(!diff.is_within_tolerance());

    let dir = write_mismatch_artifacts("selftest", &actual, &expected, 8)
        .expect("write mismatch artifacts");
    assert!(
        dir.join("selftest.actual.png").is_file(),
        "actual artifact missing in {}",
        dir.display()
    );
    assert!(
        dir.join("selftest.diff.png").is_file(),
        "diff artifact missing in {}",
        dir.display()
    );
}
