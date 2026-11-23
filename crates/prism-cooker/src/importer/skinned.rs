//! glTF skin bones + animation import (#560 P0-1b).
//!
//! This only does "source file → import structure": convert glTF's `skins` / `JOINTS_0` / `WEIGHTS_0` /
//! `animations` into [`prism_asset::formats_skinned`] `SkeletonInfo` /
//! `SkinnedMeshInfo` / `AnimClipInfo`, then callers inside [`GltfSideAssets`] encode into
//! `.rskel` / `.rskin` / `.ranim`.
//!
//! Three conventions (all written in PR description):
//!
//! 1. **Do not convert coordinates**: glTF itself is right-handed +Y up, consistent with engine; UE export cm / Z-up
//!    must be converted at export time, not guessed in importer.
//! 2. **Bone order must be topological** (parent before child). glTF's `skin.joints` does not guarantee this,
//!    Here uniformly topologically sorted, and each primitive's `JOINTS_0` remapped together.
//! 3. **`reference_local` derived from inverse bind matrix**: `inverse_bind = inverse(world bind pose)`,
//!    so reference pose need not traverse node tree again, nor be inconsistent with file bind pose.
use std::collections::{BTreeMap, HashMap};

use prism_asset::formats_skinned::{
    AnimClipInfo, AnimTrackInfo, JointInfo, JointTransform, QuatKeyInfo, SkeletonInfo,
    SkinnedMeshInfo, SubmeshInfo as SkinnedSubmeshInfo, Vec3KeyInfo,
};

use glam::{Mat3, Mat4, Quat, Vec3};

/// Source of one bone in glTF (for remap use).
#[derive(Debug, Clone)]
struct JointSource {
    node: usize,
    inverse_bind: Mat4,
}

/// Parsed bones + each skin's "local index → global index" table.
#[derive(Debug, Clone)]
pub struct GltfSkinning {
    /// Global skeleton (already topologically sorted).
    pub skeleton: SkeletonInfo,
    /// `skin index` → that skin's `joints[i]` index in global skeleton.
    pub remaps: HashMap<usize, Vec<u16>>,
}

impl Default for GltfSkinning {
    fn default() -> Self {
        Self {
            skeleton: SkeletonInfo { joints: Vec::new() },
            remaps: HashMap::new(),
        }
    }
}

impl GltfSkinning {
    pub fn is_empty(&self) -> bool {
        self.skeleton.joints.is_empty()
    }

    pub fn joint_count(&self) -> usize {
        self.skeleton.joint_count()
    }

    /// Some skin's local joint index → global bone index.
    pub fn remap(&self, skin: usize) -> Option<&[u16]> {
        self.remaps.get(&skin).map(Vec::as_slice)
    }
}

/// Exported skinning result: skeleton + each skin remap + all animation clips.
#[derive(Debug, Clone, Default)]
pub struct GltfSkinResult {
    pub skinning: GltfSkinning,
    pub clips: Vec<AnimClipInfo>,
}

