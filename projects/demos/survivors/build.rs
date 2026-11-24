//! Build script: embeds `settings.ini` and packs `assets/scenes.pak` on release-like profiles.

use std::path::Path;

fn main() {
    let ini_path = Path::new("assets/settings.ini");
    println!("cargo:rerun-if-changed={}", ini_path.display());

    let text = std::fs::read_to_string(ini_path).unwrap_or_default();
    let code = prism_ini::emit_app_config_rs(&text);
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    std::fs::write(Path::new(&out_dir).join("embedded_config.rs"), code)
        .expect("write embedded_config.rs");

    println!("cargo:rerun-if-env-changed=PROFILE");
    let profile = std::env::var("PROFILE").unwrap_or_default();
    if matches!(profile.as_str(), "release" | "dist" | "tiny") {
        pack_scenes();
    }
}

fn pack_scenes() {
    for watch in [
        "assets/scenes.json",
        "assets/scenes",
        "assets/data",
        "assets/sprites",
        "assets/audio",
    ] {
        println!("cargo:rerun-if-changed={watch}");
    }

    let manifest = prism_build_pipeline::resolve_scene_manifest(Path::new("assets/scenes.json"))
        .expect("resolve survivors scene manifest");
    prism_build_pipeline::build_scene_package(
        &manifest,
        Path::new("target/scene_pack"),
        Path::new("assets/scenes.pak"),
        3,
    )
    .expect("build survivors assets/scenes.pak");
}
