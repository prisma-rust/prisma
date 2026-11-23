# Editor MCP server

`crates/prism-editor-mcp` exposes editor data layer (`prism-editor-api`) as MCP tools,
letting agents modify scenes, run games, capture screenshots, without manually editing JSON then re-cooking.

It **does not link** engine / render, nor open Vullan: reads/writes go through `prism-editor-api` reflection
validation; when visuals are needed, `screenshot` starts a subprocess.

## Startup

```bash
cargo run -p prism-editor-mcp -- --project projects/demos/fishing
```

Project root can also be specified via `PRISMA_PROJECT`; if neither given, walks up from cwd looking for `assets/`.
Transport is stdio, one JSON-RPC message per line; handshake, protocol version negotiation, and notifications are handled by
[`rmcp`](https://docs.rs/rmcp); this crate only provides tool table and call dispatch.

Add in `.mcp.json` (example):

```json
{
  "mcpServers": {
    "prisma-editor": {
      "command": "cargo",
      "args": [
        "run", "--quiet", "-p", "prism-editor-mcp", "--",
        "--project", "projects/demos/fishing"
      ]
    }
  }
}
```

## Tools

The two tables below together form the **complete** tool list: this table is document / resource / game process tools,
egui editor tools are in the "[egui editor](#egui-editor-editor)" section. Parameters with `?` are optional, others required.

| Tool | Parameters | Purpose |
|------|-----------|---------|
| `documents_list` | `kind?` (`scene` / `prefab` / `all`) | List scenes / prefabs (project-relative paths) |
| `document_read` | `path` | Read full document (migrated to current format, entities carry stable `id`) |
| `document_write` | `path` `doc` | Validate and write back in whole; `doc` is `{ version, entities }` |
| `schema_list` | `component?` | All components + nested struct fields, enum variants, read-only marks |
| `entity_get` | `path` `entity` | Get entity by `id` or name |
| `field_set` | `path` `entity` `component` `field` `value` | **Reflection write** of a field (`a.b` / `list[0].x` both supported) |
| `component_add` | `path` `entity` `component` | Add component (default values from schema) |
| `component_remove` | `path` `entity` `component` | Remove component |
| `asset_inspect` | `path` | Inspect cooked assets (`RSKL`/`RSKN`/`RANI`/`RMES`/`RTEX`/`RMAT`): count, summary, check problems |
| `asset_validate` | `path` `skeleton?` | Run validation only (weight sums, joint bounds, index / submesh interval, keyframe time and order), `ok=true` means passed |
| `game_run` | `command?` `env?` | Start game (default `cargo run --release`), logs go to buffer |
| `game_stop` | — | End process started by `game_run` / `screenshot` |
| `game_logs` | `lines?` | Read log tail (stdout + stderr, default 200 lines) |
| `screenshot` | `path` `size?` `scale?` `frame?` `timeout_ms?` `command?` `env?` | Start game → capture screenshot → end, return PNG path |

### Why `field_set` is safe

Writes go through `prism_editor_api::reflect_edit::set_component_field`:
`from_object` restores component → `Reflect::set_field` writes field (`#[reflect(read_only)]`
rejected, type validated via reflection) → `object_to_json` restores authoring JSON. So illegal types, unknown
enum variants, read-only fields never hit disk.

### `screenshot`

Parameters: `path` (required), `size` (`1920x1080`), `scale` (window multiplier), `frame`
(default 120), `timeout_ms`, `command`, `env`.

It reuses engine's built-in screenshot (`PRISMA_SCREENSHOT*`, see `AGENTS.md`), so it captures
**the frame on screen**; scaling is nearest-neighbor, pixel art scaling doesn't blur.

`command` defaults to `cargo run --release`, executed in project root. To skip cargo startup overhead,
pass binary path directly, e.g.
`"command": "./target/release/fishing"`.

## Boundaries

- Only data + process, no rendering. To change renderer behavior, go back to engine crate.
- Does not implement resource / prompt / sampling; besides `tools/list` only `ping`.
- No concurrency protection: don't open two agents writing the same document simultaneously (`document_write` is whole-file overwrite).

## egui editor (`editor/`)

egui editor is an **independent process** (Rust + egui-ash); window screenshot and viewport zero-copy image can only be taken
in its own process, so it opens a local control channel, MCP issues commands through it:

```text
editor starts → listens on 127.0.0.1:<random port> → writes <project root>/target/editor-egui/control.port
MCP tool       → reads port file → TCP one-line JSON request / one-line JSON response
```

- Only listens on loopback; port file deleted when editor exits (`PRISMA_EDITOR_CONTROL` can override to port number).
- Requests handled on UI thread, so screenshot and UI state are **same frame**.
- Failures go through MCP `isError`, text is control channel's raw JSON response, carrying `kind`
  (`bad_request` / `not_ready` / `denied` / `internal`) and `message`:
  `not_ready` wait and retry, `bad_request` fix parameters, `denied` do not retry.
- Editor started with `--no-control` (or turn off channel in Preferences) won't generate port file,
  all `editor_*` tools report "cannot read control port".

| Tool | Parameters | Purpose |
|------|-----------|---------|
| `editor_launch` | `command?` `wait_ms?` | Start `editor/target/{release,debug}/prism-editor-egui` (falls back to `cargo run` if not built), wait for control channel ready |
| `editor_status` | — | FPS, current scene, selected entity, Play / pause, unsaved mark, project root |
| `editor_capture` | `target`(`window`/`scene`/`game`) `path?` `size?` | Capture whole window (UI + viewport) or a viewport engine offscreen image, return PNG path |
| `editor_ui` | `action`(`snapshot`/`click`/`move`/`wheel`/`drag`/`set`) `node?` `offset?` `tab?` `x?` `y?` `delta?` `from?` `to?` `steps?` `mode?` `ctrl?` `value?` | UI node tree: snapshot / click node / viewport pointer move, scroll, drag / set node value |
| `editor_play` | `action`(`play`/`stop`/`pause`) | Control Play (Play triggers project module compile + load) |
| `editor_logs` | `since?` `limit?` | Read Console buffer: engine logs + project module `project:` logs + panic call stack |
| `editor_input` | `key?` `mouse?` `state?` `click?` | Inject input into Play (one event per frame), `click=true` left-click once |
| `editor_quit` | — | Normal exit path (writes `state.json`) |

`editor_status`'s `project_module` reports Game View's user project module:
`available` / `loaded` / `building` / `auto_refresh` / `state` (`ready` / `pending` /
`compiling` / `failed` / `unavailable`) / `dirty` / `problems` / `manifest` / `error`.
Module compilation and reload are done by editor itself (see `docs/editor-egui.md` §3.2).

### UI node tree (`editor_ui`)

egui has no DOM, nor externally clickable control handles, so node tree comes from **data model** +
dock panel rectangles recorded each frame, `click` / `set` walk the **exact same code path as UI** (`Command` /
`Workspace::select` / `Workspace::set_field`), no synthetic mouse events injected.

`snapshot` returns: `dock` (one node per tab, active one expands child nodes, with
`bounds`), `documents`, `assets`, `commands`, and active panel's child nodes
(`hierarchy` → entities, `inspector` → component fields, `log` → recent 50 lines).

| Node id | Example | `click` | `set` |
|---------|---------|---------|-------|
| `command:<Name>` | `command:Save` | ✅ | — |
| `panel:<tab>` | `panel:hierarchy` | ✅ (switch tab if open, open if not) | — |
| `doc:<index>` | `doc:0` | ✅ | — |
| `entity:<id>` | `entity:3` | ✅ (select) | — |
| `asset:<path>` | `asset:assets/prefabs/a.prefab.json` | ✅ | — |
| `field:<entity>:<component>:<path>` | `field:3:Transform:position[1]` | — | ✅ |

`field`'s `path` is component JSON path: objects use dot notation (`position.x`), arrays give both whole
(`position` = `[x,y,z]`) and index (`position[1]`). When path doesn't exist, `ui.set` will
**error and list available paths** (`Workspace::set_field` silently returns if it can't find field).
Write-back goes through `Workspace::set_field`, so it's a single undoable field modification.

`target=window` uses Windows `PrintWindow` (`PW_RENDERFULLCONTENT`): samples after DWM composition,
so Vulkan swapchain content and UI are captured together, window works even offscreen; other platforms
only support `scene` / `game` (engine offscreen image, no window needed). Editor menu
"Window → Capture Window" is the same implementation's local shortcut.