/// Parse `document.skins()` + `document.animations()`.
pub fn read_skinning(document: &gltf::Document, buffers: &[gltf::buffer::Data]) -> GltfSkinResult {
    let mut joints: Vec<JointSource> = Vec::new();
    let mut by_node: HashMap<usize, u16> = HashMap::new();
    let mut remaps: HashMap<usize, Vec<u16>> = HashMap::new();

    for (skin_index, skin) in document.skins().enumerate() {
        let reader = skin.reader(|buffer| Some(&buffers[buffer.index()]));
        let inverse_binds: Vec<Mat4> = reader
            .read_inverse_bind_matrices()
            .map(|iter| iter.map(|m| Mat4::from_cols_array_2d(&m)).collect())
            .unwrap_or_default();

        let mut local_remap = Vec::new();
        for (local, node) in skin.joints().enumerate() {
            let node_index = node.index();
            let inverse_bind = inverse_binds.get(local).copied().unwrap_or(IDENTITY_4X4);
            let global = *by_node.entry(node_index).or_insert_with(|| {
                let index = joints.len() as u16;
                joints.push(JointSource {
                    node: node_index,
                    inverse_bind,
                });
                index
            });
            local_remap.push(global);
        }
        remaps.insert(skin_index, local_remap);
    }

    if joints.is_empty() {
        return GltfSkinResult::default();
    }

    let nodes: Vec<gltf::Node<'_>> = document.nodes().collect();
    let position_of_node: HashMap<usize, usize> = nodes
        .iter()
        .enumerate()
        .map(|(position, node)| (node.index(), position))
        .collect();
    let parent_of_node = node_parents(document, &nodes);
    let is_joint: HashMap<usize, ()> = joints.iter().map(|joint| (joint.node, ())).collect();

    let mut parents: Vec<Option<usize>> = Vec::with_capacity(joints.len());
    for joint in &joints {
        let mut parent = None;
        let mut cursor = parent_of_node.get(&joint.node).copied();
        while let Some(node) = cursor {
            if is_joint.contains_key(&node) {
                parent = by_node.get(&node).map(|index| *index as usize);
                break;
            }
            cursor = parent_of_node.get(&node).copied();
        }
        parents.push(parent);
    }

    let order = topological_order(&parents);

    let mut new_index: Vec<u16> = vec![0; joints.len()];
    for (new_slot, old_index) in order.iter().enumerate() {
        new_index[*old_index] = new_slot as u16;
    }
    let remaps = remaps
        .into_iter()
        .map(|(skin, local)| {
            (
                skin,
                local
                    .into_iter()
                    .map(|global| new_index[global as usize])
                    .collect(),
            )
        })
        .collect();

    let mut world: Vec<Option<Mat4>> = Vec::with_capacity(order.len());
    for old_index in &order {
        world.push(invert_affine(&joints[*old_index].inverse_bind));
    }

    let mut sorted: Vec<JointInfo> = Vec::with_capacity(order.len());
    for (slot, old_index) in order.iter().enumerate() {
        let joint = &joints[*old_index];
        let parent = parents[*old_index].map(|parent| new_index[parent]);
        let reference_world = world[slot];
        let local = match (reference_world, parent) {
            (Some(child), Some(parent)) => match world[parent as usize] {
                Some(parent_world) => invert_affine(&parent_world)
                    .map(|inverse| inverse * child)
                    .unwrap_or(IDENTITY_4X4),
                None => child,
            },
            (Some(child), None) => child,
            _ => IDENTITY_4X4,
        };
        let name = nodes
            .get(*position_of_node.get(&joint.node).unwrap_or(&0))
            .and_then(|node| node.name().map(str::to_string))
            .unwrap_or_else(|| format!("joint_{slot}"));
        sorted.push(JointInfo {
            name,
            parent,
            reference_local: decompose(&local),
            inverse_bind: flatten_column_major(&joint.inverse_bind),
        });
    }

    let skeleton = SkeletonInfo { joints: sorted };
    let clips = read_animations(document, buffers, &by_node, &new_index);

    GltfSkinResult {
        skinning: GltfSkinning { skeleton, remaps },
        clips,
    }
}

fn read_animations(
    document: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    by_node: &HashMap<usize, u16>,
    new_index: &[u16],
) -> Vec<AnimClipInfo> {
    use gltf::animation::util::ReadOutputs;

    let joint_index = |node: usize| -> Option<u16> {
        let global = by_node.get(&node)?;
        new_index.get(*global as usize).copied()
    };

    let mut clips = Vec::new();
    for (clip_index, animation) in document.animations().enumerate() {
        let mut tracks: BTreeMap<u16, AnimTrackInfo> = BTreeMap::new();
        let mut duration = 0.0f32;
        let mut skipped_nodes = 0usize;
        let mut morph_channels = 0usize;

        for channel in animation.channels() {
            let Some(joint) = joint_index(channel.target().node().index()) else {
                skipped_nodes += 1;
                continue;
            };
            let reader = channel.reader(|buffer| Some(&buffers[buffer.index()]));
            let Some(inputs) = reader.read_inputs() else {
                continue;
            };
            let times: Vec<f32> = inputs.collect();
            let Some(outputs) = reader.read_outputs() else {
                continue;
            };
            duration = duration.max(
                times
                    .iter()
                    .copied()
                    .filter(|t| t.is_finite())
                    .fold(0.0, f32::max),
            );

            let track = tracks.entry(joint).or_insert_with(|| AnimTrackInfo {
                joint: u32::from(joint),
                positions: Vec::new(),
                rotations: Vec::new(),
                scales: Vec::new(),
            });
            match outputs {
                ReadOutputs::Translations(values) => {
                    track.positions = collect_vec3_keys(&times, values);
                }
                ReadOutputs::Scales(values) => {
                    track.scales = collect_vec3_keys(&times, values);
                }
                ReadOutputs::Rotations(values) => {
                    track.rotations = collect_quat_keys(&times, values.into_f32());
                }
                ReadOutputs::MorphTargetWeights(_) => {
                    morph_channels += 1;
                }
            }
        }

        if morph_channels > 0 {
            tracing::debug!(
                "glTF animation {clip_index}: {morph_channels} morph-weight channel(s) ignored (no morph targets yet)"
            );
        }
        if skipped_nodes > 0 {
            tracing::debug!(
                "glTF animation {clip_index}: skipped {skipped_nodes} channel(s) that do not target a skeleton joint"
            );
        }

        let tracks: Vec<AnimTrackInfo> = tracks.into_values().collect();
        if tracks.is_empty() {
            continue;
        }

        let name = animation
            .name()
            .map(str::to_string)
            .unwrap_or_else(|| format!("clip_{clip_index}"));
        let key_time = tracks
            .iter()
            .flat_map(|track| {
                track
                    .positions
                    .iter()
                    .map(|key| key.time)
                    .chain(track.rotations.iter().map(|key| key.time))
                    .chain(track.scales.iter().map(|key| key.time))
            })
            .filter(|time| time.is_finite())
            .fold(0.0f32, f32::max);
        let duration = duration.max(key_time);
        clips.push(AnimClipInfo {
            name,
            duration,
            fps: 30.0,
            looped: false,
            additive: false,
            tracks,
            notifies: Vec::new(),
            sync_markers: Vec::new(),
            sections: Vec::new(),
            warp_windows: Vec::new(),
        });
    }
    clips
}

