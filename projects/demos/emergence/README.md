# Emergence

`Emergence` is a **v3.0 hardcore sandbox tagging and physics simulation project** built on Prisma engine (independent workspace, at `projects/demos/emergence/`).

- For full engine evolution plan and architecture mapping see [`DESIGN.md`](DESIGN.md).
- v3.0 five-dimension tags (base environment state enum, intrinsic material bits, transient hazard bits, spatial topology bits, external stimulus and dynamic clock) 16-byte compact memory kernel implemented in [`src/tags.rs`](src/tags.rs).

## directory structure

```text
projects/demos/emergence/
    Cargo.toml              # Independent workspace (player + preview-module dual forms)
    DESIGN.md               # v3.0 sandbox development plan around Prisma engine
    README.md               # This document
  assets/
        settings.ini          # Window and 320x180 pixel rendering config
        scenes.json           # Scene manifest (lab)
    scenes/
            lab.scene.json      # Initial lab orthographic camera scene (v3)
  src/
        main.rs               # Desktop executable entry
        lib.rs                # Player build_app / Android JNI / editor preview-module entry
        tags.rs               # v3.0 panorama tags 16-byte PhysicalCell kernel and unit tests
        grid.rs               # 8x8 chunked sleep bitmap grid, neighbor propagation and buoyancy layering
        catalog.rs            # NEI item and entity catalog with full-dimension tag state formatting
        nei_ui.rs             # NEI panel, 9-slot quick item bar and aim state inspection HUD
```

## Build and verify

Check and run `tags` kernel unit tests (editor `preview-module` form, no rendering backend linked, suitable for fast cross-platform verification):

```bash
cargo test --manifest-path projects/demos/emergence/Cargo.toml --no-default-features --features preview-module
```

Build desktop player binary (needs Vulkan / audio system libs and compiled shaders):

```bash
cargo run -p prism-build-pipeline -- build-scenes --manifest projects/demos/emergence/assets/scenes.json --staging projects/demos/emergence/target/scene_pack --output projects/demos/emergence/assets/scenes.pak
cargo build --manifest-path projects/demos/emergence/Cargo.toml
```
