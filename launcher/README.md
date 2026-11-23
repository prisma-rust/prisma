# Prisma Android launcher shell

The "Start Game" page in APK + packaging config. This Tauri project does one thing: launch the game.

- **Android**: `launch_game` starts `com.prisma.MainActivity` via Kotlin plugin
    (`GameActivity`, `android.app.lib_name = template_core` → `libtemplate_core.so`).
- **Desktop**: `launch_game` spawns local game binary, so launch page and
    `PRISMA_LAUNCH_CONFIG` parameter passing can be validated without packaging.

APK has two Activities (two processes):

| Activity | Role |
|---|---|
| `com.example.tauriandroidapp.MainActivity` | This Tauri launcher shell (this directory) |
| `com.prisma.MainActivity` | `GameActivity`, loads `libtemplate_core.so` |

## Packaging

```powershell
scripts/build-android.ps1     # Windows
bash launcher/build_release.sh  # Git Bash / WSL / Linux
```

Script compiles game cdylib, builds scene pack, puts output into
`launcher/src-tauri/gen/android/app/src/main/assets/game/`, then Gradle assembles APK.
`gen/android/` **has local customizations** (minSdk 31, arm64-v8a, signing config, second Activity),
do not regenerate with `tauri android init`.

## Development

```sh
cd launcher
pnpm install
pnpm tauri dev      # Desktop: shows launch page, click spawns local game binary
```

Frontend check:

```sh
cd launcher
pnpm build
```

Backend check / tests (independent workspace, not attached to root Prisma workspace):

```sh
cd launcher/src-tauri
cargo test
```

Game binary location does not use `PATH`: `PRISMA_GAME_BIN` → `template[.exe]` next to launcher →
`projects/template/target/{debug,release}/template[.exe]`.
