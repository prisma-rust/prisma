//! Analytic IK solvers (#560 P4), independent of engine, render, and ECS.
//!
//! Conventions match `prism-anim`: column-major matrices, `xyzw` quaternions,
//! right-handed +Y up, with positions in component space.

pub mod foot;
pub mod pbik;
pub mod two_bone;

pub use foot::{
    FootPlacement, FootPlacementReport, FootPlacementSettings, FootState, GroundHit, GroundQuery,
};
pub use two_bone::{apply_two_bone_ik, solve_two_bone, TwoBoneChain, TwoBoneSolution};
