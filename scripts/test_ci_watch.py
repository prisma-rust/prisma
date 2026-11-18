import unittest

from scripts.ci_watch import all_checks_completed, summarize_checks


class CiWatchTests(unittest.TestCase):
    def test_all_checks_completed_requires_non_empty_and_completed(self) -> None:
        self.assertFalse(all_checks_completed([]))
        self.assertFalse(
            all_checks_completed([{"name": "ci", "status": "in_progress", "conclusion": None}])
        )
        self.assertTrue(
            all_checks_completed([{"name": "ci", "status": "completed", "conclusion": "success"}])
        )

    def test_summarize_checks_treats_skipped_as_ok_and_failure_as_not_ok(self) -> None:
        ok, lines = summarize_checks(
            [
                {"name": "dist", "status": "completed", "conclusion": "skipped"},
                {"name": "lint", "status": "completed", "conclusion": "success"},
            ]
        )
        self.assertTrue(ok)
        self.assertEqual(len(lines), 2)

        ok_fail, _ = summarize_checks(
            [{"name": "desktop", "status": "completed", "conclusion": "failure"}]
        )
        self.assertFalse(ok_fail)


if __name__ == "__main__":
    unittest.main()
