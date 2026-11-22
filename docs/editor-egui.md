# egui editor (`editor/`)

Prisma's editor (egui + ash), currently the only editor.

Design goals are only two:

1. **Classic desktop editor layout**: menu + toolbar + status bar, split-tree
   docking, hierarchy / inspector / asset / document / log / history, prefab, undo, project read/write.
2. **Scene / Game view zero-copy**: engine-rendered images are directly sampled by UI, without CPU involvement.

## 1. Why egui can achieve zero-copy

egui is not a renderer: it only produces UI mesh + texture id, Vulkan device, command buffer,
swapchain are all in the host's hands. `egui-ash`'s `AppCreator::create` requires the host to create
instance / device itself and return `AshRenderState`, thus enabling the engine's model:

```text
Same VkDevice / VkQueue
├─ prism-render GraphRenderer (Shadow → Scene → GTAO → Post → offscreen image)
├─ egui-ash (UI vertex / font texture / sampled viewport image)
└─ swapchain present
```

`ImageRegistry::register_user_texture(image_view, sampler)` registers the engine's offscreen image
as `egui::TextureId`, `painter.image()` samples it in **the same command buffer**. GPU→GPU,
no readback.

## 2. Device sharing wiring

`prism-render` adds two capabilities (both in `crates/prism-render/src/context.rs`):

- `create_host_instance(entry, extensions)`: host (UI framework) holds instance itself;
- `VulkanContext::from_host(entry, instance, physical, family, extra_device_extensions)`:
  creates device on host instance with **exactly the same feature set** as window mode
  (bindless, synchronization2, timeline semaphore, device address, conditional rendering),
  but **does not own it** (`ContextOwnership::External`, not destroyed on drop, avoiding dangling handles on UI side).

`pick_host_physical_device` / `pick_present_queue_family` guarantee the selected physical device
and queue family both support graphics and present (egui-ash creates surface + swapchain per window itself).

Offscreen image side (`crates/prism-render/src/offscreen.rs` + `graph_renderer.rs`):

- `OffscreenTarget` changed from `pub(crate)` to `pub`, exposing `image_view()` / `extent()` /
  `format()`, image usage adds `SAMPLED`;
- `GraphRenderer::offscreen_new_with_context(Arc<VulkanContext>, w, h, env)`:
  builds offscreen renderer on shared device;
- `GraphRenderer::offscreen_present_for_sampling()`: after frame ends (PostPass final layout is `PRESENT_SRC_KHR`),
  inserts `PRESENT_SRC_KHR → SHADER_READ_ONLY_OPTIMAL` barrier and waits for completion, then image can be sampled by egui;
- `offscreen_image_view()`: `VkImageView` for texture registration.

Next frame's PostPass uses `initial_layout = UNDEFINED` (load op clear), so transitioning from
`SHADER_READ_ONLY_OPTIMAL` back to color attachment is valid, no extra state machine needed.

## 3. Frame pacing

