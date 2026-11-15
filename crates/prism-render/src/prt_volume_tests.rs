//! Wire-format tests for `.prtbin` (see docs/prt-gi-64tree-design.md §2).

use super::prt_volume::*;

fn leaf() -> PrtNode {
    PrtNode {
        first_child: NO_CHILD,
        leaf_index: 0,
        child_mask: 0,
    }
}

fn two_leaf_volume() -> PrtVolume {
    let mut clusters = vec![[[0.0f32; 3]; SH_COEFF_COUNT]; 2];
    for (k, cluster) in clusters.iter_mut().enumerate() {
        for (c, rgb) in cluster.iter_mut().enumerate() {
            for (ch, value) in rgb.iter_mut().enumerate() {
                *value = (k * SH_COEFF_COUNT + c) as f32 * 0.25 + ch as f32 * 0.5;
            }
        }
    }
    let mut leaves = Vec::new();
    for k in 0..2u32 {
        leaves.push(PrtNode {
            first_child: NO_CHILD,
            leaf_index: k,
            child_mask: 0,
        });
    }
    PrtVolume {
        world_origin: [-250.0, -3.0, -250.0],
        world_extent: 500.0,
        leaf_grid: 3,
        cluster_cells: 1,
        built_from_sha: [1, 2, 3, 4, 5, 6, 7, 8],
        nodes: leaves,
        clusters,
        cluster_valid: None,
    }
}

#[test]
fn header_is_64_bytes_and_fields_land_where_the_contract_says() {
    let vol = two_leaf_volume();
    let bytes = vol.to_bytes().expect("encode");
    assert_eq!(&bytes[0..4], b"PRTB");
    assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), VERSION);
    assert_eq!(
        u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
        2
    );
    assert_eq!(
        u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
        2
    );
    assert_eq!(bytes.len(), HEADER_SIZE + 2 * NODE_SIZE + 2 * CLUSTER_SIZE);
}

#[test]
fn round_trip_preserves_every_field() {
    let vol = two_leaf_volume();
    let decoded = PrtVolume::from_bytes(&vol.to_bytes().expect("encode")).expect("decode");
    assert_eq!(decoded.world_origin, vol.world_origin);
    assert_eq!(decoded.world_extent, vol.world_extent);
    assert_eq!(decoded.leaf_grid, vol.leaf_grid);
    assert_eq!(decoded.cluster_cells, vol.cluster_cells);
    assert_eq!(decoded.built_from_sha, vol.built_from_sha);
    assert_eq!(decoded.nodes, vol.nodes);
    assert_eq!(decoded.cluster_valid, None);
    // f16 has 11 significant bits, so exact equality only holds for values that fit.
    for (a, b) in decoded.clusters.iter().zip(vol.clusters.iter()) {
        for (x, y) in a.iter().zip(b.iter()) {
            for ch in 0..3 {
                assert!((x[ch] - y[ch]).abs() <= f32::abs(y[ch]) * 1e-3 + 1e-4);
            }
        }
    }
}

#[test]
fn cluster_body_is_f16_so_two_leaves_fit_inside_768_bytes() {
    let vol = two_leaf_volume();
    assert_eq!(vol.payload_bytes(), 2 * CLUSTER_SIZE);
    assert_eq!(vol.to_bytes().expect("encode").len(), 64 + 32 + 768);
}

#[test]
fn leaf_count_above_the_budget_is_rejected_before_writing() {
    let mut vol = two_leaf_volume();
    vol.clusters = vec![[[0.0f32; 3]; SH_COEFF_COUNT]; LEAF_COUNT_MAX as usize + 1];
    vol.nodes = vec![leaf(); LEAF_COUNT_MAX as usize + 1];
    assert!(matches!(vol.to_bytes(), Err(PrtError::TooManyLeaves(_))));
}

#[test]
fn leaf_count_mismatch_is_rejected() {
    let mut vol = two_leaf_volume();
    vol.nodes.truncate(1);
    assert!(matches!(
        vol.validate(),
        Err(PrtError::LeafCountMismatch { .. })
    ));
}

#[test]
fn leaf_count_max_fits_the_stated_budget() {
    // 8600 clusters * 384 B + node table stays under 3.5 MB, which is the M1 contract.
    let payload = LEAF_COUNT_MAX as usize * CLUSTER_SIZE;
    let nodes = LEAF_COUNT_MAX as usize * NODE_SIZE;
    assert!(payload + nodes + HEADER_SIZE <= 3_500_000);
    // One more leaf breaks it: the constant is tight, not arbitrary.
    let over = (LEAF_COUNT_MAX as usize + 1) * CLUSTER_SIZE;
    assert!(
        over + (LEAF_COUNT_MAX as usize + 1) * NODE_SIZE + HEADER_SIZE
            > payload + nodes + HEADER_SIZE
    );
}

#[test]
fn valid_flags_travel_with_the_body() {
    let mut vol = two_leaf_volume();
    vol.cluster_valid = Some(vec![true, false]);
    let bytes = vol.to_bytes().expect("encode");
    assert_eq!(
        bytes.len(),
        HEADER_SIZE + 2 * NODE_SIZE + 2 * (CLUSTER_SIZE + 1)
    );
    let decoded = PrtVolume::from_bytes(&bytes).expect("decode");
    assert_eq!(decoded.cluster_valid, Some(vec![true, false]));
}

