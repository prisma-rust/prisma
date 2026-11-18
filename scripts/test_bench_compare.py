import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import bench_compare


def sample_report(
    p95: float = 8.0,
    p99: float = 10.0,
    low1: float = 90.0,
    adapter: str = "Test GPU",
) -> dict:
    return {
        "format": "prism-bench-report",
        "version": 1,
        "adapter": adapter,
        "extent": "1280x720",
        "cpu": {
            "samples": 600,
            "meanMs": 6.5,
            "p50Ms": 6.2,
            "p95Ms": p95,
            "p99Ms": p99,
            "p999Ms": 12.0,
            "low1PctFps": low1,
            "low0_1PctFps": 75.0,
        },
        "gpu": None,
    }


class BenchCompareTests(unittest.TestCase):
    def test_within_thresholds_passes(self) -> None:
        md, regressions = bench_compare.compare_reports(
            sample_report(p95=8.0, p99=10.0, low1=90.0),
            sample_report(p95=8.4, p99=10.5, low1=88.0),
            {"max_p95_regress_pct": 10.0, "max_p99_regress_pct": 15.0, "max_low1_drop_pct": 10.0},
        )
        self.assertEqual(regressions, [])
        self.assertIn("✅ within thresholds", md)

    def test_p95_and_low1_regression_detected(self) -> None:
        md, regressions = bench_compare.compare_reports(
            sample_report(p95=8.0, p99=10.0, low1=90.0),
            sample_report(p95=10.0, p99=10.5, low1=70.0),
            {"max_p95_regress_pct": 10.0, "max_p99_regress_pct": 15.0, "max_low1_drop_pct": 10.0},
        )
        self.assertEqual(len(regressions), 2)
        self.assertIn("❌ regressed", md)


if __name__ == "__main__":
    unittest.main()
