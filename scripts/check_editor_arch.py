#!/usr/bin/env python3
"""Static architecture guards for `projects/editor/crates/editor-app`.

Enforces the Pit of Success boundaries:
1. `panels/` must not import engine/platform internals (`prism_engine`, `winit`, `ash`)
   or take `&mut EditorDocument` / `&mut EditorSession`.
2. `widgets.rs` must not depend on `prism_engine` or `prism_app`.
3. No production source file in `editor-app/src/` grows into a monolith (> 600 lines).
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EDITOR_SRC = ROOT / "projects" / "editor" / "crates" / "editor-app" / "src"

FORBIDDEN_IN_PANELS = re.compile(
    r"\b(prism_engine|prism_app|winit|ash)\b|&mut\s+(EditorDocument|EditorSession)\b"
)
FORBIDDEN_IN_WIDGETS = re.compile(r"\b(prism_engine|prism_app)\b")
MAX_FILE_LINES = 600


def check_editor_arch(src_root: Path = EDITOR_SRC) -> list[str]:
    if not src_root.is_dir():
        return []
    errors: list[str] = []

    panels_dir = src_root / "panels"
    if panels_dir.is_dir():
        for rs in sorted(panels_dir.glob("*.rs")):
            text = rs.read_text(encoding="utf-8", errors="replace")
            for idx, line in enumerate(text.splitlines(), 1):
                code = line.split("//", 1)[0]
                if m := FORBIDDEN_IN_PANELS.search(code):
                    errors.append(
                        f"{rs}:{idx}: panel layer must not reference `{m.group(0)}`"
                    )

    widgets_file = src_root / "widgets.rs"
    if widgets_file.is_file():
        text = widgets_file.read_text(encoding="utf-8", errors="replace")
        for idx, line in enumerate(text.splitlines(), 1):
            code = line.split("//", 1)[0]
            if m := FORBIDDEN_IN_WIDGETS.search(code):
                errors.append(
                    f"{widgets_file}:{idx}: widgets layer must not reference `{m.group(0)}`"
                )

    for rs in sorted(src_root.rglob("*.rs")):
        line_count = len(rs.read_text(encoding="utf-8", errors="replace").splitlines())
        if line_count > MAX_FILE_LINES:
            errors.append(
                f"{rs}: {line_count} lines exceeds {MAX_FILE_LINES}-line modularity cap"
            )

    return errors


def main() -> int:
    errors = check_editor_arch()
    if errors:
        for err in errors:
            print(err, file=sys.stderr)
        return 1
    print("editor_arch: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
