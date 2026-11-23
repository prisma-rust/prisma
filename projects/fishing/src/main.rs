//! Cozy Fishing — desktop entry point.
//!
//! Engine initialization, event loop and rendering handled by [`prism_app::app`]; this project only registers
//! gameplay systems (see `game::`). Scene content comes from `assets/scenes.pak`,
//! baked by `prism-build-pipeline build-scenes` from `assets/scenes/lake.scene.json`.
//!
//!
//! Run (working directory must be this project to resolve `assets/...`):
//! ```text
//! cd projects/fishing
//! cargo run
//! ```

#![deny(warnings)]

fn main() {
    fishing::build_app().run();
}
