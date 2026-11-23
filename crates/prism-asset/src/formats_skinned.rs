//! Skeleton / skinned mesh / animation clip cooked format decoder (#560 P0).
//!
//! All three formats are "magic + version + fixed records + variable blocks", all lengths derived from counters,
//! no pointers; decoder does checked math on every length, bad file returns `None`.
//!
//! Encoder side in `prism-cooker` (`cooker/skinned.rs`), runtime only decodes --- same layering as RMES.
//!
//! ## RSKL v1 (skeleton)
//!
//! ```text
//! [magic:4 "RSKL"][version:1][joint_count:4][name_bytes:4]
//! joints: joint_count × {
//!     parent: i32            // -1 = root
//!     flags: u8, pad: [u8;3]
//!     reference_local: 10×f32   // translation(3) + rotation xyzw(4) + scale(3)
//!     inverse_bind: 16×f32      // column-major
//! }
//! names: joint_count × { len: u16, utf8[len] }
//! ```
//!
//! ## RSKN v1 (skinned mesh)
//!
//! ```text
//! [magic:4 "RSKN"][version:1][vertex_count:4][index_count:4][joint_count:4]
//! [submesh_count:4][flags:4]
//! positions: v×3 f32
//! normals:   v×3 f32        // flags & RSKN_HAS_NORMALS
//! uv0:       v×2 f32        // flags & RSKN_HAS_UV0
//! tangents:  v×4 f32        // flags & RSKN_HAS_TANGENTS
//! joints:    v×4 u16
//! weights:   v×4 f32
//! indices:   i×u32
//! submeshes: s×{ material_index:u32, index_offset:u32, index_count:u32 }
//! bind_pose: 10×f32
//! ```
//!
//! ## RANI v5 (animation clip + notifies + sync markers + montage ranges)
//!
//! ```text
//! [magic:4 "RANI"][version:1][name_len:u16][name][duration:f32][fps:f32][flags:u16][track_count:4]
//! tracks: track_count × {
//!     joint: u32
//!     pos_count: u32,   pos keys:   ×{time:f32, value:[f32;3]}
//!     rot_count: u32,   rot keys:   ×{time:f32, value:[f32;4]}
//!     scale_count: u32, scale keys: ×{time:f32, value:[f32;3]}
//! }
//! notifies: notify_count:4, notify_count × { time:f32, name_len:u16, name }
//! sync markers: marker_count:4, marker_count × { phase:f32 }
//! montage sections: section_count:4, section_count × {
//!     start:f32, end:f32, name_len:u16, name
//! }
//! montage warp windows: window_count:4, window_count × {
//!     start:f32, end:f32, name_len:u16, name
//! }
//! ```
//!
//! v2 appends event table at v1 end; v3 appends sync marker table (`marker_count` = 0 means an
//! u32 of zero); v4 appends Montage segment table; v5 appends Montage warp window table. Header and track
//! layout unchanged, v1-v4 clips still readable, missing tables decode to empty.

/// RSKL magic.
pub const RSKL_MAGIC: &[u8; 4] = b"RSKL";
/// RSKN magic.
pub const RSKN_MAGIC: &[u8; 4] = b"RSKN";
/// RANI magic.
pub const RANI_MAGIC: &[u8; 4] = b"RANI";

/// RSKL / RSKN version number.
pub const SKINNED_FORMAT_VERSION: u8 = 1;

/// RANI version number (#560 P1 Notify): v2 appends event table at v1 end.
///
/// Separate from [`SKINNED_FORMAT_VERSION`] because skeleton / skinned mesh have no reason to add fields,
/// but clips do; mixing them would force RSKL/RSKN to unnecessarily bump versions.
pub const RANI_VERSION: u8 = 5;

/// First RANI version with Notify event table.
pub const RANI_NOTIFY_VERSION: u8 = 2;

/// First RANI version with sync marker table.
pub const RANI_SYNC_MARKER_VERSION: u8 = 3;

/// First RANI version with Montage segment table.
pub const RANI_SECTION_VERSION: u8 = 4;

/// First RANI version with Montage warp window table.
pub const RANI_WARP_WINDOW_VERSION: u8 = 5;

