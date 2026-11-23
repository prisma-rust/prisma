# EchoWell

`EchoWell` is a 2D gray-box platformer slice made per the design doc, currently containing two connected rooms: the 320×180 "Echo Well" and "Reservoir". The character uses fixed 60 Hz simulation with coyote time, jump input buffering, variable jump height, air dash, ability crystals, ability gates, spike death and 0.5s respawn.

Gameplay code lives on the user project side, directly reusing Prisma's existing `SpriteRenderer`, `SpriteAnimation`, `Tilemap`, `ParticleEmitter2D`, `Light2D`, orthographic camera, input snapshot, UI quad and save interface. `assets/player.json` is the hot-reloadable movement parameters; `assets/rooms.json` maintains room IDs, bounds, tiles, collisions, exit targets, landing spots and ability conditions. After the dash crystal is unlocked the player can pass the well room exit; room switches preserve velocity and permanent abilities; current room ID is written to save, and death returns to the current room entrance. `assets/scenes/well.scene.json` holds the base camera; room entities are spawned by the gameplay system under stable names, making it easy to hook up Tiled / Aseprite cooker later.

Collision convention: player, spikes, ability gates and crystals all use collision AABB center as entity position, with center pivot, and scale the sprite quad to matching collision size; preset scene entities also apply this alignment rule at runtime.

## run

```powershell
cd projects/echowell
.\scripts\run.ps1
```

First run generates original gray-box sprites and bakes the asset pack from the scene manifest. After manually editing sprites you can use `-SkipAssetGeneration` to keep changes; for code-only edits use `-SkipSceneBuild`.

Editor preview module:

```text
cargo build --no-default-features --features preview-module
```

Controls: `A/D` or arrow keys to move, `Space` jumps from the start, `Shift` unlocks dash after touching the gold crystal. Jump over ground spikes, dash through the purple gate on the right. `F1` toggles collision debug overlay; press `E` to start/end recording, press `Enter` to replay from recording start.

Room exits are defined by `exits` in `rooms.json`; an exit can declare `required_ability`, and the target room's `entry` is the player landing spot after switch. Camera clamps by current room `bounds`; rooms larger than viewport scroll with the player.

## 2D issue snapshot

This slice directly validates merged Tilemap (#529), input snapshot (#589), multi-frame sprite animation (#584), 2D particles (#594), sprite blending/sorting (#610) and Light2D (#611). Atlas cooker (#1158), Aseprite/Tiled offline import (#1159) and Rapier2D / 2D camera confiner (#1160) remain as next-stage increments; current level data already uses JSON structures leaving a migration boundary.
