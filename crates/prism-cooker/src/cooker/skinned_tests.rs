use super::*;
use prism_asset::formats_skinned::{
    decode_ranim, decode_rskel, decode_rskin, AnimTrackInfo, JointInfo, JointTransform,
    QuatKeyInfo, SubmeshInfo, Vec3KeyInfo, RANI_NOTIFY_VERSION, RANI_SECTION_VERSION,
    RANI_SYNC_MARKER_VERSION,
};

fn identity_matrix() -> [f32; 16] {
    let mut matrix = [0.0f32; 16];
    for (index, slot) in matrix.iter_mut().enumerate() {
        if index % 5 == 0 {
            *slot = 1.0;
        }
    }
    matrix
}

fn joint(name: &str, parent: Option<u16>) -> JointInfo {
    JointInfo {
        name: name.to_string(),
        parent,
        reference_local: JointTransform::default(),
        inverse_bind: identity_matrix(),
    }
}

fn two_bone_skeleton() -> SkeletonInfo {
    let mut root = joint("root", None);
    root.reference_local.translation = [0.0, 1.0, 0.0];
    let mut spine = joint("spine", Some(0));
    spine.reference_local.translation = [0.0, 0.5, 0.0];
    spine.reference_local.scale = [2.0, 2.0, 2.0];
    SkeletonInfo {
        joints: vec![root, spine],
    }
}

fn quad_mesh() -> SkinnedMeshInfo {
    SkinnedMeshInfo {
        vertex_count: 4,
        index_count: 6,
        joint_count: 2,
        flags: RSKN_HAS_NORMALS | RSKN_HAS_UV0,
        positions: vec![
            [-0.5, 0.0, -0.5],
            [0.5, 0.0, -0.5],
            [-0.5, 0.0, 0.5],
            [0.5, 0.0, 0.5],
        ],
        normals: vec![[0.0, 1.0, 0.0]; 4],
        uv0: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
        tangents: Vec::new(),
        joints: vec![[0, 1, 0, 0]; 4],
        // Deliberately not normalized: should be fixed at encode time.
        weights: vec![[2.0, 2.0, 0.0, 0.0]; 4],
        indices: vec![0, 2, 1, 1, 2, 3],
        submeshes: vec![SubmeshInfo {
            material_index: 0,
            index_offset: 0,
            index_count: 6,
        }],
        bind_pose: JointTransform::default(),
    }
}

fn walk_clip() -> AnimClipInfo {
    AnimClipInfo {
        name: "walk".into(),
        duration: 1.0,
        fps: 30.0,
        looped: true,
        additive: false,
        notifies: Vec::new(),
        sync_markers: Vec::new(),
        sections: Vec::new(),
        warp_windows: Vec::new(),
        tracks: vec![AnimTrackInfo {
            joint: 1,
            // Deliberately out of order: encoder must sort by time.
            positions: vec![
                Vec3KeyInfo {
                    time: 0.5,
                    value: [0.0, 0.2, 0.0],
                },
                Vec3KeyInfo {
                    time: 0.0,
                    value: [0.0, 0.0, 0.0],
                },
            ],
            rotations: vec![QuatKeyInfo {
                time: 0.0,
                // Non-normalized quaternion: decode side will normalize.
                value: [0.0, 0.0, 0.0, 2.0],
            }],
            scales: Vec::new(),
        }],
    }
}

#[test]
fn a_skeleton_round_trips_through_rskel() {
    let skeleton = two_bone_skeleton();
    let bytes = encode_rskel(&skeleton).expect("encode");
    assert_eq!(&bytes[..4], RSKL_MAGIC);
    let decoded = decode_rskel(&bytes).expect("decode");
    assert_eq!(decoded.joint_count(), 2);
    assert_eq!(decoded.joints[0].name, "root");
    assert_eq!(decoded.joints[1].parent, Some(0));
    assert_eq!(decoded.joints[1].reference_local.scale, [2.0, 2.0, 2.0]);
    assert_eq!(decoded.joint_index("spine"), Some(1));
}

