use std::path::Path;

fn main() {
    let ini_path = Path::new("assets/settings.ini");
    println!("cargo:rerun-if-changed={}", ini_path.display());

    let text = std::fs::read_to_string(ini_path).unwrap_or_default();
    let code = prism_ini::emit_app_config_rs(&text);
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    std::fs::write(Path::new(&out_dir).join("embedded_config.rs"), code)
        .expect("write embedded_config.rs");
}
