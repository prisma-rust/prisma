# Prisma self-hosted editor UI architecture design spec

> **Goal**: without breaking [AGENTS.md](../AGENTS.md) dependency boundaries, based on Prisma's existing `prism-ecs`, `prism-reflect`, `prism-render` (`ui_overlay` / offscreen `VkImage`) and `prism-editor-core`, design an **inheritance-free, no `Rc<RefCell<T>>`, highly composable / reusable** retained-mode UI kernel and editor architecture.
>
> **UI authority has been assigned to ECS** (§9). The `prism-ui` tree / layout / draw kernel described in §2–§4 is **no longer an independent runtime** under this decision, only keeping declarative builder and interaction state machine responsibilities; read §9 as authoritative.

---

## 1. Overall layering and dependency boundary

Respect the player-chain zero-UI / zero-editor red line (`template → prism-app → prism-engine → {scene, ecs, render, asset, physics2d}`), and adopt the **"editor is a game"** model, placing the self-hosted editor as an independent project in `projects/editor/`:

```text
projects/editor/             # Independent workspace (modeled on projects/template), "editor is a game"
├── Cargo.toml               # Independent [workspace], launched via prism-app + prism-engine
├── assets/                  # Editor's own icons, fonts, themes, default layout scene
└── crates/
    ├── prism-ui             # Reusable retained-mode UI kernel (SlotMap tree + behavior composition + DrawList)
    └── editor-app           # Binds prism-editor-core + prism-editor-api + prism-runtime-host

Existing shared foundation (directly reused):
├── editor/crates/prism-editor-core   # UI-framework-independent business core (dock / undo / textinput / theme / keys)
├── crates/prism-editor-api           # Reflection schema / reflect_edit / property_drawer
├── crates/prism-runtime-host         # Loads target game project's preview-module cdylib (RTTIBin cross ABI)
└── crates/prism-render               # UiOverlayHost / offscreen VkImage / Gizmo
```

- **Natural advantages of `projects/editor`**:
  1. **Zero-pollution isolation**: like [projects/template/Cargo.toml](../projects/template/Cargo.toml) uses independent `[workspace]`, doesn't enter root workspace, player projects unaware.
  2. **True engine self-hosting**: directly goes through `prism-app` + `prism-engine` main loop and render pipeline, naturally supports `prism-runtime-host` loading the edited game's `preview-module` (`.so`/`.dll`) for Play / Preview.
  3. **No limits on non-runtime dependencies**: since `projects/editor` is independent of player runtime chain, can confidently introduce mature ecosystem libs to replace hand-written desktop infrastructure (layout, text layout, clipboard, system file dialogs), focusing on editor business logic and reusable control composition:
     * **`taffy`** (or lightweight Flexbox kernel): drives split-screen Docking, Inspector rows / columns and adaptive panels.
     * **`slotmap`**: `SlotMap` + `SecondaryMap` flat storage and orthogonal behavior tables, zero `Rc<RefCell<T>>`.
     * **`cosmic-text`**: multi-line text layout, bidirectional text, font fallback and precise caret HitTest (with `prism-render` atlas cache uploading glyphs).
     * **`arboard` / `rfd` / `notify`**: cross-platform clipboard, native file dialogs, asset / dynamic-library hot-reload file watching.
  4. **Role of `prism-ui`**: only outputs platform-independent `UiDrawList` (rects, rounded borders, clip regions, glyph atlas, texture handle `UiTextureId`), batched to screen via `prism-render`'s `UiOverlay` / `SwapchainOverlay` (see [ui_host.rs](../crates/prism-render/src/graph_renderer/ui_host.rs)).

---

## 2. Core data model: SlotMap flat tree + stable NodeKey

Use integer handles instead of pointer references, completely eliminating self-referential lifetimes and borrow conflicts:

```rust
slotmap::new_key_type! { pub struct NodeId; }

pub struct UiTree {
    nodes: SlotMap<NodeId, UiNode>,
    key_to_id: HashMap<NodeKey, NodeId>, // Stable semantic ID (e.g. "field:3:Transform:position[1]")
    // Orthogonal behavior tables (SecondaryMap<NodeId, T>)...
}
```

