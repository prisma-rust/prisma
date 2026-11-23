//! Scene package assembly: `scenes.json` + cooked RSCN → `scenes.pak`.
//!
//! Runtime scene loading resolves `scenes.json` paths through the `.pak` path
//! manifest, so the packaged layout must keep exactly the paths declared in
//! the manifest (`scenes/intro.rscn`, `scenes/default.rscn`, …). This module
//! owns that layout instead of asking users to copy files by hand.

use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use prism_asset::GuidIndex;
use prism_cooker::cooker::scene::{parse_rscn_header, SceneCooker};
use prism_cooker::{
    pack_directory_split, pack_directory_with_options, OptionalPackSummary, PackOptions,
};
use prism_scene::references::{sync_document, SyncMode};

/// Package manifest file name inside package---runtime looks it up in `.pak` path manifest.
pub const SCENE_MANIFEST_NAME: &str = "scenes.json";

/// Summary of a completed scene-package build.
#[derive(Debug, Clone)]
pub struct ScenePackSummary {
    /// Number of scene entries cooked or copied into the package.
    pub scenes: usize,
    /// Entries skipped because they are not `.scene.json` / `.rscn` (e.g. a
    /// developer-local glTF path in `scenes.json`).
    pub skipped: usize,
    /// Summary of the underlying directory packer.
    pub pack: prism_cooker::PackSummary,
    /// Optional packages split out by `--optional-dir` (empty when the build
    /// writes a single package).
    pub optional: Vec<OptionalPackSummary>,
}

/// Cook every scene referenced by `manifest_path` into `staging`, rewrite the
/// manifest's authoring paths next to them, then pack `staging` into `output`.
///
/// `.scene.json` entries are cooked to the sibling `.rscn` path. `.rscn`
/// entries are re-cooked from a sibling `.scene.json` when possible, otherwise
/// copied verbatim. Non-scene entries (external glTF models) are skipped.
///
/// `staging` is a generated directory and is cleared before the build; it must
/// not contain the source manifest.
pub fn build_scene_package(
    manifest_path: impl AsRef<Path>,
    staging: impl AsRef<Path>,
    output: impl AsRef<Path>,
    compression: u32,
) -> Result<ScenePackSummary> {
    build_scene_package_with_options(
        manifest_path,
        staging,
        output,
        PackOptions {
            compression,
            stream_threshold_bytes: None,
        },
    )
}

/// Like [`build_scene_package`], but controls packer behaviour such as the
/// `FLAG_STREAMED` size cutoff (#509).
pub fn build_scene_package_with_options(
    manifest_path: impl AsRef<Path>,
    staging: impl AsRef<Path>,
    output: impl AsRef<Path>,
    pack_options: PackOptions,
) -> Result<ScenePackSummary> {
    build_scene_package_split(manifest_path, staging, output, pack_options, &[])
}

/// Like [`build_scene_package_with_options`], but also splits staged
/// subdirectories into `optional/optional_<group>.pak` (#509).
///
/// `optional_dirs` are relative to `staging`, so they use the packaged layout
/// (`textures`, `env`, `meshes`, `audio/music`, …) rather than the authoring
/// project paths.
pub fn build_scene_package_split(
    manifest_path: impl AsRef<Path>,
    staging: impl AsRef<Path>,
    output: impl AsRef<Path>,
    pack_options: PackOptions,
    optional_dirs: &[String],
) -> Result<ScenePackSummary> {
    let manifest_path = manifest_path.as_ref();
    let staging = staging.as_ref();
    let output = output.as_ref();

    if !manifest_path.is_file() {
        bail!("scene manifest not found: {}", manifest_path.display());
    }
    if staging.components().all(|c| matches!(c, Component::CurDir)) {
        bail!("refusing to use the current directory as scene staging");
    }
    if manifest_path.starts_with(staging) {
        bail!(
            "scene staging {} must not contain the source manifest {}",
            staging.display(),
            manifest_path.display()
        );
    }

    // Clearing a generated staging directory is safe only when we created it.
    // The marker also survives a failed pack attempt, so the next run can retry.
    let marker = staging_marker(staging)?;
    if staging.exists() {
        let non_empty = fs::read_dir(staging)
            .with_context(|| format!("read scene staging {}", staging.display()))?
            .next()
            .is_some();
        if non_empty && !marker.is_file() {
            bail!(
                "scene staging {} is not empty and was not created by build-scenes; \
                 choose another --staging directory or remove it manually",
                staging.display()
            );
        }
    }

    let manifest_text = fs::read_to_string(manifest_path)
        .with_context(|| format!("read {}", manifest_path.display()))?;
    let mut manifest = prism_scene::SceneManifest::parse(&manifest_text)
        .map_err(|error| anyhow::anyhow!("parse {}: {error}", manifest_path.display()))?;
    let asset_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let guid_index = GuidIndex::scan(asset_root);
    resolve_manifest_guids(&mut manifest, &guid_index);

    if staging.exists() {
        fs::remove_dir_all(staging)
            .with_context(|| format!("clear scene staging {}", staging.display()))?;
    }
    fs::create_dir_all(staging)
        .with_context(|| format!("create scene staging {}", staging.display()))?;

    let cooker = SceneCooker::default();
    let mut scenes = 0usize;
    let mut skipped = 0usize;
    for entry in &manifest.scenes {
        if !entry.enabled {
            skipped += 1;
            continue;
        }
        match stage_entry(&cooker, asset_root, &guid_index, staging, &entry.path)? {
            Some(_) => scenes += 1,
            None => skipped += 1,
        }
    }
    if scenes == 0 {
        bail!(
            "no cookable scenes in {} (expected .scene.json or .rscn entries)",
            manifest_path.display()
        );
    }

    // Scene-referenced resources must all go into package (#530). Any reference that cannot be resolved is collected into
    // `missing`, all reported at build end---otherwise package looks fine but runs as
    // empty scene, just one warn in log.
    let mut missing: Vec<String> = Vec::new();

    // Scene-declared env cube must be inside package, otherwise runtime IBL falls back to empty env (black screen).
    stage_environment_assets(asset_root, &guid_index, staging, &manifest, &mut missing)?;

    // Same for 2D sprite texture: `SpriteRenderer.sprite_path` PNG must be baked into package
    // RTEX format expected at runtime, otherwise sprite forever cannot find its texture.
    stage_sprite_assets(asset_root, &guid_index, staging, &manifest, &mut missing)?;
    // 3D mesh & material (#530): previously only scene cooked; imported model disappears after packaging.
    stage_mesh_assets(asset_root, &guid_index, staging, &manifest, &mut missing)?;
    stage_material_assets(asset_root, &guid_index, staging, &manifest, &mut missing)?;
    stage_shader_assets(asset_root, &guid_index, staging, &manifest, &mut missing)?;
    // UI component texture / font (#591 / #583): `Style.image`, `Tilemap.tileset_path`,
    // `Text.font` similarly must go into package, otherwise icons, tilesets, and project fonts all disappear after packaging.
    stage_ui_assets(asset_root, &guid_index, staging, &manifest, &mut missing)?;
    // Scene audio (#990): `AudioSource.sound_path` was never packaged before; release build is silent.
    stage_audio_assets(asset_root, &guid_index, staging, &manifest, &mut missing)?;
    // Sprites generated at runtime (bg layer, fish, effects) not in scene JSON, so entire
    // `sprites/` directory must be baked into package---consistent with engine "assets dir IS content" principle.
    stage_sprite_folder(asset_root, staging)?;
    // One-shot sound (#987) same: `GameWorld::play_sound("audio/xxx.wav")` is runtime string; build-time cannot enumerate, so whole `audio/` directory is packaged as-is.
    stage_audio_folder(asset_root, staging)?;
    // Project data (#1049): level config etc JSON also read at runtime by path string, no component reference to enumerate;
    // `data/` whole directory packaged as-is.
    stage_data_folder(asset_root, staging)?;
    stage_shader_folder(asset_root, staging)?;

    report_missing_references(missing)?;

    // Runtime always looks up the manifest as `scenes.json` inside the package.
    // Rewrite authoring `.scene.json` paths to the cooked `.rscn` names.
    let mut packaged = manifest.clone();
    for entry in &mut packaged.scenes {
        entry.guid = None;
        if let Some(stem) = entry.path.strip_suffix(".scene.json") {
            entry.path = format!("{stem}.rscn");
        }
    }
    let packaged_manifest_path = staging.join(SCENE_MANIFEST_NAME);
    let packaged_manifest =
        serde_json::to_string_pretty(&packaged).context("serialize the packaged scene manifest")?;
    fs::write(&packaged_manifest_path, format!("{packaged_manifest}\n"))
        .with_context(|| format!("write {}", packaged_manifest_path.display()))?;

    // #976: Write build stamp into package; runtime judges package freshness by it (old package doesn't have this,
    // will warn "built by older pipeline" on first mount).
    write_cook_stamp(asset_root, staging)?;

    fs::write(&marker, b"generated by prism-build-pipeline build-scenes\n")
        .with_context(|| format!("write {}", marker.display()))?;
    let (pack, optional) = if optional_dirs.is_empty() {
        (
            pack_directory_with_options(staging, output, pack_options)?,
            Vec::new(),
        )
    } else {
        let split = pack_directory_split(staging, output, pack_options, optional_dirs)?;
        (split.base, split.optional)
    };
    Ok(ScenePackSummary {
        scenes,
        skipped,
        pack,
        optional,
    })
}

fn write_cook_stamp(asset_root: &Path, staging: &Path) -> Result<()> {
    let stamp = prism_scene::cook_stamp::CookStamp::new(
        prism_scene::builtin_schema_fingerprint(),
        prism_scene::cook_stamp::newest_source_mtime(asset_root),
    );
    let path = staging.join(prism_scene::cook_stamp::COOK_STAMP_NAME);
    fs::write(&path, stamp.to_json()).with_context(|| format!("write {}", path.display()))?;
    log::debug!(
        "cook stamp: schema {:#x}, sources mtime {}",
        stamp.schema,
        stamp.sources_mtime
    );
    Ok(())
}

fn stage_entry(
    cooker: &SceneCooker,
    asset_root: &Path,
    guid_index: &GuidIndex,
    staging: &Path,
    entry_path: &str,
) -> Result<Option<PathBuf>> {
    let Some(stem) = entry_path.strip_suffix(".scene.json") else {
        if entry_path.ends_with(".rscn") {
            let relative = safe_relative(entry_path)?;
            let source = asset_root.join(&relative);
            let source_json = source.with_extension("scene.json");
            let cooked = if source_json.is_file() {
                cook_scene_file(cooker, asset_root, guid_index, &source_json, entry_path)?
            } else {
                if !source.is_file() {
                    bail!(
                        "scene entry '{entry_path}' has neither {} nor {}",
                        source.display(),
                        source_json.display()
                    );
                }
                // When no author doc, only repackage existing `.rscn` as-is: it came from which tool,
                // which tool version baked it, build side completely cannot tell (cook stamp's
                // `sources_mtime` also only counts document). At least say it, so
                // "picture wrong but logs all green" happens again (#976).
                log::warn!(
                    "scene entry '{entry_path}' is packaged from a pre-cooked .rscn with no source document: freshness cannot be verified, rebuild it from a source document if the scene looks wrong"
                );
                fs::read(&source).with_context(|| format!("read {}", source.display()))?
            };
            validate_rscn(entry_path, &cooked)?;
            write_staged(staging, &relative, &cooked)?;
            return Ok(Some(relative));
        }

        log::warn!("scene entry '{entry_path}' is not .scene.json/.rscn; skipping");
        return Ok(None);
    };

    safe_relative(entry_path)?;
    let source = asset_root.join(format!("{stem}.scene.json"));
    let cooked = cook_scene_file(cooker, asset_root, guid_index, &source, entry_path)?;
    let target = PathBuf::from(format!("{stem}.rscn"));
    validate_rscn(entry_path, &cooked)?;
    write_staged(staging, &target, &cooked)?;
    Ok(Some(target))
}

