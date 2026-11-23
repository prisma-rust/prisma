#!/usr/bin/env bash
# Build game-only Android APK (no launcher / no Tauri / no WebView).
#
#   bash scripts/build-android-game.sh                      # projects/template, debug
#   bash scripts/build-android-game.sh fishing              # projects/fishing, debug
#   bash scripts/build-android-game.sh fishing release       # release (unsigned, need apksigner)
#   bash scripts/build-android-game.sh debug --skip-cargo    # pack only (.so ready)
#   bash scripts/build-android-game.sh --project fishing --skip-cargo
#
# Output: <project>/android/app/build/outputs/apk/<variant>/*.apk
#
# Project name is directory name under `projects/` (can also give relative / absolute path). Lib name read from
# `<project>/assets/settings.ini [build] lib_name` or `Cargo.toml`, script no longer
# guesses `.so` name from directory name; when project lacks `android/` first run
# `bash scripts/new-android-game.sh <project>` to generate host (#598).
#
# Env vars (at least SDK and NDK needed):
#   ANDROID_HOME       Android SDK root dir (ANDROID_SDK_ROOT also ok)
#   ANDROID_NDK_HOME   NDK root dir (if unset uses newest under $ANDROID_HOME/ndk/)
#
# aarch64 host (Ubuntu glibc / Termux) additionally needs:
#   - host clang + lld + LLVM tools (apt install clang lld llvm-21 / pkg install clang lld llvm)
#   - first run bash scripts/make-ndk-host-native.sh "$ANDROID_NDK_HOME" (replaces official x86_64
#     NDK with locally executable clang shims)
#   - build-tools aapt2/aidl/zipalign/split-select replaced with aarch64 versions (Google only ships
#     x86_64), this script auto uses -Pandroid.aapt2FromMavenOverride to point to runnable one in SDK
# See docs/android-apk.md; aarch64 host also docs/android-arm64.md.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=lib/project-info.sh
. "$ROOT/scripts/lib/project-info.sh"

MODE=""
PROJECT=""
SKIP_CARGO=0
while [ $# -gt 0 ]; do
    case "$1" in
        debug|release)
                        [ -z "$MODE" ] || { echo "Mode can be given only once ($MODE / $1)" >&2; exit 1; }
            MODE="$1"; shift ;;
        --project)
                        [ $# -ge 2 ] || { echo "--project needs project name" >&2; exit 1; }
            PROJECT="$2"; shift 2 ;;
        --skip-cargo) SKIP_CARGO=1; shift ;;
        -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
                -*) echo "Unknown argument: $1" >&2; exit 1 ;;
        *)
                        [ -z "$PROJECT" ] || { echo "Only one project name needed" >&2; exit 1; }
            PROJECT="$1"; shift ;;
    esac
done
MODE="${MODE:-debug}"

PROJECT_DIR=$(prism_project_dir "$ROOT" "${PROJECT:-template}")
PROJECT_NAME=$(prism_project_name "$PROJECT_DIR")
LIB_NAME=$(prism_lib_name "$PROJECT_DIR")
ANDROID_DIR="$PROJECT_DIR/android"
GAME_MANIFEST="$PROJECT_DIR/Cargo.toml"
ASSETS_DIR="$PROJECT_DIR/assets"
JNI_DIR="$ANDROID_DIR/app/src/main/jniLibs/arm64-v8a"
ASSET_DIR="$ANDROID_DIR/app/src/main/assets/game"
TARGET=aarch64-linux-android

[ -d "$ANDROID_DIR" ] || {
        echo "Project $PROJECT_NAME has no Android host yet: $ANDROID_DIR" >&2
        echo "Generate first: bash scripts/new-android-game.sh $PROJECT_NAME" >&2
    exit 1
}

# Host manifest fixes `android.app.lib_name`; if inconsistent with lib name game will crash at
# `System.loadLibrary`, so catch at build time (#598).
MANIFEST="$ANDROID_DIR/app/src/main/AndroidManifest.xml"
DECLARED_LIB=$(sed -nE 's/.*android:name="android.app.lib_name".*//; s/.*android:value="([^"]+)".*/\1/p' "$MANIFEST" | head -n1)
if [ -z "$DECLARED_LIB" ]; then
    DECLARED_LIB=$(tr -d '\n' < "$MANIFEST" | grep -o 'android.app.lib_name"[[:space:]]*android:value="[^"]*"' | sed -E 's/.*android:value="([^"]+)"/\1/' || true)
