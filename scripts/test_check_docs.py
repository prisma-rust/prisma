import tempfile
import unittest
from pathlib import Path

from scripts.check_docs import check_docs


class CheckDocsTests(unittest.TestCase):
    def test_valid_kebab_and_registered_passes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            docs = Path(tmp)
            (docs / "README.md").write_text("- [render-test](render-test.md)\n", encoding="utf-8")
            (docs / "render-test.md").write_text("# Test\n", encoding="utf-8")
            self.assertEqual(check_docs(docs), [])

    def test_unregistered_or_bad_name_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            docs = Path(tmp)
            (docs / "README.md").write_text("# Index\n", encoding="utf-8")
            (docs / "BadName.md").write_text("# Bad\n", encoding="utf-8")
            errs = check_docs(docs)
            self.assertEqual(len(errs), 2)


if __name__ == "__main__":
    unittest.main()
