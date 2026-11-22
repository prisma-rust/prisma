# Scripting layer (`scripting`)

> **Status (2026-11-21): design contract, not implemented.** This document only fixes the seams and phase criteria,
> makes no commitment on language landing schedule, and does not mean relevant APIs have changed.

## 1. Decisions and scope

**Target form**: user project gameplay can be written in scripts, scripts and Rust project modules share the **same host boundary**,
instead of each having their own cross-language protocol.

**Why writing this document now**: #1253 A/B decision discussed "can users build standalone player without installing Rust toolchain",
conclusion is "unless engine embeds script runtime AND user project also uses scripts, not considered". This document concretizes that
precondition: what the script layer looks like, where the seam is, what the cost is, when it's worth doing.

**Non-goals**:

- Not deciding Lua vs C# scheduling order beyond that.
- Not introducing new cross-language ABI (see §2).
- Not changing 1.0 determination criteria (see §9).
- Not committing to C# support on Android (see §7, it's a separate go/no-go).

## 2. Why scripts are not a new ABI

`HostApi` (`crates/prism-project-api/src/lib.rs:573`) is already a complete script host function table.
Its current consumer is Rust preview modules, but the expressiveness it needs is exactly the same as a script runtime:

| Script capability | `HostApi` field |
|-------------------|-----------------|
| Create / destroy entities | `spawn_entity` / `despawn_entity` / `is_alive` |
| Enumerate entities by type name | `query_entities(type_name, len)` |
| Read/write components | `get_component` / `set_component` / `remove_component` |
| Logging | `log` |
| Asset reading | `asset_exists` / `read_asset_file` |
| Save/load | `save_value` / `load_value` / `save_scene_state` |
| Scene switching | `load_scene` / `unload_scene` |
| Physics query | `raycast` / `query_colliders2d` |
| Audio | `play_sound` / `stop_sound` |
| UI drawing | `draw_quad` |

**Decisive point**: component read/write is passed by **type name string + RTTIBin byte stream**
(`GetComponent` / `SetComponent` signatures see `crates/prism-project-api/src/lib.rs:509`),
not Rust generics. Therefore the script side **does not need bindings generated for each component** — component set is decided at runtime by
`ComponentRegistry`, script sees field names and values, not types.

This property must be maintained. Any design making component read/write "regenerate bindings for each component added"
will make script layer maintenance cost grow linearly, directly vetoed.

## 3. Reflection side is already sufficient

`prism-reflect` provides the dynamic access path that scripts need:

- `Value` / `Object` (`crates/prism-reflect/src/lib.rs:96,214`): field name → value generic representation;
- `TypeRegistry::from_object` / `to_object` (`crates/prism-reflect/src/registry.rs:32,138`):
  conversion between dynamic object and concrete Rust type;
- `set_field` rejects `read_only` fields, `set_field_force` for deserialization use;
- Field names are `.scene.json` field names (after `#[reflect(rename = ...)]`),
  so script API, scene document, and inspector panel all use the same names.

RTTIBin format has complete specification in [`REFLECT.md`](REFLECT.md) §RTTIBin format (v1);
script side implementing a reader is limited workload, and does not need engine-side cooperation.

## 4. The only gap: dynamic component access is private

`GameWorld`'s public API only has generic versions:

```rust
pub fn get<T: Component + Reflect + Clone>(&self, entity: EntityHandle) -> Option<T>
pub fn set<T: Component + Reflect>(&mut self, entity: EntityHandle, value: T) -> bool
```

While dynamic path by type name `fn get(&self, entity, type_name) -> Option<Vec<u8>>` (`crates/prism-game-api/src/lib.rs:1138`)
and `fn set(&self, entity, bytes)` (`:1161`) are **private**.

Script layer must take the dynamic path. Therefore the **minimum language-independent preparation** is to lift dynamic access to public API,
and return `Object` instead of raw bytes:

```rust
pub fn get_object(&self, entity: EntityHandle, type_name: &str) -> Option<Object>
pub fn set_object(&mut self, entity: EntityHandle, object: &Object) -> bool
```

Requirements:

1. **Both backends implemented** (`Backend::Host` with `Backend::Plugin`). Host backend directly borrows
   `World`, Plugin backend walks `HostApi`; same signatures are `prism-game-api`'s existing contract.
2. **No new dependencies**, no change to existing generic API behavior.
3. Codec between bytes and `Object` completed inside `prism-game-api`, script layer does not touch RTTIBin.

This step is not wasted regardless of whether Lua, C#, or neither is ultimately chosen — it is the criterion itself of "whether scripts can reuse existing boundary".

## 5. Lua track

**Form**: in-process VM, compile-time feature brick, `Subsystem::Script`.
`Subsystem` already has "placeholder but unimplemented" precedent (`Network` / `AI` in
`crates/prism-app/src/app/mod.rs:595` walking `warn` branch), can follow the same approach.

**Toolchain constraints (measured and corrected)**: player dependency tree **already** contains C toolchain —
`cc` and `pkg-config` enter `cargo tree -p prism-app -e normal,build` via `zstd-sys` (`.pak` decompression in `prism-asset`).
Therefore the concern that "introducing C-implemented Lua VM adds new compiler dependency" does not hold;
NDK side clang is already needed for zstd.

Thus selection:

