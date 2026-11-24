//! Asset directory resolution — does not depend on process current working directory.
//!
//! Desktop startup methods are more than one: `cargo run` at repo root, running in `projects/template/`,
//! double-clicking `target/release/template.exe`, launcher spawning with absolute path, shortcut with modified
//! "start position" ... while `.pak` / `settings.ini` / `scenes.json` / environment textures were all
//! opened relative to `assets/...` directly from cwd. When cwd lacks `assets/` they all fail silently,
//! appearing as black screen or "no scenes".
//!
//! This module defines the **only** asset root resolution rule ([`asset_root`]), shared at runtime:
//!
//! 1. `PRISMA_ASSET_ROOT` (points to directory containing `assets/`) — explicit override, for tests / portable builds;
//! 2. current working directory and all its parents;
//! 3. executable's directory and all its parents (covers "started from elsewhere with absolute path").
//!
//! Take first candidate that **actually contains `assets/` subdirectory**, so it will not misjudge because some parent
//! happens to have a same-named file. Android does not use this chain: `run_on_android` unpacks APK assets
//! to `<filesDir>/assets` and `set_current_dir`, first candidate (cwd) hits,
//! behavior fully consistent with before.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Environment variable explicitly specifying asset root directory (**directory containing `assets/`**).
pub const ASSET_ROOT_ENV: &str = "PRISMA_ASSET_ROOT";

/// Environment switch: when set to `1`/`true`/`yes`/`on`, even if `.pak` already mounted, assets supporting hot
/// replacement such as environment textures will prefer loose files under `assets/` (for fast iteration after packaging).
pub const LOOSE_ASSETS_ENV: &str = "PRISMA_LOOSE_ASSETS";

static HOSTED_LOOSE_ALLOWED: AtomicBool = AtomicBool::new(false);

/// Allow loose authoring assets even in release builds (used by `App::hosted` / editor).
pub fn set_hosted_loose_allowed(allowed: bool) {
    HOSTED_LOOSE_ALLOWED.store(allowed, Ordering::Relaxed);
}

/// Whether runtime must strictly load from `.pak` only (no loose files on disk).
///
/// Active in release / dist / tiny builds (`not(debug_assertions)`) unless running
/// in hosted/editor mode.
pub fn strict_pak_only() -> bool {
    !cfg!(debug_assertions) && !HOSTED_LOOSE_ALLOWED.load(Ordering::Relaxed)
}

/// Check whether loose-asset-preference override enabled (`PRISMA_LOOSE_ASSETS`).
/// Always `false` when [`strict_pak_only`] is active.
pub fn loose_assets_enabled() -> bool {
    if strict_pak_only() {
        return false;
    }
    let Ok(val) = std::env::var(LOOSE_ASSETS_ENV) else {
        return false;
    };
    matches!(
        val.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Asset subdirectory name, all `assets/...` relative paths are mounted under it.
const ASSET_DIR_NAME: &str = "assets";

/// Directory for optional packages written by split build (#509): `assets/optional/optional_*.pak`.
///
/// Auto-mount only scans top level of `assets/`, so packages in this directory **will not** be persistent at startup;
/// call `GpuAssetResolver::load_optional_packages` explicitly when needed.
pub const OPTIONAL_PAK_DIR_NAME: &str = "optional";

static WARNED_MISSING_ROOT: AtomicBool = AtomicBool::new(false);

/// Candidate root directories, sorted by priority (see module docs).
///
/// Callers needing to try multiple relative paths themselves (e.g. scene manifest also needs to support old format
/// `crates/prism-engine/assets/`) can iterate it directly.
#[expect(
    clippy::disallowed_methods,
    reason = "single source of truth for runtime asset root resolution"
)]
pub fn search_roots() -> Vec<PathBuf> {
    let cwd = std::env::current_dir().ok();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let env_root = std::env::var_os(ASSET_ROOT_ENV).map(PathBuf::from);
    collect_search_roots(env_root.as_deref(), cwd.as_deref(), exe_dir.as_deref())
}

/// Resolve directory containing `assets/` (order in module docs).
///
/// Returns `None` when not found, and **warns only once per process** — call sites usually have two or three,
/// repeated warnings would only make startup log noisier.
pub fn asset_root() -> Option<PathBuf> {
    let root = find_asset_root(&search_roots());
    match &root {
        Some(root) => log::trace!("asset root: {}", root.display()),
        None => {
            if !WARNED_MISSING_ROOT.swap(true, Ordering::Relaxed) {
                log::warn!(
                    "no `assets/` directory found from the current directory or the executable \
                     directory; resources (.pak / settings.ini / scenes.json) will not load \
                     (set {ASSET_ROOT_ENV} to override)"
                );
            }
        }
    }
    root
}