/// Copy env texture referenced by scene document into staging, path consistent with runtime parse result.
///
/// Supports `EnvironmentLighting.env_map` and old-style `Skybox.hdr_path`; path can be
/// scene-relative (`../valley.hdr`) or project-root relative (`assets/valley.hdr`).
///
/// When author doc (`*.scene.json`) exists, use it as authoritative. Only `.rscn` scenes have no author doc,
/// in which case take `Skybox.hdr_path` from already-staged RSCN---otherwise this scene packaged
/// without env cube, runtime IBL falls back to empty env (black screen).
fn stage_environment_assets(
    asset_root: &Path,
    guid_index: &GuidIndex,
    staging: &Path,
    manifest: &prism_scene::SceneManifest,
    missing: &mut Vec<String>,
) -> Result<usize> {
    let mut staged = std::collections::BTreeSet::new();

    for entry in &manifest.scenes {
        let Some(doc_rel) = scene_doc_rel(&entry.path) else {
            continue;
        };

        let mut env_values = Vec::new();
        if let Some(document) = read_scene_document(asset_root, &doc_rel, guid_index) {
            collect_env_values_from_doc(&document, &mut env_values);
        } else if let Ok(relative) = safe_relative(&entry.path) {
            if let Ok(cooked) = fs::read(staging.join(&relative)) {
                let path = prism_scene::rscn::read_env_path(&cooked);
                env_values.extend(path);
            }
        }

        for value in env_values {
            let Some(pak_path) = normalize_scene_relative(&doc_rel, &value) else {
                missing.push(format!(
                    "{doc_rel}: environment asset '{value}' is absolute or escapes assets/"
                ));
                continue;
            };
            if !staged.insert(pak_path.clone()) {
                continue;
            }

            let source = asset_root.join(&pak_path);
            if !source.is_file() {
                missing.push(format!(
                    "{doc_rel}: environment asset '{pak_path}' not found"
                ));
                continue;
            }
            let bytes = fs::read(&source)
                .with_context(|| format!("read environment {}", source.display()))?;
            let cooked_bytes = if prism_gfx::ibl::CookedIbl::is_cooked(&bytes) {
                bytes
            } else {
                let t = std::time::Instant::now();
                let cooked = prism_gfx::ibl::CookedIbl::bake_from_hdr(&bytes)
                    .with_context(|| format!("bake IBL for {}", source.display()))?;
                let encoded = cooked.encode();
                log::info!(
                    "baked IBL '{}': {} KiB ({}ms)",
                    pak_path,
                    encoded.len() / 1024,
                    t.elapsed().as_millis()
                );
                encoded
            };
            write_staged(staging, Path::new(&pak_path), &cooked_bytes)?;
            log::info!("packaged environment asset: {pak_path}");
        }
    }

    Ok(staged.len())
}

/// Copy `SpriteRenderer.sprite_path` referenced PNG into package as RTEX and put into package.
///
/// Runtime looks up via `.pak` path manifest using same relative path, so staging file name
/// must match component's `sprite_path` fold result (`sprites/boat.png`).
fn stage_sprite_assets(
    asset_root: &Path,
    guid_index: &GuidIndex,
    staging: &Path,
    manifest: &prism_scene::SceneManifest,
    missing: &mut Vec<String>,
) -> Result<usize> {
    let mut staged = std::collections::BTreeSet::new();

    for entry in &manifest.scenes {
        let doc_rel = if let Some(stem) = entry.path.strip_suffix(".scene.json") {
            format!("{stem}.scene.json")
        } else if let Some(stem) = entry.path.strip_suffix(".rscn") {
            format!("{stem}.scene.json")
        } else {
            continue;
        };

        let Some(doc) = read_scene_document(asset_root, &doc_rel, guid_index) else {
            continue;
        };
        let Some(entities) = doc.get("entities").and_then(|value| value.as_array()) else {
            continue;
        };

        for entity in entities {
            let Some(components) = entity.get("components").and_then(|value| value.as_object())
            else {
                continue;
            };
            let Some(value) = components
                .get("prism_engine::scene::SpriteRenderer")
                .and_then(|payload| payload.get("sprite_path"))
                .and_then(|value| value.as_str())
            else {
                continue;
            };
            if value.is_empty() {
                continue;
            }
            let Some(pak_path) = resolve_asset_path(asset_root, &doc_rel, value) else {
                missing.push(format!(
                    "{doc_rel}: sprite_path '{value}' is absolute or escapes assets/"
                ));
                continue;
            };
            if !staged.insert(pak_path.clone()) {
                continue;
            }

            let source = asset_root.join(&pak_path);
            if !source.is_file() {
                missing.push(format!("{doc_rel}: sprite '{pak_path}' not found"));
                continue;
            }
            let cooked = cook_texture(&source)?;
            write_staged(staging, Path::new(&pak_path), &cooked)?;
            log::info!("packaged sprite asset: {pak_path}");
        }
    }

    Ok(staged.len())
}

/// Copy entire `assets/audio/` directory raw into package (#987).
///
/// Same reason as `sprites/`: sound path is runtime string (used in `play_sound`), build-time
/// cannot enumerate, so package whole directory as-is. Audio doesn't need transcoding, just copy bytes.
fn stage_audio_folder(asset_root: &Path, staging: &Path) -> Result<usize> {
    stage_folder_verbatim(asset_root, staging, "audio")
}

/// Stage custom `.spv` shaders referenced by `MeshRenderer.shader_path` or
/// `SkinnedMeshRenderer.shader_path` in scene documents.
fn stage_shader_assets(
    asset_root: &Path,
    guid_index: &GuidIndex,
    staging: &Path,
    manifest: &prism_scene::SceneManifest,
    missing: &mut Vec<String>,
) -> Result<usize> {
    let mut staged = std::collections::BTreeSet::new();

    for (doc_rel, components) in scene_component_maps(asset_root, guid_index, manifest) {
        for entity in components {
            let shader_path =
                component_string(&entity, "prism_engine::scene::MeshRenderer", "shader_path")
                    .or_else(|| {
                        component_string(
                            &entity,
                            "prism_engine::scene::SkinnedMeshRenderer",
                            "shader_path",
                        )
                    });
            let Some(value) = shader_path else {
                continue;
            };
            let Some(pak_path) = resolve_asset_path(asset_root, &doc_rel, value) else {
                missing.push(format!(
                    "{doc_rel}: shader_path '{value}' is absolute or escapes assets/"
                ));
                continue;
            };
            if !staged.insert(pak_path.clone()) {
                continue;
            }
            let source = asset_root.join(&pak_path);
            if !source.is_file() {
                missing.push(format!("{doc_rel}: shader '{pak_path}' not found"));
                continue;
            }
            let bytes = fs::read(&source).with_context(|| format!("read shader {pak_path}"))?;
            if bytes.len() < 20
                || !bytes.len().is_multiple_of(4)
                || bytes[..4] != [0x03, 0x02, 0x23, 0x07]
            {
                bail!("{doc_rel}: shader '{pak_path}' is not valid SPIR-V bytecode");
            }
            write_staged(staging, Path::new(&pak_path), &bytes)?;
            log::info!("packaged scene shader: {pak_path}");
        }
    }

    Ok(staged.len())
}

/// Stage compiled `.spv` shaders under `assets/shaders/` so runtime shader
/// switching can resolve them from `scenes.pak` via `ShaderAsset`.
fn stage_shader_folder(asset_root: &Path, staging: &Path) -> Result<usize> {
    let dir = asset_root.join("shaders");
    if !dir.is_dir() {
        return Ok(0);
    }
    let mut packed = 0usize;
    let mut stack = vec![dir];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current)
            .with_context(|| format!("read shaders dir {}", current.display()))?
        {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if !path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("spv"))
            {
                continue;
            }
            let rel = path
                .strip_prefix(asset_root)
                .with_context(|| format!("strip prefix on {}", path.display()))?;
            let dst = staging.join(rel);
            if dst.exists() {
                continue;
            }
            let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
            write_staged(staging, rel, &bytes)?;
            packed += 1;
        }
    }
    if packed > 0 {
        log::info!("packaged {packed} shader file(s) from assets/shaders/");
    }
    Ok(packed)
}

/// Copy entire `assets/<name>/` directory raw into staging (no transcoding).
fn stage_folder_verbatim(asset_root: &Path, staging: &Path, name: &str) -> Result<usize> {
    let dir = asset_root.join(name);
    if !dir.is_dir() {
        return Ok(0);
    }
    let mut packed = 0usize;
    let mut stack = vec![dir.clone()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current)
            .with_context(|| format!("read {name} dir {}", current.display()))?
        {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let relative = path
                .strip_prefix(asset_root)
                .with_context(|| format!("{name} file outside assets/: {}", path.display()))?;
            let bytes =
                fs::read(&path).with_context(|| format!("read {name} {}", path.display()))?;
            write_staged(staging, relative, &bytes)?;
            packed += 1;
        }
    }
    if packed > 0 {
        log::info!("packaged {packed} {name} file(s) from {}", dir.display());
    }
    Ok(packed)
}

/// Put `assets/data/` project data raw into package (#1049).
///
/// Level config, timesheet etc JSON has no corresponding scene component reference, build-time cannot enumerate;
/// same as `sprites/` / `audio/` packaged wholesale as "directory = content". Runtime's
/// [`prism_engine::asset_paths::read_asset_text`] first looks for loose file, then falls back to
/// `.pak`, so desktop and Android take the same path.
fn stage_data_folder(asset_root: &Path, staging: &Path) -> Result<usize> {
    stage_folder_verbatim(asset_root, staging, "data")
}

/// Put UI component-referenced textures and fonts into staging (`#591` background image / `#529` tileset / `#583` font).
///
/// Texture baked into RTEX by runtime path (same as sprite), font copied raw (fontdue reads bytes directly).
fn stage_ui_assets(
    asset_root: &Path,
    guid_index: &GuidIndex,
    staging: &Path,
    manifest: &prism_scene::SceneManifest,
    missing: &mut Vec<String>,
) -> Result<usize> {
    let mut textures: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut fonts: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for entry in &manifest.scenes {
        let Some(doc_rel) = scene_doc_rel(&entry.path) else {
            continue;
        };
        let Some(doc) = read_scene_document(asset_root, &doc_rel, guid_index) else {
            continue;
        };
        let Some(entities) = doc.get("entities").and_then(|value| value.as_array()) else {
            continue;
        };
        for entity in entities {
            let Some(components) = entity.get("components").and_then(|value| value.as_object())
            else {
                continue;
            };
            let string_field = |component: &str, field: &str| {
                components
                    .get(component)
                    .and_then(|payload| payload.get(field))
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
            };
            for (component, field) in [
                ("prism_engine::ui::Style", "image"),
                ("prism_engine::scene::Tilemap", "tileset_path"),
            ] {
                let Some(value) = string_field(component, field) else {
                    continue;
                };
                match resolve_asset_path(asset_root, &doc_rel, value) {
                    Some(pak_path)
                        if textures.insert(pak_path.clone())
                            && asset_root.join(&pak_path).is_file() =>
                    {
                        let cooked = cook_texture(&asset_root.join(&pak_path))?;
                        write_staged(staging, Path::new(&pak_path), &cooked)?;
                        log::info!("packaged UI texture: {pak_path}");
                    }
                    Some(pak_path) => {
                        if !asset_root.join(&pak_path).is_file() {
                            missing.push(format!("{doc_rel}: UI texture '{pak_path}' not found"));
                        }
                    }
                    None => missing.push(format!(
                        "{doc_rel}: UI texture '{value}' is absolute or escapes assets/"
                    )),
                }
            }
            if let Some(value) = string_field("prism_engine::ui::Text", "font") {
                match resolve_asset_path(asset_root, &doc_rel, value) {
                    Some(pak_path)
                        if fonts.insert(pak_path.clone())
                            && asset_root.join(&pak_path).is_file() =>
                    {
                        let bytes = fs::read(asset_root.join(&pak_path))
                            .with_context(|| format!("read font {pak_path}"))?;
                        write_staged(staging, Path::new(&pak_path), &bytes)?;
                        log::info!("packaged UI font: {pak_path}");
                    }
                    Some(pak_path) => {
                        if !asset_root.join(&pak_path).is_file() {
                            missing.push(format!("{doc_rel}: UI font '{pak_path}' not found"));
                        }
                    }
                    None => missing.push(format!(
                        "{doc_rel}: UI font '{value}' is absolute or escapes assets/"
                    )),
                }
            }
        }
    }

    Ok(textures.len() + fonts.len())
}