### Key design points
1. **Dual-purpose `NodeKey`**: each editor node bound to semantic `NodeKey` (e.g. `panel:hierarchy`, `entity:3`, `field:3:Transform:position[1]`), used both for **inter-frame state retention** (animation interpolation, scroll offset, fold state, text caret, drag baseline) and directly as node ID in `prism-editor-mcp` automation protocol (`ui_tree.rs`), avoiding maintaining two trees.
2. **Unidirectional borrow flow**: event dispatch (HitTest → Capture → Bubble) and layout computation only read / write `UiTree` internal arrays; business callbacks emit via **event queue `Vec<UiAction>`**, no mutable closure callback references held during tree traversal, completely avoiding double mutable borrows.

---

## 3. Composition first: orthogonal behavior components (Behaviors) replace control inheritance

Split **appearance**, **interaction**, **data editing** into orthogonal lightweight data structures mounted in independent type tables (`SecondaryMap<NodeId, T>`):

| Orthogonal component (`SecondaryMap<NodeId, T>`) | Responsibility | Reuse scenarios |
|---|---|---|
| `VisualBox` | Background color, border, corner radius, shadow, theme token lookup by `InteractionState` | Almost all containers, buttons, input boxes, tabs |
| `Interactable` | Click / double-click detection, toggle state, key activation, pointer capture | Buttons, tree nodes, menu items, Splitter, Gizmo panels |
| `Focusable` | Tab order, focus ring drawing, shortcut scope isolation (block viewport WASD when typing) | Input boxes, numeric boxes, tree lists, panel containers |
| `TextEditable` | Caret, selection, undo, clipboard, IME preedit area | Search boxes, rename boxes, expression input, inline numeric editing |
| `DraggableValue` | Horizontal drag to change value (`Shift` fine, `Ctrl` step snap, boundary clamp) | `f32`/`i32` drag boxes, Slider, timeline cursor, Splitter |
| `Scrollable` | Viewport clipping, wheel offset, virtualized visible range `[start..end)` computation | Hierarchy, Asset Browser, Console, Inspector, dropdown lists |
| `DragPayload` / `DropTarget` | Unified drag payload (`Entity` / `AssetPath` / `DockTab`) initiation and receipt | Hierarchy drag reparenting, asset drag into viewport / fields, Dock tab drag-drop |

---

## 4. Reusable control library and composition matrix

Through **Primitives → Atoms → Molecules** three-level composition, maximize elimination of duplicate code:

### 4.1 Primitive layer
* `row` / `col` / `spacer` / `divider`
* `label` / `muted_label`
* `viewport(UiTextureId)` (zero-copy embedding offscreen `VkImageView`)
* `canvas(Vec<UiDrawCommand>)`

### 4.2 Interaction atom layer (same kernel, parameterized variants)
1. **`button_core` (unifies all button-like controls)**: combines `ElementBox` + `VisualBox` + `Interactable` + `Focusable`, covers normal buttons, toolbar icon buttons, Toggle, Tab pages and menu items via `toggle: Option<bool>`.
2. **`text_field`**: combines `ElementBox` + `VisualBox` + `Focusable` + `TextEditable`.
3. **`numeric_field` / `vec3_field`**: combines `VisualBox` + `DraggableValue` + lazily-switched `TextEditable`, single implementation covers `f32`/`i32` numeric boxes, Slider, `Vec2/3/4` and timeline cursor.

### 4.3 Composite container layer
1. **`virtual_list`**: combines `Scrollable` + visible-range clipping, directly drives Console, Asset Browser, Command Palette.
2. **`tree_view`**: built directly on `virtual_list`, only adds `Indent(depth)` + `ExpandToggle(toggle_button)`, drives Hierarchy, Asset directory tree, Prefab structure tree.

---

## 5. Data-driven Inspector and Undo/Redo composition

Relying on [crates/prism-editor-api](../crates/prism-editor-api/src/lib.rs) (`schema`, `property_drawer`, `reflect_edit`) and [crates/prism-reflect](../crates/prism-reflect/src/lib.rs):

