# 2D pixel fishing game (Cozy Fishing) design

> Status: M0 landed (2026-11), code in `projects/fishing/`.
> This document keeps design records of gameplay and engine contracts; API names / editor forms
> in early drafts have been superseded by later implementation, current conventions follow `docs/DESIGN.md`, repo-root `AGENTS.md` and code.
> Target project: `projects/fishing/` (independent workspace, same level as `projects/template`)
> Engine changes follow `docs/DESIGN.md` architecture layering: write spec for new APIs first, semantic equivalence prioritized.

---

## 1. Goals and non-goals

### M0 vertical slice (only deliverable of this phase)

A **3-minute playable** single-scene slice:

1. Top-down 2D lake surface, boat slowly drifting on water;
2. Cast → bobber lands → wait → bite prompt → reel minigame → fish caught;
3. Caught fish enter catalog, HUD shows prompts and catch count;
4. Ambient audio (water / birdsong) + cast / splash / reel sound effects;
5. Day-night slowly changes with time (global tint + sky gradient).

**Not doing**: networking, multiple scenes, quest system, economy, skeletal animation, physics engine integration.

### Non-goals (explicitly excluded to avoid scope creep)

- No C# scripting system.
- No 3D scene viewport and terrain brush in M0 (re-evaluate in M2).
- No second ECS for pixel art; sprites remain normal components.

---

## 2. Layered contract (hard boundary)

This boundary is the core constraint of this work: **engine only provides general capabilities, gameplay exists only in user project**.

| Layer | Location | Allowed | Forbidden |
|---|---|---|---|
| Engine runtime | `crates/prism-*` | General components, general systems, render passes, asset import, audio / save / time services | Any concrete gameplay concepts like fish species, fishing, boat, level, HUD layout |
| Editor data layer | `crates/prism-editor-api` | Project registry into property panel schema, document read / write | Linking engine / render / Vulkan |
| Editor UI | `editor/` (egui) | Panels, drag-drop, preview | Hardcoding some game's business fields |
| User project | `projects/fishing/` | Fishing state machine, fish data, boat control, HUD, level content, audio manifest | Direct `vk::*`, directly modifying render graph, bypassing `ComponentRegistry` to attach components |

Test: **switch to another game (e.g. farming game), can it be reused as-is?** Yes → engine; no → user project.

Component naming follows existing engine vocabulary: `SpriteRenderer`, `Sprite`, orthographic camera, `AudioSource`, `TimeOfDay`.

---

## 3. Engine-side gaps and API design

### 3.1 2D render pipeline (biggest gap)

Current: only 3D forward PBR (`forward_pass.rs` + `scene_frag.slang`), `Camera` already supports orthographic projection
(`orthographic` / `orthographic_size`, see `crates/prism-scene/src/components.rs`),
but no sprites, no 2D sorting, no pixel-perfect sampling.

#### Coordinates and camera conventions

Follow existing project conventions (right-handed, camera looks −Z, Vulkan y-flip):

- 2D scene camera placed at **+Z looking toward −Z**, sprites at `z ≈ 0`;
- Larger `z` is closer to camera (same as 3D);
- World unit ↔ pixel determined by **PPU (pixels per unit)**:
  `world_size = texture_size / pixels_per_unit`;