fi
if [ -n "$DECLARED_LIB" ] && [ "$DECLARED_LIB" != "$LIB_NAME" ]; then
        echo "Lib name mismatch: $MANIFEST declares lib_name=$DECLARED_LIB, but project lib name is $LIB_NAME" >&2
        echo "Change manifest android.app.lib_name, or set lib_name = $DECLARED_LIB in assets/settings.ini [build]" >&2
    exit 1
fi

case "$MODE" in
    debug)   GRADLE_TASK=assembleDebug;   CARGO_PROFILE_DIR=debug ;;
    release) GRADLE_TASK=assembleRelease; CARGO_PROFILE_DIR=release ;;
        *) echo "Usage: $0 [project] [debug|release] [--skip-cargo]" >&2; exit 1 ;;
esac

# ---- SDK / NDK -------------------------------------------------------------
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
[ -n "$SDK" ] && [ -d "$SDK" ] || {
        echo "Cannot find Android SDK: set ANDROID_HOME" >&2
    exit 1
}
NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
if [ -z "$NDK" ]; then
    NDK=$(ls -d "$SDK"/ndk/*/ 2>/dev/null | sed -e 's|/$||' | sort -t. -k1,1n -k2,2n -k3,3n | tail -n1)
    NDK="${NDK%/}"
fi
[ -n "$NDK" ] && [ -d "$NDK/toolchains/llvm" ] || {
        echo "Cannot find NDK: set ANDROID_NDK_HOME (sdkmanager \"ndk;<version>\")" >&2
    exit 1
}
export ANDROID_NDK_HOME="$NDK"
echo "NDK: $NDK"

HOST_PRE=$(ls -d "$NDK"/toolchains/llvm/prebuilt/*/ | head -n1)
HOST_PRE="${HOST_PRE%/}"

# ---- API level = minSdk (not lower than 26: libaaudio.so only from 26) ----------------
MIN_SDK=$(sed -nE 's/.*minSdk *= *([0-9]+).*/\1/p' "$ANDROID_DIR/app/build.gradle.kts" | head -n1)
MIN_SDK="${MIN_SDK:-31}"
API=$(( MIN_SDK < 26 ? 26 : MIN_SDK ))
LINKER="$HOST_PRE/bin/aarch64-linux-android$API-clang"
[ -x "$LINKER" ] || {
        echo "Cannot find $LINKER — aarch64 host first run: bash scripts/make-ndk-host-native.sh \"$NDK\"" >&2
    exit 1
}
echo "project=$PROJECT_NAME lib=$LIB_NAME target=$TARGET api=$API (minSdk=$MIN_SDK)"

export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$LINKER"
export CC_aarch64_linux_android="$LINKER"
export CXX_aarch64_linux_android="$HOST_PRE/bin/aarch64-linux-android$API-clang++"
export AR_aarch64_linux_android="${AR_aarch64_linux_android:-ar}"
export RANLIB_aarch64_linux_android="${RANLIB_aarch64_linux_android:-ranlib}"

printf 'sdk.dir=%s\n' "$SDK" > "$ANDROID_DIR/local.properties"

