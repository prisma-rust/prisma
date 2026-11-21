# Editor UI — Functional Requirements (new editor)

> Status: requirements baseline for `projects/editor`. Supersedes nothing yet; it is the
> functional contract that the phased plan in [`editor-self-hosted-ui.md`](editor-self-hosted-ui.md)
> §9.8 implements. Architecture stays in that document; this one states **what the editor must do**,
> with numbered, testable requirements.

## 1. Scope

The new editor is the self-hosted Prisma editor: a Prisma application (`projects/editor`) that
edits scene documents, prefabs, assets and render settings, and can play a user project's
`preview-module` in-place. It replaces `editor/` (egui) panel by panel; every requirement below is
written against the replacement, not against the current shell.

Out of scope for this document: MCP tool contracts ([`editor-mcp.md`](editor-mcp.md)), the
reflection/binary layout ([`REFLECT.md`](REFLECT.md)), and the player-chain architecture
([`DESIGN.md`](DESIGN.md)).

### 1.1 Verified baseline

Every requirement below is measured from this state. Facts verified against code; the
architecture document's §9.3 gap list still holds in full.

| Area | Current state |
|---|---|
| UI component set | `Node`, `Anchors`, `Pivot`, `Margin`, `Layout`, `Style`, `ComputedLayout`, `Text`, `Canvas`, `Interaction`, `Button` — no scroll, drag, border, stroke or semantic-id component (`crates/prism-scene/src/ui.rs`). |
| Layout solver | Two-phase measure/arrange with anchor+margin+layout-group semantics (`crates/prism-engine/src/ui/layout.rs`). No grow, shrink, percent, wrap, or absolute positioning. |
| Input | `ui_input_system` does hover/press/click/keyboard activation only. Wheel and modifier deltas are collected but never read; no text, no IME, no focus traversal. |
| Draw | `UiDrawItem` is `Quad` or `Text`; the overlay sets one scissor per frame, so per-item clipping does not exist. |
| Editor shell | `projects/editor` is a fixed three-pane layout with a working menu bar, toolbar and schema-driven inspector. No dock tree, no documents, no console, no settings, no viewport render, no play mode. |
| Data layer | `prism-editor-core` (dock, doc, undo, schema, project, assets, theme, i18n, keys, palette, settings, recovery) is complete and UI-framework-free, but `projects/editor` does not depend on it yet. |

The last row is the single largest structural gap: the shell must consume the core crate instead of
re-deriving undo, schema and entity models locally.

## 2. Hard constraints

- **C-1** The editor is a composition root in its own workspace; the player chain
  (`template → prism-app → prism-engine → {scene, ecs, render, asset, physics2d}`) gains no editor
  or UI-framework dependency (`scripts/check_deps.py`).
- **C-2** UI authority is the ECS: `Node`, `Anchors`, `Pivot`, `Margin`, `Layout`, `Style`,
  `ComputedLayout`, `Text`, `Canvas`, `Interaction`, `Button` (`crates/prism-scene/src/ui.rs`).
  The editor may add components and behaviours; it may not add a second layout/hit-test/draw stack.
- **C-3** Editor-only UI entities carry a marker that keeps them out of `.scene.json` and out of
  player builds.
- **C-4** No compatibility layer: the new editor does not preserve egui-era widget APIs, config
  formats, or saved layouts. State that cannot be re-derived is re-authored.
- **C-5** Every interactive surface is reachable from the command registry; a mouse-only control is
  an incomplete control.

## 3. Editor shell

### 3.1 Window chrome