/// `<root>/assets` — base directory for all asset relative paths.
pub fn asset_dir() -> Option<PathBuf> {
    asset_root().map(|root| root.join(ASSET_DIR_NAME))
}

/// Resolve a relative path under `assets/`, e.g. `asset_path("scenes.pak")`
/// → `<root>/assets/scenes.pak`.
pub fn asset_path(relative: impl AsRef<Path>) -> Option<PathBuf> {
    asset_dir().map(|dir| dir.join(relative.as_ref()))
}

/// Copy-pasteable two-step `build-scenes` commands: first cd to project directory, then run
/// `prism-build-pipeline` with full arguments. Just writing "run build-scenes" leaves reader guessing
/// error forms like `cargo run build-scenes`.
///
/// `assets_dir` is the project's `assets/` directory: use `Cargo.toml` when project has its own; projects under
/// `projects/<name>/` use `../../Cargo.toml` (repo root workspace).
/// When repo root not found fall back to form without `--manifest-path`.
pub fn build_scenes_steps(assets_dir: &Path) -> [String; 2] {
    let project = assets_dir.parent().unwrap_or(assets_dir);
    let project = project
        .canonicalize()
        .unwrap_or_else(|_| project.to_path_buf());
    match repo_root_for(&project) {
        Some(repo) => {
            let manifest = if repo == project {
                "Cargo.toml".to_string()
            } else {
                let depth = project
                    .components()
                    .count()
                    .saturating_sub(repo.components().count());
                let mut manifest = "../".repeat(depth.max(1));
                manifest.push_str("Cargo.toml");
                manifest
            };
            [
                format!("cd {}", project.display()),
                format!(
                    "cargo run -q --manifest-path {manifest} -p prism-build-pipeline -- build-scenes"
                ),
            ]
        }
        None => [
            format!("cd {}", project.display()),
            "cargo run -q -p prism-build-pipeline -- build-scenes".to_string(),
        ],
    }
}

/// Log form of [`build_scenes_steps`] (one line, joined with `&&`).
pub fn build_scenes_command(assets_dir: &Path) -> String {
    build_scenes_steps(assets_dir).join(" && ")
}

fn repo_root_for(project: &Path) -> Option<PathBuf> {
    let candidates = [project.join("../.."), project.to_path_buf()];
    for candidate in candidates {
        let Ok(candidate) = candidate.canonicalize() else {
            continue;
        };
        if candidate.join("Cargo.toml").is_file()
            && candidate.join("crates/prism-build-pipeline").is_dir()
        {
            return Some(candidate);
        }
    }
    None
}

fn collect_search_roots(
    env_root: Option<&Path>,
    cwd: Option<&Path>,
    exe_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    if let Some(root) = env_root {
        push_unique(&mut roots, root.to_path_buf());
    }

    for start in [cwd, exe_dir].into_iter().flatten() {
        let mut current = Some(start);
        while let Some(dir) = current {
            push_unique(&mut roots, dir.to_path_buf());
            current = dir.parent();
        }
    }

    roots
}

fn push_unique(roots: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !roots.contains(&candidate) {
        roots.push(candidate);
    }
}

fn find_asset_root(roots: &[PathBuf]) -> Option<PathBuf> {
    roots
        .iter()
        .find(|dir| dir.join(ASSET_DIR_NAME).is_dir())
        .cloned()
}

/// All `.pak` files under `dir`, sorted by filename (#530 multi-package mounting).
///
/// Packager writes multiple packages by content group (scene package, asset package...), runtime cannot hardcode
/// `scenes.pak` again. Sorting only keeps startup log and parse order stable.
pub fn pak_files_in(dir: &Path) -> Vec<PathBuf> {
    let mut paks: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("pak"))
        })
        .collect();
    paks.sort();
    paks
}

/// All `.pak` under `assets/` (resolved version of [`pak_files_in`]).
pub fn pak_files() -> Vec<PathBuf> {
    asset_dir()
        .map(|dir| pak_files_in(&dir))
        .unwrap_or_default()
}

/// `assets/optional/` — optional package directory of split build (#509).
pub fn optional_pak_dir() -> Option<PathBuf> {
    asset_dir().map(|dir| dir.join(OPTIONAL_PAK_DIR_NAME))
}

/// All `.pak` under `assets/optional/`, sorted by filename.
///
/// These packages are not auto-mounted at startup; whoever needs them calls
/// `GpuAssetResolver::load_optional_packages` to register explicitly.
pub fn optional_pak_files() -> Vec<PathBuf> {
    optional_pak_dir()
        .map(|dir| pak_files_in(&dir))
        .unwrap_or_default()
}

