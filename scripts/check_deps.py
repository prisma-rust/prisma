#!/usr/bin/env python3
"""Prisma 依赖守护：玩家依赖预算 + 全仓 crate 分层边界矩阵。

    python scripts/check_deps.py            # 完整检查（玩家预算 + 层级矩阵）
    python scripts/check_deps.py --layers   # 仅检查全仓 crate 分层边界
    python scripts/check_deps.py --list     # 打印整棵玩家依赖树

规则（改动本文件时同步更新 `docs/DESIGN.md` §11）：
  1. 玩家树（`prism-app`）不得出现 `BANNED` 中的构建期/可选能力 crate；
  2. 玩家树 crate 总数不得超过 `MAX_PLAYER_CRATES`；
  3. 根 workspace 所有 `prism-*` crate 必须保持 UI-free（禁止引入 egui 等 UI 框架）；
  4. 各层级 crate 禁止逆向或跨边界依赖（见 `LAYER_RULES`）。

stdout 是给 PR 贴的 Markdown 行，进度/诊断走 stderr。
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

# 允许的玩家树上限。留一点余量，但**不要**为了过 CI 无脑调高：
# 要么把新依赖做成 feature，要么在 PR 里说明为什么它必须随玩家发布。
MAX_PLAYER_CRATES = 155

# 构建期 / 编辑器 / 可选能力：出现即失败。
BANNED: dict[str, str] = {
    "gltf": "资产导入只在 prism-cooker（构建期）",
    "gltf-json": "资产导入只在 prism-cooker（构建期）",
    "clap": "CLI 参数解析只在构建工具",
    "rayon": "构建期并行，运行时不该有",
    "exr": "EXR 解码只在 cooker 导入",
    "jpeg-decoder": "JPEG 解码只在 cooker 导入",
    "tiff": "TIFF 解码只在 cooker 导入",
    "tokio": "异步 IO 是 prism-asset 的 `async-io` feature，默认关闭",
    "rapier3d": "物理是 prism-app 的 `physics` feature，默认关闭",
    "parry3d": "物理是 prism-app 的 `physics` feature，默认关闭",
    "cpal": "音频后端是 prism-app 的 `audio` feature，默认关闭",
    "firewheel": "音频后端是 prism-app 的 `audio` feature，默认关闭",
    "symphonia": "音频解码是 prism-app 的 `audio` feature，默认关闭",
    "symphonium": "音频解码是 prism-app 的 `audio` feature，默认关闭",
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
    """解析 `cargo tree -p prism-app`，返回唯一的 crate 名列表。"""
    proc = subprocess.run(
        ["cargo", "tree", "-p", PLAYER_CRATE, "-e", "normal", "--prefix", "none"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        raise SystemExit("check_deps: `cargo tree` 失败")

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
        problems.append(f"`{name}` 不该出现在玩家树：{reason}")
    if total > MAX_PLAYER_CRATES:
        problems.append(f"玩家依赖 {total} 个 crate，超过预算 {MAX_PLAYER_CRATES}")
    return problems, total


def check_layers() -> tuple[list[str], int]:
    """通过 `cargo metadata` 检查全仓 crate 分层边界与 UI-free 约束。"""
    proc = subprocess.run(
        ["cargo", "metadata", "--format-version", "1"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        raise SystemExit("check_deps: `cargo metadata` 失败")

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
            problems.append(f"`{name}` 违反 UI-free 约束：不得引入 {ui_hits}")

        if name in FOUNDATION_CRATES:
            allowed = {"prism-reflect-derive"} if name == "prism-reflect" else set()
            bad = [d for d in direct_prism if d not in allowed]
            if bad:
                problems.append(f"基础层 `{name}` 不得依赖其他 prism crate：{bad}")

        if name in DATA_SCHEMA_CRATES:
            bad = [d for d in direct_prism if d in UPPER_CRATES]
            if bad:
                problems.append(f"纯数据/schema 层 `{name}` 不得依赖上层 crate：{bad}")

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
                problems.append(f"`prism-render` 必须保持 ECS/Engine-free：不得依赖 {bad}")

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
                problems.append(f"`prism-runtime-host` 不得依赖引擎或渲染层：{bad}")

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
                problems.append(f"`{name}` 不得依赖应用层或编辑器/构建工具：{bad}")

        if name == "prism-app":
            forbidden = {"prism-cooker", "prism-build-pipeline", "prism-editor-api", "prism-editor-mcp"}
            bad = [d for d in direct_prism if d in forbidden]
            if bad:
                problems.append(f"`prism-app` 不得依赖构建期或编辑器 crate：{bad}")

        if name in VULKAN_FREE_CRATES:
            bad = sorted(VULKAN_DEPS & transitive)
            if bad:
                problems.append(f"`{name}` 必须保持 Vulkan/Engine-free：不得引入 {bad}")

    return problems, checked


def main() -> int:
    parser = argparse.ArgumentParser(description="Prisma dependency & layer guard")
    parser.add_argument("--list", action="store_true", help="打印玩家依赖树")
    parser.add_argument("--layers", action="store_true", help="仅检查全仓 crate 分层边界")
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
        notes.append(f"{checked} workspace crates 分层合规")

    if all_problems:
        for problem in all_problems:
            sys.stderr.write(f"check_deps: {problem}\n")
        sys.stderr.write(
            "check_deps: 修复方式见 scripts/check_deps.py 顶部说明"
            "（拆 feature / 移至上层或构建期 / 保持分层单向）\n"
        )
        print("| 依赖预算 | ❌ | " + "；".join(all_problems) + " |")
        return 1

    print(f"| 依赖预算 | ✅ | {'，'.join(notes)}，UI-free |")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