| ID | Requirement |
|---|---|
| ED-UI-001 | The window has, top to bottom: menu bar, toolbar, dock area, status bar. Only the dock area may be resized by the user. |
| ED-UI-002 | The menu bar opens on press and stays open while the pointer travels between sibling menus; `Esc` closes without activating; a second click on an open menu closes it. |
| ED-UI-003 | Menu items support: disabled state, checkmark, radio dot, shortcut hint column, submenu arrow, separators, and nested submenus of arbitrary depth (measured against the parent's edge so a nested menu never leaves the window). |
| ED-UI-004 | The toolbar exposes: transform tool (move/rotate/scale), world/local space toggle, pivot mode toggle, snapping toggle plus snap step, play/pause/step, and the project-module build status with its compile action. |
| ED-UI-005 | The status bar shows: current selection count, scene dirty state, project module state, background job progress, and the active play-mode state with a colour cue (green running, amber paused). |
| ED-UI-006 | Play state is visually unmistakable: a coloured border on the viewport and a banner in the inspector stating that runtime edits are discarded on stop. |

### 3.2 Docking

| ID | Requirement |
|---|---|
| ED-UI-010 | Panels live in a split tree. Dropping a dragged tab on a leaf edge splits in that direction; dropping on the leaf centre merges into a tab group; dropping outside the dock area floats the panel. |
| ED-UI-011 | Splitters resize with a minimum panel extent; a panel below its minimum scrolls its content rather than clipping labels mid-glyph. |
| ED-UI-012 | Layout presets: named layouts can be saved, loaded, deleted, exported to and imported from a file, and reset. Presets are stored with the editor state, never inside a project. |
| ED-UI-013 | Floating panels obey the same theme, focus and command routing as docked panels. |
| ED-UI-014 | Tab order restores across restarts; a panel whose document disappeared opens empty instead of failing startup. |

### 3.3 Theming and language

| ID | Requirement |
|---|---|
| ED-UI-020 | Every colour, radius, gap and font size comes from a theme token table; no panel hard-codes a colour. |
| ED-UI-021 | At least a dark and a light theme, both meeting 4.5:1 contrast for body text. |
| ED-UI-022 | The UI language switches at runtime (English and Simplified Chinese at minimum) without restarting; dates, numbers and file sizes are formatted per locale. |
| ED-UI-023 | UI scale follows the display scale factor; text stays crisp at 1.0–2.0×. |

## 4. Panels

### 4.1 Hierarchy

| ID | Requirement |
|---|---|
| ED-UI-100 | Displays the scene as an indented tree with per-row: expander, visibility toggle, icon, name, and a marker for prefab instances. |
| ED-UI-101 | Search filters by name (case-insensitive, substring) and dims non-matching branches while keeping ancestors visible. |
| ED-UI-102 | Drag-and-drop reorders: top 25 % inserts before, bottom 25 % inserts after, the middle reparents; the target highlights the resulting drop zone before release. |
| ED-UI-103 | `F2` (and `Return` on macOS) starts in-place rename with select-all; `Esc` cancels; `Enter` commits as one undo step. |
| ED-UI-104 | Double-click frames the selected object in the scene view. |
| ED-UI-105 | Multi-select is supported; operations act on the whole selection in one undo step. |
| ED-UI-106 | Context menu: create empty, create child, create parent, duplicate, delete, rename, copy, paste, unpack/apply prefab. |
| ED-UI-107 | Rows are virtualized: a 20 000-entity scene scrolls at the display refresh rate. |

### 4.2 Scene view

| ID | Requirement |
|---|---|
| ED-UI-110 | Orbit (drag), pan (middle drag or `Alt`+drag), zoom to cursor (wheel), and frame-selection are always available. |
| ED-UI-111 | Two display modes: perspective and orthographic; the axis gizmo in a corner snaps the camera to any world axis. |
| ED-UI-112 | Picking resolves to the deepest drawn object under the cursor; repeated clicks at the same position cycle through occluded candidates. |
| ED-UI-113 | Gizmos render for the whole selection; move/rotate/scale drags are one undo step each; snapping respects the toolbar step. |
| ED-UI-114 | Pivot mode switches between selection centre and individual pivots. |
| ED-UI-115 | Selection highlight is drawn without introducing a new render pass concept: grid, sky, icons, 2D grid and camera-frustum overlays are individually toggleable. |
| ED-UI-116 | A viewport toolbar offers grid, sky, 2D, snapping and the camera field of view / clip planes, without leaving the viewport. |
| ED-UI-117 | The rendering resolution scale (0.5×–1.5×) affects only the offscreen target, never the UI layout. |

### 4.3 Game view

| ID | Requirement |
|---|---|
| ED-UI-120 | Renders the project at a chosen resolution: match window, a fixed size, or a locked aspect ratio with letterboxing; the default comes from the project's window settings. |
| ED-UI-121 | Play runs the project's own module; Input focus, pause, single-step and stop are driven from the toolbar and the keyboard. |
| ED-UI-122 | Stopping restores the pre-play snapshot: document, undo stack, selection and dirty flags. |
| ED-UI-123 | Game-view render settings come from the project's own settings, not from the editor's scene-view settings. |

### 4.4 Inspector

| ID | Requirement |
|---|---|
| ED-UI-130 | Shows the active object: identity header (name, activation toggle, tag, layer, static flags), then component cards in a stable order with the transform card first. |
| ED-UI-131 | Multi-select shows the union of shared components; editing a shared field applies to every selected object in one undo step; the last-clicked object supplies displayed values. |
| ED-UI-132 | Component card actions: copy values, paste values, reset to schema default, move up/down, remove, presets (save/apply), and per-field copy/paste/property-path. |
| ED-UI-133 | Locking the inspector pins it to one object by identity; the lock clears when that object is deleted. |
| ED-UI-134 | Components without a schema fall back to an editable raw-JSON block with apply/reload and inline parse errors. |
| ED-UI-135 | Prefab instances show a prefab section: open, revert, apply, apply-to-all-instances, save-as-prefab, plus a breadcrumb back to the scene that opened them. |
| ED-UI-136 | Asset fields offer a picker filtered by the field's expected asset type, show the current asset's name and type, and accept drag-and-drop from the project browser. |
| ED-UI-137 | Every edit marks the document dirty immediately and coalesces continuous drag edits into a single undo entry on commit. |

### 4.5 Project browser

| ID | Requirement |
|---|---|
| ED-UI-140 | Two-pane layout: folder tree on the left, contents on the right; back/forward history and a breadcrumb path. |
| ED-UI-141 | Views: list and grid, with adjustable item size; directories are selectable rows like files. |
| ED-UI-142 | Search supports type filters (`t:texture`, `t:audio`, `t:scene`, …) and folder filters (`dir:sprites`); multiple filters AND together, multiple types OR together. |
| ED-UI-143 | Operations: create folder, create asset, rename, duplicate, delete (folders only when empty), copy path, reveal in file manager, refresh. |
| ED-UI-144 | Assets drag into the hierarchy (instantiate), into the viewport (place at the cursor), into inspector asset fields (assign), and onto folders (move, rewriting references). |
| ED-UI-145 | Assets have a stable identity (GUID sidecar) so rename/move never breaks references; references that cannot be resolved are marked broken, not silently dropped. |
| ED-UI-146 | Selecting an asset shows its details in the inspector: type, size, identity, reference count, preview, and import settings where the importer supports them. |
| ED-UI-147 | Thumbnails decode in the background with a per-frame budget; unsupported or corrupt assets show a type icon and are not retried every frame. |
| ED-UI-148 | Selected assets can ping their first referencing object in the scene, and the scene can ping the asset. |

### 4.6 Console and diagnostics

| ID | Requirement |
|---|---|
| ED-UI-150 | Console shows time, level, source and message, with filters per level and per source, a clear action, collapse of repeated lines with a count, and follow-tail. |
| ED-UI-151 | Project-module compile diagnostics appear as `file:line:col` rows in a problems list with both error and warning severity; activating a row opens the source at that location. |
| ED-UI-152 | Project-module log output and panics are bridged into the console (never to a hidden process stderr). |
| ED-UI-153 | Game-view tick failure is latched: the module stops being ticked until recompiled or reloaded, and the reason is shown once, not once per frame. |

## 5. Inspector field specification

### 5.1 Field widget matrix

| ID | Requirement |
|---|---|
| ED-UI-170 | Editor builds the widget from the reflected type and its drawer attributes: numbers → draggable numeric field (drag to change, click to type, `Shift` fine, `Ctrl` snap), ranges clamp, integers step by 1. |
| ED-UI-171 | Vectors expose per-axis fields with coloured axis labels; scalar drag on any axis field edits that axis only. |
| ED-UI-172 | Enums render as a dropdown; flags render as a multi-toggle; booleans as a checkbox; strings as a single-line field, becoming multi-line when the drawer requests it. |
| ED-UI-173 | Colours open a picker with alpha and HDR affordances matching the drawer's declared usage. |
| ED-UI-174 | Asset paths render as a field plus a picker button; object references render as an object slot with a clear action. |
| ED-UI-175 | Lists render one row per element with add/remove/move-up/move-down; fixed-size arrays deny add/remove; nested structs and options are collapsible (an option owns an enable checkbox). |
| ED-UI-176 | Read-only fields render dimmed and refuse edits, including keyboard entry. |
| ED-UI-177 | Field layout: label column left-aligned and elided with a tooltip, control column right-aligned; rows never reflow when a value's text length changes. |

### 5.2 Rect transform card

| ID | Requirement |
|---|---|
| ED-UI-180 | Shows position (X, Y, Z), size (width, height), anchor min/max, pivot, rotation and scale, in that order, with a reset control that restores the anchor-preset defaults. |
| ED-UI-181 | Anchor editing offers both numeric fields and an anchor-preset popup; presets snap anchor min/max and pivot together. |
| ED-UI-182 | Editing width/height on a stretched axis converts the axis to point anchoring explicitly and is one undo step, or is refused with a reason — it must never silently change the other axis. |
| ED-UI-183 | Position and size units are design pixels inside a canvas; the card displays the computed screen rect and scale as read-only derived values. |

### 5.3 Canvas and renderer cards

| ID | Requirement |
|---|---|
| ED-UI-190 | A canvas card shows reference resolution, scale mode (constant pixel size / scale with screen size / match width or height), and the width-or-height match factor. |
| ED-UI-191 | A renderer card exposes a single transparent-mesh culling toggle and explains its effect in a tooltip. |

### 5.4 Image card

| ID | Requirement |
|---|---|
| ED-UI-200 | Source slot: texture or sprite asset, with the current asset previewed in the slot and a picker that accepts drag-and-drop. |
| ED-UI-201 | Colour (tint) field, material slot, raycast-target toggle, maskable toggle, and a raycast-padding control that is only enabled when raycast targeting is on. |
| ED-UI-202 | Image type: simple, sliced (9-slice with per-edge borders), tiled, filled (horizontal/vertical/radial, with origin and clockwise flag). |
| ED-UI-203 | Fill amount is a 0–1 slider plus numeric field; the preview updates while dragging. |
| ED-UI-204 | Options: use sprite mesh, preserve aspect, and a "set native size" action that writes the source's pixel size into the rect. |
| ED-UI-205 | Default material row shows the assigned material and its shader, with an edit action that opens the material asset. |

### 5.5 Layout element card

| ID | Requirement |
|---|---|
| ED-UI-210 | Toggles: ignore layout (excluded from the parent's arrangement), and a layout priority that orders which element's preferred size wins when several compete. |
| ED-UI-211 | When the parent is a layout group, the card additionally exposes minimum/preferred/flexible sizes per axis; the fields are hidden for non-group parents rather than shown disabled forever. |
| ED-UI-212 | Values edited here immediately re-solve the parent's arrangement; the resulting rect is visible without a frame of lag. |

## 6. UI capability requirements (engine side)

These are prerequisites the editor consumes; each one also benefits player projects.

### 6.1 Layout algebra

| ID | Requirement |
|---|---|
| ED-UI-220 | Sizing units: auto, content-fit, pixels and percentage, per axis; unspecified means auto. |
| ED-UI-221 | Flex: grow, shrink and basis per axis with justify (start/centre/end/space-between/space-around) and cross-axis alignment (start/centre/end/stretch). |
| ED-UI-222 | Layout groups: row, column, wrap; gaps and padding; content-fit collapse; children of a group ignore their own anchors. |
| ED-UI-223 | Display (visible/hidden-but-takes-space/none) and absolute positioning (offsets instead of flow). |
| ED-UI-224 | Canvas scaling: a reference resolution maps design pixels to screen pixels with three documented modes; nested canvases inherit the nearest one. |
| ED-UI-225 | Layout is deterministic: the same document and viewport size produce byte-identical computed rects across runs and platforms. |

### 6.2 Drawing

| ID | Requirement |
|---|---|
| ED-UI-230 | Primitives: solid quad, rounded rect, border stroke (inner or SDF), line segment, image (simple/sliced/tiled/filled), text run. |
| ED-UI-231 | Per-item clip rectangles compose with nesting; GPU scissor changes only where the clip actually changes. |
| ED-UI-232 | Masks: rectangular and shape masks clip descendants; mask edges are antialiased, not stair-stepped. |
| ED-UI-233 | Text uses a shared atlas; multiple font faces in one view still batch into one draw call where possible. |
| ED-UI-234 | Optional visual effects (backdrop blur, soft shadow, tweened properties) are behind default-off features. |

### 6.3 Interaction and text

| ID | Requirement |
|---|---|
| ED-UI-240 | Platform input exposes key down/up, pointer move/button/wheel, text composition and IME events; a text field that cannot receive characters is a defect, not a limitation. |
| ED-UI-241 | Focus model: one focused element, tab order, focus ring, and scope isolation so typing into a field never reaches the viewport's camera controls. |
| ED-UI-242 | Text editing: caret by click/drag, selection by shift, double-click word, triple-click all, clipboard, undo per field, and Unicode-safe movement. |
| ED-UI-243 | Scrolling: wheel and scrollbar, momentum optional, clamped, with a virtualized visible range for long lists. |
| ED-UI-244 | Drag payloads are typed (entity, asset path, dock tab) and validated by the drop target; an incompatible drop shows a rejected cursor. |
| ED-UI-245 | Interaction states (normal/hover/pressed/disabled) are driven by the input system and are serializable with the document. |

## 7. Data, commands and recovery

| ID | Requirement |
|---|---|
| ED-UI-250 | Documents: scene, prefab, and settings; each tracks external modification by file stamp and blocks saving over a change it did not make, offering overwrite/reload/cancel. |
| ED-UI-251 | Undo/redo is field-merged, per document, bounded, and survives selection changes without applying edits to the wrong object. |
| ED-UI-252 | A command registry owns id, title, shortcut, enablement and handler; the menu, toolbar, context menus and command palette all read from it. |
| ED-UI-253 | A command palette searches commands by title and shortcut and executes them. |
| ED-UI-254 | Crash recovery: an autosave copy is written on a timer; on restart the editor offers the newest copy and never overwrites the on-disk document without consent. |
| ED-UI-255 | MCP automation addresses nodes with the stable string grammar (`panel:`, `entity:`, `field:`, `asset:`, `view:`, `dialog:`, `command:`); one id, one meaning, no hash-only handles. |

## 8. Performance budgets

| ID | Requirement |
|---|---|
| ED-UI-260 | UI layout and draw emission for a 5 000-node editor tree stay under 2 ms per frame on the reference desktop GPU. |
| ED-UI-261 | Idle cost is zero: no viewport re-render, no thumbnail decode and no asset scan when nothing changed. |
| ED-UI-262 | Scrolling the hierarchy or project browser does not allocate per row per frame beyond the visible range. |
| ED-UI-263 | Startup to an interactive window with the last project under 3 s on warm caches, excluding the first IBL prefilter which is reported separately. |

## 9. Delivery phases

Phase identifiers follow [`editor-self-hosted-ui.md`](editor-self-hosted-ui.md) §9.8.

| Phase | Requirements satisfied |
|---|---|
| P0 — input foundation | ED-UI-240, ED-UI-241 (events and focus only) |
| P1 — engine UI capability | ED-UI-220 … ED-UI-234, ED-UI-242 … ED-UI-245 |
| P2 — UI kernel consolidation | ED-UI-001 … ED-UI-023, ED-UI-170 … ED-UI-177, ED-UI-255 |
| P3 — panel migration | ED-UI-100 … ED-UI-153, ED-UI-180 … ED-UI-212 |
| P4 — multi-window | ED-UI-010 … ED-UI-014 (floating window host) |

## 10. Acceptance

A phase is done when every requirement it claims has an automated check (unit, golden, or
scripted automation through MCP) or a recorded manual verification with a screenshot, and when
`python scripts/pr_ready.py` passes. A requirement with neither evidence nor an explicit
"not verifiable here" note is open.

## 11. Open questions

1. Whether the image card's material slot is a full material asset or a shader override — decide
   before P3 starts, because it changes the asset importer contract.
2. Whether the anchor-preset popup writes pivot as well as anchors by default; the two behaviours
   differ for stretch presets and need one answer.
3. Whether the command palette ships in P3 or waits for the settings overhaul.
