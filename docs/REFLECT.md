# Reflection layer and RTTIBin

Prisma separates "tool format" from "runtime format": RTTIBin carries a complete schema, used by editors,
standalone players, and external tools; shipped RSCN stores only type id + schema fingerprint, with field layout
from `TypeDesc` compiled into the runtime. Both share the same reflection description.

## Layering

```
prism-reflect-derive   #[derive(Reflect)]  (proc-macro, compile-time only)
        │
        ▼
prism-reflect          Reflect / ReflectType / TypeDesc / Value / Object
        │              TypeRegistry (id → description + dynamic construction)
        │              rtti_bin (binary of type table + object table)
        ▼
prism-scene            #[derive(Reflect)] components + ComponentRegistry
                       RSCN v4 (type_id + schema_hash + position encoding)
        │
        ▼
prism-cooker           SceneCooker (.scene.json → RSCN)
prism-engine           SceneLoader (RSCN → ECS)
```

- Reflection layer **does not depend on egui**, nor on `prism-engine` / `prism-render`;
  `prism-engine` depends on it, and user projects (`game`) indirectly obtain it via engine.
- `prism-reflect` does not depend on `serde`. The authoring format still uses JSON + serde; RTTIBin is the self-describing
  exchange format between editors, standalone players, and external tools.

## Stable type id

`TypeId` is not `std::any::TypeId` (changes every compilation), but:

```text
fnv1a64(canonicalized type name)
```

The canonicalized name defaults to `module_path!()::TypeName`. Components use
`#[reflect(name = "prism_engine::scene::Camera")]` to fix the name in the scene format,
ensuring consistency among cook / runtime / editor.

**Renaming = changing type**. When old data needs to be retained, migrate in the loader by `TypeTable` name;
do not reuse the old id.

## Adding a new reflectable component

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::MyComponent", version = 1)]
pub struct MyComponent {
    pub speed: f32,

    // Runtime cache: does not participate in reflection, retains Default during deserialization.
    #[reflect(skip)]
    pub cache: u64,

    // Editor-not-writable; deserialization can still restore (via set_field_force).
    #[reflect(read_only)]
    pub id: u32,

    // Field names in schema can be independent of Rust field names.
    #[reflect(rename = "radius")]
    pub range: f32,
}
```

Then:

```rust
registry.register_reflect::<MyComponent>();
```

Field types must implement `ReflectType`:

- Native numeric / `bool` / `String`
- `Vec<T>` / `Option<T>` (`T: ReflectType`)
- Structs with `#[derive(Reflect)]`
- `glam::Vec2/Vec3/Vec4/Quat` (built-in implementations, fixed names `glam::Vec3` etc.)

Nested structs / glam types need to additionally register **type descriptions** (not components):

```rust
registry.register_reflect_type::<glam::Vec3>();
```

`register_builtin_components()` has already registered the glam types used by built-in components.

## Semantic conventions

- `Reflect: Default`. `from_object` starts from `Default`:
  - Missing field → retains default value (old data loaded with new code);
  - Unknown field → ignored (when new data is loaded with old code, the caller decides whether to reject per `TypeTable`);
  - Type id mismatch → `ReflectError::TypeIdMismatch`.
- `set_field` rejects `read_only` fields; `set_field_force` ignores this restriction, for deserialization use.
- One reflect name can only correspond to one Rust type: `TypeRegistry::register` panics directly when the same name is already occupied by **another** Rust type (re-registering the same type is idempotent). Otherwise previously written data would be decoded by the same-named new type into an instance with all-default fields, with no error at all.
- `to_object()` field order is determined by `BTreeMap`, so the encoding result is deterministic.
- **Transform is a structural field, not a mountable component**: the author only writes `EntityJson::transform`, and the loader / cooker bakes it into `LocalTransform` + `LocalToWorld` (the author-side `Transform` cannot be removed, `LocalTransform` belongs only to the runtime). Therefore `LocalTransform` only registers a type in `TypeRegistry`, not in `ComponentRegistry`'s mountable list; if `.scene.json`'s `components` contains it, it falls back to `transform` during migration (`json::DERIVED_COMPONENTS`), and manually constructed documents are rejected by `SceneJson::validate()`. Otherwise `registry.apply` would overwrite `LocalTransform` after `LocalToWorld` is computed, leaving a world matrix derived from the old transform.
- **UI layout output is analogous**: `ComputedLayout`'s `rect` is rewritten every frame by `prism-engine`'s `ui::layout`, and the component itself is inserted by that system as needed; the author only needs `Node` + `Style`. It is also in `json::DERIVED_COMPONENTS`, so old values in documents are stripped during migration and will not leave a dummy field that is always overwritten in the inspector.