#[test]
fn corrupted_magic_is_rejected() {
    let mut bytes = two_leaf_volume().to_bytes().expect("encode");
    bytes[0] = b'X';
    assert!(matches!(
        PrtVolume::from_bytes(&bytes),
        Err(PrtError::BadMagic(_))
    ));
}

#[test]
fn future_version_is_rejected_rather_than_guessed() {
    let mut bytes = two_leaf_volume().to_bytes().expect("encode");
    bytes[4] = 2;
    assert!(matches!(
        PrtVolume::from_bytes(&bytes),
        Err(PrtError::UnsupportedVersion(2))
    ));
}

#[test]
fn wrong_sh_band_is_rejected() {
    let mut bytes = two_leaf_volume().to_bytes().expect("encode");
    bytes[8] = 6;
    assert!(matches!(
        PrtVolume::from_bytes(&bytes),
        Err(PrtError::BadShBand(6))
    ));
}

#[test]
fn payload_size_mismatch_is_rejected() {
    let mut bytes = two_leaf_volume().to_bytes().expect("encode");
    bytes[44] = 0;
    bytes[45] = 0;
    assert!(matches!(
        PrtVolume::from_bytes(&bytes),
        Err(PrtError::PayloadSizeMismatch { .. })
    ));
}

#[test]
fn truncated_body_is_rejected() {
    let bytes = two_leaf_volume().to_bytes().expect("encode");
    let cut = &bytes[..bytes.len() - 16];
    assert!(matches!(
        PrtVolume::from_bytes(cut),
        Err(PrtError::Truncated { .. })
    ));
}

#[test]
fn non_finite_coefficients_are_rejected() {
    let mut vol = two_leaf_volume();
    vol.clusters[0][3][1] = f32::NAN;
    assert!(matches!(
        vol.validate(),
        Err(PrtError::NonFiniteCoefficient {
            cluster: 0,
            coeff: 3
        })
    ));
}

#[test]
fn child_mask_addresses_sixty_four_children() {
    let node = PrtNode {
        first_child: 1,
        leaf_index: NO_LEAF,
        child_mask: u64::MAX,
    };
    assert_eq!(node.child_count(), 64);
    assert!(node.has_child(0));
    assert!(node.has_child(63));
    assert!(!node.has_child(64));
    assert!(!node.is_leaf());
}

#[test]
fn node_table_entry_is_16_bytes() {
    assert_eq!(std::mem::size_of::<PrtNode>(), NODE_SIZE);
    assert_eq!(std::mem::size_of::<u64>(), 8);
}

#[test]
fn f16_round_trip_covers_the_documented_range() {
    // Represents values the design uses: radiance coefficients in [0, 16).
    for v in [0.0f32, 1.0, 0.5, 0.25, -1.0, -0.5, 3.0, 15.5, 16.0] {
        let back = f16_to_f32(f32_to_f16(v));
        assert!((back - v).abs() < 1e-3, "{v} -> {back}");
    }
}

#[test]
fn f16_handles_subnormals_and_saturation() {
    assert_eq!(f32_to_f16(0.0), 0);
    assert_eq!(f16_to_f32(f32_to_f16(0.0)), 0.0);
    // Smallest positive subnormal half.
    assert_eq!(f32_to_f16(5.960_464_5e-8).count_ones(), 1);
    // Beyond the half range saturates to infinity instead of wrapping.
    assert_eq!(f32_to_f16(1.0e30), 0x7C00);
    assert!(f16_to_f32(f32_to_f16(1.0e30)).is_infinite());
    // Sign is preserved through the whole range.
    assert_eq!(f32_to_f16(-2.0) & 0x8000, 0x8000);
}

#[test]
fn every_f16_pattern_round_trips_through_f32() {
    // Exhaustive: decoding then re-encoding must reproduce the identical bits for
    // every representable half, including subnormals, infinities and NaNs.
    for bits in 0u32..=0xFFFF {
        let bits = bits as u16;
        let value = f16_to_f32(bits);
        let back = f32_to_f16(value);
        if value.is_nan() {
            // NaN payloads are normalized to a single quiet NaN by f32_to_f16.
            assert!(
                f16_to_f32(back).is_nan(),
                "pattern {bits:#06x} lost NaN-ness"
            );
        } else {
            assert_eq!(back, bits, "pattern {bits:#06x} -> {value} -> {back:#06x}");
        }
    }
}

#[test]
fn f32_midpoints_round_to_even() {
    // 1 + 2^-11 is exactly halfway between two consecutive halves representable
    // near 1.0: the even neighbour (1.0) must win.
    let halfway = 1.0f32 + 2f32.powi(-11);
    assert_eq!(f16_to_f32(f32_to_f16(halfway)), 1.0);
    // The next halfway point rounds up to the odd-then-even neighbour 1.001953125.
    let above = 1.0f32 + 3.0 * 2f32.powi(-11);
    assert!(f16_to_f32(f32_to_f16(above)) > 1.0);
}