/// Per-bone cooked record size: parent(4) + flags(1) + pad(3) + local(40) + inverse_bind(64).
pub const RSKL_JOINT_RECORD_SIZE: usize = 4 + 1 + 3 + 40 + 64;
/// RSKL fixed header size.
pub const RSKL_HEADER_SIZE: usize = 4 + 1 + 4 + 4;
/// RSKN fixed header size: magic(4) + version(1) + 5×u32.
pub const RSKN_HEADER_SIZE: usize = 4 + 1 + 4 * 5;
/// RSKN bind pose byte count (10 × f32).
pub const RSKN_BIND_POSE_SIZE: usize = 40;

/// RSKN flag: has normal stream.
pub const RSKN_HAS_NORMALS: u32 = 1 << 0;
/// RSKN flag: has uv0 stream.
pub const RSKN_HAS_UV0: u32 = 1 << 1;
/// RSKN flag: has tangent stream.
pub const RSKN_HAS_TANGENTS: u32 = 1 << 2;

/// RANI flag: loop playback.
pub const RANI_LOOPED: u16 = 1 << 0;
/// RANI flag: additive clip.
pub const RANI_ADDITIVE: u16 = 1 << 1;

/// Forward-only byte reader: every read does bounds check, out-of-bounds returns `None`.
/// Fixed/variable-length record reader (also used by `formats_state_machine`).
pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }

    pub(crate) fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(len)?;
        if end > self.data.len() {
            return None;
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Some(slice)
    }

    pub(crate) fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }

    pub(crate) fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    pub(crate) fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    pub(crate) fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    pub(crate) fn f32s(&mut self, count: usize) -> Option<Vec<f32>> {
        let bytes = count.checked_mul(4)?;
        let slice = self.take(bytes)?;
        Some(
            slice
                .as_chunks::<4>()
                .0
                .iter()
                .map(|chunk| f32::from_le_bytes(*chunk))
                .collect(),
        )
    }

    pub(crate) fn u16s(&mut self, count: usize) -> Option<Vec<u16>> {
        let bytes = count.checked_mul(2)?;
        let slice = self.take(bytes)?;
        Some(
            slice
                .as_chunks::<2>()
                .0
                .iter()
                .map(|chunk| u16::from_le_bytes(*chunk))
                .collect(),
        )
    }

    fn u32s(&mut self, count: usize) -> Option<Vec<u32>> {
        let bytes = count.checked_mul(4)?;
        let slice = self.take(bytes)?;
        Some(
            slice
                .as_chunks::<4>()
                .0
                .iter()
                .map(|chunk| u32::from_le_bytes(*chunk))
                .collect(),
        )
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
}

/// Bone local TRS (translation / rotation quaternion xyzw / scale).
///
/// Separate struct instead of `glam` type: `prism-asset` does not depend on math library.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JointTransform {
    pub translation: [f32; 3],
    /// Quaternion, `xyzw` layout.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl Default for JointTransform {
    fn default() -> Self {
        Self {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }
}

impl JointTransform {
    /// Packed into 10 f32 (same layout as `.rskel` / `.rskin`).
    pub fn to_array(self) -> [f32; 10] {
        [
            self.translation[0],
            self.translation[1],
            self.translation[2],
            self.rotation[0],
            self.rotation[1],
            self.rotation[2],
            self.rotation[3],
            self.scale[0],
            self.scale[1],
            self.scale[2],
        ]
    }

    /// Read back from 10 f32.
    pub fn from_array(values: [f32; 10]) -> Self {
        Self {
            translation: [values[0], values[1], values[2]],
            rotation: [values[3], values[4], values[5], values[6]],
            scale: [values[7], values[8], values[9]],
        }
    }
}

/// One bone: parent index (`None` = root), reference pose local transform, inverse bind (column-major).
#[derive(Debug, Clone, PartialEq)]
pub struct JointInfo {
    pub name: String,
    pub parent: Option<u16>,
    pub reference_local: JointTransform,
    pub inverse_bind: [f32; 16],
}

/// Decoded `.rskel`.
#[derive(Debug, Clone, PartialEq)]
pub struct SkeletonInfo {
    pub joints: Vec<JointInfo>,
}