/// Copy `AudioSource.sound_path` referenced audio raw into package.
///
/// Audio doesn't need transcoding (runtime decodes byte-by-byte), so copy bytes directly like font.
/// Same as texture: path missing / absolute path / escapes `assets/` all go into `missing`, **build fails**
/// ---otherwise scene has sound but release build is silent, and logs all green.
fn stage_audio_assets(
    asset_root: &Path,
    guid_index: &GuidIndex,
    staging: &Path,
    manifest: &prism_scene::SceneManifest,
    missing: &mut Vec<String>,
) -> Result<usize> {
    let mut clips: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for entry in &manifest.scenes {
        let Some(doc_rel) = scene_doc_rel(&entry.path) else {
            continue;
        };
        let Some(doc) = read_scene_document(asset_root, &doc_rel, guid_index) else {
            continue;
        };
        let Some(entities) = doc.get("entities").and_then(|value| value.as_array()) else {
            continue;
        };
        for entity in entities {
            let Some(components) = entity.get("components").and_then(|value| value.as_object())
            else {
                continue;
            };
            let Some(value) = components
                .get("prism_engine::scene::AudioSource")
                .and_then(|payload| payload.get("sound_path"))
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            match resolve_asset_path(asset_root, &doc_rel, value) {
                Some(pak_path)
                    if clips.insert(pak_path.clone()) && asset_root.join(&pak_path).is_file() =>
                {
                    let bytes = fs::read(asset_root.join(&pak_path))
                        .with_context(|| format!("read audio clip {pak_path}"))?;
                    write_staged(staging, Path::new(&pak_path), &bytes)?;
                    log::info!("packaged audio clip: {pak_path}");
                }
                Some(pak_path) => {
                    if !asset_root.join(&pak_path).is_file() {
                        missing.push(format!("{doc_rel}: audio clip '{pak_path}' not found"));
                    }
                }
                None => missing.push(format!(
                    "{doc_rel}: audio clip '{value}' is absolute or escapes assets/"
                )),
            }
        }
    }

    Ok(clips.len())
}

/// Put `MeshRenderer.mesh_path` referenced model cooked as RMES into package (#530).
///
/// Built-in mesh (`prism_render::primitives`) doesn't need packaging---runtime generates directly by
/// generate. Other paths (`.gltf` / `.glb`) go through same importer/cooker at build time, written into
/// staging **keeping author-written relative path**, because runtime looks up this path in pak manifest.
fn stage_mesh_assets(
    asset_root: &Path,
    guid_index: &GuidIndex,
    staging: &Path,
    manifest: &prism_scene::SceneManifest,
    missing: &mut Vec<String>,
) -> Result<usize> {
    let mut staged = std::collections::BTreeSet::new();

    for (doc_rel, components) in scene_component_maps(asset_root, guid_index, manifest) {
        for entity in components {
            let Some(value) =
                component_string(&entity, "prism_engine::scene::MeshRenderer", "mesh_path")
            else {
                continue;
            };
            if value.starts_with(prism_scene::BUILTIN_MESH_PREFIX) {
                continue;
            }
            let Some(pak_path) = resolve_asset_path(asset_root, &doc_rel, value) else {
                missing.push(format!(
                    "{doc_rel}: mesh_path '{value}' is absolute or escapes assets/"
                ));
                continue;
            };
            if !staged.insert(pak_path.clone()) {
                continue;
            }
            let source = asset_root.join(&pak_path);
            if !source.is_file() {
                missing.push(format!("{doc_rel}: mesh '{pak_path}' not found"));
                continue;
            }
            let cooked = cook_mesh(&source)?;
            write_staged(staging, Path::new(&pak_path), &cooked)?;
            log::info!("packaged mesh: {pak_path}");

            // glTF/GLB comes with PBR material and texture (#520): cook together as RMAT/RTEX,
            // Path and scene auto-completed `material_paths` completely consistent.
            if is_gltf_path(&pak_path) {
                let side = prism_cooker::importer::GltfImporter::side_assets(&source, &pak_path)
                    .with_context(|| format!("import glTF side assets for {pak_path}"))?;
                for asset in side.assets {
                    write_staged(staging, Path::new(&asset.path), &asset.cooked_data)?;
                    log::info!("packaged glTF side asset: {}", asset.path);
                }
            }
        }
    }

    Ok(staged.len())
}

/// Put `MeshRenderer.material_path` material and its 5 textures into package (#530).
///
/// Material (`.mat.json` / `.mat`) stores material **`AssetId`**, so texture ID must
/// Consistent with packer allocation: `AssetId::from_path` derived by package-relative path, material cook
/// time writes same ID into `RMAT`. This is why [`pack_directory`] changed to path-derived ID;
/// reason---sequential allocation shifts all baked IDs when next file appears.
fn stage_material_assets(
    asset_root: &Path,
    guid_index: &GuidIndex,
    staging: &Path,
    manifest: &prism_scene::SceneManifest,
    missing: &mut Vec<String>,
) -> Result<usize> {
    use prism_asset::core::id::AssetId;
    use prism_asset::core::AssetType;
    use prism_cooker::cooker::profile::CookSettings;
    use prism_cooker::cooker::MaterialCooker;
    use prism_cooker::db::AssetDatabase;
    use prism_cooker::importer::MaterialImporter;
    use prism_cooker::importer::{ImportContext, Importer as _};
    use prism_cooker::Cooker as _;

    let mut staged = std::collections::BTreeSet::new();

    for (doc_rel, components) in scene_component_maps(asset_root, guid_index, manifest) {
        for entity in components {
            let mut material_paths: Vec<String> = Vec::new();
            if let Some(path) = component_string(
                &entity,
                "prism_engine::scene::MeshRenderer",
                "material_path",
            ) {
                material_paths.push(path.to_string());
            }
            material_paths.extend(component_string_array(
                &entity,
                "prism_engine::scene::MeshRenderer",
                "material_paths",
            ));
            material_paths.sort();
            material_paths.dedup();

            for value in material_paths {
                let Some(pak_path) = resolve_asset_path(asset_root, &doc_rel, &value) else {
                    missing.push(format!(
                        "{doc_rel}: material_path '{value}' is absolute or escapes assets/"
                    ));
                    continue;
                };
                if !staged.insert(pak_path.clone()) {
                    continue;
                }
                let source = asset_root.join(&pak_path);
                if !source.is_file() {
                    missing.push(format!("{doc_rel}: material '{pak_path}' not found"));
                    continue;
                }

                // First register image path and ID into database, importer can parse dependencies by slot order.
                let text = fs::read_to_string(&source)
                    .with_context(|| format!("read material {pak_path}"))?;
                let mut db = AssetDatabase::new();
                let mut texture_targets: Vec<String> = Vec::new();
                for raw in material_texture_paths(&text) {
                    let Some(texture_pak) = resolve_asset_path(asset_root, &doc_rel, &raw) else {
                        missing.push(format!(
                            "{doc_rel}: material texture '{raw}' is absolute or escapes assets/"
                        ));
                        continue;
                    };
                    let texture_source = asset_root.join(&texture_pak);
                    if !texture_source.is_file() {
                        missing.push(format!(
                            "{doc_rel}: material texture '{texture_pak}' not found"
                        ));
                        continue;
                    }
                    // Data DB key must exactly match material importer query string.
                    let mut record = prism_cooker::AssetRecord::new(
                        AssetId::from_path(&texture_pak),
                        raw.replace('\\', "/"),
                        AssetType::Texture,
                        "texture-importer",
                    );
                    record.source_hash = texture_hash(&texture_source)?;
                    let _ = db.insert(record);
                    texture_targets.push(texture_pak);
                }
                let db = std::sync::Arc::new(db);

                let imported = MaterialImporter
                    .import(&ImportContext {
                        source_path: source.clone(),
                        source_hash: texture_hash(&source)?,
                        settings: serde_json::json!({}),
                        db: db.clone(),
                    })
                    .with_context(|| format!("import material {pak_path}"))?;

                let mut record = prism_cooker::AssetRecord::new(
                    AssetId::from_path(&pak_path),
                    pak_path.clone(),
                    AssetType::Material,
                    "material-importer",
                );
                record.dependencies = imported.dependencies.clone();

                let cooked = MaterialCooker
                    .cook(&prism_cooker::CookContext {
                        record: &record,
                        imported_data: &imported.output_data,
                        settings: &CookSettings::default(),
                    })
                    .with_context(|| format!("cook material {pak_path}"))?;
                write_staged(staging, Path::new(&pak_path), &cooked.cooked_data)?;
                log::info!("packaged material: {pak_path}");

                for texture_pak in texture_targets {
                    let cooked = cook_texture(&asset_root.join(&texture_pak))?;
                    write_staged(staging, Path::new(&texture_pak), &cooked)?;
                    log::info!("packaged material texture: {texture_pak}");
                }
            }
        }
    }

    Ok(staged.len())
}