| Option | Conclusion |
|--------|------------|
| `mlua` + vendored PUC Lua 5.4 | **Recommended**. No new toolchain; Lua 5.4 semantics stable; Android via NDK clang |
| `mlua` + LuaJIT | Not recommended. JIT restricted on aarch64 Android and needs extra backend, benefit doesn't match |
| Pure Rust VM (`piccolo`) | Alternative. Saves C, but standard library coverage and maturity insufficient, fallback not primary |

**Hot reload**: this is Lua's real benefit over Rust preview module. `docs/editor-egui.md` §3.2 currently says
"no script layer to migrate state, so copy uniformly uses reload, not hot reload"; after script layer exists,
editor can achieve **state-preserving script reload** instead of whole-module replacement + project side static state reset.
This benefit needs separate acceptance, cannot default "having scripts equals having hot reload".

## 6. C# track

**Product requirements** (from owner): no outdated Mono; latest language features; editor-side method-level hot reload
instead of full domain reload; optional NativeAOT at build time.

**Conclusion: these four points are mutually self-consistent, and there is only one way to split.**

| Requirement | Fact | Landing form |
|-----------|------|--------------|
| Not Mono | Modern path hosts .NET runtime from native code (`nethost` / `hostfxr`) | Editor CoreCLR hosting |
| Method-level hot reload | CoreCLR Hot Reload (`MetadataUpdateHandler`) can change method bodies, add methods/types; only rude edits need restart | Editor JIT runtime |
| Build-time optional NativeAOT | NativeAOT has no runtime codegen, **in principle cannot do method-level hot reload** | Release products use NativeAOT |
| Latest features | Follow current LTS / STS `net` TFM | Both same source, only release mode different |

So the split is **forced**, not preference: editor = CoreCLR + JIT + hot reload; release = NativeAOT.
Any attempt to satisfy both with one product is not viable.

**Two risks to acknowledge in advance**:

1. **Android has no clean modern path**. Official NativeAOT platform table (.NET 8 and .NET 9+ columns consistent)
   marks Android as *"Experimental, no built-in Java interop"*. While this repo's Android contract is
   monolithic `.so` + GameActivity (see [`project-module-architecture.md`](project-module-architecture.md) §7).
   **This is an independent decision, cannot be promised incidentally.**
2. **Size**. NativeAOT minimum product is MB scale, while engine `dist` tier is 2.89–4.47 MB
   ([DESIGN.md](DESIGN.md) §12.4). C# can only be a **default-off opt-in brick**,
   and must go through §12.4 budget measurement, not estimation.

**Engineering focus** is on two places, neither is architectural risk: bind `HostApi` via `LibraryImport` source-gen
(structs must be blittable, `bool` in `InputSnapshot` encoded as `U1`);
implement RTTIBin reader on C# side (format see [`REFLECT.md`](REFLECT.md)).

## 7. Platform and size constraints

- Script runtime is a **brick**: `script-lua` / `script-csharp`, default off, enters
  [DESIGN.md](DESIGN.md) §12.2 table, §12.4 budget, and `scripts/check_features.py`
  minimum combination matrix (§12.3 hard rule 1).
- When off, `log::warn!` must explain which feature is missing at startup/use (§12.3 hard rule 3),
  consistent with existing `Subsystem::Audio` practice.
- Script layer **does not enter player chain's UI dependency**: it is a `prism-app` subsystem,
  `scripts/check_deps.py` blacklist and `cargo tree | grep -i egui` must remain empty.

## 8. Phases and go/no-go

| Phase | Content | Criterion |
|-------|---------|-----------|
| **P0 Seam preparation** | `GameWorld::get_object` / `set_object` public, both backends implemented | No script dependency involved; Lua and C# can reuse same channel |
| **P1 Lua** | `script-lua` brick, `Subsystem::Script`, `HostApi` Lua binding, state-preserving reload | A real project writes playable gameplay in Lua; budget accounted |
| **P2 C# editor** | CoreCLR hosting, `HostApi` binding, `MetadataUpdateHandler` hot reload | Change a method body without restarting editor |
| **P3 C# release** | NativeAOT product + size measurement | Size acceptable before entering `dist` tier table |
| **P4 Android C#** | Independent go/no-go | Default conclusion is **No**, unless official support graduates from experimental |

**P0 can be done immediately**, independent of language selection. Decide P2 timing after P1.

## 9. Relationship with 1.0

[`versioning.md`](versioning.md) §1 specifies 1.0 threshold is **public game API stability**
(`prism-game-api` / `prism-project-api` no longer frequently changing).

Script layer will significantly expand public API surface. Therefore only two legal routes:

1. Finalize before 1.0 and freeze together;
2. Explicitly exclude script layer from 1.0 scope, introduce as breaking change after 1.0.

Per AGENTS.md §3 zero-compatibility burden principle, **cannot ship a version then change it**.

## 10. References

- `crates/prism-project-api/src/lib.rs` — `HostApi` table, `GetComponent` / `SetComponent` signatures, capability bits
- `crates/prism-game-api/src/lib.rs` — `GameWorld` host / plugin dual backend, dynamic access current state
- `crates/prism-reflect/src/` — `Value` / `Object` / `TypeRegistry`
- `crates/prism-app/src/app/mod.rs` — `Subsystem` enum and placeholder branches
- [`REFLECT.md`](REFLECT.md) — RTTIBin v1 format
- [`project-module-architecture.md`](project-module-architecture.md) — host boundary, P3/P4 status
- [`DESIGN.md`](DESIGN.md) §12 — brick mechanism and budget
