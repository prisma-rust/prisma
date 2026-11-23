#!/usr/bin/env python3
"""Subsystem size ledger: build player binary per feature combo, measure how many bytes each "brick" costs.

    python scripts/size_report.py            # run default combo table (incremental build, fast if already built)
    python scripts/size_report.py --release=false

Method: temporarily rewrite `projects/template/Cargo.toml` feature list → `cargo build -p template`
→ record binary size → restore file. Use the **same** project and profile, so differences come only from features.

⚠️ This is **unstripped** release binary. Ratios differ after strip (symbol tables removed),
so only compare within the same table, do not compare numbers across tables.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PROJECT = ROOT / "projects" / "template"

APP_DEP = 'prism-app = { path = "../../crates/prism-app", optional = true }'
ENGINE_DEP = 'prism-engine = { path = "../../crates/prism-engine", optional = true }'

# (tag, enabled prism-app features). Baseline is "all off".
COMBOS: list[tuple[str, list[str]]] = [
    ("base (all off)", []),
    ("pathtrace", ["pathtrace"]),
    ("audio", ["audio"]),
    ("physics", ["physics"]),
    ("audio + pathtrace", ["audio", "pathtrace"]),
    ("all (audio+pathtrace+physics)", ["audio", "pathtrace", "physics"]),
]


def build(features: list[str]) -> int:
    original = (PROJECT / "Cargo.toml").read_text(encoding="utf-8")
    try:
        feat = ", ".join(f'"{f}"' for f in features)
        patched = original.replace(
            APP_DEP,
            f'prism-app = {{ path = "../../crates/prism-app", optional = true, '
            f'default-features = false, features = [{feat}] }}',
        ).replace(
            ENGINE_DEP,
            'prism-engine = { path = "../../crates/prism-engine", optional = true, '
            'default-features = false }',
        )
        if patched == original:
            raise SystemExit("size_report: cannot find template prism-app/prism-engine dependency line")
        (PROJECT / "Cargo.toml").write_text(patched, encoding="utf-8")

        proc = subprocess.run(
            ["cargo", "build", "-p", "template", "--release"],
            cwd=PROJECT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        if proc.returncode != 0:
            sys.stderr.write(proc.stderr[-3000:])
            raise SystemExit("size_report: build failed")
        return (PROJECT / "target" / "release" / "template").stat().st_size
    finally:
        (PROJECT / "Cargo.toml").write_text(original, encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description="Prisma subsystem binary-size ledger")
    parser.add_argument("--json", action="store_true", help="output JSON only")
    args = parser.parse_args()

    sizes = {label: build(feats) for label, feats in COMBOS}
    base = sizes[COMBOS[0][0]]

    if args.json:
        print(json.dumps(sizes, ensure_ascii=False, indent=2))
        return 0

    print("| Combo | Binary | vs base |")
    print("|---|---|---|")
    for label, size in sizes.items():
        delta = size - base
        sign = "" if delta == 0 else f"+{delta:,}" if delta > 0 else f"{delta:,}"
        print(f"| {label} | {size / 1e6:.2f} MB | {sign} |")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