/// Traverse component table of all scene documents in manifest (`*.scene.json` only has author data).
fn scene_component_maps(
    asset_root: &Path,
    guid_index: &GuidIndex,
    manifest: &prism_scene::SceneManifest,
) -> Vec<(String, Vec<serde_json::Map<String, serde_json::Value>>)> {
    let mut out = Vec::new();
    for entry in &manifest.scenes {
        if !entry.enabled {
            continue;
        }
        let Some(doc_rel) = scene_doc_rel(&entry.path) else {
            continue;
        };
        let Some(doc) = read_scene_document(asset_root, &doc_rel, guid_index) else {
            continue;
        };
        let components = doc
            .get("entities")
            .and_then(|value| value.as_array())
            .map(|entities| {
                entities
                    .iter()
                    .filter_map(|entity| {
                        entity
                            .get("components")
                            .and_then(|value| value.as_object())
                            .cloned()
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push((doc_rel, components));
    }
    out
}

fn component_string<'a>(
    components: &'a serde_json::Map<String, serde_json::Value>,
    component: &str,
    field: &str,
) -> Option<&'a str> {
    components
        .get(component)
        .and_then(|payload| payload.get(field))
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
}

/// Read string array field (`MeshRenderer.material_paths`).
fn component_string_array(
    components: &serde_json::Map<String, serde_json::Value>,
    component: &str,
    field: &str,
) -> Vec<String> {
    components
        .get(component)
        .and_then(|payload| payload.get(field))
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Material JSON 5 texture slots (consistent with `MATERIAL_TEX_SLOTS` order).
fn material_texture_paths(text: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    [
        "albedo_tex",
        "normal_tex",
        "metallic_roughness_tex",
        "emissive_tex",
        "occlusion_tex",
    ]
    .iter()
    .filter_map(|key| value.get(*key).and_then(|slot| slot.as_str()))
    .filter(|path| !path.is_empty())
    .map(str::to_string)
    .collect()
}

fn texture_hash(path: &Path) -> Result<u64> {
    use std::hash::{Hash, Hasher};
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    Ok(hasher.finish())
}

/// Use `GltfImporter` + `MeshCooker` to cook glTF/GLB into RMES bytes.
fn cook_mesh(source: &Path) -> Result<Vec<u8>> {
    use prism_asset::core::{AssetId, AssetType};
    use prism_cooker::cooker::profile::CookSettings;
    use prism_cooker::cooker::MeshCooker;
    use prism_cooker::importer::{GltfImporter, ImportContext, Importer as _};
    use prism_cooker::Cooker as _;

    let imported = GltfImporter
        .import(&ImportContext {
            source_path: source.to_path_buf(),
            source_hash: texture_hash(source)?,
            settings: serde_json::json!({}),
            db: std::sync::Arc::new(prism_cooker::AssetDatabase::new()),
        })
        .with_context(|| format!("import mesh {}", source.display()))?;

    let record = prism_cooker::AssetRecord::new(
        AssetId::generate(),
        source.to_string_lossy().into_owned(),
        AssetType::Mesh,
        "mesh-importer",
    );
    let cooked = MeshCooker
        .cook(&prism_cooker::CookContext {
            record: &record,
            imported_data: &imported.output_data,
            settings: &CookSettings::default(),
        })
        .with_context(|| format!("cook mesh {}", source.display()))?;
    Ok(cooked.cooked_data)
}

/// Reference missing / unpackable resource causes build failure (#530).
///
/// Package product missing one texture or model, runtime just shows "object missing", with no
/// Report wrong — must be caught at build time.
fn report_missing_references(mut missing: Vec<String>) -> Result<()> {
    if missing.is_empty() {
        return Ok(());
    }
    missing.sort();
    missing.dedup();
    let shown: Vec<String> = missing
        .iter()
        .take(20)
        .map(|entry| format!("  - {entry}"))
        .collect();
    let more = missing.len().saturating_sub(shown.len());
    let suffix = if more > 0 {
        format!("\n  … and {more} more")
    } else {
        String::new()
    };
    bail!(
        "{} asset reference(s) cannot be packaged; packaged builds would silently \
         miss them (fix the paths or remove the entities):\n{}{suffix}",
        missing.len(),
        shown.join("\n")
    )
}

/// Use cooker's `TextureImporter` + `TextureCooker` to cook source graph into RTEX bytes.
fn cook_texture(source: &Path) -> Result<Vec<u8>> {
    use std::hash::{Hash, Hasher};

    use prism_asset::core::AssetType;
    use prism_cooker::cooker::profile::CookSettings;
    use prism_cooker::cooker::TextureCooker;
    use prism_cooker::importer::{ImportContext, Importer as _, TextureImporter};
    use prism_cooker::Cooker as _;

    let bytes = fs::read(source).with_context(|| format!("read sprite {}", source.display()))?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let source_hash = hasher.finish();

    let imported = TextureImporter
        .import(&ImportContext {
            source_path: source.to_path_buf(),
            source_hash,
            settings: serde_json::json!({}),
            db: std::sync::Arc::new(prism_cooker::AssetDatabase::new()),
        })
        .with_context(|| format!("import sprite {}", source.display()))?;

    let record = prism_cooker::AssetRecord::new(
        prism_asset::core::AssetId::generate(),
        source.to_string_lossy().into_owned(),
        AssetType::Texture,
        "texture-importer",
    );
    // Per-asset import settings (#514): `<asset>.import.json` overrides profile max_size / mips.
    let mut settings = CookSettings::default();
    prism_cooker::ImportSettings::load(source).apply(&mut settings);
    let cooked = TextureCooker
        .cook(&prism_cooker::CookContext {
            record: &record,
            imported_data: &imported.output_data,
            settings: &settings,
        })
        .with_context(|| format!("cook sprite {}", source.display()))?;
    Ok(cooked.cooked_data)
}

/// Cook all graph slices under `assets/sprites/` into RTEX and write into staging.
///
/// Sprites spawned at runtime (layered background, fish, splash) do not go through scene JSON, therefore cannot rely only on
/// [`stage_sprite_assets`] reference scanning; entire directory goes into package, path kept consistent.
fn stage_sprite_folder(asset_root: &Path, staging: &Path) -> Result<usize> {
    let root = asset_root.join("sprites");
    if !root.is_dir() {
        return Ok(0);
    }

    let mut staged = 0usize;
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in
            fs::read_dir(&dir).with_context(|| format!("read sprite dir {}", dir.display()))?
        {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "tga" | "bmp") {
                continue;
            }
            let rel = path
                .strip_prefix(asset_root)
                .with_context(|| format!("sprite {} escapes assets/", path.display()))?;
            let cooked = cook_texture(&path)?;
            write_staged(staging, rel, &cooked)?;
            staged += 1;
        }
    }
    log::info!("packaged {staged} sprite file(s) from {}", root.display());
    Ok(staged)
}

/// Scene document path corresponding to scene (`*.rscn` entry maps back to same-named `.scene.json`).
fn scene_doc_rel(entry_path: &str) -> Option<String> {
    if let Some(stem) = entry_path.strip_suffix(".scene.json") {
        Some(format!("{stem}.scene.json"))
    } else {
        entry_path
            .strip_suffix(".rscn")
            .map(|stem| format!("{stem}.scene.json"))
    }
}

/// Collect environment texture paths declared by each entity in author document.
fn collect_env_values_from_doc(doc: &serde_json::Value, out: &mut Vec<String>) {
    let Some(entities) = doc.get("entities").and_then(|value| value.as_array()) else {
        return;
    };

    for entity in entities {
        let Some(components) = entity.get("components").and_then(|value| value.as_object()) else {
            continue;
        };
        for (component, field) in [
            ("prism_engine::scene::EnvironmentLighting", "env_map"),
            ("prism_engine::scene::Skybox", "hdr_path"),
        ] {
            if let Some(value) = components
                .get(component)
                .and_then(|payload| payload.get(field))
                .and_then(|value| value.as_str())
            {
                out.push(value.to_string());
            }
        }
    }
}

/// Fold `scene_rel` corresponding scene-relative path into `assets/` internal path.
///
/// Both forms accepted, tried in order:
/// 1. **Scene-relative** (`../sprites/boat.png`)---consistent with `Skybox.hdr_path` legacy;
/// 2. **Package-relative** (`sprites/boat.png` or `assets/sprites/boat.png`).
///
/// When scene-relative candidate doesn't exist on disk, fall back to package-relative path, so component can
/// directly write package-internal path (runtime `id_by_path` uses same string).
fn normalize_scene_relative(scene_rel: &str, value: &str) -> Option<String> {
    let value = value.replace('\\', "/");
    if value.is_empty() || Path::new(&value).is_absolute() {
        return None;
    }

    let mut parts: Vec<String> = Vec::new();
    let project_style = value.starts_with("assets/");
    if !project_style {
        if let Some((dir, _)) = scene_rel.rsplit_once('/') {
            parts.extend(
                dir.split('/')
                    .filter(|part| !part.is_empty())
                    .map(str::to_string),
            );
        }
    }

    let relative = value.strip_prefix("assets/").unwrap_or(&value);
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other.to_string()),
        }
    }

    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

/// Same as [`normalize_scene_relative`], but when scene relative candidate in `asset_root`
/// below not present, return package-relative path.
fn resolve_asset_path(asset_root: &Path, scene_rel: &str, value: &str) -> Option<String> {
    let scene_relative = normalize_scene_relative(scene_rel, value)?;
    if asset_root.join(&scene_relative).is_file() {
        return Some(scene_relative);
    }
    let normalized = value.replace('\\', "/");
    let root_relative = normalized
        .strip_prefix("assets/")
        .unwrap_or(&normalized)
        .to_string();
    if !root_relative.is_empty() && asset_root.join(&root_relative).is_file() {
        return Some(root_relative);
    }
    // Both candidates not present: return scene-relative path, letting caller print original target.
    Some(scene_relative)
}

/// Cook an authoring scene, first resolving stable GUID references (#466),
/// then auto-filling `MeshRenderer.material_paths` for glTF/GLB meshes from
/// their generated side assets (#520).
fn cook_scene_file(
    cooker: &SceneCooker,
    asset_root: &Path,
    guid_index: &GuidIndex,
    path: &Path,
    scene_rel: &str,
) -> Result<Vec<u8>> {
    let data = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let doc_rel = doc_relative_to(asset_root, path);
    let data = match serde_json::from_slice::<serde_json::Value>(&data) {
        Ok(mut document) => {
            resolve_document_references(&mut document, &doc_rel, guid_index);
            serde_json::to_vec(&document).unwrap_or(data)
        }
        // Let `SceneCooker` produce the canonical parse error.
        Err(_) => data,
    };
    let rewritten = rewrite_gltf_material_paths(&data, asset_root, scene_rel)?;
    let rewritten = rewrite_skinned_components(&rewritten, asset_root, scene_rel)?;
    cooker
        .cook_json_bytes(&rewritten)
        .with_context(|| format!("cook {}", path.display()))
}

/// Apply GUID → path resolution to a parsed authoring document.
fn resolve_document_references(
    document: &mut serde_json::Value,
    doc_rel: &str,
    guid_index: &GuidIndex,
) {
    sync_document(
        document,
        doc_rel,
        SyncMode::Load,
        |guid| guid_index.path_for_guid(guid).map(str::to_string),
        |path| guid_index.guid_for_path(path).map(str::to_string),
    );
}

/// Read a scene document and resolve its GUID references before staging.
fn read_scene_document(
    asset_root: &Path,
    doc_rel: &str,
    guid_index: &GuidIndex,
) -> Option<serde_json::Value> {
    let text = fs::read_to_string(asset_root.join(doc_rel)).ok()?;
    let mut document = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    resolve_document_references(&mut document, doc_rel, guid_index);
    Some(document)
}

fn doc_relative_to(asset_root: &Path, path: &Path) -> String {
    path.strip_prefix(asset_root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| {
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string()
        })
}

/// Resolve `scenes.json` GUIDs before staging/cooking.
fn resolve_manifest_guids(manifest: &mut prism_scene::SceneManifest, index: &GuidIndex) {
    for entry in &mut manifest.scenes {
        let Some(guid) = entry.guid.as_deref() else {
            continue;
        };
        if let Some(resolved) = index.path_for_guid(guid) {
            entry.path = resolved.to_string();
        }
    }
}

