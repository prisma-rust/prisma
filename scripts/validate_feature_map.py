#!/usr/bin/env python3
"""Validate `docs/feature-map.json` against editor source definitions.

Ensures panel tab IDs, Command enum names, and node ID prefixes stay aligned
with `editor/crates/prism-editor-egui` and `editor/crates/prism-editor-core`.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FEATURE_MAP_PATH = ROOT / "docs" / "feature-map.json"
APP_RS = ROOT / "editor" / "crates" / "prism-editor-egui" / "src" / "app.rs"
KEYS_RS = ROOT / "editor" / "crates" / "prism-editor-core" / "src" / "keys.rs"
UI_TREE_RS = ROOT / "editor" / "crates" / "prism-editor-egui" / "src" / "ui_tree.rs"


def parse_tabs(app_source: str) -> dict[str, str]:
    match = re.search(r"fn tab_title\([^)]*\)\s*->\s*&str\s*\{(?P<body>.*?)\n\}", app_source, re.S)
    if not match:
        raise ValueError("could not locate `fn tab_title` in app.rs")
    tabs: dict[str, str] = {}
    for arm in re.finditer(r'"([^"]+)"\s*=>\s*"([^"]+)"', match.group("body")):
        tabs[arm.group(1)] = arm.group(2)
    return tabs


def parse_commands(keys_source: str) -> set[str]:
    match = re.search(r"pub enum Command\s*\{(?P<body>.*?)\n\}", keys_source, re.S)
    if not match:
        raise ValueError("could not locate `pub enum Command` in keys.rs")
    commands: set[str] = set()
    for line in match.group("body").splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("//") or stripped.startswith("#"):
            continue
        ident = stripped.rstrip(",").split("(")[0].split("{")[0].strip()
        if re.fullmatch(r"[A-Z][A-Za-z0-9]+", ident):
            commands.add(ident)
    return commands


def validate(
    feature_map: dict,
    app_source: str,
    keys_source: str,
    ui_tree_source: str,
) -> list[str]:
    errors: list[str] = []
    actual_tabs = parse_tabs(app_source)
    actual_commands = parse_commands(keys_source)

    for tab_id, title in feature_map.get("panels", {}).items():
        if tab_id not in actual_tabs:
            errors.append(f"unknown panel tab `{tab_id}` (not in app.rs `tab_title`)")
        elif actual_tabs[tab_id] != title:
            errors.append(
                f"panel `{tab_id}` title mismatch: map={title!r} vs app.rs={actual_tabs[tab_id]!r}"
            )

    for cmd in feature_map.get("coreCommands", []):
        if cmd not in actual_commands:
            errors.append(f"unknown command `{cmd}` (not in keys.rs `Command`)")

    for prefix in feature_map.get("nodePrefixes", []):
        head = prefix.split(":", 1)[0] + ":"
        if f"`{head}" not in ui_tree_source and f'"{head}' not in ui_tree_source:
            errors.append(f"node prefix `{prefix}` not documented/emitted in ui_tree.rs")

    for flow in feature_map.get("flows", []):
        for step in flow.get("steps", []):
            node = step.get("node")
            if not isinstance(node, str):
                continue
            if node.startswith("panel:"):
                tab = node.split(":", 1)[1]
                if tab not in actual_tabs:
                    errors.append(f"flow `{flow['id']}` references unknown panel `{tab}`")
            elif node.startswith("command:"):
                cmd = node.split(":", 1)[1]
                if cmd not in actual_commands:
                    errors.append(f"flow `{flow['id']}` references unknown command `{cmd}`")

    return errors


def main() -> int:
    feature_map = json.loads(FEATURE_MAP_PATH.read_text(encoding="utf-8"))
    errors = validate(
        feature_map,
        APP_RS.read_text(encoding="utf-8"),
        KEYS_RS.read_text(encoding="utf-8"),
        UI_TREE_RS.read_text(encoding="utf-8"),
    )
    if errors:
        for err in errors:
            sys.stderr.write(f"feature_map: {err}\n")
        return 1
    print(
        f"feature_map: ok ({len(feature_map.get('panels', {}))} panels, "
        f"{len(feature_map.get('coreCommands', []))} core commands, "
        f"{len(feature_map.get('flows', []))} flows)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