fn collect_vec3_keys(times: &[f32], values: impl Iterator<Item = [f32; 3]>) -> Vec<Vec3KeyInfo> {
    let values: Vec<[f32; 3]> = values.collect();
    keys_for(times, &values)
        .into_iter()
        .map(|(time, value)| Vec3KeyInfo { time, value })
        .collect()
}

fn collect_quat_keys(times: &[f32], values: impl Iterator<Item = [f32; 4]>) -> Vec<QuatKeyInfo> {
    let values: Vec<[f32; 4]> = values.collect();
    keys_for(times, &values)
        .into_iter()
        .map(|(time, value)| QuatKeyInfo { time, value })
        .collect()
}

/// Time and numeric value correspond one-to-one.
///
/// CubicSpline sampler output is "in tangent + value + out tangent" triplet, here take intermediate value
/// (curve details left for future heavy sampling, cannot directly order triplet by key frame).
fn keys_for<T: Copy>(times: &[f32], values: &[T]) -> Vec<(f32, T)> {
    let stride = if times.is_empty() || values.len() == times.len() {
        1
    } else if values.len() == times.len() * 3 {
        3
    } else {
        1
    };
    let mut keys = Vec::with_capacity(times.len());
    for (index, time) in times.iter().enumerate() {
        if !time.is_finite() {
            continue;
        }
        let value = match values.get(index * stride + (stride - 1) / 2) {
            Some(value) => *value,
            None => break,
        };
        keys.push((*time, value));
    }
    keys.sort_by(|a, b| a.0.total_cmp(&b.0));
    keys.dedup_by(|a, b| (a.0 - b.0).abs() <= f32::EPSILON);
    keys
}

/// Pack importer vertex data + per-vertex bone influence into `.rskin` intermediate structure.
///
/// Weights normalized here (glTF allows imprecision), full-zero vertices degenerate to "all attached to joint 0".
pub(super) fn skinned_mesh_info(
    mesh: &super::MeshData,
    joint_count_u32: u32,
) -> Option<SkinnedMeshInfo> {
    if joint_count_u32 == 0 || mesh.joints.len() != mesh.positions.len() {
        return None;
    }
    if mesh.weights.len() != mesh.positions.len() {
        return None;
    }
    let mut weights = mesh.weights.clone();
    for weight in &mut weights {
        let sum: f32 = weight.iter().sum();
        if !sum.is_finite() || sum <= 1e-6 {
            *weight = [1.0, 0.0, 0.0, 0.0];
        } else {
            for value in weight.iter_mut() {
                *value /= sum;
            }
        }
    }

    let flags = (if mesh.all_normals {
        prism_asset::formats_skinned::RSKN_HAS_NORMALS
    } else {
        0
    }) | (if mesh.all_uv0 {
        prism_asset::formats_skinned::RSKN_HAS_UV0
    } else {
        0
    });

    let submeshes = mesh
        .submeshes
        .iter()
        .map(|submesh| SkinnedSubmeshInfo {
            material_index: submesh.material_index.unwrap_or(u32::MAX as usize) as u32,
            index_offset: submesh.index_start,
            index_count: submesh.index_count,
        })
        .collect();

    Some(SkinnedMeshInfo {
        vertex_count: mesh.positions.len() as u32,
        index_count: mesh.indices.len() as u32,
        joint_count: joint_count_u32,
        flags,
        positions: mesh.positions.clone(),
        normals: if mesh.all_normals {
            mesh.normals.clone()
        } else {
            Vec::new()
        },
        uv0: if mesh.all_uv0 {
            mesh.uv0.clone()
        } else {
            Vec::new()
        },
        tangents: Vec::new(),
        joints: mesh.joints.clone(),
        weights,
        indices: mesh.indices.clone(),
        submeshes,
        bind_pose: JointTransform::default(),
    })
}