#[test]
fn a_skeleton_with_an_out_of_order_parent_is_rejected() {
    let mut skeleton = two_bone_skeleton();
    skeleton.joints[0].parent = Some(1);
    assert!(
        encode_rskel(&skeleton).is_none(),
        "Parent index must come before self"
    );

    // Decode side also blocks: manually change root bone parent to 1.
    let mut bytes = encode_rskel(&two_bone_skeleton()).expect("encode");
    bytes[13..17].copy_from_slice(&1i32.to_le_bytes());
    assert!(decode_rskel(&bytes).is_none());
}

#[test]
fn an_empty_skeleton_cannot_be_encoded() {
    assert!(encode_rskel(&SkeletonInfo { joints: Vec::new() }).is_none());
}

#[test]
fn a_skinned_mesh_round_trips_and_normalises_weights() {
    let mesh = quad_mesh();
    let bytes = encode_rskin(&mesh).expect("encode");
    assert_eq!(&bytes[..4], RSKN_MAGIC);
    let decoded = decode_rskin(&bytes).expect("decode");
    assert_eq!(decoded.vertex_count, 4);
    assert_eq!(decoded.positions[3], [0.5, 0.0, 0.5]);
    assert_eq!(decoded.normals.len(), 4);
    assert!(decoded.has_uv0());
    assert!(!decoded.has_tangents());
    assert_eq!(decoded.indices, vec![0, 2, 1, 1, 2, 3]);
    assert_eq!(decoded.submeshes.len(), 1);
    assert_eq!(decoded.submeshes[0].index_count, 6);
    assert!(
        decoded.weights_are_normalized(),
        "Normalized at encode time"
    );
    assert!((decoded.weights[0][0] - 0.5).abs() < 1e-5);
    assert!(decoded.joint_indices_are_in_range());
}

#[test]
fn zero_weights_fall_back_to_the_first_joint() {
    let mut mesh = quad_mesh();
    mesh.weights = vec![[0.0; 4]; 4];
    let decoded = decode_rskin(&encode_rskin(&mesh).expect("encode")).expect("decode");
    assert_eq!(decoded.joints[0], [0, 0, 0, 0]);
    assert_eq!(decoded.weights[0], [1.0, 0.0, 0.0, 0.0]);
    assert!(decoded.weights_are_normalized());
}

#[test]
fn joint_indices_are_clamped_to_the_skeleton() {
    let mut mesh = quad_mesh();
    mesh.joints = vec![[7, 9, 1, 0]; 4];
    let decoded = decode_rskin(&encode_rskin(&mesh).expect("encode")).expect("decode");
    assert!(decoded.joint_indices_are_in_range());
    assert_eq!(decoded.joints[0][0], 1, "joint_count - 1 = 1");
}

#[test]
fn a_mesh_without_streams_round_trips() {
    let mut mesh = quad_mesh();
    mesh.normals.clear();
    mesh.uv0.clear();
    let decoded = decode_rskin(&encode_rskin(&mesh).expect("encode")).expect("decode");
    assert!(!decoded.has_normals());
    assert!(!decoded.has_uv0());
    assert!(decoded.normals.is_empty());
    assert_eq!(decoded.positions.len(), 4);
}

#[test]
fn a_mesh_whose_streams_disagree_on_length_is_rejected() {
    let mut mesh = quad_mesh();
    mesh.normals.pop();
    assert!(
        encode_rskin(&mesh).is_none(),
        "Normal stream length must match vertex count"
    );
}

