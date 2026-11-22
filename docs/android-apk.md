# Build game-only Android APK (no launcher / no Tauri)

Build any user project into a minimal APK containing only a `GameActivity` shell + `lib<project>.so` + scene pack:
no WebView, no launcher UI, no Kotlin compiler needed. Applicable to any host (x86_64 most common).

The script's first argument is the **project name** (directory name under `projects/`), default `template`:

```bash
bash scripts/build-android-game.sh                # projects/template
bash scripts/build-android-game.sh fishing        # projects/fishing
bash scripts/build-android-game.sh fishing release
```

Run `scripts/new-android-game.sh` first before packaging a new project to generate the host (see §7).

> **Host is aarch64 (Asahi / Raspberry Pi / ARM server / Termux)?**
> First install JDK/SDK/NDK per this document, then add the toolchain replacement steps from
> [android-arm64.md](android-arm64.md) — that part is for minority cases, most people do not need it.

## 1. Prerequisites

| Dependency | Description |
|---|---|
| JDK 21 | AGP 8.11 requires 17+ |
| Android SDK | `platforms;android-36` + `build-tools;36.0.0` |
| NDK | r29 (`ANDROID_NDK_HOME`), official x86_64 host version is sufficient |
| `.spv` | Already committed, directly usable after clone; only need to regenerate or take from CI `spirv` artifact after changing shaders ([build-guide-termux.md §3](build-guide-termux.md)) |

```bash
# x86_64 Linux example
sudo apt-get install -y openjdk-21-jdk-headless cmake ninja-build unzip

export ANDROID_HOME=$HOME/android-sdk
mkdir -p "$ANDROID_HOME/cmdline-tools"
curl -L -o /tmp/cmdline.zip \
  https://dl.google.com/android/repository/commandlinetools-linux-14742923_latest.zip
unzip -q /tmp/cmdline.zip -d /tmp/cmdtmp
mv /tmp/cmdtmp/cmdline-tools "$ANDROID_HOME/cmdline-tools/latest"

yes | "$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" --licenses
"$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" \
  "platforms;android-36" "build-tools;36.0.0"

curl -L -o /tmp/ndk.zip https://dl.google.com/android/repository/android-ndk-r29-linux.zip
unzip -q /tmp/ndk.zip -d "$ANDROID_HOME/ndk-tmp"        # about 2.4 GB
mkdir -p "$ANDROID_HOME/ndk"
mv "$ANDROID_HOME/ndk-tmp/android-ndk-r29" "$ANDROID_HOME/ndk/29.0.14206865"
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/29.0.14206865"
```

## 2. One command

```bash
export ANDROID_HOME=$HOME/android-sdk
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/29.0.14206865

bash scripts/build-android-game.sh                     # template, debug (installable directly)
bash scripts/build-android-game.sh fishing             # fishing, debug
bash scripts/build-android-game.sh fishing release     # unsigned release
bash scripts/build-android-game.sh fishing debug --skip-cargo   # .so ready, repackage only
```

Script flow (all for `<project>` itself): `cargo build --lib --target aarch64-linux-android`
→ `build-scenes --manifest <project>/assets/scenes.json --output <project>/assets/scenes.pak`
→ copy `.so` and game assets to `<project>/android/app/src/main/` → `gradlew clean assemble<variant>`.

- App name (`.so` and `android.app.lib_name`) is read from `<project>/assets/settings.ini`'s
  `[build] lib_name`, otherwise from `Cargo.toml`'s `[lib] name` / `[package] name`
  (`-` → `_`); the script no longer guesses by directory name; if the manifest declaration
  is inconsistent with it, the build fails directly at build time.
- If the project has no `android/`, the script prompts to first generate it with `scripts/new-android-game.sh <project>`.

- API level takes `max(minSdk, 26)`: `libaaudio.so` exists only from API 26, below it reports `-laaudio not found`.
- **Must clean before packaging**: after changing `.so`, incremental packaging leaves holes in the zip (measured: 21 MB artifact becomes 217 MB).
- It copies **all** `.pak` under `assets/` (runtime mounts by extension); the script no longer hardcodes
  `scenes.pak`; if none exist, it fails directly. `build-scenes` also exits non-zero when resources
  referenced by scenes are missing—first complete assets (e.g. `python scripts/gen_placeholder_sprites.py`) before packaging.

## 3. Artifacts

```
<project>/android/app/build/outputs/apk/<variant>/*.apk
  package com.prisma (prisma) / com.prisma.fishing (fishing), minSdk 31, targetSdk 36, arm64-v8a only
  lib/arm64-v8a/lib<project>.so  ~12 MB (release) / ~200 MB (debug, with debug symbols)
  classes*.dex                   ~6 MB (GameActivity + appcompat)
  assets/game/                   <project>/assets/*.pak + *.pak.meta.json + settings.ini + HDR
```

Installation: `adb install -r app-debug.apk`, tapping the icon launches the game.

## 4. Release signing