The editor **does not create a render thread** (unlike `prism-app`'s hosted mode): `Engine` +
`GraphRenderer` run directly on egui-ash's event loop thread, `App::ui()` synchronously produces frames.

- Asset upload is also synchronous: `GpuAssetResolver::prepare_requests` → `GraphRenderer::
  apply_asset_requests` → `apply_responses` writes back to ECS (originally split across two threads);
- Viewport re-renders only when "document changed / camera changed / selection changed / resize / Play / first frame",
  zero GPU work when idle. Resize detection must use `GraphRenderer::offscreen_extent()`
  (`extent()` is swapchain's, constantly `0x0` when offscreen; comparing it causes viewport to
  rebuild offscreen target and re-render **every frame**, wasting ~15 ms/frame GPU);
- Both engines (Scene / Game) are built at startup: first Game view runs one IBL prefiltering
  (~1.5 s) + pipeline creation, deferred to first tab switch, otherwise manifests as "tab switch freezes for a long time";
- During Play, Game view `tick()` + re-renders every frame; Scene view remains in edit mode.

**Loose asset hot reload (#495)**: Preferences'
`Rebuild scene pack when loose assets change` (default on) scans `assets/` every 1s for
**unpacked** authoring assets (textures / models / HDR / fonts / materials / `scenes.json`...),
excluding `.pak` / `.pak.meta.json` / `.rscn` / `.spv` and editor-saved `.scene.json` /
`.prefab.json`. After detecting changes, waits 800ms quiet period (continuous saves / large file chunked writes
won't trigger multiple times), then triggers existing `Build Scene Pack`; after packing,
`.pak` polling chain reopens pack and `invalidate_assets`; **new assets visible directly during Play**,
no need to exit and re-enter. No new tasks queued during build; build end re-aligns snapshot.
Detection uses `prism_editor_core::project::FileStamp` (**mtime + length**), shares the same version stamp
with external document change detection (`Workspace::check_external_changes`, triggered by focus change),
no longer writes separately; coarse-grained filesystem overwrite within the same second can still be caught by length change.

**Play mode visual cue (#500)**: during playback, entire window bordered in green, amber when paused;
status bar `PLAYING` / `PAUSED` with matching color background. Inspector top shows banner
`Play mode — changes made now are discarded when Play stops.` — edits during runtime still write to
authoring document, but `Workspace::stop()` overwrites with snapshot taken before entering Play (document,
undo stack, dirty flags, selection), so after Stop these changes are not retained.

## 3.1 Fonts and icons

- Body font prefers system UI sans-serif (Segoe UI / Liberation Sans / Noto Sans),
  falls back to egui built-in font if not found; CJK font added as fallback at the end.
- Icons use **Phosphor Icons** (`egui-phosphor 0.11`, MIT OR Apache-2.0, corresponding to egui
  0.33): `egui_phosphor::regular::{PLAY, PAUSE, EYE, FOLDER, FLOPPY_DISK, ...}`
  are ordinary text glyphs, so transport controls / hierarchy eye / asset folder / close button
  no longer rely on Unicode symbols (`▶ ⏸ ✕ ⧉` which lack glyphs in most UI fonts, showing as boxes).

## 3.2 User project code (Game View Play)

Game View Play executes **user project's own gameplay code**, not just engine built-in systems:

- Editor compiles project manifest's `preview-module` feature into cdylib
  (`cargo build --no-default-features --features preview-module --target-dir
  <project>/target/preview-module`), loaded by `prism-runtime-host` with same build ABI.
  Project module **does not link engine**: `prism-game-api` only opens `plugin` backend.
- Source changes auto-rebuild after **350 ms** settle; Play does not auto-swap module (avoid interrupting playtest),
  `Ctrl+R` / File → Compile Project Module forces rebuild. Compile failure keeps last successful
  module running, diagnostics go to Console and Problems panel.
- **Reload is whole-module replacement, not hot reload**: old session released, module `create` re-runs, project-side
  static state reset. No script layer to migrate state, so copy uniformly uses "reload", not
  "hot reload / domain reload".
- Toolbar right shows module status (`ready` / `changes not compiled` / `compiling…` /
  `compile failed`) and `Compile (Ctrl+R)` button. Source timestamp later than last compile attempt is
  `pending`, Auto Refresh off still visible.
- **Problems panel** (Window → General → Problems) lists
  `cargo --message-format=json` errors / warnings, with
  `file:line:col`; click a line opens with default editor (tries `code -g`, falls back to
  system default handler). Warnings retained on successful compile; on failure first error also goes to Console.
  Panel has two segments: **Project module** (Game View's `preview-module`) and
  **Player build** (`File → Build Player` / `Build And Run`'s `cargo build`,
  with `--message-format`).
- Before Play if module loaded but source has uncompiled changes (Auto Refresh off, or last build blocked by Play),
  compiles once first, avoiding playtest on stale code.
- Each Play frame first `ProjectSession::tick(WorldCommands)`, then engine systems: project-written
  components and `draw_quad`-submitted UI quads visible in same frame by hierarchy / `ui::render`.
- Project module reads **project's own** `assets/` via `AssetIo` (not repo root engine assets).
- Input: during Play, Game view forwards keyboard/mouse to engine `InputManager`, `InputSnapshot.keys`
  is **hold-pressed** semantics (fishing's `input.key(SPACE)` needs continuous reeling).
- Projects without `preview-module` feature (any not-yet-converted user project) only run engine, and
  explains reason in Console; `PRISMA_PROJECT_MODULE=/path/libgame.so` can directly load
  prebuilt module (testing / non-Cargo projects).
- `projects/template` and `projects/fishing` `preview-module` must explicitly open
  `prism-scene`'s three component groups (3D / 2D / UI): module is independent workspace, doesn't get engine-side
  feature unification; missing one group rejected at `ProjectModule::load` schema fingerprint check.
  (Measured: `default-features = false` `prism-scene` registers only core components,
  fingerprint 0x758a… vs host 0x03b2…, load directly fails.)
- **Symbol files follow dll**: build artifacts copied to `staged/<stem>-<nanos>/<original filename>`,
  `.pdb` placed in same directory. Uniqueness only in **directory** — dbghelp looks for symbols
  by PE-recorded `fishing.pdb` in module directory; renaming dll to `fishing-<nanos>.dll` degrades call stack
  project frames to export names (`prism_project_get_api`).
- **Module logs and panic bridged**: module is independent dylib, brings its own `log` and `std`, host's
  logger / panic hook can't see it. Project entry generated by `prism_game_api::export_project!`:
  calls `install_host_logger` + `install_host_panic_hook` in `create`,
  validates ABI and schema fingerprint, rebuilds project state on scene generation change; afterwards
  `log::info!` and `panicked at …` + `Backtrace` go through `HostApi::log` to Console
  (target `project`, source `Game`). Panic itself caught by `prism_project_api::guard()` at
  ABI boundary (cannot unwind across `extern "C"`).
- **tick failure latches**: module returns error (including panic), stops being called until recompile / reload.
  Otherwise a per-frame panicking project spams stderr 60 times/sec and slows GPU.
- **Game view render defaults come from project**: right after `EngineHost::new`, project's
  `assets/settings.ini [render]` overrides (`apply_project_render_defaults`, same as player launch
  `AppConfig.render → RenderSettings`), render settings panel only applies to Scene view —
  otherwise "what editor sees" and "packaged output" are not the same image.
- **2D pixel art uses `tonemap_mode = 2`**: Reinhard / ACES alters art palette; passthrough
  only does `saturate`, combined with sRGB swapchain can 1:1 restore source image. `projects/fishing` defaults on.
  UI color written as sRGB author value, decoded to linear at vertex expansion, finally encoded once by swapchain (#592).

## 4. Interaction

- Orbit: left/right drag → `OrbitCamera::orbit`; scroll → `zoom`;
- Pick: screen coordinates → ray → intersect with entity's real local bounding box (mesh uses cooked AABB, sprite uses
  `frame_size / pixels_per_unit` + pivot; falls back to unit cube when no data), index is
  `.scene.json`'s `entities[i]` (#600);
- Translate gizmo: after `pick_gizmo_axis` hits axis, compute world displacement by "screen displacement · axis screen direction / pixels per world unit",
  handed to `Workspace::translate_entity_world`;
- Selection highlight reuses engine's `Gizmo` component (world tri-color axis arrows), no new rendering concept.
- **Pick loop-through (#416)**: when clicking same position continuously, sequentially select next layer occluded
  (candidate set sorted by ray distance near to far, within 4px counts as same pick; change position / candidate set returns to nearest).
  Shift / Ctrl add / remove selection does not loop, always takes nearest.
- **Direction gizmo (#416)**: Scene view top-right 6 world axis handles (camera-facing one dimmer),
  click to align camera to that axis; `OrbitCamera::align_to_axis` directly solves yaw / pitch,
  orthographic view also works.
- **Multi-select gizmo and pivot mode (#416)**: Move / Rotate / Scale drag applies to **entire selection set**
  (each drag is still one undo step). Toolbar `Pivot` off is **Center** mode: gizmo at selection center,
  rotate / scale moves whole group (orbit around center / push by pivot scale); on is **Pivot** mode: gizmo at active object's own pivot,
  each object only modifies its own transform.
- **View… menu (#416)**: Scene view toolbar can adjust editor camera FOV (20–110°) and
  `near` / `far`, and toggle **camera icon / light icon** separately. FOV and clip planes written to both engine camera and
  pick ray (`CameraFrame::fov_y_degrees`), gizmo, pick and image won't misalign;
  `2D` toggle remains orthographic XY view.
- **Viewport floating toolbar (#421)**: Scene view top-left (8px) has a semi-transparent toolbar,
  no need to move gaze to top toolbar: `Grid` (`G`), `Sky` (`S`), `2D` (orthographic XY),
  `Snap` toggle and translate snap step (`Δ`, world units, 0.01–100). Left arrow folds into a small square;
  fold state exists only in `UiState`, not in document. It is this issue's suggested **minimal instance**,
  not Unity `OverlayCanvas` draggable docking system.
- Tools: `W` / `E` / `R` switch move / rotate / scale gizmo, `X` toggles world/local, `G` toggles grid;
  `S` toggles Scene view background (scene skybox / solid color, same semantics as engine's
  `SceneViewState.showSkybox`, default on);
  rotation uses `pick_rotation_axis` + `screen_angle`, scale uses `set_scale_axis`.
- Blank drag = box select (`box_select`), `Alt`+drag / right drag = orbit, double click = focus selection.

## 4.1 Log

`logging.rs` wraps `env_logger` with a tee: the same `log` record goes both to terminal (`RUST_LOG`
semantics unchanged) and to `prism-editor-core`'s `LogBuffer`, so Console panel can see
warnings from `prism_render` / `prism_engine` in viewport (same idea as engine logs). `Trace` is not forwarded,
to avoid flooding the window.

**Console's four tool toggles (#421)**: `Collapse` merges identical `(level, source, text)` into one line
and shows `×N` (keeping first occurrence's time order); `Clear on Play` clears at Play start;
`Error Pause` auto-pauses on first Error / Panic during Play (and writes a line explaining, to avoid
looking like a freeze); log lines with `path:line[:col]` **double-click** jumps to that location with default editor
(same `open_source_location` set as Problems panel).

**Play auto-maximize (#421)**: `Window → General → Maximize Game View on Play` turned on,
pressing Play auto-maximizes Game view, Stop restores pre-entry layout (if no panel maximized before entry,
also returns to "none maximized", won't swallow panels user maximized themselves).

## 4.2 Menu (structure)

Menus organized by engine grouping, two/three-level menus are all real commands:

| Menu | Two/three-level |
|---|---|
| File | New Scene / New Scene From Template ▸ / New Prefab / Open / Recent Scenes ▸ / Save / Save As / Save Project / Reload / Build Settings / Build Scene Pack / Cook Active Scene / Build Player / Build And Run / Compile Project Module / Auto Refresh Project Module / Clean Build / Exit |
| Edit | Undo / Redo / Cut / Copy / Paste / Duplicate / Delete / Select All / Deselect All / Invert Selection / Select Children / Select Prefab Root / Next·Previous Selection / Copy·Paste Transform ▸ / Paste Component / Play / Pause / Step / Project Settings / Preferences / Theme |
| Assets | Create ▸ (Scene / Prefab / **Prefab Variant…** / Folder…) / Open / Show in Explorer / Copy Path / Duplicate / Delete / Instantiate In Scene / Find References In Scene / Find References In Project / Refresh |
| GameObject | Create Empty / Create Empty Child / Create Empty Parent / Create Other ▸ (Rendering ▸ Mesh Renderer·Skybox / Light ▸ Directional·Point·Spot / Camera / UI) / Move To View / Align View to Selected / Snap to Ground |
| Component | schema category ▸ component list |
| Window | General ▸ panel toggles / Layouts ▸ presets + named layouts (Load/Delete) + Save Layout… + Save·Load Layout to File… + Reset All Layouts / Reset Layout |
| Help | Shortcuts / Editor Guide / About |

**Player build (`Build Player` / `Ctrl+B` = `Build And Run`)** follows `assets/settings.ini`'s
`[build]` section: `profile` chooses `debug` / `release` (`cargo build [--release]`), then
`prism-build-pipeline build-scenes` refreshes `scenes.pak`, then places executable at
`<output_dir>/<exe>`, artifacts at `<output_dir>/assets/{scenes.pak,settings.ini}`
(engine only recognizes `<root>/assets/`, placing flat at `output_dir/` root double-click reports "no assets directory").
`Build And Run` runs this **placed artifact**, not `cargo run`'s debug build;
non-zero exit or missing artifacts show `build failed:` on status bar, full stderr stays in Console.

`Build*` / `Cook Active Scene` / `Clean Build` invoke `prism-build-pipeline`
or `cargo` via subprocess (editor still does not link cooker), output to Console.

Shortcuts (common habits): `Ctrl+1..5` = Scene / Game / Inspector / Hierarchy / Project,
`Ctrl+Shift+C` = Console, `Ctrl+B` = Build And Run, `Ctrl+R` = rebuild and load user project module,
`Ctrl+I` = deselect,
`W/E/R` = move / rotate / scale, `X` world/local, `G` grid, `S` skybox, `F` frame all, `Del` delete.

**Rebindable shortcuts (#421)**: `Help → Shortcuts` panel **click a key** then press new combo to rebind
(Esc / Cancel cancels, row-end `Reset` resets single one, top `Reset all` clears all). During capture,
`handle_shortcuts` yields entirely, keys won't trigger commands along the way; conflicts with other commands reject change and hint occupied command in panel.
Override table stored in `Preferences::shortcut_overrides` (persisted with editor state
`target/editor-egui/state.json`, key is `Command::id()`), menu hints and actual dispatch read the same `effective_bindings` table,
after override old alias keys no longer take effect.

Named layouts exist in `target/editor-egui/layouts.json` (`{name: {version,dock,floating}}`),
can export/import whole as JSON file (same shape as Web's `prisma-editor.dock.v4`).

`Create → Prefab Variant…`: takes selected instance stored via `extract_subtree_preserving_links` as
"inherit base + override" prefab file, then repoints this scene instance to new variant (behavior).

## 4.3 Panel capabilities

- **Hierarchy**: search, engine-style drag (insert at top/bottom 25%, attach as child in middle), double-click to focus (frame selection),
  inline rename (`F2`, `Return` on macOS),
  show/hide, copy / delete / reparent, Create dropdown;
- **Inspector**: single + multi-select (public components batch-edit fields, one undo; multi-select the last clicked /
  right-clicked object is Active, batch Transform shows its value, #507); prefab block (Revert /
  Sync All / Save as Prefab…); component card header copy / remove; Add Component category menu;
  **List field editable (#418)**: `Vec<T>` one row per item, with `▲` / `▼` / `✕`, bottom
  `+ Add` generates default value by element schema (`Vec3` → `[0,0,0]`, nested struct → each field default);
  fixed-length arrays do not provide add/remove; element interiors still use their real controls, path is `field[i]` (`LineRenderer.points`
  such fields no longer need hand-editing JSON). `Option<T>` uses checkbox to control presence, nested struct collapsible.
  **Component preset (#418)**: component card header `…` → `Presets` can save current component value as named preset
  (same name overwrites), then apply to any entity (whole component = one undo step). Presets persisted with editor state
  (`target/editor-egui/state.json`), grouped by full reflect name; `Reset` still restores schema default.
  Component right-click menu has Copy / Paste Values / Move Up / Down / Remove, field right-click can
  Copy / Paste Value and Copy Property Path; inspector top-right lock pins content by **entity id**.
  **Component card header `…` menu**: Copy Component / Paste Values / Move Up·Down / Reset (writes back schema `defaultJson`, one undo step) / Remove;
  **field right-click**: Copy Value / Paste Value (only same-shaped values paste, no type coercion) / Copy Property Path. Panel top lock icon is
  **Inspector Lock**: pins current object by entity id, clicking elsewhere doesn't change content, auto-unlocks when object deleted.
  Components without schema (user project module's own types etc.) show **editable raw JSON**:
  Apply JSON / Reload, parse failure disables button and shows error inline — no need to hand-edit scene file.
  Transform row numeric box right-click has **Copy / Paste / Reset** (whole row; Position and Scale clipboards separate,
  Reset back to 0 / unit quaternion / 1).
- **Project (asset)**: back / forward / breadcrumb, directory browsing, type filter, search, rename /
  copy / delete / copy path / instantiate prefab; **directory row** selectable like file (single click select,
  double click enter), right-click has Open / Rename / New Subfolder / Copy Path / Show in Explorer /
  Delete (only empty directory), `F2` inline rename; **asset drag-and-drop**: drag to directory = move and rewrite references,
  drag to hierarchy entity = instantiate prefab, drag to inspector path field = directly write path (`AssetDrag` payload).
- **Project panel view / search syntax / Ping (#419)**: search box right has three toggles — list /
  grid view, single column / two columns (left directory tree + right content). Search supports Unity-style `t:` prefix:
  `t:texture` (English slug / `Texture` / Chinese label `纹理` all recognized), `dir:sprites`,
  multiple words are AND, multiple `t:` are OR. After selecting asset, inspector shows asset details (type / size /
  GUID / reference count / preview), bottom **Ping In Scene** and right-click menu's `Ping In Scene` will
  select first entity referencing the asset and frame it in Scene view — asset ↔ scene bidirectional positioning.
- **Asset identity (#466 step 1)**: when opening or refreshing project, editor generates `<path>.rmeta`
  (`{ "guid": "<32 hex>", "importer": "..." }`) for each asset under `assets/` still missing sidecar;
  existing valid GUID never rewritten, corrupted treated as missing. Scan skips `.rmeta` (not an importable
  asset), asset details GUID prioritizes sidecar value, falls back to old path hash if none;
  rename / move file sidecar follows, delete removes together. `.scene.json` / `.prefab.json`
  now writes both path and `*_guid`: when opening document GUID prioritizes resolving to path (falls back to path if unresolvable),
  on save sidecar supplements / refreshes GUID; cooker and dev-time JSON loader follow the
  same rule, old documents need no migration. On rename, GUID-bearing references **no longer rewrite document** (read
  resolves to new path via sidecar); only old GUID-less path references still take compatible rewrite. Reference analysis /
  delete confirmation / Find References / Ping / hierarchy tree broken-reference marking also match by GUID, stale path
  won't miss references nor be false-reported as broken links.
- **Texture thumbnail (#982)**: Project panel list rows and grid cells show texture itself, asset inspector preview
  draws texture. Decode / proportionally scale to 64 px in `prism-editor-core::thumbnails`
  (PNG, `image` crate, pure data layer), egui side uploads as `TextureHandle` and caches on first use;
  cache invalidated by `FileStamp` (mtime + length), thumbnail auto-updates after reimporting texture.
  At most 12 decoded per frame (`UiState::thumbnail_budget`), 600-sprite directory won't freeze panel;
  decode failure / unsupported format only shows type icon, no repeated retries.
- **Texture import settings (#514)**: asset inspector when selecting texture adds `Import Settings`
  (`Max Size` 0 = unlimited, `Mips` toggle). `Apply` writes sidecar file
  `<asset>.import.json` (e.g. `assets/sprites/boat.png.import.json`), `Revert` re-reads from disk,
  `Reimport` directly repacks scene pack (`BuildKind::ScenePack`, no need to switch to Build panel first).
  Cooker reads the same JSON when baking texture: `maxSize` shrinks by **area average** (keeps aspect ratio, shrink only no upscale),
  `generateMips` controls whether to generate mip chain; missing fields follow profile.
- **Problems**: project module and player build error / warning list (`file:line:col`),
  click a line to open source; toolbar module status shares source with first segment, `Ctrl+R` / `Compile` recompiles.
- **Docking**: dragging tabs (behavior): drag to another leaf's left/right/top/bottom 25% = split,
  middle = merge into tabs, drag out of panel area = floating window, drag to this leaf edge = split in place.
- **Prefabs**: instance block has **Open Prefab** / Revert / Sync All / **Apply to Prefab**
  (current subtree overwrites prefab file, then syncs same-source instances) / Save as Prefab… (no link).
  **Open Prefab** leaves breadcrumb in toolbar `← level.scene.json / crate.prefab.json`;
  click `←` returns to scene that opened it and selects original instance. Prefab document **clean** re-instantiates that entity
  (syncs prefab changes, entity id changes, selection still on instance), **dirty** only switches back to scene and selects,
  doesn't sync unsaved changes out.
- **Viewport drag-and-drop**: prefab dragged into Scene / Game viewport → instantiated and placed at cursor's ground point
  (ray ∩ `y = 0`); `.scene.json` dragged into viewport = open that scene document.
- **Disk change protection**: when window regains focus, compares mtime of each open document (immediately detects after `git checkout`);
  modified document marked ⚠ in Documents panel; save is blocked and pops
  "Overwrite / Reload from Disk / Cancel", no silent overwrite of others' version.
  Asset add/delete also re-scanned at the same focus change.
- **Render Graph**: Scene view first frame (and every 60 frames) exports current graph to
  `<project>/target/prism-render-graph.json`, panel reads it directly — no environment variable needed;
  game process wanting same report sets `PRISMA_RENDER_GRAPH_REPORT=<path>`.
  Panel also shows previous frame total GPU time and per-pass table sorted by cost (#518; device not reporting timestamps hides whole segment, no CPU wall clock substitution),
  plus **texture / mesh / material residency** and estimated bytes
  (#518 Phase 2, data from renderer manager; old report without that field also hidden).
  Same segment also has **Draw calls** list (#518 Phase 2 data layer): lists forward meshes one by one
  (mesh index · material slot), sprite batches merged by blend state, particle billboard batches, with instance count,
  triangle count and **State** column (render queue / cull / z-write / blend, sprite shows actual blend state);
  culled / shadow-only meshes marked with weakened italics. Read-only display, no pipeline / descriptor capture and replay.
- **Resolution scale**: render graph panel's `Viewport res` (0.5×–1.5×) scales only offscreen target,
  doesn't change UI points layout, a compromise for high-DPI screens / weak GPU.
- **Project-built fonts (UI text)**: place `.ttf` / `.otf` / `.ttc` into `assets/fonts/`,
  fill relative path in `Text` component's `font` field (inspector has asset selector); empty = system default font.
  Font is normal asset: `AssetType::Font`, as-is into `.pak`, references rewritten on rename,
  runtime `GpuAssetResolver::prepare_ui_fonts` reads bytes and registers into UI atlas
  (multiple fonts share one R8 atlas, still only one draw call, #583).
- **UI button (#528)**: add `prism_engine::ui::Button` to `Node + Style` element
  (Add Component is just `Button`) to get clickable button: `normal` / `hover` / `pressed` /
  `disabled` four colors auto-switched by input system, `interactable = false` only shows disabled color, no click response.
  Click semantics consistent with Unity `Clickable`: pressed inside element and **released inside same element** sets
  `clicked`; after click button gains keyboard focus, `Enter` / `Space` also submits. `Interaction`
  is now serializable component, scene / prefab can directly save.
- **UI background image (#591)**: `Node + Style` add `"image": "textures/icon.png"` draws texture in rectangle
  (full stretch, nearest-neighbor sampling), `background` changed to tint; no tint shows image original color with opaque white.
  Image walks scene-shared bindless texture table, no new pipeline; `Button`'s interaction color also tints icon,
  so same icon brightens/darkens with button. Image path and sprite similarly resolved by `GpuAssetResolver` into `.pak` (`UiImageRef`),
  asset rename rewrites references.
- **UI layout group and text wrap (#527)**: `Style.layout` lets container arrange children by direction
  (`direction = None` still old anchor positioning):
  - `spacing` adjacent spacing, `padding` content area padding (left/right/top/bottom);
  - `child_align` controls children alignment on **cross axis** (`Start` / `Center` / `End` /
    `Stretch`), main axis advances by arrangement order;
  - `fit_content` lets container shrink to content size on axes without `width` / `height`;
  - Layout group's children positioned by cursor, manually written anchor/pivot ignored; non-layout-group nodes unchanged.
  Inspector above `Style` has dedicated **Layout Group** edit block (direction dropdown, spacing, four-side
  padding, cross-axis alignment, fit content), writes via `set_field` dot path, so undo /
  dirty / reload semantics same as other fields.
  `Text.wrap = true` wraps text by rectangle width **per UAX #14 break points** (spaces, hyphens,
  CJK between characters can break, `，` / `。` terminal punctuation won't land at line start; single word too long breaks by char),
  `\n` always wraps and keeps blank lines; lines beyond bottom not drawn.
- **UI Canvas scale (#463)**: add `prism_engine::ui::Canvas` to UI root node
  (`reference_resolution` / `scale_mode` / `match_width_or_height`), whole subtree
  scales proportionally "design resolution → actual screen": layout computed at virtual resolution,
  multiplied back when writing `ComputedLayout.rect`, font size and corner radius also scaled. Three modes correspond to
  Unity `CanvasScaler`: `ConstantPixelSize` (default, 1 screen pixel = 1 design pixel),
  `ScaleWithScreenSize` (aspect ratio interpolated in log space by `match`),
  `MatchWidthOrHeight` (only width or height used). Nested Canvas inherits only nearest layer.
  Thus 1920×1080 design composes consistently on 720p and 4K, Game view selecting target resolution sees real-device proportion.
- **Game view resolution / aspect ratio**: viewport top-left dropdown. Default reads project
  `assets/settings.ini [window]` `width`/`height` (e.g. 1600×900),
  renders offscreen at that resolution and letterboxes proportionally, so editor composition matches packaged product.
  Can also choose fixed resolution (1920×1080 etc.) or "aspect-only lock" `16:9 (fit)` /
  `4:3 (fit)` etc.; `Match Window` keeps old panel-scaling behavior. Control channel uses
  `resolution:1600x900` / `resolution:fit:16x9` / `resolution:match`.
- **Internal resolution / integer scale (#613)**: project `assets/settings.ini [render]`'s
  `internal_resolution = 640x360` + `internal_scale_mode = integer` renders scene at 640×360,
  then integer-scales up to window (centered letterbox), camera projects by internal aspect ratio — so art pixels are
  equal-sized squares. Game view and packaged product follow same config, `off` (default) keeps rendering at window resolution.
- **2D grid**: Scene view switching to 2D / orthographic camera draws reference grid on `z = 0` XY plane
  (X red, Y green), not 3D XZ ground; position and line width use original 1m / 10m grading (#602).
- **Tilemap (`prism_engine::scene::Tilemap`)**: one entity manages whole grid. `tiles` row-major,
  `0` = empty, `n > 0` = tileset's `n - 1` block; `columns = 0` single row; row 0 at top.
  Engine expands non-empty cells into `SpriteDrawItem`, shares instance buffer / bindless atlas with sprites (hundreds of cells still one
  draw call), and culls per-cell by main camera frustum (#529). Inspector below Transform has grid editor:
  `Columns` / `Rows` set scale, `Palette` determines left-click cycle range, left-click increments, right-click clears single cell.
- **2D lighting (`prism_engine::scene::Light2D`, #611)**: sprites not lit by default; add a
  `Light2D` entity to scene (`kind` = `Point` / `Ambient` / `Directional`), then check `SpriteRenderer.receive_lights`
  for sprites to be lit. Intensity is art unit (`1.0` = original color), point light attenuates by
  world XY distance (`radius = 0` no attenuation), ambient light suits night scene minimum brightness. 3D's
  `PointLight` / `SpotLight` / `DirectionalLight` don't affect 2D; scene with only 3D light prints
  one hint. `Light2D` goes through reflection registration, directly visible in Add Component.
- **Sprite clip (`SpriteRenderer.clip_enabled` / `clip_rect`)**: checking clips per-pixel by **world space**
  rectangle `[x0, y0, x1, y1]` (fragment `discard`, doesn't break 2D single draw call).
  Corner order arbitrary, engine sorts itself; closed / non-finite / zero area treated as no clip, so just checking
  switch sprite won't disappear. Multiple sprites filling same rectangle = Unity `SpriteMask` rectangle mode,
  lake reflection "only below waterline" is `clip_rect = [-1e4, -1e4, 1e4, waterline y]` (#582).
- **Multi-select drag**: drag any row of multi-selection and drop in row's middle area = `reparent_selection` whole group changes parent.
- **Asset selector**: inspector path field right has `⋯` button, filters extension by field name
  (`mesh_path` → model, `sprite_path` → texture, `hdr_path`/`env_map` → HDR…),
  selected relative path directly written to field.

### 4.3.1 Skeletal animation (#560 P0)

Cook stage bakes glTF's `skins` / `JOINTS_0` / `WEIGHTS_0` / `animations` into three side assets
(`{mesh}.skinning/skeleton.rskel`, `mesh.rskin`, `clips/{n}_{name}.ranim`), and automatically adds
`SkinnedMeshRenderer` + `SkeletonRoot` + `Animator` to scene entities (author-written components take priority,
only skeleton without weights keeps normal `MeshRenderer`). Three-piece set:

- **`Animator`**: inspector shows "▶/⏸, ⏮ replay, timeline slider, Loop, speed"; slider below has 6px
  tick strip marking all tracks' key times (`Clip::key_times` merged deduplicated), dragging shows where changes occur.
  Change time / check Loop directly writes back component field (one undo step), engine's `animator::pose` system next frame
  recomputes bone palette by new time. Slider below also has Notify strip (cyan small triangles): clip's keyframe events
  (e.g. `Footstep.L` from `{mesh}.skinning/notifies.json`) drawn on axis by time, hover shows name,
  at a glance see which seconds this animation triggers events. **`blend_duration`** (seconds, default 0.2) is runtime clip-switch cross-fade duration:
  **fade-out source is snapshot of previous frame's actually displayed pose** (not "old clip + time"), so clip switch,
  entering/exiting blend space, even old clip deleted / hot-reloaded, can all fade out normally instead of hard jump; `0` means hard cut.
  Weight reaching 1 discards snapshot.
- **Additive layer**: `Animator.additive_clip_path` + `additive_weight` (default 1). Additive clip's local transform
  relative to **reference (bind) pose** computes difference, then applied to final pose by weight (rotation `slerp`
  increment, translation adds difference, scale multiplies ratio; reference scale 0 treated as 1). So breathing / aim offset / hit
  shake etc. clips only changing some bones don't need another base clip, uncovered joints stay unchanged. Applied **after**
  cross-fade — additive layer doesn't participate in transition between two base clips. Wrong path or weight 0 / NaN silently
  skipped, won't corrupt pose.
- **Root motion recording (#560 P0)**: animation system each frame writes root joint (joint 0) **local** translation
  relative to previous frame and yaw around Y into `AnimatorPlayhead.root_motion` (`RootMotionDelta`, meters / radians, shortest path);
  first frame, clip switch / playback content switch, LOD-frozen frames are zero. Default only records; enabling
  `root_motion_consume` rotates root **translation** to world space by entity orientation and adds to `LocalTransform.position`,
  and clears root translation in pose, avoids double displacement (root rotation still only recorded, left to CharacterMovement unified handling).
- **Animation LOD (`lod_enabled` / `lod_near` / `lod_far`, #560 P6)**: default off. Enabled, entity farther than
  `lod_far` meters from **active camera** (default 50, non-finite or <= 0 treated as 50), animation system this frame
  does not advance at all — time, state machine transitions, graph evaluator and Montage all frozen at current pose,
  resumes when back in range, no catch-up during freeze. `lod_near..lod_far` (default 25..50) between evaluates every other frame:
  update frame advances `2 * dt`, so playback speed unchanged; `lod_near` non-finite / <= 0 / greater than `lod_far` means no throttle tier.
  Camera comes from first `enabled` `Camera` in scene (reads `LocalToWorld`, falls back to `LocalTransform`),
  no camera means LOD not effective (editor preview won't stop because camera not placed).
  Enabling `lod_root_only`, single-clip entities beyond `lod_far` no longer freeze entirely: time still advances,
  only root joint sampled, other bones keep previous frame pose (first entering tier fully evaluates one frame, avoids T-pose pop),
  root motion consumption still effective. `lod_bones` non-empty is same tier's bone subset version: root joint + listed bones
  sampled each frame, others keep previous frame; names resolved by skeleton, not found ignored, root joint always updated. State machine /
  blend space / animation graph entities because need full evaluator advance, still freeze entirely.
- **`SkeletonRoot`**: inspector draws bone tree (`CollapsingHeader` indented by parent chain, cycle guard).
- **Blend parameter smoothing (`blend_smoothing`, #560 P1)**: default `0`, directly uses game-written
  `blend_parameter` / `blend_parameter_y`; greater than 0 approaches target with exponential `1 - exp(-dt / tau)`,
  `tau` is that field (seconds). First frame directly lands on target; non-finite parameter or smoothing off falls back to passthrough.
  Only affects blend space sampling, doesn't change component parameter values (game side still sees its own numbers).
- **1D blend space**: `Animator`'s `blend_clip_paths` / `blend_positions` / `blend_parameter`
  is one parameter axis (speed 0 → walk → run etc.). With >= 2 clips and `blend_positions` one-to-one,
  pose obtained by interpolating `blend_parameter` on axis, **priority over** `clip_path`; parameter out of range clamped to ends.
  Clips of different lengths aligned by **normalized phase** (stride alignment), walk ↔ run won't have two feet chasing each other.
  Missing one sample (path wrong / asset not loaded) falls back to single clip, won't freeze whole character. All three fields are `Vec` / numbers,
  directly editable in inspector; `blend_clip_paths` in asset rename list. Parameter axis drawn in inspector as 16px strip:
  gray line each sample's position, gold dot **current parameter**, click or drag directly changes
  `blend_parameter` (one undo step), dragging at same time scene character changes — only place to see "now between which two samples".
- **2D blend space**: `blend_2d_clip_paths` / `blend_2d_positions` (`[x, y]` one-to-one with clips) /
  `blend_parameter` / `blend_parameter_y` form parameter plane (x usually speed, y direction). At least 3 samples non-collinear,
  pose computed by Delaunay triangulation + barycentric interpolation, **priority over 1D blend space and `clip_path**;
  parameter outside sample convex hull negative weights clamped to 0 then normalized (projected to nearest boundary), won't snap character to a sample.
  Insufficient samples / collinear / non-finite coordinates quietly fall back to 1D or single clip. Inspector draws 120px parameter plane:
  gray dots samples, gold dot current parameter, click / drag directly changes `blend_parameter` + `blend_parameter_y`
  (one undo step). `blend_2d_clip_paths` in asset rename list.
- **Viewport overlay**: toolbar `Bones` (right of `Outline`) controls, default on. When skinned entity selected,
  restores joint world positions by `BonePalette` (`world = palette × inverse_bind⁻¹`), draws parent → child 1.6px
  lines + joint dots (root joint bigger). Dragging timeline overlay moves with it, because it reads animation system's current frame result,
  not authoring data. `show_skeleton` and `show_selection_outline` both stored in `state.json`'s `scene` section,
  old archives default on.

**Weight heatmap**: render mode dropdown (next to toolbar `Shaded`) selects `Skin Weights`, equivalent to
`debug_flags = 1 << 15` (in game it's `Shift+6`). Vertex stage computes hue by **dominant bone** (gold angle,
adjacent bones don't collide), brightness by that bone's weight, fragment stage outputs as-is without lighting,
so at a glance see "which bone this mesh hangs on" and whether weight transitions evenly; vertices with all-zero weight
(bad import, static mesh) are mid-gray, won't be misread as some bone's color. This bit is not a PBR component, not involved in 15-bit isolate logic.

Timeline ticks **are clickable**: clicking a tick (hit radius 4px, nearest one) jumps `time` to that keyframe;
hover on tick bolds and shows `Jump to this keyframe`; clicking blank doesn't jump, avoiding being sucked to a keyframe
when dragging timeline.

**Keyframe events (Notify, #560 P1)**: `.ranim` v5 appends event table (time + name) at clip end, sync marker table and Montage section table;
engine each frame writes "time points crossed this frame" into `AnimEvents` resource (full rebuild, not accumulation):

```rust
for event in world.get_resource::<AnimEvents>().map(|e| e.drain()).unwrap_or_default() {
    match event.name.as_str() {
        "Footstep.L" => play_footstep(Footstep::Left),
        _ => {}
    }
}
```

- Interval is `(previous frame local time, this frame local time]`, loop wraparound also counts `[0, current]`,
  so "0.9 → 1.0 → 0.1" can trigger both 0.95 and 0.05 events;
- Open start: event at exactly previous frame's time already triggered on that frame, staying won't replay repeatedly;
- **Just created / just switched clip / paused / reverse playback all do not trigger** (reverse needs backward interval scan, tentatively not playing);
- **Where events come from**: glTF has no event channel, so author writes sidecar file
  `assets/<mesh>.skinning/notifies.json` (same level as source file, key is glTF's original animation name):

  ```json
  { "clips": { "walk": [
      { "time": 0.25, "name": "Footstep.L" },
      { "time": 0.75, "name": "Footstep.R" }
  ] } }
  ```

  Cook merges into `.ranim` by clip name (encoding sorted by time, drops negative time / empty name); file **exists but
  parse failure fails build**, name not matching any animation warns — wrong event table shouldn't silently disappear.
- Editor can edit event table: after selecting `assets/<mesh>.skinning/notifies.json` in asset panel, open
  Window → General → Animation Graph, lists each event by clip (time DragValue + name text box,
  blur writes back), `×` deletes, `+ Notify` draft form can add one for existing or new clip. Above each clip is also
  a timeline: gold dot is event, hover shows "name @ seconds", left-right drag directly changes time (quantized to 0.01s,
  clamped to 0..=window length, window length auto takes >=2s by latest event), shares same write-back path with DragValue.
  Clicking blank opens new draft at that time (clip name and time pre-filled, only event name missing), clicking existing mark doesn't create new.
  After save, loose asset hot reload re-cooks, `.ranim`'s v5 event table updates accordingly.

**剪辑检查器**：在资产面板选中 `.ranim` 后，Animation Graph 面板显示检查器：时长 / fps /
loop / additive、轨道数与每条轨道的关键帧范围、Notify / 同步标记时间轴、根关节首尾位移与
偏航角。事件和标记仍通过作者侧侧车编辑，检查器不写二进制剪辑。
场景里选中带 `Animator` 的实体时，还可以把当前剪辑套到该实体（会清空状态机 / 动画图路径，
避免它们覆盖剪辑），然后点击时间轴 seek、播放 / 暂停、改倍速和 loop；这些改动走场景撤销栈。

**同步标记（Sync marker）**：走路 / 跑步循环的脚落地相位不同，混合时可以按标记对齐。
作者写侧车 `assets/<mesh>.skinning/sync_markers.json`，键是 glTF 动画名，值是归一化相位：

```json
{ "clips": { "walk": [0.0, 0.5], "run": [0.05, 0.55] } }
```

cook 时归一化到 `[0, 1)`、排序去重，写进 `.ranim` v5；运行时
`Clip::sync_marker_set()` 返回 `SyncMarkerSet`，可以构造 `SyncGroup` 做相位映射。

编辑器可以编辑这张表：在资产面板选中 `assets/<mesh>.skinning/sync_markers.json` 后打开
Window → General → Animation Graph，按剪辑列出标记。每条有 `phase` DragValue 和 `×`；
每个剪辑上方的时间轴（0..1）用绿色圆点显示相位，拖动改相位，空白处点一下会在该相位打开
新增草稿，`+ Marker` 也可以手动输入剪辑名与相位。写入时归一化到 `[0, 1)` 并拒绝重复相位
（误差 `1e-6`），数组顺序保持作者写入顺序，cook 时再排序去重。保存后由松散资产热重载重新 cook。
文件不存在 = 没有标记；存在但解析失败会让 build 失败；名字对不上任何动画会 warn。

**Montage 段（#560 P4）**：翻越 / 受击这类一次性动画需要按段跳转（Start → Vault → Land）。
作者写侧车 `assets/<mesh>.skinning/montages.json`，键是 glTF 动画原名：

```json
{
  "clips": {
    "vault": {
      "sections": [
        { "name": "Start", "start": 0.0, "end": 0.3 },
        { "name": "Vault", "start": 0.3, "end": 0.8 },
        { "name": "Land", "start": 0.8, "end": 1.0 }
      ]
    }
  }
}
```

段名必须非空且同一剪辑内唯一；时间必须有限、`0 <= start <= end`，`end` 不能超过剪辑时长。
cook 时按 `start` 排序写进 `.ranim` v5，运行时 `Clip.sections` / `MontagePlayer::active_section`
可直接读；`MontageRequest::play_section(...)` 或 `with_section("Vault")` 会从段首开始，
`MontagePlayer::jump_to_section` 可在播放中跳段。BranchIn 用 Notify 约定：
`BranchIn.<section>` 事件跨过时跳到同名段（例如 `BranchIn.Land`），可以直接在 Notify
侧车里写。文件不存在 = 没有段；存在但解析失败 / 超出剪辑长度会让 build 失败；名字对不上
任何动画会 warn。

编辑器可以编辑这张表：在资产面板选中 `assets/<mesh>.skinning/montages.json` 后打开
Window → General → Animation Graph，按剪辑列出段。每个剪辑上方有一条彩色区间条（按时间画
start..end），下面是名字文本框（失焦写回）、`start` / `end` DragValue 和 `×`；`+ Section`
给当前剪辑加段，`+ New clip section` 可以给新剪辑键加第一段。每个剪辑下面还有
`Warp windows` 区：橙色区间条显示 MotionWarping 窗口，行内可改名字 / start / end、删除，
`+ Warp` 追加。warp window 同样 cook 进 `.ranim` v5，运行时用
`MontagePlayer::active_warp_window()` / `warp_window_progress()` 查询。保存后由松散资产
热重载重新 cook。

### 状态机

状态机写成 `assets/<something>.statemachine.json`，cook 成 `.rasm`（RASM v1：状态表 +
转移表 + 条件；`from` 用 `u32::MAX` 表示「任意状态」）：

```json
{
  "states": [
    { "name": "Idle", "clip": "char/idle.ranim" },
    { "name": "Run",  "clip": "char/run.ranim", "speed": 1.2, "looped": true }
  ],
  "transitions": [
    { "from": "Idle", "to": "Run", "duration": 0.2, "min_state_fraction": 0.0,
      "conditions": [{ "type": "float_greater", "name": "speed", "threshold": 1.5 }] }
  ]
}
```

- 条件种类：`float_greater` / `float_less`（`threshold`）、`bool_is`（`value`）、`trigger`；
  面板可以切换已有条件的类型，切换时保留参数名并补齐 / 删除该类型需要的载荷；
  参数名也可以直接编辑，同一转移里 `(type, name)` 重复时拒绝；
- 缺省：`speed = 1`、`looped = true`、`from = "*"`（任意状态）、`duration = 0`（硬切）、
  `min_state_fraction = 0`；`to` / `from` 写错状态名是**硬错误**（整份资产失败），
  重复状态名、空的 `clip` / `name`、未知条件种类同样报错并指出是哪一条；
- 求值是**按作者顺序第一条命中**（条件全真 + 当前状态已播到 `min_state_fraction`），
  一帧只走一条转移；`min_state_fraction` 在剪辑时长未知时视为已播完，否则单次剪辑会卡在最后一帧；
- `states` 的顺序同时决定初始状态：运行时 `Animator.state` 为空时从第一个状态开始。
  面板用 `↑` / `↓` 重排状态；转移按状态名引用，重排不会改转移。
- 面板里改状态名会同步场景 / 预制体里 `state_machine_path` 指向同一份状态机的
  `Animator.state`：打开着的文档就地改并标脏，没打开的文档直接写回磁盘。其它状态机的
  同名状态不受影响。
编辑器提供 **Animation Graph** 面板（Window → General）：选中 `.animgraph.json` 或
`.statemachine.json` 后显示参数、节点 / 连线、输出，以及状态、转移和可读条件。
面板顶部有 `↶ Undo edit` / `↷ Redo edit`，只针对动画资产（图 / 状态机 / Notify）的
JSON 改动，和场景的 Ctrl+Z 是两条独立历史：每步存整份文件快照，连续拖拽（DragValue /
时间轴）在 800ms 内合并成一步；参数改名这种同时改多个文件的操作整组算一步，撤销 / 重做
一起回滚。撤销后写回磁盘、下一次重绘直接用磁盘内容。
图的面板顶部有一行形状检查结果（`shape check: ok` / `Shape issues (n)`）：复刻 cooker 在
cook 前会拒绝的形状问题 —— 节点名 / 类型、剪辑与坐标数量、连线目标与自连、输出数量、
环、参数类型、每样本速度数量 —— 在保存前就列出来。它不读资产内容，剪辑 / 状态机文件
是否存在仍以 cook 为准。

按 **Play** 后（Game 视图在跑）面板多出一块绿色 `Runtime · N entities`：每个动画图实体的
节点按图顺序列出名字 / 类型 / 连线权重 / 本地时间，状态机节点显示 `状态名 · 时间`；下面还有
该实体各 Slot 的 Montage（剪辑、时间、权重、暂停 / 播完 / 排队段数），以及这一帧该实体
触发的 Notify（蓝色 `• 名字 @ 时间`，来自引擎的 `AnimEvents`，每帧重建）。数据来自引擎的
`AnimGraphDebug` 资源，只在 Play 时插入，停止后移除；编辑器不复制状态，面板每帧只读。
`Parameters` 区可以直接编辑：每行是名字文本框 · 类型 + 默认值 DragValue（bool / trigger 归一成
0 / 1），`×` 删除参数（仍被混合空间节点引用的参数会被拒绝并指出是哪个节点），`+ Parameter`
打开草稿表单（名字 + 类型下拉 + 默认值）。改参数名（失焦）会同时改写混合空间节点的
`parameter` / `parameter_x` / `parameter_y`，以及图引用的状态机侧车里的条件名：
节点写 `state_machine: "anim/hero.rasm"` 时自动找 `assets/anim/hero.statemachine.json`
（文件不存在就跳过，可能还没落盘），状态栏会列出同步了哪些文件。重名会被拒绝。
图节点可以在画布上拖动，位置保存到 `<graph>.layout.json`（不改图本身）；节点卡片里可以增删
`inputs` 连线（`+ Link` / `×`），每条连线的 `w` 可以直接拖拽调 0..1 权重，保存后写回 `.animgraph.json`。使用单数 `input` 的链式节点
仍需手改 JSON。每条连线下面还有一行逗号分隔的 `bone mask`（`spine_02, arm_l`）：失焦时才写回，
字符串连线需要掩码时自动升级成 `{ node, weight, bone_mask }`；清空这行等于去掉掩码（整条姿态）。
剪辑节点的剪辑路径是文本框（失焦写回），每个节点还有 `speed` / `w` DragValue 和 `loop` 复选框。
节点名也是文本框：重命名会同步改写其它节点的 `inputs` / `input` 与顶层 `output`，但布局 sidecar
里还是旧名字，节点会回到画布的默认网格位置。
混合空间节点额外显示参数名文本框（二维各一个）和每个样本的 `x` / `y` DragValue（一维只写 `x`）；
二维节点还有一块 180×120 的样本画布：按坐标画出样本点，直接拖动点就改 `positions`（与 DragValue
共用写回路径），坐标范围标在画布下方。所有样本共线时画布下方给橙色提示「退化」——运行时三角化
会失败并退回一维 / 单剪辑，数字上不容易看出来；
节点卡片上的 `⧉ Duplicate` 复制整份节点（剪辑 / 坐标 / inputs / bone mask 原样保留），
名字自动加 `_copy` / `_copy2`；复制出的节点接线仍指向原来的上游，输出节点不能复制。
`clips` 与 `positions` 数量对不上时用橙色提示，cooker 会拒绝这种节点。样本行尾的 `×` 删除样本、
`+ Sample` 打开草稿表单（剪辑 + x/y），一维至少留 2 个、二维至少留 3 个；节点带每样本 `speeds`
时增删会同步维护那张表。`+ Node` 追加节点：类型下拉 `clip` / `blend stack` / `slot` /
`layered blend` / `cached pose` / `state machine`；clip 填剪辑路径、state machine 填机器路径，
其它类型填一个上游节点（自动写成 `input` / `inputs[0]`）。blend space 需要 >=2/3 个
剪辑与坐标，仍然手写或复制现有节点。`× Remove node` 删除节点，输出节点和仍被引用的节点会被
拒绝并说明原因，先断线再删。状态机视图支持 `+ Transition`（任意状态 / 指定 from → to）和 `× Remove transition`，
保存回 `.statemachine.json`。展开一条转移后 `from` / `to` 是下拉框（`from` 可选 `any`），
重连会拒绝重名状态、自连和与现有转移完全重复的 `(from, to)`；`↑` / `↓` 调整转移顺序，
顺序就是优先级（运行时取第一条条件命中的转移，列表上也写了这句提示）；`⧉ Duplicate` 复制整条转移
与条件并插入到原转移之后，作者再改 `from` / `to`。状态行末尾的 `⧉` 复制整个状态（剪辑 / speed / looped 原样保留，名字自动 `_copy`），
转移表不变，复制出来的状态需要自己接线。状态行的名字与剪辑是文本框（失焦写回）、`speed` 是 DragValue（0..5）、
`loop` 是复选框；重命名会同步改写转移里的 `from` / `to`，以及场景 / 预制体里指向同一状态机的
`Animator.state`（其它状态机的同名状态不受影响）。每条转移的 `blend` / `min` 是 DragValue（混合时长 / 最小状态占比）；
条件行可以直接改 float 阈值或 bool 期望值、`×` 删除，`+ Condition` 打开草稿表单（类型下拉
`float >` / `float <` / `bool ==` / `trigger`、参数名、阈值或布尔值），点 `Add condition` 才写盘。
条件只校验形状（名字非空、类型已知、`(type, name)` 不重复），参数是否真在运行时的
`AnimParameters` 里仍由运行期决定。视口里直接拖关节做 IK（P2 的 IK 另立）还没做。

运行时（#560 P1）：`Animator.state_machine_path` 指向 `.rasm`、`state` 是当前状态名（引擎每帧
写回，可在检查器里预置起始状态）。状态机生效时：

- **状态决定播什么**：`state.clip_path`、`speed`、`looped` 覆盖组件上的同名字段（状态剪辑为空
  时才回落到 `clip_path`）；
- 转移由 **`prism_anim::AnimParameters`** 资源驱动（项目侧每帧写 `speed` / `grounded` / trigger
  等），引擎在 `animator_system` 里求值；一帧最多走一条转移，换状态时播放时间归零；
- `min_state_fraction` 用**上一帧**的播放时间判定（那一帧确实播到了那里），所以跨过阈值的那
  一帧还拦着、下一帧放行；
- 转移自己的 `duration` 覆盖组件的 `blend_duration`（那条转移说了算），淡出源仍是上一帧姿态；
- 机器路径写错 / 还没加载时安静退回 `clip_path`，不会让实体消失。
- 检查器的 `Animator` 卡片里显示当前状态与机器文件名（只读）。


## 4.4 内置截图与控制通道

截图在编辑器进程里做（`editor/crates/prism-editor-egui/src/screenshot.rs`）：

| target | 取法 | 说明 |
|---|---|---|
| `window` | Windows `PrintWindow` + `PW_RENDERFULLCONTENT` 画进 32bpp DIB | 连 UI 一起抓；DWM 合成后取样，窗口部分在屏幕外也能抓全。交换链由 egui-ash 持有，宿主拿不到 `VkImage`，这是唯一路径 |
| `scene` / `game` | `GraphRenderer::copy_offscreen_to_readback` + `readback_pixels` | 引擎离屏图（`PRESENT_SRC_KHR` → 读回 → PNG），不需要窗口，无显示器 / CI 可用 |

触发方式（都不是环境变量）：MCP 的 `editor_capture`（见 `docs/editor-mcp.md`），
或菜单「Window → Capture Window」（默认写到 `target/editor-egui/captures/`）。

控制通道（`src/control.rs`）：启动时在 `127.0.0.1:0` 上监听并把端口写进
`<项目根>/target/editor-egui/control.port`，一行 JSON 一问一答：
`ping` / `status` / `play` / `stop` / `pause` / `logs` / `input` / `capture` /
`ui.snapshot` / `ui.click` / `ui.move` / `ui.wheel` / `ui.drag` / `ui.set` /
`quit` / `quit.confirm` / `quit.cancel`。
`quit` 是自动化用的强制退出（不写恢复快照、不弹确认）；`quit.confirm` 走用户点窗口 ×
的路径——有未保存文档时返回 `closing:false` / `pending:"unsaved"` 并弹出三选一，
`quit.cancel` 关掉该对话框。
`logs` 读 Console 的同一份缓冲（`since` 增量、`limit` 条数），自动化因此能拿到引擎日志、
工程模块的 `project:` 日志和 panic 栈。

失败回执除 `ok: false` 外还带 `kind`（机器可读）与 `message`；`error` 保留同一段文本，
旧调用方不用改（#1008）：

| `kind` | 含义 | 调用方该做什么 |
|---|---|---|
| `bad_request` | JSON / `cmd` / 参数不合法（未知节点、越界坐标、不认识的按键） | 改参数重试 |
| `not_ready` | 编辑器还没准备好：5 秒内没回执、视口还没布局、Play / 对话框还没打开 | 等一会再试 |
| `denied` | 明确拒绝：平台不支持整窗截图、通道被关掉 | 不要重试 |
| `internal` | 编辑器内部错误（截图写盘失败等） | 读 `logs`，重试通常无用 |

全局开关：`--no-control` 启动时不建 listener、不写 `control.port`（而不是「连上以后
逐条拒绝」）；Preferences 的 `Enable local control channel` 可以在运行中关掉，
关掉会立即停止监听并删除端口文件。`status` 回执的 `control` 段给出
`enabled` / `cli_disabled` / `port_file` / `error`。

视口里的指针操作也走控制通道（#643），坐标单位是**视口局部逻辑像素**：

| 命令 | 参数 | 行为 |
|---|---|---|
| `ui.move` | `node`+`offset` 或 `tab`+`x`/`y` | Scene：返回该点下的实体（`hit`）与 gizmo 轴（`gizmo_axis`），不改选择；Game：把光标送进玩法输入 |
| `ui.wheel` | 同上 + `delta`（默认 120） | Scene：缩放到光标（`Camera::zoom_towards`，#511）；Game：滚轮进玩法输入 |
| `ui.drag` | `from` / `to` + `steps`（默认 8） | Scene：起点命中 gizmo 就按真实拖动逻辑变换（吸附、撤销步一致），否则 `mode:"orbit"` 转轨道相机，再否则框选；Game：按下 → 逐步移动 → 松开 |

`from` / `to` 是对象：`{"node":"view:scene:entity:2","offset":[40,0]}`（物体节点锚点是
其 `screen_rect` 中心）或 `{"x":220,"y":610}`。Scene 的框选结果直接写进选择集，回执带
`selected` 节点列表；gizmo 拖动回执带 `changed`，用于断言变换真的发生了。
`ui.click` 的 Game 分支也走同一套 letterbox / 离屏尺寸换算（#463/#585）。

模态对话框也走节点树（#618）：`ui.snapshot` 的 `dialogs` 列出当前打开的对话框
（`delete-entities` / `delete-document` / `delete-assets` / `save-conflict` / `close` /
`confirm` / `prompt`）及按钮 id，`pending_dialog` 给出最上面那个；`ui.click` 接受
`dialog:<id>:<button>`（如 `dialog:delete-entities:delete` /
`dialog:close:discard`）。`ui.set` 对 `dialog:prompt` 写入输入值，再用
`dialog:prompt:ok` 提交。命令只是挂起确认框时，回执会带 `pending_dialog`，
不会出现「`ok: true` 但什么都没发生」。

`input` 给 Play 注入键鼠（`key=space` / `mouse=left` / `click=true`，`state=press|release`）。
事件**每帧只消费一个**：`just_pressed` 只在这一帧的 tick 里可见，下一帧就会被
`InputManager::begin_frame` 清掉，一次塞两个等于没按。fishing 的完整一竿就是这么跑的：
`click` → `cast` → `bobber landed` → 等到 `bite!` → 再 `click` → `hook set` →
`landed fish #1`，全流程只用控制通道，不需要窗口焦点。请求经 `mpsc` 排进 UI 线程，所以截图和
界面状态是同一帧的；启动前三帧内的 `capture` 会延后到画面合成后再抓（否则是白图）。

## 5. 数据层

`editor/crates/prism-editor-core` 是与 UI 框架无关的数据层：文档模型、schema、撤销栈、dock 树、日志缓冲、资产树、i18n、项目读写。
egui 壳通过路径依赖直接使用它。

## 6. 未完成 / 风险

- **未在真实设备上运行**：开发机是 Termux（aarch64、无显示），只做了
  `cargo check`（本机 + `x86_64-pc-windows-gnu`）。首帧呈现、resize、字体、
  docking 手感都需要桌面复核。
- **尚未实现**：材质 / 贴图拖到场景里自动挂 `MeshRenderer`；层级多选整组插入排序
  （整组只支持改父级）；多窗口（egui viewport）的独立 DPI。
- **内置图元（已打通）**：`crates/prism-render/src/primitives.rs` 生成 box / sphere /
  cylinder / capsule / quad 的顶点与索引；`mesh_path = "__builtin/<name>"` 由
  `prism-engine` 的资产解析器短路，不读 `.pak`、不需要 cook。对应的实体预设
  （`cube / sphere / cylinder / capsule / quad / plane`）在 `prism-editor-api`
  的 `entity_presets()` 里，GameObject → Create Other → 3D Object 直接用。
  尺寸固定：box/sphere/cylinder 1 单位，capsule 总高 2（半径 0.5），
  quad/plane 是 `y = 0` 上的 1×1 XZ 面、法线 +Y。
- **性能**：每次文档变化都会整场重载两个引擎实例（防抖 120ms），大场景下偏重；
  后续可以改成「只重建变化的实体」或引入 delta 上传。
- **两个引擎实例**意味着两份 GPU 资源（管线、IBL、网格）。如果以后只保留一个
  视口可见，可以按可见性懒加载 / 回收 Game view。
