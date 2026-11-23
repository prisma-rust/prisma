//! Tests for the cwd-independent asset root discovery.

use super::*;

struct TempTree {
    path: PathBuf,
}

impl TempTree {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "prism-asset-paths-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp tree");
        Self { path }
    }

    /// Create `<self>/<rel>/assets` so that a search rooted at `<self>/<rel>` hits.
    fn with_assets(&self, rel: &str) -> PathBuf {
        let root = if rel.is_empty() {
            self.path.clone()
        } else {
            self.path.join(rel)
        };
        std::fs::create_dir_all(root.join("assets")).expect("create assets dir");
        root
    }

    fn dir(&self, rel: &str) -> PathBuf {
        let dir = self.path.join(rel);
        std::fs::create_dir_all(&dir).expect("create dir");
        dir
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn finds_assets_in_the_working_directory() {
    let tree = TempTree::new("cwd");
    let root = tree.with_assets("");

    let roots = collect_search_roots(None, Some(&root), None);
    assert_eq!(find_asset_root(&roots), Some(root));
}

#[test]
fn walks_up_from_a_nested_working_directory() {
    let tree = TempTree::new("nested");
    let root = tree.with_assets("");
    let nested = tree.dir("a/b");

    let roots = collect_search_roots(None, Some(&nested), None);
    assert_eq!(find_asset_root(&roots), Some(root));
}

#[test]
fn env_override_wins_over_the_working_directory() {
    let tree = TempTree::new("env");
    let cwd_root = tree.with_assets("cwd-root");
    let env_root = tree.with_assets("env-root");

    let roots = collect_search_roots(Some(&env_root), Some(&cwd_root), None);
    assert_eq!(find_asset_root(&roots), Some(env_root));
}

#[test]
fn executable_directory_is_searched_after_the_working_directory() {
    let tree = TempTree::new("exe");
    let exe_root = tree.with_assets("install");
    let cwd = tree.dir("unrelated");
    let exe_dir = exe_root.join("bin");
    std::fs::create_dir_all(&exe_dir).unwrap();

    let roots = collect_search_roots(None, Some(&cwd), Some(&exe_dir));
    assert_eq!(find_asset_root(&roots), Some(exe_root.clone()));

    let cwd_root = tree.with_assets("unrelated");
    let roots = collect_search_roots(None, Some(&cwd_root), Some(&exe_dir));
    assert_eq!(find_asset_root(&roots), Some(cwd_root));
}

#[test]
fn returns_none_without_an_assets_directory() {
    let tree = TempTree::new("none");
    let empty = tree.dir("empty");
    let roots = collect_search_roots(None, Some(&empty), Some(&empty));
    assert_eq!(find_asset_root(&roots), None);
}

#[test]
fn asset_path_joins_under_the_assets_directory() {
    let tree = TempTree::new("join");
    let root = tree.with_assets("");
    let assets = root.join("assets");
    std::fs::write(assets.join("scenes.pak"), b"pak").unwrap();

    let resolved = find_asset_root(&collect_search_roots(None, Some(&root), None))
        .map(|found| found.join(ASSET_DIR_NAME).join("scenes.pak"));
    assert_eq!(resolved, Some(assets.join("scenes.pak")));
    assert!(resolved.unwrap().exists());
}

/// #530: at runtime mount all `.pak` under `assets/`, not hardcoding `scenes.pak`.
#[test]
fn pak_files_lists_every_package_sorted() {
    let tree = TempTree::new("pak-list");
    let dir = tree.dir("assets");
    std::fs::write(dir.join("scenes.pak"), b"a").unwrap();
    std::fs::write(dir.join("assets.pak"), b"b").unwrap();
    std::fs::write(dir.join("scenes.pak.meta.json"), b"{}").unwrap();
    std::fs::create_dir(dir.join("nested.pak")).unwrap();

    let found: Vec<String> = pak_files_in(&dir)
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(found, vec!["assets.pak", "scenes.pak"]);
}

/// #509: split packages written into `assets/optional/` subdirectory, top-level auto-scan will not bring them out;
/// the mounter uses `optional_pak_files()` / `load_optional_packages()` to get them explicitly.
#[test]
fn optional_packages_stay_out_of_the_top_level_scan() {
    let tree = TempTree::new("optional-pak");
    let dir = tree.dir("assets");
    let optional = tree.dir("assets/optional");
    std::fs::write(dir.join("game.pak"), b"base").unwrap();
    std::fs::write(optional.join("optional_audio.pak"), b"audio").unwrap();
    std::fs::write(optional.join("optional_audio.pak.meta.json"), b"{}").unwrap();

    let top: Vec<String> = pak_files_in(&dir)
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(top, vec!["game.pak"]);

    let split: Vec<String> = pak_files_in(&optional)
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(split, vec!["optional_audio.pak"]);
}

/// Test sets `PRISMA_ASSET_ROOT` during, restores original value at end.
struct EnvRoot {
    previous: Option<std::ffi::OsString>,
}

impl EnvRoot {
    fn set(root: &Path) -> Self {
        let previous = std::env::var_os(ASSET_ROOT_ENV);
        std::env::set_var(ASSET_ROOT_ENV, root);
        Self { previous }
    }
}

impl Drop for EnvRoot {
    fn drop(&mut self) {
        match self.previous.as_ref() {
            Some(value) => std::env::set_var(ASSET_ROOT_ENV, value),
            None => std::env::remove_var(ASSET_ROOT_ENV),
        }
    }
}

/// #1049: when loose file absent, `asset_exists` / `read_asset_text` fall back to `.pak`.
/// Packaged version (Android / release) only has `.pak`, level data must still be readable.
#[test]
fn packaged_assets_are_visible_without_loose_files() {
    use prism_asset::package::PackageBuilder;
    use prism_asset::{AssetId, AssetType};

    fn write_pak(assets: &Path, entries: &[(&str, &[u8])]) {
        let mut builder = PackageBuilder::new();
        for (path, bytes) in entries {
            builder.add_asset(
                AssetId::from_path(path),
                AssetType::Binary,
                bytes.to_vec(),
                &[],
            );
        }
        std::fs::write(assets.join("scenes.pak"), builder.build().unwrap()).unwrap();
    }

    let tree = TempTree::new("pak-fallback");
    let root = tree.with_assets("");
    let assets = root.join("assets");
    let layers = br#"{"layers":[]}"#;
    write_pak(&assets, &[("data/lake_layers.json", layers)]);

    let _env = EnvRoot::set(&root);
    assert!(
        !asset_path("data/lake_layers.json").unwrap().exists(),
        "Fallback test must have no loose files"
    );
    assert!(asset_exists("data/lake_layers.json"));
    assert!(!asset_exists("data/missing.json"));
    assert_eq!(
        read_asset_text("data/lake_layers.json").unwrap(),
        String::from_utf8_lossy(layers)
    );
    assert!(read_asset_text("data/missing.json").is_err());
    assert!(asset_exists("./data\\lake_layers.json"));

    write_pak(&assets, &[("data/lake_layers.json", b"v2")]);
    assert_eq!(
        read_asset_text("data/lake_layers.json").unwrap(),
        "v2".to_string()
    );
}

#[test]
fn build_scenes_steps_use_the_project_dir_and_workspace_manifest() {
    let tree = TempTree::new("build-scenes-steps");
    std::fs::create_dir_all(tree.path.join("crates/prism-build-pipeline")).unwrap();
    std::fs::write(tree.path.join("Cargo.toml"), "[workspace]\n").unwrap();
    let assets = tree.path.join("projects/demo/assets");
    std::fs::create_dir_all(&assets).unwrap();

    let steps = build_scenes_steps(&assets);
    assert!(steps[0].starts_with("cd "), "{}", steps[0]);
    let cd_dir = steps[0].trim_start_matches("cd ");
    assert!(
        std::path::Path::new(cd_dir).ends_with("projects/demo"),
        "{}",
        steps[0]
    );
    assert_eq!(
        steps[1],
        "cargo run -q --manifest-path ../../Cargo.toml -p prism-build-pipeline -- build-scenes"
    );

    let command = build_scenes_command(&assets);
    assert!(command.contains(" && cargo run -q --manifest-path"));
    assert!(command.contains("prism-build-pipeline -- build-scenes"));
}

#[test]
fn build_scenes_steps_fall_back_without_a_workspace_manifest() {
    let tree = TempTree::new("build-scenes-steps-fallback");
    let assets = tree.path.join("assets");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(tree.path.join("Cargo.toml"), "[workspace]\n").unwrap();

    let steps = build_scenes_steps(&assets);
    assert!(steps[0].starts_with("cd "));
    assert_eq!(
        steps[1],
        "cargo run -q -p prism-build-pipeline -- build-scenes"
    );
}
