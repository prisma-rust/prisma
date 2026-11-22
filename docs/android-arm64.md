# Android build toolchain on aarch64 host (minority cases)

> **Most people do not need to read this document.** Regular packaging flow see [android-apk.md](android-apk.md);
> This document is only needed when the **host machine is aarch64**: Asahi / Raspberry Pi / ARM server / aarch64 container.

Google's Android SDK / NDK native binaries are only released for x86_64. After installing SDK and NDK per [android-apk.md](android-apk.md)
on an aarch64 host, the three replacements in this document are still needed to actually build APKs.

## 1. Why official does not work

| Component | Official release form | On aarch64 host |
|---|---|---|
| NDK `toolchains/llvm/prebuilt/*` | Only `linux-x86_64` / `darwin-x86_64` / `windows-x86_64` | `Exec format error` |
| `aapt2` / `aidl` / `zipalign` / `split-select` | Only x86_64 ELF | `Exec format error` |
| Gradle / AGP / D8 / R8 / apksigner | JVM | ✅ Runs natively |
| Rust `aarch64-linux-android` | rustup target | ✅ Runs natively |

Key point: the parts of NDK **independent of host architecture** are `sysroot/` (bionic headers + aarch64 stub libraries +
`crtbegin_so.o`/`crtend_so.o`) and the target runtime under `lib/clang/<ver>/lib/linux/`
(`libclang_rt.builtins-aarch64-android.a`, `aarch64/libunwind.a`); just replace the clang driver.

## 2. Host dependencies

```bash
# Ubuntu / Debian
sudo apt-get install -y clang lld llvm-21 cmake ninja-build unzip curl
# Termux
pkg install clang lld llvm cmake ninja unzip curl
```

`scripts/make-ndk-host-native.sh` needs `clang` and same-version `llvm-strip` (`llvm-<major>` package).

## 3. NDK: replace with host clang driver

```bash
bash scripts/make-ndk-host-native.sh "$ANDROID_NDK_HOME"
```

The script rewrites NDK in place (to restore, re-extract the NDK zip):

1. `prebuilt/<host>/bin/*clang*` → shim. The shim parses API level from `argv[0]` (`aarch64-linux-android31-clang`),
   then calls the **host clang** and supplements `--target` / `--sysroot` /
   `--rtlib=compiler-rt` and NDK's `builtins`, `libunwind` search paths.
2. `llvm-{ar,ranlib,strip,objcopy,nm,readelf,...}` → host LLVM (same target architecture, can handle aarch64 ELF),
   so AGP's strip tasks and cmake can also run.

### Why the shim does not use NDK's own clang headers

NDK's `lib/clang/<ver>/include` (e.g. `arm_neon.h`) must be bound to the same clang version. Using host clang with
NDK headers directly breaks:

```
arm_neon.h:46628: error: incompatible constant for this __builtin_neon function
```

So the shim only borrows **sysroot + target runtime** from NDK; headers use host clang's own resource dir.

## 4. aapt2 and other build-tools

Google does not release aarch64 versions; use community cross-compiled versions from AOSP (requires glibc ≥ 2.36):

```bash
curl -fsSL -o /tmp/arm-tools.sh \
  https://raw.githubusercontent.com/Commit451/android-arm-build-tools/main/install.sh
bash /tmp/arm-tools.sh --sdk "$ANDROID_HOME" --version 36.0.0
"$ANDROID_HOME/build-tools/36.0.0/aapt2" version    # able to print version means success
```

On Termux, corresponding static aarch64 versions see `Lzhiyong/sdk-tools`, `hamza72x/android-sdk-linux-arm64`.

## 5. Make AGP use this aapt2

AGP 8.x pulls `aapt2-*-linux` (x86_64) from Maven. `scripts/build-android-game.sh [project]`
automatically passes `-Pandroid.aapt2FromMavenOverride=<SDK>/build-tools/<ver>/aapt2`; when running `gradlew` manually, choose one of:

```bash
./gradlew -Pandroid.aapt2FromMavenOverride=$ANDROID_HOME/build-tools/36.0.0/aapt2 :app:assembleDebug

# or write to ~/.gradle/gradle.properties (host path, do not commit)
echo "android.aapt2FromMavenOverride=$ANDROID_HOME/build-tools/36.0.0/aapt2" >> ~/.gradle/gradle.properties
```

AGP will print warning `option setting 'android.aapt2FromMavenOverride' is experimental`, which is normal.

## 6. arm64-specific pitfalls

| Symptom | Cause | Solution |
|---|---|---|
| `clang: Exec format error` | NDK prebuilt is x86_64 | `make-ndk-host-native.sh` |
| `incompatible constant for this __builtin_neon function` | NDK headers with host clang | shim does not pass `-resource-dir`, headers use host's own |
| `ld.lld: unable to find library -lunwind` | `libunwind.a` is at `lib/clang/<v>/lib/linux/aarch64/` | shim appends that `-L` |
| `ld.lld: unable to find library -lgcc` | Host clang defaults to `--rtlib=libgcc` | shim adds `--rtlib=compiler-rt` |
| API level overridden to 21 by cc-rs | cc passes `--target=aarch64-linux-android` (no API suffix) | shim puts `--target=...<api>` last |
| `aapt2 ... Exec format error` / `Daemon startup failed` | Maven-pulled aapt2 is x86_64 | `-Pandroid.aapt2FromMavenOverride=<aarch64 aapt2>` |

Common pitfalls (AppCompat theme, kotlin-stdlib duplicate classes, clean after changing `.so`, etc.) see
[android-apk.md §6](android-apk.md).

## 7. Measurement record

Ubuntu 26.04 aarch64 (glibc 2.43 / clang 21 / JDK 21 / NDK r29 / SDK 36)
→ `libtemplate_core.so` 11.7 MB → release APK 20 MB; debug APK installed on Android 16 real device starts,
Vulkan device + swapchain creation, first frame `present` succeeds.