impl SkeletonInfo {
    pub fn joint_count(&self) -> usize {
        self.joints.len()
    }

    /// Find bone index by name (for animation binding / editor skeleton tree).
    pub fn joint_index(&self, name: &str) -> Option<usize> {
        self.joints.iter().position(|joint| joint.name == name)
    }

    /// Parent index must be strictly less than self index (topological order). Decode and cooker both use it to reject cyclic skeleton.
    pub fn is_topologically_sorted(&self) -> bool {
        self.joints
            .iter()
            .enumerate()
            .all(|(index, joint)| joint.parent.is_none_or(|parent| (parent as usize) < index))
    }
}

/// Decode `.rskel`.
pub fn decode_rskel(data: &[u8]) -> Option<SkeletonInfo> {
    if data.len() < RSKL_HEADER_SIZE || &data[..4] != RSKL_MAGIC {
        return None;
    }
    let mut reader = Reader::new(data, 4);
    if reader.u8()? != SKINNED_FORMAT_VERSION {
        return None;
    }
    let joint_count = reader.u32()? as usize;
    let name_bytes = reader.u32()? as usize;
    if joint_count == 0 {
        return None;
    }

    let mut joints = Vec::with_capacity(joint_count.min(4096));
    for _ in 0..joint_count {
        let parent_raw = reader.i32()?;
        let parent = match parent_raw {
            -1 => None,
            value if value >= 0 => Some(u16::try_from(value).ok()?),
            _ => return None,
        };
        let _flags = reader.u8()?;
        reader.take(3)?;
        let local = reader.f32s(10)?;
        let inverse = reader.f32s(16)?;
        let mut inverse_bind = [0.0f32; 16];
        for (slot, value) in inverse_bind.iter_mut().enumerate() {
            *value = inverse[slot];
        }
        joints.push(JointInfo {
            name: String::new(),
            parent,
            reference_local: JointTransform::from_array([
                local[0], local[1], local[2], local[3], local[4], local[5], local[6], local[7],
                local[8], local[9],
            ]),
            inverse_bind,
        });
    }

    if reader.remaining() != name_bytes {
        return None;
    }
    for joint in &mut joints {
        let len = reader.u16()? as usize;
        let bytes = reader.take(len)?;
        joint.name = String::from_utf8(bytes.to_vec()).ok()?;
    }
    if reader.remaining() != 0 {
        return None;
    }

    let skeleton = SkeletonInfo { joints };
    skeleton.is_topologically_sorted().then_some(skeleton)
}

/// One submesh: material slot + index range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubmeshInfo {
    pub material_index: u32,
    pub index_offset: u32,
    pub index_count: u32,
}

/// Decoded `.rskin`: per-vertex 4 bone influences + indices + submesh + bind pose.
#[derive(Debug, Clone, PartialEq)]
pub struct SkinnedMeshInfo {
    pub vertex_count: u32,
    pub index_count: u32,
    /// Weight index targets skeleton bone count (for bounds check).
    pub joint_count: u32,
    pub flags: u32,
    pub positions: Vec<[f32; 3]>,
    /// Empty when no normal stream.
    pub normals: Vec<[f32; 3]>,
    /// Empty when no uv0 stream.
    pub uv0: Vec<[f32; 2]>,
    /// Empty when no tangent stream.
    pub tangents: Vec<[f32; 4]>,
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    pub submeshes: Vec<SubmeshInfo>,
    /// Mesh own bind pose (same layout as skeleton reference pose).
    pub bind_pose: JointTransform,
}

impl SkinnedMeshInfo {
    pub fn has_normals(&self) -> bool {
        self.flags & RSKN_HAS_NORMALS != 0
    }

    pub fn has_uv0(&self) -> bool {
        self.flags & RSKN_HAS_UV0 != 0
    }

    pub fn has_tangents(&self) -> bool {
        self.flags & RSKN_HAS_TANGENTS != 0
    }

    /// Whether every vertex's 4 weights are normalized (check sum ≈ 1, tolerance 1e-3).
    pub fn weights_are_normalized(&self) -> bool {
        self.weights
            .iter()
            .all(|weights| (weights.iter().sum::<f32>() - 1.0).abs() <= 1e-3)
    }