const IDENTITY_4X4: Mat4 = Mat4::IDENTITY;

/// Parent-before-child order; cyclic bones appended to end (won't hang).
fn topological_order(parents: &[Option<usize>]) -> Vec<usize> {
    let mut order = Vec::with_capacity(parents.len());
    let mut placed = vec![false; parents.len()];
    let mut progress = true;
    while progress && order.len() < parents.len() {
        progress = false;
        for (index, parent) in parents.iter().enumerate() {
            if placed[index] {
                continue;
            }
            let ready = match parent {
                None => true,
                Some(parent) => placed[*parent],
            };
            if ready {
                placed[index] = true;
                order.push(index);
                progress = true;
            }
        }
    }
    for (index, _) in parents.iter().enumerate() {
        if !placed[index] {
            order.push(index);
        }
    }
    order
}

/// Node index → parent node index. Walk from each scene root, nodes not referenced by any scene
/// become roots (consistent with `read_gltf` traversal scope).
fn node_parents(document: &gltf::Document, nodes: &[gltf::Node<'_>]) -> HashMap<usize, usize> {
    fn walk(node: gltf::Node<'_>, parents: &mut HashMap<usize, usize>) {
        for child in node.children() {
            parents.insert(child.index(), node.index());
            walk(child, parents);
        }
    }

    let mut parents = HashMap::new();
    if let Some(scene) = document.default_scene() {
        for node in scene.nodes() {
            walk(node, &mut parents);
        }
    } else {
        let mut children = std::collections::HashSet::new();
        for node in nodes {
            for child in node.children() {
                children.insert(child.index());
            }
        }
        for node in nodes {
            if !children.contains(&node.index()) {
                walk(node.clone(), &mut parents);
            }
        }
    }
    parents
}

/// Column-major 4×4 → 16 f32s (`.rskel` `inverse_bind` layout).
fn flatten_column_major(m: &Mat4) -> [f32; 16] {
    m.to_cols_array()
}

/// Affine 4×4 inverse (column-major). Degenerate matrix (determinant near 0) returns `None`.
fn invert_affine(m: &Mat4) -> Option<Mat4> {
    let determinant = Mat3::from_mat4(*m).determinant();
    if !determinant.is_finite() || determinant.abs() < 1e-12 {
        return None;
    }
    Some(m.inverse())
}

/// Decompose 4×4 affine matrix into T/R/S (`JointTransform` expressive power).
fn decompose(m: &Mat4) -> JointTransform {
    let x = m.x_axis.truncate();
    let y = m.y_axis.truncate();
    let z = m.z_axis.truncate();
    let scale = [x.length(), y.length(), z.length()];
    let normalize = |v: Vec3, fallback: Vec3| {
        if v.length() > 1e-8 {
            v.normalize()
        } else {
            fallback
        }
    };
    let basis = Mat3::from_cols(
        normalize(x, Vec3::X),
        normalize(y, Vec3::Y),
        normalize(z, Vec3::Z),
    );
    JointTransform {
        translation: m.w_axis.truncate().to_array(),
        rotation: quat_from_basis(basis),
        scale,
    }
}

/// 3×3 rotation matrix → quaternion (`xyzw`).
fn quat_from_basis(basis: Mat3) -> [f32; 4] {
    let quat = Quat::from_mat3(&basis);
    let length = quat.length();
    if length > 1e-8 {
        let signed = if quat.w < 0.0 { -quat } else { quat };
        (signed / length).to_array()
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

#[cfg(test)]
#[path = "skinned_tests.rs"]
mod tests;
