#!/usr/bin/env bash
# Build arm64-v8a release APK (output in launcher/src-tauri/gen/android/app/build/outputs/).
#
# Usage (Git Bash / WSL / Linux): bash launcher/build_release.sh
# Repo checked out per core.autocrlf; on Windows directly ./build_release.sh will fail because shebang has CR.
#
# Depends on env vars (see launcher/SETUP.md), script itself does not point to any machine path:
#   JAVA_HOME          JDK 17 (required by Tauri/Gradle)
#   ANDROID_HOME       Android SDK
#   ANDROID_NDK_HOME   or NDK_HOME, NDK install directory
set -uo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

: "${JAVA_HOME:?set JAVA_HOME to a JDK 17 install}"
: "${ANDROID_HOME:?set ANDROID_HOME to the Android SDK}"

ndk="${ANDROID_NDK_HOME:-${NDK_HOME:-}}"
if [ -z "$ndk" ]; then
    echo "set ANDROID_NDK_HOME (or NDK_HOME) to an NDK install" >&2
    exit 1
fi
export ANDROID_HOME
export ANDROID_NDK_HOME="$ndk"
export NDK_HOME="$ndk"

# On Windows Gradle cannot read Windows cert store by default; override with JAVA_TOOL_OPTIONS when needed.
export JAVA_TOOL_OPTIONS="${JAVA_TOOL_OPTIONS:--Djavax.net.ssl.trustStoreType=Windows-ROOT -Dcom.sun.net.ssl.checkRevocation=false -Dcom.sun.security.disableRevocation=true}"

cd "$repo_root/launcher"
log="$repo_root/launcher/tauri_build_release.log"

status=0
pnpm tauri android build --target aarch64 --apk --split-per-abi >"$log" 2>&1 || status=$?
echo "BUILD_EXIT=$status"
echo "log: $log"
exit "$status"
