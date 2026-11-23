#!/usr/bin/env python3
"""Building-block feature matrix: ensure combos that "turn off a capability" actually compile.

    python scripts/check_features.py          # run default matrix
    python scripts/check_features.py --list   # only print combos

Why needed: features rot when nobody compiles them (`#[cfg]` missed, discovered months later).
Project template claims "2D projects do not compile 3D / path tracing", CI must really compile those combos.

When adding a capability (feature), add a **minimally usable combo** to `MATRIX`, do not only test default.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))

from lib.console import force_utf8_streams  # noqa: E402

force_utf8_streams()

# (tag, cargo args). Each must be a combo "template project would actually build this way".
MATRIX: list[tuple[str, list[str]]] = [
    ("prism-render all off", ["check", "-p", "prism-render", "--no-default-features"]),
    # UI overlay but **no** text: only draws quads, does not link any text rasterizer backend.
    (
        "prism-render ui (no text backend)",
        ["check", "-p", "prism-render", "--no-default-features", "--features", "ui"],
    ),
    # Text backends must each stand alone (mutually exclusive, cannot enable both).
    (
        "prism-render ui + fontdue",
        ["check", "-p", "prism-render", "--no-default-features", "--features", "ui,ui-fontdue"],
    ),
    (
        "prism-render sharc",
        ["check", "-p", "prism-render", "--no-default-features", "--features", "sharc"],
    ),
    # Raster real-time GI: does not depend on pathtrace, only needs ray query. Fallback path (device unsupported)
    # determined at runtime (one-time warn in graph_renderer.rs), must compile independently.
    (
        "prism-render restir_gi",
        ["check", "-p", "prism-render", "--no-default-features", "--features", "restir_gi"],
    ),
    # `sharc` no longer implies `pathtrace`: two consumers of cache each optional, these two pin down combo "sharc only"
    # (DESIGN §12.2).
    (
        "prism-app x11 + sharc (no pathtrace)",
        ["check", "-p", "prism-app", "--no-default-features", "--features", "x11,sharc"],
    ),
    (
        "prism-engine sharc (no pathtrace)",
        ["check", "-p", "prism-engine", "--no-default-features", "--features", "sharc"],
    ),
    # Fallback path: with pathtrace but no sharc. Must compile with `gi_mode = Sharc`, degrades to pure PT
    # and warns once (one-time diagnostic in graph_renderer.rs), not a `#[cfg]` miss causing compile failure.
    (
        "prism-render pathtrace (no sharc)",
        ["check", "-p", "prism-render", "--no-default-features", "--features", "pathtrace"],
    ),
    # On Linux winit must have at least one window backend, so "all off" is not a legal combo;
    # minimal usable combo is x11-only (or wayland-only).
    ("prism-app minimal (x11)", ["check", "-p", "prism-app", "--no-default-features", "--features", "x11"]),
    ("prism-app minimal (wayland)", ["check", "-p", "prism-app", "--no-default-features", "--features", "wayland"]),
    (
        "prism-app x11 + pathtrace",
        ["check", "-p", "prism-app", "--no-default-features", "--features", "x11,pathtrace"],
    ),
    ("prism-app 2D tier (no 3D component groups)",
     ["check", "-p", "prism-app", "--no-default-features", "--features", "x11,group-2d,group-ui"],
    ),
    (
        "prism-app 3D minimal (no skybox/shadows/gtao)",
        ["check", "-p", "prism-app", "--no-default-features", "--features", "x11,group-3d,ibl"],
    ),
    # NVIDIA desktop extensions: Streamline (DLSS-SR / DLSS-RR / DLFG) and NRD are default-off bricks.
    # Each must stand alone, and dlfg implies dlss; nrd reuses Streamline dynamic library loader but does not need
    # DLSS at runtime, so both must be pinned (DESIGN §12.2).
    (
        "prism-render dlss (no nrd)",
        ["check", "-p", "prism-render", "--no-default-features", "--features", "dlss"],
    ),
    (
        "prism-render nrd (no dlss)",
        ["check", "-p", "prism-render", "--no-default-features", "--features", "nrd"],
    ),
    (
        "prism-render dlfg",
        ["check", "-p", "prism-render", "--no-default-features", "--features", "dlfg"],
    ),
    ("prism-app default (full)", ["check", "-p", "prism-app"]),
]


def main() -> int:
    parser = argparse.ArgumentParser(description="Prisma feature matrix check")
    parser.add_argument("--list", action="store_true")
    args = parser.parse_args()

    if args.list:
        for label, cmd in MATRIX:
            print(f"{label}: cargo {' '.join(cmd)}")
        return 0

    failed: list[str] = []
    for label, cmd in MATRIX:
        sys.stderr.write(f"→ {label}: cargo {' '.join(cmd)}\n")
        proc = subprocess.run(
            ["cargo", *cmd],
            cwd=ROOT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        if proc.returncode != 0:
            failed.append(label)
            sys.stderr.write(proc.stderr[-4000:] + "\n")
        else:
            sys.stderr.write("  ok\n")

    if failed:
        print(f"| Building-block feature matrix | ❌ | Failed: {', '.join(failed)} |")
        return 1
    print(f"| Building-block feature matrix | ✅ | {len(MATRIX)} combos |")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
