#!/usr/bin/env bash
# Release-tier build: nightly `-Z build-std` + immediate-abort, ~9~10% smaller than stable `dist`.
#
#   bash scripts/build-dist.sh                        # default projects/template, host target
#   bash scripts/build-dist.sh projects/fishing       # specified project
#   bash scripts/build-dist.sh projects/template aarch64-linux-android
#   bash scripts/build-dist.sh --size [project] [target]   # extreme size (tiny profile + no inlining)
#   bash scripts/build-dist.sh --size --pack [project]     # add UPX self-decompress shell (see below)
#   PROFILE=dist bash scripts/build-dist.sh           # change profile
#
# --pack: after build compress with `upx --best --lzma` into self-decompress shell. **Release only**, and compressed
# must be code-signed — unsigned compressed shell is the most classic antivirus false-positive pattern (Windows Defender etc).
# When upx not found only hint, not fail: compression is optional optimization, should not turn build red.
#
# Why: official libstd is precompiled, link can only take it wholesale; `-Z build-std` makes std and your
# code go through fat LTO together, `-Cpanic=immediate-abort` removes panic formatting (core::fmt) chunk.
# Measured (aarch64 Linux, dist): 3D full 4.47 MB → 4.01 MB (−459 KB),
# 2D minimal + X11 only 2.89 MB → 2.63 MB (−262 KB).
#
# Cost: nightly only needed for **release builds**, daily `cargo run` still stable — because parameters all on
# command line, not written into any Cargo.toml (writing would make stable cargo reject manifest parse).
#
# Note: immediate-abort leaves panic only abort, no message / backtrace; release packages only, do not debug with it.
set -euo pipefail

readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# --size: extreme size tier (tiny profile + forbid inlining). Positional params stay backward compatible.
# --pack: optional compression shell (UPX). Same, positional params unaffected.
SIZE_MODE=0
PACK_MODE=0
ARGS=()
for arg in "$@"; do
    case "$arg" in
        --size|--tiny) SIZE_MODE=1 ;;
        --pack|--upx) PACK_MODE=1 ;;
        *) ARGS+=("$arg") ;;
    esac
done
PROJECT="${ARGS[0]:-projects/template}"
TARGET="${ARGS[1]:-}"
if [ "${SIZE_MODE}" = "1" ]; then
    PROFILE="${PROFILE:-tiny}"
else
    PROFILE="${PROFILE:-dist}"
fi
NIGHTLY="${NIGHTLY:-nightly}"

# ---------------------------------------------------------------- Env check
if ! command -v rustup >/dev/null 2>&1; then
        echo "build-dist: needs rustup (to install nightly + rust-src)" >&2
    exit 1
fi

if ! rustup toolchain list | grep -q "^${NIGHTLY}"; then
        echo "build-dist: installing ${NIGHTLY} (first time ~100 MB)"
    rustup toolchain install "${NIGHTLY}" --profile minimal
fi

if ! rustup component list --toolchain "${NIGHTLY}" | grep -q '^rust-src (installed)'; then
        echo "build-dist: installing rust-src"
    rustup component add rust-src --toolchain "${NIGHTLY}"
fi

# build-std requires explicit --target, even for host.
if [ -z "${TARGET}" ]; then
    TARGET="$("${NIGHTLY_TRIPLE:-rustc}" -vV 2>/dev/null | sed -n 's/^host: //p' || true)"
    if [ -z "${TARGET}" ]; then
        TARGET="$(rustc -vV | sed -n 's/^host: //p')"
    fi
fi

MANIFEST="${ROOT}/${PROJECT}/Cargo.toml"
if [ ! -f "${MANIFEST}" ]; then
        echo "build-dist: cannot find ${MANIFEST}" >&2
    exit 1
fi

# Project own workspace root (profile read from there).
PKG="$(sed -n 's/^name = "\(.*\)"/\1/p' "${MANIFEST}" | head -1)"

