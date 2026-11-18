#!/usr/bin/env python3
"""Compare two `PRISMA_BENCH_REPORT` JSON files and emit a PR Markdown table.

    python scripts/bench_compare.py baseline.json candidate.json
    python scripts/bench_compare.py baseline.json candidate.json --max-p95-regress-pct 8

Exits 0 when within regression thresholds, 1 on regression or invalid input.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
from lib.console import force_utf8_streams  # noqa: E402

force_utf8_streams()

# (json_key, display_label, unit, higher_is_better, threshold_arg_name)
METRICS: tuple[tuple[str, str, str, bool, str | None], ...] = (
    ("meanMs", "mean", "ms", False, None),
    ("p50Ms", "P50", "ms", False, None),
    ("p95Ms", "P95", "ms", False, "max_p95_regress_pct"),
    ("p99Ms", "P99", "ms", False, "max_p99_regress_pct"),
    ("low1PctFps", "1% low", "FPS", True, "max_low1_drop_pct"),
    ("low0_1PctFps", "0.1% low", "FPS", True, None),
)


def load_report(path: Path) -> dict[str, Any]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("format") != "prism-bench-report":
        raise ValueError(f"{path}: expected format 'prism-bench-report', got {data.get('format')!r}")
    return data


def pct_change(baseline: float, candidate: float) -> float:
    if baseline == 0.0:
        return 0.0 if candidate == 0.0 else 100.0
    return ((candidate - baseline) / baseline) * 100.0


def evaluate_series(
    series_name: str,
    base_series: dict[str, Any],
    cand_series: dict[str, Any],
    thresholds: dict[str, float],
) -> tuple[list[dict[str, Any]], list[str]]:
    rows: list[dict[str, Any]] = []
    regressions: list[str] = []

    for key, label, unit, higher_is_better, threshold_key in METRICS:
        if key not in base_series or key not in cand_series:
            continue
        base_val = float(base_series[key])
        cand_val = float(cand_series[key])
        delta_pct = pct_change(base_val, cand_val)

        regressed = False
        if threshold_key is not None:
            limit = thresholds[threshold_key]
            if higher_is_better and delta_pct < -limit:
                regressed = True
            elif not higher_is_better and delta_pct > limit:
                regressed = True

        if regressed:
            regressions.append(f"{series_name} {label}: {base_val:.3f} -> {cand_val:.3f} {unit} ({delta_pct:+.1f}%)")

        rows.append(
            {
                "series": series_name,
                "label": label,
                "unit": unit,
                "base": base_val,
                "cand": cand_val,
                "delta_pct": delta_pct,
                "higher_is_better": higher_is_better,
                "regressed": regressed,
            }
        )

    return rows, regressions


def compare_reports(
    baseline: dict[str, Any],
    candidate: dict[str, Any],
    thresholds: dict[str, float],
) -> tuple[str, list[str]]:
    warnings: list[str] = []
    for meta_key in ("adapter", "extent", "scene"):
        b_val = baseline.get(meta_key)
        c_val = candidate.get(meta_key)
        if b_val and c_val and b_val != c_val:
            warnings.append(f"{meta_key} differs: baseline={b_val!r} vs candidate={c_val!r}")

    all_rows: list[dict[str, Any]] = []
    regressions: list[str] = []
    for series_key in ("cpu", "gpu"):
        b_series = baseline.get(series_key)
        c_series = candidate.get(series_key)
        if isinstance(b_series, dict) and isinstance(c_series, dict):
            rows, regs = evaluate_series(series_key.upper(), b_series, c_series, thresholds)
            all_rows.extend(rows)
            regressions.extend(regs)

    if not all_rows:
        raise ValueError("neither report contains comparable 'cpu' or 'gpu' series")

    lines = ["## Frame-time comparison (`PRISMA_BENCH_REPORT`)", ""]
    meta_parts = []
    if candidate.get("adapter") or baseline.get("adapter"):
        meta_parts.append(f"adapter: `{candidate.get('adapter') or baseline.get('adapter')}`")
    if candidate.get("extent") or baseline.get("extent"):
        meta_parts.append(f"extent: `{candidate.get('extent') or baseline.get('extent')}`")
    if meta_parts:
        lines.append("- " + " · ".join(meta_parts))
    for warn in warnings:
        lines.append(f"- ⚠️ {warn}")
    if meta_parts or warnings:
        lines.append("")

    lines.append("| Series | Metric | Baseline | Candidate | Delta | Status |")
    lines.append("|---|---|---|---|---|---|")
    for row in all_rows:
        fmt = ".1f" if row["unit"] == "FPS" else ".3f"
        status = "❌ regressed" if row["regressed"] else "✅"
        lines.append(
            f"| {row['series']} | {row['label']} | "
            f"{row['base']:{fmt}} {row['unit']} | "
            f"{row['cand']:{fmt}} {row['unit']} | "
            f"{row['delta_pct']:+.1f}% | {status} |"
        )

    lines.append("")
    if regressions:
        lines.append(f"**Result: ❌ {len(regressions)} regression(s) detected**")
    else:
        lines.append("**Result: ✅ within thresholds**")

    return "\n".join(lines), regressions


def main() -> int:
    parser = argparse.ArgumentParser(description="Compare two PRISMA_BENCH_REPORT JSON files")
    parser.add_argument("baseline", type=Path, help="baseline report JSON")
    parser.add_argument("candidate", type=Path, help="candidate report JSON")
    parser.add_argument("--max-p95-regress-pct", type=float, default=10.0)
    parser.add_argument("--max-p99-regress-pct", type=float, default=15.0)
    parser.add_argument("--max-low1-drop-pct", type=float, default=10.0)
    args = parser.parse_args()

    try:
        baseline = load_report(args.baseline)
        candidate = load_report(args.candidate)
        markdown, regressions = compare_reports(
            baseline,
            candidate,
            {
                "max_p95_regress_pct": args.max_p95_regress_pct,
                "max_p99_regress_pct": args.max_p99_regress_pct,
                "max_low1_drop_pct": args.max_low1_drop_pct,
            },
        )
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        sys.stderr.write(f"bench_compare: {exc}\n")
        return 1

    print(markdown)
    return 1 if regressions else 0


if __name__ == "__main__":
    raise SystemExit(main())
