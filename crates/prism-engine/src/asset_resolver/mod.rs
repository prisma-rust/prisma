//! GPU 资源解析器——从 `.pak` 资源包按需加载+缓存网格/材质/纹理资源。
//!
//! 拥有 [`ResourceManager`]（运行时资源数据库）与三个类型化缓存（确保同一网格/材质/纹理
//! 不会重复上传），并提供 [`GpuAssetResolver::prepare_requests`] 方法查询 ECS 世界中
//! 待处理的 [`MeshRenderer`] 实体，生成 GPU 上传请求。
//!
//! ## 异步模型（与渲染线程解耦）
//!
//! 资源解析被拆成两段，因为 `&mut World`（主线程）与 `&mut GraphRenderer`（渲染线程）
//! 不能同处一线程：
//! - **CPU 段（主线程）**：[`prepare_requests`] 查询待解析实体、加载 `.pak`、解交织顶点/
//!   纹理像素，产出纯数据的 [`AssetResolveRequest`]。不触碰渲染器。
//! - **GPU 段（渲染线程）**：`GraphRenderer::apply_asset_requests` 执行上传并回传句柄。
//! - 主线程再消费结果，把句柄写回 `MeshRef`/`MaterialRef`。
//!
//! 这样启动期渲染器在渲染线程异步构建时，主线程仍可处理窗口事件（关闭/移动/缩放）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use prism_asset::runtime::ResourceManager;
use prism_ecs::Entity;
use prism_ecs::World;
use prism_render::asset_bridge::{AssetRelease, AssetResolveRequest, MaterialResolveData};
use prism_render::managers::{MeshUploadInput, SkinAttributes};

use crate::scene::components::{
    MaterialRef, MeshRef, MeshRenderer, SpriteRef, SpriteRenderer, Tilemap,
};
use crate::scene::runtime_components::{
    MaterialSlots, MeshSubmesh, MeshSubmeshes, ShaderRef, TilemapRef, UiImageRef,
};
use crate::scene::systems::render::MeshBounds;

mod decoders;
#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// GpuAssetResolver
// ---------------------------------------------------------------------------

/// 资源槽位，用作「失败缓存」的键。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum AssetSlot {
    Mesh,
    Material,
    Shader,
    Sprite,
    /// 瓦片地图的 tileset（#529）。
    Tileset,
    /// UI `Style::image` 背景图（#591）。
    UiImage,
}

/// 一个网格的 CPU 上传数据 + submesh 表（#520）。
struct LoadedMesh {
    input: MeshUploadInput,
    submeshes: Vec<MeshSubmesh>,
}

/// 蒙皮网格的 CPU 段产物：顶点数据 + 蒙皮属性一起上传（#560 P0-3b）。
struct LoadedSkinnedMesh {
    input: MeshUploadInput,
    submeshes: Vec<MeshSubmesh>,
    skin: SkinAttributes,
}

/// 一个实体本帧要解析的资源（只包含仍然缺失的那几半）。
struct PendingRequest {
    entity: Entity,
    mesh_path: Option<String>,
    /// 作者组件快照：多材质路径要等网格加载后按 submesh 数展开。
    renderer: MeshRenderer,
    /// 这一帧是否需要解析材质（`MaterialRef.generation == 0`）。
    wants_material: bool,
    /// Optional custom fragment shader path (`MeshRenderer.shader_path`).
    shader_path: Option<String>,
    /// 精灵贴图 / 瓦片地图的 tileset / UI 背景图：三者都只需要一张贴图，
    /// 一个实体同一帧只会投递其中一个。
    sprite_path: Option<String>,
    tileset: bool,
    /// `sprite_path` 是 UI `Style::image`（#591）。
    ui_image: bool,
    /// 蒙皮网格路径（#560 P0-3b）：`.rskin` 走另一条解码路径，
    /// 同时产出 `SkinAttributes`。
    skinned_path: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct AtlasSpriteMeta {
    uv_rect: [f32; 4],
    trimmed_offset: [i32; 2],
    original_size: [u32; 2],
    size: [u32; 2],
}

/// On-demand asset loader + GPU uploader + cache.
///
/// 每帧调用 [`prepare_requests`] 收集待处理实体并产出上传请求（CPU 段）；
/// GPU 上传由渲染线程异步完成。
pub struct GpuAssetResolver {
    /// 已成功挂载的 `.pak`（首次 `prepare_requests` 时统一校验构建戳，#976）。
    mounted_paks: Vec<PathBuf>,
    /// 构建戳只报一次：`App::build` 阶段 logger 还没装好，日志会被丢掉。
    stamps_checked: bool,
    pub resource_manager: ResourceManager,
    /// 已投递上传请求的实体（去重：避免每帧重复 enqueue，直到结果写回 World）。
    enqueued: HashSet<Entity>,
    /// 已尝试且失败的槽位：`(实体, 槽位) -> 失败时的路径`。
    ///
    /// 路径不在清单里、解码失败这类错误不会「再试一次就好」，每帧重试只会反复
    /// 读 `.pak` 并刷日志。缓存以路径为值：路径被改写（换素材、热重载换包）时
    /// 键不再匹配，会自动重新尝试。
    failed: HashMap<(Entity, AssetSlot), String>,
    /// 最近一次解析的精灵路径。项目只改 `sprite_path`，这里发现不一致就重新解析，
    /// 不再要求调用方手动把 `SpriteRef::generation` 清零。
    sprite_paths: HashMap<Entity, String>,
    /// 最近一次解析的 tileset 路径（#529），语义同 [`Self::sprite_paths`]。
    tileset_paths: HashMap<Entity, String>,
    /// 最近一次解析的 UI 背景图路径（`Style::image`，#591）。
    ui_image_paths: HashMap<Entity, String>,
    /// Last resolved custom fragment shader path per entity.
    shader_paths: HashMap<Entity, String>,
    /// Staged custom shader paths/ids waiting for upload completion.
    pending_shader_meta: HashMap<Entity, (String, prism_scene::SceneAssetId)>,
    /// 已经交给渲染器的工程字体路径（#583）。
    ///
    /// 字体是整份文件注册进 UI 图集，不像贴图那样每实体一份；同一个路径只读一次。
    fonts_loaded: HashSet<String>,
    /// 读过但失败的字体路径：不再每帧重试（与 [`Self::failed`] 同理）。
    fonts_failed: HashSet<String>,
    /// 已经写到组件里的 GPU 槽位 / 句柄，按 `(实体, 槽位)` 记账（#495）。
    ///
    /// 组件被销毁、路径被改写、`generation` 归零（热重载）时据此回收显存；
    /// 以前这些资源只会被新上传覆盖，旧图像 / 网格永远留在 GPU 上。
    resolved: HashMap<(Entity, AssetSlot), AssetRelease>,
    /// 本实体的贴图请求属于哪个槽位（`Sprite` / `Tileset` / `UiImage`）。
    ///
    /// 结果回传时只有一个 `texture_slot`，必须知道它对应哪个组件才能记账。
    pending_texture_slot: HashMap<Entity, AssetSlot>,
    /// 待宿主转发给渲染器的释放请求。
    releases: Vec<AssetRelease>,
    /// 已投递、但句柄还没回写的网格局部包围盒（`entity → AABB`）。
    ///
    /// CPU 段已经在手里有顶点位置，所以包围盒在上传请求发出前就算好；句柄回来
    /// （[`Self::apply_results`]）之后连同句柄一起写进 [`MeshBounds`]，渲染提取
    /// 就能按主相机剔除（#491）。
    pending_bounds: HashMap<Entity, prism_render::culling::Aabb>,
    /// 已投递、但句柄还没回写的 submesh 表（#520）。
    pending_submeshes: HashMap<Entity, Vec<MeshSubmesh>>,
    /// 每个 submesh 使用本次请求里第几个唯一材质（`None` = 默认材质）。
    ///
    /// 相同路径的 submesh 共享一次材质上传，回传的槽位用这张表展开。
    pending_material_map: HashMap<Entity, Vec<Option<usize>>>,
    /// Staged atlas sub-sprite metadata waiting for texture upload completion (#1158).
    pending_atlas_sprites: HashMap<Entity, AtlasSpriteMeta>,
    /// 已经按需挂载的 `optional_*.pak`（#509）。
    ///
    /// 重复挂载同一个包只会让 `ResourceManager` 把每个资产 ID 当成重复项 warn
    /// 一遍，所以按路径记一笔，`load_optional_packages` 可以安全地每关调用。
    mounted_optional: HashSet<PathBuf>,
}

/// 引用断裂只报一次（每个路径），并且是 `error` 而不是 `warn`。
///
/// 路径不在资源包里意味着文档里的引用已经断了（文件被删 / 改名 / 拼错）。
/// 后果是物体整个不出现或贴图变白，不该被日志级别悄悄降级；但同一个断引用每个
/// 实体都要解析一次，每帧都刷一行会把 Console 淹掉，所以按路径去重（#467）。
fn report_missing_reference(path: &str, kind: &str) {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};