echo "build-dist: ${PROJECT} (${PKG}) → ${TARGET} / profile=${PROFILE}"

# ---------------------------------------------------------------- build
# RUSTFLAGS applies to both std and project, ensures consistent panic strategy;
# not written to Cargo.toml, stable build therefore fully unaffected.
# --size adds another cut: forbid inlining (pure size measure). Hot path slows 10–30%, only for competition / extreme scenes;
# to have both write [profile.tiny.package.<crate>] opt-level = 3 for key crates in project Cargo.toml.
EXTRA_RUSTFLAGS=""
if [ "${SIZE_MODE}" = "1" ]; then
    EXTRA_RUSTFLAGS=" -C llvm-args=-inline-threshold=0 -C llvm-args=-inlinehint-threshold=0"
fi

RUSTFLAGS="-Zunstable-options -Cpanic=immediate-abort${EXTRA_RUSTFLAGS}" \
    cargo "+${NIGHTLY}" build \
    --manifest-path "${MANIFEST}" \
    --profile "${PROFILE}" \
    --package "${PKG}" \
    --target "${TARGET}" \
    -Z build-std=std,panic_abort

# ---------------------------------------------------------------- Report
BIN="${ROOT}/${PROJECT}/target/${TARGET}/${PROFILE}/${PKG}"
# Windows target executable has `.exe` suffix.
if [ ! -f "${BIN}" ] && [ -f "${BIN}.exe" ]; then
    BIN="${BIN}.exe"
fi
if [ -f "${BIN}" ]; then
    SIZE="$(stat -c%s "${BIN}")"
    printf 'build-dist: %s  %s bytes (%.2f MB)\n' "${BIN}" "${SIZE}" "$(echo "${SIZE}" | awk '{print $1/1048576}')"
else
        echo "build-dist: build succeeded but ${BIN} not found (check package name matches bin name)" >&2
fi

# ---------------------------------------------------------------- Optional compression shell (--pack)
# Order: not on Android → no upx hint and preserve → upx present then compress.
# No step may fail the build: compression is optional release optimization, not correctness prerequisite.
if [ "${PACK_MODE}" = "1" ]; then
    if [ ! -f "${BIN}" ]; then
                echo "build-dist: --pack skipped (no artifact found)" >&2
    elif [ "${TARGET#*android}" != "${TARGET}" ]; then
                # Android .so dominated by system loader / Play scanning, compression shell only brings false positives and
                # slower first page-in, negative benefit.
                echo "build-dist: --pack skipped: Android artifact not compressed"
    elif ! command -v upx >/dev/null 2>&1; then
                echo "build-dist: upx not found, skipping compression (--pack optional, build succeeds)"
                echo "  Install: Termux  pkg install upx"
        echo "        Debian/Ubuntu  sudo apt install upx-ucl"
                echo "          Windows  winget install upx (or https://upx.github.io)"
                echo "  Uncompressed artifact preserved: ${BIN}"
    else
        BEFORE="$(stat -c%s "${BIN}")"
        echo "build-dist: $(upx --version 2>/dev/null | head -1 || echo upx) → upx --best --lzma"
        if upx --best --lzma "${BIN}"; then
            AFTER="$(stat -c%s "${BIN}")"
                        printf 'build-dist: packed  %s → %s bytes (%.2f MB, ratio %.1f%%)\n' \
                "${BEFORE}" "${AFTER}" \
                "$(awk -v a="${AFTER}" 'BEGIN { print a/1048576 }')" \
                "$(awk -v a="${BEFORE}" -v b="${AFTER}" 'BEGIN { if (a > 0) print (1-b/a)*100; else print 0 }')"
        else
                        echo "build-dist: upx compression failed, preserving uncompressed artifact (build itself still succeeds)" >&2
        fi
    fi
        echo "build-dist: note: sign artifact after compression, otherwise Windows Defender etc may false-positive (unsigned self-extracting shell is typical heuristic marker)"
fi
