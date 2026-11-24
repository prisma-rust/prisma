//! Potato & Survivors — desktop player entry point.
//!
//! Launch from `projects/demos/survivors`:
//! ```text
//! cargo run --manifest-path ../Cargo.toml -p survivors
//! ```

#![deny(warnings)]

fn main() {
    survivors_core::build_app().run();
}