#[test]
fn a_clip_round_trips_with_sorted_keys_and_unit_quaternions() {
    let bytes = encode_ranim(&walk_clip()).expect("encode");
    assert_eq!(&bytes[..4], RANI_MAGIC);
    let decoded = decode_ranim(&bytes).expect("decode");
    assert_eq!(decoded.name, "walk");
    assert!(decoded.looped);
    assert!(!decoded.additive);
    assert_eq!(decoded.fps, 30.0);
    let track = &decoded.tracks[0];
    assert_eq!(track.joint, 1);
    assert_eq!(track.positions.len(), 2);
    assert_eq!(track.positions[0].time, 0.0, "Key frames sorted by time");
    assert_eq!(track.positions[1].value, [0.0, 0.2, 0.0]);
    assert_eq!(track.rotations[0].value, [0.0, 0.0, 0.0, 1.0], "Normalized");
    assert_eq!(decoded.clip_duration(), 1.0);
}

#[test]
fn an_empty_clip_cannot_be_encoded() {
    let mut clip = walk_clip();
    clip.tracks.clear();
    assert!(encode_ranim(&clip).is_none());
}

#[test]
fn truncated_buffers_are_rejected_instead_of_panicking() {
    let skeleton = encode_rskel(&two_bone_skeleton()).expect("encode");
    let mesh = encode_rskin(&quad_mesh()).expect("encode");
    let clip = encode_ranim(&walk_clip()).expect("encode");
    for cut in [0usize, 3, 10, 20, 40] {
        if cut < skeleton.len() {
            assert!(decode_rskel(&skeleton[..cut]).is_none(), "RSKL cut {cut}");
        }
        if cut < mesh.len() {
            assert!(decode_rskin(&mesh[..cut]).is_none(), "RSKN cut {cut}");
        }
        if cut < clip.len() {
            assert!(decode_ranim(&clip[..cut]).is_none(), "RANI cut {cut}");
        }
    }
}

#[test]
fn trailing_bytes_are_rejected() {
    let mut skeleton = encode_rskel(&two_bone_skeleton()).expect("encode");
    skeleton.push(0);
    assert!(decode_rskel(&skeleton).is_none());
    let mut clip = encode_ranim(&walk_clip()).expect("encode");
    clip.push(0);
    assert!(decode_ranim(&clip).is_none());
    let mut mesh = encode_rskin(&quad_mesh()).expect("encode");
    mesh.push(0);
    assert!(decode_rskin(&mesh).is_none());
}

#[test]
fn ranim_v5_round_trips_notifies_markers_sections_and_reads_older_clips() {
    use prism_asset::formats_skinned::{decode_ranim, AnimNotifyInfo};

    // v2: event table written out sorted by time.
    let mut clip = walk_clip();
    clip.notifies = vec![
        AnimNotifyInfo {
            time: 0.8,
            name: "Footstep.R".into(),
        },
        AnimNotifyInfo {
            time: 0.2,
            name: "Footstep.L".into(),
        },
    ];
    let bytes = encode_ranim(&clip).expect("encode");
    assert_eq!(bytes[4], RANI_VERSION, "Version should be written as v5");
    let decoded = decode_ranim(&bytes).expect("decode v5");
    let names: Vec<&str> = decoded
        .notifies
        .iter()
        .map(|notify| notify.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["Footstep.L", "Footstep.R"],
        "Sorted by time at encode time"
    );
    assert!((decoded.notifies[0].time - 0.2).abs() < 1e-6);

    // v4 compat: drop trailing warp window table, write version back to 4.
    let mut legacy = encode_ranim(&walk_clip()).expect("encode");
    legacy.truncate(legacy.len() - 4); // warp_window_count
    legacy[4] = RANI_SECTION_VERSION;
    let decoded = decode_ranim(&legacy).expect("decode v4");
    assert!(
        decoded.warp_windows.is_empty(),
        "v4 clip has no warp window"
    );

    // v3 compat: again drop section table, write version back to 3, events / flags still decodable.
    let mut legacy = encode_ranim(&walk_clip()).expect("encode");
    legacy.truncate(legacy.len() - 4); // warp_window_count
    legacy.truncate(legacy.len() - 4); // section_count
    legacy[4] = RANI_SYNC_MARKER_VERSION;
    let decoded = decode_ranim(&legacy).expect("decode v3");
    assert!(
        decoded.sections.is_empty(),
        "v3 clip has no Montage segments"
    );

    // v2 compat: again drop sync marker table, write version back to 2, event table still decodable.
    let mut legacy = encode_ranim(&walk_clip()).expect("encode");
    legacy.truncate(legacy.len() - 4); // warp_window_count
    legacy.truncate(legacy.len() - 4); // section_count
    legacy.truncate(legacy.len() - 4); // marker_count
    legacy[4] = RANI_NOTIFY_VERSION;
    let decoded = decode_ranim(&legacy).expect("decode v2");
    assert!(
        decoded.sync_markers.is_empty(),
        "v2 clip has no sync markers"
    );
    assert_eq!(decoded.name, "walk");

    // v1 compat: again drop event table from v2 (notify_count = 0, 4 bytes), clip still decodable.
    let mut legacy = encode_ranim(&walk_clip()).expect("encode");
    legacy.truncate(legacy.len() - 4); // warp_window_count
    legacy.truncate(legacy.len() - 4); // section_count
    legacy.truncate(legacy.len() - 4); // marker_count
    legacy.truncate(legacy.len() - 4); // notify_count (empty)
    legacy[4] = SKINNED_FORMAT_VERSION;
    let decoded = decode_ranim(&legacy).expect("decode v1");
    assert!(decoded.notifies.is_empty(), "v1 clip has no events");
    assert!(
        decoded.sync_markers.is_empty(),
        "v1 clip has no sync markers"
    );
    assert_eq!(decoded.name, "walk");
}