1. **Single lookup mapping**: traverse entity reflection components' `FieldSchema` and `PropertyAttribute`, uniformly map to `numeric_field`, `vec3_field`, `toggle_button`, `text_field`, `button` (enum selection).
2. **Differential Undo/Redo**: control layer emits `FieldEdit { entity_id, component, field, axis, old_value, new_value }` on `ValueCommitted` into `UndoStack`.

---

## 6. World isolation and Render Graph multi-viewport pipeline

### 6.1 Three-world isolation model
* **`EditorWorld`**: holds editor's `UiTree`, editor camera, selection set and Gizmo state.
* **`SceneWorld`**: in-memory mirror of currently open level document (only runs `Transform` matrix propagation and render extraction, **skips gameplay / physics / AI systems**).
* **`PlayWorld`**: on Play click, rebuild independent `World` from `SceneWorld` via `prism-scene` / `RTTIBin` snapshot and activate full game systems; on Stop simply `drop(play_world)`.

### 6.2 Render Graph single-frame pipeline
```text
[SceneOffscreenPass] ──► OffscreenColor (VkImage)
                                │
[GizmoOverlayPass]  ────────────┼──► composited into same OffscreenColor
                                │
[UiCompositePass]   ◄───────────┘ (ImageViewport directly samples VkImageView, zero-copy)
        │
        └──► Swapchain Present (main window / independent VkSurfaceKHR for future multi-window)
```

---

## 7. Progressive migration path

> **Superseded by §9.8.** This section kept as original proposal record; under ECS authority (§9.1) actual phasing follows §9.8 —
> engine UI capability completion (Phase 0–1) must precede `prism-ui` convergence (Phase 2).

1. **Phase 1 — Render foundation alignment**: extend `UiOverlay` to support externally registered `OffscreenColor` `VkImageView` (reuse `UiOverlayHost`).
2. **Phase 2 — `prism-ui` kernel and primitives**: implement `UiTree`, orthogonal behavior tables, `button / text_field / numeric_field / virtual_list / tree_view` and unit tests.
3. **Phase 3 — `projects/editor` main loop runs**: start `editor-app` with standard Prisma game entry, integrate `prism-editor-core`, `prism-editor-api` and `prism-runtime-host`.
4. **Phase 4 — Multi-window extension**: extend `prism-platform` and `prism-render` to manage multiple `winit::Window` / `Swapchain` under one `VkDevice`.

---

## 8. Modern engine UI API and modular optional features

Continue the building-block Feature philosophy of [crates/prism-render/Cargo.toml](../crates/prism-render/Cargo.toml), all high-order visuals and animations as zero-intrusion optional Cargo Features:

| Feature switch | Capability | Underlying implementation and advantage |
|---|---|---|
| **`ui-blur`** | Frosted glass / Acrylic background (`.backdrop_blur(...)`) | Render Graph Dual-Kawase downsample blur on background texture + micro-noise banding prevention |
| **`ui-tween`** | Declarative Tween and physical Spring animation | Leverages `SlotMap` + stable `NodeKey` for cross-frame property interpolation |
| **`ui-effects`** | Analytic SDF soft shadows, inner / outer glow and custom `.slang` UI materials | Single-primitive SDF antialiasing, supports binding custom Slang shaders |
| **`ui-desktop`** | Full desktop features (multi-line layout, IME, clipboard, virtual list, Docking) | Enabled only by desktop tools like `projects/editor`, doesn't affect minimal player package |

---

## 9. UI authority: ECS (decision record and supplementary design)