- Pixel-perfect: `orthographic_size = render target height / (2 * ppu)`, sampler uses `NEAREST`.
- **Internal render resolution** (must do in M0.2, already landed): pixel art rendered at 640×360 then integer-upscaled,
  otherwise 32px sprite occupies only 3% of screen height at 1080p. Now configured via `assets/settings.ini`
  `[render] internal_resolution` + `internal_scale_mode = integer` (#613):
  ForwardPass HDR target allocated at internal resolution, PostPass nearest-upscales to swapchain.

#### New component `SpriteRenderer` (`prism-scene`)

```rust
pub struct SpriteRenderer {
    pub sprite_path: String,      // Asset path
    pub frame_size: [u32; 2],     // [0,0] = whole texture; otherwise sprite-sheet frame size
    pub frame_index: u32,         // Driven by SpriteAnimation or game code
    pub pixels_per_unit: f32,     // Default 100
    pub pivot: [f32; 2],          // 0..1, default [0.5, 0.5]
    pub color: [f32; 4],          // Vertex color multiply, default white
    pub flip_x: bool,
    pub flip_y: bool,
    pub sorting_layer: i32,       // Sorting layer number
    pub order_in_layer: i32,      // Order within layer
}
```

Runtime GPU handles in `SpriteRef` (`prism-engine/src/scene/runtime_components.rs`, isomorphic with `MeshRef`/`MaterialRef`):

```rust
pub struct SpriteRef {
    pub texture: AssetTextureHandle,
    pub size: [u32; 2],           // Texture pixel size (filled after upload)
    pub generation: u32,          // 0 = not parsed
}
```

#### Asset parsing

`AssetResolveRequest` adds `texture: Option<TextureUploadInput>`,
`AssetResolveResult` adds `texture_handle: Option<AssetTextureHandle>`;
`AssetResolver::prepare_requests` submits requests for entities holding `SpriteRenderer` with `SpriteRef.generation == 0`.
Textures go through existing `TextureImporter` (PNG → RGBA8) and bindless slots.

#### `SpritePass` (`prism-render/src/sprite_pass.rs`)

- Single instanced draw: unit quad + per-instance SSBO
  `{ model: mat4, uv_rect: vec4, color: vec4, texture: u32, _pad }`;
- Sampling via **set 1 bindless** (same layout as `scene_frag.slang`), sampler `SamplerType::Nearest`;
- Blend `SRC_ALPHA / ONE_MINUS_SRC_ALPHA`, **depth test and write disabled** (2D relies on CPU sorting for order);
- Sort key `(sorting_layer, order_in_layer, z, entity)`, stable sort in `SceneReadView::sprite_items()`;
- Draw target: HDR scene color (before `PostPass`), so `PostPass` needs new **`tonemap_mode = 2` (None / passthrough)**
  to keep pixel-art palette unmodified by ACES;
- 2D scenes don't create `Skybox` / `Gizmo` entities, camera `clear_flags = SolidColor`.

#### 2D lighting (`Light2D`, #611)

Sprites by default **unlit** (`texture × color`, pixel palette output as-is). For day-night changes:

- Place `prism_engine::scene::Light2D` entities in scene (`kind = Point | Ambient | Directional`);
- `SpriteRenderer` needing light enables `receive_lights`;
- Light intensity is **artistic units**: `1.0` = original color, `0.5` = half dark, no physical unit conversion,
  so changing same scene from "day" to "dusk" only needs `intensity` / `color` changes.

Point lights attenuate by world XY distance (`radius = 0` means no attenuation). Ambient light often used for
night minimum brightness: `color = [0.2, 0.25, 0.5]`, `intensity = 0.35`.

3D `PointLight` / `SpotLight` / `DirectionalLight` **don't participate in 2D**; when scene has only 3D lights
engine logs one warning pointing to `Light2D`, instead of silently having no effect.

#### 2D frame pass clipping (later optimization, doesn't block M0)

`RenderMode` adds `Sprites2D`: skips shadow / GTAO / GI / PT / forward. M0 accepts "empty forward pass" cost,
left as M1 optimization item.

### 3.2 Sprite animation

```rust
pub struct SpriteAnimation {
    pub frames: u32,        // Sprite-sheet horizontal frame count
    pub fps: f32,           // 0 = manual
    pub looped: bool,
    pub playing: bool,
}
```

Engine system `sprite_animation_system` accumulates time each frame and writes `SpriteRenderer.frame_index`
(`SpriteAnimation` is pure data, holds no time state, time kept in system-side `Resource` to avoid polluting scene format).

### 3.3 Line rendering (fishing line / bobber trail)

```rust
pub struct LineRenderer {
    pub points: Vec<[f32; 3]>,   // World-space polyline
    pub color: [f32; 4],
    pub width: f32,              // World units
    pub sorting_layer: i32,
}
```

`LineRenderer` triangulated on CPU into **non-overlapping** strokes (in 2D scene a widened polyline on XY plane,
supports miter / bevel joints and butt / square / round endpoints; #604),
merged into `SpritePass` instance buffer (same pipeline, white 1×1 texture). Avoids new pass.
Widths under 1 render pixel drawn at 1 pixel (#978) — sub-pixel-wide triangles let pixel centers miss sampling,
appearing as dotted line on screen; engine converts this lower bound by main camera and scene render height.

### 3.4 Audio

Current: `prism-audio` only has 2D one-shot playback, no loop, no channels, no spatialization.

New (M0 does minimal set):

- `AudioData` looping: `play_looping` / `AudioClip { looped }`;
- Named channels (`master` / `music` / `ambience` / `sfx`) with independent volume;
- 2D spatialization: left-right pan by `x` distance + distance attenuation (enough for pixel games, no HRTF).

### 3.5 Time and day-night

```rust
pub struct TimeOfDay {
    pub day_length_seconds: f32,   // 0 = frozen
    pub time: f32,                 // 0..1, 0 = midnight, 0.25 = sunrise, 0.5 = noon
    pub start_time: f32,
    pub paused: bool,
}
```

Engine system `time_of_day_system` advances `time` and drives:

- `DirectionalLight` (3D scene) direction and color;
- 2D scene global tint: add `tint` field to `FrameUBO`, or implement via `PostPass` color grade (M0 uses latter).

### 3.6 Save files

```rust
pub struct SaveGame { /* versioned JSON, under user data dir */ }
impl SaveGame {
    pub fn load(name: &str) -> Self;
    pub fn save(&self, name: &str) -> Result<()>;
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T>;
    pub fn set<T: Serialize>(&mut self, key: &str, value: T);
}
```

Only key-value + version migration hooks; doesn't touch scene serialization (that's `scene_state.rs`'s job).

### 3.7 input

M0 reads `InputManager` directly (`InputSnapshot` delivered from host to project module); gamepad / virtual joystick later.

---

## 4. Editor side

| Item | M0 needs | Notes |
|---|---|---|
| Component inspector | Auto | Schema appears automatically once component registered in `ComponentRegistry` |
| Texture asset recognition | Yes | Asset manager lists `.png` and allows drag to `sprite_path` field |
| Sprite preview thumbnail | Yes | Inspector shows texture preview + frame index slider |
| 2D scene viewport | No (M2) | M0 arranges scenes in Play mode |
| Sorting layer naming | No | M0 uses numbers |

Editor still doesn't link engine: sprite preview reads files via `prism-editor-api` and presents via frontend `<img>`, no render dependency.

---

## 5. User project `projects/fishing/`

```
projects/fishing/
  Cargo.toml            # Independent workspace, depends on prism-app / prism-engine
  src/
    main.rs             # Desktop entry (build_app().run())
    lib.rs              # build_app + Android android_main
    game/
      mod.rs
      fishing.rs        # Cast / wait / bite / reel state machine
      boat.rs           # Boat drift and camera follow
      fish_db.rs        # Fish species data (JSON-driven)
      hud.rs            # Prompts, catalog count
      ambience.rs       # Ambient audio and day-night linkage
  assets/
    sprites/            # Pixel images (PNG)
    audio/              # wav/ogg
    scenes/lake.scene.json
    fish.json
```

Gameplay state machine (user project only):

```
Idle ──press cast──> Casting ──splash──> Waiting ──bite──> Biting
  ^                                                      │
  └────────── reel success / fail <── Reeling <── press reel ──────┘
```

---

## 6. Milestones

| Milestone | Content | Exit criteria |
|---|---|---|
| **M0.1** | Sprite pipeline: `SpriteRenderer` + `SpriteRef` + `SpritePass` + `sprites.slang` + tonemap None | Can render a PNG in scene, orthographic camera, nearest sampling |
| **M0.2** | Project scaffolding: `projects/fishing` + lake scene + boat and camera follow | Double-click runs, boat moves on water |
| **M0.3** | Line rendering + fishing state machine + HUD | Can complete one "cast → catch" loop |
| **M0.4** | Audio loop / channels + day-night | Has ambient audio, sky color changes with time |
| **M0.5** | Save + catalog | Catalog persists after restart |
| **M1** | Content expansion, 2D pass clipping, sprite sheet editing, more fish species | 20+ species, stable 60fps |

---

## 7. Art specifications

M0 uses **640×360 virtual resolution + multi-layer parallax + 4 time-of-day variants per layer**, instead of "one big background image +
global tint". Internal render resolution therefore moved from M1 to M0 must-do.

| Item | Spec |
|---|---|
| Background layer | Width 500–4000 px, height 300–430 px, arranged far-to-near by parallax factor ×0.04–×1.05 |
| Fish | Tiered sizes 14 / 16 / 20 / 30 / 42 px, 4–12% of viewport height, must render low-res then upscale |
| Layers | Independent X / Y parallax per layer; name `<layer>@<dawn|day|dusk|night>` |
| Day-night | 4 textures per layer whole-layer replacement, no global tint |
| Fish species | `<Species>_<Tier>` + `<Species>_UI`; two swim frames |
| Audio | Four buses: Master / Music / UI / WorldSFX |
| Fonts | LanaPixel (CC-BY 4.0), Silkscreen, credited in acknowledgments |

### 7.1 Placing background textures by crop blocks

Each background texture is a rectangular **crop** of one art piece in the level, hard edge on texture boundary. When placing, must push
the cropped side off-screen, otherwise a straight vertical edge appears in image:

| Criterion (alpha image) | Meaning | Placement |
|---|---|---|
| Leftmost column fully opaque | Left side cropped | Align left edge to screen left (left edge x < 0) |
| Rightmost column fully opaque | Right side cropped | Align right edge to screen right (right edge x > viewport width) |
| Bottom row fully opaque | Bottom cropped | Sink bottom below waterline, covered by opaque water surface |
| All four edges have transparency | Complete prop (rock / cloud / fish) | Place freely |

- Reverse also causes trouble: placing these mountains mid-screen exposes crop edge as a "rectangle" — once misjudged as
  **sprite alpha loss**, actually alpha intact throughout (26% fully transparent pixels still in RTEX, single layer rendered alone
  shows correct cutout), just wrong geometry position.
- Each layer's real transform is in level MonoBehaviour (no typetree, unreadable beyond `m_Name`),
  so `x/y` can only be inferred by above rule, can't expect exact reproduction.
- Water surface must be **opaque** dark block: semi-transparent water over bright sky becomes gray-white fog band, looking like
  broken alpha.

## 8. Risks and tradeoffs

1. **Empty forward pass cost**: M0 accepts, M1 adds `RenderMode::Sprites2D`.
2. **Linear-space blending**: sprites blend in HDR linear space, looks different from classic 2D engines (sRGB blending);
   M0 accepts first, if color deviation obvious introduce separate 2D target.
3. **No sprite asset format**: M0 uses "path + frame size + frame index", no `.sprite.json`;
   introduce when editor needs slicing (and write migration).
4. **Animation**: M0 only frame animation; character / dog skeletal animation is separate large project, not in M0.
