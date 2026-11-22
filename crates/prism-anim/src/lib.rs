//! # prism-anim
//!
//! Skeleton, animation clip, and skinning evaluation (#560 P0-2).
//!
//! This layer only does math and does not depend on engine / render / ECS, so it can be shared
//! by the engine, editor, and offline tools:
//!
//! - [`Skeleton`] — hierarchy built from cooked `SkeletonInfo` (parent before child), with
//!   reference (bind) pose and `inverse_bind` matrix;
//! - [`Clip`] / [`Track`] — keyframe tracks from `AnimClipInfo`;
//! - [`Pose`] — author-space local TRS of each bone;
//! - [`Skeleton::bone_palette`] — `world(pose) * inverse_bind`, shared by GPU skinning and
//!   the CPU reference implementation;
//! - [`blend`] — pose blending and 1D blend space (foundation of animation graph / BlendStack);
//! - [`skinning`] — CPU reference skinning (golden baseline for the GPU path).
//!
//! Convention: column-major matrices (consistent with `prism-render` / engine), quaternion
//! `xyzw`, right-handed +Y up.

pub mod blend;
pub mod blend_space2d;
pub mod blend_stack;
pub mod camera_rigs;
pub mod clip;
pub mod foley;
pub mod graph_eval;
pub mod montage;
pub mod pose;
pub mod skeleton;
pub mod skinning;
pub mod state_machine;
pub mod sync;
pub mod traversal;
pub mod warping;

pub use blend::{
    additive_joint, apply_additive, blend_into, blend_joint, blend_poses, blend_space_weights,
    BlendSample, BlendSpace1D,
};
pub use blend_space2d::{BlendSample2D, BlendSpace2D};
pub use blend_stack::{BlendLayer, BlendStack, BoneMask};
pub use camera_rigs::{CameraRigBlender, CameraRigId, CameraRigParams};
pub use clip::{
    sample_quat, sample_vec3, Clip, ClipNotify, ClipSection, ClipWarpWindow, QuatKey, Track,
    Vec3Key,
};
pub use foley::{FoleyAction, FoleyDispatcher, FoleyEvent, SurfaceMaterialKind};
pub use graph_eval::{
    AnimGraphEvaluator, AnimGraphSource, GraphEvalError, GraphNodeSnapshot, GraphNotifyEvent,
    MontageSnapshot,
};
pub use montage::{Montage, MontageNotify, MontagePlayer, BRANCH_IN_PREFIX, MAX_MONTAGE_QUEUE};
pub use pose::Pose;
pub use prism_asset::formats_anim_graph::GraphNodeKind;
pub use prism_asset::formats_skinned::{
    decode_ranim, AnimClipInfo, AnimClipSectionInfo, AnimClipWarpWindowInfo, AnimNotifyInfo,
    AnimTrackInfo, QuatKeyInfo, Vec3KeyInfo,
};
pub use skeleton::{Joint, JointTransform, Skeleton};
pub use skinning::{palette_to_arrays, skin_matrices, skin_normals, skin_positions};
pub use state_machine::{
    AnimParameters, AnimState, AnimTransition, StateMachine, TransitionCondition, TransitionTarget,
    ANY_STATE,
};
pub use sync::{SyncGroup, SyncMarkerSet};
pub use warping::{apply_orientation_warping, apply_stride_warping, warp_root_motion_step};