    static REPORTED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let reported = REPORTED.get_or_init(|| Mutex::new(HashSet::new()));
    let Ok(mut reported) = reported.lock() else {
        return;
    };
    if !reported.insert(path.to_string()) {
        return;
    }
    log::error!(
        "broken asset reference: {kind} '{path}' is not in the resource package          (deleted, renamed, or misspelt?)"
    );
}

impl GpuAssetResolver {
    pub fn new() -> Self {
        Self {
            mounted_paks: Vec::new(),
            stamps_checked: false,
            resource_manager: ResourceManager::new(),
            enqueued: HashSet::new(),
            failed: HashMap::new(),
            sprite_paths: HashMap::new(),
            tileset_paths: HashMap::new(),
            ui_image_paths: HashMap::new(),
            shader_paths: HashMap::new(),
            pending_shader_meta: HashMap::new(),
            fonts_loaded: HashSet::new(),
            fonts_failed: HashSet::new(),
            resolved: HashMap::new(),
            pending_texture_slot: HashMap::new(),
            releases: Vec::new(),
            pending_bounds: HashMap::new(),
            pending_submeshes: HashMap::new(),
            pending_material_map: HashMap::new(),
            pending_atlas_sprites: HashMap::new(),
            mounted_optional: HashSet::new(),
        }
    }

    /// 收集场景里用到的工程自带字体并读出字节（#583）。
    ///
    /// 返回**本次新读出来**的 `(资产路径, 字体字节)`；调用方把它交给
    /// `GraphRenderer::register_ui_font`。已经交过的路径不会重复返回，读失败
    /// 的路径也不再每帧重试（与网格 / 贴图的失败缓存同一策略）。
    ///
    /// 字体走 `.pak` 的路径清单（`AssetType::Font`），所以打包产物和编辑器里
    /// 看到的是同一份文件。
    pub fn prepare_ui_fonts(&mut self, world: &World) -> Vec<(String, Vec<u8>)> {
        let _span = crate::util::timing::span("asset::prepare_ui_fonts");
        let mut paths: Vec<String> = Vec::new();
        for (_, text) in world.query::<crate::ui::components::Text>() {
            let path = text.font.trim();
            if path.is_empty()
                || self.fonts_loaded.contains(path)
                || self.fonts_failed.contains(path)
                || paths.iter().any(|known| known == path)
            {
                continue;
            }
            paths.push(path.to_string());
        }

        let mut out = Vec::new();
        for path in paths {
            let Some(id) = self.resource_manager.id_by_path(&path) else {
                report_missing_reference(&path, "font");
                self.fonts_failed.insert(path);
                continue;
            };
            match self.resource_manager.load_with_deps_raw(id) {
                Ok(bytes) if !bytes.is_empty() => {
                    log::info!("ui fonts: loaded {} ({} bytes)", path, bytes.len());
                    self.fonts_loaded.insert(path.clone());
                    out.push((path, bytes));
                }
                Ok(_) => {
                    log::warn!("ui fonts: '{path}' is empty — ignoring it");
                    self.fonts_failed.insert(path);
                }
                Err(error) => {
                    log::warn!("ui fonts: load '{path}' failed: {error}");
                    self.fonts_failed.insert(path);
                }
            }
        }
        out
    }

    /// 记下「这个实体的这个槽位现在用的是哪个 GPU 资源」。
    ///
    /// 覆盖旧值时如果资源换了（重新上传到新槽位），旧的那份立即排进释放队列
    /// —— 组件在同一帧被改写，不会出现「还在采样已释放槽位」的中间帧。
    fn record_resolved(&mut self, entity: Entity, slot: AssetSlot, value: AssetRelease) {
        if value.is_empty() {
            return;
        }
        if let Some(previous) = self.resolved.insert((entity, slot), value.clone()) {
            if previous != value {
                self.releases.push(previous);
            }
        }
    }

    /// 这一帧里该实体的这个槽位是否已经不再需要 GPU 资源。
    ///
    /// 判定标准只有两条，都是「确定不会再画」：承载它的组件没了，或
    /// `generation == 0`（`invalidate_*` 刚把它标脏，马上会重新上传）。
    fn slot_is_stale(&self, world: &World, entity: Entity, slot: AssetSlot) -> bool {
        use crate::scene::runtime_components::{TilemapRef, UiImageRef};
        match slot {
            AssetSlot::Mesh => {
                world.get::<MeshRenderer>(entity).is_none()
                    || world
                        .get::<MeshRef>(entity)
                        .map(|r| r.generation == 0)
                        .unwrap_or(true)
            }
            AssetSlot::Material => {
                world.get::<MeshRenderer>(entity).is_none()
                    || world
                        .get::<MaterialRef>(entity)
                        .map(|r| r.generation == 0)
                        .unwrap_or(true)
            }
            AssetSlot::Shader => {
                (world
                    .get::<MeshRenderer>(entity)
                    .is_none_or(|r| r.shader_path.is_empty())
                    && world
                        .get::<prism_scene::components::SkinnedMeshRenderer>(entity)
                        .is_none_or(|r| r.shader_path.is_empty()))
                    || world
                        .get::<ShaderRef>(entity)
                        .is_none_or(|r| r.generation == 0)
            }
            AssetSlot::Sprite => {
                world.get::<SpriteRenderer>(entity).is_none()
                    || world
                        .get::<SpriteRef>(entity)
                        .map(|r| r.generation == 0)
                        .unwrap_or(true)
            }
            AssetSlot::Tileset => {
                world.get::<Tilemap>(entity).is_none()
                    || world
                        .get::<TilemapRef>(entity)
                        .map(|r| r.generation == 0)
                        .unwrap_or(true)
            }
            AssetSlot::UiImage => {
                world
                    .get::<crate::ui::components::Style>(entity)
                    .map(|style| style.image.is_empty())
                    .unwrap_or(true)
                    || world
                        .get::<UiImageRef>(entity)
                        .map(|r| r.generation == 0)
                        .unwrap_or(true)
            }
        }
    }

    /// 把已经失效的 GPU 资源排进释放队列（由 [`Self::take_releases`] 交给宿主）。
    fn release_stale_assets(&mut self, world: &World) {
        let stale: Vec<(Entity, AssetSlot)> = self
            .resolved
            .keys()
            .filter(|(entity, slot)| self.slot_is_stale(world, *entity, *slot))
            .copied()
            .collect();
        for key in stale {
            if let Some(release) = self.resolved.remove(&key) {
                self.releases.push(release);
            }
        }
        // 实体被销毁后，还没回传的「贴图属于哪个槽位」的登记也没用了。
        self.pending_texture_slot.retain(|entity, _| {
            self.resolved.keys().any(|(e, _)| e == entity)
                || world.get::<SpriteRenderer>(*entity).is_some()
                || world.get::<Tilemap>(*entity).is_some()
                || world.get::<crate::ui::components::Style>(*entity).is_some()
        });
        self.pending_atlas_sprites
            .retain(|entity, _| world.get::<SpriteRenderer>(*entity).is_some());
    }

    /// 换场 / 卸载后立刻丢弃已经不存在的实体的缓存（#482）。
    ///
    /// 等价于 `prepare_requests` 开头的清理，但可以显式调用，让换场当帧就把
    /// 旧世界的槽位交回渲染器、并清掉 `enqueued` / 失败缓存里对不上的键。
    pub fn forget_missing_entities(&mut self, world: &World) {
        self.release_stale_assets(world);
        self.enqueued.retain(|entity| {
            world.get::<MeshRenderer>(*entity).is_some()
                || world.get::<SpriteRenderer>(*entity).is_some()
                || world.get::<Tilemap>(*entity).is_some()
                || world
                    .get::<crate::ui::components::Style>(*entity)
                    .is_some_and(|style| !style.image.is_empty())
        });
        self.failed.retain(|(entity, _), _| {
            world.get::<MeshRenderer>(*entity).is_some()
                || world.get::<SpriteRenderer>(*entity).is_some()
                || world.get::<Tilemap>(*entity).is_some()
                || world
                    .get::<crate::ui::components::Style>(*entity)
                    .is_some_and(|style| !style.image.is_empty())
        });
    }

