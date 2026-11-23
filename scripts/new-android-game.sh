#!/usr/bin/env bash
# Generate game-only Android host for user project (#598).
#
#   bash scripts/new-android-game.sh fishing
#   bash scripts/new-android-game.sh fishing --package com.example.fishing --label "Cozy Fishing"
#   bash scripts/new-android-game.sh fishing --force   # overwrite existing projects/fishing/android
#
# Copy Gradle project from `templates/android-game/`, replace three placeholders:
#   __GAME_NAME__  project lib name (`[lib] name` → `[package] name`, `-` replaced with `_`)
#   __PACKAGE__    Android package name (default `com.prisma.<project name>`)
#   __LABEL__      app label under desktop icon (default project name)
#
# After generating use `bash scripts/build-android-game.sh <project>` to build APK. Template already carries
# `minSdk 31`, `abiFilters arm64-v8a`, `packaging.keepDebugSymbols` and signing comments, **do not**
# overwrite with tools like `tauri android init` (see docs/android-apk.md).
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=lib/project-info.sh
. "$ROOT/scripts/lib/project-info.sh"

PROJECT=""
PACKAGE=""
LABEL=""
FORCE=0
while [ $# -gt 0 ]; do
    case "$1" in
        --package)
                        [ $# -ge 2 ] || { echo "--package requires a package name" >&2; exit 1; }
            PACKAGE="$2"; shift 2 ;;
        --label)
                        [ $# -ge 2 ] || { echo "--label requires an app name" >&2; exit 1; }
            LABEL="$2"; shift 2 ;;
        --force) FORCE=1; shift ;;
        -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
                -*) echo "Unknown argument: $1" >&2; exit 1 ;;
                *) [ -z "$PROJECT" ] || { echo "Only one project name needed" >&2; exit 1; }; PROJECT="$1"; shift ;;
    esac
done

[ -n "$PROJECT" ] || { echo "Usage: $0 <project> [--package <id>] [--label <name>] [--force]" >&2; exit 1; }

PROJECT_DIR=$(prism_project_dir "$ROOT" "$PROJECT")
PROJECT_NAME=$(prism_project_name "$PROJECT_DIR")
LIB_NAME=$(prism_lib_name "$PROJECT_DIR")
if [ -z "$PACKAGE" ]; then
    SANITIZED=$(printf '%s' "$PROJECT_NAME" | tr -c 'a-zA-Z0-9' '_' | sed 's/_*$//')
    [ -n "$SANITIZED" ] || {
                echo "Cannot infer package name from directory '$PROJECT_NAME'; use --package <id>" >&2
        exit 1
    }
    PACKAGE="com.prisma.$SANITIZED"
fi
case "$PACKAGE" in
    *[!a-zA-Z0-9._]*|.*|*.|*..*)
                echo "Package name '$PACKAGE' invalid (only letters / digits / dots, not starting or ending with dot)" >&2
        exit 1 ;;
esac
LABEL="${LABEL:-$PROJECT_NAME}"
PACKAGE_PATH=$(printf '%s' "$PACKAGE" | tr '.' '/')

TEMPLATE="$ROOT/templates/android-game"
[ -d "$TEMPLATE" ] || { echo "Missing template directory: $TEMPLATE" >&2; exit 1; }

ANDROID_DIR="$PROJECT_DIR/android"
if [ -e "$ANDROID_DIR" ] && [ "$FORCE" -eq 0 ]; then
        echo "$ANDROID_DIR already exists; add --force to overwrite" >&2
    exit 1
fi

echo "==> $PROJECT_NAME → $ANDROID_DIR"
echo "    lib_name = $LIB_NAME"
echo "    package  = $PACKAGE"
echo "    label    = $LABEL"

# Placeholder replacement: `&` / `|` in value have special meaning in sed, escape first.
escape_sed() { printf '%s' "$1" | sed -e 's/[&|\\]/\\&/g'; }
LIB_SED=$(escape_sed "$LIB_NAME")
PACKAGE_SED=$(escape_sed "$PACKAGE")
LABEL_SED=$(escape_sed "$LABEL")

rm -rf "$ANDROID_DIR"
mkdir -p "$ANDROID_DIR"
# `-a` preserves executable bit (gradlew); text and binary (gradle-wrapper.jar) copied as-is.
cp -a "$TEMPLATE/." "$ANDROID_DIR/"

# Java source must land in directory matching package name, otherwise AndroidManifest `.MainActivity` class not found.
mkdir -p "$ANDROID_DIR/app/src/main/java/$PACKAGE_PATH"
mv "$ANDROID_DIR/app/src/main/java/MainActivity.java" \
   "$ANDROID_DIR/app/src/main/java/$PACKAGE_PATH/MainActivity.java"

# Only modify text files; gradle-wrapper.jar is binary, cannot go through sed.
while IFS= read -r -d '' file; do
    case "$file" in
        *.jar) continue ;;
    esac
    # `sed -i` takes no backup suffix on GNU but requires one on BSD/macOS, so the
    # substitute is written to a temp file and moved back: identical on both.
    sed \
        -e "s|__GAME_NAME__|$LIB_SED|g" \
        -e "s|__PACKAGE__|$PACKAGE_SED|g" \
        -e "s|__LABEL__|$LABEL_SED|g" \
        "$file" > "$file.tmp" && mv "$file.tmp" "$file"
done < <(find "$ANDROID_DIR" -type f -print0)

chmod +x "$ANDROID_DIR/gradlew"
[ -f "$ANDROID_DIR/gradle/wrapper/gradle-wrapper.jar" ] || {
        echo "Template missing gradle-wrapper.jar: Android packaging will fail" >&2
    exit 1
}

echo
echo "Done. Next steps:"
echo "  bash scripts/build-android-game.sh $PROJECT_NAME            # debug APK"
echo "  bash scripts/build-android-game.sh $PROJECT_NAME release    # release APK (unsigned)"
