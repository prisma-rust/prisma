# editor

Prisma's **egui editor** (egui + ash Vulkan). Independent workspace, not attached to root workspace.

- Follows classic desktop editor layout: menu bar / main toolbar /
  status bar + split-tree docking (draggable splitter, tab switching, floating windows).
- Menus / shortcuts follow project conventions (Ctrl+1..5 panels, named layouts, Prefab Variant).
- Interaction: 2/3-level menus, hierarchy drag-to-reparent, inspector multi-select batch, W/E/R three gizmos,
  marquee select, tab drag-to-split/float, asset drag (dir move / hierarchy instance / field write).
- Data layer fully reuses `editor/crates/prism-editor-core` (UI framework agnostic:
  document model, schema, undo stack, dock tree, log buffer, asset tree, i18n, project read/write).
- **Scene / Game viewport zero-copy**: `prism-render` renders to its own offscreen `VkImage`,
  editor and engine share the same `VkDevice` / queue, egui directly registers that image's `VkImageView`
  as a texture sampler. No `vkCmdCopyImageToBuffer`, no CPU pixel readback.

## Running

```bash
cd editor
cargo run -p prism-editor-egui
```

Prerequisites:

- Desktop Vulkan device (Windows / Linux; Android needs extra winit backend port, not yet integrated);
- Compiled shaders: `.spv` checked in, available after clone; after editing `assets/shaders/slang/*.slang` run `bash assets/shaders/compile.sh`;
- Project root (containing `assets/`) can be found from cwd or parent directory; if not found, use `editor/public/demo.json`
  read-only fixture to start the UI.

Optional environment variables:

| Variable | Purpose |
|---|---|
| `PRISMA_EDITOR_DEMO` | fixture JSON path |
| `RUST_LOG` | log level (default `warn`) |

## Screenshots / MCP-driven

On startup the editor opens a local control channel on `127.0.0.1:0` and writes the port into
`<project root>/target/editor-egui/control.port`. `crates/prism-editor-mcp` uses it to drive
preview: `editor_launch` / `editor_status` / `editor_capture` / `editor_quit`
(see `docs/editor-mcp.md`). Menu "Window → Capture Window" is the local entry point of the same implementation.

- `window`: whole-window PNG (UI + viewport). Windows uses `PrintWindow`, samples after DWM composition,
  window parts off-screen can still be fully captured; other platforms not supported.
- `scene` / `game`: only captures that viewport's engine offscreen image (`PRESENT_SRC_KHR` readback),
  no window needed, available without display / in CI.

The UI itself can be read and operated by agents (`editor_ui`): `snapshot` returns a UI node tree
(dock panels / active tab / open documents / hierarchy entities / asset entries / inspector component fields /
`Command` list), node ids like `panel:hierarchy`, `entity:3`, `doc:0`,
`asset:assets/x.prefab.json`, `field:3:Transform:position[1]`, `command:Save`;
`click` / `set` by id follow the same code path as the UI (`ui_tree.rs`).

## Structure

```
crates/prism-editor-egui/src/
├── main.rs          egui-ash entry (AppCreator + RunOption)
├── gpu.rs           shared VkDevice: instance / physical device / device / allocator / command pool
├── build.rs         build / cook subprocess (`prism-build-pipeline`), output to Console
├── control.rs       local control channel (TCP + one-line JSON: ping/status/capture/quit)
├── screenshot.rs    built-in screenshot: whole window (PrintWindow) / viewport (offscreen readback)
├── app.rs           shell: engine-style menus (2/3-level), toolbar, status bar, docking, Play
├── engine_host.rs   in-process engine host: Engine + GraphRenderer + asset upload (sync)
├── viewport.rs      Scene/Game viewport: offscreen image → egui texture, orbit/pick/gizmo
├── panels.rs        hierarchy, inspector, assets, doc, log, history, render graph / settings panels
├── logging.rs       `log` tee: engine / render warnings also to Console panel
├── widgets.rs       schema-driven field controls
└── theme.rs         data-layer theme token → egui visuals + CJK font fallback
```

## Zero-copy path

```text
prism-render GraphRenderer (offscreen OffscreenTarget, R8G8B8A8_UNORM)
  └─ end of frame: PRESENT_SRC_KHR → SHADER_READ_ONLY_OPTIMAL (same-queue barrier)
        └─ ImageRegistry::register_user_texture(image_view, sampler) → TextureId
              └─ egui painter.image(TextureId, rect, uv)  (same VkDevice / queue)
```

- Only one queue, one thread (egui-ash event loop thread), no cross-thread semaphore needed;
- Stationary edit viewport **zero GPU work**: only re-renders on document change / camera change / selection change / resize /
  Play, remaining frames reuse last frame's texture;
- On resize `GraphRenderer::resize_offscreen` rebuilds the image (after `device_wait_idle`),
  editor unregisters old texture id, registers new image view.

## Scene vs Game

| | Scene View | Game View |
|---|---|---|
| Camera | Editor orbit camera (ECS camera entity with `depth = 1000`) | Camera in document |
| Simulation | Always edit mode (hierarchy propagation only) | Runs `fixed_update` / `update` / `late_update` on Play |
| Engine instance | Its own `Engine` + `GraphRenderer` | Its own `Engine` + `GraphRenderer` (created on first show) |

Each viewport holds its own engine instance, so their cameras
don't interfere with each other; on document change both worlds reload together.

## Verified / Unverified

- ✅ `cargo check` (Termux aarch64, compiled with placeholder `.spv`);
- ✅ `cargo check --target x86_64-pc-windows-gnu`;
- ❌ **Not yet run on a real Vulkan device**: current dev environment is Termux (no display / no
  Vulkan device), runtime behavior (first frame present, resize, font loading, docking drag feel) needs
  to be re-checked on desktop.
- ❌ Android: egui-ash supports `android-game-activity`, but this crate hasn't integrated
  `android_main` yet (egui editor not integrated on Android yet).