    /// 取走待释放的 GPU 资源；宿主把它们转给渲染器（`GraphRenderer::release_assets`）。
    ///
    /// 在 `prepare_requests` 之后调用：失效的槽位在这一帧内就被回收，重新上传
    /// 的请求同时入队，所以热重载不会每次都多占一份显存。
    pub fn take_releases(&mut self) -> Vec<AssetRelease> {
        std::mem::take(&mut self.releases)
    }

    /// 该实体的这个槽位是否已经因同一个路径失败过。
    fn failed_before(&self, entity: Entity, slot: AssetSlot, path: &str) -> bool {
        self.failed
            .get(&(entity, slot))
            .is_some_and(|failed_path| failed_path == path)
    }

    /// 记住一个加载失败：路径不在清单 / 解码失败都不会因为下一帧再试而变好，
    /// 缓存后同一个路径只尝试一次（路径被改写时自动重新尝试）。
    fn record_failure(&mut self, entity: Entity, slot: AssetSlot, path: &str) {
        log::warn!(
            "asset_resolver: {slot:?} '{path}' failed to load; \
             not retrying until the path changes"
        );
        self.failed.insert((entity, slot), path.to_string());
    }

    // -----------------------------------------------------------------------
    // Package loading
    // -----------------------------------------------------------------------

    /// Attempt to load the `.pak` resource package and its path manifest.
    ///
    /// Both files are optional — when absent (no CLI `build` run yet) the
    /// engine continues with only procedural geometry.
    ///
    /// Paths are resolved through [`crate::asset_paths`] instead of the process
    /// cwd, so launching the executable from any directory still finds the
    /// package that sits next to it.
    pub fn load_resource_package(&mut self) {
        let mut span = crate::util::timing::span("asset::load_resource_package");
        let Some(asset_dir) = crate::asset_paths::asset_dir() else {
            log::warn!("resource package unavailable: no `assets/` directory found");
            return;
        };
        // #530：挂载 `assets/` 下**所有** `.pak`，而不是写死 `scenes.pak`。
        // 构建可以按内容分包（场景 / 模型贴图 / 后续的流式包），运行时不需要
        // 知道分了多少个包。
        let paks = crate::asset_paths::pak_files_in(&asset_dir);
        if paks.is_empty() {
            log::warn!(
                "resource package unavailable: no `.pak` in {}; scene assets cannot load. \
                 Build the package, then restart: {}",
                asset_dir.display(),
                crate::asset_paths::build_scenes_command(&asset_dir)
            );
            return;
        }

        let loaded = self.mount_packages(&paks);
        span.detail(format!(
            "paks={} mounted={} assets={}",
            paks.len(),
            loaded.len(),
            self.resource_manager.asset_count()
        ));

        log::info!(
            "resource packages loaded from {}: {} package(s), {} assets registered",
            asset_dir.display(),
            loaded.len(),
            self.resource_manager.asset_count(),
        );
    }

    /// #509：按需挂载 `assets/optional/` 下的分包（`optional_*.pak`）。
    ///
    /// 自动挂载只扫描 `assets/` 顶层，所以分包里的字节不会在启动时常驻。进入
    /// 需要这些资源的关卡 / 玩法时显式调用；已经挂过的包会被跳过，重复调用安全。
    ///
    /// 返回**本次新挂载**的包数（目录不存在、为空或都已挂载时为 0）。
    pub fn load_optional_packages(&mut self) -> usize {
        let Some(dir) = crate::asset_paths::optional_pak_dir() else {
            return 0;
        };
        let paks: Vec<PathBuf> = crate::asset_paths::pak_files_in(&dir)
            .into_iter()
            .filter(|pak| !self.mounted_optional.contains(pak))
            .collect();
        if paks.is_empty() {
            log::debug!("no un-mounted optional packages in {}", dir.display());
            return 0;
        }

        let loaded = self.mount_packages(&paks);
        let count = loaded.len();
        // 只把真正加载成功的包记下来；失败的包下次调用还会重新尝试。
        self.mounted_optional.extend(loaded);
        log::info!(
            "optional resource packages loaded from {}: {count} package(s), {} assets registered",
            dir.display(),
            self.resource_manager.asset_count(),
        );
        count
    }

    /// #509：新挂载可选包后，让之前因为「路径不在包里」失败的实体重试一次。
    ///
    /// 失败缓存本来是「同一个坏路径只报一次」，但可选包是延迟挂载的：挂载前
    /// 解析过的实体已经记下了失败。这里只清理**现在能解析到**的路径对应的实体，
    /// 不动其它失败项。返回重新排队的实体数。
    pub fn retry_failed_after_package_change(&mut self, world: &mut World) -> usize {
        let retry: Vec<Entity> = self
            .failed
            .iter()
            .filter(|(_, path)| self.resource_manager.id_by_path(path).is_some())
            .map(|((entity, _), _)| *entity)
            .collect();
        let mut retried = 0usize;
        for entity in retry {
            self.reset_entity_refs(world, entity);
            retried += 1;
        }
        if retried > 0 {
            log::info!(
                "resource packages: {retried} entity(ies) will retry assets from a newly mounted package"
            );
        }
        retried
    }

    /// Load a list of `.pak` files plus their `<name>.pak.meta.json` manifests.
    /// Returns the packages that were registered (failed ones are skipped).
    fn mount_packages(&mut self, paks: &[PathBuf]) -> Vec<PathBuf> {
        let mut loaded = Vec::new();
        for pak_path in paks {
            let mut pak_span = crate::util::timing::span("asset::mount_package");
            pak_span.detail(pak_path.display().to_string());
            if let Err(e) = self.resource_manager.load_package(pak_path) {
                log::warn!(
                    "resource package {} could not be loaded: {e}; \
                     assets from it will be missing",
                    pak_path.display()
                );
                continue;
            }
            loaded.push(pak_path.clone());
            if !self.mounted_paks.iter().any(|mounted| mounted == pak_path) {
                self.mounted_paks.push(pak_path.clone());
            }

            // 每个包旁边有 `<name>.pak.meta.json`（路径清单）时一并加载。
            let manifest_path = pak_path.with_extension("pak.meta.json");
            if manifest_path.is_file() {
                let mut manifest_span = crate::util::timing::span("asset::load_path_manifest");
                manifest_span.detail(manifest_path.display().to_string());
                if let Err(e) = self.resource_manager.load_path_manifest(&manifest_path) {
                    log::warn!(
                        "failed to load path manifest {}: {e} \
                         (asset resolution by path won't work for this package)",
                        manifest_path.display()
                    );
                }
            } else {
                log::warn!(
                    "resource package {} has no path manifest ({})",
                    pak_path.display(),
                    manifest_path.display()
                );
            }
        }
        loaded
    }

    /// 校验包内构建戳（#976）：包过期时给出明确提示，而不是安静地画错内容。
    ///
    /// 三种情况：
    /// - 没有戳 → 包由更旧的管线构建，无法判断新鲜度（warn，不阻断）；
    /// - schema 指纹不同 → 引擎换了组件布局，旧包可能解出错误数据（error）；
    /// - 工程文档比包新 → 改了场景忘改打包（error，带重建命令）。
    /// 首次取资源时统一校验一次已挂载的包（logger 此时已就绪）。
    fn check_cook_stamps_once(&mut self) {
        if self.stamps_checked {
            return;
        }
        self.stamps_checked = true;
        let _span = crate::util::timing::span("asset::check_cook_stamps_once");
        let paks = std::mem::take(&mut self.mounted_paks);
        for pak in &paks {
            self.check_cook_stamp(pak);
        }
        self.mounted_paks = paks;
    }

