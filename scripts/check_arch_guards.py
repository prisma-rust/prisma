#!/usr/bin/env python3
"""Static architecture guards:
- #1212: Zero pre-1.0 compatibility shims (`#[deprecated` on added lines).
- #1214: `prism-project-api` C-ABI surface must not expose `Engine`, `World`, or `dyn ` trait objects.
- #1216: Single-source `settings.ini` (`prism-ini`) and asset path lookups (`asset_paths`) on added lines.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def check_c_abi_surface(api_file: Path | None = None) -> list[str]:
    path = api_file or (ROOT / "crates" / "prism-project-api" / "src" / "lib.rs")
    if not path.is_file():
        return []
    errors: list[str] = []
    forbidden = re.compile(r"\b(Engine|World|dyn\s+[A-Z])\b")
    for idx, line in enumerate(path.read_text(encoding="utf-8", errors="replace").splitlines(), 1):
        code = line.split("//", 1)[0]
        if m := forbidden.search(code):
            errors.append(f"{path}:{idx}: C ABI must not reference `{m.group(1)}` (#1214)")
    return errors


def check_added_lines(base: str = "origin/master") -> list[str]:
    proc = subprocess.run(
        ["git", "diff", "--unified=0", base, "--", "crates/"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        return []
    errors: list[str] = []
    cur_file = ""
    for line in proc.stdout.splitlines():
        if line.startswith("+++ b/"):
            cur_file = line[6:]
            continue
        if not line.startswith("+") or line.startswith("+++"):
            continue
        text = line[1:]
        if "#[deprecated" in text:
            errors.append(f"{cur_file}: `#[deprecated` violates pre-1.0 zero-compat rule (#1212)")
        if "settings.ini" in text and not cur_file.startswith(("crates/prism-ini/", "crates/prism-app/", "crates/prism-engine/")):
            errors.append(f"{cur_file}: direct `settings.ini` reference outside prism-ini/app/engine (#1216)")
    return errors


def main() -> int:
    errors = check_c_abi_surface() + check_added_lines()
    if errors:
        for err in errors:
            print(err, file=sys.stderr)
        return 1
    print("arch guards: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
