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
- `projects/template` and `projects/demos/fishing` `preview-module` must explicitly open
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
  only does `saturate`, combined with sRGB swapchain can 1:1 restore source image. `projects/demos/fishing` defaults on.
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
  `t:texture` (English slug / `Texture` / localized Chinese label all recognized), `dir:sprites`,
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
  CJK between characters can break, terminal punctuation won't land at line start; single word too long breaks by char),
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

**Clip inspector**: after selecting `.ranim` in asset panel, Animation Graph panel shows inspector: duration / fps /
loop / additive, track count and each track's keyframe range, Notify / sync marker timeline, root joint first/last displacement and
yaw angle. Events and markers still edited via author-side sidecar, inspector doesn't write binary clip.
When selecting an entity with `Animator` in scene, can also apply current clip to that entity (clears state machine / animation graph path,
so they don't override clip), then click timeline to seek, play / pause, change speed and loop; these changes go through scene undo stack.

**Sync marker**: walk / run loops have different foot-plant phases, can align by marker when blending.
Author writes sidecar `assets/<mesh>.skinning/sync_markers.json`, key is glTF animation name, value is normalized phase:

```json
{ "clips": { "walk": [0.0, 0.5], "run": [0.05, 0.55] } }
```

normalized to `[0, 1)`, sorted and deduped at cook time, written into `.ranim` v5; at runtime
`Clip::sync_marker_set()` returns `SyncMarkerSet`, can construct `SyncGroup` for phase mapping.

Editor can edit this table: after selecting `assets/<mesh>.skinning/sync_markers.json` in asset panel open
Window → General → Animation Graph, list markers per clip. Each has `phase` DragValue and `×`;
timeline above each clip (0..1) shows phase as green dot, drag to change phase, clicking blank space opens
new draft at that phase, `+ Marker` also lets manual clip name and phase entry. On write normalized to `[0, 1)` and duplicate phases rejected
(tolerance `1e-6`), array order kept as authored, sorted / deduped again at cook. After save loose-asset hot reload re-cooks.
File absent = no markers; present but parse failure fails build; name matching no animation warns.

**Montage sections (#560 P4)**: one-shot animations like vault / hit need section jumps (Start → Vault → Land).
Author writes sidecar `assets/<mesh>.skinning/montages.json`, key is original glTF animation name:

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

Section name must non-empty and unique within clip; time must finite, `0 <= start <= end`, `end` can't exceed clip duration.
At cook sorted by `start` into `.ranim` v5, at runtime `Clip.sections` / `MontagePlayer::active_section`
directly readable; `MontageRequest::play_section(...)` or `with_section("Vault")` starts from section beginning,
`MontagePlayer::jump_to_section` jumps between sections during playback. BranchIn uses Notify convention:
`BranchIn.<section>` event when crossed jumps to same-named section (e.g. `BranchIn.Land`), can be written directly in Notify
sidecar. File absent = no sections; present but parse failure / exceeding clip length fails build; name matching no
animation warns.

Editor can edit this table: after selecting `assets/<mesh>.skinning/montages.json` in asset panel open
Window → General → Animation Graph, list sections per clip. Above each clip a colored range bar (drawn by time
start..end), below is name text box (written back on blur), `start` / `end` DragValue and `×`; `+ Section`
adds section to current clip, `+ New clip section` adds first section to new clip key. Below each clip also
`Warp windows` area: orange range bar shows MotionWarping window, row can edit name / start / end, delete,
`+ Warp` appends. warp window likewise cooked into `.ranim` v5, queried at runtime via
`+ Warp` appends. warp window likewise cooked into `.ranim` v5, queried at runtime via
`MontagePlayer::active_warp_window()` / `warp_window_progress()`. After save loose-asset hot reload re-cooks.

### State machine

State machine written as `assets/<something>.statemachine.json`, cooked into `.rasm` (RASM v1: state table +
transition table + conditions; `from` uses `u32::MAX` for "any state"):

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

- Condition kinds: `float_greater` / `float_less` (`threshold`), `bool_is` (`value`), `trigger`;
  panel can switch existing condition kind, keeping parameter name and filling / removing payload needed by new kind;
  parameter name also editable, duplicate `(type, name)` within same transition rejected;
- Defaults: `speed = 1`, `looped = true`, `from = "*"` (any state), `duration = 0` (hard cut),
  `min_state_fraction = 0`; wrong `to` / `from` state name is **hard error** (whole asset fails),
  duplicate state names, empty `clip` / `name`, unknown condition kinds likewise error with item indicated;
- Evaluation is **first match in author order** (all conditions true + current state played to `min_state_fraction`),
  at most one transition per frame; `min_state_fraction` treated as complete when clip duration unknown, else one-shot would stick on last frame;
- `states` order also determines initial state: at runtime when `Animator.state` empty starts from first state.
  Panel uses `↑` / `↓` to reorder states; transitions reference by state name, reordering doesn't change them.
- Renaming state in panel syncs `Animator.state` in scenes / prefabs pointing to same state machine:
  open docs changed in place and marked dirty, unopened docs written directly to disk. Same-named states in
  other state machines unaffected.
Editor provides **Animation Graph** panel (Window → General): after selecting `.animgraph.json` or
`.statemachine.json` shows parameters, nodes / links, output, plus states, transitions and readable conditions.
Panel top has `↶ Undo edit` / `↷ Redo edit`, only for animation asset (graph / state machine / Notify)
JSON changes, separate history from scene Ctrl+Z: each step stores whole-file snapshot, consecutive drags (DragValue /
timeline) within 800ms merged into one step; parameter rename touching multiple files counts as one group, undo / redo
roll back together. After undo written to disk, next repaint uses disk content directly.
Graph panel top has one line of shape-check result (`shape check: ok` / `Shape issues (n)`): replicates shape issues cooker rejects
before cooking — node name / type, clip and coordinate counts, link targets and self-links, output count,
cycles, parameter types, per-sample speed counts — listed before save. It doesn't read asset content, clip / state machine file
existence still determined by cook.

After pressing **Play** (Game view running) panel gains green `Runtime · N entities` block: each animation-graph entity's
nodes listed in graph order with name / type / link weight / local time, state-machine nodes show `state name · time`; below are
that entity's Slot Montages (clip, time, weight, paused / finished / queued sections), and Notify fired by that entity this frame
(blue `• name @ time`, from engine `AnimEvents`, rebuilt each frame). Data comes from engine `AnimGraphDebug` resource,
only inserted during Play, removed after stop; editor doesn't copy state, panel reads only each frame.
`Parameters` area directly editable: each row name text box · type + default DragValue (bool / trigger normalized to
0 / 1), `×` deletes parameter (one still referenced by blend-space node rejected with node named), `+ Parameter`
opens draft form (name + type dropdown + default). Renaming parameter (on blur) also rewrites blend-space nodes'
`parameter` / `parameter_x` / `parameter_y`, and condition names in state-machine sidecar referenced by graph:
node with `state_machine: "anim/hero.rasm"` auto-finds `assets/anim/hero.statemachine.json`
(skip if absent, may not be written yet), status bar lists which files synced. Duplicate names rejected.
Graph nodes draggable on canvas, position saved to `<graph>.layout.json` (graph itself unchanged); node card can add / remove
`inputs` links (`+ Link` / `×`), each link's `w` directly draggable for 0..1 weight, written back to `.animgraph.json` on save. Chained nodes using singular `input`
still need manual JSON edit. Below each link a comma-separated `bone mask` row (`spine_02, arm_l`): written back on blur,
string link needing mask auto-upgraded to `{ node, weight, bone_mask }`; clearing row removes mask (whole pose).
Clip node's clip path is text box (written on blur), each node also `speed` / `w` DragValue and `loop` checkbox.
Node name also text box: rename rewrites other nodes' `inputs` / `input` and top-level `output`, but layout sidecar
keeps old name, node returns to default grid position on canvas.
Blend-space node additionally shows parameter name text box (one per 2D axis) and each sample's `x` / `y` DragValue (1D only `x`);
2D node also 180×120 sample canvas: sample points drawn by coords, dragging point changes `positions` (shares write-back with DragValue
path), coordinate range labeled below canvas. When all samples collinear orange "degenerate" hint below canvas — runtime triangulation
fails and falls back to 1D / single clip, hard to see numerically;
`⧉ Duplicate` on node card copies whole node (clip / coords / inputs / bone mask preserved),
name auto-appends `_copy` / `_copy2`; duplicated node links still point to original upstream, output node can't be copied.
When `clips` and `positions` counts mismatch orange hint, cooker rejects such node. `×` at end of sample row deletes sample,
`+ Sample` opens draft form (clip + x/y), at least 2 for 1D, 3 for 2D; node with per-sample `speeds`
maintains that table on add / remove. `+ Node` appends node: type dropdown `clip` / `blend stack` / `slot` /
`layered blend` / `cached pose` / `state machine`; clip fills clip path, state machine fills machine path,
others fill one upstream node (auto-written as `input` / `inputs[0]`). blend space needs >=2/3 clips and coords,
still handwritten or copied from existing node. `× Remove node` deletes node, output node and still-referenced nodes
rejected with reason, disconnect first. State-machine view supports `+ Transition` (any state / specified from → to) and `× Remove transition`,
saved back to `.statemachine.json`. Expanding a transition shows `from` / `to` dropdowns (`from` can be `any`),
reconnect rejects duplicate state names, self-links and exact duplicate `(from, to)` with existing; `↑` / `↓` adjusts transition order,
order is priority (runtime takes first matching transition, also noted in list); `⧉ Duplicate` copies whole transition
with conditions and inserts after original, author then edits `from` / `to`. `⧉` at end of state row copies whole state (clip / speed / looped preserved, name auto `_copy`),
transition table unchanged, copied state needs own wiring. State row name and clip are text boxes (written on blur), `speed` DragValue (0..5),
`loop` checkbox; rename rewrites `from` / `to` in transitions and `Animator.state` in scenes / prefabs pointing to same state machine
(same-named states in other state machines unaffected). Each transition's `blend` / `min` are DragValue (blend duration / min state fraction);
condition row can directly edit float threshold or bool expected value, `×` deletes, `+ Condition` opens draft form (type dropdown
`float >` / `float <` / `bool ==` / `trigger`, parameter name, threshold or bool), click `Add condition` writes to disk.
Conditions only shape-validated (name non-empty, type known, `(type, name)` not duplicate), whether parameter actually in runtime
`AnimParameters` decided at runtime. Dragging joints in viewport for IK (P2 IK separate) not done.

At runtime (#560 P1): `Animator.state_machine_path` points to `.rasm`, `state` is current state name (engine writes
each frame, can preset start state in inspector). When state machine active:

- **State determines what plays**: `state.clip_path`, `speed`, `looped` override same-named fields on component (state clip empty
  falls back to `clip_path`);
- Transitions driven by **`prism_anim::AnimParameters`** resource (project side writes `speed` / `grounded` / trigger
  each frame), engine evaluates in `animator_system`; at most one transition per frame, switching resets play time;
- `min_state_fraction` judged by **previous frame** play time (that frame actually reached there), so frame crossing threshold
  is still blocked, next frame allowed;
- Transition's own `duration` overrides component `blend_duration` (that transition wins), fade source still previous-frame pose;
- Wrong / not-yet-loaded machine path silently falls back to `clip_path`, entity won't disappear.
- Inspector `Animator` card shows current state and machine filename (read-only).


## 4.4 Built-in screenshot and control channel

Screenshot done in editor process (`editor/crates/prism-editor-egui/src/screenshot.rs`):

| target | Method | Notes |
|---|---|---|
| `window` | Windows `PrintWindow` + `PW_RENDERFULLCONTENT` into 32bpp DIB | Captures UI too; sampled after DWM composition, works even if part of window off-screen. Swapchain held by egui-ash, host can't get `VkImage`, this is only path |
| `scene` / `game` | `GraphRenderer::copy_offscreen_to_readback` + `readback_pixels` | Engine offscreen image (`PRESENT_SRC_KHR` → readback → PNG), no window needed, works headless / CI |

Triggers (none are env vars): MCP `editor_capture` (see `docs/editor-mcp.md`),
or menu "Window → Capture Window" (default writes to `target/editor-egui/captures/`).

Control channel (`src/control.rs`): listens on `127.0.0.1:0` at startup and writes port to
`<project root>/target/editor-egui/control.port`, one JSON line per request / response:
`ping` / `status` / `play` / `stop` / `pause` / `logs` / `input` / `capture` /
`ui.snapshot` / `ui.click` / `ui.move` / `ui.wheel` / `ui.drag` / `ui.set` /
`quit` / `quit.confirm` / `quit.cancel`.
`quit` is forced exit for automation (no recovery snapshot, no confirmation); `quit.confirm` follows user clicking window ×
path — with unsaved docs returns `closing:false` / `pending:"unsaved"` and shows three-choice dialog,
`quit.cancel` closes that dialog.
`logs` reads same Console buffer (`since` incremental, `limit` count), automation thus gets engine logs,
project module `project:` logs and panic stacks.

Failure receipt besides `ok: false` also carries `kind` (machine-readable) and `message`; `error` keeps same text,
old callers need no change (#1008):

| `kind` | Meaning | What caller should do |
|---|---|---|
| `bad_request` | Invalid JSON / `cmd` / args (unknown node, out-of-range coords, unrecognized key) | Fix args and retry |
| `not_ready` | Editor not ready: no receipt within 5s, viewport not laid out, Play / dialog not open | Wait and retry |
| `denied` | Explicit refusal: platform doesn't support whole-window screenshot, channel disabled | Don't retry |
| `internal` | Editor internal error (screenshot write failure etc) | Read `logs`, retry usually useless |

Global switch: `--no-control` doesn't create listener or write `control.port` at startup (rather than "reject each request after connecting");
Preferences `Enable local control channel` can disable at runtime, disabling immediately stops listening and deletes port file. `status` receipt `control` section gives
`enabled` / `cli_disabled` / `port_file` / `error`.
Pointer operations in viewport also go through control channel (#643), coordinate unit is **viewport-local logical pixels**:

| Command | Args | Behavior |

| `ui.move` | `node`+`offset` or `tab`+`x`/`y` | Scene: returns entity under point (`hit`) and gizmo axis (`gizmo_axis`), no selection change; Game: sends cursor into gameplay input |
|---|---|---|
| `ui.wheel` | same + `delta` (default 120) | Scene: zoom toward cursor (`Camera::zoom_towards`, #511); Game: wheel into gameplay input |
| `ui.drag` | `from` / `to` + `steps` (default 8) | Scene: if start hits gizmo transform by real drag logic (snap, undo step consistent), else `mode:"orbit"` orbit camera, else box select; Game: press → step move → release |
| `ui.drag` | `from` / `to` + `steps` (default 8) | Scene: if start hits gizmo transform by real drag logic (snap, undo step consistent), else `mode:"orbit"` orbit camera, else box select; Game: press → step move → release |

`from` / `to` are objects: `{"node":"view:scene:entity:2","offset":[40,0]}` (object node anchored at
center of its `screen_rect`) or `{"x":220,"y":610}`. Scene box-select result written directly to selection set, receipt carries
`selected` node list; gizmo drag receipt carries `changed` for asserting transform really happened.
`ui.click` Game branch uses same letterbox / offscreen size conversion (#463/#585).

Modal dialogs also go through node tree (#618): `ui.snapshot` `dialogs` lists currently open dialogs
(`delete-entities` / `delete-document` / `delete-assets` / `save-conflict` / `close` /
`confirm` / `prompt`) and button ids, `pending_dialog` gives topmost one; `ui.click` accepts
`dialog:<id>:<button>` (e.g. `dialog:delete-entities:delete` /
`dialog:close:discard`). `ui.set` writes input value to `dialog:prompt`, then
`dialog:prompt:ok` submits. When command only suspends confirmation dialog, receipt carries `pending_dialog`,
no "`ok: true` but nothing happened".

`input` injects key / mouse into Play (`key=space` / `mouse=left` / `click=true`, `state=press|release`).
Events **consumed one per frame**: `just_pressed` only visible in that frame's tick, next frame cleared by
`InputManager::begin_frame`, stuffing two equals none. Full fishing cast runs this way:
`click` → `cast` → `bobber landed` → wait for `bite!` → `click` again → `hook set` →
`landed fish #1`, whole flow uses only control channel, no window focus. Requests queued to UI thread via `mpsc`, so screenshot and
UI state are same frame; `capture` within first three frames after startup delayed until after composition (else blank image).

## 5. Data layer

`editor/crates/prism-editor-core` is UI-framework-independent data layer: document model, schema, undo stack, dock tree, log buffer, asset tree, i18n, project read / write.
egui shell uses it directly via path dependency.

## 6. Incomplete / risks

- **Not run on real device**: dev machine is Termux (aarch64, no display), only `cargo check` done (local + `x86_64-pc-windows-gnu`). First-frame present, resize, fonts,
  docking feel need desktop review.
  docking feel need desktop review.
- **Not implemented**: drag material / texture into scene auto-attaches `MeshRenderer`; hierarchy multi-select group insert ordering
  (group only supports changing parent); multi-window (egui viewport) independent DPI.
- **Built-in primitives (wired through)**: `crates/prism-render/src/primitives.rs` generates box / sphere /
  cylinder / capsule / quad vertices and indices; `mesh_path = "__builtin/<name>"` short-circuited by
  `prism-engine` asset parser, no `.pak` read, no cook. Corresponding entity presets
  (`cube / sphere / cylinder / capsule / quad / plane`) in `prism-editor-api`
  `entity_presets()`, available via GameObject → Create Other → 3D Object.
  Fixed sizes: box / sphere / cylinder 1 unit, capsule total height 2 (radius 0.5),
  quad / plane is 1×1 XZ plane at `y = 0`, normal +Y.
- **Performance**: every document change reloads both engine instances whole scene (debounce 120ms), heavy for large scenes;
  can later change to "rebuild only changed entities" or introduce delta upload.
- **Two engine instances** means two copies of GPU resources (pipelines, IBL, meshes). If only one viewport visible later,
  can lazily load / recycle Game view by visibility.