    /// Pub 版给热重载用：重新打包后同样要校验一次（#976）。
    ///
    /// 三种情况：
    /// - 没有戳 / 戳读不出来 → 包由更旧的管线构建（warn，不阻断）；
    /// - schema 指纹不同 → 引擎换了组件布局，旧包可能解出错误数据（error）；
    /// - 工程文档比包新 → 改了场景忘改打包（error，带重建命令）。
    ///
    /// 注意 `App::build` 阶段也会挂包，但那时 logger 还没装，所以真正的校验
    /// 推迟到第一次 `prepare_requests`（见 [`Self::check_cook_stamps_once`]）。
    pub fn check_cook_stamp(&mut self, pak_path: &Path) {
        use prism_scene::cook_stamp as stamp_mod;
        let stamp = self
            .resource_manager
            .id_by_path(stamp_mod::COOK_STAMP_NAME)
            .and_then(|id| self.resource_manager.load_with_deps_raw(id).ok())
            .and_then(|bytes| stamp_mod::CookStamp::parse(&String::from_utf8_lossy(&bytes)));
        // 发布版没有 `assets/scenes/*.scene.json`，扫出来是 0 = 只比对 schema。
        let newest = {
            let mut walk_span = crate::util::timing::span("asset::newest_source_mtime");
            let newest = pak_path
                .parent()
                .map(stamp_mod::newest_source_mtime)
                .unwrap_or(0);
            walk_span.detail(format!("newest={newest}"));
            newest
        };
        match stamp_mod::check(
            stamp,
            prism_scene::registry::builtin_schema_fingerprint(),
            newest,
        ) {
            stamp_mod::StampCheck::Fresh => {
                log::debug!("resource package {} is up to date", pak_path.display());
            }
            stamp_mod::StampCheck::Missing => {
                log::warn!(
                    "resource package {} has no cook stamp: it was built by an older pipeline, \
                     so its freshness cannot be verified. If the scene looks wrong, rebuild it: \
                     `prism-build-pipeline build-scenes`",
                    pak_path.display()
                );
            }
            stamp_mod::StampCheck::SchemaChanged { cooked, current } => {
                log::error!(
                    "resource package {} was cooked with component schema {cooked:#x}, this \
                     engine has {current:#x}: scene data may decode incorrectly. \
                     Rebuild with `prism-build-pipeline build-scenes`",
                    pak_path.display()
                );
            }
            stamp_mod::StampCheck::SourcesNewer { cooked, newest } => {
                log::error!(
                    "resource package {} is older than the project documents (pak {cooked}, \
                     newest source {newest}): the scene will render stale content. \
                     Rebuild with `prism-build-pipeline build-scenes`",
                    pak_path.display()
                );
            }
        }
    }