## RTTIBin format (v1)

Little-endian. Full definition see `crates/prism-reflect/src/rtti_bin.rs`.

```text
magic   "PRTB" (4)
version u16 = 1
types   u32
  id u64 | kind u8 | name str | version u32 | size u32 | align u32
  fields u16
    name str | field_type
objects u32
  type_id u64 | fields u16
    name str | value
```

- `str` = `u32` byte length + UTF-8.
- Type table sorted by `TypeId`; object fields sorted by name → identical input bytes are determined.
- `field_type` tag: `0..=11` native types, `12` = `Struct(TypeId)`, `13` = `List`, `14` = `Optional`, `15` = `Array`.
- `value` tag: `0` None, `1` Bool, `2` I64, `3` U64, `4` F64, `5` Str, `6` List, `7` Object.
- Decoding has depth (64) and count limits; corrupted data returns `RttiError` instead of panicking.

### Encode/decode API

```rust
let bytes = rtti_bin::encode(&objects, registry.type_registry())?;
let decoded = rtti_bin::decode(&bytes)?;        // No need to register any type

for object in &decoded.objects {
    registry.apply_object(&mut world, entity, object); // Needs registration
}
```

Editors can just `decode` + read `TypeTable` to display properties, without depending on the game side's component registry;
players look up `ComponentRegistry` by `TypeId` to restore into ECS components.

## Status and roadmap

Completed:

- Reflection derive, registry, RTTIBin encode/decode, and unit/integration tests.
- `TypeDesc::schema_hash()`: layout-independent schema fingerprint (type name + version + field names/types/order), used for version checking of compact runtime formats.
- Reflection of built-in components: `Active`, `Name`, `LocalTransform`, `Camera`,
  `FlyCameraController`, `DirectionalLight`, `PointLight`, `SpotLight`,
  `Skybox`, `EnvironmentLighting`, `GiConfig`.
- `ComponentRegistry::apply_object` / `extract_object`, can read/write components
  without knowing the concrete type.

### RSCN v4: Runtime format vs tool format separation

RTTIBin is **exchange/tool format**: complete type table + field names, for editors,
standalone players, and external asset tools to read. It **does not enter the runtime hot path**, nor is it stored repeatedly per scene.

RSCN v4 runtime payload only retains:

```text
type_id u64 | schema_hash u64        // Each scene lists each used type only once
component payload                    // By TypeDesc field order, without field names/types
```

When loading, recalculate `schema_hash` using `TypeDesc` compiled into the binary:

- Match → decode by position (still via `set_field`, not relying on Rust memory layout);
- Mismatch → hard error (immediately reports error, refuses to load corrupted or outdated binary scenes; migration only happens in authoring format `.scene.json`); editors/tools can use self-describing RTTIBin format.

This way the runtime does not carry redundant schema; the tool side can still read without compile-time types. `TypeId` only hashes the type name and cannot replace `schema_hash`.

### Who is responsible for cook

Responsibilities split by "schema / build time / runtime":

- **`prism-scene`**: component definitions, `ComponentRegistry`, `.scene.json` authoring format, RSCN v4 encode/decode. This is the only schema shared by cooker and runtime; it does not depend on render/audio/asset IO.
- **`prism-cooker`**: Build-time orchestration. `SceneCooker` parses and validates `.scene.json`, calls `prism-scene::rscn::encode_scene` to produce RSCN v4; also responsible for general importer/cooker of textures/meshes etc., asset database and `prism-cook` CLI.
- **`prism-build-pipeline`**: Batch processing entry (GI bake, scene cook, heightmap). The `cook` subcommand directly calls `SceneCooker`, without bringing the cooker chain into the game runtime.
- **`prism-engine`**: Only responsible for runtime decoding. `SceneLoader` uses the same `ComponentRegistry` to restore RSCN into ECS components, and afterwards injects GPU bindings such as `MeshRef` / `MaterialRef`.
- **Editor**: Does incremental cook / pre-PIE cook through the same registry.

`prism-asset` only retains runtime `.pak`, `AssetId`/`Handle`, `ResourceManager`, no longer containing cooker/importer/db/CLI. `prism-cook` handles non-scene assets; scene assets are handled by `prism-build-pipeline build-scenes` (or single-scene `cook`) or the editor calling `SceneCooker`, ultimately all landing in `assets/scenes.pak`.

### Positioning of serde

Components still `derive(Serialize, Deserialize)` for the `.scene.json` authoring format and
`ComponentRegistry::register_reflect_serde`'s JSON → `Object` conversion.
RSCN's runtime payload only goes through reflection field order, without writing field names; RTTIBin is what retains complete field names and the type table.