# ---- shader artifacts (.spv committed, normally never missing) --------------------------------
if ! ls "$ROOT"/assets/shaders/*.spv >/dev/null 2>&1 && ! ls "$ROOT"/assets/shaders/slang/*.spv >/dev/null 2>&1; then
        echo "Missing .spv: they are committed, first git checkout -- assets/shaders to restore;" >&2
        echo "to truly regenerate use bash assets/shaders/compile.sh (see docs/build-guide-termux.md §3)." >&2
    exit 1
fi

# ---- 1. game .so ----------------------------------------------------------
if [ "$SKIP_CARGO" -eq 0 ]; then
    echo "==> cargo build ($MODE) $TARGET"
    cargo build --manifest-path "$GAME_MANIFEST" --lib --target "$TARGET" \
        $( [ "$MODE" = release ] && echo --release )
fi
SO="$PROJECT_DIR/target/$TARGET/$CARGO_PROFILE_DIR/lib$LIB_NAME.so"
[ -f "$SO" ] || { echo "Cannot find $SO" >&2; exit 1; }
mkdir -p "$JNI_DIR"
cp -f "$SO" "$JNI_DIR/lib$LIB_NAME.so"
echo "==> lib$LIB_NAME.so $(du -h "$JNI_DIR/lib$LIB_NAME.so" | cut -f1)"

# ---- 2. scene pack + assets -----------------------------------------------------
# Each project has own assets/ and scenes.json; `build-scenes` reads project manifest, writes project
# pak (#598), no longer fixed to repo root `assets/`.
echo "==> build-scenes ($ASSETS_DIR/scenes.json)"
[ -f "$ASSETS_DIR/scenes.json" ] || { echo "Cannot find $ASSETS_DIR/scenes.json" >&2; exit 1; }
(
    cd "$ROOT"
    cargo run -q -p prism-build-pipeline -- build-scenes \
        --manifest "$ASSETS_DIR/scenes.json" \
        --staging "$PROJECT_DIR/target/scene_pack" \
        --output "$ASSETS_DIR/scenes.pak"
)
mkdir -p "$ASSET_DIR"
# At runtime mount all `.pak` under `assets/` (#530), so copy by extension.
shopt -s nullglob
PAKS=("$ASSETS_DIR"/*.pak)
METAS=("$ASSETS_DIR"/*.pak.meta.json)
HDRS=("$ASSETS_DIR"/*.hdr "$ASSETS_DIR"/*.exr)
shopt -u nullglob
[ ${#PAKS[@]} -gt 0 ] || { echo "No .pak under $ASSETS_DIR, APK will have no assets" >&2; exit 1; }
cp -f "${PAKS[@]}" "${METAS[@]}" "$ASSET_DIR/"
# New format `settings.ini`; legacy projects still only have `settings.toml`, bring along too (engine falls back).
for name in settings.ini settings.toml; do
    [ -f "$ASSETS_DIR/$name" ] && cp -f "$ASSETS_DIR/$name" "$ASSET_DIR/"
done
if [ ${#HDRS[@]} -gt 0 ]; then
    cp -f "${HDRS[@]}" "$ASSET_DIR/"
fi

# ---- 3. APK ---------------------------------------------------------------
# aarch64 host AGP pulls aapt2 from Maven (x86_64, won't run); point to runnable one in SDK.
GRADLE_ARGS=(-p "$ANDROID_DIR" "$GRADLE_TASK" --console=plain)
for cand in $(ls -d "$SDK"/build-tools/*/aapt2 2>/dev/null | sort -r -t. -k1,1n -k2,2n -k3,3n); do
    if "$cand" version >/dev/null 2>&1; then
        echo "aapt2: $cand"
        GRADLE_ARGS+=("-Pandroid.aapt2FromMavenOverride=$cand")
        break
    fi
done
if ! ls "$SDK"/build-tools/*/aapt2 >/dev/null 2>&1 || ! "$(ls -d "$SDK"/build-tools/*/aapt2 | sort -r -t. -k1,1n -k2,2n -k3,3n | head -n1)" version >/dev/null 2>&1; then
        echo "Tip: SDK has no locally executable aapt2 (aarch64 host see docs/android-arm64.md)." >&2
fi

echo "==> gradle $GRADLE_TASK"
# Must clean: after swapping .so incremental packaging leaves huge empty holes in zip (once 21MB became 217MB).
"$ANDROID_DIR/gradlew" clean "${GRADLE_ARGS[@]}" --no-daemon \
    -Dorg.gradle.jvmargs="-Xmx2g -XX:MaxMetaspaceSize=768m -Dfile.encoding=UTF-8"

APK=$(ls -t "$ANDROID_DIR"/app/build/outputs/apk/*/*.apk 2>/dev/null | head -n1)
[ -n "$APK" ] || { echo "No APK artifact found" >&2; exit 1; }
echo
echo "APK: $APK ($(du -h "$APK" | cut -f1))"
if [ "$MODE" = release ]; then
        echo "release unsigned, sign before install:"
    echo "  \$SDK/build-tools/<ver>/apksigner sign --ks <keystore> --out app-signed.apk \"$APK\""
fi