    /// 从内存资源加载 `.pak` 及其路径 manifest。
    pub fn load_resource_package_bytes(
        &mut self,
        pak_bytes: &[u8],
        path_manifest_json: Option<&str>,
    ) -> anyhow::Result<()> {
        self.resource_manager.load_package_bytes(pak_bytes)?;
        if let Some(manifest) = path_manifest_json {
            self.resource_manager
                .load_path_manifest_from_str(manifest)?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // 主线程：CPU 段——收集待解析实体并产出上传请求
    // -----------------------------------------------------------------------

    /// 查询 ECS 世界中待处理的 [`MeshRenderer`] 实体，加载 `.pak` + 解交织，
    /// 产出与 `World` 完全解耦的 [`AssetResolveRequest`]（纯数据，Send）。
    ///
    /// 不触碰 `GraphRenderer`——GPU 上传由渲染线程的 `apply_asset_requests` 完成。
    /// 返回的请求由调用方通过 [`prism_app::RenderShared`] 通道交给渲染线程。
    ///
    /// 返回数量即本帧新投递的实体数。已 enqueue 但尚未写回结果的实体会被去重跳过。
    ///
    /// 每个实体只携带「仍然缺失」的那几半：网格已解析、材质还缺时不会再加载/上传
    /// 网格（旧实现会把整份请求重发一遍，导致每帧注册新的 GPU 资源）。加载失败的
    /// 槽位会记入失败缓存（[`GpuAssetResolver::failed`]），同一坏路径只尝试一次，
    /// 直到路径被改写。
    pub fn prepare_requests(&mut self, world: &World) -> Vec<(Entity, AssetResolveRequest)> {
        let mut span = crate::util::timing::span("asset::prepare_requests");
        // #976：构建戳在**第一次**取资源时才校验。启动时的挂包发生在
        // `App::build`，那时 `env_logger` 还没初始化，任何 warn/error 都会被丢掉
        // ——这正是「旧包静默画错」能长期存在的原因之一。
        self.check_cook_stamps_once();
        // 先回收这一帧已经失效的 GPU 资源（#495）：实体没了、组件被删、或
        // `generation` 归零（热重载）时把槽位交还给渲染器，再重新上传。
        self.release_stale_assets(world);
        // 运行时生成的网格（地形，#517）：路径以 `__proc/` 开头，直接从世界里的
        // 表取，不经过 `.pak`。
        let procedural = world.get_resource::<crate::scene::systems::terrain::ProceduralMeshes>();
        // 收集待处理实体（首个），避免同时借用世界 & self。
        let pending: Vec<PendingRequest> = {
            // 失败缓存只对仍然存在的实体有意义；实体被删除后顺手回收。
            self.sprite_paths
                .retain(|entity, _| world.get::<SpriteRenderer>(*entity).is_some());
            // 实体在上传途中被删除时，包围盒也不该留在表里。
            self.pending_bounds
                .retain(|entity, _| world.get::<MeshRef>(*entity).is_some());
            self.failed.retain(|(entity, _), _| {
                world.get::<MeshRenderer>(*entity).is_some()
                    || world.get::<SpriteRenderer>(*entity).is_some()
                    || world.get::<Tilemap>(*entity).is_some()
                    || world
                        .get::<crate::ui::components::Style>(*entity)
                        .is_some_and(|style| !style.image.is_empty())
            });
            self.tileset_paths
                .retain(|entity, _| world.get::<Tilemap>(*entity).is_some());
            self.ui_image_paths.retain(|entity, _| {
                world
                    .get::<crate::ui::components::Style>(*entity)
                    .is_some_and(|style| !style.image.is_empty())
            });
            self.shader_paths.retain(|entity, _| {
                world.get::<MeshRenderer>(*entity).is_some()
                    || world
                        .get::<prism_scene::components::SkinnedMeshRenderer>(*entity)
                        .is_some()
            });

            let mut out = Vec::new();
            for (entity, mr) in world.query::<MeshRenderer>() {
                // 只有「有路径」且「句柄还没解析」才算待处理：
                // - 路径为空 = 作者没给资源，没有东西可加载（例如无材质的网格，
                //   由渲染器回落到默认材质槽）；
                // - 运行时组件缺失 = 结果无处写回，投递出去只会每帧重复 enqueue；
                // - 失败缓存命中 = 同一个坏路径不再重试。
                // 蒙皮实体（#560 P0-4）的 `MeshRenderer.mesh_path` 只是「源 glTF 路径」
                // —— 打包器靠它 stage 资源。真正画的是 `.rskin`，所以这里别再上传
                // 一份静态 RMES（浪费显存，静态绘制也会被 `skinned_draw` 跳过）。
                let skinned = world
                    .get::<crate::scene::components::SkinnedMeshRenderer>(entity)
                    .is_some();
                let wants_mesh = !skinned
                    && !mr.mesh_path.is_empty()
                    && world
                        .get::<MeshRef>(entity)
                        .is_some_and(|r| r.generation == 0)
                    && !self.failed_before(entity, AssetSlot::Mesh, &mr.mesh_path);
                // 多材质（#520）：只要还有没失败过的路径就继续尝试。失败缓存按
                // 路径记，所以某个 submesh 的坏材质不会拖住其它材质。
                let material_unresolved = world
                    .get::<MaterialRef>(entity)
                    .is_some_and(|r| r.generation == 0);
                let wants_material = material_unresolved
                    && ((!mr.material_path.is_empty()
                        && !self.failed_before(entity, AssetSlot::Material, &mr.material_path))
                        || mr.material_paths.iter().any(|path| {
                            !path.is_empty()
                                && !self.failed_before(entity, AssetSlot::Material, path)
                        }));
                let shader_unresolved = world
                    .get::<ShaderRef>(entity)
                    .is_none_or(|r| r.generation == 0);
                let shader_changed = self
                    .shader_paths
                    .get(&entity)
                    .is_some_and(|prev| prev != &mr.shader_path);
                let wants_shader = !mr.shader_path.is_empty()
                    && (shader_unresolved || shader_changed)
                    && !self.failed_before(entity, AssetSlot::Shader, &mr.shader_path);
                if (wants_mesh || wants_material || wants_shader)
                    && !self.enqueued.contains(&entity)
                {
                    out.push(PendingRequest {
                        entity,
                        mesh_path: wants_mesh.then(|| mr.mesh_path.clone()),
                        renderer: mr.clone(),
                        wants_material,
                        shader_path: wants_shader.then(|| mr.shader_path.clone()),
                        sprite_path: None,
                        tileset: false,
                        ui_image: false,
                        skinned_path: None,
                    });
                    self.enqueued.insert(entity);
                }
            }
            // 1b. 蒙皮网格（#560 P0-3b）：`.rskin` 自带顶点 + 权重，走专用解码；
            // 材质解析与静态网格共用同一套（`renderer` 是等价的 MeshRenderer）。
            for (entity, skinned) in world.query::<prism_scene::components::SkinnedMeshRenderer>() {
                let path = &skinned.skinned_mesh_path;
                let wants_mesh = !path.is_empty()
                    && world
                        .get::<MeshRef>(entity)
                        .is_some_and(|r| r.generation == 0)
                    && !self.failed_before(entity, AssetSlot::Mesh, path);
                let wants_material = world
                    .get::<MaterialRef>(entity)
                    .is_some_and(|r| r.generation == 0)
                    && ((!skinned.material_path.is_empty()
                        && !self.failed_before(
                            entity,
                            AssetSlot::Material,
                            &skinned.material_path,
                        ))
                        || skinned.material_paths.iter().any(|path| {
                            !path.is_empty()
                                && !self.failed_before(entity, AssetSlot::Material, path)
                        }));
                let shader_unresolved = world
                    .get::<ShaderRef>(entity)
                    .is_none_or(|r| r.generation == 0);
                let shader_changed = self
                    .shader_paths
                    .get(&entity)
                    .is_some_and(|prev| prev != &skinned.shader_path);
                let wants_shader = !skinned.shader_path.is_empty()
                    && (shader_unresolved || shader_changed)
                    && !self.failed_before(entity, AssetSlot::Shader, &skinned.shader_path);
                if (wants_mesh || wants_material || wants_shader)
                    && !self.enqueued.contains(&entity)
                {
                    out.push(PendingRequest {
                        entity,
                        mesh_path: None,
                        renderer: skinned.as_mesh_renderer(),
                        wants_material,
                        shader_path: wants_shader.then(|| skinned.shader_path.clone()),
                        sprite_path: None,
                        tileset: false,
                        ui_image: false,
                        skinned_path: wants_mesh.then(|| path.clone()),
                    });
                    self.enqueued.insert(entity);
                }
            }
            // 2D 精灵：只有贴图一条资源，`SpriteRef.generation == 0` 表示待解析。
            for (entity, sprite) in world.query::<SpriteRenderer>() {
                if !sprite.has_sprite() {
                    continue;
                }
                let unresolved = world
                    .get::<SpriteRef>(entity)
                    .map(|r| r.generation == 0)
                    .unwrap_or(true);
                // 没有 `SpriteRef`（运行期新生成的精灵）或路径被改写都算待解析。
                let path_changed = self
                    .sprite_paths
                    .get(&entity)
                    .is_some_and(|last| last != &sprite.sprite_path);
                if (unresolved || path_changed)
                    && !self.failed_before(entity, AssetSlot::Sprite, &sprite.sprite_path)
                    && !self.enqueued.contains(&entity)
                {
                    out.push(PendingRequest {
                        entity,
                        mesh_path: None,
                        renderer: MeshRenderer::default(),
                        wants_material: false,
                        shader_path: None,
                        sprite_path: Some(sprite.sprite_path.clone()),
                        tileset: false,
                        ui_image: false,
                        skinned_path: None,
                    });
                    self.enqueued.insert(entity);
                    self.sprite_paths.insert(entity, sprite.sprite_path.clone());
                }
            }
            // 瓦片地图：同样只有 tileset 一张贴图（#529）。
            for (entity, tilemap) in world.query::<Tilemap>() {
                if tilemap.tileset_path.is_empty() {
                    continue;
                }
                let unresolved = world
                    .get::<TilemapRef>(entity)
                    .map(|r| r.generation == 0)
                    .unwrap_or(true);
                let path_changed = self
                    .tileset_paths
                    .get(&entity)
                    .is_some_and(|last| last != &tilemap.tileset_path);
                if (unresolved || path_changed)
                    && !self.failed_before(entity, AssetSlot::Tileset, &tilemap.tileset_path)
                    && !self.enqueued.contains(&entity)
                {
                    out.push(PendingRequest {
                        entity,
                        mesh_path: None,
                        renderer: MeshRenderer::default(),
                        wants_material: false,
                        shader_path: None,
                        sprite_path: Some(tilemap.tileset_path.clone()),
                        tileset: true,
                        ui_image: false,
                        skinned_path: None,
                    });
                    self.enqueued.insert(entity);
                    self.tileset_paths
                        .insert(entity, tilemap.tileset_path.clone());
                }
            }
            // UI 背景图（#591）：和精灵一样只需要一张贴图，结果写进 `UiImageRef`。
            for (entity, style) in world.query::<crate::ui::components::Style>() {
                if style.image.is_empty() {
                    continue;
                }
                let unresolved = world
                    .get::<UiImageRef>(entity)
                    .map(|r| r.generation == 0)
                    .unwrap_or(true);
                let path_changed = self
                    .ui_image_paths
                    .get(&entity)
                    .is_some_and(|last| last != &style.image);
                if (unresolved || path_changed)
                    && !self.failed_before(entity, AssetSlot::UiImage, &style.image)
                    && !self.enqueued.contains(&entity)
                {
                    out.push(PendingRequest {
                        entity,
                        mesh_path: None,
                        renderer: MeshRenderer::default(),
                        wants_material: false,
                        shader_path: None,
                        sprite_path: Some(style.image.clone()),
                        tileset: false,
                        ui_image: true,
                        skinned_path: None,
                    });
                    self.enqueued.insert(entity);
                    self.ui_image_paths.insert(entity, style.image.clone());
                }
            }
            out
        };

        if pending.is_empty() {
            return Vec::new();
        }

        let mut reqs = Vec::with_capacity(pending.len());
        for item in pending {
            let loaded = match item.mesh_path.as_deref() {
                Some(path) => {
                    let loaded = procedural
                        .and_then(|meshes| meshes.get(path))
                        .map(|input| LoadedMesh {
                            input,
                            submeshes: Vec::new(),
                        })
                        .or_else(|| self.load_mesh_inputs(path));
                    if loaded.is_none() {
                        self.record_failure(item.entity, AssetSlot::Mesh, path);
                    }
                    loaded
                }
                None => None,
            };
            // 蒙皮网格（#560 P0-3b）：顶点 + 权重一起上传，其余流程与静态网格一致。
            let mut skin = None;
            let loaded = match item.skinned_path.as_deref() {
                Some(path) => {
                    let loaded = self.load_skinned_mesh_inputs(path);
                    if loaded.is_none() {
                        self.record_failure(item.entity, AssetSlot::Mesh, path);
                    }
                    if let Some(skinned) = loaded.as_ref() {
                        skin = Some(skinned.skin.clone());
                    }
                    loaded.map(|skinned| LoadedMesh {
                        input: skinned.input,
                        submeshes: skinned.submeshes,
                    })
                }
                None => loaded,
            };
            let (mesh, submeshes) = match loaded {
                Some(loaded) => (Some(loaded.input), loaded.submeshes),
                // 网格已经解析过、这帧只补材质：用运行时缓存的 submesh 表。
                None => (
                    None,
                    world
                        .get::<MeshSubmeshes>(item.entity)
                        .map(|cached| cached.submeshes.clone())
                        .unwrap_or_default(),
                ),
            };
            if !submeshes.is_empty() {
                self.pending_submeshes
                    .insert(item.entity, submeshes.clone());
            }

            // 材质按 submesh 展开（#520）：相同路径只上传一次，回传后按
            // `pending_material_map` 复制到每个 submesh。
            let mut materials: Vec<Option<MaterialResolveData>> = Vec::new();
            if item.wants_material {
                let submesh_count = if submeshes.is_empty() {
                    1
                } else {
                    submeshes.len()
                };
                let mut unique_paths: Vec<String> = Vec::new();
                let mut material_map: Vec<Option<usize>> = Vec::with_capacity(submesh_count);
                for submesh in 0..submesh_count {
                    let material_index = submeshes
                        .get(submesh)
                        .and_then(|submesh| submesh.material_index);
                    let path = item.renderer.material_path_for_submesh(
                        submesh,
                        submesh_count,
                        material_index,
                    );
                    match path {
                        Some(path)
                            if !self.failed_before(item.entity, AssetSlot::Material, path) =>
                        {
                            let index = unique_paths
                                .iter()
                                .position(|known| known == path)
                                .unwrap_or_else(|| {
                                    unique_paths.push(path.to_string());
                                    unique_paths.len() - 1
                                });
                            material_map.push(Some(index));
                        }
                        _ => material_map.push(None),
                    }
                }
                for path in &unique_paths {
                    let loaded = self.load_material_data(path);
                    if loaded.is_none() {
                        self.record_failure(item.entity, AssetSlot::Material, path);
                    }
                    materials.push(loaded);
                }
                self.pending_material_map.insert(item.entity, material_map);
            }

            let texture = match item.sprite_path.as_deref() {
                Some(path) => {
                    let loaded = self.load_texture_by_path(item.entity, path);
                    if loaded.is_none() {
                        let slot = if item.tileset {
                            AssetSlot::Tileset
                        } else if item.ui_image {
                            AssetSlot::UiImage
                        } else {
                            AssetSlot::Sprite
                        };
                        self.record_failure(item.entity, slot, path);
                    }
                    loaded
                }
                None => None,
            };
            if texture.is_some() {
                let slot = if item.tileset {
                    AssetSlot::Tileset
                } else if item.ui_image {
                    AssetSlot::UiImage
                } else {
                    AssetSlot::Sprite
                };
                self.pending_texture_slot.insert(item.entity, slot);
            }
            // 包围盒在请求发出前算好：GPU 上传后句柄才回来，而 CPU 段此刻
            // 已经解出顶点位置了（#491）。
            if let Some(bounds) = mesh.as_ref().and_then(MeshUploadInput::bounds) {
                self.pending_bounds.insert(item.entity, bounds);
            }
            let shader = match item.shader_path {
                Some(path) => {
                    let loaded =
                        self.resource_manager.id_by_path(&path).and_then(|id| {
                            match self
                                .resource_manager
                                .load::<prism_asset::runtime::ShaderAsset>(id)
                            {
                                Ok(handle) => self
                                    .resource_manager
                                    .get(handle)
                                    .ok()
                                    .map(|asset| (id, asset.spirv.clone())),
                                Err(err) => {
                                    log::warn!(
                                        "asset_resolver: ShaderAsset '{path}' decode failed: {err}"
                                    );
                                    None
                                }
                            }
                        });
                    match loaded {
                        Some((id, spirv)) => {
                            self.pending_shader_meta.insert(
                                item.entity,
                                (path, prism_scene::SceneAssetId::from_raw(id.0)),
                            );
                            Some(spirv)
                        }
                        None => {
                            report_missing_reference(&path, "shader");
                            self.record_failure(item.entity, AssetSlot::Shader, &path);
                            None
                        }
                    }
                }
                None => None,
            };
            reqs.push((
                item.entity,
                AssetResolveRequest {
                    skin,
                    mesh,
                    materials,
                    texture,
                    shader,
                },
            ));
        }

        span.detail(format!("staged={}", reqs.len()));
        if !reqs.is_empty() {
            log::info!(
                "prepare_requests: staged {} entit(y/ies) for async GPU upload",
                reqs.len()
            );
        }
        reqs
    }

    /// 结果写回后调用：解除该实体的 enqueue 去重标记，并写入 GPU 句柄。
    ///
    /// 必须在主线程调用（持有 `&mut World`）。
    pub fn apply_results(
        &mut self,
        world: &mut World,
        results: &[(Entity, prism_render::asset_bridge::AssetResolveResult)],
    ) {
        for (entity, result) in results {
            if let Some(mr) = world.get_mut::<MeshRef>(*entity) {
                if let Some(h) = result.mesh_handle {
                    mr.render_handle = h;
                    mr.generation = 1;
                }
            }
            if let Some(handle) = result.mesh_handle {
                self.record_resolved(
                    *entity,
                    AssetSlot::Mesh,
                    AssetRelease {
                        mesh_handle: Some(handle),
                        ..AssetRelease::default()
                    },
                );
            }
            if let Some(bounds) = self.pending_bounds.remove(entity) {
                if let Some(handle) = result.mesh_handle {
                    if world.get_resource::<MeshBounds>().is_none() {
                        world.insert_resource(MeshBounds::default());
                    }
                    if let Some(registry) = world.get_resource_mut::<MeshBounds>() {
                        registry.insert(handle, bounds);
                    }
                }
            }
            if let Some(submeshes) = self.pending_submeshes.remove(entity) {
                if result.mesh_handle.is_some() {
                    if submeshes.is_empty() {
                        world.remove::<MeshSubmeshes>(*entity);
                    } else {
                        world.insert(*entity, MeshSubmeshes { submeshes });
                    }
                }
            }
            match self.pending_material_map.remove(entity) {
                Some(material_map) => {
                    let uploaded = if result.material_slots.is_empty() {
                        result
                            .material_slot
                            .map(|slot| vec![Some(slot)])
                            .unwrap_or_default()
                    } else {
                        result.material_slots.clone()
                    };
                    let slots: Vec<Option<u32>> = material_map
                        .iter()
                        .map(|index| index.and_then(|index| uploaded.get(index).copied().flatten()))
                        .collect();
                    let first = slots.iter().find_map(|slot| *slot);
                    let mut unique: Vec<u32> = slots.iter().flatten().copied().collect();
                    unique.sort_unstable();
                    unique.dedup();

                    if let Some(mr) = world.get_mut::<MaterialRef>(*entity) {
                        if let Some(slot) = first {
                            mr.material_slot = slot;
                            mr.generation = 1;
                        }
                    }
                    if !slots.is_empty() {
                        world.insert(*entity, MaterialSlots { slots });
                    }
                    if first.is_some() {
                        self.record_resolved(
                            *entity,
                            AssetSlot::Material,
                            AssetRelease {
                                material_slot: first,
                                material_slots: unique,
                                ..AssetRelease::default()
                            },
                        );
                    }
                }
                // 没有经过 `prepare_requests` 的旧式结果 / 单材质路径：保持
                // 旧行为，只写回第一个槽位。
                None => {
                    if let Some(slot) = result.material_slot {
                        if let Some(mr) = world.get_mut::<MaterialRef>(*entity) {
                            mr.material_slot = slot;
                            mr.generation = 1;
                        }
                        self.record_resolved(
                            *entity,
                            AssetSlot::Material,
                            AssetRelease {
                                material_slot: Some(slot),
                                material_slots: vec![slot],
                                ..AssetRelease::default()
                            },
                        );
                    }
                }
            }
            if let Some((shader_path, asset_id)) = self.pending_shader_meta.remove(entity) {
                if let Some(shader_handle) = result.shader_handle {
                    self.shader_paths.insert(*entity, shader_path);
                    world.insert(
                        *entity,
                        ShaderRef {
                            asset_id,
                            shader_handle,
                            generation: 1,
                        },
                    );
                }
            }
            if let (Some(slot), Some(size)) = (result.texture_slot, result.texture_size) {
                let texture_slot = self
                    .pending_texture_slot
                    .remove(entity)
                    .unwrap_or(AssetSlot::Sprite);
                self.record_resolved(
                    *entity,
                    texture_slot,
                    AssetRelease {
                        texture_slot: Some(slot),
                        ..AssetRelease::default()
                    },
                );
                let mut sprite =
                    world
                        .get::<SpriteRef>(*entity)
                        .cloned()
                        .unwrap_or_else(|| SpriteRef {
                            asset_id: crate::scene::components::SceneAssetId::generate(),
                            texture_slot: 0,
                            size: [0, 0],
                            generation: 0,
                            uv_rect: [0.0, 0.0, 1.0, 1.0],
                            trimmed_offset: [0, 0],
                            original_size: [0, 0],
                        });
                sprite.texture_slot = slot;
                sprite.generation = 1;
                if let Some(atlas_meta) = self.pending_atlas_sprites.remove(entity) {
                    sprite.uv_rect = atlas_meta.uv_rect;
                    sprite.trimmed_offset = atlas_meta.trimmed_offset;
                    sprite.original_size = atlas_meta.original_size;
                    sprite.size = atlas_meta.size;
                } else {
                    sprite.size = size;
                    sprite.uv_rect = [0.0, 0.0, 1.0, 1.0];
                    sprite.trimmed_offset = [0, 0];
                    sprite.original_size = size;
                }
                world.insert(*entity, sprite);

                // 瓦片地图的 tileset 走同一个上传结果（#529）。
                if world.get::<Tilemap>(*entity).is_some() {
                    let tilemap = world
                        .get::<TilemapRef>(*entity)
                        .cloned()
                        .unwrap_or_else(|| TilemapRef {
                            asset_id: crate::scene::components::SceneAssetId::generate(),
                            texture_slot: 0,
                            size: [0, 0],
                            generation: 0,
                        });
                    world.insert(
                        *entity,
                        TilemapRef {
                            texture_slot: slot,
                            size,
                            generation: 1,
                            ..tilemap
                        },
                    );
                }
                // UI 背景图走同一个上传结果（#591）。
                if world
                    .get::<crate::ui::components::Style>(*entity)
                    .is_some_and(|style| !style.image.is_empty())
                {
                    let image = world
                        .get::<UiImageRef>(*entity)
                        .cloned()
                        .unwrap_or_else(|| UiImageRef {
                            asset_id: crate::scene::components::SceneAssetId::generate(),
                            texture_slot: 0,
                            size: [0, 0],
                            generation: 0,
                        });
                    world.insert(
                        *entity,
                        UiImageRef {
                            texture_slot: slot,
                            size,
                            generation: 1,
                            ..image
                        },
                    );
                }
            }
            self.enqueued.remove(entity);
        }
    }

    // -----------------------------------------------------------------------
    // 热重载：让已解析的资产重新走一遍
    // -----------------------------------------------------------------------

    /// 让**所有** `MeshRenderer` / `SpriteRenderer` 的解析结果失效（`.pak` 重建后调用）。
    ///
    /// 只清实体这一侧的状态（`MeshRef` / `MaterialRef` / `SpriteRef` 的
    /// `generation`、失败缓存、排队标记），**不动** `ResourceManager`：包里的字节由
    /// [`ResourceManager::on_pak_changed`] 换掉，新字节在下一帧
    /// [`Self::prepare_requests`] 里重新读出来，渲染线程给它们新的句柄。
    ///
    /// 返回**重新排队**的实体数。旧 GPU 句柄暂时留在渲染器里（没有从 ECS 反向释放的接口），
    /// 频繁热重载会让纹理/材质槽增长 —— 已知遗留，见 #495 的后续条目。
    pub fn invalidate_assets(&mut self, world: &mut World) -> usize {
        let mut entities: Vec<Entity> = Vec::new();
        for (entity, _) in world.query::<MeshRenderer>() {
            entities.push(entity);
        }
        for (entity, _) in world.query::<SpriteRenderer>() {
            if !entities.contains(&entity) {
                entities.push(entity);
            }
        }
        for (entity, _) in world.query::<Tilemap>() {
            if !entities.contains(&entity) {
                entities.push(entity);
            }
        }
        // UI 背景图（#591）也走同一条贴图上传路径，热重载时必须一起失效。
        for (entity, _) in world.query::<crate::scene::runtime_components::UiImageRef>() {
            if !entities.contains(&entity) {
                entities.push(entity);
            }
        }
        let count = entities.len();
        for entity in entities {
            self.reset_entity_refs(world, entity);
        }
        count
    }

    /// 只让引用了 `path` 的实体重新解析（换了一张贴图 / 一个网格）。
    ///
    /// `path` 与作者文档里的写法一致（`mesh_path` / `material_path` /
    /// `sprite_path`）。返回引用了该路径的实体数；没有实体引用它时返回 0。
    pub fn invalidate_path(&mut self, world: &mut World, path: &str) -> usize {
        let mut entities: Vec<Entity> = Vec::new();
        for (entity, renderer) in world.query::<MeshRenderer>() {
            if renderer.mesh_path == path
                || renderer.material_path == path
                || renderer
                    .material_paths
                    .iter()
                    .any(|material| material == path)
            {
                entities.push(entity);
            }
        }
        for (entity, sprite) in world.query::<SpriteRenderer>() {
            if sprite.sprite_path == path && !entities.contains(&entity) {
                entities.push(entity);
            }
        }
        for (entity, tilemap) in world.query::<Tilemap>() {
            if tilemap.tileset_path == path && !entities.contains(&entity) {
                entities.push(entity);
            }
        }
        for (entity, style) in world.query::<crate::ui::components::Style>() {
            if style.image == path && !entities.contains(&entity) {
                entities.push(entity);
            }
        }
        let count = entities.len();
        for entity in entities {
            self.reset_entity_refs(world, entity);
        }
        count
    }

    /// 把实体上已解析的句柄打回 `generation == 0`，并清掉会挡住重新解析的缓存。
    ///
    /// 返回 `true` 表示确实做了修改（本来就是 `0` 的实体不算）。
    fn reset_entity_refs(&mut self, world: &mut World, entity: Entity) -> bool {
        let mut changed = false;
        if let Some(mesh) = world.get_mut::<MeshRef>(entity) {
            if mesh.generation != 0 {
                mesh.generation = 0;
                changed = true;
            }
        }
        if let Some(material) = world.get_mut::<MaterialRef>(entity) {
            if material.generation != 0 {
                material.generation = 0;
                changed = true;
            }
        }
        if let Some(sprite) = world.get_mut::<SpriteRef>(entity) {
            if sprite.generation != 0 {
                sprite.generation = 0;
                changed = true;
            }
        }
        if let Some(tileset) = world.get_mut::<TilemapRef>(entity) {
            if tileset.generation != 0 {
                tileset.generation = 0;
                changed = true;
            }
        }
        if let Some(image) = world.get_mut::<UiImageRef>(entity) {
            if image.generation != 0 {
                image.generation = 0;
                changed = true;
            }
        }
        // 失败缓存（同一个坏路径不再重试）与排队标记都会挡住重新解析。
        self.enqueued.remove(&entity);
        self.pending_bounds.remove(&entity);
        self.pending_submeshes.remove(&entity);
        self.pending_material_map.remove(&entity);
        self.tileset_paths.remove(&entity);
        self.ui_image_paths.remove(&entity);
        self.failed.retain(|(owner, _), _| *owner != entity);
        changed
    }

    // -----------------------------------------------------------------------
    // 骨骼动画资产（#560 P0-2）
    // -----------------------------------------------------------------------

    /// 把场景里引用的 `.rskel` / `.ranim` 读进 [`SkeletonLibrary`]。
    ///
    /// 需要 `&mut World`（要写资源），所以不进调度系统；调用方在每帧的
    /// `prepare_requests` 附近调用一次即可（重复调用是幂等的）。
    pub fn load_animation_assets(&mut self, world: &mut World) -> usize {
        let _span = crate::util::timing::span("asset::load_animation_assets");
        use crate::scene::systems::animator::SkeletonLibrary;

        // 蒙皮实体不一定有 `MeshRef` / `MaterialRef`（loader 只为 `MeshRenderer`
        // 插入），没有它们 `prepare_requests` 就不会给它排上传（#560 P0-3b）。
        let mut missing_refs = Vec::new();
        for (entity, renderer) in world.query::<prism_scene::components::SkinnedMeshRenderer>() {
            if renderer.skinned_mesh_path.trim().is_empty() {
                continue;
            }
            if world.get::<MeshRef>(entity).is_none() {
                missing_refs.push((entity, true));
            } else if world.get::<MaterialRef>(entity).is_none() {
                missing_refs.push((entity, false));
            }
        }
        for (entity, needs_mesh) in missing_refs {
            if needs_mesh {
                world.insert(
                    entity,
                    MeshRef {
                        asset_id: crate::scene::components::SceneAssetId::generate(),
                        render_handle: prism_render::managers::MeshHandle::default(),
                        generation: 0,
                    },
                );
            }
            if world.get::<MaterialRef>(entity).is_none() {
                world.insert(
                    entity,
                    MaterialRef {
                        asset_id: crate::scene::components::SceneAssetId::generate(),
                        material_slot: 0,
                        generation: 0,
                    },
                );
            }
        }

        let mut requested: Vec<String> = Vec::new();
        for (entity, root) in world.query::<prism_scene::components::SkeletonRoot>() {
            if !root.skeleton_path.trim().is_empty() {
                requested.push(root.skeleton_path.clone());
            }
            if let Some(animator) = world.get::<prism_scene::components::Animator>(entity) {
                let clip = if animator.clip_path.trim().is_empty() {
                    root.default_clip_path.clone()
                } else {
                    animator.clip_path.clone()
                };
                if !clip.trim().is_empty() {
                    requested.push(clip);
                }
                // 状态机（#560 P1）：机器自己的状态剪辑在它加载完之后再补请求。
                if !animator.state_machine_path.trim().is_empty() {
                    requested.push(animator.state_machine_path.clone());
                }
                // 动画图（#560 P1）：图自己的剪辑 / 状态机在加载完之后再补请求。
                if !animator.anim_graph_path.trim().is_empty() {
                    requested.push(animator.anim_graph_path.clone());
                }
            }
        }
        if requested.is_empty() {
            return 0;
        }
        requested.sort();
        requested.dedup();

        if world.get_resource::<SkeletonLibrary>().is_none() {
            world.insert_resource(SkeletonLibrary::new());
        }
        if world
            .get_resource::<crate::scene::systems::animator::StateMachineLibrary>()
            .is_none()
        {
            world.insert_resource(crate::scene::systems::animator::StateMachineLibrary::new());
        }
        if world
            .get_resource::<crate::scene::systems::animator::AnimGraphLibrary>()
            .is_none()
        {
            world.insert_resource(crate::scene::systems::animator::AnimGraphLibrary::new());
        }
        // 参数表（项目侧每帧写 `speed` / `grounded` / trigger）。引擎不猜值，只保证
        // 资源在：没有它 `animator_system` 会当成空表（所有参数条件恒 false）。
        if world.get_resource::<prism_anim::AnimParameters>().is_none() {
            world.insert_resource(prism_anim::AnimParameters::default());
        }

        let mut loaded = 0usize;
        for path in requested {
            let already = world
                .get_resource::<SkeletonLibrary>()
                .is_some_and(|library| library.has_skeleton(&path) || library.has_clip(&path))
                || world
                    .get_resource::<crate::scene::systems::animator::StateMachineLibrary>()
                    .is_some_and(|library| library.has_state_machine(&path))
                || world
                    .get_resource::<crate::scene::systems::animator::AnimGraphLibrary>()
                    .is_some_and(|library| library.has_graph(&path));
            if already {
                continue;
            }
            let Some(id) = self.resource_manager.id_by_path(&path) else {
                report_missing_reference(&path, "animation");
                continue;
            };
            // 骨架与剪辑类型不同，用扩展名区分（cook 端写出的路径带 `.rskel` / `.ranim`）。
            if path.ends_with(".rskel") {
                let Ok(handle) = self
                    .resource_manager
                    .load_with_deps::<prism_asset::runtime::SkeletonAsset>(id)
                    .map_err(|error| log::warn!("load_animation_assets: '{path}' failed: {error}"))
                else {
                    continue;
                };
                let Ok(asset) = self.resource_manager.get(handle) else {
                    continue;
                };
                let Some(skeleton) = prism_anim::Skeleton::from_info(&asset.info) else {
                    log::warn!("load_animation_assets: '{path}' is not a valid skeleton");
                    continue;
                };
                if let Some(library) = world.get_resource_mut::<SkeletonLibrary>() {
                    library.insert_skeleton(path, skeleton);
                    loaded += 1;
                }
            } else if path.ends_with(".ranim") {
                let Ok(handle) = self
                    .resource_manager
                    .load_with_deps::<prism_asset::runtime::AnimClipAsset>(id)
                    .map_err(|error| log::warn!("load_animation_assets: '{path}' failed: {error}"))
                else {
                    continue;
                };
                let Ok(asset) = self.resource_manager.get(handle) else {
                    continue;
                };
                let clip = prism_anim::Clip::from_info(&asset.info);
                if let Some(library) = world.get_resource_mut::<SkeletonLibrary>() {
                    library.insert_clip(path, clip);
                    loaded += 1;
                }
            } else if path.ends_with(".rasm") {
                let Ok(handle) = self
                    .resource_manager
                    .load_with_deps::<prism_asset::runtime::StateMachineAsset>(id)
                    .map_err(|error| log::warn!("load_animation_assets: '{path}' failed: {error}"))
                else {
                    continue;
                };
                let Ok(asset) = self.resource_manager.get(handle) else {
                    continue;
                };
                let machine = prism_anim::StateMachine::from_info(&asset.info);
                if world
                    .get_resource_mut::<crate::scene::systems::animator::StateMachineLibrary>()
                    .is_some_and(|library| library.insert(path, machine))
                {
                    loaded += 1;
                }
            } else if path.ends_with(".rag") || path.ends_with(".animgraph.json") {
                let Ok(handle) = self
                    .resource_manager
                    .load_with_deps::<prism_asset::runtime::AnimGraphAsset>(id)
                    .map_err(|error| log::warn!("load_animation_assets: '{path}' failed: {error}"))
                else {
                    continue;
                };
                let Ok(asset) = self.resource_manager.get(handle) else {
                    continue;
                };
                if world
                    .get_resource_mut::<crate::scene::systems::animator::AnimGraphLibrary>()
                    .is_some_and(|library| library.insert(path, asset.info.clone()))
                {
                    loaded += 1;
                }
            }
        }

        // 第二轮：图引用的状态机。必须先加载机器，第三轮才能把机器状态剪辑一起补全。
        let mut graph_machines: Vec<String> = Vec::new();
        if let Some(library) =
            world.get_resource::<crate::scene::systems::animator::AnimGraphLibrary>()
        {
            for (_, graph) in library.all() {
                for node in &graph.nodes {
                    if !node.state_machine_path.trim().is_empty() {
                        graph_machines.push(node.state_machine_path.clone());
                    }
                }
            }
        }
        graph_machines.sort();
        graph_machines.dedup();
        for path in graph_machines {
            let already = world
                .get_resource::<crate::scene::systems::animator::StateMachineLibrary>()
                .is_some_and(|library| library.has_state_machine(&path));
            if already {
                continue;
            }
            let Some(id) = self.resource_manager.id_by_path(&path) else {
                report_missing_reference(&path, "animation");
                continue;
            };
            let Ok(handle) = self
                .resource_manager
                .load_with_deps::<prism_asset::runtime::StateMachineAsset>(id)
                .map_err(|error| log::warn!("load_animation_assets: '{path}' failed: {error}"))
            else {
                continue;
            };
            let Ok(asset) = self.resource_manager.get(handle) else {
                continue;
            };
            let machine = prism_anim::StateMachine::from_info(&asset.info);
            if world
                .get_resource_mut::<crate::scene::systems::animator::StateMachineLibrary>()
                .is_some_and(|library| library.insert(path, machine))
            {
                loaded += 1;
            }
        }

        // 第二轮：状态机引用的剪辑。机器刚加载完才知道它们是谁，所以只能在这之后补。
        let mut state_clips: Vec<String> = Vec::new();
        if let Some(library) =
            world.get_resource::<crate::scene::systems::animator::StateMachineLibrary>()
        {
            for machine in library.all() {
                for state in &machine.states {
                    if !state.clip_path.trim().is_empty() {
                        state_clips.push(state.clip_path.clone());
                    }
                }
            }
        }
        if let Some(library) =
            world.get_resource::<crate::scene::systems::animator::AnimGraphLibrary>()
        {
            for (_, graph) in library.all() {
                for node in &graph.nodes {
                    state_clips.extend(node.clips.iter().cloned());
                }
            }
        }
        state_clips.sort();
        state_clips.dedup();
        for path in state_clips {
            let already = world
                .get_resource::<SkeletonLibrary>()
                .is_some_and(|library| library.has_clip(&path));
            if already {
                continue;
            }
            let Some(id) = self.resource_manager.id_by_path(&path) else {
                report_missing_reference(&path, "animation");
                continue;
            };
            let Ok(handle) = self
                .resource_manager
                .load_with_deps::<prism_asset::runtime::AnimClipAsset>(id)
                .map_err(|error| log::warn!("load_animation_assets: '{path}' failed: {error}"))
            else {
                continue;
            };
            let Ok(asset) = self.resource_manager.get(handle) else {
                continue;
            };
            let clip = prism_anim::Clip::from_info(&asset.info);
            if let Some(library) = world.get_resource_mut::<SkeletonLibrary>() {
                library.insert_clip(path, clip);
                loaded += 1;
            }
        }
        loaded
    }
}

impl Default for GpuAssetResolver {
    fn default() -> Self {
        Self::new()
    }
}
