//! Gameplay module (user project side).
//!
//! This holds only fishing-game-specific things: boat, fishing line, fish data, HUD, state machine.
//! Engine crates know no fishing concepts (see
//! `projects/demos/fishing/DESIGN.md` §2 layering contract).

pub mod boat;
pub mod fishing;
pub mod world;