    /// All joint indices are within `joint_count` range.
    pub fn joint_indices_are_in_range(&self) -> bool {
        self.joints
            .iter()
            .flatten()
            .all(|joint| (*joint as u32) < self.joint_count)
    }
}

/// Decode `.rskin`.
pub fn decode_rskin(data: &[u8]) -> Option<SkinnedMeshInfo> {
    if data.len() < RSKN_HEADER_SIZE + RSKN_BIND_POSE_SIZE || &data[..4] != RSKN_MAGIC {
        return None;
    }
    let mut reader = Reader::new(data, 4);
    if reader.u8()? != SKINNED_FORMAT_VERSION {
        return None;
    }
    let vertex_count = reader.u32()?;
    let index_count = reader.u32()?;
    let joint_count = reader.u32()?;
    let submesh_count = reader.u32()?;
    let flags = reader.u32()?;
    const KNOWN_FLAGS: u32 = RSKN_HAS_NORMALS | RSKN_HAS_UV0 | RSKN_HAS_TANGENTS;
    if flags & !KNOWN_FLAGS != 0 || joint_count == 0 {
        return None;
    }

    let verts = vertex_count as usize;
    let position_values = reader.f32s(verts.checked_mul(3)?)?;
    let positions: Vec<[f32; 3]> = position_values
        .as_chunks::<3>()
        .0
        .iter()
        .map(|chunk| [chunk[0], chunk[1], chunk[2]])
        .collect();

    let normals = if flags & RSKN_HAS_NORMALS != 0 {
        reader
            .f32s(verts.checked_mul(3)?)?
            .as_chunks::<3>()
            .0
            .iter()
            .map(|chunk| [chunk[0], chunk[1], chunk[2]])
            .collect()
    } else {
        Vec::new()
    };
    let uv0 = if flags & RSKN_HAS_UV0 != 0 {
        reader
            .f32s(verts.checked_mul(2)?)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| [chunk[0], chunk[1]])
            .collect()
    } else {
        Vec::new()
    };
    let tangents = if flags & RSKN_HAS_TANGENTS != 0 {
        reader
            .f32s(verts.checked_mul(4)?)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| [chunk[0], chunk[1], chunk[2], chunk[3]])
            .collect()
    } else {
        Vec::new()
    };

    let joint_values = reader.u16s(verts.checked_mul(4)?)?;
    let joints: Vec<[u16; 4]> = joint_values
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| [chunk[0], chunk[1], chunk[2], chunk[3]])
        .collect();
    let weight_values = reader.f32s(verts.checked_mul(4)?)?;
    let weights: Vec<[f32; 4]> = weight_values
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| [chunk[0], chunk[1], chunk[2], chunk[3]])
        .collect();

    let indices = reader.u32s(index_count as usize)?;

    let mut submeshes = Vec::with_capacity((submesh_count as usize).min(256));
    for _ in 0..submesh_count {
        let material_index = reader.u32()?;
        let index_offset = reader.u32()?;
        let index_count = reader.u32()?;
        let end = index_offset.checked_add(index_count)?;
        if end as usize > indices.len() {
            return None;
        }
        submeshes.push(SubmeshInfo {
            material_index,
            index_offset,
            index_count,
        });
    }

    let bind = reader.f32s(10)?;
    let mut bind_pose = [0.0f32; 10];
    bind_pose.copy_from_slice(&bind);
    if reader.remaining() != 0 {
        return None;
    }

    Some(SkinnedMeshInfo {
        vertex_count,
        index_count,
        joint_count,
        flags,
        positions,
        normals,
        uv0,
        tangents,
        joints,
        weights,
        indices,
        submeshes,
        bind_pose: JointTransform::from_array(bind_pose),
    })
}

/// One vec3 keyframe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3KeyInfo {
    pub time: f32,
    pub value: [f32; 3],
}

/// One rotation keyframe (quaternion xyzw, normalized and unified to `w >= 0` hemisphere at decode).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuatKeyInfo {
    pub time: f32,
    pub value: [f32; 4],
}