/// Fill empty `MeshRenderer.material_paths` on entities that render a
/// glTF/GLB mesh. The paths mirror what
/// [`prism_cooker::importer::GltfImporter::side_assets`] stages, so the cooked
/// RSCN references the generated RMAT assets by their final pak paths.
///
/// Explicit author values win: only entities with neither `material_path` nor
/// `material_paths` are touched. A missing mesh file is left for
/// [`stage_mesh_assets`] to report with the usual `missing` diagnostics.
fn rewrite_gltf_material_paths(data: &[u8], asset_root: &Path, scene_rel: &str) -> Result<Vec<u8>> {
    let Ok(mut document) = serde_json::from_slice::<serde_json::Value>(data) else {
        // Let `SceneCooker` produce the canonical parse error.
        return Ok(data.to_vec());
    };
    let Some(entities) = document
        .get_mut("entities")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return Ok(data.to_vec());
    };

    let mut changed = false;
    for entity in entities {
        let Some(components) = entity
            .get_mut("components")
            .and_then(serde_json::Value::as_object_mut)
        else {
            continue;
        };
        let Some(renderer) = components
            .get_mut("prism_engine::scene::MeshRenderer")
            .and_then(serde_json::Value::as_object_mut)
        else {
            continue;
        };
        if renderer
            .get("material_path")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|path| !path.is_empty())
        {
            continue;
        }
        if renderer
            .get("material_paths")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|paths| !paths.is_empty())
        {
            continue;
        }
        let Some(mesh_path) = renderer
            .get("mesh_path")
            .and_then(serde_json::Value::as_str)
            .filter(|path| !path.is_empty())
        else {
            continue;
        };
        if !is_gltf_path(mesh_path) {
            continue;
        }
        let Some(pak_path) = resolve_asset_path(asset_root, scene_rel, mesh_path) else {
            continue;
        };
        let mesh_source = asset_root.join(&pak_path);
        if !mesh_source.is_file() {
            continue;
        }
        let material_paths =
            prism_cooker::importer::GltfImporter::submesh_material_paths(&mesh_source, &pak_path)
                .with_context(|| format!("collect glTF materials for {pak_path}"))?;
        if material_paths.is_empty() {
            continue;
        }
        renderer.insert(
            "material_paths".to_string(),
            serde_json::Value::Array(
                material_paths
                    .into_iter()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
        changed = true;
    }

    if changed {
        serde_json::to_vec(&document).context("serialize scene with generated glTF material paths")
    } else {
        Ok(data.to_vec())
    }
}

/// For entity referencing skinned glTF, automatically attach `SkinnedMeshRenderer` /
/// `SkeletonRoot` / `Animator` (#560 P0-4).
///
/// Before, packer only wrote `MeshRenderer`; after importing a skinned character, scene renders as
/// bind-pose static mesh — to make character move must manually edit `.scene.json`. This inside by
/// [`prism_cooker::importer::GltfImporter::rig_paths`] generated path to fill component:
///
/// - `SkinnedMeshRenderer.skinned_mesh_path` = `{mesh}.skinning/mesh.rskin`
///   (material follows `MeshRenderer` author-written / auto-filled path from one round);
/// - `SkeletonRoot.skeleton_path` = `{mesh}.skinning/skeleton.rskel`,
///   `default_clip_path` = first clip;
/// - `Animator.clip_path` = first clip (default play, loop).
///
/// **Author-written component takes priority**: if any already exists, skip entire block. `MeshRenderer` kept as-is —
/// Packer still relies on its `mesh_path` to stage glTF and side assets; at runtime then will because
/// entity has `SkinnedMeshRenderer`, skipping static upload (see `asset_resolver`).
fn rewrite_skinned_components(data: &[u8], asset_root: &Path, scene_rel: &str) -> Result<Vec<u8>> {
    let Ok(mut document) = serde_json::from_slice::<serde_json::Value>(data) else {
        // Let `SceneCooker` produce the canonical parse error.
        return Ok(data.to_vec());
    };
    let Some(entities) = document
        .get_mut("entities")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return Ok(data.to_vec());
    };

    let mut changed = false;
    for entity in entities {
        let Some(components) = entity
            .get_mut("components")
            .and_then(serde_json::Value::as_object_mut)
        else {
            continue;
        };
        if components.contains_key("prism_engine::scene::SkinnedMeshRenderer")
            || components.contains_key("prism_engine::scene::SkeletonRoot")
        {
            continue;
        }
        let Some(mesh_path) = components
            .get("prism_engine::scene::MeshRenderer")
            .and_then(|renderer| renderer.get("mesh_path"))
            .and_then(serde_json::Value::as_str)
            .filter(|path| !path.is_empty())
        else {
            continue;
        };
        if !is_gltf_path(mesh_path) {
            continue;
        }
        let Some(pak_path) = resolve_asset_path(asset_root, scene_rel, mesh_path) else {
            continue;
        };
        let source = asset_root.join(&pak_path);
        if !source.is_file() {
            continue;
        }
        let rig = match prism_cooker::importer::GltfImporter::rig_paths(&source, &pak_path) {
            Ok(rig) => rig,
            Err(error) => {
                log::warn!("{scene_rel}: cannot read rig of '{pak_path}': {error}");
                continue;
            }
        };
        // Only skeleton no weight (or incorrect incoming) means no skinnable mesh to draw, keep static.
        if insert_rig_components(components, &rig) {
            changed = true;
            log::info!(
                "{scene_rel}: rigged '{pak_path}' ({} clip(s)) — added skinned components",
                rig.clips.len()
            );
        }
    }

    if changed {
        serde_json::to_vec(&document).context("serialize scene with generated skinned components")
    } else {
        Ok(data.to_vec())
    }
}

/// Write the three components described by `rig` into entity; already existing component not overwritten.
///
/// Return whether anything was written. Split out to enable unit testing (no need for real glTF file).
fn insert_rig_components(
    components: &mut serde_json::Map<String, serde_json::Value>,
    rig: &prism_cooker::importer::GltfRigPaths,
) -> bool {
    let (Some(skeleton), Some(skinned_mesh)) = (rig.skeleton.clone(), rig.skinned_mesh.clone())
    else {
        return false;
    };
    if components.contains_key("prism_engine::scene::SkinnedMeshRenderer")
        || components.contains_key("prism_engine::scene::SkeletonRoot")
    {
        return false;
    }

    {
        // Material follows MeshRenderer (author-written first, then auto-filled glTF material from one round).
        let renderer = components
            .get("prism_engine::scene::MeshRenderer")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let material_path = renderer
            .get("material_path")
            .cloned()
            .unwrap_or_else(|| serde_json::json!(""));
        let material_paths = renderer
            .get("material_paths")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]));
        let shadow_mode = renderer
            .get("shadow_casting_mode")
            .cloned()
            .unwrap_or_else(|| serde_json::json!("On"));

        let default_clip = rig.clips.first().cloned().unwrap_or_default();
        components.insert(
            "prism_engine::scene::SkinnedMeshRenderer".to_string(),
            serde_json::json!({
                "skinned_mesh_path": skinned_mesh,
                "material_path": material_path,
                "material_paths": material_paths,
                "shadow_casting_mode": shadow_mode,
            }),
        );
        components.insert(
            "prism_engine::scene::SkeletonRoot".to_string(),
            serde_json::json!({
                "skeleton_path": skeleton,
                "default_clip_path": default_clip,
            }),
        );
        if !rig.clips.is_empty() {
            components.insert(
                "prism_engine::scene::Animator".to_string(),
                serde_json::json!({
                    "clip_path": rig.clips[0],
                    "time": 0.0,
                    "speed": 1.0,
                    "looped": true,
                    "playing": true,
                }),
            );
        }
    }
    true
}

fn is_gltf_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("gltf") || extension.eq_ignore_ascii_case("glb")
        })
}

fn validate_rscn(entry_path: &str, data: &[u8]) -> Result<()> {
    if parse_rscn_header(data).is_none() {
        bail!("scene entry '{entry_path}' did not produce a valid RSCN v4 blob");
    }
    Ok(())
}

fn write_staged(staging: &Path, relative: &Path, data: &[u8]) -> Result<()> {
    let target = staging.join(relative);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create directory {}", parent.display()))?;
    }
    fs::write(&target, data).with_context(|| format!("write {}", target.display()))
}

fn staging_marker(staging: &Path) -> Result<PathBuf> {
    let name = staging
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("scene staging has no file name: {}", staging.display()))?;
    let mut marker_name = OsString::from(".");
    marker_name.push(name);
    marker_name.push(".prism-scene-staging");
    Ok(staging.with_file_name(marker_name))
}