#[test]
fn ranim_v5_round_trips_normalized_markers_ranges_and_windows() {
    use prism_asset::formats_skinned::{decode_ranim, AnimClipSectionInfo, AnimClipWarpWindowInfo};

    let mut clip = walk_clip();
    clip.sync_markers = vec![0.5, 0.0, 0.5, 1.5, f32::NAN, -0.25];
    clip.sections = vec![
        AnimClipSectionInfo {
            name: "Land".into(),
            start: 0.8,
            end: 1.0,
        },
        AnimClipSectionInfo {
            name: "Start".into(),
            start: 0.0,
            end: 0.3,
        },
        AnimClipSectionInfo {
            name: "Vault".into(),
            start: 0.3,
            end: 0.8,
        },
        // Illegal segment: encoder discards, not written into asset.
        AnimClipSectionInfo {
            name: "".into(),
            start: 0.0,
            end: 0.0,
        },
    ];
    clip.warp_windows = vec![
        AnimClipWarpWindowInfo {
            name: "LandWarp".into(),
            start: 0.8,
            end: 1.0,
        },
        AnimClipWarpWindowInfo {
            name: "VaultWarp".into(),
            start: 0.3,
            end: 0.8,
        },
        AnimClipWarpWindowInfo {
            name: "Bad".into(),
            start: 0.5,
            end: 0.2,
        },
    ];
    let bytes = encode_ranim(&clip).expect("encode");
    let decoded = decode_ranim(&bytes).expect("decode");
    // Normalize, sort, deduplicate, drop non-limited values.
    assert_eq!(decoded.sync_markers, vec![0.0, 0.5, 0.75]);
    let names: Vec<&str> = decoded
        .sections
        .iter()
        .map(|section| section.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["Start", "Vault", "Land"],
        "Sorted by start at encode time"
    );
    assert!((decoded.sections[1].start - 0.3).abs() < 1e-6);
    assert!((decoded.sections[2].end - 1.0).abs() < 1e-6);
    let windows: Vec<&str> = decoded
        .warp_windows
        .iter()
        .map(|window| window.name.as_str())
        .collect();
    assert_eq!(windows, vec!["VaultWarp", "LandWarp"]);
    assert!((decoded.warp_windows[0].start - 0.3).abs() < 1e-6);
}