/// Single bone track.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimTrackInfo {
    pub joint: u32,
    pub positions: Vec<Vec3KeyInfo>,
    pub rotations: Vec<QuatKeyInfo>,
    pub scales: Vec<Vec3KeyInfo>,
}

/// One keyframe event in clip (#560 P1 Notify).
#[derive(Debug, Clone, PartialEq)]
pub struct AnimNotifyInfo {
    /// Trigger time (seconds, clip-local time).
    pub time: f32,
    /// Event name (`Footstep.L` / `Traversal.Window` etc., interpreted by project).
    pub name: String,
}

/// One Montage segment (#560 P4): jump by name / show in debug.
///
/// `start` / `end` are both clip-local time (seconds), `end` can equal `start` (zero-length segment,
/// used to mark a single time point). Encoder sorts by `start`, runtime looks up in same order.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimClipSectionInfo {
    pub name: String,
    pub start: f32,
    pub end: f32,
}

/// One MotionWarping window (#560 P4): root motion only allowed within this time window,
/// target point / orientation warping. `start` / `end` are clip-local time (seconds).
#[derive(Debug, Clone, PartialEq)]
pub struct AnimClipWarpWindowInfo {
    pub name: String,
    pub start: f32,
    pub end: f32,
}

/// Decoded `.ranim`.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimClipInfo {
    pub name: String,
    pub duration: f32,
    pub fps: f32,
    pub looped: bool,
    pub additive: bool,
    pub tracks: Vec<AnimTrackInfo>,
    /// Keyframe events (v1 clips always empty).
    pub notifies: Vec<AnimNotifyInfo>,
    /// Sync marker normalized phases (sorted, deduplicated; v1/v2 clips empty).
    pub sync_markers: Vec<f32>,
    /// Montage segments (sorted by `start`; v1-v3 clips empty).
    pub sections: Vec<AnimClipSectionInfo>,
    /// MotionWarping windows (sorted by `start`; v1-v4 clips empty).
    pub warp_windows: Vec<AnimClipWarpWindowInfo>,
}

impl AnimClipInfo {
    /// Latest keyframe time across all tracks (ignoring non-finite times).
    pub fn max_key_time(&self) -> f32 {
        let mut max = 0.0f32;
        for track in &self.tracks {
            for key in &track.positions {
                if key.time.is_finite() {
                    max = max.max(key.time);
                }
            }
            for key in &track.rotations {
                if key.time.is_finite() {
                    max = max.max(key.time);
                }
            }
            for key in &track.scales {
                if key.time.is_finite() {
                    max = max.max(key.time);
                }
            }
        }
        max
    }

    /// Actual duration: use `duration` if positive, otherwise take latest keyframe.
    pub fn clip_duration(&self) -> f32 {
        if self.duration > 0.0 {
            self.duration
        } else {
            self.max_key_time()
        }
    }
}