fn safe_relative(path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        bail!("scene entry path must be relative and stay inside the asset root: {path:?}");
    }
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_asset::runtime::ResourceManager;

    fn rig() -> prism_cooker::importer::GltfRigPaths {
        prism_cooker::importer::GltfRigPaths {
            skeleton: Some("meshes/hero.glb.skinning/skeleton.rskel".into()),
            skinned_mesh: Some("meshes/hero.glb.skinning/mesh.rskin".into()),
            clips: vec!["meshes/hero.glb.skinning/clips/0_walk.ranim".into()],
        }
    }

    fn components(json: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        json.as_object().cloned().expect("component map")
    }

    #[test]
    fn a_rigged_mesh_gets_skinned_components() {
        let mut map = components(serde_json::json!({
            "prism_engine::scene::MeshRenderer": {
                "mesh_path": "meshes/hero.glb",
                "material_path": "meshes/hero.glb.materials/0_skin.mat.json",
                "material_paths": ["meshes/hero.glb.materials/0_skin.mat.json"],
                "shadow_casting_mode": "TwoSided",
            }
        }));

        assert!(insert_rig_components(&mut map, &rig()));

        let skinned = &map["prism_engine::scene::SkinnedMeshRenderer"];
        assert_eq!(
            skinned["skinned_mesh_path"],
            "meshes/hero.glb.skinning/mesh.rskin"
        );
        // Material and shadow mode follow MeshRenderer (author-written one).
        assert_eq!(
            skinned["material_path"],
            "meshes/hero.glb.materials/0_skin.mat.json"
        );
        assert_eq!(skinned["shadow_casting_mode"], "TwoSided");

        let root = &map["prism_engine::scene::SkeletonRoot"];
        assert_eq!(
            root["skeleton_path"],
            "meshes/hero.glb.skinning/skeleton.rskel"
        );
        assert_eq!(
            root["default_clip_path"],
            "meshes/hero.glb.skinning/clips/0_walk.ranim"
        );
        let animator = &map["prism_engine::scene::Animator"];
        assert_eq!(
            animator["clip_path"],
            "meshes/hero.glb.skinning/clips/0_walk.ranim"
        );
        assert_eq!(animator["playing"], true);
        assert_eq!(animator["looped"], true);
        assert_eq!(animator["speed"], 1.0);
        // MeshRenderer must stay: packer still relies on mesh_path to stage glTF.
        assert_eq!(
            map["prism_engine::scene::MeshRenderer"]["mesh_path"],
            "meshes/hero.glb"
        );
    }

    #[test]
    fn author_components_win_over_generated_ones() {
        let mut map = components(serde_json::json!({
            "prism_engine::scene::MeshRenderer": { "mesh_path": "meshes/hero.glb" },
            "prism_engine::scene::SkinnedMeshRenderer": {
                "skinned_mesh_path": "custom/mine.rskin"
            }
        }));
        assert!(!insert_rig_components(&mut map, &rig()));
        assert_eq!(
            map["prism_engine::scene::SkinnedMeshRenderer"]["skinned_mesh_path"],
            "custom/mine.rskin"
        );
        assert!(!map.contains_key("prism_engine::scene::SkeletonRoot"));
    }

    #[test]
    fn a_skeleton_without_weights_stays_static() {
        let mut map = components(serde_json::json!({
            "prism_engine::scene::MeshRenderer": { "mesh_path": "meshes/hero.glb" }
        }));
        let rig = prism_cooker::importer::GltfRigPaths {
            skeleton: Some("meshes/hero.glb.skinning/skeleton.rskel".into()),
            skinned_mesh: None,
            clips: vec![],
        };
        assert!(!insert_rig_components(&mut map, &rig));
        assert!(!map.contains_key("prism_engine::scene::SkinnedMeshRenderer"));
    }

    fn write_minimal_scene(path: &Path) {
        let json = br#"{
            "version": 1,
            "entities": [{
                "name": "Cam",
                "parent": null,
                "transform": {},
                "components": {
                    "prism_engine::scene::Camera": { "fov_y_degrees": 60.0 }
                }
            }]
        }"#;
        fs::write(path, json).unwrap();
    }

    /// Write JSON text into `assets/scenes.json`.
    fn write_manifest(assets: &Path, json: &str) {
        fs::write(assets.join("scenes.json"), json).unwrap();
    }

    /// 1×1 transparent PNG: enough for cooker to decode one RTEX, no image dependency needed.
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    /// #591 / #583 / #529: UI textures, tileset and fonts all must go into package.
    #[test]
    fn ui_images_tilesets_and_fonts_are_packaged() {
        let temp = tempfile::tempdir().unwrap();
        let assets = temp.path().join("assets");
        fs::create_dir_all(assets.join("scenes")).unwrap();
        fs::create_dir_all(assets.join("textures")).unwrap();
        fs::create_dir_all(assets.join("fonts")).unwrap();
        fs::write(assets.join("textures/icon.png"), TINY_PNG).unwrap();
        fs::write(assets.join("textures/tiles.png"), TINY_PNG).unwrap();
        fs::write(assets.join("fonts/hud.ttf"), b"not-a-real-ttf").unwrap();
        fs::write(
            assets.join("scenes/hud.scene.json"),
            br#"{
                "version": 1,
                "entities": [{
                    "name": "HUD",
                    "parent": null,
                    "transform": {},
                    "components": {
                        "prism_engine::ui::Node": {},
                        "prism_engine::ui::Style": { "image": "textures/icon.png" },
                        "prism_engine::ui::Text": {
                            "content": "HUD", "font": "fonts/hud.ttf"
                        },
                        "prism_engine::scene::Tilemap": {
                            "tileset_path": "textures/tiles.png"
                        }
                    }
                }]
            }"#,
        )
        .unwrap();
        write_manifest(
            &assets,
            r#"{"scenes":[{"name":"hud","path":"scenes/hud.scene.json","enabled":true}]}"#,
        );

        let output = temp.path().join("scenes.pak");
        let staging = temp.path().join("cooked");
        let summary =
            build_scene_package(assets.join("scenes.json"), &staging, &output, 0).unwrap();

        let meta_text = fs::read_to_string(summary.pack.manifest_path.clone()).unwrap();
        let meta: serde_json::Value = serde_json::from_str(&meta_text).unwrap();
        let paths: Vec<_> = meta["assets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|asset| asset["path"].as_str().unwrap().to_string())
            .collect();
        for expected in [
            "textures/icon.png",
            "textures/tiles.png",
            "fonts/hud.ttf",
            "scenes/hud.rscn",
        ] {
            assert!(
                paths.iter().any(|path| path == expected),
                "{expected} missing from pak paths: {paths:?}"
            );
        }

        // Fonts copied as-is into package (fontdue reads bytes directly), textures cooked into RTEX.
        let mut rm = ResourceManager::new();
        rm.load_package_bytes(&fs::read(&output).unwrap()).unwrap();
        rm.load_path_manifest_from_str(&meta_text).unwrap();
        let font_id = rm.id_by_path("fonts/hud.ttf").expect("font path");
        assert_eq!(rm.load_with_deps_raw(font_id).unwrap(), b"not-a-real-ttf");
        let texture_id = rm.id_by_path("textures/icon.png").expect("texture path");
        let cooked = rm.load_with_deps_raw(texture_id).unwrap();
        assert_ne!(cooked, TINY_PNG, "Texture must go through cooker");
    }

    /// #990: `AudioSource.sound_path` must go into package (raw bytes), wrong path must cause build failure.
    #[test]
    fn audio_clip_references_are_packaged_and_missing_ones_fail() {
        let temp = tempfile::tempdir().unwrap();
        let assets = temp.path().join("assets");
        fs::create_dir_all(assets.join("scenes")).unwrap();
        fs::create_dir_all(assets.join("audio")).unwrap();
        fs::write(assets.join("audio/splash.ogg"), b"not-a-real-ogg").unwrap();
        fs::write(
            assets.join("scenes/lake.scene.json"),
            br#"{
                "version": 1,
                "entities": [{
                    "name": "Splash",
                    "parent": null,
                    "transform": {},
                    "components": {
                        "prism_engine::scene::AudioSource": {
                            "sound_path": "audio/splash.ogg"
                        }
                    }
                }]
            }"#,
        )
        .unwrap();
        write_manifest(
            &assets,
            r#"{"scenes":[{"name":"lake","path":"scenes/lake.scene.json","enabled":true}]}"#,
        );

        let output = temp.path().join("scenes.pak");
        let staging = temp.path().join("cooked");
        let summary =
            build_scene_package(assets.join("scenes.json"), &staging, &output, 0).unwrap();

        let meta_text = fs::read_to_string(summary.pack.manifest_path.clone()).unwrap();
        let mut rm = ResourceManager::new();
        rm.load_package_bytes(&fs::read(&output).unwrap()).unwrap();
        rm.load_path_manifest_from_str(&meta_text).unwrap();
        let clip_id = rm.id_by_path("audio/splash.ogg").expect("audio path");
        assert_eq!(rm.load_with_deps_raw(clip_id).unwrap(), b"not-a-real-ogg");

        // When referencing non-existent audio, build must fail, not silently output a soundless package.
        fs::write(
            assets.join("scenes/lake.scene.json"),
            br#"{
                "version": 1,
                "entities": [{
                    "name": "Splash",
                    "parent": null,
                    "transform": {},
                    "components": {
                        "prism_engine::scene::AudioSource": {
                            "sound_path": "audio/missing.ogg"
                        }
                    }
                }]
            }"#,
        )
        .unwrap();
        let error = build_scene_package(
            assets.join("scenes.json"),
            temp.path().join("cooked2"),
            temp.path().join("scenes2.pak"),
            0,
        )
        .expect_err("missing audio clip must fail the build");
        let text = format!("{error:#}");
        assert!(
            text.contains("audio/missing.ogg"),
            "error should name the missing clip: {text}"
        );
    }

    /// #1049: Project data inside `assets/data/` copied as-is into package, readable by key at runtime.
    #[test]
    fn project_data_folder_is_packaged_verbatim() {
        let temp = tempfile::tempdir().unwrap();
        let assets = temp.path().join("assets");
        fs::create_dir_all(assets.join("scenes")).unwrap();
        fs::create_dir_all(assets.join("data/nested")).unwrap();
        let layers = br#"{"layers":[{"slug":"sky","depth":0.9}]}"#;
        fs::write(assets.join("data/lake_layers.json"), layers).unwrap();
        fs::write(assets.join("data/nested/notes.txt"), b"hello").unwrap();
        write_minimal_scene(&assets.join("scenes/lake.scene.json"));
        write_manifest(
            &assets,
            r#"{"scenes":[{"name":"lake","path":"scenes/lake.scene.json","enabled":true}]}"#,
        );

        let output = temp.path().join("scenes.pak");
        let staging = temp.path().join("cooked");
        let summary =
            build_scene_package(assets.join("scenes.json"), &staging, &output, 0).unwrap();

        let meta_text = fs::read_to_string(summary.pack.manifest_path.clone()).unwrap();
        let mut rm = ResourceManager::new();
        rm.load_package_bytes(&fs::read(&output).unwrap()).unwrap();
        rm.load_path_manifest_from_str(&meta_text).unwrap();
        let layers_id = rm.id_by_path("data/lake_layers.json").expect("data path");
        assert_eq!(rm.load_with_deps_raw(layers_id).unwrap(), layers);
        let nested_id = rm.id_by_path("data/nested/notes.txt").expect("nested path");
        assert_eq!(rm.load_with_deps_raw(nested_id).unwrap(), b"hello");
    }

    /// #509: `build-scenes --optional-dir` splits staged subdirectory into independent package,
    /// Base package manifest lists it, asset ID still derived from package-relative path.
    #[test]
    fn optional_dirs_are_split_out_of_the_scene_package() {
        let temp = tempfile::tempdir().unwrap();
        let assets = temp.path().join("assets");
        fs::create_dir_all(assets.join("scenes")).unwrap();
        fs::create_dir_all(assets.join("textures")).unwrap();
        fs::write(assets.join("textures/icon.png"), TINY_PNG).unwrap();
        fs::write(
            assets.join("scenes/optional.scene.json"),
            br#"{
                "version": 1,
                "entities": [{
                    "name": "HUD",
                    "parent": null,
                    "transform": {},
                    "components": {
                        "prism_engine::ui::Node": {},
                        "prism_engine::ui::Style": { "image": "textures/icon.png" }
                    }
                }]
            }"#,
        )
        .unwrap();
        write_manifest(
            &assets,
            r#"{"scenes":[{"name":"optional","path":"scenes/optional.scene.json","enabled":true}]}"#,
        );

        let output = temp.path().join("game.pak");
        let staging = temp.path().join("cooked");
        let summary = build_scene_package_split(
            assets.join("scenes.json"),
            &staging,
            &output,
            PackOptions {
                compression: 0,
                stream_threshold_bytes: None,
            },
            &["textures".to_string()],
        )
        .unwrap();

        assert_eq!(summary.optional.len(), 1);
        let textures = &summary.optional[0];
        assert_eq!(textures.group, "textures");
        assert_eq!(textures.pack.asset_count, 1);
        let texture_id = prism_asset::core::id::AssetId::from_path("textures/icon.png");

        let base: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&summary.pack.manifest_path).unwrap())
                .unwrap();
        let base_paths: Vec<&str> = base["assets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|asset| asset["path"].as_str().unwrap())
            .collect();
        assert!(!base_paths.contains(&"textures/icon.png"));
        assert_eq!(base["optional_paks"][0]["group"], "textures");
        let base_reader = prism_asset::package::PackageReader::open(&output).expect("base package");
        assert!(base_reader.find_record(texture_id).is_none());
        let texture_reader = prism_asset::package::PackageReader::open(&textures.pak_path)
            .expect("optional package");
        assert!(texture_reader.find_record(texture_id).is_some());

        // Scene itself still referenced by base package's `scenes.json` manifest.
        assert!(base_paths.contains(&"scenes/optional.rscn"));
    }

    /// A glTF with only one triangle (buffer embedded as data URI): enough for importer to parse.
    const TINY_GLTF: &str = r#"{
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 1}]}],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
             "min": [0, 0, 0], "max": [1, 1, 0]},
            {"bufferView": 1, "componentType": 5123, "count": 3, "type": "SCALAR"}
        ],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": 36, "target": 34962},
            {"buffer": 0, "byteOffset": 36, "byteLength": 6, "target": 34963}
        ],
        "buffers": [{
            "byteLength": 42,
            "uri": "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAABAAIA"
        }]
    }"#;

    /// A glTF with two materials and one external texture. Two primitives share geometry but different materials.
    const GLTF_WITH_MATERIALS: &str = r#"{
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0}],
        "meshes": [{"primitives": [
            {"attributes": {"POSITION": 0}, "indices": 1, "material": 0},
            {"attributes": {"POSITION": 0}, "indices": 1, "material": 1}
        ]}],
        "materials": [
            {
                "name": "Red",
                "pbrMetallicRoughness": {
                    "baseColorFactor": [1.0, 0.0, 0.0, 1.0],
                    "metallicFactor": 0.2,
                    "roughnessFactor": 0.6,
                    "baseColorTexture": {"index": 0}
                }
            },
            {
                "name": "Blue",
                "pbrMetallicRoughness": {
                    "baseColorFactor": [0.0, 0.0, 1.0, 1.0],
                    "metallicFactor": 0.0,
                    "roughnessFactor": 0.9
                }
            }
        ],
        "images": [{"uri": "../textures/gltf-red.png", "name": "red"}],
        "textures": [{"source": 0}],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
             "min": [0, 0, 0], "max": [1, 1, 0]},
            {"bufferView": 1, "componentType": 5123, "count": 3, "type": "SCALAR"}
        ],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": 36, "target": 34962},
            {"buffer": 0, "byteOffset": 36, "byteLength": 6, "target": 34963}
        ],
        "buffers": [{
            "byteLength": 42,
            "uri": "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAABAAIA"
        }]
    }"#;

    /// #520: glTF material/texture automatically converted to RMAT/RTEX, scene inside empty
    /// `material_paths` filled at cook time with generated paths arranged by submesh.
    #[test]
    fn gltf_materials_and_images_are_packaged_and_linked() {
        let temp = tempfile::tempdir().unwrap();
        let assets = temp.path().join("assets");
        fs::create_dir_all(assets.join("scenes")).unwrap();
        fs::create_dir_all(assets.join("models")).unwrap();
        fs::create_dir_all(assets.join("textures")).unwrap();
        fs::write(assets.join("models/multi.gltf"), GLTF_WITH_MATERIALS).unwrap();
        fs::write(assets.join("textures/gltf-red.png"), TINY_PNG).unwrap();
        fs::write(
            assets.join("scenes/room.scene.json"),
            br#"{
                "version": 1,
                "entities": [{
                    "name": "Multi",
                    "parent": null,
                    "transform": {},
                    "components": {
                        "prism_engine::scene::MeshRenderer": {
                            "mesh_path": "models/multi.gltf"
                        }
                    }
                }]
            }"#,
        )
        .unwrap();
        write_manifest(
            &assets,
            r#"{"scenes":[{"name":"room","path":"scenes/room.scene.json","enabled":true}]}"#,
        );

        let output = temp.path().join("scenes.pak");
        let staging = temp.path().join("cooked");
        let summary =
            build_scene_package(assets.join("scenes.json"), &staging, &output, 0).unwrap();

        let meta_text = fs::read_to_string(summary.pack.manifest_path.clone()).unwrap();
        let meta: serde_json::Value = serde_json::from_str(&meta_text).unwrap();
        let paths: Vec<_> = meta["assets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|asset| asset["path"].as_str().unwrap().to_string())
            .collect();
        for expected in [
            "models/multi.gltf",
            "models/multi.gltf.materials/0_Red.mat.json",
            "models/multi.gltf.materials/1_Blue.mat.json",
            "models/multi.gltf.textures/0_red.png",
            "scenes/room.rscn",
        ] {
            assert!(
                paths.iter().any(|path| path == expected),
                "{expected} missing from pak paths: {paths:?}"
            );
        }

        // RSCN is string table + binary, generated material path must appear in scene inside.
        let rscn = fs::read(staging.join("scenes/room.rscn")).unwrap();
        for expected in [
            "models/multi.gltf.materials/0_Red.mat.json",
            "models/multi.gltf.materials/1_Blue.mat.json",
        ] {
            assert!(
                rscn.windows(expected.len())
                    .any(|window| window == expected.as_bytes()),
                "{expected} missing from cooked scene"
            );
        }

        let mut rm = ResourceManager::new();
        rm.load_package_bytes(&fs::read(&output).unwrap()).unwrap();
        rm.load_path_manifest_from_str(&meta_text).unwrap();

        let material_id = rm
            .id_by_path("models/multi.gltf.materials/0_Red.mat.json")
            .expect("generated material path");
        let material = rm.load_with_deps_raw(material_id).unwrap();
        let decoded = prism_cooker::cooker::decode_rmat(&material).expect("RMAT blob");
        assert_eq!(decoded.scalars[0], 1.0, "base color red");
        assert_eq!(decoded.scalars[4], 0.2, "metallic");
        assert_eq!(decoded.scalars[5], 0.6, "roughness");
        assert_eq!(
            decoded.texture_ids[0],
            Some(prism_asset::core::id::AssetId::from_path(
                "models/multi.gltf.textures/0_red.png"
            )),
            "generated material must reference the generated texture id"
        );

        let texture_id = rm
            .id_by_path("models/multi.gltf.textures/0_red.png")
            .expect("generated texture path");
        let texture = rm.load_with_deps_raw(texture_id).unwrap();
        assert_eq!(&texture[..4], b"RTEX");
    }

    /// #530: Scene-referenced model, material and material texture must all go into package, and material texture ID cooked inside
    /// must equal ID assigned by packer for same path.
    #[test]
    fn meshes_and_materials_are_packaged_with_matching_texture_ids() {
        let temp = tempfile::tempdir().unwrap();
        let assets = temp.path().join("assets");
        fs::create_dir_all(assets.join("scenes")).unwrap();
        fs::create_dir_all(assets.join("models")).unwrap();
        fs::create_dir_all(assets.join("materials")).unwrap();
        fs::create_dir_all(assets.join("textures")).unwrap();
        fs::write(assets.join("models/tri.gltf"), TINY_GLTF).unwrap();
        fs::write(assets.join("textures/marble.png"), TINY_PNG).unwrap();
        fs::write(
            assets.join("materials/marble.mat.json"),
            br#"{
                "base_color": [0.9, 0.85, 0.8, 1.0],
                "metallic": 0.0,
                "roughness": 0.4,
                "albedo_tex": "textures/marble.png"
            }"#,
        )
        .unwrap();
        fs::write(
            assets.join("materials/skin.mat.json"),
            br#"{
                "base_color": [1.0, 0.8, 0.7, 1.0],
                "metallic": 0.0,
                "roughness": 0.7
            }"#,
        )
        .unwrap();
        fs::write(
            assets.join("scenes/room.scene.json"),
            br#"{
                "version": 1,
                "entities": [{
                    "name": "Tri",
                    "parent": null,
                    "transform": { "position": [0, 0, 0] },
                    "components": {
                        "prism_engine::scene::MeshRenderer": {
                            "mesh_path": "models/tri.gltf",
                            "material_path": "materials/marble.mat.json",
                            "material_paths": [
                                "materials/marble.mat.json",
                                "materials/skin.mat.json"
                            ]
                        }
                    }
                }]
            }"#,
        )
        .unwrap();
        write_manifest(
            &assets,
            r#"{"scenes":[{"name":"room","path":"scenes/room.scene.json","enabled":true}]}"#,
        );

        let output = temp.path().join("scenes.pak");
        let staging = temp.path().join("cooked");
        let summary =
            build_scene_package(assets.join("scenes.json"), &staging, &output, 0).unwrap();

        let meta_text = fs::read_to_string(summary.pack.manifest_path.clone()).unwrap();
        let meta: serde_json::Value = serde_json::from_str(&meta_text).unwrap();
        let paths: Vec<String> = meta["assets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|asset| asset["path"].as_str().unwrap().to_string())
            .collect();
        for expected in [
            "models/tri.gltf",
            "materials/marble.mat.json",
            "materials/skin.mat.json",
            "textures/marble.png",
            "scenes/room.rscn",
        ] {
            assert!(
                paths.iter().any(|path| path == expected),
                "{expected} missing from pak paths: {paths:?}"
            );
        }

        let mut rm = ResourceManager::new();
        rm.load_package_bytes(&fs::read(&output).unwrap()).unwrap();
        rm.load_path_manifest_from_str(&meta_text).unwrap();

        let mesh_id = rm.id_by_path("models/tri.gltf").expect("mesh path");
        let mesh = rm.load_with_deps_raw(mesh_id).unwrap();
        assert_eq!(&mesh[..4], b"RMES", "Mesh must be cooked into RMES");

        let material_id = rm
            .id_by_path("materials/marble.mat.json")
            .expect("material path");
        let material = rm.load_with_deps_raw(material_id).unwrap();
        let decoded = prism_cooker::cooker::decode_rmat(&material).expect("RMAT blob");
        let expected_texture = prism_asset::core::id::AssetId::from_path("textures/marble.png");
        assert_eq!(
            decoded.texture_ids[0],
            Some(expected_texture),
            "Albedo texture ID cooked into material must match package-assigned one"
        );
    }

    /// #530: Referenced resource not found must cause build failure, not output a package missing resources.
    #[test]
    fn a_missing_mesh_reference_fails_the_build() {
        let temp = tempfile::tempdir().unwrap();
        let assets = temp.path().join("assets");
        fs::create_dir_all(assets.join("scenes")).unwrap();
        fs::write(
            assets.join("scenes/room.scene.json"),
            br#"{
                "version": 1,
                "entities": [{
                    "name": "Tri",
                    "parent": null,
                    "transform": {},
                    "components": {
                        "prism_engine::scene::MeshRenderer": {
                            "mesh_path": "models/gone.gltf"
                        }
                    }
                }]
            }"#,
        )
        .unwrap();
        write_manifest(
            &assets,
            r#"{"scenes":[{"name":"room","path":"scenes/room.scene.json","enabled":true}]}"#,
        );

        let err = build_scene_package(
            assets.join("scenes.json"),
            temp.path().join("cooked"),
            temp.path().join("scenes.pak"),
            0,
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("cannot be packaged") && message.contains("models/gone.gltf"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn builds_package_with_paths_declared_by_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let assets = temp.path().join("assets");
        fs::create_dir_all(assets.join("scenes")).unwrap();
        write_minimal_scene(&assets.join("scenes/intro.scene.json"));
        write_manifest(
            &assets,
            r#"{
                "scenes": [
                    {"name": "intro", "path": "scenes/intro.scene.json", "enabled": true},
                    {"name": "external", "path": "D:/external/model.gltf", "enabled": true}
                ]
            }"#,
        );

        let output = temp.path().join("scenes.pak");
        let staging = temp.path().join("cooked");
        let summary =
            build_scene_package(assets.join("scenes.json"), &staging, &output, 0).unwrap();

        assert_eq!(summary.scenes, 1);
        assert_eq!(summary.skipped, 1);
        assert!(staging.join("scenes.json").is_file());
        assert!(staging.join("scenes/intro.rscn").is_file());

        let pak = fs::read(&output).unwrap();
        let meta_text = fs::read_to_string(summary.pack.manifest_path.clone()).unwrap();
        let meta: serde_json::Value = serde_json::from_str(&meta_text).unwrap();
        let paths: Vec<_> = meta["assets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|asset| asset["path"].as_str().unwrap())
            .collect();
        // #976: Package always includes a build stamp (sorted by path, `__` sorts before letters).
        assert_eq!(
            paths,
            [
                prism_scene::cook_stamp::COOK_STAMP_NAME,
                "scenes.json",
                "scenes/intro.rscn"
            ]
        );

        let mut rm = ResourceManager::new();
        rm.load_package_bytes(&pak).unwrap();
        rm.load_path_manifest_from_str(&meta_text).unwrap();
        assert!(rm.id_by_path("scenes/intro.rscn").is_some());

        // The runtime reads this manifest from inside the package; every
        // cooked scene path it declares must resolve through the same package.
        let manifest_id = rm.id_by_path("scenes.json").expect("packaged manifest");
        let manifest_text = String::from_utf8(rm.load_with_deps_raw(manifest_id).unwrap()).unwrap();
        let packaged = prism_scene::SceneManifest::parse(&manifest_text).unwrap();
        // Author-written is `.scene.json`, package must rewrite to cooked `.rscn`.
        assert_eq!(packaged.scenes[0].path, "scenes/intro.rscn");
        for entry in packaged
            .scenes
            .iter()
            .filter(|entry| entry.path.ends_with(".rscn"))
        {
            assert!(
                rm.id_by_path(&entry.path).is_some(),
                "scene path {} missing from package",
                entry.path
            );
        }

        // End-to-end: the packaged RSCN bytes must be loadable by the runtime
        // decoder with the same component registry used by the game.
        let scene_id = rm.id_by_path("scenes/intro.rscn").unwrap();
        let scene_bytes = rm.load_with_deps_raw(scene_id).unwrap();
        let mut registry = prism_scene::ComponentRegistry::new();
        prism_scene::register_builtin_components(&mut registry);
        let decoded = prism_scene::rscn::decode_scene(&scene_bytes, &registry).unwrap();
        assert_eq!(decoded.entities.len(), 1);
    }

    #[test]
    fn only_clears_staging_created_by_build_scenes() {
        let temp = tempfile::tempdir().unwrap();
        let assets = temp.path().join("assets");
        fs::create_dir_all(assets.join("scenes")).unwrap();
        write_minimal_scene(&assets.join("scenes/intro.scene.json"));
        let manifest = assets.join("scenes.json");
        write_manifest(
            &assets,
            r#"{"scenes": [{"name": "intro", "path": "scenes/intro.rscn"}]}"#,
        );

        let staging = temp.path().join("cooked");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("unrelated.txt"), b"keep me").unwrap();
        let err =
            build_scene_package(&manifest, &staging, temp.path().join("a.pak"), 0).unwrap_err();
        assert!(err.to_string().contains("was not created by build-scenes"));

        fs::remove_file(staging.join("unrelated.txt")).unwrap();
        build_scene_package(&manifest, &staging, temp.path().join("a.pak"), 0).unwrap();
        assert!(staging_marker(&staging).unwrap().is_file());

        // A subsequent run may clear its own generated files.
        fs::write(staging.join("stale.rscn"), b"stale").unwrap();
        build_scene_package(&manifest, &staging, temp.path().join("b.pak"), 0).unwrap();
        assert!(!staging.join("stale.rscn").exists());
    }

    #[test]
    fn rejects_staging_that_contains_the_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = temp.path().join("scenes.json");
        fs::write(&manifest, "{\"scenes\": []}\n").unwrap();
        let err = build_scene_package(&manifest, temp.path(), temp.path().join("scenes.pak"), 0)
            .unwrap_err();
        assert!(err.to_string().contains("must not contain"));
    }

    fn skybox_scene_json(hdr_path: &str) -> String {
        format!(
            r#"{{
                "version": 1,
                "entities": [{{
                    "name": "Sky",
                    "parent": null,
                    "transform": {{}},
                    "components": {{
                        "prism_engine::scene::Skybox": {{ "hdr_path": "{hdr_path}" }}
                    }}
                }}]
            }}"#
        )
    }

    fn tiny_valid_hdr() -> Vec<u8> {
        let mut out = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 1 +X 2\n".to_vec();
        // Uncompressed 2-pixel scanline (RGBE: 128,128,128,128 = ~0.5,0.5,0.5)
        out.extend_from_slice(&[128, 128, 128, 128, 128, 128, 128, 128]);
        out
    }

    /// A `.rscn`-only manifest entry has no authoring `.scene.json` to read, but
    /// its environment map still has to end up inside the package.
    #[test]
    fn stages_environment_for_rscn_only_scenes() {
        let temp = tempfile::tempdir().unwrap();
        let assets = temp.path().join("assets");
        fs::create_dir_all(assets.join("scenes")).unwrap();
        fs::create_dir_all(assets.join("env")).unwrap();
        fs::write(assets.join("env/valley.hdr"), tiny_valid_hdr()).unwrap();

        // Cook, then drop the authoring doc so only the `.rscn` remains.
        let authoring = assets.join("scenes/valley.scene.json");
        fs::write(&authoring, skybox_scene_json("../env/valley.hdr")).unwrap();
        let cooked = SceneCooker::default()
            .cook_json_bytes(&fs::read(&authoring).unwrap())
            .unwrap();
        fs::remove_file(&authoring).unwrap();
        fs::write(assets.join("scenes/valley.rscn"), &cooked).unwrap();

        write_manifest(
            &assets,
            r#"{"scenes": [{"name": "valley", "path": "scenes/valley.rscn"}]}"#,
        );

        let staging = temp.path().join("cooked");
        let output = temp.path().join("scenes.pak");
        let summary =
            build_scene_package(assets.join("scenes.json"), &staging, &output, 0).unwrap();

        assert!(
            staging.join("env/valley.hdr").is_file(),
            "environment map of a .rscn-only scene must be staged"
        );

        let meta_text = fs::read_to_string(summary.pack.manifest_path.clone()).unwrap();
        let meta: serde_json::Value = serde_json::from_str(&meta_text).unwrap();
        let paths: Vec<_> = meta["assets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|asset| asset["path"].as_str().unwrap())
            .collect();
        assert!(paths.contains(&"env/valley.hdr"), "paths = {paths:?}");
    }

    /// #976: Manifest writes `.rscn`, but when author document also present **must re-cook**,
    /// Cannot let old `.rscn` beside take over (otherwise changed scene, rendered but old).
    #[test]
    fn the_authoring_document_wins_over_a_pre_cooked_rscn() {
        let temp = tempfile::tempdir().unwrap();
        let assets = temp.path().join("assets");
        fs::create_dir_all(assets.join("scenes")).unwrap();
        let authoring = assets.join("scenes/valley.scene.json");

        // 1) First cook an "old" `.rscn` (empty box scene), then leave it in assets inside.
        fs::write(&authoring, skybox_scene_json("../env/valley.hdr")).unwrap();
        let stale = SceneCooker::default()
            .cook_json_bytes(&fs::read(&authoring).unwrap())
            .unwrap();
        let stale_path = assets.join("scenes/valley.rscn");
        fs::write(&stale_path, &stale).unwrap();

        // 2) Author document changed to another scene (this inside only changes content, not delete old `.rscn`).
        fs::write(
            &authoring,
            r#"{"version": 1, "entities": [{"name": "Renamed", "parent": null,
                "transform": {}, "components": {}}]}"#,
        )
        .unwrap();

        write_manifest(
            &assets,
            r#"{"scenes": [{"name": "valley", "path": "scenes/valley.rscn"}]}"#,
        );
        let staging = temp.path().join("cooked");
        build_scene_package(
            assets.join("scenes.json"),
            &staging,
            temp.path().join("scenes.pak"),
            0,
        )
        .unwrap();

        let staged = fs::read(staging.join("scenes/valley.rscn")).unwrap();
        assert_ne!(
            staged, stale,
            "Manifest `.rscn` cannot blindly trust adjacent old file, must re-cook per author document"
        );
        let expected = SceneCooker::default()
            .cook_json_bytes(&fs::read(&authoring).unwrap())
            .unwrap();
        assert_eq!(
            staged, expected,
            "Packaging result should equal cook result of current document"
        );
    }

    /// #466: a stale `hdr_path` with a valid `hdr_guid` is resolved before
    /// staging, and the cooked RSCN carries the resolved path.
    #[test]
    fn guid_reference_wins_over_stale_path_during_build() {
        let temp = tempfile::tempdir().unwrap();
        let assets = temp.path().join("assets");
        fs::create_dir_all(assets.join("scenes")).unwrap();
        fs::create_dir_all(assets.join("env")).unwrap();
        fs::write(assets.join("env/sky.hdr"), tiny_valid_hdr()).unwrap();
        let meta = prism_asset::AssetMeta::ensure(&assets, "env/sky.hdr", "HdrImporter").unwrap();

        fs::write(
            assets.join("scenes/valley.scene.json"),
            format!(
                r#"{{
                    "version": 1,
                    "entities": [{{
                        "name": "Sky",
                        "parent": null,
                        "transform": {{}},
                        "components": {{
                            "prism_engine::scene::Skybox": {{
                                "hdr_path": "../env/gone.hdr",
                                "hdr_guid": "{}"
                            }}
                        }}
                    }}]
                }}"#,
                meta.guid
            ),
        )
        .unwrap();
        write_manifest(
            &assets,
            r#"{"scenes": [{"name": "valley", "path": "scenes/valley.scene.json"}]}"#,
        );

        let staging = temp.path().join("cooked");
        let output = temp.path().join("scenes.pak");
        build_scene_package(assets.join("scenes.json"), &staging, &output, 0).unwrap();

        assert!(staging.join("env/sky.hdr").is_file());
        let rscn = fs::read(staging.join("scenes/valley.rscn")).unwrap();
        assert!(
            rscn.windows(b"../env/sky.hdr".len())
                .any(|window| window == b"../env/sky.hdr"),
            "cooked scene must contain the resolved path"
        );
        assert!(
            !rscn
                .windows(b"gone.hdr".len())
                .any(|window| window == b"gone.hdr"),
            "the stale path must not survive cooking"
        );
    }

    /// #466: `scenes.json` entries can carry a stable GUID; the build resolves
    /// it before staging and strips it from the packaged manifest.
    #[test]
    fn manifest_guid_resolves_before_staging() {
        let temp = tempfile::tempdir().unwrap();
        let assets = temp.path().join("assets");
        fs::create_dir_all(assets.join("scenes")).unwrap();
        write_minimal_scene(&assets.join("scenes/renamed.scene.json"));
        let meta =
            prism_asset::AssetMeta::ensure(&assets, "scenes/renamed.scene.json", "SceneImporter")
                .unwrap();
        write_manifest(
            &assets,
            &format!(
                r#"{{"scenes":[{{"name":"valley","path":"scenes/old.rscn","guid":"{}"}}]}}"#,
                meta.guid
            ),
        );

        let staging = temp.path().join("cooked");
        let output = temp.path().join("scenes.pak");
        build_scene_package(assets.join("scenes.json"), &staging, &output, 0).unwrap();

        assert!(staging.join("scenes/renamed.rscn").is_file());
        let packaged: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(staging.join("scenes.json")).unwrap())
                .unwrap();
        assert_eq!(packaged["scenes"][0]["path"], "scenes/renamed.rscn");
        assert!(
            packaged["scenes"][0].get("guid").is_none(),
            "packaged manifests must not point at authoring-only GUIDs"
        );
    }

    /// `.scene.json` entries keep working, and each HDR is staged once.
    #[test]
    fn stages_environment_declared_by_the_authoring_doc() {
        let temp = tempfile::tempdir().unwrap();
        let assets = temp.path().join("assets");
        fs::create_dir_all(assets.join("scenes")).unwrap();
        fs::create_dir_all(assets.join("env")).unwrap();
        fs::write(assets.join("env/sky.hdr"), tiny_valid_hdr()).unwrap();
        fs::write(
            assets.join("scenes/main.scene.json"),
            skybox_scene_json("../env/sky.hdr"),
        )
        .unwrap();
        write_manifest(
            &assets,
            r#"{"scenes": [{"name": "main", "path": "scenes/main.scene.json"}]}"#,
        );

        let staging = temp.path().join("cooked");
        build_scene_package(
            assets.join("scenes.json"),
            &staging,
            temp.path().join("scenes.pak"),
            0,
        )
        .unwrap();

        let staged_env = fs::read(staging.join("env/sky.hdr")).unwrap();
        assert!(
            prism_gfx::ibl::CookedIbl::is_cooked(&staged_env),
            "staged environment must be pre-baked PIBL"
        );
    }
}