/// Whether a loose asset (outside `.pak`) exists. Editor preview and user project key-card JSON,
/// time-of-day fallback textures all go through here, no need to assemble `assets/` relative paths again.
///
/// loose file preferred; when not found look again in `assets/*.pak` path manifest (#1049), so
/// key-card data packed into package on Android / release is equally visible.
pub fn asset_exists(relative: impl AsRef<Path>) -> bool {
    let relative = relative.as_ref();
    if !strict_pak_only() && asset_path(relative).is_some_and(|path| path.is_file()) {
        return true;
    }
    packaged_asset_exists(&package_key(relative))
}

/// Read loose asset raw bytes; fall back to `.pak` when loose file missing (#1049).
/// Under [`strict_pak_only`], only reads from `.pak` and never touches loose files.
pub fn read_asset_file(relative: impl AsRef<Path>) -> std::io::Result<Vec<u8>> {
    let relative = relative.as_ref();
    if !strict_pak_only() {
        if let Some(path) = asset_path(relative) {
            if path.is_file() {
                return std::fs::read(&path);
            }
        }
    }
    if let Some(bytes) = packaged_asset_bytes(&package_key(relative)) {
        return Ok(bytes);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        if strict_pak_only() {
            format!("asset not found in package: {}", relative.display())
        } else {
            format!(
                "asset not found (loose or packaged): {}",
                relative.display()
            )
        },
    ))
}

/// Read loose text asset (key-card config, manifest, etc.); UTF-8 version of `read_asset_file`.
pub fn read_asset_text(relative: impl AsRef<Path>) -> std::io::Result<String> {
    let bytes = read_asset_file(relative)?;
    String::from_utf8(bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

/// Cache entry for already-open `.pak` reader.
///
/// `PackageReader::open` validates whole-package checksum, cannot reopen on every query;
/// use `(mtime, len)` to judge whether package was rebuilt (same criteria as hot reload; Android
/// FUSE timestamp granularity is 1 second, mtime alone would miss).
struct CachedPak {
    path: PathBuf,
    stamp: (u64, u64),
    reader: std::sync::Arc<prism_asset::package::PackageReader>,
}

static PAK_CACHE: std::sync::OnceLock<std::sync::Mutex<Vec<CachedPak>>> =
    std::sync::OnceLock::new();

/// Path key inside `.pak`: forward slashes, leading `./` removed (input to `AssetId::from_path`).
fn package_key(relative: &Path) -> String {
    relative
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string()
}

fn pak_stamp(path: &Path) -> Option<(u64, u64)> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    Some((modified, metadata.len()))
}

/// Refresh cache with current `.pak` list under `assets/`: keep unchanged readers, reopen
/// newly rebuilt packages, drop deleted ones.
fn refresh_pak_cache(cache: &mut Vec<CachedPak>) {
    let mut previous = std::mem::take(cache);
    let mut next = Vec::with_capacity(previous.len());
    for path in pak_files() {
        let Some(stamp) = pak_stamp(&path) else {
            continue;
        };
        if let Some(index) = previous
            .iter()
            .position(|cached| cached.path == path && cached.stamp == stamp)
        {
            next.push(previous.swap_remove(index));
            continue;
        }
        match prism_asset::package::PackageReader::open(&path) {
            Ok(reader) => next.push(CachedPak {
                path,
                stamp,
                reader: std::sync::Arc::new(reader),
            }),
            Err(error) => log::debug!(
                "cannot open {} for packaged-asset lookup: {error}",
                path.display()
            ),
        }
    }
    *cache = next;
}

fn with_pak_cache<R>(f: impl FnOnce(&[CachedPak]) -> R) -> R {
    let cache = PAK_CACHE.get_or_init(Default::default);
    let mut guard = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    refresh_pak_cache(&mut guard);
    f(&guard)
}

fn packaged_asset_exists(key: &str) -> bool {
    if key.is_empty() {
        return false;
    }
    let id = prism_asset::AssetId::from_path(key);
    with_pak_cache(|paks| paks.iter().any(|pak| pak.reader.find_record(id).is_some()))
}

fn packaged_asset_bytes(key: &str) -> Option<Vec<u8>> {
    if key.is_empty() {
        return None;
    }
    let id = prism_asset::AssetId::from_path(key);
    with_pak_cache(|paks| {
        for pak in paks {
            let Some(record) = pak.reader.find_record(id) else {
                continue;
            };
            match pak.reader.read_asset_record_data(record) {
                Ok(bytes) => return Some(bytes),
                Err(error) => {
                    log::warn!("read packaged '{key}' from {}: {error}", pak.path.display())
                }
            }
        }
        None
    })
}

#[cfg(test)]
#[path = "asset_paths_tests.rs"]
mod tests;