`release` artifact is unsigned (`app-release-unsigned.apk`); signing materials **are not committed**:

```bash
$ANDROID_HOME/build-tools/36.0.0/apksigner sign \
  --ks ~/template-release.jks --ks-key-alias template \
  --out template-release.apk app-release-unsigned.apk
```

## 5. Directory and responsibilities

```
templates/android-game/           Host template (#598): scripts/new-android-game.sh expands from here
  app/build.gradle.kts                minSdk 31 / targetSdk 36 / arm64-v8a / signing comment
  app/src/main/AndroidManifest.xml    GameActivity + android.app.lib_name=__GAME_NAME__
  app/src/main/java/MainActivity.java 3 lines, extends GameActivity (directory by package name)
  app/src/main/res/values/themes.xml  AppCompat theme (GameActivity extends AppCompatActivity)
  gradle/wrapper/                     wrapper jar + properties (Gradle version defined only here)
projects/<p>/android/             game-only Android host (independent Gradle project, not part of any cargo workspace)
  app/src/main/jniLibs/                Generated: lib<p>.so (gitignore)
  app/src/main/assets/game/            Generated: scene pack and settings (gitignore)
scripts/new-android-game.sh       Generate projects/<p>/android/ from template
scripts/lib/project-info.sh       Unique resolution of project directory / app name (shared by both scripts)
scripts/build-android-game.sh     One command to package any project
```

Zero engine-side changes: `prism_app::run_on_android` extracts the APK's `assets/game/` to
`<filesDir>/assets`, switches cwd, then calls `android_main` in `projects/template/src/lib.rs`.

**Relationship with `launcher/`**: `launcher/src-tauri/gen/android` builds the "Tauri launcher + game"
dual Activity APK (`scripts/build-android.ps1`), independent of this path; use that one when launcher UI is needed,
use this one when only the game itself is wanted. Launcher's manifest fixes `android.app.lib_name = template_core`,
so `build-android.ps1 -Project fishing` will instead call the game-only script (forwarding `-Mode`).

## 6. Common pitfalls

| Symptom | Cause | Solution |
|---|---|---|
| `GameActivity` compilation reports missing `AppCompatActivity` | games-activity 4.4.0 inherits `AppCompatActivity` | Add `androidx.appcompat` + AppCompat theme |
| `Cannot find style/Theme.AppCompat.NoActionBar.Fullscreen` | That theme does not exist | Custom theme `parent="Theme.AppCompat.NoActionBar"` + `android:windowFullscreen` |
| `Duplicate class kotlin.collections.jdk8.*` | appcompat brings kotlin-stdlib-jdk7/jdk8 1.6.21 | `implementation(platform("org.jetbrains.kotlin:kotlin-bom:1.9.25"))` |
| APK suddenly grows by ~200 MB | Incremental packaging after changing `.so` leaves holes | `clean` before packaging (script already defaults to clean) |
| `-laaudio not found` | API level < 26 | API level takes `max(minSdk, 26)` |
| Compilation reports `include_bytes!` cannot find `.spv` | `.spv` already committed, most likely deleted/out of sync | `git checkout -- assets/shaders`; if regeneration is really needed use `compile.sh` |
| `aapt2: cannot execute binary` / `Exec format error` | Host is not x86_64 | See [android-arm64.md](android-arm64.md) |

## 7. Connecting new project to Android (#598)

```bash
bash scripts/new-android-game.sh fishing
#   --package com.example.fishing   defaults to com.prisma.<project name>
#   --label "Cozy Fishing"          desktop icon name, defaults to project name
#   --force                         overwrite existing projects/fishing/android

bash scripts/build-android-game.sh fishing          # debug APK
bash scripts/build-android-game.sh fishing release  # unsigned release
```

Key points (template already handled; these are most easily missed when manually forking):

- **`minSdk` not lower than 26**: `libaaudio.so` exists only from API 26, below it `-laaudio not found`.
  Template defaults to 31, consistent with existing projects in the repository.
- **`abiFilters` keeps only `arm64-v8a`**: Engine's Android support is exactly this tier.
- **`android.app.lib_name` must equal the library name** (`__GAME_NAME__` → `fishing`): it determines
  which `.so` `System.loadLibrary` loads; the script verifies before build, failing directly on mismatch.
- **`packaging.jniLibs.keepDebugSymbols`**: debug `.so` carries symbols; without `llvm-strip` do not fail the whole package.
- **Do not use `tauri android init` or other scaffolding to overwrite `projects/<p>/android/`**: The template's
  customizations (minSdk / abiFilters / signing comment / theme) would be overwritten by the template version.
- The packaging script parses by **project name**, no longer hardcoding `projects/template`; scene pack, `.pak`, settings.ini,
  HDR are all read from `<project>/assets/`, and `build-scenes` writes back to the project's own `assets/`.
- Art assets in gitignored projects must be generated first (e.g. `python scripts/gen_placeholder_sprites.py`),
  otherwise `build-scenes` fails due to missing references—this is intentional (#530).
