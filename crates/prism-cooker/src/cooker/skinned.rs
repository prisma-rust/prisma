//! Skeleton / skin / animation clip cooked format **encoder** (#560 P0).
//!
//! Decode side in `prism_asset::formats_skinned` (runtime only decodes) — consistent with RMES layering.
//! Byte layout written in that module document, both implementations must sync changes.
//!
//! Cooker side does two things runtime does not do:
//! - Reject skeleton where parent index does not point to earlier bone (cycle / out of order);
//! - Normalize weights, write unused joint slots as 0, ensuring GPU side directly available.

use prism_asset::formats_skinned::{
    AnimClipInfo, SkeletonInfo, SkinnedMeshInfo, RANI_ADDITIVE, RANI_LOOPED, RANI_MAGIC,
    RANI_VERSION, RSKL_JOINT_RECORD_SIZE, RSKL_MAGIC, RSKN_BIND_POSE_SIZE, RSKN_HAS_NORMALS,
    RSKN_HAS_TANGENTS, RSKN_HAS_UV0, RSKN_HEADER_SIZE, RSKN_MAGIC, SKINNED_FORMAT_VERSION,
};

/// Encode `.rskel`. `skeleton` must be topological order (parent index < own index), otherwise return `None`.
pub fn encode_rskel(skeleton: &SkeletonInfo) -> Option<Vec<u8>> {
    if skeleton.joints.is_empty() || !skeleton.is_topologically_sorted() {
        return None;
    }
    let mut names = Vec::new();
    for joint in &skeleton.joints {
        let bytes = joint.name.as_bytes();
        if bytes.len() > u16::MAX as usize {
            return None;
        }
        names.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
        names.extend_from_slice(bytes);
    }

    let mut out = Vec::with_capacity(13 + skeleton.joints.len() * RSKL_JOINT_RECORD_SIZE);
    out.extend_from_slice(RSKL_MAGIC);
    out.push(SKINNED_FORMAT_VERSION);
    out.extend_from_slice(&(skeleton.joints.len() as u32).to_le_bytes());
    out.extend_from_slice(&(names.len() as u32).to_le_bytes());

    for joint in &skeleton.joints {
        let parent = joint.parent.map_or(-1i32, i32::from);
        out.extend_from_slice(&parent.to_le_bytes());
        out.push(0);
        out.extend_from_slice(&[0, 0, 0]);
        for value in joint.reference_local.to_array() {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for value in joint.inverse_bind {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    out.extend_from_slice(&names);
    Some(out)
}

/// Normalize weights: sum zero degenerates to "all attached to joint 0", otherwise scale by sum.
fn normalize_weights(weights: [f32; 4], joints: &mut [u16; 4]) -> [f32; 4] {
    let cleaned = weights.map(|weight| {
        if weight.is_finite() {
            weight.max(0.0)
        } else {
            0.0
        }
    });
    let sum: f32 = cleaned.iter().sum();
    if sum <= 1e-6 {
        *joints = [0, 0, 0, 0];
        return [1.0, 0.0, 0.0, 0.0];
    }
    cleaned.map(|weight| weight / sum)
}

/// Encode `.rskin`. Weights will be normalized, joint index clamped tight by `joint_count`.
pub fn encode_rskin(mesh: &SkinnedMeshInfo) -> Option<Vec<u8>> {
    let verts = mesh.positions.len();
    if verts == 0 || mesh.joints.len() != verts || mesh.weights.len() != verts {
        return None;
    }
    if mesh.joint_count == 0 {
        return None;
    }
    if !mesh.normals.is_empty() && mesh.normals.len() != verts {
        return None;
    }
    if !mesh.uv0.is_empty() && mesh.uv0.len() != verts {
        return None;
    }
    if !mesh.tangents.is_empty() && mesh.tangents.len() != verts {
        return None;
    }

    let mut flags = 0u32;
    if !mesh.normals.is_empty() {
        flags |= RSKN_HAS_NORMALS;
    }
    if !mesh.uv0.is_empty() {
        flags |= RSKN_HAS_UV0;
    }
    if !mesh.tangents.is_empty() {
        flags |= RSKN_HAS_TANGENTS;
    }

    let stream_floats = verts
        * (3 + if flags & RSKN_HAS_NORMALS != 0 { 3 } else { 0 }
            + if flags & RSKN_HAS_UV0 != 0 { 2 } else { 0 }
            + if flags & RSKN_HAS_TANGENTS != 0 { 4 } else { 0 });
    let mut out = Vec::with_capacity(
        RSKN_HEADER_SIZE
            + stream_floats * 4
            + verts * 12
            + mesh.indices.len() * 4
            + mesh.submeshes.len() * 12
            + RSKN_BIND_POSE_SIZE,
    );

    out.extend_from_slice(RSKN_MAGIC);
    out.push(SKINNED_FORMAT_VERSION);
    out.extend_from_slice(&(verts as u32).to_le_bytes());
    out.extend_from_slice(&(mesh.indices.len() as u32).to_le_bytes());
    out.extend_from_slice(&mesh.joint_count.to_le_bytes());
    out.extend_from_slice(&(mesh.submeshes.len() as u32).to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());

    for position in &mesh.positions {
        for value in position {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    if flags & RSKN_HAS_NORMALS != 0 {
        for normal in &mesh.normals {
            for value in normal {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
    }
    if flags & RSKN_HAS_UV0 != 0 {
        for uv in &mesh.uv0 {
            for value in uv {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
    }
    if flags & RSKN_HAS_TANGENTS != 0 {
        for tangent in &mesh.tangents {
            for value in tangent {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
    }

    let mut joint_stream = Vec::with_capacity(verts * 8);
    let mut weight_stream = Vec::with_capacity(verts * 16);
    for index in 0..verts {
        let mut joints = mesh.joints[index];
        for joint in &mut joints {
            *joint = (*joint).min((mesh.joint_count - 1) as u16);
        }
        let weights = normalize_weights(mesh.weights[index], &mut joints);
        for joint in joints {
            joint_stream.extend_from_slice(&joint.to_le_bytes());
        }
        for weight in weights {
            weight_stream.extend_from_slice(&weight.to_le_bytes());
        }
    }
    out.extend_from_slice(&joint_stream);
    out.extend_from_slice(&weight_stream);

    for index in &mesh.indices {
        out.extend_from_slice(&index.to_le_bytes());
    }
    for submesh in &mesh.submeshes {
        out.extend_from_slice(&submesh.material_index.to_le_bytes());
        out.extend_from_slice(&submesh.index_offset.to_le_bytes());
        out.extend_from_slice(&submesh.index_count.to_le_bytes());
    }
    for value in mesh.bind_pose.to_array() {
        out.extend_from_slice(&value.to_le_bytes());
    }

    Some(out)
}

/// Encode `.ranim`. Close key frames written out by time ascending (same timestamp retains original order).
pub fn encode_ranim(clip: &AnimClipInfo) -> Option<Vec<u8>> {
    let name = clip.name.as_bytes();
    if name.len() > u16::MAX as usize || clip.tracks.is_empty() {
        return None;
    }
    let mut flags = 0u16;
    if clip.looped {
        flags |= RANI_LOOPED;
    }
    if clip.additive {
        flags |= RANI_ADDITIVE;
    }

    let mut out = Vec::new();
    out.extend_from_slice(RANI_MAGIC);
    out.push(RANI_VERSION);
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(&clip.duration.to_le_bytes());
    out.extend_from_slice(&clip.fps.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&(clip.tracks.len() as u32).to_le_bytes());

    for track in &clip.tracks {
        out.extend_from_slice(&track.joint.to_le_bytes());

        let mut positions = track.positions.clone();
        positions.sort_by(|a, b| a.time.total_cmp(&b.time));
        out.extend_from_slice(&(positions.len() as u32).to_le_bytes());
        for key in &positions {
            out.extend_from_slice(&key.time.to_le_bytes());
            for value in key.value {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }

        let mut rotations = track.rotations.clone();
        rotations.sort_by(|a, b| a.time.total_cmp(&b.time));
        out.extend_from_slice(&(rotations.len() as u32).to_le_bytes());
        for key in &rotations {
            out.extend_from_slice(&key.time.to_le_bytes());
            for value in key.value {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }

        let mut scales = track.scales.clone();
        scales.sort_by(|a, b| a.time.total_cmp(&b.time));
        out.extend_from_slice(&(scales.len() as u32).to_le_bytes());
        for key in &scales {
            out.extend_from_slice(&key.time.to_le_bytes());
            for value in key.value {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
    }

    let mut notifies: Vec<&prism_asset::formats_skinned::AnimNotifyInfo> = clip
        .notifies
        .iter()
        .filter(|notify| notify.time.is_finite() && notify.name.len() <= u16::MAX as usize)
        .collect();
    notifies.sort_by(|a, b| a.time.total_cmp(&b.time));
    out.extend_from_slice(&(notifies.len() as u32).to_le_bytes());
    for notify in notifies {
        out.extend_from_slice(&notify.time.to_le_bytes());
        out.extend_from_slice(&(notify.name.len() as u16).to_le_bytes());
        out.extend_from_slice(notify.name.as_bytes());
    }

    let mut markers: Vec<f32> = clip
        .sync_markers
        .iter()
        .copied()
        .filter(|phase| phase.is_finite())
        .map(|phase| phase.rem_euclid(1.0))
        .collect();
    markers.sort_by(f32::total_cmp);
    markers.dedup_by(|a, b| (*a - *b).abs() <= 1e-6);
    out.extend_from_slice(&(markers.len() as u32).to_le_bytes());
    for phase in markers {
        out.extend_from_slice(&phase.to_le_bytes());
    }

    let mut sections: Vec<&prism_asset::formats_skinned::AnimClipSectionInfo> = clip
        .sections
        .iter()
        .filter(|section| {
            !section.name.trim().is_empty()
                && section.name.len() <= u16::MAX as usize
                && section.start.is_finite()
                && section.end.is_finite()
                && section.start >= 0.0
                && section.end >= section.start
        })
        .collect();
    sections.sort_by(|a, b| {
        a.start
            .total_cmp(&b.start)
            .then_with(|| a.name.cmp(&b.name))
    });
    out.extend_from_slice(&(sections.len() as u32).to_le_bytes());
    for section in sections {
        out.extend_from_slice(&section.start.to_le_bytes());
        out.extend_from_slice(&section.end.to_le_bytes());
        out.extend_from_slice(&(section.name.len() as u16).to_le_bytes());
        out.extend_from_slice(section.name.as_bytes());
    }

    let mut windows: Vec<&prism_asset::formats_skinned::AnimClipWarpWindowInfo> = clip
        .warp_windows
        .iter()
        .filter(|window| {
            !window.name.trim().is_empty()
                && window.name.len() <= u16::MAX as usize
                && window.start.is_finite()
                && window.end.is_finite()
                && window.start >= 0.0
                && window.end >= window.start
        })
        .collect();
    windows.sort_by(|a, b| {
        a.start
            .total_cmp(&b.start)
            .then_with(|| a.name.cmp(&b.name))
    });
    out.extend_from_slice(&(windows.len() as u32).to_le_bytes());
    for window in windows {
        out.extend_from_slice(&window.start.to_le_bytes());
        out.extend_from_slice(&window.end.to_le_bytes());
        out.extend_from_slice(&(window.name.len() as u16).to_le_bytes());
        out.extend_from_slice(window.name.as_bytes());
    }

    Some(out)
}

#[cfg(test)]
#[path = "skinned_tests.rs"]
mod tests;