/// Decode `.ranim`.
pub fn decode_ranim(data: &[u8]) -> Option<AnimClipInfo> {
    if data.len() < 27 || &data[..4] != RANI_MAGIC {
        return None;
    }
    let mut reader = Reader::new(data, 4);
    let version = reader.u8()?;
    if version == 0 || version > RANI_VERSION {
        return None;
    }
    let name_len = reader.u16()? as usize;
    let name = String::from_utf8(reader.take(name_len)?.to_vec()).ok()?;
    let duration = reader.f32()?;
    let fps = reader.f32()?;
    let flags = reader.u16()?;
    if flags & !(RANI_LOOPED | RANI_ADDITIVE) != 0 {
        return None;
    }
    let track_count = reader.u32()? as usize;

    let mut tracks = Vec::with_capacity(track_count.min(4096));
    for _ in 0..track_count {
        let joint = reader.u32()?;

        let pos_count = reader.u32()? as usize;
        let mut positions = Vec::with_capacity(pos_count.min(65536));
        for _ in 0..pos_count {
            let time = reader.f32()?;
            let values = reader.f32s(3)?;
            positions.push(Vec3KeyInfo {
                time,
                value: [values[0], values[1], values[2]],
            });
        }

        let rot_count = reader.u32()? as usize;
        let mut rotations = Vec::with_capacity(rot_count.min(65536));
        for _ in 0..rot_count {
            let time = reader.f32()?;
            let values = reader.f32s(4)?;
            let mut value = [values[0], values[1], values[2], values[3]];
            let length = value.iter().map(|v| v * v).sum::<f32>().sqrt();
            if length > 1e-8 && length.is_finite() {
                for component in &mut value {
                    *component /= length;
                }
                if value[3] < 0.0 {
                    for component in &mut value {
                        *component = -*component;
                    }
                }
            } else {
                value = [0.0, 0.0, 0.0, 1.0];
            }
            rotations.push(QuatKeyInfo { time, value });
        }

        let scale_count = reader.u32()? as usize;
        let mut scales = Vec::with_capacity(scale_count.min(65536));
        for _ in 0..scale_count {
            let time = reader.f32()?;
            let values = reader.f32s(3)?;
            scales.push(Vec3KeyInfo {
                time,
                value: [values[0], values[1], values[2]],
            });
        }

        tracks.push(AnimTrackInfo {
            joint,
            positions,
            rotations,
            scales,
        });
    }

    let mut notifies = Vec::new();
    if version >= RANI_NOTIFY_VERSION {
        let notify_count = reader.u32()? as usize;
        if notify_count > 1 << 20 {
            return None;
        }
        notifies.reserve(notify_count);
        for _ in 0..notify_count {
            let time = reader.f32()?;
            let name_len = reader.u16()? as usize;
            let name = String::from_utf8(reader.take(name_len)?.to_vec()).ok()?;
            notifies.push(AnimNotifyInfo { time, name });
        }
    }

    let mut sync_markers = Vec::new();
    if version >= RANI_SYNC_MARKER_VERSION {
        let marker_count = reader.u32()? as usize;
        if marker_count > 1 << 16 {
            return None;
        }
        sync_markers.reserve(marker_count);
        for _ in 0..marker_count {
            let phase = reader.f32()?;
            if !phase.is_finite() {
                return None;
            }
            sync_markers.push(phase.rem_euclid(1.0));
        }
    }

    let mut sections = Vec::new();
    if version >= RANI_SECTION_VERSION {
        let section_count = reader.u32()? as usize;
        if section_count > 1 << 12 {
            return None;
        }
        sections.reserve(section_count);
        for _ in 0..section_count {
            let start = reader.f32()?;
            let end = reader.f32()?;
            let name_len = reader.u16()? as usize;
            let name = String::from_utf8(reader.take(name_len)?.to_vec()).ok()?;
            if name.trim().is_empty()
                || !start.is_finite()
                || !end.is_finite()
                || start < 0.0
                || end < start
            {
                return None;
            }
            sections.push(AnimClipSectionInfo { name, start, end });
        }
        sections.sort_by(|a, b| {
            a.start
                .total_cmp(&b.start)
                .then_with(|| a.name.cmp(&b.name))
        });
    }

    let mut warp_windows = Vec::new();
    if version >= RANI_WARP_WINDOW_VERSION {
        let window_count = reader.u32()? as usize;
        if window_count > 1 << 12 {
            return None;
        }
        warp_windows.reserve(window_count);
        for _ in 0..window_count {
            let start = reader.f32()?;
            let end = reader.f32()?;
            let name_len = reader.u16()? as usize;
            let name = String::from_utf8(reader.take(name_len)?.to_vec()).ok()?;
            if name.trim().is_empty()
                || !start.is_finite()
                || !end.is_finite()
                || start < 0.0
                || end < start
            {
                return None;
            }
            warp_windows.push(AnimClipWarpWindowInfo { name, start, end });
        }
        warp_windows.sort_by(|a, b| {
            a.start
                .total_cmp(&b.start)
                .then_with(|| a.name.cmp(&b.name))
        });
    }

    if reader.remaining() != 0 {
        return None;
    }

    Some(AnimClipInfo {
        name,
        duration,
        fps,
        looped: flags & RANI_LOOPED != 0,
        additive: flags & RANI_ADDITIVE != 0,
        tracks,
        notifies,
        sync_markers,
        sections,
        warp_windows,
    })
}

#[cfg(test)]
#[path = "formats_skinned_tests.rs"]
mod tests;
