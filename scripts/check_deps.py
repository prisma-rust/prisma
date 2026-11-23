#!/usr/bin/env python3
"""Prisma dependency guard: player dependency budget + whole-repo crate layering boundary matrix.

    python scripts/check_deps.py            # full check (player budget + layer matrix)
    python scripts/check_deps.py --layers   # only check whole-repo crate layering
    python scripts/check_deps.py --list     # print whole player dependency tree

Rules (update `docs/DESIGN.md` §11 when changing this file):
  1. Player tree (`prism-app`) must not contain build-time / optional capability crates in `BANNED`;
  2. Total crate count in player tree must not exceed `MAX_PLAYER_CRATES`;
  3. All root workspace `prism-*` crates must stay UI-free (no egui etc UI frameworks);
  4. Each layer crate must not reverse or cross-boundary depend (see `LAYER_RULES`).

stdout is Markdown lines for PR paste, progress / diagnostics go to stderr.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))

from lib.console import force_utf8_streams  # noqa: E402

PLAYER_CRATE = "prism-app"

# Allowed player-tree upper limit. Leave some margin, but **do not** mindlessly raise to pass CI:
# either make new dependency a feature, or describe in PR why it must ship with player.
MAX_PLAYER_CRATES = 155

# Build-time / editor / optional capabilities: appearing is a failure.
BANNED: dict[str, str] = {
    "gltf": "Asset import only in prism-cooker (build time)",
    "gltf-json": "Asset import only in prism-cooker (build time)",
    "clap": "CLI arg parsing only in build tools",
    "rayon": "Build-time parallelism, should not be at runtime",
    "exr": "EXR decode only in cooker import",
    "jpeg-decoder": "JPEG decode only in cooker import",
    "tiff": "TIFF decode only in cooker import",
    "tokio": "Async IO is prism-asset `async-io` feature, default off",
    "rapier3d": "Physics is prism-app `physics` feature, default off",
    "parry3d": "Physics is prism-app `physics` feature, default off",
    "cpal": "Audio backend is prism-app `audio` feature, default off",
    "firewheel": "Audio backend is prism-app `audio` feature, default off",
    "symphonia": "Audio decode is prism-app `audio` feature, default off",
    "symphonium": "Audio decode is prism-app `audio` feature, default off",
}

UI_CRATES = {"egui", "egui-ash", "egui_extras", "eframe"}

FOUNDATION_CRATES = {
    "prism-asset",
    "prism-audio",
    "prism-ecs",
    "prism-ini",
    "prism-physics2d",
    "prism-platform",
    "prism-project-api",
    "prism-reflect",
    "prism-reflect-derive",
    "prism-render-shaders",
    "prism-ui-draw",
}

DATA_SCHEMA_CRATES = {
    "prism-anim",
    "prism-anim-ik",
    "prism-anim-retarget",
    "prism-gfx",
    "prism-motion-matching",
    "prism-scene",
}

UPPER_CRATES = {
    "prism-app",
    "prism-build-pipeline",
    "prism-cooker",
    "prism-editor-api",
    "prism-editor-mcp",
    "prism-engine",
    "prism-game-api",
    "prism-render",
    "prism-runtime-host",
}

VULKAN_FREE_CRATES = {"prism-cooker", "prism-editor-api", "prism-editor-mcp"}
VULKAN_DEPS = {"ash", "ash-window", "prism-render", "prism-engine", "prism-app"}

force_utf8_streams()


def player_tree() -> list[str]:
    """Parse `cargo tree -p prism-app`, return unique crate name list."""
    proc = subprocess.run(
        ["cargo", "tree", "-p", PLAYER_CRATE, "-e", "normal", "--prefix", "none"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        raise SystemExit("check_deps: `cargo tree` failed")

    names: set[str] = set()
    for line in proc.stdout.splitlines():
        line = line.strip()
        if not line or line.startswith("["):
            continue
        name = line.split(" ")[0]
        if name:
            names.add(name)
    return sorted(names)


def check_player_budget() -> tuple[list[str], int]:
    tree = player_tree()
    banned_hits = [(name, BANNED[name]) for name in tree if name in BANNED]
    total = len(tree)

    problems: list[str] = []
    for name, reason in banned_hits:
        problems.append(f"`{name}` should not appear in player tree: {reason}")
    if total > MAX_PLAYER_CRATES:
        problems.append(f"Player depends on {total} crates, over budget {MAX_PLAYER_CRATES}")
    return problems, total


def check_layers() -> tuple[list[str], int]:
    """Check whole-repo crate layering and UI-free via `cargo metadata`."""
    proc = subprocess.run(
        ["cargo", "metadata", "--format-version", "1"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        raise SystemExit("check_deps: `cargo metadata` failed")

    meta = json.loads(proc.stdout)
    pkgs = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    ws_ids = set(meta["workspace_members"])

    def normal_transitive_deps(root_id: str) -> set[str]:
        seen: set[str] = set()
        stack = [root_id]
        result: set[str] = set()
        while stack:
            cur = stack.pop()
            if cur in seen:
                continue
            seen.add(cur)
            if cur != root_id:
                result.add(pkgs[cur]["name"])
            node = nodes.get(cur)
            if not node:
                continue
            for dep in node["deps"]:
                if any(k["kind"] in (None, "normal") for k in dep["dep_kinds"]):
                    stack.append(dep["pkg"])
        return result

    problems: list[str] = []
    checked = 0
    for pkg_id in sorted(ws_ids, key=lambda pid: pkgs[pid]["name"]):
        pkg = pkgs[pkg_id]
        name = pkg["name"]
        if not name.startswith("prism-"):
            continue
        checked += 1

        direct_prism = sorted(
            d["name"]
            for d in pkg["dependencies"]
            if d["name"].startswith("prism-") and d.get("kind") in (None, "normal")
        )
        transitive = normal_transitive_deps(pkg_id)

        ui_hits = sorted(UI_CRATES & transitive)
        if ui_hits:
            problems.append(f"`{name}` violates UI-free: must not pull in {ui_hits}")

        if name in FOUNDATION_CRATES:
            allowed = {"prism-reflect-derive"} if name == "prism-reflect" else set()
            bad = [d for d in direct_prism if d not in allowed]
            if bad:
                problems.append(f"Base layer `{name}` must not depend on other prism crates: {bad}")

        if name in DATA_SCHEMA_CRATES:
            bad = [d for d in direct_prism if d in UPPER_CRATES]
            if bad:
                problems.append(f"Pure data/schema layer `{name}` must not depend on upper layers: {bad}")

        if name == "prism-render":
            forbidden = {
                "prism-ecs",
                "prism-scene",
                "prism-engine",
                "prism-app",
                "prism-cooker",
                "prism-editor-api",
                "prism-editor-mcp",
                "prism-game-api",
                "prism-runtime-host",
                "prism-build-pipeline",
            }
            bad = [d for d in direct_prism if d in forbidden]
            if bad:
                problems.append(f"`prism-render` must stay ECS/Engine-free: must not depend on {bad}")

        if name == "prism-runtime-host":
            forbidden = {
                "prism-engine",
                "prism-render",
                "prism-app",
                "prism-cooker",
                "prism-editor-api",
                "prism-editor-mcp",
                "prism-build-pipeline",
            }
            bad = [d for d in direct_prism if d in forbidden]
            if bad:
                problems.append(f"`prism-runtime-host` must not depend on engine or render layers: {bad}")

        if name in {"prism-engine", "prism-game-api"}:
            forbidden = {
                "prism-app",
                "prism-cooker",
                "prism-build-pipeline",
                "prism-editor-api",
                "prism-editor-mcp",
            }
            bad = [d for d in direct_prism if d in forbidden]
            if bad:
                problems.append(f"`{name}` must not depend on app or editor/build tools: {bad}")

        if name == "prism-app":
            forbidden = {"prism-cooker", "prism-build-pipeline", "prism-editor-api", "prism-editor-mcp"}
            bad = [d for d in direct_prism if d in forbidden]
            if bad:
                problems.append(f"`prism-app` must not depend on build-time or editor crates: {bad}")

        if name in VULKAN_FREE_CRATES:
            bad = sorted(VULKAN_DEPS & transitive)
            if bad:
                problems.append(f"`{name}` must stay Vulkan/Engine-free: must not pull in {bad}")

    return problems, checked


def main() -> int:
    parser = argparse.ArgumentParser(description="Prisma dependency & layer guard")
    parser.add_argument("--list", action="store_true", help="Print player dependency tree")
    parser.add_argument("--layers", action="store_true", help="Only check whole-repo crate layering")
    args = parser.parse_args()

    if args.list:
        print("\n".join(player_tree()))
        return 0

    all_problems: list[str] = []
    notes: list[str] = []

    if not args.layers:
        budget_problems, total = check_player_budget()
        all_problems.extend(budget_problems)
        if not budget_problems:
            notes.append(f"player {total}/{MAX_PLAYER_CRATES} crates")

    layer_problems, checked = check_layers()
    all_problems.extend(layer_problems)
    if not layer_problems:
        notes.append(f"{checked} workspace crates layer-compliant")

    if all_problems:
        for problem in all_problems:
            sys.stderr.write(f"check_deps: {problem}\n")
        sys.stderr.write(
            "check_deps: see scripts/check_deps.py header for fixes"
            "(split feature / move up or to build time / keep layering one-way)\n"
        )
        print("| Dependency budget | ❌ | " + "; ".join(all_problems) + " |")
        return 1

    print(f"| Dependency budget | ✅ | {', '.join(notes)}, UI-free |")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