> This section adds A/B decision from [issue #1254](https://github.com/prisma-rust/prisma/issues/1254) and
> boundary conclusions from #1252 / #1253, and corrects statements in §2–§4 about "two UI runtimes".

### 9.1 Decision: ECS is the sole authority

Engine already has a complete ECS UI: components defined in `crates/prism-scene/src/ui.rs`,
re-exported via `crates/prism-engine/src/ui/mod.rs:30-33`, 11 reflected components
(`Node` / `Anchors` / `Pivot` / `Margin` / `Layout` / `Style` / `ComputedLayout` /
`Text` / `Canvas` / `Interaction` / `Button`), driven by three systems
`ui_layout_system` → `ui_input_system` → `ui_render_system`
(registration order see `crates/prism-engine/src/engine.rs:796-808`), and written to `.scene.json`
via `#[derive(Reflect)]`.

**Conclusion**: this ECS UI is the authoritative data model. `prism-ui` **must not** own a parallel
`UiTree` / `LayoutStyle` / `UiDrawCommand` / hit testing; it only keeps two responsibilities:

1. **Declarative builder**: assemble author intent into `prism_scene::ui` components and spawn entities;
2. **Interaction state machine library**: text editing, scrolling, drag-to-change-value, drag-drop payload behaviors ECS doesn't yet have,
   attached to ECS as components / resources, not forming its own tree.

Reason for rejecting "two coexisting called sharing" is direct: both chains already implement layout solving, hit testing and draw emission,
with non-equivalent semantics (§9.2). Keeping both means permanently maintaining two truths.

**Current code is in worst state**: `editor_system` appended after engine default schedule
(`projects/editor/crates/editor-app/src/lib.rs:454`), and the last thing it does is use
`bridge::submit_draw_list` to overwrite the `UiDrawList` engine just built
(`lib.rs:445`). That means under editor ECS UI entities are entirely erased, and ECS
`ui_input_system` spins idle because it doesn't write `UiInputState`. This overwrite must be deleted.

### 9.2 Duplicate inventory (delete on convergence, not migrate)

| Concept | `prism-ui` | ECS | Disposition |
|---|---|---|---|
| Node identity | `NodeId` (`tree.rs:16-19`) | `Entity` + `Node` (`ui.rs:18`) | Delete, use `Entity` |
| Layout style | `LayoutStyle` / `Length` / `FlexDirection` / `Align` / `Justify` / `PositionType` (`style.rs:129-187`) | `Style` + `Layout` + `Anchors` + `Pivot` + `Margin` (`ui.rs:215-272`) | Delete, use ECS components |
| Layout solving | whole `layout.rs` (262 lines) | `ui_layout_system` (`ui/layout.rs:74-242`) | Delete |
| Interaction state | `InteractionState` (`style.rs:212-219`) | `Interaction` (`ui.rs:573-581`) | Delete, use ECS component |
| Stateful appearance | `VisualStyle` / `VisualBox` / `VisualPatch` (`style.rs:230-298`) | `Style` + `Button` (`ui.rs:234-272, 591-625`) | Delete |
| Draw commands | `UiDrawCommand` / `UiDrawList` (`draw.rs:1-86`) | `UiDrawItem` / `UiQuad` / `UiTextCmd` (`ui/render.rs:14-118`) | Delete |
| Hit and input routing | `hit_test` / `process_input` (`tree.rs:339-537`) | `ui_input_system` (`ui/input.rs:59-136`) | Delete |
| Draw emission | `emit_node_draw` (`tree.rs:546-644`) | `ui_render_system` (`ui/render.rs:138-237`) | Delete |
| Bridge layer | `editor-app/src/bridge.rs` (`bridge.rs:44-161`) | — | Delete, rewrite as "component writer" |

### 9.3 Capabilities ECS side must complete first

Each item below is a hard prerequisite for editor to run, and **not** something `prism-ui` can solve alone —
they belong to engine UI capabilities, player projects' UI also benefits. Ordered by dependency:

| # | Capability | Current status and evidence | How to add |
|---|---|---|---|
| 1 | **Character and IME input** | `PlatformEvent` only has `Key { key, pressed }` (`crates/prism-platform/src/event.rs:249,284`), no Character / Ime / Composition variants; `UiInputState` only has caret, left-button tri-state, key list (`ui/input.rs:22-34`) | Platform layer adds event variants → `InputManager` exposes character stream → `UiInputState` carries this frame's text; **foundation of all text editing** |
| 2 | **Clip region** | `UiDrawItem` only `Quad` / `Text` (`ui/render.rs:73-76`); GPU scissor set once per frame (`ui_overlay/gpu.rs:779,1081`) | Add clip range to draw command, or per-quad `clip_rect`; GPU switches scissor per item |
| 3 | **Border stroke** | `Style` only has `border_radius` (`ui.rs:265`), no border width / color | Add stroke fields to `Style`; render side uses inset quad or SDF |
| 4 | **Line segment** | No `Line` command (`ui/render.rs:73-76`) | Add `Line` variant (dividers, Gizmo guides, animation curves) |
| 5 | **Scrolling** | `prism-scene/src/ui.rs` has no scroll field; `UiInputState` doesn't carry wheel delta | `Scrollable` component (offset + content size) + wheel routing |
| 6 | **Drag payload** | Only `UiPressState { entity }` (`ui/input.rs:40-43`), no payload type | `DragPayload` / `DropTarget` components + events |
| 7 | **Virtualization** | `ui_render_system` traverses all `ComputedLayout + Style` (`ui/render.rs:152-192`) | Depends on #5 scroll offset, filter by visible range |
| 8 | **Layout grow / percent / wrap** | `Layout` explicitly doesn't do grow/shrink/weights (`ui.rs:200-204`); `Style::width/height` only `Option<f32>` (`ui.rs:236-241`) | Add grow weights and wrapping to `Layout`; change `Style` sizes to `Length` enum |
| 9 | **Semantic stable ID** | Sibling order relies on `SceneEntityId` (`ui/layout.rs:336-342`), no cross-frame semantic primary key | See §9.5 |

**Note priority of #1**: without character events, `text_field` can't receive input no matter how implemented.
Current `bridge.rs:36-37` always passes empty string for `text_committed`, `false` for `backspace_pressed`,
direct symptom of this missing foundation.

### 9.4 What `prism-ui` keeps

| Kept item | Location | Reason |
|---|---|---|
| Declarative builder (`ElementBox` + control composition matrix) | `elements.rs:125-330, 375-631` | Author-experience layer ECS hand-written spawn can't replace; transform into direct writer of `prism_scene::ui` components |
| `DraggableValue` drag-to-change state machine | `behavior.rs:69-105` | Shift fine / Ctrl snap / clamp, no ECS counterpart |
| `Scrollable::visible_range` / `clamp` | `behavior.rs:116-148` | Virtualized range algorithm, absent in ECS |
| `TextEditable` state machine | `behavior.rs:151-199` | Caret / selection / backspace, absent in ECS (but must merge with `prism-editor-core::textinput`, see §9.6) |
| `DragPayload` enum | `behavior.rs:45-51` | Unified cross-panel payload (Entity / AssetPath / DockTab) |
| `tween.rs` interpolation | `tween.rs:1-177` | No ECS counterpart; note currently dead code (`animate_scalar` no caller) |
| `NodeKey` semantic primary key | `tree.rs:22-92` | Need to change to string grammar, see §9.5 |

**Existing defects to fix together** (otherwise copied verbatim into new architecture):

1. `bridge.rs:151` drops `Line` / `PushClip` / `PopClip`, making `clip_children` completely ineffective;
2. `elements.rs:489-504` `text_field` displays builder input, not `TextEditable.text`,
   so even if input works image won't change;
3. Virtual list uses input `scroll_offset_y` for range (`elements.rs:574-577`), never synced with retained
   `Scrollable.offset`;
4. `InteractionState.selected/disabled` never set, `UiAction::TextCommitted` never constructed,
   `Focusable.tab_index` / `captures_text_keys` write-only, `ui-desktop` feature zero references.

### 9.5 Editor-specific entities and MCP node contract

**Editor-specific entities** (from #1254 acceptance): editor UI entities must carry an `EditorUi` marker,
and be excluded in two places:

- **Save**: skipped when writing `.scene.json`. Can reuse same approach as `DERIVED_COMPONENTS`
  (`crates/prism-scene/src/json.rs:60-66`) — there `ComputedLayout` already defined as
  "runtime output, author doesn't write" derived component.
- **Player build**: `check_deps.py` guarantees player chain doesn't link editor crates; marker ensures even if entity
  serialized into scene it won't be rendered as normal UI on player side.

**MCP node id** (promise from #1254 `NodeKey`): currently **not true**.
`prism-ui`'s `NodeKey` is `u64` hash (`tree.rs:22-44`),
`inspector.rs:57-69` hashes `field:{entity}:{component}:{field}[{axis}]` string then
**discards original string**; `snapshot_debug_nodes` (`tree.rs:646-652`) only returns `u64`,
no reverse mapping. MCP side uses string grammar (`editor/crates/prism-editor-egui/src/ui_tree.rs`):

| Prefix | Semantics |
|---|---|
| `command:<Name>` | Menu / shortcut command |
| `panel:<tab>` | Dock panel |
| `doc:<index>` | Open document |
| `entity:<id>` | Scene entity |
| `asset:<path>` | Asset entry |
| `field:<entity>:<component>:<path>` | Inspector field |
| `view:<tab>:entity:<id>` | Object inside viewport |
| `dialog:<kind>[:<button>]` | Dialog |

**Requirement**: change `NodeKey` to string grammar itself (or keep string + stable hash dual views),
grammar definition in `prism-editor-core` (UI-framework-independent, already dependable by `crates/prism-editor-mcp`),
becoming **single source of truth** for MCP, builder, cross-frame state retention. Don't maintain two ids.

### 9.6 Division of labor with `prism-editor-core`

`prism-editor-core` confirmed UI-framework-independent (`Cargo.toml:8-16` only
`serde` / `serde_json` / `log` / `image` / `glam` / `prism-ini`, whole-dir
grep `egui::` zero hits). So "binding `prism-editor-core`" in §1 table holds.

Two duplicates needing merge:

- **Text model**: `prism-editor-core::textinput` (`textinput.rs:12-206`, supports selection,
  word-wise movement, Unicode-safe) and `prism-ui::TextEditable` (`behavior.rs:151-199`,
  only insert / backspace) are two sets. Former more complete, and **currently unused** — egui editor always uses
  `egui::TextEdit`. On convergence use core's as authority, delete `prism-ui`'s.
- **Panel id registry**: `ui_tree::panel_title` hardcodes English table, inconsistent with
  `Workspace::panel_title` (i18n), causing some tabs' labels in MCP snapshot to
  degrade to `"Panel"`. On convergence use core's as authority.

### 9.7 Engineering boundary supplement (from #1252 / #1253 / #1254)

1. **CI coverage**: `projects/editor` currently only has `Cargo.lock` consistency check in `scripts/pr_ready.py:401-402`,
   **not in** `scripts/check_features.py` feature matrix. `prism-ui` unit tests and `cargo check` need to enter local gate.
2. **Project boundary**: editor still hardcodes `projects/template` / `template.exe`
   (`editor/crates/prism-editor-core/src/build.rs`), project root discovered by walking cwd upward looking for
   `assets/` (`editor/crates/prism-editor-egui/src/app.rs:218`).
   When self-hosted editor lands **must** change to explicit project path parameter, don't copy this assumption.
3. **Composition root**: `prism-app` stays library form, no `main`; `projects/editor` as independent
   workspace composition root,
   isomorphic to `projects/template` (#1252).

### 9.8 Revised phasing

§7 phasing needs reordering under ECS authority:

| Phase | Content | Prerequisite |
|---|---|---|
| **Phase 0** | Platform character / IME events + `UiInputState` text stream | None |
| **Phase 1** | ECS UI capability completion: clip, border, Line, Scrollable, DragPayload, virtualization, grow / percent | Phase 0 (scrolling and text editing depend on input) |
| **Phase 2** | `prism-ui` downgrade: delete §9.2 duplicates, `bridge.rs` becomes component writer, delete draw-list overwrite at `lib.rs:445` | Phase 1 |
| **Phase 3** | Panel migration: use `prism-editor-core` dock / undo / i18n as data source, move panels off egui one by one | Phase 2 |
| **Phase 4** | Multi-window (`prism-platform` + multiple swapchains on one `VkDevice`) | Phase 3 |

Phase 0–1 are **engine capabilities**, player projects benefit too, should land before editor migration;
only after Phase 2 `projects/editor` has conditions to replace `editor/`.

---

## 10. References

- [`project-module-architecture.md`](project-module-architecture.md) — Host boundary and preview module ABI
- [`editor-egui.md`](editor-egui.md) — Current egui editor (migration source)
- [`editor-mcp.md`](editor-mcp.md) — MCP automation contract
- [`REFLECT.md`](REFLECT.md) — Component reflection and RTTIBin
- [`scripting-layer.md`](scripting-layer.md) — Scripting layer also uses ECS + `HostApi` as boundary
- issue #1252 / #1253 / #1254 — Composition root, SDK boundary, UI authority

